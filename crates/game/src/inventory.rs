//! Player inventory, Subnautica-style rechargeable battery pouch, passive wearables,
//! two-handed heavy carry holstering, and Base Shed shop catalog.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub const INITIAL_BELT_CAPACITY: usize = 2;
pub const MAX_BELT_CAPACITY: usize = 5;
pub const INITIAL_POUCH_CAPACITY: usize = 2;
pub const MAX_POUCH_CAPACITY: usize = 6;

pub const MAX_FLARES_STACK: u8 = 4;
pub const MAX_FOAM_STACK: u8 = 8;
pub const MAX_DECOYS_STACK: u8 = 2;

pub const STEALTH_BOOTS_NOISE_MULT: f32 = 0.55;
pub const FULL_BELT_NOISE_MULT: f32 = 1.12;

/// A physical rechargeable battery cell tracked individually (Subnautica-style).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BatteryCellItem {
    /// Normalized remaining charge in `[0.0, 1.0]`.
    pub charge: f32,
    /// Relative capacity multiplier (`1.0` = standard cell, `> 1.0` = high-capacity tier).
    pub capacity_scale: f32,
}

impl BatteryCellItem {
    /// Creates a standard full (`100%`) battery cell.
    pub fn full() -> Self {
        Self {
            charge: 1.0,
            capacity_scale: 1.0,
        }
    }

    /// Creates a standard battery cell with the given normalized charge clamped to `[0.0, 1.0]`.
    pub fn with_charge(charge: f32) -> Self {
        Self {
            charge: charge.clamp(0.0, 1.0),
            capacity_scale: 1.0,
        }
    }

    /// Returns remaining charge as a rounded integer percentage in `0..=100`.
    pub fn charge_pct(&self) -> u8 {
        (self.charge.clamp(0.0, 1.0) * 100.0).round() as u8
    }

    /// Returns `true` when the cell is effectively depleted.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.charge <= 1e-4
    }

    /// Increases charge by `delta` (clamped at `1.0`). Returns `true` if charge actually increased.
    pub fn recharge(&mut self, delta: f32) -> bool {
        if delta <= 0.0 || self.charge >= 1.0 {
            return false;
        }
        let prev = self.charge;
        self.charge = (self.charge + delta).clamp(0.0, 1.0);
        self.charge > prev + 1e-6
    }
}

/// Items that occupy an active belt / hotbar slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BeltItem {
    /// Handheld flashlight powered by `PlayerInventory::loaded_battery`.
    Flashlight,
    /// Throwable glow flares (max stack: 4).
    Flares { count: u8 },
    /// Acoustic foam voxel canister (max stack: 8 charges).
    AcousticFoam { charges: u8 },
    /// Throwable acoustic noise decoys (max stack: 2).
    NoiseDecoys { count: u8 },
    /// Single-use Aid Kit for reviving downed teammates (max stack: 1).
    AidKit,
    /// Small side-loot treasure carried in a belt slot for extra credits (max stack: 1).
    BonusLoot { value: u32 },
}

impl BeltItem {
    /// Concise display label for HUD hotbar & interaction prompts.
    #[allow(dead_code)]
    pub fn display_label(&self) -> String {
        match self {
            BeltItem::Flashlight => "Flashlight".to_string(),
            BeltItem::Flares { count } => format!("Flares x{count}"),
            BeltItem::AcousticFoam { charges } => format!("Foam ({charges})"),
            BeltItem::NoiseDecoys { count } => format!("Decoys x{count}"),
            BeltItem::AidKit => "Aid Kit".to_string(),
            BeltItem::BonusLoot { value } => format!("Loot ({value}cr)"),
        }
    }
}

/// Permanent Shed wearables that do not consume active belt slots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassiveGear {
    /// Mounted headgear light: frees a belt slot and remains usable during 2-handed carry.
    pub headlamp: bool,
    /// Soft-sole footwear: reduces footstep acoustic energy by 45%.
    pub stealth_boots: bool,
}

/// Tracks whether both hands are occupied by a heavy carry or teammate drag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CarryHandState {
    #[default]
    Free,
    HeavyRelic {
        item_id: u32,
    },
    DraggingPlayer {
        target_id: u16,
    },
}

/// UI state for the Subnautica-style `[R]` scroll-wheel battery swapper.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatterySelectorState {
    pub open: bool,
    pub selected_idx: usize,
}

/// Complete per-player loadout: active belt, rechargeable battery pouch, passive gear,
/// and transient carry/selector states.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerInventory {
    pub belt_capacity: usize,
    pub belt: Vec<Option<BeltItem>>,
    pub active_slot: usize,
    pub battery_pouch_capacity: usize,
    pub battery_pouch: Vec<BatteryCellItem>,
    pub loaded_battery: Option<BatteryCellItem>,
    pub passive: PassiveGear,
    #[serde(skip)]
    pub carry_state: CarryHandState,
    #[serde(skip)]
    pub battery_selector: BatterySelectorState,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        Self::starter()
    }
}

impl PlayerInventory {
    /// Standard starting loadout for a new miner.
    pub fn starter() -> Self {
        Self {
            belt_capacity: INITIAL_BELT_CAPACITY,
            belt: vec![
                Some(BeltItem::Flashlight),
                Some(BeltItem::Flares { count: 3 }),
            ],
            active_slot: 0,
            battery_pouch_capacity: INITIAL_POUCH_CAPACITY,
            battery_pouch: vec![BatteryCellItem::full(), BatteryCellItem::full()],
            loaded_battery: Some(BatteryCellItem::full()),
            passive: PassiveGear::default(),
            carry_state: CarryHandState::Free,
            battery_selector: BatterySelectorState::default(),
        }
    }

