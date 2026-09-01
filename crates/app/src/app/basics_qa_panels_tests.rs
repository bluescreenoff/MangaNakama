//! Mangaka-basics QA, selection + paneling family (2026-09-02 round).
//!
//! Every flow here is driven through the doors a mangaka's hands actually
//! use — `AppCmd::SetTool`, pointer down/move/up on the canvas, the command
//! each Selection-Launcher button pushes, undo — and the RESULT is read off
//! the EXPORT render (`render_offscreen_drafts_off`), not off an internal
//! structure. A selection that is right in memory and wrong on the printed
//! page is still wrong.
//!
//! The two questions this family exists to answer:
//!   * does a selection land exactly where the drag drew it, and does the
//!     ink that follows it stay inside?
//!   * does a frame folder actually mask the art, with even gutters and a
//!     border of the width the Tool Property claims?

use super::new_document_tests::headless;
use crate::app::{App, PenSample, PointerKind};
use crate::cmd::{AppCmd, SelectMode, Tool, dispatch};

/// Small enough that a canvas-wide flood is instant, big enough that a
/// gutter of a few millimetres at 72 dpi is several pixels wide.
const W: u32 = 256;
const H: u32 = 256;

fn page(app: &mut App) {
    app.doc = mn_core::Document::new(W, H);
    app.doc.layers[0].name = "art".into();
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
}

/// Canvas point → the SCREEN point the pointer arms take.
fn s(app: &App, cx: f32, cy: f32) -> (f32, f32) {
    app.viewport.to_screen(cx, cy)
}

fn pump(app: &mut App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

const NO_PEN: [PenSample; 0] = [];

/// A pointer drag along a straight line through the real arms.
fn drag(app: &mut App, from: (f32, f32), to: (f32, f32)) {
    drag_path(app, &[from, to]);
}

/// A pointer drag through a list of canvas points (lasso, freehand).
fn drag_path(app: &mut App, pts: &[(f32, f32)]) {
    let (x0, y0) = s(app, pts[0].0, pts[0].1);
    app.canvas_down(x0, y0, PointerKind::Mouse, &NO_PEN);
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let steps = 24;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let (mx, my) = s(app, a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            app.canvas_move(mx, my, &NO_PEN);
        }
    }
    let last = *pts.last().unwrap();
    let (ux, uy) = s(app, last.0, last.1);
    app.canvas_up(ux, uy, &NO_PEN);
    pump(app);
}

/// One click at a canvas point through the real arms.
fn click(app: &mut App, cx: f32, cy: f32) {
    let (x, y) = s(app, cx, cy);
    app.canvas_down(x, y, PointerKind::Mouse, &NO_PEN);
    app.canvas_up(x, y, &NO_PEN);
    pump(app);
}

/// A brush stroke with real pressure samples (the SelPen and the ink
/// flows both need these — a pressureless move paints nothing).
fn brush_stroke(app: &mut App, from: (f32, f32), to: (f32, f32)) {
    brush_stroke_at(app, from, to, 0.9)
}

fn brush_stroke_at(app: &mut App, from: (f32, f32), to: (f32, f32), pressure: f32) {
    let steps = 40;
    let (x0, y0) = s(app, from.0, from.1);
    app.canvas_down(x0, y0, PointerKind::Mouse, &NO_PEN);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let (mx, my) = s(app, from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        app.canvas_move(
            mx,
            my,
            &[PenSample {
                x: mx,
                y: my,
                pressure,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            }],
        );
    }
    let (ux, uy) = s(app, to.0, to.1);
    app.canvas_up(ux, uy, &NO_PEN);
    pump(app);
}

/// Render this page the way an export would and drop the PNG where the
/// agent can look at it.
fn shot(app: &mut App, name: &str) -> image::RgbaImage {
    let dir = std::env::temp_dir().join(format!("mn-qa3-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let (w, h) = (app.doc.size.0, app.doc.size.1);
    let App { renderer, doc, .. } = app;
    let img = super::pages::render_offscreen_drafts_off(renderer, doc, w, h);
    let p = dir.join(format!("{name}.png"));
    img.save(&p).expect("write the shot");
    println!("[shot] {}", p.display());
    img
}

/// Luma of the exported pixel over white paper (0 = ink, 255 = paper).
fn luma(img: &image::RgbaImage, x: u32, y: u32) -> u8 {
    let p = img.get_pixel(x, y).0;
    let a = p[3] as f32 / 255.0;
    let mix = |c: u8| c as f32 * a + 255.0 * (1.0 - a);
    ((mix(p[0]) * 0.2126) + (mix(p[1]) * 0.7152) + (mix(p[2]) * 0.0722)) as u8
}

fn inked(img: &image::RgbaImage, x: u32, y: u32) -> bool {
    luma(img, x, y) < 128
}

/// How many pixels of the image read as ink.
fn ink_count(img: &image::RgbaImage) -> u32 {
    let (w, h) = img.dimensions();
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .filter(|&(x, y)| inked(img, x, y))
        .count() as u32
}

/// The selection's own coverage area, in canvas pixels — what the ants
/// enclose, independent of what has been painted into it.
fn sel_area(app: &App) -> u32 {
    let Some(sel) = app.doc.selection.as_ref() else {
        return 0;
    };
    let (w, h) = app.doc.size;
    let mut n = 0;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if mn_core::selection::selected(sel.coverage(x, y)) {
                n += 1;
            }
        }
    }
    n
}

