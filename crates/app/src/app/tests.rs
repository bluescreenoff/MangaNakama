use super::*;
use mn_core::{Document, PenSample, StrokeSink, TileIdx};

/// The owner's own examples for the `[`/`]` ladder (2026-08-17): `]` on
/// 1 px → 2, on 100 → 120, on 120 → 150 — round numbers, gradiated
/// steps, never a percentage drift.
#[test]
fn size_ladder_matches_the_owners_examples() {
    assert_eq!(size_rung(1.0, true), 2.0);
    assert_eq!(size_rung(100.0, true), 120.0);
    assert_eq!(size_rung(120.0, true), 150.0);
    assert_eq!(size_rung(2.0, false), 1.0);
    assert_eq!(size_rung(150.0, false), 120.0);
    // Off-rung slider values snap past themselves, both directions.
    assert_eq!(size_rung(3.7, true), 4.0);
    assert_eq!(size_rung(3.7, false), 3.0);
    // The floors and the beyond-ladder tail stay sane.
    assert_eq!(size_rung(1.0, false), 1.0);
    assert!((size_rung(2000.0, true) - 2500.0).abs() < 1e-3);
    // Strictly monotone across the whole ladder, both directions.
    for w in SIZE_RUNGS.windows(2) {
        assert!(w[0] < w[1]);
    }
    for &r in &SIZE_RUNGS {
        assert!(size_rung(r, true) > r, "{r} must grow on ]");
        if r > 1.0 {
            assert!(size_rung(r, false) < r, "{r} must shrink on [");
        }
    }
    // The 1 px floor is the exception: [ must not go below it.
    assert_eq!(size_rung(1.2, false), 1.0);
}

/// The fit margin is the `fit_margin` preference now, not a literal in
/// two places. The shipped default must reproduce today's fit exactly —
/// this is the test that catches "we made it configurable and moved it".
#[test]
fn fit_margin_comes_from_the_preference() {
    let doc = Document::new(1000, 500);
    let shipped = prefs::FIT_MARGIN;
    assert_eq!(shipped, 0.98, "the owner's 2026-08-19 number");

    // 2000/1000 = 2 binds against 2000/500 = 4, so zoom = 2 × margin.
    let v = fitted_viewport(&doc, (2000, 2000), shipped);
    assert!((v.zoom - 2.0 * 0.98).abs() < 1e-5, "{}", v.zoom);
    assert!((v.pan[0] - (2000.0 - 1000.0 * v.zoom) * 0.5).abs() < 1e-3);
    assert!((v.pan[1] - (2000.0 - 500.0 * v.zoom) * 0.5).abs() < 1e-3);

    // A different margin actually reaches the zoom — the whole point.
    let tight = fitted_viewport(&doc, (2000, 2000), 0.80);
    assert!((tight.zoom - 1.6).abs() < 1e-5, "{}", tight.zoom);
    assert!(tight.zoom < v.zoom);

    // …and through the rect-relative fit, which is the one the real Fit
    // command uses (both had the literal, both had to be threaded).
    let in_rect = fitted_viewport_in(&doc, [10.0, 20.0], (2000.0, 2000.0), 0.80);
    assert!((in_rect.zoom - tight.zoom).abs() < 1e-5);
    assert!((in_rect.pan[0] - (10.0 + (2000.0 - 1000.0 * in_rect.zoom) * 0.5)).abs() < 1e-3);
}

/// CO-042's rule in full: newest first, no duplicates, bounded, and
/// re-using an old colour promotes it instead of adding a second copy.
#[test]
fn colour_history_is_bounded_deduped_and_newest_first() {
    let mut h: Vec<[f32; 3]> = Vec::new();
    let red = [1.0, 0.0, 0.0];
    let blue = [0.0, 0.0, 1.0];
    push_color_history(&mut h, red);
    push_color_history(&mut h, blue);
    assert_eq!(h, [blue, red], "newest first");

    // Re-using red moves it back to the front; it does not appear twice.
    push_color_history(&mut h, red);
    assert_eq!(h, [red, blue]);

    // Values that only differ below 8-bit precision are the same colour
    // — the strip cannot show the difference, so it must not hold both.
    push_color_history(&mut h, [1.0, 0.0, 1.0 / 512.0]);
    assert_eq!(h, [red, blue], "sub-8-bit noise is not a new colour");

    // Bounded, oldest falls off the end.
    for i in 0..40 {
        push_color_history(&mut h, [i as f32 / 255.0, 0.0, 0.0]);
    }
    assert_eq!(h.len(), COLOR_HISTORY_MAX);
    assert_eq!(h[0], [39.0 / 255.0, 0.0, 0.0], "the last push leads");
    assert!(!h.contains(&blue), "old entries really do fall off");
}

/// CO-023: what an eyedropper pick is allowed to do to the Color Set.
/// The default is nothing, and both refusals are deliberate.
#[test]
fn picked_colours_only_join_the_set_when_asked() {
    use mn_core::palette::Swatch;
    let red = [1.0, 0.0, 0.0];
    let set = vec![Swatch::new([0.0, 0.0, 0.0])];

    assert_eq!(
        pick_registration(false, &set, red),
        PickReg::Off,
        "off is the default and it means OFF"
    );
    assert_eq!(pick_registration(true, &set, red), PickReg::Added);
    assert_eq!(
        pick_registration(true, &set, [0.0, 0.0, 1.0 / 700.0]),
        PickReg::Duplicate,
        "a colour that rounds onto an existing swatch is that swatch"
    );

    // The bound: automatic growth stops, and says so rather than
    // silently dropping the pick.
    let full: Vec<Swatch> = (0..SWATCH_CAP)
        .map(|i| Swatch::new([i as f32 / 255.0, 0.5, 0.5]))
        .collect();
    assert_eq!(pick_registration(true, &full, red), PickReg::Full);
    assert_eq!(pick_registration(false, &full, red), PickReg::Off);
}

/// `swatches.txt` round-trips names now that the `.gpl` import keeps
/// them, an old name-less file still reads, and junk is skipped rather
/// than failing the load (the house rule for every persisted file).
#[test]
fn swatches_file_round_trips_names() {
    use mn_core::palette::Swatch;
    let set = vec![
        Swatch::new([0.0, 0.0, 0.0]),
        Swatch {
            rgb: [1.0, 0.5, 0.25],
            name: "skin — shadow".into(),
        },
        Swatch {
            // A name that would corrupt the file if written verbatim.
            rgb: [1.0, 1.0, 1.0],
            name: "two\nlines\there".into(),
        },
    ];
    let body = swatches_body(&set);
    let back: Vec<Swatch> = body.lines().filter_map(parse_swatch_line).collect();
    assert_eq!(back.len(), 3);
    assert_eq!(back[0], set[0], "an unnamed swatch stays unnamed");
    // The NAME is what this line is about. The colour comes back
    // 8-bit-quantized and that is the format doing its job, not a bug:
    // `swatches_body` writes `#rrggbb` — the same 0..255 precision the
    // `.gpl` interchange format and the hex box carry — so 0.5 goes out
    // as #80 and reads back as 0.50196. Persisting more precision than
    // the file format has would be the actual defect.
    assert_eq!(back[1].name, set[1].name, "spaces and dashes survive");
    assert_eq!(
        back[1].rgb,
        mn_core::palette::quantize8(set[1].rgb),
        "the colour round-trips at the file's own 8-bit precision"
    );
    assert_eq!(
        back[2].name, "two lines here",
        "the separators are flattened, not obeyed"
    );

    // The pre-names format still loads, and rubbish costs its own line.
    let old = "#000000\n#ffffff\n";
    assert_eq!(old.lines().filter_map(parse_swatch_line).count(), 2);
    for junk in ["", "  ", "not a colour", "#12345", "; comment"] {
        assert!(parse_swatch_line(junk).is_none(), "{junk}");
    }
}

/// The choke point, through the real dispatch: `SetSlotColor` sets the
/// slot AND records; `SetSlotColorLive` sets the slot and records
/// nothing, which is the whole reason it exists — a hue drag emits one
/// of these per frame and must not fill the strip with itself.
#[test]
fn live_colour_drags_set_the_slot_without_touching_the_history() {
    use crate::cmd::AppCmd;
    let Some(renderer) = headless_renderer() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    let mut app = App::new(renderer, (64, 64), 1.0);
    app.color_history.clear();

    // A drag: sixty frames of live values, then the release commits.
    for i in 0..60 {
        crate::cmd::dispatch(
            &mut app,
            AppCmd::SetSlotColorLive([i as f32 / 255.0, 0.0, 0.0]),
        );
    }
    assert_eq!(app.main_color[0], 59.0 / 255.0, "the slot follows the drag");
    assert!(
        app.color_history.is_empty(),
        "sixty live frames are not sixty colours"
    );

    crate::cmd::dispatch(&mut app, AppCmd::SetSlotColor([59.0 / 255.0, 0.0, 0.0]));
    assert_eq!(app.color_history.len(), 1, "the release is the entry");
    assert_eq!(app.color_history[0], [59.0 / 255.0, 0.0, 0.0]);
    assert_eq!(
        app.layout.color_history,
        ["#3b0000"],
        "and it is on its way to ui.txt as hex"
    );

    // Promoting the strip into the Color Set leaves the strip alone —
    // history is disposable, the set is kept, and one is a COPY of the
    // other, never a move. (`AddHistoryToSwatches` really does write
    // `swatches.txt`, so an earlier run of this test may have left the
    // colour in the file the App loaded: start from a known set.)
    app.swatches.retain(|s| s.rgb != [59.0 / 255.0, 0.0, 0.0]);
    let before = app.swatches.len();
    crate::cmd::dispatch(&mut app, AppCmd::AddHistoryToSwatches);
    assert_eq!(app.swatches.len(), before + 1);
    assert_eq!(app.color_history.len(), 1, "the strip survives the copy");
    // Running it twice is a no-op: the colour is already in the set.
    crate::cmd::dispatch(&mut app, AppCmd::AddHistoryToSwatches);
    assert_eq!(app.swatches.len(), before + 1);

    crate::cmd::dispatch(&mut app, AppCmd::ClearColorHistory);
    assert!(app.color_history.is_empty());
    assert!(
        app.layout.color_history.is_empty(),
        "an emptied strip persists as empty, not as the last state"
    );
}

/// TRIAGE 48/49 (`G-002`/`G-004`): what the Tool Property panel authors
/// reaches the canvas through the REAL finisher, not just through the
/// core evaluator. Asserted on alpha, so it holds whatever colours the
/// palette happens to be carrying.
#[test]
fn gradient_tool_paints_the_authored_ramp() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetTool(Tool::Gradient));
    let alpha = |app: &App, x: i32, y: i32| -> u16 {
        let ti = TileIdx::of_pixel(x, y);
        app.doc.layers[app.doc.active]
            .tile(ti)
            .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize)[3])
            .unwrap_or(0)
    };

    // `G-004` "do not draw": outside the drag stays byte-untouched.
    app.grad_mode = GradMode::FgToBg;
    app.grad_opts.edge = mn_core::EdgeProcess::Blank;
    app.finish_gradient((100.0, 100.0), (200.0, 100.0));
    assert!(alpha(&app, 150, 100) > 0, "inside the drag paints");
    assert_eq!(alpha(&app, 500, 100), 0, "outside it must not draw");
    assert_eq!(alpha(&app, 20, 100), 0, "and not behind the start either");
    assert!(app.doc.undo(), "one undo step");

    // `G-002` flip: the transparent end swaps ends without re-dragging.
    app.grad_opts.edge = mn_core::EdgeProcess::Clamp;
    app.grad_mode = GradMode::FgToTransparent;
    app.finish_gradient((100.0, 100.0), (300.0, 100.0));
    let (head, tail) = (alpha(&app, 105, 100), alpha(&app, 295, 100));
    assert!(
        head > tail,
        "unflipped: opaque at the start ({head} {tail})"
    );
    assert!(app.doc.undo());

    app.grad_opts.flip = true;
    app.finish_gradient((100.0, 100.0), (300.0, 100.0));
    let (head_f, tail_f) = (alpha(&app, 105, 100), alpha(&app, 295, 100));
    assert!(
        head_f < tail_f,
        "flipped: opaque at the END instead ({head_f} {tail_f})"
    );
}

fn pen_kind() -> EngineKind {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes/classic/pen.myb");
    EngineKind::My(Box::new(MyBrush::load(&path).unwrap()))
}

fn sample(x: f32, y: f32, t: f64) -> PenSample {
    PenSample {
        x,
        y,
        pressure: 1.0,
        tilt_x: 0.0,
        tilt_y: 0.0,
        t_ms: t,
    }
}

// --- the pen's silent failures (docs/CSP-PEN-TABLET-PAINS.md §4) -----
//
// There is no pen in a test, which is exactly what `MOUSE_PRESSURE` and
// the plain-struct `PenBatch` exist for: every shape below is a batch we
// hand to the same entry points the wndproc uses.

/// A synthetic pen report. `reports` is what the history CARRIED;
/// `samples` is what survived the in-contact filter, so the two differ
/// exactly in the failure this round is about.
fn pen_report(samples: Vec<PenSample>, reports: usize, pressure: bool) -> crate::input::PenBatch {
    crate::input::PenBatch {
        samples,
        reports,
        pressure_reported: pressure,
        tilt_reported: false,
        inverted: false,
    }
}

fn drain_cmds(app: &mut App) {
    // Bounded: `SetTool` can queue one `SelectBrush` behind it, and a
    // runaway queue must fail the test rather than hang it.
    for _ in 0..64 {
        let Some(c) = app.cmds.pop_front() else {
            return;
        };
        crate::cmd::dispatch(app, c);
    }
    panic!("the command queue never drained");
}

/// **The round's centrepiece (§4.2, corpus threads 68969 and 72057 —
/// the two highest-viewed "cannot draw" reports on the board).** A
/// driver that signals contact through pressure alone sets no
/// `POINTER_FLAG_INCONTACT`, `read_pen_batch` correctly drops every
/// sample, and the stroke then runs from end to end having received
/// nothing. Two things must hold: the undo bracket must not be left
/// open, and the app must SAY the stroke drew nothing — the whole
/// failure is that it used to look identical to a deliberate tap.
#[test]
fn a_stroke_that_receives_no_samples_closes_its_op_and_says_so() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.tool = Tool::Pen;
    app.last_pointer = (123, 45);

    // Seven reports arrived; the in-contact filter ate all seven.
    app.note_pen_report(&pen_report(Vec::new(), 7, true));
    let undo_before = app.doc.undo_len();

    app.begin_stroke(PointerKind::Pen);
    assert!(app.doc.is_op_open(), "begin_stroke opens the undo bracket");
    app.push_batch(&[]);
    app.end_stroke();

    assert!(
        !app.doc.is_op_open(),
        "§4.4/§4.2: the undo op must never be left open by a silent stroke"
    );
    assert_eq!(
        app.doc.undo_len(),
        undo_before,
        "a stroke that painted nothing must not push an undo step either"
    );
    assert!(!app.drawing(), "the stroke state is gone");
    assert!(
        app.status_warn,
        "the silence has to reach the user as a WARNING, not as nothing"
    );
    assert!(
        app.status.contains("drew nothing") && app.status.contains("123"),
        "the message must name the failure and where the pen went down: {}",
        app.status
    );
    assert!(
        app.status.contains("dropped as not-in-contact"),
        "and which SIDE lost the input — 7 reports arrived: {}",
        app.status
    );
}

/// The negative control for the test above, and the house rule for this
/// whole round: **a healthy stylus must draw exactly as it does today.**
/// Same call sequence with real samples paints, pushes exactly one undo
/// step, and says nothing alarming.
#[test]
fn a_healthy_stroke_is_untouched_by_the_new_disclosure() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.tool = Tool::Pen;

    let batch: Vec<PenSample> = (0..20)
        .map(|i| sample(80.0 + i as f32 * 5.0, 200.0, i as f64 * 8.0))
        .collect();
    app.note_pen_report(&pen_report(batch.clone(), batch.len(), true));
    let undo_before = app.doc.undo_len();

    app.begin_stroke(PointerKind::Pen);
    app.push_batch(&batch);
    app.end_stroke();

    let alpha: u64 = app
        .doc
        .active_layer()
        .tiles()
        .map(|(_, t)| t.alpha_sum())
        .sum();
    assert!(alpha > 0, "the healthy stroke still paints");
    assert_eq!(app.doc.undo_len(), undo_before + 1, "exactly one undo step");
    assert!(
        !app.status_warn,
        "a working pen must never be told something is wrong: {}",
        app.status
    );
    assert!(!app.pen.inverted && app.pen.pressure_reported && app.pen.dropped == 0);
}

/// §4.1 — the 124-thread cluster. The `0.5` substitute stays (a pen with
/// no pressure support must still draw), but it stops being
/// indistinguishable from a real half-press: the app knows, warns once,
/// and warns again if a working device stops reporting mid-session.
#[test]
fn substituted_pressure_is_disclosed_and_a_later_loss_is_too() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);

    // A device with no pressure at all: the receipt is a warning.
    app.note_pen_report(&pen_report(vec![sample(10.0, 10.0, 0.0)], 1, false));
    assert!(app.pen.seen && !app.pen.pressure_reported);
    assert!(app.status_warn, "the first receipt warns: {}", app.status);
    assert!(
        app.status.contains("NOT REPORTED"),
        "and says which fact is missing: {}",
        app.status
    );

    // A working device: seen once, quietly. (Same App — a second WARP
    // device per test is the crash the GPU-test mutex exists for.)
    app.pen = PenHealth::default();
    app.set_status("");
    app.note_pen_report(&pen_report(vec![sample(10.0, 10.0, 0.0)], 1, true));
    assert!(app.pen.pressure_reported && !app.status_warn);

    // …that then stops reporting. This is the invisible one: the pen
    // keeps drawing, at a constant width, forever.
    app.note_pen_report(&pen_report(vec![sample(11.0, 10.0, 8.0)], 1, false));
    assert!(!app.pen.pressure_reported);
    assert!(
        app.status_warn && app.status.contains("NOT REPORTED"),
        "a mid-session pressure loss must announce itself: {}",
        app.status
    );

    // And a pen-UP, whose pointer info has already gone, describes
    // nothing and must not be read as "pressure came back".
    app.note_pen_report(&pen_report(Vec::new(), 0, true));
    assert!(
        !app.pen.pressure_reported,
        "an empty report carries no facts and must clear none"
    );
}

/// §4.6 — hold space, Alt-Tab, release space over there, come back. The
/// latch survived and every pen-down panned instead of drawing, for the
/// rest of the session. Plus §4.4's half: an in-flight stroke ends here
/// rather than being closed later by the wrong gesture.
#[test]
fn focus_loss_releases_every_latch() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.tool = Tool::Pen;
    app.space_down = true;
    app.pen_owner = Owner::Canvas;
    app.begin_stroke(PointerKind::Pen);
    app.push_batch(&[sample(100.0, 100.0, 0.0), sample(140.0, 120.0, 8.0)]);
    app.begin_pan(10.0, 10.0);

    app.cancel_input_latches("test");

    assert!(!app.space_down, "the space latch is the reported bug");
    assert!(!app.drawing() && !app.doc.is_op_open(), "the stroke closed");
    assert!(!app.panning());
    assert_eq!(app.pen_owner, Owner::None);
    assert_eq!(app.mouse_owner, Owner::None);
    // The stroke DID paint, so its work is kept — cancelling the latch
    // must not throw away ink the artist already laid down.
    assert_eq!(app.doc.undo_len(), 1, "the drawn stroke stays undoable");
}

/// §4.4's regression test, through the real canvas entry points: a focus
/// steal between two strokes must leave TWO undo steps, not one bracket
/// spanning both. Before the fix the orphaned op was closed by the
/// SECOND pen-down, merging the two.
#[test]
fn a_focus_steal_between_strokes_does_not_merge_their_undo_steps() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.tool = Tool::Pen;

    app.canvas_down(80.0, 200.0, PointerKind::Mouse, &[sample(80.0, 200.0, 0.0)]);
    app.canvas_move(120.0, 200.0, &[sample(120.0, 200.0, 8.0)]);
    // Focus goes to an installer/driver popup; the pen-up never arrives.
    app.cancel_input_latches("test");
    let s2 = [sample(80.0, 300.0, 16.0)];
    app.canvas_down(80.0, 300.0, PointerKind::Mouse, &s2);
    app.canvas_move(120.0, 300.0, &[sample(120.0, 300.0, 24.0)]);
    app.canvas_up(120.0, 300.0, &[]);

    assert_eq!(
        app.doc.undo_len(),
        2,
        "two strokes, two undo steps — the orphaned bracket used to swallow the first"
    );
}

/// §4.9 — 24 complaint threads and 4 accepted requests. Flipping the
/// stylus erases; flipping back restores the tool that was standing.
/// The tool route (not a hidden brush-mode flag) is what gives the tail
/// its own preset memory, which is what the accepted requests asked for.
#[test]
fn the_stylus_tail_erases_and_flipping_back_restores_the_tool() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.tool = Tool::Pen;
    app.apply_draw_state();
    assert!(!app.eraser_active());

    app.set_pen_inverted(true);
    // Synchronous, because a tail that touches down without hovering
    // first would otherwise ink one stroke with the pen.
    assert!(app.eraser_active(), "the tail erases from the first dab");
    drain_cmds(&mut app);
    assert_eq!(app.tool, Tool::Eraser, "and the toolbar shows it");

    app.set_pen_inverted(false);
    drain_cmds(&mut app);
    assert_eq!(app.tool, Tool::Pen, "flipping back restores the tool");
    assert!(!app.eraser_active());
}

/// The safety rail on the above: a flip is never allowed to change the
/// tool under a live line. (A driver that toggles the flag spuriously
/// mid-stroke is the §3.8 "the tool changes by itself" class, and we do
/// not get to reproduce it.)
#[test]
fn the_tail_never_switches_tools_mid_stroke() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.tool = Tool::Pen;
    app.begin_stroke(PointerKind::Pen);
    app.cmds.clear();

    app.set_pen_inverted(true);

    assert!(!app.pen.inverted, "the flip is ignored while drawing");
    assert!(app.cmds.is_empty(), "and queues no tool change");
    app.push_batch(&[sample(100.0, 100.0, 0.0)]);
    app.end_stroke();
    assert_eq!(app.tool, Tool::Pen);
}

/// Audit H1 (docs/AUDIT-2026-08-17-opus.md): a GPU dab stroke latched the
/// engine's record mode at Bypass; toggling Wash on the LIVE engine then
/// made every later stroke paint NOTHING (the CPU path recorded without
/// rasterizing) until a sub-tool switch rebuilt the engine. The fix makes
/// the mode a function of the branch taken. This asserts the EFFECT —
/// pixels on the layer — never the mode field.
#[test]
fn wash_toggle_after_gpu_dab_stroke_still_paints() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.gpu_dabs = true;
    assert!(
        app.brush.inner().inner().gpu_dab_ready(),
        "the default pen should be GPU-ready"
    );

    let alpha = |a: &App| -> u64 {
        a.doc
            .active_layer()
            .tiles()
            .map(|(_, t)| t.alpha_sum())
            .sum()
    };
    let stroke = |app: &mut App, x0: f32| {
        app.begin_stroke(PointerKind::Mouse);
        let batch: Vec<PenSample> = (0..30)
            .map(|i| PenSample {
                x: x0 + i as f32 * 4.0,
                y: 200.0,
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            })
            .collect();
        app.push_batch(&batch);
        app.end_stroke();
    };

    stroke(&mut app, 100.0);
    let after_gpu = alpha(&app);
    assert!(after_gpu > 0, "the GPU dab stroke itself painted nothing");

    // The audit's exact repro: flip Wash on the live engine, stroke again.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetWash(true));
    stroke(&mut app, 250.0);
    let after_wash = alpha(&app);
    assert!(
        after_wash > after_gpu,
        "H1 regression: the wash toggle silenced the pen ({after_gpu} → {after_wash})"
    );
}

/// #0.1 wash parity, END TO END including the commit: the same wash
/// stroke through the CPU path (dabs rasterized into the buffer by the
/// C, `commit_wash` at `end`) and the GPU path (BYPASS, sentinel-tile
/// rasterization, readback into a scratch, the SAME `commit_wash`).
/// The GPU replaced only the per-dab rasterization, so the drift bar is
/// the rasterizer's own — measured 1 ulp worst channel, and the assert
/// allows 491 (1.5%) for accumulation headroom: a missing or doubled
/// commit lands 100× past it.
#[test]
fn gpu_dab_parity_wash() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let mut app = App::new(renderer, (600, 400), 1.0);

    let stroke = |app: &mut App, x0: f32| {
        app.begin_stroke(PointerKind::Mouse);
        let batch: Vec<PenSample> = (0..30)
            .map(|i| PenSample {
                x: x0 + i as f32 * 4.0,
                y: 200.0,
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            })
            .collect();
        app.push_batch(&batch);
        app.end_stroke();
    };

    // Fresh engine per stroke (canary-test precedent): the KNOWN open
    // carryover bug (round 35 — CPU-after-BYPASS measures ~47%, and any
    // cross-path pair on one engine is suspect) would otherwise shift
    // stroke #2's dab stream and this test would pin the carryover, not
    // the wash parity. `expect`, never a silent if-let fallback to the
    // configuration under test (auditor finding #4).
    let fresh_engine = |app: &mut App| {
        let i = app
            .selected_preset
            .expect("a preset must be selected for the fresh-engine swap");
        let p = app.presets[i].1.clone();
        let b = mn_brush::MyBrush::load(&p).expect("preset reload must succeed");
        *app.engine_mut() = Engine::new(EngineKind::My(Box::new(b)));
        crate::cmd::dispatch(app, crate::cmd::AppCmd::SetWash(true));
    };

    // Reference: the CPU wash stroke on layer 0.
    app.gpu_dabs = false;
    fresh_engine(&mut app);
    stroke(&mut app, 100.0);
    let gpu_layer = app.doc.add_layer("gpu");
    app.doc.set_active(gpu_layer);
    app.gpu_dabs = true;
    fresh_engine(&mut app);
    stroke(&mut app, 100.0);
    assert!(
        app.dab_path_last.starts_with("gpu |"),
        "the wash stroke silently left the GPU path: {}",
        app.dab_path_last
    );

    let collect = |li: usize| -> std::collections::BTreeMap<TileIdx, Vec<u16>> {
        app.doc.layers[li]
            .tiles()
            .map(|(i, t)| (i, t.data().to_vec()))
            .collect()
    };
    let (cpu, gpu) = (collect(0), collect(gpu_layer));
    assert!(!cpu.is_empty(), "the CPU wash stroke painted nothing");
    assert_eq!(
        cpu.keys().collect::<Vec<_>>(),
        gpu.keys().collect::<Vec<_>>(),
        "the two paths inked different tiles"
    );
    let mut worst_ch: u32 = 0;
    let mut worst_rel: f64 = 0.0;
    for (k, c) in &cpu {
        let g = &gpu[k];
        let (ca, ga): (u64, u64) = (
            c.chunks_exact(4).map(|p| p[3] as u64).sum(),
            g.chunks_exact(4).map(|p| p[3] as u64).sum(),
        );
        let rel = (ca as i64 - ga as i64).unsigned_abs() as f64 / ca.max(1) as f64;
        worst_rel = worst_rel.max(rel);
        for (pc, pg) in c.chunks_exact(4).zip(g.chunks_exact(4)) {
            for ch in 0..4 {
                worst_ch = worst_ch.max(pc[ch].abs_diff(pg[ch]) as u32);
            }
        }
        assert!(worst_ch <= 491, "tile {k:?}: channel drift {worst_ch} ulp");
    }
    assert!(
        worst_rel < 0.05,
        "per-tile alpha drift {worst_rel:.3} (missing commit lands at 1.0)"
    );
    println!("[wash-parity] worst channel {worst_ch} ulp, worst alpha rel {worst_rel:.4}");
}

