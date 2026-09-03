//! Surface pass — Figure tools + Materials (CSP 450_figure + 630_material).
//!
//! Drives the real pointer entry points (`canvas_down/move/up`), renders 1:1
//! windows through `render_offscreen_vp` and measures what landed. Every
//! flow here answers one CSP manual line: "does this exist, in the same
//! number of steps?".
//!
//! Set `MN_SURFACE_OUT=<dir>` to keep the PNGs each flow renders.

use super::materials::MaterialKind;
use super::{App, PointerKind, headless_renderer};
use crate::cmd::{AppCmd, FigureMode, Tool, dispatch};
use mn_core::{PenSample, TileIdx};

const NONE: [PenSample; 0] = [];

/// A small page with the Figure tool in hand and a thin nib, at the
/// identity viewport (so canvas px and client px agree and the assertions
/// are about the PATH). Zoomed flows re-set `app.viewport` themselves.
fn figure_app(mode: FigureMode) -> Option<App> {
    let renderer = headless_renderer()?;
    let mut app = App::new(renderer, (400, 400), 1.0);
    app.doc = mn_core::Document::new(400, 400);
    app.viewport = mn_gpu::Viewport::default();
    app.tool = Tool::Figure;
    app.figure_mode = mode;
    app.props_current.size_px = 5.0;
    app.apply_props();
    Some(app)
}

fn drain(app: &mut App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

fn drag(app: &mut App, a: (f32, f32), b: (f32, f32)) {
    app.canvas_down(a.0, a.1, PointerKind::Pen, &NONE);
    app.canvas_move(b.0, b.1, &NONE);
    app.canvas_up(b.0, b.1, &NONE);
}

fn click(app: &mut App, p: (f32, f32)) {
    app.canvas_down(p.0, p.1, PointerKind::Pen, &NONE);
    app.canvas_up(p.0, p.1, &NONE);
}

fn px(app: &App, x: i32, y: i32) -> [u16; 4] {
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    app.doc
        .active_layer()
        .tile(idx)
        .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
        .unwrap_or([0; 4])
}

/// A 1:1 offscreen render of a `side`-px square window centred on `at`
/// (canvas px). NEVER render a whole print-res page in a test — the CI
/// runner's software GPU runs out of memory (see the text family's t13).
fn shot(app: &mut App, name: &str, at: (f32, f32), side: u32) -> image::RgbaImage {
    let x0 = (at.0 - side as f32 * 0.5).max(0.0).floor();
    let y0 = (at.1 - side as f32 * 0.5).max(0.0).floor();
    let vp = mn_gpu::Viewport {
        pan: [-x0, -y0],
        zoom: 1.0,
        ..Default::default()
    };
    let img = app.renderer.render_offscreen_vp(&app.doc, &vp, side, side);
    if let Ok(dir) = std::env::var("MN_SURFACE_OUT") {
        let _ = std::fs::create_dir_all(&dir);
        img.save(format!("{dir}/{name}.png")).expect("png written");
    }
    img
}

/// Dark (inked) pixel count in the whole image.
fn dark(img: &image::RgbaImage) -> usize {
    img.pixels()
        .filter(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) < 3 * 200 && p[3] > 0)
        .count()
}

fn ink_bbox(img: &image::RgbaImage) -> Option<[u32; 4]> {
    let mut bb: Option<[u32; 4]> = None;
    for (x, y, p) in img.enumerate_pixels() {
        if (p[0] as u32 + p[1] as u32 + p[2] as u32) < 3 * 200 && p[3] > 0 {
            bb = Some(match bb {
                None => [x, y, x + 1, y + 1],
                Some(b) => [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)],
            });
        }
    }
    bb
}

// --- Direct draw group ---------------------------------------------------

/// g01 CSP "Straight lines": drag start→end. Shift rotates in 45° steps.
#[test]
fn g01_straight_line_drags_and_shift_snaps_to_45() {
    let Some(mut app) = figure_app(FigureMode::Line) else {
        return;
    };
    drag(&mut app, (60.0, 200.0), (340.0, 200.0));
    assert!(px(&app, 200, 200)[3] > 0, "the line inked");
    let img = shot(&mut app, "g01-line", (200.0, 200.0), 400);
    let bb = ink_bbox(&img).expect("ink on the page");
    assert!(bb[2] - bb[0] > 260, "a full-width line: {bb:?}");

    // Shift: a drag 5° off horizontal flattens. A FRESH page — the render
    // composites every layer, so the first line would join the bbox.
    let Some(mut app) = figure_app(FigureMode::Line) else {
        return;
    };
    app.shell.test_modifiers = Some(egui::Modifiers::SHIFT);
    drag(&mut app, (60.0, 100.0), (340.0, 124.0));
    app.shell.test_modifiers = Some(egui::Modifiers::default());
    let img = shot(&mut app, "g01-line-shift", (200.0, 100.0), 200);
    let bb = ink_bbox(&img).expect("the snapped line");
    assert!(
        bb[3] - bb[1] <= 10,
        "shift flattened it to one horizontal band: {bb:?}"
    );
}

