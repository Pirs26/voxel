//! First-person camera-attached held tool & pickaxe swing viewmodel (`ViewmodelPlugin`).
//!
//! Lazily attaches a [`ViewmodelRoot`] hierarchy to the [`PlayerController`] camera entity
//! with modular blocky 3D meshes (`Mesh3d` + `MeshMaterial3d`, zero attached `PointLight`s)
//! for all belt items, the left-hand miner's pickaxe, and two-handed heavy-carry brace arms.

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use crate::{
    health::PlayerHealthState,
    inventory::{BeltItem, PlayerInventory},
    net_bridge::CoopMenuOpen,
    player::{PlayerController, PlayerFlashlight},
    settings::{AppScreen, SettingsMenuOpen},
    shed::ShopOverlayOpen,
};

/// Base local offset of [`ViewmodelRoot`] relative to the first-person `Camera3d`.
pub const VIEWMODEL_BASE_OFFSET: Vec3 = Vec3::new(0.28, -0.24, -0.45);
/// Base local offset of the left-hand miner's pickaxe pivot relative to [`ViewmodelRoot`].
pub const PICKAXE_REST_OFFSET: Vec3 = Vec3::new(-0.50, -0.02, 0.02);
/// Base local offset of the right-hand active belt tool pivot relative to [`ViewmodelRoot`].
pub const RIGHT_HAND_REST_OFFSET: Vec3 = Vec3::ZERO;
/// Base local offset of the two-handed heavy-carry brace arms relative to [`ViewmodelRoot`].
pub const CARRY_ARMS_REST_OFFSET: Vec3 = Vec3::new(-0.28, -0.03, 0.03);

/// Duration in seconds of the `[LMB]` pickaxe swing animation arc.
pub const PICKAXE_SWING_DURATION_S: f32 = 0.28;
/// Duration in seconds of the `[RMB]` acoustic foam spray recoil animation.
pub const FOAM_RECOIL_DURATION_S: f32 = 0.18;

/// Marker component placed on the camera-attached viewmodel root entity.
#[derive(Component, Debug)]
pub struct ViewmodelRoot;

/// Identifies an animated transform pivot inside the viewmodel hierarchy.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewmodelPivot {
    /// Foreground rig root attached to the player camera (`VIEWMODEL_BASE_OFFSET` + walk bob).
    Root,
    /// Left-hand pickaxe pivot (rest pose + `[LMB]` swing arc).
    Pickaxe,
    /// Right-hand active belt item pivot (rest pose + `[RMB]` foam recoil).
    RightHand,
    /// Dual-glove heavy carry arms pivot (rest pose + heavy carry bob).
    CarryArms,
}

/// Identifies a visibility-controlled group inside the viewmodel hierarchy.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewmodelVisibilityGroup {
    /// Entire viewmodel rig (hidden outside `AppScreen::InGame` or when dead).
    Root,
    /// Left-hand miner's pickaxe (visible whenever hands are free).
    Pickaxe,
    /// Two-handed carry brace arms (visible whenever `inv.is_hands_occupied()`).
    CarryArms,
    /// Handheld steel flashlight body.
    Flashlight,
    /// Glowing warm flashlight lens cap when `PlayerFlashlight::on && battery > 0.0`.
    FlashlightLensOn,
    /// Dim unlit flashlight lens cap when flashlight is switched off or depleted.
    FlashlightLensOff,
    /// Crimson chemical glow flare stick with ember tip.
    GlowFlare,
    /// Pressurized teal acoustic foam canister with white nozzle.
    AcousticFoam,
    /// Amber hazard noise decoy clacker box.
    NoiseDecoy,
    /// White medical aid kit case with red cross plates.
    AidKit,
    /// Faceted glowing cyan/gold bonus crystal shard.
    BonusCrystal,
}

/// Runtime procedural animation state for the first-person viewmodel.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct ViewmodelAnimationState {
    /// Accumulated radians for the figure-8 walking bob cycle.
    pub walk_phase_rad: f32,
    /// Smoothed normalized horizontal speed `[0.0, 1.25]` for bob blending.
    pub smoothed_speed_ratio: f32,
    /// Smoothed crouch offset in meters.
    pub smoothed_crouch_m: f32,
    /// Remaining time in seconds for the `[LMB]` pickaxe swing (`0.0..=0.28`).
    pub swing_timer_s: f32,
    /// Remaining time in seconds for the `[RMB]` foam spray recoil (`0.0..=0.18`).
    pub recoil_timer_s: f32,
}

/// First-person held tool, pickaxe swing, and heavy-carry viewmodel plugin.
pub struct ViewmodelPlugin;

impl Plugin for ViewmodelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewmodelAnimationState>().add_systems(
            Update,
            (
                attach_viewmodel_system,
                animate_viewmodel_system,
                sync_viewmodel_visibility_system,
            )
                .chain(),
        );
    }
}

/// Rest rotation for the left-hand pickaxe so the iron head frames cleanly in the lower-left view.
#[inline]
pub fn pickaxe_rest_rotation() -> Quat {
    Quat::from_euler(EulerRot::YXZ, -0.24, 0.20, -0.14)
}

/// Rest rotation for the right-hand tool pivot so held items point slightly inward toward the crosshair.
#[inline]
pub fn right_hand_rest_rotation() -> Quat {
    Quat::from_euler(EulerRot::YXZ, 0.10, 0.06, 0.04)
}

