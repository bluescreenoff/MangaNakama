//! Row 53 — mesh transformation (CSP Edit ▸ Transform ▸ メッシュ変形):
//! the layer is lifted into a float (the Transform machinery) and a
//! lattice of `n × n` points is laid over it; dragging a point bends the
//! grid, and commit resamples the lifted bitmap through the deformed
//! quads. Puppet warp (row 54) is the same lattice with pins — deferred
//! to its own round.
//!
//! The resample: for each destination pixel inside the DEFORMED
//! lattice's bounds, find the quad that contains it and invert its
//! bilinear map (Newton, from the quad's centre — two iterations land
//! sub-pixel for the gentle quads a hand makes), then bilinear-sample
//! the source at the recovered position. Premultiplied fix15
//! throughout, so alpha edges interpolate correctly.
//!
//! The buffer hands back straight canvas-space coordinates: the commit
//! path (`transform::commit_transform`'s `resampled` seam) scatters it
//! with an identity-class affine over a rect that covers it.

use crate::transform::FloatSource;

/// The identity lattice over `rect`: `n × n` points, row-major.
pub fn identity_lattice(rect: [i32; 4], n: usize) -> Vec<[f32; 2]> {
    let (x0, y0) = (rect[0] as f32, rect[1] as f32);
    let (w, h) = ((rect[2] - rect[0]) as f32, (rect[3] - rect[1]) as f32);
    let k = (n - 1) as f32;
    let mut v = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            v.push([x0 + w * i as f32 / k, y0 + h * j as f32 / k]);
        }
    }
    v
}

/// Whether `pts` still IS the identity lattice over `rect` (within a
/// half-pixel) — the commit's nothing-moved check.
pub fn lattice_is_identity(rect: [i32; 4], n: usize, pts: &[[f32; 2]]) -> bool {
    let id = identity_lattice(rect, n);
    pts.len() == id.len()
        && pts
            .iter()
            .zip(&id)
            .all(|(a, b)| (a[0] - b[0]).abs() < 0.5 && (a[1] - b[1]).abs() < 0.5)
}

/// Bilinear quad evaluation: the source-cell point `(u, v)` ∈ [0,1]²
/// under the four DEFORMED corners `q` (00, 10, 01, 11 row-major).
fn bilinear_quad(q: &[[f32; 2]; 4], u: f32, v: f32) -> [f32; 2] {
    let (a, b) = (1.0 - u, u);
    [
        q[0][0] * a * (1.0 - v)
            + q[1][0] * b * (1.0 - v)
            + q[2][0] * a * v
            + q[3][0] * b * v,
        q[0][1] * a * (1.0 - v)
            + q[1][1] * b * (1.0 - v)
            + q[2][1] * a * v
            + q[3][1] * b * v,
    ]
}

/// Invert [`bilinear_quad`] for `p` inside quad `q`: Newton from the
/// centre, `iters` rounds with a numerically-differenced Jacobian.
/// Returns the `(u, v)` — outside [0,1]² means "not in this quad".
fn inverse_quad(q: &[[f32; 2]; 4], p: [f32; 2], iters: usize) -> [f32; 2] {
    let mut uv = [0.5f32, 0.5];
    for _ in 0..iters {
        let f = bilinear_quad(q, uv[0], uv[1]);
        let (e, r) = (1.0 / 256.0, 1.0 / 256.0);
        let fx = bilinear_quad(q, (uv[0] + e).min(1.0), uv[1]);
        let fy = bilinear_quad(q, uv[0], (uv[1] + r).min(1.0));
        let j = [
            [(fx[0] - f[0]) / e, (fy[0] - f[0]) / r],
            [(fx[1] - f[1]) / e, (fy[1] - f[1]) / r],
        ];
        let det = j[0][0] * j[1][1] - j[0][1] * j[1][0];
        if det.abs() < 1e-9 {
            break;
        }
        let dx = p[0] - f[0];
        let dy = p[1] - f[1];
        uv[0] += (j[1][1] * dx - j[0][1] * dy) / det;
        uv[1] += (j[0][0] * dy - j[1][0] * dx) / det;
        uv[0] = uv[0].clamp(-0.5, 1.5);
        uv[1] = uv[1].clamp(-0.5, 1.5);
    }
    uv
}

