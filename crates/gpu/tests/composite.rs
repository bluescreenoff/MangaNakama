//! GPU compositor vs CPU compositor: the two must agree.
//!
//! `mn_core::blend` (CPU, exact, used by PNG/ORA export) and the fixed-function
//! blend states in `mn_gpu` (display) implement the same three equations. This
//! renders synthetic documents both ways and compares pixels, so the two can
//! never drift apart silently.
//!
//! **These tests skip, not fail, when no adapter can be created.** CI boxes and
//! remote sessions may have neither hardware DX12 nor WARP; a missing GPU is not
//! a broken compositor. On this laptop both DX12 and WARP work, so they run.

use mn_core::export::{self, Background};
use mn_core::{Blend, Document, TILE_SIZE, TileIdx};
use mn_gpu::{GpuConfig, Renderer};

/// Serialises the GPU tests.
///
/// Not paranoia: creating several DX12 **WARP** devices from parallel threads
/// reliably crashes the process with STATUS_ACCESS_VIOLATION inside the software
/// rasteriser (reproduced 2026-08-13; `MN_WARP=1 cargo test` dies, the same run
/// with `--test-threads=1` passes, and hardware DX12 is fine either way). One
/// device at a time costs a second and removes the whole failure mode.
static GPU_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_guard() -> std::sync::MutexGuard<'static, ()> {
    // A panicking test poisons the lock; the next test does not care.
    GPU_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// `None` means "no GPU here" — the caller prints a skip and returns.
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

/// Fill a tile with one straight colour at one alpha (premultiplied on write).
fn fill(doc: &mut Document, layer: usize, idx: TileIdx, rgba: [f32; 4]) {
    let px = [
        (rgba[0] * rgba[3] * 32768.0).round() as u16,
        (rgba[1] * rgba[3] * 32768.0).round() as u16,
        (rgba[2] * rgba[3] * 32768.0).round() as u16,
        (rgba[3] * 32768.0).round() as u16,
    ];
    let tile = doc.layers[layer].tile_mut(idx);
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            tile.set_pixel(x, y, px);
        }
    }
}

/// A gradient tile: alpha ramps across x, colour across y. Exercises partial
/// coverage, which is where a wrong blend factor shows up.
fn fill_ramp(doc: &mut Document, layer: usize, idx: TileIdx, tint: [f32; 3]) {
    let tile = doc.layers[layer].tile_mut(idx);
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            let a = x as f32 / (TILE_SIZE - 1) as f32;
            let k = y as f32 / (TILE_SIZE - 1) as f32;
            let px = [
                (tint[0] * k * a * 32768.0).round() as u16,
                (tint[1] * k * a * 32768.0).round() as u16,
                (tint[2] * k * a * 32768.0).round() as u16,
                (a * 32768.0).round() as u16,
            ];
            tile.set_pixel(x, y, px);
        }
    }
}

/// Worst channel delta between a GPU render and the CPU composite, plus the
/// number of PIXELS with any channel past `tol` (see
/// `assert_agrees_tol_outliers` for what that count is for).
fn disagreement(
    r: &mut Renderer,
    doc: &Document,
    tol: i32,
) -> (i32, (u32, u32), usize, image::RgbaImage) {
    let (w, h) = doc.size;
    let gpu = r.render_offscreen(doc, w, h);
    let cpu = export::composite(doc, Background::White);
    assert_eq!(gpu.dimensions(), cpu.dimensions(), "size");
    let mut worst = 0i32;
    let mut worst_at = (0u32, 0u32);
    let mut over = 0usize;
    for (x, y, p) in gpu.enumerate_pixels() {
        let q = cpu.get_pixel(x, y);
        let mut px_worst = 0i32;
        for c in 0..4 {
            let d = (p.0[c] as i32 - q.0[c] as i32).abs();
            px_worst = px_worst.max(d);
            if d > worst {
                worst = d;
                worst_at = (x, y);
            }
        }
        if px_worst > tol {
            over += 1;
        }
    }
    (worst, worst_at, over, gpu)
}

/// Compare a GPU render against the CPU composite of the same document.
///
/// Tolerance is 2/255: the GPU divides fix15 by 32768 in f32, blends in the
/// render target's precision and rounds to unorm8; the CPU does it all in f32
/// and rounds once. Nothing structural hides under 2 levels — a wrong blend mode
/// or a dropped opacity is off by tens.
///
/// When the HARDWARE adapter disagrees, the same document is re-checked on
/// WARP (the reference software rasteriser) before failing: this laptop's
/// 2020 Intel UHD 620 DX12 driver intermittently drops one draw from a
/// rebuild frame (reproduced 2026-08-14 on the ROUND-10 code too — a flat
/// stack, render → add layer → render loses the new layer's tile in patches;
/// WARP is exact in every case, and the flake predates the folder work).
/// A real compositor bug fails on both and still fails the test.
fn assert_agrees(r: &mut Renderer, doc: &Document, label: &str) {
    assert_agrees_tol(r, doc, label, 2);
}

fn assert_agrees_tol(r: &mut Renderer, doc: &Document, label: &str, tol: i32) {
    assert_agrees_tol_outliers(r, doc, label, tol, 0);
}

/// As `assert_agrees_tol`, but forgiving up to `max_over` individual PIXELS.
///
/// Only the STEP modes need this. Hard mix, Darker color and Lighter color
/// are discontinuous by definition, and the GPU blends against an 8-bit
/// snapshot of the canvas while the CPU reference uses full precision — so
/// pixels sitting within half a snapshot LSB of the step land on opposite
/// sides of it and disagree by the full 255. That is a rounding artefact of
/// the display path, not a formula difference (export is exact, it never
/// takes the snapshot path), and it is confined to a thin band. A real bug
/// in one of these modes is wrong over half the canvas and still fails.
fn assert_agrees_tol_outliers(
    r: &mut Renderer,
    doc: &Document,
    label: &str,
    tol: i32,
    max_over: usize,
) {
    let (worst, at, over, gpu) = disagreement(r, doc, tol);
    if over <= max_over {
        println!("[test] {label}: max channel delta {worst}, {over} pixel(s) over {tol}");
        return;
    }
    println!(
        "[test] {label}: hardware delta {worst} at {at:?} over {over} pixels (known Intel DX12 draw-drop flake) — verifying on WARP"
    );
    let cpu = export::composite(doc, Background::White);
    let mut warp = match Renderer::new_headless(GpuConfig {
        force_fallback: true,
        no_vsync: false,
    }) {
        Ok(w) => w,
        Err(e) => panic!(
            "{label}: hw disagrees by {worst} at {at:?} over {over} pixels (gpu {:?} cpu {:?}) and no WARP to verify ({e})",
            gpu.get_pixel(at.0, at.1).0,
            cpu.get_pixel(at.0, at.1).0
        ),
    };
    let (w2, at2, over2, gpu2) = disagreement(&mut warp, doc, tol);
    if over2 > max_over {
        panic!(
            "{label}: WARP and cpu disagree by {w2} at {at2:?} over {over2} pixels (budget {max_over}): gpu {:?} cpu {:?}",
            gpu2.get_pixel(at2.0, at2.1).0,
            cpu.get_pixel(at2.0, at2.1).0
        );
    }
    println!("[test] {label}: WARP agrees ({over2} pixels over {tol}); hardware flake logged");
}

