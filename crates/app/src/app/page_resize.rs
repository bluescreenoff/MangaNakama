//! Resizing a whole work: the DPI resample and the canvas resize that
//! both walk every page, parked or live. Cut out of `pages.rs`.

use super::App;
use mn_core::ResizeAnchor;

/// The 実寸 honesty the JP manuscript guides ask for, as a value rather
/// than a string built inside the dialog: モノクロ2階調 line art DEGRADES
/// when it is resized, because 1-bit ink cannot be interpolated — it can
/// only be re-decided, pixel by pixel. Tones are the exception and are
/// named as one, since a screentone re-derives at the new resolution and
/// keeps its frequency.
///
/// `None` on a colour work (there is no threshold to lose anything at) and
/// on a no-op. A function, not a `format!` in the window body, so the
/// warning can be asserted — a warning nobody tests is a warning that
/// silently stops being shown.
pub fn mono_resample_warning(
    expression: mn_core::Expression,
    from_dpi: u32,
    to_dpi: u32,
) -> Option<&'static str> {
    if expression != mn_core::Expression::Mono || to_dpi == from_dpi || to_dpi == 0 {
        return None;
    }
    Some(
        "This is a monochrome work. Resizing 1-bit line art degrades it — the ink is \
         re-made, not moved. Tones are the exception: they re-derive at the new \
         resolution and keep their frequency.",
    )
}

/// One page's new bytes, built but not yet installed — the unit the
/// atomic work resample swaps in (see [`App::resample_work`]).
enum PageResample {
    /// A parked/stashed page: re-encoded ORA bytes + its new canvas size.
    Bytes(Vec<u8>, (u32, u32)),
    /// A still-lazy blank: only its template size changes.
    Blank((u32, u32, usize)),
}

/// `IO-060`, the progress half: one whole-work resample in flight, one page
/// per frame.
///
/// [`App::resample_work`] does the same job in one blocking call, and on a
/// twenty-page B4 chapter that is a freeze long enough to look like a hang.
/// A status line written inside that loop would never be seen either:
/// `App::render` runs on `WM_PAINT` and nothing in this app paints from
/// inside a command. So phase 1 is chunked across frames — the
/// `App::reader_frame` idiom, one heavy unit per frame then yield — which
/// costs the same total work and paints a count between pages.
///
/// Chunking is safe here for exactly the reason the op is atomic: phase 1
/// writes NOTHING into the work, it only fills `pending`. That is also what
/// makes Cancel honest, and why Cancel exists only during phase 1 — phase 2
/// installs everything inside a single frame, so there is no half-installed
/// state to cancel into and nothing to refuse.
pub struct ResampleJob {
    dpi: u32,
    /// W01: the page setup the work lands on — the target paper (or the
    /// current one) at the new dpi. Installed whole in phase 2, so the
    /// guides and the pixels can never disagree about which paper this is.
    target: mn_core::PageSetup,
    interp: mn_core::transform::Interp,
    /// New paper over old, per axis — see [`App::resample_work`] for why it
    /// comes from the paper's pixels and not from the dpi.
    ratio: (f64, f64),
    pending: Vec<(usize, PageResample)>,
    /// The next page phase 1 will look at; also the count already done.
    next: usize,
    /// The finishing line's tail ("…the file on disk is still at the old
    /// resolution"), composed by the command while it still had the saved
    /// path in hand.
    back: String,
}

impl ResampleJob {
    /// Pages phase 1 has got through, and the resolution being built — what
    /// the progress window puts on screen.
    pub fn done(&self) -> usize {
        self.next
    }

    pub fn dpi(&self) -> u32 {
        self.dpi
    }
}

