//! Docking 2 (docs/DOCKING-2.md): the whole workspace is ONE dock tree and
//! the CANVAS IS A PANE in it, beside the palettes — drag any tab anywhere,
//! split any which way, tear palettes off into floating windows. The canvas
//! pane's body is a transparent hole the wgpu canvas shows through; palettes
//! and canvas never share a tab bar (a palette tabbed over the canvas would
//! bury the drawing surface — vendored patch #16), and the canvas pane can
//! neither close nor float (`owns_pointer` routes pen input by "inside the
//! canvas rect", and a floating egui window is never canvas). Everything
//! persists to `ui.txt` under `dock_tree=`; the legacy two-column keys
//! migrate on first load (vendored patch #17 grafts them into one tree).

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
    /// Recordable action sequences (CSP Auto Action).
    Actions,
    /// Reference images: the list palette; each image opens its own free
    /// floating viewer window (`ui/refs.rs`).
    References,
}

/// One pane of the dock tree: a palette, or the canvas itself. Serialized
/// into `ui.txt` (`dock_tree=`) — the variant tags are the persisted API.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Pane {
    Palette(Palette),
    /// The drawing surface. Phase 1 (docs/DOCKING-2.md): exactly one, it
    /// follows the active document, and the document tab strip is drawn
    /// inside its body. Phase 2 gives every open page its own canvas pane.
    Canvas,
}

pub const ALL: [Palette; 16] = [
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
    Palette::Actions,
    Palette::References,
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
            Palette::Actions => "Auto Actions",
            Palette::References => "References",
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
                    Palette::Actions => super::actions::actions_palette(ui, app),
                    Palette::References => super::refs::references_palette(ui, app),
                }
            });
    }
}

impl Pane {
    fn title(self) -> &'static str {
        match self {
            Pane::Palette(p) => p.title(),
            Pane::Canvas => "Canvas",
        }
    }
}

/// The one dock tree (the App field's type). Legacy: a pre-docking-2 ui.txt
/// carries two `DockState<Palette>` COLUMNS instead; `merge_columns` folds
/// them into this.
pub type DockTree = DockState<Pane>;
/// A legacy dock column, kept as a named type because migration still parses
/// them (`dock_left=` / `dock_right=` in old ui.txt and old workspaces).
pub type DockColumn = DockState<Palette>;

/// The default left column: Tool above Sub Tool above (Tool Property |
/// Layer Property) above Pages — the round-6..18 stacking, in dock form.
/// Still built as a legacy COLUMN: the default tree is `merge_columns` over
/// the two default columns, so the migration path is exercised on every
/// fresh start rather than only on upgrade day.
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

/// The default right column: (Color | Color Set) above (Layers | Auto
/// Actions) — the actions tab sits beside Layers like CSP's.
pub fn default_right() -> DockColumn {
    let mut dock = DockState::new(vec![Palette::Color, Palette::ColorSet]);
    dock.main_surface_mut().split_below(
        NodeIndex::root(),
        0.5,
        vec![Palette::Layers, Palette::Actions],
    );
    dock
}

/// The default whole-window tree: default columns either side of the canvas.
pub fn default_tree() -> DockTree {
    let l = serde_json::to_string(&default_left()).unwrap_or_default();
    let r = serde_json::to_string(&default_right()).unwrap_or_default();
    // 186 / 208 pt over a 1280pt window — the shipped column widths as
    // fractions. Drag a separator and the tree remembers; these only seed.
    merge_columns(&l, &r, 186.0, 208.0, 1280.0).unwrap_or_else(minimal_tree)
}

/// The unlosable fallback: a bare canvas. Only reachable when the default
/// columns fail to serialize, i.e. never — but a dock tree with no canvas
/// pane must not be constructible from any path.
fn minimal_tree() -> DockTree {
    DockState::new(vec![Pane::Canvas])
}

/// Parse the persisted `dock_tree=`; anything unreadable, and any tree that
/// lost its canvas pane (hand-edited ui.txt), falls back to the default —
/// a stale or mangled layout must never wedge startup or bury the canvas.
pub fn from_json_tree(s: &str) -> DockTree {
    // Sanitize non-finite rects first (serde_json wrote them as `null`,
    // which it then refuses to read back as f32): a layout saved before its
    // newest float window ever laid out must not cost the whole tree.
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(s) else {
        return default_tree();
    };
    sanitize_rects(&mut v);
    let Ok(mut tree) = serde_json::from_value::<DockTree>(v) else {
        return default_tree();
    };
    let canvases = tree
        .iter_all_tabs()
        .filter(|(_, t)| **t == Pane::Canvas)
        .count();
    match canvases {
        1 => tree,
        0 => default_tree(),
        // Phase 1 owns exactly one canvas pane; extras (hand-edits, or a
        // future build's layout) collapse to the first.
        _ => {
            dedupe_canvases(&mut tree);
            tree
        }
    }
}

