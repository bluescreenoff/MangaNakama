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

/// A kernel the caller lends [`Document::apply_filter_with`]: filter the
/// gathered buffer in place and return `true`, or return `false` to let the
/// CPU reference run. Declining is always legal — the GPU seam declines
/// whenever the adapter, the size floor or its dispatch canary says so.
pub type RasterKernel<'a> = dyn FnMut(Filter, &mut Raster) -> bool + 'a;

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

/// Which way a wave displaces (CSP's Direction row on 波形).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum WaveDir {
    /// Rows slide left/right — the wave runs down the page.
    #[default]
    Horizontal,
    /// Columns slide up/down — the wave runs across the page.
    Vertical,
}

impl WaveDir {
    pub fn label(self) -> &'static str {
        match self {
            WaveDir::Horizontal => "Horizontal",
            WaveDir::Vertical => "Vertical",
        }
    }
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
    /// FL-016 Radial (zoom) blur: each pixel averages samples along its
    /// ray from the buffer's centre, pulled inward by `strength` of the
    /// way (0..=0.95) — the classic zoom smear. The centre is the
    /// buffer's own centre (the selection's bounds centre on every
    /// caller today); a pickable centre waits on a filter preview pane.
    RadialBlur { strength: f32 },
    /// FL-017 Spin blur: each pixel averages samples along the arc of
    /// ±`angle_deg` about the buffer's centre — the rotational smear of
    /// a spinning subject.
    SpinBlur { angle_deg: f32 },
    /// FL-014 Unsharp mask: `out = orig + (orig − blur)·amount`, the blur
    /// being the same three-box Gaussian at σ = `radius`.
    Unsharp { radius: f32, amount: f32 },
    /// FL-020 Pinch: radial squeeze about the buffer's centre. Positive
    /// `amount` (0..1) drags content inward; negative bulges it out, which
    /// is the fish-eye and needs no arm of its own.
    Pinch { amount: f32 },
    /// FL-021 Ripple: concentric rings — the sample radius wobbles by
    /// `amplitude` px every `wavelength` px of radius.
    Ripple { amplitude: f32, wavelength: f32 },
    /// FL-022 Wave: one sine shear, `amplitude` px every `wavelength` px
    /// along the axis `dir` does NOT displace.
    Wave {
        amplitude: f32,
        wavelength: f32,
        dir: WaveDir,
    },
    /// FL-023 Twirl: rotate about the buffer's centre by `angle_deg`,
    /// falling linearly to nothing at the rim.
    Twirl { angle_deg: f32 },
    /// LC-001 Remove dust (CSP ゴミ取り): clear every connected speck of ink
    /// of `max_px` pixels or fewer. The unit is AREA — the count of pixels
    /// in the blob, not its width.
    RemoveDust { max_px: u32 },
    /// LC-002 Adjust line width (CSP 線幅修正): thicken the ink by `delta`
    /// pixels, or thin it when `delta` is negative.
    LineWidth { delta: i32 },
    /// Row 160 / `RD-001`–`RD-003` — the **Remove dust TOOL**'s scrub, in
    /// all four of CSP's senses of the word ([`crate::dust::DustMode`]).
    /// It rides the filter framework because everything the tool needs is
    /// already here: the tile gather with its halo, the selection window,
    /// the layer guards and the one-press undo. `color` is the current
    /// drawing colour, which only the "fill gaps" half reads.
    ///
    /// Not in the Filter menu — the menu's dust entry is [`Self::RemoveDust`]
    /// (LC-001), which is this filter's `OnTransparency` mode with no drag
    /// and no mode row. The two share [`dust_max`] so the thresholds mean
    /// the same thing in both places.
    Dust {
        max_px: u32,
        mode: crate::dust::DustMode,
        color: [f32; 3],
    },
}

impl Filter {
    /// Menu SEEDS — the values a parameterised filter's dialog opens on.
    ///
    /// One definition each, because there are three doors onto the same
    /// dialog (the Filter menu in `ui::top`, the command palette in
    /// `ui::quick`, and the palette's own test) and every one of them used
    /// to spell the numbers out. Three hand-kept copies of "Ripple starts
    /// at 8 px every 48" is a drift waiting to happen: change the menu and
    /// Ctrl+K quietly opens a different dialog. `Adjust`'s own menu-default
    /// block (`adjust.rs`) solved this for the Correction menu already;
    /// this is the same block for the Filter one.
    pub const GAUSSIAN: Self = Self::Gaussian { sigma: 4.0 };
    pub const MOTION: Self = Self::Motion {
        angle: 0.0,
        length: 20.0,
        dir: MotionDir::Both,
        mode: MotionMode::Uniform,
    };
    pub const RADIAL_BLUR: Self = Self::RadialBlur { strength: 0.3 };
    pub const SPIN_BLUR: Self = Self::SpinBlur { angle_deg: 20.0 };
    pub const UNSHARP: Self = Self::Unsharp {
        radius: 2.0,
        amount: 1.0,
    };
    pub const PINCH: Self = Self::Pinch { amount: 0.4 };
    pub const RIPPLE: Self = Self::Ripple {
        amplitude: 8.0,
        wavelength: 48.0,
    };
    pub const WAVE: Self = Self::Wave {
        amplitude: 8.0,
        wavelength: 48.0,
        dir: WaveDir::Horizontal,
    };
    pub const TWIRL: Self = Self::Twirl { angle_deg: 90.0 };
    pub const LINE_WIDTH: Self = Self::LineWidth { delta: 1 };
    /// `LC-001`. The same 5 px the dust TOOL starts at (`DustOpts::default`,
    /// `cmd.rs`) — CSP ships ゴミ取り small in both places.
    pub const REMOVE_DUST: Self = Self::RemoveDust { max_px: 5 };
    pub const MOSAIC: Self = Self::Mosaic { cell: 8 };

