//! Diegetic 3D wall-mounted Decibel Calibration Wall inside the Main Base Shed.
//! Renders a crisp `384x240` pixel-matrix LED dBFS meter, noise-gate marker, stealth
//! zone readout, and self-echo/gate shortcuts into a dynamic Bevy `Image` texture
//! (pure Rust, zero external font/image dependencies), updated at ~15 Hz only while
//! the player is inside the Base Shed (zero work while underground in the cave).

use std::hash::{Hash, Hasher};

use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use voxel_audio_dsp::MicTelemetry;
use voxel_core::GeneratedLevelInfo;

use crate::{
    audio_bridge::{AcousticTelemetry, VoiceSettings},
    player::PlayerController,
    relic::is_pos_in_shed,
    settings::AppScreen,
    shed::ShedLandmarkVisual,
    voxel_render::LevelInfoResource,
};

pub const WALL_WIDTH_PX: u32 = 384;
pub const WALL_HEIGHT_PX: u32 = 240;
pub const LED_SEGMENTS: usize = 24;

const DB_MIN: f32 = -65.0;
const DB_MAX: f32 = -5.0;
const UPDATE_INTERVAL_S: f32 = 1.0 / 15.0;

/// Resource holding the live GPU texture handle for the Shed's 3D Decibel Calibration Wall.
#[derive(Resource)]
pub struct DecibelWallTexture {
    pub image: Handle<Image>,
    pub accum_s: f32,
    pub last_signature: u64,
}

/// Spawns the diegetic `2.60 m x 1.65 m` steel-framed LED Decibel Calibration Wall mounted
/// on the Main Shed's inner `-Z` (North) wall (`facing +Z`), plus:
/// - an isolated child `PointLight` illuminating the calibration wall (NO `Mesh3d`), and
/// - a cozy dedicated `PointLight` illuminating the `-X` Wooden Gear Storage Shelf (NO `Mesh3d`).
pub fn spawn_decibel_wall(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    info: &GeneratedLevelInfo,
    telemetry: &AcousticTelemetry,
    voice_settings: &VoiceSettings,
) {
    let pixels = render_decibel_wall_rgba(&telemetry.mic, voice_settings);
    let mut img = Image::new(
        Extent3d {
            width: WALL_WIDTH_PX,
            height: WALL_HEIGHT_PX,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::nearest();
    let image_handle = images.add(img);

    let sig = compute_wall_signature(&telemetry.mic, voice_settings);
    commands.insert_resource(DecibelWallTexture {
        image: image_handle.clone(),
        accum_s: 0.0,
        last_signature: sig,
    });

    // 2.60 m wide x 1.65 m tall industrial steel bezel + 1.6:1 (384x240) LED screen
    let frame_mesh = meshes.add(Cuboid::new(2.60, 1.65, 0.14));
    let top_trim_mesh = meshes.add(Cuboid::new(2.62, 0.055, 0.16));
    let bot_trim_mesh = meshes.add(Cuboid::new(2.62, 0.04, 0.16));
    let screen_mesh = meshes.add(Rectangle::new(2.44, 1.525));

    let frame_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.19, 0.23),
        metallic: 0.78,
        perceptual_roughness: 0.44,
        ..default()
    });
    let amber_trim_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.96, 0.68, 0.18),
        emissive: LinearRgba::new(2.8, 1.75, 0.30, 1.0),
        perceptual_roughness: 0.25,
        ..default()
    });
    let cyan_trim_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.18, 0.88, 0.84),
        emissive: LinearRgba::new(0.35, 2.4, 2.3, 1.0),
        perceptual_roughness: 0.25,
        ..default()
    });
    let screen_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(image_handle.clone()),
        emissive: LinearRgba::new(1.25, 1.25, 1.25, 1.0),
        emissive_texture: Some(image_handle),
        perceptual_roughness: 0.32,
        metallic: 0.05,
        ..default()
    });

    commands
        .spawn((
            Transform::from_translation(info.decibel_wall_pos_m),
            Visibility::default(),
            ShedLandmarkVisual,
        ))
        .with_children(|parent| {
            // Industrial dark steel backing bezel flush against the North wall
            parent.spawn((
                Mesh3d(frame_mesh),
                MeshMaterial3d(frame_mat),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ));
            // Glowing amber accent rail along top of bezel
            parent.spawn((
                Mesh3d(top_trim_mesh),
                MeshMaterial3d(amber_trim_mat),
                Transform::from_xyz(0.0, 0.835, 0.01),
            ));
            // Glowing cyan accent rail along bottom of bezel
            parent.spawn((
                Mesh3d(bot_trim_mesh),
                MeshMaterial3d(cyan_trim_mat),
                Transform::from_xyz(0.0, -0.83, 0.01),
            ));
            // Dynamic LED calibration quad facing +Z (upright, un-mirrored)
            parent.spawn((
                Mesh3d(screen_mesh),
                MeshMaterial3d(screen_mat),
                Transform::from_xyz(0.0, 0.0, 0.082),
            ));
            // Dedicated child PointLight illuminating the calibration wall (NO Mesh3d)
            parent.spawn((
                PointLight {
                    color: Color::srgb(0.95, 0.82, 0.50),
                    intensity: 16_000.0,
                    range: 8.5,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, 0.25, 0.85),
            ));
        });

    // Cozy dedicated overhead worklight illuminating the -X Wooden Gear Storage Shelf (NO Mesh3d)
    let shelf_light_pos = Vec3::new(
        info.shed_bounds_min_m.x + 1.45,
        info.shed_floor_y_m + 2.35,
        info.elevator_top_pos_m.z,
    );
    commands
        .spawn((
            Transform::from_translation(shelf_light_pos),
            Visibility::default(),
            ShedLandmarkVisual,
        ))
        .with_children(|parent| {
            parent.spawn((
                PointLight {
                    color: Color::srgb(1.0, 0.76, 0.42),
                    intensity: 24_000.0,
                    range: 9.5,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::IDENTITY,
            ));
        });
}