#[test]
fn cpu_matches_gpu_for_every_blend_mode() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    for mode in [
        Blend::Normal,
        Blend::Multiply,
        Blend::Screen,
        Blend::Add,
        Blend::Subtract,
        // Blend part 2: the shader compositor path (snapshot + blend2.wgsl)
        // must agree with core::blend exactly like the fixed-function five.
        Blend::Darken,
        Blend::Lighten,
        Blend::Overlay,
        Blend::SoftLight,
        Blend::HardLight,
        Blend::Difference,
        Blend::Exclusion,
        Blend::Hue,
        Blend::Saturation,
        Blend::Color,
    ] {
        // Bottom: opaque colour field. Top: the same field in the other mode.
        let mut doc = Document::new(128, 128);
        fill(&mut doc, 0, TileIdx::new(0, 0), [0.9, 0.2, 0.2, 1.0]);
        fill(&mut doc, 0, TileIdx::new(1, 0), [0.2, 0.6, 0.9, 1.0]);
        fill(&mut doc, 0, TileIdx::new(0, 1), [1.0, 1.0, 1.0, 1.0]);
        fill(&mut doc, 0, TileIdx::new(1, 1), [0.1, 0.1, 0.1, 1.0]);

        doc.add_layer("blended");
        doc.set_layer_blend(1, mode);
        for ty in 0..2 {
            for tx in 0..2 {
                fill_ramp(&mut doc, 1, TileIdx::new(tx, ty), [0.3, 0.8, 0.5]);
            }
        }
        // The blend2 shader modes blend against the 8-BIT canvas snapshot
        // while the CPU reference uses full-precision dst — soft-light's
        // slope (~6x at the top) amplifies the 1/2-LSB rounding to ~3/255.
        // Display-path approximation only (ARCHITECTURE.md); export is exact.
        let tol = matches!(
            mode,
            Blend::SoftLight
                | Blend::Overlay
                | Blend::HardLight
                | Blend::Color
                | Blend::Hue
                | Blend::Saturation
                | Blend::Darken
                | Blend::Lighten
                | Blend::Difference
                | Blend::Exclusion
        )
        .then_some(4)
        .unwrap_or(2);
        assert_agrees_tol(&mut r, &doc, &format!("{mode:?}"), tol);
    }
}

/// The part-3 dodge/burn/light family, same pin as above with one change
/// that matters: **the base colours are exact 8-bit values.**
///
/// The whole error budget on this path is the destination snapshot — the GPU
/// blends against an `Rgba8Unorm` copy of the canvas, the CPU reference
/// against full-precision f32. With a base like 0.9 those differ by ~2/255,
/// and this family multiplies that error by its slope: colour dodge's is
/// `1/(1 - Cs)`, vivid light's `1/(1 - Cb)`, and hard mix's is infinite.
/// Choosing bases that survive the 8-bit round trip (n/255 exactly) drops the
/// snapshot error to the fix15 quantum, ~1.2e-5, which even a slope of 10
/// keeps well inside a tolerance of 2. Without that the honest tolerance for
/// vivid light would be 6, and the test would stop being able to see a bug.
///
/// Three modes are still step functions and get a small pixel budget instead
/// — see `assert_agrees_tol_outliers`.
#[test]
fn cpu_matches_gpu_for_the_dodge_burn_light_family() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    for mode in [
        Blend::ColorBurn,
        Blend::LinearBurn,
        Blend::ColorDodge,
        Blend::GlowDodge,
        Blend::VividLight,
        Blend::LinearLight,
        Blend::PinLight,
        Blend::HardMix,
        Blend::Divide,
        Blend::DarkerColor,
        Blend::LighterColor,
        Blend::Luminosity,
    ] {
        let mut doc = Document::new(128, 128);
        // 0.8 = 204/255, 0.2 = 51/255, 0.6 = 153/255, 1.0 — all exact.
        // A dark tile AND a white tile, because the burn family branches on
        // `Cb == 1` and the dodge family on `Cb == 0`.
        fill(&mut doc, 0, TileIdx::new(0, 0), [0.8, 0.2, 0.2, 1.0]);
        fill(&mut doc, 0, TileIdx::new(1, 0), [0.2, 0.6, 0.8, 1.0]);
        fill(&mut doc, 0, TileIdx::new(0, 1), [1.0, 1.0, 1.0, 1.0]);
        fill(&mut doc, 0, TileIdx::new(1, 1), [0.2, 0.2, 0.2, 1.0]);

        doc.add_layer("blended");
        doc.set_layer_blend(1, mode);
        // The ramp sweeps alpha across x and colour across y, so every tile
        // crosses the operators' 0 / ½ / 1 branch boundaries at partial
        // coverage — which is the half of the frame nobody looks at.
        for ty in 0..2 {
            for tx in 0..2 {
                fill_ramp(&mut doc, 1, TileIdx::new(tx, ty), [0.4, 0.8, 0.6]);
            }
        }
        // Hard mix flips a whole channel on one side of `Cb + Cs == 1`, and
        // the two whole-colour compares flip on `Lum(Cs) == Lum(Cb)`. Both
        // thresholds are a curve through the tile, and because Cs depends
        // only on y the flip lands on entire ROWS when it lands at all.
        // 1024 px of 16384 (6%) covers a few rows per channel; a wrong
        // formula misses over half the canvas and still fails.
        let (tol, max_over) = match mode {
            Blend::HardMix | Blend::DarkerColor | Blend::LighterColor => (2, 1024),
            // The nonseparable Lum()/ClipColor() path carries the same 4/255
            // slack the part-2 trio already needed.
            Blend::Luminosity => (4, 0),
            _ => (2, 0),
        };
        assert_agrees_tol_outliers(&mut r, &doc, &format!("{mode:?}"), tol, max_over);
    }
}

/// A part-3 mode inside a folder, and one at partial layer opacity: the
/// group-blit arm of `blend2.wgsl` (`fs_blit`) is a SECOND copy of the
/// blend, reached only through an isolation buffer, and nothing above walks
/// through it.
#[test]
fn cpu_matches_gpu_for_a_part3_mode_in_a_folder() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    // 8-bit-exact bases (n/255), same reason as the part-3 family test
    // above: the GPU blends against the Rgba8Unorm canvas snapshot, and
    // vivid light's slope (1/(1-Cb)) amplifies any snapshot error. Exact
    // bases remove the DESTINATION side of that error; the SOURCE side
    // cannot be removed here — the folder's isolation buffer is itself
    // Rgba8Unorm (GROUP_FORMAT), so the child ramp arrives quantized and
    // 65% opacity lands it between 8-bit steps. That residual, times the
    // slope, is the vivid-light assertion's wider tolerance below.
    fill(
        &mut doc,
        0,
        TileIdx::new(0, 0),
        [204.0 / 255.0, 153.0 / 255.0, 51.0 / 255.0, 1.0],
    );
    fill(
        &mut doc,
        0,
        TileIdx::new(1, 1),
        [51.0 / 255.0, 51.0 / 255.0, 153.0 / 255.0, 1.0],
    );

    doc.add_layer("child");
    doc.layers[1].depth = 1;
    fill_ramp(&mut doc, 1, TileIdx::new(0, 0), [0.4, 0.8, 0.6]);

    let mut folder = mn_core::Layer::new("F");
    folder.folder = true;
    doc.layers.push(folder);
    doc.set_layer_blend(2, Blend::ColorDodge);
    assert_agrees_tol(&mut r, &doc, "folder blitting color dodge", 2);

    doc.set_layer_opacity(2, 0.4);
    r.invalidate();
    assert_agrees_tol(&mut r, &doc, "folder color dodge at 40%", 2);

    // And a part-3 mode on a plain layer at partial opacity: opacity folds
    // into the source BEFORE the blend on both paths, which for a
    // high-slope operator is where the two would drift apart first.
    doc.set_layer_blend(2, Blend::VividLight);
    doc.set_layer_opacity(2, 0.65);
    r.invalidate();
    // Tolerance 4, display-path honesty (see the base-colour note above):
    // the ramp through the Rgba8Unorm group buffer at 65% carries a 1/2-LSB
    // source quantum that the mode's slope stretches to 4/255 on single
    // pixels (reproduced identically on hardware AND WARP — rounding, not
    // the Intel draw-drop flake). Export composites on the CPU and is exact.
    assert_agrees_tol(&mut r, &doc, "folder vivid light at 65%", 4);

    // The FIXED-FUNCTION modes on a folder: the layers palette offers them
    // for folders, and the folder blit indexes the blit-pipeline array by
    // slot — with only normal/multiply/screen built, Add here was an
    // index-out-of-bounds panic before it was a wrong pixel. Subtract
    // rides blend2 (its transparent-destination fix) and pins that path's
    // folder blit too.
    for mode in [Blend::Add, Blend::Multiply, Blend::Screen, Blend::Subtract] {
        doc.set_layer_blend(2, mode);
        doc.set_layer_opacity(2, 1.0);
        r.invalidate();
        assert_agrees_tol(&mut r, &doc, &format!("folder {mode:?}"), 2);
    }
}