    /// Undo-stack label, and the dialog title.
    pub fn label(self) -> &'static str {
        match self {
            Filter::Smoothing => "Smoothing",
            Filter::Blur => "Blur",
            Filter::BlurStrong => "Blur (strong)",
            Filter::Gaussian { .. } => "Gaussian blur",
            Filter::Motion { .. } => "Motion blur",
            Filter::Mosaic { .. } => "Mosaic",
            Filter::RadialBlur { .. } => "Radial blur",
            Filter::SpinBlur { .. } => "Spin blur",
            Filter::Unsharp { .. } => "Unsharp mask",
            Filter::Pinch { .. } => "Pinch",
            Filter::Ripple { .. } => "Ripple",
            Filter::Wave { .. } => "Wave",
            Filter::Twirl { .. } => "Twirl",
            Filter::RemoveDust { .. } => "Remove dust",
            Filter::LineWidth { .. } => "Adjust line width",
            // The undo label is the MODE: "undo Fill gaps with surrounding
            // colour" is what the artist thinks they just did.
            Filter::Dust { mode, .. } => mode.label(),
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
            // Both smears sample only WITHIN the lifted region (the inward
            // ray and the arc stay inside its bounds), so the region needs
            // no padding for them.
            Filter::RadialBlur { .. } | Filter::SpinBlur { .. } => 0,
            // Its one neighbourhood read is the blur it subtracts.
            Filter::Unsharp { radius, .. } => gaussian_reach(radius),
            // Both radial warps are confined to the buffer's INSCRIBED
            // circle by construction ([`radial_frame`]), so like the two
            // smears above they need no padding either way.
            Filter::Pinch { .. } | Filter::Twirl { .. } => 0,
            // A sine shear moves ink by at most its amplitude, in both
            // directions; +1 for the bilinear tap straddling the far sample.
            Filter::Ripple { amplitude, .. } | Filter::Wave { amplitude, .. } => {
                wave_amplitude(amplitude).abs().ceil() as i32 + 1
            }
            // The halo is what makes the speck count TRUE rather than
            // truncated. A blob with a pixel in the write region and a pixel
            // on the buffer's rim spans at least this far, so it holds at
            // least this many pixels — i.e. it is already too big to be dust,
            // and cutting it off at the rim cannot change the verdict.
            Filter::RemoveDust { max_px } => dust_max(max_px) as i32,
            // Same argument, and `dust`'s rim rule leans on it: a component
            // holding a writable pixel and a rim pixel spans at least this
            // far, so it already holds more than `max_px` pixels.
            Filter::Dust { max_px, .. } => dust_max(max_px) as i32,
            // Morphology reads (and grow writes) exactly its own radius out.
            Filter::LineWidth { delta } => line_width_radius(delta) as i32,
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
            Filter::RadialBlur { strength } => radial_blur(buf, strength),
            Filter::SpinBlur { angle_deg } => spin_blur(buf, angle_deg),
            Filter::Unsharp { radius, amount } => unsharp(buf, radius, amount),
            Filter::Pinch { amount } => pinch(buf, amount),
            Filter::Ripple {
                amplitude,
                wavelength,
            } => ripple(buf, amplitude, wavelength),
            Filter::Wave {
                amplitude,
                wavelength,
                dir,
            } => wave(buf, amplitude, wavelength, dir),
            Filter::Twirl { angle_deg } => twirl(buf, angle_deg),
            Filter::RemoveDust { max_px } => remove_dust(buf, max_px),
            Filter::LineWidth { delta } => line_width(buf, delta),
            Filter::Dust {
                max_px,
                mode,
                color,
            } => crate::dust::scrub(buf, mode, max_px, color),
        }
    }

    /// True when the parameters make this a no-op, so the caller can refuse
    /// instead of pushing an empty undo step.
    pub fn is_identity(self) -> bool {
        match self {
            Filter::Gaussian { sigma } => box_radii(sigma).iter().all(|&r| r == 0),
            Filter::Motion { length, .. } => !(length > 0.5),
            Filter::Mosaic { cell } => cell <= 1,
            Filter::RadialBlur { strength } => !(strength > 0.02),
            Filter::SpinBlur { angle_deg } => !(angle_deg >= 0.5),
            Filter::Unsharp { radius, amount } => {
                !(amount > 0.01) || box_radii(radius).iter().all(|&r| r == 0)
            }
            Filter::Pinch { amount } => !(amount.abs() > 0.01),
            Filter::Twirl { angle_deg } => !(angle_deg.abs() >= 0.5),
            Filter::Ripple {
                amplitude,
                wavelength,
            }
            | Filter::Wave {
                amplitude,
                wavelength,
                ..
            } => !(amplitude.abs() > 0.25) || !(wavelength >= 1.0),
            Filter::LineWidth { delta } => line_width_radius(delta) == 0,
            _ => false,
        }
    }

    /// The chain of integer symmetric passes this filter is, when it is a
    /// separable convolution: every pass along x in order, then every pass
    /// along y in order — exactly [`gaussian`]'s and [`smoothing`]'s shape.
    /// `None` for everything else.
    ///
    /// This is the blur family's ticket through the GPU kernel seam
    /// (`mn_gpu::Kernel::Separable`), and it is deliberately built from the
    /// same [`box_radii`] the CPU passes use, so the two can only disagree by
    /// arithmetic — and, because [`BoxPass`] carries the *integer* weights and
    /// denominator, not even by that.
    ///
    /// **Why the chain and not one composite kernel.** The obvious GPU move
    /// is to convolve the three boxes into one wide kernel and run a single
    /// gather per axis: zero-padded convolution composes, so on paper it is
    /// the same operator for the same tap count. It is not, and the parity
    /// test caught it at 4015/32768. Each CPU pass re-zero-pads its own
    /// *output*, throwing away the ink the previous pass pushed past the
    /// buffer edge, so the three-pass result differs from the composite one
    /// within `3 × reach` of every border. The chain reproduces that
    /// truncation because it *is* the truncation, it costs the same taps (the
    /// composite kernel is exactly as wide as the three boxes together), and
    /// it makes the GPU result bit-identical rather than merely close.
    pub fn separable_passes(self) -> Option<Vec<BoxPass>> {
        let radii = match self {
            // The 3×3 binomial: `tent_h` sums `(l + 2m + r + 2) / 4`.
            Filter::Smoothing => {
                return Some(vec![BoxPass {
                    half: vec![2, 1],
                    denom: 4,
                }]);
            }
            Filter::Blur => box_radii(BLUR_SIGMA),
            Filter::BlurStrong => box_radii(BLUR_STRONG_SIGMA),
            Filter::Gaussian { sigma } => box_radii(sigma),
            _ => return None,
        };
        let passes: Vec<BoxPass> = radii
            .iter()
            .filter(|&&r| r > 0)
            .map(|&r| BoxPass {
                half: vec![1; r + 1],
                denom: (2 * r + 1) as u32,
            })
            .collect();
        (!passes.is_empty()).then_some(passes)
    }
}

/// One integer symmetric convolution pass, as [`box_h`] and [`tent_h`]
/// compute it: `half[0]` is the centre weight, `half[k]` the weight at ±k,
/// samples outside the buffer count as transparent (and do **not** reduce
/// `denom`), and each output channel is `(Σ w·s + denom/2) / denom` in u32.
///
/// Spelled in integers on purpose. A float kernel would have made GPU parity
/// a tolerance argument; this way the two paths produce the same bits, and
/// the parity test asserts equality rather than closeness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxPass {
    pub half: Vec<u32>,
    pub denom: u32,
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
                out[c] = ((l[c] as u32 + 2 * m[c] as u32 + r[c] as u32 + 2) / 4)
                    .min(u16::MAX as u32) as u16;
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
                out[c] = ((u[c] as u32 + 2 * m[c] as u32 + d[c] as u32 + 2) / 4)
                    .min(u16::MAX as u32) as u16;
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

/// FL-016: the zoom smear — dest pixel p averages the segment of its own
/// ray from `p · (1−k)` to `p` (k = strength), uniformly weighted over
/// taps that scale with the smear. Premultiplied averaging, exactly the
/// motion blur's arithmetic walked radially instead of linearly.
fn radial_blur(buf: &mut Raster, strength: f32) {
    let k = strength.clamp(0.0, 0.95);
    if k <= 0.02 {
        return;
    }
    let n = ((k * buf.w.min(buf.h) as f32).ceil() as usize).clamp(8, 48);
    let c = [(buf.w as f32 - 1.0) * 0.5, (buf.h as f32 - 1.0) * 0.5];
    let mut src = Raster::new(buf.w, buf.h);
    std::mem::swap(buf, &mut src);
    for y in 0..src.h {
        for x in 0..src.w {
            let (ux, uy) = (x as f32 - c[0], y as f32 - c[1]);
            let mut acc = [0f32; TILE_CHANNELS];
            for i in 0..n {
                let t = 1.0 - k * (i as f32) / ((n - 1) as f32);
                let p = sample_bilinear(&src, c[0] + ux * t, c[1] + uy * t);
                for (a, v) in acc.iter_mut().zip(p) {
                    *a += v;
                }
            }
            let mut out = [0u16; TILE_CHANNELS];
            for (o, a) in out.iter_mut().zip(acc) {
                *o = (a / n as f32 + 0.5).clamp(0.0, u16::MAX as f32) as u16;
            }
            buf.set_pixel(x, y, out);
        }
    }
}

/// FL-017: the rotational smear — dest pixel p averages the arc of ±a
/// about the centre at its own radius. Near the centre the arc is short
/// (the samples collapse onto p), which is physically right: a spin
/// blurs the rim far more than the axle.
fn spin_blur(buf: &mut Raster, angle_deg: f32) {
    let a = angle_deg.clamp(0.5, 180.0).to_radians();
    let c = [(buf.w as f32 - 1.0) * 0.5, (buf.h as f32 - 1.0) * 0.5];
    let max_r = (c[0].max(c[1])) as f32;
    let n = ((a * max_r).ceil() as usize).clamp(8, 48);
    let mut src = Raster::new(buf.w, buf.h);
    std::mem::swap(buf, &mut src);
    for y in 0..src.h {
        for x in 0..src.w {
            let (ux, uy) = (x as f32 - c[0], y as f32 - c[1]);
            let r = ux.hypot(uy);
            let th0 = uy.atan2(ux);
            let mut acc = [0f32; TILE_CHANNELS];
            for i in 0..n {
                let t = (i as f32) / ((n - 1) as f32) * 2.0 - 1.0;
                let th = th0 + a * t;
                let p = sample_bilinear(&src, c[0] + r * th.cos(), c[1] + r * th.sin());
                for (acc_v, v) in acc.iter_mut().zip(p) {
                    *acc_v += v;
                }
            }
            let mut out = [0u16; TILE_CHANNELS];
            for (o, av) in out.iter_mut().zip(acc) {
                *o = (av / n as f32 + 0.5).clamp(0.0, u16::MAX as f32) as u16;
            }
            buf.set_pixel(x, y, out);
        }
    }
}

