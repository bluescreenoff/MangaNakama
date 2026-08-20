//! Live fill / gradient / tone layers (TRIAGE 137, `NL-001`–`014`).
//!
//! A layer whose CONTENT is parameters, not pixels: a flat colour, a
//! colour ramp, or a screentone at a density — drawn through an attached
//! MASK (the window) that any brush edits (LM-005's alpha scale: a soft
//! brush cuts a soft window). "Editable a week later" is the entire
//! point: the parameters live on the layer, the raster is DERIVED and
//! never baked, and both compositors already read it through
//! `Layer::display_tile` — the same routing the tone raster uses.
//!
//! Destructive painting stays the default for the fill/gradient TOOLS
//! (`NL-006`'s switch lives in the Tool Property panel, app side); this
//! module is the live half.

use crate::doc::{Document, Layer, LayerKind, LayerMask};
use crate::selection::Selection;
use crate::tile::{TILE_LEN, TILE_PIXELS, TILE_SIZE, Tile, TileIdx};
use crate::tone;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// What a live fill layer draws.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum FillKind {
    /// One colour, straight RGBA 0..1.
    Flat { color: [f32; 4] },
    /// CSP グラデ: the ramp `from → to` along `a → b`, projected — the
    /// band extends infinitely perpendicular to the drag, exactly like
    /// the destructive gradient tool.
    ///
    /// `mid` and `opts` are the authored ramp (`crate::gradient`): interior
    /// colour stops, edge process, flip, dithering, centre-out, mixing mode
    /// and mixing rate. Both `#[serde(default)]` and both default to the
    /// behaviour that shipped before they existed, so a file saved by an
    /// older build reloads pixel-identically.
    Gradient {
        a: [f32; 2],
        b: [f32; 2],
        from: [f32; 4],
        to: [f32; 4],
        #[serde(default)]
        mid: crate::gradient::MidStops,
        #[serde(default)]
        opts: crate::gradient::RampOpts,
    },
    /// A screentone at `density` (0..1) — the "editable a week later"
    /// case: density, pattern, frequency and angle are all parameters.
    Tone {
        tone: tone::ToneParams,
        density: f32,
    },
}

impl FillKind {
    pub fn label(&self) -> &'static str {
        match self {
            FillKind::Flat { .. } => "Fill",
            FillKind::Gradient { .. } => "Gradient",
            FillKind::Tone { .. } => "Tone",
        }
    }

    /// Straight RGBA at canvas pixel `(x, y)` — flat and gradient only;
    /// tone goes through `tone::rasterize_tile` (the screen geometry is
    /// continuous across tiles, so it needs the tile origin).
    fn eval(&self, x: i32, y: i32) -> [f32; 4] {
        match self {
            FillKind::Flat { color } => *color,
            FillKind::Gradient {
                a,
                b,
                from,
                to,
                mid,
                opts,
            } => {
                let ab = [b[0] - a[0], b[1] - a[1]];
                let ab2 = ab[0] * ab[0] + ab[1] * ab[1];
                if ab2 < 1e-6 {
                    return *from;
                }
                let px = [x as f32 + 0.5 - a[0], y as f32 + 0.5 - a[1]];
                let u = (px[0] * ab[0] + px[1] * ab[1]) / ab2;
                // "Do not draw" outside the drag = alpha 0; `build_fill_tile`
                // already skips those pixels, so the window stays clear.
                crate::gradient::Ramp::new(*from, *to, *mid, *opts)
                    .eval(u, x, y)
                    .unwrap_or([0.0; 4])
            }
            FillKind::Tone { .. } => [0.0; 4], // unreachable in this path
        }
    }
}

