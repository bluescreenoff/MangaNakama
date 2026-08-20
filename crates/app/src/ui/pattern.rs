//! The Pattern Studio window: the live repeat preview and the one-line save.
//! State + rendering live in `app/pattern.rs`; this is only the chrome.

use crate::app::App;
use crate::cmd::AppCmd;

pub(super) fn pattern_window(ctx: &egui::Context, app: &mut App) {
    if !app.pattern.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Pattern Studio")
        .open(&mut open)
        .resizable(false)
        .default_pos(egui::pos2(340.0, 80.0))
        .show(ctx, |ui| {
            // The repeat preview: one rendered tile drawn grid × grid, flush.
            let tex = app.pattern_preview_tex();
            let g = app.pattern.grid.clamp(2, 3);
            let cell = (336.0 / g as f32).floor();
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                for _row in 0..g {
                    ui.horizontal(|ui| {
                        for _col in 0..g {
                            ui.add(
                                egui::Image::from_texture(&tex)
                                    .fit_to_exact_size(egui::vec2(cell, cell)),
                            );
                        }
                    });
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Repeat");
                for n in [2u32, 3] {
                    if ui
                        .selectable_label(app.pattern.grid == n, format!("{n}×{n}"))
                        .clicked()
                    {
                        app.pattern.grid = n;
                    }
                }
                let (w, h) = app.doc.size;
                ui.weak(format!("· tile {w}×{h} px"));
            });
            // Wrap is what makes the tile seamless; if the artist turned it
            // off, say so instead of letting the preview quietly lie.
            if !(app.wrap_x && app.wrap_y) {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        super::theme::WARN,
                        "wrap is off — strokes will seam at the edges",
                    );
                    if ui.small_button("turn wrap on").clicked() {
                        app.push_cmd(AppCmd::SetWrapX(true));
                        app.push_cmd(AppCmd::SetWrapY(true));
                    }
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.add(
                    egui::TextEdit::singleline(&mut app.pattern.name)
                        .hint_text("pattern")
                        .desired_width(140.0),
                );
                if ui.button("Save as material").clicked() {
                    app.push_cmd(AppCmd::PatternSaveMaterial);
                }
            });
            ui.weak("draws wrap at every edge; saving registers the tile in the material bank");
        });
    if !open {
        app.pattern.open = false;
    }
}
