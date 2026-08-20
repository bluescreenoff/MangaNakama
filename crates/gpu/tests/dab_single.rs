//! Minimal GPU-dab unit checks: one synthetic dab, and two overlapping dabs
//! in one flush, each vs the Rust CPU mirror — bit-exact (the parity
//! tolerance of dab_parity.rs is not needed at this scale; any difference
//! here is a plumbing bug like the round-28 struct-stride mismatch).

use mn_core::{Document, TILE_LEN, TileIdx, dab::DabParams};
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

fn check_against_mirror(renderer: &mut Renderer, dabs: &[DabParams]) {
    check_against_mirror_over(renderer, Document::default(), dabs)
}

/// As above but starting from `mirror`'s existing pixels — `flush_dabs` seeds
/// each destination tile from that document, so both sides start identical.
fn check_against_mirror_over(renderer: &mut Renderer, mut mirror: Document, dabs: &[DabParams]) {
    renderer.begin_dab_stroke(0);
    renderer.flush_dabs(&mirror, dabs, false, None);
    let (layer, _wash, tiles) = renderer.end_dab_stroke().expect("stroke was open");
    let (px, canary_ok) = renderer.readback_dab_tiles(layer, &tiles);
    assert!(
        canary_ok,
        "canary must match the dispatched workgroup count"
    );

    mn_brush::rasterize_dabs(&mut mirror, 0, dabs, false, None);
    let zero = [0u16; TILE_LEN];
    for (idx, data) in &px {
        let m = mirror.layers[0]
            .tile(*idx)
            .map(|t| t.data())
            .unwrap_or(&zero);
        for (o, (g, r)) in data.iter().zip(m.iter()).enumerate() {
            assert_eq!(g, r, "tile {idx:?} value {o}: gpu={g} mirror={r}");
        }
    }
    assert!(
        px.iter().any(|(_, d)| d.iter().any(|&v| v != 0)),
        "gpu painted nothing"
    );
}

#[test]
fn gpu_dab_single_dab_is_bit_exact() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    check_against_mirror(&mut renderer, &[make(40.0, 120.0, 2.3)]);
}

#[test]
fn gpu_dab_two_dabs_one_flush_are_bit_exact() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    // Two overlapping small (AA-path) dabs: also pins the params-array
    // stride against the Rust-side struct.
    check_against_mirror(
        &mut renderer,
        &[
            make(40.0, 120.0, 2.3),
            make(42.0, 121.0, 2.3),
            make(41.5, 119.0, 4.7),
        ],
    );
}

/// The owner's 2026-08-17 bug report: with `--gpu-dabs`, drawing "drew things
/// I didn't expect and not things I did".
///
/// Cause: `flush_dabs` took the destination tile texture straight from
/// `tile_pool` (or created a fresh zero one) and dabbed into it WITHOUT
/// uploading the tile's current CPU pixels. Painting over existing artwork
/// therefore either erased it (fresh zero texture) or resurrected an unrelated
/// evicted tile's pixels (recycled texture), and the stroke-end readback
/// committed the result to the CPU tile.
///
/// Every other dab test paints into an EMPTY document, where seeding from zero
/// happens to be correct — which is exactly why none of them caught it. This
/// one pre-paints the tile the dabs land on.
#[test]
fn gpu_dabs_preserve_the_artwork_they_paint_over() {
    let _g = gpu_guard();
    let Some(mut renderer) = renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }

    let mut doc = Document::default();
    // Fill the tile the dabs will land on (pixel 40,120 -> tile 0,1) with an
    // opaque colour. Premultiplied fix15, so every channel <= alpha.
    let tile = doc.layers[0].tile_mut(TileIdx::new(0, 1));
    for y in 0..64 {
        for x in 0..64 {
            tile.set_pixel(x, y, [9000, 4500, 1200, 20000]);
        }
    }

    // A dab big enough to cover part of that tile without filling it, so both
    // the painted-over and the untouched pixels are compared.
    check_against_mirror_over(&mut renderer, doc, &[make(40.0, 120.0, 6.0)]);
}
