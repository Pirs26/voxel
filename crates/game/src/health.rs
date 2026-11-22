use std::collections::HashMap;

use bevy::prelude::*;

/// Duration in seconds before a downed player bleeds out into `Dead`.
pub const BLEEDOUT_DURATION_S: f32 = 60.0;
/// Duration in seconds required to channel a consumable Aid Kit revive.
pub const REVIVE_CHANNEL_S: f32 = 4.0;
/// Slow crawl speed (m/s) while downed.
pub const CRAWL_SPEED_M_S: f32 = 0.85;
/// Low camera eye height (m) while crawling on the cave floor.
pub const CRAWL_EYE_HEIGHT_M: f32 = 0.55;
/// Movement speed (m/s) while dragging a downed teammate.
pub const DRAG_SPEED_M_S: f32 = 1.65;
/// Maximum interaction range (m) for dragging or reviving a downed teammate.
pub const RESCUE_RANGE_M: f32 = 2.4;

/// Health lifecycle state of a player during a cave expedition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LocalHealthPhase {
    Healthy,
    Downed {
        bleedout_s: f32,
        dragged_by: Option<u16>,
    },
    Dead,
}

/// Authoritative local player health, bleedout, revive channel, and hit-feedback state.
#[derive(Resource, Clone, Debug)]
pub struct PlayerHealthState {
    pub phase: LocalHealthPhase,
    pub revive_progress_s: f32,
    pub reviving_target: Option<u16>,
    pub damage_flash_s: f32,
    pub hit_invuln_s: f32,
}

impl Default for PlayerHealthState {
    fn default() -> Self {
        Self {
            phase: LocalHealthPhase::Healthy,
            revive_progress_s: 0.0,
            reviving_target: None,
            damage_flash_s: 0.0,
            hit_invuln_s: 0.0,
        }
    }
}

impl PlayerHealthState {
    /// Applies a strike from a blind monster.
    ///
    /// If the player is `Healthy` and not inside the post-hit/post-revive invulnerability window,
    /// transitions to `Downed`, triggers the red damage flash (`1.0s`), grants `2.5s` of hit
    /// invulnerability, and returns `true`. Otherwise returns `false`.
    pub fn apply_monster_hit(&mut self) -> bool {
        if !matches!(self.phase, LocalHealthPhase::Healthy) || self.hit_invuln_s > 0.0 {
            return false;
        }
        self.phase = LocalHealthPhase::Downed {
            bleedout_s: BLEEDOUT_DURATION_S,
            dragged_by: None,
        };
        self.revive_progress_s = 0.0;
        self.reviving_target = None;
        self.damage_flash_s = 1.0;
        self.hit_invuln_s = 2.5;
        true
    }

    /// Restores the player to `Healthy` with brief post-revive invulnerability (`3.0s`).
    pub fn revive(&mut self) {
        self.phase = LocalHealthPhase::Healthy;
        self.hit_invuln_s = 3.0;
        self.revive_progress_s = 0.0;
        self.reviving_target = None;
    }

    /// Fully resets health and timers upon returning to the Base Shed.
    pub fn reset_for_shed(&mut self) {
        self.phase = LocalHealthPhase::Healthy;
        self.revive_progress_s = 0.0;
        self.reviving_target = None;
        self.damage_flash_s = 0.0;
        self.hit_invuln_s = 0.0;
    }

    /// Advances timers by `dt` seconds.
    ///
    /// Returns `true` on the exact tick a `Downed` player finishes bleeding out and transitions
    /// to `Dead`.
    pub fn tick(&mut self, dt: f32) -> bool {
        let dt = dt.max(0.0);
        self.damage_flash_s = (self.damage_flash_s - dt).max(0.0);
        self.hit_invuln_s = (self.hit_invuln_s - dt).max(0.0);

        if let LocalHealthPhase::Downed { bleedout_s, .. } = &mut self.phase {
            *bleedout_s = (*bleedout_s - dt).max(0.0);
            if *bleedout_s <= 0.0 {
                self.phase = LocalHealthPhase::Dead;
                self.revive_progress_s = 0.0;
                self.reviving_target = None;
                return true;
            }
        }
        false
    }

