//! Bevy bridge for the cross-platform GitHub Releases auto-updater (`voxel-updater`).
//!
//! Spawns a detached background check on startup (never blocking startup or rendering),
//! exposes live download/verification/install progress for the Main Menu popup & Settings UI,
//! and safely triggers relaunch + graceful `AppExit` once an update is installed.

use std::sync::{
    Mutex,
    mpsc::{self, Receiver},
};

use bevy::{app::AppExit, prelude::*};
use voxel_updater::{
    APP_VERSION, UpdateInfo, UpdateProgress, check_for_updates, cleanup_stale_old_binaries,
    download_and_install_update, format_release_notes_preview, parse_version, spawn_relaunch,
};

use crate::settings::UserSettings;

/// Commands sent by the Main Menu popup or Settings UI to drive the background updater.
#[derive(Message, Clone, Copy, Debug)]
pub enum UpdaterCommand {
    /// Manually check GitHub Releases for updates (shows status feedback in Settings).
    CheckNow,
    /// Download, SHA-256 verify, swap binary + `assets/`, and relaunch.
    StartInstall,
    /// Dismiss the update popup for the remainder of this session.
    DismissPopup,
}

/// High-level state of the updater for UI rendering.
#[derive(Clone, Debug, Default)]
pub enum UpdaterUiStatus {
    #[default]
    Idle,
    Checking,
    UpToDate,
    UpdateAvailable(UpdateInfo),
    Downloading {
        info: UpdateInfo,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Verifying(UpdateInfo),
    Extracting(UpdateInfo),
    Installing(UpdateInfo),
    Restarting(UpdateInfo),
    Failed {
        info: Option<UpdateInfo>,
        message: String,
    },
}

enum WorkerEvent {
    CheckComplete {
        manual: bool,
        result: Result<Option<UpdateInfo>, String>,
    },
    InstallProgress {
        info: UpdateInfo,
        progress: UpdateProgress,
    },
}

/// Resource tracking live auto-updater status, popup visibility, and background worker channels.
#[derive(Resource)]
pub struct UpdaterState {
    pub status: UpdaterUiStatus,
    pub popup_open: bool,
    pub dismissed_for_session: bool,
    pub settings_status_text: String,
    rx: Mutex<Option<Receiver<WorkerEvent>>>,
}

impl Default for UpdaterState {
    fn default() -> Self {
        Self {
            status: UpdaterUiStatus::Idle,
            popup_open: false,
            dismissed_for_session: false,
            settings_status_text: format!("Installed: {APP_VERSION}"),
            rx: Mutex::new(None),
        }
    }
}

impl UpdaterState {
    pub fn active_update_info(&self) -> Option<&UpdateInfo> {
        match &self.status {
            UpdaterUiStatus::UpdateAvailable(info)
            | UpdaterUiStatus::Downloading { info, .. }
            | UpdaterUiStatus::Verifying(info)
            | UpdaterUiStatus::Extracting(info)
            | UpdaterUiStatus::Installing(info)
            | UpdaterUiStatus::Restarting(info) => Some(info),
            UpdaterUiStatus::Failed { info, .. } => info.as_ref(),
            _ => None,
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.status,
            UpdaterUiStatus::Checking
                | UpdaterUiStatus::Downloading { .. }
                | UpdaterUiStatus::Verifying(_)
                | UpdaterUiStatus::Extracting(_)
                | UpdaterUiStatus::Installing(_)
                | UpdaterUiStatus::Restarting(_)
        )
    }

    pub fn progress_fraction(&self) -> f32 {
        match &self.status {
            UpdaterUiStatus::Downloading {
                downloaded_bytes,
                total_bytes,
                ..
            } => match total_bytes {
                Some(total) if *total > 0 => {
                    ((*downloaded_bytes as f64 / *total as f64) as f32).clamp(0.0, 1.0) * 0.78
                }
                _ => 0.25,
            },
            UpdaterUiStatus::Verifying(_) => 0.84,
            UpdaterUiStatus::Extracting(_) => 0.90,
            UpdaterUiStatus::Installing(_) => 0.96,
            UpdaterUiStatus::Restarting(_) => 1.0,
            _ => 0.0,
        }
    }

