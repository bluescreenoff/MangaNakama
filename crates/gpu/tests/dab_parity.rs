//! GPU dab path (P1) vs the CPU rasterizer: the two must agree.
//!
//! The same scripted stroke runs twice — once stock (the vendored C
//! rasterizer, the reference) and once in BYPASS record mode with the dabs
//! rasterized by `dab.wgsl` and read back at stroke end. The blend math is
//! u32 fix15 in both (exact); the mask math is f32 in both, so the parity
//! bar is ≤ 1/32765 per channel (docs/design/GPU-DABS.md §5) — one ulp of
//! float noise from cos/sin, never visible.
//!
//! These tests skip, not fail, when no adapter can be created (same policy
//! as composite.rs), and serialize on the same GPU lock (same reason).

use mn_brush::{MyBrush, RecordMode};
use mn_core::{Document, PenSample, StrokeSink, TILE_LEN, TileIdx};
use mn_gpu::{GpuConfig, Renderer};
use std::path::Path;

static GPU_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_guard() -> std::sync::MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn renderer() -> Option<Renderer> {
    let force_fallback = std::env::var("MN_WARP").is_ok();
    match Renderer::new_headless(GpuConfig {
        force_fallback,
        no_vsync: false,
    }) {
        Ok(r) => {
            println!("[test] adapter: {}", r.adapter_line());
            Some(r)
        }
        Err(e) => {
            println!("[test] SKIP: no usable adapter ({e})");
            None
        }
    }
}

fn pen() -> MyBrush {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes/classic/pen.myb");
    MyBrush::load(&path).expect("pen.myb loads")
}

fn sample(i: usize) -> PenSample {
    // A curve with pressure variation, crossing a tile border.
    let t = i as f32;
    PenSample {
        x: 40.0 + t * 9.0,
        y: 120.0 + 60.0 * (t * 0.25).sin(),
        pressure: 0.25 + 0.75 * (t * 0.11).sin().abs(),
        tilt_x: 0.0,
        tilt_y: 0.0,
        t_ms: i as f64 * 16.0,
    }
}

fn stroke_samples() -> Vec<PenSample> {
    (0..40).map(sample).collect()
}

fn run_stock(mut brush: MyBrush, doc: &mut Document) {
    let samples = stroke_samples();
    doc.begin_op();
    brush.begin(doc);
    for s in &samples {
        brush.sample(doc, *s);
    }
    brush.end(doc);
    doc.end_op();
}

/// The GPU pass, mirroring the app: BYPASS record, per-batch drain + flush,
/// stroke-end readback writing the CPU tiles. On a canary mismatch (the
/// cursed-driver defense) it repairs on CPU exactly as the app does — so the
/// parity assertions prove BOTH the GPU path (WARP + well-behaved adapters)
/// and the repair path (this laptop's iGPU drops dispatches). Returns the
/// canary verdict plus the full dab list (for repair-path assertions).
fn run_gpu(
    mut brush: MyBrush,
    doc: &mut Document,
    renderer: &mut Renderer,
    hard: bool,
) -> (bool, Vec<mn_core::dab::DabParams>) {
    run_gpu_maybe_composited(brush_into(&mut brush), doc, renderer, hard, false, None)
}

/// `MyBrush` is not `Clone`; this just hands the harness its owned brush.
fn brush_into(b: &mut MyBrush) -> &mut MyBrush {
    b
}

