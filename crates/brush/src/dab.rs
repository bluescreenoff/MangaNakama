//! `SimpleDab` — the placeholder brush engine.
//!
//! **This is scaffolding.** It exists so the walking skeleton can draw before
//! the libmypaint FFI lands. A later agent replaces it with `brush::MyBrush`
//! (libmypaint via `cc`-compiled `vendor/libmypaint`). The only thing that must
//! survive that swap is the boundary: everything below talks to the document
//! through `core::StrokeSink` and writes premultiplied fix15 into `core::Tile`,
//! which is exactly libmypaint's native surface contract. Nothing outside this
//! file knows what a dab is.

use mn_core::{
    Document, FIX15_ONE, PenSample, StrokeSink, TILE_CHANNELS, TILE_SIZE, Tile, TileIdx,
};

/// Round anti-aliased dab brush. Pressure drives radius and alpha.
#[derive(Clone, Debug)]
pub struct SimpleDab {
    /// Straight (non-premultiplied) colour, 0..1 per channel. Default black.
    pub color: [f32; 3],
    /// Radius in canvas px at pressure 0.
    pub min_radius: f32,
    /// Radius in canvas px at pressure 1.
    pub max_radius: f32,
    /// Per-dab alpha multiplier, 0..1.
    pub flow: f32,
    /// Dab spacing as a fraction of the current radius.
    pub spacing: f32,

    pub(crate) prev: Option<PenSample>,
    /// Distance already consumed past the previous dab, carried across segments
    /// so spacing does not reset (and clump) at every input sample.
    carry: f32,
    /// Row 42 (A-014, CSP はみ出さない): the anti-overflow barrier this
    /// stroke paints within — `None` (the default, and every older
    /// stroke) paints as before, bit for bit. Shared by every MN engine
    /// through `base`.
    pub mask: Option<std::sync::Arc<crate::AntiOverflowMask>>,
}

impl Default for SimpleDab {
    fn default() -> Self {
        Self {
            color: [0.0, 0.0, 0.0],
            min_radius: 1.0,
            max_radius: 12.0,
            flow: 0.85,
            spacing: 0.25,
            prev: None,
            carry: 0.0,
            mask: None,
        }
    }
}

impl SimpleDab {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn radius_for(&self, pressure: f32) -> f32 {
        let p = pressure.clamp(0.0, 1.0);
        self.min_radius + (self.max_radius - self.min_radius) * p
    }

    #[inline]
    pub fn alpha_for(&self, pressure: f32) -> f32 {
        (pressure.clamp(0.0, 1.0) * self.flow).clamp(0.0, 1.0)
    }

    /// Stamp one anti-aliased disc, blended source-over in premultiplied fix15.
    ///
    /// Iterates tile-major so `Layer::tile_mut` (the COW + revision-bump path)
    /// is hit once per touched tile per dab, not once per pixel.
    pub fn dab(&self, doc: &mut Document, cx: f32, cy: f32, radius: f32, alpha: f32) {
        let alpha = alpha.clamp(0.0, 1.0);
        if alpha <= 0.0 || radius <= 0.0 || !cx.is_finite() || !cy.is_finite() {
            return;
        }

        // Canvas-pixel bbox, clipped to the document. +1 for the AA fringe.
        let (dw, dh) = (doc.size.0 as i32, doc.size.1 as i32);
        let x0 = ((cx - radius - 1.0).floor() as i32).max(0);
        let y0 = ((cy - radius - 1.0).floor() as i32).max(0);
        let x1 = ((cx + radius + 1.0).ceil() as i32).min(dw - 1);
        let y1 = ((cy + radius + 1.0).ceil() as i32).min(dh - 1);
        if x1 < x0 || y1 < y0 {
            return;
        }

        // Premultiplied fix15 source colour at full coverage.
        let src_full = [
            self.color[0].clamp(0.0, 1.0),
            self.color[1].clamp(0.0, 1.0),
            self.color[2].clamp(0.0, 1.0),
        ];

        let t0 = TileIdx::of_pixel(x0, y0);
        let t1 = TileIdx::of_pixel(x1, y1);
        let layer = doc.active_layer_mut();

        for ty in t0.y..=t1.y {
            for tx in t0.x..=t1.x {
                let idx = TileIdx::new(tx, ty);
                let (ox, oy) = idx.origin();

                // Intersect the dab bbox with this tile, in tile-local coords.
                let lx0 = (x0 - ox).max(0) as usize;
                let ly0 = (y0 - oy).max(0) as usize;
                let lx1 = (x1 - ox).min(TILE_SIZE as i32 - 1) as usize;
                let ly1 = (y1 - oy).min(TILE_SIZE as i32 - 1) as usize;

                let tile = layer.tile_mut(idx);
                blend_disc(
                    tile,
                    (ox, oy),
                    (lx0, ly0, lx1, ly1),
                    (cx, cy),
                    radius,
                    alpha,
                    src_full,
                    self.mask.as_deref(),
                );
            }
        }
    }

