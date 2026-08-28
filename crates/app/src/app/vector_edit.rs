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

use mn_core::{Stabilizer, StrokeSink, Taper, VectorStroke};

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
    pub fn rederive_vector_layer(&mut self, li: usize) {
        let Some(set) = self.doc.layers.get(li).and_then(|l| l.strokes.clone()) else {
            return;
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
            let Ok(b) = mn_brush::MyBrush::load(&path) else {
                missing = true;
                continue;
            };
            let mut fresh = Stabilizer::new(
                Taper::new(Engine::new(EngineKind::My(Box::new(b)))),
                s.stabilizer,
            );
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

fn dist_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (abx, aby) = (b[0] - a[0], b[1] - a[1]);
    let len2 = abx * abx + aby * aby;
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len2).clamp(0.0, 1.0)
    };
    (p[0] - (a[0] + abx * t)).hypot(p[1] - (a[1] + aby * t))
}
