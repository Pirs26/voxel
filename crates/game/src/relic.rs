//! Physical world items (Heavy Quest Relics, Bonus Loot, Batteries, Flares,
//! Acoustic Foam canisters, Aid Kits, Flashlights, and Throwable Noise Decoys).

use bevy::prelude::*;
use voxel_core::GeneratedLevelInfo;
use voxel_net::{WorldItemKind, WorldItemNetState};

use crate::{
    inventory::{BatteryCellItem, BeltItem, PlayerInventory},
    save::{SavedWorldItem, SavedWorldItemKind},
};

/// Movement speed multiplier while dragging a Heavy Quest Relic solo (loud scraping).
pub const SOLO_RELIC_SPEED_MULT: f32 = 0.40;
/// Movement speed multiplier while two or more players lift a Heavy Quest Relic together (quiet).
pub const DUO_RELIC_SPEED_MULT: f32 = 0.75;
/// Movement speed multiplier while carrying a downed companion or fallen body.
pub const BODY_CARRY_SPEED_MULT: f32 = 0.55;
/// Interval in seconds between loud stone-scraping acoustic events during solo relic drag.
pub const RELIC_SCRAPE_INTERVAL_S: f32 = 0.35;
/// Active duration in seconds for a triggered throwable Noise Decoy.
pub const DECOY_ACTIVE_DURATION_S: f32 = 12.0;
/// Interval in seconds between acoustic decoy pulses while active.
pub const DECOY_PULSE_INTERVAL_S: f32 = 0.90;
/// Legacy interaction distance constant (replaced by crosshair raycast `INTERACT_RAY_MAX_DIST_M`).
#[allow(dead_code)]
pub const ITEM_INTERACT_RANGE_M: f32 = 2.6;
/// Radius in meters around the Shed charger bench where dropped batteries passively recharge.
pub const CHARGER_PAD_RADIUS_M: f32 = 2.3;
/// Passive battery charge rate per second on the Shed charger pad.
pub const CHARGER_PAD_RATE_PER_S: f32 = 0.16;

/// Physical item entity spawned in the Base Shed or underground Cave.
#[derive(Component, Clone, Debug)]
pub struct WorldItemEntity {
    pub id: u32,
    pub kind: WorldItemKind,
    pub vel_m: Vec3,
    pub in_shed: bool,
    pub carried_by: Vec<u16>,
    /// Remaining active duration in seconds for triggered Noise Decoys.
    pub active_timer_s: Option<f32>,
    /// Countdown timer until the next decoy noise pulse.
    pub decoy_pulse_timer_s: f32,
    /// Whether the item has come to rest on solid ground.
    pub settled: bool,
}

/// Global registry and local-player carry HUD/physics state for physical world items.
#[derive(Resource, Clone, Debug)]
pub struct WorldItemRegistry {
    pub next_id: u32,
    pub scrape_timer_s: f32,
    pub active_carry_speed_mult: f32,
    pub active_carry_is_scraping: bool,
    pub nearest_item_prompt: Option<String>,
}

impl Default for WorldItemRegistry {
    fn default() -> Self {
        Self {
            next_id: 1,
            scrape_timer_s: 0.0,
            active_carry_speed_mult: 1.0,
            active_carry_is_scraping: false,
            nearest_item_prompt: None,
        }
    }
}

impl WorldItemRegistry {
    /// Allocates the next monotonically increasing non-zero world item ID.
    pub fn alloc_id(&mut self) -> u32 {
        let id = self.next_id.max(1);
        self.next_id = id.saturating_add(1).max(1);
        id
    }
}

/// Event requesting a physical world item to be spawned (local or networked).
#[derive(Message, Clone, Debug)]
pub struct SpawnWorldItemEvent(pub WorldItemNetState);

/// Event requesting a physical world item to be despawned by ID.
#[derive(Message, Clone, Debug)]
pub struct RemoveWorldItemEvent {
    pub item_id: u32,
}

/// Converts a networked [`WorldItemKind`] to a persistent [`SavedWorldItemKind`].
pub fn world_kind_to_saved(kind: &WorldItemKind) -> SavedWorldItemKind {
    match kind {
        WorldItemKind::HeavyRelic { value, weight_kg } => SavedWorldItemKind::HeavyRelic {
            value: *value,
            weight_kg: *weight_kg,
        },
        WorldItemKind::BonusLoot { value } => SavedWorldItemKind::BonusLoot { value: *value },
        WorldItemKind::BatteryCell { charge } => SavedWorldItemKind::BatteryCell {
            charge: charge.clamp(0.0, 1.0),
        },
        WorldItemKind::FlarePack { count } => SavedWorldItemKind::FlarePack { count: *count },
        WorldItemKind::FoamCanister { charges } => {
            SavedWorldItemKind::FoamCanister { charges: *charges }
        }
        WorldItemKind::NoiseDecoy { count } => SavedWorldItemKind::NoiseDecoy { count: *count },
        WorldItemKind::AidKit => SavedWorldItemKind::AidKit,
        WorldItemKind::Flashlight { battery } => SavedWorldItemKind::Flashlight {
            battery: battery.clamp(0.0, 1.0),
        },
        // Fallen bodies do not persist in the Base Shed across saves; fallback cleanly if queried.
        WorldItemKind::FallenBody { .. } => SavedWorldItemKind::BonusLoot { value: 0 },
    }
}

