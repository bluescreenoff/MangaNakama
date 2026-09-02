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

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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

    /// The brush-preset key name: the ORA name without its `svg:`/`mn:`
    /// prefix (`"multiply"`, `"linear-burn"`, …). The `.myb` rail used to
    /// hardcode `"multiply"`/`"screen"` as the only spellings it understood;
    /// this is the shared spelling so every mode round-trips through a
    /// preset file.
    pub fn short_name(self) -> &'static str {
        let full = self.ora_name();
        &full[full.find(':').map(|i| i + 1).unwrap_or(0)..]
    }

    /// [`Blend::short_name`]'s inverse, for the brush-preset key. Tries
    /// both ORA prefixes (the name no longer says which it came from);
    /// unknown strings fall back to `Normal` (a preset from a newer
    /// build must not fail to load).
    pub fn from_short_name(s: &str) -> Self {
        let a = Self::from_ora_name(&format!("svg:{s}"));
        if a.short_name() == s {
            return a;
        }
        let b = Self::from_ora_name(&format!("mn:{s}"));
        if b.short_name() == s {
            return b;
        }
        Blend::Normal
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
// (serde: the publisher profile stores an export colour — additive derive,
// nothing existing serializes through it.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// LIVE tonal correction (row 105): the content is an [`crate::Adjust`],
    /// the raster is the corrected composite of everything BELOW, derived
    /// over the paper so it is opaque and Normal blend acts as a replace.
    /// The layer mask is the window ("paint where it applies"). See
    /// `correction`.
    Correction(crate::adjust::Adjust),
    /// FILE OBJECT (row 166, `FO-001`–`009`): the layer REFERENCES an image
    /// file on disk. Unlike every other non-`Raster` kind the pixels live in
    /// the layer's ordinary `tiles` — derived from the file, re-derivable,
    /// and saved like any raster so the page still opens on a machine
    /// without the source. See `file_object`.
    FileObject(crate::file_object::FileObject),
    Frame(FrameSet),
    Balloon(BalloonSet),
    Text(TextSet),
}

/// A layer: sparse tiles + presentation state.
///
/// Tiles are `Arc`-shared so undo snapshots (a later agent) are Arc clones, and
/// the write path goes through `Arc::make_mut` for copy-on-write.
/// A layer mask (TRIAGE 138, LM-005's ALPHA scale): per-pixel coverage in
/// the tile ALPHA channel (fix15; full = visible, absent tile = VISIBLE —
/// unmasked, the rule both compositors and the bake share; a mask only has
/// tiles where the layer had ink when it was created).
/// Any brush will edit it (part 2); soft brush ⇒ soft mask, automatically.
#[derive(Clone, Debug, Default)]
pub struct LayerMask {
    pub tiles: HashMap<TileIdx, Arc<Tile>>,
    pub enabled: bool,
    /// Bumped on every edit — the GPU tile cache's rebuild signal.
    pub revision: u64,
    /// FULL-CANVAS window: the tiles this mask does not hold read as FULL
    /// coverage. That is already what both compositors and the bake do
    /// (LM-005, `mask_apply_bake`) for every mask — the flag exists because
    /// the CORRECTION derive is the one reader that uses the opposite rule.
    ///
    /// A correction's mask is its WINDOW: `correction.rs` derives only the
    /// tiles the mask holds and treats an absent tile as "outside the
    /// window, not corrected", which is what a window cut from a selection
    /// has to mean. `full` says the other thing — "the window is the whole
    /// page, and the tiles present are only the places it has been carved
    /// away" — which is what a maskless correction gets ARMED with the
    /// first time a brush touches it (`arm_full_window`). It costs no
    /// pixels: an all-visible window is an EMPTY tile map.
    ///
    /// Two rules, one field, and every mask that is not a correction window
    /// leaves it `false` and reads exactly as it always did.
    pub full: bool,
}

impl LayerMask {
    /// An all-visible window that stores NO tiles — the arm state. 30 MB of
    /// dense coverage says exactly the same thing as this empty map.
    pub fn full_window() -> Self {
        Self {
            tiles: HashMap::new(),
            enabled: true,
            revision: crate::tile::next_revision(),
            full: true,
        }
    }

    /// The tile a brush dab materialises where the mask holds none.
    ///
    /// A carved window starts one EMPTY and the dab paints coverage in. A
    /// [`full`](Self::full) window has to start it OPAQUE — otherwise the
    /// first eraser dab anywhere new would hand back a zero tile and hide
    /// the whole 64×64 of it instead of the dab's footprint.
    pub fn blank_tile(&self) -> Tile {
        let mut t = Tile::new_transparent();
        if self.full {
            t.data_mut().fill(crate::tile::FIX15_ONE as u16);
        }
        t
    }
}

/// Which half of a mask-capped breakout a composite step draws. Only a
/// breakout layer carrying an ENABLED layer mask is ever split; everything
/// else in the stack is [`SpillPart::All`].
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SpillPart {
    /// The whole layer — the ordinary case, and an uncapped breakout.
    All,
    /// What the mask lets OUT, drawn at the escaped seat: the source scaled
    /// by the mask coverage `m`.
    Out,
    /// What the mask holds IN, drawn at the layer's own seat where the panel
    /// still clips it: the source scaled by `1 − m`. An ABSENT mask tile is
    /// full coverage (the unmasked rule every compositor shares), so it
    /// holds nothing back — an untouched mask spills exactly like no mask.
    In,
}

/// One step of the shared compositor walk: which layer, at which effective
/// depth, drawing which half of a mask-capped spill.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct CompositeStep {
    pub layer: usize,
    pub depth: u8,
    pub part: SpillPart,
}

impl CompositeStep {
    fn new(layer: usize, depth: u8, part: SpillPart) -> Self {
        Self { layer, depth, part }
    }
}

/// Stable-identity mint (the automation round). Process-global and monotonic
/// (the `tile::REVISION` idiom): a fresh id is greater than every id any open
/// document has ever seen, so within one session an id is never reissued —
/// not even by a layer deleted and re-created. `0` is the "not yet assigned"
/// sentinel (the page-identity convention, `project.rs`); serde-defaulted
/// item ids from old files carry it until [`Document::ensure_ids`] runs.
///
/// Deliberately NOT persisted as a counter: uniqueness only matters within a
/// document, and [`bump_ids_past`] at load lifts the mint above everything
/// the file holds. The soft spot this left — a persisted cross-reference to
/// an id whose layer was deleted in an EARLIER session could see that id
/// reborn in a later one — is PAID OFF: breakout part 2 is the first (and so
/// far only) persisted cross-reference, [`Layer::draws_over`], and
/// `ora::save` writes `mnc-draws-over` through
/// [`Document::live_draws_over`], which drops every id the document no
/// longer holds. Any FUTURE persisted id cross-reference owes the same
/// prune at save — the debt is per-reference, not paid once for all time.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A fresh, never-before-seen stable id.
pub fn mint_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Make sure the mint never reissues anything up to and including `seen`
/// (called with the largest id a loaded file carries).
pub(crate) fn bump_ids_past(seen: u64) {
    NEXT_ID.fetch_max(seen.saturating_add(1), Ordering::Relaxed);
}

#[derive(Clone, Debug)]
pub struct Layer {
    /// Stable identity: unique within the document, survives reorder,
    /// rename, undo/redo (snapshots clone it) and save/load (`mnc-id`).
    /// Read it with [`Layer::id`]; only the mint and the ORA loader write it.
    id: u64,
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
    /// Blend If (`crate::blendif`): `Some` = this layer only shows where the
    /// composite UNDERNEATH it has luminance inside the range, feathered at
    /// both knees. `None` — the default — shows everywhere.
    ///
    /// Unlike the layer colour and the expression preview this is NOT a
    /// display-only tint: it changes what the exported page holds, so every
    /// compositor applies it (`export::composite_size`, and the GPU through
    /// `blend2.wgsl`). Offered on non-folder layers only in v1; a folder
    /// carrying one is ignored everywhere, so a hand-edited file cannot
    /// smuggle in a behaviour the UI would never show.
    pub blend_if: Option<crate::blendif::BlendIf>,
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
    /// FB-overflow ("art bursts out of the panel"): a non-folder layer
    /// inside a sealed frame folder re-seats, for compositing only, just
    /// ABOVE its frame folder header — outside the panel mask and over the
    /// border ink — while still living inside the folder for organisation.
    /// Meaningless (and ignored) anywhere else. See `composite_order`.
    pub escape_frame: bool,
    /// FB-overflow part 2: the OTHER layers this breakout layer draws over,
    /// by STABLE ID. Empty = the shipped default, "over my own frame folder
    /// and nothing else". Paint order is a stack, so the set is always
    /// downward-closed — over a layer implies over everything below it —
    /// which is why it is only ever written through
    /// [`Document::set_layer_spill_seat`] and why the UI presents it as one
    /// insertion marker rather than N independent ticks.
    ///
    /// Ids, not indices, so the set survives reorder and delete. It is the
    /// document's ONLY persisted id cross-reference: `ora::save` prunes dead
    /// entries through [`Document::live_draws_over`] (see the mint's doc).
    /// Meaningless without `escape_frame`, and ignored there.
    pub draws_over: BTreeSet<u64>,
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
    /// Vector inking (docs/VECTOR-INKING.md): `Some` = this raster layer
    /// RECORDS its strokes as editable geometry beside the pixels. The
    /// pixels stay ordinary tiles (drawing rasterizes normally); edits
    /// re-derive by replay. Serialized as an `.ora` zip sidecar
    /// (`data/layerN.strokes.json`).
    pub strokes: Option<crate::stroke_set::StrokeSet>,
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
    /// Derived corrected-page raster + its stamps (row 105). Never
    /// serialized — rebuilt by `Document::refresh_corrections`.
    pub(crate) corr: Option<crate::correction::CorrDerived>,
}

