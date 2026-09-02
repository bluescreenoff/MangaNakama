//! Surface pass, Layers palette family (2026-09-02 round).
//!
//! Every flow a mangaka runs in the Layers palette, driven through the
//! real doors (`AppCmd`s, pointer arms) on a headless `App`, rendered
//! through the EXPORT renderer and dumped as PNGs an agent can look at.
//! The asserts pin what the page shows; the `[note]` lines are the
//! measurements the ledger quotes (what the status line said, how many
//! undo steps a gesture cost) where a hard assert would be the wrong tool.

use super::new_document_tests::headless;
use crate::app::{App, PenSample, PointerKind};
use crate::cmd::{AppCmd, Tool, dispatch};
use mn_core::tile::TileIdx;
use mn_core::{Blend, FIX15_ONE};

const W: u32 = 256;
const H: u32 = 256;
const ONE: u16 = FIX15_ONE as u16;
const BLACK: [u16; 4] = [0, 0, 0, ONE];
const GREY: [u16; 4] = [ONE / 2, ONE / 2, ONE / 2, ONE];

fn page(app: &mut App) {
    app.doc = mn_core::Document::new(W, H);
    app.doc.layers[0].name = "lineart".into();
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
    app.props_current.stabilizer = 0.0;
    app.prefs.mouse_smooth_px = 0.0;
}

