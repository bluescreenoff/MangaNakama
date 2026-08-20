//! Undo/redo: per-op tile snapshots.
//!
//! The model from docs/ARCHITECTURE.md: mutations are bracketed by
//! [`Document::begin_op`](crate::Document::begin_op) /
//! [`Document::end_op`](crate::Document::end_op), and everything the op touched
//! is remembered as its **pre-image** — the `Arc<Tile>` that was there before,
//! or `None` when the tile did not exist yet.
//!
//! Snapshots are cheap because tiles are `Arc`-shared and the write path is
//! `Arc::make_mut`: holding a snapshot costs one refcount until the next write
//! to that tile, which then pays exactly one 32 KiB copy.
//!
//! The recording itself lives in [`Layer`](crate::Layer) (see
//! `Layer::tile_mut`), which is what makes this transparent to the brush crate:
//! a `StrokeSink` just calls `tile_mut` between `begin_op` and `end_op` and
//! becomes undoable without knowing undo exists.

use std::sync::Arc;

use crate::balloon::BalloonSet;
use crate::frame::FrameSet;
use crate::text::TextSet;
use crate::tile::{Tile, TileIdx};

/// One undoable step.
///
/// Both variants carry an index into `Document::layers` at the time the op was
/// recorded. Layer-structure changes (add/remove/duplicate/reorder) invalidate
/// these indices, so `Document` clears the history when one happens. See
/// `Document::add_layer` and friends.
#[derive(Clone, Debug)]
pub enum UndoGroup {
    /// Every tile a single op touched, with its pre-image. `None` in the second
    /// slot means "this tile did not exist before the op" — undoing removes it
    /// again. Sorted by `(y, x)` so groups are deterministic (tiles live in a
    /// HashMap).
    Tiles {
        layer: usize,
        tiles: Vec<(TileIdx, Option<Arc<Tile>>)>,
    },
    /// A frame layer's vector state before the change. No tile snapshots — the
    /// raster is derived, so undo restores the vectors and re-rasterizes.
    Frames { layer: usize, frames: FrameSet },
    /// A balloon layer's vector state before the change; same model as
    /// [`UndoGroup::Frames`].
    Balloons { layer: usize, balloons: BalloonSet },
    /// A text layer's vector state before the change; same model as
    /// [`UndoGroup::Frames`] (the cached sprites ride along in the clone, so
    /// re-rasterizing needs no text engine).
    Texts { layer: usize, texts: TextSet },
    /// A layer's whole MASK field before the change (TRIAGE 138, the
    /// MaskField group — masks are small; the whole-field snapshot follows
    /// the Frames/Texts pattern). `None` = the layer had no mask.
    Mask {
        layer: usize,
        mask: Option<crate::doc::LayerMask>,
    },
    /// A tone layer's screen params before the change. Same model as
    /// [`UndoGroup::Frames`]: the painted source pixels are untouched — undo
    /// restores the params and the derived raster rebuilds on the next
    /// `refresh_derived`.
    Tones {
        layer: usize,
        tone: Option<crate::tone::ToneParams>,
    },
    /// A layer's border-effect params before the change (`LP-002`/`LP-003`).
    /// Same model as [`UndoGroup::Tones`]: the outline is derived, so undo
    /// restores the params and the raster rebuilds on the next
    /// `refresh_derived`.
    Edges {
        layer: usize,
        edge: Option<crate::edge::EdgeParams>,
    },
    /// An effect-line regeneration: the tile pre-images (recorded through
    /// the normal op bracket, exactly like [`UndoGroup::Tiles`]) PLUS the
    /// generator parameters that produced them. The two ride ONE group on
    /// purpose — the raster is rendered from the spec, so restoring the
    /// pixels without restoring the spec leaves the dialog describing art
    /// that is no longer on the layer. Only a layer that already carries a
    /// spec can be regenerated, so this one is never absent.
    GenLines {
        layer: usize,
        spec: crate::genlines::GenLinesSpec,
        tiles: Vec<(TileIdx, Option<Arc<Tile>>)>,
    },
    /// Vector inking (docs/VECTOR-INKING.md): one drawn-and-recorded stroke
    /// — the tile pre-images AND the recorded geometry ride one group, so
    /// undoing the ink also takes back the record (a half-undo would leave
    /// the set describing ink that is gone). `present` = the stroke is in
    /// the layer's set right now: swapping pops it (inverse: absent) or
    /// pushes it back.
    VectorStroke {
        layer: usize,
        tiles: Vec<(TileIdx, Option<Arc<Tile>>)>,
        stroke: Box<crate::stroke_set::VectorStroke>,
        present: bool,
    },
    /// Vector inking phase 2: one EDIT of an existing recorded stroke
    /// (move/deform). The carried stroke is the OTHER version — swapping
    /// exchanges it with `strokes[index]` while the tile pre-images swap
    /// the re-derived pixels, so geometry and ink stay one step.
    VectorEdit {
        layer: usize,
        tiles: Vec<(TileIdx, Option<Arc<Tile>>)>,
        index: usize,
        stroke: Box<crate::stroke_set::VectorStroke>,
    },
    /// PA-001: the paper colour before the change. The only DOCUMENT-level
    /// group — it belongs to no layer, which is why [`UndoGroup::layer`]
    /// returns an `Option`. The paper's EYE is not in here on purpose: it is
    /// view state like a layer's eye, and neither is undoable.
    Paper { colour: [u8; 3] },
    /// The whole ruler set before the change — document-level like
    /// [`UndoGroup::Paper`]. Rulers are tiny (a handful of anchors), so the
    /// whole-set snapshot follows the Frames/Texts idiom rather than
    /// modelling per-ruler edits. The snap SWITCHES ride along in the
    /// snapshot because they are part of the value; the gestures that
    /// record one (create / move / clear) all set them as a side effect,
    /// and a bare toggle records nothing — it is view state, like a
    /// layer's eye.
    Rulers { rulers: crate::ruler::Rulers },
}