/// Computes the sinusoidal figure-8 walking bob translation offset and subtle roll/pitch rotation.
pub fn compute_walk_bob(phase_rad: f32, speed_ratio: f32) -> (Vec3, Quat) {
    let clamped = speed_ratio.clamp(0.0, 1.25);
    if clamped <= 1e-4 {
        return (Vec3::ZERO, Quat::IDENTITY);
    }
    let sway_x = phase_rad.cos() * 0.014 * clamped;
    let bob_y = -(phase_rad.sin().abs()) * 0.016 * clamped;
    let surge_z = (phase_rad * 2.0).cos() * 0.005 * clamped;

    let pitch = (phase_rad * 2.0).sin() * 0.022 * clamped;
    let roll = -phase_rad.cos() * 0.026 * clamped;

    (
        Vec3::new(sway_x, bob_y, surge_z),
        Quat::from_euler(EulerRot::YXZ, 0.0, pitch, roll),
    )
}

/// Computes the additive translation and rotation for the `[LMB]` pickaxe swing (`0.28s` arc).
///
/// Swings forward and down-left across the lower-center foreground with a snappy strike peak
/// followed by a smooth spring return.
pub fn compute_pickaxe_swing_pose(swing_remaining_s: f32) -> (Vec3, Quat) {
    if swing_remaining_s <= 0.0 {
        return (Vec3::ZERO, Quat::IDENTITY);
    }
    let progress =
        (1.0 - (swing_remaining_s / PICKAXE_SWING_DURATION_S)).clamp(0.0, 1.0);

    // Fast windup + snappy strike during 0..0.38, elastic recovery during 0.38..1.0
    let strike_curve = if progress < 0.38 {
        let t = progress / 0.38;
        (t * std::f32::consts::FRAC_PI_2).sin()
    } else {
        let t = (progress - 0.38) / 0.62;
        let decay = 1.0 - t;
        decay * decay * (1.0 + 0.25 * (t * std::f32::consts::PI).sin())
    };

    // Slight cross-body slash component peaking mid-recovery
    let slash_lateral = (progress * std::f32::consts::PI).sin();

    let delta_pos = Vec3::new(
        0.13 * strike_curve - 0.04 * slash_lateral,
        -0.09 * strike_curve,
        -0.16 * strike_curve,
    );
    let delta_rot = Quat::from_euler(
        EulerRot::YXZ,
        -0.55 * strike_curve,
        -0.98 * strike_curve,
        -0.42 * strike_curve,
    );

    (delta_pos, delta_rot)
}

/// Computes the additive translation and rotation for the `[RMB]` foam-spray recoil (`0.18s`).
pub fn compute_foam_recoil_pose(recoil_remaining_s: f32) -> (Vec3, Quat) {
    if recoil_remaining_s <= 0.0 {
        return (Vec3::ZERO, Quat::IDENTITY);
    }
    let progress =
        (1.0 - (recoil_remaining_s / FOAM_RECOIL_DURATION_S)).clamp(0.0, 1.0);
    let envelope = (progress * std::f32::consts::PI).sin();

    let delta_pos = Vec3::new(0.008 * envelope, -0.012 * envelope, 0.048 * envelope);
    let delta_rot = Quat::from_euler(EulerRot::YXZ, 0.0, 0.18 * envelope, 0.04 * envelope);
    (delta_pos, delta_rot)
}