/// Write `colour` into every canvas pixel of layer `li` that `inside`
/// accepts (fix15 premultiplied). Direct writes: the flows under test are
/// the palette's, not the brush engine's.
fn paint(app: &mut App, li: usize, colour: [u16; 4], inside: impl Fn(i32, i32) -> bool) {
    for ty in 0..(H as i32 / 64) {
        for tx in 0..(W as i32 / 64) {
            let mut px = Vec::new();
            for y in 0..64 {
                for x in 0..64 {
                    if inside(tx * 64 + x, ty * 64 + y) {
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
    paint(app, li, colour, |x, y| (x - cx).pow(2) + (y - cy).pow(2) <= r * r);
}

fn rect(app: &mut App, li: usize, x0: i32, y0: i32, x1: i32, y1: i32, colour: [u16; 4]) {
    paint(app, li, colour, |x, y| x >= x0 && x < x1 && y >= y0 && y < y1);
}

fn shot_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mn-surface-layers-{}", std::process::id()));
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

/// What the SCREEN shows (drafts visible), same renderer.
fn screen_shot(app: &mut App, name: &str) -> image::RgbaImage {
    let (w, h) = (app.doc.size.0, app.doc.size.1);
    let img = app.renderer.render_offscreen(&app.doc, w, h);
    save(&img, name);
    img
}

fn luma(img: &image::RgbaImage, x: u32, y: u32) -> u8 {
    let p = img.get_pixel(x, y).0;
    let a = p[3] as f32 / 255.0;
    let mix = |c: u8| c as f32 * a + 255.0 * (1.0 - a);
    ((0.299 * mix(p[0]) + 0.587 * mix(p[1]) + 0.114 * mix(p[2])).round()) as u8
}

fn rgb(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = img.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

fn names(app: &App) -> Vec<String> {
    app.doc.layers.iter().map(|l| l.name.clone()).collect()
}

fn depths(app: &App) -> Vec<u8> {
    app.doc.layers.iter().map(|l| l.depth).collect()
}

fn ink(app: &App, li: usize) -> u64 {
    app.doc.layers[li].tiles().map(|(_, t)| t.alpha_sum()).sum()
}

fn cpu(app: &App) -> Vec<u8> {
    mn_core::export::composite(&app.doc, mn_core::Background::White).into_raw()
}

fn status(app: &App) -> String {
    app.status.clone()
}

fn pump(app: &mut App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

fn click(app: &mut App, cx: f32, cy: f32) {
    let (x, y) = app.viewport.to_screen(cx, cy);
    app.canvas_down(x, y, PointerKind::Mouse, &[]);
    app.canvas_up(x, y, &[]);
    pump(app);
}

/// One pen drag through the real pointer path, canvas coordinates.
fn drag(app: &mut App, from: (f32, f32), to: (f32, f32)) {
    let steps = 32;
    let (dx, dy) = ((to.0 - from.0) / steps as f32, (to.1 - from.1) / steps as f32);
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

fn pen(app: &mut App, size: f32, colour: [f32; 3]) {
    dispatch(app, AppCmd::SetTool(Tool::Pen));
    dispatch(app, AppCmd::SetBrushSizePx(size));
    dispatch(app, AppCmd::SetSlotColor(colour));
}

fn panel_text(app: &mut App) -> String {
    let ctx = egui::Context::default();
    let out = ctx.run_ui(egui::RawInput::default(), |ui| {
        crate::ui::layers::layer_property(ui, app);
    });
    fn walk(s: &egui::epaint::Shape, into: &mut String) {
        match s {
            egui::epaint::Shape::Text(t) => {
                into.push_str(t.galley.text());
                into.push('\n');
            }
            egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, into)),
            _ => {}
        }
    }
    let mut text = String::new();
    for c in &out.shapes {
        walk(&c.shape, &mut text);
    }
    out.drop_without_applying_deltas();
    text
}

// ---------------------------------------------------------------- flows

/// New raster / vector / folder / frame folder from the palette's doors,
/// then undo each — AND the recorded owner question: does a structural op
/// still wipe the undo stack? Measured, not ruled.
#[test]
fn s01_new_layers_of_every_type_then_undo() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    pen(&mut app, 6.0, [0.0, 0.0, 0.0]);
    drag(&mut app, (40.0, 40.0), (200.0, 40.0));
    let stroke_ink = ink(&app, 0);
    assert!(stroke_ink > 0, "the pen inked");
    let undo_before = app.doc.undo_len();
    println!("[note] undo steps after one stroke: {undo_before}");

    dispatch(&mut app, AppCmd::AddLayer);
    dispatch(&mut app, AppCmd::AddVectorLayer);
    dispatch(&mut app, AppCmd::AddFolder);
    dispatch(&mut app, AppCmd::NewFrameLayer);
    println!("[note] stack after 4 adds: {:?}", names(&app));
    println!("[note] undo labels: {:?}", app.doc.undo_labels());
    // A frame folder seeds its White + draw layer, so four adds = six rows.
    assert_eq!(app.doc.layers.len(), 7, "four adds (frame folder = 3 rows)");
    assert!(
        app.doc.undo_len() >= undo_before + 4,
        "structural ops are undo steps ({} → {})",
        undo_before,
        app.doc.undo_len()
    );
    for _ in 0..4 {
        dispatch(&mut app, AppCmd::Undo);
    }
    assert_eq!(app.doc.layers.len(), 1, "four undos put the stack back");
    assert_eq!(ink(&app, 0), stroke_ink, "the stroke survived the structure undos");
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(ink(&app, 0), 0, "the FIFTH undo takes the stroke — the stack was not wiped");
}

/// Duplicate, delete, merge down: the page composites the same across a
/// duplicate+delete pair and a merge, and every refusal SAYS something.
#[test]
fn s02_duplicate_delete_merge_down() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    disc(&mut app, 0, 128, 128, 60, BLACK);
    dispatch(&mut app, AppCmd::AddLayer);
    let li = app.doc.active;
    app.doc.layers[li].name = "shade".into();
    disc(&mut app, li, 100, 100, 30, GREY);
    let before = cpu(&app);

    dispatch(&mut app, AppCmd::DuplicateLayer);
    println!(
        "[note] after duplicate: {:?} active={} status={:?}",
        names(&app),
        app.doc.active,
        status(&app)
    );
    assert_eq!(app.doc.layers.len(), 3);
    assert_eq!(ink(&app, 2), ink(&app, 1), "the copy carries the ink");
    dispatch(&mut app, AppCmd::RemoveLayer);
    assert_eq!(app.doc.layers.len(), 2);
    assert_eq!(cpu(&app), before, "duplicate + delete is a no-op on the page");
    println!("[note] after delete: status={:?}", status(&app));

    dispatch(&mut app, AppCmd::SelectLayer(1));
    dispatch(&mut app, AppCmd::MergeDown);
    println!("[note] after merge down: {:?} status={:?}", names(&app), status(&app));
    assert_eq!(app.doc.layers.len(), 1, "merged");
    assert_eq!(cpu(&app), before, "merge down keeps the page");
    shot(&mut app, "s02-merged");
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers.len(), 2, "one undo un-merges");

    // Refusals: a LOCKED lower layer, the bottom layer. Each must leave a
    // status line, or Ctrl+E is a dead key.
    dispatch(&mut app, AppCmd::SelectLayer(1));
    dispatch(&mut app, AppCmd::SetLayerLock(0, true));
    app.status.clear();
    dispatch(&mut app, AppCmd::MergeDown);
    let locked_status = status(&app);
    println!(
        "[note] merge down onto a LOCKED layer: layers={} status={:?}",
        app.doc.layers.len(),
        locked_status
    );
    dispatch(&mut app, AppCmd::SetLayerLock(0, false));
    dispatch(&mut app, AppCmd::SelectLayer(0));
    app.status.clear();
    dispatch(&mut app, AppCmd::MergeDown);
    let bottom_status = status(&app);
    println!("[note] merge down on the BOTTOM layer: status={:?}", bottom_status);
    assert!(!locked_status.is_empty(), "a refused merge (locked) must say so");
    assert!(!bottom_status.is_empty(), "a refused merge (bottom) must say so");

    dispatch(&mut app, AppCmd::SelectLayer(1));
    dispatch(&mut app, AppCmd::RemoveLayer);
    app.status.clear();
    dispatch(&mut app, AppCmd::RemoveLayer);
    println!(
        "[note] delete the last layer: layers={} status={:?}",
        app.doc.layers.len(),
        status(&app)
    );
    assert_eq!(app.doc.layers.len(), 1);
    assert!(
        !status(&app).is_empty(),
        "deleting the last layer must say why nothing happened"
    );
}

/// Merge a FOLDER down (CSP: the folder flattens and merges onto the
/// layer below).
#[test]
fn s03_merge_folder_down() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    disc(&mut app, 0, 128, 128, 60, BLACK);
    dispatch(&mut app, AppCmd::AddFolder);
    let f = app.doc.active;
    assert!(app.doc.layers[f].folder);
    dispatch(&mut app, AppCmd::AddLayer);
    let a = app.doc.active;
    assert_eq!(app.doc.layers[a].depth, 1, "new layer lands INSIDE the open folder");
    disc(&mut app, a, 90, 90, 25, GREY);
    dispatch(&mut app, AppCmd::AddLayer);
    let b = app.doc.active;
    disc(&mut app, b, 170, 170, 25, GREY);
    println!("[note] stack: {:?} depths {:?}", names(&app), depths(&app));
    let before = cpu(&app);
    // The header's index moved up as children were inserted below it.
    let f = app.doc.layers.iter().position(|l| l.folder).unwrap();
    dispatch(&mut app, AppCmd::SelectLayer(f));
    app.status.clear();
    dispatch(&mut app, AppCmd::MergeDown);
    let merged_folder = app.doc.layers.len() == 1;
    println!(
        "[note] merge FOLDER down: works={merged_folder} layers={} status={:?}",
        app.doc.layers.len(),
        status(&app)
    );
    assert!(merged_folder, "a folder merges down (CSP: the group flattens onto the layer below)");
    let after = cpu(&app);
    let diff = before
        .iter()
        .zip(&after)
        .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 2)
        .count();
    println!("[note] merge folder down: pixels off by >2/255: {diff}");
    assert_eq!(diff, 0, "a merged folder keeps the page");
    assert_eq!(
        app.doc.undo_labels().last().map(String::as_str),
        Some("Merge down")
    );
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers.len(), 4, "one undo brings the folder back");
    assert_eq!(cpu(&app), before);
}

