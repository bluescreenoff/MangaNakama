//! Panel reading order — the App side (owner top item 2026-08-18).
//! Core algorithm: `mn_core::frame_order`. Here: recompute on every
//! frame change, auto-rename ONLY default `Frame N` names (a hand-typed
//! name is the owner's — its row shows the computed index as a badge),
//! manual pins persisted on the FrameSet, and the cached order the
//! Layers badge + the on-canvas reading-path overlay read.

use super::App;
use mn_core::frame_order::{self, FolderInput};

impl App {
    /// Recompute the reading order, renumber default-named folders, and
    /// cache the order for the UI. Call after ANY frame change (create,
    /// divide, delete, page switch) — the order is a computed property,
    /// never a name assigned once at birth.
    pub fn renumber_frames(&mut self) {
        self.recompute_frame_order(true);
    }

    /// The cache is keyed on the document revision, so it survives exactly
    /// as long as the geometry and the layer INDICES it holds do: adding,
    /// duplicating, moving or merging a layer shifts those indices without
    /// being a "frame command", and undo/redo/open move them wholesale.
    /// Called once per frame at the head of `App::render`, before the UI
    /// reads it — the recompute only happens when the revision moved.
    pub(crate) fn ensure_frame_order(&mut self) {
        if self.frame_order.is_none() || self.frame_order_rev != self.doc.revision {
            // Silent: a plain brush stroke moves the revision too, and the
            // ambiguity notice belongs to a frame command the user just ran.
            self.recompute_frame_order(false);
        }
    }

    /// The recompute itself. `announce` = say so when the layout is
    /// ambiguous; only the explicit frame commands do.
    fn recompute_frame_order(&mut self, announce: bool) {
        self.sync_frame_rulers();
        self.frame_order = None;
        // Stamped up front: renumbering writes `l.name` directly and never
        // touches the document, so the revision cannot move under us here.
        self.frame_order_rev = self.doc.revision;
        let spread = self.pages.get(self.page_index).is_some_and(|e| e.spread);
        // Cut tolerance: the gutter the divides themselves use (the
        // larger of the two prefs, in px) — a hair of bleed or overlap
        // must not break a cut a human sees plainly.
        let tol = self
            .mm_to_px(self.gutter_folder_mm.0.max(self.gutter_border_mm.0))
            .max(2.0);
        let sets: Vec<Option<&mn_core::FrameSet>> =
            self.doc.layers.iter().map(|l| l.frames()).collect();
        let folders: Vec<FolderInput<'_>> = self
            .doc
            .layers
            .iter()
            .enumerate()
            .filter(|(_, l)| l.folder && l.is_frame())
            .map(|(i, _l)| FolderInput {
                layer: i,
                set: sets[i].unwrap(),
                pin: sets[i].unwrap().reading_pin,
            })
            .collect();
        if folders.is_empty() {
            return;
        }
        let order = frame_order::reading_order(
            &folders,
            self.binding_right,
            spread,
            self.doc.size.0 as f32,
            tol,
        );
        if order.panels.is_empty() {
            return;
        }

        // Each folder's reading position = its first panel's index + 1.
        let mut first_pos: std::collections::HashMap<usize, usize> = Default::default();
        for (idx, pr) in order.panels.iter().enumerate() {
            first_pos.entry(pr.layer).or_insert(idx + 1);
        }
        for (&li, &pos) in &first_pos {
            let Some(l) = self.doc.layers.get_mut(li) else {
                continue;
            };
            if is_default_frame_name(&l.name) {
                l.name = format!("Frame {pos}");
            }
        }
        let ambiguous_n = order.ambiguous.iter().filter(|a| **a).count();
        if announce && ambiguous_n > 0 {
            self.set_status(format!(
                "panel order: {ambiguous_n} panel(s) in an ambiguous layout — check the badges"
            ));
        }
        self.frame_order = Some(order);
        self.needs_redraw = true;
    }

    /// The reading position of a frame folder (its first panel's), for
    /// the row badge; None when no order is cached.
    pub fn frame_pos(&self, li: usize) -> Option<(usize, bool, bool)> {
        // (1-based position, ambiguous, pinned)
        let o = self.frame_order.as_ref()?;
        let idx = o.panels.iter().position(|p| p.layer == li)?;
        let pinned = self
            .doc
            .layers
            .get(li)
            .and_then(|l| l.frames())
            .is_some_and(|fs| fs.reading_pin.is_some());
        Some((
            idx + 1,
            o.ambiguous.get(idx).copied().unwrap_or(false),
            pinned,
        ))
    }

