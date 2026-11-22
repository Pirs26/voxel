use std::f32::consts::{PI, TAU};

use glam::Vec3;
use rand::{RngExt, SeedableRng, rngs::SmallRng};
use voxel_core::{VoxelWorld, raycast_solid};

use crate::{
    bands::{Bands3, SPEED_OF_SOUND_M_S},
    occlusion::compute_direct_occlusion,
};

pub const MAX_EARLY_TAPS: usize = 6;

#[derive(Clone, Copy, Debug)]
pub struct PathTraceConfig {
    /// Number of Fibonacci-jittered rays cast from the listener per solve.
    pub reverb_rays: usize,
    /// Maximum surface bounces per ray.
    pub max_bounces: usize,
    /// Maximum ray segment length in meters.
    pub max_ray_dist_m: f32,
    /// Number of candidate first-bounce probes for per-source early reflections.
    pub early_reflection_rays: usize,
}

impl Default for PathTraceConfig {
    fn default() -> Self {
        Self {
            reverb_rays: 72,
            max_bounces: 4,
            max_ray_dist_m: 65.0,
            early_reflection_rays: 28,
        }
    }
}

/// A single directional early reflection echo tap (0..180ms).
#[derive(Clone, Copy, Debug, Default)]
pub struct EarlyTap {
    /// Propagation delay in seconds (`total_path_m / 343.0`).
    pub delay_sec: f32,
    /// 3-band reflected gain.
    pub gain: Bands3,
    /// Unit direction vector from listener toward the reflection wall (for stereo panning).
    pub direction: Vec3,
    /// Whether this tap is active.
    pub active: bool,
}

/// Dynamic room reverb characteristics extracted from path-traced voxel geometry.
#[derive(Clone, Copy, Debug)]
pub struct ReverbEstimate {
    /// Reverberation decay time (time to drop 60 dB) per frequency band in seconds.
    pub rt60_sec: Bands3,
    /// Estimated mean free path (average bounce distance in meters -> drives FDN room size).
    pub mean_free_path_m: f32,
    /// Suggested wet send gain [0.0..1.0] based on enclosure & reflectivity.
    pub wet_mix: f32,
    /// Fraction of rays that hit geometry within `max_ray_dist_m` (0 = open sky, 1 = enclosed).
    pub enclosure: f32,
}

impl Default for ReverbEstimate {
    fn default() -> Self {
        Self {
            rt60_sec: Bands3::new(0.6, 0.5, 0.35),
            mean_free_path_m: 6.0,
            wet_mix: 0.25,
            enclosure: 0.8,
        }
    }
}

impl ReverbEstimate {
    pub fn lerp(self, target: Self, alpha: f32) -> Self {
        Self {
            rt60_sec: self.rt60_sec.lerp(target.rt60_sec, alpha),
            mean_free_path_m: self.mean_free_path_m + (target.mean_free_path_m - self.mean_free_path_m) * alpha,
            wet_mix: self.wet_mix + (target.wet_mix - self.wet_mix) * alpha,
            enclosure: self.enclosure + (target.enclosure - self.enclosure) * alpha,
        }
    }
}