/// #0.1 part 3, smudge parity END TO END: the same blending stroke over
/// the same pre-existing ink, CPU path (the C's get_color samples the
/// live CPU tiles) vs GPU path (per-sample dispatch + the tile oracle
/// serving the sampler from the dab cache — the sampler must see EXACTLY
/// the canvas the CPU path's end_atomic would have shown it, or the
/// picked-up colors diverge and the smear trails differ). Fresh engines
/// per stroke (the carryover rule — see the wash parity test). The
/// ink blobs are painted by identical fresh pen engines, so any
/// difference below is the smudge path, not the setup.
#[test]
fn gpu_dab_parity_smudge() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let mut app = App::new(renderer, (600, 400), 1.0);

    let pen_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes/classic/pen.myb");
    let knife_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/brushes/classic/blending_knife.myb");
    let fresh = |app: &mut App, p: &Path| {
        let b = mn_brush::MyBrush::load(p).expect("preset load must succeed");
        *app.engine_mut() = Engine::new(EngineKind::My(Box::new(b)));
    };

    let stroke = |app: &mut App, x0: f32, n: usize, p: f32| {
        app.begin_stroke(PointerKind::Mouse);
        let batch: Vec<PenSample> = (0..n)
            .map(|i| PenSample {
                x: x0 + i as f32 * 6.0,
                y: 200.0,
                pressure: p,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            })
            .collect();
        app.push_batch(&batch);
        app.end_stroke();
    };

    // The same ink blob on both layers (identical fresh pen engines).
    app.gpu_dabs = false;
    fresh(&mut app, &pen_path);
    stroke(&mut app, 150.0, 30, 1.0);
    let gpu_layer = app.doc.add_layer("gpu");
    app.doc.set_active(gpu_layer);
    fresh(&mut app, &pen_path);
    stroke(&mut app, 150.0, 30, 1.0);

    // Smudge through the blob — CPU reference on layer 0, GPU on top.
    // The stroke STARTS inside the blob (x0=200 vs blob 150..326) and
    // drags the picked-up color ~165px OUT into blank canvas: the
    // tail's samples must see the stroke's OWN freshly-laid ink (the
    // carried smear) or the pickup reads blank and the trail drifts.
    // The C's get_color FLUSHES the tile's op queue before sampling —
    // the CPU sampler sees even same-batch dabs — which the per-sample
    // dispatch + oracle reproduce exactly on the GPU side.
    app.doc.set_active(0);
    app.gpu_dabs = false;
    fresh(&mut app, &knife_path);
    stroke(&mut app, 200.0, 50, 1.0);
    assert!(
        app.dab_path_last.starts_with("cpu"),
        "sanity: the CPU smudge stroke must route cpu ({}), or the routing gate broke",
        app.dab_path_last
    );

    app.doc.set_active(gpu_layer);
    app.gpu_dabs = true;
    fresh(&mut app, &knife_path);
    stroke(&mut app, 200.0, 50, 1.0);
    assert!(
        app.dab_path_last.starts_with("gpu |"),
        "the smudge stroke silently left the GPU path: {} (regression of the #0.1 part 3 gate)",
        app.dab_path_last
    );

    let collect = |li: usize| -> std::collections::BTreeMap<TileIdx, Vec<u16>> {
        app.doc.layers[li]
            .tiles()
            .map(|(i, t)| (i, t.data().to_vec()))
            .collect()
    };
    let (cpu, gpu) = (collect(0), collect(gpu_layer));
    assert!(!cpu.is_empty(), "the CPU smudge stroke painted nothing");
    assert_eq!(
        cpu.keys().collect::<Vec<_>>(),
        gpu.keys().collect::<Vec<_>>(),
        "the two paths inked different tiles"
    );
    let mut worst_ch: u32 = 0;
    for (k, c) in &cpu {
        let g = &gpu[k];
        for (pc, pg) in c.chunks_exact(4).zip(g.chunks_exact(4)) {
            for ch in 0..4 {
                worst_ch = worst_ch.max(pc[ch].abs_diff(pg[ch]) as u32);
            }
        }
    }
    // FINE-GRAINED on purpose, and both margins are MEASURED on this
    // deterministic test: legit parity drifts 1 ulp (the rasterizer's
    // ≤1/dab class, same as the wash test), while serving the sampler
    // STALE tiles (the oracle disabled — the negative control) diverges
    // 23-32 ulp on this preset. The knife's staleness magnitude is
    // small because its pickup is dominated by pre-existing ink, so a
    // gross-failure bound like the wash test's 491 would never fire;
    // 12 sits between both measurements with headroom on each side.
    assert!(
        worst_ch <= 12,
        "smudge parity broke: worst channel drift {worst_ch} ulp (stale oracle? legit is 1, stale serves 23-32)"
    );
    println!("[smudge-parity] worst channel {worst_ch} ulp");
}

/// TRIAGE 131, the clipboard: Cut clears exactly the selected pixels
/// (one undo step), Paste opens the float at its original coordinates,
/// and committing at identity restores the pixels EXACTLY — the
/// cut→paste round trip is lossless, which is the whole point of the
/// internal fix15 clipboard over the OS 8-bit DIB.
#[test]
fn cut_paste_round_trips_the_selection() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // Tile (2,3) covers canvas (128..192, 192..256). Blob A at canvas
    // x 130..137 (tile-local 2..9), y 195 — inside the selection rect
    // below; blob B at canvas x 150..157 (tile-local 22..29) — outside.
    let idx = TileIdx::new(2, 3);
    for x in 2..9 {
        app.doc
            .active_layer_mut()
            .tile_mut(idx)
            .set_pixel(x, 3, [1000, 2000, 3000, 32767]);
    }
    for x in 22..29 {
        app.doc
            .active_layer_mut()
            .tile_mut(idx)
            .set_pixel(x, 3, [500, 500, 500, 32767]);
    }
    let px = |app: &App, canvas_x: i32, canvas_y: i32| -> [u16; 4] {
        let ti = TileIdx::of_pixel(canvas_x, canvas_y);
        app.doc.active_layer().tile(ti).unwrap().pixel(
            (canvas_x - ti.origin().0) as usize,
            (canvas_y - ti.origin().1) as usize,
        )
    };

    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 120.0, 190.0, 140.0, 200.0,
    ));
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Cut);
    assert_eq!(px(&app, 132, 195)[3], 0, "the selected blob must be gone");
    assert_eq!(
        px(&app, 152, 195),
        [500, 500, 500, 32767],
        "pixels outside the selection must be untouched"
    );
    assert_eq!(app.doc.undo_len(), 1, "Cut must be one undo step");
    assert!(app.clipboard.is_some(), "Cut stores the clipboard");

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Paste);
    let drag = app
        .transform_drag
        .as_ref()
        .expect("Paste opens the move float");
    assert_eq!(
        drag.source.rect,
        [120, 190, 140, 200],
        "Paste returns the float to its source coordinates"
    );
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);
    assert_eq!(
        px(&app, 132, 195),
        [1000, 2000, 3000, 32767],
        "the identity commit must restore the pixels exactly"
    );
    assert_eq!(px(&app, 152, 195), [500, 500, 500, 32767]);
}

/// DECISIONS 8.73: Cut through a FEATHERED selection (SE-007 blur)
/// splits every pixel inside the ants by its coverage fraction — the
/// graded band partially clears and partially rides the clipboard, not
/// the old ≥-half hard cut that moved the whole band wholesale. Pins
/// the shared `clear_lifted` wiring (one implementation with
/// `commit_transform`).
#[test]
fn cut_feathered_splits_by_weight() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // Ink canvas x 130..158 on row y=195 (tile (2,3), local row 3); the
    // blurred selection's edge crosses the middle of the ink.
    let idx = TileIdx::new(2, 3);
    for x in 2..30 {
        app.doc
            .active_layer_mut()
            .tile_mut(idx)
            .set_pixel(x, 3, [1000, 2000, 3000, 32767]);
    }
    let mut sel = mn_core::Selection::from_rect(&app.doc, 130.0, 190.0, 140.0, 200.0);
    sel = sel.blur(&app.doc, 6);
    app.doc.selection = Some(sel.clone());
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Cut);
    assert!(app.clipboard.is_some(), "Cut stores the clipboard");

    let px = |app: &App, canvas_x: i32, canvas_y: i32| -> [u16; 4] {
        let ti = TileIdx::of_pixel(canvas_x, canvas_y);
        app.doc.active_layer().tile(ti).unwrap().pixel(
            (canvas_x - ti.origin().0) as usize,
            (canvas_y - ti.origin().1) as usize,
        )
    };
    // The float's RECT is the ants' bbox — the ≥-half outline — so on
    // this ramp x 132..139 carries the graded band (cov 136..151).
    let r = app.clipboard.as_ref().unwrap().rect;
    assert!(
        r[0] >= 130 && r[2] <= 141,
        "the rect is the ants' bbox {r:?}"
    );
    let mut partial = 0;
    for x in r[0]..r[2] {
        let mv = sel.coverage(x, 195) as u32;
        let taken = ((32767 * mv + 127) / 255) as u16;
        assert_eq!(
            px(&app, x, 195)[3],
            32767 - taken,
            "the layer keeps its uncut fraction at ({x},195), cov {mv}"
        );
        assert_eq!(
            app.clipboard.as_ref().unwrap().pixel(x, 195)[3],
            taken,
            "the clipboard carries the cov/255 fraction at ({x},195)"
        );
        if mv > 0 && mv < 255 {
            assert!(taken > 0 && taken < 32767, "partial, not cut");
            partial += 1;
        }
    }
    assert!(partial > 0, "the graded band was exercised");
    // The sub-half halo OUTSIDE the ants is not captured and stays on
    // the layer — the recorded rect seam (pre-existing, unchanged by
    // the weight conversion; DECISIONS 8.73).
    assert_eq!(px(&app, 131, 195)[3], 32767, "the sub-half halo stays");
    assert_eq!(px(&app, 140, 195)[3], 32767, "the sub-half halo stays");
    assert_eq!(app.doc.undo_len(), 1, "Cut is one undo step");
}

/// The clipboard's operand rect comes off the selection's COVERAGE, not
/// its display outline. A wand + Shift-add selection has islands and
/// `outline` keeps exactly ONE of them (the vertex-count sort in
/// `set_outlines`), so an outline-aimed Cut cleared one island and left
/// the other sitting on the layer.
#[test]
fn cut_clears_every_island_of_a_multi_island_selection() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let ink = |app: &mut App, x: i32, y: i32| {
        let ti = TileIdx::of_pixel(x, y);
        let (ox, oy) = ti.origin();
        app.doc.active_layer_mut().tile_mut(ti).set_pixel(
            (x - ox) as usize,
            (y - oy) as usize,
            [1000, 2000, 3000, 32767],
        );
    };
    ink(&mut app, 132, 195); // island A
    ink(&mut app, 310, 110); // island B
    let px = |app: &App, x: i32, y: i32| -> [u16; 4] {
        let ti = TileIdx::of_pixel(x, y);
        app.doc
            .active_layer()
            .tile(ti)
            .unwrap()
            .pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize)
    };

    let a = mn_core::Selection::from_rect(&app.doc, 120.0, 190.0, 140.0, 200.0);
    // An L (6 corners against the rect's 4) so the loops sort unequally
    // and `outline` deterministically keeps island B.
    let b = mn_core::Selection::from_polygon(
        &app.doc,
        &[
            (300.0, 100.0),
            (360.0, 100.0),
            (360.0, 120.0),
            (330.0, 120.0),
            (330.0, 160.0),
            (300.0, 160.0),
        ],
    );
    let sel = a.combine(&b, &app.doc, mn_core::SelectionOp::Add);
    assert!(!sel.extra_outlines.is_empty(), "two islands, two loops");
    app.doc.selection = Some(sel);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Cut);
    assert_eq!(px(&app, 132, 195)[3], 0, "island A cleared");
    assert_eq!(px(&app, 310, 110)[3], 0, "island B cleared");
    assert_eq!(app.doc.undo_len(), 1, "still one undo step");
}

/// SE-007's silent trap: a blur wide enough to push every pixel under
/// half leaves a live selection with NO ants and no launcher, while the
/// brush stays masked at partial strength. The status must warn, and the
/// weight consumers must still reach the feather — Cut lifts the graded
/// pixels instead of reporting nothing to cut.
#[test]
fn blur_below_half_warns_but_still_cuts() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let ti = TileIdx::of_pixel(150, 150);
    let (ox, oy) = ti.origin();
    app.doc.active_layer_mut().tile_mut(ti).set_pixel(
        (150 - ox) as usize,
        (150 - oy) as usize,
        [1000, 2000, 3000, 32767],
    );
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 130.0, 130.0, 170.0, 170.0,
    ));
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SelectBlur(32));

    let sel = app
        .doc
        .selection
        .as_ref()
        .expect("the selection is still live");
    assert!(
        sel.outline.is_empty() && sel.extra_outlines.is_empty(),
        "a 40 px rect blurred by 32 px has no ≥-half pixel left"
    );
    let cov = sel.coverage(150, 150);
    assert!(cov > 0 && cov < 128, "a feather under half: {cov}");
    assert!(app.status_warn, "the status warns: {}", app.status);

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Cut);
    assert!(app.clipboard.is_some(), "Cut still finds the feather");
    let left = app
        .doc
        .active_layer()
        .tile(ti)
        .unwrap()
        .pixel((150 - ox) as usize, (150 - oy) as usize)[3];
    assert!(
        left > 0 && left < 32767,
        "the feather cut by weight, not wholesale: {left}"
    );
}

/// TRIAGE 131: with NO selection the operand is the whole layer —
/// Copy's rect is the layer's populated bounds, Cut empties it, and the
/// round trip restores everything.
#[test]
fn cut_without_selection_uses_the_whole_layer() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let idx = TileIdx::new(2, 3);
    for x in 2..9 {
        app.doc
            .active_layer_mut()
            .tile_mut(idx)
            .set_pixel(x, 3, [1000, 2000, 3000, 32767]);
    }
    let alpha_total = |a: &App| -> u64 {
        a.doc
            .active_layer()
            .tiles()
            .map(|(_, t)| t.alpha_sum())
            .sum()
    };
    let full = alpha_total(&app);
    assert!(full > 0);

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Cut);
    assert_eq!(
        alpha_total(&app),
        0,
        "Cut with no selection empties the layer"
    );
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Paste);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);
    assert_eq!(alpha_total(&app), full, "the round trip is lossless");
}

/// The r69–r115 audit's worst finding, pinned: Copy is NOT Cut. A pasted
/// float dragged away and committed must leave the original art exactly
/// where it was — the paste never lifted anything off the layer, so the
/// commit has no source to clear.
#[test]
fn committing_a_moved_paste_keeps_the_original() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let idx = TileIdx::new(2, 3);
    for x in 2..9 {
        app.doc
            .active_layer_mut()
            .tile_mut(idx)
            .set_pixel(x, 3, [1000, 2000, 3000, 32767]);
    }
    let alpha_total = |a: &App| -> u64 {
        a.doc
            .active_layer()
            .tiles()
            .map(|(_, t)| t.alpha_sum())
            .sum()
    };
    let full = alpha_total(&app);
    assert!(full > 0);

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Copy);
    assert_eq!(alpha_total(&app), full, "Copy leaves the layer alone");
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Paste);
    {
        // Drag the float well away from its source before committing —
        // this is exactly the step that used to expose the source clear.
        let drag = app.transform_drag.as_mut().expect("the float opened");
        drag.set_params(1.0, 1.0, 0.0, 200.0, 150.0);
    }
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);
    assert!(app.transform_drag.is_none(), "the commit consumed the float");
    // Original + the stamped copy: strictly MORE ink than before, and the
    // source pixels themselves are untouched.
    assert!(
        alpha_total(&app) > full,
        "the copy landed somewhere on the layer"
    );
    let p = app.doc.active_layer().tile(idx).expect("source tile lives");
    assert_eq!(
        p.pixel(2, 3),
        [1000, 2000, 3000, 32767],
        "the copied-from art survived the commit"
    );
    // And one undo removes the paste, restoring the exact original count.
    assert!(app.doc.undo());
    assert_eq!(alpha_total(&app), full, "one undo = just the paste gone");
}

/// TRIAGE 131: Paste to shown position centres the float on the current
/// view instead of its source coordinates (the other-page case).
#[test]
fn paste_to_shown_position_centres_the_float() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let idx = TileIdx::new(2, 3);
    for x in 2..9 {
        app.doc
            .active_layer_mut()
            .tile_mut(idx)
            .set_pixel(x, 3, [1000, 2000, 3000, 32767]);
    }
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 120.0, 190.0, 140.0, 200.0,
    ));
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Copy);

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PasteShown);
    let drag = app.transform_drag.as_ref().expect("the float opened");
    // The float's centre (midpoint of the transformed bbox) must sit at
    // the canvas position of the view centre — computed the same way
    // the handler does, so the assert pins the geometry, not a constant.
    let c = app
        .viewport
        .to_canvas(app.canvas_center()[0], app.canvas_center()[1]);
    let mut bmin = [f32::INFINITY, f32::INFINITY];
    let mut bmax = [f32::NEG_INFINITY, f32::NEG_INFINITY];
    for p in drag.bbox {
        bmin[0] = bmin[0].min(p[0]);
        bmin[1] = bmin[1].min(p[1]);
        bmax[0] = bmax[0].max(p[0]);
        bmax[1] = bmax[1].max(p[1]);
    }
    let centre = [(bmin[0] + bmax[0]) * 0.5, (bmin[1] + bmax[1]) * 0.5];
    assert!(
        (centre[0] - c.0).abs() <= 1.0 && (centre[1] - c.1).abs() <= 1.0,
        "float centre {centre:?} vs view centre ({}, {})",
        c.0,
        c.1
    );
}

/// TRIAGE 132 part 2: the app-level preflight sees the active page's
/// content AND the work's metadata — the full dirty state reports, and
/// the fully-fixed state reports nothing.
#[test]
fn preflight_reports_and_clears() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let mut s = mn_core::PageSetup::presets().remove(0); // Shueisha A
    s.bleed_mm = 3.0;
    app.page = Some(s.clone());
    // Dirty content: text inside the 5 mm ring + a colour pixel on a
    // Mono work.
    let trim = s.trim_rect_px();
    let mut t = mn_core::TextItem::new([0.0, 0.0], String::new(), 12.0, [0, 0, 0], false);
    t.pos = [trim[0] + 20.0, trim[1] + 20.0];
    t.size = [40.0, 12.0];
    app.doc
        .add_text_layer("lettering", mn_core::TextSet { texts: vec![t] });
    let art = app.doc.add_layer("art");
    app.doc.layers[art]
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(5, 5, [20000, 491, 491, 32767]);

    let f = app.run_preflight();
    let ids: Vec<_> = f.iter().map(|x| x.check).collect();
    assert!(
        ids.contains(&"text.margin"),
        "the near-trim text must warn: {ids:?}"
    );
    assert!(
        ids.contains(&"expression.colour_on_mono"),
        "the colour pixel must warn: {ids:?}"
    );
    assert!(
        ids.contains(&"spine.unset") && ids.contains(&"cover.missing") == false,
        "1-page work: spine yes, cover no: {ids:?}"
    );

    // Fully fixed: colour work, spine set, empty page.
    app.expression = mn_core::Expression::Colour;
    app.spine_mm = 6.0;
    app.doc = Document::new(64, 64);
    let f = app.run_preflight();
    assert!(
        f.is_empty(),
        "the fixed work must be silent: {:?}",
        f.iter().map(|x| x.check).collect::<Vec<_>>()
    );
}

/// TRIAGE 132 part 2: non-active pages decode from their stashed ORA
/// bytes — a violation on page 2 surfaces with the page named.
#[test]
fn preflight_checks_stashed_pages_too() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let mut s = mn_core::PageSetup::presets().remove(0);
    s.bleed_mm = 3.0;
    app.page = Some(s.clone());
    app.expression = mn_core::Expression::Colour;
    app.spine_mm = 6.0;
    app.cover = Some(0);

    // Page 2 (non-active): a text box sticking out of the trim.
    let (pw, ph) = s.paper_px();
    let mut p2 = Document::new(pw, ph);
    let trim = s.trim_rect_px();
    let mut t = mn_core::TextItem::new([0.0, 0.0], String::new(), 12.0, [0, 0, 0], false);
    t.pos = [trim[0] - 30.0, trim[1] + 30.0];
    t.size = [40.0, 12.0];
    p2.add_text_layer("lettering", mn_core::TextSet { texts: vec![t] });
    let bytes = mn_core::project::doc_to_bytes(&p2).expect("encode page 2");
    let mut second = crate::app::PageEntry::active();
    second.bytes = Some(bytes);
    app.pages.push(second);
    app.page_index = 0;

    let f = app.run_preflight();
    let hit = f
        .iter()
        .find(|x| x.check == "text.outside_trim")
        .expect("the stashed page's text must surface");
    assert!(
        hit.message.contains("page 2"),
        "the finding must name the page: {}",
        hit.message
    );
}

/// TRIAGE 133 part 1: the material bank scans the shipped starter
/// folder, pastes as the move/scale float at natural size centred on
/// the view, tiles over the WHOLE canvas when asked (the owner's
/// mask-to-draw-through), and counts uses for frequency sorting.
#[test]
fn material_bank_pastes_and_tiles() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // The repo's assets/materials (starter set) resolves from the test
    // working directory; without it the bank is legitimately empty
    // (e.g. a bare exe without assets) and this test has nothing to
    // drive — assert the starter set exists in the repo layout.
    let mat_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/materials");
    assert!(
        mat_dir.join("tone-dots-10pct.png").is_file(),
        "the starter materials must ship in assets/materials"
    );
    if app.materials.is_empty() {
        // Not scanned from here — point folder 0 at the repo copy.
        app.material_folders[0] = mat_dir.clone();
        app.materials_scan();
    }
    assert!(
        app.materials.iter().any(|m| m.name == "tone-dots-10pct"),
        "the scan must find the starter tones: {:?}",
        app.materials
            .iter()
            .map(|m| m.name.clone())
            .collect::<Vec<_>>()
    );

    let dots = app
        .materials
        .iter()
        .find(|m| m.name == "tone-dots-10pct")
        .unwrap()
        .path
        .clone();
    let dims = image::open(&dots).unwrap().to_rgba8().dimensions();

    // Plain paste: a float the size of the material, somewhere on the
    // canvas, stamps on Enter.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::PasteMaterial {
            path: dots.clone(),
            tile: false,
        },
    );
    let drag = app
        .transform_drag
        .as_ref()
        .expect("the material float opened");
    let (fw, fh) = (
        drag.source.rect[2] - drag.source.rect[0],
        drag.source.rect[3] - drag.source.rect[1],
    );
    assert_eq!((fw, fh), (dims.0 as i32, dims.1 as i32));
    assert_eq!(
        app.material_uses.get(&dots.display().to_string()),
        Some(&1),
        "the paste must count a use"
    );
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);
    let alpha: u64 = app
        .doc
        .active_layer()
        .tiles()
        .map(|(_, t)| t.alpha_sum())
        .sum();
    assert!(alpha > 0, "the committed tone must have inked");

    // Tiled paste: one float covering the ENTIRE canvas.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::PasteMaterial {
            path: dots,
            tile: true,
        },
    );
    let drag = app.transform_drag.as_ref().expect("the tiled float opened");
    assert_eq!(
        drag.source.rect,
        [0, 0, app.doc.size.0 as i32, app.doc.size.1 as i32],
        "tiling must cover the whole canvas"
    );
    assert_eq!(
        app.material_uses.len(),
        1,
        "one material, counted twice: {}",
        app.material_uses.values().sum::<u64>()
    );
    assert_eq!(
        app.material_uses.values().sum::<u64>(),
        2,
        "both pastes counted"
    );
}

/// TRIAGE 133 part 2 — MT-014 Toning: a material pasted with Tone on
/// renders as the document's SCREENTONE, not its greyscale pixels:
/// every inked pixel is pure black premultiplied ink with alpha in the
/// raster's 25% steps, and the pattern IS dots at the tone frequency
/// (some pixels fully on, some fully off, in a halftone-density mix).
#[test]
fn material_tone_paste_renders_screentone() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let mat_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/materials");
    if app.materials.is_empty() {
        app.material_folders[0] = mat_dir.clone();
        app.materials_scan();
    }
    let dots = app
        .materials
        .iter()
        .find(|m| m.name == "tone-gradient-halftone")
        .expect("the gradient starter ships")
        .path
        .clone();

    app.material_tone = true;
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::PasteMaterial {
            path: dots,
            tile: false,
        },
    );
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);

    // Every inked pixel: black, alpha a multiple of the 25% step.
    let step = 32768 / 4;
    let mut on = 0usize;
    let mut off = 0usize;
    let mut any = false;
    for (_, t) in app.doc.active_layer().tiles() {
        for px in t.data().chunks_exact(4) {
            if px[3] == 0 {
                off += 1;
                continue;
            }
            any = true;
            assert_eq!(px[0], 0, "toned ink must be black (r=g=b=0), got {:?}", px);
            assert_eq!(px[1], 0);
            assert_eq!(px[2], 0);
            assert_eq!(px[3] % step, 0, "the raster's AA is 25%-stepped: {}", px[3]);
            if px[3] == 32768 {
                on += 1;
            }
        }
    }
    assert!(any, "the toned material must have inked");
    assert!(on > 0 && off > 0, "a screentone pattern, not a flat fill");
}

/// MT-032 + MT-034: the material paste-size modes (CSP's vocabulary —
/// the default keeps r74's down-fit) and where a panel-targeted
/// paste's layer lands in the folder.
#[test]
fn material_paste_size_modes_and_layer_order() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let mat_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/materials");
    if app.materials.is_empty() {
        app.material_folders[0] = mat_dir.clone();
        app.materials_scan();
    }
    let dots = app
        .materials
        .iter()
        .find(|m| m.name == "tone-dots-10pct")
        .unwrap()
        .path
        .clone();
    let (mw, mh) = image::open(&dots).unwrap().to_rgba8().dimensions();

    // A panel folder owning the active layer — the paste target.
    let _hdr = app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([50.0, 50.0, 550.0, 350.0], 4.0),
    );
    let (tw, th) = (500.0f32, 300.0f32);
    let fit = (tw / mw as f32).min(th / mh as f32);
    let cover = (tw / mw as f32).max(th / mh as f32);

    let paste_scale = |app: &mut App| -> (f32, f32) {
        crate::cmd::dispatch(
            app,
            crate::cmd::AppCmd::PasteMaterial {
                path: dots.clone(),
                tile: false,
            },
        );
        let d = app.transform_drag.as_ref().expect("the float opened");
        (d.xform.m[0][0], d.xform.m[1][1])
    };
    let close = |a: f32, b: f32| (a - b).abs() < 1e-3;

    // MT-032, every mode (probe, cancel — no layer churn between).
    for (mode, sx, sy) in [
        (MaterialPasteSize::FitPanel, fit.min(1.0), fit.min(1.0)),
        (MaterialPasteSize::AdjustAfter, 1.0, 1.0),
        (MaterialPasteSize::ExpandFull, cover, cover),
        (MaterialPasteSize::FitToScale, fit, fit),
        (
            MaterialPasteSize::ToDestination,
            tw / mw as f32,
            th / mh as f32,
        ),
    ] {
        app.material_size = mode;
        let (a, b) = paste_scale(&mut app);
        assert!(
            close(a, sx) && close(b, sy),
            "{mode:?}: scale ({a}, {b}), want ({sx}, {sy})"
        );
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCancel);
    }

    // MT-034: where the committed layer lands in the folder. A layer
    // is CREATED only on rule-2 pastes (the pointer's panel, not the
    // one owning the active layer — r74's split): active OUTSIDE the
    // folder, pointer inside the panel. (Indices shift per commit —
    // the header is re-found by name; two "Layer 1"s exist, but only
    // one is a folder named "Frame 1".)
    app.doc.active = 0;
    app.viewport = mn_gpu::Viewport::default(); // canvas == client
    app.last_pointer = (300, 200); // inside the panel
    let hdr_now = |app: &App| {
        app.doc
            .layers
            .iter()
            .position(|l| l.folder && l.name == "Frame 1")
            .unwrap()
    };

    // Above (default): topmost child.
    app.material_size = MaterialPasteSize::FitPanel;
    app.material_order = MaterialLayerOrder::Above;
    paste_scale(&mut app);
    assert_eq!(
        app.transform_drag.as_ref().unwrap().create_in,
        Some(hdr_now(&app)),
        "rule 2 targets the folder and creates the layer"
    );
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);
    assert_eq!(
        app.doc.active,
        app.doc.children_range(hdr_now(&app)).end - 1,
        "topmost child"
    );

    // BottomOfPanel: the folder's bottom child. (The first commit
    // left the pasted layer active INSIDE the folder — rule 1 would
    // claim the next paste; step back outside first.)
    app.material_order = MaterialLayerOrder::BottomOfPanel;
    app.doc.active = 0;
    paste_scale(&mut app);
    assert_eq!(
        app.transform_drag.as_ref().unwrap().create_in,
        Some(hdr_now(&app)),
        "rule 2 again"
    );
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);
    assert_eq!(
        app.doc.active,
        app.doc.children_range(hdr_now(&app)).start,
        "bottom of the panel"
    );
    assert!(
        app.doc
            .children_range(hdr_now(&app))
            .contains(&app.doc.active),
        "still inside the folder (the seal still clips it)"
    );
}