impl UndoGroup {
    pub fn tile_count(&self) -> usize {
        match self {
            UndoGroup::Tiles { tiles, .. }
            | UndoGroup::GenLines { tiles, .. }
            | UndoGroup::VectorStroke { tiles, .. }
            | UndoGroup::VectorEdit { tiles, .. } => tiles.len(),
            UndoGroup::Frames { .. }
            | UndoGroup::Balloons { .. }
            | UndoGroup::Texts { .. }
            | UndoGroup::Mask { .. }
            | UndoGroup::Tones { .. }
            | UndoGroup::Edges { .. }
            | UndoGroup::Paper { .. }
            | UndoGroup::Rulers { .. } => 0,
        }
    }

    /// The layer this group's pre-image belongs to. `None` for the
    /// document-level groups (PA-001's paper colour), which no layer owns —
    /// `drop_layer_history` must therefore never drop them.
    pub fn layer(&self) -> Option<usize> {
        match self {
            UndoGroup::Tiles { layer, .. }
            | UndoGroup::Frames { layer, .. }
            | UndoGroup::Balloons { layer, .. }
            | UndoGroup::Texts { layer, .. }
            | UndoGroup::Mask { layer, .. }
            | UndoGroup::Tones { layer, .. }
            | UndoGroup::Edges { layer, .. }
            | UndoGroup::GenLines { layer, .. }
            | UndoGroup::VectorStroke { layer, .. }
            | UndoGroup::VectorEdit { layer, .. } => Some(*layer),
            UndoGroup::Paper { .. } | UndoGroup::Rulers { .. } => None,
        }
    }
}

/// How many groups the undo stack keeps before dropping the oldest — the
/// DEFAULT for [`History::limit`], and the shipped value of the `undo_depth`
/// preference. Generous on purpose: a group is a handful of `Arc`s, not
/// pixels — a snapshot only becomes a real 32 KiB copy when a later op
/// rewrites the tile. (Owner 2026-08-14: 200 felt shallow; the deeper fix is
/// making structural layer ops undoable instead of history-clearing.)
pub const UNDO_LIMIT: usize = 400;

/// Undo + redo stacks. Owned by `Document`; you drive it through
/// `Document::undo` / `Document::redo`.
#[derive(Clone, Debug)]
pub struct History {
    undo: Vec<UndoGroup>,
    redo: Vec<UndoGroup>,
    /// CV-003: one label per undo entry ("Stroke", "Fill", …), kept in
    /// lockstep with `undo`; the redo side carries the same labels.
    undo_labels: Vec<String>,
    redo_labels: Vec<String>,
    /// Groups kept before the oldest is dropped. [`UNDO_LIMIT`] unless the
    /// `undo_depth` preference says otherwise.
    limit: usize,
    /// PR-041: how many undoable operations this document has performed,
    /// ever. Monotonic — undo counts as an operation, and `clear()` does
    /// not reset it.
    ///
    /// It exists because the "save recovery data for every operation"
    /// preference needs an edge to fire on, and the two obvious candidates
    /// are both wrong. [`History::undo_len`] STOPS MOVING once the depth
    /// cap starts dropping the oldest group, so a long session would
    /// silently stop autosaving — the exact silent failure the feature is
    /// there to prevent. `Document::revision` moves for presentation-only
    /// changes too, so dragging the opacity slider would write a whole
    /// recovery file per frame.
    ops: u64,
}

