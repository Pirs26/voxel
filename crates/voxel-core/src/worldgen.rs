use glam::{IVec3, Vec3};
use noise::{NoiseFn, Simplex};

use crate::{
    chunk::{CHUNK_SIZE, Chunk},
    config::VoxelConfig,
    material::VoxelId,
    world::VoxelWorld,
};

/// Important landmarks produced by deterministic world generation.
#[derive(Clone, Debug)]
pub struct GeneratedLevelInfo {
    /// Player spawn point in world meters (inside the starter shed / cave entry).
    pub spawn_pos_m: Vec3,
    /// Position of the elevator platform floor in world meters.
    pub elevator_pos_m: Vec3,
    /// Acoustic demo beacon position (behind a breachable cracked rock wall!).
    pub acoustic_beacon_pos_m: Vec3,
    /// Center of the grand echoing cavern in world meters.
    pub grand_cavern_pos_m: Vec3,
}

/// Deterministically builds a playable vertical testbed level:
/// - Upper wooden **Starter Shed** (warm, short reverb, acoustic test partition)
/// - Steel **Vertical Mineshaft** with stepped spiral walkway down into darkness
/// - Lower **Echoing Cave Network** (grand reverberant cavern, narrow winding tunnels,
///   a **Cracked Rock breachable wall** hiding a resonant acoustic beacon chamber,
///   and glowing bioluminescent crystal landmarks).
pub fn generate_world(seed: u64, config: VoxelConfig) -> (VoxelWorld, GeneratedLevelInfo) {
    let mut world = VoxelWorld::new(config);
    let max = config.world_voxels();
    let simplex = Simplex::new(seed as u32);

    // Coordinate landmarks in voxel space (scaled proportionally to world_voxels)
    let cx = max.x / 2;
    let cz = max.z / 2;
    let shed_floor_y = (max.y * 3) / 4; // e.g., y = 96 in a 128-tall world
    let cave_floor_y = max.y / 4;       // e.g., y = 32 in a 128-tall world

    // Helper: carve or fill chunk-by-chunk
    for cy in 0..config.world_chunks.y as i32 {
        for cz_i in 0..config.world_chunks.z as i32 {
            for cx_i in 0..config.world_chunks.x as i32 {
                let mut chunk = Chunk::new_uniform(VoxelId::Air);
                let base = IVec3::new(cx_i, cy, cz_i) * CHUNK_SIZE as i32;

                for ly in 0..CHUNK_SIZE {
                    let y = base.y + ly as i32;
                    for lz in 0..CHUNK_SIZE {
                        let z = base.z + lz as i32;
                        for lx in 0..CHUNK_SIZE {
                            let x = base.x + lx as i32;

                            let mat = sample_base_voxel(
                                IVec3::new(x, y, z),
                                max,
                                cx,
                                cz,
                                shed_floor_y,
                                cave_floor_y,
                                &simplex,
                            );
                            if mat != VoxelId::Air {
                                chunk.set(lx, ly, lz, mat);
                            }
                        }
                    }
                }
                world.insert_chunk(IVec3::new(cx_i, cy, cz_i), chunk);
            }
        }
    }

    // Carve connected walkable cave arteries + grand cavern + acoustic test alcove
    carve_sphere(&mut world, IVec3::new(cx, cave_floor_y + 7, cz), 8.0);

    // Grand Echoing Cavern to the +X side (huge vaulted stone ceiling for dramatic reverb!)
    let cavern_center = IVec3::new(cx + 26, cave_floor_y + 10, cz);
    carve_ellipsoid(&mut world, cavern_center, Vec3::new(17.0, 12.0, 15.0));
    // Tunnel from shaft base to grand cavern
    carve_tunnel(
        &mut world,
        IVec3::new(cx, cave_floor_y + 3, cz),
        IVec3::new(cx + 14, cave_floor_y + 3, cz),
        3.2,
    );

    // Secondary winding tunnel to the -Z side leading to a sealed Cracked Rock chamber
    let hallway_mid = IVec3::new(cx + 22, cave_floor_y + 3, cz - 14);
    let alcove_center = IVec3::new(cx + 22, cave_floor_y + 4, cz - 26);
    carve_tunnel(&mut world, IVec3::new(cx + 22, cave_floor_y + 3, cz - 6), hallway_mid, 2.8);
    carve_ellipsoid(&mut world, alcove_center, Vec3::new(7.5, 5.5, 7.0));
    // Connect hallway to alcove, then seal with a 2-voxel-thick CrackedRock wall!
    carve_tunnel(&mut world, hallway_mid, alcove_center, 2.5);
    for dy in 0..7 {
        for dx in -4..=4 {
            for dz in 0..2 {
                let p = IVec3::new(cx + 22 + dx, cave_floor_y + 1 + dy, cz - 19 - dz);
                if world.get(p) == VoxelId::Air {
                    world.set(p, VoxelId::CrackedRock);
                }
            }
        }
    }

    // Looping tunnel from alcove back toward shaft (-X side) for circular exploration
    carve_tunnel(
        &mut world,
        IVec3::new(cx, cave_floor_y + 3, cz),
        IVec3::new(cx - 18, cave_floor_y + 4, cz - 12),
        3.0,
    );
    carve_ellipsoid(
        &mut world,
        IVec3::new(cx - 20, cave_floor_y + 6, cz - 14),
        Vec3::new(9.0, 7.0, 9.0),
    );

    // Flat Dirt/Stone walkable floors in main cave hubs
    flatten_floor(&mut world, IVec3::new(cx, cave_floor_y, cz), 9, VoxelId::CaveDirt);
    flatten_floor(&mut world, IVec3::new(cx + 26, cave_floor_y, cz), 15, VoxelId::CaveDirt);
    flatten_floor(&mut world, IVec3::new(cx + 22, cave_floor_y, cz - 26), 6, VoxelId::CaveStone);
    flatten_floor(&mut world, IVec3::new(cx - 20, cave_floor_y, cz - 14), 8, VoxelId::CaveDirt);

    // Place a vertical steel shaft connecting Shed down to Cave + walkable spiral stairs
    // (until Week 3 elevator ride automates descent, you can walk down the shaft & hear the metallic echoes!)
    build_shaft_and_stairs(&mut world, cx, cz, cave_floor_y, shed_floor_y);

    // Build cozy Wooden Starter Shed on top with an acoustic demonstration wall
    build_starter_shed(&mut world, cx, cz, shed_floor_y);

    // Scatter glowing Bioluminescent Crystals for visual landmarks in pitch darkness
    let crystal_spots = [
        IVec3::new(cx + 12, cave_floor_y + 1, cz + 2),
        IVec3::new(cx + 26, cave_floor_y + 1, cz + 10),
        IVec3::new(cx + 35, cave_floor_y + 1, cz - 4),
        IVec3::new(cx + 22, cave_floor_y + 1, cz - 26), // inside sealed alcove
        IVec3::new(cx - 18, cave_floor_y + 1, cz - 12),
        IVec3::new(cx + 3, cave_floor_y + 1, cz - 3),
    ];
    for spot in crystal_spots {
        world.set(spot, VoxelId::BiolumCrystal);
        world.set(spot + IVec3::Y, VoxelId::BiolumCrystal);
    }

    world.mark_all_dirty();

    // Start player right in the Cave Hub near the shaft so Week-1 acoustic testing is immediate,
    // with stairs leading right up to the Wooden Shed!
    let info = GeneratedLevelInfo {
        spawn_pos_m: config.voxel_to_world_center(IVec3::new(cx + 6, cave_floor_y + 2, cz)),
        elevator_pos_m: config.voxel_to_world_center(IVec3::new(cx, shed_floor_y + 1, cz)),
        acoustic_beacon_pos_m: config.voxel_to_world_center(IVec3::new(cx + 22, cave_floor_y + 3, cz - 25)),
        grand_cavern_pos_m: config.voxel_to_world_center(cavern_center),
    };

    (world, info)
}

