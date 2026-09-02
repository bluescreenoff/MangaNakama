//! Full-surface pass, Edit ▸ Transform + Filter family (2026-09-02).
//!
//! Every flow is driven through the doors a mangaka's hands use — Ctrl+T
//! (`AppCmd::TransformStart`), the pointer on the float's handles, the
//! Tool Property fields (`TransformUpdate`), Enter/Esc, the Filter menu —
//! and the RESULT is read off the EXPORT render, the same pixels a print
//! gets. The fixture is deliberately LINE ART: 1 px hairlines in both
//! directions, a 5 px box and a disc, because "does the hairline survive"
//! is the whole question for a manga page.
//!
//! `MN_SURFACE_OUT=<dir>` keeps every PNG for a human to look at;
//! otherwise they land in the temp dir and are printed per shot.

use crate::app::new_document_tests::headless;
use crate::app::{App, PenSample, PointerKind};
use crate::cmd::{AppCmd, Tool, dispatch};
use mn_core::tile::TileIdx;

const W: u32 = 256;
const H: u32 = 256;

/// Horizontal hairlines: y = 40 + 8k, x in 32..=96 (8 of them).
const HLINES: [i32; 8] = [40, 48, 56, 64, 72, 80, 88, 96];
/// Vertical hairlines: x = 40 + 8k, y in 120..=200 (8 of them).
const VLINES: [i32; 8] = [40, 48, 56, 64, 72, 80, 88, 96];

fn page(app: &mut App) {
    app.doc = mn_core::Document::new(W, H);
    app.doc.layers[0].name = "art".into();
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
}

/// One black pixel at (x, y) on layer `li`, alpha `a` (0..=1).
fn ink_px(app: &mut App, li: usize, x: i32, y: i32, a: f32) {
    if x < 0 || y < 0 || x >= W as i32 || y >= H as i32 {
        return;
    }
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    let t = app.doc.layers[li].tile_mut(idx);
    let d = t.data_mut();
    let o = ((y - oy) as usize * 64 + (x - ox) as usize) * 4;
    d[o] = 0;
    d[o + 1] = 0;
    d[o + 2] = 0;
    d[o + 3] = mn_core::blend::f32_to_fix15(a);
}

/// The line-art fixture on the active layer. Nothing here goes through
/// the brush engine, so the pixels are exact: every hairline is one pixel
/// wide and fully opaque.
fn line_art(app: &mut App) {
    let li = app.doc.active;
    for &y in &HLINES {
        for x in 32..=96 {
            ink_px(app, li, x, y, 1.0);
        }
    }
    for &x in &VLINES {
        for y in 120..=200 {
            ink_px(app, li, x, y, 1.0);
        }
    }
    // 5 px box outline 140..220 × 120..200.
    for x in 140..=220 {
        for t in 0..5 {
            ink_px(app, li, x, 120 + t, 1.0);
            ink_px(app, li, x, 196 + t, 1.0);
        }
    }
    for y in 120..=200 {
        for t in 0..5 {
            ink_px(app, li, 140 + t, y, 1.0);
            ink_px(app, li, 216 + t, y, 1.0);
        }
    }
    // Disc r=20 at (180, 60).
    for y in 40..=80 {
        for x in 160..=200 {
            let (dx, dy) = (x - 180, y - 60);
            if dx * dx + dy * dy <= 400 {
                ink_px(app, li, x, y, 1.0);
            }
        }
    }
    app.doc.clear_history();
    app.renderer.invalidate();
}

