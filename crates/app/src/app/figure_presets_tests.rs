//! Lane A3 of the effect-lines parity plan: the artist's OWN effect-line
//! sub tools — Duplicate / Rename / Save current / Update / Delete, and the
//! `figure_presets=` line of `ui.txt` that carries them across a restart.
//!
//! Owner's ask, 2026-09-06: "i should be able to make a new subtool from
//! default that does this by varying settings ... from right click duplicate
//! subtool and rename or something".
//!
//! What these drive is the COMMAND layer (`crate::cmd::dispatch`) plus the
//! two pure helpers the palette calls, which is the whole body of every
//! right-click menu item. What they cannot reach is the menu widget itself:
//! `ui::subtool` is a private module inside `ui.rs` (lane A2's open question
//! 1 says the same thing about the row click), so what is NOT covered here
//! is exactly which button is wired to which command — the effect of every
//! one of them is.

use super::{App, headless_renderer};
use super::layout::UiLayout;
use crate::cmd::{
    AppCmd, FigureLineOpts, FigureMode, LineKind, Tool, UserLinePreset, builtin_presets, dispatch,
    held_line_opts, unique_preset_name, user_presets_from_json, user_presets_to_json,
};

fn app() -> Option<App> {
    let renderer = headless_renderer()?;
    let mut app = App::new(renderer, (400, 400), 1.0);
    app.viewport = mn_gpu::Viewport::default();
    app.tool = Tool::Figure;
    // `App::new` reads the ui.txt beside the TEST EXE, which the parallel
    // runner shares — a developer's own saved sub tools must not decide
    // whether these pass.
    app.figure_presets.clear();
    Some(app)
}

/// The shipped row with this name, priced for the app's page.
fn builtin(app: &App, name: &str) -> (LineKind, FigureLineOpts) {
    let p = builtin_presets()
        .iter()
        .find(|p| p.name == name)
        .expect("a shipped preset by that name");
    (p.kind, (p.opts)(app.tone_dpi()))
}

/// What the palette's Duplicate item does, minus the widget.
fn duplicate(app: &mut App, name: &str, kind: LineKind, opts: FigureLineOpts, mine: bool) {
    dispatch(
        app,
        AppCmd::FigurePresetAdd {
            name: if mine {
                name.to_owned()
            } else {
                format!("{name} copy")
            },
            kind,
            opts,
        },
    );
}

/// A round trip through the REAL save→load path: a command writes the list,
/// the list becomes the `figure_presets=` line, `to_body` writes ui.txt's
/// body, `from_body` reads it back (the seam the layout tests use so a run
/// never touches the ui.txt beside the test exe), and `App::new`'s own
/// seeding expression decodes it.
///
/// This is the plan's acceptance minus the mouse: duplicate a builtin,
/// rename it, retune it, Update from current, "restart".
#[test]
fn user_preset_round_trips_through_ui_txt() {
    let Some(mut app) = app() else { return };
    let (kind, opts) = builtin(&app, "Saturated line");

    duplicate(&mut app, "Saturated line", kind, opts, false);
    dispatch(
        &mut app,
        AppCmd::FigurePresetRename {
            from: "Saturated line copy".into(),
            to: "Ref 08 wedges".into(),
        },
    );
    // Retune: heavier accents than any shipped row, so "did the tuned
    // number survive?" cannot accidentally be answered by the builtin.
    crate::cmd::arm_line_preset(&mut app, kind, opts);
    app.figure_focus.accent_frac = 0.77;
    let tuned = held_line_opts(&app, kind);
    dispatch(
        &mut app,
        AppCmd::FigurePresetUpdate {
            name: "Ref 08 wedges".into(),
            opts: tuned,
        },
    );

    assert_eq!(app.figure_presets.len(), 1);
    assert_eq!(app.figure_presets[0].name, "Ref 08 wedges");
    assert_eq!(app.figure_presets[0].opts.accent_frac, 0.77);

    // Every one of those four commands must have reached the layout, or the
    // row is gone at the next start — the "it worked until I closed the app"
    // bug this line exists to prevent.
    assert!(
        app.layout.figure_presets.contains("Ref 08 wedges"),
        "the rename reached the layout line: {:?}",
        app.layout.figure_presets
    );
    let body = app.layout.to_body();
    assert!(
        body.contains("\nfigure_presets=[{"),
        "one JSON line, key named: {body}"
    );
    assert_eq!(
        body.lines().filter(|l| l.starts_with("figure_presets=")).count(),
        1,
        "exactly one line, never a wrapped value"
    );

    let back = UiLayout::from_body(&body);
    let reloaded = user_presets_from_json(&back.figure_presets);
    assert_eq!(
        reloaded, app.figure_presets,
        "the whole row survives a restart — name, kind and every knob"
    );
    assert_eq!(reloaded[0].kind, LineKind::Focus);
    assert_eq!(reloaded[0].opts, tuned);

    // And it arms exactly like a builtin: the same fn the palette row calls.
    let mut fresh = app;
    fresh.figure_mode = FigureMode::Line;
    fresh.figure_focus = FigureLineOpts::default();
    crate::cmd::arm_line_preset(&mut fresh, reloaded[0].kind, reloaded[0].opts);
    assert_eq!(fresh.figure_mode, FigureMode::Focus);
    assert!(
        held_line_opts(&fresh, LineKind::Focus).same_as(&reloaded[0].opts),
        "so the row would highlight as armed"
    );
}

