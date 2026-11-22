use std::sync::Arc;

use bevy::prelude::*;
use voxel_acoustics::{
    AcousticEvent, AcousticSourceQuery, AcousticWorkRequest, AcousticWorker, PathTraceConfig,
    ReverbEstimate, SourceAcousticParams, solve_single_source,
};
use voxel_audio_dsp::{
    AudioCommand, AudioEngine, LOCAL_LOOPBACK_SPEAKER_ID, MicCapture, MicTelemetry, SynthClip,
    VoiceDecoderPool, list_input_devices, list_output_devices,
};
use voxel_net::PlayerId;

use crate::{
    net_bridge::{CoopMenuOpen, CoopState, remote_player_head_pos},
    player::PlayerController,
    settings::{AppScreen, SettingsMenuOpen, UserSettings},
    shed::ShopOverlayOpen,
    voxel_render::{LevelInfoResource, WorldResource},
};

const BEACON_SOURCE_ID: u64 = 1;
const DRIP_SOURCE_ID: u64 = 2;
const VOICE_SOURCE_BASE_ID: u64 = 10_000;

/// One-shot spatial sound trigger message (also carries an `AcousticEvent` for Week-3 blind AI).
#[derive(Message, Clone, Debug)]
pub struct PlaySpatialSoundEvent {
    pub pos_m: Vec3,
    pub clip: SynthClip,
    pub volume: f32,
    pub pitch: f32,
    #[allow(dead_code)]
    pub ai_event: Option<AcousticEvent>,
}

/// Incoming Opus VoIP packet from a remote player to decode & feed into the acoustic mixer.
#[derive(Message, Clone, Debug)]
pub struct IncomingVoiceFrame {
    pub speaker_id: PlayerId,
    pub seq: u16,
    pub opus_payload: Vec<u8>,
}

/// Notification to clean up a disconnected remote speaker's voice slot.
#[derive(Message, Clone, Debug)]
pub struct RemoveVoiceSpeakerEvent {
    pub speaker_id: PlayerId,
}

/// Cached list of system audio output and input devices for the Settings UI.
#[derive(Resource, Clone, Debug, Default)]
pub struct AudioDeviceCatalog {
    pub output_devices: Vec<String>,
    pub input_devices: Vec<String>,
}

impl AudioDeviceCatalog {
    pub fn refresh(&mut self) {
        self.output_devices = list_output_devices();
        self.input_devices = list_input_devices();
    }
}

/// User-adjustable live voice controls (mute & self-echo test).
/// Persistent levels/devices live in `UserSettings`.
#[derive(Resource, Clone, Debug)]
pub struct VoiceSettings {
    pub muted: bool,
    pub self_echo_test: bool,
    pub gate_threshold_db: f32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            muted: false,
            self_echo_test: false,
            gate_threshold_db: -45.0,
        }
    }
}

/// Live telemetry resource displayed on the HUD so we can verify acoustic & mic physics in real time.
#[derive(Resource, Clone, Debug)]
pub struct AcousticTelemetry {
    pub reverb: ReverbEstimate,
    pub beacon_params: Option<SourceAcousticParams>,
    pub drip_params: Option<SourceAcousticParams>,
    pub mic: MicTelemetry,
}

impl Default for AcousticTelemetry {
    fn default() -> Self {
        Self {
            reverb: ReverbEstimate::default(),
            beacon_params: None,
            drip_params: None,
            mic: MicTelemetry {
                rms_db: -90.0,
                gate_open: false,
                ai_noise_level: 0.0,
                available: false,
            },
        }
    }
}

#[derive(Resource)]
pub struct AcousticBeaconState {
    pub pos_m: Vec3,
    /// Dev acoustic test beacon is OFF by default so it never ticks continuously during
    /// normal gameplay or up in the Base Shed; press `[K]` to toggle it ON/OFF for A/B tests.
    pub enabled: bool,
    pub ping_timer: Timer,
    pub drip_timer: Timer,
    pub solve_timer: Timer,
    pub frame_counter: u64,
    pub next_oneshot_id: u64,
}

#[derive(Resource)]
struct AcousticWorkerHolder(AcousticWorker);

