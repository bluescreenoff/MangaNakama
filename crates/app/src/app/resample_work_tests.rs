//! `IO-060` (workflow audit §10) — Edit ▸ Change work resolution, at WORK
//! scope: the paper stays the paper, every page's pixels are re-made, and
//! the op either lands completely or not at all.
//!
//! Same frugality rule as `new_document_tests`: 72 dpi drafts, one App at a
//! time — a B4 600 dpi page is 35 million pixels and this op touches every
//! one of them on every page.

use super::new_document_tests::{headless, small_draft};
use crate::cmd::{AppCmd, dispatch};
use mn_core::{Document, transform::Interp};

/// Drive the run the way frames do: start it, then step until the job is
/// gone. This is the SHIPPED path — phase 1 is chunked one page per frame so
/// the app can paint a count between pages, and there is no blocking entry
/// point left for a test to take a short cut through.
///
/// A refusal made before the run starts comes back from `begin`; one made
/// while building a page lands on the status line as an error, because by
/// then there is no caller left to return it to.
fn resample(
    app: &mut crate::App,
    dpi: u32,
    interp: Interp,
) -> Result<usize, String> {
    app.resample_work_begin(dpi, interp, String::new())?;
    // Bounded: a step that neither advances nor finishes is a hang, and a
    // test that hangs tells nobody anything.
    for _ in 0..10_000 {
        if app.resample_job.is_none() {
            break;
        }
        app.resample_work_step();
    }
    assert!(app.resample_job.is_none(), "the run terminated");
    if app.status_warn {
        return Err(app.status.clone());
    }
    Ok(app.pages.len())
}

fn parked_size(app: &crate::App, i: usize) -> (u32, u32) {
    if let Some((w, h, _)) = app.pages[i].blank {
        return (w, h);
    }
    let b = app.pages[i]
        .bytes
        .as_ref()
        .expect("a parked page carries bytes");
    mn_core::project::bytes_to_doc(b)
        .expect("a parked page decodes")
        .size
}

/// The op's own contract, composite: the open page, every parked page and
/// the still-lazy blank all move together; the PageSetup keeps its paper in
/// millimetres and changes only its dpi; every touched page takes a fresh
/// content revision (which is what makes a parked document stale) and drops
/// its caches.
#[test]
fn the_work_resample_moves_every_page_and_leaves_the_paper_alone() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 3, "Resolution");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let before_px = app.doc.size;
    let setup = app.page.clone().expect("a comic has a page setup");
    let paper_mm = setup.paper_mm;
    let trim_mm = setup.trim_mm;
    let revs: Vec<u64> = app.pages.iter().map(|e| e.rev).collect();
    // Page 2 is still a lazy blank: it must re-mark, not decode.
    assert!(app.pages[1].blank.is_some(), "page 2 starts lazy");
    // Page 3 carries a PARKED live document at the old resolution — the
    // thing the rev-bump invariant exists to invalidate. Undo history and
    // 600 dpi tiles cannot be reinstated onto a 350 dpi page.
    app.pages[2].parked = Some(Box::new(Document::new(before_px.0, before_px.1)));
    app.pages[2].parked_rev = app.pages[2].rev;

    // 72 -> 144 dpi. The expected size is the SETUP's own paper at the new
    // resolution, not `before × 2`: `paper_px` rounds mm→px independently
    // at each dpi, and a page that is off by one from the setup describing
    // it is a page the next Add Page will not match.
    let mut probe = setup.clone();
    probe.dpi = setup.dpi * 2;
    let want = probe.paper_px();
    assert_ne!(
        want,
        (before_px.0 * 2, before_px.1 * 2),
        "this paper is exactly the rounding case worth pinning"
    );

    let n = resample(&mut app, setup.dpi * 2, Interp::HighAccuracy).expect("the resample lands");

    assert_eq!(n, 3, "every page of the work was rebuilt");
    assert_eq!(app.doc.size, want, "the open page is the new paper");
    for i in 1..3 {
        assert_eq!(parked_size(&app, i), want, "page {} moved too", i + 1);
        assert!(
            app.pages[i].rev > revs[i],
            "page {}: a direct byte write MUST bump the revision, or a parked \
             document at the old resolution would be reinstated as fresh",
            i + 1
        );
        assert!(app.pages[i].thumb.is_none(), "page {}: thumb dropped", i + 1);
        assert!(
            app.pages[i].preview_img.is_none(),
            "page {}: sharp preview dropped",
            i + 1
        );
    }
    assert!(
        app.pages[1].blank.is_some(),
        "the untouched page stayed LAZY — a blank's size is its whole \
         content, so re-marking it is the entire resample"
    );
    assert_ne!(
        app.pages[2].parked_rev, app.pages[2].rev,
        "the parked live document is now STALE and will be dropped on \
         arrival — a 72 dpi document reinstated onto a 144 dpi page would \
         silently undo the whole operation for that page"
    );
    assert!(
        app.pages[app.page_index].bytes.is_none(),
        "active-page invariant restored: bytes live in `doc`"
    );

    let after = app.page.clone().expect("still a comic");
    assert_eq!(after.dpi, setup.dpi * 2, "the resolution is what moved");
    assert_eq!(
        after.paper_mm, paper_mm,
        "SAME PAPER: a work resample is not a page-size change"
    );
    assert_eq!(after.trim_mm, trim_mm, "and the trim did not move either");
    assert_eq!(
        after.paper_px(),
        app.doc.size,
        "the setup's derived pixel size agrees with the pages it describes"
    );
    assert!(
        app.preflight_stale,
        "the publisher-profile dpi check has to be re-run against the new number"
    );
}