impl Default for History {
    fn default() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            undo_labels: Vec::new(),
            redo_labels: Vec::new(),
            limit: UNDO_LIMIT,
            ops: 0,
        }
    }
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// How deep this document's history goes.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Set the depth (the `undo_depth` preference). Trims immediately, so
    /// lowering it frees the memory now rather than on the next stroke.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(1);
        self.trim();
    }

    /// Drop the oldest groups down to `limit`, labels in lockstep.
    fn trim(&mut self) {
        if self.undo.len() > self.limit {
            let overflow = self.undo.len() - self.limit;
            self.undo.drain(..overflow);
            self.undo_labels.drain(..overflow);
        }
    }

    /// Push a freshly recorded group. Clears the redo stack (new branch) and
    /// enforces [`History::limit`]. Unlabeled ("Edit").
    pub fn push(&mut self, group: UndoGroup) {
        self.push_labeled("Edit", group);
    }

    /// [`History::push`] with a History-palette label (CV-003).
    pub fn push_labeled(&mut self, label: &str, group: UndoGroup) {
        self.push_labeled_inner(label.to_string(), group, true);
    }

    fn push_labeled_inner(&mut self, label: String, group: UndoGroup, clear_redo: bool) {
        if clear_redo {
            self.redo.clear();
            self.redo_labels.clear();
        }
        self.undo.push(group);
        self.undo_labels.push(label);
        self.ops += 1;
        self.trim();
    }

    /// PR-041: operations performed, ever. See the `ops` field.
    pub fn ops(&self) -> u64 {
        self.ops
    }

    /// PR-041: count an operation that pushes no group — the structural
    /// layer ops, which clear the history instead. Called by
    /// `Document::clear_history`, the one place all of them meet.
    pub fn note_op(&mut self) {
        self.ops += 1;
    }

    pub fn pop_undo(&mut self) -> Option<UndoGroup> {
        self.undo_labels.pop();
        self.undo.pop()
    }

    /// Pop the newest undo group WITH its label (the undo/redo loop keeps
    /// the label on both sides).
    pub fn pop_undo_labeled(&mut self) -> Option<(String, UndoGroup)> {
        let label = self.undo_labels.pop()?;
        Some((label, self.undo.pop()?))
    }

    /// Drop every group belonging to `layer`, labels in lockstep. The tool
    /// for an op that swaps a layer's raster WHOLESALE, past the
    /// copy-on-write recording: older pre-images would splice stale ink
    /// into the new raster when undone. `regen_genlines` used to be such
    /// an op; it now writes through the tile APIs inside the op bracket
    /// and keeps its history, so nothing in the shipped paths calls this
    /// — a new wholesale swap must call it or become undoable instead.
    pub fn drop_layer_history(&mut self, layer: usize) {
        let rebuild = |groups: &mut Vec<UndoGroup>, labels: &mut Vec<String>| {
            let mut kg = Vec::with_capacity(groups.len());
            let mut kl = Vec::with_capacity(labels.len());
            for (g, l) in groups.drain(..).zip(labels.drain(..)) {
                if g.layer() != Some(layer) {
                    kg.push(g);
                    kl.push(l);
                }
            }
            *groups = kg;
            *labels = kl;
        };
        rebuild(&mut self.undo, &mut self.undo_labels);
        rebuild(&mut self.redo, &mut self.redo_labels);
    }

    pub fn pop_redo(&mut self) -> Option<UndoGroup> {
        self.redo_labels.pop();
        self.redo.pop()
    }

    /// Pop the newest redo group WITH its label.
    pub fn pop_redo_labeled(&mut self) -> Option<(String, UndoGroup)> {
        let label = self.redo_labels.pop()?;
        Some((label, self.redo.pop()?))
    }

    /// Push the inverse of an undone group onto the redo stack.
    pub fn push_redo(&mut self, group: UndoGroup) {
        self.push_redo_labeled("Edit", group);
    }

    /// [`History::push_redo`] with the original step's label.
    pub fn push_redo_labeled(&mut self, label: &str, group: UndoGroup) {
        self.redo.push(group);
        self.redo_labels.push(label.to_string());
        // PR-041: this is `undo()`'s half of the swap, and an undo is an
        // operation whose result deserves a recovery file like any other.
        self.ops += 1;
    }

    /// Push the inverse of a redone group back onto the undo stack **without**
    /// clearing redo (that would eat the rest of the redo chain).
    pub fn push_undo_keep_redo(&mut self, group: UndoGroup) {
        self.push_undo_keep_redo_labeled("Edit", group);
    }

    /// [`History::push_undo_keep_redo`] with the original step's label.
    pub fn push_undo_keep_redo_labeled(&mut self, label: &str, group: UndoGroup) {
        self.undo.push(group);
        self.undo_labels.push(label.to_string());
        // PR-041: `redo()`'s half of the same swap.
        self.ops += 1;
        self.trim();
    }

    /// CV-003: the undo stack's labels, oldest first — the History
    /// palette's past.
    pub fn undo_labels(&self) -> &[String] {
        &self.undo_labels
    }

    /// CV-003: the redo branch's labels in CHRONOLOGICAL order (the stack
    /// itself is newest-first for popping) — the palette's greyed future.
    pub fn redo_labels(&self) -> Vec<String> {
        self.redo_labels.iter().rev().cloned().collect()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.undo_labels.clear();
        self.redo_labels.clear();
    }
}
