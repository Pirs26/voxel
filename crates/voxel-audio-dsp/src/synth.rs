use std::{f32::consts::TAU, path::Path};

use crate::wav::load_wav_mono;

/// Built-in clip identifiers. Footsteps load real CC0 Foley WAVs from `assets/sfx/`
/// (falling back gracefully to procedural synthesis if a file is missing).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SynthClip {
    /// Recorded CC0 stone footstep (`assets/sfx/footstep_stone.wav`).
    FootstepStone,
    /// Grounded single-step earthen/cave-dirt footstep (replaces double-hit WAV).
    FootstepDirt,
    /// Heavy metallic boot-on-steel-grating footstep (elevator deck & shaft rim).
    FootstepSteel,
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
    /// Low metallic elevator cable groan/creak during shaft travel.
    ElevatorCreak,
    /// Loud rhythmic mechanical winch grinding during extraction climax.
    ElevatorWinch,
    /// Heavy steel elevator gate slam.
    DoorSlam,
    /// Heavy stone-on-stone grating scrape (1-player relic drag).
    RelicScrape,
    /// Resonant heavy impact thud (dropped relic / heavy gear).
    HeavyImpact,
    /// Crisp mechanical click/clatter (battery swap or drop).
    BatteryClick,
    /// Low guttural clicking growl from the blind acoustic stalker.
    MonsterGrowl,
    /// Sharp echolocation shriek when the stalker locks onto a loud sound.
    MonsterScreech,
    /// Soft medical zipper + pressurized spray hiss during Aid Kit revive.
    AidKitUse,
    /// Loud metallic rattle/chirp emitted by a throwable Noise Decoy.
    DecoyBuzz,
}

impl SynthClip {
    pub const ALL: [SynthClip; 21] = [
        SynthClip::FootstepStone,
        SynthClip::FootstepDirt,
        SynthClip::FootstepSteel,
        SynthClip::FootstepWood,
        SynthClip::FootstepWood2,
        SynthClip::FootstepWood3,
        SynthClip::PickaxeHit,
        SynthClip::FoamSpray,
        SynthClip::BeaconPing,
        SynthClip::FlareIgnite,
        SynthClip::WaterDrip,
        SynthClip::ElevatorCreak,
        SynthClip::ElevatorWinch,
        SynthClip::DoorSlam,
        SynthClip::RelicScrape,
        SynthClip::HeavyImpact,
        SynthClip::BatteryClick,
        SynthClip::MonsterGrowl,
        SynthClip::MonsterScreech,
        SynthClip::AidKitUse,
        SynthClip::DecoyBuzz,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn wav_filename(self) -> Option<&'static str> {
        match self {
            SynthClip::FootstepStone => Some("footstep_stone.wav"),
            // Note: `footstep_dirt.wav` contains a double-step high-pitched crackle, so
            // `FootstepDirt` uses our clean single-step earthen procedural foley below.
            SynthClip::FootstepWood => Some("footstep_wood1.wav"),
            SynthClip::FootstepWood2 => Some("footstep_wood2.wav"),
            SynthClip::FootstepWood3 => Some("footstep_wood3.wav"),
            _ => None,
        }
    }
}

/// Loads a clip from `assets/sfx/<file>` (checking current dir, executable dir for packaged
/// releases, and Cargo workspace dir in dev) with graceful procedural synthesis fallback.
/// Loaded WAV footsteps are RMS-balanced and gently de-clicked at the tail so surfaces match cleanly.
pub fn load_or_generate_clip(clip: SynthClip, sample_rate: f32) -> Vec<f32> {
    if let Some(file) = clip.wav_filename() {
        let mut candidates = vec![
            Path::new("assets/sfx").join(file),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/sfx")
                .join(file),
        ];
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            candidates.insert(0, dir.join("assets/sfx").join(file));
        }
        for path in candidates {
            if path.exists()
                && let Ok(mut samples) = load_wav_mono(&path, sample_rate)
                && !samples.is_empty()
            {
                normalize_and_declick(&mut samples, sample_rate, 0.11);
                return samples;
            }
        }
    }
    generate_clip(clip, sample_rate)
}

