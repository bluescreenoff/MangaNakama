//! Surface pass — the whole pipeline, end to end, on one short manga.
//!
//! The other surface families each walk one palette. This one walks the
//! production sequence a serialized chapter goes through — new work →
//! ネーム (rough on a draft layer) → コマ割り (panels) → ペン入れ (ink,
//! with rulers) → ベタ (solid black) → トーン (screen) → 写植 (lettering)
//! → 入稿 (export with トンボ and a proof sheet) — and asks at every seam
//! whether the page that comes out is the page the sequence asked for.
//!
//! **The book reads right to left.** Panels run right to left across a
//! tier, tiers top to bottom. Page 1's top tier carries a sentence broken
//! across two panels — the RIGHT panel says "I told you", the LEFT one
//! "never to come back." — which only parses in that order. The panel
//! order, the balloon geometry and the contact sheet are each checked
//! against it, because a page in the wrong order does not LOOK wrong: it
//! just puts every answer before its question.
//!
//! Set `MN_SURFACE_OUT=<dir>` to keep the stage PNGs.
//!
//! Frugality (same rule as `new_document_tests`): the work runs at 72 dpi,
//! so a page is 728 × 1032 px and a whole-page render costs 0.75 MP. A
//! 600 dpi page is 51 MP and puts the CI runner's software GPU out of
//! memory — never render one in a test.

use super::new_document_tests::headless;
use crate::app::{App, PenSample, PointerKind};
use crate::cmd::{AppCmd, BalloonMode, FrameMode, RulerKind, Tool, dispatch};

const NONE: [PenSample; 0] = [];

/// Working dpi. 72 keeps a page under a megapixel; see the module note.
const DPI: u32 = 72;

// --- plumbing ------------------------------------------------------------

fn pump(app: &mut App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

/// Canvas point → the SCREEN point the pointer arms take.
fn s(app: &App, cx: f32, cy: f32) -> (f32, f32) {
    app.viewport.to_screen(cx, cy)
}

/// A pen stroke through the real pointer arms, with pressure — a
/// pressureless move paints nothing.
fn stroke(app: &mut App, pts: &[(f32, f32)]) {
    let (x0, y0) = s(app, pts[0].0, pts[0].1);
    app.canvas_down(x0, y0, PointerKind::Pen, &NONE);
    let mut t = 0.0f64;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        for i in 1..=24 {
            let f = i as f32 / 24.0;
            let (mx, my) = s(app, a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
            t += 8.0;
            app.canvas_move(
                mx,
                my,
                &[PenSample {
                    x: mx,
                    y: my,
                    pressure: 0.95,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: t,
                }],
            );
        }
    }
    let last = *pts.last().unwrap();
    let (ux, uy) = s(app, last.0, last.1);
    app.canvas_up(ux, uy, &NONE);
    pump(app);
}

fn click(app: &mut App, cx: f32, cy: f32) {
    let (x, y) = s(app, cx, cy);
    app.canvas_down(x, y, PointerKind::Pen, &NONE);
    app.canvas_up(x, y, &NONE);
    pump(app);
}

fn drag(app: &mut App, a: (f32, f32), b: (f32, f32)) {
    let (x0, y0) = s(app, a.0, a.1);
    app.canvas_down(x0, y0, PointerKind::Pen, &NONE);
    for i in 1..=8 {
        let f = i as f32 / 8.0;
        let (mx, my) = s(app, a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
        app.canvas_move(mx, my, &NONE);
    }
    let (ux, uy) = s(app, b.0, b.1);
    app.canvas_up(ux, uy, &NONE);
    pump(app);
}

fn keep(img: &image::RgbaImage, name: &str) {
    if let Ok(dir) = std::env::var("MN_SURFACE_OUT") {
        let _ = std::fs::create_dir_all(&dir);
        img.save(format!("{dir}/{name}.png")).expect("png written");
    }
}

/// The page as it will PRINT: the export rules, drafts off.
fn render_page(app: &mut App) -> image::RgbaImage {
    let (w, h) = (app.doc.size.0, app.doc.size.1);
    let App { renderer, doc, .. } = app;
    super::pages::render_offscreen_drafts_off(renderer, doc, w, h)
}

fn shot(app: &mut App, name: &str) -> image::RgbaImage {
    let img = render_page(app);
    keep(&img, name);
    img
}

/// The page as it looks IN THE APP: drafts and all.
fn shot_screen(app: &mut App, name: &str) -> image::RgbaImage {
    let (w, h) = (app.doc.size.0, app.doc.size.1);
    let img = app.renderer.render_offscreen(&app.doc, w, h);
    keep(&img, name);
    img
}

fn inked(img: &image::RgbaImage, x: u32, y: u32) -> bool {
    if x >= img.width() || y >= img.height() {
        return false;
    }
    let p = img.get_pixel(x, y);
    (p[0] as u32 + p[1] as u32 + p[2] as u32) < 3 * 160 && p[3] > 0
}

/// Ink inside `[x0, y0, x1, y1]` (canvas px, clamped).
fn ink_in(img: &image::RgbaImage, r: [f32; 4]) -> u32 {
    let (x0, y0) = (r[0].max(0.0) as u32, r[1].max(0.0) as u32);
    let (x1, y1) = (r[2].max(0.0) as u32, r[3].max(0.0) as u32);
    let mut n = 0;
    for y in y0..y1.min(img.height()) {
        for x in x0..x1.min(img.width()) {
            if inked(img, x, y) {
                n += 1;
            }
        }
    }
    n
}

/// Bounding box of the ink inside `r`, canvas px, or None.
fn ink_bbox_in(img: &image::RgbaImage, r: [f32; 4]) -> Option<[u32; 4]> {
    let (x0, y0) = (r[0].max(0.0) as u32, r[1].max(0.0) as u32);
    let (x1, y1) = (r[2].max(0.0) as u32, r[3].max(0.0) as u32);
    let mut bb: Option<[u32; 4]> = None;
    for y in y0..y1.min(img.height()) {
        for x in x0..x1.min(img.width()) {
            if inked(img, x, y) {
                bb = Some(match bb {
                    None => [x, y, x + 1, y + 1],
                    Some(b) => [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)],
                });
            }
        }
    }
    bb
}

/// Blue-ish pixels — the rough is drawn in blue, the way a 下描き is, so
/// "is the rough on this render" is one census.
fn rough_px(img: &image::RgbaImage) -> u32 {
    img.pixels()
        .filter(|p| p[3] > 0 && p[2] > 150 && p[0] < 120)
        .count() as u32
}

/// Inked / clear census over a rect: a SCREEN prints both, a flat slab
/// only one.
fn dot_census(img: &image::RgbaImage, r: [f32; 4]) -> (u32, u32) {
    let (x0, y0) = (r[0].max(0.0) as u32, r[1].max(0.0) as u32);
    let (x1, y1) = (r[2].max(0.0) as u32, r[3].max(0.0) as u32);
    let (mut on, mut off) = (0, 0);
    for y in y0..y1.min(img.height()) {
        for x in x0..x1.min(img.width()) {
            if inked(img, x, y) {
                on += 1;
            } else {
                off += 1;
            }
        }
    }
    (on, off)
}

fn frame_headers(app: &App) -> Vec<usize> {
    app.doc
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.folder && l.is_frame())
        .map(|(i, _)| i)
        .collect()
}

