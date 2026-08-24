//! Screentone (網点) rasterization — the CSP トーンレイヤー equivalent.
//!
//! A tone layer keeps its painted SOURCE pixels untouched and presents a
//! derived halftone raster instead: an AM screen (round clustered dots or
//! lines) whose local ink coverage follows the source's ink. Converting a
//! layer to tones and back is therefore non-destructive in both directions —
//! the model is the frame folder's derived mask, not a pixel bake.
//!
//! Ink coverage of a premultiplied fix15 pixel: `ink = (a - rgb_mean) / 1.0`
//! — black ink at alpha `a` gives `a`, white gives 0 at any alpha, grey lands
//! in between. Output is black premultiplied ink; AA comes from 2×2
//! subsampling, so edges are 25 %-stepped rather than jagged.
//!
//! The screen lives in LPI against the document's DPI (cell `C = dpi / lpi`
//! px); `Document::refresh_derived` is the only caller that should need the
//! dpi, since a tone raster is meaningless without it.
//!
//! ## Where the coverage comes from ([`ToneDensity`], `LP-008`)
//!
//! Three sources, all producing the same 0..=1 "ink" the screen geometry
//! eats. [`ToneDensity::ImageColour`] is the historical one and stays the
//! DEFAULT, so nothing an existing file draws moves.
//!
//! ## Area conservation (why the shapes are written the way they are)
//!
//! A screen is only useful if 40 % density prints as 40 % ink. Every shape
//! below therefore solves its own size from the requested coverage instead
//! of scaling a picture: `Dots`, `Square`, `Ellipse`, `Lozenge`, `Cross`,
//! `Star` and `Noise` are area-EXACT by construction, and `Asterisk` is
//! area-exact against an inclusion-exclusion model of three overlapping
//! bars (good to a few percent, pinned by a test). Shapes that would grow
//! past the cell before reaching 50 % invert at 50 % into paper holes at
//! the cell corners — the classic manga behaviour — which is also what
//! keeps every shape inside its own cell at every density.

use serde::{Deserialize, Serialize};

use crate::tile::{TILE_SIZE, Tile};

/// Screen pattern shape (`LP-011`).
///
/// The nine PROCEDURAL shapes of CSP's 24. The other fifteen (cherry ×3,
/// flower ×3, clover ×2, heart, clubs, spades, sugar plum, carrot, ninja
/// star) are glyph-shaped ARTWORK rather than generated patterns; they
/// belong to the material bank under the bring-your-own rule
/// (`DECISIONS 8.5`), not to this module, and are deliberately not here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TonePattern {
    /// Round clustered dot — the classic manga screen.
    Dots,
    /// Parallel lines along the rotated axis.
    Lines,
    /// Square dot, grown from the cell centre.
    Square,
    /// 1.5 : 1 elongated dot, long axis along the screen angle.
    Ellipse,
    /// Diamond (CSP calls it Lozenge).
    Lozenge,
    /// A plus sign: two full-cell bars that thicken with density.
    Cross,
    /// White-noise dither — an FM screen, no lattice. Grain follows the
    /// cell (a quarter of it), so Frequency still means something.
    Noise,
    /// Six spokes: three bars through the cell centre, 60° apart.
    Asterisk,
    /// Five-pointed star.
    Star,
}

impl TonePattern {
    /// Every shape, in the dropdown's order.
    pub const ALL: [TonePattern; 9] = [
        TonePattern::Dots,
        TonePattern::Lines,
        TonePattern::Square,
        TonePattern::Ellipse,
        TonePattern::Lozenge,
        TonePattern::Cross,
        TonePattern::Noise,
        TonePattern::Asterisk,
        TonePattern::Star,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TonePattern::Dots => "Dots",
            TonePattern::Lines => "Lines",
            TonePattern::Square => "Square",
            TonePattern::Ellipse => "Ellipse",
            TonePattern::Lozenge => "Lozenge",
            TonePattern::Cross => "Cross",
            TonePattern::Noise => "Noise",
            TonePattern::Asterisk => "Asterisk",
            TonePattern::Star => "Star",
        }
    }

    /// True where this shape inks, for one sample on the rotated screen
    /// axes `(u, v)` at coverage `ink` in a `cell`-px screen cell.
    ///
    /// Public because a screened BALLOON fill ([`crate::balloon::BalloonTone`])
    /// screens per pixel inside the SDF rather than per tile — same geometry,
    /// different caller, so the two must never drift apart.
    pub fn on(self, u: f32, v: f32, ink: f32, cell: f32) -> bool {
        if ink <= 0.0 {
            return false;
        }
        if ink >= 1.0 {
            return true;
        }
        match self {
            TonePattern::Lines => line_on(u, ink, cell),
            TonePattern::Noise => noise_on(u, v, ink, cell),
            _ => {
                // Cell-local coordinates, centred: -C/2 .. C/2 on both axes.
                let cu = u.rem_euclid(cell) - cell * 0.5;
                let cv = v.rem_euclid(cell) - cell * 0.5;
                match self {
                    TonePattern::Dots => dot_on(cu, cv, ink, cell, 1.0),
                    TonePattern::Ellipse => dot_on(cu, cv, ink, cell, ELLIPSE_K),
                    TonePattern::Square => square_on(cu, cv, ink, cell),
                    TonePattern::Lozenge => lozenge_on(cu, cv, ink, cell),
                    TonePattern::Cross => cross_on(cu, cv, ink, cell),
                    TonePattern::Asterisk => {
                        if ink <= 0.5 {
                            asterisk_on(cu, cv, ink, cell)
                        } else {
                            dot_on(cu, cv, ink, cell, 1.0)
                        }
                    }
                    TonePattern::Star => {
                        if ink <= 0.5 {
                            star_on(cu, cv, ink, cell)
                        } else {
                            dot_on(cu, cv, ink, cell, 1.0)
                        }
                    }
                    // Handled above; kept exhaustive without a catch-all so
                    // a new shape is a compile error here, not a silent Dots.
                    TonePattern::Lines | TonePattern::Noise => false,
                }
            }
        }
    }
}

