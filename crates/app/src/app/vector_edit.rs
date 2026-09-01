//! Vector inking phase 2 (docs/VECTOR-INKING.md): Object-tool editing of
//! recorded strokes — select, translate, drag a point with a smooth
//! falloff — and the re-derivation both rest on.
//!
//! Editing is LAYER-SCOPED: the Object tool grabs the ACTIVE layer's
//! strokes only (you selected the layer to edit it — the same scoping mask
//! editing uses), so existing object picking on other layers is untouched.
//! The drag edits the GEOMETRY live (the overlay draws it); the RASTER
//! re-derives once at release, inside one op, closed by
//! `end_op_vector_edit` — geometry and pixels stay one undo step.
//!
//! Re-derivation follows CODE-MAP's two replay rules: enter at the engine
//! (`brush.begin/sample/end` on the samples as captured) and on a FRESH
//! wrapper stack per stroke, built from the stroke's own preset at the
//! stroke's own stabilizer strength.

use std::path::PathBuf;

use mn_core::{Stabilizer, StrokeSet, StrokeSink, Taper, VectorStroke};

use super::{App, Engine, EngineKind};

/// A drag in flight on the selected stroke.
pub struct VectorDrag {
    /// The grabbed sample index (`None` = body drag: translate everything).
    pub point: Option<usize>,
    /// Phase 4: Alt-drag re-WIDTHS instead of moving — vertical motion
    /// scales the PRESSURE channel (per-sample width for every
    /// pressure-driven preset) around the grabbed spot, same falloff.
    pub width: bool,
    /// The sample nearest the grab (the width falloff's centre).
    pub anchor: usize,
    pub start: [f32; 2],
    /// The stroke as it was at the grab — the undo pre-image, and the base
    /// the live deformation recomputes from (no per-move accumulation
    /// drift).
    pub before: VectorStroke,
    pub moved: bool,
}

