//! The Navigator palette (CV-030/031/036, TODO #4): the whole page as a
//! live thumbnail with the RED VIEW-RECT (drag it to pan), the 13-control
//! strip (zoom bar/steps/reset/fit, rotation steps/reset, flip H — lit
//! while mirrored; flip V deferred: the viewport math is flip-h-only
//! cross-cutting), and sticky FIT-TO-NAVIGATOR (re-fit on every window
//! resize until toggled off).
//!
//! The thumbnail re-renders only when the document revision moves; the
//! rect and readouts are per-frame painter work on top.

use crate::app::App;
use crate::cmd::AppCmd;

/// Thumbnail long side, px.
const THUMB: f32 = 176.0;

pub(super) fn navigator_palette(ui: &mut egui::Ui, app: &mut App) {
    // Sticky fit (CV-036): check every frame while the palette is open.
    app.navigator_sticky_fit_check();

    let (w, h) = (app.doc.size.0 as f32, app.doc.size.1 as f32);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let scale = THUMB / w.max(h);
    let (tw, th) = (w * scale, h * scale);

    // --- thumbnail + red view-rect ---------------------------------------
    let thumb = app.navigator_thumb();
    let img = egui::Image::from_texture(&thumb).fit_to_exact_size(egui::vec2(tw, th));
    let resp = ui.add(img.sense(egui::Sense::drag()));
    let rect = resp.rect;
    if resp.dragged()
        && let Some(p) = resp.interact_pointer_pos()
    {
        // Thumb px → canvas px, then centre the view there.
        let cx = (p.x - rect.left()) / scale;
        let cy = (p.y - rect.top()) / scale;
        app.navigator_pan_to(cx, cy);
    }
    // The red rect: the visible canvas region mapped into the thumb.
    // AABB of the four screen-corner back-projections (a rotated view's
    // visible region is a rotated quad in canvas space).
    let surface = app.renderer.surface_size();
    let cc = app.canvas_center();
    let quad = [
        app.viewport.to_canvas(0.0, 0.0),
        app.viewport.to_canvas(surface.0 as f32, 0.0),
        app.viewport.to_canvas(0.0, surface.1 as f32),
        app.viewport.to_canvas(surface.0 as f32, surface.1 as f32),
    ];
    let _ = cc;
    let (mut x0, mut y0, mut x1, mut y1) = (
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    );
    for (qx, qy) in quad {
        x0 = x0.min(qx);
        y0 = y0.min(qy);
        x1 = x1.max(qx);
        y1 = y1.max(qy);
    }
    let painter = ui.painter_at(rect);
    let r = egui::Rect::from_min_max(
        egui::pos2(rect.left() + x0 * scale, rect.top() + y0 * scale),
        egui::pos2(rect.left() + x1 * scale, rect.top() + y1 * scale),
    );
    painter.rect_stroke(
        r,
        1.0,
        egui::Stroke::new(1.5, egui::Color32::RED),
        egui::StrokeKind::Inside,
    );

    // --- controls strip ----------------------------------------------------
    ui.separator();
    ui.horizontal(|ui| {
        if ui.small_button("−").on_hover_text("zoom out").clicked() {
            app.push_cmd(AppCmd::ZoomStep(1.0 / 1.25));
        }
        if ui.small_button("＋").on_hover_text("zoom in").clicked() {
            app.push_cmd(AppCmd::ZoomStep(1.25));
        }
        if ui.small_button("100%").clicked() {
            app.push_cmd(AppCmd::Zoom100);
        }
        if ui
            .small_button("Fit")
            .on_hover_text("fit to screen (once)")
            .clicked()
        {
            app.push_cmd(AppCmd::ZoomFit);
        }
    });
    ui.horizontal(|ui| {
        if ui
            .small_button("⟲")
            .on_hover_text("rotate left 15°")
            .clicked()
        {
            app.push_cmd(AppCmd::RotateView(-15f32.to_radians()));
        }
        if ui
            .small_button("⟳")
            .on_hover_text("rotate right 15°")
            .clicked()
        {
            app.push_cmd(AppCmd::RotateView(15f32.to_radians()));
        }
        if ui
            .small_button("0°")
            .on_hover_text("reset rotation")
            .clicked()
        {
            app.push_cmd(AppCmd::RotateReset);
        }
        // LIT while mirrored (CV-031: "the flip icons stay highlighted so
        // you never forget you are mirrored").
        let flipped = app.viewport.flip_h;
        let btn = egui::Button::new(if flipped { "⇄ ●" } else { "⇄" });
        if ui
            .add(btn)
            .on_hover_text("flip view horizontally (Ctrl+9)")
            .clicked()
        {
            app.push_cmd(AppCmd::FlipView);
        }
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut app.fit_sticky, "Fit to Navigator")
            .on_hover_text(
                "keep re-fitting the page every time the window resizes, until unchecked",
            );
        ui.weak(format!(
            "{:.0}% · {:.0}°",
            app.viewport.zoom * 100.0,
            app.viewport.rotate_rad.to_degrees()
        ));
    });
}