    /// Ensures `belt` matches `belt_capacity` and `active_slot` stays in bounds.
    pub(crate) fn normalize_slots(&mut self) {
        self.belt_capacity = self.belt_capacity.clamp(1, MAX_BELT_CAPACITY);
        self.battery_pouch_capacity = self.battery_pouch_capacity.clamp(1, MAX_POUCH_CAPACITY);
        if self.belt.len() < self.belt_capacity {
            self.belt.resize(self.belt_capacity, None);
        } else if self.belt.len() > self.belt_capacity {
            self.belt.truncate(self.belt_capacity);
        }
        self.active_slot = self.active_slot.min(self.belt_capacity - 1);
        if self.battery_pouch.is_empty() {
            self.battery_selector.selected_idx = 0;
        } else {
            self.battery_selector.selected_idx = self
                .battery_selector
                .selected_idx
                .min(self.battery_pouch.len() - 1);
        }
    }

    /// Selects an active belt slot if `slot` is within the current belt capacity.
    pub fn select_slot(&mut self, slot: usize) {
        self.normalize_slots();
        if slot < self.belt_capacity {
            self.active_slot = slot;
        }
    }

    /// Scrolls the active belt slot by `delta` (wrapping cleanly across unlocked slots `0..belt_capacity`).
    pub fn scroll_active_slot(&mut self, delta: i32) {
        self.normalize_slots();
        if self.belt_capacity <= 1 || delta == 0 {
            return;
        }
        let cap = self.belt_capacity as i32;
        self.active_slot = (self.active_slot as i32 + delta).rem_euclid(cap) as usize;
    }

    /// Returns `true` when carrying a 2-handed heavy relic or dragging a downed teammate.
    pub fn is_hands_occupied(&self) -> bool {
        self.carry_state != CarryHandState::Free
    }

    /// Returns the currently selected belt item, or `None` when hands are occupied
    /// (2-handed carry holsters active belt items) or the slot is empty.
    #[allow(dead_code)]
    pub fn active_item(&self) -> Option<&BeltItem> {
        if self.is_hands_occupied() {
            return None;
        }
        self.belt
            .get(self.active_slot)
            .and_then(|slot| slot.as_ref())
    }

    /// Returns `true` if any belt slot contains a handheld `Flashlight`.
    pub fn has_flashlight_in_belt(&self) -> bool {
        self.belt
            .iter()
            .any(|slot| matches!(slot, Some(BeltItem::Flashlight)))
    }

    /// Returns `true` if the player can operate their personal light:
    /// - Always `true` with a passive Headlamp (even during 2-handed carry).
    /// - Otherwise requires free hands and a `Flashlight` in the belt.
    pub fn can_use_light(&self) -> bool {
        if self.passive.headlamp {
            true
        } else {
            !self.is_hands_occupied() && self.has_flashlight_in_belt()
        }
    }

    /// Returns the normalized charge `[0.0, 1.0]` of the loaded battery cell (`0.0` if none).
    pub fn loaded_battery_charge(&self) -> f32 {
        self.loaded_battery
            .map_or(0.0, |cell| cell.charge.clamp(0.0, 1.0))
    }

    /// Drains the loaded battery over `dt_s` seconds, scaled by `capacity_scale`.
    /// Returns the remaining normalized charge.
    pub fn drain_loaded_battery(&mut self, dt_s: f32, full_drain_seconds: f32) -> f32 {
        if dt_s <= 0.0 || full_drain_seconds <= 0.0 {
            return self.loaded_battery_charge();
        }
        if let Some(ref mut cell) = self.loaded_battery {
            let cap_scale = cell.capacity_scale.max(0.05);
            let drain = dt_s / (full_drain_seconds * cap_scale);
            cell.charge = (cell.charge - drain).clamp(0.0, 1.0);
            cell.charge
        } else {
            0.0
        }
    }

    /// Opens the `[R]` Subnautica-style battery pouch selector and clamps the cursor.
    pub fn open_battery_selector(&mut self) {
        self.battery_selector.open = true;
        if self.battery_pouch.is_empty() {
            self.battery_selector.selected_idx = 0;
        } else {
            self.battery_selector.selected_idx = self
                .battery_selector
                .selected_idx
                .min(self.battery_pouch.len() - 1);
        }
    }

    /// Closes the battery pouch selector overlay.
    pub fn close_battery_selector(&mut self) {
        self.battery_selector.open = false;
    }

    /// Moves the battery pouch selection cursor by `delta` with clean wrap-around.
    pub fn scroll_battery_selector(&mut self, delta: i32) {
        if self.battery_pouch.is_empty() {
            self.battery_selector.selected_idx = 0;
            return;
        }
        let len = self.battery_pouch.len() as i32;
        let current = self
            .battery_selector
            .selected_idx
            .min(self.battery_pouch.len() - 1) as i32;
        self.battery_selector.selected_idx = (current + delta).rem_euclid(len) as usize;
    }

    /// Swaps the highlighted battery cell from `battery_pouch` into `loaded_battery`.
    /// If a previous `loaded_battery` existed, it is returned to the pouch so drained cells
    /// can be recharged back in the Base Shed. Returns the new loaded charge if swapped.
    pub fn swap_selected_battery(&mut self) -> Option<f32> {
        if self.battery_pouch.is_empty() {
            self.battery_selector.selected_idx = 0;
            return None;
        }
        let idx = self
            .battery_selector
            .selected_idx
            .min(self.battery_pouch.len() - 1);
        let new_cell = self.battery_pouch.remove(idx);
        if let Some(old_cell) = self.loaded_battery.replace(new_cell) {
            self.battery_pouch.insert(idx, old_cell);
            self.battery_selector.selected_idx = idx;
        } else if self.battery_pouch.is_empty() {
            self.battery_selector.selected_idx = 0;
        } else {
            self.battery_selector.selected_idx = idx.min(self.battery_pouch.len() - 1);
        }
        Some(new_cell.charge)
    }