impl App {
    /// Object-tool press on the active layer's strokes. Points grab before
    /// bodies, later strokes before earlier (they draw on top); the SAME
    /// zoom-scaled tolerance gates both (the CODE-MAP hit-test rule).
    pub fn vector_hit(&mut self, cx: f32, cy: f32, width_mode: bool) -> bool {
        let li = self.doc.active;
        let Some(set) = self.doc.layers.get(li).and_then(|l| l.strokes.as_ref()) else {
            return false;
        };
        let tol = (10.0 / self.viewport.zoom.max(0.01)).max(2.0);
        let mut hit: Option<(usize, Option<usize>)> = None;
        // Point grabs target the HANDLES the overlay actually draws
        // (arc-length spaced — the resampler makes samples far denser than
        // any tolerance, so "any sample is a point" would leave body drags
        // unreachable).
        'outer: for (si, s) in set.strokes.iter().enumerate().rev() {
            for pi in handle_indices(s) {
                let (x, y) = (s.points[pi].0, s.points[pi].1);
                if (x - cx).hypot(y - cy) <= tol {
                    hit = Some((si, Some(pi)));
                    break 'outer;
                }
            }
        }
        if hit.is_none() {
            'outer: for (si, s) in set.strokes.iter().enumerate().rev() {
                for w in s.points.windows(2) {
                    let (a, b) = ([w[0].0, w[0].1], [w[1].0, w[1].1]);
                    if dist_to_segment([cx, cy], a, b) <= tol {
                        hit = Some((si, None));
                        break 'outer;
                    }
                }
            }
        }
        let Some((si, point)) = hit else {
            // A miss on a vector layer clears the selection (the Object
            // tool's click-empty-deselects convention).
            self.vector_sel = None;
            return false;
        };
        self.vector_sel = Some(si);
        // The width falloff centres on the sample nearest the grab.
        let anchor = set.strokes[si]
            .points
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let da = (a.0 - cx).hypot(a.1 - cy);
                let db = (b.0 - cx).hypot(b.1 - cy);
                da.total_cmp(&db)
            })
            .map_or(0, |(i, _)| i);
        self.vector_drag = Some(VectorDrag {
            point,
            width: width_mode,
            anchor,
            start: [cx, cy],
            before: set.strokes[si].clone(),
            moved: false,
        });
        true
    }

    /// Drag: recompute the live geometry from `before` + the total delta.
    pub fn vector_drag_move(&mut self, cx: f32, cy: f32) -> bool {
        let li = self.doc.active;
        let Some(d) = &mut self.vector_drag else {
            return false;
        };
        let (dx, dy) = (cx - d.start[0], cy - d.start[1]);
        d.moved |= dx.abs() + dy.abs() > 0.0;
        let Some(si) = self.vector_sel else {
            return false;
        };
        let Some(s) = self
            .doc
            .layers
            .get_mut(li)
            .and_then(|l| l.strokes.as_mut())
            .and_then(|set| set.strokes.get_mut(si))
        else {
            return false;
        };
        if d.width {
            // Up = thicker, down = thinner; ±100 px of drag doubles/halves.
            // The scale applies to the PRESSURE channel under the same
            // falloff the move uses (three brush-widths here — width edits
            // want a broader reach than a point nudge).
            let factor = 2f32.powf(-dy / 100.0);
            let radius = (d.before.size_px * 3.0).max(48.0);
            let (gx, gy) = (d.before.points[d.anchor].0, d.before.points[d.anchor].1);
            for (p, b) in s.points.iter_mut().zip(&d.before.points) {
                let t = ((b.0 - gx).hypot(b.1 - gy) / radius).min(1.0);
                let w = 0.5 + 0.5 * (t * std::f32::consts::PI).cos();
                let f = 1.0 + (factor - 1.0) * w;
                p.2 = (b.2 * f).clamp(0.01, 1.0);
            }
            self.needs_redraw = true;
            return true;
        }
        match d.point {
            None => {
                for (p, b) in s.points.iter_mut().zip(&d.before.points) {
                    p.0 = b.0 + dx;
                    p.1 = b.1 + dy;
                }
            }
            Some(pi) => {
                // Raised-cosine falloff around the grabbed point — ONE
                // brush-width of neighbourhood (floor 16 px), so a fat
                // brush bends a broad span and a fine pen stays local
                // without ever dragging the whole stroke.
                let radius = d.before.size_px.max(16.0);
                let (gx, gy) = (d.before.points[pi].0, d.before.points[pi].1);
                for (p, b) in s.points.iter_mut().zip(&d.before.points) {
                    let t = ((b.0 - gx).hypot(b.1 - gy) / radius).min(1.0);
                    let w = 0.5 + 0.5 * (t * std::f32::consts::PI).cos();
                    p.0 = b.0 + dx * w;
                    p.1 = b.1 + dy * w;
                }
            }
        }
        self.needs_redraw = true;
        true
    }

    /// Release: re-derive the layer once, one undo step for the gesture.
    /// A grab that never moved restores nothing and spends nothing.
    pub fn vector_drag_release(&mut self) -> bool {
        let Some(d) = self.vector_drag.take() else {
            return false;
        };
        let Some(si) = self.vector_sel else {
            return true;
        };
        if !d.moved {
            return true;
        }
        let li = self.doc.active;
        let (label, status) = if d.width {
            ("Re-width stroke", "stroke re-widthed")
        } else {
            ("Move stroke", "stroke moved")
        };
        self.doc.begin_op_on(li);
        self.rederive_vector_layer(li);
        self.doc.end_op_vector_edit(si, d.before, label);
        self.renderer.invalidate();
        self.set_status(status);
        self.needs_redraw = true;
        true
    }

    /// Replay every recorded stroke into the layer's tiles, INSIDE the
    /// caller's open op (the pre-images ride that op). CODE-MAP's replay
    /// rules: engine entry, fresh wrapper per stroke from the stroke's own
    /// preset and stabilizer.
    ///
    /// `false` = at least one stroke could NOT be replayed and its ink is
    /// gone from the layer. The caller owns the status line, so it has to
    /// carry that word out — a re-derive that quietly drops art and then
    /// reports the edit as done is the worst shape this can take.
    pub fn rederive_vector_layer(&mut self, li: usize) -> bool {
        let Some(set) = self.doc.layers.get(li).and_then(|l| l.strokes.clone()) else {
            return true;
        };
        // Clear through the tile APIs so every pre-image is captured.
        let idxs: Vec<_> = self.doc.layers[li].tiles().map(|(i, _)| i).collect();
        for idx in idxs {
            self.doc.layers[li].tile_mut(idx).data_mut().fill(0);
        }
        let saved_active = self.doc.active;
        self.doc.set_active(li);
        let mut missing = false;
        for s in &set.strokes {
            let path = self.resolve_preset(&s.preset);
            let Some(path) = path else {
                missing = true;
                continue;
            };
            // The PROCEDURAL sub tools (`mn-engine`: the dot pen, the Krita
            // engines) are asked for by name, exactly as `SelectBrush` asks —
            // their preset files carry no libmypaint settings on purpose, so
            // `MyBrush::load` refuses them. It used to be the only door here,
            // which meant a vector layer inked with the dot pen lost its ink
            // the first time anything re-derived it (one control point moved,
            // one line-correction pass) while the status said the pass had
            // worked.
            let kind = match crate::app::preset_engine(&path) {
                Some(k) => k,
                None => match mn_brush::MyBrush::load(&path) {
                    Ok(b) => EngineKind::My(Box::new(b)),
                    Err(_) => {
                        missing = true;
                        continue;
                    }
                },
            };
            let mut fresh = Stabilizer::new(Taper::new(Engine::new(kind)), s.stabilizer);
            // The stroke's own Tool Property snapshot, where it has one.
            // Without this the wrapper stack keeps its constructor defaults
            // and the engine keeps the preset's, so a layer inked at 40 %
            // with an entry taper came back opaque and blunt. A record from
            // before the snapshot existed has None and replays as it always
            // did — same stack, same defaults.
            if let Some(cfg) = s.settings {
                fresh.set_correction(cfg.correct);
                let t = fresh.inner_mut();
                t.length_px = cfg.taper_px;
                t.min = cfg.taper_min;
            }
            {
                let e = fresh.inner_mut().inner_mut();
                if let Some(cfg) = s.settings {
                    e.set_base_opacity(cfg.opacity);
                    // Row 71: the rim is baked, so a replay that skipped it
                    // would strip the watercolour edge off every stroke on
                    // the layer the first time one control point moved.
                    e.set_water_edge(cfg.water_edge);
                }
                e.set_size_px(s.size_px * s.width_scale.max(0.01));
                e.set_color([
                    f32::from(s.color[0]) / 255.0,
                    f32::from(s.color[1]) / 255.0,
                    f32::from(s.color[2]) / 255.0,
                ]);
                e.set_eraser(s.eraser);
            }
            fresh.begin(&mut self.doc);
            for smp in s.samples() {
                fresh.sample(&mut self.doc, smp);
            }
            fresh.end(&mut self.doc);
        }
        self.doc.set_active(saved_active);
        if missing {
            self.set_status("some strokes' presets are missing — their ink was not re-derived");
        }
        !missing
    }

    /// Row 169 (`E-001`…`E-007`, `VL-021`…`VL-027`): one line-correction
    /// pass over the ACTIVE layer's whole stroke record — the tidy-up that
    /// makes a hastily inked page usable.
    ///
    /// Records edit, raster re-derives: the ops rewrite the RECORDS and the
    /// pixels come back through the same replay every other vector edit
    /// uses, closed by [`mn_core::Document::end_op_vector_set`] — the whole
    /// layer, ONE undo press.
    ///
    /// No live preview in v1, deliberately: a preview means re-deriving the
    /// layer every slider frame (a full engine replay of every stroke), and
    /// the ops are cheap to undo. The window says what it did in the status
    /// line instead.
    pub fn line_correct(&mut self, op: LineCorrect) {
        let li = self.doc.active;
        let Some(before) = self.doc.layers.get(li).and_then(|l| l.strokes.clone()) else {
            self.set_status(
                "line correction needs a vector layer — its recorded strokes are what gets corrected",
            );
            return;
        };
        let mut after = before.clone();
        if !correct_lines(&mut after, op) {
            self.set_status("line correction: nothing to change at that setting");
            return;
        }
        let (was, now) = (before.strokes.len(), after.strokes.len());
        self.doc.begin_op_on(li);
        self.doc.layers[li].strokes = Some(after);
        // Indices just restructured under the Object tool's selection.
        self.vector_sel = None;
        let replayed = self.rederive_vector_layer(li);
        self.doc.end_op_vector_set(before, op.label());
        self.renderer.invalidate();
        // The pass's own line, unless the replay had to drop a stroke — then
        // its warning stands, because "4 line(s) simplified" over a layer
        // that just lost art is the one thing the artist must not be told.
        if replayed {
            self.set_status(op.status(was, now));
        }
        self.needs_redraw = true;
        self.mark_dirty();
    }

    /// A recorded preset key → its current path; falls back to the selected
    /// preset (degraded, said in the status by the caller) only when the
    /// exact brush is gone.
    fn resolve_preset(&self, key: &str) -> Option<PathBuf> {
        self.presets
            .iter()
            .map(|(_, p)| p)
            .find(|p| self.preset_key(p) == key)
            .cloned()
            .or_else(|| {
                self.selected_preset
                    .and_then(|i| self.presets.get(i))
                    .map(|(_, p)| p.clone())
            })
    }
}