/// A live layer's window from a selection: coverage u8 → fix15, white
/// premul (the existing mask convention). An empty selection yields
/// `None` — no mask = the whole canvas, adjustment-layer style.
pub fn mask_from_selection(doc: &Document, sel: &Selection) -> Option<LayerMask> {
    if sel.is_empty() {
        return None;
    }
    let mut mask = LayerMask {
        tiles: HashMap::new(),
        enabled: true,
        revision: crate::tile::next_revision(),
    };
    let (w, h) = (doc.size.0 as i32, doc.size.1 as i32);
    let t = TILE_SIZE as i32;
    for ty in 0..(h + t - 1) / t {
        for tx in 0..(w + t - 1) / t {
            let idx = TileIdx::new(tx, ty);
            let Some(cov) = sel.tile_mask(idx) else {
                continue;
            };
            let (ox, oy) = idx.origin();
            let mut tile = Tile::new_transparent();
            let d = tile.data_mut();
            for p in 0..TILE_PIXELS {
                let (x, y) = (ox + (p % TILE_SIZE) as i32, oy + (p / TILE_SIZE) as i32);
                if x >= w || y >= h || x < 0 || y < 0 {
                    continue;
                }
                let c = (cov[p] as u32 * 32768 + 127) / 255;
                let c = c.min(32768) as u16;
                d[p * 4] = c;
                d[p * 4 + 1] = c;
                d[p * 4 + 2] = c;
                d[p * 4 + 3] = c;
            }
            mask.tiles.insert(idx, Arc::new(tile));
        }
    }
    if mask.tiles.is_empty() {
        None
    } else {
        Some(mask)
    }
}

/// The derived raster of one fill tile: params × the window mask at `idx`.
fn build_fill_tile(kind: &FillKind, mask_tile: &Tile, idx: TileIdx, dpi: u32) -> Tile {
    match kind {
        FillKind::Tone { tone, density } => {
            // The tone engine eats an INK tile (premul black = coverage). The
            // window coverage is the WHERE; the density is the HOW MUCH, and
            // since `LP-008` shipped that is the engine's own
            // `ToneDensity::Specified` — the fill layer's Density slider is
            // now a view onto it rather than a second implementation of it.
            // (`Specified` multiplies by source alpha, so this is the same
            // arithmetic the hand-rolled version did: coverage × density.)
            let mut p = *tone;
            p.density = tone::ToneDensity::Specified(*density);
            let mut ink = Tile::new_transparent();
            let (si, di) = (ink.data_mut(), mask_tile.data());
            for i in 0..TILE_LEN / 4 {
                di_set(si, i, 0, 0, 0, di[i * 4 + 3]);
            }
            tone::rasterize_tile(&ink, idx.origin(), &p, dpi)
        }
        FillKind::Flat { .. } | FillKind::Gradient { .. } => {
            let (ox, oy) = idx.origin();
            let mut out = Tile::new_transparent();
            let d = out.data_mut();
            let m = mask_tile.data();
            for p in 0..TILE_PIXELS {
                let cov = m[p * 4 + 3] as f32 / 32768.0;
                if cov <= 0.0 {
                    continue;
                }
                let c = kind.eval(ox + (p % TILE_SIZE) as i32, oy + (p / TILE_SIZE) as i32);
                let a = (c[3].clamp(0.0, 1.0) * cov * 32768.0) as u16;
                if a == 0 {
                    continue;
                }
                // Premultiplied: colour scaled by alpha, then fix15.
                let pr = |v: f32| ((v.clamp(0.0, 1.0) * a as f32) as u16).min(a);
                di_set(d, p, pr(c[0]), pr(c[1]), pr(c[2]), a);
            }
            out
        }
    }
}

#[inline]
fn di_set(d: &mut [u16], p: usize, r: u16, g: u16, b: u16, a: u16) {
    d[p * 4] = r;
    d[p * 4 + 1] = g;
    d[p * 4 + 2] = b;
    d[p * 4 + 3] = a;
}

impl Document {
    /// New LIVE fill layer above the active one, carrying `kind`
    /// (TRIAGE 137). The window mask is the current selection when one
    /// exists (`from_selection`), else none — no mask means the whole
    /// canvas, and any brush then edits an implicit full window once a
    /// mask exists (the app arms mask painting on live layers).
    /// Structural like every layer add: clears history. Returns the index.
    pub fn add_fill_layer(&mut self, kind: FillKind, from_selection: bool) -> usize {
        let i = self.add_layer_above(self.active, kind.label());
        let mask = if from_selection {
            self.selection
                .as_ref()
                .and_then(|s| mask_from_selection(self, s))
        } else {
            None
        };
        let l = &mut self.layers[i];
        l.kind = LayerKind::Fill(kind);
        l.mask = mask;
        l.fill_stamp = None; // force a rebuild on the next refresh
        self.refresh_derived(600); // any dpi: tone params carry their own
        self.touch();
        i
    }
}

