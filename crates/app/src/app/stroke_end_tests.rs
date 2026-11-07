//! Perf lane 3 (2026-11-06 plan, "pen-up submits, frame-end lands"): the
//! GPU stroke's readback is SUBMITTED at pen-up and LANDED at the tail of
//! the next frame (or at the first door that needs the document first).
//!
//! What these pin: the landed bytes are the synchronous path's bytes; the
//! selection clamp, the transparent-pixel lock and the undo step still run
//! after the write and inside the stroke's own op; every door that would
//! touch the document before the landing lands it first; and CPU strokes
//! never leave anything pending.
//!
//! All headless on the machine's adapter; each test skips (with the shared
//! message) where the adapter has no storage tiles.

use super::*;
use crate::cmd::{AppCmd, dispatch};
use mn_core::{Document, PenSample, TileIdx};
use std::collections::BTreeMap;

/// A GPU-dab app on a fixed 512 × 512 page, identity viewport (screen px ==
/// canvas px, so selection rects and stroke points share one space).
fn gpu_app() -> Option<App> {
    let renderer = headless_renderer()?;
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return None;
    }
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.doc = Document::new(512, 512);
    app.viewport = mn_gpu::Viewport::default();
    app.gpu_dabs = true;
    Some(app)
}

/// Begin a stroke and push its samples. The caller ends it (the point of
/// every test here is WHICH end).
fn stroke(app: &mut App, pts: &[(f32, f32)]) {
    app.begin_stroke(PointerKind::Mouse);
    let batch: Vec<PenSample> = pts
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| PenSample {
            x,
            y,
            pressure: 0.8,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        })
        .collect();
    app.push_batch(&batch);
}

/// A horizontal run of 30 points, 4 px apart, starting at `x0`.
fn line(x0: f32, y: f32) -> Vec<(f32, f32)> {
    (0..30).map(|i| (x0 + i as f32 * 4.0, y)).collect()
}

/// Exact snapshot of every tile of a layer.
fn layer_bytes(doc: &Document, li: usize) -> BTreeMap<TileIdx, Vec<u16>> {
    doc.layers[li]
        .tiles()
        .map(|(i, t)| (i, t.data().to_vec()))
        .collect()
}

/// The same snapshot with all-zero tiles dropped: an absent tile and an
/// all-transparent tile are the same pixels (undo removes a tile the
/// stroke created; the selection clamp zeroes it in place). The compare
/// stays exact on every byte that exists.
fn ink(doc: &Document, li: usize) -> BTreeMap<TileIdx, Vec<u16>> {
    let mut m = layer_bytes(doc, li);
    m.retain(|_, v| v.iter().any(|&x| x != 0));
    m
}

/// One pixel of a snapshot; absent tile = transparent.
fn px_of(m: &BTreeMap<TileIdx, Vec<u16>>, idx: TileIdx, p: usize) -> [u16; 4] {
    m.get(&idx)
        .map(|d| [d[p * 4], d[p * 4 + 1], d[p * 4 + 2], d[p * 4 + 3]])
        .unwrap_or([0; 4])
}

/// Give layer `li` a non-trivial pre-image over the stroke area: a WHITE
/// premultiplied coat whose alpha ramps 0 → 1 across each tile's width
/// (written outside any op — it is the "before" the stroke must restore).
fn seed_ramp(doc: &mut Document, li: usize) {
    let one = mn_core::tile::FIX15_ONE as u16;
    for ty in 2..5 {
        for tx in 1..5 {
            let t = doc.layers[li].tile_mut(TileIdx::new(tx, ty));
            for y in 0..64 {
                for x in 0..64 {
                    let a = ((x as u32 * one as u32) / 63) as u16;
                    t.set_pixel(x, y, [a, a, a, a]);
                }
            }
        }
    }
}

