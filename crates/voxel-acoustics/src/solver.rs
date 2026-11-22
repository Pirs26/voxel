use glam::Vec3;
use voxel_core::VoxelWorld;

use crate::{
    bands::Bands3,
    occlusion::{OcclusionResult, compute_direct_occlusion},
    pathtracer::{
        EarlyTap, MAX_EARLY_TAPS, PathTraceConfig, ReverbEstimate, trace_early_reflections,
        trace_listener_reverb,
    },
};

/// Input descriptor for an active world sound source (SFX or remote player voice).
#[derive(Clone, Copy, Debug)]
pub struct AcousticSourceQuery {
    pub id: u64,
    pub pos_m: Vec3,
    pub ref_distance_m: f32,
    pub max_distance_m: f32,
}

/// Solved acoustic parameters for one sound source, ready for the real-time DSP engine.
#[derive(Clone, Copy, Debug)]
pub struct SourceAcousticParams {
    pub id: u64,
    pub pos_m: Vec3,
    pub direct_gain: Bands3,
    pub solid_thickness_m: f32,
    pub distance_m: f32,
    pub early_taps: [EarlyTap; MAX_EARLY_TAPS],
    /// How much of this source feeds the shared cavern FDN reverb bus.
    pub reverb_send: f32,
}

/// Complete acoustic solve output for the listener and all active sources.
#[derive(Clone, Debug)]
pub struct AcousticSnapshot {
    pub listener_pos_m: Vec3,
    pub listener_forward: Vec3,
    pub listener_right: Vec3,
    pub reverb: ReverbEstimate,
    pub sources: Vec<SourceAcousticParams>,
}

/// Solves direct occlusion, early reflections, and reverb send for a single sound source
/// while reusing a previously computed listener [`ReverbEstimate`] (avoiding redundant
/// multi-bounce listener reverb raycasting on one-shot SFX).
pub fn solve_single_source(
    world: &VoxelWorld,
    listener_pos_m: Vec3,
    source: AcousticSourceQuery,
    reverb: ReverbEstimate,
    early_reflection_rays: usize,
    frame_seed: u64,
) -> SourceAcousticParams {
    let occ: OcclusionResult = compute_direct_occlusion(
        world,
        source.pos_m,
        listener_pos_m,
        source.ref_distance_m,
        source.max_distance_m,
    );
    let early_taps = if early_reflection_rays == 0 {
        [EarlyTap::default(); MAX_EARLY_TAPS]
    } else {
        trace_early_reflections(
            world,
            source.pos_m,
            listener_pos_m,
            PathTraceConfig {
                early_reflection_rays,
                ..Default::default()
            },
            frame_seed,
        )
    };

    // Reverb send increases with distance & room enclosure, damped if heavily walled off
    // unless strong early reflections wrap through a breached doorway or tunnel corner.
    let dist_factor = (occ.distance_m / 12.0).clamp(0.15, 1.0);
    let direct_openness = occ.direct_gain.0[0] * 0.7 + occ.direct_gain.0[1] * 0.3;
    let bounce_openness = early_taps
        .iter()
        .filter(|t| t.active)
        .map(|t| t.gain.mean())
        .fold(0.0f32, f32::max);
    let wall_damp = direct_openness.max(bounce_openness * 0.85).clamp(0.05, 1.0);
    let reverb_send =
        (reverb.wet_mix * (0.35 + 0.65 * dist_factor) * wall_damp.sqrt()).clamp(0.0, 0.85);

    SourceAcousticParams {
        id: source.id,
        pos_m: source.pos_m,
        direct_gain: occ.direct_gain,
        solid_thickness_m: occ.solid_thickness_m,
        distance_m: occ.distance_m,
        early_taps,
        reverb_send,
    }
}

