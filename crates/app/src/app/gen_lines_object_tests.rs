//! Owner report 2026-08-23: the generated 流線/集中線 sets are "dogshit"
//! and "I cannot re-select them to edit properties."
//!
//! The second half was two bugs stacked. The Object tool's hit test read
//! the ONE pixel under the cursor at zero tolerance, and a generated run
//! is hairlines with paper between them — so selecting one meant landing
//! a click on a single line. And nothing was drawn until a run was
//! already selected, so there was nothing on screen to aim at; the
//! handles that did exist were computed due east of the centre and could
//! sit off the page entirely.
//!
//! These pin the three fixes end to end: the tolerance, the handles
//! staying on the paper for any placing gesture, and the fact that
//! selecting a run makes its layer active (so the canvas, the Layers
//! palette and Layer ▸ Edit effect lines agree about what you picked).

use super::{App, headless_renderer};
use crate::app::canvas_input::gen_handle_points;
use crate::cmd::{AppCmd, FigureMode, Tool, dispatch};
use mn_core::TileIdx;

fn drain(app: &mut App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

/// A position-sensitive fingerprint of one layer's raster.
fn fingerprint(app: &App, li: usize) -> (usize, u64) {
    let mut n = 0usize;
    let mut sum = 0u64;
    for (idx, t) in app.doc.layers[li].tiles() {
        let (ox, oy) = idx.origin();
        for y in 0..mn_core::tile::TILE_SIZE {
            for x in 0..mn_core::tile::TILE_SIZE {
                if t.pixel(x, y)[3] > 0 {
                    n += 1;
                    sum = sum
                        .wrapping_mul(0x0100_0000_01B3)
                        .wrapping_add((ox + x as i32) as u64 * 65_537 + (oy + y as i32) as u64);
                }
            }
        }
    }
    (n, sum)
}

/// Alpha at one canvas pixel — the test the hit path USED to make.
fn ink_at(app: &App, li: usize, x: i32, y: i32) -> bool {
    let idx = TileIdx::of_pixel(x, y);
    app.doc.layers[li].tile(idx).is_some_and(|t| {
        let (ox, oy) = idx.origin();
        t.pixel((x - ox) as usize, (y - oy) as usize)[3] > 0
    })
}

/// Place one Saturated-line run with the Figure tool and return its layer.
fn place_focus(app: &mut App, a: (f32, f32), b: (f32, f32)) -> usize {
    app.tool = Tool::Figure;
    app.figure_mode = FigureMode::Focus;
    app.finish_figure_drag(a, b);
    drain(app);
    let li = app.doc.active;
    assert!(
        app.doc.layers[li].genlines.is_some(),
        "the drag placed a generated layer"
    );
    li
}

/// A click near the ink of a run selects it — the tolerance disc, not one
/// pixel — and ACTIVATES its layer.
#[test]
fn a_click_near_the_ink_selects_the_run_and_activates_its_layer() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // 1:1, so the hit tolerance is the plain 10 px the code computes from
    // it — a fitted zoom would inflate `tol` until the handle test caught
    // half the page and the ink branch never ran.
    app.viewport.zoom = 1.0;
    let li = place_focus(&mut app, (300.0, 200.0), (300.0, 80.0));

    // 2026-08-24: placements now reach past the panel/page border by
    // default (the owner's CSP default), which fills the page with rays.
    // This test is about the HIT TEST, so shrink the run back to the
    // drag's reach first; the reach defaults are pinned in tests.rs.
    let mut small = app.doc.layers[li].genlines.clone().unwrap();
    small.d = 120.0;
    dispatch(
        &mut app,
        AppCmd::GenLinesApplyTo {
            layer: li,
            spec: small,
        },
    );

    // A point with NO ink on it but ink close by — i.e. the paper between
    // two rays, which is most of a 集中線 and is where a real click lands.
    // Well away from the driver handles, or the handle branch of the hit
    // test would answer and the ink branch would never run (which is how
    // the first draft of this test passed against the unfixed code).
    let handles = gen_handle_points(&app.doc.layers[li].genlines.unwrap(), app.doc.size);
    let mut probe = None;
    'search: for y in 90..310 {
        for x in 190..410 {
            if ink_at(&app, li, x, y) {
                continue;
            }
            // The handle branch tests Manhattan distance against
            // `tol * 1.4` (= 14 px at this zoom); stay well clear of it.
            if handles
                .iter()
                .any(|(_, h)| (h[0] - x as f32).abs() + (h[1] - y as f32).abs() < 40.0)
            {
                continue;
            }
            // Ink within 4 px, but nothing at all within 2 — a gap wide
            // enough that the old single-pixel read could not have hit.
            let near = |r: i32| {
                (-r..=r).any(|dy| {
                    (-r..=r)
                        .any(|dx| dx * dx + dy * dy <= r * r && ink_at(&app, li, x + dx, y + dy))
                })
            };
            if near(4) && !near(2) {
                probe = Some((x as f32 + 0.5, y as f32 + 0.5));
                break 'search;
            }
        }
    }
    let (px, py) = probe.expect("a focus set has paper between its rays");

    // FAIL-BEFORE-FIX: this is exactly what the old hit test read, and it
    // is empty — the click selected nothing, whatever the eye saw.
    assert!(
        !ink_at(&app, li, px as i32, py as i32),
        "the probe is on paper, not on a ray"
    );

    // Put the selection and the active layer somewhere else first, so
    // both assertions below are about the click and not the setup.
    app.doc.active = 0;
    app.gen_sel = None;
    app.tool = Tool::Object;
    app.object_hit(px, py);

    assert_eq!(app.gen_sel, Some(li), "the run under the pointer is picked");
    assert_eq!(
        app.doc.active, li,
        "and the Layers palette moved to it — Layer ▸ Edit effect lines \
         keys on the active layer, so the two must not disagree"
    );

    // Empty paper well away from the burst still selects nothing.
    app.gen_sel = None;
    app.object_hit(20.0, 380.0);
    assert_eq!(app.gen_sel, None, "far from the ink, nothing is picked");
}

