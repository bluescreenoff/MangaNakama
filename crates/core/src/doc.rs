//! Document / layer model.
//!
//! Contract (docs/ARCHITECTURE.md) — keep these signatures:
//! ```ignore
//! pub struct Document { pub layers: Vec<Layer>, pub active: usize, pub size: (u32, u32) }
//! pub struct Layer { pub opacity: f32, pub blend: Blend, pub visible: bool, pub name: String }
//! pub enum Blend { Normal, Multiply, Screen }
//! impl Layer {
//!     pub fn tile(&self, t: TileIdx) -> Option<&Tile>;
//!     pub fn tile_mut(&mut self, t: TileIdx) -> &mut Tile;
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use crate::balloon::BalloonSet;
use crate::frame::FrameSet;
use crate::text::{RenderedText, TextItem, TextSet};
use crate::tile::{TILE_SIZE, Tile, TileIdx, next_revision};
use crate::undo::{History, UndoGroup};

/// The layer blend set — 27 of CSP's 28 named modes.
///
/// Honoured by both compositors: `gpu::Renderer` and `core::export` (CPU).
/// Five of them (Normal/Multiply/Screen/Add/Subtract) are expressible as
/// fixed-function GPU blend states; **every other variant composites through
/// the `blend2.wgsl` shader pass**, which reads a snapshot of the destination
/// and evaluates the same formula the CPU does.
///
/// The exact formulas live in `core::blend`, mirrored as comments next to the
/// GPU blend states and reimplemented in `blend2.wgsl` — change one, change
/// all three; the `cpu_matches_gpu*` tests in `mn-gpu` pin them equal.
///
/// **Adding a variant means five edit sites**, and a mode added in four of
/// five is a silent bug: this enum, `ora_name`/`from_ora_name` below,
/// `core::blend::blend_premul`, `gpu::BLEND2_MODES` + `blend2.wgsl`, and
/// `app::ui::layers::{BLENDS, blend_name}`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Darken,
    Lighten,
    Add,
    Subtract,
    /// Blend part 2 (round 28): the separable operator family plus the
    /// nonseparable trio. The GPU shader compositor pass (`blend2.wgsl`)
    /// landed with them, so they are in the picker and in ORA parsing.
    Overlay,
    SoftLight,
    HardLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    /// Blend part 3 — the dodge/burn/light family (CSP's BM-004..BM-028).
    /// Same `blend2.wgsl` path as part 2. Appended, never inserted: the
    /// ORA names below are the only thing a saved file depends on, but
    /// keeping the declaration order stable keeps `Debug` output and the
    /// picker's tail stable too.
    ColorBurn,
    LinearBurn,
    ColorDodge,
    GlowDodge,
    VividLight,
    LinearLight,
    PinLight,
    HardMix,
    Divide,
    DarkerColor,
    LighterColor,
    /// CSP calls this **Brightness**; Photoshop and SVG call it Luminosity.
    /// The code uses the SVG name (it is the `svg:luminosity` operator); the
    /// picker shows the owner the CSP one.
    Luminosity,
}

impl Blend {
    /// The OpenRaster `composite-op` name for this mode. `mn:` names are our
    /// extensions (SVG has no add/subtract); foreign readers fall back to
    /// Normal per the ORA spec's advice for unknown operators.
    pub fn ora_name(self) -> &'static str {
        match self {
            Blend::Normal => "svg:src-over",
            Blend::Multiply => "svg:multiply",
            Blend::Screen => "svg:screen",
            Blend::Darken => "svg:darken",
            Blend::Lighten => "svg:lighten",
            Blend::Add => "mn:add",
            Blend::Subtract => "mn:subtract",
            Blend::Overlay => "svg:overlay",
            Blend::SoftLight => "svg:soft-light",
            Blend::HardLight => "svg:hard-light",
            Blend::Difference => "svg:difference",
            Blend::Exclusion => "svg:exclusion",
            Blend::Hue => "svg:hue",
            Blend::Saturation => "svg:saturation",
            Blend::Color => "svg:color",
            // Part 3. SVG/PDF names exist for three of these; the rest are
            // CSP/Photoshop modes SVG never standardised, so they take the
            // `mn:` prefix like add/subtract. A foreign reader falls back to
            // Normal for those, which is the ORA spec's own advice.
            Blend::ColorBurn => "svg:color-burn",
            Blend::LinearBurn => "mn:linear-burn",
            Blend::ColorDodge => "svg:color-dodge",
            Blend::GlowDodge => "mn:glow-dodge",
            Blend::VividLight => "mn:vivid-light",
            Blend::LinearLight => "mn:linear-light",
            Blend::PinLight => "mn:pin-light",
            Blend::HardMix => "mn:hard-mix",
            Blend::Divide => "mn:divide",
            Blend::DarkerColor => "mn:darker-color",
            Blend::LighterColor => "mn:lighter-color",
            Blend::Luminosity => "svg:luminosity",
        }
    }

    /// Parse an OpenRaster `composite-op`. Unknown ops fall back to `Normal`
    /// (the ORA spec's own recommendation for unsupported operators).
    ///
    /// The inverse of [`Blend::ora_name`] for every variant —
    /// `every_blend_mode_round_trips_through_its_ora_name` in this module
    /// pins that, and pins the pre-part-3 names byte-for-byte so a file the
    /// owner saved before this round still loads to the same mode.
    pub fn from_ora_name(s: &str) -> Self {
        match s {
            "svg:multiply" => Blend::Multiply,
            "svg:screen" => Blend::Screen,
            "mn:add" => Blend::Add,
            "mn:subtract" => Blend::Subtract,
            "svg:darken" => Blend::Darken,
            "svg:lighten" => Blend::Lighten,
            "svg:overlay" => Blend::Overlay,
            "svg:soft-light" => Blend::SoftLight,
            "svg:hard-light" => Blend::HardLight,
            "svg:difference" => Blend::Difference,
            "svg:exclusion" => Blend::Exclusion,
            "svg:hue" => Blend::Hue,
            "svg:saturation" => Blend::Saturation,
            "svg:color" => Blend::Color,
            "svg:color-burn" => Blend::ColorBurn,
            "mn:linear-burn" => Blend::LinearBurn,
            "svg:color-dodge" => Blend::ColorDodge,
            "mn:glow-dodge" => Blend::GlowDodge,
            "mn:vivid-light" => Blend::VividLight,
            "mn:linear-light" => Blend::LinearLight,
            "mn:pin-light" => Blend::PinLight,
            "mn:hard-mix" => Blend::HardMix,
            "mn:divide" => Blend::Divide,
            "mn:darker-color" => Blend::DarkerColor,
            "mn:lighter-color" => Blend::LighterColor,
            "svg:luminosity" => Blend::Luminosity,
            _ => Blend::Normal,
        }
    }

    /// Every variant, declaration order. Exhaustive by construction: the
    /// round-trip test asserts the length matches the enum's own arm count
    /// via `ora_name`, so a variant added without a row here shows up as a
    /// duplicate or a missing name rather than silently going untested.
    pub const ALL: [Blend; 27] = [
        Blend::Normal,
        Blend::Multiply,
        Blend::Screen,
        Blend::Darken,
        Blend::Lighten,
        Blend::Add,
        Blend::Subtract,
        Blend::Overlay,
        Blend::SoftLight,
        Blend::HardLight,
        Blend::Difference,
        Blend::Exclusion,
        Blend::Hue,
        Blend::Saturation,
        Blend::Color,
        Blend::ColorBurn,
        Blend::LinearBurn,
        Blend::ColorDodge,
        Blend::GlowDodge,
        Blend::VividLight,
        Blend::LinearLight,
        Blend::PinLight,
        Blend::HardMix,
        Blend::Divide,
        Blend::DarkerColor,
        Blend::LighterColor,
        Blend::Luminosity,
    ];
}

/// LP-022 「表現色」: how the layer is DISPLAYED, not what it is.
///
/// The print check — "would this page hold up in 1-bit?" — without converting
/// anything. Nothing here touches a pixel, nothing here is exported, and the
/// setting survives a save so the answer is one click away next session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayerExpression {
    /// As drawn.
    #[default]
    Colour,
    /// Chroma dropped; the alpha ramp survives.
    Grey,
    /// 1-bit: value AND coverage threshold at 50 %.
    Mono,
}

impl LayerExpression {
    /// ORA attribute value, or `None` for the default (which is written as
    /// no attribute at all — an old file and a colour layer look the same on
    /// disk, which is the point).
    pub fn ora_name(self) -> Option<&'static str> {
        match self {
            LayerExpression::Colour => None,
            LayerExpression::Grey => Some("grey"),
            LayerExpression::Mono => Some("mono"),
        }
    }

    /// Anything unrecognised loads as `Colour` — a foreign or future value
    /// must not hide the artwork.
    pub fn from_ora_name(s: &str) -> Self {
        match s {
            "grey" => LayerExpression::Grey,
            "mono" => LayerExpression::Mono,
            _ => LayerExpression::Colour,
        }
    }

    /// Bit-exact signature for the GPU's per-layer presentation hash. This
    /// value reaches the shader and NEVER moves a tile revision, so leaving
    /// it out of `LayerSig` would leave the canvas showing the old picture.
    pub fn sig(self) -> u32 {
        self as u32
    }
}

/// What a layer *is*. Raster layers own their pixels; frame, balloon and text
/// layers derive their pixels from vector state ([`FrameSet`] /
/// [`BalloonSet`] / [`TextSet`]) and are regenerated by `Document::set_frames`
/// / `Document::set_balloons` / `Document::set_texts` / undo.
#[derive(Clone, Debug, Default)]
pub enum LayerKind {
    #[default]
    Raster,
    /// LIVE fill/gradient/tone (TRIAGE 137): the content is parameters,
    /// the raster is derived through the layer mask (the window). See
    /// `fill_layer`.
    Fill(crate::fill_layer::FillKind),
    Frame(FrameSet),
    Balloon(BalloonSet),
    Text(TextSet),
}

/// A layer: sparse tiles + presentation state.
///
/// Tiles are `Arc`-shared so undo snapshots (a later agent) are Arc clones, and
/// the write path goes through `Arc::make_mut` for copy-on-write.
/// A layer mask (TRIAGE 138, LM-005's ALPHA scale): per-pixel coverage in
/// the tile ALPHA channel (fix15; full = visible, absent tile = hidden).
/// Any brush will edit it (part 2); soft brush ⇒ soft mask, automatically.
#[derive(Clone, Debug, Default)]
pub struct LayerMask {
    pub tiles: HashMap<TileIdx, Arc<Tile>>,
    pub enabled: bool,
    /// Bumped on every edit — the GPU tile cache's rebuild signal.
    pub revision: u64,
}

#[derive(Clone, Debug)]
pub struct Layer {
    tiles: HashMap<TileIdx, Arc<Tile>>,
    /// Undo recording, armed by `Document::begin_op`. While `Some`, every
    /// `tile_mut` stashes the tile's pre-image here (first touch wins).
    /// `None` in the value = the tile did not exist before the op.
    recording: Option<HashMap<TileIdx, Option<Arc<Tile>>>>,
    pub kind: LayerKind,
    pub opacity: f32,
    pub blend: Blend,
    pub visible: bool,
    pub name: String,
    /// CSP-style palette/label colour shown as a strip in the Layers palette.
    /// Pure organisation — never composited.
    pub label: Option<[u8; 3]>,
    /// Layer colour (CSP LP-016, the two-tone MAIN colour): a display tint —
    /// the layer's dark ink renders in this colour (white stays white),
    /// non-destructively. The pixels stay black; every compositor tints.
    /// `None` = stock.
    pub layer_colour: Option<[u8; 3]>,
    /// The two-tone SUB colour (CSP LP-017): the other end of the same
    /// ramp — main replaces black, this replaces WHITE. Only meaningful
    /// alongside `layer_colour`; `None` = the white end stays white, which
    /// is the LP-016 behaviour bit-for-bit.
    pub layer_sub_colour: Option<[u8; 3]>,
    /// LP-022 decrease-colour PREVIEW. Display only: no pixel changes, and
    /// the export composite ignores it.
    pub expression: LayerExpression,
    /// Nesting level: 0 = root. A layer at depth d+1 belongs to the nearest
    /// folder above it in the stack at depth d (children sit *below* their
    /// folder header in `Document::layers`, so the header consumes their
    /// isolated group when the compositor reaches it).
    pub depth: u8,
    /// This layer is a folder header. Children composite into an isolated
    /// buffer which the header blends onto its backdrop with the header's
    /// opacity/blend. A folder with `kind == Frame(..)` is a CSP frame border
    /// folder: `mask_tiles` clips the group to the panel interiors and
    /// `tiles` holds the border ink drawn on top.
    pub folder: bool,
    /// LF-002 Through: the folder stops isolating — each child blends
    /// against everything beneath it on the page, exactly as if loose.
    /// The folder still groups/moves/hides/locks as one. The header's own
    /// raster still draws at its depth; the group close, the group blend,
    /// and the frame-mask clip do NOT run (they belong to the seal).
    pub through: bool,
    /// Folder expand state in the Layers palette. Presentation only.
    pub open: bool,
    /// Clip to the layer below (CSP クリッピング): this layer only shows where
    /// the nearest non-clip layer below it (same depth) has alpha.
    pub clip: bool,
    /// Edit lock: strokes/fill/clear refuse. Presentation still composites.
    pub lock: bool,
    /// Layer mask (TRIAGE 138 v1): None = unmasked. Runtime-only until
    /// the persistence round (an unsaved mask is lost on reload —
    /// recorded, DECISIONS 8.38).
    pub mask: Option<LayerMask>,
    /// LM-009: the mask MOVES with the layer (CSP's default, linked).
    /// Unlinked = transform/move slides the art UNDERNEATH a fixed mask
    /// (photo-in-a-window). Editing (LM-004) is unaffected by this flag.
    pub mask_linked: bool,
    /// Transparent-pixel lock (透明ピクセルをロック): strokes only change
    /// pixels in proportion to the alpha they already had.
    pub lock_alpha: bool,
    /// Reference layer (CSP 参照レイヤー): the one layer Fill/Auto-select
    /// can be told to sample (even when hidden). Exclusive — setting it on
    /// one layer clears it on every other.
    pub reference: bool,
    /// Draft layer (CSP 下書き): still composited on screen, but excluded
    /// from fill/wand sampling and from PNG export.
    pub draft: bool,
    /// Frame folder only: derived panel-interior coverage (white premul, AA
    /// edges; absent tile = zero coverage). Never serialized — rebuilt from
    /// the FrameSet.
    mask_tiles: Option<HashMap<TileIdx, Arc<Tile>>>,
    /// Screentone configuration (CSP トーンレイヤー). `Some` = the layer's
    /// PAINTED pixels are the source ink and `tone_tiles` is what every
    /// compositor displays. Converting is non-destructive both ways.
    pub tone: Option<crate::tone::ToneParams>,
    /// SF-004/005 (TRIAGE 140): the generator params this layer was
    /// built from — effect lines stay re-editable (the dialog reopens
    /// with these; re-apply rasterizes in place).
    pub genlines: Option<crate::genlines::GenLinesSpec>,
    /// Border effect (CSP LP-002/LP-003 境界効果 ▸ フチ). `Some` = the
    /// displayed raster is the layer's pixels sitting on a grown outline;
    /// the painted pixels are untouched and turning it off restores them.
    pub edge: Option<crate::edge::EdgeParams>,
    /// Derived halftone raster, one per source tile whose revision advanced.
    /// Never serialized — rebuilt by `Document::refresh_derived`.
    tone_tiles: Option<HashMap<TileIdx, Arc<Tile>>>,
    /// Derived border-effect raster (source ink over the grown outline).
    /// Never serialized — rebuilt by `Layer::refresh_edge`.
    edge_tiles: Option<HashMap<TileIdx, Arc<Tile>>>,
    /// The (params, source-tile-SET hash, newest source revision)
    /// `edge_tiles` was built from. Equal ⇒ the whole refresh is skipped;
    /// the set hash is in there because a VANISHED tile leaves a cached
    /// outline that still looks fresh by revision alone. Sound because every
    /// write path — undo included, see `Layer::set_tile` — stamps a fresh
    /// global revision, so revisions only ever move forward.
    edge_stamp: Option<(crate::edge::EdgeParams, i64, u64)>,
    /// Derived LIVE-fill raster (TRIAGE 137), params x the window mask.
    /// Never serialized — rebuilt by `Layer::refresh_fill`.
    pub(crate) fill_tiles: Option<HashMap<TileIdx, Arc<Tile>>>,
    /// The (params, mask revision, dpi, canvas size) the current
    /// `fill_tiles` was built from — the skip-work stamp. Size is in the
    /// stamp because a maskless fill windows the whole canvas: a resize
    /// must re-derive even when nothing else moved.
    pub(crate) fill_stamp: Option<(crate::fill_layer::FillKind, Option<u64>, u32, (u32, u32))>,
}

