//! The overlay's numeric readouts: the live W×H box every drag hangs off
//! (`IO-081`) and the View-menu input probe. Moved here verbatim when
//! `overlay.rs` was split by Z-order band.

use super::super::theme;
use crate::app::App;

// --- live W×H readout during a drag (IO-081) ---------------------------

/// The readout's text for a drag spanning `w_px` × `h_px` CANVAS pixels.
///
/// コマ割り is measured work — gutters and 原稿 rules are specified in mm —
/// so millimetres lead and the pixels follow in brackets. `dpi` is the
/// work's OWN print resolution ([`App::work_dpi`]), which a plain pixel
/// canvas does not have: inventing one there would print a millimetre
/// number no printer ever agreed to, so that branch reports pixels alone.
///
/// Pixels are whole (a canvas has no half pixel) and the mm come off the
/// UNROUNDED px, so the two halves of the string never disagree by a
/// rounding step.
pub(super) fn dim_readout(w_px: f32, h_px: f32, dpi: Option<u32>) -> String {
    let (w, h) = (w_px.abs(), h_px.abs());
    let (wi, hi) = (w.round() as i64, h.round() as i64);
    match dpi.filter(|d| *d > 0) {
        Some(d) => {
            let mm = |px: f32| px * 25.4 / d as f32;
            format!("{:.1} × {:.1} mm ({wi} × {hi} px)", mm(w), mm(h))
        }
        None => format!("{wi} × {hi} px"),
    }
}

/// Paint the readout as a chip near — never under — the cursor, clamped
/// inside the canvas so a drag into a corner does not push the numbers off
/// screen. Down-right of the pointer by default, which is where the hand
/// is NOT for a right-handed pen grip.
pub(super) fn draw_dim_readout(
    painter: &egui::Painter,
    canvas_pts: egui::Rect,
    cursor: egui::Pos2,
    text: String,
) {
    let t = theme::c();
    let gal = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), t.text);
    let pad = egui::vec2(6.0, 3.0);
    let size = gal.size() + pad * 2.0;
    let mut min = cursor + egui::vec2(16.0, 18.0);
    min.x = min.x.min(canvas_pts.right() - size.x).max(canvas_pts.left());
    min.y = min.y.min(canvas_pts.bottom() - size.y).max(canvas_pts.top());
    let rect = egui::Rect::from_min_size(min, size);
    painter.rect_filled(rect, 3.0, t.panel);
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, t.accent),
        egui::StrokeKind::Inside,
    );
    painter.galley(min + pad, gal, t.text);
}

/// The axis-aligned extent of a run of canvas-px points, as (w, h).
/// Empty = zero, so a readout on a just-started freehand drag reads 0 × 0
/// rather than an infinity from a saturating fold.
pub(super) fn extent_of(pts: impl IntoIterator<Item = (f32, f32)>) -> (f32, f32) {
    let mut b: Option<[f32; 4]> = None;
    for (x, y) in pts {
        b = Some(match b {
            None => [x, y, x, y],
            Some(r) => [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)],
        });
    }
    b.map_or((0.0, 0.0), |r| (r[2] - r[0], r[3] - r[1]))
}

/// Painted in egui, over the GPU canvas, clipped to the canvas area. Guides
/// go through `Viewport::to_screen`, so they survive pan/zoom/rotation.

/// Z-order band 1: the input probe readout, drawn under everything else.
pub(super) fn input_probe(
    ui: &egui::Ui,
    app: &App,
    painter: &egui::Painter,
    canvas_pts: egui::Rect,
) {
    // The input probe readout (View menu): delivery counters, on-canvas —
    // pen AND touch (the pen-tablet corpus's kill classes are "is
    // anything arriving at all", r103).
    if app.touch_probe.enabled {
        let p = &app.touch_probe;
        let lines = format!(
            "input probe  (fingers live: {})\n  pen    {}/{} (down/update; up {})  ← zero while drawing = driver/pen path\n  touch  {}/{} (up {})\n  mouse  {}/{} (up {})\n  other  {}/{} (up {})\n  last: {}\nzero counters while using the device = input never reaches the app\n(touch: test with the pen FAR from the glass — palm rejection)",
            app.touch.len(),
            p.pen[0],
            p.pen[1],
            p.pen[2],
            p.touch[0],
            p.touch[1],
            p.touch[2],
            p.mouse[0],
            p.mouse[1],
            p.mouse[2],
            p.other[0],
            p.other[1],
            p.other[2],
            p.last,
        );
        let anchor = canvas_pts.left_top() + egui::vec2(8.0, 8.0);
        let gal = painter.layout(
            lines,
            egui::FontId::monospace(11.0),
            egui::Color32::from_white_alpha(235),
            400.0,
        );
        let r = gal.mesh_bounds.translate(anchor.to_vec2());
        painter.rect_filled(r.expand(6.0), 3.0, egui::Color32::from_black_alpha(190));
        painter.galley(anchor, gal, egui::Color32::WHITE);
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IO-081: mm leads (コマ割り is specified in mm), px follow, one
    /// decimal on the mm.
    #[test]
    fn dim_readout_leads_with_mm_and_backs_it_with_px() {
        assert_eq!(
            dim_readout(1070.0, 709.0, Some(600)),
            "45.3 × 30.0 mm (1070 × 709 px)"
        );
        // Same pixels at half the resolution are twice the paper.
        assert_eq!(
            dim_readout(1070.0, 709.0, Some(300)),
            "90.6 × 60.0 mm (1070 × 709 px)"
        );
    }

    /// A plain pixel canvas has NO dpi, and the readout must not invent
    /// one — a millimetre number nobody set is worse than no millimetres.
    #[test]
    fn dim_readout_without_a_dpi_is_pixels_only() {
        assert_eq!(dim_readout(1070.0, 709.0, None), "1070 × 709 px");
        // dpi 0 is the same statement in `PageSetup`'s pixel-preset shape.
        assert_eq!(dim_readout(1070.0, 709.0, Some(0)), "1070 × 709 px");
    }

    /// A drag up-and-left is the same rectangle as a drag down-and-right:
    /// the readout is a SIZE, never a signed delta.
    #[test]
    fn dim_readout_is_unsigned_and_rounds_px_whole() {
        assert_eq!(
            dim_readout(-1070.4, -708.6, Some(600)),
            "45.3 × 30.0 mm (1070 × 709 px)"
        );
        assert_eq!(dim_readout(0.0, 0.0, None), "0 × 0 px");
    }

    /// The freehand/mesh path measures an arbitrary point run; an empty
    /// one is zero, not an infinity out of a saturating fold.
    #[test]
    fn extent_of_spans_the_points_and_empty_is_zero() {
        assert_eq!(
            extent_of([(10.0, 5.0), (-2.0, 40.0), (3.0, 12.0)]),
            (12.0, 35.0)
        );
        assert_eq!(extent_of(std::iter::empty()), (0.0, 0.0));
    }
}
