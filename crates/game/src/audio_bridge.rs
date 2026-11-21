use std::sync::Arc;

use bevy::prelude::*;
use voxel_acoustics::{
    AcousticEvent, AcousticSourceQuery, AcousticWorkRequest, AcousticWorker, PathTraceConfig,
    ReverbEstimate, SourceAcousticParams, solve_single_source,
};
use voxel_audio_dsp::{
    AudioCommand, AudioEngine, LOCAL_LOOPBACK_SPEAKER_ID, MicCapture, MicTelemetry, SynthClip,
    VoiceDecoderPool,
};
use voxel_net::PlayerId;

use crate::{
    net_bridge::{CoopMenuOpen, CoopState, remote_player_head_pos},
    player::PlayerController,
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

/// User-adjustable voice & mic calibration controls (Shed calibration + hotkeys).
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
}

pub struct AudioBridgePlugin;

impl Plugin for AudioBridgePlugin {
    fn build(&self, app: &mut App) {
        let engine = AudioEngine::start();
        let decoder_pool = VoiceDecoderPool::new(engine.sample_rate);
        let mic = MicCapture::start(-45.0);

        app.add_message::<PlaySpatialSoundEvent>()
            .add_message::<IncomingVoiceFrame>()
            .add_message::<RemoveVoiceSpeakerEvent>()
            .init_resource::<VoiceSettings>()
            .init_resource::<AcousticTelemetry>()
            .insert_non_send(AudioEngineHolder {
                engine,
                mic,
                decoder_pool,
            })
            .add_systems(Startup, setup_beacon_and_acoustics)
            .add_systems(
                Update,
                (
                    voice_hotkeys_system,
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
        ping_timer: Timer::from_seconds(1.5, TimerMode::Repeating),
        drip_timer: Timer::from_seconds(4.2, TimerMode::Repeating),
        solve_timer: Timer::from_seconds(1.0 / 20.0, TimerMode::Repeating),
        frame_counter: 0,
        next_oneshot_id: 100,
    });

    // Visual marker for the Acoustic Test Beacon (PointLight on child entity so it is NOT
    // frustum-culled by the small sphere's AABB when the sphere leaves view!)
    commands
        .spawn((
            Mesh3d(meshes.add(Sphere::new(0.22))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.2, 0.95, 0.9),
                emissive: LinearRgba::new(1.2, 8.0, 7.5, 1.0),
                ..default()
            })),
            Transform::from_translation(beacon_pos),
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
    menu_open: Res<CoopMenuOpen>,
    mut settings: ResMut<VoiceSettings>,
) {
    if menu_open.0 {
        return;
    }
    if keys.just_pressed(KeyCode::KeyM) {
        settings.muted = !settings.muted;
    }
    if keys.just_pressed(KeyCode::KeyV) {
        settings.self_echo_test = !settings.self_echo_test;
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        settings.gate_threshold_db = (settings.gate_threshold_db - 3.0).clamp(-65.0, -15.0);
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        settings.gate_threshold_db = (settings.gate_threshold_db + 3.0).clamp(-65.0, -15.0);
    }
}

fn sync_beacon_visual(
    beacon: Res<AcousticBeaconState>,
    mut vis_q: Query<&mut Transform, With<BeaconVisual>>,
) {
    if let Ok(mut tf) = vis_q.single_mut() {
        tf.translation = beacon.pos_m;
    }
}

/// Submits 20 Hz acoustic raytracing work to the background `AcousticWorker` and polls
/// completed snapshots without ever blocking Bevy's main render thread!
fn submit_and_poll_acoustics(
    time: Res<Time>,
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

        if settings.self_echo_test {
            if let Some(beacon_params) = telemetry.beacon_params {
                audio
                    .engine
                    .update_voice_acoustics(LOCAL_LOOPBACK_SPEAKER_ID, beacon_params);
            }
        }
    }

    // 3. Trigger periodic Acoustic Beacon Ping using already-solved params (zero raycasts!)
    if beacon.ping_timer.just_finished() && !settings.self_echo_test {
        if let Some(params) = telemetry.beacon_params {
            audio.engine.send(AudioCommand::PlayClip {
                source_id: BEACON_SOURCE_ID,
                clip: SynthClip::BeaconPing,
                volume: 0.78,
                pitch: 1.0,
                looping: false,
                initial_params: params,
            });
        }
    }

    // 4. Trigger periodic Grand Cavern water drip using already-solved drip params (zero raycasts!)
    if beacon.drip_timer.just_finished() {
        if let Some(params) = telemetry.drip_params {
            audio.engine.send(AudioCommand::PlayClip {
                source_id: DRIP_SOURCE_ID,
                clip: SynthClip::WaterDrip,
                volume: 0.65,
                pitch: 1.0,
                looping: false,
                initial_params: params,
            });
        }
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
    for pkt in incoming_voice.read() {
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

        let query = AcousticSourceQuery {
            id,
            pos_m: ev.pos_m,
            ref_distance_m: 1.8,
            max_distance_m: 60.0,
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
