//! The page furniture: drop shadow, page outline, symmetry axes, the frame
//! focus veil and the manuscript guides. Moved here verbatim when
//! `overlay.rs` was split by Z-order band.

use super::super::theme;
use crate::app::App;

// --- canvas overlay: page shadow + manuscript guides --------------------

/// The page drop shadow's two strips (right edge, bottom edge), `d` px deep,
/// from the page's two TRANSFORMED corners. Issue #6: taking them as
/// top-left/bottom-right verbatim degenerates the moment the view is
/// mirrored — the corners swap and the rects come out inverted (empty).
/// Min/max instead: the light keeps falling to the screen's bottom-right,
/// whichever page corner has landed there under H, V or H+V.
fn shadow_rects(a: egui::Pos2, b: egui::Pos2, d: f32) -> [egui::Rect; 2] {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    [
        egui::Rect::from_min_max(egui::pos2(x1, y0 + d), egui::pos2(x1 + d, y1 + d)),
        egui::Rect::from_min_max(egui::pos2(x0 + d, y1), egui::pos2(x1, y1 + d)),
    ]
}

/// Z-order band 3: the page itself and everything printed on it.
pub(super) fn paint(
    app: &App,
    painter: &egui::Painter,
    canvas_pts: egui::Rect,
    to_pt: &dyn Fn(f32, f32) -> egui::Pos2,
) {
    let quad = |r: [f32; 4]| -> Vec<egui::Pos2> {
        vec![
            to_pt(r[0], r[1]),
            to_pt(r[2], r[1]),
            to_pt(r[2], r[3]),
            to_pt(r[0], r[3]),
            to_pt(r[0], r[1]),
        ]
    };

    let (w, h) = app.doc.size;
    let page = [0.0, 0.0, w as f32, h as f32];

    // Drop shadow: only when the view is unrotated (axis-aligned strips are
    // cheap and correct; a rotated shadow would overdraw the page).
    if app.viewport.rotate_rad == 0.0 {
        let shadow = egui::Color32::from_black_alpha(90);
        for r in shadow_rects(to_pt(0.0, 0.0), to_pt(w as f32, h as f32), 6.0) {
            painter.rect_filled(r, 0.0, shadow);
        }
    }
    painter.add(egui::Shape::line(
        quad(page),
        egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
    ));

    // CV-046 the canvas grid, under everything else on the page so it never
    // competes with the crop marks. Ruled through `to_pt` like the rest of
    // the furniture, so it rides a rotated or mirrored view instead of
    // staying stubbornly axis-aligned. Cell edges read at half the strength
    // of a page outline; the subdivisions at half of that again — a grid you
    // can count against but never mistake for ink.
    if app.layout.grid_on {
        let lines = crate::app::grid_lines(
            app.doc.size,
            app.page_dpi(),
            app.layout.grid_mm,
            app.layout.grid_div,
        );
        // Fixed greys, not theme colours: this is drawn ON the white page,
        // beside the manuscript guides, which are fixed RGBA for the same
        // reason. A chrome text colour would be a pale grey on a dark theme
        // and vanish into the paper.
        let major = egui::Color32::from_rgba_unmultiplied(70, 70, 70, 120);
        let minor = egui::Color32::from_rgba_unmultiplied(70, 70, 70, 55);
        for g in lines {
            let (a, b) = if g.horizontal {
                (to_pt(0.0, g.pos), to_pt(w as f32, g.pos))
            } else {
                (to_pt(g.pos, 0.0), to_pt(g.pos, h as f32))
            };
            painter.line_segment(
                [a, b],
                egui::Stroke::new(1.0, if g.major { major } else { minor }),
            );
        }
    }

    // Symmetry axes (Krita mirror): the centre lines strokes reflect across.
    // Dashed by hand (egui has no dash offset), faint accent so they read as
    // guides, not ink.
    if app.mirror_x || app.mirror_y {
        let dash = |a: egui::Pos2, b: egui::Pos2| {
            let len = (b - a).length();
            let dir = (b - a) / len.max(0.001);
            let mut t = 0.0;
            while t < len {
                let t2 = (t + 6.0).min(len);
                painter.line_segment(
                    [a + dir * t, a + dir * t2],
                    egui::Stroke::new(1.0, theme::c().accent.linear_multiply(0.55)),
                );
                t += 10.0;
            }
        };
        if app.mirror_x {
            let x = w as f32 * 0.5;
            dash(to_pt(x, 0.0), to_pt(x, h as f32));
        }
        if app.mirror_y {
            let y = h as f32 * 0.5;
            dash(to_pt(0.0, y), to_pt(w as f32, y));
        }
    }

    // Frame focus (CSP): with a frame layer active, a translucent blue veil
    // covers the page OUTSIDE its panels — picking a draw layer inside lifts
    // it. Scanline even-odd in screen space: per row, subtract the panels'
    // spans from the page span. Handles concave panels for free.
    if app.doc.active_layer().is_frame() {
        if let Some(fs) = app.doc.active_layer().frames() {
            let polys: Vec<Vec<egui::Pos2>> = fs
                .frames
                .iter()
                .map(|f| f.points.iter().map(|p| to_pt(p[0], p[1])).collect())
                .collect();
            let page_r = egui::Rect::from_min_max(to_pt(0.0, 0.0), to_pt(w as f32, h as f32));
            let area = canvas_pts.intersect(page_r);
            let tint = egui::Color32::from_rgba_unmultiplied(96, 132, 255, 42);
            let mut crossings: Vec<f32> = Vec::new();
            let mut y = area.top().ceil();
            while y < area.bottom() {
                crossings.clear();
                for poly in &polys {
                    let n = poly.len();
                    for i in 0..n {
                        let (a, b) = (poly[i], poly[(i + 1) % n]);
                        if (a.y <= y) != (b.y <= y) {
                            crossings.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
                        }
                    }
                }
                let mut x = area.left();
                if crossings.is_empty() {
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(x, y),
                            egui::pos2(area.right(), y + 1.0),
                        ),
                        0.0,
                        tint,
                    );
                } else {
                    crossings.sort_by(f32::total_cmp);
                    for pair in crossings.chunks_exact(2) {
                        let s = pair[0].clamp(area.left(), area.right());
                        let e = pair[1].clamp(area.left(), area.right());
                        if s > x {
                            painter.rect_filled(
                                egui::Rect::from_min_max(egui::pos2(x, y), egui::pos2(s, y + 1.0)),
                                0.0,
                                tint,
                            );
                        }
                        x = x.max(e);
                    }
                    if x < area.right() {
                        painter.rect_filled(
                            egui::Rect::from_min_max(
                                egui::pos2(x, y),
                                egui::pos2(area.right(), y + 1.0),
                            ),
                            0.0,
                            tint,
                        );
                    }
                }
                y += 1.0;
            }
        }
    }

    // Manuscript guides — spec section B finding #4: bleed (dim cyan), trim
    // (blue), default/inner border (green), publisher safety margin (orange,
    // dashed).
    //
    // CV-041: View ▸ Crop marks and margins puts them away. It HIDES, it
    // does not delete — the page setup is untouched, panel snapping still
    // reads the same lines (`template_lines` below), and export never drew
    // them in the first place. The switch persists in ui.txt.
    if let Some(ps) = app.page.as_ref().filter(|_| !app.layout.guides_hidden) {
        let bleed_stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(110, 190, 220, 110),
        );
        let trim_stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(110, 150, 240, 210),
        );
        let inner_stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(110, 200, 140, 190),
        );
        let safety_stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(240, 170, 90, 170),
        );
        // Book-side aware (owner report 2026-08-22): the inner frame and
        // safety margins mirror per page — ノド (binding) margin on the
        // spine side, 小口 offset outward — so pages 2 and 3 stop wearing
        // the same gutter. A combined spread draws BOTH halves' frames and
        // spans trim/bleed across the fold (nothing is cut at the fold).
        let shift = |r: [f32; 4], dx: f32| [r[0] + dx, r[1], r[2] + dx, r[3]];
        let span = |r: [f32; 4], dw: f32, pw: f32| [r[0], r[1], dw - (pw - r[2]), r[3]];
        match app.current_page_right() {
            Some(right) => {
                painter.add(egui::Shape::line(quad(ps.bleed_rect_px()), bleed_stroke));
                painter.add(egui::Shape::line(quad(ps.trim_rect_px()), trim_stroke));
                painter.add(egui::Shape::line(
                    quad(ps.inner_rect_px_on(right)),
                    inner_stroke,
                ));
                if let Some(r) = ps.safety_rect_px_on(right) {
                    painter.extend(egui::Shape::dashed_line(&quad(r), safety_stroke, 5.0, 5.0));
                }
            }
            None => {
                let pw = ps.paper_px().0 as f32;
                let dw = app.doc.size.0 as f32;
                painter.add(egui::Shape::line(
                    quad(span(ps.bleed_rect_px(), dw, pw)),
                    bleed_stroke,
                ));
                painter.add(egui::Shape::line(
                    quad(span(ps.trim_rect_px(), dw, pw)),
                    trim_stroke,
                ));
                // Left half of the spread = a LEFT page, right half = a
                // RIGHT page, whatever the binding — the fold is between.
                painter.add(egui::Shape::line(
                    quad(ps.inner_rect_px_on(false)),
                    inner_stroke,
                ));
                painter.add(egui::Shape::line(
                    quad(shift(ps.inner_rect_px_on(true), dw - pw)),
                    inner_stroke,
                ));
                if let Some(r) = ps.safety_rect_px_on(false) {
                    painter.extend(egui::Shape::dashed_line(&quad(r), safety_stroke, 5.0, 5.0));
                }
                if let Some(r) = ps.safety_rect_px_on(true) {
                    painter.extend(egui::Shape::dashed_line(
                        &quad(shift(r, dw - pw)),
                        safety_stroke,
                        5.0,
                        5.0,
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> egui::Pos2 {
        egui::pos2(x, y)
    }

    /// Issue #6: the shadow strips came straight off `to_pt(0,0)` and
    /// `to_pt(w,h)`, which are only top-left/bottom-right in an unmirrored
    /// view — under the H mirror (pre-existing) or the V flip the rects
    /// inverted and the shadow vanished. Driven through the real
    /// `Viewport`, all four flip states must produce the same two strips
    /// hugging the page box's right and bottom edges.
    #[test]
    fn the_drop_shadow_hugs_the_page_under_every_flip() {
        let (w, h, d) = (400.0_f32, 300.0_f32, 6.0_f32);
        for (fh, fv) in [(false, false), (true, false), (false, true), (true, true)] {
            let vp = mn_gpu::Viewport {
                pan: [500.0, 400.0],
                zoom: 1.0,
                rotate_rad: 0.0,
                flip_h: fh,
                flip_v: fv,
            };
            let pt = |x: f32, y: f32| {
                let (sx, sy) = vp.to_screen(x, y);
                p(sx, sy)
            };
            let [right, bottom] = shadow_rects(pt(0.0, 0.0), pt(w, h), d);
            let page = egui::Rect::from_two_pos(pt(0.0, 0.0), pt(w, h));
            let case = format!("flip_h={fh} flip_v={fv}");
            assert_eq!(right.size(), egui::vec2(d, h), "right strip ({case})");
            assert_eq!(bottom.size(), egui::vec2(w - d, d), "bottom strip ({case})");
            assert_eq!(right.min, p(page.max.x, page.min.y + d), "{case}");
            assert_eq!(bottom.min, p(page.min.x + d, page.max.y), "{case}");
        }
    }

    /// The unflipped rendering is the reference and must not have moved:
    /// the exact rects the old inline code painted.
    #[test]
    fn the_unflipped_shadow_is_byte_identical() {
        let (tl, br, d) = (p(10.0, 20.0), p(410.0, 320.0), 6.0);
        assert_eq!(
            shadow_rects(tl, br, d),
            [
                egui::Rect::from_min_max(
                    egui::pos2(br.x, tl.y + d),
                    egui::pos2(br.x + d, br.y + d)
                ),
                egui::Rect::from_min_max(egui::pos2(tl.x + d, br.y), egui::pos2(br.x, br.y + d)),
            ]
        );
    }
}