/// Fresh engine (the carryover rule — see `gpu_dab_parity_wash`): both
/// sides of a byte compare must be FIRST-stroke instances.
fn fresh_engine(app: &mut App) {
    let i = app
        .selected_preset
        .expect("a preset must be selected for the fresh-engine swap");
    let p = app.presets[i].1.clone();
    let b = mn_brush::MyBrush::load(&p).expect("preset reload must succeed");
    *app.engine_mut() = Engine::new(EngineKind::My(Box::new(b)));
}

/// 1 — the deferral moves WHEN, never WHAT.
#[test]
fn queued_end_lands_the_same_bytes_as_the_sync_end() {
    let Some(mut a) = gpu_app() else { return };
    let Some(mut b) = gpu_app() else { return };
    seed_ramp(&mut a.doc, 0);
    seed_ramp(&mut b.doc, 0);

    stroke(&mut a, &line(100.0, 200.0));
    a.end_stroke();
    assert!(a.pending_stroke_end.is_none());

    stroke(&mut b, &line(100.0, 200.0));
    b.end_stroke_queued();
    assert!(
        b.pending_stroke_end.is_some(),
        "the GPU stroke must queue its landing"
    );
    assert!(b.finish_pending_stroke_end("test").is_some());

    assert!(a.dab_path_last.starts_with("gpu |"), "{}", a.dab_path_last);
    assert_eq!(
        layer_bytes(&a.doc, 0),
        layer_bytes(&b.doc, 0),
        "queued and synchronous ends landed different pixels"
    );
    assert_eq!(a.doc.undo_len(), 1);
    assert_eq!(b.doc.undo_len(), 1);
    assert_eq!(b.renderer.readback_timing().landed, "test");
    assert!(
        b.dab_path_last.contains("landed test"),
        "HUD line: {}",
        b.dab_path_last
    );
}

/// 2 — the selection clamp still runs after the write, inside the op.
#[test]
fn ink_never_escapes_the_selection_when_queued() {
    let Some(mut app) = gpu_app() else { return };
    seed_ramp(&mut app.doc, 0);
    let sel = mn_core::selection::Selection::from_rect(&app.doc, 0.0, 0.0, 160.0, 512.0);
    app.doc.selection = Some(sel.clone());
    let pre = layer_bytes(&app.doc, 0);

    // 100 → 216: crosses the selection's right edge at x = 160.
    stroke(&mut app, &line(100.0, 200.0));
    app.end_stroke_queued();
    assert!(app.pending_stroke_end.is_some());
    assert_eq!(
        layer_bytes(&app.doc, 0),
        pre,
        "the CPU tiles must stay untouched while the landing is pending"
    );
    app.finish_pending_stroke_end("test");
    let post = layer_bytes(&app.doc, 0);

    let mut inside_changed = 0usize;
    let mut outside_checked = 0usize;
    for (&idx, _) in post.iter() {
        let mask = sel.tile_mask(idx);
        for p in 0..64 * 64 {
            let m = mask.map(|m| m[p]).unwrap_or(0);
            let (a, b) = (px_of(&pre, idx, p), px_of(&post, idx, p));
            if m == 0 {
                outside_checked += 1;
                assert_eq!(a, b, "ink escaped the selection at {idx:?} pixel {p}");
            } else if m == 255 && a != b {
                inside_changed += 1;
            }
        }
    }
    assert!(outside_checked > 0, "the stroke never reached outside");
    assert!(inside_changed > 0, "the stroke painted nothing inside");

    // The escape hatch: draw_outside lets the stroke land outside.
    let li = app.doc.add_layer("free");
    app.doc.set_active(li);
    let mut free = sel.clone();
    free.draw_outside = true;
    app.doc.selection = Some(free);
    stroke(&mut app, &line(100.0, 300.0));
    app.end_stroke_queued();
    app.finish_pending_stroke_end("test");
    let after = layer_bytes(&app.doc, li);
    let outside_inked = after.iter().any(|(&idx, d)| {
        let mask = sel.tile_mask(idx);
        (0..64 * 64).any(|p| mask.map(|m| m[p]).unwrap_or(0) == 0 && d[p * 4 + 3] != 0)
    });
    assert!(outside_inked, "draw_outside: the stroke was clamped anyway");
}