    /// Walk `a -> b`, stamping every `radius * spacing` px, interpolating
    /// position/pressure/tilt linearly along the way.
    fn stamp_segment(&mut self, doc: &mut Document, a: PenSample, b: PenSample) {
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len = (dx * dx + dy * dy).sqrt();

        if !len.is_finite() {
            return;
        }

        // Zero-length move (pen held still, pressure changing): stamp in place
        // if we owe a dab, otherwise the stroke would stall.
        if len < 1e-4 {
            if self.carry <= 0.0 {
                let r = self.radius_for(b.pressure);
                self.dab(doc, b.x, b.y, r, self.alpha_for(b.pressure));
                self.carry = (r * self.spacing).max(0.25);
            }
            return;
        }

        let mut d = self.carry;
        let mut guard = 0usize;
        while d <= len {
            let t = d / len;
            let x = a.x + dx * t;
            let y = a.y + dy * t;
            let p = a.pressure + (b.pressure - a.pressure) * t;
            let r = self.radius_for(p);
            self.dab(doc, x, y, r, self.alpha_for(p));

            d += (r * self.spacing).max(0.25);

            // Belt and braces: a NaN radius would loop forever.
            guard += 1;
            if guard > 100_000 {
                break;
            }
        }
        self.carry = d - len;
    }
}

/// Source-over composite of an AA disc into one tile, all in fix15 integers.
///
/// `dst = src + dst * (1 - src_a)`, which is the premultiplied form — the same
/// algebra libmypaint uses, so replacing this brush changes nothing downstream.
#[allow(clippy::too_many_arguments)]
fn blend_disc(
    tile: &mut Tile,
    tile_origin: (i32, i32),
    local: (usize, usize, usize, usize),
    center: (f32, f32),
    radius: f32,
    alpha: f32,
    color: [f32; 3],
    mask: Option<&crate::AntiOverflowMask>,
) {
    let (ox, oy) = tile_origin;
    let (lx0, ly0, lx1, ly1) = local;
    let (cx, cy) = center;
    let data = tile.data_mut();
    let one = FIX15_ONE as u32;

    for ly in ly0..=ly1 {
        let py = oy + ly as i32;
        let ddy = py as f32 + 0.5 - cy;
        let ddy2 = ddy * ddy;
        let row = ly * TILE_SIZE * TILE_CHANNELS;

        for lx in lx0..=lx1 {
            // Row 42: the reference barrier — a blocked pixel is never
            // painted, which is what keeps a scribble inside the lines.
            if let Some(m) = mask
                && m.blocked(ox + lx as i32, py)
            {
                continue;
            }
            let px = ox + lx as i32;
            let ddx = px as f32 + 0.5 - cx;
            let dist = (ddx * ddx + ddy2).sqrt();

            // 1px anti-aliased edge; hard round core.
            let cov = (radius - dist + 0.5).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }

            let sa = alpha * cov;
            let sa15 = (sa * one as f32) as u32;
            if sa15 == 0 {
                continue;
            }
            let inv = one - sa15.min(one);

            let o = row + lx * TILE_CHANNELS;
            for c in 0..3 {
                let s = (color[c] * sa * one as f32) as u32;
                let d = u32::from(data[o + c]);
                data[o + c] = (s + ((d * inv + (one >> 1)) >> 15)).min(u16::MAX as u32) as u16;
            }
            let d = u32::from(data[o + 3]);
            data[o + 3] = (sa15 + ((d * inv + (one >> 1)) >> 15)).min(one) as u16;
        }
    }
}

impl StrokeSink for SimpleDab {
    fn begin(&mut self, _doc: &mut Document) {
        self.prev = None;
        self.carry = 0.0;
    }

