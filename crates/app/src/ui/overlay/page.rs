//! The page furniture: drop shadow, page outline, symmetry axes, the frame
//! focus veil and the manuscript guides. Moved here verbatim when
//! `overlay.rs` was split by Z-order band.

use super::super::theme;
use crate::app::App;

/// The frame folder the focus veil belongs to: the active layer when it IS
/// a frame folder, else the frame folder ENCLOSING it (owner 2026-09-05 —
/// "the blue overlay should apply whenever I'm on any layer in the frame
/// folder", not only on the header row). Same walk as
/// `ui::layers::rows::active_frame_folder`; both are private to their
/// module, so this is six lines rather than a visibility change.
fn veil_folder(doc: &mn_core::Document, active: usize) -> Option<usize> {
    let mut i = active;
    loop {
        if doc.layers.get(i).is_some_and(|l| l.folder && l.is_frame()) {
            return Some(i);
        }
        // `enclosing_folder` only ever looks upward (i+1..), so this ends.
        i = doc.enclosing_folder(i)?;
    }
}

/// The frame-focus veil as ONE mesh (owner 2026-09-05: "MangaNakama froze
/// for a bit and almost crashed just from dividing a panel").
///
/// It used to be a scanline even-odd fill — one `rect_filled` PER SCREEN
/// ROW, so a full-height canvas cost ~1000–3000 egui shapes EVERY frame for
/// as long as a frame folder was the active layer. That is the whole lag.
///
/// Same even-odd semantics (concave panels still work), a different
/// decomposition: cut the visible area into horizontal bands at every
/// polygon vertex's `y`. Inside a band no vertex exists, so every edge that
/// crosses it is one straight segment and the even-odd spans at the band's
/// top and bottom pair up into trapezoids. Cost is O(vertices × panels),
/// not O(rows).
fn veil_mesh(polys: &[Vec<egui::Pos2>], area: egui::Rect, tint: egui::Color32) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    if !(area.width() > 0.0 && area.height() > 0.0) {
        return mesh;
    }
    let (l, r) = (area.left(), area.right());

    let mut ys: Vec<f32> = Vec::with_capacity(polys.iter().map(|p| p.len()).sum::<usize>() + 2);
    ys.push(area.top());
    ys.push(area.bottom());
    for poly in polys {
        for p in poly {
            if p.y > area.top() && p.y < area.bottom() {
                ys.push(p.y);
            }
        }
    }
    ys.sort_by(f32::total_cmp);
    ys.dedup_by(|a, b| (*a - *b).abs() < 1e-4);

    // (x where the edge sits at the band's top, x at its bottom).
    let mut tops: Vec<f32> = Vec::new();
    let mut bots: Vec<f32> = Vec::new();
    for w in ys.windows(2) {
        let (y0, y1) = (w[0], w[1]);
        let ym = 0.5 * (y0 + y1);
        tops.clear();
        bots.clear();
        for poly in polys {
            let n = poly.len();
            for i in 0..n {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                // The scanline rule, sampled at the band's MIDLINE: an edge
                // that crosses there spans the whole band.
                if (a.y <= ym) != (b.y <= ym) {
                    let at = |y: f32| a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x);
                    tops.push(at(y0));
                    bots.push(at(y1));
                }
            }
        }
        // Sorting the two ends independently is the same pairing as
        // following each edge (panels do not overlap, so the order at the
        // band's top and bottom is the same) and degrades gracefully if two
        // edges ever do cross inside a band.
        tops.sort_by(f32::total_cmp);
        bots.sort_by(f32::total_cmp);

        let mut quad = |xt0: f32, xt1: f32, xb0: f32, xb1: f32| {
            let (t0, b0) = (xt0.clamp(l, r), xb0.clamp(l, r));
            let (t1, b1) = (xt1.clamp(l, r).max(t0), xb1.clamp(l, r).max(b0));
            if t1 - t0 <= 0.0 && b1 - b0 <= 0.0 {
                return;
            }
            let i0 = mesh.vertices.len() as u32;
            mesh.colored_vertex(egui::pos2(t0, y0), tint);
            mesh.colored_vertex(egui::pos2(t1, y0), tint);
            mesh.colored_vertex(egui::pos2(b1, y1), tint);
            mesh.colored_vertex(egui::pos2(b0, y1), tint);
            mesh.add_triangle(i0, i0 + 1, i0 + 2);
            mesh.add_triangle(i0, i0 + 2, i0 + 3);
        };

        // Even-odd: crossings pair up into the panels' INTERIOR; the veil is
        // everything between those pairs, plus the two ends of the band.
        let n = tops.len().min(bots.len()) & !1;
        let (mut xt, mut xb) = (l, l);
        let mut k = 0;
        while k < n {
            quad(xt, tops[k], xb, bots[k]);
            xt = tops[k + 1].max(xt);
            xb = bots[k + 1].max(xb);
            k += 2;
        }
        quad(xt, r, xb, r);
    }
    mesh
}

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

    // Frame focus (CSP): with a frame folder in play, a translucent blue veil
    // covers the page OUTSIDE its panels — picking a layer that is not in any
    // frame folder lifts it. Even-odd in screen space, banded into ONE mesh
    // (`veil_mesh`). Handles concave panels for free.
    if let Some(fi) = veil_folder(&app.doc, app.doc.active)
        && let Some(fs) = app.doc.layers[fi].frames()
    {
        let polys: Vec<Vec<egui::Pos2>> = fs
            .frames
            .iter()
            .map(|f| f.points.iter().map(|p| to_pt(p[0], p[1])).collect())
            .collect();
        let page_r = egui::Rect::from_min_max(to_pt(0.0, 0.0), to_pt(w as f32, h as f32));
        let area = canvas_pts.intersect(page_r);
        let tint = egui::Color32::from_rgba_unmultiplied(96, 132, 255, 42);
        let mesh = veil_mesh(&polys, area, tint);
        if !mesh.is_empty() {
            painter.add(egui::Shape::mesh(mesh));
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

    /// The veil the owner was looking at: a 600×400 page, two panels, one
    /// SLANTED cut between them (a straight cut would let the old scanline
    /// off easy — every row identical).
    fn two_panels_slant_cut() -> (Vec<Vec<egui::Pos2>>, egui::Rect) {
        let panels = vec![
            vec![p(20.0, 20.0), p(580.0, 20.0), p(580.0, 170.0), p(20.0, 230.0)],
            vec![
                p(20.0, 250.0),
                p(580.0, 190.0),
                p(580.0, 380.0),
                p(20.0, 380.0),
            ],
        ];
        (panels, egui::Rect::from_min_max(p(0.0, 0.0), p(600.0, 400.0)))
    }

    /// The old drawing: one `rect_filled` per screen row. Kept as a
    /// COUNTER only, so the pin below states the real before/after numbers
    /// instead of a remembered one.
    fn scanline_shape_count(polys: &[Vec<egui::Pos2>], area: egui::Rect) -> usize {
        let mut shapes = 0;
        let mut crossings: Vec<f32> = Vec::new();
        let mut y = area.top().ceil();
        while y < area.bottom() {
            crossings.clear();
            for poly in polys {
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
                shapes += 1;
            } else {
                crossings.sort_by(f32::total_cmp);
                for pair in crossings.chunks_exact(2) {
                    let s = pair[0].clamp(area.left(), area.right());
                    let e = pair[1].clamp(area.left(), area.right());
                    if s > x {
                        shapes += 1;
                    }
                    x = x.max(e);
                }
                if x < area.right() {
                    shapes += 1;
                }
            }
            y += 1.0;
        }
        shapes
    }

    /// Owner 2026-09-05: "MangaNakama froze for a bit and almost crashed
    /// just from dividing a panel" / "dragging one frame folder lagged".
    /// The veil was redrawn EVERY frame, as one egui shape per screen row,
    /// for as long as a frame folder was in play — and item F makes that
    /// state far more common. One mesh instead.
    #[test]
    fn the_frame_veil_is_one_mesh_not_a_rect_per_row() {
        let (panels, area) = two_panels_slant_cut();
        let before = scanline_shape_count(&panels, area);
        let mesh = veil_mesh(
            &panels,
            area,
            egui::Color32::from_rgba_unmultiplied(96, 132, 255, 42),
        );
        println!("[veil] before: {before} shapes; after: 1 shape ({} tris)", mesh.indices.len() / 3);
        // 400 visible rows × the two gaps beside the panels = 800 shapes,
        // every frame, for a canvas only 400 pt tall. The owner's window is
        // ~1000 pt, and item F puts the veil on far more often.
        assert!(
            before >= 2 * area.height() as usize,
            "the scanline fill really was per-row ({before})"
        );
        // ONE shape reaches the painter, and its triangle budget is set by
        // the panels' vertices, not by the canvas height.
        assert!(
            mesh.indices.len() / 3 < 64,
            "{} triangles for two panels",
            mesh.indices.len() / 3
        );
        assert!(!mesh.is_empty());
    }

    /// The rewrite is a different DECOMPOSITION, not a different picture:
    /// every sampled point is covered by the mesh exactly when it is
    /// outside the panels by the even-odd rule (which is what makes a
    /// concave panel work).
    #[test]
    fn the_mesh_veil_covers_exactly_what_the_scanlines_did() {
        // A CONCAVE panel (an L) beside a plain one — the case even-odd
        // exists for.
        let panels = vec![
            vec![
                p(40.0, 40.0),
                p(260.0, 40.0),
                p(260.0, 300.0),
                p(160.0, 300.0),
                p(160.0, 150.0),
                p(40.0, 150.0),
            ],
            vec![p(320.0, 60.0), p(560.0, 60.0), p(560.0, 340.0), p(320.0, 340.0)],
        ];
        let area = egui::Rect::from_min_max(p(0.0, 0.0), p(600.0, 400.0));
        let mesh = veil_mesh(&panels, area, egui::Color32::WHITE);

        let inside_even_odd = |q: egui::Pos2| {
            let mut c = 0;
            for poly in &panels {
                let n = poly.len();
                for i in 0..n {
                    let (a, b) = (poly[i], poly[(i + 1) % n]);
                    if (a.y <= q.y) != (b.y <= q.y)
                        && a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x) < q.x
                    {
                        c += 1;
                    }
                }
            }
            c % 2 == 1
        };
        let covered = |q: egui::Pos2| {
            mesh.indices.chunks_exact(3).any(|t| {
                let (a, b, c) = (
                    mesh.vertices[t[0] as usize].pos,
                    mesh.vertices[t[1] as usize].pos,
                    mesh.vertices[t[2] as usize].pos,
                );
                let s = |u: egui::Pos2, v: egui::Pos2| (v.x - u.x) * (q.y - u.y) - (v.y - u.y) * (q.x - u.x);
                let (d0, d1, d2) = (s(a, b), s(b, c), s(c, a));
                (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0)
            })
        };
        // Off the band boundaries and the panel edges on purpose: a sample
        // ON a shared edge is covered by both answers and proves nothing.
        for gy in 0..40 {
            for gx in 0..60 {
                let q = p(gx as f32 * 10.0 + 5.3, gy as f32 * 10.0 + 5.7);
                assert_eq!(
                    covered(q),
                    !inside_even_odd(q),
                    "{q:?}: mesh says {}, even-odd says outside = {}",
                    covered(q),
                    !inside_even_odd(q)
                );
            }
        }
    }

    /// Owner 2026-09-05: "the blue overlay only hits when I click on the
    /// frame folder; it should apply whenever I'm on any layer in the
    /// frame folder". The veil resolves the folder by walking OUT of the
    /// active layer, and stays off for layers outside every frame folder.
    #[test]
    fn the_veil_shows_for_a_layer_inside_the_frame_folder() {
        let mut doc = mn_core::Document::new(600, 400);
        let outside = doc.active;
        let hi = doc.add_frame_folder(
            "Frame 1",
            mn_core::FrameSet::single_rect([40.0, 40.0, 560.0, 360.0], 2.0),
        );
        let draw = doc.active; // the folder's own draw layer
        assert!(draw != hi && doc.children_range(hi).contains(&draw));

        assert_eq!(veil_folder(&doc, hi), Some(hi), "the header itself");
        assert_eq!(veil_folder(&doc, draw), Some(hi), "a layer INSIDE it");
        // A layer added deep inside still belongs to the panel — and the
        // insert shifts the header's index, so re-find it by name (the
        // CODE-MAP warning about holding raw layer indices).
        let sub = doc.add_layer_in_folder(hi, "Tone").unwrap();
        let hi = doc.layers.iter().position(|l| l.name == "Frame 1").unwrap();
        assert_eq!(veil_folder(&doc, sub), Some(hi));
        assert_eq!(veil_folder(&doc, outside), None, "outside every panel");
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