/// Converts a persistent [`SavedWorldItemKind`] back into a networked [`WorldItemKind`].
pub fn saved_to_world_kind(kind: &SavedWorldItemKind) -> WorldItemKind {
    match kind {
        SavedWorldItemKind::HeavyRelic { value, weight_kg } => WorldItemKind::HeavyRelic {
            value: *value,
            weight_kg: *weight_kg,
        },
        SavedWorldItemKind::BonusLoot { value } => WorldItemKind::BonusLoot { value: *value },
        SavedWorldItemKind::BatteryCell { charge } => WorldItemKind::BatteryCell {
            charge: charge.clamp(0.0, 1.0),
        },
        SavedWorldItemKind::FlarePack { count } => WorldItemKind::FlarePack { count: *count },
        SavedWorldItemKind::FoamCanister { charges } => {
            WorldItemKind::FoamCanister { charges: *charges }
        }
        SavedWorldItemKind::NoiseDecoy { count } => WorldItemKind::NoiseDecoy { count: *count },
        SavedWorldItemKind::AidKit => WorldItemKind::AidKit,
        SavedWorldItemKind::Flashlight { battery } => WorldItemKind::Flashlight {
            battery: battery.clamp(0.0, 1.0),
        },
    }
}

/// Converts a dropped [`BeltItem`] into its corresponding physical [`WorldItemKind`].
pub fn belt_item_to_world_kind(item: &BeltItem, loaded_battery_charge: f32) -> WorldItemKind {
    match item {
        BeltItem::Flashlight => WorldItemKind::Flashlight {
            battery: loaded_battery_charge.clamp(0.0, 1.0),
        },
        BeltItem::Flares { count } => WorldItemKind::FlarePack { count: *count },
        BeltItem::AcousticFoam { charges } => WorldItemKind::FoamCanister { charges: *charges },
        BeltItem::NoiseDecoys { count } => WorldItemKind::NoiseDecoy { count: *count },
        BeltItem::AidKit => WorldItemKind::AidKit,
        BeltItem::BonusLoot { value } => WorldItemKind::BonusLoot { value: *value },
    }
}

/// Human-readable display label for proximity prompts and HUD notices.
pub fn world_item_display_name(kind: &WorldItemKind) -> String {
    match kind {
        WorldItemKind::HeavyRelic { value, weight_kg } => {
            format!("Heavy Relic ({value}cr, {weight_kg:.0}kg)")
        }
        WorldItemKind::BonusLoot { value } => format!("Bonus Crystal ({value}cr)"),
        WorldItemKind::BatteryCell { charge } => {
            let pct = (charge.clamp(0.0, 1.0) * 100.0).round() as u8;
            format!("Battery Cell ({pct}%)")
        }
        WorldItemKind::FlarePack { count } => format!("Flares x{count}"),
        WorldItemKind::FoamCanister { charges } => format!("Foam Canister ({charges})"),
        WorldItemKind::NoiseDecoy { count } => format!("Noise Decoy x{count}"),
        WorldItemKind::AidKit => "Aid Kit".to_string(),
        WorldItemKind::Flashlight { battery } => {
            let pct = (battery.clamp(0.0, 1.0) * 100.0).round() as u8;
            format!("Flashlight ({pct}%)")
        }
        WorldItemKind::FallenBody { player_name, .. } => format!("{player_name}'s Body"),
    }
}

/// Returns `true` if the item is a two-handed Heavy Quest Relic.
pub fn is_heavy_relic(kind: &WorldItemKind) -> bool {
    matches!(kind, WorldItemKind::HeavyRelic { .. })
}

/// Returns `true` if the item is a fallen teammate body.
pub fn is_fallen_body(kind: &WorldItemKind) -> bool {
    matches!(kind, WorldItemKind::FallenBody { .. })
}

/// Returns `true` if the item requires a two-handed carry (`HeavyRelic` or `FallenBody`).
pub fn is_two_handed_world_item(kind: &WorldItemKind) -> bool {
    matches!(
        kind,
        WorldItemKind::HeavyRelic { .. } | WorldItemKind::FallenBody { .. }
    )
}

/// Returns movement speed multiplier based on how many players are carrying a Heavy Relic.
///
/// - `0` carriers: `1.0` (normal movement)
/// - `1` carrier: `0.40` (heavy solo drag)
/// - `2+` carriers: `0.75` (coordinated duo/team lift)
pub fn relic_carry_speed_mult(carrier_count: usize) -> f32 {
    match carrier_count {
        0 => 1.0,
        1 => SOLO_RELIC_SPEED_MULT,
        _ => DUO_RELIC_SPEED_MULT,
    }
}

/// Returns `true` when a Heavy Relic is being dragged solo and emits loud stone-scraping noise.
pub fn relic_is_scraping(carrier_count: usize) -> bool {
    carrier_count == 1
}

/// Returns `true` if `pos_m` lies inside the persistent Base Shed interior bounds
/// (with small tolerances so items resting against walls, floors, or countertops always register).
pub fn is_pos_in_shed(pos_m: Vec3, info: &GeneratedLevelInfo) -> bool {
    const XZ_TOLERANCE_M: f32 = 0.25;
    const Y_TOLERANCE_M: f32 = 0.5;
    pos_m.x >= info.shed_bounds_min_m.x - XZ_TOLERANCE_M
        && pos_m.x <= info.shed_bounds_max_m.x + XZ_TOLERANCE_M
        && pos_m.y >= info.shed_bounds_min_m.y - Y_TOLERANCE_M
        && pos_m.y <= info.shed_bounds_max_m.y + Y_TOLERANCE_M
        && pos_m.z >= info.shed_bounds_min_m.z - XZ_TOLERANCE_M
        && pos_m.z <= info.shed_bounds_max_m.z + XZ_TOLERANCE_M
}

