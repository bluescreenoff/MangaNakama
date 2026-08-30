//! Live previews for the drawing tools that have one: Figure (with the
//! second stage and the smart-shape hold), the two gradients, the
//! brush-size ring and the eyedropper's picker circle. Moved here verbatim
//! when `overlay.rs` was split by Z-order band.

use super::super::theme;
use crate::app::App;
use crate::cmd::Tool;

/// Z-order band 9: the Figure tool's previews.
pub(super) fn figures(
    app: &App,
    painter: &egui::Painter,
    to_pt: &dyn Fn(f32, f32) -> egui::Pos2,
) {
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
    // Row 157: the second stage — the bend or the spin, drawn as the path
    // it will actually ink (`figure_stage2_path`, the same function the
    // commit calls) with the frozen baseline behind it so you can see how
    // far you have taken it.
    if let Some(s) = &app.figure_stage2 {
        let col = theme::c().accent;
        let pts: Vec<egui::Pos2> = app
            .figure_stage2_path(s)
            .iter()
            .map(|p| to_pt(p[0], p[1]))
            .collect();
        if pts.len() >= 2 {
            painter.add(egui::Shape::line(
                vec![to_pt(s.a.0, s.a.1), to_pt(s.b.0, s.b.1)],
                egui::Stroke::new(1.0, theme::c().text_weak),
            ));
            painter.add(egui::Shape::line(pts, egui::Stroke::new(1.5, col)));
            painter.circle_filled(to_pt(s.a.0, s.a.1), 3.0, col);
            painter.circle_filled(to_pt(s.b.0, s.b.1), 3.0, col);
            // The steering point itself: it is ON the curve for a bend, and
            // where the corner is heading for a spin.
            painter.circle_stroke(to_pt(s.cur.0, s.cur.1), 4.5, egui::Stroke::new(1.5, col));
        }
    }
    if let Some(pts) = &app.figure_poly {
        let col = theme::c().accent;
        for (i, a) in pts.iter().enumerate() {
            let c = to_pt(a.p.0, a.p.1);
            // `FG-016`: corner anchors are SQUARE and smooth ones round —
            // the vector-editing convention this app already reads (frame
            // and transform handles are squares, on-curve markers circles),
            // and the only way the artist can tell which of two identical
            // dots will crease. Alt+tap is invisible otherwise.
            if a.corner {
                let r = egui::Rect::from_center_size(c, egui::vec2(6.4, 6.4));
                painter.rect_filled(r, 0.0, col);
                painter.rect_stroke(
                    r,
                    0.0,
                    egui::Stroke::new(1.0, theme::c().panel),
                    egui::StrokeKind::Inside,
                );
            } else {
                painter.circle_filled(c, 3.2, col);
                painter.circle_stroke(c, 3.2, egui::Stroke::new(1.0, theme::c().panel));
            }
            // `FG-014`: the anchor a Ctrl+drag has hold of wears a ring, so
            // a drag that has not moved yet still says WHICH point it took.
            if app.figure_anchor_drag == Some(i) {
                painter.circle_stroke(c, 6.5, egui::Stroke::new(1.5, col));
            }
        }
        if pts.len() >= 2 {
            // Rows 84/85: the Curve sub tool previews the SPLINE it will
            // ink, not the chords between the clicks — otherwise the shape
            // you are judging is not the shape you get. `FG-016`'s creases
            // go through the same door for the same reason.
            let path: Vec<[f32; 2]> = pts.iter().map(|a| [a.p.0, a.p.1]).collect();
            let shape = if app.figure_mode == crate::cmd::FigureMode::Curve {
                let corners: Vec<bool> = pts.iter().map(|a| a.corner).collect();
                mn_core::balloon::tessellate_open_corners(&path, &corners)
            } else {
                path
            };
            let line: Vec<egui::Pos2> = shape.iter().map(|p| to_pt(p[0], p[1])).collect();
            painter.add(egui::Shape::line(line, egui::Stroke::new(1.2, col)));
        }
        // Rubber line to the pointer (client px → canvas). Not while an
        // anchor is being dragged: the pointer is holding a point of the
        // figure, not aiming at where the next one goes.
        let (lx, ly) = app.last_pointer;
        let (mx, my) = app.viewport.to_canvas(lx as f32, ly as f32);
        if let Some(last) = pts.last().filter(|_| app.figure_anchor_drag.is_none()) {
            painter.line_segment(
                [to_pt(last.p.0, last.p.1), to_pt(mx, my)],
                egui::Stroke::new(1.0, theme::c().text_weak),
            );
        }
    }

    // Row 156 / `FG-020`: the Smart shape hold. The wobble is already inked
    // underneath (this sub tool draws live), so what the overlay adds is the
    // ANSWER — the clean figure the release would put in its place, drawn
    // through the recognizer's own path so the preview and the swap cannot
    // disagree. Nothing is drawn until the hold matures, which is what makes
    // "keep moving and nothing happens" legible.
    if let Some(hit) = app.smart_shape.as_ref().and_then(|g| g.preview()) {
        let col = theme::c().accent;
        let mut pts: Vec<egui::Pos2> = hit.path.iter().map(|p| to_pt(p[0], p[1])).collect();
        if hit.closed() && !pts.is_empty() {
            pts.push(pts[0]);
        }
        if pts.len() >= 2 {
            painter.add(egui::Shape::line(pts, egui::Stroke::new(1.5, col)));
        }
    }
}

