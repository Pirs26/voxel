use glam::{IVec3, Vec3};
use voxel_core::{VoxelConfig, VoxelWorld};

/// Scale-agnostic meter-based player AABB dimensions.
/// Works identically at 1/2-height voxels (0.9m) and 1/4-height voxels (0.45m).
pub const PLAYER_HALF_WIDTH_M: f32 = 0.32;
pub const PLAYER_HEIGHT_STAND_M: f32 = 1.75;
pub const PLAYER_HEIGHT_CROUCH_M: f32 = 1.15;
pub const PLAYER_EYE_STAND_M: f32 = 1.58;
pub const PLAYER_EYE_CROUCH_M: f32 = 0.98;
/// Heavy, snappy gravity (m/s²) so characters feel grounded rather than floaty.
pub const PLAYER_GRAVITY_M_S2: f32 = 32.0;
/// Single-tick upward jump impulse (m/s): apex ~1.17m (clears 0.90m block crisply) in ~0.54s.
pub const PLAYER_JUMP_VELOCITY_M_S: f32 = 8.65;
/// Only auto-step ledges up to knee height (~0.50m).
/// At 1/2-height voxels (0.90m), players must jump onto blocks; at 1/4-height (0.45m), 1-voxel steps are smooth.
pub const MAX_AUTO_STEP_M: f32 = 0.50;

/// Returns `true` if placing a solid voxel at `voxel` would intersect a player's AABB hitbox
/// (with a small 2 cm safety margin so blocks never clip a player's camera or trap them inside).
pub fn voxel_overlaps_player_aabb(
    cfg: VoxelConfig,
    voxel: IVec3,
    feet_pos_m: Vec3,
    height_m: f32,
) -> bool {
    let vs = cfg.voxel_size_m;
    let vmin = voxel.as_vec3() * vs;
    let vmax = (voxel + IVec3::ONE).as_vec3() * vs;

    let pad_xz = PLAYER_HALF_WIDTH_M + 0.02;
    let pmin = Vec3::new(feet_pos_m.x - pad_xz, feet_pos_m.y + 0.01, feet_pos_m.z - pad_xz);
    let pmax = Vec3::new(
        feet_pos_m.x + pad_xz,
        feet_pos_m.y + height_m + 0.02,
        feet_pos_m.z + pad_xz,
    );

    vmin.x < pmax.x
        && vmax.x > pmin.x
        && vmin.y < pmax.y
        && vmax.y > pmin.y
        && vmin.z < pmax.z
        && vmax.z > pmin.z
}

/// Checks whether an axis-aligned box `[min_m, max_m]` overlaps any solid voxel.
pub fn aabb_intersects_world(world: &VoxelWorld, min_m: Vec3, max_m: Vec3) -> bool {
    let cfg = world.config();
    let eps = 0.001;
    let vmin = cfg.world_to_voxel(min_m + Vec3::splat(eps));
    let vmax = cfg.world_to_voxel(max_m - Vec3::splat(eps));

    for y in vmin.y..=vmax.y {
        for z in vmin.z..=vmax.z {
            for x in vmin.x..=vmax.x {
                if world.is_solid(glam::IVec3::new(x, y, z)) {
                    return true;
                }
            }
        }
    }
    false
}

/// Sweeps a player box (anchored at bottom-center `feet_pos_m`) along `delta_m`
/// axis-by-axis with automatic smooth step-up for stairs/ledges up to 0.95m!
pub fn move_and_slide(
    world: &VoxelWorld,
    mut feet_pos_m: Vec3,
    mut vel_m: Vec3,
    height_m: f32,
    dt: f32,
) -> (Vec3, Vec3, bool) {
    let half = Vec3::new(PLAYER_HALF_WIDTH_M, 0.0, PLAYER_HALF_WIDTH_M);
    let box_at = |p: Vec3| (p - half, p + half + Vec3::Y * height_m);

    let allow_auto_step = world.voxel_size_m() <= MAX_AUTO_STEP_M;

    // X axis (with auto step-up ONLY for small <= 0.5m voxels)
    let try_x = feet_pos_m + Vec3::X * (vel_m.x * dt);
    let (mn, mx) = box_at(try_x);
    if !aabb_intersects_world(world, mn, mx) {
        feet_pos_m.x = try_x.x;
    } else if allow_auto_step {
        let step_h = world.voxel_size_m() + 0.05;
        let stepped = try_x + Vec3::Y * step_h;
        let (smn, smx) = box_at(stepped);
        if !aabb_intersects_world(world, smn, smx) {
            feet_pos_m = stepped;
        } else {
            vel_m.x = 0.0;
        }
    } else {
        vel_m.x = 0.0;
    }

    // Z axis (with auto step-up ONLY for small <= 0.5m voxels)
    let try_z = feet_pos_m + Vec3::Z * (vel_m.z * dt);
    let (mn, mx) = box_at(try_z);
    if !aabb_intersects_world(world, mn, mx) {
        feet_pos_m.z = try_z.z;
    } else if allow_auto_step {
        let step_h = world.voxel_size_m() + 0.05;
        let stepped = try_z + Vec3::Y * step_h;
        let (smn, smx) = box_at(stepped);
        if !aabb_intersects_world(world, smn, smx) {
            feet_pos_m = stepped;
        } else {
            vel_m.z = 0.0;
        }
    } else {
        vel_m.z = 0.0;
    }

    // Y axis (gravity & ceiling)
    let mut grounded = false;
    let try_y = feet_pos_m + Vec3::Y * (vel_m.y * dt);
    let (mn, mx) = box_at(try_y);
    if !aabb_intersects_world(world, mn, mx) {
        feet_pos_m.y = try_y.y;
    } else {
        if vel_m.y < 0.0 {
            grounded = true;
            // Snap gently onto voxel top surface
            let vs = world.voxel_size_m();
            let snapped_y = (feet_pos_m.y / vs).floor() * vs;
            let cand = Vec3::new(feet_pos_m.x, snapped_y, feet_pos_m.z);
            let (cmn, cmx) = box_at(cand);
            if !aabb_intersects_world(world, cmn, cmx) {
                feet_pos_m.y = snapped_y;
            }
        }
        vel_m.y = 0.0;
    }

    // Probe 3cm below feet for stable grounded detection
    if !grounded && vel_m.y <= 0.05 {
        let probe = feet_pos_m - Vec3::Y * 0.03;
        let (pmn, pmx) = box_at(probe);
        if aabb_intersects_world(world, pmn, pmx) {
            grounded = true;
        }
    }

    (feet_pos_m, vel_m, grounded)
}

