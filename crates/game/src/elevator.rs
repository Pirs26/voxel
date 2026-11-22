//! Seamless mineshaft elevator state machine, async underground cave generation,
//! Bevy 0.20 industrial cage spawner, and depth-based ambient daylight falloff.

use std::sync::{Arc, Mutex, RwLock};

use bevy::prelude::*;
use voxel_acoustics::{AcousticEvent, AcousticEventKind, Bands3};
use voxel_audio_dsp::SynthClip;
use voxel_core::{GeneratedLevelInfo, VoxelWorld};
use voxel_net::ElevatorPhase;

/// Duration of the seamless shaft descent from Base Shed to Cave landing (seconds).
pub const ELEVATOR_DESCENT_DURATION_S: f32 = 10.0;
/// Duration of the loud extraction winch holdout before doors slam shut (seconds).
pub const ELEVATOR_WINCH_DURATION_S: f32 = 30.0;
/// Duration of the shaft ascent back up to the Base Shed (seconds).
pub const ELEVATOR_ASCENT_DURATION_S: f32 = 10.0;
/// Half-width / half-depth of the walkable elevator cage interior in meters.
pub const ELEVATOR_CAGE_HALF_EXTENT_M: f32 = 2.65;
/// Physical thickness of the steel elevator platform floor plate in meters.
pub const ELEVATOR_FLOOR_THICKNESS_M: f32 = 0.24;

/// Local Y offset of the `+X` sliding security door when closed.
pub const ELEVATOR_DOOR_CLOSED_Y_M: f32 = 1.50;
/// Local Y offset of the `+X` sliding security door when raised open.
pub const ELEVATOR_DOOR_OPEN_Y_M: f32 = 4.35;

const CREAK_INTERVAL_S: f32 = 2.4;
const WINCH_PULSE_INTERVAL_S: f32 = 1.35;
const SHED_AMBIENT_BRIGHTNESS: f32 = 52.0;
const CAVE_AMBIENT_BRIGHTNESS: f32 = 14.0;

/// Buffered message dispatched when a player pulls the Shed descent lever or Cave winch.
#[derive(Message, Clone, Copy, Debug)]
pub struct TriggerElevatorLeverEvent;

/// Buffered message emitted once the elevator finishes descending and cave generation is ready.
#[derive(Message, Clone, Copy, Debug)]
#[allow(dead_code)]
pub struct ElevatorArrivedCaveEvent {
    pub depth_tier: u8,
    pub cave_seed: u64,
}

/// Buffered message emitted once the elevator returns to the surface Base Shed.
#[derive(Message, Clone, Copy, Debug)]
#[allow(dead_code)]
pub struct ElevatorReturnedShedEvent {
    pub depth_tier: u8,
}

/// Marker component for the root transform entity of the moving elevator cage.
#[derive(Component)]
pub struct ElevatorCageRoot;

/// Marker component for the sliding steel security door on the `+X` boarding face.
#[derive(Component)]
pub struct ElevatorDoorVisual;

/// Marker component for the dedicated child `PointLight` illuminating the cage interior.
#[derive(Component)]
pub struct ElevatorBulbLight;

/// Shared handle holding an asynchronously generated underground cave & level metadata.
type PendingCaveGen = Arc<Mutex<Option<(VoxelWorld, GeneratedLevelInfo)>>>;

/// Authoritative runtime state of the seamless mineshaft elevator.
#[derive(Resource)]
pub struct ElevatorState {
    pub phase: ElevatorPhase,
    pub platform_y_m: f32,
    pub prev_platform_y_m: f32,
    pub phase_timer_s: f32,
    pub depth_tier: u8,
    pub cave_seed: u64,
    pub doors_closed: bool,
    pub cable_shudder_active: bool,
    pub creak_timer_s: f32,
    pub winch_pulse_timer_s: f32,
    pub sync_timer_s: f32,
    pub top_y_m: f32,
    pub bottom_y_m: f32,
    pub center_xz_m: Vec2,
    pending_gen: Option<PendingCaveGen>,
}

impl ElevatorState {
    /// Initializes the elevator docked at the Base Shed floor in [`ElevatorPhase::ShedIdle`].
    pub fn from_level_info(info: &GeneratedLevelInfo, seed: u64) -> Self {
        Self {
            phase: ElevatorPhase::ShedIdle,
            platform_y_m: info.shed_floor_y_m,
            prev_platform_y_m: info.shed_floor_y_m,
            phase_timer_s: 0.0,
            depth_tier: info.depth_tier.clamp(1, 3),
            cave_seed: seed,
            doors_closed: false,
            cable_shudder_active: false,
            creak_timer_s: 0.0,
            winch_pulse_timer_s: 0.0,
            sync_timer_s: 0.0,
            top_y_m: info.shed_floor_y_m,
            bottom_y_m: info.cave_floor_y_m,
            center_xz_m: Vec2::new(info.elevator_top_pos_m.x, info.elevator_top_pos_m.z),
            pending_gen: None,
        }
    }

