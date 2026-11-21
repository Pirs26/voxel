use bevy::prelude::*;
use voxel_acoustics::linear_to_db;

use crate::{audio_bridge::AcousticTelemetry, player::PlayerFlashlight};

#[derive(Component)]
struct HudText;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_hud)
            .add_systems(Update, update_hud);
    }
}

fn setup_hud(mut commands: Commands) {
    // Top-left live acoustic telemetry & controls panel
    commands.spawn((
        Text::new("Initializing acoustics..."),
        TextFont {
            font_size: FontSize::Px(14.0),
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
}

fn update_hud(
    telemetry: Res<AcousticTelemetry>,
    fl_q: Query<&PlayerFlashlight>,
    mut text_q: Query<&mut Text, With<HudText>>,
) {
    let Ok(mut text) = text_q.single_mut() else {
        return;
    };

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
            "Beacon: {:.1}m away | Wall: {:.2}m | Gain L/M/H: {:.0}/{:.0}/{:.0} dB | Early Echo Taps: {}",
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

    text.0 = format!(
        "ECHOES BELOW — Week 1 Acoustic Sandbox\n\
         Room Path-Trace: RT60 L/M/H = {:.2}s / {:.2}s / {:.2}s | MFP = {:.1}m | Enclosure = {:.0}%\n\
         {}\n\
         Flashlight: {} ({}%)  [F Toggle | R Battery | G Throw Flare]\n\
         [Click Grab | Esc Release | WASD Move | Shift Sprint | C Crouch | LMB Break Cracked/Foam | RMB Place Foam | E Move Beacon]",
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