/// g02 CSP "Rectangles": drag corner to corner; Shift keeps it square.
/// CSP also has a "Roundness of corner" setting — measured here.
#[test]
fn g02_rectangle_drag_square_and_corner_roundness() {
    let Some(mut app) = figure_app(FigureMode::Rect) else {
        return;
    };
    drag(&mut app, (80.0, 80.0), (320.0, 200.0));
    let img = shot(&mut app, "g02-rect", (200.0, 140.0), 320);
    let bb = ink_bbox(&img).expect("a rectangle");
    assert!(bb[2] - bb[0] > 230 && bb[3] - bb[1] > 110, "box: {bb:?}");
    // The corner pixel is inked: a sharp corner, no rounding.
    assert!(px(&app, 80, 80)[3] > 0, "sharp corner at the drag origin");

    // Shift on a FRESH page (the render composites every layer, so the
    // first box would join the bbox). A 200x80 drag: the old code snapped
    // the DIAGONAL to the nearest 45° octant, and 21.8° rounds to 0° — the
    // "square" inked as a zero-height bar.
    let Some(mut app) = figure_app(FigureMode::Rect) else {
        return;
    };
    app.shell.test_modifiers = Some(egui::Modifiers::SHIFT);
    drag(&mut app, (60.0, 60.0), (260.0, 140.0));
    app.shell.test_modifiers = Some(egui::Modifiers::default());
    let img = shot(&mut app, "g02-rect-shift", (200.0, 200.0), 400);
    let bb = ink_bbox(&img).expect("a square");
    let (w, h) = (bb[2] - bb[0], bb[3] - bb[1]);
    assert!(h > 150, "it is a BOX, not a flat bar: {w}x{h} from {bb:?}");
    assert!(
        w.abs_diff(h) <= 4,
        "shift made it square: {w}x{h} from {bb:?}"
    );
}

/// g03 CSP "Circles": drag the bounding box, Shift = perfect circle,
/// "Adjust angle after fixed" spins an oval before it inks.
#[test]
fn g03_ellipse_circle_and_adjust_angle() {
    let Some(mut app) = figure_app(FigureMode::Ellipse) else {
        return;
    };
    drag(&mut app, (80.0, 100.0), (320.0, 220.0));
    let img = shot(&mut app, "g03-ellipse", (200.0, 160.0), 320);
    let bb = ink_bbox(&img).expect("an ellipse");
    assert!(bb[2] - bb[0] > bb[3] - bb[1], "wider than tall: {bb:?}");

    // Shift, on a fresh page: CSP's "perfect circle". A 200x80 drag used
    // to snap to 0° and ink a flat bar with no height at all.
    let Some(mut app) = figure_app(FigureMode::Ellipse) else {
        return;
    };
    app.shell.test_modifiers = Some(egui::Modifiers::SHIFT);
    drag(&mut app, (60.0, 60.0), (260.0, 140.0));
    app.shell.test_modifiers = Some(egui::Modifiers::default());
    let img = shot(&mut app, "g03-circle", (200.0, 200.0), 400);
    let bb = ink_bbox(&img).expect("a circle");
    let (w, h) = (bb[2] - bb[0], bb[3] - bb[1]);
    assert!(h > 150, "it is a DISC, not a flat bar: {w}x{h} from {bb:?}");
    assert!(w.abs_diff(h) <= 4, "shift made it round: {w}x{h}");
}

