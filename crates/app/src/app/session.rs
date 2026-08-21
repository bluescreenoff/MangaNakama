//! Several documents open at once, one tab each.
//!
//! The owner, 2026-08-19: *"it would be bad if I have art in the default
//! canvas, and I make a new manga project and that automatically deletes it
//! — the new project should open in a new canvas tab."* He is right, and
//! until this module existed `File ▸ New` overwrote whatever you had open.
//!
//! # How this is built, and why it looks odd
//!
//! The obvious design is `Vec<Document>` with an index. That would mean
//! rewriting every `app.doc` in the codebase — thousands of sites — into
//! `app.docs[app.active].doc`, which is a refactor with no upside and a
//! large blast radius.
//!
//! So the ACTIVE document stays exactly where it always was: inline on
//! `App`, reached as `app.doc`, `app.pages`, `app.story` and friends. The
//! inactive ones are parked in [`DocSession`] values. `docs` has one slot
//! per open document **in tab order**, and the active slot is `None`
//! because its contents are live on the App. Switching tabs is two moves:
//! park the live fields into the slot we are leaving, take the slot we are
//! entering.
//!
//! Invariants, upheld by every function here:
//! * `docs` is never empty — closing the last document resets it to a blank
//!   one rather than leaving the app with nothing to draw on.
//! * exactly one slot is `None`, and it is `active`.
//!
//! # What must be reset on a switch
//!
//! Every cached thing that is keyed by LAYER INDEX belongs to the document
//! that produced it: the GPU tile cache, layer thumbnails, and the object
//! selections (a text/balloon/frame selection is an index into the old
//! document's layer list). Carrying any of them across is how you get the
//! other document's pixels — the exact bug the owner reported seeing after
//! `File ▸ New`.

use std::path::PathBuf;

use mn_core::{Document, PageSetup};
use mn_gpu::Viewport;

use super::{App, PageEntry};

/// One parked document: everything on `App` that belongs to a document
/// rather than to the app or the tool.
pub struct DocSession {
    pub doc: Document,
    pub viewport: Viewport,
    pub page: Option<PageSetup>,
    pub pages: Vec<PageEntry>,
    pub page_index: usize,
    pub story: String,
    pub binding_right: bool,
    pub seed_frame_folder: bool,
    pub expression: mn_core::Expression,
    pub spine_mm: f32,
    pub cover: Option<usize>,
    pub saved_revision: u64,
    pub pages_dirty: bool,
    pub doc_path: Option<PathBuf>,
    pub folder_managed: Vec<String>,
    pub folder_next_id: u32,
}

impl DocSession {
    /// The tab's label: the file name, else the story title, else
    /// "untitled" — and the page counter for a comic, which is what the
    /// single-document tab has always shown.
    pub fn tab_label(&self) -> String {
        let name = self
            .doc_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                if self.story.trim().is_empty() {
                    "untitled".to_owned()
                } else {
                    self.story.clone()
                }
            });
        if self.pages.len() > 1 {
            format!("{name}  {}/{}", self.page_index + 1, self.pages.len())
        } else {
            name
        }
    }

    pub fn dirty(&self) -> bool {
        self.doc.revision != self.saved_revision || self.pages_dirty
    }

    /// Encode this parked document as a project, so it can be autosaved
    /// without being made active first. The page being edited lives in
    /// `doc` with `bytes: None` in `pages`, exactly as it does on the App —
    /// that page is encoded here and the rest ride as they are.
    pub fn as_project(&self) -> Option<mn_core::Project> {
        let mut proj =
            mn_core::Project::new(self.story.clone(), self.page.clone(), self.binding_right);
        proj.meta.expression = self.expression;
        proj.meta.spine_mm = self.spine_mm;
        proj.meta.cover = self.cover;
        let active = mn_core::project::doc_to_bytes(&self.doc).ok()?;
        proj.pages = self
            .pages
            .iter()
            .enumerate()
            .map(|(i, e)| {
                if i == self.page_index {
                    active.clone()
                } else {
                    e.bytes.clone().unwrap_or_default()
                }
            })
            .collect();
        Some(proj)
    }
}

