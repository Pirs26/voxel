//! Week 3 Core Extraction Loop orchestration plugin:
//! - Subnautica-style `[R]` + scroll-wheel battery selector & `[Q]` physical dropping/sharing
//! - Seamless zero-loading elevator descent (async cave gen) & 30s loud extraction winch
//! - Physical Heavy Relics (1p loud scrape-drag vs 2p quiet lift), Bonus Loot & Noise Decoys
//! - Blind Acoustic Stalker AI driven strictly by `voxel_acoustics::evaluate_ai_hearing`
//! - Downed state, teammate dragging, 4s Aid Kit revive, Team Wipe & persistent Shed saves

use bevy::{
    ecs::system::SystemParam,
    input::mouse::MouseWheel,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use voxel_acoustics::{AcousticEvent, AcousticEventKind, Bands3};
use voxel_audio_dsp::SynthClip;
use voxel_net::{
    ElevatorPhase, MonsterBehaviorState, PlayerHealthStatus, WorldItemKind, WorldItemNetState,
};

use crate::{
    ai_blind::{
        BlindStalker, RemoteSyncedStalker, StalkerReaction, StalkerState,
        process_acoustic_event_for_stalker, spawn_stalker_visual, step_stalker_ai,
    },
    audio_bridge::{
        AcousticBeaconState, AcousticTelemetry, PlaySpatialSoundEvent, RemoveVoiceSpeakerEvent,
    },
    elevator::{
        ELEVATOR_CAGE_HALF_EXTENT_M, ELEVATOR_DOOR_CLOSED_Y_M, ELEVATOR_DOOR_OPEN_Y_M,
        ElevatorArrivedCaveEvent, ElevatorBulbLight, ElevatorCageRoot, ElevatorDoorVisual,
        ElevatorReturnedShedEvent, ElevatorState, TriggerElevatorLeverEvent,
        compute_ambient_brightness_for_depth, spawn_elevator_cage, step_elevator_state,
    },
    health::{
        LocalHealthPhase, MonsterHitLocalPlayerEvent, PlayerHealthState, RESCUE_RANGE_M,
        REVIVE_CHANNEL_S, RemoteHealthMap, ReviveCompletedEvent, TeamWipeEvent, check_team_wiped,
    },
    inventory::{CarryHandState, PlayerInventory, ShopCatalogItem, purchase_shop_item},
    net_bridge::{CoopMenuOpen, CoopState, clear_remote_players},
    physics::aabb_intersects_world,
    player::PlayerController,
    relic::{
        CHARGER_PAD_RADIUS_M, CHARGER_PAD_RATE_PER_S, DECOY_ACTIVE_DURATION_S,
        DECOY_PULSE_INTERVAL_S, ITEM_INTERACT_RANGE_M, RELIC_SCRAPE_INTERVAL_S,
        RemoveWorldItemEvent, SpawnWorldItemEvent, WorldItemEntity, WorldItemRegistry,
        belt_item_to_world_kind, create_cave_relic_states, default_starter_shed_items,
        is_heavy_relic, is_pos_in_shed, relic_carry_speed_mult, relic_is_scraping,
        saved_to_world_kind, spawn_world_item_visual, to_net_state,
        try_pickup_world_item_into_inventory, world_item_display_name, world_kind_to_saved,
    },
    save::{CampaignState, SavedPlayerProfile, SavedWorldItem},
    settings::{AppScreen, SettingsMenuOpen},
    shed::{
        ShedStation, ShedStationState, ShopOverlayOpen, detect_nearest_station,
        spawn_shed_landmarks,
    },
    voxel_render::{LevelInfoResource, WorldResource},
};

/// Triggered by Main Menu / Pause Menu UI to enter Solo, Host a Lobby, or Disconnect back to Menu.
#[derive(Message, Clone, Copy, Debug)]
pub enum SessionLifecycleEvent {
    StartSolo,
    StartHost,
    DisconnectToMenu,
}

/// Triggered by Shed Shop UI buttons to buy gear, switch depth tier, or manually save.
#[derive(Message, Clone, Copy, Debug)]
pub enum ShopActionEvent {
    Buy(ShopCatalogItem),
    SelectDepthTier(u8),
    ManualSave,
    Close,
}

/// Ephemeral toast notification displayed on the HUD (e.g., purchase result, extraction payout).
#[derive(Resource, Default, Clone, Debug)]
pub struct GameplayToast {
    pub text: String,
    pub timer_s: f32,
}

impl GameplayToast {
    pub fn show(&mut self, msg: impl Into<String>, secs: f32) {
        self.text = msg.into();
        self.timer_s = secs;
    }
}

/// Timers for periodic voice-to-AI emission, monster pose broadcasts, and health sync.
#[derive(Resource)]
struct GameplayTimers {
    voice_ai_timer_s: f32,
    monster_net_timer_s: f32,
    item_net_timer_s: f32,
    last_synced_health: LocalHealthPhase,
}

impl Default for GameplayTimers {
    fn default() -> Self {
        Self {
            voice_ai_timer_s: 0.0,
            monster_net_timer_s: 0.0,
            item_net_timer_s: 0.0,
            last_synced_health: LocalHealthPhase::Healthy,
        }
    }
}

pub struct GameplayPlugin;

impl Plugin for GameplayPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<TriggerElevatorLeverEvent>()
            .add_message::<ElevatorArrivedCaveEvent>()
            .add_message::<ElevatorReturnedShedEvent>()
            .add_message::<SpawnWorldItemEvent>()
            .add_message::<RemoveWorldItemEvent>()
            .add_message::<MonsterHitLocalPlayerEvent>()
            .add_message::<TeamWipeEvent>()
            .add_message::<ReviveCompletedEvent>()
            .add_message::<ShopActionEvent>()
            .add_message::<SessionLifecycleEvent>()
            .init_resource::<PlayerInventory>()
            .init_resource::<PlayerHealthState>()
            .init_resource::<RemoteHealthMap>()
            .init_resource::<WorldItemRegistry>()
            .init_resource::<ShedStationState>()
            .init_resource::<ShopOverlayOpen>()
            .init_resource::<GameplayToast>()
            .init_resource::<GameplayTimers>()
            .add_systems(Startup, setup_week3_world)
            .add_systems(
                Update,
                (
                    toast_tick_system,
                    handle_session_lifecycle_system,
                    handle_shop_actions_system,
                    inventory_input_system,
                    station_and_item_interact_system,
                    elevator_runtime_system,
                    world_items_runtime_system,
                    blind_stalker_ai_system,
                    health_and_wipe_system,
                )
                    .chain(),
            );
    }
}

fn setup_week3_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    level_info: Res<LevelInfoResource>,
    mut coop: ResMut<CoopState>,
    mut inv: ResMut<PlayerInventory>,
    mut item_reg: ResMut<WorldItemRegistry>,
) {
    // 1. Load persistent host campaign save (`saves/default.json`) & local player profile
    let mut campaign = CampaignState::load_default();
    let profile = campaign
        .save
        .get_or_create_player(&coop.local_name, coop.visor_idx);
    coop.local_name = profile.player_name.clone();
    coop.visor_idx = profile.visor_idx;
    *inv = profile.inventory;

    // 2. Initialize seamless mineshaft elevator docked in Base Shed
    let elevator = ElevatorState::from_level_info(&level_info.0, coop.level_seed);
    spawn_elevator_cage(&mut commands, &mut meshes, &mut materials, &elevator);
    commands.insert_resource(elevator);

    // 3. Spawn Base Shed & Cave station props (Shop Terminal, Charger Bench, Lever, Winch)
    spawn_shed_landmarks(&mut commands, &mut meshes, &mut materials, &level_info.0);

    // 4. Restore persisted Base Shed world items (or place starter workbench items for fresh saves)
    let shed_items = if campaign.save.shed_items.is_empty() && campaign.save.dives_completed == 0 {
        let starters = default_starter_shed_items(level_info.0.shed_workbench_pos_m);
        campaign.save.shed_items = starters.clone();
        starters
    } else {
        campaign.save.shed_items.clone()
    };

    for saved in shed_items {
        let id = item_reg.alloc_id();
        let net_state = WorldItemNetState {
            id,
            kind: saved_to_world_kind(&saved.kind),
            pos_m: Vec3::from_array(saved.pos_m),
            vel_m: Vec3::ZERO,
            in_shed: true,
            carried_by: Vec::new(),
            active_timer_s: None,
        };
        spawn_world_item_visual(&mut commands, &mut meshes, &mut materials, &net_state);
    }

    commands.insert_resource(campaign);
}

fn toast_tick_system(time: Res<Time>, mut toast: ResMut<GameplayToast>) {
    if toast.timer_s > 0.0 {
        toast.timer_s = (toast.timer_s - time.delta_secs()).max(0.0);
        if toast.timer_s <= 0.0 {
            toast.text.clear();
        }
    }
}

#[derive(SystemParam)]
struct LifecycleMenus<'w> {
    screen: ResMut<'w, AppScreen>,
    menu_open: ResMut<'w, CoopMenuOpen>,
    shop_open: ResMut<'w, ShopOverlayOpen>,
    settings_open: ResMut<'w, SettingsMenuOpen>,
}

