//! Mangaka-basics QA, toning + fill family (2026-09-02 round).
//!
//! These drive the REAL doors a mangaka uses — tool select, pointer
//! down/move/up on the canvas, Tool Property edits, undo — and render the
//! result through the EXPORT renderer (`render_offscreen_drafts_off`), so
//! what the asserts measure is what the printed page would show, not an
//! internal structure that happens to be right.
//!
//! The line art is drawn with the PEN, which means it is anti-aliased:
//! a bucket fill that only stops at fully opaque ink leaves a light halo
//! between the flat and the line, and that halo is the single most
//! visible fill defect in finished manga. `halo_ring` below measures it.

use super::new_document_tests::headless;
use crate::app::{App, PenSample, PointerKind};
use crate::cmd::{AppCmd, Tool, dispatch};
use mn_core::tile::TileIdx;

/// A page small enough that a canvas-sized flood is instant, big enough
/// that a 600 dpi / 60 LPI screen (10 px cells) resolves into real dots.
const W: u32 = 256;
const H: u32 = 256;

/// The ink square the flows fill inside of, in canvas pixels.
const BOX0: f32 = 48.0;
const BOX1: f32 = 208.0;

fn page(app: &mut App) {
    app.doc = mn_core::Document::new(W, H);
    app.doc.layers[0].name = "lineart".into();
}

/// Canvas point → the SCREEN point the pointer arms take.
fn s(app: &App, cx: f32, cy: f32) -> (f32, f32) {
    app.viewport.to_screen(cx, cy)
}