    /// World-space center of the elevator cage interior (roughly chest/head height) in meters.
    #[inline]
    pub fn cage_center_pos_m(&self) -> Vec3 {
        Vec3::new(self.center_xz_m.x, self.platform_y_m + 1.4, self.center_xz_m.y)
    }

    /// Returns `true` if `pos_m` lies within the elevator cage footprint and vertical envelope.
    #[inline]
    pub fn is_pos_inside_cage(&self, pos_m: Vec3) -> bool {
        const EPS: f32 = 1e-4;
        let dx = (pos_m.x - self.center_xz_m.x).abs();
        let dz = (pos_m.z - self.center_xz_m.y).abs();
        dx <= ELEVATOR_CAGE_HALF_EXTENT_M + EPS
            && dz <= ELEVATOR_CAGE_HALF_EXTENT_M + EPS
            && pos_m.y >= self.platform_y_m - 0.8 - EPS
            && pos_m.y <= self.platform_y_m + 3.2 + EPS
    }

    /// Transitions `ShedIdle -> Descending`, closes the cage gate, and spawns a background
    /// worker thread to regenerate the underground cave (`y < shed_floor_y`) seamlessly
    /// during the descent without touching the persistent Base Shed.
    pub fn start_descent(
        &mut self,
        world_arc: &Arc<RwLock<VoxelWorld>>,
        next_seed: u64,
        depth_tier: u8,
    ) {
        if self.phase != ElevatorPhase::ShedIdle {
            return;
        }

        let tier = depth_tier.clamp(1, 3);
        self.phase = ElevatorPhase::Descending;
        self.doors_closed = true;
        self.cable_shudder_active = false;
        self.phase_timer_s = ELEVATOR_DESCENT_DURATION_S;
        self.creak_timer_s = 0.0;
        self.prev_platform_y_m = self.platform_y_m;
        self.depth_tier = tier;
        self.cave_seed = next_seed;

        let slot: PendingCaveGen = Arc::new(Mutex::new(None));
        let slot_bg = Arc::clone(&slot);
        let world_arc_bg = Arc::clone(world_arc);

        std::thread::spawn(move || {
            let mut world_clone = {
                let guard = world_arc_bg
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                guard.clone()
            };
            let new_info = voxel_core::regenerate_underground(&mut world_clone, next_seed, tier);
            if let Ok(mut out) = slot_bg.lock() {
                *out = Some((world_clone, new_info));
            }
        });

        self.pending_gen = Some(slot);
    }

    /// Transitions `CaveIdle -> WinchActive`, initiating the loud extraction holdout climax.
    pub fn start_winch(&mut self) {
        if self.phase != ElevatorPhase::CaveIdle {
            return;
        }

        self.phase = ElevatorPhase::WinchActive;
        self.phase_timer_s = ELEVATOR_WINCH_DURATION_S;
        self.winch_pulse_timer_s = 0.0;
        self.cable_shudder_active = false;
        self.doors_closed = false;
    }

    /// Applies an authoritative host network snapshot to a guest's local elevator state.
    pub fn apply_network_sync(
        &mut self,
        phase: ElevatorPhase,
        platform_y_m: f32,
        phase_timer_s: f32,
        depth_tier: u8,
        cave_seed: u64,
    ) {
        self.prev_platform_y_m = self.platform_y_m;
        self.phase = phase;
        self.platform_y_m = platform_y_m;
        self.phase_timer_s = phase_timer_s.max(0.0);
        self.depth_tier = depth_tier.clamp(1, 3);
        self.cave_seed = cave_seed;
        self.doors_closed = matches!(phase, ElevatorPhase::Descending | ElevatorPhase::Ascending);
        if phase != ElevatorPhase::Descending {
            self.cable_shudder_active = false;
        }
    }

    /// Immediately docks the elevator back at the Base Shed (e.g., following a team wipe).
    pub fn reset_to_shed_idle(&mut self) {
        self.phase = ElevatorPhase::ShedIdle;
        self.platform_y_m = self.top_y_m;
        self.prev_platform_y_m = self.top_y_m;
        self.phase_timer_s = 0.0;
        self.doors_closed = false;
        self.cable_shudder_active = false;
        self.creak_timer_s = 0.0;
        self.winch_pulse_timer_s = 0.0;
        self.pending_gen = None;
    }