/// The selection's bounding box in canvas pixels, or `None`.
fn sel_bounds(app: &App) -> Option<[i32; 4]> {
    app.doc.selection.as_ref().and_then(|s| s.bounds())
}

// =====================================================================
// S — selection tools
// =====================================================================

/// S1. Rectangle selection: drag it, then `Edit ▸ Fill` — the ink must
/// land exactly inside the dragged rectangle and nowhere else. This is
/// the flow every other selection flow is built on.
#[test]
fn qa_rectangle_selection_fills_exactly_what_was_dragged() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Select));
    dispatch(&mut app, AppCmd::SetSelectMode(SelectMode::Rect));
    drag(&mut app, (64.0, 48.0), (192.0, 160.0));

    let b = sel_bounds(&app).expect("a rectangle drag makes a selection");
    assert!(
        (b[0] - 64).abs() <= 1 && (b[1] - 48).abs() <= 1,
        "the ants start where the press did: {b:?}"
    );
    assert!(
        (b[2] - 191).abs() <= 1 && (b[3] - 159).abs() <= 1,
        "and end where the release did: {b:?}"
    );

    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    dispatch(&mut app, AppCmd::FillSelection);
    let img = shot(&mut app, "S1-rect-fill");
    assert!(inked(&img, 128, 100), "the middle of the rect is inked");
    assert!(!inked(&img, 60, 100), "nothing left of the left edge");
    assert!(!inked(&img, 200, 100), "nothing right of the right edge");
    assert!(!inked(&img, 128, 40), "nothing above the top edge");
    assert!(!inked(&img, 128, 170), "nothing below the bottom edge");
    // 128 x 112 = 14336; anti-aliasing on the rim is allowed a little slack.
    let n = ink_count(&img);
    assert!(
        (14000..=14700).contains(&n),
        "the fill is the rectangle's own area, not more: {n}"
    );
}

/// S2. Lasso: the freehand path IS the selection. A triangle drawn by
/// hand comes back as a triangle, not as its bounding box.
#[test]
fn qa_lasso_selection_keeps_the_shape_it_was_drawn_in() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Select));
    dispatch(&mut app, AppCmd::SetSelectMode(SelectMode::Lasso));
    // A right triangle: (40,40) → (200,40) → (200,200) → back.
    drag_path(
        &mut app,
        &[(40.0, 40.0), (200.0, 40.0), (200.0, 200.0), (40.0, 40.0)],
    );
    let area = sel_area(&app);
    // Half of a 160 x 160 box = 12800; the bbox would be 25600.
    assert!(
        (12000..=13600).contains(&area),
        "the lasso selected its own triangle, not the bbox: {area}"
    );
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    dispatch(&mut app, AppCmd::FillSelection);
    let img = shot(&mut app, "S2-lasso-fill");
    assert!(inked(&img, 180, 60), "inside the triangle");
    assert!(!inked(&img, 60, 180), "the corner the triangle cut off");
}

/// S3. Auto select (magic wand): one click inside pen-drawn walls selects
/// that region and stops at the ink.
#[test]
fn qa_wand_click_selects_the_enclosed_region() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    dispatch(&mut app, AppCmd::SetBrushSizePx(7.0));
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    for (a, b) in [
        ((48.0, 48.0), (208.0, 48.0)),
        ((208.0, 48.0), (208.0, 208.0)),
        ((208.0, 208.0), (48.0, 208.0)),
        ((48.0, 208.0), (48.0, 48.0)),
    ] {
        brush_stroke(&mut app, a, b);
    }
    dispatch(&mut app, AppCmd::SetTool(Tool::Wand));
    click(&mut app, 128.0, 128.0);
    let b = sel_bounds(&app).expect("the wand made a selection");
    assert!(
        b[0] >= 45 && b[1] >= 45 && b[2] <= 211 && b[3] <= 211,
        "the wand's region stopped at the walls: {b:?}"
    );
    let area = sel_area(&app);
    assert!(
        area > 20_000 && area < 26_000,
        "and it is the room inside them, not the page: {area}"
    );
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    dispatch(&mut app, AppCmd::FillSelection);
    let img = shot(&mut app, "S3-wand-fill");
    assert!(inked(&img, 128, 128), "the room is filled");
    assert!(!inked(&img, 8, 8), "the page outside the walls is clean");
}