    /// Removes and returns the currently highlighted battery from the pouch so it can be
    /// physically dropped (`Q`) and shared with a teammate.
    pub fn drop_selected_pouch_battery(&mut self) -> Option<BatteryCellItem> {
        if self.battery_pouch.is_empty() {
            self.battery_selector.selected_idx = 0;
            return None;
        }
        let idx = self
            .battery_selector
            .selected_idx
            .min(self.battery_pouch.len() - 1);
        let dropped = self.battery_pouch.remove(idx);
        if self.battery_pouch.is_empty() {
            self.battery_selector.selected_idx = 0;
        } else {
            self.battery_selector.selected_idx = idx.min(self.battery_pouch.len() - 1);
        }
        Some(dropped)
    }

    /// Attempts to add `item` to the belt, stacking consumables into existing matching slots
    /// up to their stack limit before allocating empty slots.
    ///
    /// Atomic: if the full item/stack cannot fit, the belt is left untouched and `Err(item)` is returned.
    pub fn try_add_belt_item(&mut self, item: BeltItem) -> Result<usize, BeltItem> {
        self.normalize_slots();

        match item {
            BeltItem::Flares { count } => {
                if count == 0 {
                    return Err(BeltItem::Flares { count });
                }
                let room = self.stackable_room(MAX_FLARES_STACK, |slot| match slot {
                    BeltItem::Flares { count } => Some(*count),
                    _ => None,
                });
                if room < usize::from(count) {
                    return Err(BeltItem::Flares { count });
                }
                let mut remaining = count;
                let mut last_idx = 0;
                for (idx, slot) in self.belt.iter_mut().enumerate() {
                    if remaining == 0 {
                        break;
                    }
                    if let Some(BeltItem::Flares { count: existing }) = slot
                        && *existing < MAX_FLARES_STACK
                    {
                        let add = remaining.min(MAX_FLARES_STACK - *existing);
                        *existing += add;
                        remaining -= add;
                        last_idx = idx;
                    }
                }
                for (idx, slot) in self.belt.iter_mut().enumerate() {
                    if remaining == 0 {
                        break;
                    }
                    if slot.is_none() {
                        let add = remaining.min(MAX_FLARES_STACK);
                        *slot = Some(BeltItem::Flares { count: add });
                        remaining -= add;
                        last_idx = idx;
                    }
                }
                Ok(last_idx)
            }
            BeltItem::AcousticFoam { charges } => {
                if charges == 0 {
                    return Err(BeltItem::AcousticFoam { charges });
                }
                let room = self.stackable_room(MAX_FOAM_STACK, |slot| match slot {
                    BeltItem::AcousticFoam { charges } => Some(*charges),
                    _ => None,
                });
                if room < usize::from(charges) {
                    return Err(BeltItem::AcousticFoam { charges });
                }
                let mut remaining = charges;
                let mut last_idx = 0;
                for (idx, slot) in self.belt.iter_mut().enumerate() {
                    if remaining == 0 {
                        break;
                    }
                    if let Some(BeltItem::AcousticFoam { charges: existing }) = slot
                        && *existing < MAX_FOAM_STACK
                    {
                        let add = remaining.min(MAX_FOAM_STACK - *existing);
                        *existing += add;
                        remaining -= add;
                        last_idx = idx;
                    }
                }
                for (idx, slot) in self.belt.iter_mut().enumerate() {
                    if remaining == 0 {
                        break;
                    }
                    if slot.is_none() {
                        let add = remaining.min(MAX_FOAM_STACK);
                        *slot = Some(BeltItem::AcousticFoam { charges: add });
                        remaining -= add;
                        last_idx = idx;
                    }
                }
                Ok(last_idx)
            }
            BeltItem::NoiseDecoys { count } => {
                if count == 0 {
                    return Err(BeltItem::NoiseDecoys { count });
                }
                let room = self.stackable_room(MAX_DECOYS_STACK, |slot| match slot {
                    BeltItem::NoiseDecoys { count } => Some(*count),
                    _ => None,
                });
                if room < usize::from(count) {
                    return Err(BeltItem::NoiseDecoys { count });
                }
                let mut remaining = count;
                let mut last_idx = 0;
                for (idx, slot) in self.belt.iter_mut().enumerate() {
                    if remaining == 0 {
                        break;
                    }
                    if let Some(BeltItem::NoiseDecoys { count: existing }) = slot
                        && *existing < MAX_DECOYS_STACK
                    {
                        let add = remaining.min(MAX_DECOYS_STACK - *existing);
                        *existing += add;
                        remaining -= add;
                        last_idx = idx;
                    }
                }
                for (idx, slot) in self.belt.iter_mut().enumerate() {
                    if remaining == 0 {
                        break;
                    }
                    if slot.is_none() {
                        let add = remaining.min(MAX_DECOYS_STACK);
                        *slot = Some(BeltItem::NoiseDecoys { count: add });
                        remaining -= add;
                        last_idx = idx;
                    }
                }
                Ok(last_idx)
            }
            single => {
                if let Some((idx, slot)) = self
                    .belt
                    .iter_mut()
                    .enumerate()
                    .find(|(_, slot)| slot.is_none())
                {
                    *slot = Some(single);
                    Ok(idx)
                } else {
                    Err(single)
                }
            }
        }
    }

