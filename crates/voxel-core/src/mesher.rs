use glam::{IVec3, Vec3};

use crate::{
    chunk::CHUNK_SIZE,
    material::VoxelId,
    world::VoxelWorld,
};

/// Engine-agnostic mesh output for a single 32³ chunk (directly convertible to a Bevy Mesh).
#[derive(Clone, Debug, Default)]
pub struct ChunkMeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Linear RGBA vertex color (RGB baked with smooth per-vertex AO; A = 1.0).
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// World-space centers of emissive voxels in this chunk (for point-light seeding).
    pub emissive_centers_m: Vec<(Vec3, [f32; 3])>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CellFace {
    mat: VoxelId,
    ao: [u8; 4],
}

/// Computes voxel ambient occlusion (0..=3 neighbors) for a quad corner.
#[inline]
fn vertex_ao(side1: bool, side2: bool, corner: bool) -> u8 {
    if side1 && side2 {
        0
    } else {
        3 - (side1 as u8 + side2 as u8 + corner as u8)
    }
}

#[inline]
fn ao_multiplier(level: u8) -> f32 {
    match level {
        0 => 0.35,
        1 => 0.58,
        2 => 0.80,
        _ => 1.0,
    }
}

/// Greedy meshes a single chunk with per-vertex ambient occlusion and neighbor face culling.
pub fn mesh_chunk(world: &VoxelWorld, chunk_coord: IVec3) -> ChunkMeshData {
    let mut out = ChunkMeshData::default();
    let Some(chunk) = world.get_chunk(chunk_coord) else {
        return out;
    };
    if chunk.is_empty() {
        return out;
    }

    let cs = CHUNK_SIZE as i32;
    let base = chunk_coord * cs;
    let vs = world.voxel_size_m();

    // Collect emissive voxels for atmospheric cave lighting
    for y in 0..CHUNK_SIZE {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let mat = chunk.get(x, y, z);
                if mat.is_solid() {
                    let def = mat.def();
                    if def.emissive[0] > 0.0 || def.emissive[1] > 0.0 || def.emissive[2] > 0.0 {
                        let v = base + IVec3::new(x as i32, y as i32, z as i32);
                        out.emissive_centers_m
                            .push((world.config().voxel_to_world_center(v), def.emissive));
                    }
                }
            }
        }
    }

    // Greedy mesh across all 6 face directions: (axis, sign)
    // axis: 0 = X, 1 = Y, 2 = Z; sign: +1 or -1
    let mut mask: [Option<CellFace>; CHUNK_SIZE * CHUNK_SIZE] = [None; CHUNK_SIZE * CHUNK_SIZE];

    for axis in 0..3usize {
        let u_axis = (axis + 1) % 3;
        let v_axis = (axis + 2) % 3;

        let mut dir_vec = IVec3::ZERO;
        let mut u_vec = IVec3::ZERO;
        let mut v_vec = IVec3::ZERO;
        u_vec[u_axis] = 1;
        v_vec[v_axis] = 1;

        for sign in [-1i32, 1i32] {
            dir_vec[axis] = sign;
            let normal = dir_vec.as_vec3().to_array();

            for slice in 0..CHUNK_SIZE {
                // Build 2D mask for this slice
                mask.fill(None);

                for v in 0..CHUNK_SIZE {
                    for u in 0..CHUNK_SIZE {
                        let mut local = [0usize; 3];
                        local[axis] = slice;
                        local[u_axis] = u;
                        local[v_axis] = v;

                        let mat = chunk.get(local[0], local[1], local[2]);
                        if mat.is_air() {
                            continue;
                        }

                        let world_v = base + IVec3::new(local[0] as i32, local[1] as i32, local[2] as i32);
                        let neighbor = world_v + dir_vec;
                        if world.is_solid(neighbor) {
                            continue;
                        }

                        // Compute 4 corner AO values in neighbor plane
                        let sample = |du: i32, dv: i32| -> bool {
                            world.is_solid(neighbor + u_vec * du + v_vec * dv)
                        };

                        let ao = [
                            vertex_ao(sample(-1, 0), sample(0, -1), sample(-1, -1)), // (0,0)
                            vertex_ao(sample(1, 0), sample(0, -1), sample(1, -1)),   // (1,0)
                            vertex_ao(sample(1, 0), sample(0, 1), sample(1, 1)),     // (1,1)
                            vertex_ao(sample(-1, 0), sample(0, 1), sample(-1, 1)),   // (0,1)
                        ];

                        mask[u + v * CHUNK_SIZE] = Some(CellFace { mat, ao });
                    }
                }

                // Greedily merge identical (mat, ao) quads in the 2D mask
                let mut v = 0;
                while v < CHUNK_SIZE {
                    let mut u = 0;
                    while u < CHUNK_SIZE {
                        let Some(face) = mask[u + v * CHUNK_SIZE] else {
                            u += 1;
                            continue;
                        };

                        // Expand width along u
                        let mut w = 1;
                        while u + w < CHUNK_SIZE && mask[(u + w) + v * CHUNK_SIZE] == Some(face) {
                            w += 1;
                        }

                        // Expand height along v
                        let mut h = 1;
                        'height: while v + h < CHUNK_SIZE {
                            for du in 0..w {
                                if mask[(u + du) + (v + h) * CHUNK_SIZE] != Some(face) {
                                    break 'height;
                                }
                            }
                            h += 1;
                        }

                        // Clear merged cells
                        for dv in 0..h {
                            for du in 0..w {
                                mask[(u + du) + (v + dv) * CHUNK_SIZE] = None;
                            }
                        }

                        // Emit quad vertices in world meters
                        let plane_d = if sign > 0 { slice + 1 } else { slice } as i32;
                        let mut origin_vox = [0i32; 3];
                        origin_vox[axis] = base[axis] + plane_d;
                        origin_vox[u_axis] = base[u_axis] + u as i32;
                        origin_vox[v_axis] = base[v_axis] + v as i32;

                        let p0 = Vec3::new(origin_vox[0] as f32, origin_vox[1] as f32, origin_vox[2] as f32) * vs;
                        let du_m = u_vec.as_vec3() * (w as f32 * vs);
                        let dv_m = v_vec.as_vec3() * (h as f32 * vs);

                        let corners = [p0, p0 + du_m, p0 + du_m + dv_m, p0 + dv_m];
                        let def = face.mat.def();
                        let base_col = [
                            def.color[0] + def.emissive[0] * 1.4,
                            def.color[1] + def.emissive[1] * 1.4,
                            def.color[2] + def.emissive[2] * 1.4,
                        ];

                        let v_start = out.positions.len() as u32;
                        for i in 0..4 {
                            let ao_f = ao_multiplier(face.ao[i]);
                            out.positions.push(corners[i].to_array());
                            out.normals.push(normal);
                            out.colors.push([
                                (base_col[0] * ao_f).min(1.8),
                                (base_col[1] * ao_f).min(1.8),
                                (base_col[2] * ao_f).min(1.8),
                                1.0,
                            ]);
                        }

                        // Ensure outward CCW winding based on (u × v) · normal.
                        // Since u = (axis+1)%3 and v = (axis+2)%3, u × v == +axis.
                        // So sign > 0 -> CCW is (0, 1, 2, 0, 2, 3); sign < 0 -> reversed.
                        let flip_diag = (face.ao[0] + face.ao[2]) < (face.ao[1] + face.ao[3]);
                        let tris = match (sign > 0, flip_diag) {
                            (true, false) => [0, 1, 2, 0, 2, 3],
                            (true, true) => [1, 2, 3, 1, 3, 0],
                            (false, false) => [0, 2, 1, 0, 3, 2],
                            (false, true) => [1, 3, 2, 1, 0, 3],
                        };
                        for idx in tris {
                            out.indices.push(v_start + idx);
                        }

                        u += w;
                    }
                    v += 1;
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VoxelConfig;
    use glam::UVec3;

    #[test]
    fn greedy_mesher_merges_flat_plate() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(1));
        let mut world = VoxelWorld::new(cfg);
        for x in 2..10 {
            for z in 2..10 {
                world.set(IVec3::new(x, 5, z), VoxelId::CaveStone);
            }
        }
        let mesh = mesh_chunk(&world, IVec3::ZERO);
        // An 8x1x8 solid plate with uniform AO merges into 6 quads = 24 verts, 36 indices!
        assert_eq!(mesh.positions.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
    }
}
