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

    #[inline]
    pub fn process(
        &mut self,
        dry_mono: f32,
        taps: &[EarlyTap; MAX_EARLY_TAPS],
        listener_right: Vec3,
    ) -> (f32, f32) {
        let len = self.buf.len();
        self.buf[self.write_idx] = dry_mono;

        let mut out_l = 0.0f32;
        let mut out_r = 0.0f32;

        for tap in taps {
            if !tap.active {
                continue;
            }
            let delay_samps = ((tap.delay_sec * self.sample_rate) as usize).clamp(1, len - 2);
            let r_idx = (self.write_idx + len - delay_samps) % len;
            let sample = self.buf[r_idx];
            // Average 3-band gain for early tap energy
            let gain = tap.gain.mean().clamp(0.0, 1.2);
            let (gl, gr) = stereo_pan_gains(tap.direction, listener_right);
            out_l += sample * gain * gl;
            out_r += sample * gain * gr;
        }

        self.write_idx = (self.write_idx + 1) % len;
        (out_l, out_r)
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
    }
}