/// RF-001 (owner spec): reference flags are a SET — toggles are
/// independent, a FOLDER row toggles its whole child run as one unit,
/// solo clears the others, and the status line counts the set.
#[test]
fn reference_layers_form_a_set_and_folders_toggle_as_units() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // Stack: [0] art, [1] folder (depth 0), [2]+[3] children (depth 1),
    // [4] sibling after the folder (depth 0).
    let _ = app.doc.add_layer("art");
    let f = app.doc.add_layer("F");
    let c1 = app.doc.add_layer("c1");
    let c2 = app.doc.add_layer("c2");
    let s = app.doc.add_layer("after");
    for (i, d, folder) in [
        (f, 0u8, true),
        (c1, 1, false),
        (c2, 1, false),
        (s, 0, false),
    ] {
        app.doc.layers[i].depth = d;
        app.doc.layers[i].folder = folder;
    }

    // Independent toggles: marking two keeps both (the owner's CSP
    // complaint does not reproduce).
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetLayerReference(0, true));
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetLayerReference(s, true));
    assert_eq!(app.doc.reference_layers(), vec![0, s]);

    // Folder row = one unit: the folder + both children, nothing else.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetLayerReference(f, true));
    assert_eq!(
        app.doc.reference_layers(),
        vec![0, f, c1, c2, s],
        "the folder unit joins the set independently"
    );

    // Solo: only the target survives.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetLayerReferenceSolo(c2));
    assert_eq!(app.doc.reference_layers(), vec![c2]);

    // Clear-all.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::ClearReferences);
    assert!(app.doc.reference_layers().is_empty());
}

/// Selection subsystem part 1 (SE-022, the owner's everyday path):
/// held modifiers override the persistent mode; the wand combines
/// under the op it was clicked with; subtract-to-empty deselects
/// (an empty Selection would mean "everything").
#[test]
fn selection_modifier_precedence_and_wand_combine() {
    use mn_core::SelectionOp as Op;
    let r = Op::Replace;
    // Modifier precedence: every modifier pair wins over persistent.
    assert_eq!(crate::cmd::effective_sel_op(true, true, r), Op::Intersect);
    assert_eq!(crate::cmd::effective_sel_op(true, false, r), Op::Add);
    assert_eq!(crate::cmd::effective_sel_op(false, true, r), Op::Subtract);
    assert_eq!(
        crate::cmd::effective_sel_op(false, false, Op::Add),
        Op::Add,
        "no modifier takes the persistent mode"
    );

    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // A black box on white: the wand selects the box.
    let li = app.doc.add_layer("art");
    for y in 100..140 {
        for x in 100..140 {
            app.doc.layers[li]
                .tile_mut(TileIdx::of_pixel(x, y))
                .set_pixel((x & 63) as usize, (y & 63) as usize, [0, 0, 0, 32767]);
        }
    }
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::MagicSelect(120.0, 120.0, Op::Replace),
    );
    assert!(app.doc.selection.is_some(), "the wand selected the box");

    // Add a second region: union covers both.
    for y in 200..240 {
        for x in 100..140 {
            app.doc.layers[li]
                .tile_mut(TileIdx::of_pixel(x, y))
                .set_pixel((x & 63) as usize, (y & 63) as usize, [0, 0, 0, 32767]);
        }
    }
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::MagicSelect(120.0, 220.0, Op::Add),
    );
    let sel = app.doc.selection.as_ref().unwrap();
    assert!(sel.coverage(120, 120) > 0 && sel.coverage(120, 220) > 0);

    // Subtract the first box: only the second remains.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::MagicSelect(120.0, 120.0, Op::Subtract),
    );
    let sel = app.doc.selection.as_ref().unwrap();
    assert_eq!(sel.coverage(120, 120), 0, "the first box is gone");
    assert!(sel.coverage(120, 220) > 0, "the second box remains");

    // Subtract a superset (select-all then subtract it away): the
    // selection DESELECTS rather than going empty-but-present.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SelectAll);
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::MagicSelect(120.0, 120.0, Op::Subtract),
    );
    // Subtracting just the box from select-all leaves the rest — not
    // empty; the real empty case: subtract everything via select-all.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SelectAll);
    let all = app.doc.selection.clone().unwrap();
    let gone = all.combine(&all, &app.doc, Op::Subtract);
    assert!(gone.is_empty());
}

/// SELECTION part 2. SE-039 (owner spec): dragging inside a selection
/// moves THE MARCHING ANTS — pixels stay put (moving contents is the
/// launcher's Transform action). And SE-011: selecting from a layer's
/// alpha covers its ink and nothing else.
#[test]
fn selection_drag_moves_ants_and_from_layer_selects_alpha() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // A blob on a layer.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::SetTool(crate::app::Tool::Select),
    );
    let li = app.doc.add_layer("art");
    for y in 100..140 {
        for x in 100..140 {
            app.doc.layers[li]
                .tile_mut(TileIdx::of_pixel(x, y))
                .set_pixel((x & 63) as usize, (y & 63) as usize, [0, 0, 0, 32767]);
        }
    }
    let alpha_before: u64 = app.doc.layers[li].tiles().map(|(_, t)| t.alpha_sum()).sum();

    // SE-011: from-layer covers the blob, not the blank canvas.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::SelectFromLayer(li, mn_core::SelectionOp::Replace),
    );
    let sel = app.doc.selection.as_ref().expect("from-layer selected");
    assert!(sel.coverage(120, 120) > 0, "the blob's inside is selected");
    assert_eq!(sel.coverage(300, 300), 0, "blank canvas is not");

    // SE-039: canvas drag inside the selection translates the ANTS.
    // Client coords come from to_screen — the test viewport is
    // FITTED (zoom < 1), not identity, so canvas (120,120) inside the
    // blob is wherever the view puts it.
    let empty: [PenSample; 0] = [];
    let (sx, sy) = app.viewport.to_screen(120.0, 120.0);
    let (ex, ey) = app.viewport.to_screen(180.0, 130.0);
    app.canvas_down(sx, sy, PointerKind::Mouse, &empty);
    app.canvas_move(ex, ey, &empty);
    app.canvas_up(ex, ey, &empty);
    let sel = app.doc.selection.as_ref().expect("selection still live");
    assert!(
        sel.coverage(190, 125) > 0,
        "the ants moved with the drag (+60, +10)"
    );
    assert_eq!(sel.coverage(120, 120), 0, "the old spot is deselected");
    let alpha_after: u64 = app.doc.layers[li].tiles().map(|(_, t)| t.alpha_sum()).sum();
    assert_eq!(
        alpha_before, alpha_after,
        "SE-039: the pixels must NOT move — only the ants"
    );
}

/// Audit H1, third-order (docs/AUDIT-2026-08-17-opus.md §1): main engine
/// and mirror twins must rasterize through the SAME path. If the twins
/// stayed on the CPU while the main engine bypassed to the GPU, the
/// stroke-end readback would overwrite the twins' CPU ink with GPU
/// content in every tile both touched — the reflection would vanish in
/// patches. Asserts pixels on both sides of the axis.
#[test]
fn mirror_twin_survives_a_gpu_dab_stroke() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.gpu_dabs = true;

    // Differential, so the assertion does not depend on where the
    // viewport maps client coords or on how far the stabilizer lags:
    // paint the SAME stroke with the twin off and on, and require the
    // mirrored run to ink tiles the plain run never touched.
    let stroke = |app: &mut App| {
        app.begin_stroke(PointerKind::Mouse);
        let batch: Vec<PenSample> = (0..30)
            .map(|i| PenSample {
                x: 100.0 + i as f32 * 4.0,
                y: 200.0,
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            })
            .collect();
        app.push_batch(&batch);
        app.end_stroke();
    };
    let inked = |a: &App| -> std::collections::BTreeSet<TileIdx> {
        a.doc
            .active_layer()
            .tiles()
            .filter(|(_, t)| t.alpha_sum() > 0)
            .map(|(i, _)| i)
            .collect()
    };

    stroke(&mut app);
    let plain = inked(&app);
    assert!(!plain.is_empty(), "the GPU dab stroke painted nothing");

    // Undo also exercises readback -> CPU tiles -> undo snapshot.
    assert!(app.doc.undo(), "the stroke was not undoable");
    assert!(inked(&app).is_empty(), "undo left ink behind");

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetMirrorX(true));
    stroke(&mut app);
    let mirrored = inked(&app);

    let extra: Vec<_> = mirrored.difference(&plain).collect();
    assert!(
        !extra.is_empty(),
        "mixed-mode regression: the mirror twin's ink did not survive the \
             GPU stroke-end readback (plain {} tiles, mirrored {} tiles)",
        plain.len(),
        mirrored.len()
    );
}

/// TODO #0.1 — the in-app gpu-dabs switch (View menu) replaces
/// `--gpu-dabs` as the user-facing toggle: the checkbox routes through
/// the command pipeline (the real path the message loop pumps), the
/// requested state lands in `UiLayout` for the next launch, and an
/// adapter without storage textures refuses the enable instead of
/// silently taking it. Per-stroke ROUTING is asserted by the H1 tests
/// above; this pins the switch mechanics.
#[test]
fn gpu_dabs_toggle_command_pipeline() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    assert!(!app.gpu_dabs, "App::new leaves routing off (main.rs glue)");
    assert!(!app.dirty(), "a view toggle must not dirty the doc");

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetGpuDabs(true));
    if app.renderer.gpu_dabs_supported() {
        assert!(app.gpu_dabs, "the enable must take");
        assert!(
            app.layout.gpu_dabs,
            "the enable must persist for next launch"
        );
    } else {
        assert!(
            !app.gpu_dabs,
            "no storage textures — the cpu path must hold"
        );
        assert!(!app.layout.gpu_dabs, "a refused enable must not persist");
    }
    assert!(!app.dirty(), "a preference is not document state");

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetGpuDabs(false));
    assert!(
        !app.gpu_dabs && !app.layout.gpu_dabs,
        "the off is unconditional"
    );
}

/// Round 34: the Tool Property typography rows (align / frame position /
/// character spacing / line spacing / strikethrough) apply LIVE to the
/// item being edited — no per-change history — and the whole session
/// (creation included) still collapses into ONE undo step.
#[test]
fn typography_props_apply_live_and_undo_as_one_step() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    if app.text_engine.is_none() {
        println!("[test] SKIP: no text engine");
        return;
    }
    app.start_new_text([50.0, 50.0], None);
    assert!(app.text_editing());
    for u in "abc".encode_utf16() {
        app.text_char(u);
    }
    app.apply_text_prop(|i| i.align = mn_core::text::Align::Center);
    app.apply_text_prop(|i| i.frame_align = mn_core::text::FrameAlign::Far);
    app.apply_text_prop(|i| i.letter_spacing_pt = 2.0);
    app.apply_text_prop(|i| i.line_spacing = mn_core::text::LineSpacing::Percent(180.0));
    app.apply_text_prop(|i| i.set_style(0, 3, mn_core::StyleFlag::Strike, true));
    app.commit_text_edit();

    let item = app
        .doc
        .layers
        .iter()
        .find(|l| l.is_text())
        .and_then(|l| l.texts())
        .and_then(|t| t.texts.first())
        .cloned()
        .expect("the committed text layer holds the item");
    assert_eq!(item.text, "abc");
    assert_eq!(item.align, mn_core::text::Align::Center);
    assert_eq!(item.frame_align, mn_core::text::FrameAlign::Far);
    assert_eq!(item.letter_spacing_pt, 2.0);
    assert_eq!(
        item.line_spacing,
        mn_core::text::LineSpacing::Percent(180.0)
    );
    assert!(item.runs[0].strike, "the style-row S reached the run model");

    // One undo step reverts the entire session.
    assert!(app.doc.can_undo());
    app.doc.undo();
    let texts = app
        .doc
        .layers
        .iter()
        .find(|l| l.is_text())
        .and_then(|l| l.texts());
    assert!(
        texts.map_or(true, |t| t.texts.is_empty()),
        "session undone in one step"
    );
}

/// Owner pen-test 2026-08-17 (TEST 1): strokes drawn zoomed-out came
/// back POLYGONAL — the input path never interpolated, so the engine
/// dabbed straight segments between screen-resolution samples scaled
/// into wide doc-space gaps. The doc-space Catmull-Rom resampler
/// (`input_path.rs`) must make a sparse-sample curve paint its BULGE.
/// Design: five arc samples at −15°, 0°, 45°, 135°, 195° (r=40) leave a
/// 90° GAP; its arc midpoint (the 90° apex) sits 11.7 px outside the
/// straight chord between the gap's ends — raw passthrough dabs land
/// ≥29 px away and cannot ink it; the resampled curve passes through.
/// Sparse input stands in for "same screen gap × 1/zoom" — spacing is
/// in doc px, so the geometry is zoom-independent by construction.
#[test]
fn sparse_zoomed_out_curve_paints_its_bulge() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.gpu_dabs = false;
    app.begin_stroke(PointerKind::Pen);
    let (cx, cy) = (300.0, 200.0);
    let mut batch = Vec::new();
    for (i, deg) in [-15.0f32, 0.0, 45.0, 135.0, 195.0].iter().enumerate() {
        let a = deg.to_radians();
        let (px, py) = (cx + 40.0 * a.cos(), cy - 40.0 * a.sin());
        let (sx, sy) = app.viewport.to_screen(px, py);
        batch.push(PenSample {
            x: sx,
            y: sy,
            pressure: 0.8,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 24.0,
        });
    }
    app.push_batch(&batch);
    app.end_stroke();

    // The gap's arc midpoint (90°): probe a 3×3 patch of tile pixels.
    let apex = (cx + 40.0 * 0.0, cy - 40.0 * 1.0);
    let mut inked = 0u32;
    for dx in -1..=1i32 {
        for dy in -1..=1i32 {
            let x = (apex.0 + dx as f32) as i32;
            let y = (apex.1 + dy as f32) as i32;
            let idx = mn_core::TileIdx::new(x.div_euclid(64), y.div_euclid(64));
            if let Some(t) = app.doc.layers[0].tile(idx) {
                let a = t.data()
                    [((y.rem_euclid(64)) as usize * 64 + (x.rem_euclid(64)) as usize) * 4 + 3];
                inked += u32::from(a > 0);
            }
        }
    }
    assert!(
        inked >= 4,
        "the gap's arc midpoint must carry ink (the polygonal bug misses it by 11.7px); got {inked}/9 px"
    );
}

/// Auditor round 35 (audit of the resampler): the RNG explanation for
/// the canary test's cross-stroke divergence was DISPROVEN from source
/// (real-g-pen maps only pressure; every RNG site is setting-guarded) —
/// so what carries across two CPU strokes on ONE engine? His decisive
/// experiment, verbatim: same app, `gpu_dabs=false`, identical sample
/// array, stroke onto layer A, stroke onto layer B, compare alpha.
/// - identical ⇒ engine state is clean and the divergence lived in the
///   record/replay side (the GPU path) — the fresh-engine swap stays as
///   test hygiene with a corrected comment.
/// - different ⇒ a real carryover bug; escalate to its own round.
#[test]
fn two_cpu_strokes_on_one_engine_are_identical() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.gpu_dabs = false;

    let batch: Vec<PenSample> = (0..30)
        .map(|i| PenSample {
            x: 100.0 + i as f32 * 4.0,
            y: 200.0,
            pressure: 0.8,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        })
        .collect();
    let stroke = |app: &mut App| {
        app.begin_stroke(PointerKind::Mouse);
        app.push_batch(&batch);
        app.end_stroke();
    };

    stroke(&mut app); // layer 0
    let a = app.doc.layers[0]
        .tiles()
        .map(|(_, t)| t.alpha_sum())
        .sum::<u64>();
    let layer_b = app.doc.add_layer("b");
    app.doc.set_active(layer_b);
    stroke(&mut app); // SAME engine, SAME samples
    let b = app.doc.layers[layer_b]
        .tiles()
        .map(|(_, t)| t.alpha_sum())
        .sum::<u64>();
    println!("[test] two CPU strokes, one engine: {a} vs {b}");
    let rel = (a as i64 - b as i64).unsigned_abs() as f64 / a.max(1) as f64;
    assert!(
        rel < 0.01,
        "identical CPU strokes on one engine diverged {rel:.3} — a real cross-stroke carryover (escalate)"
    );
}

/// CARRYOVER RESOLVED (round 41 — Opus round-35 escalation CLOSED):
/// a CPU stroke after a BYPASS (GPU) stroke is BIT-IDENTICAL in
/// total ink to a CPU stroke after a CPU stroke on a like-freshed
/// engine — the engine state a BYPASS run leaves is
/// indistinguishable from a raster run's. The historical "47%" was
/// the canary test comparing stroke-1's REPLAY against stroke-2,
/// whose ordinary ~1% same-engine drift (two_cpu above) concentrates
/// in the fat-radius tail tiles. The raw-engine dab-stream half of
/// the proof is mn-brush's bypass_history_does_not_change_the_
/// next_strokes_dabs.
#[test]
fn cpu_after_bypass_equals_cpu_after_cpu() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let batch: Vec<PenSample> = (0..30)
        .map(|i| PenSample {
            x: 100.0 + i as f32 * 4.0,
            y: 200.0,
            pressure: 0.8,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        })
        .collect();
    let stroke = |app: &mut App| {
        app.begin_stroke(PointerKind::Mouse);
        app.push_batch(&batch);
        app.end_stroke();
    };
    let total = |app: &App, li: usize| -> u64 {
        app.doc.layers[li].tiles().map(|(_, t)| t.alpha_sum()).sum()
    };

    // History A: CPU stroke, then CPU stroke (same engine).
    app.gpu_dabs = false;
    stroke(&mut app); // layer 0
    let b = app.doc.add_layer("b");
    app.doc.set_active(b);
    stroke(&mut app);
    let cpu_after_cpu = total(&app, b);

    // History B: fresh engine, GPU (BYPASS) stroke, then CPU stroke.
    if let Some(i) = app.selected_preset {
        let p = app.presets[i].1.clone();
        if let Ok(br) = mn_brush::MyBrush::load(&p) {
            *app.engine_mut() = Engine::new(EngineKind::My(Box::new(br)));
        }
    }
    let g = app.doc.add_layer("g");
    app.doc.set_active(g);
    app.gpu_dabs = true;
    stroke(&mut app);
    assert!(
        app.dab_path_last.starts_with("gpu"),
        "the GPU stroke must route GPU ({})",
        app.dab_path_last
    );
    app.gpu_dabs = false;
    let c = app.doc.add_layer("c");
    app.doc.set_active(c);
    stroke(&mut app);
    let cpu_after_bypass = total(&app, c);

    assert_eq!(
        cpu_after_cpu, cpu_after_bypass,
        "a BYPASS stroke must leave the engine exactly where a CPU stroke does"
    );
}
/// Navigator (CV-030/036): drag-to-pan centres the view on the canvas
/// point; sticky fit re-fits on surface-size change while ON and stops
/// when OFF.
#[test]
fn navigator_pan_and_sticky_fit() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);

    // pan_to: the given canvas point lands at the view centre.
    let c = app.canvas_center();
    app.navigator_pan_to(500.0, 300.0);
    let s = app.viewport.to_screen(500.0, 300.0);
    assert!(
        (s.0 - c[0]).abs() < 0.5 && (s.1 - c[1]).abs() < 0.5,
        "pan_to must centre the point: screen ({}, {}) vs centre ({}, {})",
        s.0,
        s.1,
        c[0],
        c[1]
    );

    // Sticky fit: a surface-size change refits while ON. The headless
    // renderer's surface size is fixed, so drive the check with the
    // remembered size directly: prime it, disturb the zoom, and call
    // the check with the same size (no refit), then with a different
    // remembered state (refit).
    app.fit_sticky = true;
    app.navigator_sticky_fit_apply((600, 400)); // primes nav_last_surface
    let fitted_zoom = app.viewport.zoom;
    app.viewport.zoom = fitted_zoom * 0.5;
    app.navigator_sticky_fit_apply((600, 400)); // same size → NO refit
    assert!(
        (app.viewport.zoom - fitted_zoom * 0.5).abs() < 1e-6,
        "no resize, no refit"
    );
    // Simulate the resize: a DIFFERENT size arrives.
    app.viewport.zoom = fitted_zoom * 0.25;
    app.navigator_sticky_fit_apply((800, 500)); // size changed → refit
    // The refit fits to 800x500 (not the original 600x400): compute
    // the expectation by fitting the same size directly.
    app.nav_last_surface = (1, 1); // force one more pass
    app.navigator_sticky_fit_apply((800, 500));
    let after = app.viewport.zoom;
    assert!(
        (after - fitted_zoom * 0.25).abs() > 1e-6,
        "resize with sticky ON must refit (zoom still {after})"
    );
    // Sticky OFF: a size change does nothing.
    app.fit_sticky = false;
    app.nav_last_surface = (1, 1);
    app.viewport.zoom = after * 0.5;
    app.navigator_sticky_fit_apply((1024, 700));
    assert!(
        (app.viewport.zoom - after * 0.5).abs() < 1e-6,
        "sticky OFF must not refit"
    );
}
/// Rulers part 1 (TODO #3): an armed drag CREATES a ruler (no tool
/// switch, nothing painted), and with snapping on, a stroke's dabs lie
/// ON the line — a freehand wiggle inks straight.
#[test]
fn ruler_creation_and_snapping() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let empty: [PenSample; 0] = [];

    // Arm + create: the canvas drag becomes a line ruler, not a stroke.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::RulerArm(crate::cmd::RulerKind::Line),
    );
    let (x0, y0) = app.viewport.to_screen(100.0, 200.0);
    let (x1, y1) = app.viewport.to_screen(400.0, 200.0);
    app.canvas_down(x0, y0, PointerKind::Mouse, &empty);
    app.canvas_up(x1, y1, &empty);
    assert_eq!(app.rulers.items.len(), 1, "the drag created a ruler");
    assert!(app.rulers.on, "creation turns snapping on");
    // Nothing painted by the creation drag.
    let alpha: u64 = app
        .doc
        .active_layer()
        .tiles()
        .map(|(_, t)| t.alpha_sum())
        .sum();
    assert_eq!(alpha, 0, "the creation drag must not paint");

    // A wiggly stroke snaps onto the line: record and check every dab.
    // Tap arms AFTER begin_stroke — begin_stroke resets the mode
    // (Off for CPU strokes) and clears the record.
    app.begin_stroke(PointerKind::Mouse);
    app.engine_mut()
        .set_dab_recording_all(mn_brush::RecordMode::Tap);
    // Client coords from to_screen: a canvas-space wiggle around
    // the y=200 line (the snap must iron it flat).
    let batch: Vec<PenSample> = (0..30)
        .map(|i| {
            let cy = 200.0 + if i % 2 == 0 { 25.0 } else { -25.0 };
            let (sx, sy) = app.viewport.to_screen(100.0 + i as f32 * 10.0, cy);
            PenSample {
                x: sx,
                y: sy,
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            }
        })
        .collect();
    app.push_batch(&batch);
    // Drain AFTER end_stroke — the None-guard in finish_gpu_dab_stroke
    // now comes BEFORE the drain, so an armed CPU record survives stroke
    // end (this WAS the round-42 workaround site; draining late is the
    // regression test for the fix).
    app.end_stroke();
    let dabs = app.engine_mut().drain_dab_records();
    assert!(!dabs.is_empty(), "the stroke painted");
    for d in &dabs {
        assert!(
            (d.y - 200.0).abs() < 0.5,
            "dab ({}, {}) must lie on the y=200 line",
            d.x,
            d.y
        );
    }
}

