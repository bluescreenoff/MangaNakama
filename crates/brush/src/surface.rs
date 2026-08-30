//! The tile store libmypaint paints into: `core::Document` tiles, unconverted.
//!
//! # Why there is no format conversion
//!
//! libmypaint's native tile is 64x64 RGBA `uint16_t`, premultiplied, fix15
//! (`1.0 == 1<<15`). `core::Tile` is pinned to exactly that
//! (`docs/ARCHITECTURE.md`), so `tile_request_start` hands the C code a raw
//! pointer straight into the layer's tile buffer and the dab loop writes final
//! pixels in place. `csrc/mn_surface.c` static-asserts `MYPAINT_TILE_SIZE == 64`
//! so a future config change cannot make this silently wrong.
//!
//! # Aliasing story (read before touching this)
//!
//! Two raw pointers cross the boundary and both have narrow validity windows:
//!
//! 1. **The document.** `SurfaceState::doc` is set from the `&mut Document`
//!    passed to `StrokeSink::sample`, held only for the duration of one
//!    `stroke_to` + `end_atomic` batch, then cleared. The `&mut` it came from is
//!    *not touched* while it is set, so the raw pointer keeps that borrow alive
//!    (the "derive a raw pointer from `&mut`, don't use the original until the
//!    raw one is dead" pattern). Callbacks reconstruct a `&mut Document` from
//!    it; there is never a second live one. `bind`/`unbind` are the only places
//!    that field changes, and `unbind` runs even if the batch painted nothing.
//!
//! 2. **The tile buffer.** `tile_request_start` returns a pointer that must stay
//!    valid until the matching `tile_request_end`. libmypaint checks out
//!    **at most one tile at a time** on this path: `threadsafe_tile_requests` is
//!    `FALSE` (set by `mypaint_tiled_surface_init`) and we never compile with
//!    `-fopenmp`, so `process_tile_internal` and `get_color_internal` are plain
//!    sequential loops of start/…/end. The buffer itself is a `Box<[u16]>`
//!    behind an `Arc<Tile>` in a `HashMap`; growing that map moves the `Arc`,
//!    not the heap buffer, and `Arc::make_mut` can only reallocate the tile it
//!    was called on — so nothing invalidates a checked-out pointer.
//!
//! Single-threaded use is a contract with the app crate. Nothing here is `Send`
//! or `Sync`, so the compiler holds us to it.

use core::ffi::{c_int, c_void};

use std::cell::RefCell;
use std::sync::Arc;

use mn_core::{Document, TILE_LEN, Tile, TileIdx};

use crate::ffi;

/// The GPU tile oracle (#0.1 part 3, smudge): while a GPU dab stroke is
/// live, the C's canvas sampler (`get_color`, fired per dab by the smudge
/// engine) must see the freshest canvas — CPU tile seed ⊕ every dispatched
/// GPU dab — which lives in the renderer's tile cache, not the CPU tiles
/// (BYPASS never touches those). The app installs this for the stroke's
/// duration; `tile_request_start(readonly)` consults it first and falls
/// back to the CPU tile copy when it declines (no GPU tile for that idx =
/// the stroke never touched it = the CPU tile IS current).
///
/// Same validity-window discipline as the record hook: single-threaded
/// engine, one stroke at a time, cleared at stroke end before the
/// renderer could move.
pub type TileOracle = fn(ctx: *mut c_void, tx: c_int, ty: c_int, dest: &mut [u16]) -> bool;

thread_local! {
    static TILE_ORACLE: RefCell<Option<(TileOracle, *mut c_void)>> =
        const { RefCell::new(None) };
}

/// Install/clear the GPU tile oracle (app-side; `None` between strokes).
pub fn set_tile_oracle(oracle: Option<(TileOracle, *mut c_void)>) {
    TILE_ORACLE.with(|c| *c.borrow_mut() = oracle);
}