/// 3 — the landed stroke is one undo step with the true pre-images.
#[test]
fn undo_restores_exactly_after_a_queued_end() {
    let Some(mut app) = gpu_app() else { return };
    seed_ramp(&mut app.doc, 0);
    let pre = ink(&app.doc, 0);
    stroke(&mut app, &line(100.0, 200.0));
    app.end_stroke_queued();
    app.finish_pending_stroke_end("test");
    let post = ink(&app.doc, 0);
    assert_ne!(post, pre, "the stroke painted nothing");
    assert_eq!(app.doc.undo_len(), 1);

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(ink(&app.doc, 0), pre, "undo did not restore the pre-image");
    assert_eq!((app.doc.undo_len(), app.doc.redo_len()), (0, 1));
    dispatch(&mut app, AppCmd::Redo);
    assert_eq!(ink(&app.doc, 0), post, "redo did not restore the stroke");
    assert_eq!((app.doc.undo_len(), app.doc.redo_len()), (1, 0));
}

/// 4a — Undo before the landing: the dispatch door lands first, so the
/// undo takes the WHOLE stroke back (not an empty op).
#[test]
fn undo_before_the_landing_flushes_first() {
    let Some(mut app) = gpu_app() else { return };
    seed_ramp(&mut app.doc, 0);
    let pre = ink(&app.doc, 0);
    stroke(&mut app, &line(100.0, 200.0));
    app.end_stroke_queued();
    assert!(app.pending_stroke_end.is_some());

    dispatch(&mut app, AppCmd::Undo);
    assert!(app.pending_stroke_end.is_none());
    assert_eq!(app.renderer.readback_timing().landed, "dispatch");
    assert_eq!(ink(&app.doc, 0), pre, "undo left stroke pixels behind");
    assert_eq!(app.doc.redo_len(), 1);
}

