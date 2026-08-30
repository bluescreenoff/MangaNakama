//! Leak-repair refill — the owner's own idea (2026-08-29): a fill leaked
//! through a gap he forgot to close → ONE command arms repair → he draws
//! the gap-closing stroke → the fill re-runs itself from its remembered
//! seed and settings, replacing the leaked result, all of it ONE undo
//! press. Today that recovery is six actions; this makes it two, and the
//! re-aim disappears because the seed is stored.
//!
//! # The two barrier kinds
//!
//! * **Virtual** (`virtual_barrier: true`): the stroke is captured as
//!   points and rastered into a wall mask only the fill's gather step
//!   ever sees ([`mn_core::fill::barrier_mask`]) — composited nowhere,
//!   saved nowhere, lifetime this one repair. Deliberately open
//!   linework is exactly where global gap-close cannot be raised.
//! * **Real ink** (`virtual_barrier: false`): the stroke is the user's
//!   OWN pen stroke, on whatever layer they have in hand — the fill
//!   re-runs after it and its source sampling sees the ink wherever the
//!   fill's `refer` reads it. The stroke stays (that is the point); the
//!   stroke + refill land as ONE undo press via `wrap_recent`.
//!
//! Two palette rows pick the kind (no hidden modifier to remember);
//! recorded in DECISIONS.
//!
//! # Undo shape
//!
//! Arming UNDOES the leaked fill immediately (it validated first that
//! the fill is still the newest history step, so the undo takes back
//! exactly the fill). After that: virtual = one new fill op; real ink =
//! stroke op + fill op wrapped into one. Either way a single Ctrl+Z
//! returns the pre-leak state.
//!
//! # Standing down
//!
//! Esc cancels; a page or tab hop disarms (the refill validates page and
//! layer again at run time too — an armed gesture never survives into
//! the wrong page's pixels).

use crate::App;
use mn_core::fill::FillOpts;

/// The last seeded click-fill, remembered at its commit point. Lasso and
/// enclose gestures have no seed and never set this.
#[derive(Clone, Debug)]
pub(crate) struct LeakFill {
    /// STABLE layer id (indices move; ids do not).
    pub layer_id: u64,
    /// The page the fill landed on — a hop to another page disarms.
    pub page_uid: u64,
    /// The click, in doc pixels.
    pub seed: (f32, f32),
    pub color: [f32; 3],
    pub opts: FillOpts,
    /// The undo label the fill pushed ("Fill") — the arm's newest-step
    /// check compares against it.
    pub op_label: &'static str,
    /// `Document::undo_len` right after the fill pushed — the label
    /// alone is not identity ("Fill" is also Alt+Del's selection fill),
    /// and undoing THAT would destroy it, not the leak. The stack is
    /// uncapped, so a later op can only return to this depth by undoing
    /// back to this very step.
    pub undo_len: usize,
}

/// The armed repair gesture.
pub(crate) struct FillRepair {
    pub leak: LeakFill,
    pub virtual_barrier: bool,
    /// The captured stroke (doc pixels) — virtual mode only, filled by
    /// the canvas arms as the stroke streams.
    pub pts: Vec<(f32, f32)>,
    /// Brush radius at stroke start — the barrier's thickness, so the
    /// wall matches the stroke the user thinks they drew.
    pub radius: f32,
    /// `Document::undo_len` right after the arm's undo — how many ops
    /// the closing stroke actually pushed is measured against it, so the
    /// real-ink wrap never swallows earlier work when a stroke changed
    /// nothing (the row-95 discipline: an all-paper stroke pushes NO
    /// step).
    pub armed_undo_len: usize,
}

