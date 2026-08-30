//! The shared tile-kernel seam (`mn_gpu::kernel`) against its CPU reference.
//!
//! The CPU is the specification: `correct_tile` + `Adjust::map` for the
//! colour family, `Filter::run`'s box passes for the blur family. Every test
//! here runs both and compares.
//!
//! # The tolerances, and why they are what they are
//!
//! **Colour ops: ≤ 2 fix15 units per channel** (out of 32768 — 0.006 %, far
//! under one step of the 8-bit page these tiles came from). Both sides do
//! the same arithmetic in f32 and quantise with the same `min(v*32768 + 0.5,
//! 32768)`, so the gap is float divergence only: WGSL does not promise
//! IEEE-identical `pow` (Levels' gamma) or division, and a value landing
//! within one ULP of an `x.5` boundary can round the other way. Two is the
//! bar; the measured worst case on the Intel UHD 620 / DX12 is **1** for
//! Brightness/Contrast, Hue/Saturation, Levels, Tone curve, Colour balance
//! and Gradient map, and **0** for Invert, Binarize and Posterize.
//!
//! **Quantising ops (Posterize, Binarize) additionally allow bucket-boundary
//! flips.** These are step functions: a source value one ULP either side of
//! a bucket edge is *supposed* to produce two completely different outputs,
//! so a float difference of 1e-7 can legitimately show up as a delta of
//! 32768. That is not a parity failure, it is the operator being
//! discontinuous, and clamping the tolerance instead of naming the effect
//! would have hidden it. Measured: 2 flipped pixels out of 16 384 for
//! Posterize, 0 for Binarize.
//!
//! **Gaussian / Smoothing / Blur: exact, no tolerance.** `BoxPass` carries
//! integer weights and an integer denominator, so both sides accumulate the
//! same u32 sums and round with the same `(acc + denom/2) / denom`. Measured
//! max delta: 0. (This is not where the design started — see
//! `Filter::separable_passes` for the 4015-unit mistake that got it here.)
//!
//! These tests SKIP rather than fail when no adapter can be created, and
//! serialise on a GPU lock — same policy as `composite.rs` and
//! `dab_parity.rs`.

use mn_core::adjust::{Adjust, correct_tile};
use mn_core::tile::{TILE_LEN, TILE_PIXELS, TILE_SIZE, TileIdx};
use mn_core::{Filter, Raster};
use mn_gpu::{GpuConfig, Kernel, Renderer, TileJob};

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
            if !r.kernels_supported() {
                println!("[test] SKIP: adapter has no compute shaders");
                return None;
            }
            Some(r)
        }
        Err(e) => {
            println!("[test] SKIP: no usable adapter ({e})");
            None
        }
    }
}

/// A deterministic tile of premultiplied fix15 RGBA covering the awkward
/// cases on purpose: fully transparent pixels (which `correct_tile` passes
/// through untouched), partial alpha (where forgetting to unpremultiply
/// shows), pure black, pure white, and saturated primaries.
fn tile(seed: u32) -> Vec<u16> {
    let mut px = vec![0u16; TILE_LEN];
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(12345);
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    };
    for p in 0..TILE_PIXELS {
        let o = p * 4;
        let a = match p % 8 {
            0 => 0u32,           // transparent: the pass-through branch
            1 => 32768,          // opaque
            2 => 1,              // the extreme of the unpremultiply divide
            _ => next() % 32769, // everything in between
        };
        if a == 0 {
            continue;
        }
        for c in 0..3 {
            // Premultiplied, so a channel can never exceed alpha.
            px[o + c] = (next() % (a + 1)) as u16;
        }
        px[o + 3] = a as u16;
    }
    px
}

/// A coverage mask with the three interesting values: 0 (untouched), 255
/// (fully corrected) and everything between (the blend branch).
fn coverage(seed: u32) -> Vec<u8> {
    (0..TILE_PIXELS)
        .map(|p| match (p + seed as usize) % 5 {
            0 => 0,
            1 => 255,
            n => ((p * 37 + n * 51) % 256) as u8,
        })
        .collect()
}