#[derive(Component)]
struct BeaconVisual;

/// Non-Send resource holding real-time `cpal` output/input streams + Opus decoder pool.
struct AudioEngineHolder {
    engine: AudioEngine,
    mic: MicCapture,
    decoder_pool: VoiceDecoderPool,
    active_output_device: Option<String>,
    active_input_device: Option<String>,
    last_master_volume: f32,
    last_voice_volume: f32,
}

pub struct AudioBridgePlugin;

impl Plugin for AudioBridgePlugin {
    fn build(&self, app: &mut App) {
        let initial_settings = app
            .world()
            .get_resource::<UserSettings>()
            .cloned()
            .unwrap_or_default();

        let mut engine = AudioEngine::start_with_device(initial_settings.output_device.as_deref());
        engine.send(AudioCommand::SetVolumes {
            master: initial_settings.master_volume,
            voice: initial_settings.voice_volume,
        });
        let decoder_pool = VoiceDecoderPool::new(engine.sample_rate);
        let mic = MicCapture::start_with_device(
            initial_settings.gate_threshold_db,
            initial_settings.input_device.as_deref(),
        );
        mic.set_input_gain(initial_settings.mic_gain);

        let mut catalog = AudioDeviceCatalog::default();
        catalog.refresh();

        app.add_message::<PlaySpatialSoundEvent>()
            .add_message::<IncomingVoiceFrame>()
            .add_message::<RemoveVoiceSpeakerEvent>()
            .insert_resource(VoiceSettings {
                muted: false,
                self_echo_test: false,
                gate_threshold_db: initial_settings.gate_threshold_db,
            })
            .insert_resource(catalog)
            .init_resource::<AcousticTelemetry>()
            .insert_non_send(AudioEngineHolder {
                engine,
                mic,
                decoder_pool,
                active_output_device: initial_settings.output_device,
                active_input_device: initial_settings.input_device,
                last_master_volume: initial_settings.master_volume,
                last_voice_volume: initial_settings.voice_volume,
            })
            .add_systems(Startup, setup_beacon_and_acoustics)
            .add_systems(
                Update,
                (
                    voice_hotkeys_system,
                    sync_audio_user_settings_system,
                    sync_beacon_visual,
                    submit_and_poll_acoustics,
                    process_mic_and_voice_frames,
                    process_spatial_sound_events,
                )
                    .chain(),
            );
    }
}

fn setup_beacon_and_acoustics(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    world_res: Res<WorldResource>,
    level_info: Res<LevelInfoResource>,
) {
    // Spawn off-thread 20 Hz acoustic raytracer reading shared Arc<RwLock<VoxelWorld>>
    commands.insert_resource(AcousticWorkerHolder(AcousticWorker::spawn(Arc::clone(
        &world_res.0,
    ))));

    let beacon_pos = level_info.0.acoustic_beacon_pos_m;
    commands.insert_resource(AcousticBeaconState {
        pos_m: beacon_pos,
        enabled: false,
        ping_timer: Timer::from_seconds(1.5, TimerMode::Repeating),
        drip_timer: Timer::from_seconds(4.2, TimerMode::Repeating),
        solve_timer: Timer::from_seconds(1.0 / 20.0, TimerMode::Repeating),
        frame_counter: 0,
        next_oneshot_id: 100,
    });

    // Visual marker for the optional Acoustic Test Beacon (hidden until toggled with `[K]`)
    commands
        .spawn((
            Mesh3d(meshes.add(Sphere::new(0.22))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.2, 0.95, 0.9),
                emissive: LinearRgba::new(1.2, 8.0, 7.5, 1.0),
                ..default()
            })),
            Transform::from_translation(beacon_pos),
            Visibility::Hidden,
            BeaconVisual,
        ))
        .with_children(|parent| {
            parent.spawn((
                PointLight {
                    color: Color::srgb(0.2, 0.95, 0.9),
                    intensity: 24_000.0,
                    range: 12.0,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::default(),
            ));
        });
}

