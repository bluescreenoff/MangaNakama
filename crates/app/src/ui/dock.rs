//! The palette docking system (egui_dock): every palette is a **tab** — drag
//! it anywhere, drop it onto another to tab them together, split the column,
//! or tear it off into a free-floating window that can sit anywhere on
//! screen, over the canvas included. CSP pins its palettes to edge zones;
//! this is deliberately freer. Two dock columns (left of the canvas, right
//! of it) host the main surfaces; floating surfaces live wherever the user
//! dropped them. Everything persists to `ui.txt`.

use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use serde::{Deserialize, Serialize};

use super::color::{color_section, swatch_grid};
use super::layers::{layer_property, layer_section};
use super::materials::materials_palette;
use super::navigator::navigator_palette;
use super::pages::{pages_palette, preflight_palette};
use super::property::tool_property;
use super::subtool::sub_tool_list;
use super::theme;
use super::tools::tool_palette_body;
use crate::app::App;

/// A dockable palette. Serialized into `ui.txt` — never rename variants once
/// shipped (serde tags are the persisted API).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Palette {
    Tool,
    SubTool,
    ToolProperty,
    LayerProperty,
    Pages,
    Color,
    ColorSet,
    Layers,
    Preflight,
    Materials,
    Navigator,
    History,
    QuickAccess,
    LayerComps,
}

pub const ALL: [Palette; 14] = [
    Palette::Tool,
    Palette::SubTool,
    Palette::ToolProperty,
    Palette::LayerProperty,
    Palette::Pages,
    Palette::Color,
    Palette::ColorSet,
    Palette::Layers,
    Palette::Preflight,
    Palette::Materials,
    Palette::Navigator,
    Palette::History,
    Palette::QuickAccess,
    Palette::LayerComps,
];

impl Palette {
    pub fn title(self) -> &'static str {
        match self {
            Palette::Tool => "Tool",
            Palette::SubTool => "Sub Tool",
            Palette::ToolProperty => "Tool Property",
            Palette::LayerProperty => "Layer Property",
            Palette::Pages => "Pages",
            Palette::Color => "Color",
            Palette::ColorSet => "Color Set",
            Palette::Layers => "Layers",
            Palette::Preflight => "Preflight",
            Palette::Navigator => "Navigator",
            Palette::History => "History",
            Palette::QuickAccess => "Quick Access",
            Palette::LayerComps => "Layer Comps",
            Palette::Materials => "Materials",
        }
    }

    /// Bodies that should stretch to the node's full height (they manage
    /// their own scrolling / fill their rows).
    fn fills(self) -> bool {
        matches!(self, Palette::Layers | Palette::SubTool | Palette::Pages)
    }

    fn body(self, ui: &mut egui::Ui, app: &mut App) {
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 6,
                right: 6,
                top: 5,
                bottom: 6,
            })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                if self.fills() {
                    ui.set_min_height(ui.available_height());
                }
                ui.set_width(ui.available_width());
                match self {
                    Palette::Tool => tool_palette_body(ui, app),
                    Palette::SubTool => sub_tool_list(ui, app),
                    Palette::ToolProperty => tool_property(ui, app),
                    Palette::LayerProperty => layer_property(ui, app),
                    Palette::Pages => pages_palette(ui, app),
                    Palette::Color => color_section(ui, app),
                    Palette::ColorSet => swatch_grid(ui, app),
                    Palette::Layers => layer_section(ui, app),
                    Palette::Preflight => preflight_palette(ui, app),
                    Palette::Navigator => navigator_palette(ui, app),
                    Palette::Materials => materials_palette(ui, app),
                    Palette::History => super::history::history_palette(ui, app),
                    Palette::QuickAccess => super::quick::quick_palette(ui, app),
                    Palette::LayerComps => super::comps::comps_palette(ui, app),
                }
            });
    }
}

/// One dock column's state (the App fields' type).
pub type DockColumn = DockState<Palette>;

