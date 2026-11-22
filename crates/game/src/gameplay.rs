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
use voxel_core::{Destructibility, raycast_solid};
use voxel_net::{
    ElevatorPhase, MonsterBehaviorState, PlayerHealthStatus, WorldItemKind, WorldItemNetState,
};

use crate::{
    ai_blind::{
        BlindStalker, MonsterArchetype, RemoteSyncedStalker, StalkerHitResult, StalkerReaction,
        StalkerState, apply_stalker_hit, process_acoustic_event_for_stalker, spawn_monster_visual,
        stalker_hit_aabb_for, step_stalker_ai,
    },
    audio_bridge::{
        AcousticBeaconState, AcousticTelemetry, PlaySpatialSoundEvent, RemoveVoiceSpeakerEvent,
    },
    elevator::{
        ELEVATOR_CAGE_HALF_EXTENT_M, ELEVATOR_DOOR_CLOSED_Y_M, ELEVATOR_DOOR_OPEN_Y_M,
        ELEVATOR_WINCH_DURATION_S, ElevatorArrivedCaveEvent, ElevatorBulbLight, ElevatorCageRoot,
        ElevatorDoorVisual, ElevatorReturnedShedEvent, ElevatorState, TriggerElevatorLeverEvent,
        compute_ambient_brightness_for_depth, spawn_elevator_cage, step_elevator_state,
    },
    health::{
        LocalHealthPhase, MonsterHitLocalPlayerEvent, PlayerHealthState, RESCUE_RANGE_M,
        REVIVE_CHANNEL_S, RemoteHealthMap, ReviveCompletedEvent, TeamWipeEvent, check_team_wiped,
    },
    inventory::{CarryHandState, PlayerInventory, ShopCatalogItem, purchase_shop_item},
    net_bridge::{CoopMenuOpen, CoopState, clear_remote_players},
    physics::{aabb_intersects_world, ray_aabb_intersect},
    player::PlayerController,
    relic::{
        CHARGER_PAD_RADIUS_M, CHARGER_PAD_RATE_PER_S, DECOY_ACTIVE_DURATION_S,
        DECOY_PULSE_INTERVAL_S, RELIC_SCRAPE_INTERVAL_S, RemoveWorldItemEvent,
        SpawnWorldItemEvent, WorldItemEntity, WorldItemRegistry, belt_item_to_world_kind,
        create_cave_relic_states, default_starter_shed_items, is_heavy_relic, is_pos_in_shed,
        relic_carry_speed_mult, relic_is_scraping, saved_to_world_kind, spawn_world_item_visual,
        to_net_state, try_pickup_world_item_into_inventory, world_item_display_name,
        world_kind_to_saved,
    },
    save::{CampaignState, SaveSlotBrowser, SavedPlayerProfile, SavedWorldItem},
    settings::{AppScreen, SettingsMenuOpen},
    shed::{
        CrosshairTargetState, INTERACT_RAY_MAX_DIST_M, LookTargetKind, ShedStation,
        ShedStationState, ShopOverlayOpen, TargetAccent, raycast_shed_station,
        spawn_shed_landmarks,
    },
    tools::StrikeStalkerEvent,
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
            .init_resource::<CrosshairTargetState>()
            .init_resource::<ShopOverlayOpen>()
            .init_resource::<SaveSlotBrowser>()
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

/// Counts how many living players (Healthy or Downed, excluding Dead) are currently
/// outside the elevator cage. Both descending and starting the extraction winch require `0`.
pub fn count_living_players_outside_cage(
    elevator: &ElevatorState,
    local_feet_m: Vec3,
    local_alive: bool,
    coop: &CoopState,
    remote_health: &RemoteHealthMap,
) -> usize {
    let mut outside = 0usize;
    if local_alive && !elevator.is_pos_inside_cage(local_feet_m) {
        outside += 1;
    }
    for (&pid, rp) in &coop.remote_players {
        let is_dead = matches!(remote_health.0.get(&pid), Some(LocalHealthPhase::Dead));
        if !is_dead && !elevator.is_pos_inside_cage(rp.current_feet_m) {
            outside += 1;
        }
    }
    outside
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
    save_browser: ResMut<'w, SaveSlotBrowser>,
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
    let save_browser = &mut *st.save_browser;
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

                // Load selected persistent host campaign slot & restore local player profile by Player Name
                let slot_name = save_browser.selected_slot_name().to_string();
                let mut loaded = CampaignState::load_slot(&slot_name);
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
                        toast.show(
                            format!(
                                "Loaded save '{}' ({} cr, Tier {}) — Board elevator & pull interior lever!",
                                campaign.slot_name,
                                campaign.save.credits,
                                campaign.save.selected_depth_tier
                            ),
                            4.0,
                        );
                    }
                } else {
                    coop.session = None;
                    coop.join_code = None;
                    coop.pending_disconnect_s = None;
                    coop.status_text = format!("Solo Expedition (Save: {})", campaign.slot_name);
                    menu_open.0 = false;
                    shop_open.0 = false;
                    settings_open.0 = false;
                    *screen = AppScreen::InGame;
                    toast.show(
                        format!(
                            "Loaded save '{}' ({} cr, Tier {}) — Board elevator & pull interior lever!",
                            campaign.slot_name,
                            campaign.save.credits,
                            campaign.save.selected_depth_tier
                        ),
                        4.0,
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
                    save_browser.refresh();

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
    } else {
        if scroll_delta != 0 && !inv.is_hands_occupied() {
            inv.scroll_active_slot(scroll_delta);
        }
        if keys.just_pressed(KeyCode::KeyR) {
            inv.open_battery_selector();
            return;
        }
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
    world_res: Res<'w, WorldResource>,
    level_info: Res<'w, LevelInfoResource>,
    elevator: Res<'w, ElevatorState>,
    campaign: Res<'w, CampaignState>,
    coop: Res<'w, CoopState>,
    inv: ResMut<'w, PlayerInventory>,
    health: ResMut<'w, PlayerHealthState>,
    remote_health: ResMut<'w, RemoteHealthMap>,
    station_state: ResMut<'w, ShedStationState>,
    crosshair_target: ResMut<'w, CrosshairTargetState>,
    item_reg: ResMut<'w, WorldItemRegistry>,
    toast: ResMut<'w, GameplayToast>,
}

/// Center-of-screen crosshair raycast interaction for Shed stations, interior Elevator Lever,
/// physical items, Heavy Relics, downed teammates, Stalker target readout, `[B]` Shed Shop
/// shortcut, and `[H]` 4s Aid Kit revive channel.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn station_and_item_interact_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut menus: InteractMenus,
    mut cursor_q: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut st: InteractState,
    player_q: Query<(&PlayerController, &Transform)>,
    mut items_q: Query<(&mut WorldItemEntity, &Transform)>,
    stalkers_q: Query<(&Transform, Option<&BlindStalker>, Option<&RemoteSyncedStalker>)>,
    mut lever_writer: MessageWriter<TriggerElevatorLeverEvent>,
    mut remove_item_writer: MessageWriter<RemoveWorldItemEvent>,
    mut revive_writer: MessageWriter<ReviveCompletedEvent>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
) {
    let screen = &menus.screen;
    let menu_open = &menus.menu_open;
    let shop_open = &mut menus.shop_open;
    let settings_open = &menus.settings_open;
    let world_res = &st.world_res;
    let level_info = &st.level_info;
    let elevator = &st.elevator;
    let campaign = &st.campaign;
    let coop = &st.coop;
    let inv = &mut st.inv;
    let health = &mut st.health;
    let remote_health = &mut st.remote_health;
    let station_state = &mut st.station_state;
    let crosshair_target = &mut st.crosshair_target;
    let item_reg = &mut st.item_reg;
    let toast = &mut st.toast;
    if **screen != AppScreen::InGame || menu_open.0 || settings_open.0 {
        crosshair_target.clear();
        return;
    }

    let Ok((pc, cam_tf)) = player_q.single() else {
        crosshair_target.clear();
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
        crosshair_target.clear();
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
        crosshair_target.clear();
        return;
    }

    // 1. Cast voxel-occluded center-of-screen crosshair ray!
    let cam_origin = cam_tf.translation;
    let cam_dir = cam_tf.forward().as_vec3();
    let local_id = coop.local_player_id();
    let voxel_hit = {
        let world = world_res.read();
        raycast_solid(&world, cam_origin, cam_dir, INTERACT_RAY_MAX_DIST_M)
    };
    let ray_limit_m = voxel_hit
        .as_ref()
        .map_or(INTERACT_RAY_MAX_DIST_M, |h| (h.distance_m + 0.08).min(INTERACT_RAY_MAX_DIST_M));

    let missing_outside = count_living_players_outside_cage(
        elevator,
        player_pos,
        health.is_alive(),
        coop,
        remote_health,
    );

    // Compete EVERY interactable object strictly by crosshair ray intersection distance `t_hit`!
    struct RayCandidate {
        t: f32,
        target: LookTargetKind,
        key_badge: String,
        label: String,
        sublabel: String,
        accent: TargetAccent,
    }
    let mut best: Option<RayCandidate> = None;
    let mut consider = |cand: RayCandidate| {
        if best.as_ref().is_none_or(|b| cand.t < b.t) {
            best = Some(cand);
        }
    };

    // 1a. Shed stations & interior Elevator Lever (exact AABB raycast)
    if let Some((st_kind, label, sublabel, t_hit, blocked)) = raycast_shed_station(
        cam_origin,
        cam_dir,
        ray_limit_m,
        &level_info.0,
        elevator.lever_world_pos_m(),
        elevator.phase,
        campaign.save.selected_depth_tier,
        missing_outside,
    ) {
        consider(RayCandidate {
            t: t_hit,
            target: LookTargetKind::Station(st_kind),
            key_badge: if blocked { "WAIT" } else { "E" }.to_string(),
            label,
            sublabel,
            accent: if blocked {
                TargetAccent::Blocked
            } else {
                TargetAccent::Interact
            },
        });
    }

    // 1b. Physical World Items (exact AABB raycast; picking an item resting on a bench wins naturally!)
    for (item, tf) in &items_q {
        if item.active_timer_s.is_some() || item.carried_by.contains(&local_id) {
            continue;
        }
        let half = if is_heavy_relic(&item.kind) {
            Vec3::splat(0.42)
        } else {
            Vec3::splat(0.26)
        };
        if let Some(t_hit) =
            ray_aabb_intersect(cam_origin, cam_dir, tf.translation - half, tf.translation + half, ray_limit_m)
        {
            let label = world_item_display_name(&item.kind);
            let carriers = item.carried_by.len();
            let sublabel = if is_heavy_relic(&item.kind) {
                if carriers == 0 {
                    "Solo drag (loud) • 2P lift (quiet)".to_string()
                } else {
                    "Assist Duo-Lift (quiet)".to_string()
                }
            } else {
                "Pick up".to_string()
            };
            consider(RayCandidate {
                t: t_hit,
                target: LookTargetKind::WorldItem {
                    item_id: item.id,
                    kind: item.kind.clone(),
                    carriers,
                },
                key_badge: "E".to_string(),
                label,
                sublabel,
                accent: TargetAccent::Interact,
            });
        }
    }

    // 1c. Downed remote teammates (raycast target + close-range Aid Kit revive fallback)
    let mut nearby_downed_for_revive: Option<(f32, u16, String)> = None;
    for (&pid, rp) in &coop.remote_players {
        if matches!(
            remote_health.0.get(&pid),
            Some(LocalHealthPhase::Downed { .. })
        ) {
            let d = rp.current_feet_m.distance(player_pos);
            if d <= RESCUE_RANGE_M
                && nearby_downed_for_revive
                    .as_ref()
                    .is_none_or(|(bd, _, _)| d < *bd)
            {
                nearby_downed_for_revive = Some((d, pid, rp.info.name.clone()));
            }
            let center = rp.current_feet_m + Vec3::Y * 0.45;
            let half = Vec3::new(0.48, 0.50, 0.48);
            if let Some(t_hit) =
                ray_aabb_intersect(cam_origin, cam_dir, center - half, center + half, ray_limit_m)
            {
                consider(RayCandidate {
                    t: t_hit,
                    target: LookTargetKind::DownedPlayer {
                        player_id: pid,
                        name: rp.info.name.clone(),
                    },
                    key_badge: "E / H".to_string(),
                    label: format!("Rescue {}", rp.info.name),
                    sublabel: "[E] Drag • Hold [H] Aid Kit (4s)".to_string(),
                    accent: TargetAccent::Interact,
                });
            }
        }
    }

    // 1d. Blind Acoustic Predators (Stalker / Heavy Lurker) within melee `[LMB]` strike range (`3.2m`)
    let melee_limit = ray_limit_m.min(3.2);
    for (stf, host_s, remote_s) in &stalkers_q {
        let (monster_id, archetype, hp, max_hp, stunned) = if let Some(s) = host_s {
            (
                s.monster_id,
                s.archetype,
                s.hp,
                s.max_hp,
                s.state == StalkerState::Stunned,
            )
        } else if let Some(rs) = remote_s {
            let arch = if rs.monster_id >= 2 {
                MonsterArchetype::HeavyLurker
            } else {
                MonsterArchetype::Stalker
            };
            let est_hp = if arch == MonsterArchetype::HeavyLurker {
                5
            } else {
                3
            };
            (
                rs.monster_id,
                arch,
                est_hp,
                est_hp,
                rs.state == StalkerState::Stunned,
            )
        } else {
            continue;
        };
        let (mn, mx) = stalker_hit_aabb_for(stf.translation, archetype);
        if let Some(t_hit) = ray_aabb_intersect(cam_origin, cam_dir, mn, mx, melee_limit) {
            let pips: String = (0..max_hp)
                .map(|i| if i < hp { '●' } else { '○' })
                .collect();
            let title = match archetype {
                MonsterArchetype::Stalker => "Strike Stalker",
                MonsterArchetype::HeavyLurker => "Strike Heavy Lurker",
            };
            let sublabel = if stunned {
                format!("HP {pips} • STUNNED!")
            } else {
                format!("HP {pips} • Stuns & knocks back")
            };
            consider(RayCandidate {
                t: t_hit,
                target: LookTargetKind::Stalker {
                    monster_id,
                    hp,
                    max_hp,
                    stunned,
                },
                key_badge: "LMB".to_string(),
                label: title.to_string(),
                sublabel,
                accent: TargetAccent::Danger,
            });
        }
    }

    drop(consider);
    // 1e. Breakable voxel hint (`CrackedRock` / `AcousticFoam`) if no closer entity was hit
    if best.is_none()
        && !inv.is_hands_occupied()
        && let Some(ref vh) = voxel_hit
        && matches!(
            vh.material.def().destructibility,
            Destructibility::BreakableCracked | Destructibility::PlaceableTool
        )
    {
        let label = match vh.material {
            voxel_core::VoxelId::CrackedRock => "Break Cracked Wall",
            voxel_core::VoxelId::AcousticFoam => "Clear Acoustic Foam",
            _ => "Break Block",
        };
        best = Some(RayCandidate {
            t: vh.distance_m,
            target: LookTargetKind::BreakableVoxel,
            key_badge: "LMB".to_string(),
            label: label.to_string(),
            sublabel: "Opens passage (LOUD)".to_string(),
            accent: TargetAccent::Interact,
        });
    }

    // Update `CrosshairTargetState` (and legacy compatibility prompts)
    station_state.nearest = None;
    station_state.nearest_dist_m = f32::MAX;
    station_state.prompt = None;
    item_reg.nearest_item_prompt = None;

    if let CarryHandState::HeavyRelic { .. } = inv.carry_state {
        crosshair_target.target = None;
        crosshair_target.key_badge = "E / Q".to_string();
        crosshair_target.label = "Drop Heavy Relic".to_string();
        crosshair_target.sublabel = if item_reg.active_carry_is_scraping {
            "SCRAPING LOUDLY".to_string()
        } else {
            "Duo-lift (quiet)".to_string()
        };
        crosshair_target.accent = TargetAccent::Interact;
    } else if let CarryHandState::DraggingPlayer { .. } = inv.carry_state {
        crosshair_target.target = None;
        crosshair_target.key_badge = "E / Q".to_string();
        crosshair_target.label = "Release Teammate".to_string();
        crosshair_target.sublabel.clear();
        crosshair_target.accent = TargetAccent::Interact;
    } else if let Some(cand) = best {
        if let LookTargetKind::Station(st_kind) = cand.target {
            station_state.nearest = Some(st_kind);
            station_state.nearest_dist_m = cand.t;
        }
        crosshair_target.target = Some(cand.target);
        crosshair_target.key_badge = cand.key_badge;
        crosshair_target.label = cand.label;
        crosshair_target.sublabel = cand.sublabel;
        crosshair_target.accent = cand.accent;
    } else {
        crosshair_target.clear();
    }

    // 2. Hold `[H]` to channel consumable Aid Kit revive on aimed/nearby downed teammate
    let revive_target = match &crosshair_target.target {
        Some(LookTargetKind::DownedPlayer { player_id, name }) => {
            Some((*player_id, name.clone()))
        }
        _ => nearby_downed_for_revive.map(|(_, pid, name)| (pid, name)),
    };
    if keys.pressed(KeyCode::KeyH) && !inv.is_hands_occupied() {
        if let Some((target_pid, name)) = revive_target {
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

    // 3. Contextual `[E]` press acts strictly on what the center crosshair is pointing at!
    if !keys.just_pressed(KeyCode::KeyE) || inv.battery_selector.open {
        return;
    }

    // Release carried Heavy Relic or dragged teammate first
    match inv.carry_state {
        CarryHandState::HeavyRelic { item_id } => {
            release_carried_relic(
                item_id,
                local_id,
                cam_dir * 1.4,
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

    let Some(target) = crosshair_target.target.clone() else {
        return;
    };

    match target {
        LookTargetKind::Station(st_kind) => match st_kind {
            ShedStation::ElevatorLeverShed | ShedStation::ElevatorWinchCave => {
                if missing_outside > 0 {
                    toast.show(
                        format!(
                            "All living players must be inside the elevator! ({missing_outside} still outside)"
                        ),
                        2.8,
                    );
                    return;
                }
                if coop.is_host_or_solo() {
                    lever_writer.write(TriggerElevatorLeverEvent);
                } else {
                    coop.send_request_elevator_action(campaign.save.selected_depth_tier);
                    toast.show("Requested elevator activation from Host...", 2.0);
                }
            }
            ShedStation::ShopTerminal => {
                shop_open.0 = true;
                if let Ok(mut cursor) = cursor_q.single_mut() {
                    cursor.grab_mode = CursorGrabMode::None;
                    cursor.visible = true;
                }
            }
            ShedStation::ChargerBench => {
                let charged = inv.recharge_all_batteries(1.0);
                if charged {
                    toast.show("All batteries recharged to 100%!", 2.5);
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
            }
        },
        LookTargetKind::DownedPlayer { player_id, name } => {
            inv.carry_state = CarryHandState::DraggingPlayer {
                target_id: player_id,
            };
            toast.show(format!("Dragging {name} — move behind cover/foam!"), 2.4);
        }
        LookTargetKind::WorldItem { item_id, kind, .. } => {
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
                            pos_m: cam_origin,
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
        LookTargetKind::Stalker { .. } | LookTargetKind::BreakableVoxel => {}
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
    remote_health: Res<'w, RemoteHealthMap>,
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
    mut player_q: Query<&mut PlayerController>,
    mut cage_q: Query<&mut Transform, (With<ElevatorCageRoot>, Without<ElevatorDoorVisual>)>,
    mut door_q: Query<&mut Transform, (With<ElevatorDoorVisual>, Without<ElevatorCageRoot>)>,
    mut bulb_q: Query<&mut PointLight, With<ElevatorBulbLight>>,
    mut items_q: Query<
        (Entity, &mut WorldItemEntity, &mut Transform),
        (Without<ElevatorCageRoot>, Without<ElevatorDoorVisual>),
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
    let remote_health = &res.remote_health;
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
        let missing_outside = player_q
            .single()
            .map(|pc| {
                count_living_players_outside_cage(
                    elevator,
                    pc.feet_pos_m,
                    health.is_alive(),
                    coop,
                    remote_health,
                )
            })
            .unwrap_or(0);

        if missing_outside > 0 {
            toast.show(
                format!(
                    "All living players must board the elevator before pulling the lever! ({missing_outside} outside)"
                ),
                3.0,
            );
        } else {
            match elevator.phase {
                ElevatorPhase::ShedIdle => {
                    let tier = campaign.save.selected_depth_tier.clamp(1, 3);
                    let next_seed = coop
                        .level_seed
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(
                            1_442_695_040_888_963_407 ^ (campaign.save.dives_completed as u64),
                        );
                    coop.level_seed = next_seed;
                    coop.local_edits.clear();

                    elevator.start_descent(&world_res.0, next_seed, tier);
                    coop.send_start_descent(next_seed, tier);
                    coop.send_elevator_sync(elevator);
                    toast.show(
                        format!(
                            "Descending to Depth Tier {tier}... (Generating cave seamlessly)"
                        ),
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
                        format!(
                            "EXTRACTION WINCH ACTIVATED! Loud machinery drawing predators — {}s!",
                            ELEVATOR_WINCH_DURATION_S.round() as u32
                        ),
                        3.8,
                    );
                }
                _ => {}
            }
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
    let deck_y = elevator.platform_y_m + 0.02;
    let mut player_y = deck_y;
    if let Ok(mut pc) = player_q.single_mut() {
        let dx = pc.feet_pos_m.x - elevator.center_xz_m.x;
        let dz = pc.feet_pos_m.z - elevator.center_xz_m.y;
        let in_xz = dx.abs() <= ELEVATOR_CAGE_HALF_EXTENT_M + 0.15
            && dz.abs() <= ELEVATOR_CAGE_HALF_EXTENT_M + 0.15;

        if in_xz && (pc.feet_pos_m.y - prev_y).abs() <= 1.25 {
            pc.feet_pos_m.y = (pc.feet_pos_m.y + delta_y).max(deck_y);
            if pc.velocity_m.y <= 0.0 && pc.feet_pos_m.y <= deck_y + 0.05 {
                pc.feet_pos_m.y = deck_y;
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

        // Spawn Blind Acoustic Predators deep in the cave:
        // Monster #1 is a nimble Mid/High-hearing Stalker; Monster #2 (Tier 2+) is a hulking
        // Low-frequency rumble-tracking Heavy Lurker!
        for (idx, &spawn_pos) in level_info.0.monster_spawns_m.iter().enumerate() {
            let monster_id = (idx + 1) as u16;
            let archetype = if idx == 0 {
                MonsterArchetype::Stalker
            } else {
                MonsterArchetype::HeavyLurker
            };
            let ent = spawn_monster_visual(
                &mut commands,
                &mut meshes,
                &mut materials,
                spawn_pos,
                archetype,
            );
            commands.entity(ent).insert(BlindStalker::new_with_archetype(
                monster_id,
                spawn_pos,
                elevator.depth_tier,
                archetype,
            ));
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
                    volume: 1.0,
                    pitch: 1.0 + ((remaining * 3.0).sin() * 0.08),
                    ai_event: Some(AcousticEvent {
                        pos_m: pos,
                        // Overwhelmingly louder across all 3 bands than Heavy Relic scraping
                        // so players can reliably lure Stalkers away while escaping with a relic!
                        energy: Bands3::new(4.2, 7.5, 8.5),
                        sharpness: 1.0,
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
                            volume: 0.78,
                            pitch: 0.94,
                            ai_event: Some(AcousticEvent {
                                pos_m: tf.translation,
                                // Clearly audible, yet strictly quieter than a Noise Decoy lure
                                energy: Bands3::new(2.1, 1.9, 1.4),
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

#[derive(SystemParam)]
struct StalkerAiRes<'w> {
    time: Res<'w, Time>,
    screen: Res<'w, AppScreen>,
    world_res: Res<'w, WorldResource>,
    telemetry: Res<'w, AcousticTelemetry>,
    coop: Res<'w, CoopState>,
    health: Res<'w, PlayerHealthState>,
    remote_health: Res<'w, RemoteHealthMap>,
    timers: ResMut<'w, GameplayTimers>,
    item_reg: ResMut<'w, WorldItemRegistry>,
    toast: ResMut<'w, GameplayToast>,
}

/// Evaluates all acoustic events (SFX + real mic voice above whisper curve) through
/// `voxel_acoustics::evaluate_ai_hearing` to drive Blind Acoustic Stalkers, and handles
/// pickaxe/flare stun & kill strikes!
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn blind_stalker_ai_system(
    mut commands: Commands,
    mut res: StalkerAiRes,
    mut sound_io: ParamSet<(
        MessageReader<PlaySpatialSoundEvent>,
        MessageWriter<PlaySpatialSoundEvent>,
    )>,
    mut strike_events: MessageReader<StrikeStalkerEvent>,
    mut hit_writer: MessageWriter<MonsterHitLocalPlayerEvent>,
    mut spawn_item_writer: MessageWriter<SpawnWorldItemEvent>,
    player_q: Query<(&PlayerController, &Transform)>,
    mut stalkers_q: Query<
        (Entity, &mut BlindStalker, &mut Transform),
        (Without<PlayerController>, Without<RemoteSyncedStalker>),
    >,
    mut remote_stalkers_q: Query<
        (&RemoteSyncedStalker, &mut Transform),
        (Without<BlindStalker>, Without<PlayerController>),
    >,
) {
    let time = &res.time;
    let screen = &res.screen;
    let world_res = &res.world_res;
    let telemetry = &res.telemetry;
    let coop = &res.coop;
    let health = &res.health;
    let remote_health = &res.remote_health;
    let timers = &mut res.timers;
    let item_reg = &mut res.item_reg;
    let toast = &mut res.toast;

    if **screen != AppScreen::InGame {
        sound_io.p0().clear();
        strike_events.clear();
        return;
    }
    let dt = time.delta_secs().min(0.05);
    let elapsed = time.elapsed_secs();

    // Guest-side smooth interpolation + stunned shudder animation + synced vocal/step acoustics
    let alpha = (dt * 12.0).clamp(0.0, 1.0);
    let mut guest_monster_sounds: Vec<(Vec3, SynthClip, f32, f32)> = Vec::new();
    for (rs, mut tf) in &mut remote_stalkers_q {
        let prev = tf.translation;
        tf.translation = tf.translation.lerp(rs.target_pos_m, alpha);
        tf.rotation = if rs.state == StalkerState::Stunned {
            Quat::from_rotation_y(rs.target_yaw)
                * Quat::from_rotation_z((elapsed * 26.0).sin() * 0.22)
                * Quat::from_rotation_x(-0.24)
        } else {
            Quat::from_rotation_y(rs.target_yaw)
        };
        if !coop.is_host_or_solo() {
            let phase_shift = rs.monster_id as f32 * 1.37;
            let period = match rs.state {
                StalkerState::Hunt => 2.2,
                StalkerState::Investigate => 3.2,
                _ => 4.4,
            };
            let prev_tick = ((elapsed - dt + phase_shift) / period).floor() as i32;
            let curr_tick = ((elapsed + phase_shift) / period).floor() as i32;
            if curr_tick > prev_tick && rs.state != StalkerState::Stunned {
                let is_lurker = rs.monster_id >= 2;
                let pitch = if is_lurker { 0.58 } else { 0.96 };
                guest_monster_sounds.push((
                    tf.translation + Vec3::Y * 1.6,
                    SynthClip::MonsterGrowl,
                    0.85,
                    pitch,
                ));
            }
            let moved = tf.translation.distance(prev);
            if moved > 0.015 {
                let step_prev = ((elapsed - dt + phase_shift) / 0.65).floor() as i32;
                let step_curr = ((elapsed + phase_shift) / 0.65).floor() as i32;
                if step_curr > step_prev {
                    let is_lurker = rs.monster_id >= 2;
                    let (clip, vol, pitch) = if is_lurker {
                        (SynthClip::HeavyImpact, 0.50, 0.68)
                    } else {
                        (SynthClip::FootstepStone, 0.72, 0.88)
                    };
                    guest_monster_sounds.push((tf.translation + Vec3::Y * 0.25, clip, vol, pitch));
                }
            }
        }
    }

    // Collect acoustic events emitted this frame
    let mut ai_events: Vec<AcousticEvent> = sound_io
        .p0()
        .read()
        .filter_map(|ev| ev.ai_event)
        .collect();

    // Collect player stun/kill strikes (guests forward to Host; Host applies authoritatively)
    let incoming_strikes: Vec<StrikeStalkerEvent> = strike_events.read().copied().collect();
    if !coop.is_host_or_solo() {
        let mut sound_writer = sound_io.p1();
        for (pos_m, clip, volume, pitch) in guest_monster_sounds {
            sound_writer.write(PlaySpatialSoundEvent {
                pos_m,
                clip,
                volume,
                pitch,
                ai_event: None,
            });
        }
        for ev in incoming_strikes {
            coop.send_monster_damaged(ev.monster_id, ev.damage, ev.stun_s, ev.knockback_dir);
            if ev.damage > 0 {
                toast.show("Struck Stalker! (Stunned)", 1.6);
            } else {
                toast.show("Flash-Stunned Stalker!", 1.8);
            }
        }
        return;
    }

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

    if stalkers_q.is_empty() {
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

    let mut sound_writer = sound_io.p1();
    let world = world_res.read();
    for (ent, mut stalker, mut tf) in &mut stalkers_q {
        // 1. Apply any pickaxe melee or flare flash-stun strikes hitting this stalker
        let mut killed = false;
        for ev in &incoming_strikes {
            if ev.monster_id != stalker.monster_id {
                continue;
            }
            match apply_stalker_hit(&world, &mut stalker, ev.damage, ev.stun_s, ev.knockback_dir) {
                StalkerHitResult::Stunned {
                    remaining_hp,
                    max_hp,
                } => {
                    sound_writer.write(PlaySpatialSoundEvent {
                        pos_m: stalker.feet_pos_m + Vec3::Y * 1.5,
                        clip: SynthClip::MonsterScreech,
                        volume: 0.90,
                        pitch: 1.28,
                        ai_event: None,
                    });
                    if ev.damage > 0 {
                        toast.show(
                            format!(
                                "STUNNED Stalker ({:.1}s)! HP {remaining_hp}/{max_hp}",
                                ev.stun_s
                            ),
                            2.2,
                        );
                    } else {
                        toast.show(
                            format!("FLASH-STUNNED Stalker ({:.1}s)!", ev.stun_s),
                            2.4,
                        );
                    }
                }
                StalkerHitResult::Killed => {
                    killed = true;
                    let drop_pos = stalker.feet_pos_m + Vec3::Y * 0.65;
                    let bounty = if stalker.archetype == MonsterArchetype::HeavyLurker {
                        75
                    } else {
                        45
                    };
                    let name = if stalker.archetype == MonsterArchetype::HeavyLurker {
                        "Heavy Lurker"
                    } else {
                        "Stalker"
                    };
                    coop.send_monster_killed(stalker.monster_id, drop_pos);
                    sound_writer.write(PlaySpatialSoundEvent {
                        pos_m: drop_pos,
                        clip: SynthClip::MonsterScreech,
                        volume: 1.0,
                        pitch: if stalker.archetype == MonsterArchetype::HeavyLurker {
                            0.52
                        } else {
                            0.68
                        },
                        ai_event: None,
                    });
                    let loot_id = item_reg.alloc_id();
                    let loot_state = WorldItemNetState {
                        id: loot_id,
                        kind: WorldItemKind::BonusLoot { value: bounty },
                        pos_m: drop_pos,
                        vel_m: Vec3::new(0.0, 2.2, 0.0),
                        in_shed: false,
                        carried_by: Vec::new(),
                        active_timer_s: None,
                    };
                    coop.send_world_item_spawn(loot_state.clone());
                    spawn_item_writer.write(SpawnWorldItemEvent(loot_state));
                    commands.entity(ent).despawn();
                    toast.show(
                        format!("{name} ELIMINATED! Dropped Crystal (+{bounty} cr)"),
                        3.2,
                    );
                    break;
                }
            }
        }
        if killed {
            continue;
        }

        // 2. Feed acoustic events into unified AI hearing
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
        tf.rotation = if stalker.state == StalkerState::Stunned {
            Quat::from_rotation_y(stalker.yaw)
                * Quat::from_rotation_z((elapsed * 26.0).sin() * 0.22)
                * Quat::from_rotation_x(-0.24)
        } else {
            Quat::from_rotation_y(stalker.yaw)
        };

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
            let (clip, step_vol, pitch) = match stalker.archetype {
                MonsterArchetype::Stalker => (SynthClip::FootstepStone, vol, 0.88),
                MonsterArchetype::HeavyLurker => (SynthClip::HeavyImpact, vol * 0.55, 0.68),
            };
            sound_writer.write(PlaySpatialSoundEvent {
                pos_m: step_pos + Vec3::Y * 0.25,
                clip,
                volume: step_vol,
                pitch,
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
                StalkerState::Stunned => MonsterBehaviorState::Stunned,
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
    remote_q: &mut Query<(Entity, &mut RemoteSyncedStalker)>,
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
        MonsterBehaviorState::Stunned => StalkerState::Stunned,
    };
    for (_, mut rs) in remote_q.iter_mut() {
        if rs.monster_id == monster_id {
            rs.target_pos_m = pos_m;
            rs.target_yaw = yaw;
            rs.state = st;
            return;
        }
    }
    let archetype = if monster_id >= 2 {
        MonsterArchetype::HeavyLurker
    } else {
        MonsterArchetype::Stalker
    };
    let ent = spawn_monster_visual(commands, meshes, materials, pos_m, archetype);
    commands.entity(ent).insert(RemoteSyncedStalker {
        monster_id,
        target_pos_m: pos_m,
        target_yaw: yaw,
        state: st,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net_bridge::RemotePlayerData;
    use voxel_core::{VoxelConfig, generate_world_with_tier};
    use voxel_net::PlayerInfo;

    #[test]
    fn elevator_all_aboard_counts_living_players_outside_and_ignores_dead() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        let (_, info) = generate_world_with_tier(777, 1, cfg);
        let elev = ElevatorState::from_level_info(&info, 777);
        let inside = Vec3::new(elev.center_xz_m.x, elev.platform_y_m + 0.05, elev.center_xz_m.y);
        let outside = Vec3::new(
            elev.center_xz_m.x + 8.0,
            elev.platform_y_m + 0.05,
            elev.center_xz_m.y,
        );

        let mut coop = CoopState::default();
        let mut remote_health = RemoteHealthMap::default();

        // Solo inside -> 0 missing; solo outside -> 1 missing
        assert_eq!(
            count_living_players_outside_cage(&elev, inside, true, &coop, &remote_health),
            0
        );
        assert_eq!(
            count_living_players_outside_cage(&elev, outside, true, &coop, &remote_health),
            1
        );

        // Add living remote teammate outside -> 1 missing
        coop.remote_players.insert(
            2,
            RemotePlayerData {
                info: PlayerInfo {
                    id: 2,
                    name: "P2".into(),
                    visor_rgb: [1.0, 1.0, 1.0],
                },
                entity: Entity::PLACEHOLDER,
                head_entity: Entity::PLACEHOLDER,
                flashlight_entity: Entity::PLACEHOLDER,
                visor_material: Handle::default(),
                target_feet_m: outside,
                current_feet_m: outside,
                target_yaw: 0.0,
                target_pitch: 0.0,
                crouching: false,
                flashlight_on: true,
            },
        );
        remote_health.0.insert(2, LocalHealthPhase::Healthy);
        assert_eq!(
            count_living_players_outside_cage(&elev, inside, true, &coop, &remote_health),
            1
        );

        // Once remote teammate dies, they no longer block extraction
        remote_health.0.insert(2, LocalHealthPhase::Dead);
        assert_eq!(
            count_living_players_outside_cage(&elev, inside, true, &coop, &remote_health),
            0
        );
    }
}
