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
    elevator::{ELEVATOR_CAGE_HALF_EXTENT_M, ElevatorState},
    health::{CRAWL_EYE_HEIGHT_M, CRAWL_SPEED_M_S, DRAG_SPEED_M_S, PlayerHealthState},
    inventory::{CarryHandState, PlayerInventory},
    net_bridge::CoopMenuOpen,
    physics::{
        PLAYER_EYE_CROUCH_M, PLAYER_EYE_STAND_M, PLAYER_GRAVITY_M_S2, PLAYER_HEIGHT_CROUCH_M,
        PLAYER_HEIGHT_STAND_M, PLAYER_JUMP_VELOCITY_M_S, move_and_slide,
    },
    relic::WorldItemRegistry,
    settings::{AppScreen, FOV_MAX_DEG, FOV_MIN_DEG, SettingsMenuOpen, UserSettings},
    shed::ShopOverlayOpen,
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
    pub battery: f32, // 0.0..=1.0 mirrored from PlayerInventory::loaded_battery
}

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_player)
            .add_systems(
                Update,
                (
                    cursor_grab_system,
                    menu_camera_backdrop_system,
                    player_look_system,
                    player_move_system,
                    flashlight_system,
                )
                    .chain(),
            );
    }
}

fn spawn_player(
    mut commands: Commands,
    level_info: Res<LevelInfoResource>,
    settings: Res<UserSettings>,
) {
    let feet = level_info.0.spawn_pos_m;
    let eye = feet + Vec3::Y * PLAYER_EYE_STAND_M;

    // Face -X toward the Base Shed elevator shaft and workbenches on spawn
    let initial_yaw = std::f32::consts::FRAC_PI_2;
    let fov_rad = settings.fov_deg.clamp(FOV_MIN_DEG, FOV_MAX_DEG).to_radians();
    commands
        .spawn((
            Camera3d::default(),
            Projection::Perspective(PerspectiveProjection {
                fov: fov_rad,
                ..default()
            }),
            Transform::from_translation(eye)
                .with_rotation(Quat::from_euler(EulerRot::YXZ, initial_yaw, 0.0, 0.0)),
            PlayerController {
                feet_pos_m: feet,
                velocity_m: Vec3::ZERO,
                yaw: initial_yaw,
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

#[allow(clippy::too_many_arguments)]
fn cursor_grab_system(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    screen: Res<AppScreen>,
    mut prev_screen: Local<Option<AppScreen>>,
    mut menu_open: ResMut<CoopMenuOpen>,
    shop_open: Res<ShopOverlayOpen>,
    mut settings_open: ResMut<SettingsMenuOpen>,
    mut cursor_q: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let Ok(mut cursor) = cursor_q.single_mut() else {
        return;
    };

    let transitioned_to_game =
        *screen == AppScreen::InGame && *prev_screen != Some(AppScreen::InGame);
    *prev_screen = Some(*screen);

    if *screen != AppScreen::InGame {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
        return;
    }

    if transitioned_to_game {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
        return;
    }

    if keys.just_pressed(KeyCode::Tab) && !shop_open.0 {
        menu_open.0 = !menu_open.0;
        if !menu_open.0 {
            settings_open.0 = false;
        }
        if menu_open.0 {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        } else {
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        }
        return;
    }

    if keys.just_pressed(KeyCode::Escape) {
        if settings_open.0 {
            settings_open.0 = false;
        } else if menu_open.0 {
            menu_open.0 = false;
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        } else if !shop_open.0 {
            menu_open.0 = true;
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        }
        return;
    }

    if menu_open.0 || shop_open.0 || settings_open.0 {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
        return;
    }

    if mouse_buttons.just_pressed(MouseButton::Left) && cursor.grab_mode == CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
}

/// Frames the Base Shed & Mineshaft behind the Main Menu and cleanly restores the player
/// camera transform the frame `AppScreen` enters `InGame`.
fn menu_camera_backdrop_system(
    time: Res<Time>,
    screen: Res<AppScreen>,
    mut prev_screen: Local<Option<AppScreen>>,
    level_info: Res<LevelInfoResource>,
    mut player_q: Query<(&PlayerController, &mut Transform)>,
) {
    let Ok((pc, mut tf)) = player_q.single_mut() else {
        return;
    };

    if *screen == AppScreen::MainMenu {
        let t = time.elapsed_secs() * 0.22;
        let focus = level_info.0.elevator_top_pos_m + Vec3::new(1.2, 1.35, 0.0);
        let cam_pos = level_info.0.spawn_pos_m
            + Vec3::new(0.8 + t.sin() * 0.45, 2.05 + (t * 0.7).cos() * 0.12, t.cos() * 1.1);
        *tf = Transform::from_translation(cam_pos).looking_at(focus, Vec3::Y);
    } else if *prev_screen == Some(AppScreen::MainMenu) {
        tf.translation = pc.feet_pos_m + Vec3::Y * PLAYER_EYE_STAND_M;
        tf.rotation = Quat::from_euler(EulerRot::YXZ, pc.yaw, pc.pitch, 0.0);
    }

    *prev_screen = Some(*screen);
}

fn player_look_system(
    mut motion_reader: MessageReader<MouseMotion>,
    screen: Res<AppScreen>,
    menu_open: Res<CoopMenuOpen>,
    shop_open: Res<ShopOverlayOpen>,
    settings_open: Res<SettingsMenuOpen>,
    cursor_q: Query<&CursorOptions, With<PrimaryWindow>>,
    mut player_q: Query<(&mut PlayerController, &mut Transform)>,
) {
    let Ok(cursor) = cursor_q.single() else {
        return;
    };
    if *screen != AppScreen::InGame
        || menu_open.0
        || shop_open.0
        || settings_open.0
        || cursor.grab_mode == CursorGrabMode::None
    {
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

#[allow(clippy::too_many_arguments)]
fn player_move_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    screen: Res<AppScreen>,
    menu_open: Res<CoopMenuOpen>,
    shop_open: Res<ShopOverlayOpen>,
    settings_open: Res<SettingsMenuOpen>,
    world_res: Res<WorldResource>,
    elevator: Option<Res<ElevatorState>>,
    inv: Res<PlayerInventory>,
    health: Res<PlayerHealthState>,
    item_reg: Res<WorldItemRegistry>,
    mut player_q: Query<(&mut PlayerController, &mut Transform)>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
) {
    if *screen != AppScreen::InGame {
        return;
    }

    let dt = time.delta_secs().min(0.05);
    let Ok((mut pc, mut tf)) = player_q.single_mut() else {
        return;
    };

    let controls_active = !menu_open.0 && !shop_open.0 && !settings_open.0 && !health.is_dead();
    let downed = health.is_downed();
    let dragging = matches!(inv.carry_state, CarryHandState::DraggingPlayer { .. });
    let carrying_relic = matches!(inv.carry_state, CarryHandState::HeavyRelic { .. });

    pc.crouching = downed
        || (controls_active && (keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::KeyC)));
    let sprinting = controls_active
        && keys.pressed(KeyCode::ShiftLeft)
        && !pc.crouching
        && !carrying_relic
        && !dragging;

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

    let speed = if downed {
        CRAWL_SPEED_M_S
    } else if dragging {
        DRAG_SPEED_M_S
    } else {
        let base = if pc.crouching {
            2.1
        } else if sprinting {
            6.2
        } else {
            4.0
        };
        if carrying_relic {
            base * item_reg.active_carry_speed_mult
        } else {
            base
        }
    };

    pc.velocity_m.x = wish.x * speed;
    pc.velocity_m.z = wish.z * speed;

    // Heavy gravity + single-tick jump velocity impulse.
    // Holding Space (`keys.pressed`) fires a single velocity impulse the exact tick feet touch ground.
    if controls_active
        && !downed
        && !dragging
        && !carrying_relic
        && pc.grounded
        && keys.pressed(KeyCode::Space)
    {
        pc.velocity_m.y = PLAYER_JUMP_VELOCITY_M_S;
        pc.grounded = false;
    } else {
        pc.velocity_m.y -= PLAYER_GRAVITY_M_S2 * dt;
    }

    let height = if pc.crouching {
        PLAYER_HEIGHT_CROUCH_M
    } else {
        PLAYER_HEIGHT_STAND_M
    };

    let prev_feet = pc.feet_pos_m;
    let world = world_res.read();
    let (mut new_feet, mut new_vel, mut grounded) =
        move_and_slide(&world, pc.feet_pos_m, pc.velocity_m, height, dt);

    // Solid steel elevator floor collision support bridging the full shaft opening (ZERO entrance gap!)
    if let Some(ref elev) = elevator {
        let dx = (new_feet.x - elev.center_xz_m.x).abs();
        let dz = (new_feet.z - elev.center_xz_m.y).abs();
        let over_plate = dx <= ELEVATOR_CAGE_HALF_EXTENT_M + 0.20
            && dz <= ELEVATOR_CAGE_HALF_EXTENT_M + 0.20;
        if over_plate
            && prev_feet.y >= elev.platform_y_m - 0.35
            && new_feet.y <= elev.platform_y_m + 0.05
            && new_vel.y <= 0.0
        {
            new_feet.y = elev.platform_y_m + 0.02;
            new_vel.y = 0.0;
            grounded = true;
        }
    }

    pc.feet_pos_m = new_feet;
    pc.velocity_m = new_vel;
    pc.grounded = grounded;

    let eye_h = if downed {
        CRAWL_EYE_HEIGHT_M
    } else if pc.crouching {
        PLAYER_EYE_CROUCH_M
    } else {
        PLAYER_EYE_STAND_M
    };
    tf.translation = pc.feet_pos_m + Vec3::Y * eye_h;

    // Footstep acoustic emission (scaled by Stealth Boots + pack encumbrance!)
    let horiz_moved = (new_feet - prev_feet).xz().length();
    if grounded && horiz_moved > 0.001 && !downed {
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
            let below = world
                .config()
                .world_to_voxel(pc.feet_pos_m - Vec3::Y * 0.15);
            let clip = match world.get(below) {
                VoxelId::Timber => match pc.step_counter % 3 {
                    0 => SynthClip::FootstepWood,
                    1 => SynthClip::FootstepWood2,
                    _ => SynthClip::FootstepWood3,
                },
                VoxelId::CaveDirt => SynthClip::FootstepDirt,
                _ => SynthClip::FootstepStone,
            };
            let base_volume = if pc.crouching {
                0.24
            } else if sprinting {
                0.80
            } else {
                0.50
            };
            let noise_mult = inv.footstep_noise_multiplier();
            let volume = (base_volume * noise_mult).clamp(0.10, 0.95);
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

#[allow(clippy::too_many_arguments)]
fn flashlight_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    screen: Res<AppScreen>,
    menu_open: Res<CoopMenuOpen>,
    shop_open: Res<ShopOverlayOpen>,
    settings_open: Res<SettingsMenuOpen>,
    health: Res<PlayerHealthState>,
    mut inv: ResMut<PlayerInventory>,
    mut light_q: Query<(&mut SpotLight, &mut PlayerFlashlight)>,
) {
    let dt = time.delta_secs();
    let Ok((mut spot, mut fl)) = light_q.single_mut() else {
        return;
    };

    if *screen != AppScreen::InGame {
        spot.intensity = 180_000.0;
        return;
    }

    if !menu_open.0
        && !shop_open.0
        && !settings_open.0
        && health.is_alive()
        && keys.just_pressed(KeyCode::KeyF)
    {
        fl.on = !fl.on;
    }

    let can_light = health.is_alive() && inv.can_use_light();
    let charge = inv.loaded_battery_charge();
    fl.battery = charge;

    if fl.on && can_light && charge > 0.0 {
        // ~4 minutes per standard battery cell (scaled by cell capacity_scale)
        let remaining = inv.drain_loaded_battery(dt, 240.0);
        fl.battery = remaining;
        let flicker = if remaining < 0.12 {
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
