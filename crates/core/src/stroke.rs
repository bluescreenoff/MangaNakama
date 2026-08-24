//! Stroke pipeline types. Contract from docs/ARCHITECTURE.md — exact shapes.
//!
//! `PenSample`s flow: Win32 WM_POINTER (app) -> [stabilizer, later] ->
//! `StrokeSink` (brush) -> `Document` tiles.

use crate::doc::Document;

/// One pen/mouse sample in **canvas pixel space** (not screen space — the app
/// applies the viewport transform before handing samples over).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PenSample {
    pub x: f32,
    pub y: f32,
    /// 0..1. Windows reports 0..1024; the app divides by 1024.0.
    pub pressure: f32,
    /// Degrees, -90..90, from `POINTER_PEN_INFO::tiltX` (passthrough for now).
    pub tilt_x: f32,
    /// Degrees, -90..90, from `POINTER_PEN_INFO::tiltY`.
    pub tilt_y: f32,
    /// Milliseconds, monotonic within a stroke.
    pub t_ms: f64,
}

/// The global pen-pressure correction (row 89, BR-014–016): evaluate a
/// monotone-X piecewise-linear curve at pressure `p` (0..1). EMPTY points
/// = identity — the value passes through untouched, bit for bit, which is
/// what every file written before the wizard carried.
pub fn eval_pressure_curve(points: &[[f32; 2]], p: f32) -> f32 {
    if points.is_empty() {
        return p;
    }
    let p = p.clamp(0.0, 1.0);
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if p <= b[0] {
            if b[0] <= a[0] {
                return b[1];
            }
            let t = ((p - a[0]) / (b[0] - a[0])).clamp(0.0, 1.0);
            return a[1] + (b[1] - a[1]) * t;
        }
    }
    points[points.len() - 1][1]
}

/// The wizard's Stronger/Weaker output: `y = x^gamma` sampled on 17
/// points. gamma < 1 lifts light pressures ("Stronger" — a light hand
/// lays more ink), > 1 sinks them ("Weaker"), 1 = the identity diagonal.
pub fn gamma_pressure_curve(gamma: f32) -> Vec<[f32; 2]> {
    (0..=16)
        .map(|i| {
            let x = i as f32 / 16.0;
            [x, x.powf(gamma).clamp(0.0, 1.0)]
        })
        .collect()
}

/// Anything that turns `PenSample`s into pixels. `brush::SimpleDab` today,
/// `brush::MyBrush` (libmypaint) once the FFI lands — the app only ever sees
/// this trait, so swapping engines touches one line.
pub trait StrokeSink {
    fn begin(&mut self, doc: &mut Document);
    fn sample(&mut self, doc: &mut Document, s: PenSample);
    fn end(&mut self, doc: &mut Document);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Row 89: an EMPTY curve is the identity — the exact passthrough
    /// every pre-wizard stroke took, and "correction off" afterwards.
    #[test]
    fn an_empty_pressure_curve_is_the_exact_identity() {
        for p in [0.0f32, 0.001, 0.25, 0.5, 0.75, 0.999, 1.0] {
            assert_eq!(eval_pressure_curve(&[], p), p);
        }
    }

    /// Stronger lifts light pressures (γ<1), Weaker sinks them (γ>1),
    /// and both ends of every curve stay pinned at (0,0)/(1,1).
    #[test]
    fn gamma_curves_lift_and_sink_light_pressures() {
        let stronger = gamma_pressure_curve(0.5);
        let weaker = gamma_pressure_curve(2.0);
        let flat = gamma_pressure_curve(1.0);
        for (s, w, f) in stronger
            .iter()
            .zip(&weaker)
            .zip(&flat)
            .map(|((a, b), c)| (a, b, c))
        {
            assert!(s[1] >= w[1], "γ<1 sits above γ>1 at x={}", s[0]);
            assert_eq!(f, &[s[0], s[0]], "γ=1 is the diagonal");
        }
        for c in [&stronger, &weaker] {
            assert_eq!(c.first().copied(), Some([0.0, 0.0]));
            assert_eq!(c.last().copied(), Some([1.0, 1.0]));
            // Monotone in x, and evaluation hits the points exactly.
            assert!(c.windows(2).all(|w| w[0][0] < w[1][0]));
        }
        assert!(
            eval_pressure_curve(&stronger, 0.25) > 0.25,
            "Stronger lifts a light touch"
        );
        assert!(eval_pressure_curve(&weaker, 0.25) < 0.25, "Weaker sinks it");
    }

    /// Evaluation is the piecewise line BETWEEN the points, and clamps
    /// outside the curve's x-range.
    #[test]
    fn evaluation_interpolates_and_clamps() {
        let pts = [[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]];
        assert_eq!(eval_pressure_curve(&pts, 0.25), 0.125);
        assert_eq!(eval_pressure_curve(&pts, -1.0), 0.0);
        assert_eq!(eval_pressure_curve(&pts, 2.0), 1.0);
    }
}
