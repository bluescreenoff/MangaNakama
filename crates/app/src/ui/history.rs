//! The History palette (CV-003, TRIAGE 150): every labelled step this
//! session, oldest first; the current state highlighted; the redo branch
//! greyed below it (kept until a new action forks it away). Click a step
//! to travel there. Revert (CV-005) and Clear history (CV-004) live in
//! the File/Edit menus, not here.

use crate::app::App;
use crate::cmd::AppCmd;

pub fn history_palette(ui: &mut egui::Ui, app: &mut App) {
    let past = app.doc.undo_labels().to_vec();
    let future = app.doc.redo_labels().to_vec();
    if past.is_empty() && future.is_empty() {
        ui.weak("nothing yet this session");
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        // Past steps: clicking step i leaves i+1 undo entries (state i).
        let n = past.len();
        for (i, label) in past.iter().enumerate() {
            let is_now = i + 1 == n && future.is_empty();
            let txt = egui::RichText::new(format!("{}. {label}", i + 1));
            let txt = if is_now {
                txt.strong().color(egui::Color32::WHITE)
            } else {
                txt.color(egui::Color32::LIGHT_GRAY)
            };
            if ui
                .add(egui::Button::new(txt).fill(egui::Color32::TRANSPARENT))
                .clicked()
            {
                app.push_cmd(AppCmd::HistoryTo { keep: i + 1 });
            }
        }
        // The current state row (after the newest past step, or alone when
        // the branch was just forked).
        let txt = if future.is_empty() {
            egui::RichText::new("— current state —").weak()
        } else {
            egui::RichText::new(format!("— current ({} undone) —", future.len())).weak()
        };
        ui.label(txt);
        // Future steps: clicking future step j (0-based, oldest first)
        // redoes j+1 entries.
        for (j, label) in future.iter().enumerate() {
            let txt = egui::RichText::new(format!("{}. {label}", n + j + 1)).weak();
            if ui
                .add(egui::Button::new(txt).fill(egui::Color32::TRANSPARENT))
                .clicked()
            {
                app.push_cmd(AppCmd::HistoryTo { keep: n + j + 1 });
            }
        }
    });
}