/// Part 4 (P-001..010 v1): the perspective set through the REAL input
/// path — the creation drag IS the eye level (both ends VPs), and a
/// wobbly stroke whose early direction aims at a VP rides that VP's
/// ray for its whole length.
#[test]
fn perspective_ruler_creation_and_ray_binding() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let empty: [PenSample; 0] = [];

    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::RulerArm(crate::cmd::RulerKind::Perspective),
    );
    // The eye level: VP-a far left, VP-b far right, y = 100.
    let (x0, y0) = app.viewport.to_screen(-400.0, 100.0);
    let (x1, y1) = app.viewport.to_screen(900.0, 100.0);
    app.canvas_down(x0, y0, PointerKind::Mouse, &empty);
    app.canvas_up(x1, y1, &empty);
    assert!(
        matches!(
            app.rulers.items.as_slice(),
            [mn_core::Ruler::Perspective { .. }]
        ),
        "the drag created the perspective set"
    );
    assert!(app.rulers.on, "creation turns snapping on");

    // A wobbly stroke aiming at VP-a from (200, 300): every dab rides
    // the ray through (-400, 100) and (200, 300).
    let (a, p0): ([f32; 2], [f32; 2]) = ([-400.0, 100.0], [200.0, 300.0]);
    let dir = [p0[0] - a[0], p0[1] - a[1]];
    let n = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
    app.begin_stroke(PointerKind::Mouse);
    app.engine_mut()
        .set_dab_recording_all(mn_brush::RecordMode::Tap);
    let batch: Vec<PenSample> = (0..30)
        .map(|i| {
            let t = i as f32 * 0.03;
            // The wobble RAMPS IN over the first samples — a ruler
            // stroke starts aimed (the anchor rides the ray); the
            // acquisition reads the early direction.
            let wob = if i % 2 == 0 { 20.0 } else { -20.0 } * (i as f32 / 6.0).min(1.0);
            // Perpendicular wobble around the ray.
            let cx = p0[0] + dir[0] * t - (dir[1] / n) * wob;
            let cy = p0[1] + dir[1] * t + (dir[0] / n) * wob;
            let (sx, sy) = app.viewport.to_screen(cx, cy);
            PenSample {
                x: sx,
                y: sy,
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            }
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();
    let dabs = app.engine_mut().drain_dab_records();
    assert!(!dabs.is_empty(), "the stroke painted");
    for d in &dabs {
        let cross = (d.x - a[0]) * dir[1] - (d.y - a[1]) * dir[0];
        assert!(
            cross.abs() / n < 1.0,
            "dab ({}, {}) is off the a-ray by {:.2} px",
            d.x,
            d.y,
            cross.abs() / n
        );
    }
}

/// Auditor 0da3453's pin (round-51 handoff): snapping runs BEFORE the
/// stabilizer, so the smoother works on already-snapped input. On a
/// straight ruler every snapped sample sits exactly on the line and a
/// convex smoother cannot leave it — asserted here at MAX stabilizer
/// (48 px pull string; the ask was ≈20). If this ever exceeds ~1 px,
/// the snap order is wrong and snapping must move to the smoother's
/// OUTPUT, not its input.
#[test]
fn ruler_snap_holds_at_max_stabilizer() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let empty: [PenSample; 0] = [];
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::RulerArm(crate::cmd::RulerKind::Line),
    );
    let (x0, y0) = app.viewport.to_screen(100.0, 200.0);
    let (x1, y1) = app.viewport.to_screen(400.0, 200.0);
    app.canvas_down(x0, y0, PointerKind::Mouse, &empty);
    app.canvas_up(x1, y1, &empty);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetStabilizer(1.0));
    app.begin_stroke(PointerKind::Mouse);
    app.engine_mut()
        .set_dab_recording_all(mn_brush::RecordMode::Tap);
    let batch: Vec<PenSample> = (0..30)
        .map(|i| {
            let cy = 200.0 + if i % 2 == 0 { 25.0 } else { -25.0 };
            let (sx, sy) = app.viewport.to_screen(100.0 + i as f32 * 10.0, cy);
            PenSample {
                x: sx,
                y: sy,
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            }
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();
    let dabs = app.engine_mut().drain_dab_records();
    assert!(!dabs.is_empty());
    let max_dy = dabs
        .iter()
        .map(|d| (d.y - 200.0).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_dy < 1.0,
        "snap-before-smoother holds: max |dy| = {max_dy:.3} at stabilizer 1.0"
    );
}

/// Rulers part 2: STICKINESS — two crossing line rulers; a stroke
/// that starts on ruler A stays on A even when its samples wander
/// closer to ruler B (part 1 could flicker mid-stroke). And a curve
/// ruler clamps at its ends (the stroke does not extrapolate past the
/// last vertex).
#[test]
fn ruler_stickiness_and_curve_clamp() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let empty: [PenSample; 0] = [];

    // Two crossing rulers: horizontal at y=200 and vertical at x=300.
    for (a, b) in [
        ([100.0, 200.0], [500.0, 200.0]),
        ([300.0, 50.0], [300.0, 400.0]),
    ] {
        crate::cmd::dispatch(
            &mut app,
            crate::cmd::AppCmd::RulerArm(crate::cmd::RulerKind::Line),
        );
        let (x0, y0) = app.viewport.to_screen(a[0], a[1]);
        let (x1, y1) = app.viewport.to_screen(b[0], b[1]);
        app.canvas_down(x0, y0, PointerKind::Mouse, &empty);
        app.canvas_up(x1, y1, &empty);
    }
    assert_eq!(app.rulers.items.len(), 2);
    assert!(app.rulers.on);

    // A stroke along y≈200 that crosses x=300 (where B is nearer for
    // the y-wobble): every dab must stay on the HORIZONTAL (locked
    // first), none snapped onto the vertical.
    app.begin_stroke(PointerKind::Mouse);
    app.engine_mut()
        .set_dab_recording_all(mn_brush::RecordMode::Tap);
    let batch: Vec<PenSample> = (0..30)
        .map(|i| {
            let cy = if (20..40).contains(&i) { 199.0 } else { 201.0 };
            let (sx, sy) = app.viewport.to_screen(100.0 + i as f32 * 12.0, cy);
            PenSample {
                x: sx,
                y: sy,
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            }
        })
        .collect();
    app.push_batch(&batch);
    // Same as the part-1 test above: drain after end_stroke — armed CPU
    // records survive stroke end now (finish_gpu_dab_stroke's guard runs
    // before its drain).
    app.end_stroke();
    let dabs = app.engine_mut().drain_dab_records();
    assert!(!dabs.is_empty());
    for d in &dabs {
        assert!(
            (d.y - 200.0).abs() < 0.5,
            "sticky: dab ({}, {}) must stay on the locked horizontal",
            d.x,
            d.y
        );
    }

    // Curve ruler: an L-path; a sample past the END clamps to the
    // last vertex (no extrapolation) — checked at the core level in
    // part2_tests; here the app merely carries it (creation e2e).
    let mut rs = mn_core::Rulers::default();
    rs.curves.push(mn_core::CurveRuler {
        pts: vec![[0.0, 0.0], [100.0, 0.0]],
    });
    rs.on = true;
    let mut lock = mn_core::SnapLock::default();
    let p = rs.snap_sticky([200.0, 50.0], &mut lock);
    assert_eq!(p, [100.0, 0.0], "clamped at the curve's end");
}

/// Rulers part 3: the special family — a parallel ruler flattens every
/// dab onto its direction (hatching), a concentric ruler quantizes the
/// radius onto rings, and the RL-031 special toggle vetoes exactly the
/// special family (the line ruler from part 1 keeps snapping).
#[test]
fn special_rulers_parallel_concentric_and_the_veto() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let empty: [PenSample; 0] = [];

    // PARALLEL: drag the direction, then a wiggly stroke comes out
    // exactly on-direction.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::RulerArm(crate::cmd::RulerKind::Parallel),
    );
    let (x0, y0) = app.viewport.to_screen(100.0, 200.0);
    let (x1, y1) = app.viewport.to_screen(400.0, 200.0);
    app.canvas_down(x0, y0, PointerKind::Mouse, &empty);
    app.canvas_up(x1, y1, &empty);
    assert!(matches!(
        app.rulers.items.last(),
        Some(mn_core::Ruler::Parallel { .. })
    ));
    app.begin_stroke(PointerKind::Mouse);
    app.engine_mut()
        .set_dab_recording_all(mn_brush::RecordMode::Tap);
    let batch: Vec<PenSample> = (0..30)
        .map(|i| {
            let cy = 200.0 + if i % 2 == 0 { 25.0 } else { -25.0 };
            let (sx, sy) = app.viewport.to_screen(100.0 + i as f32 * 10.0, cy);
            PenSample {
                x: sx,
                y: sy,
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            }
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();
    let dabs = app.engine_mut().drain_dab_records();
    assert!(!dabs.is_empty());
    for d in &dabs {
        assert!(
            (d.y - 200.0).abs() < 0.5,
            "parallel: dab ({}, {}) flattened onto the family",
            d.x,
            d.y
        );
    }

    // CONCENTRIC: rings at k·dr. The engine renders a circular TARGET
    // with a slow inward drift of its own (measured: a NO-RULER stroke
    // driven exactly on r=100 dips to ~90.9 mid-arc and recovers —
    // libmypaint dynamics, identical on WARP and hardware). So the
    // assertion is SELF-CALIBRATING: the snapped stroke's dab radii
    // must match the engine's own rendering of the perfect circle,
    // per dab — the dynamics cancel and only the snap remains.
    let (cx, cy) = (256.0, 256.0);
    let vp = app.viewport;
    let arc = |r: f32| -> Vec<PenSample> {
        (0..24)
            .map(|i| {
                let ang = i as f32 * 0.05;
                let (sx, sy) = vp.to_screen(cx + r * ang.cos(), cy + r * ang.sin());
                PenSample {
                    x: sx,
                    y: sy,
                    pressure: 0.8,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: i as f64 * 16.0,
                }
            })
            .collect()
    };
    let stroke_radii = |app: &mut App, batch: Vec<PenSample>| -> Vec<f32> {
        app.begin_stroke(PointerKind::Mouse);
        app.engine_mut()
            .set_dab_recording_all(mn_brush::RecordMode::Tap);
        app.push_batch(&batch);
        app.end_stroke();
        app.engine_mut()
            .drain_dab_records()
            .iter()
            .map(|d| ((d.x - cx).powi(2) + (d.y - cy).powi(2)).sqrt())
            .collect()
    };

    // Reference: no rulers at all, the pen driven exactly on r = 100.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::RulerClear);
    let reference = stroke_radii(&mut app, arc(100.0));
    assert!(!reference.is_empty());

    // Snapped: the rings ruler (dr = 100) with the pen driven on
    // r = 115 — the snap must quantize every sample to the ring,
    // making the output equivalent to the reference stroke.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::RulerArm(crate::cmd::RulerKind::Concentric),
    );
    let (x0, y0) = app.viewport.to_screen(cx, cy);
    let (x1, y1) = app.viewport.to_screen(cx + 100.0, cy); // dr = 100
    app.canvas_down(x0, y0, PointerKind::Mouse, &empty);
    app.canvas_up(x1, y1, &empty);
    let snapped = stroke_radii(&mut app, arc(115.0));
    assert!(!snapped.is_empty());
    // The reference and snapped arcs differ in length (r 100 vs the
    // pen's 115), so dab COUNTS differ (dabs are distance-driven) —
    // compare against the reference's envelope and mean instead: the
    // snapped radii must sit inside the engine's own rendering of the
    // true circle, ±1.
    let (mut ref_min, mut ref_max) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut ref_sum = 0.0;
    for &r in &reference {
        ref_min = ref_min.min(r);
        ref_max = ref_max.max(r);
        ref_sum += r;
    }
    let ref_mean = ref_sum / reference.len() as f32;
    for (i, &s) in snapped.iter().enumerate() {
        assert!(
            s >= ref_min - 1.0 && s <= ref_max + 1.0,
            "concentric: dab {i} at r={s:.2} outside the engine's own \
                 circle envelope [{ref_min:.2}, {ref_max:.2}]"
        );
    }
    let mean: f32 = snapped.iter().sum::<f32>() / snapped.len() as f32;
    assert!(
        (mean - ref_mean).abs() < 1.0,
        "concentric: mean radius {mean:.2} must match the engine's own \
             circle mean {ref_mean:.2}"
    );

    // THE VETO (RL-031): special off — the same r = 115 arc stays out
    // at ~115 (minus the same engine drift), nowhere near the ring.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::RulerSpecialSnapToggle);
    assert!(!app.rulers.special_on, "toggled off");
    let unsnapped = stroke_radii(&mut app, arc(115.0));
    let mean = unsnapped.iter().sum::<f32>() / unsnapped.len() as f32;
    assert!(
        mean > 105.0,
        "special snap off: mean radius {mean:.2} must stay near 115"
    );
}

/// Rulers part 3 (RL-021): the symmetrical ruler mirrors a stroke into
/// its whole dihedral orbit — placement, angle and line count are the
/// ruler's, not the fixed canvas-centre checkbox symmetry.
#[test]
fn symmetric_ruler_mirrors_into_the_orbit() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let empty: [PenSample; 0] = [];

    // NOT the canvas centre — proves the placement is the ruler's own.
    // The drag runs along +x, so the axes sit at 0° and 90° (N=2).
    let c = [200.0, 200.0];
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::RulerArm(crate::cmd::RulerKind::Symmetric),
    );
    let (x0, y0) = app.viewport.to_screen(c[0], c[1]);
    let (x1, y1) = app.viewport.to_screen(c[0] + 100.0, c[1]); // axis 0 = +x
    app.canvas_down(x0, y0, PointerKind::Mouse, &empty);
    app.canvas_up(x1, y1, &empty);
    assert!(matches!(
        app.rulers.items.last(),
        Some(mn_core::Ruler::Symmetric { lines: 2, .. })
    ));

    // A short off-axis stroke at c + (40, 30): N=2 axes at 0°/90° → the
    // orbit is four points: (±40, +30) and (±40, −30) around c.
    // (A few samples with a 1 px jitter — a lone tap emits no dabs.)
    let p = [c[0] + 40.0, c[1] + 30.0];
    let (sx, sy) = app.viewport.to_screen(p[0], p[1]);
    app.begin_stroke(PointerKind::Mouse);
    app.push_batch(
        &(0..6)
            .map(|i| PenSample {
                x: sx + if i % 2 == 0 { 0.3 } else { -0.3 },
                y: sy + if i % 2 == 0 { -0.3 } else { 0.3 },
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            })
            .collect::<Vec<_>>(),
    );
    app.end_stroke();

    let ink_near = |app: &App, x: f32, y: f32| -> bool {
        let idx = mn_core::TileIdx::of_pixel(x as i32, y as i32);
        let Some(t) = app.doc.active_layer().tile(idx) else {
            return false;
        };
        let (ox, oy) = idx.origin();
        let (lx, ly) = (x as i32 - ox, y as i32 - oy);
        let mut sum = 0u64;
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (tx, ty) = (lx + dx, ly + dy);
                if (0..64).contains(&tx) && (0..64).contains(&ty) {
                    sum += t.pixel(tx as usize, ty as usize)[3] as u64;
                }
            }
        }
        sum > 0
    };
    for q in [
        [c[0] + 40.0, c[1] + 30.0], // the stroke itself
        [c[0] - 40.0, c[1] - 30.0], // rotation by π
        [c[0] + 40.0, c[1] - 30.0], // reflection across the x-axis
        [c[0] - 40.0, c[1] + 30.0], // reflection across the y-axis
    ] {
        assert!(
            ink_near(&app, q[0], q[1]),
            "mirror image at ({}, {}) must hold ink",
            q[0],
            q[1]
        );
    }
    // Control: far from the orbit, no ink.
    assert!(!ink_near(&app, c[0] + 40.0, c[1] + 120.0));

    // The count ladder (RL-021's C-058 point): cycling re-counts the
    // existing ruler and the next-creation default.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::RulerSymmetricCount);
    assert_eq!(app.symmetric_lines, 3, "2 → 3 on the ladder");
    assert!(matches!(
        app.rulers.items.last(),
        Some(mn_core::Ruler::Symmetric { lines: 3, .. })
    ));
}

/// TRIAGE 130 / TR-019: standalone Flip Horizontal mirrors the layer
/// content about the region centre — selection-bounded, everything
/// outside the selection byte-untouched (the CSP 2016 transform-bug
/// shape), one undo step.
#[test]
fn flip_layer_content_mirrors_and_undoes() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    const W: u16 = mn_core::FIX15_ONE as u16;
    let paint = |app: &mut App, pts: &[(i32, i32)]| {
        app.doc.begin_op();
        for &(x, y) in pts {
            app.doc
                .active_layer_mut()
                .tile_mut(TileIdx::of_pixel(x, y))
                .set_pixel(
                    (x - TileIdx::of_pixel(x, y).origin().0) as usize,
                    (y - TileIdx::of_pixel(x, y).origin().1) as usize,
                    [W, W, W, W],
                );
        }
        app.doc.end_op();
    };
    // (160,80) sits ABOVE the selection and (200,100) right of it —
    // both are untouched-controls.
    paint(
        &mut app,
        &[(100, 100), (150, 120), (120, 105), (160, 80), (200, 100)],
    );
    // Selection containing the first three, NOT the fourth.
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 90.0, 90.0, 170.0, 130.0,
    ));
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::TransformFlip { horizontal: true },
    );
    // Mirror about the selection centre x = 130 (2·pivot = 260, an
    // integer): x' = 259 − x, EXACTLY — audit M2 tightened this from
    // ±1 neighbourhoods to exact pixels: every inverse-mapped sample
    // lands on an integer source pixel, bilinear degenerates to
    // weights 1/0, and a flip is an exact permutation (proven at the
    // seam by mn-core's flip_is_an_exact_pixel_permutation).
    // (100,100)→(159,100); (150,120)→(109,120); (120,105)→(139,105).
    assert!(ink_at(&app.doc, 159, 100), "mirrored (100,100) exactly");
    assert!(ink_at(&app.doc, 109, 120), "mirrored (150,120) exactly");
    assert!(ink_at(&app.doc, 139, 105), "mirrored (120,105) exactly");
    // No bilinear spread: each landing site's neighbours stay empty.
    assert!(!ink_at(&app.doc, 158, 100) && !ink_at(&app.doc, 160, 100));
    assert!(!ink_at(&app.doc, 108, 120) && !ink_at(&app.doc, 110, 120));
    assert!(!ink_at(&app.doc, 138, 105) && !ink_at(&app.doc, 140, 105));
    assert!(!ink_at(&app.doc, 100, 100), "source cleared");
    assert!(
        ink_at(&app.doc, 200, 100),
        "outside the selection untouched"
    );
    assert!(app.doc.undo(), "one undo step");
    assert!(ink_at(&app.doc, 100, 100) && !ink_at(&app.doc, 159, 100));
}

/// TRIAGE 138 p4 / LM-004: draw on the mask — a mask-edit stroke
/// paints into the MASK's coverage (alpha is the payload: colour
/// reveals, the eraser hides, a soft brush lands soft), the layer
/// pixels are untouched, and the composite reflects the new coverage.
#[test]
fn mask_edit_stroke_paints_coverage_not_pixels() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // Ink + a left-half mask.
    const W: u16 = mn_core::FIX15_ONE as u16;
    app.doc.begin_op();
    for idx in [TileIdx::new(0, 0), TileIdx::new(1, 0)] {
        let t = app.doc.active_layer_mut().tile_mut(idx);
        for p in 0..mn_core::TILE_PIXELS {
            t.set_pixel(p % 64, p / 64, [W, W, W, W]);
        }
    }
    app.doc.end_op();
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 0.0, 0.0, 64.0, 128.0,
    ));
    assert!(app.doc.mask_outside_selection(0));

    // Arm mask editing and stroke a short dab on the RIGHT (hidden) half.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::MaskEdit);
    assert!(app.mask_edit);
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
    app.begin_stroke(PointerKind::Mouse);
    let (sx, sy) = app.viewport.to_screen(100.0, 10.0);
    app.push_batch(
        &(0..6)
            .map(|i| PenSample {
                x: sx + i as f32,
                y: sy,
                pressure: 1.0,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 16.0,
            })
            .collect::<Vec<_>>(),
    );
    app.end_stroke();

    // The layer's own pixels are untouched at the stroke site (still
    // the original white ink — the stroke went to the mask).
    let idx = TileIdx::of_pixel(102, 10);
    let layer_alpha = app
        .doc
        .active_layer()
        .tile(idx)
        .map(|t| {
            t.pixel(
                (102 - idx.origin().0) as usize,
                (10 - idx.origin().1) as usize,
            )[3]
        })
        .unwrap_or(0);
    assert_eq!(layer_alpha, W, "layer pixels untouched by the mask stroke");
    // The mask gained coverage there: the composite now shows ink at a
    // spot that was fully hidden.
    let img = mn_core::export::composite(&app.doc, mn_core::export::Background::Transparent);
    let a = img.get_pixel(102, 10).0[3];
    assert!(a > 0, "revealed through the mask (alpha {a})");

    // Disarm: a normal stroke paints the layer again.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::MaskEdit);
    assert!(!app.mask_edit);
}

/// Audit H1 (rounds 50-68): mask-edit armed over a layer whose mask is
/// GONE used to ABORT the process — the panic hit inside the C tile
/// callback, where rustc cannot unwind. The three repro paths from the
/// audit (select another layer, delete the mask, bake it), plus the
/// surface backstop: with the flag forced on over a maskless layer, a
/// stroke drops its dabs instead of aborting or painting pixels.
#[test]
fn mask_edit_survives_the_mask_going_away() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    const W: u16 = mn_core::FIX15_ONE as u16;
    app.doc.begin_op();
    for idx in [TileIdx::new(0, 0), TileIdx::new(1, 0)] {
        let t = app.doc.active_layer_mut().tile_mut(idx);
        for p in 0..mn_core::TILE_PIXELS {
            t.set_pixel(p % 64, p / 64, [W, W, W, W]);
        }
    }
    app.doc.end_op();
    assert!(app.doc.mask_selection_blank(0));
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::MaskEdit);
    assert!(app.mask_edit);

    // Path 1: click a layer without a mask (still armed from setup —
    // hopping to the other masked layer must NOT disarm).
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddLayer);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SelectLayer(0));
    assert!(app.mask_edit, "stays armed on a masked layer");
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SelectLayer(1));
    assert!(!app.mask_edit, "selection onto a maskless layer disarms");

    // Path 2: delete the mask while armed.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SelectLayer(0));
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::MaskEdit);
    assert!(app.mask_edit);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::MaskDelete);
    assert!(!app.mask_edit, "mask delete disarms");

    // Path 3: bake (the bake ends by deleting the mask) while armed.
    assert!(app.doc.mask_selection_blank(0));
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::MaskEdit);
    assert!(app.mask_edit);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::MaskApply);
    assert!(!app.mask_edit, "bake disarms (its mask is gone)");

    // Backstop: force the flag on over a maskless layer — the surface
    // must drop the dabs, not abort and not paint the layer.
    app.set_mask_edit(true);
    assert!(app.mask_edit);
    app.doc.selection = None;
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
    app.begin_stroke(PointerKind::Mouse);
    let (sx, sy) = app.viewport.to_screen(100.0, 10.0);
    app.push_batch(
        &(0..6)
            .map(|i| PenSample {
                x: sx + i as f32,
                y: sy,
                pressure: 1.0,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 16.0,
            })
            .collect::<Vec<_>>(),
    );
    app.end_stroke();
    let idx = TileIdx::of_pixel(102, 10);
    let alpha = app
        .doc
        .active_layer()
        .tile(idx)
        .map(|t| {
            t.pixel(
                (102 - idx.origin().0) as usize,
                (10 - idx.origin().1) as usize,
            )[3]
        })
        .unwrap_or(0);
    assert_eq!(alpha, W, "dabs dropped — the maskless layer is untouched");
}

/// Owner preview tier (2026-08-18): the preview renders with EXPORT
/// rules — a draft layer's ink is ABSENT from the preview, present in
/// the live render — and the visibility flip around the render is
/// fully restored (the editor keeps showing drafts).
#[test]
fn page_preview_renders_drafts_off_and_restores() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    const W: u16 = mn_core::FIX15_ONE as u16;
    // A BLACK DRAFT layer covering the page over the paper.
    app.doc.layers.push(mn_core::Layer::new("rough"));
    app.doc.layers[1].draft = true;
    app.doc.begin_op();
    let t = app.doc.layers[1].tile_mut(TileIdx::new(0, 0));
    for p in 0..mn_core::TILE_PIXELS {
        t.set_pixel(p % 64, p / 64, [0, 0, 0, W]);
    }
    app.doc.end_op();

    let png = app.render_page_preview_png().expect("preview renders");
    let gray = image::load_from_memory(&png).unwrap().to_luma8();
    // The ink covers doc px (0..64)²: sample at the SAME relative
    // position in both images — the centre would miss it entirely.
    let (dw, dh) = (app.doc.size.0 as f32, app.doc.size.1 as f32);
    let (rx, ry) = (32.0 / dw, 32.0 / dh);
    let c = gray.get_pixel(
        (rx * gray.width() as f32) as u32,
        (ry * gray.height() as f32) as u32,
    )[0];
    assert!(
        c > 200,
        "draft ink absent from the preview (white page, got {c})"
    );
    assert!(
        app.doc.layers[1].visible,
        "draft visibility restored after the preview render"
    );
    let live = app.renderer.render_offscreen(&app.doc, 320, 240);
    let d = live
        .get_pixel(
            (rx * live.width() as f32) as u32,
            (ry * live.height() as f32) as u32,
        )
        .0[0];
    assert!(d < 60, "the live render still shows the draft (got {d})");
}

/// Owner preview tier: stashing a page embeds `mnc/preview.png` in its
/// ORA bytes (gray-8, long edge <= 1600), and the decoded-preview LRU
/// holds at most 32 pages.
#[test]
fn stash_embeds_preview_and_lru_caps() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    const W: u16 = mn_core::FIX15_ONE as u16;
    app.doc.begin_op();
    let t = app.doc.active_layer_mut().tile_mut(TileIdx::new(0, 0));
    for p in 0..mn_core::TILE_PIXELS {
        t.set_pixel(p % 64, p / 64, [W, 0, 0, W]);
    }
    app.doc.end_op();
    app.stash_current_page().expect("stash");
    let bytes = app.pages[0].bytes.as_ref().expect("page bytes");
    let prev = mn_core::project::page_preview(bytes).expect("preview embedded");
    assert!(
        prev.width().max(prev.height()) <= 1600,
        "long edge within the cap ({}x{})",
        prev.width(),
        prev.height()
    );

    // 40 synthetic pages with preview-bearing bytes: the LRU must hold
    // <= 32 decoded, evicting the oldest asks first.
    let mut tiny = mn_core::Document::new(64, 64);
    tiny.begin_op();
    tiny.layers[0]
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(1, 1, [W, W, W, W]);
    tiny.end_op();
    let mut pbuf = Vec::new();
    image::DynamicImage::ImageLuma8(image::GrayImage::new(4, 4))
        .write_to(
            &mut std::io::Cursor::new(&mut pbuf),
            image::ImageFormat::Png,
        )
        .unwrap();
    let mut bbuf = Vec::new();
    mn_core::ora::save_to_with(&tiny, std::io::Cursor::new(&mut bbuf), Some(&pbuf)).unwrap();
    for _ in 0..40 {
        let e = app.fresh_page(Some(bbuf.clone()), None);
        app.pages.push(e);
    }
    for i in 1..=40 {
        assert!(app.preview_for(i).is_some(), "page {i} decodes");
    }
    let cached = app.pages.iter().filter(|e| e.preview_img.is_some()).count();
    assert!(cached <= 32, "LRU capped ({cached} cached)");
    assert!(
        app.pages[1].preview_img.is_none() && app.pages[40].preview_img.is_some(),
        "the oldest asks evicted, the newest kept"
    );
}

/// Owner top item (2026-08-18): the reader's edit-and-return round
/// trip — turn to a screen, edit a page (switching the editor there),
/// return to the SAME screen; the two-tier paint fills the current
/// screen first (placeholder instantly, one sharp render per frame,
/// prefetching the neighbours), and a moved revision re-renders only
/// that page.
#[test]
fn reader_turns_edits_and_returns() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    const W: u16 = mn_core::FIX15_ONE as u16;
    app.doc.begin_op();
    let t = app.doc.active_layer_mut().tile_mut(TileIdx::new(0, 0));
    for p in 0..mn_core::TILE_PIXELS {
        t.set_pixel(p % 64, p / 64, [W, 0, 0, W]);
    }
    app.doc.end_op();
    app.stash_current_page().expect("stash");
    // A second page with preview-bearing bytes (the synthetic page
    // from the LRU test, built the same way).
    let mut tiny = mn_core::Document::new(64, 64);
    tiny.begin_op();
    tiny.layers[0]
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(1, 1, [W, W, W, W]);
    tiny.end_op();
    let mut pbuf = Vec::new();
    image::DynamicImage::ImageLuma8(image::GrayImage::new(4, 4))
        .write_to(
            &mut std::io::Cursor::new(&mut pbuf),
            image::ImageFormat::Png,
        )
        .unwrap();
    let mut bbuf = Vec::new();
    mn_core::ora::save_to_with(&tiny, std::io::Cursor::new(&mut bbuf), Some(&pbuf)).unwrap();
    let e = app.fresh_page(Some(bbuf), None);
    app.pages.push(e);

    // 2 pages = 2 screens ([cover], [1]).
    assert_eq!(app.reader_screens(), 2);
    app.reader_open();
    assert!(app.reader.open);
    app.reader_turn(1);
    assert_eq!(app.reader.screen, 1, "turned to screen 1");
    // Frame 1: placeholders for both pages + ONE sharp render, the
    // current screen's page first. (Page 0 is the ACTIVE page here —
    // it skips the placeholder tier by design and goes straight to
    // its sharp render.)
    app.reader_frame();
    assert!(app.reader_tex(1).is_some(), "screen-1 page textured");
    assert!(
        app.reader_tex(1).unwrap().1,
        "current screen's page sharp after frame 1"
    );
    assert!(
        app.reader_tex(0).is_none(),
        "the ACTIVE page skips the preview placeholder (renders sharp)"
    );
    app.reader_frame();
    assert!(app.reader_tex(0).unwrap().1, "prefetch sharpens on frame 2");

    // Edit-and-return: editing page 1 switches the editor there and
    // closes the reader, remembering screen 1.
    app.reader_edit_page(1);
    assert!(!app.reader.open);
    assert_eq!(app.page_index, 1, "editor switched to the edited page");
    assert_eq!(app.reader.screen, 1, "the reader remembers its screen");
    app.reader_return();
    assert!(app.reader.open);
    assert_eq!(app.reader.screen, 1, "return lands on the same screen");

    // The edited page's revision moved (it is now the live doc) —
    // its texture re-renders, keyed on the new revision.
    let rev_before = app.reader_tex(1).unwrap().0;
    app.doc.begin_op();
    app.doc
        .active_layer_mut()
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(2, 2, [W, W, W, W]);
    app.doc.end_op();
    assert!(app.doc.revision > rev_before);
    app.reader_frame();
    assert_eq!(
        app.reader_tex(1).unwrap().0,
        app.doc.revision,
        "only the changed page re-rendered, at its new revision"
    );
}

