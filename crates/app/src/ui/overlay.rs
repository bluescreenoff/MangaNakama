//! Canvas overlay: everything painted in egui OVER the GPU canvas — page
//! shadow/border, manuscript guides, selection dashes, frame/balloon/text/
//! transform previews, the brush ring. Z-order here is sequential and
//! meaningful (shadow first, transform handles last). Takes &App only.

use super::theme;
use crate::app::App;
use crate::cmd::{BalloonMode, SelectMode, Tool};

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

/// Painted in egui, over the GPU canvas, clipped to the canvas area. Guides
/// go through `Viewport::to_screen`, so they survive pan/zoom/rotation.
pub(super) fn canvas_overlay(ui: &egui::Ui, app: &App, canvas_pts: egui::Rect) {
    let painter = ui.painter().with_clip_rect(canvas_pts);
    let ppp = app.shell.ppp;
    let to_pt = |cx: f32, cy: f32| {
        let (sx, sy) = app.viewport.to_screen(cx, cy);
        egui::pos2(sx / ppp, sy / ppp)
    };

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

    // SF-004/005 (TRIAGE 140): the generated run's BLUE reference line
    // and its driver handles — CSP's two-line model: the reference says
    // WHERE the set sits (moves/reshapes the run), the handles on it
    // drive the shape. Live during a drag via the shared spec math.
    //
    // Drawn for the SELECTED run and, under the Operation tool, for the
    // ACTIVE layer's run — nothing was drawn until a run was already
    // selected, so there was nothing on screen to aim the click at, and a
    // freshly placed set looked like it had no controls at all.
    let gen_li = app
        .gen_sel
        .filter(|li| {
            app.doc
                .layers
                .get(*li)
                .is_some_and(|l| l.genlines.is_some())
        })
        .or_else(|| {
            (app.tool == Tool::Object && app.doc.active_layer().genlines.is_some())
                .then_some(app.doc.active)
        });
    if let Some(li) = gen_li
        && let Some(spec) = app.doc.layers.get(li).and_then(|l| l.genlines)
    {
        let live = app
            .gen_drag
            .as_ref()
            .filter(|d| d.layer == li)
            .map(|d| crate::app::canvas_input::gen_drag_spec(d, app.doc.size))
            .unwrap_or(spec);
        let blue = egui::Color32::from_rgb(70, 130, 255);
        let pts = crate::app::canvas_input::gen_handle_points(&live, app.doc.size);
        if live.focus {
            // The reference: cross at the centre + the two radius circles.
            // `c` came through `to_pt`, so it is already a SCREEN POINT —
            // dividing the arm by the zoom on top of that shrank the cross
            // to a speck the further you zoomed in, which is exactly when
            // you want to see it. Screen sizes are literals here; only
            // canvas LENGTHS get scaled (and by `zoom / ppp`, the
            // px→point convention the brush ring uses).
            let c = to_pt(live.a, live.b);
            let arm = 10.0;
            for d in [egui::vec2(1.0, 0.0), egui::vec2(0.0, 1.0)] {
                painter.line_segment([c - d * arm, c + d * arm], egui::Stroke::new(1.5, blue));
            }
            for (r, dash) in [(live.c, true), (live.d, true)] {
                let rr = r * app.viewport.zoom.max(0.01) / ppp;
                // Dashed circle = 48 short arcs.
                const SEG: usize = 48;
                let step = std::f32::consts::TAU / SEG as f32;
                for k in (0..SEG).step_by(2) {
                    let a0 = k as f32 * step;
                    let p0 = c + egui::vec2(a0.cos(), a0.sin()) * rr;
                    let p1 = c + egui::vec2((a0 + step).cos(), (a0 + step).sin()) * rr;
                    painter.line_segment([p0, p1], egui::Stroke::new(1.0, blue));
                }
                let _ = dash;
            }
        } else {
            // The reference: the direction line through the run's own
            // anchor (the placing drag's midpoint), not the page centre —
            // a line drawn somewhere the gesture never went is not a
            // reference to anything.
            let a = crate::app::canvas_input::gen_anchor(&live, app.doc.size);
            let (sn, co) = live.a.to_radians().sin_cos();
            let dir = [co, sn];
            let p0 = to_pt(a[0] - dir[0] * live.c, a[1] - dir[1] * live.c);
            let p1 = to_pt(a[0] + dir[0] * live.c, a[1] + dir[1] * live.c);
            painter.line_segment([p0, p1], egui::Stroke::new(1.5, blue));
        }
        for (mode, p) in pts {
            let c = to_pt(p[0], p[1]);
            // A handle is a fixed size ON SCREEN (see the cross above).
            let r = 4.5;
            let rect = egui::Rect::from_center_size(c, egui::vec2(r * 2.0, r * 2.0));
            painter.rect_filled(rect, 1.0, egui::Color32::WHITE);
            let hot = app
                .gen_drag
                .as_ref()
                .is_some_and(|d| d.mode == mode && d.layer == li);
            painter.rect_stroke(
                rect,
                1.0,
                egui::Stroke::new(1.4, if hot { theme::c().accent } else { blue }),
                egui::StrokeKind::Inside,
            );
        }
    }

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

    // Selection outline (committed), plus previews for an in-progress drag or
    // a pixel move. Marching ants: dark dashes crawling along the path
    // (CSP-style), drawn in screen space so dash density stays uniform. A
    // live selection keeps the frame loop ticking so the crawl animates.
    let phase = (ui.ctx().input(|i| i.time) as f32 * 24.0) % 7.0;
    let ants = |pts: &[(f32, f32)], offset: (f32, f32), col: egui::Color32| {
        if pts.len() < 2 {
            return;
        }
        let mut path: Vec<egui::Pos2> = pts
            .iter()
            .map(|p| to_pt(p.0 + offset.0, p.1 + offset.1))
            .collect();
        path.push(path[0]);
        ants_line(&painter, &path, col, phase);
    };
    // TODO #3: rulers — cyan guide lines, clipped to the canvas. The gate
    // asks about EVERY family: a set holding only curve rulers (or only a
    // half-clicked one) drew nothing while this read `items` alone.
    if !app.doc.rulers.items.is_empty()
        || !app.doc.rulers.curves.is_empty()
        || app.curve_pending.is_some()
    {
        let col = egui::Color32::from_rgb(0, 200, 220);
        let far = 1.0e5;
        // Part 4 and the 1-/3-point variants share their furniture: the
        // eye level drawn strong through two points, and a vanishing point
        // marked by a faint 15° ray fan plus a cross.
        let eye_level = |a: [f32; 2], b: [f32; 2]| {
            let d = [b[0] - a[0], b[1] - a[1]];
            let n = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1.0);
            let u = [d[0] / n, d[1] / n];
            painter.line_segment(
                [
                    to_pt(a[0] - u[0] * far, a[1] - u[1] * far),
                    to_pt(b[0] + u[0] * far, b[1] + u[1] * far),
                ],
                egui::Stroke::new(1.5, col),
            );
        };
        let vp_mark = |vp: [f32; 2]| {
            let faint = egui::Color32::from_rgba_unmultiplied(0, 200, 220, 60);
            for k in 0..24 {
                let ang = k as f32 * std::f32::consts::TAU / 24.0;
                let dd = [ang.cos(), ang.sin()];
                painter.line_segment(
                    [
                        to_pt(vp[0] + dd[0] * 24.0, vp[1] + dd[1] * 24.0),
                        to_pt(vp[0] + dd[0] * 400.0, vp[1] + dd[1] * 400.0),
                    ],
                    egui::Stroke::new(0.5, faint),
                );
            }
            let p = to_pt(vp[0], vp[1]);
            painter.line_segment(
                [egui::pos2(p.x - 5.0, p.y), egui::pos2(p.x + 5.0, p.y)],
                egui::Stroke::new(1.5, col),
            );
            painter.line_segment(
                [egui::pos2(p.x, p.y - 5.0), egui::pos2(p.x, p.y + 5.0)],
                egui::Stroke::new(1.5, col),
            );
        };
        // Row 149: attached rulers draw only on their layer.
        let active_items = app.doc.rulers.for_layer(app.doc.active).items;
        for r in &active_items {
            match *r {
                mn_core::Ruler::Line { a, b } => {
                    let d = [b[0] - a[0], b[1] - a[1]];
                    let n = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1.0);
                    let u = [d[0] / n, d[1] / n];
                    let p0 = [a[0] - u[0] * far, a[1] - u[1] * far];
                    let p1 = [a[0] + u[0] * far, a[1] + u[1] * far];
                    painter.line_segment(
                        [to_pt(p0[0], p0[1]), to_pt(p1[0], p1[1])],
                        egui::Stroke::new(1.0, col),
                    );
                }
                mn_core::Ruler::VanishingPoint { c, rays, angle0 } => {
                    let n = rays.max(1);
                    for i in 0..n {
                        let ang = angle0 + i as f32 * std::f32::consts::TAU / n as f32;
                        let d = [ang.cos(), ang.sin()];
                        painter.line_segment(
                            [
                                to_pt(c[0], c[1]),
                                to_pt(c[0] + d[0] * far, c[1] + d[1] * far),
                            ],
                            egui::Stroke::new(1.0, col),
                        );
                    }
                }
                // Part 4: the eye level strong, faint 15° guide fans from
                // each VP, and a cross on each VP.
                mn_core::Ruler::Perspective { a, b } => {
                    eye_level(a, b);
                    vp_mark(a);
                    vp_mark(b);
                }
                // One-point: the eye level runs through the single VP; the
                // far end is a HANDLE (a ring, not a cross — it is not a
                // vanishing point, it only tilts the horizon).
                mn_core::Ruler::Perspective1 { vp, h } => {
                    eye_level(vp, h);
                    vp_mark(vp);
                    painter.circle_stroke(to_pt(h[0], h[1]), 3.0, egui::Stroke::new(1.5, col));
                }
                // Three-point: the horizon pair plus the vertical VP off
                // it — all three are real vanishing points, so all three
                // get the fan and the cross.
                mn_core::Ruler::Perspective3 { a, b, z } => {
                    eye_level(a, b);
                    vp_mark(a);
                    vp_mark(b);
                    vp_mark(z);
                }
                // Part 3 special rulers. Parallel renders its direction
                // segment extended; concentric draws its rings (capped);
                // guides span the canvas; symmetric draws its N axes.
                mn_core::Ruler::Parallel { a, b } => {
                    let d = [b[0] - a[0], b[1] - a[1]];
                    let n = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1.0);
                    let u = [d[0] / n, d[1] / n];
                    painter.line_segment(
                        [
                            to_pt(a[0] - u[0] * 40.0, a[1] - u[1] * 40.0),
                            to_pt(b[0] + u[0] * 40.0, b[1] + u[1] * 40.0),
                        ],
                        egui::Stroke::new(2.0, col),
                    );
                }
                mn_core::Ruler::Concentric { c, dr } => {
                    let reach = ((c[0]).abs().max(c[0]) + (c[1]).abs().max(c[1]) + 2048.0).max(dr);
                    for k in 1..=(reach / dr.max(1.0)) as usize {
                        let r = k as f32 * dr;
                        painter.circle_stroke(
                            to_pt(c[0], c[1]),
                            (r * app.viewport.zoom).max(1.0),
                            egui::Stroke::new(1.0, col),
                        );
                    }
                    // The centre mark.
                    painter.circle_stroke(to_pt(c[0], c[1]), 3.0, egui::Stroke::new(1.5, col));
                }
                mn_core::Ruler::Guide { horizontal, pos } => {
                    let (p0, p1) = if horizontal {
                        ([-1.0e4, pos], [1.0e4, pos])
                    } else {
                        ([pos, -1.0e4], [pos, 1.0e4])
                    };
                    painter.line_segment(
                        [to_pt(p0[0], p0[1]), to_pt(p1[0], p1[1])],
                        egui::Stroke::new(1.0, col),
                    );
                }
                mn_core::Ruler::Symmetric { c, lines, angle0 } => {
                    let n = lines.max(1);
                    for k in 0..n {
                        let ang = angle0 + k as f32 * std::f32::consts::PI / n as f32;
                        let d = [ang.cos(), ang.sin()];
                        painter.line_segment(
                            [
                                to_pt(c[0] - d[0] * far, c[1] - d[1] * far),
                                to_pt(c[0] + d[0] * far, c[1] + d[1] * far),
                            ],
                            egui::Stroke::new(1.0, col),
                        );
                    }
                    painter.circle_stroke(to_pt(c[0], c[1]), 3.0, egui::Stroke::new(1.5, col));
                }
            }
        }
        // Part 2: curve rulers (their finite path, not infinite lines) and
        // the in-progress curve being clicked out.
        for c in &app.doc.rulers.curves {
            for w in c.pts.windows(2) {
                painter.line_segment(
                    [to_pt(w[0][0], w[0][1]), to_pt(w[1][0], w[1][1])],
                    egui::Stroke::new(1.5, col),
                );
            }
        }
        if let Some(pts) = &app.curve_pending {
            for w in pts.windows(2) {
                painter.line_segment(
                    [to_pt(w[0][0], w[0][1]), to_pt(w[1][0], w[1][1])],
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 220, 235)),
                );
            }
        }
        // Rulers are movable with the Object tool, so under that tool they
        // wear the same handle every other on-canvas anchor does (white
        // square, accent outline). A guide has no anchor — its whole line
        // is the grab — so it shows none, which is the honest affordance.
        if app.tool == Tool::Object {
            let outline = egui::Stroke::new(1.2, theme::c().accent);
            let square = |c: egui::Pos2| {
                let r = egui::Rect::from_center_size(c, egui::vec2(7.0, 7.0));
                painter.rect_filled(r, 1.0, egui::Color32::WHITE);
                painter.rect_stroke(r, 1.0, outline, egui::StrokeKind::Inside);
            };
            // M3 phase A: the handle's SHAPE says what it is before the
            // label is read — a vanishing point is a diamond (the set
            // aims at it), the 1-pt set's horizon handle a circle (it
            // only tilts the eye level), every other anchor the plain
            // square every on-canvas affordance wears.
            let handle = |p: [f32; 2], role: mn_core::AnchorRole| {
                let c = to_pt(p[0], p[1]);
                match role {
                    mn_core::AnchorRole::Vp(_) | mn_core::AnchorRole::VerticalVp => {
                        let d = 5.5;
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                c + egui::vec2(0.0, -d),
                                c + egui::vec2(d, 0.0),
                                c + egui::vec2(0.0, d),
                                c + egui::vec2(-d, 0.0),
                            ],
                            egui::Color32::WHITE,
                            outline,
                        ));
                    }
                    mn_core::AnchorRole::Horizon => {
                        painter.circle(c, 4.0, egui::Color32::WHITE, outline);
                    }
                    _ => square(c),
                }
            };
            // …and the small tag spells it out ("VP1", "eye level"). Theme
            // type, set to the handle's RIGHT so it never covers the mark
            // it names — the labels teach the set, they don't compete with
            // the art.
            let font = egui::TextStyle::Small.resolve(ui.style());
            for r in &active_items {
                for (p, role) in r.anchors_with_roles() {
                    handle(p, role);
                    painter.text(
                        to_pt(p[0], p[1]) + egui::vec2(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        role.tag(),
                        font.clone(),
                        theme::c().accent,
                    );
                }
            }
            // A curve ruler's vertices are just path points — no role to
            // name, so they keep the plain square.
            for c in &app.doc.rulers.curves {
                for p in c.anchors() {
                    square(to_pt(p[0], p[1]));
                }
            }
        }
    }
    let dark = egui::Color32::from_gray(25);
    let white = egui::Color32::from_gray(235);
    let mut ants_live = false;
    if let Some((start, cur)) = &app.select_moving {
        if let Some(sel) = &app.doc.selection {
            let off = (cur.0 - start.0, cur.1 - start.1);
            ants(&sel.outline, off, white);
            for l in &sel.extra_outlines {
                ants(l, off, white);
            }
            ants_live = true;
        }
    } else if let Some(sel) = &app.doc.selection {
        ants(&sel.outline, (0.0, 0.0), dark);
        for l in &sel.extra_outlines {
            ants(l, (0.0, 0.0), dark);
        }
        ants_live = true;
    }
    if ants_live {
        // 50 ms steps read as a smooth crawl at 24 px/s without spinning the
        // GPU when the app is otherwise idle.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
    if let Some(pts) = &app.select_drag {
        let col = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 160);
        match app.select_mode {
            SelectMode::Rect if pts.len() >= 2 => {
                let (a, b) = (pts[0], pts[1]);
                ants(&[a, (b.0, a.1), b, (a.0, b.1)], (0.0, 0.0), col);
            }
            SelectMode::Lasso | SelectMode::Shrink => ants(pts, (0.0, 0.0), col),
            _ => {}
        }
    }
    // FI-003 / FI-004: the fill loop in progress. Same marching ants as the
    // lasso — it is the same gesture — but tinted so the two are not
    // mistaken for each other mid-drag.
    if let Some(pts) = &app.fill_drag {
        ants(
            pts,
            (0.0, 0.0),
            egui::Color32::from_rgba_unmultiplied(255, 214, 110, 200),
        );
    }
    // Selection-paint live preview (SE round 2026-08-19): the stroke's
    // scratch coverage traced per frame — the ants crawl under the brush
    // while it paints the selection. Bbox-traced over the SPARSE tiles,
    // so cost tracks the stroke, not the canvas.
    if app.sel_paint_active() && !app.doc.sel_scratch.tiles.is_empty() {
        let preview = mn_core::selection::scratch_outlines(&app.doc.sel_scratch);
        let col = egui::Color32::from_rgb(120, 220, 235);
        for l in &preview {
            ants(l, (0.0, 0.0), col);
        }
        if !preview.is_empty() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    // Polyline frame: placed vertices + rubber band to the pointer; a ring on
    // the first vertex marks the close target. Pen frame: the freehand trail.
    if let Some(pts) = &app.frame_poly {
        let mut line: Vec<egui::Pos2> = pts.iter().map(|p| to_pt(p.0, p.1)).collect();
        let (lx, ly) = app.last_pointer;
        let cursor = egui::pos2(lx as f32 / ppp, ly as f32 / ppp);
        if canvas_pts.contains(cursor) {
            line.push(cursor);
        }
        painter.add(egui::Shape::line(
            line,
            egui::Stroke::new(1.5, theme::c().accent),
        ));
        if let Some(first) = pts.first() {
            painter.circle_stroke(
                to_pt(first.0, first.1),
                5.0,
                egui::Stroke::new(1.5, theme::c().accent),
            );
        }
        for p in pts {
            painter.circle_filled(to_pt(p.0, p.1), 2.5, theme::c().accent);
        }
    }
    if let Some(pts) = &app.frame_pen {
        let line: Vec<egui::Pos2> = pts.iter().map(|p| to_pt(p.0, p.1)).collect();
        painter.add(egui::Shape::line(
            line,
            egui::Stroke::new(1.5, theme::c().accent),
        ));
    }

    // Frame tools: divide-drag preview (a line for cuts, a box for the
    // rectangle sub tool), and the Object tool's selected frame with
    // move/reshape handles (live preview during a drag).
    if let Some((a, b)) = &app.frame_drag {
        if app.frame_mode == crate::cmd::FrameMode::Rect {
            painter.add(egui::Shape::line(
                vec![
                    to_pt(a.0, a.1),
                    to_pt(b.0, a.1),
                    to_pt(b.0, b.1),
                    to_pt(a.0, b.1),
                    to_pt(a.0, a.1),
                ],
                egui::Stroke::new(2.0, theme::c().accent),
            ));
        } else {
            // The divide preview shows the GUTTER, not just the cut (owner,
            // 2026-08-20, CSP behaviour): two parallel lines at the exact
            // width the release will carve — the same angle-blended formula
            // as AppCmd::FrameDivide, so a drag that tilts from horizontal
            // to vertical watches its gutter change width live.
            let (g_h, g_v) = if app.frame_mode == crate::cmd::FrameMode::DivideBorder {
                app.gutter_border_mm
            } else {
                app.gutter_folder_mm
            };
            let ang = (b.1 - a.1).atan2(b.0 - a.0);
            let gutter = app.mm_to_px(g_v) * ang.cos().abs() + app.mm_to_px(g_h) * ang.sin().abs();
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            if gutter > 0.0 && len > 1.0 {
                let (nx, ny) = (-dy / len, dx / len);
                let (hx, hy) = (nx * gutter * 0.5, ny * gutter * 0.5);
                for s in [-1.0f32, 1.0] {
                    painter.line_segment(
                        [
                            to_pt(a.0 + hx * s, a.1 + hy * s),
                            to_pt(b.0 + hx * s, b.1 + hy * s),
                        ],
                        egui::Stroke::new(1.5, theme::c().accent),
                    );
                }
            } else {
                painter.line_segment(
                    [to_pt(a.0, a.1), to_pt(b.0, b.1)],
                    egui::Stroke::new(2.0, theme::c().accent),
                );
            }
        }
    }

    // THE DRAGGED FRAME STAYS LEGIBLE WHEN IT IS UNDERNEATH (owner HIGH,
    // 2026-08-18, f160fba; narrowed 2026-08-21): while an Object-tool
    // drag is live, paint the panel's real polygon PANEL WHITE above the
    // composite — the white it will actually cover with is the
    // information the artist is reading during a move/resize of an
    // occluded panel. Owner feedback killed the mere-selection washes
    // (208 under the Object tool, 160 on plain list selection): they
    // lightened the ink inside the panel every time a frame folder was
    // selected, and the blue outside-veil above already says which
    // regions are panels. Pure overlay: this painter only runs on the
    // live canvas — offscreen renders, exports and the reader never see
    // it.
    if app.tool == Tool::Object
        && app.object_sel.is_some()
        && let Some(shown) = app.object_drag.as_ref().map(|d| d.preview())
    {
        let pts: Vec<egui::Pos2> = shown.points.iter().map(|p| to_pt(p[0], p[1])).collect();
        fill_polygon(&painter, &pts, egui::Color32::from_white_alpha(255));
    }

    if app.tool == Tool::Object {
        // Vector inking phase 2: the selected recorded stroke, live —
        // during a drag the geometry moves (the raster re-derives at
        // release), so the polyline IS the honest preview. Handles at the
        // endpoints and every 8th sample (every sample would be fog; the
        // hit test still accepts any of them).
        if let Some(si) = app.vector_sel
            && let Some(s) = app
                .doc
                .active_layer()
                .strokes
                .as_ref()
                .and_then(|set| set.strokes.get(si))
        {
            let pts: Vec<egui::Pos2> = s.points.iter().map(|p| to_pt(p.0, p.1)).collect();
            painter.add(egui::Shape::line(
                pts.clone(),
                egui::Stroke::new(1.5, theme::c().accent),
            ));
            // Two handle classes, Clip Studio's convention: SQUARES are the
            // special points — today the stroke's two ENDPOINTS, and the
            // shape reserved for corner points when they land — CIRCLES the
            // ordinary handles between them. Rendering only: both classes
            // come out of the one `handle_indices` list the hit test grabs,
            // so what you see is still exactly what you can grab.
            let last = s.points.len().saturating_sub(1);
            for i in crate::app::vector_edit::handle_indices(s) {
                if i == 0 || i == last {
                    let hrect = egui::Rect::from_center_size(pts[i], egui::vec2(7.0, 7.0));
                    painter.rect_filled(hrect, 1.0, egui::Color32::WHITE);
                    painter.rect_stroke(
                        hrect,
                        1.0,
                        egui::Stroke::new(1.2, theme::c().accent),
                        egui::StrokeKind::Inside,
                    );
                } else {
                    painter.circle_filled(pts[i], 3.4, egui::Color32::WHITE);
                    painter.circle_stroke(pts[i], 3.4, egui::Stroke::new(1.2, theme::c().accent));
                }
            }
        }
        if let Some((li, fi)) = app.object_sel {
            let shown = match &app.object_drag {
                Some(d) => Some(d.preview()),
                None => app
                    .doc
                    .layers
                    .get(li)
                    .and_then(|l| l.frames())
                    .and_then(|fs| fs.frames.get(fi))
                    .cloned(),
            };
            if let Some(f) = shown {
                let mut pts: Vec<egui::Pos2> = f.points.iter().map(|p| to_pt(p[0], p[1])).collect();
                if let Some(first) = pts.first().copied() {
                    pts.push(first);
                }
                painter.add(egui::Shape::line(
                    pts,
                    egui::Stroke::new(1.5, theme::c().accent),
                ));
                for p in &f.points {
                    let c = to_pt(p[0], p[1]);
                    let hrect = egui::Rect::from_center_size(c, egui::vec2(7.0, 7.0));
                    painter.rect_filled(hrect, 1.0, egui::Color32::WHITE);
                    painter.rect_stroke(
                        hrect,
                        1.0,
                        egui::Stroke::new(1.2, theme::c().accent),
                        egui::StrokeKind::Inside,
                    );
                }

                // CSP Object affordances on the selected panel: the page in
                // blue, the panel bbox in red, 8 bbox handles (corners =
                // scale, edge mids = stretch), a rotation lollipop above the
                // top-centre, and yellow double arrows on edges a sibling
                // shares (the folder gutter) or that lie along a template
                // border line (trim/bleed/inner/safety — owner
                // clarification 2026-08-15).
                let b = f.bbox();
                let (bl, br) = (to_pt(0.0, 0.0), to_pt(w as f32, h as f32));
                painter.rect_stroke(
                    egui::Rect::from_min_max(bl, br),
                    0.0,
                    egui::Stroke::new(1.2, egui::Color32::from_rgb(96, 148, 255)),
                    egui::StrokeKind::Inside,
                );
                let (tl, tr) = (to_pt(b[0], b[1]), to_pt(b[2], b[3]));
                let brect = egui::Rect::from_min_max(tl, tr);
                painter.rect_stroke(
                    brect,
                    0.0,
                    egui::Stroke::new(1.2, egui::Color32::from_rgb(232, 76, 60)),
                    egui::StrokeKind::Inside,
                );
                let handle = |c: egui::Pos2| {
                    let hr = egui::Rect::from_center_size(c, egui::vec2(8.0, 8.0));
                    painter.rect_filled(hr, 1.0, egui::Color32::WHITE);
                    painter.rect_stroke(
                        hr,
                        1.0,
                        egui::Stroke::new(1.2, theme::c().accent),
                        egui::StrokeKind::Inside,
                    );
                };
                handle(brect.left_top());
                handle(brect.right_top());
                handle(brect.right_bottom());
                handle(brect.left_bottom());
                handle(egui::pos2(brect.center().x, brect.top()));
                handle(egui::pos2(brect.right(), brect.center().y));
                handle(egui::pos2(brect.center().x, brect.bottom()));
                handle(egui::pos2(brect.left(), brect.center().y));
                // Rotation lollipop: stem up from the top-centre.
                let stem0 = egui::pos2(brect.center().x, brect.top());
                let stem1 = egui::pos2(stem0.x, stem0.y - crate::app::ROTATE_STALK_SCREEN);
                painter.line_segment([stem0, stem1], egui::Stroke::new(1.2, theme::c().accent));
                painter.circle_stroke(stem1, 4.5, egui::Stroke::new(1.5, theme::c().accent));
                // Shared-gutter markers (yellow, along the shared edge).
                let yellow = egui::Color32::from_rgb(238, 198, 60);
                let tpl = template_lines(app);
                let n = f.points.len();
                let sibs = app
                    .doc
                    .layers
                    .get(li)
                    .and_then(|l| l.frames())
                    .map(|fs| &fs.frames);
                for i in 0..n {
                    let (a, c2) = (f.points[i], f.points[(i + 1) % n]);
                    let abx = c2[0] - a[0];
                    let aby = c2[1] - a[1];
                    let len2 = abx * abx + aby * aby;
                    if len2 < 64.0 {
                        continue;
                    }
                    let shared = sibs.is_some_and(|frames| {
                        frames.iter().enumerate().any(|(si, sib)| {
                            si != fi
                                && sib.points.iter().any(|p| {
                                    let t = ((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len2;
                                    (0.0..=1.0).contains(&t) && {
                                        let px = a[0] + abx * t - p[0];
                                        let py = a[1] + aby * t - p[1];
                                        px * px + py * py < 2.25
                                    }
                                })
                        })
                    }) || edge_on_template(a, c2, &tpl);
                    if shared {
                        let m = to_pt((a[0] + c2[0]) * 0.5, (a[1] + c2[1]) * 0.5);
                        let (ux, uy) = (abx / len2.sqrt(), aby / len2.sqrt());
                        let arrow = |dir: f32| {
                            let tip = egui::pos2(m.x + ux * 7.0 * dir, m.y + uy * 7.0 * dir);
                            let back = egui::pos2(
                                tip.x - ux * 5.0 * dir - uy * 3.0,
                                tip.y - uy * 5.0 * dir + ux * 3.0,
                            );
                            let back2 = egui::pos2(
                                tip.x - ux * 5.0 * dir + uy * 3.0,
                                tip.y - uy * 5.0 * dir - ux * 3.0,
                            );
                            painter.line_segment([back, tip], egui::Stroke::new(1.6, yellow));
                            painter.line_segment([back2, tip], egui::Stroke::new(1.6, yellow));
                        };
                        arrow(1.0);
                        arrow(-1.0);
                    }
                }
                // EXPAND arrows (owner ask 2026-08-26, CSP's yellow
                // triangles): one OUTWARD-pointing triangle just outside
                // each bbox edge that has a neighbour border or template
                // line to grow to — tap = the gutter dies there. Distinct
                // from the shared-edge double arrows above: those mark an
                // EXISTING alignment; these offer the next one.
                for (dir, tip) in app.frame_expand_arrow_pts() {
                    let (dx, dy) = match dir {
                        0 => (-1.0, 0.0),
                        1 => (1.0, 0.0),
                        2 => (0.0, -1.0),
                        _ => (0.0, 1.0),
                    };
                    let back =
                        egui::pos2(tip.x - dx * 10.0 - dy * 5.0, tip.y - dy * 10.0 + dx * 5.0);
                    let back2 =
                        egui::pos2(tip.x - dx * 10.0 + dy * 5.0, tip.y - dy * 10.0 - dx * 5.0);
                    painter.line_segment([back, tip], egui::Stroke::new(2.2, yellow));
                    painter.line_segment([back2, tip], egui::Stroke::new(2.2, yellow));
                }
            }
        }
        if let Some((li, bi)) = app.balloon_sel {
            let shown = match &app.balloon_obj_drag {
                Some(d) => Some(d.preview()),
                None => app
                    .doc
                    .layers
                    .get(li)
                    .and_then(|l| l.balloons())
                    .and_then(|bs| bs.balloons.get(bi))
                    .cloned(),
            };
            if let Some(b) = shown {
                for line in balloon_outline(&b) {
                    let pts: Vec<egui::Pos2> = line.iter().map(|p| to_pt(p[0], p[1])).collect();
                    painter.add(egui::Shape::line(
                        pts,
                        egui::Stroke::new(1.5, theme::c().accent),
                    ));
                }
                for (pos, h) in b.handles() {
                    let c = to_pt(pos[0], pos[1]);
                    // Tail handles are round so they read differently.
                    if matches!(h, mn_core::BalloonHandle::Shape(_)) {
                        let hrect = egui::Rect::from_center_size(c, egui::vec2(7.0, 7.0));
                        painter.rect_filled(hrect, 1.0, egui::Color32::WHITE);
                        painter.rect_stroke(
                            hrect,
                            1.0,
                            egui::Stroke::new(1.2, theme::c().accent),
                            egui::StrokeKind::Inside,
                        );
                    } else {
                        painter.circle_filled(c, 3.6, egui::Color32::WHITE);
                        painter.circle_stroke(c, 3.6, egui::Stroke::new(1.2, theme::c().accent));
                    }
                }
                // The Operation tool's blue box (CSP shows one around any
                // selected object): the transform frame with 8 handles and a
                // rotation lollipop — the SAME affordance language as frames.
                let bb = b.bbox();
                let a = to_pt(bb[0], bb[1]);
                let c = to_pt(bb[2], bb[3]);
                let boxr = egui::Rect::from_min_max(a, c);
                painter.rect_stroke(
                    boxr,
                    0.0,
                    egui::Stroke::new(1.2, theme::c().accent),
                    egui::StrokeKind::Inside,
                );
                let mid =
                    |p: egui::Pos2, q: egui::Pos2| egui::pos2((p.x + q.x) * 0.5, (p.y + q.y) * 0.5);
                let corners = [
                    boxr.left_top(),
                    boxr.right_top(),
                    boxr.right_bottom(),
                    boxr.left_bottom(),
                ];
                let mids = [
                    mid(boxr.left_top(), boxr.right_top()),
                    mid(boxr.right_top(), boxr.right_bottom()),
                    mid(boxr.right_bottom(), boxr.left_bottom()),
                    mid(boxr.left_bottom(), boxr.left_top()),
                ];
                for p in corners.into_iter().chain(mids) {
                    let r = egui::Rect::from_center_size(p, egui::vec2(8.0, 8.0));
                    painter.rect_filled(r, 1.0, theme::c().panel);
                    painter.rect_stroke(
                        r,
                        1.0,
                        egui::Stroke::new(1.4, theme::c().accent),
                        egui::StrokeKind::Inside,
                    );
                }
                // Rotation lollipop above the box's top edge.
                let lolly = egui::pos2(
                    boxr.center().x,
                    boxr.top() - crate::app::ROTATE_STALK_SCREEN,
                );
                painter.line_segment(
                    [egui::pos2(boxr.center().x, boxr.top()), lolly],
                    egui::Stroke::new(1.2, theme::c().accent),
                );
                painter.circle_filled(lolly, 4.5, theme::c().panel);
                painter.circle_stroke(lolly, 4.5, egui::Stroke::new(1.4, theme::c().accent));
            }
        }
    }

    // Balloon tool: live preview of the drag (bubble outline / freehand trail /
    // tail line).
    if let Some(pts) = &app.balloon_drag {
        let col = theme::c().accent;
        match app.balloon_mode {
            BalloonMode::Draw => {
                let line: Vec<egui::Pos2> = pts.iter().map(|p| to_pt(p[0], p[1])).collect();
                painter.add(egui::Shape::line(line, egui::Stroke::new(1.5, col)));
            }
            BalloonMode::Tail => {
                if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
                    painter.line_segment(
                        [to_pt(a[0], a[1]), to_pt(b[0], b[1])],
                        egui::Stroke::new(2.0, col),
                    );
                }
            }
            BalloonMode::Ellipse | BalloonMode::Round => {
                if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
                    let shape = if app.balloon_mode == BalloonMode::Ellipse {
                        mn_core::BalloonShape::Ellipse {
                            center: [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5],
                            radii: [(b[0] - a[0]).abs() * 0.5, (b[1] - a[1]).abs() * 0.5],
                        }
                    } else {
                        let (w, h) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
                        mn_core::BalloonShape::RoundRect {
                            rect: [
                                a[0].min(b[0]),
                                a[1].min(b[1]),
                                a[0].max(b[0]),
                                a[1].max(b[1]),
                            ],
                            corner: w.min(h) * 0.25,
                        }
                    };
                    let b = mn_core::Balloon {
                        shape,
                        tails: Vec::new(),
                        ..Default::default()
                    };
                    for line in balloon_outline(&b) {
                        let p: Vec<egui::Pos2> = line.iter().map(|q| to_pt(q[0], q[1])).collect();
                        painter.add(egui::Shape::line(p, egui::Stroke::new(1.5, col)));
                    }
                }
            }
        }
    }

    // Figure tool: live preview of the shape being dragged / the polygon
    // vertices placed so far.
    if let Some((a, b)) = &app.figure_drag {
        let col = theme::c().accent;
        let pts: Vec<egui::Pos2> = app
            .figure_path(*a, *b)
            .iter()
            .map(|p| to_pt(p[0], p[1]))
            .collect();
        if pts.len() >= 2 {
            painter.add(egui::Shape::line(pts.clone(), egui::Stroke::new(1.5, col)));
            painter.circle_filled(pts[0], 3.0, col);
            painter.circle_filled(*pts.last().expect("path"), 3.0, col);
        }
    }
    if let Some(pts) = &app.figure_poly {
        let col = theme::c().accent;
        for p in pts {
            let c = to_pt(p.0, p.1);
            painter.circle_filled(c, 3.2, col);
            painter.circle_stroke(c, 3.2, egui::Stroke::new(1.0, theme::c().panel));
        }
        if pts.len() >= 2 {
            // Rows 84/85: the Curve sub tool previews the SPLINE it will
            // ink, not the chords between the clicks — otherwise the shape
            // you are judging is not the shape you get.
            let path: Vec<[f32; 2]> = pts.iter().map(|p| [p.0, p.1]).collect();
            let shape = if app.figure_mode == crate::cmd::FigureMode::Curve {
                mn_core::balloon::tessellate_open(&path)
            } else {
                path
            };
            let line: Vec<egui::Pos2> = shape.iter().map(|p| to_pt(p[0], p[1])).collect();
            painter.add(egui::Shape::line(line, egui::Stroke::new(1.2, col)));
        }
        // Rubber line to the pointer (client px → canvas).
        let (lx, ly) = app.last_pointer;
        let (mx, my) = app.viewport.to_canvas(lx as f32, ly as f32);
        if let Some(last) = pts.last() {
            painter.line_segment(
                [to_pt(last.0, last.1), to_pt(mx, my)],
                egui::Stroke::new(1.0, theme::c().text_weak),
            );
        }
    }

    // Gradient tool: the ramp line with end markers.
    if let Some((a, b)) = &app.grad_drag {
        let col = theme::c().accent;
        let pa = to_pt(a.0, a.1);
        let pb = to_pt(b.0, b.1);
        painter.line_segment([pa, pb], egui::Stroke::new(2.0, col));
        painter.circle_filled(pa, 4.0, col);
        painter.circle_filled(pb, 4.0, col);
    }

    // Text: wrap-box drag preview, the edited/selected box with its handles,
    // the caret, and the selection highlight.
    if let Some(crate::text_edit::TextGesture::Box { start, cur }) = &app.text_gesture {
        let col = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 160);
        ants(
            &[*start, (cur.0, start.1), *cur, (start.0, cur.1)],
            (0.0, 0.0),
            col,
        );
    }
    let rot_off = crate::app::ROTATE_STALK_SCREEN / app.viewport.zoom.max(0.01);
    let text_shown: Option<(mn_core::TextItem, bool)> = if let Some(d) = &app.text_obj_drag {
        Some((d.preview(), true))
    } else if app.tool == Tool::Object {
        // Rows 78/76: the multi-selection set — a thin accent box on
        // every member (the primary keeps its full affordances below).
        // While a group drag is live the boxes ride the delta, so the
        // whole set visibly moves together before anything commits.
        let gd = app
            .group_drag
            .as_ref()
            .map(|d| (d.cur.0 - d.start.0, d.cur.1 - d.start.1));
        for r in &app.object_multi {
            let bb: Option<[f32; 4]> = match *r {
                crate::app::ObjRef::Text(li, ti) => app
                    .doc
                    .layers
                    .get(li)
                    .and_then(|l| l.texts())
                    .and_then(|ts| ts.texts.get(ti))
                    .map(|t| {
                        let c = t.center();
                        [c[0] - t.size[0] * 0.5, c[1] - t.size[1] * 0.5,
                         c[0] + t.size[0] * 0.5, c[1] + t.size[1] * 0.5]
                    }),
                crate::app::ObjRef::Balloon(li, bi) => app
                    .doc
                    .layers
                    .get(li)
                    .and_then(|l| l.balloons())
                    .and_then(|bs| bs.balloons.get(bi))
                    .map(|b| b.bbox()),
                crate::app::ObjRef::Frame(li, fi) => {
                    // Whole-panel box — the folder's own frame polygon.
                    app.doc
                        .layers
                        .get(li)
                        .and_then(|l| l.frames())
                        .and_then(|fs| fs.frames.get(fi))
                        .map(|f| f.bbox())
                }
                crate::app::ObjRef::Gen(li) => {
                    // Focus runs carry a centre and an outer radius.
                    app.doc
                        .layers
                        .get(li)
                        .and_then(|l| l.genlines.clone())
                        .and_then(|s| {
                            (s.focus && s.d > 0.0).then(|| {
                                [s.a - s.d, s.b - s.d, s.a + s.d, s.b + s.d]
                            })
                        })
                }
            };
            if let Some(mut b) = bb {
                if let Some((dx, dy)) = gd {
                    b = [b[0] + dx, b[1] + dy, b[2] + dx, b[3] + dy];
                }
                painter.rect_stroke(
                    egui::Rect::from_min_max(to_pt(b[0], b[1]), to_pt(b[2], b[3])),
                    2.0,
                    egui::Stroke::new(1.5, theme::c().accent),
                    egui::StrokeKind::Middle,
                );
            }
        }
        app.text_sel.and_then(|(li, ti)| {
            Some((
                app.doc.layers.get(li)?.texts()?.texts.get(ti)?.clone(),
                true,
            ))
        })
    } else {
        app.edited_item().map(|i| (i.clone(), false))
    };
    if let Some((item, with_handles)) = text_shown {
        let mut pts: Vec<egui::Pos2> = item.corners().iter().map(|p| to_pt(p[0], p[1])).collect();
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
        painter.add(egui::Shape::line(
            pts,
            egui::Stroke::new(1.2, theme::c().accent),
        ));
        if with_handles {
            for (pos, h) in item.handles(rot_off) {
                let c = to_pt(pos[0], pos[1]);
                if h == mn_core::TextHandle::Rotate {
                    // Stem from the top edge to the lollipop.
                    let top = item.to_canvas([item.size[0] * 0.5, 0.0]);
                    painter.line_segment(
                        [to_pt(top[0], top[1]), c],
                        egui::Stroke::new(1.0, theme::c().accent),
                    );
                    painter.circle_filled(c, 4.0, egui::Color32::WHITE);
                    painter.circle_stroke(c, 4.0, egui::Stroke::new(1.2, theme::c().accent));
                } else {
                    let hrect = egui::Rect::from_center_size(c, egui::vec2(7.0, 7.0));
                    painter.rect_filled(hrect, 1.0, egui::Color32::WHITE);
                    painter.rect_stroke(
                        hrect,
                        1.0,
                        egui::Stroke::new(1.2, theme::c().accent),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
    }
    if let Some(ov) = app.text_caret_overlay() {
        for quad in &ov.selection {
            let pts: Vec<egui::Pos2> = quad.iter().map(|p| to_pt(p[0], p[1])).collect();
            painter.add(egui::Shape::convex_polygon(
                pts,
                egui::Color32::from_rgba_unmultiplied(110, 150, 240, 70),
                egui::Stroke::NONE,
            ));
        }
        // THE CARET BLINKS AND IS VISIBLE ON WHITE (owner report,
        // 2026-08-19: it was near-white — `from_gray(245)` — on a white
        // page, and it never blinked, so there was nothing to catch the eye
        // even where it did show).
        //
        // Two strokes, dark over light: a manga page is white where the
        // caret usually sits and black where the ink is, so a single colour
        // disappears against one of them. The halo is the same trick the
        // brush-size ring below uses.
        //
        // The blink is driven by egui's own clock (~1.06 s period, close to
        // the Windows default) and asks for a repaint at the next phase
        // change — without that request an idle window would freeze the
        // caret in whichever half it last drew, which is worse than not
        // blinking at all.
        let t = ui.input(|i| i.time);
        const BLINK: f64 = 1.06;
        let phase = t.rem_euclid(BLINK);
        let on = phase < BLINK * 0.5;
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(
                (if on { BLINK * 0.5 } else { BLINK }) - phase + 0.005,
            ));
        if on {
            let [a, b] = ov.caret;
            let (pa, pb) = (to_pt(a[0], a[1]), to_pt(b[0], b[1]));
            painter.line_segment(
                [pa, pb],
                egui::Stroke::new(3.2, egui::Color32::from_white_alpha(190)),
            );
            painter.line_segment(
                [pa, pb],
                egui::Stroke::new(1.4, egui::Color32::from_black_alpha(235)),
            );
        }
    }

    // Brush-size ring at the cursor (stroke tools over the canvas only, and
    // only while the pointer is actually over the window — a lifted pen sends
    // WM_POINTERLEAVE and the ring hides instead of parking).
    if app.tool.strokes() && !app.drawing() && app.pointer_visible {
        let (lx, ly) = app.last_pointer;
        let pos = egui::pos2(lx as f32 / ppp, ly as f32 / ppp);
        if canvas_pts.contains(pos) {
            let r = (app.brush_radius() * app.viewport.zoom / ppp).max(1.0);
            if r < 500.0 {
                painter.circle_stroke(
                    pos,
                    r,
                    egui::Stroke::new(1.2, egui::Color32::from_black_alpha(160)),
                );
                painter.circle_stroke(
                    pos,
                    r + 1.0,
                    egui::Stroke::new(1.0, egui::Color32::from_white_alpha(120)),
                );
            }
        }
    }

    // The two "show me the area" tints (LM-008 mask, TN-011 tone) draw the
    // same way: canvas-space quads, because the view rotates and mirrors so
    // an axis-aligned rect on screen would be wrong. One `Shape` per RUN of
    // pixels rather than one painter call per pixel — the mask version used
    // to emit a 1 px `line_segment` per pixel of every mask tile every frame,
    // which is a painter call per pixel on the UI thread.
    let quad = |x0: f32, y0: f32, x1: f32, y1: f32, col: egui::Color32| {
        egui::Shape::convex_polygon(
            vec![to_pt(x0, y0), to_pt(x1, y0), to_pt(x1, y1), to_pt(x0, y1)],
            col,
            egui::Stroke::NONE,
        )
    };
    // E-017 the picker circle: while the Eyedropper is up, the ring under
    // the pen shows the colour a click would TAKE (upper arc) against the
    // colour you are drawing with now (lower arc) — CSP's "hovered on top,
    // current below", minus the magnifier. The centre stays hollow so the
    // pixel being sampled is never hidden by the readout of itself, and the
    // box the average covers is outlined in canvas space whenever it is
    // bigger than one pixel (a 5×5 pick has to show its five).
    //
    // Costs one `pick_color` per paint — the same walk one click does, and
    // only while this tool is up with the pointer over the canvas.
    if app.tool == Tool::Eyedrop && app.eyedrop_opts.circle && app.pointer_visible {
        let (lx, ly) = app.last_pointer;
        let pos = egui::pos2(lx as f32 / ppp, ly as f32 / ppp);
        let (cx, cy) = app.viewport.to_canvas(lx as f32, ly as f32);
        if canvas_pts.contains(pos)
            && let Some([r, g, b]) =
                crate::cmd::pick_color(&app.doc, cx as i32, cy as i32, app.eyedrop_opts)
        {
            let picked = egui::Color32::from_rgb(r, g, b);
            let [cr, cg, cb] = app.active_color();
            let current = egui::Color32::from_rgb(
                (cr * 255.0).round() as u8,
                (cg * 255.0).round() as u8,
                (cb * 255.0).round() as u8,
            );
            const RING: f32 = 15.0;
            const BAND: f32 = 6.0;
            let arc = |from: f32, to: f32| -> Vec<egui::Pos2> {
                (0..=24)
                    .map(|i| {
                        let a = from + (to - from) * i as f32 / 24.0;
                        pos + egui::vec2(a.cos(), a.sin()) * RING
                    })
                    .collect()
            };
            use std::f32::consts::PI;
            painter.add(egui::Shape::line(
                arc(PI, 2.0 * PI),
                egui::Stroke::new(BAND, picked),
            ));
            painter.add(egui::Shape::line(
                arc(0.0, PI),
                egui::Stroke::new(BAND, current),
            ));
            // Hairlines both sides: a pick can be white or black and the
            // ring has to survive both (the brush ring's trick).
            painter.circle_stroke(
                pos,
                RING - BAND * 0.5,
                egui::Stroke::new(1.0, egui::Color32::from_black_alpha(150)),
            );
            painter.circle_stroke(
                pos,
                RING + BAND * 0.5,
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(130)),
            );
            if app.eyedrop_opts.size > 1
                && let Some((bx, by, bw, bh)) = mn_core::export::sample_box(
                    app.doc.size,
                    cx as i32,
                    cy as i32,
                    app.eyedrop_opts.size,
                )
            {
                // Four corners through `to_pt`, not a min/max Rect: the view
                // rotates and mirrors, and an axis-aligned box would lie the
                // moment it does.
                let (x1, y1) = ((bx + bw as i32) as f32, (by + bh as i32) as f32);
                let corners = vec![
                    to_pt(bx as f32, by as f32),
                    to_pt(x1, by as f32),
                    to_pt(x1, y1),
                    to_pt(bx as f32, y1),
                ];
                painter.add(egui::Shape::closed_line(
                    corners.clone(),
                    egui::Stroke::new(2.0, egui::Color32::from_black_alpha(140)),
                ));
                painter.add(egui::Shape::closed_line(
                    corners,
                    egui::Stroke::new(1.0, egui::Color32::from_white_alpha(210)),
                ));
            }
        }
    }

    // LM-008: Show Mask Area — a purple tint over the ACTIVE layer's
    // masked-off region (coverage tiles; absent tile = hidden too).
    if app.mask_show_area
        && let Some(m) = app.doc.active_layer().mask.as_ref().filter(|m| m.enabled)
    {
        let col = egui::Color32::from_rgba_premultiplied(60, 0, 90, 70);
        let mut shapes: Vec<egui::Shape> = Vec::new();
        for (idx, t) in &m.tiles {
            let (ox, oy) = idx.origin();
            for py in 0..64usize {
                // Run-length the row: a mask is mostly flat, so the typical
                // row is one or two runs instead of 64 shapes.
                let mut run: Option<usize> = None;
                for px in 0..=64usize {
                    let hidden = px < 64 && t.pixel(px, py)[3] < 32768;
                    match (hidden, run) {
                        (true, None) => run = Some(px),
                        (false, Some(s)) => {
                            shapes.push(quad(
                                ox as f32 + s as f32,
                                oy as f32 + py as f32,
                                ox as f32 + px as f32,
                                oy as f32 + py as f32 + 1.0,
                                col,
                            ));
                            run = None;
                        }
                        _ => {}
                    }
                }
            }
        }
        painter.extend(shapes);
    }

    // TN-011: Show Tone Area — a green tint over every toned region of the
    // WHOLE stack (not just the active layer: the row exists to catch the
    // scrap of tone you forgot on some layer before it prints). Granularity
    // is the 64 px tile, which is deliberate — a 3 px scrap tints its whole
    // tile, and a scrap you can see is the entire point. Costs one
    // `is_blank()` probe per derived tile and one quad per tile that has
    // ink — and `is_blank` short-circuits on the first non-zero halfword, so
    // the tiles that matter cost a handful of reads. (A tile that really is
    // blank does scan; only toned layers are walked, and only while the
    // toggle is on.)
    if app.tone_show_area {
        let col = egui::Color32::from_rgba_premultiplied(0, 70, 55, 60);
        let vis = app.doc.effective_visibility();
        let mut shapes: Vec<egui::Shape> = Vec::new();
        for (li, l) in app.doc.layers.iter().enumerate() {
            let toned = l.tone.is_some()
                || matches!(
                    l.kind,
                    mn_core::LayerKind::Fill(mn_core::FillKind::Tone { .. })
                );
            if !toned || !vis.get(li).copied().unwrap_or(false) || l.opacity <= 0.0 {
                continue;
            }
            for (idx, t) in l.display_tiles() {
                if t.is_blank() {
                    continue;
                }
                let (ox, oy) = idx.origin();
                shapes.push(quad(
                    ox as f32,
                    oy as f32,
                    ox as f32 + 64.0,
                    oy as f32 + 64.0,
                    col,
                ));
            }
        }
        painter.extend(shapes);
    }

    // Transform: veil the vacated source region, float the transformed
    // preview over it, then the bbox + corner handles on top.
    if let Some(drag) = &app.transform_drag {
        let r = drag.source.rect;
        let veil = [
            to_pt(r[0] as f32, r[1] as f32),
            to_pt(r[2] as f32, r[1] as f32),
            to_pt(r[2] as f32, r[3] as f32),
            to_pt(r[0] as f32, r[3] as f32),
        ];
        if !drag.is_identity() || drag.mesh.is_some() {
            // Dim what is being lifted out — reads as "this region is
            // floating now" without touching the layer pixels.
            painter.add(egui::Shape::convex_polygon(
                veil.to_vec(),
                egui::Color32::from_white_alpha(110),
                egui::Stroke::NONE,
            ));
        }
        // Row 53 — mesh mode: the preview renders through the DEFORMED
        // quads (one textured triangle pair per cell), with the lattice
        // lines and draggable points on top. The affine path is untouched
        // below.
        if let Some(m) = &drag.mesh {
            if let Some(tex) = &drag.preview_tex {
                let k = (m.n - 1) as f32;
                let mut mesh = egui::Mesh {
                    texture_id: tex.id(),
                    indices: Vec::new(),
                    vertices: Vec::new(),
                };
                for cj in 0..m.n - 1 {
                    for ci in 0..m.n - 1 {
                        let corner = |i: usize, j: usize| {
                            let p = m.pts[j * m.n + i];
                            let uv = egui::pos2(i as f32 / k, j as f32 / k);
                            (
                                egui::epaint::Vertex {
                                    pos: to_pt(p[0], p[1]),
                                    uv,
                                    color: egui::Color32::WHITE,
                                },
                                [p[0], p[1]],
                            )
                        };
                        let (v00, p00) = corner(ci, cj);
                        let (v10, p10) = corner(ci + 1, cj);
                        let (v01, p01) = corner(ci, cj + 1);
                        let (v11, p11) = corner(ci + 1, cj + 1);
                        let b = mesh.vertices.len() as u32;
                        mesh.vertices.extend_from_slice(&[v00, v10, v01, v11]);
                        mesh.indices.extend_from_slice(&[b, b + 1, b + 2, b + 1, b + 3, b + 2]);
                        // The lattice lines, straight from the same corners.
                        for (a, q) in [(p00, p10), (p00, p01), (p10, p11), (p01, p11)] {
                            painter.line_segment(
                                [to_pt(a[0], a[1]), to_pt(q[0], q[1])],
                                egui::Stroke::new(1.0, theme::c().accent),
                            );
                        }
                    }
                }
                painter.add(egui::Shape::Mesh(mesh.into()));
            }
            let active = match drag.gesture.as_ref().map(|g| g.grab) {
                Some(crate::app::TransformGrab::MeshPoint(i)) => Some(i),
                _ => None,
            };
            for (i, p) in m.pts.iter().enumerate() {
                let half = if active == Some(i) { 4.5 } else { 3.0 };
                let rect = egui::Rect::from_center_size(
                    to_pt(p[0], p[1]),
                    egui::vec2(half * 2.0, half * 2.0),
                );
                painter.circle_filled(to_pt(p[0], p[1]), half, theme::c().accent);
                painter.circle_stroke(
                    to_pt(p[0], p[1]),
                    half,
                    egui::Stroke::new(1.0, egui::Color32::WHITE),
                );
                let _ = rect;
            }
            // PUPPET (row 54): pearls at the pins' current spots, a thin
            // line from where each was dropped.
            let active_pin = match drag.gesture.as_ref().map(|g| g.grab) {
                Some(crate::app::TransformGrab::PuppetPin(i)) => Some(i),
                _ => None,
            };
            for (i, pin) in m.pins.iter().enumerate() {
                if pin.orig != pin.cur {
                    painter.line_segment(
                        [to_pt(pin.orig[0], pin.orig[1]), to_pt(pin.cur[0], pin.cur[1])],
                        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(140)),
                    );
                }
                let r = if active_pin == Some(i) { 5.0 } else { 4.0 };
                painter.circle_filled(to_pt(pin.cur[0], pin.cur[1]), r, egui::Color32::WHITE);
                painter.circle_stroke(
                    to_pt(pin.cur[0], pin.cur[1]),
                    r,
                    egui::Stroke::new(2.0, theme::c().accent),
                );
            }
            return;
        }
        if let Some(tex) = &drag.preview_tex {
            let pts: Vec<egui::Pos2> = drag.bbox.iter().map(|c| to_pt(c[0], c[1])).collect();
            let uvs = [
                egui::pos2(0.0, 0.0),
                egui::pos2(1.0, 0.0),
                egui::pos2(1.0, 1.0),
                egui::pos2(0.0, 1.0),
            ];
            let mesh = egui::Mesh {
                texture_id: tex.id(),
                indices: vec![0, 1, 2, 0, 2, 3],
                vertices: pts
                    .iter()
                    .zip(uvs)
                    .map(|(pos, uv)| egui::epaint::Vertex {
                        pos: *pos,
                        uv,
                        color: egui::Color32::WHITE,
                    })
                    .collect(),
            };
            painter.add(egui::Shape::Mesh(mesh.into()));
        }
        let pts: Vec<egui::Pos2> = drag.bbox.iter().map(|c| to_pt(c[0], c[1])).collect();
        let mut line = pts.clone();
        line.push(pts[0]);
        painter.add(egui::Shape::line(
            line,
            egui::Stroke::new(1.5, theme::c().accent),
        ));
        // Corner handles (squares, CSP-style); the grabbed one is larger.
        let active_corner = match drag.gesture.as_ref().map(|g| g.grab) {
            Some(crate::app::TransformGrab::Corner(i)) => Some(i),
            _ => None,
        };
        for (i, pt) in pts.iter().enumerate() {
            let half = if active_corner == Some(i) { 5.0 } else { 4.0 };
            let rect = egui::Rect::from_center_size(*pt, egui::vec2(half * 2.0, half * 2.0));
            painter.rect_filled(rect, 1.0, theme::c().accent);
            painter.rect_stroke(
                rect,
                1.0,
                egui::Stroke::new(1.0, egui::Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }
        // TR-004: edge-midpoint handles (one-axis scale), smaller and
        // white-filled so they read differently from the corners.
        let active_edge = match drag.gesture.as_ref().map(|g| g.grab) {
            Some(crate::app::TransformGrab::Edge(i)) => Some(i),
            _ => None,
        };
        for i in 0..4 {
            let (a, b) = (drag.bbox[i], drag.bbox[(i + 1) % 4]);
            let m = egui::pos2((a[0] + b[0]) as f32 * 0.5, (a[1] + b[1]) as f32 * 0.5);
            let m = to_pt(m.x, m.y);
            let half = if active_edge == Some(i) { 4.5 } else { 3.5 };
            let rect = egui::Rect::from_center_size(m, egui::vec2(half * 2.0, half * 2.0));
            painter.rect_filled(rect, 1.0, egui::Color32::WHITE);
            painter.rect_stroke(
                rect,
                1.0,
                egui::Stroke::new(1.0, theme::c().accent),
                egui::StrokeKind::Inside,
            );
        }
        // The rotate stalk, same lollipop as frames/balloons/text: rotation
        // is THIS handle (plus dragging outside the box), never a corner.
        // Its point comes from the hit test's own helper.
        let stalk = drag.stalk_point(app.viewport.zoom);
        let stalk = to_pt(stalk[0], stalk[1]);
        let top = egui::pos2(
            (drag.bbox[0][0] + drag.bbox[1][0]) * 0.5,
            (drag.bbox[0][1] + drag.bbox[1][1]) * 0.5,
        );
        let grabbed_stalk = matches!(
            drag.gesture.as_ref().map(|g| g.grab),
            Some(crate::app::TransformGrab::Rotate)
        );
        painter.line_segment(
            [to_pt(top.x, top.y), stalk],
            egui::Stroke::new(1.2, theme::c().accent),
        );
        let r = if grabbed_stalk { 5.5 } else { 4.5 };
        painter.circle_filled(stalk, r, theme::c().panel);
        painter.circle_stroke(stalk, r, egui::Stroke::new(1.4, theme::c().accent));
        // TR-003: the reference point — a cross with a centre dot.
        let pv = drag.pivot();
        let pv = to_pt(pv[0], pv[1]);
        let arm = 8.0;
        for d in [egui::vec2(1.0, 0.0), egui::vec2(0.0, 1.0)] {
            painter.line_segment(
                [pv - d * arm, pv + d * arm],
                egui::Stroke::new(1.5, theme::c().accent),
            );
        }
        painter.circle_filled(pv, 2.0, theme::c().accent);
    }

    // Panel reading order (owner top item 2026-08-18): numbered badges on
    // each panel + the reading PATH between them — the proofreading half
    // of the feature: he can SEE the path a reader's eye will take before
    // the chapter ships. View-menu toggle.
    if app.frame_order_show
        && let Some(order) = &app.frame_order
    {
        let amber = egui::Color32::from_rgb(196, 158, 46);
        let pts: Vec<(egui::Pos2, bool, usize)> = reading_badges(&app.doc, order)
            .into_iter()
            .map(|(c, amb, n)| (to_pt(c[0], c[1]), amb, n))
            .collect();
        // The path first (under the badges).
        for w in pts.windows(2) {
            painter.line_segment(
                [w[0].0, w[1].0],
                egui::Stroke::new(2.0, amber.gamma_multiply(0.8)),
            );
        }
        for (c, amb, n) in &pts {
            let r = 9.0;
            painter.circle_filled(*c, r, if *amb { amber } else { egui::Color32::BLACK });
            painter.circle_stroke(*c, r, egui::Stroke::new(2.0, amber));
            painter.text(
                *c,
                egui::Align2::CENTER_CENTER,
                n,
                egui::FontId::proportional(10.0),
                if *amb { egui::Color32::BLACK } else { amber },
            );
        }
    }

    // L-001/L-002 magnetic lasso, drawn LAST so the wire sits over the
    // lineart it is snapped to. Three things at once, and they mean
    // different things:
    //   * the traced wire, as marching ants — same language as every other
    //     selection edge in the app;
    //   * a dot on every placed anchor, ringed on the first one, which is
    //     the click target that closes the loop;
    //   * a straight rubber band from the last anchor to the cursor while
    //     the pen is UP. It is deliberately NOT the snapped wire: the wire
    //     only recomputes while the pen is down (the shell delivers canvas
    //     moves during a drag), so drawing a stale snapped curve would
    //     promise a path that is not the one the next drag takes.
    if let Some(l) = &app.magnetic {
        let col = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200);
        ants(&l.preview(), (0.0, 0.0), col);
        let (lx, ly) = app.last_pointer;
        let cursor = egui::pos2(lx as f32 / ppp, ly as f32 / ppp);
        if canvas_pts.contains(cursor) {
            let a = l.last_anchor();
            painter.line_segment(
                [to_pt(a.0 as f32 + 0.5, a.1 as f32 + 0.5), cursor],
                egui::Stroke::new(1.0, theme::c().accent.gamma_multiply(0.6)),
            );
        }
        for (i, a) in l.anchors().iter().enumerate() {
            let p = to_pt(a.0 as f32 + 0.5, a.1 as f32 + 0.5);
            painter.circle_filled(p, 2.5, theme::c().accent);
            if i == 0 {
                painter.circle_stroke(p, 5.0, egui::Stroke::new(1.5, theme::c().accent));
            }
        }
        // The ants only crawl while something asks for the next frame.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
}

/// The reading-order badges: (panel centroid in canvas px, ambiguous, the
/// 1-based reading position). A panel whose folder or frame no longer
/// resolves is DROPPED, and dropping it must not renumber or re-flag the
/// panels after it — the position and the flag travel WITH the panel here,
/// so a gap only breaks the path polyline.
fn reading_badges(
    doc: &mn_core::Document,
    order: &mn_core::frame_order::PanelOrder,
) -> Vec<([f32; 2], bool, usize)> {
    order
        .panels
        .iter()
        .enumerate()
        .filter_map(|(i, pr)| {
            let f = doc
                .layers
                .get(pr.layer)
                .and_then(|l| l.frames())?
                .frames
                .get(pr.frame)?;
            Some((
                f.centroid(),
                order.ambiguous.get(i).copied().unwrap_or(false),
                i + 1,
            ))
        })
        .collect()
}

/// Triangulate an arbitrary (convex or concave) polygon by ear clipping —
/// frame splits produce arbitrary quads, and a convex-hull fill would lie
/// about a panel's extent. Winding is normalized first (frames store
/// either); if the clipper stalls on a degenerate remainder it fans it
/// rather than hang (rare, overfills at most the convex hull of the stub).
fn polygon_triangles(pts: &[egui::Pos2]) -> Vec<[egui::Pos2; 3]> {
    if pts.len() < 3 {
        return Vec::new();
    }
    let mut pts = pts.to_vec();
    let area: f32 = (0..pts.len())
        .map(|i| {
            let j = (i + 1) % pts.len();
            pts[i].x * pts[j].y - pts[j].x * pts[i].y
        })
        .sum();
    if area < 0.0 {
        pts.reverse();
    }
    let cross = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| {
        (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
    };
    let inside = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2, p: egui::Pos2| {
        let d1 = cross(a, b, p);
        let d2 = cross(b, c, p);
        let d3 = cross(c, a, p);
        let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(has_neg && has_pos)
    };
    let mut idx: Vec<usize> = (0..pts.len()).collect();
    let mut tris = Vec::new();
    let mut guard = 0;
    while idx.len() > 3 && guard < 10_000 {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
            if cross(a, b, c) <= 1e-4 {
                continue; // reflex or collinear vertex — not an ear
            }
            let clear = idx
                .iter()
                .all(|&j| j == ia || j == ib || j == ic || !inside(a, b, c, pts[j]));
            if !clear {
                continue;
            }
            tris.push([a, b, c]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    // Whatever is left (exactly three, or a degenerate stub): fan it.
    for k in 1..idx.len().saturating_sub(1) {
        tris.push([pts[idx[0]], pts[idx[k]], pts[idx[k + 1]]]);
    }
    tris
}

/// Fill an arbitrary polygon through [`polygon_triangles`] — the selected
/// frame's panel-white legibility fill (owner HIGH, f160fba).
fn fill_polygon(painter: &egui::Painter, pts: &[egui::Pos2], color: egui::Color32) {
    let mut mesh = egui::Mesh::default();
    for [a, b, c] in polygon_triangles(pts) {
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(a, color);
        mesh.colored_vertex(b, color);
        mesh.colored_vertex(c, color);
        mesh.add_triangle(base, base + 1, base + 2);
    }
    if !mesh.is_empty() {
        painter.add(egui::Shape::mesh(mesh));
    }
}

/// Wireframe outline of a balloon in canvas coords: the body as one closed
/// polyline plus each tail triangle. The overlay draws these; the seam where a
/// tail crosses the body is fine for a selection wireframe (the raster does
/// the real union).
fn balloon_outline(b: &mn_core::Balloon) -> Vec<Vec<[f32; 2]>> {
    let mut out = Vec::new();
    match &b.shape {
        mn_core::BalloonShape::Ellipse { center, radii } => {
            let mut line = Vec::with_capacity(49);
            for i in 0..=48 {
                let t = i as f32 / 48.0 * std::f32::consts::TAU;
                line.push([
                    center[0] + radii[0] * t.cos(),
                    center[1] + radii[1] * t.sin(),
                ]);
            }
            out.push(line);
        }
        mn_core::BalloonShape::RoundRect { rect, corner } => {
            let (x0, y0, x1, y1) = (rect[0], rect[1], rect[2], rect[3]);
            let r = corner.clamp(0.0, ((x1 - x0).abs()).min((y1 - y0).abs()) * 0.5);
            // Four corner arcs, 6 segments each, walked clockwise.
            let corners = [
                ([x1 - r, y0 + r], -0.25f32),
                ([x1 - r, y1 - r], 0.0),
                ([x0 + r, y1 - r], 0.25),
                ([x0 + r, y0 + r], 0.5),
            ];
            let mut line = Vec::new();
            for (c, start) in corners {
                for i in 0..=6 {
                    let t = (start + 0.25 * i as f32 / 6.0) * std::f32::consts::TAU;
                    line.push([c[0] + r * t.cos(), c[1] + r * t.sin()]);
                }
            }
            if let Some(first) = line.first().copied() {
                line.push(first);
            }
            out.push(line);
        }
        mn_core::BalloonShape::Polygon {
            points,
            corners,
            widths,
        } => {
            // The outline the raster actually inks: the smooth spline through
            // the anchors, not the control hull.
            let (mut line, _) = mn_core::balloon::tessellate_closed(points, corners, widths);
            if let Some(first) = line.first().copied() {
                line.push(first);
            }
            out.push(line);
        }
    }
    for t in &b.tails {
        let d = [t.tip[0] - t.base[0], t.tip[1] - t.base[1]];
        let l = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-3);
        let perp = [-d[1] / l, d[0] / l];
        let hw = t.width.max(1.0) * 0.5;
        out.push(vec![
            t.tip,
            [t.base[0] + perp[0] * hw, t.base[1] + perp[1] * hw],
            [t.base[0] - perp[0] * hw, t.base[1] - perp[1] * hw],
            t.tip,
        ]);
    }
    out
}

/// Marching-ants dashed polyline: 4 px dashes, 3 px gaps, the whole pattern
/// offset `phase` px along the arc length (egui's `dashed_line` has no offset
/// parameter, so the on-segments are emitted manually). Dashes are clipped at
/// segment ends — the standard look at corners.
fn ants_line(painter: &egui::Painter, path: &[egui::Pos2], col: egui::Color32, phase: f32) {
    if path.len() < 2 {
        return;
    }
    const DASH: f32 = 4.0;
    const PERIOD: f32 = 7.0; // dash + gap
    let stroke = egui::Stroke::new(1.0, col);
    let mut dist = 0.0f32;
    let mut prev = path[0];
    for &p in &path[1..] {
        let seg = p - prev;
        let len = seg.length();
        if len > 1e-4 {
            // Dash starts (pattern position k*PERIOD) shifted by -phase.
            let mut s = ((dist + phase) / PERIOD).ceil() * PERIOD - phase;
            while s < dist + len {
                let a = (s - dist) / len;
                let end = (s + DASH).min(dist + len);
                let b = (end - dist) / len;
                painter.line_segment([prev + seg * a, prev + seg * b], stroke);
                s += PERIOD;
            }
        }
        dist += len;
        prev = p;
    }
}

/// Template border lines (trim/bleed/inner/safety rectangle edges) in canvas
/// px. Panel edges lying along one of these get the same yellow affordance
/// as a shared gutter (owner clarification: e.g. Shueisha inner/trim guides).
/// Horizontal lines are `[x0, y, x1, y]`, vertical `[x, y0, x, y1]`.
fn template_lines(app: &App) -> Vec<[f32; 4]> {
    let Some(p) = app.page.as_ref().filter(|p| p.has_guides()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let push_rect = |out: &mut Vec<[f32; 4]>, r: [f32; 4]| {
        out.push([r[0], r[1], r[2], r[1]]); // top
        out.push([r[0], r[3], r[2], r[3]]); // bottom
        out.push([r[0], r[1], r[0], r[3]]); // left
        out.push([r[2], r[1], r[2], r[3]]); // right
    };
    push_rect(&mut out, p.trim_rect_px());
    push_rect(&mut out, p.bleed_rect_px());
    // Both book sides' variants: snapping is a tolerance affordance, and a
    // panel drawn against last session's side (or a page about to be
    // reordered) should still find a line to sit on.
    push_rect(&mut out, p.inner_rect_px_on(true));
    push_rect(&mut out, p.inner_rect_px_on(false));
    if let Some(s) = p.safety_rect_px_on(true) {
        push_rect(&mut out, s);
    }
    if let Some(s) = p.safety_rect_px_on(false) {
        push_rect(&mut out, s);
    }
    out
}

/// Does edge `a..c` run along one of the template lines? Same 1.5 px
/// tolerance as the sibling-gutter test; a partial x/y overlap counts.
fn edge_on_template(a: [f32; 2], c: [f32; 2], lines: &[[f32; 4]]) -> bool {
    const EPS2: f32 = 2.25; // squared 1.5 px
    for l in lines {
        let horizontal = (l[3] - l[1]).abs() < 1e-3;
        if horizontal {
            let dy = c[1] - a[1];
            if dy * dy > EPS2 {
                continue;
            }
            let d = a[1] - l[1];
            if d * d > EPS2 {
                continue;
            }
            let (lx0, lx1) = (l[0].min(l[2]), l[0].max(l[2]));
            let (ex0, ex1) = (a[0].min(c[0]), a[0].max(c[0]));
            if ex1 >= lx0 && ex0 <= lx1 {
                return true;
            }
        } else {
            let dx = c[0] - a[0];
            if dx * dx > EPS2 {
                continue;
            }
            let d = a[0] - l[0];
            if d * d > EPS2 {
                continue;
            }
            let (ly0, ly1) = (l[1].min(l[3]), l[1].max(l[3]));
            let (ey0, ey1) = (a[1].min(c[1]), a[1].max(c[1]));
            if ey1 >= ly0 && ey0 <= ly1 {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(p: [egui::Pos2; 3]) -> f32 {
        (p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[1].y - p[0].y) * (p[2].x - p[0].x)
    }

    fn p(x: f32, y: f32) -> egui::Pos2 {
        egui::pos2(x, y)
    }

    /// A concave dart triangulates into triangles whose areas sum to the
    /// polygon's shoelace area — no hull overfill, no gap — under BOTH
    /// windings (frames store either).
    #[test]
    fn concave_polygon_triangulates_exactly() {
        let dart = [p(0.0, 0.0), p(40.0, 0.0), p(20.0, 10.0), p(20.0, 40.0)];
        for poly in [dart.to_vec(), dart.iter().rev().copied().collect()] {
            let tris = polygon_triangles(&poly);
            assert_eq!(tris.len(), 2, "a quad is two triangles");
            let total: f32 = tris.iter().map(|t| area(*t)).sum();
            let shoe: f32 = (0..4)
                .map(|i| {
                    let j = (i + 1) % 4;
                    dart[i].x * dart[j].y - dart[j].x * dart[i].y
                })
                .sum::<f32>()
                .abs();
            assert!((total - shoe).abs() < 1e-3, "{total} vs {shoe}");
        }
    }

    /// Every triangle of a convex square stays inside it (the degenerate
    /// fan path never overfills the simple case), and a 2-gon is nothing.
    #[test]
    fn convex_and_degenerate() {
        let sq = [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)];
        let tris = polygon_triangles(&sq);
        assert_eq!(tris.len(), 2);
        for t in tris {
            for v in t {
                assert!(v.x >= -0.001 && v.x <= 10.001 && v.y >= -0.001 && v.y <= 10.001);
            }
        }
        assert!(polygon_triangles(&sq[..2]).is_empty());
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

    /// A panel the cache can no longer resolve is skipped. It must not take
    /// its successors' NUMBERS with it: the badges after a gap kept their
    /// own reading position and their own ambiguity flag, or a stale entry
    /// silently relabels the rest of the page (and moves the "?").
    #[test]
    fn a_skipped_panel_never_renumbers_the_ones_after_it() {
        let mut doc = mn_core::Document::new(400, 200);
        let a = doc.add_frame_folder(
            "a",
            mn_core::FrameSet::single_rect([0.0, 0.0, 200.0, 200.0], 2.0),
        );
        let b = doc.add_frame_folder(
            "b",
            mn_core::FrameSet::single_rect([200.0, 0.0, 400.0, 200.0], 2.0),
        );
        let pr = |layer| mn_core::frame_order::PanelRef { layer, frame: 0 };
        let order = mn_core::frame_order::PanelOrder {
            // The middle entry is the stale one (no such layer).
            panels: vec![pr(a), pr(9999), pr(b)],
            ambiguous: vec![false, false, true],
        };
        let badges = reading_badges(&doc, &order);
        assert_eq!(badges.len(), 2);
        assert_eq!((badges[0].2, badges[0].1), (1, false));
        assert_eq!(
            (badges[1].2, badges[1].1),
            (3, true),
            "third panel stays the third, and keeps its own flag"
        );
    }
}
