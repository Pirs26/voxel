//! Host-authoritative campaign save files (`saves/<slot>.json`) keyed by Player Name,
//! persisting shared credits, depth tiers, Base Shed world items, and per-player inventories.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::inventory::PlayerInventory;

pub const SAVE_VERSION: u32 = 1;
pub const STARTER_CREDITS: i32 = 100;
pub const UNRECOVERED_COMPANION_FINE: i32 = 50;
pub const DEFAULT_SAVE_SLOT: &str = "default";

/// Physical items persisted inside the Base Shed across dives and sessions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SavedWorldItemKind {
    HeavyRelic { value: u32, weight_kg: f32 },
    BonusLoot { value: u32 },
    BatteryCell { charge: f32 },
    FlarePack { count: u8 },
    FoamCanister { charges: u8 },
    NoiseDecoy { count: u8 },
    AidKit,
    Flashlight { battery: f32 },
}

/// A serialized physical object placed in the persistent Base Shed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedWorldItem {
    pub kind: SavedWorldItemKind,
    pub pos_m: [f32; 3],
    pub rot_y: f32,
}

/// Persistent per-player profile keyed by normalized player name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedPlayerProfile {
    pub player_name: String,
    pub visor_idx: usize,
    pub inventory: PlayerInventory,
}

/// Root campaign save document written to `saves/<slot>.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CampaignSave {
    pub version: u32,
    pub credits: i32,
    pub unlocked_depth_tier: u8,
    pub selected_depth_tier: u8,
    pub dives_completed: u32,
    pub shed_items: Vec<SavedWorldItem>,
    pub players: HashMap<String, SavedPlayerProfile>,
}

impl Default for CampaignSave {
    fn default() -> Self {
        Self::default_new()
    }
}

/// Normalizes a player display name into a stable lookup key (trimmed + lowercase,
/// falling back to `"miner"` when empty).
pub fn normalize_player_key(name: &str) -> String {
    let normalized = name.trim().to_lowercase();
    if normalized.is_empty() {
        "miner".to_string()
    } else {
        normalized
    }
}

fn sanitize_slot_name(slot: &str) -> String {
    let cleaned: String = slot
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        DEFAULT_SAVE_SLOT.to_string()
    } else {
        trimmed.to_string()
    }
}

impl CampaignSave {
    /// Creates a fresh campaign save with starter credits and Depth Tier 1 unlocked.
    pub fn default_new() -> Self {
        Self {
            version: SAVE_VERSION,
            credits: STARTER_CREDITS,
            unlocked_depth_tier: 1,
            selected_depth_tier: 1,
            dives_completed: 0,
            shed_items: Vec::new(),
            players: HashMap::new(),
        }
    }

    /// Retrieves an existing player's profile by normalized name, or inserts and returns
    /// a fresh starter profile using `default_visor_idx`.
    pub fn get_or_create_player(
        &mut self,
        name: &str,
        default_visor_idx: usize,
    ) -> SavedPlayerProfile {
        let key = normalize_player_key(name);
        let display_name = {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                "Miner".to_string()
            } else {
                trimmed.to_string()
            }
        };

