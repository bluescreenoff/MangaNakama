//! FB-overflow ("burst out of the panel") — the Layer Property controls and
//! the Layers-palette insertion marker.
//!
//! Three switches, one idea: a layer inside a frame folder composites just
//! above that folder instead of inside its panel mask.
//!
//! 1. **The flag** — part 1, shipped: all-or-nothing on every side.
//! 2. **The cap** — the layer's OWN mask names the region allowed out.
//!    Inside it the art clears the border; outside it the art stays where it
//!    was, clipped by the panel. Nothing is deleted either way — see the
//!    copy in [`section`], which is careful about this because "release an
//!    edge" once read as "erase that stretch of border ink".
//! 3. **The seat** — how far up the stack the burst draws. Paint order is a
//!    stack, so this can only ever be a RUN from the panel upward; there is
//!    no over-A-under-B-over-A without drawing the layer twice.
//!
//! **Presentation choice (item 3): an insertion marker, not per-row ticks.**
//! The owner offered either. The marker won on three counts. It makes the
//! cascade rule unbreakable instead of merely explained — a position IS a
//! downward-closed set, so there is no state where a tick is on above an
//! untick. The palette rows are hand-painted at a budgeted width (see
//! `ROW_MENU_W`'s note about columns that appear under the pointer), and a
//! checkbox column that materialised only while a burst layer was selected
//! would reflow every name in the stack. And it reads as what it is: a seat
//! in the paint order, drawn where it takes effect.

use super::super::theme;
use crate::app::App;
use crate::cmd::AppCmd;

/// The Layer Property section: the flag, what the mask cap is doing, and
/// where the burst draws. Only offered where it means something — a
/// non-folder layer living inside a frame folder.
pub(super) fn section(ui: &mut egui::Ui, app: &mut App, i: usize) {
    let Some(l) = app.doc.layers.get(i) else {
        return;
    };
    if l.folder || app.doc.enclosing_frame_folder(i).is_none() {
        return;
    }
    let mut esc = l.escape_frame;
    if ui
        .checkbox(&mut esc, "Burst out of the panel")
        .on_hover_text(
            "this layer draws OVER the frame border and outside the panel mask — \
             the art overflows, the panel stays editable, the layer stays in its folder",
        )
        .changed()
    {
        app.push_cmd(AppCmd::SetLayerEscape(i, esc));
    }
    if !esc {
        return;
    }

    // The cap. It is the layer's ordinary mask, edited with the ordinary
    // mask tools — so this is a status line, not a control.
    let capped = app.doc.layers[i].breakout_mask().is_some();
    ui.indent("mn.breakout.cap", |ui| {
        if capped {
            ui.weak("Capped by this layer's mask.")
                .on_hover_text(
                    "the mask picks WHERE the art gets out: masked areas burst over the \
                     border, unmasked areas stay inside the panel exactly as before. \
                     Paint the mask with any brush. Nothing is erased — the border ink \
                     lives on the frame folder and is never touched.",
                );
        } else {
            ui.weak("Bursting on every side.").on_hover_text(
                "give this layer a mask to cap the spill: the masked region is the part \
                 allowed out, the rest stays clipped by the panel. Nothing is erased \
                 either way — the border ink belongs to the frame folder.",
            );
        }
    });

    // The seat.
    let candidates = app.doc.spill_candidates(i);
    if candidates.is_empty() {
        return;
    }
    // `current` is what was PICKED; `anchor` is where the art actually
    // lands, which is higher when the pick sits inside somebody else's
    // sealed folder (a burst cannot draw inside the panel it is covering).
    // The label says both rather than quietly disagreeing with the marker
    // painted in the Layers palette, which shows the anchor.
    let current = app.doc.spill_seat(i);
    let anchor = app.doc.spill_anchor(i);
    let name_of = |j: usize| short(&app.doc.layers[j].name);
    let label = match (current, anchor) {
        (None, _) | (_, None) => "its own panel only".to_owned(),
        (Some(s), Some(a)) if a == s => format!("over “{}” and below", name_of(s)),
        (Some(s), Some(a)) => format!("over “{}” → above “{}”", name_of(s), name_of(a)),
    };
    let mut pick: Option<Option<usize>> = None;
    ui.horizontal(|ui| {
        ui.label("Draws over:");
        egui::ComboBox::from_id_salt("mn.breakout.seat")
            .selected_text(label)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(current.is_none(), "its own panel only")
                    .clicked()
                {
                    pick = Some(None);
                }
                ui.separator();
                // Top-first, like the palette itself.
                for &j in candidates.iter().rev() {
                    let name = short(&app.doc.layers[j].name);
                    let text = if app.doc.layers[j].folder {
                        format!("{name}  (whole folder)")
                    } else {
                        name
                    };
                    if ui.selectable_label(current == Some(j), text).clicked() {
                        pick = Some(Some(j));
                    }
                }
            })
            .response
            .on_hover_text(
                "how far up the stack the burst reaches. Paint order is a stack, so \
                 picking one covers everything below it as well — there is no way to \
                 draw over one layer and under a lower one without painting twice.",
            );
    });
    if let Some(top) = pick {
        app.push_cmd(AppCmd::SetLayerSpillSeat(i, top));
    }
}

/// Layer names run long; the combo has one line.
fn short(name: &str) -> String {
    let n: Vec<char> = name.chars().collect();
    if n.len() <= 24 {
        return name.to_owned();
    }
    format!("{}…", n[..23].iter().collect::<String>())
}

/// The palette-side half of item 3: the row the ACTIVE breakout layer's art
/// is inserted above, if any. `None` = nothing to draw (no burst selected,
/// or it sits at its default seat inside its own frame folder, where the row
/// marker would say nothing the checkbox does not).
pub(super) fn marker_row(doc: &mn_core::Document) -> Option<usize> {
    let active = doc.active;
    doc.layers.get(active)?.escape_frame.then_some(())?;
    let seat = doc.spill_anchor(active)?;
    (seat != doc.enclosing_frame_folder(active)?).then_some(seat)
}

/// Paint the insertion marker along the TOP edge of `rect`: a line plus a
/// word, saying that the selected burst's art lands here in the paint order.
pub(super) fn paint_marker(ui: &egui::Ui, rect: egui::Rect) {
    let p = ui.painter();
    let y = rect.top() + 0.5;
    let c = theme::c().sel_edge;
    p.hline(rect.x_range(), y, egui::Stroke::new(2.0, c));
    p.text(
        egui::pos2(rect.right() - 4.0, y),
        egui::Align2::RIGHT_CENTER,
        "burst draws in here",
        egui::FontId::proportional(9.0),
        c,
    );
}
