//! Row 154 (`CL-001`–`014`) — Layer ▸ Convert to lines and tones.
//!
//! The render→page bridge: a greyscale scan / photo / 3D render goes in,
//! MANGA MATERIALS come out — a lineart layer from directional edge
//! detection, an optional ベタ (solid black fill) layer for everything
//! darker than a threshold, and 1–4 screentone layers for the midtones
//! that are left.
//!
//! ## What is derived and what is baked
//!
//! The lines and the ベタ are ink, so they are ordinary raster tiles. The
//! TONES are not: each density band lands as a live [`FillKind::Tone`]
//! fill layer whose window is a layer MASK, so the dots re-derive at the
//! document's dpi (`Document::refresh_derived`) and stay editable a week
//! later — density, pattern, angle and frequency are all still
//! parameters. Baking dots into a raster here would throw that away and
//! print moiré at any other resolution.
//!
//! ## Shape of the result (CL-001)
//!
//! A FOLDER named after the source layer, inserted directly above it,
//! holding (bottom to top) the tone bands darkest-first, the ベタ layer,
//! then the lines. The source is HIDDEN rather than deleted when
//! "keep original layer" is on, and removed when it is off. The whole
//! conversion is ONE structural undo press.

use crate::doc::{Document, Layer, LayerKind, LayerMask};
use crate::fill_layer::FillKind;
use crate::tile::{TILE_SIZE, Tile, TileIdx};
use crate::tone::ToneParams;
use std::collections::HashMap;
use std::sync::Arc;

/// CSP allows a handful of tone bands; four is where the dialog's density
/// row stops being readable and where a printed page stops gaining.
pub const MAX_BANDS: u8 = 4;

/// How much a direction that is switched OFF still contributes (`CL-007`
/// says such edges are detected *weakly*, not that they vanish).
const WEAK_DIRECTION: f32 = 0.25;

/// The maximum magnitude a 3×3 Sobel can return on 0..=255 data — used to
/// normalize the gradient into 0..=1 so the strength knob means the same
/// thing on every image.
const SOBEL_MAX: f32 = 1020.0;

/// The tone half of the dialog (`CL-004`, `CL-011`–`014`).
#[derive(Clone, Debug, PartialEq)]
pub struct ToneOutput {
    /// How many density bands the midtones quantize into (1..=[`MAX_BANDS`]).
    pub bands: u8,
    /// Luma at or above this is PAPER — no band, no dots. Without it the
    /// lightest band would screen the whole white page at its own density.
    pub white_point: f32,
    /// Ink density per band, darkest band first. Empty (or the wrong
    /// length) = derive from each band's own midpoint luma, which is what
    /// [`band_densities`] does.
    pub densities: Vec<f32>,
    /// `CL-014` Type / Angle / Frequency — the same vocabulary as a hand
    /// placed tone (`LP-011`–`013`). The per-layer density overrides
    /// `params.density`, exactly like every other live tone fill.
    pub params: ToneParams,
    /// `CL-013`: flat grey instead of dots (the Type / Angle / Frequency
    /// controls stop meaning anything, and the band becomes a flat fill).
    pub grayscale: bool,
}

impl Default for ToneOutput {
    fn default() -> Self {
        Self {
            bands: 3,
            white_point: 0.92,
            densities: Vec::new(),
            params: ToneParams::default(),
            grayscale: false,
        }
    }
}

/// Everything the dialog collects (`CL-005`–`014`).
#[derive(Clone, Debug, PartialEq)]
pub struct LinesTonesParams {
    /// `CL-006` Strength: how eagerly edges are detected. 1 = every
    /// gradient becomes a line, 0 = only the hardest steps survive.
    pub strength: f32,
    /// `CL-006` Line thickness: the ink is dilated by this many pixels.
    /// 1 is the default because a Sobel response straddles a thin line
    /// rather than landing on it — radius 1 closes that hole.
    pub width: u8,
    /// `CL-008` Line density: a morphological CLOSE at this radius, which
    /// joins broken runs without thickening what is already solid.
    pub join: u8,
    /// `CL-007` Direction of detection, indexed by where the luma gradient
    /// points (dark → light): `[up, right, down, left]`. A direction that
    /// is off is scaled by [`WEAK_DIRECTION`] instead of dropped.
    pub directions: [bool; 4],
    /// `CL-009` Posterize before extracting: flatten the source into N
    /// bands first so edges land on band boundaries. `None` = off.
    pub posterize: Option<u8>,
    /// `CL-010` Black fill threshold: luma at or below this becomes solid
    /// black ink on its own layer (the ベタ half) and is excluded from the
    /// tone bands. `None` = no ベタ layer.
    pub black_fill: Option<f32>,
    /// `CL-004` Tone checkbox: `None` = lines only.
    pub tone: Option<ToneOutput>,
    /// Keep the source layer (hidden) instead of deleting it.
    pub keep_original: bool,
}