        self.players
            .entry(key)
            .or_insert_with(|| SavedPlayerProfile {
                player_name: display_name,
                visor_idx: default_visor_idx,
                inventory: PlayerInventory::starter(),
            })
            .clone()
    }

    /// Inserts or updates a player's persistent profile keyed by normalized `player_name`.
    pub fn upsert_player(&mut self, profile: SavedPlayerProfile) {
        let key = normalize_player_key(&profile.player_name);
        self.players.insert(key, profile);
    }

    /// Writes this campaign save as pretty-printed JSON to `path`, creating parent
    /// directories automatically if they do not exist.
    pub fn save_to_path(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|err| {
                format!("Failed to create save directory {}: {err}", parent.display())
            })?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|err| format!("Failed to serialize campaign save: {err}"))?;
        fs::write(path, json)
            .map_err(|err| format!("Failed to write save file {}: {err}", path.display()))
    }

    /// Loads and validates a campaign save JSON file from `path`.
    pub fn load_from_path(path: &Path) -> Result<Self, String> {
        let raw = fs::read_to_string(path)
            .map_err(|err| format!("Failed to read save file {}: {err}", path.display()))?;
        let mut save: Self = serde_json::from_str(&raw)
            .map_err(|err| format!("Failed to parse save file {}: {err}", path.display()))?;
        save.unlocked_depth_tier = save.unlocked_depth_tier.max(1);
        save.selected_depth_tier = save
            .selected_depth_tier
            .clamp(1, save.unlocked_depth_tier);
        for profile in save.players.values_mut() {
            profile.inventory.normalize_slots();
        }
        Ok(save)
    }

    /// Resolves a campaign slot name to `saves/<safe_slot>.json`.
    pub fn slot_path(slot: &str) -> PathBuf {
        let safe_slot = sanitize_slot_name(slot);
        PathBuf::from("saves").join(format!("{safe_slot}.json"))
    }

    /// Saves this campaign to `saves/<safe_slot>.json` and returns the written path.
    pub fn save_slot(&self, slot: &str) -> Result<PathBuf, String> {
        let path = Self::slot_path(slot);
        self.save_to_path(&path)?;
        Ok(path)
    }

    /// Loads `saves/<safe_slot>.json`, falling back to `CampaignSave::default_new()`
    /// if the file is missing or unreadable.
    #[allow(dead_code)]
    pub fn load_or_default_slot(slot: &str) -> Self {
        let path = Self::slot_path(slot);
        Self::load_from_path(&path).unwrap_or_else(|_| Self::default_new())
    }
}

/// Summary metadata for a saved campaign slot displayed on the Main Menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveSlotSummary {
    pub slot_name: String,
    pub credits: i32,
    pub unlocked_depth_tier: u8,
    pub dives_completed: u32,
    pub player_count: usize,
    pub exists_on_disk: bool,
}

/// Active selected campaign slot on the Main Menu (loaded on Play Solo or Host Co-op Lobby).
#[derive(Resource, Clone, Debug)]
pub struct SaveSlotBrowser {
    pub slots: Vec<SaveSlotSummary>,
    pub selected_idx: usize,
}

impl Default for SaveSlotBrowser {
    fn default() -> Self {
        let mut b = Self {
            slots: Vec::new(),
            selected_idx: 0,
        };
        b.refresh();
        b
    }
}

impl SaveSlotBrowser {
    pub fn refresh(&mut self) {
        let prev_name = self.selected_slot_name().to_string();
        self.slots = list_save_slots_in_dir(Path::new("saves"));
        if self.slots.is_empty() {
            self.slots.push(SaveSlotSummary {
                slot_name: DEFAULT_SAVE_SLOT.to_string(),
                credits: STARTER_CREDITS,
                unlocked_depth_tier: 1,
                dives_completed: 0,
                player_count: 0,
                exists_on_disk: false,
            });
        }
        self.selected_idx = self
            .slots
            .iter()
            .position(|s| s.slot_name == prev_name)
            .unwrap_or(0);
    }

    pub fn selected(&self) -> Option<&SaveSlotSummary> {
        self.slots.get(self.selected_idx).or_else(|| self.slots.first())
    }

    pub fn selected_slot_name(&self) -> &str {
        self.selected()
            .map(|s| s.slot_name.as_str())
            .unwrap_or(DEFAULT_SAVE_SLOT)
    }

    #[allow(dead_code)]
    pub fn step(&mut self, delta: i32) {
        if self.slots.is_empty() {
            return;
        }
        let len = self.slots.len() as i32;
        self.selected_idx = (self.selected_idx as i32 + delta).rem_euclid(len) as usize;
    }

    /// Creates a brand-new numbered campaign slot (`expedition_2`, `expedition_3`, ...) on disk
    /// and selects it without touching any existing save files.
    pub fn create_new_slot(&mut self) -> Result<String, String> {
        self.refresh();
        let mut num = 1u32;
        let name = loop {
            let cand = if num == 1 && !self.slots.iter().any(|s| s.slot_name == DEFAULT_SAVE_SLOT && s.exists_on_disk) {
                DEFAULT_SAVE_SLOT.to_string()
            } else {
                format!("expedition_{num}")
            };
            if !self.slots.iter().any(|s| s.slot_name == cand && s.exists_on_disk) {
                break cand;
            }
            num += 1;
        };
        let fresh = CampaignSave::default_new();
        fresh.save_slot(&name)?;
        self.refresh();
        if let Some(idx) = self.slots.iter().position(|s| s.slot_name == name) {
            self.selected_idx = idx;
        }
        Ok(name)
    }
}

