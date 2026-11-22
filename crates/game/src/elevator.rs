//! Seamless mineshaft elevator state machine, async underground cave generation,
//! Bevy 0.20 industrial cage spawner, and depth-based ambient daylight falloff.

use std::sync::{Arc, Mutex, RwLock};

use bevy::prelude::*;
use voxel_acoustics::{AcousticEvent, AcousticEventKind, Bands3};
use voxel_audio_dsp::SynthClip;
use voxel_core::{GeneratedLevelInfo, VoxelWorld};
use voxel_net::ElevatorPhase;

/// Duration of the seamless shaft descent from Base Shed to Cave landing (seconds).
pub const ELEVATOR_DESCENT_DURATION_S: f32 = 7.0;
/// Duration of the loud extraction winch holdout before doors slam shut (seconds).
pub const ELEVATOR_WINCH_DURATION_S: f32 = 18.0;
/// Reduced extraction winch holdout duration when the Crew Winch Overdrive upgrade is installed (seconds).
pub const ELEVATOR_WINCH_OVERDRIVE_DURATION_S: f32 = 11.0;
/// Duration of the shaft ascent back up to the Base Shed (seconds).
pub const ELEVATOR_ASCENT_DURATION_S: f32 = 7.0;
/// Half-width / half-depth of the walkable elevator cage interior in meters.
/// Matches the 7-voxel (6.30 m) shaft hole half-width (3.15 m) so there is ZERO entrance gap.
pub const ELEVATOR_CAGE_HALF_EXTENT_M: f32 = 3.12;
/// Physical thickness of the steel elevator platform floor plate in meters.
pub const ELEVATOR_FLOOR_THICKNESS_M: f32 = 0.20;
/// Local offset of the interior Elevator Lever console mounted inside the cage beside the +X door.
pub const ELEVATOR_LEVER_LOCAL_OFFSET_M: Vec3 = Vec3::new(2.30, 0.0, 2.55);

/// Local Y offset of the `+X` sliding security door when closed.
pub const ELEVATOR_DOOR_CLOSED_Y_M: f32 = 1.50;
/// Local Y offset of the `+X` sliding security door when raised open.
pub const ELEVATOR_DOOR_OPEN_Y_M: f32 = 4.35;