/// The stash for a never-saved document in slot `i`. One per slot: a single
/// shared path meant the tick overwrote one unsaved tab with another.
pub fn unsaved_autosave_path_for(i: usize) -> PathBuf {
    if i == 0 {
        // Slot 0 keeps the historical name, so a crash file written before
        // tabs existed is still found by the recovery scan.
        crate::recovery::unsaved_autosave_path()
    } else {
        std::env::temp_dir().join(format!("MangaNakama-autosave-{i}.mnc"))
    }
}

fn sibling_autosave(doc: &std::path::Path) -> PathBuf {
    crate::recovery::sibling_autosave(doc)
}

impl App {
    /// Move the live document fields into a parked session, leaving the App
    /// holding a throwaway 1×1 document. Always paired with an `install`.
    fn park(&mut self) -> DocSession {
        DocSession {
            doc: std::mem::replace(&mut self.doc, Document::new(1, 1)),
            viewport: self.viewport,
            page: self.page.take(),
            pages: std::mem::take(&mut self.pages),
            page_index: self.page_index,
            story: std::mem::take(&mut self.story),
            binding_right: self.binding_right,
            seed_frame_folder: self.seed_frame_folder,
            expression: self.expression,
            spine_mm: self.spine_mm,
            cover: self.cover,
            saved_revision: self.saved_revision,
            pages_dirty: self.pages_dirty,
            doc_path: self.doc_path.take(),
            folder_managed: std::mem::take(&mut self.folder_managed),
            folder_next_id: self.folder_next_id,
        }
    }

    fn install(&mut self, s: DocSession) {
        self.doc = s.doc;
        self.viewport = s.viewport;
        self.page = s.page;
        self.pages = s.pages;
        self.page_index = s.page_index;
        self.story = s.story;
        self.binding_right = s.binding_right;
        self.seed_frame_folder = s.seed_frame_folder;
        self.expression = s.expression;
        self.spine_mm = s.spine_mm;
        self.cover = s.cover;
        self.saved_revision = s.saved_revision;
        self.pages_dirty = s.pages_dirty;
        self.doc_path = s.doc_path;
        self.adopt_folder_state(s.folder_next_id, s.folder_managed);
    }

    /// Everything cached against the OLD document's layer indices. Called on
    /// both sides of a switch, because a stale entry is wrong in either
    /// direction.
    fn forget_document_caches(&mut self) {
        self.commit_text_edit();
        self.renderer.invalidate();
        self.layer_thumbs.clear();

        // Selections and armed gestures are LAYER INDICES into the document
        // that produced them. Carried across, each one aims an edit at
        // whatever happens to sit at that index in the other document.
        self.text_sel = None;
        self.object_sel = None;
        self.balloon_sel = None;
        self.gen_sel = None;
        // Rulers are per-document too (they ride `doc`, which parks with
        // the session), so a live move's index would aim at the OTHER
        // document's ruler set.
        self.ruler_move = None;
        // Vector stroke selection/drag index the parked document's set.
        self.vector_sel = None;
        self.vector_drag = None;
        self.renaming = None;
        self.frame_delete_armed = None;
        self.last_selection = None;
        self.eye_solo_backup = None;
        self.disarm_mask_edit_if_unmasked();

        // THE STORY EDITOR is the worst of the family, because it is a
        // non-modal window that stays open across a tab click: `story_docs`
        // holds DECODED PAGES of the document it was opened on, and its
        // write path re-encodes those pages into `self.pages` — i.e. typing
        // one character in it after a switch replaces the new document's
        // page with the old document's content, wholesale.
        self.story_open = false;
        self.story_docs.clear();
        self.story_bufs.clear();
        self.story_sel = None;

        // Derived per-document views: recomputed on demand, wrong if kept.
        self.frame_order = None;
        self.preview_order.clear();
        self.preflight_findings = None;
        self.preflight_rev = 0;
        self.preflight_page = 0;
        self.comp_selected = None;
        self.comp_multi.clear();
        self.comp_last_state = None;

        // THE READER caches by page index + rev, and independently-authored
        // works both count revs from 1 — so a carried texture map paints
        // work A's page as work B's, and carried flags leak into the next
        // work's sidecar on the first save. The options (rtl, mode, bg)
        // are the user's session taste and stay.
        self.reader.tex.clear();
        self.reader.flags.clear();
        self.reader.screen = 0;
        self.reader.visited = false;
        self.reader.show_flags = false;
    }

