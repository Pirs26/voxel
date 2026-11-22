use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use bevy::{ecs::system::SystemParam, prelude::*};
use glam::IVec3;
use voxel_acoustics::{AcousticEvent, AcousticEventKind, Bands3};
use voxel_audio_dsp::SynthClip;
use voxel_core::{VoxelConfig, VoxelId, generate_world_with_tier};
use voxel_net::{
    ElevatorPhase, HeldItemNetKind, MonsterBehaviorState, NetEvent, NetSession,
    PlayerHealthStatus, PlayerId, PlayerInfo, ReliableMessage, UnreliableDatagram,
    WorldItemNetState,
};

use crate::{
    ai_blind::{BlindStalker, RemoteSyncedStalker},
    audio_bridge::{
        AcousticBeaconState, IncomingVoiceFrame, PlaySpatialSoundEvent, RemoveVoiceSpeakerEvent,
    },
    elevator::{ElevatorState, TriggerElevatorLeverEvent},
    gameplay::{GameplayToast, upsert_remote_stalker},
    health::{
        LocalHealthPhase, MonsterHitLocalPlayerEvent, PlayerHealthState, RemoteHealthMap,
    },
    inventory::{BeltItem, PlayerInventory},
    physics::PLAYER_EYE_STAND_M,
    player::{PlayerController, PlayerFlashlight},
    relic::{RemoveWorldItemEvent, SpawnWorldItemEvent, WorldItemEntity},
    save::{CampaignState, SavedPlayerProfile},
    settings::{AppScreen, SettingsMenuOpen, UserSettings},
    shed::ShopOverlayOpen,
    tools::{SpawnFlareEvent, StrikeStalkerEvent},
    viewmodel::{PICKAXE_FRONT_TIP_PITCH_RAD, PICKAXE_REAR_TIP_PITCH_RAD, ViewmodelAnimationState},
    voxel_render::{LevelInfoResource, WorldResource},
};

pub const DEFAULT_LEVEL_SEED: u64 = 1337;

pub const VISOR_PALETTE: [(&str, [f32; 3]); 6] = [
    ("Cyan", [0.04, 0.90, 0.88]),
    ("Amber", [1.0, 0.48, 0.04]),
    ("Emerald", [0.06, 0.92, 0.24]),
    ("Crimson", [0.96, 0.08, 0.12]),
    ("Violet", [0.58, 0.12, 1.0]),
    ("Ice", [0.28, 0.72, 1.0]),
];

pub fn visor_idx_from_rgb(rgb: [f32; 3]) -> usize {
    VISOR_PALETTE
        .iter()
        .enumerate()
        .min_by(|(_, (_, a)), (_, (_, b))| {
            let da = (a[0] - rgb[0]).powi(2) + (a[1] - rgb[1]).powi(2) + (a[2] - rgb[2]).powi(2);
            let db = (b[0] - rgb[0]).powi(2) + (b[1] - rgb[1]).powi(2) + (b[2] - rgb[2]).powi(2);
            da.total_cmp(&db)
        })
        .map_or(0, |(idx, _)| idx)
}

#[derive(Resource, Default)]
pub struct CoopMenuOpen(pub bool);

#[derive(Clone, Debug)]
pub struct RemotePlayerData {
    pub info: PlayerInfo,
    pub entity: Entity,
    pub head_entity: Entity,
    pub left_arm_entity: Entity,
    pub right_arm_entity: Entity,
    pub flashlight_entity: Entity,
    pub pickaxe_entity: Entity,
    pub tool_entities: [(HeldItemNetKind, Entity); 6],
    pub fl_lens_on_entity: Entity,
    pub visor_material: Handle<StandardMaterial>,
    pub target_feet_m: Vec3,
    pub current_feet_m: Vec3,
    pub target_yaw: f32,
    pub target_pitch: f32,
    pub crouching: bool,
    pub flashlight_on: bool,
    pub held_item: HeldItemNetKind,
    pub pickaxe_swing_s: f32,
}

#[cfg(test)]
impl RemotePlayerData {
    pub fn dummy_for_test(info: PlayerInfo, feet: Vec3) -> Self {
        Self {
            info,
            entity: Entity::PLACEHOLDER,
            head_entity: Entity::PLACEHOLDER,
            left_arm_entity: Entity::PLACEHOLDER,
            right_arm_entity: Entity::PLACEHOLDER,
            flashlight_entity: Entity::PLACEHOLDER,
            pickaxe_entity: Entity::PLACEHOLDER,
            tool_entities: [
                (HeldItemNetKind::Flashlight, Entity::PLACEHOLDER),
                (HeldItemNetKind::GlowFlare, Entity::PLACEHOLDER),
                (HeldItemNetKind::AcousticFoam, Entity::PLACEHOLDER),
                (HeldItemNetKind::NoiseDecoy, Entity::PLACEHOLDER),
                (HeldItemNetKind::AidKit, Entity::PLACEHOLDER),
                (HeldItemNetKind::BonusCrystal, Entity::PLACEHOLDER),
            ],
            fl_lens_on_entity: Entity::PLACEHOLDER,
            visor_material: Handle::default(),
            target_feet_m: feet,
            current_feet_m: feet,
            target_yaw: 0.0,
            target_pitch: 0.0,
            crouching: false,
            flashlight_on: true,
            held_item: HeldItemNetKind::Flashlight,
            pickaxe_swing_s: 0.0,
        }
    }
}

#[derive(Resource)]
pub struct CoopState {
    pub session: Option<NetSession>,
    pub level_seed: u64,
    pub local_name: String,
    pub visor_idx: usize,
    pub status_text: String,
    pub join_code: Option<String>,
    pub local_edits: HashMap<IVec3, VoxelId>,
    pub remote_players: HashMap<PlayerId, RemotePlayerData>,
    pub pose_timer: Timer,
    pub pose_tick: u32,
    /// Pending graceful disconnect flush timer (lets final Guest inventory sync transmit before drop).
    pub pending_disconnect_s: Option<f32>,
    /// Tracks last synced serialized inventory JSON for guest-to-host persistence.
    pub last_synced_inv_json: String,
    pub inv_sync_timer: Timer,
}

impl Default for CoopState {
    fn default() -> Self {
        Self {
            session: None,
            level_seed: DEFAULT_LEVEL_SEED,
            local_name: "Miner".to_string(),
            visor_idx: 0,
            status_text: "Ready — Play Solo, Host, or Join a Co-op Lobby".to_string(),
            join_code: None,
            local_edits: HashMap::new(),
            remote_players: HashMap::new(),
            pose_timer: Timer::from_seconds(1.0 / 25.0, TimerMode::Repeating),
            pose_tick: 0,
            pending_disconnect_s: None,
            last_synced_inv_json: String::new(),
            inv_sync_timer: Timer::from_seconds(0.75, TimerMode::Repeating),
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

    pub fn is_host_or_solo(&self) -> bool {
        self.session.as_ref().is_none_or(|s| s.is_host())
    }

    pub fn local_player_id(&self) -> u16 {
        self.session
            .as_ref()
            .and_then(|s| s.local_player_id())
            .unwrap_or(0)
    }

    pub fn send_voxel_delta(&mut self, coord: IVec3, material: VoxelId) {
        self.local_edits.insert(coord, material);
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

    pub fn send_start_descent(&self, level_seed: u64, depth_tier: u8) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::StartDescent {
                level_seed,
                depth_tier,
            });
        }
    }

