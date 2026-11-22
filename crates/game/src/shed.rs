//! Base Shed physical stations, cave extraction winch visuals, and proximity interaction prompts.

use bevy::prelude::*;
use voxel_core::GeneratedLevelInfo;
use voxel_net::ElevatorPhase;

/// Maximum interaction radius in meters for Base Shed interactive stations.
pub const SHED_STATION_INTERACT_RANGE_M: f32 = 2.6;
/// Maximum interaction radius in meters for the underground cave extraction winch.
pub const CAVE_WINCH_INTERACT_RANGE_M: f32 = 2.8;

/// Interactive physical stations in the persistent Base Shed and Cave landing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShedStation {
    ElevatorLeverShed,
    ElevatorWinchCave,
    ShopTerminal,
    ChargerBench,
}

/// Whether the Base Shed Supply Shop & Depth Selector overlay is currently open.
#[derive(Resource, Default, Clone, Debug)]
pub struct ShopOverlayOpen(pub bool);

/// Nearest interactive Shed/Cave station and formatted prompt for the HUD.
#[derive(Resource, Default, Clone, Debug)]
pub struct ShedStationState {
    pub nearest: Option<ShedStation>,
    pub prompt: Option<String>,
}

/// Marker component for persistent Base Shed station landmark visuals.
#[derive(Component)]
pub struct ShedLandmarkVisual;

/// Marker component for the underground cave extraction winch station visual.
#[derive(Component)]
pub struct CaveWinchVisual;

/// Detects the nearest interactive Shed or Cave station within interaction range of `player_pos_m`.
///
/// - `ElevatorLeverShed` is active only when `elevator_phase == ElevatorPhase::ShedIdle` (`<= 2.6 m`).
/// - `ElevatorWinchCave` is active only when `elevator_phase == ElevatorPhase::CaveIdle` (`<= 2.8 m`).
/// - `ShopTerminal` and `ChargerBench` are always interactable when within `2.6 m`.
pub fn detect_nearest_station(
    player_pos_m: Vec3,
    info: &GeneratedLevelInfo,
    elevator_phase: ElevatorPhase,
    selected_depth_tier: u8,
) -> Option<(ShedStation, String)> {
    let mut best: Option<(f32, ShedStation, String)> = None;

    let mut consider = |station: ShedStation, pos: Vec3, max_range: f32, prompt: String| {
        let dist = player_pos_m.distance(pos);
        if dist <= max_range && best.as_ref().is_none_or(|(best_dist, _, _)| dist < *best_dist) {
            best = Some((dist, station, prompt));
        }
    };

    if elevator_phase == ElevatorPhase::ShedIdle {
        consider(
            ShedStation::ElevatorLeverShed,
            info.elevator_lever_shed_pos_m,
            SHED_STATION_INTERACT_RANGE_M,
            format!("[E] Pull Elevator Lever — Descend to Depth Tier {selected_depth_tier}"),
        );
    }

    if elevator_phase == ElevatorPhase::CaveIdle {
        consider(
            ShedStation::ElevatorWinchCave,
            info.elevator_winch_cave_pos_m,
            CAVE_WINCH_INTERACT_RANGE_M,
            "[E] Activate Extraction Winch (LOUD! 30s)".to_string(),
        );
    }

    consider(
        ShedStation::ShopTerminal,
        info.shop_terminal_pos_m,
        SHED_STATION_INTERACT_RANGE_M,
        "[E] Open Shed Supply Shop & Depth Selector".to_string(),
    );

    consider(
        ShedStation::ChargerBench,
        info.charger_bench_pos_m,
        SHED_STATION_INTERACT_RANGE_M,
        "[E] Recharge All Batteries (or drop cells on pad)".to_string(),
    );

    best.map(|(_, station, prompt)| (station, prompt))
}