/// The default left column: Tool above Sub Tool above (Tool Property |
/// Layer Property) above Pages — the round-6..18 stacking, in dock form.
pub fn default_left() -> DockColumn {
    let mut dock = DockState::new(vec![Palette::Tool]);
    // split_*(parent, fraction, tabs) -> [retained_parent, new_node]; the
    // new node carries the tabs, so that is the one to split next.
    let tree = dock.main_surface_mut();
    let [_, sub] = tree.split_below(NodeIndex::root(), 0.34, vec![Palette::SubTool]);
    let [_, prop] = tree.split_below(
        sub,
        0.62,
        vec![Palette::ToolProperty, Palette::LayerProperty],
    );
    tree.split_below(prop, 0.6, vec![Palette::Pages]);
    dock
}

/// The default right column: (Color | Color Set) above Layers.
pub fn default_right() -> DockColumn {
    let mut dock = DockState::new(vec![Palette::Color, Palette::ColorSet]);
    dock.main_surface_mut()
        .split_below(NodeIndex::root(), 0.5, vec![Palette::Layers]);
    dock
}

/// Parse a persisted dock column; anything unreadable falls back to the
/// default (a stale `ui.txt` from an older build must never wedge startup).
pub fn from_json(s: &str, fallback: fn() -> DockState<Palette>) -> DockState<Palette> {
    serde_json::from_str(s).unwrap_or_else(|_| fallback())
}

pub fn to_json(dock: &DockState<Palette>) -> String {
    serde_json::to_string(dock).unwrap_or_default()
}

struct Viewer<'a> {
    app: &'a mut App,
}

impl TabViewer for Viewer<'_> {
    type Tab = Palette;

    fn id(&mut self, tab: &mut Palette) -> egui::Id {
        egui::Id::new(("mn.dock.tab", tab.title()))
    }

    fn title(&mut self, tab: &mut Palette) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Palette) {
        tab.body(ui, self.app);
    }

    fn closeable(&mut self, _tab: &mut Palette) -> bool {
        true
    }

    /// The bodies decide their own scrolling; a dock-level scroll would
    /// double-scroll fill bodies. (0.21 spells this `scroll_bars`.)
    fn scroll_bars(&self, _tab: &Palette) -> [bool; 2] {
        [false, false]
    }
}

/// Theme the dock chrome to the app's tokens. NOTE: egui_dock's own
/// `Style::default()` is a LIGHT style — white tab bodies, white tabs, black
/// outlines (owner bug report "some parts are white now", 2026-08-16). Always
/// derive from `Style::from_egui` (dark-aware) first, then apply our tokens:
/// the tab strip is the old palette title strip (HEADER), the body is the old
/// palette body (PANEL), and the active tab merges into the body.
fn dock_style(ui: &egui::Ui) -> Style {
    let mut s = Style::from_egui(ui.style());

    s.tab_bar.bg_fill = theme::HEADER;
    s.tab_bar.hline_color = theme::BORDER;
    s.tab_bar.height = 20.0;
    // CSP tab strips: tabs divide the bar evenly and long titles ellipsize
    // (the vendored truncation patch) instead of overflowing the × buttons.
    s.tab_bar.fill_tab_bar = true;

    let tab_state = |bg: egui::Color32, text: egui::Color32| egui_dock::TabInteractionStyle {
        bg_fill: bg,
        text_color: text,
        outline_color: theme::BORDER,
        corner_radius: egui::CornerRadius::same(0),
    };
    s.tab.active = tab_state(theme::PANEL, theme::TEXT_STRONG);
    s.tab.inactive = tab_state(theme::HEADER, theme::TEXT_WEAK);
    s.tab.hovered = tab_state(theme::HOVER, theme::TEXT);
    s.tab.focused = tab_state(theme::HOVER, theme::TEXT_STRONG);
    s.tab.active_with_kb_focus = s.tab.active.clone();
    s.tab.inactive_with_kb_focus = s.tab.inactive.clone();
    s.tab.focused_with_kb_focus = s.tab.focused.clone();
    s.tab.spacing = 0.0;
    s.tab.hline_below_active_tab_name = false;

    s.tab.tab_body.bg_fill = theme::PANEL;
    s.tab.tab_body.stroke = egui::Stroke::NONE;
    s.tab.tab_body.corner_radius = egui::CornerRadius::same(0);

    s.separator.color_idle = theme::BORDER;
    s.separator.color_hovered = theme::ACCENT;
    s.separator.color_dragged = theme::ACCENT;
    s.separator.width = 1.0;
    // Easy to hit with a pen (upstream's 2.0 total ≈ 1pt per side — the owner
    // reported the resize cursor only appearing on an exact hit).
    s.separator.extra_interact_width = 12.0;

    // Tab × / floating-window buttons: transparent until hovered, our greys.
    let b = &mut s.buttons;
    b.close_tab_bg_fill = theme::HOVER;
    b.close_tab_color = theme::TEXT_WEAK;
    b.close_tab_active_color = theme::TEXT_STRONG;
    b.add_tab_bg_fill = theme::HOVER;
    b.add_tab_color = theme::TEXT_WEAK;
    b.add_tab_active_color = theme::TEXT_STRONG;
    b.add_tab_border_color = theme::BORDER;
    b.close_all_tabs_bg_fill = theme::HOVER;
    b.close_all_tabs_color = theme::TEXT_WEAK;
    b.close_all_tabs_active_color = theme::TEXT_STRONG;
    b.close_all_tabs_border_color = theme::BORDER;
    b.collapse_tabs_bg_fill = theme::HOVER;
    b.collapse_tabs_color = theme::TEXT_WEAK;
    b.collapse_tabs_active_color = theme::TEXT_STRONG;
    b.collapse_tabs_border_color = theme::BORDER;
    b.minimize_window_bg_fill = theme::HOVER;
    b.minimize_window_color = theme::TEXT_WEAK;
    b.minimize_window_active_color = theme::TEXT_STRONG;
    b.minimize_window_border_color = theme::BORDER;
    b.show_tab_bar_color = theme::TEXT_WEAK;
    b.show_tab_bar_active_color = theme::TEXT_STRONG;

    s.main_surface_border_stroke = egui::Stroke::NONE;
    s.dock_area_padding = Some(egui::Margin::same(1));
    s
}

