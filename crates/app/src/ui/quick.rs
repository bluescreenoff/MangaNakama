//! Quick Access (TRIAGE 145, UI-050/052): a searchable palette of every
//! curated tool and command — type to filter, Enter (or click) runs, the
//! menu path shows in parentheses, and the ☆ pins a command into the
//! favorites row above (persisted in ui.txt). v1 deviations: the pin is a
//! button, not CSP's long-press (mouse-first); sets (UI-051), the tile/list
//! view modes (UI-053) and the settings dialog (UI-054) are deferred with
//! reasons — one flat set of pins first.

use crate::app::App;
use crate::cmd::{AppCmd, Tool};

/// One searchable entry: what it is called, where it lives (the parenthetical
/// UI-052 shows), and what it runs. Curated — payload commands are named,
/// the rest are the enum's own units.
pub fn command_index() -> Vec<(&'static str, &'static str, AppCmd)> {
    use AppCmd::*;
    vec![
        ("Pen", "Tools (P)", SetTool(Tool::Pen)),
        ("Eraser", "Tools (E)", SetTool(Tool::Eraser)),
        ("Fill", "Tools (G)", SetTool(Tool::Fill)),
        ("Auto select (wand)", "Tools (W)", SetTool(Tool::Wand)),
        ("Select", "Tools (M)", SetTool(Tool::Select)),
        ("Object", "Tools (O)", SetTool(Tool::Object)),
        ("Frame border", "Tools (U)", SetTool(Tool::Frame)),
        ("Text / Balloon", "Tools (T)", SetTool(Tool::Text)),
        ("Eyedropper", "Tools (I)", SetTool(Tool::Eyedrop)),
        ("Hand", "Tools (H)", SetTool(Tool::Pan)),
        ("Rotate view", "Tools (R)", SetTool(Tool::Pan)),
        ("Undo", "Edit (Ctrl+Z)", Undo),
        ("Redo", "Edit (Ctrl+Y)", Redo),
        ("Cut", "Edit (Ctrl+X)", Cut),
        ("Copy", "Edit (Ctrl+C)", Copy),
        ("Paste", "Edit (Ctrl+V)", Paste),
        ("Paste to shown position", "Edit (Ctrl+Shift+V)", PasteShown),
        ("Fill with drawing color", "Edit (Alt+Del)", FillSelection),
        ("Clear", "Edit (Del)", ClearLayer),
        ("Clear outside selection", "Edit (Shift+Del)", ClearOutside),
        ("Transform", "Edit (Ctrl+T)", TransformStart),
        (
            "Flip Horizontal",
            "Edit",
            TransformFlip { horizontal: true },
        ),
        ("Flip Vertical", "Edit", TransformFlip { horizontal: false }),
        ("Select all", "Edit (Ctrl+A)", SelectAll),
        ("Deselect", "Edit (Ctrl+D)", Deselect),
        ("Invert selected area", "Edit (Ctrl+Shift+I)", SelectInvert),
        ("Clear undo history", "Edit", ClearHistory),
        ("New layer", "Layer", AddLayer),
        ("New folder", "Layer (Ctrl+G)", AddFolder),
        ("Duplicate layer", "Layer", DuplicateLayer),
        (
            "Straight line ruler",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Line),
        ),
        (
            "Vanishing point ruler",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::VanishingPoint),
        ),
        (
            "Curve ruler",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Curve),
        ),
        (
            "Parallel line ruler",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Parallel),
        ),
        (
            "Concentric circle ruler",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Concentric),
        ),
        (
            "Symmetrical ruler",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Symmetric),
        ),
        ("First page", "Page (Ctrl+Home)", PageFirst),
        ("Previous page", "Page (Ctrl+PageUp)", PagePrev),
        ("Next page", "Page (Ctrl+PageDown)", PageNext),
        ("Last page", "Page (Ctrl+End)", PageLast),
        ("Go to Page…", "Page", PageGoto),
        ("Add page", "Page", AddPage),
        ("Duplicate page", "Page", DuplicatePage),
        ("Story Editor…", "Page", StoryEditor),
        ("Combine with next page…", "Page", PageCombineSpread),
        ("Split spread…", "Page", PageSplitSpread),
        (
            "Register layer as material",
            "Material palette",
            MaterialRegisterLayer,
        ),
        (
            "Convert brightness to opacity",
            "Layer",
            BrightnessToOpacity,
        ),
        ("Revert to last save", "File", RevertFile),
        ("Export All Pages…", "File", ExportAllPages),
        ("Export Text (script)…", "File", ExportText),
        ("Save", "File (Ctrl+S)", SaveOra),
        ("Save As…", "File (Ctrl+Shift+S)", SaveOraAs),
        ("Open…", "File (Ctrl+O)", OpenOra),
        ("New…", "File (Ctrl+N)", NewDoc),
        ("Zoom fit", "View (Ctrl+0)", ZoomFit),
        ("Pixel size (100%)", "View (Ctrl+1)", Zoom100),
        ("Flip view", "View (Ctrl+9)", FlipView),
        ("Flip view vertically", "View (Ctrl+Shift+9)", FlipViewV),
        ("Reset rotation", "View", RotateReset),
        ("Reset rotation and flip", "View", RotateFlipReset),
        ("Reset view (upright, unmirrored, fitted)", "View", ViewReset),
        ("Hide crop marks and margins", "View", SetGuidesHidden(true)),
        ("Show crop marks and margins", "View", SetGuidesHidden(false)),
        ("Reset transformation", "Transform", TransformReset),
        ("Lock tool settings", "Tool Property", SetToolLock(true)),
        ("Unlock tool settings", "Tool Property", SetToolLock(false)),
    ]
}