/// Spawns readable physical station landmarks and dedicated child `PointLight` beacons
/// at the Shed Shop Terminal, Battery Charger Bench, Shed Elevator Lever, and Cave Extraction Winch.
///
/// Every `PointLight` is placed on a dedicated child entity with NO `Mesh3d` to prevent Bevy
/// frustum culling from clipping the light against a small prop mesh AABB.
pub fn spawn_shed_landmarks(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    info: &GeneratedLevelInfo,
) {
    let steel_dark = materials.add(StandardMaterial {
        base_color: Color::srgb(0.20, 0.22, 0.25),
        metallic: 0.72,
        perceptual_roughness: 0.42,
        ..default()
    });
    let steel_mid = materials.add(StandardMaterial {
        base_color: Color::srgb(0.32, 0.35, 0.38),
        metallic: 0.65,
        perceptual_roughness: 0.48,
        ..default()
    });

    // 1. Shop Terminal: steel console + glowing emerald monitor + dedicated child PointLight
    let shop_base_mesh = meshes.add(Cuboid::new(0.95, 1.05, 0.55));
    let shop_hood_mesh = meshes.add(Cuboid::new(0.90, 0.60, 0.42));
    let shop_screen_mesh = meshes.add(Cuboid::new(0.74, 0.44, 0.06));
    let emerald_screen_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.10, 0.92, 0.48),
        emissive: LinearRgba::new(0.35, 4.8, 1.9, 1.0),
        perceptual_roughness: 0.18,
        ..default()
    });

    commands
        .spawn((
            Transform::from_translation(info.shop_terminal_pos_m),
            Visibility::default(),
            ShedLandmarkVisual,
        ))
        .with_children(|parent| {
            parent.spawn((
                Mesh3d(shop_base_mesh),
                MeshMaterial3d(steel_dark.clone()),
                Transform::from_xyz(0.0, 0.525, 0.0),
            ));
            parent.spawn((
                Mesh3d(shop_hood_mesh),
                MeshMaterial3d(steel_mid.clone()),
                Transform::from_xyz(0.0, 1.32, -0.04),
            ));
            parent.spawn((
                Mesh3d(shop_screen_mesh),
                MeshMaterial3d(emerald_screen_mat),
                Transform::from_xyz(0.0, 1.34, 0.18),
            ));
            // Dedicated child PointLight with NO Mesh3d
            parent.spawn((
                PointLight {
                    color: Color::srgb(0.22, 1.0, 0.58),
                    intensity: 26_000.0,
                    range: 9.0,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, 1.45, 0.45),
            ));
        });

    // 2. Battery Charger Bench: charging dock + glowing cyan pad + dedicated child PointLight
    let bench_base_mesh = meshes.add(Cuboid::new(1.30, 0.82, 0.66));
    let pad_mesh = meshes.add(Cuboid::new(1.06, 0.06, 0.48));
    let cell_slot_mesh = meshes.add(Cuboid::new(0.16, 0.10, 0.16));
    let cyan_pad_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.10, 0.86, 0.98),
        emissive: LinearRgba::new(0.25, 3.8, 5.2, 1.0),
        perceptual_roughness: 0.22,
        ..default()
    });

    commands
        .spawn((
            Transform::from_translation(info.charger_bench_pos_m),
            Visibility::default(),
            ShedLandmarkVisual,
        ))
        .with_children(|parent| {
            parent.spawn((
                Mesh3d(bench_base_mesh),
                MeshMaterial3d(steel_dark.clone()),
                Transform::from_xyz(0.0, 0.41, 0.0),
            ));
            parent.spawn((
                Mesh3d(pad_mesh),
                MeshMaterial3d(cyan_pad_mat),
                Transform::from_xyz(0.0, 0.85, 0.0),
            ));
            for x_off in [-0.32, 0.0, 0.32] {
                parent.spawn((
                    Mesh3d(cell_slot_mesh.clone()),
                    MeshMaterial3d(steel_mid.clone()),
                    Transform::from_xyz(x_off, 0.89, -0.14),
                ));
            }
            // Dedicated child PointLight with NO Mesh3d
            parent.spawn((
                PointLight {
                    color: Color::srgb(0.25, 0.90, 1.0),
                    intensity: 24_000.0,
                    range: 8.5,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, 1.25, 0.0),
            ));
        });

    // 3. Shed Elevator Lever: industrial steel lever stand + glowing amber handle + dedicated child PointLight
    let lever_post_mesh = meshes.add(Cuboid::new(0.42, 1.08, 0.42));
    let lever_arm_mesh = meshes.add(Cylinder::new(0.032, 0.56));
    let lever_handle_mesh = meshes.add(Cuboid::new(0.34, 0.09, 0.09));
    let amber_handle_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.72, 0.14),
        emissive: LinearRgba::new(5.5, 3.0, 0.35, 1.0),
        perceptual_roughness: 0.25,
        ..default()
    });

    commands
        .spawn((
            Transform::from_translation(info.elevator_lever_shed_pos_m),
            Visibility::default(),
            ShedLandmarkVisual,
        ))
        .with_children(|parent| {
            parent.spawn((
                Mesh3d(lever_post_mesh),
                MeshMaterial3d(steel_dark.clone()),
                Transform::from_xyz(0.0, 0.54, 0.0),
            ));
            parent.spawn((
                Mesh3d(lever_arm_mesh),
                MeshMaterial3d(steel_mid.clone()),
                Transform::from_xyz(0.0, 1.24, 0.08)
                    .with_rotation(Quat::from_rotation_x(0.35)),
            ));
            parent.spawn((
                Mesh3d(lever_handle_mesh),
                MeshMaterial3d(amber_handle_mat),
                Transform::from_xyz(0.0, 1.50, 0.18),
            ));
            // Dedicated child PointLight with NO Mesh3d
            parent.spawn((
                PointLight {
                    color: Color::srgb(1.0, 0.72, 0.22),
                    intensity: 24_000.0,
                    range: 8.5,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, 1.65, 0.18),
            ));
        });

    // 4. Cave Extraction Winch: steel winch box + cable drum + glowing crimson call beacon + dedicated child PointLight
    let winch_box_mesh = meshes.add(Cuboid::new(0.68, 1.12, 0.56));
    let winch_drum_mesh = meshes.add(Cylinder::new(0.22, 0.52));
    let beacon_mesh = meshes.add(Sphere::new(0.14));
    let crimson_beacon_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.98, 0.14, 0.16),
        emissive: LinearRgba::new(7.5, 0.55, 0.55, 1.0),
        perceptual_roughness: 0.20,
        ..default()
    });

    commands
        .spawn((
            Transform::from_translation(info.elevator_winch_cave_pos_m),
            Visibility::default(),
            CaveWinchVisual,
        ))
        .with_children(|parent| {
            parent.spawn((
                Mesh3d(winch_box_mesh),
                MeshMaterial3d(steel_dark),
                Transform::from_xyz(0.0, 0.56, 0.0),
            ));
            parent.spawn((
                Mesh3d(winch_drum_mesh),
                MeshMaterial3d(steel_mid),
                Transform::from_xyz(0.0, 0.78, 0.24)
                    .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
            ));
            parent.spawn((
                Mesh3d(beacon_mesh),
                MeshMaterial3d(crimson_beacon_mat),
                Transform::from_xyz(0.0, 1.30, 0.0),
            ));
            // Dedicated child PointLight with NO Mesh3d
            parent.spawn((
                PointLight {
                    color: Color::srgb(1.0, 0.18, 0.18),
                    intensity: 34_000.0,
                    range: 12.0,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, 1.50, 0.0),
            ));
        });
}

