use glam::Vec3;
use serde::{Deserialize, Serialize};
use voxel_core::VoxelWorld;

use crate::{
    bands::Bands3,
    occlusion::compute_direct_occlusion,
    pathtracer::{PathTraceConfig, trace_early_reflections},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AcousticEventKind {
    Footstep,
    VoiceSpeech,
    ItemImpact,
    RelicScrape,
    WallBreach,
    DecoyNoise,
    ElevatorMachinery,
}

/// Physical sound event emitted into the world (shared between players and blind AI).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct AcousticEvent {
    pub pos_m: Vec3,
    /// Emitted acoustic energy per band (1.0 ≈ normal footstep; 4.0 ≈ loud shout/breach).
    pub energy: Bands3,
    /// 0.0 = continuous rumble, 1.0 = sharp percussive transient.
    pub sharpness: f32,
    pub kind: AcousticEventKind,
}

/// What a blind enemy actually perceives after voxel density muffling + 1-bounce corner check.
#[derive(Clone, Copy, Debug)]
pub struct PerceivedSound {
    /// Apparent direction/origin (actual source if direct, or nearest corner reflection if occluded).
    pub perceived_pos_m: Vec3,
    /// Received energy per frequency band at the enemy's ears.
    pub received_energy: Bands3,
    /// Scalar salience weighted by the enemy's frequency sensitivity profile.
    pub salience: f32,
    pub kind: AcousticEventKind,
}

