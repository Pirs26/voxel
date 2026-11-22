use bevy::{
    ecs::system::SystemParam,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use voxel_acoustics::{AcousticEvent, AcousticEventKind, Bands3};
use voxel_audio_dsp::SynthClip;
use voxel_core::{Destructibility, VoxelId, raycast_solid};

use crate::{
    ai_blind::{BlindStalker, MonsterArchetype, RemoteSyncedStalker, stalker_hit_aabb_for},
    audio_bridge::{AcousticBeaconState, PlaySpatialSoundEvent},
    gameplay::GameplayToast,
    health::PlayerHealthState,
    inventory::PlayerInventory,
    net_bridge::{CoopMenuOpen, CoopState},
    physics::{
        PLAYER_HEIGHT_CROUCH_M, PLAYER_HEIGHT_STAND_M, aabb_intersects_world, ray_aabb_intersect,
        voxel_overlaps_player_aabb,
    },
    player::PlayerController,
    settings::{AppScreen, SettingsMenuOpen},
    shed::ShopOverlayOpen,
    voxel_render::WorldResource,
};

/// Emitted when the local player lands a pickaxe strike (`[LMB]`) or direct flare flash (`[G]`)
/// on a Blind Acoustic Stalker.
#[derive(Message, Clone, Copy, Debug)]
pub struct StrikeStalkerEvent {
    pub monster_id: u16,
    pub damage: u8,
    pub stun_s: f32,
    pub knockback_dir: Vec3,
    #[allow(dead_code)]
    pub hit_pos_m: Vec3,
}

#[derive(Message, Clone, Copy, Debug)]
pub struct SpawnFlareEvent {
    pub origin_m: Vec3,
    pub velocity_m: Vec3,
}

#[derive(Component)]
pub struct ThrownFlare {
    pub velocity_m: Vec3,
    pub life_remaining_s: f32,
    pub settled: bool,
}

#[derive(Component)]
pub struct FlareLight;

#[derive(Resource)]
struct FlareAssets {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

pub struct ToolsPlugin;

impl Plugin for ToolsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SpawnFlareEvent>()
            .add_message::<StrikeStalkerEvent>()
            .add_systems(Startup, setup_flare_assets)
            .add_systems(
                Update,
                (
                    voxel_interact_system,
                    throw_flare_system,
                    spawn_flare_entities_system,
                    update_flares_system,
                    move_beacon_system,
                )
                    .chain(),
            );
    }
}

fn setup_flare_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(FlareAssets {
        mesh: meshes.add(Sphere::new(0.08)),
        material: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.35, 0.12),
            emissive: LinearRgba::new(12.0, 3.2, 0.8, 1.0),
            ..default()
        }),
    });
}

#[derive(SystemParam)]
struct ToolGateRes<'w> {
    screen: Res<'w, AppScreen>,
    menu_open: Res<'w, CoopMenuOpen>,
    shop_open: Res<'w, ShopOverlayOpen>,
    settings_open: Res<'w, SettingsMenuOpen>,
    health: Res<'w, PlayerHealthState>,
}

