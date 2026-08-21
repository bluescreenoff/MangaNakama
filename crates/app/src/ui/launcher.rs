//! CSP Selection Launcher: the floating action bar that appears under a live
//! selection. Default CSP button order, minus the ones our infrastructure
//! cannot honor yet (Cut/Copy&paste → undoable structural ops; New tone →
//! tones; Settings) — those live on the backlog.

use super::icons::Icon;
use super::theme;
use super::widgets::icon_btn;
use crate::app::App;
use crate::cmd::AppCmd;

/// Draw the launcher under the selection's screen bbox, clamped into the
/// canvas area. Called from `build` after the canvas overlay — real egui
/// widgets, so egui claims the pointer over the bar and the canvas never
/// sees presses that land on it.
pub(super) fn selection_launcher(ui: &mut egui::Ui, app: &mut App, canvas: egui::Rect) {
    let Some(sel) = app.doc.selection.as_ref() else {
        return;
    };
    if sel.is_empty() {
        return;
    }
    let ppp = app.shell.ppp;
    let (mut x0, mut y0) = (f32::INFINITY, f32::INFINITY);
    let (mut x1, mut y1) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut any = false;
    for p in sel
        .outline
        .iter()
        .chain(sel.extra_outlines.iter().flatten())
    {
        let (sx, sy) = app.viewport.to_screen(p.0, p.1);
        x0 = x0.min(sx / ppp);
        y0 = y0.min(sy / ppp);
        x1 = x1.max(sx / ppp);
        y1 = y1.max(sy / ppp);
        any = true;
    }
    if !any {
        return;
    }
    // Anchor just under the selection's top-left, clamped into the canvas.
    let bar_w = 14.0 * 24.0 + 34.0 + 12.0;
    let mut pos = egui::pos2(x0, (y1 + 6.0).min(canvas.bottom() - 30.0));
    pos.x = pos
        .x
        .clamp(canvas.left(), (canvas.right() - bar_w).max(canvas.left()));
    pos.y = pos.y.clamp(canvas.top(), canvas.bottom() - 30.0);
    let bar = egui::Rect::from_min_size(pos, egui::vec2(bar_w, 28.0));

    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(bar)
            .id_salt("mn.sel.launcher"),
        |ui| {
            egui::Frame::new()
                .fill(theme::WINDOW)
                .inner_margin(egui::Margin::symmetric(4, 2))
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let b = 22.0;
                        // Clipboard (SE-030's backlog, unblocked by the
                        // clipboard round): CSP order — cut, copy before crop.
                        if icon_btn(ui, Icon::SelCut, b, false, true, "Cut  (Ctrl+X)").clicked() {
                            app.push_cmd(AppCmd::Cut);
                        }
                        if icon_btn(ui, Icon::SelCopy, b, false, true, "Copy  (Ctrl+C)").clicked() {
                            app.push_cmd(AppCmd::Copy);
                        }
                        if icon_btn(ui, Icon::SelPaste, b, false, true, "Paste  (Ctrl+V)").clicked()
                        {
                            app.push_cmd(AppCmd::Paste);
                        }
                        // Crop leads (CSP order: cut/copy would sit before it).
                        if icon_btn(
                            ui,
                            Icon::SelCrop,
                            b,
                            false,
                            true,
                            "Crop canvas to selection",
                        )
                        .clicked()
                        {
                            app.push_cmd(AppCmd::CropSelection);
                        }
                        if icon_btn(ui, Icon::SelDeselect, b, false, true, "Deselect  (Ctrl+D)")
                            .clicked()
                        {
                            app.push_cmd(AppCmd::Deselect);
                        }
                        if icon_btn(
                            ui,
                            Icon::SelInvert,
                            b,
                            false,
                            true,
                            "Invert  (Ctrl+Shift+I)",
                        )
                        .clicked()
                        {
                            app.push_cmd(AppCmd::SelectInvert);
                        }
                        // Per-selection escape hatch: freehand strokes may
                        // land outside the ants (patch a lineart gap without
                        // rebuilding the selection). Commands still clamp;
                        // a NEW selection always starts clamped again.
                        let outside = app
                            .doc
                            .selection
                            .as_ref()
                            .is_some_and(|s| s.draw_outside);
                        let tip = if outside {
                            "Drawing outside the selection: ALLOWED (strokes only; click to clamp again)"
                        } else {
                            "Allow drawing outside the selection (strokes only, this selection only)"
                        };
                        if icon_btn(ui, Icon::SelDrawOutside, b, outside, true, tip).clicked() {
                            if let Some(s) = app.doc.selection.as_mut() {
                                s.draw_outside = !outside;
                            }
                            app.set_status(if !outside {
                                "strokes may now draw outside the selection"
                            } else {
                                "strokes clamp to the selection again"
                            });
                        }
                        if icon_btn(ui, Icon::SelExpand, b, false, true, "Expand selection")
                            .clicked()
                        {
                            app.push_cmd(AppCmd::SelectExpand(app.sel_px));
                        }
                        if icon_btn(ui, Icon::SelShrink, b, false, true, "Shrink selection")
                            .clicked()
                        {
                            app.push_cmd(AppCmd::SelectShrink(app.sel_px));
                        }
                        // SE-007: feather the edge (the paint/fill weight
                        // path — graduated coverage, unlike the boolean
                        // grow/shrink pair).
                        if ui
                            .small_button("Blur")
                            .on_hover_text("blur the selection border (feather)")
                            .clicked()
                        {
                            app.push_cmd(AppCmd::SelectBlur(app.sel_px));
                        }
                        // The px amount the expand/shrink buttons use.
                        ui.add(
                            egui::DragValue::new(&mut app.sel_px)
                                .range(1..=64)
                                .suffix("px")
                                .fixed_decimals(0),
                        )
                        .on_hover_text("expand / shrink / blur amount");
                        if icon_btn(ui, Icon::Trash, b, false, true, "Delete inside").clicked() {
                            app.push_cmd(AppCmd::ClearLayer);
                        }
                        if icon_btn(ui, Icon::SelClearOutside, b, false, true, "Clear outside")
                            .clicked()
                        {
                            app.push_cmd(AppCmd::ClearOutside);
                        }
                        if icon_btn(
                            ui,
                            Icon::SelTransform,
                            b,
                            false,
                            true,
                            "Move/Transform  (Ctrl+T)",
                        )
                        .clicked()
                        {
                            app.push_cmd(AppCmd::TransformStart);
                        }
                        if icon_btn(ui, Icon::Fill, b, false, true, "Fill  (Alt+Delete)").clicked()
                        {
                            app.push_cmd(AppCmd::FillSelection);
                        }
                    });
                });
        },
    );
}
