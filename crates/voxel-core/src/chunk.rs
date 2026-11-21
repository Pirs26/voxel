use crate::material::VoxelId;

pub const CHUNK_SIZE: usize = 32;
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;
pub const BRICK_SIZE: usize = 8;
pub const BRICKS_PER_AXIS: usize = CHUNK_SIZE / BRICK_SIZE; // 4

#[derive(Clone, Debug)]
enum ChunkStorage {
    Uniform(VoxelId),
    Dense(Box<[VoxelId; CHUNK_VOLUME]>),
}

/// A 32×32×32 voxel chunk with uniform compression, a 4×4×4 (`u64`) brick occupancy mask
/// for fast empty-space skipping during DDA raytracing and meshing, and incremental tracking
/// of emissive voxels so meshing avoids full-chunk scans.
#[derive(Clone, Debug)]
pub struct Chunk {
    storage: ChunkStorage,
    /// Bit `bx + bz*4 + by*16` is 1 if the 8×8×8 brick contains at least one solid voxel.
    brick_mask: u64,
    /// Exact solid voxel count per 8×8×8 brick for O(1) incremental mask updates.
    brick_solid_counts: [u16; 64],
    /// Local coordinates `[x, y, z]` of all emissive voxels in this chunk.
    /// Remains empty (zero heap allocation) for non-emissive chunks.
    emissive_voxels: Vec<[u8; 3]>,
}

impl Default for Chunk {
    fn default() -> Self {
        Self::new_uniform(VoxelId::Air)
    }
}

impl Chunk {
    pub fn new_uniform(mat: VoxelId) -> Self {
        let (mask, counts) = if mat.is_solid() {
            (u64::MAX, [512u16; 64])
        } else {
            (0u64, [0u16; 64])
        };
        let emissive_voxels = if mat.is_emissive() {
            let mut list = Vec::with_capacity(CHUNK_VOLUME);
            for y in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    for x in 0..CHUNK_SIZE {
                        list.push([x as u8, y as u8, z as u8]);
                    }
                }
            }
            list
        } else {
            Vec::new()
        };
        Self {
            storage: ChunkStorage::Uniform(mat),
            brick_mask: mask,
            brick_solid_counts: counts,
            emissive_voxels,
        }
    }

    #[inline]
    pub const fn index(x: usize, y: usize, z: usize) -> usize {
        debug_assert!(x < CHUNK_SIZE && y < CHUNK_SIZE && z < CHUNK_SIZE);
        x + (z * CHUNK_SIZE) + (y * CHUNK_SIZE * CHUNK_SIZE)
    }

    #[inline]
    pub const fn brick_index(x: usize, y: usize, z: usize) -> usize {
        (x >> 3) + ((z >> 3) << 2) + ((y >> 3) << 4)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.brick_mask == 0
    }

    #[inline]
    pub fn is_completely_solid(&self) -> bool {
        matches!(self.storage, ChunkStorage::Uniform(v) if v.is_solid())
    }

    #[inline]
    pub fn brick_is_occupied(&self, x: usize, y: usize, z: usize) -> bool {
        let bit = 1u64 << Self::brick_index(x, y, z);
        (self.brick_mask & bit) != 0
    }

    /// Local `[x, y, z]` coordinates of all emissive voxels currently in this chunk.
    #[inline]
    pub fn emissive_voxels(&self) -> &[[u8; 3]] {
        &self.emissive_voxels
    }

    /// Number of emissive voxels currently in this chunk.
    #[inline]
    pub fn emissive_count(&self) -> usize {
        self.emissive_voxels.len()
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> VoxelId {
        match &self.storage {
            ChunkStorage::Uniform(v) => *v,
            ChunkStorage::Dense(data) => data[Self::index(x, y, z)],
        }
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, mat: VoxelId) -> bool {
        let old = self.get(x, y, z);
        if old == mat {
            return false;
        }

        if let ChunkStorage::Uniform(fill) = self.storage {
            let mut data = Box::new([fill; CHUNK_VOLUME]);
            data[Self::index(x, y, z)] = mat;
            self.storage = ChunkStorage::Dense(data);
        } else if let ChunkStorage::Dense(ref mut data) = self.storage {
            data[Self::index(x, y, z)] = mat;
        }

        let bidx = Self::brick_index(x, y, z);
        match (old.is_solid(), mat.is_solid()) {
            (false, true) => {
                self.brick_solid_counts[bidx] += 1;
                self.brick_mask |= 1u64 << bidx;
            }
            (true, false) => {
                self.brick_solid_counts[bidx] -= 1;
                if self.brick_solid_counts[bidx] == 0 {
                    self.brick_mask &= !(1u64 << bidx);
                }
            }
            _ => {}
        }

        let old_emissive = old.is_emissive();
        let new_emissive = mat.is_emissive();
        if old_emissive != new_emissive {
            let coord = [x as u8, y as u8, z as u8];
            if new_emissive {
                self.emissive_voxels.push(coord);
            } else if let Some(pos) = self.emissive_voxels.iter().position(|&c| c == coord) {
                self.emissive_voxels.swap_remove(pos);
            }
        }

        true
    }

    /// Re-compresses a dense chunk back to `Uniform` if all voxels match.
    pub fn try_compact(&mut self) {
        if let ChunkStorage::Dense(ref data) = self.storage {
            let first = data[0];
            if data.iter().all(|&v| v == first) {
                self.storage = ChunkStorage::Uniform(first);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emissive_tracking_updates_incrementally() {
        let mut chunk = Chunk::new_uniform(VoxelId::Air);
        assert_eq!(chunk.emissive_count(), 0);
        assert!(chunk.emissive_voxels().is_empty());

        assert!(chunk.set(3, 4, 5, VoxelId::CaveStone));
        assert_eq!(chunk.emissive_count(), 0);

        assert!(chunk.set(3, 4, 5, VoxelId::BiolumCrystal));
        assert!(chunk.set(10, 20, 30, VoxelId::BiolumCrystal));
        assert_eq!(chunk.emissive_count(), 2);
        assert!(chunk.emissive_voxels().contains(&[3, 4, 5]));
        assert!(chunk.emissive_voxels().contains(&[10, 20, 30]));

        // Setting same emissive material is a no-op
        assert!(!chunk.set(3, 4, 5, VoxelId::BiolumCrystal));
        assert_eq!(chunk.emissive_count(), 2);

        // Replacing emissive with non-emissive removes it
        assert!(chunk.set(3, 4, 5, VoxelId::Air));
        assert_eq!(chunk.emissive_count(), 1);
        assert_eq!(chunk.emissive_voxels(), &[[10, 20, 30]]);

        // Removing last emissive leaves empty list
        assert!(chunk.set(10, 20, 30, VoxelId::CaveStone));
        assert_eq!(chunk.emissive_count(), 0);
    }
}