/// Remove every canvas pane after the first (tree order).
fn dedupe_canvases(tree: &mut DockTree) {
    loop {
        let mut seen = false;
        let extra = tree.iter_all_tabs().find_map(|(p, t)| {
            if *t == Pane::Canvas {
                if seen {
                    return Some(p);
                }
                seen = true;
            }
            None
        });
        match extra {
            Some(path) => {
                tree.remove_tab(path);
            }
            None => break,
        }
    }
}

pub fn to_json_tree(tree: &DockTree) -> String {
    serde_json::to_string(tree).unwrap_or_default()
}

pub fn to_json(dock: &DockColumn) -> String {
    serde_json::to_string(dock).unwrap_or_default()
}

/// Rewrite a serialized `DockState<Palette>` into a `DockState<Pane>` by
/// wrapping every tab: `"Tool"` → `{"Palette":"Tool"}`. Tabs only ever
/// appear under a leaf's `"tabs"` array, so the walk keys on that.
fn wrap_palettes(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Array(items) => {
            for item in items {
                wrap_palettes(item);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if key == "tabs" && val.is_array() {
                    for tab in val.as_array_mut().expect("checked is_array") {
                        *tab = serde_json::json!({ "Palette": tab.take() });
                    }
                } else if key == "rect" || key == "viewport" {
                    // A never-laid-out leaf's rect is `Rect::NOTHING`
                    // (±inf); serde_json writes non-finite floats as
                    // `null` and refuses to read `null` back as f32.
                    // Rects are recomputed on the next laid-out frame,
                    // so any finite stand-in is fine.
                    zero_nulls(val);
                    wrap_palettes(val);
                } else {
                    wrap_palettes(val);
                }
            }
        }
        _ => {}
    }
}

fn zero_nulls(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Null => *v = serde_json::json!(0.0),
        serde_json::Value::Array(items) => items.iter_mut().for_each(zero_nulls),
        serde_json::Value::Object(map) => map.values_mut().for_each(zero_nulls),
        _ => {}
    }
}

/// `zero_nulls` under every `rect`/`viewport` key, nothing else — an
/// honest `null` elsewhere (`focused_node: None`) must stay a `null`.
fn sanitize_rects(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Array(items) => items.iter_mut().for_each(sanitize_rects),
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if key == "rect" || key == "viewport" {
                    zero_nulls(val);
                } else if key == "screen_rect" && val.is_object() {
                    // `Option<Rect>`: a bare `null` is an honest None and
                    // stays; only nulls INSIDE a present rect are repaired.
                    zero_nulls(val);
                } else {
                    sanitize_rects(val);
                }
            }
        }
        _ => {}
    }
}

fn parse_column_as_panes(json: &str) -> Option<DockState<Pane>> {
    let mut v: serde_json::Value = serde_json::from_str(json).ok()?;
    wrap_palettes(&mut v);
    match serde_json::from_value(v) {
        Ok(ds) => Some(ds),
        Err(e) => {
            // The caller falls back to the default tree; say why on the
            // console so a lost layout is diagnosable, not a mystery.
            println!("[dock] legacy column migration failed: {e}");
            None
        }
    }
}

