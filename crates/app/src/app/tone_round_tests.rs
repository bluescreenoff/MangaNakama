use crate::cmd::{AppCmd, dispatch};
use mn_core::{ToneDensity, ToneParams, TonePattern};

/// TN-011 is a VIEW state and nothing else: it flips, it says so, and it
/// leaves the document alone. A pre-print check that dirtied the file or
/// spent an undo step would be worse than no check at all.
#[test]
fn show_tone_area_toggles_without_touching_the_document() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    assert!(!app.tone_show_area, "off until asked for");
    let undoable = app.doc.can_undo();
    dispatch(&mut app, AppCmd::ToneShowArea);
    assert!(app.tone_show_area);
    dispatch(&mut app, AppCmd::ToneShowArea);
    assert!(!app.tone_show_area);
    assert_eq!(app.doc.can_undo(), undoable, "a view toggle is not an edit");
}

/// Everything the tone panel can set arrives on the layer through the one
/// command it pushes, survives an undo, and comes back on redo — the
/// params are the undo unit, the painted ink underneath never moves.
#[test]
fn every_tone_parameter_round_trips_through_the_command_and_undo() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    super::new_document_tests::scribble(&mut app);
    let ink_before = super::new_document_tests::all_ink(&app);

    let p = ToneParams {
        pattern: TonePattern::Star,
        lpi: 55.0,
        angle_deg: 15.0,
        offset: [4.0, -1.5],
        posterize: Some(5),
        density: ToneDensity::ImageBrightness,
    };
    dispatch(&mut app, AppCmd::SetTone(Some(p)));
    let i = app.doc.active;
    assert_eq!(app.doc.layers[i].tone, Some(p), "the whole struct arrived");
    assert!(
        app.status.contains("lattice"),
        "a non-zero lattice offset says so: {}",
        app.status
    );

    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers[i].tone, None, "undo unmade the tone layer");
    dispatch(&mut app, AppCmd::Redo);
    assert_eq!(app.doc.layers[i].tone, Some(p), "redo put it back");
    assert_eq!(
        super::new_document_tests::all_ink(&app),
        ink_before,
        "the painted SOURCE ink is untouched in both directions"
    );
}

// --- frames: TRIAGE 127/128/129 -------------------------------------

/// TRIAGE 128 (FB-026): the three answers to "what happens to the art".
/// Create-empty gives the new half a blank White + draw pair; Duplicate
/// copies the folder's contents into it; Do-not-change declines the new
/// folder entirely and leaves the cut inside the one folder.
#[test]
fn divide_contents_decides_what_the_new_half_gets() {
    use crate::cmd::DivideContents;
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let build = |app: &mut super::App| {
        while app.doc.layers.iter().any(|l| l.is_frame()) {
            let i = app.doc.layers.iter().position(|l| l.is_frame()).unwrap();
            app.doc.remove_layer(i);
        }
        let h = app.doc.add_frame_folder(
            "Frame 1",
            mn_core::FrameSet::single_rect([20.0, 20.0, 380.0, 280.0], 4.0),
        );
        app.doc.set_active(h);
        app.frame_mode = crate::cmd::FrameMode::DivideFolder;
        h
    };
    let cut = |app: &mut super::App| {
        dispatch(
            app,
            AppCmd::FrameDivide {
                a: (200.0, 10.0),
                b: (200.0, 290.0),
            },
        );
    };
    let folders = |app: &super::App| app.doc.layers.iter().filter(|l| l.is_frame()).count();

    build(&mut app);
    app.frame_divide_contents = DivideContents::CreateEmpty;
    let before = app.doc.layers.len();
    cut(&mut app);
    assert_eq!(folders(&app), 2, "create-empty spawns the sibling folder");
    assert_eq!(
        app.doc.layers.len() - before,
        3,
        "and exactly a header + White + draw layer came with it"
    );

    build(&mut app);
    app.frame_divide_contents = DivideContents::Duplicate;
    let named: Vec<String> = app.doc.layers.iter().map(|l| l.name.clone()).collect();
    cut(&mut app);
    assert_eq!(folders(&app), 2, "duplicate spawns the sibling folder too");
    let after: Vec<String> = app.doc.layers.iter().map(|l| l.name.clone()).collect();
    assert!(
        after.len() > named.len(),
        "and the contents rode along: {after:?}"
    );
    assert!(
        app.status.contains("copy of its art"),
        "the status says which answer ran: {}",
        app.status
    );

    build(&mut app);
    app.frame_divide_contents = DivideContents::DoNotChange;
    let before = app.doc.layers.len();
    cut(&mut app);
    assert_eq!(folders(&app), 1, "do-not-change makes NO new folder");
    assert_eq!(app.doc.layers.len(), before, "and no new layers at all");
    assert_eq!(
        app.doc.layers[app.doc.active]
            .frames()
            .unwrap()
            .frames
            .len(),
        2,
        "the border was still drawn: one folder, two panels"
    );
}