fn panels(app: &App, li: usize) -> Vec<[f32; 4]> {
    app.doc.layers[li]
        .frames()
        .unwrap()
        .frames
        .iter()
        .map(|f| f.bbox())
        .collect()
}

fn centre(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5)
}

fn black(app: &mut App) {
    dispatch(app, AppCmd::SetTool(Tool::Pen));
    dispatch(app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    dispatch(app, AppCmd::SetBrushSizePx(4.0));
}

// --- the work ------------------------------------------------------------

/// `File ▸ New ▸ Comic`: four pages, right-bound, a frame border folder on
/// each. CSP: File ▸ New, pick "Comic", type the page count, OK — one
/// dialog. Ours: the same one dialog.
fn new_work(app: &mut App) {
    let mut setup = mn_core::PageSetup::presets().remove(0);
    setup.dpi = DPI;
    app.new_doc_draft.setup = setup;
    app.new_doc_draft.pages = 4;
    app.new_doc_draft.binding_right = true;
    app.new_doc_draft.frame_folder = true;
    app.new_doc_draft.story = "Storm".into();
    dispatch(app, AppCmd::NewComicCreate);
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
}

/// The rough (ネーム / 下描き): a draft-flagged layer ABOVE the frame
/// folder, scribbled in blue.
///
/// Getting a layer above the folder is the awkward part and it is a known
/// discoverability row: New layer with the seeded draw layer active lands
/// INSIDE the panel. The folder has to be collapsed and selected first —
/// two steps CSP does not charge, because its Layer palette shows the
/// folder and you click beside it.
fn rough_layer(app: &mut App) -> usize {
    let head = frame_headers(app)[0];
    app.doc.set_active(head);
    app.doc.layers[head].open = false;
    dispatch(app, AppCmd::AddLayer);
    let li = app.doc.active;
    dispatch(app, AppCmd::RenameLayer(li, "name".into()));
    dispatch(app, AppCmd::SetLayerDraft(li, true));
    li
}

/// Cut `page` into the tiers the storyboard asks for and return the panel
/// rects in READING ORDER (right to left, top to bottom).
///
/// `split_lower` = the two-panel tier is the LOWER one (page 2's shape);
/// otherwise it is the upper one (page 1's).
fn cut_page(app: &mut App, split_lower: bool) -> Vec<[f32; 4]> {
    let head = frame_headers(app)[0];
    app.doc.set_active(head);
    app.frame_mode = FrameMode::DivideBorder;
    app.gutter_border_mm = (4.0, 4.0);
    let b = panels(app, head)[0];
    let cut_y = b[1] + (b[3] - b[1]) * if split_lower { 0.45 } else { 0.42 };
    dispatch(
        app,
        AppCmd::FrameDivide {
            a: (b[0] - 20.0, cut_y),
            b: (b[2] + 20.0, cut_y),
        },
    );
    let head = frame_headers(app)[0];
    let two = panels(app, head)
        .into_iter()
        .find(|r| (centre(*r).1 > cut_y) == split_lower)
        .expect("the tier to split again");
    let mx = (two[0] + two[2]) * 0.5;
    dispatch(
        app,
        AppCmd::FrameDivide {
            a: (mx, two[1] + 4.0),
            b: (mx, two[3] - 4.0),
        },
    );
    reading_order(app)
}

/// The app's own computed panel order, as rects. This is the RTL oracle:
/// `mn_core::frame_order` is what the Layers badges and the on-canvas
/// reading path show, and it takes `binding_right` from the work.
fn reading_order(app: &mut App) -> Vec<[f32; 4]> {
    app.renumber_frames();
    app.ensure_frame_order();
    let order = app.frame_order.clone().expect("a reading order");
    order
        .panels
        .iter()
        .map(|p| app.doc.layers[p.layer].frames().unwrap().frames[p.frame].bbox())
        .collect()
}

/// Put the pen back on a layer INSIDE the frame folder, which is where
/// panel art belongs (and where the folder's mask can clip it).
fn draw_inside_frame(app: &mut App) {
    let li = (0..app.doc.layers.len())
        .find(|&i| !app.doc.layers[i].folder && app.doc.enclosing_frame_folder(i).is_some())
        .expect("the seeded draw layer inside the frame folder");
    app.doc.set_active(li);
}

/// An ellipse balloon dragged inside `panel`, and lettering clicked into
/// it — CSP's two tools, two gestures.
fn balloon_with_words(app: &mut App, panel: [f32; 4], words: &str, scale: f32) {
    let (cx, cy) = centre(panel);
    let rx = (panel[2] - panel[0]) * 0.34 * scale;
    let ry = (panel[3] - panel[1]) * 0.17 * scale;
    app.tool = Tool::Balloon;
    app.balloon_mode = BalloonMode::Ellipse;
    drag(app, (cx - rx, cy - ry), (cx + rx, cy + ry));

    dispatch(app, AppCmd::SetTool(Tool::Text));
    app.text_vertical = false;
    app.text_size_pt = 16.0;
    click(
        app,
        cx - rx * 0.62,
        cy - 12.0 * words.lines().count() as f32,
    );
    // A click-placed box does not know the balloon it landed in, so the
    // line breaks are the letterer's — see the ledger's L-03 row.
    for c in words.chars() {
        if c == '\n' {
            app.text_key(0x0D, false, false);
            continue;
        }
        let mut buf = [0u16; 2];
        for u in c.encode_utf16(&mut buf) {
            app.text_char(*u);
        }
    }
    app.text_key(0x1B, false, false); // Esc closes the box
    pump(app);
}

fn balloon_bboxes(app: &App) -> Vec<[f32; 4]> {
    let mut out = Vec::new();
    for l in &app.doc.layers {
        let Some(bs) = l.balloons() else { continue };
        for b in &bs.balloons {
            if let mn_core::BalloonShape::Ellipse { center, radii } = b.shape {
                out.push([
                    center[0] - radii[0],
                    center[1] - radii[1],
                    center[0] + radii[0],
                    center[1] + radii[1],
                ]);
            }
        }
    }
    out
}

fn text_items(app: &App) -> Vec<mn_core::text::TextItem> {
    app.doc
        .layers
        .iter()
        .filter_map(|l| l.texts())
        .flat_map(|t| t.texts.iter().cloned())
        .collect()
}

fn inside(outer: [f32; 4], inner: [f32; 4]) -> bool {
    inner[0] >= outer[0] - 1.0
        && inner[1] >= outer[1] - 1.0
        && inner[2] <= outer[2] + 1.0
        && inner[3] <= outer[3] + 1.0
}

// =========================================================================
// The pipeline, one pass
// =========================================================================

/// New comic → rough → panels → ink with rulers → beta → tone → lettering
/// → Export All with トンボ and a contact sheet, in that order, on one
/// four-page work. Every stage renders a PNG and asserts on it.
#[test]
fn e2e_a_short_manga_walks_the_whole_pipeline() {
    let Some(mut app) = headless() else { return };

    // --- 01 new work ------------------------------------------------
    new_work(&mut app);
    let (pw, ph) = app.doc.size;
    println!(
        "[note] page = {pw}x{ph} px at {DPI} dpi, {} pages",
        app.pages.len()
    );
    assert_eq!(app.pages.len(), 4, "four pages");
    assert!(
        app.binding_right,
        "right-bound: the book reads right to left"
    );
    assert_eq!(
        frame_headers(&app).len(),
        1,
        "page 1 seeds one frame folder"
    );
    let img = shot(&mut app, "01-new-work");
    assert!(!inked(&img, 2, 2), "the paper margin is clean");
    assert!(
        ink_in(&img, [0.0, 0.0, pw as f32, ph as f32]) > 0,
        "the border inked"
    );

    // --- 02 the rough on a draft layer ------------------------------
    let rough = rough_layer(&mut app);
    assert!(
        app.doc.enclosing_frame_folder(rough).is_none(),
        "the name layer sits ABOVE the frame folder, not inside a panel"
    );
    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    dispatch(&mut app, AppCmd::SetSlotColor([0.15, 0.35, 0.95]));
    dispatch(&mut app, AppCmd::SetBrushSizePx(7.0));
    let b0 = panels(&app, frame_headers(&app)[0])[0];
    let steps_before = app.doc.undo_len();
    for k in 0..3 {
        let y = b0[1] + (b0[3] - b0[1]) * (0.18 + 0.3 * k as f32);
        stroke(
            &mut app,
            &[
                (b0[0] + 24.0, y),
                (b0[2] - 40.0, y + 26.0),
                (b0[0] + 60.0, y + 62.0),
            ],
        );
    }
    // One rough line laid deliberately across the corner the ベタ will go
    // in later. A 下書き is not printed, so it must not stop the bucket
    // either — which is exactly what it used to do (see the beta stage).
    stroke(
        &mut app,
        &[
            (
                b0[0] + (b0[2] - b0[0]) * 0.55,
                b0[1] + (b0[3] - b0[1]) * 0.80,
            ),
            (b0[2] - 30.0, b0[1] + (b0[3] - b0[1]) * 0.84),
        ],
    );
    assert_eq!(
        app.doc.undo_len(),
        steps_before + 4,
        "four rough strokes, four undo steps"
    );
    let screen = shot_screen(&mut app, "02-name-layer");
    let printed = shot(&mut app, "02-name-layer-export");
    let (on_screen, on_paper) = (rough_px(&screen), rough_px(&printed));
    println!("[note] rough pixels: on screen {on_screen}, on the printed page {on_paper}");
    assert!(
        on_screen > 500,
        "the rough is visible in the app ({on_screen})"
    );
    assert_eq!(on_paper, 0, "and never reaches the export ({on_paper})");
    // One Ctrl+Z takes back exactly the last stroke.
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(
        app.doc.undo_len(),
        steps_before + 3,
        "one press, one stroke"
    );
    dispatch(&mut app, AppCmd::Redo);
    assert_eq!(app.doc.undo_len(), steps_before + 4);

    // --- 03 panels (コマ割り) ---------------------------------------
    let order = cut_page(&mut app, false);
    assert_eq!(order.len(), 3, "two panels over one wide one: {order:?}");
    let (p1, p2, p3) = (order[0], order[1], order[2]);
    println!("[note] reading order: P1 {p1:?} P2 {p2:?} P3 {p3:?}");
    // THE RTL PROOF, first half: reading position 1 is the RIGHT panel of
    // the top tier, position 2 the LEFT one, position 3 the tier below.
    assert!(
        centre(p1).0 > centre(p2).0,
        "panel 1 is to the RIGHT of panel 2 — the book reads right to left"
    );
    assert!(
        (centre(p1).1 - centre(p2).1).abs() < (p1[3] - p1[1]) * 0.5,
        "…and they share a tier"
    );
    assert!(
        centre(p3).1 > centre(p1).1,
        "the wide panel is the tier below"
    );
    let img = shot(&mut app, "03-panels");
    let waist = ((p1[1] + p1[3]) * 0.5) as u32;
    let walls = (0..pw).filter(|&x| inked(&img, x, waist)).count();
    println!("[note] ink along the top tier's waist: {walls} px of wall");
    assert!(walls >= 8, "four panel walls cross that scanline: {walls}");
    // The gutter between P1 and P2 is paper.
    let gut = (p1[0] + p2[2]) * 0.5;
    assert!(!inked(&img, gut as u32, waist), "the gutter is paper");
    let steps = app.doc.undo_len();
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(
        app.doc.layers[frame_headers(&app)[0]]
            .frames()
            .unwrap()
            .frames
            .len(),
        2,
        "one press takes back the upright cut only"
    );
    dispatch(&mut app, AppCmd::Redo);
    assert_eq!(app.doc.undo_len(), steps, "and redo puts it back");

    // --- 04 ペン入れ: figures by hand, background on a ruler ---------
    draw_inside_frame(&mut app);
    black(&mut app);
    // Two figure blobs, one per top-tier panel, the left one smaller.
    let blob = |app: &mut App, r: [f32; 4], k: f32| {
        let (cx, cy) = centre(r);
        let (w, h) = ((r[2] - r[0]) * 0.16 * k, (r[3] - r[1]) * 0.20 * k);
        let cy = cy + (r[3] - r[1]) * 0.22;
        stroke(app, &[(cx - w, cy + h), (cx, cy - h), (cx + w, cy + h)]);
        stroke(app, &[(cx - w, cy + h), (cx + w, cy + h)]);
        stroke(app, &[(cx, cy - h), (cx, cy - h * 1.9)]);
    };
    blob(&mut app, p1, 1.0);
    blob(&mut app, p2, 0.62);

    // The parallel ruler: drag the direction once, then every stroke comes
    // out parallel to it. CSP: Layer ▸ Ruler ▸ Special ruler ▸ Parallel
    // line, then drag. Ours: the same arm-then-drag.
    dispatch(&mut app, AppCmd::RulerArm(RulerKind::Parallel));
    drag(
        &mut app,
        (p3[0] + 20.0, p3[3] - 40.0),
        (p3[2] - 20.0, p3[3] - 40.0),
    );
    assert_eq!(app.doc.rulers.items.len(), 1, "one parallel ruler");
    assert!(app.doc.rulers.on, "and creating it turned snapping on");
    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    dispatch(&mut app, AppCmd::SetBrushSizePx(3.0));
    // Deliberately CROOKED drags: 40 px of drift over the run. The ruler
    // is what makes them come out level. The lines stop at the panel's
    // midline — the ベタ goes in the clear bottom-right corner, and a
    // bucket stops at any line it meets.
    let run_x1 = p3[0] + (p3[2] - p3[0]) * 0.52;
    let mut floor_ys = Vec::new();
    for k in 0..6 {
        let y = p3[1] + (p3[3] - p3[1]) * (0.46 + 0.085 * k as f32);
        floor_ys.push(y);
        stroke(&mut app, &[(p3[0] + 8.0, y), (run_x1, y + 40.0)]);
        if k == 0 {
            // Line 0 while it is still alone on the panel: a 40 px drop
            // over the run would put its ink in a 45 px tall box.
            let probe = render_page(&mut app);
            let bb = ink_bbox_in(&probe, [p3[0] + 6.0, y - 45.0, run_x1 + 6.0, y + 45.0])
                .expect("the first ruled line inked");
            println!("[note] first ruled line, ink box: {bb:?} (drag dropped 40 px)");
            assert!(
                bb[3] - bb[1] <= 12,
                "the parallel ruler flattened the crooked drag: {bb:?}"
            );
        }
    }
    // Snapping OFF again for the freehand work (CSP: the same one toggle).
    dispatch(&mut app, AppCmd::RulerSnapToggle);
    assert!(!app.doc.rulers.on, "the snap toggle let go");

    // A hand-drawn stroke crossing the ruled lines, run out PAST the panel
    // so the frame folder's mask has something to cut.
    dispatch(&mut app, AppCmd::SetBrushSizePx(5.0));
    let cross_x = p3[0] + (p3[2] - p3[0]) * 0.30;
    stroke(
        &mut app,
        &[(cross_x, p3[1] + 12.0), (cross_x, ph as f32 - 4.0)],
    );
    let img = shot(&mut app, "04-ink-rulers");
    // Ruled lines: six bands of ink across the panel's width.
    let run = run_x1 - p3[0] - 8.0;
    for (k, y) in floor_ys.iter().enumerate() {
        let band = ink_in(&img, [p3[0] + 12.0, y - 4.0, run_x1 - 4.0, y + 4.0]);
        println!("[note] ruled line {k}: {band} px inside an 8 px band over a {run:.0} px run");
        assert!(band > run as u32 / 2, "ruled line {k} printed ({band})");
    }
    // THE CLIP PROOF: the hand stroke ran to the bottom of the PAPER, and
    // the frame folder cut it at the panel edge.
    assert!(
        ink_in(
            &img,
            [cross_x - 6.0, p3[3] - 30.0, cross_x + 6.0, p3[3] - 6.0]
        ) > 0,
        "the stroke inked inside the panel"
    );
    let outside = ink_in(&img, [cross_x - 6.0, p3[3] + 6.0, cross_x + 6.0, ph as f32]);
    assert_eq!(
        outside, 0,
        "and nothing of it printed below the panel ({outside})"
    );

    // --- 05 ベタ: a solid black shadow, and it must not leak ---------
    // A closed shape in the panel's bottom-right corner, crossing the
    // lowest ruled line, then one bucket click inside it.
    let (sx0, sy0) = (
        p3[0] + (p3[2] - p3[0]) * 0.66,
        p3[1] + (p3[3] - p3[1]) * 0.62,
    );
    let (sx1, sy1) = (p3[2] - 24.0, p3[3] - 24.0);
    dispatch(&mut app, AppCmd::SetBrushSizePx(6.0));
    stroke(
        &mut app,
        &[(sx0, sy0), (sx1, sy0), (sx1, sy1), (sx0, sy1), (sx0, sy0)],
    );
    let before = app.doc.undo_len();
    dispatch(&mut app, AppCmd::SetTool(Tool::Fill));
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    click(&mut app, (sx0 + sx1) * 0.5, (sy0 + sy1) * 0.5);
    assert_eq!(app.doc.undo_len(), before + 1, "one click, one undo step");
    let img = shot(&mut app, "05-beta");
    let (on, off) = dot_census(&img, [sx0 + 12.0, sy0 + 12.0, sx1 - 12.0, sy1 - 12.0]);
    println!("[note] beta inside the shape: {on} inked / {off} clear");
    assert!(off * 20 < on, "the shape came out SOLID black ({on}/{off})");
    // …and the rest of the panel is still paper. Sample the strip just
    // ABOVE the shape: inside the panel, clear of the tone, clear of the
    // ruled lines, clear of the hand stroke. A flood that jumped the
    // outline would print here.
    let leak = ink_in(
        &img,
        [
            sx0,
            p3[1] + (p3[3] - p3[1]) * 0.45,
            p3[2] - 24.0,
            p3[1] + (p3[3] - p3[1]) * 0.56,
        ],
    );
    println!("[note] beta leak into the far corner: {leak} px");
    assert!(
        leak < 40,
        "the fill stopped at the line, it did not flood the panel ({leak})"
    );
    dispatch(&mut app, AppCmd::Undo);
    let (on2, _) = dot_census(
        &{
            let (w, h) = (app.doc.size.0, app.doc.size.1);
            let App { renderer, doc, .. } = &mut app;
            super::pages::render_offscreen_drafts_off(renderer, doc, w, h)
        },
        [sx0 + 12.0, sy0 + 12.0, sx1 - 12.0, sy1 - 12.0],
    );
    assert!(
        on2 * 4 < on,
        "one Ctrl+Z takes the beta back ({on} -> {on2})"
    );
    dispatch(&mut app, AppCmd::Redo);

    // --- 06 トーン: a screen over the panel's upper half -------------
    let sky = [
        p3[0] + 14.0,
        p3[1] + 14.0,
        p3[2] - 14.0,
        p3[1] + (p3[3] - p3[1]) * 0.40,
    ];
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, sky[0], sky[1], sky[2], sky[3],
    ));
    let layers_before = app.doc.layers.len();
    dispatch(
        &mut app,
        AppCmd::NewLiveFill(mn_core::FillKind::Tone {
            // 12 lpi at 72 dpi = a 6 px cell: a screen a test can COUNT.
            // A real 60 lpi screen at 600 dpi is the same 10 px cell.
            tone: mn_core::tone::ToneParams {
                lpi: 12.0,
                ..Default::default()
            },
            density: 0.45,
        }),
    );
    app.refresh_tones();
    assert_eq!(
        app.doc.layers.len(),
        layers_before + 1,
        "one live tone layer"
    );
    let ti = app.doc.active;
    assert!(
        matches!(
            app.doc.layers[ti].kind,
            mn_core::LayerKind::Fill(mn_core::FillKind::Tone { .. })
        ),
        "…and it is LIVE, editable a week later: {:?}",
        app.doc.layers[ti].kind
    );
    // Where the tone LANDED. CSP puts a tone made inside a panel into the
    // frame folder so the panel clips it; so does ours — the live fill
    // joins the active layer's folder rather than the top of the stack.
    assert!(
        app.doc.enclosing_frame_folder(ti).is_some(),
        "the tone joined the frame folder, so the panel clips it"
    );
    app.doc.selection = None;
    let img = shot(&mut app, "06-tone");
    let (on, off) = dot_census(
        &img,
        [sky[0] + 10.0, sky[1] + 10.0, sky[2] - 10.0, sky[3] - 10.0],
    );
    let share = on as f32 / (on + off).max(1) as f32;
    println!("[note] tone census: {on} inked / {off} clear, coverage {share:.2}");
    assert!(on > 0 && off > 0, "DOTS, not a flat slab ({on}/{off})");
    assert!(
        (0.15..0.75).contains(&share),
        "coverage {share:.2} at 45 % density"
    );

    // --- 07 写植: balloons and lettering ----------------------------
    balloon_with_words(&mut app, p1, "I told you", 1.0);
    balloon_with_words(&mut app, p2, "never to\ncome back.", 0.92);
    let bubbles = balloon_bboxes(&app);
    assert_eq!(bubbles.len(), 2, "two bubbles");
    let words = text_items(&app);
    assert_eq!(words.len(), 2, "two lettering items");
    // Each bubble is in its own panel, each item inside its bubble.
    for (bb, panel) in bubbles.iter().zip([p1, p2]) {
        assert!(
            inside(panel, *bb),
            "balloon {bb:?} sits inside panel {panel:?}"
        );
    }
    for it in &words {
        let r = [
            it.pos[0],
            it.pos[1],
            it.pos[0] + it.size[0],
            it.pos[1] + it.size[1],
        ];
        let home = bubbles
            .iter()
            .find(|b| inside(**b, r))
            .unwrap_or_else(|| panic!("lettering {:?} at {r:?} escaped its balloon", it.text));
        println!("[note] {:?} in balloon {home:?}", it.text);
    }
    // THE SPLIT-SENTENCE PROOF: sort the lettering right to left and the
    // sentence comes back whole. Left to right it is nonsense.
    let mut rtl = words.clone();
    rtl.sort_by(|a, b| b.pos[0].total_cmp(&a.pos[0]));
    let read: Vec<String> = rtl.iter().map(|t| t.text.replace('\n', " ")).collect();
    println!("[note] read right to left: {}", read.join(" "));
    assert_eq!(
        read,
        vec!["I told you", "never to come back."],
        "the sentence only parses right to left"
    );
    let img = shot(&mut app, "07-text-balloons");
    for (bb, label) in bubbles.iter().zip(["P1", "P2"]) {
        let n = ink_in(&img, [bb[0] + 6.0, bb[1] + 6.0, bb[2] - 6.0, bb[3] - 6.0]);
        println!("[note] ink inside the {label} bubble: {n} px");
        assert!(n > 60, "the {label} bubble has words printed in it ({n})");
    }

    // --- pages 2, 3, 4 ----------------------------------------------
    page_two(&mut app);
    for p in [2usize, 3] {
        dispatch(&mut app, AppCmd::SelectPage(p));
        one_panel_with_a_ruled_line(&mut app, p);
    }

    // --- 08/09/10 入稿: Export All, トンボ, contact sheet ------------
    dispatch(&mut app, AppCmd::SelectPage(0));
    export_stage(&mut app);
}