/// Left click: Strike a Stalker in crosshair melee range (`3.2m`, stuns & damages!) OR break
/// `CrackedRock` / `AcousticFoam` (breaching shortcuts & uncovering tunnels!).
/// Right click: Spray/place `AcousticFoam` (consumes 1 charge from belt AcousticFoam canister!).
#[allow(clippy::too_many_arguments)]
fn voxel_interact_system(
    time: Res<Time>,
    mut melee_cd_s: Local<f32>,
    mouse: Res<ButtonInput<MouseButton>>,
    gate: ToolGateRes,
    mut inv: ResMut<PlayerInventory>,
    mut toast: ResMut<GameplayToast>,
    cursor_q: Query<&CursorOptions, With<PrimaryWindow>>,
    player_q: Query<(&PlayerController, &Transform)>,
    stalkers_q: Query<(&Transform, Option<&BlindStalker>, Option<&RemoteSyncedStalker>)>,
    world_res: Res<WorldResource>,
    mut coop: ResMut<CoopState>,
    mut strike_writer: MessageWriter<StrikeStalkerEvent>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
) {
    *melee_cd_s = (*melee_cd_s - time.delta_secs()).max(0.0);
    let Ok(cursor) = cursor_q.single() else {
        return;
    };
    if *gate.screen != AppScreen::InGame
        || gate.menu_open.0
        || gate.shop_open.0
        || gate.settings_open.0
        || !gate.health.is_healthy()
        || inv.is_hands_occupied()
        || cursor.grab_mode == CursorGrabMode::None
    {
        return;
    }
    let Ok((pc, cam_tf)) = player_q.single() else {
        return;
    };

    let break_pressed = mouse.just_pressed(MouseButton::Left);
    let place_pressed = mouse.just_pressed(MouseButton::Right);
    if !break_pressed && !place_pressed {
        return;
    }

    let origin = cam_tf.translation;
    let dir = cam_tf.forward().as_vec3();
    let hit = {
        let world = world_res.read();
        raycast_solid(&world, origin, dir, 5.2)
    };

    // Priority on `[LMB]`: Check crosshair melee pickaxe strike against Blind Stalkers first!
    if break_pressed && *melee_cd_s <= 0.0 {
        let max_melee_m = hit.as_ref().map_or(3.2f32, |h| h.distance_m.min(3.2));
        let mut best_stalker: Option<(f32, u16, Vec3)> = None;
        for (stf, host_s, remote_s) in &stalkers_q {
            let (monster_id, archetype) = if let Some(s) = host_s {
                (s.monster_id, s.archetype)
            } else if let Some(rs) = remote_s {
                let arch = if rs.monster_id >= 2 {
                    MonsterArchetype::HeavyLurker
                } else {
                    MonsterArchetype::Stalker
                };
                (rs.monster_id, arch)
            } else {
                continue;
            };
            let (mn, mx) = stalker_hit_aabb_for(stf.translation, archetype);
            if let Some(t) = ray_aabb_intersect(origin, dir, mn, mx, max_melee_m)
                && best_stalker.as_ref().is_none_or(|(bd, _, _)| t < *bd)
            {
                best_stalker = Some((t, monster_id, origin + dir * t));
            }
        }

        if let Some((_, monster_id, hit_pos_m)) = best_stalker {
            *melee_cd_s = 0.42;
            strike_writer.write(StrikeStalkerEvent {
                monster_id,
                damage: 1,
                stun_s: 3.2,
                knockback_dir: dir,
                hit_pos_m,
            });
            sound_writer.write(PlaySpatialSoundEvent {
                pos_m: hit_pos_m,
                clip: SynthClip::HeavyImpact,
                volume: 0.92,
                pitch: 1.08,
                ai_event: Some(AcousticEvent {
                    pos_m: hit_pos_m,
                    energy: Bands3::new(2.0, 2.8, 3.0),
                    sharpness: 0.9,
                    kind: AcousticEventKind::ItemImpact,
                }),
            });
            return;
        }
    }

    let Some(hit) = hit else {
        return;
    };

    if break_pressed {
        match hit.material.def().destructibility {
            Destructibility::BreakableCracked | Destructibility::PlaceableTool => {
                world_res.write().set(hit.voxel, VoxelId::Air);
                coop.send_voxel_delta(hit.voxel, VoxelId::Air);
                sound_writer.write(PlaySpatialSoundEvent {
                    pos_m: hit.point_m,
                    clip: SynthClip::PickaxeHit,
                    volume: 0.95,
                    pitch: 1.0,
                    ai_event: Some(AcousticEvent {
                        pos_m: hit.point_m,
                        energy: Bands3::new(2.2, 3.4, 3.8),
                        sharpness: 0.95,
                        kind: AcousticEventKind::WallBreach,
                    }),
                });
            }
            Destructibility::Unbreakable => {
                sound_writer.write(PlaySpatialSoundEvent {
                    pos_m: hit.point_m,
                    clip: SynthClip::PickaxeHit,
                    volume: 0.35,
                    pitch: 1.15,
                    ai_event: Some(AcousticEvent {
                        pos_m: hit.point_m,
                        energy: Bands3::new(0.8, 1.4, 1.8),
                        sharpness: 0.9,
                        kind: AcousticEventKind::ItemImpact,
                    }),
                });
            }
        }
    } else if place_pressed {
        let target = hit.previous_voxel;
        let local_height = if pc.crouching {
            PLAYER_HEIGHT_CROUCH_M
        } else {
            PLAYER_HEIGHT_STAND_M
        };

        // Prevent building inside the local player's OR any remote player's hitbox
        // (avoids trapping teammates or creating X-ray clipping!).
        let cfg = world_res.read().config();
        let blocked_by_local = voxel_overlaps_player_aabb(cfg, target, pc.feet_pos_m, local_height);
        let blocked_by_remote = coop.remote_players.values().any(|rp| {
            let h = if rp.crouching {
                PLAYER_HEIGHT_CROUCH_M
            } else {
                PLAYER_HEIGHT_STAND_M
            };
            voxel_overlaps_player_aabb(cfg, target, rp.current_feet_m, h)
                || voxel_overlaps_player_aabb(cfg, target, rp.target_feet_m, h)
        });
        if blocked_by_local || blocked_by_remote {
            toast.show("Blocked — cannot build inside a player!", 1.8);
            return;
        }

        if !inv.consume_foam() {
            toast.show(
                "No Acoustic Foam canister in belt — grab one from the Shed workbench or Shop!",
                2.5,
            );
            return;
        }

        let (v_center, placed) = {
            let mut world = world_res.write();
            let vc = world.config().voxel_to_world_center(target);
            let ok = !world.is_solid(target) && world.set(target, VoxelId::AcousticFoam);
            (vc, ok)
        };
        if placed {
            coop.send_voxel_delta(target, VoxelId::AcousticFoam);
            sound_writer.write(PlaySpatialSoundEvent {
                pos_m: v_center,
                clip: SynthClip::FoamSpray,
                volume: 0.55,
                pitch: 1.0,
                ai_event: Some(AcousticEvent {
                    pos_m: v_center,
                    energy: Bands3::new(0.3, 0.7, 0.9),
                    sharpness: 0.3,
                    kind: AcousticEventKind::ItemImpact,
                }),
            });
        } else {
            // Refund foam charge if placement was already solid or out of bounds
            let _ = inv.try_add_belt_item(crate::inventory::BeltItem::AcousticFoam { charges: 1 });
        }
    }
}