    fn stackable_room<F>(&self, max_stack: u8, extract: F) -> usize
    where
        F: Fn(&BeltItem) -> Option<u8>,
    {
        let mut room = 0usize;
        for slot in &self.belt {
            match slot {
                None => room += usize::from(max_stack),
                Some(item) => {
                    if let Some(cur) = extract(item) {
                        room += usize::from(max_stack.saturating_sub(cur));
                    }
                }
            }
        }
        room
    }

    /// Adds a battery cell to the dedicated battery pouch if capacity permits.
    pub fn try_add_battery(&mut self, cell: BatteryCellItem) -> Result<usize, BatteryCellItem> {
        if self.battery_pouch.len() >= self.battery_pouch_capacity {
            return Err(cell);
        }
        let idx = self.battery_pouch.len();
        self.battery_pouch.push(cell);
        Ok(idx)
    }

    /// Removes and returns the currently active belt item.
    /// Returns `None` if both hands are occupied or the active slot is empty.
    pub fn drop_active_belt_item(&mut self) -> Option<BeltItem> {
        if self.is_hands_occupied() {
            return None;
        }
        self.belt.get_mut(self.active_slot)?.take()
    }

    /// Consumes 1 flare from the active slot first, or any belt slot containing flares.
    pub fn consume_flare(&mut self) -> bool {
        self.consume_matching_slot(|slot| match slot {
            Some(BeltItem::Flares { count }) if *count > 0 => {
                *count -= 1;
                if *count == 0 {
                    *slot = None;
                }
                true
            }
            _ => false,
        })
    }

    /// Consumes 1 acoustic foam charge from the active slot first, or any matching belt slot.
    pub fn consume_foam(&mut self) -> bool {
        self.consume_matching_slot(|slot| match slot {
            Some(BeltItem::AcousticFoam { charges }) if *charges > 0 => {
                *charges -= 1;
                if *charges == 0 {
                    *slot = None;
                }
                true
            }
            _ => false,
        })
    }

    /// Consumes 1 noise decoy from the active slot first, or any matching belt slot.
    pub fn consume_decoy(&mut self) -> bool {
        self.consume_matching_slot(|slot| match slot {
            Some(BeltItem::NoiseDecoys { count }) if *count > 0 => {
                *count -= 1;
                if *count == 0 {
                    *slot = None;
                }
                true
            }
            _ => false,
        })
    }

    /// Consumes 1 Aid Kit from the active slot first, or any belt slot holding an Aid Kit.
    pub fn consume_aid_kit(&mut self) -> bool {
        self.consume_matching_slot(|slot| {
            if matches!(slot, Some(BeltItem::AidKit)) {
                *slot = None;
                true
            } else {
                false
            }
        })
    }

    fn consume_matching_slot<F>(&mut self, mut try_consume: F) -> bool
    where
        F: FnMut(&mut Option<BeltItem>) -> bool,
    {
        if let Some(active) = self.belt.get_mut(self.active_slot)
            && try_consume(active)
        {
            return true;
        }
        for (idx, slot) in self.belt.iter_mut().enumerate() {
            if idx == self.active_slot {
                continue;
            }
            if try_consume(slot) {
                return true;
            }
        }
        false
    }

    /// Cashes in and removes all `BonusLoot` items held in the belt, returning total credit value.
    pub fn take_all_bonus_loot_value(&mut self) -> u32 {
        let mut total = 0u32;
        for slot in &mut self.belt {
            if let Some(BeltItem::BonusLoot { value }) = slot {
                total = total.saturating_add(*value);
                *slot = None;
            }
        }
        total
    }

    /// Recharges both the loaded battery and all pouch batteries by `amount`.
    /// Returns `true` if at least one cell gained charge.
    pub fn recharge_all_batteries(&mut self, amount: f32) -> bool {
        let mut any_recharged = false;
        if let Some(ref mut cell) = self.loaded_battery {
            any_recharged |= cell.recharge(amount);
        }
        for cell in &mut self.battery_pouch {
            any_recharged |= cell.recharge(amount);
        }
        any_recharged
    }

    /// Increases belt capacity by `+1` up to `MAX_BELT_CAPACITY` (`5`), appending an empty slot.
    pub fn upgrade_belt(&mut self) -> bool {
        self.normalize_slots();
        if self.belt_capacity >= MAX_BELT_CAPACITY {
            return false;
        }
        self.belt_capacity += 1;
        self.belt.push(None);
        true
    }

    /// Increases battery pouch capacity by `+2` up to `MAX_POUCH_CAPACITY` (`6`).
    pub fn upgrade_pouch(&mut self) -> bool {
        if self.battery_pouch_capacity >= MAX_POUCH_CAPACITY {
            return false;
        }
        self.battery_pouch_capacity = (self.battery_pouch_capacity + 2).min(MAX_POUCH_CAPACITY);
        true
    }

    /// Computes footstep acoustic noise multiplier:
    /// - `0.55` with Stealth Boots (else `1.0`)
    /// - Multiplied by `1.12` when every belt slot is filled (heavily stuffed pack).
    pub fn footstep_noise_multiplier(&self) -> f32 {
        let base = if self.passive.stealth_boots {
            STEALTH_BOOTS_NOISE_MULT
        } else {
            1.0
        };
        let all_filled = self.belt_capacity > 0
            && self.belt.len() >= self.belt_capacity
            && self
                .belt
                .iter()
                .take(self.belt_capacity)
                .all(Option::is_some);
        if all_filled {
            base * FULL_BELT_NOISE_MULT
        } else {
            base
        }
    }

