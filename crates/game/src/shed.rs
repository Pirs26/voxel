//! Base Shed physical stations (Shop Terminal & Battery Charger Bench) and
//! crosshair pointer raycast interaction detection (including the interior Elevator Lever).

use bevy::prelude::*;
use voxel_core::GeneratedLevelInfo;
use voxel_net::{ElevatorPhase, WorldItemKind};

use crate::{elevator::ELEVATOR_WINCH_DURATION_S, physics::ray_aabb_intersect};

/// Maximum pointer raycast distance in meters for interacting with stations, levers, and items.
pub const INTERACT_RAY_MAX_DIST_M: f32 = 3.4;

/// Interactive physical stations in the persistent Base Shed and inside the Elevator cage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShedStation {
    ElevatorLeverShed,
    ElevatorWinchCave,
    ShopTerminal,
    ChargerBench,
}

/// Visual accent color/style for the graphical crosshair & floating target badge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TargetAccent {
    #[default]
    None,
    Interact,
    Danger,
    Blocked,
}

/// Concrete object currently under the center-of-screen crosshair pointer.
#[derive(Clone, Debug, PartialEq)]
pub enum LookTargetKind {
    Station(ShedStation),
    WorldItem {
        item_id: u32,
        kind: WorldItemKind,
        carriers: usize,
    },
    DownedPlayer {
        player_id: u16,
        name: String,
    },
    Stalker {
        monster_id: u16,
        hp: u8,
        max_hp: u8,
        stunned: bool,
    },
    BreakableVoxel,
}

/// Center-of-screen crosshair look-at target (updated every frame by voxel-occluded raycasting).
#[derive(Resource, Default, Clone, Debug)]
pub struct CrosshairTargetState {
    pub target: Option<LookTargetKind>,
    pub key_badge: String,
    pub label: String,
    pub sublabel: String,
    pub accent: TargetAccent,
}

impl CrosshairTargetState {
    pub fn clear(&mut self) {
        self.target = None;
        self.key_badge.clear();
        self.label.clear();
        self.sublabel.clear();
        self.accent = TargetAccent::None;
    }
}

/// Whether the Base Shed Supply Shop & Depth Selector overlay is currently open.
#[derive(Resource, Default, Clone, Debug)]
pub struct ShopOverlayOpen(pub bool);

/// Legacy/compatibility summary resource (kept lightweight for any external readers).
#[derive(Resource, Default, Clone, Debug)]
pub struct ShedStationState {
    pub nearest: Option<ShedStation>,
    pub nearest_dist_m: f32,
    pub prompt: Option<String>,
}

/// Marker component for persistent Base Shed station landmark visuals.
#[derive(Component)]
pub struct ShedLandmarkVisual;