#[test]
fn cpu_matches_gpu_with_layer_opacity_and_a_deep_stack() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    fill(&mut doc, 0, TileIdx::new(0, 0), [0.8, 0.8, 0.2, 1.0]);
    fill(&mut doc, 0, TileIdx::new(1, 1), [0.2, 0.2, 0.8, 0.5]);

    doc.add_layer("multiply 40%");
    doc.set_layer_blend(1, Blend::Multiply);
    doc.set_layer_opacity(1, 0.4);
    fill_ramp(&mut doc, 1, TileIdx::new(0, 0), [1.0, 0.4, 0.4]);
    fill(&mut doc, 1, TileIdx::new(1, 1), [0.1, 0.9, 0.3, 0.75]);

    doc.add_layer("screen 65%");
    doc.set_layer_blend(2, Blend::Screen);
    doc.set_layer_opacity(2, 0.65);
    fill_ramp(&mut doc, 2, TileIdx::new(1, 0), [0.2, 0.2, 1.0]);
    fill(&mut doc, 2, TileIdx::new(0, 0), [0.9, 0.9, 0.9, 0.3]);

    doc.add_layer("normal 100%, hidden");
    fill(&mut doc, 3, TileIdx::new(0, 1), [1.0, 0.0, 0.0, 1.0]);
    doc.set_layer_visible(3, false);

    assert_agrees(&mut r, &doc, "stack");

    // Unhiding must change the picture and still agree.
    doc.set_layer_visible(3, true);
    assert_agrees(&mut r, &doc, "stack + unhidden");

    // Zero opacity is the same as hidden.
    doc.set_layer_opacity(3, 0.0);
    assert_agrees(&mut r, &doc, "stack + zero opacity");
}

#[test]
fn cpu_matches_gpu_with_a_folder_cascade() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    // [base, child (depth 1), folder header] — the child composites into an
    // isolation buffer which the folder blends at its opacity, on both paths.
    let mut doc = Document::new(128, 128);
    fill(&mut doc, 0, TileIdx::new(0, 0), [0.9, 0.6, 0.1, 1.0]);
    doc.add_layer("child");
    doc.layers[1].depth = 1;
    fill(&mut doc, 1, TileIdx::new(0, 0), [0.1, 0.3, 0.9, 0.8]);
    let mut folder = mn_core::Layer::new("F");
    folder.folder = true;
    doc.layers.push(folder);

    assert_agrees(&mut r, &doc, "folder open");

    doc.set_layer_opacity(2, 0.5);
    r.invalidate();
    assert_agrees(&mut r, &doc, "folder at 50%");

    doc.set_layer_visible(2, false);
    r.invalidate();
    assert_agrees(&mut r, &doc, "folder hidden");
}

#[test]
fn cpu_matches_gpu_with_frame_folder_mask_and_clip() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    // Art below a frame folder + ramped children + a clipped tone layer:
    // exercises the coverage-mask multiply, the group blit, the border ink
    // and the clip scratch on both compositors.
    let mut doc = Document::new(128, 128);
    for ty in 0..2 {
        for tx in 0..2 {
            fill(&mut doc, 0, TileIdx::new(tx, ty), [0.9, 0.2, 0.2, 1.0]);
        }
    }
    let fs = mn_core::FrameSet::single_rect([24.0, 24.0, 104.0, 104.0], 5.0);
    let hi = doc.add_frame_folder("F", fs);
    let draw = hi - 1;
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, draw, TileIdx::new(tx, ty), [0.2, 0.7, 0.3]);
        }
    }
    assert_agrees(&mut r, &doc, "frame folder");

    // Clip a multiply tone to the draw layer, inside the folder.
    let ti = doc
        .add_layer_in_folder(doc.layers.len() - 1, "tone")
        .unwrap();
    doc.set_layer_clip(ti, true);
    doc.set_layer_blend(ti, Blend::Multiply);
    doc.set_layer_opacity(ti, 0.6);
    for ty in 0..2 {
        fill(&mut doc, ti, TileIdx::new(0, ty), [0.4, 0.4, 0.9, 1.0]);
    }
    r.invalidate();
    assert_agrees(&mut r, &doc, "frame folder + clipped tone");

    // Folder opacity through the isolated group.
    doc.set_layer_opacity(doc.layers.len() - 1, 0.5);
    r.invalidate();
    assert_agrees(&mut r, &doc, "frame folder at 50%");
}

/// FB-knockout: a plain folder's mat (border effect grown from the union
/// of children ink) draws just beneath the group on BOTH compositors,
/// scales with the folder's opacity, and vanishes with the effect.
#[test]
fn cpu_matches_gpu_with_a_folder_knockout() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, 0, TileIdx::new(tx, ty), [0.8, 0.3, 0.2]);
        }
    }
    let fi = doc.add_folder_above(0, "grp");
    let child = doc.add_layer_in_folder(fi, "ink").unwrap();
    let hdr = doc.layers.len() - 1;
    fill(&mut doc, child, TileIdx::new(0, 0), [0.1, 0.1, 0.9, 0.8]);
    assert!(doc.set_edge(
        hdr,
        Some(mn_core::EdgeParams {
            width_px: 5.0,
            colour: [255, 255, 255],
            ..Default::default()
        })
    ));
    doc.refresh_derived(600);
    assert_agrees(&mut r, &doc, "folder knockout");

    doc.set_layer_opacity(hdr, 0.5);
    doc.refresh_derived(600);
    r.invalidate();
    assert_agrees(&mut r, &doc, "folder knockout at 50%");

    assert!(doc.set_edge(hdr, None));
    doc.refresh_derived(600);
    r.invalidate();
    assert_agrees(&mut r, &doc, "knockout off");
}

