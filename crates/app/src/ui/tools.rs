//! The tool strip palette body (icon grid + colour slots) — a dock tab since
//! round 21 (ui/dock.rs assembles the column).

use super::color::color_slots;
use crate::app::App;
use crate::cmd::{AppCmd, Tool};

use super::icons::Icon;
use super::theme;
use super::widgets::icon_btn;

// --- left column: Tool / Sub Tool / Tool Property palettes ---------------

const STRIP_TOOLS: [(Tool, Icon); 16] = [
    (Tool::Pen, Icon::Pen),
    (Tool::Eraser, Icon::Eraser),
    (Tool::Figure, Icon::Figure),
    (Tool::Gradient, Icon::Gradient),
    (Tool::Fill, Icon::Fill),
    (Tool::Tone, Icon::Tone),
    (Tool::Select, Icon::Select),
    (Tool::SelPen, Icon::Select),
    (Tool::SelEraser, Icon::Eraser),
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
        Tool::Fill => "G",
        // No key: the owner's CSP set has no spare letter, and main.rs's
        // table is where one would go.
        Tool::Tone => "",
        Tool::Select => "M",
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
/// blocks (`csp/150_tools_0002.png`, the numbered orange boxes). Every count
/// is EVEN so no block ends with a holed cell in the two-column grid — that
/// is why the split sits where it does rather than one tool either way.
const GROUPS: [usize; 3] = [6, 4, 6];

/// Icon cell. CSP's cell metric is ~34 px including its padding; ours is the
/// icon square plus [`GAP`], which lands in the same place.
const CELL: f32 = 22.0;
const GAP: f32 = 2.0;

pub(super) fn tool_palette_body(ui: &mut egui::Ui, app: &mut App) {
    // CSP's Tool palette is a FIXED TWO-COLUMN strip (csp/150_tools_0002.png):
    // the grid does NOT reflow with the leaf width. Ours used to wrap to fill
    // the leaf — 8 icons a row at the shipped width, 10 at 2560 — which left a
    // tall empty void under the palette and read as unfinished (parity P0-1).
    let avail = ui.available_width();
    let cols = if avail >= 2.0 * CELL + GAP { 2 } else { 1 };
    let grid_w = cols as f32 * CELL + (cols - 1) as f32 * GAP;
    // The column pair is centred in whatever width the leaf happens to have.
    let indent = ((avail - grid_w) * 0.5).max(0.0);
    ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);

    let mut tools = STRIP_TOOLS.iter();
    for (g, count) in GROUPS.iter().enumerate() {
        if g > 0 {
            group_rule(ui, indent, grid_w);
        }
        let block: Vec<_> = tools.by_ref().take(*count).collect();
        for row in block.chunks(cols) {
            ui.horizontal(|ui| {
                ui.add_space(indent);
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
    ui.add_space(3.0);
    color_slots(ui, app);
}

/// The rule between two tool families: a hairline the width of the grid,
/// with air either side.
fn group_rule(ui: &mut egui::Ui, indent: f32, grid_w: f32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 7.0),
        egui::Sense::hover(),
    );
    let left = rect.left() + indent;
    ui.painter().hline(
        egui::Rangef::new(left, left + grid_w),
        rect.center().y,
        egui::Stroke::new(1.0, theme::OUTLINE),
    );
}