/// Press `G` to throw a glowing flare from belt inventory that bounces off voxel walls.
#[allow(clippy::too_many_arguments)]
fn throw_flare_system(
    keys: Res<ButtonInput<KeyCode>>,
    screen: Res<AppScreen>,
    menu_open: Res<CoopMenuOpen>,
    shop_open: Res<ShopOverlayOpen>,
    settings_open: Res<SettingsMenuOpen>,
    health: Res<PlayerHealthState>,
    mut inv: ResMut<PlayerInventory>,
    mut toast: ResMut<GameplayToast>,
    coop: Res<CoopState>,
    player_q: Query<&Transform, With<PlayerController>>,
    mut flare_writer: MessageWriter<SpawnFlareEvent>,
) {
    if *screen != AppScreen::InGame
        || menu_open.0
        || shop_open.0
        || settings_open.0
        || !health.is_healthy()
        || inv.is_hands_occupied()
        || !keys.just_pressed(KeyCode::KeyG)
    {
        return;
    }
    if !inv.consume_flare() {
        toast.show("Out of flares! Buy more at the Base Shed Shop.", 2.2);
        return;
    }
    let Ok(cam_tf) = player_q.single() else {
        return;
    };

    let dir = cam_tf.forward().as_vec3();
    let origin_m = cam_tf.translation + dir * 0.55;
    let velocity_m = dir * 11.5 + Vec3::Y * 2.4;

    coop.send_flare_thrown(origin_m, velocity_m);
    flare_writer.write(SpawnFlareEvent {
        origin_m,
        velocity_m,
    });
}

