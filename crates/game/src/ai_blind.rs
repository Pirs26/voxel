use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    f32::consts::{PI, SQRT_2, TAU},
};

use bevy::prelude::*;
use glam::IVec3;
use voxel_acoustics::{AcousticEvent, AcousticEventKind, Bands3, evaluate_ai_hearing};
use voxel_audio_dsp::SynthClip;
use voxel_core::{VoxelConfig, VoxelWorld, raycast_solid};

/// Grace period (seconds) right after spawning in a new cave so the stalker
/// never rushes the elevator before players step out and orient themselves.
pub const STALKER_SPAWN_GRACE_S: f32 = 12.0;

/// Returns `(patrol, investigate, hunt, cooldown)` speeds in m/s scaled gently by `depth_tier`.
/// Even at Tier 3, the stalker stays slower than a walking player (`4.0 m/s`), making tunnel
/// jukes, breakable shortcuts, and stun counterplay fair while keeping solo relic drags tense!
pub fn stalker_speeds_for_tier(depth_tier: u8) -> (f32, f32, f32, f32) {
    match depth_tier.clamp(1, 3) {
        1 => (0.95, 1.35, 2.15, 0.95),
        2 => (1.15, 1.65, 2.55, 1.10),
        _ => (1.35, 1.95, 2.95, 1.25),
    }
}

/// Returns the world-space melee/crosshair hit AABB `(min_m, max_m)` for a Stalker at `feet_pos_m`.
pub fn stalker_hit_aabb(feet_pos_m: Vec3) -> (Vec3, Vec3) {
    (
        feet_pos_m + Vec3::new(-0.55, 0.0, -0.55),
        feet_pos_m + Vec3::new(0.55, 2.15, 0.55),
    )
}

const STRIKE_HORIZ_RANGE_M: f32 = 1.75;
const STRIKE_VERT_RANGE_M: f32 = 1.5;
const ATTACK_COOLDOWN_S: f32 = 5.5;
const FOOTSTEP_STRIDE_M: f32 = 1.8;
const DEFAULT_PATH_MAX_NODES: usize = 600;

/// Behavioral state of the blind acoustic stalker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StalkerState {
    Patrol,
    Investigate,
    Hunt,
    AttackCooldown,
    Stunned,
}

/// Outcome of hitting a Blind Acoustic Stalker with a pickaxe strike or flare stun.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StalkerHitResult {
    Stunned { remaining_hp: u8, max_hp: u8 },
    Killed,
}

/// Reaction emitted when a blind stalker actually hears a world `AcousticEvent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StalkerReaction {
    Investigate,
    AlertHunt,
}

/// Host-authoritative blind acoustic predator driven strictly by `voxel_acoustics::evaluate_ai_hearing`.
#[derive(Component, Clone, Debug)]
pub struct BlindStalker {
    pub monster_id: u16,
    pub feet_pos_m: Vec3,
    pub yaw: f32,
    pub state: StalkerState,
    pub state_timer_s: f32,
    pub target_pos_m: Option<Vec3>,
    pub waypoints: Vec<Vec3>,
    pub alert_level: f32,
    pub patrol_origin_m: Vec3,
    pub vocal_timer_s: f32,
    pub step_accum_m: f32,
    pub repath_timer_s: f32,
    pub spawn_grace_s: f32,
    /// While > 0, the stalker is strongly distracted by a loud Noise Decoy and ignores
    /// quieter noises like solo Heavy Relic scraping so players can slip past!
    pub decoy_lock_s: f32,
    pub hp: u8,
    pub max_hp: u8,
    pub band_sensitivity: Bands3,
    pub hearing_threshold: f32,
    pub hunt_threshold: f32,
    pub depth_tier: u8,
}

impl BlindStalker {
    pub fn new(monster_id: u16, feet_pos_m: Vec3, depth_tier: u8) -> Self {
        let tier = depth_tier.clamp(1, 3);
        let (hearing_threshold, hunt_threshold, max_hp) = match tier {
            1 => (0.40, 1.10, 3u8),
            2 => (0.32, 0.90, 4u8),
            _ => (0.26, 0.76, 5u8),
        };
        // High Mid/High frequency sensitivity: voices, dropped items, rock breaches, sprinting.
        let band_sensitivity = Bands3::new(0.45, 1.25, 1.65);

        Self {
            monster_id,
            feet_pos_m,
            yaw: 0.0,
            state: StalkerState::Patrol,
            state_timer_s: 0.0,
            target_pos_m: None,
            waypoints: Vec::new(),
            alert_level: 0.0,
            patrol_origin_m: feet_pos_m,
            vocal_timer_s: 5.5 + ((monster_id as f32 * 1.37) % 3.5),
            step_accum_m: 0.0,
            repath_timer_s: 0.0,
            spawn_grace_s: STALKER_SPAWN_GRACE_S,
            decoy_lock_s: 0.0,
            hp: max_hp,
            max_hp,
            band_sensitivity,
            hearing_threshold,
            hunt_threshold,
            depth_tier: tier,
        }
    }
}

/// Applies melee damage and/or stun + collision-safe knockback to a `BlindStalker`.
pub fn apply_stalker_hit(
    world: &VoxelWorld,
    stalker: &mut BlindStalker,
    damage: u8,
    stun_s: f32,
    knockback_dir: Vec3,
) -> StalkerHitResult {
    stalker.hp = stalker.hp.saturating_sub(damage);
    if stalker.hp == 0 {
        return StalkerHitResult::Killed;
    }

    stalker.state = StalkerState::Stunned;
    stalker.state_timer_s = stalker.state_timer_s.max(stun_s.max(1.0));
    stalker.waypoints.clear();
    stalker.target_pos_m = None;
    stalker.repath_timer_s = 0.0;

    // Short collision-safe horizontal stagger away from the strike
    let horiz = Vec3::new(knockback_dir.x, 0.0, knockback_dir.z).normalize_or_zero();
    if horiz.length_squared() > 0.1 {
        for step in [0.45f32, 0.40] {
            let cand = stalker.feet_pos_m + horiz * step;
            if !stalker_body_overlaps_solid(world, cand) {
                stalker.feet_pos_m = cand;
            } else {
                break;
            }
        }
    }

    StalkerHitResult::Stunned {
        remaining_hp: stalker.hp,
        max_hp: stalker.max_hp,
    }
}

