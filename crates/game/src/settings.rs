//! Persistent local user preferences (FOV, fullscreen, audio devices & sliders, identity)
//! and top-level `AppScreen` state (`MainMenu` vs `InGame`).

use std::{
    fs,
    path::{Path, PathBuf},
};

use bevy::{
    prelude::*,
    window::{MonitorSelection, PrimaryWindow, WindowMode},
};
use serde::{Deserialize, Serialize};

pub const FOV_MIN_DEG: f32 = 60.0;
pub const FOV_MAX_DEG: f32 = 110.0;
pub const DEFAULT_FOV_DEG: f32 = 75.0;

/// Top-level application screen controlling Main Menu vs live Shed/Cave gameplay.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppScreen {
    #[default]
    MainMenu,
    InGame,
}

/// Whether the unified Video & Audio Settings panel is currently open.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct SettingsMenuOpen(pub bool);

fn default_include_prereleases() -> bool {
    true
}

/// Local player preferences persisted to `saves/settings.json`.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserSettings {
    pub player_name: String,
    pub visor_idx: usize,
    pub fov_deg: f32,
    /// Regular borderless fullscreen toggle (also bound globally to `[F11]`).
    #[serde(default)]
    pub fullscreen: bool,
    pub master_volume: f32,
    pub voice_volume: f32,
    pub mic_gain: f32,
    pub gate_threshold_db: f32,
    pub output_device: Option<String>,
    pub input_device: Option<String>,
    /// Whether the auto-updater includes GitHub pre-releases (`v0.1.x` drops).
    #[serde(default = "default_include_prereleases")]
    pub include_prereleases: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            player_name: "Miner".to_string(),
            visor_idx: 0,
            fov_deg: DEFAULT_FOV_DEG,
            fullscreen: false,
            master_volume: 1.0,
            voice_volume: 1.0,
            mic_gain: 1.0,
            gate_threshold_db: -45.0,
            output_device: None,
            input_device: None,
            include_prereleases: default_include_prereleases(),
        }
    }
}

impl UserSettings {
    /// Portable settings location inside the game folder (separate from `saves/`).
    pub fn default_path() -> PathBuf {
        PathBuf::from("config").join("settings.json")
    }

    /// Legacy settings location from `v0.1.0`–`v0.1.3` (auto-migrated on first launch).
    pub fn legacy_path() -> PathBuf {
        PathBuf::from("saves").join("settings.json")
    }

    pub fn sanitize(&mut self) {
        let trimmed = self.player_name.trim();
        if trimmed.is_empty() {
            self.player_name = "Miner".to_string();
        } else {
            self.player_name = trimmed.chars().take(20).collect();
        }
        self.fov_deg = if self.fov_deg.is_finite() {
            self.fov_deg.clamp(FOV_MIN_DEG, FOV_MAX_DEG)
        } else {
            DEFAULT_FOV_DEG
        };
        self.master_volume = if self.master_volume.is_finite() {
            self.master_volume.clamp(0.0, 1.5)
        } else {
            1.0
        };
        self.voice_volume = if self.voice_volume.is_finite() {
            self.voice_volume.clamp(0.0, 2.0)
        } else {
            1.0
        };
        self.mic_gain = if self.mic_gain.is_finite() {
            self.mic_gain.clamp(0.2, 3.0)
        } else {
            1.0
        };
        self.gate_threshold_db = if self.gate_threshold_db.is_finite() {
            self.gate_threshold_db.clamp(-65.0, -15.0)
        } else {
            -45.0
        };
    }

    pub fn load_default() -> Self {
        Self::load_with_migration(&Self::default_path(), &Self::legacy_path())
    }

    /// Loads settings from `primary`, transparently migrating from `legacy` if `primary` does
    /// not yet exist.
    pub fn load_with_migration(primary: &Path, legacy: &Path) -> Self {
        if let Ok(settings) = Self::load_from_path(primary) {
            return settings;
        }
        if let Ok(migrated) = Self::load_from_path(legacy) {
            if migrated.save_to_path(primary).is_ok() {
                let _ = fs::remove_file(legacy);
            }
            return migrated;
        }
        Self::default()
    }