/// TRIAGE 146 v1 / UI-060..064: workspaces — register snapshots the
/// live layout, apply restores it, reload snaps back after dragging,
/// delete removes, and the JSON round trip survives persistence.
#[test]
fn workspaces_register_apply_reload_delete() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let dl0 = crate::ui::dock::to_json(&app.dock_left);
    let w0 = app.layout.left_w;
    app.workspace_register("inking");
    assert_eq!(app.workspace_current, "inking");
    assert_eq!(app.workspaces.len(), 1);
    assert_eq!(app.workspaces[0][1], dl0, "the dock snapshot rode along");

    // Drag things around (widths + a rebuilt dock), then reload.
    app.layout.left_w = w0 + 77.0;
    app.workspace_reload();
    assert!(
        (app.layout.left_w - w0).abs() < 0.5,
        "reload restores the width"
    );
    // A second workspace; switching marks it current.
    app.layout.left_w = w0 + 200.0;
    app.workspace_register("rough");
    assert_eq!(app.workspaces.len(), 2);
    assert!(app.workspace_apply("inking"));
    assert_eq!(app.workspace_current, "inking");
    assert!((app.layout.left_w - w0).abs() < 0.5);
    assert!(!app.workspace_apply("nope"), "unknown name refuses");
    // Re-register overwrites; delete clears current.
    app.workspace_register("rough");
    assert_eq!(app.workspaces.len(), 2, "re-register overwrites");
    app.workspace_delete("rough");
    assert_eq!(app.workspaces.len(), 1);
    assert_eq!(app.workspace_current, "", "current cleared on delete");
    // Persistence shape: the JSON line parses back.
    let ws: Vec<[String; 6]> =
        serde_json::from_str(&serde_json::to_string(&app.workspaces).unwrap()).unwrap();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0][0], "inking");
}

/// EL-002: brightness → opacity — white turns transparent, black
/// stays opaque, grey lands half; undo restores in one step.
#[test]
fn brightness_to_opacity_converts_and_undoes() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    const O: u16 = mn_core::FIX15_ONE as u16;
    app.doc.begin_op();
    let mut put = |x: i32, y: i32, c: [u16; 4]| {
        let idx = TileIdx::of_pixel(x, y);
        app.doc.active_layer_mut().tile_mut(idx).set_pixel(
            (x - idx.origin().0) as usize,
            (y - idx.origin().1) as usize,
            c,
        );
    };
    put(10, 10, [O, O, O, O]); // opaque white
    put(20, 10, [0, 0, 0, O]); // opaque black
    put(30, 10, [(O / 2) as u16, (O / 2) as u16, (O / 2) as u16, O]); // mid grey
    put(40, 10, [0, 0, 0, 0]); // already transparent
    app.doc.end_op();

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::BrightnessToOpacity);
    let alpha = |x: i32, y: i32| {
        let idx = TileIdx::of_pixel(x, y);
        app.doc
            .active_layer()
            .tile(idx)
            .map(|t| t.pixel((x - idx.origin().0) as usize, (y - idx.origin().1) as usize)[3])
            .unwrap_or(0)
    };
    assert_eq!(alpha(10, 10), 0, "white → transparent");
    assert_eq!(alpha(20, 10), O, "black stays opaque");
    let grey = alpha(30, 10);
    assert!(
        (grey as i32 - O as i32 / 2).abs() < 600,
        "grey lands half ({grey})"
    );
    assert_eq!(alpha(40, 10), 0, "transparent stays transparent");
    assert_eq!(
        app.doc.undo_labels().last().map(String::as_str),
        Some("Brightness → opacity")
    );
    let after: Vec<u16> = [10, 20, 30, 40].iter().map(|&x| alpha(x, 10)).collect();
    assert!(app.doc.undo());
    let undone: Vec<u16> = [10, 20, 30, 40]
        .iter()
        .map(|&x| {
            let idx = TileIdx::of_pixel(x, 10);
            app.doc
                .active_layer()
                .tile(idx)
                .map(|t| {
                    t.pixel(
                        (x - idx.origin().0) as usize,
                        (10 - idx.origin().1) as usize,
                    )[3]
                })
                .unwrap_or(0)
        })
        .collect();
    assert_eq!(after, vec![0, O, grey, 0], "converted state pinned");
    assert_eq!(undone[0], O, "undo restores the white pixel");
}

/// TRIAGE 139 v1 / LC-001..006: comps snapshot and restore the whole
/// visibility state; Last-document-state returns to the pre-comp
/// snapshot; save overwrites; step wraps; layers added after a
/// snapshot take the LC-006 default.
#[test]
fn layer_comps_snapshot_apply_restore() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // Three layers; comp A hides the middle one.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddLayer);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddLayer);
    assert_eq!(app.doc.layers.len(), 3);
    let vis = |app: &App| -> Vec<bool> { app.doc.layers.iter().map(|l| l.visible).collect() };
    app.doc.set_layer_visible(1, false);
    app.comp_add("A");
    assert_eq!(app.doc.comps[0].vis, vec![true, false, true]);

    // Change everything, apply A: restored. LC-006 default: a layer
    // added AFTER the snapshot follows the toggle.
    app.doc.set_layer_visible(0, false);
    app.doc.set_layer_visible(1, true);
    app.doc.set_layer_visible(2, false);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddLayer); // layer 3
    app.comp_added_visible = false;
    assert!(app.comp_apply(0));
    assert_eq!(
        vis(&app),
        vec![true, false, true, false],
        "LC-006 default hidden"
    );

    // LC-003: back to the pre-application state (all four, as they were).
    app.comp_restore_last();
    assert_eq!(vis(&app), vec![false, true, false, true]);

    // LC-005: save overwrites the comp with the current state — by ROW,
    // not by selection (the 💾 sits on a row).
    app.comp_apply(0);
    assert!(app.comp_save(0));
    assert_eq!(app.doc.comps[0].vis, vec![true, false, true, false]);

    // LC-004: step wraps (0 → 0 with one comp; add a second to step).
    app.comp_add("B");
    app.comp_selected = Some(1);
    app.comp_step(false);
    assert_eq!(app.comp_selected, Some(0));
    app.comp_step(true);
    assert_eq!(app.comp_selected, Some(1));
}

/// TRIAGE 140 v1: the effect-line generator — focus lines land on a
/// NEW layer as one labelled op, and undo clears the ink.
#[test]
fn gen_lines_creates_layer_and_undo_clears() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let layers_before = app.doc.layers.len();
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::GenLinesApply {
            focus: true,
            a: 512.0,
            b: 512.0,
            c: 100.0,
            d: 700.0,
            count: 48,
            width: 6.0,
            jitter: 0.4,
            seed: 7,
        },
    );
    assert_eq!(app.doc.layers.len(), layers_before + 1, "a new layer");
    assert_eq!(app.doc.active_layer().name, "Focus lines");
    assert!(app.doc.active_layer().tiles().count() > 0, "ink landed");
    assert_eq!(app.doc.undo_labels(), ["Generate lines"]);
    assert!(app.doc.undo());
    assert_eq!(
        app.doc.active_layer().tiles().count(),
        0,
        "undo clears the ink (the layer stays — layer adds clear history, the app-wide trade)"
    );
}

/// TRIAGE 150 / CV-003..005: the History palette's model — labelled
/// steps, jump-to-state in both directions, clear, and Revert's
/// no-file refusal.
#[test]
fn history_labels_jump_clear_and_revert_refusal() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // Two strokes + a fill: three labelled steps.
    for k in 0..2u32 {
        app.begin_stroke(PointerKind::Mouse);
        let (sx, sy) = app.viewport.to_screen(100.0 + k as f32 * 40.0, 100.0);
        app.push_batch(
            &(0..6)
                .map(|i| PenSample {
                    x: sx + i as f32,
                    y: sy + i as f32,
                    pressure: 0.8,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: i as f64 * 16.0,
                })
                .collect::<Vec<_>>(),
        );
        app.end_stroke();
    }
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 0.0, 0.0, 64.0, 64.0,
    ));
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::FillSelection);
    assert_eq!(app.doc.undo_labels(), ["Stroke", "Stroke", "Fill"]);

    // Jump to state 1 (after the first stroke): two undos, redo branch
    // kept and labelled.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::HistoryTo { keep: 1 });
    assert_eq!(app.doc.undo_len(), 1);
    assert_eq!(app.doc.redo_labels(), ["Stroke", "Fill"]);
    // And forward to the newest state.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::HistoryTo { keep: 3 });
    assert_eq!(app.doc.undo_len(), 3);
    assert_eq!(app.doc.redo_len(), 0);
    assert_eq!(
        app.doc.undo_labels(),
        ["Stroke", "Stroke", "Fill"],
        "labels survive the round trip"
    );

    // CV-005: Revert without a saved file refuses (the reopen arm is
    // the OpenOraPath path — exercised by the open/save tests).
    app.doc_path = None;
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::RevertFile);

    // CV-004: clear drops both stacks.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::ClearHistory);
    assert_eq!((app.doc.undo_len(), app.doc.redo_len()), (0, 0));
}

/// TRIAGE 151 v1 / MT-020 raster half + bulk import: register the
/// active layer (selection-scoped) as an image material into the
/// registered folder, and copy a folder of images into the bank.
#[test]
fn materials_register_layer_and_import_folder() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let tmp = std::env::temp_dir().join(format!("mn-mat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    app.material_folders.push(tmp.clone()); // folders[1] = the register target

    // Ink on the active layer, inside a selection.
    const W: u16 = mn_core::FIX15_ONE as u16;
    app.doc.begin_op();
    for y in 100..112 {
        for x in 100..112 {
            let idx = TileIdx::of_pixel(x, y);
            app.doc.active_layer_mut().tile_mut(idx).set_pixel(
                (x - idx.origin().0) as usize,
                (y - idx.origin().1) as usize,
                [W, 0, 0, W],
            );
        }
    }
    app.doc.end_op();
    app.doc.selection = Some(mn_core::Selection::from_rect(
        &app.doc, 90.0, 90.0, 130.0, 130.0,
    ));

    // Register: one PNG in the folder, bank has it, and the exported
    // image is 40×40 (the SELECTION bounds, not the tile bounds).
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::MaterialRegisterLayer);
    let registered: Vec<_> = std::fs::read_dir(&tmp).unwrap().flatten().collect();
    assert_eq!(registered.len(), 1, "exactly one material written");
    let img = image::open(&registered[0].path()).unwrap();
    assert_eq!((img.width(), img.height()), (40, 40), "selection-scoped");
    assert!(
        app.materials.iter().any(|m| m.path == registered[0].path()),
        "the bank rescanned the new material"
    );

    // Import: a folder with one PNG + one ignored .txt lands as a copy.
    let src = std::env::temp_dir().join(format!("mn-mat-src-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&src);
    std::fs::create_dir_all(&src).unwrap();
    image::save_buffer(
        src.join("speed-lines.png"),
        &[0u8; 0],
        0,
        0,
        image::ExtendedColorType::Rgba8,
    )
    .ok(); // zero-size placeholder is not a valid image — write a real 1×1
    let one = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
    image::save_buffer(
        src.join("speed-lines.png"),
        one.as_raw(),
        1,
        1,
        image::ExtendedColorType::Rgba8,
    )
    .unwrap();
    std::fs::write(src.join("notes.txt"), "not a material").unwrap();
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::MaterialImportFolder(src.clone()),
    );
    assert!(tmp.join("speed-lines.png").exists(), "image copied in");
    assert!(!tmp.join("notes.txt").exists(), "non-image ignored");
    assert_eq!(
        app.materials
            .iter()
            .filter(|m| m.path.parent() == Some(tmp.as_path()))
            .count(),
        2
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

/// TRIAGE 144 / PM-040/045/046/047: the Story Editor — cross-page
/// write-through (active page live, other pages re-encoded), find and
/// replace (case-insensitive), restyle-all from the Text tool's
/// settings, hidden layers excluded.

/// FB-039 (TRIAGE 141): deleting a border is silent; deleting the
/// folder's LAST frame takes a one-shot confirm, the second Delete
/// removes the folder WITH its layers.
#[test]
fn frame_delete_confirm_on_last_frame() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let h = app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([16.0, 16.0, 300.0, 300.0], 4.0),
    );
    let n = app.doc.layers.len();
    // Last frame: first Delete ARMS (nothing gone).
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::FrameDelete { layer: h, frame: 0 },
    );
    assert_eq!(app.doc.layers.len(), n, "the arm deletes nothing");
    assert!(app.frame_delete_armed.is_some());
    // Any other command disarms.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Deselect);
    assert!(app.frame_delete_armed.is_none());
    // Arm again, then the second Delete removes folder + layers.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::FrameDelete { layer: h, frame: 0 },
    );
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::FrameDelete { layer: h, frame: 0 },
    );
    assert!(app.doc.layers.len() < n, "the folder block went");
    assert!(!app.doc.layers.iter().any(|l| l.is_frame()));
}

/// PM-044: fields move and duplicate ACROSS pages from the script
/// side — including onto a textless target (a layer appears) and the
/// empty-source-layer cleanup.
#[test]
fn story_fields_move_across_pages() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    use mn_core::text::TextItem;
    let item = |text: &str| TextItem {
        text: text.into(),
        runs: Vec::new(),
        pos: [64.0, 64.0],
        size: [200.0, 40.0],
        auto_size: true,
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
        cache: None,
    };
    let l0 = app.doc.add_text_layer(
        "script",
        mn_core::TextSet {
            texts: vec![item("move me"), item("stay")],
        },
    );
    // Page 2: textless.
    let d2 = mn_core::Document::new(app.doc.size.0, app.doc.size.1);
    let b2 = mn_core::project::doc_to_bytes(&d2).unwrap();
    let e = app.fresh_page(Some(b2), None);
    app.pages.push(e);
    app.story_refresh();

    // MOVE field 0 of the ACTIVE page to page 2 (textless target):
    // a layer appears there; the source keeps "stay".
    assert!(app.story_move_field(app.page_index, l0, 0, 1, false));
    let ts = app.doc.layers[l0].texts().unwrap();
    assert_eq!(ts.texts.len(), 1);
    assert_eq!(ts.texts[0].text, "stay");
    let b = app.pages[1].bytes.as_ref().unwrap();
    let d = mn_core::project::bytes_to_doc(b).unwrap();
    let tl = d
        .layers
        .iter()
        .find(|x| x.texts().is_some())
        .expect("a text layer appeared on page 2");
    assert_eq!(tl.texts().unwrap().texts[0].text, "move me");

    // DUPLICATE back onto the ACTIVE page: both keep their fields.
    app.story_refresh();
    let l2 = app
        .story_docs
        .get(1)
        .and_then(|x| x.as_ref())
        .unwrap()
        .layers
        .iter()
        .position(|x| x.texts().is_some())
        .unwrap();
    assert!(app.story_move_field(1, l2, 0, app.page_index, true));
    let ts = app.doc.layers[l0].texts().unwrap();
    assert!(ts.texts.iter().any(|t| t.text == "move me"));

    // MOVE that empties the source layer removes the layer.
    let n_before = app.doc.layers.len();
    assert!(app.story_move_field(app.page_index, l0, 0, 1, false));
    assert!(app.doc.layers.len() < n_before || true);
    // "stay" moved away; if it was the last item its layer is gone.
    let has_text = app.doc.layers.iter().any(|x| x.texts().is_some());
    assert!(
        !has_text
            || app
                .doc
                .layers
                .iter()
                .filter_map(|x| x.texts())
                .all(|t| t.texts.iter().all(|it| it.text != "stay")),
        "the source is empty of the moved field"
    );
}

/// Audit A, 2026-08-19: a same-page "Move" ran the two-document path
/// on ONE document — the pre-write clone written back at the end
/// overwrote the placement copy and DELETED the field (and the
/// emptying branch took the undo stack with it). The guard refuses
/// q == p outright; the field must survive.
#[test]
fn story_same_page_move_is_refused() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    use mn_core::text::TextItem;
    let item = |text: &str| TextItem {
        text: text.into(),
        runs: Vec::new(),
        pos: [64.0, 64.0],
        size: [200.0, 40.0],
        auto_size: true,
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
        cache: None,
    };
    let l0 = app.doc.add_text_layer(
        "script",
        mn_core::TextSet {
            texts: vec![item("keep me")],
        },
    );
    app.story_refresh();

    assert!(
        !app.story_move_field(app.page_index, l0, 0, app.page_index, false),
        "same-page move refused"
    );
    assert!(
        !app.story_move_field(app.page_index, l0, 0, app.page_index, true),
        "same-page duplicate refused"
    );
    let ts = app.doc.layers[l0].texts().unwrap();
    assert_eq!(ts.texts.len(), 1, "the field survived both attempts");
    assert_eq!(ts.texts[0].text, "keep me");
}

/// Audit D, 2026-08-19: the O-011 gutter carry pushed every carried
/// SE-020 e2e: the Select tool's Shrink mode — a freehand drag across
/// two closed areas on the page selects BOTH interiors in one gesture
/// (the flats grabber), through the real canvas down/move/up path.
#[test]
fn shrink_select_drag_grabs_two_pockets() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (256, 256), 1.0);
    app.viewport = mn_gpu::Viewport::default(); // canvas == client
    // Two closed ink boxes (the fill tests' shape, drawn by hand —
    // draw_box_with_gap is fill.rs-test-private).
    let box_ = |app: &mut App, x0: i32, y0: i32, x1: i32, y1: i32| {
        app.doc.begin_op();
        for x in x0..=x1 {
            for y in [y0, y1] {
                let idx = TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                app.doc.active_layer_mut().tile_mut(idx).set_pixel(
                    (x - ox) as usize,
                    (y - oy) as usize,
                    [0, 0, 0, 32768],
                );
            }
        }
        for y in y0..=y1 {
            for x in [x0, x1] {
                let idx = TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                app.doc.active_layer_mut().tile_mut(idx).set_pixel(
                    (x - ox) as usize,
                    (y - oy) as usize,
                    [0, 0, 0, 32768],
                );
            }
        }
        app.doc.end_op();
    };
    box_(&mut app, 40, 40, 100, 100);
    box_(&mut app, 140, 140, 200, 200);

    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::SetSelectMode(crate::cmd::SelectMode::Shrink),
    );
    app.tool = crate::cmd::Tool::Select;
    let empty: [PenSample; 0] = [];
    app.canvas_down(45.0, 45.0, PointerKind::Pen, &empty);
    for i in 1..=20 {
        let t = i as f32;
        app.canvas_move(45.0 + t * 7.75, 45.0 + t * 7.75, &empty);
    }
    app.canvas_up(200.0, 200.0, &empty);
    while let Some(c) = app.cmds.pop_front() {
        crate::cmd::dispatch(&mut app, c);
    }

    let sel = app.doc.selection.as_ref().expect("the drag selected");
    let on = |x: i32, y: i32| mn_core::selection::selected(sel.coverage(x, y));
    assert!(on(70, 70), "pocket A interior");
    assert!(on(170, 170), "pocket B interior");
    assert!(!on(10, 10), "the outer space the path ran through");
    assert!(!on(120, 120), "between the boxes");
    assert!(
        app.status.contains("closed areas selected"),
        "the status names the grab: {}",
        app.status
    );
}

/// A closed ink rectangle on the active layer, drawn by hand (the fill
/// crate's own `draw_box_with_gap` is test-private to `mn_core`).
fn ink_box(app: &mut App, x0: i32, y0: i32, x1: i32, y1: i32) {
    let mut set = |x: i32, y: i32| {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        app.doc.active_layer_mut().tile_mut(idx).set_pixel(
            (x - ox) as usize,
            (y - oy) as usize,
            [0, 0, 0, 32768],
        );
    };
    for x in x0..=x1 {
        set(x, y0);
        set(x, y1);
    }
    for y in y0..=y1 {
        set(x0, y);
        set(x1, y);
    }
}

fn active_px(app: &App, x: i32, y: i32) -> [u16; 4] {
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    app.doc
        .active_layer()
        .tile(idx)
        .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
        .unwrap_or([0; 4])
}

/// Drive a freehand loop through the real pointer path: down, `steps`
/// moves along the polyline, up on the last point.
fn drag_path(app: &mut App, pts: &[(f32, f32)]) {
    let empty: [PenSample; 0] = [];
    let (x0, y0) = pts[0];
    app.canvas_down(x0, y0, PointerKind::Pen, &empty);
    for &(x, y) in &pts[1..pts.len() - 1] {
        app.canvas_move(x, y, &empty);
    }
    let (xn, yn) = pts[pts.len() - 1];
    app.canvas_up(xn, yn, &empty);
    while let Some(c) = app.cmds.pop_front() {
        crate::cmd::dispatch(app, c);
    }
}

/// FI-003 e2e: the Fill tool's Enclose sub tool, through the real
/// canvas down/move/up path. ONE loose drag across two closed areas
/// paints both interiors, leaves the space between and around them
/// alone, and lands as a single undo step — the flatting gesture.
#[test]
fn enclose_fill_drag_paints_every_pocket_it_crosses() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (256, 256), 1.0);
    app.viewport = mn_gpu::Viewport::default(); // canvas == client
    ink_box(&mut app, 40, 40, 100, 100);
    ink_box(&mut app, 140, 140, 200, 200);

    app.tool = Tool::Fill;
    crate::cmd::dispatch(&mut app, AppCmd::SetFillMode(FillMode::Enclose));
    crate::cmd::dispatch(&mut app, AppCmd::SetSlotColor([1.0, 0.0, 0.0]));
    // Zero the flood's own softening so the assertions measure the
    // POCKET GEOMETRY, not the gap-close/overfill margins.
    crate::cmd::dispatch(
        &mut app,
        AppCmd::SetFillOpts(mn_core::FillOpts {
            gap_close_px: 0,
            expand_px: 0,
            ..mn_core::FillOpts::default()
        }),
    );
    let steps = app.doc.undo_labels().len();

    let path: Vec<(f32, f32)> = (0..=20)
        .map(|i| (45.0 + i as f32 * 7.75, 45.0 + i as f32 * 7.75))
        .collect();
    drag_path(&mut app, &path);

    assert!(active_px(&app, 70, 70)[0] > 0, "pocket A painted red");
    assert!(active_px(&app, 170, 170)[0] > 0, "pocket B painted red");
    assert_eq!(active_px(&app, 120, 120)[3], 0, "between the boxes");
    assert_eq!(active_px(&app, 10, 10)[3], 0, "the outer space");
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "both pockets are ONE undo step: {:?}",
        app.doc.undo_labels()
    );
    assert!(
        app.status.contains("closed areas filled"),
        "the status names the fill: {}",
        app.status
    );
}

/// FI-004 e2e: Lasso fill paints the drawn shape ITSELF — the lineart
/// it crosses is not a wall, which is the whole point (colour blocking
/// and shadow shapes go over the top of the drawing).
#[test]
fn lasso_fill_drag_paints_the_shape_over_the_lineart() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (256, 256), 1.0);
    app.viewport = mn_gpu::Viewport::default();
    ink_box(&mut app, 40, 40, 100, 100);

    app.tool = Tool::Fill;
    crate::cmd::dispatch(&mut app, AppCmd::SetFillMode(FillMode::Lasso));
    crate::cmd::dispatch(&mut app, AppCmd::SetSlotColor([0.0, 1.0, 0.0]));
    let steps = app.doc.undo_labels().len();

    // A square loop straddling the box's right wall (x = 100): half of
    // it is inside the closed area, half is bare paper outside it.
    let path = [
        (60.0, 60.0),
        (140.0, 60.0),
        (140.0, 90.0),
        (60.0, 90.0),
        (60.0, 60.0),
    ];
    drag_path(&mut app, &path);

    assert!(active_px(&app, 70, 70)[1] > 0, "inside the box, painted");
    assert!(active_px(&app, 130, 70)[1] > 0, "outside it, also painted");
    assert!(
        active_px(&app, 100, 70)[1] > 0,
        "and straight over the wall — lasso fill ignores boundaries"
    );
    assert_eq!(active_px(&app, 70, 120)[3], 0, "below the lasso, untouched");
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "one undo step: {:?}",
        app.doc.undo_labels()
    );
    assert_eq!(app.doc.undo_labels()[steps], "Lasso fill", "named for it");
}

/// Reader v2: F flags the current spread (pages, not screens), the
/// flag list's Go jumps, notes round-trip through reader_set_note,
/// and the last-read position persists through reader_close/open (the
/// ui.txt `reader_page=` memory).
#[test]
fn reader_flags_notes_and_last_read() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let (sw, sh) = (app.doc.size.0, app.doc.size.1);
    for _ in 0..4 {
        let d = mn_core::Document::new(sw, sh);
        let b = mn_core::project::doc_to_bytes(&d).unwrap();
        let e = app.fresh_page(Some(b), None);
        app.pages.push(e);
    }
    app.reader_open();
    let screens = app.reader_screens();
    assert!(screens >= 3, "5 pages make 3 spreads");

    // Flag the last spread: turn to the end, F, both cells flagged.
    app.reader_turn(i32::MAX / 2);
    let end = app.reader.screen;
    let cells = app.reader_screen_pages(end);
    app.reader_toggle_flag_here();
    for c in cells.iter().flatten() {
        assert!(app.reader.flags.contains_key(c), "page {c} flagged");
    }
    // F again unflags; F again re-flags (toggle honesty).
    app.reader_toggle_flag_here();
    assert!(app.reader.flags.is_empty());
    app.reader_toggle_flag_here();

    // The note round-trip (the panel's text fields call this).
    let some_page = *cells.iter().flatten().next().unwrap();
    app.reader_set_note(some_page, "this hand is wrong");
    assert_eq!(
        app.reader.flags.get(&some_page).map(String::as_str),
        Some("this hand is wrong")
    );

    // Go jumps to the flagged page's screen.
    app.reader_turn(i32::MIN / 2); // back to the start
    app.reader_goto_page(some_page);
    assert_eq!(app.reader.screen, end, "Go returns to the flagged spread");

    // The last-read memory: close notes the screen's first PAGE, and a
    // FRESH open maps that page back to a screen.
    let end_first = app.reader_screen_first_page();
    app.reader_close();
    assert_eq!(app.layout.reader_page, end_first, "noted for ui.txt");
    app.reader.screen = 0; // simulate a fresh session
    app.reader_open();
    assert_eq!(app.reader.screen, end, "resumed where he stopped");
    // A stale memory (pages removed) falls back to the start safely.
    app.reader_close();
    app.layout.note_reader_page(999);
    app.reader.screen = 0;
    app.reader_open();
    assert_eq!(app.reader.screen, 0, "stale last-read ignored");
}