/// Duplicate on a SHIPPED row makes a Mine row carrying that row's numbers;
/// duplicating a Mine row numbers it instead of colliding. The two spellings
/// the plan asks for, both out of `unique_preset_name`.
#[test]
fn duplicate_of_a_builtin_lands_in_mine() {
    let Some(mut app) = app() else { return };
    let (kind, opts) = builtin(&app, "Dark burst");

    duplicate(&mut app, "Dark burst", kind, opts, false);
    assert_eq!(app.figure_presets.len(), 1);
    assert_eq!(app.figure_presets[0].name, "Dark burst copy");
    assert_eq!(app.figure_presets[0].kind, kind);
    assert_eq!(
        app.figure_presets[0].opts, opts,
        "a duplicate carries the row's own knobs, not the armed ones"
    );
    // The shipped list is untouched — a Mine row is an addition, never a
    // shadow that hides the original.
    assert!(builtin_presets().iter().any(|p| p.name == "Dark burst"));

    // Duplicate the copy: "<name> 2", per the plan.
    duplicate(&mut app, "Dark burst copy", kind, opts, true);
    assert_eq!(app.figure_presets.len(), 2);
    assert_eq!(app.figure_presets[1].name, "Dark burst copy 2");
    duplicate(&mut app, "Dark burst copy", kind, opts, true);
    assert_eq!(app.figure_presets[2].name, "Dark burst copy 3");

    // Duplicating the SHIPPED row again cannot silently overwrite the first
    // copy either.
    duplicate(&mut app, "Dark burst", kind, opts, false);
    assert_eq!(app.figure_presets[3].name, "Dark burst copy 4");

    // Names are unique across the whole list, not per group: the palette
    // addresses a row by name and two rows called the same thing would make
    // Delete a coin toss.
    let mut names: Vec<&str> = app.figure_presets.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    let n = names.len();
    names.dedup();
    assert_eq!(names.len(), n, "no two rows share a name");

    // An empty (or all-space) name is refused rather than making a row
    // nobody can click.
    assert_eq!(unique_preset_name(&app.figure_presets, "   "), None);
    dispatch(
        &mut app,
        AppCmd::FigurePresetAdd {
            name: "  ".into(),
            kind,
            opts,
        },
    );
    assert_eq!(app.figure_presets.len(), 4, "nothing was added");
    // ...and the same rule guards Rename, which is the one that could
    // otherwise erase a working row's name.
    dispatch(
        &mut app,
        AppCmd::FigurePresetRename {
            from: "Dark burst copy".into(),
            to: "".into(),
        },
    );
    assert_eq!(app.figure_presets[0].name, "Dark burst copy");
}

