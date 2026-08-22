//! The History palette (CV-003, TRIAGE 150): every labelled step this
//! session, oldest first; the current state highlighted; the redo branch
//! greyed below it (kept until a new action forks it away). Click a step
//! to travel there. Revert (CV-005) and Clear history (CV-004) live in
//! the File/Edit menus, not here.

use super::theme;
use crate::app::App;
use crate::cmd::AppCmd;

/// One step row: the number, the op label the document recorded ("New layer",
/// "Stroke", "Regenerate lines"), full palette width. CSP highlights the
/// current position and greys everything after it (`csp/270_canvas_0003.png`);
/// the rows used to be hardcoded WHITE / LIGHT_GRAY egui colours, which sat
/// outside the theme and made the redo branch nearly the same weight as the
/// past (parity P1-8).
fn step_row(ui: &mut egui::Ui, n: usize, label: &str, state: Step) -> egui::Response {
    let text = egui::RichText::new(format!("{n}. {label}"))
        .size(11.0)
        .color(match state {
            Step::Now => theme::TEXT_STRONG,
            Step::Past => theme::TEXT,
            Step::Redo => theme::TEXT_WEAK,
        });
    let text = if state == Step::Now { text.strong() } else { text };
    let fill = if state == Step::Now {
        theme::SEL_ROW
    } else {
        egui::Color32::TRANSPARENT
    };
    // `add` + `min_size`, not `add_sized`: the latter centres the label, and
    // a history list reads down its left edge.
    ui.add(
        egui::Button::new(text)
            .fill(fill)
            .stroke(egui::Stroke::NONE)
            .wrap_mode(egui::TextWrapMode::Truncate)
            .min_size(egui::vec2(ui.available_width(), 16.0)),
    )
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    /// Already applied, before the current position.
    Past,
    /// The document's current state.
    Now,
    /// Undone: the redo branch, still travellable until a new action forks it.
    Redo,
}

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
            let state = if i + 1 == n && future.is_empty() {
                Step::Now
            } else {
                Step::Past
            };
            if step_row(ui, i + 1, label, state).clicked() {
                app.push_cmd(AppCmd::HistoryTo { keep: i + 1 });
            }
        }
        // The current state row (after the newest past step, or alone when
        // the branch was just forked).
        if !future.is_empty() {
            ui.label(
                egui::RichText::new(format!("— current ({} undone) —", future.len()))
                    .size(10.5)
                    .color(theme::TEXT_WEAK),
            );
        }
        // Future steps: clicking future step j (0-based, oldest first)
        // redoes j+1 entries.
        for (j, label) in future.iter().enumerate() {
            if step_row(ui, n + j + 1, label, Step::Redo).clicked() {
                app.push_cmd(AppCmd::HistoryTo { keep: n + j + 1 });
            }
        }
    });
}
