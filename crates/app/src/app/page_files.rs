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
        self.save_work_folder_via(index, |wf, dir, managed| {
            mn_core::project::save_folder(&wf, dir, managed).map_err(|e| e.to_string())
        })
    }

    /// [`Self::save_work_folder`] with the WRITE injected (item K,
    /// 2026-09-05). Everything this method does — the foreign-file refusal,
    /// stashing the live page, encoding every dirty page to `.ora` bytes —
    /// reads the live document and must stay on the UI thread. The last step,
    /// putting those bytes on disk, does not, and that is the step that used
    /// to hold the Windows message pump long enough for "not responding".
    ///
    /// `write` is handed the finished `WorkFolder` BY VALUE (it holds every
    /// page's bytes; cloning it to hand it to a thread would double the
    /// biggest allocation in the app) and answers with the page ids and the
    /// number of files rewritten, exactly like `project::save_folder`.
    pub fn save_work_folder_via(
        &mut self,
        index: &std::path::Path,
        write: impl FnOnce(
            mn_core::project::WorkFolder,
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
        self.stash_current_page()?;
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
                .map(|e| mn_core::project::FolderPage {
                    id: e.id,
                    rev: e.rev,
                    saved_rev: e.saved_rev,
                    exported_rev: e.exported_rev,
                    uid: e.uid,
                    // A still-blank template page materializes HERE — the
                    // one place bytes are truly required (the save). This
                    // is the lazy-blank design's single deliberate cost.
                    bytes: match (&e.bytes, e.blank) {
                        (Some(b), _) => b.clone(),
                        (None, Some((bw, bh, n))) => {
                            let doc = self.blank_page_doc_at(bw, bh, n);
                            mn_core::project::doc_to_bytes(&doc).unwrap_or_default()
                        }
                        (None, None) => Vec::new(),
                    },
                })
                .collect(),
        };
        let (ids, written) = write(wf, &dir, &self.folder_managed)?;
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
            &std::path::Path,
        ) -> Result<(Vec<u32>, usize), String>,
    ) -> Result<String, String> {
        let dir = index
            .parent()
            .ok_or_else(|| "autosave folder path has no parent".to_owned())?
            .to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        self.stash_current_page()?;
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
                .map(|e| mn_core::project::FolderPage {
                    id: e.id,
                    rev: e.rev,
                    // THE TRAP THIS WHOLE METHOD EXISTS FOR: the temp
                    // watermark is the skip key, and it is the only one
                    // this write advances.
                    saved_rev: e.autosaved_rev,
                    exported_rev: e.exported_rev,
                    uid: e.uid,
                    bytes: e.bytes.clone().unwrap_or_default(),
                })
                .collect(),
        };
        let (ids, written) = write(wf, &dir)?;
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
}