/// CSP's merge on a CLIPPED layer bakes what the clip SHOWED, not the
/// raw pixels — the everyday "collapse the shading into the flats".
#[test]
fn s16_merge_down_of_a_clipped_layer_bakes_what_it_showed() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    disc(&mut app, 0, 128, 128, 50, BLACK);
    dispatch(&mut app, AppCmd::AddLayer);
    let sh = app.doc.active;
    rect(&mut app, sh, 0, 0, 256, 256, [ONE / 2, 0, 0, ONE]);
    dispatch(&mut app, AppCmd::SetLayerClip(sh, true));
    let before = cpu(&app);
    dispatch(&mut app, AppCmd::MergeDown);
    println!("[note] clipped merge: layers={} status={:?}", app.doc.layers.len(), status(&app));
    assert_eq!(app.doc.layers.len(), 1, "merged");
    assert_eq!(cpu(&app), before, "the page is what the clip showed — no red flood");
    let img = shot(&mut app, "s16-clipped-merge");
    assert_eq!(rgb(&img, 20, 20), [255, 255, 255]);
    assert!(rgb(&img, 128, 128)[0] > 100);
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers.len(), 2);
    assert!(app.doc.layers[1].clip, "undo brings the clipped layer back, flag and all");
}

/// CSP Layer ▸ Layer order ▸ Move up / Move down as key-bindable
/// commands: a plain row hops one, a folder hops with its children, the
/// ends refuse with a word, and each hop is one undo.
#[test]
fn s18_move_up_and_down_from_the_keyboard() {
    use crate::cmd::ActiveLayerCmd::{MoveDown, MoveUp};
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::AddLayer);
    app.doc.layers[1].name = "A".into();
    dispatch(&mut app, AppCmd::AddFolder);
    dispatch(&mut app, AppCmd::AddLayer);
    app.doc.layers[app.doc.active].name = "child".into();
    let f = app.doc.layers.iter().position(|l| l.folder).unwrap();
    dispatch(&mut app, AppCmd::SelectLayer(f));
    dispatch(&mut app, AppCmd::ToggleFolderOpen(f));
    dispatch(&mut app, AppCmd::AddLayer);
    app.doc.layers[app.doc.active].name = "B".into();
    println!("[note] start: {:?} depths {:?}", names(&app), depths(&app));
    // ["lineart", "A", "child", "Folder 1", "B"]
    let f = app.doc.layers.iter().position(|l| l.folder).unwrap();
    dispatch(&mut app, AppCmd::SelectLayer(f));
    dispatch(&mut app, AppCmd::ActiveLayer(MoveUp));
    println!("[note] folder up: {:?} depths {:?} status={:?}", names(&app), depths(&app), status(&app));
    assert_eq!(names(&app), ["lineart", "A", "B", "child", "Folder 1"]);
    assert_eq!(depths(&app), [0, 0, 0, 1, 0], "the folder hopped over B with its child");
    app.status.clear();
    dispatch(&mut app, AppCmd::ActiveLayer(MoveUp));
    assert_eq!(names(&app), ["lineart", "A", "B", "child", "Folder 1"], "top stays put");
    assert!(!status(&app).is_empty(), "the refusal speaks");
    dispatch(&mut app, AppCmd::ActiveLayer(MoveDown));
    dispatch(&mut app, AppCmd::ActiveLayer(MoveDown));
    println!("[note] folder down ×2: {:?} depths {:?}", names(&app), depths(&app));
    assert_eq!(names(&app), ["lineart", "child", "Folder 1", "A", "B"]);
    let steps = app.doc.undo_len();
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.undo_len(), steps - 1, "one hop, one undo");
    assert_eq!(names(&app), ["lineart", "A", "child", "Folder 1", "B"]);
}

