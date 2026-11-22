use glam::{IVec3, Vec3};

use crate::{
    chunk::{CHUNK_SIZE, Chunk},
    config::VoxelConfig,
    material::VoxelId,
};

const _: () = assert!(CHUNK_SIZE == 32);

/// Bounded 3D voxel world backed by a flat contiguous `Vec<Option<Chunk>>` grid.
///
/// Unpopulated chunks (`None`) behave as `VoxelId::Air`. Mutations mark the owning chunk
/// (and neighboring chunks if on a boundary, so face culling & AO stay seamless) as dirty.
#[derive(Clone, Debug)]
pub struct VoxelWorld {
    config: VoxelConfig,
    wx: usize,
    wy: usize,
    wz: usize,
    chunks: Vec<Option<Chunk>>,
    dirty_flags: Vec<bool>,
    dirty_list: Vec<IVec3>,
}

impl VoxelWorld {
    pub fn new(config: VoxelConfig) -> Self {
        let wx = config.world_chunks.x as usize;
        let wy = config.world_chunks.y as usize;
        let wz = config.world_chunks.z as usize;
        let total = wx * wy * wz;
        Self {
            config,
            wx,
            wy,
            wz,
            chunks: vec![None; total],
            dirty_flags: vec![false; total],
            dirty_list: Vec::new(),
        }
    }

    #[inline]
    pub fn config(&self) -> VoxelConfig {
        self.config
    }

    #[inline]
    pub fn voxel_size_m(&self) -> f32 {
        self.config.voxel_size_m
    }

    #[inline]
    fn chunk_index(&self, cc: IVec3) -> Option<usize> {
        if cc.x < 0 || cc.y < 0 || cc.z < 0 {
            return None;
        }
        let (x, y, z) = (cc.x as usize, cc.y as usize, cc.z as usize);
        if x >= self.wx || y >= self.wy || z >= self.wz {
            return None;
        }
        Some(x + z * self.wx + y * self.wx * self.wz)
    }

    #[inline]
    fn mark_dirty(&mut self, cc: IVec3) {
        if let Some(idx) = self.chunk_index(cc)
            && !self.dirty_flags[idx]
        {
            self.dirty_flags[idx] = true;
            self.dirty_list.push(cc);
        }
    }

    #[inline]
    pub fn get_chunk(&self, chunk_coord: IVec3) -> Option<&Chunk> {
        let idx = self.chunk_index(chunk_coord)?;
        self.chunks[idx].as_ref()
    }