/// Scans `dir` for campaign save `.json` files (excluding `settings.json`) and returns sorted summaries.
pub fn list_save_slots_in_dir(dir: &Path) -> Vec<SaveSlotSummary> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem.eq_ignore_ascii_case("settings") {
            continue;
        }
        if let Ok(save) = CampaignSave::load_from_path(&path) {
            out.push(SaveSlotSummary {
                slot_name: stem.to_string(),
                credits: save.credits,
                unlocked_depth_tier: save.unlocked_depth_tier,
                dives_completed: save.dives_completed,
                player_count: save.players.len(),
                exists_on_disk: true,
            });
        }
    }
    out.sort_by(|a, b| {
        let a_def = a.slot_name == DEFAULT_SAVE_SLOT;
        let b_def = b.slot_name == DEFAULT_SAVE_SLOT;
        b_def.cmp(&a_def).then_with(|| a.slot_name.cmp(&b.slot_name))
    });
    out
}

/// Active Bevy resource wrapping the host's loaded campaign save and dirty flag.
#[derive(Resource, Clone, Debug)]
pub struct CampaignState {
    pub save: CampaignSave,
    pub slot_name: String,
    pub dirty: bool,
    pub last_status: String,
}

impl Default for CampaignState {
    fn default() -> Self {
        Self {
            save: CampaignSave::default_new(),
            slot_name: DEFAULT_SAVE_SLOT.to_string(),
            dirty: false,
            last_status: "Initialized default campaign".to_string(),
        }
    }
}

impl CampaignState {
    /// Loads the default campaign slot from disk (`saves/default.json`), or starts a fresh
    /// campaign if no save file exists yet.
    pub fn load_default() -> Self {
        Self::load_slot(DEFAULT_SAVE_SLOT)
    }

    /// Loads the named campaign slot from disk, or initializes a new campaign if absent.
    pub fn load_slot(slot: &str) -> Self {
        let slot_name = sanitize_slot_name(slot);
        let path = CampaignSave::slot_path(&slot_name);
        match CampaignSave::load_from_path(&path) {
            Ok(save) => Self {
                save,
                slot_name,
                dirty: false,
                last_status: format!("Loaded campaign from {}", path.display()),
            },
            Err(_) => Self {
                save: CampaignSave::default_new(),
                slot_name,
                dirty: false,
                last_status: format!("Started new campaign ({})", path.display()),
            },
        }
    }

