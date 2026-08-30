//! The timed differential for the shared tile-kernel seam: CPU reference vs
//! GPU, at B4/600 scale, for both consumers.
//!
//! `#[ignore]` because it is wall-clock and this machine is a laptop that
//! also has an owner using it. Run it deliberately:
//!
//! ```text
//! cargo test -p mn-gpu --test kernel_bench -- --ignored --nocapture
//! ```
//!
//! # What the sizes mean
//!
//! A B4 page at 600 dpi is 6070 × 8598 = 52.2 Mpx.
//!
//! * **Tone curve** runs the whole page: 12 825 tiles, in the same 256-tile
//!   batches the real derive uses (`correction::DERIVE_BATCH`), so only 8 MB
//!   of sources is resident at a time. That is a genuine full-page number,
//!   not an extrapolation.
//! * **Gaussian** runs a full-width band of 2048 rows — 12.4 Mpx, 99 MB per
//!   buffer — and the report multiplies by 4.2 for the full page. The whole
//!   page at once would be 417 MB of source plus the CPU's own ping-pong
//!   scratch plus the GPU's output, and asking a 16 GB laptop for ~1.3 GB of
//!   transient buffers to print one number is not a trade worth making. The
//!   work is row-independent up to the halo, so the scaling is honest.
//!
//! Neither number includes the below-composite walk, which the correction's
//! source cache is what removes; the third measurement here is that split,
//! because it is the one that decides whether the kernel matters at all.

use std::time::Instant;

use mn_core::adjust::{Adjust, correct_tile};
use mn_core::tile::{TILE_LEN, TILE_PIXELS, TileIdx};
use mn_core::{Filter, Raster};
use mn_gpu::{GpuConfig, Kernel, Renderer, TileJob};

const B4_W: usize = 6070;
const B4_H: usize = 8598;
const B4_TILES: usize = 12825; // ceil(6070/64) * ceil(8598/64) = 95 * 135
const BATCH: usize = 256;
const BAND_ROWS: usize = 2048;

fn renderer() -> Option<Renderer> {
    match Renderer::new_headless(GpuConfig {
        force_fallback: std::env::var("MN_WARP").is_ok(),
        no_vsync: false,
    }) {
        Ok(r) => {
            println!("adapter: {}", r.adapter_line());
            if !r.kernels_supported() {
                println!("SKIP: no compute shaders");
                return None;
            }
            Some(r)
        }
        Err(e) => {
            println!("SKIP: no usable adapter ({e})");
            None
        }
    }
}

fn noise(n: usize, seed: u32) -> Vec<u16> {
    let mut s = seed | 1;
    let mut v = vec![0u16; n];
    for x in v.iter_mut() {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        *x = (s % 32769) as u16;
    }
    // Premultiplied: force each pixel's channels under its alpha.
    for p in 0..n / 4 {
        let a = v[p * 4 + 3];
        for c in 0..3 {
            v[p * 4 + c] = v[p * 4 + c].min(a);
        }
    }
    v
}

fn tone_curve() -> Adjust {
    let mut pts = [[0.0f32; 2]; 8];
    pts[0] = [0.0, 0.05];
    pts[1] = [0.35, 0.2];
    pts[2] = [0.7, 0.85];
    pts[3] = [1.0, 1.0];
    Adjust::ToneCurve { pts, n: 4 }
}

