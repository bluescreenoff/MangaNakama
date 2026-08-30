//! Bringing outside files IN as pages: file/folder import, the batch
//! import, and the underlay placement rules a draft scan lands under.
//! Cut out of `pages.rs`; the page slots themselves live there.

use super::App;
use mn_core::Document;

/// The layer name an imported image takes: the file's stem.
fn image_layer_name(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported".to_owned())
}

/// Scale `rgba` to sit inside a `pw × ph` page, plus the status note when
/// the file did not sit squarely on the paper.
///
/// Letterboxing is the honest answer to a mismatched aspect — the
/// alternative is reshaping a whole chapter's paper around one photo. Say
/// it in the status line and let the human decide.
pub(super) fn fit_to_paper(
    rgba: image::RgbaImage,
    pw: u32,
    ph: u32,
) -> (image::RgbaImage, Option<String>) {
    let (iw, ih) = (rgba.width(), rgba.height());
    let s = (pw as f32 / iw as f32).min(ph as f32 / ih as f32);
    let (tw, th) = (
        ((iw as f32 * s).round() as u32).max(1),
        ((ih as f32 * s).round() as u32).max(1),
    );
    let note = ((tw, th) != (pw, ph)).then(|| {
        format!(
            "{iw}x{ih} is not the page's shape — fitted to {tw}x{th} inside {pw}x{ph}, with margins"
        )
    });
    let fitted = if (tw, th) == (iw, ih) {
        rgba
    } else {
        image::imageops::resize(&rgba, tw, th, image::imageops::FilterType::Lanczos3)
    };
    (fitted, note)
}

/// Read an image file as a fitted 下書き underlay for a `pw × ph` page:
/// the layer name, the fitted pixels, and the aspect note.
///
/// The shared front half of BOTH import doors — `file_to_page_bytes`
/// (workflow audit #2) and the batch import (#4) — so the two can never
/// end up fitting the same photo differently.
pub(super) fn underlay_from_file(
    path: &std::path::Path,
    pw: u32,
    ph: u32,
) -> Result<(String, image::RgbaImage, Option<String>), String> {
    let rgba = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
    let (fitted, note) = fit_to_paper(rgba, pw, ph);
    Ok((image_layer_name(path), fitted, note))
}

/// Where a 下書き underlay lands in a page's stack, as `(slot, depth)`.
///
/// **The rule.** Normally the very BOTTOM of the stack at root depth: a
/// rough is what you draw over, so nothing already on the page may end up
/// underneath it.
///
/// **The exception** is CSP's "Fill inside the frame" White base. A page
/// that was blank or drawn carries one at the bottom of its frame folder,
/// and it paints the whole panel interior opaque — an underlay below it is
/// invisible exactly where the drawing happens (the part-19 lesson, and
/// the reason `file_to_page_bytes` seeds an IMPORTED page's folder with
/// `fill_white = false`). So on such a page the underlay goes directly
/// ABOVE the White base and INSIDE the folder, at the White's depth:
/// visible in the panel, listed in the palette, still under every ink
/// layer.
///
/// With several stacked frame folders the LOWEST White wins. An underlay
/// hidden by a folder above it is a visibility disappointment; an underlay
/// on top of that folder's ink would be a wrecked page.
fn underlay_slot(doc: &Document) -> (usize, u8) {
    match doc
        .layers
        .iter()
        .position(|l| !l.folder && l.name == "White")
    {
        Some(w) => (w + 1, doc.layers[w].depth),
        None => (0, 0),
    }
}

/// Put `img` into `doc` as a 下書き draft underlay at [`underlay_slot`].
/// Returns the index it landed at.
///
/// Records NOTHING: the byte-writing callers hold documents whose history
/// is thrown away with them, and the OPEN-page caller records the whole
/// stack ONCE beforehand so its change is a single undo press.
///
/// The layer is built in a throwaway document of the same size purely to
/// reuse `add_layer_from_image`'s centring; doing that on the real document
/// would push its own "New layer" structure group, and lowering the result
/// into place a second one.
pub(super) fn place_draft_underlay(
    doc: &mut Document,
    name: String,
    img: &image::RgbaImage,
) -> usize {
    let (slot, depth) = underlay_slot(doc);
    let mut scratch = Document::new(doc.size.0, doc.size.1);
    let at = scratch.add_layer_from_image(name, img);
    let mut layer = scratch.layers.remove(at);
    layer.depth = depth;
    layer.draft = true;
    doc.layers.insert(slot, layer);
    // The active layer must still be the layer it was: the insert shifted
    // everything at or above the slot up by one.
    if doc.active >= slot {
        doc.active += 1;
    }
    doc.touch();
    slot
}

