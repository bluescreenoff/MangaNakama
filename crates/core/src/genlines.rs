//! Speed/focus line GENERATION (TRIAGE 140 v1, SF-family): parametric,
//! seeded, deterministic black ink for manga effect lines.
//!
//! v1 is dialog-driven — parameters in, one new layer of hard-edged ink
//! out. CSP's two-driver-curve on-canvas editing (SF-004/005: the blue
//! reference line and the red shape line, editable alone) needs
//! Object-tool curve editing on generator layers and is deferred with
//! reason; the params here are exactly what those two curves will drive.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::tile::{FIX15_ONE, TILE_SIZE, Tile, TileIdx};

/// 集中線 — focus lines: `count` rays converging toward `center`, drawn
/// from `r_in` to `r_out` (jittered per line), each a segment from a
/// jittered angle. Width jitters by `width_jitter` (0..1 of `width`).
#[derive(Clone, Debug)]
pub struct FocusLinesParams {
    pub center: [f32; 2],
    pub r_in: f32,
    pub r_out: f32,
    pub count: u32,
    pub width: f32,
    /// 0..1 — per-line angle jitter as a fraction of the angular gap.
    pub angle_jitter: f32,
    /// 0..1 — per-line width jitter fraction.
    pub width_jitter: f32,
    /// 0..1 — per-line length jitter fraction of (r_out − r_in).
    pub length_jitter: f32,
    pub seed: u64,
}

/// 流線 — speed lines: `count` parallel segments along `angle` degrees,
/// lengths in [len_min, len_max], scattered across the canvas perpendic.
#[derive(Clone, Debug)]
pub struct SpeedLinesParams {
    pub angle_deg: f32,
    pub count: u32,
    pub len_min: f32,
    pub len_max: f32,
    pub width: f32,
    pub seed: u64,
}

