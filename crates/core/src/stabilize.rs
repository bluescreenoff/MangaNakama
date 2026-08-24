//! The stroke-correction stage — CSP's *Correction* group in one decorator.
//!
//! Two mechanisms live here and they are opposites:
//!
//! * the **pull-string stabilizer** (`C-028`), which buys smoothness with
//!   latency — the brush trails the pen and never sees the future; and
//! * **post correction** (`C-031`, [`PostCorrect`]), which buys smoothness with
//!   a bounded *look-ahead* instead — samples are held in a short arc-length
//!   buffer and each one is smoothed against the samples on **both** sides of
//!   it before it is handed on.
//!
//! Both should exist because they fail differently: the pull-string has a dead
//! zone (tremor inside the string emits nothing at all, so slow detail work
//! stops registering), while post correction always emits every sample and only
//! moves it. On top of post correction sit the corner exception (`C-027`), the
//! zoom compensation (`C-033`) and the entry/exit shaping (`S-023`–`S-027`).
//!
//! A `StrokeSink` **decorator**: it wraps any sink (the brush) and filters the
//! `PenSample` stream on the way through, so nothing downstream knows it exists.
//!
//! ```ignore
//! let mut sink = Stabilizer::new(SimpleDab::new(), 0.5);
//! sink.begin(&mut doc);
//! sink.sample(&mut doc, s);   // smoothed
//! sink.end(&mut doc);         // drains the string, so the stroke reaches the pen
//! ```
//!
//! # The model
//!
//! Imagine a string of length `radius` between the pen tip and the brush. While
//! the pen moves inside that radius the brush does not move at all; once the pen
//! pulls the string taut, the brush is dragged along, always staying exactly
//! `radius` behind. That is the classic "lazy mouse" / pull-string filter: it
//! kills hand tremor and lets you take a corner slowly without wobble, at the
//! cost of the brush lagging behind the cursor.
//!
//! Two properties matter and are unit-tested:
//!
//! * **Deterministic.** No time dependence, no randomness — the same input
//!   samples always produce the same output samples. (Time-based smoothing is
//!   tempting but makes strokes unreproducible and untestable.)
//! * **Drained on end.** Because the brush lags, a naive implementation ends the
//!   stroke `radius` px short of where the pen was lifted. `end()` walks the
//!   remaining string out in small steps, finishing **exactly** on the last raw
//!   sample.

use crate::doc::Document;
use crate::stroke::{PenSample, StrokeSink};

/// String length at `strength == 1.0`, in canvas pixels.
pub const MAX_STRING_PX: f32 = 48.0;
/// Step size used when draining the string at `end()`, in canvas pixels.
/// Small enough that the brush's own dab spacing stays in charge of the result.
const DRAIN_STEP_PX: f32 = 2.0;
/// Hard cap on drain samples, so a huge string cannot emit thousands of dabs.
const DRAIN_MAX_STEPS: u32 = 64;

// ---------------------------------------------------------------------------
// The CSP Correction group beyond the stabilizer.
// ---------------------------------------------------------------------------

/// How the entry/exit shaping length is specified (CSP `S-024`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SeHow {
    /// Absolute run-in / run-out distances in canvas px (`S-025`, `S-026`).
    #[default]
    Length,
    /// Ramp from full to the minimum over the *ending* length measured from
    /// the stroke's START, then hold there. CSP disables *Starting* in this
    /// mode; so do we, because the two would fight over the same first px.
    Fade,
}

/// Which way the pull-string tracks pen speed (CSP `C-030`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StabMode {
    /// A steady hand for detail: the string grows as the pen slows down.
    #[default]
    IncreaseWhenSlow,
    /// Flicks stay responsive: the string shrinks as the pen speeds up.
    ReduceWhenFast,
}

/// Everything in CSP's Correction group that is not the stabilizer slider,
/// as a per-sub-tool value (CSP keeps these with the sub tool, not globally).
///
/// **Every field defaults to off**, and with the default value the whole stage
/// is a byte-for-byte passthrough — a brush preset from before this existed
/// draws exactly as it did (`default_cfg_is_an_exact_passthrough`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CorrectCfg {
    /// `C-031` post correction, 0..1. 0 = off.
    pub post: f32,
    /// `C-032` post-correction window grows with pen speed.
    pub post_by_speed: bool,
    /// `C-033` post-correction window is held constant in SCREEN px, so a
    /// line drawn zoomed out is corrected as hard as one drawn zoomed in.
    pub post_by_scale: bool,
    /// `C-027` a corner sharper than [`SHARP_ANGLE_DEG`] is never smoothed
    /// across, so a deliberate angle survives the correction.
    pub sharp: bool,
    /// `C-029` the pull-string length tracks pen speed.
    pub stab_by_speed: bool,
    /// `C-030` which way it tracks. Only read when `stab_by_speed`.
    pub stab_mode: StabMode,
    /// `S-024` how the two lengths below are interpreted.
    pub se_how: SeHow,
    /// `S-025` entry shaping length in canvas px. 0 = off.
    pub start_px: f32,
    /// `S-026` exit shaping length in canvas px. 0 = off.
    ///
    /// **Costs latency by construction**: the last `end_px` of a stroke cannot
    /// be shaped until the pen lifts, so that much ink is held back and lands
    /// on release. `SeHow::Fade` has no such cost — it runs from the start.
    pub end_px: f32,
    /// `S-023` the Minimum the ramps run from/to, 0..1 of pressure.
    pub se_min: f32,
    /// `S-027` a slow stroke gets a shorter (weaker) ramp.
    pub se_by_speed: bool,
}

impl Default for CorrectCfg {
    fn default() -> Self {
        Self {
            post: 0.0,
            post_by_speed: false,
            post_by_scale: false,
            sharp: false,
            stab_by_speed: false,
            stab_mode: StabMode::IncreaseWhenSlow,
            se_how: SeHow::Length,
            start_px: 0.0,
            end_px: 0.0,
            se_min: 0.0,
            se_by_speed: false,
        }
    }
}