/// Drain the queue the pointer arms push into (a canvas click on the Fill
/// tool posts `AppCmd::Fill`; nothing runs until the frame pumps it).
fn pump(app: &mut App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

/// Click once at a canvas point through the real arms, then pump.
fn click(app: &mut App, cx: f32, cy: f32) {
    let (x, y) = s(app, cx, cy);
    app.canvas_down(x, y, PointerKind::Mouse, &[]);
    app.canvas_up(x, y, &[]);
    pump(app);
}

/// Draw one straight pen stroke through the real pointer path, so the ink
/// is the brush engine's own anti-aliased ribbon.
fn ink_stroke(app: &mut App, from: (f32, f32), to: (f32, f32)) {
    let steps = 48;
    let (dx, dy) = ((to.0 - from.0) / steps as f32, (to.1 - from.1) / steps as f32);
    let (x0, y0) = s(app, from.0, from.1);
    app.canvas_down(x0, y0, PointerKind::Mouse, &[]);
    for i in 1..=steps {
        let (mx, my) = s(app, from.0 + dx * i as f32, from.1 + dy * i as f32);
        app.canvas_move(
            mx,
            my,
            &[PenSample {
                x: mx,
                y: my,
                pressure: 0.9,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            }],
        );
    }
    let (ux, uy) = s(app, to.0, to.1);
    app.canvas_up(ux, uy, &[]);
    pump(app);
}

/// A closed square of anti-aliased pen ink on the active layer, with an
/// optional `gap`-px hole in the middle of its bottom edge.
fn ink_square(app: &mut App, size_px: f32, gap: f32) {
    dispatch(app, AppCmd::SetTool(Tool::Pen));
    dispatch(app, AppCmd::SetBrushSizePx(size_px));
    dispatch(app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    ink_stroke(app, (BOX0, BOX0), (BOX1, BOX0));
    ink_stroke(app, (BOX1, BOX0), (BOX1, BOX1));
    ink_stroke(app, (BOX0, BOX0), (BOX0, BOX1));
    if gap <= 0.0 {
        ink_stroke(app, (BOX0, BOX1), (BOX1, BOX1));
    } else {
        let mid = (BOX0 + BOX1) / 2.0;
        ink_stroke(app, (BOX0, BOX1), (mid - gap / 2.0, BOX1));
        ink_stroke(app, (mid + gap / 2.0, BOX1), (BOX1, BOX1));
    }
}

/// The flatting stack a mangaka actually builds: line art ON TOP, the
/// fill layer UNDER it, so an overfill hides beneath the line instead of
/// eating it. (CSP's own flatting recipe; ours has no "new layer below"
/// door, which is logged in the ledger — the fixture places it directly.)
fn flats_under_lineart(app: &mut App) -> usize {
    let li = app.doc.add_layer("flats");
    let l = app.doc.layers.remove(li);
    app.doc.layers.insert(0, l);
    app.doc.set_active(0);
    app.doc.revision = mn_core::tile::next_revision();
    0
}

/// Render this page the way an export would and drop the PNG where the
/// agent can look at it.
fn shot(app: &mut App, name: &str) -> image::RgbaImage {
    let dir = std::env::temp_dir().join(format!("mn-qa2-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let (w, h) = (app.doc.size.0, app.doc.size.1);
    let App { renderer, doc, .. } = app;
    let img = super::pages::render_offscreen_drafts_off(renderer, doc, w, h);
    let p = dir.join(format!("{name}.png"));
    img.save(&p).expect("write the shot");
    println!("[shot] {}", p.display());
    img
}

/// Luma of the exported pixel (0 = black ink, 255 = paper).
fn luma(img: &image::RgbaImage, x: u32, y: u32) -> u8 {
    let p = img.get_pixel(x, y).0;
    let a = p[3] as f32 / 255.0;
    let mix = |c: u8| c as f32 * a + 255.0 * (1.0 - a);
    ((mix(p[0]) * 0.2126) + (mix(p[1]) * 0.7152) + (mix(p[2]) * 0.0722)) as u8
}

/// THE halo test. Along scanline `y`, count the widest run of pixels
/// between `from_x` and `to_x` that read as PAPER — the white rim a fill
/// leaves when it stops at the outside of an anti-aliased line.
fn paper_run(img: &image::RgbaImage, y: u32, from_x: u32, to_x: u32) -> u32 {
    let (lo, hi) = (from_x.min(to_x), from_x.max(to_x));
    let (mut worst, mut run) = (0, 0);
    for x in lo..=hi {
        if luma(img, x, y) > 200 {
            run += 1;
            worst = u32::max(worst, run);
        } else {
            run = 0;
        }
    }
    worst
}

/// Alpha of one derived pixel of a layer (live layers have no painted
/// pixels — `display_tile` is the derived raster).
fn alpha(app: &App, li: usize, x: i32, y: i32) -> u16 {
    let ti = TileIdx::of_pixel(x, y);
    app.doc.layers[li]
        .display_tile(ti)
        .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize)[3])
        .unwrap_or(0)
}

/// Does this rectangle read as a SCREEN — some ink, some paper — rather
/// than a flat slab? Returns (inked, clear).
fn dot_census(img: &image::RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> (u32, u32) {
    let (mut inked, mut clear) = (0, 0);
    for y in y0..y1 {
        for x in x0..x1 {
            if luma(img, x, y) < 128 {
                inked += 1;
            } else {
                clear += 1;
            }
        }
    }
    (inked, clear)
}

// ---------------------------------------------------------------------
// Flow A — the bucket on anti-aliased line art
// ---------------------------------------------------------------------

/// Click the Fill tool inside a pen-drawn square with the fill layer under
/// the line art. Three things a mangaka checks: it filled, it stopped at
/// the line, and there is NO white rim between the flat and the line.
#[test]
fn qa_bucket_on_antialiased_lineart_leaves_no_halo() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    ink_square(&mut app, 9.0, 0.0);
    flats_under_lineart(&mut app);

    dispatch(&mut app, AppCmd::SetTool(Tool::Fill));
    dispatch(&mut app, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));
    click(&mut app, 128.0, 128.0);

    let img = shot(&mut app, "A-bucket-aa");
    assert!(luma(&img, 128, 128) < 160, "the click filled the square");
    assert!(luma(&img, 8, 8) > 200, "the fill stayed inside the square");

    // Between the outside of the left wall and the middle of the box
    // there must be no paper-coloured run at all: ink, then flat.
    let rim = paper_run(&img, 128, BOX0 as u32, 128);
    println!("[halo] inside the left wall, paper run = {rim} px");
    assert!(rim == 0, "a {rim} px white rim sits between the flat and the line");
}

/// The same click with Area scaling pulled to 0 — the number CSP ships as
/// the "no overfill" setting. This is the DIFFERENTIAL that says whether
/// our fill tucks under an anti-aliased line on its own geometry or only
/// because the default +1 papers over it.
#[test]
fn qa_area_scaling_zero_still_reaches_the_line_core() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    ink_square(&mut app, 9.0, 0.0);
    flats_under_lineart(&mut app);

    dispatch(&mut app, AppCmd::SetTool(Tool::Fill));
    dispatch(&mut app, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));

    let mut area = |n: i32, name: &str| -> usize {
        let mut opts = app.fill_opts;
        opts.expand_px = n;
        dispatch(&mut app, AppCmd::SetFillOpts(opts));
        click(&mut app, 128.0, 128.0);
        let img = shot(&mut app, name);
        let rim = paper_run(&img, 128, BOX0 as u32, 128);
        let px: usize = app
            .status
            .split_whitespace()
            .nth(1)
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        println!("[area] scaling {n:+}: filled {px} px, paper run inside the wall = {rim} px");
        assert!(rim == 0, "area scaling {n:+} left a {rim} px white rim");
        dispatch(&mut app, AppCmd::Undo);
        px
    };
    let zero = area(0, "A2-area-scaling-0");
    let wide = area(6, "A3-area-scaling-6");
    assert!(
        wide > zero,
        "the Area scaling row must reach the fill (+0 = {zero} px, +6 = {wide} px)"
    );
}