/// TRIAGE 129 (FB-030): a TAP on a panel edge runs it off the page, and
/// the same tap between two panels closes the gutter instead. A DRAG on
/// the same tool still divides — the two gestures must not collide.
#[test]
fn tapping_a_panel_edge_extends_it_and_dragging_still_divides() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let h = app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([40.0, 60.0, 360.0, 240.0], 4.0),
    );
    app.doc.set_active(h);
    app.frame_mode = crate::cmd::FrameMode::DivideBorder;

    // Tap the top edge (y = 60): it leaves the page.
    dispatch(&mut app, AppCmd::FrameExtendEdge { at: (200.0, 60.0) });
    let bb = app.doc.layers[h].frames().unwrap().frames[0].bbox();
    assert!(bb[1] < 0.0, "the top edge ran off the page: {bb:?}");
    assert!((bb[3] - 240.0).abs() < 0.01, "and nothing else moved");

    // A tap in the middle of the panel hits no edge and says so.
    dispatch(&mut app, AppCmd::FrameExtendEdge { at: (200.0, 150.0) });
    assert!(app.status.contains("tap ON a panel edge"), "{}", app.status);

    // A drag on the same tool still divides.
    let n = app.doc.layers[h].frames().unwrap().frames.len();
    dispatch(
        &mut app,
        AppCmd::FrameDivide {
            a: (200.0, 70.0),
            b: (200.0, 230.0),
        },
    );
    assert_eq!(
        app.doc.layers[h].frames().unwrap().frames.len(),
        n + 1,
        "the drag is still a cut"
    );
}

/// TRIAGE 129 (FB-023..025): equal division applies the grid in one
/// command, refuses to guess which panel when there are several, and
/// spends the divide-border gutter values doing it.
#[test]
fn divide_equally_lays_the_grid_in_one_command() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let h = app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([20.0, 20.0, 380.0, 280.0], 4.0),
    );
    app.doc.set_active(h);
    dispatch(
        &mut app,
        AppCmd::FrameDivideEqually {
            cols: 2,
            rows: 3,
            fit_to_side: false,
        },
    );
    let fs = app.doc.layers[h].frames().unwrap();
    assert_eq!(fs.frames.len(), 6, "2 x 3 in one command");
    assert!(app.status.contains("2 x 3"), "{}", app.status);
    let total: f32 = fs.frames.iter().map(|f| f.area()).sum();
    assert!(total < 360.0 * 260.0, "the gutters cost area");

    // Six panels now: it will not guess which one to divide again.
    dispatch(
        &mut app,
        AppCmd::FrameDivideEqually {
            cols: 2,
            rows: 2,
            fit_to_side: false,
        },
    );
    assert_eq!(app.doc.layers[h].frames().unwrap().frames.len(), 6);
    assert!(app.status.contains("Object tool"), "{}", app.status);
}