/// S4. Selection pen adds coverage, selection eraser takes it away —
/// the pair CSP ships as 選択ペン / 選択消しゴム.
#[test]
fn qa_selection_pen_paints_a_selection_and_the_eraser_takes_it_back() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::SelPen));
    dispatch(&mut app, AppCmd::SetBrushSizePx(24.0));
    brush_stroke(&mut app, (64.0, 128.0), (192.0, 128.0));
    let painted = sel_area(&app);
    // Width comes from the brush (the tool paints coverage with the same
    // engine that inks) — the number to hold is that a 128 px stroke made
    // a 128 px-long band of ants, not how fat the G-pen draws.
    assert!(painted > 500, "the selection pen painted ants: {painted}");
    let b = sel_bounds(&app).expect("ants");
    assert!(
        b[0] <= 66 && b[2] >= 190,
        "the band runs the length of the stroke: {b:?}"
    );

    dispatch(&mut app, AppCmd::SetTool(Tool::SelEraser));
    dispatch(&mut app, AppCmd::SetBrushSizePx(24.0));
    brush_stroke(&mut app, (150.0, 128.0), (192.0, 128.0));
    let after = sel_area(&app);
    assert!(
        after < painted,
        "the selection eraser subtracted: {painted} → {after}"
    );
    assert!(after > 0, "and it only took the part it rubbed: {after}");
}

/// S5. `Select all` / `Deselect` / `Invert` / `Reselect` — the four keys
/// (Ctrl+A / Ctrl+D / Ctrl+Shift+I / Ctrl+Shift+D) every CSP hand knows.
#[test]
fn qa_select_all_deselect_invert_and_reselect() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::SelectAll);
    assert_eq!(
        sel_area(&app),
        W * H,
        "Select all takes the whole canvas"
    );

    dispatch(&mut app, AppCmd::SetTool(Tool::Select));
    dispatch(&mut app, AppCmd::SetSelectMode(SelectMode::Rect));
    drag(&mut app, (64.0, 64.0), (192.0, 192.0));
    let inside = sel_area(&app);
    dispatch(&mut app, AppCmd::SelectInvert);
    let outside = sel_area(&app);
    assert_eq!(
        inside + outside,
        W * H,
        "invert is the exact complement: {inside} + {outside}"
    );

    dispatch(&mut app, AppCmd::Deselect);
    assert!(app.doc.selection.is_none(), "Deselect clears the ants");
    dispatch(&mut app, AppCmd::Reselect);
    assert_eq!(
        sel_area(&app),
        outside,
        "Reselect brings the LAST selection back"
    );
}

/// S6. The four combine modes (CSP 選択モード): New replaces, Add unions,
/// Subtract cuts, Intersect keeps the overlap. Driven through the real
/// drag arm with the persistent Tool-Property mode, which is what a hand
/// without a modifier key held gets.
#[test]
fn qa_add_subtract_and_intersect_combine_two_drags() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Select));
    dispatch(&mut app, AppCmd::SetSelectMode(SelectMode::Rect));

    // Deselect first: in New mode a press that lands INSIDE the standing
    // ants grabs them to move (SE-039), so "start over" is a real
    // deselect, not another drag. That deviation is in the ledger.
    let first = |app: &mut App| {
        dispatch(app, AppCmd::Deselect);
        app.sel_op = mn_core::SelectionOp::Replace;
        drag(app, (32.0, 32.0), (160.0, 160.0));
    };
    // 128 x 128 = 16384 each; they overlap on 64 x 64 = 4096.
    first(&mut app);
    let one = sel_area(&app);
    assert!((16000..=16500).contains(&one), "first rect: {one}");

    app.sel_op = mn_core::SelectionOp::Add;
    drag(&mut app, (96.0, 96.0), (224.0, 224.0));
    let add = sel_area(&app);
    assert!(
        (28500..=29200).contains(&add),
        "Add = union (2 x 16384 - 4096): {add}"
    );

    first(&mut app);
    app.sel_op = mn_core::SelectionOp::Subtract;
    drag(&mut app, (96.0, 96.0), (224.0, 224.0));
    let sub = sel_area(&app);
    assert!(
        (12000..=12500).contains(&sub),
        "Subtract = 16384 - 4096: {sub}"
    );

    first(&mut app);
    app.sel_op = mn_core::SelectionOp::Intersect;
    drag(&mut app, (96.0, 96.0), (224.0, 224.0));
    let and = sel_area(&app);
    assert!(
        (3900..=4300).contains(&and),
        "Intersect = the 64 x 64 overlap: {and}"
    );
}