/// Rust half of `MnSurface`. Its address is handed to C once and never moves
/// (it lives behind `Box::into_raw`).
pub(crate) struct SurfaceState {
    /// Valid only between `bind` and `unbind`; null otherwise.
    doc: *mut Document,
    /// LM-004: when set, tile requests route to the ACTIVE layer's MASK
    /// (coverage tiles) instead of its pixels — any brush edits the mask,
    /// alpha is the payload (colour ignored; soft brush ⇒ soft mask).
    mask_mode: bool,
    /// The selection-paint target (SE round 2026-08-19): tile traffic
    /// routes to the DOCUMENT's selection scratch — the brush paints
    /// selection coverage with full engine fidelity (soft brush ⇒ soft
    /// selection). Mutually exclusive with `mask_mode` by construction
    /// (the app sets exactly one per stroke).
    sel_mode: bool,
    /// The SMUDGE-UNDER-WASH read base (TODO #6): when a wash stroke runs on
    /// a smudge brush, the sampler must see layer ⊕ buffer (the ink the user
    /// sees), not the blank buffer alone. This is the DOCUMENT under the
    /// wash buffer; null on every non-wash-smudge path.
    composite_base: *mut Document,
    /// Handed out for read-only requests and for anything off-canvas, so
    /// libmypaint always gets a writable 64x64 tile even where we store none.
    /// Writes to it are discarded by design.
    scratch: Box<[u16]>,
    /// Row 42: the anti-overflow barrier for this stroke (None = paint
    /// freely, the behaviour of every stroke before the switch existed).
    anti: Option<std::sync::Arc<crate::AntiOverflowMask>>,
    /// The write-tile snapshot belonging to the barrier — taken at the
    /// writable request START, replayed over blocked pixels at END.
    anti_snap: Option<(TileIdx, Vec<u16>, std::sync::Arc<crate::AntiOverflowMask>)>,
}

/// A `MyPaintTiledSurface` subclass backed by a `core::Document`'s active layer.
pub(crate) struct TileSurface {
    raw: *mut ffi::MnSurface,
    /// `Box::into_raw`. Accessed *only* through this pointer so there is exactly
    /// one provenance for the memory C also points at.
    state: *mut SurfaceState,
}

impl TileSurface {
    pub(crate) fn new() -> TileSurface {
        let state = Box::into_raw(Box::new(SurfaceState {
            doc: std::ptr::null_mut(),
            mask_mode: false,
            sel_mode: false,
            composite_base: std::ptr::null_mut(),
            scratch: vec![0u16; TILE_LEN].into_boxed_slice(),
            anti: None,
            anti_snap: None,
        }));
        let raw = unsafe { ffi::mn_surface_new(state as *mut c_void) };
        assert!(!raw.is_null(), "mn_surface_new: out of memory");
        TileSurface { raw, state }
    }

    /// The `MyPaintSurface*` to pass to `mypaint_brush_stroke_to`.
    pub(crate) fn interface(&self) -> *mut ffi::MyPaintSurface {
        unsafe { ffi::mn_surface_interface(self.raw) }
    }

    /// Point the surface at a document for one batch.
    ///
    /// # Safety
    /// `doc` must stay valid and untouched by Rust until [`Self::unbind`].
    pub(crate) unsafe fn bind(&self, doc: *mut Document) {
        unsafe { (*self.state).doc = doc };
    }

    /// LM-004: route this batch's tile traffic to the active layer's mask.
    pub(crate) unsafe fn set_mask_mode(&self, on: bool) {
        unsafe { (*self.state).mask_mode = on };
    }

    /// Route this batch's tile traffic to the document's selection
    /// scratch (selection pen / eraser / Quick Mask).
    pub(crate) unsafe fn set_sel_mode(&self, on: bool) {
        unsafe { (*self.state).sel_mode = on };
    }

    /// Row 42: arm the anti-overflow barrier for this batch — writable
    /// tile requests snapshot, and their END restores every blocked pixel.
    pub(crate) unsafe fn set_anti_overflow(
        &self,
        m: Option<std::sync::Arc<crate::AntiOverflowMask>>,
    ) {
        let st = unsafe { &mut *self.state };
        st.anti = m;
        st.anti_snap = None;
    }

    /// Set the smudge-under-wash composite base (TODO #6): the sampler
    /// reads buffer OVER this document. `None` on every other path.
    pub(crate) unsafe fn bind_composite_base(&self, base: *mut Document) {
        unsafe { (*self.state).composite_base = base };
    }