    pub fn load_from_path(path: &Path) -> Result<Self, String> {
        let raw = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read settings {}: {e}", path.display()))?;
        let mut parsed: Self = serde_json::from_str(&raw)
            .map_err(|e| format!("Failed to parse settings {}: {e}", path.display()))?;
        parsed.sanitize();
        Ok(parsed)
    }

    pub fn save_default(&self) -> Result<(), String> {
        self.save_to_path(&Self::default_path())
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create settings dir {}: {e}", parent.display()))?;
        }
        let mut clean = self.clone();
        clean.sanitize();
        let json = serde_json::to_string_pretty(&clean)
            .map_err(|e| format!("Failed to serialize settings: {e}"))?;
        fs::write(path, json)
            .map_err(|e| format!("Failed to write settings {}: {e}", path.display()))
    }
}

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        let loaded = UserSettings::load_default();
        app.insert_resource(AppScreen::MainMenu)
            .insert_resource(SettingsMenuOpen(false))
            .insert_resource(loaded)
            .add_systems(Update, apply_video_settings_system);
    }
}

fn apply_video_settings_system(
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mut settings: ResMut<UserSettings>,
    mut cam_q: Query<&mut Projection, With<Camera3d>>,
    mut win_q: Query<&mut Window, With<PrimaryWindow>>,
) {
    // Global `[F11]` fullscreen toggle (Minecraft-style instant borderless fullscreen)
    if keys
        .as_ref()
        .is_some_and(|k| k.just_pressed(KeyCode::F11))
    {
        settings.fullscreen = !settings.fullscreen;
        let _ = settings.save_default();
    }

    let target_fov = settings.fov_deg.clamp(FOV_MIN_DEG, FOV_MAX_DEG).to_radians();
    for mut proj in &mut cam_q {
        if let Projection::Perspective(ref mut persp) = *proj
            && (persp.fov - target_fov).abs() > 1e-4
        {
            persp.fov = target_fov;
        }
    }

    if let Ok(mut window) = win_q.single_mut() {
        let desired = if settings.fullscreen {
            WindowMode::BorderlessFullscreen(MonitorSelection::Current)
        } else {
            WindowMode::Windowed
        };
        if window.mode != desired {
            window.mode = desired;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn sanitize_clamps_ranges_and_roundtrips_json() {
        let mut s = UserSettings {
            player_name: "   ".into(),
            visor_idx: 3,
            fov_deg: 200.0,
            fullscreen: true,
            master_volume: -1.0,
            voice_volume: 9.0,
            mic_gain: 0.01,
            gate_threshold_db: 5.0,
            output_device: Some("Headphones".into()),
            input_device: Some("USB Mic".into()),
            include_prereleases: false,
        };
        s.sanitize();
        assert_eq!(s.player_name, "Miner");
        assert!((s.fov_deg - FOV_MAX_DEG).abs() < 1e-4);
        assert_eq!(s.master_volume, 0.0);
        assert_eq!(s.voice_volume, 2.0);
        assert!((s.mic_gain - 0.2).abs() < 1e-4);
        assert_eq!(s.gate_threshold_db, -15.0);

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir()
            .join(format!("echoes_below_settings_test_{nanos}"))
            .join("settings.json");

        s.save_to_path(&path).expect("save settings");
        let loaded = UserSettings::load_from_path(&path).expect("load settings");
        assert_eq!(loaded, s);

        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn migrates_legacy_settings_out_of_saves_into_config() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let root = std::env::temp_dir().join(format!("echoes_below_settings_mig_{nanos}"));
        let legacy = root.join("saves").join("settings.json");
        let primary = root.join("config").join("settings.json");

        let old = UserSettings {
            fov_deg: 95.0,
            fullscreen: true,
            ..Default::default()
        };
        old.save_to_path(&legacy).expect("write legacy settings");

        let loaded = UserSettings::load_with_migration(&primary, &legacy);
        assert_eq!(loaded.fov_deg, 95.0);
        assert!(loaded.fullscreen);
        assert!(primary.is_file(), "migrated config/settings.json must exist");
        assert!(!legacy.exists(), "legacy saves/settings.json should be cleaned up");

        let _ = fs::remove_dir_all(&root);
    }
}
