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
    hidden_from_line, move_entry, order_from_json, order_to_json, palette_row_ids,
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
    // Every section of the busiest converted context, plus a section that
    // does not apply (Figure with an effect-line preset armed).
    for (tool, mode, secs) in [
        (Tool::Text, FigureMode::Line, 9usize),
        (Tool::Figure, FigureMode::Stream, 5),
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
            ids.contains(&"figure.opts"),
            "the effect-line knobs are still there: {ids:?}"
        );
    }
}