    pub fn send_elevator_sync(&self, elevator: &ElevatorState) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::ElevatorStateSync {
                phase: elevator.phase,
                platform_y_m: elevator.platform_y_m,
                phase_timer_s: elevator.phase_timer_s,
                depth_tier: elevator.depth_tier,
                cave_seed: elevator.cave_seed,
            });
        }
    }

    pub fn send_request_elevator_action(&self, depth_tier: u8) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::RequestElevatorAction { depth_tier });
        }
    }

    pub fn send_world_item_spawn(&self, item: WorldItemNetState) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::WorldItemSpawn(item));
        }
    }

    pub fn send_world_item_update(&self, item: WorldItemNetState) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::WorldItemUpdate(item));
        }
    }

    pub fn send_world_item_remove(&self, item_id: u32) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::WorldItemRemove { item_id });
        }
    }

    pub fn send_player_health(&self, status: PlayerHealthStatus) {
        if let Some(ref s) = self.session {
            let player_id = s.local_player_id().unwrap_or(0);
            s.send_reliable(ReliableMessage::PlayerHealthSync { player_id, status });
        }
    }

    pub fn send_player_revived(&self, target_id: u16, reviver_id: u16) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::PlayerRevived {
                target_id,
                reviver_id,
            });
        }
    }

    pub fn send_credits_sync(
        &self,
        credits: i32,
        unlocked_depth_tier: u8,
        winch_overdrive: bool,
        fast_charger: bool,
    ) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::CreditsSync {
                credits,
                unlocked_depth_tier,
                winch_overdrive,
                fast_charger,
            });
        }
    }

    pub fn send_parkour_record_submit(&self, player_name: String, time_ms: u32) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::ParkourRecordSubmit {
                player_name,
                time_ms,
            });
        }
    }

    pub fn send_parkour_leaderboard_sync(&self, records: &[crate::save::ParkourRecord]) {
        if let Some(ref s) = self.session {
            let wire = records
                .iter()
                .map(|r| (r.player_name.clone(), r.time_ms))
                .collect();
            s.send_reliable(ReliableMessage::ParkourLeaderboardSync { records: wire });
        }
    }

    pub fn send_player_carry_sync(
        &self,
        carrier_id: PlayerId,
        target_id: PlayerId,
        target_feet_m: Vec3,
        active: bool,
    ) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::PlayerCarrySync {
                carrier_id,
                target_id,
                target_feet_m,
                active,
            });
        }
    }

    pub fn send_player_profile_sync(
        &self,
        player_id: PlayerId,
        player_name: String,
        visor_rgb: [f32; 3],
        inventory_json: String,
    ) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::PlayerProfileSync {
                player_id,
                player_name,
                visor_rgb,
                inventory_json,
            });
        }
    }

    /// Broadcasts the local player's current visor color, display name, and inventory snapshot.
    pub fn sync_local_profile(&mut self, inv: &PlayerInventory) {
        if let Some(ref s) = self.session {
            let player_id = s.local_player_id().unwrap_or(0);
            let inv_json = serde_json::to_string(inv).unwrap_or_default();
            self.last_synced_inv_json = inv_json.clone();
            s.send_reliable(ReliableMessage::PlayerProfileSync {
                player_id,
                player_name: self.local_name.clone(),
                visor_rgb: self.visor_rgb(),
                inventory_json: inv_json,
            });
        }
    }

    pub fn send_monster_attack(&self, target_player_id: u16, monster_pos_m: Vec3) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::MonsterAttack {
                target_player_id,
                monster_pos_m,
            });
        }
    }

    pub fn send_monster_damaged(
        &self,
        monster_id: u16,
        damage: u8,
        stun_s: f32,
        knockback_dir: Vec3,
    ) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::MonsterDamaged {
                monster_id,
                damage,
                stun_s,
                knockback_dir,
            });
        }
    }

    pub fn send_monster_killed(&self, monster_id: u16, pos_m: Vec3) {
        if let Some(ref s) = self.session {
            s.send_reliable(ReliableMessage::MonsterKilled { monster_id, pos_m });
        }
    }

    pub fn send_item_pose(&self, item_id: u32, pos_m: Vec3) {
        if let Some(ref s) = self.session {
            s.send_datagram(UnreliableDatagram::ItemPose { item_id, pos_m });
        }
    }

    pub fn send_monster_pose(
        &self,
        monster_id: u16,
        pos_m: Vec3,
        yaw: f32,
        state: MonsterBehaviorState,
    ) {
        if let Some(ref s) = self.session {
            s.send_datagram(UnreliableDatagram::MonsterPose {
                monster_id,
                pos_m,
                yaw,
                state,
            });
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

    pub fn start_host(&mut self, beacon_pos_m: Vec3) -> bool {
        self.session = None;
        self.join_code = None;
        self.pending_disconnect_s = None;
        let edits_snapshot: Vec<(IVec3, VoxelId)> =
            self.local_edits.iter().map(|(&c, &m)| (c, m)).collect();
        match NetSession::start_host(
            self.local_name.clone(),
            self.visor_rgb(),
            voxel_updater::APP_VERSION.to_string(),
            self.level_seed,
            beacon_pos_m,
            edits_snapshot,
        ) {
            Ok(session) => {
                self.session = Some(session);
                self.status_text =
                    "Starting Iroh P2P host (fetching NAT/relay address)...".into();
                true
            }
            Err(e) => {
                self.status_text = format!("Host failed: {e}");
                false
            }
        }
    }

    pub fn start_join(&mut self, code: String) -> bool {
        let trimmed = code.trim().to_string();
        if trimmed.is_empty() {
            self.status_text = "Clipboard is empty — copy a host's ECHO-... code first!".into();
            return false;
        }
        self.session = None;
        self.join_code = None;
        self.pending_disconnect_s = None;
        self.last_synced_inv_json.clear();
        match NetSession::start_client(
            trimmed,
            self.local_name.clone(),
            self.visor_rgb(),
            voxel_updater::APP_VERSION.to_string(),
        ) {
            Ok(session) => {
                self.session = Some(session);
                self.status_text = "Connecting to host via Iroh P2P...".into();
                true
            }
            Err(e) => {
                self.status_text = format!("Invalid join code: {e}");
                false
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
        let initial_settings = app
            .world()
            .get_resource::<UserSettings>()
            .cloned()
            .unwrap_or_default();

        let coop = CoopState {
            local_name: initial_settings.player_name,
            visor_idx: initial_settings.visor_idx % VISOR_PALETTE.len(),
            ..Default::default()
        };

        app.init_resource::<CoopMenuOpen>()
            .insert_resource(coop)
            .add_systems(
                Update,
                (
                    poll_network_events_system,
                    sync_guest_inventory_to_host_system,
                    pending_disconnect_flush_system,
                    send_local_pose_system,
                    interpolate_remote_players_system,
                )
                    .chain(),
            );
    }
}

/// Computes a saturated linear HDR emissive color so tonemapping preserves rich visor color
/// instead of desaturating into blinding white.
fn visor_emissive_linear(rgb: [f32; 3]) -> LinearRgba {
    let lin = Color::srgb(rgb[0], rgb[1], rgb[2]).to_linear();
    LinearRgba::new(lin.red * 2.4, lin.green * 2.4, lin.blue * 2.4, 1.0)
}

fn make_visor_material(rgb: [f32; 3]) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgb(rgb[0], rgb[1], rgb[2]),
        emissive: visor_emissive_linear(rgb),
        unlit: false,
        perceptual_roughness: 0.25,
        ..default()
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
    let glove_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.32, 0.28, 0.22),
        perceptual_roughness: 0.85,
        ..default()
    });
    let dark_steel = materials.add(StandardMaterial {
        base_color: Color::srgb(0.20, 0.22, 0.25),
        perceptual_roughness: 0.42,
        metallic: 0.78,
        ..default()
    });
    let dark_iron = materials.add(StandardMaterial {
        base_color: Color::srgb(0.14, 0.15, 0.17),
        perceptual_roughness: 0.50,
        metallic: 0.82,
        ..default()
    });
    let timber_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.46, 0.29, 0.15),
        perceptual_roughness: 0.85,
        ..default()
    });
    let lens_on_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.94, 0.76),
        emissive: LinearRgba::new(4.5, 3.6, 1.8, 1.0),
        perceptual_roughness: 0.15,
        ..default()
    });
    let flare_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.80, 0.14, 0.12),
        perceptual_roughness: 0.65,
        ..default()
    });
    let flare_tip_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.42, 0.14),
        emissive: LinearRgba::new(5.5, 1.8, 0.4, 1.0),
        ..default()
    });
    let foam_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.14, 0.64, 0.60),
        perceptual_roughness: 0.45,
        ..default()
    });
    let white_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.92, 0.93, 0.95),
        perceptual_roughness: 0.50,
        ..default()
    });
    let decoy_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.90, 0.60, 0.12),
        perceptual_roughness: 0.45,
        ..default()
    });
    let aid_red_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.88, 0.14, 0.16),
        emissive: LinearRgba::new(0.8, 0.08, 0.08, 1.0),
        ..default()
    });
    let crystal_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.24, 0.90, 0.98),
        emissive: LinearRgba::new(1.2, 4.2, 5.2, 1.0),
        perceptual_roughness: 0.22,
        ..default()
    });
    let visor_mat = materials.add(make_visor_material(info.visor_rgb));

    let arm_sleeve_mesh = meshes.add(Cuboid::new(0.15, 0.44, 0.15));
    let arm_cuff_mesh = meshes.add(Cuboid::new(0.165, 0.08, 0.165));
    let arm_glove_mesh = meshes.add(Cuboid::new(0.16, 0.14, 0.17));

    let mut head_entity = Entity::PLACEHOLDER;
    let mut left_arm_entity = Entity::PLACEHOLDER;
    let mut right_arm_entity = Entity::PLACEHOLDER;
    let mut flashlight_entity = Entity::PLACEHOLDER;
    let mut pickaxe_entity = Entity::PLACEHOLDER;
    let mut fl_ent = Entity::PLACEHOLDER;
    let mut fl_lens_on_entity = Entity::PLACEHOLDER;
    let mut flare_ent = Entity::PLACEHOLDER;
    let mut foam_ent = Entity::PLACEHOLDER;
    let mut decoy_ent = Entity::PLACEHOLDER;
    let mut aid_ent = Entity::PLACEHOLDER;
    let mut crystal_ent = Entity::PLACEHOLDER;

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
                    MeshMaterial3d(suit_mat.clone()),
                    Transform::from_xyz(0.0, 1.46, 0.0),
                ))
                .with_children(|head| {
                    // Glowing front visor strip facing -Z (forward), clearly proud of helmet front
                    head.spawn((
                        Mesh3d(meshes.add(Cuboid::new(0.38, 0.16, 0.06))),
                        MeshMaterial3d(visor_mat.clone()),
                        Transform::from_xyz(0.0, 0.04, -0.22),
                    ));
                    // Matching rear helmet ID light strip (+Z) so teammates are identifiable from behind too
                    head.spawn((
                        Mesh3d(meshes.add(Cuboid::new(0.24, 0.08, 0.04))),
                        MeshMaterial3d(visor_mat.clone()),
                        Transform::from_xyz(0.0, 0.04, 0.215),
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

            // Left articulated arm + hand holding miner's Pickaxe (downward-curved head!)
            left_arm_entity = parent
                .spawn((
                    Transform::from_xyz(-0.36, 1.06, 0.0),
                    Visibility::Inherited,
                ))
                .with_children(|arm| {
                    arm.spawn((
                        Mesh3d(arm_sleeve_mesh.clone()),
                        MeshMaterial3d(suit_mat.clone()),
                        Transform::from_xyz(0.0, -0.20, 0.0),
                    ));
                    arm.spawn((
                        Mesh3d(arm_cuff_mesh.clone()),
                        MeshMaterial3d(dark_steel.clone()),
                        Transform::from_xyz(0.0, -0.43, 0.0),
                    ));
                    arm.spawn((
                        Mesh3d(arm_glove_mesh.clone()),
                        MeshMaterial3d(glove_mat.clone()),
                        Transform::from_xyz(0.0, -0.52, -0.01),
                    ));

                    // Third-person Pickaxe gripped in left hand
                    pickaxe_entity = arm
                        .spawn((
                            Transform::from_xyz(0.0, -0.52, -0.10)
                                .with_rotation(Quat::from_rotation_x(-1.15)),
                            Visibility::Inherited,
                        ))
                        .with_children(|pick| {
                            pick.spawn((
                                Mesh3d(meshes.add(Cuboid::new(0.045, 0.56, 0.045))),
                                MeshMaterial3d(timber_mat),
                                Transform::from_xyz(0.0, 0.16, 0.0),
                            ));
                            pick.spawn((
                                Mesh3d(meshes.add(Cuboid::new(0.065, 0.07, 0.22))),
                                MeshMaterial3d(dark_iron.clone()),
                                Transform::from_xyz(0.0, 0.44, 0.0),
                            ));
                            pick.spawn((
                                Mesh3d(meshes.add(Cuboid::new(0.045, 0.05, 0.18))),
                                MeshMaterial3d(dark_iron.clone()),
                                Transform::from_xyz(0.0, 0.41, -0.17)
                                    .with_rotation(Quat::from_rotation_x(PICKAXE_FRONT_TIP_PITCH_RAD)),
                            ));
                            pick.spawn((
                                Mesh3d(meshes.add(Cuboid::new(0.05, 0.045, 0.15))),
                                MeshMaterial3d(dark_iron.clone()),
                                Transform::from_xyz(0.0, 0.415, 0.15)
                                    .with_rotation(Quat::from_rotation_x(PICKAXE_REAR_TIP_PITCH_RAD)),
                            ));
                        })
                        .id();
                })
                .id();

            // Right articulated arm + hand holding active belt tool (visible to teammates!)
            right_arm_entity = parent
                .spawn((
                    Transform::from_xyz(0.36, 1.06, 0.0),
                    Visibility::Inherited,
                ))
                .with_children(|arm| {
                    arm.spawn((
                        Mesh3d(arm_sleeve_mesh),
                        MeshMaterial3d(suit_mat),
                        Transform::from_xyz(0.0, -0.20, 0.0),
                    ));
                    arm.spawn((
                        Mesh3d(arm_cuff_mesh),
                        MeshMaterial3d(dark_steel.clone()),
                        Transform::from_xyz(0.0, -0.43, 0.0),
                    ));
                    arm.spawn((
                        Mesh3d(arm_glove_mesh),
                        MeshMaterial3d(glove_mat),
                        Transform::from_xyz(0.0, -0.52, -0.01),
                    ));

                    // Tool grip anchor at the right hand
                    arm.spawn((
                        Transform::from_xyz(0.0, -0.53, -0.08),
                        Visibility::Inherited,
                    ))
                    .with_children(|hand| {
                        // 1. Flashlight
                        fl_ent = hand
                            .spawn((Transform::default(), Visibility::Inherited))
                            .with_children(|fl| {
                                fl.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.085, 0.085, 0.30))),
                                    MeshMaterial3d(dark_steel.clone()),
                                    Transform::from_xyz(0.0, 0.0, -0.10),
                                ));
                                fl.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.11, 0.11, 0.07))),
                                    MeshMaterial3d(dark_iron.clone()),
                                    Transform::from_xyz(0.0, 0.0, -0.25),
                                ));
                                fl_lens_on_entity = fl
                                    .spawn((
                                        Mesh3d(meshes.add(Cuboid::new(0.09, 0.09, 0.02))),
                                        MeshMaterial3d(lens_on_mat),
                                        Transform::from_xyz(0.0, 0.0, -0.29),
                                        Visibility::Inherited,
                                    ))
                                    .id();
                            })
                            .id();

                        // 2. Glow Flare
                        flare_ent = hand
                            .spawn((
                                Transform::from_xyz(0.0, 0.04, -0.04),
                                Visibility::Hidden,
                            ))
                            .with_children(|f| {
                                f.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.06, 0.30, 0.06))),
                                    MeshMaterial3d(flare_mat),
                                    Transform::default(),
                                ));
                                f.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.07, 0.06, 0.07))),
                                    MeshMaterial3d(flare_tip_mat),
                                    Transform::from_xyz(0.0, 0.17, 0.0),
                                ));
                            })
                            .id();

                        // 3. Acoustic Foam Canister
                        foam_ent = hand
                            .spawn((
                                Transform::from_xyz(0.0, 0.02, -0.06),
                                Visibility::Hidden,
                            ))
                            .with_children(|fm| {
                                fm.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.11, 0.28, 0.11))),
                                    MeshMaterial3d(foam_mat),
                                    Transform::default(),
                                ));
                                fm.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.04, 0.04, 0.11))),
                                    MeshMaterial3d(white_mat.clone()),
                                    Transform::from_xyz(0.0, 0.16, -0.06),
                                ));
                            })
                            .id();

                        // 4. Noise Decoy
                        decoy_ent = hand
                            .spawn((
                                Transform::from_xyz(0.0, 0.01, -0.06),
                                Visibility::Hidden,
                            ))
                            .with_children(|dc| {
                                dc.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.14, 0.17, 0.10))),
                                    MeshMaterial3d(decoy_mat),
                                    Transform::default(),
                                ));
                                dc.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.02, 0.10, 0.02))),
                                    MeshMaterial3d(dark_steel.clone()),
                                    Transform::from_xyz(0.04, 0.12, 0.0),
                                ));
                            })
                            .id();

                        // 5. Aid Kit
                        aid_ent = hand
                            .spawn((
                                Transform::from_xyz(0.0, 0.0, -0.07),
                                Visibility::Hidden,
                            ))
                            .with_children(|aid| {
                                aid.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.20, 0.14, 0.10))),
                                    MeshMaterial3d(white_mat),
                                    Transform::default(),
                                ));
                                aid.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.09, 0.03, 0.02))),
                                    MeshMaterial3d(aid_red_mat.clone()),
                                    Transform::from_xyz(0.0, 0.0, -0.052),
                                ));
                                aid.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.03, 0.09, 0.02))),
                                    MeshMaterial3d(aid_red_mat),
                                    Transform::from_xyz(0.0, 0.0, -0.052),
                                ));
                            })
                            .id();

                        // 6. Bonus Crystal
                        crystal_ent = hand
                            .spawn((
                                Transform::from_xyz(0.0, 0.04, -0.05),
                                Visibility::Hidden,
                            ))
                            .with_children(|cr| {
                                cr.spawn((
                                    Mesh3d(meshes.add(Cuboid::new(0.08, 0.24, 0.08))),
                                    MeshMaterial3d(crystal_mat),
                                    Transform::from_rotation(Quat::from_euler(
                                        EulerRot::YXZ,
                                        0.35,
                                        0.10,
                                        -0.12,
                                    )),
                                ));
                            })
                            .id();
                    });
                })
                .id();
        })
        .id();

    RemotePlayerData {
        info: info.clone(),
        entity: body_entity,
        head_entity,
        left_arm_entity,
        right_arm_entity,
        flashlight_entity,
        pickaxe_entity,
        tool_entities: [
            (HeldItemNetKind::Flashlight, fl_ent),
            (HeldItemNetKind::GlowFlare, flare_ent),
            (HeldItemNetKind::AcousticFoam, foam_ent),
            (HeldItemNetKind::NoiseDecoy, decoy_ent),
            (HeldItemNetKind::AidKit, aid_ent),
            (HeldItemNetKind::BonusCrystal, crystal_ent),
        ],
        fl_lens_on_entity,
        visor_material: visor_mat,
        target_feet_m: spawn_feet,
        current_feet_m: spawn_feet,
        target_yaw: 0.0,
        target_pitch: 0.0,
        crouching: false,
        flashlight_on: true,
        held_item: HeldItemNetKind::Flashlight,
        pickaxe_swing_s: 0.0,
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

#[derive(SystemParam)]
struct PollNetMenus<'w> {
    screen: ResMut<'w, AppScreen>,
    menu_open: ResMut<'w, CoopMenuOpen>,
    shop_open: ResMut<'w, ShopOverlayOpen>,
    settings_open: ResMut<'w, SettingsMenuOpen>,
}

