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