/// Captures a persistent snapshot of all physical items currently inside the Base Shed
/// (including dropped items on floors, workbenches, elevator deck, and parkour platforms),
/// skipping fallen teammate bodies, armed/active Noise Decoys, and any IDs in `excluded_ids`.
pub fn collect_shed_items_snapshot<'a>(
    items: impl IntoIterator<Item = (&'a WorldItemEntity, &'a Transform)>,
    info: &GeneratedLevelInfo,
    excluded_ids: &std::collections::HashSet<u32>,
) -> Vec<SavedWorldItem> {
    items
        .into_iter()
        .filter(|(item, tf)| {
            !excluded_ids.contains(&item.id)
                && !is_fallen_body(&item.kind)
                && item.active_timer_s.is_none()
                && is_pos_in_shed(tf.translation, info)
        })
        .map(|(item, tf)| SavedWorldItem {
            kind: world_kind_to_saved(&item.kind),
            pos_m: tf.translation.to_array(),
            rot_y: tf.rotation.to_euler(EulerRot::YXZ).0,
        })
        .collect()
}

/// Attempts to pick up a physical world item into `inv`'s belt or battery pouch.
pub fn try_pickup_world_item_into_inventory(
    inv: &mut PlayerInventory,
    kind: &WorldItemKind,
) -> Result<&'static str, &'static str> {
    match kind {
        WorldItemKind::BatteryCell { charge } => inv
            .try_add_battery(BatteryCellItem::with_charge(*charge))
            .map(|_| "Picked up Battery Cell")
            .map_err(|_| "Battery pouch full"),
        WorldItemKind::FlarePack { count } => inv
            .try_add_belt_item(BeltItem::Flares { count: *count })
            .map(|_| "Picked up Flares")
            .map_err(|_| "Belt full"),
        WorldItemKind::FoamCanister { charges } => inv
            .try_add_belt_item(BeltItem::AcousticFoam { charges: *charges })
            .map(|_| "Picked up Foam Canister")
            .map_err(|_| "Belt full"),
        WorldItemKind::NoiseDecoy { count } => inv
            .try_add_belt_item(BeltItem::NoiseDecoys { count: *count })
            .map(|_| "Picked up Noise Decoys")
            .map_err(|_| "Belt full"),
        WorldItemKind::AidKit => inv
            .try_add_belt_item(BeltItem::AidKit)
            .map(|_| "Picked up Aid Kit")
            .map_err(|_| "Belt full"),
        WorldItemKind::BonusLoot { value } => inv
            .try_add_belt_item(BeltItem::BonusLoot { value: *value })
            .map(|_| "Picked up Bonus Loot")
            .map_err(|_| "Belt full"),
        WorldItemKind::Flashlight { battery } => inv
            .try_add_belt_item(BeltItem::Flashlight)
            .map(|_| {
                if inv.loaded_battery.is_none() {
                    inv.loaded_battery = Some(BatteryCellItem::with_charge(*battery));
                }
                "Picked up Flashlight"
            })
            .map_err(|_| "Belt full"),
        WorldItemKind::HeavyRelic { .. } => Err("Heavy relics require two-handed carry"),
        WorldItemKind::FallenBody { .. } => Err("Fallen bodies require two-handed carry"),
    }
}

/// Creates the 4 default physical starter supplies arranged neatly along the Shed workbench.
pub fn default_starter_shed_items(workbench_pos_m: Vec3) -> Vec<SavedWorldItem> {
    let base_y = workbench_pos_m.y + 0.14;
    let z = workbench_pos_m.z;
    vec![
        SavedWorldItem {
            kind: SavedWorldItemKind::FoamCanister { charges: 8 },
            pos_m: [workbench_pos_m.x - 0.85, base_y, z],
            rot_y: 0.0,
        },
        SavedWorldItem {
            kind: SavedWorldItemKind::AidKit,
            pos_m: [workbench_pos_m.x - 0.25, base_y, z],
            rot_y: 0.15,
        },
        SavedWorldItem {
            kind: SavedWorldItemKind::NoiseDecoy { count: 2 },
            pos_m: [workbench_pos_m.x + 0.35, base_y, z],
            rot_y: -0.2,
        },
        SavedWorldItem {
            kind: SavedWorldItemKind::BatteryCell { charge: 1.0 },
            pos_m: [workbench_pos_m.x + 0.95, base_y, z],
            rot_y: 0.35,
        },
    ]
}