/// The 参照 row. "Refer other layers" (our default, and CSP's) sees the
/// line art on the layer below; "Editing layer" does not and floods the
/// whole page. Both are correct — this pins that the ROW changes the
/// answer, which is the beginner trap CSP's default exists to dodge.
#[test]
fn qa_the_refer_row_decides_whether_the_lineart_is_a_wall() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    ink_square(&mut app, 9.0, 0.0);
    flats_under_lineart(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Fill));
    dispatch(&mut app, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));

    assert_eq!(
        app.fill_opts.refer,
        mn_core::FillRefer::All,
        "the shipped default is CSP's Refer other layers"
    );
    click(&mut app, 128.0, 128.0);
    let refer_all = shot(&mut app, "A4-refer-other-layers");
    assert!(luma(&refer_all, 8, 8) > 200, "referring to other layers, the line walls");
    dispatch(&mut app, AppCmd::Undo);

    let mut opts = app.fill_opts;
    opts.refer = mn_core::FillRefer::Active;
    dispatch(&mut app, AppCmd::SetFillOpts(opts));
    click(&mut app, 128.0, 128.0);
    let refer_active = shot(&mut app, "A5-refer-editing-layer");
    assert!(
        luma(&refer_active, 8, 8) < 200,
        "on the editing layer alone the empty flats layer has no walls, so the page floods"
    );
}

/// **The bucket used to go silent.** Every other fill door — enclose,
/// leftover, lasso, and the Tone tool — says so when it writes nothing.
/// The plain click said nothing at all, which reads as a dead tool.
///
/// The way a mangaka reaches it: a selection is up (flatting one
/// character at a time is exactly that workflow) and the next click lands
/// OUTSIDE it. The flood runs, the selection clips every pixel of it away,
/// and the bucket writes nothing — with no word about why.
#[test]
fn qa_a_bucket_click_that_fills_nothing_says_so() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    ink_square(&mut app, 9.0, 0.0);
    flats_under_lineart(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Fill));
    dispatch(&mut app, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));

    // A selection INSIDE the square; the click lands on the background
    // outside it, so the flooded region and the selection do not touch.
    app.doc.selection = Some(mn_core::Selection::from_rect(&app.doc, 64.0, 64.0, 192.0, 192.0));
    app.set_status("");
    click(&mut app, 16.0, 16.0);

    let img = shot(&mut app, "J-clipped-away-fill");
    assert!(luma(&img, 16, 16) > 200, "the selection clipped the whole fill away");
    assert!(luma(&img, 128, 128) > 200, "…and nothing landed inside it either");
    println!("[silent] status after a fill the selection ate: {:?}", app.status);
    assert!(
        !app.status.is_empty(),
        "a bucket click that writes nothing must say so, not go silent"
    );
    assert!(
        !app.status.starts_with("filled"),
        "…and it must not claim it filled something: {:?}",
        app.status
    );
}

// ---------------------------------------------------------------------
// Flow B — the leak, Close gap, and the leak-repair refill
// ---------------------------------------------------------------------

