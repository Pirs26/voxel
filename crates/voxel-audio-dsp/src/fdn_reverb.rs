use std::f32::consts::TAU;

use voxel_acoustics::{Bands3, ReverbEstimate};

const CHANNELS: usize = 8;
/// Mutually prime base delay lengths (in samples at 48 kHz) scaled by path-traced room size.
const BASE_DELAY_MS: [f32; CHANNELS] = [19.3, 23.7, 29.1, 35.9, 41.3, 47.7, 55.1, 63.3];

/// 8-channel Hadamard Feedback Delay Network (FDN) reverb dynamically driven by
/// path-traced voxel `ReverbEstimate` (RT60 per frequency band + mean free path).
pub struct FdnReverb {
    sample_rate: f32,
    buffers: [Vec<f32>; CHANNELS],
    write_idx: [usize; CHANNELS],
    lp_state: [f32; CHANNELS],
    smoothed_rt60: Bands3,
    smoothed_scale: f32,
}

impl FdnReverb {
    pub fn new(sample_rate: f32) -> Self {
        let max_samples = ((sample_rate * 0.25) as usize).max(2048);
        let buffers = std::array::from_fn(|_| vec![0.0f32; max_samples]);
        Self {
            sample_rate,
            buffers,
            write_idx: [0; CHANNELS],
            lp_state: [0.0; CHANNELS],
            smoothed_rt60: Bands3::new(0.8, 0.6, 0.4),
            smoothed_scale: 1.0,
        }
    }

    #[inline]
    pub fn process_stereo(&mut self, in_l: f32, in_r: f32, target: ReverbEstimate) -> (f32, f32) {
        // Smooth target room size & RT60 gently so moving between shed & cavern never clicks
        self.smoothed_rt60 = self.smoothed_rt60.lerp(target.rt60_sec, 0.0005);
        let target_scale = (target.mean_free_path_m / 10.0).clamp(0.45, 2.4);
        self.smoothed_scale += (target_scale - self.smoothed_scale) * 0.0002;

        let rt_low = self.smoothed_rt60.0[0].clamp(0.12, 6.0);
        let rt_high = self.smoothed_rt60.0[2].clamp(0.10, 4.0);
        let damp_alpha = 1.0 - (-TAU * 2400.0 / self.sample_rate).exp();

        let mut delayed = [0.0f32; CHANNELS];
        let mut fb_gains = [0.0f32; CHANNELS];
        let mut hf_ratios = [0.0f32; CHANNELS];

        for ch in 0..CHANNELS {
            let buf_len = self.buffers[ch].len();
            let delay_sec = (BASE_DELAY_MS[ch] * 0.001 * self.smoothed_scale).clamp(0.008, 0.22);
            let delay_samples = ((delay_sec * self.sample_rate) as usize).clamp(16, buf_len - 2);
            let read_idx = (self.write_idx[ch] + buf_len - delay_samples) % buf_len;
            delayed[ch] = self.buffers[ch][read_idx];

            // Compute exact per-delay feedback gain for target RT60: g = 10^(-3 * delay / RT60)
            let g_low = 10.0f32.powf(-3.0 * delay_sec / rt_low).clamp(0.0, 0.965);
            let g_high = 10.0f32.powf(-3.0 * delay_sec / rt_high).clamp(0.0, g_low);
            fb_gains[ch] = g_low;
            hf_ratios[ch] = if g_low > 1e-4 { (g_high / g_low).clamp(0.05, 1.0) } else { 0.5 };
        }

        // Fast orthonormal 8x8 Walsh-Hadamard mixing matrix (energy-preserving!)
        let mixed = hadamard8(delayed);

        let mut out_l = 0.0f32;
        let mut out_r = 0.0f32;

        for ch in 0..CHANNELS {
            // Apply frequency-dependent wall damping inside the feedback loop
            let s = mixed[ch];
            self.lp_state[ch] += (s - self.lp_state[ch]) * damp_alpha;
            let damped = self.lp_state[ch] + (s - self.lp_state[ch]) * hf_ratios[ch];

            let inj = if ch % 2 == 0 { in_l } else { in_r } * 0.35;
            let next = (damped * fb_gains[ch] + inj).clamp(-8.0, 8.0);

            let w = self.write_idx[ch];
            self.buffers[ch][w] = next;
            self.write_idx[ch] = (w + 1) % self.buffers[ch].len();

            if ch % 2 == 0 {
                out_l += delayed[ch];
            } else {
                out_r += delayed[ch];
            }
        }

        (out_l * 0.32, out_r * 0.32)
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
            let imp = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = fdn.process_stereo(imp, imp, cavern);
            assert!(l.is_finite() && r.is_finite());
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak > 0.01 && peak < 4.0, "unexpected peak: {peak}");
    }
}
