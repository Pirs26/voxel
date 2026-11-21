use bevy::{
    picking::events::{PointerClick, PointerOut, PointerOver},
    prelude::*,
};
use voxel_acoustics::linear_to_db;

use crate::{
    audio_bridge::{
        AcousticBeaconState, AcousticTelemetry, RemoveVoiceSpeakerEvent, VoiceSettings,
    },
    net_bridge::{
        CoopMenuOpen, CoopState, VISOR_PALETTE, clear_remote_players, copy_to_clipboard,
        paste_from_clipboard,
    },
    player::PlayerFlashlight,
};

#[derive(Component)]
struct HudText;

#[derive(Component)]
struct CoopPanelRoot;

#[derive(Component)]
struct CoopPanelStatusText;

#[derive(Component, Clone, Copy, Debug)]
enum MenuAction {
    HostSession,
    CopyJoinCode,
    PasteAndJoin,
    Disconnect,
    CycleVisor,
    GateDown,
    GateUp,
    ToggleMute,
    ToggleSelfEcho,
    Resume,
}

#[derive(Message, Clone, Copy, Debug)]
struct TriggerMenuAction(MenuAction);

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<TriggerMenuAction>()
            .add_systems(Startup, setup_hud)
            .add_systems(
                Update,
                (update_hud, toggle_coop_panel_visibility, handle_menu_actions),
            );
    }
}

fn spawn_menu_button(parent: &mut ChildSpawnerCommands, label: &str, action: MenuAction) {
    parent
        .spawn((
            Node {
                padding: UiRect::axes(Val::Px(12.0), Val::Px(7.0)),
                margin: UiRect::all(Val::Px(3.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.16, 0.22, 0.28, 0.95)),
            action,
        ))
        .observe(
            move |_click: On<PointerClick>, mut writer: MessageWriter<TriggerMenuAction>| {
                writer.write(TriggerMenuAction(action));
            },
        )
        .observe(
            |over: On<PointerOver>, mut bg_q: Query<&mut BackgroundColor>| {
                if let Ok(mut bg) = bg_q.get_mut(over.entity) {
                    *bg = BackgroundColor(Color::srgba(0.24, 0.38, 0.46, 0.98));
                }
            },
        )
        .observe(
            |out: On<PointerOut>, mut bg_q: Query<&mut BackgroundColor>| {
                if let Ok(mut bg) = bg_q.get_mut(out.entity) {
                    *bg = BackgroundColor(Color::srgba(0.16, 0.22, 0.28, 0.95));
                }
            },
        )
        .with_children(|btn| {
            btn.spawn((
                Text::new(label),
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(Color::srgb(0.92, 0.97, 0.96)),
            ));
        });
}

fn setup_hud(mut commands: Commands) {
    // Top-left live acoustic, mic calibration & co-op telemetry panel
    commands.spawn((
        Text::new("Initializing acoustics..."),
        TextFont {
            font_size: FontSize::Px(13.5),
            ..default()
        },
        TextColor(Color::srgba(0.86, 0.94, 0.92, 0.92)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(14.0),
            ..default()
        },
        HudText,
    ));

    // Minimal center crosshair dot
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(50.0),
            top: Val::Percent(50.0),
            width: Val::Px(4.0),
            height: Val::Px(4.0),
            margin: UiRect {
                left: Val::Px(-2.0),
                top: Val::Px(-2.0),
                ..default()
            },
            ..default()
        },
        BackgroundColor(Color::srgba(0.95, 0.95, 0.95, 0.7)),
    ));

    // Toggleable Co-op & Mic Calibration Overlay ([Tab])
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                width: Val::Px(540.0),
                margin: UiRect {
                    left: Val::Px(-270.0),
                    top: Val::Px(-180.0),
                    ..default()
                },
                padding: UiRect::all(Val::Px(18.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(10.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.05, 0.07, 0.10, 0.94)),
            Visibility::Hidden,
            CoopPanelRoot,
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new("CO-OP SESSION (ZERO-API IROH P2P) & MIC CALIBRATION"),
                TextFont {
                    font_size: FontSize::Px(16.0),
                    ..default()
                },
                TextColor(Color::srgb(0.30, 0.92, 0.86)),
            ));

            panel.spawn((
                Text::new("Status: Offline"),
                TextFont {
                    font_size: FontSize::Px(13.5),
                    ..default()
                },
                TextColor(Color::srgb(0.88, 0.92, 0.95)),
                CoopPanelStatusText,
            ));

            // Row 1: Co-op Host / Copy / Paste & Join / Disconnect
            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: Val::Px(6.0),
                    ..default()
                })
                .with_children(|row| {
                    spawn_menu_button(row, "Host Session", MenuAction::HostSession);
                    spawn_menu_button(row, "Copy Join Code", MenuAction::CopyJoinCode);
                    spawn_menu_button(row, "Paste & Join Code", MenuAction::PasteAndJoin);
                    spawn_menu_button(row, "Disconnect", MenuAction::Disconnect);
                });

            // Row 2: Mic Calibration & Self-Echo Acoustic Test
            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: Val::Px(6.0),
                    ..default()
                })
                .with_children(|row| {
                    spawn_menu_button(row, "Gate -3 dB ([)", MenuAction::GateDown);
                    spawn_menu_button(row, "Gate +3 dB (])", MenuAction::GateUp);
                    spawn_menu_button(row, "Mute Mic (M)", MenuAction::ToggleMute);
                    spawn_menu_button(row, "Self-Echo Test (V)", MenuAction::ToggleSelfEcho);
                });

            // Row 3: Visor & Resume
            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(6.0),
                    ..default()
                })
                .with_children(|row| {
                    spawn_menu_button(row, "Cycle Visor Color", MenuAction::CycleVisor);
                    spawn_menu_button(row, "Back to Cave [Tab]", MenuAction::Resume);
                });
        });
}