/// "Save current settings as sub tool" captures the knobs AS TUNED, not the
/// shipped numbers of the row it was invoked on. This is the owner's actual
/// gesture — vary the settings, then keep them.
#[test]
fn save_current_captures_the_tuned_knobs() {
    let Some(mut app) = app() else { return };
    let (kind, shipped) = builtin(&app, "Stream line");
    crate::cmd::arm_line_preset(&mut app, kind, shipped);

    // Tune three knobs across both halves of the panel (Figure + Wobble).
    app.figure_stream.gap_px = shipped.gap_px * 2.5;
    app.figure_stream.accent_mul = 6.0;
    app.figure_stream.jit_len = 0.9;
    let tuned = held_line_opts(&app, kind);
    assert!(!tuned.same_as(&shipped), "the set really did move");

    dispatch(
        &mut app,
        AppCmd::FigurePresetAdd {
            name: "Stream line tuned".into(),
            kind,
            opts: tuned,
        },
    );

    let row = &app.figure_presets[0];
    assert_eq!(row.name, "Stream line tuned");
    assert_eq!(row.kind, LineKind::Stream);
    assert_eq!(row.opts, tuned, "the whole struct, field for field");
    assert_ne!(row.opts.gap_px, shipped.gap_px);
    assert_eq!(row.opts.accent_mul, 6.0);
    assert_eq!(row.opts.jit_len, 0.9);

    // Saving does not disturb what is in hand: you are still drawing with
    // the knobs you just saved.
    assert_eq!(held_line_opts(&app, kind), tuned);
    assert_eq!(app.figure_mode, FigureMode::Stream);

    // Update from current keeps the ROW's reroll seed rather than adopting
    // the tool's, so two clicks on the row still place two different sets.
    let seed_before = app.figure_presets[0].opts.seed;
    let mut later = tuned;
    later.seed = seed_before.wrapping_add(4242);
    later.taper = 0.11;
    dispatch(
        &mut app,
        AppCmd::FigurePresetUpdate {
            name: "Stream line tuned".into(),
            opts: later,
        },
    );
    assert_eq!(app.figure_presets[0].opts.taper, 0.11, "the edit landed");
    assert_eq!(
        app.figure_presets[0].opts.seed, seed_before,
        "the seed is a reroll counter, not a parameter"
    );

    // A verb aimed at a name nobody has is a no-op, not a panic and not a
    // new row: commands sit in the queue for a frame, and the row can be
    // gone by the time one runs.
    dispatch(
        &mut app,
        AppCmd::FigurePresetUpdate {
            name: "not a row".into(),
            opts: shipped,
        },
    );
    dispatch(&mut app, AppCmd::FigurePresetDelete("not a row".into()));
    dispatch(
        &mut app,
        AppCmd::FigurePresetRename {
            from: "not a row".into(),
            to: "hello".into(),
        },
    );
    assert_eq!(app.figure_presets.len(), 1);
    assert_eq!(app.figure_presets[0].name, "Stream line tuned");
}

/// Delete removes the way BACK to a set of numbers, never the numbers. The
/// deleted row may well be the one you are drawing with — a Figure tool that
/// silently re-armed itself from somewhere else would be a lost drawing.
#[test]
fn deleting_the_armed_preset_keeps_the_knobs() {
    let Some(mut app) = app() else { return };
    let (kind, opts) = builtin(&app, "Sea urchin flash");
    duplicate(&mut app, "Sea urchin flash", kind, opts, false);
    let mine = app.figure_presets[0].clone();

    // Arm the Mine row exactly as its palette click would.
    crate::cmd::arm_line_preset(&mut app, mine.kind, mine.opts);
    let armed_mode = app.figure_mode;
    let armed_focus = app.figure_focus;
    let armed_stream = app.figure_stream;
    assert_eq!(armed_mode, FigureMode::Urchin);
    assert!(held_line_opts(&app, kind).same_as(&mine.opts));

    dispatch(&mut app, AppCmd::FigurePresetDelete(mine.name.clone()));

    assert!(app.figure_presets.is_empty(), "the row is gone");
    assert_eq!(app.figure_mode, armed_mode, "still the same generator");
    assert_eq!(
        app.figure_focus, armed_focus,
        "and still the same knobs — every one of them"
    );
    assert_eq!(app.figure_stream, armed_stream, "the other holder too");

    // The empty list is a REAL state and has to persist as one, or the next
    // start would resurrect the deleted row from a stale line.
    let back = UiLayout::from_body(&app.layout.to_body());
    assert_eq!(back.figure_presets, "[]");
    assert!(user_presets_from_json(&back.figure_presets).is_empty());
}

