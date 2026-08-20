//! Pixel filters: the blur family (CSP Filter ▸ Blur) plus Mosaic.
//!
//! **The first convolution in this tree.** Every other whole-layer op here
//! (brightness→opacity, gradient, fill) is POINTWISE: it reads the pixel it
//! writes and nothing else, so the 64×64 tile grid is invisible to it. A blur
//! reads a NEIGHBOURHOOD, and a tile edge is not a real edge — running the
//! kernel per tile would fabricate a boundary condition every 64 px and leave
//! seams that read as clean at 25% zoom and as a grid in print.
//!
//! So nothing here ever sees a tile. The whole affected region is gathered
//! into one flat [`Raster`] **with a halo margin of the filter's own reach**,
//! the kernel runs on that flat buffer, and only the interior — the part whose
//! every tap landed inside the buffer — is scattered back into tiles. The halo
//! is the entire trick; [`Filter::reach`] is its width and is the one number
//! that must never be under-stated.
//!
//! **Premultiplied is the right space to average in.** Tile pixels are
//! premultiplied fix15 (`1.0 == 32768`) and we blur them as they lie. That is
//! deliberate and it is correct: premultiplied RGB is colour×coverage, i.e.
//! *light contribution*, and light is what a lens integrates. Averaging
//! un-premultiplied colour instead makes a transparent pixel's arbitrary
//! colour leak into its neighbours (the classic black halo). This is NOT the
//! same mistake as averaging display-encoded bytes (the zoomed-out ink bug):
//! fix15 is linear in coverage, and premultiplication is linear too.
//!
//! What is deliberately NOT here, and why:
//! * Radial blur and Spin blur — both need a draggable centre handle on the
//!   canvas, which is an interaction round, not a filter.
//! * Lens blur — aperture-shaped bokeh is a different algorithm (a polygonal
//!   kernel with highlight clipping), not a parameter of these.

use crate::doc::{Document, Layer};
use crate::tile::{TILE_CHANNELS, TILE_LEN, TILE_SIZE, TileIdx};

/// Hard ceiling on Gaussian σ, in canvas pixels. Not a taste limit: the box
/// radii grow with σ and the halo grows with them, so an unbounded σ would let
/// one menu click ask for a scratch buffer the size of the sky.
pub const MAX_SIGMA: f32 = 250.0;

/// FL-010 "Blur": the light one-shot. σ chosen so the three box passes come
/// out as radius 1,1,1 — the smallest genuinely symmetric blur this
/// approximation can express.
const BLUR_SIGMA: f32 = 1.4;
/// FL-010 "Blur (strong)": the heavy one-shot, ~4 px of softening.
const BLUR_STRONG_SIGMA: f32 = 4.0;

// ------------------------------------------------------------------ raster --

/// A flat rectangle of premultiplied fix15 RGBA — the scratch space filters
/// work in. Row-major, `w * h * 4` `u16`s, no tiles, no gaps.
///
/// Outside the rectangle is defined as fully transparent. That convention only
/// ever applies to the halo band, which is never written back, so it can never
/// reach the document.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Raster {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u16>,
}

impl Raster {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![0u16; w * h * TILE_CHANNELS],
        }
    }

    /// Premultiplied fix15 RGBA at `(x, y)`; transparent outside.
    pub fn pixel(&self, x: usize, y: usize) -> [u16; 4] {
        if x >= self.w || y >= self.h {
            return [0; 4];
        }
        let o = (y * self.w + x) * TILE_CHANNELS;
        [self.px[o], self.px[o + 1], self.px[o + 2], self.px[o + 3]]
    }

    pub fn set_pixel(&mut self, x: usize, y: usize, v: [u16; 4]) {
        let o = (y * self.w + x) * TILE_CHANNELS;
        self.px[o..o + TILE_CHANNELS].copy_from_slice(&v);
    }
}

// ------------------------------------------------------------------ filter --

/// Which way a motion blur runs (CSP's Direction row).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MotionDir {
    /// Smear both ways around the pixel — the streak stays centred.
    #[default]
    Both,
    /// Trail from the pixel along +angle only.
    Forward,
    /// Trail from the pixel along −angle only.
    Backward,
}

/// How a motion blur weights its samples (CSP's Mode row).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MotionMode {
    /// Flat weights — a hard-ended streak, CSP's "Box".
    #[default]
    Uniform,
    /// Weights taper to zero at the far end of the streak, CSP's "Smooth".
    Taper,
}

/// One filter, parameters included. The enum IS the command payload: the menu
/// pushes a value of this and `Document::apply_filter` runs it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Filter {
    /// FL-013 Smoothing: one 3×3 binomial pass — softens stair-stepped edges
    /// by adding the intermediate values, without visibly blurring the art.
    Smoothing,
    /// FL-010 Blur: the light no-dialog one-shot.
    Blur,
    /// FL-010 Blur (strong): the heavy no-dialog one-shot.
    BlurStrong,
    /// FL-011 Gaussian blur: σ in canvas pixels.
    Gaussian { sigma: f32 },
    /// FL-015 Motion blur: `angle` in degrees (0 = →, positive = clockwise on
    /// a y-down canvas), `length` in canvas pixels.
    Motion {
        angle: f32,
        length: f32,
        dir: MotionDir,
        mode: MotionMode,
    },
    /// FL-033 Mosaic: pixelate to `cell`-px squares, anchored to the CANVAS
    /// origin so the grid does not shift with the selection.
    Mosaic { cell: u32 },
}