/// Fold the two legacy dock columns into one tree around a center canvas
/// pane: `[left | canvas | right]`, fractions seeded from the persisted
/// column widths against the window width. Column-internal splits and
/// floating palettes carry over intact (vendored patch #17). `None` only
/// when a side fails to parse — the caller falls back to the default tree.
pub fn merge_columns(
    left_json: &str,
    right_json: &str,
    left_w: f32,
    right_w: f32,
    win_w: f32,
) -> Option<DockTree> {
    let left = parse_column_as_panes(left_json)?;
    let right = parse_column_as_panes(right_json)?;
    let win_w = win_w.max(400.0);
    let lfrac = (left_w / win_w).clamp(0.08, 0.4);
    let rfrac = (right_w / win_w).clamp(0.08, 0.4);

    let mut merged = DockState::new(vec![Pane::Canvas]);
    let has_tabs = |ds: &DockState<Pane>| ds.main_surface().num_tabs() > 0;

    // split() refuses a parent as the new child, so each side goes in as a
    // placeholder leaf that patch #17 then overwrites with the whole column.
    let mut canvas_node = NodeIndex::root();
    if has_tabs(&left) {
        let tree = merged.main_surface_mut();
        let [canvas, slot] = tree.split_left(
            canvas_node,
            1.0 - lfrac,
            vec![Pane::Palette(Palette::Tool)],
        );
        tree.graft_at(slot, left.main_surface());
        canvas_node = canvas;
    }
    if has_tabs(&right) {
        // The canvas node's area is what is left of the window after the
        // left column; the right fraction is relative to THAT.
        let rel = (rfrac / (1.0 - lfrac)).clamp(0.08, 0.5);
        let tree = merged.main_surface_mut();
        let [_, slot] = tree.split_right(
            canvas_node,
            1.0 - rel,
            vec![Pane::Palette(Palette::Layers)],
        );
        tree.graft_at(slot, right.main_surface());
    }
    merged.absorb_windows(left);
    merged.absorb_windows(right);
    Some(merged)
}

struct Viewer<'a> {
    app: &'a mut App,
}

impl TabViewer for Viewer<'_> {
    type Tab = Pane;

    fn id(&mut self, tab: &mut Pane) -> egui::Id {
        egui::Id::new(("mn.dock.tab", tab.title()))
    }

    fn title(&mut self, tab: &mut Pane) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Pane) {
        match tab {
            Pane::Palette(p) => p.body(ui, self.app),
            Pane::Canvas => canvas_pane_body(ui, self.app),
        }
    }

    fn closeable(&mut self, tab: &mut Pane) -> bool {
        // The app always shows a canvas; the pane cannot close (phase 1 —
        // with pages-as-panes, closing becomes the document close flow).
        !matches!(tab, Pane::Canvas)
    }

    /// The canvas never floats: a floating egui window is a non-background
    /// layer, so `Shell::owns_pointer` would hand every pen event inside it
    /// to egui and the canvas would go permanently deaf.
    fn allowed_in_windows(&self, tab: &mut Pane) -> bool {
        !matches!(tab, Pane::Canvas)
    }

    /// The canvas pane's body is a HOLE — the wgpu canvas is already in the
    /// frame under egui, so nothing may paint over it.
    fn clear_background(&self, tab: &Pane) -> bool {
        !matches!(tab, Pane::Canvas)
    }

    /// Canvas tabs and palette tabs never share a tab bar (patch #16): a
    /// palette tabbed over the canvas buries the drawing surface. Splitting
    /// beside either class is the layout feature and stays free.
    fn can_tab_into(&self, tab: &Pane, dst_tabs: &[Pane]) -> bool {
        let canvas = matches!(tab, Pane::Canvas);
        dst_tabs
            .iter()
            .all(|t| matches!(t, Pane::Canvas) == canvas)
    }

    /// The bodies decide their own scrolling; a dock-level scroll would
    /// double-scroll fill bodies. (0.21 spells this `scroll_bars`.)
    fn scroll_bars(&self, _tab: &Pane) -> [bool; 2] {
        [false, false]
    }
}

/// The canvas pane's body: the document tab strip across the top, and the
/// rest is the transparent hole the wgpu canvas shows through. The hole rect
/// is what pen routing, zoom anchoring, the canvas overlay and the selection
/// launcher all key off — same contract as the old panel-free rect.
fn canvas_pane_body(ui: &mut egui::Ui, app: &mut App) {
    // The strip paints its own background: the pane body is unpainted
    // (clear_background = false) and the hole below must STAY unpainted.
    let strip = egui::Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(ui.available_width(), super::top::DOC_TAB_H),
    );
    ui.painter()
        .rect_filled(strip, egui::CornerRadius::ZERO, theme::HEADER);
    super::top::doc_tab(ui, app);

    let hole = ui.available_rect_before_wrap();
    app.shell.set_canvas_rect_points(hole);
    super::overlay::canvas_overlay(ui, app, hole);
    super::launcher::selection_launcher(ui, app, hole);
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