fn sample_base_voxel(
    p: IVec3,
    max: IVec3,
    _cx: i32,
    _cz: i32,
    shed_floor_y: i32,
    _cave_floor_y: i32,
    simplex: &Simplex,
) -> VoxelId {
    // Solid bedrock boundary shell
    if p.x <= 1 || p.z <= 1 || p.x >= max.x - 2 || p.z >= max.z - 2 || p.y <= 1 {
        return VoxelId::Bedrock;
    }

    // Open sky above surface shed
    if p.y > shed_floor_y {
        return VoxelId::Air;
    }

    // Surface layer right below shed
    if p.y == shed_floor_y {
        return VoxelId::CaveDirt;
    }

    // Underground: 3D cave noise creates natural side pockets while keeping strong stone walls
    let n = simplex.get([
        p.x as f64 * 0.055,
        p.y as f64 * 0.075,
        p.z as f64 * 0.055,
    ]);
    let n2 = simplex.get([
        p.x as f64 * 0.14 + 31.7,
        p.y as f64 * 0.14,
        p.z as f64 * 0.14 - 19.3,
    ]);

    if p.y < shed_floor_y - 8 && p.y > 8 && n > 0.56 {
        return VoxelId::Air;
    }

    if n2 > 0.62 && p.y < shed_floor_y - 6 {
        VoxelId::CrackedRock
    } else {
        VoxelId::CaveStone
    }
}

fn carve_sphere(world: &mut VoxelWorld, center: IVec3, radius: f32) {
    let r = radius.ceil() as i32;
    let r2 = radius * radius;
    for dy in -r..=r {
        for dz in -r..=r {
            for dx in -r..=r {
                if (dx * dx + dy * dy + dz * dz) as f32 <= r2 {
                    let p = center + IVec3::new(dx, dy, dz);
                    if p.y > 2 {
                        world.set(p, VoxelId::Air);
                    }
                }
            }
        }
    }
}