    fn sample(&mut self, doc: &mut Document, s: PenSample) {
        match self.prev {
            None => {
                // First sample of the stroke: one dab where the pen touched.
                let r = self.radius_for(s.pressure);
                self.dab(doc, s.x, s.y, r, self.alpha_for(s.pressure));
                self.carry = (r * self.spacing).max(0.25);
            }
            Some(prev) => self.stamp_segment(doc, prev, s),
        }
        self.prev = Some(s);
    }

    fn end(&mut self, doc: &mut Document) {
        self.prev = None;
        self.carry = 0.0;
        doc.revision = mn_core::next_revision();
    }
}

/// Krita GRID engine (TODO #7, the first `mn-engine` sub-tool): hard dots
/// snapped to a square lattice — the stroke's dabs land only ON grid
/// crossings whose cells the path enters. The classic screentone-dot /
/// halftone hand tool.
pub struct GridDab {
    /// Reuses SimpleDab's stamping (colour/radius/flow).
    pub base: SimpleDab,
    /// Lattice pitch, canvas px.
    pub pitch: f32,
    /// Dot radius as a fraction of pitch/2.
    pub dot: f32,
    /// Grid origin, canvas px.
    pub origin: [f32; 2],
    pub(crate) prev: Option<PenSample>,
}

impl Default for GridDab {
    fn default() -> Self {
        Self {
            base: SimpleDab::default(),
            pitch: 12.0,
            dot: 0.35,
            origin: [0.0, 0.0],
            prev: None,
        }
    }
}

impl GridDab {
    /// Clone the engine for a mirror twin (state reset).
    pub fn twin(&self) -> GridDab {
        GridDab {
            base: SimpleDab {
                prev: None,
                carry: 0.0,
                ..self.base.clone()
            },
            pitch: self.pitch,
            dot: self.dot,
            origin: self.origin,
            prev: None,
        }
    }
    /// Stamp one dot at the lattice crossing nearest to (x, y).
    fn dot_at(&self, doc: &mut Document, x: f32, y: f32, pressure: f32) {
        let gx = self.origin[0] + ((x - self.origin[0]) / self.pitch).round() * self.pitch;
        let gy = self.origin[1] + ((y - self.origin[1]) / self.pitch).round() * self.pitch;
        let r = (self.pitch * 0.5 * self.dot.clamp(0.05, 1.0)).max(0.5)
            * (0.5 + 0.5 * pressure.clamp(0.0, 1.0));
        self.base.dab(doc, gx, gy, r, self.base.alpha_for(pressure));
    }

    /// Every lattice crossing inside the cell the segment (a→b) passes
    /// through — the cells' nearest crossings, deduped.
    fn crossings(&self, a: [f32; 2], b: [f32; 2], out: &mut Vec<[f32; 2]>) {
        let steps = (((b[0] - a[0]).abs() + (b[1] - a[1]).abs()) / (self.pitch * 0.5))
            .ceil()
            .max(1.0) as usize;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = a[0] + (b[0] - a[0]) * t;
            let y = a[1] + (b[1] - a[1]) * t;
            let gx = self.origin[0] + ((x - self.origin[0]) / self.pitch).round() * self.pitch;
            let gy = self.origin[1] + ((y - self.origin[1]) / self.pitch).round() * self.pitch;
            if let Some(last) = out.last() {
                if (last[0] - gx).abs() < 0.01 && (last[1] - gy).abs() < 0.01 {
                    continue;
                }
            }
            out.push([gx, gy]);
        }
    }
}

impl StrokeSink for GridDab {
    fn begin(&mut self, _doc: &mut Document) {
        self.prev = None;
    }

    fn sample(&mut self, doc: &mut Document, s: PenSample) {
        let mut pts = Vec::with_capacity(8);
        match self.prev {
            None => pts.push([s.x, s.y]),
            Some(p) => self.crossings([p.x, p.y], [s.x, s.y], &mut pts),
        }
        for [x, y] in pts {
            self.dot_at(doc, x, y, s.pressure);
        }
        self.prev = Some(s);
    }

    fn end(&mut self, doc: &mut Document) {
        self.prev = None;
        doc.revision = mn_core::next_revision();
    }
}

/// Krita HAIRY engine (TODO #7): bristles — a fixed fan of strands
/// around the pen, each stamping its own small dab as the stroke moves.
/// The strand pattern is DETERMINISTIC (golden-angle spokes at fixed
/// radial factors — no RNG state to twin or persist); pressure opens the
/// fan and tapers the outer strands' alpha. Hair, grass, sketchy texture.
pub struct HairyDab {
    /// Reuses SimpleDab's stamping (colour/radius/flow).
    pub base: SimpleDab,
    /// Strand count.
    pub bristles: u16,
    /// Fan radius at pressure 1, canvas px.
    pub spread: f32,
    pub(crate) prev: Option<PenSample>,
    carry: f32,
}

