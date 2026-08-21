//! The egui layout: top bar + status bar sandwich ONE dock tree (docking 2,
//! docs/DOCKING-2.md) whose tabs are the palettes AND the canvas pane,
//! freely draggable, tabbable, splittable — palettes also tear off into
//! floating windows (ui/dock.rs — freer than CSP's edge-pinned palettes).
//! The canvas pane's body is the transparent hole the wgpu canvas shows
//! through, and `Shell::owns_pointer` routes on its rect. Widgets never
//! mutate app state directly — they push [`AppCmd`]s.

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
use overlay::canvas_overlay;
use top::{status_bar, top_bar};
use widgets::chrome_frame;

use crate::app::App;

pub fn build(ui: &mut egui::Ui, app: &mut App) {
    // One brush preview generated per frame: startup trickles, never hitches.
    app.preview_budget = 1;
    // Same rule for the docking-2 page panes' display textures.
    app.page_pane_budget = 1;
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

    // Docking 2 (docs/DOCKING-2.md): everything between the bars is ONE dock
    // tree, and the canvas is a pane inside it — its body sets the canvas
    // rect and draws the overlay + launcher (ui/dock.rs, canvas_pane_body).
    // Tab hides every palette for a clean drawing view (CSP behaviour): no
    // tree at all, the whole rect is canvas and there is no doc strip.
    if app.panels_hidden {
        let canvas = ui.available_rect_before_wrap();
        app.shell.set_canvas_rect_points(canvas);
        canvas_overlay(ui, app, canvas);
        launcher::selection_launcher(ui, app, canvas);
    } else {
        dock::tree(ui, app);
    }

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

