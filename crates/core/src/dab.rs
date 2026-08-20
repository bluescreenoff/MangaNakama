//! The recorded-dab ABI (GPU-dabs P0/P1, vendor/PATCHES.md #11): one dab as
//! the CPU rasterizer sees it, after `draw_dab_internal`'s early-outs and
//! clamps — exactly the values the raster path consumes. Lives in core so
//! both consumers stay on the documented dependency arrows: mn-brush records
//! it, mn-gpu's compute path rasterizes from it.

use std::collections::BTreeSet;

use crate::TileIdx;

/// One recorded dab (the P0 ABI, docs/design/GPU-DABS.md).
#[derive(Clone, Copy, Debug)]
pub struct DabParams {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    /// Straight colour, fix15 (`1.0 == 1<<15`).
    pub color: [u16; 3],
    /// Straight alpha `color_a`, 0..1.
    pub alpha: f32,
    pub opaque: f32,
    pub hardness: f32,
    pub aspect_ratio: f32,
    pub angle: f32,
    pub lock_alpha: f32,
    /// Spectral paint weight (the `get_color` path); Normal mode is 0.
    pub paint: f32,
    /// Texture-tip scroll offset THIS dab sees, in mask px — `(int)` of the
    /// brush's crawl accumulator, captured at record time exactly as the C's
    /// `render_dab_mask` would cast it. `[0, 0]` when no texture is active
    /// (and ignored then); the mask DATA itself rides per-flush, not per dab.
    pub tex_off: [i32; 2],
    /// Dab-anchored stamp angle in degrees, UNFOLDED (PATCHES.md #10
    /// amendment 2) — captured at record time like `tex_off`. Only read in
    /// dab-anchored texture mode; 0 otherwise.
    pub tex_angle: f32,
}

/// What one stroke recorded: the dab list in issue order plus every tile
/// index any dab touched (the P1 compute dispatch's workgroup set).
#[derive(Default, Debug)]
pub struct DabRecord {
    pub dabs: Vec<DabParams>,
    pub tiles: BTreeSet<TileIdx>,
}
