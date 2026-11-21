use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use voxel_acoustics::{AcousticEvent, AcousticEventKind, Bands3};
use voxel_audio_dsp::SynthClip;
use voxel_core::{Destructibility, VoxelId, raycast_solid};

use crate::{
    audio_bridge::{AcousticBeaconState, PlaySpatialSoundEvent},
    net_bridge::{CoopMenuOpen, CoopState},
    physics::aabb_intersects_world,
    player::PlayerController,
    voxel_render::WorldResource,
};

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

pub struct ToolsPlugin;

impl Plugin for ToolsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SpawnFlareEvent>().add_systems(
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

/// Left click: Break `CrackedRock` or `AcousticFoam` (breaching shortcuts / opening sound paths!).
/// Right click: Spray/place `AcousticFoam` (sealing passages to muffle sound & deaden echoes!).
fn voxel_interact_system(
    mouse: Res<ButtonInput<MouseButton>>,
    menu_open: Res<CoopMenuOpen>,
    cursor_q: Query<&CursorOptions, With<PrimaryWindow>>,
    player_q: Query<(&PlayerController, &Transform)>,
    mut world_res: ResMut<WorldResource>,
    mut coop: ResMut<CoopState>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
) {
    let Ok(cursor) = cursor_q.single() else {
        return;
    };
    if menu_open.0 || cursor.grab_mode == CursorGrabMode::None {
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
    let Some(hit) = raycast_solid(&world_res.0, origin, dir, 5.2) else {
        return;
    };

    if break_pressed {
        match hit.material.def().destructibility {
            Destructibility::BreakableCracked | Destructibility::PlaceableTool => {
                world_res.0.set(hit.voxel, VoxelId::Air);
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
                // Dull clink on unbreakable rock/steel
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
        // Prevent placing foam directly inside the player's own body
        let v_center = world_res.0.config().voxel_to_world_center(target);
        let player_mid = pc.feet_pos_m + Vec3::Y * 0.9;
        if v_center.distance(player_mid) > 0.78 && world_res.0.set(target, VoxelId::AcousticFoam) {
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
        }
    }
}

/// Press `G` to throw a glowing flare that bounces off voxel walls with real impact echoes.
fn throw_flare_system(
    keys: Res<ButtonInput<KeyCode>>,
    menu_open: Res<CoopMenuOpen>,
    coop: Res<CoopState>,
    player_q: Query<&Transform, With<PlayerController>>,
    mut flare_writer: MessageWriter<SpawnFlareEvent>,
) {
    if menu_open.0 || !keys.just_pressed(KeyCode::KeyG) {
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
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut events: MessageReader<SpawnFlareEvent>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
) {
    for ev in events.read() {
        // Put PointLight on a dedicated child entity (with NO Mesh3d!) so Bevy culls the light
        // using its full 18m range sphere rather than the tiny 0.08m flare mesh AABB!
        commands
            .spawn((
                Mesh3d(meshes.add(Sphere::new(0.08))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(1.0, 0.35, 0.12),
                    emissive: LinearRgba::new(12.0, 3.2, 0.8, 1.0),
                    ..default()
                })),
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

fn update_flares_system(
    time: Res<Time>,
    mut commands: Commands,
    world_res: Res<WorldResource>,
    mut flares_q: Query<(Entity, &mut Transform, &mut ThrownFlare, &Children)>,
    mut lights_q: Query<&mut PointLight, With<FlareLight>>,
    mut sound_writer: MessageWriter<PlaySpatialSoundEvent>,
) {
    let dt = time.delta_secs().min(0.05);
    let r = Vec3::splat(0.10);

    for (ent, mut tf, mut flare, children) in &mut flares_q {
        flare.life_remaining_s -= dt;
        if flare.life_remaining_s <= 0.0 {
            commands.entity(ent).despawn();
            continue;
        }

        // Lifespan dimming & subtle flame flicker on the child PointLight
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
            if aabb_intersects_world(&world_res.0, next - r, next + r) {
                flare.velocity_m[axis] *= -0.42;
                flare.velocity_m *= 0.78;
                bounced = true;
            } else {
                pos = next;
            }
        }
        tf.translation = pos;

        if bounced && impact_speed > 2.2 {
            let vol = (impact_speed / 12.0).clamp(0.18, 0.75);
            sound_writer.write(PlaySpatialSoundEvent {
                pos_m: pos,
                clip: SynthClip::FootstepStone,
                volume: vol,
                pitch: 1.35,
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

/// Press `E` to move the looping Acoustic Test Beacon right to your position
/// so you can walk behind stone walls or spray Acoustic Foam around it to test muffling + echoes!
fn move_beacon_system(
    keys: Res<ButtonInput<KeyCode>>,
    menu_open: Res<CoopMenuOpen>,
    coop: Res<CoopState>,
    player_q: Query<&Transform, With<PlayerController>>,
    mut beacon: ResMut<AcousticBeaconState>,
) {
    if menu_open.0 || !keys.just_pressed(KeyCode::KeyE) {
        return;
    }
    if let Ok(cam_tf) = player_q.single() {
        let new_pos = cam_tf.translation + cam_tf.forward().as_vec3() * 1.2;
        beacon.pos_m = new_pos;
        coop.send_beacon_moved(new_pos);
    }
}
