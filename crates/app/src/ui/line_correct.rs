//! Layer ▸ Line correction… (row 169; CSP `E-001`…`E-007`, `VL-021`…
//! `VL-027`) — the four passes that make a hastily inked vector layer
//! usable, over the WHOLE layer, one undo press each.
//!
//! Four buttons, not a checkbox set with one Apply: a mangaka wants to
//! sweep the stubs, look, and only then decide whether to simplify — and
//! each pass being its own undo step is what makes that safe.
//!
//! No live preview, deliberately: previewing means re-deriving the layer
//! (a full engine replay of every recorded stroke) on every slider frame.
//! The op is one Ctrl+Z away instead.

use crate::app::App;
use crate::app::vector_edit::LineCorrect;
use crate::cmd::AppCmd;

pub(super) fn line_correct_window(ctx: &egui::Context, app: &mut App) {
    if !app.line_correct_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Line Correction")
        .open(&mut open)
        .resizable(false)
        .default_pos(egui::pos2(340.0, 140.0))
        .show(ctx, |ui| {
            // The refusal is stated here as well as in the status line: the
            // window can be left open while you click through the palette,
            // and a greyed row you understand beats a button that does
            // nothing.
            let vector = app
                .doc
                .layers
                .get(app.doc.active)
                .is_some_and(|l| l.records_strokes());
            if !vector {
                ui.label("the active layer records no strokes");
                ui.small(
                    "line correction edits recorded geometry — make a vector layer \
                     (Layer ▸ New vector layer) and ink on that",
                );
                ui.separator();
            }
            let short_px = app.mm_to_px(app.line_correct_short_mm);
            let gap_px = app.mm_to_px(app.line_correct_gap_mm);

            ui.add(
                egui::Slider::new(&mut app.line_correct_short_mm, 0.1..=10.0)
                    .text("short line under (mm)"),
            )
            .on_hover_text(format!(
                "at this page's dpi that is {short_px:.0} canvas px — mm so the \
                 threshold means the same thing after a resample"
            ));
            if ui
                .add_enabled(vector, egui::Button::new("Delete short lines"))
                .on_hover_text("E-007: sweeps up the stubs and stray fragments")
                .clicked()
            {
                app.push_cmd(AppCmd::LineCorrect(LineCorrect::DeleteShort {
                    px: short_px,
                }));
            }
            ui.separator();

            ui.add(
                egui::Slider::new(&mut app.line_correct_gap_mm, 0.05..=5.0)
                    .text("close gaps up to (mm)"),
            )
            .on_hover_text(format!("{gap_px:.0} canvas px at this page's dpi"));
            ui.checkbox(
                &mut app.line_correct_across,
                "join lines with different properties",
            )
            .on_hover_text("E-006: colour and tip may differ — the longer line's win");
            if ui
                .add_enabled(vector, egui::Button::new("Connect lines"))
                .on_hover_text("E-005: near-touching ends become one line, direction and all")
                .clicked()
            {
                app.push_cmd(AppCmd::LineCorrect(LineCorrect::Connect {
                    px: gap_px,
                    across: app.line_correct_across,
                }));
            }
            ui.separator();

            ui.add(
                egui::Slider::new(&mut app.line_correct_simplify_px, 0.2..=20.0)
                    .text("simplify tolerance (px)"),
            )
            .on_hover_text("how far a point may sit off the line before it is kept");
            if ui
                .add_enabled(vector, egui::Button::new("Simplify"))
                .on_hover_text("E-001: drops redundant points; corners survive by construction")
                .clicked()
            {
                app.push_cmd(AppCmd::LineCorrect(LineCorrect::Simplify {
                    px: app.line_correct_simplify_px,
                }));
            }
            ui.separator();

            ui.add(
                egui::Slider::new(&mut app.line_correct_width, 0.1..=5.0)
                    .text("width ×")
                    .logarithmic(true),
            );
            if ui
                .add_enabled(vector, egui::Button::new("Adjust line width"))
                .on_hover_text(
                    "VL-026 scale up/down — tapers stay pointed, and VL-027's \
                     one-pixel floor means narrowing never erases a line",
                )
                .clicked()
            {
                app.push_cmd(AppCmd::LineCorrect(LineCorrect::Width {
                    scale: app.line_correct_width,
                }));
            }
            ui.separator();
            ui.small("each button is one undo press over the whole layer");
        });
    if !open {
        app.line_correct_open = false;
    }
}
