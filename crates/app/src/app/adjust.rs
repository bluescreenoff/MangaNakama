//! Tonal correction, app side (TC-004/005/006/007/011): the live preview's
//! lifecycle around `mn_core::Adjust`.
//!
//! The preview is the real thing — the correction is painted into the
//! document's own pixels so the canvas shows exactly what Apply will bake,
//! at full resolution, through the real compositor. It is written OUTSIDE
//! the undo bracket, which buys the fidelity and costs a rule:
//!
//! **while a preview is live, nothing else may look at the document.**
//!
//! Two doors enforce that. `cmd::dispatch` reverts the preview before any
//! command that is not one of this feature's own (so a save, an undo, a
//! layer switch or a doc tab can never see previewed pixels), and
//! `App::begin_stroke` refuses to paint. Both routes end at
//! [`App::adjust_preview_revert`], which restores bit-for-bit.

use std::sync::Arc;

use mn_core::{Adjust, Tile, TileIdx};

use crate::app::App;

/// The pixels a live preview overwrote, and where they came from.
pub struct AdjustPreview {
    /// Per-target restore points: each selected layer with its pre-image
    /// tiles (Arc handles — no pixels were copied). The set was taken from
    /// the palette selection when the dialog opened; nothing may change
    /// that selection while a preview is live (see the module note), so
    /// these are also the layers Apply commits to.
    pub targets: Vec<(usize, Vec<(TileIdx, Arc<Tile>)>)>,
    /// The dialog's Preview checkbox. Off puts the untouched layers back on
    /// screen without closing the dialog — the "before" of before/after,
    /// which is the only way to judge a binarization threshold.
    pub live: bool,
    /// What is actually painted into the document right now; `None` = the
    /// original pixels. A UI frame that changed nothing costs nothing.
    pub painted: Option<Adjust>,
}

impl App {
    /// Open a correction dialog and take the preview's restore points —
    /// one per selected layer that has pixels in reach (TC-013).
    pub fn adjust_begin(&mut self, adj: Adjust) {
        self.adjust_preview_revert();
        let targets: Vec<_> = self
            .doc
            .multi_targets()
            .into_iter()
            .filter_map(|li| {
                let tiles = self.doc.adjust_snapshot(li);
                (!tiles.is_empty()).then_some((li, tiles))
            })
            .collect();
        if targets.is_empty() {
            self.set_status("nothing to correct — no selected layer has pixels in reach");
            return;
        }
        self.adjust_draft = Some(adj);
        self.adjust_preview = Some(AdjustPreview {
            targets,
            live: true,
            painted: None,
        });
        self.mark_dirty();
    }

    /// Bring the canvas in line with the draft parameters. Called once per
    /// UI frame while the dialog is open; a no-op when nothing moved.
    pub fn adjust_preview_sync(&mut self) {
        let Some(adj) = self.adjust_draft else {
            return;
        };
        let Some(p) = self.adjust_preview.as_ref() else {
            return;
        };
        // Preview off, or every slider at rest, both mean "show the layer
        // as it is" — and an identity correction is not worth a pass.
        let want = if p.live && !adj.is_identity() {
            Some(adj)
        } else {
            None
        };
        if p.painted == want {
            return;
        }
        let targets = p.targets.clone();
        for (layer, tiles) in &targets {
            self.doc.preview_adjust(*layer, tiles, want.as_ref());
        }
        if let Some(p) = self.adjust_preview.as_mut() {
            p.painted = want;
        }
        self.mark_dirty();
    }

    /// Put the previewed pixels back and close the dialog. True when there
    /// was a preview to undo — callers use that to report "abandoned".
    pub fn adjust_preview_revert(&mut self) -> bool {
        self.adjust_draft = None;
        let Some(p) = self.adjust_preview.take() else {
            return false;
        };
        if p.painted.is_some() {
            for (layer, tiles) in &p.targets {
                self.doc.preview_adjust(*layer, tiles, None);
            }
            self.mark_dirty();
        }
        true
    }

