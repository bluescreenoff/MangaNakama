//! The egui layout: top bar + status bar sandwich a dockable workspace —
//! two dock columns (left of the canvas, right of it) whose tabs are the
//! palettes, freely draggable, tabbable, splittable and tearable-off into
//! floating windows (ui/dock.rs — freer than CSP's edge-pinned palettes).
//! The rect the columns leave free is the canvas, and `Shell::owns_pointer`
//! routes on it. Widgets never mutate app state directly — they push
//! [`AppCmd`]s.

mod actions;
mod comps;
mod history;
pub mod icons;
pub mod preview;
/// Public because `App` parks the command palette's gathered rows
/// (`quick::Entry`) between frames.
pub mod quick;
pub mod refs;
pub mod theme;

mod color;
mod dialogs;
pub mod dock;
mod launcher;
mod layers;
mod materials;
mod navigator;
mod overlay;
mod pages;
mod batch;
mod pattern;
mod property;
mod reader;
mod subtool;
mod tools;
mod top;
mod widgets;

/// The Ctrl+K door, for `main::shortcut` (the overlay itself lives in
/// `ui/quick.rs`, next to the index it searches).
pub use quick::open_command_palette;

use color::picker_sync;
use dialogs::{
    adjust_window, canvas_size_window, detail_window, export_all_window, feedback_window,
    filter_window, gen_lines_window, goto_page_window, hud, new_doc_window, prefs_window,
    property_detail_window, spread_window, story_window, text_styles_window, work_settings_window, workspace_window,
};
use dock::column;
use overlay::canvas_overlay;
use top::{doc_tab, status_bar, top_bar};
use widgets::chrome_frame;

use crate::app::App;