impl Layer {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            tiles: HashMap::new(),
            recording: None,
            kind: LayerKind::Raster,
            opacity: 1.0,
            blend: Blend::Normal,
            visible: true,
            name: name.into(),
            label: None,
            layer_colour: None,
            layer_sub_colour: None,
            expression: LayerExpression::Colour,
            depth: 0,
            folder: false,
            through: false,
            open: true,
            clip: false,
            lock: false,
            mask: None,
            mask_linked: true,
            lock_alpha: false,
            reference: false,
            draft: false,
            mask_tiles: None,
            tone: None,
            genlines: None,
            edge: None,
            tone_tiles: None,
            edge_tiles: None,
            edge_stamp: None,
            fill_tiles: None,
            fill_stamp: None,
        }
    }

    /// The frame folder's derived coverage mask, if any.
    pub fn mask_tiles(&self) -> Option<&HashMap<TileIdx, Arc<Tile>>> {
        self.mask_tiles.as_ref()
    }

    /// Swap in a freshly derived coverage mask (or drop it).
    pub fn replace_mask_tiles(&mut self, mask: Option<HashMap<TileIdx, Arc<Tile>>>) {
        self.mask_tiles = mask;
    }

    /// The tiles the border effect grows FROM: the derived tone raster on a
    /// tone layer, the live-fill raster on a fill layer, the painted pixels
    /// otherwise. Not what the compositor draws — see [`Self::display_tiles`].
    fn base_tiles(&self) -> &HashMap<TileIdx, Arc<Tile>> {
        static EMPTY: std::sync::OnceLock<HashMap<TileIdx, Arc<Tile>>> = std::sync::OnceLock::new();
        if self.tone.is_some() {
            self.tone_tiles
                .as_ref()
                .unwrap_or_else(|| EMPTY.get_or_init(HashMap::new))
        } else if matches!(self.kind, LayerKind::Fill(_)) {
            self.fill_tiles
                .as_ref()
                .unwrap_or_else(|| EMPTY.get_or_init(HashMap::new))
        } else {
            &self.tiles
        }
    }

    /// The tiles every compositor must display: the border-effect raster
    /// when the layer has one, else the derived tone/fill raster, else the
    /// painted pixels. A derived layer whose raster has not been built yet
    /// displays nothing — the app's contract is `refresh_derived` before
    /// compositing.
    pub fn display_tiles(&self) -> &HashMap<TileIdx, Arc<Tile>> {
        static EMPTY: std::sync::OnceLock<HashMap<TileIdx, Arc<Tile>>> = std::sync::OnceLock::new();
        if self.edge.is_some() {
            return self
                .edge_tiles
                .as_ref()
                .unwrap_or_else(|| EMPTY.get_or_init(HashMap::new));
        }
        self.base_tiles()
    }

    /// The displayed tile at `idx` — the border-effect raster over the
    /// derived tone raster over the painted pixels, first one that applies.
    pub fn display_tile(&self, idx: TileIdx) -> Option<&Arc<Tile>> {
        if self.edge.is_some() {
            self.edge_tiles.as_ref()?.get(&idx)
        } else if self.tone.is_some() {
            self.tone_tiles.as_ref()?.get(&idx)
        } else if let LayerKind::Fill(_) = self.kind {
            self.fill_tiles.as_ref()?.get(&idx)
        } else {
            self.tiles.get(&idx)
        }
    }

    /// Re-derive the tone raster of stale source tiles at `dpi`. Cheap when
    /// nothing changed: the only work is comparing revisions.
    pub fn refresh_tone(&mut self, dpi: u32) {
        let Some(p) = self.tone else {
            if self.tone_tiles.is_some() {
                self.tone_tiles = None;
            }
            return;
        };
        let map = self.tone_tiles.get_or_insert_with(HashMap::new);
        // Sources that vanished (undo of the stroke that made them).
        map.retain(|idx, _| self.tiles.contains_key(idx));
        for (idx, src) in &self.tiles {
            let stale = match map.get(idx) {
                Some(t) => t.revision() < src.revision(),
                None => true,
            };
            if stale {
                let t = crate::tone::rasterize_tile(src, idx.origin(), &p, dpi);
                map.insert(*idx, Arc::new(t));
            }
        }
    }

    /// Re-derive the border-effect raster (`LP-002`/`LP-003`). Runs AFTER
    /// `refresh_tone`/`refresh_fill`, because the outline grows around what
    /// the layer actually shows — a tone layer gets a keyline around its
    /// dots, not around the grey it was painted with.
    ///
    /// # Cost, and the early-out that makes it liveable
    ///
    /// The render loop calls this every frame. A dilation is not pointwise,
    /// so the work is per CANDIDATE tile — every source tile plus the ring
    /// the outline can reach into — and a candidate's freshness depends on a
    /// whole NEIGHBOURHOOD of source revisions, not on one tile's. Doing
    /// that sweep 60 times a second on an idle page would be silly, so the
    /// cheap `(params, source-tile SET, newest revision)` triple
    /// short-circuits the whole function when nothing moved.
    pub fn refresh_edge(&mut self, size: (u32, u32)) {
        let Some(p) = self.edge else {
            if self.edge_tiles.is_some() || self.edge_stamp.is_some() {
                self.edge_tiles = None;
                self.edge_stamp = None;
            }
            return;
        };
        let stamp = {
            let base = self.base_tiles();
            // The SET, not its size. Counting was not enough: one op that
            // prunes an emptied tile and creates another leaves the count
            // equal, and the per-tile reuse below then keeps a ghost outline
            // round ink that is gone — its neighbourhood's newest revision
            // DROPPED, so "derived after the newest source" still holds.
            // Order-independent so the HashMap's iteration order cannot
            // fake a change; collisions only cost a needless re-derive.
            let keys = base
                .keys()
                .map(|k| (k.x as i64).wrapping_mul(0x9E37_79B9) ^ ((k.y as i64) << 21))
                .fold(0i64, i64::wrapping_add);
            (
                p,
                keys,
                base.values().map(|t| t.revision()).max().unwrap_or(0),
            )
        };
        if self.edge_stamp == Some(stamp) && self.edge_tiles.is_some() {
            return;
        }
        // Params or the source tile SET moved ⇒ every derived tile is
        // suspect; a pure edit keeps the cache and re-derives per tile below.
        if self.edge_stamp.map(|(p, k, _)| (p, k)) != Some((stamp.0, stamp.1)) {
            self.edge_tiles = None;
        }
        self.edge_stamp = Some(stamp);

        let r = p.reach();
        let ts = TILE_SIZE as i32;
        let span = r as i32 / ts + 1;
        // Rounded up on the UNSIGNED size: `div_ceil` is stable for u32 and
        // still unstable for i32 (`int_roundings`).
        let tsu = TILE_SIZE as u32;
        let (cw, chh) = (
            size.0.div_ceil(tsu) as i32,
            size.1.div_ceil(tsu) as i32,
        );
        let mut out: HashMap<TileIdx, Arc<Tile>> = HashMap::new();
        {
            let base = self.base_tiles();
            let old = self.edge_tiles.as_ref();
            // Candidates: source tiles dilated by the outline's tile reach,
            // clipped to the page (off-page outline is never displayed and
            // never exported, so deriving it is pure cost).
            let mut cands: std::collections::HashSet<TileIdx> = Default::default();
            for k in base.keys() {
                for dy in -span..=span {
                    for dx in -span..=span {
                        let i = TileIdx::new(k.x + dx, k.y + dy);
                        if i.x >= 0 && i.y >= 0 && i.x < cw && i.y < chh {
                            cands.insert(i);
                        }
                    }
                }
            }
            let side = TILE_SIZE + 2 * r;
            let mut seed = vec![crate::edge::INF; side * side];
            for idx in cands {
                let neighbours = || {
                    (-span..=span).flat_map(move |dy| {
                        (-span..=span).map(move |dx| TileIdx::new(idx.x + dx, idx.y + dy))
                    })
                };
                // Freshness: the newest source revision anywhere this tile's
                // outline can read from. Revisions come from one global
                // counter, so "derived after the newest source" is exact.
                let newest = neighbours()
                    .filter_map(|n| base.get(&n))
                    .map(|t| t.revision())
                    .max()
                    .unwrap_or(0);
                if let Some(t) = old.and_then(|m| m.get(&idx))
                    && t.revision() > newest
                {
                    out.insert(idx, t.clone());
                    continue;
                }
                seed.fill(crate::edge::INF);
                let (ox, oy) = idx.origin();
                let (px0, py0) = (ox - r as i32, oy - r as i32);
                for n in neighbours() {
                    let Some(t) = base.get(&n) else { continue };
                    let (nx, ny) = n.origin();
                    let x0 = px0.max(nx);
                    let x1 = (px0 + side as i32).min(nx + ts);
                    let y0 = py0.max(ny);
                    let y1 = (py0 + side as i32).min(ny + ts);
                    let d = t.data();
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let o = Tile::offset((x - nx) as usize, (y - ny) as usize);
                            if d[o + 3] >= crate::edge::INK_ALPHA {
                                seed[(y - py0) as usize * side + (x - px0) as usize] = 0.0;
                            }
                        }
                    }
                }
                let t = crate::edge::derive_tile(&mut seed, r, base.get(&idx).map(|a| &**a), p);
                out.insert(idx, Arc::new(t));
            }
        }
        self.edge_tiles = Some(out);
    }

    /// The frame layer's vector state, if this is one.
    pub fn frames(&self) -> Option<&FrameSet> {
        match &self.kind {
            LayerKind::Frame(fs) => Some(fs),
            _ => None,
        }
    }

    /// Mutable counterpart of [`Self::frames`] (the reading-order pin).
    pub fn frames_mut(&mut self) -> Option<&mut FrameSet> {
        match &mut self.kind {
            LayerKind::Frame(fs) => Some(fs),
            _ => None,
        }
    }

    /// The balloon layer's vector state, if this is one.
    pub fn balloons(&self) -> Option<&BalloonSet> {
        match &self.kind {
            LayerKind::Balloon(bs) => Some(bs),
            _ => None,
        }
    }

    /// The text layer's vector state, if this is one.
    pub fn texts(&self) -> Option<&TextSet> {
        match &self.kind {
            LayerKind::Text(ts) => Some(ts),
            _ => None,
        }
    }

    pub fn is_frame(&self) -> bool {
        matches!(self.kind, LayerKind::Frame(_))
    }

    pub fn is_balloon(&self) -> bool {
        matches!(self.kind, LayerKind::Balloon(_))
    }

    pub fn is_text(&self) -> bool {
        matches!(self.kind, LayerKind::Text(_))
    }

    /// Any layer whose raster is derived from vectors — painting on it would
    /// be overwritten by the next re-rasterize, and merge is refused.
    pub fn is_vector(&self) -> bool {
        !matches!(self.kind, LayerKind::Raster)
    }

    /// Brush/fill target? Folders organise, vector layers derive — neither
    /// takes ink.
    pub fn paintable(&self) -> bool {
        !self.folder && !self.is_vector()
    }

    /// Fill the whole canvas with opaque white, cheaply: every tile shares
    /// **one** allocation (the write path un-shares per tile on first paint).
    /// This is the "White" layer at the bottom of a frame folder — real and
    /// paintable, but costing one tile of memory even at B4 600dpi.
    pub fn fill_white(&mut self, size: (u32, u32)) {
        let mut t = Tile::new_transparent();
        t.data_mut().fill(crate::tile::FIX15_ONE as u16);
        let white = Arc::new(t);
        let tx = (size.0 as usize).div_ceil(TILE_SIZE) as i32;
        let ty = (size.1 as usize).div_ceil(TILE_SIZE) as i32;
        for y in 0..ty {
            for x in 0..tx {
                // Direct insert, not `set_tile`: that would `Arc::make_mut`
                // and copy the tile once per index, defeating the sharing.
                self.tiles.insert(TileIdx::new(x, y), white.clone());
            }
        }
    }

    /// Swap in a freshly derived tile set (frame rasterization). Must never run
    /// while an undo op is recording on this layer — derived pixels are not
    /// undone through tile snapshots.
    pub fn replace_tiles(&mut self, tiles: HashMap<TileIdx, Arc<Tile>>) {
        debug_assert!(self.recording.is_none(), "replace_tiles during an open op");
        self.tiles = tiles;
    }

    /// Read a tile if it exists. Missing == fully transparent.
    pub fn tile(&self, t: TileIdx) -> Option<&Tile> {
        self.tiles.get(&t).map(|a| &**a)
    }

    /// Get a tile for writing: creates a transparent one if absent, performs the
    /// copy-on-write unshare, and bumps the revision so the GPU re-uploads.
    ///
    /// If an undo op is open on this layer (`Document::begin_op`), the tile's
    /// pre-image is recorded here on first touch. That is the whole reason the
    /// brush crate needs no undo awareness.
    pub fn tile_mut(&mut self, t: TileIdx) -> &mut Tile {
        // Disjoint field borrows: `recording` and `tiles` are separate fields.
        if let Some(rec) = &mut self.recording {
            if !rec.contains_key(&t) {
                rec.insert(t, self.tiles.get(&t).cloned());
            }
        }
        let arc = self
            .tiles
            .entry(t)
            .or_insert_with(|| Arc::new(Tile::new_transparent()));
        let tile = Arc::make_mut(arc);
        tile.touch();
        tile
    }

    /// Shared handle to a tile — the cheap snapshot undo will take.
    pub fn tile_arc(&self, t: TileIdx) -> Option<&Arc<Tile>> {
        self.tiles.get(&t)
    }

    /// Restore/replace a tile wholesale (`None` removes it). Undo's write path.
    pub fn set_tile(&mut self, t: TileIdx, tile: Option<Arc<Tile>>) {
        // Whole-tile inserts participate in undo like pixel writes: record
        // the pre-image once when an op is armed (found by the round-60
        // generator — set_tile silently bypassed the op bracket).
        if let Some(rec) = self.recording.as_mut() {
            rec.entry(t).or_insert_with(|| self.tiles.get(&t).cloned());
        }
        match tile {
            Some(mut a) => {
                Arc::make_mut(&mut a).touch();
                self.tiles.insert(t, a);
            }
            None => {
                self.tiles.remove(&t);
            }
        }
    }

    /// Iterate the populated tiles in unspecified order.
    pub fn tiles(&self) -> impl Iterator<Item = (TileIdx, &Arc<Tile>)> {
        self.tiles.iter().map(|(k, v)| (*k, v))
    }

    /// Spread combine/split (PM-030/033): one horizontal pixel move applied
    /// to EVERY raster plane the layer owns — the ink, the layer mask, and
    /// the derived tone/fill/edge rasters. Pixels with canvas x in
    /// `[keep.0, keep.1)` land at `x + dx`, clipped to `(w, h)`; the rest
    /// drop. The derived planes MOVE instead of clearing on purpose: the
    /// tone screen is derived on the pre-split geometry precisely so both
    /// halves keep the spread's dot phase (the page export derives before
    /// splitting and does not re-derive after). Before this existed the
    /// split cloned the derived maps at their SPREAD coordinates, so a
    /// split-spread print carried the left half's tone dots on the right
    /// page. The mask plane keeps its ZERO-coverage pixels: an absent mask
    /// tile reads as visible on the export path, so dropping a hidden
    /// region's pixels would un-hide it.
    pub(crate) fn remap_planes_x(&mut self, keep: (i64, i64), dx: i64, w: u32, h: u32) {
        let ts = TILE_SIZE as i64;
        let remap = |map: &HashMap<TileIdx, Arc<Tile>>,
                     keep_zero: bool|
         -> HashMap<TileIdx, Arc<Tile>> {
            let mut out: HashMap<TileIdx, Arc<Tile>> = HashMap::new();
            for (ti, t) in map {
                let (ox, oy) = ti.origin();
                for py in 0..TILE_SIZE {
                    for px in 0..TILE_SIZE {
                        let p = t.pixel(px, py);
                        if p[3] == 0 && !keep_zero {
                            continue;
                        }
                        let (x, y) = (ox as i64 + px as i64, oy as i64 + py as i64);
                        if x < keep.0 || x >= keep.1 || y < 0 || y >= h as i64 {
                            continue;
                        }
                        let nx = x + dx;
                        if nx < 0 || nx >= w as i64 {
                            continue;
                        }
                        let ni = TileIdx::of_pixel(nx as i32, y as i32);
                        let tile = out
                            .entry(ni)
                            .or_insert_with(|| Arc::new(Tile::new_transparent()));
                        Arc::make_mut(tile).set_pixel(
                            (nx - ni.x as i64 * ts) as usize,
                            (y - ni.y as i64 * ts) as usize,
                            p,
                        );
                    }
                }
            }
            for t in out.values_mut() {
                Arc::make_mut(t).touch();
            }
            out
        };
        self.tiles = remap(&self.tiles, false);
        if let Some(m) = &mut self.mask {
            m.tiles = remap(&m.tiles, true);
            m.revision = crate::tile::next_revision();
        }
        if let Some(t) = self.tone_tiles.take() {
            self.tone_tiles = Some(remap(&t, false));
        }
        if let Some(t) = self.edge_tiles.take() {
            self.edge_tiles = Some(remap(&t, false));
        }
        if let Some(t) = self.fill_tiles.take() {
            self.fill_tiles = Some(remap(&t, false));
        }
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Highest revision present, or 0 for an empty layer. Lets the renderer skip
    /// work without walking every tile twice.
    pub fn max_revision(&self) -> u64 {
        self.tiles.values().map(|t| t.revision()).max().unwrap_or(0)
    }

    /// Bounding box of the populated tiles in **canvas pixels**, tile-aligned:
    /// `(x, y, w, h)`. `None` when the layer has no tiles. Used by ORA save,
    /// which stores each layer cropped with an x/y offset.
    pub fn tile_bounds(&self) -> Option<(i32, i32, u32, u32)> {
        let mut it = self.tiles.keys();
        let first = *it.next()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for k in it {
            x0 = x0.min(k.x);
            y0 = y0.min(k.y);
            x1 = x1.max(k.x);
            y1 = y1.max(k.y);
        }
        let t = TILE_SIZE as i32;
        Some((
            x0 * t,
            y0 * t,
            ((x1 - x0 + 1) * t) as u32,
            ((y1 - y0 + 1) * t) as u32,
        ))
    }

    /// Arm undo recording. Idempotent: re-arming keeps the existing recording so
    /// a stray `begin_op` cannot silently split a stroke's snapshot in two.
    fn arm_recording(&mut self) {
        if self.recording.is_none() {
            self.recording = Some(HashMap::new());
        }
    }

    /// Disarm and take whatever was recorded.
    fn take_recording(&mut self) -> Option<HashMap<TileIdx, Option<Arc<Tile>>>> {
        self.recording.take()
    }

    /// True while `tile_mut` is snapshotting into an open op.
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Tiles the open recording has captured so far (unspecified order).
    pub fn recorded_tiles(&self) -> Vec<TileIdx> {
        self.recording
            .as_ref()
            .map(|r| r.keys().copied().collect())
            .unwrap_or_default()
    }

    /// The recorded pre-image of a tile. `None` = not recorded, or recorded as
    /// previously absent (fully transparent) — callers treat both as "was
    /// transparent", which is correct for tiles listed by `recorded_tiles`.
    pub fn recorded_pre_image(&self, t: TileIdx) -> Option<&Arc<Tile>> {
        self.recording
            .as_ref()
            .and_then(|r| r.get(&t))
            .and_then(|o| o.as_ref())
    }

    /// Shift all tile content and vector geometry by `(dx_px, dy_px)` canvas
    /// pixels. Only shifts that are an exact multiple of `TILE_SIZE` are
    /// supported (spread combine/split and canvas resizes always qualify —
    /// they shift by whole page widths). Sub-tile offsets are a no-op.
    ///
    /// Raster: tiles remapped to new indices.
    /// Mask tiles (frame folder coverage): remapped the same way.
    /// Frame/balloon/text vectors: coordinates translated by (dx_px, dy_px).
    pub fn shift_content(&mut self, dx_px: i32, dy_px: i32) {
        if dx_px == 0 && dy_px == 0 {
            return;
        }
        let ts = TILE_SIZE as i32;
        // Only whole-tile shifts are supported.
        if dx_px % ts != 0 || dy_px % ts != 0 {
            return;
        }
        let dtx = dx_px / ts;
        let dty = dy_px / ts;

        // Remap raster tiles.
        let old = std::mem::take(&mut self.tiles);
        self.tiles = old
            .into_iter()
            .map(|(ti, arc)| (TileIdx::new(ti.x + dtx, ti.y + dty), arc))
            .collect();

        // Remap mask tiles (frame folder coverage mask).
        if let Some(mask) = self.mask_tiles.as_mut() {
            let old_mask = std::mem::take(mask);
            *mask = old_mask
                .into_iter()
                .map(|(ti, arc)| (TileIdx::new(ti.x + dtx, ti.y + dty), arc))
                .collect();
        }
        // Derived tone/border rasters re-derive from the shifted sources.
        self.tone_tiles = None;
        self.edge_tiles = None;
        self.edge_stamp = None;

        self.translate_vectors(dx_px as f32, dy_px as f32);
    }

    /// Shift ALL content by whole PIXELS (sub-tile accurate, unlike
    /// `shift_content`, which snaps to the tile grid). Raster tiles blit
    /// with a fractional-tile offset, splitting across up to four
    /// destination tiles; derived frame masks are dropped (the folder's
    /// own `set_frames` re-derives them); vectors translate exactly.
    /// This is the Object tool's move-frame-folder-with-content seam.
    pub fn translate_content(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        // If an op is recording, snapshot every SOURCE tile before the
        // mem::take below empties the map — after it, tile_mut can only see
        // an empty map and records `None` pre-images, so undo DELETED the
        // moved art instead of putting it back (GLM-audit survivor #1).
        // Destinations still record through tile_mut as usual; a destination
        // that was also a source keeps its true pre-image via or_insert.
        if self.recording.is_some() {
            let pre: Vec<_> = self.tiles.iter().map(|(k, v)| (*k, v.clone())).collect();
            if let Some(rec) = &mut self.recording {
                for (k, v) in pre {
                    rec.entry(k).or_insert(Some(v));
                }
            }
        }
        // Raster: per-source-tile blit at a pixel offset. The tile grid is
        // unbounded (content may rest off-canvas), so the destination tile
        // comes from euclidean division — negative origins included.
        let ts = TILE_SIZE as i32;
        let old = std::mem::take(&mut self.tiles);
        self.tiles = Default::default();
        for (ti, arc) in old {
            if arc.data().iter().all(|c| *c == 0) {
                continue; // a zeroed tile (undo residue) shifts to nothing
            }
            let (ox, oy) = ti.origin();
            let (fx, fy) = (ox + dx, oy + dy);
            let (nx0, ny0) = (fx.div_euclid(ts), fy.div_euclid(ts));
            let (lx, ly) = (fx.rem_euclid(ts) as usize, fy.rem_euclid(ts) as usize);
            for (oxs, oys) in [(0i32, 0i32), (1, 0), (0, 1), (1, 1)] {
                // The remainder (lx, ly) splits each axis: the first chunk
                // of the source lands in this dst tile, the tail in the next.
                let (sx0, sx1) = if oxs == 0 {
                    (0, TILE_SIZE - lx)
                } else {
                    (TILE_SIZE - lx, TILE_SIZE)
                };
                let (sy0, sy1) = if oys == 0 {
                    (0, TILE_SIZE - ly)
                } else {
                    (TILE_SIZE - ly, TILE_SIZE)
                };
                if sx0 >= sx1 || sy0 >= sy1 {
                    continue;
                }
                let dst = TileIdx::new(nx0 + oxs, ny0 + oys);
                let tile = self.tile_mut(dst);
                for y in sy0..sy1 {
                    for x in sx0..sx1 {
                        let v = arc.pixel(x, y);
                        if v[3] > 0 {
                            tile.set_pixel(
                                lx + x - oxs as usize * TILE_SIZE,
                                ly + y - oys as usize * TILE_SIZE,
                                v,
                            );
                        }
                    }
                }
            }
        }
        // LM-009: linked (the default) — the user mask rides with the
        // content, sub-tile accurate like the raster above; unlinked, it
        // stays put and the art slides underneath it.
        if self.mask_linked
            && let Some(m) = &mut self.mask
        {
            let shifted = shift_tile_map(&m.tiles, dx, dy);
            m.tiles = shifted;
            m.revision = crate::tile::next_revision();
        }
        // Derived masks are regenerated by the frame re-raster.
        self.mask_tiles = None;
        // Derived tone/border rasters re-derive from the shifted sources.
        self.tone_tiles = None;
        self.edge_tiles = None;
        self.edge_stamp = None;

        self.translate_vectors(dx as f32, dy as f32);
    }

    /// Translate every vector geometry (frame points, balloon shapes and
    /// tails, text positions) — shared by the content-shift seams.
    fn translate_vectors(&mut self, dx: f32, dy: f32) {
        match &mut self.kind {
            LayerKind::Fill(crate::fill_layer::FillKind::Gradient { a, b, .. }) => {
                a[0] += dx;
                a[1] += dy;
                b[0] += dx;
                b[1] += dy;
            }
            LayerKind::Fill(_) => {}
            LayerKind::Frame(fs) => {
                for f in &mut fs.frames {
                    for p in &mut f.points {
                        p[0] += dx;
                        p[1] += dy;
                    }
                }
            }
            LayerKind::Balloon(bs) => {
                use crate::balloon::BalloonShape;
                for b in &mut bs.balloons {
                    match &mut b.shape {
                        BalloonShape::Ellipse { center, .. } => {
                            center[0] += dx;
                            center[1] += dy;
                        }
                        BalloonShape::RoundRect { rect, .. } => {
                            rect[0] += dx;
                            rect[1] += dy;
                            rect[2] += dx;
                            rect[3] += dy;
                        }
                        BalloonShape::Polygon { points, .. } => {
                            for p in points.iter_mut() {
                                p[0] += dx;
                                p[1] += dy;
                            }
                        }
                    }
                    for tail in &mut b.tails {
                        tail.base[0] += dx;
                        tail.base[1] += dy;
                        tail.tip[0] += dx;
                        tail.tip[1] += dy;
                    }
                }
            }
            LayerKind::Text(ts) => {
                for item in &mut ts.texts {
                    item.pos[0] += dx;
                    item.pos[1] += dy;
                }
            }
            LayerKind::Raster => {}
        }
    }
}

impl Default for Layer {
    fn default() -> Self {
        Self::new("Layer 1")
    }
}

/// Default canvas: 2048x2048. Print-res documents (B4 600dpi) come later.
pub const DEFAULT_SIZE: (u32, u32) = (2048, 2048);

#[derive(Clone, Debug)]
pub struct Document {
    pub layers: Vec<Layer>,
    pub active: usize,
    pub size: (u32, u32),
    /// Bumped whenever a stroke ends; lets the shell invalidate cheaply.
    pub revision: u64,
    /// Active selection; `None` = everything selectable (CSP semantics).
    /// See `core::selection` for the op-masking mechanism.
    pub selection: Option<crate::selection::Selection>,
    /// The selection-paint stroke's live scratch (selection pen / eraser
    /// / Quick Mask): the brush engine paints coverage HERE (a mask field
    /// — alpha is the payload), the overlay previews the ants per frame,
    /// and `end_stroke` commits it into `selection` through SE-022's
    /// combine. On the Document because the engine's surface reaches its
    /// target through the document pointer.
    pub sel_scratch: LayerMask,
    /// Undo/redo stacks. Private: drive it with `begin_op`/`end_op`/`undo`/`redo`.
    history: History,
    /// Layer index an open op is recording into (`None` = no op open).
    op_layer: Option<usize>,
    /// LM-004: the stroke bracket's mask snapshot.
    mask_op_snapshot: Option<(LayerMask, u64)>,
    /// CV-003: the NEXT op's History-palette label, set by the caller
    /// between `begin_op` and `end_op` ("Stroke", "Fill", …). Consumed
    /// by `end_op`; unset = "Edit".
    pub pending_op_label: Option<String>,
    /// LC-001 layer comps (TRIAGE 139): named whole-visibility snapshots.
    /// Positional — comp.vis[i] maps to layers[i] — because the comp's
    /// daily use (text/no-text chapter versions) has IDENTICAL structure
    /// on every page; a length mismatch refuses on apply rather than
    /// guessing. Persisted as `mnc-comps` on the ORA image element.
    pub comps: Vec<LayerComp>,
    /// PA-001: the paper under the stack. Drive it with `set_paper_colour`
    /// (undoable) / `set_paper_visible` (view state, like a layer's eye).
    pub paper: Paper,
}

/// PA-001: the opaque base underneath the whole stack — exactly one per
/// canvas, never a row you can delete, reorder or draw on.
///
/// Two knobs. The **colour** is document content: it is what an empty page
/// composites to and what an export writes, so a cream page prints cream.
/// The **eye** is a view state and deliberately does NOT reach export
/// (`export::composite_for_export` is handed the colour whatever the eye
/// says): hiding the paper swaps it for the transparency checker so a
/// missed spot in a flat fill — invisible against white, obvious against a
/// checker — shows up before print, and that check must never be one
/// keystroke away from shipping a page with a transparent background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paper {
    /// False = the canvas shows the transparency checker instead.
    pub visible: bool,
    pub colour: [u8; 3],
}

impl Default for Paper {
    /// Opaque white: exactly what every document did before PA-001, so old
    /// files load and render unchanged.
    fn default() -> Self {
        Self {
            visible: true,
            colour: [255, 255, 255],
        }
    }
}

/// One layer comp (TRIAGE 139, LC-001): every layer's visibility flag
/// under a name.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LayerComp {
    pub name: String,
    pub vis: Vec<bool>,
}