impl CorrectCfg {
    /// Clamp every dial into range. Command dispatch calls this so a bad
    /// value from the UI or a future file format cannot produce a NaN window.
    pub fn sanitized(mut self) -> Self {
        self.post = if self.post.is_finite() {
            self.post.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.start_px = if self.start_px.is_finite() {
            self.start_px.clamp(0.0, MAX_SE_PX)
        } else {
            0.0
        };
        self.end_px = if self.end_px.is_finite() {
            self.end_px.clamp(0.0, MAX_SE_PX)
        } else {
            0.0
        };
        self.se_min = if self.se_min.is_finite() {
            self.se_min.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self
    }

    /// True when the stage changes anything at all. False = exact passthrough.
    fn shapes_anything(&self) -> bool {
        self.post > 0.0 || self.start_px > 0.0 || self.end_px > 0.0
    }
}

// --- the taste constants ---------------------------------------------------
//
// EVERY constant in this block is a GUESS. CSP exposes these dials as
// unitless 0-100 sliders and does not publish what they map onto, so these are
// CSP-*shaped* starting points chosen to be visible-but-not-silly, not
// measured against CSP. They are collected here on purpose: this is the short
// list to hand the owner after a pen test.

/// Post-correction window at strength 1.0, in canvas px — the arc length on
/// EACH side of a sample that gets averaged into it. Also the buffer depth,
/// so it is the latency cost of post correction. **GUESS.**
pub const MAX_POST_WINDOW_PX: f32 = 32.0;
/// Pen speed, in SCREEN px per ms, that counts as "full speed" for every
/// by-speed dial. Screen and not canvas px: "fast" is a property of the hand,
/// not of the zoom. ~1.5 px/ms is a brisk 1500 px/s flick. **GUESS.**
const SPEED_REF_PX_PER_MS: f32 = 1.5;
/// Post-correction window multiplier at full speed, with `post_by_speed`.
/// **GUESS**, and so is the DIRECTION: we smooth a fast stroke *more*, on the
/// reading that a fast hand is the inaccurate one. **GUESS.**
const POST_SPEED_BOOST: f32 = 2.0;
/// `IncreaseWhenSlow`: string-length multiplier at a standstill. **GUESS.**
const STAB_SLOW_BOOST: f32 = 2.0;
/// `ReduceWhenFast`: string-length multiplier at full speed. **GUESS.**
const STAB_FAST_CUT: f32 = 0.25;
/// A turn sharper than this is a corner the user meant (`C-027`). CSP ships
/// this as a bare toggle with no threshold, so the number is ours. **GUESS.**
pub const SHARP_ANGLE_DEG: f32 = 60.0;
/// Arc-length baseline the turn angle is measured over. Too small and every
/// bit of tremor reads as a corner; too large and a real corner is missed.
/// **GUESS.**
const CORNER_BASE_PX: f32 = 6.0;
/// `S-027`: ramp-length multiplier for a stroke started/ended at a standstill.
/// **GUESS.**
const SE_SLOW_TAPER: f32 = 0.25;
/// Bounds on the 1/zoom factor of `post_by_scale`, so 800% does not switch
/// correction off and 5% does not turn a line into a noodle. **GUESS.**
const SCALE_FACTOR_MIN: f32 = 0.25;
const SCALE_FACTOR_MAX: f32 = 4.0;
/// Ceiling on the entry/exit lengths. The owner's Real G-Pen carries a 217 px
/// CSP entry taper, so the range has to comfortably clear that.
pub const MAX_SE_PX: f32 = 500.0;

/// One buffered sample plus what the correction needs to know about it.
#[derive(Clone, Copy, Debug)]
struct Node {
    p: PenSample,
    /// Arc length from the first sample of the stroke, canvas px.
    arc: f32,
    /// Smoothing half-window for THIS sample (speed/scale already applied).
    w: f32,
    /// Pen speed at this sample in screen px/ms; `None` when the timestamps
    /// carry no usable delta (coincident samples, and every synthetic test
    /// that does not bother with time).
    speed: Option<f32>,
    /// Turn angle in degrees over `CORNER_BASE_PX`, once enough of the stroke
    /// exists on both sides to measure it.
    turn: Option<f32>,
}

/// Post correction (`C-031`) and entry/exit shaping (`S-023`–`S-027`) as a
/// plain sample filter: push raw samples, take smoothed ones back.
///
/// Not a `StrokeSink` — it needs no `Document`, which is what makes it
/// testable with nothing but a list of coordinates. [`Stabilizer`] owns one
/// and feeds it; the app never sees it directly.
///
/// # How a look-ahead can be "post"
///
/// A sample is held until the pen is at least `lag_px` further along the
/// stroke, and only then smoothed — by which time every neighbour inside its
/// own window has already arrived. The result is identical to smoothing the
/// finished stroke offline, for every sample except the last `lag_px` worth,
/// which `finish` handles with the true stroke length in hand. The cost is
/// that ink trails the pen by `lag_px`; the benefit over the pull-string is
/// that the shape is centred (a curve is not cut short, a corner is not
/// rounded) and nothing is ever swallowed.
#[derive(Clone, Debug)]
pub struct PostCorrect {
    cfg: CorrectCfg,
    /// Canvas zoom at stroke start, for `post_by_scale` and the speed unit.
    zoom: f32,
    /// Buffered samples, oldest first. Pruned from the front once a sample is
    /// too far behind to be anyone's neighbour.
    buf: Vec<Node>,
    /// Index into `buf` of the first sample not yet handed on.
    done: usize,
    /// Index of the first sample whose `turn` has been resolved.
    turned: usize,
    /// Arc length of the newest buffered sample.
    head: f32,
    /// Speed at the first sample that had one, for `S-027` on the entry ramp.
    start_speed: Option<f32>,
}

impl Default for PostCorrect {
    fn default() -> Self {
        Self::new()
    }
}

impl PostCorrect {
    pub fn new() -> Self {
        Self {
            cfg: CorrectCfg::default(),
            zoom: 1.0,
            buf: Vec::new(),
            done: 0,
            turned: 0,
            head: 0.0,
            start_speed: None,
        }
    }

    pub fn cfg(&self) -> CorrectCfg {
        self.cfg
    }

    /// Takes effect on the next stroke — `reset` is what actually adopts it,
    /// so a slider dragged mid-stroke cannot change the buffer depth under a
    /// half-emitted sample.
    pub fn set_cfg(&mut self, cfg: CorrectCfg) {
        self.cfg = cfg.sanitized();
    }

    /// Canvas zoom, for `C-033` and for reading speed in screen px.
    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = if zoom.is_finite() && zoom > 0.0 {
            zoom
        } else {
            1.0
        };
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.done = 0;
        self.turned = 0;
        self.head = 0.0;
        self.start_speed = None;
    }

    /// The 1/zoom factor `post_by_scale` applies (1.0 when it is off).
    fn scale_factor(&self) -> f32 {
        if self.cfg.post_by_scale {
            (1.0 / self.zoom).clamp(SCALE_FACTOR_MIN, SCALE_FACTOR_MAX)
        } else {
            1.0
        }
    }

    /// Largest window any sample of this stroke can ask for — the bound the
    /// buffer depth and the pruning are both sized from.
    fn w_max(&self) -> f32 {
        if self.cfg.post <= 0.0 {
            return 0.0;
        }
        let mut w = self.cfg.post * MAX_POST_WINDOW_PX * self.scale_factor();
        if self.cfg.post_by_speed {
            w *= POST_SPEED_BOOST;
        }
        w
    }

    /// How far behind the pen a sample must fall before it can be emitted.
    fn lag_px(&self) -> f32 {
        let mut lag = self.w_max();
        if self.cfg.sharp && lag > 0.0 {
            // Corner flags inside the window need their own look-ahead.
            lag += CORNER_BASE_PX;
        }
        if self.cfg.se_how == SeHow::Length && self.cfg.end_px > 0.0 {
            // The exit ramp cannot be applied without the stroke's length.
            lag = lag.max(self.cfg.end_px);
        }
        lag
    }

    /// This sample's own window, after the speed dial.
    fn window_for(&self, speed: Option<f32>) -> f32 {
        if self.cfg.post <= 0.0 {
            return 0.0;
        }
        let mut w = self.cfg.post * MAX_POST_WINDOW_PX * self.scale_factor();
        if self.cfg.post_by_speed {
            if let Some(f) = speed_factor(speed) {
                w *= 1.0 + (POST_SPEED_BOOST - 1.0) * f;
            }
        }
        w
    }

    /// Feed one sample; returns the samples ready to be drawn now.
    pub fn push(&mut self, s: PenSample) -> Vec<PenSample> {
        if !self.cfg.shapes_anything() {
            // Nothing configured: the sample goes on untouched, bit for bit.
            return vec![s];
        }
        if !(s.x.is_finite() && s.y.is_finite()) {
            return Vec::new();
        }
        let (arc, speed) = match self.buf.last() {
            None => (0.0, None),
            Some(prev) => {
                let d = (s.x - prev.p.x).hypot(s.y - prev.p.y);
                let d = if d.is_finite() { d } else { 0.0 };
                (prev.arc + d, screen_speed(prev.p, s, d, self.zoom))
            }
        };
        if self.start_speed.is_none() {
            self.start_speed = speed;
        }
        self.head = arc;
        let w = self.window_for(speed);
        self.buf.push(Node {
            p: s,
            arc,
            w,
            speed,
            turn: None,
        });
        self.resolve_turns();

        let lag = self.lag_px();
        let mut out = Vec::new();
        while self.done < self.buf.len() && self.head - self.buf[self.done].arc >= lag {
            out.push(self.shaped(self.done, None));
            self.done += 1;
        }
        self.prune();
        out
    }

    /// The pen lifted: the stroke's true length is known, so the tail can be
    /// smoothed with a window that shrinks into the endpoint and the exit ramp
    /// can finally be applied. Leaves the filter reset for the next stroke.
    pub fn finish(&mut self) -> Vec<PenSample> {
        if !self.cfg.shapes_anything() {
            self.reset();
            return Vec::new();
        }
        let total = self.head;
        let end_speed = self.buf.last().and_then(|n| n.speed);
        let mut out = Vec::new();
        while self.done < self.buf.len() {
            out.push(self.shaped_with(self.done, Some(total), end_speed));
            self.done += 1;
        }
        self.reset();
        out
    }

    /// Fill in `turn` for every buffered sample that now has `CORNER_BASE_PX`
    /// of stroke on both sides of it.
    fn resolve_turns(&mut self) {
        if !self.cfg.sharp {
            return;
        }
        while self.turned < self.buf.len() {
            let i = self.turned;
            if self.head - self.buf[i].arc < CORNER_BASE_PX {
                break;
            }
            self.buf[i].turn = self.measure_turn(i);
            self.turned += 1;
        }
    }

    /// Angle in degrees between the run-in and the run-out of sample `i`,
    /// measured over `CORNER_BASE_PX` of arc on each side. `None` when the
    /// buffer does not reach that far — the ends of a stroke are pinned by the
    /// shrinking window anyway, so they never need a corner flag.
    fn measure_turn(&self, i: usize) -> Option<f32> {
        let n = self.buf[i];
        let before = self.buf[..i]
            .iter()
            .rev()
            .find(|m| n.arc - m.arc >= CORNER_BASE_PX)?;
        let after = self.buf[i + 1..]
            .iter()
            .find(|m| m.arc - n.arc >= CORNER_BASE_PX)?;
        let (ax, ay) = (n.p.x - before.p.x, n.p.y - before.p.y);
        let (bx, by) = (after.p.x - n.p.x, after.p.y - n.p.y);
        let (la, lb) = (ax.hypot(ay), bx.hypot(by));
        if !(la > 1e-4 && lb > 1e-4) {
            return None;
        }
        let cos = ((ax * bx + ay * by) / (la * lb)).clamp(-1.0, 1.0);
        Some(cos.acos().to_degrees())
    }

    /// Is `i` a corner the user drew on purpose? Requires the turn to clear
    /// the threshold AND to be the sharpest point in its own neighbourhood —
    /// without the local-maximum test a gentle 90° bend registers as twenty
    /// corners in a row and blocks all smoothing through it.
    fn is_corner(&self, i: usize) -> bool {
        let Some(t) = self.buf[i].turn else {
            return false;
        };
        if t < SHARP_ANGLE_DEG {
            return false;
        }
        let a = self.buf[i].arc;
        let sharper = |m: &Node| m.turn > Some(t);
        !self.buf[..i]
            .iter()
            .rev()
            .take_while(|m| a - m.arc <= CORNER_BASE_PX)
            .any(sharper)
            && !self.buf[i + 1..]
                .iter()
                .take_while(|m| m.arc - a <= CORNER_BASE_PX)
                .any(sharper)
    }

    /// Drop samples that can no longer be a neighbour of anything unemitted.
    fn prune(&mut self) {
        if self.done == 0 {
            return;
        }
        let keep_from = self.buf[self.done.min(self.buf.len() - 1)].arc - self.w_max();
        let drop = self.buf[..self.done]
            .iter()
            .take_while(|n| n.arc < keep_from)
            .count();
        if drop > 0 {
            self.buf.drain(..drop);
            self.done -= drop;
            self.turned = self.turned.saturating_sub(drop);
        }
    }

    fn shaped(&self, i: usize, total: Option<f32>) -> PenSample {
        self.shaped_with(i, total, None)
    }

    /// The finished sample: smoothed position, shaped pressure.
    fn shaped_with(&self, i: usize, total: Option<f32>, end_speed: Option<f32>) -> PenSample {
        let n = self.buf[i];
        let (x, y) = self.smoothed(i, total);
        let f = self.shape_factor(n.arc, total, end_speed);
        PenSample {
            x,
            y,
            pressure: n.p.pressure * f,
            ..n.p
        }
    }

    /// Tent-weighted average of the neighbours within this sample's window,
    /// renormalized over the neighbours that exist — near the stroke ends
    /// the window is one-sided, NOT shrunk.
    ///
    /// The old symmetric shrink (`w.min(arc).min(total − arc)`) pinned the
    /// first and last samples verbatim and left the first/last window-length
    /// of a stroke essentially uncorrected: with a ±3 px tremor and a 32 px
    /// window, the ends kept the full wobble band while the interior
    /// flattened — the r69–r115 audit's "post correction is a no-op through
    /// the real brush chain" red test, since the painted band is set by its
    /// extremes. One-sided averaging corrects pen-down/pen-up tremor like
    /// any other tremor; the tent's own d = 0 weight still biases the ends
    /// strongly toward where the pen actually touched (they move by at most
    /// about the tremor amplitude at full strength).
    fn smoothed(&self, i: usize, total: Option<f32>) -> (f32, f32) {
        let n = self.buf[i];
        let mut w = n.w;
        if let Some(t) = total {
            // A stroke shorter than the window smooths LOCALLY instead of
            // averaging itself into its own centroid — a deliberate short
            // flick must stay a flick.
            w = w.min(t * 0.5);
        }
        if self.cfg.sharp && w > 0.0 {
            if self.is_corner(i) {
                return (n.p.x, n.p.y);
            }
            for k in (0..i).rev() {
                let d = n.arc - self.buf[k].arc;
                if d > w {
                    break;
                }
                if self.is_corner(k) {
                    w = d;
                    break;
                }
            }
            for k in i + 1..self.buf.len() {
                let d = self.buf[k].arc - n.arc;
                if d > w {
                    break;
                }
                if self.is_corner(k) {
                    w = d;
                    break;
                }
            }
        }
        if w <= 0.0 {
            return (n.p.x, n.p.y);
        }
        let (mut sx, mut sy, mut sw) = (0.0f32, 0.0f32, 0.0f32);
        let (mut lo, mut hi) = (n, n);
        for m in &self.buf {
            let d = (m.arc - n.arc).abs();
            if d > w {
                continue;
            }
            let k = 1.0 - d / w;
            sx += m.p.x * k;
            sy += m.p.y * k;
            sw += k;
            if m.arc < lo.arc {
                lo = *m;
            }
            if m.arc > hi.arc {
                hi = *m;
            }
        }
        if sw <= 0.0 {
            return (n.p.x, n.p.y);
        }
        let (mut ox, mut oy) = (sx / sw, sy / sw);
        // End-shortening guard. A one-sided window pulls the average INWARD
        // along the stroke — unguarded, the corrected ink started ~6 px
        // late and stopped short. The correction's along-tangent component
        // is removed in proportion to how one-sided the window is (fully at
        // the endpoints, not at all one window-length in); the
        // PERPENDICULAR component — the tremor, the thing being corrected —
        // is kept everywhere, ends included.
        let end_d = match total {
            Some(t) => n.arc.min(t - n.arc),
            // Streaming: the look-ahead guarantees a full right side, so
            // only the distance to the start can be short.
            None => n.arc,
        };
        let onesided = 1.0 - (end_d / w).clamp(0.0, 1.0);
        if onesided > 0.0 {
            let (tx, ty) = (hi.p.x - lo.p.x, hi.p.y - lo.p.y);
            let len = tx.hypot(ty);
            if len > 1e-4 {
                let (ux, uy) = (tx / len, ty / len);
                let along = (ox - n.p.x) * ux + (oy - n.p.y) * uy;
                ox -= ux * along * onesided;
                oy -= uy * along * onesided;
            }
        }
        (ox, oy)
    }

    /// Ramp length after `S-027`. A stroke begun or ended at a standstill gets
    /// a shorter ramp; no timestamps means no modulation at all.
    fn se_len(&self, px: f32, speed: Option<f32>) -> f32 {
        if px <= 0.0 || !self.cfg.se_by_speed {
            return px;
        }
        match speed_factor(speed) {
            Some(f) => px * (SE_SLOW_TAPER + (1.0 - SE_SLOW_TAPER) * f),
            None => px,
        }
    }

    /// The pressure multiplier at this arc length (`S-023`–`S-026`).
    fn shape_factor(&self, arc: f32, total: Option<f32>, end_speed: Option<f32>) -> f32 {
        let m = self.cfg.se_min;
        match self.cfg.se_how {
            SeHow::Fade => {
                let l = self.se_len(self.cfg.end_px, self.start_speed);
                if l <= 0.0 {
                    return 1.0;
                }
                1.0 - (1.0 - m) * (arc / l).clamp(0.0, 1.0)
            }
            SeHow::Length => {
                let mut f = 1.0f32;
                let ls = self.se_len(self.cfg.start_px, self.start_speed);
                if ls > 0.0 {
                    f = f.min(m + (1.0 - m) * (arc / ls).clamp(0.0, 1.0));
                }
                if let Some(t) = total {
                    let le = self.se_len(self.cfg.end_px, end_speed);
                    if le > 0.0 {
                        f = f.min(m + (1.0 - m) * ((t - arc) / le).clamp(0.0, 1.0));
                    }
                }
                f
            }
        }
    }
}

/// Speed between two samples in SCREEN px per ms, or `None` when the
/// timestamps carry no usable delta. `None` means "no speed information", and
/// every by-speed dial treats it as "do not modulate" rather than as "slow" —
/// otherwise a synthetic stroke with no clock would silently get a different
/// brush than a real one.
fn screen_speed(prev: PenSample, s: PenSample, dist: f32, zoom: f32) -> Option<f32> {
    let dt = s.t_ms - prev.t_ms;
    if !(dt.is_finite() && dt > 0.0) || !dist.is_finite() {
        return None;
    }
    Some(dist * zoom / dt as f32)
}

/// Speed as 0..1 of [`SPEED_REF_PX_PER_MS`].
fn speed_factor(speed: Option<f32>) -> Option<f32> {
    speed.map(|v| (v / SPEED_REF_PX_PER_MS).clamp(0.0, 1.0))
}

/// Pull-string smoothing wrapper around any [`StrokeSink`], and the host of
/// the rest of the correction stage ([`PostCorrect`]).
///
/// The post stage is composed in rather than stacked as another decorator on
/// purpose: the app's brush chain is spelled `Stabilizer<Taper<Engine>>` and
/// reaches the engine through `inner_mut().inner_mut()` in dozens of places,
/// so a fourth layer would have been a rename of every one of those.
#[derive(Clone, Debug)]
pub struct Stabilizer<S> {
    inner: S,
    /// String length in canvas px. 0 == passthrough.
    radius: f32,
    /// Current brush position (the end of the string we emit).
    anchor: Option<(f32, f32)>,
    /// Newest raw sample, kept so `end()` can drain towards it.
    last_in: Option<PenSample>,
    /// Previous raw sample, for the pull-string's own speed estimate.
    prev: Option<PenSample>,
    /// The rest of the Correction group.
    post: PostCorrect,
}

impl<S> Stabilizer<S> {
    /// `strength` is 0..1 and maps linearly to a string length of
    /// `strength * MAX_STRING_PX`. **0 is an exact passthrough**: every sample
    /// reaches the inner sink verbatim, byte for byte.
    pub fn new(inner: S, strength: f32) -> Self {
        Self {
            inner,
            radius: strength.clamp(0.0, 1.0) * MAX_STRING_PX,
            anchor: None,
            last_in: None,
            prev: None,
            post: PostCorrect::new(),
        }
    }