/// A combined spread is double-width, and it must still be double-width
/// afterwards. The op scales each page by the RATIO rather than to a target
/// size, which is what makes this true by construction — a target-size
/// version of the same feature would squash the spread onto one page.
#[test]
fn a_combined_spread_keeps_its_double_width() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 2, "Spread");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let (w, h) = app.doc.size;
    let setup = app.page.clone().expect("setup");
    let mut probe = setup.clone();
    probe.dpi = setup.dpi * 2;
    let want = probe.paper_px();

    let spread = mn_core::project::doc_to_bytes(&Document::new(w * 2, h)).unwrap();
    let mut e = app.fresh_page(Some(spread), None);
    e.spread = true;
    app.pages.push(e);

    resample(&mut app, setup.dpi * 2, Interp::HighAccuracy).expect("the resample lands");

    assert_eq!(app.doc.size, want, "the single page is the new paper");
    assert_eq!(
        parked_size(&app, 2),
        (want.0 * 2, want.1),
        "the spread stayed exactly twice the new paper — the ratio is applied \
         to the page's OWN pixels, so nothing had to know it was a spread"
    );
    assert!(app.pages[2].spread, "and kept its badge");
}

/// ATOMICITY. A page that cannot be decoded aborts the whole run before a
/// single entry is written — no half-resampled chapter, ever.
///
/// The injected failure sits on page 3 of 4, so a naive implementation (the
/// shape `batch_other_pages` and `resize_other_pages` both use, editing
/// entries as it goes) would already have rewritten pages 1 and 2 by the
/// time it noticed. That is precisely the state this asserts cannot exist.
#[test]
fn one_unreadable_page_leaves_the_whole_work_untouched() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 4, "Atomic");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let dpi = app.page.as_ref().expect("setup").dpi;
    let open_px = app.doc.size;

    // Force pages 2..4 to be real byte pages (not lazy blanks), so the run
    // genuinely has work in flight when it hits the bad one.
    for i in 1..4 {
        let good = mn_core::project::doc_to_bytes(&Document::new(open_px.0, open_px.1)).unwrap();
        app.pages[i].blank = None;
        app.pages[i].bytes = Some(good);
    }
    app.pages[2].bytes = Some(b"this is not an ORA zip".to_vec());

    let sizes: Vec<(u32, u32)> = (0..4)
        .map(|i| if i == 2 { (0, 0) } else { parked_size(&app, i) })
        .collect();
    let revs: Vec<u64> = app.pages.iter().map(|e| e.rev).collect();
    let bytes2 = app.pages[2].bytes.clone();

    let err = resample(&mut app, dpi * 2, Interp::HighAccuracy)
        .expect_err("an unreadable page must refuse the whole run");
    assert!(err.contains("page 3"), "the offender is named: {err}");

    assert_eq!(app.doc.size, open_px, "the open page never moved");
    assert_eq!(app.page.as_ref().map(|s| s.dpi), Some(dpi), "nor did the dpi");
    for i in 0..4 {
        assert_eq!(app.pages[i].rev, revs[i], "page {}: no revision bump", i + 1);
        if i != 2 {
            assert_eq!(parked_size(&app, i), sizes[i], "page {} untouched", i + 1);
        }
    }
    assert_eq!(app.pages[2].bytes, bytes2, "the bad page is as it was");
}