    /// Non-blocking poll for a completed background cave generation job.
    ///
    /// Returns `Some((world, info))` and clears the pending slot once ready.
    pub fn poll_generated_cave(&mut self) -> Option<(VoxelWorld, GeneratedLevelInfo)> {
        let result = {
            let slot = self.pending_gen.as_ref()?;
            match slot.try_lock() {
                Ok(mut guard) => guard.take(),
                Err(_) => None,
            }
        };

        if result.is_some() {
            self.pending_gen = None;
        }
        result
    }

    /// Returns `true` while a background underground generation job has not yet been taken.
    #[inline]
    pub fn is_gen_pending(&self) -> bool {
        self.pending_gen.is_some()
    }

    /// Human-readable status banner for the gameplay HUD.
    pub fn hud_status(&self) -> String {
        match self.phase {
            ElevatorPhase::ShedIdle => "Base Shed — Elevator Ready".to_string(),
            ElevatorPhase::Descending => {
                if self.cable_shudder_active {
                    format!(
                        "DESCENDING TO TIER {} (Stabilizing cables...)",
                        self.depth_tier
                    )
                } else {
                    format!(
                        "DESCENDING TO TIER {} ({:.1}s)",
                        self.depth_tier,
                        self.phase_timer_s.max(0.0)
                    )
                }
            }
            ElevatorPhase::CaveIdle => "CAVE ACTIVE — Pull Winch to Extract".to_string(),
            ElevatorPhase::WinchActive => format!(
                "WINCH ACTIVE — HOLD OUT ({:.0}s)!",
                self.phase_timer_s.max(0.0).ceil()
            ),
            ElevatorPhase::Ascending => format!(
                "ASCENDING TO SHED ({:.1}s)",
                self.phase_timer_s.max(0.0)
            ),
        }
    }
}

/// Output produced by [`step_elevator_state`] for a single simulation tick.
#[derive(Clone, Debug, Default)]
pub struct ElevatorStepOutput {
    pub arrived_cave: bool,
    pub returned_shed: bool,
    pub started_ascent: bool,
    /// Optional spatial sound `(clip, volume, pitch, ai_acoustic_event)` to trigger at cage center.
    pub emit_sound: Option<(SynthClip, f32, f32, Option<AcousticEvent>)>,
}