impl Filter {
    /// Undo-stack label, and the dialog title.
    pub fn label(self) -> &'static str {
        match self {
            Filter::Smoothing => "Smoothing",
            Filter::Blur => "Blur",
            Filter::BlurStrong => "Blur (strong)",
            Filter::Gaussian { .. } => "Gaussian blur",
            Filter::Motion { .. } => "Motion blur",
            Filter::Mosaic { .. } => "Mosaic",
        }
    }

    /// How far, in pixels, one output pixel can reach for its input.
    ///
    /// **The load-bearing number.** It is both the halo width of the scratch
    /// buffer and how far the op is allowed to spread ink beyond the layer's
    /// existing footprint. Understate it and the outermost written pixels read
    /// the halo's transparent fill instead of the real neighbours — which is
    /// exactly a tile seam, just moved to the region edge. Every arm below
    /// shares its arithmetic with the code that does the work, so the two
    /// cannot drift.
    pub fn reach(self) -> i32 {
        match self {
            Filter::Smoothing => 1,
            Filter::Blur => gaussian_reach(BLUR_SIGMA),
            Filter::BlurStrong => gaussian_reach(BLUR_STRONG_SIGMA),
            Filter::Gaussian { sigma } => gaussian_reach(sigma),
            Filter::Motion { length, dir, .. } => {
                let (t0, t1) = motion_span(length, dir);
                // +1 for the bilinear tap straddling the far sample.
                t0.abs().max(t1.abs()).ceil() as i32 + 1
            }
            // A cell straddling the region edge must be whole inside the
            // buffer, and a cell is at most `cell` wide.
            Filter::Mosaic { cell } => cell.clamp(1, 4096) as i32 - 1,
        }
    }

    /// Run the filter in place on `buf`. `(ox, oy)` is the canvas-pixel
    /// coordinate of `buf`'s top-left, which only Mosaic needs (its cell grid
    /// is anchored to the canvas, not to the buffer).
    pub fn run(self, buf: &mut Raster, ox: i32, oy: i32) {
        match self {
            Filter::Smoothing => smoothing(buf),
            Filter::Blur => gaussian(buf, BLUR_SIGMA),
            Filter::BlurStrong => gaussian(buf, BLUR_STRONG_SIGMA),
            Filter::Gaussian { sigma } => gaussian(buf, sigma),
            Filter::Motion {
                angle,
                length,
                dir,
                mode,
            } => motion(buf, angle, length, dir, mode),
            Filter::Mosaic { cell } => mosaic(buf, cell.clamp(1, 4096) as i32, ox, oy),
        }
    }

    /// True when the parameters make this a no-op, so the caller can refuse
    /// instead of pushing an empty undo step.
    pub fn is_identity(self) -> bool {
        match self {
            Filter::Gaussian { sigma } => box_radii(sigma).iter().all(|&r| r == 0),
            Filter::Motion { length, .. } => !(length > 0.5),
            Filter::Mosaic { cell } => cell <= 1,
            _ => false,
        }
    }
}

// ----------------------------------------------------------------- kernels --

/// Kovesi's three-box approximation of a Gaussian: the box widths whose
/// successive application has (very nearly) the requested σ.
///
/// Three uniform passes convolve to a quadratic B-spline, which is within
/// ~0.5 % of a true Gaussian and costs O(1) per pixel per pass instead of
/// O(σ). At 600 dpi a σ-20 shadow is a 121-tap kernel; the direct form is not
/// shippable and the approximation is what every fast implementation uses.
///
/// The cost of the trick is quantisation at the bottom of the range: the
/// widths are odd integers, so σ below ~1 rounds to three identity passes.
/// [`Filter::is_identity`] reports that rather than pretending.
fn box_radii(sigma: f32) -> [usize; 3] {
    const N: f32 = 3.0;
    let sigma = sigma.clamp(0.0, MAX_SIGMA);
    if !(sigma > 0.0) {
        return [0; 3];
    }
    let v = 12.0 * sigma * sigma;
    let mut wl = (v / N + 1.0).sqrt().floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let m = ((v - N * (wl * wl) as f32 - 4.0 * N * wl as f32 - 3.0 * N) / (-4.0 * wl as f32 - 4.0))
        .round()
        .clamp(0.0, N) as usize;
    let mut out = [0usize; 3];
    for (i, o) in out.iter_mut().enumerate() {
        let w = if i < m { wl } else { wu } as usize;
        *o = (w - 1) / 2;
    }
    out
}

/// Total reach of the three box passes — they compose, so the radii add.
fn gaussian_reach(sigma: f32) -> i32 {
    box_radii(sigma).iter().sum::<usize>() as i32
}

