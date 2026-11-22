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
    /// Player spawn point in world meters (inside the Base Shed on solid Timber floor).
    pub spawn_pos_m: Vec3,
    /// Position of the elevator platform floor at the top of the shaft in world meters.
    pub elevator_pos_m: Vec3,
    /// Acoustic demo beacon position (behind a breachable cracked rock wall!).
    pub acoustic_beacon_pos_m: Vec3,
    /// Center of the grand echoing cavern in world meters.
    pub grand_cavern_pos_m: Vec3,
    /// Active depth tier (1..=3).
    pub depth_tier: u8,
    /// Top surface Y of the Base Shed floor in meters (`(shed_floor_y + 1) * voxel_size_m`).
    pub shed_floor_y_m: f32,
    /// Top surface Y of the Cave landing floor in meters (`(cave_floor_y + 1) * voxel_size_m`).
    pub cave_floor_y_m: f32,
    /// Top elevator stop (shaft center XZ, Y = `shed_floor_y_m`).
    pub elevator_top_pos_m: Vec3,
    /// Bottom elevator stop (shaft center XZ, Y = `cave_floor_y_m`).
    pub elevator_bottom_pos_m: Vec3,
    /// Elevator lever console inside the Shed right next to the +X shaft boarding entrance.
    pub elevator_lever_shed_pos_m: Vec3,
    /// Elevator winch call station in the Cave landing right next to the +X shaft opening.
    pub elevator_winch_cave_pos_m: Vec3,
    /// Shop terminal station inside the Shed on a distinct wall/corner.
    pub shop_terminal_pos_m: Vec3,
    /// Battery charger bench inside the Shed on a distinct wall/corner.
    pub charger_bench_pos_m: Vec3,
    /// Wooden workbench counter top inside the Shed for starter spare items.
    pub shed_workbench_pos_m: Vec3,
    /// Interior AABB minimum corner of the Base Shed in world meters.
    pub shed_bounds_min_m: Vec3,
    /// Interior AABB maximum corner of the Base Shed in world meters.
    pub shed_bounds_max_m: Vec3,
    /// Guaranteed walkable spawn positions for heavy quest relics (2 for T1, 3 for T2, 4 for T3).
    pub relic_spawns_m: Vec<Vec3>,
    /// Guaranteed walkable spawn positions for small bonus loot items (2-4).
    pub bonus_loot_spawns_m: Vec<Vec3>,
    /// Guaranteed walkable spawn positions for blind acoustic predators (1 for T1, 2 for T2-T3).
    pub monster_spawns_m: Vec<Vec3>,
}

const SHED_HALF_MIN_X: i32 = 10;
const SHED_HALF_MAX_X: i32 = 11;
const SHED_HALF_Z: i32 = 9;
const SHED_ROOF_OFFSET: i32 = 7;
const SHAFT_RADIUS: i32 = 4;

/// Deterministically builds a playable vertical testbed level at depth tier 1.
pub fn generate_world(seed: u64, config: VoxelConfig) -> (VoxelWorld, GeneratedLevelInfo) {
    generate_world_with_tier(seed, 1, config)
}