/// The stroke's visible handles: the endpoints plus one every ~24 canvas px
/// of arc length. One definition — the overlay draws these and the hit test
/// grabs them, so what you see is exactly what you can grab.
pub fn handle_indices(s: &VectorStroke) -> Vec<usize> {
    const SPACING: f32 = 24.0;
    let mut out = Vec::new();
    let mut accum = f32::INFINITY; // the first point is always a handle
    for (i, w) in std::iter::once(None)
        .chain(s.points.windows(2).map(Some))
        .enumerate()
    {
        if let Some(w) = w {
            accum += (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1);
        }
        if accum >= SPACING {
            out.push(i);
            accum = 0.0;
        }
    }
    let last = s.points.len().saturating_sub(1);
    if out.last() != Some(&last) && !s.points.is_empty() {
        out.push(last);
    }
    out
}

// ---------------------------------------------------------------------------
// Row 169 — line correction (`E-001`…`E-007`, `VL-021`…`VL-027`)
// ---------------------------------------------------------------------------

/// One recorded sample, as `VectorStroke::points` flattens it:
/// (x, y, pressure, tilt_x, tilt_y, t_ms).
type Sample = (f32, f32, f32, f32, f32, f64);

/// One line-correction pass over a whole recorded layer. Each variant is
/// ONE user gesture and therefore one undo press — the dialog's four
/// buttons, not a checkbox set, because a mangaka wants to sweep the stubs
/// and keep the simplify.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineCorrect {
    /// `E-007` / `VL-024`: a stroke whose polyline is shorter than this many
    /// canvas px is a stub — sweep it up. The one that makes a hatched page
    /// usable.
    DeleteShort { px: f32 },
    /// `E-005` / `VL-025`: endpoints within `px` of each other become one
    /// stroke. `across` is `E-006` — join lines with DIFFERENT properties
    /// too, and (CSP's rule) the LONGER line's properties win.
    Connect { px: f32, across: bool },
    /// `E-001` / `VL-021`: Douglas–Peucker over the recorded samples at this
    /// tolerance in canvas px. Corners survive by construction (they are
    /// exactly the points furthest from a chord), so `E-002`'s smooth-corner
    /// arm is not modelled here.
    Simplify { px: f32 },
    /// `VL-026` "scale up/down": multiply every recorded width, which keeps
    /// tapers pointed (the pressure channel is untouched — the thicken/narrow
    /// arm that rounds them is a different op). `VL-027`'s "at least 1 pixel"
    /// floor is unconditional: narrowing never erases a line.
    Width { scale: f32 },
}