/// Builds initial networked states for all underground Heavy Relics and Bonus Loot
/// for the generated cave level, scaling reward values with `info.depth_tier`.
pub fn create_cave_relic_states(
    info: &GeneratedLevelInfo,
    next_id: &mut u32,
) -> Vec<WorldItemNetState> {
    let tier = u32::from(info.depth_tier.max(1));
    let relic_value = 220 + tier * 90;
    let bonus_value = 65 + tier * 35;

    let mut items =
        Vec::with_capacity(info.relic_spawns_m.len() + info.bonus_loot_spawns_m.len());

    for &spawn_pos in &info.relic_spawns_m {
        let id = (*next_id).max(1);
        *next_id = id.saturating_add(1).max(1);
        items.push(WorldItemNetState {
            id,
            kind: WorldItemKind::HeavyRelic {
                value: relic_value,
                weight_kg: 58.0,
            },
            pos_m: spawn_pos + Vec3::Y * 0.20,
            vel_m: Vec3::ZERO,
            in_shed: false,
            carried_by: Vec::new(),
            active_timer_s: None,
        });
    }

    for &spawn_pos in &info.bonus_loot_spawns_m {
        let id = (*next_id).max(1);
        *next_id = id.saturating_add(1).max(1);
        items.push(WorldItemNetState {
            id,
            kind: WorldItemKind::BonusLoot { value: bonus_value },
            pos_m: spawn_pos + Vec3::Y * 0.18,
            vel_m: Vec3::ZERO,
            in_shed: false,
            carried_by: Vec::new(),
            active_timer_s: None,
        });
    }

    items
}

/// Converts a live [`WorldItemEntity`] and its current world position into a [`WorldItemNetState`].
pub fn to_net_state(ent: &WorldItemEntity, pos_m: Vec3) -> WorldItemNetState {
    WorldItemNetState {
        id: ent.id,
        kind: ent.kind.clone(),
        pos_m,
        vel_m: ent.vel_m,
        in_shed: ent.in_shed,
        carried_by: ent.carried_by.clone(),
        active_timer_s: ent.active_timer_s,
    }
}