/// A 20 px hole in the bottom edge. The default 2 px Close gap cannot
/// seal it, so the fill escapes; raising Close gap seals it; and the
/// leak-repair command undoes the leak and re-fills behind one stroke.
#[test]
fn qa_a_leak_is_visible_then_closed_then_repaired() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    ink_square(&mut app, 9.0, 20.0);
    flats_under_lineart(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Fill));
    dispatch(&mut app, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));

    // 1. The leak.
    click(&mut app, 128.0, 128.0);
    let leaked = shot(&mut app, "B1-leak");
    assert!(luma(&leaked, 8, 8) < 200, "the fill escaped through the gap");

    // 2. Close gap wide enough to bridge it.
    dispatch(&mut app, AppCmd::Undo);
    let mut opts = app.fill_opts;
    opts.gap_close_px = 14;
    dispatch(&mut app, AppCmd::SetFillOpts(opts));
    click(&mut app, 128.0, 128.0);
    let closed = shot(&mut app, "B2-close-gap");
    assert!(luma(&closed, 8, 8) > 200, "Close gap 14 sealed a 20 px hole");
    assert!(luma(&closed, 128, 128) < 160, "…and the inside is still filled");
    // The bite Close gap leaves. A wide gap-close bridge that held the
    // flat back by its own radius would leave a 14 px white notch across
    // the hole; what the artist may see is the hole in the LINE itself,
    // one row deep, which is the same thing CSP leaves.
    let notch = |img: &image::RgbaImage, y: u32| {
        (60..196).filter(|x| luma(img, *x, y) > 200).count()
    };
    for y in [(BOX1 as u32) - 3, BOX1 as u32] {
        println!("[notch] y={y}: {} paper px along the bottom band", notch(&closed, y));
    }
    assert_eq!(
        notch(&closed, BOX1 as u32 - 3),
        0,
        "Close gap must not hold the flat back by its own radius"
    );

    // 3. Leak repair: back to the leaking settings, leak, arm, draw the
    //    closing stroke, and the fill re-runs itself contained.
    dispatch(&mut app, AppCmd::Undo);
    let mut opts = app.fill_opts;
    opts.gap_close_px = 2;
    dispatch(&mut app, AppCmd::SetFillOpts(opts));
    click(&mut app, 128.0, 128.0);
    assert!(luma(&shot(&mut app, "B3-leak-again"), 8, 8) < 200, "leaking again");

    dispatch(&mut app, AppCmd::ArmFillRepair { virtual_barrier: true });
    let mid = (BOX0 + BOX1) / 2.0;
    let (dx, dy) = s(&app, mid - 16.0, BOX1);
    app.canvas_down(dx, dy, PointerKind::Mouse, &[]);
    for x in [-10.0, -4.0, 2.0, 8.0, 14.0] {
        let (mx, my) = s(&app, mid + x, BOX1);
        app.canvas_move(mx, my, &[]);
    }
    let (ux, uy) = s(&app, mid + 16.0, BOX1);
    app.canvas_up(ux, uy, &[]);
    pump(&mut app);

    let fixed = shot(&mut app, "B4-repaired");
    assert!(luma(&fixed, 8, 8) > 200, "the repaired fill stayed inside");
    assert!(luma(&fixed, 128, 128) < 160, "…and the inside is filled");
    assert!(app.fill_repair.is_none(), "the repair gesture closed itself");
}

// ---------------------------------------------------------------------
// Flow C — the Tone tool: one click screens a region
// ---------------------------------------------------------------------

/// Pick the Tone tool, click inside the square, and the page must come
/// back as DOTS at the tool's lpi / density / angle — not a grey slab.
#[test]
fn qa_the_tone_tool_screens_a_region_in_one_click() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    ink_square(&mut app, 9.0, 0.0);
    let before = app.doc.layers.len();

    dispatch(&mut app, AppCmd::SetTool(Tool::Tone));
    click(&mut app, 128.0, 128.0);
    app.refresh_tones();

    assert_eq!(app.doc.layers.len(), before + 1, "one click, one tone layer");
    let li = app.doc.active;
    assert!(
        matches!(app.doc.layers[li].kind, mn_core::LayerKind::Fill(mn_core::FillKind::Tone { .. })),
        "a LIVE tone layer, editable a week later: {:?}",
        app.doc.layers[li].kind
    );
    assert!(app.doc.selection.is_none(), "the gesture left no selection to clear");

    let img = shot(&mut app, "C-tone-tool");
    let (inked, clear) = dot_census(&img, 80, 80, 176, 176);
    println!("[tone] inside the square: {inked} inked, {clear} clear");
    assert!(inked > 0 && clear > 0, "dots, not a slab ({inked}/{clear})");
    // 40 % density: the ink share must land in the same neighbourhood.
    let share = inked as f32 / (inked + clear) as f32;
    assert!(
        (0.2..0.6).contains(&share),
        "the screen printed {share:.2} coverage at 40 % density"
    );
    // ONE undo press takes the whole gesture back.
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers.len(), before, "one Ctrl+Z, no tone layer");
}

