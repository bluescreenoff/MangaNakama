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

mod derived;
mod frames;
mod history;
mod layers;
mod masks;
mod merge;
mod paint;
mod props;
mod resize;
mod spill;
mod vector_layers;

#[cfg(test)]
mod tests;

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

/// One layer's speech: the balloons AND the words inside them, the way CSP's
/// text layer holds both. Ink order is balloons first, texts over them —
/// see [`SpeechSet::rasterize`].
///
/// Before 2026-09-06 these were two layer kinds (`Text` and `Balloon`).
/// They are one kind now so that drawing a bubble around a line of text can
/// put the bubble on the text's own layer, and the layer move tool then
/// moves both (item P). A set with no balloons is exactly the old text
/// layer; a set with no texts is exactly the old balloon layer; `.ora`
/// files written by either era load into this without merging anything
/// (see `ora.rs`).
#[derive(Clone, Debug, Default)]
pub struct SpeechSet {
    pub texts: TextSet,
    pub balloons: BalloonSet,
    /// Which half the layer was MADE for. Content answers
    /// [`crate::Layer::is_text`] / [`crate::Layer::is_balloon`] whenever
    /// there IS content; this is the tiebreak for the empty layer, which is
    /// a real thing: `layers.add_balloon` over MCP makes an empty bubble
    /// layer for a script to fill on the next call, and the palette's "new
    /// text layer" makes an empty one for the caret. Not stored as a field
    /// in `.ora` — which of the two attributes the layer writes says it.
    pub born: SpeechBorn,
}

/// See [`SpeechSet::born`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpeechBorn {
    #[default]
    Text,
    Balloon,
}

impl SpeechSet {
    /// Only words (what the old `LayerKind::Text` held).
    pub fn of_texts(texts: TextSet) -> Self {
        Self {
            texts,
            balloons: BalloonSet::default(),
            born: SpeechBorn::Text,
        }
    }

    /// Only bubbles (what the old `LayerKind::Balloon` held).
    pub fn of_balloons(balloons: BalloonSet) -> Self {
        Self {
            texts: TextSet::default(),
            balloons,
            born: SpeechBorn::Balloon,
        }
    }

    /// Neither half holds anything yet.
    pub fn is_empty(&self) -> bool {
        self.texts.texts.is_empty() && self.balloons.balloons.is_empty()
    }

    /// The layer's pixels: the bubbles, then the words source-over them, in
    /// ONE tile map. Pure, so undo re-derives the raster from cloned state.
    pub fn rasterize(&self, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
        self.texts
            .rasterize_over(size, self.balloons.rasterize(size))
    }

    /// Mint both halves' stable ids (the commit door's contract).
    pub fn mint_ids(&mut self) {
        self.texts.mint_ids();
        self.balloons.mint_ids();
    }
}

/// What a layer *is*. Raster layers own their pixels; frame and speech
/// layers derive their pixels from vector state ([`FrameSet`] /
/// [`SpeechSet`]) and are regenerated by `Document::set_frames`
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
    /// Balloons and the words in them on ONE layer (CSP's text layer).
    /// Was two kinds, `Balloon` and `Text`, until item P.
    Speech(SpeechSet),
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

    /// The balloons on this layer, if it is a speech layer. A speech layer
    /// that holds only words answers `Some` with an EMPTY set — the field
    /// exists, it is just empty — so callers that ask "can this layer take a
    /// balloon?" get a yes.
    pub fn balloons(&self) -> Option<&BalloonSet> {
        match &self.kind {
            LayerKind::Speech(s) => Some(&s.balloons),
            _ => None,
        }
    }

    /// The words on this layer, if it is a speech layer. Same rule as
    /// [`Self::balloons`]: a bubbles-only layer answers with an empty set.
    pub fn texts(&self) -> Option<&TextSet> {
        match &self.kind {
            LayerKind::Speech(s) => Some(&s.texts),
            _ => None,
        }
    }

    /// Both halves at once, if this is a speech layer.
    pub fn speech(&self) -> Option<&SpeechSet> {
        match &self.kind {
            LayerKind::Speech(s) => Some(s),
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

    /// Any speech layer, words or bubbles or both. The kind test; the two
    /// below are the CONTENT tests.
    pub fn is_speech(&self) -> bool {
        matches!(self.kind, LayerKind::Speech(_))
    }

    /// Carries at least one bubble — or was made to and is still empty.
    /// Since item P a layer can be both this and [`Self::is_text`]: that is
    /// the whole point of the change.
    pub fn is_balloon(&self) -> bool {
        match &self.kind {
            LayerKind::Speech(s) => {
                !s.balloons.balloons.is_empty() || (s.is_empty() && s.born == SpeechBorn::Balloon)
            }
            _ => false,
        }
    }

    /// Carries words — or was made to and is still empty.
    pub fn is_text(&self) -> bool {
        match &self.kind {
            LayerKind::Speech(s) => {
                !s.texts.texts.is_empty() || (s.is_empty() && s.born == SpeechBorn::Text)
            }
            _ => false,
        }
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
            // Item P: ONE layer, BOTH sets move — the layer-move drag, K,
            // Ctrl+T and every content shift translate a bubble and the words
            // in it together, which is the behaviour the owner asked for.
            LayerKind::Speech(sp) => {
                use crate::balloon::BalloonShape;
                let bs = &mut sp.balloons;
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
                for item in &mut sp.texts.texts {
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
            LayerKind::Speech(sp) => {
                use crate::balloon::BalloonShape;
                let bs = &mut sp.balloons;
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
                for item in &mut sp.texts.texts {
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
    /// S03: the print resolution this canvas was drawn at, when it has one.
    /// A page inside a work reads its dpi from the work's `PageSetup`, but a
    /// BARE `.ora` has no page setup at all — and without a dpi every live
    /// tone re-screens at the 600 dpi default on reopen, which visibly
    /// changes a saved page. Persisted as `mnc-dpi` on the ORA image
    /// element; `None` = a pixel canvas that never had one, which is what
    /// every file written before this attribute says.
    pub dpi: Option<u32>,
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
            dpi: None,
        }
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
/// Paint `img` into `layer`, CENTRED on a `size` canvas — the shape every
/// caller but I03's placement replay wants (a whole-canvas composite
/// centres at 0,0 anyway).
fn fill_layer_from_image(layer: &mut Layer, size: (u32, u32), img: &image::RgbaImage) {
    let ox = (size.0 as i64 - img.width() as i64) / 2;
    let oy = (size.1 as i64 - img.height() as i64) / 2;
    fill_layer_from_image_at(layer, size, img, ox, oy);
}

fn fill_layer_from_image_at(
    layer: &mut Layer,
    size: (u32, u32),
    img: &image::RgbaImage,
    ox: i64,
    oy: i64,
) {
    let (w, h) = (size.0 as i64, size.1 as i64);
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