#[derive(SystemParam)]
struct PollNetWorldState<'w> {
    world_res: Res<'w, WorldResource>,
    level_info: ResMut<'w, LevelInfoResource>,
    beacon: ResMut<'w, AcousticBeaconState>,
    elevator: ResMut<'w, ElevatorState>,
    campaign: ResMut<'w, CampaignState>,
    inv: ResMut<'w, PlayerInventory>,
    health: ResMut<'w, PlayerHealthState>,
    remote_health: ResMut<'w, RemoteHealthMap>,
    toast: ResMut<'w, GameplayToast>,
}

#[derive(SystemParam)]
struct PollNetWriters<'w> {
    voice_writer: MessageWriter<'w, IncomingVoiceFrame>,
    remove_voice: MessageWriter<'w, RemoveVoiceSpeakerEvent>,
    sound_writer: MessageWriter<'w, PlaySpatialSoundEvent>,
    flare_writer: MessageWriter<'w, SpawnFlareEvent>,
    spawn_item_writer: MessageWriter<'w, SpawnWorldItemEvent>,
    remove_item_writer: MessageWriter<'w, RemoveWorldItemEvent>,
    lever_writer: MessageWriter<'w, TriggerElevatorLeverEvent>,
    hit_writer: MessageWriter<'w, MonsterHitLocalPlayerEvent>,
    strike_writer: MessageWriter<'w, StrikeStalkerEvent>,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn poll_network_events_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut coop: ResMut<CoopState>,
    mut menus: PollNetMenus,
    mut ws: PollNetWorldState,
    mut player_q: Query<&mut PlayerController>,
    mut items_q: Query<(Entity, &mut WorldItemEntity, &mut Transform)>,
    mut remote_stalkers_q: Query<(Entity, &mut RemoteSyncedStalker)>,
    stalker_ents_q: Query<Entity, Or<(With<BlindStalker>, With<RemoteSyncedStalker>)>>,
    mut writers: PollNetWriters,
) {
    let screen = &mut *menus.screen;
    let menu_open = &mut *menus.menu_open;
    let shop_open = &mut *menus.shop_open;
    let settings_open = &mut *menus.settings_open;
    let world_res = &*ws.world_res;
    let level_info = &mut *ws.level_info;
    let beacon = &mut *ws.beacon;
    let elevator = &mut *ws.elevator;
    let campaign = &mut *ws.campaign;
    let inv = &mut *ws.inv;
    let health = &mut *ws.health;
    let remote_health = &mut *ws.remote_health;
    let toast = &mut *ws.toast;
    let voice_writer = &mut writers.voice_writer;
    let remove_voice = &mut writers.remove_voice;
    let sound_writer = &mut writers.sound_writer;
    let flare_writer = &mut writers.flare_writer;
    let spawn_item_writer = &mut writers.spawn_item_writer;
    let remove_item_writer = &mut writers.remove_item_writer;
    let lever_writer = &mut writers.lever_writer;
    let hit_writer = &mut writers.hit_writer;
    let strike_writer = &mut writers.strike_writer;

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
                toast.show(
                    if copied {
                        "Hosting Lobby — ECHO join code copied to clipboard!"
                    } else {
                        "Hosting Lobby — Press [Tab] to copy ECHO join code"
                    },
                    4.0,
                );
            }
            NetEvent::Connected {
                local_id,
                level_seed,
                depth_tier,
                elevator_phase,
                credits,
                world_items,
                players,
                voxel_edits,
                beacon_pos_m,
            } => {
                clear_remote_players(&mut commands, &mut coop, remove_voice);
                remote_health.0.clear();
                for ent in &stalker_ents_q {
                    commands.entity(ent).despawn();
                }
                for (ent, _, _) in &items_q {
                    commands.entity(ent).despawn();
                }

                // Deterministically regenerate the host's exact world & replay runtime edits
                let tier = depth_tier.clamp(1, 3);
                let (mut new_world, new_info) =
                    generate_world_with_tier(level_seed, tier, VoxelConfig::default());
                for (coord, mat) in &voxel_edits {
                    new_world.set(*coord, *mat);
                }
                new_world.mark_all_dirty();
                *world_res.write() = new_world;
                level_info.0 = new_info;
                coop.level_seed = level_seed;
                coop.local_edits = voxel_edits.into_iter().collect();
                beacon.pos_m = beacon_pos_m;

                // Sync elevator & shared campaign state from host Welcome
                *elevator = ElevatorState::from_level_info(&level_info.0, level_seed);
                let plat_y = match elevator_phase {
                    ElevatorPhase::ShedIdle => level_info.0.shed_floor_y_m,
                    ElevatorPhase::CaveIdle | ElevatorPhase::WinchActive => {
                        level_info.0.cave_floor_y_m
                    }
                    ElevatorPhase::Descending | ElevatorPhase::Ascending => {
                        0.5 * (level_info.0.shed_floor_y_m + level_info.0.cave_floor_y_m)
                    }
                };
                elevator.apply_network_sync(elevator_phase, plat_y, 0.0, tier, level_seed);

                campaign.save.credits = credits;
                campaign.save.selected_depth_tier = tier;
                campaign.save.unlocked_depth_tier = campaign.save.unlocked_depth_tier.max(tier);

                // Spawn all authoritative world items from host
                for item in world_items {
                    spawn_item_writer.write(SpawnWorldItemEvent(item));
                }

                // Place local guest cleanly in Shed (or elevator cage if mid-expedition)
                let spawn_pos = if elevator_phase == ElevatorPhase::ShedIdle {
                    level_info.0.spawn_pos_m
                } else {
                    Vec3::new(
                        elevator.center_xz_m.x,
                        elevator.platform_y_m + 0.05,
                        elevator.center_xz_m.y,
                    )
                };
                if let Ok(mut pc) = player_q.single_mut() {
                    pc.feet_pos_m = spawn_pos;
                    pc.velocity_m = Vec3::ZERO;
                }
                health.reset_for_shed();
                *inv = PlayerInventory::starter();
                coop.last_synced_inv_json.clear();

                // Spawn avatars for host and all existing teammates
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

                // Transition cleanly from Main Menu into the game world!
                menu_open.0 = false;
                shop_open.0 = false;
                settings_open.0 = false;
                *screen = AppScreen::InGame;
                toast.show("Joined Co-op Lobby! Syncing saved loadout from Host...", 3.5);
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
                remote_health.0.insert(info.id, LocalHealthPhase::Healthy);
                coop.status_text = format!(
                    "{} joined! ({} players)",
                    info.name,
                    coop.remote_players.len() + 1
                );
                toast.show(format!("{} joined the lobby!", info.name), 3.0);

                // Host-side persistent inventory restoration by Player Name!
                if coop.is_host_or_solo() {
                    let incoming_visor_idx = visor_idx_from_rgb(info.visor_rgb);
                    let mut profile = campaign
                        .save
                        .get_or_create_player(&info.name, incoming_visor_idx);
                    profile.visor_idx = incoming_visor_idx;
                    campaign.save.upsert_player(profile.clone());
                    let inv_json = serde_json::to_string(&profile.inventory).unwrap_or_default();
                    coop.send_player_profile_sync(
                        info.id,
                        info.name.clone(),
                        info.visor_rgb,
                        inv_json,
                    );
                    coop.send_credits_sync(
                        campaign.save.credits,
                        campaign.save.unlocked_depth_tier,
                        campaign.save.winch_overdrive,
                        campaign.save.fast_charger,
                    );
                    coop.send_parkour_leaderboard_sync(&campaign.save.parkour_leaderboard);
                }
            }
            NetEvent::PlayerLeft(id) => {
                remote_health.0.remove(&id);
                if let Some(rp) = coop.remote_players.remove(&id) {
                    commands.entity(rp.entity).despawn();
                    remove_voice.write(RemoveVoiceSpeakerEvent { speaker_id: id });
                    coop.status_text = format!(
                        "{} left ({} players)",
                        rp.info.name,
                        coop.remote_players.len() + 1
                    );
                    toast.show(
                        format!("{} left (inventory saved on Host)", rp.info.name),
                        3.0,
                    );
                    if coop.is_host_or_solo() {
                        let _ = campaign.save_now();
                    }
                }
            }
            NetEvent::PlayerProfileSync {
                player_id,
                player_name,
                visor_rgb,
                inventory_json,
            } => {
                // 1. Live visor color & display name update on remote avatar!
                if let Some(rp) = coop.remote_players.get_mut(&player_id) {
                    rp.info.visor_rgb = visor_rgb;
                    if !player_name.trim().is_empty() {
                        rp.info.name = player_name.clone();
                    }
                    if let Some(mut mat) = materials.get_mut(&rp.visor_material) {
                        mat.base_color = Color::srgb(visor_rgb[0], visor_rgb[1], visor_rgb[2]);
                        mat.emissive = visor_emissive_linear(visor_rgb);
                    }
                }

                // 2. Host persists every guest's inventory by Player Name
                let local_id = coop.local_player_id();
                if coop.is_host_or_solo() && player_id != 0 {
                    if !inventory_json.is_empty()
                        && let Ok(mut guest_inv) =
                            serde_json::from_str::<PlayerInventory>(&inventory_json)
                    {
                        guest_inv.normalize_slots();
                        let name = if player_name.trim().is_empty() {
                            coop.remote_players
                                .get(&player_id)
                                .map(|rp| rp.info.name.clone())
                                .unwrap_or_else(|| format!("Miner_{player_id}"))
                        } else {
                            player_name.clone()
                        };
                        campaign.save.upsert_player(SavedPlayerProfile {
                            player_name: name,
                            visor_idx: visor_idx_from_rgb(visor_rgb),
                            inventory: guest_inv,
                        });
                        campaign.dirty = true;
                    }
                } else if !coop.is_host_or_solo()
                    && player_id == local_id
                    && !inventory_json.is_empty()
                {
                    // 3. Guest restores their saved inventory pushed by the Host!
                    if let Ok(mut restored) =
                        serde_json::from_str::<PlayerInventory>(&inventory_json)
                    {
                        restored.normalize_slots();
                        *inv = restored;
                        coop.last_synced_inv_json = inventory_json;
                        toast.show("Restored your saved inventory from Host!", 3.0);
                    }
                }
            }
            NetEvent::StartDescent {
                level_seed,
                depth_tier,
            } => {
                if !coop.is_host_or_solo() {
                    coop.level_seed = level_seed;
                    coop.local_edits.clear();
                    elevator.start_descent(&world_res.0, level_seed, depth_tier);
                    toast.show(
                        format!("Descending to Depth Tier {depth_tier}..."),
                        3.5,
                    );
                }
            }
            NetEvent::ElevatorStateSync {
                phase,
                platform_y_m,
                phase_timer_s,
                depth_tier,
                cave_seed,
            } => {
                if !coop.is_host_or_solo() {
                    let prev_phase = elevator.phase;
                    elevator.apply_network_sync(
                        phase,
                        platform_y_m,
                        phase_timer_s,
                        depth_tier,
                        cave_seed,
                    );
                    if prev_phase != ElevatorPhase::ShedIdle && phase == ElevatorPhase::ShedIdle {
                        health.reset_for_shed();
                        for ent in &stalker_ents_q {
                            commands.entity(ent).despawn();
                        }
                    }
                }
            }
            NetEvent::RequestElevatorAction { depth_tier } => {
                if coop.is_host_or_solo() {
                    let tier = depth_tier.clamp(1, campaign.save.unlocked_depth_tier.max(1));
                    campaign.save.selected_depth_tier = tier;
                    lever_writer.write(TriggerElevatorLeverEvent);
                }
            }
            NetEvent::WorldItemSpawn(item) => {
                spawn_item_writer.write(SpawnWorldItemEvent(item));
            }
            NetEvent::WorldItemUpdate(state) => {
                let mut found = false;
                for (_, mut item, mut tf) in &mut items_q {
                    if item.id == state.id {
                        item.kind = state.kind.clone();
                        item.vel_m = state.vel_m;
                        item.in_shed = state.in_shed;
                        item.carried_by = state.carried_by.clone();
                        item.active_timer_s = state.active_timer_s;
                        item.settled = state.vel_m.length_squared() < 1e-4;
                        tf.translation = state.pos_m;
                        found = true;
                        break;
                    }
                }
                if !found {
                    spawn_item_writer.write(SpawnWorldItemEvent(state));
                }
            }
            NetEvent::WorldItemRemove { item_id } => {
                remove_item_writer.write(RemoveWorldItemEvent { item_id });
            }
            NetEvent::ItemPose { item_id, pos_m } => {
                for (_, item, mut tf) in &mut items_q {
                    if item.id == item_id {
                        tf.translation = tf.translation.lerp(pos_m, 0.55);
                        break;
                    }
                }
            }
            NetEvent::PlayerHealthSync { player_id, status } => {
                let phase = match status {
                    PlayerHealthStatus::Healthy => LocalHealthPhase::Healthy,
                    PlayerHealthStatus::Downed {
                        bleedout_s,
                        dragged_by,
                    } => LocalHealthPhase::Downed {
                        bleedout_s,
                        dragged_by,
                    },
                    PlayerHealthStatus::Dead => LocalHealthPhase::Dead,
                };
                remote_health.0.insert(player_id, phase);
            }
            NetEvent::PlayerRevived { target_id, .. } => {
                if target_id == coop.local_player_id() {
                    health.revive();
                    toast.show("You were revived by a teammate's Aid Kit!", 3.5);
                } else {
                    remote_health.0.insert(target_id, LocalHealthPhase::Healthy);
                }
            }
            NetEvent::PlayerCarrySync {
                carrier_id,
                target_id,
                target_feet_m,
                active,
            } => {
                let local_id = coop.local_player_id();
                if target_id == local_id {
                    if let LocalHealthPhase::Downed {
                        ref mut dragged_by, ..
                    } = health.phase
                    {
                        if active {
                            if dragged_by.is_none() {
                                toast.show(
                                    "Being carried by teammate! Whisper to guide or press [Space]/[Q] to drop",
                                    3.2,
                                );
                            }
                            *dragged_by = Some(carrier_id);
                            if let Ok(mut pc) = player_q.single_mut() {
                                pc.feet_pos_m = target_feet_m;
                                pc.velocity_m = Vec3::ZERO;
                            }
                        } else {
                            *dragged_by = None;
                        }
                    }
                } else {
                    if carrier_id == local_id
                        && !active
                        && matches!(
                            inv.carry_state,
                            crate::inventory::CarryHandState::DraggingPlayer { target_id: tid }
                            if tid == target_id
                        )
                    {
                        inv.carry_state = crate::inventory::CarryHandState::Free;
                        toast.show("Teammate dropped out of your carry", 2.0);
                    }
                    if let Some(LocalHealthPhase::Downed { dragged_by, .. }) =
                        remote_health.0.get_mut(&target_id)
                    {
                        *dragged_by = if active { Some(carrier_id) } else { None };
                    }
                    if active && let Some(rp) = coop.remote_players.get_mut(&target_id) {
                        rp.target_feet_m = target_feet_m;
                    }
                }
            }
            NetEvent::CreditsSync {
                credits,
                unlocked_depth_tier,
                winch_overdrive,
                fast_charger,
            } => {
                campaign.save.credits = credits;
                campaign.save.unlocked_depth_tier = unlocked_depth_tier.max(1);
                campaign.save.winch_overdrive = winch_overdrive;
                campaign.save.fast_charger = fast_charger;
            }
            NetEvent::DecoyTriggered { pos_m } => {
                sound_writer.write(PlaySpatialSoundEvent {
                    pos_m,
                    clip: SynthClip::DecoyBuzz,
                    volume: 0.95,
                    pitch: 1.0,
                    ai_event: None,
                });
            }
            NetEvent::MonsterPose {
                monster_id,
                pos_m,
                yaw,
                state,
            } => {
                if !coop.is_host_or_solo() {
                    upsert_remote_stalker(
                        &mut commands,
                        &mut meshes,
                        &mut materials,
                        &mut remote_stalkers_q,
                        monster_id,
                        pos_m,
                        yaw,
                        state,
                    );
                }
            }
            NetEvent::MonsterAttack {
                target_player_id,
                monster_pos_m,
            } => {
                if target_player_id == coop.local_player_id() {
                    hit_writer.write(MonsterHitLocalPlayerEvent { monster_pos_m });
                }
            }
            NetEvent::MonsterDamaged {
                monster_id,
                damage,
                stun_s,
                knockback_dir,
            } => {
                if coop.is_host_or_solo() {
                    strike_writer.write(StrikeStalkerEvent {
                        monster_id,
                        damage,
                        stun_s,
                        knockback_dir,
                        hit_pos_m: Vec3::ZERO,
                    });
                }
            }
            NetEvent::MonsterKilled { monster_id, pos_m } => {
                if !coop.is_host_or_solo() {
                    for (ent, rs) in &remote_stalkers_q {
                        if rs.monster_id == monster_id {
                            commands.entity(ent).despawn();
                        }
                    }
                    sound_writer.write(PlaySpatialSoundEvent {
                        pos_m,
                        clip: SynthClip::MonsterScreech,
                        volume: 0.95,
                        pitch: 0.72,
                        ai_event: None,
                    });
                    toast.show("Stalker eliminated!", 2.6);
                }
            }
            NetEvent::ParkourRecordSubmit {
                player_name,
                time_ms,
            } => {
                if coop.is_host_or_solo() {
                    let outcome = campaign.save.record_parkour_run(&player_name, time_ms);
                    let _ = campaign.save_now();
                    coop.send_parkour_leaderboard_sync(&campaign.save.parkour_leaderboard);
                    let pretty = crate::shed::format_parkour_time_ms(time_ms);
                    if outcome.is_personal_best && outcome.rank == Some(1) {
                        toast.show(
                            format!("{player_name} set a NEW PARKOUR RECORD: {pretty}!"),
                            4.0,
                        );
                    } else {
                        toast.show(format!("{player_name} finished Parkour in {pretty}!"), 3.0);
                    }
                }
            }
            NetEvent::ParkourLeaderboardSync { records } => {
                if !coop.is_host_or_solo() {
                    campaign.save.parkour_leaderboard = records
                        .into_iter()
                        .map(|(player_name, time_ms)| crate::save::ParkourRecord {
                            player_name,
                            time_ms,
                        })
                        .collect();
                }
            }
            NetEvent::VoxelDelta { coord, material } => {
                coop.local_edits.insert(coord, material);
                let changed_pos = {
                    let mut world = world_res.write();
                    if world.set(coord, material) {
                        Some(world.config().voxel_to_world_center(coord))
                    } else {
                        None
                    }
                };
                if let Some(pos_m) = changed_pos {
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
                held_item,
                pickaxe_swing,
                ..
            } => {
                if let Some(rp) = coop.remote_players.get_mut(&player_id) {
                    rp.target_feet_m = feet_pos_m;
                    rp.target_yaw = yaw;
                    rp.target_pitch = pitch;
                    rp.crouching = crouching;
                    rp.flashlight_on = flashlight_on;
                    rp.held_item = held_item;
                    if pickaxe_swing && rp.pickaxe_swing_s <= 0.02 {
                        rp.pickaxe_swing_s = 0.28;
                    }
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
                clear_remote_players(&mut commands, &mut coop, remove_voice);
                remote_health.0.clear();
                for ent in &stalker_ents_q {
                    commands.entity(ent).despawn();
                }
                coop.session = None;
                coop.join_code = None;
                coop.pending_disconnect_s = None;
                coop.status_text = format!("Disconnected: {reason}");
                menu_open.0 = false;
                shop_open.0 = false;
                settings_open.0 = false;
                *screen = AppScreen::MainMenu;
            }
        }
    }
}