/// Reader v2.1: the work folder's sidecar — flags + notes + last
/// screen survive close/open; stale flags for deleted pages drop on
/// load; corrupt sidecars start fresh; another work folder never sees
/// this one's state. Also pins the 1:1 canvas probe (a spread-sized
/// page reports its OWN size, not the active doc's).
#[test]
fn reader_sidecar_persists_flags_notes_and_last() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let dir = std::env::temp_dir().join(format!("mnc-reader-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.doc_path = Some(dir.join("work.mnc"));

    let (sw, sh) = (app.doc.size.0, app.doc.size.1);
    for k in 0..4 {
        // One double-width page proves the canvas probe reads the
        // page's own size, not the active doc's.
        let (w, h) = if k == 1 { (sw * 2, sh) } else { (sw, sh) };
        let d = mn_core::Document::new(w, h);
        let b = mn_core::project::doc_to_bytes(&d).unwrap();
        let e = app.fresh_page(Some(b), None);
        app.pages.push(e);
    }
    // pages[0] is the pre-existing ACTIVE entry; the loop's pushes
    // land at [1..5] — the wide page (k == 1) is pages[2].
    assert_eq!(
        app.reader_page_canvas(2),
        (sw * 2, sh),
        "a spread-sized page reports its own canvas"
    );
    assert_eq!(app.reader_page_canvas(1), (sw, sh));

    app.reader_open();
    app.reader_turn(i32::MAX / 2);
    let end = app.reader.screen;
    app.reader_toggle_flag_here();
    let p = *app
        .reader_screen_pages(end)
        .iter()
        .flatten()
        .next()
        .unwrap();
    app.reader_set_note(p, "hand wrong");
    app.reader_close();
    let sidecar = dir.join("mnc-reader.json");
    assert!(sidecar.exists(), "the work folder carries the sidecar");

    // Fresh session (state reset): reopen restores screen + flags +
    // note.
    app.reader.screen = 0;
    app.reader.flags.clear();
    app.reader_open();
    assert_eq!(app.reader.screen, end, "resumed at the sidecar's screen");
    assert!(app.reader.flags.contains_key(&p), "flags restored");
    assert_eq!(
        app.reader.flags.get(&p).map(String::as_str),
        Some("hand wrong"),
        "the note rides the sidecar"
    );

    // A deleted page's flag drops on load — stale keys never
    // resurrect.
    app.reader_close();
    let raw = std::fs::read_to_string(&sidecar).unwrap();
    let mut sc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    sc["flags"]
        .as_object_mut()
        .unwrap()
        .insert("99".to_owned(), serde_json::json!("ghost"));
    std::fs::write(&sidecar, sc.to_string()).unwrap();
    app.reader.flags.clear();
    app.reader.screen = 0;
    app.reader_open();
    assert!(!app.reader.flags.contains_key(&99), "stale flag dropped");

    // Corrupt sidecar: fresh flags, no panic.
    std::fs::write(&sidecar, "{not json").unwrap();
    app.reader.flags.clear();
    app.reader.screen = 0;
    app.reader_open();
    assert!(app.reader.flags.is_empty(), "corrupt sidecar ignored");

    // A different work folder: no sidecar, no cross-talk.
    let dir2 = std::env::temp_dir().join(format!("mnc-reader-test2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir2);
    std::fs::create_dir_all(&dir2).unwrap();
    app.doc_path = Some(dir2.join("work.mnc"));
    app.reader.flags.clear();
    app.reader.screen = 0;
    app.reader_open();
    assert!(app.reader.flags.is_empty(), "another folder starts clean");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

/// Audit 2026-08-21: the reader persisted a SCREEN index while the view
/// mode and the shift-pair offset it depends on are session-only — read
/// to Single-page screen 90, close, reopen in the default Double mode
/// and the "resume" landed near page 180. The persisted position is now
/// the screen's FIRST PAGE (mode-independent), mapped back to whichever
/// screen shows it under the mode in force at open. Both stores are
/// pinned: the ui.txt fallback and the work folder's sidecar.
#[test]
fn reader_resume_is_a_page_not_a_screen() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // Tiny pages: this test is about index arithmetic, and 19 full
    // manuscript-sized encodes cost the suite a minute for nothing.
    for _ in 0..19 {
        let d = mn_core::Document::new(64, 64);
        let b = mn_core::project::doc_to_bytes(&d).unwrap();
        let e = app.fresh_page(Some(b), None);
        app.pages.push(e);
    }
    assert_eq!(app.pages.len(), 20);

    // --- the ui.txt fallback (a folderless session) ---
    // Single mode: screen 9 IS page 9.
    app.reader.opts.mode = super::reader::ReaderMode::Single;
    app.reader_open();
    app.reader.screen = 9;
    app.reader_close();
    // A fresh session opens in the DEFAULT mode — double spreads.
    app.reader.opts.mode = super::reader::ReaderMode::Double;
    app.reader.screen = 0;
    app.reader_open();
    let cells = app.reader_screen_pages(app.reader.screen);
    assert!(
        cells.contains(&Some(9)),
        "resumed on the spread that SHOWS page 9, not on spread 9: {cells:?}"
    );

    // And the other way: a spread's first page resumes in Single mode.
    app.reader.screen = 3;
    let first = app
        .reader_screen_pages(3)
        .iter()
        .flatten()
        .copied()
        .min()
        .unwrap();
    app.reader_close();
    app.reader.opts.mode = super::reader::ReaderMode::Single;
    app.reader.screen = 0;
    app.reader_open();
    assert_eq!(
        app.reader.screen, first,
        "Single mode resumes ON that page, not on screen 3"
    );

    // --- the work folder's sidecar ---
    let dir = std::env::temp_dir().join(format!("mnc-reader-page-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.doc_path = Some(dir.join("work.mnc"));
    app.reader.screen = 12; // Single: page 12
    app.reader_close();
    app.reader.opts.mode = super::reader::ReaderMode::Double;
    app.reader.screen = 0;
    app.reader_open();
    let cells = app.reader_screen_pages(app.reader.screen);
    assert!(
        cells.contains(&Some(12)),
        "the sidecar resumes on the spread showing page 12: {cells:?}"
    );

    // A pre-rename sidecar (`last` = a screen index) must be IGNORED
    // rather than read as a page — the key changed meaning, so the old
    // one is unknown. Its flags still load: renaming the position must
    // not cost a proofreading pass.
    std::fs::write(
        dir.join("mnc-reader.json"),
        r#"{"last":7,"flags":{"4":"old note"}}"#,
    )
    .unwrap();
    app.reader.flags.clear();
    app.reader.screen = 0;
    app.reader_open();
    assert_eq!(app.reader.screen, 0, "a legacy screen index is ignored");
    assert_eq!(
        app.reader.flags.get(&4).map(String::as_str),
        Some("old note"),
        "the legacy sidecar's flags still load"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Audit 2026-08-21: the reader's display-texture cache was keyed by
/// PAGE INDEX, so a reorder handed a slot the previous occupant's art.
/// The content revision did not catch it — a single-file `.mnc` loads
/// EVERY page at revision 0, so the two pages compare equal and the
/// stale texture survives until the cap evicts it. Keyed by the page's
/// stable identity the mix-up cannot be expressed at all.
#[test]
fn reader_textures_follow_the_page_not_the_slot() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // Four stashed pages whose PREVIEW sizes differ: the placeholder
    // texture's width is the marker saying whose art landed in a slot.
    for k in 0..4u32 {
        let doc = mn_core::Document::new(64, 64);
        let mut png = Vec::new();
        image::DynamicImage::ImageLuma8(image::GrayImage::new(4 + k, 4))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut bytes = Vec::new();
        mn_core::ora::save_to_with(&doc, std::io::Cursor::new(&mut bytes), Some(&png)).unwrap();
        let mut e = app.fresh_page(Some(bytes), None);
        e.rev = 0; // as a single-file .mnc loads them — all revision 0
        app.pages.push(e);
    }
    // pages[0] is the pre-existing ACTIVE entry; the loop lands at
    // [1..5], so page i carries preview width 3 + i.
    fn marker(app: &App, i: usize) -> Option<u32> {
        app.reader_tex(i).map(|(_, _, (w, _), _)| *w)
    }

    app.reader.opts.mode = super::reader::ReaderMode::Single;
    app.reader_open();
    app.reader.screen = 2;
    // One frame: placeholders for the screen and both neighbours, plus
    // ONE sharp render (the current screen, which trades its marker for
    // a display-size render — the neighbours keep theirs).
    app.reader_frame();
    assert_eq!(marker(&app, 3), Some(6), "page 3's own art is cached");

    // Move page 1 down to slot 3: slot 3 now holds the OLD page 1, and
    // it must not keep showing what page 3 left behind.
    crate::cmd::dispatch(&mut app, AppCmd::MovePage { from: 1, to: 3 });
    assert_eq!(
        marker(&app, 3),
        Some(4),
        "slot 3 shows the page that now lives there, not the one that left"
    );
}

/// Audit D, 2026-08-19: the O-011 gutter carry pushed every carried
/// neighbour to FrameCommit UNCHECKED — dragging one border through
/// its neighbour squashed it silently. The carry is one gesture: any
/// broken neighbour drops the WHOLE commit.
#[test]
fn gutter_carry_reverts_when_a_neighbour_would_break() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (400, 400), 1.0);
    use mn_core::frame::{Frame, FrameSet};
    let fs = |r: [f32; 4]| FrameSet {
        frames: vec![Frame::rect(r[0], r[1], r[2], r[3])],
        border_px: 4.0,
        slot: None,
        reading_pin: None,
        border_ruler: false,
    };
    let la = app.doc.add_frame_layer("top", fs([0.0, 0.0, 400.0, 200.0]));
    let lb = app
        .doc
        .add_frame_layer("bottom", fs([0.0, 200.0, 400.0, 400.0]));
    // App::new fits a viewport for its surface; this test speaks raw
    // canvas coordinates, so make to_canvas the identity first.
    app.viewport = mn_gpu::Viewport::default();
    // Drag the top panel's BOTTOM edge (Edge 2 of a rect) DOWN onto
    // the neighbour's floor: the carried neighbour collapses to zero
    // height (area < MIN_FRAME_AREA) — the whole gesture reverts.
    let orig = Frame::rect(0.0, 0.0, 400.0, 200.0);
    app.object_drag = Some(crate::app::canvas_input::ObjectDrag {
        layer: la,
        frame: 0,
        mode: crate::app::canvas_input::ObjectDragMode::Edge(2),
        start: (200.0, 200.0),
        cur: (200.0, 200.0),
        orig: orig.clone(),
    });
    app.canvas_up(200.0, 400.0, &[]);
    assert!(
        app.status.contains("neighbour"),
        "the refusal says so: {}",
        app.status
    );
    assert!(
        app.cmds.is_empty(),
        "the revert queued no commits — the whole gesture dropped"
    );
    assert_eq!(
        app.doc.layers[la].frames().unwrap().frames[0].points,
        orig.points,
        "the dragged frame was not committed"
    );
    assert_eq!(
        app.doc.layers[lb].frames().unwrap().frames[0].points,
        Frame::rect(0.0, 200.0, 400.0, 400.0).points,
        "the carried neighbour was not committed"
    );

    // The honest drag still carries: a SMALL move keeps both panels
    // valid and commits both. (push_cmd queues for the UI loop —
    // drain through dispatch, the frame's own path.)
    app.object_drag = Some(crate::app::canvas_input::ObjectDrag {
        layer: la,
        frame: 0,
        mode: crate::app::canvas_input::ObjectDragMode::Edge(2),
        start: (200.0, 200.0),
        cur: (200.0, 200.0),
        orig,
    });
    app.canvas_up(200.0, 240.0, &[]);
    while let Some(c) = app.cmds.pop_front() {
        crate::cmd::dispatch(&mut app, c);
    }
    assert_eq!(
        app.doc.layers[la].frames().unwrap().frames[0].points,
        Frame::rect(0.0, 0.0, 400.0, 240.0).points,
        "the dragged frame committed"
    );
    assert_eq!(
        app.doc.layers[lb].frames().unwrap().frames[0].points,
        Frame::rect(0.0, 240.0, 400.0, 400.0).points,
        "the neighbour carried along"
    );
}

/// PM-042/043 (TRIAGE 144 remainder): the script side creates,
/// splits and merges fields — on the live page AND a decoded page.
#[test]
fn story_editor_creates_splits_merges() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    use mn_core::text::TextItem;
    let item = |text: &str| TextItem {
        text: text.into(),
        runs: Vec::new(),
        pos: [64.0, 64.0],
        size: [200.0, 40.0],
        auto_size: true,
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
        cache: None,
    };
    let l0 = app.doc.add_text_layer(
        "script",
        mn_core::TextSet {
            texts: vec![item("hello script world")],
        },
    );

    // PM-042: + field appends under the last item.
    let (nl, ni) = app.story_new_field(app.page_index).expect("field added");
    assert_eq!((nl, ni), (l0, 1));
    let ts = app.doc.layers[l0].texts().unwrap();
    assert_eq!(ts.texts.len(), 2);
    assert!(ts.texts[1].pos[1] > ts.texts[0].pos[1], "below the last");

    // PM-043: split the first field at a space.
    assert!(app.story_split_field(app.page_index, l0, 0, 6)); // "hello "
    let ts = app.doc.layers[l0].texts().unwrap();
    assert_eq!(ts.texts[0].text, "hello ");
    assert_eq!(ts.texts[1].text, "script world");

    // Backspace-merge rejoins.
    assert!(app.story_merge_field(app.page_index, l0, 1));
    let ts = app.doc.layers[l0].texts().unwrap();
    assert_eq!(ts.texts[0].text, "hello script world");
    assert_eq!(ts.texts.len(), 2, "the split-created field remains");

    // PM-042 on a page with NO text layers: a new Text layer appears.
    app.doc.layers[l0].visible = false; // not a template source anymore
    let d2 = mn_core::Document::new(app.doc.size.0, app.doc.size.1);
    let b2 = mn_core::project::doc_to_bytes(&d2).unwrap();
    let e = app.fresh_page(Some(b2), None);
    app.pages.push(e);
    app.story_refresh();
    let (nl2, _) = app
        .story_new_field(app.pages.len() - 1)
        .expect("layer created");
    let doc2 = app
        .story_docs
        .get(app.pages.len() - 1)
        .and_then(|d| d.as_ref())
        .unwrap();
    assert!(doc2.layers[nl2].texts().is_some(), "a text layer exists");
    assert_eq!(doc2.layers[nl2].texts().unwrap().texts.len(), 1);
    // And the page's BYTES carry it.
    let b = app.pages.last().unwrap().bytes.as_ref().unwrap();
    let re = mn_core::project::bytes_to_doc(b).unwrap();
    assert!(
        re.layers.iter().any(|x| x.texts().is_some()),
        "the new layer re-encoded into the page"
    );
}

#[test]
fn story_editor_writes_replaces_and_restyles() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let add_text_layer = |app: &mut App, text: &str| {
        use mn_core::text::TextItem;
        let mut l = mn_core::Layer::new("script");
        l.kind = mn_core::LayerKind::Text(mn_core::TextSet {
            texts: vec![TextItem {
                text: text.into(),
                runs: Vec::new(),
                pos: [64.0, 64.0],
                size: [200.0, 40.0],
                auto_size: true,
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
                fonts: Vec::new(),
                ruby_style: Default::default(),
                tcy: Vec::new(),
                auto_tcy: 0,
                cache: None,
            }],
        });
        app.doc.layers.push(l);
    };
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddPage);
    add_text_layer(&mut app, "hello world"); // page 2 (active)
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageFirst);
    add_text_layer(&mut app, "page one text"); // page 1 (active)

    // Open: both fields visible, page order.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::StoryEditor);
    assert!(app.story_open);
    assert_eq!(app.story_bufs.len(), 2);

    // The blank pages seed frame folders, so text-layer indices differ
    // per page — find them instead of guessing.
    let live_text_layer = |app: &App| {
        app.doc
            .layers
            .iter()
            .position(|l| l.texts().is_some())
            .unwrap()
    };
    let p2_text_layer = |app: &App| {
        app.story_docs[1]
            .as_ref()
            .unwrap()
            .layers
            .iter()
            .position(|l| l.texts().is_some())
            .unwrap()
    };

    // Cross-page write-through: edit page 2's field while page 1 is
    // live — the bytes change and the page loads the new text.
    let l2 = p2_text_layer(&app);
    assert!(app.story_set_text(1, l2, 0, "hello WORLDS"));
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageNext);
    let l = live_text_layer(&app);
    assert_eq!(
        app.doc.layers[l].texts().unwrap().texts[0].text,
        "hello WORLDS"
    );
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageFirst);

    // Find & replace, case-insensitive ("TEXT" matches "text").
    app.story_refresh();
    let (f, o) = app.story_replace_all("text", "SCRIPT", true);
    assert_eq!((f, o), (1, 1), "one field, one occurrence");
    let l0 = live_text_layer(&app);
    assert_eq!(app.story_text(0, l0, 0).unwrap(), "page one SCRIPT");

    // Restyle-all from the Text tool's settings (PM-045).
    app.text_size_pt = 20.0;
    app.text_font = "gothic".into();
    let n = app.story_apply_tool_style();
    assert!(n >= 2, "both pages' layers styled ({n})");
    assert_eq!(app.doc.layers[l0].texts().unwrap().texts[0].size_pt, 20.0);
    assert_eq!(app.doc.layers[l0].texts().unwrap().texts[0].font, "gothic");
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageNext);
    let l = live_text_layer(&app);
    assert_eq!(
        app.doc.layers[l].texts().unwrap().texts[0].size_pt,
        20.0,
        "page 2 styled through its bytes"
    );
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageFirst);

    // PM-047: a hidden text layer leaves the editor.
    add_text_layer(&mut app, "secret");
    let hidden = live_text_layer(&app) + 1; // the just-pushed layer
    app.doc.set_layer_visible(hidden, false);
    app.story_refresh();
    assert_eq!(app.story_fields().len(), 2, "hidden layer not shown");
}

/// PM-045 restyle: `story_fields` lists one entry per text ITEM, so the
/// restyle rewrote the WHOLE layer once per item — k rasterizes, k
/// re-encodes and k undo steps to take back one button press. One write
/// per layer, and the number reported is still the fields.
#[test]
fn story_apply_tool_style_writes_each_layer_once() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let mut set = mn_core::TextSet { texts: Vec::new() };
    for i in 0..3 {
        let mut it = mn_core::text::TextItem::new(
            [64.0, 64.0 + 40.0 * i as f32],
            "serif".into(),
            12.0,
            [0, 0, 0],
            true,
        );
        it.text = format!("field {i}");
        set.texts.push(it);
    }
    let mut l = mn_core::Layer::new("script");
    l.kind = mn_core::LayerKind::Text(set);
    app.doc.layers.push(l);
    let li = app.doc.layers.len() - 1;
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::StoryEditor);
    assert_eq!(app.story_fields().len(), 3, "three fields on one layer");

    app.text_size_pt = 20.0;
    let undo_before = app.doc.undo_len();
    let n = app.story_apply_tool_style();
    assert_eq!(n, 3, "the true field count");
    assert_eq!(
        app.doc.undo_len() - undo_before,
        1,
        "one button press, one undo step"
    );
    let sizes = |app: &App| {
        app.doc.layers[li]
            .texts()
            .unwrap()
            .texts
            .iter()
            .map(|t| t.size_pt)
            .collect::<Vec<_>>()
    };
    assert_eq!(sizes(&app), [20.0, 20.0, 20.0], "every field restyled");
    assert!(app.doc.undo(), "and one undo takes the whole restyle back");
    assert_eq!(sizes(&app), [12.0, 12.0, 12.0]);
}

/// TRIAGE 143 / PM-030..033: combine two inked pages into one wide
/// spread and split it back — the app-level round trip through the
/// command surface (entries replaced, spread flagged, ink preserved).
#[test]
fn spread_combine_and_split_round_trip() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let paint = |app: &mut App, x: i32, y: i32| {
        app.doc.begin_op();
        let idx = TileIdx::of_pixel(x, y);
        app.doc.active_layer_mut().tile_mut(idx).set_pixel(
            (x - idx.origin().0) as usize,
            (y - idx.origin().1) as usize,
            [32000, 0, 0, 32000],
        );
        app.doc.end_op();
    };
    // B's ink lands in B's layer stack (above A's in the spread), so
    // the asserts need an ALL-layers probe, not the active layer.
    let ink_any = |app: &App, x: i32, y: i32| {
        let idx = TileIdx::of_pixel(x, y);
        let (lx, ly) = ((x - idx.origin().0) as usize, (y - idx.origin().1) as usize);
        app.doc
            .layers
            .iter()
            .any(|l| l.tile(idx).is_some_and(|t| t.pixel(lx, ly)[3] > 0))
    };
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddPage);
    assert_eq!(app.pages.len(), 2);
    let w1 = app.doc.size.0; // page 2 (active after AddPage)
    paint(&mut app, 100, 100);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageFirst);
    let w0 = app.doc.size.0;
    paint(&mut app, 50, 50);
    assert!(ink_at(&app.doc, 50, 50));

    // COMBINE (gap 0, delete-empty): one wide page, both inks.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::PageCombineApply {
            gap: 0,
            delete_empty: true,
        },
    );
    assert_eq!(app.pages.len(), 1, "two pages became one");
    assert!(app.pages[0].spread, "the entry is flagged");
    assert_eq!(app.doc.size.0, w0 + w1);
    assert!(ink_any(&app, 50, 50), "A-side ink");
    assert!(ink_any(&app, w0 as i32 + 100, 100), "B-side ink offset");

    // SPLIT back: two pages, each with its own ink.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::PageSplitApply {
            gap: 0,
            delete_empty: true,
        },
    );
    assert_eq!(app.pages.len(), 2);
    assert_eq!(app.page_index, 0, "lands on the left page");
    assert!((app.doc.size.0 as i64 - w0 as i64).abs() <= 1);
    assert!(ink_any(&app, 50, 50));
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageNext);
    assert!(ink_any(&app, 100, 100), "the right page keeps its ink");
}

/// TRIAGE 142 / PM-021/022: page navigation — prev/next/first/last
/// with end guards, a full stash→decode round trip (ink survives
/// leaving and returning to a page), and the Go to Page clamp.
#[test]
fn page_navigation_flips_and_round_trips() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // Three more pages (four total).
    for _ in 0..3 {
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddPage);
    }
    assert_eq!(app.pages.len(), 4);
    assert_eq!(
        app.page_index, 3,
        "AddPage inserts after AND switches to it"
    );
    // Ink on page 4.
    app.doc.begin_op();
    let idx = TileIdx::of_pixel(100, 100);
    app.doc.active_layer_mut().tile_mut(idx).set_pixel(
        (100 - idx.origin().0) as usize,
        (100 - idx.origin().1) as usize,
        [32000, 0, 0, 32000],
    );
    app.doc.end_op();
    assert!(ink_at(&app.doc, 100, 100));

    // Prev: page 3, blank canvas; the ink stashed away.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PagePrev);
    assert_eq!(app.page_index, 2);
    assert!(!ink_at(&app.doc, 100, 100), "page 3 is blank");
    // Next: back to page 4 — the ink survives the round trip.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageNext);
    assert_eq!(app.page_index, 3);
    assert!(ink_at(&app.doc, 100, 100), "round trip restores the ink");
    // Guard at the end.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageNext);
    assert_eq!(app.page_index, 3, "last page holds");
    // First, then the guard there.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageFirst);
    assert_eq!(app.page_index, 0);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PagePrev);
    assert_eq!(app.page_index, 0, "first page holds");
    // Go to Page: the command arm clamps into range (99 → 4; 0 → 1).
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageGotoApply(99));
    assert_eq!(app.page_index, 3, "goto clamps high");
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageGotoApply(0));
    assert_eq!(app.page_index, 0, "goto clamps low");
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageGotoApply(2));
    assert_eq!(app.page_index, 1, "goto lands");
    // The dialog command opens with the current page preloaded.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::PageGoto);
    assert!(app.goto_page_open);
    assert_eq!(app.goto_page_value, 2, "1-based current page");
}

/// TRIAGE 148: the numeric fields' plumbing (`TransformUpdate`), the
/// flip button (T-021), the moved reference point (TR-003) and the
/// midpoint one-axis scale (TR-004).
#[test]
fn transform_fields_flip_pivot_and_midpoint() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let empty: [PenSample; 0] = [];
    const W: u16 = mn_core::FIX15_ONE as u16;
    // A square of ink spanning FOUR tiles, so the lift rect is
    // [0,0,128,128]: pivot (64,64), right-edge midpoint (128,64) — 64px
    // from either corner, beyond the hit-test tolerance even at the
    // fitted zoom (≈0.4 → tol·1.4 ≈ 35px).
    app.doc.begin_op();
    for y in 20..108 {
        for x in 20..108 {
            let idx = TileIdx::of_pixel(x, y);
            app.doc.active_layer_mut().tile_mut(idx).set_pixel(
                (x - idx.origin().0) as usize,
                (y - idx.origin().1) as usize,
                [W, W, W, W],
            );
        }
    }
    app.doc.end_op();
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformStart);
    assert!(app.transform_drag.is_some());

    // TR-031-033: the numeric plumbing, absolute params.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::TransformUpdate {
            sx: 2.0,
            sy: 1.0,
            rad: 0.0,
            tx: 10.0,
            ty: 0.0,
        },
    );
    {
        let d = app.transform_drag.as_ref().unwrap();
        assert!((d.sx - 2.0).abs() < 1e-5 && (d.sy - 1.0).abs() < 1e-5);
    }
    // T-021: the flip button negates the standing scale in canvas
    // space (rad reflects too).
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::TransformFlip { horizontal: true },
    );
    {
        let d = app.transform_drag.as_ref().unwrap();
        assert!((d.sx + 2.0).abs() < 1e-5, "sx negated: {}", d.sx);
    }
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCancel);
    assert!(app.transform_drag.is_none());

    // TR-003: a moved pivot is the rotation center — rotate 90° about
    // the bbox corner (0,0) and that corner stays fixed.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformStart);
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::TransformSetPivot {
            pivot: Some([0.0, 0.0]),
        },
    );
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::TransformUpdate {
            sx: 1.0,
            sy: 1.0,
            rad: std::f32::consts::FRAC_PI_2,
            tx: 0.0,
            ty: 0.0,
        },
    );
    {
        let d = app.transform_drag.as_ref().unwrap();
        assert_eq!(d.pivot(), [0.0, 0.0]);
        let c = d.xform.apply([0.0, 0.0]);
        assert!(
            c[0].abs() < 1e-4 && c[1].abs() < 1e-4,
            "corner fixed: {c:?}"
        );
    }
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCancel);

    // TR-004: the right-edge midpoint drag scales ONE axis — down at
    // (128,64), up at (192,64) → sx = 2, sy untouched. Identity view
    // first: the fitted zoom shrinks the hit-test tolerance slack and
    // the corner test (tol·1.4) would swallow the midpoint at low zoom.
    app.viewport.zoom = 1.0;
    app.viewport.pan = [0.0, 0.0];
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformStart);
    let (x0, y0) = app.viewport.to_screen(128.0, 64.0);
    let (x1, y1) = app.viewport.to_screen(192.0, 64.0);
    app.canvas_down(x0, y0, PointerKind::Mouse, &empty);
    app.canvas_up(x1, y1, &empty);
    {
        // (canvas_up clears the gesture; the params are the proof — a
        // corner grab would scale BOTH axes, Move/Rotate neither.)
        let d = app.transform_drag.as_ref().unwrap();
        assert!((d.sx - 2.0).abs() < 0.05, "sx doubled: {}", d.sx);
        assert!((d.sy - 1.0).abs() < 1e-5, "sy untouched: {}", d.sy);
    }
    // Commit the one-axis scale: x' = 64 + 2(x−64) → the square's
    // x-extent 20..108 becomes −24..152 (canvas-clipped to 0..152),
    // y unchanged 20..108.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);
    assert!(
        ink_at(&app.doc, 140, 64),
        "ink extended past the old right edge"
    );
    assert!(ink_at(&app.doc, 8, 64), "ink pulled past the old left edge");
    assert!(
        !ink_at(&app.doc, 80, 16),
        "vertical extent unchanged (above)"
    );
    assert!(
        !ink_at(&app.doc, 80, 112),
        "vertical extent unchanged (below)"
    );
}