fn voice_hotkeys_system(
    keys: Res<ButtonInput<KeyCode>>,
    screen: Res<AppScreen>,
    menu_open: Res<CoopMenuOpen>,
    shop_open: Res<ShopOverlayOpen>,
    settings_open: Res<SettingsMenuOpen>,
    mut settings: ResMut<VoiceSettings>,
    mut user_settings: ResMut<UserSettings>,
) {
    if *screen != AppScreen::InGame || menu_open.0 || shop_open.0 || settings_open.0 {
        return;
    }
    if keys.just_pressed(KeyCode::KeyM) {
        settings.muted = !settings.muted;
    }
    if keys.just_pressed(KeyCode::KeyV) {
        settings.self_echo_test = !settings.self_echo_test;
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        user_settings.gate_threshold_db =
            (user_settings.gate_threshold_db - 3.0).clamp(-65.0, -15.0);
        settings.gate_threshold_db = user_settings.gate_threshold_db;
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        user_settings.gate_threshold_db =
            (user_settings.gate_threshold_db + 3.0).clamp(-65.0, -15.0);
        settings.gate_threshold_db = user_settings.gate_threshold_db;
    }
}

fn sync_audio_user_settings_system(
    user_settings: Res<UserSettings>,
    mut voice_settings: ResMut<VoiceSettings>,
    mut audio: NonSendMut<AudioEngineHolder>,
) {
    voice_settings.gate_threshold_db = user_settings.gate_threshold_db;

    // Hot-swap output device if changed in Settings
    if user_settings.output_device != audio.active_output_device {
        let mut new_engine =
            AudioEngine::start_with_device(user_settings.output_device.as_deref());
        new_engine.send(AudioCommand::SetVolumes {
            master: user_settings.master_volume,
            voice: user_settings.voice_volume,
        });
        let new_pool = VoiceDecoderPool::new(new_engine.sample_rate);
        audio.engine = new_engine;
        audio.decoder_pool = new_pool;
        audio.active_output_device = user_settings.output_device.clone();
        audio.last_master_volume = user_settings.master_volume;
        audio.last_voice_volume = user_settings.voice_volume;
    }

    // Hot-swap input microphone device if changed in Settings
    if user_settings.input_device != audio.active_input_device {
        let new_mic = MicCapture::start_with_device(
            user_settings.gate_threshold_db,
            user_settings.input_device.as_deref(),
        );
        new_mic.set_input_gain(user_settings.mic_gain);
        new_mic.set_muted(voice_settings.muted);
        audio.mic = new_mic;
        audio.active_input_device = user_settings.input_device.clone();
    }

    // Push master & voice output volumes only when changed
    if (user_settings.master_volume - audio.last_master_volume).abs() > 1e-4
        || (user_settings.voice_volume - audio.last_voice_volume).abs() > 1e-4
    {
        audio.engine.send(AudioCommand::SetVolumes {
            master: user_settings.master_volume,
            voice: user_settings.voice_volume,
        });
        audio.last_master_volume = user_settings.master_volume;
        audio.last_voice_volume = user_settings.voice_volume;
    }

    // Sync mic input gain (atomic lock-free store)
    audio.mic.set_input_gain(user_settings.mic_gain);
}