impl App {
    /// The op's refusals and its scale factor, before anything is stashed
    /// or built. New paper over old, per axis.
    ///
    /// The ratio comes from the PAPER in pixels, not from the dpi.
    ///
    /// Both are "new over old" to within a rounding, but `paper_px` rounds
    /// mm→px independently at each resolution, so a B4 page that is 729 px
    /// at 72 dpi is 1457 px at 144 dpi — not 1458. Scaling by the dpi would
    /// leave every page one pixel wider than the setup that describes it,
    /// and the next Add Page would then produce a page that does not match
    /// the ones beside it. Deriving the ratio from the paper makes a
    /// paper-sized page land EXACTLY on the new paper, and a double-width
    /// spread on exactly twice it.
    /// W01: the setup a resample lands on — `paper`'s geometry, or the
    /// work's own, at `new_dpi`. The target preset's own dpi is deliberately
    /// dropped: the dialog's resolution field decides, so picking "A4 Color
    /// 350dpi" as a SHAPE does not also drag a resolution the artist did not
    /// ask for. A paper with no guides (a pixel preset, where `paper_mm`
    /// holds pixels) is refused rather than read as millimetres.
    pub(crate) fn resample_target(
        setup: &mn_core::PageSetup,
        new_dpi: u32,
        paper: Option<&mn_core::PageSetup>,
    ) -> Result<mn_core::PageSetup, String> {
        let mut t = match paper {
            Some(p) if !p.has_guides() => {
                return Err(format!("{} is a pixel size, not a paper", p.name));
            }
            Some(p) => p.clone(),
            None => setup.clone(),
        };
        t.dpi = new_dpi;
        Ok(t)
    }

    fn resample_plan(
        &self,
        new_dpi: u32,
        paper: Option<&mn_core::PageSetup>,
    ) -> Result<((f64, f64), mn_core::PageSetup), String> {
        let Some(setup) = self.page.as_ref().filter(|s| s.has_guides()) else {
            return Err("this work is a pixel canvas — it has no resolution to change".into());
        };
        if new_dpi == 0 {
            return Err("pick a resolution above zero".into());
        }
        let target = Self::resample_target(setup, new_dpi, paper)?;
        if target == *setup {
            return Err(format!(
                "the work is already {} at {new_dpi} dpi",
                setup.name
            ));
        }
        let old_px = setup.paper_px();
        let new_px = target.paper_px();
        Ok((
            (
                new_px.0 as f64 / old_px.0.max(1) as f64,
                new_px.1 as f64 / old_px.1.max(1) as f64,
            ),
            target,
        ))
    }

    fn scaled(ratio: (f64, f64), wh: (u32, u32)) -> (u32, u32) {
        (
            ((wh.0 as f64 * ratio.0).round() as u32).max(1),
            ((wh.1 as f64 * ratio.1).round() as u32).max(1),
        )
    }

    /// Phase 1 for ONE page: build its new bytes, install nothing.
    /// `Ok(None)` = there was nothing to build.
    fn resample_page(
        &mut self,
        i: usize,
        ratio: (f64, f64),
        new_dpi: u32,
        interp: mn_core::transform::Interp,
    ) -> Result<Option<PageResample>, String> {
        if i == self.page_index {
            // The live document takes the op itself, in phase 2. Resampling
            // the stash as well would do a B4 page's worth of work twice
            // and then throw one copy away.
            return Ok(None);
        }
        if self.pages[i].bytes.is_none()
            && let Some((w, h, n)) = self.pages[i].blank
        {
            // A lazy blank never decodes for this: its size IS its whole
            // content, so the resample is one multiplication.
            let (w, h) = Self::scaled(ratio, (w, h));
            return Ok(Some(PageResample::Blank((w, h, n))));
        }
        let Some(bytes) = self.pages[i].bytes.as_deref() else {
            return Err(format!("page {} has no content to resample", i + 1));
        };
        let mut doc = mn_core::project::bytes_to_doc(bytes)
            .map_err(|e| format!("page {} could not be read: {e}", i + 1))?;
        // The target comes from THIS page's own pixels, so a combined
        // spread (double width) stays a spread and an odd page keeps
        // whatever size it really has.
        let target = Self::scaled(ratio, doc.size);
        if !doc.resample_to(target.0, target.1, interp) {
            return Err(format!("page {} could not be resampled", i + 1));
        }
        crate::app::refresh_derived_gpu(&mut doc, &mut self.renderer, new_dpi);
        let nb = mn_core::project::doc_to_bytes(&doc)
            .map_err(|e| format!("page {} could not be re-encoded: {e}", i + 1))?;
        Ok(Some(PageResample::Bytes(nb, target)))
    }