fn toggle_coop_panel_visibility(
    menu_open: Res<CoopMenuOpen>,
    mut root_q: Query<&mut Visibility, With<CoopPanelRoot>>,
) {
    if let Ok(mut vis) = root_q.single_mut() {
        *vis = if menu_open.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

fn handle_menu_actions(
    mut commands: Commands,
    mut menu_open: ResMut<CoopMenuOpen>,
    mut coop: ResMut<CoopState>,
    mut settings: ResMut<VoiceSettings>,
    beacon: Res<AcousticBeaconState>,
    mut remove_voice: MessageWriter<RemoveVoiceSpeakerEvent>,
    mut actions: MessageReader<TriggerMenuAction>,
) {
    for TriggerMenuAction(action) in actions.read().copied() {
        match action {
            MenuAction::HostSession => {
                clear_remote_players(&mut commands, &mut coop, &mut remove_voice);
                coop.start_host(beacon.pos_m);
            }
            MenuAction::CopyJoinCode => {
                if let Some(code) = coop.join_code.clone() {
                    if copy_to_clipboard(&code) {
                        coop.status_text = "Join code copied to clipboard!".into();
                    } else {
                        coop.status_text = "Could not access clipboard.".into();
                    }
                } else {
                    coop.status_text = "Start hosting first to generate a join code.".into();
                }
            }
            MenuAction::PasteAndJoin => {
                clear_remote_players(&mut commands, &mut coop, &mut remove_voice);
                if let Some(code) = paste_from_clipboard() {
                    coop.start_join(code);
                } else {
                    coop.status_text =
                        "Clipboard empty — copy host's ECHO-... code first!".into();
                }
            }
            MenuAction::Disconnect => {
                clear_remote_players(&mut commands, &mut coop, &mut remove_voice);
                coop.session = None;
                coop.join_code = None;
                coop.status_text = "Disconnected (Offline Solo)".into();
            }
            MenuAction::CycleVisor => {
                coop.visor_idx = (coop.visor_idx + 1) % VISOR_PALETTE.len();
            }
            MenuAction::GateDown => {
                settings.gate_threshold_db =
                    (settings.gate_threshold_db - 3.0).clamp(-65.0, -15.0);
            }
            MenuAction::GateUp => {
                settings.gate_threshold_db =
                    (settings.gate_threshold_db + 3.0).clamp(-65.0, -15.0);
            }
            MenuAction::ToggleMute => {
                settings.muted = !settings.muted;
            }
            MenuAction::ToggleSelfEcho => {
                settings.self_echo_test = !settings.self_echo_test;
            }
            MenuAction::Resume => {
                menu_open.0 = false;
            }
        }
    }
}

fn format_db_meter(rms_db: f32, gate_db: f32) -> String {
    // Visual 20-slot bar spanning -70 dB .. -5 dB with '|' marking the noise gate threshold
    let slots = 20usize;
    let norm = |db: f32| ((db + 70.0) / 65.0).clamp(0.0, 1.0);
    let level_idx = (norm(rms_db) * slots as f32).round() as usize;
    let gate_idx = ((norm(gate_db) * slots as f32).round() as usize).min(slots - 1);

    let mut bar = String::with_capacity(slots + 2);
    bar.push('[');
    for i in 0..slots {
        if i == gate_idx {
            bar.push('|');
        } else if i < level_idx {
            bar.push('#');
        } else {
            bar.push('.');
        }
    }
    bar.push(']');
    bar
}

fn update_hud(
    telemetry: Res<AcousticTelemetry>,
    coop: Res<CoopState>,
    settings: Res<VoiceSettings>,
    fl_q: Query<&PlayerFlashlight>,
    mut hud_q: Query<&mut Text, (With<HudText>, Without<CoopPanelStatusText>)>,
    mut panel_q: Query<&mut Text, (With<CoopPanelStatusText>, Without<HudText>)>,
) {
    let battery_pct = fl_q
        .single()
        .map(|f| (f.battery * 100.0).round() as i32)
        .unwrap_or(0);
    let fl_state = fl_q
        .single()
        .map(|f| if f.on { "ON" } else { "OFF" })
        .unwrap_or("OFF");

    let rev = telemetry.reverb;
    let beacon_line = if let Some(b) = telemetry.beacon_params {
        let active_taps = b.early_taps.iter().filter(|t| t.active).count();
        format!(
            "Beacon: {:.1}m | Wall: {:.2}m | Gain L/M/H: {:.0}/{:.0}/{:.0} dB | Echo Taps: {}",
            b.distance_m,
            b.solid_thickness_m,
            linear_to_db(b.direct_gain.0[0]),
            linear_to_db(b.direct_gain.0[1]),
            linear_to_db(b.direct_gain.0[2]),
            active_taps,
        )
    } else {
        "Beacon: solving...".to_string()
    };

    let mic = telemetry.mic;
    let mic_bar = format_db_meter(mic.rms_db, settings.gate_threshold_db);
    let voice_state = if !mic.available {
        "NO MIC"
    } else if settings.muted {
        "MUTED"
    } else if !mic.gate_open {
        "SILENT"
    } else if mic.ai_noise_level < 0.35 {
        "WHISPER (safe)"
    } else if mic.ai_noise_level < 1.2 {
        "TALKING"
    } else {
        "LOUD! (alerts AI)"
    };

    let echo_tag = if settings.self_echo_test {
        "ON (hear yourself at Beacon!)"
    } else {
        "OFF"
    };

    if let Ok(mut text) = hud_q.single_mut() {
        text.0 = format!(
            "ECHOES BELOW — Week 2 Co-op & Proximity VoIP\n\
             Co-op: {}  |  Visor: {}  |  [Tab] Co-op & Mic Menu\n\
             Mic: {} {:.0} dB (Gate {:.0} dB) — {}  |  Self-Echo [V]: {}\n\
             Room Path-Trace: RT60 L/M/H = {:.2}/{:.2}/{:.2}s | MFP = {:.1}m | Enclosure = {:.0}%\n\
             {}\n\
             Flashlight: {} ({}%) [F/R] | [G] Flare | [LMB] Break | [RMB] Foam | [E] Move Beacon | [M] Mute | [[]/[]] Gate",
            coop.status_text,
            coop.visor_label(),
            mic_bar,
            mic.rms_db,
            settings.gate_threshold_db,
            voice_state,
            echo_tag,
            rev.rt60_sec.0[0],
            rev.rt60_sec.0[1],
            rev.rt60_sec.0[2],
            rev.mean_free_path_m,
            rev.enclosure * 100.0,
            beacon_line,
            fl_state,
            battery_pct,
        );
    }

    if let Ok(mut panel_text) = panel_q.single_mut() {
        let code_preview = match &coop.join_code {
            Some(c) if c.len() > 44 => format!("{}...{} ({} chars)", &c[..24], &c[c.len() - 12..], c.len()),
            Some(c) => c.clone(),
            None => "None (Click 'Host Session' to generate)".into(),
        };
        panel_text.0 = format!(
            "Status: {}\n\
             Join Code: {}\n\
             Mic Level: {} {:.0} dB  |  Gate: {:.0} dB  |  State: {}\n\
             Self-Echo Acoustic Test: {}   |   Visor Color: {}",
            coop.status_text,
            code_preview,
            mic_bar,
            mic.rms_db,
            settings.gate_threshold_db,
            voice_state,
            echo_tag,
            coop.visor_label(),
        );
    }
}
