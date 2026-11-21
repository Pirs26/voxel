use serde::{Deserialize, Serialize};

/// Speed of sound in dry cave air (meters per second).
pub const SPEED_OF_SOUND_M_S: f32 = 343.0;

/// 3-band acoustic quantity: `[Low ~250 Hz, Mid ~1 kHz, High ~4 kHz]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bands3(pub [f32; 3]);

impl Default for Bands3 {
    fn default() -> Self {
        Self([1.0, 1.0, 1.0])
    }
}

impl Bands3 {
    pub const ZERO: Self = Self([0.0, 0.0, 0.0]);
    pub const ONE: Self = Self([1.0, 1.0, 1.0]);

    #[inline]
    pub const fn new(low: f32, mid: f32, high: f32) -> Self {
        Self([low, mid, high])
    }

    #[inline]
    pub fn splat(v: f32) -> Self {
        Self([v, v, v])
    }

    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn mul(self, rhs: Self) -> Self {
        Self([
            self.0[0] * rhs.0[0],
            self.0[1] * rhs.0[1],
            self.0[2] * rhs.0[2],
        ])
    }

    #[inline]
    pub fn scale(self, s: f32) -> Self {
        Self([self.0[0] * s, self.0[1] * s, self.0[2] * s])
    }

    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn add(self, rhs: Self) -> Self {
        Self([
            self.0[0] + rhs.0[0],
            self.0[1] + rhs.0[1],
            self.0[2] + rhs.0[2],
        ])
    }

    #[inline]
    pub fn lerp(self, target: Self, alpha: f32) -> Self {
        Self([
            self.0[0] + (target.0[0] - self.0[0]) * alpha,
            self.0[1] + (target.0[1] - self.0[1]) * alpha,
            self.0[2] + (target.0[2] - self.0[2]) * alpha,
        ])
    }

    #[inline]
    pub fn clamp(self, min: f32, max: f32) -> Self {
        Self([
            self.0[0].clamp(min, max),
            self.0[1].clamp(min, max),
            self.0[2].clamp(min, max),
        ])
    }

    #[inline]
    pub fn mean(self) -> f32 {
        (self.0[0] + self.0[1] + self.0[2]) / 3.0
    }
}

#[inline]
pub fn db_to_linear(db: f32) -> f32 {
    if db <= -120.0 {
        0.0
    } else {
        10.0f32.powf(db / 20.0)
    }
}

#[inline]
pub fn linear_to_db(gain: f32) -> f32 {
    if gain <= 1e-6 {
        -120.0
    } else {
        20.0 * gain.log10()
    }
}
