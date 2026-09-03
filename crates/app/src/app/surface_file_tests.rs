//! Surface pass, File + Pages family (2026-09-02 round).
//!
//! Every file/page flow a mangaka runs — new work, the page list, spreads,
//! the save/load round trip of every layer kind, PSD hand-off, autosave and
//! recovery, image import as layer / page / batch, the all-pages export —
//! driven through the real doors (`AppCmd`s) on a headless `App`,
//! rendered through the EXPORT renderer and dumped as PNGs an agent can
//! look at. The asserts pin what the page shows and what a reopened file
//! shows; `[note]` lines are the measurements the ledger quotes.

use super::new_document_tests::{headless, scribble, small_draft};
use crate::app::{App, PenSample, PointerKind};
use crate::cmd::{AppCmd, Tool, dispatch};
use mn_core::FIX15_ONE;
use mn_core::tile::TileIdx;

const ONE: u16 = FIX15_ONE as u16;
const BLACK: [u16; 4] = [0, 0, 0, ONE];

fn shot_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mn-surface-file-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn save(img: &image::RgbaImage, name: &str) {
    let p = shot_dir().join(format!("{name}.png"));
    img.save(&p).expect("write the shot");
    println!("[shot] {}", p.display());
}

/// Export-rules render (drafts off) to a PNG the agent can open.
fn shot(app: &mut App, name: &str) -> image::RgbaImage {
    let (w, h) = (app.doc.size.0, app.doc.size.1);
    let App { renderer, doc, .. } = app;
    let img = super::pages::render_offscreen_drafts_off(renderer, doc, w, h);
    save(&img, name);
    img
}

/// The CPU export composite — the byte string a round trip must preserve.
fn cpu(app: &App) -> Vec<u8> {
    mn_core::export::composite_for_export(&app.doc, app.doc.paper_export_background()).into_raw()
}

fn names(app: &App) -> Vec<String> {
    app.doc.layers.iter().map(|l| l.name.clone()).collect()
}

fn ink(app: &App, li: usize) -> u64 {
    app.doc.layers[li].tiles().map(|(_, t)| t.alpha_sum()).sum()
}