/// FB-overflow: an escaped child re-seats above its frame folder header on
/// BOTH compositors (the shared `composite_order` walk) — outside the panel
/// mask, over the border ink, immune to the folder's opacity, and back to
/// clipped when the flag drops.
#[test]
fn cpu_matches_gpu_with_an_escaped_frame_child() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    for ty in 0..2 {
        for tx in 0..2 {
            fill(&mut doc, 0, TileIdx::new(tx, ty), [0.9, 0.2, 0.2, 1.0]);
        }
    }
    let fs = mn_core::FrameSet::single_rect([24.0, 24.0, 104.0, 104.0], 5.0);
    let hi = doc.add_frame_folder("F", fs);
    let draw = hi - 1;
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, draw, TileIdx::new(tx, ty), [0.2, 0.7, 0.3]);
        }
    }
    // A second child bursts out — half-alpha, so the escapee exercises the
    // ordinary blend path at its new seat, not just an opaque overwrite.
    let bi = doc
        .add_layer_in_folder(doc.layers.len() - 1, "burst")
        .unwrap();
    for tx in 0..2 {
        fill(&mut doc, bi, TileIdx::new(tx, 0), [0.2, 0.3, 0.9, 0.6]);
    }
    assert!(doc.set_layer_escape(bi, true));
    assert_agrees(&mut r, &doc, "escaped frame child");

    // Folder opacity scales the sealed group only — the escapee left it.
    let hdr = doc.layers.len() - 1;
    doc.set_layer_opacity(hdr, 0.5);
    r.invalidate();
    assert_agrees(&mut r, &doc, "escaped child + folder at 50%");

    assert!(doc.set_layer_escape(bi, false));
    r.invalidate();
    assert_agrees(&mut r, &doc, "escape removed: clipped shape again");
}

/// FB-overflow part 2 on BOTH compositors: the layer's own mask caps the
/// spill (the GPU folds `1 − mask` into a second texture variant, the CPU
/// scales at composite time), and the draws-over set moves the escaped seat.
/// The seat/cap changes are made WITHOUT `invalidate()` on purpose — they
/// move no tile revision, so `LayerSig` is the only thing that can notice.
#[test]
fn cpu_matches_gpu_with_a_mask_capped_breakout() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    for ty in 0..2 {
        for tx in 0..2 {
            fill(&mut doc, 0, TileIdx::new(tx, ty), [0.9, 0.2, 0.2, 1.0]);
        }
    }
    // Lower panel + a half-alpha escapee spanning the whole page, so the
    // two halves of the cap exercise the ordinary blend path (a double
    // blend at the mask's edge would show up as a seam here).
    let lo = doc.add_frame_folder("lower", mn_core::FrameSet::single_rect([8.0, 64.0, 120.0, 120.0], 5.0));
    let burst = lo - 1;
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, burst, TileIdx::new(tx, ty), [0.2, 0.7, 0.3]);
        }
    }
    assert!(doc.set_layer_escape(burst, true));
    assert_agrees(&mut r, &doc, "breakout, no cap");

    // Cap it: out through the top-left tile, held everywhere else.
    let mut m = mn_core::doc::LayerMask {
        tiles: std::collections::HashMap::new(),
        enabled: true,
        revision: 1,
        full: false,
    };
    for (tx, ty, cov) in [(0, 0, 32768u16), (1, 0, 0), (0, 1, 0), (1, 1, 12000)] {
        let mut t = mn_core::tile::Tile::new_transparent();
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                t.set_pixel(x, y, [cov, cov, cov, cov]);
            }
        }
        m.tiles.insert(TileIdx::new(tx, ty), std::sync::Arc::new(t));
    }
    doc.layers[burst].mask = Some(m);
    // Mask edits go through invalidate() in the app (see the upload loop's
    // note); poking the field here is not the thing under test.
    r.invalidate();
    assert_agrees(&mut r, &doc, "breakout capped by its own mask");

    // An upper panel above it in the stack, then the draws-over move — no
    // invalidate(), so this also pins the LayerSig spill word.
    let up = doc.add_frame_folder("upper", mn_core::FrameSet::single_rect([8.0, 8.0, 120.0, 56.0], 5.0));
    for tx in 0..2 {
        fill(&mut doc, up - 1, TileIdx::new(tx, 0), [0.2, 0.3, 0.9, 1.0]);
    }
    r.invalidate();
    assert_agrees(&mut r, &doc, "upper panel added");

    assert!(doc.set_layer_spill_seat(burst, Some(up - 1)));
    assert_agrees(&mut r, &doc, "capped spill re-seated over the upper panel");

    assert!(doc.set_layer_spill_seat(burst, None));
    assert_agrees(&mut r, &doc, "back to the default seat");

    doc.layers[burst].mask.as_mut().unwrap().enabled = false;
    r.invalidate();
    assert_agrees(&mut r, &doc, "cap off: all-or-nothing spill again");
}

/// docs/CLIPPING-SCENARIOS.md 2a, clip-to-folder: the CPU captures the
/// group alpha at the folder's close, the GPU copies the group texture
/// into the clip-base capture — the two must agree, including the
/// pre-opacity capture point and the hidden/empty-folder zero cases.
#[test]
fn cpu_matches_gpu_with_clip_to_folder() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    // [base, child (depth 1), header, shade (clip)] — group ink on the
    // left column only; the shade layer spans all four tiles.
    let mut doc = Document::new(128, 128);
    fill(&mut doc, 0, TileIdx::new(0, 0), [0.9, 0.6, 0.1, 1.0]);
    fill(&mut doc, 0, TileIdx::new(1, 1), [0.2, 0.2, 0.8, 1.0]);
    doc.add_layer("child");
    doc.layers[1].depth = 1;
    fill(&mut doc, 1, TileIdx::new(0, 0), [0.1, 0.3, 0.9, 0.8]);
    fill_ramp(&mut doc, 1, TileIdx::new(0, 1), [0.3, 0.8, 0.4]);
    let mut folder = mn_core::Layer::new("F");
    folder.folder = true;
    doc.layers.push(folder);
    let mut shade = mn_core::Layer::new("shade");
    shade.clip = true;
    doc.layers.push(shade);
    let si = doc.layers.len() - 1;
    for ty in 0..2 {
        for tx in 0..2 {
            fill(&mut doc, si, TileIdx::new(tx, ty), [0.9, 0.2, 0.5, 0.8]);
        }
    }
    assert_agrees(&mut r, &doc, "clip to folder");

    // The capture happens BEFORE the folder's opacity is applied, on both
    // paths — the clipped layer must not thin with the folder.
    doc.set_layer_opacity(2, 0.5);
    r.invalidate();
    assert_agrees(&mut r, &doc, "clip to folder at 50%");

    // A hidden folder composites no children: zero base ink on both paths.
    doc.set_layer_visible(2, false);
    r.invalidate();
    assert_agrees(&mut r, &doc, "clip to hidden folder");

    // Visible folder, hidden child: the group is EMPTY (the GPU takes the
    // clear-only capture path, the CPU captures a zeroed accumulator).
    doc.set_layer_visible(2, true);
    doc.set_layer_opacity(2, 1.0);
    doc.set_layer_visible(1, false);
    r.invalidate();
    assert_agrees(&mut r, &doc, "clip to empty group");

    // A through folder has no isolated composite: the chain breaks and the
    // shade layer draws unclipped, identically on both paths.
    doc.set_layer_visible(1, true);
    doc.layers[2].through = true;
    r.invalidate();
    assert_agrees(&mut r, &doc, "through folder breaks the chain");
}