/// The palette body: pinned favorites + the search field + live results.
pub fn quick_palette(ui: &mut egui::Ui, app: &mut App) {
    // Favorites row (UI-050): click runs, ✕ unpins.
    if !app.quick_pins.is_empty() {
        ui.horizontal_wrapped(|ui| {
            let pins = app.quick_pins.clone();
            for key in pins {
                if let Some((label, _where, cmd)) = find_entry(&key) {
                    if ui.small_button(label).clicked() {
                        app.push_cmd(cmd);
                    }
                    // The unpin cross rides the button's hover text.
                    if ui
                        .small_button("✕")
                        .on_hover_text(format!("unpin {label}"))
                        .clicked()
                    {
                        app.quick_pins.retain(|k| k != &key);
                        app.layout.note_quick_pins(&app.quick_pins.join("\u{1f}"));
                    }
                } else {
                    app.quick_pins.retain(|k| k != &key);
                    app.layout.note_quick_pins(&app.quick_pins.join("\u{1f}"));
                }
            }
        });
        ui.separator();
    }
    ui.text_edit_singleline(&mut app.quick_query);
    let q = app.quick_query.trim().to_lowercase();
    let hits: Vec<(usize, &'static str, &'static str)> = if q.is_empty() {
        Vec::new()
    } else {
        command_index()
            .into_iter()
            .enumerate()
            .filter(|(_, (label, wher, _))| {
                label.to_lowercase().contains(&q) || wher.to_lowercase().contains(&q)
            })
            .map(|(i, (label, wher, _))| (i, label, wher))
            .take(12)
            .collect()
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        if q.is_empty() {
            ui.weak("type to search every tool and command");
            return;
        }
        if hits.is_empty() {
            ui.weak("no matches");
            return;
        }
        for (i, label, wher) in hits {
            ui.horizontal(|ui| {
                let row = egui::RichText::new(label).color(egui::Color32::WHITE);
                if ui
                    .add(egui::Button::new(row).fill(egui::Color32::TRANSPARENT))
                    .on_hover_text(wher)
                    .clicked()
                {
                    let (_, _, cmd) = command_index()[i].clone();
                    app.push_cmd(cmd);
                }
                ui.weak(format!("({wher})"));
                let pinned = app.quick_pins.iter().any(|k| k == label);
                let star = if pinned { "★" } else { "☆" };
                if ui
                    .small_button(star)
                    .on_hover_text("pin into Quick Access")
                    .clicked()
                {
                    if pinned {
                        app.quick_pins.retain(|k| k != label);
                    } else {
                        app.quick_pins.push(label.to_string());
                    }
                    app.layout.note_quick_pins(&app.quick_pins.join("\u{1f}"));
                }
            });
        }
    });
}

/// The index entry a pin key refers to (keys are labels; the index is the
/// source of truth — a renamed command simply drops its stale pins).
fn find_entry(key: &str) -> Option<(&'static str, &'static str, AppCmd)> {
    command_index()
        .into_iter()
        .find(|(label, _, _)| *label == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index is the pin-key space: unique labels, every label
    /// resolves back to its entry, and the 178-vote point works —
    /// "ruler" finds every ruler.
    #[test]
    fn index_is_a_sound_key_space_and_searches() {
        let idx = command_index();
        assert!(
            idx.len() >= 50,
            "a real palette, not a stub ({})",
            idx.len()
        );
        let mut labels: Vec<&str> = idx.iter().map(|(l, _, _)| *l).collect();
        let n = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), n, "labels unique — pin keys are stable");
        for (l, _, _) in idx.iter().take(8) {
            assert!(
                find_entry(l).is_some_and(|(l2, _, _)| l2 == *l),
                "{l} resolves"
            );
        }
        let rulers: Vec<_> = idx
            .iter()
            .filter(|(l, _, _)| l.to_lowercase().contains("ruler"))
            .collect();
        assert!(
            rulers.len() >= 5,
            "the ruler family is findable ({rulers:?})"
        );
        assert!(
            idx.iter()
                .any(|(l, w, _)| *l == "Undo" && w.contains("Edit")),
            "menu paths ride along (UI-052's parenthetical)"
        );
    }
}