/// Render one dock column inside `ui`. The state is swapped out of `App` for
/// the call (the viewer borrows the app for the tab bodies — egui immediate
/// mode, two mutable aliases of the same struct would not fly).
///
/// Each column gets its OWN DockArea id: egui widget ids are derived from it,
/// and two sibling DockAreas sharing the default id clobber each other's tab
/// widget rects every frame (the round-22 "can't drag palettes" bug — the
/// left column's tabs literally lost their hit rects to the right column's).
/// Cross-column drag-and-drop still works: the vendored crate carries the
/// DnD payloads under GLOBAL keys (vendor/PATCHES.md).
pub fn column(ui: &mut egui::Ui, app: &mut App, left: bool) {
    let placeholder = if left {
        default_left()
    } else {
        default_right()
    };
    let mut dock = std::mem::replace(
        if left {
            &mut app.dock_left
        } else {
            &mut app.dock_right
        },
        placeholder,
    );
    DockArea::new(&mut dock)
        .id(egui::Id::new(if left {
            "mn.dock.left"
        } else {
            "mn.dock.right"
        }))
        .style(dock_style(ui))
        // CSP's strips have no close-all control — with per-tab ×s (on the
        // active tab, vendor patch #7b) a strip-end ✕ reads as a stray
        // second close button (owner report 2026-08-17).
        .show_leaf_close_all_buttons(false)
        .show_inside(ui, &mut Viewer { app });
    if left {
        app.dock_left = dock;
    } else {
        app.dock_right = dock;
    }
}

/// Is this palette open anywhere (either column, floating included)?
pub fn is_open(app: &App, p: Palette) -> bool {
    app.dock_left.iter_all_tabs().any(|(_, t)| *t == p)
        || app.dock_right.iter_all_tabs().any(|(_, t)| *t == p)
}

/// Reopen a closed palette: back into the right column's first leaf
/// (CSP's Window ▸ palette behaviour).
pub fn reopen(app: &mut App, p: Palette) {
    if !is_open(app, p) {
        app.dock_right.main_surface_mut().push_to_first_leaf(p);
    }
}