impl Layer {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: mint_id(),
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
            blend_if: None,
            depth: 0,
            folder: false,
            through: false,
            open: true,
            clip: false,
            escape_frame: false,
            draws_over: BTreeSet::new(),
            lock: false,
            mask: None,
            mask_linked: true,
            lock_alpha: false,
            reference: false,
            draft: false,
            mask_tiles: None,
            tone: None,
            genlines: None,
            strokes: None,
            edge: None,
            tone_tiles: None,
            edge_tiles: None,
            edge_stamp: None,
            fill_tiles: None,
            fill_stamp: None,
            corr: None,
        }
    }

    /// Stable identity — see the field doc.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// ORA load / heal only: everyone else keeps the minted id.
    pub(crate) fn set_id(&mut self, id: u64) {
        self.id = id;
    }

    /// The mask that CAPS this layer's spill, if it has one: an enabled
    /// layer mask on a layer that is bursting out of its panel. `Some` is
    /// exactly the condition that splits the layer into two composite steps
    /// ([`SpillPart`]) — the mask stops scaling alpha and starts naming the
    /// region allowed out. `None` = spill on every side, the shipped
    /// all-or-nothing behaviour.
    ///
    /// It does NOT check the enclosing frame folder: only `composite_order`
    /// knows whether the escape is real, and it asks this after deciding.
    pub fn breakout_mask(&self) -> Option<&LayerMask> {
        if !self.escape_frame || self.folder {
            return None;
        }
        self.mask.as_ref().filter(|m| m.enabled)
    }

    /// The LIVE Blend If gate, or `None` when this layer shows everywhere.
    ///
    /// **The single door every compositor asks** — CPU (`export`), GPU
    /// (`LayerSig` + the blend2 routing) and any future one. It folds in the
    /// two cases that are "off" without being `None`:
    ///
    /// * a FOLDER carrying a gate (v1 offers it on painted layers only, and
    ///   a hand-edited or future file must not sneak one in — the same
    ///   defence `edge` gets on frame folders at load), and
    /// * an OPEN range, which passes every luminance and would otherwise
    ///   cost the GPU a whole destination-snapshot pass for a no-op.
    ///
    /// If the two compositors asked this question separately they would
    /// eventually answer it differently, and the screen would stop matching
    /// the exported page.
    pub fn gate(&self) -> Option<crate::blendif::BlendIf> {
        self.blend_if
            .filter(|_| !self.folder)
            .map(|b| b.normalized())
            .filter(|b| !b.is_open())
    }

    /// The frame folder's derived coverage mask, if any.
    pub fn mask_tiles(&self) -> Option<&HashMap<TileIdx, Arc<Tile>>> {
        self.mask_tiles.as_ref()
    }

    /// The border effect's derived raster, if any. On a plain FOLDER this
    /// is the FB-knockout mat (mat only, no source baked in) that the
    /// compositors lay just beneath the group at its close.
    pub fn edge_tiles(&self) -> Option<&HashMap<TileIdx, Arc<Tile>>> {
        self.edge_tiles.as_ref()
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
        } else if matches!(self.kind, LayerKind::Correction(_)) {
            self.corr_tiles()
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
        } else if let LayerKind::Correction(_) = self.kind {
            self.corr_tiles()?.get(&idx)
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
        let (cw, chh) = (size.0.div_ceil(tsu) as i32, size.1.div_ceil(tsu) as i32);
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
            // LP-004: the watercolour rim samples the window's own ink
            // colours — premultiplied fix15, zero where nothing inked.
            // Filled beside the seed below; empty for the solid style,
            // which never reads it.
            let want_cwin = p.style == crate::edge::EdgeStyle::Watercolour;
            let mut cwin = vec![0u16; side * side * 4];
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
                if want_cwin {
                    cwin.fill(0);
                }
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
                                let w = (y - py0) as usize * side + (x - px0) as usize;
                                seed[w] = 0.0;
                                if want_cwin {
                                    let cw = &mut cwin[w * 4..w * 4 + 4];
                                    cw.copy_from_slice(&d[o..o + 4]);
                                }
                            }
                        }
                    }
                }
                let t = crate::edge::derive_tile(
                    &mut seed,
                    r,
                    base.get(&idx).map(|a| &**a),
                    p,
                    &cwin,
                );
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

    /// The external-file reference, if this layer is a file object
    /// (row 166). `None` for everything else.
    pub fn file_object(&self) -> Option<&crate::file_object::FileObject> {
        match &self.kind {
            LayerKind::FileObject(fo) => Some(fo),
            _ => None,
        }
    }

    /// The layer's own pixels, moved out. Only the file-object re-derive
    /// uses this (it builds the new raster in a throwaway layer and swaps
    /// it in through [`Self::replace_tiles`]).
    pub(crate) fn take_tiles(&mut self) -> HashMap<TileIdx, Arc<Tile>> {
        std::mem::take(&mut self.tiles)
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

    /// A stroke-recording layer (vector inking): `LayerKind::Raster` with a
    /// stroke set beside the pixels, so `is_vector()` is FALSE for it and
    /// every guard written as `is_vector() || folder` used to let raster ops
    /// straight through. Its raster is re-derived from the record at the
    /// next control-point nudge — anything else that wrote tiles is zeroed
    /// there without a word.
    pub fn records_strokes(&self) -> bool {
        self.strokes.is_some()
    }

    /// Takes a raster EDIT? Folders organise, vector layers derive, and a
    /// stroke-recording layer replays — none of the three keeps pixels that
    /// arrive any other way. (Inking itself does not ask: a pen stroke on a
    /// recording layer is captured, so it survives the replay. This is the
    /// guard for everything else — fill, gradient, transform, filter,
    /// correction, cut, clear.)
    pub fn paintable(&self) -> bool {
        !self.folder && !self.is_vector() && !self.records_strokes()
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
        let remap =
            |map: &HashMap<TileIdx, Arc<Tile>>, keep_zero: bool| -> HashMap<TileIdx, Arc<Tile>> {
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

    /// Tight bounding box of the INK in **canvas pixels** — `[x0, y0, x1, y1]`,
    /// far edges exclusive — every pixel with any alpha at all. `None` when
    /// nothing is painted. This is the box a transform float hugs: CSP's
    /// bounding box sits on the drawing, and a box on the TILE grid
    /// (`tile_bounds`) put the handles up to 63 px off the art and the
    /// centre of rotation — and of every standalone Flip — off its centre.
    pub fn ink_bounds(&self) -> Option<[i32; 4]> {
        let mut b = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for (ti, t) in self.tiles() {
            if t.is_blank() {
                continue;
            }
            let (ox, oy) = ti.origin();
            for py in 0..TILE_SIZE {
                for px in 0..TILE_SIZE {
                    if t.pixel(px, py)[3] > 0 {
                        b[0] = b[0].min(ox + px as i32);
                        b[1] = b[1].min(oy + py as i32);
                        b[2] = b[2].max(ox + px as i32 + 1);
                        b[3] = b[3].max(oy + py as i32 + 1);
                    }
                }
            }
        }
        (b[0] < b[2]).then_some(b)
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
        self.corr = None;

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
            // A correction has no geometry of its own; its window mask
            // shifted with the layer above.
            LayerKind::Correction(_) => {}
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
            // Row 166: a file object's geometry IS its raster (it shifted
            // with the tiles above). The path and the fit box are not
            // positions, so there is nothing here to move — and the shifted
            // pixels are what the next refresh will overwrite, which is the
            // documented v1 cut: a moved file object snaps back to centred
            // when its source changes.
            LayerKind::FileObject(_) => {}
            LayerKind::Raster => {}
        }
    }

    /// Scale every PIXEL-space number the layer owns by `(sx, sy)` — the
    /// geometry half of `IO-060` (Edit ▸ Change work resolution), and the
    /// exact counterpart of [`Self::translate_vectors`].
    ///
    /// # What scales and what deliberately does not
    ///
    /// Anything stored in canvas px scales: positions, radii, corner radii,
    /// stroke widths, the tone lattice ORIGIN, a balloon's screen cell
    /// (`BalloonTone::cell_px` is px by design, not LPI).
    ///
    /// Anything stored PHYSICALLY does not, because the whole point of the
    /// op is that the paper stays the same size:
    /// * `ToneParams::lpi` — lines per INCH. 60 lpi is still 60 lpi at 350
    ///   dpi; the cell is `dpi / lpi`, so the screen re-flows by itself the
    ///   moment `refresh_derived` runs at the new dpi. Scaling it here would
    ///   change the printed screen, which is the bug this op exists to avoid.
    /// * `TextItem::size_pt` and every other `*_pt` — a point is 1/72 inch.
    ///   Same argument: 12 pt prints 12 pt at either dpi. The BOX around the
    ///   type is px and does scale, and the shaped sprite cache is dropped so
    ///   the next shape pass rebuilds it at the new dpi.
    /// * Angles, opacities, pressure widths (0..1), `Tail::bend` (a fraction
    ///   of the tail's own length) — all dimensionless.
    ///
    /// `s` is the width scale for numbers that are not tied to one axis;
    /// callers pass the mean, and on a dpi change `sx == sy` anyway.
    fn scale_vectors(&mut self, sx: f32, sy: f32, s: f32) {
        if let Some(t) = &mut self.tone {
            // LP-014's lattice origin is canvas px; lpi is physical.
            t.offset[0] *= sx;
            t.offset[1] *= sy;
        }
        if let Some(e) = &mut self.edge {
            e.width_px *= s;
        }
        if let Some(g) = &mut self.genlines {
            g.scale(sx, sy, s);
        }
        if let Some(st) = &mut self.strokes {
            st.scale(sx, sy, s);
        }
        match &mut self.kind {
            LayerKind::Fill(crate::fill_layer::FillKind::Gradient { a, b, .. }) => {
                a[0] *= sx;
                a[1] *= sy;
                b[0] *= sx;
                b[1] *= sy;
            }
            LayerKind::Fill(crate::fill_layer::FillKind::Tone { tone, .. }) => {
                tone.offset[0] *= sx;
                tone.offset[1] *= sy;
            }
            LayerKind::Fill(_) => {}
            LayerKind::Correction(_) => {}
            LayerKind::Frame(fs) => {
                for f in &mut fs.frames {
                    for p in &mut f.points {
                        p[0] *= sx;
                        p[1] *= sy;
                    }
                }
                fs.border_px *= s;
                if let Some(sl) = &mut fs.slot {
                    sl[0] *= sx;
                    sl[1] *= sy;
                    sl[2] *= sx;
                    sl[3] *= sy;
                }
            }
            LayerKind::Balloon(bs) => {
                use crate::balloon::BalloonShape;
                for b in &mut bs.balloons {
                    match &mut b.shape {
                        BalloonShape::Ellipse { center, radii } => {
                            center[0] *= sx;
                            center[1] *= sy;
                            radii[0] *= sx;
                            radii[1] *= sy;
                        }
                        BalloonShape::RoundRect { rect, corner } => {
                            rect[0] *= sx;
                            rect[1] *= sy;
                            rect[2] *= sx;
                            rect[3] *= sy;
                            *corner *= s;
                        }
                        BalloonShape::Polygon { points, .. } => {
                            for p in points.iter_mut() {
                                p[0] *= sx;
                                p[1] *= sy;
                            }
                        }
                    }
                    for tail in &mut b.tails {
                        tail.base[0] *= sx;
                        tail.base[1] *= sy;
                        tail.tip[0] *= sx;
                        tail.tip[1] *= sy;
                        tail.width *= s;
                    }
                    if let Some(t) = &mut b.fill_tone {
                        // Stored in canvas px (balloon.rs says so out loud),
                        // so unlike a tone LAYER it will not re-flow itself.
                        t.cell_px *= s;
                    }
                }
                bs.border_px *= s;
            }
            LayerKind::Text(ts) => {
                for item in &mut ts.texts {
                    item.pos[0] *= sx;
                    item.pos[1] *= sy;
                    item.size[0] *= sx;
                    item.size[1] *= sy;
                    item.outline_px *= s;
                    // pt sizes are physical and stay; the SPRITE was shaped
                    // at the old dpi, so it must be re-shaped, not scaled.
                    item.cache = None;
                }
            }
            // A file object's geometry is its raster (resampled with every
            // other tile map). `fit` is the box the source was scaled into,
            // in canvas px, so it moves with the canvas or the next refresh
            // re-derives at the OLD pixel size.
            LayerKind::FileObject(fo) => {
                fo.fit = (
                    ((fo.fit.0 as f32 * sx).round() as u32).max(1),
                    ((fo.fit.1 as f32 * sy).round() as u32).max(1),
                );
            }
            LayerKind::Raster => {}
        }
    }

    /// Resample every raster this layer owns and scale its geometry —
    /// `IO-060`'s per-layer half. Derived rasters are DROPPED rather than
    /// resampled: a tone screen, a border effect, a live fill and a frame
    /// mat all re-derive from sources that just scaled, and re-deriving is
    /// both crisper and cheaper than filtering a lattice (the moiré the
    /// runner-up 13 export choice is also about).
    fn resample_content(&mut self, sx: f32, sy: f32, interp: crate::transform::Interp) {
        self.tiles = crate::transform::resample_tile_map(&self.tiles, sx, sy, interp);
        self.resample_meta(sx, sy, interp);
    }

    /// Everything [`Self::resample_content`] does EXCEPT the layer's own
    /// tiles — the mask, the derived caches, the geometry. Split out for
    /// the paper case: a canvas-filling sheet of uniform white is re-laid
    /// rather than resampled (a full page of solid white through the box
    /// filter buys only a half-alpha fringe at the edges, at the cost of
    /// the most expensive resample on the page), but it still owns a mask
    /// and caches that have to move with everything else.
    fn resample_meta(&mut self, sx: f32, sy: f32, interp: crate::transform::Interp) {
        let s = 0.5 * (sx + sy);
        if let Some(m) = &mut self.mask {
            m.tiles = crate::transform::resample_tile_map(&m.tiles, sx, sy, interp);
            m.revision = crate::tile::next_revision();
        }
        self.mask_tiles = None;
        self.tone_tiles = None;
        self.edge_tiles = None;
        self.edge_stamp = None;
        self.fill_tiles = None;
        self.fill_stamp = None;
        self.corr = None;
        self.scale_vectors(sx, sy, s);
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
    /// Palette multi-selection (TC-013): rows selected BESIDE `active`,
    /// sorted, never containing `active` itself. Session-only, like the
    /// rulers. Cleared by `clear_history` — the structural ops that could
    /// let these indices go stale are exactly the ones that clear the
    /// history, so the one door covers both (the `Compound` safety
    /// argument, reused).
    pub layer_multi: Vec<usize>,
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
    /// The LM-004 bracket's pre-image: the mask as it stood at stroke start
    /// (`None` = there was none) beside its revision then (`None` likewise).
    /// Comparing the REVISION OPTION at `end` is what makes "no mask before,
    /// no mask after" a non-event and "no mask before, a window now" a step.
    mask_op_snapshot: Option<(Option<LayerMask>, Option<u64>)>,
    /// CV-003: the NEXT op's History-palette label, set by the caller
    /// between `begin_op` and `end_op` ("Stroke", "Fill", …). Consumed
    /// by `end_op`; unset = "Edit".
    pub pending_op_label: Option<String>,
    /// LC-001 layer comps (TRIAGE 139): named whole-stack presentation
    /// snapshots (eyes, opacity, blend, layer colour — see [`LayerComp`]).
    /// Positional — comp.vis[i] maps to layers[i] — because the comp's
    /// daily use (text/no-text chapter versions) has IDENTICAL structure
    /// on every page; a length mismatch refuses on apply rather than
    /// guessing. Persisted as `mnc-comps` on the ORA image element.
    pub comps: Vec<LayerComp>,
    /// TX-styles: the work's named text styles (dialogue / thought / …).
    /// Seeded with JP-convention defaults on a new document; persists as
    /// one `mnc-textstyles` attr on the ORA image element (mnc-comps'
    /// pattern). Items reference styles by NAME (`TextItem::style`).
    pub text_styles: Vec<crate::text::TextStyle>,
    /// PA-001: the paper under the stack. Drive it with `set_paper_colour`
    /// (undoable) / `set_paper_visible` (view state, like a layer's eye).
    pub paper: Paper,
    /// The ruler set (TODO #3). Persists as its own `mnc/rulers.json` zip
    /// entry (skipped when empty, so ruler-less files keep their old
    /// bytes) — perspective grids used to die with the session. A page
    /// decoded WITHOUT its own set still inherits the tab's working set
    /// (`App::adopt_page_doc`), so rulers keep following the artist onto
    /// fresh pages.
    ///
    /// It lives on the Document rather than the App so the document's ONE
    /// undo history can own ruler edits ([`UndoGroup::Rulers`]) — an
    /// app-level undo species running beside it would let a Ctrl+Z mean two
    /// different things depending on what you touched last.
    pub rulers: crate::ruler::Rulers,
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

/// One layer comp (TRIAGE 139, LC-001): a named snapshot of the layer
/// PRESENTATION state — the eyes, plus opacity, blend and the LP-016/017
/// layer colour. Every field is positional: index `i` is `layers[i]`.
///
/// **Persisted format (`mnc-comps`, serde JSON on the ORA image element).**
/// `vis` is the v1 field and is always written. Every property added since
/// rides an `Option<Vec<_>>` that is omitted when absent, and absent means
/// *this comp does not touch that property* — never *reset it to the
/// default*. A comp the owner saved before this round records only eyes,
/// and applying it must still change only eyes; a plain `#[serde(default)]`
/// `Vec` would read back empty, which `apply_to` cannot tell from "recorded
/// nothing" — and the failure is silent, every layer's opacity snapping to
/// 1.0 on a comp that never claimed to own opacity. New captures always
/// fill all of them, so an omission only ever describes an old file, and an
/// old build reading a new file still finds the `vis` it knows.
/// Blend rides its ORA `composite-op` name (`Blend::ora_name`) rather than a
/// second serde spelling of the enum: that mapping is already the file
/// format's word and is round-trip tested, so the picker's variant order
/// stays free to move.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LayerComp {
    pub name: String,
    pub vis: Vec<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<Vec<String>>,
    /// LP-016 layer colour (the MAIN end of the two-tone ramp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<Vec<Option<[u8; 3]>>>,
    /// LP-017 SUB colour — the other end of the same ramp, so it travels
    /// with the main one or the tint restores half-set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_colour: Option<Vec<Option<[u8; 3]>>>,
}

impl LayerComp {
    /// Snapshot the stack under `name`. A new capture records EVERY
    /// property (see the format note) — the `Option`s exist for files, not
    /// for capture-time choices.
    pub fn capture(name: &str, layers: &[Layer]) -> Self {
        Self {
            name: name.to_string(),
            vis: layers.iter().map(|l| l.visible).collect(),
            opacity: Some(layers.iter().map(|l| l.opacity).collect()),
            blend: Some(
                layers
                    .iter()
                    .map(|l| l.blend.ora_name().to_string())
                    .collect(),
            ),
            colour: Some(layers.iter().map(|l| l.layer_colour).collect()),
            sub_colour: Some(layers.iter().map(|l| l.layer_sub_colour).collect()),
        }
    }

    /// Push the snapshot back onto `layers`. `added_visible` is LC-006's
    /// default eye for a layer added AFTER the snapshot; `None` leaves such
    /// a layer alone (LC-003's restore, where the pre-application state is
    /// the truth and there is nothing to default). No other property has a
    /// default — a layer past the recorded end keeps what it has, the same
    /// rule as a property this comp never recorded.
    pub fn apply_to(&self, layers: &mut [Layer], added_visible: Option<bool>) {
        for (li, l) in layers.iter_mut().enumerate() {
            match (self.vis.get(li), added_visible) {
                (Some(v), _) => l.visible = *v,
                (None, Some(v)) => l.visible = v,
                (None, None) => {}
            }
            if let Some(o) = self.opacity.as_ref().and_then(|v| v.get(li)) {
                l.opacity = *o;
            }
            if let Some(b) = self.blend.as_ref().and_then(|v| v.get(li)) {
                l.blend = Blend::from_ora_name(b);
            }
            if let Some(c) = self.colour.as_ref().and_then(|v| v.get(li)) {
                l.layer_colour = *c;
            }
            if let Some(c) = self.sub_colour.as_ref().and_then(|v| v.get(li)) {
                l.layer_sub_colour = *c;
            }
        }
    }
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
            layer_multi: Vec::new(),
            size: (width, height),
            revision: next_revision(),
            selection: None,
            sel_scratch: LayerMask {
                tiles: HashMap::new(),
                enabled: true,
                revision: 0,
                full: false,
            },
            history: History::new(),
            op_layer: None,
            mask_op_snapshot: None,
            pending_op_label: None,
            comps: Vec::new(),
            text_styles: crate::text::TextStyle::defaults(),
            paper: Paper::default(),
            rulers: crate::ruler::Rulers::default(),
        }
    }

    /// Record one ruler gesture as ONE undo step: `before` is the whole set
    /// as it was when the gesture began, and the live `self.rulers` is
    /// already in its finished state. A no-op gesture pushes nothing.
    ///
    /// DOES `touch()` since rulers persist (`mnc/rulers.json`): a ruler
    /// edit must move `revision` or page stashing would skip the page and
    /// the edit would never reach disk.
    pub fn record_rulers(&mut self, before: crate::ruler::Rulers, label: &str) -> bool {
        if before == self.rulers {
            return false;
        }
        self.history
            .push_labeled(label, UndoGroup::Rulers { rulers: before });
        self.touch();
        true
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
    ///
    /// The snapshot records the mask's ABSENCE too (`None`), which is what
    /// lets [`Self::arm_full_window`] run inside the bracket and be undone
    /// with the stroke it armed for. Nothing is pushed when the state at
    /// `end` matches the state at `begin` — including maskless-to-maskless,
    /// the H1 backstop's case, which spent no undo step before and does not
    /// now.
    pub fn mask_op_begin(&mut self) {
        let m = self.active_layer().mask.as_ref();
        self.mask_op_snapshot = Some((m.cloned(), m.map(|m| m.revision)));
    }

    /// Row 105: ARM an all-visible window on the active layer, for the
    /// stroke that is about to start. A maskless correction layer refused
    /// brush strokes the way a maskless live fill does — but "erase the
    /// correction off the face" is what a CSP user reaches for first, and
    /// there is nothing to refuse: the window a correction wants here is
    /// the whole page, and [`LayerMask::full_window`] is that in an empty
    /// map. Call INSIDE the [`Self::mask_op_begin`] bracket; the pre-image
    /// is then "no mask at all" and one undo takes the window and the
    /// stroke together.
    ///
    /// Returns false when a mask is already there (nothing to arm).
    pub fn arm_full_window(&mut self) -> bool {
        let li = self.active;
        let Some(l) = self.layers.get_mut(li) else {
            return false;
        };
        if l.mask.is_some() {
            return false;
        }
        l.mask = Some(LayerMask::full_window());
        self.touch();
        true
    }

    /// Returns true when a group was pushed.
    pub fn mask_op_end(&mut self) -> bool {
        let Some((before, rev0)) = self.mask_op_snapshot.take() else {
            return false;
        };
        if self.active_layer().mask.as_ref().map(|m| m.revision) == rev0 {
            return false;
        }
        let li = self.active;
        // An armed window the stroke never wrote into: the arm was
        // speculative (the pen came down and went up again), so it is taken
        // back rather than spending the undo step an empty stroke has never
        // spent. An armed window that DID take a dab keeps every tile it
        // materialised.
        if before.is_none()
            && self.layers[li]
                .mask
                .as_ref()
                .is_some_and(|m| m.full && m.tiles.is_empty())
        {
            self.layers[li].mask = None;
            return false;
        }
        let label = if before.is_none() {
            "Correction window"
        } else {
            "Mask stroke"
        };
        self.push_mask_group(li, before, label);
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
                // An ABSENT mask tile is VISIBLE — that is what both
                // compositors render (export.rs LM-005, gpu/lib.rs), and
                // mask tiles only exist where the layer had ink when the
                // mask was made. `unwrap_or(0)` here silently DELETED any
                // ink painted after that: shown on screen, erased by the
                // bake. Bake must match the screen.
                let m = cov.map(|c| c.data()[p * 4 + 3] as u32).unwrap_or(32768);
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
        // NOT `paintable()`: a mask lives beside the tiles and the stroke
        // replay never touches it, so a stroke-recording layer masks like
        // any other raster one. Only derived rasters and folders refuse.
        if l.is_vector() || l.lock || l.folder {
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
            // LM-001/002 cut a window over the layer's own inked tiles.
            full: false,
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
        // A FULL window cannot express "all hidden" by zeroing what it
        // holds — the tiles it does NOT hold are the visible part, and there
        // is no dense form of them worth 30 MB. Clearing one drops the flag
        // instead: an empty CARVED window is a window that reaches nothing,
        // which is the same page and the same command's meaning.
        if m.full {
            m.full = false;
            m.tiles.clear();
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
        let Some((li, label, tiles)) = self.take_op() else {
            return false;
        };
        self.history
            .push_labeled(&label, UndoGroup::Tiles { layer: li, tiles });
        self.touch();
        true
    }

    /// Close the open op and hand back its `Tiles` group WITHOUT pushing:
    /// a multi-layer loop (`apply_adjust_many`) collects one per layer and
    /// pushes them as ONE step via [`Self::push_compound`]. `None` when no
    /// op was open or nothing was touched.
    pub fn end_op_take(&mut self) -> Option<UndoGroup> {
        let (li, _label, tiles) = self.take_op()?;
        Some(UndoGroup::Tiles { layer: li, tiles })
    }

    /// Push already-collected groups as ONE labelled undo step. A single
    /// member skips the `Compound` wrapper (identical history to the op
    /// having pushed itself). Index drift cannot bite for the same reason
    /// `set_tone_many` is safe: every index-shifting op clears the history.
    pub fn push_compound(&mut self, label: &str, mut members: Vec<UndoGroup>) -> bool {
        match members.len() {
            0 => return false,
            1 => self.history.push_labeled(label, members.pop().unwrap()),
            _ => self
                .history
                .push_labeled(label, UndoGroup::Compound(members)),
        }
        self.touch();
        true
    }

    /// Bundle the newest `n` history steps into ONE labelled step
    /// (recordable action runs, whole-work reflows). Members are collected
    /// newest-first, which IS the swap order `Compound` needs (undo unwinds
    /// the run backwards; the reversed inverse replays it forward on redo).
    /// Structural members are fine: each is a Structure swap, and the LIFO
    /// argument on `UndoGroup`'s doc comment holds inside a Compound too.
    pub fn wrap_recent(&mut self, label: &str, n: usize) -> bool {
        if n == 0 {
            return false;
        }
        let mut members = Vec::with_capacity(n);
        for _ in 0..n {
            match self.history.pop_undo() {
                Some(g) => members.push(g),
                None => break,
            }
        }
        if members.is_empty() {
            return false;
        }
        // `push_compound` unwraps a single member — a one-step run reads
        // in the History palette exactly like the step itself would.
        self.push_compound(label, members)
    }

    /// The layer stack as an undo pre-image: `Arc`-cheap clones with any
    /// open op's recording scrubbed (a snapshot must never inherit one —
    /// the same rule `duplicate_layer` follows). Taken at the TOP of every
    /// structural op, before the first mutation.
    ///
    /// Public because the app records structure too: a live-layer parameter
    /// edit and the correction dialog both snapshot the stack before they
    /// touch it, and both need the same scrub.
    pub fn stack_snapshot(&self) -> Vec<Layer> {
        let mut v = self.layers.clone();
        for l in &mut v {
            l.recording = None;
        }
        v
    }

    /// Record a structural op (add/remove/duplicate/move/merge/divide/
    /// combine) as ONE undoable step: the caller took [`Self::stack_snapshot`]
    /// and noted `active` BEFORE mutating, and calls this on the success
    /// path. This replaced the old clear-the-history model (2026-08-21):
    /// undo is LIFO, so an index-carrying group deeper in the stack is only
    /// ever swapped once the Structure swaps above it have restored the
    /// exact stack it was recorded against — indices cannot go stale as
    /// long as every structural change records one of these.
    pub fn record_structure(&mut self, label: &str, before: Vec<Layer>, active_before: usize) {
        self.cancel_op();
        // The palette multi-selection is index-keyed; a structural shift is
        // exactly what it must not survive. Cheap to rebuild, so clearing
        // stays the safe move even though the history now survives.
        self.layer_multi.clear();
        self.history.push_labeled(
            label,
            UndoGroup::Structure {
                layers: before,
                active: active_before,
            },
        );
    }

    /// Recordable actions: make a whole replayed run ONE undo press. The
    /// caller cloned `layers` and noted `active` BEFORE replaying; whatever
    /// the run pushed or cleared since is superseded by the snapshot pair
    /// (pre-run stack in the group, post-run stack live), so the history is
    /// cleared first and this group lands alone.
    pub fn push_structure(&mut self, label: &str, before: Vec<Layer>, active_before: usize) {
        self.clear_history();
        self.history.push_labeled(
            label,
            UndoGroup::Structure {
                layers: before,
                active: active_before,
            },
        );
        self.touch();
    }

    /// True when `g` is, or contains, a [`UndoGroup::Structure`] — a
    /// Compound from a recorded action run can carry one in its belly, and
    /// the cache-invalidation door below must see through the wrapper.
    fn group_is_structural(g: &UndoGroup) -> bool {
        match g {
            UndoGroup::Structure { .. } => true,
            UndoGroup::Compound(members) => members.iter().any(Self::group_is_structural),
            _ => false,
        }
    }

    /// True when the next undo would move a [`UndoGroup::Structure`]
    /// (possibly inside a Compound) — the app must fully invalidate the GPU
    /// tile cache around that swap (restored tiles keep their old, lower
    /// revisions; the cache uploads only on newer).
    pub fn next_undo_is_structure(&self) -> bool {
        self.history
            .peek_undo()
            .is_some_and(Self::group_is_structural)
    }

    /// Same door, redo side.
    pub fn next_redo_is_structure(&self) -> bool {
        self.history
            .peek_redo()
            .is_some_and(Self::group_is_structural)
    }

    /// Vector inking (docs/VECTOR-INKING.md): close the open op as ONE
    /// stroke group — the tile pre-images AND the recorded geometry, so a
    /// single undo takes back both. An empty gesture spends nothing; a
    /// layer that turns out not to record degrades to plain [`Self::end_op`]
    /// semantics.
    pub fn end_op_vector_stroke(&mut self, stroke: crate::stroke_set::VectorStroke) -> bool {
        let Some((li, label, tiles)) = self.take_op() else {
            return false;
        };
        match self.layers.get_mut(li).and_then(|l| l.strokes.as_mut()) {
            Some(set) => set.strokes.push(stroke.clone()),
            None => {
                self.history
                    .push_labeled(&label, UndoGroup::Tiles { layer: li, tiles });
                self.touch();
                return true;
            }
        }
        self.history.push_labeled(
            &label,
            UndoGroup::VectorStroke {
                layer: li,
                tiles,
                stroke: Box::new(stroke),
                present: true,
            },
        );
        self.touch();
        true
    }

    /// Vector inking phase 3: close the open op as ONE set-restructuring
    /// group (trim eraser, stroke delete) — the re-derived tiles' pre-images
    /// plus the whole set as it was BEFORE.
    pub fn end_op_vector_set(&mut self, before: crate::stroke_set::StrokeSet, label: &str) -> bool {
        let Some((li, _label, tiles)) = self.take_op() else {
            return false;
        };
        self.history.push_labeled(
            label,
            UndoGroup::VectorSet {
                layer: li,
                tiles,
                strokes: before,
            },
        );
        self.touch();
        true
    }

    /// Vector inking phase 2: close the open op as ONE stroke-edit group —
    /// the re-derived tiles' pre-images plus the stroke as it was BEFORE
    /// the edit (`strokes[index]` must already hold the edited version).
    pub fn end_op_vector_edit(
        &mut self,
        index: usize,
        before: crate::stroke_set::VectorStroke,
        label: &str,
    ) -> bool {
        let Some((li, _label, tiles)) = self.take_op() else {
            return false;
        };
        self.history.push_labeled(
            label,
            UndoGroup::VectorEdit {
                layer: li,
                tiles,
                index,
                stroke: Box::new(before),
            },
        );
        self.touch();
        true
    }

    /// Close the open op and hand back its layer, label and sorted
    /// pre-images WITHOUT pushing a group — the shared half of `end_op`,
    /// for the ops that wrap the same recording in a richer group (the
    /// effect-line regen, whose spec rides the pixels). `None` when no op
    /// was open or nothing was touched.
    #[allow(clippy::type_complexity)]
    fn take_op(&mut self) -> Option<(usize, String, Vec<(TileIdx, Option<Arc<Tile>>)>)> {
        let li = self.op_layer.take()?;
        let rec = self.layers.get_mut(li).and_then(Layer::take_recording)?;
        if rec.is_empty() {
            return None;
        }
        let mut tiles: Vec<(TileIdx, Option<Arc<Tile>>)> = rec.into_iter().collect();
        // HashMap order is not deterministic; groups are compared in tests and
        // replayed in order, so sort them.
        tiles.sort_by_key(|(idx, _)| (idx.y, idx.x));
        let label = self
            .pending_op_label
            .take()
            .unwrap_or_else(|| "Edit".into());
        Some((li, label, tiles))
    }

    /// Close the open op by RESTORING every pre-image — as if the op never
    /// happened, and no step is spent. (The vector eraser that touched no
    /// stroke reverts its live raster erase this way.)
    pub fn abort_op_restore(&mut self) {
        if let Some((li, _label, tiles)) = self.take_op()
            && let Some(l) = self.layers.get_mut(li)
        {
            for (idx, snap) in tiles {
                l.set_tile(idx, snap);
            }
        }
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
            UndoGroup::GenLines { layer, spec, tiles } => {
                let l = self.layers.get_mut(layer)?;
                // Parameters and pixels swap together — see the group's
                // doc comment. Same tile door as `Tiles`, so restored tiles
                // get fresh revisions and the compositor re-uploads them.
                // The layer always has one here (only generated layers
                // regenerate); the fallback keeps the group's own spec
                // rather than inventing state if that ever changes.
                let spec_before = l.genlines.unwrap_or(spec);
                l.genlines = Some(spec);
                let mut inverse = Vec::with_capacity(tiles.len());
                for (idx, snapshot) in tiles {
                    inverse.push((idx, l.tile_arc(idx).cloned()));
                    l.set_tile(idx, snapshot);
                }
                Some(UndoGroup::GenLines {
                    layer,
                    spec: spec_before,
                    tiles: inverse,
                })
            }
            UndoGroup::VectorStroke {
                layer,
                tiles,
                stroke,
                present,
            } => {
                let l = self.layers.get_mut(layer)?;
                // Pixels and record swap together — the group's doc comment.
                // Same tile door as `Tiles` (fresh revisions, compositor
                // re-uploads).
                let mut inverse = Vec::with_capacity(tiles.len());
                for (idx, snapshot) in tiles {
                    inverse.push((idx, l.tile_arc(idx).cloned()));
                    l.set_tile(idx, snapshot);
                }
                if let Some(set) = &mut l.strokes {
                    if present {
                        // The recorded stroke is the set's newest; take it
                        // back with the ink.
                        set.strokes.pop();
                    } else {
                        set.strokes.push((*stroke).clone());
                    }
                }
                Some(UndoGroup::VectorStroke {
                    layer,
                    tiles: inverse,
                    stroke,
                    present: !present,
                })
            }
            UndoGroup::VectorEdit {
                layer,
                tiles,
                index,
                mut stroke,
            } => {
                let l = self.layers.get_mut(layer)?;
                let mut inverse = Vec::with_capacity(tiles.len());
                for (idx, snapshot) in tiles {
                    inverse.push((idx, l.tile_arc(idx).cloned()));
                    l.set_tile(idx, snapshot);
                }
                if let Some(s) = l
                    .strokes
                    .as_mut()
                    .and_then(|set| set.strokes.get_mut(index))
                {
                    std::mem::swap(s, &mut stroke);
                }
                Some(UndoGroup::VectorEdit {
                    layer,
                    tiles: inverse,
                    index,
                    stroke,
                })
            }
            UndoGroup::VectorSet {
                layer,
                tiles,
                strokes,
            } => {
                let l = self.layers.get_mut(layer)?;
                let mut inverse = Vec::with_capacity(tiles.len());
                for (idx, snapshot) in tiles {
                    inverse.push((idx, l.tile_arc(idx).cloned()));
                    l.set_tile(idx, snapshot);
                }
                let strokes_before = match &mut l.strokes {
                    Some(set) => std::mem::replace(set, strokes),
                    None => strokes,
                };
                Some(UndoGroup::VectorSet {
                    layer,
                    tiles: inverse,
                    strokes: strokes_before,
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
            UndoGroup::Rulers { rulers } => {
                let inverse = UndoGroup::Rulers {
                    rulers: self.rulers.clone(),
                };
                self.rulers = rulers;
                Some(inverse)
            }
            UndoGroup::Compound(groups) => {
                // Members swap in order; the inverse carries them REVERSED
                // so redo replays forward. Members cannot fail here for the
                // same reason any group's layer lookup cannot: a structure
                // change would have cleared the history.
                let mut inverses = Vec::with_capacity(groups.len());
                for g in groups {
                    inverses.push(self.swap_group(g)?);
                }
                inverses.reverse();
                Some(UndoGroup::Compound(inverses))
            }
            UndoGroup::Structure { mut layers, active } => {
                // Wholesale stack swap. NO tile revisions are stamped (see
                // the variant's doc comment — the app invalidates the GPU
                // cache when this group moves).
                std::mem::swap(&mut self.layers, &mut layers);
                let active_before = self.active;
                self.active = active.min(self.layers.len().saturating_sub(1));
                // The multi-selection is index-keyed; a stack swap is
                // exactly the shift it must not survive.
                self.layer_multi.clear();
                Some(UndoGroup::Structure {
                    layers,
                    active: active_before,
                })
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// The label of the step the next undo would take (`None` when there
    /// is nothing to undo) — the leak-repair arm's "is the fill still the
    /// newest step?" check.
    pub fn peek_undo_label(&self) -> Option<&str> {
        self.history.peek_undo_label()
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

    /// Throw the history away (file load, or a change undo cannot express —
    /// today that means `resize_to`; the other structural ops record a
    /// [`UndoGroup::Structure`] instead, see `record_structure`).
    pub fn clear_history(&mut self) {
        self.cancel_op();
        // The palette multi-selection is index-keyed; everything that
        // shifts indices clears it (see `layer_multi`'s note).
        self.layer_multi.clear();
        self.history.clear();
        // PR-041: a change that comes through here pushes no group, so
        // counting only pushes would leave exactly the unrecoverable
        // changes uncounted. `clear` deliberately does not reset the tally.
        self.history.note_op();
    }

    // ---------------------------------------------------------- layer ops --
    //
    // Order convention: `layers[0]` is the **bottom** layer, composited first;
    // the last element is the top. (ORA's stack.xml is the other way round —
    // `core::ora` reverses on the way in and out.)
    //
    // Any op that shifts layer indices records a `UndoGroup::Structure`
    // snapshot through `record_structure` (pattern: take `stack_snapshot()`
    // + `active` BEFORE the first mutation, record on the success path).
    // Index-carrying groups deeper in the stack stay valid because undo is
    // LIFO — see the enum's doc comment. The one exception is `resize_to`,
    // which still clears (canvas size is outside the snapshot).

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
    /// the nearest layer below at the same depth that is not itself clipped —
    /// a plain layer, or a FOLDER header (clip-to-folder, scenario 2a: the
    /// group's combined ink is the base). A THROUGH folder has no isolated
    /// composite to clip to, so it breaks the chain like no base at all.
    /// `None` = not clipped (or no valid base — the flag is then ignored,
    /// CSP-style).
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
                if b.depth != l.depth {
                    break;
                }
                if b.folder {
                    if !b.through {
                        out[i] = Some(j);
                    }
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
    /// Records one structural undo step. Returns the new index.
    pub fn add_folder_above(&mut self, index: usize, name: impl Into<String>) -> usize {
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let depth = self.layers.get(index).map(|l| l.depth).unwrap_or(0);
        let at = (index + 1).min(self.layers.len());
        let mut f = Layer::new(name);
        f.folder = true;
        f.depth = depth;
        self.layers.insert(at, f);
        self.active = at;
        self.normalize_depths();
        self.record_structure("New folder", before, active_before);
        self.touch();
        at
    }

    /// New empty layer as the **topmost child** of the folder at `index`, and
    /// make it active. Records one structural undo step. Returns the new index.
    pub fn add_layer_in_folder(&mut self, index: usize, name: impl Into<String>) -> Option<usize> {
        if !self.layers.get(index)?.folder {
            return None;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let mut l = Layer::new(name);
        l.depth = self.layers[index].depth + 1;
        self.layers.insert(index, l);
        self.active = index;
        self.record_structure("New layer", before, active_before);
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
        let (before, active_before) = (self.stack_snapshot(), self.active);
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
        self.record_structure("New frame folder", before, active_before);
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
        let (before, active_before) = (self.stack_snapshot(), self.active);
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
        self.record_structure("Divide frame folder", before, active_before);
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
            // Nothing to duplicate — the empty-folder answer IS the answer
            // (and it records its own structural undo step).
            return self.divide_frame_folder(index, keep, split_off);
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        for c in &mut block {
            // A clone must never inherit an open op's recording.
            c.recording = None;
            // The originals stay in the source folder — new identities here.
            c.id = mint_id();
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
        self.record_structure("Divide frame folder", before, active_before);
        self.touch();
        Some(at + k)
    }

    /// Move a whole block (a layer, or a folder with everything in it) so its
    /// bottom lands at gap `slot` (an insertion point in the **current**
    /// stack, 0..=len), and give the moved layer depth `depth` (children keep
    /// their relative depths; the result is normalized). Refuses to drop a
    /// folder into itself. Records one structural undo step.
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
        let (before, active_before) = (self.stack_snapshot(), self.active);
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
        self.record_structure("Move layer", before, active_before);
        self.touch();
        true
    }

    /// Insert a new empty layer directly above `index` (same depth — a
    /// sibling) and make it active. Returns the new layer's index. Records
    /// one structural undo step.
    /// The top of the clip run riding `index`: the last CONSECUTIVE clipped
    /// (same depth, non-folder) layer above it — `index` itself when nothing
    /// rides. From a mid-run member it finds the same top, so an insert
    /// relative to any member of the run lands outside it.
    pub fn clip_run_top(&self, index: usize) -> usize {
        let Some(l) = self.layers.get(index) else {
            return index;
        };
        let mut j = index;
        while let Some(n) = self.layers.get(j + 1) {
            if n.depth != l.depth || n.folder || !n.clip {
                break;
            }
            j += 1;
        }
        j
    }

    pub fn add_layer_above(&mut self, index: usize, name: impl Into<String>) -> usize {
        let (before, active_before) = (self.stack_snapshot(), self.active);
        // docs/CLIPPING-SCENARIOS.md: a plain insert INSIDE a clip run would
        // silently re-base the members above it onto the new empty layer —
        // everything clipped goes invisible. Hop above the run instead; a
        // layer meant to join the run still can (clip resolves through the
        // members to the same base wherever it sits in the run).
        let index = self.clip_run_top(index);
        let at = (index + 1).min(self.layers.len());
        let mut l = Layer::new(name);
        l.depth = self.layers.get(index).map(|x| x.depth).unwrap_or(0);
        self.layers.insert(at, l);
        self.active = at;
        self.normalize_depths();
        self.record_structure("New layer", before, active_before);
        self.touch();
        at
    }

    /// Insert a new empty layer above the active one and make it active.
    pub fn add_layer(&mut self, name: impl Into<String>) -> usize {
        self.add_layer_above(self.active, name)
    }

    /// Current index of the layer with stable id `id`. THE door for anything
    /// holding an id across edits (automation, future cross-references) —
    /// linear, stacks are small.
    pub fn layer_index_of(&self, id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// Make every stable id in the document real and unique: layers, text
    /// items and balloons. `0` (a file from before ids existed, or a fresh
    /// item awaiting its commit) and duplicates (a hand-edited file; first
    /// occurrence keeps the id) are reminted. Lifts the mint past the largest
    /// id seen FIRST, so a heal can never hand out an id the file also holds.
    /// Called by the ORA loader; harmless anywhere else.
    pub fn ensure_ids(&mut self) {
        let mut max = 0u64;
        for l in &self.layers {
            max = max.max(l.id);
            match &l.kind {
                LayerKind::Text(ts) => {
                    for t in &ts.texts {
                        max = max.max(t.id);
                    }
                }
                LayerKind::Balloon(bs) => {
                    for b in &bs.balloons {
                        max = max.max(b.id);
                    }
                }
                _ => {}
            }
        }
        bump_ids_past(max);
        let mut seen = std::collections::HashSet::new();
        for l in &mut self.layers {
            if l.id == 0 || !seen.insert(l.id) {
                l.id = mint_id();
                seen.insert(l.id);
            }
            match &mut l.kind {
                LayerKind::Text(ts) => ts.mint_ids(),
                LayerKind::Balloon(bs) => bs.mint_ids(),
                _ => {}
            }
        }
    }

    /// Remove a layer — a folder goes with everything inside it. Refuses to
    /// empty the document and refuses an out-of-range index; both return
    /// `false`. Records one structural undo step.
    pub fn remove_layer(&mut self, index: usize) -> bool {
        if index >= self.layers.len() {
            return false;
        }
        let r = self.block_range(index);
        if r.len() >= self.layers.len() {
            return false;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        self.layers.drain(r);
        if self.active >= self.layers.len() {
            self.active = self.layers.len() - 1;
        }
        self.normalize_depths();
        self.record_structure("Delete layer", before, active_before);
        self.touch();
        true
    }

    /// Copy a layer (pixels included — `Arc` clones, so it is cheap until one
    /// of the two is painted on) and insert the copy above it. A folder is
    /// copied with its children. Returns the new index of the copied layer.
    /// Records one structural undo step.
    pub fn duplicate_layer(&mut self, index: usize) -> Option<usize> {
        if index >= self.layers.len() {
            return None;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let r = self.block_range(index);
        let mut block: Vec<Layer> = self.layers[r.clone()].to_vec();
        for l in &mut block {
            // A clone must never inherit an open op's recording.
            l.recording = None;
            // Both copies live from here on — the copy is a NEW identity.
            l.id = mint_id();
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
        self.record_structure("Duplicate layer", before, active_before);
        self.touch();
        Some(at)
    }

    /// Move a layer to a new index (reorder), keeping its depth. `to` is the
    /// index it should end up at in the resulting stack. A folder moves with
    /// its children. Returns `false` on a bad index. Records one structural undo step;
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
    /// canvas (oversized images are clipped). Records one structural undo step like any
    /// structural layer op. Returns the new layer's index.
    pub fn add_layer_from_image(
        &mut self,
        name: impl Into<String>,
        img: &image::RgbaImage,
    ) -> usize {
        let at = self.add_layer(name);
        let size = self.size;
        fill_layer_from_image(&mut self.layers[at], size, img);
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
    /// the art they mask — rasterized from `frames` and made active. Records
    /// one structural undo step. Returns the new index.
    pub fn add_frame_layer(&mut self, name: impl Into<String>, frames: FrameSet) -> usize {
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let mut l = Layer::new(name);
        l.replace_tiles(frames.rasterize(self.size));
        l.kind = LayerKind::Frame(frames);
        self.layers.push(l);
        self.active = self.layers.len() - 1;
        self.record_structure("New frame layer", before, active_before);
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
        let (before, active_before) = (self.stack_snapshot(), self.active);
        // Audit 2026-08-21: the combine DESTROYS B's header, and everything
        // that renders at FOLDER level goes with it — the compositor reads
        // visibility/opacity/blend/through/draft off the header, and border
        // width, the ruler flag and the reading pin live on the `FrameSet`,
        // not on a `Frame`. None of it has a per-child home to move to (a
        // group blend is not a per-child blend, a hidden group is not a
        // hidden child, and a `Frame` carries no border of its own), so a
        // pair that disagrees refuses instead of silently repainting one
        // side's panels in the other's style. `slot` is exempt: it is
        // divide provenance, and clearing it is this op's documented job.
        let look = |l: &Layer| {
            (
                l.visible,
                l.through,
                l.draft,
                l.opacity.to_bits(),
                l.blend,
                l.frames()
                    .map(|f| (f.border_px.to_bits(), f.border_ruler, f.reading_pin)),
            )
        };
        // A layer mask is a canvas-space raster: it cannot be split between
        // the two, and the survivor's would start clipping the partner's ink.
        if look(ha) != look(hb) || ha.mask.is_some() || hb.mask.is_some() {
            return None;
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
        self.record_structure("Combine frame folders", before, active_before);
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
        let (before, active_before) = (self.stack_snapshot(), self.active);
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
        self.record_structure("Group frame folders", before, active_before);
        self.touch();
        Some(run.end)
    }

    /// SF-004/005: re-rasterize the effect-line layer at `index` from
    /// `spec` — ONE undo step covering both the pixels and the parameters.
    /// Returns false, document untouched, when the layer carries no spec of
    /// its own or the render produced nothing (the caller's status message).
    ///
    /// The spec is an argument rather than a field read (it used to be
    /// stored first, then rendered) because both halves must go into the
    /// same group: undo that restored the old pixels while leaving the new
    /// parameters on the layer would make the Edit dialog describe art that
    /// is no longer there. The tiles are written through `set_tile` inside
    /// the op bracket — the old wholesale `replace_tiles` swap bypassed the
    /// copy-on-write recording, which is why the regen was un-undoable and
    /// had to purge the layer's history to stay consistent.
    pub fn regen_genlines(&mut self, index: usize, spec: crate::genlines::GenLinesSpec) -> bool {
        // Only a layer that was generated regenerates: this is the in-place
        // door, and pointing it at an ink layer would eat the drawing.
        let Some(before) = self.layers.get(index).and_then(|l| l.genlines) else {
            return false;
        };
        let tiles = spec.render(self.size);
        if tiles.is_empty() {
            return false;
        }
        // An op left open belongs to whatever gesture came before; close it
        // rather than nest (same rule `undo` follows), or `begin_op_on`
        // would no-op and these writes would go unrecorded.
        if self.op_layer.is_some() {
            self.end_op();
        }
        self.begin_op_on(index);
        self.set_op_label("Regenerate lines");
        // Wholesale replacement, tile by tile: every index the old raster
        // covered and the new one does not has to be cleared, or the
        // previous lines survive around the new ones.
        let stale: Vec<TileIdx> = self.layers[index]
            .tiles()
            .map(|(i, _)| i)
            .filter(|i| !tiles.contains_key(i))
            .collect();
        for idx in stale {
            self.layers[index].set_tile(idx, None);
        }
        for (idx, tile) in tiles {
            self.layers[index].set_tile(idx, Some(tile));
        }
        self.layers[index].genlines = Some(spec);
        if let Some((li, label, tiles)) = self.take_op() {
            self.history.push_labeled(
                &label,
                UndoGroup::GenLines {
                    layer: li,
                    spec: before,
                    tiles,
                },
            );
        }
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
        // A PLAIN folder takes the effect as FB-knockout: the outline grows
        // from the union of its children's ink and lies just beneath the
        // group — the hand-painted "White" mat, automated. Frame folders
        // still refuse: their close already owns a mask + border ink.
        if (l.folder && l.is_frame()) || l.edge == edge {
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
        self.refresh_derived_with(dpi, &mut |_, _| None);
    }

    /// [`Self::refresh_derived`] with a correction kernel lent by the caller
    /// (`crate::correction::CorrKernel`). Only the correction stage consults
    /// it; every other derived raster is CPU-only and unaffected. The app's
    /// render loop passes the GPU seam here — see `App::refresh_tones`.
    pub fn refresh_derived_with(
        &mut self,
        dpi: u32,
        corr: &mut crate::correction::CorrKernel<'_>,
    ) {
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
            // Folders derive from their CHILDREN (below), not from here —
            // a folder's own raster is border ink, not the group's alpha.
            if !l.folder && (l.edge.is_some() || l.edge_tiles.is_some()) {
                l.refresh_edge(size);
            }
        }
        // FB-knockout: folder mats, AFTER the loop so children's derived
        // rasters (tone, live fill, their own edges) are what gets grown.
        for fi in 0..self.layers.len() {
            if self.layers[fi].folder {
                self.refresh_folder_edge(fi);
            }
        }
        // Correction layers LAST of all: their derivation composites the
        // layers below through the real compositor, so every other derived
        // raster must already be fresh when they read it.
        self.refresh_corrections_with(dpi, corr);
    }

    /// FB-knockout: derive a plain folder's border-effect raster from the
    /// union of its effectively-visible children's DISPLAY ink. The result
    /// is the mat alone (no source baked in — the group draws itself on
    /// top); both compositors lay it on the page just beneath the group at
    /// the folder's close, scaled by the folder's opacity.
    fn refresh_folder_edge(&mut self, fi: usize) {
        use std::collections::HashMap;
        use std::sync::Arc;
        let Some(p) = self.layers[fi].edge else {
            if self.layers[fi].edge_tiles.is_some() || self.layers[fi].edge_stamp.is_some() {
                self.layers[fi].edge_tiles = None;
                self.layers[fi].edge_stamp = None;
            }
            return;
        };
        let size = self.size;
        // Children that actually show: own eye on, and every folder between
        // them and this header open-eyed too. A hidden child must not leave
        // a ghost mat.
        let kids: Vec<usize> = self
            .children_range(fi)
            .filter(|&k| {
                if !self.layers[k].visible {
                    return false;
                }
                let mut a = k;
                while let Some(f) = self.enclosing_folder(a) {
                    if f == fi {
                        return true;
                    }
                    if !self.layers[f].visible {
                        return false;
                    }
                    a = f;
                }
                true
            })
            .collect();
        let stamp = {
            // Same shape as `refresh_edge`'s stamp: params, an
            // order-independent hash of WHICH tiles exist (child identity
            // and visibility folded in), and the newest source revision.
            let mut keys = 0i64;
            let mut newest = 0u64;
            for &k in &kids {
                keys = keys.wrapping_add((k as i64).wrapping_mul(0x51_7C_C1B7));
                for (idx, t) in self.layers[k].display_tiles() {
                    keys = keys.wrapping_add(
                        (idx.x as i64).wrapping_mul(0x9E37_79B9) ^ ((idx.y as i64) << 21),
                    );
                    newest = newest.max(t.revision());
                }
            }
            (p, keys, newest)
        };
        if self.layers[fi].edge_stamp == Some(stamp) && self.layers[fi].edge_tiles.is_some() {
            return;
        }
        let keep_cache =
            self.layers[fi].edge_stamp.map(|(q, k, _)| (q, k)) == Some((stamp.0, stamp.1));

        let r = p.reach();
        let ts = TILE_SIZE as i32;
        let span = r as i32 / ts + 1;
        let tsu = TILE_SIZE as u32;
        let (cw, chh) = (size.0.div_ceil(tsu) as i32, size.1.div_ceil(tsu) as i32);
        let mut cands: std::collections::HashSet<TileIdx> = Default::default();
        for &k in &kids {
            for (idx, _) in self.layers[k].display_tiles() {
                for dy in -span..=span {
                    for dx in -span..=span {
                        let i = TileIdx::new(idx.x + dx, idx.y + dy);
                        if i.x >= 0 && i.y >= 0 && i.x < cw && i.y < chh {
                            cands.insert(i);
                        }
                    }
                }
            }
        }
        let old = if keep_cache {
            self.layers[fi].edge_tiles.clone()
        } else {
            None
        };
        let mut out: HashMap<TileIdx, Arc<Tile>> = HashMap::new();
        let side = TILE_SIZE + 2 * r;
        let mut seed = vec![crate::edge::INF; side * side];
        for idx in cands {
            let neighbours = || {
                (-span..=span).flat_map(move |dy| {
                    (-span..=span).map(move |dx| TileIdx::new(idx.x + dx, idx.y + dy))
                })
            };
            let newest = neighbours()
                .map(|n| {
                    kids.iter()
                        .filter_map(|&k| self.layers[k].display_tile(n))
                        .map(|t| t.revision())
                        .max()
                        .unwrap_or(0)
                })
                .max()
                .unwrap_or(0);
            if let Some(t) = old.as_ref().and_then(|m| m.get(&idx))
                && t.revision() > newest
            {
                out.insert(idx, t.clone());
                continue;
            }
            seed.fill(crate::edge::INF);
            let (ox, oy) = idx.origin();
            let (px0, py0) = (ox - r as i32, oy - r as i32);
            for n in neighbours() {
                let (nx, ny) = n.origin();
                let x0 = px0.max(nx);
                let x1 = (px0 + side as i32).min(nx + ts);
                let y0 = py0.max(ny);
                let y1 = (py0 + side as i32).min(ny + ts);
                if x0 >= x1 || y0 >= y1 {
                    continue;
                }
                for &k in &kids {
                    let Some(t) = self.layers[k].display_tile(n) else {
                        continue;
                    };
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
            }
            // No colour window: the knockout mat is a solid backing — a
            // watercolour style on a folder header would fall back to the
            // picked colour (nearest_ink_colour's empty-window rule).
            let t = crate::edge::derive_tile(&mut seed, r, None, p, &[]);
            out.insert(idx, Arc::new(t));
        }
        self.layers[fi].edge_stamp = Some(stamp);
        self.layers[fi].edge_tiles = Some(out);
    }

    /// New balloon layer at the **top** of the stack — balloons sit above the
    /// art and the frames they annotate — rasterized from `balloons` and made
    /// active. Records one structural undo step.
    pub fn add_balloon_layer(&mut self, name: impl Into<String>, mut balloons: BalloonSet) -> usize {
        balloons.mint_ids();
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let mut l = Layer::new(name);
        l.replace_tiles(balloons.rasterize(self.size));
        l.kind = LayerKind::Balloon(balloons);
        self.layers.push(l);
        self.active = self.layers.len() - 1;
        self.record_structure("New balloon layer", before, active_before);
        self.touch();
        self.active
    }

    /// Replace a balloon layer's vector state, re-rasterize, and push one
    /// undo step. Returns `false` when `index` is not a balloon layer.
    pub fn set_balloons(&mut self, index: usize, mut balloons: BalloonSet) -> bool {
        // New items arrive with id 0 (and a duplicated item carries its
        // source's id) — the commit door is where identities become real.
        balloons.mint_ids();
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
    /// balloons included), rasterized from `texts` and made active. Records
    /// one structural undo step.
    pub fn add_text_layer(&mut self, name: impl Into<String>, mut texts: TextSet) -> usize {
        texts.mint_ids();
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let mut l = Layer::new(name);
        l.replace_tiles(texts.rasterize(self.size));
        l.kind = LayerKind::Text(texts);
        self.layers.push(l);
        self.active = self.layers.len() - 1;
        self.record_structure("New text layer", before, active_before);
        self.touch();
        self.active
    }

    /// Replace a text layer's vector state, re-rasterize, and push one undo
    /// step. Returns `false` when `index` is not a text layer.
    /// Set a text layer's vector state with NO rasterize and NO undo —
    /// the Story Editor's non-active-page path (the doc re-encodes to
    /// bytes; its raster rebuilds when the page loads and warms).
    pub fn set_texts_raw(&mut self, index: usize, mut texts: TextSet) -> bool {
        texts.mint_ids();
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

    /// Re-blit a text layer from the sprites it already holds — no undo
    /// step, no reshaping, no change to the vector state.
    ///
    /// `IO-060`'s text half: a work resample drops every sprite (they were
    /// shaped at the old dpi) and leaves the RESAMPLED pixels standing so
    /// the page is never blank. Once the app has re-warmed the caches at
    /// the new dpi this lays the crisp sprites down over them. Pushing an
    /// undo step here would be a lie — the step before it is a text layer
    /// with no sprites, which rasterizes to nothing.
    pub fn reraster_text(&mut self, index: usize) -> bool {
        let size = self.size;
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        let LayerKind::Text(cur) = &l.kind else {
            return false;
        };
        if cur.texts.iter().all(|t| t.cache.is_none()) {
            return false;
        }
        let raster = cur.rasterize(size);
        l.replace_tiles(raster);
        self.touch();
        true
    }

    pub fn set_texts(&mut self, index: usize, mut texts: TextSet) -> bool {
        // Same contract as `set_balloons`: the commit mints new identities.
        texts.mint_ids();
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
    ///
    /// A FOLDER merges down too (CSP: the group flattens onto the layer
    /// under it — the everyday "collapse this shading folder into the
    /// flats"): its isolated composite is baked with the header's own
    /// opacity and blend, then the whole block leaves. A CLIPPED layer
    /// bakes what it SHOWS — its ink cut to the base's alpha — when the
    /// layer under it is that base (CSP's merge); when the layer under it
    /// is another member of the same clip run the ink lands as-is and
    /// stays clipped through it.
    ///
    /// The "layer below" is the row under the whole BLOCK (a folder's
    /// children sit below its header in `layers`), see
    /// [`Self::merge_down_refusal`] for every rule.
    pub fn merge_down(&mut self, index: usize) -> bool {
        if self.merge_down_refusal(index).is_some() {
            return false;
        }
        let block = self.block_range(index);
        let below = block.start - 1;
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let mut upper = if self.layers[index].folder {
            self.flatten_block(block.clone())
        } else {
            self.layers[index].clone()
        };
        if upper.clip && self.clip_bases()[index] == Some(below) {
            let base = &self.layers[below];
            let keys: Vec<TileIdx> = upper.tiles.keys().copied().collect();
            for idx in keys {
                let Some(b) = base.display_tile(idx) else {
                    upper.tiles.remove(&idx);
                    continue;
                };
                let bd = b.data();
                let ud = Arc::make_mut(upper.tiles.get_mut(&idx).expect("own key")).data_mut();
                for p in 0..crate::tile::TILE_PIXELS {
                    let a = bd[p * 4 + 3] as u32;
                    for c in 0..4 {
                        ud[p * 4 + c] =
                            ((ud[p * 4 + c] as u32 * a) / crate::tile::FIX15_ONE as u32) as u16;
                    }
                }
            }
        }
        bake_layer_into(&mut self.layers[below], &upper);
        self.layers.drain(block);
        self.active = below;
        self.normalize_depths();
        self.record_structure("Merge down", before, active_before);
        self.touch();
        true
    }

    /// Why [`Self::merge_down`] would refuse `index`, in the words the
    /// status line says — `None` = it will merge. One list so the palette
    /// never refuses silently: Ctrl+E doing nothing with no message was
    /// the 2026-09-02 surface pass's first finding in this family.
    pub fn merge_down_refusal(&self, index: usize) -> Option<&'static str> {
        let l = self.layers.get(index)?;
        let block = self.block_range(index);
        let Some(below) = block.start.checked_sub(1) else {
            return Some("nothing below this layer to merge into");
        };
        let b = &self.layers[below];
        if l.is_frame() {
            return Some("frame folders keep their vectors — they never merge");
        }
        if l.is_vector() {
            return Some("balloon and text layers keep their vectors — rasterize first");
        }
        if b.folder {
            return Some("the row below is a folder — open it and merge onto a layer inside, or merge the folder itself");
        }
        if b.is_vector() {
            return Some("the row below keeps its vectors (frame/balloon/text) — nothing can merge onto it");
        }
        // The DESTINATION may not be a stroke-recording layer: the merged
        // pixels would land in tiles the next replay zeroes. (The source
        // may be one — that is CSP's rasterize-and-merge, and its record
        // leaves with the layer.)
        if b.records_strokes() {
            return Some("the row below is a vector layer — its next edit would replay over the merged ink");
        }
        if l.tone.is_some() || b.tone.is_some() {
            return Some("merge refuses tone layers — remove the tone first (it is non-destructive)");
        }
        if l.lock {
            return Some("this layer is locked — unlock it to merge");
        }
        if b.lock {
            return Some("the layer below is locked — unlock it to merge into it");
        }
        // Merging across a folder boundary would smuggle pixels in or out
        // of a mask.
        if l.depth != b.depth {
            return Some("the row below is outside this folder — move the layer out first");
        }
        None
    }

    /// The isolated composite of the block `r` (a folder header and its
    /// children) as ONE raster layer, with the header's own opacity and
    /// blend carried on the result so the caller bakes it exactly as the
    /// page showed the group. Children are composited through the header
    /// at full opacity/Normal inside a scratch document, which is the
    /// group's isolated buffer by construction.
    fn flatten_block(&self, r: std::ops::Range<usize>) -> Layer {
        let header = &self.layers[r.end - 1];
        let mut scratch = Document::new(self.size.0, self.size.1);
        scratch.layers = self.layers[r].to_vec();
        let base = header.depth;
        for l in &mut scratch.layers {
            l.depth -= base;
            l.recording = None;
        }
        let h = scratch.layers.len() - 1;
        scratch.layers[h].opacity = 1.0;
        scratch.layers[h].blend = Blend::Normal;
        scratch.layers[h].visible = true;
        let img = crate::export::composite(&scratch, crate::export::Background::Transparent);
        let mut out = Layer::new(header.name.clone());
        out.opacity = header.opacity;
        out.blend = header.blend;
        out.visible = header.visible;
        fill_layer_from_image(&mut out, self.size, &img);
        out
    }

    /// CSP Layer ▸ Flatten image: every visible layer composites into ONE
    /// raster layer and the rest of the stack goes — hidden layers too, as
    /// CSP discards them. One structural undo step. Refused on a
    /// single-layer stack that is already a plain raster (nothing to do).
    pub fn flatten(&mut self) -> bool {
        if self.layers.len() == 1 {
            let l = &self.layers[0];
            if !l.folder && !l.is_vector() && l.tone.is_none() && l.strokes.is_none() {
                return false;
            }
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let img = crate::export::composite(self, crate::export::Background::Transparent);
        let mut out = Layer::new("Flattened");
        fill_layer_from_image(&mut out, self.size, &img);
        self.layers = vec![out];
        self.active = 0;
        self.record_structure("Flatten image", before, active_before);
        self.touch();
        true
    }

    /// CSP "Merge selected layers" (選択中のレイヤーを結合, the owner's
    /// Shift+Alt+E): flatten the palette's multi-selection into ONE raster
    /// layer at the LOWEST selected position, bottom-up, honouring each
    /// layer's blend, opacity and visibility. ONE structural undo step.
    ///
    /// Same refusals as [`Self::merge_down`], for the same reasons, applied
    /// to the whole set: folders, vector kinds and stroke-recording
    /// destinations never merge, a locked layer refuses edits, a clipped
    /// layer's raw pixels are not what it shows, and a set spanning a folder
    /// boundary would smuggle pixels in or out of a mask. The lowest
    /// selected layer MAY be clipped — it keeps its clip and the merge lands
    /// inside it, exactly as merging down into a clipped layer does.
    ///
    /// Unselected layers sitting BETWEEN selected ones are skipped, not
    /// preserved in order: the merged result lands at the lowest selected
    /// position with the rest of the sandwich still above it. That is CSP's
    /// behaviour and it only shows when an interleaved layer blends
    /// non-Normally — for the ordinary contiguous selection the page
    /// composites identically before and after.
    pub fn merge_selected(&mut self, indices: &[usize]) -> bool {
        let mut idx: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|&i| i < self.layers.len())
            .collect();
        idx.sort_unstable();
        idx.dedup();
        if idx.len() < 2 {
            return false;
        }
        let depth = self.layers[idx[0]].depth;
        let refuses = |l: &Layer| {
            l.folder || l.is_vector() || l.records_strokes() || l.lock || l.depth != depth
        };
        if idx.iter().any(|&i| refuses(&self.layers[i]))
            || idx[1..].iter().any(|&i| self.layers[i].clip)
        {
            return false;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let dst = idx[0];
        for &i in &idx[1..] {
            let upper = self.layers[i].clone();
            bake_layer_into(&mut self.layers[dst], &upper);
        }
        // Top-down, so the indices below each removal stay valid.
        for &i in idx[1..].iter().rev() {
            self.layers.remove(i);
        }
        self.active = dst;
        self.record_structure("Merge selected layers", before, active_before);
        self.touch();
        true
    }

    /// CSP "Release folder" (レイヤーフォルダーを解除, the owner's
    /// Ctrl+Shift+G): dissolve the folder at `index` — every descendant
    /// rises one level (nested folders keep their own nesting), the header
    /// goes, order is untouched. ONE structural undo step.
    ///
    /// Refused on a FRAME folder: its header carries the panel vectors AND
    /// the coverage mask its children are clipped by, so dropping it would
    /// quietly un-clip the art. [`Self::rasterize_frame_folder`] is that
    /// folder's release — it hands the mask down first. Also refused on a
    /// locked header, and on the last layer standing (a document always has
    /// a layer).
    ///
    /// The header's own rendering state — opacity, blend, mask, border
    /// effect, and the isolation a non-Through folder gives its children —
    /// cannot come with it, because no per-child value reproduces a group
    /// effect over overlapping children. A neutral folder therefore
    /// composites identically afterwards and a dressed one does not;
    /// [`Self::folder_release_is_lossless`] is the door callers warn from.
    pub fn release_folder(&mut self, index: usize) -> bool {
        let Some(h) = self.layers.get(index) else {
            return false;
        };
        if !h.folder || h.is_frame() || h.lock || self.layers.len() <= 1 {
            return false;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        for k in self.children_range(index).collect::<Vec<_>>() {
            // Every descendant is at least one deeper than the header, so
            // this is a plain rise; nesting between them is preserved.
            self.layers[k].depth = self.layers[k].depth.saturating_sub(1);
        }
        self.layers.remove(index);
        self.active = match self.active.cmp(&index) {
            std::cmp::Ordering::Greater => self.active - 1,
            std::cmp::Ordering::Equal => index.saturating_sub(1),
            std::cmp::Ordering::Less => self.active,
        }
        .min(self.layers.len() - 1);
        self.normalize_depths();
        self.record_structure("Release folder", before, active_before);
        self.touch();
        true
    }

    /// Whether releasing the folder at `index` leaves the page compositing
    /// the same — i.e. the header dresses nothing its children can inherit.
    /// `false` is not a refusal, it is the warning the command says out
    /// loud before doing it anyway (the artist can undo).
    pub fn folder_release_is_lossless(&self, index: usize) -> bool {
        let Some(h) = self.layers.get(index) else {
            return false;
        };
        let isolates = !h.through
            && self
                .children_range(index)
                .any(|k| self.layers[k].blend != Blend::Normal);
        h.visible
            && h.opacity >= 1.0
            && h.blend == Blend::Normal
            && h.mask.is_none()
            && h.edge.is_none()
            && h.tone.is_none()
            && !isolates
    }

    /// Batch: one tone change across many layers as ONE undo step
    /// (`UndoGroup::Compound` of the individual `Tones` groups). Layers
    /// that refuse (folders, vector kinds, already-equal) are skipped;
    /// returns how many changed. Zero changes push nothing.
    pub fn set_tone_many(
        &mut self,
        indices: &[usize],
        tone: Option<crate::tone::ToneParams>,
    ) -> usize {
        let mut members = Vec::new();
        for &i in indices {
            let Some(l) = self.layers.get_mut(i) else {
                continue;
            };
            if l.folder || l.is_vector() || l.tone == tone {
                continue;
            }
            let before = l.tone;
            l.tone = tone;
            l.tone_tiles = None;
            members.push(UndoGroup::Tones {
                layer: i,
                tone: before,
            });
        }
        let n = members.len();
        if n > 0 {
            self.history
                .push_labeled("Batch tone", UndoGroup::Compound(members));
            self.touch();
        }
        n
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
    ///
    /// A plain selection collapses the palette multi-selection (CSP: any
    /// non-modified click or keyboard move leaves one row selected) — the
    /// gesture methods below manage `active` directly instead.
    pub fn set_active(&mut self, index: usize) -> bool {
        if index >= self.layers.len() {
            return false;
        }
        self.active = index;
        self.layer_multi.clear();
        self.touch();
        true
    }

    /// TC-013 Ctrl+click: toggle `index` in the palette multi-selection.
    /// Toggling a row ON also makes it the editing target (CSP moves the
    /// pen); toggling the ACTIVE row off hands the target to the nearest
    /// remaining selected row. The last selected row cannot be toggled off.
    pub fn toggle_multi(&mut self, index: usize) -> bool {
        if index >= self.layers.len() {
            return false;
        }
        if index == self.active {
            // Deselect the target: someone else must take the pen.
            let Some(pos) = self.layer_multi.iter().rposition(|&m| m < index).or(
                if self.layer_multi.is_empty() {
                    None
                } else {
                    Some(0)
                },
            ) else {
                return false; // the only selected row stays selected
            };
            self.active = self.layer_multi.remove(pos);
        } else if let Some(pos) = self.layer_multi.iter().position(|&m| m == index) {
            self.layer_multi.remove(pos);
        } else {
            self.layer_multi.push(self.active);
            self.active = index;
            self.layer_multi.retain(|&m| m != index);
            self.layer_multi.sort_unstable();
        }
        self.touch();
        true
    }

    /// TC-013 Shift+click: select the contiguous range between the active
    /// row and `index`; the active row keeps the pen. Replaces any prior
    /// multi-selection, like CSP's range gesture.
    pub fn range_multi(&mut self, index: usize) -> bool {
        if index >= self.layers.len() {
            return false;
        }
        let (lo, hi) = (self.active.min(index), self.active.max(index));
        self.layer_multi = (lo..=hi).filter(|&i| i != self.active).collect();
        self.touch();
        true
    }

    /// The rows a "selected layers" operation targets: the active layer
    /// plus the multi-selection, bottom-to-top. Never empty.
    pub fn multi_targets(&self) -> Vec<usize> {
        let mut t = self.layer_multi.clone();
        t.push(self.active);
        t.sort_unstable();
        t
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

    /// Row 33 (CSP Convert layer): convert `li` — v1 rasterizes Text /
    /// Balloon / vector layers (the rendered tiles are kept as-is, the
    /// vector state dropped), optionally changes the expression colour
    /// and blend mode, renames, and either keeps or replaces the
    /// original. ONE structural undo step.
    pub fn convert_layer(
        &mut self,
        li: usize,
        rasterize: bool,
        expression: Option<LayerExpression>,
        blend: Option<Blend>,
        keep_original: bool,
        name: Option<String>,
    ) -> bool {
        let Some(src) = self.layers.get(li) else {
            return false;
        };
        if src.folder {
            return false;
        }
        let before = self.stack_snapshot();
        let active_before = self.active;
        let mut l = src.clone();
        if rasterize {
            // Bake: the tiles already hold the rendered vectors — the
            // conversion is dropping the vector state that regenerates
            // them.
            l.kind = LayerKind::Raster;
            l.strokes = None;
        }
        if let Some(e) = expression {
            l.expression = e;
        }
        if let Some(b) = blend {
            l.blend = b;
        }
        if let Some(n) = name {
            if !n.trim().is_empty() {
                l.name = n.trim().to_owned();
            }
        }
        if keep_original {
            // Original and converted copy both live — the copy is new.
            l.id = mint_id();
            self.layers.insert(li + 1, l);
            self.active = li + 1;
        } else {
            self.layers[li] = l;
            self.active = li;
        }
        self.record_structure("Convert layer", before, active_before);
        self.touch();
        true
    }
    /// Row 32 (CSP Rasterize on a FRAME folder — "do it their way"): the
    /// border is just ink, so the header becomes a plain raster layer
    /// holding it; the panel clipping is expressible as a layer mask, so
    /// every child gains the folder's interior-coverage mask (children
    /// that already carry their own keep it — the two combine visually);
    /// the children STAY separate layers, hoisted loose at the folder's
    /// depth. What you give up is the frame object — no more dragging
    /// panel edges. ONE structural undo step.
    pub fn rasterize_frame_folder(&mut self, li: usize) -> bool {
        let Some(h) = self.layers.get(li) else {
            return false;
        };
        if !(h.folder && h.is_frame()) {
            return false;
        }
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let depth = h.depth;
        let mask_tiles = h.mask_tiles().cloned();
        let mut header = h.clone();
        let kids: Vec<usize> = self.children_range(li).collect();
        for k in kids {
            if let Some(c) = self.layers.get_mut(k) {
                // Loose at the folder's own depth — the folder is gone.
                c.depth = depth;
                if c.mask.is_none()
                    && let Some(tiles) = mask_tiles.clone()
                {
                    c.mask = Some(LayerMask {
                        tiles,
                        enabled: true,
                        revision: crate::tile::next_revision(),
                        full: false,
                    });
                }
            }
        }
        header.folder = false;
        header.kind = LayerKind::Raster;
        self.layers[li] = header;
        self.record_structure("Rasterize frame folder", before, active_before);
        self.touch();
        true
    }


    /// Row 31 (CSP 画像から線画を抽出, Extract lines): lift the active
    /// layer's DARK pixels as lineart onto a fresh layer above — per
    /// pixel, alpha scales with how far below the threshold the luma
    /// sits (a black line is a full-opacity line; a mid grey is a faint
    /// one), colour straight black. Returns the new layer's index.
    pub fn extract_lines(&mut self, li: usize, detection: f32) -> Option<usize> {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let thr = detection.clamp(0.02, 1.0);
        let src: Vec<(TileIdx, std::sync::Arc<Tile>)> =
            self.layers.get(li)?.tiles().map(|(i, t)| (i, t.clone())).collect();
        if src.is_empty() {
            return None;
        }
        let mut out = Layer::new("Extracted lines");
        out.name = "Extracted lines".into();
        for (idx, t) in &src {
            let (ox, oy) = idx.origin();
            let d = t.data();
            let nt = out.tile_mut(*idx);
            let nd = nt.data_mut();
            for py in 0..crate::tile::TILE_SIZE {
                for px in 0..crate::tile::TILE_SIZE {
                    let o = (py * crate::tile::TILE_SIZE + px) * 4;
                    let a = d[o + 3] as f32;
                    if a == 0.0 {
                        continue;
                    }
                    let (x, y) = (ox + px as i32, oy + py as i32);
                    if x < 0 || y < 0 || x >= w || y >= h {
                        continue;
                    }
                    // Straight luma over the alpha (the pixel's real
                    // coverage is carried by its own alpha below).
                    let inv = 1.0 / a;
                    let luma = (d[o] as f32 * inv).min(1.0) * 0.2126
                        + (d[o + 1] as f32 * inv).min(1.0) * 0.7152
                        + (d[o + 2] as f32 * inv).min(1.0) * 0.0722;
                    if luma >= thr {
                        continue;
                    }
                    // How much darker than the threshold = the line's
                    // strength, times the pixel's own alpha.
                    let strength = (thr - luma) / thr;
                    let alpha = (strength * a / crate::blend::FIX15_ONE_F)
                        .clamp(0.0, 1.0);
                    let f15 = crate::blend::f32_to_fix15(alpha);
                    nd[o] = 0;
                    nd[o + 1] = 0;
                    nd[o + 2] = 0;
                    nd[o + 3] = f15;
                }
            }
        }
        let before = self.stack_snapshot();
        let active_before = self.active;
        self.layers.insert(li + 1, out);
        self.active = li + 1;
        self.record_structure("Extract lines", before, active_before);
        self.touch();
        Some(self.active)
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
        // The folders ENCLOSING the soloed row stay on: a hidden parent
        // hides its children, so soloing a layer inside a folder would
        // otherwise show a blank page (surface pass 2026-09-02).
        let keep = self.ancestors(index);
        for (i, l) in self.layers.iter_mut().enumerate() {
            l.visible = i == index || keep.contains(&i);
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

    /// Every folder header enclosing `index`, innermost first.
    pub fn ancestors(&self, index: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut cur = index;
        while let Some(f) = self.enclosing_folder(cur) {
            out.push(f);
            cur = f;
        }
        out
    }

    /// Is `index` the only visible layer, its enclosing folders aside?
    /// (The solo's second-press test.)
    pub fn only_visible(&self, index: usize) -> bool {
        let keep = self.ancestors(index);
        if keep.iter().any(|&f| self.layers[f].visible)
            && self
                .layers
                .iter()
                .enumerate()
                .all(|(i, l)| l.visible == (i == index || keep.contains(&i)))
        {
            return true;
        }
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

    /// Blend If: set (or clear) this layer's underlying-luminance gate.
    ///
    /// Folders refuse — v1 offers the gate on painted layers only, and the
    /// compositors read `Layer::blend_if` through [`Layer::gate`], which
    /// refuses folders too. Returns false on a no-op so a slider tick that
    /// did not move the value records no undo step.
    ///
    /// The value is [`crate::blendif::BlendIf::normalized`] on the way in:
    /// no compositor ever sees `hi < lo` (it would hide the layer outright).
    pub fn set_layer_blend_if(
        &mut self,
        index: usize,
        gate: Option<crate::blendif::BlendIf>,
    ) -> bool {
        let gate = gate.map(|g| g.normalized());
        let Some(l) = self.layers.get_mut(index) else {
            return false;
        };
        if l.folder || l.blend_if == gate {
            return false;
        }
        l.blend_if = gate;
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

    /// FB-overflow: refuses folders (re-seating a whole group is a v2) and
    /// layers with no frame folder above them (the flag would be a lie).
    pub fn set_layer_escape(&mut self, index: usize, on: bool) -> bool {
        let ok = self
            .layers
            .get(index)
            .is_some_and(|l| !l.folder && (!on || self.enclosing_frame_folder(index).is_some()));
        if !ok {
            return false;
        }
        self.layers[index].escape_frame = on;
        self.touch();
        true
    }

    /// The nearest frame folder ABOVE `index` (never `index` itself).
    pub fn enclosing_frame_folder(&self, index: usize) -> Option<usize> {
        let mut i = index;
        while let Some(f) = self.enclosing_folder(i) {
            if self.layers[f].is_frame() {
                return Some(f);
            }
            i = f;
        }
        None
    }

    /// The breakout layer at `index` re-seated: the composite step it is
    /// emitted immediately AFTER. `None` = not a breakout layer (the walk is
    /// the identity for it).
    ///
    /// The default seat is the layer's own frame folder header — today's
    /// shipped behaviour, and the floor: `draws_over` can only push the seat
    /// UP. Every covered layer is lifted out of any sealed folder the
    /// escapee's own seat is not already inside (see [`Self::lift_seat`]),
    /// then the highest of them wins.
    pub fn spill_anchor(&self, index: usize) -> Option<usize> {
        let l = self.layers.get(index)?;
        if l.folder || !l.escape_frame {
            return None;
        }
        let ff = self.enclosing_frame_folder(index)?;
        if self.layers[ff].through {
            return None; // a through frame folder never clipped it anyway
        }
        let mut seat = ff;
        if !l.draws_over.is_empty() {
            for (j, other) in self.layers.iter().enumerate().skip(ff + 1) {
                // At or below the header is already covered by the default
                // seat, so only the layers above it can move anything.
                if l.draws_over.contains(&other.id) {
                    seat = seat.max(self.lift_seat(j, ff));
                }
            }
        }
        Some(seat)
    }

    /// Lift a covered layer out of every SEALED folder that does not also
    /// enclose the escapee's own seat: inside one, the escapee would join
    /// that group's isolation — and, for a frame folder, be clipped by the
    /// very panel mask it is trying to spill over. Hopping to the folder's
    /// header instead draws it over the whole finished group, which is what
    /// "draws over the art in that panel" has to mean. A Through folder has
    /// no seal to escape, so it is walked past without moving the seat.
    fn lift_seat(&self, target: usize, ff: usize) -> usize {
        // The folders enclosing the escapee's own seat: the escapee already
        // composites inside these, so they are not walls.
        let mut open: Vec<usize> = Vec::new();
        let mut p = ff;
        while let Some(f) = self.enclosing_folder(p) {
            open.push(f);
            p = f;
        }
        let mut seat = target;
        let mut probe = target;
        while let Some(f) = self.enclosing_folder(probe) {
            if open.contains(&f) {
                break;
            }
            if !self.layers[f].through {
                seat = f;
            }
            probe = f;
        }
        seat
    }

    /// The order both compositors walk the stack in, each step with its
    /// EFFECTIVE depth and which half of a mask-capped spill it draws.
    /// Identity except FB-overflow: a non-folder layer with `escape_frame`
    /// inside a SEALED frame folder re-seats immediately after its
    /// [`Self::spill_anchor`], at that anchor's own depth — so its ink lands
    /// in the accumulator the walk has open right there, above the panel
    /// mask and the border ink. Escapees keep their stack order. A through
    /// frame folder never clips, so its escapees stay in place.
    ///
    /// **The mask cap** (part 2, item 1): when a breakout layer carries an
    /// ENABLED layer mask, the mask stops being an alpha mask and becomes
    /// the allowed breakout REGION, so the layer composites TWICE — the
    /// masked-in part ([`SpillPart::Out`]) at the escaped seat, the rest
    /// ([`SpillPart::In`]) at the layer's own seat where the panel still
    /// clips it. The two halves are exact complements (`m` and `1 − m`), so
    /// a half-opacity layer does not double-blend anywhere.
    ///
    /// Both compositors (export.rs and gpu) MUST walk this, not
    /// `self.layers` — a disagreement here is a CPU/GPU parity break.
    pub fn composite_order(&self) -> Vec<CompositeStep> {
        let mut order: Vec<CompositeStep> = Vec::with_capacity(self.layers.len());
        // (anchor, escapee, mask-capped)
        let mut pending: Vec<(usize, usize, bool)> = Vec::new();
        for (li, l) in self.layers.iter().enumerate() {
            match self.spill_anchor(li) {
                Some(anchor) => {
                    let capped = l.breakout_mask().is_some();
                    if capped {
                        // The half the mask holds IN stays exactly where it
                        // always was, panel clip and all.
                        order.push(CompositeStep::new(li, l.depth, SpillPart::In));
                    }
                    pending.push((anchor, li, capped));
                    // No release here: anything anchored to THIS layer waits
                    // for its escaped seat below.
                    continue;
                }
                None => order.push(CompositeStep::new(li, l.depth, SpillPart::All)),
            }
            // Children walk before their header, so everything stashed for
            // this anchor bursts out right after it — folder header or
            // ordinary layer alike.
            Self::release_spills(&mut order, &mut pending, li, l.depth);
        }
        // Unreachable orphans (a stale anchor outrunning a structure edit, or
        // two escapees anchored to each other): walk them in place rather
        // than dropping their art.
        for (_, e, capped) in pending {
            let part = if capped { SpillPart::Out } else { SpillPart::All };
            order.push(CompositeStep::new(e, self.layers[e].depth, part));
        }
        order
    }

    /// Emit every spill anchored at `anchor` (and, transitively, everything
    /// anchored at those), all at the anchor's own effective depth.
    fn release_spills(
        order: &mut Vec<CompositeStep>,
        pending: &mut Vec<(usize, usize, bool)>,
        anchor: usize,
        depth: u8,
    ) {
        let mut queue = std::collections::VecDeque::from([anchor]);
        while let Some(a) = queue.pop_front() {
            let mut k = 0;
            while k < pending.len() {
                if pending[k].0 == a {
                    let (_, e, capped) = pending.remove(k);
                    let part = if capped { SpillPart::Out } else { SpillPart::All };
                    order.push(CompositeStep::new(e, depth, part));
                    queue.push_back(e);
                } else {
                    k += 1;
                }
            }
        }
    }

    /// Every layer the breakout at `index` could be told to draw over: the
    /// stack ABOVE its frame folder header (bottom-first, like `layers`).
    /// Anything at or below that header is covered by the default seat, so
    /// offering it would be a switch that does nothing.
    pub fn spill_candidates(&self, index: usize) -> Vec<usize> {
        match self.spill_anchor(index) {
            Some(_) => {
                let ff = self.enclosing_frame_folder(index).unwrap_or(0);
                ((ff + 1)..self.layers.len()).filter(|&j| j != index).collect()
            }
            None => Vec::new(),
        }
    }

    /// The topmost layer the breakout at `index` currently draws over, i.e.
    /// where the insertion marker sits. `None` = the default seat.
    pub fn spill_seat(&self, index: usize) -> Option<usize> {
        let l = self.layers.get(index)?;
        if l.draws_over.is_empty() {
            return None;
        }
        let ff = self.enclosing_frame_folder(index)?;
        ((ff + 1)..self.layers.len())
            .filter(|&j| l.draws_over.contains(&self.layers[j].id))
            .next_back()
    }

    /// The cascade made explicit: "draws over the layer at `top`" means
    /// "draws over everything from the frame folder header up to `top`",
    /// because paint order is a stack and there is no drawing a layer twice.
    /// `None` = back to the default seat (the empty set).
    pub fn draws_over_cascade(&self, index: usize, top: Option<usize>) -> BTreeSet<u64> {
        let (Some(top), Some(ff)) = (top, self.enclosing_frame_folder(index)) else {
            return BTreeSet::new();
        };
        ((ff + 1)..=top.min(self.layers.len().saturating_sub(1)))
            .filter(|&j| j != index)
            .map(|j| self.layers[j].id)
            .collect()
    }

    /// Move the breakout layer's insertion marker (item 3's one control).
    /// Refuses anything that is not a breakout layer, so a stale set can
    /// never be written onto a layer that would silently ignore it. One
    /// undo press: the Structure snapshot carries the whole stack, and the
    /// set lives on `Layer`.
    pub fn set_layer_spill_seat(&mut self, index: usize, top: Option<usize>) -> bool {
        if self.spill_anchor(index).is_none() {
            return false;
        }
        let want = self.draws_over_cascade(index, top);
        if self.layers[index].draws_over == want {
            return false;
        }
        let before = self.stack_snapshot();
        let active_before = self.active;
        self.layers[index].draws_over = want;
        self.record_structure("Breakout draws over", before, active_before);
        self.touch();
        true
    }

    /// `draws_over` with every dead id dropped — the SAVE-TIME prune the
    /// stable-id mint's doc comment owes. A layer deleted in one session
    /// frees its id; the mint restarts next session and `ensure_ids` can
    /// hand that number to somebody else, so a cross-reference to it must
    /// never reach the file.
    pub fn live_draws_over(&self, index: usize) -> BTreeSet<u64> {
        let live: std::collections::HashSet<u64> = self.layers.iter().map(|l| l.id).collect();
        self.layers
            .get(index)
            .map(|l| l.draws_over.iter().copied().filter(|id| live.contains(id)).collect())
            .unwrap_or_default()
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
        // STILL clears (2026-08-21, the structural-undo round): the canvas
        // SIZE is not in a `UndoGroup::Structure` snapshot, and dropped
        // outside-tiles are destroyed for real — a resize is the one
        // structural change undo genuinely cannot express yet.
        self.clear_history();
        self.touch();
    }

    /// CSP's Image ▸ **Change image resolution** (`IO-060`), the other half
    /// of the pair whose first half is [`Self::resize_canvas`]: the paper
    /// stays the same PHYSICAL page and every pixel is re-made at a new
    /// resolution. Nothing is re-framed and nothing is cropped.
    ///
    /// Returns `false` (document untouched) for a degenerate target.
    ///
    /// # Why this is not "scale everything by the same number"
    ///
    /// * RASTER content resamples through `interp` — [`Interp::HighAccuracy`]
    ///   is the reduction kernel and the reason a 1 px hairline comes through
    ///   grey instead of missing.
    /// * DERIVED content does not resample: a tone layer's dots, a live
    ///   fill, a border effect and a frame folder's mat are all dropped here
    ///   and rebuilt by `refresh_derived(new_dpi)` / the re-derive below.
    ///   That is the whole tone-awareness the JP guides ask for — the screen
    ///   re-flows at the new dpi at the SAME lpi rather than being filtered
    ///   like a photograph.
    /// * VECTOR geometry scales and re-derives (`Layer::scale_vectors`).
    /// * PHYSICAL numbers — lpi, pt — do not move at all.
    ///
    /// The caller owns `PageSetup`: `dpi` changes, `paper_mm` does not.
    ///
    /// Like every structural op the history is cleared — a whole-work
    /// resample is not an undo step, it is a decision (see `resize_to`).
    pub fn resample_to(&mut self, new_w: u32, new_h: u32, interp: crate::transform::Interp) -> bool {
        let old = self.size;
        let new = (new_w.max(1), new_h.max(1));
        if old.0 == 0 || old.1 == 0 {
            return false;
        }
        if new == old {
            return true;
        }
        let sx = new.0 as f32 / old.0 as f32;
        let sy = new.1 as f32 / old.1 as f32;
        for l in &mut self.layers {
            // A canvas-filling uniform-white layer is a frame folder's White
            // base (and a new page's paper): re-lay it at the new size
            // rather than resampling a page of solid white, which would only
            // buy a fringe of half-alpha along the edges.
            if l.covers_canvas(old) && l.is_uniform_white() {
                l.tiles.clear();
                l.extend_white(new);
                l.resample_meta(sx, sy, interp);
                continue;
            }
            l.resample_content(sx, sy, interp);
        }
        // Vector rasters rebuild from the geometry that just scaled — the
        // resampled blit `resample_content` produced is discarded here.
        // Text is the exception: its sprites are shaped by the APP at a dpi
        // the core cannot reach, so a re-raster now would blank the layer.
        // The resampled pixels stand in until the app re-warms the caches.
        for l in &mut self.layers {
            if l.is_frame() {
                Self::derive_frame_raster(l, new);
            } else if let Some(bs) = l.balloons().cloned() {
                let raster = bs.rasterize(new);
                l.replace_tiles(raster);
            }
        }
        self.rulers.scale(sx, sy);
        self.size = new;
        self.selection = None;
        self.sel_scratch = LayerMask::default();
        self.clear_history();
        self.touch();
        true
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

    /// Tiles per freeform batch — the same 256 the compositor's
    /// `UPLOAD_BATCH`, `correction::DERIVE_BATCH` and the kernel's
    /// `TILE_BATCH` use: 1 Mpx, 8 MB of pixels, one dispatch's worth.
    ///
    /// Also the granularity of a kernel's decline, so a page whose densest
    /// batch overflows a segment cap still runs every other batch on the
    /// GPU.
    const FREEFORM_BATCH: usize = 256;

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
                let (tx1, ty1) = (
                    (ox + TILE_SIZE as i32) as f32,
                    (oy + TILE_SIZE as i32) as f32,
                );
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

    /// `FI-050` — the FREEFORM gradient. The ramp runs from guide polyline
    /// `l1` (parameter 0) to guide polyline `l2` (parameter 1), FOLLOWING
    /// their shapes: every pixel takes the ratio of its distances to the two
    /// guides. Same layer rules, selection clip and single undo step as
    /// [`Self::paint_gradient_ramp`]. False on a refusing layer, or when
    /// either guide has no usable point.
    ///
    /// See [`crate::freeform`] for the geometry, the per-tile segment cull
    /// that makes a full page affordable, and why this needs no
    /// anti-aliasing switch on the guides (they are never pixels).
    ///
    /// Unlike the linear ramp there is no tile to skip: the parameter is
    /// defined over the whole canvas, so "do not draw" has no outside to
    /// leave alone and every selected tile is painted.
    pub fn paint_gradient_freeform(
        &mut self,
        l1: &[[f32; 2]],
        l2: &[[f32; 2]],
        ramp: &crate::gradient::Ramp,
    ) -> bool {
        self.paint_gradient_freeform_with(l1, l2, ramp, &mut |_, _| None)
    }

    /// [`Self::paint_gradient_freeform`] with a kernel lent by the caller —
    /// the GPU seam's door into the freeform field.
    ///
    /// The batch is [`FREEFORM_BATCH`] tiles at a time, each carrying its own
    /// CULLED segment lists (`Window::pack_into`) so a kernel does exactly
    /// the work the CPU loop does and no more. `run` sees the flattened
    /// field plus the batch's CURRENT pixels and returns the painted ones —
    /// src-over included, because the destination is what it was handed —
    /// or `None` to decline the batch, in which case the loop below paints
    /// it. Declining per batch rather than per page is deliberate: a
    /// segment pool that overflows the kernel's cap on ONE dense batch does
    /// not cost the rest of the page its speed-up.
    ///
    /// Undo is untouched: every tile still goes through `tile_mut`, which is
    /// what stashes the pre-image, and the whole apply is one `begin_op` /
    /// `end_op` bracket whoever painted the pixels.
    pub fn paint_gradient_freeform_with(
        &mut self,
        l1: &[[f32; 2]],
        l2: &[[f32; 2]],
        ramp: &crate::gradient::Ramp,
        run: &mut crate::freeform::FieldKernel<'_>,
    ) -> bool {
        if !self.paint_guard() {
            return false;
        }
        let Some(field) = crate::freeform::Freeform::new(l1, l2) else {
            return false;
        };
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let sel = self.selection.clone();
        let li = self.active;
        let lock_alpha = self.layers[li].lock_alpha;
        // Half the diagonal of one tile: no pixel in a tile is further than
        // this from its centre, which is exactly what the cull's bound needs.
        let hd = TILE_SIZE as f32 * 0.5 * std::f32::consts::SQRT_2;
        let half = TILE_SIZE as f32 * 0.5;
        let todo: Vec<TileIdx> = (0..(h + TILE_SIZE as i32 - 1) / TILE_SIZE as i32)
            .flat_map(|ty| {
                (0..(w + TILE_SIZE as i32 - 1) / TILE_SIZE as i32).map(move |tx| TileIdx::new(tx, ty))
            })
            .filter(|idx| sel.as_ref().is_none_or(|s| s.tile_mask(*idx).is_some()))
            .collect();
        self.begin_op();
        for chunk in todo.chunks(Self::FREEFORM_BATCH) {
            // Cull BEFORE `tile_mut`: a guide with hundreds of segments is
            // otherwise re-scanned 4096 times per tile.
            let mut pool: Vec<f32> = Vec::new();
            let mut plans = Vec::with_capacity(chunk.len());
            let mut wins = Vec::with_capacity(chunk.len());
            for &idx in chunk {
                let (ox, oy) = idx.origin();
                let win = field.window([ox as f32 + half, oy as f32 + half], hd);
                plans.push(win.pack_into(&mut pool, (ox, oy)));
                wins.push(win);
            }
            // The tiles as they stand — the kernel's source AND destination.
            let mut px = vec![0u16; chunk.len() * crate::tile::TILE_LEN];
            for (n, &idx) in chunk.iter().enumerate() {
                if let Some(t) = self.layers[li].tile(idx) {
                    px[n * crate::tile::TILE_LEN..(n + 1) * crate::tile::TILE_LEN]
                        .copy_from_slice(t.data());
                }
            }
            let job = crate::freeform::FieldJob {
                segs: &pool,
                plans: &plans,
                ramp,
                size: self.size,
            };
            // A host that hands back the wrong length is a bug on its side,
            // never a reason to write short tiles — same guard the
            // correction derive puts on its lent kernel.
            let lent = run(&job, &px).filter(|o| o.len() == px.len());
            for (n, &idx) in chunk.iter().enumerate() {
                let tile = self.layers[li].tile_mut(idx);
                let data = tile.data_mut();
                if let Some(out) = &lent {
                    data.copy_from_slice(
                        &out[n * crate::tile::TILE_LEN..(n + 1) * crate::tile::TILE_LEN],
                    );
                    continue;
                }
                let (ox, oy) = idx.origin();
                for p in 0..TILE_SIZE * TILE_SIZE {
                    let x = ox + (p % TILE_SIZE) as i32;
                    let y = oy + (p / TILE_SIZE) as i32;
                    if x >= w || y >= h {
                        continue;
                    }
                    let t = wins[n].t_at([x as f32 + 0.5, y as f32 + 0.5]);
                    let s = ramp.eval_unit(t, x, y);
                    // Premultiply, for the reason spelled out in
                    // `paint_gradient_ramp`.
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

    /// `FI-051` — the freeform gradient with THREE OR MORE guides, each
    /// carrying its own colour. Every pixel is the inverse-distance blend of
    /// the guide colours, so the field follows all of the drawn shapes at
    /// once ([`crate::freeform::Multi`] for the maths and the cull).
    ///
    /// TWO guides are NOT blended here: they route to
    /// [`Self::paint_gradient_freeform`], the shipped ramp path, which is
    /// what carries the ramp's interior stops, flip and edge process. The
    /// caller passes `ramp` for that case; with three or more guides only
    /// its mixing space, brightness lift and dither are read (module doc).
    ///
    /// Same layer rules, selection clip and single undo step as the two-line
    /// form. False on a refusing layer, on fewer than two guides, or when
    /// any guide has no usable point.
    pub fn paint_gradient_freeform_multi(
        &mut self,
        guides: &[crate::freeform::ColourGuide],
        ramp: &crate::gradient::Ramp,
    ) -> bool {
        self.paint_gradient_freeform_multi_with(guides, ramp, &mut |_, _| None)
    }

    /// [`Self::paint_gradient_freeform_multi`] with a kernel lent by the
    /// caller.
    ///
    /// The kernel reaches the TWO-guide path only. `FI-051`'s N-guide field
    /// is an inverse-distance blend whose colour stage is a SEQUENTIAL mix —
    /// `idw_colour` folds guide k into the running average with
    /// `mix::mix_rgba`, and two of the three mixing spaces are `powf`/`cbrt`
    /// — so it has no exact-parity kernel form the way the two-line ramp
    /// does. It runs the CPU reference, which is where it is exact.
    pub fn paint_gradient_freeform_multi_with(
        &mut self,
        guides: &[crate::freeform::ColourGuide],
        ramp: &crate::gradient::Ramp,
        run: &mut crate::freeform::FieldKernel<'_>,
    ) -> bool {
        if let [a, b] = guides {
            // The pinned two-line path, byte for byte — the ramp's ends are
            // the two guides' colours, which is where the gesture put them.
            let ramp = crate::gradient::Ramp::new(a.colour, b.colour, ramp.mid, ramp.opts);
            return self.paint_gradient_freeform_with(&a.pts, &b.pts, &ramp, run);
        }
        if guides.len() < 2 || !self.paint_guard() {
            return false;
        }
        let Some(field) = crate::freeform::Multi::new(guides) else {
            return false;
        };
        let (mix, bright, dither) = (ramp.opts.mix, ramp.opts.bright, ramp.opts.dither);
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let sel = self.selection.clone();
        let li = self.active;
        let lock_alpha = self.layers[li].lock_alpha;
        let hd = TILE_SIZE as f32 * 0.5 * std::f32::consts::SQRT_2;
        let half = TILE_SIZE as f32 * 0.5;
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
                // Cull BEFORE `tile_mut`, as the two-line form does.
                let win = field.window([ox as f32 + half, oy as f32 + half], hd);
                let tile = self.layers[li].tile_mut(idx);
                let data = tile.data_mut();
                for p in 0..TILE_SIZE * TILE_SIZE {
                    let x = ox + (p % TILE_SIZE) as i32;
                    let y = oy + (p / TILE_SIZE) as i32;
                    if x >= w || y >= h {
                        continue;
                    }
                    let mut s = win.colour_at([x as f32 + 0.5, y as f32 + 0.5], mix, bright);
                    if dither {
                        // No ramp parameter to be "inside" of — the whole
                        // field is interior, so the noise applies flat.
                        let d = crate::gradient::dither_offset(x, y) / 255.0;
                        for v in s.iter_mut() {
                            *v = (*v + d).clamp(0.0, 1.0);
                        }
                    }
                    // Premultiply, for the reason spelled out in
                    // `paint_gradient_ramp`.
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

/// Bake `upper` down onto `dst`, honouring the upper layer's blend mode and
/// opacity — the pixel half of every merge. A hidden upper layer contributes
/// nothing (CSP: it merges as if it were not there).
///
/// One definition on purpose: [`Document::merge_down`] and
/// [`Document::merge_selected`] must agree pixel for pixel, or merging a
/// two-row selection would come out different from merging down.
/// Write a straight RGBA8 image into `layer`'s tiles, centred on a canvas
/// of `size` (oversized images are clipped; fully transparent pixels leave
/// no tile behind). The one image→tiles door, shared by import, stamp,
/// flatten and folder merge.
fn fill_layer_from_image(layer: &mut Layer, size: (u32, u32), img: &image::RgbaImage) {
    let (w, h) = (size.0 as i64, size.1 as i64);
    let ox = (w - img.width() as i64) / 2;
    let oy = (h - img.height() as i64) / 2;
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
}

fn bake_layer_into(dst: &mut Layer, upper: &Layer) {
    use crate::blend::{blend_premul, f32_to_fix15, fix15_to_f32, scale_opacity};
    if !upper.visible {
        return;
    }
    for (idx, tile) in upper.tiles() {
        if tile.is_blank() {
            continue;
        }
        let sd = tile.data();
        let dd = dst.tile_mut(idx).data_mut();
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

    /// LM-001: the starter mask is all-visible.
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

    /// The bake matches the screen. A mask only has tiles where the layer
    /// had ink at creation; ink painted AFTER lands in tiles the mask never
    /// covers, which both compositors render VISIBLE. Apply-mask used to
    /// treat those absent tiles as coverage 0 and silently DELETE that ink
    /// — shown one moment, gone after the bake. Verified failing against
    /// the old `unwrap_or(0)`.
    #[test]
    fn bake_keeps_ink_painted_after_the_mask_was_made() {
        let mut doc = Document::new(192, 128);
        doc.begin_op();
        let t = doc.layers[0].tile_mut(TileIdx::new(0, 0));
        t.set_pixel(5, 5, [32768, 0, 0, 32768]);
        doc.end_op();
        assert!(doc.mask_selection_blank(0), "mask over tile (0,0) only");

        // New ink in a tile the mask has no entry for.
        doc.begin_op();
        let t = doc.layers[0].tile_mut(TileIdx::new(2, 0));
        t.set_pixel(10, 5, [0, 32768, 0, 32768]);
        doc.end_op();
        let before = composite(&doc, Background::Transparent);
        assert_eq!(before.get_pixel(138, 5).0[3], 255, "on screen before");

        assert!(doc.mask_apply_bake(0));
        assert!(doc.layers[0].mask.is_none(), "bake consumed the mask");
        let after = composite(&doc, Background::Transparent);
        assert_eq!(after.get_pixel(5, 5).0[3], 255, "masked-visible ink kept");
        assert_eq!(
            after.get_pixel(138, 5).0[3],
            255,
            "ink painted after the mask must survive the bake"
        );
    }
}

#[cfg(test)]
mod tests {    use super::*;

    /// Row 33: rasterizing a text layer keeps its RENDERED tiles and
    /// drops the vector state; keep-original leaves the source beside
    /// it; ONE structural undo restores the stack.
    #[test]
    fn convert_layer_rasterizes_and_undoes_in_one_step() {
        let mut doc = Document::new(128, 128);
        let t = crate::text::TextItem::new([10.0, 10.0], "Gothic".into(), 9.0, [0, 0, 0], true);
        let li = doc.add_text_layer("lettering", crate::text::TextSet { texts: vec![t] });
        assert!(matches!(doc.layers[li].kind, crate::doc::LayerKind::Text(_)));
        let had_tiles = doc.layers[li].tiles().count() > 0;

        let ok = doc.convert_layer(
            li,
            true,
            Some(crate::doc::LayerExpression::Grey),
            None,
            true,
            Some("baked lettering".into()),
        );
        assert!(ok);
        assert!(matches!(doc.layers[li + 1].kind, crate::doc::LayerKind::Raster), "the copy is raster");
        assert!(matches!(doc.layers[li].kind, crate::doc::LayerKind::Text(_)), "the original stays text");
        assert_eq!(doc.layers[li + 1].name, "baked lettering");
        assert_eq!(doc.layers[li + 1].expression, crate::doc::LayerExpression::Grey);
        assert_eq!(doc.layers[li + 1].tiles().count() > 0, had_tiles, "the rendered tiles came along");

        assert!(doc.undo(), "one undo");
        // A fresh document carries a base layer: the stack is back to
        // base + the original text layer.
        assert_eq!(doc.layers.len(), 2, "the copy is gone");
        assert!(matches!(doc.layers[1].kind, crate::doc::LayerKind::Text(_)));

        // Replace mode: no new layer, the layer itself converts.
        let ok = doc.convert_layer(li, true, None, None, false, None);
        assert!(ok);
        assert!(matches!(doc.layers[li].kind, crate::doc::LayerKind::Raster));
    }

    /// Row 31: extraction keeps the DARK pixels as scaled-black lines
    /// and drops the light ones; a fresh layer above; one undo.
    #[test]
    fn extract_lines_lifts_the_dark_ink() {
        let mut doc = Document::new(128, 128);
        let li = doc.add_layer("scan");
        let put = |doc: &mut Document, x: i32, y: i32, v: f32| {
            let idx = crate::tile::TileIdx::of_pixel(x, y);
            let (ox, oy) = idx.origin();
            let t = doc.layers[li].tile_mut(idx);
            let f = crate::blend::f32_to_fix15(v);
            t.set_pixel((x - ox) as usize, (y - oy) as usize, [f, f, f, crate::blend::f32_to_fix15(1.0)]);
        };
        put(&mut doc, 10, 10, 0.0); // a black line pixel
        put(&mut doc, 12, 10, 0.5); // mid grey
        put(&mut doc, 14, 10, 0.95); // paper
        let out = doc.extract_lines(li, 0.8).expect("lines extracted");
        assert_eq!(out, li + 1, "the new layer sits above");
        let get = |doc: &Document, x: i32, y: i32| -> u16 {
            let idx = crate::tile::TileIdx::of_pixel(x, y);
            let (ox, oy) = idx.origin();
            doc.layers[out]
                .tile_arc(idx)
                .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize)[3])
                .unwrap_or(0)
        };
        let black = get(&doc, 10, 10);
        let grey = get(&doc, 12, 10);
        assert_eq!(black, crate::blend::f32_to_fix15(1.0), "black is a full line");
        assert!(
            grey > crate::blend::f32_to_fix15(0.3) && grey < crate::blend::f32_to_fix15(0.4),
            "mid grey is a ~0.375 line: {grey}"
        );
        assert_eq!(get(&doc, 14, 10), 0, "paper dropped");
        assert!(doc.undo(), "one undo");
        assert_eq!(doc.layers.len(), 2, "the extraction layer is gone");
        assert_ne!(doc.layers[1].name, "Extracted lines");
    }


    use crate::tile::FIX15_ONE;

    /// The ruler set is document state with ONE undo history: a recorded
    /// gesture undoes to the exact set that was there before it, redoes to
    /// the finished one, and a gesture that changed nothing records
    /// nothing. Document-level, so it belongs to no layer.
    #[test]
    fn rulers_undo_and_redo_the_whole_set() {
        let mut doc = Document::new(64, 64);
        assert!(doc.rulers.items.is_empty(), "a fresh document has none");

        let before = doc.rulers.clone();
        doc.rulers.items.push(crate::ruler::Ruler::Line {
            a: [0.0, 0.0],
            b: [10.0, 0.0],
        });
        doc.rulers.on = true;
        assert!(doc.record_rulers(before, "Add ruler"));
        assert_eq!(doc.undo_labels(), ["Add ruler"]);

        // A gesture that ended where it started is not a step.
        let noop = doc.rulers.clone();
        assert!(!doc.record_rulers(noop, "Move ruler"));
        assert_eq!(doc.undo_labels().len(), 1);

        // Move it, then undo back to the exact geometry.
        let before = doc.rulers.clone();
        doc.rulers.items[0].translate([0.0, 25.0]);
        assert!(doc.record_rulers(before, "Move ruler"));
        assert!(doc.undo());
        assert_eq!(
            doc.rulers.items[0],
            crate::ruler::Ruler::Line {
                a: [0.0, 0.0],
                b: [10.0, 0.0]
            }
        );
        assert!(doc.redo(), "and redo puts the move back");
        assert_eq!(
            doc.rulers.items[0],
            crate::ruler::Ruler::Line {
                a: [0.0, 25.0],
                b: [10.0, 25.0]
            }
        );
        assert_eq!(doc.redo_labels(), Vec::<String>::new());

        // Undo the move AND the creation: back to nothing, snap switch and
        // all — the snapshot is the whole value.
        assert!(doc.undo());
        assert!(doc.undo());
        assert!(doc.rulers.items.is_empty());
        assert!(!doc.rulers.on);
    }

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

    /// The brush-preset key spelling round-trips every mode too (the
    /// `.myb` parser used to accept multiply/screen only), and keeps the
    /// two legacy spellings plus the unknown-string fallback.
    #[test]
    fn every_blend_mode_round_trips_through_its_short_name() {
        for b in Blend::ALL {
            assert_eq!(Blend::from_short_name(b.short_name()), b, "{b:?}");
        }
        assert_eq!(Blend::from_short_name("multiply"), Blend::Multiply);
        assert_eq!(Blend::from_short_name("screen"), Blend::Screen);
        assert_eq!(Blend::from_short_name("from-a-newer-build"), Blend::Normal);
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
            full: false,
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
        let li = doc.add_text_layer("Text 1", one);
        assert!(doc.layers[li].is_text() && doc.layers[li].is_vector());
        assert!(doc.layers[li].tile_count() > 0, "sprite rasterized");
        // The commit door minted the item's id; clones of the DOC's set are
        // how the app really edits, and they keep that identity.
        let one = doc.layers[li].texts().unwrap().clone();
        assert_ne!(one.texts[0].id, 0, "commit mints");

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

    /// Stable ids (the automation round): minted unique, copies are new
    /// identities, and undo restores a deleted layer WITH its id — the
    /// property an id-holding automation client depends on.
    #[test]
    fn stable_layer_ids_mint_survive_reorder_and_undo() {
        let mut doc = Document::default();
        doc.add_layer("2");
        doc.add_layer("3");
        let ids: Vec<u64> = doc.layers.iter().map(|l| l.id()).collect();
        assert!(ids.iter().all(|&i| i != 0), "every layer has a real id");
        let mut uniq = ids.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), ids.len(), "ids are unique");

        // Reorder: identity follows the layer, not the slot.
        let moved = ids[2];
        assert!(doc.move_layer(2, 0));
        assert_eq!(doc.layer_index_of(moved), Some(0));

        // Duplicate: the copy is a NEW identity.
        let src = doc.layers[0].id();
        let at = doc.duplicate_layer(0).unwrap();
        assert_ne!(doc.layers[at].id(), src);
        assert_eq!(doc.layer_index_of(src), Some(0), "original keeps its id");

        // Delete + undo: the SAME identity comes back.
        let gone = doc.layers[at].id();
        assert!(doc.remove_layer(at));
        assert_eq!(doc.layer_index_of(gone), None);
        assert!(doc.undo());
        assert_eq!(doc.layer_index_of(gone), Some(at), "undo restores the id");

        // Convert-keeping-original: the kept copy is a new identity too.
        let before = doc.layers[0].id();
        assert!(doc.convert_layer(0, true, None, None, true, None));
        assert_eq!(doc.layers[0].id(), before);
        assert_ne!(doc.layers[1].id(), before);
    }

    /// `ensure_ids` (the ORA loader's heal): zeros and duplicates remint —
    /// first occurrence keeps the id — and the mint is lifted first, so a
    /// healed id can never equal one the file also holds.
    #[test]
    fn ensure_ids_heals_zeros_and_duplicates() {
        use crate::text::{TextItem, TextSet};
        let mut doc = Document::default();
        doc.add_layer("2");
        doc.add_layer("3");
        let item = || TextItem::new([0.0, 0.0], "Meiryo".into(), 12.0, [0, 0, 0], false);
        doc.add_text_layer("T", TextSet { texts: vec![item(), item(), item()] });
        // Fake a hand-edited / pre-id file's stack, BEHIND the commit doors
        // (they would heal on the way in). Uniqueness is PER COLLECTION —
        // lookups are typed — so a layer and an item may share a number;
        // only siblings must differ.
        doc.layers[0].set_id(7);
        doc.layers[1].set_id(7);
        doc.layers[2].set_id(0);
        if let LayerKind::Text(ts) = &mut doc.layers[3].kind {
            ts.texts[0].id = 9;
            ts.texts[1].id = 9;
            ts.texts[2].id = 0;
        }
        doc.ensure_ids();
        let mut lids: Vec<u64> = doc.layers.iter().map(|l| l.id()).collect();
        assert_eq!(doc.layers[0].id(), 7, "first occurrence keeps the id");
        assert!(lids.iter().all(|&i| i != 0));
        let n = lids.len();
        lids.sort();
        lids.dedup();
        assert_eq!(lids.len(), n, "layers healed unique");
        let LayerKind::Text(ts) = &doc.layers[3].kind else {
            unreachable!()
        };
        let iids: Vec<u64> = ts.texts.iter().map(|t| t.id).collect();
        assert_eq!(iids[0], 9, "first occurrence keeps the id");
        assert!(
            iids[1] > 9 && iids[2] > 9,
            "duplicate and zero healed from past the file's max (9): {iids:?}"
        );
    }

    /// A frame folder with one child, plus `n` loose layers stacked above
    /// it. Returns `(escapee, header, the loose indices bottom-first)`.
    fn breakout_stack(n: usize) -> (Document, usize, usize, Vec<usize>) {
        let mut doc = Document::new(64, 64);
        let hdr = doc.add_frame_folder(
            "panel",
            crate::frame::FrameSet::single_rect([8.0, 8.0, 56.0, 56.0], 2.0),
        );
        let burst = hdr - 1;
        assert!(doc.set_layer_escape(burst, true));
        let above: Vec<usize> = (0..n)
            .map(|k| doc.add_layer_above(doc.layers.len() - 1, format!("up{k}")))
            .collect();
        (doc, burst, hdr, above)
    }

    /// Item 3, the invariant: paint order is a stack, so "draws over layer
    /// N" HAS to mean "over everything below N". The set is stored, and the
    /// resolved seat is the topmost member — never a hole in the middle.
    #[test]
    fn the_draws_over_set_only_ever_fills_downward() {
        let (mut doc, burst, hdr, up) = breakout_stack(3);
        assert_eq!(doc.spill_anchor(burst), Some(hdr), "default seat");
        assert_eq!(doc.spill_candidates(burst), up, "only the stack above");

        // Marking the TOP one covers the two below it as well.
        assert!(doc.set_layer_spill_seat(burst, Some(up[2])));
        let set = doc.layers[burst].draws_over.clone();
        for &j in &up {
            assert!(
                set.contains(&doc.layers[j].id()),
                "over {j} is implied by over {}",
                up[2]
            );
        }
        assert_eq!(doc.spill_anchor(burst), Some(up[2]));
        assert_eq!(doc.spill_seat(burst), Some(up[2]), "the marker's position");

        // Moving it down drops the ones above — the set is exactly the run.
        assert!(doc.set_layer_spill_seat(burst, Some(up[0])));
        assert_eq!(
            doc.layers[burst].draws_over,
            BTreeSet::from([doc.layers[up[0]].id()])
        );
        assert_eq!(doc.spill_anchor(burst), Some(up[0]));

        // …and back to the default seat.
        assert!(doc.set_layer_spill_seat(burst, None));
        assert!(doc.layers[burst].draws_over.is_empty());
        assert_eq!(doc.spill_anchor(burst), Some(hdr));

        // A layer that is not bursting out has no seat to move.
        assert!(!doc.set_layer_spill_seat(up[0], Some(up[2])));
    }

    /// The set is keyed on STABLE IDS, so a reorder or a delete leaves the
    /// marker on the same art — the whole reason part 2 waited for ids.
    #[test]
    fn the_draws_over_set_survives_reorder_and_delete() {
        let (mut doc, burst, hdr, up) = breakout_stack(3);
        assert!(doc.set_layer_spill_seat(burst, Some(up[1])));
        let kept = doc.layers[up[1]].id();

        // Delete the layer between the marker and the panel: the marker
        // stays on the same art, one row lower.
        // (`burst` sits BELOW everything removed here, so its own index
        // never moves — the ids are what has to do the work.)
        assert!(doc.remove_layer(up[0]));
        let now = doc.layer_index_of(kept).expect("still there");
        assert_eq!(doc.spill_anchor(burst), Some(now), "same art, new index");
        assert!(now < up[1], "and it really did move");

        // Delete the marker itself: the seat falls back, never dangles.
        assert!(doc.remove_layer(now));
        assert!(doc.layer_index_of(kept).is_none());
        let seat = doc.spill_anchor(burst).expect("still a breakout");
        assert!(seat < doc.layers.len(), "the seat is a live index");
        assert_ne!(seat, hdr + 99, "sanity");
    }

    /// A covered layer living inside somebody ELSE's sealed folder lifts to
    /// that folder's header: inside the group the burst would be clipped by
    /// the very panel it is spilling over.
    #[test]
    fn a_covered_layer_inside_a_sealed_folder_lifts_to_its_header() {
        let mut doc = Document::new(64, 64);
        let hdr = doc.add_frame_folder(
            "lower",
            crate::frame::FrameSet::single_rect([8.0, 32.0, 56.0, 56.0], 2.0),
        );
        let burst = hdr - 1;
        assert!(doc.set_layer_escape(burst, true));
        let up = doc.add_frame_folder(
            "upper",
            crate::frame::FrameSet::single_rect([8.0, 8.0, 56.0, 24.0], 2.0),
        );
        let inside = up - 1;
        assert!(doc.set_layer_spill_seat(burst, Some(inside)));
        assert_eq!(
            doc.spill_anchor(burst),
            Some(up),
            "lifted out of the upper panel's seal to its header"
        );
    }

    /// One toggle, one undo press — the set lives on `Layer`, so the
    /// Structure snapshot carries it.
    #[test]
    fn a_draws_over_toggle_is_one_undo_press() {
        let (mut doc, burst, hdr, up) = breakout_stack(2);
        assert!(doc.set_layer_spill_seat(burst, Some(up[0])));
        assert!(doc.set_layer_spill_seat(burst, Some(up[1])));
        assert_eq!(doc.spill_anchor(burst), Some(up[1]));

        doc.undo();
        assert_eq!(doc.spill_anchor(burst), Some(up[0]), "one press, one step");
        doc.undo();
        assert!(doc.layers[burst].draws_over.is_empty());
        assert_eq!(doc.spill_anchor(burst), Some(hdr), "back to the default");
        doc.redo();
        assert_eq!(doc.spill_anchor(burst), Some(up[0]), "redo restores it");

        // Setting the value it already has is not a step.
        let before = doc.spill_anchor(burst);
        assert!(!doc.set_layer_spill_seat(burst, Some(up[0])));
        assert_eq!(doc.spill_anchor(burst), before);
    }

    /// The shared walk: a mask-capped breakout appears TWICE (held-in at
    /// its own seat, spilled at the anchor's), an uncapped one once, and
    /// every other layer exactly once.
    #[test]
    fn composite_order_splits_only_a_mask_capped_breakout() {
        let (mut doc, burst, hdr, up) = breakout_stack(1);
        let steps = doc.composite_order();
        assert_eq!(steps.len(), doc.layers.len(), "uncapped: one step each");
        let seat = steps.iter().position(|s| s.layer == burst).unwrap();
        let hdr_at = steps.iter().position(|s| s.layer == hdr).unwrap();
        assert_eq!(seat, hdr_at + 1, "right after its frame folder header");
        assert_eq!(steps[seat].depth, doc.layers[hdr].depth);
        assert!(steps.iter().all(|s| s.part == SpillPart::All));

        // Give it a mask: now it is two steps, and only two.
        doc.layers[burst].tile_mut(TileIdx::new(0, 0));
        assert!(doc.mask_selection_blank(burst));
        let steps = doc.composite_order();
        assert_eq!(steps.len(), doc.layers.len() + 1);
        let parts: Vec<SpillPart> = steps
            .iter()
            .filter(|s| s.layer == burst)
            .map(|s| s.part)
            .collect();
        assert_eq!(parts, vec![SpillPart::In, SpillPart::Out]);
        let held = steps.iter().position(|s| s.part == SpillPart::In).unwrap();
        let out = steps.iter().position(|s| s.part == SpillPart::Out).unwrap();
        assert!(held < out, "the held half walks in place, first");
        assert_eq!(steps[held].depth, doc.layers[burst].depth, "its own depth");

        // And the seat still moves with the set.
        assert!(doc.set_layer_spill_seat(burst, Some(up[0])));
        let steps = doc.composite_order();
        let out = steps.iter().position(|s| s.part == SpillPart::Out).unwrap();
        let anchor = steps.iter().position(|s| s.layer == up[0]).unwrap();
        assert_eq!(out, anchor + 1, "right after the layer it draws over");
    }

    /// Item ids: the commit doors mint fresh (id 0) and cloned (duplicate
    /// id) items, existing items keep their identity across edits, and
    /// `index_of_id` tracks an item through a reorder of its set.
    #[test]
    fn text_and_balloon_item_ids_mint_and_stay_stable() {
        use crate::balloon::{Balloon, BalloonSet};
        use crate::text::{TextItem, TextSet};
        let mut doc = Document::default();
        let item = TextItem::new([0.0, 0.0], "Meiryo".into(), 12.0, [0, 0, 0], false);
        let li = doc.add_text_layer("T", TextSet { texts: vec![item] });
        let ts = doc.layers[li].texts().unwrap().clone();
        let id0 = ts.texts[0].id;
        assert_ne!(id0, 0);

        // A cloned item (duplicate id) and a fresh one (id 0) both mint;
        // the original keeps its id.
        let mut edited = ts.clone();
        let mut dup = edited.texts[0].clone();
        dup.pos = [50.0, 50.0];
        edited.texts.push(dup);
        edited
            .texts
            .push(TextItem::new([9.0, 9.0], "Meiryo".into(), 12.0, [0, 0, 0], false));
        assert!(doc.set_texts(li, edited));
        let got = doc.layers[li].texts().unwrap();
        assert_eq!(got.texts[0].id, id0, "original identity kept");
        let all: Vec<u64> = got.texts.iter().map(|t| t.id).collect();
        assert!(all.iter().all(|&i| i != 0));
        let mut uniq = all.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), all.len(), "minted unique");
        assert_eq!(got.index_of_id(id0), Some(0));

        // Balloons: same contract through their door.
        let bli = doc.add_balloon_layer(
            "B",
            BalloonSet {
                balloons: vec![Balloon::default(), Balloon::default()],
                border_px: 4.0,
                pressure_width: false,
            },
        );
        let bs = doc.layers[bli].balloons().unwrap().clone();
        assert!(bs.balloons.iter().all(|b| b.id != 0));
        assert_ne!(bs.balloons[0].id, bs.balloons[1].id);
        // Reorder inside the set: the id still finds the item.
        let follow = bs.balloons[0].id;
        let mut swapped = bs.clone();
        swapped.balloons.swap(0, 1);
        assert!(doc.set_balloons(bli, swapped));
        assert_eq!(
            doc.layers[bli].balloons().unwrap().index_of_id(follow),
            Some(1)
        );
    }

    #[test]
    fn structural_layer_ops_record_and_the_history_survives() {
        // The 2026-08-21 model: a structural op pushes a Structure snapshot
        // instead of clearing. Undo is LIFO, so the paint group recorded
        // BEFORE the add is still valid once the add's swap has restored
        // the one-layer stack it was recorded against.
        let mut doc = Document::default();
        doc.begin_op();
        paint(&mut doc, TileIdx::new(0, 0), 10);
        doc.end_op();
        assert!(doc.can_undo());
        doc.add_layer("2");
        assert_eq!(doc.layers.len(), 2);
        assert_eq!(doc.undo_len(), 2, "the paint step survived the add");
        assert!(doc.next_undo_is_structure(), "the add is on top");
        assert!(doc.undo(), "undo the add");
        assert_eq!(doc.layers.len(), 1, "the new layer is gone");
        assert!(doc.undo(), "…then undo the paint on the restored stack");
        assert!(doc.layers[0].tiles().next().is_none(), "paint took back");
        assert!(doc.redo() && doc.redo(), "the whole chain replays forward");
        assert_eq!(doc.layers.len(), 2);
    }

    /// The full LIFO round trip across several structural shapes: every op
    /// undoes in reverse order and redoes forward, and pixel groups
    /// recorded between structural steps restore into the right layers.
    #[test]
    fn structural_undo_round_trips_a_mixed_chain() {
        let mut doc = Document::default();
        doc.begin_op();
        paint(&mut doc, TileIdx::new(0, 0), 10);
        doc.end_op();
        doc.add_layer("2");
        doc.begin_op();
        paint(&mut doc, TileIdx::new(1, 0), 20);
        doc.end_op();
        let dup = doc.duplicate_layer(doc.active).unwrap();
        assert_eq!(doc.layers.len(), 3);
        assert!(doc.remove_layer(dup));
        assert_eq!(doc.layers.len(), 2);
        assert_eq!(doc.undo_len(), 5);
        for _ in 0..5 {
            assert!(doc.undo());
        }
        assert_eq!(doc.layers.len(), 1, "back to the single empty layer");
        assert!(
            doc.layers[0].tiles().next().is_none(),
            "first paint undone last"
        );
        assert!(!doc.can_undo());
        for _ in 0..5 {
            assert!(doc.redo());
        }
        assert_eq!(doc.layers.len(), 2, "dup redone, then its removal");
        assert!(
            doc.layers[1].tiles().next().is_some(),
            "second paint replayed into the re-added layer"
        );
    }

    /// Merge-down is destructive on the lower layer's pixels; the Structure
    /// snapshot holds the pre-merge tiles by Arc, so undo restores both
    /// layers exactly.
    #[test]
    fn merge_down_undoes_to_both_layers() {
        let mut doc = Document::default();
        doc.begin_op();
        paint(&mut doc, TileIdx::new(0, 0), 10);
        doc.end_op();
        doc.add_layer("upper");
        doc.begin_op();
        paint(&mut doc, TileIdx::new(0, 0), 200);
        doc.end_op();
        assert!(doc.merge_down(1));
        assert_eq!(doc.layers.len(), 1);
        assert!(doc.undo(), "undo the merge");
        assert_eq!(doc.layers.len(), 2, "upper layer is back");
        assert_eq!(
            px(&doc, 0, TileIdx::new(0, 0)).unwrap()[0],
            10,
            "lower layer's pre-merge pixels restored"
        );
    }

    /// The multi-selection is index-keyed: a structural op still clears it
    /// even though the history now survives.
    #[test]
    fn structural_ops_clear_the_multi_selection_not_the_history() {
        let mut doc = Document::default();
        doc.add_layer("2");
        doc.toggle_multi(0);
        assert_eq!(doc.multi_targets().len(), 2);
        doc.add_layer("3");
        assert_eq!(
            doc.multi_targets().len(),
            1,
            "selection cleared back to the active row alone"
        );
        assert!(doc.can_undo(), "history kept");
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
        assert!(
            doc.can_undo(),
            "adding the layer records one structural step"
        );
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

    /// Row 32: rasterizing a frame folder keeps what is losable-for-free
    /// and gives up only the frame OBJECT — the header's border ink
    /// becomes a plain raster layer, the panel clip becomes the
    /// children's layer masks, the children stay separate, and ONE undo
    /// restores the folder.
    #[test]
    fn rasterize_frame_folder_keeps_children_and_clips_by_mask() {
        let mut doc = Document::new(256, 256);
        let fs = FrameSet {
            frames: vec![crate::Frame::rect(32.0, 32.0, 224.0, 224.0)],
            border_px: 4.0,
            slot: None,
            reading_pin: None,
            border_ruler: false,
            color: [0, 0, 0],
        };
        let hi = doc.add_frame_folder("Frame 1", fs);
        // add_frame_folder lands [White, Layer 1, header] with the draw
        // layer active; children sit BELOW the header index.
        let kids: Vec<usize> = doc.children_range(hi).collect();
        assert_eq!(kids.len(), 2, "white + draw layer");
        assert!(doc.layers[hi].folder && doc.layers[hi].is_frame());
        assert!(doc.layers[hi].mask_tiles().is_some_and(|m| !m.is_empty()));

        assert!(doc.rasterize_frame_folder(hi));
        let h = &doc.layers[hi];
        assert!(!h.folder, "the folder is gone");
        assert!(matches!(h.kind, LayerKind::Raster), "the header is plain ink now");
        assert!(h.tile_count() > 0, "the border ink raster survives");
        for k in &kids {
            let c = &doc.layers[*k];
            assert_eq!(c.depth, h.depth, "child hoisted loose beside the ink");
            assert!(
                c.mask.as_ref().is_some_and(|m| !m.tiles.is_empty()),
                "the panel clip rides as a layer mask"
            );
        }
        // add_frame_folder pushed its own structural step — the rasterize
        // is exactly ONE more.
        assert_eq!(doc.undo_labels().len(), 2, "setup + ONE rasterize step");
        assert!(doc.undo());
        let h = &doc.layers[hi];
        assert!(h.folder && h.is_frame(), "undo restores the frame folder");
        for k in &kids {
            assert!(
                doc.layers[*k].mask.is_none(),
                "undo takes the clip back into the folder"
            );
        }

        // Only frame folders: a plain layer is refused untouched.
        let plain = doc.add_layer("plain");
        assert!(!doc.rasterize_frame_folder(plain));
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
        let li = doc.add_balloon_layer("Balloon 1", one);
        assert!(doc.layers[li].is_balloon() && doc.layers[li].is_vector());
        assert!(doc.layers[li].tile_count() > 0);
        // Post-mint state — clones of it keep identity (text test's twin).
        let one = doc.layers[li].balloons().unwrap().clone();
        assert_ne!(one.balloons[0].id, 0, "commit mints");

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
        assert!(!doc.merge_down(3), "a FRAME folder never merges (its vectors)");
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

    /// Recordable actions: `push_structure` makes a replayed run — however
    /// many structural and pixel steps it contained — ONE undo press, and
    /// redo re-applies the run's whole result.
    #[test]
    fn structure_snapshot_is_one_undo_press_for_a_whole_run() {
        let mut doc = Document::new(64, 64);
        doc.begin_op();
        doc.layers[0].tile_mut(TileIdx::new(0, 0)).data_mut()[0] = 123;
        doc.end_op();
        let before = doc.layers.clone();
        let active_before = doc.active;
        // The "run": a structural step (clears history) then a pixel step
        // (pushes its own group) — both superseded by the snapshot pair.
        doc.add_layer("SFX");
        let li = doc.active;
        doc.begin_op();
        doc.layers[li].tile_mut(TileIdx::new(0, 0)).data_mut()[0] = 77;
        doc.end_op();
        doc.push_structure("Create SFX Layer", before, active_before);
        assert_eq!(doc.undo_labels().len(), 1, "the run's step stands alone");
        assert_eq!(doc.undo_labels()[0], "Create SFX Layer");
        assert!(doc.undo());
        assert_eq!(doc.layers.len(), 1, "stack restored wholesale");
        assert_eq!(doc.active, active_before);
        assert_eq!(
            doc.layers[0].tile_arc(TileIdx::new(0, 0)).unwrap().data()[0],
            123,
            "pre-run pixels intact"
        );
        assert!(doc.redo());
        assert_eq!(doc.layers.len(), 2, "redo re-applies the run");
        assert_eq!(doc.layers[1].name, "SFX");
        assert_eq!(
            doc.layers[1].tile_arc(TileIdx::new(0, 0)).unwrap().data()[0],
            77,
            "run-created pixels return"
        );
    }

    /// docs/CLIPPING-SCENARIOS.md 2a: a layer above a folder clips to the
    /// folder's combined ink — the header is a valid base now, and a clip
    /// run above it shares that base. A THROUGH folder has no isolated
    /// composite to take an alpha from, so it still breaks the chain.
    #[test]
    fn clip_above_a_folder_resolves_to_the_folder() {
        let mut doc = Document::new(64, 64);
        let hi = doc.add_folder_above(0, "F");
        doc.add_layer_in_folder(hi, "in").unwrap();
        let hi = hi + 1; // the child inserted below shifted the header up
        let top = doc.add_layer_above(hi, "Shade");
        assert!(doc.set_layer_clip(top, true));
        let above = doc.add_layer("Shade 2");
        assert!(doc.set_layer_clip(above, true));
        let bases = doc.clip_bases();
        assert_eq!(bases[top], Some(hi), "folder header is the base");
        assert_eq!(
            bases[above],
            Some(hi),
            "the run resolves through the member"
        );
        assert!(doc.set_folder_through(hi, true));
        let bases = doc.clip_bases();
        assert_eq!(bases[top], None, "a through folder breaks the chain");
        assert_eq!(bases[above], None, "for the whole run");
    }

    /// docs/CLIPPING-SCENARIOS.md #1/#2: a new layer added from the base —
    /// or from any member — of a clip run lands ABOVE the run, never inside
    /// it. Inside, the run would silently re-base onto the new empty layer
    /// and every clipped member would go invisible.
    #[test]
    fn new_layer_hops_above_the_clip_run() {
        let mut doc = Document::new(64, 64);
        let base = doc.add_layer("Base");
        let c1 = doc.add_layer("Clip 1");
        doc.set_layer_clip(c1, true);
        let c2 = doc.add_layer("Clip 2");
        doc.set_layer_clip(c2, true);
        doc.set_active(base);
        let n = doc.add_layer("Pasted");
        assert_eq!(n, c2 + 1, "insert hops above the whole run");
        let bases = doc.clip_bases();
        assert_eq!(bases[c1], Some(base), "run keeps its base");
        assert_eq!(bases[c2], Some(base));
        // From a mid-run member: same landing spot.
        doc.set_active(c1);
        let m = doc.add_layer("Mid add");
        assert_eq!(m, c2 + 1, "mid-run insert lands just above the run");
        assert_eq!(doc.clip_bases()[c1], Some(base));
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
        assert_eq!(
            doc.merge_down_refusal(1),
            None,
            "unlocked again, the merge is allowed (a clipped layer bakes what it shows — see the app suite)"
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
        assert_eq!(
            doc.palette_colour(outer),
            Some(BLUE),
            "no leak from outside"
        );

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

    /// The combine DESTROYS B's header, so everything the compositor reads
    /// from a folder (visibility, opacity, blend, through, draft) and
    /// everything that lives on the `FrameSet` rather than on a `Frame`
    /// (border width, the ruler flag, the reading pin) went with it — B's
    /// panels silently re-rendered wearing A's look. None of it pushes down:
    /// a GROUP blend/opacity is not a per-child blend/opacity, a hidden
    /// group is not a hidden child, and a `Frame` has no border of its own.
    /// So a pair that disagrees refuses rather than restyling art.
    #[test]
    fn combine_frame_folders_refuses_to_silently_restyle_the_partner() {
        let build = || {
            let mut doc = Document::new(400, 400);
            let a = doc.add_frame_folder(
                "Frame 1",
                FrameSet::single_rect([16.0, 16.0, 200.0, 300.0], 4.0),
            );
            let b = doc.add_frame_folder(
                "Frame 2",
                FrameSet::single_rect([200.0, 16.0, 384.0, 300.0], 4.0),
            );
            (doc, a, b)
        };
        // Two folders that agree still combine — the divide-siblings case.
        let (mut doc, a, b) = build();
        assert!(doc.combine_frame_folders(a, b, false).is_some());

        let cases: [(&str, fn(&mut Layer)); 9] = [
            ("blend", |l| l.blend = Blend::Multiply),
            ("opacity", |l| l.opacity = 0.5),
            ("visibility", |l| l.visible = false),
            ("through", |l| l.through = true),
            ("draft", |l| l.draft = true),
            ("mask", |l| l.mask = Some(LayerMask::default())),
            ("border width", |l| {
                l.frames_mut().unwrap().border_px = 9.0;
            }),
            ("border ruler", |l| {
                l.frames_mut().unwrap().border_ruler = true;
            }),
            ("reading pin", |l| {
                l.frames_mut().unwrap().reading_pin = Some(3);
            }),
        ];
        for (what, edit) in cases {
            let (mut doc, a, b) = build();
            edit(&mut doc.layers[b]);
            assert!(
                doc.combine_frame_folders(a, b, false).is_none(),
                "B's {what} would have been dropped"
            );
            // The same disagreement refuses from either side.
            let (mut doc, a, b) = build();
            edit(&mut doc.layers[a]);
            assert!(
                doc.combine_frame_folders(a, b, false).is_none(),
                "A's {what} would have been forced onto B"
            );
        }
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
                ..EdgeParams::default()
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
            ..Default::default()
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
        // FB-knockout: a PLAIN folder accepts (the group mat); a FRAME
        // folder refuses — its close already owns a mask + border ink.
        let fi = doc.add_folder_above(0, "Folder");
        assert!(doc.set_edge(fi, Some(p)), "plain folder takes the mat");
        assert!(doc.set_edge(fi, None));
        let hi = doc.add_frame_folder(
            "F",
            crate::frame::FrameSet::single_rect([1.0, 1.0, 9.0, 9.0], 2.0),
        );
        assert!(!doc.set_edge(hi, Some(p)), "frame folder refuses");
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
                ..Default::default()
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

    /// Structural layer ops record a Structure group now, so they count
    /// through the ordinary push path; the change undo genuinely cannot
    /// express (`clear_history`, e.g. a resize) still counts via `note_op`
    /// and never rewinds the tally.
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
        assert_eq!(doc.undo_len(), 2, "…and it is on the stack, not a wipe");
        let ops = doc.op_count();
        doc.clear_history();
        assert_eq!(doc.undo_len(), 0);
        assert!(
            doc.op_count() > ops,
            "the tally is monotonic: clear_history counts and never rewinds"
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