/// Four real byte pages, so a run genuinely has work in flight between
/// steps rather than four lazy blanks it re-marks in one multiplication.
fn four_real_pages(app: &mut crate::App) {
    let px = app.doc.size;
    for i in 1..4 {
        let good = mn_core::project::doc_to_bytes(&Document::new(px.0, px.1)).unwrap();
        app.pages[i].blank = None;
        app.pages[i].bytes = Some(good);
    }
}

/// The progress half: phase 1 goes one page per step, the count on the
/// status line moves with it, and NOTHING is installed until the step after
/// the last page is built. That last claim is the one that matters — it is
/// what makes cancelling safe and what makes the op still atomic now that it
/// is spread over frames.
#[test]
fn the_run_counts_pages_off_one_per_step_and_installs_only_at_the_end() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 4, "Progress");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let dpi = app.page.as_ref().expect("setup").dpi;
    four_real_pages(&mut app);
    let before = app.doc.size;

    app.resample_work_begin(dpi * 2, Interp::HighAccuracy, String::new())
        .expect("the run starts");
    assert!(
        app.status.contains("page 1 of 4"),
        "the count is up before the first page is built: {}",
        app.status
    );

    app.resample_work_step();
    assert_eq!(
        app.resample_job.as_ref().map(|j| j.done()),
        Some(1),
        "one page per step"
    );
    assert!(app.status.contains("of 4"), "{}", app.status);

    while app.resample_job.as_ref().is_some_and(|j| j.done() < 4) {
        app.resample_work_step();
    }
    assert!(
        app.resample_job.is_some(),
        "every page is built and the run is still going — phase 2 is a step of its own"
    );
    assert_eq!(
        app.doc.size, before,
        "and nothing has been installed yet: that is the whole reason \
         Cancel is honest"
    );
    assert_eq!(
        app.page.as_ref().map(|s| s.dpi),
        Some(dpi),
        "the setup has not moved either"
    );

    app.resample_work_step();
    assert!(app.resample_job.is_none(), "that step was phase 2");
    assert_ne!(app.doc.size, before, "…and it installed");
    assert_eq!(app.page.as_ref().map(|s| s.dpi), Some(dpi * 2));
    assert!(
        app.status.contains("work resampled"),
        "the finishing line: {}",
        app.status
    );
}