impl Default for HairyDab {
    fn default() -> Self {
        Self {
            base: SimpleDab::default(),
            bristles: 9,
            spread: 9.0,
            prev: None,
            carry: 0.0,
        }
    }
}

/// The golden angle — consecutive spokes spread evenly without symmetry.
const GOLDEN_ANGLE: f32 = 2.399_963_2;

impl HairyDab {
    /// Clone the engine for a mirror twin (state reset).
    pub fn twin(&self) -> HairyDab {
        HairyDab {
            base: SimpleDab {
                prev: None,
                carry: 0.0,
                ..self.base.clone()
            },
            bristles: self.bristles,
            spread: self.spread,
            prev: None,
            carry: 0.0,
        }
    }

    fn strand(&self, i: u16, p: f32) -> ([f32; 2], f32, f32) {
        // (offset, radius factor, alpha factor) — outer strands lay down
        // thinner and lighter.
        let ang = i as f32 * GOLDEN_ANGLE;
        let frac = 0.25 + 0.75 * ((i % 7) as f32 / 6.0);
        let r = self.spread * p.clamp(0.0, 1.0) * frac;
        (
            [ang.cos() * r, ang.sin() * r],
            0.2 + 0.15 * (1.0 - frac),
            1.0 - 0.6 * (frac - 0.25) / 0.75,
        )
    }

    fn stamp(&self, doc: &mut Document, x: f32, y: f32, p: f32) {
        let a = self.base.alpha_for(p);
        for i in 0..self.bristles.max(1) {
            let (off, rf, af) = self.strand(i, p);
            self.base.dab(
                doc,
                x + off[0],
                y + off[1],
                (self.base.radius_for(p) * rf).max(0.4),
                a * af,
            );
        }
    }
}

impl StrokeSink for HairyDab {
    fn begin(&mut self, _doc: &mut Document) {
        self.prev = None;
        self.carry = 0.0;
    }

    fn sample(&mut self, doc: &mut Document, s: PenSample) {
        match self.prev {
            None => {
                self.stamp(doc, s.x, s.y, s.pressure);
                self.carry = self.base.radius_for(s.pressure) * 0.5;
            }
            Some(a) => {
                let (dx, dy) = (s.x - a.x, s.y - a.y);
                let len = (dx * dx + dy * dy).sqrt();
                if !len.is_finite() {
                    return;
                }
                let mut d = self.carry;
                let mut guard = 0usize;
                while d <= len {
                    let t = d / len;
                    let p = a.pressure + (s.pressure - a.pressure) * t;
                    self.stamp(doc, a.x + dx * t, a.y + dy * t, p);
                    d += (self.base.radius_for(p) * 0.5).max(0.5);
                    guard += 1;
                    if guard > 100_000 {
                        break;
                    }
                }
                self.carry = d - len;
            }
        }
        self.prev = Some(s);
    }

    fn end(&mut self, doc: &mut Document) {
        self.prev = None;
        self.carry = 0.0;
        doc.revision = mn_core::next_revision();
    }
}

/// Krita CURVE engine (TODO #7): a small arch of dabs tiled along the
/// path — `steps` dabs along a parabola whose CHORD runs perpendicular
/// to the travel and bows backward. The repeated scallop/stitch texture.
pub struct CurveDab {
    pub base: SimpleDab,
    /// Arch chord width, canvas px.
    pub w: f32,
    /// Bow height, canvas px.
    pub h: f32,
    /// Dabs per arch.
    pub steps: u16,
    pub(crate) prev: Option<PenSample>,
    carry: f32,
}

impl Default for CurveDab {
    fn default() -> Self {
        Self {
            base: SimpleDab::default(),
            w: 14.0,
            h: 5.0,
            steps: 7,
            prev: None,
            carry: 0.0,
        }
    }
}

impl CurveDab {
    pub fn twin(&self) -> CurveDab {
        CurveDab {
            base: SimpleDab {
                prev: None,
                carry: 0.0,
                ..self.base.clone()
            },
            w: self.w,
            h: self.h,
            steps: self.steps,
            prev: None,
            carry: 0.0,
        }
    }

