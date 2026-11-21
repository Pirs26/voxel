use glam::{IVec3, Vec3};

use crate::{material::VoxelId, world::VoxelWorld};

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

/// Inspects an in-bounds voxel and returns:
/// - `Err(mat)` if the voxel is solid,
/// - `Ok(32)` if the entire owning 32³ chunk is missing or empty,
/// - `Ok(8)` if the owning 8³ brick is empty,
/// - `Ok(1)` if the voxel is air inside an occupied 8³ brick.
#[inline]
fn inspect_in_bounds_voxel(world: &VoxelWorld, voxel: IVec3) -> Result<i32, VoxelId> {
    let cc = IVec3::new(voxel.x >> 5, voxel.y >> 5, voxel.z >> 5);
    match world.get_chunk(cc) {
        None => Ok(32),
        Some(chunk) if chunk.is_empty() => Ok(32),
        Some(chunk) => {
            let lx = (voxel.x & 31) as usize;
            let ly = (voxel.y & 31) as usize;
            let lz = (voxel.z & 31) as usize;
            if !chunk.brick_is_occupied(lx, ly, lz) {
                Ok(8)
            } else {
                let mat = chunk.get(lx, ly, lz);
                if mat.is_solid() {
                    Err(mat)
                } else {
                    Ok(1)
                }
            }
        }
    }
}

/// Exact parametric `t` at which the ray crosses the exit plane of voxel coordinate `coord`
/// along an axis with origin `origin_a`, step sign `step_a`, and `abs_inv_dir_a = 1.0 / |dir_a|`.
#[inline]
fn voxel_exit_t(coord: i32, origin_a: f32, step_a: i32, abs_inv_dir_a: f32) -> f32 {
    if !abs_inv_dir_a.is_finite() {
        return f32::INFINITY;
    }
    let dist = if step_a > 0 {
        (coord + 1) as f32 - origin_a
    } else {
        origin_a - coord as f32
    };
    dist * abs_inv_dir_a
}