// ---------------------------------------------------------------------
// Flow D — the Selection Launcher's "New tone", and the EXPORT
// ---------------------------------------------------------------------

/// Select a region, press the launcher's New tone button, and check the
/// dots survive the export renderer — the tone is derived, so an export
/// that forgets to re-derive would print a blank or a stale slab.
#[test]
fn qa_launcher_tone_screens_and_survives_the_export() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    app.doc.selection = Some(mn_core::Selection::from_rect(&app.doc, 32.0, 32.0, 160.0, 160.0));

    dispatch(&mut app, crate::ui::launcher::new_tone_cmd());
    app.refresh_tones();
    let li = app.doc.active;

    let img = shot(&mut app, "D-launcher-tone");
    let (inked, clear) = dot_census(&img, 40, 40, 184, 184);
    println!("[tone] launcher tone on export: {inked} inked, {clear} clear");
    assert!(inked > 0 && clear > 0, "the exported page shows dots ({inked}/{clear})");
    // Outside the marching ants the paper is untouched.
    assert!(luma(&img, 220, 220) > 200, "nothing was screened outside the selection");
    assert!(alpha(&app, li, 220, 220) == 0, "…and the window mask says so");
}

/// The three Layer Property knobs on a live tone (frequency, density,
/// angle) each change the printed page, and each is undoable.
#[test]
fn qa_live_tone_knobs_change_the_print_and_undo() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    app.doc.selection = Some(mn_core::Selection::from_rect(&app.doc, 32.0, 32.0, 224.0, 224.0));
    dispatch(&mut app, crate::ui::launcher::new_tone_cmd());
    app.refresh_tones();
    let li = app.doc.active;
    let mn_core::LayerKind::Fill(base) = app.doc.layers[li].kind else {
        panic!("a live tone layer");
    };
    let before = shot(&mut app, "E0-tone-default");
    let (i0, c0) = dot_census(&before, 40, 40, 216, 216);

    // Density 40 % → 80 %: strictly more ink.
    let mn_core::FillKind::Tone { tone, .. } = base else { panic!() };
    dispatch(&mut app, AppCmd::SetFillParams(li, mn_core::FillKind::Tone { tone, density: 0.8 }));
    app.refresh_tones();
    let dense = shot(&mut app, "E1-tone-density-80");
    let (i1, _) = dot_census(&dense, 40, 40, 216, 216);
    println!("[tone] density 40 % = {i0} inked, 80 % = {i1} inked (of {})", i0 + c0);
    assert!(i1 > i0, "raising density printed more ink ({i0} → {i1})");

    // Frequency 60 → 20 LPI: far fewer, far bigger dots. The COUNT of
    // ink/paper transitions along a scanline is the honest measure.
    let coarse_tone = mn_core::tone::ToneParams { lpi: 20.0, ..tone };
    dispatch(
        &mut app,
        AppCmd::SetFillParams(li, mn_core::FillKind::Tone { tone: coarse_tone, density: 0.4 }),
    );
    app.refresh_tones();
    let coarse = shot(&mut app, "E2-tone-20lpi");
    let edges = |img: &image::RgbaImage| {
        (41..216).filter(|x| (luma(img, *x, 128) < 128) != (luma(img, x - 1, 128) < 128)).count()
    };
    println!("[tone] 60 LPI = {} edges, 20 LPI = {} edges", edges(&before), edges(&coarse));
    assert!(edges(&coarse) < edges(&before), "20 LPI is a coarser screen than 60");

    // Angle 45° → 0°: the lattice moves, so the page is not identical.
    let flat_tone = mn_core::tone::ToneParams { angle_deg: 0.0, ..tone };
    dispatch(
        &mut app,
        AppCmd::SetFillParams(li, mn_core::FillKind::Tone { tone: flat_tone, density: 0.4 }),
    );
    app.refresh_tones();
    let turned = shot(&mut app, "E3-tone-0deg");
    assert!(turned.as_raw() != before.as_raw(), "the screen angle moved the dots");

    // Undo walks the parameter edits back to the layer we started with.
    for _ in 0..8 {
        dispatch(&mut app, AppCmd::Undo);
        if let Some(mn_core::LayerKind::Fill(k)) = app.doc.layers.get(li).map(|l| l.kind.clone()) {
            if k == base {
                break;
            }
        }
    }
    app.refresh_tones();
    let mn_core::LayerKind::Fill(now) = app.doc.layers[li].kind else { panic!() };
    assert_eq!(now, base, "undo walked the tone parameters back");
}