/// Spawns a distinct, readable Bevy 0.20 visual representation for a [`WorldItemNetState`].
///
/// Any `PointLight` is ALWAYS spawned on a dedicated child entity with NO `Mesh3d`
/// to avoid Bevy frustum-culling the light using a small item mesh's AABB.
pub fn spawn_world_item_visual(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    state: &WorldItemNetState,
) -> Entity {
    let component = WorldItemEntity {
        id: state.id,
        kind: state.kind.clone(),
        vel_m: state.vel_m,
        in_shed: state.in_shed,
        carried_by: state.carried_by.clone(),
        active_timer_s: state.active_timer_s,
        decoy_pulse_timer_s: 0.0,
        settled: state.vel_m.length_squared() < 1e-4,
    };

    match &state.kind {
        WorldItemKind::HeavyRelic { .. } => {
            let stone_mesh = meshes.add(Cuboid::new(0.48, 0.36, 0.48));
            let stone_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.24, 0.23, 0.21),
                perceptual_roughness: 0.85,
                metallic: 0.12,
                ..default()
            });
            let gold_mesh = meshes.add(Cuboid::new(0.28, 0.42, 0.28));
            let gold_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.95, 0.76, 0.22),
                emissive: LinearRgba::new(4.2, 2.8, 0.45, 1.0),
                metallic: 0.85,
                perceptual_roughness: 0.25,
                ..default()
            });

            commands
                .spawn((
                    Mesh3d(stone_mesh),
                    MeshMaterial3d(stone_mat),
                    Transform::from_translation(state.pos_m),
                    component,
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Mesh3d(gold_mesh),
                        MeshMaterial3d(gold_mat),
                        Transform::from_xyz(0.0, 0.03, 0.0),
                    ));
                    // Dedicated child PointLight with NO Mesh3d!
                    parent.spawn((
                        PointLight {
                            color: Color::srgb(1.0, 0.78, 0.30),
                            intensity: 24_000.0,
                            range: 9.0,
                            shadow_maps_enabled: false,
                            ..default()
                        },
                        Transform::from_xyz(0.0, 0.35, 0.0),
                    ));
                })
                .id()
        }
        WorldItemKind::BonusLoot { .. } => {
            let crystal_mesh = meshes.add(Cuboid::new(0.18, 0.34, 0.18));
            let crystal_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.68, 0.28, 0.98),
                emissive: LinearRgba::new(3.6, 0.9, 6.2, 1.0),
                metallic: 0.35,
                perceptual_roughness: 0.18,
                ..default()
            });

            commands
                .spawn((
                    Mesh3d(crystal_mesh),
                    MeshMaterial3d(crystal_mat),
                    Transform::from_translation(state.pos_m)
                        .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.6, 0.25, 0.2)),
                    component,
                ))
                .with_children(|parent| {
                    // Dedicated child PointLight with NO Mesh3d!
                    parent.spawn((
                        PointLight {
                            color: Color::srgb(0.75, 0.35, 1.0),
                            intensity: 9_500.0,
                            range: 5.5,
                            shadow_maps_enabled: false,
                            ..default()
                        },
                        Transform::from_xyz(0.0, 0.20, 0.0),
                    ));
                })
                .id()
        }
        WorldItemKind::BatteryCell { charge } => {
            let charge_frac = charge.clamp(0.0, 1.0);
            let body_mesh = meshes.add(Cylinder::new(0.065, 0.20));
            let body_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.92, 0.74, 0.12),
                metallic: 0.35,
                perceptual_roughness: 0.45,
                ..default()
            });
            let strip_mesh = meshes.add(Cylinder::new(0.070, 0.055));
            let strip_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.12, 0.88, 0.96),
                emissive: LinearRgba::new(0.2, 2.2 * charge_frac, 2.8 * charge_frac, 1.0),
                perceptual_roughness: 0.25,
                ..default()
            });

            commands
                .spawn((
                    Mesh3d(body_mesh),
                    MeshMaterial3d(body_mat),
                    Transform::from_translation(state.pos_m),
                    component,
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Mesh3d(strip_mesh),
                        MeshMaterial3d(strip_mat),
                        Transform::from_xyz(0.0, 0.02, 0.0),
                    ));
                })
                .id()
        }
        WorldItemKind::FlarePack { .. } => {
            let pack_mesh = meshes.add(Cuboid::new(0.16, 0.09, 0.24));
            let pack_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.88, 0.16, 0.12),
                emissive: LinearRgba::new(0.8, 0.12, 0.05, 1.0),
                perceptual_roughness: 0.55,
                ..default()
            });
            let cap_mesh = meshes.add(Cuboid::new(0.17, 0.095, 0.05));
            let cap_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.95, 0.65, 0.15),
                perceptual_roughness: 0.5,
                ..default()
            });

            commands
                .spawn((
                    Mesh3d(pack_mesh),
                    MeshMaterial3d(pack_mat),
                    Transform::from_translation(state.pos_m),
                    component,
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Mesh3d(cap_mesh),
                        MeshMaterial3d(cap_mat),
                        Transform::from_xyz(0.0, 0.0, 0.10),
                    ));
                })
                .id()
        }
        WorldItemKind::FoamCanister { .. } => {
            let can_mesh = meshes.add(Cylinder::new(0.085, 0.26));
            let can_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.12, 0.68, 0.64),
                metallic: 0.4,
                perceptual_roughness: 0.35,
                ..default()
            });
            let nozzle_mesh = meshes.add(Cylinder::new(0.032, 0.07));
            let nozzle_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.18, 0.20, 0.22),
                metallic: 0.6,
                perceptual_roughness: 0.4,
                ..default()
            });

            commands
                .spawn((
                    Mesh3d(can_mesh),
                    MeshMaterial3d(can_mat),
                    Transform::from_translation(state.pos_m),
                    component,
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Mesh3d(nozzle_mesh),
                        MeshMaterial3d(nozzle_mat),
                        Transform::from_xyz(0.0, 0.15, 0.0),
                    ));
                })
                .id()
        }
        WorldItemKind::NoiseDecoy { .. } => {
            let is_active = state.active_timer_s.is_some();
            let body_mesh = meshes.add(Cylinder::new(0.085, 0.16));
            let body_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.84, 0.52, 0.16),
                emissive: if is_active {
                    LinearRgba::new(6.5, 2.4, 0.35, 1.0)
                } else {
                    LinearRgba::new(0.45, 0.18, 0.02, 1.0)
                },
                metallic: 0.7,
                perceptual_roughness: 0.32,
                ..default()
            });
            let ring_mesh = meshes.add(Cylinder::new(0.095, 0.035));
            let ring_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.95, 0.36, 0.08),
                emissive: if is_active {
                    LinearRgba::new(8.0, 2.8, 0.4, 1.0)
                } else {
                    LinearRgba::BLACK
                },
                perceptual_roughness: 0.4,
                ..default()
            });

            commands
                .spawn((
                    Mesh3d(body_mesh),
                    MeshMaterial3d(body_mat),
                    Transform::from_translation(state.pos_m),
                    component,
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Mesh3d(ring_mesh),
                        MeshMaterial3d(ring_mat),
                        Transform::from_xyz(0.0, 0.03, 0.0),
                    ));
                    if is_active {
                        // Dedicated child PointLight with NO Mesh3d when Noise Decoy is active!
                        parent.spawn((
                            PointLight {
                                color: Color::srgb(1.0, 0.45, 0.12),
                                intensity: 28_000.0,
                                range: 10.0,
                                shadow_maps_enabled: false,
                                ..default()
                            },
                            Transform::from_xyz(0.0, 0.18, 0.0),
                        ));
                    }
                })
                .id()
        }
        WorldItemKind::AidKit => {
            let box_mesh = meshes.add(Cuboid::new(0.28, 0.12, 0.22));
            let box_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.94, 0.95, 0.96),
                perceptual_roughness: 0.45,
                ..default()
            });
            let cross_h_mesh = meshes.add(Cuboid::new(0.14, 0.015, 0.04));
            let cross_v_mesh = meshes.add(Cuboid::new(0.04, 0.015, 0.14));
            let cross_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.92, 0.12, 0.12),
                emissive: LinearRgba::new(1.2, 0.08, 0.08, 1.0),
                perceptual_roughness: 0.35,
                ..default()
            });

            commands
                .spawn((
                    Mesh3d(box_mesh),
                    MeshMaterial3d(box_mat),
                    Transform::from_translation(state.pos_m),
                    component,
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Mesh3d(cross_h_mesh),
                        MeshMaterial3d(cross_mat.clone()),
                        Transform::from_xyz(0.0, 0.065, 0.0),
                    ));
                    parent.spawn((
                        Mesh3d(cross_v_mesh),
                        MeshMaterial3d(cross_mat),
                        Transform::from_xyz(0.0, 0.065, 0.0),
                    ));
                })
                .id()
        }
        WorldItemKind::Flashlight { .. } => {
            let tube_mesh = meshes.add(Cylinder::new(0.042, 0.26));
            let tube_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.18, 0.20, 0.24),
                metallic: 0.8,
                perceptual_roughness: 0.32,
                ..default()
            });
            let lens_mesh = meshes.add(Cylinder::new(0.052, 0.035));
            let lens_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.95, 0.95, 0.82),
                emissive: LinearRgba::new(1.1, 1.05, 0.8, 1.0),
                perceptual_roughness: 0.2,
                ..default()
            });

            commands
                .spawn((
                    Mesh3d(tube_mesh),
                    MeshMaterial3d(tube_mat),
                    Transform::from_translation(state.pos_m)
                        .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                    component,
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Mesh3d(lens_mesh),
                        MeshMaterial3d(lens_mat),
                        Transform::from_xyz(0.0, 0.135, 0.0),
                    ));
                })
                .id()
        }
        WorldItemKind::FallenBody { .. } => {
            let torso_mesh = meshes.add(Cuboid::new(0.46, 0.24, 0.78));
            let torso_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.16, 0.18, 0.22),
                metallic: 0.25,
                perceptual_roughness: 0.75,
                ..default()
            });
            let helmet_mesh = meshes.add(Cuboid::new(0.28, 0.26, 0.28));
            let helmet_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.22, 0.25, 0.30),
                metallic: 0.35,
                perceptual_roughness: 0.55,
                ..default()
            });
            let beacon_strip_mesh = meshes.add(Cuboid::new(0.22, 0.05, 0.14));
            let beacon_strip_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.95, 0.62, 0.14),
                emissive: LinearRgba::new(2.8, 1.4, 0.22, 1.0),
                perceptual_roughness: 0.3,
                ..default()
            });

            commands
                .spawn((
                    Mesh3d(torso_mesh),
                    MeshMaterial3d(torso_mat),
                    Transform::from_translation(state.pos_m),
                    component,
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Mesh3d(helmet_mesh),
                        MeshMaterial3d(helmet_mat),
                        Transform::from_xyz(0.0, 0.03, -0.48),
                    ));
                    parent.spawn((
                        Mesh3d(beacon_strip_mesh),
                        MeshMaterial3d(beacon_strip_mat),
                        Transform::from_xyz(0.0, 0.13, -0.05),
                    ));
                    // Dedicated child PointLight with NO Mesh3d so teammates can spot fallen friends!
                    parent.spawn((
                        PointLight {
                            color: Color::srgb(1.0, 0.65, 0.18),
                            intensity: 11_000.0,
                            range: 6.5,
                            shadow_maps_enabled: false,
                            ..default()
                        },
                        Transform::from_xyz(0.0, 0.30, 0.0),
                    ));
                })
                .id()
        }
    }
}