impl App {
    /// Convert a file (.ora or image) to page ORA bytes, plus a status note
    /// when the file did not sit squarely on the paper. Used by ImportPage
    /// and ReplacePage; `number1` is the reading-order number the resulting
    /// page will carry, which decides the seeded frame's ノド/小口 side.
    ///
    /// **Workflow audit #2.** An image used to become a page of the IMAGE's
    /// own pixel size: a phone photo of a ネーム dropped into a B4/600 dpi
    /// chapter turned into a foreign-paper page with no trim, no bleed, no
    /// 基本枠 and no dpi, and — being an ordinary raster layer — its content
    /// EXPORTED as art. So the image branch now builds the work's own page
    /// (`blank_page_doc_at`, the same seeding a blank page gets) and places
    /// the photo in it scaled to fit, as a 下書き draft layer at the bottom
    /// of the stack: on screen, never in the export, drawn over.
    ///
    /// A work with no `PageSetup` is a plain canvas, not a manga project —
    /// there is no paper to inherit, so there the image's own size is still
    /// the only size there is and the old behaviour stands.
    pub fn file_to_page_bytes(
        &self,
        path: &std::path::Path,
        number1: usize,
    ) -> Result<(Vec<u8>, Option<String>), String> {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("ora") {
            // Already ORA: read raw bytes.
            return std::fs::read(path).map(|b| (b, None)).map_err(|e| e.to_string());
        }
        // Assume image.
        let Some((pw, ph)) = self.page.as_ref().map(|p| p.paper_px()) else {
            // Plain canvas: import as a single-layer doc at the image's size.
            let rgba = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
            let mut doc = mn_core::Document::new(rgba.width(), rgba.height());
            doc.add_layer_from_image(image_layer_name(path), &rgba);
            // Drop the empty default "Layer 1" underneath.
            if doc.layers.len() > 1 && doc.layers[1].is_empty() {
                doc.layers.remove(1);
                doc.active = 0;
            }
            let bytes = mn_core::project::doc_to_bytes(&doc).map_err(|e| e.to_string())?;
            return Ok((bytes, None));
        };
        let (name, fitted, note) = underlay_from_file(path, pw, ph)?;
        // fill_white = false (CSP's "Fill inside the frame" off): the
        // underlay goes to the BOTTOM of the stack, and the seeded
        // folder's White base would hide it across the whole panel
        // interior — an invisible 下書き is no 下書き. Export is
        // unchanged either way: the draft never prints, and panels
        // composite to paper white with or without the base.
        let mut doc = self.seeded_page_doc(pw, ph, number1, false);
        place_draft_underlay(&mut doc, name, &fitted);
        let bytes = mn_core::project::doc_to_bytes(&doc).map_err(|e| e.to_string())?;
        Ok((bytes, note))
    }