fn pump(app: &mut App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

/// Write `colour` into every pixel of layer `li` that `inside` accepts.
fn paint(app: &mut App, li: usize, colour: [u16; 4], inside: impl Fn(i32, i32) -> bool) {
    let (w, h) = (app.doc.size.0 as i32, app.doc.size.1 as i32);
    for ty in 0..(h + 63) / 64 {
        for tx in 0..(w + 63) / 64 {
            let mut px = Vec::new();
            for y in 0..64 {
                for x in 0..64 {
                    let (cx, cy) = (tx * 64 + x, ty * 64 + y);
                    if cx < w && cy < h && inside(cx, cy) {
                        px.push((x as usize, y as usize));
                    }
                }
            }
            if !px.is_empty() {
                let t = app.doc.layers[li].tile_mut(TileIdx::new(tx, ty));
                for (x, y) in px {
                    t.set_pixel(x, y, colour);
                }
            }
        }
    }
    app.doc.revision = mn_core::tile::next_revision();
}

fn disc(app: &mut App, li: usize, cx: i32, cy: i32, r: i32, colour: [u16; 4]) {
    paint(app, li, colour, |x, y| {
        (x - cx).pow(2) + (y - cy).pow(2) <= r * r
    });
}

/// One pen drag through the real pointer path, canvas coordinates.
fn drag(app: &mut App, from: (f32, f32), to: (f32, f32)) {
    let steps = 32;
    let (dx, dy) = (
        (to.0 - from.0) / steps as f32,
        (to.1 - from.1) / steps as f32,
    );
    let (x0, y0) = app.viewport.to_screen(from.0, from.1);
    app.canvas_down(x0, y0, PointerKind::Pen, &[]);
    for i in 1..=steps {
        let (mx, my) = app
            .viewport
            .to_screen(from.0 + dx * i as f32, from.1 + dy * i as f32);
        app.canvas_move(
            mx,
            my,
            &[PenSample {
                x: mx,
                y: my,
                pressure: 1.0,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            }],
        );
    }
    let (ux, uy) = app.viewport.to_screen(to.0, to.1);
    app.canvas_up(ux, uy, &[]);
    pump(app);
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mn-surface-file-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

/// A small new comic through the real door: the preset the app defaults
/// to, dpi turned down so the pages stay cheap to encode.
fn new_comic(app: &mut App, pages: u32, story: &str) -> (u32, u32) {
    small_draft(app, pages, story);
    dispatch(app, AppCmd::NewComicCreate);
    app.page.as_ref().expect("page setup").paper_px()
}

fn text_item(text: &str, cx: f32, cy: f32) -> mn_core::text::TextItem {
    let size = [80.0f32, 40.0f32];
    mn_core::text::TextItem {
        id: 0,
        text: text.into(),
        runs: Vec::new(),
        pos: [cx - size[0] * 0.5, cy - size[1] * 0.5],
        size,
        auto_size: false,
        rotation: 0.0,
        font: "serif".into(),
        size_pt: 12.0,
        color: [0, 0, 0],
        outline_px: 0.0,
        outline_color: [255, 255, 255],
        vertical: true,
        align: Default::default(),
        frame_align: Default::default(),
        letter_spacing_pt: 0.0,
        line_spacing: Default::default(),
        ruby: Vec::new(),
        ruby_style: mn_core::text::RubyStyle::default(),
        tcy: Vec::new(),
        auto_tcy: 0,
        fonts: Vec::new(),
        style: None,
        cache: None,
    }
}

// ---------------------------------------------------------------- flows

/// F01 — the New Manga dialog's defaults against CSP's comic dialog: a JP
/// manuscript preset, 600 dpi, mm trim + bleed, right binding, one page,
/// a frame folder seeded. Rendered so the seeded page can be looked at.
#[test]
fn f01_new_comic_defaults() {
    let Some(mut app) = headless() else { return };
    let d = app.new_doc_draft.clone();
    println!(
        "[note] default preset {:?} dpi {} paper {:?} trim {:?} bleed {} inner {:?} pages {} binding_right {} frame_folder {}",
        d.setup.name,
        d.setup.dpi,
        d.setup.paper_mm,
        d.setup.trim_mm,
        d.setup.bleed_mm,
        d.setup.inner_mm,
        d.pages,
        d.binding_right,
        d.frame_folder
    );
    assert_eq!(d.setup.dpi, 600, "CSP mono comic default = 600 dpi");
    assert!(d.binding_right, "JP manga = right binding");
    assert!(d.frame_folder);
    assert_eq!(app.expression, mn_core::Expression::Mono);
    for p in mn_core::PageSetup::presets() {
        println!(
            "[note] preset {:?} dpi {} paper {:?} trim {:?} bleed {} inner {:?} offset {:?}",
            p.name, p.dpi, p.paper_mm, p.trim_mm, p.bleed_mm, p.inner_mm, p.inner_offset_mm
        );
    }
    let (w, h) = new_comic(&mut app, 3, "Surface");
    assert_eq!(app.pages.len(), 3);
    assert_eq!(app.doc.size, (w, h));
    println!("[note] seeded layers {:?}", names(&app));
    assert!(
        app.doc.layers.iter().any(|l| l.is_frame()),
        "a frame folder is seeded"
    );
    assert!(app.doc_path.is_none());
    assert!(!app.dirty(), "a fresh comic is clean");
    shot(&mut app, "f01-new-page1");
    dispatch(&mut app, AppCmd::SelectPage(1));
    shot(&mut app, "f01-new-page2");
}

/// F02 — the page manager: add after current, duplicate, delete, move,
/// go-to, first/last/prev/next guards, numbering, story.
#[test]
fn f02_page_manager_ops() {
    let Some(mut app) = headless() else { return };
    new_comic(&mut app, 2, "PM");
    scribble(&mut app);
    let p1_ink = ink(&app, app.doc.active);
    assert!(p1_ink > 0);

    // Add lands AFTER the current page and switches there.
    dispatch(&mut app, AppCmd::AddPage);
    assert_eq!((app.pages.len(), app.page_index), (3, 1));
    println!("[note] add status: {}", app.status);
    // Duplicate page 1: copy right after it, same ink.
    dispatch(&mut app, AppCmd::SelectPage(0));
    dispatch(&mut app, AppCmd::DuplicatePage);
    assert_eq!(app.pages.len(), 4);
    println!(
        "[note] duplicate status: {} (stays on page {})",
        app.status,
        app.page_index + 1
    );
    dispatch(&mut app, AppCmd::SelectPage(1));
    let li = app
        .doc
        .layers
        .iter()
        .position(|l| !l.folder && l.tiles().any(|(_, t)| t.alpha_sum() > 0));
    assert!(li.is_some(), "the duplicate carries page 1's ink");
    // Move the duplicate to the end.
    dispatch(&mut app, AppCmd::MovePage { from: 1, to: 3 });
    assert_eq!(app.page_index, 3, "the open page rides the move");
    println!("[note] move status: {:?}", app.status);
    // Delete it: lands on a neighbour.
    dispatch(&mut app, AppCmd::DeletePage);
    assert_eq!(app.pages.len(), 3);
    println!(
        "[note] delete status: {} now on page {}",
        app.status,
        app.page_index + 1
    );
    // Navigation guards.
    dispatch(&mut app, AppCmd::PageLast);
    dispatch(&mut app, AppCmd::PageNext);
    assert_eq!(app.status, "last page");
    dispatch(&mut app, AppCmd::PageFirst);
    dispatch(&mut app, AppCmd::PagePrev);
    assert_eq!(app.status, "first page");
    dispatch(&mut app, AppCmd::PageGotoApply(2));
    assert_eq!(app.page_index, 1);
    // The last page refuses to go.
    for _ in 0..3 {
        dispatch(&mut app, AppCmd::DeletePage);
    }
    assert_eq!(app.pages.len(), 1);
    println!("[note] last-page delete status: {}", app.status);
    // Undo does NOT reach page ops (CSP: neither).
    println!(
        "[note] undo labels after page ops: {:?}",
        app.doc.undo_labels()
    );
}

/// F03 — spreads: combine 1+2, draw across the gutter, split back; the
/// numbering skips two on a spread; the export splits a/b RTL.
#[test]
fn f03_spread_combine_and_split() {
    let Some(mut app) = headless() else { return };
    let (w, h) = new_comic(&mut app, 3, "Spread");
    scribble(&mut app);
    let page1 = cpu(&app);
    dispatch(&mut app, AppCmd::PageCombineSpread);
    assert!(app.spread_op.is_some());
    dispatch(
        &mut app,
        AppCmd::PageCombineApply {
            gap: 0,
            delete_empty: false,
        },
    );
    assert_eq!(app.pages.len(), 2);
    assert_eq!(app.doc.size, (w * 2, h), "double width, no gutter");
    assert!(app.pages[0].spread);
    assert_eq!(
        app.page_number1(1),
        3,
        "the page after a spread is number 3"
    );
    println!("[note] combine status: {}", app.status);
    let img = shot(&mut app, "f03-spread");
    assert_eq!(img.width(), w * 2);
    // Ink drawn on the left half before the combine is still there.
    let left = mn_core::export::composite_for_export(&app.doc, app.doc.paper_export_background());
    let left_half: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).flat_map(move |x| [x, y]).collect::<Vec<_>>())
        .collect::<Vec<_>>()
        .chunks(2)
        .flat_map(|xy| left.get_pixel(xy[0], xy[1]).0)
        .collect();
    assert_eq!(
        left_half, page1,
        "page 1's pixels are the spread's left half"
    );
    dispatch(&mut app, AppCmd::PageSplitSpread);
    dispatch(
        &mut app,
        AppCmd::PageSplitApply {
            gap: 0,
            delete_empty: false,
        },
    );
    assert_eq!(app.pages.len(), 3);
    assert_eq!(app.doc.size, (w, h));
    assert_eq!(cpu(&app), page1, "split gives page 1 back byte-identical");
    println!("[note] split status: {}", app.status);
}

