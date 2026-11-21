use glam::Vec3;
use voxel_core::{VoxelWorld, march_segment_occlusion};

use crate::bands::{Bands3, db_to_linear};

/// Result of direct-line voxel density occlusion from source to listener.
#[derive(Clone, Copy, Debug)]
pub struct OcclusionResult {
    /// Total solid material thickness traversed in meters.
    pub solid_thickness_m: f32,
    /// Accumulated transmission loss in positive dB per frequency band.
    pub transmission_loss_db: Bands3,
    /// Linear direct gain including distance falloff + voxel density muffling.
    pub direct_gain: Bands3,
    /// Geometric distance in meters.
    pub distance_m: f32,
}

/// Computes physically grounded direct-path muffling by marching exact voxel
/// segment lengths in meters and summing each material's `transmission_loss_db_per_m`.
pub fn compute_direct_occlusion(
    world: &VoxelWorld,
    source_pos_m: Vec3,
    listener_pos_m: Vec3,
    ref_distance_m: f32,
    max_distance_m: f32,
) -> OcclusionResult {
    let distance_m = source_pos_m.distance(listener_pos_m);
    if distance_m >= max_distance_m {
        return OcclusionResult {
            solid_thickness_m: 0.0,
            transmission_loss_db: Bands3::splat(120.0),
            direct_gain: Bands3::ZERO,
            distance_m,
        };
    }

    let mut solid_thickness_m = 0.0f32;
    let mut loss_db = [0.0f32; 3];

    march_segment_occlusion(world, source_pos_m, listener_pos_m, |mat, seg_m| {
        solid_thickness_m += seg_m;
        let tl = mat.def().acoustic.transmission_loss_db_per_m;
        loss_db[0] += tl[0] * seg_m;
        loss_db[1] += tl[1] * seg_m;
        loss_db[2] += tl[2] * seg_m;
    });

    // Air absorption (high frequencies roll off gently over long cavern distances)
    loss_db[0] += distance_m * 0.01;
    loss_db[1] += distance_m * 0.03;
    loss_db[2] += distance_m * 0.09;

    // Smooth inverse-distance rolloff with reference distance clamp
    let ref_d = ref_distance_m.max(0.5);
    let dist_atten = ref_d / (ref_d + (distance_m - ref_d).max(0.0));
    // Smooth fade to zero near max_distance_m
    let edge_fade = ((max_distance_m - distance_m) / (max_distance_m * 0.15)).clamp(0.0, 1.0);

    let direct_gain = Bands3::new(
        db_to_linear(-loss_db[0]) * dist_atten * edge_fade,
        db_to_linear(-loss_db[1]) * dist_atten * edge_fade,
        db_to_linear(-loss_db[2]) * dist_atten * edge_fade,
    );

    OcclusionResult {
        solid_thickness_m,
        transmission_loss_db: Bands3(loss_db),
        direct_gain,
        distance_m,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{IVec3, UVec3};
    use voxel_core::{VoxelConfig, VoxelId};

    #[test]
    fn stone_wall_muffles_highs_far_more_than_lows_and_breaching_clears_it() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(2));
        let mut world = VoxelWorld::new(cfg);

        for y in 4..12 {
            for z in 4..12 {
                world.set(IVec3::new(10, y, z), VoxelId::CaveStone);
                world.set(IVec3::new(11, y, z), VoxelId::CaveStone);
            }
        }

        let src = cfg.voxel_to_world_center(IVec3::new(6, 8, 8));
        let dst = cfg.voxel_to_world_center(IVec3::new(15, 8, 8));

        let blocked = compute_direct_occlusion(&world, src, dst, 1.5, 60.0);
        // High frequency should be much more attenuated than low frequency (muffled!)
        assert!(
            blocked.direct_gain.0[0] > blocked.direct_gain.0[2] * 10.0,
            "expected muffled low-pass profile, got {:?}",
            blocked.direct_gain
        );

        // Breach a 1-voxel peephole right along the ray
        world.set(IVec3::new(10, 8, 8), VoxelId::Air);
        world.set(IVec3::new(11, 8, 8), VoxelId::Air);

        let open = compute_direct_occlusion(&world, src, dst, 1.5, 60.0);
        assert!(open.solid_thickness_m < 1e-4);
        assert!(open.direct_gain.0[2] > blocked.direct_gain.0[2] * 50.0);
    }
}