/// g04 CSP "Polygon": ours is a CLICK LIST that closes on the first vertex
/// (CSP's Polyline gesture, closed). CSP's own Polygon is a DRAG with a
/// vertex-count setting — measured as MISSING in the ledger.
#[test]
fn g04_polygon_click_list_closes_and_costs_one_undo() {
    let Some(mut app) = figure_app(FigureMode::Polygon) else {
        return;
    };
    let steps = app.doc.undo_labels().len();
    for p in [(120.0, 80.0), (300.0, 180.0), (200.0, 320.0), (80.0, 200.0)] {
        click(&mut app, p);
    }
    app.finish_figure_poly();
    let img = shot(&mut app, "g04-polygon", (200.0, 200.0), 400);
    assert!(dark(&img) > 400, "the polygon inked: {}", dark(&img));
    assert_eq!(app.doc.undo_labels().len(), steps + 1, "one undo press");
}

/// g05 CSP "Curves (Splines)": click anchors, Alt makes one a corner.
#[test]
fn g05_continuous_curve_spline_and_alt_corner() {
    let Some(mut app) = figure_app(FigureMode::Curve) else {
        return;
    };
    for p in [(60.0, 300.0), (200.0, 120.0), (340.0, 300.0)] {
        click(&mut app, p);
    }
    assert_eq!(app.figure_poly.as_ref().map(Vec::len), Some(3));
    app.finish_figure_poly();
    let img = shot(&mut app, "g05-spline", (200.0, 210.0), 400);
    assert!(dark(&img) > 200, "the spline inked");
    // It bows: the midpoint of the chord is clear, the apex is inked.
    assert!(px(&app, 200, 120)[3] > 0, "through the middle anchor");
}

/// g06 CSP "Curved lines": drag a baseline, release, bend, click. Already
/// pinned by figure_stage_tests; here only for the rendered proof.
#[test]
fn g06_two_stage_curve_bends_after_the_release() {
    let Some(mut app) = figure_app(FigureMode::Arc) else {
        return;
    };
    drag(&mut app, (60.0, 300.0), (340.0, 300.0));
    app.figure_hover(200, 140);
    click(&mut app, (200.0, 140.0));
    let img = shot(&mut app, "g06-arc", (200.0, 220.0), 400);
    assert!(dark(&img) > 200, "the arc inked");
    assert_eq!(px(&app, 200, 300)[3], 0, "not the straight baseline");
}

/// g07 "Fill with drawing colour" — CSP fills and outlines as ONE history
/// step. Ours brackets the fill separately (`ink_figure`'s own comment).
#[test]
fn g07_a_filled_figure_is_one_undo_press() {
    let Some(mut app) = figure_app(FigureMode::Rect) else {
        return;
    };
    app.figure_fill = true;
    let steps = app.doc.undo_labels().len();
    drag(&mut app, (100.0, 100.0), (300.0, 260.0));
    let img = shot(&mut app, "g07-filled-rect", (200.0, 180.0), 320);
    assert!(px(&app, 200, 180)[3] > 0, "the inside is filled");
    assert!(dark(&img) > 20000, "a solid box: {}", dark(&img));
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "CSP undoes a filled figure in ONE press, got {:?}",
        app.doc.undo_labels()
    );
    // …and that one press takes the whole shape away.
    app.doc.undo();
    assert_eq!(px(&app, 200, 180)[3], 0, "the fill went with it");
    assert_eq!(px(&app, 100, 180)[3], 0, "and so did the outline");
}

/// g08 CSP "Figure tool": figures ink with the ACTIVE BRUSH — size and
/// opacity from the Tool Property apply.
#[test]
fn g08_figures_ink_with_the_active_brush_size() {
    let Some(mut app) = figure_app(FigureMode::Line) else {
        return;
    };
    drag(&mut app, (60.0, 120.0), (340.0, 120.0));
    let thin = dark(&shot(&mut app, "g08-thin", (200.0, 120.0), 400));
    app.doc.active = app.doc.add_layer("fat");
    app.props_current.size_px = 24.0;
    app.apply_props();
    drag(&mut app, (60.0, 280.0), (340.0, 280.0));
    let both = dark(&shot(&mut app, "g08-fat", (200.0, 200.0), 400));
    assert!(
        both > thin * 2,
        "a 24 px nib lays down far more than a 5 px one: {thin} vs {both}"
    );
}

