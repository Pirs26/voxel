//! Base Shed physical stations (Shop Terminal & Battery Charger Bench) and
//! proximity interaction detection (including the interior Elevator Lever).

use bevy::prelude::*;
use voxel_core::GeneratedLevelInfo;
use voxel_net::ElevatorPhase;

use crate::elevator::ELEVATOR_WINCH_DURATION_S;

/// Tight interaction radius in meters for Base Shed Shop Terminal & Charger Bench
/// so dropped items next to/behind them can always be picked up cleanly!
pub const SHED_STATION_INTERACT_RANGE_M: f32 = 1.55;
/// Interaction radius in meters for the Elevator Lever mounted inside the elevator cage.
pub const ELEVATOR_LEVER_INTERACT_RANGE_M: f32 = 2.15;

/// Interactive physical stations in the persistent Base Shed and inside the Elevator cage.
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

/// Nearest interactive Shed/Elevator station and formatted prompt for the HUD.
#[derive(Resource, Default, Clone, Debug)]
pub struct ShedStationState {
    pub nearest: Option<ShedStation>,
    pub nearest_dist_m: f32,
    pub prompt: Option<String>,
}

/// Marker component for persistent Base Shed station landmark visuals.
#[derive(Component)]
pub struct ShedLandmarkVisual;