/// Stochastic 3D voxel path-tracer from the listener position to estimate room size,
/// enclosure, and frequency-dependent RT60 reverb decay for the FDN engine.
pub fn trace_listener_reverb(
    world: &VoxelWorld,
    listener_pos_m: Vec3,
    cfg: PathTraceConfig,
    frame_seed: u64,
) -> ReverbEstimate {
    let mut rng = SmallRng::seed_from_u64(frame_seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xA5A5_5A5A);

    let mut total_seg_dist = 0.0f32;
    let mut hit_segments = 0u32;
    let mut primary_hits = 0u32;
    let mut refl_sum = Bands3::ZERO;
    let mut refl_hits = 0u32;

    let n = cfg.reverb_rays.max(8);
    let golden_angle = PI * (3.0 - 5.0f32.sqrt());
    let jitter_yaw = rng.random_range(0.0..TAU);

    for i in 0..n {
        // Fibonacci sphere direction with slight per-frame rotational jitter
        let y = 1.0 - (2.0 * (i as f32 + 0.5)) / (n as f32);
        let r = (1.0 - y * y).max(0.0).sqrt();
        let theta = golden_angle * i as f32 + jitter_yaw;
        let mut dir = Vec3::new(theta.cos() * r, y, theta.sin() * r).normalize();
        let mut origin = listener_pos_m;

        for bounce in 0..cfg.max_bounces {
            let Some(hit) = raycast_solid(world, origin, dir, cfg.max_ray_dist_m) else {
                break;
            };

            if bounce == 0 {
                primary_hits += 1;
            }
            if hit.distance_m > 0.05 {
                total_seg_dist += hit.distance_m;
                hit_segments += 1;
            }

            let mat = hit.material.def().acoustic;
            refl_sum = refl_sum.add(Bands3(mat.reflection_coeff));
            refl_hits += 1;

            // Compute next bounce direction: blend mirror reflection and diffuse hemisphere scatter
            let specular = (dir - 2.0 * dir.dot(hit.normal) * hit.normal).normalize_or_zero();
            let diffuse = sample_cosine_hemisphere(hit.normal, &mut rng);
            dir = specular.lerp(diffuse, mat.scattering).normalize_or_zero();
            if dir.length_squared() < 0.1 {
                dir = hit.normal;
            }
            // Offset slightly off surface to avoid self-intersection
            origin = hit.point_m + hit.normal * 0.04;
        }
    }

    let enclosure = (primary_hits as f32) / (n as f32);
    if hit_segments == 0 || refl_hits == 0 || enclosure < 0.08 {
        return ReverbEstimate {
            rt60_sec: Bands3::new(0.15, 0.12, 0.10),
            mean_free_path_m: 15.0,
            wet_mix: 0.03,
            enclosure,
        };
    }

    let mfp = (total_seg_dist / hit_segments as f32).clamp(1.5, 45.0);
    let mean_refl = refl_sum.scale(1.0 / refl_hits as f32).clamp(0.05, 0.96);

    // Eyring acoustic formula: RT60 = (-60 dB) / (20 * log10(R)) * (MFP / c)
    // Scaled with enclosure factor so open sky leaks reverb energy naturally.
    let mut rt60 = [0.0f32; 3];
    for (b, slot) in rt60.iter_mut().enumerate() {
        let effective_r = (mean_refl.0[b] * (0.35 + 0.65 * enclosure)).clamp(0.05, 0.96);
        let db_loss_per_bounce = -20.0 * effective_r.log10(); // positive dB
        let bounces_to_60db = 60.0 / db_loss_per_bounce.max(0.25);
        let raw_rt60 = bounces_to_60db * (mfp / SPEED_OF_SOUND_M_S);
        // Air absorption damps very long high-frequency tails
        let air_cap = match b {
            0 => 5.5,
            1 => 4.2,
            _ => 2.8,
        };
        *slot = raw_rt60.clamp(0.15, air_cap);
    }

    let mean_r = mean_refl.mean();
    let wet_mix = (enclosure * mean_r * 0.72).clamp(0.04, 0.70);

    ReverbEstimate {
        rt60_sec: Bands3(rt60),
        mean_free_path_m: mfp,
        wet_mix,
        enclosure,
    }
}

/// Traces 1st-bounce directional early reflections (and corner diffraction paths!)
/// between a specific sound source and the listener using next-event estimation.
pub fn trace_early_reflections(
    world: &VoxelWorld,
    source_pos_m: Vec3,
    listener_pos_m: Vec3,
    cfg: PathTraceConfig,
    frame_seed: u64,
) -> [EarlyTap; MAX_EARLY_TAPS] {
    let mut taps = [EarlyTap::default(); MAX_EARLY_TAPS];
    let direct_dist = source_pos_m.distance(listener_pos_m);
    if direct_dist > cfg.max_ray_dist_m {
        return taps;
    }

    let mut rng = SmallRng::seed_from_u64(frame_seed.wrapping_mul(0xD1B5_4A32_D192_ED03) ^ 0x517C_C1B7);
    let n = cfg.early_reflection_rays.max(8);
    let golden_angle = PI * (3.0 - 5.0f32.sqrt());
    let jitter = rng.random_range(0.0..TAU);

    let mut candidates: Vec<EarlyTap> = Vec::with_capacity(n);

    for i in 0..n {
        let y = 1.0 - (2.0 * (i as f32 + 0.5)) / (n as f32);
        let r = (1.0 - y * y).max(0.0).sqrt();
        let theta = golden_angle * i as f32 + jitter;
        let dir = Vec3::new(theta.cos() * r, y, theta.sin() * r).normalize();

        // Cast from listener to find a reflecting wall surface
        let Some(hit) = raycast_solid(world, listener_pos_m, dir, cfg.max_ray_dist_m * 0.65) else {
            continue;
        };
        if hit.distance_m < 0.35 {
            continue;
        }

        // Next-event check: can the sound source see this bounce point?
        let bounce_pt = hit.point_m + hit.normal * 0.08;
        let to_src = source_pos_m - bounce_pt;
        let leg2_dist = to_src.length();
        if leg2_dist < 0.25 || to_src.dot(hit.normal) <= 0.05 {
            continue;
        }

        let total_path = hit.distance_m + leg2_dist;
        let delay_sec = total_path / SPEED_OF_SOUND_M_S;
        if delay_sec > 0.22 {
            continue;
        }

        let leg2_occ = compute_direct_occlusion(world, source_pos_m, bounce_pt, 1.5, cfg.max_ray_dist_m);
        if leg2_occ.solid_thickness_m > 0.5 {
            // Skip heavily blocked secondary paths; focus early taps on clean bounce/corner paths!
            continue;
        }

        let mat_refl = Bands3(hit.material.def().acoustic.reflection_coeff);
        let path_atten = (1.5 / (1.5 + total_path)).clamp(0.0, 1.0);
        let tap_gain = mat_refl
            .mul(leg2_occ.direct_gain)
            .scale(path_atten * 0.55);

        if tap_gain.mean() > 0.01 {
            candidates.push(EarlyTap {
                delay_sec,
                gain: tap_gain,
                direction: dir,
                active: true,
            });
        }
    }

    // Sort strongest first and keep up to MAX_EARLY_TAPS well-separated delays
    candidates.sort_by(|a, b| b.gain.mean().total_cmp(&a.gain.mean()));
    let mut out_idx = 0;
    for cand in candidates {
        if out_idx >= MAX_EARLY_TAPS {
            break;
        }
        // Avoid stacking duplicate taps within 6ms of each other (prevents metallic comb artifacts)
        let too_close = taps[..out_idx]
            .iter()
            .any(|t| (t.delay_sec - cand.delay_sec).abs() < 0.006);
        if !too_close {
            taps[out_idx] = cand;
            out_idx += 1;
        }
    }

    taps
}

