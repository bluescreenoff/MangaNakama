//! File objects, app half (row 166, `FO-001`/`FO-008`/`FO-009`).
//!
//! The document half — the layer kind, the derive, the refresh rules, the
//! ORA attributes — is `mn_core::file_object`; read its module doc first,
//! including the undo decision (a refresh is external truth and records
//! nothing). This file is the wiring: the import command, the two update
//! doors, the relink repair, and where "the file changed" is noticed.
//!
//! # When updates happen (v1)
//!
//! Honest polling at natural moments, no watcher thread:
//!
//! 1. **The app regains focus** — `WM_SETFOCUS` in `main.rs`. That IS the
//!    gesture: you alt-tab to the background file, redraw it, alt-tab back.
//!    Silent when nothing changed, so an idle click on the taskbar costs a
//!    `stat()` per file object and says nothing.
//! 2. **A document arrives from disk** — `App::set_doc_path`, the one line
//!    every open branch shares. That is what puts the broken-link mark on
//!    the palette row at LOAD time instead of at the next alt-tab, with no
//!    modal anywhere.
//! 3. **The explicit command** — *Update file objects* in the File menu and
//!    the command palette, for the case the stamp test missed (see the core
//!    module's v1-limits list) and for the artist who wants to be sure.
//!
//! Deferred, recorded rather than half-built:
//!
//! * a filesystem watcher;
//! * a refresh on PAGE SWITCH inside a work folder (`set_doc_path` fires
//!   once per work, not once per page). The focus door and the explicit
//!   command both cover it, and the alternative is a hook in the page
//!   install path;
//! * refreshing file objects on pages that are NOT open — their rasters
//!   live in parked ORA bytes, so a background pass would have to decode,
//!   edit and re-encode every page (the batch-import machinery) for a
//!   workflow whose payoff is on the page you are looking at.

use crate::App;
use mn_core::file_object::{RefreshReport, resolve};
use std::path::{Path, PathBuf};

impl App {
    /// The folder the document lives in — the basename fallback's search
    /// path (`mn_core::file_object::resolve`). `None` for a document that
    /// has never been saved, which simply means no fallback.
    fn file_object_near(&self) -> Option<PathBuf> {
        self.doc_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
    }

    /// Shared tail of every refresh: repaint what changed and report.
    fn after_file_object_refresh(&mut self, r: RefreshReport) {
        if r.is_quiet() {
            return;
        }
        // The tiles were swapped wholesale, so the GPU's per-tile cache and
        // the palette thumbnails are both stale.
        self.renderer.invalidate();
        self.layer_thumbs.clear();
        self.mark_dirty();
        self.needs_redraw = true;
    }

    /// `FO-001` — File ▸ Import ▸ Image as file object…
    pub fn import_file_object(&mut self, path: &Path) {
        match self.doc.add_file_object_layer(path) {
            Ok(at) => {
                self.renderer.invalidate();
                self.layer_thumbs.clear();
                self.mark_dirty();
                let name = self.doc.layers[at].name.clone();
                self.set_status(format!(
                    "“{name}” placed as a file object — it re-reads {} whenever the app \
                     comes back to the front",
                    path.display()
                ));
            }
            Err(e) => self.set_error(format!("file object: {e}")),
        }
    }

    /// The quiet door (focus regain, page open): says nothing unless
    /// something actually moved.
    pub fn refresh_file_objects_quiet(&mut self) {
        // Cheap out before touching the filesystem at all: the common
        // page has no file objects, and every alt-tab runs through here.
        if !self.doc.layers.iter().any(|l| l.file_object().is_some()) {
            return;
        }
        let near = self.file_object_near();
        let r = self.doc.refresh_file_objects(near.as_deref());
        self.after_file_object_refresh(r);
        if let Some(s) = r.status() {
            self.set_status(s);
        }
    }

    /// `FO-008` — the explicit *Update file objects* command. Always
    /// answers, including "there was nothing to do": a command you pressed
    /// on purpose and that says nothing reads as a broken command.
    pub fn update_file_objects(&mut self) {
        let near = self.file_object_near();
        let r = self.doc.refresh_file_objects(near.as_deref());
        self.after_file_object_refresh(r);
        let msg = match (r.checked, r.status()) {
            (0, _) => "no file objects on this page".to_owned(),
            (n, None) => format!("{n} file object(s) already up to date"),
            (_, Some(s)) => s,
        };
        self.set_status(msg);
    }

    /// `FO-009` — repoint layer `li` at `path` and re-derive. The repair
    /// path for a broken link; one undo press.
    pub fn relink_file_object(&mut self, li: usize, path: &Path) {
        match self.doc.relink_file_object(li, path) {
            Ok(()) => {
                self.renderer.invalidate();
                self.layer_thumbs.clear();
                self.mark_dirty();
                self.needs_redraw = true;
                self.set_status(format!("file object relinked to {}", path.display()));
            }
            Err(e) => self.set_error(format!("relink: {e}")),
        }
    }

    /// The layer the *Relink file object…* picker should aim at: the row
    /// the command named, or the active row when it named none. `None`
    /// when that row is not a file object — the command palette can offer
    /// the row on any layer, and refusing beats relinking the wrong one.
    pub fn relink_target(&self, li: Option<usize>) -> Option<usize> {
        let li = li.unwrap_or(self.doc.active);
        self.doc.layers.get(li)?.file_object().map(|_| li)
    }

    /// The source path to start the relink picker in: the folder the link
    /// last pointed at, so the common "it moved one folder over" repair is
    /// already most of the way there.
    pub fn file_object_dir(&self, li: usize) -> Option<PathBuf> {
        let fo = self.doc.layers.get(li)?.file_object()?;
        // Resolve first: if the basename fallback already found it beside
        // the work, that folder is the better guess than a dead absolute.
        let p = resolve(fo, self.file_object_near().as_deref()).unwrap_or_else(|| fo.path.clone());
        p.parent().map(Path::to_path_buf)
    }
}