/// One horizontal box pass, running-sum. Outside the buffer counts as
/// transparent (the denominator stays the full window), which is the same
/// convention the gather uses for absent tiles.
fn box_h(src: &Raster, dst: &mut Raster, r: usize) {
    let (w, h) = (src.w, src.h);
    let denom = (2 * r + 1) as u32;
    let half = denom / 2;
    for y in 0..h {
        let row = y * w * TILE_CHANNELS;
        let mut acc = [0u32; TILE_CHANNELS];
        for x in 0..(r + 1).min(w) {
            let o = row + x * TILE_CHANNELS;
            for (c, a) in acc.iter_mut().enumerate() {
                *a += src.px[o + c] as u32;
            }
        }
        for x in 0..w {
            let o = row + x * TILE_CHANNELS;
            for (c, a) in acc.iter().enumerate() {
                dst.px[o + c] = ((*a + half) / denom) as u16;
            }
            if x >= r {
                let drop = row + (x - r) * TILE_CHANNELS;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a -= src.px[drop + c] as u32;
                }
            }
            let add = x + r + 1;
            if add < w {
                let take = row + add * TILE_CHANNELS;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += src.px[take + c] as u32;
                }
            }
        }
    }
}

/// One vertical box pass. Same running sum, striding by a row.
fn box_v(src: &Raster, dst: &mut Raster, r: usize) {
    let (w, h) = (src.w, src.h);
    let denom = (2 * r + 1) as u32;
    let half = denom / 2;
    let stride = w * TILE_CHANNELS;
    for x in 0..w {
        let col = x * TILE_CHANNELS;
        let mut acc = [0u32; TILE_CHANNELS];
        for y in 0..(r + 1).min(h) {
            let o = col + y * stride;
            for (c, a) in acc.iter_mut().enumerate() {
                *a += src.px[o + c] as u32;
            }
        }
        for y in 0..h {
            let o = col + y * stride;
            for (c, a) in acc.iter().enumerate() {
                dst.px[o + c] = ((*a + half) / denom) as u16;
            }
            if y >= r {
                let drop = col + (y - r) * stride;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a -= src.px[drop + c] as u32;
                }
            }
            let add = y + r + 1;
            if add < h {
                let take = col + add * stride;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += src.px[take + c] as u32;
                }
            }
        }
    }
}

/// FL-011: separable Gaussian, in place. Three box passes per axis; the
/// horizontal ones all run before the vertical ones, which is legal because
/// box blur is separable and the passes commute.
fn gaussian(buf: &mut Raster, sigma: f32) {
    let radii = box_radii(sigma);
    if radii.iter().all(|&r| r == 0) {
        return;
    }
    let mut tmp = Raster::new(buf.w, buf.h);
    for r in radii {
        if r > 0 {
            box_h(buf, &mut tmp, r);
            std::mem::swap(buf, &mut tmp);
        }
    }
    for r in radii {
        if r > 0 {
            box_v(buf, &mut tmp, r);
            std::mem::swap(buf, &mut tmp);
        }
    }
}

/// FL-013: the 3×3 binomial [1 2 1]⊗[1 2 1]/16, separable, in place. Weak on
/// purpose — its job is filling in the missing intermediate values along a
/// jagged edge, not softening the drawing.
fn smoothing(buf: &mut Raster) {
    let mut tmp = Raster::new(buf.w, buf.h);
    tent_h(buf, &mut tmp);
    std::mem::swap(buf, &mut tmp);
    tent_v(buf, &mut tmp);
    std::mem::swap(buf, &mut tmp);
}

fn tent_h(src: &Raster, dst: &mut Raster) {
    let (w, h) = (src.w, src.h);
    for y in 0..h {
        for x in 0..w {
            let (l, m, r) = (
                src.pixel(x.wrapping_sub(1), y),
                src.pixel(x, y),
                src.pixel(x + 1, y),
            );
            let mut out = [0u16; TILE_CHANNELS];
            for c in 0..TILE_CHANNELS {
                out[c] =
                    ((l[c] as u32 + 2 * m[c] as u32 + r[c] as u32 + 2) / 4).min(u16::MAX as u32)
                        as u16;
            }
            dst.set_pixel(x, y, out);
        }
    }
}

fn tent_v(src: &Raster, dst: &mut Raster) {
    let (w, h) = (src.w, src.h);
    for y in 0..h {
        for x in 0..w {
            let (u, m, d) = (
                src.pixel(x, y.wrapping_sub(1)),
                src.pixel(x, y),
                src.pixel(x, y + 1),
            );
            let mut out = [0u16; TILE_CHANNELS];
            for c in 0..TILE_CHANNELS {
                out[c] =
                    ((u[c] as u32 + 2 * m[c] as u32 + d[c] as u32 + 2) / 4).min(u16::MAX as u32)
                        as u16;
            }
            dst.set_pixel(x, y, out);
        }
    }
}

/// The parameter range a motion blur integrates over, in pixels along the
/// angle. Shared by [`Filter::reach`] and [`motion`] so the halo and the taps
/// can never disagree.
fn motion_span(length: f32, dir: MotionDir) -> (f32, f32) {
    let l = length.max(0.0).min(4096.0);
    match dir {
        MotionDir::Both => (-l * 0.5, l * 0.5),
        MotionDir::Forward => (0.0, l),
        MotionDir::Backward => (-l, 0.0),
    }
}

