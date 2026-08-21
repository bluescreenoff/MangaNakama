//! The Batch layer operations window — chrome over `app/batch.rs`.

use super::layers::{BLENDS, blend_name};
use crate::app::App;
use crate::app::batch::{BATCH_TINTS, BatchOp, BatchScope};
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
            // Count the palette selection the same way the scope does, so
            // the label never promises rows the scope would drop.
            let picked = app
                .doc
                .multi_targets()
                .into_iter()
                .filter(|&i| app.doc.layers.get(i).is_some_and(|l| !l.folder))
                .count();
            let selected = format!("Selected layers ({picked})");
            ui.horizontal(|ui| {
                for (s, label) in [
                    (BatchScope::AllLayers, "All"),
                    (BatchScope::FolderChildren, "Active folder's children"),
                    (BatchScope::Prefix, "Name starts with"),
                    (BatchScope::Pattern, "Name matches"),
                    (BatchScope::Selected, selected.as_str()),
                ] {
                    if ui.selectable_label(app.batch.scope == s, label).clicked() {
                        app.batch.scope = s;
                    }
                }
            });
            if app.batch.scope == BatchScope::Selected {
                ui.weak("the rows picked in the Layers palette (Ctrl+click, Shift+click) — with none picked, just the active layer");
            }
            if app.batch.scope == BatchScope::Prefix {
                ui.add(
                    egui::TextEdit::singleline(&mut app.batch.prefix)
                        .hint_text("Panel")
                        .desired_width(140.0),
                );
            }
            if app.batch.scope == BatchScope::Pattern {
                ui.add(
                    egui::TextEdit::singleline(&mut app.batch.name_pat)
                        .hint_text("sketch")
                        .desired_width(200.0),
                );
                ui.weak("anywhere in the name, upper/lower case ignored — add * to anchor it: sketch* starts with, *sketch ends with, rough*v2 both");
            }
            let n = app.batch_matches().len();
            ui.weak(format!("{n} layers match (folder headers never do)"));
            all_pages_row(ui, app);
            ui.separator();
            ui.label("Operation");
            ui.horizontal(|ui| {
                for (o, label) in [
                    (BatchOp::Rename, "Rename"),
                    (BatchOp::Draft, "Draft flag"),
                    (BatchOp::Colour, "Layer colour"),
                    (BatchOp::BlendMode, "Blend mode"),
                ] {
                    if ui.selectable_label(app.batch.op == o, label).clicked() {
                        app.batch.op = o;
                    }
                }
            });
            ui.horizontal(|ui| {
                for (o, label) in [
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
                BatchOp::Draft => {
                    ui.checkbox(&mut app.batch.draft_on, "Mark as draft layers")
                        .on_hover_text("unticked clears the flag instead");
                    ui.weak("draft layers stay on screen and drop out of fill references and export — like the single-layer toggle, this is not undoable");
                }
                BatchOp::Colour => {
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(app.batch.colour.is_none(), "Stock")
                            .on_hover_text("clear the display colour")
                            .clicked()
                        {
                            app.batch.colour = None;
                        }
                        for t in BATCH_TINTS {
                            let lit = app.batch.colour == Some(t);
                            let chip = egui::Button::new(
                                egui::RichText::new("■")
                                    .color(egui::Color32::from_rgb(t[0], t[1], t[2])),
                            );
                            let chip = if lit { chip.small() } else { chip };
                            if ui
                                .add(chip)
                                .on_hover_text(format!("#{:02x}{:02x}{:02x}", t[0], t[1], t[2]))
                                .clicked()
                            {
                                app.batch.colour = Some(t);
                            }
                        }
                    });
                    ui.weak("dark ink DISPLAYS in this colour; the pixels stay black — like the single-layer chip, not undoable");
                }
                BatchOp::BlendMode => {
                    egui::ComboBox::from_id_salt("mn.batch.blend")
                        .selected_text(blend_name(app.batch.blend))
                        .show_ui(ui, |ui| {
                            for b in BLENDS {
                                ui.selectable_value(&mut app.batch.blend, b, blend_name(b));
                            }
                        });
                    ui.weak("same door as the Layers palette's blend picker — like it, not undoable");
                }
                BatchOp::ExportPngs => {
                    ui.weak("one full-canvas PNG per match, numbered from the top, into a folder you pick");
                }
            }
            ui.separator();
            if app.batch.all_pages_live() {
                ui.weak("the other pages are saved directly — undo covers only the page you have open");
            }
            if ui
                .add_enabled(n > 0 || app.batch.all_pages_live(), egui::Button::new("Apply"))
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

/// The "every page" modifier. Disabled — with the reason on the tooltip —
/// for the scopes and the operation that cannot mean anything on a page
/// nobody has open, rather than silently doing less than the tick says.
fn all_pages_row(ui: &mut egui::Ui, app: &mut App) {
    let pages = app.pages.len();
    let why = if pages < 2 {
        Some("this work has one page")
    } else if !app.batch.scope.travels_to_other_pages() {
        Some("that scope reads the open page's active folder or picked rows — another page has neither. Use All or a name scope.")
    } else if app.batch.op == BatchOp::ExportPngs {
        Some("export writes the open page's layers; the File menu's Export All Pages does the whole work")
    } else {
        None
    };
    let label = format!("Every page in this work ({pages})");
    let r = ui
        .add_enabled(
            why.is_none(),
            egui::Checkbox::new(&mut app.batch.all_pages, label),
        )
        .on_disabled_hover_text(why.unwrap_or_default());
    if why.is_none() {
        r.on_hover_text(
            "the open page goes through undo as usual; the rest are edited and saved in place",
        );
    }
}