#[test]
#[ignore = "wall-clock timing; run deliberately with --ignored --nocapture"]
fn timed_tone_curve_full_page_derive() {
    let Some(mut r) = renderer() else { return };
    let adj = tone_curve();
    let batch: Vec<Vec<u16>> = (0..BATCH).map(|i| noise(TILE_LEN, 1000 + i as u32)).collect();
    let idxs: Vec<TileIdx> = (0..BATCH).map(|i| TileIdx::new(i as i32, 0)).collect();
    let src: Vec<(TileIdx, &[u16], Option<&[u8]>)> = idxs
        .iter()
        .zip(&batch)
        .map(|(i, t)| (*i, &t[..], None))
        .collect();
    let rounds = B4_TILES.div_ceil(BATCH);

    // Warm the pipeline and the scratch buffers: the first job pays for
    // buffer creation, which a drag's second frame never does.
    let _ = r.run_tile_kernel(
        Kernel::Adjust(&adj),
        &TileJob {
            src: &src,
            out: &[],
        },
    );

    let mut dst = vec![0u16; TILE_LEN];
    let t0 = Instant::now();
    for _ in 0..rounds {
        for t in &batch {
            correct_tile(&mut dst, t, &adj, None);
        }
    }
    let cpu = t0.elapsed().as_secs_f64() * 1000.0;

    let t0 = Instant::now();
    for _ in 0..rounds {
        assert!(
            r.run_tile_kernel(
                Kernel::Adjust(&adj),
                &TileJob {
                    src: &src,
                    out: &[]
                }
            )
            .is_some(),
            "the seam declined mid-benchmark"
        );
    }
    let gpu = t0.elapsed().as_secs_f64() * 1000.0;

    println!(
        "\ntone curve, B4/600 full page ({B4_TILES} tiles, {rounds} × {BATCH}-tile batches)\n\
           cpu {cpu:8.1} ms\n  gpu {gpu:8.1} ms\n  ratio {:.2}× ({})",
        gpu / cpu,
        if gpu < cpu { "GPU wins" } else { "CPU wins" }
    );
}

#[test]
#[ignore = "wall-clock timing; run deliberately with --ignored --nocapture"]
fn timed_gaussian_full_width_band() {
    let Some(mut r) = renderer() else { return };
    let f = Filter::Gaussian { sigma: 8.0 };
    let passes = f.separable_passes().expect("separable");
    let px = noise(B4_W * BAND_ROWS * 4, 7);
    let scale = B4_H as f64 / BAND_ROWS as f64;

    // Warm-up on a small region so the pipeline is compiled and the scratch
    // buffers exist before the clock starts.
    let mut warm = vec![0u16; 64 * 64 * 4];
    let _ = r.run_region_kernel(Kernel::Separable(&passes), &mut warm, 64, 64);

    let mut cpu_buf = Raster {
        w: B4_W,
        h: BAND_ROWS,
        px: px.clone(),
    };
    let t0 = Instant::now();
    f.run(&mut cpu_buf, 0, 0);
    let cpu = t0.elapsed().as_secs_f64() * 1000.0;

    let mut gpu_buf = px.clone();
    let t0 = Instant::now();
    assert!(
        r.run_region_kernel(Kernel::Separable(&passes), &mut gpu_buf, B4_W, BAND_ROWS),
        "the seam declined mid-benchmark"
    );
    let gpu = t0.elapsed().as_secs_f64() * 1000.0;

    assert_eq!(gpu_buf, cpu_buf.px, "the benchmark's two paths disagreed");
    println!(
        "\ngaussian σ8, {B4_W}×{BAND_ROWS} band ({:.1} Mpx)\n\
           cpu {cpu:8.1} ms   (full B4/600 page ≈ {:.0} ms)\n\
           gpu {gpu:8.1} ms   (full B4/600 page ≈ {:.0} ms)\n\
           ratio {:.2}× ({})",
        (B4_W * BAND_ROWS) as f64 / 1.0e6,
        cpu * scale,
        gpu * scale,
        gpu / cpu,
        if gpu < cpu { "GPU wins" } else { "CPU wins" }
    );
}

