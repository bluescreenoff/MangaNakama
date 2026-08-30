//! `FI-050` — `Document::paint_gradient_freeform`, the painting half of the
//! freeform gradient. [`crate::freeform`]'s own tests pin the geometry; these
//! pin what actually lands on a layer: the guide colours, the bend, the
//! selection clip, the single undo press, and that the mixing modes reach the
//! canvas.
//!
//! Its own file so the two long-lived modules it spans (`doc.rs`,
//! `gradient.rs`) stay out of each other's way — the `resample_work_tests`
//! precedent.

use crate::doc::Document;
use crate::freeform::Freeform;
use crate::gradient::{MixMode, Ramp};
use crate::tile::{FIX15_ONE, TileIdx};

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// A canvas pixel as STRAIGHT 0..1 RGBA — the tile stores it premultiplied,
/// and every assertion below is about colour, not coverage.
fn px(doc: &Document, x: i32, y: i32) -> [f32; 4] {
    let ti = TileIdx::of_pixel(x, y);
    let raw = doc.layers[doc.active]
        .tile(ti)
        .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize))
        .unwrap_or([0; 4]);
    let a = raw[3] as f32 / FIX15_ONE as f32;
    let un = |v: u16| {
        if a > 0.0 {
            (v as f32 / FIX15_ONE as f32 / a).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    [un(raw[0]), un(raw[1]), un(raw[2]), a]
}

fn alpha(doc: &Document, x: i32, y: i32) -> u16 {
    let ti = TileIdx::of_pixel(x, y);
    doc.layers[doc.active]
        .tile(ti)
        .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize)[3])
        .unwrap_or(0)
}

/// Two vertical guides at x=16 and x=112 of a 128-wide page.
fn two_verticals() -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
    (
        vec![[16.0, 0.0], [16.0, 128.0]],
        vec![[112.0, 0.0], [112.0, 128.0]],
    )
}

/// The claim the whole feature rests on: the FIRST guide comes out the ramp's
/// start colour, the SECOND its end colour, and the locus between them is a
/// mix — asserted on the composited pixels, not on the parameter.
#[test]
fn each_guide_wears_its_own_colour_and_the_middle_mixes() {
    let mut doc = Document::new(128, 128);
    let (l1, l2) = two_verticals();
    assert!(doc.paint_gradient_freeform(&l1, &l2, &Ramp::two(RED, BLUE)));

    let on_first = px(&doc, 16, 64);
    assert!(
        on_first[0] > 0.99 && on_first[2] < 0.01,
        "guide 1 is the ramp's start colour: {on_first:?}"
    );
    let on_second = px(&doc, 112, 64);
    assert!(
        on_second[2] > 0.99 && on_second[0] < 0.01,
        "guide 2 is its end colour: {on_second:?}"
    );
    let mid = px(&doc, 64, 64);
    assert!(
        (mid[0] - 0.5).abs() < 0.03 && (mid[2] - 0.5).abs() < 0.03,
        "the midline is halfway through the ramp: {mid:?}"
    );

    // It is opaque EVERYWHERE, including beyond both guides: the parameter is
    // defined over the whole canvas, so unlike a linear drag there is no
    // unpainted outside.
    for x in [0, 5, 64, 120, 127] {
        assert_eq!(alpha(&doc, x, 64), FIX15_ONE as u16, "x={x} must be filled");
    }
    // OUTSIDE the two guides the ramp turns around: a distance RATIO has its
    // extreme ON each guide and drifts slowly back toward the middle further
    // out (both distances converge at infinity). So the far edge is still
    // nearly guide 1's colour, just not exactly it — a soft return rather
    // than the linear tool's hard clamp. Pinned because it is a design
    // decision, not an accident.
    let past = px(&doc, 0, 64);
    assert!(past[0] > 0.9, "past guide 1 is still nearly its colour: {past:?}");
    assert!(
        past[0] < on_first[0],
        "but the extreme is ON the guide, not beyond it: {past:?}"
    );
}

