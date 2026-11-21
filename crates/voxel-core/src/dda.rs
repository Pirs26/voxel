use glam::{IVec3, Vec3};

use crate::{config::VoxelConfig, material::VoxelId, world::VoxelWorld};

/// Result of a solid voxel raycast in world meters.
#[derive(Clone, Copy, Debug)]
pub struct RaycastHit {
    /// Grid coordinate of the hit solid voxel.
    pub voxel: IVec3,
    /// Grid coordinate of the air voxel immediately before the hit (for placement).
    pub previous_voxel: IVec3,
    /// Hit point in world meters.
    pub point_m: Vec3,
    /// Outward face normal of the hit voxel face.
    pub normal: Vec3,
    /// Distance from ray origin in world meters.
    pub distance_m: f32,
    /// Material of the hit voxel.
    pub material: VoxelId,
}

/// 3D Amanatides-Woo DDA raycast in world meters with fast chunk/brick air skipping.
pub fn raycast_solid(
    world: &VoxelWorld,
    origin_m: Vec3,
    dir: Vec3,
    max_dist_m: f32,
) -> Option<RaycastHit> {
    let dir_len = dir.length();
    if dir_len < 1e-6 || max_dist_m <= 0.0 {
        return None;
    }
    let dir = dir / dir_len;
    let vs = world.voxel_size_m();
    let origin_v = origin_m / vs;
    let max_dist_v = max_dist_m / vs;

    let mut voxel = origin_v.floor().as_ivec3();
    let step = IVec3::new(
        if dir.x >= 0.0 { 1 } else { -1 },
        if dir.y >= 0.0 { 1 } else { -1 },
        if dir.z >= 0.0 { 1 } else { -1 },
    );

    let t_delta = Vec3::new(
        if dir.x.abs() > 1e-8 { 1.0 / dir.x.abs() } else { f32::INFINITY },
        if dir.y.abs() > 1e-8 { 1.0 / dir.y.abs() } else { f32::INFINITY },
        if dir.z.abs() > 1e-8 { 1.0 / dir.z.abs() } else { f32::INFINITY },
    );

    let mut t_max = Vec3::new(
        if dir.x > 1e-8 {
            ((voxel.x as f32 + 1.0) - origin_v.x) / dir.x
        } else if dir.x < -1e-8 {
            (origin_v.x - voxel.x as f32) / -dir.x
        } else {
            f32::INFINITY
        },
        if dir.y > 1e-8 {
            ((voxel.y as f32 + 1.0) - origin_v.y) / dir.y
        } else if dir.y < -1e-8 {
            (origin_v.y - voxel.y as f32) / -dir.y
        } else {
            f32::INFINITY
        },
        if dir.z > 1e-8 {
            ((voxel.z as f32 + 1.0) - origin_v.z) / dir.z
        } else if dir.z < -1e-8 {
            (origin_v.z - voxel.z as f32) / -dir.z
        } else {
            f32::INFINITY
        },
    );

    // Check starting voxel (if origin is already inside solid)
    let start_mat = world.get(voxel);
    if start_mat.is_solid() {
        return Some(RaycastHit {
            voxel,
            previous_voxel: voxel,
            point_m: origin_m,
            normal: -dir,
            distance_m: 0.0,
            material: start_mat,
        });
    }

    let max_steps = (max_dist_v * 3.0) as usize + 16;
    for _ in 0..max_steps {
        let prev_voxel = voxel;
        let t;
        let last_normal;

        if t_max.x <= t_max.y && t_max.x <= t_max.z {
            t = t_max.x;
            t_max.x += t_delta.x;
            voxel.x += step.x;
            last_normal = Vec3::new(-step.x as f32, 0.0, 0.0);
        } else if t_max.y <= t_max.z {
            t = t_max.y;
            t_max.y += t_delta.y;
            voxel.y += step.y;
            last_normal = Vec3::new(0.0, -step.y as f32, 0.0);
        } else {
            t = t_max.z;
            t_max.z += t_delta.z;
            voxel.z += step.z;
            last_normal = Vec3::new(0.0, 0.0, -step.z as f32);
        }

        if t > max_dist_v {
            return None;
        }

        // Fast brick occupancy check inside chunk before full lookup
        if !world.config().in_bounds(voxel) {
            return Some(RaycastHit {
                voxel,
                previous_voxel: prev_voxel,
                point_m: origin_m + dir * (t * vs),
                normal: last_normal,
                distance_m: t * vs,
                material: VoxelId::Bedrock,
            });
        }

        let (cc, lc) = VoxelConfig::split_voxel_coord(voxel);
        if let Some(chunk) = world.get_chunk(cc) {
            if chunk.brick_is_occupied(lc.x as usize, lc.y as usize, lc.z as usize) {
                let mat = chunk.get(lc.x as usize, lc.y as usize, lc.z as usize);
                if mat.is_solid() {
                    return Some(RaycastHit {
                        voxel,
                        previous_voxel: prev_voxel,
                        point_m: origin_m + dir * (t * vs),
                        normal: last_normal,
                        distance_m: t * vs,
                        material: mat,
                    });
                }
            }
        }
    }

    None
}