#[derive(SystemParam)]
struct LifecycleState<'w> {
    coop: ResMut<'w, CoopState>,
    campaign: ResMut<'w, CampaignState>,
    inv: ResMut<'w, PlayerInventory>,
    health: ResMut<'w, PlayerHealthState>,
    remote_health: ResMut<'w, RemoteHealthMap>,
    elevator: ResMut<'w, ElevatorState>,
    item_reg: ResMut<'w, WorldItemRegistry>,
    toast: ResMut<'w, GameplayToast>,
    beacon: Res<'w, AcousticBeaconState>,
    level_info: Res<'w, LevelInfoResource>,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn handle_session_lifecycle_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut events: MessageReader<SessionLifecycleEvent>,
    mut menus: LifecycleMenus,
    mut st: LifecycleState,
    mut player_q: Query<&mut PlayerController>,
    items_q: Query<(Entity, &WorldItemEntity, &Transform)>,
    stalkers_q: Query<Entity, Or<(With<BlindStalker>, With<RemoteSyncedStalker>)>>,
    mut remove_voice: MessageWriter<RemoveVoiceSpeakerEvent>,
) {
    let screen = &mut *menus.screen;
    let menu_open = &mut *menus.menu_open;
    let shop_open = &mut *menus.shop_open;
    let settings_open = &mut *menus.settings_open;
    let coop = &mut *st.coop;
    let campaign = &mut *st.campaign;
    let inv = &mut *st.inv;
    let health = &mut *st.health;
    let remote_health = &mut *st.remote_health;
    let elevator = &mut *st.elevator;
    let item_reg = &mut *st.item_reg;
    let toast = &mut *st.toast;
    let beacon = &*st.beacon;
    let level_info = &*st.level_info;
    for ev in events.read().copied() {
        match ev {
            SessionLifecycleEvent::StartSolo | SessionLifecycleEvent::StartHost => {
                let is_host = matches!(ev, SessionLifecycleEvent::StartHost);

                clear_remote_players(&mut commands, coop, &mut remove_voice);
                remote_health.0.clear();
                for ent in &stalkers_q {
                    commands.entity(ent).despawn();
                }
                for (ent, _, _) in &items_q {
                    commands.entity(ent).despawn();
                }

                // Load persistent host campaign & restore local player profile by Player Name
                let mut loaded = CampaignState::load_default();
                let mut profile = loaded
                    .save
                    .get_or_create_player(&coop.local_name, coop.visor_idx);
                profile.visor_idx = coop.visor_idx;
                loaded.save.upsert_player(profile.clone());
                *inv = profile.inventory;
                inv.normalize_slots();
                inv.carry_state = CarryHandState::Free;
                inv.close_battery_selector();

                let shed_items =
                    if loaded.save.shed_items.is_empty() && loaded.save.dives_completed == 0 {
                        let starters = default_starter_shed_items(level_info.0.shed_workbench_pos_m);
                        loaded.save.shed_items = starters.clone();
                        starters
                    } else {
                        loaded.save.shed_items.clone()
                    };

                *campaign = loaded;
                health.reset_for_shed();
                elevator.reset_to_shed_idle();
                elevator.depth_tier = campaign.save.selected_depth_tier.clamp(1, 3);

                if let Ok(mut pc) = player_q.single_mut() {
                    pc.feet_pos_m = level_info.0.spawn_pos_m;
                    pc.velocity_m = Vec3::ZERO;
                    pc.yaw = std::f32::consts::FRAC_PI_2;
                    pc.pitch = 0.0;
                }

                let mut spawned_states = Vec::with_capacity(shed_items.len());
                for saved in shed_items {
                    let id = item_reg.alloc_id();
                    let net_state = WorldItemNetState {
                        id,
                        kind: saved_to_world_kind(&saved.kind),
                        pos_m: Vec3::from_array(saved.pos_m),
                        vel_m: Vec3::ZERO,
                        in_shed: true,
                        carried_by: Vec::new(),
                        active_timer_s: None,
                    };
                    spawn_world_item_visual(&mut commands, &mut meshes, &mut materials, &net_state);
                    spawned_states.push(net_state);
                }

                if is_host {
                    if coop.start_host(beacon.pos_m) {
                        // Seed HostState with authoritative credits, elevator & Shed items for Welcome
                        coop.send_credits_sync(
                            campaign.save.credits,
                            campaign.save.unlocked_depth_tier,
                        );
                        coop.send_elevator_sync(elevator);
                        for st in spawned_states {
                            coop.send_world_item_spawn(st);
                        }
                        menu_open.0 = false;
                        shop_open.0 = false;
                        settings_open.0 = false;
                        *screen = AppScreen::InGame;
                    }
                } else {
                    coop.session = None;
                    coop.join_code = None;
                    coop.pending_disconnect_s = None;
                    coop.status_text = "Solo Expedition (Offline)".into();
                    menu_open.0 = false;
                    shop_open.0 = false;
                    settings_open.0 = false;
                    *screen = AppScreen::InGame;
                    toast.show(
                        format!(
                            "Welcome back, {}! Pull the elevator lever to descend.",
                            coop.local_name
                        ),
                        3.5,
                    );
                }
            }
            SessionLifecycleEvent::DisconnectToMenu => {
                if coop.is_host_or_solo() {
                    // Persist host inventory, synced guest inventories, credits & Shed items
                    let shed_snapshot: Vec<SavedWorldItem> = items_q
                        .iter()
                        .filter(|(_, item, tf)| {
                            item.in_shed && is_pos_in_shed(tf.translation, &level_info.0)
                        })
                        .map(|(_, item, tf)| SavedWorldItem {
                            kind: world_kind_to_saved(&item.kind),
                            pos_m: tf.translation.to_array(),
                            rot_y: 0.0,
                        })
                        .collect();
                    campaign.save.shed_items = shed_snapshot;
                    campaign.save.upsert_player(SavedPlayerProfile {
                        player_name: coop.local_name.clone(),
                        visor_idx: coop.visor_idx,
                        inventory: inv.clone(),
                    });
                    let _ = campaign.save_now();

                    clear_remote_players(&mut commands, coop, &mut remove_voice);
                    remote_health.0.clear();
                    for ent in &stalkers_q {
                        commands.entity(ent).despawn();
                    }
                    coop.session = None;
                    coop.join_code = None;
                    coop.pending_disconnect_s = None;
                    coop.status_text = "Session saved & closed — Back in Main Menu".into();
                    menu_open.0 = false;
                    shop_open.0 = false;
                    settings_open.0 = false;
                    *screen = AppScreen::MainMenu;
                } else {
                    // Guest: push one final inventory snapshot to Host and flush briefly before drop
                    coop.sync_local_profile(inv);
                    coop.pending_disconnect_s = Some(0.18);
                    toast.show("Saving inventory on Host & disconnecting...", 1.5);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_shop_actions_system(
    mut actions: MessageReader<ShopActionEvent>,
    mut inv: ResMut<PlayerInventory>,
    mut campaign: ResMut<CampaignState>,
    mut shop_open: ResMut<ShopOverlayOpen>,
    mut toast: ResMut<GameplayToast>,
    mut cursor_q: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut coop: ResMut<CoopState>,
    items_q: Query<(&WorldItemEntity, &Transform)>,
    level_info: Res<LevelInfoResource>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
    player_q: Query<&PlayerController>,
) {
    let player_pos = player_q
        .single()
        .map(|pc| pc.feet_pos_m + Vec3::Y * 1.2)
        .unwrap_or(level_info.0.shop_terminal_pos_m);

    for ev in actions.read().copied() {
        match ev {
            ShopActionEvent::Buy(catalog_item) => {
                match purchase_shop_item(&mut inv, &mut campaign.save.credits, catalog_item) {
                    Ok(msg) => {
                        toast.show(msg, 3.2);
                        campaign.dirty = true;
                        if coop.is_host_or_solo() {
                            persist_campaign_snapshot(
                                &mut campaign,
                                &coop,
                                &inv,
                                &items_q,
                                &level_info.0,
                            );
                        } else {
                            coop.sync_local_profile(&inv);
                        }
                        coop.send_credits_sync(
                            campaign.save.credits,
                            campaign.save.unlocked_depth_tier,
                        );
                        sound_writer.write(PlaySpatialSoundEvent {
                            pos_m: player_pos,
                            clip: SynthClip::BatteryClick,
                            volume: 0.65,
                            pitch: 1.15,
                            ai_event: None,
                        });
                    }
                    Err(err) => {
                        toast.show(err, 2.8);
                    }
                }
            }
            ShopActionEvent::SelectDepthTier(tier) => {
                let tier = tier.clamp(1, 3);
                if tier <= campaign.save.unlocked_depth_tier {
                    campaign.save.selected_depth_tier = tier;
                    campaign.dirty = true;
                    toast.show(format!("Selected Depth Tier {tier} for next dive"), 2.5);
                } else {
                    toast.show(
                        format!(
                            "Depth Tier {tier} locked — extract from Tier {} first!",
                            tier - 1
                        ),
                        3.0,
                    );
                }
            }
            ShopActionEvent::ManualSave => {
                if coop.is_host_or_solo() {
                    persist_campaign_snapshot(&mut campaign, &coop, &inv, &items_q, &level_info.0);
                    toast.show(campaign.last_status.clone(), 3.0);
                } else {
                    coop.sync_local_profile(&inv);
                    toast.show("Synced your inventory to Host's campaign save!", 2.8);
                }
            }
            ShopActionEvent::Close => {
                shop_open.0 = false;
                if let Ok(mut cursor) = cursor_q.single_mut() {
                    cursor.grab_mode = CursorGrabMode::Locked;
                    cursor.visible = false;
                }
            }
        }
    }
}

#[derive(SystemParam)]
struct InventoryInputRes<'w> {
    screen: Res<'w, AppScreen>,
    menu_open: Res<'w, CoopMenuOpen>,
    shop_open: Res<'w, ShopOverlayOpen>,
    settings_open: Res<'w, SettingsMenuOpen>,
    health: Res<'w, PlayerHealthState>,
    coop: Res<'w, CoopState>,
    level_info: Res<'w, LevelInfoResource>,
    inv: ResMut<'w, PlayerInventory>,
    item_reg: ResMut<'w, WorldItemRegistry>,
    toast: ResMut<'w, GameplayToast>,
}

/// Handles belt hotkeys (`1`–`5`), Subnautica-style `[R]` + Scroll Wheel battery swapper,
/// `[Q]` physical battery/item dropping, and `[T]` throwable Noise Decoys.
#[allow(clippy::too_many_arguments)]
fn inventory_input_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel_reader: MessageReader<MouseWheel>,
    mut res: InventoryInputRes,
    player_q: Query<(&PlayerController, &Transform)>,
    mut items_q: Query<(&mut WorldItemEntity, &Transform)>,
    mut spawn_item_writer: MessageWriter<SpawnWorldItemEvent>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
) {
    let screen = &res.screen;
    let menu_open = &res.menu_open;
    let shop_open = &res.shop_open;
    let settings_open = &res.settings_open;
    let health = &res.health;
    let coop = &res.coop;
    let level_info = &res.level_info;
    let inv = &mut res.inv;
    let item_reg = &mut res.item_reg;
    let toast = &mut res.toast;
    if **screen != AppScreen::InGame
        || menu_open.0
        || shop_open.0
        || settings_open.0
        || !health.is_healthy()
    {
        wheel_reader.clear();
        inv.close_battery_selector();
        return;
    }

    let Ok((pc, cam_tf)) = player_q.single() else {
        wheel_reader.clear();
        return;
    };
    let forward = cam_tf.forward().as_vec3();
    let local_id = coop.local_player_id();

    // 1. Active Belt Slot Selection (`1`..`5`)
    for (idx, key) in [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
    ]
    .into_iter()
    .enumerate()
    {
        if keys.just_pressed(key) && idx < inv.belt_capacity {
            inv.select_slot(idx);
        }
    }

    // 2. Subnautica-style `[R]` Battery Pouch Selector + Scroll Wheel + `[Q]` share-drop
    let mut scroll_delta = 0i32;
    for ev in wheel_reader.read() {
        if ev.y > 0.01 {
            scroll_delta -= 1;
        } else if ev.y < -0.01 {
            scroll_delta += 1;
        }
    }

    if inv.battery_selector.open {
        if scroll_delta != 0 {
            inv.scroll_battery_selector(scroll_delta);
        }
        if keys.just_pressed(KeyCode::ArrowUp) {
            inv.scroll_battery_selector(-1);
        }
        if keys.just_pressed(KeyCode::ArrowDown) {
            inv.scroll_battery_selector(1);
        }

        if keys.just_pressed(KeyCode::KeyR) || keys.just_pressed(KeyCode::Enter) {
            if let Some(new_charge) = inv.swap_selected_battery() {
                toast.show(
                    format!("Swapped battery cell ({:.0}%)", new_charge * 100.0),
                    2.0,
                );
                sound_writer.write(PlaySpatialSoundEvent {
                    pos_m: cam_tf.translation,
                    clip: SynthClip::BatteryClick,
                    volume: 0.55,
                    pitch: 1.0,
                    ai_event: Some(AcousticEvent {
                        pos_m: cam_tf.translation,
                        energy: Bands3::new(0.15, 0.35, 0.45),
                        sharpness: 0.7,
                        kind: AcousticEventKind::ItemImpact,
                    }),
                });
            }
            inv.close_battery_selector();
            return;
        }

        if keys.just_pressed(KeyCode::KeyQ) {
            if let Some(cell) = inv.drop_selected_pouch_battery() {
                let drop_pos = cam_tf.translation + forward * 0.65;
                let drop_vel = forward * 3.8 + Vec3::Y * 1.4;
                let id = item_reg.alloc_id();
                let net_state = WorldItemNetState {
                    id,
                    kind: WorldItemKind::BatteryCell {
                        charge: cell.charge,
                    },
                    pos_m: drop_pos,
                    vel_m: drop_vel,
                    in_shed: is_pos_in_shed(drop_pos, &level_info.0),
                    carried_by: Vec::new(),
                    active_timer_s: None,
                };
                coop.send_world_item_spawn(net_state.clone());
                spawn_item_writer.write(SpawnWorldItemEvent(net_state));
                toast.show(
                    format!("Dropped Battery ({}%) for teammates", cell.charge_pct()),
                    2.0,
                );
                sound_writer.write(PlaySpatialSoundEvent {
                    pos_m: drop_pos,
                    clip: SynthClip::BatteryClick,
                    volume: 0.65,
                    pitch: 0.9,
                    ai_event: Some(AcousticEvent {
                        pos_m: drop_pos,
                        energy: Bands3::new(0.4, 0.9, 1.2),
                        sharpness: 0.85,
                        kind: AcousticEventKind::ItemImpact,
                    }),
                });
            }
            if inv.battery_pouch.is_empty() {
                inv.close_battery_selector();
            }
            return;
        }

        if keys.just_pressed(KeyCode::Escape) {
            inv.close_battery_selector();
            return;
        }
    } else if keys.just_pressed(KeyCode::KeyR) {
        inv.open_battery_selector();
        return;
    }

    // 3. Normal `[Q]`: release Heavy Relic / stop dragging / drop active belt item physically
    if keys.just_pressed(KeyCode::KeyQ) {
        match inv.carry_state {
            CarryHandState::HeavyRelic { item_id } => {
                release_carried_relic(
                    item_id,
                    local_id,
                    forward * 2.2 + Vec3::Y * 0.8,
                    inv,
                    &mut items_q,
                    coop,
                    &mut sound_writer,
                );
                toast.show("Dropped Heavy Relic!", 1.8);
            }
            CarryHandState::DraggingPlayer { .. } => {
                inv.carry_state = CarryHandState::Free;
                toast.show("Released teammate", 1.5);
            }
            CarryHandState::Free => {
                let loaded_charge = inv.loaded_battery_charge();
                if let Some(dropped_item) = inv.drop_active_belt_item() {
                    let kind = belt_item_to_world_kind(&dropped_item, loaded_charge);
                    let drop_pos = cam_tf.translation + forward * 0.65;
                    let drop_vel = forward * 3.4 + Vec3::Y * 1.1;
                    let id = item_reg.alloc_id();
                    let net_state = WorldItemNetState {
                        id,
                        kind: kind.clone(),
                        pos_m: drop_pos,
                        vel_m: drop_vel,
                        in_shed: is_pos_in_shed(drop_pos, &level_info.0),
                        carried_by: Vec::new(),
                        active_timer_s: None,
                    };
                    coop.send_world_item_spawn(net_state.clone());
                    spawn_item_writer.write(SpawnWorldItemEvent(net_state));
                    toast.show(format!("Dropped {}", world_item_display_name(&kind)), 1.8);
                    sound_writer.write(PlaySpatialSoundEvent {
                        pos_m: drop_pos,
                        clip: SynthClip::BatteryClick,
                        volume: 0.6,
                        pitch: 0.85,
                        ai_event: Some(AcousticEvent {
                            pos_m: drop_pos,
                            energy: Bands3::new(0.5, 1.0, 1.2),
                            sharpness: 0.8,
                            kind: AcousticEventKind::ItemImpact,
                        }),
                    });
                }
            }
        }
    }

    // 4. `[T]` Throw an active Noise Decoy to lure Blind Acoustic Stalkers!
    if keys.just_pressed(KeyCode::KeyT) && !inv.is_hands_occupied() {
        if inv.consume_decoy() {
            let origin = cam_tf.translation + forward * 0.6;
            let vel = forward * 11.0 + Vec3::Y * 2.5;
            let id = item_reg.alloc_id();
            let net_state = WorldItemNetState {
                id,
                kind: WorldItemKind::NoiseDecoy { count: 1 },
                pos_m: origin,
                vel_m: vel,
                in_shed: is_pos_in_shed(origin, &level_info.0),
                carried_by: Vec::new(),
                active_timer_s: Some(DECOY_ACTIVE_DURATION_S),
            };
            coop.send_world_item_spawn(net_state.clone());
            spawn_item_writer.write(SpawnWorldItemEvent(net_state));
            toast.show("Noise Decoy thrown! (12s loud lure)", 2.2);
            sound_writer.write(PlaySpatialSoundEvent {
                pos_m: pc.feet_pos_m + Vec3::Y * 1.2,
                clip: SynthClip::BatteryClick,
                volume: 0.6,
                pitch: 1.3,
                ai_event: None,
            });
        } else {
            toast.show("No Noise Decoys in belt — grab some in the Shed!", 2.2);
        }
    }
}

#[derive(SystemParam)]
struct InteractMenus<'w> {
    screen: Res<'w, AppScreen>,
    menu_open: Res<'w, CoopMenuOpen>,
    shop_open: ResMut<'w, ShopOverlayOpen>,
    settings_open: Res<'w, SettingsMenuOpen>,
}

#[derive(SystemParam)]
struct InteractState<'w> {
    level_info: Res<'w, LevelInfoResource>,
    elevator: Res<'w, ElevatorState>,
    campaign: Res<'w, CampaignState>,
    coop: Res<'w, CoopState>,
    inv: ResMut<'w, PlayerInventory>,
    health: ResMut<'w, PlayerHealthState>,
    remote_health: ResMut<'w, RemoteHealthMap>,
    station_state: ResMut<'w, ShedStationState>,
    item_reg: ResMut<'w, WorldItemRegistry>,
    toast: ResMut<'w, GameplayToast>,
}

/// Contextual `[E]` interaction for Shed stations, Cave winch, physical items, Heavy Relics,
/// downed teammates, `[B]` Shed Shop shortcut, and `[H]` 4s Aid Kit revive channel.
#[allow(clippy::too_many_arguments)]
fn station_and_item_interact_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut menus: InteractMenus,
    mut cursor_q: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut st: InteractState,
    player_q: Query<(&PlayerController, &Transform)>,
    mut items_q: Query<(&mut WorldItemEntity, &Transform)>,
    mut lever_writer: MessageWriter<TriggerElevatorLeverEvent>,
    mut remove_item_writer: MessageWriter<RemoveWorldItemEvent>,
    mut revive_writer: MessageWriter<ReviveCompletedEvent>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
) {
    let screen = &menus.screen;
    let menu_open = &menus.menu_open;
    let shop_open = &mut menus.shop_open;
    let settings_open = &menus.settings_open;
    let level_info = &st.level_info;
    let elevator = &st.elevator;
    let campaign = &st.campaign;
    let coop = &st.coop;
    let inv = &mut st.inv;
    let health = &mut st.health;
    let remote_health = &mut st.remote_health;
    let station_state = &mut st.station_state;
    let item_reg = &mut st.item_reg;
    let toast = &mut st.toast;
    if **screen != AppScreen::InGame || menu_open.0 || settings_open.0 {
        return;
    }

    let Ok((pc, cam_tf)) = player_q.single() else {
        return;
    };
    let player_pos = pc.feet_pos_m;
    let in_shed = is_pos_in_shed(player_pos, &level_info.0);

    // Quick `[B]` toggle for Shed Shop while inside the Base Shed
    if keys.just_pressed(KeyCode::KeyB) && in_shed && health.is_healthy() {
        shop_open.0 = !shop_open.0;
        if let Ok(mut cursor) = cursor_q.single_mut() {
            if shop_open.0 {
                cursor.grab_mode = CursorGrabMode::None;
                cursor.visible = true;
            } else {
                cursor.grab_mode = CursorGrabMode::Locked;
                cursor.visible = false;
            }
        }
        return;
    }

    if shop_open.0 {
        if keys.just_pressed(KeyCode::Escape) {
            shop_open.0 = false;
            if let Ok(mut cursor) = cursor_q.single_mut() {
                cursor.grab_mode = CursorGrabMode::Locked;
                cursor.visible = false;
            }
        }
        return;
    }

    if !health.is_healthy() {
        station_state.nearest = None;
        station_state.prompt = None;
        item_reg.nearest_item_prompt = None;
        return;
    }

    // 1. Detect nearest Shed/Cave station
    let nearest_station = detect_nearest_station(
        player_pos,
        &level_info.0,
        elevator.phase,
        campaign.save.selected_depth_tier,
    );
    if let Some((st, prompt)) = nearest_station {
        station_state.nearest = Some(st);
        station_state.prompt = Some(prompt);
    } else {
        station_state.nearest = None;
        station_state.prompt = None;
    }

    // 2. Detect nearest interactable world item (excluding already-active thrown decoys)
    let local_id = coop.local_player_id();
    let mut best_item: Option<(f32, u32, WorldItemKind, usize)> = None;
    for (item, tf) in &items_q {
        if item.active_timer_s.is_some() {
            continue;
        }
        if item.carried_by.contains(&local_id) {
            continue;
        }
        let d = tf.translation.distance(cam_tf.translation);
        if d <= ITEM_INTERACT_RANGE_M && best_item.as_ref().is_none_or(|(bd, _, _, _)| d < *bd) {
            best_item = Some((d, item.id, item.kind.clone(), item.carried_by.len()));
        }
    }

    // 3. Detect nearest downed remote teammate within rescue range
    let mut nearest_downed: Option<(f32, u16, String)> = None;
    for (&pid, rp) in &coop.remote_players {
        if matches!(
            remote_health.0.get(&pid),
            Some(LocalHealthPhase::Downed { .. })
        ) {
            let d = rp.current_feet_m.distance(player_pos);
            if d <= RESCUE_RANGE_M && nearest_downed.as_ref().is_none_or(|(bd, _, _)| d < *bd) {
                nearest_downed = Some((d, pid, rp.info.name.clone()));
            }
        }
    }

    // Build contextual HUD prompt
    item_reg.nearest_item_prompt = if let CarryHandState::HeavyRelic { .. } = inv.carry_state {
        Some("[E / Q] Release Heavy Relic".to_string())
    } else if let CarryHandState::DraggingPlayer { .. } = inv.carry_state {
        Some("[E / Q] Release Downed Teammate".to_string())
    } else if let Some((_, _, ref name)) = nearest_downed {
        Some(format!(
            "[E] Drag {name} to safety  |  Hold [H] Revive with Aid Kit (4s)"
        ))
    } else if let Some((_, _, ref kind, carriers)) = best_item {
        let label = world_item_display_name(kind);
        if is_heavy_relic(kind) {
            if carriers == 0 {
                Some(format!("[E] Drag {label} (Solo = loud scrape; 2P = quiet lift)"))
            } else {
                Some(format!("[E] Assist Duo-Lift {label} (Quiet 75% speed!)"))
            }
        } else {
            Some(format!("[E] Pick up {label}"))
        }
    } else {
        None
    };

    // 4. Hold `[H]` to channel consumable Aid Kit revive on a downed teammate
    if keys.pressed(KeyCode::KeyH) && !inv.is_hands_occupied() {
        if let Some((_, target_pid, ref name)) = nearest_downed {
            let has_aid = inv
                .belt
                .iter()
                .any(|s| matches!(s, Some(crate::inventory::BeltItem::AidKit)));
            if has_aid {
                if health.reviving_target != Some(target_pid) {
                    health.reviving_target = Some(target_pid);
                    health.revive_progress_s = 0.0;
                    sound_writer.write(PlaySpatialSoundEvent {
                        pos_m: player_pos + Vec3::Y * 0.8,
                        clip: SynthClip::AidKitUse,
                        volume: 0.48,
                        pitch: 1.0,
                        ai_event: Some(AcousticEvent {
                            pos_m: player_pos,
                            energy: Bands3::new(0.2, 0.45, 0.55),
                            sharpness: 0.3,
                            kind: AcousticEventKind::ItemImpact,
                        }),
                    });
                }
                health.revive_progress_s += time.delta_secs();
                if health.revive_progress_s >= REVIVE_CHANNEL_S {
                    if inv.consume_aid_kit() {
                        remote_health.0.insert(target_pid, LocalHealthPhase::Healthy);
                        coop.send_player_revived(target_pid, local_id);
                        revive_writer.write(ReviveCompletedEvent {
                            target_player_id: target_pid,
                        });
                        toast.show(format!("Revived {name} with Aid Kit!"), 3.0);
                    }
                    health.revive_progress_s = 0.0;
                    health.reviving_target = None;
                }
            } else if keys.just_pressed(KeyCode::KeyH) {
                toast.show("Need an Aid Kit in your belt to revive!", 2.5);
            }
        } else {
            health.revive_progress_s = 0.0;
            health.reviving_target = None;
        }
    } else {
        health.revive_progress_s = 0.0;
        health.reviving_target = None;
    }

    // 5. Contextual `[E]` press
    if !keys.just_pressed(KeyCode::KeyE) || inv.battery_selector.open {
        return;
    }

    // Priority 1: release carried Heavy Relic or dragged teammate
    match inv.carry_state {
        CarryHandState::HeavyRelic { item_id } => {
            release_carried_relic(
                item_id,
                local_id,
                cam_tf.forward().as_vec3() * 1.4,
                inv,
                &mut items_q,
                coop,
                &mut sound_writer,
            );
            return;
        }
        CarryHandState::DraggingPlayer { .. } => {
            inv.carry_state = CarryHandState::Free;
            return;
        }
        CarryHandState::Free => {}
    }

    // Priority 2: Shed / Cave interactive stations
    if let Some(st) = station_state.nearest {
        match st {
            ShedStation::ElevatorLeverShed | ShedStation::ElevatorWinchCave => {
                if coop.is_host_or_solo() {
                    lever_writer.write(TriggerElevatorLeverEvent);
                } else {
                    coop.send_request_elevator_action(campaign.save.selected_depth_tier);
                    toast.show("Requested elevator activation from Host...", 2.0);
                }
                return;
            }
            ShedStation::ShopTerminal => {
                shop_open.0 = true;
                if let Ok(mut cursor) = cursor_q.single_mut() {
                    cursor.grab_mode = CursorGrabMode::None;
                    cursor.visible = true;
                }
                return;
            }
            ShedStation::ChargerBench => {
                let charged = inv.recharge_all_batteries(1.0);
                if charged {
                    toast.show("All pouch & loaded batteries recharged to 100%!", 2.5);
                } else {
                    toast.show("All carried batteries are already at 100%", 2.0);
                }
                sound_writer.write(PlaySpatialSoundEvent {
                    pos_m: level_info.0.charger_bench_pos_m + Vec3::Y * 0.9,
                    clip: SynthClip::BatteryClick,
                    volume: 0.6,
                    pitch: 1.25,
                    ai_event: None,
                });
                return;
            }
        }
    }

    // Priority 3: Start dragging a nearby downed teammate
    if let Some((_, target_pid, ref name)) = nearest_downed {
        inv.carry_state = CarryHandState::DraggingPlayer {
            target_id: target_pid,
        };
        toast.show(format!("Dragging {name} — move behind cover/foam!"), 2.4);
        return;
    }

    // Priority 4: Grab Heavy Relic or pick up world item
    if let Some((_, item_id, kind, _)) = best_item {
        if is_heavy_relic(&kind) {
            for (mut item, tf) in &mut items_q {
                if item.id == item_id {
                    if !item.carried_by.contains(&local_id) {
                        item.carried_by.push(local_id);
                    }
                    item.settled = false;
                    inv.carry_state = CarryHandState::HeavyRelic { item_id };
                    coop.send_world_item_update(to_net_state(&item, tf.translation));
                    let msg = if item.carried_by.len() >= 2 {
                        "Duo-Lifting Heavy Relic! (Quiet 75% speed)"
                    } else {
                        "Dragging Heavy Relic solo (LOUD stone scraping!)"
                    };
                    toast.show(msg, 2.5);
                    break;
                }
            }
        } else {
            match try_pickup_world_item_into_inventory(inv, &kind) {
                Ok(msg) => {
                    toast.show(msg, 2.0);
                    coop.send_world_item_remove(item_id);
                    remove_item_writer.write(RemoveWorldItemEvent { item_id });
                    sound_writer.write(PlaySpatialSoundEvent {
                        pos_m: cam_tf.translation,
                        clip: SynthClip::BatteryClick,
                        volume: 0.5,
                        pitch: 1.1,
                        ai_event: None,
                    });
                }
                Err(err) => {
                    toast.show(err, 2.4);
                }
            }
        }
    }
}

