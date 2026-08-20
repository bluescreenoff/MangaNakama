//! Zoom-independent stroke geometry: document-space input resampling.
//!
//! Owner pen-test 2026-08-17 (auditor round 35 mailbox, TEST 1): the same
//! stroke drawn at 25% zoom comes back POLYGONAL. The input path never
//! interpolated — pen samples arrive at screen resolution, `to_canvas`
//! scales them into doc px, and the engine dabs straight segments between
//! consecutive samples. At 100% the gap is 1–3 doc px (invisible); at 25%
//! the same gap is 4–12 doc px and the vertices show. The jaggedness scales
//! with 1/zoom, exactly as reported.
//!
//! The fix: streaming UNIFORM Catmull-Rom through the raw samples, emitting
//! intermediates so consecutive samples stay ≈1 doc px apart at ANY zoom.
//! Shape-PRESERVING by construction — CR passes exactly through every
//! control point, so the emitted polyline is the same curve at higher
//! density. This is deliberately NOT the stabilizer (which bends toward the
//! average and changes feel); the drawn path is unchanged, only sampled
//! more densely. Segments already denser than `MIN_GAP_PX` pass through
//! untouched, so 100%-zoom pen input and the synthetic-stroke harness are
//! byte-identical to before.

use mn_core::PenSample;

/// Max doc-px spacing between emitted samples.
const SPACING_PX: f32 = 1.0;
/// Segments at or below this length pass through untouched — below it,
/// resampling changes nothing visible and only costs engine time.
const MIN_GAP_PX: f32 = 2.0;
/// Cap on intermediates per segment (auditor round 35): zoom clamps at
/// 0.01, so a 200-screen-px move at minimum zoom is a 20,000 doc-px
/// segment — uncapped that is 20k samples through stabilizer + engine for
/// ONE input event, most discarded by the mouse floor anyway. The C
/// interpolates dabs along the chord regardless, so past this there is
/// nothing to buy. `stabilize.rs` caps its drain at 64 for the same reason;
/// this bound is generous (512 ≈ a full 512px segment at 1px spacing).
const MAX_SUBDIV: usize = 512;

/// One stroke's resampler; `reset` at `begin_stroke`, fed the raw
/// canvas-space samples, flushed before `end_stroke`.
pub struct InputResampler {
    /// The 4-sample Catmull-Rom window (canvas px + pressure/tilt/time).
    w: [PenSample; 4],
    filled: usize,
}

impl Default for InputResampler {
    fn default() -> Self {
        Self::new()
    }
}

impl InputResampler {
    pub fn new() -> Self {
        Self {
            w: [Self::zero(); 4],
            filled: 0,
        }
    }

    pub fn reset(&mut self) {
        self.filled = 0;
    }

    fn zero() -> PenSample {
        PenSample {
            x: 0.0,
            y: 0.0,
            pressure: 0.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: 0.0,
        }
    }

    /// Feed one raw sample; returns the samples to emit NOW. The very first
    /// sample of a stroke passes straight through (the ink starts where the
    /// pen touched); after that the window fills silently. Every emission
    /// convention is uniform (auditor round 35): each segment EXCLUDES its
    /// start and ends with its endpoint VERBATIM — consecutive segments
    /// share endpoints exactly once, no raw sample is ever lost (s0 goes
    /// out at push #1; every other sample arrives as some segment's
    /// endpoint), and no point is ever duplicated.
    pub fn push(&mut self, s: PenSample) -> Vec<PenSample> {
        if !(s.x.is_finite() && s.y.is_finite() && s.pressure.is_finite()) {
            return Vec::new();
        }
        if self.filled == 0 {
            self.w[0] = s;
            self.filled = 1;
            return vec![s];
        }
        if self.filled < 4 {
            self.w[self.filled] = s;
            self.filled += 1;
            if self.filled < 4 {
                return Vec::new();
            }
            // The window just completed. TWO segments are emittable: the
            // head s0→s1 (phantom-p0 start condition — the mirror of the
            // flush's phantom-p4 end; without it every stroke's first
            // segment reached the engine as a bare chord, the exact kink
            // artifact this module exists to fix) and then s1→s2. s0 was
            // already emitted at push #1, so the head excludes its start.
            let mut out = emit_segment(self.w[0], self.w[0], self.w[1], self.w[2]);
            out.extend(emit_segment(self.w[0], self.w[1], self.w[2], self.w[3]));
            return out;
        }
        // Slide: [a,b,c,d] + e -> [b,c,d,e]; segment c→d is now complete.
        self.w.rotate_left(1);
        self.w[3] = s;
        emit_segment(self.w[0], self.w[1], self.w[2], self.w[3])
    }