pub fn build(ui: &mut egui::Ui, app: &mut App) {
    // One brush preview generated per frame: startup trickles, never hitches.
    app.preview_budget = 1;
    // Same rule for reference images (thumbnails and viewer textures share
    // the budget): a board of forty photos loads over forty frames instead of
    // decoding all of them the first time the palette is shown.
    app.refs.budget = 1;
    picker_sync(app);

    // The reader REPLACES the whole UI while open (owner top item
    // 2026-08-18) — no toolbars, no panels, pages only.
    if app.reader.open {
        reader::reader_overlay(ui, app);
        return;
    }

    // Shift+Tab hides the chrome (UI-032): menu bar, command row and status
    // bar. The palettes are Tab's, separately — one key for "get the tools
    // out of the way", one for "get everything out of the way".
    if !app.chrome_hidden {
        egui::Panel::top("mn.topbar")
            .resizable(false)
            .frame(chrome_frame(egui::Margin::symmetric(6, 3)))
            .show(ui, |ui| top_bar(ui, app));

        egui::Panel::bottom("mn.status")
            .resizable(false)
            .frame(chrome_frame(egui::Margin::symmetric(8, 3)))
            .show(ui, |ui| status_bar(ui, app));
    }

    let mut left_w = app.layout.left_w;
    let mut right_w = app.layout.right_w;

    // Tab hides every palette for a clean drawing view (CSP behaviour).
    if !app.panels_hidden {
        // A COLLAPSED column is a thin strip carrying only the expand
        // chevron; the canvas takes the rest, since it sizes itself from
        // whatever rect the panels leave (`available_rect_before_wrap`).
        let lw = column_width(app.layout.left_w, app.layout.left_collapsed);
        let rw = column_width(app.layout.right_w, app.layout.right_collapsed);
        // The column widths are OURS, not egui's panel-resize machinery: egui's
        // separator highlights the edge white on hover and only flips the
        // cursor inside a razor-thin band (owner report 2026-08-16 — CSP shows
        // the cursor and nothing else). The panels are fixed-size; the handles
        // below each edge do the dragging with a generous grab band.
        let lp = egui::Panel::left("mn.left")
            .exact_size(lw)
            .resizable(false)
            .show_separator_line(false)
            .frame(chrome_frame(egui::Margin::same(2)))
            .show(ui, |ui| palette_column(ui, app, true));

        let rp = egui::Panel::right("mn.side")
            .exact_size(rw)
            .resizable(false)
            .show_separator_line(false)
            .frame(chrome_frame(egui::Margin::same(2)))
            .show(ui, |ui| palette_column(ui, app, false));

        // Document tab strip, spanning only the canvas area (built after the
        // side panels, so it inherits the shrunken rect).
        egui::Panel::top("mn.doctab")
            .resizable(false)
            .frame(chrome_frame(egui::Margin::same(0)))
            .show(ui, |ui| doc_tab(ui, app));

        column_handles(ui, app, lp.response.rect, rp.response.rect);
        // The handles may have written a new width mid-frame; the panel shows
        // it next frame, but `note_widths` must persist the NEW value, not the
        // width this frame rendered at.
        left_w = app.layout.left_w;
        right_w = app.layout.right_w;
    }
    app.layout.note_widths(left_w, right_w);

    // Everything the columns did not take is canvas.
    let canvas = ui.available_rect_before_wrap();
    app.shell.set_canvas_rect_points(canvas);
    canvas_overlay(ui, app, canvas);
    launcher::selection_launcher(ui, app, canvas);

    new_doc_window(ui.ctx(), app);
    work_settings_window(ui.ctx(), app);
    canvas_size_window(ui.ctx(), app);
    prefs_window(ui.ctx(), app);
    pattern::pattern_window(ui.ctx(), app);
    batch::batch_window(ui.ctx(), app);
    adjust_window(ui.ctx(), app);
    goto_page_window(ui.ctx(), app);
    spread_window(ui.ctx(), app);
    export_all_window(ui.ctx(), app);
    story_window(ui.ctx(), app);
    gen_lines_window(ui.ctx(), app);
    filter_window(ui.ctx(), app);
    workspace_window(ui.ctx(), app);
    text_styles_window(ui.ctx(), app);
    detail_window(ui.ctx(), app);
    property_detail_window(ui.ctx(), app);
    feedback_window(ui.ctx(), app);
    // Reference viewers: free floating windows, one per open reference.
    refs::reference_windows(ui.ctx(), app);
    hud(ui.ctx(), app);
    // Last, so the Ctrl+K overlay floats over every palette and dialog.
    quick::command_palette(ui.ctx(), app);

    app.sync_dock_layout();
    app.layout.save_if_dirty();
    app.prefs.save_if_dirty();
}

/// A collapsed palette column: just wide enough for the expand chevron.
const STRIP_W: f32 = 18.0;
/// The slim header strip that carries a column's collapse chevron.
const COLUMN_HEADER_H: f32 = 14.0;

/// The width a palette column is actually laid out at.
fn column_width(stored: f32, collapsed: bool) -> f32 {
    if collapsed { STRIP_W } else { stored }
}

/// One palette column: a slim header with the collapse chevron, then the dock
/// column itself — or, collapsed, the chevron alone in an 18pt strip.
///
/// KNOWN LIMIT: a column's torn-off FLOATING palettes are windows of that
/// column's own `DockState`, and egui_dock only draws them from
/// `DockArea::show_inside`. Collapsing a column therefore hides its floating
/// palettes too, until it is expanded again. Nothing is lost (the dock state
/// is untouched), but it is a surprise worth knowing about.
fn palette_column(ui: &mut egui::Ui, app: &mut App, left: bool) {
    let collapsed = if left {
        app.layout.left_collapsed
    } else {
        app.layout.right_collapsed
    };
    // The chevron points at the screen edge the column folds towards, and
    // back at the canvas once it is folded away.
    let icon = if left != collapsed {
        icons::Icon::ChevronLeft
    } else {
        icons::Icon::ChevronRight
    };
    let tip = if collapsed {
        "Show this palette column"
    } else {
        "Collapse this palette column"
    };
    // Docked against the canvas-side edge, where the user's eye already is.
    let layout = if left {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    };
    let toggled = ui
        .allocate_ui_with_layout(
            egui::vec2(ui.available_width(), COLUMN_HEADER_H),
            layout,
            |ui| widgets::icon_btn(ui, icon, COLUMN_HEADER_H, false, true, tip).clicked(),
        )
        .inner;
    if toggled {
        let (l, r) = (app.layout.left_collapsed, app.layout.right_collapsed);
        if left {
            app.layout.note_collapsed(!l, r);
        } else {
            app.layout.note_collapsed(l, !r);
        }
    }
    if !collapsed {
        column(ui, app, left);
    }
}