/// Guest-side interpolated representation of a host-simulated blind stalker.
#[derive(Component, Clone, Debug)]
pub struct RemoteSyncedStalker {
    pub monster_id: u16,
    pub target_pos_m: Vec3,
    pub target_yaw: f32,
    pub state: StalkerState,
}

/// Side effects produced by a single headless simulation step of `step_stalker_ai`.
#[derive(Default, Debug, Clone)]
pub struct StalkerStepOutput {
    /// `(player_id, monster_feet_pos_m)` when the stalker strikes a close, unobstructed player.
    pub strike_target: Option<(u16, Vec3)>,
    /// `(clip, volume, pitch)` vocalization to emit into the spatial acoustic mixer.
    pub emit_vocal: Option<(SynthClip, f32, f32)>,
    /// `(pos_m, volume)` heavy stalker footstep to emit into the spatial acoustic mixer.
    pub emit_footstep: Option<(Vec3, f32)>,
}

/// Returns `true` if voxel `v` is walkable for a standing/hunched creature:
/// Air at `v`, Air at `v + Y` (2 voxels headroom), and a solid floor directly beneath at `v - Y`.
#[inline]
pub fn is_walkable_voxel(world: &VoxelWorld, v: IVec3) -> bool {
    world.get(v).is_air() && world.get(v + IVec3::Y).is_air() && world.is_solid(v - IVec3::Y)
}

#[inline]
fn voxel_feet_world_pos(cfg: VoxelConfig, v: IVec3) -> Vec3 {
    let vs = cfg.voxel_size_m;
    Vec3::new((v.x as f32 + 0.5) * vs, v.y as f32 * vs, (v.z as f32 + 0.5) * vs)
}

fn find_nearest_walkable(world: &VoxelWorld, center: IVec3, radius: i32) -> Option<IVec3> {
    if is_walkable_voxel(world, center) {
        return Some(center);
    }

    let mut best: Option<(i32, IVec3)> = None;
    for dy in -radius..=radius {
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let cand = center + IVec3::new(dx, dy, dz);
                if !is_walkable_voxel(world, cand) {
                    continue;
                }
                let dist_sq = dx * dx + dy * dy + dz * dz;
                if best.is_none_or(|(best_d, _)| dist_sq < best_d) {
                    best = Some((dist_sq, cand));
                }
            }
        }
    }
    best.map(|(_, v)| v)
}

#[derive(Clone, Copy, Debug)]
struct OpenNode {
    f_cost: f32,
    h_cost: f32,
    voxel: IVec3,
}

impl PartialEq for OpenNode {
    fn eq(&self, other: &Self) -> bool {
        self.voxel == other.voxel
            && self.f_cost.to_bits() == other.f_cost.to_bits()
            && self.h_cost.to_bits() == other.h_cost.to_bits()
    }
}

impl Eq for OpenNode {}

impl PartialOrd for OpenNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OpenNode {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse order so `BinaryHeap` pops the smallest `f_cost` first.
        other
            .f_cost
            .total_cmp(&self.f_cost)
            .then_with(|| other.h_cost.total_cmp(&self.h_cost))
            .then_with(|| self.voxel.x.cmp(&other.voxel.x))
            .then_with(|| self.voxel.y.cmp(&other.voxel.y))
            .then_with(|| self.voxel.z.cmp(&other.voxel.z))
    }
}

/// Fast bounded A* pathfinder over walkable cave voxels.
///
/// Supports 8-neighbor horizontal movement (without diagonal wall-corner clipping) plus ±1 voxel
/// vertical steps, snapping `start_m` and `goal_m` to the nearest walkable voxel within radius 2.
/// Always terminates after at most `max_nodes` node expansions; if `goal_m` is unreachable,
/// returns a path to the closest explored walkable voxel.
pub fn find_voxel_path(
    world: &VoxelWorld,
    start_m: Vec3,
    goal_m: Vec3,
    max_nodes: usize,
) -> Vec<Vec3> {
    let cfg = world.config();
    let vs = cfg.voxel_size_m;
    let nudge = Vec3::Y * (vs * 0.05);

    let start_raw = cfg.world_to_voxel(start_m + nudge);
    let goal_raw = cfg.world_to_voxel(goal_m + nudge);

    let Some(start_v) = find_nearest_walkable(world, start_raw, 2) else {
        return Vec::new();
    };
    let goal_snapped = find_nearest_walkable(world, goal_raw, 2);
    let target_v = goal_snapped.unwrap_or(goal_raw);

    if start_v == target_v {
        return vec![voxel_feet_world_pos(cfg, target_v)];
    }

    let budget = max_nodes.max(1);
    let heuristic = |v: IVec3| (v - target_v).as_vec3().length();

    let mut open = BinaryHeap::new();
    let mut g_score: HashMap<IVec3, f32> = HashMap::new();
    let mut came_from: HashMap<IVec3, IVec3> = HashMap::new();

    let h0 = heuristic(start_v);
    g_score.insert(start_v, 0.0);
    open.push(OpenNode {
        f_cost: h0,
        h_cost: h0,
        voxel: start_v,
    });

    let mut best_voxel = start_v;
    let mut best_h = h0;
    let mut expanded = 0usize;

    const HORIZ_DIRS: [(i32, i32, f32); 8] = [
        (1, 0, 1.0),
        (-1, 0, 1.0),
        (0, 1, 1.0),
        (0, -1, 1.0),
        (1, 1, SQRT_2),
        (1, -1, SQRT_2),
        (-1, 1, SQRT_2),
        (-1, -1, SQRT_2),
    ];

    while let Some(curr) = open.pop() {
        let Some(&known_g) = g_score.get(&curr.voxel) else {
            continue;
        };
        if curr.f_cost - curr.h_cost > known_g + 1e-3 {
            continue;
        }

        if curr.h_cost < best_h {
            best_h = curr.h_cost;
            best_voxel = curr.voxel;
        }

        if curr.voxel == target_v {
            best_voxel = curr.voxel;
            break;
        }

        expanded += 1;
        if expanded >= budget {
            break;
        }

        for &(dx, dz, base_cost) in &HORIZ_DIRS {
            // Prevent squeezing diagonally between two solid corner walls
            if dx != 0 && dz != 0 {
                let side_a = curr.voxel + IVec3::new(dx, 0, 0);
                let side_b = curr.voxel + IVec3::new(0, 0, dz);
                if world.is_solid(side_a)
                    || world.is_solid(side_b)
                    || world.is_solid(side_a + IVec3::Y)
                    || world.is_solid(side_b + IVec3::Y)
                {
                    continue;
                }
            }

            for dy in [0, 1, -1] {
                let next = curr.voxel + IVec3::new(dx, dy, dz);
                if !is_walkable_voxel(world, next) {
                    continue;
                }

                let step_cost = base_cost + if dy != 0 { 0.35 } else { 0.0 };
                let tentative_g = known_g + step_cost;
                if g_score
                    .get(&next)
                    .is_some_and(|&prev_g| tentative_g >= prev_g - 1e-4)
                {
                    continue;
                }

                g_score.insert(next, tentative_g);
                came_from.insert(next, curr.voxel);
                let h = heuristic(next);
                open.push(OpenNode {
                    f_cost: tentative_g + h,
                    h_cost: h,
                    voxel: next,
                });
            }
        }
    }

    // Reconstruct path from `best_voxel` back to `start_v`
    let mut rev_voxels = Vec::new();
    let mut cursor = best_voxel;
    rev_voxels.push(cursor);
    while let Some(&prev) = came_from.get(&cursor) {
        cursor = prev;
        rev_voxels.push(cursor);
        if cursor == start_v {
            break;
        }
    }
    rev_voxels.reverse();

    // Skip starting voxel when moving so the first waypoint advances immediately
    let start_idx = usize::from(rev_voxels.len() > 1);
    rev_voxels[start_idx..]
        .iter()
        .map(|&v| voxel_feet_world_pos(cfg, v))
        .collect()
}