fn pump(app: &mut App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

const NO_PEN: [PenSample; 0] = [];

fn s(app: &App, cx: f32, cy: f32) -> (f32, f32) {
    app.viewport.to_screen(cx, cy)
}

/// A pointer drag along a straight line through the real arms, in CANVAS
/// coordinates; the float stays open afterwards (no Enter here).
fn drag(app: &mut App, from: (f32, f32), to: (f32, f32)) {
    let (x0, y0) = s(app, from.0, from.1);
    app.canvas_down(x0, y0, PointerKind::Mouse, &NO_PEN);
    let steps = 24;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let (mx, my) = s(app, from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        app.canvas_move(mx, my, &NO_PEN);
    }
    let (ux, uy) = s(app, to.0, to.1);
    app.canvas_up(ux, uy, &NO_PEN);
    pump(app);
}

fn out_dir() -> std::path::PathBuf {
    let dir = match std::env::var_os("MN_SURFACE_OUT") {
        Some(d) => std::path::PathBuf::from(d),
        None => std::env::temp_dir().join(format!("mn-f3-{}", std::process::id())),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Render this page the way an export would and drop the PNG where the
/// agent can look at it.
fn shot(app: &mut App, name: &str) -> image::RgbaImage {
    let (w, h) = (app.doc.size.0, app.doc.size.1);
    let App { renderer, doc, .. } = app;
    let img = crate::app::pages::render_offscreen_drafts_off(renderer, doc, w, h);
    let p = out_dir().join(format!("{name}.png"));
    img.save(&p).expect("write the shot");
    println!("[shot] {}", p.display());
    img
}

/// Luma of the exported pixel over white paper (0 = ink, 255 = paper).
fn luma(img: &image::RgbaImage, x: i32, y: i32) -> u8 {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return 255;
    }
    let p = img.get_pixel(x as u32, y as u32).0;
    let a = p[3] as f32 / 255.0;
    let mix = |c: u8| c as f32 * a + 255.0 * (1.0 - a);
    ((mix(p[0]) * 0.2126) + (mix(p[1]) * 0.7152) + (mix(p[2]) * 0.0722)) as u8
}

fn inked(img: &image::RgbaImage, x: i32, y: i32) -> bool {
    luma(img, x, y) < 128
}

/// Any trace of ink at all — a hairline that came through grey still counts
/// as "there" for the artist, a missing one does not.
fn traced(img: &image::RgbaImage, x: i32, y: i32) -> bool {
    luma(img, x, y) < 230
}

fn ink_count(img: &image::RgbaImage) -> u32 {
    let (w, h) = img.dimensions();
    (0..h as i32)
        .flat_map(|y| (0..w as i32).map(move |x| (x, y)))
        .filter(|&(x, y)| inked(img, x, y))
        .count() as u32
}

/// Ink inside a rect, exclusive of the far edge.
fn ink_in(img: &image::RgbaImage, r: [i32; 4]) -> u32 {
    (r[1]..r[3])
        .flat_map(|y| (r[0]..r[2]).map(move |x| (x, y)))
        .filter(|&(x, y)| inked(img, x, y))
        .count() as u32
}

/// How many of the horizontal hairlines still read along column `x`:
/// scan a window around each expected row.
fn hlines_traced(img: &image::RgbaImage, x: i32, rows: &[i32], slack: i32) -> usize {
    rows.iter()
        .filter(|&&y| (-slack..=slack).any(|d| traced(img, x, y + d)))
        .count()
}

fn xf(app: &App) -> mn_core::Affine2 {
    app.transform_drag.as_ref().expect("a float is open").xform
}

fn start(app: &mut App) {
    dispatch(app, AppCmd::TransformStart);
    assert!(app.transform_drag.is_some(), "Ctrl+T opens the float: {}", app.status);
}

fn set(app: &mut App, sx: f32, sy: f32, rad: f32, tx: f32, ty: f32) {
    dispatch(app, AppCmd::TransformUpdate { sx, sy, rad, tx, ty });
}

fn commit(app: &mut App) {
    dispatch(app, AppCmd::TransformCommit);
    assert!(app.transform_drag.is_none(), "Enter closes the float");
}

// ------------------------------------------------------------------ T01 --

/// Ctrl+T with no selection lifts the whole layer; Esc puts every pixel
/// back and leaves NO undo step behind.
#[test]
fn t01_lift_whole_layer_and_esc_is_free() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    let before = shot(&mut app, "t01-before");
    let n0 = ink_count(&before);
    assert!(n0 > 1500, "the fixture inked something: {n0}");
    start(&mut app);
    let d = app.transform_drag.as_ref().unwrap();
    let r = d.source.rect;
    // CSP's bounding box sits ON the drawing. Ours used to sit on the tile
    // grid (0..256 here — every handle off the ink, the pivot off centre).
    assert_eq!(r, [32, 40, 221, 201], "the float hugs the ink, not the tile grid");
    assert!(d.is_identity());
    // While the float is open the page is visibly the same (the lifted
    // pixels are shown through the overlay, and the export ignores the
    // overlay — what a print gets mid-transform is a page with a hole).
    dispatch(&mut app, AppCmd::TransformCancel);
    assert!(app.transform_drag.is_none());
    let after = shot(&mut app, "t01-after-esc");
    assert_eq!(ink_count(&after), n0, "Esc put every pixel back");
    assert_eq!(app.doc.undo_len(), 0, "a cancelled transform is not an undo step");
    assert!(app.status.contains("canceled"), "{}", app.status);
}

// ------------------------------------------------------------------ T02 --

/// Numeric move (Tool Property X/Y) commits exactly; one undo step; undo
/// restores the page byte-for-byte on the render.
#[test]
fn t02_numeric_move_commits_exactly_and_undoes_in_one_step() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    let before = shot(&mut app, "t02-before");
    start(&mut app);
    set(&mut app, 1.0, 1.0, 0.0, 50.0, 30.0);
    commit(&mut app);
    let after = shot(&mut app, "t02-moved-50-30");
    for &y in &HLINES {
        assert!(inked(&after, 64 + 50, y + 30), "hairline y={y} moved by (50,30)");
        assert!(!inked(&after, 34, y), "and left nothing behind at y={y}");
    }
    assert_eq!(app.doc.undo_len(), 1, "one commit = one undo step");
    dispatch(&mut app, AppCmd::Undo);
    let undone = shot(&mut app, "t02-undone");
    assert_eq!(
        undone.as_raw(),
        before.as_raw(),
        "undo restores the page exactly"
    );
}

// ------------------------------------------------------------------ T03 --

/// Shift while dragging inside the box constrains the move to 45° steps
/// (CSP: "hold Shift while dragging to move in 45-degree increments").
#[test]
fn t03_shift_drag_constrains_the_move_to_45_degree_steps() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    start(&mut app);
    app.shell.test_modifiers = Some(egui::Modifiers::SHIFT);
    // Press inside the float (on the box outline) and drag mostly right,
    // a little down.
    let (x0, y0) = s(&app, 180.0, 160.0);
    app.canvas_down(x0, y0, PointerKind::Mouse, &NO_PEN);
    let (x1, y1) = s(&app, 240.0, 172.0);
    app.canvas_move(x1, y1, &NO_PEN);
    let t = xf(&app).t;
    assert!(t[0] > 55.0, "moved right: {t:?}");
    assert!(t[1].abs() < 0.5, "Shift pinned the drag to the horizontal: {t:?}");
    app.canvas_up(x1, y1, &NO_PEN);
    pump(&mut app);
    app.shell.test_modifiers = None;
    dispatch(&mut app, AppCmd::TransformCancel);
}

