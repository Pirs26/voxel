//! Diegetic 3D wall-mounted Acoustic Calibration Wall & physical Condenser Microphone Stand
//! inside the Main Base Shed (West `-X` Acoustic Calibration Bay behind the elevator).
//!
//! ALL noises in the Base Shed — local & co-op voice chat, footsteps, dropped items, thrown
//! flares, acoustic foam sprays, tool impacts, decoys, and elevator machinery — are measured
//! physically at the standing condenser microphone capsule (`info.decibel_mic_pos_m`) using the
//! exact same 3-band voxel occlusion & 1-bounce diffraction physics (`voxel_acoustics`) that
//! drives the Blind Stalker & Heavy Lurker predators!
//!
//! Renders a crisp `384x240` pixel-matrix LED dB meter with peak-hold, voice gate threshold
//! marker, 3-band (`LOW`/`MID`/`HIGH`) spectral bars, detected source + distance badge, and
//! live Stalker audibility readout into a dynamic Bevy `Image` texture at ~15 Hz while the
//! player is inside the Base Shed (zero work while underground in the cave).

use std::{
    f32::consts::FRAC_PI_2,
    hash::{Hash, Hasher},
};

use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use voxel_acoustics::{
    AcousticEvent, AcousticEventKind, Bands3, compute_direct_occlusion, evaluate_ai_hearing,
    linear_to_db,
};
use voxel_audio_dsp::MicTelemetry;
use voxel_core::{GeneratedLevelInfo, VoxelWorld};

use crate::{
    audio_bridge::{
        AcousticTelemetry, IncomingVoiceFrame, PlaySpatialSoundEvent, VoiceSettings,
    },
    net_bridge::{CoopState, remote_player_head_pos},
    player::PlayerController,
    relic::is_pos_in_shed,
    settings::AppScreen,
    shed::ShedLandmarkVisual,
    voxel_render::{LevelInfoResource, WorldResource},
};

pub const WALL_WIDTH_PX: u32 = 384;
pub const WALL_HEIGHT_PX: u32 = 240;
pub const LED_SEGMENTS: usize = 24;

const DB_MIN: f32 = -65.0;
const DB_MAX: f32 = -5.0;
const UPDATE_INTERVAL_S: f32 = 1.0 / 15.0;

/// Exact Tier-1 Blind Stalker frequency sensitivity (`Mid/High`-biased).
const STALKER_SENSITIVITY: Bands3 = Bands3::new(0.45, 1.25, 1.65);
/// Exact Tier-1 Heavy Lurker frequency sensitivity (`Low`-rumble biased).
const LURKER_SENSITIVITY: Bands3 = Bands3::new(2.20, 0.55, 0.30);

const STALKER_HEAR_THRESHOLD: f32 = 0.40;
const STALKER_HUNT_THRESHOLD: f32 = 1.10;
const LURKER_HEAR_THRESHOLD: f32 = 0.18;

const METER_DECAY_DB_PER_S: f32 = 28.0;
const PEAK_HOLD_DURATION_S: f32 = 1.4;
const PEAK_DECAY_DB_PER_S: f32 = 18.0;
const SOURCE_HOLD_DURATION_S: f32 = 1.6;

/// Marker component for the live status LED ring on the physical 3D Condenser Microphone Stand.
#[derive(Component)]
pub struct DecibelMicLed;

/// Instantaneous physical reading measured at the Shed condenser microphone capsule.
#[derive(Clone, Copy, Debug)]
pub struct MicSensorReading {
    pub meter_db: f32,
    pub bands_norm: [f32; 3],
    pub stalker_salience: f32,
    pub lurker_salience: f32,
    pub source_label: &'static str,
    pub distance_m: f32,
    pub occluded_db: f32,
}

impl Default for MicSensorReading {
    fn default() -> Self {
        Self {
            meter_db: DB_MIN,
            bands_norm: [0.0; 3],
            stalker_salience: 0.0,
            lurker_salience: 0.0,
            source_label: "QUIET",
            distance_m: 0.0,
            occluded_db: 0.0,
        }
    }
}

/// Smoothed ballistic VU meter state for the physical Shed Microphone Sensor.
#[derive(Clone, Debug)]
pub struct DecibelSensorState {
    pub current_db: f32,
    pub peak_db: f32,
    pub peak_hold_s: f32,
    pub bands_norm: [f32; 3],
    pub stalker_salience: f32,
    pub lurker_salience: f32,
    pub last_source_label: &'static str,
    pub last_distance_m: f32,
    pub last_occluded_db: f32,
    pub source_hold_s: f32,
}

impl Default for DecibelSensorState {
    fn default() -> Self {
        Self {
            current_db: DB_MIN,
            peak_db: DB_MIN,
            peak_hold_s: 0.0,
            bands_norm: [0.0; 3],
            stalker_salience: 0.0,
            lurker_salience: 0.0,
            last_source_label: "AMBIENT",
            last_distance_m: 0.0,
            last_occluded_db: 0.0,
            source_hold_s: 0.0,
        }
    }
}