/// 4b — a second pen-down before the landing: the first stroke lands and
/// closes as its own step before the second opens its op.
#[test]
fn a_second_stroke_before_the_landing_is_its_own_step() {
    let Some(mut app) = gpu_app() else { return };
    stroke(&mut app, &line(100.0, 200.0));
    app.end_stroke_queued();
    assert!(app.pending_stroke_end.is_some());

    app.begin_stroke(PointerKind::Mouse);
    assert!(app.pending_stroke_end.is_none());
    assert_eq!(app.renderer.readback_timing().landed, "begin_stroke");
    let after_a = ink(&app.doc, 0);
    assert!(!after_a.is_empty(), "stroke A painted nothing");
    let batch: Vec<PenSample> = line(100.0, 300.0)
        .into_iter()
        .enumerate()
        .map(|(i, (x, y))| PenSample {
            x,
            y,
            pressure: 0.8,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();
    assert_eq!(app.doc.undo_len(), 2, "{:?}", app.doc.undo_labels());

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(ink(&app.doc, 0), after_a, "undo took more than stroke B");
}

/// 4c — focus / capture loss lands the stroke.
#[test]
fn focus_loss_lands_the_stroke() {
    let Some(mut app) = gpu_app() else { return };
    stroke(&mut app, &line(100.0, 200.0));
    app.end_stroke_queued();
    assert!(app.pending_stroke_end.is_some());
    app.cancel_input_latches("test");
    assert!(app.pending_stroke_end.is_none());
    assert_eq!(app.renderer.readback_timing().landed, "focus");
    assert_eq!(app.doc.undo_len(), 1);
}

/// 4d — a layer switch before the landing: the ink lands on the stroke's
/// own layer, never on the newly active one.
#[test]
fn layer_switch_before_the_landing_keeps_the_stroke_on_its_layer() {
    let Some(mut app) = gpu_app() else { return };
    let top = app.doc.add_layer("top");
    app.doc.set_active(0);
    let top_before = layer_bytes(&app.doc, top);
    stroke(&mut app, &line(100.0, 200.0));
    app.end_stroke_queued();
    assert!(app.pending_stroke_end.is_some());

    dispatch(&mut app, AppCmd::SelectLayer(top));
    assert!(app.pending_stroke_end.is_none());
    assert_eq!(app.doc.active, top);
    assert!(!ink(&app.doc, 0).is_empty(), "layer 0 lost the stroke");
    assert_eq!(
        layer_bytes(&app.doc, top),
        top_before,
        "the stroke landed on the newly active layer"
    );
}

/// 4e — a synchronous `end_stroke` with a landing outstanding (pages,
/// text edit, screenshots, bench) lands it exactly once.
#[test]
fn sync_end_stroke_with_a_landing_outstanding_is_idempotent() {
    let Some(mut app) = gpu_app() else { return };
    stroke(&mut app, &line(100.0, 200.0));
    app.end_stroke_queued();
    assert!(app.pending_stroke_end.is_some());
    app.end_stroke();
    assert!(app.pending_stroke_end.is_none());
    assert_eq!(app.renderer.readback_timing().landed, "end_stroke");
    assert_eq!(app.doc.undo_len(), 1);
    let once = layer_bytes(&app.doc, 0);
    app.end_stroke();
    assert_eq!(app.doc.undo_len(), 1);
    assert_eq!(layer_bytes(&app.doc, 0), once);
}

/// 5 — the transparent-pixel lock runs once, after the write.
#[test]
fn alpha_lock_holds_when_queued() {
    let Some(mut app) = gpu_app() else { return };
    seed_ramp(&mut app.doc, 0);
    app.doc.layers[0].lock_alpha = true;
    let pre = layer_bytes(&app.doc, 0);
    stroke(&mut app, &line(100.0, 200.0));
    app.end_stroke_queued();
    app.finish_pending_stroke_end("test");
    let post = layer_bytes(&app.doc, 0);
    let mut colour_changed = false;
    for (&idx, d) in post.iter() {
        for p in 0..64 * 64 {
            let a = px_of(&pre, idx, p);
            assert_eq!(
                d[p * 4 + 3],
                a[3],
                "alpha moved under the lock at {idx:?} pixel {p}"
            );
            colour_changed |= d[p * 4..p * 4 + 3] != a[..3];
        }
    }
    assert!(colour_changed, "the locked stroke painted nothing");
}

/// 6 — the canary repair (a dropped dispatch) still runs at the landing:
/// byte-identical to the synchronous repair, and within the canary test's
/// own tolerance of a pure-CPU reference stroke.
#[test]
fn canary_repair_still_runs_when_queued() {
    let Some(mut s) = gpu_app() else { return };
    let Some(mut q) = gpu_app() else { return };
    s.renderer.debug_drop_next_flush();
    stroke(&mut s, &line(100.0, 200.0));
    s.end_stroke();

    q.renderer.debug_drop_next_flush();
    stroke(&mut q, &line(100.0, 200.0));
    q.end_stroke_queued();
    assert!(q.pending_stroke_end.is_some());
    q.finish_pending_stroke_end("test");
    assert_eq!(q.dab_path_last, "gpu → cpu repair!");
    assert_eq!(q.diag.dab_canary_repairs, 1);
    assert_eq!(q.doc.undo_len(), 1);
    assert_eq!(
        layer_bytes(&s.doc, 0),
        layer_bytes(&q.doc, 0),
        "queued repair differs from the synchronous repair"
    );

    // CPU reference (canary_repair_composites_the_healed_tiles' method):
    // fresh engine, CPU path, second layer; per-tile alpha within 5%.
    q.gpu_dabs = false;
    fresh_engine(&mut q);
    let r = q.doc.add_layer("ref");
    q.doc.set_active(r);
    stroke(&mut q, &line(100.0, 200.0));
    q.end_stroke_queued();
    assert!(q.pending_stroke_end.is_none());
    let sums = |li: usize| -> BTreeMap<TileIdx, u64> {
        q.doc.layers[li]
            .tiles()
            .filter(|(_, t)| t.alpha_sum() > 0)
            .map(|(i, t)| (i, t.alpha_sum()))
            .collect()
    };
    let (healed, reference) = (sums(0), sums(r));
    assert_eq!(
        healed.keys().collect::<Vec<_>>(),
        reference.keys().collect::<Vec<_>>()
    );
    for (k, a) in &reference {
        let b = healed[k];
        let rel = (*a as i64 - b as i64).unsigned_abs() as f64 / (*a).max(1) as f64;
        assert!(rel < 0.05, "repair alpha drifted on {k:?}: {b} vs {a}");
    }
}

/// 7 — the GPU wash commit runs at the landing, byte-identical to the
/// synchronous wash end (cloned from `gpu_dab_parity_wash`).
#[test]
fn wash_commit_lands_at_the_landing() {
    let Some(mut app) = gpu_app() else { return };
    fresh_engine(&mut app);
    dispatch(&mut app, AppCmd::SetWash(true));
    stroke(&mut app, &line(100.0, 200.0));
    app.end_stroke();
    assert!(
        app.dab_path_last.starts_with("gpu |"),
        "the wash stroke silently left the GPU path: {}",
        app.dab_path_last
    );

    let li = app.doc.add_layer("queued");
    app.doc.set_active(li);
    fresh_engine(&mut app);
    dispatch(&mut app, AppCmd::SetWash(true));
    stroke(&mut app, &line(100.0, 200.0));
    app.end_stroke_queued();
    assert!(app.pending_stroke_end.is_some());
    assert!(
        layer_bytes(&app.doc, li).is_empty(),
        "the wash committed before the landing"
    );
    app.finish_pending_stroke_end("test");
    assert!(app.pending_stroke_end.is_none());
    let sync = layer_bytes(&app.doc, 0);
    assert!(!sync.is_empty(), "the wash stroke painted nothing");
    assert_eq!(sync, layer_bytes(&app.doc, li));
}

/// 8 — the frame tail is the normal landing; the renderer stroke (the
/// eviction guard) stays up until then, so a composite in between keeps
/// every tile the stroke created.
///
/// `App::render` cannot run headless: with no surface, `render_with_overlay`
/// returns before the egui painter applies the frame's texture deltas, and
/// epaint panics on the unapplied delta. So, per the plan's fallback, frame
/// N+1 is built the way `screenshot.rs` `frame()` builds it (the UI pass),
/// composited offscreen (standing in for acquire + present), and then the
/// frame-tail call `App::render` makes is made directly.
#[test]
fn the_render_tail_lands_the_stroke() {
    let Some(mut app) = gpu_app() else { return };
    stroke(&mut app, &line(100.0, 200.0));
    // The touched set, read BEFORE pen-up (after one frame-head flush), so
    // the eviction check below does not depend on the guard it tests.
    app.flush_gpu_dabs();
    let (layer, _, touched) = app.renderer.dab_stroke_info().expect("a GPU stroke");
    assert!(!touched.is_empty());
    app.end_stroke_queued();
    assert!(app.pending_stroke_end.is_some());

    // Frame N+1, UI half: building the panels must not land the stroke
    // (I16 — widgets push commands; they never touch the op directly).
    let ctx = app.shell.ctx.clone();
    let raw = app.shell.begin((600, 400));
    let mut out = ctx.run_ui(raw, |ui| crate::ui::build(ui, &mut app));
    app.shell.end(&out);
    out.textures_delta.clear();
    assert!(
        app.pending_stroke_end.is_some(),
        "the UI build landed the stroke"
    );
    // Frame N+1, composite half: the guard keeps the stroke's tiles (none
    // of which exist on the CPU yet) in the cache.
    let vp = app.viewport;
    let _ = app.renderer.render_offscreen_vp(&app.doc, &vp, 320, 240);
    for idx in &touched {
        assert!(
            app.renderer.readback_dab_tile(layer, *idx).is_some(),
            "tile {idx:?} was evicted before the landing (holes in frame N+1)"
        );
    }
    assert!(
        app.renderer.dab_stroke_info().is_some(),
        "the renderer stroke (eviction guard) must stay open while pending"
    );

    // Frame N+1, tail.
    assert!(app.finish_pending_stroke_end("frame-end").is_some());
    assert!(app.pending_stroke_end.is_none());
    assert!(app.renderer.dab_stroke_info().is_none());
    assert_eq!(app.doc.undo_len(), 1);
    assert_eq!(app.renderer.readback_timing().landed, "frame-end");
}

/// 9 — CPU strokes have nothing to read back and never go pending.
#[test]
fn no_pending_state_for_cpu_strokes() {
    let Some(mut app) = gpu_app() else { return };
    app.gpu_dabs = false;
    stroke(&mut app, &line(100.0, 200.0));
    app.end_stroke_queued();
    assert!(app.pending_stroke_end.is_none());
    assert_eq!(app.doc.undo_len(), 1);
}

/// 10 — a GPU stroke with no samples touches no tile: nothing pending,
/// and the renderer stroke is closed at once.
#[test]
fn empty_gpu_stroke_leaves_nothing_pending() {
    let Some(mut app) = gpu_app() else { return };
    app.begin_stroke(PointerKind::Mouse);
    assert!(app.renderer.dab_stroke_info().is_some(), "not a GPU stroke");
    app.end_stroke_queued();
    assert!(app.pending_stroke_end.is_none());
    assert!(app.renderer.dab_stroke_info().is_none());
}

/// Door beyond the plan's table (2026-11-07): `canvas_up` runs
/// `finish_smart_shape` right after the pen-up, and its swap presses Undo
/// only when exactly one op closed since the press. A pending stroke has
/// not closed its op yet, so without the door the swap refused ("kept it
/// as drawn") on every GPU stroke.
#[test]
fn smart_shape_swap_lands_a_queued_gpu_stroke_first() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let mut app = App::new(renderer, (400, 400), 1.0);
    app.viewport = mn_gpu::Viewport::default();
    app.tool = crate::cmd::Tool::Figure;
    app.figure_mode = crate::cmd::FigureMode::Smart;
    app.prefs.smart_shape = true;
    app.prefs.smart_hold_ms = crate::app::SMART_HOLD_MS;
    app.prefs.smart_fit_tol = mn_core::shape_fit::FIT_TOL;
    app.props_current.size_px = 5.0;
    app.apply_props();
    app.gpu_dabs = true;
    let steps = app.doc.undo_labels().len();

    let one = |p: [f32; 2], i: usize| {
        [PenSample {
            x: p[0],
            y: p[1],
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        }]
    };
    let path: Vec<[f32; 2]> = (0..=96)
        .map(|i| {
            let t = i as f32 / 96.0 * std::f32::consts::TAU;
            let rr = 100.0 + if i % 2 == 0 { 4.0 } else { -4.0 };
            [200.0 + rr * t.cos(), 200.0 + rr * t.sin()]
        })
        .collect();
    app.canvas_down(path[0][0], path[0][1], PointerKind::Pen, &one(path[0], 0));
    for (i, p) in path.iter().enumerate().skip(1) {
        app.canvas_move(p[0], p[1], &one(*p, i));
    }
    assert!(
        app.dab_path_last.starts_with("gpu"),
        "the smart-shape stroke is not on the GPU path: {}",
        app.dab_path_last
    );
    let ms = app.prefs.smart_hold_ms + 50;
    {
        let g = app.smart_shape.as_mut().expect("a smart shape gesture");
        g.refused = false;
        g.still_since = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_millis(ms))
            .unwrap();
    }
    app.smart_shape_tick();
    assert!(
        app.smart_shape.as_ref().and_then(|g| g.preview()).is_some(),
        "the hold recognized nothing"
    );
    let last = path[path.len() - 1];
    app.canvas_up(last[0], last[1], &one(last, 9999));

    assert!(app.pending_stroke_end.is_none());
    assert!(app.status.contains("circle"), "status: {}", app.status);
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "{:?}",
        app.doc.undo_labels()
    );
}