/// Every `Adjust` variant, with parameters that actually move pixels.
fn variants() -> Vec<Adjust> {
    vec![
        Adjust::BrightnessContrast {
            brightness: 0.12,
            contrast: 0.35,
        },
        Adjust::HueSaturation {
            hue: 47.0,
            saturation: 0.4,
            luminosity: -0.2,
        },
        Adjust::Posterize { levels: 6 },
        Adjust::Invert,
        Adjust::Binarize { threshold: 0.42 },
        Adjust::Levels {
            in_black: 0.1,
            in_white: 0.85,
            gamma: 1.7,
            out_black: 0.05,
            out_white: 0.95,
        },
        Adjust::ToneCurve {
            pts: {
                let mut p = [[0.0f32; 2]; 8];
                p[0] = [0.0, 0.08];
                p[1] = [0.3, 0.15];
                p[2] = [0.62, 0.8];
                p[3] = [1.0, 1.0];
                p
            },
            n: 4,
        },
        Adjust::ColourBalance {
            cyan_red: 0.2,
            magenta_green: -0.15,
            yellow_blue: 0.3,
        },
        Adjust::GradientMap {
            stops: {
                let mut s = [[0.0f32; 5]; 8];
                s[0] = [0.0, 0.1, 0.0, 0.25, 0.0];
                s[1] = [0.45, 0.9, 0.35, 0.1, 0.0];
                s[2] = [1.0, 1.0, 1.0, 0.7, 0.0];
                s
            },
            n: 3,
        },
    ]
}

/// True for the operators that are step functions of the input, where a
/// float difference of one ULP legitimately produces a whole bucket's
/// difference in the output (see the module docs).
fn quantising(adj: &Adjust) -> bool {
    matches!(adj, Adjust::Posterize { .. } | Adjust::Binarize { .. })
}

#[test]
fn adjust_parity_every_variant() {
    let _g = gpu_guard();
    let Some(mut r) = renderer() else { return };
    let src: Vec<Vec<u16>> = (0..4).map(tile).collect();
    let idxs: Vec<TileIdx> = (0..4).map(|i| TileIdx::new(i, 0)).collect();

    for adj in variants() {
        let job_src: Vec<(TileIdx, &[u16], Option<&[u8]>)> = idxs
            .iter()
            .zip(&src)
            .map(|(i, t)| (*i, &t[..], None))
            .collect();
        let got = r
            .run_tile_kernel(
                Kernel::Adjust(&adj),
                &TileJob {
                    src: &job_src,
                    out: &[],
                },
            )
            .unwrap_or_else(|| panic!("{} declined on a supported adapter", adj.label()));
        assert_eq!(got.len(), src.len());

        let (mut worst, mut flips) = (0i32, 0usize);
        for (gpu, cpu_src) in got.iter().zip(&src) {
            let mut want = vec![0u16; TILE_LEN];
            correct_tile(&mut want, cpu_src, &adj, None);
            for p in 0..TILE_PIXELS {
                let d = (0..4)
                    .map(|c| (gpu[p * 4 + c] as i32 - want[p * 4 + c] as i32).abs())
                    .max()
                    .unwrap_or(0);
                if d > 2 && quantising(&adj) {
                    flips += 1;
                } else {
                    worst = worst.max(d);
                }
            }
        }
        println!("[parity] {:<26} max delta {worst}  flips {flips}", adj.label());
        assert!(
            worst <= 2,
            "{}: {worst} fix15 units off the CPU reference",
            adj.label()
        );
        // A step function may disagree at bucket edges, but only there: if
        // this fired the op would be wrong, not merely discontinuous.
        assert!(
            flips * 200 < src.len() * TILE_PIXELS,
            "{}: {flips} bucket-boundary flips is too many to be float noise",
            adj.label()
        );
    }
}

