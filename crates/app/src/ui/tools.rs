//! The tool strip palette body (icon grid + colour slots) — a dock tab since
//! round 21 (ui/dock.rs assembles the column).

use super::color::color_slots;
use crate::app::App;
use crate::cmd::{AppCmd, Tool};

use super::icons::Icon;
use super::theme;
use super::widgets::icon_btn;

// --- left column: Tool / Sub Tool / Tool Property palettes ---------------

/// The strip's own tools. `Tool::SelPen`/`Tool::SelEraser` are deliberately
/// ABSENT (owner, 2026-08-23: "select pen duplicates the G-pen with the same
/// icon"): CSP files 選択ペン / 選択消し as SUB tools of the Selection tool
/// with a fixed create-type, and so do we — they live in `ui/subtool.rs`'s
/// Selection list, reachable there, from Ctrl+K, and from the `,`/`.` cycle.
pub(super) const STRIP_TOOLS: [(Tool, Icon); 15] = [
    (Tool::Pen, Icon::Pen),
    (Tool::Eraser, Icon::Eraser),
    (Tool::Figure, Icon::Figure),
    (Tool::Gradient, Icon::Gradient),
    (Tool::Liquify, Icon::Liquify),
    (Tool::Fill, Icon::Fill),
    (Tool::Tone, Icon::Tone),
    (Tool::Select, Icon::Select),
    (Tool::Wand, Icon::Wand),
    (Tool::Object, Icon::Object),
    (Tool::Frame, Icon::Frame),
    (Tool::Balloon, Icon::Balloon),
    (Tool::Text, Icon::Text),
    (Tool::Eyedrop, Icon::Eyedrop),
    (Tool::Pan, Icon::Pan),
];

/// The tool key, shown in the strip tooltip (the owner's CSP set).
fn tool_key(t: Tool) -> &'static str {
    match t {
        Tool::Pen => "P",
        Tool::Eraser => "E",
        Tool::Figure => "F",
        Tool::Gradient => "V",
        Tool::Liquify => "",
        Tool::Fill => "G",
        // No key: the owner's CSP set has no spare letter, and main.rs's
        // table is where one would go.
        Tool::Tone => "",
        Tool::Select => "M",
        // Sub tools of Select now, not strip cells — no key of their own.
        Tool::SelPen | Tool::SelEraser => "",
        Tool::Wand => "W",
        Tool::Object => "O",
        Tool::Frame => "U",
        Tool::Balloon => "T",
        Tool::Text => "T",
        Tool::Eyedrop => "I",
        Tool::Pan => "H/R",
    }
}

/// Tool families, in `STRIP_TOOLS` order: draw+erase+fill, selection,
/// objects+utility. CSP separates its tool palette into exactly three such
/// blocks (`csp/150_tools_0002.png`, the numbered orange boxes).
const GROUPS: [usize; 3] = [7, 2, 6];

/// Icon cell. CSP's cell metric is ~34 px including its padding; ours is the
/// icon square plus [`GAP`], which lands in the same place.
const CELL: f32 = 22.0;
const GAP: f32 = 2.0;

pub(super) fn tool_palette_body(ui: &mut egui::Ui, app: &mut App) {
    // CSP's Tool palette is a compact icon grid anchored TOP-LEFT that reflows
    // to the palette width: one column when the leaf is a sliver, n columns
    // when it is wide, and the block always starts at the left margin.
    //
    // Round 21 pinned it to two columns and CENTRED the pair, which in a
    // floated palette read as a narrow strip of icons swimming mid-panel with
    // a wide dead band down the left (owner, 2026-08-22: "this is fucked and
    // not like csp"). Reflowing keeps the palette's height content-driven —
    // the thing P0-1 was actually about — without the dead band.
    let avail = ui.available_width();
    let cols = (((avail + GAP) / (CELL + GAP)).floor() as usize).max(1);
    let grid_w = cols as f32 * CELL + (cols - 1) as f32 * GAP;
    ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);

    let mut tools = STRIP_TOOLS.iter();
    for (g, count) in GROUPS.iter().enumerate() {
        if g > 0 {
            group_rule(ui, grid_w);
        }
        let block: Vec<_> = tools.by_ref().take(*count).collect();
        for row in block.chunks(cols) {
            ui.horizontal(|ui| {
                for (t, icon) in row {
                    let key = tool_key(*t);
                    let tip = if key.is_empty() {
                        t.label().to_owned()
                    } else {
                        format!("{} ({key})", t.label())
                    };
                    if icon_btn(ui, *icon, CELL, app.tool == *t, t.enabled(), &tip).clicked() {
                        app.push_cmd(AppCmd::SetTool(*t));
                    }
                }
            });
        }
    }
    ui.add_space(4.0);
    color_slots(ui, app);
}

/// The rule between two tool families: a hairline the width of the grid,
/// with air either side.
fn group_rule(ui: &mut egui::Ui, grid_w: f32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 7.0), egui::Sense::hover());
    let left = rect.left();
    ui.painter().hline(
        egui::Rangef::new(left, left + grid_w),
        rect.center().y,
        egui::Stroke::new(1.0, theme::c().outline),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::SubTool;

    /// The group counts ARE the strip: a mismatch silently drops the tail of
    /// the list (the iterator simply runs out), which is how a removed cell
    /// would take an unrelated tool with it.
    #[test]
    fn the_groups_cover_every_strip_tool() {
        assert_eq!(
            GROUPS.iter().sum::<usize>(),
            STRIP_TOOLS.len(),
            "every strip tool belongs to exactly one family block"
        );
    }

    /// The fold-in, both halves: the selection pen and eraser are gone from
    /// the strip AND still reachable — as Selection sub tools, which is
    /// where CSP keeps them.
    #[test]
    fn the_selection_paint_tools_moved_into_the_sub_tool_list() {
        for t in [Tool::SelPen, Tool::SelEraser] {
            assert!(
                !STRIP_TOOLS.iter().any(|(s, _)| *s == t),
                "{t:?} is a Selection sub tool, not a strip cell"
            );
            assert!(
                SubTool::ALL.iter().any(|s| s.tool() == t),
                "{t:?} must stay reachable from the Sub Tool list and Ctrl+K"
            );
        }
    }

    /// Two tools may never share a glyph — that is the whole complaint the
    /// fold-in came from ("select pen duplicates the G-pen with the same
    /// icon"), and it would come back the moment someone reused one.
    #[test]
    fn every_strip_tool_has_its_own_icon() {
        for (i, (t, icon)) in STRIP_TOOLS.iter().enumerate() {
            for (other, other_icon) in &STRIP_TOOLS[i + 1..] {
                assert_ne!(icon, other_icon, "{t:?} and {other:?} share a glyph");
            }
        }
    }
}