/// The smear family at the largest region the seam will take it.
///
/// 2000² is 4.0 Mpx, just inside `REGION_CHUNK_PX` — which is the ceiling on
/// purpose: a smear's taps land anywhere in the region, so unlike the
/// separable chain it cannot be banded, and a whole B4 page (52 Mpx, 417 MB
/// of source) is not bindable on any adapter this project targets. This is
/// therefore the honest headline number: a big marquee, not a page.
#[test]
#[ignore = "wall-clock timing; run deliberately with --ignored --nocapture"]
fn timed_radial_smear_full_chunk() {
    let Some(mut r) = renderer() else { return };
    const W: usize = 2000;
    const H: usize = 2000;
    let f = Filter::RadialBlur { strength: 0.3 };
    let s = f.smear_samples(W, H).expect("a smear filter");
    let px = noise(W * H * 4, 11);

    let mut warm = vec![0u16; 64 * 64 * 4];
    let warm_s = f.smear_samples(64, 64).expect("a smear filter");
    let _ = r.run_region_kernel(Kernel::Smear(&warm_s), &mut warm, 64, 64);

    let mut cpu_buf = Raster {
        w: W,
        h: H,
        px: px.clone(),
    };
    let t0 = Instant::now();
    f.run(&mut cpu_buf, 0, 0);
    let cpu = t0.elapsed().as_secs_f64() * 1000.0;

    let mut gpu_buf = px.clone();
    let t0 = Instant::now();
    assert!(
        r.run_region_kernel(Kernel::Smear(&s), &mut gpu_buf, W, H),
        "the seam declined mid-benchmark"
    );
    let gpu = t0.elapsed().as_secs_f64() * 1000.0;

    let worst = gpu_buf
        .iter()
        .zip(&cpu_buf.px)
        .map(|(g, c)| (*g as i32 - *c as i32).abs())
        .max()
        .unwrap_or(0);
    println!(
        "\nradial blur k=0.3, {W}×{H} ({:.1} Mpx, {} samples)\n\
           cpu {cpu:8.1} ms\n  gpu {gpu:8.1} ms\n  ratio {:.2}× ({})  max delta {worst}",
        (W * H) as f64 / 1.0e6,
        s.mats.len(),
        gpu / cpu,
        if gpu < cpu { "GPU wins" } else { "CPU wins" }
    );
}

/// The measurement that decided the design: on a parameter drag, how much of
/// an uncached correction derive is the below-composite re-walk versus the
/// correction arithmetic? If the composite dominates, moving only
/// `correct_tile` to the GPU buys nothing, and the source cache
/// (`CorrDerived::src_stamp`) is the actual fix.
#[test]
#[ignore = "wall-clock timing; run deliberately with --ignored --nocapture"]
fn timed_where_a_param_drag_actually_spends_its_time() {
    use mn_core::Document;
    let adj = tone_curve();
    // A page big enough for the ratio to be stable but small enough to build
    // in a test: 40 × 40 tiles' worth of canvas with real art under it.
    let mut doc = Document::new(2560, 2560);
    for i in 0..3 {
        let li = if i == 0 { 0 } else { doc.add_layer("art") };
        doc.set_active(li);
        doc.begin_op();
        for ty in 0..40 {
            for tx in 0..40 {
                let idx = TileIdx::new(tx, ty);
                let d = doc.layers[li].tile_mut(idx).data_mut();
                let n = noise(TILE_LEN, (tx * 97 + ty * 31 + i * 7) as u32 + 1);
                d.copy_from_slice(&n);
            }
        }
        doc.end_op();
    }
    let ci = doc.add_correction_layer(adj, false);
    doc.refresh_derived(600);

    // A parameter change with the source cache in play — the shipped path.
    let bump = |doc: &mut Document, v: f32| {
        let mut pts = [[0.0f32; 2]; 8];
        pts[0] = [0.0, v];
        pts[1] = [0.35, 0.2];
        pts[2] = [0.7, 0.85];
        pts[3] = [1.0, 1.0];
        doc.layers[ci].kind =
            mn_core::doc::LayerKind::Correction(Adjust::ToneCurve { pts, n: 4 });
    };
    bump(&mut doc, 0.06);
    let t0 = Instant::now();
    doc.refresh_derived(600);
    let cached = t0.elapsed().as_secs_f64() * 1000.0;

    // The same change with the cache forcibly cold — a stroke below moves
    // every tile's key, so every source re-composites. That is what EVERY
    // drag tick cost before the stamp split.
    doc.set_active(0);
    doc.begin_op();
    for ty in 0..40 {
        for tx in 0..40 {
            doc.layers[0].tile_mut(TileIdx::new(tx, ty)).data_mut()[0] ^= 1;
        }
    }
    doc.end_op();
    bump(&mut doc, 0.07);
    let t0 = Instant::now();
    doc.refresh_derived(600);
    let cold = t0.elapsed().as_secs_f64() * 1000.0;

    println!(
        "\nparam drag on a 2560² page, 3 art layers, 1600 derived tiles\n\
         \x20 sources cached (shipped)  {cached:8.1} ms\n\
         \x20 sources re-composited     {cold:8.1} ms\n\
         \x20 the composite is {:.0}% of an uncached derive",
        (1.0 - cached / cold) * 100.0
    );
    let _ = TILE_PIXELS;
}

