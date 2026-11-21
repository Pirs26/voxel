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
        let occ: OcclusionResult = compute_direct_occlusion(
            world,
            src.pos_m,
            listener_pos_m,
            src.ref_distance_m,
            src.max_distance_m,
        );
        let early_taps = trace_early_reflections(
            world,
            src.pos_m,
            listener_pos_m,
            cfg,
            frame_seed.wrapping_add(idx as u64 * 31 + 7),
        );

        // Reverb send increases with distance & room enclosure, damped if heavily walled off
        let dist_factor = (occ.distance_m / 12.0).clamp(0.15, 1.0);
        let wall_damp = (occ.direct_gain.0[0] * 0.7 + occ.direct_gain.0[1] * 0.3).clamp(0.05, 1.0);
        let reverb_send = (reverb.wet_mix * (0.35 + 0.65 * dist_factor) * wall_damp.sqrt()).clamp(0.0, 0.85);

        solved.push(SourceAcousticParams {
            id: src.id,
            pos_m: src.pos_m,
            direct_gain: occ.direct_gain,
            solid_thickness_m: occ.solid_thickness_m,
            distance_m: occ.distance_m,
            early_taps,
            reverb_send,
        });
    }

    AcousticSnapshot {
        listener_pos_m,
        listener_forward,
        listener_right,
        reverb,
        sources: solved,
    }
}
