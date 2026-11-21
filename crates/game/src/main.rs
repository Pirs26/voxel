mod audio_bridge;
mod hud;
mod net_bridge;
mod physics;
mod player;
mod tools;
mod voxel_render;

use bevy::prelude::*;
use voxel_core::{VoxelConfig, generate_world};

use crate::{
    audio_bridge::AudioBridgePlugin,
    hud::HudPlugin,
    net_bridge::{DEFAULT_LEVEL_SEED, NetBridgePlugin},
    player::PlayerPlugin,
    tools::ToolsPlugin,
    voxel_render::{LevelInfoResource, VoxelRenderPlugin, WorldResource},
};

fn main() {
    let config = VoxelConfig::default();
    let (world, level_info) = generate_world(DEFAULT_LEVEL_SEED, config);

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Echoes Below — Week 2 Co-op & Proximity Voice".into(),
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
            NetBridgePlugin,
            PlayerPlugin,
            ToolsPlugin,
            AudioBridgePlugin,
            HudPlugin,
        ))
        .run();
}
