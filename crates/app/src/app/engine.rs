//! The stroke engine: the brush kinds, the symmetry/wrap twins, and the
//! GPU dab-stroke state.

use super::*;

/// What actually makes pixels.
///
/// libmypaint (`MyBrush`) is the brush; `SimpleDab` is only reached when the
/// default preset cannot be loaded (missing/corrupt `assets/`), so the app still
/// draws instead of dying. Both are `core::StrokeSink`s, so everything upstream
/// — the stabilizer, the input code, undo — is identical either way.
/// One make of stroke engine. What actually makes pixels.
///
/// libmypaint (`My`) is the brush; `SimpleDab` is only reached when the
/// default preset cannot be loaded (missing/corrupt `assets/`), so the app
/// still draws instead of dying. Both are `core::StrokeSink`s, so everything
/// upstream — the stabilizer, the input code, undo — is identical either way.
pub enum EngineKind {
    My(Box<MyBrush>),
    Dab(SimpleDab),
    Grid(GridDab),
    Hairy(HairyDab),
    Curve(CurveDab),
    Dyna(DynaDab),
}

/// The engine a preset asks for through its optional `mn-engine` key:
/// grid/hairy/curve/dyna are procedural, so they get their own engine instead
/// of the MyPaint one (a sub-tool identity without a second preset format).
/// `None` = an ordinary `.myb` for `MyBrush::load`.
///
/// `cmd.rs`'s `SelectBrush` reads the same key to build the LIVE engine — the
/// two must stay in step, or a preset draws as one brush and previews as
/// another. The reading half is [`mn_brush::preset_engine_key`]; this is the
/// only place that turns the name into an engine.
///
/// An UNKNOWN name is `None`, not an error: a preset from a newer build falls
/// back to the MyPaint path rather than leaving the sub tool with no engine.
pub fn preset_engine(path: &Path) -> Option<EngineKind> {
    match mn_brush::preset_engine_key(path)?.as_str() {
        "grid" => Some(EngineKind::Grid(GridDab::default())),
        "hairy" => Some(EngineKind::Hairy(HairyDab::default())),
        "curve" => Some(EngineKind::Curve(CurveDab::default())),
        "dyna" => Some(EngineKind::Dyna(DynaDab::default())),
        _ => None,
    }
}

/// App-side GPU dab stroke state (the Renderer holds its own; this is the
/// repair list + counters the HUD shows).
pub struct DabStrokeApp {
    // No `layer` here on purpose: `Renderer::end_dab_stroke` returns the
    // stroke's layer alongside its touched tiles, and a second copy could
    // drift from it.
    /// Every dab drained this stroke — the CPU repair input if the canary
    /// reports a dropped dispatch (the cursed-driver defense).
    pub all_dabs: Vec<mn_core::dab::DabParams>,
    pub hard: bool,
    pub flushes: u32,
    /// WASH stroke (#0.1): dabs rasterize into the GPU sentinel wash
    /// buffer; the stroke-end commit runs the CPU `commit_wash` math on
    /// the readback.
    pub wash: bool,
    /// SMUDGE stroke (#0.1 part 3): dabs dispatch per input sample (the
    /// sampler's visibility granularity must match the CPU path's
    /// end_atomic) and the surface's tile oracle serves `get_color` from
    /// the GPU tile cache.
    pub smudge: bool,
}

/// The smudge sampler's GPU tile fetch (#0.1 part 3). The ctx is the
/// `Box<(*mut Renderer, layer)>` installed at `begin_stroke`; valid for the
/// stroke's duration (the engine is single-threaded and `&mut self.renderer`
/// is only taken outside C calls), freed in `finish_gpu_dab_stroke`.
/// Serves the freshest tile — CPU seed ⊕ every dispatched dab — or declines
/// (no cache entry = the stroke never touched this tile = the CPU tile is
/// current and the surface's fallback copy is exact).
pub(crate) fn smudge_tile_oracle(
    ctx: *mut core::ffi::c_void,
    tx: core::ffi::c_int,
    ty: core::ffi::c_int,
    dest: &mut [u16],
) -> bool {
    let (rptr, layer) = unsafe { &*(ctx as *const (*mut mn_gpu::Renderer, usize)) };
    let Some(px) = unsafe { &mut **rptr }.readback_dab_tile(*layer, mn_core::TileIdx::new(tx, ty))
    else {
        return false;
    };
    dest.copy_from_slice(&px);
    true
}

