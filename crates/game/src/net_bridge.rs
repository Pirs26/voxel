use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::io::Write;

use bevy::prelude::*;
use glam::IVec3;
use voxel_acoustics::{AcousticEvent, AcousticEventKind, Bands3};
use voxel_audio_dsp::SynthClip;
use voxel_core::{VoxelConfig, VoxelId, generate_world};
use voxel_net::{NetEvent, NetSession, PlayerId, PlayerInfo, ReliableMessage, UnreliableDatagram};

use crate::{
    audio_bridge::{AcousticBeaconState, IncomingVoiceFrame, PlaySpatialSoundEvent, RemoveVoiceSpeakerEvent},
    physics::PLAYER_EYE_STAND_M,
    player::{PlayerController, PlayerFlashlight},
    tools::SpawnFlareEvent,
    voxel_render::{LevelInfoResource, WorldResource},
};

pub const DEFAULT_LEVEL_SEED: u64 = 1337;

pub const VISOR_PALETTE: [(&str, [f32; 3]); 6] = [
    ("Cyan", [0.15, 0.92, 0.88]),
    ("Amber", [1.0, 0.68, 0.14]),
    ("Emerald", [0.22, 0.95, 0.42]),
    ("Crimson", [0.98, 0.24, 0.28]),
    ("Violet", [0.72, 0.35, 1.0]),
    ("Ice", [0.88, 0.94, 1.0]),
];

#[derive(Resource, Default)]
pub struct CoopMenuOpen(pub bool);

#[derive(Clone, Debug)]
pub struct RemotePlayerData {
    pub info: PlayerInfo,
    pub entity: Entity,
    pub head_entity: Entity,
    pub flashlight_entity: Entity,
    pub target_feet_m: Vec3,
    pub current_feet_m: Vec3,
    pub target_yaw: f32,
    pub target_pitch: f32,
    pub crouching: bool,
    pub flashlight_on: bool,
}

#[derive(Resource)]
pub struct CoopState {
    pub session: Option<NetSession>,
    pub level_seed: u64,
    pub local_name: String,
    pub visor_idx: usize,
    pub status_text: String,
    pub join_code: Option<String>,
    pub local_edits: Vec<(IVec3, VoxelId)>,
    pub remote_players: HashMap<PlayerId, RemotePlayerData>,
    pub pose_timer: Timer,
    pub pose_tick: u32,
}

impl Default for CoopState {
    fn default() -> Self {
        Self {
            session: None,
            level_seed: DEFAULT_LEVEL_SEED,
            local_name: "Miner".to_string(),
            visor_idx: 0,
            status_text: "Offline (Solo) — Press [Tab] to Host or Join".to_string(),
            join_code: None,
            local_edits: Vec::new(),
            remote_players: HashMap::new(),
            pose_timer: Timer::from_seconds(1.0 / 25.0, TimerMode::Repeating),
            pose_tick: 0,
        }
    }
}

impl CoopState {
    pub fn visor_rgb(&self) -> [f32; 3] {
        VISOR_PALETTE[self.visor_idx % VISOR_PALETTE.len()].1
    }

    pub fn visor_label(&self) -> &'static str {
        VISOR_PALETTE[self.visor_idx % VISOR_PALETTE.len()].0
    }

    pub fn is_connected(&self) -> bool {
        self.session.is_some()
    }

    pub fn send_voxel_delta(&mut self, coord: IVec3, material: VoxelId) {
        self.local_edits.push((coord, material));
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::VoxelDelta { coord, material });
        }
    }

    pub fn send_flare_thrown(&self, origin_m: Vec3, velocity_m: Vec3) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::FlareThrown {
                origin_m,
                velocity_m,
            });
        }
    }

    pub fn send_beacon_moved(&self, pos_m: Vec3) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::BeaconMoved { pos_m });
        }
    }

    pub fn send_voice_frame(&self, seq: u16, opus_payload: Vec<u8>) {
        if let Some(ref s) = self.session {
            let speaker_id = s.local_player_id().unwrap_or(0);
            s.send_datagram(UnreliableDatagram::VoiceFrame {
                speaker_id,
                seq,
                opus_payload,
            });
        }
    }

    pub fn start_host(&mut self, beacon_pos_m: Vec3) {
        self.session = None;
        self.join_code = None;
        match NetSession::start_host(
            self.local_name.clone(),
            self.visor_rgb(),
            self.level_seed,
            beacon_pos_m,
            self.local_edits.clone(),
        ) {
            Ok(session) => {
                self.session = Some(session);
                self.status_text = "Starting Iroh P2P host (fetching NAT/relay address)...".into();
            }
            Err(e) => {
                self.status_text = format!("Host failed: {e}");
            }
        }
    }

    pub fn start_join(&mut self, code: String) {
        let trimmed = code.trim().to_string();
        if trimmed.is_empty() {
            self.status_text = "Clipboard is empty — copy a host's ECHO-... code first!".into();
            return;
        }
        self.session = None;
        self.join_code = None;
        match NetSession::start_client(trimmed, self.local_name.clone(), self.visor_rgb()) {
            Ok(session) => {
                self.session = Some(session);
                self.status_text = "Connecting to host via Iroh P2P...".into();
            }
            Err(e) => {
                self.status_text = format!("Invalid join code: {e}");
            }
        }
    }
}