impl DecibelSensorState {
    /// Applies ballistic VU decay over `dt` seconds before ingesting new frame events.
    pub fn decay(&mut self, dt: f32) {
        self.current_db = (self.current_db - METER_DECAY_DB_PER_S * dt).max(DB_MIN);
        if self.peak_hold_s > 0.0 {
            self.peak_hold_s = (self.peak_hold_s - dt).max(0.0);
        } else {
            self.peak_db = (self.peak_db - PEAK_DECAY_DB_PER_S * dt).max(self.current_db);
        }

        let band_decay = (-dt * 4.5).exp();
        for b in &mut self.bands_norm {
            *b *= band_decay;
            if *b < 0.005 {
                *b = 0.0;
            }
        }

        let salience_decay = (-dt * 2.4).exp();
        self.stalker_salience *= salience_decay;
        self.lurker_salience *= salience_decay;
        if self.stalker_salience < 0.01 {
            self.stalker_salience = 0.0;
        }
        if self.lurker_salience < 0.01 {
            self.lurker_salience = 0.0;
        }

        if self.source_hold_s > 0.0 {
            self.source_hold_s = (self.source_hold_s - dt).max(0.0);
            if self.source_hold_s <= 0.0 {
                self.last_source_label = "AMBIENT";
                self.last_distance_m = 0.0;
                self.last_occluded_db = 0.0;
            }
        }
    }

    /// Feeds an instantaneous reading measured at the physical condenser microphone capsule.
    pub fn ingest(&mut self, reading: MicSensorReading) {
        if reading.meter_db <= DB_MIN + 0.5 && reading.stalker_salience <= 0.01 {
            return;
        }

        let dominates_source = reading.meter_db >= self.current_db - 2.0
            || self.source_hold_s <= 0.0
            || reading.stalker_salience > self.stalker_salience;

        if reading.meter_db > self.current_db {
            self.current_db = reading.meter_db.clamp(DB_MIN, DB_MAX);
        }
        if reading.meter_db >= self.peak_db {
            self.peak_db = reading.meter_db.clamp(DB_MIN, DB_MAX);
            self.peak_hold_s = PEAK_HOLD_DURATION_S;
        }
        for i in 0..3 {
            self.bands_norm[i] = self.bands_norm[i].max(reading.bands_norm[i].clamp(0.0, 1.0));
        }
        self.stalker_salience = self.stalker_salience.max(reading.stalker_salience);
        self.lurker_salience = self.lurker_salience.max(reading.lurker_salience);

        if dominates_source {
            self.last_source_label = reading.source_label;
            self.last_distance_m = reading.distance_m;
            self.last_occluded_db = reading.occluded_db;
            self.source_hold_s = SOURCE_HOLD_DURATION_S;
        }
    }
}

/// Resource holding the live GPU texture handle and physical microphone sensor state for the
/// Shed's 3D Acoustic Calibration Wall.
#[derive(Resource)]
pub struct DecibelWallTexture {
    pub image: Handle<Image>,
    pub accum_s: f32,
    pub last_signature: u64,
    pub sensor: DecibelSensorState,
}

/// Maps an [`AcousticEventKind`] to a crisp uppercase display tag on the LED board.
pub fn source_label_for_kind(kind: AcousticEventKind) -> &'static str {
    match kind {
        AcousticEventKind::Footstep => "FOOTSTEP",
        AcousticEventKind::VoiceSpeech => "VOICE",
        AcousticEventKind::ItemImpact => "ITEM IMPACT",
        AcousticEventKind::RelicScrape => "RELIC SCRAPE",
        AcousticEventKind::WallBreach => "WALL BREACH",
        AcousticEventKind::DecoyNoise => "DECOY LURE",
        AcousticEventKind::ElevatorMachinery => "ELEVATOR",
    }
}

/// Evaluates ANY world [`AcousticEvent`] physically at the Shed condenser microphone capsule
/// (`mic_pos_m`) using the exact same voxel density occlusion & 1-bounce reflection solver
/// (`voxel_acoustics::evaluate_ai_hearing`) that drives Blind Stalkers and Heavy Lurkers.
pub fn evaluate_mic_sensor_event(
    world: &VoxelWorld,
    event: AcousticEvent,
    mic_pos_m: Vec3,
) -> MicSensorReading {
    let distance_m = event.pos_m.distance(mic_pos_m);
    if distance_m > 32.0 {
        return MicSensorReading::default();
    }

    let occ = compute_direct_occlusion(world, event.pos_m, mic_pos_m, 2.0, 55.0);
    let Some(perceived) = evaluate_ai_hearing(world, event, mic_pos_m, STALKER_SENSITIVITY, 0.002)
    else {
        return MicSensorReading::default();
    };

    let received = perceived.received_energy;
    let stalker_salience = perceived.salience;
    let lw = received.mul(LURKER_SENSITIVITY);
    let lurker_salience = lw.0[0] + lw.0[1] + lw.0[2];

    // Convert received 3-band acoustic energy into calibrated -65..=-5 dB studio meter scale:
    // - Quiet crouch step / soft click (~0.18 salience) -> ~-42 dB (green whisper-safe)
    // - Normal walk near mic (~0.55 salience) -> ~-28 dB (amber audible)
    // - Sprint / heavy drop / decoy (~1.8+ salience) -> ~-14..-6 dB (red hunt alert)
    let peak_salience = stalker_salience.max(lurker_salience * 0.65);
    let meter_db = if peak_salience <= 0.004 {
        DB_MIN
    } else {
        (-21.0 + 18.0 * peak_salience.log10()).clamp(DB_MIN, DB_MAX)
    };

    let band_to_norm = |e: f32| -> f32 {
        if e <= 0.002 {
            0.0
        } else {
            ((e.log10() + 1.8) / 2.3).clamp(0.0, 1.0)
        }
    };

    let occluded_db = occ.transmission_loss_db.0[1].max(0.0);

    MicSensorReading {
        meter_db,
        bands_norm: [
            band_to_norm(received.0[0]),
            band_to_norm(received.0[1]),
            band_to_norm(received.0[2]),
        ],
        stalker_salience,
        lurker_salience,
        source_label: source_label_for_kind(event.kind),
        distance_m,
        occluded_db,
    }
}

