//! Pressure response curve.
//!
//! Tablets report a raw 0..1 pressure that almost never feels right as-is: pens
//! bottom out early, or need a shove before anything shows. This is the one knob
//! that fixes both — a gamma curve with an optional input range.
//!
//! ```text
//! t   = clamp((p - low) / (high - low), 0, 1)     // range remap
//! out = t ^ gamma                                  // response
//! ```
//!
//! * `gamma == 1` — linear (the identity when `low == 0, high == 1`).
//! * `gamma > 1` — softer: light pressure gives much less ink; more control in
//!   the thin range.
//! * `gamma < 1` — harder: ink comes on fast, saturates early.
//! * `low` — dead zone: pressure below this reads as zero (kills a pen that
//!   idles at 0.05 and never lets you make a hairline).
//! * `high` — saturation point: pressure above this reads as full (for a pen you
//!   cannot physically push to 1.0).
//!
//! Applied **sample-side**, before the sink: the brush engine never sees raw
//! pressure, so every engine gets the same feel for free.

use crate::stroke::PenSample;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PressureCurve {
    /// Response exponent. 1.0 = linear. Clamped to a sane positive range.
    pub gamma: f32,
    /// Input pressure that maps to 0 (dead zone). 0..1.
    pub low: f32,
    /// Input pressure that maps to 1 (saturation). 0..1.
    pub high: f32,
}

impl PressureCurve {
    /// Straight-through: `apply(p) == p`.
    pub const IDENTITY: Self = Self {
        gamma: 1.0,
        low: 0.0,
        high: 1.0,
    };

    /// Gamma only, full 0..1 input range.
    pub const fn new(gamma: f32) -> Self {
        Self {
            gamma,
            low: 0.0,
            high: 1.0,
        }
    }

    /// Gamma plus an input range clamp.
    pub const fn with_range(gamma: f32, low: f32, high: f32) -> Self {
        Self { gamma, low, high }
    }

    /// True when this curve is a no-op (lets callers skip the work).
    pub fn is_identity(&self) -> bool {
        self.gamma == 1.0 && self.low <= 0.0 && self.high >= 1.0
    }

    /// Map a raw 0..1 pressure through the curve. Always returns 0..1, and
    /// never NaN — a NaN input reads as 0.
    pub fn apply(&self, pressure: f32) -> f32 {
        if !pressure.is_finite() {
            return 0.0;
        }
        let p = pressure.clamp(0.0, 1.0);
        let low = self.low.clamp(0.0, 1.0);
        let high = self.high.clamp(0.0, 1.0);

        let t = if high > low {
            ((p - low) / (high - low)).clamp(0.0, 1.0)
        } else if p >= high {
            // Degenerate range (high <= low): a hard step at `high`.
            1.0
        } else {
            0.0
        };

        if self.gamma == 1.0 || t <= 0.0 || t >= 1.0 {
            t
        } else {
            t.powf(self.gamma.clamp(0.01, 100.0))
        }
    }

    /// Copy of `s` with its pressure run through the curve.
    pub fn map_sample(&self, s: PenSample) -> PenSample {
        PenSample {
            pressure: self.apply(s.pressure),
            ..s
        }
    }
}

impl Default for PressureCurve {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn identity_is_identity() {
        let c = PressureCurve::IDENTITY;
        assert!(c.is_identity());
        for p in [0.0, 0.01, 0.25, 0.5, 0.75, 1.0] {
            assert!(close(c.apply(p), p), "{p}");
        }
    }

    #[test]
    fn gamma_bends_the_response() {
        let soft = PressureCurve::new(2.0);
        assert!(close(soft.apply(0.5), 0.25));
        assert!(close(soft.apply(0.0), 0.0));
        assert!(close(soft.apply(1.0), 1.0));

        let hard = PressureCurve::new(0.5);
        assert!(close(hard.apply(0.25), 0.5));
        assert!(hard.apply(0.3) > soft.apply(0.3));
    }

    #[test]
    fn range_clamps_dead_zone_and_saturation() {
        let c = PressureCurve::with_range(1.0, 0.2, 0.8);
        assert!(close(c.apply(0.0), 0.0));
        assert!(close(c.apply(0.2), 0.0));
        assert!(close(c.apply(0.5), 0.5));
        assert!(close(c.apply(0.8), 1.0));
        assert!(close(c.apply(1.0), 1.0));
    }

    #[test]
    fn output_is_bounded_monotonic_and_nan_safe() {
        let c = PressureCurve::with_range(1.7, 0.1, 0.9);
        let mut prev = -1.0;
        for i in 0..=100 {
            let v = c.apply(i as f32 / 100.0);
            assert!((0.0..=1.0).contains(&v), "{v} out of range");
            assert!(v >= prev, "not monotonic at {i}");
            prev = v;
        }
        assert_eq!(c.apply(f32::NAN), 0.0);
        assert_eq!(c.apply(-5.0), 0.0);
        assert!(close(c.apply(5.0), 1.0));
        // Degenerate range must not divide by zero.
        let d = PressureCurve::with_range(1.0, 0.7, 0.7);
        assert_eq!(d.apply(0.6), 0.0);
        assert_eq!(d.apply(0.8), 1.0);
    }

    #[test]
    fn map_sample_only_touches_pressure() {
        let c = PressureCurve::new(2.0);
        let s = PenSample {
            x: 3.0,
            y: 4.0,
            pressure: 0.5,
            tilt_x: 10.0,
            tilt_y: -10.0,
            t_ms: 99.0,
        };
        let out = c.map_sample(s);
        assert!(close(out.pressure, 0.25));
        assert_eq!(
            (out.x, out.y, out.tilt_x, out.tilt_y, out.t_ms),
            (3.0, 4.0, 10.0, -10.0, 99.0)
        );
    }
}