fn release_carried_relic(
    item_id: u32,
    local_id: u16,
    toss_vel: Vec3,
    inv: &mut PlayerInventory,
    items_q: &mut Query<(&mut WorldItemEntity, &Transform)>,
    coop: &CoopState,
    sound_writer: &mut MessageWriter<PlaySpatialSoundEvent>,
) {
    inv.carry_state = CarryHandState::Free;
    for (mut item, tf) in items_q.iter_mut() {
        if item.id == item_id {
            item.carried_by.retain(|&id| id != local_id);
            if item.carried_by.is_empty() {
                item.vel_m = toss_vel;
                item.settled = false;
                sound_writer.write(PlaySpatialSoundEvent {
                    pos_m: tf.translation,
                    clip: SynthClip::HeavyImpact,
                    volume: 0.9,
                    pitch: 0.95,
                    ai_event: Some(AcousticEvent {
                        pos_m: tf.translation,
                        energy: Bands3::new(3.2, 2.8, 2.2),
                        sharpness: 0.9,
                        kind: AcousticEventKind::ItemImpact,
                    }),
                });
            }
            coop.send_world_item_update(to_net_state(&item, tf.translation));
            break;
        }
    }
}

#[derive(SystemParam)]
struct ElevatorWorldRes<'w> {
    screen: Res<'w, AppScreen>,
    world_res: Res<'w, WorldResource>,
    level_info: ResMut<'w, LevelInfoResource>,
    elevator: ResMut<'w, ElevatorState>,
    campaign: ResMut<'w, CampaignState>,
    coop: ResMut<'w, CoopState>,
    inv: ResMut<'w, PlayerInventory>,
    health: ResMut<'w, PlayerHealthState>,
    item_reg: ResMut<'w, WorldItemRegistry>,
    toast: ResMut<'w, GameplayToast>,
    ambient: ResMut<'w, GlobalAmbientLight>,
}

