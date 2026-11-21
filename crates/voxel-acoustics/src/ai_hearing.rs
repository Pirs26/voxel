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
        if let Some(best_tap) = taps.iter().find(|t| t.active) {
            let bounce_energy = event.energy.mul(best_tap.gain);
            if bounce_energy.mean() > received.mean() {
                received = received.add(bounce_energy);
                // Enemy hears echo coming from down the corridor rather than cheating through walls!
                perceived_pos = listener_pos_m + best_tap.direction * 4.0;
            }
        }
    }

    let weighted = received.mul(band_sensitivity);
    let salience = weighted.0[0] + weighted.0[1] + weighted.0[2];

    if salience >= hearing_threshold {
        Some(PerceivedSound {
            perceived_pos_m: perceived_pos,
            received_energy: received,
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
}