/// S7. The Selection Launcher's own commands: expand, shrink, blur,
/// invert, fill, clear-outside. Every one is pressed as the BUTTON
/// pushes it, and the numbers are read off the coverage.
#[test]
fn qa_launcher_expand_shrink_and_blur_move_the_edge() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Select));
    dispatch(&mut app, AppCmd::SetSelectMode(SelectMode::Rect));
    drag(&mut app, (64.0, 64.0), (192.0, 192.0));
    let base = sel_bounds(&app).unwrap();

    dispatch(&mut app, AppCmd::SelectExpand(8));
    let grown = sel_bounds(&app).unwrap();
    assert_eq!(
        [grown[0], grown[1]],
        [base[0] - 8, base[1] - 8],
        "expand 8 px pushed the edge out by 8: {base:?} → {grown:?}"
    );

    dispatch(&mut app, AppCmd::SelectShrink(8));
    let back = sel_bounds(&app).unwrap();
    assert_eq!(
        [back[0], back[1]],
        [base[0], base[1]],
        "shrink 8 px put it back: {back:?}"
    );

    // Feather: the edge stops being binary — some pixels read partial.
    dispatch(&mut app, AppCmd::SelectBlur(6));
    let sel = app.doc.selection.as_ref().unwrap();
    let partial = (60..70)
        .filter(|&x| {
            let c = sel.coverage(x, 128);
            c > 0 && c < 255
        })
        .count();
    assert!(partial > 0, "the blurred edge has graduated coverage");
}

/// S8. Transform of a selection: lift, drag, commit — the ink must land
/// where the pointer put it, and one undo takes the whole move back.
#[test]
fn qa_transform_moves_the_selected_ink_where_it_was_dragged() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Select));
    dispatch(&mut app, AppCmd::SetSelectMode(SelectMode::Rect));
    drag(&mut app, (32.0, 32.0), (160.0, 160.0));
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    dispatch(&mut app, AppCmd::FillSelection);
    let before = shot(&mut app, "S8a-before-transform");
    assert!(inked(&before, 64, 64), "the block starts top-left");
    assert!(!inked(&before, 200, 200), "and nothing is bottom-right");

    dispatch(&mut app, AppCmd::TransformStart);
    assert!(app.transform_drag.is_some(), "Ctrl+T lifted the selection");
    // Grab INSIDE the box, away from the pivot marker at its centre and
    // from every handle (those are the scale/rotate grabs), and pull it
    // 80 px right and down.
    let (dx0, dy0) = s(&app, 64.0, 132.0);
    let (dx1, dy1) = s(&app, 144.0, 212.0);
    app.canvas_down(dx0, dy0, PointerKind::Mouse, &NO_PEN);
    app.canvas_move(dx1, dy1, &NO_PEN);
    app.canvas_up(dx1, dy1, &NO_PEN);
    pump(&mut app);
    dispatch(&mut app, AppCmd::TransformCommit);

    let after = shot(&mut app, "S8b-after-transform");
    assert!(
        inked(&after, 200, 200),
        "the block landed under the pointer"
    );
    assert!(!inked(&after, 50, 50), "and left where it came from");

    dispatch(&mut app, AppCmd::Undo);
    let undone = shot(&mut app, "S8c-transform-undone");
    assert!(inked(&undone, 64, 64), "one undo put the block back");
    assert!(!inked(&undone, 200, 200), "and cleared where it had gone");
}