/// Render the whole dock tree inside `ui`. The state is swapped out of `App`
/// for the call (the viewer borrows the app for the pane bodies — egui
/// immediate mode, two mutable aliases of the same struct would not fly).
pub fn tree(ui: &mut egui::Ui, app: &mut App) {
    let mut dock = std::mem::replace(&mut app.dock, minimal_tree());
    DockArea::new(&mut dock)
        .id(egui::Id::new("mn.dock"))
        .style(dock_style(ui))
        // CSP's strips have no close-all control — with per-tab ×s (on the
        // active tab, vendor patch #7b) a strip-end ✕ reads as a stray
        // second close button (owner report 2026-08-17).
        .show_leaf_close_all_buttons(false)
        .show_inside(ui, &mut Viewer { app });
    app.dock = dock;
}

/// Is this palette open anywhere (docked or floating)?
pub fn is_open(app: &App, p: Palette) -> bool {
    app.dock
        .iter_all_tabs()
        .any(|(_, t)| *t == Pane::Palette(p))
}

/// Reopen a closed palette. It joins the leaf holding the Layers palette
/// (the palette home CSP's Window menu targets), else the first palette
/// leaf, else a fresh column split off the tree's right edge — never the
/// canvas leaf (patch #16's rule, upheld on this path too).
pub fn reopen(app: &mut App, p: Palette) {
    if is_open(app, p) {
        return;
    }
    let pane = Pane::Palette(p);
    // Main-surface leaves only: a palette reopened "into" a floating window
    // the user parked off-screen would look like it never opened.
    let docked_leaf = |want: &dyn Fn(&[Pane]) -> bool| {
        app.dock.iter_all_nodes().find_map(|(path, node)| {
            if !path.surface.is_main() {
                return None;
            }
            let leaf = node.get_leaf()?;
            want(&leaf.tabs).then_some(path.node)
        })
    };
    let target = docked_leaf(&|tabs: &[Pane]| {
        tabs.contains(&Pane::Palette(Palette::Layers))
    })
    .or_else(|| docked_leaf(&|tabs: &[Pane]| tabs.iter().any(|t| matches!(t, Pane::Palette(_)))));
    match target {
        Some(idx) => {
            if let Ok(leaf) = app.dock.main_surface_mut().leaf_mut(idx) {
                leaf.tabs.push(pane);
                leaf.active = egui_dock::TabIndex(leaf.tabs.len() - 1);
            }
        }
        None => {
            // Every palette is closed: split a fresh column off the right.
            let canvas = docked_leaf(&|tabs: &[Pane]| tabs.contains(&Pane::Canvas))
                .unwrap_or(NodeIndex::root());
            app.dock
                .main_surface_mut()
                .split_right(canvas, 0.82, vec![pane]);
        }
    }
}