/// What decides the size of each dot (`LP-008`, CSP Layer Property ▸ Density).
///
/// [`ToneDensity::ImageColour`] is the DEFAULT and is what every tone layer
/// written before this existed keeps doing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ToneDensity {
    /// "Use colour of image": the darker the pixel, the bigger the dot —
    /// `ink = a - mean(rgb)` on the premultiplied source. All three channels
    /// weigh the same, so a saturated red and a saturated blue screen alike.
    ImageColour,
    /// "Use brightness of image": the dot follows the pixel's perceived
    /// BRIGHTNESS over paper white — Rec. 709 luma instead of a flat mean, so
    /// a blue reads darker than a green of the same mean. **Identical to
    /// [`ToneDensity::ImageColour`] for every neutral (r = g = b) pixel**, so
    /// switching modes only moves COLOURED art.
    ImageBrightness,
    /// "Use specified density": the art's colour is ignored and every dot is
    /// the same size — the flat "40 % tone" fill. Alpha still gates the
    /// region (transparent stays empty, a soft edge fades), which is exactly
    /// what a live [`crate::fill_layer::FillKind::Tone`] layer needs and what
    /// it now routes through.
    Specified(f32),
}

impl Default for ToneDensity {
    fn default() -> Self {
        ToneDensity::ImageColour
    }
}

impl ToneDensity {
    pub fn label(self) -> &'static str {
        match self {
            ToneDensity::ImageColour => "Colour of image",
            ToneDensity::ImageBrightness => "Brightness of image",
            ToneDensity::Specified(_) => "Specified density",
        }
    }
}

/// One screentone configuration. Manga defaults: 60 LPI at 45°.
///
/// **Every field added here must also appear in [`ToneParams::sig`]** — that
/// is the GPU tile cache's freshness key, and a field missing from it means
/// the canvas keeps showing the raster from before your edit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToneParams {
    pub pattern: TonePattern,
    /// Screen frequency, lines per inch (5..~80; 50–65 is the manga range).
    pub lpi: f32,
    /// Screen angle in degrees. Dots classically 45°, lines 90° (vertical).
    pub angle_deg: f32,
    /// `LP-014` dot position X / Y: shifts the lattice origin in CANVAS
    /// pixels, before rotation. The moiré fix — two tone layers at the same
    /// frequency and angle interfere, and nudging one layer's origin by a few
    /// px breaks the interference. Also the mechanism behind `TN-009`
    /// (move the tone pattern under fixed art).
    #[serde(default)]
    pub offset: [f32; 2],
    /// `LP-010` posterization: quantize the coverage into N flat steps
    /// (2..=20) instead of a continuous ramp. `None` = the continuous ramp,
    /// which is the historical behaviour.
    #[serde(default)]
    pub posterize: Option<u8>,
    /// `LP-008`: what drives the dot size.
    #[serde(default)]
    pub density: ToneDensity,
}

impl Default for ToneParams {
    fn default() -> Self {
        Self {
            pattern: TonePattern::Dots,
            lpi: 60.0,
            angle_deg: 45.0,
            offset: [0.0, 0.0],
            posterize: None,
            density: ToneDensity::ImageColour,
        }
    }
}

impl ToneParams {
    /// Screen cell size in pixels at `dpi`.
    fn cell_px(&self, dpi: u32) -> f32 {
        (dpi as f32 / self.lpi.max(1.0)).max(2.0)
    }