/// Shift a tile map by whole pixels, sub-tile accurate: each source tile
/// blits into up to four destination tiles (the same split
/// `translate_content` uses for the raster). Zero tiles drop out; the
/// result is a fresh map of fresh tiles (no Arc sharing with the source).
pub fn shift_tile_map(
    map: &HashMap<TileIdx, Arc<Tile>>,
    dx: i32,
    dy: i32,
) -> HashMap<TileIdx, Arc<Tile>> {
    let mut out: HashMap<TileIdx, Arc<Tile>> = HashMap::new();
    let ts = TILE_SIZE as i32;
    for (ti, arc) in map {
        if arc.data().iter().all(|c| *c == 0) {
            continue;
        }
        let (ox, oy) = ti.origin();
        let (fx, fy) = (ox + dx, oy + dy);
        let (nx0, ny0) = (fx.div_euclid(ts), fy.div_euclid(ts));
        let (lx, ly) = (fx.rem_euclid(ts) as usize, fy.rem_euclid(ts) as usize);
        for (oxs, oys) in [(0i32, 0i32), (1, 0), (0, 1), (1, 1)] {
            let (sx0, sx1) = if oxs == 0 {
                (0, TILE_SIZE - lx)
            } else {
                (TILE_SIZE - lx, TILE_SIZE)
            };
            let (sy0, sy1) = if oys == 0 {
                (0, TILE_SIZE - ly)
            } else {
                (TILE_SIZE - ly, TILE_SIZE)
            };
            if sx0 >= sx1 || sy0 >= sy1 {
                continue;
            }
            let dst = TileIdx::new(nx0 + oxs, ny0 + oys);
            let tile = out
                .entry(dst)
                .or_insert_with(|| Arc::new(Tile::new_transparent()));
            let tile = Arc::make_mut(tile);
            for y in sy0..sy1 {
                for x in sx0..sx1 {
                    let v = arc.pixel(x, y);
                    if v[3] > 0 {
                        tile.set_pixel(
                            lx + x - oxs as usize * TILE_SIZE,
                            ly + y - oys as usize * TILE_SIZE,
                            v,
                        );
                    }
                }
            }
        }
    }
    out
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            layers: vec![Layer::new("Layer 1")],
            active: 0,
            size: (width, height),
            revision: next_revision(),
            selection: None,
            sel_scratch: LayerMask {
                tiles: HashMap::new(),
                enabled: true,
                revision: 0,
            },
            history: History::new(),
            op_layer: None,
            mask_op_snapshot: None,
            pending_op_label: None,
            comps: Vec::new(),
            paper: Paper::default(),
        }
    }

    /// Push a MaskField undo group (the BEFORE state; masks are small,
    /// whole-field snapshots — the Frames/Texts pattern).
    fn push_mask_group(&mut self, layer: usize, before: Option<LayerMask>, label: &str) {
        self.history.push_labeled(
            label,
            UndoGroup::Mask {
                layer,
                mask: before,
            },
        );
    }

    /// Push a Mask undo group for `layer` with `before` as the pre-image.
    /// For mutations that shift a linked mask OUTSIDE the LM-004 stroke
    /// bracket (the Object tool's folder move): snapshot the mask first,
    /// and hand the before-state here when the revision moved.
    pub fn record_mask_change(&mut self, layer: usize, before: Option<LayerMask>, label: &str) {
        self.push_mask_group(layer, before, label);
    }

    /// LM-004 stroke bracket: snapshot at stroke start; mask_op_end pushes
    /// one group when the coverage changed (revision moved).
    pub fn mask_op_begin(&mut self) {
        self.mask_op_snapshot = self
            .active_layer()
            .mask
            .as_ref()
            .map(|m| (m.clone(), m.revision));
    }

    /// Returns true when a group was pushed.
    pub fn mask_op_end(&mut self) -> bool {
        let Some((before, rev0)) = self.mask_op_snapshot.take() else {
            return false;
        };
        let changed = self
            .active_layer()
            .mask
            .as_ref()
            .map(|m| m.revision != rev0)
            .unwrap_or(true);
        if !changed {
            return false;
        }
        let li = self.active;
        self.push_mask_group(li, Some(before), "Mask stroke");
        true
    }

    /// LM-002: Mask Outside Selection — the mask hides everything the
    /// selection does not cover (coverage = the selection). No selection
    /// = the whole layer is masked. Returns false for a bad layer.
    pub fn mask_outside_selection(&mut self, index: usize) -> bool {
        let sel = self.selection.clone();
        self.mask_from_coverage(index, move |x, y| {
            sel.as_ref()
                .map(|s| s.coverage(x, y) as u32 * 32768 / 255)
                .unwrap_or(0)
        })
    }

    /// LM-006: apply the mask to the layer — bake. Every layer pixel is
    /// multiplied by its coverage (premultiplied fix15), the mask is
    /// deleted, and the whole thing is ONE undoable op (tile pre-images).
    /// A disabled mask bakes nothing (CSP compares by toggling first).
    pub fn mask_apply_bake(&mut self, index: usize) -> bool {
        let Some(mask) = self
            .layers
            .get(index)
            .and_then(|l| l.mask.clone())
            .filter(|m| m.enabled)
        else {
            return false;
        };
        let idxs: Vec<TileIdx> = self.layers[index].tiles().map(|(i, _)| i).collect();
        self.begin_op();
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        for ti in idxs {
            let cov = mask.tiles.get(&ti);
            let tile = l.tile_mut(ti);
            let d = tile.data_mut();
            for p in 0..d.len() / 4 {
                let m = cov.map(|c| c.data()[p * 4 + 3] as u32).unwrap_or(0);
                if m == 32768 {
                    continue;
                }
                for c in 0..4 {
                    let i = p * 4 + c;
                    d[i] = (d[i] as u32 * m / 32768) as u16;
                }
            }
        }
        self.end_op();
        self.mask_delete(index);
        true
    }

    /// LM-001: Mask Selection — CSP's starter mask: ALL-VISIBLE (hides
    /// nothing yet; you paint the hiding in — part 2). With a selection
    /// present CSP still starts blank; the row's text is explicit.
    pub fn mask_selection_blank(&mut self, index: usize) -> bool {
        self.mask_from_coverage(index, |_, _| 32768)
    }

    fn mask_from_coverage(&mut self, index: usize, cov: impl Fn(i32, i32) -> u32) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if !l.paintable() || l.lock || l.folder {
            return false;
        }
        let idxs: Vec<TileIdx> = l.tiles().map(|(i, _)| i).collect();
        if idxs.is_empty() {
            return false;
        }
        let mut mask = LayerMask {
            tiles: HashMap::new(),
            enabled: true,
            revision: crate::tile::next_revision(),
        };
        for ti in idxs {
            let (ox, oy) = ti.origin();
            let mut t = Tile::new_transparent();
            let d = t.data_mut();
            for p in 0..crate::tile::TILE_PIXELS {
                let (x, y) = (
                    ox + (p % crate::tile::TILE_SIZE) as i32,
                    oy + (p / crate::tile::TILE_SIZE) as i32,
                );
                let c = cov(x, y).min(32768) as u16;
                d[p * 4] = c;
                d[p * 4 + 1] = c;
                d[p * 4 + 2] = c;
                d[p * 4 + 3] = c;
            }
            mask.tiles.insert(ti, Arc::new(t));
        }
        self.layers[index].mask = Some(mask);
        self.push_mask_group(index, None, "Mask");
        self.touch();
        true
    }

    /// LM-007: toggle the mask's effect without deleting it.
    pub fn mask_set_enabled(&mut self, index: usize, on: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let Some(m) = l.mask.as_mut() else {
            return false;
        };
        let before = m.clone();
        m.enabled = on;
        m.revision = crate::tile::next_revision();
        self.push_mask_group(index, Some(before), "Mask");
        self.touch();
        true
    }

    /// LM-003: Delete Mask — remove entirely (the layer shows as-is).
    pub fn mask_delete(&mut self, index: usize) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let before = l.mask.take();
        if before.is_some() {
            self.push_mask_group(index, before, "Mask");
            self.touch();
            true
        } else {
            false
        }
    }

    /// LM-003: Clear Mask — keep the mask, empty its coverage (all
    /// hidden). The two destructive actions stay distinct commands.
    pub fn mask_clear(&mut self, index: usize) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let Some(m) = l.mask.as_mut() else {
            return false;
        };
        let before = m.clone();
        for t in m.tiles.values_mut() {
            Arc::make_mut(t).data_mut().fill(0);
        }
        m.revision = crate::tile::next_revision();
        self.push_mask_group(index, Some(before), "Mask");
        self.touch();
        true
    }

    /// EL-002: luminance → alpha per pixel (white becomes transparent,
    /// black stays opaque; Rec.709 luma on the UNPREMULTIPLIED colour,
    /// scaled by the existing alpha). THE scanned-lineart import path —
    /// one undoable op over the layer's tiles.
    pub fn convert_brightness_to_opacity(&mut self, index: usize) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if !l.paintable() || l.lock {
            return false;
        }
        let idxs: Vec<TileIdx> = l.tiles().map(|(i, _)| i).collect();
        if idxs.is_empty() {
            return false;
        }
        self.begin_op();
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        for ti in idxs {
            let tile = l.tile_mut(ti);
            let data = tile.data_mut();
            for p in 0..data.len() / 4 {
                let i = p * 4;
                let (r, g, b, a) = (
                    data[i] as u32,
                    data[i + 1] as u32,
                    data[i + 2] as u32,
                    data[i + 3] as u32,
                );
                if a == 0 {
                    continue;
                }
                // Un-premultiply (fix15), luma, then re-premultiply with the
                // new alpha = a · (1 − luma).
                let un = |c: u32| c * 32768 / a;
                let (ur, ug, ub) = (un(r), un(g), un(b));
                let luma = (ur * 6967 + ug * 23435 + ub * 2366) / 32768; // Rec.709 in fix15 (sums to 32768)
                let na = a * (32768 - luma) / 32768;
                let sc = |c: u32| (un(c) * na / 32768).min(na) as u16;
                data[i] = sc(r);
                data[i + 1] = sc(g);
                data[i + 2] = sc(b);
                data[i + 3] = na as u16;
            }
        }
        self.end_op();
        true
    }

    /// CV-003: name the op being opened (call between the begin and the
    /// end; the History palette shows it).
    pub fn set_op_label(&mut self, label: &str) {
        self.pending_op_label = Some(label.to_string());
    }

    /// CV-003: the undo stack's labels, oldest first.
    pub fn undo_labels(&self) -> &[String] {
        self.history.undo_labels()
    }

    /// CV-003: the redo branch's labels, oldest first.
    pub fn redo_labels(&self) -> Vec<String> {
        self.history.redo_labels()
    }

    /// Layer index of the open op, if one is open.
    pub fn op_layer_index(&self) -> Option<usize> {
        self.op_layer
    }

    pub fn active_layer(&self) -> &Layer {
        &self.layers[self.active]
    }

    pub fn active_layer_mut(&mut self) -> &mut Layer {
        let i = self.active;
        &mut self.layers[i]
    }

    /// Number of tiles spanning the canvas, (across, down).
    pub fn tile_extent(&self) -> (i32, i32) {
        let t = TILE_SIZE as u32;
        (
            self.size.0.div_ceil(t) as i32,
            self.size.1.div_ceil(t) as i32,
        )
    }

    /// True when the canvas pixel is inside the document.
    #[inline]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as u32) < self.size.0 && (y as u32) < self.size.1
    }

    /// Highest tile revision across all layers.
    pub fn max_revision(&self) -> u64 {
        self.layers
            .iter()
            .map(Layer::max_revision)
            .max()
            .unwrap_or(0)
    }

    /// Publish a new document revision. Call after any change the tile-revision
    /// path cannot see (layer opacity, visibility, order, name).
    pub fn touch(&mut self) {
        self.revision = next_revision();
    }

    // ---------------------------------------------------------------- undo --

    /// Open an undo op on the **active** layer.
    ///
    /// Every `Layer::tile_mut` between here and `end_op` snapshots its
    /// pre-image, so a `StrokeSink` becomes undoable without knowing undo
    /// exists. Calling `begin_op` while an op is already open does nothing (the
    /// existing recording keeps accumulating) — a stroke is never split in two.
    ///
    /// Note the recording is armed on the layer that is active *now*; switching
    /// layers mid-op keeps recording into the original one.
    pub fn begin_op(&mut self) {
        let li = self.active.min(self.layers.len().saturating_sub(1));
        self.begin_op_on(li);
    }

    /// Open an undo op on a SPECIFIC layer. The Object tool's folder move
    /// records each child in turn — children are almost never the active
    /// layer, and `begin_op` recording "whichever layer happened to be
    /// active" is exactly how a folder drag became un-undoable art loss.
    /// Same no-nesting rule as `begin_op`.
    pub fn begin_op_on(&mut self, li: usize) {
        if self.op_layer.is_some() || li >= self.layers.len() {
            return;
        }
        self.layers[li].arm_recording();
        self.op_layer = Some(li);
    }

    /// Close the open op and push it onto the undo stack.
    ///
    /// Returns `true` when a group was actually pushed — an op that touched no
    /// tiles (pen-down/pen-up with no movement inside the canvas) pushes
    /// nothing, so undo never eats a no-op.
    pub fn end_op(&mut self) -> bool {
        let Some(li) = self.op_layer.take() else {
            return false;
        };
        let Some(rec) = self.layers.get_mut(li).and_then(Layer::take_recording) else {
            return false;
        };
        if rec.is_empty() {
            return false;
        }
        let mut tiles: Vec<(TileIdx, Option<Arc<Tile>>)> = rec.into_iter().collect();
        // HashMap order is not deterministic; groups are compared in tests and
        // replayed in order, so sort them.
        tiles.sort_by_key(|(idx, _)| (idx.y, idx.x));
        let label = self
            .pending_op_label
            .take()
            .unwrap_or_else(|| "Edit".into());
        self.history
            .push_labeled(&label, UndoGroup::Tiles { layer: li, tiles });
        self.touch();
        true
    }

    /// Drop the open op's recording **without** restoring anything. The pixels
    /// stay; they just stop being undoable. Used when the history is discarded.
    pub fn cancel_op(&mut self) {
        if let Some(li) = self.op_layer.take() {
            if let Some(l) = self.layers.get_mut(li) {
                l.take_recording();
            }
        }
    }

    pub fn is_op_open(&self) -> bool {
        self.op_layer.is_some()
    }

    /// Restore the newest undo group. Returns `false` when there is nothing to
    /// undo. An op left open is closed first, so ctrl-Z mid-stroke is safe.
    pub fn undo(&mut self) -> bool {
        if self.op_layer.is_some() {
            self.end_op();
        }
        let Some((label, group)) = self.history.pop_undo_labeled() else {
            return false;
        };
        match self.swap_group(group) {
            Some(inverse) => {
                self.history.push_redo_labeled(&label, inverse);
                self.touch();
                true
            }
            None => false,
        }
    }

    /// Re-apply the newest undone group. Returns `false` when there is nothing
    /// to redo.
    pub fn redo(&mut self) -> bool {
        if self.op_layer.is_some() {
            self.end_op();
        }
        let Some((label, group)) = self.history.pop_redo_labeled() else {
            return false;
        };
        match self.swap_group(group) {
            Some(inverse) => {
                self.history.push_undo_keep_redo_labeled(&label, inverse);
                self.touch();
                true
            }
            None => false,
        }
    }

    /// Swap a group's state into the document, returning the group that undoes
    /// the swap (i.e. the state that was there a moment ago).
    ///
    /// `Layer::set_tile` stamps a fresh revision on every restored tile, so the
    /// GPU cache re-uploads them; that is why the revision counter is global.
    /// Frame groups swap the vector state and re-rasterize — the fresh tiles
    /// carry fresh revisions for the same reason.
    fn swap_group(&mut self, group: UndoGroup) -> Option<UndoGroup> {
        match group {
            UndoGroup::Tiles { layer, tiles } => {
                let l = self.layers.get_mut(layer)?;
                let mut inverse = Vec::with_capacity(tiles.len());
                for (idx, snapshot) in tiles {
                    inverse.push((idx, l.tile_arc(idx).cloned()));
                    l.set_tile(idx, snapshot);
                }
                Some(UndoGroup::Tiles {
                    layer,
                    tiles: inverse,
                })
            }
            UndoGroup::Frames { layer, frames } => {
                let size = self.size;
                let l = self.layers.get_mut(layer)?;
                let LayerKind::Frame(cur) = &mut l.kind else {
                    return None;
                };
                let inverse = UndoGroup::Frames {
                    layer,
                    frames: cur.clone(),
                };
                *cur = frames;
                Self::derive_frame_raster(l, size);
                Some(inverse)
            }
            UndoGroup::Balloons { layer, balloons } => {
                let size = self.size;
                let l = self.layers.get_mut(layer)?;
                let LayerKind::Balloon(cur) = &mut l.kind else {
                    return None;
                };
                let inverse = UndoGroup::Balloons {
                    layer,
                    balloons: cur.clone(),
                };
                let raster = balloons.rasterize(size);
                *cur = balloons;
                l.replace_tiles(raster);
                Some(inverse)
            }
            UndoGroup::Texts { layer, texts } => {
                let size = self.size;
                let l = self.layers.get_mut(layer)?;
                let LayerKind::Text(cur) = &mut l.kind else {
                    return None;
                };
                let inverse = UndoGroup::Texts {
                    layer,
                    texts: cur.clone(),
                };
                let raster = texts.rasterize(size);
                *cur = texts;
                l.replace_tiles(raster);
                Some(inverse)
            }
            UndoGroup::Mask { layer, mask } => {
                let l = self.layers.get_mut(layer)?;
                let inverse = UndoGroup::Mask {
                    layer,
                    mask: l.mask.clone(),
                };
                l.mask = mask;
                if let Some(m) = l.mask.as_mut() {
                    m.revision = crate::tile::next_revision();
                }
                Some(inverse)
            }
            UndoGroup::Tones { layer, tone } => {
                let l = self.layers.get_mut(layer)?;
                let inverse = UndoGroup::Tones {
                    layer,
                    tone: l.tone,
                };
                l.tone = tone;
                l.tone_tiles = None;
                // The border effect grows around the tone raster, so a tone
                // undo invalidates it too.
                l.edge_tiles = None;
                l.edge_stamp = None;
                Some(inverse)
            }
            UndoGroup::Edges { layer, edge } => {
                let l = self.layers.get_mut(layer)?;
                let inverse = UndoGroup::Edges {
                    layer,
                    edge: l.edge,
                };
                l.edge = edge;
                l.edge_tiles = None;
                l.edge_stamp = None;
                Some(inverse)
            }
            UndoGroup::Paper { colour } => {
                let inverse = UndoGroup::Paper {
                    colour: self.paper.colour,
                };
                self.paper.colour = colour;
                Some(inverse)
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn undo_len(&self) -> usize {
        self.history.undo_len()
    }

    pub fn redo_len(&self) -> usize {
        self.history.redo_len()
    }

    /// PR-041: undoable operations performed on this document, ever —
    /// monotonic, and unaffected by the depth cap or by `clear_history`.
    /// The edge the "save recovery data for every operation" preference
    /// fires on; see `undo::History`'s `ops` field for why neither
    /// `undo_len` nor `revision` would do.
    pub fn op_count(&self) -> u64 {
        self.history.ops()
    }

    /// How many undo groups this document keeps (the `undo_depth`
    /// preference; [`crate::undo::UNDO_LIMIT`] until something sets it).
    pub fn undo_limit(&self) -> usize {
        self.history.limit()
    }

    /// Apply the `undo_depth` preference. Trims immediately when lowered.
    pub fn set_undo_limit(&mut self, limit: usize) {
        self.history.set_limit(limit);
    }

    /// Throw the history away (file load, or a change undo cannot express).
    pub fn clear_history(&mut self) {
        self.cancel_op();
        self.history.clear();
        // PR-041: the structural layer ops (add, delete, reorder, import)
        // are the ones that come through here INSTEAD of pushing a group,
        // so counting only pushes would leave exactly the unrecoverable
        // changes uncounted. `clear` deliberately does not reset the tally.
        self.history.note_op();
    }

    // ---------------------------------------------------------- layer ops --
    //
    // Order convention: `layers[0]` is the **bottom** layer, composited first;
    // the last element is the top. (ORA's stack.xml is the other way round —
    // `core::ora` reverses on the way in and out.)
    //
    // Any op that shifts layer indices clears the undo history, because
    // `UndoGroup::layer` is an index. Blunt, but it cannot restore pixels into
    // the wrong layer. A stable `LayerId` would fix it properly — noted for a
    // later pass.

    // ------------------------------------------------------------ folders --
    //
    // Folders are encoded *flat*: a folder header at depth d owns the
    // contiguous run of layers directly below it (lower indices) whose depth
    // is > d. Undo indices, the GPU tile cache and every existing iteration
    // keep working; only presentation (visibility/opacity) cascades, via
    // `effective_presentation`.

    /// The indices of `index`'s children (empty when it is not a folder or has
    /// none). Includes nested descendants — a folder inside a folder counts
    /// with everything in it.
    pub fn children_range(&self, index: usize) -> std::ops::Range<usize> {
        if index >= self.layers.len() || !self.layers[index].folder {
            return index..index;
        }
        let d = self.layers[index].depth;
        let mut s = index;
        while s > 0 && self.layers[s - 1].depth > d {
            s -= 1;
        }
        s..index
    }

    /// The block a structural op moves as one unit: the layer itself, plus its
    /// children when it is a folder. Always non-empty, ends at `index`
    /// inclusive.
    pub fn block_range(&self, index: usize) -> std::ops::Range<usize> {
        let c = self.children_range(index);
        c.start..index + 1
    }

    /// The innermost folder ENCLOSING `index` (None at the top level).
    /// Equal depth is not parenthood: two folders at the same depth may
    /// live in different parents, and combining them would land the
    /// result in one parent while silently emptying the other (audit H,
    /// 2026-08-19) — the combine paths compare these instead.
    pub fn enclosing_folder(&self, index: usize) -> Option<usize> {
        let d = self.layers.get(index)?.depth;
        ((index + 1)..self.layers.len()).find(|&i| {
            let l = &self.layers[i];
            l.folder && l.depth < d
        })
    }

    /// Per-layer visibility with every ancestor folder's eye folded in.
    /// Opacity does NOT cascade here: with true group isolation a folder's
    /// opacity is applied once, when its composited group blends onto the
    /// backdrop — the compositors read each layer's own opacity.
    pub fn effective_visibility(&self) -> Vec<bool> {
        let mut out: Vec<bool> = self.layers.iter().map(|l| l.visible).collect();
        for i in 0..self.layers.len() {
            if self.layers[i].folder && !self.layers[i].visible {
                for j in self.children_range(i) {
                    out[j] = false;
                }
            }
        }
        out
    }

    /// For each layer, the index of the layer its `clip` flag clips it to:
    /// the nearest layer below at the same depth that is not itself clipped
    /// and not a folder. `None` = not clipped (or no valid base — the flag is
    /// then ignored, CSP-style).
    pub fn clip_bases(&self) -> Vec<Option<usize>> {
        let n = self.layers.len();
        let mut out = vec![None; n];
        for i in 0..n {
            let l = &self.layers[i];
            if !l.clip || l.folder {
                continue;
            }
            let mut j = i;
            while j > 0 {
                j -= 1;
                let b = &self.layers[j];
                if b.depth != l.depth || b.folder {
                    break;
                }
                if !b.clip {
                    out[i] = Some(j);
                    break;
                }
            }
        }
        out
    }

    /// Repair the depth invariant after a structural edit: a layer can never
    /// be deeper than the layer above it allows (folder above → its depth + 1,
    /// plain layer above → its depth). `pub(crate)` for the ORA loader.
    pub(crate) fn normalize_depths(&mut self) {
        let mut allowed: u8 = 0;
        for i in (0..self.layers.len()).rev() {
            let l = &mut self.layers[i];
            if l.depth > allowed {
                l.depth = allowed;
            }
            allowed = l.depth + u8::from(l.folder);
        }
    }

    /// Re-derive a frame layer's raster from its vectors. A frame FOLDER gets
    /// border ink in `tiles` + the panel coverage in `mask_tiles` (true
    /// isolation); a flat frame layer keeps the round-7 white-gutter raster.
    pub(crate) fn derive_frame_raster(l: &mut Layer, size: (u32, u32)) {
        let LayerKind::Frame(fs) = &l.kind else {
            return;
        };
        let fs = fs.clone();
        if l.folder {
            l.replace_tiles(fs.rasterize_border(size));
            l.replace_mask_tiles(Some(fs.rasterize_mask(size)));
        } else {
            l.replace_tiles(fs.rasterize(size));
            l.replace_mask_tiles(None);
        }
    }

    /// Toggle a folder's expand state (presentation only, like rename).
    pub fn set_folder_open(&mut self, index: usize, open: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if !l.folder {
            return false;
        }
        l.open = open;
        self.touch();
        true
    }

    /// New empty folder directly above `index`'s block, same depth, active.
    /// Clears the undo history. Returns the new index.
    pub fn add_folder_above(&mut self, index: usize, name: impl Into<String>) -> usize {
        let depth = self.layers.get(index).map(|l| l.depth).unwrap_or(0);
        let at = (index + 1).min(self.layers.len());
        let mut f = Layer::new(name);
        f.folder = true;
        f.depth = depth;
        self.layers.insert(at, f);
        self.active = at;
        self.normalize_depths();
        self.clear_history();
        self.touch();
        at
    }

    /// New empty layer as the **topmost child** of the folder at `index`, and
    /// make it active. Clears the undo history. Returns the new index.
    pub fn add_layer_in_folder(&mut self, index: usize, name: impl Into<String>) -> Option<usize> {
        if !self.layers.get(index)?.folder {
            return None;
        }
        let mut l = Layer::new(name);
        l.depth = self.layers[index].depth + 1;
        self.layers.insert(index, l);
        self.active = index;
        self.clear_history();
        self.touch();
        Some(index)
    }

    /// CSP-style frame border folder: a folder header carrying the frame
    /// vectors (its derived raster — white gutter + borders — masks the
    /// children), with a shared-white "White" layer at the bottom so art below
    /// the folder never shows through the panels, and an empty draw layer
    /// which becomes active. Pushed at the top of the stack. Clears the undo
    /// history. Returns the header's index.
    pub fn add_frame_folder(&mut self, name: impl Into<String>, frames: FrameSet) -> usize {
        self.add_frame_folder_with(name, frames, true)
    }

    /// Same, with CSP's "Fill inside the frame" choice: `fill_white = false`
    /// skips the shared-white base layer (art below shows through the panel).
    pub fn add_frame_folder_with(
        &mut self,
        name: impl Into<String>,
        frames: FrameSet,
        fill_white: bool,
    ) -> usize {
        let mut draw = Layer::new("Layer 1");
        draw.depth = 1;
        let mut header = Layer::new(name);
        header.kind = LayerKind::Frame(frames);
        header.folder = true;
        Self::derive_frame_raster(&mut header, self.size);
        if fill_white {
            let mut white = Layer::new("White");
            white.depth = 1;
            white.fill_white(self.size);
            self.layers.push(white);
        }
        self.layers.push(draw);
        self.layers.push(header);
        // Draw layer active: the next pen stroke lands inside the folder.
        self.active = self.layers.len() - 2;
        self.clear_history();
        self.touch();
        self.layers.len() - 1
    }

    /// CSP "Divide frame folder": the folder at `index` keeps `keep` as its
    /// vectors, and a **new sibling frame folder** (with its own White + draw
    /// layer, like [`Self::add_frame_folder`]) is inserted directly below the
    /// original's block carrying `split_off`. Both rasters re-derive. Clears
    /// the undo history (structural). Returns the new header's index, with the
    /// new folder's draw layer active.
    pub fn divide_frame_folder(
        &mut self,
        index: usize,
        keep: FrameSet,
        split_off: FrameSet,
    ) -> Option<usize> {
        let size = self.size;
        let l = self.layers.get_mut(index)?;
        if !(l.folder && l.is_frame()) {
            return None;
        }
        let depth = l.depth;
        let LayerKind::Frame(cur) = &mut l.kind else {
            return None;
        };
        *cur = keep;
        Self::derive_frame_raster(l, size);

        let n = self.layers.iter().filter(|x| x.is_frame()).count() + 1;
        let at = self.children_range(index).start;
        let mut white = Layer::new("White");
        white.depth = depth + 1;
        white.fill_white(size);
        let mut draw = Layer::new("Layer 1");
        draw.depth = depth + 1;
        let mut header = Layer::new(format!("Frame {n}"));
        header.kind = LayerKind::Frame(split_off);
        header.folder = true;
        header.depth = depth;
        Self::derive_frame_raster(&mut header, size);
        // Children sit below their header in the flat encoding.
        self.layers.insert(at, header);
        self.layers.insert(at, draw);
        self.layers.insert(at, white);
        self.active = at + 1; // the new draw layer
        self.normalize_depths();
        self.clear_history();
        self.touch();
        Some(at + 2)
    }

    /// `FB-026` "Duplicate layer" — the other answer to *what happens to the
    /// art* when a panel with drawing in it is cut. Same structural move as
    /// [`Self::divide_frame_folder`], except the new folder gets a **copy of
    /// the original's contents** (the pixels ride along as `Arc` clones, so
    /// it is cheap until one of the two is painted on) instead of a fresh
    /// White + empty draw layer.
    ///
    /// Both halves then mask the same art to their own shape, which is the
    /// point: the artist cuts a drawn panel and keeps the drawing in both.
    /// Returns the new header's index; the copy's topmost child is active.
    pub fn divide_frame_folder_dup(
        &mut self,
        index: usize,
        keep: FrameSet,
        split_off: FrameSet,
    ) -> Option<usize> {
        let size = self.size;
        let l = self.layers.get(index)?;
        if !(l.folder && l.is_frame()) {
            return None;
        }
        let depth = l.depth;
        // Snapshot the contents BEFORE the header is rewritten.
        let mut block: Vec<Layer> = self.layers[self.children_range(index)].to_vec();
        if block.is_empty() {
            // Nothing to duplicate — the empty-folder answer IS the answer.
            return self.divide_frame_folder(index, keep, split_off);
        }
        for c in &mut block {
            // A clone must never inherit an open op's recording.
            c.recording = None;
        }
        let l = self.layers.get_mut(index)?;
        let LayerKind::Frame(cur) = &mut l.kind else {
            return None;
        };
        *cur = keep;
        Self::derive_frame_raster(l, size);

        let n = self.layers.iter().filter(|x| x.is_frame()).count() + 1;
        let at = self.children_range(index).start;
        let mut header = Layer::new(format!("Frame {n}"));
        header.kind = LayerKind::Frame(split_off);
        header.folder = true;
        header.depth = depth;
        Self::derive_frame_raster(&mut header, size);
        let k = block.len();
        self.layers.insert(at, header);
        // Children sit below their header, in their original order.
        for (i, c) in block.into_iter().enumerate() {
            self.layers.insert(at + i, c);
        }
        self.active = at + k - 1; // the copy's topmost child
        self.normalize_depths();
        self.clear_history();
        self.touch();
        Some(at + k)
    }

    /// Move a whole block (a layer, or a folder with everything in it) so its
    /// bottom lands at gap `slot` (an insertion point in the **current**
    /// stack, 0..=len), and give the moved layer depth `depth` (children keep
    /// their relative depths; the result is normalized). Refuses to drop a
    /// folder into itself. Clears the undo history.
    pub fn move_block_to_slot(&mut self, from: usize, slot: usize, depth: u8) -> bool {
        let n = self.layers.len();
        if from >= n || slot > n {
            return false;
        }
        let r = self.block_range(from);
        if slot > r.start && slot <= from {
            return false; // inside the moving block
        }
        let base = self.layers[from].depth;
        if (slot == r.start || slot == from + 1) && depth == base {
            return false; // dropped where it already sits
        }
        let active_offset = if self.active >= r.start && self.active <= from {
            Some(self.active - r.start)
        } else {
            None
        };
        let block: Vec<Layer> = self.layers.drain(r.clone()).collect();
        let k = block.len();
        let at = if slot > from { slot - k } else { slot };
        for (i, mut l) in block.into_iter().enumerate() {
            // Children shift with the header; saturate rather than wrap.
            let rel = l.depth.saturating_sub(base);
            l.depth = depth.saturating_add(rel);
            self.layers.insert(at + i, l);
        }
        self.active = match active_offset {
            Some(off) => at + off,
            None => {
                let a = self.active;
                if slot > from && a > from && a < slot {
                    a - k // was between the block and the gap; block hopped over it
                } else if slot <= r.start && a >= slot && a < r.start {
                    a + k // block landed under it
                } else {
                    a
                }
            }
        };
        self.normalize_depths();
        self.clear_history();
        self.touch();
        true
    }

    /// Insert a new empty layer directly above `index` (same depth — a
    /// sibling) and make it active. Returns the new layer's index. Clears the
    /// undo history.
    pub fn add_layer_above(&mut self, index: usize, name: impl Into<String>) -> usize {
        let at = (index + 1).min(self.layers.len());
        let mut l = Layer::new(name);
        l.depth = self.layers.get(index).map(|x| x.depth).unwrap_or(0);
        self.layers.insert(at, l);
        self.active = at;
        self.normalize_depths();
        self.clear_history();
        self.touch();
        at
    }

    /// Insert a new empty layer above the active one and make it active.
    pub fn add_layer(&mut self, name: impl Into<String>) -> usize {
        self.add_layer_above(self.active, name)
    }

    /// Remove a layer — a folder goes with everything inside it. Refuses to
    /// empty the document and refuses an out-of-range index; both return
    /// `false`. Clears the undo history.
    pub fn remove_layer(&mut self, index: usize) -> bool {
        if index >= self.layers.len() {
            return false;
        }
        let r = self.block_range(index);
        if r.len() >= self.layers.len() {
            return false;
        }
        self.layers.drain(r);
        if self.active >= self.layers.len() {
            self.active = self.layers.len() - 1;
        }
        self.normalize_depths();
        self.clear_history();
        self.touch();
        true
    }

    /// Copy a layer (pixels included — `Arc` clones, so it is cheap until one
    /// of the two is painted on) and insert the copy above it. A folder is
    /// copied with its children. Returns the new index of the copied layer.
    /// Clears the undo history.
    pub fn duplicate_layer(&mut self, index: usize) -> Option<usize> {
        if index >= self.layers.len() {
            return None;
        }
        let r = self.block_range(index);
        let mut block: Vec<Layer> = self.layers[r.clone()].to_vec();
        for l in &mut block {
            // A clone must never inherit an open op's recording.
            l.recording = None;
        }
        if let Some(top) = block.last_mut() {
            top.name = format!("{} copy", top.name);
        }
        let k = block.len();
        for (i, l) in block.into_iter().enumerate() {
            self.layers.insert(index + 1 + i, l);
        }
        let at = index + k;
        self.active = at;
        self.clear_history();
        self.touch();
        Some(at)
    }

    /// Move a layer to a new index (reorder), keeping its depth. `to` is the
    /// index it should end up at in the resulting stack. A folder moves with
    /// its children. Returns `false` on a bad index. Clears the undo history;
    /// keeps the moved layer active if it already was.
    pub fn move_layer(&mut self, from: usize, to: usize) -> bool {
        let n = self.layers.len();
        if from >= n || to >= n {
            return false;
        }
        if from == to {
            return true;
        }
        let depth = self.layers[from].depth;
        // Final-index semantics -> insertion gap in the current stack.
        let slot = if to > from { to + 1 } else { to };
        self.move_block_to_slot(from, slot, depth)
    }

    /// Move a layer one step up (towards the top of the stack).
    pub fn raise_layer(&mut self, index: usize) -> bool {
        index + 1 < self.layers.len() && self.move_layer(index, index + 1)
    }

    /// Move a layer one step down (towards the bottom).
    pub fn lower_layer(&mut self, index: usize) -> bool {
        index > 0 && self.move_layer(index, index - 1)
    }

    /// New layer above the active one, filled from an image, centred on the
    /// canvas (oversized images are clipped). Clears the undo history like any
    /// structural layer op. Returns the new layer's index.
    pub fn add_layer_from_image(
        &mut self,
        name: impl Into<String>,
        img: &image::RgbaImage,
    ) -> usize {
        let at = self.add_layer(name);
        let (w, h) = (self.size.0 as i64, self.size.1 as i64);
        let ox = (w - img.width() as i64) / 2;
        let oy = (h - img.height() as i64) / 2;
        let layer = &mut self.layers[at];
        for (px, py, p) in img.enumerate_pixels() {
            if p.0[3] == 0 {
                continue;
            }
            let (x, y) = (ox + px as i64, oy + py as i64);
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            let idx = TileIdx::of_pixel(x as i32, y as i32);
            let (tx, ty) = idx.origin();
            layer.tile_mut(idx).set_pixel(
                (x as i32 - tx) as usize,
                (y as i32 - ty) as usize,
                crate::blend::straight_u8_to_fix15(p.0),
            );
        }
        self.touch();
        at
    }

    /// IO-043: import with a selection active. The same layer
    /// [`Document::add_layer_from_image`] makes, plus the layer mask CSP
    /// builds for you — everything outside the selection is hidden, and
    /// nothing is destroyed doing it (delete the mask and the whole image
    /// is back). Returns the new layer's index and whether a mask was
    /// actually built, so the caller can say which of the two things it
    /// just did.
    ///
    /// **Why a mask and not a crop.** The gesture this serves is "select
    /// the panel, drop the photo in" — and the user is guessing at the
    /// crop. A mask is the guess he can take back; a crop is not, and the
    /// pixels outside a panel are exactly the ones he reaches for when the
    /// panel turns out to be the wrong size. This is also what CSP does,
    /// for the same reason.
    ///
    /// No selection ⇒ no mask, and emphatically not an all-hidden one:
    /// [`Document::mask_outside_selection`] masks EVERYTHING when
    /// `selection` is `None`, which as an import result would look exactly
    /// like a failed import.
    pub fn add_layer_from_image_masked(
        &mut self,
        name: impl Into<String>,
        img: &image::RgbaImage,
    ) -> (usize, bool) {
        let at = self.add_layer_from_image(name, img);
        let masked = self.selection.is_some() && self.mask_outside_selection(at);
        (at, masked)
    }

    /// New frame (koma) layer at the **top** of the stack — frames sit above
    /// the art they mask — rasterized from `frames` and made active. Clears the
    /// undo history like any structural layer op. Returns the new index.
    pub fn add_frame_layer(&mut self, name: impl Into<String>, frames: FrameSet) -> usize {
        let mut l = Layer::new(name);
        l.replace_tiles(frames.rasterize(self.size));
        l.kind = LayerKind::Frame(frames);
        self.layers.push(l);
        self.active = self.layers.len() - 1;
        self.clear_history();
        self.touch();
        self.active
    }

    /// Replace a frame layer's vector state, re-rasterize, and push a normal
    /// undo step (frame edits are undoable — they shift no layer indices).
    /// Returns `false` when `index` is not a frame layer.
    /// FB-035/036/038 (TRIAGE 141): combine two sibling frame folders.
    /// The UPPER folder's header survives; both folders' children pool
    /// under it; frames CONCAT (FB-036 "keep shapes", non-adjacent fine
    /// per FB-038) — or, with `merge_borders` and exactly one frame each
    /// sharing an edge, the two become ONE rect frame at the union bbox
    /// (FB-035 "combine border"; exact for the rect frames divides
    /// produce — a non-rect shape would need a real polygon union,
    /// recorded). Both frames' `slot` fields drop to None: the merged
    /// folder's reading order is decided by geometry alone (8.54).
    /// Returns the surviving header's index. Structural (clears history).
    pub fn combine_frame_folders(
        &mut self,
        a: usize,
        b: usize,
        merge_borders: bool,
    ) -> Option<usize> {
        let (ia, ib) = (a, b);
        if ia >= self.layers.len() || ib >= self.layers.len() || ia == ib {
            return None;
        }
        let (ha, hb) = (&self.layers[ia], &self.layers[ib]);
        if !(ha.folder && ha.is_frame() && hb.folder && hb.is_frame()) {
            return None;
        }
        if ha.depth != hb.depth {
            return None; // not siblings — nesting needs FB-037's own round
        }
        if self.enclosing_folder(ia) != self.enclosing_folder(ib) {
            return None; // same depth, DIFFERENT parents — not siblings
        }
        let mut set_a = ha.frames()?.clone();
        let set_b = hb.frames()?.clone();
        if merge_borders && set_a.frames.len() == 1 && set_b.frames.len() == 1 {
            let (ra, rb) = (set_a.frames[0].bbox(), set_b.frames[0].bbox());
            let tol = 2.0;
            let share_x = (ra[2] - rb[0]).abs() <= tol || (rb[2] - ra[0]).abs() <= tol;
            let share_y = (ra[3] - rb[1]).abs() <= tol || (rb[3] - ra[1]).abs() <= tol;
            let overlap_x = ra[0] < rb[2] - tol && rb[0] < ra[2] - tol;
            let overlap_y = ra[1] < rb[3] - tol && rb[1] < ra[3] - tol;
            if (share_x && overlap_y) || (share_y && overlap_x) {
                let u = [
                    ra[0].min(rb[0]),
                    ra[1].min(rb[1]),
                    ra[2].max(rb[2]),
                    ra[3].max(rb[3]),
                ];
                set_a.frames = vec![crate::frame::Frame::rect(u[0], u[1], u[2], u[3])];
            } else {
                set_a.frames.extend(set_b.frames);
            }
        } else {
            set_a.frames.extend(set_b.frames);
        }
        set_a.slot = None;
        let ba = self.block_range(ia);
        let bb = self.block_range(ib);
        // Children of both blocks, then the surviving header.
        let n = (ia - ba.start) + (ib - bb.start) + 1;
        let mut block: Vec<Layer> = self.layers[ba.start..ia].to_vec(); // A's children
        block.extend(self.layers[bb.start..ib].iter().cloned()); // B's children
        let mut header = self.layers[ia].clone();
        header.kind = LayerKind::Frame(set_a);
        Self::derive_frame_raster(&mut header, self.size);
        block.push(header);
        // Remove the HIGHER block first so the lower indices stay valid.
        let (lo, hi) = if ba.start < bb.start {
            (ba, bb)
        } else {
            (bb, ba)
        };
        self.layers.drain(hi.clone());
        self.layers.splice(lo.start..lo.end, block);
        self.normalize_depths();
        // Agree with group_frame_folders_common_parent: the new folder's
        // header is the selection after a combine (both used to differ —
        // audit H, 2026-08-19).
        self.active = lo.start + n - 1;
        self.clear_history();
        self.touch();
        Some(lo.start + n - 1)
    }

    /// FB-037 (TRIAGE 141): wrap two sibling frame folders in a NEW
    /// COMMON PARENT — a plain folder at their depth; both blocks move
    /// one level deeper, headers and shapes untouched ("originals
    /// survive"). The non-destructive combine. Returns the new header.
    /// Structural (clears history).
    ///
    /// Non-adjacent siblings are legal: the LOWER block is spliced up,
    /// directly under the higher block's start, before the folder is
    /// inserted — a folder's children must be contiguous above its
    /// header, and CSP's grouping does the same (the selection moves to
    /// the highest selected position; the intervening layers stay put,
    /// below both). Audit E, 2026-08-19: the old guard refused every
    /// separated pair as "not siblings" and its insert position would
    /// have left the lower block outside the parent.
    pub fn group_frame_folders_common_parent(&mut self, a: usize, b: usize) -> Option<usize> {
        let (ia, ib) = (a, b);
        if ia >= self.layers.len() || ib >= self.layers.len() || ia == ib {
            return None;
        }
        let (ha, hb) = (&self.layers[ia], &self.layers[ib]);
        if !(ha.folder && ha.is_frame() && hb.folder && hb.is_frame()) {
            return None;
        }
        if ha.depth != hb.depth {
            return None;
        }
        if self.enclosing_folder(ia) != self.enclosing_folder(ib) {
            return None; // same depth, DIFFERENT parents — not siblings
        }
        let ba = self.block_range(ia);
        let bb = self.block_range(ib);
        let (lo, hi) = if ba.start < bb.start {
            (ba, bb)
        } else {
            (bb, ba)
        };
        // Same-depth blocks cannot overlap; refuse anything that would
        // interleave rather than guess (an invariant breach, not a layout).
        if lo.end > hi.start {
            return None;
        }
        let parent_depth = ha.depth;
        // Splice the lower block up, adjacent to the higher block. When
        // the pair is already adjacent this is a no-op (insert_at ==
        // lo.start); otherwise the lower block crosses the intervening
        // layers, keeping its internal order.
        let moved: Vec<Layer> = self.layers.drain(lo.clone()).collect();
        let insert_at = hi.start - lo.len();
        self.layers.splice(insert_at..insert_at, moved);
        // One contiguous run now: deepen both blocks, parent below them.
        let run = insert_at..(insert_at + lo.len() + hi.len());
        for i in run.clone() {
            self.layers[i].depth += 1;
        }
        let mut header = Layer::new("Frames");
        header.folder = true;
        header.depth = parent_depth;
        // Children sit BELOW their header in the vec: the parent goes
        // AFTER the last layer of the second block.
        self.layers.insert(run.end, header);
        self.normalize_depths();
        self.active = run.end;
        self.clear_history();
        self.touch();
        Some(run.end)
    }

    /// SF-004/005: re-rasterize a generated effect-line layer from its
    /// stored spec (replace tiles wholesale). Returns false when the
    /// layer carries no spec or the render produced nothing.
    pub fn regen_genlines(&mut self, index: usize) -> bool {
        let Some(spec) = self.layers.get(index).and_then(|l| l.genlines) else {
            return false;
        };
        let tiles = spec.render(self.size);
        if tiles.is_empty() {
            return false;
        }
        // replace_tiles is a wholesale swap and cannot sit inside a tile
        // op (its debug_assert); the regen is therefore NOT undoable v1 —
        // same class as every layer-list change. Recorded. Older
        // pre-images for this layer would splice stale ink into the
        // regenerated raster when undone, so they go with the swap.
        self.history.drop_layer_history(index);
        self.layers[index].replace_tiles(tiles);
        self.touch();
        true
    }

    pub fn set_frames(&mut self, index: usize, frames: FrameSet) -> bool {
        let size = self.size;
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let LayerKind::Frame(cur) = &mut l.kind else {
            return false;
        };
        let before = cur.clone();
        *cur = frames;
        Self::derive_frame_raster(l, size);
        self.history.push_labeled(
            "Frame",
            UndoGroup::Frames {
                layer: index,
                frames: before,
            },
        );
        self.touch();
        true
    }

    /// Convert the layer at `index` into a screentone layer (or back): sets
    /// the screen params and drops the derived raster — the caller's next
    /// `refresh_derived` builds it. The painted pixels are the ink source and
    /// survive both directions (CSP トーンレイヤー, non-destructive). One undo
    /// step storing the previous params. Refuses vector layers and folders.
    pub fn set_tone(&mut self, index: usize, tone: Option<crate::tone::ToneParams>) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if l.folder || l.is_vector() {
            return false;
        }
        if l.tone == tone {
            return false;
        }
        let before = l.tone;
        l.tone = tone;
        l.tone_tiles = None;
        self.history.push_labeled(
            "Tone",
            UndoGroup::Tones {
                layer: index,
                tone: before,
            },
        );
        self.touch();
        true
    }

    /// LP-002/LP-003: turn the border effect on (or off) for the layer at
    /// `index`. Non-destructive in both directions like the tone conversion —
    /// the painted pixels are the source and survive — so this follows the
    /// same model: one undo step holding the previous params, the derived
    /// raster dropped for the next `refresh_derived` to rebuild.
    ///
    /// Refuses FOLDERS: a folder header composites an isolated group, and
    /// its own raster is only the frame border ink — there is no single
    /// alpha for an outline to grow from. Vector layers (text, balloons,
    /// frames) are allowed; a keyline round a balloon is exactly the want.
    pub fn set_edge(&mut self, index: usize, edge: Option<crate::edge::EdgeParams>) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if l.folder || l.edge == edge {
            return false;
        }
        let before = l.edge;
        l.edge = edge;
        l.edge_tiles = None;
        l.edge_stamp = None;
        self.history.push_labeled(
            "Border effect",
            UndoGroup::Edges {
                layer: index,
                edge: before,
            },
        );
        self.touch();
        true
    }

    /// Re-derive every tone layer's halftone raster at `dpi`. Cheap when
    /// nothing changed (per-tile revision compare). The app calls this before
    /// compositing — render, export, fill/wand/eyedropper sampling, save-side
    /// composites.
    pub fn refresh_derived(&mut self, dpi: u32) {
        let size = self.size;
        for l in &mut self.layers {
            if l.tone.is_some() {
                l.refresh_tone(dpi);
            }
            if matches!(l.kind, LayerKind::Fill(_)) {
                l.refresh_fill(dpi, size);
            }
            // LAST: the border effect grows around whatever the two above
            // produced, so it must see their output, not their input.
            if l.edge.is_some() || l.edge_tiles.is_some() {
                l.refresh_edge(size);
            }
        }
    }

    /// New balloon layer at the **top** of the stack — balloons sit above the
    /// art and the frames they annotate — rasterized from `balloons` and made
    /// active. Clears the undo history like any structural layer op.
    pub fn add_balloon_layer(&mut self, name: impl Into<String>, balloons: BalloonSet) -> usize {
        let mut l = Layer::new(name);
        l.replace_tiles(balloons.rasterize(self.size));
        l.kind = LayerKind::Balloon(balloons);
        self.layers.push(l);
        self.active = self.layers.len() - 1;
        self.clear_history();
        self.touch();
        self.active
    }

    /// Replace a balloon layer's vector state, re-rasterize, and push one
    /// undo step. Returns `false` when `index` is not a balloon layer.
    pub fn set_balloons(&mut self, index: usize, balloons: BalloonSet) -> bool {
        let size = self.size;
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let LayerKind::Balloon(cur) = &mut l.kind else {
            return false;
        };
        let before = cur.clone();
        let raster = balloons.rasterize(size);
        *cur = balloons;
        l.replace_tiles(raster);
        self.history.push_labeled(
            "Balloon",
            UndoGroup::Balloons {
                layer: index,
                balloons: before,
            },
        );
        self.touch();
        true
    }

    /// New text layer at the **top** of the stack (text sits above everything,
    /// balloons included), rasterized from `texts` and made active. Clears the
    /// undo history like any structural layer op.
    pub fn add_text_layer(&mut self, name: impl Into<String>, texts: TextSet) -> usize {
        let mut l = Layer::new(name);
        l.replace_tiles(texts.rasterize(self.size));
        l.kind = LayerKind::Text(texts);
        self.layers.push(l);
        self.active = self.layers.len() - 1;
        self.clear_history();
        self.touch();
        self.active
    }

    /// Replace a text layer's vector state, re-rasterize, and push one undo
    /// step. Returns `false` when `index` is not a text layer.
    /// Set a text layer's vector state with NO rasterize and NO undo —
    /// the Story Editor's non-active-page path (the doc re-encodes to
    /// bytes; its raster rebuilds when the page loads and warms).
    pub fn set_texts_raw(&mut self, index: usize, texts: TextSet) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let LayerKind::Text(cur) = &mut l.kind else {
            return false;
        };
        *cur = texts;
        self.touch();
        true
    }

    pub fn set_texts(&mut self, index: usize, texts: TextSet) -> bool {
        let size = self.size;
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let LayerKind::Text(cur) = &mut l.kind else {
            return false;
        };
        let before = cur.clone();
        let raster = texts.rasterize(size);
        *cur = texts;
        l.replace_tiles(raster);
        self.history.push_labeled(
            "Text",
            UndoGroup::Texts {
                layer: index,
                texts: before,
            },
        );
        self.touch();
        true
    }

    /// Fill missing sprite caches on a text layer in place — no history, no
    /// re-raster. An ORA-loaded text layer keeps its PNG pixels and has no
    /// caches; the app must warm them (via the text engine) before the first
    /// `set_texts`, or undoing that first edit would restore cache-less items
    /// that rasterize to nothing.
    pub fn warm_text_caches(
        &mut self,
        index: usize,
        mut shape: impl FnMut(&TextItem) -> Option<Arc<RenderedText>>,
    ) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let LayerKind::Text(cur) = &mut l.kind else {
            return false;
        };
        for item in &mut cur.texts {
            if item.cache.is_none() {
                item.cache = shape(item);
            }
        }
        true
    }

    /// Merge layer `index` down into `index - 1`, honouring the upper layer's
    /// blend mode and opacity (CSP "Merge with layer below"). A hidden upper
    /// layer contributes nothing and is simply removed. Clears the undo
    /// history like other structural ops. Returns `false` on a bad index.
    /// Vector layers (frames, balloons) refuse to merge — baking the derived
    /// raster into art would destroy both the vectors and the pixels under it.
    pub fn merge_down(&mut self, index: usize) -> bool {
        use crate::blend::{blend_premul, f32_to_fix15, fix15_to_f32, scale_opacity};
        if index == 0 || index >= self.layers.len() {
            return false;
        }
        if self.layers[index].is_vector() || self.layers[index - 1].is_vector() {
            return false;
        }
        // Folders never merge, merging across a folder boundary would smuggle
        // pixels in or out of a mask, a clipped layer's raw pixels are not
        // what it shows, and a locked layer refuses edits.
        if self.layers[index].folder
            || self.layers[index - 1].folder
            || self.layers[index].depth != self.layers[index - 1].depth
            || self.layers[index].clip
            || self.layers[index].lock
            || self.layers[index - 1].lock
        {
            return false;
        }
        let upper = self.layers[index].clone();
        if upper.visible {
            let lower = &mut self.layers[index - 1];
            for (idx, tile) in upper.tiles() {
                if tile.is_blank() {
                    continue;
                }
                let dst = lower.tile_mut(idx);
                let dd = dst.data_mut();
                let sd = tile.data();
                for p in 0..crate::tile::TILE_PIXELS {
                    let i = p * 4;
                    if sd[i + 3] == 0 && upper.blend == Blend::Normal {
                        continue;
                    }
                    let s = scale_opacity(
                        [
                            fix15_to_f32(sd[i]),
                            fix15_to_f32(sd[i + 1]),
                            fix15_to_f32(sd[i + 2]),
                            fix15_to_f32(sd[i + 3]),
                        ],
                        upper.opacity,
                    );
                    let d = [
                        fix15_to_f32(dd[i]),
                        fix15_to_f32(dd[i + 1]),
                        fix15_to_f32(dd[i + 2]),
                        fix15_to_f32(dd[i + 3]),
                    ];
                    let out = blend_premul(upper.blend, s, d);
                    for c in 0..4 {
                        dd[i + c] = f32_to_fix15(out[c]);
                    }
                }
            }
        }
        self.layers.remove(index);
        self.active = index - 1;
        self.clear_history();
        self.touch();
        true
    }

    pub fn rename_layer(&mut self, index: usize, name: impl Into<String>) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.name = name.into();
        self.touch();
        true
    }

    /// Select the active layer. Out-of-range indices are rejected.
    pub fn set_active(&mut self, index: usize) -> bool {
        if index >= self.layers.len() {
            return false;
        }
        self.active = index;
        self.touch();
        true
    }

    /// Set layer opacity (clamped 0..1) and publish a new document revision —
    /// the renderer cannot see this through tile revisions.
    pub fn set_layer_opacity(&mut self, index: usize, opacity: f32) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.opacity = opacity.clamp(0.0, 1.0);
        self.touch();
        true
    }

    pub fn set_layer_blend(&mut self, index: usize, blend: Blend) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.blend = blend;
        self.touch();
        true
    }

    /// LF-002: set a folder Through (children stop isolating — they blend
    /// against everything beneath, as if loose). Presentation-only like
    /// visibility: no undo, composites recompute. Non-folder layers refuse.
    pub fn set_folder_through(&mut self, index: usize, on: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if !l.folder {
            return false;
        }
        l.through = on;
        self.touch();
        true
    }

    pub fn set_layer_visible(&mut self, index: usize, visible: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.visible = visible;
        self.touch();
        true
    }

    /// The EYE solo (RF-001's hover promise, made real r113): hide every
    /// layer except `index`, returning the previous visibility vector for
    /// the restoring press. Presentation state like visibility itself —
    /// no undo.
    pub fn set_layer_visibility_solo(&mut self, index: usize) -> Option<Vec<bool>> {
        if self.layers.get(index).is_none() {
            return None;
        }
        let backup: Vec<bool> = self.layers.iter().map(|l| l.visible).collect();
        for (i, l) in self.layers.iter_mut().enumerate() {
            l.visible = i == index;
        }
        self.touch();
        Some(backup)
    }

    /// Restore a visibility snapshot (the solo's second press). A layer
    /// list that changed length in between simply truncates the restore.
    pub fn restore_visibility(&mut self, vis: &[bool]) {
        for (l, v) in self.layers.iter_mut().zip(vis) {
            l.visible = *v;
        }
        self.touch();
    }

    /// Is `index` the only visible layer? (The solo's second-press test.)
    pub fn only_visible(&self, index: usize) -> bool {
        self.layers
            .iter()
            .enumerate()
            .all(|(i, l)| !l.visible || i == index)
    }

    /// Set/clear the palette-colour label (presentation only, like rename).
    pub fn set_layer_label(&mut self, index: usize, label: Option<[u8; 3]>) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.label = label;
        self.touch();
        true
    }

    /// PC-002: the palette colour the layer palette should PAINT for this
    /// row — its own label, or, for a folder without one, the topmost label
    /// found inside it. A collapsed folder still says what is in it, which is
    /// the only reason to colour layers in the first place.
    ///
    /// THE RULE, stated: a folder's own label always wins (CSP skips the
    /// inheritance entirely when the folder has one). Otherwise scan the
    /// folder's contents from the top down and take the first label. Nested
    /// folders need no recursion — a nested folder without a label of its own
    /// is skipped and the scan carries straight on into its children, which is
    /// the answer recursion would give. An empty folder, or one whose contents
    /// are all unlabelled, keeps a bare strip (`None`) rather than inventing a
    /// colour. Non-folders are always their own label.
    ///
    /// `layers` is bottom-first and a folder header sits ABOVE its contents,
    /// so "inside" is the run of lower indices at a greater depth.
    pub fn palette_colour(&self, index: usize) -> Option<[u8; 3]> {
        let l = self.layers.get(index)?;
        if !l.folder || l.label.is_some() {
            return l.label;
        }
        (0..index)
            .rev()
            .take_while(|&j| self.layers[j].depth > l.depth)
            .find_map(|j| self.layers[j].label)
    }

    /// PA-001: show/hide the paper. VIEW STATE like a layer's eye — no undo
    /// and no effect on export; hiding it puts the transparency checker
    /// under the stack so holes in a flat fill stop being invisible.
    /// Returns false when nothing changed (the caller skips its redraw).
    pub fn set_paper_visible(&mut self, visible: bool) -> bool {
        if self.paper.visible == visible {
            return false;
        }
        self.paper.visible = visible;
        self.touch();
        true
    }

    /// PA-001: set the paper colour. UNDOABLE, unlike the eye above — this
    /// one changes what the page exports, so it is content.
    pub fn set_paper_colour(&mut self, colour: [u8; 3]) -> bool {
        if self.paper.colour == colour {
            return false;
        }
        self.history.push_labeled(
            "Paper colour",
            UndoGroup::Paper {
                colour: self.paper.colour,
            },
        );
        self.paper.colour = colour;
        self.touch();
        true
    }

    /// PA-001: what sits under the stack ON SCREEN. `Transparent` when the
    /// paper's eye is off — the viewer then shows the transparency checker
    /// through the holes, which is the whole point of the switch.
    pub fn paper_background(&self) -> crate::export::Background {
        if self.paper.visible {
            crate::export::Background::Solid(self.paper.colour)
        } else {
            crate::export::Background::Transparent
        }
    }

    /// PA-001: what sits under the stack ON EXPORT — the paper colour,
    /// **whatever the eye says**. Hiding the paper is a look-at-it check, not
    /// an export mode: a hidden paper must never be the reason a page ships
    /// with a transparent background. (An explicitly transparent PNG is still
    /// available: the caller passes [`Background::Transparent`] itself.)
    ///
    /// [`Background::Transparent`]: crate::export::Background::Transparent
    pub fn paper_export_background(&self) -> crate::export::Background {
        crate::export::Background::Solid(self.paper.colour)
    }

    /// LP-016: set the layer colour (display tint). Presentation-only like
    /// visibility: no undo, composites recompute.
    pub fn set_layer_colour(&mut self, index: usize, colour: Option<[u8; 3]>) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.layer_colour = colour;
        self.touch();
        true
    }

    /// LP-017: set the two-tone SUB colour (the white end). Same
    /// presentation-only contract as the main colour above.
    pub fn set_layer_sub_colour(&mut self, index: usize, colour: Option<[u8; 3]>) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.layer_sub_colour = colour;
        self.touch();
        true
    }

    /// LP-022: set the decrease-colour PREVIEW. Display only — no pixel
    /// changes, no undo step, and the export composite ignores it.
    pub fn set_layer_expression(&mut self, index: usize, e: LayerExpression) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if l.expression == e {
            return false;
        }
        l.expression = e;
        self.touch();
        true
    }

    /// Toggle clip-to-layer-below. Folders refuse (their group already
    /// isolates). Like visibility, not undoable.
    pub fn set_layer_clip(&mut self, index: usize, clip: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if l.folder {
            return false;
        }
        l.clip = clip;
        self.touch();
        true
    }

    /// Toggle the edit lock (the app's stroke/fill/clear paths check it).
    pub fn set_layer_lock(&mut self, index: usize, lock: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.lock = lock;
        self.touch();
        true
    }

    /// Toggle the transparent-pixel lock.
    pub fn set_layer_lock_alpha(&mut self, index: usize, lock: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.lock_alpha = lock;
        self.touch();
        true
    }

    /// The reference-layer SET (RF-001, owner spec 2026-08-17): any number
    /// of layers, marked independently. Stack order (bottom→top).
    pub fn reference_layers(&self) -> Vec<usize> {
        self.layers
            .iter()
            .enumerate()
            .filter(|(_, l)| l.reference)
            .map(|(i, _)| i)
            .collect()
    }

    /// The topmost reference layer, if any (compat for single-layer
    /// consumers; the SET is `reference_layers`).
    pub fn reference_layer_index(&self) -> Option<usize> {
        self.layers.iter().rposition(|l| l.reference)
    }

    /// Toggle ONE layer's reference flag, independently of every other
    /// (click). The owner REJECTED CSP's exclusivity: marking a sixth with
    /// five marked must not clear them. Like visibility, presentation-only
    /// and not undoable.
    pub fn set_layer_reference(&mut self, index: usize, on: bool) -> bool {
        if self.layers.get(index).is_none() {
            return false;
        }
        self.layers[index].reference = on;
        self.touch();
        true
    }

    /// SOLO: clear every other layer's flag and set this one (Alt+click).
    pub fn set_layer_reference_solo(&mut self, index: usize) -> bool {
        if self.layers.get(index).is_none() {
            return false;
        }
        for (i, l) in &mut self.layers.iter_mut().enumerate() {
            l.reference = i == index;
        }
        self.touch();
        true
    }

    /// Clear every reference flag (the owner's "clear all" command).
    pub fn clear_references(&mut self) {
        for l in &mut self.layers {
            l.reference = false;
        }
        self.touch();
    }

    /// Mark this layer as a draft (CSP 下書き): visible on screen, excluded
    /// from fill reference sampling and from export.
    pub fn set_layer_draft(&mut self, index: usize, on: bool) -> bool {
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        l.draft = on;
        self.touch();
        true
    }

    /// Per-layer draft state with ancestor folders folded in — a draft
    /// folder drafts everything inside it (mirrors
    /// [`Self::effective_visibility`]).
    pub fn effective_drafts(&self) -> Vec<bool> {
        let mut out: Vec<bool> = self.layers.iter().map(|l| l.draft).collect();
        for i in 0..self.layers.len() {
            if self.layers[i].folder && self.layers[i].draft {
                for j in self.children_range(i) {
                    out[j] = true;
                }
            }
        }
        out
    }

    /// Clamp the open op's changes back to the alpha the layer had before the
    /// op (transparent-pixel lock, CSP 透明ピクセルをロック): per pixel the
    /// alpha stays EXACTLY what it was; the colour takes the stroke as if it
    /// had painted a surface of that opacity. Recovering the stroke from the
    /// pre-image and the src-over result gives
    ///
    /// ```text
    /// sa    = (new.a − old.a) / (1 − old.a)      (0 when erasing)
    /// out.c = (1 − old.a) · old.c · (1 − sa) + old.a · new.c
    /// out.a = old.a
    /// ```
    ///
    /// Call ONCE, right before `end_op` — unlike the selection mask it is not
    /// idempotent.
    pub fn mask_op_to_alpha(&mut self) {
        use crate::blend::FIX15_ONE_F;
        let Some(li) = self.op_layer_index() else {
            return;
        };
        let layer = &mut self.layers[li];
        for idx in layer.recorded_tiles() {
            let pre = layer.recorded_pre_image(idx).cloned();
            match pre {
                // Tile did not exist: everything painted here lands on alpha
                // zero, so nothing sticks — drop the tile again.
                None => layer.set_tile(idx, None),
                Some(old) => {
                    let t = layer.tile_mut(idx);
                    let data = t.data_mut();
                    let od = old.data();
                    for p in 0..crate::tile::TILE_PIXELS {
                        let i = p * 4;
                        let m = od[i + 3] as f32 / FIX15_ONE_F;
                        if m >= 1.0 {
                            continue; // opaque: the stroke stands as painted
                        }
                        if m <= 0.0 {
                            for c in 0..4 {
                                data[i + c] = od[i + c];
                            }
                            continue;
                        }
                        let new_a = data[i + 3] as f32 / FIX15_ONE_F;
                        let sa = ((new_a - m) / (1.0 - m)).clamp(0.0, 1.0);
                        for c in 0..3 {
                            let oldv = od[i + c] as f32 / FIX15_ONE_F;
                            let newv = data[i + c] as f32 / FIX15_ONE_F;
                            let out = (1.0 - m) * oldv * (1.0 - sa) + m * newv;
                            data[i + c] = crate::blend::f32_to_fix15(out);
                        }
                        data[i + 3] = od[i + 3];
                    }
                }
            }
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new(DEFAULT_SIZE.0, DEFAULT_SIZE.1)
    }
}