/// Remove every occurrence of a palette from the tree (docked or floating).
pub fn close_palette(app: &mut App, p: Palette) {
    loop {
        // (a plain `while let` pins the iterator's temporary borrow through
        // the body — remove_tab needs &mut.)
        let path = app
            .dock
            .iter_all_tabs()
            .find(|(_, t)| **t == Pane::Palette(p))
            .map(|(path, _)| path);
        let Some(path) = path else { break };
        app.dock.remove_tab(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The legacy two-column layout folds into one tree: every palette
    /// survives with its grouping, the canvas pane sits between the
    /// columns, and floating palettes ride across as windows.
    #[test]
    fn merge_columns_carries_every_palette_and_the_floats() {
        let mut left = default_left();
        // Tear one palette off into a floating window, position and all.
        let path = left
            .iter_all_tabs()
            .find(|(_, t)| **t == Palette::Pages)
            .map(|(p, _)| p)
            .expect("Pages in the default left column");
        left.remove_tab(path);
        let si = left.add_window(vec![Palette::Pages]);
        if let Some(ws) = left.get_window_state_mut(si) {
            ws.set_position(egui::pos2(300.0, 200.0));
        }
        let right = default_right();

        let merged = merge_columns(
            &to_json(&left),
            &to_json(&right),
            186.0,
            208.0,
            1280.0,
        )
        .expect("default columns must merge");

        for p in ALL {
            assert!(
                merged
                    .iter_all_tabs()
                    .any(|(_, t)| *t == Pane::Palette(p))
                    == (left
                        .iter_all_tabs()
                        .any(|(_, t)| *t == p)
                        || right.iter_all_tabs().any(|(_, t)| *t == p)),
                "{p:?} must survive the merge exactly when it was open"
            );
        }
        assert_eq!(
            merged
                .iter_all_tabs()
                .filter(|(_, t)| **t == Pane::Canvas)
                .count(),
            1,
            "exactly one canvas pane"
        );
        // The float is a window surface of the merged state now.
        assert!(
            merged
                .iter_all_tabs()
                .any(|(path, t)| *t == Pane::Palette(Palette::Pages)
                    && !path.surface.is_main()),
            "the floating Pages window must ride across as a float"
        );
        // Tab GROUPING survives the graft: ToolProperty and LayerProperty
        // shared a leaf in the default column and still do.
        let prop_leaf = merged
            .iter_all_tabs()
            .find(|(_, t)| **t == Pane::Palette(Palette::ToolProperty))
            .map(|(p, _)| p)
            .expect("ToolProperty docked");
        let leaf = merged[prop_leaf.surface][prop_leaf.node]
            .get_leaf()
            .expect("a leaf");
        assert!(
            leaf.tabs
                .contains(&Pane::Palette(Palette::LayerProperty)),
            "grouped tabs stay grouped through the graft"
        );
    }

    /// The persisted tree round-trips, and every broken form of it degrades
    /// to the default rather than wedging startup or losing the canvas.
    #[test]
    fn dock_tree_roundtrips_and_degrades_safely() {
        // A CUSTOMIZED tree (Pages closed — the DEFAULT tree has it), so the
        // round trip is provably a real parse: the fallback path would
        // resurrect the closed palette.
        let mut tree = default_tree();
        let path = tree
            .iter_all_tabs()
            .find(|(_, t)| **t == Pane::Palette(Palette::Pages))
            .map(|(p, _)| p)
            .expect("Pages is in the default tree");
        tree.remove_tab(path);
        let json = to_json_tree(&tree);
        let back = from_json_tree(&json);
        // The first parse may repair never-laid-out rects (null → 0.0), so
        // byte-stability is asserted from the second round trip on.
        let json2 = to_json_tree(&back);
        assert_eq!(
            to_json_tree(&from_json_tree(&json2)),
            json2,
            "byte-stable after the sanitizing round trip"
        );
        assert!(
            !back
                .iter_all_tabs()
                .any(|(_, t)| *t == Pane::Palette(Palette::Pages)),
            "the customization survived — this was a parse, not the fallback"
        );
        assert_eq!(
            back.iter_all_tabs()
                .filter(|(_, t)| **t == Pane::Canvas)
                .count(),
            1
        );

        // Junk falls back to the default.
        let junk = from_json_tree("{not json");
        assert!(
            junk.iter_all_tabs().any(|(_, t)| *t == Pane::Canvas),
            "fallback tree has a canvas"
        );

        // A VALID tree with no canvas pane (hand-edit) also falls back:
        // the canvas must never be losable through ui.txt.
        let no_canvas: DockState<Pane> =
            DockState::new(vec![Pane::Palette(Palette::Layers)]);
        let restored = from_json_tree(&to_json_tree(&no_canvas));
        assert!(
            restored.iter_all_tabs().any(|(_, t)| *t == Pane::Canvas),
            "a canvasless layout must not survive the load"
        );

        // Two canvas panes (a future build's layout, or a hand-edit)
        // collapse to one instead of confusing the phase-1 invariant.
        let mut two = DockState::new(vec![Pane::Canvas]);
        two.main_surface_mut()
            .split_right(NodeIndex::root(), 0.5, vec![Pane::Canvas]);
        let deduped = from_json_tree(&to_json_tree(&two));
        assert_eq!(
            deduped
                .iter_all_tabs()
                .filter(|(_, t)| **t == Pane::Canvas)
                .count(),
            1
        );
    }

    /// An old build's column JSON (bare palette names as tabs) parses into
    /// panes — the exact bytes a shipped ui.txt carries.
    #[test]
    fn legacy_column_json_parses_into_panes() {
        let col = to_json(&default_left());
        assert!(
            col.contains("\"Tool\""),
            "legacy tabs serialize as bare names: {col}"
        );
        let panes = parse_column_as_panes(&col).expect("wrap + parse");
        assert!(
            panes
                .iter_all_tabs()
                .any(|(_, t)| *t == Pane::Palette(Palette::SubTool)),
            "palettes arrive wrapped"
        );
        assert!(parse_column_as_panes("{").is_none(), "junk is None");
    }
}