#[derive(SystemParam)]
struct ElevatorEvents<'w, 's> {
    lever_events: MessageReader<'w, 's, TriggerElevatorLeverEvent>,
    arrived_writer: MessageWriter<'w, ElevatorArrivedCaveEvent>,
    returned_writer: MessageWriter<'w, ElevatorReturnedShedEvent>,
    sound_writer: MessageWriter<'w, PlaySpatialSoundEvent>,
}

/// Drives the seamless zero-loading elevator descent, async underground cave swap,
/// 30s loud winch climax, cage visuals, player/relic platform riding, and Shed cash-in/save!
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn elevator_runtime_system(
    time: Res<Time>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut res: ElevatorWorldRes,
    mut evs: ElevatorEvents,
    mut player_q: Query<(&mut PlayerController, &mut Transform), Without<ElevatorCageRoot>>,
    mut cage_q: Query<&mut Transform, (With<ElevatorCageRoot>, Without<ElevatorDoorVisual>)>,
    mut door_q: Query<&mut Transform, (With<ElevatorDoorVisual>, Without<ElevatorCageRoot>)>,
    mut bulb_q: Query<&mut PointLight, With<ElevatorBulbLight>>,
    mut items_q: Query<
        (Entity, &mut WorldItemEntity, &mut Transform),
        (Without<PlayerController>, Without<ElevatorCageRoot>, Without<ElevatorDoorVisual>),
    >,
    stalkers_q: Query<Entity, Or<(With<BlindStalker>, With<RemoteSyncedStalker>)>>,
) {
    let screen = &res.screen;
    let world_res = &res.world_res;
    let level_info = &mut res.level_info;
    let elevator = &mut res.elevator;
    let campaign = &mut res.campaign;
    let coop = &mut res.coop;
    let inv = &mut res.inv;
    let health = &mut res.health;
    let item_reg = &mut res.item_reg;
    let toast = &mut res.toast;
    let ambient = &mut res.ambient;
    let lever_events = &mut evs.lever_events;
    let arrived_writer = &mut evs.arrived_writer;
    let returned_writer = &mut evs.returned_writer;
    let sound_writer = &mut evs.sound_writer;

    if **screen != AppScreen::InGame {
        lever_events.clear();
        ambient.brightness = 46.0;
        return;
    }
    let dt = time.delta_secs().min(0.05);

    // 1. Handle elevator lever / winch activation on Host / Solo
    let mut lever_triggered = false;
    for _ in lever_events.read() {
        lever_triggered = true;
    }

    if lever_triggered && coop.is_host_or_solo() {
        match elevator.phase {
            ElevatorPhase::ShedIdle => {
                let tier = campaign.save.selected_depth_tier.clamp(1, 3);
                let next_seed = coop
                    .level_seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407 ^ (campaign.save.dives_completed as u64));
                coop.level_seed = next_seed;
                coop.local_edits.clear();

                // Pull local player safely into cage if they pulled the lever from just outside
                if let Ok((mut pc, _)) = player_q.single_mut() {
                    let dist_to_shaft = Vec2::new(
                        pc.feet_pos_m.x - elevator.center_xz_m.x,
                        pc.feet_pos_m.z - elevator.center_xz_m.y,
                    )
                    .length();
                    if dist_to_shaft < 6.0 && !elevator.is_pos_inside_cage(pc.feet_pos_m) {
                        pc.feet_pos_m = Vec3::new(
                            elevator.center_xz_m.x + 0.8,
                            elevator.platform_y_m + 0.05,
                            elevator.center_xz_m.y,
                        );
                    }
                }

                elevator.start_descent(&world_res.0, next_seed, tier);
                coop.send_start_descent(next_seed, tier);
                coop.send_elevator_sync(elevator);
                toast.show(
                    format!("Descending to Depth Tier {tier}... (Generating cave seamlessly)"),
                    3.5,
                );
                sound_writer.write(PlaySpatialSoundEvent {
                    pos_m: elevator.cage_center_pos_m(),
                    clip: SynthClip::DoorSlam,
                    volume: 0.9,
                    pitch: 0.95,
                    ai_event: None,
                });
            }
            ElevatorPhase::CaveIdle => {
                elevator.start_winch();
                coop.send_elevator_sync(elevator);
                toast.show(
                    "EXTRACTION WINCH ACTIVATED! Loud machinery drawing predators — 30s!",
                    4.0,
                );
            }
            _ => {}
        }
    }

    // 2. Poll background underground cave generation and swap seamlessly into WorldResource!
    if let Some((new_world, new_info)) = elevator.poll_generated_cave() {
        *world_res.write() = new_world;
        level_info.0 = new_info;
    }

    // 3. Step authoritative elevator state machine on Host / Solo (guests apply network sync)
    let prev_y = elevator.platform_y_m;
    let gen_ready = !elevator.is_gen_pending();
    let step_out = if coop.is_host_or_solo() {
        step_elevator_state(elevator, dt, gen_ready)
    } else {
        Default::default()
    };

    if let Some((clip, vol, pitch, ai_ev)) = step_out.emit_sound {
        sound_writer.write(PlaySpatialSoundEvent {
            pos_m: elevator.cage_center_pos_m(),
            clip,
            volume: vol,
            pitch,
            ai_event: ai_ev,
        });
    }

    let delta_y = elevator.platform_y_m - prev_y;

    // 4. Ride moving elevator cage smoothly (player + items inside cage!)
    let mut player_y = elevator.platform_y_m;
    if let Ok((mut pc, _)) = player_q.single_mut() {
        let dx = pc.feet_pos_m.x - elevator.center_xz_m.x;
        let dz = pc.feet_pos_m.z - elevator.center_xz_m.y;
        let in_xz = dx.abs() <= ELEVATOR_CAGE_HALF_EXTENT_M + 0.1
            && dz.abs() <= ELEVATOR_CAGE_HALF_EXTENT_M + 0.1;

        if in_xz && (pc.feet_pos_m.y - prev_y).abs() <= 1.25 {
            pc.feet_pos_m.y = (pc.feet_pos_m.y + delta_y).max(elevator.platform_y_m);
            if pc.velocity_m.y < 0.0 && pc.feet_pos_m.y <= elevator.platform_y_m + 0.05 {
                pc.feet_pos_m.y = elevator.platform_y_m;
                pc.velocity_m.y = 0.0;
                pc.grounded = true;
            }
            if elevator.doors_closed {
                let clamp_r = ELEVATOR_CAGE_HALF_EXTENT_M - 0.35;
                pc.feet_pos_m.x = elevator.center_xz_m.x + dx.clamp(-clamp_r, clamp_r);
                pc.feet_pos_m.z = elevator.center_xz_m.y + dz.clamp(-clamp_r, clamp_r);
            }
        }
        player_y = pc.feet_pos_m.y;
    }

    if delta_y.abs() > 1e-5 {
        for (_, item, mut tf) in &mut items_q {
            if item.carried_by.is_empty() {
                let dx = (tf.translation.x - elevator.center_xz_m.x).abs();
                let dz = (tf.translation.z - elevator.center_xz_m.y).abs();
                if dx <= ELEVATOR_CAGE_HALF_EXTENT_M
                    && dz <= ELEVATOR_CAGE_HALF_EXTENT_M
                    && (tf.translation.y - prev_y).abs() <= 2.2
                {
                    tf.translation.y += delta_y;
                }
            }
        }
    }

    // 5. Update cage visuals, sliding door, flickering cage bulb, and ambient daylight falloff
    if let Ok(mut cage_tf) = cage_q.single_mut() {
        cage_tf.translation.y = elevator.platform_y_m;
    }
    if let Ok(mut door_tf) = door_q.single_mut() {
        let target_y = if elevator.doors_closed {
            ELEVATOR_DOOR_CLOSED_Y_M
        } else {
            ELEVATOR_DOOR_OPEN_Y_M
        };
        door_tf.translation.y += (target_y - door_tf.translation.y) * (dt * 7.0).min(1.0);
    }
    if let Ok(mut bulb) = bulb_q.single_mut() {
        let t = time.elapsed_secs();
        let flicker = match elevator.phase {
            ElevatorPhase::Descending | ElevatorPhase::Ascending => {
                if (t * 17.0).sin() > -0.65 {
                    0.85 + 0.15 * (t * 31.0).cos()
                } else {
                    0.22
                }
            }
            ElevatorPhase::WinchActive => {
                // Tense pulsing amber-red alarm bulb during loud winch holdout!
                bulb.color = Color::srgb(1.0, 0.32, 0.18);
                0.45 + 0.55 * (t * 6.5).sin().abs()
            }
            _ => {
                bulb.color = Color::srgb(1.0, 0.82, 0.52);
                1.0
            }
        };
        bulb.intensity = 42_000.0 * flicker;
    }
    ambient.brightness = compute_ambient_brightness_for_depth(
        elevator.platform_y_m,
        elevator.top_y_m,
        elevator.bottom_y_m,
        player_y,
    );

    // 6. Arrival in Cave: spawn Heavy Relics, Bonus Loot & Blind Acoustic Stalkers!
    if step_out.arrived_cave {
        arrived_writer.write(ElevatorArrivedCaveEvent {
            depth_tier: elevator.depth_tier,
            cave_seed: elevator.cave_seed,
        });
        toast.show(
            format!(
                "Arrived at Depth Tier {} — Stay quiet! Find Heavy Relics and return to the shaft.",
                elevator.depth_tier
            ),
            4.5,
        );

        // Despawn any leftover cave-only entities before spawning fresh ones
        for ent in &stalkers_q {
            commands.entity(ent).despawn();
        }
        for (ent, item, _) in &items_q {
            if !item.in_shed {
                commands.entity(ent).despawn();
            }
        }

        // Spawn depth-scaled relics & bonus loot
        let cave_items = create_cave_relic_states(&level_info.0, &mut item_reg.next_id);
        for net_state in cave_items {
            coop.send_world_item_spawn(net_state.clone());
            spawn_world_item_visual(&mut commands, &mut meshes, &mut materials, &net_state);
        }

        // Spawn Blind Acoustic Stalker(s) deep in the cave
        for (idx, &spawn_pos) in level_info.0.monster_spawns_m.iter().enumerate() {
            let monster_id = (idx + 1) as u16;
            let ent = spawn_stalker_visual(&mut commands, &mut meshes, &mut materials, spawn_pos);
            commands
                .entity(ent)
                .insert(BlindStalker::new(monster_id, spawn_pos, elevator.depth_tier));
        }

        coop.send_elevator_sync(elevator);
    }

    // 7. Return to Base Shed: cash in relics inside cage + carried bonus loot, heal, unlock tier, save!
    if step_out.returned_shed {
        let mut payout = inv.take_all_bonus_loot_value();
        let mut relics_extracted = 0u32;

        for (ent, item, tf) in &items_q {
            if !item.in_shed {
                if elevator.is_pos_inside_cage(tf.translation) {
                    match item.kind {
                        WorldItemKind::HeavyRelic { value, .. } => {
                            payout = payout.saturating_add(value);
                            relics_extracted += 1;
                        }
                        WorldItemKind::BonusLoot { value } => {
                            payout = payout.saturating_add(value);
                        }
                        _ => {}
                    }
                }
                coop.send_world_item_remove(item.id);
                commands.entity(ent).despawn();
            }
        }
        for ent in &stalkers_q {
            commands.entity(ent).despawn();
        }

        inv.carry_state = CarryHandState::Free;
        health.reset_for_shed();
        campaign.save.credits = campaign.save.credits.saturating_add(payout);
        campaign.save.dives_completed = campaign.save.dives_completed.saturating_add(1);
        if relics_extracted > 0 && elevator.depth_tier >= campaign.save.unlocked_depth_tier {
            campaign.save.unlocked_depth_tier = (elevator.depth_tier + 1).min(3);
        }
        campaign.dirty = true;

        // Snapshot Shed items & player profile and auto-save!
        let shed_items_snapshot: Vec<SavedWorldItem> = items_q
            .iter()
            .filter(|(_, item, tf)| item.in_shed && is_pos_in_shed(tf.translation, &level_info.0))
            .map(|(_, item, tf)| SavedWorldItem {
                kind: world_kind_to_saved(&item.kind),
                pos_m: tf.translation.to_array(),
                rot_y: 0.0,
            })
            .collect();
        campaign.save.shed_items = shed_items_snapshot;
        campaign.save.upsert_player(SavedPlayerProfile {
            player_name: coop.local_name.clone(),
            visor_idx: coop.visor_idx,
            inventory: inv.clone(),
        });
        let _ = campaign.save_now();

        coop.send_credits_sync(campaign.save.credits, campaign.save.unlocked_depth_tier);
        coop.send_elevator_sync(elevator);

        returned_writer.write(ElevatorReturnedShedEvent {
            depth_tier: elevator.depth_tier,
        });
        toast.show(
            format!(
                "Extracted! +{payout} credits ({relics_extracted} relic(s)) — Campaign Auto-Saved!"
            ),
            5.0,
        );
    }

    // 8. Periodic host elevator sync while moving or winching
    if coop.is_host_or_solo()
        && coop.is_connected()
        && !matches!(elevator.phase, ElevatorPhase::ShedIdle | ElevatorPhase::CaveIdle)
    {
        elevator.sync_timer_s += dt;
        if elevator.sync_timer_s >= 0.12 {
            elevator.sync_timer_s = 0.0;
            coop.send_elevator_sync(elevator);
        }
    }
}

