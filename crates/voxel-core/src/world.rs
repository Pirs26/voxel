use std::collections::{HashMap, HashSet};

use glam::{IVec3, Vec3};

use crate::{
    chunk::{CHUNK_SIZE, Chunk},
    config::VoxelConfig,
    material::VoxelId,
};

/// Bounded sparse 3D voxel world.
///
/// Unpopulated chunks behave as `VoxelId::Air`. Mutations mark the owning chunk (and
/// neighboring chunks if on a boundary, so face culling & AO stay seamless) as dirty.
#[derive(Clone, Debug)]
pub struct VoxelWorld {
    config: VoxelConfig,
    chunks: HashMap<IVec3, Chunk>,
    dirty_chunks: HashSet<IVec3>,
}

impl VoxelWorld {
    pub fn new(config: VoxelConfig) -> Self {
        Self {
            config,
            chunks: HashMap::new(),
            dirty_chunks: HashSet::new(),
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
    pub fn get_chunk(&self, chunk_coord: IVec3) -> Option<&Chunk> {
        self.chunks.get(&chunk_coord)
    }

    pub fn chunk_coords(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.chunks.keys().copied()
    }

    #[inline]
    pub fn get(&self, voxel: IVec3) -> VoxelId {
        if !self.config.in_bounds(voxel) {
            return VoxelId::Bedrock;
        }
        let (cc, lc) = VoxelConfig::split_voxel_coord(voxel);
        match self.chunks.get(&cc) {
            Some(chunk) => chunk.get(lc.x as usize, lc.y as usize, lc.z as usize),
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
        let (cc, lc) = VoxelConfig::split_voxel_coord(voxel);
        let chunk = self
            .chunks
            .entry(cc)
            .or_insert_with(|| Chunk::new_uniform(VoxelId::Air));

        if !chunk.set(lc.x as usize, lc.y as usize, lc.z as usize, mat) {
            return false;
        }

        self.dirty_chunks.insert(cc);
        let max_local = (CHUNK_SIZE - 1) as u32;
        if lc.x == 0 {
            self.dirty_chunks.insert(cc - IVec3::X);
        } else if lc.x == max_local {
            self.dirty_chunks.insert(cc + IVec3::X);
        }
        if lc.y == 0 {
            self.dirty_chunks.insert(cc - IVec3::Y);
        } else if lc.y == max_local {
            self.dirty_chunks.insert(cc + IVec3::Y);
        }
        if lc.z == 0 {
            self.dirty_chunks.insert(cc - IVec3::Z);
        } else if lc.z == max_local {
            self.dirty_chunks.insert(cc + IVec3::Z);
        }
        true
    }

    /// Bulk-fill a chunk during generation and mark it dirty.
    pub fn insert_chunk(&mut self, chunk_coord: IVec3, mut chunk: Chunk) {
        chunk.try_compact();
        if !chunk.is_empty() {
            self.chunks.insert(chunk_coord, chunk);
            self.dirty_chunks.insert(chunk_coord);
        }
    }

    /// Mark all existing chunks dirty (e.g., right after initial worldgen).
    pub fn mark_all_dirty(&mut self) {
        self.dirty_chunks.extend(self.chunks.keys().copied());
    }

    pub fn take_dirty_chunks(&mut self) -> Vec<IVec3> {
        self.dirty_chunks.drain().collect()
    }
}