/// Warp the float through the deformed lattice: returns
/// `(dst_rect, premultiplied fix15 RGBA bytes)` — 4 bytes? No: 4 × u16
/// per pixel, `dst_rect`-sized, canvas-space. Transparent where no
/// deformed quad reaches.
pub fn warp_buffer(src: &FloatSource, pts: &[[f32; 2]], n: usize) -> ([i32; 4], Vec<u16>) {
    let rect = src.rect;
    let (x0, y0) = (rect[0] as f32, rect[1] as f32);
    let (w, h) = ((rect[2] - rect[0]) as f32, (rect[3] - rect[1]) as f32);
    let k = (n - 1) as f32;
    // Deformed bounds.
    let mut bb = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for p in pts {
        bb[0] = bb[0].min(p[0]);
        bb[1] = bb[1].min(p[1]);
        bb[2] = bb[2].max(p[0]);
        bb[3] = bb[3].max(p[1]);
    }
    let dst = [
        bb[0].floor() as i32,
        bb[1].floor() as i32,
        bb[2].ceil() as i32,
        bb[3].ceil() as i32,
    ];
    let (dw, dh) = ((dst[2] - dst[0]) as usize, (dst[3] - dst[1]) as usize);
    let mut buf = vec![0u16; dw * dh * 4];
    // Per-cell cached quads.
    let quad = |ci: usize, cj: usize| -> [[f32; 2]; 4] {
        let at = |i: usize, j: usize| pts[j * n + i];
        [at(ci, cj), at(ci + 1, cj), at(ci, cj + 1), at(ci + 1, cj + 1)]
    };
    for cy in dst[1]..dst[3] {
        for cx in dst[0]..dst[2] {
            let p = [cx as f32 + 0.5, cy as f32 + 0.5];
            for cj in 0..n - 1 {
                for ci in 0..n - 1 {
                    let q = quad(ci, cj);
                    // Cheap bbox reject before the Newton.
                    if p[0] < q[0][0].min(q[2][0]) - 1.0
                        || p[0] > q[1][0].max(q[3][0]) + 1.0
                        || p[1] < q[0][1].min(q[1][1]) - 1.0
                        || p[1] > q[2][1].max(q[3][1]) + 1.0
                    {
                        continue;
                    }
                    let uv = inverse_quad(&q, p, 3);
                    if uv[0] < 0.0 || uv[0] > 1.0 || uv[1] < 0.0 || uv[1] > 1.0 {
                        continue;
                    }
                    let sp = [
                        x0 + w * ((ci as f32 + uv[0]) / k),
                        y0 + h * ((cj as f32 + uv[1]) / k),
                    ];
                    let px = sample_frac(src, sp[0] - 0.5, sp[1] - 0.5);
                    let o = ((cy - dst[1]) as usize * dw + (cx - dst[0]) as usize) * 4;
                    buf[o] = px[0].round().clamp(0.0, u16::MAX as f32) as u16;
                    buf[o + 1] = px[1].round().clamp(0.0, u16::MAX as f32) as u16;
                    buf[o + 2] = px[2].round().clamp(0.0, u16::MAX as f32) as u16;
                    buf[o + 3] = px[3].round().clamp(0.0, u16::MAX as f32) as u16;
                    break;
                }
            }
        }
    }
    (dst, buf)
}

/// Bilinear sample of the float at a fractional position, premultiplied
/// fix15 (correct at alpha edges without unpremultiplying).
fn sample_frac(src: &FloatSource, fx: f32, fy: f32) -> [f32; 4] {
    if fx < src.rect[0] as f32 - 1.0
        || fy < src.rect[1] as f32 - 1.0
        || fx >= src.rect[2] as f32
        || fy >= src.rect[3] as f32
    {
        return [0.0; 4];
    }
    let (x0, y0) = (fx.floor() as i32, fy.floor() as i32);
    let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
    let at = |x: i32, y: i32| {
        let p = src.pixel(x, y);
        [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
    };
    let (a, b) = (at(x0, y0), at(x0 + 1, y0));
    let (c, d) = (at(x0, y0 + 1), at(x0 + 1, y0 + 1));
    let mut out = [0.0f32; 4];
    for i in 0..4 {
        let top = a[i] * (1.0 - ax) + b[i] * ax;
        let bot = c[i] * (1.0 - ax) + d[i] * ax;
        out[i] = top * (1.0 - ay) + bot * ay;
    }
    out
}

/// Row 54 — one puppet-warp pin: anchored at `orig` (where it grips the
/// unwarped image), currently at `cur`. A pin whose `cur` has left its
/// `orig` drags its neighbourhood with it; every other pin holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PuppetPin {
    pub orig: [f32; 2],
    pub cur: [f32; 2],
}