/// Door beyond the plan's table (2026-11-07): `canvas_up` runs
/// `finish_fill_repair` right after the pen-up. Real-ink mode re-runs the
/// fill against the closing stroke's PIXELS and wraps stroke + refill into
/// one undo press; with the stroke still pending the fill would leak
/// through the gap again and join the stroke's open op.
#[test]
fn fill_repair_lands_a_queued_gpu_stroke_first() {
    use super::new_document_tests::headless;
    let Some(mut app) = headless() else { return };
    if !app.renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    // The U with a 16 px gap in its bottom band (fill_repair_tests' shape).
    let ink_px = [0, 0, 0, mn_core::tile::FIX15_ONE as u16];
    let paint = |doc: &mut Document, x: i32, y: i32| {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        doc.active_layer_mut()
            .tile_mut(idx)
            .set_pixel((x - ox) as usize, (y - oy) as usize, ink_px);
    };
    let lineart = app.doc.active;
    for x in 20..=108 {
        paint(&mut app.doc, x, 20);
        if !(56..72).contains(&x) {
            paint(&mut app.doc, x, 108);
        }
    }
    for y in 20..=108 {
        paint(&mut app.doc, 20, y);
        paint(&mut app.doc, 108, y);
    }
    let flats = app.doc.add_layer("flats");
    app.doc.set_active(flats);
    let px = |app: &App, li: usize, x: i32, y: i32| -> [u16; 4] {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        app.doc.layers[li]
            .display_tile(idx)
            .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
            .unwrap_or([0; 4])
    };

    dispatch(&mut app, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));
    dispatch(&mut app, AppCmd::Fill(60.0, 60.0));
    assert!(px(&app, flats, 60, 124)[3] > 0, "the leak");
    dispatch(
        &mut app,
        AppCmd::ArmFillRepair {
            virtual_barrier: false,
        },
    );
    dispatch(&mut app, AppCmd::SetTool(crate::cmd::Tool::Pen));
    app.doc.set_active(lineart);
    dispatch(&mut app, AppCmd::SetBrushSizePx(9.0));
    app.gpu_dabs = true;
    let s = |app: &App, cx: f32, cy: f32| app.viewport.to_screen(cx, cy);
    let (ax, ay) = s(&app, 52.0, 106.0);
    app.canvas_down(ax, ay, PointerKind::Mouse, &[]);
    for x in [56.0, 60.0, 64.0, 68.0, 72.0] {
        let (mx, my) = s(&app, x, 106.0);
        app.canvas_move(
            mx,
            my,
            &[PenSample {
                x: mx,
                y: my,
                pressure: 0.9,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: 0.0,
            }],
        );
    }
    assert!(
        app.dab_path_last.starts_with("gpu"),
        "the closing stroke is not on the GPU path: {}",
        app.dab_path_last
    );
    let (ux, uy) = s(&app, 76.0, 106.0);
    app.canvas_up(ux, uy, &[]);

    assert!(app.pending_stroke_end.is_none());
    assert!(app.fill_repair.is_none(), "the gesture closed");
    assert!(px(&app, lineart, 64, 106)[3] > 0, "the closing ink");
    assert!(px(&app, flats, 60, 60)[3] > 0, "the fill re-ran");
    assert_eq!(px(&app, flats, 60, 124)[3], 0, "contained by the ink");
    assert_eq!(
        app.doc.undo_labels().last().map(String::as_str),
        Some("Repair fill"),
        "{:?}",
        app.doc.undo_labels()
    );
}