#[test]
fn adjust_parity_through_a_window_mask() {
    let _g = gpu_guard();
    let Some(mut r) = renderer() else { return };
    let src: Vec<Vec<u16>> = (0..3).map(tile).collect();
    let cov: Vec<Vec<u8>> = (0..3).map(coverage).collect();
    let adj = Adjust::BrightnessContrast {
        brightness: -0.2,
        contrast: 0.5,
    };
    let job_src: Vec<(TileIdx, &[u16], Option<&[u8]>)> = (0..3)
        .map(|i| (TileIdx::new(i as i32, 0), &src[i][..], Some(&cov[i][..])))
        .collect();
    let got = r
        .run_tile_kernel(
            Kernel::Adjust(&adj),
            &TileJob {
                src: &job_src,
                out: &[],
            },
        )
        .expect("windowed batch runs");

    let mut worst = 0i32;
    let mut zero_cov_seen = 0;
    for (n, gpu) in got.iter().enumerate() {
        let mut want = vec![0u16; TILE_LEN];
        let c: &[u8; TILE_PIXELS] = cov[n][..].try_into().unwrap();
        correct_tile(&mut want, &src[n], &adj, Some(c));
        for p in 0..TILE_PIXELS {
            for ch in 0..4 {
                worst = worst.max((gpu[p * 4 + ch] as i32 - want[p * 4 + ch] as i32).abs());
            }
            // Coverage 0 must be byte-identical to the source — that branch
            // is what keeps pixels outside a window untouched.
            if cov[n][p] == 0 {
                zero_cov_seen += 1;
                assert_eq!(
                    &gpu[p * 4..p * 4 + 4],
                    &src[n][p * 4..p * 4 + 4],
                    "an uncovered pixel was corrected"
                );
            }
        }
    }
    println!("[parity] windowed max delta {worst} over {zero_cov_seen} uncovered pixels");
    assert!(zero_cov_seen > 100, "the mask never exercised coverage 0");
    assert!(worst <= 2, "windowed correction {worst} units off");
}

/// A raster with ink, holes and edges — the halo convention (outside is
/// transparent) has to survive at all four borders.
fn raster(w: usize, h: usize) -> Raster {
    let mut r = Raster::new(w, h);
    let mut s = 987654321u32;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    };
    for y in 0..h {
        for x in 0..w {
            // Blobs of solid ink on a transparent field: a blur's edges are
            // where the two conventions have to agree.
            let solid = ((x / 7) + (y / 5)) % 3 == 0;
            let a = if solid { 32768u32 } else { next() % 4096 };
            if a == 0 {
                continue;
            }
            let px = [
                (next() % (a + 1)) as u16,
                (next() % (a + 1)) as u16,
                (next() % (a + 1)) as u16,
                a as u16,
            ];
            r.set_pixel(x, y, px);
        }
    }
    r
}

/// The blur family has NO tolerance: `BoxPass` carries integer weights and
/// an integer denominator, so both sides compute the same u32 sums and the
/// same `(acc + denom/2) / denom`. Equality is the assertion.
fn separable_parity(f: Filter, w: usize, h: usize) {
    separable_parity_chunked(f, w, h, None)
}

fn separable_parity_chunked(f: Filter, w: usize, h: usize, chunk: Option<usize>) {
    let _g = gpu_guard();
    let Some(mut r) = renderer() else { return };
    r.debug_region_chunk_px(chunk);
    let base = raster(w, h);
    let mut want = base.clone();
    f.run(&mut want, 0, 0);

    let passes = f.separable_passes().expect("a separable filter");
    let mut got = base.clone();
    assert!(
        r.run_region_kernel(Kernel::Separable(&passes), &mut got.px, w, h),
        "{} declined on a supported adapter",
        f.label()
    );

    let worst = got
        .px
        .iter()
        .zip(&want.px)
        .map(|(g, c)| (*g as i32 - *c as i32).abs())
        .max()
        .unwrap_or(0);
    println!("[parity] {:<26} max delta {worst}", f.label());
    assert_eq!(
        worst,
        0,
        "{}: {worst} fix15 units off the box passes — integer arithmetic on \
         both sides means any difference is a real disagreement, not rounding",
        f.label()
    );
}

#[test]
fn gaussian_parity_matches_the_box_passes() {
    separable_parity(Filter::Gaussian { sigma: 4.0 }, 200, 160);
}

#[test]
fn a_wide_gaussian_still_matches() {
    // σ 12 is a 3-box reach of ~26 px: the halo bands and the tap loop get a
    // workout σ 4 does not give them.
    separable_parity(Filter::Gaussian { sigma: 12.0 }, 192, 192);
}

#[test]
fn smoothing_parity_matches_the_tent() {
    separable_parity(Filter::Smoothing, 130, 90);
}

