//! Diegetic 3D wall-mounted Parkour Leaderboard & Live Clock billboard inside the Base Shed
//! Parkour Room. Renders crisp pixel-matrix LED typography into a dynamic Bevy `Image` texture
//! (pure Rust, zero external font/image dependencies) and updates only when the leaderboard or
//! live clock state changes.

use std::hash::{Hash, Hasher};

use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use voxel_core::GeneratedLevelInfo;

use crate::{
    save::{CampaignState, MAX_PARKOUR_LEADERBOARD, ParkourRecord},
    shed::{ParkourTimerState, ShedLandmarkVisual, format_parkour_time_ms},
};

pub const BOARD_WIDTH_PX: u32 = 512;
pub const BOARD_HEIGHT_PX: u32 = 320;

/// Resource holding the live GPU texture handle for the diegetic Parkour Wall Billboard.
#[derive(Resource)]
pub struct ParkourBillboardTexture {
    pub image: Handle<Image>,
    pub last_signature: u64,
}

/// Spawns the diegetic steel-framed LED Parkour Leaderboard & Clock billboard mounted on the
/// North wall of the Parkour Room (`facing +Z`), plus an isolated child `PointLight` (NO `Mesh3d`).
pub fn spawn_parkour_billboard(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    info: &GeneratedLevelInfo,
    leaderboard: &[ParkourRecord],
    parkour: &ParkourTimerState,
) {
    let pixels = render_parkour_billboard_rgba(leaderboard, parkour);
    let mut img = Image::new(
        Extent3d {
            width: BOARD_WIDTH_PX,
            height: BOARD_HEIGHT_PX,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::nearest();
    let image_handle = images.add(img);

    let sig = compute_board_signature(leaderboard, parkour);
    commands.insert_resource(ParkourBillboardTexture {
        image: image_handle.clone(),
        last_signature: sig,
    });

    let frame_mesh = meshes.add(Cuboid::new(3.84, 2.48, 0.14));
    let trim_mesh = meshes.add(Cuboid::new(3.86, 0.08, 0.16));
    let screen_mesh = meshes.add(Rectangle::new(3.60, 2.25));

    let frame_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.19, 0.23),
        metallic: 0.78,
        perceptual_roughness: 0.44,
        ..default()
    });
    let cyan_trim_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.18, 0.90, 0.84),
        emissive: LinearRgba::new(0.45, 3.2, 3.0, 1.0),
        perceptual_roughness: 0.25,
        ..default()
    });
    let screen_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(image_handle.clone()),
        emissive: LinearRgba::new(1.30, 1.30, 1.30, 1.0),
        emissive_texture: Some(image_handle),
        perceptual_roughness: 0.32,
        metallic: 0.05,
        ..default()
    });

    commands
        .spawn((
            Transform::from_translation(info.parkour_billboard_pos_m),
            Visibility::default(),
            ShedLandmarkVisual,
        ))
        .with_children(|parent| {
            // Industrial dark steel backing frame flush against the North wall
            parent.spawn((
                Mesh3d(frame_mesh),
                MeshMaterial3d(frame_mat),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ));
            // Glowing cyan accent rail along top of bezel
            parent.spawn((
                Mesh3d(trim_mesh),
                MeshMaterial3d(cyan_trim_mat),
                Transform::from_xyz(0.0, 1.25, 0.01),
            ));
            // Dynamic LED scoreboard quad facing +Z (upright, un-mirrored)
            parent.spawn((
                Mesh3d(screen_mesh),
                MeshMaterial3d(screen_mat),
                Transform::from_xyz(0.0, 0.0, 0.082),
            ));
            // Dedicated child PointLight illuminating the billboard & wall (NO Mesh3d)
            parent.spawn((
                PointLight {
                    color: Color::srgb(0.55, 0.92, 1.0),
                    intensity: 18_000.0,
                    range: 9.0,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, 0.30, 0.95),
            ));
        });
}

/// Refreshes the diegetic Parkour Wall Billboard texture whenever the leaderboard changes
/// or the live stopwatch advances (at 10 Hz while running, zero work while idle).
pub fn update_parkour_billboard_system(
    board: Option<ResMut<ParkourBillboardTexture>>,
    mut images: ResMut<Assets<Image>>,
    parkour: Res<ParkourTimerState>,
    campaign: Res<CampaignState>,
) {
    let Some(mut board) = board else {
        return;
    };
    let sig = compute_board_signature(&campaign.save.parkour_leaderboard, &parkour);
    if sig == board.last_signature {
        return;
    }
    board.last_signature = sig;

    if let Some(mut img) = images.get_mut(&board.image) {
        img.data = Some(render_parkour_billboard_rgba(
            &campaign.save.parkour_leaderboard,
            &parkour,
        ));
    }
}

