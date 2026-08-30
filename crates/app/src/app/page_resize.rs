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

impl App {
    /// `IO-060` — resample the WHOLE work to `new_dpi`.
    ///
    /// Returns `Ok(pages touched)`, or `Err` with the page that refused and
    /// the work COMPLETELY unchanged.
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
    pub fn resample_work(
        &mut self,
        new_dpi: u32,
        interp: mn_core::transform::Interp,
    ) -> Result<usize, String> {
        let Some(setup) = self.page.clone().filter(|s| s.has_guides()) else {
            return Err("this work is a pixel canvas — it has no resolution to change".into());
        };
        if new_dpi == 0 {
            return Err("pick a resolution above zero".into());
        }
        if new_dpi == setup.dpi {
            return Err(format!("the work is already {new_dpi} dpi"));
        }
        // The ratio comes from the PAPER in pixels, not from the dpi.
        //
        // Both are "new over old" to within a rounding, but `paper_px`
        // rounds mm→px independently at each resolution, so a B4 page that
        // is 729 px at 72 dpi is 1457 px at 144 dpi — not 1458. Scaling by
        // the dpi would leave every page one pixel wider than the setup
        // that describes it, and the next Add Page would then produce a
        // page that does not match the ones beside it. Deriving the ratio
        // from the paper makes a paper-sized page land EXACTLY on the new
        // paper, and a double-width spread on exactly twice it.
        let old_px = setup.paper_px();
        let mut probe = setup.clone();
        probe.dpi = new_dpi;
        let new_px = probe.paper_px();
        let ratio_x = new_px.0 as f64 / old_px.0.max(1) as f64;
        let ratio_y = new_px.1 as f64 / old_px.1.max(1) as f64;
        self.stash_current_page()
            .map_err(|e| format!("the open page could not be stashed: {e}"))?;

        // --- phase 1: build every page's new bytes, installing nothing ---
        let mut pending: Vec<(usize, PageResample)> = Vec::new();
        let scaled = |wh: (u32, u32)| -> (u32, u32) {
            (
                ((wh.0 as f64 * ratio_x).round() as u32).max(1),
                ((wh.1 as f64 * ratio_y).round() as u32).max(1),
            )
        };
        for i in 0..self.pages.len() {
            if i == self.page_index {
                // The live document takes the op itself, below. Resampling
                // the stash as well would do a B4 page's worth of work
                // twice and then throw one copy away.
                continue;
            }
            if self.pages[i].bytes.is_none()
                && let Some((w, h, n)) = self.pages[i].blank
            {
                // A lazy blank never decodes for this: its size IS its
                // whole content, so the resample is one multiplication.
                let (w, h) = scaled((w, h));
                pending.push((i, PageResample::Blank((w, h, n))));
                continue;
            }
            let Some(bytes) = self.pages[i].bytes.as_deref() else {
                return Err(format!("page {} has no content to resample", i + 1));
            };
            let mut doc = mn_core::project::bytes_to_doc(bytes)
                .map_err(|e| format!("page {} could not be read: {e}", i + 1))?;
            // The target comes from THIS page's own pixels, so a combined
            // spread (double width) stays a spread and an odd page keeps
            // whatever size it really has.
            let target = scaled(doc.size);
            if !doc.resample_to(target.0, target.1, interp) {
                return Err(format!("page {} could not be resampled", i + 1));
            }
            crate::app::refresh_derived_gpu(&mut doc, &mut self.renderer, new_dpi);
            let nb = mn_core::project::doc_to_bytes(&doc)
                .map_err(|e| format!("page {} could not be re-encoded: {e}", i + 1))?;
            pending.push((i, PageResample::Bytes(nb, target)));
        }

        // --- phase 2: install, which cannot fail ---
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
        let target = scaled(self.doc.size);
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

        // The paper is the SAME paper: only the resolution moved.
        if let Some(s) = self.page.as_mut() {
            s.dpi = new_dpi;
        }
        self.preflight_stale = true;
        self.mark_pages_dirty();
        self.mark_dirty();
        Ok(touched)
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
