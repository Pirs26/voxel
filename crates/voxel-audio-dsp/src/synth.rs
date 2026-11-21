use std::{f32::consts::TAU, path::Path};

use crate::wav::load_wav_mono;

/// Built-in clip identifiers. Footsteps load real CC0 Foley WAVs from `assets/sfx/`
/// (falling back gracefully to procedural synthesis if a file is missing).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SynthClip {
    /// Recorded CC0 stone footstep (`assets/sfx/footstep_stone.wav`).
    FootstepStone,
    /// Recorded CC0 dirt/gravel footstep (`assets/sfx/footstep_dirt.wav`).
    FootstepDirt,
    /// Recorded CC0 wooden shed footsteps (`assets/sfx/footstep_wood1..3.wav`).
    FootstepWood,
    FootstepWood2,
    FootstepWood3,
    /// Sharp metallic pickaxe impact on cracked rock.
    PickaxeHit,
    /// Soft spongy hiss of spraying acoustic foam.
    FoamSpray,
    /// Clear periodic crystalline acoustic beacon ping (ideal for A/B muffling & echo tests!).
    BeaconPing,
    /// Flare strike + brief sizzle.
    FlareIgnite,
    /// Atmospheric cavern water drip.
    WaterDrip,
}

impl SynthClip {
    pub const ALL: [SynthClip; 10] = [
        SynthClip::FootstepStone,
        SynthClip::FootstepDirt,
        SynthClip::FootstepWood,
        SynthClip::FootstepWood2,
        SynthClip::FootstepWood3,
        SynthClip::PickaxeHit,
        SynthClip::FoamSpray,
        SynthClip::BeaconPing,
        SynthClip::FlareIgnite,
        SynthClip::WaterDrip,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn wav_filename(self) -> Option<&'static str> {
        match self {
            SynthClip::FootstepStone => Some("footstep_stone.wav"),
            SynthClip::FootstepDirt => Some("footstep_dirt.wav"),
            SynthClip::FootstepWood => Some("footstep_wood1.wav"),
            SynthClip::FootstepWood2 => Some("footstep_wood2.wav"),
            SynthClip::FootstepWood3 => Some("footstep_wood3.wav"),
            _ => None,
        }
    }
}

/// Loads a clip from `assets/sfx/<file>` (checking current dir, executable dir for packaged
/// releases, and Cargo workspace dir in dev) with graceful procedural synthesis fallback.
pub fn load_or_generate_clip(clip: SynthClip, sample_rate: f32) -> Vec<f32> {
    if let Some(file) = clip.wav_filename() {
        let mut candidates = vec![
            Path::new("assets/sfx").join(file),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/sfx")
                .join(file),
        ];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.insert(0, dir.join("assets/sfx").join(file));
            }
        }
        for path in candidates {
            if path.exists() {
                if let Ok(samples) = load_wav_mono(&path, sample_rate) {
                    if !samples.is_empty() {
                        return samples;
                    }
                }
            }
        }
    }
    generate_clip(clip, sample_rate)
}

/// Generates a mono sample buffer for procedural clips (and WAV fallbacks).
pub fn generate_clip(clip: SynthClip, sample_rate: f32) -> Vec<f32> {
    let mut seed: u32 = 0x1234_5678;
    let mut next_noise = || -> f32 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((seed >> 9) as f32 / ((1u32 << 23) as f32)) * 2.0 - 1.0
    };

    match clip {
        SynthClip::FootstepStone | SynthClip::FootstepDirt => {
            let len = (sample_rate * 0.14) as usize;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (-t * 36.0).exp();
                    let thump = (TAU * (95.0 - t * 220.0).max(45.0) * t).sin() * 0.65;
                    let grit = next_noise() * (-t * 55.0).exp() * 0.45;
                    (thump + grit) * env * 0.75
                })
                .collect()
        }
        SynthClip::FootstepWood | SynthClip::FootstepWood2 | SynthClip::FootstepWood3 => {
            let len = (sample_rate * 0.16) as usize;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (-t * 28.0).exp();
                    let body = (TAU * 145.0 * t).sin() * 0.55 + (TAU * 290.0 * t).sin() * 0.25;
                    let tap = next_noise() * (-t * 80.0).exp() * 0.25;
                    (body + tap) * env * 0.7
                })
                .collect()
        }
        SynthClip::PickaxeHit => {
            let len = (sample_rate * 0.32) as usize;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let clang = (TAU * 1380.0 * t).sin() * (-t * 16.0).exp() * 0.45
                        + (TAU * 2740.0 * t).sin() * (-t * 24.0).exp() * 0.30;
                    let crack = next_noise() * (-t * 40.0).exp() * 0.65;
                    let bass = (TAU * 110.0 * t).sin() * (-t * 30.0).exp() * 0.55;
                    (clang + crack + bass) * 0.65
                })
                .collect()
        }
        SynthClip::FoamSpray => {
            let len = (sample_rate * 0.24) as usize;
            let mut lp = 0.0f32;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (t * 25.0).min(1.0) * (-t * 12.0).exp();
                    let n = next_noise();
                    lp += (n - lp) * 0.22;
                    let hiss = (n - lp) * 0.55 + lp * 0.2;
                    hiss * env * 0.55
                })
                .collect()
        }
        SynthClip::BeaconPing => {
            // Broadband transient + clear harmonic chime so both low-pass muffling and
            // crisp cavern slap-back echoes are immediately audible!
            let len = (sample_rate * 0.45) as usize;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let click = next_noise() * (-t * 180.0).exp() * 0.50;
                    let tone = (TAU * 320.0 * t).sin() * (-t * 9.0).exp() * 0.40
                        + (TAU * 880.0 * t).sin() * (-t * 11.0).exp() * 0.45
                        + (TAU * 2640.0 * t).sin() * (-t * 16.0).exp() * 0.35;
                    (click + tone) * 0.72
                })
                .collect()
        }
        SynthClip::FlareIgnite => {
            let len = (sample_rate * 0.38) as usize;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let strike = next_noise() * (-t * 14.0).exp() * 0.60;
                    let pop = (TAU * 420.0 * t).sin() * (-t * 45.0).exp() * 0.40;
                    (strike + pop) * 0.65
                })
                .collect()
        }
        SynthClip::WaterDrip => {
            let len = (sample_rate * 0.14) as usize;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    // Classic rising pitch water-bubble chirp
                    let freq = 750.0 + t * 5200.0;
                    let env = (t * 180.0).min(1.0) * (-t * 32.0).exp();
                    (TAU * freq * t).sin() * env * 0.60
                })
                .collect()
        }
    }
}