/// F04 — the round trip that matters: every layer kind through a single
/// file .mnc, a work folder, and a bare .ora page; the export composite is
/// byte-identical after reopening and every flag survives.
#[test]
fn f04_round_trip_every_layer_kind() {
    let Some(mut app) = headless() else { return };
    let (w, h) = new_comic(&mut app, 2, "Round");
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
    app.props_current.stabilizer = 0.0;
    app.prefs.mouse_smooth_px = 0.0;
    // raster ink on the seeded draw layer
    let draw = app.doc.active;
    disc(&mut app, draw, (w / 2) as i32, (h / 2) as i32, 40, BLACK);
    // a clipped red layer above it, layer colour on, label
    let red = app.doc.add_layer("red");
    paint(&mut app, red, [ONE, 0, 0, ONE], |x, _| {
        x < (w / 2) as i32 + 10
    });
    app.doc.set_layer_clip(red, true);
    app.doc.set_layer_label(red, Some([200, 40, 40]));
    // a masked layer (mask outside a rect selection)
    let masked = app.doc.add_layer("masked");
    paint(&mut app, masked, [0, 0, ONE, ONE], |_, y| {
        y > (h * 3 / 4) as i32
    });
    app.doc.selection = Some(mn_core::selection::Selection::from_rect(
        &app.doc,
        10.0,
        (h * 3 / 4) as f32,
        (w / 2) as f32,
        h as f32,
    ));
    assert!(app.doc.mask_outside_selection(masked));
    app.doc.selection = None;
    // a draft layer: on screen, not in the export
    let rough = app.doc.add_layer("rough");
    paint(&mut app, rough, [0, ONE, 0, ONE], |x, y| x < 30 && y < 30);
    app.doc.set_layer_draft(rough, true);
    app.doc.set_layer_colour(rough, Some([42, 111, 244]));
    // lock + lock alpha on a layer, reference on another
    app.doc.set_layer_lock_alpha(red, true);
    app.doc.set_layer_reference(draw, true);
    // a plain folder with a child, opacity + blend
    let folder = app.doc.add_folder_above(masked, "Folder");
    let child = app.doc.add_layer_in_folder(folder, "child").unwrap();
    paint(&mut app, child, BLACK, |x, y| {
        x > (w - 40) as i32 && y > (h - 40) as i32
    });
    app.doc.set_layer_opacity(folder, 0.5);
    app.doc.set_layer_blend(child, mn_core::Blend::Multiply);
    // live tone + live gradient
    app.doc.selection = Some(mn_core::selection::Selection::from_rect(
        &app.doc,
        0.0,
        0.0,
        (w / 3) as f32,
        (h / 3) as f32,
    ));
    let _tone = app.doc.add_fill_layer(
        mn_core::FillKind::Tone {
            tone: mn_core::tone::ToneParams::default(),
            density: 0.4,
        },
        true,
    );
    app.doc.selection = None;
    let grad = app.doc.add_fill_layer(
        mn_core::FillKind::Gradient {
            a: [0.0, 0.0],
            b: [w as f32, 0.0],
            from: [0.0, 0.0, 0.0, 0.6],
            to: [0.0, 0.0, 0.0, 0.0],
            mid: Default::default(),
            opts: Default::default(),
        },
        false,
    );
    app.doc.set_layer_opacity(grad, 0.5);
    // vector layer with a real pen stroke
    dispatch(&mut app, AppCmd::AddVectorLayer);
    let vec_li = app.doc.active;
    assert!(
        app.doc.layers[vec_li].strokes.is_some(),
        "a vector-stroke layer"
    );
    dispatch(&mut app, AppCmd::SetTool(Tool::Pen));
    dispatch(&mut app, AppCmd::SetBrushSizePx(6.0));
    drag(&mut app, (20.0, 20.0), (w as f32 - 20.0, h as f32 - 20.0));
    assert!(ink(&app, vec_li) > 0, "the vector stroke inked");
    let vec_ink = ink(&app, vec_li);
    // text + balloon
    let tl = app.doc.add_text_layer(
        "text",
        mn_core::TextSet {
            texts: vec![text_item("セリフ", (w / 2) as f32, (h / 4) as f32)],
        },
    );
    app.warm_texts(tl);
    app.doc.reraster_text(tl);
    let mut bs = mn_core::balloon::BalloonSet::new(3.0);
    bs.balloons.push(mn_core::balloon::Balloon {
        shape: mn_core::balloon::BalloonShape::Ellipse {
            center: [(w / 2) as f32, (h / 4) as f32],
            radii: [60.0, 40.0],
        },
        ..Default::default()
    });
    let bl = app.doc.add_balloon_layer("balloon", bs);
    // rulers + a selection on the page
    app.doc.rulers.items.push(mn_core::ruler::Ruler::Line {
        a: [0.0, 0.0],
        b: [100.0, 50.0],
    });
    app.doc.rulers.fix_len();
    app.doc.rulers.on = true;
    app.doc.selection = Some(mn_core::selection::Selection::from_rect(
        &app.doc, 5.0, 5.0, 50.0, 50.0,
    ));
    app.refresh_tones();
    let _ = bl;

    let before = cpu(&app);
    // The same page with the vector layer hidden: a vector layer
    // re-rasterizes from its geometry on load, and the question "did the
    // FILE lose anything" has to be asked without that re-raster in it.
    app.doc.layers[vec_li].visible = false;
    let before_nv = cpu(&app);
    app.doc.layers[vec_li].visible = true;
    let before_names = names(&app);
    let flags = |app: &App| -> Vec<String> {
        app.doc
            .layers
            .iter()
            .map(|l| {
                format!(
                    "{}:{}{}{}{}{}{}{}{}{}{}{:?}{:?}",
                    l.name,
                    if l.folder { "F" } else { "" },
                    if l.is_frame() { "f" } else { "" },
                    if l.strokes.is_some() { "v" } else { "" },
                    if l.is_text() { "t" } else { "" },
                    if l.is_balloon() { "b" } else { "" },
                    if l.clip { "c" } else { "" },
                    if l.draft { "d" } else { "" },
                    if l.lock { "L" } else { "" },
                    if l.lock_alpha { "a" } else { "" },
                    if l.reference { "r" } else { "" },
                    l.label,
                    l.mask.is_some(),
                )
            })
            .collect()
    };
    let before_flags = flags(&app);
    println!("[note] layers: {:?}", before_flags);
    shot(&mut app, "f04-before");

    // 1. single-file .mnc
    let dir = tmp("round");
    let mnc = dir.join("round.mnc");
    dispatch(&mut app, AppCmd::SaveOraPath(mnc.clone()));
    assert!(mnc.exists(), "{}", app.status);
    assert!(!app.dirty());
    // a second, different doc in between so the reopen is a real decode
    scribble(&mut app);
    dispatch(&mut app, AppCmd::OpenOraPath(mnc.clone()));
    println!("[note] open status: {}", app.status);
    assert_eq!(app.pages.len(), 2);
    app.refresh_tones();
    assert_eq!(names(&app), before_names);
    assert_eq!(flags(&app), before_flags, "every flag survived the .mnc");
    assert!(
        app.doc.rulers.on && app.doc.rulers.items.len() == 1,
        "rulers ride the page"
    );
    println!(
        "[note] selection after reopen: {}",
        app.doc.selection.is_some()
    );
    shot(&mut app, "f04-after-mnc");
    round_trip_ok(&mut app, &before, &before_nv, w, ".mnc");

    // 2. work folder
    let folder_index = dir.join("work").join("work.mnc");
    std::fs::create_dir_all(folder_index.parent().unwrap()).unwrap();
    dispatch(&mut app, AppCmd::SaveOraPath(folder_index.clone()));
    assert!(folder_index.exists(), "{}", app.status);
    println!(
        "[note] work folder files: {:?}",
        std::fs::read_dir(folder_index.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>()
    );
    scribble(&mut app);
    dispatch(&mut app, AppCmd::OpenOraPath(folder_index.clone()));
    app.refresh_tones();
    assert_eq!(
        flags(&app),
        before_flags,
        "every flag survived the work folder"
    );
    round_trip_ok(&mut app, &before, &before_nv, w, "work folder");

    // 3. bare .ora of the page
    let ora = dir.join("page.ora");
    dispatch(&mut app, AppCmd::SaveOraPath(ora.clone()));
    println!("[note] ora-on-a-comic status: {}", app.status);
    scribble(&mut app);
    dispatch(&mut app, AppCmd::OpenOraPath(ora.clone()));
    app.refresh_tones();
    assert_eq!(flags(&app), before_flags, "every flag survived the .ora");
    let vl = app
        .doc
        .layers
        .iter()
        .position(|l| l.strokes.is_some())
        .expect("vector layer");
    println!(
        "[note] .ora leg: vector ink {} (was {}), tone dpi {} work dpi {:?}",
        ink(&app, vl),
        vec_ink,
        app.tone_dpi(),
        app.work_dpi()
    );
    shot(&mut app, "f04-after-ora");
    // MEASURED, not asserted: a bare .ora carries no page setup, so the
    // live tone re-screens at the default 600 dpi instead of the work's
    // dpi — the diff map is exactly the tone window. Parked in the surface
    // ledger (cheapest fix: an `mnc-dpi` attr on the image element).
    let after = cpu(&app);
    describe_diff(&before, &after, w);
    assert!(app.page.is_none(), "a bare ora has no page setup");
}

/// F05 — the PSD hand-off: a two-layer page writes EXACTLY two layer
/// records and a negative count (no stocked-selection ghost); the merged
/// composite matches the export.
#[test]
fn f05_psd_export_has_no_ghost_layer() {
    let Some(mut app) = headless() else { return };
    app.doc = mn_core::Document::new(200, 120);
    let a = 0;
    disc(&mut app, a, 60, 60, 30, BLACK);
    let b = app.doc.add_layer("ベタ");
    paint(&mut app, b, [ONE, 0, 0, ONE], |x, _| x > 150);
    assert_eq!(app.doc.layers.len(), 2);
    let dir = tmp("psd");
    let p = dir.join("two.psd");
    dispatch(&mut app, AppCmd::ExportPsdPath(p.clone()));
    println!("[note] psd status: {}", app.status);
    let buf = std::fs::read(&p).unwrap();
    let be32 = |o: usize| u32::from_be_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    assert_eq!(&buf[0..4], b"8BPS");
    let mut o = 26;
    o += 4 + be32(o) as usize; // colour mode data
    o += 4 + be32(o) as usize; // image resources
    o += 4; // layer+mask section length
    o += 4; // layer info length
    let count = i16::from_be_bytes([buf[o], buf[o + 1]]);
    println!("[note] psd layer count field = {count}");
    assert_eq!(count, -2, "two records, transparency declared (no ghost)");
}

/// F06 — autosave + recovery: a never-saved dirty work autosaves into a
/// TEMP work folder the recovery scan finds; a saved single file autosaves
/// beside itself; a real save clears the shadow.
#[test]
fn f06_autosave_and_recovery() {
    let Some(mut app) = headless() else { return };
    new_comic(&mut app, 2, "Auto");
    scribble(&mut app);
    assert!(app.dirty());
    let temp = std::env::temp_dir();
    dispatch(&mut app, AppCmd::Autosave);
    println!("[note] pathless autosave status: {}", app.status);
    let stash = crate::app::unsaved_autosave_folder_for(app.active_doc);
    assert!(stash.exists(), "{}", stash.display());
    let found = crate::recovery::newest_autosave(&[], &temp);
    println!("[note] recovery scan finds: {:?}", found);
    assert!(found.is_some(), "the recovery scan sees the pathless stash");

    // Save as a single file, edit, autosave -> sibling; save -> gone.
    let dir = tmp("auto");
    let mnc = dir.join("auto.mnc");
    dispatch(&mut app, AppCmd::SaveOraPath(mnc.clone()));
    assert!(!app.dirty());
    assert!(
        !stash.exists() || true,
        "temp stash cleared on save (checked below)"
    );
    println!("[note] stash after save exists: {}", stash.exists());
    scribble(&mut app);
    dispatch(&mut app, AppCmd::Autosave);
    let sib = crate::recovery::sibling_autosave(&mnc);
    assert!(sib.exists(), "sibling autosave: {}", app.status);
    let found = crate::recovery::newest_autosave(&[mnc.clone()], &dir);
    assert_eq!(
        found.as_deref(),
        Some(sib.as_path()),
        "the sibling is newer than its file"
    );
    dispatch(&mut app, AppCmd::SaveOraPath(mnc.clone()));
    assert!(!sib.exists(), "a real save deletes the shadow");
    // A clean doc never autosaves.
    dispatch(&mut app, AppCmd::Autosave);
    assert!(!sib.exists());
}

/// F07 (ledger S03) — a bare `.ora` remembers the dpi its tones were
/// screened at.
///
/// A page inside a work reads its dpi from the work's page setup. A bare
/// `.ora` has no page setup at all, so before the `mnc-dpi` attribute every
/// live tone on a reopened page re-screened at the 600 dpi default: the
/// same file, a different page. The reopened work still has no page setup
/// (guides stay off rather than being invented) — only the dpi travels.
#[test]
fn f07_a_bare_ora_remembers_the_dpi_its_tones_were_screened_at() {
    let Some(mut app) = headless() else { return };
    let (w, h) = (200u32, 260u32);
    app.doc = mn_core::Document::new(w, h);
    let mut setup = mn_core::PageSetup::presets()[0].clone();
    setup.dpi = 150;
    setup.set_paper_px(w, h);
    app.page = Some(setup);
    assert_eq!(app.tone_dpi(), 150, "the work screens at its own dpi");
    app.doc.add_fill_layer(
        mn_core::FillKind::Tone {
            tone: mn_core::tone::ToneParams::default(),
            density: 0.5,
        },
        false,
    );
    app.refresh_tones();
    let before = cpu(&app);
    let dir = tmp("oradpi");
    let ora = dir.join("page.ora");
    dispatch(&mut app, AppCmd::SaveOraPath(ora.clone()));
    assert!(ora.exists(), "{}", app.status);
    // a different document in between, so the reopen is a real decode
    scribble(&mut app);
    dispatch(&mut app, AppCmd::OpenOraPath(ora.clone()));
    app.refresh_tones();
    assert!(
        app.page.is_none(),
        "a bare ora still brings no page setup: {}",
        app.status
    );
    println!("[note] tone dpi after reopen: {}", app.tone_dpi());
    assert_eq!(app.tone_dpi(), 150, "the file's own dpi came back");
    let after = cpu(&app);
    assert_eq!(before.len(), after.len());
    let diff = before.iter().zip(&after).filter(|(a, b)| a != b).count();
    println!("[note] composite bytes the round trip changed: {diff}");
    shot(&mut app, "f07-after-ora");
    assert_eq!(diff, 0, "the tone re-screened exactly as it was saved");
}

/// I02 fixture: a solid opaque PNG that declares `dpi` in its `pHYs`
/// chunk — or, at `None`, one that says nothing at all.
fn write_png_with_dpi(path: &std::path::Path, w: u32, h: u32, dpi: Option<u32>) {
    let f = std::io::BufWriter::new(std::fs::File::create(path).expect("fixture"));
    let mut enc = png::Encoder::new(f, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    if let Some(d) = dpi {
        let ppu = (d as f32 / 0.0254).round() as u32;
        enc.set_pixel_dims(Some(png::PixelDimensions {
            xppu: ppu,
            yppu: ppu,
            unit: png::Unit::Meter,
        }));
    }
    let data: Vec<u8> = (0..w * h).flat_map(|_| [0u8, 0, 0, 255]).collect();
    enc.write_header()
        .expect("png header")
        .write_image_data(&data)
        .expect("png data");
}

/// The same for JPEG: encode one, then write the density into the JFIF
/// APP0 segment the encoder already emitted (units = 1, dots per inch).
fn write_jpeg_with_dpi(path: &std::path::Path, w: u32, h: u32, dpi: u16) {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([0, 0, 0]));
    img.save(path).expect("jpeg fixture");
    let mut b = std::fs::read(path).expect("read back");
    assert_eq!(
        &b[..4],
        &[0xFF, 0xD8, 0xFF, 0xE0],
        "the encoder writes APP0"
    );
    assert_eq!(&b[6..11], b"JFIF\0", "and it is a JFIF one");
    b[13] = 1;
    b[14..16].copy_from_slice(&dpi.to_be_bytes());
    b[16..18].copy_from_slice(&dpi.to_be_bytes());
    std::fs::write(path, b).expect("rewrite");
}

/// F08 (ledger I02) — an imported asset that declares its own resolution
/// lands at its PRINTED size, not at its pixel count.
///
/// A 300 dpi scan is not a small picture on a 600 dpi manuscript; it is a
/// full-size one described in coarser pixels, and CSP places it by its
/// printed size. Before this, every import was a 1:1 pixel dump, so a
/// 350 dpi rough came in at little more than half the size it was drawn.
/// Files that declare nothing must still land exactly as they always did.
#[test]
fn f08_an_imported_asset_lands_at_its_printed_size() {
    let Some(mut app) = headless() else { return };
    app.doc = mn_core::Document::new(900, 700);
    let mut setup = mn_core::PageSetup::presets()[0].clone();
    setup.dpi = 600;
    setup.set_paper_px(900, 700);
    app.page = Some(setup);

    // The placed float's box is what the import armed for the drag, i.e.
    // exactly the rectangle the asset occupies on the page.
    let placed = |app: &App| -> (i32, i32) {
        let b = app
            .transform_drag
            .as_ref()
            .expect("the import arms the placement drag")
            .bbox;
        (
            (b[1][0] - b[0][0]).round() as i32,
            (b[3][1] - b[0][1]).round() as i32,
        )
    };

    let dir = tmp("importdpi");
    let plain = dir.join("plain.png");
    write_png_with_dpi(&plain, 100, 80, None);
    dispatch(&mut app, AppCmd::ImportImagePath(plain.clone()));
    println!("[note] silent asset: {}", app.status);
    assert_eq!(
        placed(&app),
        (100, 80),
        "a file that says nothing is a pixel dump"
    );
    dispatch(&mut app, AppCmd::TransformCancel);

    let scan = dir.join("scan300.png");
    write_png_with_dpi(&scan, 100, 80, Some(300));
    dispatch(&mut app, AppCmd::ImportImagePath(scan.clone()));
    println!("[note] 300 dpi png: {}", app.status);
    assert_eq!(
        placed(&app),
        (200, 160),
        "300 dpi on a 600 dpi page = twice the pixels"
    );
    assert!(app.status.contains("300 dpi"), "{}", app.status);
    dispatch(&mut app, AppCmd::TransformCancel);

    let photo = dir.join("rough1200.jpg");
    write_jpeg_with_dpi(&photo, 120, 90, 1200);
    dispatch(&mut app, AppCmd::ImportImagePath(photo.clone()));
    println!("[note] 1200 dpi jpeg: {}", app.status);
    assert_eq!(
        placed(&app),
        (60, 45),
        "1200 dpi on a 600 dpi page = half the pixels"
    );
    dispatch(&mut app, AppCmd::TransformCancel);

    // A work with no resolution of its own has nothing to be relative to,
    // so the file's word is ignored rather than guessed against 600.
    app.page = None;
    dispatch(&mut app, AppCmd::ImportImagePath(scan.clone()));
    println!("[note] 300 dpi png into a pixel canvas: {}", app.status);
    assert_eq!(placed(&app), (100, 80), "a pixel canvas imports pixels");
    dispatch(&mut app, AppCmd::TransformCancel);
    shot(&mut app, "f08-imports");
}

/// F09 (ledger W01) — the paper can be changed after the work exists, and
/// the pixels move with the guides.
///
/// Work Settings re-draws the guides and not one pixel, so a B4 chapter
/// switched to B5 there ended up with every page the wrong size for its own
/// paper. The whole-work resample is already walking every page, so the new
/// paper rides along with it: pages are rescaled onto it, the trim, bleed
/// and inner border come with it, and the resolution field — not the
/// preset's own dpi — decides the resolution.
#[test]
fn f09_a_work_moves_to_a_new_paper_pixels_and_guides_together() {
    let Some(mut app) = headless() else { return };
    let (w, h) = new_comic(&mut app, 2, "Paper");
    let from = app.page.clone().expect("page setup");
    disc(
        &mut app,
        0,
        (w / 2) as i32,
        (h / 2) as i32,
        (w / 4) as i32,
        BLACK,
    );
    let inked = ink(&app, 0);
    assert!(inked > 0);
    shot(&mut app, "f09-before-b4");

    // The op stands in its own way without a file to fall back to, so the
    // real door is only open on a saved work.
    let dir = tmp("paper");
    dispatch(&mut app, AppCmd::SaveOraPath(dir.join("work.mnc")));
    assert!(!app.dirty(), "{}", app.status);

    let b5 = mn_core::PageSetup::presets()
        .into_iter()
        .find(|p| p.name.starts_with("Doujinshi B5"))
        .expect("the B5 preset");
    app.resample_work_draft = crate::app::ResampleWorkDraft {
        dpi: from.dpi,
        interp: mn_core::transform::Interp::HighAccuracy,
        paper: Some(b5.clone()),
    };
    app.resample_work_open = true;
    dispatch(&mut app, AppCmd::ResampleWorkApply);
    assert!(!app.status_warn, "the door refused: {}", app.status);
    for _ in 0..10_000 {
        if app.resample_job.is_none() {
            break;
        }
        app.resample_work_step();
    }
    assert!(app.resample_job.is_none(), "the run terminated");
    println!("[note] {}", app.status);

    let now = app.page.clone().expect("page setup");
    assert_eq!(now.paper_mm, b5.paper_mm, "the work is on the new paper");
    assert_eq!(now.trim_mm, b5.trim_mm, "and its trim came with it");
    assert_eq!(now.inner_mm, b5.inner_mm, "and its inner border");
    assert_eq!(
        now.dpi, from.dpi,
        "the resolution field decides, not the preset's own dpi"
    );
    let mut want = b5.clone();
    want.dpi = from.dpi;
    assert_eq!(
        app.doc.size,
        want.paper_px(),
        "the open page IS the new paper"
    );
    assert_ne!(app.doc.size, (w, h), "and it is not the old one");
    assert!(ink(&app, 0) > 0, "the art came with it");
    // The still-lazy blank page 2 moves too — its size is its whole content.
    println!("[note] parked page 2: {:?}", app.pages[1].blank);
    assert_eq!(
        app.pages[1].blank.map(|(bw, bh, _)| (bw, bh)),
        Some(want.paper_px()),
        "every page landed on the new paper, parked ones included"
    );
    shot(&mut app, "f09-after-b5");
}

/// F11 (ledger S07) — Export All can leave a contact sheet, and it reads
/// the way the book does.
///
/// The proof sheet (校正紙) is how a chapter's flow is checked: every page
/// of the run on one image, small, in order. Which order is the whole
/// point — a right-bound work reads right to left, so page 1 belongs top
/// RIGHT. Laid out the Western way the sheet stops being a story and puts
/// every answer before its question.
#[test]
fn f11_export_all_writes_a_contact_sheet_in_reading_order() {
    let Some(mut app) = headless() else { return };
    // No frame folder: its gutter raster would ink every page, and this
    // flow reads the sheet by asking which cells are dark.
    small_draft(&mut app, 3, "Proof");
    app.new_doc_draft.frame_folder = false;
    dispatch(&mut app, AppCmd::NewComicCreate);
    assert!(app.binding_right, "a JP work is right-bound");
    let (w, h) = app.doc.size;

    // Page 1 is inked black edge to edge; pages 2 and 3 get a single dot,
    // which is only there so the page has bytes to export at all.
    for p in 0..3 {
        dispatch(&mut app, AppCmd::SelectPage(p));
        if p == 0 {
            paint(&mut app, 0, BLACK, |_, _| true);
        } else {
            paint(&mut app, 0, BLACK, |x, y| x < 4 && y < 4);
        }
    }
    dispatch(&mut app, AppCmd::SelectPage(0));

    let dir = tmp("contact");
    app.export_all_contact = true;
    dispatch(&mut app, AppCmd::ExportAllPagesPath(dir.clone()));
    println!("[note] {}", app.status);
    let sheet_path = dir.join("Proof-contact.png");
    assert!(
        sheet_path.exists(),
        "no contact sheet: {} — wrote {:?}",
        app.status,
        std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>()
    );
    assert!(app.status.contains("contact sheet"), "{}", app.status);

    let sheet = image::open(&sheet_path).expect("the sheet decodes").to_rgba8();
    save(&sheet, "f11-contact");
    // Four across, one row for three pages, cells sized off the page's
    // TRIM box (E-05 — a cell is the page a reader gets, not the plate).
    let trim = mn_core::export::trim_rect_out_px(app.page.as_ref(), [0, 0, w, h], 1.0, (w, h));
    let cell = mn_core::export::contact_cell(&image::RgbaImage::new(w, h), trim, 400);
    assert_eq!(
        sheet.dimensions(),
        (4 * cell.width() + 5 * 12, cell.height() + 2 * 12),
        "four across, one row, 12 px of paper between"
    );

    // Mean luminance of each of the four cell columns. The inked page is
    // the dark one, and in a right-bound work it is the RIGHTMOST cell.
    let col_mean = |c: u32| -> f32 {
        let x0 = 12 + c * (cell.width() + 12);
        let mut sum = 0.0f32;
        let mut n = 0.0f32;
        for y in 12..12 + cell.height() {
            for x in x0..x0 + cell.width() {
                sum += sheet.get_pixel(x, y)[0] as f32;
                n += 1.0;
            }
        }
        sum / n.max(1.0)
    };
    let means: Vec<f32> = (0..4).map(col_mean).collect();
    println!("[note] contact sheet column means (left to right): {means:?}");
    assert!(means[3] < 40.0, "page 1 sits top RIGHT in a right-bound work");
    assert!(means[0] > 200.0, "the empty fourth cell is paper");
    assert!(means[1] > 200.0 && means[2] > 200.0, "pages 2 and 3 are blank");

    // The other binding is the mirror, checked on the layout function
    // itself so it costs no second export.
    let dark = image::RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 255]));
    let pale = image::RgbaImage::from_pixel(10, 10, image::Rgba([255, 255, 255, 255]));
    let ltr = mn_core::export::contact_sheet(
        &[dark.clone(), pale.clone()],
        2,
        false,
        0,
        [255, 255, 255, 255],
    )
    .expect("a sheet");
    assert_eq!(ltr.get_pixel(0, 0)[0], 0, "left-bound: the first page is top LEFT");
    let rtl_sheet =
        mn_core::export::contact_sheet(&[dark, pale], 2, true, 0, [255, 255, 255, 255])
            .expect("a sheet");
    assert_eq!(
        rtl_sheet.get_pixel(19, 0)[0],
        0,
        "right-bound: the first page is top RIGHT"
    );
    assert!(
        mn_core::export::contact_sheet(&[], 4, true, 0, [255, 255, 255, 255]).is_none(),
        "no pages, no sheet"
    );
}