/// Evaluates how well a blind monster hears an `AcousticEvent` using the exact same
/// voxel density occlusion and bounce paths as the player audio engine.
pub fn evaluate_ai_hearing(
    world: &VoxelWorld,
    event: AcousticEvent,
    listener_pos_m: Vec3,
    band_sensitivity: Bands3,
    hearing_threshold: f32,
) -> Option<PerceivedSound> {
    let occ = compute_direct_occlusion(world, event.pos_m, listener_pos_m, 2.0, 55.0);
    let mut received = event.energy.mul(occ.direct_gain);
    let mut perceived_pos = event.pos_m;

    // If direct path is occluded, check 1-bounce corner diffraction paths so sound wraps around hallways
    if occ.solid_thickness_m > 0.2 {
        let taps = trace_early_reflections(
            world,
            event.pos_m,
            listener_pos_m,
            PathTraceConfig {
                early_reflection_rays: 16,
                ..Default::default()
            },
            42,
        );
        let direct_weighted = received.mul(band_sensitivity);
        let direct_salience =
            direct_weighted.0[0] + direct_weighted.0[1] + direct_weighted.0[2];

        let mut best_tap_dir: Option<(f32, Vec3)> = None;
        let mut corner_energy = Bands3::ZERO;
        for tap in taps.iter().filter(|t| t.active) {
            let bounce_e = event.energy.mul(tap.gain);
            let w = bounce_e.mul(band_sensitivity);
            let s = w.0[0] + w.0[1] + w.0[2];
            if best_tap_dir.is_none_or(|(best_s, _)| s > best_s) {
                best_tap_dir = Some((s, tap.direction));
            }
            corner_energy = corner_energy.add(bounce_e.scale(0.55));
        }

        if let Some((best_tap_salience, best_dir)) = best_tap_dir {
            let corner_weighted = corner_energy.mul(band_sensitivity);
            let corner_salience =
                corner_weighted.0[0] + corner_weighted.0[1] + corner_weighted.0[2];
            if best_tap_salience > direct_salience || corner_salience > direct_salience {
                received = received.add(corner_energy);
                // Enemy hears echo coming from down the corridor rather than cheating through walls!
                perceived_pos = listener_pos_m + best_dir * 4.0;
            }
        }
    }

    // Subtle physical transient/rumble shaping:
    // - Continuous low-frequency rumbles (low sharpness, e.g. heavy relic drag or winch machinery)
    //   couple slightly stronger into Low-band ground/rock vibration.
    // - Sharp percussive transients (high sharpness, e.g. pickaxe breach, dropped item, shout)
    //   accent Mid/High bands for transient-hunting predators.
    let sharp = event.sharpness.clamp(0.0, 1.0);
    let shaped_received = Bands3::new(
        received.0[0] * (1.08 - 0.08 * sharp),
        received.0[1] * (0.97 + 0.06 * sharp),
        received.0[2] * (0.95 + 0.10 * sharp),
    );

    let weighted = shaped_received.mul(band_sensitivity);
    let salience = weighted.0[0] + weighted.0[1] + weighted.0[2];

    if salience >= hearing_threshold {
        Some(PerceivedSound {
            perceived_pos_m: perceived_pos,
            received_energy: shaped_received,
            salience,
            kind: event.kind,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{IVec3, UVec3};
    use voxel_core::{VoxelConfig, VoxelId};

    #[test]
    fn sealed_stone_wall_hides_whisper_from_blind_monster() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(2));
        let mut world = VoxelWorld::new(cfg);

        // Seal a complete room wall at x = 10..=11
        for y in 0..24 {
            for z in 0..24 {
                world.set(IVec3::new(10, y, z), VoxelId::CaveStone);
                world.set(IVec3::new(11, y, z), VoxelId::CaveStone);
            }
        }

        let whisper = AcousticEvent {
            pos_m: cfg.voxel_to_world_center(IVec3::new(7, 8, 8)),
            energy: Bands3::new(0.4, 0.7, 0.9),
            sharpness: 0.4,
            kind: AcousticEventKind::VoiceSpeech,
        };
        let monster_pos = cfg.voxel_to_world_center(IVec3::new(15, 8, 8));
        let stalker_sens = Bands3::new(0.4, 1.2, 1.6);

        // Sealed wall blocks whisper
        assert!(evaluate_ai_hearing(&world, whisper, monster_pos, stalker_sens, 0.35).is_none());

        // Breach doorway -> monster now hears it!
        for dy in 7..10 {
            world.set(IVec3::new(10, dy, 8), VoxelId::Air);
            world.set(IVec3::new(11, dy, 8), VoxelId::Air);
        }
        assert!(evaluate_ai_hearing(&world, whisper, monster_pos, stalker_sens, 0.35).is_some());
    }

    #[test]
    fn low_freq_vs_high_freq_predator_hearing_behind_thin_stone_wall() {
        let cfg = VoxelConfig::half_player_scale(UVec3::splat(2));
        let mut world = VoxelWorld::new(cfg);

        // Seal a 1-voxel-thick (0.90m) stone wall at x = 10 across the room cross-section
        for y in 0..24 {
            for z in 0..24 {
                world.set(IVec3::new(10, y, z), VoxelId::CaveStone);
            }
        }

        let src_pos = cfg.voxel_to_world_center(IVec3::new(7, 8, 8));
        let listener_pos = cfg.voxel_to_world_center(IVec3::new(13, 8, 8));

        // Mid/High-sensitive predator (Blind Stalker) vs Low-frequency-sensitive predator (Brute/Lurker)
        let high_sens_predator = Bands3::new(0.40, 1.25, 1.65);
        let low_sens_predator = Bands3::new(1.85, 0.60, 0.15);
        let threshold = 0.35;

        // 1. Heavy low-frequency relic scraping behind the thin stone wall:
        // Low frequencies penetrate 1 voxel of CaveStone (8 dB/m), while Highs drop 32 dB/m.
        let relic_scrape = AcousticEvent {
            pos_m: src_pos,
            energy: Bands3::new(2.2, 0.65, 0.20),
            sharpness: 0.10,
            kind: AcousticEventKind::RelicScrape,
        };

        let low_heard = evaluate_ai_hearing(
            &world,
            relic_scrape,
            listener_pos,
            low_sens_predator,
            threshold,
        );
        let high_heard = evaluate_ai_hearing(
            &world,
            relic_scrape,
            listener_pos,
            high_sens_predator,
            threshold,
        );
        assert!(
            low_heard.is_some(),
            "low-frequency-sensitive predator should detect heavy relic rumble through thin stone wall"
        );
        assert!(
            high_heard.is_none(),
            "high-frequency-sensitive predator should not detect muffled low rumble behind sealed stone wall"
        );

        // 2. Sharp high-frequency transient impact: blocked by thin stone wall for both,
        // but once a doorway is breached, the Mid/High-sensitive predator hears it far more strongly!
        let sharp_impact = AcousticEvent {
            pos_m: src_pos,
            energy: Bands3::new(0.15, 0.70, 1.40),
            sharpness: 0.95,
            kind: AcousticEventKind::ItemImpact,
        };
        assert!(
            evaluate_ai_hearing(&world, sharp_impact, listener_pos, high_sens_predator, threshold)
                .is_none(),
            "thin stone wall should muffle high-frequency impact below threshold"
        );

        // Breach doorway at x = 10
        for dy in 7..10 {
            world.set(IVec3::new(10, dy, 8), VoxelId::Air);
        }

        let open_high = evaluate_ai_hearing(
            &world,
            sharp_impact,
            listener_pos,
            high_sens_predator,
            threshold,
        )
        .expect("high-freq predator hears impact through open doorway");
        let open_low = evaluate_ai_hearing(
            &world,
            sharp_impact,
            listener_pos,
            low_sens_predator,
            0.05,
        )
        .expect("low-freq predator receives weak energy");

        assert!(
            open_high.salience > open_low.salience * 2.0,
            "high-freq predator salience ({}) should dominate low-freq predator ({}) on sharp high impact",
            open_high.salience,
            open_low.salience
        );
    }
}