/// Page 2: a wide top panel of speed lines off a RADIAL ruler, then a
/// two-panel tier — right bubble "…", left a 描き文字 "END" with an edge.
fn page_two(app: &mut App) {
    dispatch(app, AppCmd::SelectPage(1));
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
    let order = cut_page(app, true);
    assert_eq!(order.len(), 3, "one wide panel over two");
    let (p4, p5, p6) = (order[0], order[1], order[2]);
    assert!(
        centre(p4).1 < centre(p5).1,
        "the wide panel is the top tier"
    );
    assert!(
        centre(p5).0 > centre(p6).0,
        "and the lower tier reads right to left too"
    );

    draw_inside_frame(app);
    black(app);
    // CSP "Special ruler ▸ Radial line" (集中線): CLICK the centre, then
    // every stroke runs along the line through it. Two steps, same as ours.
    let (cx, cy) = centre(p4);
    // Ledger row R-04, closed 2026-09-04: rulers are PER PAGE now. Page 1's
    // parallel ruler stayed on page 1, so page 2 opens clean and the radial
    // is the only ruler the pen can hear — no hand-clearing step here.
    println!(
        "[note] page 2 opened with {} rulers (page 1's parallel stayed home)",
        app.doc.rulers.items.len()
    );
    assert!(
        app.doc.rulers.items.is_empty(),
        "the page turn left page 1's ruler on page 1: {:?}",
        app.doc.rulers.items
    );
    dispatch(app, AppCmd::RulerArm(RulerKind::Radial));
    click(app, cx, cy);
    assert_eq!(app.doc.rulers.items.len(), 1, "one radial ruler, one click");
    dispatch(app, AppCmd::SetTool(Tool::Pen));
    dispatch(app, AppCmd::SetBrushSizePx(3.0));
    let r = (p4[2] - p4[0]) * 0.5;
    for k in 0..8 {
        let a = k as f32 / 8.0 * std::f32::consts::TAU;
        let (dx, dy) = (a.cos(), a.sin());
        stroke(
            app,
            &[
                (cx + dx * r * 0.30, cy + dy * r * 0.30),
                (cx + dx * r * 0.92, cy + dy * r * 0.55),
            ],
        );
    }
    let img = shot(app, "04b-radial-speed-lines");
    let n = ink_in(&img, [p4[0] + 4.0, p4[1] + 4.0, p4[2] - 4.0, p4[3] - 4.0]);
    println!("[note] page 2 speed lines: {n} px of ink in the top panel");
    assert!(n > 400, "the speed lines printed ({n})");

    balloon_with_words(app, p5, "...", 0.7);

    // 描き文字 stand-in: "END" set large with an edge (フチ) on.
    dispatch(app, AppCmd::SetTool(Tool::Text));
    app.text_vertical = false;
    app.text_size_pt = 40.0;
    app.text_outline_mm = 1.2;
    let (ex, ey) = centre(p6);
    click(app, ex - 46.0, ey - 26.0);
    for c in "END".chars() {
        let mut buf = [0u16; 2];
        for u in c.encode_utf16(&mut buf) {
            app.text_char(*u);
        }
    }
    app.text_key(0x1B, false, false);
    pump(app);
    let end = text_items(app)
        .into_iter()
        .find(|t| t.text == "END")
        .expect("the 描き文字");
    assert!(
        end.outline_px > 0.0,
        "it carries an edge: {}",
        end.outline_px
    );
    let img = shot(app, "07b-page2-lettering");
    assert!(
        ink_in(&img, [p6[0] + 4.0, p6[1] + 4.0, p6[2] - 4.0, p6[3] - 4.0]) > 60,
        "END printed in the left panel"
    );
}