// ------------------------------------------------------------------ T04 --

/// Corner handle scales both axes; Shift on the corner keeps the aspect;
/// an edge handle scales ONE axis. Read off the params, then the print.
#[test]
fn t04_corner_and_edge_handles_scale_as_csp_describes() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    start(&mut app);
    // CSP ships Keep aspect ratio ON; the free deformation is the
    // checkbox's off state.
    assert!(app.transform_keep_aspect, "Keep aspect ratio defaults on (CSP)");
    app.transform_keep_aspect = false;
    let r = app.transform_drag.as_ref().unwrap().source.rect;
    let (x0, y0, x1, y1) = (r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32);
    // Bottom-right corner, dragged out by (+40, +10): sx and sy differ.
    let (px, py) = s(&app, x1, y1);
    app.canvas_down(px, py, PointerKind::Mouse, &NO_PEN);
    let (qx, qy) = s(&app, x1 + 40.0, y1 + 10.0);
    app.canvas_move(qx, qy, &NO_PEN);
    let d = app.transform_drag.as_ref().unwrap();
    let (sx, sy) = (d.sx, d.sy);
    assert!(sx > 1.15 && sy > 1.02 && sx > sy + 0.05, "free corner: sx={sx} sy={sy}");
    // The anchor (opposite corner) did not move.
    let c = d.bbox[0];
    assert!((c[0] - x0).abs() < 0.5 && (c[1] - y0).abs() < 0.5, "anchor pinned: {c:?}");
    app.canvas_up(qx, qy, &NO_PEN);
    pump(&mut app);
    dispatch(&mut app, AppCmd::TransformReset);
    // Same corner with Shift: one ratio for both axes.
    app.shell.test_modifiers = Some(egui::Modifiers::SHIFT);
    app.canvas_down(px, py, PointerKind::Mouse, &NO_PEN);
    app.canvas_move(qx, qy, &NO_PEN);
    let d = app.transform_drag.as_ref().unwrap();
    assert!((d.sx - d.sy).abs() < 1e-3, "Shift keeps aspect: sx={} sy={}", d.sx, d.sy);
    app.canvas_up(qx, qy, &NO_PEN);
    pump(&mut app);
    app.shell.test_modifiers = None;
    dispatch(&mut app, AppCmd::TransformReset);
    // Right edge midpoint, dragged +40: sx only.
    let (ex, ey) = s(&app, x1, (y0 + y1) * 0.5);
    app.canvas_down(ex, ey, PointerKind::Mouse, &NO_PEN);
    let (fx, fy) = s(&app, x1 + 40.0, (y0 + y1) * 0.5);
    app.canvas_move(fx, fy, &NO_PEN);
    let d = app.transform_drag.as_ref().unwrap();
    assert!(d.sx > 1.15, "edge scaled x: {}", d.sx);
    assert!((d.sy - 1.0).abs() < 1e-4, "edge left y alone: {}", d.sy);
    app.canvas_up(fx, fy, &NO_PEN);
    pump(&mut app);
    let m = xf(&app);
    commit(&mut app);
    let img = shot(&mut app, "t04-edge-scaled-x");
    // The vertical hairlines spread out: x=40 stays near the anchor, x=96
    // moved right by ~ (96-x0)*(sx-1).
    let e = m.apply([40.5, 160.5]);
    assert!(inked(&img, e[0].floor() as i32, 160), "first line where the affine put it: {e:?}");
    assert!(!inked(&img, 96, 160), "the last line left x=96");
}

// ------------------------------------------------------------------ T05 --

/// Rotate 90° exactly: a 1 px hairline must come out a 1 px hairline, in
/// the place the affine says, with the ink count preserved.
#[test]
fn t05_rotate_90_keeps_hairlines_crisp() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    let before = shot(&mut app, "t05-before");
    let n0 = ink_count(&before);
    start(&mut app);
    set(&mut app, 1.0, 1.0, std::f32::consts::FRAC_PI_2, 0.0, 0.0);
    let m = xf(&app);
    commit(&mut app);
    let img = shot(&mut app, "t05-rot90");
    let n1 = ink_count(&img);
    // A quarter turn about a half-integer pivot lands on pixel centres
    // exactly, so bilinear reproduces the art 1:1 — no grey, no loss.
    assert!(
        (n1 as i64 - n0 as i64).abs() <= (n0 / 50) as i64,
        "ink count preserved through a quarter turn: {n0} -> {n1}"
    );
    // Each vertical hairline became a horizontal one at the mapped place.
    for &x in &VLINES {
        let p = m.apply([x as f32 + 0.5, 160.5]);
        let (px, py) = (p[0].floor() as i32, p[1].floor() as i32);
        assert!(
            inked(&img, px, py),
            "hairline x={x} should be at ({px},{py}) after the turn"
        );
        // …and one pixel wide: the rows above and below are paper.
        assert!(
            !inked(&img, px, py - 1) || !inked(&img, px, py + 1),
            "the turned hairline at ({px},{py}) is still one pixel wide"
        );
    }
}

// ------------------------------------------------------------------ T06 --