/// Evaluates the player's live microphone voice signal at the physical Shed condenser microphone
/// stand (`mic_pos_m`), combining real `mic.rms_db` calibration with physical distance & voxel
/// occlusion (`compute_direct_occlusion`) between the speaker's mouth and the stand mic!
pub fn evaluate_mic_sensor_voice(
    world: &VoxelWorld,
    speaker_pos_m: Vec3,
    mic_pos_m: Vec3,
    mic: &MicTelemetry,
    voice: &VoiceSettings,
) -> MicSensorReading {
    if !mic.available || voice.muted {
        return MicSensorReading::default();
    }

    let distance_m = speaker_pos_m.distance(mic_pos_m);
    if distance_m > 32.0 {
        return MicSensorReading::default();
    }

    let occ = compute_direct_occlusion(world, speaker_pos_m, mic_pos_m, 2.0, 55.0);
    let avg_gain = (occ.direct_gain.0[0] + occ.direct_gain.0[1] + occ.direct_gain.0[2]) / 3.0;
    let propagation_db = linear_to_db(avg_gain.max(1e-4));
    let meter_db = (mic.rms_db + propagation_db).clamp(DB_MIN, DB_MAX);

    let (stalker_salience, lurker_salience, bands_norm) = if mic.gate_open {
        let e = mic.ai_noise_level.max(0.05);
        let emitted = Bands3::new(e * 0.7, e * 1.4, e * 1.6);
        let received = emitted.mul(occ.direct_gain);
        let sw = received.mul(STALKER_SENSITIVITY);
        let lw = received.mul(LURKER_SENSITIVITY);
        let norm_db = ((meter_db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0);
        (
            sw.0[0] + sw.0[1] + sw.0[2],
            lw.0[0] + lw.0[1] + lw.0[2],
            [
                (norm_db * 0.55 * occ.direct_gain.0[0].clamp(0.1, 1.0)).clamp(0.0, 1.0),
                (norm_db * 0.95 * occ.direct_gain.0[1].clamp(0.1, 1.0)).clamp(0.0, 1.0),
                (norm_db * 1.05 * occ.direct_gain.0[2].clamp(0.1, 1.0)).clamp(0.0, 1.0),
            ],
        )
    } else {
        // Below noise gate: still show subtle calibration meter movement so the player can
        // tune their gate threshold visually right in front of the mic stand!
        let norm_db = ((meter_db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0);
        (0.0, 0.0, [norm_db * 0.25, norm_db * 0.45, norm_db * 0.45])
    };

    MicSensorReading {
        meter_db,
        bands_norm,
        stalker_salience,
        lurker_salience,
        source_label: if mic.gate_open {
            "VOICE"
        } else {
            "VOICE (GATED)"
        },
        distance_m,
        occluded_db: occ.transmission_loss_db.0[1].max(0.0),
    }
}

/// Spawns the diegetic `2.60 m x 1.65 m` steel-framed LED Acoustic Calibration Wall mounted on
/// the Main Shed's inner `-X` (West) wall behind the elevator (`facing +X`), the standing physical
/// Studio Condenser Microphone Stand directly in front of it (`info.decibel_mic_pos_m`), and
/// isolated child `PointLight` entities for the Acoustic Bay & South Wooden Gear Storage Counter
/// (ZERO `PointLight` sharing an entity with `Mesh3d`).
pub fn spawn_decibel_wall(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    info: &GeneratedLevelInfo,
    telemetry: &AcousticTelemetry,
    voice_settings: &VoiceSettings,
) {
    let sensor = DecibelSensorState::default();
    let pixels = render_decibel_wall_rgba(&sensor, &telemetry.mic, voice_settings);
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

    let sig = compute_wall_signature(&sensor, &telemetry.mic, voice_settings);
    commands.insert_resource(DecibelWallTexture {
        image: image_handle.clone(),
        accum_s: 0.0,
        last_signature: sig,
        sensor,
    });

    // 1. Acoustic Calibration Wall (2.60 m x 1.65 m bezel + 384x240 LED screen) mounted flush
    //    on the inner West (-X) wall behind the elevator, rotated +90 deg around Y so local +Z
    //    faces +X (upright & un-mirrored when viewed from the Acoustic Bay!).
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
    let brushed_steel_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.46, 0.50, 0.54),
        metallic: 0.85,
        perceptual_roughness: 0.32,
        ..default()
    });
    let rubber_cable_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.08, 0.09, 0.11),
        metallic: 0.10,
        perceptual_roughness: 0.85,
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
    let mic_led_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.18, 0.90, 0.86),
        emissive: LinearRgba::new(0.35, 2.6, 2.4, 1.0),
        perceptual_roughness: 0.20,
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
            Transform::from_translation(info.decibel_wall_pos_m)
                .with_rotation(Quat::from_rotation_y(FRAC_PI_2)),
            Visibility::default(),
            ShedLandmarkVisual,
        ))
        .with_children(|parent| {
            // Industrial dark steel backing bezel flush against the -X West wall
            parent.spawn((
                Mesh3d(frame_mesh),
                MeshMaterial3d(frame_mat.clone()),
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
            // Dynamic LED calibration quad facing +X (upright, un-mirrored)
            parent.spawn((
                Mesh3d(screen_mesh),
                MeshMaterial3d(screen_mat),
                Transform::from_xyz(0.0, 0.0, 0.082),
            ));
            // Dedicated child PointLight illuminating the Acoustic Calibration Bay (NO Mesh3d)
            parent.spawn((
                PointLight {
                    color: Color::srgb(0.92, 0.88, 0.72),
                    intensity: 26_000.0,
                    range: 11.5,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, 0.35, 1.35),
            ));
        });

    // 2. Physical Studio Condenser Microphone Stand directly in front of the Decibel Wall!
    //    Its capsule sits at `info.decibel_mic_pos_m` and a floor XLR cable runs back into the
    //    board so players visually see that THIS microphone feeds the Acoustic Calibration Wall.
    let mic_base_pos = Vec3::new(
        info.decibel_mic_pos_m.x,
        info.shed_floor_y_m,
        info.decibel_mic_pos_m.z,
    );
    let capsule_local_y = (info.decibel_mic_pos_m.y - info.shed_floor_y_m).clamp(1.15, 1.75);
    let pole_h = (capsule_local_y - 0.18).max(0.8);
    let cable_len = (info.decibel_mic_pos_m.x - info.decibel_wall_pos_m.x).max(0.4);

    let mic_base_mesh = meshes.add(Cylinder::new(0.24, 0.05));
    let mic_pole_mesh = meshes.add(Cylinder::new(0.022, pole_h));
    let mic_mount_mesh = meshes.add(Cuboid::new(0.09, 0.07, 0.09));
    let mic_body_mesh = meshes.add(Cylinder::new(0.040, 0.14));
    let mic_led_mesh = meshes.add(Cylinder::new(0.044, 0.024));
    let mic_grille_mesh = meshes.add(Sphere::new(0.050));
    let cable_mesh = meshes.add(Cuboid::new(cable_len, 0.022, 0.042));

    commands
        .spawn((
            Transform::from_translation(mic_base_pos),
            Visibility::default(),
            ShedLandmarkVisual,
        ))
        .with_children(|parent| {
            // Weighted steel floor base
            parent.spawn((
                Mesh3d(mic_base_mesh),
                MeshMaterial3d(frame_mat.clone()),
                Transform::from_xyz(0.0, 0.025, 0.0),
            ));
            // Telescoping upright brushed steel stand
            parent.spawn((
                Mesh3d(mic_pole_mesh),
                MeshMaterial3d(brushed_steel_mat.clone()),
                Transform::from_xyz(0.0, 0.05 + pole_h * 0.5, 0.0),
            ));
            // Shock-mount cradle block
            parent.spawn((
                Mesh3d(mic_mount_mesh),
                MeshMaterial3d(frame_mat.clone()),
                Transform::from_xyz(0.0, capsule_local_y - 0.12, 0.0),
            ));
            // Condenser microphone cylindrical housing
            parent.spawn((
                Mesh3d(mic_body_mesh),
                MeshMaterial3d(frame_mat),
                Transform::from_xyz(0.0, capsule_local_y - 0.05, 0.0),
            ));
            // Live reactive emissive status LED collar around the microphone body
            parent.spawn((
                Mesh3d(mic_led_mesh),
                MeshMaterial3d(mic_led_mat),
                Transform::from_xyz(0.0, capsule_local_y - 0.01, 0.0),
                DecibelMicLed,
            ));
            // Metallic studio condenser capsule mesh grille (top at `info.decibel_mic_pos_m`)
            parent.spawn((
                Mesh3d(mic_grille_mesh),
                MeshMaterial3d(brushed_steel_mat),
                Transform::from_xyz(0.0, capsule_local_y + 0.035, 0.0),
            ));
            // Insulated floor XLR audio cable running straight back (-X) into the Decibel Wall!
            parent.spawn((
                Mesh3d(cable_mesh),
                MeshMaterial3d(rubber_cable_mat),
                Transform::from_xyz(-cable_len * 0.5, 0.011, 0.0),
            ));
        });

    // 3. Cozy dedicated overhead worklight illuminating the ergonomic South (+Z) Wooden Gear
    //    Storage Counter & Workbench (NO Mesh3d — avoids tiny-AABB frustum culling!).
    let counter_light_pos = Vec3::new(
        info.shed_workbench_pos_m.x,
        info.shed_floor_y_m + 2.55,
        info.shed_workbench_pos_m.z - 0.70,
    );
    commands
        .spawn((
            Transform::from_translation(counter_light_pos),
            Visibility::default(),
            ShedLandmarkVisual,
        ))
        .with_children(|parent| {
            parent.spawn((
                PointLight {
                    color: Color::srgb(1.0, 0.78, 0.44),
                    intensity: 26_000.0,
                    range: 10.5,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::IDENTITY,
            ));
        });
}

/// Samples ALL noises in the Base Shed (voice chat, co-op voice, footsteps, item impacts, flares,
/// foam sprays, pickaxe strikes, decoys, elevator machinery) physically at the standing condenser
/// microphone capsule (`info.decibel_mic_pos_m`), updates the live microphone LED ring color, and
/// refreshes the 3D Acoustic Calibration Wall texture at ~15 Hz while the player is in the Shed.
///
/// Performs zero work while underground in the cave or on menus.
#[allow(clippy::too_many_arguments)]
pub fn update_decibel_wall_system(
    time: Res<Time>,
    screen: Res<AppScreen>,
    level_info: Option<Res<LevelInfoResource>>,
    world_res: Res<WorldResource>,
    wall: Option<ResMut<DecibelWallTexture>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    telemetry: Res<AcousticTelemetry>,
    voice_settings: Res<VoiceSettings>,
    coop: Res<CoopState>,
    mut sound_events: MessageReader<PlaySpatialSoundEvent>,
    mut incoming_voice: MessageReader<IncomingVoiceFrame>,
    player_q: Query<(&PlayerController, &Transform)>,
    mic_led_q: Query<&MeshMaterial3d<StandardMaterial>, With<DecibelMicLed>>,
) {
    if *screen != AppScreen::InGame {
        sound_events.clear();
        incoming_voice.clear();
        return;
    }
    let (Some(level_info), Some(mut wall)) = (level_info, wall) else {
        sound_events.clear();
        incoming_voice.clear();
        return;
    };
    let Ok((player, player_tf)) = player_q.single() else {
        sound_events.clear();
        incoming_voice.clear();
        return;
    };
    if !is_pos_in_shed(player.feet_pos_m, &level_info.0) {
        sound_events.clear();
        incoming_voice.clear();
        return;
    }

    let dt = time.delta_secs().min(0.05);
    let mic_pos = level_info.0.decibel_mic_pos_m;
    wall.sensor.decay(dt);

    // 1. Sample every spatial game sound event emitted this frame at the condenser microphone stand!
    let world = world_res.read();
    for ev in sound_events.read() {
        let acoustic_ev = ev.ai_event.unwrap_or_else(|| AcousticEvent {
            pos_m: ev.pos_m,
            energy: Bands3::splat((ev.volume * 0.90).clamp(0.05, 3.0)),
            sharpness: 0.65,
            kind: AcousticEventKind::ItemImpact,
        });
        let reading = evaluate_mic_sensor_event(&world, acoustic_ev, mic_pos);
        wall.sensor.ingest(reading);
    }

    // 2. Sample remote co-op teammates speaking in the Shed at the condenser microphone stand!
    for pkt in incoming_voice.read() {
        if let Some(rp) = coop.remote_players.get(&pkt.speaker_id) {
            let head = remote_player_head_pos(rp);
            let mut reading = evaluate_mic_sensor_event(
                &world,
                AcousticEvent {
                    pos_m: head,
                    energy: Bands3::new(0.55, 1.15, 1.35),
                    sharpness: 0.55,
                    kind: AcousticEventKind::VoiceSpeech,
                },
                mic_pos,
            );
            reading.source_label = "CO-OP VOICE";
            wall.sensor.ingest(reading);
        }
    }

    // 3. Sample the local player's live microphone voice from their mouth position to the stand mic!
    let local_mouth_pos = player_tf.translation;
    let voice_reading = evaluate_mic_sensor_voice(
        &world,
        local_mouth_pos,
        mic_pos,
        &telemetry.mic,
        &voice_settings,
    );
    wall.sensor.ingest(voice_reading);
    drop(world);

    // 4. Update the glowing status LED collar on the 3D Condenser Microphone Stand
    if let Ok(mat_handle) = mic_led_q.single()
        && let Some(mut mat) = materials.get_mut(&mat_handle.0)
    {
        let (base_col, emissive_col) = if wall.sensor.stalker_salience >= STALKER_HUNT_THRESHOLD {
            (
                Color::srgb(0.98, 0.22, 0.22),
                LinearRgba::new(5.2, 0.65, 0.65, 1.0),
            )
        } else if wall.sensor.stalker_salience >= STALKER_HEAR_THRESHOLD
            || wall.sensor.lurker_salience >= LURKER_HEAR_THRESHOLD
        {
            (
                Color::srgb(0.98, 0.72, 0.16),
                LinearRgba::new(4.2, 2.5, 0.30, 1.0),
            )
        } else if wall.sensor.current_db > DB_MIN + 5.0 {
            (
                Color::srgb(0.20, 0.94, 0.52),
                LinearRgba::new(0.55, 3.8, 1.6, 1.0),
            )
        } else {
            (
                Color::srgb(0.18, 0.88, 0.84),
                LinearRgba::new(0.30, 2.0, 1.9, 1.0),
            )
        };
        mat.base_color = base_col;
        mat.emissive = emissive_col;
    }

    // 5. Re-render the 384x240 LED wall bitmap at ~15 Hz only when visual state changed
    wall.accum_s += dt;
    if wall.accum_s < UPDATE_INTERVAL_S {
        return;
    }
    wall.accum_s %= UPDATE_INTERVAL_S;

    let sig = compute_wall_signature(&wall.sensor, &telemetry.mic, &voice_settings);
    if sig == wall.last_signature {
        return;
    }
    wall.last_signature = sig;

    if let Some(mut img) = images.get_mut(&wall.image) {
        img.data = Some(render_decibel_wall_rgba(
            &wall.sensor,
            &telemetry.mic,
            &voice_settings,
        ));
    }
}

fn compute_wall_signature(
    sensor: &DecibelSensorState,
    mic: &MicTelemetry,
    voice: &VoiceSettings,
) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ((sensor.current_db * 2.0).round() as i32).hash(&mut h);
    ((sensor.peak_db * 2.0).round() as i32).hash(&mut h);
    for b in sensor.bands_norm {
        ((b * 12.0).round() as i32).hash(&mut h);
    }
    let alert_tier: u8 = if sensor.stalker_salience >= STALKER_HUNT_THRESHOLD {
        3
    } else if sensor.stalker_salience >= STALKER_HEAR_THRESHOLD {
        2
    } else if sensor.lurker_salience >= LURKER_HEAR_THRESHOLD {
        1
    } else {
        0
    };
    alert_tier.hash(&mut h);
    sensor.last_source_label.hash(&mut h);
    ((sensor.last_distance_m * 2.0).round() as i32).hash(&mut h);
    (sensor.last_occluded_db.round() as i32).hash(&mut h);
    mic.available.hash(&mut h);
    mic.gate_open.hash(&mut h);
    voice.muted.hash(&mut h);
    voice.self_echo_test.hash(&mut h);
    ((voice.gate_threshold_db.clamp(-65.0, -15.0) * 2.0).round() as i32).hash(&mut h);
    h.finish()
}

/// Renders the `384x240` RGBA8 LED Acoustic Calibration Wall display driven by the physical
/// condenser microphone stand (`sensor`) plus voice calibration indicators (`mic`, `voice`).
pub fn render_decibel_wall_rgba(
    sensor: &DecibelSensorState,
    mic: &MicTelemetry,
    voice: &VoiceSettings,
) -> Vec<u8> {
    let mut fb = Canvas::new(WALL_WIDTH_PX, WALL_HEIGHT_PX, [10, 14, 22, 255]);

    // Outer subtle industrial bezel inner border
    fb.stroke_rect(4, 4, WALL_WIDTH_PX - 8, WALL_HEIGHT_PX - 8, 2, [42, 62, 82, 255]);

    // Top Header Band
    fb.fill_rect(8, 8, WALL_WIDTH_PX - 16, 32, [16, 26, 38, 255]);
    fb.fill_rect(8, 40, WALL_WIDTH_PX - 16, 2, [242, 174, 52, 255]);
    fb.draw_text_centered("ACOUSTIC SENSOR WALL", 17, 2, [250, 206, 86, 255]);

    // Meter Panel Container
    fb.fill_rect(14, 48, WALL_WIDTH_PX - 28, 76, [13, 20, 30, 255]);
    fb.stroke_rect(14, 48, WALL_WIDTH_PX - 28, 76, 1, [38, 62, 84, 255]);

    // Sub-header above meter: live source + distance on left, live dB + peak on right
    let src_header = if sensor.last_source_label != "AMBIENT" && sensor.last_distance_m > 0.05 {
        if sensor.last_occluded_db >= 2.0 {
            format!(
                "MIC: {} ({:.1}M, -{:.0}DB)",
                sensor.last_source_label, sensor.last_distance_m, sensor.last_occluded_db
            )
        } else {
            format!(
                "MIC: {} ({:.1}M)",
                sensor.last_source_label, sensor.last_distance_m
            )
        }
    } else {
        "MIC STAND FEED (ALL NOISE)".to_string()
    };
    fb.draw_text(&src_header, 22, 54, 1, [142, 198, 220, 255]);

    let db_display = format!(
        "{:.0} DB (PK {:.0})",
        sensor.current_db.clamp(DB_MIN, DB_MAX),
        sensor.peak_db.clamp(DB_MIN, DB_MAX)
    );
    let db_w = text_width_px(&db_display, 1);
    fb.draw_text(
        &db_display,
        WALL_WIDTH_PX - 22 - db_w,
        54,
        1,
        [226, 242, 252, 255],
    );

    // 24 Segmented LED Bars across -65 dB .. -5 dB
    let norm_level = ((sensor.current_db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0);
    let lit_bars = if sensor.current_db <= DB_MIN + 0.5 {
        0
    } else {
        (norm_level * LED_SEGMENTS as f32).round() as usize
    };

    let peak_norm = ((sensor.peak_db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0);
    let peak_idx = if sensor.peak_db <= DB_MIN + 1.0 {
        None
    } else {
        Some(
            ((peak_norm * LED_SEGMENTS as f32).round() as usize)
                .saturating_sub(1)
                .min(LED_SEGMENTS - 1),
        )
    };

    let seg_w = 12u32;
    let seg_gap = 2u32;
    let meter_w = LED_SEGMENTS as u32 * seg_w + (LED_SEGMENTS as u32 - 1) * seg_gap; // 334 px
    let meter_x = (WALL_WIDTH_PX - meter_w) / 2; // 25 px
    let meter_y = 68u32;
    let meter_h = 24u32;

    for i in 0..LED_SEGMENTS {
        let sx = meter_x + i as u32 * (seg_w + seg_gap);
        let is_lit = i < lit_bars;
        let is_peak = peak_idx == Some(i) && !is_lit;
        let col = if is_lit {
            if i >= 19 || sensor.stalker_salience >= STALKER_HUNT_THRESHOLD {
                [246, 74, 74, 255]
            } else if i >= 14
                || sensor.stalker_salience >= STALKER_HEAR_THRESHOLD
                || sensor.lurker_salience >= LURKER_HEAR_THRESHOLD
            {
                [248, 182, 54, 255]
            } else {
                [56, 224, 130, 255]
            }
        } else if is_peak {
            [250, 240, 152, 255]
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
    fb.fill_rect(
        gate_x.saturating_sub(1),
        meter_y - 3,
        4,
        meter_h + 6,
        [44, 210, 228, 255],
    );
    fb.fill_rect(gate_x, meter_y - 4, 2, meter_h + 8, [242, 252, 255, 255]);

    // 3-Band Live Spectral Bar Strip right beneath the main dB meter (LOW / MID / HIGH)
    let band_specs: [(&str, f32, u32, [u8; 4]); 3] = [
        ("LOW", sensor.bands_norm[0], 22, [248, 148, 62, 255]),
        ("MID", sensor.bands_norm[1], 138, [72, 224, 156, 255]),
        ("HI", sensor.bands_norm[2], 254, [82, 204, 248, 255]),
    ];
    for (label, norm, bx, col) in band_specs {
        fb.draw_text(label, bx, 102, 1, [142, 168, 190, 255]);
        let bar_x = bx + 24;
        let bar_w = 82u32;
        fb.fill_rect(bar_x, 102, bar_w, 7, [20, 30, 42, 255]);
        let fill_w = (norm.clamp(0.0, 1.0) * bar_w as f32).round() as u32;
        if fill_w > 0 {
            fb.fill_rect(bar_x, 102, fill_w, 7, col);
        }
    }

    // Scale dB ticks along bottom of meter box
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
        fb.draw_text(lbl, tx, 113, 1, [104, 134, 158, 255]);
    }

    // Live Predator Audibility Readout Card
    fb.fill_rect(14, 132, WALL_WIDTH_PX - 28, 40, [14, 22, 34, 255]);
    let db_int = sensor.current_db.clamp(DB_MIN, DB_MAX).round() as i32;
    let (status_str, status_col, border_col) = if sensor.stalker_salience >= STALKER_HUNT_THRESHOLD
    {
        (
            format!("STALKER HUNT ALERT! • {db_int} DB"),
            [252, 82, 82, 255],
            [168, 44, 48, 255],
        )
    } else if sensor.stalker_salience >= STALKER_HEAR_THRESHOLD {
        (
            format!("AUDIBLE TO STALKERS • {db_int} DB"),
            [250, 186, 56, 255],
            [154, 110, 36, 255],
        )
    } else if sensor.lurker_salience >= LURKER_HEAR_THRESHOLD {
        (
            format!("LOW RUMBLE (LURKER!) • {db_int} DB"),
            [248, 146, 62, 255],
            [148, 84, 34, 255],
        )
    } else if sensor.current_db > DB_MIN + 5.0 {
        (
            format!("WHISPER SAFE • {db_int} DB"),
            [64, 236, 144, 255],
            [40, 132, 84, 255],
        )
    } else {
        (
            format!("QUIET ROOM • {db_int} DB"),
            [156, 178, 200, 255],
            [52, 76, 98, 255],
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

    // Bottom Voice Calibration & Shortcut Status Bar
    fb.fill_rect(14, 180, WALL_WIDTH_PX - 28, 46, [12, 19, 29, 255]);
    fb.stroke_rect(14, 180, WALL_WIDTH_PX - 28, 46, 1, [38, 62, 82, 255]);

    let (mic_str, mic_col) = if !mic.available {
        ("VOICE MIC: NONE", [246, 148, 64, 255])
    } else if voice.muted {
        ("VOICE: MUTED [M]", [248, 92, 92, 255])
    } else if mic.gate_open {
        ("VOICE: LIVE", [64, 236, 144, 255])
    } else {
        ("VOICE: GATED", [150, 178, 202, 255])
    };
    fb.draw_text(mic_str, 22, 188, 1, mic_col);

    let (echo_str, echo_col) = if voice.self_echo_test {
        ("ECHO [V]: ON", [64, 232, 220, 255])
    } else {
        ("ECHO [V]: OFF", [140, 164, 186, 255])
    };
    fb.draw_text(echo_str, 152, 188, 1, echo_col);

    let gate_int = voice.gate_threshold_db.clamp(-65.0, -15.0).round() as i32;
    let gate_str = format!("GATE: {gate_int} DB");
    let gate_w = text_width_px(&gate_str, 1);
    fb.draw_text(
        &gate_str,
        WALL_WIDTH_PX - 22 - gate_w,
        188,
        1,
        [242, 196, 84, 255],
    );

    fb.draw_text_centered(
        "SPEAK, WALK, DROP ITEMS OR FOAM BY THE MIC STAND",
        207,
        1,
        [104, 146, 174, 255],
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
        ',' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100, 0b01000],
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
    use glam::IVec3;
    use voxel_core::{VoxelConfig, VoxelId, generate_world_with_tier};

    use super::*;

    #[test]
    fn decibel_wall_rgba_has_exact_dimensions_and_distinguishes_noise_states() {
        let voice = VoiceSettings::default();
        let mic = MicTelemetry {
            rms_db: -60.0,
            gate_open: false,
            ai_noise_level: 0.0,
            available: true,
        };

        let quiet_sensor = DecibelSensorState::default();
        let mut footstep_sensor = DecibelSensorState::default();
        footstep_sensor.ingest(MicSensorReading {
            meter_db: -34.0,
            bands_norm: [0.35, 0.45, 0.30],
            stalker_salience: 0.25,
            lurker_salience: 0.12,
            source_label: "FOOTSTEP",
            distance_m: 1.5,
            occluded_db: 0.0,
        });
        let mut alert_sensor = DecibelSensorState::default();
        alert_sensor.ingest(MicSensorReading {
            meter_db: -12.0,
            bands_norm: [0.75, 0.90, 0.95],
            stalker_salience: 1.65,
            lurker_salience: 0.80,
            source_label: "ITEM IMPACT",
            distance_m: 1.2,
            occluded_db: 0.0,
        });

        let quiet_buf = render_decibel_wall_rgba(&quiet_sensor, &mic, &voice);
        let step_buf = render_decibel_wall_rgba(&footstep_sensor, &mic, &voice);
        let alert_buf = render_decibel_wall_rgba(&alert_sensor, &mic, &voice);

        let expected_len = (WALL_WIDTH_PX * WALL_HEIGHT_PX * 4) as usize;
        assert_eq!(quiet_buf.len(), expected_len);
        assert_eq!(step_buf.len(), expected_len);
        assert_eq!(alert_buf.len(), expected_len);

        assert_ne!(quiet_buf, step_buf, "quiet vs footstep must render differently");
        assert_ne!(step_buf, alert_buf, "footstep vs alert must render differently");
        assert_ne!(quiet_buf, alert_buf, "quiet vs alert must render differently");
    }

    #[test]
    fn mic_sensor_measures_world_noises_and_muffles_behind_acoustic_foam() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        let (mut world, info) = generate_world_with_tier(4242, 1, cfg);
        let mic_pos = info.decibel_mic_pos_m;
        let source_pos = mic_pos + Vec3::new(2.2, 0.0, 0.0);

        let impact = AcousticEvent {
            pos_m: source_pos,
            energy: Bands3::new(1.8, 2.2, 2.5),
            sharpness: 0.85,
            kind: AcousticEventKind::ItemImpact,
        };

        let clear_reading = evaluate_mic_sensor_event(&world, impact, mic_pos);
        assert_eq!(clear_reading.source_label, "ITEM IMPACT");
        assert!(
            clear_reading.meter_db > -25.0 && clear_reading.stalker_salience > STALKER_HUNT_THRESHOLD,
            "close unoccluded impact should spike mic sensor loudly: {clear_reading:?}"
        );

        // Place a 2-voxel-thick AcousticFoam barrier directly between the noise source and the mic stand!
        let mid_vox = cfg.world_to_voxel(mic_pos.lerp(source_pos, 0.5));
        for dy in -2..=2 {
            for dz in -2..=2 {
                world.set(mid_vox + IVec3::new(0, dy, dz), VoxelId::AcousticFoam);
                world.set(mid_vox + IVec3::new(1, dy, dz), VoxelId::AcousticFoam);
            }
        }

        let muffled_reading = evaluate_mic_sensor_event(&world, impact, mic_pos);
        assert!(
            muffled_reading.meter_db < clear_reading.meter_db - 6.0,
            "acoustic foam between source and mic stand must noticeably lower dB: clear={} muffled={}",
            clear_reading.meter_db,
            muffled_reading.meter_db
        );
        assert!(
            muffled_reading.occluded_db > 5.0,
            "muffled reading should report physical occlusion dB loss: {muffled_reading:?}"
        );
    }

    #[test]
    fn spawn_decibel_wall_creates_mic_stand_and_isolates_point_lights() {
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

        let mut led_q = world.query_filtered::<Entity, With<DecibelMicLed>>();
        assert_eq!(
            led_q.iter(&world).count(),
            1,
            "expected physical condenser microphone stand with 1 live status LED ring"
        );

        let mut light_q = world.query::<(&PointLight, Option<&Mesh3d>)>();
        let lights: Vec<_> = light_q.iter(&world).collect();
        assert_eq!(
            lights.len(),
            2,
            "expected Acoustic Bay light + ergonomic South gear counter light"
        );
        for (_, mesh) in lights {
            assert!(
                mesh.is_none(),
                "Decibel wall and gear counter PointLights must NOT share an entity with Mesh3d"
            );
        }
    }
}