    /// Applies the Team Wipe penalty on forced Shed return:
    /// - Drops all consumables, bonus loot, and spare cave batteries.
    /// - Keeps passive wearables, belt capacity, and battery pouch capacity.
    /// - Ensures the player has a `Flashlight` if they do not own a `Headlamp`, plus
    ///   1 full loaded battery and 1 full pouch battery so they are never soft-locked.
    pub fn apply_team_wipe_penalty(&mut self) {
        self.normalize_slots();
        self.carry_state = CarryHandState::Free;
        self.battery_selector.open = false;

        for slot in &mut self.belt {
            if !matches!(slot, Some(BeltItem::Flashlight)) {
                *slot = None;
            }
        }

        if !self.passive.headlamp && !self.has_flashlight_in_belt() {
            if let Some(empty) = self.belt.iter_mut().find(|slot| slot.is_none()) {
                *empty = Some(BeltItem::Flashlight);
            } else if let Some(first) = self.belt.first_mut() {
                *first = Some(BeltItem::Flashlight);
            }
        }

        self.loaded_battery = Some(BatteryCellItem::full());
        self.battery_pouch.clear();
        if self.battery_pouch_capacity > 0 {
            self.battery_pouch.push(BatteryCellItem::full());
        }
        self.battery_selector.selected_idx = 0;
        self.active_slot = self.active_slot.min(self.belt_capacity.saturating_sub(1));
    }

    /// Formats the active belt bar & passive gear status for the HUD.
    #[allow(dead_code)]
    pub fn format_belt_hud(&self) -> String {
        let mut parts = Vec::with_capacity(self.belt_capacity);
        let occupied = self.is_hands_occupied();

        for idx in 0..self.belt_capacity {
            let label = self
                .belt
                .get(idx)
                .and_then(|s| s.as_ref())
                .map_or_else(|| "Empty".to_string(), BeltItem::display_label);
            let slot_num = idx + 1;
            if !occupied && idx == self.active_slot {
                parts.push(format!(">[{slot_num}] {label}<"));
            } else {
                parts.push(format!("[{slot_num}] {label}"));
            }
        }

        let mut line = parts.join("  ");

        if occupied {
            let carry_tag = match self.carry_state {
                CarryHandState::HeavyRelic { .. } => "2H RELIC (Belt Holstered)",
                CarryHandState::DraggingPlayer { .. } => "DRAGGING TEAMMATE (Belt Holstered)",
                CarryHandState::Free => "",
            };
            line = format!("[{carry_tag}]  {line}");
        }

        let mut passives = Vec::new();
        if self.passive.headlamp {
            passives.push("Headlamp");
        }
        if self.passive.stealth_boots {
            passives.push("Stealth Boots");
        }
        if !passives.is_empty() {
            line.push_str(&format!("  | Gear: {}", passives.join(", ")));
        }

        let bat_str = match self.loaded_battery {
            Some(cell) => format!("{}%", cell.charge_pct()),
            None => "None".to_string(),
        };
        line.push_str(&format!(
            "  | Bat: {bat_str} (Pouch {}/{})",
            self.battery_pouch.len(),
            self.battery_pouch_capacity
        ));

        line
    }

    /// Formats the Subnautica-style `[R]` battery pouch selector HUD overlay.
    #[allow(dead_code)]
    pub fn format_battery_selector_hud(&self) -> String {
        let loaded = match self.loaded_battery {
            Some(cell) => format!("{}%", cell.charge_pct()),
            None => "None".to_string(),
        };

        if self.battery_pouch.is_empty() {
            return format!("BATTERY POUCH [Loaded: {loaded}] — (Empty)  |  [R] Close");
        }

        let selected = self
            .battery_selector
            .selected_idx
            .min(self.battery_pouch.len() - 1);
        let cells: Vec<String> = self
            .battery_pouch
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let pct = cell.charge_pct();
                if i == selected {
                    format!(">[{}] {}%<", i + 1, pct)
                } else {
                    format!("[{}] {}%", i + 1, pct)
                }
            })
            .collect();

        format!(
            "BATTERY POUCH [Loaded: {loaded}]  {}  |  [Scroll] Select  [R] Swap  [Q] Drop",
            cells.join("  ")
        )
    }
}

/// Items and permanent upgrades purchasable from the Base Shed Shop Terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShopCatalogItem {
    BatteryCell,
    FlarePack,
    FoamCanister,
    NoiseDecoys,
    AidKit,
    Headlamp,
    StealthBoots,
    ToolBeltUpgrade,
    BatteryPouchUpgrade,
}

/// Static catalog metadata for a Base Shed shop entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShopItemInfo {
    pub item: ShopCatalogItem,
    pub name: &'static str,
    pub cost: u32,
    pub description: &'static str,
}