/// Rotate 15°: the eye test for aliasing. Nothing lands outside the
/// transformed box (no ghost edges), and the hairlines are still traceable.
#[test]
fn t06_rotate_15_has_no_ghosts_and_keeps_every_hairline() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    start(&mut app);
    set(&mut app, 1.0, 1.0, 15f32.to_radians(), 0.0, 0.0);
    let d = app.transform_drag.as_ref().unwrap();
    let bbox = d.bbox;
    let m = d.xform;
    commit(&mut app);
    let img = shot(&mut app, "t06-rot15");
    // Outside the rotated quad (+2 px slack): paper only.
    let mut ghosts = 0;
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            if inked(&img, x, y) {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                let inside = super::point_in_quad(p, bbox)
                    || (-2..=2).any(|dx| {
                        (-2..=2).any(|dy| {
                            super::point_in_quad([p[0] + dx as f32, p[1] + dy as f32], bbox)
                        })
                    });
                if !inside {
                    ghosts += 1;
                }
            }
        }
    }
    assert_eq!(ghosts, 0, "ink outside the rotated box = ghost edges");
    // Every hairline traceable at its rotated midpoint.
    for &y in &HLINES {
        let p = m.apply([64.5, y as f32 + 0.5]);
        let (px, py) = (p[0].floor() as i32, p[1].floor() as i32);
        let hit = (-1..=1).any(|dx| (-1..=1).any(|dy| traced(&img, px + dx, py + dy)));
        assert!(hit, "hairline y={y} vanished at ({px},{py}) after 15°");
    }
}

// ------------------------------------------------------------------ T07 --

/// Scale up ×2 then down ×0.5 — the round trip a mangaka does when a
/// panel is re-laid. Every hairline must survive, and the page must not
/// have grown a grey halo.
#[test]
fn t07_scale_up_then_down_round_trip_keeps_hairlines() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    // Hairlines only, centred, so ×2 stays on the page.
    let rows: Vec<i32> = (0..8).map(|k| 100 + 8 * k).collect();
    for &y in &rows {
        for x in 96..=160 {
            ink_px(&mut app, 0, x, y, 1.0);
        }
    }
    app.doc.clear_history();
    app.renderer.invalidate();
    let before = shot(&mut app, "t07-before");
    let n0 = ink_count(&before);
    start(&mut app);
    set(&mut app, 2.0, 2.0, 0.0, 0.0, 0.0);
    let m1 = xf(&app);
    commit(&mut app);
    let up = shot(&mut app, "t07-up-2x");
    // Bilinear at exactly 2x about a pixel-centre pivot: each hairline
    // becomes one full row flanked by two half rows — the same ink, twice
    // as tall, not twice as dark.
    assert!(ink_count(&up) >= n0 * 2 - 16, "2x has twice the rows: {} vs {n0}", ink_count(&up));
    start(&mut app);
    set(&mut app, 0.5, 0.5, 0.0, 0.0, 0.0);
    let m2 = xf(&app);
    commit(&mut app);
    let down = shot(&mut app, "t07-down-back");
    let col = m2.apply(m1.apply([128.5, 0.0]))[0].floor() as i32;
    let back: Vec<i32> = rows
        .iter()
        .map(|&y| m2.apply(m1.apply([0.0, y as f32 + 0.5]))[1].floor() as i32)
        .collect();
    assert_eq!(
        hlines_traced(&down, col, &back, 1),
        rows.len(),
        "every horizontal hairline is traceable after the round trip"
    );
    let n1 = ink_count(&down);
    assert!(
        n1 as f32 > n0 as f32 * 0.7 && (n1 as f32) < n0 as f32 * 1.6,
        "no halo, no loss: {n0} -> {n1}"
    );
}

// ------------------------------------------------------------------ T08 --

/// Shrink to 35 %: the manual's claim — Smooth edges (bilinear) loses 1 px
/// lines, High accuracy keeps them. Pin both so the claim stays true.
#[test]
fn t08_shrink_35_percent_bilinear_vs_high_accuracy() {
    let Some(mut app) = headless() else { return };
    let mut kept = [0usize; 2];
    for (i, interp) in [
        mn_core::transform::Interp::Bilinear,
        mn_core::transform::Interp::HighAccuracy,
    ]
    .into_iter()
    .enumerate()
    {
        page(&mut app);
        line_art(&mut app);
        dispatch(&mut app, AppCmd::SetTransformInterp(interp));
        start(&mut app);
        set(&mut app, 0.35, 0.35, 0.0, 0.0, 0.0);
        let m = xf(&app);
        commit(&mut app);
        let img = shot(&mut app, &format!("t08-shrink35-{}", interp.label().replace(' ', "-")));
        // Count hairlines at their mapped rows along the mapped column.
        let col = m.apply([64.5, 0.0])[0].floor() as i32;
        let rows: Vec<i32> = HLINES
            .iter()
            .map(|&y| m.apply([0.0, y as f32 + 0.5])[1].floor() as i32)
            .collect();
        kept[i] = hlines_traced(&img, col, &rows, 0);
    }
    assert_eq!(kept[1], HLINES.len(), "High accuracy keeps every hairline: {kept:?}");
    assert!(kept[0] <= kept[1], "bilinear never beats the area average: {kept:?}");
}

// ------------------------------------------------------------------ T09 --