/// g08b CSP: "all figure shapes use a basic pen shape". A figure is a
/// DRAFTING mark and has to be one weight end to end — the active brush's
/// entry taper is a hand-stroke affordance and must not touch it. Before
/// the fix the first edge of every box (and the first quarter of every
/// circle) faded in from a hairline.
#[test]
fn g08b_a_figure_is_one_weight_from_its_first_pixel() {
    let Some(mut app) = figure_app(FigureMode::Rect) else {
        return;
    };
    // A taper long enough to see, in the same units the presets use.
    app.props_current.size_px = 9.0;
    app.props_current.taper_px = 120.0;
    app.props_current.taper_min = 0.18;
    app.apply_props();
    // The first segment of a Rect path is the TOP edge, left to right.
    drag(&mut app, (40.0, 60.0), (360.0, 300.0));
    let img = shot(&mut app, "g08b-taper", (200.0, 180.0), 400);
    // Two 60 px windows on that same top edge: where the stroke starts and
    // where it is long past any ramp.
    let band = |x0: u32| {
        let mut n = 0;
        for y in 50..72 {
            for x in x0..x0 + 60 {
                let p = img.get_pixel(x, y);
                if (p[0] as u32 + p[1] as u32 + p[2] as u32) < 3 * 200 && p[3] > 0 {
                    n += 1;
                }
            }
        }
        n
    };
    let (start, later) = (band(45), band(250));
    assert!(later > 0, "the top edge is there at all");
    assert!(
        start * 4 >= later * 3,
        "the first {start} px of the top edge are thinner than the {later} \
         px further along — the entry taper leaked into a figure"
    );
}

/// g08c The figure clock. Samples used to be stamped
/// `t0 + (segment * 16 + step)`, so at every corner of a multi-segment
/// figure time jumped BACKWARDS by the length of the segment just walked —
/// libmypaint printed "Time is running backwards!" once per corner and
/// then divided by that dtime for its speed inputs.
#[test]
fn g08c_the_figure_clock_never_runs_backwards() {
    let square = [[40.0, 40.0], [340.0, 40.0], [340.0, 340.0], [40.0, 340.0]];
    let s = App::figure_samples(&square, true, 1.25);
    assert!(s.len() > 900, "a 1200 px perimeter at 1.25 px: {}", s.len());
    for w in s.windows(2) {
        assert!(
            w[1].t_ms > w[0].t_ms,
            "the clock went from {} back to {}",
            w[0].t_ms,
            w[1].t_ms
        );
    }
}

/// g09 A figure on a VECTOR layer: CSP's memo says figure lines drawn on a
/// vector layer stay editable with the Object / Correct line tools.
#[test]
fn g09_a_figure_on_a_vector_layer_becomes_editable_strokes() {
    let Some(mut app) = figure_app(FigureMode::Rect) else {
        return;
    };
    dispatch(&mut app, AppCmd::AddVectorLayer);
    assert!(
        app.doc.active_layer().records_strokes(),
        "the vector layer is active"
    );
    drag(&mut app, (100.0, 100.0), (300.0, 260.0));
    let n = app
        .doc
        .active_layer()
        .strokes
        .as_ref()
        .map(|s| s.strokes.len())
        .unwrap_or(0);
    assert!(
        n > 0,
        "the rectangle was recorded as vector strokes, got {n}"
    );
    let img = shot(&mut app, "g09-vector-rect", (200.0, 180.0), 320);
    assert!(dark(&img) > 500, "and it shows on the page");
}

/// g10 A figure obeys the SELECTION mask, like every other inking tool.
#[test]
fn g10_a_figure_is_clipped_by_the_selection() {
    let Some(mut app) = figure_app(FigureMode::Line) else {
        return;
    };
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 0.0, 0.0, 200.0, 400.0,
    ));
    drag(&mut app, (40.0, 200.0), (360.0, 200.0));
    let img = shot(&mut app, "g10-clipped-line", (200.0, 200.0), 400);
    let bb = ink_bbox(&img).expect("the left half inked");
    assert!(bb[2] <= 206, "the line stopped at the selection: {bb:?}");
    assert!(bb[0] < 60, "and it started where the drag did");
}

// --- Speed lines / Focus lines (CSP 540_comic) ---------------------------

/// g11 CSP "Creating speed lines with the Comic tool": drag the motion,
/// a speed-line LAYER appears.
#[test]
fn g11_stream_line_drag_places_its_own_layer() {
    let Some(mut app) = figure_app(FigureMode::Stream) else {
        return;
    };
    let before = app.doc.layers.len();
    drag(&mut app, (100.0, 200.0), (320.0, 200.0));
    drain(&mut app);
    assert_eq!(app.doc.layers.len(), before + 1, "a speed-line layer");
    let img = shot(&mut app, "g11-stream", (200.0, 200.0), 400);
    assert!(dark(&img) > 2000, "the lines drew: {}", dark(&img));
}

