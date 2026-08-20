//! Brush setting/input ids, generated at build time from the same
//! `vendor/libmypaint/brushsettings.json` that produces libmypaint's C enums.
//!
//! The two **must** agree: these values are passed straight to
//! `mypaint_brush_set_base_value` and friends, and an off-by-one would not
//! error — it would apply a `.myb` value to the neighbouring setting and make
//! every brush feel subtly wrong. Generating both from one file in one pass is
//! what makes that structurally impossible; `settings_ids_match_the_c_enum` in
//! `lib.rs` re-checks it against the C at test time anyway.
//!
//! Provides: `SETTING_NAMES`, `INPUT_NAMES`, `SETTING_DEFAULTS`, and the
//! `setting::` / `input::` id constant modules.

use core::ffi::c_int;

include!(concat!(env!("OUT_DIR"), "/settings_gen.rs"));

/// `MyPaintBrushSetting` id for a canonical name, or `None` if this libmypaint
/// has no such setting (a preset written by a newer MyPaint).
pub fn setting_id(name: &str) -> Option<c_int> {
    SETTING_NAMES
        .iter()
        .position(|n| *n == name)
        .map(|i| i as c_int)
}

/// `MyPaintBrushInput` id for a canonical name, or `None`.
pub fn input_id(name: &str) -> Option<c_int> {
    INPUT_NAMES
        .iter()
        .position(|n| *n == name)
        .map(|i| i as c_int)
}