    pub fn chunk_coords(&self) -> impl Iterator<Item = IVec3> + '_ {
        let wx = self.wx;
        let wxz = self.wx * self.wz;
        self.chunks.iter().enumerate().filter_map(move |(idx, slot)| {
            slot.as_ref().map(|_| {
                let y = idx / wxz;
                let rem = idx - y * wxz;
                let z = rem / wx;
                let x = rem - z * wx;
                IVec3::new(x as i32, y as i32, z as i32)
            })
        })
    }

    #[inline]
    pub fn get(&self, voxel: IVec3) -> VoxelId {
        if !self.config.in_bounds(voxel) {
            return VoxelId::Bedrock;
        }
        let cx = (voxel.x as usize) >> 5;
        let cy = (voxel.y as usize) >> 5;
        let cz = (voxel.z as usize) >> 5;
        let idx = cx + cz * self.wx + cy * self.wx * self.wz;
        match &self.chunks[idx] {
            Some(chunk) => chunk.get(
                (voxel.x as usize) & 31,
                (voxel.y as usize) & 31,
                (voxel.z as usize) & 31,
            ),
            None => VoxelId::Air,
        }
    }

    #[inline]
    pub fn is_solid(&self, voxel: IVec3) -> bool {
        self.get(voxel).is_solid()
    }

    #[inline]
    pub fn is_solid_world_m(&self, pos_m: Vec3) -> bool {
        self.is_solid(self.config.world_to_voxel(pos_m))
    }

    /// Sets a voxel and marks affected chunks dirty for re-meshing.
    pub fn set(&mut self, voxel: IVec3, mat: VoxelId) -> bool {
        if !self.config.in_bounds(voxel) {
            return false;
        }
        let cx = (voxel.x as usize) >> 5;
        let cy = (voxel.y as usize) >> 5;
        let cz = (voxel.z as usize) >> 5;
        let lx = (voxel.x as usize) & 31;
        let ly = (voxel.y as usize) & 31;
        let lz = (voxel.z as usize) & 31;
        let idx = cx + cz * self.wx + cy * self.wx * self.wz;

        if self.chunks[idx].is_none() && mat.is_air() {
            return false;
        }

        let chunk = self.chunks[idx].get_or_insert_with(|| Chunk::new_uniform(VoxelId::Air));
        if !chunk.set(lx, ly, lz, mat) {
            return false;
        }

        let cc = IVec3::new(cx as i32, cy as i32, cz as i32);
        self.mark_dirty(cc);
        let max_local = CHUNK_SIZE - 1;
        if lx == 0 {
            self.mark_dirty(cc - IVec3::X);
        } else if lx == max_local {
            self.mark_dirty(cc + IVec3::X);
        }
        if ly == 0 {
            self.mark_dirty(cc - IVec3::Y);
        } else if ly == max_local {
            self.mark_dirty(cc + IVec3::Y);
        }
        if lz == 0 {
            self.mark_dirty(cc - IVec3::Z);
        } else if lz == max_local {
            self.mark_dirty(cc + IVec3::Z);
        }
        true
    }

    /// Bulk-fill a chunk during generation and mark it dirty.
    pub fn insert_chunk(&mut self, chunk_coord: IVec3, mut chunk: Chunk) {
        let Some(idx) = self.chunk_index(chunk_coord) else {
            return;
        };
        chunk.try_compact();
        if !chunk.is_empty() {
            self.chunks[idx] = Some(chunk);
            self.mark_dirty(chunk_coord);
        } else if self.chunks[idx].take().is_some() {
            self.mark_dirty(chunk_coord);
        }
    }

    /// Marks a specific chunk coordinate dirty for re-meshing.
    pub fn mark_chunk_dirty(&mut self, chunk_coord: IVec3) {
        self.mark_dirty(chunk_coord);
    }

    /// Mark all existing chunks dirty (e.g., right after initial worldgen).
    pub fn mark_all_dirty(&mut self) {
        let wx = self.wx;
        let wxz = self.wx * self.wz;
        for (idx, slot) in self.chunks.iter().enumerate() {
            if slot.is_some() && !self.dirty_flags[idx] {
                self.dirty_flags[idx] = true;
                let y = idx / wxz;
                let rem = idx - y * wxz;
                let z = rem / wx;
                let x = rem - z * wx;
                self.dirty_list.push(IVec3::new(x as i32, y as i32, z as i32));
            }
        }
    }

    pub fn take_dirty_chunks(&mut self) -> Vec<IVec3> {
        for &cc in &self.dirty_list {
            if let Some(idx) = self.chunk_index(cc) {
                self.dirty_flags[idx] = false;
            }
        }
        std::mem::take(&mut self.dirty_list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::UVec3;

    #[test]
    fn flat_grid_lookup_and_dirty_tracking() {
        let cfg = VoxelConfig::half_player_scale(UVec3::new(2, 3, 2));
        let mut world = VoxelWorld::new(cfg);

        // Out-of-bounds reads behave as Bedrock
        assert_eq!(world.get(IVec3::new(-1, 0, 0)), VoxelId::Bedrock);
        assert_eq!(world.get(IVec3::new(64, 0, 0)), VoxelId::Bedrock);
        assert!(world.get_chunk(IVec3::new(-1, 0, 0)).is_none());
        assert!(world.get_chunk(IVec3::new(2, 0, 0)).is_none());

        // Setting Air into empty chunk does not allocate or dirty
        assert!(!world.set(IVec3::new(5, 5, 5), VoxelId::Air));
        assert!(world.get_chunk(IVec3::ZERO).is_none());
        assert!(world.take_dirty_chunks().is_empty());

        // Setting boundary voxel dirties both owning and neighbor chunks without duplicates
        assert!(world.set(IVec3::new(31, 32, 0), VoxelId::CaveStone));
        assert!(world.set(IVec3::new(31, 32, 0), VoxelId::Timber));
        let dirty = world.take_dirty_chunks();
        assert_eq!(dirty.len(), 3);
        assert!(dirty.contains(&IVec3::new(0, 1, 0)));
        assert!(dirty.contains(&IVec3::new(1, 1, 0)));
        assert!(dirty.contains(&IVec3::new(0, 0, 0)));

        // Taking dirty chunks clears flags
        assert!(world.take_dirty_chunks().is_empty());

        // chunk_coords and mark_all_dirty match populated chunks
        let coords: Vec<IVec3> = world.chunk_coords().collect();
        assert_eq!(coords, vec![IVec3::new(0, 1, 0)]);
        world.mark_all_dirty();
        assert_eq!(world.take_dirty_chunks(), vec![IVec3::new(0, 1, 0)]);
    }
}