/// Deterministically builds the persistent surface Base Shed (`y >= shed_floor_y`)
/// and the underground vertical mineshaft + echoing cave network (`y < shed_floor_y`)
/// for the requested `depth_tier` (clamped to `1..=3`).
pub fn generate_world_with_tier(
    seed: u64,
    depth_tier: u8,
    config: VoxelConfig,
) -> (VoxelWorld, GeneratedLevelInfo) {
    let mut world = VoxelWorld::new(config);
    let max = config.world_voxels();
    let (cx, cz, shed_floor_y, _cave_floor_y) = compute_layout_coords(max);

    // 1. Populate the surface layer (`y >= shed_floor_y`) across all chunks first.
    for cy in 0..config.world_chunks.y as i32 {
        let base_y = cy * CHUNK_SIZE as i32;
        if base_y + CHUNK_SIZE as i32 <= shed_floor_y {
            continue;
        }
        for cz_i in 0..config.world_chunks.z as i32 {
            for cx_i in 0..config.world_chunks.x as i32 {
                let mut chunk = Chunk::new_uniform(VoxelId::Air);
                let base = IVec3::new(cx_i, cy, cz_i) * CHUNK_SIZE as i32;

                for ly in 0..CHUNK_SIZE {
                    let y = base.y + ly as i32;
                    if y < shed_floor_y {
                        continue;
                    }
                    for lz in 0..CHUNK_SIZE {
                        let z = base.z + lz as i32;
                        for lx in 0..CHUNK_SIZE {
                            let x = base.x + lx as i32;
                            let mat = sample_surface_voxel(IVec3::new(x, y, z), max, shed_floor_y);
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

    // 2. Build the persistent wooden Base Shed on top of `shed_floor_y`.
    build_starter_shed(&mut world, cx, cz, shed_floor_y);

    // 3. Populate the underground cave and steel shaft strictly below `shed_floor_y`.
    let info = regenerate_underground(&mut world, seed, depth_tier);
    world.mark_all_dirty();

    (world, info)
}

/// Regenerates ONLY the underground region (`y < shed_floor_y`) and the vertical steel
/// elevator shaft below `shed_floor_y`, leaving EVERY voxel at `y >= shed_floor_y`
/// (the persistent Base Shed and any player edits/foam inside it) completely untouched.
///
/// Marks all affected chunks dirty for re-meshing.
pub fn regenerate_underground(
    world: &mut VoxelWorld,
    seed: u64,
    depth_tier: u8,
) -> GeneratedLevelInfo {
    let config = world.config();
    let max = config.world_voxels();
    let tier = depth_tier.clamp(1, 3);
    let (cx, cz, shed_floor_y, cave_floor_y) = compute_layout_coords(max);

    let cave_seed = mix_seed(seed, tier);
    let simplex = Simplex::new((cave_seed ^ (cave_seed >> 32)) as u32);

    // 1. Regenerate underground base rock strictly for `y < shed_floor_y`.
    for cy in 0..config.world_chunks.y as i32 {
        let base_y = cy * CHUNK_SIZE as i32;
        if base_y >= shed_floor_y {
            continue;
        }
        let spans_shed_floor = base_y + CHUNK_SIZE as i32 > shed_floor_y;

        for cz_i in 0..config.world_chunks.z as i32 {
            for cx_i in 0..config.world_chunks.x as i32 {
                let cc = IVec3::new(cx_i, cy, cz_i);
                let base = cc * CHUNK_SIZE as i32;

                let mut chunk = if spans_shed_floor {
                    // Preserve all voxels at `y >= shed_floor_y` verbatim!
                    world
                        .get_chunk(cc)
                        .cloned()
                        .unwrap_or_else(|| Chunk::new_uniform(VoxelId::Air))
                } else {
                    Chunk::new_uniform(VoxelId::Air)
                };

                let ly_limit = if spans_shed_floor {
                    (shed_floor_y - base_y) as usize
                } else {
                    CHUNK_SIZE
                };

                for ly in 0..ly_limit {
                    let y = base.y + ly as i32;
                    for lz in 0..CHUNK_SIZE {
                        let z = base.z + lz as i32;
                        for lx in 0..CHUNK_SIZE {
                            let x = base.x + lx as i32;
                            let mat = sample_underground_voxel(
                                IVec3::new(x, y, z),
                                max,
                                shed_floor_y,
                                tier,
                                &simplex,
                            );
                            if spans_shed_floor || mat != VoxelId::Air {
                                chunk.set(lx, ly, lz, mat);
                            }
                        }
                    }
                }

                world.insert_chunk(cc, chunk);

                // If this chunk sits directly beneath the shed floor boundary, dirty the
                // chunk immediately above so bottom-face culling & AO stay seamless.
                if base_y + CHUNK_SIZE as i32 == shed_floor_y {
                    world.mark_chunk_dirty(cc + IVec3::Y);
                }
            }
        }
    }

    // 2. Carve a claustrophobic tunnel-maze labyrinth around the central shaft with
    //    breakable `CrackedRock` walls sealing shortcuts, flanking loops, and relic vaults.
    let (cavern_center, alcove_center, relic_voxels, bonus_voxels, monster_voxels, crystal_spots) =
        carve_tunnel_maze(world, cave_seed, tier, cx, cz, cave_floor_y, shed_floor_y);

    // 3. Build unobstructed 7x7 vertical steel mineshaft strictly below `shed_floor_y`.
    build_shaft(world, cx, cz, cave_floor_y, shed_floor_y);

    // 4. Scatter glowing Bioluminescent Crystals at key tunnel intersections (sparser on deeper tiers).
    for spot in crystal_spots {
        if spot.y + 1 < shed_floor_y {
            world.set(spot, VoxelId::BiolumCrystal);
            world.set(spot + IVec3::Y, VoxelId::BiolumCrystal);
        }
    }

    // 5. Guarantee walkable spawn points for relics, bonus loot, and blind predators.
    let relic_spawns_m = relic_voxels
        .into_iter()
        .map(|v| ensure_walkable_spawn(world, v, VoxelId::CaveStone, shed_floor_y))
        .collect();

    let bonus_loot_spawns_m = bonus_voxels
        .into_iter()
        .map(|v| ensure_walkable_spawn(world, v, VoxelId::CaveDirt, shed_floor_y))
        .collect();

    let monster_spawns_m = monster_voxels
        .into_iter()
        .map(|v| ensure_walkable_spawn(world, v, VoxelId::CaveDirt, shed_floor_y))
        .collect();

    // Ensure vestibule right outside the +X shaft exit is clear and walkable
    let winch_voxel = IVec3::new(cx + 5, cave_floor_y + 1, cz + 1);
    ensure_walkable_spawn(world, winch_voxel, VoxelId::CaveDirt, shed_floor_y);

    build_level_info(
        config,
        tier,
        cx,
        cz,
        shed_floor_y,
        cave_floor_y,
        cavern_center,
        alcove_center,
        relic_spawns_m,
        bonus_loot_spawns_m,
        monster_spawns_m,
    )
}

#[allow(clippy::type_complexity)]
fn carve_tunnel_maze(
    world: &mut VoxelWorld,
    cave_seed: u64,
    tier: u8,
    cx: i32,
    cz: i32,
    cave_floor_y: i32,
    shed_floor_y: i32,
) -> (
    IVec3,
    IVec3,
    Vec<IVec3>,
    Vec<IVec3>,
    Vec<IVec3>,
    Vec<IVec3>,
) {
    // 5x5 maze junction grid (indices 0..5 correspond to grid offsets -2..=2 around the shaft).
    // Center cell (2, 2) is occupied by the 7x7 vertical steel mineshaft.
    const GRID: usize = 5;
    const STEP: i32 = 11;

    let mut nodes = [[IVec3::ZERO; GRID]; GRID];
    for (gz, row) in nodes.iter_mut().enumerate() {
        for (gx, slot) in row.iter_mut().enumerate() {
            let ox = gx as i32 - 2;
            let oz = gz as i32 - 2;
            let salt = (gx as u64 + 1) * 31 + (gz as u64 + 1) * 131;
            let is_corner = (gx == 0 || gx == 4) && (gz == 0 || gz == 4);
            let jitter_bound = if tier >= 3 && is_corner { 1 } else { 2 };
            let (jx, jz) = if (gx == 2 && gz == 2) || (gx == 3 && gz == 2) {
                // Keep shaft cell & immediate +X vestibule entrance aligned
                (0, 0)
            } else {
                (
                    deterministic_range(cave_seed, salt, -jitter_bound, jitter_bound),
                    deterministic_range(cave_seed, salt + 7, -jitter_bound, jitter_bound),
                )
            };
            let node_y = compute_node_floor_y(cave_seed, tier, gx, gz, cave_floor_y);
            *slot = IVec3::new(cx + ox * STEP + jx, node_y, cz + oz * STEP + jz);
        }
    }

    // 1. Build a deterministic randomized DFS spanning tree starting at entry node (3, 2)
    //    so EVERY maze cell is guaranteed reachable through winding tunnels!
    let mut visited = [[false; GRID]; GRID];
    visited[2][2] = true; // shaft cell is not a walkable maze node
    let mut tree_edges: Vec<((usize, usize), (usize, usize))> = Vec::new();
    let mut stack: Vec<(usize, usize)> = vec![(3, 2)];
    visited[2][3] = true;
    let mut step_salt = 500u64;

    while let Some((gx, gz)) = stack.last().copied() {
        let mut neighbors = Vec::with_capacity(4);
        if gx > 0 && !visited[gz][gx - 1] {
            neighbors.push((gx - 1, gz));
        }
        if gx + 1 < GRID && !visited[gz][gx + 1] {
            neighbors.push((gx + 1, gz));
        }
        if gz > 0 && !visited[gz - 1][gx] {
            neighbors.push((gx, gz - 1));
        }
        if gz + 1 < GRID && !visited[gz + 1][gx] {
            neighbors.push((gx, gz + 1));
        }

        if neighbors.is_empty() {
            stack.pop();
        } else {
            step_salt += 1;
            let pick = deterministic_range(cave_seed, step_salt, 0, neighbors.len() as i32 - 1)
                as usize;
            let next = neighbors[pick];
            visited[next.1][next.0] = true;
            tree_edges.push(((gx, gz), next));
            stack.push(next);
        }
    }

    // Helper to check if an undirected edge is already in the spanning tree
    let has_tree_edge = |a: (usize, usize), b: (usize, usize)| -> bool {
        tree_edges
            .iter()
            .any(|&(u, v)| (u == a && v == b) || (u == b && v == a))
    };

    // 2. Carve shaft landing vestibule & all spanning-tree main tunnels first
    //    (with smooth 1-voxel stair steps when connecting terraced elevations on Tiers 2 & 3).
    let vestibule = IVec3::new(cx + 6, cave_floor_y, cz);
    carve_junction_pocket(world, vestibule, 2, 4, VoxelId::CaveDirt, shed_floor_y);
    carve_maze_corridor(
        world,
        IVec3::new(cx + 4, cave_floor_y, cz),
        nodes[2][3],
        shed_floor_y,
    );

    // Seal the final entrance into the NE sealed relic vault (4, 0) with a CrackedRock wall
    // so players discover a breachable vault + acoustic demo beacon behind it.
    let alcove_cell = (4usize, 0usize);
    let mut barricade_spots: Vec<(IVec3, bool, i32, i32)> = Vec::new();

    for &(a, b) in &tree_edges {
        let pa = nodes[a.1][a.0];
        let pb = nodes[b.1][b.0];
        carve_maze_corridor(world, pa, pb, shed_floor_y);
        if a == alcove_cell || b == alcove_cell {
            let mid = (pa + pb) / 2;
            let x_major = (pb.x - pa.x).abs() >= (pb.z - pa.z).abs();
            barricade_spots.push((mid, x_major, pa.y.min(pb.y), pa.y.max(pb.y)));
        }
    }

    // 3. Add hidden shortcut/loop tunnels between non-tree adjacent cells, and block most of
    //    them with breakable `CrackedRock` barricades so smashing walls uncovers new tunnels!
    let loop_chance = match tier {
        1 => 60,
        2 => 72,
        _ => 80,
    };
    let barricade_chance = match tier {
        1 => 44,
        2 => 58,
        _ => 68,
    };
    let mut loop_salt = 900u64;
    for gz in 0..GRID {
        for gx in 0..GRID {
            if gx == 2 && gz == 2 {
                continue;
            }
            for (nx, nz) in [(gx + 1, gz), (gx, gz + 1)] {
                if nx >= GRID || nz >= GRID || (nx == 2 && nz == 2) {
                    continue;
                }
                if has_tree_edge((gx, gz), (nx, nz)) {
                    continue;
                }
                loop_salt += 1;
                let roll = deterministic_range(cave_seed, loop_salt, 0, 99);
                if roll < loop_chance {
                    let pa = nodes[gz][gx];
                    let pb = nodes[nz][nx];
                    carve_maze_corridor(world, pa, pb, shed_floor_y);
                    // Seal extra loop corridors with breakable `CrackedRock` walls
                    let touches_alcove = (gx, gz) == alcove_cell || (nx, nz) == alcove_cell;
                    if touches_alcove || roll < barricade_chance {
                        let mid = (pa + pb) / 2;
                        let x_major = (pb.x - pa.x).abs() >= (pb.z - pa.z).abs();
                        barricade_spots.push((mid, x_major, pa.y.min(pb.y), pa.y.max(pb.y)));
                    }
                }
            }
        }
    }

    // 4. Carve tunnel junctions and tier-specific vault chambers at every non-shaft grid cell
    for (gz, row) in nodes.iter().enumerate() {
        for (gx, &node) in row.iter().enumerate() {
            if gx == 2 && gz == 2 {
                continue;
            }
            let is_corner = (gx == 0 || gx == 4) && (gz == 0 || gz == 4);
            if tier == 3 && is_corner {
                // Tier 3: Recessed ancient ruined sanctum vault with symmetrical stone pillars,
                // overhead ruined archways, and a central stepped altar plinth.
                carve_ruined_sanctum_vault(world, node, shed_floor_y);
            } else {
                let radius = if is_corner { 3 } else { 2 };
                let height = if is_corner { 5 } else { 4 };
                let floor_mat = if (gx + gz) % 2 == 0 {
                    VoxelId::CaveStone
                } else {
                    VoxelId::CaveDirt
                };
                carve_junction_pocket(world, node, radius, height, floor_mat, shed_floor_y);
            }
        }
    }

    // Modest resonant echo chamber at east-mid node (4, 2) so acoustic cavern reverb still shines
    // without turning the level into an open arena.
    let east_node = nodes[2][4];
    let cavern_center = IVec3::new(east_node.x, east_node.y + 4, east_node.z);
    carve_ellipsoid(
        world,
        cavern_center,
        Vec3::new(5.8, 5.2 + tier as f32 * 0.5, 5.8),
        shed_floor_y,
    );
    flatten_floor(
        world,
        east_node,
        5,
        VoxelId::CaveDirt,
        shed_floor_y,
    );

    // 5. Stamp tier-specific structural landmarks at interior tunnel junctions:
    //    - Tier 1 (Damp Timber Mine Tunnels): frequent wooden mining support frames/arches.
    //    - Tier 2 (Stepped Catacombs): wooden mine bracing at mid-catacomb junctions.
    //    - Tier 3 (Modular Ancient Ruins): weathered stone ruined archways at interior junctions.
    for (gz, row) in nodes.iter().enumerate() {
        for (gx, &node) in row.iter().enumerate() {
            if (gx == 2 && gz == 2) || (gx == 4 && gz == 2) {
                continue;
            }
            let is_corner = (gx == 0 || gx == 4) && (gz == 0 || gz == 4);
            if is_corner {
                continue;
            }
            let x_aligned = (gx + gz) % 2 == 0;
            match tier {
                1 => {
                    if (gx + gz) % 2 == 1 || (gx == 3 && gz == 2) || gx == 2 {
                        build_timber_mine_frame(world, node, x_aligned, shed_floor_y);
                    }
                }
                2 => {
                    if gx == 2 || gz == 2 {
                        build_timber_mine_frame(world, node, x_aligned, shed_floor_y);
                    }
                }
                _ => {
                    if (gx + gz) % 2 == 1 || (gx == 3 && gz == 2) {
                        build_ruined_stone_archway(world, node, x_aligned, shed_floor_y);
                    }
                }
            }
        }
    }

    // 6. Stamp all breakable `CrackedRock` barricades cleanly across their carved tunnel sections
    for (mid, x_major, min_floor_y, max_floor_y) in barricade_spots {
        place_breakable_barricade(
            world,
            mid,
            x_major,
            min_floor_y.min(cave_floor_y),
            max_floor_y.max(cave_floor_y),
            shed_floor_y,
        );
    }

    // 7. Pick well-spaced landmarks across the tunnel maze (respecting each node's elevation
    //    and Tier 3's 1-voxel-high stepped altar plinths at the four outer corner sanctums).
    let alcove_node = nodes[alcove_cell.1][alcove_cell.0];
    let alcove_center = IVec3::new(alcove_node.x, alcove_node.y + 3, alcove_node.z);
    let corner_relic_lift = if tier == 3 { 2 } else { 1 };

    let mut relic_voxels = vec![
        // Relic 1: NE sealed CrackedRock vault
        IVec3::new(alcove_node.x, alcove_node.y + corner_relic_lift, alcove_node.z),
        // Relic 2: NW far vault
        IVec3::new(nodes[0][0].x, nodes[0][0].y + corner_relic_lift, nodes[0][0].z),
    ];
    if tier >= 2 {
        // Relic 3: Deep SE corner vault
        relic_voxels.push(IVec3::new(
            nodes[4][4].x,
            nodes[4][4].y + corner_relic_lift,
            nodes[4][4].z,
        ));
    }
    if tier >= 3 {
        // Relic 4: Deep SW corner vault
        relic_voxels.push(IVec3::new(
            nodes[4][0].x,
            nodes[4][0].y + corner_relic_lift,
            nodes[4][0].z,
        ));
    }

    let mut bonus_voxels = vec![
        IVec3::new(nodes[0][2].x, nodes[0][2].y + 1, nodes[0][2].z),
        IVec3::new(nodes[2][0].x, nodes[2][0].y + 1, nodes[2][0].z),
    ];
    if tier >= 2 {
        bonus_voxels.push(IVec3::new(nodes[3][4].x, nodes[3][4].y + 1, nodes[3][4].z));
    }
    if tier >= 3 {
        bonus_voxels.push(IVec3::new(nodes[4][2].x, nodes[4][2].y + 1, nodes[4][2].z));
    }

    // Monsters spawn in the far southern corner vaults (~28m from the shaft around multiple bends).
    // On Tiers 2-3, offset by 1 voxel onto the chamber floor so they stand beside the corner relic/altar.
    let m_off = if tier >= 2 { 1 } else { 0 };
    let mut monster_voxels = vec![IVec3::new(
        nodes[4][4].x - m_off,
        nodes[4][4].y + 1,
        nodes[4][4].z - m_off,
    )];
    if tier >= 2 {
        monster_voxels.push(IVec3::new(
            nodes[4][0].x + m_off,
            nodes[4][0].y + 1,
            nodes[4][0].z - m_off,
        ));
    }

    // Subtle bioluminescent crystals tucked into corners of select tunnel junctions
    // (progressively sparser on deeper tiers: 5 clusters on T1, 3 on T2, 2 eerie clusters on T3).
    let mut crystal_spots = vec![
        IVec3::new(nodes[0][4].x - 2, nodes[0][4].y + 1, nodes[0][4].z + 1),
    ];
    if tier == 1 {
        crystal_spots.push(IVec3::new(
            nodes[2][3].x + 2,
            nodes[2][3].y + 1,
            nodes[2][3].z - 1,
        ));
        crystal_spots.push(IVec3::new(
            nodes[0][0].x + 2,
            nodes[0][0].y + 1,
            nodes[0][0].z + 1,
        ));
        crystal_spots.push(IVec3::new(
            nodes[2][4].x + 2,
            nodes[2][4].y + 1,
            nodes[2][4].z + 2,
        ));
        crystal_spots.push(IVec3::new(
            nodes[2][0].x + 1,
            nodes[2][0].y + 1,
            nodes[2][0].z - 1,
        ));
    } else if tier == 2 {
        crystal_spots.push(IVec3::new(
            nodes[2][3].x + 2,
            nodes[2][3].y + 1,
            nodes[2][3].z - 1,
        ));
        crystal_spots.push(IVec3::new(
            nodes[0][0].x + 2,
            nodes[0][0].y + 1,
            nodes[0][0].z + 1,
        ));
    } else {
        crystal_spots.push(IVec3::new(
            nodes[4][1].x + 1,
            nodes[4][1].y + 1,
            nodes[4][1].z - 1,
        ));
    }

    (
        cavern_center,
        alcove_center,
        relic_voxels,
        bonus_voxels,
        monster_voxels,
        crystal_spots,
    )
}

/// Computes the deterministic floor elevation for a 5x5 maze junction cell `(gx, gz)`:
/// - Tier 1 stays flat at `cave_floor_y`.
/// - Tier 2 (Stepped Catacombs) varies across `cave_floor_y - 2..=cave_floor_y + 1`.
/// - Tier 3 (Modular Ancient Ruins) terraces across `cave_floor_y - 2..=cave_floor_y + 2` (`±2` voxels).
/// - The central shaft `(2, 2)`, entry junction `(3, 2)`, and east echo cavern `(4, 2)` always stay at `cave_floor_y`.
fn compute_node_floor_y(
    cave_seed: u64,
    tier: u8,
    gx: usize,
    gz: usize,
    cave_floor_y: i32,
) -> i32 {
    const TIER2_OFFSETS: [[i32; 5]; 5] = [
        [-2, -1, 0, 1, 1],
        [-1, -1, 0, 0, 1],
        [-1, 0, 0, 0, 0],
        [-1, -1, 0, -1, -1],
        [-2, -1, -1, -1, -2],
    ];
    const TIER3_OFFSETS: [[i32; 5]; 5] = [
        [-2, -1, 0, 1, 2],
        [-1, -1, 0, 1, 1],
        [-1, 0, 0, 0, 0],
        [-1, -1, 0, -1, -1],
        [-2, -2, -1, -1, -2],
    ];

    if tier <= 1 || (gz == 2 && gx >= 2) {
        return cave_floor_y;
    }
    let eff_gz = if (cave_seed & 1) == 1 { 4 - gz } else { gz };
    let offset = if tier == 2 {
        TIER2_OFFSETS[eff_gz][gx]
    } else {
        TIER3_OFFSETS[eff_gz][gx]
    };
    (cave_floor_y + offset).max(4)
}

/// Carves a compact subterranean tunnel junction chamber with flat walkable floor and arched roof.
fn carve_junction_pocket(
    world: &mut VoxelWorld,
    center: IVec3,
    radius: i32,
    height: i32,
    floor_mat: VoxelId,
    shed_floor_y: i32,
) {
    let r2 = radius * radius + 1;
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let d2 = dx * dx + dz * dz;
            if d2 > r2 {
                continue;
            }
            let floor_p = IVec3::new(center.x + dx, center.y, center.z + dz);
            if floor_p.y < shed_floor_y {
                world.set(floor_p, floor_mat);
                if floor_p.y > 1 {
                    world.set(floor_p - IVec3::Y, VoxelId::CaveStone);
                }
                if floor_p.y > 2 {
                    world.set(floor_p - IVec3::new(0, 2, 0), VoxelId::CaveStone);
                }
            }
            let local_h = if d2 <= 2 { height } else { (height - 1).max(3) };
            for dy in 1..=local_h {
                let p = floor_p + IVec3::new(0, dy, 0);
                if p.y > 2 && p.y < shed_floor_y {
                    world.set(p, VoxelId::Air);
                }
            }
        }
    }
}

/// Stamps a wooden mining support frame (`VoxelId::Timber` side corner posts + overhead collar beams)
/// at a tunnel junction without obstructing the 3-voxel-wide cardinal tunnel openings.
fn build_timber_mine_frame(
    world: &mut VoxelWorld,
    center: IVec3,
    x_aligned: bool,
    shed_floor_y: i32,
) {
    // 4 corner vertical timber support posts at `(dx = ±2, dz = ±2)`
    for dz in [-2, 2] {
        for dx in [-2, 2] {
            let base = IVec3::new(center.x + dx, center.y, center.z + dz);
            if base.y < shed_floor_y {
                world.set(base, VoxelId::CaveStone);
            }
            for dy in 1..=4 {
                let p = base + IVec3::new(0, dy, 0);
                if p.y > 2 && p.y < shed_floor_y {
                    world.set(p, VoxelId::Timber);
                }
            }
        }
    }

    // Overhead timber crossbeams at `center.y + 4` (leaving 3 full voxels of clear Air below)
    let beam_y = center.y + 4;
    if beam_y > 2 && beam_y < shed_floor_y {
        for span in -2..=2 {
            for side in [-1, 1] {
                let p = if x_aligned {
                    IVec3::new(center.x + span, beam_y, center.z + side)
                } else {
                    IVec3::new(center.x + side, beam_y, center.z + span)
                };
                world.set(p, VoxelId::Timber);
            }
        }
    }
}

/// Carves a Tier 3 recessed Modular Ancient Ruin sanctum vault (`7x7` dressed stone chamber)
/// featuring 4 symmetrical freestanding `CaveStone` columns, overhead ruined stone archways,
/// and a central 1-voxel-high stepped stone altar plinth.
fn carve_ruined_sanctum_vault(world: &mut VoxelWorld, center: IVec3, shed_floor_y: i32) {
    for dz in -3..=3 {
        for dx in -3..=3 {
            let floor_p = IVec3::new(center.x + dx, center.y, center.z + dz);
            if floor_p.y < shed_floor_y {
                world.set(floor_p, VoxelId::CaveStone);
                if floor_p.y > 1 {
                    world.set(floor_p - IVec3::Y, VoxelId::CaveStone);
                }
                if floor_p.y > 2 {
                    world.set(floor_p - IVec3::new(0, 2, 0), VoxelId::CaveStone);
                }
            }
            let max_h = if dx.abs() <= 2 && dz.abs() <= 2 { 6 } else { 5 };
            for dy in 1..=max_h {
                let p = floor_p + IVec3::new(0, dy, 0);
                if p.y > 2 && p.y < shed_floor_y {
                    world.set(p, VoxelId::Air);
                }
            }
        }
    }

    // 4 symmetrical freestanding interior stone columns at `(center.x ± 2, center.z ± 2)`
    for sz in [-2, 2] {
        for sx in [-2, 2] {
            for dy in 1..=5 {
                let p = IVec3::new(center.x + sx, center.y + dy, center.z + sz);
                if p.y > 2 && p.y < shed_floor_y {
                    world.set(p, VoxelId::CaveStone);
                }
            }
        }
    }

    // Overhead ruined stone archways (`dy = 5`) linking the 4 columns
    let arch_y = center.y + 5;
    if arch_y > 2 && arch_y < shed_floor_y {
        for d in -1..=1 {
            let mat = if d == 0 {
                VoxelId::CrackedRock
            } else {
                VoxelId::CaveStone
            };
            for side in [-2, 2] {
                world.set(IVec3::new(center.x + d, arch_y, center.z + side), mat);
                world.set(IVec3::new(center.x + side, arch_y, center.z + d), mat);
            }
        }
    }

    // Central 1-voxel-high stepped stone altar plinth at `(center.x, center.y + 1, center.z)`
    let plinth = IVec3::new(center.x, center.y + 1, center.z);
    if plinth.y > 2 && plinth.y < shed_floor_y {
        world.set(plinth, VoxelId::CaveStone);
    }
}

/// Stamps an ancient ruined stone archway (`CaveStone` corner pilasters + overhead stone lintel)
/// at an interior Tier 3 junction without narrowing the 3-voxel-wide walkways.
fn build_ruined_stone_archway(
    world: &mut VoxelWorld,
    center: IVec3,
    x_aligned: bool,
    shed_floor_y: i32,
) {
    for dz in [-2, 2] {
        for dx in [-2, 2] {
            let base = IVec3::new(center.x + dx, center.y, center.z + dz);
            if base.y < shed_floor_y {
                world.set(base, VoxelId::CaveStone);
            }
            for dy in 1..=4 {
                let p = base + IVec3::new(0, dy, 0);
                if p.y > 2 && p.y < shed_floor_y {
                    world.set(p, VoxelId::CaveStone);
                }
            }
        }
    }

    let arch_y = center.y + 4;
    if arch_y > 2 && arch_y < shed_floor_y {
        for span in -2..=2 {
            let mat = if span == 0 {
                VoxelId::CrackedRock
            } else {
                VoxelId::CaveStone
            };
            for side in [-1, 1] {
                let p = if x_aligned {
                    IVec3::new(center.x + span, arch_y, center.z + side)
                } else {
                    IVec3::new(center.x + side, arch_y, center.z + span)
                };
                world.set(p, VoxelId::CaveStone.def().id);
                if span == 0 {
                    world.set(p, mat);
                }
            }
        }
    }
}

/// Carves a narrow, tense winding subterranean mine tunnel (3 voxels wide × 3–4 voxels tall)
/// with guaranteed solid floor, subtle organic curve, and smooth 1-voxel stair transitions
/// when connecting junctions at different elevations (`a.y != b.y`).
fn carve_maze_corridor(
    world: &mut VoxelWorld,
    a: IVec3,
    b: IVec3,
    shed_floor_y: i32,
) {
    let a_xz = Vec3::new(a.x as f32, 0.0, a.z as f32);
    let b_xz = Vec3::new(b.x as f32, 0.0, b.z as f32);
    let dist = a_xz.distance(b_xz);
    let steps = (dist * 2.0).ceil().max(1.0) as i32;
    let delta = b - a;
    let perp = Vec3::new(-delta.z as f32, 0.0, delta.x as f32).normalize_or_zero();
    // Slight deterministic S-bend so tunnels feel hand-dug rather than laser-straight
    let bend_dir = if ((a.x.wrapping_mul(7) ^ b.z.wrapping_mul(13)) & 1) == 0 {
        1.0
    } else {
        -1.0
    };

    let mut centerline: Vec<(i32, i32, f32)> = Vec::with_capacity(steps as usize + 1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let curve = (t * std::f32::consts::PI).sin() * 1.15 * bend_dir;
        let p_f = a_xz.lerp(b_xz, t) + perp * curve;
        let cx_i = p_f.x.round() as i32;
        let cz_i = p_f.z.round() as i32;
        if centerline
            .last()
            .is_none_or(|&(lx, lz, _)| lx != cx_i || lz != cz_i)
        {
            centerline.push((cx_i, cz_i, t));
        }
    }

    // Assign floor height `y` along the centerline with max 1-voxel step delta
    // and flat landings inside both endpoint junction radii.
    let mut center_pts: Vec<IVec3> = Vec::with_capacity(centerline.len());
    let mut cur_y = a.y;
    let mut hold_count = 3i32;
    for &(cx_i, cz_i, t) in &centerline {
        let target_y = if a.y == b.y || t <= 0.30 {
            a.y
        } else if t >= 0.70 {
            b.y
        } else {
            let local_t = ((t - 0.30) / 0.40).clamp(0.0, 1.0);
            (a.y as f32 + (b.y - a.y) as f32 * local_t).round() as i32
        };

        if cur_y != target_y && hold_count >= 2 {
            cur_y += (target_y - cur_y).signum();
            hold_count = 0;
        } else {
            hold_count += 1;
        }
        center_pts.push(IVec3::new(cx_i, cur_y, cz_i));
    }
    if let Some(last) = center_pts.last_mut() {
        last.y = b.y;
    }

    let highest_y = a.y.max(b.y);
    for &c in &center_pts {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let wx = c.x + dx;
                let wz = c.z + dz;
                let floor_y = if a.y == b.y {
                    a.y
                } else {
                    center_pts
                        .iter()
                        .min_by_key(|pt| (wx - pt.x).pow(2) + (wz - pt.z).pow(2))
                        .map_or(c.y, |pt| pt.y)
                };

                let floor_p = IVec3::new(wx, floor_y, wz);
                if floor_p.y < shed_floor_y {
                    world.set(floor_p, VoxelId::CaveDirt);
                    if floor_p.y > 1 {
                        world.set(floor_p - IVec3::Y, VoxelId::CaveStone);
                    }
                    if floor_p.y > 2 {
                        world.set(floor_p - IVec3::new(0, 2, 0), VoxelId::CaveStone);
                    }
                }
                let is_center = dx == 0 && dz == 0;
                let max_h = if is_center { 4 } else { 3 };
                let top_air_y = (floor_y.max(highest_y) + max_h).min(shed_floor_y - 1);
                for air_y in (floor_y + 1)..=top_air_y {
                    if air_y > 2 {
                        world.set(IVec3::new(wx, air_y, wz), VoxelId::Air);
                    }
                }
            }
        }
    }
}

/// Seals a carved tunnel midpoint with a 2-voxel-thick breakable `CrackedRock` wall
/// so smashing it with `[LMB]` uncovers the hidden tunnel/shortcut beyond.
fn place_breakable_barricade(
    world: &mut VoxelWorld,
    mid: IVec3,
    x_major: bool,
    min_floor_y: i32,
    max_floor_y: i32,
    shed_floor_y: i32,
) {
    for y in (min_floor_y + 1)..=(max_floor_y + 5) {
        if y <= 2 || y >= shed_floor_y {
            continue;
        }
        for along in 0..=1 {
            for across in -3..=3 {
                let p = if x_major {
                    IVec3::new(mid.x + along, y, mid.z + across)
                } else {
                    IVec3::new(mid.x + across, y, mid.z + along)
                };
                if world.get(p) == VoxelId::Air {
                    world.set(p, VoxelId::CrackedRock);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_level_info(
    config: VoxelConfig,
    depth_tier: u8,
    cx: i32,
    cz: i32,
    shed_floor_y: i32,
    cave_floor_y: i32,
    cavern_center: IVec3,
    alcove_center: IVec3,
    relic_spawns_m: Vec<Vec3>,
    bonus_loot_spawns_m: Vec<Vec3>,
    monster_spawns_m: Vec<Vec3>,
) -> GeneratedLevelInfo {
    let vs = config.voxel_size_m;
    let shed_floor_y_m = (shed_floor_y + 1) as f32 * vs;
    let cave_floor_y_m = (cave_floor_y + 1) as f32 * vs;

    let shaft_center_x = (cx as f32 + 0.5) * vs;
    let shaft_center_z = (cz as f32 + 0.5) * vs;
    let elevator_top_pos_m = Vec3::new(shaft_center_x, shed_floor_y_m, shaft_center_z);
    let elevator_bottom_pos_m = Vec3::new(shaft_center_x, cave_floor_y_m, shaft_center_z);

    // Spawn inside Base Shed on solid Timber floor facing the shaft/workbench
    let spawn_pos_m = Vec3::new(
        ((cx + 7) as f32 + 0.5) * vs,
        shed_floor_y_m + 0.05,
        (cz as f32 + 0.5) * vs,
    );

    // Interactive Shed & Cave stations on distinct walls/corners
    let elevator_lever_shed_pos_m = Vec3::new(
        ((cx + 5) as f32 + 0.5) * vs,
        shed_floor_y_m,
        ((cz + 3) as f32 + 0.5) * vs,
    );
    let elevator_winch_cave_pos_m = Vec3::new(
        ((cx + 5) as f32 + 0.5) * vs,
        cave_floor_y_m,
        ((cz + 3) as f32 + 0.5) * vs,
    );
    let shop_terminal_pos_m = Vec3::new(
        ((cx + 9) as f32 + 0.5) * vs,
        shed_floor_y_m,
        ((cz - 5) as f32 + 0.5) * vs,
    );
    let charger_bench_pos_m = Vec3::new(
        ((cx + 4) as f32 + 0.5) * vs,
        shed_floor_y_m,
        ((cz - 7) as f32 + 0.5) * vs,
    );
    // Top surface of the 1-voxel-tall wooden workbench counter along the +Z wall
    let shed_workbench_pos_m = Vec3::new(
        ((cx + 6) as f32 + 0.5) * vs,
        (shed_floor_y + 2) as f32 * vs + 0.05,
        ((cz + 7) as f32 + 0.5) * vs,
    );

    let min_x = cx - SHED_HALF_MIN_X;
    let max_x = cx + SHED_HALF_MAX_X;
    let min_z = cz - SHED_HALF_Z;
    let max_z = cz + SHED_HALF_Z;
    let roof_y = shed_floor_y + SHED_ROOF_OFFSET;

    let shed_bounds_min_m =
        config.voxel_to_world_min(IVec3::new(min_x + 1, shed_floor_y + 1, min_z + 1));
    let shed_bounds_max_m = config.voxel_to_world_min(IVec3::new(max_x, roof_y, max_z));

    GeneratedLevelInfo {
        spawn_pos_m,
        elevator_pos_m: elevator_top_pos_m,
        acoustic_beacon_pos_m: config.voxel_to_world_center(IVec3::new(
            alcove_center.x,
            alcove_center.y,
            alcove_center.z + 1,
        )),
        grand_cavern_pos_m: config.voxel_to_world_center(cavern_center),
        depth_tier,
        shed_floor_y_m,
        cave_floor_y_m,
        elevator_top_pos_m,
        elevator_bottom_pos_m,
        elevator_lever_shed_pos_m,
        elevator_winch_cave_pos_m,
        shop_terminal_pos_m,
        charger_bench_pos_m,
        shed_workbench_pos_m,
        shed_bounds_min_m,
        shed_bounds_max_m,
        relic_spawns_m,
        bonus_loot_spawns_m,
        monster_spawns_m,
    }
}

#[inline]
fn compute_layout_coords(max: IVec3) -> (i32, i32, i32, i32) {
    let cx = max.x / 2;
    let cz = max.z / 2;
    let shed_floor_y = (max.y * 3) / 4; // e.g., y = 96 in a 128-tall world
    let cave_floor_y = max.y / 4; // e.g., y = 32 in a 128-tall world
    (cx, cz, shed_floor_y, cave_floor_y)
}

#[inline]
fn mix_seed(seed: u64, depth_tier: u8) -> u64 {
    let mut x = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((depth_tier as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9));
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

#[inline]
fn deterministic_range(seed: u64, salt: u64, min: i32, max: i32) -> i32 {
    let h = mix_seed(seed ^ salt.wrapping_mul(0xD6E8_FEB8_6659_FD93), salt as u8);
    let span = (max - min + 1).max(1) as u64;
    min + (h % span) as i32
}

fn sample_surface_voxel(p: IVec3, max: IVec3, shed_floor_y: i32) -> VoxelId {
    if p.x <= 1 || p.z <= 1 || p.x >= max.x - 2 || p.z >= max.z - 2 {
        return VoxelId::Bedrock;
    }
    if p.y > shed_floor_y {
        return VoxelId::Air;
    }
    VoxelId::CaveDirt
}

fn sample_underground_voxel(
    p: IVec3,
    max: IVec3,
    shed_floor_y: i32,
    depth_tier: u8,
    simplex: &Simplex,
) -> VoxelId {
    // Solid bedrock boundary shell
    if p.x <= 1 || p.z <= 1 || p.x >= max.x - 2 || p.z >= max.z - 2 || p.y <= 1 {
        return VoxelId::Bedrock;
    }

    // Underground: keep solid stone walls crisp between carved maze tunnels while leaving
    // occasional cracked seams and rare natural pockets.
    let freq = 0.065 + (depth_tier.saturating_sub(1) as f64) * 0.006;
    let air_threshold = 0.78 - (depth_tier.saturating_sub(1) as f64) * 0.02;
    let cracked_threshold = 0.66 - (depth_tier.saturating_sub(1) as f64) * 0.03;

    let n = simplex.get([p.x as f64 * freq, p.y as f64 * 0.075, p.z as f64 * freq]);
    let n2 = simplex.get([
        p.x as f64 * 0.14 + 31.7,
        p.y as f64 * 0.14,
        p.z as f64 * 0.14 - 19.3,
    ]);

    if p.y < shed_floor_y - 8 && p.y > 8 && n > air_threshold {
        return VoxelId::Air;
    }

    if n2 > cracked_threshold && p.y < shed_floor_y - 6 {
        VoxelId::CrackedRock
    } else {
        VoxelId::CaveStone
    }
}

fn carve_ellipsoid(world: &mut VoxelWorld, center: IVec3, radii: Vec3, shed_floor_y: i32) {
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
                    if p.y > 2 && p.y < shed_floor_y {
                        world.set(p, VoxelId::Air);
                    }
                }
            }
        }
    }
}

fn flatten_floor(
    world: &mut VoxelWorld,
    center: IVec3,
    radius: i32,
    mat: VoxelId,
    shed_floor_y: i32,
) {
    let r2 = radius * radius;
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dz * dz <= r2 {
                let p = IVec3::new(center.x + dx, center.y, center.z + dz);
                if p.y < shed_floor_y {
                    world.set(p, mat);
                    if p.y > 1 {
                        world.set(p - IVec3::Y, VoxelId::CaveStone);
                    }
                }
                // Ensure headroom right above floor without crossing `shed_floor_y`
                for h in 1..=3 {
                    let above = p + IVec3::new(0, h, 0);
                    if above.y < shed_floor_y {
                        world.set(above, VoxelId::Air);
                    }
                }
            }
        }
    }
}

/// Ensures `voxel` (and 2 voxels above it) are `Air` with solid ground directly beneath,
/// strictly staying below `shed_floor_y`, and returns the walkable world-meter position.
fn ensure_walkable_spawn(
    world: &mut VoxelWorld,
    voxel: IVec3,
    floor_mat: VoxelId,
    shed_floor_y: i32,
) -> Vec3 {
    let below = voxel - IVec3::Y;
    if below.y >= 1 && below.y < shed_floor_y && !world.is_solid(below) {
        world.set(below, floor_mat);
    }
    for h in 0..=2 {
        let p = voxel + IVec3::new(0, h, 0);
        if p.y > 2 && p.y < shed_floor_y {
            world.set(p, VoxelId::Air);
        }
    }
    let vs = world.voxel_size_m();
    Vec3::new(
        (voxel.x as f32 + 0.5) * vs,
        voxel.y as f32 * vs + 0.05,
        (voxel.z as f32 + 0.5) * vs,
    )
}

/// Builds an unobstructed 7x7 interior vertical steel elevator shaft strictly below `shed_floor_y`,
/// with a +X doorway at the cave bottom (`y <= cave_floor_y + 5`, `dz.abs() <= 2`).
fn build_shaft(
    world: &mut VoxelWorld,
    cx: i32,
    cz: i32,
    cave_floor_y: i32,
    shed_floor_y: i32,
) {
    // Solid steel landing pad directly underneath the elevator cage
    for dz in -SHAFT_RADIUS..=SHAFT_RADIUS {
        for dx in -SHAFT_RADIUS..=SHAFT_RADIUS {
            let floor_p = IVec3::new(cx + dx, cave_floor_y, cz + dz);
            if floor_p.y < shed_floor_y {
                world.set(floor_p, VoxelId::Steel);
            }
        }
    }

    for y in (cave_floor_y + 1)..shed_floor_y {
        for dz in -SHAFT_RADIUS..=SHAFT_RADIUS {
            for dx in -SHAFT_RADIUS..=SHAFT_RADIUS {
                let p = IVec3::new(cx + dx, y, cz + dz);
                let on_wall = dx.abs() == SHAFT_RADIUS || dz.abs() == SHAFT_RADIUS;
                if on_wall {
                    // Leave cave entrance opening at bottom on +X side
                    if y <= cave_floor_y + 5 && dx == SHAFT_RADIUS && dz.abs() <= 2 {
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
}

fn build_starter_shed(world: &mut VoxelWorld, cx: i32, cz: i32, shed_floor_y: i32) {
    let min_x = cx - SHED_HALF_MIN_X;
    let max_x = cx + SHED_HALF_MAX_X;
    let min_z = cz - SHED_HALF_Z;
    let max_z = cz + SHED_HALF_Z;
    let roof_y = shed_floor_y + SHED_ROOF_OFFSET;

    for y in shed_floor_y..=roof_y {
        for z in min_z..=max_z {
            for x in min_x..=max_x {
                let p = IVec3::new(x, y, z);
                let dx = x - cx;
                let dz = z - cz;
                let in_shaft_hole = dx.abs() < SHAFT_RADIUS && dz.abs() < SHAFT_RADIUS;
                let on_shaft_rim = !in_shaft_hole
                    && dx.abs() <= SHAFT_RADIUS
                    && dz.abs() <= SHAFT_RADIUS;

                if y == shed_floor_y {
                    if in_shaft_hole {
                        world.set(p, VoxelId::Air);
                    } else if on_shaft_rim {
                        world.set(p, VoxelId::Steel);
                    } else {
                        world.set(p, VoxelId::Timber);
                    }
                } else if y == roof_y
                    || x == min_x
                    || x == max_x
                    || z == min_z
                    || z == max_z
                {
                    world.set(p, VoxelId::Timber);
                } else {
                    world.set(p, VoxelId::Air);
                }
            }
        }
    }

    // Low 1-voxel steel safety railings around -X, -Z, +Z edges of the shaft hole,
    // leaving the +X boarding edge (`dx == SHAFT_RADIUS`, `dz in -3..=3`) wide open!
    let rail_y = shed_floor_y + 1;
    for d in -SHAFT_RADIUS..=SHAFT_RADIUS {
        // -X railing
        world.set(IVec3::new(cx - SHAFT_RADIUS, rail_y, cz + d), VoxelId::Steel);
        // -Z railing
        world.set(IVec3::new(cx + d, rail_y, cz - SHAFT_RADIUS), VoxelId::Steel);
        // +Z railing
        world.set(IVec3::new(cx + d, rail_y, cz + SHAFT_RADIUS), VoxelId::Steel);
    }

    // Wooden workbench counter along the +Z wall for starter spare items
    for x in (cx + 5)..=(cx + 8) {
        world.set(IVec3::new(x, shed_floor_y + 1, cz + 7), VoxelId::Timber);
    }

    // Bioluminescent worklights mounted near the Shed ceiling
    world.set(
        IVec3::new(cx - 6, shed_floor_y + 5, cz),
        VoxelId::BiolumCrystal,
    );
    world.set(
        IVec3::new(cx + 7, shed_floor_y + 5, cz),
        VoxelId::BiolumCrystal,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::UVec3;

    #[test]
    fn worldgen_is_deterministic_across_seeds_and_tiers() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        for tier in 1..=3 {
            let (w1, i1) = generate_world_with_tier(1337, tier, cfg);
            let (w2, i2) = generate_world_with_tier(1337, tier, cfg);
            assert_eq!(i1.depth_tier, tier);
            assert_eq!(i1.spawn_pos_m, i2.spawn_pos_m);
            assert_eq!(i1.elevator_top_pos_m, i2.elevator_top_pos_m);
            assert_eq!(i1.elevator_bottom_pos_m, i2.elevator_bottom_pos_m);
            assert_eq!(i1.relic_spawns_m, i2.relic_spawns_m);
            assert_eq!(i1.bonus_loot_spawns_m, i2.bonus_loot_spawns_m);
            assert_eq!(i1.monster_spawns_m, i2.monster_spawns_m);
            for x in 10..50 {
                for y in 10..80 {
                    let p = IVec3::new(x, y, 48);
                    assert_eq!(w1.get(p), w2.get(p));
                }
            }
        }
    }

    #[test]
    fn generate_world_delegates_to_tier_one() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        let (_w1, i1) = generate_world(4242, cfg);
        let (_w2, i2) = generate_world_with_tier(4242, 1, cfg);
        assert_eq!(i1.depth_tier, 1);
        assert_eq!(i1.spawn_pos_m, i2.spawn_pos_m);
        assert_eq!(i1.relic_spawns_m, i2.relic_spawns_m);
        assert_eq!(i1.bonus_loot_spawns_m, i2.bonus_loot_spawns_m);
        assert_eq!(i1.monster_spawns_m, i2.monster_spawns_m);
    }

    #[test]
    fn regenerate_underground_preserves_shed_and_changes_cave() {
        // Test on BOTH 3x3x3 (shed_floor_y = 72, mid-chunk!) and 4x4x4 (shed_floor_y = 96, chunk boundary!)
        for chunks in [UVec3::splat(3), UVec3::splat(4)] {
            let cfg = VoxelConfig::half_player_scale(chunks);
            let max = cfg.world_voxels();
            let (cx, cz, shed_floor_y, _cave_floor_y) = compute_layout_coords(max);

            let (mut world, info_before) = generate_world_with_tier(1001, 1, cfg);

            // Place player AcousticFoam edits inside the Base Shed (both on the floor level and interior)
            let foam_interior = IVec3::new(cx + 6, shed_floor_y + 1, cz - 2);
            let foam_upper = IVec3::new(cx - 5, shed_floor_y + 3, cz + 2);
            let foam_floor_level = IVec3::new(cx + 8, shed_floor_y, cz + 1);
            assert!(world.set(foam_interior, VoxelId::AcousticFoam));
            assert!(world.set(foam_upper, VoxelId::AcousticFoam));
            assert!(world.set(foam_floor_level, VoxelId::AcousticFoam));

            // Snapshot EVERY voxel inside the shed & surface layer (`y >= shed_floor_y`) around the shed
            let mut surface_snapshot = Vec::new();
            for y in shed_floor_y..=(shed_floor_y + SHED_ROOF_OFFSET) {
                for z in (cz - SHED_HALF_Z)..=(cz + SHED_HALF_Z) {
                    for x in (cx - SHED_HALF_MIN_X)..=(cx + SHED_HALF_MAX_X) {
                        let p = IVec3::new(x, y, z);
                        surface_snapshot.push((p, world.get(p)));
                    }
                }
            }

            // Snapshot an underground region before regeneration
            let mut cave_before = Vec::new();
            for y in 12..(shed_floor_y - 10) {
                for x in 10..(max.x - 10) {
                    let p = IVec3::new(x, y, cz + 15);
                    cave_before.push((p, world.get(p)));
                }
            }

            // Clear dirty chunks so we can also confirm regeneration marks underground chunks dirty
            let _ = world.take_dirty_chunks();

            // Regenerate underground with a new seed and deeper tier
            let info_after = regenerate_underground(&mut world, 99999, 3);
            assert_eq!(info_after.depth_tier, 3);
            assert_eq!(info_before.shed_floor_y_m, info_after.shed_floor_y_m);
            assert!(!world.take_dirty_chunks().is_empty());

            // Every single voxel at `y >= shed_floor_y` MUST be untouched, including player foam!
            assert_eq!(world.get(foam_interior), VoxelId::AcousticFoam);
            assert_eq!(world.get(foam_upper), VoxelId::AcousticFoam);
            assert_eq!(world.get(foam_floor_level), VoxelId::AcousticFoam);
            for (p, expected) in surface_snapshot {
                assert_eq!(
                    world.get(p),
                    expected,
                    "surface/shed voxel at {p:?} was unexpectedly modified by regenerate_underground"
                );
            }

            // Underground voxels MUST have changed with the new seed & tier
            let changed_count = cave_before
                .iter()
                .filter(|&&(p, old)| world.get(p) != old)
                .count();
            assert!(
                changed_count > 100,
                "expected significant underground changes after regeneration, got {changed_count}"
            );
        }
    }

    #[test]
    fn spawns_are_walkable_and_shaft_is_unobstructed() {
        let cfg = VoxelConfig::default(); // 4x4x4 = 128^3
        let max = cfg.world_voxels();
        let (cx, cz, shed_floor_y, cave_floor_y) = compute_layout_coords(max);

        for tier in 1..=3 {
            let (world, info) = generate_world_with_tier(777, tier, cfg);

            // Check expected spawn counts per tier
            let expected_relics = match tier {
                1 => 2,
                2 => 3,
                _ => 4,
            };
            assert_eq!(info.relic_spawns_m.len(), expected_relics);
            assert!((2..=4).contains(&info.bonus_loot_spawns_m.len()));
            assert_eq!(info.monster_spawns_m.len(), if tier == 1 { 1 } else { 2 });

            // Check spawn_pos_m is inside Base Shed on solid Timber floor with >= 2 Air headroom
            let spawn_v = cfg.world_to_voxel(info.spawn_pos_m);
            assert_eq!(world.get(spawn_v), VoxelId::Air);
            assert_eq!(world.get(spawn_v + IVec3::Y), VoxelId::Air);
            assert_eq!(world.get(spawn_v - IVec3::Y), VoxelId::Timber);
            assert!(info.spawn_pos_m.x >= info.shed_bounds_min_m.x);
            assert!(info.spawn_pos_m.x <= info.shed_bounds_max_m.x);
            assert!(info.spawn_pos_m.y >= info.shed_bounds_min_m.y);
            assert!(info.spawn_pos_m.y <= info.shed_bounds_max_m.y);
            assert!(info.spawn_pos_m.z >= info.shed_bounds_min_m.z);
            assert!(info.spawn_pos_m.z <= info.shed_bounds_max_m.z);

            // Check all relic, bonus loot, and monster spawns are Air with solid floor directly below
            for (label, list) in [
                ("relic", &info.relic_spawns_m),
                ("bonus_loot", &info.bonus_loot_spawns_m),
                ("monster", &info.monster_spawns_m),
            ] {
                for &pos in list {
                    let v = cfg.world_to_voxel(pos);
                    assert_eq!(
                        world.get(v),
                        VoxelId::Air,
                        "{label} spawn at {pos:?} (voxel {v:?}) must be Air (tier {tier})"
                    );
                    assert_eq!(
                        world.get(v + IVec3::Y),
                        VoxelId::Air,
                        "{label} spawn at {pos:?} needs headroom (tier {tier})"
                    );
                    assert!(
                        world.is_solid(v - IVec3::Y),
                        "{label} spawn at {pos:?} (voxel {v:?}) must have solid floor beneath (tier {tier})"
                    );
                }
            }

            // Monsters must spawn far from the cave elevator landing (no spawn camping)
            for &mpos in &info.monster_spawns_m {
                let dist = mpos.distance(info.elevator_bottom_pos_m);
                assert!(
                    dist >= 18.0,
                    "monster spawn too close to shaft landing: {dist:.1}m (tier {tier})"
                );
            }

            // Unobstructed 7x7 interior shaft (`dx, dz in -3..=3`) from cave floor to shed floor
            for y in (cave_floor_y + 1)..=shed_floor_y {
                for dz in -3..=3 {
                    for dx in -3..=3 {
                        assert_eq!(
                            world.get(IVec3::new(cx + dx, y, cz + dz)),
                            VoxelId::Air,
                            "shaft interior obstructed at dx={dx}, y={y}, dz={dz}"
                        );
                    }
                }
            }

            // Shed +X shaft boarding opening is clear (`dx == 4, dz in -3..=3`)
            for dz in -3..=3 {
                assert_eq!(
                    world.get(IVec3::new(cx + SHAFT_RADIUS, shed_floor_y + 1, cz + dz)),
                    VoxelId::Air
                );
            }
            // Shed -X, -Z, +Z safety railings exist at `shed_floor_y + 1`
            assert_eq!(
                world.get(IVec3::new(cx - SHAFT_RADIUS, shed_floor_y + 1, cz)),
                VoxelId::Steel
            );
            assert_eq!(
                world.get(IVec3::new(cx, shed_floor_y + 1, cz - SHAFT_RADIUS)),
                VoxelId::Steel
            );
            assert_eq!(
                world.get(IVec3::new(cx, shed_floor_y + 1, cz + SHAFT_RADIUS)),
                VoxelId::Steel
            );

            // Tunnel maze must include breakable CrackedRock barricades at tunnel floor height
            let mut cracked_count = 0usize;
            for dz in -28..=28 {
                for dx in -28..=28 {
                    if world.get(IVec3::new(cx + dx, cave_floor_y + 2, cz + dz))
                        == VoxelId::CrackedRock
                    {
                        cracked_count += 1;
                    }
                }
            }
            assert!(
                cracked_count >= 12,
                "expected breakable CrackedRock barricades across tunnel maze, got {cracked_count}"
            );
        }
    }

    #[test]
    fn depth_tiers_exhibit_distinct_biomes_elevations_and_ruins() {
        let cfg = VoxelConfig::default();
        let max = cfg.world_voxels();
        let (cx, cz, _shed_floor_y, cave_floor_y) = compute_layout_coords(max);

        let (w1, i1) = generate_world_with_tier(2026, 1, cfg);
        let (w2, i2) = generate_world_with_tier(2026, 2, cfg);
        let (w3, i3) = generate_world_with_tier(2026, 3, cfg);

        let count_underground = |world: &VoxelWorld, target: VoxelId| -> usize {
            let mut count = 0usize;
            for y in (cave_floor_y - 3)..=(cave_floor_y + 8) {
                for dz in -30..=30 {
                    for dx in -30..=30 {
                        if world.get(IVec3::new(cx + dx, y, cz + dz)) == target {
                            count += 1;
                        }
                    }
                }
            }
            count
        };

        // 1. Timber mine support frames: Tier 1 has abundant timber frames, Tier 2 has mid-catacomb
        //    timber bracing, and Tier 3 replaces timber frames with stone ruin archways & columns.
        let timber_t1 = count_underground(&w1, VoxelId::Timber);
        let timber_t2 = count_underground(&w2, VoxelId::Timber);
        let timber_t3 = count_underground(&w3, VoxelId::Timber);
        assert!(
            timber_t1 > timber_t2 && timber_t2 > 0 && timber_t3 == 0,
            "expected underground Timber counts T1 ({timber_t1}) > T2 ({timber_t2}) > T3 ({timber_t3} == 0)"
        );

        // 2. Bioluminescent crystals become progressively sparser on deeper tiers.
        let crystals_t1 = count_underground(&w1, VoxelId::BiolumCrystal);
        let crystals_t2 = count_underground(&w2, VoxelId::BiolumCrystal);
        let crystals_t3 = count_underground(&w3, VoxelId::BiolumCrystal);
        assert!(
            crystals_t1 > crystals_t2 && crystals_t2 > crystals_t3 && crystals_t3 >= 4,
            "expected sparser underground BiolumCrystal counts T1 ({crystals_t1}) > T2 ({crystals_t2}) > T3 ({crystals_t3})"
        );

        // 3. Elevation variety: Tier 1 spawns all sit at `cave_floor_y + 1`, whereas Tier 2 and Tier 3
        //    spawns span multiple stepped vertical elevations while keeping entry node (3, 2) at `cave_floor_y`.
        let collect_spawn_ys = |info: &GeneratedLevelInfo| -> Vec<i32> {
            let mut ys: Vec<i32> = info
                .relic_spawns_m
                .iter()
                .chain(info.bonus_loot_spawns_m.iter())
                .chain(info.monster_spawns_m.iter())
                .map(|&p| cfg.world_to_voxel(p).y)
                .collect();
            ys.sort_unstable();
            ys.dedup();
            ys
        };
        let ys_t1 = collect_spawn_ys(&i1);
        let ys_t2 = collect_spawn_ys(&i2);
        let ys_t3 = collect_spawn_ys(&i3);
        assert_eq!(
            ys_t1,
            vec![cave_floor_y + 1],
            "Tier 1 spawns should all lie on the flat cave_floor_y level"
        );
        assert!(
            ys_t2.len() >= 3,
            "Tier 2 stepped catacombs should place spawns across >= 3 distinct Y elevations, got {ys_t2:?}"
        );
        assert!(
            ys_t3.len() >= 3,
            "Tier 3 terraced ruins should place spawns across >= 3 distinct Y elevations, got {ys_t3:?}"
        );

        // Entry junction (3, 2) right outside the shaft vestibule must remain at `cave_floor_y` on all tiers
        for (tier, world) in [(1, &w1), (2, &w2), (3, &w3)] {
            let entry_floor = IVec3::new(cx + 11, cave_floor_y, cz);
            assert!(
                world.is_solid(entry_floor),
                "Tier {tier} entry node floor at {entry_floor:?} must be solid at cave_floor_y"
            );
            assert_eq!(
                world.get(entry_floor + IVec3::Y),
                VoxelId::Air,
                "Tier {tier} entry node headroom above cave_floor_y must be Air"
            );
        }

        // 4. Tier 3 Modular Ancient Ruins: outer corner sanctum vaults feature freestanding
        //    symmetrical `CaveStone` columns (surrounded on all 4 horizontal sides by `Air`)
        //    and 1-voxel-high stepped stone altar plinths under each corner relic.
        let count_freestanding_stone_pillars = |world: &VoxelWorld| -> usize {
            let mut pillar_voxels = 0usize;
            for y in (cave_floor_y - 1)..=(cave_floor_y + 5) {
                for dz in -28..=28 {
                    for dx in -28..=28 {
                        let p = IVec3::new(cx + dx, y, cz + dz);
                        if world.get(p) == VoxelId::CaveStone
                            && world.get(p + IVec3::X) == VoxelId::Air
                            && world.get(p - IVec3::X) == VoxelId::Air
                            && world.get(p + IVec3::Z) == VoxelId::Air
                            && world.get(p - IVec3::Z) == VoxelId::Air
                        {
                            pillar_voxels += 1;
                        }
                    }
                }
            }
            pillar_voxels
        };
        let pillars_t1 = count_freestanding_stone_pillars(&w1);
        let pillars_t3 = count_freestanding_stone_pillars(&w3);
        assert!(
            pillars_t3 >= 32 && pillars_t3 > pillars_t1 + 24,
            "expected freestanding CaveStone sanctum column voxels in Tier 3 ({pillars_t3}) >> Tier 1 ({pillars_t1})"
        );

        // Verify each of the 4 Tier 3 relics rests atop a 1-voxel stepped CaveStone altar plinth
        // surrounded by walkable Air on the sanctum floor level.
        assert_eq!(i3.relic_spawns_m.len(), 4);
        for &rpos in &i3.relic_spawns_m {
            let rv = cfg.world_to_voxel(rpos);
            let plinth = rv - IVec3::Y;
            assert_eq!(
                w3.get(plinth),
                VoxelId::CaveStone,
                "Tier 3 relic at {rv:?} must sit atop a CaveStone altar plinth"
            );
            assert_eq!(
                w3.get(plinth + IVec3::X),
                VoxelId::Air,
                "Tier 3 altar plinth at {plinth:?} must rise 1 voxel above the surrounding sanctum floor"
            );
            assert_eq!(
                w3.get(plinth - IVec3::X),
                VoxelId::Air,
                "Tier 3 altar plinth at {plinth:?} must rise 1 voxel above the surrounding sanctum floor"
            );
        }
    }
}