    pub fn popup_status_line(&self) -> (String, bool) {
        match &self.status {
            UpdaterUiStatus::UpdateAvailable(_) => (String::new(), false),
            UpdaterUiStatus::Downloading {
                downloaded_bytes,
                total_bytes,
                ..
            } => {
                let pct = match total_bytes {
                    Some(total) if *total > 0 => format!(
                        "{:.0}%",
                        ((*downloaded_bytes as f64 / *total as f64) * 100.0).clamp(0.0, 100.0)
                    ),
                    _ => format!("{:.1} MB", *downloaded_bytes as f64 / (1024.0 * 1024.0)),
                };
                (format!("Downloading… {pct}"), false)
            }
            UpdaterUiStatus::Verifying(_) => ("Verifying checksum…".to_string(), false),
            UpdaterUiStatus::Extracting(_) => ("Extracting files…".to_string(), false),
            UpdaterUiStatus::Installing(_) => ("Installing update…".to_string(), false),
            UpdaterUiStatus::Restarting(_) => ("Restarting…".to_string(), false),
            UpdaterUiStatus::Failed { message, .. } => (message.clone(), true),
            _ => (String::new(), false),
        }
    }

    pub fn notes_preview(&self) -> String {
        match self.active_update_info() {
            Some(info) => {
                let preview = format_release_notes_preview(&info.release_notes, 340);
                if preview.is_empty() {
                    "New build ready to install.".to_string()
                } else {
                    preview
                }
            }
            None => String::new(),
        }
    }
}

pub struct UpdaterPlugin;

impl Plugin for UpdaterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UpdaterState>()
            .add_message::<UpdaterCommand>()
            .add_systems(Startup, startup_updater_check_system)
            .add_systems(
                Update,
                (handle_updater_commands_system, poll_updater_worker_system).chain(),
            );
    }
}

fn spawn_check_worker(state: &mut UpdaterState, include_prereleases: bool, manual: bool) {
    // Optional debug override so developers can inspect the Main Menu popup without mocking GitHub
    if let Ok(sim_tag) = std::env::var("ECHOES_SIMULATE_UPDATE") {
        let trimmed = sim_tag.trim();
        if !trimmed.is_empty()
            && let Some(parsed) = parse_version(trimmed)
        {
            let info = UpdateInfo {
                version_tag: trimmed.to_string(),
                parsed_version: parsed,
                prerelease: true,
                release_notes: "Simulated update preview via ECHOES_SIMULATE_UPDATE.\n- Tests Main Menu update popup & dev-build overwrite safety guard.".to_string(),
                html_url: "https://github.com/Pirs26/voxel/releases".to_string(),
                archive_name: voxel_updater::current_platform_asset_name().to_string(),
                archive_url: "https://example.invalid/simulated.zip".to_string(),
                archive_size_bytes: Some(24 * 1024 * 1024),
                checksums_url: Some("https://example.invalid/SHA256SUMS.txt".to_string()),
            };
            state.status = UpdaterUiStatus::UpdateAvailable(info);
            state.popup_open = true;
            state.settings_status_text = format!("Update {trimmed} available!");
            return;
        }
    }

    let (tx, rx) = mpsc::channel();
    if let Ok(mut slot) = state.rx.lock() {
        *slot = Some(rx);
    }
    state.status = UpdaterUiStatus::Checking;
    if manual {
        state.settings_status_text = "Checking GitHub Releases…".to_string();
    }

    let current = APP_VERSION.to_string();
    let _ = std::thread::Builder::new()
        .name("echoes-updater-check".to_string())
        .spawn(move || {
            let result = check_for_updates(&current, include_prereleases);
            let _ = tx.send(WorkerEvent::CheckComplete { manual, result });
        });
}

fn startup_updater_check_system(
    settings: Res<UserSettings>,
    mut updater: ResMut<UpdaterState>,
) {
    if let Ok(exe) = std::env::current_exe() {
        cleanup_stale_old_binaries(&exe);
    }

    // Never hit external network during `cargo test` headless runs
    if cfg!(test) {
        return;
    }

    spawn_check_worker(&mut updater, settings.include_prereleases, false);
}