    /// Stroke end: emit the pending tail. Nothing here may drop a raw
    /// sample: every buffered sample after the first is still unemitted.
    /// Arms 2/3 also densify the HEAD segment (their window never reached
    /// the 4-sample push path that does it) — same phantom-p0 condition.
    pub fn flush(&mut self) -> Vec<PenSample> {
        let mut out = Vec::new();
        match self.filled {
            0 | 1 => {}
            2 => out.extend(emit_segment(self.w[0], self.w[0], self.w[1], self.w[1])),
            3 => {
                out.extend(emit_segment(self.w[0], self.w[0], self.w[1], self.w[2]));
                out.extend(emit_segment(self.w[0], self.w[1], self.w[2], self.w[2]));
            }
            4 => {
                // Pushes have already emitted through segment w[1]→w[2]
                // (endpoint w[2] verbatim); the pending tail is the final
                // segment w[2]→w[3], endpoint duplicated as the phantom p4
                // (the standard CR end condition).
                out.extend(emit_segment(self.w[1], self.w[2], self.w[3], self.w[3]));
            }
            _ => unreachable!("filled caps at 4"),
        }
        self.filled = 0;
        out
    }
}

/// Densify the segment p1→p2 with tangents from p0/p3 (uniform Catmull-Rom).
/// Emission EXCLUDES the start (a prior emission's endpoint covered it —
/// s0 went out directly at push #1) and always ends with p2 VERBATIM (t=1
/// is exact in real arithmetic; emitting the control point directly keeps
/// the pass-through guarantee free of f32 rounding). Short segments pass
/// through as the bare endpoint — dense input is emitted exactly as it
/// arrived, just sparser by one shared point per pair.
fn emit_segment(p0: PenSample, p1: PenSample, p2: PenSample, p3: PenSample) -> Vec<PenSample> {
    let d = ((p2.x - p1.x).powi(2) + (p2.y - p1.y).powi(2)).sqrt();
    if !(d.is_finite() && d > MIN_GAP_PX) {
        return vec![p2];
    }
    let n = (((d / SPACING_PX).ceil() as usize).max(2)).min(MAX_SUBDIV);
    // Overshoot envelope (found by the cap test): uniform CR's basis is not
    // hull-positive, so a segment following a HUGE jump (100px after a
    // 20,000px one) inherits a tangent dominated by that jump — (p2−p0)/2 —
    // and the curve rockets ~15× its own length past its endpoints. Corner
    // rounding legitimately needs a little overshoot; 15% of the segment
    // keeps that and bounds the pathological case. Scalar channels
    // (pressure/tilt) stay unclamped — they are bounded by the engine.
    let slack = (0.15 * d).max(0.5);
    let (lox, hix) = (p1.x.min(p2.x) - slack, p1.x.max(p2.x) + slack);
    let (loy, hiy) = (p1.y.min(p2.y) - slack, p1.y.max(p2.y) + slack);
    let mut out = Vec::with_capacity(n);
    for i in 1..n {
        let t = i as f32 / n as f32;
        out.push(PenSample {
            // CR weights (uniform): the same four coefficients per channel.
            x: cr(p0.x, p1.x, p2.x, p3.x, t).clamp(lox, hix),
            y: cr(p0.y, p1.y, p2.y, p3.y, t).clamp(loy, hiy),
            pressure: cr(p0.pressure, p1.pressure, p2.pressure, p3.pressure, t),
            tilt_x: cr(p0.tilt_x, p1.tilt_x, p2.tilt_x, p3.tilt_x, t),
            tilt_y: cr(p0.tilt_y, p1.tilt_y, p2.tilt_y, p3.tilt_y, t),
            // Monotonic within the segment; the engine clamps negative dtime
            // for coincident timestamps the same as it does for real bursts.
            t_ms: p1.t_ms + (p2.t_ms - p1.t_ms) * t as f64,
        });
    }
    out.push(p2);
    out
}