fn sync_beacon_visual(
    beacon: Res<AcousticBeaconState>,
    mut vis_q: Query<(&mut Transform, &mut Visibility), With<BeaconVisual>>,
) {
    if let Ok((mut tf, mut vis)) = vis_q.single_mut() {
        tf.translation = beacon.pos_m;
        *vis = if beacon.enabled {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// Submits 20 Hz acoustic raytracing work to the background `AcousticWorker` and polls
/// completed snapshots without ever blocking Bevy's main render thread!
#[allow(clippy::too_many_arguments)]
fn submit_and_poll_acoustics(
    time: Res<Time>,
    screen: Res<AppScreen>,
    level_info: Res<LevelInfoResource>,
    coop: Res<CoopState>,
    settings: Res<VoiceSettings>,
    worker: Res<AcousticWorkerHolder>,
    player_q: Query<&Transform, With<PlayerController>>,
    mut beacon: ResMut<AcousticBeaconState>,
    mut telemetry: ResMut<AcousticTelemetry>,
    mut audio: NonSendMut<AudioEngineHolder>,
) {
    let Ok(cam_tf) = player_q.single() else {
        return;
    };

    beacon.solve_timer.tick(time.delta());
    beacon.ping_timer.tick(time.delta());
    beacon.drip_timer.tick(time.delta());

    let listener_pos = cam_tf.translation;
    let listener_fwd = cam_tf.forward().as_vec3();
    let listener_right = cam_tf.right().as_vec3();

    // 1. Submit non-blocking work request at 20 Hz (overwrites any unstarted pending request)
    if beacon.solve_timer.just_finished() {
        beacon.frame_counter = beacon.frame_counter.wrapping_add(1);
        let mut sources = Vec::with_capacity(2 + coop.remote_players.len());
        sources.push(AcousticSourceQuery {
            id: BEACON_SOURCE_ID,
            pos_m: beacon.pos_m,
            ref_distance_m: 2.0,
            max_distance_m: 65.0,
        });
        sources.push(AcousticSourceQuery {
            id: DRIP_SOURCE_ID,
            pos_m: level_info.0.grand_cavern_pos_m,
            ref_distance_m: 2.5,
            max_distance_m: 70.0,
        });

        for (&player_id, rp) in &coop.remote_players {
            sources.push(AcousticSourceQuery {
                id: VOICE_SOURCE_BASE_ID + player_id as u64,
                pos_m: remote_player_head_pos(rp),
                ref_distance_m: 2.2,
                max_distance_m: 55.0,
            });
        }

        worker.0.submit(AcousticWorkRequest {
            listener_pos_m: listener_pos,
            listener_forward: listener_fwd,
            listener_right,
            sources,
            cfg: PathTraceConfig::default(),
            frame_seed: beacon.frame_counter,
        });
    }

    // 2. Poll latest completed snapshot from background worker (completely non-blocking)
    if let Some(snap) = worker.0.try_take_snapshot() {
        telemetry.reverb = telemetry.reverb.lerp(snap.reverb, 0.25);

        audio.engine.send(AudioCommand::UpdateListener {
            pos_m: snap.listener_pos_m,
            right: snap.listener_right,
            reverb: telemetry.reverb,
        });

        for src in snap.sources {
            if src.id == BEACON_SOURCE_ID {
                telemetry.beacon_params = Some(src);
                audio.engine.send(AudioCommand::UpdateSource(src));
            } else if src.id == DRIP_SOURCE_ID {
                telemetry.drip_params = Some(src);
                audio.engine.send(AudioCommand::UpdateSource(src));
            } else if src.id >= VOICE_SOURCE_BASE_ID {
                let speaker_id = (src.id - VOICE_SOURCE_BASE_ID) as u16;
                audio.engine.update_voice_acoustics(speaker_id, src);
            } else {
                audio.engine.send(AudioCommand::UpdateSource(src));
            }
        }

        if settings.self_echo_test
            && let Some(beacon_params) = telemetry.beacon_params
        {
            audio
                .engine
                .update_voice_acoustics(LOCAL_LOOPBACK_SPEAKER_ID, beacon_params);
        }
    }

    // Keep the Main Menu quiet (no repeating beacon pings/drips until InGame)
    if *screen != AppScreen::InGame {
        return;
    }

    // 3. Trigger optional Acoustic Test Beacon Ping ONLY when explicitly toggled ON (`[K]`)
    if beacon.enabled
        && beacon.ping_timer.just_finished()
        && !settings.self_echo_test
        && let Some(params) = telemetry.beacon_params
    {
        audio.engine.send(AudioCommand::PlayClip {
            source_id: BEACON_SOURCE_ID,
            clip: SynthClip::BeaconPing,
            volume: 0.78,
            pitch: 1.0,
            looping: false,
            initial_params: params,
        });
    }

    // 4. Trigger subtle underground cavern water drips ONLY when below the surface Base Shed
    let is_underground = listener_pos.y < level_info.0.shed_floor_y_m - 4.0;
    if is_underground
        && beacon.drip_timer.just_finished()
        && let Some(params) = telemetry.drip_params
    {
        audio.engine.send(AudioCommand::PlayClip {
            source_id: DRIP_SOURCE_ID,
            clip: SynthClip::WaterDrip,
            volume: 0.50,
            pitch: 1.0,
            looping: false,
            initial_params: params,
        });
    }
}

fn process_mic_and_voice_frames(
    coop: Res<CoopState>,
    settings: Res<VoiceSettings>,
    mut telemetry: ResMut<AcousticTelemetry>,
    mut incoming_voice: MessageReader<IncomingVoiceFrame>,
    mut removed_speakers: MessageReader<RemoveVoiceSpeakerEvent>,
    mut audio: NonSendMut<AudioEngineHolder>,
) {
    // Sync user mic calibration settings to the capture thread
    audio.mic.set_muted(settings.muted);
    audio.mic.set_gate_threshold_db(settings.gate_threshold_db);
    telemetry.mic = audio.mic.telemetry();

    // Poll newly encoded 20ms Opus frames from our mic
    let frames = audio.mic.poll_encoded_frames();
    for (seq, opus_payload) in frames {
        if coop.is_connected() {
            coop.send_voice_frame(seq, opus_payload.clone());
        }
        if settings.self_echo_test {
            let pcm = audio
                .decoder_pool
                .decode_packet(LOCAL_LOOPBACK_SPEAKER_ID, seq, &opus_payload);
            if !pcm.is_empty() {
                audio.engine.push_voice_pcm(LOCAL_LOOPBACK_SPEAKER_ID, &pcm);
            }
        }
    }

    if !settings.self_echo_test {
        audio.engine.remove_voice_speaker(LOCAL_LOOPBACK_SPEAKER_ID);
        audio.decoder_pool.remove_speaker(LOCAL_LOOPBACK_SPEAKER_ID);
    }

    // Clean up disconnected remote speakers
    for rem in removed_speakers.read() {
        audio.engine.remove_voice_speaker(rem.speaker_id);
        audio.decoder_pool.remove_speaker(rem.speaker_id);
    }

    // Decode incoming remote players' Opus frames and inject straight into the acoustic mixer!
    let local_id = coop.session.as_ref().and_then(|s| s.local_player_id());
    for pkt in incoming_voice.read() {
        if Some(pkt.speaker_id) == local_id {
            continue;
        }
        let pcm = audio
            .decoder_pool
            .decode_packet(pkt.speaker_id, pkt.seq, &pkt.opus_payload);
        if !pcm.is_empty() {
            audio.engine.push_voice_pcm(pkt.speaker_id, &pcm);
        }
    }
}

/// Solves one-shot SFX (footsteps, flare bounces, breaches) using `solve_single_source` —
/// reuses the background worker's `ReverbEstimate` with ZERO wasted 4-bounce reverb rays!
fn process_spatial_sound_events(
    world_res: Res<WorldResource>,
    telemetry: Res<AcousticTelemetry>,
    player_q: Query<&Transform, With<PlayerController>>,
    mut beacon: ResMut<AcousticBeaconState>,
    mut events: MessageReader<PlaySpatialSoundEvent>,
    mut audio: NonSendMut<AudioEngineHolder>,
) {
    let Ok(cam_tf) = player_q.single() else {
        events.clear();
        return;
    };

    let listener_pos = cam_tf.translation;
    let world = world_res.read();

    for ev in events.read() {
        let id = beacon.next_oneshot_id;
        beacon.next_oneshot_id = beacon.next_oneshot_id.wrapping_add(1).max(100);

        let (ref_distance_m, max_distance_m) = match ev.clip {
            SynthClip::MonsterGrowl
            | SynthClip::MonsterScreech
            | SynthClip::ElevatorWinch
            | SynthClip::DecoyBuzz
            | SynthClip::HeavyImpact => (4.5, 75.0),
            _ => (1.8, 55.0),
        };
        let query = AcousticSourceQuery {
            id,
            pos_m: ev.pos_m,
            ref_distance_m,
            max_distance_m,
        };
        let params = solve_single_source(&world, listener_pos, query, telemetry.reverb, 16, id);

        audio.engine.send(AudioCommand::PlayClip {
            source_id: id,
            clip: ev.clip,
            volume: ev.volume,
            pitch: ev.pitch,
            looping: false,
            initial_params: params,
        });
    }
}