/// FL-015: a directional line integral — the same machinery as the Gaussian,
/// walked along one angle instead of the two axes. Not separable, so it is the
/// one filter here whose cost grows with its parameter.
fn motion(buf: &mut Raster, angle_deg: f32, length: f32, dir: MotionDir, mode: MotionMode) {
    let (t0, t1) = motion_span(length, dir);
    let span = t1 - t0;
    if span <= 0.5 {
        return;
    }
    // One sample per pixel of travel; bilinear between them, so a 37° streak
    // does not come out as a staircase.
    let n = (span.ceil() as usize + 1).max(2);
    let a = angle_deg.to_radians();
    let (dx, dy) = (a.cos(), a.sin());
    let far = t0.abs().max(t1.abs()).max(1e-6);
    let mut src = Raster::new(buf.w, buf.h);
    std::mem::swap(buf, &mut src);
    for y in 0..src.h {
        for x in 0..src.w {
            let mut acc = [0f32; TILE_CHANNELS];
            let mut wsum = 0f32;
            for i in 0..n {
                let t = t0 + span * (i as f32) / ((n - 1) as f32);
                let w = match mode {
                    MotionMode::Uniform => 1.0,
                    // Linear taper to zero at the far end; the +ε keeps the
                    // outermost sample from contributing literally nothing.
                    MotionMode::Taper => 1.0 - (t.abs() / far) * 0.999,
                };
                let p = sample_bilinear(&src, x as f32 + t * dx, y as f32 + t * dy);
                for c in 0..TILE_CHANNELS {
                    acc[c] += p[c] * w;
                }
                wsum += w;
            }
            let mut out = [0u16; TILE_CHANNELS];
            for c in 0..TILE_CHANNELS {
                out[c] = (acc[c] / wsum + 0.5).clamp(0.0, u16::MAX as f32) as u16;
            }
            buf.set_pixel(x, y, out);
        }
    }
}

/// Bilinear tap; outside the raster is transparent.
fn sample_bilinear(src: &Raster, fx: f32, fy: f32) -> [f32; TILE_CHANNELS] {
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let mut out = [0f32; TILE_CHANNELS];
    for (j, wy) in [(0i32, 1.0 - ty), (1, ty)] {
        if wy <= 0.0 {
            continue;
        }
        let yy = y0 as i32 + j;
        if yy < 0 || yy as usize >= src.h {
            continue;
        }
        for (i, wx) in [(0i32, 1.0 - tx), (1, tx)] {
            if wx <= 0.0 {
                continue;
            }
            let xx = x0 as i32 + i;
            if xx < 0 || xx as usize >= src.w {
                continue;
            }
            let p = src.pixel(xx as usize, yy as usize);
            for c in 0..TILE_CHANNELS {
                out[c] += p[c] as f32 * wx * wy;
            }
        }
    }
    out
}

/// FL-033: average each cell and flood it back. The grid is anchored to the
/// CANVAS origin (`ox`/`oy` place the buffer inside it), not to the buffer, so
/// mosaicking a selection twice — or mosaicking two selections — lands on the
/// same squares both times instead of shifting by the marquee's offset.
fn mosaic(buf: &mut Raster, cell: i32, ox: i32, oy: i32) {
    if cell <= 1 || buf.w == 0 || buf.h == 0 {
        return;
    }
    let cx0 = ox.div_euclid(cell);
    let cx1 = (ox + buf.w as i32 - 1).div_euclid(cell);
    let cy0 = oy.div_euclid(cell);
    let cy1 = (oy + buf.h as i32 - 1).div_euclid(cell);
    for cy in cy0..=cy1 {
        let y0 = (cy * cell - oy).max(0) as usize;
        let y1 = ((cy + 1) * cell - oy).min(buf.h as i32).max(0) as usize;
        for cx in cx0..=cx1 {
            let x0 = (cx * cell - ox).max(0) as usize;
            let x1 = ((cx + 1) * cell - ox).min(buf.w as i32).max(0) as usize;
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let count = ((x1 - x0) * (y1 - y0)) as u64;
            let mut acc = [0u64; TILE_CHANNELS];
            for y in y0..y1 {
                let row = (y * buf.w + x0) * TILE_CHANNELS;
                for p in buf.px[row..row + (x1 - x0) * TILE_CHANNELS].chunks_exact(TILE_CHANNELS) {
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += p[c] as u64;
                    }
                }
            }
            let mut avg = [0u16; TILE_CHANNELS];
            for (c, a) in acc.iter().enumerate() {
                avg[c] = ((*a + count / 2) / count) as u16;
            }
            for y in y0..y1 {
                let row = (y * buf.w + x0) * TILE_CHANNELS;
                for p in
                    buf.px[row..row + (x1 - x0) * TILE_CHANNELS].chunks_exact_mut(TILE_CHANNELS)
                {
                    p.copy_from_slice(&avg);
                }
            }
        }
    }
}

// ------------------------------------------------------- gather / scatter --