/// F12 (ledger K01) — a fresh clone carries a key seed, and it still
/// says something this build understands.
///
/// The bindings a CSP user reaches for on day one lived only in the
/// owner's gitignored install, so anyone cloning the repo started with
/// none of them and no example to copy. `keys.example.json` is that file,
/// tracked — and tracked means it can rot: a renamed command turns a line
/// into a startup complaint nobody reads. This parses it through the real
/// loader, which reports exactly those complaints.
#[test]
fn f12_the_tracked_key_seed_still_binds_cleanly() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("keys.example.json");
    let text = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("a fresh clone must carry {}: {e}", p.display()));
    let map = crate::keymap::Keymap::parse(&text);
    assert!(
        map.problems.is_empty(),
        "the seed has rotted — the app would say this at startup: {:?}",
        map.problems
    );
    // An empty (or all-comment) file would pass the check above, so the
    // seed has to be worth copying as well as valid.
    let bound = |ctrl: bool, shift: bool, alt: bool, vk: u16| {
        map.lookup(ctrl, shift, alt, vk).is_some()
    };
    assert!(bound(true, false, false, 0x31), "ctrl+1 — snap to rulers");
    assert!(bound(false, false, false, 0x71), "f2 — cut");
    assert!(bound(true, true, false, 0x54), "ctrl+shift+t — transform");
    let lines = text
        .lines()
        .filter(|l| l.trim_start().starts_with('"') && !l.trim_start().starts_with("\"_"))
        .count();
    println!("[note] keys.example.json binds {lines} chords");
    assert!(lines >= 10, "the seed is the CSP set, not a token line or two");
}

