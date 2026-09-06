//! Lane B1 of the effect-lines parity plan (`docs/plans/2026-09-06-effect-lines-parity.md`):
//! the Tool Property ROW model — per-row eye toggles, per-context ordering,
//! and the `prop_hidden=` migration that keeps a section an artist hid from
//! quietly coming back on the update that split it into rows.
//!
//! What these drive is the registry plus the two seams the window and the
//! palette both go through: `palette_row_ids` is literally the loop
//! `ui::property::tool_property_body` runs, and `move_entry` is the body of
//! the window's up/down arrows. What they cannot reach is which BUTTON is
//! wired to which call — `ui::dialogs` is private to `ui` — so what is not
//! covered here is the widget, and the effect of every one of them is.

use super::App;
use super::headless_renderer;
use super::layout::UiLayout;
use crate::cmd::{FigureMode, Tool};
use crate::ui::property::{
    flat_order, hidden_from_line, move_entry, order_from_json, order_to_json, palette_row_ids,
};

/// `App::new` reads the ui.txt beside the TEST EXE, which the parallel
/// runner shares — a developer's own hidden rows must not decide whether
/// these pass.
fn app() -> Option<App> {
    let renderer = headless_renderer()?;
    let mut app = App::new(renderer, (400, 400), 1.0);
    app.prop_hidden.clear();
    app.prop_order.clear();
    Some(app)
}

fn at(ids: &[&str], id: &str) -> usize {
    ids.iter()
        .position(|s| *s == id)
        .unwrap_or_else(|| panic!("{id} is not in the palette: {ids:?}"))
}

/// A `prop_hidden=` line written before B1 names SECTIONS. The sections that
/// have since been split must expand into their row ids — otherwise the
/// update that split Furigana into five rows silently un-hides Furigana,
/// which is the failure mode nobody would report as a bug and everybody
/// would feel.
///
/// The other half matters just as much: a section that was NOT split keeps
/// its id, because that id already IS its single row's id. Expanding it
/// (or dropping it) would un-hide it.
#[test]
fn hidden_section_id_migrates_to_its_rows() {
    let hidden = hidden_from_line("text.ruby,text.dir,text.edge,who.knows");

    for id in [
        "text.ruby.reading",
        "text.ruby.size",
        "text.ruby.gap",
        "text.ruby.adjust",
        "text.ruby.along",
        "text.dir.vertical",
        "text.dir.auto_tcy",
    ] {
        assert!(hidden.contains(id), "{id} should have been migrated in");
    }
    assert!(
        !hidden.contains("text.ruby") && !hidden.contains("text.dir"),
        "a split section id names nothing any more and must not be kept"
    );
    assert!(
        hidden.contains("text.edge"),
        "an UNSPLIT section id is its row id — leave it exactly as written"
    );
    assert!(
        hidden.contains("who.knows"),
        "an id this build does not know is carried through, not dropped"
    );
}

/// The order survives the real save -> load path: the map becomes the
/// `prop_order=` line, `to_body` writes ui.txt's body, `from_body` reads it
/// back (the seam the layout tests use so a run never touches the ui.txt
/// beside the test exe) and `order_from_json` decodes it.
///
/// Also pinned: an EMPTY map writes an EMPTY line, not `{}`. Writing `{}`
/// would differ from the default every single start, mark the layout dirty
/// and rewrite ui.txt for a user who never reordered anything.
#[test]
fn prop_order_round_trips_through_ui_txt() {
    let Some(mut app) = app() else {
        return;
    };
    app.tool = Tool::Text;

    assert_eq!(order_to_json(&app.prop_order), "", "empty writes nothing");
    app.sync_dock_layout();
    assert_eq!(app.layout.prop_order, "");

    app.prop_order.insert(
        "Text".to_owned(),
        vec!["text.dir".to_owned(), "text.font".to_owned()],
    );
    app.prop_hidden.insert("text.align.in_frame".to_owned());
    app.sync_dock_layout();

    let back = UiLayout::from_body(&app.layout.to_body());
    assert_eq!(
        order_from_json(&back.prop_order),
        app.prop_order,
        "the prop_order= line must survive a round trip"
    );
    assert_eq!(
        hidden_from_line(&back.prop_hidden),
        app.prop_hidden,
        "row ids in prop_hidden= survive too (and migrate to themselves)"
    );

    // A line this build cannot read costs the ORDER and nothing else.
    let junk = UiLayout::from_body("left_w=222\nprop_order=not json at all\nright_w=333\n");
    assert!(order_from_json(&junk.prop_order).is_empty());
    assert_eq!(junk.left_w, 222.0);
    assert_eq!(junk.right_w, 333.0);
}