    /// Phase 2: install everything phase 1 built, then take the open page
    /// through core. Cannot fail, and runs whole inside one caller.
    fn resample_install(
        &mut self,
        pending: Vec<(usize, PageResample)>,
        new_setup: mn_core::PageSetup,
        interp: mn_core::transform::Interp,
        ratio: (f64, f64),
    ) -> usize {
        for (i, p) in pending {
            let rev = self.page_rev_next();
            let e = &mut self.pages[i];
            match p {
                PageResample::Bytes(nb, canvas) => {
                    e.bytes = Some(nb);
                    e.canvas = Some(canvas);
                }
                PageResample::Blank(b) => e.blank = Some(b),
            }
            // The rev bump is what makes a parked document stale: its
            // pixels are still at the old resolution.
            e.rev = rev;
            e.doc_rev = 0;
            e.thumb = None;
            e.preview_img = None;
            e.prev_tex = None;
            e.pane_tex = None;
        }
        let touched = self.pages.len();

        // The OPEN page's live document takes the same op through core, so
        // it keeps being the truth its `bytes: None` slot promises.
        let target = Self::scaled(ratio, self.doc.size);
        self.doc.resample_to(target.0, target.1, interp);
        // Text sprites were shaped at the OLD dpi and dropped by the
        // resample; the resampled pixels are standing in. Re-shape at the
        // new dpi and lay the crisp sprites down (no undo step — see
        // `Document::reraster_text`).
        for li in 0..self.doc.layers.len() {
            if self.doc.layers[li].texts().is_some() {
                self.warm_texts(li);
                self.doc.reraster_text(li);
            }
        }
        // The open page's slot moves with it: `bytes: None` is the
        // active-page invariant, and its content genuinely changed, so it
        // takes a fresh revision like every other page (the folder save's
        // skip hint and the sharp preview both key on it).
        let rev = self.page_rev_next();
        let e = &mut self.pages[self.page_index];
        e.bytes = None;
        e.canvas = Some(target);
        e.rev = rev;
        e.doc_rev = 0;
        e.thumb = None;
        e.preview_img = None;
        e.prev_tex = None;
        e.pane_tex = None;

        // W01: the finished setup goes in whole — the same paper at a new
        // resolution when no target was picked, the new paper otherwise.
        // Guides and pixels move in the same breath or they disagree.
        self.page = Some(new_setup);
        self.preflight_stale = true;
        self.mark_pages_dirty();
        self.mark_dirty();
        touched
    }

    // --- the chunked run, so twenty pages is not one silent freeze --------