/// Feeds an `AcousticEvent` through the unified `voxel_acoustics::evaluate_ai_hearing` pipeline.
///
/// The stalker NEVER cheats through walls: if direct density occlusion + 1-bounce corner checks
/// attenuate the event below `stalker.hearing_threshold`, this returns `None`.
pub fn process_acoustic_event_for_stalker(
    world: &VoxelWorld,
    stalker: &mut BlindStalker,
    event: AcousticEvent,
) -> Option<StalkerReaction> {
    if stalker.spawn_grace_s > 0.0 || stalker.state == StalkerState::Stunned {
        return None;
    }
    let ear_pos_m = stalker.feet_pos_m + Vec3::Y * 1.4;
    let perceived = evaluate_ai_hearing(
        world,
        event,
        ear_pos_m,
        stalker.band_sensitivity,
        stalker.hearing_threshold,
    )?;

    // Noise Decoys are overwhelmingly loud acoustic lures: while locked onto a decoy,
    // quieter sounds like relic scraping or footsteps cannot pull the stalker off the decoy!
    if stalker.decoy_lock_s > 0.0
        && !matches!(
            perceived.kind,
            AcousticEventKind::DecoyNoise | AcousticEventKind::ElevatorMachinery
        )
    {
        return None;
    }
    if perceived.kind == AcousticEventKind::DecoyNoise {
        stalker.decoy_lock_s = 2.6;
    }

    stalker.alert_level = (stalker.alert_level + perceived.salience * 0.45).min(1.5);
    stalker.target_pos_m = Some(perceived.perceived_pos_m);
    stalker.repath_timer_s = 0.0;

    let triggers_hunt = perceived.salience >= stalker.hunt_threshold
        || stalker.alert_level >= 1.0
        || matches!(
            perceived.kind,
            AcousticEventKind::DecoyNoise | AcousticEventKind::ElevatorMachinery
        );

    if triggers_hunt {
        if stalker.state != StalkerState::AttackCooldown {
            stalker.state = StalkerState::Hunt;
            stalker.state_timer_s = 12.0;
        }
        Some(StalkerReaction::AlertHunt)
    } else if stalker.state != StalkerState::Hunt && stalker.state != StalkerState::AttackCooldown {
        stalker.state = StalkerState::Investigate;
        stalker.state_timer_s = 9.0;
        Some(StalkerReaction::Investigate)
    } else if stalker.state == StalkerState::Hunt {
        stalker.state_timer_s = stalker.state_timer_s.max(9.0);
        Some(StalkerReaction::Investigate)
    } else {
        None
    }
}

fn stalker_body_overlaps_solid(world: &VoxelWorld, feet_pos_m: Vec3) -> bool {
    let cfg = world.config();
    let r = 0.22f32;
    let offsets = [
        Vec3::new(-r, 0.25, -r),
        Vec3::new(r, 0.25, -r),
        Vec3::new(-r, 0.25, r),
        Vec3::new(r, 0.25, r),
        Vec3::new(-r, 1.35, -r),
        Vec3::new(r, 1.35, -r),
        Vec3::new(-r, 1.35, r),
        Vec3::new(r, 1.35, r),
    ];
    offsets
        .iter()
        .any(|&off| world.is_solid(cfg.world_to_voxel(feet_pos_m + off)))
}

fn has_clear_strike_line(world: &VoxelWorld, stalker_feet_m: Vec3, player_feet_m: Vec3) -> bool {
    let from = stalker_feet_m + Vec3::Y * 0.90;
    let to = player_feet_m + Vec3::Y * 0.90;
    let delta = to - from;
    let dist = delta.length();
    if dist < 0.05 {
        return true;
    }
    raycast_solid(world, from, delta / dist, dist).is_none()
}