/// Pages 3 and 4: the seeded single panel, plus one straight-ruler stroke,
/// so the contact sheet has four pages that differ.
fn one_panel_with_a_ruled_line(app: &mut App, page0: usize) {
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
    let head = frame_headers(app)[0];
    let b = panels(app, head)[0];
    draw_inside_frame(app);
    black(app);
    // Per-page rulers again: nothing rode along from the page before, so
    // the straight ruler drawn here is the only one on this page.
    assert!(
        app.doc.rulers.items.is_empty(),
        "page {} opened clean: {:?}",
        page0 + 1,
        app.doc.rulers.items
    );
    dispatch(app, AppCmd::RulerArm(RulerKind::Line));
    let y = b[1] + (b[3] - b[1]) * 0.5;
    drag(app, (b[0] + 20.0, y), (b[2] - 20.0, y));
    dispatch(app, AppCmd::SetTool(Tool::Pen));
    dispatch(app, AppCmd::SetBrushSizePx(3.0));
    stroke(app, &[(b[0] + 24.0, y + 14.0), (b[2] - 24.0, y - 14.0)]);
    let img = shot(app, &format!("04c-page{}-ruled", page0 + 1));
    assert!(
        ink_in(&img, [b[0] + 20.0, y - 8.0, b[2] - 20.0, y + 8.0]) > 40,
        "the ruled stroke printed on page {}",
        page0 + 1
    );
}