fn normalize_and_declick(samples: &mut [f32], sample_rate: f32, target_rms: f32) {
    if samples.is_empty() {
        return;
    }
    let fade_in = ((sample_rate * 0.002) as usize).min(samples.len() / 4);
    for (i, s) in samples.iter_mut().take(fade_in).enumerate() {
        *s *= (i + 1) as f32 / (fade_in + 1) as f32;
    }
    let fade_out = ((sample_rate * 0.015) as usize).min(samples.len() / 2);
    let len = samples.len();
    for i in 0..fade_out {
        let g = (fade_out - i) as f32 / (fade_out + 1) as f32;
        samples[len - 1 - i] *= g;
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    if rms > 1e-4 {
        let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let gain = (target_rms / rms).clamp(0.45, 1.8).min(0.88 / peak.max(1e-4));
        for s in samples.iter_mut() {
            *s *= gain;
        }
    }
}

/// Generates a mono sample buffer for procedural clips (and WAV fallbacks).
pub fn generate_clip(clip: SynthClip, sample_rate: f32) -> Vec<f32> {
    let mut seed: u32 = 0x1234_5678;
    let mut next_noise = || -> f32 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((seed >> 9) as f32 / ((1u32 << 23) as f32)) * 2.0 - 1.0
    };

    match clip {
        SynthClip::FootstepStone => {
            let len = (sample_rate * 0.14) as usize;
            let mut lp = 0.0f32;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (t * 240.0).min(1.0) * (-t * 36.0).exp();
                    let thump = (TAU * (105.0 - t * 240.0).max(48.0) * t).sin() * 0.55;
                    let n = next_noise();
                    lp += (n - lp) * 0.35;
                    let grit = (n - lp * 0.5) * (-t * 58.0).exp() * 0.42;
                    (thump + grit) * env * 0.70
                })
                .collect()
        }
        SynthClip::FootstepDirt => {
            // Single grounded earthen boot step: warm packed-dirt heel thump + soft low-passed scuff
            let len = (sample_rate * 0.15) as usize;
            let mut lp = 0.0f32;
            let mut lp2 = 0.0f32;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (t * 160.0).min(1.0) * (-t * 30.0).exp();
                    let heel = (TAU * (82.0 - t * 180.0).max(40.0) * t).sin() * 0.55;
                    let n = next_noise();
                    lp += (n - lp) * 0.18;
                    lp2 += (lp - lp2) * 0.28;
                    let scuff = (lp - lp2 * 0.4) * (-t * 38.0).exp() * 0.65;
                    (heel + scuff) * env * 0.68
                })
                .collect()
        }
        SynthClip::FootstepSteel => {
            // Crisp metallic boot step on steel elevator grating: solid heel + short damped steel ring
            let len = (sample_rate * 0.17) as usize;
            let mut lp = 0.0f32;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (t * 320.0).min(1.0) * (-t * 28.0).exp();
                    let thud = (TAU * (125.0 - t * 180.0).max(58.0) * t).sin() * 0.52;
                    let plate = (TAU * 460.0 * t).sin() * (-t * 34.0).exp() * 0.24
                        + (TAU * 890.0 * t).sin() * (-t * 48.0).exp() * 0.16
                        + (TAU * 1540.0 * t).sin() * (-t * 65.0).exp() * 0.10;
                    let n = next_noise();
                    lp += (n - lp) * 0.45;
                    let clank = lp * (-t * 75.0).exp() * 0.30;
                    (thud + plate + clank) * env * 0.72
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
        SynthClip::ElevatorCreak => {
            let len = (sample_rate * 0.95) as usize;
            let mut lp = 0.0f32;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (t * 5.0).min(1.0) * (-t * 2.1).exp();
                    let wobble = (TAU * 5.5 * t).sin() * 14.0;
                    let groan = (TAU * (82.0 + wobble) * t).sin() * 0.45
                        + (TAU * (168.0 + wobble * 1.6) * t).sin() * 0.30
                        + (TAU * (310.0 + wobble * 2.2) * t).sin() * 0.15;
                    let n = next_noise();
                    lp += (n - lp) * 0.08;
                    (groan + lp * 0.35) * env * 0.68
                })
                .collect()
        }
        SynthClip::ElevatorWinch => {
            let len = (sample_rate * 0.65) as usize;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (t * 20.0).min(1.0) * (-t * 2.8).exp();
                    // Ratcheting metallic gear pulses + heavy motor hum
                    let ratchet = ((TAU * 14.0 * t).sin().max(0.0)).powf(3.0);
                    let gear = (TAU * 540.0 * t).sin() * ratchet * 0.55
                        + (TAU * 1120.0 * t).sin() * ratchet * 0.30;
                    let motor = (TAU * 62.0 * t).sin() * 0.40 + (TAU * 124.0 * t).sin() * 0.22;
                    let grit = next_noise() * (0.20 + 0.35 * ratchet);
                    (gear + motor + grit) * env * 0.78
                })
                .collect()
        }
        SynthClip::DoorSlam => {
            let len = (sample_rate * 0.48) as usize;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let thud = (TAU * (92.0 - t * 90.0).max(38.0) * t).sin() * (-t * 14.0).exp() * 0.70;
                    let clang = (TAU * 420.0 * t).sin() * (-t * 18.0).exp() * 0.35
                        + (TAU * 890.0 * t).sin() * (-t * 24.0).exp() * 0.25;
                    let smash = next_noise() * (-t * 32.0).exp() * 0.55;
                    (thud + clang + smash) * 0.80
                })
                .collect()
        }
        SynthClip::RelicScrape => {
            let len = (sample_rate * 0.32) as usize;
            let mut lp = 0.0f32;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (t * 18.0).min(1.0) * (-t * 6.5).exp();
                    let n = next_noise();
                    lp += (n - lp) * 0.18;
                    let grit = (n - lp) * 0.48 + lp * 0.55;
                    let rumble = (TAU * 76.0 * t).sin() * 0.35 + (TAU * 148.0 * t).sin() * 0.22;
                    (grit + rumble) * env * 0.72
                })
                .collect()
        }
        SynthClip::HeavyImpact => {
            let len = (sample_rate * 0.42) as usize;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let sub = (TAU * (78.0 - t * 110.0).max(32.0) * t).sin() * (-t * 11.0).exp() * 0.80;
                    let stone = next_noise() * (-t * 28.0).exp() * 0.55;
                    let ring = (TAU * 235.0 * t).sin() * (-t * 19.0).exp() * 0.30;
                    (sub + stone + ring) * 0.82
                })
                .collect()
        }
        SynthClip::BatteryClick => {
            // Crisp tactile mechanical latch click (warm metallic snap, never piercing)
            let len = (sample_rate * 0.095) as usize;
            let mut lp = 0.0f32;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let snap1 = (TAU * 920.0 * t).sin() * (-t * 115.0).exp() * 0.45;
                    let snap2 = if t > 0.026 {
                        let t2 = t - 0.026;
                        (TAU * 1380.0 * t2).sin() * (-t2 * 130.0).exp() * 0.38
                    } else {
                        0.0
                    };
                    let n = next_noise();
                    lp += (n - lp) * 0.42;
                    let tick = lp * (-t * 145.0).exp() * 0.48;
                    (snap1 + snap2 + tick) * 0.65
                })
                .collect()
        }
        SynthClip::MonsterGrowl => {
            // Haunting blind-predator vocalization: resonant guttural throat growl (92/184/295 Hz
            // audible on all speakers + penetrates stone as muffled rumble), raspy snarl, and
            // crisp bony echolocation jaw-clicks that slap-back down winding tunnels!
            let len = (sample_rate * 0.96) as usize;
            let mut lp_low = 0.0f32;
            let mut lp_mid = 0.0f32;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (t * 7.0).min(1.0) * (-t * 2.1).exp();
                    let vib = (TAU * 5.2 * t).sin() * 7.5;
                    let f0 = 92.0 + vib - t * 14.0;
                    let throat = (TAU * f0 * t).sin() * 0.44
                        + (TAU * (f0 * 2.02) * t).sin() * 0.34
                        + (TAU * (f0 * 3.18) * t).sin() * 0.22;
                    let n = next_noise();
                    lp_low += (n - lp_low) * 0.09;
                    lp_mid += (n - lp_mid) * 0.32;
                    let rasp = (lp_mid - lp_low) * 0.55;
                    // 4 sharp bony echolocation clicks during the middle of the exhale
                    let click_env = ((TAU * 10.5 * t).sin().max(0.0)).powf(12.0)
                        * if t > 0.10 && t < 0.72 { 1.0 } else { 0.0 };
                    let click_tone = (TAU * 1580.0 * t).sin() * 0.45 + (n - lp_mid) * 0.55;
                    let clicks = click_tone * click_env * 0.72;
                    ((throat + rasp) * env + clicks) * 0.88
                })
                .collect()
        }
        SynthClip::MonsterScreech => {
            let len = (sample_rate * 0.58) as usize;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (t * 32.0).min(1.0) * (-t * 4.2).exp();
                    let sweep = 1450.0 - t * 920.0 + (TAU * 38.0 * t).sin() * 140.0;
                    let shriek = (TAU * sweep * t).sin() * 0.52
                        + (TAU * sweep * 1.49 * t).sin() * 0.30;
                    let hiss = next_noise() * (-t * 9.0).exp() * 0.42;
                    (shriek + hiss) * env * 0.78
                })
                .collect()
        }
        SynthClip::AidKitUse => {
            let len = (sample_rate * 0.35) as usize;
            let mut lp = 0.0f32;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (t * 16.0).min(1.0) * (-t * 7.5).exp();
                    let n = next_noise();
                    lp += (n - lp) * 0.32;
                    let spray = (n - lp) * 0.45;
                    let beep = (TAU * 980.0 * t).sin() * (-t * 22.0).exp() * 0.18;
                    (spray + beep) * env * 0.50
                })
                .collect()
        }
        SynthClip::DecoyBuzz => {
            let len = (sample_rate * 0.28) as usize;
            (0..len)
                .map(|i| {
                    let t = i as f32 / sample_rate;
                    let env = (t * 60.0).min(1.0) * (-t * 8.5).exp();
                    let mod_sig = (TAU * 28.0 * t).sin().signum();
                    let tone = (TAU * 1620.0 * t).sin() * 0.45 + (TAU * 2430.0 * t).sin() * 0.35;
                    let clack = next_noise() * 0.35 * (0.5 + 0.5 * mod_sig);
                    (tone * (0.6 + 0.4 * mod_sig) + clack) * env * 0.75
                })
                .collect()
        }
    }
}