#[test]
fn blur_presets_route_through_the_same_kernel() {
    separable_parity(Filter::Blur, 128, 96);
    separable_parity(Filter::BlurStrong, 128, 96);
}

/// The band chunker has to produce the same pixels as an unchunked run.
/// At any size the suite can afford, the real 4 Mpx chunk is one band and
/// the halo bookkeeping never runs — so the chunk is forced down until the
/// raster splits several ways. σ 16 gives a reach of ~44, which is a large
/// fraction of each band: exactly where an off-by-one would show as a seam.
#[test]
fn banded_chunking_leaves_no_seams() {
    let f = Filter::Gaussian { sigma: 16.0 };
    let reach: usize = f
        .separable_passes()
        .unwrap()
        .iter()
        .map(|p| p.half.len() - 1)
        .sum();
    // 128 wide, so this budget is ~(reach*2 + 60) rows per band over 640
    // rows — around eight bands, with halos overlapping heavily.
    let budget = 128 * (2 * reach + 60);
    separable_parity_chunked(f, 128, 640, Some(budget));
}

/// A band budget too small to fit even one output row must DECLINE, not
/// silently produce a short or wrong result.
#[test]
fn an_impossible_band_budget_declines() {
    let _g = gpu_guard();
    let Some(mut r) = renderer() else { return };
    let f = Filter::Gaussian { sigma: 16.0 };
    let passes = f.separable_passes().unwrap();
    let mut px = raster(128, 200);
    let before = px.clone();
    r.debug_region_chunk_px(Some(128)); // one row of budget, halo needs many
    assert!(!r.run_region_kernel(Kernel::Separable(&passes), &mut px.px, 128, 200));
    assert_eq!(px, before, "a declined job wrote through the caller's buffer");
}

#[test]
fn a_dropped_dispatch_declines_and_leaves_the_caller_untouched() {
    let _g = gpu_guard();
    let Some(mut r) = renderer() else { return };

    // Colour family: the canary fails, the job declines, nothing comes back.
    let src = tile(9);
    let job_src = [(TileIdx::new(0, 0), &src[..], None)];
    let adj = Adjust::Invert;
    r.debug_fail_next_kernel();
    assert!(
        r.run_tile_kernel(
            Kernel::Adjust(&adj),
            &TileJob {
                src: &job_src,
                out: &[],
            }
        )
        .is_none(),
        "a dropped dispatch must decline, not hand back a torn tile"
    );
    // And the very next job succeeds: the failure left no state behind.
    assert!(
        r.run_tile_kernel(
            Kernel::Adjust(&adj),
            &TileJob {
                src: &job_src,
                out: &[],
            }
        )
        .is_some(),
        "the seam did not recover after a simulated driver drop"
    );

    // Blur family: the same, and the caller's pixels must be BYTE-identical
    // afterwards — `apply_filter_with` runs the CPU reference over them.
    let base = raster(96, 96);
    let mut buf = base.clone();
    let passes = Filter::Gaussian { sigma: 3.0 }
        .separable_passes()
        .expect("separable");
    r.debug_fail_next_kernel();
    assert!(
        !r.run_region_kernel(Kernel::Separable(&passes), &mut buf.px, 96, 96),
        "a dropped dispatch must decline"
    );
    assert_eq!(
        buf, base,
        "a declined region job wrote through the caller's buffer — the CPU \
         fallback would then filter already-filtered pixels"
    );
}