/// Anti-aliased BY CONSTRUCTION, which is the trap CSP requires you to turn
/// AA off to avoid. Walking across a guide there is no step anywhere — a
/// raster-membership fill would have a hard edge exactly there.
#[test]
fn the_painted_ramp_has_no_step_at_a_guide() {
    let mut doc = Document::new(128, 128);
    let (l1, l2) = two_verticals();
    assert!(doc.paint_gradient_freeform(&l1, &l2, &Ramp::two(RED, BLUE)));
    let mut prev = px(&doc, 0, 64)[2];
    for x in 1..128 {
        let now = px(&doc, x, 64)[2];
        assert!(
            (now - prev).abs() < 0.05,
            "a one-pixel step at x={x} ({prev} -> {now}) is a hard edge"
        );
        prev = now;
    }
    // And it is genuinely a ramp, not a flat field.
    assert!(px(&doc, 100, 64)[2] > px(&doc, 30, 64)[2] + 0.5);
}

/// `FI-050`'s point of difference from the linear tool: the colour FOLLOWS
/// the drawn shapes. The same off-axis pixel reads very differently once one
/// guide is bent toward the other, and identically far from the bend.
#[test]
fn a_curved_guide_bends_the_painted_gradient() {
    let straight = vec![[112.0, 0.0], [112.0, 128.0]];
    let mut flat = Document::new(128, 128);
    assert!(flat.paint_gradient_freeform(
        &[[16.0, 0.0], [16.0, 128.0]],
        &straight,
        &Ramp::two(RED, BLUE)
    ));

    // The same guide with a bulge that reaches x=80 around y=64.
    let bulged = vec![
        [16.0, 0.0],
        [16.0, 40.0],
        [80.0, 64.0],
        [16.0, 88.0],
        [16.0, 128.0],
    ];
    let mut bent = Document::new(128, 128);
    assert!(bent.paint_gradient_freeform(&bulged, &straight, &Ramp::two(RED, BLUE)));

    // Level with the bulge, an off-axis pixel is much REDDER than it was:
    // guide 1 (the red one) has come to meet it.
    let (f, b) = (px(&flat, 96, 64), px(&bent, 96, 64));
    assert!(
        b[0] > f[0] + 0.2,
        "the bend must move the painted colour: {f:?} -> {b:?}"
    );
    // The bulge's tip wears guide 1's colour exactly, out in open canvas.
    let tip = px(&bent, 80, 64);
    assert!(
        tip[0] > 0.99 && tip[2] < 0.01,
        "the tip of the drawn guide is the guide's colour: {tip:?}"
    );
    // And the bend is LOCAL: at the top of the page, where the two guides
    // are the same line, the same pixel has barely moved. (The exact
    // "identical away from the bend" claim needs guides longer than this
    // canvas to state cleanly — `freeform::tests` pins it bit-for-bit.)
    let (f_far, b_far) = (px(&flat, 96, 4), px(&bent, 96, 4));
    let near_shift = (b[0] - f[0]).abs();
    let far_shift = (b_far[0] - f_far[0]).abs();
    assert!(
        far_shift * 3.0 < near_shift,
        "the bend must be local: {far_shift} away vs {near_shift} level with it"
    );
}

/// Selection-else-layer, exactly like every other gradient: outside the
/// marching ants the layer is byte-untouched, not painted and then masked.
#[test]
fn a_selection_clips_the_freeform_gradient() {
    let mut doc = Document::new(128, 128);
    doc.selection = Some(crate::selection::Selection::from_rect(
        &doc, 0.0, 0.0, 64.0, 128.0,
    ));
    let (l1, l2) = two_verticals();
    assert!(doc.paint_gradient_freeform(&l1, &l2, &Ramp::two(RED, BLUE)));
    assert_eq!(alpha(&doc, 30, 64), FIX15_ONE as u16, "inside: painted");
    assert_eq!(alpha(&doc, 100, 64), 0, "outside the selection: untouched");
    // The clip does not move the ramp — the parameter still comes from the
    // guides, so what IS painted is the same colour it would have been.
    let mut whole = Document::new(128, 128);
    assert!(whole.paint_gradient_freeform(&l1, &l2, &Ramp::two(RED, BLUE)));
    assert_eq!(px(&doc, 30, 64), px(&whole, 30, 64));
}