    /// `IO-060` — resample the WHOLE work to `new_dpi`. Refuses, or starts
    /// a [`ResampleJob`] the frame loop then steps to completion.
    ///
    /// Every refusal is made HERE, before anything is stashed or built: a
    /// run that cannot start must not leave the open page's slot
    /// rearranged. `back` is the finishing line's tail, which the caller
    /// composes while it still has the saved path in hand.
    ///
    /// # Atomic by construction
    ///
    /// Every other page is decoded, resampled and re-encoded into a pending
    /// list FIRST; nothing is installed until every page has produced its
    /// bytes. A page that will not decode aborts the run before a single
    /// entry is written. The alternative — the shape `batch_other_pages`
    /// and `resize_other_pages` use, editing entries as it goes — would
    /// leave a chapter half at 600 dpi and half at 350 the first time an
    /// unreadable page turned up, and a half-resampled work is not
    /// something an artist can diagnose or repair.
    ///
    /// That same split is what lets phase 1 be spread over frames with a
    /// count and a Cancel on screen instead of freezing the app for a
    /// chapter's worth of pixels.
    ///
    /// # Not undoable, and what stands in for undo
    ///
    /// A work-level resample is not an undo step (CSP treats it the same
    /// way, and `Document::resize_to` already clears the history for the
    /// much smaller canvas resize). The caller refuses to run on a work
    /// with unsaved changes, so the file on disk is always the way back —
    /// that is cheaper and more honest than a per-page history nobody can
    /// hold in their head. The open page is resampled LAST, after every
    /// other page has succeeded, so the visible document is never the one
    /// left inconsistent.
    pub fn resample_work_begin(
        &mut self,
        new_dpi: u32,
        interp: mn_core::transform::Interp,
        paper: Option<mn_core::PageSetup>,
        back: String,
    ) -> Result<(), String> {
        if self.resample_job.is_some() {
            return Err("a work resample is already running".into());
        }
        let (ratio, target) = self.resample_plan(new_dpi, paper.as_ref())?;
        self.stash_current_page()
            .map_err(|e| format!("the open page could not be stashed: {e}"))?;
        let total = self.pages.len();
        self.resample_job = Some(ResampleJob {
            dpi: new_dpi,
            target,
            interp,
            ratio,
            pending: Vec::new(),
            next: 0,
            back,
        });
        self.set_status(format!(
            "changing work resolution to {new_dpi} dpi — page 1 of {total}…"
        ));
        self.needs_redraw = true;
        Ok(())
    }

    /// One page of phase 1 per call, from the frame head — the whole point:
    /// the app paints between pages, so twenty pages reads as twenty counted
    /// steps instead of one hang. The last call installs.
    pub fn resample_work_step(&mut self) {
        let Some(j) = self.resample_job.as_ref() else {
            return;
        };
        let (i, ratio, dpi, interp) = (j.next, j.ratio, j.dpi, j.interp);
        let total = self.pages.len();
        // A running job sends no input events, so there is a next frame
        // only if we ask for one — the same rule as the liquify and
        // smart-shape holds in `App::render`.
        self.shell
            .ctx
            .request_repaint_after(std::time::Duration::ZERO);
        if i < total {
            match self.resample_page(i, ratio, dpi, interp) {
                Ok(built) => {
                    let Some(j) = self.resample_job.as_mut() else {
                        return;
                    };
                    if let Some(p) = built {
                        j.pending.push((i, p));
                    }
                    j.next = i + 1;
                    let at = (i + 2).min(total);
                    self.set_status(format!(
                        "changing work resolution to {dpi} dpi — page {at} of {total}…"
                    ));
                }
                Err(e) => {
                    // Same atomicity as the blocking run: nothing was
                    // installed, so abandoning IS the rollback.
                    self.resample_abandon();
                    self.set_error(format!("resolution unchanged: {e}"));
                }
            }
            self.needs_redraw = true;
            return;
        }
        // Phase 1 is complete. Phase 2 runs WHOLE, here, inside one frame:
        // it cannot fail and it must never be observed half-done.
        let Some(job) = self.resample_job.take() else {
            return;
        };
        let ResampleJob {
            pending,
            back,
            target,
            ..
        } = job;
        let paper_moved = self
            .page
            .as_ref()
            .is_some_and(|s| s.paper_mm != target.paper_mm);
        let paper_note = paper_moved
            .then(|| format!(" on {}", target.name))
            .unwrap_or_default();
        let n = self.resample_install(pending, target, interp, ratio);
        // Structural: the texture changes size and every cached thumb is
        // stale (the canvas-resize rule).
        self.renderer.invalidate();
        self.layer_thumbs.clear();
        self.set_status(format!(
            "work resampled to {dpi} dpi{paper_note} ({}) — {n} page(s), {}×{} — \
             history cleared{back}",
            interp.label(),
            self.doc.size.0,
            self.doc.size.1,
        ));
        self.mark_dirty();
        self.needs_redraw = true;
    }

