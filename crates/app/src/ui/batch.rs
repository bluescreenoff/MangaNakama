//! The Batch layer operations window — chrome over `app/batch.rs`.

use crate::app::App;
use crate::app::batch::{BatchOp, BatchScope};
use crate::cmd::AppCmd;

pub(super) fn batch_window(ctx: &egui::Context, app: &mut App) {
    if !app.batch.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Batch Layer Operations")
        .open(&mut open)
        .resizable(false)
        .default_pos(egui::pos2(320.0, 120.0))
        .show(ctx, |ui| {
            ui.label("Which layers");
            ui.horizontal(|ui| {
                for (s, label) in [
                    (BatchScope::AllLayers, "All"),
                    (BatchScope::FolderChildren, "Active folder's children"),
                    (BatchScope::Prefix, "Name starts with"),
                ] {
                    if ui.selectable_label(app.batch.scope == s, label).clicked() {
                        app.batch.scope = s;
                    }
                }
            });
            if app.batch.scope == BatchScope::Prefix {
                ui.add(
                    egui::TextEdit::singleline(&mut app.batch.prefix)
                        .hint_text("Panel")
                        .desired_width(140.0),
                );
            }
            let n = app.batch_matches().len();
            ui.weak(format!("{n} layers match (folder headers never do)"));
            ui.separator();
            ui.label("Operation");
            ui.horizontal(|ui| {
                for (o, label) in [
                    (BatchOp::Rename, "Rename"),
                    (BatchOp::ToneFromActive, "Apply active tone"),
                    (BatchOp::ToneClear, "Clear tone"),
                    (BatchOp::ExportPngs, "Export PNGs"),
                ] {
                    if ui.selectable_label(app.batch.op == o, label).clicked() {
                        app.batch.op = o;
                    }
                }
            });
            match app.batch.op {
                BatchOp::Rename => {
                    ui.add(
                        egui::TextEdit::singleline(&mut app.batch.pattern)
                            .hint_text("コマ {n}")
                            .desired_width(200.0),
                    );
                    ui.weak("{n} counts from the TOP of the stack; {name} keeps the old name");
                }
                BatchOp::ToneFromActive => {
                    ui.weak("copies the active layer's tone settings onto every match — one undo step");
                }
                BatchOp::ToneClear => {
                    ui.weak("painted ink returns to plain pixels on every match — one undo step");
                }
                BatchOp::ExportPngs => {
                    ui.weak("one full-canvas PNG per match, numbered from the top, into a folder you pick");
                }
            }
            ui.separator();
            if ui
                .add_enabled(n > 0, egui::Button::new("Apply"))
                .clicked()
            {
                app.push_cmd(if app.batch.op == BatchOp::ExportPngs {
                    AppCmd::BatchExportPngs
                } else {
                    AppCmd::BatchApply
                });
            }
        });
    if !open {
        app.batch.open = false;
    }
}