    /// Detach the document. Must run before the caller's `&mut Document` is used
    /// again — call it on every path out of a batch.
    pub(crate) fn unbind(&self) {
        unsafe { (*self.state).doc = std::ptr::null_mut() };
    }

    /// RAII for one bound batch: `Drop` clears `composite_base` AND the doc
    /// binding, so an unwind out of the C callbacks (`end_atomic` processes
    /// the dab queue through Rust tile handlers that can panic) cannot leave
    /// a stale pointer for the next stroke's fetches to read through. The
    /// doc-binding half of this hole predates the composite base (audit
    /// 36–48 §3); one guard closes both.
    pub(crate) fn bound_guard(&self) -> BoundGuard<'_> {
        BoundGuard { surface: self }
    }
}

/// The guard [`TileSurface::bound_guard`] hands out. Bind it to a NAMED
/// `_bound` variable that lives to the end of the batch — `let _ = …`
/// drops it instantly and silently restores the hole this closes. Do NOT
/// call `unbind`/`bind_composite_base(null)` by hand while it is live.
pub(crate) struct BoundGuard<'a> {
    surface: &'a TileSurface,
}

impl Drop for BoundGuard<'_> {
    fn drop(&mut self) {
        unsafe { self.surface.bind_composite_base(std::ptr::null_mut()) };
        self.surface.unbind();
    }
}

impl Drop for TileSurface {
    fn drop(&mut self) {
        unsafe {
            // Frees the operation queue and the C struct, not our state.
            ffi::mn_surface_free(self.raw);
            drop(Box::from_raw(self.state));
        }
    }
}