/// The palette draws sections in the stored order and, inside each, rows in
/// the stored order — with the two degradation rules: an id in the stored
/// list that this build no longer has is ignored, and an id this build has
/// that the list lacks appends at the end in DEFAULT order (never silently
/// disappears, which is what a strict "the list is the list" would do).
#[test]
fn palette_draws_rows_in_stored_order() {
    let Some(mut app) = app() else {
        return;
    };
    app.tool = Tool::Text;

    // Default: Font's own row, then Size, then the Direction section.
    let ids = palette_row_ids(&app);
    assert!(at(&ids, "text.font.font") < at(&ids, "text.font.size"));
    assert!(at(&ids, "text.font.size") < at(&ids, "text.dir.vertical"));

    // A ROW move stays inside its section.
    move_entry(&mut app, "text.font.size", false, true);
    let ids = palette_row_ids(&app);
    assert!(
        at(&ids, "text.font.size") < at(&ids, "text.font.font"),
        "Size moved above Font: {ids:?}"
    );
    assert!(
        at(&ids, "text.font.font") < at(&ids, "text.dir.vertical"),
        "a row move must not reorder the sections: {ids:?}"
    );

    // A SECTION move takes its rows with it.
    move_entry(&mut app, "text.dir", true, true);
    let ids = palette_row_ids(&app);
    assert!(
        at(&ids, "text.dir.vertical") < at(&ids, "text.font.size"),
        "Direction moved above Font, rows and all: {ids:?}"
    );
    assert!(
        at(&ids, "text.dir.vertical") < at(&ids, "text.dir.auto_tcy"),
        "the moved section keeps its own row order: {ids:?}"
    );

    // A hand-written list: one real id, one this build never had.
    app.prop_order.insert(
        "Text".to_owned(),
        vec!["text.spacing".to_owned(), "text.nonesuch".to_owned()],
    );
    let ids = palette_row_ids(&app);
    assert_eq!(
        at(&ids, "text.spacing.line_mode"),
        0,
        "the one listed section leads: {ids:?}"
    );
    assert!(
        at(&ids, "text.workstyle") < at(&ids, "text.font.font"),
        "everything unlisted appends in DEFAULT order: {ids:?}"
    );
    assert!(!ids.contains(&"text.nonesuch"));

    // The eye toggle takes a single row out and leaves its neighbours.
    app.prop_order.clear();
    app.prop_hidden.insert("text.align.in_frame".to_owned());
    let ids = palette_row_ids(&app);
    assert!(!ids.contains(&"text.align.in_frame"));
    assert!(ids.contains(&"text.align.rows"));
}

/// B1.4, straight from the owner's example: "for fonts the
/// horizontal/vertical ordering and centered/left/right aligned should be
/// next to each other". Direction is now directly above Align — Style and
/// Furigana, which used to sit between them, moved below Spacing.
#[test]
fn default_text_order_puts_direction_before_align() {
    let Some(mut app) = app() else {
        return;
    };
    app.tool = Tool::Text;
    let ids = palette_row_ids(&app);

    let order = [
        "text.workstyle",
        "text.font.font",
        "text.font.size",
        "text.dir.vertical",
        "text.dir.auto_tcy",
        "text.align.rows",
        "text.align.in_frame",
        "text.spacing.line_mode",
        "text.spacing.letter",
        "text.style.marks",
        "text.edge",
        "text.ruby.reading",
        "text.guide",
    ];
    let mut last = 0;
    for id in order {
        let i = at(&ids, id);
        assert!(i >= last, "{id} is out of place in {ids:?}");
        last = i;
    }
    assert!(
        at(&ids, "text.dir.auto_tcy") + 1 == at(&ids, "text.align.rows"),
        "Direction must end where Align begins: {ids:?}"
    );

    // The Operation tool's text context is the SAME list (plan B1.4).
    app.tool = Tool::Object;
    // No text box is selected in a fresh document, so this is the shape
    // check that matters: the Text tool's own list is what obj.text reuses,
    // and `TEXT_SECTIONS` is the single definition of it.
    assert!(!palette_row_ids(&app).is_empty());
}

