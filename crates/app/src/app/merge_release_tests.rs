//! Wave 1's two missing layer commands: **Merge selected layers** (CSP
//! 選択中のレイヤーを結合) and **Release folder** (レイヤーフォルダーを解除).
//! Neither existed at all, so `keys.json` had nothing for the owner's
//! Shift+Alt+E / Ctrl+Shift+G to point at.
//!
//! The two things that matter for both: it is ONE undo press, and the page
//! still composites to the same pixels afterwards. A merge or an ungroup
//! that quietly changed the art would be the worst kind of bug here —
//! nothing errors, the page just looks wrong three pages later.

use super::*;
use crate::cmd::{AppCmd, dispatch};

fn page(app: &App) -> Vec<u8> {
    mn_core::export::composite(&app.doc, mn_core::Background::White).into_raw()
}

/// Ink a horizontal band across layer `li` so the merge has something to
/// blend. Direct tile writes: the point here is the stack maths, not the
/// brush. `y` is tile-local, so the bands stack inside one 64×64 tile.
fn band(app: &mut App, li: usize, y: usize, colour: [u16; 4]) {
    let t = app.doc.layers[li].tile_mut(mn_core::TileIdx::new(0, 0));
    for x in 0..48 {
        t.set_pixel(x, y, colour);
    }
}

fn names(app: &App) -> Vec<String> {
    app.doc.layers.iter().map(|l| l.name.clone()).collect()
}

/// The headline: three selected rows become one, in one press, and the page
/// composites to exactly the same pixels — a contiguous Normal-blend
/// selection is the everyday case and it must be lossless.
#[test]
fn merge_selected_flattens_the_selection_for_one_press_and_one_page() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    const INK: [u16; 4] = [0, 0, 0, mn_core::FIX15_ONE as u16];
    band(&mut app, 0, 10, INK);
    dispatch(&mut app, AppCmd::AddLayer);
    let li = app.doc.active;
    band(&mut app, li, 14, INK);
    dispatch(&mut app, AppCmd::AddLayer);
    let li = app.doc.active;
    band(&mut app, li, 18, INK);
    // A half-opacity layer, so the merge has to honour opacity and not just
    // copy pixels.
    app.doc.layers[2].opacity = 0.5;
    let before = page(&app);
    let stack = app.doc.layers.len();

    assert!(app.doc.toggle_multi(0));
    assert!(app.doc.toggle_multi(1));
    assert_eq!(app.doc.multi_targets(), vec![0, 1, 2]);
    let steps = app.doc.undo_labels().len();

    dispatch(&mut app, AppCmd::MergeSelected);
    assert_eq!(app.doc.layers.len(), stack - 2, "three rows became one");
    assert_eq!(app.doc.active, 0, "the result lands at the lowest of them");
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "ONE press for the whole set"
    );
    assert_eq!(
        app.doc.undo_labels().last().map(String::as_str),
        Some("Merge selected layers")
    );
    assert_eq!(page(&app), before, "and the page is pixel-for-pixel the same");

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers.len(), stack, "one press puts them all back");
    assert_eq!(page(&app), before);

    // It must agree with merging DOWN — a two-row selection and Ctrl+E on
    // the same pair are the same operation, and a second blend loop that
    // drifted from the first is exactly how they would stop agreeing.
    app.doc.set_active(1);
    assert!(app.doc.toggle_multi(2));
    assert_eq!(app.doc.multi_targets(), vec![1, 2]);
    dispatch(&mut app, AppCmd::MergeSelected);
    let via_selection = page(&app);
    dispatch(&mut app, AppCmd::Undo);
    app.doc.set_active(2);
    dispatch(&mut app, AppCmd::MergeDown);
    assert_eq!(page(&app), via_selection, "one blend loop, two doors");
}