#[cfg(test)]
mod tests {
    use voxel_core::{VoxelConfig, generate_world_with_tier};

    use super::*;

    fn sample_level() -> GeneratedLevelInfo {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        generate_world_with_tier(4242, 1, cfg).1
    }

    #[test]
    fn detects_shed_elevator_lever_only_when_shed_idle() {
        let info = sample_level();
        let near_lever = info.elevator_lever_shed_pos_m + Vec3::new(0.5, 0.0, 0.5);

        let hit = detect_nearest_station(near_lever, &info, ElevatorPhase::ShedIdle, 2);
        assert_eq!(
            hit,
            Some((
                ShedStation::ElevatorLeverShed,
                "[E] Pull Elevator Lever — Descend to Depth Tier 2".to_string()
            ))
        );

        // Inactive during other phases
        for phase in [
            ElevatorPhase::Descending,
            ElevatorPhase::CaveIdle,
            ElevatorPhase::WinchActive,
            ElevatorPhase::Ascending,
        ] {
            let result = detect_nearest_station(near_lever, &info, phase, 2);
            assert_ne!(
                result.map(|(s, _)| s),
                Some(ShedStation::ElevatorLeverShed)
            );
        }
    }

    #[test]
    fn detects_cave_extraction_winch_only_when_cave_idle() {
        let info = sample_level();
        let near_winch = info.elevator_winch_cave_pos_m + Vec3::new(1.2, 0.0, 0.0);

        let hit = detect_nearest_station(near_winch, &info, ElevatorPhase::CaveIdle, 1);
        assert_eq!(
            hit,
            Some((
                ShedStation::ElevatorWinchCave,
                "[E] Activate Extraction Winch (LOUD! 30s)".to_string()
            ))
        );

        // Inactive when winch is already running or elevator is in Shed/moving
        for phase in [
            ElevatorPhase::ShedIdle,
            ElevatorPhase::Descending,
            ElevatorPhase::WinchActive,
            ElevatorPhase::Ascending,
        ] {
            assert_eq!(
                detect_nearest_station(near_winch, &info, phase, 1),
                None
            );
        }

        // Out of 2.8m range returns None
        let too_far = info.elevator_winch_cave_pos_m + Vec3::new(3.1, 0.0, 0.0);
        assert_eq!(
            detect_nearest_station(too_far, &info, ElevatorPhase::CaveIdle, 1),
            None
        );
    }