/// Lazily spawns and attaches the [`ViewmodelRoot`] hierarchy to the [`PlayerController`] camera
/// as soon as the player entity exists.
fn attach_viewmodel_system(
    mut commands: Commands,
    player_q: Query<Entity, With<PlayerController>>,
    existing_root_q: Query<Entity, With<ViewmodelRoot>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !existing_root_q.is_empty() {
        return;
    }
    let Ok(player_entity) = player_q.single() else {
        return;
    };

    // Shared blocky materials (zero PointLights attached to any viewmodel mesh entity)
    let mat_dark_steel = materials.add(StandardMaterial {
        base_color: Color::srgb(0.20, 0.22, 0.25),
        perceptual_roughness: 0.42,
        metallic: 0.78,
        ..default()
    });
    let mat_dark_iron = materials.add(StandardMaterial {
        base_color: Color::srgb(0.14, 0.15, 0.17),
        perceptual_roughness: 0.50,
        metallic: 0.82,
        ..default()
    });
    let mat_timber = materials.add(StandardMaterial {
        base_color: Color::srgb(0.46, 0.29, 0.15),
        perceptual_roughness: 0.85,
        metallic: 0.04,
        ..default()
    });
    let mat_leather_grip = materials.add(StandardMaterial {
        base_color: Color::srgb(0.24, 0.16, 0.11),
        perceptual_roughness: 0.90,
        ..default()
    });
    let mat_lens_on = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.94, 0.78),
        emissive: LinearRgba::new(9.5, 7.6, 4.0, 1.0),
        perceptual_roughness: 0.15,
        ..default()
    });
    let mat_lens_off = materials.add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.18, 0.20),
        perceptual_roughness: 0.25,
        metallic: 0.30,
        ..default()
    });
    let mat_flare_crimson = materials.add(StandardMaterial {
        base_color: Color::srgb(0.76, 0.14, 0.12),
        perceptual_roughness: 0.65,
        ..default()
    });
    let mat_flare_ember = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.42, 0.14),
        emissive: LinearRgba::new(8.0, 2.5, 0.6, 1.0),
        perceptual_roughness: 0.30,
        ..default()
    });
    let mat_foam_teal = materials.add(StandardMaterial {
        base_color: Color::srgb(0.12, 0.58, 0.56),
        perceptual_roughness: 0.45,
        metallic: 0.35,
        ..default()
    });
    let mat_nozzle_white = materials.add(StandardMaterial {
        base_color: Color::srgb(0.90, 0.92, 0.94),
        perceptual_roughness: 0.40,
        ..default()
    });
    let mat_decoy_amber = materials.add(StandardMaterial {
        base_color: Color::srgb(0.88, 0.58, 0.10),
        perceptual_roughness: 0.48,
        metallic: 0.25,
        ..default()
    });
    let mat_decoy_beacon = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.72, 0.18),
        emissive: LinearRgba::new(3.5, 2.1, 0.3, 1.0),
        perceptual_roughness: 0.30,
        ..default()
    });
    let mat_aid_white = materials.add(StandardMaterial {
        base_color: Color::srgb(0.92, 0.93, 0.95),
        perceptual_roughness: 0.52,
        ..default()
    });
    let mat_aid_red = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.14, 0.16),
        emissive: LinearRgba::new(0.9, 0.08, 0.08, 1.0),
        perceptual_roughness: 0.45,
        ..default()
    });
    let mat_crystal_cyan = materials.add(StandardMaterial {
        base_color: Color::srgb(0.28, 0.88, 0.96),
        emissive: LinearRgba::new(1.8, 6.2, 7.8, 1.0),
        perceptual_roughness: 0.20,
        metallic: 0.15,
        ..default()
    });
    let mat_crystal_gold = materials.add(StandardMaterial {
        base_color: Color::srgb(0.96, 0.78, 0.24),
        emissive: LinearRgba::new(5.2, 3.8, 0.7, 1.0),
        perceptual_roughness: 0.24,
        metallic: 0.45,
        ..default()
    });
    let mat_glove_fabric = materials.add(StandardMaterial {
        base_color: Color::srgb(0.32, 0.28, 0.22),
        perceptual_roughness: 0.85,
        ..default()
    });
    let mat_sleeve_canvas = materials.add(StandardMaterial {
        base_color: Color::srgb(0.22, 0.26, 0.30),
        perceptual_roughness: 0.88,
        ..default()
    });

    // Pre-build blocky cuboid meshes
    let mesh_pick_handle = meshes.add(Cuboid::new(0.030, 0.33, 0.030));
    let mesh_pick_grip = meshes.add(Cuboid::new(0.035, 0.11, 0.035));
    let mesh_pick_collar = meshes.add(Cuboid::new(0.044, 0.045, 0.046));
    let mesh_pick_head_main = meshes.add(Cuboid::new(0.036, 0.036, 0.22));
    let mesh_pick_tip = meshes.add(Cuboid::new(0.028, 0.028, 0.075));

    let mesh_fl_body = meshes.add(Cuboid::new(0.050, 0.050, 0.21));
    let mesh_fl_grip = meshes.add(Cuboid::new(0.055, 0.055, 0.09));
    let mesh_fl_bezel = meshes.add(Cuboid::new(0.066, 0.066, 0.045));
    let mesh_fl_switch = meshes.add(Cuboid::new(0.016, 0.012, 0.030));
    let mesh_fl_lens = meshes.add(Cuboid::new(0.054, 0.054, 0.010));

    let mesh_flare_body = meshes.add(Cuboid::new(0.036, 0.20, 0.036));
    let mesh_flare_band = meshes.add(Cuboid::new(0.040, 0.024, 0.040));
    let mesh_flare_tip = meshes.add(Cuboid::new(0.042, 0.036, 0.042));

    let mesh_foam_tank = meshes.add(Cuboid::new(0.070, 0.19, 0.070));
    let mesh_foam_cap = meshes.add(Cuboid::new(0.075, 0.022, 0.075));
    let mesh_foam_valve = meshes.add(Cuboid::new(0.032, 0.034, 0.032));
    let mesh_foam_nozzle = meshes.add(Cuboid::new(0.022, 0.022, 0.075));

    let mesh_decoy_box = meshes.add(Cuboid::new(0.092, 0.115, 0.066));
    let mesh_decoy_grille = meshes.add(Cuboid::new(0.068, 0.058, 0.012));
    let mesh_decoy_antenna = meshes.add(Cuboid::new(0.012, 0.065, 0.012));
    let mesh_decoy_bulb = meshes.add(Cuboid::new(0.026, 0.024, 0.026));

    let mesh_aid_case = meshes.add(Cuboid::new(0.128, 0.092, 0.064));
    let mesh_aid_handle = meshes.add(Cuboid::new(0.054, 0.018, 0.020));
    let mesh_aid_cross_h = meshes.add(Cuboid::new(0.054, 0.016, 0.008));
    let mesh_aid_cross_v = meshes.add(Cuboid::new(0.016, 0.054, 0.008));

    let mesh_crystal_base = meshes.add(Cuboid::new(0.076, 0.044, 0.072));
    let mesh_crystal_main = meshes.add(Cuboid::new(0.046, 0.175, 0.046));
    let mesh_crystal_side = meshes.add(Cuboid::new(0.034, 0.115, 0.034));

    let mesh_arm_sleeve = meshes.add(Cuboid::new(0.068, 0.064, 0.22));
    let mesh_arm_cuff = meshes.add(Cuboid::new(0.074, 0.070, 0.045));
    let mesh_arm_glove = meshes.add(Cuboid::new(0.078, 0.058, 0.095));

    commands.entity(player_entity).with_children(|parent| {
        parent
            .spawn((
                ViewmodelRoot,
                ViewmodelPivot::Root,
                ViewmodelVisibilityGroup::Root,
                Transform::from_translation(VIEWMODEL_BASE_OFFSET),
                Visibility::Hidden,
            ))
            .with_children(|root| {
                // 1. Left-Hand / Main Swing Pickaxe
                root.spawn((
                    ViewmodelPivot::Pickaxe,
                    ViewmodelVisibilityGroup::Pickaxe,
                    Transform::from_translation(PICKAXE_REST_OFFSET)
                        .with_rotation(pickaxe_rest_rotation()),
                    Visibility::Inherited,
                ))
                .with_children(|pick| {
                    pick.spawn((
                        Mesh3d(mesh_pick_handle),
                        MeshMaterial3d(mat_timber),
                        Transform::from_xyz(0.0, 0.0, 0.0),
                    ));
                    pick.spawn((
                        Mesh3d(mesh_pick_grip),
                        MeshMaterial3d(mat_leather_grip),
                        Transform::from_xyz(0.0, -0.09, 0.0),
                    ));
                    pick.spawn((
                        Mesh3d(mesh_pick_collar),
                        MeshMaterial3d(mat_dark_steel.clone()),
                        Transform::from_xyz(0.0, 0.14, 0.0),
                    ));
                    pick.spawn((
                        Mesh3d(mesh_pick_head_main),
                        MeshMaterial3d(mat_dark_iron.clone()),
                        Transform::from_xyz(0.0, 0.145, -0.01),
                    ));
                    pick.spawn((
                        Mesh3d(mesh_pick_tip.clone()),
                        MeshMaterial3d(mat_dark_iron.clone()),
                        Transform::from_xyz(0.0, 0.132, -0.135)
                            .with_rotation(Quat::from_rotation_x(0.28)),
                    ));
                    pick.spawn((
                        Mesh3d(mesh_pick_tip),
                        MeshMaterial3d(mat_dark_iron.clone()),
                        Transform::from_xyz(0.0, 0.135, 0.11)
                            .with_rotation(Quat::from_rotation_x(-0.22)),
                    ));
                });

                // 2. Right-Hand Active Belt Tool Pivot
                root.spawn((
                    ViewmodelPivot::RightHand,
                    Transform::from_translation(RIGHT_HAND_REST_OFFSET)
                        .with_rotation(right_hand_rest_rotation()),
                    Visibility::Inherited,
                ))
                .with_children(|right| {
                    // 2a. Flashlight
                    right
                        .spawn((
                            ViewmodelVisibilityGroup::Flashlight,
                            Transform::from_xyz(0.0, 0.0, -0.02),
                            Visibility::Inherited,
                        ))
                        .with_children(|fl| {
                            fl.spawn((
                                Mesh3d(mesh_fl_body),
                                MeshMaterial3d(mat_dark_steel.clone()),
                                Transform::from_xyz(0.0, 0.0, 0.0),
                            ));
                            fl.spawn((
                                Mesh3d(mesh_fl_grip),
                                MeshMaterial3d(mat_dark_iron.clone()),
                                Transform::from_xyz(0.0, 0.0, 0.02),
                            ));
                            fl.spawn((
                                Mesh3d(mesh_fl_bezel),
                                MeshMaterial3d(mat_dark_steel.clone()),
                                Transform::from_xyz(0.0, 0.0, -0.105),
                            ));
                            fl.spawn((
                                Mesh3d(mesh_fl_switch),
                                MeshMaterial3d(mat_flare_crimson.clone()),
                                Transform::from_xyz(0.0, 0.030, -0.035),
                            ));
                            fl.spawn((
                                ViewmodelVisibilityGroup::FlashlightLensOn,
                                Mesh3d(mesh_fl_lens.clone()),
                                MeshMaterial3d(mat_lens_on),
                                Transform::from_xyz(0.0, 0.0, -0.128),
                                Visibility::Inherited,
                            ));
                            fl.spawn((
                                ViewmodelVisibilityGroup::FlashlightLensOff,
                                Mesh3d(mesh_fl_lens),
                                MeshMaterial3d(mat_lens_off),
                                Transform::from_xyz(0.0, 0.0, -0.128),
                                Visibility::Hidden,
                            ));
                        });

                    // 2b. Glow Flare
                    right
                        .spawn((
                            ViewmodelVisibilityGroup::GlowFlare,
                            Transform::from_xyz(0.0, 0.02, -0.02)
                                .with_rotation(Quat::from_rotation_x(-0.35)),
                            Visibility::Hidden,
                        ))
                        .with_children(|flare| {
                            flare.spawn((
                                Mesh3d(mesh_flare_body),
                                MeshMaterial3d(mat_flare_crimson),
                                Transform::from_xyz(0.0, 0.0, 0.0),
                            ));
                            flare.spawn((
                                Mesh3d(mesh_flare_band.clone()),
                                MeshMaterial3d(mat_dark_iron.clone()),
                                Transform::from_xyz(0.0, -0.055, 0.0),
                            ));
                            flare.spawn((
                                Mesh3d(mesh_flare_band),
                                MeshMaterial3d(mat_dark_iron.clone()),
                                Transform::from_xyz(0.0, 0.055, 0.0),
                            ));
                            flare.spawn((
                                Mesh3d(mesh_flare_tip),
                                MeshMaterial3d(mat_flare_ember),
                                Transform::from_xyz(0.0, 0.112, 0.0),
                            ));
                        });

                    // 2c. Acoustic Foam Canister
                    right
                        .spawn((
                            ViewmodelVisibilityGroup::AcousticFoam,
                            Transform::from_xyz(0.0, 0.01, -0.02)
                                .with_rotation(Quat::from_rotation_x(-0.18)),
                            Visibility::Hidden,
                        ))
                        .with_children(|foam| {
                            foam.spawn((
                                Mesh3d(mesh_foam_tank),
                                MeshMaterial3d(mat_foam_teal),
                                Transform::from_xyz(0.0, 0.0, 0.0),
                            ));
                            foam.spawn((
                                Mesh3d(mesh_foam_cap.clone()),
                                MeshMaterial3d(mat_dark_steel.clone()),
                                Transform::from_xyz(0.0, 0.095, 0.0),
                            ));
                            foam.spawn((
                                Mesh3d(mesh_foam_cap),
                                MeshMaterial3d(mat_dark_steel.clone()),
                                Transform::from_xyz(0.0, -0.095, 0.0),
                            ));
                            foam.spawn((
                                Mesh3d(mesh_foam_valve),
                                MeshMaterial3d(mat_nozzle_white.clone()),
                                Transform::from_xyz(0.0, 0.12, 0.0),
                            ));
                            foam.spawn((
                                Mesh3d(mesh_foam_nozzle),
                                MeshMaterial3d(mat_nozzle_white),
                                Transform::from_xyz(0.0, 0.122, -0.045),
                            ));
                        });

                    // 2d. Noise Decoy
                    right
                        .spawn((
                            ViewmodelVisibilityGroup::NoiseDecoy,
                            Transform::from_xyz(-0.01, 0.01, -0.02)
                                .with_rotation(Quat::from_rotation_y(-0.18)),
                            Visibility::Hidden,
                        ))
                        .with_children(|decoy| {
                            decoy.spawn((
                                Mesh3d(mesh_decoy_box),
                                MeshMaterial3d(mat_decoy_amber),
                                Transform::from_xyz(0.0, 0.0, 0.0),
                            ));
                            decoy.spawn((
                                Mesh3d(mesh_decoy_grille),
                                MeshMaterial3d(mat_dark_iron.clone()),
                                Transform::from_xyz(0.0, -0.005, -0.032),
                            ));
                            decoy.spawn((
                                Mesh3d(mesh_decoy_antenna),
                                MeshMaterial3d(mat_dark_steel.clone()),
                                Transform::from_xyz(0.026, 0.088, 0.0),
                            ));
                            decoy.spawn((
                                Mesh3d(mesh_decoy_bulb),
                                MeshMaterial3d(mat_decoy_beacon),
                                Transform::from_xyz(-0.020, 0.068, 0.0),
                            ));
                        });

                    // 2e. Aid Kit
                    right
                        .spawn((
                            ViewmodelVisibilityGroup::AidKit,
                            Transform::from_xyz(-0.015, 0.01, -0.02)
                                .with_rotation(Quat::from_euler(EulerRot::YXZ, -0.22, 0.15, 0.0)),
                            Visibility::Hidden,
                        ))
                        .with_children(|aid| {
                            aid.spawn((
                                Mesh3d(mesh_aid_case),
                                MeshMaterial3d(mat_aid_white),
                                Transform::from_xyz(0.0, 0.0, 0.0),
                            ));
                            aid.spawn((
                                Mesh3d(mesh_aid_handle),
                                MeshMaterial3d(mat_dark_steel.clone()),
                                Transform::from_xyz(0.0, 0.054, 0.0),
                            ));
                            aid.spawn((
                                Mesh3d(mesh_aid_cross_h),
                                MeshMaterial3d(mat_aid_red.clone()),
                                Transform::from_xyz(0.0, 0.0, -0.031),
                            ));
                            aid.spawn((
                                Mesh3d(mesh_aid_cross_v),
                                MeshMaterial3d(mat_aid_red),
                                Transform::from_xyz(0.0, 0.0, -0.031),
                            ));
                        });

                    // 2f. Bonus Crystal
                    right
                        .spawn((
                            ViewmodelVisibilityGroup::BonusCrystal,
                            Transform::from_xyz(0.0, 0.01, -0.02),
                            Visibility::Hidden,
                        ))
                        .with_children(|crystal| {
                            crystal.spawn((
                                Mesh3d(mesh_crystal_base),
                                MeshMaterial3d(mat_dark_iron.clone()),
                                Transform::from_xyz(0.0, -0.065, 0.0),
                            ));
                            crystal.spawn((
                                Mesh3d(mesh_crystal_main),
                                MeshMaterial3d(mat_crystal_cyan),
                                Transform::from_xyz(0.0, 0.018, 0.0)
                                    .with_rotation(Quat::from_euler(
                                        EulerRot::YXZ,
                                        0.35,
                                        0.08,
                                        -0.10,
                                    )),
                            ));
                            crystal.spawn((
                                Mesh3d(mesh_crystal_side.clone()),
                                MeshMaterial3d(mat_crystal_gold.clone()),
                                Transform::from_xyz(0.026, -0.008, -0.015)
                                    .with_rotation(Quat::from_euler(
                                        EulerRot::YXZ,
                                        -0.40,
                                        0.22,
                                        -0.38,
                                    )),
                            ));
                            crystal.spawn((
                                Mesh3d(mesh_crystal_side),
                                MeshMaterial3d(mat_crystal_gold),
                                Transform::from_xyz(-0.024, -0.012, 0.014)
                                    .with_rotation(Quat::from_euler(
                                        EulerRot::YXZ,
                                        0.25,
                                        -0.18,
                                        0.36,
                                    )),
                            ));
                        });
                });

                // 3. Two-Handed Carry Arms (lower-left & lower-right dual-glove brace bars)
                root.spawn((
                    ViewmodelPivot::CarryArms,
                    ViewmodelVisibilityGroup::CarryArms,
                    Transform::from_translation(CARRY_ARMS_REST_OFFSET),
                    Visibility::Hidden,
                ))
                .with_children(|arms| {
                    // Left raised forearm & work glove
                    arms.spawn((
                        Mesh3d(mesh_arm_sleeve.clone()),
                        MeshMaterial3d(mat_sleeve_canvas.clone()),
                        Transform::from_xyz(-0.19, -0.02, 0.05)
                            .with_rotation(Quat::from_euler(EulerRot::YXZ, -0.22, 0.26, -0.12)),
                    ));
                    arms.spawn((
                        Mesh3d(mesh_arm_cuff.clone()),
                        MeshMaterial3d(mat_dark_steel.clone()),
                        Transform::from_xyz(-0.155, 0.015, -0.04)
                            .with_rotation(Quat::from_euler(EulerRot::YXZ, -0.22, 0.26, -0.12)),
                    ));
                    arms.spawn((
                        Mesh3d(mesh_arm_glove.clone()),
                        MeshMaterial3d(mat_glove_fabric.clone()),
                        Transform::from_xyz(-0.13, 0.035, -0.10)
                            .with_rotation(Quat::from_euler(EulerRot::YXZ, -0.28, 0.30, -0.18)),
                    ));

                    // Right raised forearm & work glove
                    arms.spawn((
                        Mesh3d(mesh_arm_sleeve),
                        MeshMaterial3d(mat_sleeve_canvas),
                        Transform::from_xyz(0.19, -0.02, 0.05)
                            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.22, 0.26, 0.12)),
                    ));
                    arms.spawn((
                        Mesh3d(mesh_arm_cuff),
                        MeshMaterial3d(mat_dark_steel),
                        Transform::from_xyz(0.155, 0.015, -0.04)
                            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.22, 0.26, 0.12)),
                    ));
                    arms.spawn((
                        Mesh3d(mesh_arm_glove),
                        MeshMaterial3d(mat_glove_fabric),
                        Transform::from_xyz(0.13, 0.035, -0.10)
                            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.28, 0.30, 0.18)),
                    ));
                });
            });
    });
}