/// `tile_request_start` — called from `csrc/mn_surface.c`.
///
/// Returns a pointer to 64*64*4 `u16` that stays valid until the matching
/// `mn_brush_tile_request_end`. Never returns null: libmypaint only prints a
/// warning and bails on null, which would turn a bug here into a silently
/// missing dab.
///
/// # Safety
/// `state` must be the `SurfaceState` pointer given to `mn_surface_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mn_brush_tile_request_start(
    state: *mut c_void,
    tx: c_int,
    ty: c_int,
    readonly: c_int,
) -> *mut u16 {
    let st = unsafe { &mut *(state as *mut SurfaceState) };
    // Copy the pointer out first: nothing borrows `st` across the `&mut Document`.
    let doc_ptr = st.doc;
    if doc_ptr.is_null() {
        st.scratch.fill(0);
        return st.scratch.as_mut_ptr();
    }
    let doc = unsafe { &mut *doc_ptr };

    let idx = TileIdx::new(tx, ty);
    let (ex, ey) = doc.tile_extent();
    let on_canvas = tx >= 0 && ty >= 0 && tx < ex && ty < ey;

    if readonly != 0 && st.sel_mode {
        // The selection scratch reads as transparent-black-with-coverage:
        // RGB 0 keeps the premultiplied math honest while the engine
        // accumulates alpha (the coverage payload).
        let t = if on_canvas {
            doc.sel_scratch.tiles.get(&idx)
        } else {
            None
        };
        match t {
            Some(tile) => {
                for (d, px) in st
                    .scratch
                    .chunks_exact_mut(4)
                    .zip(tile.data().chunks_exact(4))
                {
                    let a = px[3];
                    d[0] = 0;
                    d[1] = 0;
                    d[2] = 0;
                    d[3] = a;
                }
            }
            None => st.scratch.fill(0),
        }
        return st.scratch.as_mut_ptr();
    }
    if readonly != 0 && st.mask_mode {
        // LM-004: sample the MASK's coverage (no wash-composite path —
        // masks are plain coverage; wash-on-mask is out of scope).
        let mask = doc.active_layer().mask.as_ref();
        let m = if on_canvas {
            mask.and_then(|m| m.tiles.get(&idx))
        } else {
            None
        };
        match m {
            Some(tile) => st.scratch.copy_from_slice(tile.data()),
            // Same rule the paint half below writes with: a tile a FULL
            // window does not hold reads as full coverage, not as empty.
            None => st.scratch.fill(if on_canvas && mask.is_some_and(|m| m.full) {
                mn_core::tile::FIX15_ONE as u16
            } else {
                0
            }),
        }
        return st.scratch.as_mut_ptr();
    }
    if readonly != 0 {
        // Smudge/`get_color` sampling. Hand out a copy rather than a `*mut` into
        // a shared tile: it costs one 32 KiB memcpy per sampled tile per dab,
        // only for smudge brushes, and it keeps "C got a mutable pointer to
        // something Rust considers shared" out of the codebase entirely.
        // Under a live GPU dab stroke the oracle goes FIRST: the freshest
        // pixels are in the renderer's tile cache (the CPU tiles are stale by
        // construction under BYPASS), and a declined oracle means the stroke
        // never touched this tile — the CPU copy below is then exact.
        let served = TILE_ORACLE.with(|c| {
            c.borrow()
                .map(|(f, ctx)| f(ctx, tx, ty, &mut st.scratch))
                .unwrap_or(false)
        });
        if served {
            // SMUDGE-UNDER-WASH on the GPU path (P4): the oracle's tile is
            // the IN-FLIGHT wash buffer; with a composite base bound the
            // sampler must still see buffer OVER layer — the ink the user
            // sees — same as the CPU branch below. Same premul-over math
            // as `composite_into`, with the oracle's scratch as the over.
            let base_ptr = st.composite_base;
            if !base_ptr.is_null() {
                let base = unsafe { &*base_ptr };
                if let Some(u) = if on_canvas {
                    base.active_layer().tile(idx)
                } else {
                    None
                } {
                    for (d, b) in st.scratch.chunks_exact_mut(4).zip(u.data().chunks_exact(4)) {
                        let sa = d[3] as u32;
                        for c in 0..4 {
                            d[c] = (d[c] as u32 + b[c] as u32 * (32768 - sa) / 32768) as u16;
                        }
                    }
                }
            }
            return st.scratch.as_mut_ptr();
        }
        {
            // SMUDGE-UNDER-WASH (TODO #6): with a composite base set, the
            // sampler sees buffer OVER layer — the ink the user sees — not
            // the blank wash buffer alone.
            let base_ptr = st.composite_base;
            if !base_ptr.is_null() {
                let base = unsafe { &*base_ptr };
                let over = if on_canvas {
                    doc.active_layer().tile(idx)
                } else {
                    None
                };
                let under = if on_canvas {
                    base.active_layer().tile(idx)
                } else {
                    None
                };
                composite_into(&mut st.scratch, over, under);
                return st.scratch.as_mut_ptr();
            }
            let src = if on_canvas {
                doc.active_layer().tile(idx)
            } else {
                None
            };
            match src {
                Some(tile) => st.scratch.copy_from_slice(tile.data()),
                None => st.scratch.fill(0),
            }
        }
        return st.scratch.as_mut_ptr();
    }

    if !on_canvas {
        // Off-canvas dab: give it a scratch tile and drop the writes, rather
        // than growing the layer past the document bounds.
        st.scratch.fill(0);
        return st.scratch.as_mut_ptr();
    }

    if st.sel_mode {
        // The paint path lands in the selection scratch's coverage tiles
        // (same shape as the mask branch: Arc::make_mut unshares, the
        // revision is the GPU's rebuild signal — though nothing renders
        // the scratch; the OVERLAY draws the preview ants). Selection
        // changes are not undo steps (CSP parity), so no op recording.
        let t = doc
            .sel_scratch
            .tiles
            .entry(idx)
            .or_insert_with(|| Arc::new(Tile::new_transparent()));
        doc.sel_scratch.revision = mn_core::tile::next_revision();
        return Arc::make_mut(t).data_mut().as_mut_ptr();
    }

    if st.mask_mode {
        // LM-004: the paint path lands in the mask's coverage tiles.
        // Arc::make_mut unshares against snapshots; the mask revision is
        // the GPU upload-fold's rebuild signal. NOTE: these writes bypass
        // the layer's op recording — undo comes from the app's
        // mask_op_begin/mask_op_end bracket instead, and BECAUSE the writes
        // land live per dab, that bracket must open at stroke BEGIN (the
        // begin-half rule in docs/CODE-MAP.md).
        //
        // Audit H1 (rounds 50-68): the flag can outlive the mask — layer
        // selection, mask delete, bake, undo all move the ground under it.
        // A panic here unwinds out of this `extern "C"` callback and
        // ABORTS the process, so the maskless case degrades to dropped
        // dabs (scratch tile, like the readonly half above). The app
        // layer also disarms on every known transition; this is the
        // backstop that makes an unknown one harmless.
        let Some(m) = doc.active_layer_mut().mask.as_mut() else {
            st.scratch.fill(0);
            return st.scratch.as_mut_ptr();
        };
        // A FULL window materialises new tiles OPAQUE, a carved one empty
        // (`LayerMask::blank_tile`): under a full window an eraser dab
        // landing where the mask holds nothing must take the correction off
        // the dab's footprint, not off the whole 64×64 tile.
        let blank = m.blank_tile();
        let t = m.tiles.entry(idx).or_insert_with(|| Arc::new(blank));
        m.revision = mn_core::tile::next_revision();
        return Arc::make_mut(t).data_mut().as_mut_ptr();
    }

    // The paint path. `tile_mut` creates-if-absent, does the copy-on-write
    // unshare against undo snapshots, and bumps the revision the GPU watches.
    let t = doc.active_layer_mut().tile_mut(idx);
    // Row 42 (anti-overflow): C writes straight into the tile, so the
    // barrier is enforced by snapshot/restore — remember the tile as it
    // stood, and `tile_request_end` puts every BLOCKED pixel back.
    if let Some(m) = st.anti.as_ref() {
        st.anti_snap = Some((idx, t.data().to_vec(), m.clone()));
    }
    t.data_mut().as_mut_ptr()
}