/// Z-order band 10: the gradient ramps.
pub(super) fn gradients(
    app: &App,
    painter: &egui::Painter,
    to_pt: &dyn Fn(f32, f32) -> egui::Pos2,
) {
    // Gradient tool: the ramp line with end markers.
    if let Some((a, b)) = &app.grad_drag {
        let col = theme::c().accent;
        let pa = to_pt(a.0, a.1);
        let pb = to_pt(b.0, b.1);
        painter.line_segment([pa, pb], egui::Stroke::new(2.0, col));
        painter.circle_filled(pa, 4.0, col);
        painter.circle_filled(pb, 4.0, col);
    }

    // FI-050 / freeform gradient: the two guide lines, each in the colour it
    // is about to lay down — the preview says which end of the ramp you are
    // drawing, which a single accent colour could not. A thin dark casing
    // under each keeps a white or pale guide visible on white paper.
    if let Some(g) = &app.grad_free {
        let rgb = |[r, gg, b]: [f32; 3]| {
            egui::Color32::from_rgb(
                (r.clamp(0.0, 1.0) * 255.0).round() as u8,
                (gg.clamp(0.0, 1.0) * 255.0).round() as u8,
                (b.clamp(0.0, 1.0) * 255.0).round() as u8,
            )
        };
        let casing = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 90);
        let guide = |pts: &[[f32; 2]], col: egui::Color32| {
            let line: Vec<egui::Pos2> = pts.iter().map(|p| to_pt(p[0], p[1])).collect();
            if line.len() < 2 {
                if let Some(p) = line.first() {
                    painter.circle_filled(*p, 3.5, col);
                }
                return;
            }
            painter.add(egui::Shape::line(
                line.clone(),
                egui::Stroke::new(3.5, casing),
            ));
            painter.add(egui::Shape::line(line, egui::Stroke::new(1.6, col)));
        };
        // Each finished guide in the colour IT is carrying — recorded when
        // it was drawn, so the preview is what the apply will lay down even
        // if the palette has moved since (`GradFree`'s doc).
        let col4 = |c: [f32; 4]| rgb([c[0], c[1], c[2]]);
        for done in &g.done {
            guide(&done.pts, col4(done.colour));
        }
        // The live stroke wears the colour it is ABOUT to take: sub for the
        // second line, main for the first and for every line after that.
        guide(
            &g.cur,
            if g.done.len() == 1 {
                rgb(app.sub_color)
            } else {
                rgb(app.active_color())
            },
        );
    }
}

/// Z-order band 12: the brush-size ring at the cursor.
pub(super) fn brush_ring(app: &App, painter: &egui::Painter, canvas_pts: egui::Rect, ppp: f32) {
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
}

/// Z-order band 13: the eyedropper's picker circle.
pub(super) fn eyedropper(
    app: &App,
    painter: &egui::Painter,
    canvas_pts: egui::Rect,
    ppp: f32,
    to_pt: &dyn Fn(f32, f32) -> egui::Pos2,
) {
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
}