#[test]
fn incremental_redraw_matches_a_cold_render() {
    // The damage-driven path is the one that can rot: paint, render, paint
    // again, render again, and compare against a renderer that saw the final
    // document only. Also covers undo, whose restored tiles must re-upload, and
    // layer removal, whose cached tiles must be evicted.
    let _serial = gpu_guard();
    let Some(mut warm) = renderer() else { return };
    let Some(mut cold) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    fill(&mut doc, 0, TileIdx::new(0, 0), [0.9, 0.1, 0.1, 1.0]);
    let _ = warm.render_offscreen(&doc, 128, 128);

    doc.add_layer("ink");
    doc.set_layer_blend(1, Blend::Multiply);
    fill_ramp(&mut doc, 1, TileIdx::new(1, 1), [0.2, 0.2, 0.2]);
    let _ = warm.render_offscreen(&doc, 128, 128);

    // An undoable stroke, then undo it.
    doc.begin_op();
    fill(&mut doc, 1, TileIdx::new(0, 1), [0.0, 0.0, 0.0, 1.0]);
    assert!(doc.end_op());
    let _ = warm.render_offscreen(&doc, 128, 128);
    assert!(doc.undo(), "undo should have something to do");
    let _ = warm.render_offscreen(&doc, 128, 128);

    // Layer removal: the cached tiles of layer 1 must not linger.
    doc.set_layer_opacity(1, 0.5);
    let _ = warm.render_offscreen(&doc, 128, 128);

    let a = warm.render_offscreen(&doc, 128, 128);
    let b = cold.render_offscreen(&doc, 128, 128);
    let diff = a
        .pixels()
        .zip(b.pixels())
        .map(|(p, q)| {
            (0..4)
                .map(|c| (p.0[c] as i32 - q.0[c] as i32).abs())
                .max()
                .unwrap_or(0)
        })
        .max()
        .unwrap_or(0);
    assert!(
        diff <= 1,
        "incremental render drifted from a cold one by {diff}"
    );

    // And the warm one still matches the CPU.
    assert_agrees(&mut warm, &doc, "after undo");
}

#[test]
fn removing_a_layer_evicts_its_tiles() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    fill(&mut doc, 0, TileIdx::new(0, 0), [1.0, 1.0, 1.0, 1.0]);
    doc.add_layer("to be deleted");
    fill(&mut doc, 1, TileIdx::new(0, 0), [1.0, 0.0, 0.0, 1.0]);
    let _ = r.render_offscreen(&doc, 128, 128);
    assert!(r.cached_tile_count() >= 2);

    assert!(doc.remove_layer(1));
    assert_agrees(&mut r, &doc, "after remove_layer");
    assert_eq!(r.cached_tile_count(), 1, "dead layer's tiles still cached");

    // invalidate() is the blunt hammer and must also be correct.
    r.invalidate();
    assert_eq!(r.cached_tile_count(), 0);
    assert_agrees(&mut r, &doc, "after invalidate");
}

#[test]
fn empty_document_renders_as_paper() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };
    let doc = Document::new(64, 64);
    let img = r.render_offscreen(&doc, 64, 64);
    assert_eq!(img.get_pixel(32, 32).0, [255, 255, 255, 255]);
    assert_agrees(&mut r, &doc, "empty");
}

/// Regression probe for the pre-existing Intel DX12 draw-drop flake (see
/// `assert_agrees`): a flat stack rendered, grown by one layer, and rendered
/// again — the exact shape that loses the new layer's tiles on hardware.
/// Verified via the WARP fallback like every other agreement test; if WARP
/// ever disagrees here, the compositor itself has broken.
#[test]
fn rebuild_after_layer_add_agrees() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };
    let mut doc = Document::new(128, 128);
    for ty in 0..2 {
        for tx in 0..2 {
            fill(&mut doc, 0, TileIdx::new(tx, ty), [0.9, 0.2, 0.2, 1.0]);
        }
    }
    doc.add_layer("mid");
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, 1, TileIdx::new(tx, ty), [0.2, 0.7, 0.3]);
        }
    }
    assert_agrees(&mut r, &doc, "flat before");
    doc.add_layer("top");
    for ty in 0..2 {
        fill(&mut doc, 2, TileIdx::new(0, ty), [0.4, 0.4, 0.9, 0.7]);
    }
    r.invalidate();
    assert_agrees(&mut r, &doc, "flat after layer add");
}

/// Zoomed-out quality (round 13): a 1-px stripe pattern rendered at quarter
/// scale must come out as a smooth mid-grey, not the shimmering black/white
/// picks the mip-less bilinear sampler produced (owner report 2026-08-14).
#[test]
fn downscaled_render_area_averages_via_mips() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };
    let mut doc = Document::new(256, 256);
    // Vertical 1-px black/white stripes over the whole page.
    for ty in 0..4 {
        for tx in 0..4 {
            let tile = doc.layers[0].tile_mut(TileIdx::new(tx, ty));
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let v = if x % 2 == 0 { 0u16 } else { 32768 };
                    tile.set_pixel(x, y, [v, v, v, 32768]);
                }
            }
        }
    }
    let img = r.render_offscreen(&doc, 64, 64);
    // Every interior pixel should sit near the area average — and the
    // average is taken in LINEAR LIGHT (2026-08-20), so half black and half
    // white is ~188 encoded, not 128.
    //
    // 128 is what you get by averaging the display-encoded bytes, which is
    // not averaging light: it makes downscaled ink darker and harder than it
    // should be, which is what the owner saw when he compared our zoomed-out
    // text with CSP's. The band below is deliberately chosen to EXCLUDE both
    // failure modes — 128 (encoded-space averaging) is out of range, and so
    // are the near-0/near-255 picks of mip-less sampling that round 13
    // originally fixed.
    const EXPECT: i32 = 188;
    let mut worst = 0i32;
    for y in 8..56 {
        for x in 8..56 {
            let p = img.get_pixel(x, y).0;
            worst = worst.max((p[0] as i32 - EXPECT).abs());
        }
    }
    assert!(
        worst <= 40,
        "downscale should area-average the stripes in linear light \
         (expect ~{EXPECT}); worst deviation {worst}. ~128 means the average \
         went back to encoded space; near 0 or 255 means the mips are not \
         being sampled at all."
    );
}

#[test]
fn cpu_matches_gpu_with_a_through_folder() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    // LF-002: the same folder-cascade doc with THROUGH set — the multiply
    // child must reach the base art on BOTH compositors, and the two paths
    // must still agree pixel-for-pixel.
    let mut doc = Document::new(128, 128);
    fill(&mut doc, 0, TileIdx::new(0, 0), [0.9, 0.6, 0.1, 1.0]);
    doc.add_layer("child");
    doc.layers[1].depth = 1;
    doc.layers[1].blend = Blend::Multiply;
    fill(&mut doc, 1, TileIdx::new(0, 0), [0.5, 0.5, 0.5, 0.8]);
    let mut folder = mn_core::Layer::new("F");
    folder.folder = true;
    folder.through = true;
    doc.layers.push(folder);

    assert_agrees(&mut r, &doc, "through folder");

    // And the effect is REAL: the multiply child darkens the base (the
    // sealed version would leave it untouched).
    let sealed = {
        let mut s = doc.clone();
        s.layers[2].through = false;
        mn_core::export::composite(&s, mn_core::export::Background::White)
    };
    let open = mn_core::export::composite(&doc, mn_core::export::Background::White);
    let darkened = open
        .pixels()
        .zip(sealed.pixels())
        .filter(|(o, s)| o.0[0] < s.0[0])
        .count();
    assert!(
        darkened > 100,
        "through must darken the base ({darkened} px)"
    );
}

#[test]
fn cpu_matches_gpu_with_layer_colour_tint() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    // LP-016: a layer holding black, grey and white ink, tinted blue —
    // both compositors must agree, and the tint must actually change the
    // output (black-over-white shows blue, not black).
    let mut doc = Document::new(128, 128);
    let li = doc.add_layer("ink");
    {
        let t = doc.layers[li].tile_mut(TileIdx::new(0, 0));
        t.set_pixel(5, 5, [0, 0, 0, 32767]); // black
        t.set_pixel(10, 10, [16384, 16384, 16384, 32767]); // grey
        t.set_pixel(15, 15, [32767; 4]); // white
    }
    doc.set_layer_colour(li, Some([0, 0, 255]));

    assert_agrees(&mut r, &doc, "layer colour tint");

    let tinted = mn_core::export::composite(&doc, mn_core::export::Background::White);
    let p = tinted.get_pixel(5, 5);
    assert!(
        p.0[2] > 200 && p.0[0] < 60,
        "black must display as the tint (blue), got {:?}",
        p.0
    );
    let w = tinted.get_pixel(15, 15);
    assert_eq!(w.0[0], 255, "white stays white");
}