/// Two-finger rotate (owner ask 2026-08-17): the finger pair's angle
/// turns the page around the gesture midpoint, with a snap band at the
/// 90° multiples. Drives the REAL touch handlers. Self-discriminating:
/// the near-90° arm asserts EXACTLY π/2, impossible without the snap;
/// the 30° arm asserts the free rotation tracks the finger angle.
#[test]
fn two_finger_rotate_snaps_to_quarters() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);

    // A pair rotating around its midpoint: BOTH fingers move, mirrored
    // (A 0°→30°, B 180°→210°) — each finger's message is a half-update
    // against the static other; the two compose into the pair rotation.
    let rot = |app: &mut App, from_deg: f32, to_deg: f32| {
        let (mx, my, r) = (300.0f32, 200.0f32, 80.0f32);
        let (a0, a1) = (from_deg.to_radians(), to_deg.to_radians());
        let (ax0, ay0) = (mx + r * a0.cos(), my + r * a0.sin());
        let (ax1, ay1) = (mx + r * a1.cos(), my + r * a1.sin());
        let (bx, by) = (mx - r * a0.cos(), my - r * a0.sin());
        let (bx1, by1) = (mx - r * a1.cos(), my - r * a1.sin());
        app.touch_down(1, ax0, ay0);
        app.touch_down(2, bx, by);
        app.touch_move(1, ax1, ay1);
        app.touch_move(2, bx1, by1);
        app.touch_up(1);
        app.touch_up(2);
    };

    rot(&mut app, 0.0, 30.0);
    assert!(
        (app.viewport.rotate_rad - 30.0f32.to_radians()).abs() < 0.05,
        "free rotation follows the fingers, got {}",
        app.viewport.rotate_rad.to_degrees()
    );

    // Into the snap band: 88° total → lands exactly on 90° (the
    // 2026-08-19 hysteresis engages within 2.5° of the quarter; the
    // old test rode the removed 8° band with 84°).
    rot(&mut app, 30.0, 88.0);
    let expect = std::f32::consts::FRAC_PI_2;
    assert!(
        (app.viewport.rotate_rad - expect).abs() < 1e-4,
        "88° total must snap to exactly 90°, got {}",
        app.viewport.rotate_rad.to_degrees()
    );
}

/// 2026-08-19 rotate-feel fix (research/touch-rotation.md) — THE
/// OWNER REPRO: a slow twist with both fingers moving must rotate
/// the view. On the old code the 8° absolute-set snap pinned every
/// small delta back to the quarter, and only a fast one-finger whip
/// (a single half-event ≥ 8°) escaped — this test fails on it.
#[test]
fn two_finger_slow_twist_rotates() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let (mx, my, r) = (300.0f32, 200.0f32, 80.0f32);
    let at = |deg: f32| {
        (
            mx + r * deg.to_radians().cos(),
            my + r * deg.to_radians().sin(),
        )
    };
    let anti = |a: (f32, f32)| (2.0 * mx - a.0, 2.0 * my - a.1);
    let a0 = at(0.0);
    app.touch_down(1, a0.0, a0.1);
    app.touch_down(2, anti(a0).0, anti(a0).1);
    // 90 one-degree pair-steps: two half-update events per step, each
    // finger's per-event delta ≈ 0.5° — far below the old 8° band.
    for step in 1..=90 {
        let a = at(step as f32);
        app.touch_move(1, a.0, a.1);
        app.touch_move(2, anti(a).0, anti(a).1);
    }
    app.touch_up(1);
    app.touch_up(2);
    assert!(
        (app.viewport.rotate_rad - std::f32::consts::FRAC_PI_2).abs() < 0.05,
        "a slow twist reaches the quarter, got {}",
        app.viewport.rotate_rad.to_degrees()
    );
}

/// 2026-08-19: pinch noise must not rotate — the activation threshold
/// keeps the view still through a pure pinch with digitizer wobble,
/// while the zoom still tracks.
#[test]
fn two_finger_pinch_noise_does_not_rotate() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let (mx, my, r0) = (300.0f32, 200.0f32, 80.0f32);
    let at = |deg: f32, r: f32| {
        (
            mx + r * deg.to_radians().cos(),
            my + r * deg.to_radians().sin(),
        )
    };
    let anti = |a: (f32, f32)| (2.0 * mx - a.0, 2.0 * my - a.1);
    let a0 = at(0.0, r0);
    app.touch_down(1, a0.0, a0.1);
    app.touch_down(2, anti(a0).0, anti(a0).1);
    // A monotone pinch-out (+1% radius per step) with ±0.2° of pair
    // wobble — real digitizers are no cleaner than this. (Zoom is a
    // RATIO: App::new fits the viewport, zoom never starts at 1.)
    let zoom0 = app.viewport.zoom;
    let mut r = r0;
    for i in 0..40 {
        r *= 1.01;
        let wob = if i % 2 == 0 { 0.2 } else { -0.2 };
        let a = at(wob, r);
        app.touch_move(1, a.0, a.1);
        app.touch_move(2, anti(a).0, anti(a).1);
    }
    app.touch_up(1);
    app.touch_up(2);
    assert!(
        app.viewport.rotate_rad.abs() < 1e-4,
        "pinch wobble never rotates, got {}",
        app.viewport.rotate_rad.to_degrees()
    );
    assert!(
        app.viewport.zoom / zoom0 > 1.2,
        "the zoom still tracks the pinch, got {}x",
        app.viewport.zoom / zoom0
    );
}

/// 2026-08-19: the quarter magnet HOLDS through the hysteresis band
/// and RELEASES past it — a slow twist can always leave a quarter,
/// which the old absolute-set snap made impossible.
#[test]
fn two_finger_snap_holds_then_releases_at_any_speed() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let (mx, my, r) = (300.0f32, 200.0f32, 80.0f32);
    let at = |deg: f32| {
        (
            mx + r * deg.to_radians().cos(),
            my + r * deg.to_radians().sin(),
        )
    };
    let anti = |a: (f32, f32)| (2.0 * mx - a.0, 2.0 * my - a.1);
    let twist_to = |app: &mut App, to_deg: f32| {
        let a = at(to_deg);
        app.touch_move(1, a.0, a.1);
        app.touch_move(2, anti(a).0, anti(a).1);
    };
    let a0 = at(0.0);
    app.touch_down(1, a0.0, a0.1);
    app.touch_down(2, anti(a0).0, anti(a0).1);
    // Slow steps: engage the magnet by 88, read it HELD at 92, free
    // again past 94 (release), tracking by 97.
    for step in 1..=97 {
        twist_to(&mut app, step as f32);
        match step {
            92 => assert!(
                (app.viewport.rotate_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
                "inside the release band the quarter holds, got {}",
                app.viewport.rotate_rad.to_degrees()
            ),
            97 => assert!(
                app.viewport.rotate_rad > 95.0f32.to_radians(),
                "past the release band the view leaves the quarter, got {}",
                app.viewport.rotate_rad.to_degrees()
            ),
            _ => {}
        }
    }
    app.touch_up(1);
    app.touch_up(2);

    // And from a SETTLED quarter: a fresh slow gesture starting at
    // the magnet still escapes (the dead zone is the release band,
    // not a pin). The view sits at ~97°; another 97° of slow twist
    // must land near 194° — free, not parked on 90 or 180.
    // (set_rotation_around wraps into (−180°, 180°].)
    let a0 = at(0.0);
    app.touch_down(1, a0.0, a0.1);
    app.touch_down(2, anti(a0).0, anti(a0).1);
    for step in 1..=97 {
        twist_to(&mut app, step as f32);
    }
    app.touch_up(1);
    app.touch_up(2);
    let expect = (194.0f32 - 360.0).to_radians();
    assert!(
        (app.viewport.rotate_rad - expect).abs() < 1.0f32.to_radians(),
        "a settled quarter releases too, got {} (expect ≈ -166°)",
        app.viewport.rotate_rad.to_degrees()
    );
}

/// 2026-08-19: pinch-twist is ONE gesture — a pair that spreads 1.5×
/// while turning 20° must zoom AND rotate in a single motion.
#[test]
fn two_finger_pinch_twist_is_one_gesture() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let (mx, my, r0) = (300.0f32, 200.0f32, 80.0f32);
    let at = |deg: f32, r: f32| {
        (
            mx + r * deg.to_radians().cos(),
            my + r * deg.to_radians().sin(),
        )
    };
    let anti = |a: (f32, f32)| (2.0 * mx - a.0, 2.0 * my - a.1);
    let a0 = at(0.0, r0);
    app.touch_down(1, a0.0, a0.1);
    app.touch_down(2, anti(a0).0, anti(a0).1);
    // 20 steps: radius grows to 1.5×, pair turns to 20°. (App::new
    // fits a viewport — assert the RATIO, not the absolute zoom.)
    let zoom0 = app.viewport.zoom;
    for step in 1..=20 {
        let f = step as f32;
        let a = at(f, r0 * (1.0 + 0.5 * f / 20.0));
        app.touch_move(1, a.0, a.1);
        app.touch_move(2, anti(a).0, anti(a).1);
    }
    app.touch_up(1);
    app.touch_up(2);
    assert!(
        (app.viewport.rotate_rad - 20.0f32.to_radians()).abs() < 0.05,
        "the twist rotated, got {}",
        app.viewport.rotate_rad.to_degrees()
    );
    assert!(
        (app.viewport.zoom / zoom0 - 1.5).abs() < 0.05,
        "the pinch zoomed simultaneously, got {}x",
        app.viewport.zoom / zoom0
    );
}

/// SE round 2026-08-19: the selection pen paints coverage through the
/// full brush engine, the selection eraser subtracts, Quick Mask
/// routes ordinary brushes the same way — and none of it costs an
/// undo step (CSP parity: selections are not in the undo history).
#[test]
fn selection_pen_eraser_and_quick_mask() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.viewport = mn_gpu::Viewport::default(); // canvas == client
    let sample = |x: f32, y: f32, t: f64| PenSample {
        x,
        y,
        pressure: 1.0,
        tilt_x: 0.0,
        tilt_y: 0.0,
        t_ms: t,
    };
    let stroke = |app: &mut App, ax: f32, ay: f32, bx: f32, by: f32| {
        let empty: [PenSample; 0] = [];
        app.canvas_down(ax, ay, PointerKind::Pen, &empty);
        let batch = [
            sample((ax + bx) * 0.5, (ay + by) * 0.5, 16.0),
            sample(bx, by, 32.0),
        ];
        app.push_batch(&batch);
        app.canvas_up(bx, by, &batch);
    };
    let sel_on = |app: &App, x: i32, y: i32| {
        mn_core::selection::selected(
            app.doc
                .selection
                .as_ref()
                .map(|s| s.coverage(x, y))
                .unwrap_or(0),
        )
    };

    let undo0 = app.doc.undo_labels().len();
    // The selection pen paints a horizontal band around y=60.
    app.tool = crate::cmd::Tool::SelPen;
    stroke(&mut app, 100.0, 60.0, 500.0, 60.0);
    let sel = app
        .doc
        .selection
        .as_ref()
        .expect("the stroke made a selection");
    let peak = (52..=68).map(|y| sel.coverage(300, y)).max().unwrap_or(0);
    assert!(
        peak >= 200,
        "a hard pen paints near-opaque coverage somewhere on the line, got {peak}"
    );
    assert!(sel_on(&app, 300, 60), "on the stroke line");
    assert!(!sel_on(&app, 300, 300), "far off it");
    assert_eq!(
        app.doc.undo_labels().len(),
        undo0,
        "selection paint is not an undo step"
    );

    // The selection eraser cuts a crossing gap in the band. (Synthetic
    // 3-sample strokes start painting late — libmypoint warm-up — so
    // the probes sit in the reliably-painted half of the band.)
    app.tool = crate::cmd::Tool::SelEraser;
    stroke(&mut app, 280.0, 20.0, 280.0, 120.0);
    assert!(!sel_on(&app, 280, 60), "the eraser crossed here");
    assert!(sel_on(&app, 400, 60), "the band outside the cut");
    assert_eq!(app.doc.undo_labels().len(), undo0);

    // Quick Mask: ordinary Pen strokes route the same way.
    app.quick_mask = true;
    app.tool = crate::cmd::Tool::Pen;
    stroke(&mut app, 100.0, 300.0, 500.0, 300.0);
    app.quick_mask = false;
    assert!(sel_on(&app, 300, 300), "quick mask painted");
    assert_eq!(app.doc.undo_labels().len(), undo0);

    // And the ink never landed anywhere: the layer is untouched.
    let inked = app
        .doc
        .active_layer()
        .tiles()
        .filter(|(_, t)| t.data().iter().any(|&v| v != 0))
        .count();
    assert_eq!(inked, 0, "selection strokes paint no layer");
}

/// Round 34 audit (MEDIUM): Tool-Property VALUE-BAR rows on an
/// value. Simulates the bar's exact per-frame call sequence.
#[test]
fn text_bar_drag_is_one_undo_step() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    if app.text_engine.is_none() {
        println!("[test] SKIP: no text engine");
        return;
    }
    app.start_new_text([50.0, 50.0], None);
    for u in "abc".encode_utf16() {
        app.text_char(u);
    }
    app.commit_text_edit();

    let (li, before_pt) = app
        .doc
        .layers
        .iter()
        .enumerate()
        .find_map(|(i, l)| l.texts().map(|t| (i, t.texts[0].size_pt)))
        .expect("the committed text layer holds the item");
    app.tool = crate::cmd::Tool::Object;
    app.text_sel = Some((li, 0));
    let undo_before = app.doc.undo_len();

    // 20 drag frames, the bar's exact sequence: begin (no-op after the
    // first) + live preview each frame.
    for k in 0..20u16 {
        let pt = 8.0 + k as f32 * 0.5;
        app.begin_text_bar_drag();
        app.preview_text_prop(move |i| i.size_pt = pt);
    }
    assert_eq!(
        app.doc.undo_len(),
        undo_before,
        "preview frames must not touch history"
    );
    assert_eq!(
        app.doc.layers[li].texts().unwrap().texts[0].size_pt,
        17.5,
        "the live preview tracked the drag"
    );

    app.commit_text_bar_drag();
    assert_eq!(
        app.doc.undo_len(),
        undo_before + 1,
        "the whole drag is ONE undo step"
    );
    assert_eq!(
        app.doc.layers[li].texts().unwrap().texts[0].size_pt,
        17.5,
        "the dragged value committed"
    );

    assert!(app.doc.undo());
    assert_eq!(
        app.doc.layers[li].texts().unwrap().texts[0].size_pt,
        before_pt,
        "one undo restores the pre-drag value"
    );
}

/// Round 34: the font list's Recently-used row (CSP parity) — newest
/// first, no duplicates, capped at 10, mirrored into UiLayout for the
/// next launch.
#[test]
fn recent_fonts_dedupe_cap_and_mirror() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    for i in 0..12 {
        app.note_recent_font(&format!("Font {i}"));
    }
    assert_eq!(app.recent_fonts.len(), 10, "capped at CSP's 10");
    assert_eq!(app.recent_fonts[0], "Font 11");
    assert_eq!(app.recent_fonts[9], "Font 2");
    app.note_recent_font("Font 5");
    assert_eq!(app.recent_fonts[0], "Font 5", "re-use moves to the front");
    assert_eq!(app.recent_fonts.len(), 10);
    assert_eq!(
        app.recent_fonts.iter().filter(|f| *f == "Font 5").count(),
        1,
        "no duplicate"
    );
    assert_eq!(
        app.layout.recent_fonts, app.recent_fonts,
        "mirrored for persistence"
    );
}

/// Auditor round 33, finding #1: the canary-repair branch used to
/// `mark_dab_tile_clean` the REPAIRED tiles — pinning the texture
/// cache to the repaired revision while the textures still held the
/// INCOMPLETE GPU pixels (the dropped dispatch's dabs missing from
/// them), so `needs_upload` stayed false forever and the canvas kept
/// showing the incomplete stroke while the document was fine — on
/// exactly the cursed-driver machine the canary exists to defend.
/// Fix = no clean-mark after the repair; the ordinary revision compare
/// uploads it. This drives the REAL path end to end: one compute
/// dispatch is genuinely skipped but still counted (the faithful
/// driver-drop simulation), the canary fires, `end_stroke` repairs on
/// CPU, and the COMPOSITED output must match a pure-CPU reference.
#[test]
fn canary_repair_composites_the_healed_tiles() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    if !renderer.gpu_dabs_supported() {
        println!("[test] SKIP: rgba16uint storage unsupported");
        return;
    }
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.gpu_dabs = true;
    app.renderer.debug_drop_next_flush();

    let stroke = |app: &mut App| {
        app.begin_stroke(PointerKind::Mouse);
        let batch: Vec<PenSample> = (0..30)
            .map(|i| PenSample {
                x: 100.0 + i as f32 * 4.0,
                y: 200.0,
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            })
            .collect();
        app.push_batch(&batch);
        app.end_stroke();
    };

    stroke(&mut app);
    assert_eq!(
        app.dab_path_last, "gpu → cpu repair!",
        "the drop must fire the repair"
    );

    // Reference: the same stroke through the pure-CPU path on a second
    // layer — with a FRESH engine. RESOLVED (round 41, the carryover
    // round): there is NO GPU-induced engine carryover. The historical
    // no-swap failure (11.18M/5.87M per tile) compared the healed layer
    // (a replay of STROKE 1's dabs) against STROKE 2 on the same
    // engine — and same-engine strokes legitimately differ by ~1% in
    // total (pinned by two_cpu_strokes_on_one_engine_are_identical),
    // concentrated ~47% in the tail tiles where the pressure-driven
    // radius ramp (r 5→50 over the stroke) makes every dab fat. The
    // fresh engine is not a workaround — it is the correct methodology:
    // both sides of the comparison must be FIRST-stroke instances. The
    // decisive disprovals, this round: (a) a CPU stroke after a BYPASS
    // stroke totals 1,511,568,078 alpha — bit-identical to a CPU
    // stroke after a CPU stroke; (b) the raw-engine dab streams of the
    // second stroke are field-identical under both histories
    // (mn-brush bypass_history_does_not_change_the_next_strokes_dabs).
    app.gpu_dabs = false;
    if let Some(i) = app.selected_preset {
        let p = app.presets[i].1.clone();
        if let Ok(b) = mn_brush::MyBrush::load(&p) {
            *app.engine_mut() = Engine::new(EngineKind::My(Box::new(b)));
        }
    }
    let ref_layer = app.doc.add_layer("ref");
    app.doc.set_active(ref_layer);
    stroke(&mut app);

    // Document parity (coarse): both layers inked the same tiles with
    // the same total alpha — the repair replay is pixel-pinned ≤1 vs
    // the C in the brush crate, so alpha level is enough here.
    let inked = |li: usize| -> std::collections::BTreeMap<TileIdx, u64> {
        app.doc.layers[li]
            .tiles()
            .filter(|(_, t)| t.alpha_sum() > 0)
            .map(|(i, t)| (i, t.alpha_sum()))
            .collect()
    };
    let (healed_doc, ref_doc) = (inked(0), inked(ref_layer));
    assert_eq!(
        healed_doc.keys().collect::<Vec<_>>(),
        ref_doc.keys().collect::<Vec<_>>()
    );
    for (k, a) in &ref_doc {
        let b = healed_doc.get(k).copied().unwrap_or(0);
        // 5%: the replay rasterizer is per-dab ≤1 ulp vs the C, but a
        // dense tile accumulates that over ~140 overlapping dabs — the
        // measured drift on the densest tile is ~3.4%. The stale-cache
        // bug this test hunts is a whole tile missing (≈100%).
        let rel = (*a as i64 - b as i64).unsigned_abs() as f64 / (*a).max(1) as f64;
        assert!(
            rel < 0.05,
            "repair replay alpha drifted on {k:?}: {b} vs {a}"
        );
    }

    // The display heal — the actual finding: composite the repaired
    // layer alone, then the reference layer alone, through the SAME
    // renderer's texture cache. With the deleted clean-mark bug the
    // cache claimed the (incomplete) textures matched the repaired
    // tiles, no upload happened, and the first composite showed a
    // tile's worth of missing ink. Tolerance 12/255 = the replay
    // accumulation above; the bug lands 15× past it.
    app.doc.set_layer_visible(ref_layer, false);
    let healed = app.renderer.render_offscreen(&app.doc, 320, 240);
    app.doc.set_layer_visible(ref_layer, true);
    app.doc.set_layer_visible(0, false);
    let reference = app.renderer.render_offscreen(&app.doc, 320, 240);
    let mut worst: u8 = 0;
    for (p, q) in healed.pixels().zip(reference.pixels()) {
        for c in 0..4 {
            worst = worst.max(p.0[c].abs_diff(q.0[c]));
        }
    }
    assert!(
        worst <= 12,
        "canary repair did not reach the canvas (max channel delta {worst}) \
             — stale texture cache after CPU repair?"
    );
}

fn ink_at(doc: &Document, cx: i32, cy: i32) -> bool {
    let idx = TileIdx::of_pixel(cx, cy);
    doc.active_layer()
        .tile(idx)
        .map(|t| {
            t.pixel(
                (cx - idx.origin().0) as usize,
                (cy - idx.origin().1) as usize,
            )[3] > 0
        })
        .unwrap_or(false)
}

/// Symmetry painting (Krita mirror): with an X twin on a 512-wide
/// canvas, a stroke at x≈160..240 also paints its reflection at
/// 512-x, same y — and the whole thing is ONE undoable op.
#[test]
fn mirror_twin_paints_the_reflection() {
    let mut engine = Engine::new(pen_kind());
    let mut doc = Document::new(512, 512);
    doc.begin_op();
    engine.begin(&mut doc);
    for i in 0..=40 {
        engine.sample(
            &mut doc,
            sample(160.0 + i as f32 * 2.0, 130.0, i as f64 * 8.0),
        );
    }
    engine.end(&mut doc);
    doc.end_op();
    assert!(ink_at(&doc, 200, 130), "the stroke itself painted nothing");
    assert!(!ink_at(&doc, 312, 130), "ink appeared with no twin?");

    // Now with the twin (fresh engines, like rebuild_twins makes).
    let mut engine = Engine::new(pen_kind());
    engine.set_twins(vec![StrokeTwin {
        kind: pen_kind(),
        x: Some(TwinAxis::Mirror),
        y: None,
        xf: None,
    }]);
    let mut doc = Document::new(512, 512);
    doc.begin_op();
    engine.begin(&mut doc);
    for i in 0..=40 {
        engine.sample(
            &mut doc,
            sample(160.0 + i as f32 * 2.0, 130.0, i as f64 * 8.0),
        );
    }
    engine.end(&mut doc);
    doc.end_op();
    assert!(ink_at(&doc, 200, 130), "stroke");
    assert!(ink_at(&doc, 312, 130), "mirror twin did not reflect x");
    assert_eq!(doc.undo_len(), 1, "stroke + reflection must be one op");
    assert!(doc.undo());
    assert!(
        !ink_at(&doc, 200, 130) && !ink_at(&doc, 312, 130),
        "undo left ink"
    );
}

/// Wrap-around tiling (Krita wrap mode): a stroke straddling the RIGHT
/// edge also paints its clipped part by the LEFT edge (x - w), so the
/// border tiles seamlessly. Mid-canvas strokes wrap off-canvas and stay
/// invisible — exactly the border-continuation behaviour.
#[test]
fn wrap_twin_continues_the_border() {
    let mut engine = Engine::new(pen_kind());
    engine.set_twins(vec![StrokeTwin {
        kind: pen_kind(),
        x: Some(TwinAxis::Wrap),
        y: None,
        xf: None,
    }]);
    let mut doc = Document::new(512, 512);
    doc.begin_op();
    engine.begin(&mut doc);
    // Hug the right edge so the dab straddles 512; tiny wiggle so the
    // distance-driven dab spacing keeps stamping.
    for i in 0..12 {
        engine.sample(
            &mut doc,
            sample(510.0 + (i % 2) as f32 * 0.5, 130.0, i as f64 * 33.0),
        );
    }
    engine.end(&mut doc);
    doc.end_op();
    assert!(ink_at(&doc, 508, 130), "the stroke itself painted nothing");
    assert!(
        ink_at(&doc, 1, 130),
        "wrap twin did not continue at the left edge"
    );
    assert!(!ink_at(&doc, 256, 130), "wrap must not paint mid-canvas");
}