// ---------------------------------------------------------------------
// Flow F — the raster tone EFFECT on painted ink
// ---------------------------------------------------------------------

/// `Layer ▸ Layer property ▸ Effect ▸ Tone` on a layer of painted grey:
/// the grey must come back as a halftone on the exported page.
#[test]
fn qa_the_raster_tone_effect_screens_painted_ink_on_export() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    // A slab of mid grey through the real brush.
    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    dispatch(&mut app, AppCmd::SetBrushSizePx(40.0));
    dispatch(&mut app, AppCmd::SetSlotColor([0.5, 0.5, 0.5]));
    for y in (64..192).step_by(16) {
        ink_stroke(&mut app, (64.0, y as f32), (192.0, y as f32));
    }
    let plain = shot(&mut app, "F0-grey-slab");

    dispatch(&mut app, AppCmd::SetTone(Some(mn_core::ToneParams::default())));
    app.refresh_tones();
    let toned = shot(&mut app, "F1-grey-toned");
    let (inked, clear) = dot_census(&toned, 80, 80, 176, 176);
    // The busiest scanline in the slab: a flat grey has NO ink/paper
    // transitions anywhere, a screen has many on almost every row.
    let edges = |img: &image::RgbaImage| {
        (80..176)
            .map(|y| (81..176).filter(|x| (luma(img, *x, y) < 128) != (luma(img, x - 1, y) < 128)).count())
            .max()
            .unwrap_or(0)
    };
    println!(
        "[effect] plain slab edges={}, toned edges={}, toned {inked} inked / {clear} clear",
        edges(&plain),
        edges(&toned)
    );
    assert!(inked > 0 && clear > 0, "the effect screened the grey ({inked}/{clear})");
    assert!(
        edges(&toned) > edges(&plain) + 4,
        "…a flat grey became a row of DOTS, not a flat grey"
    );
}

// ---------------------------------------------------------------------
// Flow G — gradient layers
// ---------------------------------------------------------------------

/// Drag the Gradient tool with the live switch on: a gradient LAYER, its
/// ramp visible on the exported page, dark at one end and light at the
/// other, and one undo press takes it away.
#[test]
fn qa_a_gradient_layer_ramps_across_the_page_and_undoes() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    app.fill_live = true;
    let before = app.doc.layers.len();
    dispatch(&mut app, AppCmd::SetTool(Tool::Gradient));
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    app.sub_color = [1.0, 1.0, 1.0];

    let (dx, dy) = s(&app, 24.0, 128.0);
    app.canvas_down(dx, dy, PointerKind::Mouse, &[]);
    for x in [64.0, 128.0, 192.0] {
        let (mx, my) = s(&app, x, 128.0);
        app.canvas_move(mx, my, &[]);
    }
    let (ux, uy) = s(&app, 232.0, 128.0);
    app.canvas_up(ux, uy, &[]);
    pump(&mut app);
    app.refresh_tones();

    assert_eq!(app.doc.layers.len(), before + 1, "one drag, one gradient layer");
    let li = app.doc.active;
    assert!(
        matches!(app.doc.layers[li].kind, mn_core::LayerKind::Fill(mn_core::FillKind::Gradient { .. })),
        "a LIVE gradient layer: {:?}",
        app.doc.layers[li].kind
    );

    let img = shot(&mut app, "G-gradient-layer");
    let (dark, light) = (luma(&img, 32, 128), luma(&img, 224, 128));
    println!("[grad] left={dark} right={light}");
    assert!(dark + 40 < light, "the ramp runs dark → light ({dark} → {light})");
    let mid = luma(&img, 128, 128);
    assert!(dark < mid && mid < light, "…and it is a RAMP, not two halves");

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers.len(), before, "one Ctrl+Z takes the gradient back");
}