/// The puppet-warp lattice: every node takes the UNNORMALIZED
/// Gaussian-weighted sum of the pins' deltas, measured against each
/// pin's ORIGINAL position. σ = one lattice cell: a pin holds its own
/// node EXACTLY (weight 1 at distance 0), the neighbours follow
/// smoothly, and the far corner of the mesh barely moves (a normalized
/// field was tried first and is wrong — with a single pin it drags the
/// whole mesh by the full delta, corners included). Overlapping pins
/// superpose; no pins, or none moved, = the identity lattice.
pub fn puppet_lattice(rect: [i32; 4], n: usize, pins: &[PuppetPin]) -> Vec<[f32; 2]> {
    let mut pts = identity_lattice(rect, n);
    let cell = ((rect[2] - rect[0]).max(rect[3] - rect[1]) as f32) / (n - 1) as f32;
    let s2 = 2.0 * cell * cell;
    for pt in pts.iter_mut() {
        // Distances are measured against the node's IDENTITY position —
        // mutating pt inside the pin loop made later pins see the
        // already-moved node and skewed their weights (the probe that
        // found it is in the git history of this round's notes).
        let base = *pt;
        for pin in pins {
            let d2 = {
                let dx = pin.orig[0] - base[0];
                let dy = pin.orig[1] - base[1];
                dx * dx + dy * dy
            };
            if d2 > 9.0 * s2 {
                continue; // beyond 3σ the weight is noise; skip the exp
            }
            let w = (-d2 / s2).exp();
            pt[0] += w * (pin.cur[0] - pin.orig[0]);
            pt[1] += w * (pin.cur[1] - pin.orig[1]);
        }
    }
    pts
}