#[cfg(test)]
mod tests {
    use voxel_core::{VoxelConfig, generate_world_with_tier};

    use super::*;

    #[test]
    fn solo_vs_duo_carry_speed_and_scrape_rules() {
        assert!((relic_carry_speed_mult(0) - 1.0).abs() < 1e-6);
        assert!((relic_carry_speed_mult(1) - SOLO_RELIC_SPEED_MULT).abs() < 1e-6);
        assert!((relic_carry_speed_mult(2) - DUO_RELIC_SPEED_MULT).abs() < 1e-6);
        assert!((relic_carry_speed_mult(4) - DUO_RELIC_SPEED_MULT).abs() < 1e-6);

        assert!(!relic_is_scraping(0));
        assert!(relic_is_scraping(1));
        assert!(!relic_is_scraping(2));
        assert!(!relic_is_scraping(3));
    }

    #[test]
    fn world_saved_and_belt_conversions_roundtrip() {
        let kinds = [
            WorldItemKind::HeavyRelic {
                value: 310,
                weight_kg: 58.0,
            },
            WorldItemKind::BonusLoot { value: 100 },
            WorldItemKind::BatteryCell { charge: 0.64 },
            WorldItemKind::FlarePack { count: 4 },
            WorldItemKind::FoamCanister { charges: 8 },
            WorldItemKind::NoiseDecoy { count: 2 },
            WorldItemKind::AidKit,
            WorldItemKind::Flashlight { battery: 0.42 },
        ];

        for kind in &kinds {
            let saved = world_kind_to_saved(kind);
            let restored = saved_to_world_kind(&saved);
            assert_eq!(&restored, kind);
            assert!(!world_item_display_name(kind).is_empty());
        }

        assert!(is_heavy_relic(&kinds[0]));
        assert!(!is_heavy_relic(&kinds[1]));
        assert!(is_two_handed_world_item(&kinds[0]));
        assert!(!is_two_handed_world_item(&kinds[1]));

        let body = WorldItemKind::FallenBody {
            player_id: 2,
            player_name: "Scout".to_string(),
        };
        assert!(is_fallen_body(&body));
        assert!(!is_heavy_relic(&body));
        assert!(is_two_handed_world_item(&body));
        assert_eq!(world_item_display_name(&body), "Scout's Body");
        let _ = world_kind_to_saved(&body);

        assert_eq!(
            belt_item_to_world_kind(&BeltItem::Flashlight, 0.77),
            WorldItemKind::Flashlight { battery: 0.77 }
        );
        assert_eq!(
            belt_item_to_world_kind(&BeltItem::Flares { count: 3 }, 1.0),
            WorldItemKind::FlarePack { count: 3 }
        );
        assert_eq!(
            belt_item_to_world_kind(&BeltItem::AcousticFoam { charges: 6 }, 1.0),
            WorldItemKind::FoamCanister { charges: 6 }
        );
        assert_eq!(
            belt_item_to_world_kind(&BeltItem::NoiseDecoys { count: 2 }, 1.0),
            WorldItemKind::NoiseDecoy { count: 2 }
        );
        assert_eq!(
            belt_item_to_world_kind(&BeltItem::AidKit, 1.0),
            WorldItemKind::AidKit
        );
        assert_eq!(
            belt_item_to_world_kind(&BeltItem::BonusLoot { value: 95 }, 1.0),
            WorldItemKind::BonusLoot { value: 95 }
        );
    }