// ---------------------------------------------------------- canvas resize --
//
// CSP's Edit ▸ "Change canvas size": content is NOT resampled, it is pinned
// to one of nine anchor points (基準位置) while the paper grows or shrinks
// around it. Crop is the same primitive with an explicit content offset.

/// Which corner/edge of the canvas the existing content stays pinned to when
/// the canvas resizes (CSP 基準位置, the 3×3 anchor grid).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ResizeAnchor {
    #[default]
    Center,
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl ResizeAnchor {
    /// Where the old canvas's (0, 0) lands in the new canvas.
    pub fn offsets(self, old: (u32, u32), new: (u32, u32)) -> (i32, i32) {
        use ResizeAnchor::*;
        let dx = match self {
            TopLeft | Left | BottomLeft => 0,
            TopRight | Right | BottomRight => new.0 as i32 - old.0 as i32,
            _ => ((new.0 as i64 - old.0 as i64) / 2) as i32,
        };
        let dy = match self {
            TopLeft | Top | TopRight => 0,
            BottomLeft | Bottom | BottomRight => new.1 as i32 - old.1 as i32,
            _ => ((new.1 as i64 - old.1 as i64) / 2) as i32,
        };
        (dx, dy)
    }
}

impl Document {
    /// Resize with an explicit content offset: after this, the pixel that was
    /// at (x, y) sits at (x + dx, y + dy) — Crop is `resize_to(w, h, -x0, -y0)`
    /// for the selection's bbox. Tiles that land fully outside the new canvas
    /// are dropped (the crop is destructive: undo cannot express it, so the
    /// history is cleared like every structural op). Vector layers translate
    /// and re-derive their rasters at the new size; a frame folder's White
    /// base re-extends to cover the grown canvas so panels placed in the new
    /// area still sit on paper.
    pub fn resize_to(&mut self, new_w: u32, new_h: u32, dx: i32, dy: i32) {
        let old = self.size;
        let new = (new_w.max(1), new_h.max(1));
        if new == old && dx == 0 && dy == 0 {
            return;
        }
        for l in &mut self.layers {
            // A canvas-filling uniform-white layer is a frame folder's White
            // base: keep it covering the whole (possibly grown) canvas.
            let was_paper = l.covers_canvas(old) && l.is_uniform_white();
            l.translate_content(dx, dy);
            if was_paper {
                l.extend_white(new);
            }
        }
        // Derived rasters rebuild at the new size (translate_content already
        // moved the vectors; its raster blit is discarded by the re-derive).
        for l in &mut self.layers {
            if l.is_frame() {
                Self::derive_frame_raster(l, new);
            } else if let Some(bs) = l.balloons().cloned() {
                let raster = bs.rasterize(new);
                l.replace_tiles(raster);
            } else if let Some(ts) = l.texts().cloned() {
                let raster = ts.rasterize(new);
                l.replace_tiles(raster);
            }
        }
        self.trim_outside(new);
        self.size = new;
        self.selection = None;
        self.clear_history();
        self.touch();
    }