/// The settings window BUILDS, and it settles — the Preferences window's
/// bug, not repeated. A vertical `ui.separator()` between the rail and the
/// body of an auto-sized window is a feedback loop (the separator stretches
/// to last frame's height, the footer adds to it) and the window grew ~50 pt
/// every frame until it walked off the desktop, in a build that shipped.
/// This window copies the PAINTED divider instead, and this is the test that
/// says so out loud.
#[test]
fn the_settings_window_settles_on_screen() {
    let Some(mut app) = app() else {
        return;
    };
    let (w, h) = (1280u32, 860u32);
    app.tool = Tool::Text;
    app.prop_detail_open = true;
    let ctx = app.shell.ctx.clone();
    let window_rect = |ctx: &egui::Context| -> egui::Rect {
        ctx.memory(|m| {
            m.areas()
                .visible_layer_ids()
                .iter()
                .filter(|l| l.order == egui::Order::Middle)
                .filter_map(|l| m.area_rect(l.id))
                .next()
                .expect("the settings window is visible")
        })
    };
    // Every section of every context lane B2 converted. Two things at once:
    // the window settles (its own subject), and every row BODY survives
    // being drawn — the window draws rows that do not apply to the armed sub
    // tool, greyed, so a row whose body assumed its old `if` would panic
    // here and nowhere else.
    for (tool, mode, secs) in [
        (Tool::Text, FigureMode::Line, 9usize),
        (Tool::Figure, FigureMode::Stream, 5),
        (Tool::Figure, FigureMode::Rect, 5),
        (Tool::Balloon, FigureMode::Line, 4),
        (Tool::Frame, FigureMode::Line, 2),
        (Tool::Fill, FigureMode::Line, 2),
        (Tool::Tone, FigureMode::Line, 3),
        (Tool::Wand, FigureMode::Line, 2),
        (Tool::Select, FigureMode::Line, 1),
        (Tool::Gradient, FigureMode::Line, 4),
        (Tool::Ruler, FigureMode::Line, 3),
    ] {
        app.tool = tool;
        app.figure_mode = mode;
        for sec in 0..secs {
            app.prop_detail_sec = sec;
            let mut last = egui::Rect::NOTHING;
            // Four frames to settle (anchor, then the auto-size), then STILL
            // — a window that keeps moving on frame five is the feedback
            // loop this window was shaped to avoid.
            for i in 0..8 {
                let raw = app.shell.begin((w, h));
                let mut out = ctx.run_ui(raw, |ui| crate::ui::build(ui, &mut app));
                out.textures_delta.clear();
                let r = window_rect(&ctx);
                if i >= 4 {
                    assert_eq!(r, last, "{tool:?} section {sec} frame {i}: still moving");
                }
                last = r;
            }
            assert!(
                last.top() >= 0.0 && last.bottom() <= h as f32,
                "{tool:?} section {sec} settled off-screen: {last:?}"
            );
        }
    }
}

/// B1.1: a section that does not apply to the armed sub tool is skipped in
/// the palette. With an effect-line preset armed the Figure tool's Brush and
/// Dynamics editors did nothing at all — the generator places its own layer
/// and never touches the brush — so they took a screenful of a palette that
/// has nineteen numbers in it already.
#[test]
fn non_applying_sections_are_skipped() {
    let Some(mut app) = app() else {
        return;
    };
    app.tool = Tool::Figure;

    app.figure_mode = FigureMode::Line;
    let ids = palette_row_ids(&app);
    assert!(
        ids.contains(&"figure.brush") && ids.contains(&"figure.dynamics"),
        "an INKING figure sub tool still gets the brush: {ids:?}"
    );

    for mode in [
        FigureMode::Stream,
        FigureMode::Focus,
        FigureMode::Urchin,
        FigureMode::SolidFlash,
    ] {
        app.figure_mode = mode;
        let ids = palette_row_ids(&app);
        assert!(
            !ids.contains(&"figure.brush"),
            "{mode:?} generates its own layer — no Brush: {ids:?}"
        );
        assert!(
            !ids.contains(&"figure.dynamics"),
            "{mode:?} generates its own layer — no Dynamics: {ids:?}"
        );
        assert!(
            ids.contains(&"figure.opts.width"),
            "the effect-line knobs are still there: {ids:?}"
        );
    }
}

