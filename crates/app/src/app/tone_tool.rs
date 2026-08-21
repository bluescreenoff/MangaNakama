//! One-gesture screentone (ROADMAP "further out").
//!
//! The recipe this replaces was: wand-click the area, Layer ▸ New live
//! layer ▸ Tone, deselect. Three gestures, three chances to leave a
//! selection behind. The Tone tool does the whole thing on one click.
//!
//! Nothing here is a new tone model. The flood is the FILL tool's own
//! `mn_core::fill::flood_region` (so tolerance, gap closing, area scaling
//! and 参照 mean what they mean under the bucket), and the result is the
//! existing live tone layer — `FillKind::Tone` + a window mask, parameters
//! not pixels, editable a week later through the same Tool Property that
//! edits every other live layer.

use crate::app::App;
use mn_core::{FillKind, Selection, SelectionOp};

/// One click = one live tone layer = one undo press.
///
/// The layer add is `Document::add_fill_layer`, which records a single
/// `UndoGroup::Structure` — the whole gesture is that one step, and the
/// selection detour below is invisible to it (a Structure snapshot holds
/// layers and the active index, never the selection).
pub(crate) fn tone_region(app: &mut App, x: f32, y: f32) {
    // The flood samples DISPLAY pixels; a tone or frame layer that has not
    // re-derived would be sampled as its stale raster.
    app.refresh_tones();
    let opts = app.tone_opts;
    let seed = (x as i32, y as i32);
    let Some(region) = mn_core::fill::flood_region(&app.doc, seed, &opts.region) else {
        app.set_status("click inside the page");
        return;
    };
    let w = app.doc.size.0 as usize;
    let mut window = Selection::from_mask(&app.doc, &region, w);
    // An active selection still clips, exactly as it clips a bucket fill —
    // the tool removes the deselect STEP, it does not ignore a selection
    // the artist deliberately made.
    if let Some(active) = app.doc.selection.clone() {
        window = active.combine(&window, &app.doc, SelectionOp::Intersect);
    }
    let px = covered_px(app, &window);
    if px == 0 {
        app.set_status("nothing enclosed there — raise Close gap, or click inside the area");
        return;
    }

    // `add_fill_layer` cuts the window from `doc.selection`, so the region
    // is handed over as one — and PUT BACK, because a selection left on the
    // page is the step this gesture exists to delete.
    let had = app.doc.selection.take();
    app.doc.selection = Some(window);
    let kind = FillKind::Tone {
        tone: opts.tone,
        density: opts.density,
    };
    app.doc.add_fill_layer(kind, true);
    app.doc.selection = had;

    app.refresh_tones();
    app.renderer.invalidate();
    app.set_status(format!(
        "toned region {px} px², {:.0} LPI {} at {:.0}° — {:.0}% density",
        opts.tone.lpi,
        opts.tone.pattern.label().to_lowercase(),
        opts.tone.angle_deg,
        opts.density * 100.0
    ));
    app.mark_dirty();
}

/// Canvas pixels the window actually covers, walked per TILE rather than
/// per pixel — a page-sized `coverage()` sweep is a hash lookup per pixel.
fn covered_px(app: &App, sel: &Selection) -> u64 {
    use mn_core::tile::{TILE_SIZE, TileIdx};
    let (w, h) = (app.doc.size.0 as i32, app.doc.size.1 as i32);
    let t = TILE_SIZE as i32;
    let mut n = 0u64;
    for ty in 0..h.div_euclid(t) + 1 {
        for tx in 0..w.div_euclid(t) + 1 {
            let idx = TileIdx::new(tx, ty);
            let Some(cov) = sel.tile_mask(idx) else {
                continue;
            };
            let (ox, oy) = idx.origin();
            for (p, c) in cov.iter().enumerate() {
                let (px, py) = (ox + (p % TILE_SIZE) as i32, oy + (p / TILE_SIZE) as i32);
                if *c != 0 && px < w && py < h {
                    n += 1;
                }
            }
        }
    }
    n
}