#[test]
fn a_tile_map_separable_agrees_with_the_region_path() {
    let _g = gpu_guard();
    let Some(mut r) = renderer() else { return };
    // A 3×2 block of tiles assembled from one region, so the two entry
    // points can be compared pixel for pixel.
    let (tw, th) = (3usize, 2usize);
    let (w, h) = (tw * TILE_SIZE, th * TILE_SIZE);
    let region = raster(w, h);
    let tiles: Vec<Vec<u16>> = (0..tw * th)
        .map(|n| {
            let (cx, cy) = ((n % tw) * TILE_SIZE, (n / tw) * TILE_SIZE);
            let mut t = vec![0u16; TILE_LEN];
            for row in 0..TILE_SIZE {
                let s = ((cy + row) * w + cx) * 4;
                let d = row * TILE_SIZE * 4;
                t[d..d + TILE_SIZE * 4].copy_from_slice(&region.px[s..s + TILE_SIZE * 4]);
            }
            t
        })
        .collect();
    let job_src: Vec<(TileIdx, &[u16], Option<&[u8]>)> = (0..tw * th)
        .map(|n| {
            (
                TileIdx::new((n % tw) as i32, (n / tw) as i32),
                &tiles[n][..],
                None,
            )
        })
        .collect();

    let passes = Filter::Gaussian { sigma: 3.0 }
        .separable_passes()
        .expect("separable");
    let got = r
        .run_tile_kernel(
            Kernel::Separable(&passes),
            &TileJob {
                src: &job_src,
                out: &[],
            },
        )
        .expect("tile-map separable runs");

    let mut want = region.clone();
    assert!(r.run_region_kernel(Kernel::Separable(&passes), &mut want.px, w, h));
    for (n, t) in got.iter().enumerate() {
        let (cx, cy) = ((n % tw) * TILE_SIZE, (n / tw) * TILE_SIZE);
        for row in 0..TILE_SIZE {
            let s = ((cy + row) * w + cx) * 4;
            let d = row * TILE_SIZE * 4;
            assert_eq!(
                &t[d..d + TILE_SIZE * 4],
                &want.px[s..s + TILE_SIZE * 4],
                "tile {n} row {row} disagrees with the region path"
            );
        }
    }
}

/// The smear family (`Kernel::Smear`) against `Filter::run`.
///
/// **A tolerance, not equality, and the reason is the reverse of the box
/// passes'.** `BoxPass` is integers all the way down, so those two sides
/// produce the same bits. A smear averages `n` BILINEAR taps in f32, and the
/// two things that could have made it much worse are already gone from the
/// shader: the sample matrices are built on the host, so nothing here
/// evaluates `sin`/`cos` (WGSL promises them only to 2⁻¹¹ absolute, which at
/// a page's radius is a large fraction of a pixel of drift), and the tap
/// weights are applied in the reference's own association (`(p·wx)·wy`, four
/// corners summed before joining the accumulator).
///
/// What is left is sub-ULP: the shader may contract `c + m·u` into an FMA
/// where the Rust does not, which moves a sample position by ~1e-7 px and
/// therefore a tap weight by ~1e-7. Against a fix15 range of 32768 that is
/// ~0.003 of a unit — visible only when the final `+ 0.5` was already sitting
/// on a rounding boundary, i.e. ±1. Two is the bar, the same one the colour
/// ops carry for the same class of reason.
///
/// Measured, and the two adapters make the argument for it: **max delta 1**
/// on the Intel UHD 620 / DX12, on ~0.4 % of channels, and **max delta 0** —
/// exact — on WARP, which does not contract. A transcription error would not
/// have that shape; it would be wrong on both.
fn smear_parity(f: Filter, w: usize, h: usize) {
    let _g = gpu_guard();
    let Some(mut r) = renderer() else { return };
    let base = raster(w, h);
    let mut want = base.clone();
    f.run(&mut want, 0, 0);

    let s = f.smear_samples(w, h).expect("a smear filter");
    let mut got = base.clone();
    assert!(
        r.run_region_kernel(Kernel::Smear(&s), &mut got.px, w, h),
        "{} declined on a supported adapter",
        f.label()
    );

    let worst = got
        .px
        .iter()
        .zip(&want.px)
        .map(|(g, c)| (*g as i32 - *c as i32).abs())
        .max()
        .unwrap_or(0);
    let off = got
        .px
        .iter()
        .zip(&want.px)
        .filter(|(g, c)| g != c)
        .count();
    println!(
        "[parity] {:<26} max delta {worst}  ({off} of {} channels differ)",
        f.label(),
        got.px.len()
    );
    assert!(
        worst <= 2,
        "{}: {worst} fix15 units off the CPU smear — f32 contraction is worth \
         ±1, not this",
        f.label()
    );
    // …and the result has to BE a smear, not a copy that trivially matches
    // to within the tolerance.
    assert_ne!(got.px, base.px, "{} did nothing", f.label());
}

#[test]
fn radial_blur_parity_matches_the_cpu_smear() {
    smear_parity(Filter::RadialBlur { strength: 0.5 }, 200, 160);
}