    #[inline]
    pub fn is_healthy(&self) -> bool {
        matches!(self.phase, LocalHealthPhase::Healthy)
    }

    #[inline]
    pub fn is_downed(&self) -> bool {
        matches!(self.phase, LocalHealthPhase::Downed { .. })
    }

    #[inline]
    pub fn is_dead(&self) -> bool {
        matches!(self.phase, LocalHealthPhase::Dead)
    }

    #[inline]
    pub fn is_alive(&self) -> bool {
        !self.is_dead()
    }

    #[inline]
    #[allow(dead_code)]
    pub fn bleedout_remaining_s(&self) -> Option<f32> {
        match self.phase {
            LocalHealthPhase::Downed { bleedout_s, .. } => Some(bleedout_s),
            _ => None,
        }
    }

    /// Formats a concise HUD status label for the current health/revive state.
    pub fn hud_label(&self) -> String {
        match self.phase {
            LocalHealthPhase::Healthy => {
                if self.reviving_target.is_some() && self.revive_progress_s > 0.0 {
                    let pct = (self.revive_progress_s / REVIVE_CHANNEL_S * 100.0).clamp(0.0, 100.0);
                    format!("REVIVING TEAMMATE ({pct:.0}%)")
                } else {
                    "HEALTHY".to_string()
                }
            }
            LocalHealthPhase::Downed {
                bleedout_s,
                dragged_by,
            } => {
                let secs = bleedout_s.ceil().max(0.0) as u32;
                if dragged_by.is_some() {
                    format!("DOWNED (BEING DRAGGED) — {secs}s")
                } else {
                    format!("DOWNED (CRAWL / WHISPER) — {secs}s")
                }
            }
            LocalHealthPhase::Dead => "DEAD".to_string(),
        }
    }
}

/// Synced health phases for all connected remote teammates keyed by network `PlayerId` (`u16`).
#[derive(Resource, Default, Clone, Debug)]
pub struct RemoteHealthMap(pub HashMap<u16, LocalHealthPhase>);

/// Emitted when a blind monster lands a strike on the local player.
#[derive(Message, Clone, Copy, Debug)]
pub struct MonsterHitLocalPlayerEvent {
    pub monster_pos_m: Vec3,
}

/// Emitted once every player on the expedition is `Dead`.
#[derive(Message, Clone, Copy, Debug)]
pub struct TeamWipeEvent;

/// Emitted when a 4s Aid Kit revive channel finishes on a downed teammate.
#[derive(Message, Clone, Copy, Debug)]
#[allow(dead_code)]
pub struct ReviveCompletedEvent {
    pub target_player_id: u16,
}