/// Every driver handle lands ON THE PAGE, whichever way the placing drag
/// went — including a burst dropped in a corner, where the old "always
/// due east" radius handles were off the paper and ungrabbable.
#[test]
fn every_handle_stays_on_the_page_for_any_drag_direction() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let size = app.doc.size;
    let page =
        |p: [f32; 2]| p[0] >= 0.0 && p[1] >= 0.0 && p[0] <= size.0 as f32 && p[1] <= size.1 as f32;

    for centre in [(560.0f32, 360.0f32), (40.0, 40.0), (300.0, 200.0)] {
        for k in 0..8 {
            let ang = k as f32 * std::f32::consts::TAU / 8.0;
            let b = (centre.0 + ang.cos() * 120.0, centre.1 + ang.sin() * 120.0);
            let li = place_focus(&mut app, centre, b);
            let spec = app.doc.layers[li].genlines.unwrap();
            for (mode, p) in gen_handle_points(&spec, size) {
                assert!(
                    page(p),
                    "{mode:?} handle off the page at {p:?} (centre {centre:?}, {k}/8)"
                );
            }
            // The sweep moves the ANGLE, never the radius: the drag reads
            // distance from the centre, so a handle at the wrong radius
            // would make the grab jump. That holds while the ring FITS —
            // but the 2026-08-24 reach defaults can exceed the
            // centre-to-farthest-corner distance, where no angle of the
            // ring is on the paper at all and the handle falls back to
            // the ray (on-page, grabbable; a drag re-sets d). There only
            // the on-page assert above applies.
            let far = [
                [0.0, 0.0],
                [size.0 as f32, 0.0],
                [0.0, size.1 as f32],
                [size.0 as f32, size.1 as f32],
            ]
            .iter()
            .map(|c| (c[0] - spec.a).hypot(c[1] - spec.b))
            .fold(0.0f32, f32::max);
            for (mode, p) in gen_handle_points(&spec, size) {
                let want = match mode {
                    crate::app::canvas_input::GenDragMode::RIn => spec.c,
                    crate::app::canvas_input::GenDragMode::ROut if spec.d <= far => spec.d,
                    _ => continue,
                };
                let r = (p[0] - spec.a).hypot(p[1] - spec.b);
                assert!(
                    (r - want).abs() < 1.0,
                    "{mode:?} moved off its radius ({r} vs {want})"
                );
            }
        }
    }

    // A stream run anchors its reference line on the DRAG, not the canvas
    // centre — a reference drawn where the gesture never went is not one.
    app.tool = Tool::Figure;
    app.figure_mode = FigureMode::Stream;
    app.finish_figure_drag((80.0, 60.0), (220.0, 100.0));
    drain(&mut app);
    let li = app.doc.active;
    let spec = app.doc.layers[li].genlines.unwrap();
    assert_eq!(
        spec.anchor,
        Some([150.0, 80.0]),
        "the reference sits at the drag's midpoint"
    );
    for (mode, p) in gen_handle_points(&spec, app.doc.size) {
        assert!(page(p), "{mode:?} stream handle off the page at {p:?}");
    }
}