/// Exact segment traversal in meters for acoustic density occlusion.
///
/// Walks every voxel intersected by the line segment `from_m -> to_m` and invokes
/// `visit(material, segment_len_meters)` for every solid voxel traversed.
pub fn march_segment_occlusion<F>(world: &VoxelWorld, from_m: Vec3, to_m: Vec3, mut visit: F)
where
    F: FnMut(VoxelId, f32),
{
    let delta = to_m - from_m;
    let total_dist_m = delta.length();
    if total_dist_m < 1e-5 {
        return;
    }

    let dir = delta / total_dist_m;
    let vs = world.voxel_size_m();
    let origin_v = from_m / vs;
    let total_dist_v = total_dist_m / vs;

    let mut voxel = origin_v.floor().as_ivec3();
    let step = IVec3::new(
        if dir.x >= 0.0 { 1 } else { -1 },
        if dir.y >= 0.0 { 1 } else { -1 },
        if dir.z >= 0.0 { 1 } else { -1 },
    );

    let t_delta = Vec3::new(
        if dir.x.abs() > 1e-8 { 1.0 / dir.x.abs() } else { f32::INFINITY },
        if dir.y.abs() > 1e-8 { 1.0 / dir.y.abs() } else { f32::INFINITY },
        if dir.z.abs() > 1e-8 { 1.0 / dir.z.abs() } else { f32::INFINITY },
    );

    let mut t_max = Vec3::new(
        if dir.x > 1e-8 {
            ((voxel.x as f32 + 1.0) - origin_v.x) / dir.x
        } else if dir.x < -1e-8 {
            (origin_v.x - voxel.x as f32) / -dir.x
        } else {
            f32::INFINITY
        },
        if dir.y > 1e-8 {
            ((voxel.y as f32 + 1.0) - origin_v.y) / dir.y
        } else if dir.y < -1e-8 {
            (origin_v.y - voxel.y as f32) / -dir.y
        } else {
            f32::INFINITY
        },
        if dir.z > 1e-8 {
            ((voxel.z as f32 + 1.0) - origin_v.z) / dir.z
        } else if dir.z < -1e-8 {
            (origin_v.z - voxel.z as f32) / -dir.z
        } else {
            f32::INFINITY
        },
    );

    let mut t_curr = 0.0f32;
    let max_steps = (total_dist_v * 3.0) as usize + 16;

    for _ in 0..max_steps {
        let t_next = t_max.x.min(t_max.y).min(t_max.z).min(total_dist_v);
        let seg_v = (t_next - t_curr).max(0.0);

        if seg_v > 0.0 {
            let mat = world.get(voxel);
            if mat.is_solid() {
                visit(mat, seg_v * vs);
            }
        }

        if t_next >= total_dist_v - 1e-6 {
            break;
        }

        t_curr = t_next;
        if t_max.x <= t_max.y && t_max.x <= t_max.z {
            t_max.x += t_delta.x;
            voxel.x += step.x;
        } else if t_max.y <= t_max.z {
            t_max.y += t_delta.y;
            voxel.y += step.y;
        } else {
            t_max.z += t_delta.z;
            voxel.z += step.z;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::UVec3;

    #[test]
    fn raycast_and_occlusion_wall_thickness() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(2));
        let mut world = VoxelWorld::new(cfg);

        // Place a 2-voxel-thick CaveStone wall at x = 10, 11
        for y in 5..15 {
            for z in 5..15 {
                world.set(IVec3::new(10, y, z), VoxelId::CaveStone);
                world.set(IVec3::new(11, y, z), VoxelId::CaveStone);
            }
        }

        let from = cfg.voxel_to_world_center(IVec3::new(5, 10, 10));
        let to = cfg.voxel_to_world_center(IVec3::new(15, 10, 10));

        let hit = raycast_solid(&world, from, Vec3::X, 50.0).expect("should hit wall");
        assert_eq!(hit.voxel, IVec3::new(10, 10, 10));
        assert_eq!(hit.normal, -Vec3::X);

        let mut solid_m = 0.0f32;
        march_segment_occlusion(&world, from, to, |mat, dist_m| {
            assert_eq!(mat, VoxelId::CaveStone);
            solid_m += dist_m;
        });

        let expected = 2.0 * cfg.voxel_size_m;
        assert!(
            (solid_m - expected).abs() < 1e-3,
            "expected {expected}m solid wall, got {solid_m}m"
        );
    }
}