/// CSP Layer ▸ Flatten image.
#[test]
fn s17_flatten_image() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    disc(&mut app, 0, 128, 128, 50, BLACK);
    dispatch(&mut app, AppCmd::AddFolder);
    dispatch(&mut app, AppCmd::AddLayer);
    let a = app.doc.active;
    disc(&mut app, a, 60, 60, 20, GREY);
    dispatch(&mut app, AppCmd::AddLayer);
    let hidden = app.doc.active;
    disc(&mut app, hidden, 200, 200, 20, BLACK);
    dispatch(&mut app, AppCmd::SetLayerVisible(hidden, false));
    let before = cpu(&app);
    let n = app.doc.layers.len();
    dispatch(&mut app, AppCmd::FlattenImage);
    println!("[note] flatten: {:?} status={:?}", names(&app), status(&app));
    assert_eq!(app.doc.layers.len(), 1, "one layer");
    assert_eq!(cpu(&app), before, "the visible page is unchanged; the hidden disc is gone");
    shot(&mut app, "s17-flattened");
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers.len(), n, "one undo restores the stack");
}

/// Move up/down, drag into a folder and back out — the palette's drag is
/// `MoveLayer{from, slot, depth}`.
#[test]
fn s04_move_and_drag_into_folder() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::AddLayer);
    dispatch(&mut app, AppCmd::AddLayer);
    app.doc.layers[1].name = "A".into();
    app.doc.layers[2].name = "B".into();
    dispatch(&mut app, AppCmd::SelectLayer(0));
    dispatch(&mut app, AppCmd::AddFolder);
    println!("[note] stack: {:?} depths {:?}", names(&app), depths(&app));
    let f = app.doc.layers.iter().position(|l| l.folder).unwrap();
    let b = app.doc.layers.iter().position(|l| l.name == "B").unwrap();
    dispatch(&mut app, AppCmd::MoveLayer { from: b, slot: f, depth: 1 });
    println!(
        "[note] after drag B into folder: {:?} depths {:?}",
        names(&app),
        depths(&app)
    );
    let b = app.doc.layers.iter().position(|l| l.name == "B").unwrap();
    assert_eq!(app.doc.layers[b].depth, 1, "B is inside the folder");
    assert_eq!(
        app.doc.undo_labels().last().map(String::as_str),
        Some("Move layer")
    );
    let f = app.doc.layers.iter().position(|l| l.folder).unwrap();
    dispatch(&mut app, AppCmd::MoveLayer { from: b, slot: f + 1, depth: 0 });
    let b = app.doc.layers.iter().position(|l| l.name == "B").unwrap();
    assert_eq!(app.doc.layers[b].depth, 0, "B is loose again");
    println!("[note] after drag out: {:?} depths {:?}", names(&app), depths(&app));
    let a = app.doc.layers.iter().position(|l| l.name == "A").unwrap();
    assert!(app.doc.lower_layer(a), "A steps down one row");
    println!("[note] lower A: {:?}", names(&app));
    dispatch(&mut app, AppCmd::Undo);
    println!("[note] undo lower: {:?}", names(&app));
    assert_eq!(app.doc.layers.iter().position(|l| l.name == "A"), Some(a));
}

/// Lock (all) refuses the brush with a status; lock transparent pixels
/// keeps the silhouette while the colour changes.
#[test]
fn s05_lock_and_lock_alpha() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    disc(&mut app, 0, 128, 128, 50, BLACK);
    let before = ink(&app, 0);
    dispatch(&mut app, AppCmd::SetLayerLock(0, true));
    pen(&mut app, 10.0, [0.0, 0.0, 0.0]);
    app.status.clear();
    drag(&mut app, (20.0, 20.0), (240.0, 20.0));
    println!("[note] stroke on a locked layer: status={:?}", status(&app));
    assert_eq!(ink(&app, 0), before, "the lock held");
    assert!(status(&app).contains("lock"), "the refusal names the lock");
    dispatch(&mut app, AppCmd::SetLayerLock(0, false));

    dispatch(&mut app, AppCmd::SetLayerLockAlpha(0, true));
    pen(&mut app, 10.0, [1.0, 0.0, 0.0]);
    drag(&mut app, (20.0, 128.0), (240.0, 128.0));
    let img = shot(&mut app, "s05-lock-alpha");
    assert_eq!(ink(&app, 0), before, "alpha lock: coverage unchanged");
    let inside = rgb(&img, 128, 128);
    let outside = rgb(&img, 20, 128);
    println!("[note] alpha lock: inside {:?} outside {:?}", inside, outside);
    assert!(inside[0] > 150 && inside[1] < 80, "the disc went red inside");
    assert_eq!(outside, [255, 255, 255], "nothing landed outside the silhouette");
}