    /// CSP "Change canvas size": pin content to `anchor` while the canvas
    /// becomes `new_w × new_h`.
    pub fn resize_canvas(&mut self, new_w: u32, new_h: u32, anchor: ResizeAnchor) {
        let (dx, dy) = anchor.offsets(self.size, (new_w, new_h));
        self.resize_to(new_w, new_h, dx, dy);
    }

    /// Drop every tile that lies entirely outside the canvas (crop residue).
    fn trim_outside(&mut self, size: (u32, u32)) {
        let ts = TILE_SIZE as i32;
        let (cw, ch) = (size.0 as i32, size.1 as i32);
        for l in &mut self.layers {
            l.tiles.retain(|ti, _| {
                let (ox, oy) = ti.origin();
                ox < cw && oy < ch && ox + ts > 0 && oy + ts > 0
            });
        }
    }

    /// Guard for the paint ops below: the active layer must accept pixels.
    fn paint_guard(&self) -> bool {
        let l = self.active_layer();
        l.paintable() && !l.lock
    }

    /// Src-over one premultiplied fix15 pixel onto `dst`.
    fn over_pixel(dst: &mut [u16], s: [u16; 4]) {
        let a = s[3] as u32;
        let inv = 32768 - a;
        for k in 0..3 {
            let d = dst[k] as u32;
            dst[k] = ((s[k] as u32).saturating_add((d * inv + 16384) >> 15)).min(32768) as u16;
        }
        let d = dst[3] as u32;
        dst[3] = (a.saturating_add((d * inv + 16384) >> 15)).min(32768) as u16;
    }

    /// Paint a linear gradient on the active layer (the CSP Gradient tool's
    /// core op): colours interpolate along `a`→`b` (canvas px, straight
    /// colour RGBA, premultiplied here); perpendicular strips are constant.
    /// Selection-clipped, one undo step. Returns false on a refusing layer.
    ///
    /// The two-colour form — every ramp option at its default. Anything the
    /// Tool Property panel authored goes through [`Self::paint_gradient_ramp`].
    pub fn paint_gradient(
        &mut self,
        a: [f32; 2],
        b: [f32; 2],
        from: [f32; 4],
        to: [f32; 4],
    ) -> bool {
        self.paint_gradient_ramp(a, b, &crate::gradient::Ramp::two(from, to))
    }

    /// The authored-ramp form: interior colour stops, edge process, flip,
    /// dithering, centre-out, mixing mode and mixing rate all apply. Pixels
    /// the ramp declines to draw ("do not draw" outside the dragged span)
    /// are left byte-untouched, not painted transparent.
    pub fn paint_gradient_ramp(
        &mut self,
        a: [f32; 2],
        b: [f32; 2],
        ramp: &crate::gradient::Ramp,
    ) -> bool {
        if !self.paint_guard() {
            return false;
        }
        let ab = [b[0] - a[0], b[1] - a[1]];
        let ab2 = ab[0] * ab[0] + ab[1] * ab[1];
        if ab2 < 1e-6 {
            return false;
        }
        // The gradient spans infinitely ACROSS `a→b` (CSP behaviour: the ramp
        // band extends over the whole canvas perpendicular to the drag).
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let sel = self.selection.clone();
        let li = self.active;
        let lock_alpha = self.layers[li].lock_alpha;
        self.begin_op();
        for ty in 0..(h + TILE_SIZE as i32 - 1) / TILE_SIZE as i32 {
            for tx in 0..(w + TILE_SIZE as i32 - 1) / TILE_SIZE as i32 {
                let idx = TileIdx::new(tx, ty);
                if let Some(s) = &sel {
                    if s.tile_mask(idx).is_none() {
                        continue;
                    }
                }
                let (ox, oy) = idx.origin();
                // The projection is affine, so the tile's four corners bound
                // it: a "do not draw" ramp can reject the whole tile here,
                // BEFORE `tile_mut` allocates it and stashes an undo
                // pre-image for a tile it was never going to touch.
                let proj = |px: f32, py: f32| ((px - a[0]) * ab[0] + (py - a[1]) * ab[1]) / ab2;
                let (tx1, ty1) = ((ox + TILE_SIZE as i32) as f32, (oy + TILE_SIZE as i32) as f32);
                let us = [
                    proj(ox as f32, oy as f32),
                    proj(tx1, oy as f32),
                    proj(ox as f32, ty1),
                    proj(tx1, ty1),
                ];
                let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                for u in us {
                    lo = lo.min(u);
                    hi = hi.max(u);
                }
                if !ramp.draws_span(lo, hi) {
                    continue;
                }
                let tile = self.layers[li].tile_mut(idx);
                let data = tile.data_mut();
                for p in 0..TILE_SIZE * TILE_SIZE {
                    let x = ox + (p % TILE_SIZE) as i32;
                    let y = oy + (p / TILE_SIZE) as i32;
                    if x >= w || y >= h {
                        continue;
                    }
                    let px = [x as f32 + 0.5, y as f32 + 0.5];
                    let u = proj(px[0], px[1]);
                    let Some(s) = ramp.eval(u, x, y) else {
                        continue; // edge process "do not draw"
                    };
                    // PREMULTIPLY. This op used to write the straight colour
                    // beside a faded alpha, which is not a valid fix15 pixel:
                    // a mid-grey fading to transparent came out brightening
                    // toward white, and the LIVE gradient layer (which does
                    // premultiply, `fill_layer::build_fill_tile`) disagreed
                    // with the destructive tool on the same parameters.
                    let al = crate::blend::f32_to_fix15(s[3]);
                    let pr = |v: f32| crate::blend::f32_to_fix15(v * s[3]).min(al);
                    let c = [pr(s[0]), pr(s[1]), pr(s[2]), al];
                    Self::over_pixel(&mut data[p * 4..p * 4 + 4], c);
                }
            }
        }
        self.mask_op_to_selection();
        if lock_alpha {
            self.mask_op_to_alpha();
        }
        self.end_op();
        true
    }

    /// Fill a closed polygon on the active layer (even-odd scanline, canvas
    /// px, anti-aliasing by 2x2 subsampling), src-over. Used by the Figure
    /// tool's "fill shape" option. One undo step; false on a refusing layer.
    pub fn fill_polygon(&mut self, pts: &[[f32; 2]], color: [f32; 3], alpha: f32) -> bool {
        if pts.len() < 3 || !self.paint_guard() {
            return false;
        }
        let mut bb = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for p in pts {
            bb[0] = bb[0].min(p[0]);
            bb[1] = bb[1].min(p[1]);
            bb[2] = bb[2].max(p[0]);
            bb[3] = bb[3].max(p[1]);
        }
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let inside = |x: f32, y: f32| -> bool {
            // Even-odd crossing test.
            let mut inside = false;
            let n = pts.len();
            let mut j = n - 1;
            for i in 0..n {
                let pi = pts[i];
                let pj = pts[j];
                if (pi[1] > y) != (pj[1] > y) {
                    let xint = pj[0] + (y - pj[1]) * (pi[0] - pj[0]) / (pi[1] - pj[1]);
                    if x < xint {
                        inside = !inside;
                    }
                }
                j = i;
            }
            inside
        };
        let sel = self.selection.clone();
        let li = self.active;
        let lock_alpha = self.layers[li].lock_alpha;
        let base = [
            crate::blend::f32_to_fix15(color[0]),
            crate::blend::f32_to_fix15(color[1]),
            crate::blend::f32_to_fix15(color[2]),
            crate::blend::f32_to_fix15(alpha),
        ];
        self.begin_op();
        let t0x = (bb[0].floor().max(0.0) as i32 / TILE_SIZE as i32).max(0);
        let t0y = (bb[1].floor().max(0.0) as i32 / TILE_SIZE as i32).max(0);
        let t1x = (((bb[2].ceil().min(w as f32) as i32) + TILE_SIZE as i32 - 1) / TILE_SIZE as i32)
            .min((w + TILE_SIZE as i32 - 1) / TILE_SIZE as i32);
        let t1y = (((bb[3].ceil().min(h as f32) as i32) + TILE_SIZE as i32 - 1) / TILE_SIZE as i32)
            .min((h + TILE_SIZE as i32 - 1) / TILE_SIZE as i32);
        for ty in t0y..t1y {
            for tx in t0x..t1x {
                let idx = TileIdx::new(tx, ty);
                if let Some(s) = &sel {
                    if s.tile_mask(idx).is_none() {
                        continue;
                    }
                }
                let (ox, oy) = idx.origin();
                let tile = self.layers[li].tile_mut(idx);
                let data = tile.data_mut();
                for p in 0..TILE_SIZE * TILE_SIZE {
                    let x = ox + (p % TILE_SIZE) as i32;
                    let y = oy + (p / TILE_SIZE) as i32;
                    if x >= w || y >= h {
                        continue;
                    }
                    let fx = x as f32;
                    let fy = y as f32;
                    if fx + 1.0 < bb[0] || fx > bb[2] || fy + 1.0 < bb[1] || fy > bb[3] {
                        continue;
                    }
                    let cov = [
                        inside(fx + 0.25, fy + 0.25),
                        inside(fx + 0.75, fy + 0.25),
                        inside(fx + 0.25, fy + 0.75),
                        inside(fx + 0.75, fy + 0.75),
                    ]
                    .iter()
                    .filter(|&&c| c)
                    .count() as f32
                        / 4.0;
                    if cov <= 0.0 {
                        continue;
                    }
                    let mut c = base;
                    c[3] = crate::blend::f32_to_fix15(alpha * cov);
                    c[0] = (c[0] as f32 * cov) as u16;
                    c[1] = (c[1] as f32 * cov) as u16;
                    c[2] = (c[2] as f32 * cov) as u16;
                    Self::over_pixel(&mut data[p * 4..p * 4 + 4], c);
                }
            }
        }
        self.mask_op_to_selection();
        if lock_alpha {
            self.mask_op_to_alpha();
        }
        self.end_op();
        true
    }
}

impl Layer {
    /// Does this layer's tile footprint cover the whole given canvas?
    fn covers_canvas(&self, size: (u32, u32)) -> bool {
        self.tiles.len() >= tile_count_for(size)
            && self.tile_bounds().is_some_and(|(x, y, w, h)| {
                x <= 0 && y <= 0 && x + w as i32 >= size.0 as i32 && y + h as i32 >= size.1 as i32
            })
    }

    /// Is every pixel opaque white — the White base layer's invariant
    /// (`fill_white` writes exactly this)? Early-exits on the first
    /// non-white pixel, so a normal art layer costs one tile.
    fn is_uniform_white(&self) -> bool {
        let w = crate::tile::FIX15_ONE as u16;
        self.tiles
            .values()
            .all(|t| t.data().iter().all(|&c| c == w))
    }

    /// Insert full-white tiles (sharing one Arc) wherever the given canvas
    /// extent is not covered yet. Used after growing the canvas so the White
    /// base still spans the page; painted (un-shared) tiles stay as they are.
    fn extend_white(&mut self, size: (u32, u32)) {
        let mut t = Tile::new_transparent();
        t.data_mut().fill(crate::tile::FIX15_ONE as u16);
        let white = Arc::new(t);
        for ti in tile_range(size) {
            self.tiles.entry(ti).or_insert_with(|| white.clone());
        }
    }
}

/// Every tile index spanning a canvas of `size` pixels.
fn tile_range(size: (u32, u32)) -> impl Iterator<Item = TileIdx> {
    let tx = (size.0 as usize).div_ceil(TILE_SIZE) as i32;
    let ty = (size.1 as usize).div_ceil(TILE_SIZE) as i32;
    (0..ty).flat_map(move |y| (0..tx).map(move |x| TileIdx::new(x, y)))
}

/// Number of tiles spanning a canvas of `size` pixels.
fn tile_count_for(size: (u32, u32)) -> usize {
    let tx = (size.0 as usize).div_ceil(TILE_SIZE);
    let ty = (size.1 as usize).div_ceil(TILE_SIZE);
    tx * ty
}

/// IO-043 — import with a selection active builds the layer mask.
#[cfg(test)]
mod import_mask_tests {
    use super::*;
    use crate::export::{Background, composite};

    /// A 128×128 red image, which centres exactly over a 128×128 canvas.
    fn red_page() -> image::RgbaImage {
        image::RgbaImage::from_pixel(128, 128, image::Rgba([255, 0, 0, 255]))
    }

    #[test]
    fn import_with_no_selection_makes_a_plain_unmasked_layer() {
        let mut doc = Document::new(128, 128);
        let (at, masked) = doc.add_layer_from_image_masked("ref", &red_page());
        assert!(!masked, "no selection, nothing to mask against");
        assert!(
            doc.layers[at].mask.is_none(),
            "an all-hidden mask here would look exactly like a failed import"
        );
        let img = composite(&doc, Background::Transparent);
        assert_eq!(img.get_pixel(100, 5).0[3], 255, "the whole image is there");
    }

    #[test]
    fn import_with_a_selection_hides_everything_outside_it() {
        let mut doc = Document::new(128, 128);
        // The left half only.
        doc.selection = Some(crate::selection::Selection::from_rect(
            &doc, 0.0, 0.0, 64.0, 128.0,
        ));
        let (at, masked) = doc.add_layer_from_image_masked("ref", &red_page());
        assert!(masked, "a selection was active, so the mask is the point");
        assert!(doc.layers[at].mask.is_some());

        let img = composite(&doc, Background::Transparent);
        assert_eq!(img.get_pixel(5, 5).0[3], 255, "inside the selection: kept");
        assert_eq!(img.get_pixel(100, 5).0[3], 0, "outside: hidden");
    }

    /// The mask hides; it does not destroy. Deleting it brings the whole
    /// import back — which is the entire argument for a mask over a crop.
    #[test]
    fn the_mask_is_reversible_and_the_pixels_survive() {
        let mut doc = Document::new(128, 128);
        doc.selection = Some(crate::selection::Selection::from_rect(
            &doc, 0.0, 0.0, 64.0, 128.0,
        ));
        let (at, _) = doc.add_layer_from_image_masked("ref", &red_page());
        assert!(doc.mask_delete(at));
        let img = composite(&doc, Background::Transparent);
        assert_eq!(
            img.get_pixel(100, 5).0[3],
            255,
            "the pixels outside the selection were never thrown away"
        );
    }
}

#[cfg(test)]
mod mask_tests {
    use super::*;
    use crate::export::{Background, composite};

    /// LM-002/007/003: the mask scales the layer's contribution in the
    /// CPU composite — outside-selection hides, the toggle restores,
    /// delete removes, clear empties.
    #[test]
    fn mask_hides_in_composite_and_toggles() {
        let mut doc = Document::new(128, 128);
        doc.begin_op();
        // BOTH tiles the probes land in — (5,5) and (100,5) straddle the
        // x=64 selection edge, so paint tiles (0,0) and (1,0).
        for idx in [TileIdx::new(0, 0), TileIdx::new(1, 0)] {
            let t = doc.layers[0].tile_mut(idx);
            for p in 0..crate::tile::TILE_PIXELS {
                t.set_pixel(p % 64, p / 64, [32768, 0, 0, 32768]);
            }
        }
        doc.end_op();
        let full = composite(&doc, Background::Transparent);
        assert_eq!(full.get_pixel(5, 5).0[3], 255, "unmasked: opaque");

        // Mask outside the LEFT half-selection → the right half hides.
        doc.selection = Some(crate::selection::Selection::from_rect(
            &doc, 0.0, 0.0, 64.0, 128.0,
        ));
        assert!(doc.mask_outside_selection(0));
        let masked = composite(&doc, Background::Transparent);
        assert_eq!(
            masked.get_pixel(5, 5).0[3],
            255,
            "inside the selection: kept"
        );
        assert_eq!(masked.get_pixel(100, 5).0[3], 0, "outside: hidden");

        // LM-007: disable → everything shows again.
        assert!(doc.mask_set_enabled(0, false));
        let off = composite(&doc, Background::Transparent);
        assert_eq!(off.get_pixel(100, 5).0[3], 255, "mask off: full layer");

        // LM-003: clear (all hidden) vs delete (mask gone).
        assert!(doc.mask_set_enabled(0, true));
        assert!(doc.mask_clear(0));
        let cleared = composite(&doc, Background::Transparent);
        assert_eq!(cleared.get_pixel(5, 5).0[3], 0, "cleared: all hidden");
        assert!(doc.layers[0].mask.is_some(), "the mask itself kept");
        assert!(doc.mask_delete(0));
        let deleted = composite(&doc, Background::Transparent);
        assert_eq!(deleted.get_pixel(5, 5).0[3], 255, "deleted: unmasked again");
    }