impl LineCorrect {
    /// The undo-history label — what the History palette shows.
    pub fn label(self) -> &'static str {
        match self {
            LineCorrect::DeleteShort { .. } => "Delete short lines",
            LineCorrect::Connect { .. } => "Connect lines",
            LineCorrect::Simplify { .. } => "Simplify lines",
            LineCorrect::Width { .. } => "Adjust line width",
        }
    }

    fn status(self, was: usize, now: usize) -> String {
        match self {
            LineCorrect::DeleteShort { .. } => {
                format!("{} short line(s) swept up", was.saturating_sub(now))
            }
            LineCorrect::Connect { .. } => format!(
                "{} join(s) — {now} line(s) left",
                was.saturating_sub(now)
            ),
            LineCorrect::Simplify { .. } => format!("{now} line(s) simplified"),
            LineCorrect::Width { scale } => format!("line width ×{scale:.2}"),
        }
    }
}

/// Apply one correction to a stroke record. Pure and total — no engine, no
/// document — so the four behaviours test on their own before any raster
/// re-derives. Returns whether anything actually changed; `false` means the
/// caller must NOT spend an undo step.
pub fn correct_lines(set: &mut StrokeSet, op: LineCorrect) -> bool {
    match op {
        LineCorrect::DeleteShort { px } => {
            let (px, n) = (px.max(0.0), set.strokes.len());
            set.strokes.retain(|s| polyline_px(s) >= px);
            set.strokes.len() != n
        }
        LineCorrect::Connect { px, across } => connect(set, px.max(0.0), across),
        LineCorrect::Simplify { px } => {
            let mut changed = false;
            for s in &mut set.strokes {
                let keep = rdp_indices(&s.points, px.max(0.01));
                if keep.len() < s.points.len() {
                    s.points = keep.iter().map(|&i| s.points[i]).collect();
                    changed = true;
                }
            }
            changed
        }
        LineCorrect::Width { scale } => {
            let mut changed = false;
            for s in &mut set.strokes {
                // `VL-027`: the floor is on the DERIVED width (`size_px ×
                // width_scale`, what `rederive_vector_layer` feeds the
                // engine), so a 0.1× pass on a 4 px pen stops at 1 px
                // instead of vanishing.
                let floor = 1.0 / s.size_px.max(0.01);
                let w = (s.width_scale * scale).max(floor);
                if (w - s.width_scale).abs() > 1e-6 {
                    s.width_scale = w;
                    changed = true;
                }
            }
            changed
        }
    }
}

