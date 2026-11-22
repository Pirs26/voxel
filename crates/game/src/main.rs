mod ai_blind;
mod audio_bridge;
mod decibel_wall;
mod elevator;
mod gameplay;
mod health;
mod hud;
mod inventory;
mod net_bridge;
mod parkour_board;
mod physics;
mod player;
mod relic;
mod save;
mod settings;
mod shed;
mod tools;
mod updater_bridge;
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
    updater_bridge::UpdaterPlugin,
    viewmodel::ViewmodelPlugin,
    voxel_render::{LevelInfoResource, VoxelRenderPlugin, WorldResource},
};

/// Embedded SIL OFL 1.1 JetBrains Mono Regular TTF so global UI text renders with full
/// Unicode glyph coverage regardless of platform or working directory.
pub const JETBRAINS_MONO_TTF: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

/// Replaces Bevy's subsetted default font at [`AssetId::<Font>::default()`] during [`PreStartup`]
/// before the first `PostUpdate` font-collection pass runs, ensuring every `TextFont::default()`
/// across menus, HUD, modals, and billboards uses JetBrains Mono without missing-glyph boxes (`□`).
pub fn install_global_ui_font(mut fonts: Option<ResMut<Assets<Font>>>) {
    if let Some(ref mut assets) = fonts {
        let _ = assets.insert(
            AssetId::<Font>::default(),
            Font::from_bytes(JETBRAINS_MONO_TTF.to_vec()),
        );
    }
}

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
        .add_systems(PreStartup, install_global_ui_font)
        .add_systems(Startup, configure_gpu_clustering)
        .add_plugins((
            SettingsPlugin,
            UpdaterPlugin,
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
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<Font>>()
            .insert_resource(GlobalAmbientLight::default())
            .insert_resource(WorldResource(Arc::new(RwLock::new(world))))
            .insert_resource(LevelInfoResource(level_info))
            .add_systems(PreStartup, install_global_ui_font)
            .add_plugins((
                SettingsPlugin,
                UpdaterPlugin,
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

        let default_font = app
            .world()
            .resource::<Assets<Font>>()
            .get(AssetId::<Font>::default())
            .expect("default Font asset must be installed");
        assert_eq!(default_font.data.as_ref(), JETBRAINS_MONO_TTF);
    }

    /// Parses the TrueType `cmap` table (Format 4 and Format 12) directly from raw TTF bytes
    /// and returns the set of all Unicode scalar values mapped to a non-zero glyph index.
    fn parse_ttf_supported_codepoints(ttf: &[u8]) -> std::collections::HashSet<u32> {
        fn read_u16(buf: &[u8], off: usize) -> u16 {
            u16::from_be_bytes([buf[off], buf[off + 1]])
        }
        fn read_i16(buf: &[u8], off: usize) -> i16 {
            i16::from_be_bytes([buf[off], buf[off + 1]])
        }
        fn read_u32(buf: &[u8], off: usize) -> u32 {
            u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
        }

        let num_tables = read_u16(ttf, 4) as usize;
        let mut cmap_offset = None;
        for i in 0..num_tables {
            let rec = 12 + i * 16;
            if &ttf[rec..rec + 4] == b"cmap" {
                cmap_offset = Some(read_u32(ttf, rec + 8) as usize);
                break;
            }
        }
        let cmap_off = cmap_offset.expect("TTF must contain a cmap table");
        let num_subtables = read_u16(ttf, cmap_off + 2) as usize;
        let mut codepoints = std::collections::HashSet::new();

        for i in 0..num_subtables {
            let sub_rel = read_u32(ttf, cmap_off + 4 + i * 8 + 4) as usize;
            let sub_off = cmap_off + sub_rel;
            let fmt = read_u16(ttf, sub_off);
            if fmt == 4 {
                let seg_count = (read_u16(ttf, sub_off + 6) as usize) / 2;
                let end_off = sub_off + 14;
                let start_off = end_off + 2 * seg_count + 2;
                let delta_off = start_off + 2 * seg_count;
                let range_off = delta_off + 2 * seg_count;
                for s in 0..seg_count {
                    let ec = read_u16(ttf, end_off + s * 2) as u32;
                    let sc = read_u16(ttf, start_off + s * 2) as u32;
                    if sc == 0xFFFF {
                        continue;
                    }
                    let delta = read_i16(ttf, delta_off + s * 2) as i32;
                    let ro = read_u16(ttf, range_off + s * 2) as usize;
                    for cp in sc..=ec {
                        let gid = if ro == 0 {
                            (cp as i32 + delta) as u16
                        } else {
                            let idx_addr =
                                range_off + s * 2 + ro + ((cp - sc) as usize) * 2;
                            let raw_gid = read_u16(ttf, idx_addr);
                            if raw_gid == 0 {
                                0
                            } else {
                                (raw_gid as i32 + delta) as u16
                            }
                        };
                        if gid != 0 {
                            codepoints.insert(cp);
                        }
                    }
                }
            } else if fmt == 12 {
                let num_groups = read_u32(ttf, sub_off + 12) as usize;
                for g in 0..num_groups {
                    let grp_off = sub_off + 16 + g * 12;
                    let sc = read_u32(ttf, grp_off);
                    let ec = read_u32(ttf, grp_off + 4);
                    let sgid = read_u32(ttf, grp_off + 8);
                    for cp in sc..=ec {
                        if sgid + (cp - sc) != 0 {
                            codepoints.insert(cp);
                        }
                    }
                }
            }
        }
        codepoints
    }

    #[test]
    fn jetbrains_mono_loads_and_covers_every_ui_glyph() {
        let font = Font::from_bytes(JETBRAINS_MONO_TTF.to_vec());
        assert_eq!(font.data.len(), JETBRAINS_MONO_TTF.len());

        let supported = parse_ttf_supported_codepoints(JETBRAINS_MONO_TTF);
        assert!(
            supported.len() > 1000,
            "expected >1000 supported glyphs in JetBrainsMono-Regular.ttf, got {}",
            supported.len()
        );

        let required_ui_glyphs = [
            '•', '—', '–', '▼', '▲', '→', '●', '○', '°', '…', '±', '²',
        ];
        for ch in required_ui_glyphs {
            assert!(
                supported.contains(&(ch as u32)),
                "JetBrainsMono-Regular.ttf is missing required UI glyph '{}' (U+{:04X})",
                ch,
                ch as u32
            );
        }
    }
}
