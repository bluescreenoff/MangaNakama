//! Writing a multi-page work to disk: the work folder save and its
//! autosave twin, plus the file names and index test they agree on.
//! Cut out of `pages.rs`.

use super::App;

impl App {
    /// File names the current pages map to in a work folder (the managed set).
    pub fn page_file_names(&self) -> Vec<String> {
        self.pages
            .iter()
            .map(|e| mn_core::project::page_file_name(e.id))
            .collect()
    }

    /// Is `p` the index of the work folder we are currently editing?
    pub fn is_our_work_index(&self, p: &std::path::Path) -> bool {
        self.doc_path.as_deref() == Some(p)
    }

    /// Save the whole work into `index`'s folder — the native multi-page
    /// format: `work.mnc` (tiny index) + `pNNN.ora` side by side, rewriting
    /// only pages whose revision advanced (see `mn_core::project::save_folder`
    /// for the atomicity story). Refuses to touch a folder that already holds
    /// work files that are not ours.
    pub fn save_work_folder(&mut self, index: &std::path::Path) -> Result<String, String> {
        self.save_work_folder_via(index, |mut wf, encodes, dir, managed| {
            for (i, page) in encodes {
                wf.pages[i].bytes = page.encode()?;
            }
            mn_core::project::save_folder(&wf, dir, managed).map_err(|e| e.to_string())
        })
    }