/// The GPU pass. `composite` reproduces the REAL app's frame loop, which the
/// original harness did not: `App::render` calls `flush_gpu_dabs()` and then
/// composites the document in the same frame (the Pages thumbnail, then the
/// canvas). Compositing walks the tile cache and can upload / evict entries
/// mid-stroke, which is invisible to a flush-only test.
fn run_gpu_maybe_composited(
    brush: &mut MyBrush,
    doc: &mut Document,
    renderer: &mut Renderer,
    hard: bool,
    composite: bool,
    tex: Option<(&[u8], u32, bool)>,
) -> (bool, Vec<mn_core::dab::DabParams>) {
    let samples = stroke_samples();
    let mut all_dabs = Vec::new();
    brush.set_dab_recording(RecordMode::Bypass);
    renderer.begin_dab_stroke(0);
    doc.begin_op();
    brush.begin(doc);
    for (i, s) in samples.iter().enumerate() {
        brush.sample(doc, *s);
        if i % 7 == 6 {
            let rec = brush.take_dab_record();
            renderer.flush_dabs(doc, &rec.dabs, hard, tex);
            all_dabs.extend(rec.dabs);
            if composite {
                let _ = renderer.render_offscreen(doc, 320, 240);
            }
        }
    }
    brush.end(doc);
    let rec = brush.take_dab_record();
    renderer.flush_dabs(doc, &rec.dabs, hard, tex);
    all_dabs.extend(rec.dabs);
    if composite {
        let _ = renderer.render_offscreen(doc, 320, 240);
    }
    let (layer, _wash, tiles) = renderer.end_dab_stroke().expect("stroke was open");
    let (px, canary_ok) = renderer.readback_dab_tiles(layer, &tiles);
    if canary_ok {
        for (idx, data) in px {
            let tile = doc.layers[layer].tile_mut(idx);
            let rev = tile.revision();
            tile.data_mut()[..TILE_LEN].copy_from_slice(&data);
            renderer.mark_dab_tile_clean(layer, idx, rev);
        }
    } else {
        eprintln!("[test] canary mismatch — a dispatch was dropped; repairing on CPU");
        let nz = px
            .iter()
            .filter(|(_, d)| d.iter().any(|v: &u16| *v != 0))
            .count();
        eprintln!("[test] gpu pixels: {}/{} tiles non-zero", nz, px.len());
        mn_brush::rasterize_dabs(doc, layer, &all_dabs, hard, tex);
        for idx in &tiles {
            let rev = doc.layers[layer]
                .tile(*idx)
                .map(|t| t.revision())
                .unwrap_or(0);
            renderer.mark_dab_tile_clean(layer, *idx, rev);
        }
    }
    doc.end_op();
    (canary_ok, all_dabs)
}

fn max_diff(a: &Document, b: &Document) -> (u32, usize) {
    let mut max: u32 = 0;
    let mut over = 0usize;
    let mut tiles: std::collections::BTreeSet<TileIdx> = Default::default();
    for (idx, _) in a.layers[0].tiles() {
        tiles.insert(idx);
    }
    for (idx, _) in b.layers[0].tiles() {
        tiles.insert(idx);
    }
    let zero = [0u16; TILE_LEN];
    for idx in tiles {
        let pa = a.layers[0].tile(idx).map(|t| t.data()).unwrap_or(&zero);
        let pb = b.layers[0].tile(idx).map(|t| t.data()).unwrap_or(&zero);
        for (x, y) in pa.iter().zip(pb.iter()) {
            let d = (*x).abs_diff(*y) as u32;
            if d > max {
                max = d;
            }
            if d > 1 {
                over += 1;
            }
        }
    }
    (max, over)
}

fn assert_inked(doc: &Document) -> usize {
    let n = doc.layers[0]
        .tiles()
        .filter(|(_, t)| t.data().iter().any(|&v| v != 0))
        .count();
    assert!(n > 0, "reference stroke painted nothing");
    n
}

/// Parity bar: max channel diff <= 1 (one ulp of float noise; the blend
/// math is u32 and exact). The canary must hold on WARP (byte-deterministic)
/// and any well-behaved adapter; on the cursed iGPU a dropped dispatch is
/// the DOCUMENTED trap — the repair path then carries parity, which `max`
/// still asserts.
fn check(canary_ok: bool, max: u32) {
    if std::env::var("MN_WARP").is_ok() {
        assert!(
            canary_ok,
            "canary must match the dispatched workgroup count on WARP"
        );
    }
    assert!(max <= 1, "parity bar is <= 1/32765 per channel, got {max}");
}

/// The owner's 2026-08-17 report: with `--gpu-dabs`, normal strokes on a
/// FRESH canvas came out with tile-shaped blocks of solid ink and chunks of
/// the stroke missing.
///
/// Every other parity test flushes repeatedly and only then reads back, but
/// the real app COMPOSITES between flushes — `App::render` calls
/// `flush_gpu_dabs()` and then composites twice in the same frame (Pages
/// thumbnail + canvas). That compositing walks the same tile cache the dab
/// path writes into. This test is the flush-composite-flush loop the app
/// actually runs.
#[test]
fn gpu_dab_parity_survives_compositing_between_flushes() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let mut ref_doc = Document::default();
    run_stock(pen(), &mut ref_doc);
    let n = assert_inked(&ref_doc);

    let mut gpu_doc = Document::default();
    let mut b = pen();
    let (canary_ok, _) =
        run_gpu_maybe_composited(&mut b, &mut gpu_doc, &mut renderer, false, true, None);

    let (max, over) = max_diff(&ref_doc, &gpu_doc);
    println!(
        "[test] composited parity: {n} tiles, max channel diff {max}, channels over 1: {over}"
    );
    check(canary_ok, max);
}