/// S9. The other two thirds of Transform on a selection: a CORNER drag
/// scales about the opposite corner (CSP's anchor, not the centre), and a
/// rotate turns the lifted pixels — both judged on the printed page.
#[test]
fn qa_transform_scales_from_a_corner_and_rotates_the_lifted_pixels() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Select));
    dispatch(&mut app, AppCmd::SetSelectMode(SelectMode::Rect));
    drag(&mut app, (64.0, 64.0), (128.0, 128.0));
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    dispatch(&mut app, AppCmd::FillSelection);

    // Corner (128,128) pulled to (192,192): x2 about the pinned (64,64).
    dispatch(&mut app, AppCmd::TransformStart);
    let (x0, y0) = s(&app, 128.0, 128.0);
    let (x1, y1) = s(&app, 192.0, 192.0);
    app.canvas_down(x0, y0, PointerKind::Mouse, &NO_PEN);
    app.canvas_move(x1, y1, &NO_PEN);
    app.canvas_up(x1, y1, &NO_PEN);
    pump(&mut app);
    dispatch(&mut app, AppCmd::TransformCommit);
    let img = shot(&mut app, "S9a-corner-scale");
    assert!(inked(&img, 70, 70), "the pinned corner stayed put");
    assert!(inked(&img, 185, 185), "and the block grew to the pointer");
    assert!(!inked(&img, 200, 200), "not past it");

    // A rotation: a wide bar becomes a tall one about its own centre.
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Select));
    dispatch(&mut app, AppCmd::SetSelectMode(SelectMode::Rect));
    drag(&mut app, (48.0, 80.0), (176.0, 112.0));
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    dispatch(&mut app, AppCmd::FillSelection);
    let before = shot(&mut app, "S9b-bar-before");
    assert!(inked(&before, 60, 96), "the bar starts wide");
    assert!(!inked(&before, 112, 40), "and short");

    dispatch(&mut app, AppCmd::TransformStart);
    dispatch(
        &mut app,
        AppCmd::TransformUpdate {
            sx: 1.0,
            sy: 1.0,
            rad: std::f32::consts::FRAC_PI_2,
            tx: 0.0,
            ty: 0.0,
        },
    );
    dispatch(&mut app, AppCmd::TransformCommit);
    let after = shot(&mut app, "S9c-bar-rotated");
    assert!(inked(&after, 112, 40), "a quarter turn makes it tall");
    assert!(!inked(&after, 60, 96), "and no longer wide");
}

/// S10. Ellipse selection (CSP 楕円選択, the sub tool beside Rectangle):
/// the same diagonal drag, but what fills is the ellipse INSIDE the
/// dragged box — the corners stay paper.
#[test]
fn qa_ellipse_selection_fills_the_oval_not_its_box() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Select));
    dispatch(&mut app, AppCmd::SetSelectMode(SelectMode::Ellipse));
    drag(&mut app, (48.0, 64.0), (208.0, 192.0));

    let b = sel_bounds(&app).expect("an ellipse drag makes a selection");
    assert!(
        (b[0] - 48).abs() <= 2 && (b[2] - 207).abs() <= 2,
        "the oval spans the drag: {b:?}"
    );
    // pi/4 of the 160 x 128 box = 16085; a rectangle would be 20480.
    let area = sel_area(&app);
    assert!(
        (15300..=16800).contains(&area),
        "the area is the ellipse's, not the box's: {area}"
    );

    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    dispatch(&mut app, AppCmd::FillSelection);
    let img = shot(&mut app, "S10-ellipse-fill");
    assert!(inked(&img, 128, 128), "the middle of the oval is inked");
    assert!(!inked(&img, 52, 68), "the box's top-left corner stays paper");
    assert!(
        !inked(&img, 204, 188),
        "and so does its bottom-right corner"
    );
    assert!(inked(&img, 128, 68), "but the top of the oval is inked");
    assert!(inked(&img, 52, 128), "and its left flank");
}
// =====================================================================
// P — frame folders, the comic page a mangaka actually starts from
// =====================================================================

/// Every frame folder header on the page, bottom of the stack first.
fn frame_headers(app: &App) -> Vec<usize> {
    app.doc
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.folder && l.is_frame())
        .map(|(i, _)| i)
        .collect()
}

/// The panel rectangle `fi` of the frame folder at `li`.
fn panel_bbox(app: &App, li: usize, fi: usize) -> [f32; 4] {
    app.doc.layers[li].frames().unwrap().frames[fi].bbox()
}

/// Along scanline `y`, the runs of INK between `x0` and `x1` — how the
/// border's thickness and the panel walls are counted.
fn ink_runs(img: &image::RgbaImage, y: u32, x0: u32, x1: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut run: Option<u32> = None;
    for x in x0..=x1 {
        if inked(img, x, y) {
            run.get_or_insert(x);
        } else if let Some(s) = run.take() {
            out.push((s, x - s));
        }
    }
    if let Some(s) = run {
        out.push((s, x1 + 1 - s));
    }
    out
}

/// Ink inside a horizontal band `x0..=x1`, five scanlines tall around
/// `y` — a stroke's tapered end is thin and wanders a pixel, so "did any
/// of this stroke print here" is a band question, not a pixel one.
fn band_ink(img: &image::RgbaImage, x0: u32, x1: u32, y: u32) -> u32 {
    let (w, h) = img.dimensions();
    let mut n = 0;
    for yy in y.saturating_sub(3)..(y + 4).min(h) {
        for xx in x0..=x1.min(w - 1) {
            if inked(img, xx, yy) {
                n += 1;
            }
        }
    }
    n
}