/// Detects the nearest interactive Shed station or interior Elevator Lever within range.
///
/// Returns `(station, prompt, distance_m)`:
/// - Interior `ElevatorLeverShed` / `ElevatorWinchCave` is checked against `elevator_lever_pos_m`
///   and reports a waiting prompt if `missing_players_in_cage > 0`.
/// - `ShopTerminal` and `ChargerBench` use a tight `1.55 m` range so nearby dropped items
///   remain easy to pick up.
pub fn detect_nearest_station(
    player_pos_m: Vec3,
    info: &GeneratedLevelInfo,
    elevator_lever_pos_m: Vec3,
    elevator_phase: ElevatorPhase,
    selected_depth_tier: u8,
    missing_players_in_cage: usize,
) -> Option<(ShedStation, String, f32)> {
    let mut best: Option<(f32, ShedStation, String)> = None;

    let mut consider = |station: ShedStation, pos: Vec3, max_range: f32, prompt: String| {
        let dist = player_pos_m.distance(pos);
        if dist <= max_range && best.as_ref().is_none_or(|(best_dist, _, _)| dist < *best_dist) {
            best = Some((dist, station, prompt));
        }
    };

    if elevator_phase == ElevatorPhase::ShedIdle {
        let prompt = if missing_players_in_cage > 0 {
            format!(
                "[Elevator Lever] Waiting for {missing_players_in_cage} player(s) to board the elevator"
            )
        } else {
            format!("[E] Pull Elevator Lever — Descend to Depth Tier {selected_depth_tier}")
        };
        consider(
            ShedStation::ElevatorLeverShed,
            elevator_lever_pos_m,
            ELEVATOR_LEVER_INTERACT_RANGE_M,
            prompt,
        );
    }

    if elevator_phase == ElevatorPhase::CaveIdle {
        let winch_secs = ELEVATOR_WINCH_DURATION_S.round() as u32;
        let prompt = if missing_players_in_cage > 0 {
            format!(
                "[Elevator Lever] Waiting for {missing_players_in_cage} player(s) to board before extraction"
            )
        } else {
            format!("[E] Pull Lever — Activate Extraction Winch (LOUD! {winch_secs}s)")
        };
        consider(
            ShedStation::ElevatorWinchCave,
            elevator_lever_pos_m,
            ELEVATOR_LEVER_INTERACT_RANGE_M,
            prompt,
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

    best.map(|(dist, station, prompt)| (station, prompt, dist))
}

/// Spawns readable physical station landmarks and dedicated child `PointLight` beacons
/// for the Shed Shop Terminal and Battery Charger Bench.
///
/// The Elevator Lever is spawned inside the moving elevator cage itself (`elevator::spawn_elevator_cage`).
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
                MeshMaterial3d(steel_dark),
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
}

#[cfg(test)]
mod tests {
    use voxel_core::{VoxelConfig, generate_world_with_tier};

    use super::*;
    use crate::elevator::ElevatorState;

    fn sample_level() -> GeneratedLevelInfo {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        generate_world_with_tier(4242, 1, cfg).1
    }

    #[test]
    fn detects_interior_elevator_lever_and_reports_missing_teammates() {
        let info = sample_level();
        let elev = ElevatorState::from_level_info(&info, 4242);
        let lever_pos = elev.lever_world_pos_m();

        // All players aboard -> ready to pull
        let hit = detect_nearest_station(
            lever_pos + Vec3::new(-0.4, 0.0, -0.4),
            &info,
            lever_pos,
            ElevatorPhase::ShedIdle,
            2,
            0,
        );
        assert_eq!(hit.as_ref().map(|(s, _, _)| *s), Some(ShedStation::ElevatorLeverShed));
        assert!(hit.unwrap().1.contains("Pull Elevator Lever"));

        // Missing 1 player -> shows waiting prompt
        let waiting = detect_nearest_station(
            lever_pos + Vec3::new(-0.4, 0.0, -0.4),
            &info,
            lever_pos,
            ElevatorPhase::ShedIdle,
            2,
            1,
        );
        assert!(waiting.unwrap().1.contains("Waiting for 1 player(s)"));

        // CaveIdle uses the exact same interior lever for extraction winch
        let cave_lever = Vec3::new(lever_pos.x, info.cave_floor_y_m, lever_pos.z);
        let winch = detect_nearest_station(
            cave_lever + Vec3::new(-0.4, 0.0, -0.4),
            &info,
            cave_lever,
            ElevatorPhase::CaveIdle,
            1,
            0,
        );
        assert_eq!(winch.as_ref().map(|(s, _, _)| *s), Some(ShedStation::ElevatorWinchCave));
        assert!(winch.unwrap().1.contains("Extraction Winch"));
    }

    #[test]
    fn shop_and_charger_use_tight_hitboxes_so_nearby_items_arent_blocked() {
        let info = sample_level();
        let elev = ElevatorState::from_level_info(&info, 4242);
        let lever_pos = elev.lever_world_pos_m();

        // Directly in front of shop (0.9m) triggers shop
        let near_shop = info.shop_terminal_pos_m + Vec3::new(-0.8, 0.0, 0.4);
        assert_eq!(
            detect_nearest_station(near_shop, &info, lever_pos, ElevatorPhase::ShedIdle, 1, 0)
                .map(|(s, _, _)| s),
            Some(ShedStation::ShopTerminal)
        );

        // 1.8m away from shop must NOT hijack interactions
        let behind_shop = info.shop_terminal_pos_m + Vec3::new(-1.8, 0.0, 0.0);
        assert_eq!(
            detect_nearest_station(behind_shop, &info, lever_pos, ElevatorPhase::ShedIdle, 1, 0),
            None
        );

        // Directly at charger (1.0m) triggers charger, 1.9m away does not
        let near_charger = info.charger_bench_pos_m + Vec3::new(0.0, 0.0, 1.0);
        assert_eq!(
            detect_nearest_station(near_charger, &info, lever_pos, ElevatorPhase::ShedIdle, 1, 0)
                .map(|(s, _, _)| s),
            Some(ShedStation::ChargerBench)
        );
        let beside_charger = info.charger_bench_pos_m + Vec3::new(1.9, 0.0, 0.0);
        assert_eq!(
            detect_nearest_station(beside_charger, &info, lever_pos, ElevatorPhase::ShedIdle, 1, 0),
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
        assert_eq!(shed_q.iter(&world).count(), 2);

        let mut light_q = world.query::<(&PointLight, Option<&Mesh3d>)>();
        let lights: Vec<_> = light_q.iter(&world).collect();
        assert_eq!(lights.len(), 2);
        for (_, mesh) in lights {
            assert!(
                mesh.is_none(),
                "Shed landmark PointLight must live on a dedicated child entity with no Mesh3d"
            );
        }
    }
}
