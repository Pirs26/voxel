use glam::Vec3;
use voxel_acoustics::{EarlyTap, MAX_EARLY_TAPS};

/// Computes stereo panning weights (ILD) from a world direction and listener basis.
#[inline]
pub fn stereo_pan_gains(dir: Vec3, listener_right: Vec3) -> (f32, f32) {
    let pan = dir.normalize_or_zero().dot(listener_right).clamp(-1.0, 1.0);
    // Equal-power pan law
    let angle = (pan + 1.0) * 0.25 * std::f32::consts::PI;
    (angle.cos(), angle.sin())
}

/// Per-source ring-buffer delay line supporting up to `MAX_EARLY_TAPS` directional echoes.
pub struct EarlyReflectionLine {
    sample_rate: f32,
    buf: Vec<f32>,
    write_idx: usize,
}

impl EarlyReflectionLine {
    pub fn new(sample_rate: f32) -> Self {
        let max_len = ((sample_rate * 0.25) as usize).max(1024);
        Self {
            sample_rate,
            buf: vec![0.0; max_len],
            write_idx: 0,
        }
    }

    /// Precomputes packed `(gain_l, gain_r, delay_samples)` for all active early reflection taps,
    /// returning the number of active entries written to `out`.
    #[inline]
    pub fn prepare_cached_taps(
        &self,
        taps: &[EarlyTap; MAX_EARLY_TAPS],
        listener_right: Vec3,
        out: &mut [(f32, f32, usize); MAX_EARLY_TAPS],
    ) -> usize {
        let max_delay = self.buf.len() - 2;
        let mut count = 0usize;
        for tap in taps {
            if !tap.active {
                continue;
            }
            let delay_samps = ((tap.delay_sec * self.sample_rate) as usize).clamp(1, max_delay);
            let gain = tap.gain.mean().clamp(0.0, 1.2);
            let (gl, gr) = stereo_pan_gains(tap.direction, listener_right);
            out[count] = (gain * gl, gain * gr, delay_samps);
            count += 1;
        }
        count
    }

    /// Ultra-lean per-sample early reflection step using precomputed `(gain_l, gain_r, delay_samples)`
    /// tuples from `prepare_cached_taps` — zero trigonometry, zero `sqrt()`, zero integer division.
    #[inline]
    pub fn process_cached(
        &mut self,
        dry_mono: f32,
        cached_taps: &[(f32, f32, usize)],
    ) -> (f32, f32) {
        let len = self.buf.len();
        let w = self.write_idx;
        self.buf[w] = dry_mono;

        let mut out_l = 0.0f32;
        let mut out_r = 0.0f32;

        for &(gain_l, gain_r, delay_samps) in cached_taps {
            let d = delay_samps.min(len - 1);
            let r_idx = if w >= d { w - d } else { w + len - d };
            let sample = self.buf[r_idx];
            out_l += sample * gain_l;
            out_r += sample * gain_r;
        }

        let next_w = w + 1;
        self.write_idx = if next_w < len { next_w } else { 0 };
        (out_l, out_r)
    }

    #[inline]
    pub fn process(
        &mut self,
        dry_mono: f32,
        taps: &[EarlyTap; MAX_EARLY_TAPS],
        listener_right: Vec3,
    ) -> (f32, f32) {
        let mut cached = [(0.0f32, 0.0f32, 1usize); MAX_EARLY_TAPS];
        let count = self.prepare_cached_taps(taps, listener_right, &mut cached);
        self.process_cached(dry_mono, &cached[..count])
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
    }
}