#[test]
fn spin_blur_parity_matches_the_cpu_smear() {
    // 30° at this size is 48 samples — the clamp's top end, so the tap loop
    // runs as long as it ever does.
    smear_parity(Filter::SpinBlur { angle_deg: 30.0 }, 192, 192);
}

/// A gentle smear is the case where the taps pile up near the pixel itself
/// and the `+ 0.5` sits on a boundary most often — the worst case for the
/// tolerance, and the one a real page mostly asks for.
#[test]
fn a_gentle_smear_stays_inside_the_tolerance() {
    smear_parity(Filter::RadialBlur { strength: 0.08 }, 150, 130);
    smear_parity(Filter::SpinBlur { angle_deg: 2.0 }, 150, 130);
}

/// A smear cannot be banded — its taps land anywhere in the region — so a
/// region past the chunk ceiling must DECLINE rather than produce a
/// per-band answer that would be wrong everywhere the taps crossed a band.
#[test]
fn a_smear_past_the_chunk_ceiling_declines() {
    let _g = gpu_guard();
    let Some(mut r) = renderer() else { return };
    let f = Filter::RadialBlur { strength: 0.5 };
    let s = f.smear_samples(128, 200).expect("a smear filter");
    let mut px = raster(128, 200);
    let before = px.clone();
    r.debug_region_chunk_px(Some(1024)); // 25 600 px of region, 1 024 of budget
    assert!(!r.run_region_kernel(Kernel::Smear(&s), &mut px.px, 128, 200));
    assert_eq!(px, before, "a declined smear wrote through the caller's buffer");
    // The ceiling is the only reason it declined: restore it and the same
    // job runs.
    r.debug_region_chunk_px(None);
    assert!(r.run_region_kernel(Kernel::Smear(&s), &mut px.px, 128, 200));
}

/// FL-014 end to end: an unsharp applied with the GPU seam lent to
/// `apply_filter_with` must be the same PAGE as `apply_filter`'s. The seam
/// never sees the unsharp — it sees the Gaussian half, byte-identical by the
/// box-pass argument — and `filter::run_split` does the combine, so this is
/// the pin that the split did not change the operator.
#[test]
fn the_unsharp_split_is_byte_identical_through_the_gpu_blur() {
    let _g = gpu_guard();
    let Some(mut r) = renderer() else { return };
    let f = Filter::Unsharp {
        radius: 4.0,
        amount: 1.5,
    };
    let page = || {
        let mut doc = mn_core::Document::new(256, 256);
        let mut s = 24680u32;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s
        };
        for y in 40..200i32 {
            for x in 30..210i32 {
                let idx = TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                let o = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
                let a = if (x / 9 + y / 6) % 3 == 0 {
                    32768
                } else {
                    next() % 4096
                };
                let d = doc.layers[0].tile_mut(idx).data_mut();
                for c in 0..3 {
                    d[o + c] = (next() % (a + 1)) as u16;
                }
                d[o + 3] = a as u16;
            }
        }
        doc
    };

    let mut want = page();
    assert!(want.apply_filter(f));

    let mut halves = 0usize;
    let mut got = page();
    assert!(got.apply_filter_with(f, &mut |g, buf| {
        let Some(passes) = g.separable_passes() else {
            return false;
        };
        halves += 1;
        r.run_region_kernel(Kernel::Separable(&passes), &mut buf.px, buf.w, buf.h)
    }));
    assert_eq!(halves, 1, "the blur half was never offered to the seam");

    let a = mn_core::export::composite(&want, mn_core::export::Background::White);
    let b = mn_core::export::composite(&got, mn_core::export::Background::White);
    assert!(
        a.pixels().zip(b.pixels()).all(|(p, q)| p.0 == q.0),
        "the GPU-blurred unsharp is a different page from the reference"
    );
}

#[test]
fn routing_declines_software_adapters_and_small_jobs() {
    let _g = gpu_guard();
    let Some(r) = renderer() else { return };
    // Below the floor is always CPU, on every adapter.
    assert!(!r.kernels_preferred(mn_gpu::KERNEL_FLOOR_PX - 1));
    let software = r.adapter_is_software();
    assert_eq!(
        r.kernels_preferred(mn_gpu::KERNEL_FLOOR_PX),
        !software,
        "a software adapter must route CPU even above the floor"
    );
}
