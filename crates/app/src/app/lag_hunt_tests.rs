//! Lag hunt (2026-09-10): the MEASUREMENTS, not assertions.
//!
//! The owner reports "a lot of various manganakama actions trigger lag" and
//! that speed/radial lines "crash manganakama from slowness". Everything in
//! here is `#[ignore]`d and prints a markdown table: the round's rule is
//! numbers before fixes, and a number nobody can reproduce is an opinion.
//!
//! Run: `CARGO_BUILD_JOBS=2 cargo test -p mn-app lag_hunt -- --ignored
//! --test-threads=1 --nocapture`.
//!
//! Deliberately at **B4 600 dpi = 6070 × 8598** — the page the owner draws
//! his chapter on, 12,825 tiles, forty times the pixels of the 72 dpi drafts
//! the rest of the suite uses on purpose. Headless uses the machine's real
//! adapter (no `MN_WARP`), which is the Intel UHD 620 he has.
//!
//! One thing headless CANNOT reproduce: there is no swapchain, so there is
//! no vsync back-pressure and the GPU queue is never as busy as it is in the
//! live app. The readback table below is therefore a FLOOR — the real app's
//! `[dab]` log lines are the ceiling.

use super::*;
use mn_core::genlines::builtin_presets;
use mn_core::{FrameSet, tone::ToneParams};

/// B4 at 600 dpi — the owner's real page. 6070 × 8598 px = 95 × 135 =
/// 12,825 tiles.
const B4: (u32, u32) = (6070, 8598);

/// One tile is 64 × 64 RGBA-u16 premultiplied fix15.
const TILE_KB: usize = 64 * 64 * 4 * 2 / 1024;

fn ms(t: Instant) -> f32 {
    t.elapsed().as_secs_f32() * 1000.0
}

/// The plan's document: B4 600 dpi, eight raster layers, one frame folder
/// cut into four panels, one text layer, one tone layer.
fn b4_document() -> mn_core::Document {
    let mut doc = mn_core::Document::new(B4.0, B4.1);
    doc.dpi = Some(600);
    let (w, h) = (B4.0 as f32, B4.1 as f32);
    for i in 0..8 {
        doc.add_layer(format!("raster {i}"));
    }
    // Eight raster layers in, put a halftone on one of them: a live tone is
    // a derived raster the compositor has to keep current, and "toggle a
    // tone layer" is one of the actions the owner calls laggy.
    doc.set_tone(1, Some(ToneParams::default()));
    doc.add_text_layer("text", mn_core::text::TextSet::default());
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| FrameSet::single_rect([x0, y0, x1, y1], 8.0);
    let f = doc.add_frame_folder("Frame 1", rect(0.0, 0.0, w, h));
    // Cut in half, then each half in half — four panels from one folder,
    // the same three gestures a page gets by hand.
    if let Some(lower) = doc.divide_frame_folder(f, rect(0.0, 0.0, w, h * 0.5), rect(0.0, h * 0.5, w, h), false)
    {
        doc.divide_frame_folder(f, rect(0.0, 0.0, w * 0.5, h * 0.5), rect(w * 0.5, 0.0, w, h * 0.5), false);
        doc.divide_frame_folder(lower, rect(0.0, h * 0.5, w * 0.5, h), rect(w * 0.5, h * 0.5, w, h), false);
    }
    doc
}

/// A placing gesture in the middle of the page — press at the centre, drag
/// a sixth of the page width to the right. That is what the effect-line
/// tool hands `LineOpts::place`.
fn placed_spec(i: usize, bounds: [f32; 4]) -> mn_core::genlines::GenLinesSpec {
    let p = &builtin_presets()[i];
    let a = [
        (bounds[0] + bounds[2]) * 0.5,
        (bounds[1] + bounds[3]) * 0.5,
    ];
    let b = [a[0] + (bounds[2] - bounds[0]) / 6.0, a[1]];
    (p.opts)(600).place(p.kind, a, b, bounds, 0x51ED_5EED ^ i as u64)
}

// --- effect lines --------------------------------------------------------