pub fn copy_to_clipboard(text: &str) -> bool {
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("pbcopy");
    #[cfg(target_os = "windows")]
    let mut cmd = Command::new("clip");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xclip");

    #[cfg(all(unix, not(target_os = "macos")))]
    cmd.args(["-selection", "clipboard"]);

    if let Ok(mut child) = cmd.stdin(Stdio::piped()).spawn() {
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
        return child.wait().map(|s| s.success()).unwrap_or(false);
    }
    false
}

pub fn paste_from_clipboard() -> Option<String> {
    #[cfg(target_os = "macos")]
    let output = Command::new("pbpaste").output().ok()?;
    #[cfg(target_os = "windows")]
    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", "Get-Clipboard"])
        .output()
        .ok()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    let output = Command::new("xclip")
        .args(["-o", "-selection", "clipboard"])
        .output()
        .ok()?;

    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}

pub struct NetBridgePlugin;

impl Plugin for NetBridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CoopMenuOpen>()
            .init_resource::<CoopState>()
            .add_systems(
                Update,
                (
                    poll_network_events_system,
                    send_local_pose_system,
                    interpolate_remote_players_system,
                )
                    .chain(),
            );
    }
}

fn spawn_remote_avatar(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    info: &PlayerInfo,
    spawn_feet: Vec3,
) -> RemotePlayerData {
    let suit_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.22, 0.25, 0.30),
        perceptual_roughness: 0.85,
        ..default()
    });
    let visor_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(info.visor_rgb[0], info.visor_rgb[1], info.visor_rgb[2]),
        emissive: LinearRgba::new(
            info.visor_rgb[0] * 6.0,
            info.visor_rgb[1] * 6.0,
            info.visor_rgb[2] * 6.0,
            1.0,
        ),
        ..default()
    });

    let mut head_entity = Entity::PLACEHOLDER;
    let mut flashlight_entity = Entity::PLACEHOLDER;

    let body_entity = commands
        .spawn((
            Transform::from_translation(spawn_feet),
            Visibility::default(),
        ))
        .with_children(|parent| {
            // Chunky blocky voxel torso
            parent.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.56, 1.15, 0.36))),
                MeshMaterial3d(suit_mat.clone()),
                Transform::from_xyz(0.0, 0.60, 0.0),
            ));

            // Articulated head pivoting with remote player's pitch
            head_entity = parent
                .spawn((
                    Mesh3d(meshes.add(Cuboid::new(0.42, 0.42, 0.42))),
                    MeshMaterial3d(suit_mat),
                    Transform::from_xyz(0.0, 1.46, 0.0),
                ))
                .with_children(|head| {
                    // Glowing visor strip facing -Z (forward)
                    head.spawn((
                        Mesh3d(meshes.add(Cuboid::new(0.34, 0.14, 0.05))),
                        MeshMaterial3d(visor_mat),
                        Transform::from_xyz(0.0, 0.03, -0.21),
                    ));
                    // Remote player's live flashlight beam (child light so no mesh frustum culling!)
                    flashlight_entity = head
                        .spawn((
                            SpotLight {
                                color: Color::srgb(1.0, 0.94, 0.82),
                                intensity: 320_000.0,
                                range: 36.0,
                                inner_angle: 0.24,
                                outer_angle: 0.54,
                                shadow_maps_enabled: false,
                                ..default()
                            },
                            Transform::from_xyz(0.16, -0.05, -0.25),
                        ))
                        .id();
                })
                .id();
        })
        .id();

    RemotePlayerData {
        info: info.clone(),
        entity: body_entity,
        head_entity,
        flashlight_entity,
        target_feet_m: spawn_feet,
        current_feet_m: spawn_feet,
        target_yaw: 0.0,
        target_pitch: 0.0,
        crouching: false,
        flashlight_on: true,
    }
}