impl Layer {
    /// Rebuild the derived fill raster when the params or the window mask
    /// moved (the mask's whole-field `revision` is the signal). Mirrors
    /// `refresh_tone`; called from `Document::refresh_derived`. `size` is
    /// the canvas — a layer with NO mask windows the whole canvas
    /// (adjustment-layer convention: deleting the mask does not delete
    /// the fill).
    pub fn refresh_fill(&mut self, dpi: u32, size: (u32, u32)) {
        let kind = match self.kind {
            LayerKind::Fill(k) => k,
            _ => {
                self.fill_tiles = None;
                self.fill_stamp = None;
                return;
            }
        };
        // The canvas size is part of the stamp: a MASKLESS fill windows the
        // whole canvas, so a canvas resize with unchanged params/mask must
        // re-derive — without it, a flat-fill background stayed the old
        // rectangle after Edit ▸ Canvas size, on screen and in exports.
        let stamp = (kind, self.mask.as_ref().map(|m| m.revision), dpi, size);
        if self.fill_stamp == Some(stamp) {
            return;
        }
        let mut out: HashMap<TileIdx, Arc<Tile>> = HashMap::new();
        match &self.mask {
            Some(m) => {
                for (idx, mt) in &m.tiles {
                    let t = build_fill_tile(&kind, mt, *idx, dpi);
                    out.insert(*idx, Arc::new(t));
                }
            }
            None => {
                let (w, h) = (size.0 as i32, size.1 as i32);
                let t = TILE_SIZE as i32;
                for ty in 0..(h + t - 1) / t {
                    for tx in 0..(w + t - 1) / t {
                        let idx = TileIdx::new(tx, ty);
                        let full = full_window_tile(idx, w, h);
                        let ft = build_fill_tile(&kind, &full, idx, dpi);
                        out.insert(idx, Arc::new(ft));
                    }
                }
            }
        }
        self.fill_tiles = Some(out);
        self.fill_stamp = Some(stamp);
    }
}