    /// Pin a folder's reading position one step earlier/later than its
    /// CURRENT computed position (the badge context menu's arrows).
    /// Persists on the FrameSet; recomputes the order + names.
    pub fn frame_pin_step(&mut self, li: usize, delta: i32) {
        let Some(cur) = self.frame_pos(li) else {
            return;
        };
        // Base the new pin on the PINNED position when one exists, so
        // repeated steps accumulate.
        let base = self
            .doc
            .layers
            .get(li)
            .and_then(|l| l.frames())
            .and_then(|fs| fs.reading_pin)
            .unwrap_or(cur.0 as u32);
        let next = base.saturating_add_signed(delta).max(1);
        if let Some(l) = self.doc.layers.get_mut(li)
            && let Some(fs) = l.frames_mut()
        {
            fs.reading_pin = Some(next);
        }
        self.renumber_frames();
        self.mark_dirty();
    }

    /// TRIAGE 127 (`FB-053`/`FB-054`): keep the curve-ruler set in step with
    /// every frame folder whose border is a ruler instead of ink. Called on
    /// every frame change (it rides `renumber_frames`), so moving or
    /// reshaping such a panel moves the ruler with it.
    ///
    /// The retraction is by VALUE, against what this function added last
    /// time — a hand-drawn curve ruler that happens to sit in the list is
    /// never the one removed. Turning the first one on turns snapping on,
    /// because a ruler you cannot snap to is not a feature.
    pub fn sync_frame_rulers(&mut self) {
        let want: Vec<mn_core::CurveRuler> = self
            .doc
            .layers
            .iter()
            .filter_map(|l| l.frames())
            .flat_map(|fs| fs.ruler_curves())
            .collect();
        if want == self.frame_rulers {
            return;
        }
        for old in std::mem::take(&mut self.frame_rulers) {
            if let Some(i) = self.doc.rulers.curves.iter().position(|c| *c == old) {
                self.doc.rulers.curves.remove(i);
            }
        }
        self.doc.rulers.curves.extend(want.iter().cloned());
        if !want.is_empty() {
            self.doc.rulers.on = true;
        }
        self.frame_rulers = want;
        self.needs_redraw = true;
    }

    /// Drop a folder's pin — back to fully automatic ordering.
    pub fn frame_pin_clear(&mut self, li: usize) {
        if let Some(l) = self.doc.layers.get_mut(li)
            && let Some(fs) = l.frames_mut()
        {
            fs.reading_pin = None;
        }
        self.renumber_frames();
        self.mark_dirty();
    }
}