    /// [`Self::save_work_folder`] with the WRITE injected (item K).
    ///
    /// What stays on the UI thread is only what has to: the foreign-file
    /// refusal, landing the live stroke, the GPU-rendered page preview, and
    /// the page bookkeeping. BOTH slow halves — encoding a page to `.ora`
    /// bytes and putting those bytes on disk — go to `write`, which the app
    /// points at the background writer (`cmd/save_bg.rs`). Measured: the
    /// encode is ~4.8 s a page against 1 ms for the disk write, so moving
    /// only the write (round 1) moved nothing the artist could feel.
    ///
    /// `write` gets the `WorkFolder` BY VALUE (it holds every page's bytes;
    /// cloning it would double the biggest allocation in the app) plus the
    /// pages that arrive as DOCUMENT SNAPSHOTS rather than bytes, and answers
    /// with the page ids and the number of files rewritten, exactly like
    /// `project::save_folder`.
    ///
    /// A page already safe on disk at its current revision is handed over as
    /// EMPTY bytes with no snapshot: `save_folder` skips writing it, and now
    /// nothing copies or encodes it either. That skip is the exact negation
    /// of `save_folder`'s write condition — keep the two in step, or a page
    /// gets written empty.
    pub fn save_work_folder_via(
        &mut self,
        index: &std::path::Path,
        write: impl FnOnce(
            mn_core::project::WorkFolder,
            Vec<(usize, crate::cmd::save_bg::PageEncode)>,
            &std::path::Path,
            &[String],
        ) -> Result<(Vec<u32>, usize), String>,
    ) -> Result<String, String> {
        let dir = index
            .parent()
            .ok_or_else(|| "work folder path has no parent".to_owned())?
            .to_path_buf();
        if !self.is_our_work_index(index) {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                let foreign = rd.flatten().any(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    mn_core::project::is_workfolder_file(&n)
                        && !self.folder_managed.iter().any(|m| *m == n)
                });
                if foreign {
                    return Err("folder already holds work files — pick an empty folder".into());
                }
            }
        }
        let mut encodes = Vec::new();
        if let Some(page) = self.snapshot_active_page()? {
            encodes.push((self.page_index, page));
        }
        let bytes = self.page_bytes_for_folder_save(&dir, false, &mut encodes);
        let wf = mn_core::project::WorkFolder {
            story: self.story.clone(),
            binding_right: self.binding_right,
            setup: self.page.clone(),
            expression: self.expression,
            spine_mm: self.spine_mm,
            cover: self.cover,
            template_page: self.template_page,
            print_margin_info: self.print_margin_info,
            print_crop_marks: self.print_crop_marks,
            profile: self.profile.clone(),
            next_id: self.folder_next_id,
            pages: self
                .pages
                .iter()
                .zip(bytes)
                .map(|(e, bytes)| mn_core::project::FolderPage {
                    id: e.id,
                    rev: e.rev,
                    saved_rev: e.saved_rev,
                    exported_rev: e.exported_rev,
                    uid: e.uid,
                    bytes,
                })
                .collect(),
        };
        let (ids, written) = write(wf, encodes, &dir, &self.folder_managed)?;
        for (e, &id) in self.pages.iter_mut().zip(&ids) {
            e.id = id;
            e.saved_rev = e.rev.max(1);
        }
        // The active page keeps living in `doc`, not in bytes.
        self.pages[self.page_index].bytes = None;
        self.folder_managed = self
            .pages
            .iter()
            .map(|e| mn_core::project::page_file_name(e.id))
            .collect();
        let max_id = self.pages.iter().map(|e| e.id).max().unwrap_or(0);
        self.folder_next_id = self.folder_next_id.max(max_id + 1);
        Ok(format!(
            "saved work folder {} ({} pages, {written} rewritten)",
            dir.display(),
            self.pages.len()
        ))
    }

    /// Autosave the whole work into a TEMP work folder — `index` is
    /// [`crate::app::unsaved_autosave_folder_for`]'s
    /// `%TEMP%\MangaNakama-autosave[-N]\work.mnc` (05 item 1: the
    /// pathless-work crash net). Same per-dirty-page incremental format
    /// as [`Self::save_work_folder`], with two deliberate differences:
    ///
    /// * the skip key is each page's `autosaved_rev` watermark, and ONLY
    ///   that advances — `saved_rev` still means "safe in the work's real
    ///   home", so a later Save As rewrites every page it should.
    /// * no stale-file cleanup and no foreign-file refusal: the folder is
    ///   ours by construction (slot-keyed under `%TEMP%`) and dies whole
    ///   in `recovery::clear_unsaved_stash`.
    /// The write is INJECTED — the autosave twin of
    /// [`Self::save_work_folder_via`], and for the same reason: an autosave
    /// tick that blocks the message pump is the worst place to block it,
    /// because it lands while the artist is drawing rather than when they
    /// asked for something. (There is no plain `autosave_work_folder`
    /// wrapper next to this one on purpose: `AppCmd::Autosave` is the only
    /// caller, and a second door that writes on this thread is exactly the
    /// door someone would use by accident.)
    pub fn autosave_work_folder_via(
        &mut self,
        index: &std::path::Path,
        write: impl FnOnce(
            mn_core::project::WorkFolder,
            Vec<(usize, crate::cmd::save_bg::PageEncode)>,
            &std::path::Path,
        ) -> Result<(Vec<u32>, usize), String>,
    ) -> Result<String, String> {
        let dir = index
            .parent()
            .ok_or_else(|| "autosave folder path has no parent".to_owned())?
            .to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut encodes = Vec::new();
        if let Some(page) = self.snapshot_active_page()? {
            encodes.push((self.page_index, page));
        }
        let bytes = self.page_bytes_for_folder_save(&dir, true, &mut encodes);
        let wf = mn_core::project::WorkFolder {
            story: self.story.clone(),
            binding_right: self.binding_right,
            setup: self.page.clone(),
            expression: self.expression,
            spine_mm: self.spine_mm,
            cover: self.cover,
            template_page: self.template_page,
            print_margin_info: self.print_margin_info,
            print_crop_marks: self.print_crop_marks,
            profile: self.profile.clone(),
            next_id: self.folder_next_id,
            pages: self
                .pages
                .iter()
                .zip(bytes)
                .map(|(e, bytes)| mn_core::project::FolderPage {
                    id: e.id,
                    rev: e.rev,
                    // THE TRAP THIS WHOLE METHOD EXISTS FOR: the temp
                    // watermark is the skip key, and it is the only one
                    // this write advances.
                    saved_rev: e.autosaved_rev,
                    exported_rev: e.exported_rev,
                    uid: e.uid,
                    bytes,
                })
                .collect(),
        };
        let (ids, written) = write(wf, encodes, &dir)?;
        for (e, &id) in self.pages.iter_mut().zip(&ids) {
            e.id = id;
            e.autosaved_rev = e.rev.max(1);
        }
        // The active page keeps living in `doc`, not in bytes.
        self.pages[self.page_index].bytes = None;
        let max_id = self.pages.iter().map(|e| e.id).max().unwrap_or(0);
        self.folder_next_id = self.folder_next_id.max(max_id + 1);
        Ok(format!("{} page(s) -> {}", written, dir.display()))
    }

    /// The active page's content for a SAVE, without encoding it — the encode
    /// is what the writer thread is for (item K round 2).
    ///
    /// Everything [`Self::stash_current_page`] does except `doc_to_bytes`:
    /// land the stroke and the text edit, refresh the palette thumbnail,
    /// render the sharp page preview (GPU, so it cannot leave this thread),
    /// and advance the page's revision bookkeeping. What it hands back is a
    /// `Document` snapshot — pointer copies, measured under a millisecond,
    /// against ~4.8 s for the encode it defers.
    ///
    /// `None` = the page's stashed bytes are already current, the same skip
    /// `stash_current_page` makes.
    ///
    /// The page's own `bytes` are CLEARED rather than replaced: they are
    /// stale the moment the document moves past them, the fresh ones are
    /// being built on another thread, and every caller of this method already
    /// ended by clearing them ("the active page keeps living in `doc`").
    fn snapshot_active_page(&mut self) -> Result<Option<crate::cmd::save_bg::PageEncode>, String> {
        self.end_stroke();
        self.commit_text_edit();
        let i = self.page_index;
        let changed = self.pages[i].doc_rev != self.doc.revision;
        // A lazy-blank page that was never touched stashes to NOTHING: its
        // template marker still describes it exactly.
        if !changed && (self.pages[i].bytes.is_some() || self.pages[i].blank.is_some()) {
            return Ok(None);
        }
        // A preview failure never blocks the save — the page simply rides
        // without the sharp preview, exactly as pre-preview files do.
        let preview_png = self.render_page_preview_png().ok();
        let thumb = self.thumb_of_current();
        let rev = if changed { self.page_rev_next() } else { 0 };
        let doc = Box::new(self.doc.clone());
        let e = &mut self.pages[i];
        e.thumb = Some(thumb);
        e.blank = None;
        e.bytes = None;
        if changed {
            e.rev = rev;
            e.doc_rev = self.doc.revision;
        }
        Ok(Some(crate::cmd::save_bg::PageEncode { doc, preview_png }))
    }

    /// One `bytes` entry per page for a work-folder write, plus any extra
    /// encode jobs appended to `encodes`.
    ///
    /// A page already on disk at its current revision contributes EMPTY bytes
    /// and no job: `save_folder` skips writing it, and this is what stops the
    /// caller from cloning (or re-encoding) megabytes for it first. The skip
    /// test is the exact negation of `save_folder`'s write test —
    /// `rev > saved_rev || !path.exists()` — so a page it decides to write
    /// always has real bytes or a job. The `temp` flag picks the watermark:
    /// the autosave folder skips on `autosaved_rev`, the real save on
    /// `saved_rev`.
    fn page_bytes_for_folder_save(
        &mut self,
        dir: &std::path::Path,
        temp: bool,
        encodes: &mut Vec<(usize, crate::cmd::save_bg::PageEncode)>,
    ) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = Vec::with_capacity(self.pages.len());
        for i in 0..self.pages.len() {
            let e = &self.pages[i];
            let watermark = if temp { e.autosaved_rev } else { e.saved_rev };
            let on_disk = e.rev <= watermark
                && dir.join(mn_core::project::page_file_name(e.id)).exists();
            if on_disk || encodes.iter().any(|(j, _)| *j == i) {
                out.push(Vec::new());
                continue;
            }
            if e.bytes.is_some() {
                out.push(self.pages[i].bytes.clone().unwrap_or_default());
                continue;
            }
            // A still-blank template page materializes HERE — the one place
            // bytes are truly required (the save). The autosave folder does
            // not materialize blanks; a blank page has nothing to lose in a
            // crash. It is now the writer thread that pays for it.
            match (temp, e.blank) {
                (false, Some((bw, bh, n))) => {
                    let doc = Box::new(self.blank_page_doc_at(bw, bh, n));
                    encodes.push((
                        i,
                        crate::cmd::save_bg::PageEncode {
                            doc,
                            preview_png: None,
                        },
                    ));
                    out.push(Vec::new());
                }
                _ => out.push(Vec::new()),
            }
        }
        out
    }

    /// The page bytes for a single-file `.mnc` (save or export), plus the
    /// encode jobs for the pages that are not bytes yet — today only the live
    /// page, which is handed over as a snapshot instead of being encoded here.
    pub fn project_pages_for_save(
        &mut self,
    ) -> Result<(Vec<Vec<u8>>, Vec<(usize, crate::cmd::save_bg::PageEncode)>), String> {
        let mut encodes = Vec::new();
        if let Some(page) = self.snapshot_active_page()? {
            encodes.push((self.page_index, page));
        }
        let bytes = self
            .pages
            .iter()
            .map(|e| e.bytes.clone().unwrap_or_default())
            .collect();
        Ok((bytes, encodes))
    }
}