/// The refusals are Merge-down's, said out loud rather than silently
/// half-done: a folder in the selection, a locked row, or rows on either
/// side of a folder edge all stop the whole thing.
#[test]
fn merge_selected_refuses_what_merge_down_refuses() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    dispatch(&mut app, AppCmd::AddLayer);
    dispatch(&mut app, AppCmd::AddLayer);
    let stack = app.doc.layers.len();
    let steps = app.doc.undo_labels().len();
    let unchanged = |app: &App, why: &str| {
        assert_eq!(app.doc.layers.len(), stack, "{why}");
        assert_eq!(app.doc.undo_labels().len(), steps, "no step spent: {why}");
    };

    // A single row is not a selection.
    app.doc.set_active(0);
    dispatch(&mut app, AppCmd::MergeSelected);
    unchanged(&app, "one row is not a selection");
    assert!(app.status.contains("select the layers"), "{}", app.status);

    // A locked row refuses edits, and refusing means the WHOLE set stands
    // — a half-merge would be worse than no merge.
    app.doc.layers[1].lock = true;
    app.doc.set_active(0);
    assert!(app.doc.toggle_multi(1));
    dispatch(&mut app, AppCmd::MergeSelected);
    unchanged(&app, "a locked layer refuses edits");
    assert!(app.status.contains("will not merge"), "{}", app.status);
    app.doc.layers[1].lock = false;

    // A folder in the set, and a set that spans a folder edge (which would
    // smuggle pixels into or out of a mask). `add_folder_above` makes an
    // EMPTY folder; `add_layer_in_folder` inserts below the header and so
    // pushes the header up one — hence the recomputed index.
    app.doc.set_active(1);
    dispatch(&mut app, AppCmd::AddFolder);
    let kid = app
        .doc
        .add_layer_in_folder(app.doc.active, "inside")
        .expect("a child of the fresh folder");
    let header = kid + 1;
    assert!(app.doc.layers[header].folder, "the header is above its child");
    assert_eq!(app.doc.layers[kid].depth, 1, "and the child is nested");
    let stack = app.doc.layers.len();
    let steps = app.doc.undo_labels().len();
    let unchanged = |app: &App, why: &str| {
        assert_eq!(app.doc.layers.len(), stack, "{why}");
        assert_eq!(app.doc.undo_labels().len(), steps, "no step spent: {why}");
    };

    app.doc.set_active(header);
    assert!(app.doc.toggle_multi(0));
    dispatch(&mut app, AppCmd::MergeSelected);
    unchanged(&app, "a folder never merges");

    app.doc.set_active(kid);
    assert!(app.doc.toggle_multi(0));
    dispatch(&mut app, AppCmd::MergeSelected);
    unchanged(&app, "a set spanning a folder edge never merges");
}

/// Release folder: the children step out to the header's level, keep their
/// order, the header goes — one press, same page.
#[test]
fn release_folder_steps_the_children_out_for_one_press_and_one_page() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    const INK: [u16; 4] = [0, 0, 0, mn_core::FIX15_ONE as u16];
    band(&mut app, 0, 10, INK);
    // One layer outside the folder, two inside it. `add_layer_in_folder`
    // inserts BELOW the header, so the header's index climbs with each
    // child — recompute it, never cache it.
    dispatch(&mut app, AppCmd::AddLayer);
    let outside = app.doc.active;
    band(&mut app, outside, 14, INK);
    dispatch(&mut app, AppCmd::AddFolder);
    let inner_a = app
        .doc
        .add_layer_in_folder(app.doc.active, "inside A")
        .expect("a child of the fresh folder");
    band(&mut app, inner_a, 18, INK);
    let inner_b = app
        .doc
        .add_layer_in_folder(inner_a + 1, "inside B")
        .expect("a second child");
    band(&mut app, inner_b, 22, INK);
    let folder = inner_b + 1;
    assert!(app.doc.layers[folder].folder, "the header sits above both");
    assert!(app.doc.rename_layer(folder, "group"));
    assert_eq!(app.doc.layers[inner_b].depth, 1, "they really are nested");

    let before = page(&app);
    let stack = app.doc.layers.len();
    let kept: Vec<String> = names(&app).into_iter().filter(|n| n != "group").collect();
    let steps = app.doc.undo_labels().len();

    app.doc.set_active(folder);
    assert!(
        app.doc.folder_release_is_lossless(folder),
        "a plain folder dresses nothing"
    );
    dispatch(&mut app, AppCmd::ReleaseFolder);

    assert_eq!(app.doc.layers.len(), stack - 1, "only the header went");
    assert_eq!(names(&app), kept, "order is untouched");
    assert!(
        app.doc.layers.iter().all(|l| l.depth == 0),
        "the children stepped out to the folder's own level"
    );
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "ONE press for the dissolve"
    );
    assert_eq!(
        app.doc.undo_labels().last().map(String::as_str),
        Some("Release folder")
    );
    assert_eq!(page(&app), before, "and the page is unchanged");

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers.len(), stack, "one press rebuilds the folder");
    assert!(app.doc.layers[folder].folder, "the header is back");
    assert_eq!(app.doc.layers[inner_b].depth, 1, "…and so is the nesting");
    assert_eq!(page(&app), before);
}

