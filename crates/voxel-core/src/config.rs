use glam::{IVec3, UVec3, Vec3};
use serde::{Deserialize, Serialize};

use crate::chunk::CHUNK_SIZE;

/// Scale-agnostic voxel world configuration.
///
/// All gameplay physics, audio propagation, and player dimensions work in SI meters,
/// converting through `voxel_size_m`. Starting at `0.90` m gives 1/2 player height
/// (player ~1.80 m = 2 voxels tall) while keeping a switch to `0.45` m (1/4 player
/// height = 4 voxels tall) completely viable without rewriting simulation math.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct VoxelConfig {
    /// Physical edge length of a single voxel in meters.
    pub voxel_size_m: f32,
    /// Bounded world size measured in 32³ chunks.
    pub world_chunks: UVec3,
}

impl Default for VoxelConfig {
    fn default() -> Self {
        Self {
            voxel_size_m: 0.90,
            // 4 x 4 x 4 chunks = 128³ voxels (~115m cube at 0.9m) for fast Week-1 iteration;
            // cleanly scales to 8x8x8 (256³) or larger per depth tier.
            world_chunks: UVec3::new(4, 4, 4),
        }
    }
}

impl VoxelConfig {
    pub fn half_player_scale(world_chunks: UVec3) -> Self {
        Self {
            voxel_size_m: 0.90,
            world_chunks,
        }
    }

    pub fn quarter_player_scale(world_chunks: UVec3) -> Self {
        Self {
            voxel_size_m: 0.45,
            world_chunks,
        }
    }

    #[inline]
    pub fn world_voxels(&self) -> IVec3 {
        self.world_chunks.as_ivec3() * CHUNK_SIZE as i32
    }

    #[inline]
    pub fn world_size_m(&self) -> Vec3 {
        self.world_voxels().as_vec3() * self.voxel_size_m
    }

    #[inline]
    pub fn in_bounds(&self, voxel: IVec3) -> bool {
        let max = self.world_voxels();
        voxel.x >= 0
            && voxel.y >= 0
            && voxel.z >= 0
            && voxel.x < max.x
            && voxel.y < max.y
            && voxel.z < max.z
    }

    #[inline]
    pub fn world_to_voxel(&self, pos_m: Vec3) -> IVec3 {
        (pos_m / self.voxel_size_m).floor().as_ivec3()
    }

    #[inline]
    pub fn voxel_to_world_min(&self, voxel: IVec3) -> Vec3 {
        voxel.as_vec3() * self.voxel_size_m
    }

    #[inline]
    pub fn voxel_to_world_center(&self, voxel: IVec3) -> Vec3 {
        (voxel.as_vec3() + Vec3::splat(0.5)) * self.voxel_size_m
    }

    #[inline]
    pub fn split_voxel_coord(voxel: IVec3) -> (IVec3, UVec3) {
        let cs = CHUNK_SIZE as i32;
        let chunk = voxel.div_euclid(IVec3::splat(cs));
        let local = voxel.rem_euclid(IVec3::splat(cs)).as_uvec3();
        (chunk, local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_scale_invariance() {
        for cfg in [
            VoxelConfig::half_player_scale(UVec3::splat(4)),
            VoxelConfig::quarter_player_scale(UVec3::splat(8)),
        ] {
            for v in [IVec3::new(0, 0, 0), IVec3::new(17, 42, 63), IVec3::new(127, 99, 5)] {
                let center = cfg.voxel_to_world_center(v);
                assert_eq!(cfg.world_to_voxel(center), v);
            }
        }
    }
}
