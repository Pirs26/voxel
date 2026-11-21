use bevy::{
    input::mouse::MouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use voxel_acoustics::{AcousticEvent, AcousticEventKind, Bands3};
use voxel_audio_dsp::SynthClip;
use voxel_core::VoxelId;

use crate::{
    audio_bridge::PlaySpatialSoundEvent,
    net_bridge::CoopMenuOpen,
    physics::{
        PLAYER_EYE_CROUCH_M, PLAYER_EYE_STAND_M, PLAYER_HEIGHT_CROUCH_M, PLAYER_HEIGHT_STAND_M,
        move_and_slide,
    },
    voxel_render::{LevelInfoResource, WorldResource},
};

#[derive(Component)]
pub struct PlayerController {
    pub feet_pos_m: Vec3,
    pub velocity_m: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub grounded: bool,
    pub crouching: bool,
    pub footstep_accum_m: f32,
    pub step_counter: u32,
}

#[derive(Component)]
pub struct PlayerFlashlight {
    pub on: bool,
    pub battery: f32, // 0.0..=1.0
}

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_player)
            .add_systems(
                Update,
                (
                    cursor_grab_system,
                    player_look_system,
                    player_move_system,
                    flashlight_system,
                )
                    .chain(),
            );
    }
}

fn spawn_player(mut commands: Commands, level_info: Res<LevelInfoResource>) {
    let feet = level_info.0.spawn_pos_m;
    let eye = feet + Vec3::Y * PLAYER_EYE_STAND_M;

    commands
        .spawn((
            Camera3d::default(),
            Transform::from_translation(eye).looking_to(Vec3::X, Vec3::Y),
            PlayerController {
                feet_pos_m: feet,
                velocity_m: Vec3::ZERO,
                yaw: -std::f32::consts::FRAC_PI_2,
                pitch: 0.0,
                grounded: false,
                crouching: false,
                footstep_accum_m: 0.0,
                step_counter: 0,
            },
        ))
        .with_children(|parent| {
            parent.spawn((
                SpotLight {
                    color: Color::srgb(1.0, 0.95, 0.85),
                    intensity: 420_000.0,
                    range: 42.0,
                    inner_angle: 0.24,
                    outer_angle: 0.56,
                    shadow_maps_enabled: true,
                    ..default()
                },
                Transform::from_xyz(0.14, -0.10, -0.15),
                PlayerFlashlight {
                    on: true,
                    battery: 1.0,
                },
            ));
        });
}