    /// Construct from an explicit string length in canvas pixels.
    pub fn with_radius_px(inner: S, radius_px: f32) -> Self {
        Self {
            inner,
            radius: radius_px.max(0.0),
            anchor: None,
            last_in: None,
            prev: None,
            post: PostCorrect::new(),
        }
    }

    /// The rest of the Correction group (`C-027`–`C-033`, `S-023`–`S-027`).
    /// Like `set_strength`, this lands on the next stroke.
    pub fn set_correction(&mut self, cfg: CorrectCfg) {
        self.post.set_cfg(cfg);
    }

    pub fn correction(&self) -> CorrectCfg {
        self.post.cfg()
    }

    /// Canvas zoom for `C-033` and for reading pen speed in screen px. The app
    /// sets this at stroke start, next to the stabilizer's own strength.
    pub fn set_zoom(&mut self, zoom: f32) {
        self.post.set_zoom(zoom);
    }

    /// Current strength, 0..1.
    pub fn strength(&self) -> f32 {
        self.radius / MAX_STRING_PX
    }

    /// Change strength mid-session. Takes effect on the next stroke segment; it
    /// does not teleport the brush.
    pub fn set_strength(&mut self, strength: f32) {
        self.radius = strength.clamp(0.0, 1.0) * MAX_STRING_PX;
    }