/// xorshift64* — small, deterministic, no deps.
fn rand(seed: &mut u64) -> f32 {
    // splitmix64 — full-range, no low-bit correlation (the first xorshift
    // attempt biased the top 24 bits to [0.5, 1) for small seeds).
    *seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

fn put(map: &mut HashMap<TileIdx, Tile>, x: i32, y: i32) {
    if x < 0 || y < 0 {
        return;
    }
    let idx = TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    let tile = map.entry(idx).or_insert_with(Tile::new_transparent);
    let lx = (x - ox) as usize;
    let ly = (y - oy) as usize;
    if lx < TILE_SIZE && ly < TILE_SIZE {
        let o = Tile::offset(lx, ly);
        let d = tile.data_mut();
        d[o] = FIX15_ONE as u16;
        d[o + 1] = FIX15_ONE as u16;
        d[o + 2] = FIX15_ONE as u16;
        d[o + 3] = FIX15_ONE as u16;
    }
}

/// Rasterize one thick segment (a x b, half-width hw) by scanning its
/// bbox and testing point-to-segment distance. Hard edges — speed lines
/// are print black; AA lives in the resample on export.
///
/// The bbox is CLIPPED to the canvas here, not after: the dialog's own
/// maximums (count 512, outer radius 2×width) put a segment's unclipped
/// bbox at ~10^7 pixels on a 600 dpi page — unclipped, the scan was
/// quadratic in the radius and allocated unbounded off-canvas tiles that
/// `retain` only discarded after building (a multi-minute UI hang and a
/// commit spike, from three slider drags).
fn segment(map: &mut HashMap<TileIdx, Tile>, a: [f32; 2], b: [f32; 2], hw: f32, size: (u32, u32)) {
    let d = [b[0] - a[0], b[1] - a[1]];
    let dd = d[0] * d[0] + d[1] * d[1];
    if dd <= f32::EPSILON {
        return;
    }
    let x0 = (a[0].min(b[0]) - hw - 1.0).max(0.0);
    let x1 = (a[0].max(b[0]) + hw + 1.0).min(size.0 as f32);
    let y0 = (a[1].min(b[1]) - hw - 1.0).max(0.0);
    let y1 = (a[1].max(b[1]) + hw + 1.0).min(size.1 as f32);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let hw2 = hw * hw;
    for y in y0.floor() as i32..=y1.ceil() as i32 {
        for x in x0.floor() as i32..=x1.ceil() as i32 {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let t = (((px - a[0]) * d[0] + (py - a[1]) * d[1]) / dd).clamp(0.0, 1.0);
            let qx = a[0] + t * d[0];
            let qy = a[1] + t * d[1];
            let ex = px - qx;
            let ey = py - qy;
            if ex * ex + ey * ey <= hw2 {
                put(map, x, y);
            }
        }
    }
}

/// Render focus lines into sparse tiles (opaque black premul fix15).
pub fn render_focus(p: &FocusLinesParams, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
    let mut map: HashMap<TileIdx, Tile> = HashMap::new();
    let mut seed = p.seed | 1;
    let span = (p.r_out - p.r_in).max(1.0);
    for i in 0..p.count.max(1) {
        let base = i as f32 * std::f32::consts::TAU / p.count.max(1) as f32;
        let ang = base
            + (rand(&mut seed) - 0.5)
                * p.angle_jitter
                * (std::f32::consts::TAU / p.count.max(1) as f32);
        let r1 = p.r_in + rand(&mut seed) * p.length_jitter * span * 0.5;
        let r2 = p.r_out - rand(&mut seed) * p.length_jitter * span * 0.5;
        let w = p.width * (1.0 - rand(&mut seed) * p.width_jitter);
        let (s, c) = ang.sin_cos();
        segment(
            &mut map,
            [p.center[0] + c * r1, p.center[1] + s * r1],
            [p.center[0] + c * r2, p.center[1] + s * r2],
            (w * 0.5).max(0.5),
            size,
        );
    }
    // Clip: drop fully-off-canvas tiles; per-pixel clipping happened in put.
    let (w, h) = (size.0 as i32, size.1 as i32);
    map.retain(|idx, _| {
        let (ox, oy) = idx.origin();
        ox < w && oy < h && ox + TILE_SIZE as i32 > 0 && oy + TILE_SIZE as i32 > 0
    });
    map.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
}

/// Render speed lines: parallel runs scattered across the canvas along
/// `angle_deg`, each starting within the canvas's perpendicular extent.
pub fn render_speed(p: &SpeedLinesParams, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
    let mut map: HashMap<TileIdx, Tile> = HashMap::new();
    let mut seed = p.seed | 1;
    let (w, h) = (size.0 as f32, size.1 as f32);
    let rad = p.angle_deg.to_radians();
    let dir = [rad.cos(), rad.sin()];
    let nrm = [-rad.sin(), rad.cos()];
    // The canvas extent along the normal — lines scatter across it.
    let corners = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for c in corners {
        let t = c[0] * nrm[0] + c[1] * nrm[1];
        lo = lo.min(t);
        hi = hi.max(t);
    }
    for _ in 0..p.count.max(1) {
        let t = lo + rand(&mut seed) * (hi - lo);
        let len = p.len_min + rand(&mut seed) * (p.len_max - p.len_min).max(0.0);
        // Start offset along the direction so the run crosses the canvas.
        // `t` is already the ABSOLUTE normal coordinate (corner
        // projection) — no canvas-centre offset.
        let along = rand(&mut seed) * (w.max(h) + len) - len;
        let base = [
            nrm[0] * t + dir[0] * (along - len * 0.5),
            nrm[1] * t + dir[1] * (along - len * 0.5),
        ];
        let tip = [base[0] + dir[0] * len, base[1] + dir[1] * len];
        segment(&mut map, base, tip, (p.width * 0.5).max(0.5), size);
    }
    let (wi, hi_) = (size.0 as i32, size.1 as i32);
    map.retain(|idx, _| {
        let (ox, oy) = idx.origin();
        ox < wi && oy < hi_ && ox + TILE_SIZE as i32 > 0 && oy + TILE_SIZE as i32 > 0
    });
    map.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink_at(map: &HashMap<TileIdx, Arc<Tile>>, x: i32, y: i32) -> bool {
        let idx = TileIdx::of_pixel(x, y);
        map.get(&idx).is_some_and(|t| {
            t.pixel((x - idx.origin().0) as usize, (y - idx.origin().1) as usize)[3] > 0
        })
    }

    /// Focus lines: ink near the outer ring at many angles, none inside
    /// the inner radius, deterministic under a fixed seed.
    #[test]
    fn focus_lines_ring_and_hole() {
        let p = FocusLinesParams {
            center: [256.0, 256.0],
            r_in: 100.0,
            r_out: 240.0,
            count: 64,
            width: 6.0,
            angle_jitter: 0.5,
            width_jitter: 0.5,
            length_jitter: 0.2,
            seed: 7,
        };
        let m = render_focus(&p, (512, 512));
        // Sectors with ink at r ≈ 200 — each sector samples a short ARC
        // (7 points ±3°) because a single point can fall between two
        // jittered lines.
        let mut sectors = 0;
        for k in 0..32 {
            let base = k as f32 * std::f32::consts::TAU / 32.0;
            let hit = (-3..=3).map(|d| d as f32).any(|d| {
                let a = base + d.to_radians();
                let (s, c) = a.sin_cos();
                ink_at(&m, (256.0 + c * 200.0) as i32, (256.0 + s * 200.0) as i32)
            });
            if hit {
                sectors += 1;
            }
        }
        assert!(sectors >= 26, "most sectors carry ink ({sectors}/32)");
        assert!(!ink_at(&m, 256, 256), "the hole is empty");
        let m2 = render_focus(&p, (512, 512));
        assert_eq!(m.len(), m2.len(), "seeded = deterministic");
    }

    /// Speed lines: horizontal runs at many heights (0° set).
    #[test]
    fn speed_lines_parallel_bands() {
        let p = SpeedLinesParams {
            angle_deg: 0.0,
            count: 80,
            len_min: 100.0,
            len_max: 300.0,
            width: 4.0,
            seed: 3,
        };
        let m = render_speed(&p, (512, 512));
        let mut rows = 0;
        for y in (0..512).step_by(2) {
            if ink_at(&m, 256, y) {
                rows += 1;
            }
        }
        // ~30% of runs cross x=256 at these lengths; each covers ~2 of
        // the 2-px samples → tens of hits.
        assert!(rows >= 20, "many horizontal bands ({rows})");
        // And the spread reaches BOTH halves of the canvas.
        let top = (0..256).step_by(2).any(|y| ink_at(&m, 256, y));
        let bot = (256..512).step_by(2).any(|y| ink_at(&m, 256, y));
        assert!(top && bot, "runs scatter over the full normal extent");
    }
}

// --- SF-004/005 (TRIAGE 140, r85): the generator's parameters persist on
// the layer, so effect lines stay EDITABLE — the dialog reopens with the
// layer's own values and re-applies in place ("a week later" is the
// point). The dialog's (focus, a..d, count, width, jitter, seed) tuple
// is the serialized form; the two render fns remain the raster source.

/// A generated effect-line layer's parameters, as the dialog holds them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenLinesSpec {
    pub focus: bool,
    /// focus: center.x, center.y, r_in, r_out; speed: angle_deg, len_min, len_max (d unused).
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub count: u32,
    pub width: f32,
    pub jitter: f32,
    pub seed: u64,
}