    /// Bit-exact signature of everything that changes the rasterized output.
    ///
    /// **THE TRAP THIS EXISTS FOR:** the GPU keeps a per-layer tile cache and
    /// only rebuilds it when the layer's signature moves (`gpu::LayerSig`).
    /// A new `ToneParams` field that is not in here means an edit changes the
    /// CPU raster while the canvas keeps drawing the old tiles — a silent
    /// lie, not a crash. `sig_covers_every_field` in this module's tests is
    /// the guard.
    pub fn sig(&self) -> [u32; 8] {
        let (dtag, dval) = match self.density {
            ToneDensity::ImageColour => (0, 0),
            ToneDensity::ImageBrightness => (1, 0),
            ToneDensity::Specified(v) => (2, v.to_bits()),
        };
        [
            self.pattern as u32,
            self.lpi.to_bits(),
            self.angle_deg.to_bits(),
            self.offset[0].to_bits(),
            self.offset[1].to_bits(),
            // +1 so `None` and `Some(0)` are distinct.
            self.posterize.map_or(0, |n| n as u32 + 1),
            dtag,
            dval,
        ]
    }

    /// Coverage 0..=1 the screen should render for one source pixel: the
    /// density source, then posterization.
    fn ink_at(&self, px: [u16; 4]) -> f32 {
        let ink = match self.density {
            ToneDensity::ImageColour => ink_of(px),
            ToneDensity::ImageBrightness => brightness_ink_of(px),
            // The art is ignored; alpha still says WHERE. Rounded onto the
            // source's own fix15 grid — the other two modes are quantized
            // that way because they READ fix15 pixels, and matching them
            // keeps a live fill layer's raster bit-identical to the
            // hand-rolled premultiply this replaced (`fill_layer`).
            ToneDensity::Specified(d) => {
                let a = (px[3] as f32 / 32768.0).clamp(0.0, 1.0);
                ((d.clamp(0.0, 1.0) * a * 32768.0) as u16) as f32 / 32768.0
            }
        };
        match self.posterize {
            Some(n) => posterize(ink, n),
            None => ink,
        }
    }
}

/// `LP-010`: snap coverage to one of `n` evenly spaced flat steps. 0 and 1
/// are always steps, so blank stays blank and solid stays solid.
fn posterize(ink: f32, n: u8) -> f32 {
    let steps = (n.max(2) as f32) - 1.0;
    (ink * steps).round() / steps
}

/// Ink coverage of one premultiplied fix15 pixel, 0..=1 (FIX15_ONE scaled).
#[inline]
fn ink_of(px: [u16; 4]) -> f32 {
    let a = px[3] as f32;
    let mean = (px[0] as f32 + px[1] as f32 + px[2] as f32) / 3.0;
    ((a - mean) / 32768.0).clamp(0.0, 1.0)
}

/// Ink coverage from the pixel's BRIGHTNESS over paper white: composite the
/// premultiplied pixel onto white, take its Rec. 709 luma, invert.
///
/// For a neutral pixel this is algebraically identical to [`ink_of`]
/// (`1 - (v/one + 1 - a/one)` is `(a - v)/one`), which is why switching a
/// greyscale tone layer between the two modes changes nothing.
#[inline]
fn brightness_ink_of(px: [u16; 4]) -> f32 {
    let a = px[3] as f32 / 32768.0;
    let over = |c: u16| (c as f32 / 32768.0 + (1.0 - a)).clamp(0.0, 1.0);
    let luma = 0.2126 * over(px[0]) + 0.7152 * over(px[1]) + 0.0722 * over(px[2]);
    (1.0 - luma).clamp(0.0, 1.0)
}

/// Rasterize one tile of the tone layer. `origin` is the tile's canvas-pixel
/// corner (the screen is continuous across tiles, so the canvas position
/// matters). The result carries a fresh revision.
pub fn rasterize_tile(src: &Tile, origin: (i32, i32), p: &ToneParams, dpi: u32) -> Tile {
    let cell = p.cell_px(dpi);
    let rad = p.angle_deg.to_radians();
    let (cs, sn) = (rad.cos(), rad.sin());
    let mut out = Tile::new_transparent();
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            let ink = p.ink_at(src.pixel(x, y));
            if ink <= 0.0 {
                continue;
            }
            // 2×2 subsampling: AA without smoothing the screen geometry.
            let mut ink_acc = 0u32;
            for (sx, sy) in [(0.25f32, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                // LP-014: the lattice origin moves by SUBTRACTING the offset
                // from the sample position, so a positive X slides the dots
                // right.
                let fx = origin.0 as f32 + x as f32 + sx - p.offset[0];
                let fy = origin.1 as f32 + y as f32 + sy - p.offset[1];
                // Project onto the rotated screen axes.
                let u = fx * cs - fy * sn;
                let v = fx * sn + fy * cs;
                ink_acc += p.pattern.on(u, v, ink, cell) as u32;
            }
            if ink_acc > 0 {
                let a = (ink_acc * 32768 / 4) as u16;
                out.set_pixel(x, y, [0, 0, 0, a]);
            }
        }
    }
    out
}