    #[test]
    fn detects_shop_terminal_and_charger_bench_across_phases() {
        let info = sample_level();

        let near_shop = info.shop_terminal_pos_m + Vec3::new(-0.8, 0.0, 0.4);
        assert_eq!(
            detect_nearest_station(near_shop, &info, ElevatorPhase::ShedIdle, 3),
            Some((
                ShedStation::ShopTerminal,
                "[E] Open Shed Supply Shop & Depth Selector".to_string()
            ))
        );
        assert_eq!(
            detect_nearest_station(near_shop, &info, ElevatorPhase::Ascending, 3),
            Some((
                ShedStation::ShopTerminal,
                "[E] Open Shed Supply Shop & Depth Selector".to_string()
            ))
        );

        let near_charger = info.charger_bench_pos_m + Vec3::new(0.0, 0.0, 1.0);
        assert_eq!(
            detect_nearest_station(near_charger, &info, ElevatorPhase::ShedIdle, 1),
            Some((
                ShedStation::ChargerBench,
                "[E] Recharge All Batteries (or drop cells on pad)".to_string()
            ))
        );

        // Far away in the cavern returns None
        assert_eq!(
            detect_nearest_station(info.grand_cavern_pos_m, &info, ElevatorPhase::CaveIdle, 1),
            None
        );
    }

    #[test]
    fn spawn_shed_landmarks_creates_stations_with_isolated_point_lights() {
        let info = sample_level();
        let mut world = World::new();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();

        {
            let mut commands = world.commands();
            spawn_shed_landmarks(&mut commands, &mut meshes, &mut materials, &info);
        }
        world.flush();

        let mut shed_q = world.query::<&ShedLandmarkVisual>();
        assert_eq!(shed_q.iter(&world).count(), 3);

        let mut winch_q = world.query::<&CaveWinchVisual>();
        assert_eq!(winch_q.iter(&world).count(), 1);

        let mut light_q = world.query::<(&PointLight, Option<&Mesh3d>)>();
        let lights: Vec<_> = light_q.iter(&world).collect();
        assert_eq!(lights.len(), 4);
        for (_, mesh) in lights {
            assert!(
                mesh.is_none(),
                "Shed landmark PointLight must live on a dedicated child entity with no Mesh3d"
            );
        }
    }
}