/// Copy a canvas-pixel rectangle out of a layer's tiles into one flat buffer.
/// Absent tiles are left transparent, which is what they are.
///
/// Row-slice copies, not per-pixel: a full-page gather is ~24 M pixels and the
/// per-pixel form is measurably the slower half of the whole filter.
fn gather(layer: &Layer, gx: i32, gy: i32, gw: usize, gh: usize) -> Raster {
    let mut out = Raster::new(gw, gh);
    if gw == 0 || gh == 0 {
        return out;
    }
    let t = TILE_SIZE as i32;
    let tx0 = gx.div_euclid(t);
    let tx1 = (gx + gw as i32 - 1).div_euclid(t);
    let ty0 = gy.div_euclid(t);
    let ty1 = (gy + gh as i32 - 1).div_euclid(t);
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let idx = TileIdx::new(tx, ty);
            let Some(tile) = layer.tile(idx) else {
                continue;
            };
            let (ox, oy) = idx.origin();
            let sx0 = (gx - ox).max(0);
            let sx1 = (gx + gw as i32 - ox).min(t);
            let sy0 = (gy - oy).max(0);
            let sy1 = (gy + gh as i32 - oy).min(t);
            if sx1 <= sx0 || sy1 <= sy0 {
                continue;
            }
            let run = (sx1 - sx0) as usize * TILE_CHANNELS;
            for sy in sy0..sy1 {
                let s = (sy as usize * TILE_SIZE + sx0 as usize) * TILE_CHANNELS;
                let dy = (oy + sy - gy) as usize;
                let dx = (ox + sx0 - gx) as usize;
                let d = (dy * gw + dx) * TILE_CHANNELS;
                out.px[d..d + run].copy_from_slice(&tile.data()[s..s + run]);
            }
        }
    }
    out
}

impl Document {
    /// FL-010/011/013/015/033: run `f` over the active layer as ONE undo step,
    /// clipped to the selection when there is one.
    ///
    /// Returns `false` — pushing nothing onto the undo stack — when the layer
    /// refuses (folder, vector, locked), when it is empty, when the selection
    /// covers none of it, or when the parameters are a no-op.
    ///
    /// The shape of the work, and the reason it is shaped that way:
    ///
    /// 1. **Write region** = the layer's tile footprint grown by
    ///    [`Filter::reach`] (a blur bleeds ink OUTWARDS past the ink that made
    ///    it), clipped to canvas ∪ footprint so one click cannot grow a layer
    ///    off into space, then intersected with the selection's tiles and
    ///    rounded out to whole tiles.
    /// 2. **Gather region** = the write region grown by `reach` again. This
    ///    band is the halo. Every pixel we will WRITE has its entire
    ///    neighbourhood inside the buffer, so no tap ever falls on a tile edge
    ///    or on the buffer's transparent surround.
    /// 3. Filter the flat buffer; scatter only the write region back.
    ///
    /// Reads always come from the ORIGINAL pixels because the gather completes
    /// before the first write — a blur must never see its own output.
    ///
    /// Memory: the scratch is `(region + 2·reach)` pixels at 8 bytes, twice
    /// over for the ping-pong. A whole-page Gaussian on B4/600dpi is a few
    /// hundred MB transient; a selection-scoped one is proportional to the
    /// marquee, which is the case that actually happens while drawing.
    pub fn apply_filter(&mut self, f: Filter) -> bool {
        let li = self.active;
        let Some(layer) = self.layers.get(li) else {
            return false;
        };
        if !layer.paintable() || layer.lock || f.is_identity() {
            return false;
        }
        let Some((bx, by, bw, bh)) = layer.tile_bounds() else {
            return false;
        };
        let reach = f.reach();
        let t = TILE_SIZE as i32;

        // Canvas ∪ footprint: the layer may already hold off-canvas tiles
        // (transforms push content out there), and those still get filtered —
        // but the blur is not allowed to invent new territory beyond them.
        let (fx1, fy1) = (bx + bw as i32, by + bh as i32);
        let lim_x0 = bx.min(0);
        let lim_y0 = by.min(0);
        let lim_x1 = fx1.max(self.size.0 as i32);
        let lim_y1 = fy1.max(self.size.1 as i32);
        let dx0 = (bx - reach).max(lim_x0);
        let dy0 = (by - reach).max(lim_y0);
        let dx1 = (fx1 + reach).min(lim_x1);
        let dy1 = (fy1 + reach).min(lim_y1);
        if dx1 <= dx0 || dy1 <= dy0 {
            return false;
        }
        let (mut tx0, mut ty0) = (dx0.div_euclid(t), dy0.div_euclid(t));
        let (mut tx1, mut ty1) = ((dx1 - 1).div_euclid(t) + 1, (dy1 - 1).div_euclid(t) + 1);

        // Which tiles actually get written. With a selection this both skips
        // the untouched ones and shrinks the gather to the marquee's tiles —
        // otherwise a 100 px blur inside a small marquee would gather the
        // whole page to throw all but a corner of it away.
        let sel = self.selection.clone();
        let mut dirty: Vec<TileIdx> = Vec::new();
        if let Some(s) = &sel {
            let (mut sx0, mut sy0, mut sx1, mut sy1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
            for ty in ty0..ty1 {
                for tx in tx0..tx1 {
                    if s.tile_mask(TileIdx::new(tx, ty)).is_some() {
                        dirty.push(TileIdx::new(tx, ty));
                        sx0 = sx0.min(tx);
                        sy0 = sy0.min(ty);
                        sx1 = sx1.max(tx + 1);
                        sy1 = sy1.max(ty + 1);
                    }
                }
            }
            if dirty.is_empty() {
                return false;
            }
            tx0 = sx0;
            ty0 = sy0;
            tx1 = sx1;
            ty1 = sy1;
        } else {
            for ty in ty0..ty1 {
                for tx in tx0..tx1 {
                    dirty.push(TileIdx::new(tx, ty));
                }
            }
        }

        // The halo. `reach` on every side of the write region, gathered from
        // the untouched layer.
        let gx = tx0 * t - reach;
        let gy = ty0 * t - reach;
        let gw = ((tx1 - tx0) * t + 2 * reach) as usize;
        let gh = ((ty1 - ty0) * t + 2 * reach) as usize;
        let mut buf = gather(&self.layers[li], gx, gy, gw, gh);
        f.run(&mut buf, gx, gy);

        let lock_alpha = self.layers[li].lock_alpha;
        self.begin_op();
        self.set_op_label(f.label());
        // One heap block reused for every tile: a `[u16; TILE_LEN]` local is
        // 32 KiB of stack and debug builds choke on it (ARCHITECTURE traps).
        let mut block = vec![0u16; TILE_LEN];
        for idx in dirty {
            let (ox, oy) = idx.origin();
            let sx = (ox - gx) as usize;
            let sy = (oy - gy) as usize;
            let run = TILE_SIZE * TILE_CHANNELS;
            for row in 0..TILE_SIZE {
                let s = ((sy + row) * gw + sx) * TILE_CHANNELS;
                block[row * run..(row + 1) * run].copy_from_slice(&buf.px[s..s + run]);
            }
            // Don't materialise a tile the filter left empty: a whole-canvas
            // blur otherwise allocates every tile of every margin.
            if self.layers[li].tile(idx).is_none() && block.iter().all(|&v| v == 0) {
                continue;
            }
            self.layers[li]
                .tile_mut(idx)
                .data_mut()
                .copy_from_slice(&block);
        }
        // Same order as every paint op: the selection restores what is outside
        // it from the recorded pre-images, then alpha-lock clamps once.
        self.mask_op_to_selection();
        if lock_alpha {
            self.mask_op_to_alpha();
        }
        self.end_op()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Solid opaque black over a canvas-pixel rect, as one undo step.
    fn paint_rect(doc: &mut Document, x0: i32, y0: i32, x1: i32, y1: i32) {
        doc.begin_op();
        for y in y0..y1 {
            for x in x0..x1 {
                let idx = TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                doc.layers[0].tile_mut(idx).set_pixel(
                    (x - ox) as usize,
                    (y - oy) as usize,
                    [0, 0, 0, 32768],
                );
            }
        }
        doc.end_op();
    }

    fn alpha_at(doc: &Document, x: i32, y: i32) -> u16 {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        doc.layers[0]
            .tile(idx)
            .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize)[3])
            .unwrap_or(0)
    }