/// Elongation of the elliptical dot: √1.5, giving a 1.5 : 1 dot. Chosen so
/// the long semi-axis at 50 % coverage (`C·√(0.5/π)·k = 0.489 C`) still fits
/// inside the half-cell — a rounder aspect would clip and lose density.
const ELLIPSE_K: f32 = 1.224_745;

/// Round clustered-dot screen, optionally stretched into an ellipse by `k`
/// (`k = 1` is the circle). Black dots grow from the cell centres, and past
/// 50 % coverage white dots shrink at the corners — the classic manga dot.
/// No sqrt on the hot path: `d²·π ≤ C²·a` compares squared quantities.
///
/// Scaling `(cu/k, cv·k)` has determinant 1, so the ellipse encloses exactly
/// the circle's area and the density law is unchanged.
fn dot_on(cu: f32, cv: f32, ink: f32, cell: f32, k: f32) -> bool {
    if ink <= 0.5 {
        // Dot radius r = C·sqrt(ink/π)  ⇔  d²·π ≤ C²·ink.
        let (du, dv) = (cu / k, cv * k);
        du * du + dv * dv <= cell * cell * ink / std::f32::consts::PI
    } else {
        // Paper hole at the cell corners with radius C·sqrt((1-ink)/π):
        // ink where the corner distance EXCEEDS it.
        // Corner coordinates: ±cell/2 in both axes.
        let du = (cell * 0.5 - cu.abs()) / k;
        let dv = (cell * 0.5 - cv.abs()) * k;
        // distance² to the nearest corner = du² + dv²
        du * du + dv * dv > cell * cell * (1.0 - ink) / std::f32::consts::PI
    }
}

/// Square dot of side `C·√ink` (area exactly `ink·C²`), inverting past 50 %
/// into paper squares of side `C·√(1-ink)` straddling the cell corners —
/// four quarters, one per neighbouring cell, so the area law still holds.
fn square_on(cu: f32, cv: f32, ink: f32, cell: f32) -> bool {
    if ink <= 0.5 {
        let half = cell * ink.sqrt() * 0.5;
        cu.abs() <= half && cv.abs() <= half
    } else {
        let half = cell * (1.0 - ink).sqrt() * 0.5;
        let du = cell * 0.5 - cu.abs();
        let dv = cell * 0.5 - cv.abs();
        !(du < half && dv < half)
    }
}

/// Diamond with half-diagonal `C·√(ink/2)` (area `2h² = ink·C²`), inverting
/// past 50 % into paper diamonds at the corners. At exactly 50 % the diamond
/// is inscribed in the cell, which is why it never clips.
fn lozenge_on(cu: f32, cv: f32, ink: f32, cell: f32) -> bool {
    if ink <= 0.5 {
        let h = cell * (ink * 0.5).sqrt();
        cu.abs() + cv.abs() <= h
    } else {
        let h = cell * ((1.0 - ink) * 0.5).sqrt();
        let du = cell * 0.5 - cu.abs();
        let dv = cell * 0.5 - cv.abs();
        du + dv > h
    }
}

/// Plus sign: two full-cell bars of width `w` crossing at the centre. Their
/// union is `2wC - w²`, so `w = C(1 - √(1-ink))` is area-exact at every
/// density and needs no inversion — at `ink = 1` the bars fill the cell.
fn cross_on(cu: f32, cv: f32, ink: f32, cell: f32) -> bool {
    let half = cell * (1.0 - (1.0 - ink).sqrt()) * 0.5;
    cu.abs() <= half || cv.abs() <= half
}

/// Three bars through the cell centre at 0°, 60° and 120° — six spokes.
///
/// Area by inclusion-exclusion on `t = w/C`: each bar covers `t·C²` times its
/// chord factor (1, 1.1547, 1.1547 for those three angles through a square),
/// and the three pairwise rhombus overlaps take `≈ 2.3 t²C²` back, giving
/// `ink ≈ 3.309 t - 2.3 t²`. Inverted here. It is a MODEL, not an identity —
/// `asterisk_coverage_tracks_ink` is what pins it, and the shape swaps to
/// paper holes past 50 % (see [`TonePattern::on`]) where the model would run
/// out.
fn asterisk_on(cu: f32, cv: f32, ink: f32, cell: f32) -> bool {
    const A: f32 = 3.309;
    const B: f32 = 2.3;
    let disc = (A * A - 4.0 * B * ink).max(0.0);
    let half = cell * (A - disc.sqrt()) / (2.0 * B) * 0.5;
    // Distance from each bar's centre line: |−cu·sin θ + cv·cos θ|.
    const SPOKES: [(f32, f32); 3] = [
        (0.0, 1.0),          // 0°
        (0.866_025_4, 0.5),  // 60°
        (0.866_025_4, -0.5), // 120°
    ];
    SPOKES.iter().any(|(s, c)| (-cu * s + cv * c).abs() <= half)
}