/// FL-014: the classic unsharp mask — `out = orig + (orig − blur)·amount`,
/// the blur being the same Kovesi three-box Gaussian the blur family runs. All
/// the sharpening is in the sign: the difference is large only where the blur
/// disagrees with the original, which is exactly at an edge, so a flat field
/// comes out untouched and an edge gains the overshoot on both sides that
/// reads as "crisper".
///
/// Sharpened premultiplied, then REPAIRED. Colour and alpha overshoot
/// independently, and an overshot colour channel can land above the alpha it
/// is premultiplied by, which is not a representable pixel — so alpha is
/// computed first and clamps the three colour channels. That is the right
/// answer visually too: an over-sharpened edge should saturate, not glow.
fn unsharp(buf: &mut Raster, radius: f32, amount: f32) {
    let amount = amount.clamp(0.0, 10.0);
    if amount <= 0.01 || box_radii(radius).iter().all(|&r| r == 0) {
        return;
    }
    let orig = buf.clone();
    gaussian(buf, radius);
    for i in (0..buf.px.len()).step_by(TILE_CHANNELS) {
        let mut out = [0u16; TILE_CHANNELS];
        // Alpha (channel 3) first — it is the ceiling for the other three.
        for c in (0..TILE_CHANNELS).rev() {
            let o = orig.px[i + c] as f32;
            let v = o + (o - buf.px[i + c] as f32) * amount;
            let hi = if c == 3 {
                crate::blend::FIX15_ONE_F
            } else {
                out[3] as f32
            };
            out[c] = (v + 0.5).clamp(0.0, hi) as u16;
        }
        buf.px[i..i + TILE_CHANNELS].copy_from_slice(&out);
    }
}

// --------------------------------------------------------------- distort --
//
// FL-020..023, the CSP Filter ▸ Distort family. All four are the SAME op with
// a different two lines in the middle: for every destination pixel, work out
// where its colour comes from and take one bilinear tap there — the INVERSE
// map, the idiom `liquify.rs` warps with. Forward-mapping instead (push each
// source pixel to where it lands) leaves holes wherever the map stretches, and
// no amount of splatting fixes that; the inverse map cannot leave a hole
// because it fills every destination exactly once.
//
// Sampling is premultiplied fix15, for the same reason the blur family
// averages there: a bilinear tap IS a weighted average, and averaging
// un-premultiplied colour drags a transparent neighbour's arbitrary colour
// into a soft edge.

/// Run one inverse map over `buf` in place. `inverse` answers, for a
/// destination pixel, the SOURCE coordinate its colour comes from.
fn warp(buf: &mut Raster, inverse: impl Fn(f32, f32) -> (f32, f32)) {
    let mut src = Raster::new(buf.w, buf.h);
    std::mem::swap(buf, &mut src);
    for y in 0..src.h {
        for x in 0..src.w {
            let (sx, sy) = inverse(x as f32, y as f32);
            let p = sample_bilinear(&src, sx, sy);
            let mut out = [0u16; TILE_CHANNELS];
            for (o, v) in out.iter_mut().zip(p) {
                *o = (v + 0.5).clamp(0.0, u16::MAX as f32) as u16;
            }
            buf.set_pixel(x, y, out);
        }
    }
}

/// Centre and working radius of a buffer — the frame the two radial warps
/// live in, as `(cx, cy, radius)`.
///
/// The radius is the INSCRIBED circle's, not the half-diagonal's, and that is
/// load-bearing: a map that never sends a sample outside the inscribed circle
/// never reads the buffer's transparent surround, which is why Pinch and Twirl
/// can honestly declare a [`Filter::reach`] of zero. Outside the circle both
/// warps are the identity, so the corners of a selection come through
/// untouched rather than smeared against the marquee.
///
/// The centre is the buffer's own — the selection's bounds centre on every
/// caller today, exactly as for radial and spin blur. A draggable centre
/// handle is the same missing interaction round for all four.
fn radial_frame(buf: &Raster) -> (f32, f32, f32) {
    let cx = (buf.w as f32 - 1.0) * 0.5;
    let cy = (buf.h as f32 - 1.0) * 0.5;
    (cx, cy, cx.min(cy).max(1.0))
}

/// The amplitude a sine warp is allowed, shared by [`Filter::reach`] and the
/// kernels so the halo and the taps cannot disagree.
fn wave_amplitude(amplitude: f32) -> f32 {
    amplitude.clamp(-1024.0, 1024.0)
}

/// FL-020: `r_src = R·(r/R)^(1−a)`. For `a > 0` the exponent is below one, so
/// the source radius is the LARGER — each destination ring pulls in content
/// from further out and the picture contracts toward the centre, which is the
/// pinch. Negative `a` runs it the other way and is the bulge/fish-eye; that
/// is why there is no separate Fish-eye arm. The exponent stays positive, so
/// `r_src ≤ R` always and no tap leaves the inscribed circle.
fn pinch(buf: &mut Raster, amount: f32) {
    let a = amount.clamp(-0.95, 0.95);
    let (cx, cy, rad) = radial_frame(buf);
    warp(buf, |x, y| {
        let (ux, uy) = (x - cx, y - cy);
        let r = ux.hypot(uy);
        if r <= 0.0 || r >= rad {
            return (x, y);
        }
        // (r_src / r), so the ray direction comes along for free.
        let k = (r / rad).powf(1.0 - a) * rad / r;
        (cx + ux * k, cy + uy * k)
    });
}

/// FL-021: the sample radius wobbles — `r_src = r + A·sin(2πr/λ)`. Purely
/// radial, so ink never leaves the ray it started on; the rings are what a
/// drop in water does to a reflection.
fn ripple(buf: &mut Raster, amplitude: f32, wavelength: f32) {
    let amp = wave_amplitude(amplitude);
    let lam = wavelength.max(1.0);
    let (cx, cy, _) = radial_frame(buf);
    warp(buf, |x, y| {
        let (ux, uy) = (x - cx, y - cy);
        let r = ux.hypot(uy);
        if r <= 0.0 {
            return (x, y);
        }
        let rs = (r + amp * (std::f32::consts::TAU * r / lam).sin()).max(0.0);
        (cx + ux * rs / r, cy + uy * rs / r)
    });
}

/// FL-022: one sine shear. Horizontal slides each ROW sideways by
/// `A·sin(2πy/λ)`; vertical does the transpose. Nothing moves along the axis
/// the wave runs down, so straight lines parallel to it stay exactly as long
/// as they were.
fn wave(buf: &mut Raster, amplitude: f32, wavelength: f32, dir: WaveDir) {
    let amp = wave_amplitude(amplitude);
    let phase = std::f32::consts::TAU / wavelength.max(1.0);
    warp(buf, |x, y| match dir {
        WaveDir::Horizontal => (x + amp * (phase * y).sin(), y),
        WaveDir::Vertical => (x, y + amp * (phase * x).sin()),
    });
}

/// FL-023: rotate about the centre by `angle_deg`, the turn falling linearly
/// to zero at the rim so the warp blends into the untouched surround instead
/// of tearing against it. Radius is preserved exactly, so — like pinch — no
/// tap escapes the inscribed circle.
fn twirl(buf: &mut Raster, angle_deg: f32) {
    let a = angle_deg.clamp(-1440.0, 1440.0).to_radians();
    let (cx, cy, rad) = radial_frame(buf);
    warp(buf, |x, y| {
        let (ux, uy) = (x - cx, y - cy);
        let r = ux.hypot(uy);
        if r >= rad {
            return (x, y);
        }
        let th = uy.atan2(ux) - a * (1.0 - r / rad);
        (cx + r * th.cos(), cy + r * th.sin())
    });
}

// -------------------------------------------------------- line correction --

/// The speck size a dust removal is allowed to look for, in pixels of area.
/// Shared by [`Filter::reach`] and [`remove_dust`] so the halo and the count
/// can never disagree; the ceiling bounds the halo the same way `MAX_SIGMA`
/// bounds the blur's.
pub(crate) fn dust_max(max_px: u32) -> u32 {
    max_px.clamp(1, 256)
}

