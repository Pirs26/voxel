use std::collections::HashMap;

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use glam::IVec3;
use voxel_core::{ChunkMeshData, GeneratedLevelInfo, VoxelWorld, mesh_chunk};

#[derive(Resource)]
pub struct WorldResource(pub VoxelWorld);

#[derive(Resource)]
pub struct LevelInfoResource(pub GeneratedLevelInfo);

#[derive(Resource, Default)]
struct ChunkEntities {
    meshes: HashMap<IVec3, Entity>,
    lights: HashMap<IVec3, Vec<Entity>>,
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

fn sync_dirty_chunks(
    mut commands: Commands,
    mut world_res: ResMut<WorldResource>,
    mut chunk_entities: ResMut<ChunkEntities>,
    mut meshes: ResMut<Assets<Mesh>>,
    mat_handle: Res<VoxelMaterialHandle>,
) {
    let dirty = world_res.0.take_dirty_chunks();
    if dirty.is_empty() {
        return;
    }

    for cc in dirty {
        // Despawn old chunk mesh and crystal lights
        if let Some(old_ent) = chunk_entities.meshes.remove(&cc) {
            commands.entity(old_ent).despawn();
        }
        if let Some(old_lights) = chunk_entities.lights.remove(&cc) {
            for e in old_lights {
                commands.entity(e).despawn();
            }
        }

        let mesh_data = mesh_chunk(&world_res.0, cc);
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
