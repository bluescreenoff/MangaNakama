//! Edit ▸ Outline selection… (CSP "Outline selection", Other Layer
//! Filters): a small window over `Document::outline_selection`.

use crate::app::App;
use crate::cmd::AppCmd;
use mn_core::filter::OutlineBorder;

pub(super) fn outline_window(ctx: &egui::Context, app: &mut App) {
    if !app.outline_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Outline Selection")
        .open(&mut open)
        .resizable(false)
        .default_pos(egui::pos2(320.0, 120.0))
        .show(ctx, |ui| {
            let has_sel = app.doc.selection.as_ref().is_some_and(|s| !s.is_empty());
            if !has_sel {
                ui.label("make a selection first");
            }
            ui.add_enabled(
                has_sel,
                egui::Slider::new(&mut app.outline_width, 1.0..=64.0).text("line width px"),
            )
            .on_hover_text("the band's width in canvas pixels");
            ui.label("border type");
            ui.horizontal(|ui| {
                for b in [
                    OutlineBorder::Outside,
                    OutlineBorder::OnBorder,
                    OutlineBorder::Inside,
                ] {
                    if ui
                        .selectable_label(app.outline_border == b, b.label())
                        .on_hover_text(match b {
                            OutlineBorder::Outside => "the band sits outside the ants",
                            OutlineBorder::OnBorder => "centred on the ants, half each side",
                            OutlineBorder::Inside => "the band sits inside the ants",
                        })
                        .clicked()
                    {
                        app.outline_border = b;
                    }
                }
            });
            ui.checkbox(&mut app.outline_round, "round corners")
                .on_hover_text("a disc-shaped brush instead of a square");
            ui.separator();
            let apply = ui.add_enabled(
                has_sel,
                egui::Button::new("Outline"),
            );
            if apply.clicked() {
                app.push_cmd(AppCmd::OutlineSelection {
                    width: app.outline_width,
                    border: app.outline_border,
                    round: app.outline_round,
                });
            }
            ui.small(
                "strokes the band on the active raster layer in the main colour — one undo",
            );
        });
    if !open {
        app.outline_open = false;
    }
}
