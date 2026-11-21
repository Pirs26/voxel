mod audio_bridge;
mod hud;
mod physics;
mod player;
mod tools;
mod voxel_render;

use bevy::prelude::*;
use voxel_core::{VoxelConfig, generate_world};

use crate::{
    audio_bridge::AudioBridgePlugin,
    hud::HudPlugin,
    player::PlayerPlugin,
    tools::ToolsPlugin,
    voxel_render::{LevelInfoResource, VoxelRenderPlugin, WorldResource},
};

fn main() {
    let config = VoxelConfig::default();
    let (world, level_info) = generate_world(1337, config);

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Echoes Below — Week 1 Voxel Acoustic Prototype".into(),
                resolution: (1280, 720).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.01, 0.012, 0.018)))
        .insert_resource(GlobalAmbientLight {
            color: Color::srgb(0.45, 0.52, 0.65),
            brightness: 22.0,
            ..default()
        })
        .insert_resource(WorldResource(world))
        .insert_resource(LevelInfoResource(level_info))
        .add_plugins((
            VoxelRenderPlugin,
            PlayerPlugin,
            ToolsPlugin,
            AudioBridgePlugin,
            HudPlugin,
        ))
        .run();
}