    /// LM-006: baking multiplies the layer by coverage and removes the
    /// mask — one undo op that restores BOTH the pixels and the mask
    /// (undo restores pixels; the mask returns via the op's... check:
    /// mask_delete runs OUTSIDE the op, so undo restores pixels only.
    /// The test pins the pixels + the mask's absence.)
    #[test]
    fn bake_multiplies_and_removes_mask() {
        let mut doc = Document::new(128, 128);
        doc.begin_op();
        for idx in [TileIdx::new(0, 0), TileIdx::new(1, 0)] {
            let t = doc.layers[0].tile_mut(idx);
            for p in 0..crate::tile::TILE_PIXELS {
                t.set_pixel(p % 64, p / 64, [32768, 0, 0, 32768]);
            }
        }
        doc.end_op();
        doc.selection = Some(crate::selection::Selection::from_rect(
            &doc, 0.0, 0.0, 64.0, 128.0,
        ));
        assert!(doc.mask_outside_selection(0));

        assert!(doc.mask_apply_bake(0));
        assert!(doc.layers[0].mask.is_none(), "the mask is gone");
        // The layer pixels are now half-opacity on the right (masked) side.
        let alpha = |doc: &Document, x: i32| {
            let idx = TileIdx::of_pixel(x, 5);
            doc.layers[0]
                .tile(idx)
                .map(|t| t.pixel((x - idx.origin().0) as usize, (5 - idx.origin().1) as usize)[3])
                .unwrap_or(0)
        };
        assert_eq!(alpha(&doc, 5), 32768, "inside: untouched");
        assert_eq!(alpha(&doc, 100), 0, "outside: baked away");
        // TWO undo steps now (the round-68 MaskField group fixed the old
        // deviation): the first restores the MASK, the second the pixels.
        assert!(doc.undo());
        assert!(doc.layers[0].mask.is_some(), "the mask returns first");
        assert!(doc.undo());
        assert_eq!(alpha(&doc, 100), 32768, "then the pixels");
    }

    /// The MaskField group: mask create/toggle/clear/delete and STROKES
    /// all undo — and redo — through whole-field snapshots.
    #[test]
    fn maskfield_group_makes_mask_ops_undoable() {
        let mut doc = Document::new(128, 128);
        doc.begin_op();
        doc.layers[0]
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(5, 5, [32768, 0, 0, 32768]);
        doc.end_op();
        doc.selection = Some(crate::selection::Selection::from_rect(
            &doc, 0.0, 0.0, 64.0, 128.0,
        ));
        assert!(doc.mask_outside_selection(0));
        assert_eq!(doc.undo_len(), 2, "layer op + mask creation");
        assert!(doc.undo(), "undo creation");
        assert!(doc.layers[0].mask.is_none(), "creation undone");
        assert!(doc.redo());
        assert!(doc.layers[0].mask.is_some(), "creation redone");

        // Toggle: undo restores the previous enabled state.
        assert!(doc.mask_set_enabled(0, false));
        assert!(doc.undo());
        assert!(doc.layers[0].mask.as_ref().unwrap().enabled);
        // Delete: undo brings the mask back.
        assert!(doc.mask_delete(0));
        assert!(doc.layers[0].mask.is_none());
        assert!(doc.undo());
        assert!(doc.layers[0].mask.is_some(), "delete undone");
    }

    /// LM-001: the starter mask is all-visible.    /// LM-001: the starter mask is all-visible.
    #[test]
    fn starter_mask_is_all_visible() {
        let mut doc = Document::new(128, 128);
        doc.begin_op();
        let t = doc.layers[0].tile_mut(TileIdx::new(0, 0));
        t.set_pixel(5, 5, [0, 0, 32768, 32768]);
        doc.end_op();
        assert!(doc.mask_selection_blank(0));
        let img = composite(&doc, Background::Transparent);
        assert_eq!(img.get_pixel(5, 5).0[3], 255, "blank mask hides nothing");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::FIX15_ONE;

    /// Every mode survives a save/load, and no two share a name.
    #[test]
    fn every_blend_mode_round_trips_through_its_ora_name() {
        let mut seen = std::collections::HashSet::new();
        for b in Blend::ALL {
            let n = b.ora_name();
            assert!(seen.insert(n), "{n} is claimed by two modes ({b:?})");
            assert_eq!(Blend::from_ora_name(n), b, "{b:?} did not survive {n}");
            assert!(
                n.starts_with("svg:") || n.starts_with("mn:"),
                "{b:?} name {n} has no namespace"
            );
        }
        assert_eq!(seen.len(), Blend::ALL.len());
    }

    /// **Old files must load pixel-identically.** These fifteen names were
    /// written into every .ora the owner saved before the part-3 modes
    /// existed; they are a file format, not an implementation detail. A new
    /// variant may append a name, never move one of these.
    #[test]
    fn the_pre_part3_ora_names_are_frozen() {
        for (b, n) in [
            (Blend::Normal, "svg:src-over"),
            (Blend::Multiply, "svg:multiply"),
            (Blend::Screen, "svg:screen"),
            (Blend::Darken, "svg:darken"),
            (Blend::Lighten, "svg:lighten"),
            (Blend::Add, "mn:add"),
            (Blend::Subtract, "mn:subtract"),
            (Blend::Overlay, "svg:overlay"),
            (Blend::SoftLight, "svg:soft-light"),
            (Blend::HardLight, "svg:hard-light"),
            (Blend::Difference, "svg:difference"),
            (Blend::Exclusion, "svg:exclusion"),
            (Blend::Hue, "svg:hue"),
            (Blend::Saturation, "svg:saturation"),
            (Blend::Color, "svg:color"),
        ] {
            assert_eq!(b.ora_name(), n, "{b:?} renamed — old files would shift");
            assert_eq!(Blend::from_ora_name(n), b, "{n} no longer loads as {b:?}");
        }
        // And the default is still Normal: an .ora with no composite-op, or
        // one we do not know, must not land on a part-3 mode.
        assert_eq!(Blend::default(), Blend::Normal);
        assert_eq!(Blend::from_ora_name("svg:plus-lighter"), Blend::Normal);
        assert_eq!(Blend::from_ora_name(""), Blend::Normal);
    }

    #[test]
    fn gradient_paints_the_axis_and_clamps_outside() {
        let mut doc = Document::new(64, 8);
        // Ramp from black to white along x = 8..56.
        assert!(doc.paint_gradient(
            [8.0, 4.0],
            [56.0, 4.0],
            [0.0, 0.0, 0.0, 1.0],
            [1.0, 1.0, 1.0, 1.0]
        ));
        let px = |x: i32| -> u16 {
            let ti = TileIdx::of_pixel(x, 4);
            doc.layers[0]
                .tile(ti)
                .map(|t| t.pixel((x - ti.origin().0) as usize, 4)[0])
                .unwrap_or(0)
        };
        let alpha = |x: i32| -> u16 {
            let ti = TileIdx::of_pixel(x, 4);
            doc.layers[0]
                .tile(ti)
                .map(|t| t.pixel((x - ti.origin().0) as usize, 4)[3])
                .unwrap_or(0)
        };
        let one = FIX15_ONE as u16;
        assert_eq!(px(0), 0, "before the ramp: clamped to FROM (black)");
        assert_eq!(alpha(0), one, "clamped pixels are still opaque");
        assert_eq!(px(63), one, "beyond the ramp: clamped to TO (white)");
        assert_eq!(alpha(63), one);
        let near0 = px(9) as f32 / one as f32;
        let mid = px(32) as f32 / one as f32;
        let near1 = px(55) as f32 / one as f32;
        assert!(near0 < 0.2, "start dark ({near0})");
        assert!((mid - 0.5).abs() < 0.08, "midpoint half ({mid})");
        assert!(near1 > 0.8, "end bright ({near1})");
    }

    #[test]
    fn gradient_alpha_fades_to_transparent() {
        let mut doc = Document::new(32, 4);
        assert!(doc.paint_gradient(
            [0.0, 2.0],
            [31.0, 2.0],
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0, 0.0]
        ));
        let a = |x: i32| -> u16 {
            let ti = TileIdx::of_pixel(x, 2);
            doc.layers[0]
                .tile(ti)
                .map(|t| t.pixel((x - ti.origin().0) as usize, 2)[3])
                .unwrap_or(0)
        };
        let one = FIX15_ONE as u16;
        assert!(a(1) as f32 / (one as f32) > 0.85, "opaque start");
        assert!(a(30) as f32 / (one as f32) < 0.15, "transparent end");
    }

