//! Canvas overlay: everything painted in egui OVER the GPU canvas — page
//! shadow/border, manuscript guides, selection dashes, frame/balloon/text/
//! transform previews, the brush ring. Z-order here is sequential and
//! meaningful (shadow first, transform handles last). Takes &App only.

use crate::app::App;

mod areas;
mod frames;
mod page;
mod readouts;
mod rulers;
mod selection;
mod text;
mod tools;
mod transform;

use readouts::{dim_readout, draw_dim_readout, extent_of};

pub(super) fn canvas_overlay(ui: &egui::Ui, app: &App, canvas_pts: egui::Rect) {
    let painter = ui.painter().with_clip_rect(canvas_pts);
    let ppp = app.shell.ppp;
    let to_pt = |cx: f32, cy: f32| {
        let (sx, sy) = app.viewport.to_screen(cx, cy);
        egui::pos2(sx / ppp, sy / ppp)
    };
    // The pointer in POINTS — the anchor every live-drag readout hangs off.
    let cursor_pt = || {
        let (lx, ly) = app.last_pointer;
        egui::pos2(lx as f32 / ppp, ly as f32 / ppp)
    };
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

    // THE Z-ORDER, and the only place it is decided. Each band is one
    // module; they paint bottom-up, exactly in the order this list reads,
    // which is the order the single function these were cut out of ran in.
    readouts::input_probe(ui, app, &painter, canvas_pts);
    frames::gen_lines(app, &painter, ppp, &to_pt);
    page::paint(app, &painter, canvas_pts, &to_pt);
    rulers::paint(ui, app, &painter, &to_pt);
    selection::paint(ui, app, &painter, canvas_pts, &cursor_pt, &ants);
    frames::previews(app, &painter, canvas_pts, ppp, &to_pt, &cursor_pt);
    frames::object(app, &painter, &to_pt);
    frames::balloon(app, &painter, &to_pt);
    tools::figures(app, &painter, &to_pt);
    tools::gradients(app, &painter, &to_pt);
    text::paint(ui, app, &painter, &to_pt, &ants);
    tools::brush_ring(app, &painter, canvas_pts, ppp);
    tools::eyedropper(app, &painter, canvas_pts, ppp, &to_pt);
    areas::paint(app, &painter, &to_pt);
    // The mesh path stops the overlay dead — the two bands below it did not
    // paint before the split either.
    if transform::paint(app, &painter, canvas_pts, &to_pt, &cursor_pt) {
        return;
    }
    frames::reading_order(app, &painter, &to_pt);
    selection::magnetic(ui, app, &painter, canvas_pts, ppp, &to_pt, &ants);
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