fn sample_cosine_hemisphere(normal: Vec3, rng: &mut SmallRng) -> Vec3 {
    let u1: f32 = rng.random();
    let u2: f32 = rng.random();
    let r = u1.sqrt();
    let phi = TAU * u2;
    let x = r * phi.cos();
    let y = r * phi.sin();
    let z = (1.0 - u1).max(0.0).sqrt();

    let up = if normal.y.abs() < 0.95 { Vec3::Y } else { Vec3::X };
    let tangent = up.cross(normal).normalize();
    let bitangent = normal.cross(tangent);
    (tangent * x + bitangent * y + normal * z).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{IVec3, UVec3};
    use voxel_core::{VoxelConfig, VoxelId};

    fn build_box_room(size: i32, mat: VoxelId) -> (VoxelWorld, Vec3) {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(2));
        let mut world = VoxelWorld::new(cfg);
        let start = IVec3::splat(4);
        let end = start + IVec3::splat(size);
        for y in start.y..=end.y {
            for z in start.z..=end.z {
                for x in start.x..=end.x {
                    let shell = x == start.x
                        || x == end.x
                        || y == start.y
                        || y == end.y
                        || z == start.z
                        || z == end.z;
                    if shell {
                        world.set(IVec3::new(x, y, z), mat);
                    }
                }
            }
        }
        let center = cfg.voxel_to_world_center(start + IVec3::splat(size / 2));
        (world, center)
    }

    #[test]
    fn large_stone_cavern_has_longer_reverb_than_small_wooden_shed_or_foam_room() {
        let (small_wood, pos_wood) = build_box_room(6, VoxelId::Timber);
        let (large_stone, pos_stone) = build_box_room(26, VoxelId::CaveStone);
        let (large_foam, pos_foam) = build_box_room(26, VoxelId::AcousticFoam);

        let cfg = PathTraceConfig {
            reverb_rays: 96,
            ..Default::default()
        };

        let rev_wood = trace_listener_reverb(&small_wood, pos_wood, cfg, 1);
        let rev_stone = trace_listener_reverb(&large_stone, pos_stone, cfg, 1);
        let rev_foam = trace_listener_reverb(&large_foam, pos_foam, cfg, 1);

        assert!(
            rev_stone.rt60_sec.0[1] > rev_wood.rt60_sec.0[1] * 2.5,
            "stone cavern RT60 ({:?}) should be much longer than wooden shed ({:?})",
            rev_stone.rt60_sec,
            rev_wood.rt60_sec
        );
        assert!(
            rev_stone.rt60_sec.0[1] > rev_foam.rt60_sec.0[1] * 2.5,
            "stone cavern RT60 ({:?}) should be much longer than acoustic foam room ({:?})",
            rev_stone.rt60_sec,
            rev_foam.rt60_sec
        );
    }
}