/// Visibility and the Alt+click solo — including a solo on a layer INSIDE
/// a folder, where hiding the parent would hide the soloed layer too.
#[test]
fn s06_visibility_and_solo_inside_a_folder() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    disc(&mut app, 0, 60, 60, 30, BLACK);
    dispatch(&mut app, AppCmd::AddFolder);
    dispatch(&mut app, AppCmd::AddLayer);
    let child = app.doc.active;
    assert_eq!(app.doc.layers[child].depth, 1);
    disc(&mut app, child, 190, 190, 30, BLACK);
    let f = app.doc.layers.iter().position(|l| l.folder).unwrap();
    dispatch(&mut app, AppCmd::SelectLayer(f));
    dispatch(&mut app, AppCmd::SetFolderThrough(f, false));
    // A sibling ABOVE the folder (select the header collapsed → sibling).
    dispatch(&mut app, AppCmd::ToggleFolderOpen(f));
    dispatch(&mut app, AppCmd::AddLayer);
    let sib = app.doc.active;
    disc(&mut app, sib, 190, 60, 30, BLACK);
    println!("[note] stack: {:?} depths {:?}", names(&app), depths(&app));

    dispatch(&mut app, AppCmd::SetLayerVisible(0, false));
    let img = shot(&mut app, "s06-hidden");
    assert_eq!(luma(&img, 60, 60), 255, "hidden lineart is gone");
    dispatch(&mut app, AppCmd::SetLayerVisible(0, true));

    dispatch(&mut app, AppCmd::SetLayerEyeSolo(child));
    let img = shot(&mut app, "s06-solo-child");
    let vis: Vec<bool> = app.doc.layers.iter().map(|l| l.visible).collect();
    println!("[note] solo child: visible={:?} status={:?}", vis, status(&app));
    println!(
        "[note] solo child: child luma {} sibling luma {} lineart luma {}",
        luma(&img, 190, 190),
        luma(&img, 190, 60),
        luma(&img, 60, 60)
    );
    assert_eq!(luma(&img, 60, 60), 255, "solo hid the lineart");
    assert_eq!(luma(&img, 190, 60), 255, "solo hid the sibling");
    assert!(
        luma(&img, 190, 190) < 40,
        "the SOLOED child is still visible — its folder must stay on"
    );
    dispatch(&mut app, AppCmd::SetLayerEyeSolo(child));
    let vis2: Vec<bool> = app.doc.layers.iter().map(|l| l.visible).collect();
    println!("[note] solo again: visible={:?} status={:?}", vis2, status(&app));
    assert!(vis2.iter().all(|&v| v), "second press restores");
}

/// Opacity and the two blend modes a mangaka uses (Multiply for shadow
/// over tone, Screen for highlights).
#[test]
fn s07_opacity_and_blend() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    rect(&mut app, 0, 0, 0, 256, 256, GREY);
    dispatch(&mut app, AppCmd::AddLayer);
    let li = app.doc.active;
    disc(&mut app, li, 128, 128, 60, GREY);
    let img = shot(&mut app, "s07-normal");
    println!("[note] normal: {}", luma(&img, 128, 128));
    dispatch(&mut app, AppCmd::SetLayerBlend(li, Blend::Multiply));
    let img = shot(&mut app, "s07-multiply");
    let m = luma(&img, 128, 128);
    println!("[note] multiply: {m}");
    assert!(m < 80 && m > 40, "grey × grey ≈ 25 % ({m})");
    dispatch(&mut app, AppCmd::SetLayerBlend(li, Blend::Screen));
    let img = shot(&mut app, "s07-screen");
    let s = luma(&img, 128, 128);
    println!("[note] screen: {s}");
    assert!(s > 180 && s < 205, "grey screen grey ≈ 75 % ({s})");
    dispatch(&mut app, AppCmd::SetLayerBlend(li, Blend::Normal));
    disc(&mut app, li, 128, 128, 60, BLACK);
    dispatch(&mut app, AppCmd::SetLayerOpacity(li, 0.5));
    let img = shot(&mut app, "s07-opacity50");
    let o = luma(&img, 128, 128);
    println!("[note] black at 50 % over grey: {o}");
    assert!(o > 50 && o < 80, "half of grey ({o})");
    println!(
        "[note] undo labels after opacity+blend: {:?}",
        app.doc.undo_labels()
    );
}