/// Grab half-band (points) along the column edges, and the width limits the
/// drag respects. CSP's edge zones are easy to hit with a pen; egui's
/// `resize_grab_radius_side` default (4pt) is not.
const EDGE_BAND: f32 = 7.0;
const LEFT_MIN: f32 = 150.0;
const RIGHT_MIN: f32 = 176.0;
const COL_MAX: f32 = 420.0;

/// The column resize handles: one invisible band on each column's canvas-side
/// edge. Hover/drag = `<->` cursor; a 1px seam line is painted where the
/// column meets the canvas — deliberately NO color change on hover (the old
/// egui separator turned the edge white; the owner called it overboard).
fn column_handles(ui: &mut egui::Ui, app: &mut App, left: egui::Rect, right: egui::Rect) {
    let win_w = right.right() - left.left();
    // Never let the two columns swallow the canvas. A collapsed column costs
    // the strip, not its stored width.
    let canvas_min = 120.0;
    let left_max = (COL_MAX)
        .min(win_w - column_width(app.layout.right_w, app.layout.right_collapsed) - canvas_min)
        .max(LEFT_MIN);
    let right_max = (COL_MAX)
        .min(win_w - column_width(app.layout.left_w, app.layout.left_collapsed) - canvas_min)
        .max(RIGHT_MIN);

    // A collapsed column keeps its seam line but grows NO resize handle: the
    // drag would write the 18pt strip's width into `left_w`/`right_w`, and
    // the column would come back as a strip on the next launch.
    let x = left.right();
    if !app.layout.left_collapsed {
        let band = egui::Rect::from_x_y_ranges(
            egui::Rangef::new(x - EDGE_BAND, x + EDGE_BAND),
            egui::Rangef::new(left.top(), left.bottom()),
        );
        let r = ui.interact(
            band,
            egui::Id::new("mn.resize.left"),
            egui::Sense::click_and_drag(),
        );
        if r.dragged() {
            if let Some(pt) = r.interact_pointer_pos() {
                app.layout.left_w = (pt.x - left.left()).clamp(LEFT_MIN, left_max);
            }
        }
        r.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    }
    ui.painter().vline(
        x,
        egui::Rangef::new(left.top(), left.bottom()),
        egui::Stroke::new(1.0, theme::BORDER),
    );

    let x = right.left();
    if !app.layout.right_collapsed {
        let band = egui::Rect::from_x_y_ranges(
            egui::Rangef::new(x - EDGE_BAND, x + EDGE_BAND),
            egui::Rangef::new(right.top(), right.bottom()),
        );
        let r = ui.interact(
            band,
            egui::Id::new("mn.resize.right"),
            egui::Sense::click_and_drag(),
        );
        if r.dragged() {
            if let Some(pt) = r.interact_pointer_pos() {
                app.layout.right_w = (right.right() - pt.x).clamp(RIGHT_MIN, right_max);
            }
        }
        r.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    }
    ui.painter().vline(
        x,
        egui::Rangef::new(right.top(), right.bottom()),
        egui::Stroke::new(1.0, theme::BORDER),
    );
}