/// A Mine row stores canvas PIXELS plus the dpi it was priced at, so it has
/// to restate itself on another page the way the shipped rows do — otherwise
/// a 0.2 mm width saved on a 600 dpi B4 draws twice as wide on a 300 dpi
/// draft, right next to a `Stream line` row that halved itself correctly.
///
/// Exactly three fields are pixels (`width`, `gap_px`, `start_back`);
/// degrees, fractions, multiples and counts mean the same thing at any
/// resolution and must not move.
#[test]
fn mine_rows_reprice_to_the_page_dpi() {
    let Some(mut app) = app() else { return };
    assert_eq!(app.tone_dpi(), 600, "a bare canvas is the manga standard");

    let (kind, at600) = builtin(&app, "Saturated line");
    duplicate(&mut app, "Saturated line", kind, at600, false);
    assert_eq!(app.figure_presets[0].dpi, 600, "saved at the page's dpi");
    assert_eq!(app.figure_presets[0].opts, at600);

    // Open a page at half the resolution.
    app.doc.dpi = Some(300);
    assert_eq!(app.tone_dpi(), 300);

    let row = app.figure_presets[0].clone();
    let rep = row.repriced(app.tone_dpi());
    assert!(
        (rep.width - at600.width * 0.5).abs() < 1e-4,
        "the width halves: {} → {}",
        at600.width,
        rep.width
    );
    assert_eq!(
        rep.gap_deg, at600.gap_deg,
        "but the angular gap does NOT — scaling it would rotate the burst"
    );
    assert_eq!(rep.count, at600.count);
    assert_eq!(rep.group, at600.group);
    assert_eq!(rep.group_gap, at600.group_gap);
    assert_eq!(rep.taper, at600.taper);
    assert_eq!(rep.jit_len, at600.jit_len);
    assert_eq!(rep.accent_mul, at600.accent_mul);
    assert_eq!(rep.sweep_deg, at600.sweep_deg);

    // The same, for every shipped preset at 2× — and this form is the one
    // that catches a NEW px field being added to `LineOpts` without being
    // added to `repriced`, because it pins the whole struct at once.
    for p in builtin_presets() {
        let o = (p.opts)(600);
        let row = UserLinePreset {
            name: p.name.into(),
            kind: p.kind,
            opts: o,
            dpi: 600,
        };
        let rep = row.repriced(1200);
        assert_eq!(
            rep,
            FigureLineOpts {
                width: (o.width * 2.0).max(0.5),
                gap_px: o.gap_px * 2.0,
                start_back: o.start_back * 2.0,
                ..o
            },
            "{}: exactly the three pixel fields scale, nothing else",
            p.name
        );
        // And the same dpi is a no-op, so a row that never leaves its page
        // cannot drift through a float multiply.
        assert_eq!(row.repriced(600), o, "{}", p.name);
        // A missing or hand-edited 0 passes through rather than guessing.
        assert_eq!(UserLinePreset { dpi: 0, ..row.clone() }.repriced(1200), o);
        assert_eq!(row.repriced(0), o, "{}", p.name);
    }

    // Arming: the row hands the palette its RE-PRICED knobs, so the click
    // arms those and the highlight lights on the page it was armed on. The
    // stored 600 dpi numbers would light nothing here.
    crate::cmd::arm_line_preset(&mut app, kind, rep);
    assert!(held_line_opts(&app, kind).same_as(&rep), "the row is lit");
    assert!(
        !held_line_opts(&app, kind).same_as(&at600),
        "and it is NOT the stored numbers that light it"
    );

    // Duplicating a Mine row on this page stores the re-priced values under
    // THIS page's dpi, so the copy is already correct here and re-prices
    // from the right base anywhere else.
    duplicate(&mut app, &row.name, kind, rep, true);
    let copy = app.figure_presets[1].clone();
    assert_eq!(copy.dpi, 300);
    assert_eq!(copy.opts, rep);
    assert_eq!(copy.repriced(300), rep, "no second re-pricing at home");
    assert!(
        (copy.repriced(600).width - at600.width).abs() < 1e-3,
        "and back at 600 it is the width it started as"
    );

    // Update from current moves the row's dpi with the knobs — leaving the
    // old one would re-price the row a second time on the very page it was
    // just updated from.
    dispatch(
        &mut app,
        AppCmd::FigurePresetUpdate {
            name: row.name.clone(),
            opts: rep,
        },
    );
    assert_eq!(app.figure_presets[0].dpi, 300);
    assert_eq!(app.figure_presets[0].repriced(300), rep);
}