impl Default for LinesTonesParams {
    fn default() -> Self {
        Self {
            strength: 0.5,
            width: 1,
            join: 0,
            directions: [true; 4],
            posterize: None,
            black_fill: Some(0.15),
            tone: Some(ToneOutput::default()),
            keep_original: true,
        }
    }
}

/// The luma window `[lo, hi)` band `i` of `bands` covers, between the
/// black-fill floor and the white point.
pub fn band_bounds(bands: u8, lo: f32, white: f32) -> Vec<(f32, f32)> {
    let n = bands.clamp(1, MAX_BANDS) as f32;
    let lo = lo.clamp(0.0, 1.0);
    let white = white.clamp(lo + 1e-3, 1.0);
    let step = (white - lo) / n;
    (0..bands.clamp(1, MAX_BANDS) as usize)
        .map(|i| (lo + step * i as f32, lo + step * (i as f32 + 1.0)))
        .collect()
}

/// The density each band prints at when nobody has overridden it: one
/// minus the band's own midpoint luma, so a 40 % grey screens at 40 % ink.
/// Darkest band first, therefore strictly descending.
pub fn band_densities(bands: u8, lo: f32, white: f32) -> Vec<f32> {
    band_bounds(bands, lo, white)
        .into_iter()
        .map(|(a, b)| (1.0 - (a + b) * 0.5).clamp(0.0, 1.0))
        .collect()
}

impl ToneOutput {
    /// The densities actually used: the authored ones when they match the
    /// band count, else the derived ramp.
    pub fn resolved_densities(&self, lo: f32) -> Vec<f32> {
        let n = self.bands.clamp(1, MAX_BANDS) as usize;
        if self.densities.len() == n {
            self.densities.iter().map(|d| d.clamp(0.0, 1.0)).collect()
        } else {
            band_densities(self.bands, lo, self.white_point)
        }
    }
}