/// `tile_request_end` — called from `csrc/mn_surface.c`.
///
/// Normally nothing to do: writes landed directly in the tile and
/// `tile_mut` already published a fresh revision at request time. Kept
/// because libmypaint's contract requires the callback to exist, and
/// because per-tile dirty tracking (if the GPU ever wants finer grain
/// than "revision changed") belongs here.
///
/// Row 42: with the anti-overflow barrier armed, the matching snapshot
/// taken at request START is replayed here over every blocked pixel —
/// C's dab never keeps its paint on the reference's ink.
///
/// # Safety
/// `state` must be the `SurfaceState` pointer given to `mn_surface_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mn_brush_tile_request_end(
    state: *mut c_void,
    tx: c_int,
    ty: c_int,
    readonly: c_int,
) {
    if readonly != 0 {
        return;
    }
    let st = unsafe { &mut *(state as *mut SurfaceState) };
    let Some((snap_idx, snap, mask)) = st.anti_snap.take() else {
        return;
    };
    if snap_idx != TileIdx::new(tx, ty) {
        return;
    }
    let Some(doc) = (unsafe { st.doc.as_mut() }) else {
        return;
    };
    let (ox, oy) = snap_idx.origin();
    let tile = doc.active_layer_mut().tile_mut(snap_idx);
    let data = tile.data_mut();
    let ts = mn_core::TILE_SIZE as usize;
    for (i, (d, s)) in data
        .chunks_exact_mut(4)
        .zip(snap.chunks_exact(4))
        .enumerate()
    {
        let x = ox as usize + i % ts;
        let y = oy as usize + i / ts;
        if mask.blocked(x as i32, y as i32) {
            d.copy_from_slice(s);
        }
    }
}

/// `over` premultiplied fix15 tile composited onto `under` into `out`
/// (src-over, the document composite's math). Missing tiles read as
/// transparent.
fn composite_into(out: &mut [u16], over: Option<&mn_core::Tile>, under: Option<&mn_core::Tile>) {
    match (over, under) {
        (Some(o), Some(u)) => {
            for ((d, s), b) in out
                .chunks_exact_mut(4)
                .zip(o.data().chunks_exact(4))
                .zip(u.data().chunks_exact(4))
            {
                let sa = s[3] as u32;
                for c in 0..4 {
                    d[c] = (s[c] as u32 + b[c] as u32 * (32768 - sa) / 32768) as u16;
                }
            }
        }
        (Some(o), None) => out.copy_from_slice(o.data()),
        (None, Some(u)) => out.copy_from_slice(u.data()),
        (None, None) => out.fill(0),
    }
}