/// The line is user-editable text in a file that also holds the window
/// position and every palette width. A row this build cannot read costs the
/// ROWS and nothing else — never the rest of ui.txt, and never a panic.
#[test]
fn garbage_figure_presets_line_loads_empty() {
    for junk in [
        "",
        "   ",
        "not json at all",
        "[{",
        "{}",
        // The right shape, one field short: `UserLinePreset` has no
        // `#[serde(default)]`, so a row without a kind is not a row.
        r#"[{"name":"half a row"}]"#,
        // A kind this build does not have (a newer one's).
        r#"[{"name":"x","kind":"Lightning","opts":{}}]"#,
    ] {
        assert!(
            user_presets_from_json(junk).is_empty(),
            "junk must degrade to an empty list, not a panic: {junk:?}"
        );
    }

    // The rest of the file is untouched by a bad line, and the key survives
    // a round trip when it IS readable.
    let bad = UiLayout::from_body("left_w=222\nfigure_presets=not json at all\nright_w=333\n");
    assert_eq!(bad.left_w, 222.0);
    assert_eq!(bad.right_w, 333.0);
    assert!(user_presets_from_json(&bad.figure_presets).is_empty());

    // A row with UNKNOWN extra fields (a newer build's) still loads: serde
    // ignores what it does not know, so a downgrade keeps the artist's rows
    // instead of eating them.
    let forward = r#"[{"name":"from tomorrow","kind":"Stream","opts":{"width":3.5},"halo":9}]"#;
    let rows = user_presets_from_json(forward);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "from tomorrow");
    assert_eq!(rows[0].opts.width, 3.5);
    assert_eq!(
        rows[0].opts.taper, 0.0,
        "and the fields it did not carry take LineOpts' own defaults"
    );
    // The same clause covers the rows written in the hour between this
    // lane's first commit and the `dpi` field: no key ⇒ 600, the manga
    // standard and `tone_dpi`'s own fallback, so an old row re-prices as
    // what it almost certainly is instead of dropping the whole line.
    assert_eq!(rows[0].dpi, 600, "a row with no dpi is a 600 dpi row");

    // The encoder is the decoder's inverse for a real list.
    let list = vec![UserLinePreset {
        name: "Ref 08 wedges".into(),
        kind: LineKind::Solid,
        opts: FigureLineOpts {
            width: 7.5,
            accent_frac: 0.4,
            ..FigureLineOpts::default()
        },
        dpi: 1200,
    }];
    let json = user_presets_to_json(&list);
    assert!(!json.contains('\n'), "one line: {json}");
    assert_eq!(user_presets_from_json(&json), list);
}