/// Inner/outer radius ratio of the five-pointed star, picked so its area law
/// is `2R²`: `5·ρ·sin 36° = 2`. That makes `R = C·√(ink/2)`, which is exactly
/// `C/2` at 50 % coverage — the star touches the cell edge and never clips.
const STAR_RHO: f32 = 0.680_53;
/// Half-angle of one star point, 36°.
const STAR_ALPHA: f32 = std::f32::consts::PI / 5.0;

/// Five-pointed star polygon, area `2R²` with `R = C·√(ink/2)`.
///
/// The boundary radius inside one 36° sector is the polar form of the line
/// from `(R, 0)` to `(ρR, 36°)`.
fn star_on(cu: f32, cv: f32, ink: f32, cell: f32) -> bool {
    let r = cell * (ink * 0.5).sqrt();
    let d2 = cu * cu + cv * cv;
    if d2 > r * r {
        return false; // outside the circumscribed circle — cheap reject
    }
    let sector = 2.0 * STAR_ALPHA;
    let mut phi = cv.atan2(cu).rem_euclid(sector);
    if phi > STAR_ALPHA {
        phi = sector - phi;
    }
    let sa = STAR_ALPHA.sin();
    let bound = r * STAR_RHO * sa / (phi.sin() + STAR_RHO * (STAR_ALPHA - phi).sin());
    d2 <= bound * bound
}

/// Line screen: lines of width `ink·C`, centred in the period, along the
/// rotated v axis.
fn line_on(u: f32, ink: f32, cell: f32) -> bool {
    let s = u.rem_euclid(cell);
    let d = s.min(cell - s); // distance to the nearest line centre
    d <= ink * cell * 0.5
}

/// White-noise dither (an FM screen — no lattice, so no moiré at all). Grain
/// is a quarter of the screen cell, floored at 1 px, so the Frequency slider
/// still controls how coarse the grain reads. `LP-013`'s explicit Size and
/// Factor controls are NOT implemented; this is the whole of the noise
/// vocabulary today.
///
/// Coverage is exact in expectation: the hash is uniform on 0..1 and the
/// test is `hash < ink`.
fn noise_on(u: f32, v: f32, ink: f32, cell: f32) -> bool {
    let grain = (cell * 0.25).max(1.0);
    hash01((u / grain).floor() as i32, (v / grain).floor() as i32) < ink
}