    fn px_at(doc: &Document, x: i32, y: i32) -> [u16; 4] {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        doc.layers[0]
            .tile(idx)
            .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
            .unwrap_or([0; 4])
    }

    #[test]
    fn box_radii_follow_kovesi() {
        assert_eq!(box_radii(0.0), [0, 0, 0]);
        // σ 1.4 is picked so the light one-shot is the smallest symmetric blur.
        assert_eq!(box_radii(BLUR_SIGMA), [1, 1, 1]);
        assert_eq!(box_radii(BLUR_STRONG_SIGMA), [3, 3, 4]);
        // The radii add up to the reach the halo is sized from.
        assert_eq!(gaussian_reach(BLUR_STRONG_SIGMA), 10);
        // σ is clamped, so the halo can never be asked for the sky.
        assert!(gaussian_reach(1.0e9) <= (MAX_SIGMA * 3.0) as i32);
    }

    #[test]
    fn a_flat_field_survives_every_blur() {
        // Repeated integer box passes must not drift: a constant field is the
        // one case where the answer is knowable exactly.
        for f in [
            Filter::Smoothing,
            Filter::Blur,
            Filter::BlurStrong,
            Filter::Gaussian { sigma: 12.0 },
        ] {
            let mut r = Raster::new(40, 40);
            r.px.fill(0);
            for y in 0..40 {
                for x in 0..40 {
                    r.set_pixel(x, y, [1000, 2000, 3000, 32768]);
                }
            }
            let mut out = r.clone();
            f.run(&mut out, 0, 0);
            // Interior only: the buffer edge is the halo, where transparent
            // surround is the defined answer.
            let m = f.reach() as usize;
            for y in m..40 - m {
                for x in m..40 - m {
                    assert_eq!(
                        out.pixel(x, y),
                        [1000, 2000, 3000, 32768],
                        "{:?} drifted at {x},{y}",
                        f
                    );
                }
            }
        }
    }

    /// THE SEAM TEST. A shape centred exactly on a tile boundary must blur to
    /// a result that is symmetric about that boundary. Any per-tile kernel —
    /// or a halo one pixel too narrow — breaks the symmetry precisely there,
    /// which is the seam, and this asserts across the full blur radius on
    /// both sides of x = 64.
    #[test]
    fn gaussian_is_symmetric_across_a_tile_boundary() {
        let mut doc = Document::new(256, 256);
        // 40..88 is centred on x = 64, the tile-0/tile-1 seam.
        paint_rect(&mut doc, 40, 100, 88, 140);
        assert!(doc.apply_filter(Filter::Gaussian { sigma: 6.0 }));
        let reach = Filter::Gaussian { sigma: 6.0 }.reach();
        for d in 0..=reach + 4 {
            let l = px_at(&doc, 64 - 1 - d, 120);
            let r = px_at(&doc, 64 + d, 120);
            assert_eq!(l, r, "seam at x=64, distance {d}: {l:?} vs {r:?}");
        }
        // And it really did blur: the edge is a ramp now, not a cliff.
        assert!(alpha_at(&doc, 64, 120) > 30000, "centre still solid");
        assert!(
            alpha_at(&doc, 40, 120) < 30000 && alpha_at(&doc, 40, 120) > 0,
            "the old hard edge is a ramp"
        );
        assert!(alpha_at(&doc, 34, 120) > 0, "ink spread outside the shape");
    }