/// Simulates physical world items: Heavy Relic carry/scrape acoustics, dropped item gravity/bounces,
/// active Noise Decoy loud acoustic lures, and passive Shed Charger Pad battery charging.
#[allow(clippy::too_many_arguments)]
fn world_items_runtime_system(
    time: Res<Time>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    world_res: Res<WorldResource>,
    level_info: Res<LevelInfoResource>,
    coop: Res<CoopState>,
    mut inv: ResMut<PlayerInventory>,
    mut item_reg: ResMut<WorldItemRegistry>,
    mut timers: ResMut<GameplayTimers>,
    mut spawn_events: MessageReader<SpawnWorldItemEvent>,
    mut remove_events: MessageReader<RemoveWorldItemEvent>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
    player_q: Query<(&PlayerController, &Transform)>,
    mut items_q: Query<(Entity, &mut WorldItemEntity, &mut Transform), Without<PlayerController>>,
) {
    let dt = time.delta_secs().min(0.05);

    // 1. Spawn newly requested/networked world items
    for SpawnWorldItemEvent(net_state) in spawn_events.read() {
        let already_exists = items_q.iter().any(|(_, item, _)| item.id == net_state.id);
        if !already_exists {
            spawn_world_item_visual(&mut commands, &mut meshes, &mut materials, net_state);
        }
    }

    // 2. Despawn removed world items
    for RemoveWorldItemEvent { item_id } in remove_events.read() {
        for (ent, item, _) in &items_q {
            if item.id == *item_id {
                if matches!(inv.carry_state, CarryHandState::HeavyRelic { item_id: cid } if cid == *item_id)
                {
                    inv.carry_state = CarryHandState::Free;
                }
                commands.entity(ent).despawn();
            }
        }
    }

    let local_id = coop.local_player_id();
    let local_player = player_q.single().ok();
    let world = world_res.read();

    item_reg.active_carry_speed_mult = 1.0;
    item_reg.active_carry_is_scraping = false;

    timers.item_net_timer_s += dt;
    let send_item_poses = coop.is_host_or_solo() && coop.is_connected() && timers.item_net_timer_s >= 0.08;
    if send_item_poses {
        timers.item_net_timer_s = 0.0;
    }

    for (ent, mut item, mut tf) in &mut items_q {
        // A. Active thrown Noise Decoy pulses loud acoustic lures every 0.90s for 12s!
        if let Some(mut remaining) = item.active_timer_s {
            remaining -= dt;
            item.active_timer_s = Some(remaining);
            item.decoy_pulse_timer_s -= dt;
            if item.decoy_pulse_timer_s <= 0.0 {
                item.decoy_pulse_timer_s = DECOY_PULSE_INTERVAL_S;
                let pos = tf.translation;
                sound_writer.write(PlaySpatialSoundEvent {
                    pos_m: pos,
                    clip: SynthClip::DecoyBuzz,
                    volume: 0.95,
                    pitch: 1.0 + ((remaining * 3.0).sin() * 0.08),
                    ai_event: Some(AcousticEvent {
                        pos_m: pos,
                        energy: Bands3::new(2.4, 4.4, 4.8),
                        sharpness: 0.95,
                        kind: AcousticEventKind::DecoyNoise,
                    }),
                });
            }
            if remaining <= 0.0 {
                commands.entity(ent).despawn();
                continue;
            }
        }

        // B. Carried Heavy Relic positioning + 1P loud stone scraping vs 2P quiet lift
        if !item.carried_by.is_empty() && is_heavy_relic(&item.kind) {
            let carrier_count = item.carried_by.len();
            let mut sum_pos = Vec3::ZERO;
            let mut valid_carriers = 0usize;
            let mut local_moving = false;

            for &cid in &item.carried_by {
                if cid == local_id {
                    if let Some((pc, cam_tf)) = local_player {
                        let fwd = cam_tf.forward().as_vec3();
                        let horiz_fwd = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
                        let hold_y = if carrier_count >= 2 { 0.85 } else { 0.35 };
                        sum_pos += pc.feet_pos_m + horiz_fwd * 1.15 + Vec3::Y * hold_y;
                        valid_carriers += 1;
                        local_moving = pc.velocity_m.xz().length() > 0.35;
                    }
                } else if let Some(rp) = coop.remote_players.get(&cid) {
                    let hold_y = if carrier_count >= 2 { 0.85 } else { 0.35 };
                    sum_pos += rp.current_feet_m + Vec3::Y * hold_y;
                    valid_carriers += 1;
                }
            }

            if valid_carriers > 0 {
                let target = sum_pos / valid_carriers as f32;
                tf.translation = tf.translation.lerp(target, (dt * 12.0).min(1.0));
                item.vel_m = Vec3::ZERO;
                item.settled = false;
            }

            if item.carried_by.contains(&local_id) {
                item_reg.active_carry_speed_mult = relic_carry_speed_mult(carrier_count);
                let scraping = relic_is_scraping(carrier_count);
                item_reg.active_carry_is_scraping = scraping;

                if scraping && local_moving {
                    item_reg.scrape_timer_s += dt;
                    if item_reg.scrape_timer_s >= RELIC_SCRAPE_INTERVAL_S {
                        item_reg.scrape_timer_s = 0.0;
                        sound_writer.write(PlaySpatialSoundEvent {
                            pos_m: tf.translation,
                            clip: SynthClip::RelicScrape,
                            volume: 0.85,
                            pitch: 0.94,
                            ai_event: Some(AcousticEvent {
                                pos_m: tf.translation,
                                energy: Bands3::new(2.8, 2.4, 1.8),
                                sharpness: 0.45,
                                kind: AcousticEventKind::RelicScrape,
                            }),
                        });
                    }
                }
            }

            if send_item_poses {
                coop.send_item_pose(item.id, tf.translation);
            }
            continue;
        }

        // C. Passive battery recharging when placed near the Base Shed Charger Pad!
        if let WorldItemKind::BatteryCell { ref mut charge } = item.kind
            && *charge < 1.0
            && tf
                .translation
                .distance(level_info.0.charger_bench_pos_m + Vec3::Y * 0.5)
                <= CHARGER_PAD_RADIUS_M
        {
            *charge = (*charge + CHARGER_PAD_RATE_PER_S * dt).clamp(0.0, 1.0);
        }

        // D. Simple gravity & bounce physics for dropped/thrown items
        if item.settled {
            continue;
        }

        let r = if is_heavy_relic(&item.kind) {
            Vec3::splat(0.32)
        } else {
            Vec3::splat(0.14)
        };
        item.vel_m.y -= 16.0 * dt;
        let mut pos = tf.translation;
        let mut bounced = false;
        let impact_speed = item.vel_m.length();

        for axis in 0..3 {
            let mut next = pos;
            next[axis] += item.vel_m[axis] * dt;
            if aabb_intersects_world(&world, next - r, next + r) {
                item.vel_m[axis] *= -0.32;
                item.vel_m *= 0.72;
                bounced = true;
            } else {
                pos = next;
            }
        }
        tf.translation = pos;
        item.in_shed = is_pos_in_shed(pos, &level_info.0);

        if bounced && impact_speed > 2.0 {
            let is_heavy = is_heavy_relic(&item.kind);
            let clip = if is_heavy {
                SynthClip::HeavyImpact
            } else {
                SynthClip::BatteryClick
            };
            let vol = (impact_speed / 10.0).clamp(0.25, if is_heavy { 0.95 } else { 0.7 });
            sound_writer.write(PlaySpatialSoundEvent {
                pos_m: pos,
                clip,
                volume: vol,
                pitch: 1.0,
                ai_event: Some(AcousticEvent {
                    pos_m: pos,
                    energy: if is_heavy {
                        Bands3::new(3.2, 2.8, 2.2)
                    } else {
                        Bands3::splat(vol * 1.5)
                    },
                    sharpness: 0.85,
                    kind: AcousticEventKind::ItemImpact,
                }),
            });
        }

        if bounced && item.vel_m.length() < 0.3 {
            item.settled = true;
            item.vel_m = Vec3::ZERO;
        }

        if send_item_poses {
            coop.send_item_pose(item.id, tf.translation);
        }
    }
}