/// Refreshes the diegetic Decibel Calibration Wall texture at ~15 Hz while the player is
/// inside the Base Shed, performing zero work while underground in the cave or in menus.
#[allow(clippy::too_many_arguments)]
pub fn update_decibel_wall_system(
    time: Res<Time>,
    screen: Res<AppScreen>,
    level_info: Option<Res<LevelInfoResource>>,
    wall: Option<ResMut<DecibelWallTexture>>,
    mut images: ResMut<Assets<Image>>,
    telemetry: Res<AcousticTelemetry>,
    voice_settings: Res<VoiceSettings>,
    player_q: Query<&PlayerController>,
) {
    if *screen != AppScreen::InGame {
        return;
    }
    let (Some(level_info), Some(mut wall)) = (level_info, wall) else {
        return;
    };
    let Ok(player) = player_q.single() else {
        return;
    };
    if !is_pos_in_shed(player.feet_pos_m, &level_info.0) {
        return;
    }

    wall.accum_s += time.delta_secs();
    if wall.accum_s < UPDATE_INTERVAL_S {
        return;
    }
    wall.accum_s %= UPDATE_INTERVAL_S;

    let sig = compute_wall_signature(&telemetry.mic, &voice_settings);
    if sig == wall.last_signature {
        return;
    }
    wall.last_signature = sig;

    if let Some(mut img) = images.get_mut(&wall.image) {
        img.data = Some(render_decibel_wall_rgba(&telemetry.mic, &voice_settings));
    }
}