    /// Workflow audit #4 — CSP EX's *File ▸ Import ▸ Batch import*: the
    /// "I named the whole chapter on paper" step. Each picked file becomes
    /// the 下書き underlay of ONE page, in name order, starting at the
    /// dialog's page slot, and pages are ADDED when there are more images
    /// than pages.
    ///
    /// Two doors, by whether the target page exists:
    ///
    /// * **it exists** — the page keeps everything it has; only the
    ///   underlay is inserted, at [`underlay_slot`]. The OPEN page takes
    ///   that through `self.doc` with the whole stack recorded ONCE, so its
    ///   change is a single undo press; every other page is decoded from
    ///   its bytes, edited, and re-encoded.
    /// * **it does not** — a NEW page of the work's own paper through the
    ///   finding-2 door ([`App::file_to_page_bytes`]).
    ///
    /// The byte writes are the round trip `batch_other_pages` /
    /// [`App::resize_other_pages`] use, with the invariant that matters
    /// most since workflow audit #1: each written page takes a fresh
    /// content revision from `page_rev_next`, which is exactly what makes a
    /// parked live document stale (`PageEntry::parked_rev`) so a later
    /// switch decodes what the batch wrote instead of reinstalling the
    /// page as it was. Undo covers the OPEN page only — the dialog says so.
    ///
    /// The deferred half of the audit's row: CSP places the rectangle once
    /// with handles on page 1 and reuses it. That needs a cross-page
    /// placement gesture we do not have; every image is scale-to-fit here.
    pub fn batch_import_pages(&mut self) -> String {
        let files = std::mem::take(&mut self.batch_import.files);
        if files.is_empty() {
            return "batch import: no files were picked".into();
        }
        let Some((pw, ph)) = self.page.as_ref().map(|p| p.paper_px()) else {
            return "batch import: this work has no page setup — File ▸ Import Image as Draft…"
                .into();
        };
        if let Err(e) = self.stash_current_page() {
            return format!("batch import: {e}");
        }
        // 1-based slot -> index, clamped to "append at the end": a start
        // past the end would otherwise leave a hole of pages nobody asked
        // for between the chapter and the roughs.
        let start = self.batch_import.start.clamp(1, self.pages.len() + 1) - 1;
        let (mut written, mut added, mut failed) = (0usize, 0usize, 0usize);
        // ONE note, not N: twenty photos off the same phone all mismatch
        // the paper the same way, and twenty copies of that sentence is
        // not twenty times the information.
        let mut note: Option<String> = None;
        for (i, path) in files.iter().enumerate() {
            let target = start + i;
            if target >= self.pages.len() {
                // Past the end: a new page, the finding-2 way.
                let number1 = self.page_number1(self.pages.len());
                match self.file_to_page_bytes(path, number1) {
                    Ok((bytes, n)) => {
                        note = note.or(n);
                        let e = self.fresh_page(Some(bytes), None);
                        self.pages.push(e);
                        added += 1;
                    }
                    Err(e) => {
                        failed += 1;
                        self.set_error(format!("batch import: {} — {e}", path.display()));
                    }
                }
                continue;
            }
            let (name, fitted, n) = match underlay_from_file(path, pw, ph) {
                Ok(v) => v,
                Err(e) => {
                    failed += 1;
                    self.set_error(format!("batch import: {} — {e}", path.display()));
                    continue;
                }
            };
            note = note.or(n);
            if target == self.page_index {
                // THE OPEN PAGE. Record the pre-image once and then edit
                // the stack directly (the `comps.rs` pattern): going
                // through `add_layer_from_image` + `move_layer` would push
                // two structure groups, and the artist would need two undo
                // presses to take one import back.
                let before = self.doc.layers.clone();
                let active_before = self.doc.active;
                self.doc
                    .record_structure("Batch import underlay", before, active_before);
                place_draft_underlay(&mut self.doc, name, &fitted);
                self.renderer.invalidate();
                self.layer_thumbs.clear();
                written += 1;
                continue;
            }
            // A still-LAZY blank page has no bytes to decode — materialize
            // its template the way `save_work_folder` does.
            let blank = self.pages[target].blank;
            let mut doc = match self.pages[target].bytes.as_deref() {
                Some(b) => match mn_core::project::bytes_to_doc(b) {
                    Ok(d) => d,
                    Err(_) => {
                        failed += 1;
                        continue;
                    }
                },
                None => match blank {
                    Some((bw, bh, n1)) => self.blank_page_doc_at(bw, bh, n1),
                    None => {
                        failed += 1;
                        continue;
                    }
                },
            };
            place_draft_underlay(&mut doc, name, &fitted);
            let Ok(nb) = mn_core::project::doc_to_bytes(&doc) else {
                failed += 1;
                continue;
            };
            let rev = self.page_rev_next();
            let e = &mut self.pages[target];
            e.bytes = Some(nb);
            // It has real content now — the template marker is spent.
            e.blank = None;
            // THE park-staleness bump: `switch_page` compares this against
            // `parked_rev` and drops a parked document that no longer
            // matches the bytes.
            e.rev = rev;
            e.doc_rev = 0;
            e.thumb = None;
            written += 1;
        }
        // Restore the active-page invariant (bytes live in `doc`).
        self.pages[self.page_index].bytes = None;
        self.mark_pages_dirty();
        self.mark_dirty();
        let mut s = format!("batch import: {written} page(s) written, {added} added");
        if let Some(n) = note {
            s.push_str(&format!(" — {n}"));
        }
        if failed > 0 {
            s.push_str(&format!(" — {failed} file(s) could not be read"));
        }
        s
    }
}