    /// How many documents are open. Always at least 1.
    pub fn doc_count(&self) -> usize {
        self.docs.len().max(1)
    }

    /// Tab labels in tab order, with the dirty flag — the active tab reads
    /// its label from the live fields, the rest from their parked sessions.
    pub fn doc_tabs(&self) -> Vec<(String, bool)> {
        (0..self.doc_count())
            .map(|i| match self.docs.get(i).and_then(|s| s.as_ref()) {
                Some(s) => (s.tab_label(), s.dirty()),
                None => (self.active_tab_label(), self.dirty()),
            })
            .collect()
    }

    fn active_tab_label(&self) -> String {
        let name = self
            .doc_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                if self.story.trim().is_empty() {
                    "untitled".to_owned()
                } else {
                    self.story.clone()
                }
            });
        if self.pages.len() > 1 {
            format!("{name}  {}/{}", self.page_index + 1, self.pages.len())
        } else {
            name
        }
    }

    /// Make sure the slot table exists. A fresh App has one document and an
    /// empty table; this is the lazy one-time initialisation so nothing in
    /// `App::new` has to know about tabs.
    fn ensure_slots(&mut self) {
        if self.docs.is_empty() {
            self.docs.push(None);
            self.active_doc = 0;
        }
    }

    /// Switch to document `i`. No-op (returns false) for the active tab or
    /// an index that does not exist.
    pub fn switch_doc(&mut self, i: usize) -> bool {
        self.ensure_slots();
        if i == self.active_doc || i >= self.docs.len() {
            return false;
        }
        self.forget_document_caches();
        let parked = self.park();
        self.docs[self.active_doc] = Some(parked);
        let Some(next) = self.docs[i].take() else {
            // Cannot happen while the invariant holds; restoring is still
            // better than panicking on a user's page.
            let back = self.docs[self.active_doc].take().expect("just parked");
            self.install(back);
            return false;
        };
        self.install(next);
        self.active_doc = i;
        self.forget_document_caches();
        self.mark_dirty();
        true
    }

    /// Park the current document and start a NEW empty slot beside it,
    /// which becomes active. The caller then builds the new document into
    /// the live fields exactly as it always did — this is why `File ▸ New`
    /// needed only one line to stop destroying your work.
    pub fn push_doc_slot(&mut self) {
        self.ensure_slots();
        self.forget_document_caches();
        let parked = self.park();
        self.docs[self.active_doc] = Some(parked);
        self.docs.push(None);
        self.active_doc = self.docs.len() - 1;
        // The live fields currently hold the 1×1 placeholder `park` left
        // behind; the caller overwrites them immediately.
    }

    /// The first document with unsaved changes, active one included.
    ///
    /// The close flow HAS to ask this rather than `self.dirty()`: with tabs,
    /// `dirty()` only speaks for the document you happen to be looking at,
    /// and quitting would drop the work in every other one without a word.
    /// That hole opened the moment tabs landed, which is the shape of bug
    /// worth naming in a doc comment.
    pub fn first_dirty_doc(&self) -> Option<usize> {
        (0..self.doc_count()).find(|&i| match self.docs.get(i).and_then(|s| s.as_ref()) {
            Some(s) => s.dirty(),
            None => self.dirty(),
        })
    }

    /// Autosave every PARKED document that has unsaved changes.
    ///
    /// The 15-minute tick only ever knew about the active document, which
    /// since tabs means unsaved work in a background tab was never written
    /// anywhere and a crash took it with no recovery file to offer. Worse,
    /// two never-saved documents shared ONE `%TEMP%` path, so the tick
    /// overwrote one tab's stash with another's.
    ///
    /// Parked sessions are encoded straight from their own fields — no tab
    /// switching, so this cannot disturb what the user is drawing. Failures
    /// are swallowed on purpose: an autosave that cannot write must never
    /// interrupt the session (the preview-render policy).
    pub fn autosave_parked(&mut self) -> usize {
        let mut written = 0;
        for i in 0..self.docs.len() {
            let Some(s) = self.docs.get(i).and_then(|s| s.as_ref()) else {
                continue; // the active slot; the normal Autosave arm has it
            };
            if !s.dirty() {
                continue;
            }
            let Some(proj) = s.as_project() else { continue };
            // Beside its own file when it has one, else its OWN temp stash —
            // keyed by slot, which is the collision fix.
            let path = match s.doc_path.as_ref() {
                Some(p) => sibling_autosave(p),
                None => unsaved_autosave_path_for(i),
            };
            if mn_core::project::save(&proj, &path).is_ok() {
                written += 1;
            }
        }
        written
    }

    /// Does document `i` have unsaved changes? Works for parked tabs too,
    /// which is the whole point: the tab × has to ask before it destroys
    /// one, and `dirty()` only ever spoke for the active document.
    pub fn doc_dirty(&self, i: usize) -> bool {
        match self.docs.get(i).and_then(|s| s.as_ref()) {
            Some(s) => s.dirty(),
            None => i == self.active_doc && self.dirty(),
        }
    }

    /// Give up on this document's unsaved changes — the "No" answer to the
    /// save prompt. Only the ACTIVE document can be discarded, because only
    /// it has live state; the close flow switches to each dirty tab first.
    pub fn discard_changes(&mut self) {
        self.mark_saved();
    }

    /// About to load a document from disk: decide whether it lands in a new
    /// tab or replaces what is here.
    ///
    /// It REPLACES only an untouched blank — the document you get at startup
    /// and have not drawn on or saved. Anything else opens beside it,
    /// because "open a file" should never be the gesture that loses work.
    /// The rule also stops the tab strip filling with empty canvases the way
    /// it would if every open pushed a slot unconditionally.
    ///
    /// Call it from a load's SUCCESS path, immediately before installing the
    /// new document — a failed load must not leave an empty tab behind.
    pub fn prepare_open_target(&mut self) {
        let untouched = self.doc_path.is_none() && !self.dirty() && self.pages.len() <= 1;
        if untouched {
            self.forget_document_caches();
        } else {
            self.push_doc_slot();
        }
    }

    /// Close document `i`. Returns false when it is the last one — the
    /// caller decides what "close the only document" means (we reset it to
    /// a blank page rather than leaving the app with nothing).
    pub fn close_doc(&mut self, i: usize) -> bool {
        self.ensure_slots();
        if i >= self.docs.len() || self.docs.len() < 2 {
            return false;
        }
        self.forget_document_caches();
        if i == self.active_doc {
            // Closing the active tab: step to the neighbour on the left,
            // else the one on the right — the behaviour every editor has.
            let next = if i > 0 { i - 1 } else { 1 };
            let Some(session) = self.docs[next].take() else {
                return false;
            };
            self.docs.remove(i);
            self.install(session);
            self.active_doc = if next > i { next - 1 } else { next };
        } else {
            self.docs.remove(i);
            if self.active_doc > i {
                self.active_doc -= 1;
            }
        }
        self.forget_document_caches();
        self.mark_dirty();
        true
    }

    /// PR-041 — "save recovery data for every operation". Has an operation
    /// finished that the recovery copy on disk does not cover yet?
    ///
    /// `main::pump_commands` asks once per drained batch and runs
    /// [`crate::cmd::AppCmd::Autosave`] when the answer is yes. Asking is
    /// free (two field reads and a comparison) and the answer is an EDGE,
    /// so a pass that writes nothing costs nothing and a pass that does
    /// write cannot write twice for the same operation.
    ///
    /// Three guards, and each one is load-bearing:
    ///
    /// - **`drawing()`** — mid-stroke the tiles are the brush engine's, and
    ///   a stroke is not an operation until it ends. Note the counter is
    ///   deliberately NOT advanced when this is what stopped us, so the
    ///   save happens the moment the pen lifts.
    /// - **`dirty()`** — nothing to protect, and the timer arm skips a
    ///   clean document for the same reason.
    /// - **the count** — `Document::op_count`, not `Document::revision`:
    ///   dragging the opacity slider bumps the revision every frame and
    ///   would otherwise rewrite the whole recovery file every frame.
    ///
    /// Switching tabs also moves the count (a different document, a
    /// different tally), which costs one extra save on the switch. That is
    /// the harmless direction to be wrong in for a recovery feature.
    pub fn autosave_op_due(&mut self) -> bool {
        // `Autosave: Off` means off. The panel greys the checkbox out to
        // say so; this is the same rule where it is enforced, so a
        // hand-edited `prefs.txt` cannot get a different answer.
        if self.prefs.autosave_min == 0 || !self.prefs.autosave_every_op {
            return false;
        }
        if self.drawing() || !self.dirty() {
            return false;
        }
        let ops = self.doc.op_count();
        if ops == self.autosave_op_seen {
            return false;
        }
        self.autosave_op_seen = ops;
        true
    }
}