fn carve_ellipsoid(world: &mut VoxelWorld, center: IVec3, radii: Vec3) {
    let rx = radii.x.ceil() as i32;
    let ry = radii.y.ceil() as i32;
    let rz = radii.z.ceil() as i32;
    for dy in -ry..=ry {
        for dz in -rz..=rz {
            for dx in -rx..=rx {
                let nx = dx as f32 / radii.x;
                let ny = dy as f32 / radii.y;
                let nz = dz as f32 / radii.z;
                if nx * nx + ny * ny + nz * nz <= 1.0 {
                    let p = center + IVec3::new(dx, dy, dz);
                    if p.y > 2 {
                        world.set(p, VoxelId::Air);
                    }
                }
            }
        }
    }
}

fn carve_tunnel(world: &mut VoxelWorld, a: IVec3, b: IVec3, radius: f32) {
    let dist = a.as_vec3().distance(b.as_vec3());
    let steps = (dist * 1.5).ceil().max(1.0) as i32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let p = a.as_vec3().lerp(b.as_vec3(), t).round().as_ivec3();
        carve_sphere(world, p, radius);
    }
}

fn flatten_floor(world: &mut VoxelWorld, center: IVec3, radius: i32, mat: VoxelId) {
    let r2 = radius * radius;
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dz * dz <= r2 {
                let p = IVec3::new(center.x + dx, center.y, center.z + dz);
                world.set(p, mat);
                world.set(p - IVec3::Y, VoxelId::CaveStone);
                // Ensure headroom right above floor
                for h in 1..=3 {
                    world.set(p + IVec3::new(0, h, 0), VoxelId::Air);
                }
            }
        }
    }
}

fn build_shaft_and_stairs(
    world: &mut VoxelWorld,
    cx: i32,
    cz: i32,
    cave_floor_y: i32,
    shed_floor_y: i32,
) {
    let shaft_r = 4i32;
    for y in (cave_floor_y + 1)..=shed_floor_y {
        for dz in -shaft_r..=shaft_r {
            for dx in -shaft_r..=shaft_r {
                let p = IVec3::new(cx + dx, y, cz + dz);
                let on_wall = dx.abs() == shaft_r || dz.abs() == shaft_r;
                if on_wall {
                    // Leave cave entrance opening at bottom on +X side
                    if y <= cave_floor_y + 5 && dx == shaft_r && dz.abs() <= 2 {
                        world.set(p, VoxelId::Air);
                    } else {
                        world.set(p, VoxelId::Steel);
                    }
                } else {
                    world.set(p, VoxelId::Air);
                }
            }
        }
    }

    // Simple spiral steel ledges along shaft walls so player can climb between cave and shed in Week 1
    let mut angle_step = 0i32;
    for y in (cave_floor_y + 1)..shed_floor_y {
        let side = (angle_step / 4) % 4;
        let offset = (angle_step % 4) - 1;
        let (dx, dz) = match side {
            0 => (offset, -3),
            1 => (3, offset),
            2 => (-offset, 3),
            _ => (-3, -offset),
        };
        world.set(IVec3::new(cx + dx, y, cz + dz), VoxelId::Steel);
        angle_step += 1;
    }
}

fn build_starter_shed(world: &mut VoxelWorld, cx: i32, cz: i32, shed_floor_y: i32) {
    // Wooden floor & walls around shaft
    let min_x = cx - 9;
    let max_x = cx + 9;
    let min_z = cz - 8;
    let max_z = cz + 8;
    let roof_y = shed_floor_y + 7;

    for y in shed_floor_y..=roof_y {
        for z in min_z..=max_z {
            for x in min_x..=max_x {
                let p = IVec3::new(x, y, z);
                // Keep elevator shaft hole open on floor
                let in_shaft = (x - cx).abs() < 4 && (z - cz).abs() < 4;
                if y == shed_floor_y {
                    if !in_shaft {
                        world.set(p, VoxelId::Timber);
                    }
                } else if y == roof_y {
                    world.set(p, VoxelId::Timber);
                } else if x == min_x || x == max_x || z == min_z || z == max_z {
                    world.set(p, VoxelId::Timber);
                } else {
                    world.set(p, VoxelId::Air);
                }
            }
        }
    }

    // Add a bioluminescent shed worklight block
    world.set(IVec3::new(cx - 6, shed_floor_y + 5, cz), VoxelId::BiolumCrystal);
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::UVec3;

    #[test]
    fn worldgen_is_deterministic() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        let (w1, i1) = generate_world(1337, cfg);
        let (w2, i2) = generate_world(1337, cfg);
        assert_eq!(i1.spawn_pos_m, i2.spawn_pos_m);
        for x in 10..40 {
            for y in 10..40 {
                let p = IVec3::new(x, y, 25);
                assert_eq!(w1.get(p), w2.get(p));
            }
        }
    }
}