/// Evaluates all acoustic events (SFX + real mic voice above whisper curve) through
/// `voxel_acoustics::evaluate_ai_hearing` to drive Blind Acoustic Stalkers!
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn blind_stalker_ai_system(
    time: Res<Time>,
    screen: Res<AppScreen>,
    world_res: Res<WorldResource>,
    telemetry: Res<AcousticTelemetry>,
    coop: Res<CoopState>,
    health: Res<PlayerHealthState>,
    remote_health: Res<RemoteHealthMap>,
    mut timers: ResMut<GameplayTimers>,
    mut sound_reader: MessageReader<PlaySpatialSoundEvent>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
    mut hit_writer: MessageWriter<MonsterHitLocalPlayerEvent>,
    player_q: Query<(&PlayerController, &Transform)>,
    mut stalkers_q: Query<(&mut BlindStalker, &mut Transform), Without<PlayerController>>,
    mut remote_stalkers_q: Query<
        (&RemoteSyncedStalker, &mut Transform),
        (Without<BlindStalker>, Without<PlayerController>),
    >,
) {
    if *screen != AppScreen::InGame {
        sound_reader.clear();
        return;
    }
    let dt = time.delta_secs().min(0.05);

    // Guest-side smooth interpolation of host-simulated Blind Stalkers
    let alpha = (dt * 12.0).clamp(0.0, 1.0);
    for (rs, mut tf) in &mut remote_stalkers_q {
        tf.translation = tf.translation.lerp(rs.target_pos_m, alpha);
        tf.rotation = Quat::from_rotation_y(rs.target_yaw);
    }

    // Collect acoustic events emitted this frame
    let mut ai_events: Vec<AcousticEvent> = sound_reader
        .read()
        .filter_map(|ev| ev.ai_event)
        .collect();

    // Emit periodic VoiceSpeech AcousticEvent whenever player mic exceeds whisper-safe curve!
    timers.voice_ai_timer_s += dt;
    if timers.voice_ai_timer_s >= 0.25 {
        timers.voice_ai_timer_s = 0.0;
        let mic = telemetry.mic;
        if mic.available
            && mic.gate_open
            && mic.ai_noise_level >= 0.35
            && let Ok((_, cam_tf)) = player_q.single()
        {
            let e = mic.ai_noise_level;
            ai_events.push(AcousticEvent {
                pos_m: cam_tf.translation,
                energy: Bands3::new(e * 0.7, e * 1.4, e * 1.6),
                sharpness: 0.55,
                kind: AcousticEventKind::VoiceSpeech,
            });
        }
    }

    if !coop.is_host_or_solo() || stalkers_q.is_empty() {
        return;
    }

    let local_id = coop.local_player_id();
    let mut living_players: Vec<(u16, Vec3)> = Vec::new();
    if health.is_healthy()
        && let Ok((pc, _)) = player_q.single()
    {
        living_players.push((local_id, pc.feet_pos_m));
    }
    for (&pid, rp) in &coop.remote_players {
        let is_healthy = matches!(
            remote_health.0.get(&pid).unwrap_or(&LocalHealthPhase::Healthy),
            LocalHealthPhase::Healthy
        );
        if is_healthy {
            living_players.push((pid, rp.current_feet_m));
        }
    }

    timers.monster_net_timer_s += dt;
    let send_poses = coop.is_connected() && timers.monster_net_timer_s >= 0.07;
    if send_poses {
        timers.monster_net_timer_s = 0.0;
    }

    let world = world_res.read();
    for (mut stalker, mut tf) in &mut stalkers_q {
        for &ev in &ai_events {
            if let Some(StalkerReaction::AlertHunt) =
                process_acoustic_event_for_stalker(&world, &mut stalker, ev)
            {
                sound_writer.write(PlaySpatialSoundEvent {
                    pos_m: stalker.feet_pos_m + Vec3::Y * 1.6,
                    clip: SynthClip::MonsterScreech,
                    volume: 0.95,
                    pitch: 1.0,
                    ai_event: None,
                });
            }
        }

        let out = step_stalker_ai(&world, &mut stalker, &living_players, dt);
        tf.translation = stalker.feet_pos_m;
        tf.rotation = Quat::from_rotation_y(stalker.yaw);

        if let Some((clip, vol, pitch)) = out.emit_vocal {
            sound_writer.write(PlaySpatialSoundEvent {
                pos_m: stalker.feet_pos_m + Vec3::Y * 1.6,
                clip,
                volume: vol,
                pitch,
                ai_event: None,
            });
        }
        if let Some((step_pos, vol)) = out.emit_footstep {
            sound_writer.write(PlaySpatialSoundEvent {
                pos_m: step_pos + Vec3::Y * 0.15,
                clip: SynthClip::FootstepStone,
                volume: vol,
                pitch: 0.72,
                ai_event: None,
            });
        }
        if let Some((target_pid, monster_pos)) = out.strike_target {
            if target_pid == local_id {
                hit_writer.write(MonsterHitLocalPlayerEvent {
                    monster_pos_m: monster_pos,
                });
            } else {
                coop.send_monster_attack(target_pid, monster_pos);
            }
        }

        if send_poses {
            let net_state = match stalker.state {
                StalkerState::Patrol => MonsterBehaviorState::Patrol,
                StalkerState::Investigate => MonsterBehaviorState::Investigate,
                StalkerState::Hunt => MonsterBehaviorState::Hunt,
                StalkerState::AttackCooldown => MonsterBehaviorState::Attack,
            };
            coop.send_monster_pose(stalker.monster_id, stalker.feet_pos_m, stalker.yaw, net_state);
        }
    }
}