    /// Bake the draft correction as one undo step.
    ///
    /// The revert first is mandatory, not tidiness: the undo group snapshots
    /// whatever the tiles hold when the op opens, and that is currently the
    /// preview. Committing on top of it would make Undo restore the
    /// *previewed* image instead of the original.
    pub fn adjust_commit(&mut self) {
        let Some(adj) = self.adjust_draft else {
            return;
        };
        // The layers the preview snapshotted are the layers to commit to —
        // the palette selection cannot have moved while the dialog was open
        // (dispatch's head guard), but taking them from the preview makes
        // the commit correct even if that ever stopped being true.
        let layers: Vec<usize> = self
            .adjust_preview
            .as_ref()
            .map(|p| p.targets.iter().map(|(li, _)| *li).collect())
            .unwrap_or_default();
        self.adjust_preview_revert();
        match self.doc.apply_adjust_many(&adj, &layers) {
            0 if adj.is_identity() => {
                self.set_status("nothing to apply — every slider is at rest");
            }
            0 => {
                self.set_status("nothing to correct (needs an unlocked raster layer with pixels)");
            }
            1 => {
                self.set_status(format!("{} applied", adj.label()));
                self.mark_dirty();
            }
            n => {
                self.set_status(format!("{} applied to {n} layers", adj.label()));
                self.mark_dirty();
            }
        }
    }
}