#[test]
fn gpu_dab_parity_pen() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let mut ref_doc = Document::default();
    run_stock(pen(), &mut ref_doc);
    let n = assert_inked(&ref_doc);

    let mut gpu_doc = Document::default();
    let (canary_ok, _) = run_gpu(pen(), &mut gpu_doc, &mut renderer, false);

    let (max, over) = max_diff(&ref_doc, &gpu_doc);
    println!("[test] pen parity: {n} tiles, max channel diff {max}, channels over 1: {over}");
    check(canary_ok, max);
}

#[test]
fn gpu_dab_parity_eraser_over_ink() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    // The eraser arrives as colour_a < 1 — the Normal_and_Eraser blend —
    // over existing ink.
    let paint = |doc: &mut Document| {
        let tile = doc.layers[0].tile_mut(TileIdx::new(1, 1));
        for px in tile.data_mut().chunks_exact_mut(4) {
            px[0] = 20000;
            px[1] = 15000;
            px[2] = 10000;
            px[3] = 32768;
        }
    };
    let mut ref_doc = Document::default();
    paint(&mut ref_doc);
    let mut ref_brush = pen();
    ref_brush.set_eraser(true);
    run_stock(ref_brush, &mut ref_doc);
    assert_inked(&ref_doc);

    let mut gpu_doc = Document::default();
    paint(&mut gpu_doc);
    let mut gpu_brush = pen();
    gpu_brush.set_eraser(true);
    let (canary_ok, _) = run_gpu(gpu_brush, &mut gpu_doc, &mut renderer, false);

    let (max, over) = max_diff(&ref_doc, &gpu_doc);
    println!("[test] eraser parity: max channel diff {max}, over 1: {over}");
    check(canary_ok, max);
}

/// Colorize (P4 port): the brush hue/sat replace the canvas pixel's,
/// keeping its luma — over a mid-colour base so the de-premult, SetLum and
/// ClipColor integer paths all run.
#[test]
fn gpu_dab_parity_colorize_over_ink() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let paint = |doc: &mut Document| {
        let tile = doc.layers[0].tile_mut(TileIdx::new(1, 1));
        for px in tile.data_mut().chunks_exact_mut(4) {
            px[0] = 20000;
            px[1] = 15000;
            px[2] = 10000;
            px[3] = 32768;
        }
    };
    let colorized = |mut b: MyBrush| -> MyBrush {
        b.set_colorize(1.0);
        b.set_color_rgb([0.9, 0.2, 0.4]);
        b
    };
    let mut ref_doc = Document::default();
    paint(&mut ref_doc);
    run_stock(colorized(pen()), &mut ref_doc);
    assert_inked(&ref_doc);

    let mut gpu_doc = Document::default();
    paint(&mut gpu_doc);
    let (canary_ok, _) = run_gpu(colorized(pen()), &mut gpu_doc, &mut renderer, false);

    let (max, over) = max_diff(&ref_doc, &gpu_doc);
    println!("[test] colorize parity: max channel diff {max}, over 1: {over}");
    check(canary_ok, max);
}