/// Advances [`ElevatorState`] deterministically by `dt` seconds.
///
/// `gen_ready` indicates whether the underground cave meshes/voxels for the current descent
/// are ready to be entered; if `false` when the descent timer expires, the cage seamlessly
/// enters a brief cable-shudder hold beat until `gen_ready` becomes `true`.
pub fn step_elevator_state(
    state: &mut ElevatorState,
    dt: f32,
    gen_ready: bool,
) -> ElevatorStepOutput {
    let dt = dt.max(0.0);
    state.prev_platform_y_m = state.platform_y_m;
    let mut out = ElevatorStepOutput::default();

    match state.phase {
        ElevatorPhase::ShedIdle | ElevatorPhase::CaveIdle => {}

        ElevatorPhase::Descending => {
            state.doors_closed = true;
            state.phase_timer_s = (state.phase_timer_s - dt).max(0.0);
            state.creak_timer_s += dt;

            if state.phase_timer_s <= 0.0 && !gen_ready {
                // Seamless loading hide: hold right above bottom landing with subtle cable shudder.
                state.cable_shudder_active = true;
                let shudder_offset = 0.10 + 0.045 * (state.creak_timer_s * 15.0).sin();
                state.platform_y_m = state.bottom_y_m + shudder_offset;
                if state.creak_timer_s >= CREAK_INTERVAL_S {
                    state.creak_timer_s -= CREAK_INTERVAL_S;
                    out.emit_sound = Some((SynthClip::ElevatorCreak, 0.72, 0.92, None));
                }
            } else if state.phase_timer_s <= 0.0 && gen_ready {
                state.cable_shudder_active = false;
                state.doors_closed = false;
                state.platform_y_m = state.bottom_y_m;
                state.phase = ElevatorPhase::CaveIdle;
                state.creak_timer_s = 0.0;
                out.arrived_cave = true;
                out.emit_sound = Some((
                    SynthClip::DoorSlam,
                    0.85,
                    1.0,
                    Some(AcousticEvent {
                        pos_m: state.cage_center_pos_m(),
                        energy: Bands3::new(1.8, 2.2, 1.6),
                        sharpness: 0.80,
                        kind: AcousticEventKind::ElevatorMachinery,
                    }),
                ));
            } else {
                state.cable_shudder_active = false;
                let progress = (1.0 - state.phase_timer_s / ELEVATOR_DESCENT_DURATION_S)
                    .clamp(0.0, 1.0);
                let eased = smoothstep(progress);
                state.platform_y_m =
                    state.top_y_m + (state.bottom_y_m - state.top_y_m) * eased;

                if state.creak_timer_s >= CREAK_INTERVAL_S {
                    state.creak_timer_s -= CREAK_INTERVAL_S;
                    out.emit_sound = Some((SynthClip::ElevatorCreak, 0.68, 1.0, None));
                }
            }
        }

        ElevatorPhase::WinchActive => {
            state.doors_closed = false;
            state.platform_y_m = state.bottom_y_m;
            state.phase_timer_s = (state.phase_timer_s - dt).max(0.0);

            if state.phase_timer_s <= 0.0 {
                state.doors_closed = true;
                state.phase = ElevatorPhase::Ascending;
                state.phase_timer_s = ELEVATOR_ASCENT_DURATION_S;
                state.creak_timer_s = 0.0;
                state.winch_pulse_timer_s = 0.0;
                out.started_ascent = true;
                out.emit_sound = Some((
                    SynthClip::DoorSlam,
                    0.90,
                    0.95,
                    Some(AcousticEvent {
                        pos_m: state.cage_center_pos_m(),
                        energy: Bands3::new(2.4, 2.8, 2.2),
                        sharpness: 0.85,
                        kind: AcousticEventKind::ElevatorMachinery,
                    }),
                ));
            } else {
                state.winch_pulse_timer_s -= dt;
                if state.winch_pulse_timer_s <= 0.0 {
                    state.winch_pulse_timer_s += WINCH_PULSE_INTERVAL_S;
                    if state.winch_pulse_timer_s <= 0.0 {
                        state.winch_pulse_timer_s = WINCH_PULSE_INTERVAL_S;
                    }
                    out.emit_sound = Some((
                        SynthClip::ElevatorWinch,
                        0.95,
                        1.0,
                        Some(AcousticEvent {
                            pos_m: state.cage_center_pos_m(),
                            energy: Bands3::new(3.8, 4.2, 3.2),
                            sharpness: 0.75,
                            kind: AcousticEventKind::ElevatorMachinery,
                        }),
                    ));
                }
            }
        }

        ElevatorPhase::Ascending => {
            state.doors_closed = true;
            state.phase_timer_s = (state.phase_timer_s - dt).max(0.0);
            state.creak_timer_s += dt;

            if state.phase_timer_s <= 0.0 {
                state.doors_closed = false;
                state.platform_y_m = state.top_y_m;
                state.phase = ElevatorPhase::ShedIdle;
                state.creak_timer_s = 0.0;
                out.returned_shed = true;
                out.emit_sound = Some((SynthClip::DoorSlam, 0.85, 1.0, None));
            } else {
                let progress = (1.0 - state.phase_timer_s / ELEVATOR_ASCENT_DURATION_S)
                    .clamp(0.0, 1.0);
                let eased = smoothstep(progress);
                state.platform_y_m =
                    state.bottom_y_m + (state.top_y_m - state.bottom_y_m) * eased;

                if state.creak_timer_s >= CREAK_INTERVAL_S {
                    state.creak_timer_s -= CREAK_INTERVAL_S;
                    out.emit_sound = Some((SynthClip::ElevatorCreak, 0.68, 1.04, None));
                }
            }
        }
    }

    out
}