/// The source layer's DISPLAYED pixels as page-sized luma over white
/// paper (0 = black, 255 = paper). Displayed rather than painted so a
/// tone / fill / border-effect layer converts as what the eye sees.
fn read_luma(src: &Layer, w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![255u8; w * h];
    let t = TILE_SIZE;
    for ty in 0..(h + t - 1) / t {
        for tx in 0..(w + t - 1) / t {
            let idx = TileIdx::new(tx as i32, ty as i32);
            let Some(tile) = src.display_tile(idx) else {
                continue;
            };
            let d = tile.data();
            let (ox, oy) = idx.origin();
            for py in 0..t {
                let y = oy as usize + py;
                if y >= h {
                    break;
                }
                for px in 0..t {
                    let x = ox as usize + px;
                    if x >= w {
                        break;
                    }
                    let o = (py * t + px) * 4;
                    let af = d[o + 3] as f32;
                    if af <= 0.0 {
                        continue; // transparent = paper, already 255
                    }
                    let inv = 1.0 / af;
                    let lum = (d[o] as f32 * inv).min(1.0) * 0.2126
                        + (d[o + 1] as f32 * inv).min(1.0) * 0.7152
                        + (d[o + 2] as f32 * inv).min(1.0) * 0.0722;
                    // Composited over white paper: a half-transparent grey
                    // reads as the lighter grey it prints as.
                    let a = (af / 32768.0).clamp(0.0, 1.0);
                    let over = lum * a + (1.0 - a);
                    out[y * w + x] = (over.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
    }
    out
}

/// `CL-009`: snap the source into `levels` flat bands before extraction.
fn posterize_luma(buf: &mut [u8], levels: u8) {
    let steps = (levels.max(2) as f32) - 1.0;
    for v in buf.iter_mut() {
        let f = *v as f32 / 255.0;
        *v = (((f * steps).round() / steps) * 255.0).round() as u8;
    }
}

/// Separable box max (`grow`) or min (`shrink`) over radius `r`.
fn box_morph(buf: &mut [u8], w: usize, h: usize, r: u8, grow: bool) {
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    let r = r as isize;
    let pick = |a: u8, b: u8| if grow { a.max(b) } else { a.min(b) };
    let mut tmp = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut v = buf[y * w + x];
            for d in -r..=r {
                let sx = (x as isize + d).clamp(0, w as isize - 1) as usize;
                v = pick(v, buf[y * w + sx]);
            }
            tmp[y * w + x] = v;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut v = tmp[y * w + x];
            for d in -r..=r {
                let sy = (y as isize + d).clamp(0, h as isize - 1) as usize;
                v = pick(v, tmp[sy * w + x]);
            }
            buf[y * w + x] = v;
        }
    }
}

/// Directional Sobel → line ink, 0..=255 (`CL-006`–`008`).
fn extract_ink(luma: &[u8], w: usize, h: usize, p: &LinesTonesParams) -> Vec<u8> {
    // Strength is the eagerness knob: it lowers the magnitude a gradient
    // has to clear before it inks at all.
    let thr = ((1.0 - p.strength.clamp(0.0, 1.0)) * 0.5).clamp(0.0, 0.98);
    let span = (1.0 - thr).max(1e-3);
    let mut ink = vec![0u8; w * h];
    let at = |x: isize, y: isize| -> f32 {
        let sx = x.clamp(0, w as isize - 1) as usize;
        let sy = y.clamp(0, h as isize - 1) as usize;
        luma[sy * w + sx] as f32
    };
    for y in 0..h {
        for x in 0..w {
            let (xi, yi) = (x as isize, y as isize);
            let (nw, n, ne) = (at(xi - 1, yi - 1), at(xi, yi - 1), at(xi + 1, yi - 1));
            let (we, ea) = (at(xi - 1, yi), at(xi + 1, yi));
            let (sw, s, se) = (at(xi - 1, yi + 1), at(xi, yi + 1), at(xi + 1, yi + 1));
            let gx = (ne + 2.0 * ea + se) - (nw + 2.0 * we + sw);
            let gy = (sw + 2.0 * s + se) - (nw + 2.0 * n + ne);
            let mag = (gx * gx + gy * gy).sqrt() / SOBEL_MAX;
            // Which of the four arrows this edge faces — no atan2 needed,
            // the dominant axis and its sign say it exactly.
            let dir = if gx.abs() >= gy.abs() {
                if gx > 0.0 { 1 } else { 3 }
            } else if gy > 0.0 {
                2
            } else {
                0
            };
            let gate = if p.directions[dir] {
                1.0
            } else {
                WEAK_DIRECTION
            };
            let v = ((mag * gate - thr) / span).clamp(0.0, 1.0);
            ink[y * w + x] = (v * 255.0).round() as u8;
        }
    }
    // CL-008 density: close (grow then shrink) joins broken runs without
    // fattening a run that was already solid. Thickness grows after.
    if p.join > 0 {
        box_morph(&mut ink, w, h, p.join, true);
        box_morph(&mut ink, w, h, p.join, false);
    }
    box_morph(&mut ink, w, h, p.width, true);
    ink
}

/// Which band a luma belongs to, or `None` for ベタ / paper.
fn band_of(luma: f32, bounds: &[(f32, f32)]) -> Option<usize> {
    if bounds.is_empty() {
        return None;
    }
    let (lo, hi) = (bounds[0].0, bounds[bounds.len() - 1].1);
    if luma < lo || luma >= hi {
        return None;
    }
    let i = ((luma - lo) / ((hi - lo) / bounds.len() as f32)) as usize;
    Some(i.min(bounds.len() - 1))
}

#[inline]
fn put(tile: &mut Tile, px: usize, py: usize, rgb: u16, a: u16) {
    let o = (py * TILE_SIZE + px) * 4;
    let d = tile.data_mut();
    d[o] = rgb;
    d[o + 1] = rgb;
    d[o + 2] = rgb;
    d[o + 3] = a;
}

#[inline]
fn to_fix15(cov: u8) -> u16 {
    ((cov as u32 * 32768 + 127) / 255).min(32768) as u16
}

fn into_arcs(m: HashMap<TileIdx, Tile>) -> HashMap<TileIdx, Arc<Tile>> {
    m.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
}

impl Document {
    /// Row 154 (`CL-001`–`014`): convert the layer at `li` into a folder of
    /// manga materials — lines, an optional ベタ fill, and up to four live
    /// tone layers. Returns the new folder's index, or `None` when the
    /// layer is a folder / off the end / produced nothing at all.
    ///
    /// `dpi` is only used to derive the tone rasters immediately; the tone
    /// LAYERS carry their parameters and re-derive at whatever dpi the
    /// document is later printed at.
    ///
    /// Respects the selection when one is active: nothing outside the ants
    /// is read into any output, so converting a photo inside a panel does
    /// not spray tone across the whole page.
    ///
    /// ONE structural undo press for the whole conversion.
    pub fn convert_to_lines_and_tones(
        &mut self,
        li: usize,
        p: &LinesTonesParams,
        dpi: u32,
    ) -> Option<usize> {
        let src = self.layers.get(li)?;
        if src.folder {
            return None;
        }
        let (w, h) = (self.size.0 as usize, self.size.1 as usize);
        if w == 0 || h == 0 {
            return None;
        }
        let name = src.name.clone();
        let depth = src.depth;

        let mut luma = read_luma(src, w, h);
        if let Some(n) = p.posterize {
            posterize_luma(&mut luma, n);
        }
        let ink = extract_ink(&luma, w, h, p);

        let black = p.black_fill.map(|t| t.clamp(0.0, 1.0));
        let bounds = p
            .tone
            .as_ref()
            .map(|t| band_bounds(t.bands, black.unwrap_or(0.0), t.white_point))
            .unwrap_or_default();
        let densities = p
            .tone
            .as_ref()
            .map(|t| t.resolved_densities(black.unwrap_or(0.0)))
            .unwrap_or_default();

        let sel = self.selection.clone();
        let mut line_tiles: HashMap<TileIdx, Tile> = HashMap::new();
        let mut beta_tiles: HashMap<TileIdx, Tile> = HashMap::new();
        let mut band_tiles: Vec<HashMap<TileIdx, Tile>> = vec![HashMap::new(); bounds.len()];
        let t = TILE_SIZE;
        for ty in 0..(h + t - 1) / t {
            for tx in 0..(w + t - 1) / t {
                let idx = TileIdx::new(tx as i32, ty as i32);
                let (ox, oy) = idx.origin();
                for py in 0..t {
                    let y = oy as usize + py;
                    if y >= h {
                        break;
                    }
                    for px in 0..t {
                        let x = ox as usize + px;
                        if x >= w {
                            break;
                        }
                        let cov = match &sel {
                            Some(s) => s.coverage(x as i32, y as i32),
                            None => 255,
                        };
                        if cov == 0 {
                            continue;
                        }
                        let covf = cov as f32 / 255.0;
                        let lf = luma[y * w + x] as f32 / 255.0;

                        let a = ink[y * w + x] as f32 / 255.0 * covf;
                        if a > 0.0 {
                            let tile = line_tiles.entry(idx).or_insert_with(Tile::new_transparent);
                            put(tile, px, py, 0, (a * 32768.0).round() as u16);
                        }
                        if black.is_some_and(|b| lf <= b) {
                            let tile = beta_tiles.entry(idx).or_insert_with(Tile::new_transparent);
                            put(tile, px, py, 0, to_fix15(cov));
                            continue; // ベタ is not also a tone band
                        }
                        if let Some(b) = band_of(lf, &bounds) {
                            let tile =
                                band_tiles[b].entry(idx).or_insert_with(Tile::new_transparent);
                            let c = to_fix15(cov);
                            put(tile, px, py, c, c); // white premul = mask coverage
                        }
                    }
                }
            }
        }

        // Bottom-to-top: the tone bands darkest first, then the ベタ, then
        // the lines on top — the stacking a hand-built page uses.
        let mut kids: Vec<Layer> = Vec::new();
        if let Some(tp) = &p.tone {
            for (i, tiles) in band_tiles.into_iter().enumerate() {
                if tiles.is_empty() {
                    continue;
                }
                let d = densities.get(i).copied().unwrap_or(0.5);
                let mut l = Layer::new(format!("Tone {}%", (d * 100.0).round() as i32));
                l.depth = depth + 1;
                l.kind = LayerKind::Fill(if tp.grayscale {
                    let g = (1.0 - d).clamp(0.0, 1.0);
                    FillKind::Flat {
                        color: [g, g, g, 1.0],
                    }
                } else {
                    FillKind::Tone {
                        tone: tp.params,
                        density: d,
                    }
                });
                l.mask = Some(LayerMask {
                    tiles: into_arcs(tiles),
                    enabled: true,
                    revision: crate::tile::next_revision(),
                });
                l.fill_stamp = None;
                kids.push(l);
            }
        }
        if !beta_tiles.is_empty() {
            let mut l = Layer::new("Black fill");
            l.depth = depth + 1;
            l.replace_tiles(into_arcs(beta_tiles));
            kids.push(l);
        }
        if !line_tiles.is_empty() {
            let mut l = Layer::new("Lines");
            l.depth = depth + 1;
            l.replace_tiles(into_arcs(line_tiles));
            kids.push(l);
        }
        if kids.is_empty() {
            return None; // a blank source: nothing to show for it
        }

        let before = self.stack_snapshot();
        let active_before = self.active;
        let mut at = li + 1;
        for k in kids {
            self.layers.insert(at, k);
            at += 1;
        }
        let mut folder = Layer::new(name);
        folder.folder = true;
        folder.depth = depth;
        self.layers.insert(at, folder);
        let mut folder_at = at;
        if p.keep_original {
            // CSP hides rather than destroys: the scan is still there to
            // re-run the conversion from with other settings.
            self.layers[li].visible = false;
        } else {
            self.layers.remove(li);
            folder_at -= 1;
        }
        self.active = folder_at;
        self.normalize_depths();
        self.record_structure("Convert to lines and tones", before, active_before);
        self.refresh_derived(dpi);
        self.touch();
        Some(folder_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Background, composite, composite_pixel};

    /// Paint the whole layer one luma, then stamp rectangles over it.
    fn paint(doc: &mut Document, li: usize, rect: (i32, i32, i32, i32), luma: f32) {
        let f = crate::blend::f32_to_fix15(luma);
        let one = crate::blend::f32_to_fix15(1.0);
        for y in rect.1..rect.3 {
            for x in rect.0..rect.2 {
                let idx = TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                doc.layers[li].tile_mut(idx).set_pixel(
                    (x - ox) as usize,
                    (y - oy) as usize,
                    [f, f, f, one],
                );
            }
        }
    }

    fn px(doc: &Document, x: i32, y: i32) -> [u8; 3] {
        composite_pixel(doc, x, y).unwrap()
    }

    /// THE composite test (part-19's lesson: tiles-exist proves nothing —
    /// a line layer full of WHITE ink passes that and prints blank paper).
    /// A dark square on a white page, lines only: after the conversion the
    /// source is hidden, so every pixel the reader sees comes from the
    /// generated lineart — the square's BORDER must be dark ink and its
    /// interior must be back to paper.
    #[test]
    fn lines_only_conversion_inks_the_border_in_the_composite() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("scan");
        paint(&mut doc, li, (0, 0, 128, 128), 1.0);
        paint(&mut doc, li, (40, 40, 88, 88), 0.0);
        let p = LinesTonesParams {
            black_fill: None,
            tone: None,
            ..Default::default()
        };
        let f = doc
            .convert_to_lines_and_tones(li, &p, 600)
            .expect("the conversion produced a folder");
        assert!(doc.layers[f].folder, "the result is a folder");
        assert!(!doc.layers[li].visible, "the source is hidden, not deleted");

        // The border: dark ink where the square's edge was.
        let edge = px(&doc, 40, 64);
        assert!(edge[0] < 60, "the left edge inked: {edge:?}");
        let bottom = px(&doc, 64, 87);
        assert!(bottom[0] < 60, "the bottom edge inked: {bottom:?}");
        // The interior: the black square is GONE — lines are an outline.
        let inside = px(&doc, 64, 64);
        assert_eq!(inside, [255, 255, 255], "the interior is paper again");
        // And the untouched page stays paper.
        assert_eq!(px(&doc, 8, 8), [255, 255, 255], "far from any edge");
    }

    /// The ベタ half (`CL-010`) plus the tone quantizer (`CL-011`–`012`),
    /// judged on the COMPOSITE: a very dark region prints solid black, a
    /// mid grey prints a screen whose measured ink lands on the band's
    /// density, and near-white stays paper.
    #[test]
    fn black_fill_is_solid_and_a_grey_band_screens_at_its_density() {
        let mut doc = Document::new(192, 192);
        let li = doc.add_layer("render");
        paint(&mut doc, li, (0, 0, 192, 192), 1.0);
        paint(&mut doc, li, (0, 0, 64, 192), 0.02); // ベタ
        paint(&mut doc, li, (64, 0, 128, 192), 0.5); // one tone band
        let p = LinesTonesParams {
            strength: 0.0,
            width: 0,
            black_fill: Some(0.2),
            tone: Some(ToneOutput {
                bands: 3,
                ..Default::default()
            }),
            ..Default::default()
        };
        let f = doc
            .convert_to_lines_and_tones(li, &p, 600)
            .expect("converted");

        let solid = px(&doc, 20, 96);
        assert_eq!(solid, [0, 0, 0], "the ベタ region is solid black: {solid:?}");
        assert_eq!(px(&doc, 180, 96), [255, 255, 255], "near-white is paper");

        // The 0.5-grey column sits in band 1 of [0.2, 0.92): midpoint 0.56,
        // so it screens at 0.44 ink. Measure the real composite coverage
        // over many screen cells rather than trusting one dot.
        let img = composite(&doc, Background::White);
        let (mut sum, mut n) = (0.0f64, 0u32);
        for y in 40..152 {
            for x in 74..118 {
                sum += 1.0 - img.get_pixel(x, y).0[0] as f64 / 255.0;
                n += 1;
            }
        }
        let measured = sum / n as f64;
        assert!(
            (measured - 0.44).abs() < 0.10,
            "band ink ≈ its density: measured {measured:.3}"
        );

        // The band that exists carries exactly that density as a LIVE tone.
        let kids: Vec<_> = doc.children_range(f).collect();
        let tone_layers: Vec<f32> = kids
            .iter()
            .filter_map(|&i| match doc.layers[i].kind {
                LayerKind::Fill(FillKind::Tone { density, .. }) => Some(density),
                _ => None,
            })
            .collect();
        assert_eq!(tone_layers.len(), 1, "only the occupied band became a layer");
        assert!(
            (tone_layers[0] - 0.44).abs() < 1e-4,
            "density {}",
            tone_layers[0]
        );
        assert!(
            doc.layers[kids[0]].mask.is_some(),
            "the band's window is a MASK, so the dots re-derive at any dpi"
        );
    }

    /// Every occupied band becomes its own layer, densities descending
    /// darkest-first, and each one is a re-derivable tone rather than a
    /// baked raster.
    #[test]
    fn tone_bands_quantize_into_descending_densities() {
        let mut doc = Document::new(256, 128);
        let li = doc.add_layer("ramp");
        // Four vertical strips, one per band of [0.0, 0.92) / 4:
        // 0.00–0.23, 0.23–0.46, 0.46–0.69, 0.69–0.92.
        for (i, l) in [0.10f32, 0.35, 0.55, 0.80].iter().enumerate() {
            let x0 = i as i32 * 64;
            paint(&mut doc, li, (x0, 0, x0 + 64, 128), *l);
        }
        let p = LinesTonesParams {
            strength: 0.0,
            width: 0,
            black_fill: None,
            tone: Some(ToneOutput {
                bands: 4,
                ..Default::default()
            }),
            ..Default::default()
        };
        let f = doc.convert_to_lines_and_tones(li, &p, 600).expect("converted");
        let densities: Vec<f32> = doc
            .children_range(f)
            .filter_map(|i| match doc.layers[i].kind {
                LayerKind::Fill(FillKind::Tone { density, .. }) => Some(density),
                _ => None,
            })
            .collect();
        assert_eq!(densities.len(), 4, "four bands, four tone layers");
        let want = band_densities(4, 0.0, 0.92);
        for (got, exp) in densities.iter().zip(want.iter()) {
            assert!((got - exp).abs() < 1e-4, "{got} vs {exp}");
        }
        assert!(
            densities.windows(2).all(|w| w[0] > w[1]),
            "darkest band first, descending: {densities:?}"
        );
        // Bottom-to-top the darkest band is at the bottom of the folder.
        assert!(densities[0] > 0.8, "the darkest band screens heaviest");
    }

    /// `CL-013` grayscale: the same bands, flat grey instead of dots.
    #[test]
    fn grayscale_tones_are_flat_fills_at_one_minus_the_density() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("render");
        paint(&mut doc, li, (0, 0, 128, 128), 0.5);
        let p = LinesTonesParams {
            strength: 0.0,
            width: 0,
            black_fill: None,
            tone: Some(ToneOutput {
                bands: 3,
                grayscale: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let f = doc.convert_to_lines_and_tones(li, &p, 600).expect("converted");
        let flats: Vec<[f32; 4]> = doc
            .children_range(f)
            .filter_map(|i| match doc.layers[i].kind {
                LayerKind::Fill(FillKind::Flat { color }) => Some(color),
                _ => None,
            })
            .collect();
        assert_eq!(flats.len(), 1);
        // Band 1 of [0, 0.92) spans 0.3067–0.6133: midpoint 0.46, so the
        // density is 0.54 and the flat grey it paints is 0.46.
        assert!((flats[0][0] - 0.46).abs() < 2e-3, "{:?}", flats[0]);
        let p = px(&doc, 64, 64);
        assert!(
            (p[0] as i32 - 117).abs() < 8 && p[0] == p[1] && p[1] == p[2],
            "flat grey in the composite, no dots: {p:?}"
        );
    }

    /// ONE structural undo press takes the whole conversion back —
    /// folder, every child, and the source's visibility.
    #[test]
    fn the_whole_conversion_is_one_undo_press() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("scan");
        paint(&mut doc, li, (0, 0, 128, 128), 1.0);
        paint(&mut doc, li, (30, 30, 90, 90), 0.05);
        let n0 = doc.layers.len();
        let p = LinesTonesParams::default();
        let f = doc.convert_to_lines_and_tones(li, &p, 600).expect("converted");
        assert_eq!(
            doc.layers.len(),
            n0 + 3,
            "the folder plus its ベタ and lines children"
        );
        assert!(!doc.layers[li].visible);
        assert_eq!(doc.active, f);

        assert!(doc.undo(), "one press");
        assert_eq!(doc.layers.len(), n0, "the whole folder went away at once");
        assert!(doc.layers[li].visible, "and the source is visible again");
        assert_eq!(doc.active, li);
    }

    /// Not keeping the original DELETES it (CL-001 is destructive on OK),
    /// and the folder still lands where the layer was.
    #[test]
    fn dropping_the_original_removes_it_and_keeps_one_undo() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("scan");
        paint(&mut doc, li, (0, 0, 128, 128), 1.0);
        paint(&mut doc, li, (30, 30, 90, 90), 0.05);
        let n0 = doc.layers.len();
        let p = LinesTonesParams {
            keep_original: false,
            ..Default::default()
        };
        let f = doc.convert_to_lines_and_tones(li, &p, 600).expect("converted");
        assert!(doc.layers[f].folder);
        assert_eq!(doc.layers[f].name, "scan", "the folder took the name");
        assert!(
            !doc.layers.iter().any(|l| l.name == "scan" && !l.folder),
            "the source layer itself is gone"
        );
        assert!(doc.undo(), "one press");
        assert_eq!(doc.layers.len(), n0);
        assert!(
            doc.layers.iter().any(|l| l.name == "scan" && !l.folder),
            "it came back"
        );
    }

    /// `CL-007`: turning a direction off detects edges facing that way
    /// WEAKLY — a left-facing step keeps its ink when "right" is on and
    /// loses most of it when it is off, while the other edges are
    /// untouched.
    #[test]
    fn direction_toggles_weaken_only_the_edges_facing_that_way() {
        // A page that is dark on the left and light on the right: the luma
        // gradient points RIGHT everywhere on the step.
        let build = |dirs: [bool; 4]| -> u16 {
            let mut doc = Document::new(128, 128);
            let li = doc.add_layer("step");
            paint(&mut doc, li, (0, 0, 128, 128), 1.0);
            paint(&mut doc, li, (0, 0, 64, 128), 0.0);
            let p = LinesTonesParams {
                strength: 1.0,
                width: 0,
                directions: dirs,
                black_fill: None,
                tone: None,
                ..Default::default()
            };
            let f = doc.convert_to_lines_and_tones(li, &p, 600).unwrap();
            let lines = doc
                .children_range(f)
                .find(|&i| doc.layers[i].name == "Lines")
                .expect("a lines layer");
            let idx = TileIdx::of_pixel(64, 64);
            let (ox, oy) = idx.origin();
            doc.layers[lines]
                .tile_arc(idx)
                .map(|t| t.pixel((64 - ox) as usize, (64 - oy) as usize)[3])
                .unwrap_or(0)
        };
        let on = build([true; 4]);
        let off = build([true, false, true, true]); // "right" off
        let others_off = build([false, true, false, false]); // only "right" on
        assert!(on > 0, "the step inked with every direction on");
        assert!(off < on / 2, "right-facing edges came out weak: {off} vs {on}");
        assert_eq!(others_off, on, "the other three arrows do not touch it");
    }

    /// The selection is the conversion's canvas: nothing outside the ants
    /// is read into any output layer.
    #[test]
    fn the_conversion_respects_the_selection() {
        let mut doc = Document::new(192, 192);
        let li = doc.add_layer("scan");
        paint(&mut doc, li, (0, 0, 192, 192), 0.05); // dark everywhere
        doc.selection = Some(crate::selection::Selection::from_rect(
            &doc, 20.0, 20.0, 100.0, 100.0,
        ));
        let p = LinesTonesParams::default();
        let f = doc.convert_to_lines_and_tones(li, &p, 600).expect("converted");
        let beta = doc
            .children_range(f)
            .find(|&i| doc.layers[i].name == "Black fill")
            .expect("a ベタ layer");
        let sample = |d: &Document, x: i32, y: i32| -> u16 {
            let idx = TileIdx::of_pixel(x, y);
            let (ox, oy) = idx.origin();
            d.layers[beta]
                .tile_arc(idx)
                .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize)[3])
                .unwrap_or(0)
        };
        assert!(sample(&doc, 60, 60) > 30000, "inside the ants: filled");
        assert_eq!(sample(&doc, 150, 150), 0, "outside the ants: untouched");
    }

    /// `CL-009` posterize flattens the source first, so edges land on the
    /// band boundaries: a smooth ramp is too gentle to detect at all, and
    /// posterizing it turns exactly its step edges into lines.
    #[test]
    fn posterize_before_extracting_collapses_a_ramp_into_steps() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("ramp");
        for x in 0..128 {
            paint(&mut doc, li, (x, 0, x + 1, 128), x as f32 / 127.0);
        }
        let count = |post: Option<u8>| -> usize {
            let mut d = doc.clone();
            let p = LinesTonesParams {
                width: 0,
                posterize: post,
                black_fill: None,
                tone: None,
                ..Default::default()
            };
            let Some(f) = d.convert_to_lines_and_tones(li, &p, 600) else {
                return 0; // no ink at all: the ramp was never detected
            };
            let Some(lines) = d.children_range(f).find(|&i| d.layers[i].name == "Lines") else {
                return 0;
            };
            let mut n = 0;
            for x in 0..128 {
                let idx = TileIdx::of_pixel(x, 64);
                let (ox, oy) = idx.origin();
                if d.layers[lines]
                    .tile_arc(idx)
                    .map(|t| t.pixel((x - ox) as usize, (64 - oy) as usize)[3])
                    .unwrap_or(0)
                    > 0
                {
                    n += 1;
                }
            }
            n
        };
        let smooth = count(None);
        let stepped = count(Some(4));
        assert_eq!(smooth, 0, "a smooth ramp has no edge to detect");
        // Four levels = three step boundaries, ~2 px of Sobel response each.
        assert!(
            (4..=12).contains(&stepped),
            "only the posterized band boundaries ink: {stepped}"
        );
    }

    /// A folder refuses, and so does a blank layer (nothing to show).
    #[test]
    fn folders_and_blank_layers_refuse() {
        let mut doc = Document::new(64, 64);
        let fi = doc.add_folder_above(doc.active, "group");
        let p = LinesTonesParams::default();
        assert!(doc.convert_to_lines_and_tones(fi, &p, 600).is_none());
        let li = doc.add_layer("blank");
        let n = doc.layers.len();
        let lines_only = LinesTonesParams {
            black_fill: None,
            tone: None,
            ..Default::default()
        };
        assert!(doc.convert_to_lines_and_tones(li, &lines_only, 600).is_none());
        assert_eq!(doc.layers.len(), n, "a refusal changes nothing");
    }
}