/// Posterize (P4 port): canvas rgb quantized to the level count, blended at
/// the stamp opacity — over a gradient-ish base so several quantization
/// buckets are hit.
#[test]
fn gpu_dab_parity_posterize_over_ink() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let paint = |doc: &mut Document| {
        let tile = doc.layers[0].tile_mut(TileIdx::new(1, 1));
        for (i, px) in tile.data_mut().chunks_exact_mut(4).enumerate() {
            let v = ((i % 64) * 512) as u16;
            px[0] = v;
            px[1] = 32768 - v;
            px[2] = v / 2 + 8000;
            px[3] = 32768;
        }
    };
    let posterized = |mut b: MyBrush| -> MyBrush {
        // .myb scale: the C computes CLAMP(ROUND(0.05 * 100), 1, 128) = 5.
        b.set_posterize(1.0, 0.05);
        b
    };
    let mut ref_doc = Document::default();
    paint(&mut ref_doc);
    run_stock(posterized(pen()), &mut ref_doc);
    assert_inked(&ref_doc);

    let mut gpu_doc = Document::default();
    paint(&mut gpu_doc);
    let (canary_ok, _) = run_gpu(posterized(pen()), &mut gpu_doc, &mut renderer, false);

    let (max, over) = max_diff(&ref_doc, &gpu_doc);
    println!("[test] posterize parity: max channel diff {max}, over 1: {over}");
    check(canary_ok, max);
}

/// Wave-4 spectral port: Perceptual (Paint) mixing over existing ink — the
/// WGM arm proper. The base is a saturated mid-colour so the un-premult,
/// `rgb_to_spectral`, `fastpow` WGM and re-premult paths all run with real
/// numbers; the brush colour is far from the base so a wrong mix is a
/// large-scale hue error, not one ulp. The repair leg pins the pure-Rust
/// mirror (cpu_raster's paint arms) against the same C reference.
#[test]
fn gpu_dab_parity_paint_over_ink() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let paint_base = |doc: &mut Document| {
        let tile = doc.layers[0].tile_mut(TileIdx::new(1, 1));
        for px in tile.data_mut().chunks_exact_mut(4) {
            px[0] = 6000;
            px[1] = 15000;
            px[2] = 28000;
            px[3] = 32768;
        }
    };
    let pigment = |mut b: MyBrush| -> MyBrush {
        b.set_color_mixing(mn_brush::BrushMix::Perceptual);
        b.set_color_rgb([0.9, 0.8, 0.1]);
        b
    };
    let mut ref_doc = Document::default();
    paint_base(&mut ref_doc);
    run_stock(pigment(pen()), &mut ref_doc);
    assert_inked(&ref_doc);

    let mut gpu_doc = Document::default();
    paint_base(&mut gpu_doc);
    let mut gpu_brush = pigment(pen());
    let (canary_ok, all_dabs) =
        run_gpu_maybe_composited(&mut gpu_brush, &mut gpu_doc, &mut renderer, false, false, None);

    let (max, over) = max_diff(&ref_doc, &gpu_doc);
    println!("[test] paint parity: max channel diff {max}, over 1: {over}");
    check(canary_ok, max);

    let mut repair_doc = Document::default();
    paint_base(&mut repair_doc);
    repair_doc.begin_op();
    mn_brush::rasterize_dabs(&mut repair_doc, 0, &all_dabs, false, None);
    repair_doc.end_op();
    let (max, over) = max_diff(&ref_doc, &repair_doc);
    println!("[test] paint repair parity: max channel diff {max}, over 1: {over}");
    assert!(max <= 1, "repair parity bar is <= 1, got {max}");
}

/// Paint mode on a FRESH canvas: the first dabs take the C's zero-alpha
/// additive shortcut, the overlapping ones mix spectrally with the stroke's
/// own ink — both branches of Normal_Paint, plus the low-opacity 150 clamp
/// at the stroke's faded pressure ends.
#[test]
fn gpu_dab_parity_paint_fresh_canvas() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let pigment = |mut b: MyBrush| -> MyBrush {
        b.set_color_mixing(mn_brush::BrushMix::Perceptual);
        b.set_color_rgb([0.2, 0.5, 0.9]);
        b
    };
    let mut ref_doc = Document::default();
    run_stock(pigment(pen()), &mut ref_doc);
    assert_inked(&ref_doc);

    let mut gpu_doc = Document::default();
    let mut gpu_brush = pigment(pen());
    let (canary_ok, all_dabs) =
        run_gpu_maybe_composited(&mut gpu_brush, &mut gpu_doc, &mut renderer, false, false, None);

    let (max, over) = max_diff(&ref_doc, &gpu_doc);
    println!("[test] paint fresh-canvas parity: max channel diff {max}, over 1: {over}");
    check(canary_ok, max);

    let mut repair_doc = Document::default();
    repair_doc.begin_op();
    mn_brush::rasterize_dabs(&mut repair_doc, 0, &all_dabs, false, None);
    repair_doc.end_op();
    let (max, over) = max_diff(&ref_doc, &repair_doc);
    println!("[test] paint fresh-canvas repair parity: max diff {max}, over 1: {over}");
    assert!(max <= 1, "repair parity bar is <= 1, got {max}");
}