    fn stamp(&self, doc: &mut Document, x: f32, y: f32, dir: f32, p: f32) {
        let a = self.base.alpha_for(p);
        let (ux, uy) = (dir.cos(), dir.sin());
        // Perpendicular chord, bowing against the travel.
        let (px, py) = (-uy, ux);
        for k in 0..self.steps.max(1) {
            let u = (k as f32 + 0.5) / self.steps.max(1) as f32 - 0.5;
            let bow = self.h * (1.0 - (2.0 * u).powi(2));
            let cx = x + px * (u * self.w) - ux * bow;
            let cy = y + py * (u * self.w) - uy * bow;
            self.base
                .dab(doc, cx, cy, (self.base.radius_for(p) * 0.22).max(0.4), a);
        }
    }
}

impl StrokeSink for CurveDab {
    fn begin(&mut self, _doc: &mut Document) {
        self.prev = None;
        self.carry = 0.0;
    }

    fn sample(&mut self, doc: &mut Document, s: PenSample) {
        match self.prev {
            None => {
                self.stamp(doc, s.x, s.y, 0.0, s.pressure);
                self.carry = self.w * 0.75;
            }
            Some(a) => {
                let (dx, dy) = (s.x - a.x, s.y - a.y);
                let len = (dx * dx + dy * dy).sqrt();
                if !len.is_finite() || len < 1e-4 {
                    return;
                }
                let dir = dy.atan2(dx);
                let mut d = self.carry;
                let mut guard = 0usize;
                while d <= len {
                    let t = d / len;
                    let p = a.pressure + (s.pressure - a.pressure) * t;
                    self.stamp(doc, a.x + dx * t, a.y + dy * t, dir, p);
                    d += (self.w * 0.75).max(1.0);
                    guard += 1;
                    if guard > 100_000 {
                        break;
                    }
                }
                self.carry = d - len;
            }
        }
        self.prev = Some(s);
    }

    fn end(&mut self, doc: &mut Document) {
        self.prev = None;
        self.carry = 0.0;
        doc.revision = mn_core::next_revision();
    }
}

/// Krita DYNA engine (TODO #7): a mass-spring pen tip — the drawn point
/// trails the pen on a damped spring (semi-implicit Euler at fixed
/// substeps, dt from the sample clock clamped to sane bounds). Wobbly,
/// delayed, physical lines; the overshoot at direction changes is the
/// point.
pub struct DynaDab {
    pub base: SimpleDab,
    /// Spring stiffness (force per px of pen-tip distance).
    pub k: f32,
    /// Velocity damping.
    pub drag: f32,
    tip: [f32; 2],
    vel: [f32; 2],
    started: bool,
    prev_tip: Option<[f32; 2]>,
    carry: f32,
    last_t: f64,
    /// Where the pen was at the last sample, and how hard — `end()` pins the
    /// spring's target here while it settles.
    last_pen: [f32; 2],
    last_pressure: f32,
}

impl Default for DynaDab {
    fn default() -> Self {
        Self {
            base: SimpleDab::default(),
            k: 90.0,
            drag: 14.0,
            tip: [0.0; 2],
            vel: [0.0; 2],
            started: false,
            prev_tip: None,
            carry: 0.0,
            last_t: 0.0,
            last_pen: [0.0; 2],
            last_pressure: 1.0,
        }
    }
}

impl DynaDab {
    pub fn twin(&self) -> DynaDab {
        DynaDab {
            base: SimpleDab {
                prev: None,
                carry: 0.0,
                ..self.base.clone()
            },
            k: self.k,
            drag: self.drag,
            ..DynaDab::default()
        }
    }

    /// Integrate the tip toward the pen; 4 fixed substeps per sample keep
    /// stiff springs stable at input rates.
    fn integrate(&mut self, pen: [f32; 2], dt: f32) {
        let h = dt / 4.0;
        for _ in 0..4 {
            let ax = self.k * (pen[0] - self.tip[0]) - self.drag * self.vel[0];
            let ay = self.k * (pen[1] - self.tip[1]) - self.drag * self.vel[1];
            self.vel[0] += ax * h;
            self.vel[1] += ay * h;
            self.tip[0] += self.vel[0] * h;
            self.tip[1] += self.vel[1] * h;
        }
    }