fn spawn_flare_entities_system(
    mut commands: Commands,
    flare_assets: Res<FlareAssets>,
    mut events: MessageReader<SpawnFlareEvent>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
) {
    for ev in events.read() {
        commands
            .spawn((
                Mesh3d(flare_assets.mesh.clone()),
                MeshMaterial3d(flare_assets.material.clone()),
                Transform::from_translation(ev.origin_m),
                ThrownFlare {
                    velocity_m: ev.velocity_m,
                    life_remaining_s: 45.0,
                    settled: false,
                },
            ))
            .with_children(|parent| {
                parent.spawn((
                    PointLight {
                        color: Color::srgb(1.0, 0.42, 0.18),
                        intensity: 65_000.0,
                        range: 18.0,
                        shadow_maps_enabled: false,
                        ..default()
                    },
                    Transform::default(),
                    FlareLight,
                ));
            });

        sound_writer.write(PlaySpatialSoundEvent {
            pos_m: ev.origin_m,
            clip: SynthClip::FlareIgnite,
            volume: 0.65,
            pitch: 1.0,
            ai_event: Some(AcousticEvent {
                pos_m: ev.origin_m,
                energy: Bands3::new(0.5, 1.2, 1.5),
                sharpness: 0.6,
                kind: AcousticEventKind::ItemImpact,
            }),
        });
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_flares_system(
    time: Res<Time>,
    mut commands: Commands,
    world_res: Res<WorldResource>,
    mut flares_q: Query<(Entity, &mut Transform, &mut ThrownFlare, &Children)>,
    stalkers_q: Query<
        (&Transform, Option<&BlindStalker>, Option<&RemoteSyncedStalker>),
        Without<ThrownFlare>,
    >,
    mut lights_q: Query<&mut PointLight, With<FlareLight>>,
    mut strike_writer: MessageWriter<StrikeStalkerEvent>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
) {
    let dt = time.delta_secs().min(0.05);
    let r = Vec3::splat(0.10);
    let world = world_res.read();

    for (ent, mut tf, mut flare, children) in &mut flares_q {
        flare.life_remaining_s -= dt;
        if flare.life_remaining_s <= 0.0 {
            commands.entity(ent).despawn();
            continue;
        }

        let life_frac = (flare.life_remaining_s / 8.0).clamp(0.0, 1.0);
        let flicker = 0.88 + 0.12 * (time.elapsed_secs() * 19.0 + ent.index_u32() as f32).sin();
        for child in children.iter() {
            if let Ok(mut light) = lights_q.get_mut(child) {
                light.intensity = 65_000.0 * life_frac * flicker;
            }
        }

        if flare.settled {
            continue;
        }

        flare.velocity_m.y -= 16.0 * dt;
        let mut pos = tf.translation;
        let mut bounced = false;
        let impact_speed = flare.velocity_m.length();

        for axis in 0..3 {
            let mut next = pos;
            next[axis] += flare.velocity_m[axis] * dt;
            if aabb_intersects_world(&world, next - r, next + r) {
                flare.velocity_m[axis] *= -0.42;
                flare.velocity_m *= 0.78;
                bounced = true;
            } else {
                pos = next;
            }
        }
        tf.translation = pos;

        // Direct/close flare impact flashes & stuns Blind Stalkers (gives ranged escape counterplay!)
        if impact_speed > 2.5 {
            for (stf, host_s, remote_s) in &stalkers_q {
                let monster_id = if let Some(s) = host_s {
                    s.monster_id
                } else if let Some(rs) = remote_s {
                    rs.monster_id
                } else {
                    continue;
                };
                let stalker_center = stf.translation + Vec3::Y * 1.1;
                if pos.distance(stalker_center) <= 1.75 {
                    let knock = flare.velocity_m.normalize_or_zero();
                    strike_writer.write(StrikeStalkerEvent {
                        monster_id,
                        damage: 0,
                        stun_s: 4.0,
                        knockback_dir: knock,
                        hit_pos_m: pos,
                    });
                    flare.velocity_m *= -0.25;
                    flare.settled = true;
                    break;
                }
            }
        }

        if bounced && impact_speed > 2.2 {
            let vol = (impact_speed / 12.0).clamp(0.18, 0.65);
            sound_writer.write(PlaySpatialSoundEvent {
                pos_m: pos,
                clip: SynthClip::BatteryClick,
                volume: vol,
                pitch: 1.18,
                ai_event: Some(AcousticEvent {
                    pos_m: pos,
                    energy: Bands3::splat(vol * 1.6),
                    sharpness: 0.85,
                    kind: AcousticEventKind::ItemImpact,
                }),
            });
        }

        if flare.velocity_m.length() < 0.35 && bounced {
            flare.settled = true;
            flare.velocity_m = Vec3::ZERO;
        }
    }
}

/// Press `K` to toggle the optional Acoustic Test Beacon ON at your position (or OFF to silence it).
#[allow(clippy::too_many_arguments)]
fn move_beacon_system(
    keys: Res<ButtonInput<KeyCode>>,
    screen: Res<AppScreen>,
    menu_open: Res<CoopMenuOpen>,
    shop_open: Res<ShopOverlayOpen>,
    settings_open: Res<SettingsMenuOpen>,
    coop: Res<CoopState>,
    player_q: Query<&Transform, With<PlayerController>>,
    mut beacon: ResMut<AcousticBeaconState>,
    mut toast: ResMut<GameplayToast>,
) {
    if *screen != AppScreen::InGame
        || menu_open.0
        || shop_open.0
        || settings_open.0
        || !keys.just_pressed(KeyCode::KeyK)
    {
        return;
    }
    if let Ok(cam_tf) = player_q.single() {
        beacon.enabled = !beacon.enabled;
        if beacon.enabled {
            let new_pos = cam_tf.translation + cam_tf.forward().as_vec3() * 1.2;
            beacon.pos_m = new_pos;
            coop.send_beacon_moved(new_pos);
            toast.show("Acoustic Test Beacon ON (press [K] again to turn off)", 2.6);
        } else {
            toast.show("Acoustic Test Beacon OFF", 1.8);
        }
    }
}