/// A synthetic whole-coverage window tile (canvas-clipped at the edges —
/// the compositor clips anyway, but the tone engine's ink would not).
fn full_window_tile(idx: TileIdx, w: i32, h: i32) -> Tile {
    let mut t = Tile::new_transparent();
    let (ox, oy) = idx.origin();
    let d = t.data_mut();
    for p in 0..TILE_PIXELS {
        let (x, y) = (ox + (p % TILE_SIZE) as i32, oy + (p / TILE_SIZE) as i32);
        if x >= 0 && y >= 0 && x < w && y < h {
            di_set(d, p, 32768, 32768, 32768, 32768);
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;

    /// Flat fill through a selection-cut window: inside = the colour,
    /// outside = untouched. And a layer with NO mask windows everything.
    #[test]
    fn flat_fill_windows_by_mask() {
        let mut doc = Document::new(128, 128);
        doc.selection = Some(Selection::from_rect(&doc, 16.0, 16.0, 64.0, 64.0));
        let i = doc.add_fill_layer(
            FillKind::Flat {
                color: [1.0, 0.0, 0.0, 1.0],
            },
            true,
        );
        assert!(doc.layers[i].mask.is_some(), "the selection cut a window");
        doc.refresh_derived(600);
        let px = |d: &Document, x: i32, y: i32| crate::export::composite_pixel(d, x, y).unwrap();
        let inside = px(&doc, 40, 40);
        assert!(
            inside[0] > 200 && inside[1] < 60 && inside[2] < 60,
            "{inside:?}"
        );
        let outside = px(&doc, 100, 100);
        assert!(outside == [255, 255, 255], "outside untouched: {outside:?}");

        // Delete the mask: the fill windows the whole canvas instead.
        doc.layers[i].mask = None;
        doc.refresh_derived(600);
        let far = px(&doc, 120, 120);
        assert!(far[0] > 200 && far[1] < 60, "{far:?}");
    }

    /// The gradient ramp follows the drag line, projected (the band is
    /// perpendicular-infinite), matching the destructive tool's math.
    #[test]
    fn gradient_ramp_evaluates_by_projection() {
        let mut doc = Document::new(128, 128);
        doc.add_fill_layer(
            FillKind::Gradient {
                a: [10.0, 64.0],
                b: [118.0, 64.0],
                from: [0.0, 0.0, 1.0, 1.0],
                to: [1.0, 1.0, 0.0, 1.0],
                mid: Default::default(),
                opts: Default::default(),
            },
            false,
        );
        doc.refresh_derived(600);
        let px = |d: &Document, x: i32, y: i32| crate::export::composite_pixel(d, x, y).unwrap();
        let mid = px(&doc, 64, 90); // off the drag line: same projection
        assert!((mid[0] as i32 - mid[2] as i32).abs() < 12, "{mid:?}");
        let blue_end = px(&doc, 12, 64);
        let yellow_end = px(&doc, 116, 64);
        assert!(blue_end[2] > blue_end[0], "start is blue: {blue_end:?}");
        assert!(
            yellow_end[0] > yellow_end[2],
            "end is yellow: {yellow_end:?}"
        );
    }

    /// THE point of TRIAGE 137: the parameters are editable after the
    /// fact — change the tone density (or the flat colour) a week later
    /// and the derived raster follows.
    #[test]
    fn params_stay_editable_after_creation() {
        let mut doc = Document::new(128, 128);
        let i = doc.add_fill_layer(
            FillKind::Flat {
                color: [0.0, 1.0, 0.0, 1.0],
            },
            false,
        );
        doc.refresh_derived(600);
        let px = |d: &Document, x: i32, y: i32| crate::export::composite_pixel(d, x, y).unwrap();
        assert!(px(&doc, 64, 64)[1] > 200, "green");
        doc.layers[i].kind = LayerKind::Fill(FillKind::Flat {
            color: [0.0, 0.0, 1.0, 1.0],
        });
        doc.refresh_derived(600);
        let p = px(&doc, 64, 64);
        assert!(p[2] > 200 && p[1] < 60, "re-derived blue: {p:?}");

        // Tone at density 0 screens to nothing; at 1 it has ink.
        doc.layers[i].kind = LayerKind::Fill(FillKind::Tone {
            tone: crate::tone::ToneParams::default(),
            density: 0.0,
        });
        doc.refresh_derived(600);
        assert_eq!(px(&doc, 64, 64), [255, 255, 255], "density 0 = no ink");
        doc.layers[i].kind = LayerKind::Fill(FillKind::Tone {
            tone: crate::tone::ToneParams::default(),
            density: 1.0,
        });
        doc.refresh_derived(600);
        assert!(px(&doc, 64, 64)[0] < 250, "the screen has ink");
    }

    /// `LP-008` folded the live tone fill's density into the tone engine
    /// (`ToneDensity::Specified`). That refactor must not have moved a single
    /// pixel of anybody's existing file, so this reproduces the PREVIOUS code
    /// verbatim — premultiply the window coverage by the density by hand,
    /// screen the result at face value — and demands the same bytes, through
    /// a soft window as well as a flat one.
    #[test]
    fn live_tone_fill_matches_the_pre_unification_raster() {
        let idx = TileIdx::new(0, 0);
        let flat = full_window_tile(idx, 128, 128);
        // A soft window: coverage ramps across the tile, so the density
        // multiply lands on partial values rather than on 1.0 everywhere.
        let mut soft = Tile::new_transparent();
        {
            let d = soft.data_mut();
            for p in 0..TILE_PIXELS {
                let c = ((p % TILE_SIZE) as u32 * 32768 / (TILE_SIZE as u32 - 1)) as u16;
                di_set(d, p, c, c, c, c);
            }
        }
        for window in [&flat, &soft] {
            for density in [0.1f32, 0.4, 0.75, 1.0] {
                let tone = crate::tone::ToneParams::default();
                let got = build_fill_tile(&FillKind::Tone { tone, density }, window, idx, 600);
                // The pre-unification code, verbatim.
                let mut ink = Tile::new_transparent();
                {
                    let (si, di) = (ink.data_mut(), window.data());
                    for p in 0..TILE_LEN / 4 {
                        let a = ((di[p * 4 + 3] as f32 / 32768.0)
                            * density.clamp(0.0, 1.0)
                            * 32768.0) as u16;
                        di_set(si, p, 0, 0, 0, a);
                    }
                }
                let want = crate::tone::rasterize_tile(&ink, idx.origin(), &tone, 600);
                assert_eq!(got.data(), want.data(), "density {density} moved pixels");
            }
        }
    }

    /// An edit to the window mask (any brush, LM-005) re-derives the fill:
    /// the stamp is keyed on the mask's whole-field revision.
    #[test]
    fn mask_edit_rederives_the_window() {
        let mut doc = Document::new(128, 128);
        let i = doc.add_fill_layer(
            FillKind::Flat {
                color: [1.0, 0.0, 0.0, 1.0],
            },
            false,
        );
        doc.refresh_derived(600);
        // Clear the whole mask field: window gone, fill invisible.
        doc.layers[i].mask = None;
        doc.layers[i].fill_stamp = None; // revision unchanged — force
        doc.refresh_derived(600);
        // Re-seed a small window by hand (a mask edit bumps revision).
        let mut m = LayerMask::default();
        let mut t = Tile::new_transparent();
        let d = t.data_mut();
        for p in 0..TILE_PIXELS {
            d[p * 4] = 32768;
            d[p * 4 + 1] = 32768;
            d[p * 4 + 2] = 32768;
            d[p * 4 + 3] = 32768;
        }
        m.revision = crate::tile::next_revision();
        m.tiles.insert(TileIdx::new(1, 1), Arc::new(t));
        doc.layers[i].mask = Some(m);
        doc.refresh_derived(600);
        let px = |d: &Document, x: i32, y: i32| crate::export::composite_pixel(d, x, y).unwrap();
        assert!(px(&doc, 100, 100)[0] > 200, "the new window shows the fill");
        assert_eq!(
            px(&doc, 10, 10),
            [255, 255, 255],
            "outside the window: clean"
        );
    }
}

#[cfg(test)]
mod ora_tests {
    use super::*;
    use crate::doc::{Document, LayerKind};

    /// A live layer round-trips through ORA: the params as `mnc-fill`, the
    /// window as the persisted mask — and the reload re-derives the same
    /// composite pixels (TRIAGE 137's "editable a week later" must survive
    /// a save).
    #[test]
    fn live_fill_round_trips_through_ora() {
        let mut doc = Document::new(128, 128);
        doc.selection = Some(Selection::from_rect(&doc, 16.0, 16.0, 64.0, 64.0));
        let kind = FillKind::Gradient {
            a: [10.0, 64.0],
            b: [118.0, 64.0],
            from: [0.0, 0.0, 1.0, 1.0],
            to: [1.0, 1.0, 0.0, 1.0],
            mid: Default::default(),
            opts: Default::default(),
        };
        let _ = doc.add_fill_layer(kind, true);
        doc.refresh_derived(600);
        let before = crate::export::composite(&doc, crate::export::Background::Transparent);

        let mut buf = std::io::Cursor::new(Vec::new());
        crate::ora::save_to(&doc, &mut buf).unwrap();
        {
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(buf.get_ref().clone())).unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut z.by_name("stack.xml").unwrap(), &mut s).unwrap();
            assert!(s.contains("mnc-fill="), "params saved: {s}");
            assert!(s.contains("mnc-mask="), "window saved");
        }
        let mut reloaded = crate::ora::load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        reloaded.refresh_derived(600);
        let re = reloaded
            .layers
            .iter()
            .position(|l| matches!(l.kind, LayerKind::Fill(FillKind::Gradient { .. })))
            .expect("the live layer survived as a Fill kind");
        assert!(reloaded.layers[re].mask.is_some(), "the window survived");
        let after = crate::export::composite(&reloaded, crate::export::Background::Transparent);
        // Same visible raster: every pixel of the derived fill matches.
        for (a, b) in before.pixels().zip(after.pixels()).skip(2000).take(4000) {
            assert_eq!(a.0, b.0, "reloaded composite matches");
        }
    }

    /// AN OLD FILE MUST NOT MOVE. `mnc-fill` written before ramp stops
    /// existed carries no `mid`/`opts` keys at all; it has to parse, take
    /// the pre-stops defaults, and derive the SAME pixels — that is the
    /// whole contract behind `#[serde(default)]` on those two fields.
    #[test]
    fn a_pre_stops_gradient_loads_pixel_identically() {
        let old = r#"{"Gradient":{"a":[10.0,64.0],"b":[118.0,64.0],
            "from":[0.0,0.0,1.0,1.0],"to":[1.0,1.0,0.0,1.0]}}"#;
        let loaded: FillKind = serde_json::from_str(old).expect("old params still parse");
        let fresh = FillKind::Gradient {
            a: [10.0, 64.0],
            b: [118.0, 64.0],
            from: [0.0, 0.0, 1.0, 1.0],
            to: [1.0, 1.0, 0.0, 1.0],
            mid: Default::default(),
            opts: Default::default(),
        };
        assert_eq!(loaded, fresh, "the absent keys default to the old ramp");

        let derive = |k: FillKind| {
            let mut d = Document::new(128, 128);
            d.add_fill_layer(k, false);
            d.refresh_derived(600);
            crate::export::composite(&d, crate::export::Background::Transparent)
        };
        let (a, b) = (derive(loaded), derive(fresh));
        assert!(
            a.pixels().zip(b.pixels()).all(|(x, y)| x.0 == y.0),
            "an unedited gradient must reload pixel-for-pixel"
        );
    }

    /// And the new parameters survive a real save/load: an authored ramp
    /// (interior stop + edge process + mixing mode) is worth nothing if it
    /// only lives until the file is closed.
    #[test]
    fn an_authored_ramp_round_trips_through_ora() {
        let mut mid = crate::gradient::MidStops::default();
        mid.insert(crate::gradient::GradStop {
            pos: 0.3,
            color: [1.0, 0.0, 0.0, 0.5],
        });
        let opts = crate::gradient::RampOpts {
            edge: crate::gradient::EdgeProcess::Repeat,
            flip: true,
            dither: true,
            from_center: true,
            mix: crate::gradient::MixMode::Perceptual,
            bright: 3,
            curve: -0.4,
        };
        let kind = FillKind::Gradient {
            a: [10.0, 64.0],
            b: [60.0, 64.0],
            from: [0.0, 0.0, 1.0, 1.0],
            to: [1.0, 1.0, 0.0, 1.0],
            mid,
            opts,
        };
        let mut doc = Document::new(128, 128);
        doc.add_fill_layer(kind, false);
        doc.refresh_derived(600);
        let before = crate::export::composite(&doc, crate::export::Background::Transparent);

        let mut buf = std::io::Cursor::new(Vec::new());
        crate::ora::save_to(&doc, &mut buf).unwrap();
        let mut back = crate::ora::load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        back.refresh_derived(600);
        let found = back
            .layers
            .iter()
            .find_map(|l| match l.kind {
                LayerKind::Fill(k @ FillKind::Gradient { .. }) => Some(k),
                _ => None,
            })
            .expect("the gradient layer survived");
        assert_eq!(found, kind, "every ramp parameter came back");
        let after = crate::export::composite(&back, crate::export::Background::Transparent);
        assert!(
            before.pixels().zip(after.pixels()).all(|(x, y)| x.0 == y.0),
            "and it derives the same raster"
        );
    }
}