    /// Stamp along the TIP path from `a` to the current tip with the standard
    /// carry spacing.
    fn ink_to_tip(&mut self, doc: &mut Document, a: [f32; 2], pressure: f32) {
        let (dx, dy) = (self.tip[0] - a[0], self.tip[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if !len.is_finite() || len <= 0.0 {
            return;
        }
        let mut d = self.carry;
        let mut guard = 0usize;
        while d <= len {
            let t = d / len;
            self.base.dab(
                doc,
                a[0] + dx * t,
                a[1] + dy * t,
                self.base.radius_for(pressure),
                self.base.alpha_for(pressure),
            );
            d += (self.base.radius_for(pressure) * self.base.spacing).max(0.25);
            guard += 1;
            if guard > 100_000 {
                break;
            }
        }
        self.carry = d - len;
    }
}

impl StrokeSink for DynaDab {
    fn begin(&mut self, _doc: &mut Document) {
        self.started = false;
        self.prev_tip = None;
        self.carry = 0.0;
        self.vel = [0.0; 2];
    }

    fn sample(&mut self, doc: &mut Document, s: PenSample) {
        if !self.started {
            self.tip = [s.x, s.y];
            self.vel = [0.0; 2];
            self.started = true;
            self.last_t = s.t_ms;
            self.base.dab(
                doc,
                s.x,
                s.y,
                self.base.radius_for(s.pressure),
                self.base.alpha_for(s.pressure),
            );
            self.prev_tip = Some(self.tip);
            self.carry = self.base.radius_for(s.pressure) * self.base.spacing;
            return;
        }
        let dt = ((s.t_ms - self.last_t) / 1000.0).clamp(1.0 / 240.0, 1.0 / 30.0) as f32;
        self.last_t = s.t_ms;
        self.integrate([s.x, s.y], dt);
        // Stamp along the TIP path (not the pen path) with the standard
        // carry spacing.
        if let Some(a) = self.prev_tip {
            self.ink_to_tip(doc, a, s.pressure);
        }
        self.prev_tip = Some(self.tip);
        self.last_pen = [s.x, s.y];
        self.last_pressure = s.pressure;
    }

    fn end(&mut self, doc: &mut Document) {
        // CODE-MAP seam #4: the interior's rule holds at the boundary too.
        // The tip TRAILS the pen by v·drag/k, so at lift it can still be
        // 100+ px behind the last sample and the stroke would simply stop in
        // mid-air. Settle the spring with its target pinned at that sample —
        // exactly what a dwell would do — inking each settling step, so the
        // line runs INTO the lift point. Mid-stroke feel is untouched.
        if self.started {
            let pen = self.last_pen;
            let p = self.last_pressure;
            // 1/120 s steps; the envelope decays as e^(-drag/2 · t), so the
            // bound is generous headroom, not the expected cost (~0.9 s at
            // the default k/drag), and a stiff-enough preset just stops early.
            for _ in 0..600 {
                let from = self.prev_tip.unwrap_or(self.tip);
                self.integrate(pen, 1.0 / 120.0);
                self.ink_to_tip(doc, from, p);
                self.prev_tip = Some(self.tip);
                let (dx, dy) = (pen[0] - self.tip[0], pen[1] - self.tip[1]);
                let speed = (self.vel[0] * self.vel[0] + self.vel[1] * self.vel[1]).sqrt();
                // Converged: within a fraction of a px AND no longer moving
                // (an underdamped tip flies through the target at speed).
                if (dx * dx + dy * dy).sqrt() < 0.25 && speed < 1.0 {
                    break;
                }
            }
        }
        self.started = false;
        self.prev_tip = None;
        self.carry = 0.0;
        doc.revision = mn_core::next_revision();
    }
}

#[cfg(test)]
mod grid_tests {
    use super::*;
    use mn_core::{Document, PenSample, StrokeSink, TileIdx};

    fn sample(x: f32, y: f32, t: f64) -> PenSample {
        PenSample {
            x,
            y,
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: t,
        }
    }

    /// Dots land ON the lattice: every inked pixel's nearest crossing is a
    /// multiple of pitch from the origin — and a stroke leaves a dotted
    /// line (ink, gaps, ink), not a continuous one.
    #[test]
    fn grid_dab_dots_on_the_lattice() {
        let mut g = GridDab::default();
        g.pitch = 10.0;
        g.dot = 0.4;
        let mut doc = Document::new(128, 128);
        g.begin(&mut doc);
        for i in 0..40 {
            g.sample(
                &mut doc,
                sample(10.0 + i as f32 * 3.0, 64.0, i as f64 * 8.0),
            );
        }
        g.end(&mut doc);

        let mut on_lattice = 0usize;
        let mut total = 0usize;
        // Walk the inked row: pixels cluster at crossings (x ≡ 0 mod 10).
        for (idx, t) in doc.active_layer().tiles() {
            let (ox, _oy) = idx.origin();
            for (i, px) in t.data().chunks_exact(4).enumerate() {
                if px[3] == 0 {
                    continue;
                }
                total += 1;
                let x = ox as f32 + (i % 64) as f32;
                if (x - 10.0 * (x / 10.0).round()).abs() < 2.5 {
                    on_lattice += 1;
                }
            }
        }
        assert!(total > 30, "the stroke inked ({total} px)");
        assert!(
            on_lattice as f32 / total as f32 > 0.9,
            ">=90% of ink sits on lattice crossings ({on_lattice}/{total})"
        );
        // Dotted, not continuous: there are gaps between crossing clusters
        // along the stroke's row.
        let mut runs = 0;
        let mut prev_ink = false;
        // The dots sit on canvas y=60 (the row the lattice snapped to);
        // scan that row of the (0,0) tile.
        let tile = doc
            .active_layer()
            .tile(TileIdx::of_pixel(50, 60))
            .expect("inked");
        for lx in 0..64 {
            let ink = tile.pixel(lx, 60)[3] > 0;
            if ink && !prev_ink {
                runs += 1;
            }
            prev_ink = ink;
        }
        assert!(runs >= 2, "a dotted line has gaps ({runs} runs)");
    }
}

#[cfg(test)]
mod krita_engine_tests {
    use super::*;
    use mn_core::{Document, PenSample, StrokeSink};

    fn sample(x: f32, y: f32, t: f64) -> PenSample {
        PenSample {
            x,
            y,
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: t,
        }
    }

    fn alpha_at(doc: &Document, x: i32, y: i32) -> u16 {
        let ti = TileIdx::of_pixel(x, y);
        doc.active_layer()
            .tile(ti)
            .map(|t| {
                t.pixel(
                    (x - ti.x * TILE_SIZE as i32) as usize,
                    (y - ti.y * TILE_SIZE as i32) as usize,
                )[3]
            })
            .unwrap_or(0)
    }

    /// A horizontal hairy stroke leaves ink ABOVE and BELOW the path —
    /// the bristle fan's strands, offset by design (a plain dab would
    /// only ink the line itself).
    #[test]
    fn hairy_strands_ink_off_axis() {
        let mut doc = Document::new(256, 128);
        let mut h = HairyDab::default();
        doc.begin_op();
        h.begin(&mut doc);
        for i in 0..40 {
            h.sample(
                &mut doc,
                sample(20.0 + i as f32 * 4.0, 64.0, i as f64 * 8.0),
            );
        }
        h.end(&mut doc);
        doc.end_op();
        let path = alpha_at(&doc, 100, 64);
        let above = alpha_at(&doc, 100, 56);
        let below = alpha_at(&doc, 100, 72);
        assert!(path > 0, "the path itself inks");
        assert!(above > 0 && below > 0, "strands reach off-axis");
    }

    /// A horizontal curve stroke leaves ink at MULTIPLE heights across
    /// the path — the arch chord is perpendicular to travel.
    #[test]
    fn curve_arches_span_perpendicular() {
        let mut doc = Document::new(256, 128);
        let mut c = CurveDab::default();
        doc.begin_op();
        c.begin(&mut doc);
        for i in 0..40 {
            c.sample(
                &mut doc,
                sample(20.0 + i as f32 * 4.0, 64.0, i as f64 * 8.0),
            );
        }
        c.end(&mut doc);
        doc.end_op();
        // The chord spans the path line (w=14 → ±7 px).
        assert!(alpha_at(&doc, 100, 58) > 0, "above the path");
        assert!(alpha_at(&doc, 100, 64) > 0, "on the path");
        assert!(alpha_at(&doc, 100, 70) > 0, "below the path");
        assert_eq!(alpha_at(&doc, 100, 40), 0, "beyond the arch");
    }

    /// The dyna tip LAGS the pen (a damped spring cannot keep up at speed:
    /// steady-state lag = v·drag/k ≈ 117 px at 750 px/s) and CATCHES UP
    /// when the pen dwells.
    #[test]
    fn dyna_tip_lags_and_catches_up() {
        let mut doc = Document::new(256, 128);
        let mut d = DynaDab::default();
        doc.begin_op();
        d.begin(&mut doc);
        // Phase 1 — a fast horizontal run to x = 194.
        let mut t = 0.0f64;
        for i in 0..30 {
            d.sample(&mut doc, sample(20.0 + i as f32 * 6.0, 64.0, t));
            t += 8.0;
        }
        // The rightmost ink on the arm sits well LEFT of the pen's last
        // x (194): the measurable lag. Scan a band around y=64.
        let mut rightmost = 0i32;
        for x in (0..256).rev() {
            if (56..=72).any(|y| alpha_at(&doc, x, y) > 0) {
                rightmost = x;
                break;
            }
        }
        assert!(
            rightmost < 160,
            "the tip lags the pen (rightmost ink {rightmost}, pen at 194)"
        );
        // Phase 2 — dwell at the end point; the spring converges and the
        // ink reaches the pen.
        for _ in 0..60 {
            d.sample(&mut doc, sample(194.0, 64.0, t));
            t += 8.0;
        }
        d.end(&mut doc);
        doc.end_op();
        assert!(
            alpha_at(&doc, 194, 64) > 0,
            "the tip caught up on the dwell"
        );
    }

    /// CODE-MAP seam #4 (end conditions exempted from the interior's rule):
    /// a pen that LIFTS mid-run gets no dwell, so the lagging tip used to
    /// freeze wherever it happened to be and the stroke stopped in mid-air.
    /// Measured against the pre-fix code: the pen lifted at x = 194 and the
    /// rightmost ink sat at x = 94 — a 100 px shortfall (~112 px of tip lag,
    /// less the 12 px nib radius). `end()` now settles the spring onto the
    /// last sample, inking the trailing segments; the same run reaches
    /// x = 208 (194 plus the nib).
    #[test]
    fn dyna_end_settles_onto_the_lift_point() {
        let mut doc = Document::new(256, 128);
        let mut d = DynaDab::default();
        doc.begin_op();
        d.begin(&mut doc);
        // A fast horizontal run to x = 194, then the pen LIFTS — no dwell.
        let mut t = 0.0f64;
        for i in 0..30 {
            d.sample(&mut doc, sample(20.0 + i as f32 * 6.0, 64.0, t));
            t += 8.0;
        }
        d.end(&mut doc);
        doc.end_op();

        let mut rightmost = 0i32;
        for x in (0..256).rev() {
            if (48..=80).any(|y| alpha_at(&doc, x, y) > 0) {
                rightmost = x;
                break;
            }
        }
        // The nib centre must land within a couple of px of the lift point;
        // its own 12 px radius then carries the ink past it.
        assert!(
            rightmost >= 192,
            "the stroke reaches the lift point (rightmost ink {rightmost}, pen lifted at 194)"
        );
    }
}

#[cfg(test)]
mod anti_overflow_tests {
    use super::*;
    use mn_core::{Document, TileIdx};

    /// Row 42 (A-014, はみ出さない): a masked dab never paints a blocked
    /// pixel — a scribble may reach AROUND the wall, but the reference's
    /// ink stays exactly as it was.
    #[test]
    fn a_masked_dab_never_paints_blocked_pixels() {
        let mut doc = Document::new(64, 64);
        let mut d = SimpleDab::default();
        d.color = [1.0, 0.0, 0.0];
        let mut allow = vec![255u8; 64 * 64];
        for y in 0..64 {
            allow[y * 64 + 32] = 0;
        }
        d.mask = Some(std::sync::Arc::new(crate::AntiOverflowMask {
            w: 64,
            allow,
        }));
        d.dab(&mut doc, 32.0, 32.0, 8.0, 1.0);
        fn alpha(doc: &Document, x: i32, y: i32) -> u16 {
            let idx = TileIdx::of_pixel(x, y);
            doc.active_layer()
                .tile(idx)
                .map(|t| {
                    let (ox, oy) = idx.origin();
                    t.pixel((x - ox) as usize, (y - oy) as usize)[3]
                })
                .unwrap_or(0)
        }
        assert!(alpha(&doc, 24, 32) > 0, "the near side painted");
        assert_eq!(alpha(&doc, 32, 32), 0, "the wall column is untouched");
        assert!(
            alpha(&doc, 36, 32) > 0,
            "the far side painted around the wall"
        );
        // And unmasked (the default, every stroke before the switch):
        // the same dab paints the wall column too.
        let mut plain = SimpleDab::default();
        plain.color = [1.0, 0.0, 0.0];
        plain.dab(&mut doc, 32.0, 10.0, 8.0, 1.0);
        assert!(alpha(&doc, 32, 10) > 0, "no mask = paint as before");
    }
}