/// Runs a full voxel-density occlusion + path-traced reflection acoustic solve.
pub fn solve_acoustics(
    world: &VoxelWorld,
    listener_pos_m: Vec3,
    listener_forward: Vec3,
    listener_right: Vec3,
    sources: &[AcousticSourceQuery],
    cfg: PathTraceConfig,
    frame_seed: u64,
) -> AcousticSnapshot {
    let reverb = trace_listener_reverb(world, listener_pos_m, cfg, frame_seed);

    let mut solved = Vec::with_capacity(sources.len());
    for (idx, src) in sources.iter().enumerate() {
        solved.push(solve_single_source(
            world,
            listener_pos_m,
            *src,
            reverb,
            cfg.early_reflection_rays,
            frame_seed.wrapping_add(idx as u64 * 31 + 7),
        ));
    }

    AcousticSnapshot {
        listener_pos_m,
        listener_forward,
        listener_right,
        reverb,
        sources: solved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{IVec3, UVec3};
    use voxel_core::{VoxelConfig, VoxelId};

    fn build_cavern_world() -> (VoxelWorld, VoxelConfig) {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(2));
        let mut world = VoxelWorld::new(cfg);
        let start = IVec3::splat(2);
        let end = IVec3::splat(22);
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
                        world.set(IVec3::new(x, y, z), VoxelId::CaveStone);
                    }
                }
            }
        }
        // Partial divider wall to create both occlusion and reflections
        for y in 3..14 {
            for z in 6..16 {
                world.set(IVec3::new(12, y, z), VoxelId::CaveStone);
            }
        }
        (world, cfg)
    }

    #[test]
    fn solve_single_source_matches_solve_acoustics_source_results() {
        let (world, vcfg) = build_cavern_world();
        let listener_pos = vcfg.voxel_to_world_center(IVec3::new(8, 8, 10));
        let sources = [
            AcousticSourceQuery {
                id: 101,
                pos_m: vcfg.voxel_to_world_center(IVec3::new(16, 8, 10)),
                ref_distance_m: 2.0,
                max_distance_m: 60.0,
            },
            AcousticSourceQuery {
                id: 202,
                pos_m: vcfg.voxel_to_world_center(IVec3::new(10, 8, 18)),
                ref_distance_m: 1.5,
                max_distance_m: 50.0,
            },
        ];

        let cfg = PathTraceConfig {
            reverb_rays: 32,
            early_reflection_rays: 20,
            ..Default::default()
        };
        let frame_seed = 4242;

        let snapshot = solve_acoustics(
            &world,
            listener_pos,
            Vec3::NEG_Z,
            Vec3::X,
            &sources,
            cfg,
            frame_seed,
        );

        assert_eq!(snapshot.sources.len(), sources.len());

        for (idx, (query, batch_params)) in sources.iter().zip(&snapshot.sources).enumerate() {
            let single_params = solve_single_source(
                &world,
                listener_pos,
                *query,
                snapshot.reverb,
                cfg.early_reflection_rays,
                frame_seed.wrapping_add(idx as u64 * 31 + 7),
            );

            assert_eq!(single_params.id, batch_params.id);
            assert_eq!(single_params.pos_m, batch_params.pos_m);
            assert_eq!(single_params.direct_gain, batch_params.direct_gain);
            assert_eq!(
                single_params.solid_thickness_m,
                batch_params.solid_thickness_m
            );
            assert_eq!(single_params.distance_m, batch_params.distance_m);
            assert_eq!(single_params.reverb_send, batch_params.reverb_send);

            for (a, b) in single_params
                .early_taps
                .iter()
                .zip(batch_params.early_taps.iter())
            {
                assert_eq!(a.active, b.active);
                assert_eq!(a.delay_sec, b.delay_sec);
                assert_eq!(a.gain, b.gain);
                assert_eq!(a.direction, b.direction);
            }
        }
    }

    #[test]
    fn solve_single_source_with_zero_early_rays_skips_taps() {
        let (world, vcfg) = build_cavern_world();
        let listener_pos = vcfg.voxel_to_world_center(IVec3::new(8, 8, 10));
        let query = AcousticSourceQuery {
            id: 77,
            pos_m: vcfg.voxel_to_world_center(IVec3::new(10, 8, 14)),
            ref_distance_m: 1.5,
            max_distance_m: 50.0,
        };

        let params = solve_single_source(
            &world,
            listener_pos,
            query,
            ReverbEstimate::default(),
            0,
            123,
        );

        assert_eq!(params.id, 77);
        assert!(params.direct_gain.mean() > 0.0);
        assert!(params.reverb_send > 0.0);
        assert!(params.early_taps.iter().all(|t| !t.active));
    }
}