#[cfg(test)]
mod stamp_tests {
    use super::*;
    use crate::doc::{Document, LayerKind};

    /// SELF-AUDIT (Opus 0ee84f8's named blind spot): the derive-per-frame
    /// stamp must SKIP nothing-changed frames — proven by Arc identity:
    /// an untouched refresh keeps the SAME derived tile allocation; a
    /// mask-revision bump rebuilds it.
    #[test]
    fn refresh_skips_when_nothing_moved() {
        let mut doc = Document::new(128, 128);
        let i = doc.add_fill_layer(
            FillKind::Flat {
                color: [1.0, 0.0, 0.0, 1.0],
            },
            false,
        );
        doc.refresh_derived(600);
        let key = TileIdx::new(0, 0);
        let before = std::sync::Arc::clone(
            doc.layers[i]
                .fill_tiles
                .as_ref()
                .unwrap()
                .get(&key)
                .unwrap(),
        );
        doc.refresh_derived(600);
        doc.refresh_derived(600);
        let after = std::sync::Arc::clone(
            doc.layers[i]
                .fill_tiles
                .as_ref()
                .unwrap()
                .get(&key)
                .unwrap(),
        );
        assert!(
            std::sync::Arc::ptr_eq(&before, &after),
            "no-op refreshes keep the derived tile"
        );
        doc.layers[i].mask = Some(crate::doc::LayerMask::default()); // rev bump path
        doc.refresh_derived(600);
        let rebuilt = doc.layers[i].fill_tiles.as_ref().unwrap();
        // The mask is now EMPTY: the window closed, the tile is GONE.
        assert!(
            !rebuilt.contains_key(&key),
            "an empty mask window hides everything"
        );
        let _ = LayerKind::Raster;
    }
}
