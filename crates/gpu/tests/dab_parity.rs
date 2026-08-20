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
    tex: Option<(&[u8], u32)>,
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
    let tex = (mask.data.as_slice(), mask.size);
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