/// The Tool Property editor's commit: a width change on the SELECTED
/// layer re-rasterizes that layer in place, keeps its spec and its stack
/// position, and costs one undo press.
#[test]
fn apply_to_regenerates_the_selected_run() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let li = place_focus(&mut app, (300.0, 200.0), (300.0, 60.0));
    // A second run on top, so "the selected one" is not "the active one"
    // by accident.
    let top = place_focus(&mut app, (120.0, 300.0), (120.0, 240.0));
    assert_ne!(li, top);

    let before = fingerprint(&app, li);
    let name = app.doc.layers[li].name.clone();
    let mut spec = app.doc.layers[li].genlines.unwrap();
    spec.width *= 3.0;
    let steps = app.doc.undo_len();
    dispatch(&mut app, AppCmd::GenLinesApplyTo { layer: li, spec });

    assert_eq!(
        app.doc.layers[li].genlines,
        Some(spec),
        "the layer carries what was applied"
    );
    assert_ne!(before, fingerprint(&app, li), "and the raster followed");
    assert_eq!(app.doc.layers[li].name, name, "regenerated IN PLACE");
    assert_eq!(app.doc.undo_len(), steps + 1, "one undo press");
    assert!(app.doc.undo(), "which undoes");
    assert_eq!(fingerprint(&app, li), before, "back to the old pixels");

    // A layer that was never generated refuses, rather than inventing a
    // spec for it.
    let plain = app.doc.add_layer("plain");
    dispatch(&mut app, AppCmd::GenLinesApplyTo { layer: plain, spec });
    assert_eq!(app.doc.layers[plain].genlines, None);
}

/// The angular pitch between neighbouring rays, read off a ring of ink —
/// the same measurement `mn-core`'s `radial_grouping_leaves_angular_holes`
/// makes, done here on a real placed LAYER instead of a renderer call.
///
/// Walk a circle of radius `r` about the centre, note where each run of ink
/// starts and stops, and return the angle between consecutive run centres.
/// An unbundled burst gives one number repeated; a bundled one gives tight
/// gaps in runs of `group` with a big hole after each.
fn ray_gaps(app: &App, li: usize, c: [f32; 2], r: f32) -> Vec<f32> {
    const N: usize = 3600;
    let hit: Vec<bool> = (0..N)
        .map(|k| {
            let a = k as f32 * std::f32::consts::TAU / N as f32;
            let (s, cs) = a.sin_cos();
            ink_at(app, li, (c[0] + cs * r) as i32, (c[1] + s * r) as i32)
        })
        .collect();
    let mut centres = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    for (k, on) in hit.iter().enumerate() {
        if *on {
            run = Some(match run {
                Some((a, _)) => (a, k),
                None => (k, k),
            });
        } else if let Some((a, b)) = run.take() {
            centres.push((a + b) as f32 * 0.5);
        }
    }
    if let Some((a, b)) = run {
        centres.push((a + b) as f32 * 0.5);
    }
    centres
        .windows(2)
        .map(|w| (w[1] - w[0]) * 360.0 / N as f32)
        .collect()
}