/// Standalone Edit ▸ Flip Horizontal with a selection: mirrors INSIDE the
/// selection about its centre and leaves every pixel outside untouched.
#[test]
fn t09_flip_horizontal_inside_a_selection_leaves_the_rest_alone() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    let before = shot(&mut app, "t09-before");
    // Left half only: the vertical hairlines live there.
    app.doc.selection = Some(mn_core::selection::Selection::from_rect(
        &app.doc, 0.0, 110.0, 128.0, 210.0,
    ));
    dispatch(&mut app, AppCmd::TransformFlip { horizontal: true });
    assert!(app.status.contains("flipped"), "{}", app.status);
    let img = shot(&mut app, "t09-flipped-left-half");
    // x=40 → 128-1-40 = 87 (pixel-exact mirror about the selection).
    for &x in &VLINES {
        let mx = 127 - x;
        assert!(
            inked(&img, mx, 160) || inked(&img, mx + 1, 160),
            "hairline x={x} mirrored to x≈{mx}"
        );
    }
    // Right half + top half: byte-identical on the render.
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            if x >= 128 || y < 110 || y >= 210 {
                assert_eq!(
                    img.get_pixel(x as u32, y as u32),
                    before.get_pixel(x as u32, y as u32),
                    "pixel ({x},{y}) outside the selection changed"
                );
            }
        }
    }
    assert_eq!(app.doc.undo_len(), 1, "a flip is one undo step");
    app.doc.selection = None;
}

// ------------------------------------------------------------------ T10 --

/// Flip during an open transform (the Tool Property button) composes with
/// the rest of the transform and commits as ONE step.
#[test]
fn t10_flip_button_inside_a_transform_is_part_of_the_one_step() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    start(&mut app);
    set(&mut app, 1.0, 1.0, 0.0, 20.0, 0.0);
    dispatch(&mut app, AppCmd::TransformFlip { horizontal: false });
    let d = app.transform_drag.as_ref().unwrap();
    assert!(d.sy < 0.0 && d.tx == 20.0, "flip V kept the move: sy={} tx={}", d.sy, d.tx);
    let m = d.xform;
    commit(&mut app);
    assert_eq!(app.doc.undo_len(), 1);
    let img = shot(&mut app, "t10-flipV-plus-move");
    // The disc's centre went where the affine says, and it is solid there.
    let c = m.apply([180.5, 60.5]);
    let (cx, cy) = (c[0].floor() as i32, c[1].floor() as i32);
    assert!(cy > 120, "the disc swung to the lower half: ({cx},{cy})");
    assert_eq!(ink_in(&img, [cx - 10, cy - 10, cx + 10, cy + 10]), 400, "solid disc there");
    assert_eq!(ink_in(&img, [170, 50, 191, 71]), 0, "and nothing where it was");
}

// ------------------------------------------------------------------ T11 --

/// The reference point: moved to the art's top-left corner, a quarter
/// turn swings the art about THAT corner (the box lands where the affine
/// about the new pivot says), and Reset keeps the pivot where it was put.
#[test]
fn t11_pivot_moves_the_centre_of_rotation_and_reset_keeps_it() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    start(&mut app);
    let r = app.transform_drag.as_ref().unwrap().source.rect;
    let corner = [r[0] as f32, r[1] as f32];
    dispatch(&mut app, AppCmd::TransformSetPivot { pivot: Some(corner) });
    set(&mut app, 1.0, 1.0, std::f32::consts::FRAC_PI_2, 0.0, 0.0);
    let d = app.transform_drag.as_ref().unwrap();
    let c0 = d.bbox[0];
    assert!(
        (c0[0] - corner[0]).abs() < 0.01 && (c0[1] - corner[1]).abs() < 0.01,
        "the pivot corner stays put under rotation: {c0:?} vs {corner:?}"
    );
    // Top-right corner swung DOWN to below the pivot (clockwise, y-down).
    let c1 = d.bbox[1];
    assert!(c1[1] > corner[1] + 100.0 && (c1[0] - corner[0]).abs() < 0.01, "{c1:?}");
    dispatch(&mut app, AppCmd::TransformReset);
    let d = app.transform_drag.as_ref().unwrap();
    assert_eq!(d.pivot_override, Some(corner), "Reset leaves the reference point");
    assert!(d.is_identity(), "Reset is the identity transform");
    dispatch(&mut app, AppCmd::TransformCancel);
}

// ------------------------------------------------------------------ T12 --

/// Enter on an untouched float is a cancel, not an empty undo step.
#[test]
fn t12_enter_on_an_untouched_float_is_a_free_cancel() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    let before = shot(&mut app, "t12-before");
    start(&mut app);
    commit(&mut app);
    assert_eq!(app.doc.undo_len(), 0, "identity commit pushed nothing");
    let after = shot(&mut app, "t12-after");
    assert_eq!(after.as_raw(), before.as_raw());
}

// ------------------------------------------------------------------ T13 --