#[derive(SystemParam)]
struct HealthWipeRes<'w> {
    screen: Res<'w, AppScreen>,
    level_info: Res<'w, LevelInfoResource>,
    coop: Res<'w, CoopState>,
    health: ResMut<'w, PlayerHealthState>,
    remote_health: ResMut<'w, RemoteHealthMap>,
    inv: ResMut<'w, PlayerInventory>,
    elevator: ResMut<'w, ElevatorState>,
    campaign: ResMut<'w, CampaignState>,
    timers: ResMut<'w, GameplayTimers>,
    toast: ResMut<'w, GameplayToast>,
}

/// Processes monster strikes on the local player, bleedout countdown, health sync, and Team Wipe!
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn health_and_wipe_system(
    time: Res<Time>,
    mut commands: Commands,
    mut res: HealthWipeRes,
    mut hit_events: MessageReader<MonsterHitLocalPlayerEvent>,
    mut wipe_writer: MessageWriter<TeamWipeEvent>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
    mut player_q: Query<&mut PlayerController>,
    items_q: Query<(Entity, &WorldItemEntity, &Transform)>,
    stalkers_q: Query<Entity, Or<(With<BlindStalker>, With<RemoteSyncedStalker>)>>,
) {
    let screen = &res.screen;
    let level_info = &res.level_info;
    let coop = &res.coop;
    let health = &mut res.health;
    let remote_health = &mut res.remote_health;
    let inv = &mut res.inv;
    let elevator = &mut res.elevator;
    let campaign = &mut res.campaign;
    let timers = &mut res.timers;
    let toast = &mut res.toast;
    if **screen != AppScreen::InGame {
        hit_events.clear();
        return;
    }
    let dt = time.delta_secs().min(0.05);

    for ev in hit_events.read() {
        if health.apply_monster_hit() {
            inv.carry_state = CarryHandState::Free;
            inv.close_battery_selector();
            toast.show(
                "DOWNED! 60s bleed-out — crawl to cover & whisper for an Aid Kit!",
                4.5,
            );
            sound_writer.write(PlaySpatialSoundEvent {
                pos_m: ev.monster_pos_m + Vec3::Y * 1.4,
                clip: SynthClip::HeavyImpact,
                volume: 1.0,
                pitch: 0.78,
                ai_event: None,
            });
        }
    }

    let bled_out = health.tick(dt);
    if bled_out {
        toast.show("You bled out...", 4.0);
    }

    // Sync local health transitions over network
    if health.phase != timers.last_synced_health {
        timers.last_synced_health = health.phase;
        let net_status = match health.phase {
            LocalHealthPhase::Healthy => PlayerHealthStatus::Healthy,
            LocalHealthPhase::Downed {
                bleedout_s,
                dragged_by,
            } => PlayerHealthStatus::Downed {
                bleedout_s,
                dragged_by,
            },
            LocalHealthPhase::Dead => PlayerHealthStatus::Dead,
        };
        coop.send_player_health(net_status);
    }

    // Check Team Wipe (solo dead, or every party member dead)
    if check_team_wiped(&health.phase, &remote_health.0) {
        wipe_writer.write(TeamWipeEvent);
        inv.apply_team_wipe_penalty();
        health.reset_for_shed();
        timers.last_synced_health = LocalHealthPhase::Healthy;
        for status in remote_health.0.values_mut() {
            *status = LocalHealthPhase::Healthy;
        }
        elevator.reset_to_shed_idle();

        if let Ok(mut pc) = player_q.single_mut() {
            pc.feet_pos_m = level_info.0.spawn_pos_m;
            pc.velocity_m = Vec3::ZERO;
        }

        // Despawn cave stalkers & cave items (keep persistent Base Shed items untouched!)
        for ent in &stalkers_q {
            commands.entity(ent).despawn();
        }
        for (ent, item, _) in &items_q {
            if !item.in_shed {
                commands.entity(ent).despawn();
            }
        }

        // Save persistent Shed state & updated profile
        campaign.save.upsert_player(SavedPlayerProfile {
            player_name: coop.local_name.clone(),
            visor_idx: coop.visor_idx,
            inventory: inv.clone(),
        });
        let _ = campaign.save_now();
        toast.show(
            "TEAM WIPE — Returned to Base Shed. Cave loot lost; passive gear & Shed items preserved.",
            5.5,
        );
    }
}