/// Clipping: to the layer below, to a folder, and CLIPPING-SCENARIOS 3c —
/// a clipped member dragged elsewhere re-clips to whatever is below it.
#[test]
fn s08_clipping() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    disc(&mut app, 0, 128, 128, 50, BLACK);
    dispatch(&mut app, AppCmd::AddLayer);
    let sh = app.doc.active;
    app.doc.layers[sh].name = "shade".into();
    rect(&mut app, sh, 0, 0, 256, 256, [ONE / 2, 0, 0, ONE]);
    dispatch(&mut app, AppCmd::SetLayerClip(sh, true));
    let img = shot(&mut app, "s08-clip");
    println!(
        "[note] clip: status={:?} inside {:?} outside {:?}",
        status(&app),
        rgb(&img, 128, 128),
        rgb(&img, 20, 20)
    );
    assert_eq!(rgb(&img, 20, 20), [255, 255, 255], "clipped red shows only over the base");
    assert!(rgb(&img, 128, 128)[0] > 100, "red over the disc");
    println!(
        "[note] clip edge: r+2 {:?} r-2 {:?}",
        rgb(&img, 128 + 52, 128),
        rgb(&img, 128 + 48, 128)
    );
    assert_eq!(rgb(&img, 128 + 53, 128), [255, 255, 255]);

    dispatch(&mut app, AppCmd::SelectLayer(0));
    dispatch(&mut app, AppCmd::AddFolder);
    let f = app.doc.active;
    dispatch(&mut app, AppCmd::MoveLayer { from: 0, slot: f, depth: 1 });
    println!(
        "[note] stack: {:?} depths {:?} clip {:?}",
        names(&app),
        depths(&app),
        app.doc.layers.iter().map(|l| l.clip).collect::<Vec<_>>()
    );
    let img = shot(&mut app, "s08-clip-to-folder");
    assert_eq!(rgb(&img, 20, 20), [255, 255, 255], "still clipped, now to the folder");
    assert!(rgb(&img, 128, 128)[0] > 100, "red over the disc through the folder");

    let sh = app.doc.layers.iter().position(|l| l.name == "shade").unwrap();
    dispatch(&mut app, AppCmd::SelectLayer(sh));
    dispatch(&mut app, AppCmd::AddLayer);
    let b2 = app.doc.active;
    app.doc.layers[b2].name = "base2".into();
    disc(&mut app, b2, 60, 200, 30, BLACK);
    let sh = app.doc.layers.iter().position(|l| l.name == "shade").unwrap();
    let b2 = app.doc.layers.iter().position(|l| l.name == "base2").unwrap();
    app.status.clear();
    dispatch(&mut app, AppCmd::MoveLayer { from: sh, slot: b2 + 1, depth: 0 });
    println!("[note] 3c drag: {:?} status={:?}", names(&app), status(&app));
    let img = shot(&mut app, "s08-clip-3c");
    println!(
        "[note] 3c: over base2 {:?} over old base {:?}",
        rgb(&img, 60, 200),
        rgb(&img, 128, 128)
    );
    assert!(rgb(&img, 60, 200)[0] > 100, "re-clipped to base2");
    assert_eq!(rgb(&img, 128, 128), [0, 0, 0], "the old base is bare black again");
    dispatch(&mut app, AppCmd::Undo);
    let img = shot(&mut app, "s08-clip-3c-undo");
    assert!(rgb(&img, 128, 128)[0] > 100, "undo restores the old clip");
}

/// Masks: create from a selection, paint on it, disable, apply.
#[test]
fn s09_masks() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    rect(&mut app, 0, 0, 0, 256, 256, GREY);
    dispatch(&mut app, AppCmd::AddLayer);
    let li = app.doc.active;
    rect(&mut app, li, 0, 0, 256, 256, BLACK);
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 64.0, 64.0, 192.0, 192.0,
    ));
    dispatch(&mut app, AppCmd::MaskOutsideSelection);
    println!(
        "[note] mask outside selection: status={:?} has_mask={}",
        status(&app),
        app.doc.layers[li].mask.is_some()
    );
    assert!(app.doc.layers[li].mask.is_some());
    dispatch(&mut app, AppCmd::Deselect);
    let img = shot(&mut app, "s09-mask");
    assert_eq!(luma(&img, 128, 128), 0, "inside the window: black");
    assert!(luma(&img, 20, 20) > 100, "outside the window: the grey below shows");
    println!(
        "[note] mask edge: 63 {} 64 {} 65 {}",
        luma(&img, 63, 128),
        luma(&img, 64, 128),
        luma(&img, 65, 128)
    );

    dispatch(&mut app, AppCmd::MaskEdit);
    assert!(app.mask_edit, "mask edit armed");
    dispatch(&mut app, AppCmd::SetTool(Tool::Eraser));
    dispatch(&mut app, AppCmd::SetBrushSizePx(12.0));
    let layer_before = ink(&app, li);
    drag(&mut app, (64.0, 128.0), (192.0, 128.0));
    let img = shot(&mut app, "s09-mask-eraser");
    println!(
        "[note] eraser on mask: centre luma {} (was 0); layer ink unchanged={}",
        luma(&img, 128, 128),
        ink(&app, li) == layer_before
    );
    pen(&mut app, 12.0, [0.0, 0.0, 0.0]);
    drag(&mut app, (0.0, 30.0), (256.0, 30.0));
    let img = shot(&mut app, "s09-mask-pen");
    println!(
        "[note] pen on mask outside the window: luma at (128,30) = {}",
        luma(&img, 128, 30)
    );
    assert_eq!(ink(&app, li), layer_before, "mask strokes never touch the art");
    dispatch(&mut app, AppCmd::MaskEdit);
    assert!(!app.mask_edit);

    dispatch(&mut app, AppCmd::MaskToggle);
    let img = shot(&mut app, "s09-mask-off");
    println!(
        "[note] mask disabled: status={:?} corner luma {}",
        status(&app),
        luma(&img, 20, 20)
    );
    assert_eq!(luma(&img, 20, 20), 0, "with the mask off the whole black layer shows");
    dispatch(&mut app, AppCmd::MaskToggle);

    let shown = cpu(&app);
    dispatch(&mut app, AppCmd::MaskApply);
    println!(
        "[note] mask apply: status={:?} mask={}",
        status(&app),
        app.doc.layers[li].mask.is_some()
    );
    assert!(app.doc.layers[li].mask.is_none(), "applied");
    assert_eq!(cpu(&app), shown, "baking the mask changes nothing on the page");
    println!("[note] undo labels: {:?}", app.doc.undo_labels());
}