// ---------------------------------------------------------------------
// Flow H — filling a selection
// ---------------------------------------------------------------------

/// `Edit ▸ Fill` with a selection up paints exactly the selection, with no
/// flood and no regard for line art.
#[test]
fn qa_fill_selection_paints_the_ants_and_nothing_else() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    app.doc.selection = Some(mn_core::Selection::from_rect(&app.doc, 64.0, 64.0, 192.0, 192.0));
    dispatch(&mut app, AppCmd::FillSelection);

    let img = shot(&mut app, "H-fill-selection");
    assert!(luma(&img, 128, 128) < 32, "inside the ants is solid");
    assert!(luma(&img, 32, 32) > 200, "outside them is paper");
    assert!(luma(&img, 66, 66) < 32, "the corner of the ants filled too");
    dispatch(&mut app, AppCmd::Undo);
    let back = shot(&mut app, "H2-fill-selection-undone");
    assert!(luma(&back, 128, 128) > 200, "one Ctrl+Z clears it");
}

// ---------------------------------------------------------------------
// Flow I — fill on a real comic page (frame folder present)
// ---------------------------------------------------------------------

/// The page a mangaka actually opens: `File ▸ New comic` seeds a frame
/// folder. Clicking the bucket with the FRAME layer active must say
/// something useful rather than silently doing nothing, and the fill must
/// work the moment a raster layer is picked.
#[test]
fn qa_the_bucket_on_a_frame_folder_page_says_what_to_do() {
    let Some(mut app) = headless() else { return };
    super::new_document_tests::small_draft(&mut app, 1, "");
    dispatch(&mut app, AppCmd::NewComicCreate);
    dispatch(&mut app, AppCmd::SetTool(Tool::Fill));
    dispatch(&mut app, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));

    // Stand on the frame folder itself — what a fresh page leaves active.
    let framey = app.doc.layers.iter().position(|l| l.is_frame());
    if let Some(fi) = framey {
        app.doc.set_active(fi);
        app.status.clear();
        let (w, h) = (app.doc.size.0 as f32, app.doc.size.1 as f32);
        click(&mut app, w / 2.0, h / 2.0);
        println!("[frame] status after a bucket click on the frame layer: {:?}", app.status);
        assert!(
            !app.status.is_empty(),
            "a bucket click on a frame layer must SAY something, not fail silently"
        );
    }

    // A raster layer above the frames: the same click fills.
    let li = app.doc.add_layer("flats");
    app.doc.set_active(li);
    let (w, h) = (app.doc.size.0 as f32, app.doc.size.1 as f32);
    click(&mut app, w / 2.0, h / 2.0);
    shot(&mut app, "I-comic-page-fill");
    let inked: u64 = app.doc.layers[li].tiles().map(|(_, t)| t.alpha_sum()).sum();
    println!("[frame] fill on a raster layer inside a comic page inked {inked}");
    assert!(inked > 0, "the bucket works on a comic page once a raster layer is active");
}

// ---------------------------------------------------------------------
// Flow K — the promise: "parameters, not pixels, editable a week later"
// ---------------------------------------------------------------------