/// Only names still matching the default `Frame N` pattern are renamed —
/// a hand-typed name ("Frame 3 — reaction") is the owner's, and the
/// computed index shows on the badge instead.
pub fn is_default_frame_name(s: &str) -> bool {
    match s.strip_prefix("Frame ") {
        Some(digits) => !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use mn_core::FrameSet;

    /// Owner top item (2026-08-18), HIS exact scenario: dividing a page
    /// numbered the top-RIGHT panel "Frame 2" because numbering was
    /// creation-order. Renumbering is by READING order (RTL right-first),
    /// recomputed on demand; hand-typed names are never clobbered (the
    /// badge carries the number); pins overrule the computed order.
    #[test]
    fn frames_renumber_by_reading_order_not_creation() {
        let Ok(renderer) = mn_gpu::Renderer::new_headless(mn_gpu::GpuConfig {
            force_fallback: std::env::var("MN_WARP").is_ok(),
            no_vsync: false,
        }) else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        let mut app = App::new(renderer, (600, 400), 1.0);
        let (dw, dh) = (app.doc.size.0 as f32, app.doc.size.1 as f32);
        let (half, border) = (dw * 0.5, 2.0);
        // Created LEFT first, RIGHT second — creation order numbers the
        // top-right panel 2; a reader (RTL) starts there.
        let tl = app.doc.add_frame_folder(
            "Frame 1",
            FrameSet::single_rect([0.0, 0.0, half, dh], border),
        );
        let tr = app.doc.add_frame_folder(
            "Frame 2",
            FrameSet::single_rect([half, 0.0, dw, dh], border),
        );
        app.renumber_frames();
        assert_eq!(
            (
                app.doc.layers[tr].name.as_str(),
                app.doc.layers[tl].name.as_str()
            ),
            ("Frame 1", "Frame 2"),
            "RTL: the top-right folder reads first regardless of creation order"
        );
        assert_eq!(app.frame_pos(tr), Some((1, false, false)));
        assert_eq!(app.frame_pos(tl), Some((2, false, false)));

        // A hand-typed name is the owner's: the number moves to the badge.
        app.doc.layers[tr].name = "splash".into();
        app.renumber_frames();
        assert_eq!(app.doc.layers[tr].name, "splash", "never clobbered");
        assert_eq!(app.frame_pos(tr).unwrap().0, 1, "the badge carries it");

        // A pin overrules the computed order and survives recompute.
        app.frame_pin_step(tl, -1); // TL pinned to reading position 1
        assert_eq!(app.frame_pos(tl), Some((1, false, true)));
        assert_eq!(app.frame_pos(tr).unwrap().0, 2);
        app.frame_pin_clear(tl);
        assert_eq!(
            app.frame_pos(tl),
            Some((2, false, false)),
            "back to automatic"
        );
    }

    /// Two side-by-side frame folders with hand-typed names (so renumbering
    /// never renames them and the name stays the handle on a moving index).
    /// Returns (app, a_draw, b_draw) — the folders' draw layers.
    fn two_frame_folders() -> Option<(App, usize, usize)> {
        let Ok(renderer) = mn_gpu::Renderer::new_headless(mn_gpu::GpuConfig {
            force_fallback: std::env::var("MN_WARP").is_ok(),
            no_vsync: false,
        }) else {
            println!("[test] SKIP: no usable adapter");
            return None;
        };
        let mut app = App::new(renderer, (600, 400), 1.0);
        let (dw, dh) = (app.doc.size.0 as f32, app.doc.size.1 as f32);
        let (half, border) = (dw * 0.5, 2.0);
        app.doc
            .add_frame_folder("alpha", FrameSet::single_rect([0.0, 0.0, half, dh], border));
        let a_draw = app.doc.active;
        app.doc
            .add_frame_folder("beta", FrameSet::single_rect([half, 0.0, dw, dh], border));
        let b_draw = app.doc.active;
        app.renumber_frames();
        Some((app, a_draw, b_draw))
    }

    fn header(app: &App, name: &str) -> usize {
        app.doc
            .layers
            .iter()
            .position(|l| l.name == name)
            .expect("folder header")
    }

    /// The cache holds RAW layer indices. `AddLayer` inside a folder inserts
    /// at the folder's own index, so every index at or above it shifts —
    /// and nothing about that command is a "frame command". Before the
    /// revision stamp, the badges kept pointing at whatever slid into those
    /// slots (a plain draw layer), which is how a reading-order number
    /// landed on the wrong panel.
    #[test]
    fn adding_a_layer_does_not_leave_the_reading_order_pointing_at_strangers() {
        let Some((mut app, a_draw, _)) = two_frame_folders() else {
            return;
        };
        let before: Vec<_> = app.frame_order.as_ref().unwrap().panels.clone();
        let (pos_a, pos_b) = (
            app.frame_pos(header(&app, "alpha")),
            app.frame_pos(header(&app, "beta")),
        );
        assert_eq!((pos_a.unwrap().0, pos_b.unwrap().0), (2, 1), "RTL");

        app.doc.set_active(a_draw);
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddLayer);
        app.ensure_frame_order();

        let after = app.frame_order.as_ref().unwrap();
        assert_eq!(after.panels.len(), before.len(), "same panels on the page");
        for pr in &after.panels {
            assert!(
                app.doc.layers[pr.layer].frames().is_some(),
                "panel {pr:?} points at a layer that is not a frame folder"
            );
        }
        assert_eq!(app.frame_pos(header(&app, "alpha")), pos_a);
        assert_eq!(app.frame_pos(header(&app, "beta")), pos_b);
    }

    /// Reordering the stack moves the same panels to different indices.
    /// The order is GEOMETRY's, so a folder keeps its badge — it is the
    /// index inside the cache that has to follow it.
    #[test]
    fn moving_a_folder_in_the_stack_keeps_each_badge_on_its_own_panel() {
        let Some((mut app, _, _)) = two_frame_folders() else {
            return;
        };
        let (pos_a, pos_b) = (
            app.frame_pos(header(&app, "alpha")),
            app.frame_pos(header(&app, "beta")),
        );
        // alpha's whole block over the top of beta's.
        let from = header(&app, "alpha");
        let n = app.doc.layers.len();
        crate::cmd::dispatch(
            &mut app,
            crate::cmd::AppCmd::MoveLayer {
                from,
                slot: n,
                depth: 0,
            },
        );
        app.ensure_frame_order();
        assert!(
            header(&app, "alpha") > header(&app, "beta"),
            "the move happened"
        );
        assert_eq!(app.frame_pos(header(&app, "alpha")), pos_a);
        assert_eq!(app.frame_pos(header(&app, "beta")), pos_b);
    }

    /// A freshly opened document (`forget_document_caches` drops the cache
    /// and nothing recomputed it on demand) showed NO reading order at all
    /// until some frame command ran. The frame-head ensure is that demand.
    #[test]
    fn a_dropped_cache_comes_back_without_a_frame_command() {
        let Some((mut app, _, _)) = two_frame_folders() else {
            return;
        };
        app.frame_order = None;
        assert_eq!(
            app.frame_pos(header(&app, "beta")),
            None,
            "no cache, no badge"
        );
        app.ensure_frame_order();
        assert_eq!(app.frame_pos(header(&app, "beta")).map(|p| p.0), Some(1));
    }
}