/// The blue draft: layer colour tints the ink on screen AND in export
/// (CSP layer colour prints); the draft FLAG keeps it on screen but out of
/// the export.
#[test]
fn s10_layer_colour_and_draft_export() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    disc(&mut app, 0, 128, 128, 50, BLACK);
    dispatch(
        &mut app,
        AppCmd::ActiveLayer(crate::cmd::ActiveLayerCmd::ToggleColour),
    );
    println!(
        "[note] Ctrl+B layer colour: {:?} status={:?}",
        app.doc.layers[0].layer_colour,
        status(&app)
    );
    let img = shot(&mut app, "s10-layer-colour");
    let c = rgb(&img, 128, 128);
    println!("[note] tinted ink: {:?}", c);
    assert!(c[2] > c[0] + 40, "the stock layer colour is a blue");
    dispatch(&mut app, AppCmd::SetLayerDraft(0, true));
    println!("[note] draft: status={:?}", status(&app));
    let on_screen = screen_shot(&mut app, "s10-draft-screen");
    let exported = shot(&mut app, "s10-draft-export");
    assert!(luma(&on_screen, 128, 128) < 200, "the draft still shows on screen");
    assert_eq!(luma(&exported, 128, 128), 255, "the draft is out of the export");
    println!(
        "[note] undo labels after colour+draft: {:?}",
        app.doc.undo_labels()
    );
}

/// Reference layer: the bucket on a blank layer refers to the reference
/// lineart's box and stops there.
#[test]
fn s11_reference_layer_fill() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    rect(&mut app, 0, 48, 48, 208, 52, BLACK);
    rect(&mut app, 0, 48, 204, 208, 208, BLACK);
    rect(&mut app, 0, 48, 48, 52, 208, BLACK);
    rect(&mut app, 0, 204, 48, 208, 208, BLACK);
    dispatch(&mut app, AppCmd::SetLayerReference(0, true));
    println!("[note] reference: status={:?}", status(&app));
    dispatch(&mut app, AppCmd::AddLayer);
    let flats = app.doc.active;
    dispatch(&mut app, AppCmd::SetLayerVisible(0, false));
    dispatch(&mut app, AppCmd::SetTool(Tool::Fill));
    dispatch(
        &mut app,
        AppCmd::SetFillOpts(mn_core::FillOpts {
            refer: mn_core::FillRefer::Reference,
            ..Default::default()
        }),
    );
    dispatch(&mut app, AppCmd::SetSlotColor([0.0, 0.0, 0.0]));
    click(&mut app, 128.0, 128.0);
    println!(
        "[note] fill via reference: status={:?} ink={}",
        status(&app),
        ink(&app, flats)
    );
    dispatch(&mut app, AppCmd::SetLayerVisible(0, true));
    let img = shot(&mut app, "s11-reference-fill");
    assert_eq!(luma(&img, 128, 128), 0, "filled inside the box");
    assert_eq!(luma(&img, 20, 20), 255, "the fill stopped at the hidden reference box");
    dispatch(&mut app, AppCmd::SetLayerReference(flats, true));
    assert_eq!(
        app.doc.reference_layers().len(),
        2,
        "reference is a SET here (CSP: multiple allowed)"
    );
    dispatch(&mut app, AppCmd::SetLayerReferenceSolo(0));
    assert_eq!(app.doc.reference_layers(), vec![0]);
}