impl GenLinesSpec {
    /// Rasterize the spec into tiles (the shared source with the dialog).
    pub fn render(&self, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
        if self.focus {
            render_focus(
                &FocusLinesParams {
                    center: [self.a, self.b],
                    r_in: self.c,
                    r_out: self.d,
                    count: self.count.max(1),
                    width: self.width,
                    angle_jitter: self.jitter,
                    width_jitter: self.jitter,
                    length_jitter: self.jitter,
                    seed: self.seed,
                },
                size,
            )
        } else {
            render_speed(
                &SpeedLinesParams {
                    angle_deg: self.a,
                    count: self.count.max(1),
                    len_min: self.b,
                    len_max: self.c,
                    width: self.width,
                    seed: self.seed,
                },
                size,
            )
        }
    }
}

#[cfg(test)]
mod spec_tests {
    use super::*;
    use crate::doc::Document;

    /// SF-004/005: the spec persists through ORA and regen renders from
    /// it — a re-applied layer keeps its stack position, the tiles follow
    /// the new params.
    #[test]
    fn spec_round_trips_and_regens_in_place() {
        let mut doc = Document::new(400, 400);
        doc.add_layer("Focus lines");
        let spec = GenLinesSpec {
            focus: true,
            a: 200.0,
            b: 200.0,
            c: 20.0,
            d: 180.0,
            count: 40,
            width: 2.0,
            jitter: 0.2,
            seed: 7,
        };
        let li = doc.layers.len() - 1;
        doc.layers[li].genlines = Some(spec);
        assert!(doc.regen_genlines(li, spec));
        assert!(doc.layers[li].tiles().next().is_some(), "focus lines inked");

        let mut buf = std::io::Cursor::new(Vec::new());
        crate::ora::save_to(&doc, &mut buf).unwrap();
        let re = crate::ora::load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        let gl = re
            .layers
            .iter()
            .position(|l| l.name == "Focus lines")
            .unwrap();
        let g = re.layers[gl].genlines.expect("spec survived");
        assert_eq!(g, spec);

        // Change a param: regen follows, same layer, and the new spec is
        // stored BY the regen (it owns both halves now).
        let mut doc = re;
        let mut s2 = g;
        s2.count = 80;

        assert!(doc.regen_genlines(gl, s2));
        assert!(doc.layers[gl].tiles().next().is_some(), "regen inked");
        assert_eq!(doc.layers[gl].genlines, Some(s2), "the spec went on");
        assert!(
            !doc.regen_genlines(usize::MAX, s2),
            "out-of-bounds index, no regen"
        );
        // A real layer that carries NO spec also refuses (audit H: the
        // old test only exercised the out-of-bounds arm).
        let plain = doc.add_layer("plain");
        assert!(
            !doc.regen_genlines(plain, s2),
            "layer without spec, no regen"
        );
    }