/// The app-side half of a correction is entirely about the preview writing
/// real pixels outside the undo bracket. These tests are the fence around
/// that: commit from the ORIGINAL, and nothing else gets to look.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PointerKind;
    use crate::cmd::{AppCmd, dispatch};
    use mn_core::TILE_SIZE;

    /// `app.rs` keeps the same four lines in `new_document_tests::headless`,
    /// private to that module. No usable adapter = skip, per the GPU-test
    /// rule in docs/ARCHITECTURE.md.
    fn headless() -> Option<App> {
        let renderer = mn_gpu::Renderer::new_headless(mn_gpu::GpuConfig {
            force_fallback: std::env::var("MN_WARP").is_ok(),
            no_vsync: false,
        })
        .ok()?;
        Some(App::new(renderer, (1280, 860), 1.0))
    }

    /// Opaque straight grey at (10,10) on the active layer.
    fn seed(app: &mut App, v: f32) {
        let li = app.doc.active;
        let idx = TileIdx::new(0, 0);
        let p = (10 * TILE_SIZE + 10) * 4;
        let d = app.doc.layers[li].tile_mut(idx).data_mut();
        for c in 0..3 {
            d[p + c] = mn_core::blend::f32_to_fix15(v);
        }
        d[p + 3] = mn_core::blend::f32_to_fix15(1.0);
    }

    fn read(app: &App, li: usize) -> f32 {
        let p = (10 * TILE_SIZE + 10) * 4;
        app.doc.layers[li]
            .tile_arc(TileIdx::new(0, 0))
            .map(|t| t.data()[p] as f32 / 32768.0)
            .unwrap_or(0.0)
    }

    #[test]
    fn apply_commits_from_the_original_not_from_the_preview() {
        // THE trap this whole module exists for. The preview has already
        // written the inverted pixels; if the commit ran on top of them the
        // layer would come back to 0.6 (inverted twice) and Undo would
        // "restore" the preview.
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        let li = app.doc.active;
        seed(&mut app, 0.6);
        // The fixture speaks for itself before the feature is asked to: a
        // seed that did not land would make every assertion below vacuous.
        assert!(
            (read(&app, li) - 0.6).abs() < 0.002,
            "the seed landed: {} (layer {}, {} tiles)",
            read(&app, li),
            li,
            app.doc.layers[li].tile_count()
        );
        app.adjust_begin(Adjust::Invert);
        app.adjust_preview_sync();
        assert!(
            (read(&app, li) - 0.4).abs() < 0.002,
            "preview is live: {} (draft {:?})",
            read(&app, li),
            app.adjust_draft
        );
        app.adjust_commit();
        assert!(
            (read(&app, li) - 0.4).abs() < 0.002,
            "applied ONCE, not twice: {}",
            read(&app, li)
        );
        assert_eq!(app.doc.undo_labels().len(), 1, "exactly one undo step");
        assert!(app.doc.undo());
        assert!(
            (read(&app, li) - 0.6).abs() < 0.002,
            "undo restores the ORIGINAL: {}",
            read(&app, li)
        );
        assert!(app.adjust_draft.is_none() && app.adjust_preview.is_none());
    }

    #[test]
    fn any_other_command_reverts_the_preview() {
        // A save, an undo or a layer switch must never see previewed pixels.
        // `dispatch`'s head guard is what stops that; this is its fence.
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        let li = app.doc.active;
        seed(&mut app, 0.6);
        app.adjust_begin(Adjust::Invert);
        app.adjust_preview_sync();
        assert!(
            (read(&app, li) - 0.4).abs() < 0.002,
            "the preview painted the inverted pixels: {}",
            read(&app, li)
        );
        dispatch(&mut app, AppCmd::Zoom100);
        assert!(
            (read(&app, li) - 0.6).abs() < 0.002,
            "an unrelated command left previewed pixels behind: {}",
            read(&app, li)
        );
        assert!(app.adjust_draft.is_none() && app.adjust_preview.is_none());
        assert!(app.doc.undo_labels().is_empty(), "a preview is not history");
    }

    #[test]
    fn a_stroke_is_refused_while_a_preview_is_live() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        seed(&mut app, 0.6);
        app.adjust_begin(Adjust::Invert);
        app.begin_stroke(PointerKind::Mouse);
        assert!(app.stroke.is_none(), "the stroke must not have opened");
        assert!(app.adjust_preview.is_some(), "and the dialog stays open");
    }

    #[test]
    fn multi_selection_previews_and_commits_every_selected_layer() {
        // TC-013: two layers in the palette selection — the preview paints
        // both, Apply bakes both as ONE undo step, undo restores both.
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        seed(&mut app, 0.6); // layer 0
        dispatch(&mut app, AppCmd::AddLayer);
        seed(&mut app, 0.2); // layer 1, active after AddLayer
        assert!(app.doc.toggle_multi(0), "layer 0 joins the selection");
        assert_eq!(app.doc.multi_targets(), vec![0, 1]);
        app.adjust_begin(Adjust::Invert);
        app.adjust_preview_sync();
        assert!((read(&app, 0) - 0.4).abs() < 0.002, "preview on layer 0");
        assert!((read(&app, 1) - 0.8).abs() < 0.002, "preview on layer 1");
        app.adjust_commit();
        assert!((read(&app, 0) - 0.4).abs() < 0.002, "applied once to 0");
        assert!((read(&app, 1) - 0.8).abs() < 0.002, "applied once to 1");
        assert_eq!(app.doc.undo_labels().len(), 1, "ONE step for the set");
        assert!(app.doc.undo());
        assert!((read(&app, 0) - 0.6).abs() < 0.002, "undo restores 0");
        assert!((read(&app, 1) - 0.2).abs() < 0.002, "undo restores 1");
    }

    #[test]
    fn the_preview_checkbox_shows_the_before() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        let li = app.doc.active;
        seed(&mut app, 0.6);
        app.adjust_begin(Adjust::Invert);
        app.adjust_preview_sync();
        assert!(
            (read(&app, li) - 0.4).abs() < 0.002,
            "the preview painted the inverted pixels: {}",
            read(&app, li)
        );
        app.adjust_preview.as_mut().unwrap().live = false;
        app.adjust_preview_sync();
        assert!((read(&app, li) - 0.6).abs() < 0.002, "before");
        app.adjust_preview.as_mut().unwrap().live = true;
        app.adjust_preview_sync();
        assert!((read(&app, li) - 0.4).abs() < 0.002, "after");
    }
}