/// TRIAGE 127 (FB-053/054): the border-as-ruler toggle drops the ink,
/// publishes the outline as a curve ruler, turns snapping on, and takes
/// exactly its own curves back when switched off — a hand-drawn curve
/// ruler sitting in the same list is never the one that disappears.
#[test]
fn border_as_ruler_publishes_the_outline_and_retracts_only_its_own() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let mine = mn_core::CurveRuler {
        pts: vec![[0.0, 0.0], [10.0, 10.0]],
    };
    app.doc.rulers.curves.push(mine.clone());
    let h = app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([20.0, 20.0, 380.0, 280.0], 4.0),
    );

    dispatch(&mut app, AppCmd::FrameBorderRuler { layer: h });
    assert!(app.doc.layers[h].frames().unwrap().border_ruler);
    assert_eq!(
        app.doc.layers[h].frames().unwrap().border_px,
        4.0,
        "the width is remembered, not zeroed"
    );
    assert_eq!(
        app.doc.rulers.curves.len(),
        2,
        "the outline joined the rulers"
    );
    assert!(app.doc.rulers.on, "and snapping came on with it");
    assert_eq!(
        app.doc.rulers.curves[0], mine,
        "the hand-drawn curve ruler kept its place"
    );

    // Reshaping the panel moves its ruler with it.
    let mut fs = app.doc.layers[h].frames().unwrap().clone();
    fs.frames[0].translate(5.0, 7.0);
    app.doc.set_frames(h, fs);
    app.renumber_frames();
    assert_eq!(app.doc.rulers.curves.len(), 2, "still one frame ruler");
    assert_eq!(
        app.doc.rulers.curves[1].pts[0],
        [25.0, 27.0],
        "and it followed the panel"
    );

    dispatch(&mut app, AppCmd::FrameBorderRuler { layer: h });
    assert!(!app.doc.layers[h].frames().unwrap().border_ruler);
    assert_eq!(
        app.doc.rulers.curves,
        vec![mine],
        "only the frame's own curve was retracted"
    );
}

/// Issue #3: `Clear rulers` emptied `items` and left `curves` untouched, so
/// every curve ruler survived a clear. It clears the hand-made ones now —
/// and leaves the frame-published ones alone, because those belong to the
/// panel and `sync_frame_rulers` retracts them by value (clearing them here
/// would strand that bookkeeping until the next frame edit).
#[test]
fn clearing_rulers_takes_the_hand_made_curves_but_not_the_frames() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let mine = mn_core::CurveRuler {
        pts: vec![[0.0, 0.0], [10.0, 10.0]],
    };
    app.doc.rulers.curves.push(mine.clone());
    app.doc.rulers.items.push(mn_core::Ruler::Line {
        a: [0.0, 0.0],
        b: [50.0, 50.0],
    });
    let h = app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([20.0, 20.0, 380.0, 280.0], 4.0),
    );
    dispatch(&mut app, AppCmd::FrameBorderRuler { layer: h });
    assert_eq!(
        app.doc.rulers.curves.len(),
        2,
        "hand-made + the panel outline"
    );

    dispatch(&mut app, AppCmd::RulerClear);
    assert!(app.doc.rulers.items.is_empty(), "the line family went");
    assert_eq!(
        app.doc.rulers.curves, app.frame_rulers,
        "the hand-made curve went; the panel's own stayed"
    );
    assert_eq!(app.doc.rulers.curves.len(), 1, "and it is the frame's one");
    assert!(
        !app.doc.rulers.curves.contains(&mine),
        "the hand-made one is gone"
    );

    // The frame's ruler is still the frame's: switching the border back on
    // retracts exactly it, leaving nothing behind.
    dispatch(&mut app, AppCmd::FrameBorderRuler { layer: h });
    assert!(
        app.doc.rulers.curves.is_empty(),
        "retraction still finds its own curve after a clear"
    );

    // With no frame rulers at all, a clear empties the list outright.
    app.doc.rulers.curves.push(mine.clone());
    dispatch(&mut app, AppCmd::RulerClear);
    assert!(
        app.doc.rulers.curves.is_empty(),
        "{:?}",
        app.doc.rulers.curves
    );
}