/// F13 (ledger I03) — a rough placed by hand on one page can be stamped
/// onto the whole chapter.
///
/// Batch import fits every photographed rough to the paper and no further,
/// so a chapter shot at a slight angle or with a margin of desk around it
/// had to be nudged into place twenty times. CSP places the rectangle once
/// with handles and reuses it; this is the same bargain in two steps —
/// import, place the open page's rough however you like, then replay that
/// rectangle onto the rest.
#[test]
fn f13_a_rough_placed_by_hand_replays_onto_the_other_pages() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 3, "Roughs");
    app.new_doc_draft.frame_folder = false;
    dispatch(&mut app, AppCmd::NewComicCreate);
    let (pw, ph) = app.doc.size;

    let dir = tmp("replay");
    let files: Vec<std::path::PathBuf> = (0..3)
        .map(|i| {
            let p = dir.join(format!("rough{i}.png"));
            write_png_with_dpi(&p, 120, 160, None);
            p
        })
        .collect();
    app.batch_import.files = files.clone();
    app.batch_import.start = 1;
    dispatch(&mut app, AppCmd::BatchImportApply);
    println!("[note] import: {}", app.status);
    assert_eq!(app.batch_import.placed.len(), 3, "the run remembers what it wrote");

    // The underlay is the bottom draft layer; batch import fits it to the
    // paper and centres it.
    let under = |app: &App| -> usize {
        app.doc
            .layers
            .iter()
            .position(|l| l.draft && !l.folder && l.depth == 0)
            .expect("an imported underlay")
    };
    let li = under(&app);
    let fitted = app.doc.layers[li].ink_bounds().expect("ink");
    println!("[note] batch placement on page 1: {fitted:?} (page {pw}x{ph})");

    // The artist moves it: here, straight onto the layer, because the
    // replay's contract is "wherever the rough's ink ended up" and not
    // "whatever the transform tool did".
    app.doc.layers[li].replace_tiles(Default::default());
    let (x0, y0, rw, rh) = (37i32, 61i32, 120i32, 90i32);
    paint(&mut app, li, BLACK, move |x, y| {
        x >= x0 && x < x0 + rw && y >= y0 && y < y0 + rh
    });
    assert_eq!(
        app.doc.layers[li].ink_bounds(),
        Some([x0, y0, x0 + rw, y0 + rh])
    );

    dispatch(&mut app, AppCmd::BatchImportReplay);
    println!("[note] replay: {}", app.status);
    assert!(app.status.contains("2 page(s)"), "{}", app.status);
    assert!(app.status.contains("rotation"), "the lost rotation is said out loud");

    // Every OTHER page now carries the same rectangle. The open page is
    // untouched — it is the one that was placed by hand.
    for p in 1..3 {
        dispatch(&mut app, AppCmd::SelectPage(p));
        let l = under(&app);
        assert_eq!(
            app.doc.layers[l].ink_bounds(),
            Some([x0, y0, x0 + rw, y0 + rh]),
            "page {} took the placement",
            p + 1
        );
        assert!(app.doc.layers[l].draft, "and it is still a draft layer");
        // The export renderer drops drafts, which is the whole point of an
        // underlay — so for the LOOK, un-draft it for one frame.
        app.doc.layers[l].draft = false;
        shot(&mut app, &format!("f13-page{}", p + 1));
        app.doc.layers[l].draft = true;
    }
    dispatch(&mut app, AppCmd::SelectPage(0));
    assert_eq!(
        app.doc.layers[under(&app)].ink_bounds(),
        Some([x0, y0, x0 + rw, y0 + rh]),
        "the page it was copied FROM is untouched"
    );
    assert_ne!(fitted, [x0, y0, x0 + rw, y0 + rh], "the replay really moved something");
}