/// Spawns the industrial mineshaft elevator cage hierarchy at the state's current position.
///
/// The overhead cage bulb light is spawned on a dedicated child entity with **no `Mesh3d`**
/// to prevent Bevy frustum culling based on the tiny bulb mesh's AABB.
pub fn spawn_elevator_cage(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    state: &ElevatorState,
) -> Entity {
    let steel_dark_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.20, 0.22, 0.25),
        metallic: 0.82,
        perceptual_roughness: 0.55,
        ..default()
    });
    let steel_frame_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.30, 0.32, 0.35),
        metallic: 0.78,
        perceptual_roughness: 0.48,
        ..default()
    });
    let door_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.36, 0.29, 0.16),
        metallic: 0.72,
        perceptual_roughness: 0.52,
        ..default()
    });
    let bulb_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.90, 0.65),
        emissive: LinearRgba::new(9.0, 6.8, 3.2, 1.0),
        ..default()
    });

    // Shared geometry handles reused across symmetric struts, beams, and safety rails.
    let floor_mesh = meshes.add(Cuboid::new(5.4, ELEVATOR_FLOOR_THICKNESS_M, 5.4));
    let corner_strut_mesh = meshes.add(Cuboid::new(0.18, 3.2, 0.18));
    let mid_post_mesh = meshes.add(Cuboid::new(0.10, 3.2, 0.10));
    let top_beam_x_mesh = meshes.add(Cuboid::new(5.4, 0.16, 0.18));
    let top_beam_z_mesh = meshes.add(Cuboid::new(0.18, 0.16, 5.4));
    let cross_beam_x_mesh = meshes.add(Cuboid::new(5.2, 0.10, 0.14));
    let cross_beam_z_mesh = meshes.add(Cuboid::new(0.14, 0.10, 5.2));
    let rail_x_mesh = meshes.add(Cuboid::new(5.2, 0.08, 0.08));
    let rail_z_mesh = meshes.add(Cuboid::new(0.08, 0.08, 5.2));
    let door_mesh = meshes.add(Cuboid::new(0.12, 2.95, 5.0));
    let bulb_mesh = meshes.add(Sphere::new(0.14));

    let door_y = if state.doors_closed {
        ELEVATOR_DOOR_CLOSED_Y_M
    } else {
        ELEVATOR_DOOR_OPEN_Y_M
    };

    commands
        .spawn((
            Transform::from_xyz(state.center_xz_m.x, state.platform_y_m, state.center_xz_m.y),
            Visibility::default(),
            ElevatorCageRoot,
        ))
        .with_children(|parent| {
            // 1. Heavy industrial steel floor plate (top surface flush with y = 0.0).
            parent.spawn((
                Mesh3d(floor_mesh),
                MeshMaterial3d(steel_dark_mat.clone()),
                Transform::from_xyz(0.0, -ELEVATOR_FLOOR_THICKNESS_M * 0.5, 0.0),
            ));

            // 2. Four corner steel vertical struts.
            for sx in [-2.60_f32, 2.60_f32] {
                for sz in [-2.60_f32, 2.60_f32] {
                    parent.spawn((
                        Mesh3d(corner_strut_mesh.clone()),
                        MeshMaterial3d(steel_frame_mat.clone()),
                        Transform::from_xyz(sx, 1.6, sz),
                    ));
                }
            }

            // 3. Top structural steel frame + overhead cable cross-beams.
            for sz in [-2.60_f32, 2.60_f32] {
                parent.spawn((
                    Mesh3d(top_beam_x_mesh.clone()),
                    MeshMaterial3d(steel_frame_mat.clone()),
                    Transform::from_xyz(0.0, 3.2, sz),
                ));
            }
            for sx in [-2.60_f32, 2.60_f32] {
                parent.spawn((
                    Mesh3d(top_beam_z_mesh.clone()),
                    MeshMaterial3d(steel_frame_mat.clone()),
                    Transform::from_xyz(sx, 3.2, 0.0),
                ));
            }
            parent.spawn((
                Mesh3d(cross_beam_x_mesh),
                MeshMaterial3d(steel_dark_mat.clone()),
                Transform::from_xyz(0.0, 3.2, 0.0),
            ));
            parent.spawn((
                Mesh3d(cross_beam_z_mesh),
                MeshMaterial3d(steel_dark_mat.clone()),
                Transform::from_xyz(0.0, 3.2, 0.0),
            ));

            // 4. Safety bars & vertical mid-posts on the -X, -Z, and +Z faces (+X is boarding side).
            for (px, pz) in [(-2.60_f32, 0.0_f32), (0.0, -2.60), (0.0, 2.60)] {
                parent.spawn((
                    Mesh3d(mid_post_mesh.clone()),
                    MeshMaterial3d(steel_dark_mat.clone()),
                    Transform::from_xyz(px, 1.6, pz),
                ));
            }
            for bar_y in [0.55_f32, 1.15_f32, 1.75_f32] {
                // -X side safety bar
                parent.spawn((
                    Mesh3d(rail_z_mesh.clone()),
                    MeshMaterial3d(steel_frame_mat.clone()),
                    Transform::from_xyz(-2.60, bar_y, 0.0),
                ));
                // -Z side safety bar
                parent.spawn((
                    Mesh3d(rail_x_mesh.clone()),
                    MeshMaterial3d(steel_frame_mat.clone()),
                    Transform::from_xyz(0.0, bar_y, -2.60),
                ));
                // +Z side safety bar
                parent.spawn((
                    Mesh3d(rail_x_mesh.clone()),
                    MeshMaterial3d(steel_frame_mat.clone()),
                    Transform::from_xyz(0.0, bar_y, 2.60),
                ));
            }

            // 5. Sliding steel security gate on the +X boarding face.
            parent.spawn((
                Mesh3d(door_mesh),
                MeshMaterial3d(door_mat),
                Transform::from_xyz(2.60, door_y, 0.0),
                ElevatorDoorVisual,
            ));

            // 6. Overhead industrial bulb visual mesh (NO PointLight attached here!).
            parent.spawn((
                Mesh3d(bulb_mesh),
                MeshMaterial3d(bulb_mat),
                Transform::from_xyz(0.0, 3.02, 0.0),
            ));

            // 7. Dedicated child PointLight entity (NO Mesh3d — avoids tiny-AABB frustum culling!).
            parent.spawn((
                PointLight {
                    color: Color::srgb(1.0, 0.82, 0.52),
                    intensity: 42_000.0,
                    range: 16.0,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, 2.85, 0.0),
                ElevatorBulbLight,
            ));
        })
        .id()
}