/// One gesture, ONE undo press — the two strokes are gesture state, never
/// history, so the whole page comes back on a single Ctrl+Z.
#[test]
fn the_whole_apply_is_one_undo_press() {
    let mut doc = Document::new(128, 128);
    let (l1, l2) = two_verticals();
    let before = doc.op_count();
    assert!(doc.paint_gradient_freeform(&l1, &l2, &Ramp::two(RED, BLUE)));
    assert_eq!(doc.op_count(), before + 1, "exactly one op was recorded");
    assert_ne!(alpha(&doc, 64, 64), 0);
    assert!(doc.undo(), "one press");
    for (x, y) in [(0, 0), (16, 64), (64, 64), (112, 64), (127, 127)] {
        assert_eq!(alpha(&doc, x, y), 0, "({x},{y}) survived the undo");
    }
    assert!(doc.redo(), "and it comes back");
    assert_ne!(alpha(&doc, 64, 64), 0);
}

/// `G-009` rides along for free: the ramp is the SAME `Ramp` every gradient
/// evaluates, so Perceptual mixing works here without a line of its own. The
/// mix.rs idiom — the perceptual midpoint of blue→yellow is the less grey of
/// the two.
#[test]
fn perceptual_mixing_reaches_the_freeform_canvas() {
    let blue = [0.0, 0.0, 1.0, 1.0];
    let yellow = [1.0, 1.0, 0.0, 1.0];
    let (l1, l2) = two_verticals();

    let mut std_doc = Document::new(128, 128);
    assert!(std_doc.paint_gradient_freeform(&l1, &l2, &Ramp::two(blue, yellow)));
    let mut perc_ramp = Ramp::two(blue, yellow);
    perc_ramp.opts.mix = MixMode::Perceptual;
    let mut perc_doc = Document::new(128, 128);
    assert!(perc_doc.paint_gradient_freeform(&l1, &l2, &perc_ramp));

    let (a, b) = (px(&std_doc, 64, 64), px(&perc_doc, 64, 64));
    let diff: f32 = (0..3).map(|k| (a[k] - b[k]).abs()).sum();
    assert!(diff > 0.05, "the two midpoints must differ: {a:?} {b:?}");
    // "Less grey" = further from its own mean channel. The sRGB midpoint of
    // blue and yellow is a flat mid-grey; the Oklab one keeps some colour.
    let chroma = |c: [f32; 4]| {
        let m = (c[0] + c[1] + c[2]) / 3.0;
        (0..3).map(|k| (c[k] - m).abs()).sum::<f32>()
    };
    assert!(
        chroma(b) > chroma(a),
        "Perceptual keeps the middle less grey: {a:?} {b:?}"
    );
    // Both guides are still exactly their authored colours in either mode.
    for d in [&std_doc, &perc_doc] {
        let g1 = px(d, 16, 64);
        assert!(g1[2] > 0.99 && g1[0] < 0.01, "guide 1 stayed blue: {g1:?}");
    }
}

/// Interior stops (`G-008`) come free too — the ramp is walked by
/// `Ramp::color_at`, which knows nothing about how the parameter was made.
#[test]
fn interior_stops_ride_along_free() {
    let mut mid = crate::gradient::MidStops::default();
    mid.insert(crate::gradient::GradStop {
        pos: 0.5,
        color: [0.0, 1.0, 0.0, 1.0],
    });
    let ramp = Ramp::new(RED, BLUE, mid, crate::gradient::RampOpts::default());
    let mut doc = Document::new(128, 128);
    let (l1, l2) = two_verticals();
    assert!(doc.paint_gradient_freeform(&l1, &l2, &ramp));
    let m = px(&doc, 64, 64);
    assert!(
        m[1] > 0.95 && m[0] < 0.05 && m[2] < 0.05,
        "the midline is the interior stop's green: {m:?}"
    );
}

/// A refusing layer and a guide with no points both come back false, with
/// nothing painted and — the part that matters — no undo step banked for an
/// op that never happened.
#[test]
fn a_refusing_layer_or_an_empty_guide_paints_nothing() {
    let (l1, l2) = two_verticals();
    let ramp = Ramp::two(RED, BLUE);

    let mut locked = Document::new(128, 128);
    locked.layers[locked.active].lock = true;
    let before = locked.op_count();
    assert!(!locked.paint_gradient_freeform(&l1, &l2, &ramp));
    assert_eq!(locked.op_count(), before, "a refused op is not history");
    assert_eq!(alpha(&locked, 64, 64), 0);

    let mut empty = Document::new(128, 128);
    let before = empty.op_count();
    assert!(!empty.paint_gradient_freeform(&[], &l2, &ramp), "no guide 1");
    assert!(!empty.paint_gradient_freeform(&l1, &[], &ramp), "no guide 2");
    assert_eq!(empty.op_count(), before);
    assert_eq!(alpha(&empty, 64, 64), 0);
}