/// Every shipped preset, at its DEFAULT parameters, over the whole page and
/// over one panel. This is the row that has to come under 100 ms.
#[test]
#[ignore = "lag hunt measurement"]
fn lag_hunt_effect_lines() {
    let (w, h) = (B4.0 as f32, B4.1 as f32);
    println!("\n| preset | full page ms | tiles | MB | panel ms | tiles |");
    println!("|---|---|---|---|---|---|");
    for (i, p) in builtin_presets().iter().enumerate() {
        let full = placed_spec(i, [0.0, 0.0, w, h]);
        let t = Instant::now();
        let map = full.render(B4);
        let full_ms = ms(t);

        // A quarter-page panel: the same preset placed inside one frame.
        let panel = placed_spec(i, [0.0, 0.0, w * 0.5, h * 0.5]);
        let t = Instant::now();
        let pmap = panel.render(B4);
        let panel_ms = ms(t);

        println!(
            "| {} | {:.1} | {} | {:.1} | {:.1} | {} |",
            p.name,
            full_ms,
            map.len(),
            (map.len() * TILE_KB) as f32 / 1024.0,
            panel_ms,
            pmap.len(),
        );
    }
}

/// Repeated regens of the same layer — the shape a slider drag WOULD have
/// if it regenerated per move.
///
/// It does not. Both live-edit paths already coalesce to the release edge
/// (`ui/property/effect_lines.rs::gen_commit` holds the draft in
/// `App::gen_edit`; `canvas_input.rs`'s `gen_drag` regenerates once in the
/// pointer-UP handler). So this measures the repeat cost, not a real
/// gesture — five, not thirty, because at seconds per regen a
/// thirty-deep loop over twelve presets is a forty-minute test and nobody
/// would run it twice.
#[test]
#[ignore = "lag hunt measurement"]
fn lag_hunt_effect_lines_repeat() {
    let (w, h) = (B4.0 as f32, B4.1 as f32);
    const N: usize = 5;
    println!("\n| preset | {N} regens, total ms | per regen ms |");
    println!("|---|---|---|");
    for (i, p) in builtin_presets().iter().enumerate() {
        let mut spec = placed_spec(i, [0.0, 0.0, w, h]);
        let t = Instant::now();
        for step in 0..N {
            // A drag moves a handle: nudge the outer reach, which is what
            // the radius handle writes, and re-render at full quality —
            // exactly what `Document::regen_genlines` does today.
            spec.d += step as f32;
            let _ = spec.render(B4);
        }
        let total = ms(t);
        println!("| {} | {:.0} | {:.1} |", p.name, total, total / N as f32);
    }
}

// --- compositor ----------------------------------------------------------

/// `composite_order` + one full compositor rebuild of the B4 document.
/// Headless has no swapchain, so this goes through `render_offscreen_vp`,
/// which runs the same `update_canvas` the live frame does.
#[test]
#[ignore = "lag hunt measurement (allocates a document-sized canvas)"]
fn lag_hunt_composite() {
    let Some(mut renderer) = headless_renderer() else {
        return;
    };
    let doc = b4_document();
    println!("\ndoc: {}x{}, {} layers", doc.size.0, doc.size.1, doc.layers.len());

    let t = Instant::now();
    let order = doc.composite_order();
    println!("\n| op | ms | note |");
    println!("|---|---|---|");
    println!("| composite_order | {:.2} | {} steps |", ms(t), order.len());

    let vp = mn_gpu::Viewport::fit(doc.size, (1600, 1000));
    let t = Instant::now();
    let _ = renderer.render_offscreen_vp(&doc, &vp, 1600, 1000);
    let first = ms(t);
    let fs = renderer.frame_stats();
    println!(
        "| first composite (cold) | {:.0} | {} tiles, {} uploads, full={} , gpu-side {:.0} ms |",
        first, fs.composite_tiles, fs.uploads, fs.full, fs.ms
    );

    let t = Instant::now();
    let _ = renderer.render_offscreen_vp(&doc, &vp, 1600, 1000);
    let warm = ms(t);
    let fs = renderer.frame_stats();
    println!(
        "| second composite (warm) | {:.0} | {} tiles, {} uploads, full={} |",
        warm, fs.composite_tiles, fs.uploads, fs.full
    );

    renderer.invalidate();
    let t = Instant::now();
    let _ = renderer.render_offscreen_vp(&doc, &vp, 1600, 1000);
    let rebuild = ms(t);
    let fs = renderer.frame_stats();
    println!(
        "| forced full rebuild | {:.0} | {} tiles, {} uploads, gpu-side {:.0} ms |",
        rebuild, fs.composite_tiles, fs.uploads, fs.ms
    );
}

