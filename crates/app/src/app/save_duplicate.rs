//! `IO-003` — File ▸ Save Duplicate…: write a COPY of the work to a picked
//! path and stay in the original. The everyday send-it move: hand someone
//! today's state, keep drawing in the file you were in.
//!
//! What makes it a duplicate rather than a Save As is everything it does
//! NOT do: `doc_path` never moves, the dirty flag never clears, the recents
//! list never learns the copy's name, and the autosave shadowing the REAL
//! file is left exactly where it is (clearing it would throw away the crash
//! net for the document you are still working in).
//!
//! The copy takes the shape of the work, not of the current path: a comic
//! duplicates as a work FOLDER (`work.mnc` + `pNNN.ora`) or a single-file
//! `.mnc` depending on the path you pick, and a one-page work as `.ora` —
//! the same three branches `SaveOraPath` has.
//!
//! ## The trap this module exists for
//!
//! `save_work_folder` is INCREMENTAL: it rewrites only pages whose `rev`
//! moved past `saved_rev`, and it updates that watermark plus the managed
//! file list as it goes. Pointed at an empty folder without care it would
//! (a) skip every already-saved page, leaving a duplicate missing most of
//! the comic, and (b) leave the ORIGINAL believing its pages are safe in a
//! directory it is not saved to — so the next real Save would skip them
//! too. Both halves are silent. So the duplicate save runs with the
//! bookkeeping zeroed (write everything) and the original's restored
//! afterwards, whether it succeeded or not.
//!
//! ## The freeze (2026-09-06)
//!
//! Every branch here used to ENCODE and WRITE on the window thread, which
//! measured 4.3 s for a three-page work — the last of the file commands that
//! could still make Windows paint "not responding" over the window. They go
//! through `cmd::save_bg` now, the same way `SaveOraPath` does: the snapshot
//! (a `Document::clone`, pointer copies) is taken here, the encode and the
//! write happen on the writer thread, and the "Saving…" pill covers the
//! wait. The three functions therefore return the line that says the copy is
//! *being* written; the line that says it landed arrives from
//! `save_bg::poll_saves`.
//!
//! Nothing in the ledger dance below changed shape: the borrow and the
//! give-back still happen on this thread, before the write runs, because
//! `save_bg::folder_page_ids` answers the one question the write used to
//! answer (which id each page got).

use std::path::Path;

use super::App;
use crate::cmd::save_bg::{self, PageEncode, Write};

/// The incremental-save bookkeeping a duplicate must borrow and give back.
struct Ledger {
    ids: Vec<u32>,
    saved: Vec<u64>,
    managed: Vec<String>,
    next_id: u32,
}

impl App {
    /// Write a copy of the work to `p`. Returns the status line, or the
    /// error to show — the caller changes NO document state either way.
    pub(crate) fn save_duplicate(&mut self, p: &Path) -> Result<String, String> {
        let is_work_index = p
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("work.mnc"));
        if is_work_index {
            return self.duplicate_work_folder(p);
        }
        if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mnc")) {
            return self.duplicate_single_file(p);
        }
        // Bare ORA: the current page only, exactly as Save As to an .ora
        // does — said out loud in the status so a comic's author is not
        // surprised by a one-page copy.
        self.stamp_doc_dpi();
        save_bg::submit(
            p.display().to_string(),
            // NOT a save: a duplicate that fails leaves the work exactly as
            // dirty (or as clean) as it already was, so `poll_saves` must
            // not re-dirty anything on its behalf.
            false,
            Write::Ora {
                path: p.to_path_buf(),
                page: PageEncode {
                    doc: Box::new(self.doc.clone()),
                    preview_png: None,
                },
            },
        );
        Ok(if self.is_comic() {
            format!(
                "writing duplicate: CURRENT PAGE ONLY to {} — pick .mnc or a folder for the whole comic",
                p.display()
            )
        } else {
            format!("writing duplicate to {}…", p.display())
        })
    }

    /// Single-file `.mnc`: `Project::save` writes every page unconditionally,
    /// so there is no watermark to protect — the only mutation is the
    /// current page's snapshot, which is the ordinary round trip.
    fn duplicate_single_file(&mut self, p: &Path) -> Result<String, String> {
        let (page_bytes, encodes) = self.project_pages_for_save()?;
        let mut proj =
            mn_core::Project::new(self.story.clone(), self.page.clone(), self.binding_right);
        proj.meta.expression = self.expression;
        proj.meta.spine_mm = self.spine_mm;
        proj.meta.cover = self.cover;
        proj.meta.template_page = self.template_page;
        proj.meta.print_margin_info = self.print_margin_info;
        proj.meta.print_crop_marks = self.print_crop_marks;
        proj.meta.profile = self.profile.clone();
        proj.meta.page_uids = self.page_uids();
        proj.pages = page_bytes;
        let n = proj.pages.len();
        save_bg::submit(
            p.display().to_string(),
            false,
            Write::Project {
                path: p.to_path_buf(),
                proj: Box::new(proj),
                encodes,
            },
        );
        Ok(format!(
            "writing duplicate to {} ({n} pages)…",
            p.display()
        ))
    }

    /// Work folder: see the module note. Zero the watermark so every page is
    /// written into the new folder, clear the managed list so the
    /// foreign-files guard judges the DESTINATION on its own contents, then
    /// give the original its ledger back.
    fn duplicate_work_folder(&mut self, index: &Path) -> Result<String, String> {
        if self.is_our_work_index(index) {
            return Err("that is this work's own folder — use Save".into());
        }
        let before = Ledger {
            ids: self.pages.iter().map(|e| e.id).collect(),
            saved: self.pages.iter().map(|e| e.saved_rev).collect(),
            managed: std::mem::take(&mut self.folder_managed),
            next_id: self.folder_next_id,
        };
        for e in &mut self.pages {
            e.saved_rev = 0;
        }
        let label = index.display().to_string();
        let out = self.save_work_folder_via(index, |wf, encodes, dir, managed| {
            let ids = save_bg::folder_page_ids(&wf);
            save_bg::submit(
                label,
                false,
                Write::Folder {
                    wf: Box::new(wf),
                    encodes,
                    dir: dir.to_path_buf(),
                    managed: managed.to_vec(),
                    verb: "duplicate written to",
                },
            );
            Ok((ids, 0))
        });
        for (e, (&id, &saved)) in self
            .pages
            .iter_mut()
            .zip(before.ids.iter().zip(before.saved.iter()))
        {
            e.id = id;
            e.saved_rev = saved;
        }
        self.folder_managed = before.managed;
        self.folder_next_id = before.next_id;
        out.map(|msg| msg.replacen("saved work folder", "writing duplicate to", 1))
            .map_err(|e| format!("duplicate failed: {e}"))
    }
}