/// Continuously syncs a Guest's inventory changes up to the Host so the Host's campaign save
/// always retains the Guest's exact belt, battery pouch, and wearables even across disconnects.
fn sync_guest_inventory_to_host_system(
    time: Res<Time>,
    screen: Res<AppScreen>,
    inv: Res<PlayerInventory>,
    mut coop: ResMut<CoopState>,
) {
    if *screen != AppScreen::InGame
        || !coop.is_connected()
        || coop.is_host_or_solo()
        || coop.pending_disconnect_s.is_some()
    {
        return;
    }
    coop.inv_sync_timer.tick(time.delta());
    if !coop.inv_sync_timer.just_finished() {
        return;
    }
    if let Ok(json) = serde_json::to_string(&*inv)
        && json != coop.last_synced_inv_json
    {
        coop.sync_local_profile(&inv);
    }
}

/// Finishes a graceful guest disconnect after letting the final `PlayerProfileSync` flush.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn pending_disconnect_flush_system(
    time: Res<Time>,
    mut commands: Commands,
    mut coop: ResMut<CoopState>,
    mut screen: ResMut<AppScreen>,
    mut menu_open: ResMut<CoopMenuOpen>,
    mut shop_open: ResMut<ShopOverlayOpen>,
    mut settings_open: ResMut<SettingsMenuOpen>,
    mut remote_health: ResMut<RemoteHealthMap>,
    stalker_ents_q: Query<Entity, Or<(With<BlindStalker>, With<RemoteSyncedStalker>)>>,
    mut remove_voice: MessageWriter<RemoveVoiceSpeakerEvent>,
) {
    let Some(ref mut rem) = coop.pending_disconnect_s else {
        return;
    };
    *rem -= time.delta_secs();
    if *rem <= 0.0 {
        clear_remote_players(&mut commands, &mut coop, &mut remove_voice);
        remote_health.0.clear();
        for ent in &stalker_ents_q {
            commands.entity(ent).despawn();
        }
        coop.session = None;
        coop.join_code = None;
        coop.pending_disconnect_s = None;
        coop.status_text =
            "Disconnected — Returned to Main Menu (your inventory stays saved on Host)".into();
        menu_open.0 = false;
        shop_open.0 = false;
        settings_open.0 = false;
        *screen = AppScreen::MainMenu;
    }
}