/// Casts the center-of-screen camera ray `(origin_m, dir)` up to `max_dist_m` against the
/// physical AABBs of the interior Elevator Lever, Shed Shop Terminal, and Battery Charger Bench.
///
/// Returns `(station, short_label, sublabel, hit_dist_m, is_blocked)` for the closest hit.
#[allow(clippy::too_many_arguments)]
pub fn raycast_shed_station(
    origin_m: Vec3,
    dir: Vec3,
    max_dist_m: f32,
    info: &GeneratedLevelInfo,
    elevator_lever_pos_m: Vec3,
    elevator_phase: ElevatorPhase,
    selected_depth_tier: u8,
    missing_players_in_cage: usize,
) -> Option<(ShedStation, String, String, f32, bool)> {
    let mut best: Option<(f32, ShedStation, String, String, bool)> = None;

    let mut consider_box = |station: ShedStation,
                            min_m: Vec3,
                            max_m: Vec3,
                            label: String,
                            sublabel: String,
                            blocked: bool| {
        if let Some(t) = ray_aabb_intersect(origin_m, dir, min_m, max_m, max_dist_m)
            && best.as_ref().is_none_or(|(best_t, _, _, _, _)| t < *best_t)
        {
            best = Some((t, station, label, sublabel, blocked));
        }
    };

    // Interior Elevator Lever AABB (box + handle mounted inside the moving cage)
    let lever_min = elevator_lever_pos_m + Vec3::new(-0.36, 0.0, -0.36);
    let lever_max = elevator_lever_pos_m + Vec3::new(0.36, 1.50, 0.36);
    if elevator_phase == ElevatorPhase::ShedIdle {
        if missing_players_in_cage > 0 {
            consider_box(
                ShedStation::ElevatorLeverShed,
                lever_min,
                lever_max,
                "Elevator Lever".to_string(),
                format!("Wait: {missing_players_in_cage} player(s) outside"),
                true,
            );
        } else {
            consider_box(
                ShedStation::ElevatorLeverShed,
                lever_min,
                lever_max,
                format!("Descend — Tier {selected_depth_tier}"),
                "All aboard".to_string(),
                false,
            );
        }
    } else if elevator_phase == ElevatorPhase::CaveIdle {
        let winch_secs = ELEVATOR_WINCH_DURATION_S.round() as u32;
        if missing_players_in_cage > 0 {
            consider_box(
                ShedStation::ElevatorWinchCave,
                lever_min,
                lever_max,
                "Extraction Lever".to_string(),
                format!("Wait: {missing_players_in_cage} player(s) outside"),
                true,
            );
        } else {
            consider_box(
                ShedStation::ElevatorWinchCave,
                lever_min,
                lever_max,
                "Start Winch".to_string(),
                format!("LOUD • {winch_secs}s holdout"),
                false,
            );
        }
    }

    // Shop Terminal console AABB (matches physical mesh: 0.95 x 1.65 x 0.55)
    let shop_min = info.shop_terminal_pos_m + Vec3::new(-0.55, 0.0, -0.36);
    let shop_max = info.shop_terminal_pos_m + Vec3::new(0.55, 1.72, 0.36);
    consider_box(
        ShedStation::ShopTerminal,
        shop_min,
        shop_max,
        "Supply Terminal".to_string(),
        "Gear & Depth Tier".to_string(),
        false,
    );

    // Battery Charger Bench AABB (matches physical mesh: 1.30 x 0.95 x 0.66)
    let charger_min = info.charger_bench_pos_m + Vec3::new(-0.72, 0.0, -0.42);
    let charger_max = info.charger_bench_pos_m + Vec3::new(0.72, 1.02, 0.42);
    consider_box(
        ShedStation::ChargerBench,
        charger_min,
        charger_max,
        "Battery Charger".to_string(),
        "Recharge all cells".to_string(),
        false,
    );

    best.map(|(t, station, label, sublabel, blocked)| (station, label, sublabel, t, blocked))
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
    fn raycast_detects_interior_lever_and_blocks_when_players_outside() {
        let info = sample_level();
        let elev = ElevatorState::from_level_info(&info, 4242);
        let lever_pos = elev.lever_world_pos_m();
        let eye = lever_pos + Vec3::new(-1.4, 1.1, 0.0);
        let aim = (lever_pos + Vec3::Y * 0.9 - eye).normalize();

        // Pointing at lever with all players aboard
        let hit = raycast_shed_station(
            eye,
            aim,
            INTERACT_RAY_MAX_DIST_M,
            &info,
            lever_pos,
            ElevatorPhase::ShedIdle,
            2,
            0,
        );
        assert_eq!(hit.as_ref().map(|h| h.0), Some(ShedStation::ElevatorLeverShed));
        assert!(!hit.unwrap().4);

        // Pointing AWAY from lever while standing right next to it -> None!
        let miss = raycast_shed_station(
            eye,
            -aim,
            INTERACT_RAY_MAX_DIST_M,
            &info,
            lever_pos,
            ElevatorPhase::ShedIdle,
            2,
            0,
        );
        assert!(miss.is_none());

        // Missing 1 player -> blocked = true
        let waiting = raycast_shed_station(
            eye,
            aim,
            INTERACT_RAY_MAX_DIST_M,
            &info,
            lever_pos,
            ElevatorPhase::ShedIdle,
            2,
            1,
        );
        assert!(waiting.unwrap().4);
    }

    #[test]
    fn raycast_shop_and_charger_only_trigger_when_pointer_aims_at_them() {
        let info = sample_level();
        let elev = ElevatorState::from_level_info(&info, 4242);
        let lever_pos = elev.lever_world_pos_m();

        let eye = info.shop_terminal_pos_m + Vec3::new(0.0, 1.3, 1.8);
        let aim_at_shop = (info.shop_terminal_pos_m + Vec3::Y * 1.1 - eye).normalize();
        assert_eq!(
            raycast_shed_station(
                eye,
                aim_at_shop,
                INTERACT_RAY_MAX_DIST_M,
                &info,
                lever_pos,
                ElevatorPhase::ShedIdle,
                1,
                0,
            )
            .map(|h| h.0),
            Some(ShedStation::ShopTerminal)
        );
        // Looking away from shop even when standing close does NOT hit the shop
        assert!(
            raycast_shed_station(
                eye,
                Vec3::Z,
                INTERACT_RAY_MAX_DIST_M,
                &info,
                lever_pos,
                ElevatorPhase::ShedIdle,
                1,
                0,
            )
            .is_none()
        );

        let eye_c = info.charger_bench_pos_m + Vec3::new(0.0, 1.3, 1.5);
        let aim_at_charger = (info.charger_bench_pos_m + Vec3::Y * 0.6 - eye_c).normalize();
        assert_eq!(
            raycast_shed_station(
                eye_c,
                aim_at_charger,
                INTERACT_RAY_MAX_DIST_M,
                &info,
                lever_pos,
                ElevatorPhase::ShedIdle,
                1,
                0,
            )
            .map(|h| h.0),
            Some(ShedStation::ChargerBench)
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