pub fn clear_remote_players(
    commands: &mut Commands,
    coop: &mut CoopState,
    remove_voice: &mut MessageWriter<RemoveVoiceSpeakerEvent>,
) {
    for (id, rp) in coop.remote_players.drain() {
        commands.entity(rp.entity).despawn();
        remove_voice.write(RemoveVoiceSpeakerEvent { speaker_id: id });
    }
}

fn poll_network_events_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut coop: ResMut<CoopState>,
    mut world_res: ResMut<WorldResource>,
    mut level_info: ResMut<LevelInfoResource>,
    mut beacon: ResMut<AcousticBeaconState>,
    mut voice_writer: MessageWriter<IncomingVoiceFrame>,
    mut remove_voice: MessageWriter<RemoveVoiceSpeakerEvent>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
    mut flare_writer: MessageWriter<SpawnFlareEvent>,
) {
    let events = match coop.session {
        Some(ref s) => s.poll_events(),
        None => return,
    };

    for ev in events {
        match ev {
            NetEvent::HostReady { join_code } => {
                let copied = copy_to_clipboard(&join_code);
                coop.join_code = Some(join_code);
                coop.status_text = if copied {
                    "Hosting — Join code copied to clipboard! Share with friends.".into()
                } else {
                    "Hosting — Click 'Copy Join Code' to share with friends.".into()
                };
            }
            NetEvent::Connected {
                local_id,
                level_seed,
                players,
                voxel_edits,
                beacon_pos_m,
            } => {
                clear_remote_players(&mut commands, &mut coop, &mut remove_voice);

                // Deterministically regenerate the host's exact world & replay runtime edits
                if level_seed != coop.level_seed || !voxel_edits.is_empty() {
                    let (mut new_world, new_info) =
                        generate_world(level_seed, VoxelConfig::default());
                    for (coord, mat) in &voxel_edits {
                        new_world.set(*coord, *mat);
                    }
                    new_world.mark_all_dirty();
                    world_res.0 = new_world;
                    level_info.0 = new_info;
                    coop.level_seed = level_seed;
                    coop.local_edits = voxel_edits;
                }
                beacon.pos_m = beacon_pos_m;

                let default_spawn = level_info.0.spawn_pos_m;
                for p in &players {
                    if p.id != local_id {
                        let rp = spawn_remote_avatar(
                            &mut commands,
                            &mut meshes,
                            &mut materials,
                            p,
                            default_spawn,
                        );
                        coop.remote_players.insert(p.id, rp);
                    }
                }
                coop.status_text = format!(
                    "Connected as #{} ({} players in session)",
                    local_id,
                    coop.remote_players.len() + 1
                );
            }
            NetEvent::PlayerJoined(info) => {
                let local_id = coop.session.as_ref().and_then(|s| s.local_player_id());
                if Some(info.id) == local_id || coop.remote_players.contains_key(&info.id) {
                    continue;
                }
                let spawn = level_info.0.spawn_pos_m;
                let rp = spawn_remote_avatar(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &info,
                    spawn,
                );
                coop.remote_players.insert(info.id, rp);
                coop.status_text = format!(
                    "{} joined! ({} players)",
                    info.name,
                    coop.remote_players.len() + 1
                );
            }
            NetEvent::PlayerLeft(id) => {
                if let Some(rp) = coop.remote_players.remove(&id) {
                    commands.entity(rp.entity).despawn();
                    remove_voice.write(RemoveVoiceSpeakerEvent { speaker_id: id });
                    coop.status_text = format!(
                        "{} left ({} players)",
                        rp.info.name,
                        coop.remote_players.len() + 1
                    );
                }
            }
            NetEvent::VoxelDelta { coord, material } => {
                coop.local_edits.push((coord, material));
                if world_res.0.set(coord, material) {
                    let pos_m = world_res.0.config().voxel_to_world_center(coord);
                    let clip = if material.is_air() {
                        SynthClip::PickaxeHit
                    } else {
                        SynthClip::FoamSpray
                    };
                    sound_writer.write(PlaySpatialSoundEvent {
                        pos_m,
                        clip,
                        volume: 0.8,
                        pitch: 1.0,
                        ai_event: Some(AcousticEvent {
                            pos_m,
                            energy: Bands3::new(1.5, 2.5, 3.0),
                            sharpness: 0.9,
                            kind: AcousticEventKind::WallBreach,
                        }),
                    });
                }
            }
            NetEvent::FlareThrown {
                origin_m,
                velocity_m,
            } => {
                flare_writer.write(SpawnFlareEvent {
                    origin_m,
                    velocity_m,
                });
            }
            NetEvent::BeaconMoved { pos_m } => {
                beacon.pos_m = pos_m;
            }
            NetEvent::PlayerPose {
                player_id,
                feet_pos_m,
                yaw,
                pitch,
                crouching,
                flashlight_on,
                ..
            } => {
                if let Some(rp) = coop.remote_players.get_mut(&player_id) {
                    rp.target_feet_m = feet_pos_m;
                    rp.target_yaw = yaw;
                    rp.target_pitch = pitch;
                    rp.crouching = crouching;
                    rp.flashlight_on = flashlight_on;
                }
            }
            NetEvent::VoiceFrame {
                speaker_id,
                seq,
                opus_payload,
            } => {
                voice_writer.write(IncomingVoiceFrame {
                    speaker_id,
                    seq,
                    opus_payload,
                });
            }
            NetEvent::Error(err) => {
                coop.status_text = format!("Net error: {err}");
            }
            NetEvent::Disconnected(reason) => {
                clear_remote_players(&mut commands, &mut coop, &mut remove_voice);
                coop.session = None;
                coop.join_code = None;
                coop.status_text = format!("Disconnected: {reason}");
            }
        }
    }
}