/// Advances walking bob, `[LMB]` pickaxe swing (`0.28s`), and `[RMB]` foam-spray recoil (`0.18s`),
/// applying transforms to all [`ViewmodelPivot`] entities in a single non-conflicting query.
#[allow(clippy::too_many_arguments)]
fn animate_viewmodel_system(
    time: Res<Time>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    screen: Option<Res<AppScreen>>,
    menu_open: Option<Res<CoopMenuOpen>>,
    shop_open: Option<Res<ShopOverlayOpen>>,
    settings_open: Option<Res<SettingsMenuOpen>>,
    health: Option<Res<PlayerHealthState>>,
    inv: Option<Res<PlayerInventory>>,
    cursor_q: Query<&CursorOptions, With<PrimaryWindow>>,
    player_q: Query<&PlayerController>,
    mut anim: ResMut<ViewmodelAnimationState>,
    mut pivot_q: Query<(&ViewmodelPivot, &mut Transform)>,
) {
    let dt = time.delta_secs().min(0.05);
    anim.swing_timer_s = (anim.swing_timer_s - dt).max(0.0);
    anim.recoil_timer_s = (anim.recoil_timer_s - dt).max(0.0);

    let in_game = screen.as_deref().copied() == Some(AppScreen::InGame);
    let overlays_closed = !menu_open.as_ref().is_some_and(|m| m.0)
        && !shop_open.as_ref().is_some_and(|s| s.0)
        && !settings_open.as_ref().is_some_and(|s| s.0);
    let is_healthy = health.as_ref().is_none_or(|h| h.is_healthy());
    let hands_occupied = inv.as_ref().is_some_and(|i| i.is_hands_occupied());
    let cursor_locked = cursor_q
        .single()
        .is_ok_and(|c| c.grab_mode != CursorGrabMode::None)
        || cursor_q.single().is_err();

    if in_game && overlays_closed && is_healthy && !hands_occupied && cursor_locked
        && let Some(ref mouse_input) = mouse
    {
        if mouse_input.just_pressed(MouseButton::Left) {
            anim.swing_timer_s = PICKAXE_SWING_DURATION_S;
        }
        if mouse_input.just_pressed(MouseButton::Right) {
            anim.recoil_timer_s = FOAM_RECOIL_DURATION_S;
        }
    }

    let (horiz_speed, grounded, crouching) = if let Ok(pc) = player_q.single() {
        (pc.velocity_m.xz().length(), pc.grounded, pc.crouching)
    } else {
        (0.0, false, false)
    };

    let target_speed_ratio = if in_game && grounded && horiz_speed > 0.05 {
        (horiz_speed / 4.2).clamp(0.0, 1.25)
    } else {
        0.0
    };
    let blend = (dt * 12.0).clamp(0.0, 1.0);
    anim.smoothed_speed_ratio += (target_speed_ratio - anim.smoothed_speed_ratio) * blend;

    if anim.smoothed_speed_ratio > 0.01 {
        let freq = 6.5 + anim.smoothed_speed_ratio * 3.2;
        anim.walk_phase_rad =
            (anim.walk_phase_rad + dt * freq).rem_euclid(std::f32::consts::TAU);
    }

    let target_crouch_m = if crouching { -0.025 } else { 0.0 };
    anim.smoothed_crouch_m += (target_crouch_m - anim.smoothed_crouch_m) * blend;

    // Gentle idle breathing motion
    let idle_t = time.elapsed_secs();
    let idle_offset = Vec3::new(
        (idle_t * 1.3).sin() * 0.0025,
        (idle_t * 2.1).cos() * 0.0035 + anim.smoothed_crouch_m,
        0.0,
    );

    let (bob_pos, bob_rot) = compute_walk_bob(anim.walk_phase_rad, anim.smoothed_speed_ratio);
    let (swing_pos, swing_rot) = compute_pickaxe_swing_pose(anim.swing_timer_s);
    let (recoil_pos, recoil_rot) = compute_foam_recoil_pose(anim.recoil_timer_s);

    for (pivot, mut tf) in &mut pivot_q {
        match pivot {
            ViewmodelPivot::Root => {
                tf.translation = VIEWMODEL_BASE_OFFSET + idle_offset + bob_pos;
                tf.rotation = bob_rot;
            }
            ViewmodelPivot::Pickaxe => {
                tf.translation = PICKAXE_REST_OFFSET + swing_pos;
                tf.rotation = pickaxe_rest_rotation() * swing_rot;
            }
            ViewmodelPivot::RightHand => {
                tf.translation = RIGHT_HAND_REST_OFFSET + recoil_pos;
                tf.rotation = right_hand_rest_rotation() * recoil_rot;
            }
            ViewmodelPivot::CarryArms => {
                let carry_strain_y = (idle_t * 2.8).sin() * 0.004;
                tf.translation = CARRY_ARMS_REST_OFFSET + Vec3::Y * carry_strain_y;
                tf.rotation = Quat::IDENTITY;
            }
        }
    }
}