    /// Writes the active campaign save to disk immediately and clears the `dirty` flag.
    pub fn save_now(&mut self) -> Result<PathBuf, String> {
        let path = self.save.save_slot(&self.slot_name)?;
        self.dirty = false;
        self.last_status = format!("Saved campaign to {}", path.display());
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::inventory::{BatteryCellItem, BeltItem};

    fn unique_temp_save_path(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        std::env::temp_dir()
            .join(format!("echoes_below_save_test_{tag}_{nanos}"))
            .join("campaign.json")
    }

    #[test]
    fn normalize_player_key_trims_lowercases_and_falls_back() {
        assert_eq!(normalize_player_key("  Alice  "), "alice");
        assert_eq!(normalize_player_key("BoB_99"), "bob_99");
        assert_eq!(normalize_player_key("   "), "miner");
        assert_eq!(normalize_player_key(""), "miner");
    }

    #[test]
    fn slot_path_sanitizes_unsafe_characters() {
        assert_eq!(
            CampaignSave::slot_path("Friday Night!"),
            PathBuf::from("saves").join("friday_night.json")
        );
        assert_eq!(
            CampaignSave::slot_path("../escape/slot"),
            PathBuf::from("saves").join("escape_slot.json")
        );
        assert_eq!(
            CampaignSave::slot_path("   "),
            PathBuf::from("saves").join("default.json")
        );
    }

    #[test]
    fn get_or_create_and_upsert_player_profiles() {
        let mut save = CampaignSave::default_new();
        assert_eq!(save.version, 1);
        assert_eq!(save.credits, 100);
        assert_eq!(save.unlocked_depth_tier, 1);
        assert_eq!(save.selected_depth_tier, 1);

        let mut p1 = save.get_or_create_player("  Valkyrie ", 2);
        assert_eq!(p1.player_name, "Valkyrie");
        assert_eq!(p1.visor_idx, 2);
        assert_eq!(p1.inventory, PlayerInventory::starter());

        // Modify inventory & visor, upsert, and retrieve with different casing.
        p1.visor_idx = 4;
        p1.inventory.passive.headlamp = true;
        p1.inventory.upgrade_belt();
        save.upsert_player(p1.clone());

        let loaded = save.get_or_create_player("valkyrie", 0);
        assert_eq!(loaded, p1);
        assert_eq!(save.players.len(), 1);
    }

    #[test]
    fn save_and_load_roundtrip_on_disk() {
        let path = unique_temp_save_path("roundtrip");
        let mut save = CampaignSave::default_new();
        save.credits = 425;
        save.unlocked_depth_tier = 3;
        save.selected_depth_tier = 2;
        save.dives_completed = 5;
        save.shed_items.push(SavedWorldItem {
            kind: SavedWorldItemKind::HeavyRelic {
                value: 250,
                weight_kg: 42.5,
            },
            pos_m: [1.5, 0.9, -2.25],
            rot_y: 1.2,
        });
        save.shed_items.push(SavedWorldItem {
            kind: SavedWorldItemKind::BatteryCell { charge: 0.65 },
            pos_m: [2.0, 1.1, 0.5],
            rot_y: 0.0,
        });
        save.shed_items.push(SavedWorldItem {
            kind: SavedWorldItemKind::Flashlight { battery: 0.40 },
            pos_m: [-1.0, 1.0, 0.0],
            rot_y: 0.5,
        });

        let mut host = save.get_or_create_player("HostPlayer", 1);
        host.inventory.passive.stealth_boots = true;
        host.inventory.loaded_battery = Some(BatteryCellItem::with_charge(0.75));
        host.inventory.upgrade_belt();
        host.inventory
            .try_add_belt_item(BeltItem::AidKit)
            .expect("aid kit fits");
        save.upsert_player(host);

        save.save_to_path(&path).expect("save should succeed");
        let reloaded = CampaignSave::load_from_path(&path).expect("load should succeed");
        assert_eq!(reloaded, save);

        // Verify negative credits (company debt) roundtrip cleanly in JSON.
        save.credits = -150;
        save.save_to_path(&path).expect("save negative credits should succeed");
        let reloaded_debt =
            CampaignSave::load_from_path(&path).expect("load negative credits should succeed");
        assert_eq!(reloaded_debt.credits, -150);
        assert_eq!(reloaded_debt, save);

        // Cleanup temporary directory.
        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn load_from_missing_path_returns_err() {
        let missing = unique_temp_save_path("missing");
        assert!(CampaignSave::load_from_path(&missing).is_err());
    }

    #[test]
    fn list_save_slots_discovers_campaigns_and_ignores_settings_json() {
        let path = unique_temp_save_path("list_slots");
        let dir = path.parent().expect("temp dir");
        fs::create_dir_all(dir).expect("create dir");

        let mut s1 = CampaignSave::default_new();
        s1.credits = 250;
        s1.dives_completed = 2;
        s1.save_to_path(&dir.join("default.json")).expect("save default");

        let mut s2 = CampaignSave::default_new();
        s2.credits = 900;
        s2.unlocked_depth_tier = 3;
        s2.save_to_path(&dir.join("friday_coop.json")).expect("save coop");

        // settings.json must be ignored
        fs::write(dir.join("settings.json"), r#"{"fov_deg":80.0}"#).expect("write settings");

        let found = list_save_slots_in_dir(dir);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].slot_name, "default");
        assert_eq!(found[0].credits, 250);
        assert_eq!(found[1].slot_name, "friday_coop");
        assert_eq!(found[1].credits, 900);
        assert_eq!(found[1].unlocked_depth_tier, 3);

        let _ = fs::remove_dir_all(dir);
    }
}
