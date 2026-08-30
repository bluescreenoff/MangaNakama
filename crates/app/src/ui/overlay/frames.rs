//! Panels, balloons and the generated-line runs: every on-canvas object
//! affordance the Frame, Balloon and Object tools draw, plus the reading
//! order badges. Moved here verbatim when `overlay.rs` was split by Z-order
//! band.

use super::super::theme;
use super::{dim_readout, draw_dim_readout, extent_of};
use crate::app::App;
use crate::cmd::{BalloonMode, Tool};

/// Z-order band 2: the generated run's blue reference line and its handles.
pub(super) fn gen_lines(
    app: &App,
    painter: &egui::Painter,
    ppp: f32,
    to_pt: &dyn Fn(f32, f32) -> egui::Pos2,
) {
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
}

/// Z-order band 6: the frame tools' in-progress previews.
pub(super) fn previews(
    app: &App,
    painter: &egui::Painter,
    canvas_pts: egui::Rect,
    ppp: f32,
    to_pt: &dyn Fn(f32, f32) -> egui::Pos2,
    cursor_pt: &dyn Fn() -> egui::Pos2,
) {
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
        // IO-081: a freehand panel is still a panel with a size — the
        // trail's own extent is what it will occupy.
        let (w, h) = extent_of(pts.iter().copied());
        draw_dim_readout(
            &painter,
            canvas_pts,
            cursor_pt(),
            dim_readout(w, h, app.work_dpi()),
        );
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
            // IO-081. Only the RECT sub tool: a divide drag is a cut line,
            // and a bounding box around a line is not a panel size.
            draw_dim_readout(
                &painter,
                canvas_pts,
                cursor_pt(),
                dim_readout(b.0 - a.0, b.1 - a.1, app.work_dpi()),
            );
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
}

/// Z-order band 7: the Object tool's selected stroke / panel / balloon and
/// all of their handles.
pub(super) fn object(app: &App, painter: &egui::Painter, to_pt: &dyn Fn(f32, f32) -> egui::Pos2) {
    let (w, h) = app.doc.size;
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
}

/// Z-order band 8: the Balloon tool's live drag preview.
pub(super) fn balloon(app: &App, painter: &egui::Painter, to_pt: &dyn Fn(f32, f32) -> egui::Pos2) {
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
}

/// Z-order band 16: the panel reading order badges and the path between
/// them.
pub(super) fn reading_order(
    app: &App,
    painter: &egui::Painter,
    to_pt: &dyn Fn(f32, f32) -> egui::Pos2,
) {
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