/// `FI-050`'s full-page cost, CPU reference against the seam.
///
/// The whole page for real: `Document::paint_gradient_freeform_with` walks
/// its own 256-tile batches, so only 8 MB of pixels is in flight at a time
/// and the number is a measurement rather than an extrapolation. Two guides
/// of 40 segments is the realistic end (a hand-drawn guide simplified at 2
/// screen px is a few dozen points); 200 each is the pathological one, a
/// deliberately shaky line drawn zoomed in, and it is where the distance
/// field dominates.
///
/// The layer is allocated once per run and dropped before the next, because
/// a B4 fix15 layer is 418 MB and this laptop shares its RAM with the GPU.
#[test]
#[ignore = "wall-clock timing; run deliberately with --ignored --nocapture"]
fn timed_freeform_full_page() {
    let Some(mut r) = renderer() else { return };
    let (w, h) = (6071u32, 8598u32);
    let guide = |x: f32, n: usize| -> Vec<[f32; 2]> {
        (0..=n)
            .map(|i| {
                let y = i as f32 * (h as f32 / n as f32);
                [x + (i as f32 * 60.0 / n as f32).sin() * 120.0, y]
            })
            .collect()
    };
    let ramp = mn_core::Ramp::two([1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]);
    println!(
        "B4/600 = {w}x{h} = {:.1} Mpx, freeform gradient",
        (w as f64 * h as f64) / 1e6
    );

    // The seam pays for its buffers on the first batch and never again; a
    // real page's second batch does not, so warm them before the clock.
    {
        let mut warm = mn_core::Document::new(512, 512);
        let (a, b) = (guide(120.0, 8), guide(390.0, 8));
        warm.paint_gradient_freeform_with(&a, &b, &ramp, &mut |job, px| {
            r.run_freeform_kernel(job, px)
        });
    }

    for n in [40usize, 200] {
        let (l1, l2) = (guide(1200.0, n), guide(4800.0, n));
        let cpu = {
            let mut doc = mn_core::Document::new(w, h);
            let t = Instant::now();
            assert!(doc.paint_gradient_freeform(&l1, &l2, &ramp));
            t.elapsed()
        };
        let (gpu, batches, ran) = {
            let mut doc = mn_core::Document::new(w, h);
            let (mut batches, mut ran) = (0usize, 0usize);
            let t = Instant::now();
            assert!(
                doc.paint_gradient_freeform_with(&l1, &l2, &ramp, &mut |job, px| {
                    batches += 1;
                    let out = r.run_freeform_kernel(job, px);
                    ran += usize::from(out.is_some());
                    out
                })
            );
            (t.elapsed(), batches, ran)
        };
        assert_eq!(ran, batches, "the seam declined mid-benchmark");
        println!(
            "  {} guide segments: cpu {:?}  gpu {:?}  = {:.2}x  ({batches} batches)",
            n * 2,
            cpu,
            gpu,
            cpu.as_secs_f64() / gpu.as_secs_f64(),
        );
    }
}