/// PR-041 — "save recovery data for every operation".
#[cfg(test)]
mod per_op_autosave_tests {
    use super::*;

    /// One undoable edit, the cheapest thing that counts as an operation.
    fn edit(app: &mut App) {
        app.doc.begin_op();
        let li = app.doc.active;
        app.doc.layers[li]
            .tile_mut(mn_core::TileIdx::new(0, 0))
            .set_pixel(1, 1, [32768, 0, 0, 32768]);
        app.doc.end_op();
    }

    #[test]
    fn it_fires_once_per_operation_and_only_when_asked_for() {
        let Some(renderer) = crate::app::headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (600, 400), 1.0);

        // OFF by default — a user who never opens Preferences keeps
        // exactly today's behaviour, which is the rule the whole prefs
        // round was built on.
        assert!(!app.prefs.autosave_every_op);
        edit(&mut app);
        assert!(!app.autosave_op_due(), "off is off");

        app.prefs.autosave_every_op = true;
        assert!(app.autosave_op_due(), "an operation is outstanding");
        assert!(
            !app.autosave_op_due(),
            "an EDGE, not a level: asking twice must not write twice"
        );

        edit(&mut app);
        assert!(app.autosave_op_due(), "the next operation arms it again");
        assert!(!app.autosave_op_due());

        // `Autosave: Off` outranks the checkbox — the panel greys it out,
        // and a hand-edited prefs.txt gets the same answer.
        app.prefs.autosave_min = 0;
        edit(&mut app);
        assert!(!app.autosave_op_due(), "Off has to mean off");

        // A saved document has nothing to protect.
        app.prefs.autosave_min = 15;
        app.mark_saved();
        assert!(!app.autosave_op_due(), "clean: nothing to recover");
    }

    /// The one guard that cannot be inferred from the others: mid-stroke
    /// the answer is no, and the operation is NOT consumed — so the save
    /// lands the moment the pen lifts rather than being skipped.
    #[test]
    fn a_stroke_in_progress_defers_rather_than_swallows() {
        let Some(renderer) = crate::app::headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (600, 400), 1.0);
        app.prefs.autosave_every_op = true;

        edit(&mut app);
        app.begin_stroke(crate::app::PointerKind::Mouse);
        assert!(app.drawing(), "the stroke is open");
        assert!(!app.autosave_op_due(), "not while the pen is down");

        app.end_stroke();
        assert!(!app.drawing());
        assert!(
            app.autosave_op_due(),
            "the deferred operation was not swallowed"
        );
    }
}