/// Ray vs axis-aligned bounding box intersection (`origin + dir * t`, `t in [0.0, max_dist]`).
/// Returns the nearest positive hit distance `t`, or `0.0` if the ray origin starts inside the box.
pub fn ray_aabb_intersect(
    origin: Vec3,
    dir: Vec3,
    min_m: Vec3,
    max_m: Vec3,
    max_dist: f32,
) -> Option<f32> {
    let mut t_min = 0.0f32;
    let mut t_max = max_dist;

    for axis in 0..3 {
        let d = dir[axis];
        if d.abs() < 1e-6 {
            if origin[axis] < min_m[axis] || origin[axis] > max_m[axis] {
                return None;
            }
        } else {
            let inv = 1.0 / d;
            let mut t0 = (min_m[axis] - origin[axis]) * inv;
            let mut t1 = (max_m[axis] - origin[axis]) * inv;
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            t_min = t_min.max(t0);
            t_max = t_max.min(t1);
            if t_max < t_min {
                return None;
            }
        }
    }

    if t_min <= max_dist && t_max >= 0.0 {
        Some(t_min.max(0.0))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heavy_jump_physics_clears_one_voxel_without_floatiness() {
        let apex_m = (PLAYER_JUMP_VELOCITY_M_S * PLAYER_JUMP_VELOCITY_M_S)
            / (2.0 * PLAYER_GRAVITY_M_S2);
        let airtime_s = (2.0 * PLAYER_JUMP_VELOCITY_M_S) / PLAYER_GRAVITY_M_S2;
        assert!(
            apex_m > 1.05 && apex_m < 1.35,
            "expected crisp 1-block jump apex, got {apex_m} m"
        );
        assert!(
            airtime_s > 0.40 && airtime_s < 0.62,
            "expected non-floaty airtime, got {airtime_s} s"
        );
    }

    #[test]
    fn voxel_overlaps_player_aabb_blocks_body_and_allows_floor_or_clear_space() {
        let cfg = VoxelConfig::default(); // 0.90m voxels
        let feet = Vec3::new(4.95, 1.80, 4.95); // center of voxel (5, 2, 5) standing on y=2

        // Feet voxel (5,2,5) and head voxel (5,3,5) MUST overlap
        assert!(voxel_overlaps_player_aabb(
            cfg,
            IVec3::new(5, 2, 5),
            feet,
            PLAYER_HEIGHT_STAND_M
        ));
        assert!(voxel_overlaps_player_aabb(
            cfg,
            IVec3::new(5, 3, 5),
            feet,
            PLAYER_HEIGHT_STAND_M
        ));
        // Floor voxel directly below feet (5,1,5) must NOT overlap
        assert!(!voxel_overlaps_player_aabb(
            cfg,
            IVec3::new(5, 1, 5),
            feet,
            PLAYER_HEIGHT_STAND_M
        ));
        // Adjacent voxel 2 away must NOT overlap
        assert!(!voxel_overlaps_player_aabb(
            cfg,
            IVec3::new(7, 2, 5),
            feet,
            PLAYER_HEIGHT_STAND_M
        ));
    }

    #[test]
    fn ray_aabb_intersect_hits_front_face_and_rejects_misses() {
        let mn = Vec3::new(-0.5, 0.0, -3.5);
        let mx = Vec3::new(0.5, 1.0, -2.5);
        let hit = ray_aabb_intersect(Vec3::new(0.0, 0.5, 0.0), Vec3::NEG_Z, mn, mx, 5.0);
        assert!(hit.is_some());
        assert!((hit.unwrap() - 2.5).abs() < 1e-4);

        // Aiming away misses
        assert!(ray_aabb_intersect(Vec3::new(0.0, 0.5, 0.0), Vec3::Z, mn, mx, 5.0).is_none());
        // Beyond max distance misses
        assert!(ray_aabb_intersect(Vec3::new(0.0, 0.5, 0.0), Vec3::NEG_Z, mn, mx, 2.0).is_none());
    }
}