/// Paint-mode ERASER (colour_a < 1) over ink: Normal_and_Eraser_Paint — the
/// additive/spectral cross-fade on canvas alpha, the `fac_a *= color_a`
/// erase adjustment, and no min-opacity clamp. The pre-ink alpha varies by
/// row so the sigmoid fade runs at several points, not one.
#[test]
fn gpu_dab_parity_paint_eraser_over_ink() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let paint_base = |doc: &mut Document| {
        let tile = doc.layers[0].tile_mut(TileIdx::new(1, 1));
        for (i, px) in tile.data_mut().chunks_exact_mut(4).enumerate() {
            // Straight colour premultiplied by a per-row alpha ramp.
            let a = ((i / 64) * 512).min(32768) as u32;
            px[0] = (20000 * a / 32768) as u16;
            px[1] = (15000 * a / 32768) as u16;
            px[2] = (10000 * a / 32768) as u16;
            px[3] = a as u16;
        }
    };
    let pigment_eraser = |mut b: MyBrush| -> MyBrush {
        b.set_color_mixing(mn_brush::BrushMix::Perceptual);
        b.set_eraser(true);
        b
    };
    let mut ref_doc = Document::default();
    paint_base(&mut ref_doc);
    run_stock(pigment_eraser(pen()), &mut ref_doc);
    assert_inked(&ref_doc);

    let mut gpu_doc = Document::default();
    paint_base(&mut gpu_doc);
    let mut gpu_brush = pigment_eraser(pen());
    let (canary_ok, all_dabs) =
        run_gpu_maybe_composited(&mut gpu_brush, &mut gpu_doc, &mut renderer, false, false, None);

    let (max, over) = max_diff(&ref_doc, &gpu_doc);
    println!("[test] paint-eraser parity: max channel diff {max}, over 1: {over}");
    check(canary_ok, max);

    let mut repair_doc = Document::default();
    paint_base(&mut repair_doc);
    repair_doc.begin_op();
    mn_brush::rasterize_dabs(&mut repair_doc, 0, &all_dabs, false, None);
    repair_doc.end_op();
    let (max, over) = max_diff(&ref_doc, &repair_doc);
    println!("[test] paint-eraser repair parity: max diff {max}, over 1: {over}");
    assert!(max <= 1, "repair parity bar is <= 1, got {max}");
}

#[test]
fn gpu_dab_parity_hard_stamp() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let mut ref_doc = Document::default();
    let mut ref_brush = pen();
    ref_brush.set_hard_dab(true);
    run_stock(ref_brush, &mut ref_doc);
    assert_inked(&ref_doc);

    let mut gpu_doc = Document::default();
    let mut gpu_brush = pen();
    gpu_brush.set_hard_dab(true);
    let (canary_ok, _) = run_gpu(gpu_brush, &mut gpu_doc, &mut renderer, true);

    let (max, over) = max_diff(&ref_doc, &gpu_doc);
    println!("[test] hard-stamp parity: max channel diff {max}, over 1: {over}");
    check(canary_ok, max);
}

/// A deterministic synthetic mask: 16×16, five distinct gray levels in a
/// non-repeating (x·7+y·13) pattern including hard zeros — enough structure
/// that a wrong anchor, wrap, or per-dab offset shows up as a big diff.
fn synthetic_mask() -> std::sync::Arc<mn_brush::TextureMask> {
    let size = 16u32;
    let data = (0..size * size)
        .map(|i| {
            let (x, y) = (i % size, i / size);
            ((x * 7 + y * 13) % 5 * 63) as u8
        })
        .collect();
    std::sync::Arc::new(mn_brush::TextureMask {
        name: "synthetic".into(),
        size,
        data: std::sync::Arc::new(data),
    })
}