fn cursor_grab_system(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut menu_open: ResMut<CoopMenuOpen>,
    mut cursor_q: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let Ok(mut cursor) = cursor_q.single_mut() else {
        return;
    };

    if keys.just_pressed(KeyCode::Tab) {
        menu_open.0 = !menu_open.0;
        if menu_open.0 {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        } else {
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        }
        return;
    }

    if menu_open.0 {
        if keys.just_pressed(KeyCode::Escape) {
            menu_open.0 = false;
        }
        return;
    }

    if mouse_buttons.just_pressed(MouseButton::Left) && cursor.grab_mode == CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

fn player_look_system(
    mut motion_reader: MessageReader<MouseMotion>,
    menu_open: Res<CoopMenuOpen>,
    cursor_q: Query<&CursorOptions, With<PrimaryWindow>>,
    mut player_q: Query<(&mut PlayerController, &mut Transform)>,
) {
    let Ok(cursor) = cursor_q.single() else {
        return;
    };
    if menu_open.0 || cursor.grab_mode == CursorGrabMode::None {
        motion_reader.clear();
        return;
    }

    let mut delta = Vec2::ZERO;
    for ev in motion_reader.read() {
        delta += ev.delta;
    }
    if delta == Vec2::ZERO {
        return;
    }

    let sensitivity = 0.0022;
    if let Ok((mut pc, mut tf)) = player_q.single_mut() {
        pc.yaw -= delta.x * sensitivity;
        pc.pitch = (pc.pitch - delta.y * sensitivity).clamp(-1.48, 1.48);
        tf.rotation = Quat::from_euler(EulerRot::YXZ, pc.yaw, pc.pitch, 0.0);
    }
}

fn player_move_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    menu_open: Res<CoopMenuOpen>,
    world_res: Res<WorldResource>,
    mut player_q: Query<(&mut PlayerController, &mut Transform)>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
) {
    let dt = time.delta_secs().min(0.05);
    let Ok((mut pc, mut tf)) = player_q.single_mut() else {
        return;
    };

    let controls_active = !menu_open.0;
    pc.crouching =
        controls_active && (keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::KeyC));
    let sprinting = controls_active && keys.pressed(KeyCode::ShiftLeft) && !pc.crouching;

    let forward = Vec3::new(-pc.yaw.sin(), 0.0, -pc.yaw.cos()).normalize_or_zero();
    let right = Vec3::new(pc.yaw.cos(), 0.0, -pc.yaw.sin()).normalize_or_zero();

    let mut wish = Vec3::ZERO;
    if controls_active {
        if keys.pressed(KeyCode::KeyW) {
            wish += forward;
        }
        if keys.pressed(KeyCode::KeyS) {
            wish -= forward;
        }
        if keys.pressed(KeyCode::KeyD) {
            wish += right;
        }
        if keys.pressed(KeyCode::KeyA) {
            wish -= right;
        }
    }
    wish = wish.normalize_or_zero();

    let speed = if pc.crouching {
        2.1
    } else if sprinting {
        6.2
    } else {
        4.0
    };

    pc.velocity_m.x = wish.x * speed;
    pc.velocity_m.z = wish.z * speed;

    // Gravity + Jump
    pc.velocity_m.y -= 18.0 * dt;
    if controls_active && pc.grounded && keys.just_pressed(KeyCode::Space) {
        pc.velocity_m.y = 7.4;
        pc.grounded = false;
    }

    let height = if pc.crouching {
        PLAYER_HEIGHT_CROUCH_M
    } else {
        PLAYER_HEIGHT_STAND_M
    };

    let prev_feet = pc.feet_pos_m;
    let (new_feet, new_vel, grounded) =
        move_and_slide(&world_res.0, pc.feet_pos_m, pc.velocity_m, height, dt);
    pc.feet_pos_m = new_feet;
    pc.velocity_m = new_vel;
    pc.grounded = grounded;

    let eye_h = if pc.crouching {
        PLAYER_EYE_CROUCH_M
    } else {
        PLAYER_EYE_STAND_M
    };
    tf.translation = pc.feet_pos_m + Vec3::Y * eye_h;

    // Footstep acoustic emission
    let horiz_moved = (new_feet - prev_feet).xz().length();
    if grounded && horiz_moved > 0.001 {
        pc.footstep_accum_m += horiz_moved;
        let stride = if pc.crouching {
            1.4
        } else if sprinting {
            2.15
        } else {
            1.75
        };
        if pc.footstep_accum_m >= stride {
            pc.footstep_accum_m = 0.0;
            pc.step_counter = pc.step_counter.wrapping_add(1);
            let below = world_res
                .0
                .config()
                .world_to_voxel(pc.feet_pos_m - Vec3::Y * 0.15);
            let clip = match world_res.0.get(below) {
                VoxelId::Timber => match pc.step_counter % 3 {
                    0 => SynthClip::FootstepWood,
                    1 => SynthClip::FootstepWood2,
                    _ => SynthClip::FootstepWood3,
                },
                VoxelId::CaveDirt => SynthClip::FootstepDirt,
                _ => SynthClip::FootstepStone,
            };
            let volume = if pc.crouching {
                0.24
            } else if sprinting {
                0.80
            } else {
                0.50
            };
            // Deterministic subtle pitch variation per step (~0.92..1.08) avoids robotic repetition
            let h = pc.step_counter.wrapping_mul(2_654_435_761) >> 24;
            let pitch = 0.92 + (h as f32 / 255.0) * 0.16;
            sound_writer.write(PlaySpatialSoundEvent {
                pos_m: pc.feet_pos_m + Vec3::Y * 0.15,
                clip,
                volume,
                pitch,
                ai_event: Some(AcousticEvent {
                    pos_m: pc.feet_pos_m,
                    energy: Bands3::splat(volume * 1.4),
                    sharpness: 0.7,
                    kind: AcousticEventKind::Footstep,
                }),
            });
        }
    }
}

fn flashlight_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut light_q: Query<(&mut SpotLight, &mut PlayerFlashlight)>,
) {
    let dt = time.delta_secs();
    let Ok((mut spot, mut fl)) = light_q.single_mut() else {
        return;
    };

    if keys.just_pressed(KeyCode::KeyF) {
        fl.on = !fl.on;
    }
    if keys.just_pressed(KeyCode::KeyR) {
        // Week-1 sandbox fresh battery swap
        fl.battery = 1.0;
    }

    if fl.on && fl.battery > 0.0 {
        // ~4 minutes per battery
        fl.battery = (fl.battery - dt / 240.0).max(0.0);
        let flicker = if fl.battery < 0.12 {
            let t = time.elapsed_secs() * 23.0;
            if t.sin() > -0.35 { 0.6 } else { 0.08 }
        } else {
            1.0
        };
        spot.intensity = 420_000.0 * flicker;
    } else {
        spot.intensity = 0.0;
    }
}