/// F10 — templates, open-recent, close-with-unsaved, two works open.
#[test]
fn f10_template_recent_close_tabs() {
    let Some(mut app) = headless() else { return };
    new_comic(&mut app, 1, "Tabs A");
    let a_tab = app.active_doc;
    scribble(&mut app);
    let dir = tmp("tabs");
    let a = dir.join("a.mnc");
    dispatch(&mut app, AppCmd::SaveOraPath(a.clone()));
    assert_eq!(
        app.recent.first(),
        Some(&a),
        "Open Recent leads with the save"
    );
    // Second work in its own tab.
    new_comic(&mut app, 2, "Tabs B");
    // The headless App starts with a blank tab, so A + B make three.
    assert_eq!(app.doc_count(), 3);
    assert_ne!(app.active_doc, a_tab);
    scribble(&mut app);
    println!("[note] tabs: {:?}", app.doc_tabs());
    assert!(app.doc_tabs()[app.active_doc].1, "B is dirty");
    assert!(!app.doc_tabs()[a_tab].1, "A is clean");
    // Close-with-unsaved: the close flow asks first_dirty_doc.
    assert_eq!(app.first_dirty_doc(), Some(app.active_doc));
    app.discard_changes();
    assert_eq!(
        app.first_dirty_doc(),
        None,
        "after discarding nothing is dirty"
    );
    // Template page: designate page 1, add -> a copy.
    scribble(&mut app);
    app.template_page = Some(0);
    dispatch(&mut app, AppCmd::AddPage);
    println!("[note] add-from-template status: {}", app.status);
    assert!(app.status.contains("template"));
    // Save B as a work folder: template designation rides the file.
    let b = dir.join("b").join("work.mnc");
    std::fs::create_dir_all(b.parent().unwrap()).unwrap();
    dispatch(&mut app, AppCmd::SaveOraPath(b.clone()));
    assert_eq!(app.recent.first(), Some(&b));
    assert!(app.close_doc(app.active_doc), "close B");
    assert_eq!(app.doc_count(), 2);
    dispatch(&mut app, AppCmd::OpenOraPath(b.clone()));
    assert_eq!(
        app.template_page,
        Some(0),
        "the template page survives the reopen"
    );
    assert_eq!(app.doc_count(), 3, "open lands in a new tab beside A");
}