fn handle_updater_commands_system(
    mut commands: MessageReader<UpdaterCommand>,
    settings: Res<UserSettings>,
    mut updater: ResMut<UpdaterState>,
) {
    for cmd in commands.read().copied() {
        match cmd {
            UpdaterCommand::CheckNow => {
                if updater.is_busy() {
                    continue;
                }
                updater.dismissed_for_session = false;
                spawn_check_worker(&mut updater, settings.include_prereleases, true);
            }
            UpdaterCommand::DismissPopup => {
                if matches!(
                    updater.status,
                    UpdaterUiStatus::Downloading { .. }
                        | UpdaterUiStatus::Verifying(_)
                        | UpdaterUiStatus::Extracting(_)
                        | UpdaterUiStatus::Installing(_)
                        | UpdaterUiStatus::Restarting(_)
                ) {
                    continue;
                }
                updater.popup_open = false;
                updater.dismissed_for_session = true;
            }
            UpdaterCommand::StartInstall => {
                if updater.is_busy() {
                    continue;
                }
                let Some(info) = updater.active_update_info().cloned() else {
                    continue;
                };
                let current_exe = match std::env::current_exe() {
                    Ok(p) => p,
                    Err(e) => {
                        updater.status = UpdaterUiStatus::Failed {
                            info: Some(info),
                            message: format!("Could not locate current executable: {e}"),
                        };
                        continue;
                    }
                };

                let (tx, rx) = mpsc::channel();
                if let Ok(mut slot) = updater.rx.lock() {
                    *slot = Some(rx);
                }
                updater.status = UpdaterUiStatus::Downloading {
                    info: info.clone(),
                    downloaded_bytes: 0,
                    total_bytes: info.archive_size_bytes,
                };
                updater.popup_open = true;

                let _ = std::thread::Builder::new()
                    .name("echoes-updater-install".to_string())
                    .spawn(move || {
                        let info_for_cb = info.clone();
                        let res = download_and_install_update(&info, &current_exe, |progress| {
                            let _ = tx.send(WorkerEvent::InstallProgress {
                                info: info_for_cb.clone(),
                                progress,
                            });
                        });
                        if let Err(err) = res {
                            let _ = tx.send(WorkerEvent::InstallProgress {
                                info,
                                progress: UpdateProgress::Failed(err),
                            });
                        }
                    });
            }
        }
    }
}

fn poll_updater_worker_system(
    mut updater: ResMut<UpdaterState>,
    mut exit_writer: MessageWriter<AppExit>,
) {
    let mut events = Vec::new();
    let mut disconnected = false;
    if let Ok(guard) = updater.rx.lock()
        && let Some(ref rx) = *guard
    {
        loop {
            match rx.try_recv() {
                Ok(ev) => events.push(ev),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
    }
    if disconnected && let Ok(mut guard) = updater.rx.lock() {
        *guard = None;
    }

    for ev in events {
        match ev {
            WorkerEvent::CheckComplete { manual, result } => match result {
                Ok(Some(info)) => {
                    let tag = info.version_tag.clone();
                    updater.status = UpdaterUiStatus::UpdateAvailable(info);
                    updater.settings_status_text = format!("Update {tag} available!");
                    if manual || !updater.dismissed_for_session {
                        updater.popup_open = true;
                    }
                }
                Ok(None) => {
                    updater.status = UpdaterUiStatus::UpToDate;
                    updater.settings_status_text = format!("Up to date ({APP_VERSION})");
                }
                Err(err) => {
                    debug!("Updater check failed: {err}");
                    if manual {
                        updater.status = UpdaterUiStatus::Failed {
                            info: None,
                            message: err.clone(),
                        };
                        updater.settings_status_text = format!("Check failed: {err}");
                    } else {
                        // Fail silently on automatic startup check (offline / rate-limited)
                        updater.status = UpdaterUiStatus::Idle;
                        updater.settings_status_text = format!("Installed: {APP_VERSION}");
                    }
                }
            },
            WorkerEvent::InstallProgress { info, progress } => match progress {
                UpdateProgress::Downloading {
                    downloaded_bytes,
                    total_bytes,
                } => {
                    updater.status = UpdaterUiStatus::Downloading {
                        info,
                        downloaded_bytes,
                        total_bytes,
                    };
                }
                UpdateProgress::Verifying => {
                    updater.status = UpdaterUiStatus::Verifying(info);
                }
                UpdateProgress::Extracting => {
                    updater.status = UpdaterUiStatus::Extracting(info);
                }
                UpdateProgress::Installing => {
                    updater.status = UpdaterUiStatus::Installing(info);
                }
                UpdateProgress::ReadyToRelaunch { exe_path } => {
                    updater.status = UpdaterUiStatus::Restarting(info.clone());
                    match spawn_relaunch(&exe_path) {
                        Ok(()) => {
                            exit_writer.write(AppExit::Success);
                        }
                        Err(err) => {
                            updater.status = UpdaterUiStatus::Failed {
                                info: Some(info),
                                message: format!(
                                    "Installed successfully, but auto-relaunch failed: {err}"
                                ),
                            };
                        }
                    }
                }
                UpdateProgress::Failed(message) => {
                    updater.status = UpdaterUiStatus::Failed {
                        info: Some(info),
                        message,
                    };
                }
            },
        }
    }
}