fn compute_board_signature(leaderboard: &[ParkourRecord], parkour: &ParkourTimerState) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    leaderboard.len().hash(&mut h);
    for rec in leaderboard {
        rec.player_name.hash(&mut h);
        rec.time_ms.hash(&mut h);
    }
    parkour.running.hash(&mut h);
    if parkour.running {
        // Update wall clock at 10 Hz while a run is active
        ((parkour.elapsed_s * 10.0).floor() as u32).hash(&mut h);
    }
    let show_finish = parkour.finish_banner_s > 0.0;
    show_finish.hash(&mut h);
    if show_finish {
        parkour.last_finish_ms.hash(&mut h);
        parkour.last_finish_rank.hash(&mut h);
    }
    h.finish()
}

/// Renders the `512x320` RGBA8 LED Parkour Leaderboard & Live Clock screen.
pub fn render_parkour_billboard_rgba(
    leaderboard: &[ParkourRecord],
    parkour: &ParkourTimerState,
) -> Vec<u8> {
    let mut fb = Canvas::new(BOARD_WIDTH_PX, BOARD_HEIGHT_PX, [10, 16, 24, 255]);

    // Outer subtle inner border
    fb.stroke_rect(6, 6, BOARD_WIDTH_PX - 12, BOARD_HEIGHT_PX - 12, 2, [34, 62, 82, 255]);

    // Top Header Band
    fb.fill_rect(10, 10, BOARD_WIDTH_PX - 20, 42, [14, 28, 42, 255]);
    fb.fill_rect(10, 52, BOARD_WIDTH_PX - 20, 2, [56, 232, 218, 255]);
    fb.draw_text_centered("PARKOUR LEADERBOARD", 21, 3, [56, 232, 218, 255]);

    // Live Stopwatch Strip
    fb.fill_rect(16, 62, BOARD_WIDTH_PX - 32, 34, [15, 23, 34, 255]);
    fb.stroke_rect(16, 62, BOARD_WIDTH_PX - 32, 34, 1, [42, 74, 96, 255]);

    let (clock_str, clock_col) = if parkour.running {
        let ms = (parkour.elapsed_s * 1000.0).round() as u32;
        (
            format!("CLOCK: {}  [RUNNING]", format_parkour_time_ms(ms)),
            [56, 235, 224, 255],
        )
    } else if let Some(finish_ms) = parkour.last_finish_ms
        && parkour.finish_banner_s > 0.0
    {
        let tag = match parkour.last_finish_rank {
            Some(1) => "NEW #1!".to_string(),
            Some(r) => format!("#{r}"),
            None => "DONE".to_string(),
        };
        (
            format!("FINISH: {}  [{tag}]", format_parkour_time_ms(finish_ms)),
            [64, 235, 146, 255],
        )
    } else {
        (
            "PASS START GATE -> RING SUMMIT BELL".to_string(),
            [238, 186, 62, 255],
        )
    };
    fb.draw_text_centered(&clock_str, 72, 2, clock_col);

    // Column headers
    fb.draw_text("RANK  MINER", 24, 106, 2, [132, 160, 184, 255]);
    let best_lbl = "BEST TIME";
    let best_w = text_width_px(best_lbl, 2);
    fb.draw_text(best_lbl, BOARD_WIDTH_PX - 24 - best_w, 106, 2, [132, 160, 184, 255]);
    fb.fill_rect(20, 124, BOARD_WIDTH_PX - 40, 1, [42, 64, 84, 255]);

    if leaderboard.is_empty() {
        fb.draw_text_centered("NO OFFICIAL RUNS YET", 182, 2, [150, 174, 194, 255]);
        fb.draw_text_centered("STEP THROUGH EMERALD GATE TO START", 214, 2, [96, 134, 158, 255]);
    } else {
        let mut y = 132u32;
        for (idx, rec) in leaderboard.iter().take(MAX_PARKOUR_LEADERBOARD).enumerate() {
            if idx % 2 == 0 {
                fb.fill_rect(20, y.saturating_sub(2), BOARD_WIDTH_PX - 40, 20, [14, 22, 32, 255]);
            }
            let row_col = match idx {
                0 => [255, 204, 64, 255],  // #1 Gold
                1 => [236, 244, 252, 255], // #2 Bright Silver
                2 => [64, 232, 220, 255],  // #3 Cyan
                _ => [164, 188, 208, 255], // #4-8 Cool Slate
            };
            let rank_str = format!("#{}", idx + 1);
            let name_clean: String = rec.player_name.chars().take(16).collect();
            let time_str = format_parkour_time_ms(rec.time_ms);

            fb.draw_text(&rank_str, 26, y + 1, 2, row_col);
            fb.draw_text(&name_clean, 78, y + 1, 2, row_col);
            let tw = text_width_px(&time_str, 2);
            fb.draw_text(&time_str, BOARD_WIDTH_PX - 26 - tw, y + 1, 2, row_col);
            y += 22;
        }
    }

    fb.pixels
}