    /// The same content, moved by half a tile, must blur to the same picture.
    /// This is the independent check on the seam test: it needs no assumption
    /// about the kernel, only that the tile grid is invisible. A per-tile blur
    /// fails it because the two copies sit differently inside their tiles.
    #[test]
    fn blur_is_translation_invariant_across_the_tile_grid() {
        let shift = 32;
        let mut a = Document::new(256, 256);
        let mut b = Document::new(256, 256);
        paint_rect(&mut a, 100, 100, 141, 141);
        paint_rect(&mut b, 100 + shift, 100, 141 + shift, 141);
        let f = Filter::Gaussian { sigma: 5.0 };
        assert!(a.apply_filter(f));
        assert!(b.apply_filter(f));
        let m = f.reach() + 2;
        for y in 100 - m..141 + m {
            for x in 100 - m..141 + m {
                assert_eq!(
                    px_at(&a, x, y),
                    px_at(&b, x + shift, y),
                    "tile grid leaked at {x},{y}"
                );
            }
        }
    }

    /// Motion blur has the same neighbourhood problem at a different angle —
    /// check the seam there too, on a horizontal streak crossing x = 64.
    #[test]
    fn motion_blur_crosses_a_tile_boundary_cleanly() {
        let mut doc = Document::new(256, 256);
        paint_rect(&mut doc, 60, 100, 69, 109); // 9×9 block astride x=64
        let f = Filter::Motion {
            angle: 0.0,
            length: 30.0,
            dir: MotionDir::Both,
            mode: MotionMode::Uniform,
        };
        assert!(doc.apply_filter(f));
        // A horizontal smear: alpha decays away from the block along x and
        // falls monotonically, with no step at the x=64 tile edge.
        let row: Vec<u16> = (40..90).map(|x| alpha_at(&doc, x, 104)).collect();
        let peak = row.iter().copied().max().unwrap();
        assert!(peak > 0, "the streak exists");
        for w in row.windows(2).take(24) {
            assert!(w[1] >= w[0], "left half must rise monotonically: {row:?}");
        }
        // Vertically it did NOT spread: row 99 is above the block.
        assert_eq!(alpha_at(&doc, 64, 98), 0, "no vertical smear at angle 0");
        assert!(alpha_at(&doc, 80, 104) > 0, "smeared along +x past the block");
        assert!(alpha_at(&doc, 48, 104) > 0, "and along -x");
    }

    /// A mosaic cell straddling a tile edge must be ONE colour: this is the
    /// halo bug in its most visible form, since a broken cell shows as a hard
    /// vertical line every 64 px.
    #[test]
    fn mosaic_cells_do_not_break_at_tile_edges() {
        let mut doc = Document::new(256, 256);
        // A gradient-ish block so neighbouring cells differ; cell 20 does not
        // divide 64, so cells straddle the tile grid.
        doc.begin_op();
        for y in 0..128 {
            for x in 0..128 {
                let idx = TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                let v = (x * 256) as u16;
                doc.layers[0].tile_mut(idx).set_pixel(
                    (x - ox) as usize,
                    (y - oy) as usize,
                    [v, v, v, 32768],
                );
            }
        }
        doc.end_op();
        assert!(doc.apply_filter(Filter::Mosaic { cell: 20 }));
        // The cell covering x 60..80 spans the x=64 tile edge.
        let want = px_at(&doc, 60, 30);
        for x in 60..80 {
            assert_eq!(px_at(&doc, x, 30), want, "cell broken at x={x}");
        }
        // Neighbouring cells really are different (it is not one flat fill).
        assert_ne!(px_at(&doc, 45, 30), want);
    }

    #[test]
    fn selection_clips_the_write_and_leaves_the_rest_byte_identical() {
        let mut doc = Document::new(256, 256);
        paint_rect(&mut doc, 20, 20, 200, 200);
        let before: Vec<[u16; 4]> = (0..256).map(|x| px_at(&doc, x, 100)).collect();
        doc.selection = Some(crate::selection::Selection::from_rect(
            &doc, 0.0, 0.0, 100.0, 256.0,
        ));
        assert!(doc.apply_filter(Filter::Gaussian { sigma: 4.0 }));
        for x in 110..256 {
            assert_eq!(
                px_at(&doc, x, 100),
                before[x as usize],
                "outside the selection changed at x={x}"
            );
        }
        // Inside it did change — the shape's edge at x=20 is a ramp now.
        assert!(alpha_at(&doc, 18, 100) > 0, "blur ran inside the selection");
    }

