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
pub const STARTER_CREDITS: i32 = 60;
pub const UNRECOVERED_COMPANION_FINE: i32 = 120;
pub const DEFAULT_SAVE_SLOT: &str = "default";
pub const MAX_SAVE_SLOT_NAME_LEN: usize = 28;
pub const MAX_PARKOUR_LEADERBOARD: usize = 8;

/// A single completed official Parkour Room run persisted in the campaign save
/// (the leaderboard keeps only each player's single fastest Personal Best).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParkourRecord {
    pub player_name: String,
    pub time_ms: u32,
}

/// Result of submitting a timed Parkour Room finish to [`CampaignSave::record_parkour_run`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParkourRunOutcome {
    /// 1-based leaderboard rank of this player's Personal Best if within top [`MAX_PARKOUR_LEADERBOARD`].
    pub rank: Option<usize>,
    /// `true` if this run set a new Personal Best for the player.
    pub is_personal_best: bool,
    /// Player's Personal Best time after this run (ms).
    pub best_time_ms: u32,
}

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
    #[serde(default)]
    pub shed_initialized: bool,
    #[serde(default)]
    pub winch_overdrive: bool,
    #[serde(default)]
    pub fast_charger: bool,
    pub shed_items: Vec<SavedWorldItem>,
    pub players: HashMap<String, SavedPlayerProfile>,
    #[serde(default)]
    pub parkour_leaderboard: Vec<ParkourRecord>,
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

