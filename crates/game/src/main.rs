mod ai_blind;
mod audio_bridge;
mod elevator;
mod gameplay;
mod health;
mod hud;
mod inventory;
mod net_bridge;
mod physics;
mod player;
mod relic;
mod save;
mod settings;
mod shed;
mod tools;
mod viewmodel;
mod voxel_render;

use std::sync::{Arc, RwLock};

use bevy::{light::cluster::GlobalClusterSettings, prelude::*};
use voxel_core::{VoxelConfig, generate_world};

use crate::{
    audio_bridge::AudioBridgePlugin,
    gameplay::GameplayPlugin,
    hud::HudPlugin,
    net_bridge::{DEFAULT_LEVEL_SEED, NetBridgePlugin},
    player::PlayerPlugin,
    settings::SettingsPlugin,
    tools::ToolsPlugin,
    viewmodel::ViewmodelPlugin,
    voxel_render::{LevelInfoResource, VoxelRenderPlugin, WorldResource},
};

fn configure_gpu_clustering(mut cluster_settings: Option<ResMut<GlobalClusterSettings>>) {
    if let Some(ref mut settings) = cluster_settings
        && let Some(ref mut gpu) = settings.gpu_clustering
    {
        gpu.initial_index_list_capacity = gpu.initial_index_list_capacity.max(131_072);
    }
}

fn main() {
    let config = VoxelConfig::default();
    let (world, level_info) = generate_world(DEFAULT_LEVEL_SEED, config);

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Echoes Below — Week 3 Core Extraction Loop".into(),
                resolution: (1280, 720).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.01, 0.012, 0.018)))
        .insert_resource(GlobalAmbientLight {
            color: Color::srgb(0.45, 0.52, 0.65),
            brightness: 52.0,
            ..default()
        })
        .insert_resource(WorldResource(Arc::new(RwLock::new(world))))
        .insert_resource(LevelInfoResource(level_info))
        .add_systems(Startup, configure_gpu_clustering)
        .add_plugins((
            SettingsPlugin,
            VoxelRenderPlugin,
            NetBridgePlugin,
            PlayerPlugin,
            ToolsPlugin,
            ViewmodelPlugin,
            GameplayPlugin,
            AudioBridgePlugin,
            HudPlugin,
        ))
        .run();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Headless ECS schedule verification: catches any Bevy B0001 query-aliasing panics
    /// across all game plugins during `cargo test` before building an executable!
    #[test]
    fn all_game_plugins_schedule_without_ecs_conflicts() {
        let config = VoxelConfig::half_player_scale(glam::UVec3::splat(3));
        let (world, level_info) = generate_world(DEFAULT_LEVEL_SEED, config);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<bevy::input::mouse::MouseMotion>()
            .add_message::<bevy::input::mouse::MouseWheel>()
            .add_message::<bevy::input::keyboard::KeyboardInput>()
            .add_message::<bevy::app::AppExit>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .insert_resource(GlobalAmbientLight::default())
            .insert_resource(WorldResource(Arc::new(RwLock::new(world))))
            .insert_resource(LevelInfoResource(level_info))
            .add_plugins((
                SettingsPlugin,
                VoxelRenderPlugin,
                NetBridgePlugin,
                PlayerPlugin,
                ToolsPlugin,
                ViewmodelPlugin,
                GameplayPlugin,
                AudioBridgePlugin,
                HudPlugin,
            ));

        // Run 2 full Startup + Update frames to validate every system's queries & Params!
        app.update();
        app.update();
    }
}