    #[test]
    fn a_filter_is_exactly_one_undo_step() {
        let mut doc = Document::new(256, 256);
        paint_rect(&mut doc, 40, 40, 120, 120);
        let before: Vec<[u16; 4]> = (0..256).map(|x| px_at(&doc, x, 80)).collect();
        let depth = doc.undo_len();
        assert!(doc.apply_filter(Filter::BlurStrong));
        assert_eq!(doc.undo_len(), depth + 1, "one step, not one per tile");
        assert_eq!(doc.undo_labels().last().map(String::as_str), Some("Blur (strong)"));
        assert!(doc.undo());
        for x in 0..256 {
            assert_eq!(px_at(&doc, x, 80), before[x as usize], "undo missed x={x}");
        }
    }

    #[test]
    fn alpha_lock_keeps_the_silhouette() {
        let mut doc = Document::new(256, 256);
        paint_rect(&mut doc, 40, 40, 120, 120);
        doc.layers[0].lock_alpha = true;
        assert!(doc.apply_filter(Filter::BlurStrong));
        assert_eq!(alpha_at(&doc, 60, 60), 32768, "inside stayed opaque");
        assert_eq!(alpha_at(&doc, 130, 60), 0, "outside stayed empty");
    }

    #[test]
    fn refusals_push_nothing() {
        let mut doc = Document::new(128, 128);
        // Empty layer: nothing to filter.
        assert!(!doc.apply_filter(Filter::Blur));
        paint_rect(&mut doc, 10, 10, 60, 60);
        let depth = doc.undo_len();
        // Locked layer.
        doc.layers[0].lock = true;
        assert!(!doc.apply_filter(Filter::Blur));
        doc.layers[0].lock = false;
        // No-op parameters.
        assert!(!doc.apply_filter(Filter::Gaussian { sigma: 0.0 }));
        assert!(!doc.apply_filter(Filter::Mosaic { cell: 1 }));
        assert!(!doc.apply_filter(Filter::Motion {
            angle: 0.0,
            length: 0.0,
            dir: MotionDir::Both,
            mode: MotionMode::Uniform,
        }));
        // A selection that touches none of the layer.
        doc.selection = Some(crate::selection::Selection::from_rect(
            &doc, 100.0, 100.0, 128.0, 128.0,
        ));
        assert!(!doc.apply_filter(Filter::Blur));
        assert_eq!(doc.undo_len(), depth, "no empty undo steps were pushed");
    }

    #[test]
    fn smoothing_softens_a_step_without_moving_it() {
        let mut doc = Document::new(128, 128);
        paint_rect(&mut doc, 64, 0, 128, 128);
        assert!(doc.apply_filter(Filter::Smoothing));
        // One binomial pass: the step becomes a 3-px ramp, centred on the old
        // edge, and both sides mirror.
        assert_eq!(alpha_at(&doc, 62, 64), 0);
        assert!(alpha_at(&doc, 63, 64) > 0 && alpha_at(&doc, 63, 64) < 32768);
        assert!(alpha_at(&doc, 64, 64) > alpha_at(&doc, 63, 64));
        assert_eq!(alpha_at(&doc, 66, 64), 32768);
    }

    /// WHY PREMULTIPLIED IS THE RIGHT SPACE. An opaque red shape against real
    /// transparency: every pixel of the blurred ramp must still be PURE red,
    /// which in premultiplied fix15 is exactly `r == a`. Blurring
    /// un-premultiplied colour instead has to invent a colour for the
    /// transparent side — usually black — and the ramp darkens as it fades,
    /// which is the black-halo artefact this pins against.
    #[test]
    fn premultiplied_blur_keeps_the_fading_edge_pure() {
        let mut doc = Document::new(256, 256);
        doc.begin_op();
        for y in 40..140 {
            for x in 40..140 {
                let idx = TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                doc.layers[0].tile_mut(idx).set_pixel(
                    (x - ox) as usize,
                    (y - oy) as usize,
                    [32768, 0, 0, 32768],
                );
            }
        }
        doc.end_op();
        assert!(doc.apply_filter(Filter::BlurStrong));
        let mut saw_ramp = false;
        for x in 28..52 {
            let p = px_at(&doc, x, 90);
            assert_eq!(p[0], p[3], "the edge drifted off pure red at x={x}: {p:?}");
            assert_eq!(p[1], 0, "green appeared at x={x}");
            assert_eq!(p[2], 0, "blue appeared at x={x}");
            if p[3] > 0 && p[3] < 32768 {
                saw_ramp = true;
            }
        }
        assert!(saw_ramp, "there was a partial-coverage ramp to check");
    }

    #[test]
    fn the_halo_is_wide_enough_for_the_whole_kernel() {
        // Directly: a lone dot must blur to a profile that decays smoothly to
        // zero on BOTH sides even when it sits one pixel from a tile edge.
        let mut doc = Document::new(256, 256);
        paint_rect(&mut doc, 63, 63, 65, 65); // 2×2 straddling (64,64)
        let f = Filter::Gaussian { sigma: 8.0 };
        assert!(doc.apply_filter(f));
        let reach = f.reach();
        for d in 0..=reach {
            assert_eq!(
                alpha_at(&doc, 64 - 1 - d, 64),
                alpha_at(&doc, 64 + d, 64),
                "asymmetric at distance {d} on x"
            );
            assert_eq!(
                alpha_at(&doc, 64, 64 - 1 - d),
                alpha_at(&doc, 64, 64 + d),
                "asymmetric at distance {d} on y"
            );
        }
    }
}