/// The affine whose `map_rect(src_rect)` covers the DEFORMED bounds —
/// the commit's destination loop is driven by the affine's rect, and
/// the resampled buffer may reach past the source rect when the mesh
/// stretches outward.
pub fn cover_affine(rect: [i32; 4], pts: &[[f32; 2]]) -> crate::transform::Affine2 {
    let mut bb = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for p in pts {
        bb[0] = bb[0].min(p[0]);
        bb[1] = bb[1].min(p[1]);
        bb[2] = bb[2].max(p[0]);
        bb[3] = bb[3].max(p[1]);
    }
    let u = [
        bb[0].min(rect[0] as f32).floor(),
        bb[1].min(rect[1] as f32).floor(),
        bb[2].max(rect[2] as f32).ceil(),
        bb[3].max(rect[3] as f32).ceil(),
    ];
    let (sx, sy) = (
        (u[2] - u[0]) / (rect[2] - rect[0]) as f32,
        (u[3] - u[1]) / (rect[3] - rect[1]) as f32,
    );
    crate::transform::Affine2 {
        m: [[sx, 0.0], [0.0, sy]],
        t: [u[0] - rect[0] as f32 * sx, u[1] - rect[1] as f32 * sy],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::f32_to_fix15;
    use crate::TileIdx;
    use crate::doc::Document;
    use crate::tile::TILE_SIZE;

    /// Opaque black square on a fresh layer.
    fn ink(doc: &mut Document, li: usize, x0: i32, y0: i32, x1: i32, y1: i32) {
        for y in y0..y1 {
            for x in x0..x1 {
                let idx = TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                let t = doc.layers[li].tile_mut(idx);
                let o = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
                let f = f32_to_fix15(0.0);
                let d = t.data_mut();
                d[o] = f;
                d[o + 1] = f;
                d[o + 2] = f;
                d[o + 3] = f32_to_fix15(1.0);
            }
        }
    }

    fn src(doc: &Document, li: usize, r: [i32; 4]) -> FloatSource {
        crate::transform::lift_region(&doc.layers[li], r, None)
    }

    fn buf_px(dst: [i32; 4], buf: &[u16], x: i32, y: i32) -> u16 {
        let (dw, _) = ((dst[2] - dst[0]) as usize, ());
        let o = ((y - dst[1]) as usize * dw + (x - dst[0]) as usize) * 4 + 3;
        buf.get(o).copied().unwrap_or(0)
    }

    #[test]
    fn an_identity_lattice_copies_the_source_in_place() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("l");
        ink(&mut doc, li, 40, 40, 60, 60);
        let r = [32, 32, 72, 72];
        let s = src(&doc, li, r);
        let pts = identity_lattice(r, 5);
        let (dst, buf) = warp_buffer(&s, &pts, 5);
        assert_eq!(dst, r, "identity bounds");
        assert!(buf_px(dst, &buf, 50, 50) == f32_to_fix15(1.0), "ink kept");
        assert_eq!(buf_px(dst, &buf, 35, 35), 0, "paper kept");
    }

    #[test]
    fn one_dragged_point_bends_only_its_neighbourhood() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("l");
        ink(&mut doc, li, 40, 40, 60, 60);
        let r = [32, 32, 72, 72];
        let s = src(&doc, li, r);
        let mut pts = identity_lattice(r, 5);
        // The centre point (2,2) drags right by 30 — a hard pull on a
        // coarse lattice still only bends its own neighbourhood.
        pts[2 * 5 + 2][0] += 30.0;
        let (dst0, buf0) = warp_buffer(&s, &identity_lattice(r, 5), 5);
        let (dst, buf) = warp_buffer(&s, &pts, 5);
        // The dragged point pulls the neighbourhood's mass WITH it: the
        // ink centroid (alpha-weighted) moves east, and the far-west ink
        // (outside every deformed cell's influence) stays put.
        let centroid = |d: [i32; 4], b: &[u16]| -> f32 {
            let (mut m, mut wsum) = (0.0f32, 0.0f32);
            for y in d[1]..d[3] {
                for x in d[0]..d[2] {
                    let a = buf_px(d, b, x, y) as f32;
                    m += x as f32 * a;
                    wsum += a;
                }
            }
            m / wsum.max(1.0)
        };
        let (c0, c1) = (centroid(dst0, &buf0), centroid(dst, &buf));
        assert!(
            c1 - c0 > 0.8,
            "the bar's mass moved toward the drag: {c0} → {c1}"
        );
        let west = buf_px(dst, &buf, 41, 50);
        assert!(west > 30000, "far-west ink barely moved ({west})");
    }

    #[test]
    fn the_cover_affine_reaches_the_deformed_bounds() {
        let r = [10, 10, 50, 50];
        let mut pts = identity_lattice(r, 3);
        for p in pts.iter_mut() {
            p[0] += 20.0;
        }
        let xf = cover_affine(r, &pts);
        let m = xf.map_rect([r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32]);
        assert!(m[0] <= 29.0 && m[2] >= 69.0, "covers the shifted mesh: {m:?}");
        assert!(m[1] <= 10.0 && m[3] >= 50.0);
    }

    /// Row 54 — the puppet field: a pin drags its neighbourhood with it,
    /// holds nearly exactly at its own position, and leaves far corners
    /// alone; two pins pulling apart tear between themselves.
    #[test]
    fn puppet_pins_drag_their_neighbourhood_and_hold() {
        let r = [0, 0, 80, 80];
        let pin = |o: [f32; 2], c: [f32; 2]| PuppetPin { orig: o, cur: c };
        // One pin at the centre node (40,40), dragged 20 east.
        let pts = puppet_lattice(r, 5, &[pin([40.0, 40.0], [60.0, 40.0])]);
        let at = |i: usize, j: usize| pts[j * 5 + i];
        assert!(
            (at(2, 2)[0] - 60.0).abs() < 1.5,
            "the node under the pin follows it: {:?}",
            at(2, 2)
        );
        let corner = at(0, 0);
        assert!(
            (corner[0] - 0.0).abs() < 3.0,
            "the far corner barely moves: {corner:?}"
        );
        // No pins / no moved pins = identity.
        assert!(lattice_is_identity(r, 5, &puppet_lattice(r, 5, &[])));
        let idle = puppet_lattice(r, 5, &[pin([40.0, 40.0], [40.0, 40.0])]);
        assert!(lattice_is_identity(r, 5, &idle), "an unmoved pin bends nothing");
        // Two pins pulling apart: the midpoint between them splits the
        // difference instead of following either.
        let pts = puppet_lattice(
            r,
            5,
            &[pin([20.0, 40.0], [0.0, 40.0]), pin([60.0, 40.0], [80.0, 40.0])],
        );
        let mid = pts[2 * 5 + 2];
        assert!(
            (mid[0] - 40.0).abs() < 2.0,
            "the middle node splits the two pulls: {mid:?}"
        );
        let right_node = pts[2 * 5 + 3];
        assert!(
            (right_node[0] - 77.0).abs() < 2.0,
            "the node ON the right pin follows it: {right_node:?}"
        );
    }

    #[test]
    fn identity_checks() {
        let r = [0, 0, 40, 40];
        let pts = identity_lattice(r, 4);
        assert!(lattice_is_identity(r, 4, &pts));
        let mut moved = pts.clone();
        moved[5][1] += 3.0;
        assert!(!lattice_is_identity(r, 4, &moved));
    }
}