    #[test]
    fn fill_polygon_covers_interior_not_exterior() {
        let mut doc = Document::new(64, 64);
        // A diamond around (32,32).
        let pts = [[32.0, 12.0], [52.0, 32.0], [32.0, 52.0], [12.0, 32.0]];
        assert!(doc.fill_polygon(&pts, [1.0, 0.0, 0.0], 1.0));
        let a = |x: i32, y: i32| -> u16 {
            let ti = TileIdx::of_pixel(x, y);
            doc.layers[0]
                .tile(ti)
                .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize)[3])
                .unwrap_or(0)
        };
        assert_eq!(a(32, 32), FIX15_ONE as u16, "centre filled");
        assert!(a(32, 30) > FIX15_ONE as u16 / 2, "inside mostly filled");
        assert_eq!(a(2, 2), 0, "far outside untouched");
        assert_eq!(a(2, 32), 0, "left of the diamond untouched");
    }

    /// `G-004`. Repeat tiles the ramp outside the drag; Reverse ping-pongs;
    /// "do not draw" leaves the outside byte-untouched — and does not even
    /// ALLOCATE those tiles, which is what keeps the undo step small.
    #[test]
    fn gradient_edge_process_repeats_and_declines() {
        use crate::gradient::{EdgeProcess, Ramp};
        let val = |doc: &Document, x: i32| -> Option<u16> {
            let ti = TileIdx::of_pixel(x, 2);
            doc.layers[0]
                .tile(ti)
                .map(|t| t.pixel((x - ti.origin().0) as usize, 2)[0])
        };

        // Ramp black→white across x 0..16 of a 64-wide canvas.
        let mut rep = Document::new(64, 4);
        let mut ramp = Ramp::two([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]);
        ramp.opts.edge = EdgeProcess::Repeat;
        assert!(rep.paint_gradient_ramp([0.0, 2.0], [16.0, 2.0], &ramp));
        // x = 4 is a quarter in; x = 20 and x = 36 are the same quarter of
        // the next tiles. Within a rounding step of each other.
        let a = val(&rep, 4).unwrap() as i32;
        let b = val(&rep, 20).unwrap() as i32;
        let c = val(&rep, 36).unwrap() as i32;
        assert!((a - b).abs() < 64 && (a - c).abs() < 64, "{a} {b} {c}");
        assert!(a < 12000, "and it is genuinely the dark quarter: {a}");

        let mut rev = Document::new(64, 4);
        ramp.opts.edge = EdgeProcess::Reverse;
        assert!(rev.paint_gradient_ramp([0.0, 2.0], [16.0, 2.0], &ramp));
        // Ping-pong about the drag's END (x = 16), and pixels are sampled at
        // their CENTRES: 20.5 folds to 11.5, so x = 20 mirrors x = 11 — not
        // x = 12, which is the off-by-one this assertion is here to pin.
        let m = val(&rev, 20).unwrap() as i32;
        let mirror = val(&rev, 11).unwrap() as i32;
        assert!((m - mirror).abs() < 64, "reverse mirrors: {m} vs {mirror}");
        assert_ne!(
            m,
            val(&rev, 12).unwrap() as i32,
            "and the fold really is half a pixel off the integer grid"
        );

        let mut blank = Document::new(256, 4);
        blank.layers[0].tile_mut(TileIdx::of_pixel(200, 2)); // pre-existing
        let before = blank.layers[0].tile_count();
        ramp.opts.edge = EdgeProcess::Blank;
        assert!(blank.paint_gradient_ramp([0.0, 2.0], [16.0, 2.0], &ramp));
        assert_eq!(
            val(&blank, 200),
            Some(0),
            "outside the drag: not drawn, not cleared"
        );
        assert!(val(&blank, 8).is_some_and(|v| v > 0), "inside still paints");
        assert!(
            blank.layers[0].tile_count() < before + 3,
            "a declined tile must not be allocated (undo pays for those)"
        );
    }

    /// The destructive tool writes PREMULTIPLIED pixels, like every other
    /// paint op — and therefore agrees with the LIVE gradient layer built
    /// from the same parameters. Before this, a mid-grey fading to
    /// transparent was written straight beside a faded alpha, which reads
    /// back as an over-bright colour.
    #[test]
    fn gradient_premultiplies_and_matches_the_live_layer() {
        use crate::fill_layer::FillKind;
        let grey = [0.5, 0.5, 0.5, 1.0];
        let clear = [0.5, 0.5, 0.5, 0.0];

        let mut baked = Document::new(64, 4);
        assert!(baked.paint_gradient([0.0, 2.0], [63.0, 2.0], grey, clear));

        let mut live = Document::new(64, 4);
        let li = live.add_fill_layer(
            FillKind::Gradient {
                a: [0.0, 2.0],
                b: [63.0, 2.0],
                from: grey,
                to: clear,
                mid: Default::default(),
                opts: Default::default(),
            },
            false,
        );
        live.refresh_derived(600);

        let ti = TileIdx::new(0, 0);
        let bt = baked.layers[0].tile(ti).expect("the ramp painted");
        let lt = live.layers[li].display_tile(ti).expect("the fill derived");
        for x in [1usize, 8, 16, 31, 47, 62] {
            let p = bt.pixel(x, 2);
            let q = lt.pixel(x, 2);
            for k in 0..3 {
                assert!(
                    p[k] <= p[3],
                    "x={x}: premultiplied means colour <= alpha, got {p:?}"
                );
                // Both paths quantize once; a rounding LSB apart is fine.
                assert!(
                    (p[k] as i32 - q[k] as i32).abs() <= 2,
                    "x={x} ch{k}: baked {p:?} vs live {q:?}"
                );
            }
            assert!((p[3] as i32 - q[3] as i32).abs() <= 2, "{p:?} {q:?}");
        }
    }

    #[test]
    fn gradient_is_one_undo_step() {
        let mut doc = Document::new(32, 4);
        assert!(doc.paint_gradient(
            [0.0, 2.0],
            [31.0, 2.0],
            [1.0, 1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 1.0]
        ));
        assert!(doc.undo());
        // Everything back to transparent.
        let ti = TileIdx::of_pixel(16, 2);
        let a = doc.layers[0]
            .tile(ti)
            .map(|t| t.pixel((16 - ti.origin().0) as usize, 2)[3])
            .unwrap_or(0);
        assert_eq!(a, 0, "undo restores the layer");
    }

    /// LM-009: a LINKED mask (the default) rides `translate_content`,
    /// sub-tile accurate; an UNLINKED mask stays put — the art slides
    /// underneath a fixed window.
    #[test]
    fn mask_link_rides_translate_and_unlink_stays() {
        let mut doc = Document::new(256, 256);
        // A mask with one full-coverage tile at (1,1) and a hole at local
        // (0,0) so the shift is observable per-pixel.
        let mut m = crate::doc::LayerMask {
            enabled: true,
            revision: crate::tile::next_revision(),
            tiles: std::collections::HashMap::new(),
        };
        let mut t = Tile::new_transparent();
        for y in 0..crate::tile::TILE_SIZE {
            for x in 0..crate::tile::TILE_SIZE {
                if !(x < 8 && y < 8) {
                    t.set_pixel(x, y, [32768, 32768, 32768, 32768]);
                }
            }
        }
        m.tiles.insert(TileIdx::new(1, 1), std::sync::Arc::new(t));
        doc.layers[0].mask = Some(m);

        let cov = |d: &Document, x: i32, y: i32| -> u16 {
            let ti = TileIdx::of_pixel(x, y);
            d.layers[0]
                .mask
                .as_ref()
                .and_then(|m| m.tiles.get(&ti))
                .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize)[3])
                .unwrap_or(0)
        };
        assert_eq!(cov(&doc, 64, 64), 0, "the hole is at the tile corner");
        assert_eq!(cov(&doc, 80, 80), 32768);

        // Linked (default): +70,+30 sub-tile — the hole moves with it.
        doc.layers[0].translate_content(70, 30);
        assert_eq!(cov(&doc, 134, 94), 0, "the hole rode the shift");
        assert_eq!(cov(&doc, 150, 110), 32768, "coverage rode the shift");
        assert_eq!(cov(&doc, 64, 64), 0, "the vacated corner is empty");

        // Unlinked: an equal back-shift moves nothing about the mask.
        doc.layers[0].mask_linked = false;
        doc.layers[0].translate_content(-70, -30);
        assert_eq!(cov(&doc, 134, 94), 0, "unlinked: the mask stayed");
        assert_eq!(
            cov(&doc, 64, 64),
            0,
            "unlinked: untouched (still the shifted state)"
        );
    }

    /// The unlinked flag round-trips through ORA (absent = linked, so old
    /// files load linked — CSP's default).
    #[test]
    fn mask_link_flag_round_trips() {
        let mut doc = Document::new(128, 128);
        let mut m = crate::doc::LayerMask::default();
        let mut t = Tile::new_transparent();
        t.set_pixel(0, 0, [32768, 32768, 32768, 32768]);
        m.tiles.insert(TileIdx::new(0, 0), std::sync::Arc::new(t));
        doc.layers[0].mask = Some(m);
        doc.layers[0].mask_linked = false;
        let mut buf = std::io::Cursor::new(Vec::new());
        crate::ora::save_to(&doc, &mut buf).unwrap();
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(buf.get_ref().clone())).unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut z.by_name("stack.xml").unwrap(), &mut s).unwrap();
        assert!(s.contains("mnc-mask-unlinked=\"1\""), "{s}");
        let reloaded = crate::ora::load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        assert!(!reloaded.layers[0].mask_linked, "unlinked survived");
        // A plain save/load without the attr stays linked.
        let mut b2 = std::io::Cursor::new(Vec::new());
        crate::ora::save_to(&Document::new(128, 128), &mut b2).unwrap();
        let r2 = crate::ora::load_from(std::io::Cursor::new(b2.into_inner())).unwrap();
        assert!(r2.layers[0].mask_linked, "absent attr = linked");
    }

    #[test]
    fn translate_content_shifts_pixels_sub_tile_accurately() {
        let mut doc = Document::new(256, 256);
        let alpha = |d: &Document, x: i32, y: i32| -> u16 {
            let ti = TileIdx::of_pixel(x, y);
            d.layers[0]
                .tile(ti)
                .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize)[3])
                .unwrap_or(0)
        };
        for y in 10..20 {
            for x in 10..20 {
                let ti = TileIdx::of_pixel(x, y);
                let (ox, oy) = ti.origin();
                doc.layers[0].tile_mut(ti).set_pixel(
                    (x - ox) as usize,
                    (y - oy) as usize,
                    [1, 2, 3, FIX15_ONE as u16],
                );
            }
        }
        doc.layers[0].translate_content(37, -5);
        assert_eq!(alpha(&doc, 47, 5), FIX15_ONE as u16, "landed at +37,-5");
        assert_eq!(alpha(&doc, 56, 14), FIX15_ONE as u16, "far corner too");
        assert_eq!(alpha(&doc, 10, 10), 0, "source vacated");
        assert_eq!(alpha(&doc, 46, 5), 0, "no smear left of the box");
    }

    #[test]
    fn translate_content_handles_negative_offsets() {
        let mut doc = Document::new(256, 256);
        let ti = TileIdx::of_pixel(3, 3);
        let (ox, oy) = ti.origin();
        doc.layers[0].tile_mut(ti).set_pixel(
            (3 - ox) as usize,
            (3 - oy) as usize,
            [0, 0, 0, FIX15_ONE as u16],
        );
        doc.layers[0].translate_content(-8, 0);
        let t2 = TileIdx::of_pixel(-5, 3);
        let got = doc.layers[0]
            .tile(t2)
            .map(|t| t.pixel((-5 - t2.origin().0) as usize, (3 - t2.origin().1) as usize));
        assert_eq!(
            got.unwrap()[3],
            FIX15_ONE as u16,
            "off-canvas tile holds it"
        );
    }

    #[test]
    fn default_document_is_2048_square_with_one_layer() {
        let d = Document::default();
        assert_eq!(d.size, (2048, 2048));
        assert_eq!(d.layers.len(), 1);
        assert_eq!(d.active, 0);
        assert_eq!(d.tile_extent(), (32, 32));
        assert!(d.active_layer().is_empty());
    }

    #[test]
    fn tile_mut_creates_and_bumps_revision() {
        let mut d = Document::default();
        let idx = TileIdx::new(1, 2);
        assert!(d.active_layer().tile(idx).is_none());

        let r0 = {
            let t = d.active_layer_mut().tile_mut(idx);
            t.set_pixel(3, 4, [0, 0, 0, FIX15_ONE as u16]);
            t.revision()
        };
        assert_eq!(d.active_layer().tile_count(), 1);
        assert_eq!(d.active_layer().tile(idx).unwrap().pixel(3, 4)[3], 32768);

        let r1 = d.active_layer_mut().tile_mut(idx).revision();
        assert!(r1 > r0, "second write must publish a newer revision");
    }

    #[test]
    fn write_is_copy_on_write_against_snapshots() {
        let mut layer = Layer::new("L");
        let idx = TileIdx::new(0, 0);
        layer.tile_mut(idx).set_pixel(0, 0, [0, 0, 0, 32768]);

        // Undo would keep exactly this handle.
        let snapshot = layer.tile_arc(idx).unwrap().clone();
        assert_eq!(Arc::strong_count(&snapshot), 2);

        layer.tile_mut(idx).set_pixel(0, 0, [0, 0, 0, 0]);

        assert_eq!(
            snapshot.pixel(0, 0)[3],
            32768,
            "snapshot must not be mutated"
        );
        assert_eq!(layer.tile(idx).unwrap().pixel(0, 0)[3], 0);
        assert_eq!(
            Arc::strong_count(&snapshot),
            1,
            "make_mut must have unshared"
        );
    }

    /// Paint one pixel through the normal write path (what a brush does).
    fn paint(doc: &mut Document, idx: TileIdx, v: u16) {
        doc.active_layer_mut()
            .tile_mut(idx)
            .set_pixel(0, 0, [v, v, v, v]);
    }

    fn px(doc: &Document, layer: usize, idx: TileIdx) -> Option<[u16; 4]> {
        doc.layers[layer].tile(idx).map(|t| t.pixel(0, 0))
    }

    #[test]
    fn undo_redo_roundtrip_restores_pixel_state() {
        let mut doc = Document::default();
        let a = TileIdx::new(1, 1);
        let b = TileIdx::new(2, 1);

        // Stroke 1 creates a tile that did not exist (pre-image = None).
        doc.begin_op();
        paint(&mut doc, a, 100);
        assert!(doc.end_op());
        assert_eq!(px(&doc, 0, a).unwrap()[3], 100);

        // Stroke 2 overwrites `a` and creates `b`.
        doc.begin_op();
        paint(&mut doc, a, 200);
        paint(&mut doc, b, 200);
        assert!(doc.end_op());
        assert_eq!(doc.undo_len(), 2);

        assert!(doc.undo());
        assert_eq!(px(&doc, 0, a).unwrap()[3], 100, "tile restored to stroke 1");
        assert!(
            px(&doc, 0, b).is_none(),
            "tile that did not exist is removed"
        );

        assert!(doc.undo());
        assert!(px(&doc, 0, a).is_none(), "back to a blank layer");
        assert!(!doc.can_undo());
        assert!(!doc.undo());

        assert!(doc.redo());
        assert_eq!(px(&doc, 0, a).unwrap()[3], 100);
        assert!(doc.redo());
        assert_eq!(px(&doc, 0, a).unwrap()[3], 200);
        assert_eq!(px(&doc, 0, b).unwrap()[3], 200);
        assert!(!doc.redo());
    }

    #[test]
    fn restored_tiles_carry_fresh_revisions() {
        let mut doc = Document::default();
        let a = TileIdx::new(0, 0);
        doc.begin_op();
        paint(&mut doc, a, 100);
        doc.end_op();
        let before = doc.max_revision();

        doc.begin_op();
        paint(&mut doc, a, 200);
        doc.end_op();
        doc.undo();

        assert_eq!(px(&doc, 0, a).unwrap()[3], 100);
        assert!(
            doc.max_revision() > before,
            "an undone tile must look new to the GPU cache"
        );
    }

    #[test]
    fn new_op_clears_the_redo_stack() {
        let mut doc = Document::default();
        let a = TileIdx::new(0, 0);
        doc.begin_op();
        paint(&mut doc, a, 10);
        doc.end_op();
        doc.undo();
        assert!(doc.can_redo());

        doc.begin_op();
        paint(&mut doc, a, 20);
        doc.end_op();
        assert!(!doc.can_redo(), "a new op forks the history");
    }

    #[test]
    fn empty_op_pushes_nothing_and_undo_stack_is_capped() {
        let mut doc = Document::default();
        doc.begin_op();
        assert!(!doc.end_op());
        assert!(!doc.can_undo());

        for i in 0..(crate::undo::UNDO_LIMIT + 25) {
            doc.begin_op();
            paint(&mut doc, TileIdx::new(0, 0), (i % 100) as u16 + 1);
            doc.end_op();
        }
        assert_eq!(doc.undo_len(), crate::undo::UNDO_LIMIT);
    }

    /// The depth is a preference now (`prefs.txt` `undo_depth=`), not a
    /// constant: the cap follows whatever the document was told, and
    /// LOWERING it frees the memory at once rather than on the next stroke.
    #[test]
    fn undo_depth_is_settable_and_lowering_it_trims_now() {
        let mut doc = Document::default();
        assert_eq!(doc.undo_limit(), crate::undo::UNDO_LIMIT, "default = today");

        doc.set_undo_limit(5);
        for i in 0..12 {
            doc.begin_op();
            paint(&mut doc, TileIdx::new(0, 0), i + 1);
            doc.end_op();
        }
        assert_eq!(doc.undo_len(), 5);

        // Raising it keeps what is there and simply allows more.
        doc.set_undo_limit(50);
        assert_eq!(doc.undo_len(), 5);
        for i in 0..20 {
            doc.begin_op();
            paint(&mut doc, TileIdx::new(0, 0), i + 100);
            doc.end_op();
        }
        assert_eq!(doc.undo_len(), 25);

        // Lowering trims immediately, oldest first, labels in lockstep.
        doc.set_undo_limit(3);
        assert_eq!(doc.undo_len(), 3);
        assert_eq!(doc.undo_labels().len(), 3);

        // A cleared history keeps the depth it was given.
        doc.clear_history();
        assert_eq!(doc.undo_limit(), 3);
    }

    #[test]
    fn writes_outside_an_op_are_not_undoable() {
        let mut doc = Document::default();
        paint(&mut doc, TileIdx::new(0, 0), 42);
        assert!(!doc.can_undo());
    }

    #[test]
    fn layer_ops() {
        let mut doc = Document::default();
        assert!(!doc.remove_layer(0), "the last layer cannot be removed");

        let i = doc.add_layer("Ink");
        assert_eq!((i, doc.active, doc.layers.len()), (1, 1, 2));
        assert_eq!(doc.layers[1].name, "Ink");

        paint(&mut doc, TileIdx::new(0, 0), 500);
        let dup = doc.duplicate_layer(1).unwrap();
        assert_eq!(dup, 2);
        assert_eq!(doc.layers[2].name, "Ink copy");
        assert_eq!(px(&doc, 2, TileIdx::new(0, 0)).unwrap()[3], 500);

        // Painting the copy must not touch the original (Arc COW).
        paint(&mut doc, TileIdx::new(0, 0), 900);
        assert_eq!(px(&doc, 1, TileIdx::new(0, 0)).unwrap()[3], 500);

        assert!(doc.move_layer(2, 0));
        assert_eq!(doc.layers[0].name, "Ink copy");
        assert_eq!(doc.active, 0, "the moved layer stays active");
        assert_eq!(doc.layers[1].name, "Layer 1");

        assert!(doc.rename_layer(1, "Paper"));
        assert_eq!(doc.layers[1].name, "Paper");
        assert!(!doc.rename_layer(9, "nope"));

        assert!(doc.set_active(2));
        assert!(!doc.set_active(3));

        let r = doc.revision;
        assert!(doc.set_layer_opacity(0, 2.0));
        assert_eq!(doc.layers[0].opacity, 1.0, "opacity is clamped");
        assert!(doc.set_layer_opacity(0, 0.25));
        assert!(doc.set_layer_blend(0, Blend::Multiply));
        assert!(doc.set_layer_visible(0, false));
        assert!(doc.revision > r, "presentation changes bump the revision");

        assert!(doc.remove_layer(0));
        assert_eq!(doc.layers.len(), 2);
        assert!(doc.active < doc.layers.len());
    }

    #[test]
    fn text_edits_are_undoable_and_warm_fills_caches() {
        use crate::text::{RenderedText, TextItem, TextSet};
        let sprite = |v: u8| {
            Arc::new(RenderedText {
                origin: [10, 10],
                size: [8, 8],
                rgba: (0..8 * 8).flat_map(|_| [v, v, v, 255]).collect(),
            })
        };
        let mut item = TextItem::new([10.0, 10.0], "Meiryo".into(), 12.0, [0, 0, 0], false);
        item.insert(0, "a");
        item.cache = Some(sprite(0));
        let one = TextSet { texts: vec![item] };

        let mut doc = Document::default();
        let li = doc.add_text_layer("Text 1", one.clone());
        assert!(doc.layers[li].is_text() && doc.layers[li].is_vector());
        assert!(doc.layers[li].tile_count() > 0, "sprite rasterized");

        let mut moved = one.clone();
        moved.texts[0].cache = Some(sprite(0));
        moved.texts[0].translate(30.0, 0.0);
        assert!(doc.set_texts(li, moved.clone()));
        assert_eq!(doc.layers[li].texts().unwrap(), &moved);
        assert!(doc.can_undo());
        doc.undo();
        assert_eq!(doc.layers[li].texts().unwrap(), &one, "vectors restored");
        doc.redo();
        assert_eq!(doc.layers[li].texts().unwrap(), &moved, "redo re-moves");
        assert!(
            !doc.set_texts(0, one.clone()),
            "raster layers have no texts"
        );

        // Warm: strip the cache (as an ORA load would) and refill it.
        let mut bare = moved.clone();
        bare.texts[0].cache = None;
        let LayerKind::Text(cur) = &mut doc.layers[li].kind else {
            unreachable!()
        };
        *cur = bare;
        assert!(doc.warm_text_caches(li, |_| Some(sprite(7))));
        assert!(doc.layers[li].texts().unwrap().texts[0].cache.is_some());
        // Already-cached items are left alone.
        assert!(doc.warm_text_caches(li, |_| panic!("must not re-shape")));
    }

    #[test]
    fn structural_layer_ops_clear_the_history() {
        // UndoGroup::layer is an index; reordering would aim it at the wrong
        // layer, so the history is dropped instead.
        let mut doc = Document::default();
        doc.begin_op();
        paint(&mut doc, TileIdx::new(0, 0), 10);
        doc.end_op();
        assert!(doc.can_undo());
        doc.add_layer("2");
        assert!(!doc.can_undo());
    }

    #[test]
    fn tile_bounds_are_tile_aligned() {
        let mut layer = Layer::new("L");
        assert!(layer.tile_bounds().is_none());
        layer.tile_mut(TileIdx::new(1, 2));
        layer.tile_mut(TileIdx::new(3, 2));
        assert_eq!(layer.tile_bounds(), Some((64, 128, 192, 64)));
    }

    #[test]
    fn frame_edits_are_undoable_and_rerasterize() {
        use crate::frame::FrameSet;
        let mut doc = Document::new(256, 256);
        let one_frame = FrameSet::single_rect([64.0, 64.0, 192.0, 192.0], 4.0);
        let li = doc.add_frame_layer("Frame 1", one_frame.clone());
        assert!(doc.layers[li].is_frame());
        assert!(!doc.can_undo(), "adding the layer is structural");
        let raster_before = doc.layers[li].tile_count();
        assert!(raster_before > 0);

        // Divide the panel — one undoable step.
        let mut divided = one_frame.clone();
        let (a, b) = divided.frames[0]
            .split([128.0, 0.0], [128.0, 256.0], 8.0)
            .unwrap();
        divided.frames = vec![a, b];
        assert!(doc.set_frames(li, divided.clone()));
        assert_eq!(doc.layers[li].frames().unwrap().frames.len(), 2);

        assert!(doc.undo());
        assert_eq!(
            doc.layers[li].frames().unwrap(),
            &one_frame,
            "vectors restored"
        );
        assert!(doc.redo());
        assert_eq!(
            doc.layers[li].frames().unwrap(),
            &divided,
            "redo re-divides"
        );

        // Guards: no painting semantics change, but merge refuses frames and
        // set_frames refuses raster layers.
        assert!(!doc.merge_down(li), "frame layers never merge");
        assert!(
            !doc.set_frames(0, one_frame),
            "raster layers have no frames"
        );
    }

    #[test]
    fn balloon_edits_are_undoable_and_rerasterize() {
        use crate::balloon::{Balloon, BalloonSet, BalloonShape};
        let mut doc = Document::new(256, 256);
        let one = BalloonSet {
            balloons: vec![Balloon {
                shape: BalloonShape::Ellipse {
                    center: [128.0, 128.0],
                    radii: [60.0, 40.0],
                },
                tails: Vec::new(),

                ..Default::default()
            }],
            border_px: 4.0,
            pressure_width: false,
        };
        let li = doc.add_balloon_layer("Balloon 1", one.clone());
        assert!(doc.layers[li].is_balloon() && doc.layers[li].is_vector());
        assert!(doc.layers[li].tile_count() > 0);

        let mut moved = one.clone();
        moved.balloons[0].translate(30.0, 0.0);
        assert!(doc.set_balloons(li, moved.clone()));
        assert_eq!(doc.layers[li].balloons().unwrap(), &moved);

        assert!(doc.undo());
        assert_eq!(doc.layers[li].balloons().unwrap(), &one, "vectors restored");
        assert!(doc.redo());
        assert_eq!(doc.layers[li].balloons().unwrap(), &moved, "redo re-moves");

        assert!(!doc.merge_down(li), "balloon layers never merge");
        assert!(!doc.set_balloons(0, one), "raster layers have no balloons");
    }

    #[test]
    fn frame_folder_structure_and_white_fill_share_one_tile() {
        let mut doc = Document::new(256, 256);
        let fs = FrameSet::single_rect([32.0, 32.0, 224.0, 224.0], 4.0);
        let hi = doc.add_frame_folder("Frame 1", fs.clone());
        // Stack bottom→top: Layer 1(0), White(1), Layer 1(1), Frame 1 header.
        assert_eq!(doc.layers.len(), 4);
        assert_eq!(hi, 3);
        assert!(doc.layers[3].folder && doc.layers[3].is_frame());
        assert_eq!(doc.layers[3].depth, 0);
        assert_eq!(doc.layers[1].name, "White");
        assert_eq!(doc.layers[1].depth, 1);
        assert_eq!(doc.layers[2].depth, 1);
        assert_eq!(doc.active, 2, "the draw layer is active");
        assert_eq!(doc.children_range(3), 1..3);
        assert_eq!(doc.block_range(3), 1..4);

        // The white fill really is one shared allocation.
        let whites: Vec<_> = doc.layers[1].tiles().map(|(_, a)| Arc::as_ptr(a)).collect();
        assert_eq!(whites.len(), 16);
        assert!(
            whites.iter().all(|p| *p == whites[0]),
            "all tiles share one Arc"
        );
        assert_eq!(
            doc.layers[1].tile(TileIdx::new(0, 0)).unwrap().pixel(5, 5),
            [FIX15_ONE as u16; 4]
        );

        // Painting on the header or merging through the mask is refused.
        assert!(!doc.layers[3].paintable());
        assert!(doc.layers[2].paintable());
        assert!(!doc.merge_down(3), "folder never merges");
        assert!(!doc.merge_down(1), "White sits over a depth boundary");
    }

    #[test]
    fn frame_folder_without_fill_has_no_white_child() {
        let mut doc = Document::new(128, 128);
        let fs = FrameSet::single_rect([16.0, 16.0, 112.0, 112.0], 2.0);
        let hi = doc.add_frame_folder_with("Frame 1", fs, false);
        assert_eq!(doc.children_range(hi).len(), 1, "just the draw layer");
        assert!(doc.layers.iter().all(|l| l.name != "White"));
    }

    #[test]
    fn divide_frame_folder_spawns_a_sibling_folder_with_children() {
        let mut doc = Document::new(256, 256);
        let fs = FrameSet::single_rect([32.0, 32.0, 224.0, 224.0], 4.0);
        let hi = doc.add_frame_folder("Frame 1", fs.clone());
        // Split the panel in half by hand and hand the pieces over.
        let keep = FrameSet::single_rect([32.0, 32.0, 224.0, 120.0], 4.0);
        let off = FrameSet::single_rect([32.0, 136.0, 224.0, 224.0], 4.0);
        let new_hi = doc
            .divide_frame_folder(hi, keep.clone(), off.clone())
            .unwrap();

        // Stack bottom→top: Layer 1, [White, Layer 1, Frame 2], [White, Layer 1, Frame 1].
        assert_eq!(doc.layers.len(), 7);
        assert_eq!(new_hi, 3);
        let new = &doc.layers[new_hi];
        assert!(new.folder && new.is_frame());
        assert_eq!(new.frames().unwrap().frames, off.frames);
        assert_eq!(doc.children_range(new_hi), 1..3);
        assert_eq!(doc.layers[1].name, "White");
        assert_eq!(doc.active, 2, "new folder's draw layer active");
        // The original folder kept its piece and its own children.
        let orig = doc
            .layers
            .iter()
            .rposition(|l| l.name == "Frame 1")
            .unwrap();
        assert_eq!(doc.layers[orig].frames().unwrap().frames, keep.frames);
        assert_eq!(doc.children_range(orig), 4..6);
        // Both folders re-derived isolation masks.
        assert!(doc.layers[orig].mask_tiles().is_some());
        assert!(doc.layers[new_hi].mask_tiles().is_some());

        // A flat (non-folder) frame layer refuses.
        let mut flat = Document::new(64, 64);
        let fi = flat.add_frame_layer("F", FrameSet::single_rect([8.0, 8.0, 56.0, 56.0], 2.0));
        assert!(
            flat.divide_frame_folder(
                fi,
                FrameSet::single_rect([8.0, 8.0, 56.0, 30.0], 2.0),
                FrameSet::single_rect([8.0, 34.0, 56.0, 56.0], 2.0)
            )
            .is_none()
        );
    }

    #[test]
    fn folder_visibility_cascades_and_clip_bases_resolve() {
        let mut doc = Document::new(128, 128);
        let fs = FrameSet::single_rect([16.0, 16.0, 112.0, 112.0], 2.0);
        let hi = doc.add_frame_folder("F", fs);
        assert!(doc.effective_visibility().iter().all(|v| *v));

        doc.set_layer_visible(hi, false);
        let eff = doc.effective_visibility();
        assert!(!eff[1] && !eff[2], "children hidden with the folder");
        assert!(eff[0], "the root layer below is untouched");
        doc.set_layer_visible(hi, true);

        // Clip chain: draw layer clips to White; a second clipped layer above
        // resolves to the same base. Folders refuse the flag.
        assert!(doc.set_layer_clip(2, true));
        let li = doc.add_layer_in_folder(hi, "Tone").unwrap();
        assert!(doc.set_layer_clip(li, true));
        let bases = doc.clip_bases();
        assert_eq!(bases[2], Some(1), "draw layer clips to White");
        assert_eq!(bases[li], Some(1), "the chain shares one base");
        assert!(
            !doc.set_layer_clip(doc.layers.len() - 1, true),
            "folder refuses"
        );
        // The bottom root layer has nothing below it at its depth.
        assert!(doc.set_layer_clip(0, true));
        assert_eq!(doc.clip_bases()[0], None, "no base -> flag ignored");
    }

    #[test]
    fn alpha_lock_masks_the_open_op_and_locks_guard_merge() {
        let mut doc = Document::new(128, 128);
        // Base state: alpha ramp 0 / half / full across three pixels.
        let idx = TileIdx::new(0, 0);
        doc.layers[0].tile_mut(idx).set_pixel(0, 0, [0, 0, 0, 0]);
        doc.layers[0]
            .tile_mut(idx)
            .set_pixel(1, 0, [8192, 8192, 8192, 16384]);
        doc.layers[0]
            .tile_mut(idx)
            .set_pixel(2, 0, [0, 0, 32768, 32768]);

        doc.begin_op();
        // Paint opaque red over all three.
        for x in 0..3 {
            doc.layers[0]
                .tile_mut(idx)
                .set_pixel(x, 0, [32768, 0, 0, 32768]);
        }
        doc.mask_op_to_alpha();
        doc.end_op();

        let t = doc.layers[0].tile(idx).unwrap();
        assert_eq!(t.pixel(0, 0)[3], 0, "alpha 0 keeps nothing");
        assert_eq!(t.pixel(1, 0)[3], 16384, "alpha is preserved EXACTLY");
        assert_eq!(
            t.pixel(2, 0),
            [32768, 0, 0, 32768],
            "opaque takes full paint"
        );
        // Half-alpha pixel now shows pure red at its original opacity: the
        // opaque stroke replaces the colour (sa = 1), alpha stays 0.5.
        assert_eq!(t.pixel(1, 0), [16384, 0, 0, 16384]);

        // A brand-new tile under alpha lock evaporates.
        let idx2 = TileIdx::new(1, 1);
        doc.begin_op();
        doc.layers[0]
            .tile_mut(idx2)
            .set_pixel(0, 0, [32768, 0, 0, 32768]);
        doc.mask_op_to_alpha();
        doc.end_op();
        assert!(doc.layers[0].tile(idx2).is_none());

        // Locked layers refuse merges.
        doc.add_layer("top");
        doc.set_layer_lock(1, true);
        assert!(!doc.merge_down(1));
        doc.set_layer_lock(1, false);
        doc.set_layer_clip(1, true);
        assert!(
            !doc.merge_down(1),
            "a clipped layer's raw pixels are not what it shows"
        );
    }

    #[test]
    fn folder_blocks_move_remove_and_duplicate_together() {
        let mut doc = Document::new(128, 128);
        let fs = FrameSet::single_rect([16.0, 16.0, 112.0, 112.0], 2.0);
        doc.add_frame_folder("F", fs);
        doc.rename_layer(0, "Base");
        // [Base, White, Layer 1, F]

        // Move the folder block below Base: slot 0.
        assert!(doc.move_block_to_slot(3, 0, 0));
        let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["White", "Layer 1", "F", "Base"]);
        assert_eq!(doc.children_range(2), 0..2, "children travelled with it");
        assert_eq!(doc.active, 1, "active stayed on the draw layer");

        // Refuse dropping a folder into itself.
        assert!(!doc.move_block_to_slot(2, 1, 1));

        // Duplicate copies the whole block.
        let at = doc.duplicate_layer(2).unwrap();
        assert_eq!(doc.layers.len(), 7);
        assert_eq!(doc.layers[at].name, "F copy");
        assert!(doc.layers[at].folder);
        assert_eq!(doc.children_range(at).len(), 2);

        // Remove takes children along; the last block cannot be removed.
        assert!(doc.remove_layer(at));
        assert_eq!(doc.layers.len(), 4);
        assert!(doc.remove_layer(3), "Base can go");
        assert!(
            !doc.remove_layer(2),
            "removing the only remaining block would empty the doc"
        );
    }

    #[test]
    fn depth_normalization_clamps_orphans() {
        let mut doc = Document::new(64, 64);
        doc.add_layer("A");
        // Force an invalid depth by hand, then run any structural op.
        doc.layers[1].depth = 3;
        doc.add_layer_above(1, "B");
        assert_eq!(doc.layers[1].depth, 0, "no folder above — depth clamped");
        assert_eq!(doc.layers[2].depth, 0);
    }

    #[test]
    fn add_layer_in_folder_and_folder_above() {
        let mut doc = Document::new(64, 64);
        let fi = doc.add_folder_above(0, "Folder");
        assert!(doc.layers[fi].folder && doc.layers[fi].open);
        let li = doc.add_layer_in_folder(fi, "Inner").unwrap();
        assert_eq!(doc.layers[li].depth, 1);
        assert_eq!(doc.children_range(fi + 1), li..li + 1);
        assert!(doc.add_layer_in_folder(0, "nope").is_none());
    }

    /// PC-002: the folder wears its topmost child's palette colour, unless it
    /// has one of its own. The nesting case is the one worth pinning — the
    /// scan is flat, and it must still give the recursive answer.
    #[test]
    fn a_folder_shows_the_topmost_palette_colour_from_inside_it() {
        const RED: [u8; 3] = [200, 40, 40];
        const BLUE: [u8; 3] = [40, 60, 200];
        const GREEN: [u8; 3] = [40, 200, 60];

        // `add_layer_in_folder` inserts AT the header's index (new topmost
        // child), so the header shifts up by one each time.
        let mut doc = Document::new(64, 64);
        let mut outer = doc.add_folder_above(0, "Outer");
        let lower = doc.add_layer_in_folder(outer, "lower").unwrap();
        outer += 1;
        let upper = doc.add_layer_in_folder(outer, "upper").unwrap();
        outer += 1;
        assert!(doc.layers[outer].folder && doc.layers[upper].depth == 1);

        // Nothing labelled: no colour invented.
        assert_eq!(doc.palette_colour(outer), None, "empty of labels: bare");

        doc.set_layer_label(lower, Some(RED));
        assert_eq!(doc.palette_colour(outer), Some(RED), "the only one there");
        doc.set_layer_label(upper, Some(BLUE));
        assert_eq!(
            doc.palette_colour(outer),
            Some(BLUE),
            "TOPmost child wins, not the first one found from the bottom"
        );

        // The folder's own colour beats anything inside it (CSP skips the
        // inheritance entirely once the folder has one).
        doc.set_layer_label(outer, Some(GREEN));
        assert_eq!(doc.palette_colour(outer), Some(GREEN));
        doc.set_layer_label(outer, None);

        // Neither neighbour outside the folder leaks in.
        let above = doc.add_layer_above(outer, "above");
        doc.set_layer_label(above, Some(GREEN));
        doc.set_layer_label(0, Some(GREEN));
        assert_eq!(doc.palette_colour(above), Some(GREEN), "its own label");
        assert_eq!(doc.palette_colour(outer), Some(BLUE), "no leak from outside");

        // Nesting: an inner folder with no colour of its own is transparent to
        // the flat scan — the outer folder still finds the label further in,
        // which is the answer recursion would have given.
        let mut doc = Document::new(64, 64);
        let outer = doc.add_folder_above(0, "Outer");
        let child = doc.add_layer_in_folder(outer, "child").unwrap();
        let inner = doc.add_folder_above(child, "Inner");
        let deep = doc.add_layer_in_folder(inner, "deep").unwrap();
        let (inner, outer) = (inner + 1, outer + 3);
        assert!(doc.layers[inner].folder && doc.layers[outer].folder);
        assert_eq!(doc.layers[deep].depth, 2, "two levels in");
        doc.set_layer_label(deep, Some(RED));
        assert_eq!(doc.palette_colour(inner), Some(RED), "one level up");
        assert_eq!(doc.palette_colour(outer), Some(RED), "and two");
    }

    #[test]
    fn blend_variants_exist() {
        // Present-but-unused by design; keeps the enum from being "added later"
        // in a way that churns match arms across three crates.
        let all = [Blend::Normal, Blend::Multiply, Blend::Screen];
        assert_eq!(all[0], Blend::default());
    }

    #[test]
    fn resize_grow_center_pins_content_to_the_middle() {
        let mut doc = Document::new(128, 128);
        let put = |doc: &mut Document, x: i32, y: i32| {
            let ti = TileIdx::of_pixel(x, y);
            let (ox, oy) = ti.origin();
            doc.layers[0].tile_mut(ti).set_pixel(
                (x - ox) as usize,
                (y - oy) as usize,
                [1, 2, 3, FIX15_ONE as u16],
            );
        };
        let get = |doc: &Document, x: i32, y: i32| -> Option<[u16; 4]> {
            let ti = TileIdx::of_pixel(x, y);
            doc.layers[0]
                .tile(ti)
                .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize))
        };
        put(&mut doc, 10, 10);
        doc.begin_op();
        put(&mut doc, 11, 10);
        doc.end_op();
        assert!(doc.can_undo());

        doc.resize_canvas(256, 256, ResizeAnchor::Center);
        assert_eq!(doc.size, (256, 256));
        // (256-128)/2 = +64: the pixel moved with the content.
        assert_eq!(get(&doc, 74, 74).unwrap()[3], FIX15_ONE as u16);
        assert!(get(&doc, 10, 10).is_none(), "source vacated");
        assert!(!doc.can_undo(), "structural: history cleared");
        assert!(doc.selection.is_none());
    }

    #[test]
    fn crop_offsets_and_trims_outside_tiles() {
        let mut doc = Document::new(256, 256);
        let put = |doc: &mut Document, x: i32, y: i32| {
            let ti = TileIdx::of_pixel(x, y);
            let (ox, oy) = ti.origin();
            doc.layers[0].tile_mut(ti).set_pixel(
                (x - ox) as usize,
                (y - oy) as usize,
                [9, 9, 9, FIX15_ONE as u16],
            );
        };
        let get = |doc: &Document, x: i32, y: i32| -> Option<[u16; 4]> {
            let ti = TileIdx::of_pixel(x, y);
            doc.layers[0]
                .tile(ti)
                .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize))
        };
        put(&mut doc, 130, 130); // tile (2, 2) — survives the crop
        put(&mut doc, 200, 200); // tile (3, 3) — fully outside 64x64, dropped

        // Crop to the 64x64 area whose top-left is (128, 128).
        doc.resize_to(64, 64, -128, -128);
        assert_eq!(doc.size, (64, 64));
        assert_eq!(
            get(&doc, 2, 2).unwrap()[3],
            FIX15_ONE as u16,
            "kept pixel at origin"
        );
        assert!(
            doc.layers[0].tile(TileIdx::new(3, 3)).is_none(),
            "outside tile trimmed"
        );
    }

    #[test]
    fn resize_translates_frame_vectors_and_reextends_white() {
        let mut doc = Document::new(256, 256);
        let fs = FrameSet::single_rect([32.0, 32.0, 224.0, 224.0], 4.0);
        let hi = doc.add_frame_folder("Frame 1", fs);
        let white = doc.layers.iter().position(|l| l.name == "White").unwrap();

        // Grow top-left-anchored: content stays, new paper appears right/below.
        doc.resize_canvas(512, 512, ResizeAnchor::TopLeft);
        assert_eq!(doc.size, (512, 512));
        // The frame moved with the content (dx = dy = 0 here, but the
        // rasters re-derived at the larger size and White re-extended).
        assert_eq!(
            doc.layers[hi].frames().unwrap().frames[0].points[0],
            [32.0, 32.0]
        );
        let far = TileIdx::new(7, 7);
        assert_eq!(
            doc.layers[white].tile(far).map(|t| t.pixel(1, 1)),
            Some([FIX15_ONE as u16; 4]),
            "White covers the grown corner"
        );
        assert!(doc.layers[hi].mask_tiles().is_some(), "mask re-derived");

        // Shrink + move: vectors follow the offset.
        doc.resize_to(256, 256, -64, -64);
        assert_eq!(
            doc.layers[hi].frames().unwrap().frames[0].points[0],
            [-32.0, -32.0]
        );
    }

    #[test]
    fn anchor_offsets_match_the_nine_positions() {
        use ResizeAnchor::*;
        let old = (100u32, 100u32);
        assert_eq!(TopLeft.offsets(old, (300, 50)), (0, 0));
        assert_eq!(Center.offsets(old, (300, 50)), (100, -25));
        assert_eq!(BottomRight.offsets(old, (300, 50)), (200, -50));
        assert_eq!(Top.offsets(old, (300, 50)), (100, 0));
        assert_eq!(Left.offsets(old, (300, 50)), (0, -25));
    }

    #[test]
    fn tone_conversion_is_nondestructive_and_undoable() {
        use crate::tone::ToneParams;
        let mut doc = Document::new(256, 256);
        // A half-ink block in the middle tile — non-blank through the screen
        // at any phase (32×32 at ~50 % coverage), and guaranteed DIFFERENT
        // from the uniform source (full ink rasterizes to itself: 100 %
        // coverage is solid black by design).
        doc.begin_op();
        {
            let t = doc.active_layer_mut().tile_mut(TileIdx::new(2, 2));
            for y in 16..48 {
                for x in 16..48 {
                    t.set_pixel(x, y, [0, 0, 0, (FIX15_ONE / 2) as u16]);
                }
            }
        }
        doc.end_op();
        let src_arc = doc
            .active_layer()
            .tile(TileIdx::new(2, 2))
            .cloned()
            .unwrap();

        assert!(doc.set_tone(0, Some(ToneParams::default())));
        doc.refresh_derived(600);
        let shown = doc.layers[0]
            .display_tile(TileIdx::new(2, 2))
            .cloned()
            .unwrap();
        assert!(!shown.is_blank(), "tone raster derived");
        assert_ne!(shown.data(), src_arc.data(), "display differs from source");
        // The SOURCE is untouched — that is the non-destructive half.
        assert_eq!(
            doc.active_layer().tile(TileIdx::new(2, 2)).unwrap().data(),
            src_arc.data()
        );

        // Undo restores plain pixels as one step; redo re-tons.
        assert!(doc.undo());
        assert!(doc.layers[0].tone.is_none());
        assert_eq!(
            doc.layers[0]
                .display_tile(TileIdx::new(2, 2))
                .unwrap()
                .data(),
            src_arc.data(),
            "after undo the display is the plain source again"
        );
        assert!(doc.redo());
        doc.refresh_derived(600);
        assert!(doc.layers[0].tone.is_some());

        // Vector layers and folders refuse conversion.
        doc.add_balloon_layer("B", crate::balloon::BalloonSet::new(2.0));
        assert!(!doc.set_tone(1, Some(ToneParams::default())));
    }

    #[test]
    fn tone_raster_follows_new_source_edits() {
        use crate::tone::ToneParams;
        let mut doc = Document::new(256, 256);
        doc.active_layer_mut()
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(10, 10, [0, 0, 0, FIX15_ONE as u16]);
        assert!(doc.set_tone(0, Some(ToneParams::default())));
        doc.refresh_derived(600);
        let rev1 = doc.layers[0]
            .display_tile(TileIdx::new(0, 0))
            .unwrap()
            .revision();

        // Paint more ink on the source (as a stroke would): the derived tile
        // must re-derive with a newer revision; untouched tiles don't.
        doc.active_layer_mut()
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(12, 12, [0, 0, 0, FIX15_ONE as u16]);
        doc.refresh_derived(600);
        let rev2 = doc.layers[0]
            .display_tile(TileIdx::new(0, 0))
            .unwrap()
            .revision();
        assert!(rev2 > rev1, "dirty source re-derives");

        // A clean refresh is a no-op (same revision object).
        doc.refresh_derived(600);
        assert_eq!(
            doc.layers[0]
                .display_tile(TileIdx::new(0, 0))
                .unwrap()
                .revision(),
            rev2
        );

        // Param change re-derives everything (map dropped).
        let mut p = ToneParams::default();
        p.lpi = 42.5;
        assert!(doc.set_tone(0, Some(p)));
        doc.refresh_derived(600);
        assert!(
            doc.layers[0]
                .display_tile(TileIdx::new(0, 0))
                .unwrap()
                .revision()
                > rev2
        );
    }
}

