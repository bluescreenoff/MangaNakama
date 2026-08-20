//! The Layer Comps palette (TRIAGE 139 v1, LC-001..005): the pinned
//! "Last document state" row, comp rows (the eye applies), add with a
//! name field, save-overwrite, step buttons, delete, and LC-006's
//! default for layers added after a snapshot.

use crate::app::App;
use crate::cmd::AppCmd;

pub fn comps_palette(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        if ui.button("＋ comp").clicked() {
            app.comp_name_draft = if app.comp_name_draft.is_empty() {
                format!("Comp {}", app.doc.comps.len() + 1)
            } else {
                std::mem::take(&mut app.comp_name_draft)
            };
            app.comp_add(&app.comp_name_draft.clone());
            app.set_status("layer comp added — toggles eyes then ＋ comp again");
        }
        ui.text_edit_singleline(&mut app.comp_name_draft)
            .on_hover_text("the next comp's name");
    });
    ui.separator();
    // LC-003: the pinned pre-comp state.
    let can_last = !app.comp_last_state.is_empty();
    let row = egui::RichText::new("Last document state").weak();
    if ui
        .add_enabled(can_last, egui::Button::new(row))
        .on_disabled_hover_text("nothing applied yet")
        .clicked()
    {
        app.comp_restore_last();
    }
    ui.separator();
    if app.doc.comps.is_empty() {
        ui.weak("no comps yet");
        return;
    }
    let rows: Vec<(usize, String, bool)> = app
        .doc
        .comps
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.name.clone(), app.comp_selected == Some(i)))
        .collect();
    let (ctrl, shift) = ui.input(|i| (i.modifiers.ctrl, i.modifiers.shift));
    egui::ScrollArea::vertical().show(ui, |ui| {
        let mut delete = None;
        // LC-007 drag-reorder bookkeeping: the dragged row, the row
        // rects (for the insertion boundary), and whether the pointer
        // releases this frame (the button's own release API varies by
        // egui version — the global pointer is version-proof).
        let mut drag_src: Option<usize> = None;
        let mut rects: Vec<egui::Rect> = Vec::new();
        for (i, name, selected) in &rows {
            let in_multi = app.comp_multi.contains(i);
            let before = ui.min_rect();
            ui.horizontal(|ui| {
                let eye = if *selected { "◉" } else { "○" };
                let r = ui.add(
                    egui::Button::new(format!("{eye} {name}")).selected(in_multi || *selected),
                );
                if r.clicked() {
                    if ctrl {
                        app.comp_toggle_multi(*i);
                    } else if shift {
                        app.comp_range_select(*i);
                    } else {
                        // Plain click applies (LC-002) and resets to a
                        // single selection.
                        app.push_cmd(AppCmd::CompApply(*i));
                    }
                }
                if r.dragged() {
                    drag_src = Some(*i);
                }
                if ui
                    .small_button("💾")
                    .on_hover_text("overwrite with current visibility")
                    .clicked()
                {
                    app.push_cmd(AppCmd::CompSave(*i));
                }
                if ui.small_button("✕").on_hover_text("delete comp").clicked() {
                    delete = Some(*i);
                }
            });
            // The full row's extent, buttons included.
            rects.push(egui::Rect::from_min_max(before.min, ui.min_rect().max));
        }
        // The red insertion line while dragging, and the move on release
        // (CSP's LC-007 affordance). `at` is the boundary index on the
        // ORIGINAL order, 0..=len. A drag that ends outside the palette
        // has no boundary to read — the reorder is dropped, never guessed.
        if let Some(src) = drag_src
            && let Some(p) = ui.input(|i| i.pointer.interact_pos())
        {
            let mut at = rects.len();
            for (k, r) in rects.iter().enumerate() {
                if p.y < r.center().y {
                    at = k;
                    break;
                }
            }
            let (y, xr) = if at < rects.len() {
                (rects[at].top(), rects[at].x_range())
            } else {
                let last = rects[rects.len() - 1];
                (last.bottom(), last.x_range())
            };
            ui.painter()
                .hline(xr, y, egui::Stroke::new(2.0, egui::Color32::RED));
            if ui.input(|i| i.pointer.any_released()) {
                app.comp_move(src, at);
            }
        }
        if let Some(i) = delete {
            app.comp_delete_at(i);
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("◀ prev").clicked() {
            app.comp_step(false);
        }
        if ui.button("next ▶").clicked() {
            app.comp_step(true);
        }
        ui.checkbox(&mut app.comp_added_visible, "new layers visible")
            .on_hover_text("LC-006: layers added AFTER a snapshot default to this");
        if ui
            .add_enabled(
                app.comp_selected.is_some(),
                egui::Button::new("apply to all pages"),
            )
            .on_disabled_hover_text("select a comp first")
            .clicked()
        {
            if let Some(i) = app.comp_selected {
                app.push_cmd(AppCmd::CompApplyAllPages(i));
            }
        }
    });
}