    /// String length in canvas pixels.
    pub fn radius_px(&self) -> f32 {
        self.radius
    }

    pub fn inner(&self) -> &S {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: StrokeSink> Stabilizer<S> {
    /// Everything the pull-string lets past goes through the post stage before
    /// it reaches the brush.
    fn emit(&mut self, doc: &mut Document, s: PenSample) {
        let out = self.post.push(s);
        for p in out {
            self.inner.sample(doc, p);
        }
    }

    /// The string length for this sample after `C-029`/`C-030`. Unmodulated
    /// when the dial is off or the samples carry no usable timestamps.
    fn string_radius(&self, speed: Option<f32>) -> f32 {
        if !self.post.cfg.stab_by_speed {
            return self.radius;
        }
        let Some(f) = speed_factor(speed) else {
            return self.radius;
        };
        match self.post.cfg.stab_mode {
            StabMode::IncreaseWhenSlow => self.radius * (1.0 + (STAB_SLOW_BOOST - 1.0) * (1.0 - f)),
            StabMode::ReduceWhenFast => self.radius * (1.0 - (1.0 - STAB_FAST_CUT) * f),
        }
    }
}

impl<S: StrokeSink> StrokeSink for Stabilizer<S> {
    fn begin(&mut self, doc: &mut Document) {
        self.anchor = None;
        self.last_in = None;
        self.prev = None;
        self.post.reset();
        self.inner.begin(doc);
    }

    fn sample(&mut self, doc: &mut Document, s: PenSample) {
        let speed = match self.prev {
            Some(prev) => {
                let d = (s.x - prev.x).hypot(s.y - prev.y);
                screen_speed(prev, s, d, self.post.zoom)
            }
            None => None,
        };
        self.prev = Some(s);
        let radius = self.string_radius(speed);

        if radius <= 0.0 {
            self.last_in = Some(s);
            self.emit(doc, s);
            return;
        }

        self.last_in = Some(s);

        let Some(anchor) = self.anchor else {
            // The stroke starts under the pen; only later samples lag.
            self.anchor = Some((s.x, s.y));
            self.emit(doc, s);
            return;
        };

        let (dx, dy) = (s.x - anchor.0, s.y - anchor.1);
        let dist = (dx * dx + dy * dy).sqrt();
        if !dist.is_finite() || dist <= radius {
            // String still slack: the brush does not move, so nothing is drawn.
            return;
        }

        // Drag the anchor along the pen direction until it is exactly `radius`
        // behind the pen.
        let k = (dist - radius) / dist;
        let next = (anchor.0 + dx * k, anchor.1 + dy * k);
        self.anchor = Some(next);
        self.emit(
            doc,
            PenSample {
                x: next.0,
                y: next.1,
                ..s
            },
        );
    }

    fn end(&mut self, doc: &mut Document) {
        // Drain: walk the remaining string so the stroke does not stop short of
        // where the pen was actually lifted.
        if let (Some(anchor), Some(last)) = (self.anchor, self.last_in) {
            let (dx, dy) = (last.x - anchor.0, last.y - anchor.1);
            let dist = (dx * dx + dy * dy).sqrt();
            if dist.is_finite() && dist > 0.0 {
                let steps = ((dist / DRAIN_STEP_PX).ceil() as u32).clamp(1, DRAIN_MAX_STEPS);
                for i in 1..=steps {
                    let s = if i == steps {
                        // Land on the raw endpoint exactly, not on a lerp that
                        // is one ULP off.
                        PenSample {
                            x: last.x,
                            y: last.y,
                            ..last
                        }
                    } else {
                        let t = i as f32 / steps as f32;
                        PenSample {
                            x: anchor.0 + dx * t,
                            y: anchor.1 + dy * t,
                            ..last
                        }
                    };
                    self.emit(doc, s);
                }
            }
        }
        // The post stage's own tail: the samples still inside its look-ahead
        // buffer, now that the stroke's true length is known. Must run before
        // `inner.end`, exactly like the drain above — the brush closes its
        // stroke there and anything after it would land in the next one.
        let tail = self.post.finish();
        for p in tail {
            self.inner.sample(doc, p);
        }
        self.anchor = None;
        self.last_in = None;
        self.prev = None;
        self.inner.end(doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;

    /// Sink that records what it was handed. The test double for "the brush".
    #[derive(Default)]
    struct Recorder {
        begins: usize,
        ends: usize,
        got: Vec<PenSample>,
    }

    impl StrokeSink for Recorder {
        fn begin(&mut self, _doc: &mut Document) {
            self.begins += 1;
        }
        fn sample(&mut self, _doc: &mut Document, s: PenSample) {
            self.got.push(s);
        }
        fn end(&mut self, _doc: &mut Document) {
            self.ends += 1;
        }
    }

    fn s(x: f32, y: f32) -> PenSample {
        PenSample {
            x,
            y,
            pressure: 0.5,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: 0.0,
        }
    }

    fn run(strength: f32, input: &[PenSample]) -> Recorder {
        let mut doc = Document::new(64, 64);
        let mut st = Stabilizer::new(Recorder::default(), strength);
        st.begin(&mut doc);
        for &p in input {
            st.sample(&mut doc, p);
        }
        st.end(&mut doc);
        st.into_inner()
    }

    fn line() -> Vec<PenSample> {
        (0..=40).map(|i| s(i as f32 * 5.0, 100.0)).collect()
    }

    #[test]
    fn strength_zero_is_an_exact_passthrough() {
        let input = line();
        let out = run(0.0, &input);
        assert_eq!(out.begins, 1);
        assert_eq!(out.ends, 1);
        assert_eq!(out.got, input, "strength 0 must not touch the samples");
    }

    #[test]
    fn smoothing_is_deterministic() {
        let input = line();
        let a = run(0.5, &input);
        let b = run(0.5, &input);
        assert_eq!(a.got, b.got);
        assert!(!a.got.is_empty());
    }

    #[test]
    fn the_brush_lags_by_the_string_length() {
        let input = line();
        let out = run(0.5, &input); // radius = 24 px
        // While the pen runs ahead, every emitted point (except the drain tail)
        // trails it. Check the point emitted for the last input sample.
        let pen_end = input.last().unwrap().x;
        // Second-to-last emitted sample is still mid-drag, i.e. behind the pen.
        let mid = out.got[out.got.len() / 2];
        assert!(mid.x < pen_end, "brush should trail the pen");
        assert!(mid.x > 0.0);
    }

    #[test]
    fn end_drains_the_string_to_the_last_raw_sample() {
        let input = line();
        let out = run(0.6, &input);
        let last_in = *input.last().unwrap();
        let last_out = *out.got.last().unwrap();
        assert_eq!(
            (last_out.x, last_out.y),
            (last_in.x, last_in.y),
            "stroke must not fall short of where the pen was lifted"
        );
        // The drain is stepped, not one jump.
        let trailing = out
            .got
            .iter()
            .rev()
            .take_while(|p| p.x > last_in.x - MAX_STRING_PX)
            .count();
        assert!(trailing >= 4, "drain should be subdivided, got {trailing}");
    }

    #[test]
    fn tremor_inside_the_string_emits_nothing() {
        // Jitter within the radius: the brush must stay put (no dabs at all
        // beyond the first), which is the whole point of the filter.
        let mut input = vec![s(100.0, 100.0)];
        for i in 0..20 {
            let d = if i % 2 == 0 { 1.5 } else { -1.5 };
            input.push(s(100.0 + d, 100.0 + d));
        }
        let out = run(0.5, &input);
        // One initial sample + the drain tail; nothing from the jitter itself.
        assert!(out.got.len() < input.len(), "jitter leaked through");
        assert_eq!(out.got[0], input[0]);
    }

    #[test]
    fn strength_maps_to_pixels() {
        let st = Stabilizer::new(Recorder::default(), 0.5);
        assert!((st.radius_px() - MAX_STRING_PX * 0.5).abs() < 1e-6);
        assert!((st.strength() - 0.5).abs() < 1e-6);
        let st = Stabilizer::with_radius_px(Recorder::default(), 12.0);
        assert!((st.radius_px() - 12.0).abs() < 1e-6);
    }

    // ---------------------------------------------------------- correction --
    //
    // There is no pen in a test, so every case below is a scripted path fed
    // through the filter and judged on the GEOMETRY that comes out —
    // `MOUSE_PRESSURE`'s reason for existing, applied to the correction stage.

    fn t(x: f32, y: f32, t_ms: f64) -> PenSample {
        PenSample {
            x,
            y,
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms,
        }
    }

    /// Push a whole path through the post stage and collect everything it
    /// hands back, streaming emissions and the `finish` tail alike.
    fn post(cfg: CorrectCfg, zoom: f32, input: &[PenSample]) -> Vec<PenSample> {
        let mut pc = PostCorrect::new();
        pc.set_cfg(cfg);
        pc.set_zoom(zoom);
        let mut out = Vec::new();
        for &p in input {
            out.extend(pc.push(p));
        }
        out.extend(pc.finish());
        out
    }

    /// A dead-straight line at y = 0 with a deliberate ±1.5 px tremor on it,
    /// one sample per px. What a correction is for.
    fn wobble(dt_ms: f64) -> Vec<PenSample> {
        (0..200)
            .map(|i| {
                let y = if (i / 3) % 2 == 0 { 1.5 } else { -1.5 };
                t(i as f32, y, i as f64 * dt_ms)
            })
            .collect()
    }

    /// Worst deviation from the true line, ignoring the ends (where the
    /// window shrinks into the endpoints by design).
    fn residual(out: &[PenSample]) -> f32 {
        out.iter()
            .filter(|p| p.x > 60.0 && p.x < 140.0)
            .fold(0.0f32, |m, p| m.max(p.y.abs()))
    }

    #[test]
    fn default_cfg_is_an_exact_passthrough() {
        // The promise the round is allowed to ship on: a preset saved before
        // any of this existed draws EXACTLY as it did.
        let input = wobble(4.0);
        let out = post(CorrectCfg::default(), 1.0, &input);
        assert_eq!(out, input, "the default correction must not touch a stroke");
    }

    #[test]
    fn post_correction_flattens_a_wobble_and_keeps_every_sample() {
        let input = wobble(4.0);
        let raw = residual(&post(CorrectCfg::default(), 1.0, &input));
        let cfg = CorrectCfg {
            post: 1.0,
            ..Default::default()
        };
        let out = post(cfg, 1.0, &input);
        assert!((raw - 1.5).abs() < 1e-4, "uncorrected tremor should be 1.5");
        assert!(
            residual(&out) < 0.4,
            "post correction should flatten the tremor, got {}",
            residual(&out)
        );
        // Unlike the pull-string, nothing is ever swallowed: the dead zone is
        // exactly what makes stabilization eat slow detail work.
        assert_eq!(out.len(), input.len(), "post correction dropped samples");
    }

    /// The ends stay NEAR where the pen touched/lifted — biased there by
    /// the tent's own d = 0 weight — but they are corrected like every
    /// other sample: pen-down tremor is tremor. (They were pinned VERBATIM
    /// once, via a window that shrank to the end distance; that left a
    /// whole window-length of stroke uncorrected at each end, which is the
    /// audit's "post correction is a no-op through the real brush chain"
    /// finding — the painted band is set by its extremes.)
    #[test]
    fn post_correction_keeps_the_ends_near_the_pen() {
        let input = wobble(4.0);
        let cfg = CorrectCfg {
            post: 1.0,
            ..Default::default()
        };
        let out = post(cfg, 1.0, &input);
        let (a, b) = (out.first().unwrap(), out.last().unwrap());
        let last = input.last().unwrap();
        // Within the tremor amplitude (the wobble's residual is 1.5), at
        // FULL strength — the ends may not wander further than the noise
        // the correction exists to remove.
        for (got, want) in [(a, &input[0]), (b, last)] {
            let d = (got.x - want.x).hypot(got.y - want.y);
            assert!(
                d <= 1.5,
                "an end moved {d} px from the pen (limit: the tremor amplitude)"
            );
            assert!(d > 0.0, "ends should be corrected too, not pinned verbatim");
        }
    }

    /// An L: 40 px right, then 40 px down. The corner is the whole point.
    fn corner_path() -> Vec<PenSample> {
        let mut v: Vec<PenSample> = (0..40).map(|i| t(i as f32, 0.0, i as f64 * 4.0)).collect();
        v.extend((1..=40).map(|i| t(39.0, i as f32, (39 + i) as f64 * 4.0)));
        v
    }

    fn nearest_to_corner(out: &[PenSample]) -> f32 {
        out.iter()
            .map(|p| (p.x - 39.0).hypot(p.y))
            .fold(f32::MAX, f32::min)
    }

    #[test]
    fn sharp_angles_keeps_a_corner_that_smoothing_would_round_off() {
        let path = corner_path();
        let round = CorrectCfg {
            post: 1.0,
            ..Default::default()
        };
        let keep = CorrectCfg {
            post: 1.0,
            sharp: true,
            ..Default::default()
        };
        let rounded = nearest_to_corner(&post(round, 1.0, &path));
        let kept = nearest_to_corner(&post(keep, 1.0, &path));
        assert!(
            rounded > 3.0,
            "without C-027 a 90° corner must visibly round off, got {rounded}"
        );
        assert!(
            kept < 0.01,
            "with C-027 the corner sample must survive verbatim, got {kept}"
        );
    }

    /// A straight line with ONE sample kicked 6 px off it.
    ///
    /// The probe for "did the window get wider?", and deliberately not the
    /// tremor above: a tent average attenuates a LONE spike strictly
    /// monotonically in its window width, while its response to a PERIODIC
    /// signal has nulls — widen that window past one and the wobble can come
    /// back. Only the spike lets a wider window be asserted as such.
    fn spike(dt_ms: f64) -> Vec<PenSample> {
        (0..200)
            .map(|i| t(i as f32, if i == 100 { 6.0 } else { 0.0 }, i as f64 * dt_ms))
            .collect()
    }

    fn peak(out: &[PenSample]) -> f32 {
        out.iter().fold(0.0f32, |m, p| m.max(p.y.abs()))
    }

    #[test]
    fn adjust_by_scale_corrects_harder_when_zoomed_out() {
        // The nine-year complaint: at fit-to-screen the same hand tremor is
        // worth four times as many document px, so a window measured in
        // document px under-corrects exactly where it is needed most.
        let input = spike(4.0);
        let off = CorrectCfg {
            post: 0.25,
            ..Default::default()
        };
        let on = CorrectCfg {
            post_by_scale: true,
            ..off
        };
        let a = peak(&post(off, 0.25, &input));
        let b = peak(&post(on, 0.25, &input));
        assert!(
            b < a * 0.7,
            "C-033 should smooth harder at 25% zoom: {b} vs {a}"
        );
        // And at 100% it changes nothing at all — the factor is 1/zoom.
        let c = post(off, 1.0, &input);
        let d = post(on, 1.0, &input);
        assert_eq!(c, d, "C-033 must be a no-op at 100% zoom");
    }

    #[test]
    fn post_by_speed_smooths_a_fast_stroke_harder() {
        let cfg = CorrectCfg {
            post: 0.3,
            post_by_speed: true,
            ..Default::default()
        };
        let fast = peak(&post(cfg, 1.0, &spike(0.5)));
        let slow = peak(&post(cfg, 1.0, &spike(40.0)));
        assert!(
            fast < slow * 0.7,
            "C-032: the fast stroke should come out smoother ({fast} vs {slow})"
        );
    }

    #[test]
    fn no_timestamps_means_no_speed_modulation() {
        // Every synthetic path in this repo has a flat clock, and mouse
        // reports can repeat one. "No speed information" must mean "leave it
        // alone", never "treat it as a standstill".
        let input: Vec<PenSample> = wobble(0.0).iter().map(|p| t(p.x, p.y, 0.0)).collect();
        let plain = CorrectCfg {
            post: 0.4,
            ..Default::default()
        };
        let by_speed = CorrectCfg {
            post_by_speed: true,
            ..plain
        };
        assert_eq!(post(plain, 1.0, &input), post(by_speed, 1.0, &input));
    }

    /// Distance the brush trails the pen at the end of a straight run.
    fn string_lag(cfg: CorrectCfg, step_ms: f64) -> f32 {
        let mut doc = Document::new(64, 64);
        let mut st = Stabilizer::new(Recorder::default(), 1.0); // 48 px string
        st.set_correction(cfg);
        st.begin(&mut doc);
        let mut last = 0.0;
        for i in 0..=60 {
            last = i as f32 * 5.0;
            st.sample(&mut doc, t(last, 100.0, i as f64 * step_ms));
        }
        last - st.inner().got.last().unwrap().x
    }

    #[test]
    fn stabilization_mode_moves_the_string_the_two_opposite_ways() {
        let off = CorrectCfg::default();
        assert!(
            (string_lag(off, 2.5) - MAX_STRING_PX).abs() < 0.5,
            "unmodulated, the brush trails by the whole string"
        );
        let fast = CorrectCfg {
            stab_by_speed: true,
            stab_mode: StabMode::ReduceWhenFast,
            ..Default::default()
        };
        // 5 px per 2.5 ms = 2 screen px/ms, past the reference speed.
        let lag = string_lag(fast, 2.5);
        assert!(
            lag < MAX_STRING_PX * 0.5,
            "C-030 Reduce when fast should let a flick catch up, got {lag}"
        );
        let slow = CorrectCfg {
            stab_by_speed: true,
            stab_mode: StabMode::IncreaseWhenSlow,
            ..Default::default()
        };
        // 5 px per 100 ms = 0.05 screen px/ms: a careful, detail-speed hand.
        let lag = string_lag(slow, 100.0);
        assert!(
            lag > MAX_STRING_PX * 1.5,
            "C-030 Increase when slow should lengthen the string, got {lag}"
        );
        // And with no clock at all, neither mode may change anything.
        assert!((string_lag(fast, 0.0) - MAX_STRING_PX).abs() < 0.5);
        assert!((string_lag(slow, 0.0) - MAX_STRING_PX).abs() < 0.5);
    }

    fn straight(n: usize) -> Vec<PenSample> {
        (0..n).map(|i| t(i as f32, 0.0, i as f64 * 4.0)).collect()
    }

    #[test]
    fn starting_ramps_up_from_the_minimum() {
        let cfg = CorrectCfg {
            start_px: 50.0,
            se_min: 0.2,
            ..Default::default()
        };
        let out = post(cfg, 1.0, &straight(200));
        assert!((out[0].pressure - 0.2).abs() < 1e-4, "{}", out[0].pressure);
        assert!(
            (out[25].pressure - 0.6).abs() < 1e-3,
            "{}",
            out[25].pressure
        );
        assert!(
            (out[60].pressure - 1.0).abs() < 1e-4,
            "{}",
            out[60].pressure
        );
        // The entry ramp is causal — it must not hold ink back.
        let mut pc = PostCorrect::new();
        pc.set_cfg(cfg);
        assert_eq!(pc.push(t(0.0, 0.0, 0.0)).len(), 1, "entry ramp lagged");
    }

    #[test]
    fn ending_ramps_down_and_holds_the_tail_until_the_pen_lifts() {
        let cfg = CorrectCfg {
            end_px: 50.0,
            se_min: 0.2,
            ..Default::default()
        };
        let path = straight(200);
        let out = post(cfg, 1.0, &path);
        assert_eq!(out.len(), path.len());
        assert!((out[0].pressure - 1.0).abs() < 1e-4);
        assert!((out[100].pressure - 1.0).abs() < 1e-4);
        assert!(
            (out[199].pressure - 0.2).abs() < 1e-4,
            "the tail must reach the minimum, got {}",
            out[199].pressure
        );
        // The documented cost: the last `end_px` cannot exist until the pen
        // lifts, so that much ink arrives on release.
        let mut pc = PostCorrect::new();
        pc.set_cfg(cfg);
        let streamed: usize = path.iter().map(|&p| pc.push(p).len()).sum();
        let held = pc.finish().len();
        assert!(
            held >= 50,
            "the exit ramp must hold back at least end_px of ink, held {held}"
        );
        assert_eq!(streamed + held, path.len());
    }

    #[test]
    fn fade_runs_from_the_start_and_holds_at_the_minimum() {
        let cfg = CorrectCfg {
            se_how: SeHow::Fade,
            end_px: 100.0,
            se_min: 0.3,
            start_px: 40.0, // ignored in Fade, exactly as CSP greys it out
            ..Default::default()
        };
        let out = post(cfg, 1.0, &straight(200));
        assert!((out[0].pressure - 1.0).abs() < 1e-4, "{}", out[0].pressure);
        assert!(
            (out[50].pressure - 0.65).abs() < 1e-3,
            "{}",
            out[50].pressure
        );
        assert!((out[100].pressure - 0.3).abs() < 1e-4);
        assert!((out[199].pressure - 0.3).abs() < 1e-4, "fade must hold");
        // And it costs nothing: Fade never buffers.
        let mut pc = PostCorrect::new();
        pc.set_cfg(cfg);
        assert_eq!(pc.push(t(0.0, 0.0, 0.0)).len(), 1);
    }

    #[test]
    fn starting_and_ending_by_speed_shortens_the_ramp_when_slow() {
        let cfg = CorrectCfg {
            start_px: 100.0,
            se_min: 0.0,
            se_by_speed: true,
            ..Default::default()
        };
        let slow: Vec<PenSample> = (0..200)
            .map(|i| t(i as f32, 0.0, i as f64 * 60.0))
            .collect();
        let fast: Vec<PenSample> = (0..200).map(|i| t(i as f32, 0.0, i as f64 * 0.5)).collect();
        let a = post(cfg, 1.0, &slow)[50].pressure;
        let b = post(cfg, 1.0, &fast)[50].pressure;
        assert!(
            a > b + 0.3,
            "S-027: a slow stroke gets the weaker (shorter) taper: {a} vs {b}"
        );
        // At 25 px in, the slow stroke's ramp (25 px long) is already done.
        assert!((post(cfg, 1.0, &slow)[30].pressure - 1.0).abs() < 1e-4);
    }

    #[test]
    fn correction_never_outlives_a_stroke() {
        // A stroke's buffer must be empty when the next one begins, or the
        // tail of stroke N smears into the head of stroke N+1 — the exact
        // shape of bug `MyBrush::FIRST_SAMPLE_DTIME` exists to prevent.
        let mut doc = Document::new(64, 64);
        let mut st = Stabilizer::new(Recorder::default(), 0.0);
        st.set_correction(CorrectCfg {
            post: 1.0,
            end_px: 40.0,
            se_min: 0.1,
            ..Default::default()
        });
        for y in [10.0f32, 40.0] {
            st.begin(&mut doc);
            for i in 0..60 {
                st.sample(&mut doc, t(i as f32, y, i as f64 * 4.0));
            }
            st.end(&mut doc);
        }
        let got = &st.inner().got;
        assert_eq!(got.len(), 120, "every sample of both strokes, once");
        assert!(
            got[..60].iter().all(|p| (p.y - 10.0).abs() < 2.0),
            "stroke 1 leaked into stroke 2"
        );
        assert!(got[60..].iter().all(|p| (p.y - 40.0).abs() < 2.0));
    }
}