/// Synchronizes the visibility of the viewmodel root, left-hand pickaxe, right-hand belt item,
/// flashlight lens emissive state, and two-handed carry arms in a single query.
fn sync_viewmodel_visibility_system(
    screen: Option<Res<AppScreen>>,
    health: Option<Res<PlayerHealthState>>,
    inv: Option<Res<PlayerInventory>>,
    flashlight_q: Query<&PlayerFlashlight>,
    mut vis_q: Query<(&ViewmodelVisibilityGroup, &mut Visibility)>,
) {
    let in_game = screen.as_deref().copied() == Some(AppScreen::InGame);
    let is_dead = health.as_ref().is_some_and(|h| h.is_dead());
    let show_root = in_game && !is_dead;

    let (hands_occupied, active_item, fallback_charge) = if let Some(ref inventory) = inv {
        (
            inventory.is_hands_occupied(),
            inventory.active_item().cloned(),
            inventory.loaded_battery_charge(),
        )
    } else {
        (false, Some(BeltItem::Flashlight), 1.0)
    };

    let flashlight_lit = if let Ok(fl) = flashlight_q.single() {
        fl.on && fl.battery > 0.0
    } else {
        fallback_charge > 0.0
    };

    for (group, mut vis) in &mut vis_q {
        let should_show = match group {
            ViewmodelVisibilityGroup::Root => show_root,
            ViewmodelVisibilityGroup::Pickaxe => !hands_occupied,
            ViewmodelVisibilityGroup::CarryArms => hands_occupied,
            ViewmodelVisibilityGroup::Flashlight => {
                !hands_occupied && matches!(active_item, Some(BeltItem::Flashlight))
            }
            ViewmodelVisibilityGroup::FlashlightLensOn => {
                !hands_occupied
                    && matches!(active_item, Some(BeltItem::Flashlight))
                    && flashlight_lit
            }
            ViewmodelVisibilityGroup::FlashlightLensOff => {
                !hands_occupied
                    && matches!(active_item, Some(BeltItem::Flashlight))
                    && !flashlight_lit
            }
            ViewmodelVisibilityGroup::GlowFlare => {
                !hands_occupied && matches!(active_item, Some(BeltItem::Flares { .. }))
            }
            ViewmodelVisibilityGroup::AcousticFoam => {
                !hands_occupied && matches!(active_item, Some(BeltItem::AcousticFoam { .. }))
            }
            ViewmodelVisibilityGroup::NoiseDecoy => {
                !hands_occupied && matches!(active_item, Some(BeltItem::NoiseDecoys { .. }))
            }
            ViewmodelVisibilityGroup::AidKit => {
                !hands_occupied && matches!(active_item, Some(BeltItem::AidKit))
            }
            ViewmodelVisibilityGroup::BonusCrystal => {
                !hands_occupied && matches!(active_item, Some(BeltItem::BonusLoot { .. }))
            }
        };

        let desired = if should_show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != desired {
            *vis = desired;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        health::LocalHealthPhase,
        inventory::{CarryHandState, PlayerInventory},
    };

    #[test]
    fn swing_recoil_and_bob_math_helpers_behave_smoothly() {
        let (pos_zero, rot_zero) = compute_walk_bob(1.0, 0.0);
        assert_eq!(pos_zero, Vec3::ZERO);
        assert_eq!(rot_zero, Quat::IDENTITY);

        let (pos_walk, _rot_walk) = compute_walk_bob(0.8, 1.0);
        assert!(pos_walk.length() > 0.005);
        assert!(pos_walk.y <= 0.0);

        let (swing_idle_pos, swing_idle_rot) = compute_pickaxe_swing_pose(0.0);
        assert_eq!(swing_idle_pos, Vec3::ZERO);
        assert_eq!(swing_idle_rot, Quat::IDENTITY);

        // Peak of the 0.28s swing arc moves forward (-Z) and down (-Y)
        let (swing_mid_pos, _swing_mid_rot) =
            compute_pickaxe_swing_pose(PICKAXE_SWING_DURATION_S * 0.62);
        assert!(swing_mid_pos.z < -0.05);
        assert!(swing_mid_pos.y < -0.03);

        let (recoil_idle_pos, _) = compute_foam_recoil_pose(0.0);
        assert_eq!(recoil_idle_pos, Vec3::ZERO);

        // Mid-recoil kicks backward (+Z)
        let (recoil_mid_pos, _) = compute_foam_recoil_pose(FOAM_RECOIL_DURATION_S * 0.5);
        assert!(recoil_mid_pos.z > 0.02);
    }

    #[test]
    fn viewmodel_plugin_attaches_and_updates_without_ecs_conflicts() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .insert_resource(AppScreen::MainMenu)
            .insert_resource(PlayerInventory::starter())
            .insert_resource(PlayerHealthState::default())
            .insert_resource(CoopMenuOpen(false))
            .insert_resource(ShopOverlayOpen(false))
            .insert_resource(SettingsMenuOpen(false))
            .add_plugins(ViewmodelPlugin);

        // Spawn a mock PlayerController camera entity with a child PlayerFlashlight
        app.world_mut()
            .spawn((
                Camera3d::default(),
                Transform::default(),
                PlayerController {
                    feet_pos_m: Vec3::ZERO,
                    velocity_m: Vec3::new(3.5, 0.0, 0.0),
                    yaw: 0.0,
                    pitch: 0.0,
                    grounded: true,
                    crouching: false,
                    footstep_accum_m: 0.0,
                    step_counter: 0,
                },
            ))
            .with_children(|parent| {
                parent.spawn((
                    Transform::default(),
                    PlayerFlashlight {
                        on: true,
                        battery: 1.0,
                    },
                ));
            });

        // Tick 1: in MainMenu, ViewmodelRoot attaches lazily and stays Hidden
        app.update();

        let mut root_q = app
            .world_mut()
            .query_filtered::<&Visibility, With<ViewmodelRoot>>();
        let root_vis = *root_q.single(app.world()).expect("ViewmodelRoot attached");
        assert_eq!(root_vis, Visibility::Hidden);

        // Switch to InGame: ViewmodelRoot becomes visible (Inherited)
        *app.world_mut().resource_mut::<AppScreen>() = AppScreen::InGame;
        app.update();

        let root_vis = *root_q.single(app.world()).expect("ViewmodelRoot exists");
        assert_eq!(root_vis, Visibility::Inherited);

        // Occupy hands with HeavyRelic and verify CarryArms shows while Pickaxe hides
        app.world_mut()
            .resource_mut::<PlayerInventory>()
            .carry_state = CarryHandState::HeavyRelic { item_id: 42 };
        app.update();

        let mut vis_q = app
            .world_mut()
            .query::<(&ViewmodelVisibilityGroup, &Visibility)>();
        for (group, vis) in vis_q.iter(app.world()) {
            match group {
                ViewmodelVisibilityGroup::CarryArms => {
                    assert_eq!(*vis, Visibility::Inherited);
                }
                ViewmodelVisibilityGroup::Pickaxe | ViewmodelVisibilityGroup::Flashlight => {
                    assert_eq!(*vis, Visibility::Hidden);
                }
                _ => {}
            }
        }

        // Transition player to Dead: entire ViewmodelRoot hides cleanly
        app.world_mut().resource_mut::<PlayerHealthState>().phase = LocalHealthPhase::Dead;
        app.update();

        let root_vis = *root_q.single(app.world()).expect("ViewmodelRoot exists");
        assert_eq!(root_vis, Visibility::Hidden);
    }
}