/// Inside a frame folder, a transform that pushes art past the panel edge
/// stays clipped by the folder — the panel seal survives the move.
#[test]
fn t13_transform_inside_a_frame_folder_stays_clipped() {
    let Some(mut app) = headless() else { return };
    crate::app::new_document_tests::small_draft(&mut app, 1, "");
    dispatch(&mut app, AppCmd::NewComicCreate);
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
    let head = app
        .doc
        .layers
        .iter()
        .position(|l| l.folder && l.is_frame())
        .expect("a comic page seeds a frame folder");
    assert_eq!(app.doc.enclosing_frame_folder(app.doc.active), Some(head));
    let b = app.doc.layers[head].frames().unwrap().frames[0].bbox();
    let li = app.doc.active;
    let (pw, ph) = app.doc.size;
    // A 5 px bar across the middle of the panel.
    let y = ((b[1] + b[3]) * 0.5) as i32;
    for x in (b[0] as i32 + 10)..(b[2] as i32 - 10) {
        for t in 0..5 {
            let (xx, yy) = (x, y + t);
            if xx >= 0 && yy >= 0 && xx < pw as i32 && yy < ph as i32 {
                let idx = TileIdx::of_pixel(xx, yy);
                let (ox, oy) = idx.origin();
                let tile = app.doc.layers[li].tile_mut(idx);
                let d = tile.data_mut();
                let o = ((yy - oy) as usize * 64 + (xx - ox) as usize) * 4;
                d[o] = 0;
                d[o + 1] = 0;
                d[o + 2] = 0;
                d[o + 3] = mn_core::blend::f32_to_fix15(1.0);
            }
        }
    }
    app.doc.clear_history();
    app.renderer.invalidate();
    start(&mut app);
    // Shove it half a panel to the right.
    set(&mut app, 1.0, 1.0, 0.0, (b[2] - b[0]) * 0.5, 0.0);
    commit(&mut app);
    let img = shot(&mut app, "t13-folder-clip");
    let (w, _) = img.dimensions();
    let outside = ink_in(&img, [b[2] as i32 + 3, y - 2, w as i32, y + 8]);
    assert_eq!(outside, 0, "nothing printed right of the panel after the move");
    assert!(
        ink_in(&img, [b[0] as i32 + 10, y - 2, b[2] as i32 - 3, y + 8]) > 0,
        "the bar is still inside the panel"
    );
}

// ------------------------------------------------------------------ T14 --

/// The Object tool grabs the ink and a pure drag commits on release —
/// CSP's Object tool moves layers directly, no Enter.
#[test]
fn t14_object_tool_grab_moves_the_ink_and_commits_on_release() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Object));
    // Press on the box outline (5 px, easy to hit) and drag it 30 px right.
    drag(&mut app, (142.0, 160.0), (172.0, 160.0));
    assert!(app.transform_drag.is_none(), "a pure move committed on release: {}", app.status);
    let img = shot(&mut app, "t14-object-moved");
    assert!(inked(&img, 142 + 30, 160), "the box's left wall moved 30 px right");
    assert!(!inked(&img, 142, 160), "and left nothing behind");
    assert_eq!(app.doc.undo_len(), 1);
}

// ------------------------------------------------------------------ T15 --

/// Mesh transform: bend one lattice point, commit, look. The eye test for
/// the resample; the assertion is only that the bend landed and undid.
#[test]
fn t15_mesh_transform_bends_and_renders() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    let before = shot(&mut app, "t15-before");
    dispatch(&mut app, AppCmd::TransformMeshStart);
    let d = app.transform_drag.as_mut().expect("mesh float");
    let m = d.mesh.as_mut().unwrap();
    // The centre lattice point, pulled 16 px down-right (a third of a cell —
    // a real bend; past half a cell the cell folds over itself and the
    // fold renders dotted, noted in the ledger).
    let ci = (m.n / 2) * m.n + m.n / 2;
    let p = m.pts[ci];
    let (sx, sy) = s(&app, p[0], p[1]);
    app.canvas_down(sx, sy, PointerKind::Mouse, &NO_PEN);
    let (tx, ty) = s(&app, p[0] + 16.0, p[1] + 16.0);
    app.canvas_move(tx, ty, &NO_PEN);
    app.canvas_up(tx, ty, &NO_PEN);
    pump(&mut app);
    commit(&mut app);
    let img = shot(&mut app, "t15-mesh-bent");
    assert_ne!(img.as_raw(), before.as_raw(), "the bend changed the page");
    // No seams: inside the disc's neighbourhood a white pixel with ink on
    // both sides is a hole the warp punched along a lattice edge (the old
    // Newton left a one-pixel white line from the pulled point through
    // the disc).
    let mut seams = Vec::new();
    for y in 30..112 {
        for x in 150..235 {
            if luma(&img, x, y) >= 250
                && ((inked(&img, x - 1, y) && inked(&img, x + 1, y))
                    || (inked(&img, x, y - 1) && inked(&img, x, y + 1)))
            {
                seams.push((x, y));
            }
        }
    }
    assert!(seams.is_empty(), "seam pixels in the disc: {:?}", &seams[..seams.len().min(6)]);
    assert_eq!(app.doc.undo_len(), 1);
    dispatch(&mut app, AppCmd::Undo);
    let undone = shot(&mut app, "t15-mesh-undone");
    assert_eq!(undone.as_raw(), before.as_raw());
}

// ------------------------------------------------------------------ F01 --

/// Gaussian blur on hairlines: the line goes soft and wider, the ink is
/// conserved (no darkening, no vanishing), one undo step, and the layer
/// outside a selection is untouched.
#[test]
fn f01_gaussian_blur_softens_hairlines_and_respects_the_selection() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    let before = shot(&mut app, "f01-before");
    app.doc.selection = Some(mn_core::selection::Selection::from_rect(
        &app.doc, 0.0, 0.0, 128.0, 110.0,
    ));
    dispatch(&mut app, AppCmd::FilterApply(mn_core::Filter::Gaussian { sigma: 2.0 }));
    assert!(app.status.contains("applied"), "{}", app.status);
    let img = shot(&mut app, "f01-gaussian-top-left");
    // Hairline centre lighter than solid, neighbours darker than paper.
    let c = luma(&img, 64, 56);
    assert!(c > 60 && c < 250, "centre softened: {c}");
    assert!(luma(&img, 64, 58) < 250, "and spread to the neighbour rows");
    // Outside the selection: identical.
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            if x >= 132 || y >= 114 {
                assert_eq!(
                    img.get_pixel(x as u32, y as u32),
                    before.get_pixel(x as u32, y as u32),
                    "({x},{y}) outside the selection changed"
                );
            }
        }
    }
    assert_eq!(app.doc.undo_len(), 1);
    app.doc.selection = None;
}