/// Balanced Base Shed shop catalog.
pub const SHOP_CATALOG: [ShopItemInfo; 9] = [
    ShopItemInfo {
        item: ShopCatalogItem::BatteryCell,
        name: "Battery Cell",
        cost: 35,
        description: "Full 100% rechargeable battery cell for your pouch.",
    },
    ShopItemInfo {
        item: ShopCatalogItem::FlarePack,
        name: "Flare Pack (x3)",
        cost: 40,
        description: "Pack of 3 throwable glow flares (stacks up to 4).",
    },
    ShopItemInfo {
        item: ShopCatalogItem::FoamCanister,
        name: "Acoustic Foam (8)",
        cost: 50,
        description: "8 acoustic foam charges to seal tunnels & deaden echoes.",
    },
    ShopItemInfo {
        item: ShopCatalogItem::NoiseDecoys,
        name: "Noise Decoys (x2)",
        cost: 55,
        description: "2 throwable clicker decoys to lure blind predators away.",
    },
    ShopItemInfo {
        item: ShopCatalogItem::AidKit,
        name: "Aid Kit",
        cost: 75,
        description: "Consumable medical kit to revive a downed teammate.",
    },
    ShopItemInfo {
        item: ShopCatalogItem::Headlamp,
        name: "Headlamp",
        cost: 180,
        description: "Passive headgear light — frees a belt slot & works during 2H carry.",
    },
    ShopItemInfo {
        item: ShopCatalogItem::StealthBoots,
        name: "Stealth Boots",
        cost: 160,
        description: "Passive soft-sole boots — reduces footstep noise by 45%.",
    },
    ShopItemInfo {
        item: ShopCatalogItem::ToolBeltUpgrade,
        name: "Tool Belt (+1)",
        cost: 140,
        description: "Permanently adds +1 active belt slot (max 5).",
    },
    ShopItemInfo {
        item: ShopCatalogItem::BatteryPouchUpgrade,
        name: "Battery Pouch (+2)",
        cost: 120,
        description: "Permanently adds +2 battery pouch slots (max 6).",
    },
];

/// Looks up the catalog metadata for a given `ShopCatalogItem`.
pub fn shop_item_info(item: ShopCatalogItem) -> &'static ShopItemInfo {
    SHOP_CATALOG
        .iter()
        .find(|entry| entry.item == item)
        .expect("all ShopCatalogItem variants exist in SHOP_CATALOG")
}