fn compute_wall_signature(mic: &MicTelemetry, voice: &VoiceSettings) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    mic.available.hash(&mut h);
    mic.gate_open.hash(&mut h);
    ((mic.rms_db.clamp(-90.0, 0.0) * 2.0).round() as i32).hash(&mut h);
    let ai_tier: u8 = if mic.ai_noise_level >= 1.2 {
        2
    } else if mic.ai_noise_level >= 0.35 {
        1
    } else {
        0
    };
    ai_tier.hash(&mut h);
    voice.muted.hash(&mut h);
    voice.self_echo_test.hash(&mut h);
    ((voice.gate_threshold_db.clamp(-65.0, -15.0) * 2.0).round() as i32).hash(&mut h);
    h.finish()
}

/// Renders the `384x240` RGBA8 LED Voice Calibration Wall display.
pub fn render_decibel_wall_rgba(mic: &MicTelemetry, voice: &VoiceSettings) -> Vec<u8> {
    let mut fb = Canvas::new(WALL_WIDTH_PX, WALL_HEIGHT_PX, [10, 14, 22, 255]);

    // Outer subtle industrial bezel inner border
    fb.stroke_rect(4, 4, WALL_WIDTH_PX - 8, WALL_HEIGHT_PX - 8, 2, [42, 62, 82, 255]);

    // Top Header Band
    fb.fill_rect(8, 8, WALL_WIDTH_PX - 16, 36, [16, 26, 38, 255]);
    fb.fill_rect(8, 44, WALL_WIDTH_PX - 16, 2, [242, 174, 52, 255]);
    fb.draw_text_centered("VOICE CALIBRATION WALL", 19, 2, [250, 206, 86, 255]);

    // Meter Panel Container
    fb.fill_rect(14, 54, WALL_WIDTH_PX - 28, 70, [13, 20, 30, 255]);
    fb.stroke_rect(14, 54, WALL_WIDTH_PX - 28, 70, 1, [38, 62, 84, 255]);

    // Sub-header above meter
    fb.draw_text("INPUT LEVEL (dBFS)", 22, 60, 1, [136, 172, 198, 255]);
    let db_display = if !mic.available {
        "-- dB".to_string()
    } else if voice.muted {
        "MUTED".to_string()
    } else {
        format!("{:.0} dB", mic.rms_db.clamp(-90.0, 0.0))
    };
    let db_w = text_width_px(&db_display, 1);
    fb.draw_text(
        &db_display,
        WALL_WIDTH_PX - 22 - db_w,
        60,
        1,
        [220, 240, 252, 255],
    );

    // 24 Segmented LED Bars across -65 dB .. -5 dB
    let norm_mic = ((mic.rms_db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0);
    let lit_bars = if !mic.available || voice.muted {
        0
    } else {
        (norm_mic * LED_SEGMENTS as f32).round() as usize
    };

    let seg_w = 12u32;
    let seg_gap = 2u32;
    let meter_w = LED_SEGMENTS as u32 * seg_w + (LED_SEGMENTS as u32 - 1) * seg_gap; // 334 px
    let meter_x = (WALL_WIDTH_PX - meter_w) / 2; // 25 px
    let meter_y = 75u32;
    let meter_h = 28u32;

    for i in 0..LED_SEGMENTS {
        let sx = meter_x + i as u32 * (seg_w + seg_gap);
        let is_lit = i < lit_bars;
        let col = if is_lit {
            if !mic.gate_open {
                [66, 112, 140, 255]
            } else if i >= 19 || mic.ai_noise_level >= 1.2 {
                [246, 74, 74, 255]
            } else if i >= 14 || mic.ai_noise_level >= 0.35 {
                [248, 182, 54, 255]
            } else {
                [56, 224, 130, 255]
            }
        } else if i >= 19 {
            [40, 20, 24, 255]
        } else if i >= 14 {
            [38, 30, 18, 255]
        } else {
            [18, 34, 28, 255]
        };
        fb.fill_rect(sx, meter_y, seg_w, meter_h, col);
    }

    // Visible vertical white/cyan marker line for VoiceSettings::gate_threshold_db
    let gate_norm = ((voice.gate_threshold_db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0);
    let gate_x = meter_x + (gate_norm * (meter_w - 2) as f32).round() as u32;
    fb.fill_rect(gate_x.saturating_sub(1), meter_y - 3, 4, meter_h + 6, [44, 210, 228, 255]);
    fb.fill_rect(gate_x, meter_y - 4, 2, meter_h + 8, [242, 252, 255, 255]);

    // Scale dB markers under meter
    for (db_val, lbl) in [
        (-65.0_f32, "-65"),
        (-45.0, "-45"),
        (-28.0, "-28"),
        (-18.0, "-18"),
        (-5.0, "-5"),
    ] {
        let t = ((db_val - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0);
        let lw = text_width_px(lbl, 1);
        let tx = (meter_x + (t * meter_w as f32).round() as u32)
            .saturating_sub(lw / 2)
            .clamp(20, WALL_WIDTH_PX - 20 - lw);
        let lbl_col = if db_val >= -18.0 {
            [216, 98, 98, 255]
        } else if db_val >= -28.0 {
            [224, 174, 84, 255]
        } else {
            [118, 152, 178, 255]
        };
        fb.draw_text(lbl, tx, 110, 1, lbl_col);
    }

    // Live Status Readout Card
    fb.fill_rect(14, 132, WALL_WIDTH_PX - 28, 40, [14, 22, 34, 255]);
    let db_int = mic.rms_db.clamp(-90.0, 0.0).round() as i32;
    let gate_int = voice.gate_threshold_db.clamp(-65.0, -15.0).round() as i32;
    let (status_str, status_col, border_col) = if !mic.available {
        (
            "NO MICROPHONE DETECTED".to_string(),
            [246, 148, 64, 255],
            [110, 70, 36, 255],
        )
    } else if voice.muted {
        (
            "MIC: MUTED (PRESS [M])".to_string(),
            [248, 84, 84, 255],
            [142, 42, 46, 255],
        )
    } else if !mic.gate_open {
        (
            format!("QUIET  •  {db_int} dB  (GATE {gate_int} dB)"),
            [156, 178, 200, 255],
            [52, 76, 98, 255],
        )
    } else if mic.ai_noise_level < 0.35 {
        (
            format!("WHISPER SAFE  •  {db_int} dB"),
            [64, 236, 144, 255],
            [40, 132, 84, 255],
        )
    } else if mic.ai_noise_level < 1.2 {
        (
            format!("AUDIBLE TO STALKERS! • {db_int} dB"),
            [250, 186, 56, 255],
            [148, 106, 36, 255],
        )
    } else {
        (
            format!("AUDIBLE TO STALKERS! • {db_int} dB"),
            [252, 82, 82, 255],
            [158, 44, 48, 255],
        )
    };
    fb.stroke_rect(14, 132, WALL_WIDTH_PX - 28, 40, 1, border_col);
    let status_scale = if text_width_px(&status_str, 2) <= WALL_WIDTH_PX - 40 {
        2
    } else {
        1
    };
    let status_y = if status_scale == 2 { 145 } else { 149 };
    fb.draw_text_centered(&status_str, status_y, status_scale, status_col);

    // Bottom Calibration Controls Status Bar
    fb.fill_rect(14, 180, WALL_WIDTH_PX - 28, 46, [12, 19, 29, 255]);
    fb.stroke_rect(14, 180, WALL_WIDTH_PX - 28, 46, 1, [38, 62, 82, 255]);

    let (echo_str, echo_col) = if voice.self_echo_test {
        ("SELF-ECHO [V]: ON", [64, 232, 220, 255])
    } else {
        ("SELF-ECHO [V]: OFF", [150, 174, 196, 255])
    };
    fb.draw_text(echo_str, 22, 188, 1, echo_col);

    let gate_str = format!("GATE: {gate_int} dB (ESC -> SETTINGS)");
    let gate_w = text_width_px(&gate_str, 1);
    fb.draw_text(
        &gate_str,
        WALL_WIDTH_PX - 22 - gate_w,
        188,
        1,
        [242, 196, 84, 255],
    );

    fb.draw_text_centered(
        "[M] MUTE  •  [V] SELF-ECHO  •  [ / ] GATE",
        207,
        1,
        [104, 138, 164, 255],
    );

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
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
        '[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
        ']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
        '>' => [0b10000, 0b01000, 0b00100, 0b00010, 0b00100, 0b01000, 0b10000],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        '•' => [0b00000, 0b00000, 0b01110, 0b01110, 0b01110, 0b00000, 0b00000],
        ' ' => [0; 7],
        _ => [0b00000, 0b01110, 0b00001, 0b00110, 0b00100, 0b00000, 0b00100],
    }
}

#[cfg(test)]
mod tests {
    use voxel_core::{VoxelConfig, generate_world_with_tier};

    use super::*;

    #[test]
    fn decibel_wall_rgba_has_exact_dimensions_and_distinguishes_voice_states() {
        let voice = VoiceSettings::default();
        let muted_voice = VoiceSettings {
            muted: true,
            ..VoiceSettings::default()
        };

        let quiet_mic = MicTelemetry {
            rms_db: -60.0,
            gate_open: false,
            ai_noise_level: 0.0,
            available: true,
        };
        let whisper_mic = MicTelemetry {
            rms_db: -34.0,
            gate_open: true,
            ai_noise_level: 0.16,
            available: true,
        };
        let loud_mic = MicTelemetry {
            rms_db: -14.0,
            gate_open: true,
            ai_noise_level: 1.85,
            available: true,
        };

        let muted_buf = render_decibel_wall_rgba(&whisper_mic, &muted_voice);
        let quiet_buf = render_decibel_wall_rgba(&quiet_mic, &voice);
        let whisper_buf = render_decibel_wall_rgba(&whisper_mic, &voice);
        let loud_buf = render_decibel_wall_rgba(&loud_mic, &voice);

        let expected_len = (WALL_WIDTH_PX * WALL_HEIGHT_PX * 4) as usize;
        assert_eq!(muted_buf.len(), expected_len);
        assert_eq!(quiet_buf.len(), expected_len);
        assert_eq!(whisper_buf.len(), expected_len);
        assert_eq!(loud_buf.len(), expected_len);

        assert_ne!(muted_buf, whisper_buf, "muted vs whisper states must render differently");
        assert_ne!(whisper_buf, loud_buf, "whisper vs loud states must render differently");
        assert_ne!(muted_buf, loud_buf, "muted vs loud states must render differently");
        assert_ne!(quiet_buf, whisper_buf, "quiet vs whisper states must render differently");
    }

    #[test]
    fn spawn_decibel_wall_isolates_point_lights_from_meshes() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        let (_w, info) = generate_world_with_tier(4242, 1, cfg);
        let mut world = World::new();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let mut images = Assets::<Image>::default();
        let telemetry = AcousticTelemetry::default();
        let voice = VoiceSettings::default();

        {
            let mut commands = world.commands();
            spawn_decibel_wall(
                &mut commands,
                &mut meshes,
                &mut materials,
                &mut images,
                &info,
                &telemetry,
                &voice,
            );
        }
        world.flush();

        assert!(world.contains_resource::<DecibelWallTexture>());
        let mut light_q = world.query::<(&PointLight, Option<&Mesh3d>)>();
        let lights: Vec<_> = light_q.iter(&world).collect();
        assert_eq!(
            lights.len(),
            2,
            "expected calibration wall light + -X gear storage shelf light"
        );
        for (_, mesh) in lights {
            assert!(
                mesh.is_none(),
                "Decibel wall and gear shelf PointLights must NOT share an entity with Mesh3d"
            );
        }
    }
}