/// Release works from a CHILD row too (you rarely have the header
/// selected), a frame folder is sent to the command that can actually
/// dissolve it, and a folder carrying its own opacity says so instead of
/// changing the page quietly.
#[test]
fn release_folder_targets_the_enclosing_folder_and_warns_when_it_costs() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    dispatch(&mut app, AppCmd::AddLayer);
    dispatch(&mut app, AppCmd::AddFolder);
    let kid = app
        .doc
        .add_layer_in_folder(app.doc.active, "inside")
        .expect("a child");
    // The child went in BELOW the header, so the header moved up one.
    let folder = kid + 1;
    assert!(app.doc.layers[folder].folder);
    const INK: [u16; 4] = [0, 0, 0, mn_core::FIX15_ONE as u16];
    band(&mut app, kid, 20, INK);
    app.doc.layers[folder].opacity = 0.5;
    let dressed = page(&app);

    // Selected row = the CHILD, not the header.
    app.doc.set_active(kid);
    assert!(
        !app.doc.folder_release_is_lossless(folder),
        "a half-opacity folder cannot hand that down"
    );
    let steps = app.doc.undo_labels().len();
    dispatch(&mut app, AppCmd::ReleaseFolder);
    assert!(
        !app.doc.layers.iter().any(|l| l.folder),
        "it found the enclosing folder from a child row"
    );
    assert_eq!(app.doc.undo_labels().len(), steps + 1);
    assert!(app.status.contains("looks different"), "{}", app.status);
    assert_ne!(
        page(&app),
        dressed,
        "the warning is honest — the group opacity really is gone"
    );
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(page(&app), dressed, "and one press puts it back");

    // A FRAME folder keeps its header (it holds the panel and the mask its
    // children are clipped by) and is pointed at the command that can.
    dispatch(&mut app, AppCmd::NewFrameLayer);
    let frame = app
        .doc
        .layers
        .iter()
        .position(|l| l.folder && l.is_frame())
        .expect("a frame folder");
    app.doc.set_active(frame);
    let stack = app.doc.layers.len();
    let steps = app.doc.undo_labels().len();
    dispatch(&mut app, AppCmd::ReleaseFolder);
    assert_eq!(app.doc.layers.len(), stack, "the frame folder stands");
    assert_eq!(app.doc.undo_labels().len(), steps, "no step spent");
    assert!(app.status.contains("Rasterize frame folder"), "{}", app.status);
}

/// Both commands must be findable by name, because that — not a built-in
/// chord — is how `keys.json` binds them.
#[test]
fn both_commands_are_reachable_by_name() {
    let index = crate::ui::quick::command_index();
    for (want, is_it) in [
        (
            "Merge selected layers (combine the palette selection)",
            &(|c: &AppCmd| matches!(c, AppCmd::MergeSelected)) as &dyn Fn(&AppCmd) -> bool,
        ),
        (
            "Release folder (ungroup, children step out)",
            &(|c: &AppCmd| matches!(c, AppCmd::ReleaseFolder)) as &dyn Fn(&AppCmd) -> bool,
        ),
    ] {
        let hit = index
            .iter()
            .find(|(label, _, _)| *label == want)
            .unwrap_or_else(|| panic!("keys.json cannot bind what the palette does not list: {want}"));
        assert!(is_it(&hit.2), "{want} runs the wrong command");
        // The label a user has to type must be typeable: ASCII only, no
        // em dash smuggled in by a doc comment.
        assert!(want.is_ascii(), "{want} is not typeable in keys.json");
    }
}