#[cfg(test)]
mod combine_tests {
    use super::*;

    /// FB-035/036: combining two divide-siblings pools children under
    /// one header; with `merge_borders` the two adjacent rects become
    /// ONE frame at the union bbox; keep-shapes concatenates.
    #[test]
    fn combine_frame_folders_pools_and_merges() {
        let mut doc = Document::new(400, 400);
        let a = doc.add_frame_folder(
            "Frame 1",
            FrameSet::single_rect([16.0, 16.0, 200.0, 300.0], 4.0),
        );
        let b = doc.add_frame_folder(
            "Frame 2",
            FrameSet::single_rect([200.0, 16.0, 384.0, 300.0], 4.0),
        );
        let before = doc.layers.len();
        // Keep shapes: two frames, one header, all children pooled.
        let h = doc.combine_frame_folders(a, b, false).expect("combined");
        assert_eq!(doc.layers.len(), before - 1, "one header gone");
        assert!(doc.layers[h].is_frame());
        let fs = doc.layers[h].frames().unwrap();
        assert_eq!(fs.frames.len(), 2, "shapes kept");
        assert!(doc.children_range(h).len() >= 4, "children pooled");
        assert!(doc.layers.iter().filter(|l| l.is_frame()).count() == 1);

        // Combine-borders on adjacent siblings: one union-bbox frame.
        let mut doc2 = Document::new(400, 400);
        let a2 = doc2.add_frame_folder(
            "Frame 1",
            FrameSet::single_rect([16.0, 16.0, 200.0, 300.0], 4.0),
        );
        let b2 = doc2.add_frame_folder(
            "Frame 2",
            FrameSet::single_rect([200.0, 16.0, 384.0, 300.0], 4.0),
        );
        let h2 = doc2.combine_frame_folders(a2, b2, true).expect("combined");
        let fs2 = doc2.layers[h2].frames().unwrap();
        assert_eq!(fs2.frames.len(), 1, "one merged border");
        let u = fs2.frames[0].bbox();
        assert_eq!(u, [16.0, 16.0, 384.0, 300.0], "the union bbox");
        // Self-combine refuses (no meaningful merge with itself).
        let mut doc3 = Document::new(400, 400);
        let x = doc3.add_frame_folder("F", FrameSet::single_rect([0.0, 0.0, 100.0, 100.0], 4.0));
        assert!(doc3.combine_frame_folders(x, x, false).is_none());
        // Differing depths refuse (a nested child and a top-level folder).
        let y = doc3.add_frame_folder("G", FrameSet::single_rect([0.0, 0.0, 100.0, 100.0], 4.0));
        let _p = doc3
            .group_frame_folders_common_parent(x, y)
            .expect("grouped for the depth case");
        // `x` and `y` now nest at depth 1 inside the new parent.
        let z = doc3.add_frame_folder("H", FrameSet::single_rect([0.0, 0.0, 100.0, 100.0], 4.0));
        assert!(
            doc3.combine_frame_folders(x, z, false).is_none(),
            "differing depths refuse"
        );
    }

    #[test]
    fn group_common_parent_splices_separated_siblings() {
        // Audit E, 2026-08-19: the old guard REFUSED any non-adjacent
        // sibling pair as "not siblings" (the condition tested for
        // separation), and its insert position would have left the lower
        // block outside the parent. Separated siblings group by splicing
        // the lower block adjacent to the higher one (CSP semantics: the
        // selection moves to the highest position; intervening layers
        // stay put, below both). Indices MOVE in a splice — re-find by
        // name after the call.
        let mut doc = Document::new(400, 400);
        let a = doc.add_frame_folder(
            "Frame 1",
            FrameSet::single_rect([16.0, 16.0, 184.0, 300.0], 4.0),
        );
        let b0 = doc.add_frame_folder(
            "Frame 2",
            FrameSet::single_rect([216.0, 16.0, 384.0, 300.0], 4.0),
        );
        // A plain TOP-LEVEL layer BETWEEN the two blocks (add_layer would
        // land inside Frame 1's block — the active layer is its draw
        // layer — so insert explicitly at Frame 2's block start).
        let mut between = Layer::new("bg");
        between.depth = 0;
        doc.layers.insert(b0 - 2, between);
        let b = b0 + 1;
        let h = doc
            .group_frame_folders_common_parent(a, b)
            .expect("grouped");
        assert!(
            doc.layers[h].folder && !doc.layers[h].is_frame(),
            "a plain parent"
        );
        let kids = doc.children_range(h);
        assert_eq!(kids.len(), 6, "both folders' whole blocks inside");
        let headers: Vec<usize> = kids
            .clone()
            .filter(|&i| doc.layers[i].is_frame() && doc.layers[i].folder)
            .collect();
        assert_eq!(headers.len(), 2, "both frame headers are children");
        assert!(headers.iter().all(|&i| doc.layers[i].depth == 1));
        let bg = doc
            .layers
            .iter()
            .position(|l| l.name == "bg")
            .expect("the plain layer survives");
        assert_eq!(doc.layers[bg].depth, 0, "the plain layer stays outside");
        assert!(bg < kids.start, "the moved block crossed it, staying above");
        assert_eq!(doc.active, h, "the new parent header is the selection");
    }

    #[test]
    fn combine_refuses_folders_in_different_parents() {
        // Audit H: equal depth is not parenthood — folders in DIFFERENT
        // parents must not combine (the merged folder would land in one
        // parent and silently empty the other). Grouping inserts layers,
        // so the headers are re-found by name before each call.
        let hdr =
            |doc: &Document, name: &str| doc.layers.iter().position(|l| l.name == name).unwrap();
        let mut doc = Document::new(400, 400);
        for n in ["A", "B", "C", "D"] {
            doc.add_frame_folder(n, FrameSet::single_rect([0.0, 0.0, 100.0, 100.0], 4.0));
        }
        doc.group_frame_folders_common_parent(hdr(&doc, "A"), hdr(&doc, "B"))
            .expect("P1");
        doc.group_frame_folders_common_parent(hdr(&doc, "C"), hdr(&doc, "D"))
            .expect("P2");
        assert!(
            doc.combine_frame_folders(hdr(&doc, "A"), hdr(&doc, "C"), false)
                .is_none(),
            "same depth, different parents — refused"
        );
        assert!(
            doc.group_frame_folders_common_parent(hdr(&doc, "A"), hdr(&doc, "C"))
                .is_none(),
            "same depth, different parents — refused (group)"
        );
        assert!(
            doc.combine_frame_folders(hdr(&doc, "A"), hdr(&doc, "B"), false)
                .is_some(),
            "true siblings still combine"
        );
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;
    use crate::tile::FIX15_ONE;

    /// FB-037: a common parent wraps both frame folders — headers and
    /// shapes survive, both blocks deepen by one.
    #[test]
    fn common_parent_wraps_without_touching_originals() {
        let mut doc = Document::new(400, 400);
        let a = doc.add_frame_folder(
            "Frame 1",
            FrameSet::single_rect([0.0, 0.0, 180.0, 300.0], 4.0),
        );
        let b = doc.add_frame_folder(
            "Frame 2",
            FrameSet::single_rect([200.0, 0.0, 380.0, 300.0], 4.0),
        );
        let fa = doc.layers[a].frames().unwrap().clone();
        let fb = doc.layers[b].frames().unwrap().clone();
        let h = doc
            .group_frame_folders_common_parent(a, b)
            .expect("grouped");
        assert!(
            doc.layers[h].folder && !doc.layers[h].is_frame(),
            "a PLAIN parent"
        );
        assert_eq!(doc.layers[h].depth, 0);
        // Both frame folders survive verbatim, one level deeper.
        let kids = doc.children_range(h);
        let frames: Vec<_> = kids.clone().filter(|&i| doc.layers[i].is_frame()).collect();
        assert_eq!(frames.len(), 2, "both headers survive");
        for &fh in &frames {
            assert_eq!(doc.layers[fh].depth, 1);
        }
        assert!(
            frames
                .iter()
                .any(|&fh| doc.layers[fh].frames().unwrap().frames[0].bbox() == fa.frames[0].bbox())
                && frames
                    .iter()
                    .any(|&fh| doc.layers[fh].frames().unwrap().frames[0].bbox()
                        == fb.frames[0].bbox()),
            "shapes untouched"
        );
    }

    /// The r83 handoff flag: combining across an INTERVENING plain
    /// folder block — the pooled children land inside the surviving
    /// block and the intervening folder stays intact.
    #[test]
    fn combine_across_an_intervening_folder() {
        let mut doc = Document::new(400, 400);
        doc.add_frame_folder(
            "Frame 1",
            FrameSet::single_rect([0.0, 0.0, 180.0, 300.0], 4.0),
        );
        // A plain folder BETWEEN the two frame folders' blocks: insert
        // above the ROOT layer so it lands outside Frame 1's block.
        doc.add_folder_above(0, "notes");
        doc.add_frame_folder(
            "Frame 2",
            FrameSet::single_rect([200.0, 0.0, 380.0, 300.0], 4.0),
        );
        // Resolve by name — indices shifted with every insert.
        let a = doc
            .layers
            .iter()
            .position(|l| l.name == "Frame 1" && l.is_frame())
            .unwrap();
        let b = doc
            .layers
            .iter()
            .position(|l| l.name == "Frame 2" && l.is_frame())
            .unwrap();
        let plain = doc.layers.iter().position(|l| l.name == "notes").unwrap();
        let before_plain = doc.layers[plain].clone();
        let plain_kids = doc.children_range(plain).len();
        let h = doc.combine_frame_folders(a, b, false).expect("combined");
        // One frame folder fewer; both frames in the survivor; the plain
        // folder block untouched.
        assert_eq!(
            doc.layers.iter().filter(|l| l.is_frame()).count(),
            1,
            "one frame folder remains"
        );
        assert_eq!(doc.layers[h].frames().unwrap().frames.len(), 2);
        let p = doc.layers.iter().position(|l| l.name == "notes").unwrap();
        assert_eq!(doc.layers[p].depth, before_plain.depth);
        assert_eq!(
            doc.children_range(p).len(),
            plain_kids,
            "plain folder intact"
        );
    }

    /// LP-002/LP-003. The property the whole feature rests on: the keyline is
    /// grown from the layer's OWN alpha and lands in tiles the layer never
    /// painted — the dilation is not pointwise, and a compositor that only
    /// walked source tiles would clip the outline at every tile edge. Also
    /// the non-destructive half: the pixels survive and one undo restores.
    #[test]
    fn border_effect_rings_the_ink_across_a_tile_edge() {
        use crate::edge::EdgeParams;
        let mut doc = Document::new(256, 256);
        // One opaque pixel in the LAST column of tile (0,0), so a 6 px
        // keyline round it has to spill into tile (1,0).
        doc.begin_op();
        doc.active_layer_mut()
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(63, 32, [0, 0, 0, FIX15_ONE as u16]);
        doc.end_op();
        let src = doc
            .active_layer()
            .tile(TileIdx::new(0, 0))
            .cloned()
            .unwrap();

        assert!(doc.set_edge(
            0,
            Some(EdgeParams {
                width_px: 6.0,
                colour: [255, 255, 255],
            })
        ));
        doc.refresh_derived(600);

        // Canvas (66,32) is 3 px from the ink, in the NEXT tile along.
        let right = doc.layers[0]
            .display_tile(TileIdx::new(1, 0))
            .cloned()
            .expect("the neighbour tile is displayed even with no source in it");
        assert_eq!(right.pixel(2, 32), [32768, 32768, 32768, 32768]);
        assert_eq!(right.pixel(60, 60), [0; 4], "and it stops well short");
        assert!(
            doc.layers[0].tile(TileIdx::new(1, 0)).is_none(),
            "no SOURCE tile was invented to hold the outline"
        );
        // The ink shows through the ring, and the painted tile is untouched.
        assert_eq!(
            doc.layers[0]
                .display_tile(TileIdx::new(0, 0))
                .unwrap()
                .pixel(63, 32),
            [0, 0, 0, 32768],
            "the ink itself is still black"
        );
        assert_eq!(
            doc.layers[0].tile(TileIdx::new(0, 0)).unwrap().data(),
            src.data(),
            "the painted pixels never changed"
        );

        // One undo step, and the layer is exactly the drawing again.
        assert!(doc.undo());
        assert!(doc.layers[0].edge.is_none());
        assert!(
            doc.layers[0].display_tile(TileIdx::new(1, 0)).is_none(),
            "no outline left over in the neighbour tile"
        );
        assert_eq!(
            doc.layers[0]
                .display_tile(TileIdx::new(0, 0))
                .unwrap()
                .data(),
            src.data()
        );
    }

    /// The other half of TRIAGE 27: the outline must FOLLOW the layer. New
    /// ink on an already-outlined layer gets its own keyline on the next
    /// refresh — this is what the `edge_stamp` early-out is allowed to skip
    /// and must not skip wrongly. Folders have no alpha of their own and are
    /// refused outright.
    #[test]
    fn border_effect_follows_new_ink_and_refuses_folders() {
        use crate::edge::EdgeParams;
        let mut doc = Document::new(256, 256);
        doc.begin_op();
        doc.active_layer_mut()
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(10, 10, [0, 0, 0, FIX15_ONE as u16]);
        doc.end_op();
        let p = EdgeParams {
            width_px: 6.0,
            colour: [255, 255, 255],
        };
        assert!(doc.set_edge(0, Some(p)));
        doc.refresh_derived(600);
        assert!(
            doc.layers[0].display_tile(TileIdx::new(2, 2)).is_none(),
            "a tile the outline cannot reach is never derived"
        );

        // Draw somewhere new; refresh; the new mark is ringed too.
        doc.begin_op();
        doc.active_layer_mut()
            .tile_mut(TileIdx::new(2, 2))
            .set_pixel(10, 10, [0, 0, 0, FIX15_ONE as u16]);
        doc.end_op();
        doc.refresh_derived(600);
        let t = doc.layers[0]
            .display_tile(TileIdx::new(2, 2))
            .cloned()
            .expect("the new ink's tile is derived");
        assert_eq!(t.pixel(14, 10), [32768, 32768, 32768, 32768]);
        assert_eq!(t.pixel(10, 10), [0, 0, 0, 32768], "the new ink is intact");
        // …and the OLD mark still has its ring (the cache reuse is not a leak).
        assert_eq!(
            doc.layers[0]
                .display_tile(TileIdx::new(0, 0))
                .unwrap()
                .pixel(14, 10),
            [32768, 32768, 32768, 32768]
        );

        // Setting the same params again is a no-op — no undo step is spent.
        assert!(!doc.set_edge(0, Some(p)));
        // A folder header composites a group; there is no single alpha to
        // grow an outline from.
        let fi = doc.add_folder_above(0, "Folder");
        assert!(!doc.set_edge(fi, Some(p)));
    }

    /// The early-out's sharp edge. One op that PRUNES an emptied tile and
    /// creates another leaves the source-tile COUNT unchanged, and where the
    /// ink vanished the neighbourhood's newest revision DROPS — so a cache
    /// keyed on the count alone still reads "derived after the newest
    /// source" and keeps a ghost outline round ink that is gone. The stamp
    /// hashes the tile SET for exactly this.
    #[test]
    fn border_effect_does_not_ghost_when_one_tile_is_traded_for_another() {
        use crate::edge::EdgeParams;
        let mut doc = Document::new(512, 512);
        doc.begin_op();
        doc.active_layer_mut()
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(10, 10, [0, 0, 0, FIX15_ONE as u16]);
        doc.end_op();
        assert!(doc.set_edge(
            0,
            Some(EdgeParams {
                width_px: 4.0,
                colour: [255, 255, 255],
            })
        ));
        doc.refresh_derived(600);
        assert_eq!(
            doc.layers[0]
                .display_tile(TileIdx::new(0, 0))
                .unwrap()
                .pixel(13, 10)[3],
            32768
        );

        // Same count, different set.
        let mut moved = Tile::new_transparent();
        moved.set_pixel(10, 10, [0, 0, 0, FIX15_ONE as u16]);
        doc.layers[0].set_tile(TileIdx::new(0, 0), None);
        doc.layers[0].set_tile(TileIdx::new(4, 4), Some(Arc::new(moved)));
        doc.refresh_derived(600);
        assert!(
            doc.layers[0].display_tile(TileIdx::new(0, 0)).is_none(),
            "the outline left with the ink it belonged to"
        );
        assert_eq!(
            doc.layers[0]
                .display_tile(TileIdx::new(4, 4))
                .expect("the ink's new home is outlined")
                .pixel(13, 10)[3],
            32768
        );
    }

    /// LP-017 + LP-022 are presentation switches: they change what is shown,
    /// spend no undo step (like visibility), and reject a no-op.
    #[test]
    fn sub_colour_and_expression_are_presentation_switches() {
        let mut doc = Document::new(64, 64);
        let before = doc.undo_len();
        assert!(doc.set_layer_sub_colour(0, Some([1, 2, 3])));
        assert!(doc.set_layer_expression(0, LayerExpression::Mono));
        assert_eq!(doc.layers[0].layer_sub_colour, Some([1, 2, 3]));
        assert_eq!(doc.layers[0].expression, LayerExpression::Mono);
        assert!(
            !doc.set_layer_expression(0, LayerExpression::Mono),
            "setting what is already set is not a change"
        );
        assert_eq!(doc.undo_len(), before, "no undo step is spent");
        assert!(doc.set_layer_sub_colour(0, None));
        assert!(doc.set_layer_expression(0, LayerExpression::Colour));
        // Out-of-range indices are a false, not a panic.
        assert!(!doc.set_layer_sub_colour(99, None));
        assert!(!doc.set_layer_expression(99, LayerExpression::Grey));
        assert!(!doc.set_edge(99, None));
    }
}

/// PR-041 — the operation counter the "save recovery data for every
/// operation" preference fires on.
#[cfg(test)]
mod op_count_tests {
    use super::*;

    /// One undoable edit on the active layer.
    fn edit(doc: &mut Document) {
        doc.begin_op();
        let li = doc.active;
        doc.layers[li]
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(1, 1, [32768, 0, 0, 32768]);
        doc.end_op();
    }

    #[test]
    fn every_edit_undo_and_redo_moves_the_count() {
        let mut doc = Document::new(128, 128);
        let start = doc.op_count();

        edit(&mut doc);
        let after_edit = doc.op_count();
        assert!(after_edit > start, "an edit is an operation");

        assert!(doc.undo());
        let after_undo = doc.op_count();
        assert!(
            after_undo > after_edit,
            "undo is an operation too — its result is what needs recovering"
        );

        assert!(doc.redo());
        assert!(doc.op_count() > after_undo, "and so is redo");
    }

    /// The reason this is not `undo_len()`. Past the depth cap the stack
    /// stops growing while the work does not, and a recovery save keyed to
    /// the stack length would quietly stop firing — mid-session, with no
    /// symptom, in the one feature whose entire job is to not do that.
    #[test]
    fn the_count_keeps_moving_after_the_depth_cap_is_reached() {
        let mut doc = Document::new(128, 128);
        doc.set_undo_limit(50); // the preference's own floor
        for _ in 0..50 {
            edit(&mut doc);
        }
        let capped = doc.undo_len();
        let ops = doc.op_count();

        for _ in 0..10 {
            edit(&mut doc);
        }
        assert_eq!(doc.undo_len(), capped, "the stack is full and stays full");
        assert_eq!(
            doc.op_count(),
            ops + 10,
            "the tally is not the stack; it counted all ten"
        );
    }

    /// Structural layer ops push no group — they clear the history instead
    /// — so counting only pushes would leave exactly the changes undo
    /// cannot bring back uncounted.
    #[test]
    fn structural_ops_count_and_clearing_the_history_does_not_reset_the_tally() {
        let mut doc = Document::new(128, 128);
        edit(&mut doc);
        let before = doc.op_count();

        doc.add_layer("ref");
        assert!(
            doc.op_count() > before,
            "adding a layer is an operation ({before} -> {})",
            doc.op_count()
        );
        assert_eq!(doc.undo_len(), 0, "…and it threw the history away");
        assert!(
            doc.op_count() >= before,
            "the tally is monotonic: clear_history must never rewind it"
        );
    }
    // --- PA-001: the paper + the transparency checker ---------------------

    /// The colour is CONTENT: an empty page composites to it, and that is
    /// what a PNG export writes. A cream page prints cream.
    #[test]
    fn paper_colour_is_what_an_empty_page_exports() {
        let mut doc = Document::new(64, 64);
        assert_eq!(doc.paper, Paper::default(), "new documents are white paper");

        assert!(doc.set_paper_colour([250, 243, 224]), "cream");
        let img = crate::export::composite_for_export(&doc, doc.paper_export_background());
        assert_eq!(
            img.get_pixel(10, 10).0,
            [250, 243, 224, 255],
            "the empty page IS the paper"
        );

        // Setting the same colour again is a no-op, and pushes no undo entry.
        let depth = doc.history.undo_len();
        assert!(!doc.set_paper_colour([250, 243, 224]));
        assert_eq!(doc.history.undo_len(), depth, "a no-op pushes nothing");
    }

    /// THE EXPORT RULE. Hiding the paper is a look-at-it check — it must not
    /// be able to ship a page with a transparent background, and the checker
    /// must never reach a PNG. The exported bytes are identical either way.
    #[test]
    fn hiding_the_paper_never_changes_an_export() {
        let mut doc = Document::new(64, 64);
        doc.set_paper_colour([250, 243, 224]);
        doc.begin_op();
        doc.layers[0]
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(4, 4, [0, 0, 0, 32768]);
        doc.end_op();

        let shown = crate::export::composite_for_export(&doc, doc.paper_export_background());
        assert!(doc.set_paper_visible(false));
        let hidden = crate::export::composite_for_export(&doc, doc.paper_export_background());
        assert_eq!(
            shown.as_raw(),
            hidden.as_raw(),
            "the paper's eye is view state: an export cannot see it"
        );
        assert_eq!(
            hidden.get_pixel(10, 10).0,
            [250, 243, 224, 255],
            "still exports ON the paper colour with the eye off, and opaque"
        );
    }

    /// The other half: on SCREEN the eye does reach the composite, and it
    /// reaches it as real transparency — which is what the viewer draws the
    /// checker through. Painted pixels are untouched either way.
    #[test]
    fn hiding_the_paper_makes_the_screen_composite_transparent() {
        let mut doc = Document::new(64, 64);
        doc.begin_op();
        doc.layers[0]
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(4, 4, [0, 0, 0, 32768]);
        doc.end_op();

        assert_eq!(
            doc.paper_background(),
            crate::export::Background::Solid([255, 255, 255])
        );
        assert!(doc.set_paper_visible(false));
        assert_eq!(
            doc.paper_background(),
            crate::export::Background::Transparent,
            "no paper means nothing under the stack"
        );

        let img = crate::export::composite(&doc, doc.paper_background());
        assert_eq!(img.get_pixel(10, 10).0[3], 0, "empty pixels are a hole");
        assert_eq!(img.get_pixel(4, 4).0[3], 255, "the art is untouched");
    }

    /// The colour is undoable (it is content); the eye is not (it is view
    /// state, like a layer's own eye). The paper's undo group belongs to no
    /// layer, so a per-layer history purge must not drop it.
    #[test]
    fn paper_colour_undoes_and_the_eye_does_not() {
        let mut doc = Document::new(64, 64);
        doc.add_layer("second");

        assert!(doc.set_paper_colour([12, 34, 56]));
        assert_eq!(doc.paper.colour, [12, 34, 56]);
        assert!(doc.undo(), "the colour undoes");
        assert_eq!(doc.paper.colour, [255, 255, 255], "back to white");
        assert!(doc.redo(), "and redoes");
        assert_eq!(doc.paper.colour, [12, 34, 56]);

        // The eye pushes nothing.
        let depth = doc.history.undo_len();
        assert!(doc.set_paper_visible(false));
        assert_eq!(
            doc.history.undo_len(),
            depth,
            "the eye is view state — no undo entry"
        );
        assert!(!doc.set_paper_visible(false), "idempotent");
        assert!(doc.set_paper_visible(true));

        // A layer's history is purged: the paper's group has no layer to
        // belong to and must survive it.
        doc.history.drop_layer_history(1);
        assert!(doc.undo(), "the paper colour is still undoable");
        assert_eq!(doc.paper.colour, [255, 255, 255]);
    }
}