fn send_local_pose_system(
    time: Res<Time>,
    mut coop: ResMut<CoopState>,
    player_q: Query<&PlayerController>,
    fl_q: Query<&PlayerFlashlight>,
) {
    if !coop.is_connected() {
        return;
    }
    coop.pose_timer.tick(time.delta());
    if !coop.pose_timer.just_finished() {
        return;
    }

    let Ok(pc) = player_q.single() else {
        return;
    };
    let flashlight_on = fl_q
        .single()
        .map(|f| f.on && f.battery > 0.0)
        .unwrap_or(false);

    coop.pose_tick = coop.pose_tick.wrapping_add(1);
    let tick = coop.pose_tick;
    if let Some(ref session) = coop.session {
        let player_id = session.local_player_id().unwrap_or(0);
        session.send_datagram(UnreliableDatagram::PlayerPose {
            player_id,
            tick,
            feet_pos_m: pc.feet_pos_m,
            yaw: pc.yaw,
            pitch: pc.pitch,
            crouching: pc.crouching,
            flashlight_on,
        });
    }
}

fn interpolate_remote_players_system(
    time: Res<Time>,
    mut coop: ResMut<CoopState>,
    mut tf_q: Query<&mut Transform>,
    mut light_q: Query<&mut SpotLight>,
) {
    let alpha = (time.delta_secs() * 14.0).clamp(0.0, 1.0);

    for rp in coop.remote_players.values_mut() {
        rp.current_feet_m = rp.current_feet_m.lerp(rp.target_feet_m, alpha);
        if let Ok(mut tf) = tf_q.get_mut(rp.entity) {
            tf.translation = rp.current_feet_m;
            tf.rotation = Quat::from_rotation_y(rp.target_yaw);
        }
        if let Ok(mut head_tf) = tf_q.get_mut(rp.head_entity) {
            let head_y = if rp.crouching { 1.05 } else { 1.46 };
            head_tf.translation.y = head_y;
            head_tf.rotation = Quat::from_rotation_x(rp.target_pitch);
        }
        if let Ok(mut spot) = light_q.get_mut(rp.flashlight_entity) {
            spot.intensity = if rp.flashlight_on { 320_000.0 } else { 0.0 };
        }
    }
}

/// World-space head position for a remote player's proximity voice source.
pub fn remote_player_head_pos(rp: &RemotePlayerData) -> Vec3 {
    let eye = if rp.crouching {
        1.0
    } else {
        PLAYER_EYE_STAND_M
    };
    rp.current_feet_m + Vec3::Y * eye
}