/// Which end of the target meets which end of the incoming stroke. A join is
/// DIRECTION-AWARE: the incoming run is reversed when its far end is the one
/// that touches, so the result is a single continuous polyline rather than a
/// path that doubles back through the seam.
#[derive(Clone, Copy, Debug)]
enum Join {
    /// `a.end ~ b.start` — `a ++ b`.
    AppendFwd,
    /// `a.end ~ b.end` — `a ++ rev(b)`.
    AppendRev,
    /// `a.start ~ b.end` — `b ++ a`.
    PrependFwd,
    /// `a.start ~ b.start` — `rev(b) ++ a`.
    PrependRev,
}

/// `E-005`: one greedy pass in DRAW ORDER. Each stroke either attaches to an
/// already-emitted one (nearest of the four endpoint pairings wins) or is
/// emitted itself — so chains grow naturally (a joined stroke's new ends
/// stay eligible), order is preserved, and the whole thing is O(n²) instead
/// of the repeat-until-quiet O(n³) a hatching-heavy page would feel.
fn connect(set: &mut StrokeSet, tol: f32, across: bool) -> bool {
    if set.strokes.len() < 2 {
        return false;
    }
    let mut out: Vec<VectorStroke> = Vec::with_capacity(set.strokes.len());
    let mut changed = false;
    for s in std::mem::take(&mut set.strokes) {
        if s.points.len() < 2 {
            out.push(s);
            continue;
        }
        let mut best: Option<(usize, Join, f32)> = None;
        for (k, o) in out.iter().enumerate() {
            if o.points.len() < 2 || (!across && !same_properties(o, &s)) {
                continue;
            }
            if let Some((j, d)) = nearest_join(o, &s, tol)
                && best.is_none_or(|b| d < b.2)
            {
                best = Some((k, j, d));
            }
        }
        match best {
            Some((k, j, _)) => {
                join_into(&mut out[k], s, j, across);
                changed = true;
            }
            None => out.push(s),
        }
    }
    set.strokes = out;
    changed
}

/// Everything the replay rebuilds the engine from. `E-006` off means two
/// strokes only join when all of it matches — a 2 px pen never absorbs a
/// 20 px marker behind your back.
fn same_properties(a: &VectorStroke, b: &VectorStroke) -> bool {
    a.preset == b.preset
        && (a.size_px - b.size_px).abs() < 1e-4
        && (a.width_scale - b.width_scale).abs() < 1e-4
        && a.color == b.color
        && a.eraser == b.eraser
        && a.settings == b.settings
}