/// A small real comic page: `File ▸ New comic` at 72 dpi so the render is
/// a few hundred pixels, not a few thousand.
fn comic_page(app: &mut App) {
    super::new_document_tests::small_draft(app, 1, "");
    dispatch(app, AppCmd::NewComicCreate);
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
}

/// P1. A new comic page arrives with one frame folder whose border sits
/// on the inner border, inside the paper with margin all round — the page
/// a mangaka starts every chapter from.
#[test]
fn qa_a_new_comic_page_comes_with_a_panel_on_the_inner_border() {
    let Some(mut app) = headless() else { return };
    comic_page(&mut app);
    let heads = frame_headers(&app);
    assert_eq!(heads.len(), 1, "one frame folder seeds the page");
    let b = panel_bbox(&app, heads[0], 0);
    let (pw, ph) = app.doc.size;
    assert!(
        b[0] > 2.0 && b[1] > 2.0 && b[2] < pw as f32 - 2.0 && b[3] < ph as f32 - 2.0,
        "the panel sits inside the paper, not on it: {b:?} in {pw}x{ph}"
    );

    let img = shot(&mut app, "P1-new-comic-page");
    let (iw, ih) = img.dimensions();
    let my = ((b[1] + b[3]) * 0.5) as u32;
    let runs = ink_runs(&img, my, 0, iw - 1);
    assert_eq!(
        runs.len(),
        2,
        "across the panel's waist: left wall, right wall, nothing else: {runs:?}"
    );
    assert!(!inked(&img, 1, 1), "the paper corner is clean");
    assert!(!inked(&img, iw - 2, ih - 2), "and so is the far one");
}

/// P2. Divide frame border, level cut: two panels with the gutter the
/// Tool Property asks for — measured on the printed page, not just in the
/// geometry.
#[test]
fn qa_dividing_a_panel_leaves_the_gutter_the_tool_property_asks_for() {
    let Some(mut app) = headless() else { return };
    comic_page(&mut app);
    let head = frame_headers(&app)[0];
    app.doc.set_active(head);
    app.frame_mode = crate::cmd::FrameMode::DivideBorder;
    app.gutter_border_mm = (4.0, 4.0);
    let want = app.mm_to_px(4.0);

    let b = panel_bbox(&app, head, 0);
    let mid_y = (b[1] + b[3]) * 0.5;
    dispatch(
        &mut app,
        AppCmd::FrameDivide {
            a: (b[0] - 20.0, mid_y),
            b: (b[2] + 20.0, mid_y),
        },
    );
    let fs = app.doc.layers[head].frames().unwrap();
    assert_eq!(fs.frames.len(), 2, "the level drag cut the panel in two");
    let (f0, f1) = (fs.frames[0].bbox(), fs.frames[1].bbox());
    let (upper, lower) = if f0[1] < f1[1] { (f0, f1) } else { (f1, f0) };
    let gap = lower[1] - upper[3];
    assert!(
        (gap - want).abs() <= 1.0,
        "the gutter is the 4 mm asked for ({want:.1} px), measured {gap:.1} px"
    );

    let img = shot(&mut app, "P2-divide-horizontal");
    // Straight down the panel's middle: the widest white run between the
    // two panels IS the gutter as printed.
    let cx = ((upper[0] + upper[2]) * 0.5) as u32;
    let (mut paper, mut worst) = (0u32, 0u32);
    for y in (upper[3] as u32).saturating_sub(4)..(lower[1] as u32 + 4) {
        if inked(&img, cx, y) {
            worst = worst.max(paper);
            paper = 0;
        } else {
            paper += 1;
        }
    }
    worst = worst.max(paper);
    assert!(
        (worst as f32 - want).abs() <= 3.0,
        "the white gutter on the printed page is the asked-for {want:.1} px, not {worst}"
    );

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(
        app.doc.layers[head].frames().unwrap().frames.len(),
        1,
        "one undo takes the cut back"
    );
}

