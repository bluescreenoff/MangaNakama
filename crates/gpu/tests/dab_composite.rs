//! Composited-display agreement for the GPU dab path — the class no
//! tile-level parity test can see (round 31, owner-ordered).
//!
//! `dab_parity.rs`/`dab_single.rs` compare DOCUMENT TILES: the readback
//! proves the rasterizer. They are structurally blind to everything between
//! the tiles and the screen — texture-cache freshness, damage derivation,
//! the composite itself. The 2026-08-17 divergence hunt existed precisely
//! because a correct document rendered from a stale canvas. These tests
//! compare `render_offscreen` OUTPUT (the full composite path) between the
//! CPU reference and the GPU dab path driven exactly like the app drives it
//! (flush → stroke-end readback → CPU tiles written → `mark_dab_tile_clean`)
//! — if damage derivation regresses (a reintroduced upload/redraw conflation
//! or one-shot damage set), the canvas shows paper or pre-stroke pixels and
//! these go red.

use mn_core::{Document, dab::DabParams};
use mn_gpu::{GpuConfig, Renderer};

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
        Ok(r) => Some(r),
        Err(e) => {
            println!("[test] SKIP: no usable adapter ({e})");
            None
        }
    }
}

fn make(x: f32, y: f32, r: f32) -> DabParams {
    DabParams {
        x,
        y,
        radius: r,
        color: [0, 0, 0],
        alpha: 1.0,
        opaque: 0.9,
        hardness: 0.8,
        aspect_ratio: 1.0,
        angle: 0.0,
        lock_alpha: 0.0,
        paint: 0.0,
        tex_off: [0, 0],
    }
}

/// A dab list spanning several tiles (so the flush + damage path covers
/// multi-tile regions, like a real stroke).
fn stroke_dabs() -> Vec<DabParams> {
    let mut v = Vec::new();
    for k in 0..48 {
        let t = k as f32 / 47.0;
        v.push(make(
            30.0 + t * 200.0,
            100.0 + (t * 9.0).sin() * 40.0,
            3.0 + t * 5.0,
        ));
    }
    v
}

/// The app's `finish_gpu_dab_stroke` happy path, verbatim shape: final
/// flush, single readback, CPU tiles become authoritative again, texture
/// cache marked clean WITHOUT touching the canvas-side record.
fn gpu_stroke_like_the_app(renderer: &mut Renderer, doc: &mut Document, dabs: &[DabParams]) {
    renderer.begin_dab_stroke(0);
    renderer.flush_dabs(doc, dabs, false, None);
    let (layer, _wash, tiles) = renderer.end_dab_stroke().expect("stroke was open");
    let (px, canary_ok) = renderer.readback_dab_tiles(layer, &tiles);
    assert!(
        canary_ok,
        "canary must match the dispatched workgroup count"
    );
    for (idx, data) in px {
        let tile = doc.layers[layer].tile_mut(idx);
        let rev = tile.revision();
        tile.data_mut()[..data.len()].copy_from_slice(&data);
        renderer.mark_dab_tile_clean(layer, idx, rev);
    }
}

fn max_channel_diff(a: &image::RgbaImage, b: &image::RgbaImage) -> (u32, u32) {
    assert_eq!((a.width(), a.height()), (b.width(), b.height()));
    let mut max: u32 = 0;
    let mut over: u32 = 0;
    for (p, q) in a.pixels().zip(b.pixels()) {
        let m = (p.0[0].abs_diff(q.0[0]) as u32)
            .max(p.0[1].abs_diff(q.0[1]) as u32)
            .max(p.0[2].abs_diff(q.0[2]) as u32);
        if m > 2 {
            over += 1;
        }
        max = max.max(m);
    }
    (over, max)
}