/// Attempts to buy `item` from the Shed shop, deducting `credits` and updating `inv`.
/// Returns a user-facing status message on success or failure without partial state changes.
pub fn purchase_shop_item(
    inv: &mut PlayerInventory,
    credits: &mut u32,
    item: ShopCatalogItem,
) -> Result<&'static str, &'static str> {
    let info = shop_item_info(item);

    // Check permanent upgrade caps/ownership before checking credits for clear feedback.
    match item {
        ShopCatalogItem::Headlamp if inv.passive.headlamp => {
            return Err("Headlamp already owned.");
        }
        ShopCatalogItem::StealthBoots if inv.passive.stealth_boots => {
            return Err("Stealth Boots already owned.");
        }
        ShopCatalogItem::ToolBeltUpgrade if inv.belt_capacity >= MAX_BELT_CAPACITY => {
            return Err("Tool Belt is already at maximum capacity (5).");
        }
        ShopCatalogItem::BatteryPouchUpgrade
            if inv.battery_pouch_capacity >= MAX_POUCH_CAPACITY =>
        {
            return Err("Battery Pouch is already at maximum capacity (6).");
        }
        _ => {}
    }

    if *credits < info.cost {
        return Err("Not enough credits.");
    }

    let ok_msg = match item {
        ShopCatalogItem::BatteryCell => {
            inv.try_add_battery(BatteryCellItem::full())
                .map_err(|_| "Battery pouch is full.")?;
            "Purchased Battery Cell."
        }
        ShopCatalogItem::FlarePack => {
            inv.try_add_belt_item(BeltItem::Flares { count: 3 })
                .map_err(|_| "Belt has no room for Flare Pack (x3).")?;
            "Purchased Flare Pack (x3)."
        }
        ShopCatalogItem::FoamCanister => {
            inv.try_add_belt_item(BeltItem::AcousticFoam { charges: 8 })
                .map_err(|_| "Belt has no room for Acoustic Foam canister.")?;
            "Purchased Acoustic Foam (8 charges)."
        }
        ShopCatalogItem::NoiseDecoys => {
            inv.try_add_belt_item(BeltItem::NoiseDecoys { count: 2 })
                .map_err(|_| "Belt has no room for Noise Decoys (x2).")?;
            "Purchased Noise Decoys (x2)."
        }
        ShopCatalogItem::AidKit => {
            inv.try_add_belt_item(BeltItem::AidKit)
                .map_err(|_| "Belt has no room for Aid Kit.")?;
            "Purchased Aid Kit."
        }
        ShopCatalogItem::Headlamp => {
            inv.passive.headlamp = true;
            "Equipped Headlamp! Belt light slot is now optional."
        }
        ShopCatalogItem::StealthBoots => {
            inv.passive.stealth_boots = true;
            "Equipped Stealth Boots (-45% footstep noise)."
        }
        ShopCatalogItem::ToolBeltUpgrade => {
            if !inv.upgrade_belt() {
                return Err("Tool Belt is already at maximum capacity (5).");
            }
            "Upgraded Tool Belt (+1 slot)."
        }
        ShopCatalogItem::BatteryPouchUpgrade => {
            if !inv.upgrade_pouch() {
                return Err("Battery Pouch is already at maximum capacity (6).");
            }
            "Upgraded Battery Pouch (+2 slots)."
        }
    };

    *credits -= info.cost;
    Ok(ok_msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_inventory_matches_design_defaults() {
        let inv = PlayerInventory::starter();
        assert_eq!(inv.belt_capacity, 2);
        assert_eq!(inv.belt.len(), 2);
        assert_eq!(inv.active_slot, 0);
        assert_eq!(inv.active_item(), Some(&BeltItem::Flashlight));
        assert_eq!(inv.battery_pouch_capacity, 2);
        assert_eq!(inv.battery_pouch.len(), 2);
        assert_eq!(inv.loaded_battery_charge(), 1.0);
        assert!(!inv.passive.headlamp);
        assert!(!inv.passive.stealth_boots);
        assert!(inv.can_use_light());
        assert!(!inv.is_hands_occupied());
    }

    #[test]
    fn heavy_carry_holsters_belt_and_requires_headlamp_for_light() {
        let mut inv = PlayerInventory::starter();
        inv.carry_state = CarryHandState::HeavyRelic { item_id: 7 };

        assert!(inv.is_hands_occupied());
        assert_eq!(inv.active_item(), None);
        assert_eq!(inv.drop_active_belt_item(), None);
        assert!(!inv.can_use_light());

        // Equipping Headlamp restores light even while carrying a 2-handed relic.
        inv.passive.headlamp = true;
        assert!(inv.can_use_light());
        assert_eq!(inv.active_item(), None);
    }

    #[test]
    fn battery_drain_swap_drop_and_recharge_roundtrip() {
        let mut inv = PlayerInventory::starter();
        inv.battery_pouch = vec![
            BatteryCellItem::with_charge(0.85),
            BatteryCellItem::with_charge(0.30),
        ];

        // Drain loaded cell halfway.
        let rem = inv.drain_loaded_battery(50.0, 100.0);
        assert!((rem - 0.50).abs() < 1e-5);
        assert_eq!(inv.loaded_battery.unwrap().charge_pct(), 50);

        // Open selector, scroll backward with wrap-around -> selects index 1 (30%).
        inv.open_battery_selector();
        inv.scroll_battery_selector(-1);
        assert_eq!(inv.battery_selector.selected_idx, 1);

        // Swap: 30% cell becomes loaded, old 50% cell returns to pouch slot 1 for Shed recharge.
        let swapped = inv.swap_selected_battery().expect("swap should succeed");
        assert!((swapped - 0.30).abs() < 1e-5);
        assert_eq!(inv.loaded_battery.unwrap().charge_pct(), 30);
        assert_eq!(inv.battery_pouch.len(), 2);
        assert_eq!(inv.battery_pouch[1].charge_pct(), 50);

        // Drop highlighted battery (50%) for a friend.
        let dropped = inv.drop_selected_pouch_battery().expect("should drop cell");
        assert_eq!(dropped.charge_pct(), 50);
        assert_eq!(inv.battery_pouch.len(), 1);
        assert_eq!(inv.battery_selector.selected_idx, 0);

        // Recharge all remaining batteries back in the Shed.
        assert!(inv.recharge_all_batteries(1.0));
        assert_eq!(inv.loaded_battery.unwrap().charge_pct(), 100);
        assert_eq!(inv.battery_pouch[0].charge_pct(), 100);
        assert!(!inv.recharge_all_batteries(0.25));
    }

    #[test]
    fn battery_drain_respects_capacity_scale() {
        let mut inv = PlayerInventory::starter();
        inv.loaded_battery = Some(BatteryCellItem {
            charge: 1.0,
            capacity_scale: 2.0,
        });
        let rem = inv.drain_loaded_battery(50.0, 100.0);
        assert!((rem - 0.75).abs() < 1e-5);
    }

    #[test]
    fn belt_stacking_atomic_overflow_and_consumables() {
        let mut inv = PlayerInventory::starter();
        // Starter has [Flashlight, Flares(3)] and capacity 2.
        // Adding 1 flare stacks onto slot 1 -> Flares(4).
        let idx = inv
            .try_add_belt_item(BeltItem::Flares { count: 1 })
            .expect("1 flare should stack to max 4");
        assert_eq!(idx, 1);
        assert_eq!(inv.belt[1], Some(BeltItem::Flares { count: 4 }));

        // Adding another flare fails atomically when belt is full.
        assert!(inv.try_add_belt_item(BeltItem::Flares { count: 1 }).is_err());

        // Upgrade belt to 3, add foam, consume from active & non-active slots.
        assert!(inv.upgrade_belt());
        assert_eq!(inv.belt_capacity, 3);
        let foam_idx = inv
            .try_add_belt_item(BeltItem::AcousticFoam { charges: 2 })
            .expect("foam fits in new slot");
        assert_eq!(foam_idx, 2);

        // Active slot is 0 (Flashlight), consuming foam still finds slot 2.
        assert!(inv.consume_foam());
        assert_eq!(inv.belt[2], Some(BeltItem::AcousticFoam { charges: 1 }));
        assert!(inv.consume_foam());
        assert_eq!(inv.belt[2], None);
        assert!(!inv.consume_foam());

        // Decoys, AidKit, and BonusLoot cash-in.
        assert!(inv.try_add_belt_item(BeltItem::NoiseDecoys { count: 1 }).is_ok());
        assert!(inv.consume_decoy());
        assert!(!inv.consume_decoy());

        assert!(inv.try_add_belt_item(BeltItem::AidKit).is_ok());
        assert!(inv.consume_aid_kit());
        assert!(!inv.consume_aid_kit());

        assert!(inv.try_add_belt_item(BeltItem::BonusLoot { value: 65 }).is_ok());
        assert_eq!(inv.take_all_bonus_loot_value(), 65);
        assert_eq!(inv.take_all_bonus_loot_value(), 0);
    }

    #[test]
    fn footstep_noise_multiplier_reflects_stealth_boots_and_full_pack() {
        let mut inv = PlayerInventory::starter();
        // Both starter slots are filled -> 1.0 * 1.12
        assert!((inv.footstep_noise_multiplier() - 1.12).abs() < 1e-5);

        // Drop active item -> not full -> 1.0
        inv.drop_active_belt_item();
        assert!((inv.footstep_noise_multiplier() - 1.0).abs() < 1e-5);

        // Stealth boots + not full -> 0.55
        inv.passive.stealth_boots = true;
        assert!((inv.footstep_noise_multiplier() - 0.55).abs() < 1e-5);

        // Stealth boots + full -> 0.55 * 1.12
        inv.try_add_belt_item(BeltItem::AidKit).unwrap();
        assert!((inv.footstep_noise_multiplier() - (0.55 * 1.12)).abs() < 1e-5);
    }

    #[test]
    fn team_wipe_penalty_keeps_upgrades_and_prevents_softlock() {
        let mut inv = PlayerInventory::starter();
        assert!(inv.upgrade_belt());
        assert!(inv.upgrade_pouch());
        inv.passive.stealth_boots = true;
        inv.belt = vec![
            Some(BeltItem::BonusLoot { value: 90 }),
            Some(BeltItem::AidKit),
            Some(BeltItem::Flares { count: 4 }),
        ];
        inv.loaded_battery = Some(BatteryCellItem::with_charge(0.0));
        inv.battery_pouch.clear();
        inv.carry_state = CarryHandState::HeavyRelic { item_id: 99 };

        inv.apply_team_wipe_penalty();

        assert_eq!(inv.carry_state, CarryHandState::Free);
        assert_eq!(inv.belt_capacity, 3);
        assert_eq!(inv.battery_pouch_capacity, 4);
        assert!(inv.passive.stealth_boots);
        assert!(inv.has_flashlight_in_belt());
        assert_eq!(inv.belt.iter().flatten().count(), 1);
        assert_eq!(inv.loaded_battery, Some(BatteryCellItem::full()));
        assert_eq!(inv.battery_pouch, vec![BatteryCellItem::full()]);

        // With Headlamp owned, wipe does not force a handheld flashlight into the belt.
        inv.passive.headlamp = true;
        inv.belt = vec![Some(BeltItem::AidKit), None, None];
        inv.apply_team_wipe_penalty();
        assert!(!inv.has_flashlight_in_belt());
        assert!(inv.can_use_light());
    }

    #[test]
    fn shop_purchases_validate_credits_capacity_and_upgrades() {
        let mut inv = PlayerInventory::starter();
        let mut credits = 500;

        // Pouch is already 2/2 full -> buying BatteryCell fails without charging credits.
        assert!(purchase_shop_item(&mut inv, &mut credits, ShopCatalogItem::BatteryCell).is_err());
        assert_eq!(credits, 500);

        // Upgrade pouch (120cr) -> 4 slots, then buy BatteryCell (35cr).
        assert!(
            purchase_shop_item(&mut inv, &mut credits, ShopCatalogItem::BatteryPouchUpgrade)
                .is_ok()
        );
        assert_eq!(credits, 380);
        assert_eq!(inv.battery_pouch_capacity, 4);
        assert!(purchase_shop_item(&mut inv, &mut credits, ShopCatalogItem::BatteryCell).is_ok());
        assert_eq!(credits, 345);
        assert_eq!(inv.battery_pouch.len(), 3);

        // Buy Headlamp (180cr); second purchase rejected.
        assert!(purchase_shop_item(&mut inv, &mut credits, ShopCatalogItem::Headlamp).is_ok());
        assert_eq!(credits, 165);
        assert!(inv.passive.headlamp);
        assert!(purchase_shop_item(&mut inv, &mut credits, ShopCatalogItem::Headlamp).is_err());
        assert_eq!(credits, 165);

        // Buy StealthBoots (160cr) -> 5cr left; cannot afford FlarePack (40cr).
        assert!(purchase_shop_item(&mut inv, &mut credits, ShopCatalogItem::StealthBoots).is_ok());
        assert_eq!(credits, 5);
        assert!(purchase_shop_item(&mut inv, &mut credits, ShopCatalogItem::FlarePack).is_err());
        assert_eq!(credits, 5);
    }

    #[test]
    fn hud_formatters_produce_readable_output() {
        let mut inv = PlayerInventory::starter();
        let belt_hud = inv.format_belt_hud();
        assert!(belt_hud.contains(">[1] Flashlight<"));
        assert!(belt_hud.contains("[2] Flares x3"));
        assert!(belt_hud.contains("Bat: 100%"));

        inv.open_battery_selector();
        let selector_hud = inv.format_battery_selector_hud();
        assert!(selector_hud.contains("Loaded: 100%"));
        assert!(selector_hud.contains(">[1] 100%<"));
    }

    #[test]
    fn serde_roundtrip_skips_transient_states() {
        let mut inv = PlayerInventory::starter();
        inv.carry_state = CarryHandState::DraggingPlayer { target_id: 3 };
        inv.battery_selector = BatterySelectorState {
            open: true,
            selected_idx: 1,
        };
        inv.passive.headlamp = true;

        let json = serde_json::to_string(&inv).expect("serialize");
        let restored: PlayerInventory = serde_json::from_str(&json).expect("deserialize");
        assert!(restored.passive.headlamp);
        assert_eq!(restored.belt, inv.belt);
        assert_eq!(restored.battery_pouch, inv.battery_pouch);
        assert_eq!(restored.carry_state, CarryHandState::Free);
        assert_eq!(restored.battery_selector, BatterySelectorState::default());
    }

    #[test]
    fn scroll_active_slot_wraps_across_unlocked_belt_capacity() {
        let mut inv = PlayerInventory::starter(); // belt_capacity = 2, active_slot = 0
        inv.scroll_active_slot(1);
        assert_eq!(inv.active_slot, 1);
        inv.scroll_active_slot(1);
        assert_eq!(inv.active_slot, 0);
        inv.scroll_active_slot(-1);
        assert_eq!(inv.active_slot, 1);

        // Expand capacity to 4 and confirm wrap matches new capacity
        inv.belt_capacity = 4;
        inv.normalize_slots();
        inv.scroll_active_slot(2);
        assert_eq!(inv.active_slot, 3);
        inv.scroll_active_slot(1);
        assert_eq!(inv.active_slot, 0);
    }
}
