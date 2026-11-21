use glam::Vec3;
use voxel_core::VoxelWorld;

/// Scale-agnostic meter-based player AABB dimensions.
/// Works identically at 1/2-height voxels (0.9m) and 1/4-height voxels (0.45m).
pub const PLAYER_HALF_WIDTH_M: f32 = 0.32;
pub const PLAYER_HEIGHT_STAND_M: f32 = 1.75;
pub const PLAYER_HEIGHT_CROUCH_M: f32 = 1.15;
pub const PLAYER_EYE_STAND_M: f32 = 1.58;
pub const PLAYER_EYE_CROUCH_M: f32 = 0.98;
/// Only auto-step ledges up to knee height (~0.50m).
/// At 1/2-height voxels (0.90m), players must jump onto blocks; at 1/4-height (0.45m), 1-voxel steps are smooth.
pub const MAX_AUTO_STEP_M: f32 = 0.50;

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