    #[test]
    fn pickup_into_inventory_and_battery_pouch() {
        let mut inv = PlayerInventory::starter();
        // Starter has full belt (2/2) and full pouch (2/2); free slots to test pickups cleanly.
        inv.belt = vec![None, None, None];
        inv.belt_capacity = 3;
        inv.battery_pouch.clear();
        inv.loaded_battery = None;

        // Battery goes into pouch
        assert!(
            try_pickup_world_item_into_inventory(&mut inv, &WorldItemKind::BatteryCell { charge: 0.55 })
                .is_ok()
        );
        assert_eq!(inv.battery_pouch.len(), 1);
        assert!((inv.battery_pouch[0].charge - 0.55).abs() < 1e-6);

        // Flashlight loads its battery if player has no loaded battery yet
        assert!(
            try_pickup_world_item_into_inventory(&mut inv, &WorldItemKind::Flashlight { battery: 0.80 })
                .is_ok()
        );
        assert!(inv.has_flashlight_in_belt());
        assert!((inv.loaded_battery_charge() - 0.80).abs() < 1e-6);

        // Consumables and BonusLoot go into belt
        assert!(
            try_pickup_world_item_into_inventory(&mut inv, &WorldItemKind::AidKit).is_ok()
        );
        assert!(
            try_pickup_world_item_into_inventory(&mut inv, &WorldItemKind::BonusLoot { value: 120 })
                .is_ok()
        );

        // Belt is now full (3/3): Foam canister fails cleanly
        assert!(
            try_pickup_world_item_into_inventory(&mut inv, &WorldItemKind::FoamCanister { charges: 8 })
                .is_err()
        );

        // Heavy relic and fallen body always reject standard inventory pickup
        assert_eq!(
            try_pickup_world_item_into_inventory(
                &mut inv,
                &WorldItemKind::HeavyRelic {
                    value: 300,
                    weight_kg: 58.0,
                }
            ),
            Err("Heavy relics require two-handed carry")
        );
        assert_eq!(
            try_pickup_world_item_into_inventory(
                &mut inv,
                &WorldItemKind::FallenBody {
                    player_id: 1,
                    player_name: "Valkyrie".to_string(),
                }
            ),
            Err("Fallen bodies require two-handed carry")
        );
    }

    #[test]
    fn shed_bounds_detection_and_starter_workbench_items() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        let (_world, info) = generate_world_with_tier(777, 1, cfg);

        assert!(is_pos_in_shed(info.spawn_pos_m, &info));
        assert!(is_pos_in_shed(info.shed_workbench_pos_m, &info));
        assert!(!is_pos_in_shed(info.elevator_bottom_pos_m, &info));
        assert!(!is_pos_in_shed(info.grand_cavern_pos_m, &info));