/// Border effect (the white keyline round an SFX) and the tone effect.
#[test]
fn s12_border_effect_and_tone_effect() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    rect(&mut app, 0, 0, 0, 256, 256, GREY);
    dispatch(&mut app, AppCmd::AddLayer);
    let li = app.doc.active;
    disc(&mut app, li, 128, 128, 40, BLACK);
    dispatch(
        &mut app,
        AppCmd::SetEdge(
            li,
            Some(mn_core::EdgeParams {
                width_px: 6.0,
                colour: [255, 255, 255],
                style: mn_core::edge::EdgeStyle::Solid,
            }),
        ),
    );
    println!("[note] border effect: status={:?}", status(&app));
    let img = shot(&mut app, "s12-border");
    let ring = luma(&img, 128 + 43, 128);
    let beyond = luma(&img, 128 + 50, 128);
    println!(
        "[note] border: ring {} beyond {} centre {}",
        ring,
        beyond,
        luma(&img, 128, 128)
    );
    assert!(ring > 240, "white keyline just outside the ink");
    assert!(beyond < 160 && beyond > 100, "grey again past the keyline");
    dispatch(&mut app, AppCmd::SetEdge(li, None));

    disc(&mut app, li, 128, 128, 40, GREY);
    dispatch(&mut app, AppCmd::SetTone(Some(mn_core::ToneParams::default())));
    println!("[note] tone effect: status={:?}", status(&app));
    let img = shot(&mut app, "s12-tone");
    let (mut dark, mut light) = (0, 0);
    for y in 100..156 {
        for x in 100..156 {
            // The backdrop under the dots is the 50 % grey layer.
            if luma(&img, x, y) < 60 {
                dark += 1;
            } else if luma(&img, x, y) > 100 {
                light += 1;
            }
        }
    }
    println!("[note] tone: dark {dark} light {light} of {}", 56 * 56);
    assert!(dark > 300 && light > 300, "a screen of dots, not a flat grey");
}

/// What Layer Property lists for the everyday rows.
#[test]
fn s13_layer_property_rows() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    disc(&mut app, 0, 128, 128, 40, BLACK);
    let raster = panel_text(&mut app);
    println!("[note] Layer Property (raster):\n{raster}");
    dispatch(&mut app, AppCmd::AddFolder);
    let folder = panel_text(&mut app);
    println!("[note] Layer Property (folder):\n{folder}");
    dispatch(&mut app, AppCmd::SelectLayer(0));
    dispatch(&mut app, AppCmd::SetTone(Some(mn_core::ToneParams::default())));
    let tone = panel_text(&mut app);
    println!("[note] Layer Property (tone effect):\n{tone}");
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 64.0, 64.0, 192.0, 192.0,
    ));
    dispatch(&mut app, AppCmd::MaskOutsideSelection);
    let masked = panel_text(&mut app);
    println!("[note] Layer Property (masked):\n{masked}");
    let lower = raster.to_lowercase();
    for want in ["border", "colour", "tone"] {
        assert!(lower.contains(want), "raster property panel lists {want}");
    }
}

/// Select layer by pointing at the page (Operation ▸ Select layer, D).
#[test]
fn s14_select_layer_by_click() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    disc(&mut app, 0, 60, 60, 30, BLACK);
    dispatch(&mut app, AppCmd::AddLayer);
    let li = app.doc.active;
    app.doc.layers[li].name = "tone".into();
    disc(&mut app, li, 190, 190, 30, BLACK);
    dispatch(
        &mut app,
        AppCmd::SetSubTool(crate::cmd::SubTool::Object(
            crate::cmd::ObjectMode::PickLayer,
        )),
    );
    click(&mut app, 60.0, 60.0);
    println!(
        "[note] pick at lineart: active={} status={:?}",
        app.doc.active,
        status(&app)
    );
    assert_eq!(app.doc.active, 0);
    click(&mut app, 190.0, 190.0);
    assert_eq!(app.doc.active, 1);
    dispatch(&mut app, AppCmd::SetLayerVisible(1, false));
    click(&mut app, 190.0, 190.0);
    println!(
        "[note] pick at a HIDDEN layer's ink: active={} status={:?}",
        app.doc.active,
        status(&app)
    );
    assert_eq!(app.doc.active, 1, "a hidden layer is never the answer; the active row stays");
    click(&mut app, 20.0, 240.0);
    println!("[note] pick on paper: status={:?}", status(&app));
}

/// Rename + the "new layer above the frame folder" gap (QA-3 A2) measured
/// on a real comic page.
#[test]
fn s15_rename_and_layer_above_frame_folder() {
    let Some(mut app) = headless() else { return };
    page(&mut app);
    dispatch(&mut app, AppCmd::RenameLayer(0, "ペン入れ".into()));
    assert_eq!(app.doc.layers[0].name, "ペン入れ");
    println!("[note] rename undoable? labels={:?}", app.doc.undo_labels());
    dispatch(&mut app, AppCmd::NewFrameLayer);
    println!(
        "[note] after New frame folder: {:?} depths {:?} active={}",
        names(&app),
        depths(&app),
        app.doc.active
    );
    dispatch(&mut app, AppCmd::AddLayer);
    println!(
        "[note] New layer with the frame folder active: {:?} depths {:?} active={}",
        names(&app),
        depths(&app),
        app.doc.active
    );
    let a = app.doc.active;
    println!(
        "[note] the new layer's depth = {} (0 = above the folder, 1 = inside it)",
        app.doc.layers[a].depth
    );
}