    #[test]
    fn failed_regen_keeps_spec_and_tiles_agreeing() {
        // Audit F, 2026-08-19: a regen that renders nothing must move
        // NEITHER half — the stored spec still describes the pixels that
        // are on screen (the store now happens inside regen_genlines, so
        // this pins both halves rather than the app's old dance).
        let mut doc = Document::new(400, 400);
        doc.add_layer("Focus lines");
        let li = doc.layers.len() - 1;
        let spec = GenLinesSpec {
            focus: true,
            a: 200.0,
            b: 200.0,
            c: 20.0,
            d: 180.0,
            count: 40,
            width: 2.0,
            jitter: 0.2,
            seed: 7,
        };
        doc.layers[li].genlines = Some(spec);
        assert!(doc.regen_genlines(li, spec));
        let tiles_before: Vec<_> = doc.layers[li]
            .tiles()
            .map(|(i, t)| (i, t.clone()))
            .collect();
        assert!(!tiles_before.is_empty());

        // A spec that renders nothing (convergence point far off the
        // canvas — the clip drops every tile): regen refuses, the inked
        // raster stays exactly as it was.
        let mut dead = spec;
        dead.a = -10000.0;
        dead.b = -10000.0;
        assert!(!doc.regen_genlines(li, dead), "nothing rendered, no regen");
        assert_eq!(
            doc.layers[li].genlines,
            Some(spec),
            "the dead spec was not stored"
        );
        let tiles_after: Vec<_> = doc.layers[li]
            .tiles()
            .map(|(i, t)| (i, t.clone()))
            .collect();
        assert_eq!(tiles_before.len(), tiles_after.len(), "tiles unchanged");
        for ((i0, t0), (i1, t1)) in tiles_before.iter().zip(tiles_after.iter()) {
            assert_eq!(i0, i1);
            assert_eq!(t0.data(), t1.data());
        }
    }

    #[test]
    fn regen_is_one_undo_step_and_keeps_the_layers_history() {
        // Audit F's old shape: replace_tiles swapped the raster wholesale,
        // past the copy-on-write recording, so the regen was not undoable
        // and had to purge the layer's pre-images to stay consistent. It
        // now writes through set_tile inside the op bracket — ONE step, and
        // the ink that was on the layer before the regen still undoes.
        let mut doc = Document::new(400, 400);
        let li = doc.add_layer("Focus lines");
        let spec = GenLinesSpec {
            focus: true,
            a: 200.0,
            b: 200.0,
            c: 20.0,
            d: 180.0,
            count: 40,
            width: 2.0,
            jitter: 0.2,
            seed: 7,
        };
        // A first generation, then an ordinary tile write on top of it:
        // two steps for the regen under test to sit above.
        doc.layers[li].genlines = Some(spec);
        assert!(doc.regen_genlines(li, spec));
        doc.begin_op_on(li);
        doc.set_op_label("Stroke");
        doc.layers[li].set_tile(
            crate::tile::TileIdx::new(0, 0),
            Some(std::sync::Arc::new(crate::tile::Tile::default())),
        );
        doc.end_op();
        assert_eq!(doc.undo_labels(), ["Regenerate lines", "Stroke"]);
        let snap = |d: &Document| -> std::collections::BTreeMap<crate::tile::TileIdx, Vec<u16>> {
            d.layers[li]
                .tiles()
                .map(|(i, t)| (i, t.data().to_vec()))
                .collect()
        };
        let before = snap(&doc);

        let mut s2 = spec;
        s2.count = 90;
        s2.seed = 11;
        assert!(doc.regen_genlines(li, s2));
        let regenerated = snap(&doc);
        assert_ne!(before, regenerated, "the regen changed the raster");
        assert_eq!(
            doc.undo_labels(),
            ["Regenerate lines", "Stroke", "Regenerate lines"],
            "one step for the regen, and the older steps survived it"
        );

        assert!(doc.undo(), "the regen undoes");
        assert_eq!(snap(&doc), before, "pixels back, bit for bit");
        assert_eq!(doc.layers[li].genlines, Some(spec), "and the parameters");
        assert!(doc.redo(), "and redoes");
        assert_eq!(snap(&doc), regenerated);
        assert_eq!(doc.layers[li].genlines, Some(s2));

        // The pre-regen history is still walkable: the stroke, then the
        // first generation.
        assert!(doc.undo() && doc.undo(), "back past the stroke");
        assert!(doc.undo(), "back past the first generation");
        assert!(
            doc.layers[li].tiles().next().is_none(),
            "the layer is empty again"
        );
    }
}