/// Cancel, which is only offered during phase 1 — and phase 1 writes
/// nothing, so it has to leave the work byte-identical. Same assertions as
/// the unreadable-page atomicity test, from the other direction.
#[test]
fn cancelling_part_way_leaves_the_whole_work_untouched() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 4, "Cancel");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let dpi = app.page.as_ref().expect("setup").dpi;
    let open_px = app.doc.size;
    four_real_pages(&mut app);
    let sizes: Vec<(u32, u32)> = (1..4).map(|i| parked_size(&app, i)).collect();
    let revs: Vec<u64> = app.pages.iter().map(|e| e.rev).collect();

    app.resample_work_begin(dpi * 2, Interp::HighAccuracy, String::new())
        .expect("the run starts");
    app.resample_work_step();
    app.resample_work_step();
    assert!(
        app.resample_job.as_ref().is_some_and(|j| j.done() == 2),
        "stopped part way, with pages built and held"
    );

    app.resample_work_cancel();

    assert!(app.resample_job.is_none(), "the run is over");
    assert_eq!(app.doc.size, open_px, "the open page never moved");
    assert_eq!(app.page.as_ref().map(|s| s.dpi), Some(dpi), "nor the dpi");
    for i in 1..4 {
        assert_eq!(app.pages[i].rev, revs[i], "page {}: no revision bump", i + 1);
        assert_eq!(
            parked_size(&app, i),
            sizes[i - 1],
            "page {} untouched",
            i + 1
        );
    }
    assert!(
        app.pages[app.page_index].bytes.is_none(),
        "active-page invariant restored: an abandoned run must not leave the \
         open page a second, stale copy of itself in the slot that promises \
         to be empty"
    );
    assert!(
        app.status.contains("nothing was written"),
        "and it says so: {}",
        app.status
    );
}

/// While the run is going the app takes no commands. The pending list is
/// keyed by page INDEX, so a page turn landing between two pages would
/// install work built against a document set that no longer exists.
#[test]
fn no_command_lands_while_the_run_is_going() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 4, "Locked");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let dpi = app.page.as_ref().expect("setup").dpi;
    four_real_pages(&mut app);
    let page = app.page_index;

    app.resample_work_begin(dpi * 2, Interp::HighAccuracy, String::new())
        .expect("the run starts");
    app.resample_work_step();
    dispatch(&mut app, AppCmd::PageNext);
    assert_eq!(app.page_index, page, "the page turn was refused");
    assert!(
        app.resample_job.as_ref().is_some_and(|j| j.done() == 1),
        "and it did not disturb the run"
    );

    app.resample_work_cancel();
    dispatch(&mut app, AppCmd::PageNext);
    assert_ne!(app.page_index, page, "and commands land again afterwards");
}

/// The door that stands in for undo. A resample cannot be undone, so the
/// command refuses unless there is a CURRENT file on disk to fall back to.
///
/// The never-saved case is the one worth pinning: a freshly created comic
/// reads as NOT dirty (the create path syncs the saved revision) while
/// having no file at all, so a `dirty()`-only guard would wave through
/// exactly the work with nothing behind it.
#[test]
fn the_command_refuses_a_work_with_no_current_file_behind_it() {
    let Some(mut app) = headless() else { return };
    small_draft(&mut app, 2, "Unsaved");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let dpi = app.page.as_ref().expect("setup").dpi;
    let px = app.doc.size;
    assert!(
        app.doc_path.is_none(),
        "a new comic has no file yet — and reads as not dirty, which is the trap"
    );

    app.resample_work_draft.dpi = dpi * 2;
    app.resample_work_open = true;
    dispatch(&mut app, AppCmd::ResampleWorkApply);

    assert_eq!(app.doc.size, px, "nothing happened");
    assert_eq!(app.page.as_ref().map(|s| s.dpi), Some(dpi));
    assert!(
        app.resample_work_open,
        "the dialog stays open so the refusal is next to the button that caused it"
    );
}

/// A pixel canvas has no resolution, so there is nothing to change — the
/// refusal says that rather than inventing 600 dpi and resampling to it.
#[test]
fn a_pixel_canvas_and_a_no_op_are_both_refused_by_name() {
    let Some(mut app) = headless() else { return };
    // Default document: no page setup at all.
    let err = resample(&mut app, 350, Interp::HighAccuracy).expect_err("a pixel canvas has no dpi");
    assert!(err.contains("pixel canvas"), "{err}");

    small_draft(&mut app, 1, "No-op");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let dpi = app.page.as_ref().expect("setup").dpi;
    let err = resample(&mut app, dpi, Interp::HighAccuracy)
        .expect_err("the same dpi is not a resample");
    assert!(err.contains("already"), "{err}");
}