/// Lane B2: every panel is rows now, and an id is only a stable handle if it
/// is UNIQUE inside the context it lives in. `prop_order` stores one flat
/// list of section ids and row ids per context and looks a row up by
/// `position`, so two rows sharing an id would silently both jump to
/// wherever the first one sits, and one eye toggle would hide both.
///
/// The check runs over the FULL id list (`flat_order`), not the visible
/// palette: a row that does not apply to the armed sub tool still occupies
/// its id, and it is the id space `prop_order` writes down.
#[test]
fn every_context_has_unique_row_ids() {
    let Some(mut app) = app() else {
        return;
    };

    let check = |app: &App, what: &str| {
        let ids = flat_order(app);
        let mut seen = std::collections::BTreeSet::new();
        for id in &ids {
            assert!(
                seen.insert(id.clone()),
                "{what}: {id} appears twice — {ids:?}"
            );
        }
    };

    for tool in [
        Tool::Pen,
        Tool::Eraser,
        Tool::Figure,
        Tool::Gradient,
        Tool::Fill,
        Tool::Tone,
        Tool::Select,
        Tool::SelPen,
        Tool::SelEraser,
        Tool::Wand,
        Tool::Object,
        Tool::Frame,
        Tool::Balloon,
        Tool::Text,
        Tool::Eyedrop,
        Tool::Liquify,
        Tool::Ruler,
        Tool::Pan,
    ] {
        app.tool = tool;
        // The Figure tool swaps knobs per armed sub tool, and the sub tool
        // decides which rows exist at all — walk them too.
        if tool == Tool::Figure {
            for m in [
                FigureMode::Line,
                FigureMode::Stream,
                FigureMode::Focus,
                FigureMode::Urchin,
                FigureMode::SolidFlash,
            ] {
                app.figure_mode = m;
                check(&app, &format!("{tool:?}/{m:?}"));
            }
            app.figure_mode = FigureMode::Line;
            continue;
        }
        check(&app, &format!("{tool:?}"));
    }

    // The Operation tool's palette is the SELECTED OBJECT's, so each kind of
    // selection is its own context with its own stored order.
    app.tool = Tool::Object;
    app.object_mode = crate::cmd::ObjectMode::PickLayer;
    check(&app, "obj.picklayer");
    app.object_mode = crate::cmd::ObjectMode::Object;

    app.text_sel = Some((0, 0));
    check(&app, "obj.text");
    app.text_sel = None;

    app.balloon_sel = Some((0, 0));
    check(&app, "obj.balloon");
    app.balloon_sel = None;

    app.doc.layers[0].genlines = Some(mn_core::genlines::GenLinesSpec::default());
    app.gen_sel = Some(0);
    check(&app, "obj.gen");
    app.gen_sel = None;
    app.doc.layers[0].genlines = None;

    app.object_sel = Some((0, 0));
    check(&app, "obj.frame");
    app.object_sel = None;
}

/// The knobs an effect-line sub tool has depend on WHICH KIND it is, and lane
/// B2 moved those conditions out of the section bodies onto `Row::applies`.
/// The two that would be nonsense are the pairs below: a stream has no centre
/// to hollow and no arc to sweep, and a burst's rays cannot start off a line
/// or fan toward a point — they already converge on one.
///
/// This is the test that says the move kept the conditions. Before it, the
/// same rule lived inside an `if` nothing could reach.
#[test]
fn effect_line_rows_apply_per_kind() {
    let Some(mut app) = app() else {
        return;
    };
    app.tool = Tool::Figure;

    app.figure_mode = FigureMode::Stream;
    let ids = palette_row_ids(&app);
    for id in ["figure.opts.sweep", "figure.opts.hollow"] {
        assert!(
            !ids.contains(&id),
            "a stream has no hollow centre and no sweep: {id} in {ids:?}"
        );
    }
    for id in ["figure.opts.start", "figure.opts.fan", "figure.opts.width"] {
        assert!(ids.contains(&id), "a stream keeps {id}: {ids:?}");
    }
    // A stream's own wobbles: the angular one is its alone, the radial ones
    // are not there at all.
    assert!(ids.contains(&"figure.wobble.angle"));
    assert!(!ids.contains(&"figure.wobble.core"));
    assert!(!ids.contains(&"figure.wobble.outer_length"));

    app.figure_mode = FigureMode::Focus;
    let ids = palette_row_ids(&app);
    for id in ["figure.opts.start", "figure.opts.fan"] {
        assert!(
            !ids.contains(&id),
            "a burst's rays already converge — no {id}: {ids:?}"
        );
    }
    for id in [
        "figure.opts.sweep",
        "figure.opts.hollow",
        "figure.wobble.core",
        "figure.wobble.outer_length",
    ] {
        assert!(ids.contains(&id), "a burst keeps {id}: {ids:?}");
    }
    assert!(!ids.contains(&"figure.wobble.angle"));

    // A FLASH has neither stroke profile nor length skew: its teeth are
    // filled wedges, counted and spread over the whole circle.
    for m in [FigureMode::Urchin, FigureMode::SolidFlash] {
        app.figure_mode = m;
        let ids = palette_row_ids(&app);
        for id in [
            "figure.opts.taper",
            "figure.opts.entry",
            "figure.opts.needle",
            "figure.opts.accents",
            "figure.opts.sweep",
            "figure.wobble.width",
        ] {
            assert!(!ids.contains(&id), "{m:?} has no {id}: {ids:?}");
        }
        assert!(ids.contains(&"figure.opts.hollow"), "{m:?}: {ids:?}");
        assert!(ids.contains(&"figure.wobble.core"), "{m:?}: {ids:?}");
    }

    // An INKING figure sub tool has none of them, and gets its own two rows.
    app.figure_mode = FigureMode::Rect;
    let ids = palette_row_ids(&app);
    assert!(!ids.iter().any(|id| id.starts_with("figure.wobble.")));
    assert!(ids.contains(&"figure.opts.fill"));
    assert!(ids.contains(&"figure.opts.adjust_angle"));
}