// ------------------------------------------------------------------ F02 --

/// Adjust line width: +1 turns a 1 px hairline into 3 px; −1 on a 1 px
/// hairline. CSP's Narrow has "At least 1 pixel" to keep the centre line;
/// pin what ours does.
#[test]
fn f02_line_width_thicken_and_narrow_on_hairlines() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    dispatch(&mut app, AppCmd::FilterApply(mn_core::Filter::LineWidth { delta: 1 }));
    let img = shot(&mut app, "f02-thicken-1");
    assert!(inked(&img, 64, 55) && inked(&img, 64, 56) && inked(&img, 64, 57), "3 px now");
    assert!(!inked(&img, 64, 54) && !inked(&img, 64, 58), "and not 5");
    dispatch(&mut app, AppCmd::Undo);
    dispatch(&mut app, AppCmd::FilterApply(mn_core::Filter::LineWidth { delta: -1 }));
    let img = shot(&mut app, "f02-narrow-1");
    // The 5 px box wall is 3 px now.
    assert!(inked(&img, 142, 160) && !inked(&img, 140, 160) && !inked(&img, 144, 160));
    // The 1 px hairlines: does anything remain?
    let left = hlines_traced(&img, 64, &HLINES, 0);
    println!("[f02] hairlines left after narrow −1: {left} of {}", HLINES.len());
    assert_eq!(left, HLINES.len(), "Narrow keeps a 1 px centre line (CSP: at least 1 pixel)");
}

// ------------------------------------------------------------------ F03 --

/// Remove dust: specks up to the size go, the hairlines (long blobs) stay.
#[test]
fn f03_remove_dust_takes_specks_and_keeps_hairlines() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    line_art(&mut app);
    // Six 2×2 specks in the empty band between the hairlines and the box.
    for k in 0..6 {
        let (x, y) = (110 + k * 6, 100 + (k % 2) * 4);
        for dx in 0..2 {
            for dy in 0..2 {
                ink_px(&mut app, 0, x + dx, y + dy, 0.8);
            }
        }
    }
    app.doc.clear_history();
    app.renderer.invalidate();
    let before = shot(&mut app, "f03-dusty");
    let n0 = ink_count(&before);
    dispatch(&mut app, AppCmd::FilterApply(mn_core::Filter::RemoveDust { max_px: 5 }));
    let img = shot(&mut app, "f03-dust-removed");
    assert_eq!(ink_in(&img, [108, 98, 150, 108]), 0, "the specks are gone");
    assert_eq!(hlines_traced(&img, 64, &HLINES, 0), HLINES.len(), "hairlines kept");
    assert_eq!(ink_count(&img), n0 - 24, "exactly the 24 speck pixels went");
    assert_eq!(app.doc.undo_len(), 1);
}

// ------------------------------------------------------------------ F04 --

/// Filters on the wrong layer say so instead of silently doing nothing.
#[test]
fn f04_a_filter_on_a_vector_layer_says_why_it_did_nothing() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::AddVectorLayer);
    let steps = app.doc.undo_len();
    dispatch(&mut app, AppCmd::FilterApply(mn_core::Filter::Blur));
    assert!(
        app.status.contains("did nothing") || app.status.contains("raster"),
        "{}",
        app.status
    );
    assert_eq!(app.doc.undo_len(), steps, "a refused filter pushes no step");
}

// ------------------------------------------------------------------ F05 --

/// Binarize (mono conversion) on a grey wash: every pixel ends black or
/// white at the threshold.
#[test]
fn f05_binarize_makes_a_grey_wash_pure_black_and_white() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    // A grey ramp: alpha 0.1..0.9 in 9 columns.
    for k in 0..9 {
        for y in 20..100 {
            for x in 0..20 {
                ink_px(&mut app, 0, 30 + k * 22 + x, y, 0.1 * (k + 1) as f32);
            }
        }
    }
    app.doc.clear_history();
    app.renderer.invalidate();
    let _ = shot(&mut app, "f05-ramp");
    // Tonal correction ▸ Binarization is COLOUR-only (CSP too): black ink
    // at 10 % alpha stays 10 % alpha, so the ramp is untouched on the
    // print. The manga door is the layer's expression colour = Monochrome.
    dispatch(&mut app, AppCmd::AdjustOpen(mn_core::Adjust::BINARIZE));
    dispatch(&mut app, AppCmd::AdjustApply);
    let img = shot(&mut app, "f05-binarized-colour-only");
    assert!(luma(&img, 40, 60) > 200 && luma(&img, 40, 60) < 250, "10 % alpha is still a light grey");
    dispatch(&mut app, AppCmd::Undo);
    dispatch(
        &mut app,
        AppCmd::SetLayerExpression(0, mn_core::LayerExpression::Mono),
    );
    let img = shot(&mut app, "f05-mono-expression");
    for k in 0..9 {
        let l = luma(&img, 40 + k * 22, 60);
        assert!(l < 16 || l > 239, "column {k} is pure: luma {l}");
    }
    // Nothing between: the light half went white, the dark half black.
    assert!(luma(&img, 40, 60) > 239, "10 % is white");
    assert!(luma(&img, 40 + 8 * 22, 60) < 16, "90 % is black");
}

// ------------------------------------------------------------------ F06 --