/// The per-tile cull cannot change the picture, only the time it takes: the
/// painted page must be identical to one painted by consulting every segment
/// of a long, wiggly pair of guides.
#[test]
fn the_per_tile_cull_paints_the_identical_page() {
    let wiggle = |x: f32, phase: f32| -> Vec<[f32; 2]> {
        (0..=120)
            .map(|i| {
                let y = i as f32 * 2.0;
                [x + (i as f32 * 0.4 + phase).sin() * 9.0, y]
            })
            .collect()
    };
    let (l1, l2) = (wiggle(40.0, 0.0), wiggle(200.0, 1.7));
    let ramp = Ramp::two(RED, BLUE);
    let mut doc = Document::new(256, 240);
    assert!(doc.paint_gradient_freeform(&l1, &l2, &ramp));

    // The reference: the same ramp, evaluated through the ALL-segments
    // parameter, pixel by pixel.
    let field = Freeform::new(&l1, &l2).unwrap();
    for y in (1..240).step_by(17) {
        for x in (1..256).step_by(13) {
            let want = ramp.eval_unit(field.t_at([x as f32 + 0.5, y as f32 + 0.5]), x, y);
            let got = px(&doc, x, y);
            for k in 0..3 {
                assert!(
                    (got[k] - want[k]).abs() < 2.0 / 255.0,
                    "({x},{y}) ch {k}: culled {got:?} vs exact {want:?}"
                );
            }
        }
    }
}

/// The cost of a real page, measured rather than guessed. Ignored by
/// default: it allocates a whole B4/600 layer, which is minutes in a debug
/// build and hundreds of MB either way.
///
///     cargo test -p mn-core --release -- --ignored freeform_full_page
#[test]
#[ignore = "timing measurement — release only, allocates a full B4/600 page"]
fn freeform_full_page_timing() {
    // B4 at 600 DPI, the house page size.
    let (w, h) = (6071u32, 8598u32);
    // `n` points per guide. After `simplify_polyline` at 2 screen px a real
    // hand-drawn guide across a page is a few dozen; 200 is the pathological
    // end (a deliberately shaky line, drawn zoomed in).
    let guide = |x: f32, n: usize| -> Vec<[f32; 2]> {
        (0..=n)
            .map(|i| {
                let y = i as f32 * (h as f32 / n as f32);
                [x + (i as f32 * 60.0 / n as f32).sin() * 120.0, y]
            })
            .collect()
    };

    // The linear tool on the same page, for scale: the difference is what
    // the distance field costs over and above evaluating the ramp, which
    // both tools pay.
    let mut lin = Document::new(w, h);
    let t1 = std::time::Instant::now();
    assert!(lin.paint_gradient_ramp([0.0, 0.0], [w as f32, h as f32], &Ramp::two(RED, BLUE)));
    println!(
        "B4/600 = {w}x{h} = {:.1} Mpx\nlinear gradient (the shipping tool): {:?}",
        (w as f64 * h as f64) / 1e6,
        t1.elapsed()
    );
    drop(lin);

    for n in [40usize, 200] {
        let (l1, l2) = (guide(1200.0, n), guide(4800.0, n));
        // How well the cull works, over the real tile grid.
        let field = Freeform::new(&l1, &l2).unwrap();
        let hd = 32.0 * std::f32::consts::SQRT_2;
        let (mut tiles, mut kept) = (0u64, 0u64);
        for ty in 0..h.div_ceil(64) {
            for tx in 0..w.div_ceil(64) {
                let (a, b) = field
                    .window([tx as f32 * 64.0 + 32.0, ty as f32 * 64.0 + 32.0], hd)
                    .segment_counts();
                tiles += 1;
                kept += (a + b) as u64;
            }
        }
        let mut doc = Document::new(w, h);
        let t0 = std::time::Instant::now();
        assert!(doc.paint_gradient_freeform(&l1, &l2, &Ramp::two(RED, BLUE)));
        println!(
            "freeform, {} guide segments: {:?}  (cull keeps {:.1} per tile)",
            n * 2,
            t0.elapsed(),
            kept as f64 / tiles as f64,
        );
    }
}