/// Returns `true` when the entire expedition party is wiped out.
///
/// In solo play (`remotes` empty), this is `true` as soon as the local player is `Dead`.
/// In co-op, this is `true` only when the local player AND every remote player are `Dead`.
pub fn check_team_wiped(
    local: &LocalHealthPhase,
    remotes: &HashMap<u16, LocalHealthPhase>,
) -> bool {
    matches!(local, LocalHealthPhase::Dead)
        && remotes
            .values()
            .all(|phase| matches!(phase, LocalHealthPhase::Dead))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_transitions_to_downed_and_respects_invuln_window() {
        let mut health = PlayerHealthState::default();
        assert!(health.is_healthy());
        assert!(health.is_alive());
        assert_eq!(health.bleedout_remaining_s(), None);

        // First hit downs the player and sets flash + invuln timers
        assert!(health.apply_monster_hit());
        assert!(health.is_downed());
        assert!(health.is_alive());
        assert_eq!(health.bleedout_remaining_s(), Some(BLEEDOUT_DURATION_S));
        assert!((health.damage_flash_s - 1.0).abs() < 1e-5);
        assert!((health.hit_invuln_s - 2.5).abs() < 1e-5);

        // Additional hits while downed are ignored
        assert!(!health.apply_monster_hit());

        // Reviving restores Healthy with 3.0s invulnerability
        health.revive();
        assert!(health.is_healthy());
        assert!((health.hit_invuln_s - 3.0).abs() < 1e-5);

        // Hits during post-revive invulnerability window are blocked
        assert!(!health.apply_monster_hit());
        assert!(!health.tick(2.0));
        assert!(!health.apply_monster_hit());

        // Once invulnerability expires, a hit downs the player again
        assert!(!health.tick(1.1));
        assert!(health.apply_monster_hit());
        assert!(health.is_downed());
    }

    #[test]
    fn bleedout_transitions_to_dead_and_reset_clears_state() {
        let mut health = PlayerHealthState::default();
        assert!(health.apply_monster_hit());

        assert!(!health.tick(30.0));
        assert!(health.is_downed());
        assert!((health.bleedout_remaining_s().unwrap() - 30.0).abs() < 1e-4);
        assert_eq!(health.damage_flash_s, 0.0);
        assert_eq!(health.hit_invuln_s, 0.0);

        // Final tick reaches zero and triggers transition to Dead exactly once
        assert!(health.tick(30.0));
        assert!(health.is_dead());
        assert!(!health.is_alive());
        assert_eq!(health.bleedout_remaining_s(), None);

        // Subsequent ticks do not re-trigger Dead transition
        assert!(!health.tick(5.0));
        assert!(health.is_dead());

        // Shed reset restores Healthy and clears timers
        health.damage_flash_s = 0.5;
        health.hit_invuln_s = 1.5;
        health.revive_progress_s = 2.0;
        health.reviving_target = Some(7);
        health.reset_for_shed();
        assert!(health.is_healthy());
        assert_eq!(health.damage_flash_s, 0.0);
        assert_eq!(health.hit_invuln_s, 0.0);
        assert_eq!(health.revive_progress_s, 0.0);
        assert_eq!(health.reviving_target, None);
    }

    #[test]
    fn check_team_wiped_handles_solo_and_coop() {
        let mut remotes = HashMap::new();

        // Solo mode: only Dead triggers wipe
        assert!(!check_team_wiped(&LocalHealthPhase::Healthy, &remotes));
        assert!(!check_team_wiped(
            &LocalHealthPhase::Downed {
                bleedout_s: 25.0,
                dragged_by: None,
            },
            &remotes,
        ));
        assert!(check_team_wiped(&LocalHealthPhase::Dead, &remotes));

        // Co-op mode: any non-Dead teammate prevents wipe
        remotes.insert(2, LocalHealthPhase::Healthy);
        assert!(!check_team_wiped(&LocalHealthPhase::Dead, &remotes));

        remotes.insert(
            2,
            LocalHealthPhase::Downed {
                bleedout_s: 12.0,
                dragged_by: Some(3),
            },
        );
        assert!(!check_team_wiped(&LocalHealthPhase::Dead, &remotes));

        remotes.insert(2, LocalHealthPhase::Dead);
        remotes.insert(3, LocalHealthPhase::Dead);
        assert!(!check_team_wiped(&LocalHealthPhase::Healthy, &remotes));
        assert!(!check_team_wiped(
            &LocalHealthPhase::Downed {
                bleedout_s: 5.0,
                dragged_by: None,
            },
            &remotes,
        ));
        assert!(check_team_wiped(&LocalHealthPhase::Dead, &remotes));
    }

    #[test]
    fn hud_label_formats_all_phases() {
        let mut health = PlayerHealthState::default();
        assert_eq!(health.hud_label(), "HEALTHY");

        health.reviving_target = Some(2);
        health.revive_progress_s = 2.0;
        assert!(health.hud_label().contains("50%"));

        health.reviving_target = None;
        health.revive_progress_s = 0.0;
        assert!(health.apply_monster_hit());
        assert!(health.hud_label().contains("DOWNED"));

        health.phase = LocalHealthPhase::Downed {
            bleedout_s: 18.2,
            dragged_by: Some(4),
        };
        assert!(health.hud_label().contains("DRAGGED"));

        health.phase = LocalHealthPhase::Dead;
        assert_eq!(health.hud_label(), "DEAD");
    }
}
