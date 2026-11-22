use std::f32::consts::TAU;

use voxel_acoustics::{Bands3, ReverbEstimate};

const CHANNELS: usize = 8;
/// Mutually prime base delay lengths (in samples at 48 kHz) scaled by path-traced room size.
const BASE_DELAY_MS: [f32; CHANNELS] = [19.3, 23.7, 29.1, 35.9, 41.3, 47.7, 55.1, 63.3];

/// 8-channel Hadamard Feedback Delay Network (FDN) reverb dynamically driven by
/// path-traced voxel `ReverbEstimate` (RT60 per frequency band + mean free path).
pub struct FdnReverb {
    sample_rate: f32,
    damp_alpha: f32,
    hp_alpha: f32,
    buffers: [Vec<f32>; CHANNELS],
    write_idx: [usize; CHANNELS],
    lp_state: [f32; CHANNELS],
    hp_state: [f32; CHANNELS],
    smoothed_rt60: Bands3,
    smoothed_scale: f32,
    delay_samples: [usize; CHANNELS],
    fb_gains: [f32; CHANNELS],
    hf_ratios: [f32; CHANNELS],
}

impl FdnReverb {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let max_samples = ((sr * 0.25) as usize).max(2048);
        let buffers = std::array::from_fn(|_| vec![0.0f32; max_samples]);
        let damp_alpha = 1.0 - (-TAU * 2600.0 / sr).exp();
        let hp_alpha = 1.0 - (-TAU * 105.0 / sr).exp();
        let mut reverb = Self {
            sample_rate: sr,
            damp_alpha,
            hp_alpha,
            buffers,
            write_idx: [0; CHANNELS],
            lp_state: [0.0; CHANNELS],
            hp_state: [0.0; CHANNELS],
            smoothed_rt60: Bands3::new(0.8, 0.6, 0.4),
            smoothed_scale: 1.0,
            delay_samples: [16; CHANNELS],
            fb_gains: [0.0; CHANNELS],
            hf_ratios: [0.5; CHANNELS],
        };
        reverb.recompute_cached_params();
        reverb
    }

    #[inline]
    fn recompute_cached_params(&mut self) {
        let rt_low = self.smoothed_rt60.0[0].clamp(0.12, 6.0);
        let rt_mid = self.smoothed_rt60.0[1].clamp(0.12, 5.0);
        let rt_high = self.smoothed_rt60.0[2].clamp(0.10, 4.0);
        // Weight core FDN decay toward the Mid band so vocal formants and footsteps stay
        // clear and atmospheric without excessive low-frequency mud accumulation.
        let rt_body = (rt_mid * 0.68 + rt_low * 0.32).clamp(0.12, 5.5);

        for (ch, &base_delay_ms) in BASE_DELAY_MS.iter().enumerate() {
            let buf_len = self.buffers[ch].len();
            let delay_sec = (base_delay_ms * 0.001 * self.smoothed_scale).clamp(0.008, 0.22);
            self.delay_samples[ch] = ((delay_sec * self.sample_rate) as usize).clamp(16, buf_len - 2);

            // Compute exact per-delay feedback gain for target RT60: g = 10^(-3 * delay / RT60)
            let g_body = 10.0f32.powf(-3.0 * delay_sec / rt_body).clamp(0.0, 0.962);
            let g_high = 10.0f32.powf(-3.0 * delay_sec / rt_high).clamp(0.0, g_body);
            self.fb_gains[ch] = g_body;
            self.hf_ratios[ch] = if g_body > 1e-4 {
                (g_high / g_body).clamp(0.05, 1.0)
            } else {
                0.5
            };
        }
    }

    /// Advances smoothed room acoustics once per audio buffer and precomputes per-channel
    /// delay lengths, feedback gains, and HF damping ratios so `process_stereo_sample`
    /// runs with zero `powf()` or `exp()` calls per sample.
    #[inline]
    pub fn prepare_block(&mut self, target: ReverbEstimate, block_frames: usize) {
        // Advance exponential smoothing identically to `block_frames` per-sample steps
        // (0.00075/sample for RT60, 0.00035/sample for room scale) so moving through a
        // breached doorway or between tunnel & cavern responds quickly without clicking.
        let n = block_frames.max(1) as f64;
        let rt60_alpha = (1.0 - (1.0 - 0.00075_f64).powf(n)).clamp(0.0, 1.0) as f32;
        let scale_alpha = (1.0 - (1.0 - 0.00035_f64).powf(n)).clamp(0.0, 1.0) as f32;

        self.smoothed_rt60 = self.smoothed_rt60.lerp(target.rt60_sec, rt60_alpha);
        let target_scale = (target.mean_free_path_m / 10.0).clamp(0.45, 2.4);
        self.smoothed_scale += (target_scale - self.smoothed_scale) * scale_alpha;

        self.recompute_cached_params();
    }

    /// Ultra-lean per-sample stereo FDN reverb step using parameters cached by `prepare_block`.
    #[inline]
    pub fn process_stereo_sample(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let mut delayed = [0.0f32; CHANNELS];

        for (ch, slot) in delayed.iter_mut().enumerate() {
            let buf = &self.buffers[ch];
            let buf_len = buf.len();
            let w = self.write_idx[ch];
            let d = self.delay_samples[ch];
            let read_idx = if w >= d { w - d } else { w + buf_len - d };
            *slot = buf[read_idx];
        }

        // Fast orthonormal 8x8 Walsh-Hadamard mixing matrix (energy-preserving!)
        let mixed = hadamard8(delayed);

        let mut out_l = 0.0f32;
        let mut out_r = 0.0f32;
        let damp_alpha = self.damp_alpha;
        let hp_alpha = self.hp_alpha;

        for ch in 0..CHANNELS {
            // 1. High-frequency wall/air absorption inside the feedback loop
            let s = mixed[ch];
            self.lp_state[ch] += (s - self.lp_state[ch]) * damp_alpha;
            let hf_damped = self.lp_state[ch] + (s - self.lp_state[ch]) * self.hf_ratios[ch];

            // 2. Sub-105 Hz low-cut shelf prevents muddy bass buildup on voices & footsteps
            self.hp_state[ch] += (hf_damped - self.hp_state[ch]) * hp_alpha;
            let clean = hf_damped - self.hp_state[ch] * 0.28;

            let inj = if ch % 2 == 0 { in_l } else { in_r } * 0.35;
            let next = (clean * self.fb_gains[ch] + inj).clamp(-8.0, 8.0);

            let w = self.write_idx[ch];
            let buf = &mut self.buffers[ch];
            buf[w] = next;
            let next_w = w + 1;
            self.write_idx[ch] = if next_w < buf.len() { next_w } else { 0 };

            if ch % 2 == 0 {
                out_l += delayed[ch];
            } else {
                out_r += delayed[ch];
            }
        }

        (out_l * 0.32, out_r * 0.32)
    }

    /// Backwards-compatible single-sample entry point. Prefer calling `prepare_block` once
    /// per audio buffer followed by `process_stereo_sample` in real-time callbacks.
    #[inline]
    pub fn process_stereo(&mut self, in_l: f32, in_r: f32, target: ReverbEstimate) -> (f32, f32) {
        self.prepare_block(target, 1);
        self.process_stereo_sample(in_l, in_r)
    }
}