/// Owner, 2026-09-06: effect lines "doesn't seem to have a grouping
/// setting". He was right — まとまり only ever existed in the SPEED walk, so
/// a 集中線 came out at one even pitch whatever the palette did, and the
/// Tool Property section did not offer the knob at all for a radial set.
///
/// This is the Object-tool half of the fix: the Density section's Bundle and
/// Bundle-gap bars on a SELECTED focus set commit through
/// `AppCmd::GenLinesApplyTo` (that is all `gen_commit` does), and the layer
/// re-rasterizes with real angular holes in it.
#[test]
fn object_panel_edits_radial_bundle() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let li = place_focus(&mut app, (300.0, 200.0), (300.0, 80.0));

    // A CLEAN set to measure: an even 3° pitch, no wobble of any kind, a
    // flat 2 px width, and a reach that stays on the page. Every wobble is
    // switched off on purpose — this test is about the pitch, and a
    // position wobble of a quarter of the gap would blur the very thing
    // being counted. (The shipped preset carries all of them; that is what
    // makes the sets look hand-made, and what makes them unmeasurable.)
    let placed = app.doc.layers[li].genlines.expect("the drag placed a spec");
    let even = mn_core::genlines::GenLinesSpec {
        c: 60.0,
        d: 300.0,
        gap_deg: 3.0,
        width: 2.0,
        taper: 0.0,
        entry: 0.0,
        needle: 0.0,
        jitter: 0.0,
        jit_gap: 0.0,
        jit_len: 0.0,
        jit_width: 0.0,
        jit_len_out: 0.0,
        core_jit: 0.0,
        len_skew: 0.0,
        accent_frac: 0.0,
        group: 0,
        group_gap: 0.0,
        group_jit: 0.0,
        ..placed
    };
    dispatch(
        &mut app,
        AppCmd::GenLinesApplyTo {
            layer: li,
            spec: even,
        },
    );
    let flat = ray_gaps(&app, li, [300.0, 200.0], 150.0);
    assert!(flat.len() > 30, "enough rays to see a pattern ({flat:?})");
    let hi = flat.iter().copied().fold(0.0f32, f32::max);
    let lo = flat.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(
        hi - lo < 0.6,
        "BEFORE: one even pitch, no bundles anywhere ({lo}..{hi})"
    );

    // Now the two bars the palette gained: Bundle 4, Bundle gap 3×.
    let bundled = mn_core::genlines::GenLinesSpec {
        group: 4,
        group_gap: 3.0,
        ..even
    };
    let steps = app.doc.undo_len();
    dispatch(
        &mut app,
        AppCmd::GenLinesApplyTo {
            layer: li,
            spec: bundled,
        },
    );
    assert_eq!(
        app.doc.layers[li].genlines,
        Some(bundled),
        "the layer carries the bundling"
    );
    assert_eq!(app.doc.undo_len(), steps + 1, "one undo press, as before");

    let gaps = ray_gaps(&app, li, [300.0, 200.0], 150.0);
    assert!(gaps.len() > 20, "still a burst ({} rays)", gaps.len() + 1);
    // 4 rays a 3° apart, then a hole of 3 × 3° = 9°.
    let tight = gaps.iter().filter(|g| (**g - 3.0).abs() < 0.6).count();
    let holes = gaps.iter().filter(|g| (**g - 9.0).abs() < 1.0).count();
    assert!(
        holes >= 8,
        "AFTER: the bundles stand apart ({holes} holes in {gaps:?})"
    );
    assert!(
        tight >= holes,
        "and each bundle is several tight rays ({tight} tight, {holes} holes)"
    );

    // And it undoes back to the even set, pixels and parameters together.
    assert!(app.doc.undo(), "the bundling undoes");
    assert_eq!(app.doc.layers[li].genlines, Some(even));
    let back = ray_gaps(&app, li, [300.0, 200.0], 150.0);
    let hi = back.iter().copied().fold(0.0f32, f32::max);
    let lo = back.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(hi - lo < 0.6, "the even pitch is back ({lo}..{hi})");
}