/// LP-017: the two-tone pair (main colour on the black end, sub colour on
/// the WHITE end) is a per-layer display maths that lives twice — once in
/// `mn_core::blend::layer_colour_tint`, once in `tiles.wgsl`/`blend2.wgsl`.
/// The lerp has to land on the same value at every alpha, so the ramp tile
/// (alpha across x, value across y) is the real subject here; the flat
/// pixels are only there to prove the two ends did not swap.
///
/// The second half runs the same layer through a blend2 SHADER mode: the
/// packed `fx` word rides a different pipeline's instance buffer there, and
/// a layout that agreed only in the fixed-function path would pass the test
/// above and still paint the wrong colour on an Overlay layer.
#[test]
fn cpu_matches_gpu_with_a_two_tone_layer() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    fill(&mut doc, 0, TileIdx::new(0, 0), [0.9, 0.2, 0.2, 1.0]);
    fill(&mut doc, 0, TileIdx::new(1, 0), [1.0, 1.0, 1.0, 1.0]);
    fill(&mut doc, 0, TileIdx::new(0, 1), [0.1, 0.1, 0.1, 1.0]);

    let li = doc.add_layer("two-tone");
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, li, TileIdx::new(tx, ty), [1.0, 1.0, 1.0]);
        }
    }
    {
        let t = doc.layers[li].tile_mut(TileIdx::new(0, 0));
        t.set_pixel(5, 5, [0, 0, 0, 32768]); // black end
        t.set_pixel(15, 15, [32768; 4]); // white end
    }
    doc.set_layer_colour(li, Some([0, 0, 255]));
    doc.set_layer_sub_colour(li, Some([255, 192, 0]));
    assert_agrees(&mut r, &doc, "two-tone layer");

    // The ends must be the two chips, not one chip and paper white.
    let cpu = export::composite(&doc, Background::White);
    let k = cpu.get_pixel(5, 5).0;
    assert!(
        k[2] > 200 && k[0] < 60,
        "black end → main colour, got {k:?}"
    );
    let w = cpu.get_pixel(15, 15).0;
    assert!(
        w[0] > 200 && w[1] > 140 && w[2] < 60,
        "white end → sub colour, got {w:?}"
    );

    // Same document through the shader-composite pass. Tolerance 4 for the
    // same reason the blend-mode sweep uses it: blend2 blends against an
    // 8-bit snapshot of the canvas, the CPU reference against f32.
    doc.set_layer_blend(li, Blend::Overlay);
    assert_agrees_tol(&mut r, &doc, "two-tone through blend2", 4);

    // The compatibility promise on the GPU side: an explicit white sub and
    // no sub at all are the same render. Checked against the CPU reference
    // rather than GPU-against-GPU, so the laptop's dropped-draw flake still
    // has its WARP re-check (see `assert_agrees`); the byte-for-byte half of
    // this promise is pinned on the CPU in `core::export`.
    doc.set_layer_blend(li, Blend::Normal);
    doc.set_layer_sub_colour(li, None);
    assert_agrees(&mut r, &doc, "layer colour, no sub");
    doc.set_layer_sub_colour(li, Some([255, 255, 255]));
    assert_agrees(&mut r, &doc, "layer colour, white sub");
}

/// Upload path: a full-page rebuild pushes HUNDREDS of tiles in one frame
/// (a real doc open uploaded 964). Those go through one shared staging
/// buffer, flushed in batches, so this pins the two things batching can get
/// wrong and a per-tile `write_texture` never could:
///
/// * a tile written at the wrong buffer offset (colours swap between tiles —
///   every tile here has its own colour, so a swap is a visible block), and
/// * a tile dropped at a batch boundary (600 tiles crosses it twice).
///
/// The five masked tiles are scattered on purpose. LM-005 folds a layer mask
/// into the uploaded pixels, so those are the tiles that hand the batcher an
/// OWNED buffer instead of a borrowed tile slice, and each still has to land
/// at the same offset as its unmasked neighbours.
#[test]
fn cpu_matches_gpu_for_a_many_tile_upload() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let (tw, th) = (20usize, 15usize);
    let mut doc = Document::new((tw * TILE_SIZE) as u32, (th * TILE_SIZE) as u32);
    for ty in 0..th {
        for tx in 0..tw {
            let k = (ty * tw + tx) as f32 / (tw * th) as f32;
            fill(
                &mut doc,
                0,
                TileIdx::new(tx as i32, ty as i32),
                [k, 1.0 - k, (tx as f32) / tw as f32, 1.0],
            );
        }
    }
    let li = doc.add_layer("ramps");
    for ty in 0..th {
        for tx in 0..tw {
            let k = (tx as f32) / tw as f32;
            fill_ramp(
                &mut doc,
                li,
                TileIdx::new(tx as i32, ty as i32),
                [1.0 - k, k, 0.5],
            );
        }
    }

    // Five masked tiles, each a different half-covered pattern.
    let mut m = mn_core::doc::LayerMask {
        enabled: true,
        revision: 1,
        tiles: std::collections::HashMap::new(),
        full: false,
    };
    for (n, (tx, ty)) in [(0usize, 0usize), (7, 3), (11, 8), (19, 14), (4, 12)]
        .into_iter()
        .enumerate()
    {
        let mut t = mn_core::Tile::new_transparent();
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let cov = if (x + y + n) % 3 == 0 { 32768 } else { 8000 };
                t.set_pixel(x, y, [cov, cov, cov, cov]);
            }
        }
        m.tiles
            .insert(TileIdx::new(tx as i32, ty as i32), std::sync::Arc::new(t));
    }
    doc.layers[li].mask = Some(m);

    let t0 = std::time::Instant::now();
    assert_agrees(&mut r, &doc, "many-tile upload");
    let stats = r.frame_stats();
    println!(
        "[test] many-tile upload: {} tiles, {:.1} ms in-renderer, {:.1} ms wall (incl. readback)",
        stats.uploads,
        stats.ms,
        t0.elapsed().as_secs_f32() * 1000.0
    );
}

// --- Blend If ------------------------------------------------------------
//
// The gate reads the DESTINATION, which on the GPU only exists as the
// snapshot the blend2 pass takes — so a gated layer composites through
// `blend2.wgsl` whatever its blend mode, and `blend2.wgsl` grew arms for the
// fixed-function four to make that possible. Every one of those arms is a
// second copy of a formula `mn_core::blend` already owns, which is exactly
// the shape of drift these tests exist to catch.

/// A destination the gate can actually discriminate on: four tiles of flat
/// grey at four luminances, bottom to top of the range.
fn luma_steps(doc: &mut Document) {
    for (i, v) in [0.0f32, 0.35, 0.65, 1.0].into_iter().enumerate() {
        fill(
            doc,
            0,
            TileIdx::new((i % 2) as i32, (i / 2) as i32),
            [v, v, v, 1.0],
        );
    }
}