#[allow(clippy::too_many_arguments)]
fn send_local_pose_system(
    time: Res<Time>,
    screen: Res<AppScreen>,
    inv: Res<PlayerInventory>,
    vm_anim: Option<Res<ViewmodelAnimationState>>,
    mut coop: ResMut<CoopState>,
    player_q: Query<&PlayerController>,
    fl_q: Query<&PlayerFlashlight>,
) {
    if *screen != AppScreen::InGame || !coop.is_connected() {
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
        .map(|f| f.active_beam)
        .unwrap_or(false);
    let held_item = if inv.is_hands_occupied() {
        HeldItemNetKind::TwoHandedCarry
    } else {
        match inv.active_item() {
            Some(BeltItem::Flashlight) => HeldItemNetKind::Flashlight,
            Some(BeltItem::Flares { .. }) => HeldItemNetKind::GlowFlare,
            Some(BeltItem::AcousticFoam { .. }) => HeldItemNetKind::AcousticFoam,
            Some(BeltItem::NoiseDecoys { .. }) => HeldItemNetKind::NoiseDecoy,
            Some(BeltItem::AidKit) => HeldItemNetKind::AidKit,
            Some(BeltItem::BonusLoot { .. }) => HeldItemNetKind::BonusCrystal,
            None => HeldItemNetKind::None,
        }
    };
    let pickaxe_swing = vm_anim.as_ref().is_some_and(|a| a.swing_timer_s > 0.0);

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
            held_item,
            pickaxe_swing,
        });
    }
}

