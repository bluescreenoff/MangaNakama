//! The tool strip palette body (icon grid + colour slots) — a dock tab since
//! round 21 (ui/dock.rs assembles the column).

use super::color::color_slots;
use crate::app::App;
use crate::cmd::{AppCmd, Tool};

use super::icons::Icon;
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

pub(super) fn tool_palette_body(ui: &mut egui::Ui, app: &mut App) {
    // CSP's compact icon grid: tight rows that wrap, no captions.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
        for (t, icon) in STRIP_TOOLS {
            let key = tool_key(t);
            let tip = if key.is_empty() {
                t.label().to_owned()
            } else {
                format!("{} ({key})", t.label())
            };
            if icon_btn(ui, icon, 22.0, app.tool == t, t.enabled(), &tip).clicked() {
                app.push_cmd(AppCmd::SetTool(t));
            }
        }
    });
    ui.add_space(3.0);
    color_slots(ui, app);
}