/// Deterministic uniform hash of a grain cell — the noise screen must be
/// identical every frame and continuous across tile seams, so it can only
/// depend on the canvas position.
fn hash01(x: i32, y: i32) -> f32 {
    let mut h = (x as u32)
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add((y as u32).wrapping_mul(0x85EB_CA77));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^= h >> 13;
    (h >> 8) as f32 / 16_777_216.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::{FIX15_ONE, TileIdx};

    /// A source tile uniformly painted at the given black-ink coverage.
    fn flat_source(ink: f32) -> Tile {
        let mut t = Tile::new_transparent();
        let a = (ink * FIX15_ONE as f32) as u16;
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                t.set_pixel(x, y, [0, 0, 0, a]);
            }
        }
        t
    }

    fn coverage_of(t: &Tile) -> f32 {
        t.alpha_sum() as f32 / (TILE_SIZE * TILE_SIZE) as f32 / 32768.0
    }

    #[test]
    fn zero_ink_is_empty_and_full_ink_is_solid() {
        let p = ToneParams::default();
        let dpi = 600;
        assert!(rasterize_tile(&flat_source(0.0), (0, 0), &p, dpi).is_blank());

        let solid = rasterize_tile(&flat_source(1.0), (0, 0), &p, dpi);
        assert!(!solid.is_blank());
        assert!(
            coverage_of(&solid) > 0.99,
            "solid ink coverage {}",
            coverage_of(&solid)
        );
    }

    #[test]
    fn dot_coverage_tracks_ink() {
        // 50 % ink through a 45° 60 LPI screen at 600 dpi (10 px cells) must
        // land near 50 % coverage — the screen conserves area by design.
        let p = ToneParams::default();
        let t = rasterize_tile(&flat_source(0.5), (0, 0), &p, 600);
        let c = coverage_of(&t);
        assert!((c - 0.5).abs() < 0.08, "50 % ink gave {c:.3} coverage");
    }

    #[test]
    fn line_screen_tracks_ink_and_angle_rotates() {
        let p = ToneParams {
            pattern: TonePattern::Lines,
            angle_deg: 0.0,
            ..Default::default()
        };
        let t = rasterize_tile(&flat_source(0.5), (0, 0), &p, 600);
        let c = coverage_of(&t);
        assert!((c - 0.5).abs() < 0.08, "50 % lines gave {c:.3} coverage");

        // Vertical-ish lines at angle 0: columns near the line centres are
        // inked, others are not — variance ACROSS x must be high.
        let mut col_means = Vec::new();
        for x in 0..TILE_SIZE {
            let mut s = 0u64;
            for y in 0..TILE_SIZE {
                s += t.pixel(x, y)[3] as u64;
            }
            col_means.push(s);
        }
        let mx = *col_means.iter().max().unwrap() as f32;
        let mn = *col_means.iter().min().unwrap() as f32;
        assert!(mx > mn, "line screen has no column structure");

        // A rotated screen stays continuous across the tile seam: the tile at
        // (1,0) must not be blank for 100 % ink regardless of angle.
        let q = ToneParams {
            pattern: TonePattern::Lines,
            lpi: 42.0,
            angle_deg: 33.0,
            ..Default::default()
        };
        let t2 = rasterize_tile(&flat_source(0.4), TileIdx::new(1, 0).origin(), &q, 600);
        assert!(
            coverage_of(&t2) > 0.2,
            "rotated off-origin tile lost the screen"
        );
    }

    #[test]
    fn white_source_rasterizes_to_nothing() {
        // The ink formula: white at full alpha is zero ink.
        let mut t = Tile::new_transparent();
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                t.set_pixel(x, y, [FIX15_ONE as u16; 4]);
            }
        }
        assert!(rasterize_tile(&t, (0, 0), &ToneParams::default(), 600).is_blank());
    }

    // --- LP-011 shapes ---------------------------------------------------

    /// Every shape is a SCREEN, not a picture: coverage must track the
    /// requested density. The generated shapes are area-exact by
    /// construction; Asterisk is area-exact against a model, so it gets the
    /// looser band. Anything failing this prints at the wrong density.
    #[test]
    fn every_shape_conserves_area() {
        for pat in TonePattern::ALL {
            for ink in [0.15f32, 0.3, 0.5, 0.75] {
                let p = ToneParams {
                    pattern: pat,
                    ..Default::default()
                };
                let c = coverage_of(&rasterize_tile(&flat_source(ink), (0, 0), &p, 600));
                let tol = match pat {
                    TonePattern::Asterisk => 0.10,
                    _ => 0.08,
                };
                assert!(
                    (c - ink).abs() < tol,
                    "{:?} at {ink} ink gave {c:.3} coverage",
                    pat
                );
            }
        }
    }

    /// The asterisk's width law is an inclusion-exclusion MODEL of three
    /// overlapping bars, not an identity — this is the test the comment on
    /// `asterisk_on` points at, pinned tighter than the sweep above.
    #[test]
    fn asterisk_coverage_tracks_ink() {
        let p = ToneParams {
            pattern: TonePattern::Asterisk,
            ..Default::default()
        };
        for ink in [0.2f32, 0.35, 0.5] {
            let c = coverage_of(&rasterize_tile(&flat_source(ink), (0, 0), &p, 600));
            assert!((c - ink).abs() < 0.08, "asterisk {ink} → {c:.3}");
        }
    }

    /// Shapes must actually differ: two screens at the same density and the
    /// same lattice cannot rasterize to the same pixels, or the dropdown is
    /// decoration.
    #[test]
    fn shapes_are_distinguishable() {
        let mut seen: Vec<(TonePattern, Vec<u16>)> = Vec::new();
        for pat in TonePattern::ALL {
            let p = ToneParams {
                pattern: pat,
                ..Default::default()
            };
            let t = rasterize_tile(&flat_source(0.35), (0, 0), &p, 600);
            let px: Vec<u16> = (0..TILE_SIZE)
                .flat_map(|y| (0..TILE_SIZE).map(move |x| (x, y)))
                .map(|(x, y)| t.pixel(x, y)[3])
                .collect();
            for (other, prev) in &seen {
                assert!(*prev != px, "{pat:?} rasterizes identically to {other:?}");
            }
            seen.push((pat, px));
        }
    }

    /// The noise screen is an FM screen: it must be deterministic (same
    /// pixels every call) and continuous across tile seams, since it hashes
    /// canvas position rather than tile-local position.
    #[test]
    fn noise_is_deterministic_and_tile_continuous() {
        let p = ToneParams {
            pattern: TonePattern::Noise,
            ..Default::default()
        };
        let a = rasterize_tile(&flat_source(0.4), (0, 0), &p, 600);
        let b = rasterize_tile(&flat_source(0.4), (0, 0), &p, 600);
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                assert_eq!(a.pixel(x, y), b.pixel(x, y), "noise is not deterministic");
            }
        }
        let far = rasterize_tile(&flat_source(0.4), TileIdx::new(3, 2).origin(), &p, 600);
        let c = coverage_of(&far);
        assert!((c - 0.4).abs() < 0.08, "off-origin noise coverage {c:.3}");
        // …and a different region of the field, not a repeat of tile (0,0).
        let same = (0..TILE_SIZE)
            .flat_map(|y| (0..TILE_SIZE).map(move |x| (x, y)))
            .all(|(x, y)| a.pixel(x, y) == far.pixel(x, y));
        assert!(!same, "the noise field repeats per tile");
    }

    // --- LP-014 / TN-009 lattice offset ----------------------------------

    /// The offset moves the lattice and nothing else: same coverage, moved
    /// pixels. A full-cell offset (10 px at 600 dpi / 60 LPI) along the
    /// screen's own axis lands back on the SAME raster — that is the proof it
    /// is a lattice shift rather than a repaint.
    #[test]
    fn offset_shifts_the_lattice_without_changing_density() {
        let base = ToneParams {
            angle_deg: 0.0,
            ..Default::default()
        };
        let moved = ToneParams {
            offset: [3.0, 0.0],
            ..base
        };
        let a = rasterize_tile(&flat_source(0.4), (0, 0), &base, 600);
        let b = rasterize_tile(&flat_source(0.4), (0, 0), &moved, 600);
        assert!(
            (coverage_of(&a) - coverage_of(&b)).abs() < 0.02,
            "the offset changed the density"
        );
        let differs = (0..TILE_SIZE)
            .flat_map(|y| (0..TILE_SIZE).map(move |x| (x, y)))
            .any(|(x, y)| a.pixel(x, y) != b.pixel(x, y));
        assert!(differs, "the offset moved nothing");

        // One whole cell along x at angle 0 is a no-op.
        let whole = ToneParams {
            offset: [10.0, 0.0],
            ..base
        };
        let c = rasterize_tile(&flat_source(0.4), (0, 0), &whole, 600);
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                assert_eq!(
                    a.pixel(x, y),
                    c.pixel(x, y),
                    "a full-cell offset moved the screen"
                );
            }
        }
    }

    /// The moiré fix in one assertion: two identical screens overlap
    /// perfectly (every inked pixel of one is inked in the other), and
    /// offsetting one breaks that lock-step.
    #[test]
    fn offset_breaks_two_identical_screens_apart() {
        let p = ToneParams::default();
        let q = ToneParams {
            offset: [4.0, 4.0],
            ..Default::default()
        };
        let a = rasterize_tile(&flat_source(0.35), (0, 0), &p, 600);
        let b = rasterize_tile(&flat_source(0.35), (0, 0), &p, 600);
        let c = rasterize_tile(&flat_source(0.35), (0, 0), &q, 600);
        let agree = |l: &Tile, r: &Tile| {
            (0..TILE_SIZE)
                .flat_map(|y| (0..TILE_SIZE).map(move |x| (x, y)))
                .filter(|(x, y)| l.pixel(*x, *y)[3] == r.pixel(*x, *y)[3])
                .count()
        };
        let n = TILE_SIZE * TILE_SIZE;
        assert_eq!(
            agree(&a, &b),
            n,
            "identical params must be identical rasters"
        );
        assert!(
            agree(&a, &c) < n * 9 / 10,
            "the offset screen still lines up with the original"
        );
    }

    // --- LP-010 posterization --------------------------------------------

    /// Posterization must collapse a smooth ramp into N flat plateaus. The
    /// source is a horizontal gradient; the number of DISTINCT coverages
    /// across it drops to the step count.
    #[test]
    fn posterize_quantizes_the_ramp() {
        let mut src = Tile::new_transparent();
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let a = (x as f32 / (TILE_SIZE - 1) as f32 * 32768.0) as u16;
                src.set_pixel(x, y, [0, 0, 0, a]);
            }
        }
        // The engine's own view of the source coverage, per column.
        let levels = |p: &ToneParams| {
            let mut v: Vec<u32> = (0..TILE_SIZE)
                .map(|x| (p.ink_at(src.pixel(x, 0)) * 1000.0).round() as u32)
                .collect();
            v.sort_unstable();
            v.dedup();
            v.len()
        };
        let smooth = ToneParams::default();
        let stepped = ToneParams {
            posterize: Some(4),
            ..Default::default()
        };
        assert!(
            levels(&smooth) > 40,
            "the ramp was not continuous to begin with"
        );
        assert_eq!(levels(&stepped), 4, "4 steps means 4 distinct densities");

        // And it still rasterizes: a mid-grey posterized to 4 steps is one of
        // 0, 1/3, 2/3, 1 — never the raw 0.5.
        assert_eq!(posterize(0.5, 4), 2.0 / 3.0);
        assert_eq!(posterize(0.0, 8), 0.0);
        assert_eq!(posterize(1.0, 8), 1.0);
    }

    // --- LP-008 density source -------------------------------------------

    /// The two image-driven modes agree EXACTLY on neutral art (the claim in
    /// `brightness_ink_of`'s doc comment), and diverge on colour — a
    /// saturated blue reads darker by luma than by mean.
    #[test]
    fn brightness_matches_colour_on_greys_and_differs_on_colour() {
        for v in [0u16, 4000, 16384, 32768] {
            for a in [8192u16, 32768] {
                let px = [v.min(a), v.min(a), v.min(a), a];
                let (c, b) = (ink_of(px), brightness_ink_of(px));
                assert!((c - b).abs() < 1e-4, "neutral {px:?}: {c} vs {b}");
            }
        }
        // Opaque saturated blue, premultiplied: mean = 1/3 → 0.667 ink;
        // luma = 0.0722 → 0.928 ink.
        let blue = [0, 0, 32768, 32768];
        assert!((ink_of(blue) - 0.667).abs() < 0.01);
        assert!((brightness_ink_of(blue) - 0.928).abs() < 0.01);
    }

    /// "Specified" ignores what was painted and screens the region at one
    /// flat density — the "40 % tone" fill. Alpha still says WHERE.
    #[test]
    fn specified_density_ignores_the_art() {
        let p = ToneParams {
            density: ToneDensity::Specified(0.4),
            ..Default::default()
        };
        // A near-white source: colour mode reads 8 % ink out of it.
        let mut pale = Tile::new_transparent();
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                pale.set_pixel(x, y, [30000, 30000, 30000, 32768]);
            }
        }
        let control = coverage_of(&rasterize_tile(&pale, (0, 0), &ToneParams::default(), 600));
        assert!(
            control < 0.15,
            "the control: colour mode reads this pale art as light ({control:.3})"
        );
        let c = coverage_of(&rasterize_tile(&pale, (0, 0), &p, 600));
        assert!((c - 0.4).abs() < 0.08, "specified 40 % gave {c:.3}");

        // Transparent stays empty — the density is not a canvas flood.
        let empty = Tile::new_transparent();
        assert!(rasterize_tile(&empty, (0, 0), &p, 600).is_blank());
    }

    // --- the GPU cache-key trap ------------------------------------------

    /// THE TRAP: `gpu::LayerSig` keys the tile cache on [`ToneParams::sig`].
    /// A field that does not move the signature means an edit changes the CPU
    /// raster while the GPU keeps drawing the old tiles. Every field is
    /// perturbed here; add a row when you add a field.
    #[test]
    fn sig_covers_every_field() {
        let base = ToneParams::default();
        let variants = [
            ToneParams {
                pattern: TonePattern::Star,
                ..base
            },
            ToneParams { lpi: 61.0, ..base },
            ToneParams {
                angle_deg: 30.0,
                ..base
            },
            ToneParams {
                offset: [1.0, 0.0],
                ..base
            },
            ToneParams {
                offset: [0.0, 1.0],
                ..base
            },
            ToneParams {
                posterize: Some(4),
                ..base
            },
            ToneParams {
                density: ToneDensity::ImageBrightness,
                ..base
            },
            ToneParams {
                density: ToneDensity::Specified(0.4),
                ..base
            },
            ToneParams {
                density: ToneDensity::Specified(0.5),
                ..base
            },
        ];
        for v in variants {
            assert_ne!(base.sig(), v.sig(), "{v:?} shares the base signature");
        }
        // Distinctness both ways: equal params, equal signature.
        assert_eq!(base.sig(), ToneParams::default().sig());
        // `None` posterization and `Some(0)` must not collide.
        assert_ne!(
            ToneParams {
                posterize: Some(0),
                ..base
            }
            .sig(),
            base.sig()
        );
    }

    // --- old files load unchanged ----------------------------------------

    /// HARD REQUIREMENT: an .ora written before this round carries only the
    /// three original attributes, and must deserialize into params that
    /// rasterize BIT-IDENTICALLY to what it drew then.
    #[test]
    fn pre_round_params_deserialize_to_the_old_behaviour() {
        let legacy = r#"{"pattern":"dots","lpi":60.0,"angle_deg":45.0}"#;
        let p: ToneParams = serde_json::from_str(legacy).expect("old attribute still loads");
        assert_eq!(p, ToneParams::default());
        assert_eq!(p.offset, [0.0, 0.0]);
        assert_eq!(p.posterize, None);
        assert_eq!(p.density, ToneDensity::ImageColour);

        let legacy_lines = r#"{"pattern":"lines","lpi":42.5,"angle_deg":12.0}"#;
        let q: ToneParams = serde_json::from_str(legacy_lines).unwrap();
        assert_eq!(q.pattern, TonePattern::Lines);
        assert!((q.lpi - 42.5).abs() < 1e-6);

        // The pixels, not just the fields: the old ramp is the new default.
        for ink in [0.2f32, 0.5, 0.8] {
            let src = flat_source(ink);
            let old = rasterize_tile(&src, (0, 0), &p, 600);
            let new = rasterize_tile(&src, (0, 0), &ToneParams::default(), 600);
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    assert_eq!(old.pixel(x, y), new.pixel(x, y), "legacy raster moved");
                }
            }
        }
    }
}