const CREAK_INTERVAL_S: f32 = 2.4;
const WINCH_PULSE_INTERVAL_S: f32 = 2.0;
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
    /// Platform elevation consumed by the last `elevator_runtime_system` frame so guest
    /// network syncs and local interpolation never lose vertical platform displacement.
    pub last_ridden_y_m: f32,
    pub phase_timer_s: f32,
    pub depth_tier: u8,
    pub cave_seed: u64,
    pub doors_closed: bool,
    pub cable_shudder_active: bool,
    pub winch_holding_for_players: bool,
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
            last_ridden_y_m: info.shed_floor_y_m,
            phase_timer_s: 0.0,
            depth_tier: info.depth_tier.clamp(1, 3),
            cave_seed: seed,
            doors_closed: false,
            cable_shudder_active: false,
            winch_holding_for_players: false,
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

    /// World-space base position of the interior Elevator Lever console mounted inside the cage.
    #[inline]
    pub fn lever_world_pos_m(&self) -> Vec3 {
        Vec3::new(
            self.center_xz_m.x + ELEVATOR_LEVER_LOCAL_OFFSET_M.x,
            self.platform_y_m,
            self.center_xz_m.y + ELEVATOR_LEVER_LOCAL_OFFSET_M.z,
        )
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

    /// Returns `true` if a player's feet (`feet_pos_m`) lie within the stricter boarding envelope
    /// inside the elevator cage (`3.00 m` half-extent instead of `3.12 m`, so standing right outside
    /// the open `+X` shaft lip never false-positives as aboard).
    #[inline]
    pub fn is_player_inside_cage(&self, feet_pos_m: Vec3) -> bool {
        let player_half = ELEVATOR_CAGE_HALF_EXTENT_M - 0.12;
        let dx = (feet_pos_m.x - self.center_xz_m.x).abs();
        let dz = (feet_pos_m.z - self.center_xz_m.y).abs();
        dx <= player_half
            && dz <= player_half
            && feet_pos_m.y >= self.platform_y_m - 0.55
            && feet_pos_m.y <= self.platform_y_m + 2.85
    }

    /// Resolves collision between a dynamic dropped item or throwable (`prev_pos -> pos`,
    /// half-extents `radius`, velocity `vel`) and the moving steel elevator floor plate +
    /// closed cage walls.
    ///
    /// Returns `true` if the body bounced on the steel deck or against a closed cage wall.
    pub fn collide_dynamic_with_elevator(
        &self,
        prev_pos: Vec3,
        pos: &mut Vec3,
        vel: &mut Vec3,
        radius: Vec3,
        restitution: f32,
        damping: f32,
    ) -> bool {
        let mut bounced = false;
        let deck_half_extent = ELEVATOR_CAGE_HALF_EXTENT_M + 0.15;
        let ref_floor_y = self.platform_y_m.min(self.prev_platform_y_m);

        // 1. Prevent dropped/thrown objects from escaping through closed steel cage doors/walls
        // while traveling up or down the shaft (run before deck check so diagonal corner throws
        // clamp back over the deck and bounce on the floor in the same tick).
        if self.doors_closed {
            let prev_in_cage_xz = (prev_pos.x - self.center_xz_m.x).abs() <= deck_half_extent
                && (prev_pos.z - self.center_xz_m.y).abs() <= deck_half_extent;
            let max_floor_y = self.platform_y_m.max(self.prev_platform_y_m);
            let in_cage_y = pos.y >= ref_floor_y - 0.25 && pos.y <= max_floor_y + 3.25;
            if prev_in_cage_xz && in_cage_y {
                let wall_limit_x = (ELEVATOR_CAGE_HALF_EXTENT_M - radius.x - 0.06).max(0.5);
                let wall_limit_z = (ELEVATOR_CAGE_HALF_EXTENT_M - radius.z - 0.06).max(0.5);
                let dx = pos.x - self.center_xz_m.x;
                let dz = pos.z - self.center_xz_m.y;
                if dx.abs() > wall_limit_x {
                    pos.x = self.center_xz_m.x + dx.clamp(-wall_limit_x, wall_limit_x);
                    vel.x *= -restitution;
                    *vel *= damping;
                    bounced = true;
                }
                if dz.abs() > wall_limit_z {
                    pos.z = self.center_xz_m.y + dz.clamp(-wall_limit_z, wall_limit_z);
                    vel.z *= -restitution;
                    *vel *= damping;
                    bounced = true;
                }
            }
        }

        // 2. Steel floor plate collision: top sits +2 cm above `platform_y_m`.
        // Catches normal drops, fast shaft falls, and upward-moving decks while leaving
        // objects strictly below the elevator in the cave untouched.
        let over_deck = (pos.x - self.center_xz_m.x).abs() <= deck_half_extent
            && (pos.z - self.center_xz_m.y).abs() <= deck_half_extent;
        let deck_center_y = self.platform_y_m + 0.02 + radius.y;
        if over_deck
            && prev_pos.y >= ref_floor_y - ELEVATOR_FLOOR_THICKNESS_M - 0.15
            && pos.y <= deck_center_y
        {
            pos.y = deck_center_y;
            if vel.y < 0.0 {
                vel.y = -vel.y * restitution;
            }
            *vel *= damping;
            bounced = true;
        }

        bounced
    }

    /// Translates an item or throwable vertically with the moving elevator deck when inside
    /// the cage footprint, keeping it seated on top of the steel plate and inside closed gates.
    pub fn ride_with_elevator(&self, pos: &mut Vec3, prev_platform_y_m: f32, radius: Vec3) {
        let delta_y = self.platform_y_m - prev_platform_y_m;
        let deck_half_extent = ELEVATOR_CAGE_HALF_EXTENT_M + 0.15;
        let dx = pos.x - self.center_xz_m.x;
        let dz = pos.z - self.center_xz_m.y;
        if dx.abs() <= deck_half_extent
            && dz.abs() <= deck_half_extent
            && (pos.y - prev_platform_y_m).abs() <= 2.4
        {
            if delta_y.abs() > 1e-5 {
                pos.y += delta_y;
            }
            let min_y = self.platform_y_m + 0.02 + radius.y;
            if pos.y < min_y {
                pos.y = min_y;
            }
            if self.doors_closed {
                let wall_limit_x = (ELEVATOR_CAGE_HALF_EXTENT_M - radius.x - 0.06).max(0.5);
                let wall_limit_z = (ELEVATOR_CAGE_HALF_EXTENT_M - radius.z - 0.06).max(0.5);
                pos.x = self.center_xz_m.x + dx.clamp(-wall_limit_x, wall_limit_x);
                pos.z = self.center_xz_m.y + dz.clamp(-wall_limit_z, wall_limit_z);
            }
        }
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
        self.winch_holding_for_players = false;
        self.phase_timer_s = ELEVATOR_DESCENT_DURATION_S;
        self.creak_timer_s = 0.0;
        self.prev_platform_y_m = self.platform_y_m;
        self.last_ridden_y_m = self.platform_y_m;
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
    /// When `overdrive` is `true` (Crew Winch Overdrive upgrade), the holdout duration is cut
    /// from [`ELEVATOR_WINCH_DURATION_S`] (`18.0s`) to [`ELEVATOR_WINCH_OVERDRIVE_DURATION_S`] (`11.0s`).
    pub fn start_winch_with_overdrive(&mut self, overdrive: bool) {
        if self.phase != ElevatorPhase::CaveIdle {
            return;
        }

        self.phase = ElevatorPhase::WinchActive;
        self.phase_timer_s = if overdrive {
            ELEVATOR_WINCH_OVERDRIVE_DURATION_S
        } else {
            ELEVATOR_WINCH_DURATION_S
        };
        self.winch_pulse_timer_s = 0.0;
        self.cable_shudder_active = false;
        self.winch_holding_for_players = false;
        self.doors_closed = false;
    }

    /// Transitions `CaveIdle -> WinchActive` with the standard holdout duration.
    #[allow(dead_code)]
    pub fn start_winch(&mut self) {
        self.start_winch_with_overdrive(false);
    }

    /// Applies an authoritative host network snapshot to a guest's local elevator state.
    ///
    /// Does NOT overwrite `last_ridden_y_m` during motion so `elevator_runtime_system`
    /// always consumes the full `delta_y = platform_y_m - last_ridden_y_m` since the previous frame.
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
        if phase != ElevatorPhase::WinchActive {
            self.winch_holding_for_players = false;
        } else if self.phase_timer_s <= 0.0 {
            self.winch_holding_for_players = true;
        }
    }

    /// Immediately docks the elevator back at the Base Shed (e.g., following a team wipe).
    pub fn reset_to_shed_idle(&mut self) {
        self.phase = ElevatorPhase::ShedIdle;
        self.platform_y_m = self.top_y_m;
        self.prev_platform_y_m = self.top_y_m;
        self.last_ridden_y_m = self.top_y_m;
        self.phase_timer_s = 0.0;
        self.doors_closed = false;
        self.cable_shudder_active = false;
        self.winch_holding_for_players = false;
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
    #[allow(dead_code)]
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
            ElevatorPhase::WinchActive => {
                if self.winch_holding_for_players {
                    "WINCH READY — ALL PLAYERS MUST BOARD!".to_string()
                } else {
                    format!(
                        "WINCH ACTIVE — HOLD OUT ({:.0}s)!",
                        self.phase_timer_s.max(0.0).ceil()
                    )
                }
            }
            ElevatorPhase::Ascending => format!(
                "ASCENDING TO SHED ({:.1}s)",
                self.phase_timer_s.max(0.0)
            ),
        }
    }
}

