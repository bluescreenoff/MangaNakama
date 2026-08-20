//! Hand-written FFI to the vendored libmypaint (no bindgen — this machine has
//! no libclang, and the surface we need is small enough to read in one screen).
//!
//! Every signature here was transcribed from the v1.6.1 headers in
//! `vendor/libmypaint/`. If you bump the vendored version, re-check them: C has
//! no link-time type checking, so a changed signature is a silent stack
//! mismatch, not an error. The pinned ones:
//!
//! - `mypaint-brush.h` — `mypaint_brush_stroke_to` is the **8-argument** 1.6
//!   form (`brush, surface, x, y, pressure, xtilt, ytilt, dtime`). 1.6 also
//!   ships `_2` variants with viewzoom/rotation/barrel; we do not use them.
//! - `mypaint-surface.h` — `begin_atomic` / `end_atomic(surface, roi)`.
//! - `mypaint-brush-settings.h` — `*_from_cname` lookups, used only by tests to
//!   prove our generated Rust ids match the C enum.
//! - `csrc/mn_surface.c` — our own tiled-surface subclass.
//!
//! `MyPaintBrushSetting` / `MyPaintBrushInput` are plain C enums, so they cross
//! the boundary as `c_int`. `gboolean` is `gint` is `c_int`.

use core::ffi::{c_char, c_double, c_float, c_int, c_void};

/// Opaque `struct MyPaintBrush`.
#[repr(C)]
pub struct MyPaintBrush {
    _private: [u8; 0],
}

/// Opaque `struct MyPaintSurface`. Only ever handled as a pointer.
#[repr(C)]
pub struct MyPaintSurface {
    _private: [u8; 0],
}

/// Opaque `MnSurface` from `csrc/mn_surface.c` — our tiled-surface subclass.
#[repr(C)]
pub struct MnSurface {
    _private: [u8; 0],
}

/// `MyPaintRectangle` (mypaint-rectangle.h): four ints, in this order.
///
/// libmypaint fills this as the invalidation rect of an `end_atomic` batch. We
/// take it and drop it: `core::Layer::tile_mut` already bumps a per-tile
/// revision, which is the dirty signal the GPU cache actually reads.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MyPaintRectangle {
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
}

unsafe extern "C" {
    // -- brush lifecycle ---------------------------------------------------
    pub fn mypaint_brush_new() -> *mut MyPaintBrush;
    pub fn mypaint_brush_unref(self_: *mut MyPaintBrush);

    /// Discard stroke state; the next `stroke_to` only positions the pen.
    pub fn mypaint_brush_reset(self_: *mut MyPaintBrush);
    /// Start a new stroke (resets stroke-duration/painting-time counters).
    pub fn mypaint_brush_new_stroke(self_: *mut MyPaintBrush);

    /// PATCHES.md #12: the legacy entry with the view transform surfaced —
    /// speed/direction inputs are computed in view space (zoom > 0). This is
    /// the ONLY stroke entry we call; at (1.0, 0.0, 0) it is bit-identical
    /// to the stock 8-argument 1.6 `mypaint_brush_stroke_to`. viewflip
    /// (gboolean) mirrors the motion-direction inputs under a flipped view.
    pub fn mypaint_brush_stroke_to_view(
        self_: *mut MyPaintBrush,
        surface: *mut MyPaintSurface,
        x: c_float,
        y: c_float,
        pressure: c_float,
        xtilt: c_float,
        ytilt: c_float,
        dtime: c_double,
        viewzoom: c_float,
        viewrotation: c_float,
        viewflip: c_int,
    ) -> c_int;

    // -- settings ----------------------------------------------------------
    pub fn mypaint_brush_set_base_value(self_: *mut MyPaintBrush, id: c_int, value: c_float);
    pub fn mypaint_brush_get_base_value(self_: *mut MyPaintBrush, id: c_int) -> c_float;
    pub fn mypaint_brush_set_mapping_n(self_: *mut MyPaintBrush, id: c_int, input: c_int, n: c_int);
    pub fn mypaint_brush_get_mapping_n(self_: *mut MyPaintBrush, id: c_int, input: c_int) -> c_int;
    pub fn mypaint_brush_set_mapping_point(
        self_: *mut MyPaintBrush,
        id: c_int,
        input: c_int,
        index: c_int,
        x: c_float,
        y: c_float,
    );
    /// Read one mapping point back. Asserts inside the C if `index >= n`, so
    /// always bound it with `mypaint_brush_get_mapping_n` first.
    pub fn mypaint_brush_get_mapping_point(
        self_: *mut MyPaintBrush,
        id: c_int,
        input: c_int,
        index: c_int,
        x: *mut c_float,
        y: *mut c_float,
    );
    /// Stock values for every setting, plus the default pressure->opacity ramp.
    pub fn mypaint_brush_from_defaults(self_: *mut MyPaintBrush);

    /// `-1` (as a C enum) when the name is not a setting.
    ///
    /// Production code uses the generated `settings::setting_id`; this exists so
    /// tests can ask the C what it thinks the ids are, instead of trusting the
    /// generator that produced both sides.
    #[allow(dead_code)]
    pub fn mypaint_brush_setting_from_cname(cname: *const c_char) -> c_int;
    /// `-1` (as a C enum) when the name is not an input. See above.
    #[allow(dead_code)]
    pub fn mypaint_brush_input_from_cname(cname: *const c_char) -> c_int;

    // -- surface -----------------------------------------------------------
    pub fn mypaint_surface_begin_atomic(self_: *mut MyPaintSurface);
    pub fn mypaint_surface_end_atomic(self_: *mut MyPaintSurface, roi: *mut MyPaintRectangle);

    // -- our tiled-surface subclass (csrc/mn_surface.c) --------------------
    pub fn mn_surface_new(rust_state: *mut c_void) -> *mut MnSurface;
    pub fn mn_surface_interface(self_: *mut MnSurface) -> *mut MyPaintSurface;
    pub fn mn_surface_free(self_: *mut MnSurface);
}