/// LC-001: clear every 8-connected blob of `max_px` pixels or fewer.
///
/// 8-connected, not 4-: a scanner speck is as often a diagonal pair as a
/// square one, and under 4-connectivity a four-pixel diagonal reads as four
/// separate one-pixel specks — which would delete a chain the eye sees as one
/// mark, at a threshold the user set to keep it.
///
/// Anything with ink at all counts, at any alpha: dust is usually the faint
/// grey the scanner invented, and thresholding would keep exactly the specks
/// worth removing. The flood is iterative (a page of ink is millions of
/// pixels deep for a recursive one) and stops RECORDING a blob's pixels once
/// it is too big to clear, so the scratch stays bounded by `max_px` rather
/// than by the largest connected drawing on the layer.
fn remove_dust(buf: &mut Raster, max_px: u32) {
    let max = dust_max(max_px) as usize;
    let (w, h) = (buf.w, buf.h);
    if w == 0 || h == 0 {
        return;
    }
    let inked = |b: &Raster, p: usize| b.px[p * TILE_CHANNELS + 3] != 0;
    let mut seen = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let mut speck: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if seen[start] || !inked(buf, start) {
            continue;
        }
        seen[start] = true;
        stack.clear();
        speck.clear();
        stack.push(start);
        let mut count = 0usize;
        while let Some(p) = stack.pop() {
            count += 1;
            if count <= max {
                speck.push(p);
            }
            let (x, y) = ((p % w) as i32, (p / w) as i32);
            for ny in (y - 1).max(0)..=(y + 1).min(h as i32 - 1) {
                for nx in (x - 1).max(0)..=(x + 1).min(w as i32 - 1) {
                    let q = ny as usize * w + nx as usize;
                    if seen[q] || !inked(buf, q) {
                        continue;
                    }
                    seen[q] = true;
                    stack.push(q);
                }
            }
        }
        if count <= max {
            for &p in &speck {
                buf.px[p * TILE_CHANNELS..(p + 1) * TILE_CHANNELS].fill(0);
            }
        }
    }
}

/// How far a line-width adjustment reaches, in pixels. Shared by
/// [`Filter::reach`], [`Filter::is_identity`] and [`line_width`].
fn line_width_radius(delta: i32) -> usize {
    delta.clamp(-64, 64).unsigned_abs() as usize
}

/// One separable pass of a square-ball greyscale morphology along one axis.
///
/// Square ball = Chebyshev ball, and a Chebyshev ball is SEPARABLE — one
/// horizontal pass then one vertical one, so any radius costs two passes
/// instead of `r` rounds of a 3×3. That is `Selection::grow`'s trick; what is
/// different here is the operator. `grow` runs on a boolean mask and can
/// answer its windows from a prefix sum, and this cannot: thresholding the
/// alpha to a mask would throw away the anti-aliasing on every line in the
/// drawing, which for a tool whose whole job is line quality is the one
/// unacceptable outcome. So the window extremum comes from a monotonic deque
/// instead — every index enters and leaves once, still O(1) per pixel.
///
/// The winner's WHOLE premultiplied pixel travels, not just its alpha:
/// thickening a line has to bring the line's colour out with it, and thinning
/// one has to leave the thinned edge the colour it was, not black. Ties keep
/// the centre pixel, which is what stops a flat region of two colours — where
/// every alpha is equal — from swapping one for the other.
fn morph_pass(src: &Raster, dst: &mut Raster, r: usize, vertical: bool, grow: bool) {
    let (outer, inner) = if vertical { (src.w, src.h) } else { (src.h, src.w) };
    let mut dq: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for o in 0..outer {
        let at = |i: usize| {
            if vertical {
                src.pixel(o, i)
            } else {
                src.pixel(i, o)
            }
        };
        dq.clear();
        let mut next = 0usize;
        for i in 0..inner {
            // Admit everything the window at `i` newly covers…
            let hi = (i + r).min(inner - 1);
            while next <= hi {
                let a = at(next)[3];
                while dq
                    .back()
                    .is_some_and(|&b| if grow { at(b)[3] <= a } else { at(b)[3] >= a })
                {
                    dq.pop_back();
                }
                dq.push_back(next);
                next += 1;
            }
            // …and retire what it has left behind. The front is the extremum.
            while dq.front().is_some_and(|&f| f + r < i) {
                dq.pop_front();
            }
            let own = at(i);
            let win = at(*dq.front().expect("the window always holds `i`"));
            let take = if grow {
                win[3] > own[3]
            } else {
                win[3] < own[3]
            };
            let p = if take { win } else { own };
            if vertical {
                dst.set_pixel(o, i, p);
            } else {
                dst.set_pixel(i, o, p);
            }
        }
    }
}

/// LC-002: thicken (`delta > 0`) or thin (`delta < 0`) the ink by `delta`
/// pixels — a signed square-ball dilation of the coverage, run as two
/// [`morph_pass`]es.
fn line_width(buf: &mut Raster, delta: i32) {
    let r = line_width_radius(delta);
    if r == 0 {
        return;
    }
    let grow = delta > 0;
    let mut tmp = Raster::new(buf.w, buf.h);
    morph_pass(buf, &mut tmp, r, false, grow);
    std::mem::swap(buf, &mut tmp);
    morph_pass(buf, &mut tmp, r, true, grow);
    std::mem::swap(buf, &mut tmp);
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
pub(crate) fn gather(layer: &Layer, gx: i32, gy: i32, gw: usize, gh: usize) -> Raster {
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

/// CSP "Outline selection" ▸ Border type: which side of the selection
/// edge the band is drawn on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineBorder {
    Outside,
    OnBorder,
    Inside,
}

impl OutlineBorder {
    pub fn label(&self) -> &'static str {
        match self {
            OutlineBorder::Outside => "Draw outside",
            OutlineBorder::OnBorder => "Draw on border",
            OutlineBorder::Inside => "Draw inside",
        }
    }
}

impl Document {
    /// CSP "Convert to drawing color": recolour the active layer's ink to
    /// `colour`, keeping every pixel's coverage — premultiplied, so soft
    /// edges stay exactly as soft. Selection-bounded when ants are up.
    /// Refuses (with the reason) when the layer is not plain raster, or
    /// the layer's expression is Grey/Mono and the picked colour is pure
    /// black or pure white (CSP's rule: that conversion is a no-op that
    /// would flatten the tone structure).
    pub fn convert_to_drawing_colour(&mut self, colour: [f32; 3]) -> String {
        let li = self.active;
        let Some(l) = self.layers.get(li) else {
            return String::new();
        };
        if l.folder || l.strokes.is_some() || !matches!(l.kind, crate::doc::LayerKind::Raster) {
            return "convert to drawing colour applies to raster layers".into();
        }
        let pure_bw = {
            let (mx, mn) = (
                colour.iter().cloned().fold(f32::MIN, f32::max),
                colour.iter().cloned().fold(f32::MAX, f32::min),
            );
            mx - mn < 1.0 / 255.0 && (mx <= 1.0 / 255.0 || mx >= 254.0 / 255.0)
        };
        if pure_bw && !matches!(l.expression, crate::doc::LayerExpression::Colour) {
            return "pick a colour first — black or white on a grey/mono layer has nothing to convert".into();
        }
        let sel = self.selection.clone();
        let (li_w, li_h) = (self.size.0 as i32, self.size.1 as i32);
        let mut touched = 0usize;
        self.begin_op();
        for idx in self.layers[li].tiles().map(|(i, _)| i).collect::<Vec<_>>() {
            let (ox, oy) = idx.origin();
            for py in 0..crate::tile::TILE_SIZE {
                for px in 0..crate::tile::TILE_SIZE {
                    let (x, y) = (ox + px as i32, oy + py as i32);
                    if x < 0 || y < 0 || x >= li_w || y >= li_h {
                        continue;
                    }
                    let t = self.layers[li].tile_mut(idx);
                    let o = (py * crate::tile::TILE_SIZE + px) * 4;
                    let d = t.data_mut();
                    let a = d[o + 3];
                    if a == 0 {
                        continue;
                    }
                    if sel
                        .as_ref()
                        .is_some_and(|s| s.coverage(x, y) < 128)
                    {
                        continue;
                    }
                    let af = a as f32 / crate::blend::FIX15_ONE_F;
                    d[o] = crate::blend::f32_to_fix15(colour[0].clamp(0.0, 1.0) * af);
                    d[o + 1] = crate::blend::f32_to_fix15(colour[1].clamp(0.0, 1.0) * af);
                    d[o + 2] = crate::blend::f32_to_fix15(colour[2].clamp(0.0, 1.0) * af);
                    touched += 1;
                }
            }
        }
        self.end_op();
        if touched == 0 {
            return "nothing to convert — the layer has no ink".into();
        }
        self.set_op_label("Convert colour");
        format!("recoloured {touched} px to the drawing colour")
    }