        let starters = default_starter_shed_items(info.shed_workbench_pos_m);
        assert_eq!(starters.len(), 4);
        assert!(matches!(
            starters[0].kind,
            SavedWorldItemKind::FoamCanister { charges: 8 }
        ));
        assert!(matches!(starters[1].kind, SavedWorldItemKind::AidKit));
        assert!(matches!(
            starters[2].kind,
            SavedWorldItemKind::NoiseDecoy { count: 2 }
        ));
        assert!(matches!(
            starters[3].kind,
            SavedWorldItemKind::BatteryCell { .. }
        ));
        for item in &starters {
            assert!(is_pos_in_shed(Vec3::from_array(item.pos_m), &info));
        }
    }

    #[test]
    fn collect_shed_items_snapshot_keeps_dropped_items_and_skips_active_decoys_and_cave_items() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        let (_w, info) = generate_world_with_tier(77, 1, cfg);

        let dropped_pos = info.spawn_pos_m + Vec3::new(0.8, 0.14, -0.5);
        let dropped_item = WorldItemEntity {
            id: 10,
            kind: WorldItemKind::FlarePack { count: 2 },
            vel_m: Vec3::ZERO,
            // Even if `in_shed` wasn't flagged yet (e.g. carried back from cave), spatial bounds save it!
            in_shed: false,
            carried_by: Vec::new(),
            active_timer_s: None,
            decoy_pulse_timer_s: 0.0,
            settled: true,
        };
        let dropped_tf =
            Transform::from_translation(dropped_pos).with_rotation(Quat::from_rotation_y(0.75));

        let active_decoy = WorldItemEntity {
            id: 11,
            kind: WorldItemKind::NoiseDecoy { count: 1 },
            vel_m: Vec3::ZERO,
            in_shed: true,
            carried_by: Vec::new(),
            active_timer_s: Some(8.0),
            decoy_pulse_timer_s: 0.0,
            settled: true,
        };
        let decoy_tf = Transform::from_translation(info.spawn_pos_m + Vec3::X);

        let cave_item = WorldItemEntity {
            id: 12,
            kind: WorldItemKind::BatteryCell { charge: 0.5 },
            vel_m: Vec3::ZERO,
            in_shed: false,
            carried_by: Vec::new(),
            active_timer_s: None,
            decoy_pulse_timer_s: 0.0,
            settled: true,
        };
        let cave_tf = Transform::from_translation(info.elevator_bottom_pos_m + Vec3::Y * 0.2);

        let excluded = std::collections::HashSet::new();
        let snap = collect_shed_items_snapshot(
            [
                (&dropped_item, &dropped_tf),
                (&active_decoy, &decoy_tf),
                (&cave_item, &cave_tf),
            ],
            &info,
            &excluded,
        );

        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].kind, SavedWorldItemKind::FlarePack { count: 2 });
        assert_eq!(snap[0].pos_m, dropped_pos.to_array());
        assert!((snap[0].rot_y - 0.75).abs() < 1e-4);
    }

    #[test]
    fn cave_relic_spawning_scales_with_depth_tier() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(3));
        let mut reg = WorldItemRegistry::default();
        assert_eq!(reg.next_id, 1);

        let (_w1, info1) = generate_world_with_tier(101, 1, cfg);
        let items1 = create_cave_relic_states(&info1, &mut reg.next_id);
        assert_eq!(
            items1.len(),
            info1.relic_spawns_m.len() + info1.bonus_loot_spawns_m.len()
        );
        let t1_relic_val = match items1[0].kind {
            WorldItemKind::HeavyRelic { value, weight_kg } => {
                assert!((weight_kg - 58.0).abs() < 1e-6);
                value
            }
            ref other => panic!("expected HeavyRelic, got {other:?}"),
        };
        assert_eq!(t1_relic_val, 220 + 90);

        let (_w3, info3) = generate_world_with_tier(101, 3, cfg);
        let items3 = create_cave_relic_states(&info3, &mut reg.next_id);
        let t3_relic_count = items3.iter().filter(|i| is_heavy_relic(&i.kind)).count();
        assert_eq!(t3_relic_count, info3.relic_spawns_m.len());
        assert!(t3_relic_count > info1.relic_spawns_m.len());

        let t3_relic_val = match items3[0].kind {
            WorldItemKind::HeavyRelic { value, .. } => value,
            ref other => panic!("expected HeavyRelic, got {other:?}"),
        };
        assert_eq!(t3_relic_val, 220 + 3 * 90);
        assert!(t3_relic_val > t1_relic_val);

        // Verify unique IDs and to_net_state roundtrip
        let first = &items3[0];
        let ent = WorldItemEntity {
            id: first.id,
            kind: first.kind.clone(),
            vel_m: first.vel_m,
            in_shed: first.in_shed,
            carried_by: vec![0, 1],
            active_timer_s: None,
            decoy_pulse_timer_s: 0.0,
            settled: true,
        };
        let net = to_net_state(&ent, first.pos_m);
        assert_eq!(net.id, first.id);
        assert_eq!(net.carried_by, vec![0, 1]);
        assert_eq!(net.pos_m, first.pos_m);
    }

    #[test]
    fn spawn_world_item_visual_isolates_point_lights_from_meshes() {
        let mut world = World::new();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();

        let states = [
            WorldItemNetState {
                id: 1,
                kind: WorldItemKind::HeavyRelic {
                    value: 310,
                    weight_kg: 58.0,
                },
                pos_m: Vec3::new(1.0, 2.0, 3.0),
                vel_m: Vec3::ZERO,
                in_shed: false,
                carried_by: Vec::new(),
                active_timer_s: None,
            },
            WorldItemNetState {
                id: 2,
                kind: WorldItemKind::NoiseDecoy { count: 1 },
                pos_m: Vec3::new(4.0, 2.0, 3.0),
                vel_m: Vec3::ZERO,
                in_shed: false,
                carried_by: Vec::new(),
                active_timer_s: Some(DECOY_ACTIVE_DURATION_S),
            },
            WorldItemNetState {
                id: 3,
                kind: WorldItemKind::AidKit,
                pos_m: Vec3::new(6.0, 2.0, 3.0),
                vel_m: Vec3::ZERO,
                in_shed: true,
                carried_by: Vec::new(),
                active_timer_s: None,
            },
            WorldItemNetState {
                id: 4,
                kind: WorldItemKind::FallenBody {
                    player_id: 2,
                    player_name: "Scout".to_string(),
                },
                pos_m: Vec3::new(8.0, 2.0, 3.0),
                vel_m: Vec3::ZERO,
                in_shed: false,
                carried_by: Vec::new(),
                active_timer_s: None,
            },
        ];

        {
            let mut commands = world.commands();
            for state in &states {
                spawn_world_item_visual(&mut commands, &mut meshes, &mut materials, state);
            }
        }
        world.flush();

        let mut item_q = world.query::<&WorldItemEntity>();
        assert_eq!(item_q.iter(&world).count(), 4);

        let mut light_q = world.query::<(&PointLight, Option<&Mesh3d>)>();
        let lights: Vec<_> = light_q.iter(&world).collect();
        assert_eq!(lights.len(), 3);
        for (_, mesh) in lights {
            assert!(
                mesh.is_none(),
                "WorldItem PointLight must live on a dedicated child entity with no Mesh3d"
            );
        }
    }
}