/// Save the page and load it back: a live tone must return as a LIVE tone
/// with the same lpi / density / angle, not as a baked raster. This is the
/// whole reason to prefer a tone layer over a screened fill, and nothing
/// else in the suite checks it through the file.
#[test]
fn qa_a_live_tone_is_still_editable_after_a_save_and_load() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    app.doc.selection = Some(mn_core::Selection::from_rect(&app.doc, 32.0, 32.0, 200.0, 200.0));
    dispatch(&mut app, crate::ui::launcher::new_tone_cmd());
    let li = app.doc.active;
    let tuned = mn_core::tone::ToneParams { lpi: 42.5, angle_deg: 15.0, ..Default::default() };
    dispatch(
        &mut app,
        AppCmd::SetFillParams(li, mn_core::FillKind::Tone { tone: tuned, density: 0.65 }),
    );
    app.refresh_tones();
    let before = shot(&mut app, "K0-tone-before-save");

    let bytes = mn_core::project::doc_to_bytes(&app.doc).expect("the page encodes");
    app.doc = mn_core::project::bytes_to_doc(&bytes).expect("the page decodes");
    app.refresh_tones();
    app.renderer.invalidate();

    let kind = app.doc.layers[li].kind.clone();
    let mn_core::LayerKind::Fill(mn_core::FillKind::Tone { tone, density }) = kind else {
        panic!("the tone came back as {:?}, not a live tone", app.doc.layers[li].kind);
    };
    assert_eq!(tone.lpi, 42.5, "the frequency survived the file");
    assert_eq!(tone.angle_deg, 15.0, "the angle survived the file");
    assert!((density - 0.65).abs() < 1e-6, "the density survived the file");

    let after = shot(&mut app, "K1-tone-after-load");
    assert_eq!(before.as_raw(), after.as_raw(), "and it prints identically");
}

/// The Layer Property palette must show a live GRADIENT layer's own rows,
/// the way it shows a live tone's (friction 6's fix covers "any live
/// layer" — this is the other half of that claim, on the real widget).
#[test]
fn qa_layer_property_shows_a_gradient_layers_rows() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    app.fill_live = true;
    dispatch(&mut app, AppCmd::SetTool(Tool::Gradient));
    let (dx, dy) = s(&app, 24.0, 128.0);
    app.canvas_down(dx, dy, PointerKind::Mouse, &[]);
    let (mx, my) = s(&app, 128.0, 128.0);
    app.canvas_move(mx, my, &[]);
    let (ux, uy) = s(&app, 232.0, 128.0);
    app.canvas_up(ux, uy, &[]);
    pump(&mut app);

    let ctx = egui::Context::default();
    let out = ctx.run_ui(egui::RawInput::default(), |ui| {
        crate::ui::layers::layer_property(ui, &mut app);
    });
    fn walk(sh: &egui::epaint::Shape, into: &mut String) {
        match sh {
            egui::epaint::Shape::Text(t) => {
                into.push_str(t.galley.text());
                into.push('\n');
            }
            egui::epaint::Shape::Vec(v) => v.iter().for_each(|sh| walk(sh, into)),
            _ => {}
        }
    }
    let mut text = String::new();
    for c in &out.shapes {
        walk(&c.shape, &mut text);
    }
    out.drop_without_applying_deltas();
    println!("[panel] Layer Property over a gradient layer painted:
{text}");
    assert!(
        text.contains("Gradient"),
        "Layer Property must name the live gradient it is standing on:
{text}"
    );
}

/// The manual promises "Auto gap & fringe" NARRATES what it measured, and
/// says so when it cannot measure. Both halves, through the real status.
#[test]
fn qa_auto_gap_and_fringe_narrates_what_it_measured() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    ink_square(&mut app, 9.0, 0.0);
    flats_under_lineart(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Fill));
    dispatch(&mut app, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));
    let mut opts = app.fill_opts;
    opts.auto = true;
    dispatch(&mut app, AppCmd::SetFillOpts(opts));

    click(&mut app, 128.0, 128.0);
    println!("[auto] inside the line art: {:?}", app.status);
    assert!(app.status.contains("auto"), "the auto fill says what it chose: {:?}", app.status);
    assert!(
        app.status.contains("lines ~"),
        "…and names the line width it measured: {:?}",
        app.status
    );

    // Blank paper: nothing to measure, and it must SAY so rather than
    // inventing a number.
    let Some(mut app2) = headless() else { return };
    page(&mut app2);
    dispatch(&mut app2, AppCmd::SetTool(Tool::Fill));
    dispatch(&mut app2, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));
    let mut o2 = app2.fill_opts;
    o2.auto = true;
    dispatch(&mut app2, AppCmd::SetFillOpts(o2));
    click(&mut app2, 128.0, 128.0);
    println!("[auto] on blank paper: {:?}", app2.status);
    assert!(
        app2.status.contains("no lines to measure"),
        "blank paper must say the measurement failed: {:?}",
        app2.status
    );
}