// --- the pen's stroke-end readback ---------------------------------------

/// `finish_gpu_dab_stroke`'s readback, split into submit / poll(wait) /
/// copy-out. The claim under test: the milliseconds are the WAIT, not the
/// copy — which is what makes an async readback the right fix rather than a
/// smaller one.
#[test]
#[ignore = "lag hunt measurement"]
fn lag_hunt_readback() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let mut app = App::new(renderer, (1600, 1000), 1.0);
    app.doc = b4_document();
    app.gpu_dabs = true;

    println!("\n| stroke | tiles | total ms | submit ms | poll(wait) ms | copy ms |");
    println!("|---|---|---|---|---|---|");
    for (label, n, step) in [
        ("short (2 tiles)", 8u32, 6.0f32),
        ("medium (10 tiles)", 40, 14.0),
        ("long (40 tiles)", 160, 22.0),
    ] {
        app.begin_stroke(PointerKind::Mouse);
        let batch: Vec<PenSample> = (0..n)
            .map(|i| PenSample {
                x: 400.0 + i as f32 * step,
                y: 400.0 + i as f32 * step * 0.35,
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            })
            .collect();
        app.push_batch(&batch);
        let t = Instant::now();
        app.end_stroke();
        let total = ms(t);
        let rt = app.renderer.readback_timing();
        println!(
            "| {} | {} | {:.1} | {:.2} | {:.2} | {:.2} |",
            label, rt.tiles, total, rt.submit_ms, rt.wait_ms, rt.copy_ms
        );
    }
    println!("\n(headless: no swapchain, so no vsync back-pressure — these are the FLOOR.)");
}

// --- the ordinary document actions the owner calls laggy ------------------

#[test]
#[ignore = "lag hunt measurement"]
fn lag_hunt_doc_ops() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (1600, 1000), 1.0);
    app.doc = b4_document();
    app.gpu_dabs = app.renderer.gpu_dabs_supported();

    // A 200-dab stroke to undo and redo.
    app.doc.set_active(2);
    app.begin_stroke(PointerKind::Mouse);
    let batch: Vec<PenSample> = (0..200)
        .map(|i| PenSample {
            x: 300.0 + i as f32 * 12.0,
            y: 500.0 + (i as f32 * 0.11).sin() * 300.0,
            pressure: 0.8,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();

    println!("\n| op | ms |");
    println!("|---|---|");
    let row = |name: &str, t: Instant| println!("| {} | {:.2} |", name, ms(t));

    let t = Instant::now();
    app.doc.set_layer_visible(1, false);
    row("layer visibility toggle (the tone layer)", t);

    let t = Instant::now();
    app.doc.set_layer_opacity(2, 0.5);
    row("layer opacity change", t);

    let t = Instant::now();
    app.doc.move_layer(2, 5);
    row("move_layer", t);

    let t = Instant::now();
    app.doc.undo();
    row("undo (200-dab stroke somewhere in the stack)", t);

    let t = Instant::now();
    app.doc.redo();
    row("redo", t);

    let (w, h) = (B4.0 as f32, B4.1 as f32);
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| FrameSet::single_rect([x0, y0, x1, y1], 8.0);
    let folder = app.doc.add_frame_folder("Frame X", rect(0.0, 0.0, w, h));
    let t = Instant::now();
    app.doc.divide_frame_folder(
        folder,
        rect(0.0, 0.0, w, h * 0.5),
        rect(0.0, h * 0.5, w, h),
        false,
    );
    row("divide_frame_folder", t);

    let t = Instant::now();
    let _ = app.doc.composite_order();
    row("composite_order", t);
}