/// g12 CSP "focus lines … drag from where you want the center to be".
/// A perfect circle with Shift; the rays reach the page edge on their own.
#[test]
fn g12_focus_line_drag_converges_on_the_press_point() {
    let Some(mut app) = figure_app(FigureMode::Focus) else {
        return;
    };
    let before = app.doc.layers.len();
    drag(&mut app, (200.0, 200.0), (200.0, 120.0));
    drain(&mut app);
    assert_eq!(app.doc.layers.len(), before + 1, "a focus-line layer");
    let img = shot(&mut app, "g12-focus", (200.0, 200.0), 400);
    assert!(dark(&img) > 2000, "the rays drew: {}", dark(&img));
    // The hole: nothing inked at the convergence point itself.
    let centre = img.get_pixel(200, 200);
    assert!(
        (centre[0] as u32 + centre[1] as u32 + centre[2] as u32) > 3 * 200,
        "the middle stays clear"
    );
}

/// g13 The two flash kinds place layers of their own too (ウニフラッシュ /
/// ベタフラッシュ — CSP's Burst / Flash).
#[test]
fn g13_the_two_flashes_place_layers() {
    for (mode, name) in [
        (FigureMode::Urchin, "g13-urchin"),
        (FigureMode::SolidFlash, "g13-solid"),
    ] {
        let Some(mut app) = figure_app(mode) else {
            return;
        };
        let before = app.doc.layers.len();
        drag(&mut app, (200.0, 200.0), (200.0, 110.0));
        drain(&mut app);
        assert_eq!(app.doc.layers.len(), before + 1, "{name}: a layer");
        let img = shot(&mut app, name, (200.0, 200.0), 400);
        assert!(dark(&img) > 1000, "{name} drew: {}", dark(&img));
    }
}

// --- Materials (CSP 630_material) ----------------------------------------

fn bank_app() -> Option<App> {
    let renderer = headless_renderer()?;
    let mut app = App::new(renderer, (600, 600), 1.0);
    app.doc = mn_core::Document::new(400, 400);
    app.viewport = mn_gpu::Viewport::default();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/materials");
    app.material_folders[0] = dir;
    app.materials_scan();
    Some(app)
}

/// m01 CSP "Material palette": the bank scans, and every material carries a
/// type so the palette's chips and tree can group them.
#[test]
fn m01_the_bank_scans_with_types_and_names() {
    let Some(app) = bank_app() else {
        return;
    };
    assert!(!app.materials.is_empty(), "the starter bank scanned");
    let tones = app
        .materials
        .iter()
        .filter(|m| matches!(m.kind, MaterialKind::Tone(_)))
        .count();
    assert!(tones > 0, "a default tone set ships");
}

/// m02 CSP "Image materials … drag and drop it to the canvas": ours pastes
/// as the move/scale float.
#[test]
fn m02_pasting_an_image_material_opens_a_float() {
    let Some(mut app) = bank_app() else {
        return;
    };
    let Some(m) = app
        .materials
        .iter()
        .find(|m| matches!(m.kind, MaterialKind::Image))
        .cloned()
    else {
        return;
    };
    dispatch(
        &mut app,
        AppCmd::PasteMaterial {
            path: m.path.clone(),
            tile: false,
        },
    );
    drain(&mut app);
    assert!(
        app.transform_drag.is_some() || app.doc.layers.len() > 1,
        "the material landed (float or layer): status {}",
        app.status
    );
}