    /// Cancel, which is only ever offered during phase 1. Phase 2 installs
    /// inside a single frame, so a cancel aimed at it could not arrive
    /// between two pages — there is no mid-install state to refuse.
    pub fn resample_work_cancel(&mut self) {
        let Some(done) = self.resample_job.as_ref().map(|j| j.next) else {
            return;
        };
        self.resample_abandon();
        self.set_status(format!(
            "work resolution unchanged — cancelled after {done} page(s); nothing was written"
        ));
    }

    /// Drop the run and put the open page's slot back the way phase 2 would
    /// have left it. `bytes: None` is the active-page invariant, and the
    /// stash phase 1 needed is the only mark an abandoned run leaves.
    fn resample_abandon(&mut self) {
        self.resample_job = None;
        self.pages[self.page_index].bytes = None;
        self.needs_redraw = true;
    }

    /// Resize every OTHER page of the work to `w × h`, pinning content to
    /// `anchor`. The OPEN page is not touched here — its caller takes it
    /// through the normal canvas door so undo, caches and the renderer all
    /// see it.
    ///
    /// Same bytes round trip as `App::batch_other_pages` /
    /// `AppCmd::CompApplyAllPages`: stash the open page first, decode each
    /// parked page's bytes, edit the decoded document, re-encode, hand it a
    /// fresh content revision and drop its thumbnail (the Pages panel and
    /// the rev-keyed sharp preview both cache on that). The decoded
    /// documents never become `self.doc`, so the `adopt_page_doc` ruler trap
    /// cannot bite. Afterwards the active-page invariant is restored: bytes
    /// live in `doc`.
    ///
    /// A COMBINED spread is a double-width page, so it takes `2w × h` — the
    /// same 1.5× width test the export split uses decides, because the
    /// `spread` flag is runtime-only and a reloaded work has none.
    ///
    /// These writes are DIRECT: undo covers the open page only. Returns
    /// (resized, unreadable).
    pub fn resize_other_pages(
        &mut self,
        w: u32,
        h: u32,
        anchor: ResizeAnchor,
        normal_w: Option<u32>,
    ) -> (usize, usize) {
        if let Err(e) = self.stash_current_page() {
            self.set_error(format!("other pages skipped: {e}"));
            return (0, self.pages.len().saturating_sub(1));
        }
        let (mut done, mut failed) = (0usize, 0usize);
        for i in 0..self.pages.len() {
            if i == self.page_index {
                continue; // the live document already took it
            }
            // A still-LAZY blank never encodes for this: resizing a blank
            // page is re-marking its template size, which keeps the whole
            // point of the marker (no ORA walk for untouched pages).
            if self.pages[i].bytes.is_none() && self.pages[i].blank.is_some() {
                let spread = self.pages[i].spread;
                let target = if spread {
                    (w.saturating_mul(2).max(1), h)
                } else {
                    (w, h)
                };
                let n = self.pages[i].blank.unwrap().2;
                self.pages[i].blank = Some((target.0, target.1, n));
                done += 1;
                continue;
            }
            let Some(bytes) = self.pages[i].bytes.as_deref() else {
                failed += 1;
                continue;
            };
            let Ok(mut doc) = mn_core::project::bytes_to_doc(bytes) else {
                failed += 1;
                continue;
            };
            let target = if crate::cmd::is_spread_page(&doc, self.pages[i].spread, normal_w) {
                (w.saturating_mul(2).max(1), h)
            } else {
                (w, h)
            };
            if doc.size == target {
                continue;
            }
            doc.resize_canvas(target.0, target.1, anchor);
            let Ok(nb) = mn_core::project::doc_to_bytes(&doc) else {
                failed += 1;
                continue;
            };
            let rev = self.page_rev_next();
            let e = &mut self.pages[i];
            e.bytes = Some(nb);
            e.rev = rev;
            e.doc_rev = 0;
            e.thumb = None;
            // The reader's 1:1 view reads this cache, not the bytes.
            e.canvas = Some(target);
            done += 1;
        }
        // Restore the active-page invariant (bytes live in `doc`).
        self.pages[self.page_index].bytes = None;
        self.mark_pages_dirty();
        self.mark_dirty();
        (done, failed)
    }
}