/// The JP-guide warning is a value, so it can be asserted. A warning that
/// silently stopped being shown is the failure mode this exists for.
#[test]
fn the_mono_warning_appears_only_where_it_is_true() {
    use crate::app::mono_resample_warning;
    use mn_core::Expression;
    let w = mono_resample_warning(Expression::Mono, 600, 350).expect("mono work, real change");
    assert!(w.contains("1-bit"), "it names what degrades: {w}");
    assert!(
        w.contains("Tones are the exception"),
        "and the one thing that does NOT: {w}"
    );
    assert!(
        mono_resample_warning(Expression::Colour, 600, 350).is_none(),
        "a colour work has no 1-bit threshold to lose ink at"
    );
    assert!(
        mono_resample_warning(Expression::Mono, 600, 600).is_none(),
        "and a no-op degrades nothing"
    );
}

/// Runner-up 13 (`IO-030`), the export half: the CHOICE, in one function.
///
/// `Frequency` is the default and byte-for-byte the old behaviour — the
/// screen derives at the work's dpi and is reduced with the page, so 60 lpi
/// prints as 60 lpi and the reduction is where moiré comes from. `Dots`
/// derives at `work / scale`, so each cell lands back at its work-pixel
/// size after the reduction.
#[test]
fn the_export_tone_choice_only_moves_the_derive_dpi_on_a_reduction() {
    use mn_core::export::{ToneScale, tone_export_dpi};
    // The default arm never moves the number, whatever the scale.
    for s in [1.0, 0.5, 0.25] {
        assert_eq!(tone_export_dpi(600, s, ToneScale::Frequency), 600);
    }
    // Nor does the other arm when nothing is being reduced.
    assert_eq!(tone_export_dpi(600, 1.0, ToneScale::Dots), 600);
    assert_eq!(tone_export_dpi(600, 0.0, ToneScale::Dots), 600, "no divide by zero");
    // 600 -> 350 dpi is a 0.5833 scale: the screen derives at ~1029 dpi, so
    // a 60 lpi cell is 17.1 work px and lands at 10.0 px in the output —
    // the size it had in the work, on whole pixels, no beat.
    let d = tone_export_dpi(600, 350.0 / 600.0, ToneScale::Dots);
    assert_eq!(d, 1029);
    let cell_out = (d as f32 / 60.0) * (350.0 / 600.0);
    assert!(
        (cell_out - 10.0).abs() < 0.05,
        "the cell comes back to its work-pixel size, got {cell_out}"
    );
    assert_eq!(ToneScale::default(), ToneScale::Frequency);
}

/// And that the choice rides `ExportFinish` like every other finishing
/// decision, so a preset and a profile round-trip it instead of leaving a
/// stale value from the last run.
#[test]
fn the_export_tone_choice_round_trips_through_the_finish() {
    let Some(mut app) = headless() else { return };
    assert_eq!(
        app.export_finish().tone,
        mn_core::export::ToneScale::Frequency,
        "the default run is unchanged"
    );
    app.export_all_tone = mn_core::export::ToneScale::Dots;
    assert_eq!(app.export_finish().tone, mn_core::export::ToneScale::Dots);
    // Every built-in preset keeps today's behaviour — changing what a named
    // submission spec MEANS is not this row's business.
    for p in mn_core::export::PRINT_PRESETS {
        assert_eq!(
            p.finish.tone,
            mn_core::export::ToneScale::Frequency,
            "{}",
            p.name
        );
        app.set_export_finish(p.finish);
        assert_eq!(app.export_all_tone, mn_core::export::ToneScale::Frequency);
        assert!(
            mn_core::export::matching_preset(app.export_finish()).is_some(),
            "{} still matches itself after the round trip",
            p.name
        );
    }
}