/// #0.1: texture tips on the GPU. The CPU reference multiplies the dab
/// profile by the canvas-anchored mask in `render_dab_mask`; the GPU must
/// reproduce the SAME multiply (canvas-anchored wrap + the per-dab crawl
/// offset carried in `DabParams::tex_off`) with the same f32-before-u16
/// order. The crawl step is nonzero so consecutive dabs see different
/// offsets — pinning the per-dab plumbing, not just a fixed-offset lookup.
/// A wrong anchor or offset changes large-scale mask structure, not one
/// ulp; the ≤1 bar catches it.
#[test]
fn gpu_dab_parity_texture_tips() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let mask = synthetic_mask();

    let mut ref_doc = Document::default();
    let mut ref_brush = pen();
    ref_brush.set_texture(Some(mask.clone()));
    ref_brush.set_texture_scroll(2.0);
    run_stock(ref_brush, &mut ref_doc);
    assert_inked(&ref_doc);

    let mut gpu_doc = Document::default();
    let mut gpu_brush = pen();
    gpu_brush.set_texture(Some(mask.clone()));
    gpu_brush.set_texture_scroll(2.0);
    let tex = (mask.data.as_slice(), mask.size, false);
    let (canary_ok, all_dabs) = run_gpu_maybe_composited(
        &mut gpu_brush,
        &mut gpu_doc,
        &mut renderer,
        false,
        false,
        Some(tex),
    );

    let (max, over) = max_diff(&ref_doc, &gpu_doc);
    println!("[test] texture-tips parity: max channel diff {max}, over 1: {over}");
    check(canary_ok, max);

    // The canary-repair path must reproduce the C too: re-raster the SAME
    // recorded dabs through the pure-Rust rasterizer (texture included — the
    // per-dab crawl offsets ride in DabParams) onto a fresh doc and diff
    // against the C reference. ≤1 is the repair's correctness bar on any
    // adapter where the canary fires (the iGPU among them).
    let mut repair_doc = Document::default();
    repair_doc.begin_op();
    mn_brush::rasterize_dabs(&mut repair_doc, 0, &all_dabs, false, Some(tex));
    repair_doc.end_op();
    let (max, over) = max_diff(&ref_doc, &repair_doc);
    println!("[test] texture-tips repair parity: max channel diff {max}, over 1: {over}");
    assert!(max <= 1, "repair parity bar is <= 1, got {max}");
}

/// #10 amendment 2: DAB-ANCHORED stamps with per-dab rotation. The stroke
/// is a curve, so direction-following rotates every dab differently — the
/// per-dab `tex_angle` channel (op snapshot → record → GPU struct) is what
/// this pins, against the C reference AND through the repair rasterizer.
/// A wrong or folded angle re-orients whole stamps, not one ulp.
#[test]
fn gpu_dab_parity_dab_anchored_stamps() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let mask = synthetic_mask();
    let arm = |b: &mut MyBrush| {
        b.set_texture(Some(mask.clone()));
        b.set_texture_anchor_dab(true);
        b.set_texture_rotate(mn_brush::TextureRotate::Direction);
        b.set_texture_angle_deg(30.0);
    };

    let mut ref_doc = Document::default();
    let mut ref_brush = pen();
    arm(&mut ref_brush);
    run_stock(ref_brush, &mut ref_doc);
    assert_inked(&ref_doc);

    let mut gpu_doc = Document::default();
    let mut gpu_brush = pen();
    arm(&mut gpu_brush);
    let tex = (mask.data.as_slice(), mask.size, true);
    let (canary_ok, all_dabs) = run_gpu_maybe_composited(
        &mut gpu_brush,
        &mut gpu_doc,
        &mut renderer,
        false,
        false,
        Some(tex),
    );

    let (max, over) = max_diff(&ref_doc, &gpu_doc);
    println!("[test] dab-anchored stamp parity: max channel diff {max}, over 1: {over}");
    check(canary_ok, max);

    let mut repair_doc = Document::default();
    repair_doc.begin_op();
    mn_brush::rasterize_dabs(&mut repair_doc, 0, &all_dabs, false, Some(tex));
    repair_doc.end_op();
    let (max, over) = max_diff(&ref_doc, &repair_doc);
    println!("[test] dab-anchored stamp repair parity: max diff {max}, over 1: {over}");
    assert!(max <= 1, "repair parity bar is <= 1, got {max}");
}