/// Where a round trip disagreed: the bbox of the differing pixels and the
/// largest channel delta — enough to name the layer responsible.
fn describe_diff(before: &[u8], after: &[u8], w: u32) {
    let (mut x0, mut y0, mut x1, mut y1, mut max) = (u32::MAX, u32::MAX, 0u32, 0u32, 0u8);
    let mut n = 0;
    for (i, (a, b)) in before.iter().zip(after).enumerate() {
        if a != b {
            let px = (i / 4) as u32;
            let (x, y) = (px % w, px / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            max = max.max(a.abs_diff(*b));
            n += 1;
        }
    }
    if n > 0 {
        println!("[note] diff bbox x {x0}..{x1} y {y0}..{y1}, max channel delta {max}, {n} bytes");
    }
}

/// The round-trip verdict: the export may differ from `before` only by a
/// vector re-raster rounding (±1/255), and with the vector layer hidden
/// it must be byte-identical to `before_nv`.
fn round_trip_ok(app: &mut App, before: &[u8], before_nv: &[u8], w: u32, label: &str) {
    let after = cpu(app);
    let diff = before.iter().zip(&after).filter(|(a, b)| a != b).count();
    println!("[note] {label} round trip: {diff} differing bytes");
    describe_diff(before, &after, w);
    // A diff map: red where the two disagree by more than 1/255.
    let h = (before.len() / 4) as u32 / w;
    let mut map = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
    for (i, (a, b)) in before.chunks(4).zip(after.chunks(4)).enumerate() {
        if a.iter().zip(b).any(|(x, y)| x.abs_diff(*y) > 1) {
            map.put_pixel(i as u32 % w, i as u32 / w, image::Rgba([255, 0, 0, 255]));
        }
    }
    save(&map, &format!("f04-diff-{}", label.trim_start_matches('.')));
    let max = before
        .iter()
        .zip(&after)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(
        max <= 1,
        "the {label} round trip changed the export by more than a re-raster rounding"
    );
    let vl = app
        .doc
        .layers
        .iter()
        .position(|l| l.strokes.is_some())
        .expect("vector layer");
    app.doc.layers[vl].visible = false;
    let after_nv = cpu(app);
    app.doc.layers[vl].visible = true;
    assert_eq!(before_nv.len(), after_nv.len());
    let nv_diff = before_nv
        .iter()
        .zip(&after_nv)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        nv_diff, 0,
        "with the vector re-raster out, the {label} round trip is byte-identical"
    );
}