/// The owner-ordered pin: the GPU dab path's COMPOSITED display must agree
/// with the CPU reference's composited display. Tolerance ≤2 per channel —
/// the GPU raster differs from the CPU by fix15 rounding (measured ~21 alpha
/// units summed over a stroke), which quantizes away in the u8 canvas; the
/// pre-fix display divergences were max-channel 170.
#[test]
fn gpu_dab_composited_display_matches_cpu_reference() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let dabs = stroke_dabs();

    // CPU reference first (its tile revisions are then the OLDEST, so the
    // GPU document's fresh revisions force honest uploads + damage in the
    // second composite — no accidental aliasing shielding the path).
    let mut ref_doc = Document::default();
    mn_brush::rasterize_dabs(&mut ref_doc, 0, &dabs, false, None);
    let ref_img = renderer.render_offscreen(&ref_doc, ref_doc.size.0, ref_doc.size.1);

    let mut gpu_doc = Document::default();
    gpu_stroke_like_the_app(&mut renderer, &mut gpu_doc, &dabs);
    let gpu_img = renderer.render_offscreen(&gpu_doc, gpu_doc.size.0, gpu_doc.size.1);

    let (over, max) = max_channel_diff(&ref_img, &gpu_img);
    assert!(
        over == 0 && max <= 2,
        "composited display disagrees: {over} px >2, max channel diff {max} \
         (the canvas composited stale content — damage-derivation regression)"
    );
}

/// Repeated composites of the same document (the Pages-thumbnail-then-main-
/// canvas flow, twice per frame in the live app) must be idempotent: the
/// stroke stays visible and the pixels do not drift.
#[test]
fn gpu_dab_composite_is_idempotent_across_repeated_composites() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let dabs = stroke_dabs();
    let mut doc = Document::default();
    gpu_stroke_like_the_app(&mut renderer, &mut doc, &dabs);
    let a = renderer.render_offscreen(&doc, doc.size.0, doc.size.1);
    let b = renderer.render_offscreen(&doc, doc.size.0, doc.size.1);
    let (over, max) = max_channel_diff(&a, &b);
    assert_eq!((over, max), (0, 0), "second composite changed the canvas");
    // And the ink is actually there (a paper-white canvas would trivially
    // "agree" with itself).
    assert!(
        a.pixels().any(|p| p.0[0] < 250),
        "composite shows no stroke ink"
    );
}

/// Live preview: mid-stroke, the flushes write tile textures ahead of any
/// CPU revision (BYPASS freezes the CPU tiles). The composite must still
/// show the dabs — via the stroke-state damage in `update_canvas`, not a
/// revision that has not bumped yet.
#[test]
fn gpu_dab_live_preview_composites_mid_stroke() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let dabs = stroke_dabs();
    let mut doc = Document::default();
    renderer.begin_dab_stroke(0);
    // Two flushes with a composite between them — the message-loop shape.
    renderer.flush_dabs(&doc, &dabs[..24], false, None);
    let mid1 = renderer.render_offscreen(&doc, doc.size.0, doc.size.1);
    renderer.flush_dabs(&doc, &dabs[24..], false, None);
    let mid2 = renderer.render_offscreen(&doc, doc.size.0, doc.size.1);
    assert!(
        mid1.pixels().any(|p| p.0[0] < 250),
        "first flush's dabs invisible mid-stroke"
    );
    assert!(
        mid2.pixels().any(|p| p.0[0] < 250),
        "second flush's dabs invisible mid-stroke"
    );
    // Stroke end still completes cleanly (readback over all touched tiles).
    gpu_stroke_finalize(&mut renderer, &mut doc);
}

fn gpu_stroke_finalize(renderer: &mut Renderer, doc: &mut Document) {
    let (layer, _wash, tiles) = renderer.end_dab_stroke().expect("stroke was open");
    let (px, canary_ok) = renderer.readback_dab_tiles(layer, &tiles);
    assert!(canary_ok);
    for (idx, data) in px {
        let tile = doc.layers[layer].tile_mut(idx);
        let rev = tile.revision();
        tile.data_mut()[..data.len()].copy_from_slice(&data);
        renderer.mark_dab_tile_clean(layer, idx, rev);
    }
}