/// 入稿: Export All with トンボ and a proof sheet, and everything the
/// finished folder has to be true about.
fn export_stage(app: &mut App) {
    let dir = std::env::temp_dir().join(format!("mn-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("export dir");

    app.print_crop_marks = true;
    app.export_all_contact = true;
    app.export_all_prefix = "Storm".into();
    let findings = app.preflight_cached();
    for f in &findings {
        println!(
            "[note] preflight {:?}: {} — {}",
            f.level, f.check, f.message
        );
    }
    // The rough is blue and the work is mono, but a 下書き layer is not
    // printed — so it must not raise a colour warning. It used to, on
    // every page of every chapter.
    assert!(
        !findings.iter().any(|f| f.message.contains("\"name\"")),
        "preflight complained about the draft layer: {findings:?}"
    );
    dispatch(app, AppCmd::ExportAllPagesPath(dir.clone()));
    if let Some((_, findings)) = app.export_preflight.clone() {
        panic!("the export parked on preflight: {findings:?}");
    }
    println!("[note] export status: {}", app.status);

    let mut wrote: Vec<String> = std::fs::read_dir(&dir)
        .expect("the folder")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    wrote.sort();
    println!("[note] Export All wrote: {wrote:?}");
    assert_eq!(
        wrote,
        vec![
            "Storm-contact.png",
            "Storm-p001.png",
            "Storm-p002.png",
            "Storm-p003.png",
            "Storm-p004.png",
        ],
        "four pages in page order, plus the proof sheet"
    );

    // --- 08 the exported page 1 -------------------------------------
    let p1 = image::open(dir.join("Storm-p001.png"))
        .expect("page 1 decodes")
        .to_rgba8();
    keep(&p1, "08-export-page1");
    assert_eq!(rough_px(&p1), 0, "the rough is not in the shipped page");
    // The tone is still a SCREEN in the file, not a flat slab.
    let (w, h) = p1.dimensions();
    let band = [
        w as f32 * 0.25,
        h as f32 * 0.52,
        w as f32 * 0.75,
        h as f32 * 0.62,
    ];
    let (on, off) = dot_census(&p1, band);
    println!("[note] exported tone band: {on} inked / {off} clear");
    assert!(on > 0 && off > 0, "the exported tone is dots ({on}/{off})");

    // --- 09 トンボ ---------------------------------------------------
    let p3 = image::open(dir.join("Storm-p003.png"))
        .expect("page 3 decodes")
        .to_rgba8();
    let setup = app.page.clone().expect("a page setup");
    let trim = setup.trim_rect_px();
    let mut marks = 0u32;
    for (x, y, px) in p3.enumerate_pixels() {
        if px.0 != [0, 0, 0, 255] {
            continue;
        }
        let inside_trim = (x as f32) >= trim[0]
            && (x as f32) < trim[2]
            && (y as f32) >= trim[1]
            && (y as f32) < trim[3];
        if !inside_trim {
            marks += 1;
        }
    }
    println!("[note] flat-black pixels outside the trim on page 3: {marks}");
    assert!(marks > 100, "the register marks reached the file ({marks})");
    // Keep the top-left corner crop, where a corner mark lives.
    let crop = image::RgbaImage::from_fn(220, 220, |x, y| *p3.get_pixel(x, y));
    keep(&crop, "09-export-marks");

    // --- 10 the contact sheet, right to left -------------------------
    let sheet = image::open(dir.join("Storm-contact.png"))
        .expect("the sheet")
        .to_rgba8();
    keep(&sheet, "10-contact-sheet");
    let cell = mn_core::export::contact_cell(&image::RgbaImage::new(w, h), 400);
    assert_eq!(
        sheet.dimensions(),
        (4 * cell.width() + 5 * 12, cell.height() + 2 * 12),
        "four across, one row"
    );
    let col_ink = |c: u32| -> u32 {
        let x0 = 12 + c * (cell.width() + 12);
        let mut n = 0;
        for y in 12..12 + cell.height() {
            for x in x0..x0 + cell.width() {
                if inked(&sheet, x, y) {
                    n += 1;
                }
            }
        }
        n
    };
    let ink: Vec<u32> = (0..4).map(col_ink).collect();
    println!("[note] contact sheet ink per column (left to right): {ink:?}");
    // Page 1 is far and away the busiest page (three panels, tone, beta,
    // two bubbles); page 4 is one panel and one line. In a right-bound
    // work page 1 is the RIGHTMOST cell and page 4 the leftmost.
    assert!(
        ink[3] > ink[0] * 2,
        "page 1 sits at the RIGHT of the sheet, page 4 at the left: {ink:?}"
    );
    assert!(
        ink[3] > ink[2] && ink[2] >= ink[1],
        "and the pages run 1,2,3,4 leftwards: {ink:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
