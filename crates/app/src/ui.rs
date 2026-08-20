//! The egui layout: top bar + status bar sandwich a dockable workspace —
//! two dock columns (left of the canvas, right of it) whose tabs are the
//! palettes, freely draggable, tabbable, splittable and tearable-off into
//! floating windows (ui/dock.rs — freer than CSP's edge-pinned palettes).
//! The rect the columns leave free is the canvas, and `Shell::owns_pointer`
//! routes on it. Widgets never mutate app state directly — they push
//! [`AppCmd`]s.

mod comps;
mod history;
pub mod icons;
pub mod preview;
mod quick;
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
mod pattern;
mod property;
mod reader;
mod subtool;
mod tools;
mod top;
mod widgets;

use color::picker_sync;
use dialogs::{
    adjust_window, canvas_size_window, detail_window, export_all_window, feedback_window,
    filter_window, gen_lines_window, goto_page_window, hud, new_doc_window, prefs_window,
    property_detail_window, spread_window, story_window, work_settings_window, workspace_window,
};
use dock::column;
use overlay::canvas_overlay;
use top::{doc_tab, status_bar, top_bar};
use widgets::chrome_frame;

use crate::app::App;

pub fn build(ui: &mut egui::Ui, app: &mut App) {
    // One brush preview generated per frame: startup trickles, never hitches.
    app.preview_budget = 1;
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
        // The column widths are OURS, not egui's panel-resize machinery: egui's
        // separator highlights the edge white on hover and only flips the
        // cursor inside a razor-thin band (owner report 2026-08-16 — CSP shows
        // the cursor and nothing else). The panels are fixed-size; the handles
        // below each edge do the dragging with a generous grab band.
        let lp = egui::Panel::left("mn.left")
            .exact_size(app.layout.left_w)
            .resizable(false)
            .show_separator_line(false)
            .frame(chrome_frame(egui::Margin::same(2)))
            .show(ui, |ui| column(ui, app, true));

        let rp = egui::Panel::right("mn.side")
            .exact_size(app.layout.right_w)
            .resizable(false)
            .show_separator_line(false)
            .frame(chrome_frame(egui::Margin::same(2)))
            .show(ui, |ui| column(ui, app, false));

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
    adjust_window(ui.ctx(), app);
    goto_page_window(ui.ctx(), app);
    spread_window(ui.ctx(), app);
    export_all_window(ui.ctx(), app);
    story_window(ui.ctx(), app);
    gen_lines_window(ui.ctx(), app);
    filter_window(ui.ctx(), app);
    workspace_window(ui.ctx(), app);
    detail_window(ui.ctx(), app);
    property_detail_window(ui.ctx(), app);
    feedback_window(ui.ctx(), app);
    hud(ui.ctx(), app);

    app.sync_dock_layout();
    app.layout.save_if_dirty();
    app.prefs.save_if_dirty();
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
    // Never let the two columns swallow the canvas.
    let canvas_min = 120.0;
    let left_max = (COL_MAX)
        .min(win_w - app.layout.right_w - canvas_min)
        .max(LEFT_MIN);
    let right_max = (COL_MAX)
        .min(win_w - app.layout.left_w - canvas_min)
        .max(RIGHT_MIN);

    let x = left.right();
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
    ui.painter().vline(
        x,
        egui::Rangef::new(left.top(), left.bottom()),
        egui::Stroke::new(1.0, theme::BORDER),
    );

    let x = right.left();
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
    ui.painter().vline(
        x,
        egui::Rangef::new(right.top(), right.bottom()),
        egui::Stroke::new(1.0, theme::BORDER),
    );
}