/// 3D Amanatides-Woo DDA raycast in world meters with hierarchical 32³ chunk & 8³ brick empty-space skipping.
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
        voxel_exit_t(voxel.x, origin_v.x, step.x, t_delta.x),
        voxel_exit_t(voxel.y, origin_v.y, step.y, t_delta.y),
        voxel_exit_t(voxel.z, origin_v.z, step.z, t_delta.z),
    );

    // Check starting voxel (if origin is already outside world bounds or inside solid)
    if !world.config().in_bounds(voxel) {
        return Some(RaycastHit {
            voxel,
            previous_voxel: voxel,
            point_m: origin_m,
            normal: -dir,
            distance_m: 0.0,
            material: VoxelId::Bedrock,
        });
    }

    let mut skip_size = match inspect_in_bounds_voxel(world, voxel) {
        Err(start_mat) => {
            return Some(RaycastHit {
                voxel,
                previous_voxel: voxel,
                point_m: origin_m,
                normal: -dir,
                distance_m: 0.0,
                material: start_mat,
            });
        }
        Ok(size) => size,
    };

    let max_steps = (max_dist_v * 3.0) as usize + 16;
    for _ in 0..max_steps {
        let prev_voxel;
        let t;
        let last_normal;

        if skip_size == 1 {
            prev_voxel = voxel;
            if t_max.x <= t_max.y && t_max.x <= t_max.z {
                t = t_max.x;
                voxel.x += step.x;
                t_max.x = voxel_exit_t(voxel.x, origin_v.x, step.x, t_delta.x);
                last_normal = Vec3::new(-step.x as f32, 0.0, 0.0);
            } else if t_max.y <= t_max.z {
                t = t_max.y;
                voxel.y += step.y;
                t_max.y = voxel_exit_t(voxel.y, origin_v.y, step.y, t_delta.y);
                last_normal = Vec3::new(0.0, -step.y as f32, 0.0);
            } else {
                t = t_max.z;
                voxel.z += step.z;
                t_max.z = voxel_exit_t(voxel.z, origin_v.z, step.z, t_delta.z);
                last_normal = Vec3::new(0.0, 0.0, -step.z as f32);
            }
        } else {
            // Exact hierarchical empty-box skip across the aligned 8³ brick or 32³ chunk.
            // Computes exact parametric exit planes directly from `origin_v` to avoid drift
            // and preserves exact Amanatides-Woo tie-breaking on edges and corners.
            let mask = !(skip_size - 1);
            let bmin = IVec3::new(voxel.x & mask, voxel.y & mask, voxel.z & mask);

            let last_x = if step.x > 0 { bmin.x + skip_size - 1 } else { bmin.x };
            let last_y = if step.y > 0 { bmin.y + skip_size - 1 } else { bmin.y };
            let last_z = if step.z > 0 { bmin.z + skip_size - 1 } else { bmin.z };

            let tx = voxel_exit_t(last_x, origin_v.x, step.x, t_delta.x);
            let ty = voxel_exit_t(last_y, origin_v.y, step.y, t_delta.y);
            let tz = voxel_exit_t(last_z, origin_v.z, step.z, t_delta.z);

            let (exit_axis, t_exit) = if tx <= ty && tx <= tz {
                (0usize, tx)
            } else if ty <= tz {
                (1usize, ty)
            } else {
                (2usize, tz)
            };

            if t_exit > max_dist_v {
                return None;
            }
            t = t_exit;

            // Advance the two non-exit axes to their exact voxel inside `[bmin, bmin + skip_size)`
            // at parametric time `t_exit` with exact DDA tie-breaking.
            let last_arr = [last_x, last_y, last_z];
            for a in 0..3usize {
                if a == exit_axis {
                    continue;
                }
                let td = t_delta[a];
                if !td.is_finite() {
                    continue;
                }
                let lo = bmin[a];
                let hi = lo + skip_size - 1;
                let a_wins_tie = a < exit_axis;
                let should_step = |t_val: f32| -> bool {
                    if a_wins_tie {
                        t_val <= t_exit
                    } else {
                        t_val < t_exit
                    }
                };

                let mut v = ((origin_v[a] + dir[a] * t_exit).floor() as i32).clamp(lo, hi);
                if step[a] > 0 {
                    v = v.max(voxel[a]);
                    while v > voxel[a] && !should_step(voxel_exit_t(v - 1, origin_v[a], 1, td)) {
                        v -= 1;
                    }
                    while v < hi && should_step(voxel_exit_t(v, origin_v[a], 1, td)) {
                        v += 1;
                    }
                } else {
                    v = v.min(voxel[a]);
                    while v < voxel[a] && !should_step(voxel_exit_t(v + 1, origin_v[a], -1, td)) {
                        v += 1;
                    }
                    while v > lo && should_step(voxel_exit_t(v, origin_v[a], -1, td)) {
                        v -= 1;
                    }
                }

                voxel[a] = v;
                t_max[a] = voxel_exit_t(v, origin_v[a], step[a], td);
            }

            // Advance exit axis across the box exit face
            let mut pv = voxel;
            pv[exit_axis] = last_arr[exit_axis];
            prev_voxel = pv;
            voxel[exit_axis] = last_arr[exit_axis] + step[exit_axis];
            t_max[exit_axis] = voxel_exit_t(
                voxel[exit_axis],
                origin_v[exit_axis],
                step[exit_axis],
                t_delta[exit_axis],
            );
            last_normal = match exit_axis {
                0 => Vec3::new(-step.x as f32, 0.0, 0.0),
                1 => Vec3::new(0.0, -step.y as f32, 0.0),
                _ => Vec3::new(0.0, 0.0, -step.z as f32),
            };
        }

        if t > max_dist_v {
            return None;
        }

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

        match inspect_in_bounds_voxel(world, voxel) {
            Err(mat) => {
                return Some(RaycastHit {
                    voxel,
                    previous_voxel: prev_voxel,
                    point_m: origin_m + dir * (t * vs),
                    normal: last_normal,
                    distance_m: t * vs,
                    material: mat,
                });
            }
            Ok(size) => {
                skip_size = size;
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
    use crate::{config::VoxelConfig, worldgen::generate_world};
    use glam::UVec3;

    /// Naive 1-voxel-at-a-time Amanatides-Woo DDA reference implementation (no brick/chunk skipping).
    fn reference_raycast_solid(
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
            let last_normal = if t_max.x <= t_max.y && t_max.x <= t_max.z {
                t = t_max.x;
                t_max.x += t_delta.x;
                voxel.x += step.x;
                Vec3::new(-step.x as f32, 0.0, 0.0)
            } else if t_max.y <= t_max.z {
                t = t_max.y;
                t_max.y += t_delta.y;
                voxel.y += step.y;
                Vec3::new(0.0, -step.y as f32, 0.0)
            } else {
                t = t_max.z;
                t_max.z += t_delta.z;
                voxel.z += step.z;
                Vec3::new(0.0, 0.0, -step.z as f32)
            };

            if t > max_dist_v {
                return None;
            }

            let mat = world.get(voxel);
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

        None
    }

    /// Deterministic SplitMix64 PRNG for reproducible raycast stress tests.
    struct Rng(u64);
    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn next_f32(&mut self) -> f32 {
            ((self.next_u64() >> 40) as f32) / ((1u32 << 24) as f32)
        }

        fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.next_f32()
        }
    }

    fn assert_hits_match(
        label: &str,
        origin: Vec3,
        dir: Vec3,
        max_dist: f32,
        fast: Option<RaycastHit>,
        naive: Option<RaycastHit>,
    ) {
        match (fast, naive) {
            (None, None) => {}
            (Some(f), Some(n)) => {
                assert_eq!(
                    f.voxel, n.voxel,
                    "{label}: voxel mismatch (origin={origin:?}, dir={dir:?}, max_dist={max_dist})"
                );
                assert_eq!(
                    f.previous_voxel, n.previous_voxel,
                    "{label}: previous_voxel mismatch (origin={origin:?}, dir={dir:?})"
                );
                assert_eq!(
                    f.normal, n.normal,
                    "{label}: normal mismatch (origin={origin:?}, dir={dir:?})"
                );
                assert_eq!(
                    f.material, n.material,
                    "{label}: material mismatch (origin={origin:?}, dir={dir:?})"
                );
                assert!(
                    (f.distance_m - n.distance_m).abs() < 1e-3,
                    "{label}: distance mismatch fast={} vs naive={} (origin={origin:?}, dir={dir:?})",
                    f.distance_m,
                    n.distance_m
                );
            }
            (f, n) => panic!(
                "{label}: hit/miss mismatch fast={f:?} vs naive={n:?} (origin={origin:?}, dir={dir:?}, max_dist={max_dist})"
            ),
        }
    }

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

    #[test]
    fn raycast_skipping_matches_naive_reference_in_generated_cave() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(4));
        let (world, info) = generate_world(1337, cfg);
        let mut rng = Rng(0x5EED_C0DE_1234_5678);

        // Diverse origins spanning cavern air, spawn hub, shaft, alcove, and open sky (empty 32³ chunks)
        let origins = [
            info.spawn_pos_m,
            info.grand_cavern_pos_m,
            info.acoustic_beacon_pos_m,
            info.elevator_pos_m + Vec3::new(0.0, 2.0, 0.0),
            // High open sky where entire 32³ chunks are None/empty
            cfg.voxel_to_world_center(IVec3::new(15, 115, 15)),
            cfg.voxel_to_world_center(IVec3::new(110, 118, 110)),
        ];

        // 1) Shoot 300 pseudo-random rays across all origins (both long and short max_dist_m)
        let mut hit_count = 0usize;
        let mut miss_count = 0usize;
        for i in 0..300 {
            let base_origin = origins[i % origins.len()];
            let jitter = Vec3::new(
                rng.range_f32(-1.5, 1.5),
                rng.range_f32(-1.0, 1.0),
                rng.range_f32(-1.5, 1.5),
            );
            let origin = base_origin + jitter;
            let dir = Vec3::new(
                rng.range_f32(-1.0, 1.0),
                rng.range_f32(-1.0, 1.0),
                rng.range_f32(-1.0, 1.0),
            );
            if dir.length_squared() < 1e-4 {
                continue;
            }
            let max_dist = if i % 5 == 0 {
                rng.range_f32(2.0, 14.0)
            } else {
                rng.range_f32(20.0, 140.0)
            };

            let fast = raycast_solid(&world, origin, dir, max_dist);
            let naive = reference_raycast_solid(&world, origin, dir, max_dist);
            if fast.is_some() {
                hit_count += 1;
            } else {
                miss_count += 1;
            }
            assert_hits_match(&format!("cave_ray_{i}"), origin, dir, max_dist, fast, naive);
        }
        assert!(hit_count > 100, "expected plenty of solid hits, got {hit_count}");
        assert!(miss_count > 5, "expected some max_dist misses, got {miss_count}");

        // 2) Axis-aligned and exact 45-degree diagonal corner rays
        let special_dirs = [
            Vec3::X,
            -Vec3::X,
            Vec3::Y,
            -Vec3::Y,
            Vec3::Z,
            -Vec3::Z,
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(0.0, -1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(-1.0, 1.0, -1.0),
        ];
        for (oi, &origin) in origins.iter().enumerate() {
            for (di, &dir) in special_dirs.iter().enumerate() {
                let fast = raycast_solid(&world, origin, dir, 120.0);
                let naive = reference_raycast_solid(&world, origin, dir, 120.0);
                assert_hits_match(
                    &format!("special_o{oi}_d{di}"),
                    origin,
                    dir,
                    120.0,
                    fast,
                    naive,
                );
            }
        }
    }

    #[test]
    fn raycast_chunk_and_brick_boundary_corners() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        let mut world = VoxelWorld::new(cfg);

        // Leave chunk (0,0,0) and (1,0,0) completely empty (None), place a single voxel right on
        // the corner of chunk (2,0,0) and isolated brick corners in chunk (1,1,1).
        world.set(IVec3::new(64, 8, 8), VoxelId::BiolumCrystal);
        world.set(IVec3::new(40, 40, 40), VoxelId::Steel);
        world.set(IVec3::new(47, 40, 40), VoxelId::CrackedRock);
        world.set(IVec3::new(48, 48, 48), VoxelId::Timber);

        let test_cases = [
            // Across two empty 32³ chunks into exact corner voxel (64, 8, 8)
            (
                cfg.voxel_to_world_center(IVec3::new(4, 8, 8)),
                Vec3::X,
                100.0,
            ),
            // Exact 45-degree 3D diagonal across empty chunks & bricks into (40, 40, 40)
            (
                cfg.voxel_to_world_center(IVec3::new(5, 5, 5)),
                Vec3::ONE,
                100.0,
            ),
            // Reverse diagonal from (60, 60, 60) back into (48, 48, 48)
            (
                cfg.voxel_to_world_center(IVec3::new(60, 60, 60)),
                -Vec3::ONE,
                100.0,
            ),
            // Short max_dist stopping inside an empty 32³ chunk
            (
                cfg.voxel_to_world_center(IVec3::new(4, 8, 8)),
                Vec3::X,
                10.0,
            ),
        ];

        for (i, &(origin, dir, max_dist)) in test_cases.iter().enumerate() {
            let fast = raycast_solid(&world, origin, dir, max_dist);
            let naive = reference_raycast_solid(&world, origin, dir, max_dist);
            assert_hits_match(&format!("corner_{i}"), origin, dir, max_dist, fast, naive);
        }
    }
}
