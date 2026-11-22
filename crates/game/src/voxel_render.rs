use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use glam::IVec3;
use voxel_core::{CHUNK_SIZE, ChunkMeshData, GeneratedLevelInfo, VoxelWorld, mesh_chunk};
use voxel_net::ElevatorPhase;

use crate::elevator::ElevatorState;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoadedWorldZone {
    /// Players are in the Base Shed: only Shed/surface chunks are meshed & loaded;
    /// strictly-underground mine chunks & crystal PointLights are unloaded from GPU/ECS.
    #[default]
    ShedOnly,
    /// Elevator is actively traveling up/down the shaft: load both zones seamlessly.
    Transition,
    /// Players are down in the mine: only underground chunks & crystal PointLights are loaded;
    /// strictly-Shed chunks are unloaded from GPU/ECS.
    CaveOnly,
}

/// Maps the current elevator phase to which world chunk zones should be resident on the GPU.
pub fn desired_zone_for_elevator(phase: ElevatorPhase) -> LoadedWorldZone {
    match phase {
        ElevatorPhase::ShedIdle => LoadedWorldZone::ShedOnly,
        ElevatorPhase::Descending | ElevatorPhase::Ascending => LoadedWorldZone::Transition,
        ElevatorPhase::CaveIdle | ElevatorPhase::WinchActive => LoadedWorldZone::CaveOnly,
    }
}

/// Returns `true` if chunk coordinate `cc` overlaps the requested [`LoadedWorldZone`]
/// relative to `shed_floor_y`.
pub fn chunk_allowed_in_zone(cc: IVec3, shed_floor_y: i32, zone: LoadedWorldZone) -> bool {
    let chunk_min_y = cc.y * CHUNK_SIZE as i32;
    let chunk_max_y_excl = chunk_min_y + CHUNK_SIZE as i32;
    match zone {
        LoadedWorldZone::ShedOnly => chunk_max_y_excl > shed_floor_y,
        LoadedWorldZone::CaveOnly => chunk_min_y < shed_floor_y,
        LoadedWorldZone::Transition => true,
    }
}

#[derive(Resource, Clone)]
pub struct WorldResource(pub Arc<RwLock<VoxelWorld>>);

impl WorldResource {
    pub fn read(&self) -> RwLockReadGuard<'_, VoxelWorld> {
        self.0.read().expect("voxel world rwlock poisoned")
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, VoxelWorld> {
        self.0.write().expect("voxel world rwlock poisoned")
    }
}

#[derive(Resource)]
pub struct LevelInfoResource(pub GeneratedLevelInfo);

#[derive(Resource, Default)]
struct ChunkEntities {
    meshes: HashMap<IVec3, Entity>,
    lights: HashMap<IVec3, Vec<Entity>>,
    active_zone: Option<LoadedWorldZone>,
}

#[derive(Resource)]
struct VoxelMaterialHandle(Handle<StandardMaterial>);

pub struct VoxelRenderPlugin;

impl Plugin for VoxelRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChunkEntities>()
            .add_systems(Startup, setup_voxel_material)
            .add_systems(Update, sync_dirty_chunks);
    }
}

fn setup_voxel_material(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.92,
        metallic: 0.05,
        ..default()
    });
    commands.insert_resource(VoxelMaterialHandle(mat));
}