/// How one twin transforms a sample's axes. `Mirror` reflects around the
/// canvas centre (symmetry painting); `Wrap` shifts by ±canvas-size, so dabs
/// within a radius of an edge continue on the opposite side — seamless
/// tiling at the borders (Krita's wrap-around mode, the clothing/wall
/// texture case).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TwinAxis {
    Mirror,
    Wrap,
}

/// One transformed copy of the stroke: `None` on an axis leaves it alone.
/// A symmetric-ruler twin carries a full affine instead (`xf`) — the axis
/// flags are the canvas-centred mirror/wrap pair, kept for those uses.
pub(crate) struct StrokeTwin {
    pub(crate) kind: EngineKind,
    pub(crate) x: Option<TwinAxis>,
    pub(crate) y: Option<TwinAxis>,
    pub(crate) xf: Option<mn_core::Affine2>,
}

/// The 2N−1 mirror images of a stroke under a symmetric ruler's dihedral
/// group: rotations by 2πk/N (k = 1..N) plus a reflection across EACH of
/// the N axes (angle0 + kπ/N). The identity (the user's own stroke) is the
/// main engine and is not duplicated. `p`'s tilt is left untouched, same
/// as the axis twins.
pub(crate) fn symmetric_affines(r: &mn_core::Ruler) -> Vec<mn_core::Affine2> {
    let mn_core::Ruler::Symmetric { c, lines, angle0 } = *r else {
        return Vec::new();
    };
    let n = lines.max(1) as i32;
    let about = |m: [[f32; 2]; 2]| mn_core::Affine2 {
        m,
        t: [
            c[0] - (m[0][0] * c[0] + m[0][1] * c[1]),
            c[1] - (m[1][0] * c[0] + m[1][1] * c[1]),
        ],
    };
    let mut out = Vec::with_capacity((2 * n - 1) as usize);
    for k in 1..n {
        let rad = k as f32 * std::f32::consts::TAU / n as f32;
        let (s, co) = rad.sin_cos();
        out.push(about([[co, -s], [s, co]]));
    }
    for k in 0..n {
        let phi = angle0 + k as f32 * std::f32::consts::PI / n as f32;
        let (s2, c2) = ((2.0 * phi).sin(), (2.0 * phi).cos());
        out.push(about([[c2, s2], [s2, -c2]]));
    }
    out
}

/// The stroke engine: the user's brush, plus — when symmetry painting or
/// wrap-around tiling is on — up to three transformed twins fed the same
/// post-stabilizer samples. All configuration fans out to the twins, so a
/// copy is always the same brush with the same settings.
pub struct Engine {
    main: EngineKind,
    twins: Vec<StrokeTwin>,
    /// The kind's own dab DIAMETER in canvas px, read once at construction —
    /// the DEFAULT a sub tool starts at, NOT a ceiling. Only the fallback
    /// kinds need it remembered; `MyBrush` keeps its own shipped radius and
    /// re-derives from that (see `MyBrush::base_size_px`).
    base_px: f32,
}

impl Engine {
    pub fn new(kind: EngineKind) -> Engine {
        let mut e = Engine {
            main: kind,
            twins: Vec::new(),
            base_px: 0.0,
        };
        // Every kind is fresh here (a preset load or a `new()`), so its live
        // radius IS its base — captured before any size edit can move it.
        e.base_px = e.radius_px() * 2.0;
        e
    }

    /// The user's own engine (readbacks and preset identity come from here).
    pub fn kind(&self) -> &EngineKind {
        &self.main
    }

    /// Swap the whole twin set (symmetry/wrap toggles rebuild them from the
    /// current preset + props; empty = plain single-stroke painting).
    pub(crate) fn set_twins(&mut self, twins: Vec<StrokeTwin>) {
        self.twins = twins;
    }

    /// Apply Tool Property values to the main engine AND every twin, through
    /// the per-kind guarded path — a fresh preset twin and the live main
    /// both end up configured correctly (see `EngineKind::apply_props`).
    pub fn apply_props_all(
        &mut self,
        p: &crate::cmd::ToolProps,
        texture_mask: Option<&std::sync::Arc<mn_brush::TextureMask>>,
    ) {
        self.main.apply_props(p, texture_mask);
        for t in &mut self.twins {
            t.kind.apply_props(p, texture_mask);
        }
    }