struct Canvas {
    w: u32,
    h: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(w: u32, h: u32, bg: [u8; 4]) -> Self {
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        for px in pixels.as_chunks_mut::<4>().0 {
            *px = bg;
        }
        Self { w, h, pixels }
    }

    #[inline]
    fn set_px(&mut self, x: u32, y: u32, col: [u8; 4]) {
        if x < self.w && y < self.h {
            let i = ((y * self.w + x) * 4) as usize;
            self.pixels[i..i + 4].copy_from_slice(&col);
        }
    }

    fn fill_rect(&mut self, x: u32, y: u32, rw: u32, rh: u32, col: [u8; 4]) {
        let x1 = (x + rw).min(self.w);
        let y1 = (y + rh).min(self.h);
        for py in y..y1 {
            for px in x..x1 {
                self.set_px(px, py, col);
            }
        }
    }

    fn stroke_rect(&mut self, x: u32, y: u32, rw: u32, rh: u32, t: u32, col: [u8; 4]) {
        self.fill_rect(x, y, rw, t, col);
        self.fill_rect(x, y + rh.saturating_sub(t), rw, t, col);
        self.fill_rect(x, y, t, rh, col);
        self.fill_rect(x + rw.saturating_sub(t), y, t, rh, col);
    }

    fn draw_text_centered(&mut self, text: &str, y: u32, scale: u32, col: [u8; 4]) {
        let w = text_width_px(text, scale);
        let x = self.w.saturating_sub(w) / 2;
        self.draw_text(text, x, y, scale, col);
    }

    fn draw_text(&mut self, text: &str, mut x: u32, y: u32, scale: u32, col: [u8; 4]) {
        let adv = 6 * scale;
        for ch in text.chars() {
            let glyph = glyph_5x7(ch.to_ascii_uppercase());
            for (row, &bits) in glyph.iter().enumerate() {
                for col_i in 0..5u32 {
                    if (bits >> (4 - col_i)) & 1 != 0 {
                        self.fill_rect(
                            x + col_i * scale,
                            y + row as u32 * scale,
                            scale,
                            scale,
                            col,
                        );
                    }
                }
            }
            x = x.saturating_add(adv);
        }
    }
}

fn text_width_px(text: &str, scale: u32) -> u32 {
    let n = text.chars().count() as u32;
    if n == 0 {
        0
    } else {
        n * 6 * scale - scale
    }
}

/// Compact 5x7 uppercase bitmap font (each byte's lower 5 bits represent one horizontal row).
fn glyph_5x7(ch: char) -> [u8; 7] {
    match ch {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b01110, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        ':' => [0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
        '#' => [0b01010, 0b11111, 0b01010, 0b01010, 0b11111, 0b01010, 0b00000],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '_' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
        '[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
        ']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
        '>' => [0b10000, 0b01000, 0b00100, 0b00010, 0b00100, 0b01000, 0b10000],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        ' ' => [0; 7],
        _ => [0b00000, 0b01110, 0b00001, 0b00110, 0b00100, 0b00000, 0b00100],
    }
}

#[cfg(test)]
mod tests {
    use voxel_core::{VoxelConfig, generate_world_with_tier};

    use super::*;

    #[test]
    fn billboard_renderer_produces_exact_rgba_buffer_and_changes_with_records() {
        let idle = ParkourTimerState::default();
        let empty_buf = render_parkour_billboard_rgba(&[], &idle);
        assert_eq!(empty_buf.len(), (BOARD_WIDTH_PX * BOARD_HEIGHT_PX * 4) as usize);

        let populated = vec![
            ParkourRecord {
                player_name: "Alice".to_string(),
                time_ms: 11_420,
            },
            ParkourRecord {
                player_name: "Bob".to_string(),
                time_ms: 14_850,
            },
        ];
        let pop_buf = render_parkour_billboard_rgba(&populated, &idle);
        assert_eq!(pop_buf.len(), empty_buf.len());
        assert_ne!(empty_buf, pop_buf, "leaderboard entries must render onto billboard");
    }

    #[test]
    fn spawn_parkour_billboard_isolates_point_light_from_mesh() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        let (_w, info) = generate_world_with_tier(4242, 1, cfg);
        let mut world = World::new();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let mut images = Assets::<Image>::default();
        let timer = ParkourTimerState::default();

        {
            let mut commands = world.commands();
            spawn_parkour_billboard(
                &mut commands,
                &mut meshes,
                &mut materials,
                &mut images,
                &info,
                &[],
                &timer,
            );
        }
        world.flush();

        assert!(world.contains_resource::<ParkourBillboardTexture>());
        let mut light_q = world.query::<(&PointLight, Option<&Mesh3d>)>();
        let lights: Vec<_> = light_q.iter(&world).collect();
        assert_eq!(lights.len(), 1);
        assert!(
            lights[0].1.is_none(),
            "Billboard PointLight must NOT share an entity with Mesh3d"
        );
    }
}