fn despawn_chunk_entities(
    commands: &mut Commands,
    chunk_entities: &mut ChunkEntities,
    cc: IVec3,
) {
    if let Some(old_ent) = chunk_entities.meshes.remove(&cc) {
        commands.entity(old_ent).despawn();
    }
    if let Some(old_lights) = chunk_entities.lights.remove(&cc) {
        for e in old_lights {
            commands.entity(e).despawn();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_dirty_chunks(
    mut commands: Commands,
    world_res: Res<WorldResource>,
    level_info: Res<LevelInfoResource>,
    elevator: Option<Res<ElevatorState>>,
    mut chunk_entities: ResMut<ChunkEntities>,
    mut meshes: ResMut<Assets<Mesh>>,
    mat_handle: Res<VoxelMaterialHandle>,
) {
    let (shed_floor_y, world_chunks) = {
        let world = world_res.read();
        let shed_floor_y =
            (level_info.0.shed_floor_y_m / world.voxel_size_m()).round() as i32 - 1;
        (shed_floor_y, world.config().world_chunks)
    };
    let desired = elevator
        .as_deref()
        .map_or(LoadedWorldZone::ShedOnly, |e| {
            desired_zone_for_elevator(e.phase)
        });

    if chunk_entities.active_zone != Some(desired) {
        let to_unload: Vec<IVec3> = chunk_entities
            .meshes
            .keys()
            .chain(chunk_entities.lights.keys())
            .copied()
            .filter(|&cc| !chunk_allowed_in_zone(cc, shed_floor_y, desired))
            .collect();
        for cc in to_unload {
            despawn_chunk_entities(&mut commands, &mut chunk_entities, cc);
        }

        let mut to_dirty = Vec::new();
        for cy in 0..world_chunks.y as i32 {
            for cz in 0..world_chunks.z as i32 {
                for cx in 0..world_chunks.x as i32 {
                    let cc = IVec3::new(cx, cy, cz);
                    if chunk_allowed_in_zone(cc, shed_floor_y, desired)
                        && !chunk_entities.meshes.contains_key(&cc)
                    {
                        to_dirty.push(cc);
                    }
                }
            }
        }
        if !to_dirty.is_empty() {
            let mut world = world_res.write();
            for cc in to_dirty {
                world.mark_chunk_dirty(cc);
            }
        }

        chunk_entities.active_zone = Some(desired);
    }

    let dirty = world_res.write().take_dirty_chunks();
    if dirty.is_empty() {
        return;
    }

    let world = world_res.read();
    for cc in dirty {
        // Always despawn old chunk mesh and crystal lights first
        despawn_chunk_entities(&mut commands, &mut chunk_entities, cc);

        if !chunk_allowed_in_zone(cc, shed_floor_y, desired) {
            continue;
        }

        let mesh_data = mesh_chunk(&world, cc);
        if mesh_data.indices.is_empty() {
            continue;
        }

        // Spawn subtle atmospheric point lights on bioluminescent crystals (clustered per chunk)
        let mut light_ents = Vec::new();
        for (idx, (pos_m, emissive)) in mesh_data.emissive_centers_m.iter().enumerate() {
            // Limit lights per chunk so performance stays blazing fast
            if idx % 2 == 0 && light_ents.len() < 4 {
                let light = commands
                    .spawn((
                        PointLight {
                            color: Color::srgb(emissive[0], emissive[1], emissive[2]),
                            intensity: 18_000.0,
                            range: 14.0,
                            shadow_maps_enabled: false,
                            ..default()
                        },
                        Transform::from_translation(*pos_m + Vec3::Y * 0.3),
                    ))
                    .id();
                light_ents.push(light);
            }
        }
        if !light_ents.is_empty() {
            chunk_entities.lights.insert(cc, light_ents);
        }

        let bevy_mesh = build_bevy_mesh(mesh_data);
        let handle = meshes.add(bevy_mesh);
        let ent = commands
            .spawn((
                Mesh3d(handle),
                MeshMaterial3d(mat_handle.0.clone()),
                Transform::default(),
            ))
            .id();
        chunk_entities.meshes.insert(cc, ent);
    }
}

fn build_bevy_mesh(data: ChunkMeshData) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, data.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, data.colors);
    mesh.insert_indices(Indices::U32(data.indices));
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::UVec3;
    use voxel_core::{VoxelConfig, generate_world};

    #[test]
    fn desired_zone_for_elevator_maps_all_phases() {
        assert_eq!(
            desired_zone_for_elevator(ElevatorPhase::ShedIdle),
            LoadedWorldZone::ShedOnly
        );
        assert_eq!(
            desired_zone_for_elevator(ElevatorPhase::Descending),
            LoadedWorldZone::Transition
        );
        assert_eq!(
            desired_zone_for_elevator(ElevatorPhase::CaveIdle),
            LoadedWorldZone::CaveOnly
        );
        assert_eq!(
            desired_zone_for_elevator(ElevatorPhase::WinchActive),
            LoadedWorldZone::CaveOnly
        );
        assert_eq!(
            desired_zone_for_elevator(ElevatorPhase::Ascending),
            LoadedWorldZone::Transition
        );
    }

    #[test]
    fn chunk_allowed_in_zone_partitions_4x4x4_and_handles_3x3x3_boundary() {
        // 4x4x4 (128^3): shed_floor_y = 96 is an exact chunk boundary (cc.y == 3 vs cc.y in 0..=2).
        let shed_floor_y_128 = 96;
        for cy in 0..4 {
            let cc = IVec3::new(1, cy, 1);
            assert_eq!(
                chunk_allowed_in_zone(cc, shed_floor_y_128, LoadedWorldZone::ShedOnly),
                cy == 3,
                "4x4x4 ShedOnly mismatch at cy={cy}"
            );
            assert_eq!(
                chunk_allowed_in_zone(cc, shed_floor_y_128, LoadedWorldZone::CaveOnly),
                cy < 3,
                "4x4x4 CaveOnly mismatch at cy={cy}"
            );
            assert!(chunk_allowed_in_zone(
                cc,
                shed_floor_y_128,
                LoadedWorldZone::Transition
            ));
        }

        // 3x3x3 (96^3): shed_floor_y = 72 lies inside boundary chunk cc.y == 2 (y = 64..96).
        let shed_floor_y_96 = 72;
        assert!(!chunk_allowed_in_zone(
            IVec3::new(0, 0, 0),
            shed_floor_y_96,
            LoadedWorldZone::ShedOnly
        ));
        assert!(!chunk_allowed_in_zone(
            IVec3::new(0, 1, 0),
            shed_floor_y_96,
            LoadedWorldZone::ShedOnly
        ));
        assert!(chunk_allowed_in_zone(
            IVec3::new(0, 2, 0),
            shed_floor_y_96,
            LoadedWorldZone::ShedOnly
        ));

        assert!(chunk_allowed_in_zone(
            IVec3::new(0, 0, 0),
            shed_floor_y_96,
            LoadedWorldZone::CaveOnly
        ));
        assert!(chunk_allowed_in_zone(
            IVec3::new(0, 1, 0),
            shed_floor_y_96,
            LoadedWorldZone::CaveOnly
        ));
        assert!(chunk_allowed_in_zone(
            IVec3::new(0, 2, 0),
            shed_floor_y_96,
            LoadedWorldZone::CaveOnly
        ));
        assert!(!chunk_allowed_in_zone(
            IVec3::new(0, 3, 0),
            shed_floor_y_96,
            LoadedWorldZone::CaveOnly
        ));

        // Verify exact recovery of shed_floor_y from shed_floor_y_m across world sizes & scales.
        for cfg in [
            VoxelConfig::half_player_scale(UVec3::splat(3)),
            VoxelConfig::half_player_scale(UVec3::splat(4)),
            VoxelConfig::quarter_player_scale(UVec3::splat(4)),
        ] {
            let (world, info) = generate_world(4242, cfg);
            let expected_shed_floor_y = (cfg.world_voxels().y * 3) / 4;
            let recovered_shed_floor_y =
                (info.shed_floor_y_m / world.voxel_size_m()).round() as i32 - 1;
            assert_eq!(recovered_shed_floor_y, expected_shed_floor_y);
        }
    }

    #[test]
    fn sync_dirty_chunks_streams_shed_and_cave_across_elevator_phases() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(4));
        let (voxel_world, level_info) = generate_world(1337, cfg);
        let elevator = ElevatorState::from_level_info(&level_info, 1337);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .insert_resource(WorldResource(Arc::new(RwLock::new(voxel_world))))
            .insert_resource(LevelInfoResource(level_info))
            .insert_resource(elevator)
            .add_plugins(VoxelRenderPlugin);

        let count_point_lights = |world: &mut World| -> usize {
            let mut q = world.query::<&PointLight>();
            q.iter(world).count()
        };

        // 1. Startup in ShedIdle -> ShedOnly: only cc.y == 3 meshes, zero underground meshes/lights.
        app.update();
        {
            let ce = app.world().resource::<ChunkEntities>();
            assert_eq!(ce.active_zone, Some(LoadedWorldZone::ShedOnly));
            assert!(!ce.meshes.is_empty());
            assert!(ce.meshes.keys().all(|cc| cc.y == 3));
            assert!(ce.lights.is_empty());
        }
        assert_eq!(count_point_lights(app.world_mut()), 0);

        // 2. Descending -> Transition: loads both Shed (cc.y == 3) and Cave (cc.y < 3) + crystal lights.
        app.world_mut().resource_mut::<ElevatorState>().phase = ElevatorPhase::Descending;
        app.update();
        {
            let ce = app.world().resource::<ChunkEntities>();
            assert_eq!(ce.active_zone, Some(LoadedWorldZone::Transition));
            assert!(ce.meshes.keys().any(|cc| cc.y == 3));
            assert!(ce.meshes.keys().any(|cc| cc.y < 3));
            assert!(!ce.lights.is_empty());
            assert!(ce.lights.keys().all(|cc| cc.y < 3));
        }
        assert!(count_point_lights(app.world_mut()) > 0);

        // 3. CaveIdle -> CaveOnly: despawns all Shed (cc.y == 3) meshes, keeps Cave meshes & crystal lights.
        let shed_entities_before_cave: Vec<Entity> = app
            .world()
            .resource::<ChunkEntities>()
            .meshes
            .iter()
            .filter_map(|(cc, &ent)| (cc.y == 3).then_some(ent))
            .collect();
        assert!(!shed_entities_before_cave.is_empty());

        app.world_mut().resource_mut::<ElevatorState>().phase = ElevatorPhase::CaveIdle;
        app.update();
        {
            let ce = app.world().resource::<ChunkEntities>();
            assert_eq!(ce.active_zone, Some(LoadedWorldZone::CaveOnly));
            assert!(!ce.meshes.is_empty());
            assert!(ce.meshes.keys().all(|cc| cc.y < 3));
            assert!(!ce.lights.is_empty());
        }
        for ent in shed_entities_before_cave {
            assert!(
                app.world().get_entity(ent).is_err(),
                "Shed chunk mesh entity should be despawned in CaveIdle"
            );
        }
        assert!(count_point_lights(app.world_mut()) > 0);

        // 4. Ascending -> Transition: reloads Shed (cc.y == 3) alongside Cave (cc.y < 3).
        app.world_mut().resource_mut::<ElevatorState>().phase = ElevatorPhase::Ascending;
        app.update();
        {
            let ce = app.world().resource::<ChunkEntities>();
            assert_eq!(ce.active_zone, Some(LoadedWorldZone::Transition));
            assert!(ce.meshes.keys().any(|cc| cc.y == 3));
            assert!(ce.meshes.keys().any(|cc| cc.y < 3));
        }

        // 5. Returned to ShedIdle -> ShedOnly: despawns all Cave meshes & crystal lights, keeps VoxelWorld intact.
        let cave_entities_before_shed: Vec<Entity> = {
            let ce = app.world().resource::<ChunkEntities>();
            ce.meshes
                .iter()
                .filter_map(|(cc, &ent)| (cc.y < 3).then_some(ent))
                .chain(
                    ce.lights
                        .iter()
                        .filter(|(cc, _)| cc.y < 3)
                        .flat_map(|(_, ents)| ents.iter().copied()),
                )
                .collect()
        };
        assert!(!cave_entities_before_shed.is_empty());

        app.world_mut().resource_mut::<ElevatorState>().phase = ElevatorPhase::ShedIdle;
        app.update();
        {
            let ce = app.world().resource::<ChunkEntities>();
            assert_eq!(ce.active_zone, Some(LoadedWorldZone::ShedOnly));
            assert!(!ce.meshes.is_empty());
            assert!(ce.meshes.keys().all(|cc| cc.y == 3));
            assert!(ce.lights.is_empty());
        }
        for ent in cave_entities_before_shed {
            assert!(
                app.world().get_entity(ent).is_err(),
                "Underground chunk/light entity should be despawned in ShedIdle"
            );
        }
        assert_eq!(count_point_lights(app.world_mut()), 0);
        assert!(
            app.world()
                .resource::<WorldResource>()
                .read()
                .chunk_coords()
                .any(|cc| cc.y < 3),
            "VoxelWorld RAM data must remain intact when GPU meshes are unloaded"
        );
    }
}
