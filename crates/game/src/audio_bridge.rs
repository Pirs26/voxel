use bevy::prelude::*;
use voxel_acoustics::{
    AcousticEvent, AcousticSourceQuery, PathTraceConfig, ReverbEstimate, SourceAcousticParams,
    solve_acoustics,
};
use voxel_audio_dsp::{AudioCommand, AudioEngine, SynthClip};

use crate::{
    player::PlayerController,
    voxel_render::{LevelInfoResource, WorldResource},
};

const BEACON_SOURCE_ID: u64 = 1;
const DRIP_SOURCE_ID: u64 = 2;

/// One-shot spatial sound trigger message (also carries an `AcousticEvent` for Week-3 blind AI).
#[derive(Message, Clone, Debug)]
pub struct PlaySpatialSoundEvent {
    pub pos_m: Vec3,
    pub clip: SynthClip,
    pub volume: f32,
    pub pitch: f32,
    pub ai_event: Option<AcousticEvent>,
}

/// Live telemetry resource displayed on the HUD so we can verify acoustic physics in real time.
#[derive(Resource, Clone, Debug)]
pub struct AcousticTelemetry {
    pub reverb: ReverbEstimate,
    pub beacon_params: Option<SourceAcousticParams>,
}

impl Default for AcousticTelemetry {
    fn default() -> Self {
        Self {
            reverb: ReverbEstimate::default(),
            beacon_params: None,
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

#[derive(Component)]
struct BeaconVisual;

/// Non-Send resource holding the `cpal` real-time stream safely on the main thread.
struct AudioEngineHolder(AudioEngine);

pub struct AudioBridgePlugin;

impl Plugin for AudioBridgePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<PlaySpatialSoundEvent>()
            .init_resource::<AcousticTelemetry>()
            .insert_non_send(AudioEngineHolder(AudioEngine::start()))
            .add_systems(Startup, setup_beacon_and_acoustics)
            .add_systems(
                Update,
                (
                    sync_beacon_visual,
                    solve_and_update_acoustics,
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
    level_info: Res<LevelInfoResource>,
) {
    let beacon_pos = level_info.0.acoustic_beacon_pos_m;
    commands.insert_resource(AcousticBeaconState {
        pos_m: beacon_pos,
        ping_timer: Timer::from_seconds(1.5, TimerMode::Repeating),
        drip_timer: Timer::from_seconds(4.2, TimerMode::Repeating),
        solve_timer: Timer::from_seconds(1.0 / 20.0, TimerMode::Repeating), // 20 Hz acoustic solver
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

fn sync_beacon_visual(
    beacon: Res<AcousticBeaconState>,
    mut vis_q: Query<&mut Transform, With<BeaconVisual>>,
) {
    if let Ok(mut tf) = vis_q.single_mut() {
        tf.translation = beacon.pos_m;
    }
}

fn solve_and_update_acoustics(
    time: Res<Time>,
    world_res: Res<WorldResource>,
    level_info: Res<LevelInfoResource>,
    player_q: Query<&Transform, With<PlayerController>>,
    mut beacon: ResMut<AcousticBeaconState>,
    mut telemetry: ResMut<AcousticTelemetry>,
    mut engine: NonSendMut<AudioEngineHolder>,
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

    // Re-solve voxel acoustics at 20 Hz and smooth reverb gently
    if beacon.solve_timer.just_finished() {
        beacon.frame_counter = beacon.frame_counter.wrapping_add(1);
        let queries = [
            AcousticSourceQuery {
                id: BEACON_SOURCE_ID,
                pos_m: beacon.pos_m,
                ref_distance_m: 2.0,
                max_distance_m: 65.0,
            },
            AcousticSourceQuery {
                id: DRIP_SOURCE_ID,
                pos_m: level_info.0.grand_cavern_pos_m,
                ref_distance_m: 2.5,
                max_distance_m: 70.0,
            },
        ];

        let snap = solve_acoustics(
            &world_res.0,
            listener_pos,
            listener_fwd,
            listener_right,
            &queries,
            PathTraceConfig::default(),
            beacon.frame_counter,
        );

        telemetry.reverb = telemetry.reverb.lerp(snap.reverb, 0.25);
        telemetry.beacon_params = snap.sources.first().copied();

        engine.0.send(AudioCommand::UpdateListener {
            pos_m: listener_pos,
            right: listener_right,
            reverb: telemetry.reverb,
        });
        for src in snap.sources {
            engine.0.send(AudioCommand::UpdateSource(src));
        }
    }

    // Trigger periodic Acoustic Beacon Ping (tests stone/foam wall muffling & breach clarity!)
    if beacon.ping_timer.just_finished() {
        if let Some(params) = telemetry.beacon_params {
            engine.0.send(AudioCommand::PlayClip {
                source_id: BEACON_SOURCE_ID,
                clip: SynthClip::BeaconPing,
                volume: 0.78,
                pitch: 1.0,
                looping: false,
                initial_params: params,
            });
        }
    }

    // Trigger periodic atmospheric Grand Cavern water drip
    if beacon.drip_timer.just_finished() {
        let drip_pos = level_info.0.grand_cavern_pos_m;
        let q = [AcousticSourceQuery {
            id: DRIP_SOURCE_ID,
            pos_m: drip_pos,
            ref_distance_m: 2.5,
            max_distance_m: 70.0,
        }];
        let snap = solve_acoustics(
            &world_res.0,
            listener_pos,
            listener_fwd,
            listener_right,
            &q,
            PathTraceConfig {
                reverb_rays: 16,
                early_reflection_rays: 16,
                ..Default::default()
            },
            beacon.frame_counter.wrapping_add(99),
        );
        if let Some(params) = snap.sources.first().copied() {
            engine.0.send(AudioCommand::PlayClip {
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

fn process_spatial_sound_events(
    world_res: Res<WorldResource>,
    player_q: Query<&Transform, With<PlayerController>>,
    mut beacon: ResMut<AcousticBeaconState>,
    mut events: MessageReader<PlaySpatialSoundEvent>,
    mut engine: NonSendMut<AudioEngineHolder>,
) {
    let Ok(cam_tf) = player_q.single() else {
        events.clear();
        return;
    };

    let listener_pos = cam_tf.translation;
    let listener_fwd = cam_tf.forward().as_vec3();
    let listener_right = cam_tf.right().as_vec3();

    for ev in events.read() {
        let id = beacon.next_oneshot_id;
        beacon.next_oneshot_id = beacon.next_oneshot_id.wrapping_add(1).max(100);

        let q = [AcousticSourceQuery {
            id,
            pos_m: ev.pos_m,
            ref_distance_m: 1.8,
            max_distance_m: 60.0,
        }];
        let snap = solve_acoustics(
            &world_res.0,
            listener_pos,
            listener_fwd,
            listener_right,
            &q,
            PathTraceConfig {
                reverb_rays: 12,
                early_reflection_rays: 18,
                ..Default::default()
            },
            id,
        );

        if let Some(params) = snap.sources.first().copied() {
            engine.0.send(AudioCommand::PlayClip {
                source_id: id,
                clip: ev.clip,
                volume: ev.volume,
                pitch: ev.pitch,
                looping: false,
                initial_params: params,
            });
        }
    }
}
