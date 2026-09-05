//! Tool Property for the Ruler tool (CSP 定規).
//!
//! The knobs that used to live only in the Layer ▸ Ruler menu — the
//! symmetry line count, the concentric ring spacing, which perspective set
//! the drag builds — are HERE now, beside the sub tool they belong to, which
//! is where CSP keeps them and where you are already looking when you pick a
//! ruler. The menu keeps every one of its rows: both sides push the same
//! commands, so neither can drift into meaning something else.

use super::*;
use crate::cmd::RulerKind;

/// The row in hand: what its gesture is, and the parameters only that row
/// uses. Empty middle for the rows that take no parameter at all (a straight
/// line, a guide) — those are one drag and one number, and inventing a knob
/// for them would be noise on a palette the owner reads at a glance.
pub(crate) fn sec_ruler_tool(ui: &mut egui::Ui, app: &mut App) {
    let kind = app.ruler_mode;
    ui.weak(kind.hint());
    match kind {
        // The three perspective sets are three sub tool rows, so the picker
        // is a shortcut back into the list rather than a separate setting —
        // it is the one place the count reads as a NUMBER (1/2/3 point),
        // which is how the owner asks for it.
        RulerKind::Perspective1 | RulerKind::Perspective | RulerKind::Perspective3 => {
            ui.label("Vanishing points");
            ui.horizontal(|ui| {
                for (n, k) in [
                    ("1 point", RulerKind::Perspective1),
                    ("2 point", RulerKind::Perspective),
                    ("3 point", RulerKind::Perspective3),
                ] {
                    if ui.selectable_label(kind == k, n).clicked() {
                        app.push_cmd(AppCmd::SetSubTool(crate::cmd::SubTool::Ruler(k)));
                    }
                }
            });
        }
        RulerKind::Symmetric => {
            let mut n = app.symmetric_lines as f32;
            if ValueBar::new("Symmetry lines", 2.0, 16.0)
                .step(1.0)
                .show(ui, &mut n)
                .changed()
            {
                app.symmetric_lines = n.round().clamp(2.0, 16.0) as u16;
            }
            ui.weak("the count the NEXT drag creates at");
            // Existing rulers are the command's business (it walks every
            // symmetric ruler on the page and spends an undo step doing
            // it); the slider above is only a creation default, so the two
            // are separate buttons rather than one slider pretending.
            if ui
                .button("Re-count the rulers on this page")
                .on_hover_text("steps every symmetric ruler through CSP's ladder: 2, 3, 4, 6, 8, 12, 16")
                .clicked()
            {
                app.push_cmd(AppCmd::RulerSymmetricCount);
            }
        }
        RulerKind::Concentric => {
            // Only once one exists, for the menu's own reason: unlike the
            // symmetry count, the spacing is not a creation default — a
            // drag sets it, and a click leaves it free.
            let dr = app.doc.rulers.items.iter().rev().find_map(|r| match r {
                mn_core::Ruler::Concentric { dr, .. } => Some(*dr),
                _ => None,
            });
            match dr {
                Some(dr) => {
                    ui.label(if dr <= 0.0 {
                        "Ring spacing: free".to_string()
                    } else {
                        format!("Ring spacing: {dr:.0} px")
                    });
                    if ui
                        .button("Next spacing")
                        .on_hover_text("free, 25, 50, 100, 200 px — free is where a click leaves it")
                        .clicked()
                    {
                        app.push_cmd(AppCmd::RulerRingSpacing);
                    }
                }
                None => {
                    ui.weak("the spacing appears here once a ring ruler is on the page");
                }
            }
        }
        _ => {}
    }
}

/// Snapping, and the one sentence that says where a ruler LIVES. Both
/// toggles are page state (they ride `doc.rulers`), so they are shown, not
/// remembered per sub tool.
pub(crate) fn sec_ruler_snap(ui: &mut egui::Ui, app: &mut App) {
    let mut on = app.doc.rulers.on;
    if ui
        .checkbox(&mut on, "Snap to rulers")
        .on_hover_text("off leaves the rulers drawn — it only stops strokes obeying them")
        .changed()
    {
        app.push_cmd(AppCmd::RulerSnapToggle);
    }
    let mut spec = app.doc.rulers.special_on;
    if ui
        .checkbox(&mut spec, "Snap to special rulers")
        .on_hover_text("parallel, radial, concentric, guide and symmetry sets")
        .changed()
    {
        app.push_cmd(AppCmd::RulerSpecialSnapToggle);
    }
    ui.weak(format!(
        "{} on this page — rulers belong to the page, so a page turn brings its own set",
        app.doc.rulers.items.len() + app.doc.rulers.curves.len()
    ));
}

pub(crate) fn sec_ruler_guide(ui: &mut egui::Ui, _app: &mut App) {
    ui.weak("the Object tool drags a ruler's handles; Layer ▸ Ruler clears them");
}