    /// CSP "Outline selection": stroke a border around the selection on
    /// the active raster layer, in `colour`. `border` picks which side of
    /// the ants the band sits (outside / centred / inside); `round`
    /// swaps the square structuring element for a disc. Anti-aliasing is
    /// vector-only in CSP and absent here on purpose.
    pub fn outline_selection(
        &mut self,
        width_px: f32,
        border: OutlineBorder,
        round: bool,
        colour: [f32; 3],
    ) -> String {
        let Some(sel) = self.selection.clone() else {
            return "outline needs a selection first".into();
        };
        let li = self.active;
        let Some(l) = self.layers.get(li) else {
            return String::new();
        };
        if l.lock {
            return "layer is locked".into();
        }
        if l.folder || l.strokes.is_some() || !matches!(l.kind, crate::doc::LayerKind::Raster) {
            return "outline applies to raster layers".into();
        }
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let inside = |x: i32, y: i32| -> bool {
            x >= 0 && y >= 0 && x < w && y < h && sel.coverage(x, y) >= 128
        };
        // The half-window the border type needs: outside/inside = the
        // full width from the boundary; on-border = half either way.
        let r_full = (width_px.max(0.5) as i32).max(1);
        let r_half = (r_full + 1) / 2;
        let near = |x: i32, y: i32, r: i32, want_inside: bool| -> bool {
            for dy in -r..=r {
                for dx in -r..=r {
                    if round && dx * dx + dy * dy > r * r {
                        continue;
                    }
                    if inside(x + dx, y + dy) == want_inside {
                        return true;
                    }
                }
            }
            false
        };
        let is_ring = |x: i32, y: i32| -> bool {
            match border {
                OutlineBorder::Outside => !inside(x, y) && near(x, y, r_full, true),
                OutlineBorder::OnBorder => {
                    near(x, y, r_half, true) && near(x, y, r_half, false)
                }
                OutlineBorder::Inside => inside(x, y) && near(x, y, r_full, false),
            }
        };
        // Only scan the selection's bounds grown by the width.
        let Some(b) = sel.bounds() else {
            return "outline needs a selection with area".into();
        };
        let (x0, y0) = ((b[0] - r_full - 1).max(0), (b[1] - r_full - 1).max(0));
        let (x1, y1) = ((b[2] + r_full + 2).min(w), (b[3] + r_full + 2).min(h));
        let mut painted = 0usize;
        self.begin_op();
        for y in y0..y1 {
            for x in x0..x1 {
                if !is_ring(x, y) {
                    continue;
                }
                let idx = crate::tile::TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                let t = self.layers[li].tile_mut(idx);
                t.set_pixel(
                    (x - ox) as usize,
                    (y - oy) as usize,
                    [
                        crate::blend::f32_to_fix15(colour[0].clamp(0.0, 1.0)),
                        crate::blend::f32_to_fix15(colour[1].clamp(0.0, 1.0)),
                        crate::blend::f32_to_fix15(colour[2].clamp(0.0, 1.0)),
                        crate::blend::f32_to_fix15(1.0),
                    ],
                );
                painted += 1;
            }
        }
        self.end_op();
        if painted == 0 {
            return "nothing to outline".into();
        }
        self.set_op_label("Outline");
        format!("outlined the selection — {painted} px")
    }

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
        self.apply_filter_with(f, &mut |_, _| false)
    }

    /// [`Self::apply_filter`] with a kernel lent by the caller — the GPU
    /// seam's door into the filter path.
    ///
    /// `run` is handed the gathered halo buffer (step 3 below) and returns
    /// `true` if it filtered it in place; `false` — always legal, always
    /// correct — runs [`Filter::run`], the CPU reference. Everything around
    /// it (write region, halo, selection clip, the single undo step) is
    /// unchanged and unaware of which one ran.
    pub fn apply_filter_with(&mut self, f: Filter, run: &mut RasterKernel<'_>) -> bool {
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
        if !run(f, &mut buf) {
            f.run(&mut buf, gx, gy);
        }

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
    use crate::doc::{Document, LayerExpression};
    use crate::selection::Selection;
    use crate::tile::{TILE_SIZE, TileIdx};

    fn ink(doc: &mut Document, li: usize, x0: i32, y0: i32, x1: i32, y1: i32, rgb: [f32; 3], a: f32) {
        for y in y0..y1 {
            for x in x0..x1 {
                let idx = TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                let t = doc.layers[li].tile_mut(idx);
                let o = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
                let d = t.data_mut();
                d[o] = crate::blend::f32_to_fix15(rgb[0] * a);
                d[o + 1] = crate::blend::f32_to_fix15(rgb[1] * a);
                d[o + 2] = crate::blend::f32_to_fix15(rgb[2] * a);
                d[o + 3] = crate::blend::f32_to_fix15(a);
            }
        }
    }

    fn px(doc: &Document, li: usize, x: i32, y: i32) -> [u16; 4] {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        doc.layers[li]
            .tile_arc(idx)
            .map(|t| {
                let o = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
                let d = t.data();
                [d[o], d[o + 1], d[o + 2], d[o + 3]]
            })
            .unwrap_or([0, 0, 0, 0])
    }

    /// FL-016: the zoom smear reads each pixel's own INWARD ray — a dot
    /// west of centre smears further west (outward), never east, never
    /// sideways.
    #[test]
    fn radial_blur_smears_along_the_ray() {
        let mut doc = Document::new(64, 64);
        let li = 0; // paint_rect writes layer 0 — the fresh doc's base layer
        let _ = li;
        paint_rect(&mut doc, 24, 32, 25, 33); // dot 8px west of centre
        assert!(doc.apply_filter(Filter::RadialBlur { strength: 0.5 }));
        // k = 0.5: a dest pixel at radius R samples its ray from 0.5R to
        // R, so the dot (radius 8) lands on dest pixels with R in ~[8, 16]
        // on the WEST ray — and only there.
        let west = alpha_at(&doc, 20, 32); // radius 12, on the ray
        assert!(west > 4000, "the smear reached outward along the ray ({west})");
        assert_eq!(
            alpha_at(&doc, 20, 24),
            0,
            "the same radius OFF the ray is untouched"
        );
        assert_eq!(alpha_at(&doc, 44, 32), 0, "the east ray never samples west");
        // The dot's own spot faded (its ink was averaged with the empty
        // inward half of its ray).
        assert!(alpha_at(&doc, 24, 32) < 32768 / 2, "the dot itself faded");
    }

    /// FL-017: the rotational smear follows the arc — a dot at angle 0
    /// bleeds to nearby angles at its own radius, not to the far side.
    #[test]
    fn spin_blur_follows_the_arc() {
        let mut doc = Document::new(64, 64);
        paint_rect(&mut doc, 40, 32, 42, 34); // a 2×2 dot 8px EAST of centre
        assert!(doc.apply_filter(Filter::SpinBlur { angle_deg: 45.0 }));
        // The dot bleeds along the arc of ±45° at radius 8: (say) 15°
        // above east gains; 60° does not; the opposite side never does.
        // The smear stays AT the dot's radius (each pixel averages its own
        // arc), so probe the strongest pixel in the arc's neighbourhood:
        // angles ~5-40° at radius 7-9.
        let mut near_arc = 0u16;
        for (dx, dy) in [(8, 1), (8, 2), (7, 2), (8, 3), (7, 3), (8, 4), (7, 4)] {
            near_arc = near_arc.max(alpha_at(&doc, 32 + dx, 32 + dy));
        }
        assert!(near_arc > 4000, "the smear followed the arc ({near_arc})");
        assert_eq!(
            alpha_at(&doc, 44, 26),
            0,
            "a different radius at a nearby angle is clean"
        );
        assert_eq!(alpha_at(&doc, 24, 32), 0, "the opposite side is clean");
        // The dot's own spot faded into the average.
        assert!(alpha_at(&doc, 40, 32) < 32768 / 2, "the dot itself faded");
    }

    /// CSP Convert to drawing colour: rgb swaps, coverage survives; the
    /// grey/mono + pure-B/W refusal; the selection bound.
    #[test]
    fn convert_to_drawing_colour_keeps_coverage() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("l");
        ink(&mut doc, li, 10, 10, 20, 20, [1.0, 0.0, 0.0], 0.5);
        let status = doc.convert_to_drawing_colour([0.0, 0.0, 1.0]);
        assert!(status.contains("recoloured"), "{status}");
        let p = px(&doc, li, 15, 15);
        assert_eq!(p[3], crate::blend::f32_to_fix15(0.5), "alpha kept");
        assert_eq!(p[0], 0, "red gone");
        assert_eq!(p[2], crate::blend::f32_to_fix15(0.5), "blue at half (premul)");
        assert!(doc.undo(), "one undo");
        assert_eq!(px(&doc, li, 15, 15)[0], crate::blend::f32_to_fix15(0.5));

        // Grey layer + pure black pick → refused, nothing touched.
        doc.layers[li].expression = LayerExpression::Grey;
        let before = px(&doc, li, 15, 15);
        let status = doc.convert_to_drawing_colour([0.0, 0.0, 0.0]);
        assert!(status.contains("pick a colour"), "{status}");
        assert_eq!(px(&doc, li, 15, 15), before);
    }

    /// CSP Outline selection: the band lands on the right side of the
    /// ants for all three border types.
    #[test]
    fn outline_selection_draws_the_band_on_the_chosen_side() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("l");
        doc.selection = Some(Selection::from_rect(&doc, 40.0, 40.0, 60.0, 60.0));
        let status = doc.outline_selection(4.0, OutlineBorder::Outside, false, [0.0, 0.0, 0.0]);
        assert!(status.contains("outlined"), "{status}");
        let a = |x: i32, y: i32| px(&doc, li, x, y)[3];
        assert!(a(37, 50) > 0, "outside band, 3px from the edge");
        assert_eq!(a(50, 50), 0, "the selection's interior untouched");
        assert_eq!(a(35, 50), 0, "beyond the width, untouched");
        assert!(a(63, 50) > 0, "3px outside the far edge is inked");
        assert_eq!(a(64, 50), 0, "4+1 outside the far edge is not");
        assert!(doc.undo());

        // Inside: the band sits within the ants.
        doc.outline_selection(4.0, OutlineBorder::Inside, false, [0.0, 0.0, 0.0]);
        assert!(px(&doc, li, 42, 50)[3] > 0, "inside band near the edge");
        assert_eq!(px(&doc, li, 50, 50)[3], 0, "deep interior untouched");
        assert_eq!(px(&doc, li, 37, 50)[3], 0, "outside untouched");
        assert!(doc.undo());

        // On border: centred — both sides get half.
        doc.outline_selection(6.0, OutlineBorder::OnBorder, false, [0.0, 0.0, 0.0]);
        assert!(px(&doc, li, 40, 50)[3] > 0, "just outside inked");
        assert!(px(&doc, li, 41, 50)[3] > 0, "just inside inked");
        assert_eq!(px(&doc, li, 50, 50)[3], 0, "deep interior untouched");
    }

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
        assert!(
            alpha_at(&doc, 80, 104) > 0,
            "smeared along +x past the block"
        );
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
        assert_eq!(
            doc.undo_labels().last().map(String::as_str),
            Some("Blur (strong)")
        );
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
        assert!(!doc.apply_filter(Filter::Unsharp {
            radius: 3.0,
            amount: 0.0,
        }));
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

    /// FL-014: the mask overshoots on both sides of an edge and — the half
    /// that matters — leaves everything that is not an edge exactly alone.
    #[test]
    fn unsharp_overshoots_an_edge_and_leaves_a_flat_field() {
        let mut r = Raster::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let v = if x < 32 { 8000 } else { 24000 };
                r.set_pixel(x, y, [v, v, v, 32768]);
            }
        }
        let f = Filter::Unsharp {
            radius: 3.0,
            amount: 1.0,
        };
        let mut out = r.clone();
        f.run(&mut out, 0, 0);
        // Flat ground, further than the reach from both the step and the
        // buffer's transparent surround: byte-identical.
        let m = f.reach() as usize;
        assert!(m >= 4 && 32 - m > m, "the probes below sit in real interior");
        assert_eq!(out.pixel(32 - m - 1, 32), [8000, 8000, 8000, 32768]);
        assert_eq!(out.pixel(32 + m, 32), [24000, 24000, 24000, 32768]);
        // The step itself: dark side darker, light side lighter.
        assert!(out.pixel(31, 32)[0] < 8000, "undershoot on the dark side");
        assert!(out.pixel(32, 32)[0] > 24000, "overshoot on the light side");
        // Alpha was flat everywhere, so it never moved — and no colour
        // channel ever escaped above it.
        for x in m..64 - m {
            let p = out.pixel(x, 32);
            assert_eq!(p[3], 32768, "alpha moved at x={x}");
            assert!(p[0] <= p[3], "colour above its own alpha at x={x}: {p:?}");
        }
        // Amount zero is nothing, and says so instead of pushing an undo.
        assert!(Filter::Unsharp {
            radius: 3.0,
            amount: 0.0
        }
        .is_identity());
    }

    /// FL-014 reads a NEIGHBOURHOOD like every blur here, so it gets the same
    /// seam test — a soft edge centred on the tile boundary must sharpen
    /// symmetrically — plus the point of the filter: the ramp a blur laid
    /// down is pulled back in.
    #[test]
    fn unsharp_sharpens_symmetrically_across_a_tile_boundary() {
        let mut doc = Document::new(256, 256);
        paint_rect(&mut doc, 40, 100, 88, 140); // centred on x = 64
        assert!(doc.apply_filter(Filter::Gaussian { sigma: 6.0 }));
        let skirt = alpha_at(&doc, 34, 120);
        assert!(skirt > 0, "the blur laid down an outer skirt to sharpen");
        let f = Filter::Unsharp {
            radius: 3.0,
            amount: 1.0,
        };
        assert!(doc.apply_filter(f));
        for d in 0..=f.reach() + 4 {
            let l = px_at(&doc, 64 - 1 - d, 120);
            let r = px_at(&doc, 64 + d, 120);
            assert_eq!(l, r, "seam at x=64, distance {d}: {l:?} vs {r:?}");
        }
        assert!(
            alpha_at(&doc, 34, 120) < skirt,
            "the ramp's outer skirt pulled back"
        );
        assert_eq!(alpha_at(&doc, 64, 120), 32768, "the middle is still solid");
        assert_eq!(
            doc.undo_labels().last().map(String::as_str),
            Some("Unsharp mask")
        );
    }

    /// One opaque pixel on an otherwise empty square raster, for the distort
    /// tests: `n` must be ODD so the centre lands exactly on a pixel and the
    /// arithmetic below has no half-pixel in it.
    fn dot_raster(n: usize, x: usize, y: usize) -> Raster {
        assert!(n % 2 == 1, "the centre must land on a pixel");
        let mut r = Raster::new(n, n);
        r.set_pixel(x, y, [0, 0, 0, 32768]);
        r
    }

    /// The strongest alpha in the 3×3 around `(x, y)` — the distorts land
    /// their ink at a fractional position, so a probe is a neighbourhood.
    fn near(r: &Raster, x: usize, y: usize) -> u16 {
        let mut m = 0;
        for dy in 0..3 {
            for dx in 0..3 {
                m = m.max(r.pixel(x + dx - 1, y + dy - 1)[3]);
            }
        }
        m
    }

    /// FL-020: the sign of the amount is the whole feature. Both directions
    /// are checked against the radius the map solves for exactly:
    /// `R·(r/R)^(1−a) = 16` for the dot at radius 16, R = 32.
    #[test]
    fn pinch_pulls_inward_and_a_negative_amount_bulges_out() {
        // Pinch: (r/32)^0.5 = 0.5 → r = 8 → the dot arrives at x = 40.
        let mut out = dot_raster(65, 48, 32);
        Filter::Pinch { amount: 0.5 }.run(&mut out, 0, 0);
        assert!(near(&out, 40, 32) > 4000, "the dot moved inward to r = 8");
        assert_eq!(out.pixel(48, 32)[3], 0, "and left r = 16");
        assert_eq!(near(&out, 32, 24), 0, "nothing appeared off the ray");

        // Bulge: (r/32)^1.5 = 0.5 → r = 32·0.5^(2/3) ≈ 20.2 → x ≈ 52.
        let mut out = dot_raster(65, 48, 32);
        Filter::Pinch { amount: -0.5 }.run(&mut out, 0, 0);
        assert!(near(&out, 52, 32) > 4000, "the dot moved outward to r ≈ 20");
        assert_eq!(out.pixel(48, 32)[3], 0, "and left r = 16");

        // Outside the inscribed circle the map is the identity, which is
        // what lets this filter claim a reach of zero.
        assert_eq!(Filter::Pinch { amount: 0.5 }.reach(), 0);
        let mut out = dot_raster(65, 64, 32); // r = 32 = R, on the rim
        Filter::Pinch { amount: 0.5 }.run(&mut out, 0, 0);
        assert_eq!(out.pixel(64, 32)[3], 32768, "the rim pixel did not move");
    }

    /// FL-023: the turn is strongest at the centre and dies at the rim. A dot
    /// at r = 8 of R = 32 turns by `90° · (1 − 8/32)` = 67.5°, and its radius
    /// is preserved exactly.
    #[test]
    fn twirl_turns_by_the_falloff_and_pins_the_rim() {
        let mut out = dot_raster(65, 32, 24); // r = 8, due NORTH of centre
        Filter::Twirl { angle_deg: 90.0 }.run(&mut out, 0, 0);
        // Source angle −90°, destination angle −90° + 67.5° = −22.5°:
        // (32 + 8·cos, 32 + 8·sin) = (39.4, 28.9).
        assert!(near(&out, 39, 29) > 4000, "the dot turned by 67.5°");
        assert_eq!(out.pixel(32, 24)[3], 0, "and left where it was");
        // Radius preserved: nothing appeared inside or outside the ring.
        for (x, y) in [(32usize, 32usize), (32, 12), (52, 32)] {
            assert_eq!(near(&out, x, y), 0, "ink at the wrong radius: {x},{y}");
        }
        assert_eq!(Filter::Twirl { angle_deg: 90.0 }.reach(), 0);
        let mut out = dot_raster(65, 64, 32);
        Filter::Twirl { angle_deg: 90.0 }.run(&mut out, 0, 0);
        assert_eq!(out.pixel(64, 32)[3], 32768, "the rim pixel did not move");
    }

    /// FL-021: purely radial. The dot changes radius but never leaves its ray
    /// — the property that separates a ripple from a smear.
    #[test]
    fn ripple_moves_along_the_ray_and_never_off_it() {
        let mut out = dot_raster(65, 48, 32); // r = 16, due EAST
        let f = Filter::Ripple {
            amplitude: 4.0,
            wavelength: 64.0,
        };
        f.run(&mut out, 0, 0);
        // r_src(16) = 16 + 4·sin(π/2) = 20, which is empty, so the dot's old
        // seat is clear and its ink sits at the r solving r_src(r) = 16.
        assert_eq!(out.pixel(48, 32)[3], 0, "the dot's old seat is clear");
        let on_ray = (33..65).map(|x| out.pixel(x, 32)[3]).max().unwrap();
        assert!(on_ray > 4000, "the ink is still on the east ray ({on_ray})");
        for y in [30usize, 34] {
            let off = (0..65).map(|x| out.pixel(x, y)[3]).max().unwrap();
            assert_eq!(off, 0, "ink left the ray onto row {y}");
        }
        // The halo has to cover the amplitude, both ways.
        assert_eq!(f.reach(), 5);
    }

    /// FL-022: a horizontal wave slides ROWS and only rows — a vertical line
    /// becomes a sine and keeps every one of its pixels on its own row.
    #[test]
    fn wave_shears_rows_by_the_sine_and_leaves_the_other_axis_alone() {
        let mut r = Raster::new(64, 64);
        for y in 0..64 {
            r.set_pixel(32, y, [0, 0, 0, 32768]); // a vertical line at x = 32
        }
        let f = Filter::Wave {
            amplitude: 4.0,
            wavelength: 16.0,
            dir: WaveDir::Horizontal,
        };
        let mut out = r.clone();
        f.run(&mut out, 0, 0);
        // Destination x samples x + 4·sin(2πy/16), so the line lands at
        // x = 32 − 4·sin(2πy/16): y = 4 → x = 28, y = 12 → x = 36.
        assert_eq!(out.pixel(28, 4)[3], 32768, "the crest slid four left");
        assert_eq!(out.pixel(32, 4)[3], 0, "and left the straight column");
        assert_eq!(out.pixel(36, 12)[3], 32768, "the trough slid four right");
        // y = 0, 8, 16 are the zero crossings — the line is where it was.
        for y in [0usize, 8, 16] {
            assert_eq!(out.pixel(32, y)[3], 32768, "zero crossing moved at y={y}");
        }
        // Every row still holds exactly the ink it started with — a
        // fractional shift splits one pixel across two, but the row TOTAL is
        // conserved, and no row gained any. That is what "the other axis is
        // untouched" means, and it is the assertion a vertical leak breaks.
        for y in 0..64 {
            let sum: u32 = (0..64).map(|x| out.pixel(x, y)[3] as u32).sum();
            assert!(
                sum.abs_diff(32768) <= 2,
                "row {y} holds {sum}, not the one pixel of ink it started with"
            );
        }
        assert_eq!(f.reach(), 5);
    }

    /// The whole distort family through the document path: one undo step
    /// each, labelled, and the no-op parameter sets refuse.
    #[test]
    fn every_distort_is_one_labelled_undo_step() {
        for (f, label) in [
            (Filter::Pinch { amount: 0.5 }, "Pinch"),
            (
                Filter::Ripple {
                    amplitude: 4.0,
                    wavelength: 32.0,
                },
                "Ripple",
            ),
            (
                Filter::Wave {
                    amplitude: 4.0,
                    wavelength: 32.0,
                    dir: WaveDir::Horizontal,
                },
                "Wave",
            ),
            (Filter::Twirl { angle_deg: 90.0 }, "Twirl"),
        ] {
            let mut doc = Document::new(256, 256);
            paint_rect(&mut doc, 40, 40, 200, 160); // not radially symmetric
            let before: Vec<[u16; 4]> = (0..256).map(|x| px_at(&doc, x, 100)).collect();
            let depth = doc.undo_len();
            assert!(doc.apply_filter(f), "{label} ran");
            assert_eq!(doc.undo_len(), depth + 1, "{label}: one step, not many");
            assert_eq!(doc.undo_labels().last().map(String::as_str), Some(label));
            assert!(doc.undo(), "{label} undoes");
            for x in 0..256 {
                assert_eq!(px_at(&doc, x, 100), before[x as usize], "{label} at x={x}");
            }
        }
        let mut doc = Document::new(128, 128);
        paint_rect(&mut doc, 10, 10, 60, 60);
        let depth = doc.undo_len();
        assert!(!doc.apply_filter(Filter::Pinch { amount: 0.0 }));
        assert!(!doc.apply_filter(Filter::Twirl { angle_deg: 0.0 }));
        assert!(!doc.apply_filter(Filter::Ripple {
            amplitude: 0.0,
            wavelength: 32.0,
        }));
        assert!(!doc.apply_filter(Filter::Wave {
            amplitude: 4.0,
            wavelength: 0.0,
            dir: WaveDir::Vertical,
        }));
        assert_eq!(doc.undo_len(), depth, "no empty undo steps were pushed");
    }

    /// LC-001: the threshold is an AREA, and it separates specks from the
    /// drawing rather than from each other.
    #[test]
    fn remove_dust_clears_small_specks_and_keeps_the_line() {
        let mut doc = Document::new(128, 128);
        paint_rect(&mut doc, 20, 20, 100, 22); // the drawing: a 160 px bar
        paint_rect(&mut doc, 60, 60, 61, 61); // 1 px
        paint_rect(&mut doc, 70, 70, 72, 72); // 4 px
        paint_rect(&mut doc, 80, 80, 83, 83); // 9 px
        let depth = doc.undo_len();
        assert!(doc.apply_filter(Filter::RemoveDust { max_px: 5 }));
        assert_eq!(doc.undo_len(), depth + 1, "one step, not one per speck");
        assert_eq!(
            doc.undo_labels().last().map(String::as_str),
            Some("Remove dust")
        );
        assert_eq!(alpha_at(&doc, 60, 60), 0, "the 1 px speck went");
        assert_eq!(alpha_at(&doc, 70, 70), 0, "the 4 px speck went");
        assert_eq!(alpha_at(&doc, 71, 71), 0, "all of it, not just a corner");
        assert_eq!(alpha_at(&doc, 81, 81), 32768, "the 9 px blob stayed");
        assert_eq!(alpha_at(&doc, 50, 21), 32768, "the drawing stayed");
        assert!(doc.undo(), "and it all comes back in one press");
        assert_eq!(alpha_at(&doc, 60, 60), 32768);
    }

    /// LC-001 is 8-connected: a diagonal chain is ONE speck, so a threshold
    /// below its length keeps it. Under 4-connectivity the same chain would
    /// be four one-pixel specks and this would wipe it.
    #[test]
    fn remove_dust_reads_a_diagonal_chain_as_one_speck() {
        let mut doc = Document::new(128, 128);
        for i in 0..4 {
            paint_rect(&mut doc, 40 + i, 40 + i, 41 + i, 41 + i);
        }
        paint_rect(&mut doc, 60, 60, 61, 61); // a lone pixel, for contrast
        assert!(doc.apply_filter(Filter::RemoveDust { max_px: 2 }));
        for i in 0..4 {
            assert_eq!(
                alpha_at(&doc, 40 + i, 40 + i),
                32768,
                "the diagonal chain counts as four, not one — pixel {i}"
            );
        }
        assert_eq!(alpha_at(&doc, 60, 60), 0, "the lone pixel still went");
    }

    /// LC-002: the ball is exact in both directions and the soft edge rides
    /// along with it — the anti-aliased column moves out by the grow and in
    /// by the erode, keeping both its coverage and its colour.
    #[test]
    fn line_width_moves_the_edge_by_exactly_the_radius() {
        // A red bar, x 30..34 opaque, with one anti-aliased column at x = 29.
        let mut bar = Raster::new(64, 64);
        for y in 0..64 {
            bar.set_pixel(29, y, [16384, 0, 0, 16384]);
            for x in 30..34 {
                bar.set_pixel(x, y, [32768, 0, 0, 32768]);
            }
        }
        let mut out = bar.clone();
        Filter::LineWidth { delta: 2 }.run(&mut out, 0, 0);
        assert_eq!(out.pixel(28, 32), [32768, 0, 0, 32768], "solid grew two");
        assert_eq!(out.pixel(27, 32), [16384, 0, 0, 16384], "the soft edge too");
        assert_eq!(out.pixel(26, 32)[3], 0, "and not one pixel further");
        assert_eq!(out.pixel(35, 32)[3], 32768, "the same on the far side");
        assert_eq!(out.pixel(36, 32)[3], 0);

        let mut out = bar.clone();
        Filter::LineWidth { delta: -1 }.run(&mut out, 0, 0);
        assert_eq!(out.pixel(29, 32)[3], 0, "the soft column eroded away");
        assert_eq!(
            out.pixel(30, 32),
            [16384, 0, 0, 16384],
            "and left the soft edge one in, still pure red"
        );
        assert_eq!(out.pixel(31, 32)[3], 32768, "the core is still solid");
        assert_eq!(out.pixel(33, 32)[3], 0, "the far side lost its column");

        assert_eq!(Filter::LineWidth { delta: 2 }.reach(), 2);
        assert_eq!(Filter::LineWidth { delta: -3 }.reach(), 3);
        assert!(Filter::LineWidth { delta: 0 }.is_identity());
    }

    /// LC-002 through the document: a grow spreads ink PAST the layer's old
    /// footprint (so the halo has to be there), it is one undo step, and it
    /// crosses a tile boundary without a seam.
    #[test]
    fn line_width_grows_past_the_footprint_in_one_step() {
        let mut doc = Document::new(256, 256);
        paint_rect(&mut doc, 40, 100, 88, 140); // centred on the x = 64 seam
        let depth = doc.undo_len();
        let f = Filter::LineWidth { delta: 3 };
        assert!(doc.apply_filter(f));
        assert_eq!(doc.undo_len(), depth + 1);
        assert_eq!(
            doc.undo_labels().last().map(String::as_str),
            Some("Adjust line width")
        );
        assert_eq!(alpha_at(&doc, 37, 120), 32768, "grew three px outward");
        assert_eq!(alpha_at(&doc, 36, 120), 0, "and no further");
        assert_eq!(alpha_at(&doc, 64, 97), 32768, "on every side");
        for d in 0..=f.reach() + 2 {
            assert_eq!(
                px_at(&doc, 64 - 1 - d, 120),
                px_at(&doc, 64 + d, 120),
                "seam at x=64, distance {d}"
            );
        }
        assert!(!doc.apply_filter(Filter::LineWidth { delta: 0 }));
        assert_eq!(doc.undo_len(), depth + 1, "a zero adjustment pushed nothing");
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

    /// The GPU seam's door: a lent kernel that declines must leave a page
    /// byte-identical to the plain call, and one that runs must be what the
    /// page shows. Both halves matter — the first is the fallback contract,
    /// the second proves the door is actually wired to the pixels.
    #[test]
    fn a_lent_raster_kernel_replaces_the_filter_or_declines_cleanly() {
        let page = |run: &mut RasterKernel<'_>| {
            let mut doc = Document::new(192, 192);
            paint_rect(&mut doc, 40, 40, 90, 90);
            assert!(doc.apply_filter_with(Filter::Gaussian { sigma: 5.0 }, run));
            doc
        };
        let mut plain = Document::new(192, 192);
        paint_rect(&mut plain, 40, 40, 90, 90);
        assert!(plain.apply_filter(Filter::Gaussian { sigma: 5.0 }));

        let declined = page(&mut |_, _| false);
        for y in (0..192).step_by(7) {
            for x in (0..192).step_by(7) {
                assert_eq!(
                    alpha_at(&declined, x, y),
                    alpha_at(&plain, x, y),
                    "a declined kernel changed ({x},{y})"
                );
            }
        }

        // A kernel that blanks the buffer: the page must be empty, which is
        // only true if the lent closure's output is what gets scattered back.
        let ran = page(&mut |_, buf| {
            buf.px.fill(0);
            true
        });
        assert_eq!(alpha_at(&ran, 64, 64), 0, "the lent kernel's buffer was ignored");
    }

    /// The pass chain the GPU seam runs has to be the same shape the CPU
    /// runs — same radii, same denominators — or parity is an accident.
    #[test]
    fn separable_passes_mirror_the_box_radii() {
        assert_eq!(Filter::Gaussian { sigma: 0.0 }.separable_passes(), None);
        assert_eq!(Filter::Mosaic { cell: 8 }.separable_passes(), None);
        assert_eq!(
            Filter::Smoothing.separable_passes(),
            Some(vec![BoxPass {
                half: vec![2, 1],
                denom: 4
            }])
        );
        for sigma in [1.4f32, 4.0, 12.0, 60.0, MAX_SIGMA] {
            let radii = box_radii(sigma);
            let passes = Filter::Gaussian { sigma }
                .separable_passes()
                .unwrap_or_default();
            let want: Vec<usize> = radii.iter().copied().filter(|&r| r > 0).collect();
            assert_eq!(passes.len(), want.len(), "pass count at σ={sigma}");
            for (p, r) in passes.iter().zip(&want) {
                assert_eq!(p.half.len(), r + 1, "reach at σ={sigma}");
                assert!(p.half.iter().all(|&w| w == 1), "a box is flat");
                assert_eq!(p.denom, (2 * r + 1) as u32, "denominator at σ={sigma}");
            }
        }
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