fn interpolate_remote_players_system(
    time: Res<Time>,
    mut coop: ResMut<CoopState>,
    mut tf_q: Query<&mut Transform>,
    mut light_q: Query<&mut SpotLight>,
    mut vis_q: Query<&mut Visibility>,
) {
    let dt = time.delta_secs().min(0.05);
    let alpha = (dt * 14.0).clamp(0.0, 1.0);

    for rp in coop.remote_players.values_mut() {
        rp.current_feet_m = rp.current_feet_m.lerp(rp.target_feet_m, alpha);
        rp.pickaxe_swing_s = (rp.pickaxe_swing_s - dt).max(0.0);

        if let Ok(mut tf) = tf_q.get_mut(rp.entity) {
            tf.translation = rp.current_feet_m;
            tf.rotation = Quat::from_rotation_y(rp.target_yaw);
        }
        if let Ok(mut head_tf) = tf_q.get_mut(rp.head_entity) {
            let head_y = if rp.crouching { 1.05 } else { 1.46 };
            head_tf.translation.y = head_y;
            head_tf.rotation = Quat::from_rotation_x(rp.target_pitch);
        }
        let shoulder_y = if rp.crouching { 0.80 } else { 1.06 };
        let two_handed = rp.held_item == HeldItemNetKind::TwoHandedCarry;

        if let Ok(mut left_tf) = tf_q.get_mut(rp.left_arm_entity) {
            left_tf.translation.y = shoulder_y;
            if two_handed {
                left_tf.rotation = Quat::from_euler(
                    EulerRot::YXZ,
                    -0.25,
                    0.82 + rp.target_pitch * 0.40,
                    0.0,
                );
            } else {
                let swing_t = (rp.pickaxe_swing_s / 0.28).clamp(0.0, 1.0);
                let swing_arc = (swing_t * std::f32::consts::PI).sin() * 1.15;
                left_tf.rotation = Quat::from_euler(
                    EulerRot::YXZ,
                    0.06,
                    0.42 + rp.target_pitch * 0.35 + swing_arc,
                    0.0,
                );
            }
        }
        if let Ok(mut right_tf) = tf_q.get_mut(rp.right_arm_entity) {
            right_tf.translation.y = shoulder_y;
            if two_handed {
                right_tf.rotation = Quat::from_euler(
                    EulerRot::YXZ,
                    0.25,
                    0.82 + rp.target_pitch * 0.40,
                    0.0,
                );
            } else if rp.held_item == HeldItemNetKind::None {
                right_tf.rotation = Quat::from_rotation_x(0.10);
            } else {
                right_tf.rotation = Quat::from_euler(
                    EulerRot::YXZ,
                    -0.08,
                    0.68 + rp.target_pitch * 0.65,
                    0.0,
                );
            }
        }
        if let Ok(mut pick_vis) = vis_q.get_mut(rp.pickaxe_entity) {
            let desired = if two_handed {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
            if *pick_vis != desired {
                *pick_vis = desired;
            }
        }
        for &(kind, tool_ent) in &rp.tool_entities {
            if let Ok(mut vis) = vis_q.get_mut(tool_ent) {
                let desired = if !two_handed && rp.held_item == kind {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
                if *vis != desired {
                    *vis = desired;
                }
            }
        }
        if let Ok(mut lens_vis) = vis_q.get_mut(rp.fl_lens_on_entity) {
            let desired = if !two_handed
                && rp.held_item == HeldItemNetKind::Flashlight
                && rp.flashlight_on
            {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            if *lens_vis != desired {
                *lens_vis = desired;
            }
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