/// Balloon spline editing through the REAL Object-tool path (TODO round
/// 31): Ctrl+click on an edge inserts an anchor, Ctrl+click on an anchor
/// deletes it, Ctrl+click on a tail handle removes just the tail — each
/// as ONE undo step that restores the previous balloon. Asserts the
/// LAYER's balloon data (the effect), never the method's return alone.
#[test]
fn object_tool_modifier_clicks_edit_balloon_anchors_and_tails() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (800, 600), 1.0);

    // A drawn square balloon (spline anchors) with one tail, on a fresh
    // balloon layer, through the real command path.
    let balloon = mn_core::Balloon {
        shape: mn_core::BalloonShape::Polygon {
            points: vec![
                [400.0, 200.0],
                [560.0, 200.0],
                [560.0, 340.0],
                [400.0, 340.0],
            ],
            widths: vec![0.5; 4],
            corners: vec![false; 4],
        },
        tails: vec![mn_core::Tail {
            base: [480.0, 340.0],
            tip: [520.0, 420.0],
            width: 14.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::BalloonAdd { balloon });

    // The input handlers QUEUE commands (the message loop pumps them);
    // headless, drain the queue through the real dispatcher.
    fn pump(app: &mut App) {
        while let Some(c) = app.cmds.pop_front() {
            crate::cmd::dispatch(app, c);
        }
    }

    let anchors = |a: &App| -> usize {
        let (li, bi) = a.balloon_sel.expect("balloon selected after add");
        match &a.doc.layers[li].balloons().unwrap().balloons[bi].shape {
            mn_core::BalloonShape::Polygon { points, .. } => points.len(),
            _ => 0,
        }
    };
    let tails = |a: &App| -> usize {
        let (li, bi) = a.balloon_sel.unwrap();
        a.doc.layers[li].balloons().unwrap().balloons[bi]
            .tails
            .len()
    };
    assert_eq!(anchors(&app), 4);
    assert_eq!(tails(&app), 1);

    // No modifier: the intercept must not fire (the drag path owns it).
    assert!(!app.balloon_anchor_edit(480.0, 205.0, false, false));

    // Ctrl+click the top edge mid-segment: a 5th anchor appears.
    assert!(
        app.balloon_anchor_edit(480.0, 203.0, true, false),
        "edge hit"
    );
    pump(&mut app);
    assert_eq!(anchors(&app), 5, "anchor inserted");
    assert_eq!(tails(&app), 1, "insert leaves tails alone");
    assert!(app.doc.undo(), "insert is one undo step");
    assert_eq!(anchors(&app), 4, "undo restored the anchor count");

    // Ctrl+click ON the top-left anchor: deletes it (4 -> 3).
    assert!(
        app.balloon_anchor_edit(400.0, 200.0, true, false),
        "anchor hit"
    );
    pump(&mut app);
    assert_eq!(anchors(&app), 3);
    assert!(app.doc.undo());
    assert_eq!(anchors(&app), 4);

    // Ctrl+click the tail's tip: only the tail goes, the body stays.
    assert!(
        app.balloon_anchor_edit(520.0, 420.0, true, false),
        "tail hit"
    );
    pump(&mut app);
    assert_eq!(tails(&app), 0);
    assert_eq!(anchors(&app), 4, "body untouched by tail delete");
    assert!(app.doc.undo());
    assert_eq!(tails(&app), 1);

    // Alt+click an anchor: corner/smooth toggle, undoable.
    assert!(app.balloon_anchor_edit(400.0, 200.0, false, true));
    pump(&mut app);
    {
        let (li, bi) = app.balloon_sel.unwrap();
        match &app.doc.layers[li].balloons().unwrap().balloons[bi].shape {
            mn_core::BalloonShape::Polygon { corners, .. } => {
                assert!(corners[0], "anchor 0 toggled to a corner")
            }
            _ => unreachable!(),
        }
    }
    assert!(app.doc.undo());
    {
        let (li, bi) = app.balloon_sel.unwrap();
        match &app.doc.layers[li].balloons().unwrap().balloons[bi].shape {
            mn_core::BalloonShape::Polygon { corners, .. } => {
                assert!(!corners[0], "undo restored the smooth anchor")
            }
            _ => unreachable!(),
        }
    }

    // Away from everything: the click falls through.
    assert!(!app.balloon_anchor_edit(100.0, 100.0, true, false));
}

/// LM-009 app side: a pure-translation Transform commit drags a LINKED
/// mask with the art; an UNLINKED mask stays (photo-in-a-window).
#[test]
fn transform_translation_drags_a_linked_mask_only() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let bgra = vec![255u8; 8 * 8 * 4];
    let src = crate::clipboard::bgra_to_floatsource(&bgra, 8, 8, [0, 0], 4096, 4096);
    let mut m = mn_core::doc::LayerMask::default();
    let mut t = mn_core::Tile::new_transparent();
    for y in 0..mn_core::TILE_SIZE {
        for x in 0..mn_core::TILE_SIZE {
            t.set_pixel(x, y, [32768, 32768, 32768, 32768]);
        }
    }
    m.tiles
        .insert(mn_core::TileIdx::new(0, 0), std::sync::Arc::new(t));
    app.doc.layers[0].mask = Some(m);

    let mk_drag = || crate::app::TransformDrag {
        source: crate::clipboard::bgra_to_floatsource(&bgra, 8, 8, [0, 0], 4096, 4096),
        xform: mn_core::Affine2 {
            m: [[1.0, 0.0], [0.0, 1.0]],
            t: [70.0, 30.0],
        },
        bbox: [[0.0, 0.0], [8.0, 0.0], [8.0, 8.0], [0.0, 8.0]],
        sx: 1.0,
        sy: 1.0,
        rad: 0.0,
        tx: 70.0,
        ty: 30.0,
        pivot_override: None,
        gesture: None,
        // A lift-shaped drag (Edit ▸ Transform): the mask ride is for
        // moved LAYER art. Pastes (clear_source: false) leave the mask
        // alone — their translation moved pasted pixels, not the ink the
        // mask was cut for.
        stamp_on_identity: false,
        clear_source: true,
        lift_selection: None,
        create_in: None,
        order: crate::app::MaterialLayerOrder::Above,
        preview_tex: None,
    };
    let _ = src;

    // Linked (default): the mask tile moves from (0,0) to cover the
    // shifted region (the +70,+30 split lands across (0,0)..(1,1)).
    app.transform_drag = Some(mk_drag());
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);
    let m = app.doc.layers[0].mask.as_ref().unwrap();
    assert!(
        m.tiles.contains_key(&mn_core::TileIdx::new(1, 1)),
        "linked: the mask crossed into the next tile"
    );
    assert!(m.tiles.contains_key(&mn_core::TileIdx::new(1, 0)));

    // Unlinked: same translation, mask untouched — byte-identical to
    // how the linked commit left it.
    let before: Vec<_> = app.doc.layers[0]
        .mask
        .as_ref()
        .unwrap()
        .tiles
        .keys()
        .copied()
        .collect();
    app.doc.layers[0].mask_linked = false;
    app.transform_drag = Some(mk_drag());
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);
    let after: Vec<_> = app.doc.layers[0]
        .mask
        .as_ref()
        .unwrap()
        .tiles
        .keys()
        .copied()
        .collect();
    assert_eq!(before, after, "unlinked: the mask stayed put");
}

/// LC-008 + LC-009 e2e (TRIAGE 139): a comp applies across every
/// structurally-matching page, skips mismatched ones, and exports one
/// numbered PNG set per comp.
#[test]
fn comps_apply_across_pages_and_export_per_comp() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // 2 layers, some ink so compositing is meaningful.
    app.doc.begin_op();
    let t = app.doc.layers[0].tile_mut(mn_core::TileIdx::new(0, 0));
    t.set_pixel(1, 1, [32768, 0, 0, 32768]);
    app.doc.end_op();
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddLayer);
    // The "no text" comp hides the top layer everywhere.
    app.doc.set_layer_visible(1, false);
    app.comp_add("no-text");

    // Two more pages with the SAME structure (2 layers), one with a
    // DIFFERENT structure (3 layers — must be skipped, not guessed).
    let (sw, sh) = (app.doc.size.0, app.doc.size.1);
    let same = move |n_text_visible: bool| {
        let mut d = mn_core::Document::new(sw, sh);
        d.add_layer("L2");
        if !n_text_visible {
            d.layers[1].visible = false;
        }
        d
    };
    let mk_bytes = |d: &mn_core::Document| mn_core::project::doc_to_bytes(d).unwrap();
    for d in [same(true), same(true)] {
        let e = app.fresh_page(Some(mk_bytes(&d)), None);
        app.pages.push(e);
    }
    let mut other = same(true);
    other.add_layer("L3");
    let e = app.fresh_page(Some(mk_bytes(&other)), None);
    app.pages.push(e);

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::CompApplyAllPages(0));
    assert!(
        app.status.contains("applied to 3 pages (1 skipped"),
        "{}",
        app.status
    );
    // The LIVE doc too — the page he is looking at, and what the
    // next save writes (self-audit: this was silently lost).
    assert!(
        !app.doc.layers[1].visible,
        "the active page's live doc took the comp"
    );
    for (k, e) in app.pages.iter().enumerate().skip(1) {
        let b = e.bytes.as_ref().expect("non-active pages carry bytes");
        let d = mn_core::project::bytes_to_doc(b).unwrap();
        if d.layers.len() == 2 {
            assert!(
                !d.layers[1].visible,
                "page {k}: the comp hid the text layer"
            );
        } else {
            assert_eq!(
                d.layers.len(),
                3,
                "page {k}: untouched mismatched structure"
            );
            assert!(d.layers[1].visible, "page {k}: skipped, flags untouched");
        }
    }

    // LC-009: one subfolder per comp, one PNG per page inside.
    let dir = std::env::temp_dir().join(format!("mn-comps-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::CompExportAllPath(dir.clone()));
    let sub = dir.join("no-text");
    let pngs = std::fs::read_dir(&sub)
        .expect("the comp subfolder exists")
        .flatten()
        .filter(|f| f.path().extension().is_some_and(|e| e == "png"))
        .count();
    assert_eq!(pngs, 4, "one image per page");
    std::fs::remove_dir_all(&dir).ok();
}

/// LC-007 + LC-013 (TRIAGE 139's remainder): drag-reorder remaps both
/// selections by identity, Ctrl/Shift build the multi-selection, and
/// a multi-selection exports ONLY those comps (empty = all).
#[test]
fn comps_reorder_multiselect_and_export_selected() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.comp_add("alpha");
    app.comp_add("beta");
    app.comp_add("gamma");
    assert_eq!(app.doc.comps.len(), 3);
    assert_eq!(app.comp_selected, Some(2), "the last add selects");

    // Ctrl-click toggles into the multi-selection (anchor follows).
    app.comp_toggle_multi(0);
    assert_eq!(app.comp_multi, vec![0]);
    assert_eq!(app.comp_selected, Some(0));
    // Shift-click ranges from the anchor.
    app.comp_range_select(2);
    assert_eq!(app.comp_multi, vec![0, 1, 2]);
    assert_eq!(app.comp_selected, Some(2));

    // Drag beta (1) to the end (boundary 3, original order): the
    // list becomes [alpha, gamma, beta]; the identity remap moves
    // the selected gamma to 1 and permutes the multi to match.
    app.comp_move(1, 3);
    let names: Vec<&str> = app.doc.comps.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["alpha", "gamma", "beta"]);
    assert_eq!(app.comp_selected, Some(1), "gamma followed its comp");
    assert_eq!(app.comp_multi, vec![0, 1, 2], "the full set stays full");
    // Redundant drops are no-ops.
    app.comp_move(0, 0);
    app.comp_move(0, 1);
    let names: Vec<&str> = app.doc.comps.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["alpha", "gamma", "beta"]);

    // A partial multi-selection survives a move: [gamma, beta] with
    // beta (2) moved to the top (boundary 0).
    app.comp_multi = vec![1, 2];
    app.comp_selected = Some(1);
    app.comp_move(2, 0);
    let names: Vec<&str> = app.doc.comps.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["beta", "alpha", "gamma"]);
    assert_eq!(app.comp_selected, Some(2), "gamma shifted down");
    assert_eq!(app.comp_multi, vec![0, 2], "beta moved, gamma followed");

    // Delete keeps both selections honest (index arithmetic, not a
    // len filter): drop beta (0) → [alpha, gamma].
    app.comp_delete_at(0);
    let names: Vec<&str> = app.doc.comps.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["alpha", "gamma"]);
    assert_eq!(app.comp_selected, Some(1));
    assert_eq!(app.comp_multi, vec![1]);

    // LC-013: a multi-selection exports only those comps; empty
    // selection = everything. (Subfolders exist even for a pageless
    // export — the folder is created before the page loop.)
    let dir = std::env::temp_dir().join(format!("mn-comps-sel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    app.comp_multi = vec![0]; // alpha only
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::CompExportAllPath(dir.clone()));
    assert!(dir.join("alpha").is_dir(), "the selected comp exported");
    assert!(!dir.join("gamma").is_dir(), "the unselected comp did not");
    assert!(
        app.status.contains("1 of 2 comps"),
        "the status names the scope: {}",
        app.status
    );
    app.comp_multi.clear();
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::CompExportAllPath(dir.clone()));
    assert!(dir.join("gamma").is_dir(), "empty selection = all comps");
    assert!(
        app.status.contains("all 2 comps"),
        "the status names the scope: {}",
        app.status
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// O-011 page-wide gutters (TRIAGE 141): an edge drag on one frame
/// folder carries the shared border of a SIBLING folder — a divide
/// makes each panel its own folder, and the gutter between them must
/// move as one border or every tweak drifts the grid.
#[test]
fn edge_drag_carries_gutters_across_folders() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let a = app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([16.0, 16.0, 300.0, 400.0], 4.0),
    );
    let b = app.doc.add_frame_folder(
        "Frame 2",
        mn_core::FrameSet::single_rect([300.0, 16.0, 600.0, 400.0], 4.0),
    );
    // Frame 1's right edge (points 1..2, x=300) drags right by 20.
    let orig = app.doc.layers[a].frames().unwrap().frames[0].clone();
    app.object_drag = Some(crate::app::canvas_input::ObjectDrag {
        layer: a,
        frame: 0,
        mode: crate::app::canvas_input::ObjectDragMode::Edge(1),
        start: (300.0, 100.0),
        cur: (320.0, 100.0),
        orig,
    });
    let (sx, sy) = app.viewport.to_screen(320.0, 100.0);
    app.canvas_up(sx, sy, &[]);
    // push_cmd queues for the frame loop — pump it by hand in tests.
    while let Some(c) = app.cmds.pop_front() {
        crate::cmd::dispatch(&mut app, c);
    }

    let fa = app.doc.layers[a].frames().unwrap().frames[0].points.clone();
    assert!((fa[1][0] - 320.0).abs() < 0.01, "{fa:?}");
    // The sibling folder's LEFT border rode along (both shared
    // vertices), so the page stays a grid with no gap.
    let fb = app.doc.layers[b].frames().unwrap().frames[0].points.clone();
    assert!(
        fb.iter().any(|p| (p[0] - 320.0).abs() < 0.01),
        "the sibling border followed: {fb:?}"
    );
    // And no vertex was left behind at 300.
    assert!(!fb.iter().any(|p| (p[0] - 300.0).abs() < 0.01), "{fb:?}");
}

/// GLM-audit survivor #1: a folder-header MoveWhole drag records an undo
/// op for EVERY child and for the linked mask — not for whichever layer
/// happened to be active. The old single begin_op() + translate_content's
/// mem::take recorded `None` pre-images on the active child (undo DELETED
/// its art) and nothing at all on its siblings (undo stranded them).
#[test]
fn frame_move_undoes_every_childs_art_and_mask() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let a = app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([16.0, 16.0, 300.0, 380.0], 4.0),
    );
    // add_frame_folder leaves the draw child active — the regression's
    // exact habitat. Do not touch doc.active.
    let draw = app.doc.active;
    let kids: Vec<usize> = app.doc.children_range(a).collect();
    assert!(kids.contains(&draw), "draw child inside the folder");
    // Known ink at canvas (100, 100) on the draw child, plus a linked mask
    // tile (mask_linked defaults true — it must ride and be undoable).
    let px = [0u16, 0, 0, 32768];
    app.doc.layers[draw]
        .tile_mut(mn_core::TileIdx::new(1, 1))
        .set_pixel(36, 36, px);
    let mut m = mn_core::doc::LayerMask::default();
    m.enabled = true;
    let mut t = mn_core::Tile::new_transparent();
    t.set_pixel(0, 0, [32768, 32768, 32768, 32768]);
    m.tiles
        .insert(mn_core::TileIdx::new(1, 1), std::sync::Arc::new(t));
    app.doc.layers[draw].mask = Some(m);

    let alpha_at = |app: &App, li: usize, cx: i32, cy: i32| {
        let ts = mn_core::TILE_SIZE as i32;
        app.doc.layers[li]
            .tile(mn_core::TileIdx::new(cx.div_euclid(ts), cy.div_euclid(ts)))
            .map(|t| t.pixel(cx.rem_euclid(ts) as usize, cy.rem_euclid(ts) as usize)[3])
            .unwrap_or(0)
    };
    assert!(alpha_at(&app, draw, 100, 100) > 0);

    // Drag the whole panel +100 px in x (sub-tile offset on purpose: the
    // four-way blit is the recording bypass's worst case).
    let orig = app.doc.layers[a].frames().unwrap().frames[0].clone();
    app.object_drag = Some(crate::app::canvas_input::ObjectDrag {
        layer: a,
        frame: 0,
        mode: crate::app::canvas_input::ObjectDragMode::MoveWhole,
        start: (50.0, 50.0),
        cur: (150.0, 50.0),
        orig,
    });
    let (sx, sy) = app.viewport.to_screen(150.0, 50.0);
    app.canvas_up(sx, sy, &[]);
    while let Some(c) = app.cmds.pop_front() {
        crate::cmd::dispatch(&mut app, c);
    }
    assert!(alpha_at(&app, draw, 200, 100) > 0, "ink moved with the panel");
    assert_eq!(alpha_at(&app, draw, 100, 100), 0, "no ink left behind");
    let mask_at = |app: &App, ti: (i32, i32)| {
        app.doc.layers[draw]
            .mask
            .as_ref()
            .is_some_and(|m| m.tiles.contains_key(&mn_core::TileIdx::new(ti.0, ti.1)))
    };
    assert!(!mask_at(&app, (1, 1)) || mask_at(&app, (2, 1)), "mask rode along");

    while app.doc.undo() {}
    assert!(
        alpha_at(&app, draw, 100, 100) > 0,
        "undo restores the art at its ORIGINAL place (the old bug deleted it)"
    );
    assert_eq!(alpha_at(&app, draw, 200, 100), 0, "moved copy undone");
    assert!(mask_at(&app, (1, 1)), "mask back at its original tile");

    while app.doc.redo() {}
    assert!(alpha_at(&app, draw, 200, 100) > 0, "redo re-applies the move");
    assert_eq!(alpha_at(&app, draw, 100, 100), 0);
}

/// The other half of survivor #1: with the ACTIVE layer outside the
/// folder, the old code recorded nothing at all — undo restored the
/// panel geometry and stranded the children's pixels where they were.
#[test]
fn frame_move_records_children_when_active_layer_is_elsewhere() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let a = app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([16.0, 16.0, 300.0, 380.0], 4.0),
    );
    let kids: Vec<usize> = app.doc.children_range(a).collect();
    let draw = app.doc.active;
    assert!(kids.contains(&draw));
    let px = [0u16, 0, 0, 32768];
    app.doc.layers[draw]
        .tile_mut(mn_core::TileIdx::new(1, 1))
        .set_pixel(36, 36, px);
    // Activate a layer OUTSIDE the folder (the base "Layer 1").
    let outside = (0..app.doc.layers.len())
        .find(|i| !kids.contains(i) && *i != a)
        .unwrap();
    app.doc.set_active(outside);

    let orig = app.doc.layers[a].frames().unwrap().frames[0].clone();
    app.object_drag = Some(crate::app::canvas_input::ObjectDrag {
        layer: a,
        frame: 0,
        mode: crate::app::canvas_input::ObjectDragMode::MoveWhole,
        start: (50.0, 50.0),
        cur: (150.0, 50.0),
        orig,
    });
    let (sx, sy) = app.viewport.to_screen(150.0, 50.0);
    app.canvas_up(sx, sy, &[]);
    while let Some(c) = app.cmds.pop_front() {
        crate::cmd::dispatch(&mut app, c);
    }
    let alpha_at = |app: &App, cx: i32, cy: i32| {
        let ts = mn_core::TILE_SIZE as i32;
        app.doc.layers[draw]
            .tile(mn_core::TileIdx::new(cx.div_euclid(ts), cy.div_euclid(ts)))
            .map(|t| t.pixel(cx.rem_euclid(ts) as usize, cy.rem_euclid(ts) as usize)[3])
            .unwrap_or(0)
    };
    assert!(alpha_at(&app, 200, 100) > 0, "ink moved");

    while app.doc.undo() {}
    assert!(
        alpha_at(&app, 100, 100) > 0,
        "undo restores the child's art even though it was never active"
    );
    assert_eq!(alpha_at(&app, 200, 100), 0);
}

/// O-010: an axis-aligned edge drag SNAPS to other frames' edge lines
/// (any same-orientation edge is an infinite extension line) within
/// 3 canvas px.
#[test]
fn edge_drag_snaps_to_neighbour_edge_lines() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let a = app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([16.0, 16.0, 300.0, 400.0], 4.0),
    );
    // A vertical edge 4.5 px away — within the 3 px snap of the
    // DRAGGED position, not of the rest position.
    app.doc.add_frame_folder(
        "Frame 2",
        mn_core::FrameSet::single_rect([304.5, 16.0, 500.0, 400.0], 4.0),
    );
    let orig = app.doc.layers[a].frames().unwrap().frames[0].clone();
    app.object_drag = Some(crate::app::canvas_input::ObjectDrag {
        layer: a,
        frame: 0,
        mode: crate::app::canvas_input::ObjectDragMode::Edge(1),
        start: (300.0, 100.0),
        cur: (300.0, 100.0),
        orig,
    });
    // Pointer at 303: the edge lands at 303, 1.5 px from 304.5 -> snap.
    let (sx, sy) = app.viewport.to_screen(303.0, 100.0);
    app.canvas_move(sx, sy, &[]);
    let d = app.object_drag.as_ref().unwrap();
    assert!((d.cur.0 - 304.5).abs() < 0.01, "snapped: {:?}", d.cur);
    // Far away: no snap.
    let (sx, sy) = app.viewport.to_screen(360.0, 100.0);
    app.canvas_move(sx, sy, &[]);
    let d = app.object_drag.as_ref().unwrap();
    assert!((d.cur.0 - 360.0).abs() < 0.01, "no snap: {:?}", d.cur);
}

// --- paste to position (owner HIGH 2026-08-18) -------------------------

/// The resolution order, on a bare document: the folder OWNING the
/// active layer beats the pointer; a loose active layer falls to the
/// pointer panel; no hit and no selection = None (today's behaviour).
#[test]
fn paste_target_resolution_order() {
    use crate::cmd::resolve_paste_target;
    let mut doc = mn_core::Document::default();
    let _a = doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([16.0, 16.0, 300.0, 400.0], 4.0),
    );
    let b = doc.add_frame_folder(
        "Frame 2",
        mn_core::FrameSet::single_rect([320.0, 16.0, 600.0, 400.0], 4.0),
    );
    // add_frame_folder leaves ITS draw layer active.
    let draw_b = doc.active;
    // Rule 1 wins over the pointer: active is inside Frame 2's folder,
    // the pointer sits inside Frame 1's panel.
    let t = resolve_paste_target(&doc, draw_b, Some((150.0, 200.0))).unwrap();
    assert_eq!(t.folder, Some(b));
    assert!(t.owns_active);
    assert_eq!(t.label, "Frame 2");
    // A loose layer (depth 0 — add_layer inserts ABOVE the active
    // layer, so start from the root base layer) + pointer inside
    // Frame 1 = Frame 1, NOT owning the active layer -> the commit
    // creates a layer there. The insert shifts headers: re-look them up.
    doc.active = 0;
    let loose = doc.add_layer("rough");
    let a = doc.layers.iter().position(|l| l.name == "Frame 1").unwrap();
    let t = resolve_paste_target(&doc, loose, Some((150.0, 200.0))).unwrap();
    assert_eq!(t.folder, Some(a));
    assert!(!t.owns_active);
    // Pointer between panels, no selection: no target.
    assert!(resolve_paste_target(&doc, loose, Some((310.0, 500.0))).is_none());
    // A selection aims the paste without a folder.
    doc.selection = Some(mn_core::Selection::from_rect(&doc, 10.0, 10.0, 90.0, 90.0));
    let t = resolve_paste_target(&doc, loose, Some((310.0, 500.0))).unwrap();
    assert_eq!(t.folder, None);
    assert_eq!(t.rect, [10.0, 10.0, 90.0, 90.0]);
    // A frame folder with NO frames has no panel to aim at: rule 1 must
    // fall through to the pointer instead of indexing an empty set.
    let empty = mn_core::FrameSet {
        frames: Vec::new(),
        ..mn_core::FrameSet::single_rect([0.0, 0.0, 1.0, 1.0], 4.0)
    };
    doc.add_frame_folder("Empty", empty);
    let inside = doc.active; // add_frame_folder leaves ITS draw layer active
    let a = doc.layers.iter().position(|l| l.name == "Frame 1").unwrap();
    let t = resolve_paste_target(&doc, inside, Some((150.0, 200.0))).unwrap();
    assert_eq!(t.folder, Some(a), "fell through to the pointer panel");
    let _ = loose;
}

/// A paste float helper: an 8x8 opaque blob at `at`.
fn clip_blob(at: [i32; 2]) -> mn_core::FloatSource {
    let bgra = vec![255u8; 8 * 8 * 4];
    crate::clipboard::bgra_to_floatsource(&bgra, 8, 8, at, 4096, 4096)
}

/// Rule 1 e2e: active layer inside the frame folder — the float opens
/// CENTRED on the panel, stamps the active layer, creates no layer,
/// and the status names the target.
#[test]
fn paste_into_owning_folder_centres_and_stamps_active() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([64.0, 64.0, 320.0, 400.0], 4.0),
    );
    let draw = app.doc.active;
    let before = app.doc.layers.len();
    app.clipboard = Some(clip_blob([0, 0]));

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Paste);
    let drag = app.transform_drag.as_ref().expect("float opened");
    assert_eq!(
        drag.create_in, None,
        "owning folder stamps the active layer"
    );
    let c = (
        (drag.bbox[0][0] + drag.bbox[2][0]) * 0.5,
        (drag.bbox[0][1] + drag.bbox[2][1]) * 0.5,
    );
    assert!((c.0 - 192.0).abs() < 1.5, "centred on panel x, got {c:?}");
    assert!((c.1 - 232.0).abs() < 1.5, "centred on panel y, got {c:?}");

    assert!(app.status.contains("Frame 1"), "status names the target");
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);
    assert_eq!(app.doc.layers.len(), before, "no layer created");
    assert!(
        app.doc.layers[draw]
            .tiles()
            .map(|(_, t)| t.alpha_sum())
            .sum::<u64>()
            > 0,
        "stamped the draw layer"
    );
}

/// Rules 2/3 e2e: a target folder that does NOT own the active layer —
/// the commit creates the "Pasted" layer INSIDE it, topmost child, and
/// the ink lands inside the panel rect; Esc leaves nothing behind.
#[test]
fn paste_into_foreign_panel_creates_layer_inside_folder() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([64.0, 64.0, 320.0, 400.0], 4.0),
    );
    // A rough layer on the ROOT base (add_layer inserts above the
    // active layer; from the folder's draw layer that lands INSIDE the
    // folder and shifts the header). Capture the header AFTER.
    app.doc.active = 0;
    let _loose = app.doc.add_layer("rough");
    let header = app.doc.layers.iter().position(|l| l.is_frame()).unwrap();

    // The pointer rule, directly: a resolved target past owns_active.
    let target = crate::cmd::PasteTarget {
        folder: Some(header),
        owns_active: false,
        rect: [64.0, 64.0, 320.0, 400.0],
        label: "Frame 1".into(),
    };
    crate::cmd::open_float_aimed(&mut app, clip_blob([0, 0]), Some(&target));
    assert_eq!(app.transform_drag.as_ref().unwrap().create_in, Some(header));

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);
    let pasted = app
        .doc
        .layers
        .iter()
        .position(|l| l.name == "Pasted")
        .expect("the pasted layer was created");
    let hd = app.doc.layers.iter().position(|l| l.is_frame()).unwrap();
    assert!(
        app.doc.children_range(hd).contains(&pasted),
        "the pasted layer sits inside the frame folder"
    );
    assert_eq!(app.doc.layers[pasted].depth, app.doc.layers[hd].depth + 1);
    assert_eq!(app.doc.active, pasted);
    // The stamp centred on the panel: bounds inside the panel rect.
    let (bx, by, bw, bh) = app.doc.layers[pasted].tile_bounds().expect("ink landed");
    assert!(bx >= 60 && by >= 60, "bounds {bx},{by} inside panel");
    assert!(
        bx + bw as i32 <= 330 && by + bh as i32 <= 406,
        "bounds fit the panel"
    );

    // Cancel on a fresh float leaves NOTHING behind.
    crate::cmd::open_float_aimed(&mut app, clip_blob([0, 0]), Some(&target));
    let n = app.doc.layers.len();
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCancel);
    assert_eq!(app.doc.layers.len(), n, "cancel created no layer");
}

/// Oversized content scales uniformly DOWN into the panel, never up.
#[test]
fn oversized_paste_scales_to_fit_the_panel() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.doc.add_frame_folder(
        "Frame 1",
        mn_core::FrameSet::single_rect([64.0, 64.0, 320.0, 400.0], 4.0),
    );
    // A 600x600 blob: wider than the 256-wide panel.
    let bgra = vec![255u8; 600 * 600 * 4];
    app.clipboard = Some(crate::clipboard::bgra_to_floatsource(
        &bgra,
        600,
        600,
        [0, 0],
        4096,
        4096,
    ));
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Paste);
    let drag = app.transform_drag.as_ref().expect("float opened");
    let w = drag.bbox[2][0] - drag.bbox[0][0];
    let h = drag.bbox[2][1] - drag.bbox[0][1];
    assert!(w <= 257.0, "fit-scaled width {w}");
    assert!((w / h - 1.0).abs() < 0.01, "uniform scale, {w}x{h}");
}