/// Orthonormal 8x8 Walsh-Hadamard transform (factor 1/sqrt(8) = 0.35355339).
#[inline]
fn hadamard8(mut x: [f32; 8]) -> [f32; 8] {
    for i in (0..8).step_by(2) {
        let (a, b) = (x[i], x[i + 1]);
        x[i] = a + b;
        x[i + 1] = a - b;
    }
    for i in (0..8).step_by(4) {
        for j in 0..2 {
            let (a, b) = (x[i + j], x[i + j + 2]);
            x[i + j] = a + b;
            x[i + j + 2] = a - b;
        }
    }
    for j in 0..4 {
        let (a, b) = (x[j], x[j + 4]);
        x[j] = a + b;
        x[j + 4] = a - b;
    }
    const NORM: f32 = 0.35355338;
    for v in &mut x {
        *v *= NORM;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fdn_reverb_decays_stably_without_runaway() {
        let mut fdn = FdnReverb::new(48_000.0);
        let cavern = ReverbEstimate {
            rt60_sec: Bands3::new(3.2, 2.5, 1.4),
            mean_free_path_m: 18.0,
            wet_mix: 0.6,
            enclosure: 1.0,
        };

        let mut peak = 0.0f32;
        for i in 0..24_000 {
            if i % 256 == 0 {
                fdn.prepare_block(cavern, 256);
            }
            let imp = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = fdn.process_stereo_sample(imp, imp);
            assert!(l.is_finite() && r.is_finite());
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak > 0.01 && peak < 4.0, "unexpected peak: {peak}");
    }

    #[test]
    fn prepare_block_smoothing_matches_per_sample_rate() {
        let cavern = ReverbEstimate {
            rt60_sec: Bands3::new(3.2, 2.5, 1.4),
            mean_free_path_m: 18.0,
            wet_mix: 0.6,
            enclosure: 1.0,
        };

        let mut per_sample = FdnReverb::new(48_000.0);
        let mut blocked = FdnReverb::new(48_000.0);

        for _ in 0..1024 {
            let _ = per_sample.process_stereo(0.0, 0.0, cavern);
        }
        for _ in 0..4 {
            blocked.prepare_block(cavern, 256);
        }

        assert!((per_sample.smoothed_scale - blocked.smoothed_scale).abs() < 1e-4);
        for band in 0..3 {
            assert!(
                (per_sample.smoothed_rt60.0[band] - blocked.smoothed_rt60.0[band]).abs() < 1e-3,
                "band {band}: per_sample={} vs blocked={}",
                per_sample.smoothed_rt60.0[band],
                blocked.smoothed_rt60.0[band]
            );
        }
    }

    #[test]
    fn fdn_sub_bass_damping_prevents_dc_and_low_mud_buildup() {
        let cavern = ReverbEstimate {
            rt60_sec: Bands3::new(3.0, 2.4, 1.4),
            mean_free_path_m: 12.0,
            wet_mix: 0.6,
            enclosure: 1.0,
        };

        let mut fdn_sub = FdnReverb::new(48_000.0);
        let mut fdn_mid = FdnReverb::new(48_000.0);
        fdn_sub.prepare_block(cavern, 48_000);
        fdn_mid.prepare_block(cavern, 48_000);

        // Excite both tanks with equal-amplitude 50 ms bursts:
        // - `fdn_sub`: 40 Hz sub-bass rumble + DC offset (mud)
        // - `fdn_mid`: 600 Hz vocal/footstep midrange tone
        for i in 0..2_400 {
            let t = i as f32 / 48_000.0;
            let sub = 0.15 + (TAU * 40.0 * t).sin() * 0.5;
            let mid = (TAU * 600.0 * t).sin() * 0.5;
            let _ = fdn_sub.process_stereo_sample(sub, sub);
            let _ = fdn_mid.process_stereo_sample(mid, mid);
        }

        // Advance 350 ms into the late reverb tail, then measure energy over the next 250 ms
        for _ in 0..16_800 {
            let _ = fdn_sub.process_stereo_sample(0.0, 0.0);
            let _ = fdn_mid.process_stereo_sample(0.0, 0.0);
        }

        let mut late_sub_energy = 0.0f32;
        let mut late_mid_energy = 0.0f32;
        for _ in 0..12_000 {
            let (sl, sr) = fdn_sub.process_stereo_sample(0.0, 0.0);
            let (ml, mr) = fdn_mid.process_stereo_sample(0.0, 0.0);
            assert!(sl.is_finite() && sr.is_finite() && ml.is_finite() && mr.is_finite());
            late_sub_energy += sl * sl + sr * sr;
            late_mid_energy += ml * ml + mr * mr;
        }

        assert!(
            late_mid_energy > late_sub_energy * 2.0,
            "FDN should preserve vocal/footstep midrange ({late_mid_energy}) while damping sub-bass mud ({late_sub_energy})"
        );
    }
}