/// Smoothly transitions ambient brightness from warm Base Shed daylight (`52.0`)
/// down to dark underground cavern (`14.0`) based on elevation.
pub fn compute_ambient_brightness_for_depth(
    platform_y_m: f32,
    top_y_m: f32,
    bottom_y_m: f32,
    player_y_m: f32,
) -> f32 {
    let span = (top_y_m - bottom_y_m).max(0.01);
    let effective_y = if (player_y_m - platform_y_m).abs() <= 3.5 && platform_y_m < top_y_m {
        player_y_m.min(platform_y_m)
    } else {
        player_y_m
    };
    let t = ((effective_y - bottom_y_m) / span).clamp(0.0, 1.0);
    let eased = smoothstep(t);
    CAVE_AMBIENT_BRIGHTNESS + (SHED_AMBIENT_BRIGHTNESS - CAVE_AMBIENT_BRIGHTNESS) * eased
}

#[inline]
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use glam::UVec3;
    use voxel_core::{VoxelConfig, generate_world_with_tier};

    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    fn sample_world_and_state() -> (Arc<RwLock<VoxelWorld>>, GeneratedLevelInfo, ElevatorState) {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        let (world, info) = generate_world_with_tier(1337, 1, cfg);
        let state = ElevatorState::from_level_info(&info, 1337);
        (Arc::new(RwLock::new(world)), info, state)
    }

    #[test]
    fn elevator_state_is_send_and_sync() {
        assert_send_sync::<ElevatorState>();
    }

    #[test]
    fn full_elevator_lifecycle_with_cable_shudder_and_winch_acoustics() {
        let (world_arc, info, mut state) = sample_world_and_state();

        // 1. Starts in ShedIdle at top floor.
        assert_eq!(state.phase, ElevatorPhase::ShedIdle);
        assert!((state.platform_y_m - info.shed_floor_y_m).abs() < 1e-4);
        assert!(!state.doors_closed);
        assert_eq!(state.hud_status(), "Base Shed — Elevator Ready");

        // 2. Start descent -> Descending, doors slam closed, background gen spawned.
        state.start_descent(&world_arc, 9001, 2);
        assert_eq!(state.phase, ElevatorPhase::Descending);
        assert!(state.doors_closed);
        assert_eq!(state.depth_tier, 2);
        assert_eq!(state.cave_seed, 9001);
        assert!(state.is_gen_pending());
        assert!(state.hud_status().contains("DESCENDING TO TIER 2"));

        // 3. Step full descent duration while `gen_ready = false` -> triggers cable shudder!
        let mut saw_creak_during_descent = false;
        for _ in 0..20 {
            let out = step_elevator_state(&mut state, 0.5, false);
            assert!(!out.arrived_cave);
            if let Some((clip, _, _, _)) = out.emit_sound
                && clip == SynthClip::ElevatorCreak
            {
                saw_creak_during_descent = true;
            }
        }
        assert!(saw_creak_during_descent, "expected creak SFX during descent");
        assert_eq!(state.phase, ElevatorPhase::Descending);
        assert!(state.cable_shudder_active);
        assert!(state.doors_closed);
        assert!(state.hud_status().contains("Stabilizing cables"));

        // Additional ticks while still waiting keep cable shudder active without transitioning.
        let shudder_out = step_elevator_state(&mut state, 0.2, false);
        assert!(!shudder_out.arrived_cave);
        assert_eq!(state.phase, ElevatorPhase::Descending);
        assert!(state.cable_shudder_active);

        // 4. Once `gen_ready = true`, arrives seamlessly at CaveIdle!
        let arrive_out = step_elevator_state(&mut state, 0.1, true);
        assert!(arrive_out.arrived_cave);
        assert_eq!(state.phase, ElevatorPhase::CaveIdle);
        assert!(!state.doors_closed);
        assert!(!state.cable_shudder_active);
        assert!((state.platform_y_m - info.cave_floor_y_m).abs() < 1e-4);
        assert_eq!(state.hud_status(), "CAVE ACTIVE — Pull Winch to Extract");
        match arrive_out.emit_sound {
            Some((SynthClip::DoorSlam, _, _, _)) => {}
            other => panic!("expected DoorSlam on cave arrival, got {other:?}"),
        }

        // 5. Start winch -> WinchActive emits loud ElevatorMachinery acoustic events every 1.35s.
        state.start_winch();
        assert_eq!(state.phase, ElevatorPhase::WinchActive);
        assert!((state.phase_timer_s - ELEVATOR_WINCH_DURATION_S).abs() < 1e-4);
        assert!(state.hud_status().contains("WINCH ACTIVE"));

        let mut winch_events = 0usize;
        let mut started_ascent = false;
        for _ in 0..300 {
            let out = step_elevator_state(&mut state, 0.1, true);
            if let Some((SynthClip::ElevatorWinch, _, _, Some(ai_ev))) = out.emit_sound {
                assert_eq!(ai_ev.kind, AcousticEventKind::ElevatorMachinery);
                assert!((ai_ev.energy.0[0] - 3.8).abs() < 1e-4);
                assert!((ai_ev.energy.0[1] - 4.2).abs() < 1e-4);
                assert!((ai_ev.energy.0[2] - 3.2).abs() < 1e-4);
                assert!((ai_ev.sharpness - 0.75).abs() < 1e-4);
                winch_events += 1;
            }
            if out.started_ascent {
                started_ascent = true;
                match out.emit_sound {
                    Some((SynthClip::DoorSlam, _, _, _)) => {}
                    other => panic!("expected DoorSlam on ascent start, got {other:?}"),
                }
                break;
            }
        }
        assert!(
            winch_events >= 20,
            "expected ~22 loud winch pulses over 30s, got {winch_events}"
        );
        assert!(started_ascent, "expected WinchActive to transition to Ascending");
        assert_eq!(state.phase, ElevatorPhase::Ascending);
        assert!(state.doors_closed);
        assert!(state.hud_status().contains("ASCENDING TO SHED"));

        // 6. Step full ascent -> smoothly climbs back to ShedIdle!
        let mut saw_creak_during_ascent = false;
        let mut returned_shed = false;
        let mut last_y = state.platform_y_m;
        for _ in 0..105 {
            let out = step_elevator_state(&mut state, 0.1, true);
            assert!(
                state.platform_y_m >= last_y - 1e-4,
                "platform should rise monotonically during ascent"
            );
            last_y = state.platform_y_m;
            if let Some((SynthClip::ElevatorCreak, _, _, _)) = out.emit_sound {
                saw_creak_during_ascent = true;
            }
            if out.returned_shed {
                returned_shed = true;
                match out.emit_sound {
                    Some((SynthClip::DoorSlam, _, _, _)) => {}
                    other => panic!("expected DoorSlam on shed return, got {other:?}"),
                }
                break;
            }
        }
        assert!(saw_creak_during_ascent, "expected creak SFX during ascent");
        assert!(returned_shed, "expected Ascending to return to ShedIdle");
        assert_eq!(state.phase, ElevatorPhase::ShedIdle);
        assert!(!state.doors_closed);
        assert!((state.platform_y_m - info.shed_floor_y_m).abs() < 1e-4);
    }

    #[test]
    fn start_descent_background_thread_generates_cave_and_polls_cleanly() {
        let (world_arc, _info, mut state) = sample_world_and_state();
        state.start_descent(&world_arc, 42_4242, 3);
        assert!(state.is_gen_pending());

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut generated = None;
        while Instant::now() < deadline && generated.is_none() {
            generated = state.poll_generated_cave();
            if generated.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        let (_new_world, new_info) = generated.expect("background cave gen did not finish in time");
        assert_eq!(new_info.depth_tier, 3);
        assert!(!state.is_gen_pending());
        assert!(state.poll_generated_cave().is_none());
    }

    #[test]
    fn is_pos_inside_cage_checks_xz_and_y_bounds() {
        let (_world_arc, _info, state) = sample_world_and_state();
        let center = Vec3::new(state.center_xz_m.x, state.platform_y_m, state.center_xz_m.y);

        // Inside cage floor, head height, and right at edge.
        assert!(state.is_pos_inside_cage(center));
        assert!(state.is_pos_inside_cage(center + Vec3::new(0.0, 1.62, 0.0)));
        assert!(state.is_pos_inside_cage(center + Vec3::new(ELEVATOR_CAGE_HALF_EXTENT_M, 0.0, 0.0)));
        assert!(state.is_pos_inside_cage(center + Vec3::new(0.0, -0.75, -ELEVATOR_CAGE_HALF_EXTENT_M)));
        assert!(state.is_pos_inside_cage(center + Vec3::new(0.0, 3.15, 0.0)));

        // Outside XZ or Y envelope.
        assert!(!state.is_pos_inside_cage(
            center + Vec3::new(ELEVATOR_CAGE_HALF_EXTENT_M + 0.1, 0.5, 0.0)
        ));
        assert!(!state.is_pos_inside_cage(
            center + Vec3::new(0.0, 0.5, -(ELEVATOR_CAGE_HALF_EXTENT_M + 0.1))
        ));
        assert!(!state.is_pos_inside_cage(center + Vec3::new(0.0, -0.85, 0.0)));
        assert!(!state.is_pos_inside_cage(center + Vec3::new(0.0, 3.25, 0.0)));
    }

    #[test]
    fn ambient_brightness_transitions_from_shed_to_cave() {
        let top_y = 87.3_f32;
        let bottom_y = 29.7_f32;
        let mid_y = 0.5 * (top_y + bottom_y);

        let shed_b = compute_ambient_brightness_for_depth(top_y, top_y, bottom_y, top_y);
        let mid_b = compute_ambient_brightness_for_depth(mid_y, top_y, bottom_y, mid_y + 1.6);
        let cave_b = compute_ambient_brightness_for_depth(bottom_y, top_y, bottom_y, bottom_y + 1.6);

        assert!((shed_b - SHED_AMBIENT_BRIGHTNESS).abs() < 1e-3);
        assert!(mid_b > CAVE_AMBIENT_BRIGHTNESS && mid_b < SHED_AMBIENT_BRIGHTNESS);
        assert!((mid_b - 33.0).abs() < 1e-3);
        assert!((cave_b - CAVE_AMBIENT_BRIGHTNESS).abs() < 1e-3);

        // Player staying in the Shed while cage is down in the cave keeps Shed daylight.
        let shed_while_cage_low =
            compute_ambient_brightness_for_depth(bottom_y, top_y, bottom_y, top_y);
        assert!((shed_while_cage_low - SHED_AMBIENT_BRIGHTNESS).abs() < 1e-3);
    }

    #[test]
    fn network_sync_and_reset_to_shed_idle_behave_correctly() {
        let (_world_arc, info, mut state) = sample_world_and_state();

        state.apply_network_sync(ElevatorPhase::Descending, 55.0, 4.2, 3, 7777);
        assert_eq!(state.phase, ElevatorPhase::Descending);
        assert!((state.platform_y_m - 55.0).abs() < 1e-4);
        assert!((state.prev_platform_y_m - info.shed_floor_y_m).abs() < 1e-4);
        assert!(state.doors_closed);
        assert_eq!(state.depth_tier, 3);
        assert_eq!(state.cave_seed, 7777);

        state.reset_to_shed_idle();
        assert_eq!(state.phase, ElevatorPhase::ShedIdle);
        assert!((state.platform_y_m - info.shed_floor_y_m).abs() < 1e-4);
        assert!(!state.doors_closed);
        assert!(!state.cable_shudder_active);
        assert!(!state.is_gen_pending());
    }

    #[test]
    fn spawn_elevator_cage_isolates_point_light_from_mesh() {
        let (_world_arc, _info, state) = sample_world_and_state();
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<StandardMaterial>::default());

        let root = {
            let mut meshes = world.remove_resource::<Assets<Mesh>>().unwrap();
            let mut materials = world.remove_resource::<Assets<StandardMaterial>>().unwrap();
            let mut commands = world.commands();
            let root = spawn_elevator_cage(&mut commands, &mut meshes, &mut materials, &state);
            world.flush();
            world.insert_resource(meshes);
            world.insert_resource(materials);
            root
        };

        assert!(world.entity(root).contains::<ElevatorCageRoot>());

        let mut door_q = world.query_filtered::<Entity, With<ElevatorDoorVisual>>();
        assert_eq!(door_q.iter(&world).count(), 1);

        let mut light_q =
            world.query_filtered::<(Entity, &PointLight, Option<&Mesh3d>), With<ElevatorBulbLight>>();
        let lights: Vec<_> = light_q.iter(&world).collect();
        assert_eq!(lights.len(), 1);
        assert!(
            lights[0].2.is_none(),
            "ElevatorBulbLight must NOT share an entity with Mesh3d (frustum culling rule)"
        );
    }
}