/// #10 amendment 3: PURE STAMP. In dab-anchored mode the tip mask IS the
/// coverage — no radial profile — and the stamp's square is not clipped to
/// the dab's disc. An all-ink mask must therefore ink the square's CORNER
/// (diagonal distance ~1.24r, where the old profile was already zero), at
/// full strength (where the old gaussian would have faded). Runs on the
/// repair rasterizer: explicit dab centre, no engine placement noise; the
/// C and GPU agree with it via the parity tests above.
#[test]
fn anchored_stamp_mask_is_the_coverage() {
    let all_ink = std::sync::Arc::new(mn_brush::TextureMask {
        name: "ink".into(),
        size: 16,
        data: std::sync::Arc::new(vec![255u8; 16 * 16]),
    });
    let dab = mn_core::dab::DabParams {
        x: 100.0,
        y: 100.0,
        radius: 16.0,
        color: [0, 0, 0],
        alpha: 1.0,
        opaque: 1.0,
        hardness: 0.8,
        aspect_ratio: 1.0,
        angle: 0.0,
        lock_alpha: 0.0,
        paint: 0.0,
        colorize: 0.0,
        posterize: 0.0,
        posterize_num: 1,
        tex_off: [0, 0],
        tex_angle: 0.0,
    };
    let mut doc = Document::default();
    doc.begin_op();
    mn_brush::rasterize_dabs(
        &mut doc,
        0,
        &[dab],
        false,
        Some((all_ink.data.as_slice(), all_ink.size, true)),
    );
    doc.end_op();
    let alpha_at = |x: i32, y: i32| -> u16 {
        let idx = mn_core::TileIdx::of_pixel(x, y);
        doc.layers[0]
            .tile(idx)
            .map(|t| {
                let (ox, oy) = idx.origin();
                t.pixel((x - ox) as usize, (y - oy) as usize)[3]
            })
            .unwrap_or(0)
    };
    // Corner of the stamp square: (114, 114) is 14,14 from centre —
    // diagonal 19.8 px > radius 16, rr = 1.53, profile = 0 in the old code.
    let corner = alpha_at(114, 114);
    assert!(
        corner > 30_000,
        "stamp corner must be (near) full ink, got {corner}"
    );
    // Mid-edge, just inside the square: full strength, not a gaussian tail.
    let edge = alpha_at(114, 100);
    assert!(edge > 30_000, "stamp edge must be full ink, got {edge}");
    // One px outside the square on the axis: over.
    assert_eq!(alpha_at(117, 100), 0, "outside the stamp square is dry");
}

/// Rows 58 + 167 (`I-014`), RETARGETED by the wave-4 spectral port: the
/// predecessor of this test asserted Perceptual could never reach the GPU
/// path because `dab.wgsl` had no pigment model — and told its replacer to
/// swap it for a real parity run once one was ported. The parity runs are
/// the `gpu_dab_parity_paint_*` tests above; what remains here is the
/// ROUTING and the RECORD: Perceptual keeps `gpu_ready` and its dabs carry
/// the spectral weight the shader's `*_Paint` arms consume, while Standard
/// records zero (the additive path untouched to the byte).
#[test]
fn spectral_routing_and_the_recorded_paint_weight() {
    let mut b = pen();
    assert!(b.gpu_ready(), "the stock pen is the GPU path's own brush");
    b.set_color_mixing(mn_brush::BrushMix::Perceptual);
    assert!(
        b.gpu_ready(),
        "static spectral mixing rides the GPU arms since the wave-4 port"
    );

    let record = |mix: mn_brush::BrushMix| -> Vec<mn_core::dab::DabParams> {
        let mut doc = Document::default();
        let mut rec = pen();
        rec.set_color_mixing(mix);
        rec.set_dab_recording(RecordMode::Tap);
        rec.begin(&mut doc);
        for s in stroke_samples() {
            rec.sample(&mut doc, s);
        }
        rec.end(&mut doc);
        rec.take_dab_record().dabs
    };
    let standard = record(mn_brush::BrushMix::Standard);
    assert!(!standard.is_empty(), "the reference stroke recorded no dabs");
    assert!(
        standard.iter().all(|d| d.paint == 0.0),
        "a Standard brush recorded a dab with a spectral weight"
    );
    let pigment = record(mn_brush::BrushMix::Perceptual);
    assert!(
        pigment.iter().all(|d| d.paint == 1.0),
        "a Perceptual brush's dabs must carry the weight the paint arms read"
    );
}