impl App {
    /// The palette command. Refuses with a status line (never a dialog)
    /// when there is nothing safely repairable; otherwise undoes the
    /// leaked fill and waits for the stroke.
    /// Could a repair arm RIGHT NOW? `Ok` is the leaked layer's index;
    /// `Err` is the refusal, worded for a status line and for the Tool
    /// Property button's disabled hover. Side-effect free — the arm and
    /// the UI both ask HERE, so the button can never grey for a reason
    /// the command would not refuse.
    pub(crate) fn fill_repairable(&self) -> Result<usize, &'static str> {
        let Some(f) = &self.last_fill else {
            return Err("no fill to repair yet — click a fill first");
        };
        let page_uid = self.pages.get(self.page_index).map(|p| p.uid).unwrap_or(0);
        if page_uid != f.page_uid {
            return Err("the remembered fill is on another page");
        }
        let Some(li) = self.doc.layer_index_of(f.layer_id) else {
            return Err("the filled layer is gone");
        };
        if !self.doc.layers[li].paintable() {
            return Err("the filled layer no longer takes pixels (folder or derived)");
        }
        // Label AND depth: "Fill" is also Alt+Del's selection-fill label,
        // and undoing one of those here would silently destroy it. The
        // uncapped stack makes the depth an identity check — only undoing
        // back to this very step can match both.
        if self.doc.peek_undo_label() != Some(f.op_label) || self.doc.undo_len() != f.undo_len {
            return Err(
                "the fill is no longer the newest undo step — undo your later work, or fill again",
            );
        }
        Ok(li)
    }

    pub(crate) fn arm_fill_repair(&mut self, virtual_barrier: bool) {
        if let Some(r) = &mut self.fill_repair {
            // Re-arming swaps the mode; the undo already happened and
            // must not happen twice.
            r.virtual_barrier = virtual_barrier;
            r.pts.clear();
            let kind = if virtual_barrier { "virtual" } else { "real-ink" };
            self.set_status(format!("repair re-armed: {kind} barrier — draw the closing stroke"));
            return;
        }
        let li = match self.fill_repairable() {
            Ok(li) => li,
            Err(e) => {
                self.set_status(e);
                return;
            }
        };
        let f = self.last_fill.clone().expect("fill_repairable checked it");
        if !self.doc.undo() {
            self.set_status("nothing to undo — the fill already went");
            return;
        }
        // The refill re-runs ON the leaked layer — aim the doc there so
        // the fill writes where the original did, whatever the user had
        // active meanwhile.
        self.doc.set_active(li);
        self.fill_repair = Some(FillRepair {
            leak: f,
            virtual_barrier,
            pts: Vec::new(),
            radius: 1.0,
            armed_undo_len: self.doc.undo_len(),
        });
        self.set_status(if virtual_barrier {
            "fill repair armed — draw the closing stroke; the fill re-runs on release \
             (Esc cancels)"
        } else {
            "fill repair armed — draw the closing stroke as real ink where the fill reads \
             it; release re-runs the fill (Esc cancels)"
        });
        self.renderer.invalidate();
        self.needs_redraw = true;
    }

    /// Esc / page hop: stand down without refilling. The leaked fill was
    /// already undone at arm time — standing down leaves it undone (the
    /// user saw it go; refilling behind their back is the one thing this
    /// feature must never do).
    pub(crate) fn cancel_fill_repair(&mut self) {
        if self.fill_repair.take().is_some() {
            self.set_status("fill repair cancelled — the leaked fill stays undone");
            self.needs_redraw = true;
        }
    }

    /// The stroke is done: re-run the fill from the remembered seed and
    /// settings, with the virtual barrier if there is one, and land the
    /// whole tail as one undo press (real-ink mode: the stroke op and
    /// the refill wrap together).
    pub(crate) fn finish_fill_repair(&mut self, mut repair: FillRepair) {
        let f = repair.leak.clone();
        let page_uid = self.pages.get(self.page_index).map(|p| p.uid).unwrap_or(0);
        let Some(li) = self.doc.layer_index_of(f.layer_id) else {
            self.set_status("the filled layer is gone — repair stood down");
            return;
        };
        if page_uid != f.page_uid {
            self.set_status("the page moved — repair stood down");
            return;
        }
        self.refresh_tones();
        self.doc.set_active(li);
        // How many ops the closing stroke pushed since the arm — an
        // all-paper stroke pushes NONE (the row-95 discipline), and the
        // real-ink wrap below must never reach past the arm point and
        // swallow earlier work.
        let stroke_ops = self.doc.undo_len().saturating_sub(repair.armed_undo_len);
        let seed = (f.seed.0 as i32, f.seed.1 as i32);
        let (n, _auto) = if repair.virtual_barrier {
            let mask = if repair.pts.len() >= 2 {
                mn_core::fill::barrier_mask(
                    self.doc.size.0,
                    self.doc.size.1,
                    &repair.pts,
                    repair.radius,
                )
            } else {
                Vec::new()
            };
            mn_core::fill::bucket_fill_measured_with(
                &mut self.doc,
                seed,
                f.color,
                &f.opts,
                &mask,
            )
        } else {
            mn_core::fill::bucket_fill_measured(&mut self.doc, seed, f.color, &f.opts)
        };
        if n == 0 {
            self.set_status("repair filled nothing — the barrier may have sealed the seed");
            self.mark_dirty();
            return;
        }
        if !repair.virtual_barrier {
            // The user's stroke op + the refill: one press takes both
            // back to the pre-leak state (the leak itself was undone at
            // arm time). Only when the stroke actually pushed one —
            // wrapping past the arm point would fold earlier work into
            // "Repair fill".
            if stroke_ops >= 1 {
                self.doc.wrap_recent("Repair fill", 2);
            }
            self.set_status("fill repaired — ink kept; one undo takes it all back");
        } else {
            self.set_status("fill repaired (virtual barrier) — one undo takes it all back");
        }
        repair.pts.clear();
        self.renderer.invalidate();
        self.mark_dirty();
        self.needs_redraw = true;
    }
}
