//! Selection edges: the committed marching ants and their live previews,
//! plus the magnetic lasso that is drawn last of all. Moved here verbatim
//! when `overlay.rs` was split by Z-order band.

use super::super::theme;
use super::{dim_readout, draw_dim_readout};
use crate::app::App;
use crate::cmd::SelectMode;

/// Z-order band 5: the selection outline, the in-progress drags and the
/// selection-paint preview.
pub(super) fn paint(
    ui: &egui::Ui,
    app: &App,
    painter: &egui::Painter,
    canvas_pts: egui::Rect,
    cursor_pt: &dyn Fn() -> egui::Pos2,
    ants: &dyn Fn(&[(f32, f32)], (f32, f32), egui::Color32),
) {
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
                // IO-081: the number CSP puts in its Information palette.
                draw_dim_readout(
                    &painter,
                    canvas_pts,
                    cursor_pt(),
                    dim_readout(b.0 - a.0, b.1 - a.1, app.work_dpi()),
                );
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
}

/// Z-order band 17, the last one: the magnetic lasso's wire, anchors and
/// rubber band.
pub(super) fn magnetic(
    ui: &egui::Ui,
    app: &App,
    painter: &egui::Painter,
    canvas_pts: egui::Rect,
    ppp: f32,
    to_pt: &dyn Fn(f32, f32) -> egui::Pos2,
    ants: &dyn Fn(&[(f32, f32)], (f32, f32), egui::Color32),
) {
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
