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

/// One pane of the dock tree: a palette, the canvas, or a page view.
/// Serialized into `ui.txt` (`dock_tree=`) — the variant tags are the
/// persisted API.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Pane {
    Palette(Palette),
    /// THE drawing surface. Exactly one, it follows the active document,
    /// and the document tab strip is drawn inside its body.
    Canvas,
    /// Docking 2 phase 2: a live-updating view of one page of the OPEN
    /// work, arrangeable like any pane — `page 1 | tools | page 2 |
    /// layers | page 3` is this. Clicking it makes that page current on
    /// the Canvas pane (one live page is a load-bearing invariant:
    /// parked pages are bytes, and the target machine has an iGPU — see
    /// docs/DOCKING-2.md). Index-bound: after a reorder or in a different
    /// work it simply shows whatever page holds that index now, and an
    /// index past the work's end says so instead of vanishing.
    PageView {
        page: usize,
    },
    /// CV-021 (CSP's Window ▸ Canvas ▸ **New Window**): a SECOND live view
    /// of the page being drawn, with its own zoom and pan — ink zoomed in
    /// on the Canvas pane, watch the whole page here, both moving together.
    /// Exactly one (the runtime state — viewport and texture — lives on
    /// `App`, and two panes would thrash one cache); extras collapse on
    /// load, like the canvas pane's own dedupe.
    ///
    /// View-only by design, not by shortfall: `Shell::owns_pointer` routes
    /// the pen by ONE canvas rect and two live GPU viewports are ruled out
    /// in docs/DOCKING-2.md, so this composites offscreen through its own
    /// viewport instead (`App::view_pane_texture`).
    CanvasView,
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
    fn title(self) -> String {
        match self {
            Pane::Palette(p) => p.title().to_string(),
            Pane::Canvas => "Canvas".to_string(),
            Pane::PageView { page } => format!("p.{}", page + 1),
            Pane::CanvasView => "View 2".to_string(),
        }
    }

    /// Canvas-class panes share tab bars with each other and never with
    /// palettes (patch #16's rule).
    fn is_canvas_class(self) -> bool {
        matches!(
            self,
            Pane::Canvas | Pane::PageView { .. } | Pane::CanvasView
        )
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
    //
    // Fractions approximate each fixed palette's CONTENT height over the
    // ~780pt dock column of the shipped 1280×860 window — CSP palettes hug
    // their content and the old seeds (0.34/0.62/0.6) left the Tool leaf
    // two-thirds empty (parity audit T1). The slack goes to the palettes
    // `fills()` names (Sub Tool, Pages). Drag a separator and the tree
    // remembers; these only seed.
    let tree = dock.main_surface_mut();
    let [_, sub] = tree.split_below(NodeIndex::root(), 0.22, vec![Palette::SubTool]);
    let [_, prop] = tree.split_below(
        sub,
        0.38,
        vec![Palette::ToolProperty, Palette::LayerProperty],
    );
    tree.split_below(prop, 0.55, vec![Palette::Pages]);
    dock
}

/// The default right column: (Color | Color Set) above (Navigator |
/// Materials) above (Layers | Auto Actions) — the actions tab sits beside
/// Layers like CSP's, and Navigator + Materials ship visible (parity audit
/// T3: they existed only in the enum, and there is no Window-style menu, so
/// a new user never learned they exist).
pub fn default_right() -> DockColumn {
    let mut dock = DockState::new(vec![Palette::Color, Palette::ColorSet]);
    let tree = dock.main_surface_mut();
    let [_, nav] = tree.split_below(
        NodeIndex::root(),
        0.36,
        vec![Palette::Navigator, Palette::Materials],
    );
    tree.split_below(nav, 0.3, vec![Palette::Layers, Palette::Actions]);
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
    if canvases == 0 {
        return default_tree();
    }
    // Phase 1 owns exactly one canvas pane; extras (hand-edits, or a future
    // build's layout) collapse to the first. CV-021's second view is
    // singular for a different reason — its viewport and texture live on
    // `App` — but the repair is the same one.
    dedupe(&mut tree, Pane::Canvas);
    dedupe(&mut tree, Pane::CanvasView);
    tree
}