/// Validates and normalizes a user-supplied campaign save slot name into a safe filename stem.
pub fn validate_save_slot_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || !trimmed.chars().any(|c| c.is_ascii_alphanumeric()) {
        return Err("Enter a save name using letters or numbers".to_string());
    }
    let safe = sanitize_slot_name(trimmed);
    if safe.eq_ignore_ascii_case("settings") {
        return Err("The name 'settings' is reserved".to_string());
    }
    if safe.len() > MAX_SAVE_SLOT_NAME_LEN {
        return Err(format!(
            "Save name must be at most {MAX_SAVE_SLOT_NAME_LEN} characters"
        ));
    }
    Ok(safe)
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
            shed_initialized: false,
            winch_overdrive: false,
            fast_charger: false,
            shed_items: Vec::new(),
            players: HashMap::new(),
            parkour_leaderboard: Vec::new(),
        }
    }

    /// Records an official timed Parkour Room run while keeping ONLY each player's single
    /// fastest Personal Best (matched case-insensitively via [`normalize_player_key`]),
    /// sorted ascending by time and capped at [`MAX_PARKOUR_LEADERBOARD`].
    pub fn record_parkour_run(&mut self, player_name: &str, time_ms: u32) -> ParkourRunOutcome {
        let trimmed = player_name.trim();
        let display_name: String = if trimmed.is_empty() {
            "Miner".to_string()
        } else {
            trimmed.chars().take(20).collect()
        };
        let key = normalize_player_key(&display_name);
        let clamped_ms = time_ms.max(100);

        self.deduplicate_parkour_leaderboard();

        let (is_personal_best, best_time_ms) = if let Some(existing) = self
            .parkour_leaderboard
            .iter_mut()
            .find(|r| normalize_player_key(&r.player_name) == key)
        {
            if clamped_ms < existing.time_ms {
                existing.time_ms = clamped_ms;
                existing.player_name = display_name;
                (true, clamped_ms)
            } else {
                (false, existing.time_ms)
            }
        } else {
            self.parkour_leaderboard.push(ParkourRecord {
                player_name: display_name,
                time_ms: clamped_ms,
            });
            (true, clamped_ms)
        };

        self.parkour_leaderboard.sort_by_key(|r| r.time_ms);
        self.parkour_leaderboard.truncate(MAX_PARKOUR_LEADERBOARD);

        let rank = self
            .parkour_leaderboard
            .iter()
            .position(|r| normalize_player_key(&r.player_name) == key)
            .map(|i| i + 1);

        ParkourRunOutcome {
            rank,
            is_personal_best,
            best_time_ms,
        }
    }

    /// Deduplicates any legacy leaderboard entries so only the single best time per player remains.
    pub fn deduplicate_parkour_leaderboard(&mut self) {
        let mut best_by_player: HashMap<String, ParkourRecord> = HashMap::new();
        for rec in self.parkour_leaderboard.drain(..) {
            let key = normalize_player_key(&rec.player_name);
            best_by_player
                .entry(key)
                .and_modify(|e| {
                    if rec.time_ms < e.time_ms {
                        *e = rec.clone();
                    }
                })
                .or_insert(rec);
        }
        let mut deduped: Vec<ParkourRecord> = best_by_player.into_values().collect();
        deduped.sort_by_key(|r| r.time_ms);
        deduped.truncate(MAX_PARKOUR_LEADERBOARD);
        self.parkour_leaderboard = deduped;
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
        if !save.shed_items.is_empty() || save.dives_completed > 0 {
            save.shed_initialized = true;
        }
        save.deduplicate_parkour_leaderboard();
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
        self.refresh_in_dir(Path::new("saves"));
    }

    fn refresh_in_dir(&mut self, dir: &Path) {
        let prev_name = self.selected().map(|s| s.slot_name.clone());
        self.slots = list_save_slots_in_dir(dir);
        self.selected_idx = prev_name
            .as_deref()
            .and_then(|prev| self.slots.iter().position(|s| s.slot_name == prev))
            .unwrap_or(0);
    }

    pub fn selected(&self) -> Option<&SaveSlotSummary> {
        self.slots.get(self.selected_idx).or_else(|| self.slots.first())
    }

    /// Returns `true` when at least one campaign save exists on disk and is currently selected.
    pub fn has_selected_world(&self) -> bool {
        self.selected().is_some_and(|s| s.exists_on_disk)
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

    /// Computes the next unused default save name (`default`, `expedition_1`, `expedition_2`, ...).
    pub fn next_default_new_slot_name(&self) -> String {
        let mut num = 1u32;
        loop {
            let cand = if num == 1
                && !self
                    .slots
                    .iter()
                    .any(|s| s.slot_name == DEFAULT_SAVE_SLOT && s.exists_on_disk)
            {
                DEFAULT_SAVE_SLOT.to_string()
            } else {
                format!("expedition_{num}")
            };
            if !self
                .slots
                .iter()
                .any(|s| s.slot_name == cand && s.exists_on_disk)
            {
                return cand;
            }
            num += 1;
        }
    }

    /// Creates a brand-new named campaign save slot on disk (using [`Self::next_default_new_slot_name`]
    /// if `requested_name` is blank) and selects it without overwriting existing saves.
    pub fn create_named_slot(&mut self, requested_name: &str) -> Result<String, String> {
        self.create_named_slot_in_dir(Path::new("saves"), requested_name)
    }

    pub fn create_named_slot_in_dir(
        &mut self,
        dir: &Path,
        requested_name: &str,
    ) -> Result<String, String> {
        self.refresh_in_dir(dir);
        let name = if requested_name.trim().is_empty() {
            self.next_default_new_slot_name()
        } else {
            validate_save_slot_name(requested_name)?
        };
        let path = dir.join(format!("{name}.json"));
        if path.exists() {
            return Err(format!("A save named '{name}' already exists"));
        }
        let fresh = CampaignSave::default_new();
        fresh.save_to_path(&path)?;
        self.refresh_in_dir(dir);
        if let Some(idx) = self.slots.iter().position(|s| s.slot_name == name) {
            self.selected_idx = idx;
        }
        Ok(name)
    }

    /// Renames the currently selected campaign save slot on disk (preserving all progress)
    /// and updates the selection to the new name.
    pub fn rename_selected_slot(&mut self, new_name: &str) -> Result<String, String> {
        self.rename_selected_slot_in_dir(Path::new("saves"), new_name)
    }

    pub fn rename_selected_slot_in_dir(
        &mut self,
        dir: &Path,
        new_name: &str,
    ) -> Result<String, String> {
        self.refresh_in_dir(dir);
        if !self.has_selected_world() {
            return Err("No save slot selected to rename".to_string());
        }
        let safe_new = validate_save_slot_name(new_name)?;
        let old_name = self.selected_slot_name().to_string();
        if safe_new == old_name {
            return Ok(safe_new);
        }

        let old_path = dir.join(format!("{old_name}.json"));
        let new_path = dir.join(format!("{safe_new}.json"));
        if !old_name.eq_ignore_ascii_case(&safe_new) && new_path.exists() {
            return Err(format!("A save named '{safe_new}' already exists"));
        }

        if old_path.exists() {
            fs::rename(&old_path, &new_path).map_err(|err| {
                format!(
                    "Failed to rename {} to {}: {err}",
                    old_path.display(),
                    new_path.display()
                )
            })?;
        } else {
            // Starter placeholder slot not yet written to disk: create it under the requested name.
            CampaignSave::default_new().save_to_path(&new_path)?;
        }

        self.refresh_in_dir(dir);
        if let Some(idx) = self.slots.iter().position(|s| s.slot_name == safe_new) {
            self.selected_idx = idx;
        }
        Ok(safe_new)
    }

    /// Permanently deletes the currently selected save slot `.json` file from disk
    /// and selects the next remaining slot (or the unsaved `default` placeholder if none remain).
    pub fn delete_selected_slot(&mut self) -> Result<String, String> {
        self.delete_selected_slot_in_dir(Path::new("saves"))
    }

    pub fn delete_selected_slot_in_dir(&mut self, dir: &Path) -> Result<String, String> {
        self.refresh_in_dir(dir);
        let Some(selected) = self.selected().cloned() else {
            return Err("No save slot selected".to_string());
        };
        if !selected.exists_on_disk {
            return Err("No saved file on disk to delete".to_string());
        }
        let path = dir.join(format!("{}.json", selected.slot_name));
        if !path.exists() {
            self.refresh_in_dir(dir);
            return Err("Save file was already removed".to_string());
        }
        fs::remove_file(&path)
            .map_err(|err| format!("Failed to delete {}: {err}", path.display()))?;
        let deleted = selected.slot_name;
        self.refresh_in_dir(dir);
        Ok(deleted)
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
        assert_eq!(save.credits, 60);
        assert_eq!(save.unlocked_depth_tier, 1);
        assert_eq!(save.selected_depth_tier, 1);
        assert!(!save.winch_overdrive);
        assert!(!save.fast_charger);

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
        save.shed_initialized = true;
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

    #[test]
    fn validate_save_slot_name_handles_valid_reserved_and_empty() {
        assert_eq!(
            validate_save_slot_name("Friday Squad!").unwrap(),
            "friday_squad"
        );
        assert_eq!(validate_save_slot_name("Tier-3_Run").unwrap(), "tier-3_run");
        assert!(validate_save_slot_name("   ").is_err());
        assert!(validate_save_slot_name("!!!").is_err());
        assert!(validate_save_slot_name("Settings").is_err());
        assert!(validate_save_slot_name(&"a".repeat(MAX_SAVE_SLOT_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn create_named_and_rename_save_slots_preserve_data_and_prevent_duplicates() {
        let path = unique_temp_save_path("create_rename");
        let dir = path.parent().expect("temp dir");
        fs::create_dir_all(dir).expect("create dir");

        let mut browser = SaveSlotBrowser {
            slots: Vec::new(),
            selected_idx: 0,
        };
        browser.refresh_in_dir(dir);

        // 1. Create a custom-named save slot.
        let created = browser
            .create_named_slot_in_dir(dir, "Deep Boys")
            .expect("create named save");
        assert_eq!(created, "deep_boys");
        assert_eq!(browser.selected_slot_name(), "deep_boys");

        // Duplicate name must be rejected cleanly without overwriting.
        assert!(browser.create_named_slot_in_dir(dir, "deep boys").is_err());

        // Blank name uses next_default_new_slot_name cleanly.
        let auto = browser
            .create_named_slot_in_dir(dir, "   ")
            .expect("auto-named save");
        assert_eq!(auto, DEFAULT_SAVE_SLOT);

        // Modify saved credits on `deep_boys` so we can confirm rename preserves contents.
        let mut custom = CampaignSave::load_from_path(&dir.join("deep_boys.json")).unwrap();
        custom.credits = 777;
        custom.dives_completed = 4;
        custom
            .save_to_path(&dir.join("deep_boys.json"))
            .expect("update custom save");

        browser.refresh_in_dir(dir);
        browser.selected_idx = browser
            .slots
            .iter()
            .position(|s| s.slot_name == "deep_boys")
            .expect("find deep_boys");

        // Renaming onto an existing slot must fail.
        assert!(browser.rename_selected_slot_in_dir(dir, "default").is_err());

        // Renaming to a new name preserves credits/dives and removes the old file.
        let renamed = browser
            .rename_selected_slot_in_dir(dir, "Abyssal Crew")
            .expect("rename save");
        assert_eq!(renamed, "abyssal_crew");
        assert_eq!(browser.selected_slot_name(), "abyssal_crew");
        assert!(!dir.join("deep_boys.json").exists());
        let reloaded = CampaignSave::load_from_path(&dir.join("abyssal_crew.json")).unwrap();
        assert_eq!(reloaded.credits, 777);
        assert_eq!(reloaded.dives_completed, 4);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn parkour_leaderboard_keeps_only_best_run_per_player_and_caps_top_entries() {
        let mut save = CampaignSave::default_new();
        let bob1 = save.record_parkour_run("Bob", 18_500);
        assert_eq!(bob1.rank, Some(1));
        assert!(bob1.is_personal_best);

        let alice1 = save.record_parkour_run("Alice", 12_340);
        assert_eq!(alice1.rank, Some(1));
        assert!(alice1.is_personal_best);

        // Slower run by Alice (different casing) must NOT duplicate or overwrite her PB
        let alice_slow = save.record_parkour_run("alice", 16_000);
        assert!(!alice_slow.is_personal_best);
        assert_eq!(alice_slow.best_time_ms, 12_340);
        assert_eq!(alice_slow.rank, Some(1));
        assert_eq!(save.parkour_leaderboard.len(), 2);

        // Faster run by Bob replaces his old entry and overtakes Alice for #1
        let bob_pb = save.record_parkour_run("Bob", 11_200);
        assert!(bob_pb.is_personal_best);
        assert_eq!(bob_pb.rank, Some(1));
        assert_eq!(save.parkour_leaderboard.len(), 2);
        assert_eq!(save.parkour_leaderboard[0].player_name, "Bob");
        assert_eq!(save.parkour_leaderboard[0].time_ms, 11_200);
        assert_eq!(save.parkour_leaderboard[1].player_name, "Alice");

        // Adding 8 more unique runners caps the leaderboard at MAX_PARKOUR_LEADERBOARD
        for i in 0..8 {
            save.record_parkour_run(&format!("Runner_{i}"), 20_000 + i * 500);
        }
        assert_eq!(save.parkour_leaderboard.len(), MAX_PARKOUR_LEADERBOARD);
        assert_eq!(save.record_parkour_run("Slowest", 99_999).rank, None);
    }

    #[test]
    fn delete_selected_slot_removes_file_and_handles_last_save() {
        let path = unique_temp_save_path("delete_slot");
        let dir = path.parent().expect("temp dir");
        fs::create_dir_all(dir).expect("create dir");

        let mut browser = SaveSlotBrowser {
            slots: Vec::new(),
            selected_idx: 0,
        };
        browser.refresh_in_dir(dir);
        // Empty save directory must have no slots and has_selected_world() == false
        assert!(browser.slots.is_empty());
        assert!(!browser.has_selected_world());
        assert!(browser.selected().is_none());
        assert!(browser.delete_selected_slot_in_dir(dir).is_err());
        assert!(browser.rename_selected_slot_in_dir(dir, "renamed").is_err());

        browser
            .create_named_slot_in_dir(dir, "alpha")
            .expect("create alpha");
        assert!(browser.has_selected_world());
        browser
            .create_named_slot_in_dir(dir, "beta")
            .expect("create beta");
        assert!(browser.has_selected_world());
        assert_eq!(browser.selected_slot_name(), "beta");

        let deleted = browser
            .delete_selected_slot_in_dir(dir)
            .expect("delete beta");
        assert_eq!(deleted, "beta");
        assert!(!dir.join("beta.json").exists());
        assert!(browser.has_selected_world());
        assert_eq!(browser.selected_slot_name(), "alpha");

        let deleted_last = browser
            .delete_selected_slot_in_dir(dir)
            .expect("delete alpha");
        assert_eq!(deleted_last, "alpha");
        assert!(!dir.join("alpha.json").exists());
        assert!(browser.slots.is_empty());
        assert!(!browser.has_selected_world());
        assert!(browser.selected().is_none());
        assert!(browser.delete_selected_slot_in_dir(dir).is_err());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn campaign_save_deserializes_legacy_json_without_new_upgrade_fields() {
        let legacy_json = r#"{
            "version": 1,
            "credits": 240,
            "unlocked_depth_tier": 2,
            "selected_depth_tier": 2,
            "dives_completed": 3,
            "shed_initialized": true,
            "shed_items": [],
            "players": {
                "miner": {
                    "player_name": "Miner",
                    "visor_idx": 0,
                    "inventory": {
                        "belt_capacity": 2,
                        "belt": ["Flashlight", {"Flares": {"count": 3}}],
                        "active_slot": 0,
                        "battery_pouch_capacity": 2,
                        "battery_pouch": [{"charge": 1.0, "capacity_scale": 1.0}],
                        "loaded_battery": {"charge": 1.0, "capacity_scale": 1.0},
                        "passive": {
                            "headlamp": true,
                            "stealth_boots": false
                        }
                    }
                }
            }
        }"#;

        let parsed: CampaignSave =
            serde_json::from_str(legacy_json).expect("legacy save JSON must deserialize cleanly");
        assert_eq!(parsed.credits, 240);
        assert_eq!(parsed.unlocked_depth_tier, 2);
        assert!(!parsed.winch_overdrive);
        assert!(!parsed.fast_charger);
        let miner = parsed.players.get("miner").expect("miner profile");
        assert!(miner.inventory.passive.headlamp);
        assert!(!miner.inventory.passive.reinforced_pickaxe);
    }
}