fn deterministic_hash(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn pick_patrol_target(world: &VoxelWorld, stalker: &BlindStalker) -> Option<Vec3> {
    let cfg = world.config();
    let base_seed = (stalker.monster_id as u64).wrapping_mul(0x517c_c1b7_2722_0a95)
        ^ ((stalker.feet_pos_m.x.to_bits() as u64) << 1)
        ^ ((stalker.feet_pos_m.z.to_bits() as u64) << 17)
        ^ (stalker.vocal_timer_s.to_bits() as u64);

    for attempt in 0..14u64 {
        let h = deterministic_hash(base_seed.wrapping_add(attempt.wrapping_mul(0x9e37_79b9)));
        let angle = ((h & 0xffff) as f32 / 65535.0) * TAU;
        let radius = 4.0 + (((h >> 16) & 0xffff) as f32 / 65535.0) * 8.0;
        let dy_m = (((h >> 32) & 0xff) as i32 % 3 - 1) as f32 * cfg.voxel_size_m;

        let cand_m = stalker.patrol_origin_m
            + Vec3::new(angle.cos() * radius, dy_m, angle.sin() * radius);
        let cand_v = cfg.world_to_voxel(cand_m + Vec3::Y * (cfg.voxel_size_m * 0.05));
        if let Some(walk_v) = find_nearest_walkable(world, cand_v, 2) {
            let pos = voxel_feet_world_pos(cfg, walk_v);
            if pos.distance(stalker.feet_pos_m) >= 2.2 {
                return Some(pos);
            }
        }
    }
    None
}

fn pick_retreat_target(world: &VoxelWorld, stalker_feet_m: Vec3, away_from_m: Vec3) -> Option<Vec3> {
    let cfg = world.config();
    let mut away = Vec3::new(
        stalker_feet_m.x - away_from_m.x,
        0.0,
        stalker_feet_m.z - away_from_m.z,
    );
    if away.length_squared() < 1e-4 {
        away = Vec3::X;
    } else {
        away = away.normalize();
    }

    for dist in [6.0f32, 4.5, 3.0] {
        let cand_m = stalker_feet_m + away * dist;
        let cand_v = cfg.world_to_voxel(cand_m + Vec3::Y * (cfg.voxel_size_m * 0.05));
        if let Some(walk_v) = find_nearest_walkable(world, cand_v, 2) {
            return Some(voxel_feet_world_pos(cfg, walk_v));
        }
    }
    None
}

fn wrap_angle_pi(mut a: f32) -> f32 {
    while a > PI {
        a -= TAU;
    }
    while a < -PI {
        a += TAU;
    }
    a
}

/// Advances a `BlindStalker` by `dt` seconds using voxel pathfinding, collision-safe steering,
/// close-range unobstructed strike checks, eerie vocalizations, and footstep emissions.
pub fn step_stalker_ai(
    world: &VoxelWorld,
    stalker: &mut BlindStalker,
    living_players: &[(u16, Vec3)],
    dt: f32,
) -> StalkerStepOutput {
    let dt = dt.clamp(0.0, 0.25);
    let mut output = StalkerStepOutput::default();
    if dt <= 0.0 {
        return output;
    }

    // 1. Decay alert & advance timers
    let alert_decay = match stalker.state {
        StalkerState::Patrol => 0.12,
        StalkerState::Investigate => 0.08,
        StalkerState::Hunt => 0.05,
        StalkerState::AttackCooldown => 0.20,
        StalkerState::Stunned => 0.25,
    };
    stalker.alert_level = (stalker.alert_level - alert_decay * dt).max(0.0);
    stalker.repath_timer_s = (stalker.repath_timer_s - dt).max(0.0);
    stalker.state_timer_s = (stalker.state_timer_s - dt).max(0.0);
    stalker.spawn_grace_s = (stalker.spawn_grace_s - dt).max(0.0);
    stalker.decoy_lock_s = (stalker.decoy_lock_s - dt).max(0.0);

    if stalker.state == StalkerState::Stunned {
        if stalker.state_timer_s <= 0.0 {
            stalker.state = StalkerState::Patrol;
            stalker.target_pos_m = None;
            stalker.waypoints.clear();
        }
        return output;
    }

    match stalker.state {
        StalkerState::Hunt if stalker.state_timer_s <= 0.0 => {
            stalker.state = StalkerState::Investigate;
            stalker.state_timer_s = 6.0;
        }
        StalkerState::Investigate if stalker.state_timer_s <= 0.0 => {
            stalker.state = StalkerState::Patrol;
            stalker.target_pos_m = None;
            stalker.waypoints.clear();
        }
        StalkerState::AttackCooldown if stalker.state_timer_s <= 0.0 => {
            if stalker.target_pos_m.is_some() {
                stalker.state = StalkerState::Investigate;
                stalker.state_timer_s = 7.0;
            } else {
                stalker.state = StalkerState::Patrol;
            }
        }
        _ => {}
    }

    // 2. Close-range strike check (ONLY in Hunt or Investigate, NEVER during AttackCooldown)
    if matches!(stalker.state, StalkerState::Hunt | StalkerState::Investigate) {
        let mut closest_strike: Option<(f32, u16, Vec3)> = None;
        for &(pid, player_feet_m) in living_players {
            let dx = player_feet_m.x - stalker.feet_pos_m.x;
            let dz = player_feet_m.z - stalker.feet_pos_m.z;
            let horiz_dist = (dx * dx + dz * dz).sqrt();
            let vert_diff = (player_feet_m.y - stalker.feet_pos_m.y).abs();

            if horiz_dist <= STRIKE_HORIZ_RANGE_M
                && vert_diff <= STRIKE_VERT_RANGE_M
                && has_clear_strike_line(world, stalker.feet_pos_m, player_feet_m)
                && closest_strike.is_none_or(|(best_d, _, _)| horiz_dist < best_d)
            {
                closest_strike = Some((horiz_dist, pid, player_feet_m));
            }
        }

        if let Some((_, pid, player_feet_m)) = closest_strike {
            output.strike_target = Some((pid, stalker.feet_pos_m));
            output.emit_vocal = Some((SynthClip::MonsterScreech, 1.0, 1.0));
            stalker.state = StalkerState::AttackCooldown;
            stalker.state_timer_s = ATTACK_COOLDOWN_S;
            stalker.target_pos_m = None;
            stalker.waypoints.clear();
            stalker.repath_timer_s = 0.0;
            // Pick a brief retreat path so teammates have a rescue window
            if let Some(retreat_m) = pick_retreat_target(world, stalker.feet_pos_m, player_feet_m) {
                stalker.waypoints = find_voxel_path(
                    world,
                    stalker.feet_pos_m,
                    retreat_m,
                    DEFAULT_PATH_MAX_NODES,
                );
            }
            return output;
        }
    }

    // 3. Patrol wandering target when idle
    if stalker.state == StalkerState::Patrol
        && stalker.target_pos_m.is_none()
        && stalker.waypoints.is_empty()
    {
        stalker.target_pos_m = pick_patrol_target(world, stalker);
        stalker.repath_timer_s = 0.0;
    }

    // 4. Recompute bounded voxel path when needed
    if let Some(goal_m) = stalker.target_pos_m
        && (stalker.waypoints.is_empty() || stalker.repath_timer_s <= 0.0)
    {
        stalker.waypoints =
            find_voxel_path(world, stalker.feet_pos_m, goal_m, DEFAULT_PATH_MAX_NODES);
        stalker.repath_timer_s = match stalker.state {
            StalkerState::Hunt => 0.45,
            StalkerState::Investigate => 0.75,
            StalkerState::Patrol | StalkerState::AttackCooldown | StalkerState::Stunned => 1.25,
        };
        if stalker.waypoints.is_empty() && stalker.state == StalkerState::Patrol {
            stalker.target_pos_m = None;
        }
    }

    // 5. Follow waypoints & steer smoothly without clipping into solid voxels
    let (patrol_spd, invest_spd, hunt_spd, cooldown_spd) =
        stalker_speeds_for_tier(stalker.depth_tier);
    let speed = match stalker.state {
        StalkerState::Patrol => patrol_spd,
        StalkerState::Investigate => invest_spd,
        StalkerState::Hunt => hunt_spd,
        StalkerState::AttackCooldown => cooldown_spd,
        StalkerState::Stunned => 0.0,
    };

    while let Some(&wp) = stalker.waypoints.first() {
        let horiz = Vec3::new(
            wp.x - stalker.feet_pos_m.x,
            0.0,
            wp.z - stalker.feet_pos_m.z,
        )
        .length();
        let vert = (wp.y - stalker.feet_pos_m.y).abs();
        if horiz < 0.22 && vert < 0.45 {
            stalker.waypoints.remove(0);
        } else {
            break;
        }
    }

    let prev_pos = stalker.feet_pos_m;
    if let Some(&wp) = stalker.waypoints.first() {
        let to_wp = wp - stalker.feet_pos_m;
        let horiz = Vec3::new(to_wp.x, 0.0, to_wp.z);
        let horiz_len = horiz.length();

        if horiz_len > 1e-4 {
            // Bevy forward is -Z: `Quat::from_rotation_y(yaw) * Vec3::NEG_Z == (-sin(yaw), 0, -cos(yaw))`
            let target_yaw = (-horiz.x).atan2(-horiz.z);
            let delta_yaw = wrap_angle_pi(target_yaw - stalker.yaw);
            stalker.yaw = wrap_angle_pi(stalker.yaw + delta_yaw * (9.5 * dt).min(1.0));
        }

        let max_step = speed * dt;
        let horiz_move = if horiz_len > max_step {
            horiz * (max_step / horiz_len)
        } else {
            horiz
        };
        let vert_step = to_wp.y.clamp(-speed * 1.4 * dt, speed * 1.4 * dt);

        // Apply vertical first if stepping up onto a ledge so horizontal clears the block edge
        let mut next_pos = stalker.feet_pos_m;
        if vert_step > 0.0 {
            let try_y = next_pos + Vec3::Y * vert_step;
            if !stalker_body_overlaps_solid(world, try_y) {
                next_pos.y = try_y.y;
            }
        }

        let try_xz = next_pos + horiz_move;
        if !stalker_body_overlaps_solid(world, try_xz) {
            next_pos.x = try_xz.x;
            next_pos.z = try_xz.z;
        } else {
            // Axis-separated fallback so the stalker slides along voxel edges cleanly
            let try_x = next_pos + Vec3::X * horiz_move.x;
            if !stalker_body_overlaps_solid(world, try_x) {
                next_pos.x = try_x.x;
            }
            let try_z = next_pos + Vec3::Z * horiz_move.z;
            if !stalker_body_overlaps_solid(world, try_z) {
                next_pos.z = try_z.z;
            }
        }

        if vert_step < 0.0 {
            let try_y = next_pos + Vec3::Y * vert_step;
            if !stalker_body_overlaps_solid(world, try_y) {
                next_pos.y = try_y.y;
            }
        }

        stalker.feet_pos_m = next_pos;

        // Pop waypoint if reached after moving
        let rem = wp - stalker.feet_pos_m;
        if Vec3::new(rem.x, 0.0, rem.z).length() < 0.22 && rem.y.abs() < 0.35 {
            stalker.waypoints.remove(0);
        }
    } else if stalker.state == StalkerState::Patrol {
        stalker.target_pos_m = None;
    }

    // 6. Footstep stride accumulation
    let moved_m = (stalker.feet_pos_m - prev_pos).length();
    stalker.step_accum_m += moved_m;
    if stalker.step_accum_m >= FOOTSTEP_STRIDE_M {
        stalker.step_accum_m -= FOOTSTEP_STRIDE_M;
        let vol = match stalker.state {
            StalkerState::Hunt => 0.85,
            StalkerState::Investigate => 0.58,
            StalkerState::Patrol | StalkerState::AttackCooldown | StalkerState::Stunned => 0.38,
        };
        output.emit_footstep = Some((stalker.feet_pos_m, vol));
    }

    // 7. Periodic eerie vocalization so players can track the blind stalker by its cavern echoes
    stalker.vocal_timer_s -= dt;
    if stalker.vocal_timer_s <= 0.0 && output.emit_vocal.is_none() {
        let jitter = ((deterministic_hash(
            (stalker.monster_id as u64) ^ (stalker.feet_pos_m.x.to_bits() as u64),
        ) & 0xff) as f32)
            / 255.0;
        let (next_interval, vol, pitch) = match stalker.state {
            StalkerState::Hunt => (3.0 + jitter * 1.4, 0.85, 1.12),
            StalkerState::Investigate => (4.5 + jitter * 1.8, 0.65, 1.00),
            StalkerState::Patrol | StalkerState::AttackCooldown | StalkerState::Stunned => {
                (6.5 + jitter * 2.6, 0.50, 0.88)
            }
        };
        stalker.vocal_timer_s = next_interval;
        output.emit_vocal = Some((SynthClip::MonsterGrowl, vol, pitch));
    }

    output
}

/// Spawns the Bevy 0.20 blocky voxel visual hierarchy for a Blind Acoustic Stalker (~2.15m tall):
/// spindly obsidian legs, hunched ribcage torso, eyeless elongated skull with pale bone-like
/// acoustic ear-crests, and long claws. Faces `-Z` in local space.
pub fn spawn_stalker_visual(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    feet_pos_m: Vec3,
) -> Entity {
    let obsidian_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.05, 0.055, 0.075),
        perceptual_roughness: 0.72,
        metallic: 0.18,
        ..default()
    });
    let sinew_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.11, 0.09, 0.13),
        perceptual_roughness: 0.86,
        ..default()
    });
    let bone_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.76, 0.72, 0.64),
        emissive: LinearRgba::new(0.09, 0.08, 0.07, 1.0),
        perceptual_roughness: 0.62,
        ..default()
    });

    let shin_mesh = meshes.add(Cuboid::new(0.11, 0.66, 0.12));
    let thigh_mesh = meshes.add(Cuboid::new(0.15, 0.58, 0.16));
    let rib_mesh = meshes.add(Cuboid::new(0.50, 0.08, 0.34));
    let upper_arm_mesh = meshes.add(Cuboid::new(0.11, 0.58, 0.11));
    let forearm_mesh = meshes.add(Cuboid::new(0.095, 0.64, 0.10));
    let claw_mesh = meshes.add(Cuboid::new(0.035, 0.28, 0.055));
    let ear_fin_mesh = meshes.add(Cuboid::new(0.055, 0.38, 0.26));

    commands
        .spawn((
            Transform::from_translation(feet_pos_m),
            Visibility::default(),
        ))
        .with_children(|root| {
            // Spindly digitigrade legs (left & right)
            for side in [-1.0f32, 1.0f32] {
                root.spawn((
                    Mesh3d(shin_mesh.clone()),
                    MeshMaterial3d(obsidian_mat.clone()),
                    Transform::from_xyz(side * 0.21, 0.33, 0.08)
                        .with_rotation(Quat::from_rotation_x(0.16)),
                ));
                root.spawn((
                    Mesh3d(thigh_mesh.clone()),
                    MeshMaterial3d(obsidian_mat.clone()),
                    Transform::from_xyz(side * 0.22, 0.86, -0.02)
                        .with_rotation(Quat::from_rotation_x(-0.24)),
                ));
            }

            // Pelvis
            root.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.42, 0.20, 0.26))),
                MeshMaterial3d(sinew_mat.clone()),
                Transform::from_xyz(0.0, 1.12, 0.04),
            ));

            // Hunched spine & dark core torso
            root.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.24, 0.72, 0.24))),
                MeshMaterial3d(obsidian_mat.clone()),
                Transform::from_xyz(0.0, 1.48, -0.05)
                    .with_rotation(Quat::from_rotation_x(-0.34)),
            ));

            // Exposed stepped ribcage plates
            for (i, (y, z)) in [(1.34, -0.03), (1.48, -0.08), (1.62, -0.13)]
                .into_iter()
                .enumerate()
            {
                let mat = if i == 1 {
                    bone_mat.clone()
                } else {
                    sinew_mat.clone()
                };
                root.spawn((
                    Mesh3d(rib_mesh.clone()),
                    MeshMaterial3d(mat),
                    Transform::from_xyz(0.0, y, z)
                        .with_rotation(Quat::from_rotation_x(-0.28)),
                ));
            }

            // Broad hunched shoulder hunch
            root.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.74, 0.22, 0.30))),
                MeshMaterial3d(obsidian_mat.clone()),
                Transform::from_xyz(0.0, 1.76, -0.15),
            ));

            // Craned neck
            root.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.16, 0.24, 0.18))),
                MeshMaterial3d(sinew_mat.clone()),
                Transform::from_xyz(0.0, 1.85, -0.25)
                    .with_rotation(Quat::from_rotation_x(-0.45)),
            ));

            // Elongated eyeless skull (no eyes — smooth blind predator cranium)
            root.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.28, 0.28, 0.48))),
                MeshMaterial3d(obsidian_mat.clone()),
                Transform::from_xyz(0.0, 1.93, -0.37)
                    .with_rotation(Quat::from_rotation_x(0.14)),
            ));

            // Pale bone acoustic mandible / lower jaw
            root.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.20, 0.08, 0.34))),
                MeshMaterial3d(bone_mat.clone()),
                Transform::from_xyz(0.0, 1.77, -0.40)
                    .with_rotation(Quat::from_rotation_x(0.22)),
            ));

            // Dorsal pale acoustic crest reaching ~2.14m height
            root.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.05, 0.15, 0.38))),
                MeshMaterial3d(bone_mat.clone()),
                Transform::from_xyz(0.0, 2.07, -0.30)
                    .with_rotation(Quat::from_rotation_x(0.22)),
            ));

            // Flared pale bone-like acoustic ear-fins (sweeping up to ~2.15m)
            for side in [-1.0f32, 1.0f32] {
                root.spawn((
                    Mesh3d(ear_fin_mesh.clone()),
                    MeshMaterial3d(bone_mat.clone()),
                    Transform::from_xyz(side * 0.22, 1.98, -0.24).with_rotation(
                        Quat::from_rotation_y(side * 0.35)
                            * Quat::from_rotation_x(-0.30)
                            * Quat::from_rotation_z(-side * 0.42),
                    ),
                ));
            }

            // Long spindly arms + pale wicked claws
            for side in [-1.0f32, 1.0f32] {
                root.spawn((
                    Mesh3d(upper_arm_mesh.clone()),
                    MeshMaterial3d(obsidian_mat.clone()),
                    Transform::from_xyz(side * 0.40, 1.48, -0.18)
                        .with_rotation(Quat::from_rotation_x(0.18)),
                ));
                root.spawn((
                    Mesh3d(forearm_mesh.clone()),
                    MeshMaterial3d(obsidian_mat.clone()),
                    Transform::from_xyz(side * 0.42, 0.94, -0.30)
                        .with_rotation(Quat::from_rotation_x(0.28)),
                ));
                for finger in [-0.045f32, 0.0, 0.045f32] {
                    root.spawn((
                        Mesh3d(claw_mesh.clone()),
                        MeshMaterial3d(bone_mat.clone()),
                        Transform::from_xyz(side * 0.42 + finger, 0.54, -0.39)
                            .with_rotation(Quat::from_rotation_x(0.32)),
                    ));
                }
            }
        })
        .id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::UVec3;
    use voxel_core::VoxelId;

    fn build_flat_cave_room(size_chunks: UVec3) -> (VoxelConfig, VoxelWorld) {
        let cfg = VoxelConfig::half_player_scale(size_chunks);
        let mut world = VoxelWorld::new(cfg);
        let max = cfg.world_voxels();

        // Solid floor at y = 4 across the test area
        for z in 1..(max.z - 1).min(28) {
            for x in 1..(max.x - 1).min(28) {
                world.set(IVec3::new(x, 4, z), VoxelId::CaveStone);
            }
        }
        (cfg, world)
    }

    #[test]
    fn stalker_ignores_whisper_behind_stone_wall_and_hears_after_breach() {
        let (cfg, mut world) = build_flat_cave_room(UVec3::splat(1));

        // Seal a 2-voxel-thick stone wall across the entire room cross-section at x = 12..=13
        for y in 0..32 {
            for z in 0..32 {
                world.set(IVec3::new(12, y, z), VoxelId::CaveStone);
                world.set(IVec3::new(13, y, z), VoxelId::CaveStone);
            }
        }

        let stalker_feet = voxel_feet_world_pos(cfg, IVec3::new(17, 5, 10));
        let mut stalker = BlindStalker::new(1, stalker_feet, 1);

        let whisper = AcousticEvent {
            pos_m: cfg.voxel_to_world_center(IVec3::new(8, 6, 10)),
            energy: Bands3::new(0.4, 0.7, 0.9),
            sharpness: 0.4,
            kind: AcousticEventKind::VoiceSpeech,
        };

        // During spawn grace period, even loud sounds are ignored (prevents elevator spawn-camping!)
        assert!(stalker.spawn_grace_s > 0.0);
        let loud_arrival = AcousticEvent {
            pos_m: stalker_feet + Vec3::X * 2.0,
            energy: Bands3::new(3.0, 3.0, 3.0),
            sharpness: 1.0,
            kind: AcousticEventKind::ElevatorMachinery,
        };
        assert_eq!(
            process_acoustic_event_for_stalker(&world, &mut stalker, loud_arrival),
            None
        );
        stalker.spawn_grace_s = 0.0;

        // Behind a solid stone wall, the blind stalker genuinely cannot hear the whisper
        let reaction_sealed = process_acoustic_event_for_stalker(&world, &mut stalker, whisper);
        assert_eq!(reaction_sealed, None);
        assert_eq!(stalker.state, StalkerState::Patrol);
        assert!(stalker.target_pos_m.is_none());

        // Breach a doorway through the wall -> stalker now hears the exact same whisper!
        for dy in 5..8 {
            world.set(IVec3::new(12, dy, 10), VoxelId::Air);
            world.set(IVec3::new(13, dy, 10), VoxelId::Air);
        }

        let reaction_open = process_acoustic_event_for_stalker(&world, &mut stalker, whisper);
        assert!(reaction_open.is_some());
        assert_ne!(stalker.state, StalkerState::Patrol);
        assert!(stalker.target_pos_m.is_some());
    }

    #[test]
    fn stalker_hunts_loud_decoy_and_wall_breach() {
        let (cfg, world) = build_flat_cave_room(UVec3::splat(1));
        let stalker_feet = voxel_feet_world_pos(cfg, IVec3::new(18, 5, 10));

        // 1. Noise decoy in open corridor immediately triggers AlertHunt
        let mut stalker_decoy = BlindStalker::new(1, stalker_feet, 1);
        stalker_decoy.spawn_grace_s = 0.0;
        let decoy_event = AcousticEvent {
            pos_m: cfg.voxel_to_world_center(IVec3::new(10, 5, 10)),
            energy: Bands3::new(1.5, 2.6, 3.2),
            sharpness: 0.95,
            kind: AcousticEventKind::DecoyNoise,
        };
        let reaction = process_acoustic_event_for_stalker(&world, &mut stalker_decoy, decoy_event);
        assert_eq!(reaction, Some(StalkerReaction::AlertHunt));
        assert_eq!(stalker_decoy.state, StalkerState::Hunt);
        assert!(stalker_decoy.target_pos_m.is_some());

        // 2. Loud rock wall breach in earshot triggers AlertHunt via salience >= hunt_threshold
        let mut stalker_breach = BlindStalker::new(2, stalker_feet, 2);
        stalker_breach.spawn_grace_s = 0.0;
        let breach_event = AcousticEvent {
            pos_m: cfg.voxel_to_world_center(IVec3::new(12, 6, 10)),
            energy: Bands3::new(2.8, 3.4, 3.8),
            sharpness: 1.0,
            kind: AcousticEventKind::WallBreach,
        };
        let reaction =
            process_acoustic_event_for_stalker(&world, &mut stalker_breach, breach_event);
        assert_eq!(reaction, Some(StalkerReaction::AlertHunt));
        assert_eq!(stalker_breach.state, StalkerState::Hunt);
        assert!(stalker_breach.alert_level > 0.0);
    }

    #[test]
    fn voxel_pathfinder_routes_around_wall_through_doorway() {
        let (cfg, mut world) = build_flat_cave_room(UVec3::splat(1));

        // Build dividing wall at x = 12 across the entire room width, leaving a 2-voxel doorway at z = 17..=18
        for y in 5..14 {
            for z in 0..32 {
                if (z == 17 || z == 18) && (y == 5 || y == 6) {
                    continue;
                }
                world.set(IVec3::new(12, y, z), VoxelId::CaveStone);
            }
        }

        let start_m = voxel_feet_world_pos(cfg, IVec3::new(8, 5, 8));
        let goal_m = voxel_feet_world_pos(cfg, IVec3::new(16, 5, 8));

        let path = find_voxel_path(&world, start_m, goal_m, 600);
        assert!(!path.is_empty(), "expected valid path through doorway");

        let last = *path.last().unwrap();
        assert!(
            last.distance(goal_m) < 0.05,
            "path should reach goal {goal_m:?}, ended at {last:?}"
        );

        let mut passed_through_doorway = false;
        for &wp in &path {
            let v = cfg.world_to_voxel(wp + Vec3::Y * 0.05);
            assert!(
                is_walkable_voxel(&world, v),
                "waypoint {v:?} must be walkable"
            );
            if v.x == 12 && (v.z == 17 || v.z == 18) {
                passed_through_doorway = true;
            }
        }
        assert!(
            passed_through_doorway,
            "path must route through the z=17..=18 doorway at x=12"
        );
    }

    #[test]
    fn stalker_strike_respects_wall_and_enters_attack_cooldown() {
        let (cfg, mut world) = build_flat_cave_room(UVec3::splat(1));
        let stalker_feet = voxel_feet_world_pos(cfg, IVec3::new(10, 5, 10));
        let player_feet = voxel_feet_world_pos(cfg, IVec3::new(11, 5, 10));

        // Separate them with a wall right between x=10 and x=12 (wait: place player at x=12, wall at x=11)
        let blocked_player = voxel_feet_world_pos(cfg, IVec3::new(12, 5, 10));
        world.set(IVec3::new(11, 5, 10), VoxelId::CaveStone);
        world.set(IVec3::new(11, 6, 10), VoxelId::CaveStone);

        let mut stalker = BlindStalker::new(3, stalker_feet, 1);
        stalker.state = StalkerState::Hunt;
        stalker.state_timer_s = 10.0;

        let blocked_out = step_stalker_ai(&world, &mut stalker, &[(1, blocked_player)], 0.05);
        assert!(
            blocked_out.strike_target.is_none(),
            "stalker must not strike through a solid stone wall"
        );

        // Clear the wall and place player within 1.85m unobstructed
        world.set(IVec3::new(11, 5, 10), VoxelId::Air);
        world.set(IVec3::new(11, 6, 10), VoxelId::Air);

        let strike_out = step_stalker_ai(&world, &mut stalker, &[(42, player_feet)], 0.05);
        assert_eq!(strike_out.strike_target, Some((42, stalker_feet)));
        assert_eq!(stalker.state, StalkerState::AttackCooldown);
        assert!(matches!(
            strike_out.emit_vocal,
            Some((SynthClip::MonsterScreech, _, _))
        ));
    }

    #[test]
    fn stalker_speeds_scale_by_tier_and_stay_slower_than_walking() {
        let (p1, i1, h1, _) = stalker_speeds_for_tier(1);
        let (p2, i2, h2, _) = stalker_speeds_for_tier(2);
        let (p3, i3, h3, _) = stalker_speeds_for_tier(3);
        assert!(p1 < p2 && p2 < p3);
        assert!(i1 < i2 && i2 < i3);
        assert!(h1 < h2 && h2 < h3);
        assert!(h3 < 3.5, "even Tier 3 hunt speed should stay below player walk");
    }

    #[test]
    fn stalker_can_be_stunned_and_killed_by_strikes() {
        let (cfg, world) = build_flat_cave_room(UVec3::splat(1));
        let start = voxel_feet_world_pos(cfg, IVec3::new(10, 5, 10));
        let mut stalker = BlindStalker::new(7, start, 1);
        stalker.spawn_grace_s = 0.0;
        assert_eq!(stalker.hp, 3);

        // Hit 1 stuns & knocks back
        let r1 = apply_stalker_hit(&world, &mut stalker, 1, 3.0, Vec3::X);
        assert_eq!(
            r1,
            StalkerHitResult::Stunned {
                remaining_hp: 2,
                max_hp: 3
            }
        );
        assert_eq!(stalker.state, StalkerState::Stunned);
        assert!(stalker.feet_pos_m.x > start.x);

        // While stunned, cannot strike adjacent player or react to sounds
        let pos_now = stalker.feet_pos_m;
        let step = step_stalker_ai(&world, &mut stalker, &[(1, pos_now)], 0.1);
        assert!(step.strike_target.is_none());

        // Hit 2 & Hit 3 kill the stalker
        assert!(matches!(
            apply_stalker_hit(&world, &mut stalker, 1, 3.0, Vec3::X),
            StalkerHitResult::Stunned { remaining_hp: 1, .. }
        ));
        assert_eq!(
            apply_stalker_hit(&world, &mut stalker, 1, 3.0, Vec3::X),
            StalkerHitResult::Killed
        );
    }
}