/// The headline: a gated layer over a four-step grey wedge, in every blend
/// mode that has a FIXED-FUNCTION state. Those four (Normal, Multiply,
/// Screen, Add) never reached `blend2.wgsl` before this round; a gate is the
/// only thing that sends them there, so this is the test that proves the new
/// arms compute what `mn_core::blend` computes.
#[test]
fn cpu_matches_gpu_for_a_gated_layer_in_the_fixed_function_modes() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    for mode in [Blend::Normal, Blend::Multiply, Blend::Screen, Blend::Add] {
        let mut doc = Document::new(128, 128);
        luma_steps(&mut doc);
        doc.add_layer("gated");
        doc.set_layer_blend(1, mode);
        // Shadows only, softly: the wedge crosses the knee, so the render
        // holds full-strength, feathered and fully-gated pixels at once.
        doc.set_layer_blend_if(
            1,
            Some(mn_core::BlendIf {
                lo: 0.0,
                hi: 0.4,
                feather: 0.3,
                ..mn_core::BlendIf::FULL
            }),
        );
        for ty in 0..2 {
            for tx in 0..2 {
                fill_ramp(&mut doc, 1, TileIdx::new(tx, ty), [0.4, 0.8, 0.6]);
            }
        }
        assert_agrees_tol(&mut r, &doc, &format!("gated {mode:?}"), 2);
    }
}

/// The gate on top of a mode that ALREADY took the shader path: the two
/// modifiers have to compose, not fight. (`s` is scaled by the weight before
/// the part-2 formula sees it, on both sides.)
#[test]
fn cpu_matches_gpu_for_a_gated_layer_in_a_shader_blend_mode() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    luma_steps(&mut doc);
    doc.add_layer("gated overlay");
    doc.set_layer_blend(1, Blend::Overlay);
    doc.set_layer_blend_if(
        1,
        Some(mn_core::BlendIf {
            lo: 0.5,
            hi: 1.0,
            feather: 0.25,
            ..mn_core::BlendIf::FULL
        }),
    );
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, 1, TileIdx::new(tx, ty), [0.5, 0.3, 0.9]);
        }
    }
    // The part-2 trio's usual 4/255: an 8-bit destination snapshot through
    // Overlay's slope, now also through the gate's ramp.
    assert_agrees_tol(&mut r, &doc, "gated Overlay", 4);
}

/// Two gated layers stacked. This is the ordinary case (a shadow tone and a
/// highlight tone on the same page) and the one that used to be broken: both
/// take the snapshot path, so each needs its OWN snapshot of the backdrop.
#[test]
fn cpu_matches_gpu_with_two_gated_layers_stacked() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    luma_steps(&mut doc);
    doc.add_layer("shadows");
    doc.set_layer_blend_if(
        1,
        Some(mn_core::BlendIf {
            lo: 0.0,
            hi: 0.4,
            feather: 0.2,
            ..mn_core::BlendIf::FULL
        }),
    );
    doc.add_layer("highlights");
    doc.set_layer_blend_if(
        2,
        Some(mn_core::BlendIf {
            lo: 0.6,
            hi: 1.0,
            feather: 0.2,
            ..mn_core::BlendIf::FULL
        }),
    );
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, 1, TileIdx::new(tx, ty), [0.9, 0.2, 0.2]);
            fill_ramp(&mut doc, 2, TileIdx::new(tx, ty), [0.2, 0.4, 0.9]);
        }
    }
    assert_agrees_tol(&mut r, &doc, "two gated layers", 2);
}

/// THE BUG the `snap_owner` field records, in its own right: two ADJACENT
/// shader-composite layers with no Blend If anywhere.
///
/// Before 2026-08-30 both landed in one snapshot pass, so the upper one
/// blended against the backdrop from *below the lower one* and — the pass
/// writes with a REPLACE state — erased it. Verified failing against the old
/// code on WARP: worst channel delta 102/255 at (63, 0), 9640 pixels out of
/// budget. Found while routing Blend If through the same machinery.
#[test]
fn two_stacked_shader_blend_layers_agree() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    fill(&mut doc, 0, TileIdx::new(0, 0), [0.8, 0.2, 0.2, 1.0]);
    fill(&mut doc, 0, TileIdx::new(1, 0), [0.2, 0.6, 0.8, 1.0]);
    fill(&mut doc, 0, TileIdx::new(0, 1), [1.0, 1.0, 1.0, 1.0]);
    fill(&mut doc, 0, TileIdx::new(1, 1), [0.2, 0.2, 0.2, 1.0]);
    doc.add_layer("lower overlay");
    doc.set_layer_blend(1, Blend::Overlay);
    doc.add_layer("upper overlay");
    doc.set_layer_blend(2, Blend::Overlay);
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, 1, TileIdx::new(tx, ty), [0.4, 0.8, 0.6]);
            fill_ramp(&mut doc, 2, TileIdx::new(tx, ty), [0.6, 0.3, 0.9]);
        }
    }
    // Two stacked 8-bit snapshot round trips through Overlay's slope.
    assert_agrees_tol(&mut r, &doc, "two overlays", 6);
}

/// A gated CLIP layer: it reaches the canvas through `fs_blit` (its scratch
/// group is the source), which is a SECOND copy of the gate call. Nothing
/// else in this file walks a clip layer through the shader path.
///
/// # Why the tolerance is 6 and why that is still a real test
///
/// Same story as the dodge/burn family above: the GPU reads the destination
/// from an `Rgba8Unorm` snapshot, the CPU from full-precision f32, and a
/// **steep knee multiplies that error**. The gate's slope is `1 / feather`,
/// here 6.7, and the destination underneath is a RAMP (the clip base), so
/// there is no way to pick 8-bit-exact bases the way the part-3 test does.
/// One LSB of snapshot error (0.004 measured) becomes 0.029 of weight,
/// which on a source at 0.9 premultiplied is 6/255 out.
///
/// Measured, not assumed: at `feather: 0.6` (slope 1.7) the same document
/// agrees inside 2, i.e. the disagreement scales exactly with the slope. A
/// wrong formula — the feather ramping inward, an unnormalised range, the
/// gate applied before opacity instead of after — moves whole regions by
/// tens of levels, not by six.
#[test]
fn cpu_matches_gpu_with_a_gated_clip_layer() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    luma_steps(&mut doc);
    // The clip BASE: partial coverage so the base-alpha multiply is live.
    doc.add_layer("base");
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, 1, TileIdx::new(tx, ty), [0.9, 0.9, 0.9]);
        }
    }
    doc.add_layer("gated clip");
    doc.set_layer_clip(2, true);
    doc.set_layer_blend_if(
        2,
        Some(mn_core::BlendIf {
            lo: 0.2,
            hi: 0.7,
            feather: 0.15,
            ..mn_core::BlendIf::FULL
        }),
    );
    for ty in 0..2 {
        for tx in 0..2 {
            fill(&mut doc, 2, TileIdx::new(tx, ty), [0.1, 0.5, 0.9, 1.0]);
        }
    }
    assert_agrees_tol(&mut r, &doc, "gated clip", 6);
}

/// Inside a sealed folder the "underlying" is the GROUP, not the page. The
/// CPU gets that from its accumulator model and the GPU from its group
/// target; this pins that they get the SAME answer, with a page underneath
/// that is deliberately the wrong brightness for the gate.
#[test]
fn cpu_matches_gpu_with_a_gated_layer_inside_a_folder() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    // Page: solid white. A "shadows only" gate reading the PAGE would hide
    // the layer entirely; reading the group (dark ink) shows it.
    for ty in 0..2 {
        for tx in 0..2 {
            fill(&mut doc, 0, TileIdx::new(tx, ty), [1.0, 1.0, 1.0, 1.0]);
        }
    }

    let inner = doc.add_layer("inner");
    for ty in 0..2 {
        for tx in 0..2 {
            fill(&mut doc, inner, TileIdx::new(tx, ty), [0.1, 0.1, 0.1, 1.0]);
        }
    }
    let gated = doc.add_layer("gated");
    doc.set_layer_blend_if(
        gated,
        Some(mn_core::BlendIf {
            lo: 0.0,
            hi: 0.3,
            feather: 0.2,
            ..mn_core::BlendIf::FULL
        }),
    );
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, gated, TileIdx::new(tx, ty), [0.9, 0.4, 0.2]);
        }
    }
    let folder = doc.add_layer("folder");
    doc.layers[folder].folder = true;
    doc.layers[inner].depth = 1;
    doc.layers[gated].depth = 1;

    assert_agrees_tol(&mut r, &doc, "gated inside a folder", 2);
}