/// Smoothing on a stair-stepped diagonal: intermediate greys appear on the
/// steps, the line is not visibly blurred (the centre stays dark).
#[test]
fn f06_smoothing_antialiases_a_jaggy_diagonal() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    // A 2-px-wide diagonal with hard steps.
    for i in 0..120 {
        let (x, y) = (40 + i, 40 + i / 2);
        ink_px(&mut app, 0, x, y, 1.0);
        ink_px(&mut app, 0, x, y + 1, 1.0);
    }
    app.doc.clear_history();
    app.renderer.invalidate();
    let before = shot(&mut app, "f06-jaggy");
    dispatch(&mut app, AppCmd::FilterApply(mn_core::Filter::Smoothing));
    let img = shot(&mut app, "f06-smoothed");
    assert_ne!(img.as_raw(), before.as_raw());
    // A grey appeared on the step edge.
    let greys = (40..160)
        .flat_map(|x| (38..104).map(move |y| (x, y)))
        .filter(|&(x, y)| {
            let l = luma(&img, x, y);
            l > 40 && l < 215
        })
        .count();
    assert!(greys > 60, "intermediate values on the steps: {greys}");
    assert!(luma(&img, 100, 70) < 100, "the line's body stays dark");
}

// ------------------------------------------------------------------ T16 --

/// Edit ▸ Flip Horizontal on a whole layer mirrors the art IN PLACE — about
/// the ink's own centre. With the box on the tile grid the pivot sat at the
/// tile's centre and the flip MOVED the art (here by 42 px).
#[test]
fn t16_whole_layer_flip_keeps_the_art_where_it_was() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    // An asymmetric mark inside one tile: an L of 1 px lines at 70..80.
    for i in 70..=80 {
        ink_px(&mut app, 0, i, 80, 1.0);
        ink_px(&mut app, 0, 70, i, 1.0);
    }
    app.doc.clear_history();
    app.renderer.invalidate();
    let before = shot(&mut app, "t16-before");
    let n0 = ink_count(&before);
    dispatch(&mut app, AppCmd::TransformFlip { horizontal: true });
    let img = shot(&mut app, "t16-flipped-in-place");
    assert_eq!(ink_count(&img), n0, "a flip is a permutation");
    // The vertical stroke at x=70 is now at x=80; the bar still spans 70..80.
    assert!(inked(&img, 80, 75), "the upright mirrored to x=80");
    assert!(!inked(&img, 70, 75), "and left x=70");
    assert!(inked(&img, 70, 80) && inked(&img, 80, 80), "the bar stayed 70..80");
    assert_eq!(ink_in(&img, [90, 60, 140, 100]), 0, "nothing drifted right (tile-grid pivot)");
}

// ------------------------------------------------------------------ T17 --

/// A vector-ink layer transforms like a raster one from the outside —
/// Ctrl+T, move, Enter — but its RECORDS move: the strokes stay editable,
/// one undo step, and a standalone Flip mirrors the records too.
#[test]
fn t17_vector_layer_transform_moves_the_strokes_and_undoes() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::AddVectorLayer);
    let li = app.doc.active;
    assert!(app.doc.layers[li].records_strokes());
    app.props_current.stabilizer = 0.0;
    app.props_current.size_px = 6.0;
    app.prefs.mouse_smooth_px = 0.0;
    app.apply_props();
    app.begin_stroke(PointerKind::Mouse);
    let batch: Vec<PenSample> = (0..30)
        .map(|i| PenSample {
            x: 40.0 + i as f32 * 4.0,
            y: 100.0,
            pressure: 0.9,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();
    let steps = app.doc.undo_len();
    let rec0 = app.doc.layers[li].strokes.clone().unwrap();
    assert_eq!(rec0.strokes.len(), 1, "one recorded stroke");
    let b0 = app.doc.layers[li].ink_bounds().expect("ink");
    let before = shot(&mut app, "t17-vector-before");

    start(&mut app);
    set(&mut app, 1.0, 1.0, 0.0, 30.0, 40.0);
    commit(&mut app);
    assert!(app.status.contains("strokes"), "{}", app.status);
    let rec1 = app.doc.layers[li].strokes.clone().unwrap();
    assert_eq!(rec1.strokes.len(), 1, "still one editable stroke, not a raster stamp");
    let p0 = rec0.strokes[0].points[5];
    let p1 = rec1.strokes[0].points[5];
    assert!((p1.0 - p0.0 - 30.0).abs() < 1e-3 && (p1.1 - p0.1 - 40.0).abs() < 1e-3, "the record moved by (30,40): {p0:?} -> {p1:?}");
    let b1 = app.doc.layers[li].ink_bounds().expect("ink after");
    assert_eq!([b1[0] - b0[0], b1[1] - b0[1]], [30, 40], "the re-derived ink moved with it: {b0:?} -> {b1:?}");
    let _ = shot(&mut app, "t17-vector-moved");
    assert_eq!(app.doc.undo_len(), steps + 1, "one step");
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers[li].strokes.as_ref().unwrap(), &rec0, "undo restores the record");
    let undone = shot(&mut app, "t17-vector-undone");
    assert_eq!(undone.as_raw(), before.as_raw(), "and the pixels");

    // Standalone Flip V about the ink's centre: the record mirrors.
    dispatch(&mut app, AppCmd::TransformFlip { horizontal: false });
    assert!(app.status.contains("strokes"), "{}", app.status);
    let rec2 = app.doc.layers[li].strokes.clone().unwrap();
    let q = rec2.strokes[0].points[5];
    assert!((q.0 - p0.0).abs() < 1e-3, "x untouched by a vertical flip");
    let _ = shot(&mut app, "t17-vector-flipped");
}