fn persist_campaign_snapshot(
    campaign: &mut CampaignState,
    coop: &CoopState,
    inv: &PlayerInventory,
    items_q: &Query<(&WorldItemEntity, &Transform)>,
    info: &voxel_core::GeneratedLevelInfo,
) {
    campaign.save.shed_items = items_q
        .iter()
        .filter(|(item, tf)| item.in_shed && is_pos_in_shed(tf.translation, info))
        .map(|(item, tf)| SavedWorldItem {
            kind: world_kind_to_saved(&item.kind),
            pos_m: tf.translation.to_array(),
            rot_y: 0.0,
        })
        .collect();
    campaign.save.upsert_player(SavedPlayerProfile {
        player_name: coop.local_name.clone(),
        visor_idx: coop.visor_idx,
        inventory: inv.clone(),
    });
    let _ = campaign.save_now();
}

/// Helper to spawn or update a guest's `RemoteSyncedStalker` from `NetEvent::MonsterPose`.
#[allow(clippy::too_many_arguments)]
pub fn upsert_remote_stalker(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    remote_q: &mut Query<&mut RemoteSyncedStalker>,
    monster_id: u16,
    pos_m: Vec3,
    yaw: f32,
    state: MonsterBehaviorState,
) {
    let st = match state {
        MonsterBehaviorState::Idle | MonsterBehaviorState::Patrol => StalkerState::Patrol,
        MonsterBehaviorState::Investigate => StalkerState::Investigate,
        MonsterBehaviorState::Hunt => StalkerState::Hunt,
        MonsterBehaviorState::Attack => StalkerState::AttackCooldown,
    };
    for mut rs in remote_q.iter_mut() {
        if rs.monster_id == monster_id {
            rs.target_pos_m = pos_m;
            rs.target_yaw = yaw;
            rs.state = st;
            return;
        }
    }
    let ent = spawn_stalker_visual(commands, meshes, materials, pos_m);
    commands.entity(ent).insert(RemoteSyncedStalker {
        monster_id,
        target_pos_m: pos_m,
        target_yaw: yaw,
        state: st,
    });
}