/// `LayerSig` half: turning a gate on (and off again) moves no tile
/// revision at all, so a canvas that did not watch it would keep showing the
/// ungated composite. The wave-5 lesson, as a test.
#[test]
fn toggling_a_gate_rebuilds_the_canvas() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    luma_steps(&mut doc);
    doc.add_layer("gated");
    for ty in 0..2 {
        for tx in 0..2 {
            fill(&mut doc, 1, TileIdx::new(tx, ty), [0.9, 0.2, 0.2, 1.0]);
        }
    }
    // Warm the canvas with the UNGATED picture first — that is the state a
    // missing signature would leave on screen.
    assert_agrees_tol(&mut r, &doc, "before the gate", 2);

    doc.set_layer_blend_if(
        1,
        Some(mn_core::BlendIf {
            lo: 0.0,
            hi: 0.4,
            feather: 0.0,
            ..mn_core::BlendIf::FULL
        }),
    );
    assert_agrees_tol(&mut r, &doc, "gate on", 2);

    // …and OFF is the dangerous direction: nothing is damaged, the tiles
    // still carry the same revisions, and only the signature notices.
    doc.set_layer_blend_if(1, None);
    assert_agrees_tol(&mut r, &doc, "gate off again", 2);
}

// --- round 2: the THIS-layer arm and the per-channel arms ------------------
// The shader reads a different pixel now (`gate_value` + the mode word), and
// the THIS arm reads the source AFTER opacity is folded in — which is the one
// place the two sides could drift, because the CPU folds opacity in earlier
// in its own loop. These are the tests that would catch it.

/// Every source×channel pair, on a page and an ink that make all eight
/// answers different: a coloured wedge under a ramped colour layer, gated on
/// a band that bites in the middle of both.
#[test]
fn cpu_matches_gpu_for_every_blend_if_arm() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    for source in mn_core::blendif::GateSource::ALL {
        for channel in mn_core::blendif::GateChannel::ALL {
            let mut doc = Document::new(128, 128);
            // A destination with the three channels far apart, per tile.
            for (i, c) in [
                [0.9f32, 0.2, 0.1],
                [0.1, 0.8, 0.3],
                [0.2, 0.3, 0.95],
                [0.5, 0.5, 0.5],
            ]
            .into_iter()
            .enumerate()
            {
                fill(
                    &mut doc,
                    0,
                    TileIdx::new((i % 2) as i32, (i / 2) as i32),
                    [c[0], c[1], c[2], 1.0],
                );
            }
            doc.add_layer("gated");
            doc.set_layer_blend_if(
                1,
                Some(mn_core::BlendIf {
                    lo: 0.3,
                    hi: 0.7,
                    feather: 0.25,
                    source,
                    channel,
                }),
            );
            for ty in 0..2 {
                for tx in 0..2 {
                    fill_ramp(&mut doc, 1, TileIdx::new(tx, ty), [0.85, 0.45, 0.7]);
                }
            }
            assert_agrees_tol(&mut r, &doc, &format!("{source:?}/{channel:?}"), 3);
        }
    }
}

/// The THIS arm at partial opacity, which is where the two compositors fold
/// the opacity in at different moments. Both read the STRAIGHT value, so the
/// gate must not move at all — and a shader that gated on the premultiplied
/// pixel would drift here and nowhere else.
#[test]
fn cpu_matches_gpu_for_a_this_layer_gate_at_partial_opacity() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    luma_steps(&mut doc);
    doc.add_layer("gated");
    doc.set_layer_opacity(1, 0.45);
    doc.set_layer_blend_if(
        1,
        Some(mn_core::BlendIf {
            lo: 0.55,
            hi: 1.0,
            feather: 0.2,
            source: mn_core::blendif::GateSource::This,
            ..mn_core::BlendIf::FULL
        }),
    );
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, 1, TileIdx::new(tx, ty), [0.95, 0.6, 0.25]);
        }
    }
    assert_agrees_tol(&mut r, &doc, "this-layer gate at 45%", 3);
}

/// The arms are in `LayerSig`, so swapping the CHANNEL — which moves no
/// float and no tile revision — has to rebuild the canvas. Same trap as
/// `toggling_a_gate_rebuilds_the_canvas`, one level down.
#[test]
fn swapping_the_gate_channel_rebuilds_the_canvas() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    for (i, c) in [
        [0.9f32, 0.1, 0.1],
        [0.1, 0.9, 0.1],
        [0.1, 0.1, 0.9],
        [0.6, 0.6, 0.6],
    ]
    .into_iter()
    .enumerate()
    {
        fill(
            &mut doc,
            0,
            TileIdx::new((i % 2) as i32, (i / 2) as i32),
            [c[0], c[1], c[2], 1.0],
        );
    }
    doc.add_layer("gated");
    for ty in 0..2 {
        for tx in 0..2 {
            fill(&mut doc, 1, TileIdx::new(tx, ty), [0.2, 0.2, 0.2, 1.0]);
        }
    }
    let band = mn_core::BlendIf {
        lo: 0.5,
        hi: 1.0,
        feather: 0.0,
        ..mn_core::BlendIf::FULL
    };
    doc.set_layer_blend_if(1, Some(band));
    assert_agrees_tol(&mut r, &doc, "red channel", 2);
    // Only the channel moves: the three floats and every tile revision stay
    // exactly where they were.
    doc.set_layer_blend_if(
        1,
        Some(mn_core::BlendIf {
            channel: mn_core::blendif::GateChannel::B,
            ..band
        }),
    );
    assert_agrees_tol(&mut r, &doc, "blue channel", 2);
}

/// The THIS arm down the CLIP path — the `fs_blit` half of the shader,
/// which reads its source from a scratch GROUP rather than a tile. The CPU
/// folds the clip base's alpha into `src` before the gate; the blit's
/// source has it folded in already, so the two must read the same straight
/// value. Tolerance as `cpu_matches_gpu_with_a_gated_clip_layer` (the same
/// 8-bit snapshot through the same steep knee).
#[test]
fn cpu_matches_gpu_for_a_this_layer_gate_on_a_clip_layer() {
    let _serial = gpu_guard();
    let Some(mut r) = renderer() else { return };

    let mut doc = Document::new(128, 128);
    luma_steps(&mut doc);
    doc.add_layer("base");
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, 1, TileIdx::new(tx, ty), [0.9, 0.9, 0.9]);
        }
    }
    doc.add_layer("gated clip");
    doc.set_layer_clip(2, true);
    doc.set_layer_blend_if(
        2,
        Some(mn_core::BlendIf {
            lo: 0.2,
            hi: 0.7,
            feather: 0.15,
            source: mn_core::blendif::GateSource::This,
            channel: mn_core::blendif::GateChannel::G,
        }),
    );
    for ty in 0..2 {
        for tx in 0..2 {
            fill_ramp(&mut doc, 2, TileIdx::new(tx, ty), [0.2, 0.8, 0.5]);
        }
    }
    assert_agrees_tol(&mut r, &doc, "this-layer gated clip", 6);
}