/// One channel of uniform Catmull-Rom at parameter t (0..1 over p1→p2).
fn cr(a: f32, b: f32, c: f32, d: f32, t: f32) -> f32 {
    0.5 * ((2.0 * b)
        + (-a + c) * t
        + (2.0 * a - 5.0 * b + 4.0 * c - d) * t * t
        + (-a + 3.0 * b - 3.0 * c + d) * t * t * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: f32, y: f32, i: u16) -> PenSample {
        PenSample {
            x,
            y,
            pressure: 0.5,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 16.0,
        }
    }

    fn collect(points: &[PenSample]) -> Vec<(f32, f32)> {
        points.iter().map(|p| (p.x, p.y)).collect()
    }

    /// Sparse collinear input passes through EXACTLY (shape-preserving means
    /// the control points are always emitted verbatim; a straight sparse
    /// stroke cannot move).
    #[test]
    fn control_points_pass_through_verbatim() {
        let mut r = InputResampler::new();
        let path: Vec<PenSample> = (0..8).map(|i| s(10.0 + i as f32 * 5.0, 40.0, i)).collect();
        let mut got = Vec::new();
        for p in path.clone() {
            got.extend(r.push(p));
        }
        got.extend(r.flush());
        // Every raw sample appears in the emitted stream unchanged.
        for p in &path {
            assert!(
                got.iter()
                    .any(|g| g.x == p.x && g.y == p.y && g.t_ms == p.t_ms),
                "raw sample ({},{}) missing from the output",
                p.x,
                p.y
            );
        }
        // Order preserved (strictly increasing x).
        for w in got.windows(2) {
            assert!(w[1].x > w[0].x, "emission out of order at x={}", w[0].x);
        }
    }

    /// The point of the fix: a sparse segment becomes a dense run under
    /// ~1 px spacing, and the emitted times stay monotonic.
    #[test]
    fn sparse_segments_dense_and_monotonic() {
        let mut r = InputResampler::new();
        let mut got = Vec::new();
        for p in [
            s(0.0, 0.0, 0),
            s(0.0, 0.0, 1),
            s(0.0, 0.0, 2),
            s(30.0, 0.0, 3),
        ] {
            got.extend(r.push(p));
        }
        // Pushes so far: the first sample (direct) + the head + first
        // segment emissions — both degenerate (0,0)→(0,0) passthroughs
        // contribute one endpoint each. Total 3.
        assert_eq!(got.len(), 3);
        got.extend(r.flush());
        // flush emitted segment (0,0)->(30,0) (≤2px gap? no: 30px) — dense.
        assert!(
            got.len() >= 15,
            "a 30px segment must densify, got {}",
            got.len()
        );
        let mut last = 0.0;
        for p in &got {
            assert!(p.t_ms >= last, "timestamps must not go backwards");
            last = p.t_ms;
        }
        let pts = collect(&got);
        let max_step = pts
            .windows(2)
            .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
            .fold(0.0f32, f32::max);
        assert!(
            max_step <= 2.5,
            "emitted spacing must stay ~1px, got {max_step}"
        );
    }

    /// The shape test — the actual bug: a sparse quarter-arc's samples sit on
    /// a curve whose APEX is far off the chord; Catmull-Rom follows the
    /// bulge. Points ON the polygon would miss it.
    #[test]
    fn curves_follow_the_bulge_not_the_chord() {
        // Quarter circle r=40 centred at origin: samples every 15°.
        let mut r = InputResampler::new();
        let raw: Vec<PenSample> = (0..=6)
            .map(|i| {
                let a = i as f32 * 15.0f32.to_radians();
                s(40.0 * a.cos(), -40.0 * a.sin(), i as u16)
            })
            .collect();
        let mut got = Vec::new();
        for p in raw {
            got.extend(r.push(p));
        }
        got.extend(r.flush());
        // The apex of the middle 45° arc: r*(cos45, -sin45) ≈ (28.28, -28.28).
        // The straight chord between its endpoints passes (34.6, -14.6) at
        // the parametric midpoint — ~3.7px inside the arc. Every emitted
        // point must lie ON the arc within tolerance (CR hugs smooth data).
        for (x, y) in collect(&got) {
            let rad = (x * x + y * y).sqrt();
            assert!(
                (rad - 40.0).abs() < 1.0,
                "emitted point ({x:.2},{y:.2}) off the arc by {:.2}px",
                (rad - 40.0).abs()
            );
        }
    }

    /// Short segments (the 100%-zoom case) pass through untouched — the
    /// harness and dense pen input must be byte-identical to before.
    #[test]
    fn dense_input_is_untouched() {
        let mut r = InputResampler::new();
        let mut got = Vec::new();
        // Gentle 1.07px steps: every segment ≤ MIN_GAP_PX.
        for i in 0..10 {
            got.extend(r.push(s(i as f32, i as f32 * 0.4, i as u16)));
        }
        got.extend(r.flush());
        assert!(
            got.iter()
                .all(|p| p.x.fract() == 0.0 && (p.y * 5.0).fract() == 0.0),
            "no synthetic points may appear for dense input"
        );
    }

    /// Non-finite input is dropped, never forwarded (the FFI guard's rule).
    #[test]
    fn non_finite_dropped() {
        let mut r = InputResampler::new();
        assert!(r.push(s(f32::NAN, 0.0, 0)).is_empty());
        let mut got = r.push(s(5.0, 5.0, 1));
        assert_eq!(got.len(), 1, "the first finite sample starts the stroke");
        got.extend(r.flush());
        // One buffered sample has no pending tail — nothing more to emit.
        assert_eq!(got.len(), 1);
    }

    /// Auditor round 35: the FIRST segment (s0→s1) used to reach the engine
    /// as a bare chord — the kink artifact at the head of every stroke.
    /// A 45° head gap on r=40 (the widest gap a phantom-p0 CR handles
    /// well; at 90° the zero start tangent undershoots by ~10px — the
    /// test's first draft): the gap's arc midpoint sits 3.04px off the
    /// chord; the densified head must reach within 1.2px of the arc there
    /// (the bare chord cannot — that is the discrimination).
    #[test]
    fn head_segment_is_densified() {
        let mut r = InputResampler::new();
        let mut got = Vec::new();
        // Samples at 0°, 45°, 90°, 180°, 225° — the head gap is FIRST.
        for (i, deg) in [0.0f32, 45.0, 90.0, 180.0, 225.0].iter().enumerate() {
            let a = deg.to_radians();
            got.extend(r.push(s(40.0 * a.cos(), -40.0 * a.sin(), i as u16)));
        }
        got.extend(r.flush());
        // The head-gap midpoint: 22.5° on the arc.
        let (mx, my) = (
            40.0 * 22.5f32.to_radians().cos(),
            -40.0 * 22.5f32.to_radians().sin(),
        );
        let best = got
            .iter()
            .map(|p| (p.x - mx).hypot(p.y - my))
            .fold(f32::MAX, f32::min);
        assert!(
            best < 2.2,
            "the densified head must reach the arc's 22.5° midpoint (chord misses 3.04; bar 2.2 discriminates), missed by {best:.2}px"
        );
    }

    /// Auditor round 35: densification must be capped — a single
    /// 20,000px segment (a 200px screen move at the 0.01 zoom floor)
    /// must not emit 20k samples for one input event.
    #[test]
    fn subdivision_is_capped() {
        let mut r = InputResampler::new();
        let mut got = Vec::new();
        for (i, p) in [
            s(0.0, 0.0, 0),
            s(20000.0, 0.0, 1),
            s(20100.0, 0.0, 2),
            s(20200.0, 0.0, 3),
            s(20300.0, 0.0, 4),
        ]
        .iter()
        .enumerate()
        {
            let _ = i;
            got.extend(r.push(*p));
        }
        got.extend(r.flush());
        assert!(
            got.len() < 900,
            "20k/100px segments must stay capped (512 intermediates max per segment), got {}",
            got.len()
        );
        // The property the envelope actually guarantees (monotone-x per
        // segment is NOT one — real strokes curve back): no point may
        // escape the input's overall span by more than a small segment's
        // overshoot allowance. Pre-envelope, the CR after the 20k jump
        // reached x=21507; this bound catches exactly that class.
        for p in &got {
            assert!(
                (-1.0..=20316.0).contains(&p.x) && p.y.abs() <= 16.0,
                "point ({},{}) escaped the input span — CR overshoot is back",
                p.x,
                p.y
            );
        }
    }
}