fn nearest_join(a: &VectorStroke, b: &VectorStroke, tol: f32) -> Option<(Join, f32)> {
    let (a0, a1) = (a.points[0], a.points[a.points.len() - 1]);
    let (b0, b1) = (b.points[0], b.points[b.points.len() - 1]);
    let d = |p: Sample, q: Sample| (p.0 - q.0).hypot(p.1 - q.1);
    [
        (Join::AppendFwd, d(a1, b0)),
        (Join::AppendRev, d(a1, b1)),
        (Join::PrependFwd, d(a0, b1)),
        (Join::PrependRev, d(a0, b0)),
    ]
    .into_iter()
    .filter(|&(_, dd)| dd <= tol)
    .min_by(|x, y| x.1.total_cmp(&y.1))
}

fn join_into(a: &mut VectorStroke, b: VectorStroke, j: Join, across: bool) {
    // `E-006`: joining across properties, the LONGER line's win.
    if across && !same_properties(a, &b) && polyline_px(&b) > polyline_px(a) {
        a.preset = b.preset.clone();
        a.size_px = b.size_px;
        a.width_scale = b.width_scale;
        a.color = b.color;
        a.eraser = b.eraser;
        a.stabilizer = b.stabilizer;
        a.settings = b.settings;
    }
    let bp = b.points;
    let mut pts = std::mem::take(&mut a.points);
    match j {
        Join::AppendFwd => pts.extend(bp),
        Join::AppendRev => pts.extend(bp.into_iter().rev()),
        Join::PrependFwd => {
            let mut v = bp;
            v.extend(pts);
            pts = v;
        }
        Join::PrependRev => {
            let mut v: Vec<Sample> = bp.into_iter().rev().collect();
            v.extend(pts);
            pts = v;
        }
    }
    restamp(&mut pts);
    a.points = pts;
}

/// A join concatenates two independently-timed runs, and a reversed run has
/// its clock running backwards. The replay feeds these samples to the
/// stabilizer, which reads their timing — so re-stamp monotonic. Each step
/// keeps the |Δt| it was drawn with, clamped to a plausible pen range so the
/// seam (two unrelated clocks) can hand the engine neither a zero nor a
/// forty-second pause.
fn restamp(pts: &mut [Sample]) {
    let deltas: Vec<f64> = pts
        .windows(2)
        .map(|w| (w[1].5 - w[0].5).abs().clamp(1.0, 100.0))
        .collect();
    let mut t = pts.first().map_or(0.0, |p| p.5);
    for (i, d) in deltas.into_iter().enumerate() {
        t += d;
        pts[i + 1].5 = t;
    }
}

fn polyline_px(s: &VectorStroke) -> f32 {
    s.points
        .windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
        .sum()
}

/// Douglas–Peucker returning KEPT INDICES.
/// `mn_core::balloon::simplify_polyline` is the same algorithm, but it hands
/// back coordinates, and a recorded sample is (x, y, pressure, tilt, tilt,
/// t) — matching coordinates back to samples goes ambiguous the moment an
/// ink line crosses or doubles back over itself, which manga hatching does
/// constantly. Carrying the index through keeps the WHOLE sample, pressure
/// and all, so a simplified stroke re-derives with its taper intact.
/// Iterative: a long stroke is thousands of samples deep.
fn rdp_indices(pts: &[Sample], eps: f32) -> Vec<usize> {
    if pts.len() < 3 {
        return (0..pts.len()).collect();
    }
    let last = pts.len() - 1;
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[last] = true;
    let mut stack = vec![(0usize, last)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = ([pts[a].0, pts[a].1], [pts[b].0, pts[b].1]);
        let mut worst = (0usize, -1.0f32);
        for (i, p) in pts.iter().enumerate().take(b).skip(a + 1) {
            let d = dist_to_segment([p.0, p.1], pa, pb);
            if d > worst.1 {
                worst = (i, d);
            }
        }
        if worst.1 > eps {
            keep[worst.0] = true;
            stack.push((a, worst.0));
            stack.push((worst.0, b));
        }
    }
    (0..pts.len()).filter(|&i| keep[i]).collect()
}

/// Point-to-segment distance, the hit test every "did I click the line?"
/// question in the app funnels through — the Object tool's stroke picking
/// here, and `FG-013`'s insert-a-point tap on the Figure tool's live path.
pub(crate) fn dist_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (abx, aby) = (b[0] - a[0], b[1] - a[1]);
    let len2 = abx * abx + aby * aby;
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len2).clamp(0.0, 1.0)
    };
    (p[0] - (a[0] + abx * t)).hypot(p[1] - (a[1] + aby * t))
}
