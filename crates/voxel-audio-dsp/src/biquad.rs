use std::f32::consts::TAU;

use voxel_acoustics::Bands3;

/// Lightweight 3-band acoustic filter built from cascaded first-order low/high shelf
/// approximations — completely click-free under rapid real-time parameter modulation.
#[derive(Clone, Copy, Debug)]
pub struct ThreeBandFilter {
    lp_low_state: f32,
    lp_mid_state: f32,
    g_low: f32,
    g_mid: f32,
    g_high: f32,
    alpha_low: f32,
    alpha_mid: f32,
}

impl ThreeBandFilter {
    pub fn new(sample_rate: f32) -> Self {
        let alpha_low = 1.0 - (-TAU * 350.0 / sample_rate).exp();
        let alpha_mid = 1.0 - (-TAU * 2200.0 / sample_rate).exp();
        Self {
            lp_low_state: 0.0,
            lp_mid_state: 0.0,
            g_low: 1.0,
            g_mid: 1.0,
            g_high: 1.0,
            alpha_low: alpha_low.clamp(0.001, 0.99),
            alpha_mid: alpha_mid.clamp(0.001, 0.99),
        }
    }

    #[inline]
    pub fn process(&mut self, input: f32, target_gains: Bands3) -> f32 {
        // Gentle per-sample slew (~15ms time constant at 48kHz) prevents zipper noise
        const SLEW: f32 = 0.0015;
        self.g_low += (target_gains.0[0] - self.g_low) * SLEW;
        self.g_mid += (target_gains.0[1] - self.g_mid) * SLEW;
        self.g_high += (target_gains.0[2] - self.g_high) * SLEW;

        // Split input into 3 phase-coherent bands: low, mid, high
        self.lp_low_state += (input - self.lp_low_state) * self.alpha_low;
        self.lp_mid_state += (input - self.lp_mid_state) * self.alpha_mid;

        let low = self.lp_low_state;
        let mid = self.lp_mid_state - self.lp_low_state;
        let high = input - self.lp_mid_state;

        low * self.g_low + mid * self.g_mid + high * self.g_high
    }

    pub fn reset(&mut self) {
        self.lp_low_state = 0.0;
        self.lp_mid_state = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_stays_finite_under_abrupt_wall_muffling() {
        let mut f = ThreeBandFilter::new(48_000.0);
        for i in 0..4096 {
            let x = ((i as f32) * 0.15).sin();
            let gains = if i < 2048 {
                Bands3::ONE
            } else {
                Bands3::new(0.4, 0.08, 0.005)
            };
            let y = f.process(x, gains);
            assert!(y.is_finite() && y.abs() < 4.0);
        }
    }
}