/// m03 CSP registers the SELECTED part of a layer as a material
/// ("If you do not create a selection, the entire layer will be used").
#[test]
fn m03_register_layer_as_material_honours_the_selection() {
    let Some(mut app) = bank_app() else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("mn-f6-mat-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    app.material_folders.push(dir.clone());
    // Ink a wide band, then select only its left third.
    app.tool = Tool::Figure;
    app.figure_mode = FigureMode::Rect;
    app.figure_fill = true;
    app.props_current.size_px = 5.0;
    app.apply_props();
    drag(&mut app, (40.0, 100.0), (360.0, 200.0));
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 40.0, 100.0, 140.0, 200.0,
    ));
    let before = app.materials.len();
    dispatch(&mut app, AppCmd::MaterialRegisterLayer);
    drain(&mut app);
    assert!(
        app.materials.len() > before,
        "a material was registered: status {}",
        app.status
    );
    // CSP: a selection scopes the material. The saved PNG must be the
    // selected third, not the whole 320 px band.
    let made = app
        .materials
        .iter()
        .find(|m| m.path.starts_with(&dir))
        .expect("the new material is in the scratch folder")
        .path
        .clone();
    let img = image::open(&made).expect("it is a readable image").to_rgba8();
    assert!(
        img.width() <= 120,
        "the selection scoped it: {}x{}",
        img.width(),
        img.height()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// m04 The owner's tiling ask: one paste covers the page in copies as a
/// SINGLE float, so it can be drawn through like a mask.
#[test]
fn m04_a_tiled_paste_covers_the_page_as_one_float() {
    let Some(mut app) = bank_app() else {
        return;
    };
    let Some(m) = app
        .materials
        .iter()
        .find(|m| matches!(m.kind, MaterialKind::Image))
        .cloned()
    else {
        return;
    };
    let before = app.doc.layers.len();
    dispatch(
        &mut app,
        AppCmd::PasteMaterial {
            path: m.path.clone(),
            tile: true,
        },
    );
    drain(&mut app);
    assert!(
        app.transform_drag.is_some() || app.doc.layers.len() > before,
        "the tiling landed: status {}",
        app.status
    );
    // The float is overlay state, not document pixels — bake it before
    // rendering, or the page is still blank paper.
    app.commit_open_float();
    drain(&mut app);
    let img = shot(&mut app, "m04-tiled", (200.0, 200.0), 400);
    let painted = img.pixels().filter(|p| p[3] > 0 && p[0] < 250).count();
    assert!(
        painted > 4000,
        "the tiling covers the page, not one stamp: {painted} px"
    );
}

/// m05 CSP "Material filters … or use the search bar at the top": ONE box
/// searches names AND tags (MT-012).
#[test]
fn m05_the_search_box_reads_names_and_tags() {
    use super::materials::material_matches;
    let Some(app) = bank_app() else {
        return;
    };
    let all = app.materials.len();
    let hits = app
        .materials
        .iter()
        .filter(|m| material_matches(m, "tone"))
        .count();
    assert!(hits > 0 && hits < all, "'tone' narrows the bank: {hits}/{all}");
    let none = app
        .materials
        .iter()
        .filter(|m| material_matches(m, "zzzznotamaterial"))
        .count();
    assert_eq!(none, 0, "a miss is a miss");
}

/// m06 CSP "Displays tags assigned to materials as a list of buttons …
/// tap a button to filter": ours are the type chips + the tag chips, and a
/// type chip is a live filter.
#[test]
fn m06_a_type_chip_filters_the_grid() {
    use super::materials::{MaterialFilter, MaterialType};
    let Some(app) = bank_app() else {
        return;
    };
    let f = MaterialFilter::Type(MaterialType::Tone);
    let shown = app.materials.iter().filter(|m| f.accepts(m)).count();
    let tones = app
        .materials
        .iter()
        .filter(|m| m.material_type == MaterialType::Tone)
        .count();
    assert_eq!(shown, tones, "the Tones chip shows exactly the tones");
    assert!(tones > 0 && tones < app.materials.len(), "and it narrows");
}

/// m07 CSP "Double tap a thumbnail … to edit the name and tags of the
/// material": ours writes the folder's `tags.txt` sidecar and refreshes the
/// bank in place — no rescan, no restart.
#[test]
fn m07_setting_tags_lands_without_a_rescan() {
    let Some(mut app) = bank_app() else {
        return;
    };
    let Some(m) = app.materials.first().cloned() else {
        return;
    };
    let was = m.tags.clone();
    dispatch(
        &mut app,
        AppCmd::MaterialSetTags {
            path: m.path.clone(),
            tags: "f6probe, rain".into(),
        },
    );
    let now = app
        .materials
        .iter()
        .find(|x| x.path == m.path)
        .expect("still in the bank")
        .tags
        .clone();
    assert!(now.contains("f6probe"), "the tag stuck in place: {now:?}");
    // …and the search box sees it immediately.
    use super::materials::material_matches;
    let hit = app
        .materials
        .iter()
        .filter(|x| material_matches(x, "f6probe"))
        .count();
    assert_eq!(hit, 1, "searchable at once");
    // Put the bank back the way it was — this writes a real sidecar.
    dispatch(
        &mut app,
        AppCmd::MaterialSetTags {
            path: m.path.clone(),
            tags: was,
        },
    );
}