    /// LM-004: route strokes to the mask on every MyPaint engine.
    pub fn set_mask_mode_all(&mut self, on: bool) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_mask_mode(on);
            }
        });
    }

    /// Route strokes to the document's selection scratch on every MyPaint
    /// engine (selection pen / eraser / Quick Mask).
    pub fn set_sel_mode_all(&mut self, on: bool) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_sel_mode(on);
            }
        });
    }

    /// Row 42 (A-014, はみ出さない): arm the anti-overflow barrier on
    /// every engine — the MyPaint surface snapshot/restores around C's
    /// dabs, the MN engines skip blocked pixels in `blend_disc`.
    pub fn set_anti_overflow_all(&mut self, m: Option<std::sync::Arc<mn_brush::AntiOverflowMask>>) {
        self.each_kind(|k| match k {
            EngineKind::My(b) => b.set_anti_overflow(m.clone()),
            EngineKind::Dab(d) => d.mask = m.clone(),
            EngineKind::Grid(e) => e.base.mask = m.clone(),
            EngineKind::Hairy(e) => e.base.mask = m.clone(),
            EngineKind::Curve(e) => e.base.mask = m.clone(),
            EngineKind::Dyna(e) => e.base.mask = m.clone(),
        });
    }

    /// Run a setter on the main engine and every twin.
    fn each_kind(&mut self, mut f: impl FnMut(&mut EngineKind)) {
        f(&mut self.main);
        for t in &mut self.twins {
            f(&mut t.kind);
        }
    }

    /// Display name for the HUD/panel.
    pub fn name(&self) -> &str {
        match &self.main {
            EngineKind::My(b) => b.name(),
            EngineKind::Dab(_) => "simple dab (fallback)",
            EngineKind::Grid(_) => "grid dots",
            EngineKind::Hairy(_) => "hairy bristles",
            EngineKind::Curve(_) => "curve brush",
            EngineKind::Dyna(_) => "dyna spring",
        }
    }

    /// GPU-dabs routing: every MyPaint kind (main + twins) must be
    /// `MyBrush::gpu_ready` for the stroke to take the compute path.
    /// WASH strokes route GPU only WITHOUT twins (#0.1 v1): mirror twins
    /// each commit their own CPU buffer (two saturating composites); a
    /// merged GPU buffer would saturate once — different wet semantics.
    pub fn gpu_dab_ready(&self) -> bool {
        let wash = matches!(&self.main, EngineKind::My(b) if b.wash());
        if wash && !self.twins.is_empty() {
            return false;
        }
        let ok = |k: &EngineKind| match k {
            EngineKind::My(b) => b.gpu_ready(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => false,
        };
        ok(&self.main) && self.twins.iter().all(|t| ok(&t.kind))
    }

    /// The main brush's texture-tip mask, if any — twins share the preset so
    /// the main's mask is the stroke's mask (GPU flush + CPU raster both
    /// consume it; the record carries the per-dab crawl offsets).
    pub fn texture_mask(&self) -> Option<&std::sync::Arc<mn_brush::TextureMask>> {
        match &self.main {
            EngineKind::My(b) => b.texture(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => None,
        }
    }

    /// Whether the mask stamps per dab (#10 amendment 2) — rides beside
    /// [`Self::texture_mask`] into the GPU flush and the repair rasterizer.
    pub fn texture_anchor_dab(&self) -> bool {
        match &self.main {
            EngineKind::My(b) => b.texture_anchor_dab(),
            _ => false,
        }
    }

    /// The `(mask bytes, size, dab-anchored)` triple every dab consumer
    /// takes — ONE spelling so the GPU flush, the wash flush and the repair
    /// rasterizer can never disagree about the anchor mode.
    pub fn texture_flush(&self) -> Option<(&[u8], u32, bool)> {
        self.texture_mask()
            .map(|m| (m.data.as_slice(), m.size, self.texture_anchor_dab()))
    }

    /// The main brush's live wash buffer (#0.1 GPU wash flush seeding).
    pub fn wash_buffer(&self) -> Option<&mn_core::Document> {
        match &self.main {
            EngineKind::My(b) => b.wash_buffer(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => None,
        }
    }

    /// Wash commit parameters (stroke opacity, blend, erase arm).
    pub fn wash_commit_params(&self) -> (f32, Blend, bool) {
        match &self.main {
            EngineKind::My(b) => b.wash_commit_params(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => (1.0, Default::default(), false),
        }
    }

    /// Claim the main brush's wash buffer after the GPU wash commit (#0.1) —
    /// `end` leaves it alive under BYPASS; this drops it so no stroke's
    /// buffer outlives its stroke. Twins never run GPU wash (the routing
    /// gate), so the main brush is the only holder.
    pub fn take_wash_buffer(&mut self) {
        if let EngineKind::My(b) = &mut self.main {
            b.take_wash_buffer();
        }
    }

    /// Set the record mode on every MyPaint kind.
    pub fn set_dab_recording_all(&mut self, mode: mn_brush::RecordMode) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_dab_recording(mode);
            }
        });
    }

    /// Publish the view transform to every MyPaint kind — the C compensates
    /// speed/direction inputs with it (vendor patch #12). Without this,
    /// zoomed-out drawing fires speed-mapped dynamics 1/zoom times too hard
    /// (owner report 2026-08-17: strokes drawn zoomed-out look bumpy when
    /// zoomed back in). `rotation_rad` is RADIANS — the C applies
    /// `DEGREES()` to the arg itself (upstream's "degrees" docstring is a
    /// doc bug; auditor 2026-08-17). `flip_h` mirrors the motion-direction
    /// inputs under a flipped view (the patch's flip extension — the
    /// viewport's own rotation is already negated by the flip).
    pub fn set_view(&mut self, zoom: f32, rotation_rad: f32, flip_h: bool) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_view(zoom, rotation_rad, flip_h);
            }
        });
    }

    /// Drain every kind's dab record. Main first, then twins — the per-sample
    /// interleave across engines is lost, which at most reorders overlapping
    /// twin dabs at the symmetry axis (single-brush order is exact).
    pub fn drain_dab_records(&mut self) -> Vec<mn_core::dab::DabParams> {
        let mut out = Vec::new();
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                out.extend(b.take_dab_record().dabs);
            }
        });
        out
    }

    /// The main engine's tip mode (the shader's hard-stamp flag).
    pub fn hard_dab_main(&self) -> bool {
        matches!(&self.main, EngineKind::My(b) if b.hard_dab())
    }

    pub fn set_color(&mut self, rgb: [f32; 3]) {
        self.each_kind(|k| match k {
            EngineKind::My(b) => b.set_color_rgb(rgb),
            EngineKind::Grid(g) => g.base.color = rgb,
            EngineKind::Hairy(h) => h.base.color = rgb,
            EngineKind::Curve(c) => c.base.color = rgb,
            EngineKind::Dyna(y) => y.base.color = rgb,
            EngineKind::Dab(d) => d.color = rgb,
        });
    }

    /// The size the current kind ships with, as a dab DIAMETER in canvas px.
    /// What a sub tool met for the first time starts at.
    pub fn base_size_px(&self) -> f32 {
        match &self.main {
            EngineKind::My(b) => b.base_size_px(),
            EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_)
            | EngineKind::Dab(_) => self.base_px,
        }
    }

    /// Set the dab DIAMETER in canvas px — absolute, not a multiplier, so the
    /// Size control and the `[`/`]` ladder share one honest number and nothing
    /// silently caps the ladder.
    ///
    /// Every kind computes its size FROM `px` rather than scaling what it
    /// currently holds, so setting the same size twice is setting it once
    /// (`MyBrush` re-derives from the radius the preset shipped; the fallback
    /// kinds derive their one size scalar from `px` and keep their own
    /// min:max ratio).
    pub fn set_size_px(&mut self, px: f32) {
        let r = (px * 0.5).max(1e-4);
        self.each_kind(|k| match k {
            EngineKind::My(b) => b.set_size_px(px),
            EngineKind::Grid(g) => g.pitch = r.clamp(2.0, 512.0),
            EngineKind::Hairy(h) => h.spread = r.clamp(1.0, 512.0),
            EngineKind::Curve(c) => c.w = r.clamp(2.0, 1024.0),
            EngineKind::Dyna(y) => {
                // Ratio-preserving: max lands on `r` exactly and min follows
                // it, so a second identical call is a no-op.
                let k = r / y.base.max_radius.max(1e-4);
                y.base.min_radius *= k;
                y.base.max_radius = r;
            }
            EngineKind::Dab(d) => {
                d.min_radius = r * (BASE_MIN_RADIUS / BASE_MAX_RADIUS);
                d.max_radius = r;
            }
        });
    }

    /// Real erasing = libmypaint's `eraser` setting (dabs subtract alpha), which
    /// is correct on a layer stack. `SimpleDab` has no erase mode at all, so on
    /// the fallback path the eraser is inert rather than painting paper white —
    /// a white brush is wrong the moment there is more than one layer.
    pub fn set_eraser(&mut self, on: bool) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_eraser(on);
            }
        });
    }

    /// Base dab radius in canvas px (pressure/speed dynamics move around it).
    pub fn radius_px(&self) -> f32 {
        match &self.main {
            EngineKind::My(b) => b.radius_px(),
            EngineKind::Grid(g) => g.pitch,
            EngineKind::Hairy(h) => h.spread,
            EngineKind::Curve(c) => c.w,
            EngineKind::Dyna(y) => y.base.max_radius,
            EngineKind::Dab(d) => d.max_radius,
        }
    }

    pub fn set_base_opacity(&mut self, o: f32) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_base_opacity(o);
            }
        });
    }

    pub fn base_opacity(&self) -> f32 {
        match &self.main {
            EngineKind::My(b) => b.base_opacity(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => 1.0,
        }
    }

    pub fn set_size_min_pct(&mut self, pct: f32) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_size_min_pct(pct);
            }
        });
    }

    /// Brush-size randomization (CSP 乱数): amount, pressure floor %, and the
    /// fixed-pixel mode whose deviation does not scale with brush size.
    pub fn set_randomization(&mut self, amount: f32, min_pct: f32, absolute_px: bool) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_randomization(amount, min_pct, absolute_px);
            }
        });
    }

    pub fn randomization(&self) -> (f32, f32, bool) {
        match &self.main {
            EngineKind::My(b) => b.randomization(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => (0.0, 0.0, false),
        }
    }

    /// Krita-style hard stamp dabs (vendor hook; off = stock gaussian).
    pub fn set_hard_dab(&mut self, on: bool) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_hard_dab(on);
            }
        });
    }

    pub fn hard_dab(&self) -> bool {
        matches!(&self.main, EngineKind::My(b) if b.hard_dab())
    }

    /// Krita Scatter: dab centre jitter as a fraction of the radius.
    pub fn set_scatter(&mut self, v: f32) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_scatter(v);
            }
        });
    }

    pub fn scatter(&self) -> f32 {
        match &self.main {
            EngineKind::My(b) => b.scatter(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => 0.0,
        }
    }

    /// CSP Stroke ▸ Interval (S-028): dab spacing, on every kind.
    pub fn set_interval(&mut self, interval: mn_brush::Interval) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_interval(interval);
            }
        });
    }

    // No `Engine::interval()` / `Engine::anti_alias()` getter on purpose. The
    // authority for "which CSP mode is selected" is `ToolProps`, not the
    // engine: the engine only holds the numbers a mode produced, and a preset
    // nobody has touched has numbers but no mode. A getter here would be a
    // second, weaker answer to a question `props_current` already answers —
    // and it was dead code. `dab_gap_px` / `anti_alias_px` below are the
    // opposite case and DO belong here: they are readings, not modes.

    /// The live gap between dabs at the base radius, canvas px (the panel's
    /// readout). `f32::INFINITY` when nothing stamps by distance.
    pub fn dab_gap_px(&self) -> f32 {
        match &self.main {
            EngineKind::My(b) => b.dab_gap_px(),
            _ => f32::INFINITY,
        }
    }

    /// CSP Adjust brush density by gap (B-029).
    pub fn set_density_by_gap(&mut self, on: bool) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_density_by_gap(on);
            }
        });
    }

    pub fn density_by_gap(&self) -> bool {
        matches!(&self.main, EngineKind::My(b) if b.density_by_gap())
    }

    /// CSP Anti-aliasing (A-010): the four-level edge feather.
    pub fn set_anti_alias(&mut self, aa: mn_brush::AntiAlias) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_anti_alias(aa);
            }
        });
    }

    /// The minimum edge feather the engine will enforce, canvas px.
    pub fn anti_alias_px(&self) -> f32 {
        match &self.main {
            EngineKind::My(b) => b.anti_alias_px(),
            _ => 0.0,
        }
    }

    /// Krita Wash (flow vs opacity): `stroke_opacity` is the stroke-level
    /// opacity applied at the commit; `blend` its compositing mode.
    pub fn set_wash(&mut self, on: bool, stroke_opacity: f32, blend: Blend) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_wash(on, stroke_opacity, blend);
            }
        });
    }

    pub fn wash(&self) -> bool {
        matches!(&self.main, EngineKind::My(b) if b.wash())
    }

    /// Whether the main engine's preset samples the canvas per dab (#0.1
    /// part 3 — the GPU smudge wiring). Preset-set only; twins share the
    /// preset.
    pub fn smudge(&self) -> bool {
        matches!(&self.main, EngineKind::My(b) if b.smudge())
    }

    pub fn wash_opacity(&self) -> f32 {
        match &self.main {
            EngineKind::My(b) => b.wash_opacity(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => 1.0,
        }
    }

    pub fn set_wash_opacity(&mut self, o: f32) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_wash_opacity(o);
            }
        });
    }

    pub fn set_wash_blend(&mut self, blend: Blend) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_wash_blend(blend);
            }
        });
    }

    pub fn wash_blend(&self) -> Blend {
        match &self.main {
            EngineKind::My(b) => b.wash_blend(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => Blend::Normal,
        }
    }

    /// Per-dab alpha inside a wash stroke (Krita: Flow). Same knob as
    /// `set_base_opacity`; this alias exists so call sites read honestly.
    pub fn set_flow(&mut self, flow: f32) {
        self.set_base_opacity(flow);
    }

    /// Krita texture tip: install a grayscale dab mask (`None` = stock).
    pub fn set_texture_mask(&mut self, mask: Option<std::sync::Arc<mn_brush::TextureMask>>) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_texture(mask.clone());
            }
        });
    }

    /// The active texture's name, if any (the picker's honest readback).
    pub fn texture_name(&self) -> Option<&str> {
        match &self.main {
            EngineKind::My(b) => b.texture().map(|m| m.name.as_str()),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => None,
        }
    }

    /// Texture crawl per dab, in mask px (0 = the pattern is static).
    pub fn set_texture_scroll(&mut self, px: f32) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_texture_scroll(px);
            }
        });
    }

    pub fn texture_scroll(&self) -> f32 {
        match &self.main {
            EngineKind::My(b) => b.texture_scroll(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => 0.0,
        }
    }

    /// Krita SKETCH engine: link strokes back to their recent history
    /// (hatching webs); `None` = stock.
    pub fn set_sketch(&mut self, params: Option<mn_brush::SketchParams>) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_sketch(params);
            }
        });
    }

    pub fn sketch(&self) -> Option<mn_brush::SketchParams> {
        match &self.main {
            EngineKind::My(b) => b.sketch(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => None,
        }
    }

    /// One setting's response curve for one input (Krita per-sensor curves).
    /// Empty = no response.
    pub fn mapping(&self, setting_id: i32, input_id: i32) -> Vec<(f32, f32)> {
        match &self.main {
            EngineKind::My(b) => b.mapping(setting_id, input_id),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => Vec::new(),
        }
    }

    pub fn set_mapping(&mut self, setting_id: i32, input_id: i32, points: &[(f32, f32)]) {
        self.each_kind(|k| {
            if let EngineKind::My(b) = k {
                b.set_mapping(setting_id, input_id, points);
            }
        });
    }

    /// CSP entry-taper metadata carried by the preset, if any.
    pub fn taper_hint(&self) -> Option<(f32, f32)> {
        match &self.main {
            EngineKind::My(b) => b.taper_hint(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => None,
        }
    }

    pub fn size_min_pct(&self) -> f32 {
        match &self.main {
            EngineKind::My(b) => b.size_min_pct(),
            EngineKind::Dab(_)
            | EngineKind::Grid(_)
            | EngineKind::Hairy(_)
            | EngineKind::Curve(_)
            | EngineKind::Dyna(_) => 0.0,
        }
    }
}

impl EngineKind {
    /// Apply Tool Property values with the preset-preserving guards, reading
    /// THIS kind's own state — a fresh-from-preset twin and the live main
    /// engine both configure correctly through the same code.
    fn apply_props(
        &mut self,
        p: &crate::cmd::ToolProps,
        texture_mask: Option<&std::sync::Arc<mn_brush::TextureMask>>,
    ) {
        let EngineKind::My(b) = self else {
            return; // the fallback dab has no preset state to preserve
        };
        b.set_size_px(p.size_px);
        b.set_base_opacity(if p.wash { p.flow } else { p.opacity });
        // `set_size_min_pct` REPLACES the preset's pressure→size curve with a
        // canonical one — Real G-Pen ships a 12-point measured curve, so only
        // overwrite it when the user actually moved the slider away from the
        // preset's own honest reading.
        if (p.min_size - b.size_min_pct()).abs() > 0.5 {
            b.set_size_min_pct(p.min_size);
        }
        // Randomization: same rule (the setter replaces the preset's mapping
        // for `radius_by_random`).
        let (r_amt, r_min, r_abs) = b.randomization();
        if (p.random - r_amt).abs() > 1e-3
            || (p.random_min - r_min).abs() > 0.5
            || p.random_abs != r_abs
        {
            b.set_randomization(p.random, p.random_min, p.random_abs);
        }
        // Krita modes are plain per-brush flags — no preset mapping to
        // preserve, so they apply unconditionally.
        b.set_hard_dab(p.hard_dab);
        b.set_scatter(p.scatter);
        // The three feel rows. Each carries its own "as the preset ships it"
        // state, so these are unconditional AND still byte-identical on a
        // preset nobody has touched — no read-back guard needed. Interval
        // AFTER `set_size_px` above: a Fixed-px gap is converted
        // against the radius that call just set.
        b.set_interval(p.interval);
        if let Some(on) = p.density_by_gap {
            b.set_density_by_gap(on);
        }
        b.set_anti_alias(p.anti_alias);
        // Wash semantics: in build-up the one Opacity slider IS the per-dab
        // alpha; in wash it becomes the stroke-level opacity and Flow is the
        // per-dab alpha inside the buffer (Krita's Opacity/Flow pair).
        if p.wash {
            b.set_wash(true, p.opacity, p.brush_blend);
        } else {
            b.set_wash(false, 1.0, Blend::Normal);
        }
        b.set_texture(texture_mask.cloned());
        b.set_texture_scroll(p.texture_scroll);
        b.set_sketch(p.sketch.then_some(mn_brush::SketchParams {
            distance: p.sketch_dist,
            density: p.sketch_density,
        }));
    }
}

impl StrokeSink for EngineKind {
    fn begin(&mut self, doc: &mut Document) {
        match self {
            EngineKind::My(b) => b.begin(doc),
            EngineKind::Grid(g) => g.begin(doc),
            EngineKind::Hairy(h) => h.begin(doc),
            EngineKind::Curve(c) => c.begin(doc),
            EngineKind::Dyna(y) => y.begin(doc),
            EngineKind::Dab(d) => d.begin(doc),
        }
    }
    fn sample(&mut self, doc: &mut Document, s: PenSample) {
        match self {
            EngineKind::My(b) => b.sample(doc, s),
            EngineKind::Grid(g) => g.sample(doc, s),
            EngineKind::Hairy(h) => h.sample(doc, s),
            EngineKind::Curve(c) => c.sample(doc, s),
            EngineKind::Dyna(y) => y.sample(doc, s),
            EngineKind::Dab(d) => d.sample(doc, s),
        }
    }
    fn end(&mut self, doc: &mut Document) {
        match self {
            EngineKind::My(b) => b.end(doc),
            EngineKind::Grid(g) => g.end(doc),
            EngineKind::Hairy(h) => h.end(doc),
            EngineKind::Curve(c) => c.end(doc),
            EngineKind::Dyna(y) => y.end(doc),
            EngineKind::Dab(d) => d.end(doc),
        }
    }
}

impl StrokeSink for Engine {
    fn begin(&mut self, doc: &mut Document) {
        self.main.begin(doc);
        for t in &mut self.twins {
            t.kind.begin(doc);
        }
    }
    /// The twins receive the POST-stabilizer, post-taper sample transformed
    /// (mirrored around the canvas centre, or wrapped by ±canvas-size) — so
    /// a copy is smoothed exactly as much as the stroke the user actually
    /// paints, without running a second stabilizer string.
    fn sample(&mut self, doc: &mut Document, s: PenSample) {
        self.main.sample(doc, s);
        let (cx, cy) = (doc.size.0 as f32 * 0.5, doc.size.1 as f32 * 0.5);
        let (w, h) = (doc.size.0 as f32, doc.size.1 as f32);
        for t in &mut self.twins {
            let mut m = s;
            if let Some(xf) = &t.xf {
                // Symmetric-ruler twin: one image of the dihedral group.
                let q = xf.apply([m.x, m.y]);
                m.x = q[0];
                m.y = q[1];
            } else {
                match t.x {
                    Some(TwinAxis::Mirror) => m.x = 2.0 * cx - m.x,
                    // Shift to the neighbouring tile: a dab hugging the right
                    // edge lands by the left one and vice versa.
                    Some(TwinAxis::Wrap) => m.x = if m.x >= cx { m.x - w } else { m.x + w },
                    None => {}
                }
                match t.y {
                    Some(TwinAxis::Mirror) => m.y = 2.0 * cy - m.y,
                    Some(TwinAxis::Wrap) => m.y = if m.y >= cy { m.y - h } else { m.y + h },
                    None => {}
                }
            }
            t.kind.sample(doc, m);
        }
    }
    fn end(&mut self, doc: &mut Document) {
        self.main.end(doc);
        for t in &mut self.twins {
            t.kind.end(doc);
        }
    }
}

#[cfg(test)]
mod preset_engine_tests {
    use super::*;

    /// Which arm of [`EngineKind`] a preset landed on, as a name a table can
    /// compare — the enum carries engines, which are not `PartialEq`.
    fn arm(k: &EngineKind) -> &'static str {
        match k {
            EngineKind::My(_) => "my",
            EngineKind::Dab(_) => "dab",
            EngineKind::Grid(_) => "grid",
            EngineKind::Hairy(_) => "hairy",
            EngineKind::Curve(_) => "curve",
            EngineKind::Dyna(_) => "dyna",
        }
    }

    /// The whole `mn-engine` contract as one table. Three readers build an
    /// engine from this key (the live tool, the Sub Tool swatch, the property
    /// panel's test strip) and they only agree because they all come through
    /// here.
    #[test]
    fn the_mn_engine_key_selects_its_engine() {
        let dir = std::env::temp_dir().join("mn-preset-engine-table");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let cases: [(&str, Option<&str>); 8] = [
            (r#"{"version":3,"mn-engine":"grid"}"#, Some("grid")),
            (r#"{"version":3,"mn-engine":"hairy"}"#, Some("hairy")),
            (r#"{"version":3,"mn-engine":"curve"}"#, Some("curve")),
            (r#"{"version":3,"mn-engine":"dyna"}"#, Some("dyna")),
            // A name this build does not know falls back to MyPaint rather
            // than leaving the sub tool with no engine at all.
            (r#"{"version":3,"mn-engine":"sparkle"}"#, None),
            // The key is optional: an ordinary preset is a MyPaint preset.
            (r#"{"version":3,"settings":{}}"#, None),
            // Wrong type, and not JSON at all — neither may panic.
            (r#"{"mn-engine":7}"#, None),
            ("not json", None),
        ];
        for (i, (json, want)) in cases.iter().enumerate() {
            let p = dir.join(format!("case{i}.myb"));
            std::fs::write(&p, json).expect("write preset");
            assert_eq!(
                preset_engine(&p).as_ref().map(arm),
                *want,
                "case {i}: {json}"
            );
        }
        // A path that does not exist is `None`, not a panic.
        assert!(preset_engine(&dir.join("absent.myb")).is_none());
    }

    /// And the SHIPPED presets really carry those keys — the table above would
    /// happily pass while every asset had a typo in it.
    #[test]
    fn the_shipped_procedural_presets_select_their_engines() {
        for (name, want) in [
            ("grid-dots", "grid"),
            ("hairy-bristles", "hairy"),
            ("curve-brush", "curve"),
            ("dyna-spring", "dyna"),
        ] {
            let p = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../assets/brushes/krita/{name}.myb"));
            assert_eq!(
                preset_engine(&p).as_ref().map(arm),
                Some(want),
                "{name} lost its mn-engine key"
            );
        }
        let pen =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes/classic/pen.myb");
        assert!(
            preset_engine(&pen).is_none(),
            "an ordinary preset must stay on the MyPaint path"
        );
    }
}