/// Remove every occurrence of `pane` after the first (tree order).
fn dedupe(tree: &mut DockTree, pane: Pane) {
    loop {
        let mut seen = false;
        let extra = tree.iter_all_tabs().find_map(|(p, t)| {
            if *t == pane {
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
    //
    // FRACTION TRAP: `Tree::split`'s doc says the fraction is "how much the
    // OLD node occupies", but the layout gives it to the LEFT child by
    // POSITION (`midpoint = min + width * fraction`) — so for split_LEFT
    // the fraction sizes the NEW node. Passing `1.0 - lfrac` here handed
    // the palette column 85% of the window and crushed the canvas into the
    // remainder (owner's first docking-2 launch).
    let mut canvas_node = NodeIndex::root();
    if has_tabs(&left) {
        let tree = merged.main_surface_mut();
        let [canvas, slot] =
            tree.split_left(canvas_node, lfrac, vec![Pane::Palette(Palette::Tool)]);
        tree.graft_at(slot, left.main_surface());
        canvas_node = canvas;
    }
    if has_tabs(&right) {
        // The canvas node's area is what is left of the window after the
        // left column; the right fraction is relative to THAT.
        let rel = (rfrac / (1.0 - lfrac)).clamp(0.08, 0.5);
        let tree = merged.main_surface_mut();
        let [_, slot] =
            tree.split_right(canvas_node, 1.0 - rel, vec![Pane::Palette(Palette::Layers)]);
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
        // Display only — `Pane::title` stays the stable identity (the tab
        // `id` above, the serialized `dock_tree` tag, the Window menu). CSP
        // names this palette after the tool whose sub tools it is showing,
        // so "Sub Tool" becomes "Figure Tools" / "Pen Tools" / ... A tool
        // with no sub tool list would keep the generic name.
        if let Pane::Palette(Palette::SubTool) = tab {
            return format!("{} Tools", self.app.tool.label()).into();
        }
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Pane) {
        match tab {
            Pane::Palette(p) => p.body(ui, self.app),
            Pane::Canvas => canvas_pane_body(ui, self.app),
            Pane::PageView { page } => page_view_body(ui, self.app, *page),
            Pane::CanvasView => canvas_view_body(ui, self.app),
        }
    }

    fn closeable(&mut self, tab: &mut Pane) -> bool {
        // The app always shows a canvas; THE canvas pane cannot close.
        // Page views are just views — they close freely.
        !matches!(tab, Pane::Canvas)
    }

    /// The canvas never floats: a floating egui window is a non-background
    /// layer, so `Shell::owns_pointer` would hand every pen event inside it
    /// to egui and the canvas would go permanently deaf. A page VIEW is an
    /// ordinary egui widget (preview + click), so it may float.
    fn allowed_in_windows(&self, tab: &mut Pane) -> bool {
        !matches!(tab, Pane::Canvas)
    }

    /// The canvas pane's body is a HOLE — the wgpu canvas is already in the
    /// frame under egui, so nothing may paint over it.
    fn clear_background(&self, tab: &Pane) -> bool {
        !matches!(tab, Pane::Canvas)
    }

    /// Canvas-class tabs (canvas + page views) and palette tabs never share
    /// a tab bar (patch #16): a palette tabbed over the canvas buries the
    /// drawing surface. Splitting beside either class is the layout feature
    /// and stays free.
    fn can_tab_into(&self, tab: &Pane, dst_tabs: &[Pane]) -> bool {
        let canvas = tab.is_canvas_class();
        dst_tabs.iter().all(|t| t.is_canvas_class() == canvas)
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
        .rect_filled(strip, egui::CornerRadius::ZERO, theme::c().header);
    super::top::doc_tab(ui, app);

    let hole = ui.available_rect_before_wrap();
    app.shell.set_canvas_rect_points(hole);
    super::overlay::canvas_overlay(ui, app, hole);
    super::launcher::selection_launcher(ui, app, hole);
}

/// A page-view pane: the page fitted to the pane, live-updating (parked
/// pages re-render from their sharp preview when their revision moves; the
/// CURRENT page is already live on the Canvas pane and shows its live
/// thumbnail here). One click makes the page current on the Canvas pane —
/// through `AppCmd::SelectPage`, i.e. the ordinary page-switch door with
/// its stash/decode and cache resets; nothing here installs state by hand.
fn page_view_body(ui: &mut egui::Ui, app: &mut App, page: usize) {
    let avail = ui.available_rect_before_wrap();
    let caption_h = 16.0;
    if page >= app.pages.len() {
        // A layout can outlive the work that shaped it (restart, another
        // work, deleted pages): say so instead of vanishing the pane.
        ui.painter().text(
            avail.center(),
            egui::Align2::CENTER_CENTER,
            format!("No page {} in this work", page + 1),
            egui::FontId::proportional(12.0),
            theme::c().text_weak,
        );
        return;
    }

    let current = page == app.page_index;
    let img_rect = egui::Rect::from_min_max(
        avail.min,
        egui::pos2(avail.max.x, (avail.max.y - caption_h).max(avail.min.y)),
    );

    // The texture: the current page's live thumb (already minted every
    // revision for the Pages palette), else this pane's OWN display-size
    // texture from the sharp preview — separate from the palette's
    // `prev_tex`, whose size follows the palette cell (`pane_tex` doc).
    let aspect = {
        let (w, h) = app.pages[page]
            .canvas
            .or_else(|| current.then_some((app.doc.size.0, app.doc.size.1)))
            .unwrap_or((1, 1));
        h.max(1) as f32 / w.max(1) as f32
    };
    let fit_w = (img_rect.width().min(img_rect.height() / aspect)).max(1.0);
    let fit = egui::Rect::from_center_size(img_rect.center(), egui::vec2(fit_w, fit_w * aspect));

    if !current {
        let e = &app.pages[page];
        let stale = e.pane_tex.is_none()
            || e.pane_tex_rev != e.rev
            || (fit.width() - e.pane_tex_px).abs() > e.pane_tex_px * 0.25;
        if stale
            && app.page_pane_budget > 0
            && let Some(gray) = app.preview_for(page)
        {
            app.page_pane_budget -= 1;
            let tex = super::preview::mint_gray_tex(
                ui.ctx(),
                &gray,
                fit.width().round() as u32,
                fit.height().round() as u32,
                format!("mn.page.pane.{page}"),
            );
            let e = &mut app.pages[page];
            e.pane_tex = Some(tex);
            e.pane_tex_px = fit.width();
            e.pane_tex_rev = e.rev;
        }
    }
    let e = &app.pages[page];
    let tex = if current {
        e.thumb.as_ref()
    } else {
        e.pane_tex
            .as_ref()
            .or(e.prev_tex.as_ref())
            .or(e.thumb.as_ref())
    };
    match tex {
        Some(t) => {
            ui.painter().image(
                t.id(),
                fit,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            ui.painter().rect_filled(fit, 2.0, egui::Color32::WHITE);
        }
    }
    ui.painter().rect_stroke(
        fit,
        0.0,
        egui::Stroke::new(1.0, theme::c().border),
        egui::StrokeKind::Outside,
    );

    let caption = if current {
        format!("page {} — editing on the canvas", page + 1)
    } else {
        format!("page {}", page + 1)
    };
    ui.painter().text(
        egui::pos2(avail.center().x, avail.max.y - caption_h * 0.5),
        egui::Align2::CENTER_CENTER,
        caption,
        egui::FontId::proportional(11.0),
        if current {
            theme::c().text
        } else {
            theme::c().text_weak
        },
    );

    if !current {
        let resp = ui.interact(
            avail,
            ui.id().with(("mn.page.pane.act", page)),
            egui::Sense::click(),
        );
        if resp.hovered() {
            ui.painter().rect_stroke(
                fit,
                0.0,
                egui::Stroke::new(1.5, theme::c().accent),
                egui::StrokeKind::Outside,
            );
        }
        let resp = resp.on_hover_text("Click to edit this page on the canvas");
        if resp.clicked() {
            app.push_cmd(crate::cmd::AppCmd::SelectPage(page));
        }
    }
}

/// CV-021's second live view (`Pane::CanvasView`): the page being drawn,
/// composited offscreen through the pane's OWN viewport, so one view can sit
/// at 400% on an eye while this one holds the whole page — and both move
/// together, because both read the live document.
///
/// It re-renders when the document revision moves (stroke end, layer edit,
/// undo), when the pane is resized, and when its own zoom or pan changes.
/// Drag to pan, wheel to zoom, and the strip's Fit puts the whole page back;
/// clicking never draws (the Canvas pane is the one drawing surface — see
/// `Pane::CanvasView`).
fn canvas_view_body(ui: &mut egui::Ui, app: &mut App) {
    let full = ui.available_rect_before_wrap();
    const STRIP_H: f32 = 20.0;
    let img_rect = egui::Rect::from_min_max(
        full.min,
        egui::pos2(full.max.x, (full.max.y - STRIP_H).max(full.min.y)),
    );
    if img_rect.width() < 16.0 || img_rect.height() < 16.0 {
        return;
    }

    // Target pixels per point: the pane renders at display resolution, up to
    // the long-edge cap (a pane dragged to fill the window must not mint a
    // page-sized texture on every stroke).
    let ppp = ui.ctx().pixels_per_point();
    let long_pt = img_rect.width().max(img_rect.height());
    let scale = ppp * (App::VIEW_PANE_MAX_PX / (long_pt * ppp)).min(1.0);
    let size_px = (
        (img_rect.width() * scale).round().max(1.0) as u32,
        (img_rect.height() * scale).round().max(1.0) as u32,
    );

    let resp = ui.interact(
        img_rect,
        ui.id().with("mn.viewpane"),
        egui::Sense::click_and_drag(),
    );
    // Input BEFORE the render, so a drag shows this frame instead of next.
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.1
            && let Some(p) = ui.input(|i| i.pointer.hover_pos())
        {
            let anchor = [
                (p.x - img_rect.left()) * scale,
                (p.y - img_rect.top()) * scale,
            ];
            app.view_pane_zoom(size_px, anchor, (scroll * 0.0015).exp());
        }
    }
    if resp.dragged() {
        let d = resp.drag_delta();
        app.view_pane_pan(size_px, d.x * scale, d.y * scale);
    }
    if resp.double_clicked() {
        app.view_pane_fit();
    }

    let tex = app.view_pane_texture(size_px);
    ui.painter().image(
        tex.id(),
        img_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );

    // The strip: the Navigator's vocabulary, aimed at THIS view instead of
    // the canvas — nothing here touches `app.viewport`.
    let zoom = app.view_pane_viewport(size_px).zoom;
    let strip = egui::Rect::from_min_max(egui::pos2(full.min.x, img_rect.max.y), full.max);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(strip)
            .id_salt("mn.viewpane.strip"),
        |ui| {
            ui.horizontal(|ui| {
                if ui
                    .small_button("−")
                    .on_hover_text("zoom this view out")
                    .clicked()
                {
                    app.view_pane_zoom(size_px, centre_of(size_px), 1.0 / 1.25);
                }
                if ui
                    .small_button("＋")
                    .on_hover_text("zoom this view in")
                    .clicked()
                {
                    app.view_pane_zoom(size_px, centre_of(size_px), 1.25);
                }
                if ui
                    .small_button("Fit")
                    .on_hover_text("the whole page again — and it stays fitted as the pane resizes")
                    .clicked()
                {
                    app.view_pane_fit();
                }
                ui.weak(format!("{:.0}%", zoom * 100.0));
                if app.view_pane_vp.is_none() {
                    ui.weak("· whole page");
                }
            });
        },
    );
}

/// Centre of a target-pixel rect — the anchor the strip's zoom buttons use
/// (the wheel anchors on the pointer instead).
fn centre_of(size_px: (u32, u32)) -> [f32; 2] {
    [size_px.0 as f32 * 0.5, size_px.1 as f32 * 0.5]
}

/// Is the second live view open anywhere (docked or floating)?
pub fn canvas_view_open(app: &App) -> bool {
    app.dock
        .iter_all_tabs()
        .any(|(_, t)| *t == Pane::CanvasView)
}

/// Open (or focus) the second live view (CV-021). Like `open_page_pane` it
/// splits off the canvas leaf's right side — beside where the eye already is
/// — and a second call focuses the pane that exists instead of adding one.
pub fn open_canvas_view(app: &mut App) {
    let existing = app
        .dock
        .iter_all_tabs()
        .find(|(_, t)| **t == Pane::CanvasView)
        .map(|(path, _)| path);
    if let Some(path) = existing {
        let _ = app.dock.set_active_tab(path);
        app.dock.set_focused_node_and_surface(path.node_path());
        return;
    }
    let canvas = canvas_leaf(&app.dock).unwrap_or(NodeIndex::root());
    app.dock
        .main_surface_mut()
        .split_right(canvas, 0.6, vec![Pane::CanvasView]);
}

/// The main-surface leaf holding THE canvas pane.
fn canvas_leaf(dock: &DockTree) -> Option<NodeIndex> {
    dock.iter_all_nodes().find_map(|(path, node)| {
        (path.surface.is_main()
            && node
                .get_leaf()
                .is_some_and(|l| l.tabs.contains(&Pane::Canvas)))
        .then_some(path.node)
    })
}

/// Open (or focus) a page-view pane for `page`. A new pane splits off the
/// canvas leaf's right side — beside where the eye already is; from there
/// the user drags it wherever the layout wants it.
pub fn open_page_pane(app: &mut App, page: usize) {
    let pane = Pane::PageView { page };
    let existing = app
        .dock
        .iter_all_tabs()
        .find(|(_, t)| **t == pane)
        .map(|(path, _)| path);
    if let Some(path) = existing {
        let _ = app.dock.set_active_tab(path);
        app.dock.set_focused_node_and_surface(path.node_path());
        return;
    }
    let canvas = canvas_leaf(&app.dock).unwrap_or(NodeIndex::root());
    app.dock
        .main_surface_mut()
        .split_right(canvas, 0.55, vec![pane]);
}

/// Theme the dock chrome to the app's tokens. NOTE: egui_dock's own
/// `Style::default()` is a LIGHT style — white tab bodies, white tabs, black
/// outlines (owner bug report "some parts are white now", 2026-08-16). Always
/// derive from `Style::from_egui` (dark-aware) first, then apply our tokens:
/// the tab strip is the old palette title strip (HEADER), the body is the old
/// palette body (PANEL), and the active tab merges into the body.
fn dock_style(ui: &egui::Ui) -> Style {
    let mut s = Style::from_egui(ui.style());

    s.tab_bar.bg_fill = theme::c().header;
    s.tab_bar.hline_color = theme::c().border;
    s.tab_bar.height = 20.0;
    // CSP tab strips: tabs divide the bar evenly and long titles ellipsize
    // (the vendored truncation patch) instead of overflowing the × buttons.
    s.tab_bar.fill_tab_bar = true;

    let tab_state = |bg: egui::Color32, text: egui::Color32| egui_dock::TabInteractionStyle {
        bg_fill: bg,
        text_color: text,
        outline_color: theme::c().border,
        corner_radius: egui::CornerRadius::same(0),
    };
    s.tab.active = tab_state(theme::c().panel, theme::c().text_strong);
    s.tab.inactive = tab_state(theme::c().header, theme::c().text_weak);
    s.tab.hovered = tab_state(theme::c().hover, theme::c().text);
    s.tab.focused = tab_state(theme::c().hover, theme::c().text_strong);
    s.tab.active_with_kb_focus = s.tab.active.clone();
    s.tab.inactive_with_kb_focus = s.tab.inactive.clone();
    s.tab.focused_with_kb_focus = s.tab.focused.clone();
    s.tab.spacing = 0.0;
    s.tab.hline_below_active_tab_name = false;

    s.tab.tab_body.bg_fill = theme::c().panel;
    s.tab.tab_body.stroke = egui::Stroke::NONE;
    s.tab.tab_body.corner_radius = egui::CornerRadius::same(0);

    s.separator.color_idle = theme::c().border;
    s.separator.color_hovered = theme::c().accent;
    s.separator.color_dragged = theme::c().accent;
    s.separator.width = 1.0;
    // Easy to hit with a pen (upstream's 2.0 total ≈ 1pt per side — the owner
    // reported the resize cursor only appearing on an exact hit).
    s.separator.extra_interact_width = 12.0;

    // Tab × / floating-window buttons: transparent until hovered, our greys.
    let b = &mut s.buttons;
    b.close_tab_bg_fill = theme::c().hover;
    b.close_tab_color = theme::c().text_weak;
    b.close_tab_active_color = theme::c().text_strong;
    b.add_tab_bg_fill = theme::c().hover;
    b.add_tab_color = theme::c().text_weak;
    b.add_tab_active_color = theme::c().text_strong;
    b.add_tab_border_color = theme::c().border;
    b.close_all_tabs_bg_fill = theme::c().hover;
    b.close_all_tabs_color = theme::c().text_weak;
    b.close_all_tabs_active_color = theme::c().text_strong;
    b.close_all_tabs_border_color = theme::c().border;
    b.collapse_tabs_bg_fill = theme::c().hover;
    b.collapse_tabs_color = theme::c().text_weak;
    b.collapse_tabs_active_color = theme::c().text_strong;
    b.collapse_tabs_border_color = theme::c().border;
    b.minimize_window_bg_fill = theme::c().hover;
    b.minimize_window_color = theme::c().text_weak;
    b.minimize_window_active_color = theme::c().text_strong;
    b.minimize_window_border_color = theme::c().border;
    b.show_tab_bar_color = theme::c().text_weak;
    b.show_tab_bar_active_color = theme::c().text_strong;

    s.main_surface_border_stroke = egui::Stroke::NONE;
    s.dock_area_padding = Some(egui::Margin::same(1));
    s
}

/// Render the whole dock tree inside `ui`. The state is swapped out of `App`
/// for the call (the viewer borrows the app for the pane bodies — egui
/// immediate mode, two mutable aliases of the same struct would not fly).
pub fn tree(ui: &mut egui::Ui, app: &mut App) {
    let mut dock = std::mem::replace(&mut app.dock, minimal_tree());
    // The canvas leaf never collapses: the leaf-collapse chevron folding
    // the drawing surface away is a burial by another door. Un-collapsing
    // BEFORE the pass keeps the chevron from ever taking effect on it;
    // palette leaves keep theirs.
    for (path, node) in dock.iter_all_nodes_mut() {
        if !path.surface.is_main() {
            continue;
        }
        if let Some(leaf) = node.get_leaf_mut()
            && leaf.tabs.iter().any(|t| t.is_canvas_class())
        {
            leaf.collapsed = false;
        }
    }
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

/// Reopen a closed palette and FOCUS it — the Window-menu path, where the
/// user just asked for that palette by name.
pub fn reopen(app: &mut App, p: Palette) {
    reopen_in(&mut app.dock, p, true);
}

/// Reopen a closed palette WITHOUT taking its leaf's active tab. For the
/// automatic paths: `App::sync_pages_palette` reopens Pages whenever the
/// document turns out to be a manga, and with focus that landed the Pages
/// tab on top of Layers — a fresh launch showed page thumbnails where CSP
/// shows the Layer palette (parity P0-3; `research/ui-shots/ours/layers.png`
/// is three failed attempts to screenshot Layers because of this).
pub fn reopen_unfocused(app: &mut App, p: Palette) {
    reopen_in(&mut app.dock, p, false);
}

/// Reopen a closed palette. It joins the leaf holding the Layers palette
/// (the palette home CSP's Window menu targets), else the first palette
/// leaf, else a fresh column split off the tree's right edge — never the
/// canvas leaf (patch #16's rule, upheld on this path too). Takes the tree
/// rather than the `App` so the activation rule is testable without a GPU.
fn reopen_in(dock: &mut DockTree, p: Palette, focus: bool) {
    let pane = Pane::Palette(p);
    if dock.iter_all_tabs().any(|(_, t)| *t == pane) {
        return;
    }
    // Main-surface leaves only: a palette reopened "into" a floating window
    // the user parked off-screen would look like it never opened.
    let docked_leaf = |dock: &DockTree, want: &dyn Fn(&[Pane]) -> bool| {
        dock.iter_all_nodes().find_map(|(path, node)| {
            if !path.surface.is_main() {
                return None;
            }
            let leaf = node.get_leaf()?;
            want(&leaf.tabs).then_some(path.node)
        })
    };
    let target = docked_leaf(dock, &|tabs: &[Pane]| {
        tabs.contains(&Pane::Palette(Palette::Layers))
    })
    .or_else(|| {
        docked_leaf(dock, &|tabs: &[Pane]| {
            tabs.iter().any(|t| matches!(t, Pane::Palette(_)))
        })
    });
    match target {
        Some(idx) => {
            if let Ok(leaf) = dock.main_surface_mut().leaf_mut(idx) {
                leaf.tabs.push(pane);
                if focus {
                    leaf.active = egui_dock::TabIndex(leaf.tabs.len() - 1);
                }
            }
        }
        None => {
            // Every palette is closed: split a fresh column off the right.
            let canvas = docked_leaf(dock, &|tabs: &[Pane]| tabs.contains(&Pane::Canvas))
                .unwrap_or(NodeIndex::root());
            dock.main_surface_mut()
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

        let merged = merge_columns(&to_json(&left), &to_json(&right), 186.0, 208.0, 1280.0)
            .expect("default columns must merge");

        for p in ALL {
            assert!(
                merged.iter_all_tabs().any(|(_, t)| *t == Pane::Palette(p))
                    == (left.iter_all_tabs().any(|(_, t)| *t == p)
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
        // The FRACTION TRAP pin (merge_columns comment): the root split's
        // fraction sizes the LEFT child by position, so the palette column
        // must get ITS width share — inverted, it swallowed 85% of the
        // window and crushed the canvas (owner's first docking-2 launch).
        let v: serde_json::Value = serde_json::from_str(&to_json_tree(&merged)).expect("tree json");
        let root_frac = v["surfaces"][0]["Main"]["nodes"][0]["Horizontal"]["fraction"]
            .as_f64()
            .expect("root split fraction");
        assert!(
            (root_frac - 186.0 / 1280.0).abs() < 0.02,
            "left column keeps its width share of the window, got {root_frac}"
        );
        // The float is a window surface of the merged state now.
        assert!(
            merged
                .iter_all_tabs()
                .any(|(path, t)| *t == Pane::Palette(Palette::Pages) && !path.surface.is_main()),
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
            leaf.tabs.contains(&Pane::Palette(Palette::LayerProperty)),
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
        let no_canvas: DockState<Pane> = DockState::new(vec![Pane::Palette(Palette::Layers)]);
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

    /// Phase 2: page-view panes ride the tree, round-trip serde, class
    /// with the canvas (never with palettes), and `open_page_pane` focuses
    /// an existing pane instead of stacking duplicates.
    #[test]
    fn page_view_panes_serde_class_and_dedupe() {
        let mut tree = default_tree();
        let canvas = tree
            .iter_all_nodes()
            .find_map(|(path, node)| {
                (path.surface.is_main()
                    && node
                        .get_leaf()
                        .is_some_and(|l| l.tabs.contains(&Pane::Canvas)))
                .then_some(path.node)
            })
            .expect("canvas leaf");
        tree.main_surface_mut()
            .split_right(canvas, 0.5, vec![Pane::PageView { page: 2 }]);

        let back = from_json_tree(&to_json_tree(&tree));
        assert!(
            back.iter_all_tabs()
                .any(|(_, t)| *t == Pane::PageView { page: 2 }),
            "a page view survives the ui.txt round trip"
        );
        assert_eq!(
            back.iter_all_tabs()
                .filter(|(_, t)| **t == Pane::Canvas)
                .count(),
            1,
            "the page view is NOT deduped away as a second canvas"
        );

        // Class rules (what `Viewer::can_tab_into` keys on): page views
        // tab with the canvas, never with palettes.
        let pv = Pane::PageView { page: 0 };
        assert!(pv.is_canvas_class() && Pane::Canvas.is_canvas_class());
        assert!(!Pane::Palette(Palette::Layers).is_canvas_class());
    }

    /// `open_page_pane` focuses an existing pane for the page instead of
    /// stacking duplicates, and distinct pages get distinct panes.
    #[test]
    fn open_page_pane_focuses_instead_of_duplicating() {
        let Some(renderer) = crate::app::headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (800, 600), 1.0);
        app.dock = default_tree();
        let count = |app: &App, page: usize| {
            app.dock
                .iter_all_tabs()
                .filter(|(_, t)| **t == Pane::PageView { page })
                .count()
        };
        open_page_pane(&mut app, 1);
        assert_eq!(count(&app, 1), 1, "the pane opened");
        open_page_pane(&mut app, 1);
        assert_eq!(count(&app, 1), 1, "reopening focuses, never duplicates");
        open_page_pane(&mut app, 2);
        assert_eq!(count(&app, 2), 1, "another page gets its own pane");
        assert_eq!(
            app.dock
                .iter_all_tabs()
                .filter(|(_, t)| **t == Pane::Canvas)
                .count(),
            1,
            "the canvas pane is untouched by page panes"
        );
    }

    /// CV-021: the second live view rides the tree exactly like a page
    /// view — it round-trips `ui.txt`, classes with the canvas so a
    /// palette can never be tabbed over it (and it can never be tabbed
    /// over the drawing surface), and it is SINGULAR: its viewport and
    /// texture live on `App`, so a hand-edited or future layout carrying
    /// two collapses to one on load, the canvas pane's own repair.
    ///
    /// This deliberately extends the pinned layout round trip rather than
    /// loosening it: the pin still says "one canvas, byte-stable JSON",
    /// with the new pane inside the tree while it says so.
    #[test]
    fn the_second_view_pane_serdes_classes_and_is_singular() {
        let mut tree = default_tree();
        let canvas = tree
            .iter_all_tabs()
            .find(|(_, t)| **t == Pane::Canvas)
            .map(|(p, _)| p)
            .expect("canvas leaf");
        tree.main_surface_mut()
            .split_right(canvas.node, 0.6, vec![Pane::CanvasView]);

        let back = from_json_tree(&to_json_tree(&tree));
        assert!(
            back.iter_all_tabs().any(|(_, t)| *t == Pane::CanvasView),
            "the second view survives the ui.txt round trip"
        );
        assert_eq!(
            back.iter_all_tabs()
                .filter(|(_, t)| **t == Pane::Canvas)
                .count(),
            1,
            "and is NOT deduped away as a second canvas pane"
        );
        // The pin: still byte-stable with the new pane in the tree.
        let json = to_json_tree(&back);
        assert_eq!(to_json_tree(&from_json_tree(&json)), json);

        // Class: with the canvas, never with palettes (patch #16's rule,
        // which `Viewer::can_tab_into` keys on).
        assert!(Pane::CanvasView.is_canvas_class());

        // Two collapse to one.
        let mut two = tree;
        let canvas = two
            .iter_all_tabs()
            .find(|(_, t)| **t == Pane::Canvas)
            .map(|(p, _)| p)
            .expect("canvas leaf");
        two.main_surface_mut()
            .split_left(canvas.node, 0.3, vec![Pane::CanvasView]);
        assert_eq!(
            two.iter_all_tabs()
                .filter(|(_, t)| **t == Pane::CanvasView)
                .count(),
            2,
            "two were really built"
        );
        let deduped = from_json_tree(&to_json_tree(&two));
        assert_eq!(
            deduped
                .iter_all_tabs()
                .filter(|(_, t)| **t == Pane::CanvasView)
                .count(),
            1,
            "a layout with two second views loads as one"
        );
        assert_eq!(
            deduped
                .iter_all_tabs()
                .filter(|(_, t)| **t == Pane::Canvas)
                .count(),
            1,
            "and the canvas is still there"
        );
    }

    /// `open_canvas_view` is the Workspace-menu door: it opens ONE pane and
    /// then focuses that one, never stacking a second (the state it steers
    /// is per-App, so a duplicate would thrash one texture cache).
    #[test]
    fn open_canvas_view_focuses_instead_of_duplicating() {
        let Some(renderer) = crate::app::headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (800, 600), 1.0);
        app.dock = default_tree();
        assert!(!canvas_view_open(&app), "closed by default");

        open_canvas_view(&mut app);
        assert!(canvas_view_open(&app));
        let count = |app: &App| {
            app.dock
                .iter_all_tabs()
                .filter(|(_, t)| **t == Pane::CanvasView)
                .count()
        };
        assert_eq!(count(&app), 1, "the pane opened");
        open_canvas_view(&mut app);
        assert_eq!(count(&app), 1, "asking again focuses, never duplicates");
        assert_eq!(
            app.dock
                .iter_all_tabs()
                .filter(|(_, t)| **t == Pane::Canvas)
                .count(),
            1,
            "the drawing surface is untouched"
        );

        // Closing it is ordinary tab removal — nothing refuses, unlike the
        // canvas pane.
        let path = app
            .dock
            .iter_all_tabs()
            .find(|(_, t)| **t == Pane::CanvasView)
            .map(|(p, _)| p)
            .expect("open");
        app.dock.remove_tab(path);
        assert!(!canvas_view_open(&app), "and it closes freely");
    }

    /// Parity P0-3: a fresh launch must show the LAYER palette. Two things
    /// have to hold — the default tree's Layers leaf comes up with Layers
    /// active, and the automatic Pages reopen (`sync_pages_palette`, which
    /// fires on the first manga document) joins that leaf WITHOUT taking
    /// the tab. Before the fix the second half failed: Pages was pushed and
    /// activated, so page thumbnails covered Layers on every manga launch.
    #[test]
    fn fresh_launch_leaves_layers_the_active_tab() {
        let active_of = |tree: &DockTree, p: Palette| -> Pane {
            let path = tree
                .iter_all_tabs()
                .find(|(_, t)| **t == Pane::Palette(p))
                .map(|(path, _)| path)
                .unwrap_or_else(|| panic!("{p:?} is in the default tree"));
            let leaf = tree[path.surface][path.node]
                .get_leaf()
                .expect("palettes live in leaves");
            leaf.tabs[leaf.active.0]
        };

        let mut tree = default_tree();
        assert_eq!(
            active_of(&tree, Palette::Layers),
            Pane::Palette(Palette::Layers),
            "the default tree comes up with Layers focused in its leaf"
        );

        // A plain image closes Pages; opening a manga reopens it — into the
        // Layers leaf, silently.
        let path = tree
            .iter_all_tabs()
            .find(|(_, t)| **t == Pane::Palette(Palette::Pages))
            .map(|(p, _)| p)
            .expect("Pages ships in the default tree");
        tree.remove_tab(path);
        reopen_in(&mut tree, Palette::Pages, false);
        assert!(
            tree.iter_all_tabs()
                .any(|(_, t)| *t == Pane::Palette(Palette::Pages)),
            "Pages did reopen — it is available as a tab"
        );
        assert_eq!(
            active_of(&tree, Palette::Layers),
            Pane::Palette(Palette::Layers),
            "reopening Pages must not steal the Layers leaf's active tab"
        );

        // The Window-menu path is unchanged: asking for a palette focuses it.
        let path = tree
            .iter_all_tabs()
            .find(|(_, t)| **t == Pane::Palette(Palette::Pages))
            .map(|(p, _)| p)
            .expect("Pages open");
        tree.remove_tab(path);
        reopen_in(&mut tree, Palette::Pages, true);
        assert_eq!(
            active_of(&tree, Palette::Pages),
            Pane::Palette(Palette::Pages),
            "an explicitly reopened palette still comes up focused"
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