/// P3. An upright cut across the level one: the four-panel page every
/// chapter opener is.
#[test]
fn qa_a_second_cut_crosses_the_first_into_four_panels() {
    let Some(mut app) = headless() else { return };
    comic_page(&mut app);
    let head = frame_headers(&app)[0];
    app.doc.set_active(head);
    app.frame_mode = crate::cmd::FrameMode::DivideBorder;
    app.gutter_border_mm = (4.0, 4.0);
    let b = panel_bbox(&app, head, 0);
    let (mx, my) = ((b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5);
    dispatch(
        &mut app,
        AppCmd::FrameDivide {
            a: (b[0] - 20.0, my),
            b: (b[2] + 20.0, my),
        },
    );
    dispatch(
        &mut app,
        AppCmd::FrameDivide {
            a: (mx, b[1] - 20.0),
            b: (mx, b[3] + 20.0),
        },
    );
    assert_eq!(
        app.doc.layers[head].frames().unwrap().frames.len(),
        4,
        "a level cut and an upright cut = four panels"
    );
    let img = shot(&mut app, "P3-four-panels");
    let top = app.doc.layers[head]
        .frames()
        .unwrap()
        .frames
        .iter()
        .map(|f| f.bbox())
        .min_by(|a, c| a[1].total_cmp(&c[1]))
        .unwrap();
    let waist = ((top[1] + top[3]) * 0.5) as u32;
    let runs = ink_runs(&img, waist, 0, img.dimensions().0 - 1);
    assert_eq!(
        runs.len(),
        4,
        "two panels side by side = four walls on that scanline: {runs:?}"
    );
}

/// P4. The Border row is the ink's width on the page: the same panel at
/// 0.4 mm and at 1.6 mm inks visibly different walls.
#[test]
fn qa_the_border_width_row_is_the_ink_on_the_page() {
    let width_of = |mm: f32| -> u32 {
        let Some(mut app) = headless() else { return 0 };
        comic_page(&mut app);
        let head = frame_headers(&app)[0];
        let b = panel_bbox(&app, head, 0);
        // Through the door the Border row uses.
        let mut fs = app.doc.layers[head].frames().unwrap().clone();
        fs.border_px = app.mm_to_px(mm);
        dispatch(
            &mut app,
            AppCmd::FrameCommit {
                layer: head,
                frames: fs,
            },
        );
        let img = shot(&mut app, &format!("P4-border-{mm}mm"));
        let waist = ((b[1] + b[3]) * 0.5) as u32;
        let runs = ink_runs(&img, waist, 0, img.dimensions().0 - 1);
        assert_eq!(runs.len(), 2, "left wall and right wall: {runs:?}");
        runs[0].1
    };
    let thin = width_of(0.4);
    if thin == 0 {
        return; // no renderer
    }
    let fat = width_of(1.6);
    assert!(
        fat > thin * 2,
        "1.6 mm inks a visibly fatter wall than 0.4 mm: {thin} px vs {fat} px"
    );
}

/// P5. THE frame-folder promise: ink on a layer inside the folder is
/// clipped to the panel. A stroke that runs the width of the page stops
/// at the border and the paper outside stays paper.
#[test]
fn qa_ink_inside_a_frame_folder_is_clipped_to_the_panel() {
    let Some(mut app) = headless() else { return };
    comic_page(&mut app);
    let head = frame_headers(&app)[0];
    let b = panel_bbox(&app, head, 0);
    assert_eq!(
        app.doc.enclosing_frame_folder(app.doc.active),
        Some(head),
        "a new comic page leaves you on a layer INSIDE the panel"
    );
    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    dispatch(&mut app, AppCmd::SetBrushSizePx(9.0));
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    let y = (b[1] + b[3]) * 0.5;
    let right = app.doc.size.0 as f32 - 2.0;
    brush_stroke_at(&mut app, (2.0, y), (right, y), 1.0);

    let img = shot(&mut app, "P5-clipped-inside");
    let yy = y as u32;
    assert_eq!(
        band_ink(&img, 2, b[0] as u32 - 4, yy),
        0,
        "the half of the stroke left of the panel never printed"
    );
    assert_eq!(
        band_ink(&img, b[2] as u32 + 4, img.dimensions().0 - 3, yy),
        0,
        "nor the half right of it"
    );
    assert!(
        inked(&img, ((b[0] + b[2]) * 0.5) as u32, yy),
        "and the half inside the panel did"
    );
}

/// P6. The breakout (CSP's hand-made overflow, one tick here): the same
/// stroke on the same layer draws past the border, and unticking puts it
/// back inside.
#[test]
fn qa_burst_out_of_the_panel_lets_the_art_escape() {
    let Some(mut app) = headless() else { return };
    comic_page(&mut app);
    let head = frame_headers(&app)[0];
    let b = panel_bbox(&app, head, 0);
    let li = app.doc.active;
    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    dispatch(&mut app, AppCmd::SetBrushSizePx(9.0));
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    let y = (b[1] + b[3]) * 0.5;
    let right = app.doc.size.0 as f32 - 2.0;
    brush_stroke_at(&mut app, (2.0, y), (right, y), 1.0);

    dispatch(&mut app, AppCmd::SetLayerEscape(li, true));
    assert!(
        app.doc.layers[li].escape_frame,
        "the Layer Property tick took"
    );
    let img = shot(&mut app, "P6-burst-out");
    let yy = y as u32;
    assert!(
        band_ink(&img, 2, b[0] as u32 - 4, yy) > 0,
        "the art now prints left of the panel"
    );
    assert!(
        band_ink(&img, b[2] as u32 + 4, img.dimensions().0 - 3, yy) > 0,
        "and right of it"
    );

    dispatch(&mut app, AppCmd::SetLayerEscape(li, false));
    let back = shot(&mut app, "P6b-burst-unticked");
    assert_eq!(
        band_ink(&back, 2, b[0] as u32 - 4, yy),
        0,
        "unticking puts the art back inside the panel"
    );
}

/// P7. CSP's own advice for art that spans panels — "draw on a layer
/// ABOVE the frame folder" — must work here too: no clipping at all.
///
/// Getting such a layer is the awkward part, and this test pins the
/// route: New layer with the seeded draw layer active lands as its
/// SIBLING, still inside the panel (CSP-correct), and so does New layer
/// on an OPEN folder. The layer that is not clipped is the one added
/// with the folder COLLAPSED — which is the only door on a fresh comic
/// page, and is in the ledger as a discoverability item.
#[test]
fn qa_a_layer_above_the_frame_folder_is_not_clipped() {
    let Some(mut app) = headless() else { return };
    comic_page(&mut app);
    let head = frame_headers(&app)[0];
    let b = panel_bbox(&app, head, 0);

    // The two doors that keep you inside. (`head` moves as layers are
    // inserted below it — read it back each time.)
    dispatch(&mut app, AppCmd::AddLayer);
    assert_eq!(
        app.doc.enclosing_frame_folder(app.doc.active),
        Some(frame_headers(&app)[0]),
        "New layer beside the draw layer stays in the panel"
    );
    dispatch(&mut app, AppCmd::Undo);
    let head = frame_headers(&app)[0];
    app.doc.set_active(head);
    app.doc.layers[head].open = true;
    dispatch(&mut app, AppCmd::AddLayer);
    assert_eq!(
        app.doc.enclosing_frame_folder(app.doc.active),
        Some(frame_headers(&app)[0]),
        "New layer on an open frame folder goes inside it"
    );
    dispatch(&mut app, AppCmd::Undo);

    // The door that gets out: collapse the folder first.
    let head = frame_headers(&app)[0];
    app.doc.set_active(head);
    app.doc.layers[head].open = false;
    dispatch(&mut app, AppCmd::AddLayer);
    let li = app.doc.active;
    assert!(
        app.doc.enclosing_frame_folder(li).is_none(),
        "with the folder collapsed the new layer lands above it"
    );

    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    dispatch(&mut app, AppCmd::SetBrushSizePx(9.0));
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    let y = (b[1] + b[3]) * 0.5;
    let right = app.doc.size.0 as f32 - 2.0;
    brush_stroke_at(&mut app, (2.0, y), (right, y), 1.0);
    let img = shot(&mut app, "P7-above-the-folder");
    let yy = y as u32;
    assert!(
        band_ink(&img, 2, b[0] as u32 - 4, yy) > 0,
        "it prints outside the panel"
    );
    assert!(
        inked(&img, ((b[0] + b[2]) * 0.5) as u32, yy),
        "and inside it"
    );
}

/// P8. The yellow expand arrows (CSP's triangle icons): with a panel
/// picked in the Object tool every side that can still bleed offers one,
/// and taking one runs the edge off the paper.
#[test]
fn qa_the_expand_arrows_run_a_panel_edge_out_to_the_paper() {
    let Some(mut app) = headless() else { return };
    comic_page(&mut app);
    let head = frame_headers(&app)[0];
    dispatch(&mut app, AppCmd::SetTool(Tool::Object));
    app.object_sel = Some((head, 0));
    let arrows = app.frame_expand_arrow_pts();
    assert_eq!(
        arrows.len(),
        4,
        "a lone panel can bleed off all four sides: {arrows:?}"
    );

    let b = panel_bbox(&app, head, 0);
    dispatch(
        &mut app,
        AppCmd::FrameExtendEdge {
            at: ((b[0] + b[2]) * 0.5, b[1]),
        },
    );
    let after = panel_bbox(&app, head, 0);
    assert!(
        after[1] < 0.0,
        "the top edge ran off the paper: {b:?} -> {after:?}"
    );
    let img = shot(&mut app, "P8-bleed-panel");
    let cx = ((b[0] + b[2]) * 0.5) as u32;
    assert!(
        !inked(&img, cx, 1),
        "no border ink is left along the top of the page"
    );
}