/// Smoothly advances a Guest's local elevator platform and countdown timer between authoritative
/// Host `ElevatorStateSync` snapshots (`0.30s` cadence) so guests glide smoothly at 60+ fps.
pub fn step_guest_elevator_interpolation(state: &mut ElevatorState, dt: f32) {
    let dt = dt.max(0.0);
    state.prev_platform_y_m = state.platform_y_m;
    match state.phase {
        ElevatorPhase::Descending => {
            state.doors_closed = true;
            state.phase_timer_s = (state.phase_timer_s - dt).max(0.0);
            let progress =
                (1.0 - state.phase_timer_s / ELEVATOR_DESCENT_DURATION_S).clamp(0.0, 1.0);
            state.platform_y_m =
                state.top_y_m + (state.bottom_y_m - state.top_y_m) * smoothstep(progress);
        }
        ElevatorPhase::Ascending => {
            state.doors_closed = true;
            state.phase_timer_s = (state.phase_timer_s - dt).max(0.0);
            let progress =
                (1.0 - state.phase_timer_s / ELEVATOR_ASCENT_DURATION_S).clamp(0.0, 1.0);
            state.platform_y_m =
                state.bottom_y_m + (state.top_y_m - state.bottom_y_m) * smoothstep(progress);
        }
        ElevatorPhase::WinchActive => {
            state.phase_timer_s = (state.phase_timer_s - dt).max(0.0);
        }
        ElevatorPhase::ShedIdle | ElevatorPhase::CaveIdle => {}
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

/// Advances [`ElevatorState`] deterministically by `dt` seconds assuming all living players are aboard.
#[allow(dead_code)]
pub fn step_elevator_state(
    state: &mut ElevatorState,
    dt: f32,
    gen_ready: bool,
) -> ElevatorStepOutput {
    step_elevator_state_with_boarding(state, dt, gen_ready, true)
}

/// Advances [`ElevatorState`] deterministically by `dt` seconds.
///
/// `gen_ready` indicates whether the underground cave meshes/voxels for the current descent
/// are ready to be entered; if `false` when the descent timer expires, the cage seamlessly
/// enters a brief cable-shudder hold beat until `gen_ready` becomes `true`.
///
/// `all_aboard` indicates whether all living players are inside the elevator cage; if `false`
/// when the `WinchActive` countdown reaches `0.0`, the cage holds open at the bottom landing
/// and continues pulsing loud winch machinery until all living players board.
pub fn step_elevator_state_with_boarding(
    state: &mut ElevatorState,
    dt: f32,
    gen_ready: bool,
    all_aboard: bool,
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
                // Arrival door opening is local audio only (no AI alert — prevents spawn-camping!)
                out.emit_sound = Some((SynthClip::DoorSlam, 0.85, 1.0, None));
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

            if state.phase_timer_s <= 0.0 && all_aboard {
                state.winch_holding_for_players = false;
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
                if state.phase_timer_s <= 0.0 {
                    state.phase_timer_s = 0.0;
                    state.winch_holding_for_players = true;
                } else {
                    state.winch_holding_for_players = false;
                }
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
    let lever_handle_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.72, 0.14),
        emissive: LinearRgba::new(5.5, 3.0, 0.35, 1.0),
        perceptual_roughness: 0.25,
        ..default()
    });

    // Non-overlapping dimensions sized to bridge the full 6.30 m shaft hole with ZERO Z-fighting!
    let floor_mesh = meshes.add(Cuboid::new(6.32, ELEVATOR_FLOOR_THICKNESS_M, 6.32));
    let corner_strut_mesh = meshes.add(Cuboid::new(0.18, 3.10, 0.18));
    let mid_post_mesh = meshes.add(Cuboid::new(0.10, 3.08, 0.10));
    let top_beam_x_mesh = meshes.add(Cuboid::new(5.90, 0.16, 0.18));
    let top_beam_z_mesh = meshes.add(Cuboid::new(0.18, 0.16, 6.26));
    let cross_beam_x_mesh = meshes.add(Cuboid::new(5.88, 0.08, 0.14));
    let cross_beam_z_mesh = meshes.add(Cuboid::new(0.14, 0.08, 5.88));
    // Solid industrial steel ceiling plate enclosing the top of the cage so players never see up the open shaft.
    // Sized `x in [-3.12, +3.06]` to seal over the top frame without intersecting the `+X` sliding door (`x = 3.135`).
    let roof_plate_mesh = meshes.add(Cuboid::new(6.18, 0.14, 6.26));
    let rail_x_mesh = meshes.add(Cuboid::new(5.90, 0.08, 0.06));
    let rail_z_mesh = meshes.add(Cuboid::new(0.06, 0.08, 5.90));
    let door_mesh = meshes.add(Cuboid::new(0.10, 2.95, 5.88));
    let bulb_mesh = meshes.add(Sphere::new(0.14));
    let lever_post_mesh = meshes.add(Cuboid::new(0.36, 1.05, 0.36));
    let lever_arm_mesh = meshes.add(Cylinder::new(0.03, 0.52));
    let lever_handle_mesh = meshes.add(Cuboid::new(0.30, 0.085, 0.085));

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
            // 1. Heavy industrial steel floor plate lifted +2cm above voxel floor (ZERO Z-fighting at cave landing!).
            parent.spawn((
                Mesh3d(floor_mesh),
                MeshMaterial3d(steel_dark_mat.clone()),
                Transform::from_xyz(0.0, -ELEVATOR_FLOOR_THICKNESS_M * 0.5 + 0.02, 0.0),
            ));

            // 2. Four corner steel vertical struts (stopping cleanly below top beams at y = 3.12).
            for sx in [-3.04_f32, 3.04_f32] {
                for sz in [-3.04_f32, 3.04_f32] {
                    parent.spawn((
                        Mesh3d(corner_strut_mesh.clone()),
                        MeshMaterial3d(steel_frame_mat.clone()),
                        Transform::from_xyz(sx, 1.57, sz),
                    ));
                }
            }

            // 3. Top structural steel frame + vertically stacked cross-beams (ZERO coplanar overlap!).
            for sz in [-3.04_f32, 3.04_f32] {
                parent.spawn((
                    Mesh3d(top_beam_x_mesh.clone()),
                    MeshMaterial3d(steel_frame_mat.clone()),
                    Transform::from_xyz(0.0, 3.20, sz),
                ));
            }
            for sx in [-3.04_f32, 3.04_f32] {
                parent.spawn((
                    Mesh3d(top_beam_z_mesh.clone()),
                    MeshMaterial3d(steel_frame_mat.clone()),
                    Transform::from_xyz(sx, 3.20, 0.0),
                ));
            }
            parent.spawn((
                Mesh3d(cross_beam_x_mesh),
                MeshMaterial3d(steel_dark_mat.clone()),
                Transform::from_xyz(0.0, 3.23, 0.0),
            ));
            parent.spawn((
                Mesh3d(cross_beam_z_mesh),
                MeshMaterial3d(steel_dark_mat.clone()),
                Transform::from_xyz(0.0, 3.13, 0.0),
            ));
            // Solid steel roof plate resting 2mm above the top perimeter beams (y = 3.282..3.422, ZERO Z-fighting!).
            parent.spawn((
                Mesh3d(roof_plate_mesh),
                MeshMaterial3d(steel_dark_mat.clone()),
                Transform::from_xyz(-0.03, 3.352, 0.0),
            ));

            // 4. Mid-posts (slightly inset at +-2.96) & safety bars (outside at +-3.05) — ZERO Z-fighting!
            for (px, pz) in [(-2.96_f32, 0.0_f32), (0.0, -2.96), (0.0, 2.96)] {
                parent.spawn((
                    Mesh3d(mid_post_mesh.clone()),
                    MeshMaterial3d(steel_dark_mat.clone()),
                    Transform::from_xyz(px, 1.56, pz),
                ));
            }
            for bar_y in [0.55_f32, 1.15_f32, 1.75_f32] {
                parent.spawn((
                    Mesh3d(rail_z_mesh.clone()),
                    MeshMaterial3d(steel_frame_mat.clone()),
                    Transform::from_xyz(-3.05, bar_y, 0.0),
                ));
                parent.spawn((
                    Mesh3d(rail_x_mesh.clone()),
                    MeshMaterial3d(steel_frame_mat.clone()),
                    Transform::from_xyz(0.0, bar_y, -3.05),
                ));
                parent.spawn((
                    Mesh3d(rail_x_mesh.clone()),
                    MeshMaterial3d(steel_frame_mat.clone()),
                    Transform::from_xyz(0.0, bar_y, 3.05),
                ));
            }

            // 5. Sliding steel security gate on the +X boarding face (offset outside struts at +3.14).
            parent.spawn((
                Mesh3d(door_mesh),
                MeshMaterial3d(door_mat),
                Transform::from_xyz(3.135, door_y, 0.0),
                ElevatorDoorVisual,
            ));

            // 6. Interior Elevator Lever Console (rides inside the cage; requires all players aboard!).
            let lv = ELEVATOR_LEVER_LOCAL_OFFSET_M;
            parent.spawn((
                Mesh3d(lever_post_mesh),
                MeshMaterial3d(steel_dark_mat),
                Transform::from_xyz(lv.x, 0.545, lv.z),
            ));
            parent.spawn((
                Mesh3d(lever_arm_mesh),
                MeshMaterial3d(steel_frame_mat),
                Transform::from_xyz(lv.x, 1.22, lv.z - 0.08)
                    .with_rotation(Quat::from_rotation_x(-0.35)),
            ));
            parent.spawn((
                Mesh3d(lever_handle_mesh),
                MeshMaterial3d(lever_handle_mat),
                Transform::from_xyz(lv.x, 1.46, lv.z - 0.18),
            ));

            // 7. Overhead industrial bulb visual mesh (NO PointLight attached here!).
            parent.spawn((
                Mesh3d(bulb_mesh),
                MeshMaterial3d(bulb_mat),
                Transform::from_xyz(0.0, 2.98, 0.0),
            ));

            // 8. Dedicated child PointLight entity (NO Mesh3d — avoids tiny-AABB frustum culling!).
            parent.spawn((
                PointLight {
                    color: Color::srgb(1.0, 0.82, 0.52),
                    intensity: 44_000.0,
                    range: 17.0,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, 2.82, 0.0),
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
            winch_events >= 5,
            "expected ~6 loud winch pulses over 12s, got {winch_events}"
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

    #[test]
    fn collide_dynamic_with_elevator_stops_falling_items_and_flares() {
        let (_world_arc, _info, mut state) = sample_world_and_state();
        let floor_y = state.platform_y_m;

        // 1. Dropped Heavy Relic (r = 0.32) inside the shaft bounces onto the steel deck instead of falling through.
        let relic_r = Vec3::splat(0.32);
        let prev_relic = Vec3::new(state.center_xz_m.x, floor_y + 0.55, state.center_xz_m.y);
        let mut relic_pos = Vec3::new(state.center_xz_m.x, floor_y - 0.10, state.center_xz_m.y);
        let mut relic_vel = Vec3::new(0.4, -6.0, 0.0);
        let bounced = state.collide_dynamic_with_elevator(
            prev_relic,
            &mut relic_pos,
            &mut relic_vel,
            relic_r,
            0.32,
            0.72,
        );
        assert!(bounced, "heavy relic must bounce on elevator deck");
        assert!(
            (relic_pos.y - (floor_y + 0.02 + relic_r.y)).abs() < 1e-4,
            "heavy relic should rest on top of the +2cm steel plate"
        );
        assert!(relic_vel.y > 0.0, "downward velocity should invert on bounce");

        // 2. Thrown Flare (r = 0.10) bounces on the deck AND stays contained when doors are closed.
        state.doors_closed = true;
        let flare_r = Vec3::splat(0.10);
        let prev_flare = Vec3::new(state.center_xz_m.x + 2.9, floor_y + 0.40, state.center_xz_m.y);
        let mut flare_pos = Vec3::new(state.center_xz_m.x + 3.40, floor_y - 0.05, state.center_xz_m.y);
        let mut flare_vel = Vec3::new(5.0, -4.0, 0.0);
        let flare_bounced = state.collide_dynamic_with_elevator(
            prev_flare,
            &mut flare_pos,
            &mut flare_vel,
            flare_r,
            0.42,
            0.78,
        );
        assert!(flare_bounced, "thrown flare must bounce on elevator deck/wall");
        assert!(
            (flare_pos.y - (floor_y + 0.02 + flare_r.y)).abs() < 1e-4,
            "flare should not fall through steel floor"
        );
        assert!(
            (flare_pos.x - state.center_xz_m.x).abs() <= ELEVATOR_CAGE_HALF_EXTENT_M - flare_r.x,
            "flare should stay inside closed elevator gate"
        );
        assert!(flare_vel.x < 0.0, "flare hitting +X closed door should reflect inward");

        // 3. Item far below the elevator (down in the cave) or outside the shaft XZ is unaffected.
        let cave_prev = Vec3::new(state.center_xz_m.x, floor_y - 25.0, state.center_xz_m.y);
        let mut cave_pos = Vec3::new(state.center_xz_m.x, floor_y - 25.5, state.center_xz_m.y);
        let mut cave_vel = Vec3::new(0.0, -5.0, 0.0);
        assert!(!state.collide_dynamic_with_elevator(
            cave_prev,
            &mut cave_pos,
            &mut cave_vel,
            flare_r,
            0.42,
            0.78
        ));
        assert!((cave_pos.y - (floor_y - 25.5)).abs() < 1e-4);

        let outside_prev = Vec3::new(state.center_xz_m.x + 8.0, floor_y + 0.5, state.center_xz_m.y);
        let mut outside_pos = Vec3::new(state.center_xz_m.x + 8.0, floor_y - 0.2, state.center_xz_m.y);
        let mut outside_vel = Vec3::new(0.0, -4.0, 0.0);
        assert!(!state.collide_dynamic_with_elevator(
            outside_prev,
            &mut outside_pos,
            &mut outside_vel,
            relic_r,
            0.32,
            0.72
        ));

        // 4. Settled flare rides vertically with the descending elevator deck.
        let prev_y = state.platform_y_m;
        state.prev_platform_y_m = prev_y;
        state.platform_y_m -= 2.5;
        let mut riding_flare = Vec3::new(state.center_xz_m.x, prev_y + 0.02 + flare_r.y, state.center_xz_m.y);
        state.ride_with_elevator(&mut riding_flare, prev_y, flare_r);
        assert!(
            (riding_flare.y - (state.platform_y_m + 0.02 + flare_r.y)).abs() < 1e-4,
            "settled flare should move smoothly with the descending deck"
        );
    }

    #[test]
    fn start_winch_with_overdrive_reduces_holdout_from_18s_to_11s() {
        let (_world_arc, _info, mut state) = sample_world_and_state();
        state.phase = ElevatorPhase::CaveIdle;
        state.start_winch_with_overdrive(false);
        assert_eq!(state.phase, ElevatorPhase::WinchActive);
        assert!((state.phase_timer_s - 18.0).abs() < 1e-4);

        state.phase = ElevatorPhase::CaveIdle;
        state.start_winch_with_overdrive(true);
        assert_eq!(state.phase, ElevatorPhase::WinchActive);
        assert!((state.phase_timer_s - 11.0).abs() < 1e-4);
    }

    #[test]
    fn winch_holds_at_zero_until_all_players_aboard_then_ascends() {
        let (_world_arc, info, mut state) = sample_world_and_state();
        state.phase = ElevatorPhase::CaveIdle;
        state.platform_y_m = info.cave_floor_y_m;
        state.start_winch_with_overdrive(true);
        assert_eq!(state.phase, ElevatorPhase::WinchActive);
        assert!(!state.winch_holding_for_players);

        // Expire the winch timer while a living teammate is still outside (`all_aboard = false`).
        let mut saw_winch_pulse_while_holding = false;
        for _ in 0..140 {
            let out = step_elevator_state_with_boarding(&mut state, 0.1, true, false);
            assert!(
                !out.started_ascent,
                "elevator must NEVER slam doors or ascend while living players are outside"
            );
            if state.phase_timer_s <= 0.0
                && let Some((SynthClip::ElevatorWinch, _, _, Some(ai_ev))) = out.emit_sound
            {
                assert_eq!(ai_ev.kind, AcousticEventKind::ElevatorMachinery);
                saw_winch_pulse_while_holding = true;
            }
        }
        assert_eq!(state.phase, ElevatorPhase::WinchActive);
        assert_eq!(state.phase_timer_s, 0.0);
        assert!(!state.doors_closed);
        assert!(state.winch_holding_for_players);
        assert_eq!(
            state.hud_status(),
            "WINCH READY — ALL PLAYERS MUST BOARD!"
        );
        assert!(
            saw_winch_pulse_while_holding,
            "winch must keep pulsing loud machinery acoustics while holding for players"
        );

        // As soon as all living players board (`all_aboard = true`), doors slam and ascent begins!
        let liftoff = step_elevator_state_with_boarding(&mut state, 0.1, true, true);
        assert!(liftoff.started_ascent);
        assert_eq!(state.phase, ElevatorPhase::Ascending);
        assert!(state.doors_closed);
        assert!(!state.winch_holding_for_players);
        match liftoff.emit_sound {
            Some((SynthClip::DoorSlam, _, _, _)) => {}
            other => panic!("expected DoorSlam on winch liftoff, got {other:?}"),
        }
    }

    #[test]
    fn guest_elevator_interpolation_and_last_ridden_y_tracks_smooth_descent() {
        let (_world_arc, info, mut state) = sample_world_and_state();
        assert!((state.last_ridden_y_m - info.shed_floor_y_m).abs() < 1e-4);

        // Simulate receiving an initial network sync at the start of descent (`t = 6.7s` left),
        // which updates `platform_y_m` earlier in the frame without clobbering `last_ridden_y_m`.
        let synced_y = info.shed_floor_y_m - 0.35;
        state.apply_network_sync(ElevatorPhase::Descending, synced_y, 6.7, 1, 1337);
        assert!(
            (state.last_ridden_y_m - info.shed_floor_y_m).abs() < 1e-4,
            "apply_network_sync must preserve last_ridden_y_m for elevator_runtime_system"
        );

        // Guest steps local interpolation by 16ms: platform_y_m advances smoothly and
        // `platform_y_m - last_ridden_y_m` is strictly negative (non-zero descent delta).
        step_guest_elevator_interpolation(&mut state, 1.0 / 60.0);
        let delta_y = state.platform_y_m - state.last_ridden_y_m;
        assert!(
            delta_y < -0.1,
            "expected negative descent delta_y after sync + interpolation, got {delta_y}"
        );
        state.last_ridden_y_m = state.platform_y_m;

        // Between 0.30s Host syncs, each 60fps frame continues descending smoothly.
        for _ in 0..18 {
            step_guest_elevator_interpolation(&mut state, 1.0 / 60.0);
            let frame_delta = state.platform_y_m - state.last_ridden_y_m;
            assert!(
                frame_delta < -1e-3,
                "expected smooth per-frame guest descent delta, got {frame_delta}"
            );
            state.last_ridden_y_m = state.platform_y_m;
        }
    }
}
