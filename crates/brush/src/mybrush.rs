//! `MyBrush` — the real brush engine: libmypaint doing the dabs, `.myb` presets
//! doing the feel, `core::Tile` doing the storage.
//!
//! `.myb` is JSON:
//!
//! ```text
//! { "version": 3,
//!   "settings": { "<setting>": { "base_value": f,
//!                                "inputs": { "<input>": [[x, y], ...] } } } }
//! ```
//!
//! We parse it in Rust and replay it through libmypaint's public setters, which
//! is exactly what the (now stubbed, see vendor/PATCHES.md)
//! `mypaint_brush_from_string` did with json-c — including the load order:
//! `mypaint_brush_from_defaults` first, so the ~19 settings the classic presets
//! omit get their stock values instead of zero.

use std::ffi::c_int;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mn_core::blend::{blend_premul, f32_to_fix15, fix15_to_f32, px_to_f32, scale_opacity};
use mn_core::dab::{DabParams, DabRecord};
use mn_core::{Blend, Document, PenSample, StrokeSink, Tile, TileIdx};
use serde_json::Value;

use crate::ffi;
use crate::settings::{self, input, setting};
use crate::surface::TileSurface;

/// Everything that can go wrong loading a preset.
#[derive(Debug)]
pub enum BrushError {
    Io(std::io::Error),
    Json(serde_json::Error),
    /// Well-formed JSON that is not a v3 `.myb`.
    Format(String),
}

impl fmt::Display for BrushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BrushError::Io(e) => write!(f, "reading brush preset: {e}"),
            BrushError::Json(e) => write!(f, "parsing brush preset: {e}"),
            BrushError::Format(m) => write!(f, "bad brush preset: {m}"),
        }
    }
}

impl std::error::Error for BrushError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BrushError::Io(e) => Some(e),
            BrushError::Json(e) => Some(e),
            BrushError::Format(_) => None,
        }
    }
}

impl From<std::io::Error> for BrushError {
    fn from(e: std::io::Error) -> Self {
        BrushError::Io(e)
    }
}

impl From<serde_json::Error> for BrushError {
    fn from(e: serde_json::Error) -> Self {
        BrushError::Json(e)
    }
}

/// A grayscale dab mask (Krita texture tip): `size × size` bytes, 0..255.
///
/// The mask covers each dab's bounding square and multiplies the dab profile
/// (gaussian or hard); the pattern can crawl dab-by-dab via
/// [`MyBrush::set_texture_scroll`] — Krita's "texture offset per dab" feel.
/// Shared through `Arc` because the app's mirror-twin engines load the same
/// preset.
#[derive(Debug)]
pub struct TextureMask {
    pub name: String,
    pub size: u32,
    pub data: Arc<Vec<u8>>,
}

/// Load `textures/<name>.png` under the brushes root as a dab mask. Square,
/// up to 1024 px; anything else is warned about and ignored (the brush stays
/// usable, just untextured). Public so the app can resolve its Tool Property
/// texture picker without re-implementing the rules.
pub fn load_texture(brushes_root: &Path, name: &str) -> Option<Arc<TextureMask>> {
    let path = brushes_root.join("textures").join(format!("{name}.png"));
    let img = match image::open(&path) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("texture {name:?}: {e}, texture off");
            return None;
        }
    };
    let gray = img.to_luma8();
    let (w, h) = gray.dimensions();
    if w == 0 || w != h || w > 1024 {
        eprintln!("texture {name:?}: {w}x{h}, want square <=1024, texture off");
        return None;
    }
    Some(Arc::new(TextureMask {
        name: name.to_owned(),
        size: w,
        data: Arc::new(gray.into_raw()),
    }))
}

/// A libmypaint brush bound to a MangaNakama document.
///
/// Not `Send`/`Sync` (raw pointers): libmypaint keeps mutable stroke state and
/// the surface hands out pointers into document tiles. Single-threaded by
/// contract with the app crate — see `surface.rs` for the full aliasing story.
pub struct MyBrush {
    brush: *mut ffi::MyPaintBrush,
    surface: TileSurface,
    name: String,
    /// `radius_logarithmic` as the preset shipped it. `set_size_multiplier`
    /// re-derives the live value from this, so repeated calls do not compound.
    base_radius_log: f32,
    size_mul: f32,
    /// Timestamp of the previous sample; `None` at the head of a stroke.
    last_t_ms: Option<f64>,
    /// CSP entry taper the import could not bake: (length, min factor 0..1).
    /// Parsed from the `mn.unmapped` note; the app's `core::Taper` applies it.
    taper_hint: Option<(f32, f32)>,
    /// When set, `radius_by_random` is a size-independent pixel deviation
    /// (the vendored absolute hook) instead of stock log-radius noise.
    radius_random_abs: bool,
    /// Krita-style hard stamp dabs (the vendored tiled-surface hook):
    /// exact AA discs instead of the gaussian hardness falloff.
    hard_dab: bool,
    /// Krita Scatter: each dab's centre jitters within `radius * scatter`.
    scatter: f32,
    /// Krita Wash mode (flow vs opacity): dabs accumulate in an off-canvas
    /// stroke buffer; `end` composites it ONCE at the stroke-level opacity
    /// (and blend mode), so a single stroke never exceeds that opacity no
    /// matter how much its dabs overlap. The preset's `opaque` (the Flow
    /// slider) is the per-dab alpha inside the buffer. Off (the default) is
    /// build-up: every dab composites straight into the layer, stock
    /// behaviour.
    wash: bool,
    /// Stroke-level opacity applied at the wash commit (Krita: Opacity).
    wash_opacity: f32,
    /// Blend mode of the wash commit (Krita: per-brush blending mode).
    wash_blend: Blend,
    /// The stroke buffer, live between `begin` and `end` of a wash stroke.
    wash_buf: Option<Box<Document>>,
    /// Eraser mode captured at `begin` of a wash stroke: dabs must LAY PAINT
    /// into the buffer (so it records where to erase), and the commit
    /// subtracts instead of compositing. The `eraser` setting itself is
    /// forced off for the stroke and restored at `end`.
    wash_erase: bool,
    /// Krita texture tip: the grayscale mask multiplying every dab's profile.
    texture: Option<Arc<TextureMask>>,
    /// Generated-variation tips (M4): the full tip LIST — when non-empty,
    /// the per-dab hook swaps the ACTIVE mask among these (seeded random),
    /// so one brush stamps a whole family of marks. `mn-texture-list`.
    texture_tips: Vec<Arc<TextureMask>>,
    /// 0..1 — how much per-dab MIRRORING and angle JITTER the tip list
    /// gets (0 = tips swap but never mirror/rotate). `mn-variation`.
    variation: f32,
    /// The variant table for [`variation`]/`texture_tips`: each tip plus
    /// its mirrored copies (when variation > 0), as (data ptr, size)
    /// pairs. BOXED: the hook publishes this buffer's address, so it must
    /// never move — and its Arcs' buffers never move either.
    tip_variants: Box<[(*const u8, i32)]>,
    /// Keeps the mirrored variant buffers alive (the table points in).
    _variant_masks: Vec<Arc<TextureMask>>,
    /// Texture crawl per dab, in mask px (0 = the pattern is static, Krita's
    /// default). The step direction is fixed diagonal (1, 0.5) so the crawl
    /// reads as drift, not wobble.
    texture_scroll_px: f32,
    /// PATCHES.md #10 amendment 2: the mask is DAB-anchored — a stamped tip
    /// covering the dab's bounding square. Off = canvas-anchored grain, the
    /// mn default.
    texture_anchor_dab: bool,
    /// Stamp rotation follows the stroke direction (on top of the base).
    /// UNFOLDED per dab — the elliptical angle cannot do this (mod 180).
    texture_rotate_direction: bool,
    /// Stamp base angle, degrees.
    texture_angle_deg: f32,
    /// Live crawl offset (mask px). Owned here, advanced by the C-side
    /// per-dab hook; reset at every `begin`.
    tex_accum: (f32, f32),
    /// Krita SKETCH mode: after each real sample, connect the stroke to a
    /// recent history point within `sketch_distance` — scribbles knot into
    /// hatching webs. `None` = off, stock behaviour.
    sketch: Option<SketchParams>,
    /// Recent stroke positions the filaments link back to (ring buffer).
    history: Vec<(f32, f32)>,
    /// Tiny LCG for picking link targets — deterministic per stroke seed.
    rng: u64,
    /// GPU-dabs P0 record mode (vendor/PATCHES.md #11). Off by default —
    /// the app wires it when P1 lands.
    record_mode: RecordMode,
    /// The live record (owned here; the C hook reaches it through a
    /// thread-local pointer valid for one `stroke_to`).
    record: DabRecord,
    /// Preset uses a mode the P1 GPU path does not port (spectral paint,
    /// colorize, posterize) — stroke routing consults this (`gpu_ready`);
    /// the CPU path is the reference, not a fallback.
    exotic: bool,
    /// Preset samples the canvas per dab (the `smudge` setting). GPU-routed
    /// since #0.1 part 3: the dabs themselves are ordinary (color computed
    /// engine-side before the record), but the app must serve the smudge
    /// sampler from the GPU tile cache — see the tile oracle in surface.rs.
    smudge: bool,
    /// View zoom for the speed-input compensation (PATCHES.md #12): the C
    /// computes SPEED1/SPEED2 & co. as document velocity × this value, so
    /// zoomed-out drawing must pass the zoom or every speed-mapped dynamic
    /// fires 1/zoom times too hard. 1.0 = the stock behaviour exactly.
    view_zoom: f32,
    /// View rotation in RADIANS (direction-input compensation, same patch).
    /// The C applies `DEGREES()` to the argument itself (mypaint-brush.c
    /// `update_states_and_setting_values`), so despite the vendored
    /// docstring saying "degrees" the value it wants is radians — MyPaint
    /// itself passes `tdw.rotation`, radians (auditor 2026-08-17: our
    /// `.to_degrees()` call was a unit mismatch skewing every
    /// direction-mapped input on rotated canvases).
    view_rotation_rad: f32,
    /// Horizontal view mirror (same patch, flip extension): a mirror maps
    /// doc angle θ to screen angle 180−θ+r — the C negates the direction
    /// vectors' DX under this flag, then the raw `+viewrotation` arithmetic
    /// carries the rest with the flipped view's own (already-negated)
    /// rotation.
    view_flip: bool,
    /// LM-004: strokes edit the active layer's MASK (coverage), not pixels.
    mask_mode: bool,
    /// Strokes edit the DOCUMENT's selection scratch (selection pen /
    /// eraser / Quick Mask) — alpha is the coverage payload.
    sel_mode: bool,
    /// CSP Advanced ▸ Stroke ▸ Interval (S-028) as the user last set it.
    /// `AsPreset` — the default — never writes the engine's dab spacing.
    interval: Interval,
    /// The preset's own `(dabs_per_actual_radius, dabs_per_basic_radius)`,
    /// captured at load so [`Interval::AsPreset`] is an exact revert rather
    /// than an approximation of one.
    base_dabs: (f32, f32),
    /// The preset's own `opaque_linearize` (CSP B-029's amount), captured at
    /// load: switching the toggle back on restores THIS, not a house value.
    base_linearize: f32,
    /// The preset's own `anti_aliasing` (px feather), captured at load.
    base_aa: f32,
    /// CSP Tool Settings ▸ Anti-aliasing (A-010) as the user last set it.
    anti_alias: AntiAlias,
}

/// CSP Advanced ▸ Stroke ▸ Interval (S-028) — the gap between the individual
/// dabs a stroke is stamped from.
///
/// CSP stores its own `BrushInterval` as a **percentage of the tip diameter**
/// (proven by `tools/cspmap.mjs`, which converts it with
/// `dabs_per_actual_radius = 100 / (2 × interval)`), so [`Percent`] is CSP's
/// native unit and not an invention. [`FixedPx`] is the fourth CSP mode: a
/// literal canvas-pixel gap that does not scale when the Size slider moves.
///
/// [`AsPreset`](Interval::AsPreset) is ours and is the default: it leaves the
/// preset's `dabs_per_*` base values exactly as the `.myb` shipped them, which
/// is what makes a brush the owner has used for months keep drawing the same
/// pixels after this control existed.
///
/// [`Percent`]: Interval::Percent
/// [`FixedPx`]: Interval::FixedPx
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Interval {
    /// The preset's own spacing, untouched.
    #[default]
    AsPreset,
    /// Gap as a percent of the tip DIAMETER — spacing scales with the brush.
    Percent(f32),
    /// Gap in canvas pixels — spacing does NOT scale with the brush.
    FixedPx(f32),
}

impl Interval {
    /// CSP's three relative presets, in percent of tip diameter.
    ///
    /// **These three numbers are a judgement call and want the owner's hand**
    /// (they are the only part of S-028 not derivable from the code): the two
    /// outer ones are the values his own CSP tools already ship through
    /// `cspmap` — 10 % is the Real G-Pen / milli pen / pencil interval and
    /// 20 % is the crayon's — and Narrow is the geometric step below Normal.
    pub const NARROW_PCT: f32 = 5.0;
    pub const NORMAL_PCT: f32 = 10.0;
    pub const WIDE_PCT: f32 = 20.0;

    /// Percent range the control accepts. 100 % is a gap of one whole tip
    /// diameter (a visible row of beads); 1 % is 50 dabs per radius, which is
    /// where the stamping cost stops buying anything.
    pub const MIN_PCT: f32 = 1.0;
    pub const MAX_PCT: f32 = 100.0;
    /// Pixel range of the Fixed mode.
    pub const MIN_PX: f32 = 0.25;
    pub const MAX_PX: f32 = 64.0;
}

/// CSP Tool Settings ▸ Anti-aliasing (A-010): four levels, not a toggle.
///
/// libmypaint's knob is `anti_aliasing` ("Pixel feather"), a MINIMUM edge
/// fadeout in canvas pixels: when a dab's own `radius × (1 − hardness)` falls
/// below it, `prepare_and_draw_dab` softens hardness and grows the radius
/// together so the OPTICAL radius is preserved. So the levels below are
/// feather widths in px, and [`AsPreset`](AntiAlias::AsPreset) — the default —
/// leaves the preset's own value alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AntiAlias {
    /// The preset's own `anti_aliasing`, untouched.
    #[default]
    AsPreset,
    /// CSP "None" — no minimum feather; a hard tip stays aliased.
    Off,
    Weak,
    Middle,
    Strong,
}

impl AntiAlias {
    /// The four ladder rungs, CSP's order.
    pub const LEVELS: [AntiAlias; 4] = [
        AntiAlias::Off,
        AntiAlias::Weak,
        AntiAlias::Middle,
        AntiAlias::Strong,
    ];

    /// Minimum edge feather in canvas px, or `None` for `AsPreset`.
    ///
    /// **The three non-zero rungs are guesses and want the owner's eye.**
    /// The anchors: libmypaint's own default is 1.0 and its tooltip calls
    /// that "blur one pixel (good value)", so Middle is the stock value;
    /// `cspmap.mjs` maps CSP's AntiAlias enum onto 0.0 / 0.5 / 0.5 / 1.0,
    /// which is where Weak's 0.5 comes from (and every CSP-derived preset in
    /// the tree sits at 0.5). Strong's 2.0 is an extrapolation — the setting
    /// runs to 5.0, where the vendored tooltip says thin strokes vanish.
    pub fn feather_px(self) -> Option<f32> {
        match self {
            AntiAlias::AsPreset => None,
            AntiAlias::Off => Some(0.0),
            AntiAlias::Weak => Some(0.5),
            AntiAlias::Middle => Some(1.0),
            AntiAlias::Strong => Some(2.0),
        }
    }
}

/// The knobs of the sketch engine.
#[derive(Clone, Copy, Debug)]
pub struct SketchParams {
    /// Maximum distance (canvas px) to a linkable history point.
    pub distance: f32,
    /// Probability of attempting a link per sample, 0..1.
    pub density: f32,
}

/// Recording mode of the vendored tap (PATCHES.md #11).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RecordMode {
    #[default]
    Off,
    /// Record AND rasterize — P0: pixels unchanged, the buffer fills.
    Tap,
    /// Record ONLY, skip rasterization — the P1 GPU path's seam.
    Bypass,
}

impl MyBrush {
    /// Load a `.myb` preset. Unknown setting/input names are warned about and
    /// skipped (a newer MyPaint can write settings this libmypaint lacks).
    pub fn load(path: &Path) -> Result<MyBrush, BrushError> {
        let text = std::fs::read_to_string(path)?;
        let json: Value = serde_json::from_str(&text)?;

        let version = json.get("version").and_then(Value::as_i64);
        match version {
            Some(3) => {}
            Some(v) => {
                return Err(BrushError::Format(format!(
                    "unsupported .myb version {v} (expected 3)"
                )));
            }
            None => return Err(BrushError::Format("no 'version' field".into())),
        }
        let mut exotic = false;
        let mut smudge = false;
        let settings_obj = json
            .get("settings")
            .and_then(Value::as_object)
            .ok_or_else(|| BrushError::Format("no 'settings' object".into()))?;

        // MyPaint itself names a brush by its file name; ours may also carry a
        // `"name"` string, which is how the CSP-derived presets keep their
        // Japanese display names while living in ASCII-safe files.
        let name = json
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "brush".to_string());

        let brush = unsafe { ffi::mypaint_brush_new() };
        if brush.is_null() {
            return Err(BrushError::Format("mypaint_brush_new returned null".into()));
        }
        // Stock defaults first, then the preset on top — upstream's order.
        unsafe { ffi::mypaint_brush_from_defaults(brush) };

        for (setting_name, body) in settings_obj {
            let Some(id) = settings::setting_id(setting_name) else {
                eprintln!("{}: unknown brush setting {setting_name:?}, skipped", name);
                continue;
            };
            let Some(body) = body.as_object() else {
                eprintln!(
                    "{}: setting {setting_name:?} is not an object, skipped",
                    name
                );
                continue;
            };

            // GPU-dabs routing (docs/design/GPU-DABS.md §8): the modes the
            // compute shader does not port stay on the CPU path. Detected at
            // load so `gpu_ready` is a cheap field read at stroke start.
            // `smudge` is NOT here since #0.1 part 3: its dabs are ordinary
            // (the canvas sample happens engine-side, before the record), and
            // the app serves the sampler through the GPU tile oracle.
            // `colorize`/`posterize` left this list in the P4 round — their
            // stamps are ported (dab.wgsl + cpu_raster mirror); only the
            // spectral `paint` mode remains CPU-bound.
            if setting_name.as_str() == "paint" {
                let base = body
                    .get("base_value")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let mapped = body
                    .get("inputs")
                    .and_then(Value::as_object)
                    .is_some_and(|m| !m.is_empty());
                if base > 0.0 || mapped {
                    exotic = true;
                }
            }
            if setting_name == "smudge" {
                let base = body
                    .get("base_value")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let mapped = body
                    .get("inputs")
                    .and_then(Value::as_object)
                    .is_some_and(|m| !m.is_empty());
                if base > 0.0 || mapped {
                    smudge = true;
                }
            }

            if let Some(base) = body.get("base_value").and_then(Value::as_f64) {
                unsafe { ffi::mypaint_brush_set_base_value(brush, id, base as f32) };
            }

            let Some(inputs) = body.get("inputs").and_then(Value::as_object) else {
                continue;
            };
            for (input_name, points) in inputs {
                let Some(input_id) = settings::input_id(input_name) else {
                    eprintln!("{}: unknown brush input {input_name:?}, skipped", name);
                    continue;
                };
                let Some(points) = points.as_array() else {
                    eprintln!(
                        "{}: mapping {setting_name}/{input_name} is not an array, skipped",
                        name
                    );
                    continue;
                };
                unsafe {
                    ffi::mypaint_brush_set_mapping_n(brush, id, input_id, points.len() as c_int)
                };
                for (i, pt) in points.iter().enumerate() {
                    let (x, y) = match pt.as_array() {
                        Some(xy) if xy.len() >= 2 => (
                            xy[0].as_f64().unwrap_or(0.0) as f32,
                            xy[1].as_f64().unwrap_or(0.0) as f32,
                        ),
                        _ => {
                            eprintln!(
                                "{}: mapping point {i} of {setting_name}/{input_name} is not [x, y]",
                                name
                            );
                            (0.0, 0.0)
                        }
                    };
                    unsafe {
                        ffi::mypaint_brush_set_mapping_point(brush, id, input_id, i as c_int, x, y)
                    };
                }
            }
        }

        let base_radius_log =
            unsafe { ffi::mypaint_brush_get_base_value(brush, setting::RADIUS_LOGARITHMIC) };

        // The three "feel" base values, captured BEFORE anything can touch
        // them, so `AsPreset` / the density toggle's on-state are exact
        // reverts to what the file said rather than to a house default.
        let base_dabs = unsafe {
            (
                ffi::mypaint_brush_get_base_value(brush, setting::DABS_PER_ACTUAL_RADIUS),
                ffi::mypaint_brush_get_base_value(brush, setting::DABS_PER_BASIC_RADIUS),
            )
        };
        let base_linearize =
            unsafe { ffi::mypaint_brush_get_base_value(brush, setting::OPAQUE_LINEARIZE) };
        let base_aa = unsafe { ffi::mypaint_brush_get_base_value(brush, setting::ANTI_ALIASING) };

        // "entry taper (length 217, 18.3%)" from the CSP import's mn.unmapped
        // notes — the geometry the .myb format could not express.
        let taper_hint = json
            .get("mn")
            .and_then(|m| m.get("unmapped"))
            .and_then(Value::as_array)
            .and_then(|arr| {
                arr.iter().filter_map(Value::as_str).find_map(|s| {
                    let rest = s.strip_prefix("entry taper (length ")?;
                    let (len, rest) = rest.split_once(',')?;
                    let pct = rest.trim().strip_suffix("%)")?;
                    Some((
                        len.trim().parse::<f32>().ok()?,
                        pct.trim().parse::<f32>().ok()? / 100.0,
                    ))
                })
            });

        // Krita-inspired opt-in modes (vendor/PATCHES.md, round 25): top-level
        // JSON keys so stock .myb files stay untouched. Absent = off, which
        // keeps every existing preset pixel-identical.
        let hard_dab = json
            .get("mn-hard-dab")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let scatter = json
            .get("mn-scatter")
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .unwrap_or(0.0)
            .clamp(0.0, 4.0);

        // Krita Wash (flow vs opacity): per-stroke compositing. The preset's
        // own `opaque` base value is the per-dab FLOW; "mn-wash-opacity" is
        // the stroke-level opacity (default: fully opaque); the optional
        // blend mode applies at the commit, Krita's per-brush blending.
        let wash = json
            .get("mn-wash")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let wash_opacity = json
            .get("mn-wash-opacity")
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        let wash_blend = match json.get("mn-brush-blend").and_then(Value::as_str) {
            Some("multiply") => Blend::Multiply,
            Some("screen") => Blend::Screen,
            _ => Blend::Normal,
        };

        // Krita texture tip: a grayscale PNG under `textures/` beside the
        // preset groups, multiplied into every dab (vendor/PATCHES.md #10).
        // A name that does not resolve keeps the brush usable, untextured.
        let texture = json
            .get("mn-texture")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .and_then(|name| {
                let brushes_root = path.parent().and_then(Path::parent)?;
                load_texture(brushes_root, name)
            });
        let texture_scroll_px = json
            .get("mn-texture-scroll")
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .unwrap_or(0.0)
            .clamp(0.0, 64.0);
        // #10 amendment 2: "dab" = stamped tip; anything else (or absent) =
        // the canvas-anchored grain every existing preset means. The stamp's
        // rotation: a fixed base angle plus, optionally, the live stroke
        // direction ("mn-texture-rotate": "direction").
        let texture_anchor_dab = json
            .get("mn-texture-anchor")
            .and_then(Value::as_str)
            .is_some_and(|s| s == "dab");
        let texture_rotate_direction = json
            .get("mn-texture-rotate")
            .and_then(Value::as_str)
            .is_some_and(|s| s == "direction");
        let texture_angle_deg = json
            .get("mn-texture-angle")
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .filter(|v| v.is_finite())
            .unwrap_or(0.0);
        // M4 generated variation: a tip LIST (`mn-texture-list`, slugs under
        // textures/) + `mn-variation`. A list that resolves to fewer than
        // two masks keeps the single-mask behaviour (a name that does not
        // resolve is ignored, exactly like `mn-texture`).
        let texture_tips: Vec<Arc<TextureMask>> = json
            .get("mn-texture-list")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|name| {
                        let brushes_root = path.parent().and_then(Path::parent)?;
                        load_texture(brushes_root, name)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let variation = json
            .get("mn-variation")
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        // The variant table: plain pointers when there is no variation,
        // mirrored copies when there is. Built once here; the per-stroke
        // hook only publishes its address.
        let mut variant_masks: Vec<Arc<TextureMask>> = Vec::new();
        for t in &texture_tips {
            variant_masks.push(t.clone());
            if variation > 0.0 {
                variant_masks.push(Arc::new(mirror_mask(t, true, false)));
                variant_masks.push(Arc::new(mirror_mask(t, false, true)));
                variant_masks.push(Arc::new(mirror_mask(t, true, true)));
            }
        }
        let tip_variants: Box<[(*const u8, i32)]> = variant_masks
            .iter()
            .map(|m| (m.data.as_ptr(), m.size as i32))
            .collect();

        // Krita sketch mode (round 27): link the stroke back to its recent
        // history — scribble webs / hatching for roughing.
        let sketch = json
            .get("mn-sketch")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .then(|| SketchParams {
                distance: json
                    .get("mn-sketch-distance")
                    .and_then(Value::as_f64)
                    .map(|v| v as f32)
                    .unwrap_or(40.0)
                    .clamp(2.0, 500.0),
                density: json
                    .get("mn-sketch-density")
                    .and_then(Value::as_f64)
                    .map(|v| v as f32)
                    .unwrap_or(0.3)
                    .clamp(0.0, 1.0),
            });

        Ok(MyBrush {
            brush,
            surface: TileSurface::new(),
            name,
            base_radius_log,
            size_mul: 1.0,
            last_t_ms: None,
            taper_hint,
            radius_random_abs: false,
            hard_dab,
            scatter,
            wash,
            wash_opacity,
            wash_blend,
            wash_buf: None,
            wash_erase: false,
            texture,
            texture_tips,
            variation,
            tip_variants,
            _variant_masks: variant_masks,
            texture_scroll_px,
            texture_anchor_dab,
            texture_rotate_direction,
            texture_angle_deg,
            tex_accum: (0.0, 0.0),
            sketch,
            history: Vec::new(),
            rng: 0x9E3779B97F4A7C15,
            record_mode: RecordMode::Off,
            record: DabRecord::default(),
            exotic,
            smudge,
            view_zoom: 1.0,
            view_rotation_rad: 0.0,
            view_flip: false,
            mask_mode: false,
            sel_mode: false,
            interval: Interval::AsPreset,
            base_dabs,
            base_linearize,
            base_aa,
            anti_alias: AntiAlias::AsPreset,
        })
    }

    /// CSP entry taper carried as metadata: (length in px-ish units, min
    /// pressure factor). `None` when the preset defines none.
    pub fn taper_hint(&self) -> Option<(f32, f32)> {
        self.taper_hint
    }

    /// The preset's display name: its `"name"` field, else its file stem.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Set the paint colour from straight (non-premultiplied) sRGB, 0..1.
    ///
    /// libmypaint stores colour as HSV base values, which is also what the
    /// `change_color_*` dynamics operate on — so this must go through HSV, not
    /// straight into the dab call.
    pub fn set_color_rgb(&mut self, rgb: [f32; 3]) {
        let (h, s, v) = rgb_to_hsv(rgb);
        unsafe {
            ffi::mypaint_brush_set_base_value(self.brush, setting::COLOR_H, h);
            ffi::mypaint_brush_set_base_value(self.brush, setting::COLOR_S, s);
            ffi::mypaint_brush_set_base_value(self.brush, setting::COLOR_V, v);
        }
    }

    /// Scale the brush size. `1.0` is the preset's own size.
    ///
    /// Radius is stored logarithmically, so a multiplier is an *addition* of
    /// `ln(m)`. Always re-derived from the value the preset shipped, so calling
    /// this twice does not compound.
    pub fn set_size_multiplier(&mut self, m: f32) {
        // Radius is exp()'d in the C; a non-positive multiplier would give
        // -inf/NaN and poison the dab loop.
        self.size_mul = if m.is_finite() && m > 1e-4 { m } else { 1e-4 };
        unsafe {
            ffi::mypaint_brush_set_base_value(
                self.brush,
                setting::RADIUS_LOGARITHMIC,
                self.base_radius_log + self.size_mul.ln(),
            )
        };
        // A FIXED-px interval is expressed to the engine as "dabs per BASIC
        // radius", and the basic radius is exactly what just moved — so the
        // conversion has to be redone or Fixed silently becomes relative
        // again the moment the Size slider is touched.
        if matches!(self.interval, Interval::FixedPx(_)) {
            self.apply_interval();
        }
    }

    /// Current size multiplier (what was last passed to `set_size_multiplier`).
    pub fn size_multiplier(&self) -> f32 {
        self.size_mul
    }

    /// The dab DIAMETER the preset shipped with, in canvas px — `exp()` of the
    /// radius the file carried, doubled. This is the size a sub tool DEFAULTS
    /// to, never a ceiling: [`set_size_px`](Self::set_size_px) will happily go
    /// far above it.
    pub fn base_size_px(&self) -> f32 {
        self.base_radius_log.exp() * 2.0
    }

    /// Set the dab DIAMETER in canvas px — the number artists think in, and
    /// what the Size control and the `[`/`]` ladder both write.
    ///
    /// Expressed as a multiplier of the preset's own size and pushed through
    /// [`set_size_multiplier`](Self::set_size_multiplier), so it inherits that
    /// method's contract exactly: re-derived from `base_radius_log` every
    /// time, hence setting the same size twice is setting it once.
    pub fn set_size_px(&mut self, px: f32) {
        self.set_size_multiplier(px / self.base_size_px().max(1e-4));
    }

    /// Base dab radius in canvas pixels, multiplier included. Radius is stored
    /// logarithmically, so this is the `exp()` the C would take. Pressure and
    /// speed dynamics move around this value — it is what a size readout should
    /// show, not a promise about any one dab.
    pub fn radius_px(&self) -> f32 {
        self.base_value(setting::RADIUS_LOGARITHMIC).exp()
    }

    /// Set the base opacity — libmypaint's `opaque`, 0..1, the alpha a stroke
    /// reaches where it is fully laid down.
    ///
    /// Only the *base*: whether pressure moves it is the separate
    /// `opaque_multiply` mapping, which belongs to the preset. That split is
    /// deliberate. The owner's Real G-Pen inks at constant alpha (its CSP opacity and
    /// flow effectors have no source enabled), so its preset pins
    /// `opaque_multiply` to 1.0 and this setter scales the whole stroke evenly;
    /// a preset that *does* ramp alpha with pressure keeps its ramp and this
    /// scales its ceiling.
    pub fn set_base_opacity(&mut self, o: f32) {
        let o = if o.is_finite() {
            o.clamp(0.0, 1.0)
        } else {
            1.0
        };
        unsafe { ffi::mypaint_brush_set_base_value(self.brush, setting::OPAQUE, o) };
    }

    /// Current base opacity (`opaque`), 0..1.
    pub fn base_opacity(&self) -> f32 {
        self.base_value(setting::OPAQUE)
    }

    /// Set the floor of the pressure→size response: the lightest possible dab
    /// becomes `pct` % of the dab at full pressure. CSP calls this "minimum
    /// value" on the brush-size effector; his Real G-Pen sits at 3 %.
    ///
    /// This **replaces** the preset's pressure→`radius_logarithmic` mapping
    /// with a canonical four-point curve rather than rescaling whatever shape
    /// was there: the 35 classic MyPaint presets have wildly different shapes
    /// (`pen.myb` is `(0,0)→(1,0.5)`, i.e. it grows *above* the base radius
    /// with pressure and never goes thin, which is exactly the "minimum size is
    /// too high" complaint), and there is no shape-independent way to rescale
    /// them. The curve is the composed CSP one: concave, still only a quarter
    /// width at three-quarter pressure, saturating where his measured pressure
    /// calibration saturates.
    ///
    /// Full-pressure width is preserved exactly — the y at pressure 1 is read
    /// from the existing mapping and re-emitted — so this is idempotent and
    /// composes with [`set_size_multiplier`](Self::set_size_multiplier), which
    /// only touches the base value. Other inputs' mappings (`speed1`, …) are
    /// left alone.
    pub fn set_size_min_pct(&mut self, pct: f32) {
        let pct = if pct.is_finite() {
            pct.clamp(0.0, 100.0)
        } else {
            100.0
        };
        // ln(0) would poison every dab radius; 0 % means "as thin as it goes".
        let floor = (pct / 100.0).max(MIN_SIZE_FACTOR);
        let full = eval_mapping(&self.pressure_size_points(), 1.0);
        let knee = floor + (1.0 - floor) * SIZE_KNEE_Y;
        let points = [
            (0.0, full + floor.ln()),
            (SIZE_KNEE_X, full + knee.ln()),
            (PRESSURE_SATURATION, full),
            (1.0, full),
        ];
        let (id, input) = (setting::RADIUS_LOGARITHMIC, input::PRESSURE);
        unsafe {
            ffi::mypaint_brush_set_mapping_n(self.brush, id, input, points.len() as c_int);
            for (i, (x, y)) in points.iter().enumerate() {
                ffi::mypaint_brush_set_mapping_point(self.brush, id, input, i as c_int, *x, *y);
            }
        }
    }

    /// The current floor of the pressure→size response, in percent of the
    /// full-pressure dab. Derived from the mapping, not remembered, so it is
    /// also an honest reading of a preset nobody has touched: `pen.myb` reports
    /// ~61 %, the CSP Real G-Pen 3 %. `100` means size does not follow pressure.
    pub fn size_min_pct(&self) -> f32 {
        let points = self.pressure_size_points();
        if points.is_empty() {
            return 100.0;
        }
        let (low, full) = (eval_mapping(&points, 0.0), eval_mapping(&points, 1.0));
        ((low - full).exp() * 100.0).clamp(0.0, 100.0)
    }

    /// The pressure→`radius_logarithmic` mapping as it currently stands.
    fn pressure_size_points(&self) -> Vec<(f32, f32)> {
        let (id, input) = (setting::RADIUS_LOGARITHMIC, input::PRESSURE);
        self.pressure_points(id, input)
    }

    /// Configure brush-size randomization (CSP 乱数).
    ///
    /// `amount` is the deviation at full pressure; `min_pct` the floor at zero
    /// pressure, % of `amount` — together they form a linear pressure→
    /// deviation curve, exactly the shape the size and opacity controls give.
    /// In stock mode the unit is log-radius (deviation scales **with** brush
    /// size); with `absolute_px` it is canvas pixels around the current dab
    /// radius — size-independent, via the vendored hook in
    /// `mypaint-brush.c` (vendor/PATCHES.md).
    ///
    /// Like [`set_size_min_pct`](Self::set_size_min_pct), this replaces the
    /// preset's own mapping for the setting; only call it when the user has
    /// actually moved the control (read back via [`randomization`](Self::randomization)).
    pub fn set_randomization(&mut self, amount: f32, min_pct: f32, absolute_px: bool) {
        let amount = if amount.is_finite() {
            amount.max(0.0)
        } else {
            0.0
        };
        let min = (min_pct.clamp(0.0, 100.0) / 100.0).min(1.0) * amount;
        self.radius_random_abs = absolute_px && amount > 0.0;
        let (id, input) = (setting::RADIUS_BY_RANDOM, input::PRESSURE);
        unsafe {
            if amount <= 0.0 {
                ffi::mypaint_brush_set_base_value(self.brush, id, 0.0);
                ffi::mypaint_brush_set_mapping_n(self.brush, id, input, 0);
            } else if min >= amount - 1e-6 {
                // Flat: a curve would clamp the noise floor to the ceiling.
                ffi::mypaint_brush_set_base_value(self.brush, id, amount);
                ffi::mypaint_brush_set_mapping_n(self.brush, id, input, 0);
            } else {
                ffi::mypaint_brush_set_base_value(self.brush, id, 0.0);
                ffi::mypaint_brush_set_mapping_n(self.brush, id, input, 2);
                ffi::mypaint_brush_set_mapping_point(self.brush, id, input, 0, 0.0, min);
                ffi::mypaint_brush_set_mapping_point(self.brush, id, input, 1, 1.0, amount);
            }
        }
    }

    /// Read back the randomization as the UI seeded it: (amount at full
    /// pressure, floor at zero pressure as % of amount, absolute-px mode).
    /// Derived from the live mapping, so it is also an honest reading of a
    /// preset nobody has touched.
    pub fn randomization(&self) -> (f32, f32, bool) {
        let (id, input) = (setting::RADIUS_BY_RANDOM, input::PRESSURE);
        let points = self.pressure_points(id, input);
        let base = self.base_value(id);
        let (at0, at1) = match points.as_slice() {
            [] => (base, base),
            _ => (
                base + eval_mapping(&points, 0.0),
                base + eval_mapping(&points, 1.0),
            ),
        };
        let amount = at1.max(0.0);
        let min_pct = if amount > 1e-6 {
            (at0.max(0.0) / amount * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        (amount, min_pct, self.radius_random_abs)
    }

    /// The pressure mapping of one setting, as point pairs.
    fn pressure_points(&self, id: c_int, input: c_int) -> Vec<(f32, f32)> {
        let n = self.mapping_n(id, input);
        (0..n)
            .map(|i| {
                let (mut x, mut y) = (0.0f32, 0.0f32);
                unsafe {
                    ffi::mypaint_brush_get_mapping_point(self.brush, id, input, i, &mut x, &mut y)
                };
                (x, y)
            })
            .collect()
    }

    /// The mapping of one setting for one input, as point pairs — the curve
    /// editor's read path. Empty = the setting does not respond to the input.
    /// Works for every setting × input combination libmypaint has, not just
    /// pressure (Krita's per-sensor curves).
    pub fn mapping(&self, setting_id: c_int, input_id: c_int) -> Vec<(f32, f32)> {
        self.pressure_points(setting_id, input_id)
    }

    /// Replace one setting's mapping for one input — the curve editor's write
    /// path. Points must be x-ascending (libmypaint interpolates segment-wise
    /// and extrapolates the outer segments); an empty slice turns the
    /// response off.
    pub fn set_mapping(&mut self, setting_id: c_int, input_id: c_int, points: &[(f32, f32)]) {
        unsafe {
            ffi::mypaint_brush_set_mapping_n(
                self.brush,
                setting_id,
                input_id,
                points.len() as c_int,
            );
            for (i, (x, y)) in points.iter().enumerate() {
                ffi::mypaint_brush_set_mapping_point(
                    self.brush, setting_id, input_id, i as c_int, *x, *y,
                );
            }
        }
    }

    /// Toggle eraser mode (libmypaint's `eraser` setting: dabs subtract alpha).
    pub fn set_eraser(&mut self, on: bool) {
        unsafe {
            ffi::mypaint_brush_set_base_value(
                self.brush,
                setting::ERASER,
                if on { 1.0 } else { 0.0 },
            )
        };
    }

    /// Smudge weight — the test harness's knob (presets set it at load).
    /// Sets BOTH the engine setting and the routing flag the app reads.
    pub fn set_smudge(&mut self, v: f32) {
        unsafe {
            ffi::mypaint_brush_set_base_value(self.brush, setting::SMUDGE, v.clamp(0.0, 1.0))
        };
        self.smudge = v > 0.0;
    }

    /// Colorize stamp weight (`BlendMode_Color`, GPU-ported in the P4
    /// round): 0 = off. Load-time `exotic` detection is unaffected — this
    /// is the test harness's knob.
    pub fn set_colorize(&mut self, v: f32) {
        unsafe {
            ffi::mypaint_brush_set_base_value(self.brush, setting::COLORIZE, v.clamp(0.0, 1.0))
        };
    }

    /// Posterize stamp weight + level knob (the .myb `posterize_num` scale
    /// the C multiplies by 100 and clamps 1..=128). Same P4 test knob.
    pub fn set_posterize(&mut self, v: f32, num: f32) {
        unsafe {
            ffi::mypaint_brush_set_base_value(self.brush, setting::POSTERIZE, v.clamp(0.0, 1.0));
            ffi::mypaint_brush_set_base_value(self.brush, setting::POSTERIZE_NUM, num);
        };
    }

    /// Krita-style hard stamp dabs (vendor/PATCHES.md): exact anti-aliased
    /// discs instead of the gaussian hardness falloff — the crisp ink edge
    /// CSP pens have. Off (the default) keeps stock behaviour pixel-for-pixel.
    pub fn set_hard_dab(&mut self, on: bool) {
        self.hard_dab = on;
    }

    pub fn hard_dab(&self) -> bool {
        self.hard_dab
    }

    /// Krita Scatter: each dab's centre jitters within `radius * scatter` of
    /// the stroke path (0 = off, stock behaviour).
    pub fn set_scatter(&mut self, scatter: f32) {
        self.scatter = if scatter.is_finite() {
            scatter.clamp(0.0, 4.0)
        } else {
            0.0
        };
    }

    pub fn scatter(&self) -> f32 {
        self.scatter
    }

    /// CSP Advanced ▸ Stroke ▸ Interval (S-028): how far apart the dabs sit.
    ///
    /// [`Interval::AsPreset`] restores the two `dabs_per_*` base values the
    /// `.myb` shipped, so the control is a true no-op until it is moved.
    /// The two live modes each drive ONE term and zero the other, because
    /// they are answering different questions: `Percent` is dabs per ACTUAL
    /// radius (spacing tracks the dab, so it survives pressure and the Size
    /// slider), `FixedPx` is dabs per BASIC radius converted from the current
    /// radius (spacing is a canvas distance, so it does not).
    ///
    /// A preset with its own `dabs_per_second` keeps it either way: that term
    /// is time-driven, not distance-driven, and it is CSP's separate
    /// "Continuous spraying" row (S-029), not this one.
    pub fn set_interval(&mut self, interval: Interval) {
        self.interval = match interval {
            Interval::Percent(p) if p.is_finite() => {
                Interval::Percent(p.clamp(Interval::MIN_PCT, Interval::MAX_PCT))
            }
            Interval::FixedPx(g) if g.is_finite() => {
                Interval::FixedPx(g.clamp(Interval::MIN_PX, Interval::MAX_PX))
            }
            // A non-finite number would reach the engine as a NaN dab count.
            Interval::Percent(_) | Interval::FixedPx(_) | Interval::AsPreset => Interval::AsPreset,
        };
        self.apply_interval();
    }

    /// The interval as the user set it (NOT re-derived from the engine: a
    /// preset's own spacing has no mode, which is what `AsPreset` says).
    pub fn interval(&self) -> Interval {
        self.interval
    }

    /// The distance-driven gap between dabs at the current base radius, in
    /// canvas px — the honest readout for the panel, whatever mode set it.
    /// `f32::INFINITY` when the preset stamps only on the clock.
    pub fn dab_gap_px(&self) -> f32 {
        let per_radius = self.base_value(setting::DABS_PER_ACTUAL_RADIUS)
            + self.base_value(setting::DABS_PER_BASIC_RADIUS);
        if per_radius > 0.0 {
            self.radius_px() / per_radius
        } else {
            f32::INFINITY
        }
    }

    /// Push `self.interval` into the engine's two dab-count terms.
    fn apply_interval(&mut self) {
        let (actual, basic) = match self.interval {
            Interval::AsPreset => self.base_dabs,
            Interval::Percent(p) => (dabs_per_radius(100.0 / (2.0 * p)), 0.0),
            Interval::FixedPx(g) => (0.0, dabs_per_radius(self.radius_px() / g)),
        };
        unsafe {
            ffi::mypaint_brush_set_base_value(self.brush, setting::DABS_PER_ACTUAL_RADIUS, actual);
            ffi::mypaint_brush_set_base_value(self.brush, setting::DABS_PER_BASIC_RADIUS, basic);
        }
    }

    /// CSP Brush tip ▸ Adjust brush density by gap (B-029): compensate each
    /// dab's alpha for how many of them land on a pixel, so the gap stops
    /// deciding how dark the stroke comes out.
    ///
    /// libmypaint's `opaque_linearize` is the same idea and the same math —
    /// `alpha_dab = 1 − (1 − opaque)^(1/dabs_per_pixel)` — so this is a
    /// toggle over an amount, not a new mechanism. ON restores the amount the
    /// preset shipped (0.9 for every CSP-derived preset here, which is also
    /// libmypaint's stock default) or [`DENSITY_BY_GAP_DEFAULT`] for a preset
    /// that shipped it off; OFF is a flat zero.
    ///
    /// It only ever LOWERS per-dab alpha — the C clamps its dab count at 1
    /// first ("the correction is probably not wanted if the dabs don't
    /// overlap") — so it cannot make a wide-gap stroke darker to compensate.
    pub fn set_density_by_gap(&mut self, on: bool) {
        let v = if !on {
            0.0
        } else if self.base_linearize > 0.0 {
            self.base_linearize
        } else {
            DENSITY_BY_GAP_DEFAULT
        };
        unsafe { ffi::mypaint_brush_set_base_value(self.brush, setting::OPAQUE_LINEARIZE, v) };
    }

    /// Whether density-by-gap compensation is active (read from the engine,
    /// so it is also an honest reading of a preset nobody has touched).
    pub fn density_by_gap(&self) -> bool {
        self.base_value(setting::OPAQUE_LINEARIZE) > 0.0
    }

    /// CSP Tool Settings ▸ Anti-aliasing (A-010): the four-level edge feather.
    /// [`AntiAlias::AsPreset`] re-states the preset's own value, so the
    /// control is a no-op until the user picks a rung.
    pub fn set_anti_alias(&mut self, aa: AntiAlias) {
        self.anti_alias = aa;
        let px = aa.feather_px().unwrap_or(self.base_aa);
        unsafe { ffi::mypaint_brush_set_base_value(self.brush, setting::ANTI_ALIASING, px) };
    }

    /// The AA level as the user set it (`AsPreset` until they pick one).
    pub fn anti_alias(&self) -> AntiAlias {
        self.anti_alias
    }

    /// The live minimum edge feather in canvas px — what the engine will
    /// actually enforce, preset value included.
    pub fn anti_alias_px(&self) -> f32 {
        self.base_value(setting::ANTI_ALIASING)
    }

    /// Krita Wash mode (flow vs opacity). See the field docs — the short
    /// version: `on` makes one stroke composite once at `stroke_opacity`
    /// instead of per dab, with `blend` as its compositing mode. Off keeps
    /// stock build-up, pixel-for-pixel.
    pub fn set_wash(&mut self, on: bool, stroke_opacity: f32, blend: Blend) {
        self.wash = on;
        self.wash_opacity = if stroke_opacity.is_finite() {
            stroke_opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.wash_blend = blend;
        // A toggle must never leak a live buffer: if this lands mid-session
        // (it can only land between strokes) the old buffer, if any, held a
        // stroke that already ended.
        self.wash_buf = None;
    }

    pub fn wash(&self) -> bool {
        self.wash
    }

    pub fn wash_opacity(&self) -> f32 {
        self.wash_opacity
    }

    pub fn set_wash_opacity(&mut self, o: f32) {
        self.wash_opacity = if o.is_finite() {
            o.clamp(0.0, 1.0)
        } else {
            1.0
        };
    }

    pub fn set_wash_blend(&mut self, blend: Blend) {
        self.wash_blend = blend;
    }

    pub fn wash_blend(&self) -> Blend {
        self.wash_blend
    }

    /// Per-dab alpha inside a wash stroke (Krita: Flow). In wash mode this is
    /// what the Flow slider drives; it is the same knob as
    /// [`set_base_opacity`](Self::set_base_opacity), named for the UI that
    /// shows both.
    pub fn set_flow(&mut self, flow: f32) {
        self.set_base_opacity(flow);
    }

    /// Krita texture tip: multiply every dab's profile by this grayscale
    /// mask. `None` = stock, pixel-for-pixel.
    pub fn set_texture(&mut self, mask: Option<Arc<TextureMask>>) {
        self.texture = mask;
        self.tex_accum = (0.0, 0.0);
    }

    pub fn texture(&self) -> Option<&Arc<TextureMask>> {
        self.texture.as_ref()
    }

    /// Texture crawl per dab in mask px (0 = static pattern).
    /// #10 amendment 2: whether the mask stamps per dab (rotating with the
    /// dab's elliptical angle) instead of reading as canvas grain. The GPU
    /// flush and the repair rasterizer must follow the same mode.
    pub fn texture_anchor_dab(&self) -> bool {
        self.texture_anchor_dab
    }

    pub fn set_texture_anchor_dab(&mut self, on: bool) {
        self.texture_anchor_dab = on;
    }

    /// Stamp rotation follows the stroke direction (dab-anchored mode).
    pub fn set_texture_rotate_direction(&mut self, on: bool) {
        self.texture_rotate_direction = on;
    }

    /// Stamp base angle, degrees (dab-anchored mode).
    pub fn set_texture_angle_deg(&mut self, deg: f32) {
        self.texture_angle_deg = if deg.is_finite() { deg } else { 0.0 };
    }

    pub fn set_texture_scroll(&mut self, px_per_dab: f32) {
        self.texture_scroll_px = if px_per_dab.is_finite() {
            px_per_dab.clamp(0.0, 64.0)
        } else {
            0.0
        };
    }

    pub fn texture_scroll(&self) -> f32 {
        self.texture_scroll_px
    }

    /// Krita SKETCH mode: `Some(params)` links the stroke back to its recent
    /// history within `distance` px (hatching webs); `None` = stock.
    pub fn set_sketch(&mut self, params: Option<SketchParams>) {
        self.sketch = params;
        self.history.clear();
    }

    pub fn sketch(&self) -> Option<SketchParams> {
        self.sketch
    }

    /// GPU-dabs P0: set the record mode and clear any previous record. In
    /// [`Tap`](RecordMode::Tap) pixels are unchanged; [`Bypass`] stops the
    /// CPU from rasterizing entirely (the P1 compute path rasterizes from
    /// the record instead).
    pub fn set_dab_recording(&mut self, mode: RecordMode) {
        self.record_mode = mode;
        self.record = DabRecord::default();
    }

    /// Whether this brush can run the P1 GPU dab path: the shader ports the
    /// gaussian and hard-stamp masks + Normal / Normal-and-Eraser / LockAlpha
    /// blends, the canvas-anchored texture-tip multiply, WASH strokes (GPU
    /// dabs rasterize into a sentinel wash buffer; the stroke-end commit
    /// reuses the CPU `commit_wash` math, so the wet semantics are identical
    /// by construction), and — since #0.1 part 3 — SMUDGE strokes (the
    /// per-dab canvas sample is served from the GPU tile cache through the
    /// surface's tile oracle; the dabs themselves were always ordinary).
    /// Still CPU: smudge+wash (v1: the sampler would have to read the
    /// sentinel wash buffer, not the layer — deferred with the twins-wash
    /// case), spectral paint, colorize, posterize.
    pub fn gpu_ready(&self) -> bool {
        // wash+smudge stays CPU — MEASURED, not assumed (P4 attempt,
        // 2026-08-21): the C's `get_color` PROCESSES THE PENDING OP QUEUE
        // before sampling (get_color_internal → process_tile_internal), so
        // the CPU sampler sees every dab up to the current one, per dab.
        // A batched GPU path can only show dabs up to the last flush; for
        // plain smudge that gap is invisible (the sampler mostly reads
        // pre-existing ink), but a wash stroke's sampler reads ONLY the
        // stroke's own accumulation — pure self-feedback — and the
        // intra-batch gap compounded to ~19% channel drift on the parity
        // harness (`gpu_dab_parity_wash_smudge`, kept #[ignore]d as the
        // re-entry point). Per-dab round trips are the design doc's
        // non-starter, so the honest answer is CPU. The app-side wiring
        // (wash-key oracle + per-sample wash flush) is in place and
        // correct for the day a cheaper visibility trick exists.
        !self.exotic && !(self.wash && self.smudge)
    }

    /// Whether the preset samples the canvas per dab (the `smudge`
    /// setting) — the app's per-sample dispatch + tile-oracle wiring keys
    /// off this.
    pub fn smudge(&self) -> bool {
        self.smudge
    }

    /// The live wash buffer, if a wash stroke is in flight (created at
    /// `begin`; under GPU BYPASS it stays blank — the GPU sentinel buffer
    /// replaces it, but its EXISTENCE marks wash mode for the flush path).
    pub fn wash_buffer(&self) -> Option<&Document> {
        self.wash_buf.as_deref()
    }

    /// Claim the wash buffer after a GPU wash stroke (#0.1): `end` leaves it
    /// alive under BYPASS (the GPU owns the commit); the app drops it here
    /// once the readback commit has run, so no stroke's buffer outlives its
    /// stroke.
    pub fn take_wash_buffer(&mut self) -> Option<Box<Document>> {
        self.wash_buf.take()
    }

    /// (stroke opacity, blend, erase-arm) for the stroke-end wash commit.
    pub fn wash_commit_params(&self) -> (f32, Blend, bool) {
        (self.wash_opacity, self.wash_blend, self.wash_erase)
    }

    /// Take the recorded dabs and touched tiles of the strokes so far.
    pub fn take_dab_record(&mut self) -> DabRecord {
        std::mem::take(&mut self.record)
    }

    /// Dabs the C side clamped to the per-dab tile budget (PATCHES.md #19)
    /// so far — take-and-reset, so one stroke reads exactly its own
    /// clamps. Associated (no `self`) on purpose: the counter is
    /// per-THREAD and the app strokes symmetry/wrap twins through it too,
    /// so the count belongs to the stroke, not to one brush instance.
    pub fn take_dab_clamp_count() -> u32 {
        DAB_CLAMP_COUNT.with(|c| c.replace(0))
    }

    /// Publish the view transform for the speed/direction input compensation
    /// (PATCHES.md #12). `1.0` / `0.0` / `false` — the defaults — reproduce
    /// the stock legacy `stroke_to` exactly. `rotation_rad` is RADIANS — the
    /// C applies `DEGREES()` to it itself; the vendored "@viewrotation: in
    /// degrees" docstring is an upstream doc bug (MyPaint passes
    /// `tdw.rotation`). `flip_h` mirrors the motion-direction inputs under a
    /// horizontally flipped view — `rotation_rad` stays the same stored
    /// field either way (the C's DX negation carries the mirror; do not
    /// assume a pre-negated value). The app sets this per input batch so
    /// mid-stroke view changes stay correct too.
    pub fn set_view(&mut self, zoom: f32, rotation_rad: f32, flip_h: bool) {
        // The C divides nothing by it but multiplies velocities, and its own
        // _2 entry documents > 0 as a hard requirement.
        self.view_zoom = if zoom.is_finite() && zoom > 0.0 {
            zoom
        } else {
            1.0
        };
        self.view_rotation_rad = if rotation_rad.is_finite() {
            rotation_rad
        } else {
            0.0
        };
        self.view_flip = flip_h;
    }

    /// One LCG step, uniform 0..1.
    ///
    /// Take the top **32** bits, not 33: `>> 33` leaves 31 bits, which over a
    /// `u32::MAX` divisor only ever reaches 0.5 (audit 2026-08-17, finding
    /// M1). That halved range made the sketch engine's density gate fire at
    /// roughly double its setting, and confined every link target to the older
    /// half of the history ring.
    pub(crate) fn rng01(&mut self) -> f32 {
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.rng >> 32) as u32 as f32) / u32::MAX as f32
    }

    /// LM-004: route strokes to the active layer's mask. The caller
    /// guarantees the layer HAS one for the whole stroke.
    pub fn set_mask_mode(&mut self, on: bool) {
        self.mask_mode = on;
    }

    /// Route strokes to the DOCUMENT's selection scratch (selection pen
    /// / eraser / Quick Mask) — the caller re-states it per stroke.
    pub fn set_sel_mode(&mut self, on: bool) {
        self.sel_mode = on;
    }

    /// Read a setting's base value. Handy for tests and for a settings panel.
    /// Set one libmypaint base value directly (the mirror of
    /// [`Self::base_value`]). The named accessors stay the app's surface;
    /// this exists for tools and tests that speak setting ids.
    pub fn set_base_value(&mut self, setting_id: c_int, v: f32) {
        unsafe { ffi::mypaint_brush_set_base_value(self.brush, setting_id, v) };
    }

    pub fn base_value(&self, setting_id: c_int) -> f32 {
        unsafe { ffi::mypaint_brush_get_base_value(self.brush, setting_id) }
    }

    /// How many mapping points a setting has for one input; `0` means the
    /// setting does not respond to that input at all.
    pub fn mapping_n(&self, setting_id: c_int, input_id: c_int) -> c_int {
        unsafe { ffi::mypaint_brush_get_mapping_n(self.brush, setting_id, input_id) }
    }

    /// One `stroke_to`, bracketed by begin/end_atomic so the dab queue is
    /// flushed into tiles before we let go of the document.
    fn stroke_to(&mut self, doc: &mut Document, s: PenSample, dtime: f64) {
        // Non-finite input must never cross the FFI: a NaN pressure propagates
        // through the pressure→radius mapping into a NaN dab radius, and
        // libmypaint then computes garbage tile bounds and corrupts the heap
        // (found the hard way — `f32::clamp` keeps NaN).
        if !(s.x.is_finite() && s.y.is_finite() && s.pressure.is_finite()) {
            return;
        }
        // Per-call, before the FFI: the vendored absolute-randomization hook
        // reads this (single-threaded engine; previews and the active brush
        // each re-state their mode on every call, so no value leaks across).
        //
        // CONSTRAINT (auditor 0da3453, same shape as the bound guard one
        // layer down): set_texture_hook/set_record_hook park &mut self
        // fields as thread-local raw pointers across the FFI call and
        // nothing clears them on unwind — safe ONLY because stroke_to is
        // the crate's ONE FFI entry point and re-states every hook
        // unconditionally. A second entry point (a preview path, a batch
        // replay) must either re-state the hooks the same way or give
        // them the bound_guard treatment.
        set_radius_random_abs(self.radius_random_abs);
        set_hard_dab_flag(self.hard_dab);
        set_scatter_flag(self.scatter);
        set_texture_hook(
            self.texture.as_ref(),
            &mut self.tex_accum,
            self.texture_scroll_px,
            self.texture_anchor_dab,
            self.texture_rotate_direction,
            self.texture_angle_deg,
        );
        // M4: a tip list overrides the single mask's pointer at the first
        // advance anyway; arming the set is what makes dabs VARY. No list
        // (every stock preset) arms count 0 — the advance hook's early
        // return keeps this path bit-identical.
        if self.texture_tips.len() >= 2 {
            set_tip_set_hook(
                self.tip_variants.as_ptr(),
                self.tip_variants.len(),
                self.variation,
                self.texture_angle_deg,
                self.texture_anchor_dab && !self.texture_rotate_direction,
            );
        } else {
            set_tip_set_hook(std::ptr::null(), 0, 0.0, 0.0, false);
        }
        set_record_hook(self.record_mode, &mut self.record);
        let surface = self.surface.interface();
        // Wash mode paints into the stroke buffer, not the document — the
        // commit in `end` is the only thing that touches the layer. The
        // buffer outlives this call (it lives in `self`), so the raw pointer
        // keeps its validity window exactly like the document's.
        let wash_buf: Option<*mut Document> =
            self.wash_buf.as_mut().map(|b| &mut **b as *mut Document);
        let target = wash_buf.unwrap_or(doc as *mut Document);
        // Derive the raw pointer and then do not touch `doc` until `unbind`.
        let doc_ptr: *mut Document = target;
        // Clears BOTH bindings on unwind and on the normal exit path — a
        // panic inside the C callbacks must not leave a stale doc pointer
        // (or composite base) for the next stroke's tile fetches.
        let _bound = self.surface.bound_guard();
        unsafe {
            self.surface.bind(doc_ptr);
            self.surface.set_mask_mode(self.mask_mode && !self.sel_mode);
            self.surface.set_sel_mode(self.sel_mode);
            // SMUDGE-UNDER-WASH (TODO #6): the sampler reads buffer OVER
            // layer — the ink the user sees — not the blank buffer alone.
            // (Wash-smudge combos run CPU; the GPU deferral holds.)
            self.surface.bind_composite_base(
                wash_buf
                    .map(|_| doc as *mut Document)
                    .unwrap_or(std::ptr::null_mut()),
            );
            ffi::mypaint_surface_begin_atomic(surface);
            // View-aware entry (PATCHES.md #12): identical legacy dab
            // counting to the plain call, with the speed/direction inputs
            // computed in view space. Defaults (1.0, 0.0) are bit-identical
            // to stock `mypaint_brush_stroke_to`.
            ffi::mypaint_brush_stroke_to_view(
                self.brush,
                surface,
                s.x,
                s.y,
                s.pressure.clamp(0.0, 1.0),
                norm_tilt(s.tilt_x),
                norm_tilt(s.tilt_y),
                dtime,
                self.view_zoom,
                self.view_rotation_rad,
                self.view_flip as c_int,
            );
            let mut roi = ffi::MyPaintRectangle::default();
            ffi::mypaint_surface_end_atomic(surface, &mut roi);
        }
    }
}

impl Drop for MyBrush {
    fn drop(&mut self) {
        unsafe { ffi::mypaint_brush_unref(self.brush) };
    }
}

impl StrokeSink for MyBrush {
    fn begin(&mut self, doc: &mut Document) {
        self.last_t_ms = None;
        self.tex_accum = (0.0, 0.0);
        self.history.clear();
        // Each stroke reseeds the link picker from the clock-ish counter, so
        // identical strokes still produce natural (non-repeating) webs.
        self.rng ^= self.rng.rotate_left(17);
        if self.wash {
            // Erase-with-wash: the buffer must record WHERE the dabs landed,
            // so the eraser setting is forced off for the stroke (dabs lay
            // paint into the buffer) and the commit subtracts instead. The
            // flag is restored at `end` so the engine's honest eraser state
            // survives the stroke.
            self.wash_erase = self.base_value(setting::ERASER) >= 0.5;
            if self.wash_erase {
                unsafe { ffi::mypaint_brush_set_base_value(self.brush, setting::ERASER, 0.0) };
            }
            let (w, h) = doc.size;
            self.wash_buf = Some(Box::new(Document::new(w, h)));
        }
        unsafe {
            // reset() makes the next stroke_to a pure "put the pen here" — it
            // seeds position/pressure state and paints nothing, which is how
            // libmypaint avoids a smear from wherever the last stroke ended.
            ffi::mypaint_brush_reset(self.brush);
            ffi::mypaint_brush_new_stroke(self.brush);
        }
    }

    fn sample(&mut self, doc: &mut Document, s: PenSample) {
        let dtime = match self.last_t_ms {
            // libmypaint divides by dtime; it clamps <= 0 itself, but a
            // backwards jump also makes it print to stdout, so clamp here.
            Some(prev) => ((s.t_ms - prev) / 1000.0).max(0.0001),
            None => FIRST_SAMPLE_DTIME,
        };
        self.last_t_ms = Some(s.t_ms);
        self.stroke_to(doc, s, dtime);

        // Krita sketch filaments: link the stroke to a nearby history point.
        // The link is drawn BY the engine itself — a stroke_to at the target
        // makes libmypaint lay dabs from the current position to it, and the
        // next real sample draws the return leg. Together with the pen path
        // that is the sketch web.
        if let Some(p) = self.sketch
            && self.history.len() > 4
            && self.rng01() < p.density
        {
            // Try a few random targets; link the first within distance.
            for _ in 0..3 {
                let idx = (self.rng01() * self.history.len() as f32) as usize;
                let (hx, hy) = self.history[idx.min(self.history.len() - 1)];
                let (dx, dy) = (hx - s.x, hy - s.y);
                if dx * dx + dy * dy > p.distance * p.distance {
                    continue;
                }
                // March to the target in three quick steps (small dtimes so
                // speed-driven dynamics stay calm), same pressure.
                for k in 1..=3u32 {
                    let t = k as f32 / 3.0;
                    let link = PenSample {
                        x: s.x + dx * t,
                        y: s.y + dy * t,
                        ..s
                    };
                    self.stroke_to(doc, link, 0.004);
                }
                break;
            }
        }

        // Ring buffer of recent positions (real samples only — the filaments
        // are connections, not new anchors).
        const HISTORY: usize = 96;
        if self.history.len() >= HISTORY {
            self.history.remove(0);
        }
        self.history.push((s.x, s.y));
    }

    fn end(&mut self, doc: &mut Document) {
        self.last_t_ms = None;
        // Under BYPASS the buffer is blank by construction (the CPU never
        // rasterized) and the GPU path owns the stroke-end commit: leave the
        // buffer alive for the app's flush — it seeds the GPU tiles and is
        // claimed via `take_wash_buffer` after the readback commit. Only the
        // eraser restore is ours on that path; without it a wash-erase GPU
        // stroke would leave the engine's eraser forced off.
        if self.record_mode == RecordMode::Bypass {
            if self.wash_erase {
                unsafe { ffi::mypaint_brush_set_base_value(self.brush, setting::ERASER, 1.0) };
            }
        } else if let Some(buf) = self.wash_buf.take() {
            if self.wash_erase {
                unsafe { ffi::mypaint_brush_set_base_value(self.brush, setting::ERASER, 1.0) };
            }
            commit_wash(
                &buf,
                doc,
                self.wash_opacity,
                self.wash_blend,
                self.wash_erase,
            );
        }
        doc.revision = mn_core::next_revision();
    }
}

/// Composite a finished wash stroke buffer onto the document's active layer.
///
/// Paint: one src-over (or the chosen blend) per pixel with the buffer
/// pre-scaled by the stroke opacity — overlapping dabs inside the buffer
/// cannot push the result past that opacity, which is the whole point of
/// wash mode. Erase: the buffer's alpha is an erase mask; the destination
/// keeps its colour and loses `stroke_opacity × mask` of its coverage,
/// saturating exactly like the paint arm.
///
/// Runs inside the caller's `begin_op` bracket (the app's stroke path), so
/// the tile writes snapshot their pre-images and the stroke stays one undo
/// step; `mask_op_to_selection`/`mask_op_to_alpha` apply after it, as for
/// any stroke.
pub fn commit_wash(
    buf: &Document,
    doc: &mut Document,
    stroke_opacity: f32,
    blend: Blend,
    erase: bool,
) {
    let op = stroke_opacity.clamp(0.0, 1.0);
    // Collect the source tiles first: the destination walk below mutates the
    // layer (Arc::make_mut may copy-on-write), and the source side is a
    // different document anyway — but cloning the Arcs keeps the loop body
    // free of cross-document borrow puzzles.
    let src_tiles: Vec<(TileIdx, Arc<Tile>)> = buf
        .active_layer()
        .tiles()
        .map(|(i, t)| (i, t.clone()))
        .collect();
    for (idx, src) in src_tiles {
        if src.is_blank() {
            continue;
        }
        let s = src.data();
        let d = doc.active_layer_mut().tile_mut(idx).data_mut();
        for (dst, sp) in d.chunks_exact_mut(4).zip(s.chunks_exact(4)) {
            if sp[3] == 0 {
                continue;
            }
            let out = if erase {
                let k = fix15_to_f32(sp[3]) * op;
                let dpx = px_to_f32([dst[0], dst[1], dst[2], dst[3]]);
                [
                    dpx[0] * (1.0 - k),
                    dpx[1] * (1.0 - k),
                    dpx[2] * (1.0 - k),
                    dpx[3] * (1.0 - k),
                ]
            } else {
                blend_premul(
                    blend,
                    scale_opacity(px_to_f32([sp[0], sp[1], sp[2], sp[3]]), op),
                    px_to_f32([dst[0], dst[1], dst[2], dst[3]]),
                )
            };
            for c in 0..4 {
                dst[c] = f32_to_fix15(out[c]);
            }
        }
    }
}

/// `dtime` for the first sample of a stroke, in seconds.
///
/// Must exceed libmypaint's `max_dtime` (5.0), which is the *other* trigger for
/// the "this input is unrelated to the current state" branch in `stroke_to`.
///
/// `mypaint_brush_reset()` on its own is **not** enough, and the reason is a
/// statement-ordering detail in `mypaint-brush.c`: the `slow_tracking` smoothing
///
/// ```c
/// x = STATE(X) + (x - STATE(X)) * fac;   // fac ~ 0 for a small dtime
/// ```
///
/// runs *before* the `if (dtime > max_dtime || reset_requested)` block that
/// snaps `STATE(X)` to the new position. With a small dtime, `fac` is near zero,
/// so the pen gets planted next to wherever the brush state happened to be —
/// the canvas origin for a freshly loaded preset — and the following samples
/// smear a line from there to where the user actually touched down. A large
/// dtime makes `fac` ~ 1, so the smoothing is a no-op and the snap is exact.
///
/// Measured before the fix: a stroke at y=256 painted a bounding box from
/// (0, 1) to (399, 257). After: it stays on the stroke.
const FIRST_SAMPLE_DTIME: f64 = 10.0;

/// Density-by-gap amount for a preset that shipped `opaque_linearize` at
/// zero and is then switched ON. libmypaint's own stock default, and the
/// value every CSP-derived preset in `assets/brushes/csp` carries — not a
/// number chosen here.
pub const DENSITY_BY_GAP_DEFAULT: f32 = 0.9;

/// Ceiling on dabs per radius. The engine stamps this many dabs for every
/// radius of travel, so an unbounded value is an unbounded stroke cost (and
/// `Interval::MIN_PX` on a fat brush would ask for one). 50 is `MIN_PCT`'s
/// own density — the two limits are the same limit.
const MAX_DABS_PER_RADIUS: f32 = 50.0;

/// Clamp a dabs-per-radius term into something the dab loop survives.
fn dabs_per_radius(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, MAX_DABS_PER_RADIUS)
    } else {
        0.0
    }
}

/// Thinnest dab [`MyBrush::set_size_min_pct`] will ask for, as a fraction of the
/// full-pressure dab. `0 %` is a legitimate thing to want and a fatal thing to
/// compute with: radius is `exp(base + mapping)`, so a mapping of `ln(0)` is
/// `-inf` and every dab after it is NaN.
const MIN_SIZE_FACTOR: f32 = 0.01;
/// Knee of the canonical pressure→size curve, in raw pressure. This and
/// `SIZE_KNEE_Y` are the owner's Real G-Pen curve `(0.745, 0.255)` composed with his
/// measured pressure calibration (`docs/CSP-TOOLS.md`): thin is cheap, fat
/// needs commitment.
const SIZE_KNEE_X: f32 = 0.5609;
/// Size factor at the knee, before the `min%` floor is folded in.
const SIZE_KNEE_Y: f32 = 0.2545;
/// Where his calibrated pressure reaches full ink — above this, size is flat.
const PRESSURE_SATURATION: f32 = 0.7525;

// The vendored absolute-randomization mode flag (see mypaint-brush.c and
// vendor/PATCHES.md): non-zero = `radius_by_random` is a pixel deviation
// independent of brush size. THREAD-LOCAL: the C side reads it once per dab
// inside one `stroke_to`, so a process-global would let brushes on other
// threads clobber the mode mid-stroke (parallel test runners do exactly
// that); per-thread state matches the one-brush-per-thread reality.
thread_local! {
    static RADIUS_RANDOM_ABS_PX: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static HARD_DAB: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static SCATTER: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// Texture-tip state (round 26). The accumulator lives in the OWNING
    /// brush — the raw pointer is set per `stroke_to` (never dereferenced
    /// outside one, so it cannot dangle) and advanced by the C-side per-dab
    /// hook. Same single-engine-thread contract as the flags above.
    static TEXTURE_PTR: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEXTURE_SIZE: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    static TEXTURE_SCROLL_X: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static TEXTURE_SCROLL_Y: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static TEXTURE_ACCUM: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEXTURE_STEP: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// #10 amendment 2: 1 = the mask is DAB-anchored (a stamped tip), 0 =
    /// canvas-anchored grain.
    static TEXTURE_ANCHOR_DAB: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    /// Stamp rotation: 0 = fixed base only, 1 = base + stroke direction.
    /// The C hands the UNFOLDED direction per dab (`mnc_brush_texture_stamp`)
    /// because the elliptical angle folds mod 180 — right for an ellipse,
    /// wrong for a stamp; the published angle is what the dab renders with.
    static TEXTURE_STAMP_DIRECTION: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    static TEXTURE_STAMP_BASE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static TEXTURE_STAMP_ANGLE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// GPU-dabs record mode (round 27, PATCHES.md #11): the pointer is set
    /// per `stroke_to` to the owning brush's recorder — only dereferenced
    /// inside one call, never across strokes.
    static RECORD_MODE: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    static RECORD_BUF: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Per-dab tile budget (PATCHES.md #19): max tiles one dab may touch
    /// before `draw_dab_internal` clamps its radius. 1024 = a 32×32-tile
    /// ≈ 2048 px square dab — above the size slider's 2000 px ceiling, so
    /// every hand-authored brush renders bit-identically; the guard exists
    /// for imported tips whose stored "size" is not pixels. 0 = unlimited
    /// (stock).
    static DAB_TILE_BUDGET: std::cell::Cell<u32> = const { std::cell::Cell::new(1024) };
    /// Dabs clamped by that budget this stroke (PATCHES.md #19) — read via
    /// [`MyBrush::take_dab_clamp_count`].
    static DAB_CLAMP_COUNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// M4 generated variation: the ACTIVE-TIP SET for this stroke — a
    /// leaked box of (data ptr, size) pairs, one per VARIANT (each tip,
    /// plus mirrored copies when variation > 0). The per-dab advance hook
    /// swaps TEXTURE_PTR/SIZE among these, seeded-random, so the C sampler
    /// and the GPU record both see the swap by construction.
    static TIP_SET: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TIP_COUNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// The variant rng (xorshift64), fixed seed per stroke series —
    /// identical strokes stamp identically (pinned); variation is between
    /// DABS, not between strokes.
    static TIP_RNG: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static VARIATION: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// The un-jittered stamp angle (advance re-publishes base ± jitter).
    static TIP_ANGLE_BASE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// 1 = base-angle mode, so jitter may apply (direction mode
    /// re-publishes its own angle per dab and would eat it).
    static TIP_JITTER_OK: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// The mirrored copy of a tip mask (M4's variation variants).
fn mirror_mask(t: &TextureMask, flip_x: bool, flip_y: bool) -> TextureMask {
    let s = t.size as usize;
    let mut out: Vec<u8> = t.data.as_ref().clone();
    if flip_x {
        for y in 0..s {
            let row = &mut out[y * s..(y + 1) * s];
            row.reverse();
        }
    }
    if flip_y {
        for y in 0..s / 2 {
            let (a, b) = (y * s, (s - 1 - y) * s);
            for i in 0..s {
                out.swap(a + i, b + i);
            }
        }
    }
    TextureMask {
        name: t.name.clone(),
        size: t.size,
        data: Arc::new(out),
    }
}

/// Point the vendored record hooks at this brush's recorder for the coming
/// `stroke_to` (same validity-window discipline as the texture accumulator).
fn set_record_hook(mode: RecordMode, record: *mut DabRecord) {
    RECORD_MODE.with(|c| {
        c.set(match mode {
            RecordMode::Off => 0,
            RecordMode::Tap => 1,
            RecordMode::Bypass => 2,
        })
    });
    RECORD_BUF.with(|c| c.set(record as usize));
}

/// Called from the patched `mypaint-tiled-surface.c` per dab (PATCHES.md
/// #11): append the dab and its touched-tile range (the C's own
/// `r_fringe = radius + 1` math, mirrored exactly).
#[unsafe(no_mangle)]
pub extern "C" fn mnc_record_dab(
    x: f32,
    y: f32,
    radius: f32,
    color_r: u16,
    color_g: u16,
    color_b: u16,
    color_a: f32,
    opaque: f32,
    hardness: f32,
    aspect_ratio: f32,
    angle: f32,
    lock_alpha: f32,
    paint: f32,
    tex_angle: f32,
    colorize: f32,
    posterize: f32,
    posterize_num: f32,
) {
    let p = RECORD_BUF.with(|c| c.get()) as *mut DabRecord;
    if p.is_null() {
        return;
    }
    unsafe {
        let tex_off = if TEXTURE_SIZE.with(|c| c.get()) > 0 {
            [
                f32::from_bits(TEXTURE_SCROLL_X.with(|c| c.get())) as i32,
                f32::from_bits(TEXTURE_SCROLL_Y.with(|c| c.get())) as i32,
            ]
        } else {
            [0, 0]
        };
        (*p).dabs.push(DabParams {
            x,
            y,
            radius,
            color: [color_r, color_g, color_b],
            alpha: color_a,
            opaque,
            hardness,
            aspect_ratio,
            angle,
            lock_alpha,
            paint,
            colorize,
            posterize,
            // Already CLAMP(ROUND(num*100), 1, 128) on the C side.
            posterize_num: posterize_num as u16,
            tex_off,
            tex_angle,
        });
        let r_fringe = radius + 1.0;
        // Mirror the C exactly: floor(floor(x ± r) / 64) — div_euclid because
        // Rust's `/` truncates toward zero and tile coords go negative.
        let tile = |v: f32| (v.floor() as i32).div_euclid(64);
        let (tx1, tx2) = (tile(x - r_fringe), tile(x + r_fringe));
        let (ty1, ty2) = (tile(y - r_fringe), tile(y + r_fringe));
        for ty in ty1..=ty2 {
            for tx in tx1..=tx2 {
                (*p).tiles.insert(TileIdx::new(tx, ty));
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mnc_record_dab_mode() -> c_int {
    RECORD_MODE.with(|c| c.get())
}

/// Override the per-dab tile budget (PATCHES.md #19). TEST seam only — the
/// shipping guard is the 1024 constant in the thread-local above; restore
/// it on exit (tests on one harness thread run sequentially, a leaked low
/// budget would clamp every later stroke's dabs).
#[cfg(test)]
pub fn set_dab_tile_budget(n: u32) {
    DAB_TILE_BUDGET.with(|c| c.set(n));
}

/// Called from the patched `mypaint-tiled-surface.c` per dab (PATCHES.md
/// #19): the active per-dab tile budget, 0 = unlimited.
#[unsafe(no_mangle)]
pub extern "C" fn mnc_dab_tile_budget() -> c_int {
    DAB_TILE_BUDGET.with(|c| c.get() as c_int)
}

/// Called from the patched `mypaint-tiled-surface.c` when a dab's radius
/// was shrunk to fit the budget (PATCHES.md #19).
#[unsafe(no_mangle)]
pub extern "C" fn mnc_notify_dab_clamped() {
    DAB_CLAMP_COUNT.with(|c| c.set(c.get().saturating_add(1)));
}

fn set_radius_random_abs(on: bool) {
    let bits = if on {
        1.0f32.to_bits()
    } else {
        0.0f32.to_bits()
    };
    RADIUS_RANDOM_ABS_PX.with(|c| c.set(bits));
}

/// Point the vendored texture hooks at this brush's mask for the coming
/// `stroke_to`. `accum` is the brush's own crawl offset — the hook advances
/// it in place, per dab.
fn set_texture_hook(
    mask: Option<&Arc<TextureMask>>,
    accum: *mut (f32, f32),
    step_px: f32,
    anchor_dab: bool,
    rotate_direction: bool,
    angle_deg: f32,
) {
    TEXTURE_ANCHOR_DAB.with(|c| c.set(anchor_dab as i32));
    TEXTURE_STAMP_DIRECTION.with(|c| c.set(rotate_direction as i32));
    TEXTURE_STAMP_BASE.with(|c| c.set(angle_deg.to_bits()));
    // Publish the base so the stroke's FIRST dab (before any direction
    // exists) renders at it.
    TEXTURE_STAMP_ANGLE.with(|c| c.set(angle_deg.to_bits()));
    match mask {
        Some(m) => {
            TEXTURE_PTR.with(|c| c.set(m.data.as_ptr() as usize));
            TEXTURE_SIZE.with(|c| c.set(m.size as i32));
            TEXTURE_ACCUM.with(|c| c.set(accum as usize));
            TEXTURE_STEP.with(|c| c.set(step_px.to_bits()));
            // Publish the current offset so the very first dab sees it.
            unsafe {
                TEXTURE_SCROLL_X.with(|c| c.set((*accum).0.to_bits()));
                TEXTURE_SCROLL_Y.with(|c| c.set((*accum).1.to_bits()));
            }
        }
        None => {
            TEXTURE_SIZE.with(|c| c.set(0));
            TEXTURE_PTR.with(|c| c.set(usize::MAX));
        }
    }
}

/// Called from the patched `mypaint-brush.c` per dab.
#[unsafe(no_mangle)]
pub extern "C" fn mnc_brush_radius_random_abs_px() -> f32 {
    RADIUS_RANDOM_ABS_PX.with(|c| f32::from_bits(c.get()))
}

/// Called from the patched `mypaint-tiled-surface.c` per dab pixel.
#[unsafe(no_mangle)]
pub extern "C" fn mnc_brush_hard_dab() -> f32 {
    HARD_DAB.with(|c| f32::from_bits(c.get()))
}

/// Called from the patched `mypaint-brush.c` per dab.
#[unsafe(no_mangle)]
pub extern "C" fn mnc_brush_scatter() -> f32 {
    SCATTER.with(|c| f32::from_bits(c.get()))
}

/// Called from the patched `mypaint-tiled-surface.c` per dab pixel (patch
/// #10): the active texture's side length, 0 = texture off.
#[unsafe(no_mangle)]
pub extern "C" fn mnc_brush_texture_size() -> c_int {
    TEXTURE_SIZE.with(|c| c.get())
}

/// Called from the patched `mypaint-tiled-surface.c` per dab pixel (patch
/// #10 amendment 2): non-zero = the mask is dab-anchored (stamped tip).
#[unsafe(no_mangle)]
pub extern "C" fn mnc_brush_texture_anchor_dab() -> c_int {
    TEXTURE_ANCHOR_DAB.with(|c| c.get())
}

/// Called from the patched `mypaint-brush.c` once per dab (#10 amendment
/// 2), with the dab's UNFOLDED stroke direction in degrees: compute and
/// publish the stamp angle this dab renders with.
#[unsafe(no_mangle)]
pub extern "C" fn mnc_brush_texture_stamp(direction_deg: f32) {
    let base = f32::from_bits(TEXTURE_STAMP_BASE.with(|c| c.get()));
    let angle = if TEXTURE_STAMP_DIRECTION.with(|c| c.get()) != 0 && direction_deg.is_finite() {
        base + direction_deg
    } else {
        base
    };
    TEXTURE_STAMP_ANGLE.with(|c| c.set(angle.to_bits()));
}

/// The published stamp angle — snapshotted into the op (and the GPU
/// record) by `draw_dab_internal`, exactly like the crawl offset.
#[unsafe(no_mangle)]
pub extern "C" fn mnc_brush_texture_stamp_angle() -> f32 {
    f32::from_bits(TEXTURE_STAMP_ANGLE.with(|c| c.get()))
}

/// The grayscale mask, `size × size` bytes. Only read while a texture is
/// active (size > 0), and the owning brush re-points this per `stroke_to`.
#[unsafe(no_mangle)]
pub extern "C" fn mnc_brush_texture_data() -> *const u8 {
    let p = TEXTURE_PTR.with(|c| c.get());
    if p == usize::MAX {
        std::ptr::null()
    } else {
        p as *const u8
    }
}

/// The crawl offset the current stroke has reached, in mask px.
#[unsafe(no_mangle)]
pub extern "C" fn mnc_brush_texture_scroll(dx: *mut f32, dy: *mut f32) {
    unsafe {
        if !dx.is_null() {
            *dx = f32::from_bits(TEXTURE_SCROLL_X.with(|c| c.get()));
        }
        if !dy.is_null() {
            *dy = f32::from_bits(TEXTURE_SCROLL_Y.with(|c| c.get()));
        }
    }
}

/// Called from the patched `mypaint-brush.c` ONCE per dab (patch #10): step
/// the owning brush's crawl offset and publish it. Fixed diagonal direction
/// (1, 0.5) × step so the pattern drifts instead of wobble-marching.
/// M4: when a tip SET is armed, this also picks the dab's VARIANT —
/// seeded-random tip, mirrored or not — and (base-angle mode) re-publishes
/// the stamp angle with the variation's jitter. Still one call per dab, so
/// a multi-tile dab stays seamless.
#[unsafe(no_mangle)]
pub extern "C" fn mnc_brush_texture_advance() {
    let step = f32::from_bits(TEXTURE_STEP.with(|c| c.get()));
    let accum = TEXTURE_ACCUM.with(|c| c.get()) as *mut (f32, f32);
    if !accum.is_null() {
        unsafe {
            (*accum).0 += step;
            (*accum).1 += step * 0.5;
            TEXTURE_SCROLL_X.with(|c| c.set((*accum).0.to_bits()));
            TEXTURE_SCROLL_Y.with(|c| c.set((*accum).1.to_bits()));
        }
    }
    // M4 variant swap. Empty set (every stock preset) leaves the pointers
    // exactly as set_texture_hook left them — bit-identical path.
    let count = TIP_COUNT.with(|c| c.get());
    if count == 0 {
        return;
    }
    let r = tip_rng_next();
    let set = TIP_SET.with(|c| c.get()) as *const (*const u8, i32);
    let idx = ((r >> 33) % count as u64) as usize;
    unsafe {
        if !set.is_null() {
            let (p, s) = *set.add(idx);
            TEXTURE_PTR.with(|c| c.set(p as usize));
            TEXTURE_SIZE.with(|c| c.set(s));
        }
    }    // Angle jitter, base-angle mode only (see TIP_JITTER_OK's doc).
    if TIP_JITTER_OK.with(|c| c.get()) != 0 {
        let v = f32::from_bits(VARIATION.with(|c| c.get()));
        if v > 0.0 {
            // (r >> 2) low bits as a signed 0..1 range: the two draws are
            // independent enough at this sample size and cost nothing.
            let unit = ((r & 0xffff) as f32 / 65535.0) * 2.0 - 1.0;
            let base = f32::from_bits(TIP_ANGLE_BASE.with(|c| c.get()));
            TEXTURE_STAMP_ANGLE.with(|c| c.set((base + unit * v * 90.0).to_bits()));
        }
    }
}

/// One xorshift64 step of the variant rng (M4). A Cell, not a Rc<RefCell>:
/// the C calls this from the middle of its dab loop.
fn tip_rng_next() -> u64 {
    TIP_RNG.with(|c| {
        let mut x = c.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        c.set(x);
        x
    })
}

/// Arm the M4 tip set for the coming `stroke_to`. `set` is the brush's own
/// stable variant table (built at load, Boxed so its address never moves):
/// each tip plus mirrored copies when variation > 0 — mirroring is a MASK
/// swap (a precomputed flipped buffer), not a C sampler change, so CPU, GPU
/// record and repair raster all agree by construction.
fn set_tip_set_hook(
    set: *const (*const u8, i32),
    count: usize,
    variation: f32,
    base_angle: f32,
    jitter_ok: bool,
) {
    TIP_SET.with(|c| c.set(set as usize));
    TIP_COUNT.with(|c| c.set(count as u32));
    TIP_RNG.with(|c| c.set(0x9E3779B97F4A7C15));
    VARIATION.with(|c| c.set(variation.to_bits()));
    TIP_ANGLE_BASE.with(|c| c.set(base_angle.to_bits()));
    TIP_JITTER_OK.with(|c| c.set(jitter_ok as i32));
}

fn set_hard_dab_flag(on: bool) {
    let bits = if on {
        1.0f32.to_bits()
    } else {
        0.0f32.to_bits()
    };
    HARD_DAB.with(|c| c.set(bits));
}

fn set_scatter_flag(v: f32) {
    SCATTER.with(|c| c.set(v.clamp(0.0, 4.0).to_bits()));
}

/// Evaluate a libmypaint mapping the way `mypaint_mapping_calculate` does,
/// quirks included: it interpolates the segment containing `x` and extrapolates
/// along the outer segments, except that a flat segment returns its own `y`.
/// Reading a mapping back any other way would make round-trips drift.
fn eval_mapping(points: &[(f32, f32)], x: f32) -> f32 {
    match points {
        [] => 0.0,
        [(_, y)] => *y,
        [first, second, rest @ ..] => {
            let (mut x0, mut y0) = *first;
            let (mut x1, mut y1) = *second;
            for p in rest {
                if x <= x1 {
                    break;
                }
                (x0, y0) = (x1, y1);
                (x1, y1) = *p;
            }
            if x0 == x1 || y0 == y1 {
                y0
            } else {
                (y1 * (x - x0) + y0 * (x1 - x)) / (x1 - x0)
            }
        }
    }
}

/// Windows reports pen tilt in degrees (-90..90); libmypaint wants a normalised
/// component where `hypot(xtilt, ytilt) == 1.0` means 60 degrees off vertical
/// (see `tilt_declination = 90 - rad * 60` in `mypaint-brush.c`). It clamps to
/// -1..1 internally, so anything past 60 degrees saturates — as it does in
/// MyPaint itself.
#[inline]
fn norm_tilt(degrees: f32) -> f32 {
    if degrees.is_finite() {
        (degrees / 60.0).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// sRGB 0..1 -> HSV 0..1, matching libmypaint's `hsv_to_rgb_float` convention
/// (hue wraps at 1.0, not 360).
fn rgb_to_hsv(rgb: [f32; 3]) -> (f32, f32, f32) {
    let r = rgb[0].clamp(0.0, 1.0);
    let g = rgb[1].clamp(0.0, 1.0);
    let b = rgb[2].clamp(0.0, 1.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;

    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        (((g - b) / d) % 6.0 + 6.0) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;

    let s = if max <= 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

/// Discovery for the brush picker: every `.myb` under a directory tree.
pub struct BrushLibrary;

impl BrushLibrary {
    /// Recursively collect `(display_name, path)` for every `*.myb` under `dir`,
    /// sorted by display name. Unreadable subdirectories are skipped rather than
    /// failing the whole scan — a missing brush folder should not stop the app
    /// from starting.
    pub fn scan(dir: &Path) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        collect(dir, &mut out);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

/// What to call a preset in a picker, without handing it to libmypaint: its
/// `"name"` field, else the de-underscored file stem. The CSP-derived presets
/// live in ASCII-safe files (`ink-gire-fude-pen.myb`) and carry their real,
/// often Japanese, names inside — so the two must agree with `MyBrush::name`.
fn display_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().replace('_', " "))
        .unwrap_or_default();
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|j| {
            j.get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or(stem)
}

fn collect(dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("myb"))
        {
            let display = display_name(&path);
            out.push((display, path));
        }
    }
}

#[cfg(test)]
mod carryover_probe {
    use super::*;
    use mn_core::{Document, PenSample, StrokeSink, TILE_LEN, TileIdx};

    fn sample(x: f32, y: f32, t: f64) -> PenSample {
        PenSample {
            x,
            y,
            pressure: 0.8,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: t,
        }
    }

    fn stroke(b: &mut MyBrush, doc: &mut Document, x0: f32) {
        b.begin(doc);
        for i in 0..30 {
            b.sample(doc, sample(x0 + i as f32 * 4.0, 200.0, i as f64 * 8.0));
        }
        b.end(doc);
    }

    /// CARRYOVER RESOLVED (round 41, the raw-engine half of the proof):
    /// the IDENTICAL second stroke, run in TAP mode after a raster
    /// history and after a BYPASS history, produces field-identical dab
    /// streams — the engine state a BYPASS run leaves is the same a
    /// raster run leaves. The app-level half (CPU-after-BYPASS inks
    /// bit-identically to CPU-after-CPU) is mn-app's
    /// cpu_after_bypass_equals_cpu_after_cpu. Together they close Opus's
    /// round-35 escalation: the "47%" was same-engine stroke-index
    /// drift concentrated in fat-radius tail tiles, never engine damage.
    #[test]
    fn bypass_history_does_not_change_the_next_strokes_dabs() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/brushes/csp/real-g-pen.myb");
        let Ok(_) = MyBrush::load(&path) else {
            eprintln!("[probe] preset missing, skipping");
            return;
        };

        let run = |mode1: RecordMode| -> (Vec<DabParams>, Vec<DabParams>) {
            let mut b = MyBrush::load(&path).unwrap();
            let mut doc = Document::new(512, 512);
            // Stroke 1 under the chosen history — recorded in BOTH arms
            // (Off rasterizes without recording, so take a second engine
            // pair with Tap for the stroke-1 comparison).
            b.set_dab_recording(mode1);
            stroke(&mut b, &mut doc, 100.0);
            let stroke1 = b.take_dab_record().dabs;
            // Stroke 2: IDENTICAL input, TAP (records + rasterizes).
            b.set_dab_recording(RecordMode::Tap);
            stroke(&mut b, &mut doc, 100.0);
            (stroke1, b.take_dab_record().dabs)
        };

        let (raster_s1, raster_hist) = run(RecordMode::Tap);
        let (bypass_s1, bypass_hist) = run(RecordMode::Bypass);
        eprintln!(
            "[probe] stroke1 dabs: tap {} bypass {}",
            raster_s1.len(),
            bypass_s1.len()
        );
        let n1 = raster_s1.len().min(bypass_s1.len());
        for i in 0..n1 {
            let a = &raster_s1[i];
            let c = &bypass_s1[i];
            let fields = [
                ("x", a.x, c.x),
                ("y", a.y, c.y),
                ("radius", a.radius, c.radius),
                ("alpha", a.alpha, c.alpha),
                ("opaque", a.opaque, c.opaque),
            ];
            for (name, va, vc) in fields {
                if (va - vc).abs() > 1e-6 {
                    eprintln!(
                        "[probe] STROKE1 DIVERGENCE dab {i} field {name}: tap {va} bypass {vc}"
                    );
                    panic!("stroke-1 divergence: dab {i} {name} {va} vs {vc}");
                }
            }
        }
        eprintln!(
            "[probe] dabs: raster-hist {} bypass-hist {}",
            raster_hist.len(),
            bypass_hist.len()
        );
        let n = raster_hist.len().min(bypass_hist.len());
        for i in 0..n {
            let a = &raster_hist[i];
            let c = &bypass_hist[i];
            let fields = [
                ("x", a.x, c.x),
                ("y", a.y, c.y),
                ("radius", a.radius, c.radius),
                ("alpha", a.alpha, c.alpha),
                ("opaque", a.opaque, c.opaque),
                ("hardness", a.hardness, c.hardness),
                ("paint", a.paint, c.paint),
            ];
            for (name, va, vc) in fields {
                if (va - vc).abs() > 1e-6 {
                    eprintln!(
                        "[probe] FIRST DIVERGENCE dab {i} field {name}: raster-hist {va} bypass-hist {vc}"
                    );
                    panic!("carryover reproduced: dab {i} {name} {va} vs {vc}");
                }
            }
        }
        assert_eq!(raster_hist.len(), bypass_hist.len(), "dab counts differ");
        let _ = TILE_LEN;
        let _ = TileIdx::new(0, 0);
    }
}

#[cfg(test)]
mod wash_smudge_tests {
    use super::*;
    use mn_core::{Document, PenSample, StrokeSink, TILE_LEN, TileIdx};

    const FIX15_ONE: u16 = 32767;

    /// TODO #6, the smudge-under-wash read fix: a wash-mode smudge stroke
    /// must sample the ink the user SEES (buffer over layer), not the blank
    /// wash buffer. Before the fix, smearing over existing ink with
    /// wash+wash-smudge picked up NOTHING (the buffer starts blank).
    #[test]
    fn wash_smudge_samples_the_layer_under_the_buffer() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/brushes/classic/blending_knife.myb");
        let Ok(mut b) = MyBrush::load(&path) else {
            eprintln!("[test] preset missing");
            return;
        };
        // Wash mode on (the buffer path); flow high so dabs lay ink.
        b.set_wash(true, 1.0, Blend::Normal);

        // A document with a red blob on the active layer.
        let mut doc = Document::new(256, 256);
        for y in 100..140 {
            for x in 100..140 {
                doc.active_layer_mut()
                    .tile_mut(TileIdx::of_pixel(x, y))
                    .set_pixel(
                        (x & 63) as usize,
                        (y & 63) as usize,
                        [30000, 0, 0, FIX15_ONE],
                    );
            }
        }

        // A smudge stroke across the blob: the engine's get_color must
        // return the blob's RED (layer-under-buffer), not transparent.
        // The dab colors land in the record — assert some non-red-ink
        // picks up red-ish channels via the smudge bucket effect: the
        // simplest observable is that the committed buffer paints SOME
        // red-weighted ink NEAR the blob after crossing it (before the
        // fix the buffer stayed near-transparent → the commit inks far
        // less).
        b.set_dab_recording(RecordMode::Tap);
        b.begin(&mut doc);
        for i in 0..30 {
            b.sample(
                &mut doc,
                PenSample {
                    x: 60.0 + i as f32 * 6.0,
                    y: 120.0,
                    pressure: 0.9,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: i as f64 * 8.0,
                },
            );
        }
        b.end(&mut doc);

        // The committed ink must carry red picked up from the blob:
        // count red-dominant pixels on the whole layer.
        let mut red_dominant = 0usize;
        for (_, t) in doc.active_layer().tiles() {
            for px in t.data().chunks_exact(4) {
                if px[3] > 0 && px[0] as u32 > px[1] as u32 * 2 + 200 {
                    red_dominant += 1;
                }
            }
        }
        assert!(
            red_dominant > 50,
            "the smudge must pick up the layer's red under the wash buffer ({red_dominant} px)"
        );
        let _ = TILE_LEN;
    }

    /// PATCHES.md #19: a dab whose tile footprint exceeds the budget is
    /// clamped BEFORE the record tap — the GPU replay sees the same clamped
    /// radius — and the clamp counter moves. Runs with a lowered budget
    /// because the engine's own `ACTUAL_RADIUS_MAX` (1000 px radius) already
    /// caps what can arrive at ~32×32 tiles: the SHIPPING 1024 budget is a
    /// rail for the 2b unit fix (which will raise that ceiling), inert until
    /// then — pinned inert by `normal_brush_strokes_record_zero_clamps`.
    #[test]
    fn giant_dab_radius_is_clamped_to_the_tile_budget() {
        struct RestoreBudget;
        impl Drop for RestoreBudget {
            fn drop(&mut self) {
                set_dab_tile_budget(1024);
            }
        }
        set_dab_tile_budget(16);
        let _restore = RestoreBudget;

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/brushes/csp/real-g-pen.myb");
        let Ok(mut b) = MyBrush::load(&path) else {
            eprintln!("[probe] preset missing, skipping");
            return;
        };
        b.set_size_px(5000.0); // engine caps the arrival radius at 1000
        // BYPASS: record only, skip the tile queue — the clamp is asserted
        // from the record, so the test pays nothing for the giant dabs.
        b.set_dab_recording(RecordMode::Bypass);
        let mut doc = Document::new(512, 512);
        b.begin(&mut doc);
        // At ~1000 px radius the engine spaces dabs ~500 px apart — a long
        // travel is what a real giant-size stroke looks like.
        for i in 0..40 {
            b.sample(
                &mut doc,
                PenSample {
                    x: -600.0 + i as f32 * 40.0,
                    y: 200.0,
                    pressure: 0.8,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: i as f64 * 16.0,
                },
            );
        }
        b.end(&mut doc);
        assert!(
            MyBrush::take_dab_clamp_count() >= 1,
            "the over-budget dabs must count as clamped"
        );
        // 16 tiles = a 4-tile square = 256 px across → radius ≤ 129
        // (+1 fringe, floor alignment).
        let rec = b.take_dab_record();
        assert!(!rec.dabs.is_empty(), "the stroke must record its dabs");
        for d in &rec.dabs {
            assert!(
                d.radius <= 4.0 * 64.0 / 2.0 + 1.0,
                "recorded radius {} escaped the budget clamp",
                d.radius
            );
        }
    }

    /// PATCHES.md #19 negative control: a normal 300 px brush (the `.abr`
    /// import cap, above most hand-authored sizes) never trips the guard —
    /// stock presets render bit-identically and the app warning cannot
    /// fire for them.
    #[test]
    fn normal_brush_strokes_record_zero_clamps() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/brushes/csp/real-g-pen.myb");
        let Ok(mut b) = MyBrush::load(&path) else {
            eprintln!("[probe] preset missing, skipping");
            return;
        };
        b.set_size_px(300.0);
        b.set_dab_recording(RecordMode::Tap);
        let mut doc = Document::new(512, 512);
        b.begin(&mut doc);
        for i in 0..30 {
            b.sample(
                &mut doc,
                PenSample {
                    x: 100.0 + i as f32 * 4.0,
                    y: 200.0,
                    pressure: 0.8,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: i as f64 * 8.0,
                },
            );
        }
        b.end(&mut doc);
        assert_eq!(
            MyBrush::take_dab_clamp_count(),
            0,
            "a 300 px brush must not trip the tile budget"
        );
        let rec = b.take_dab_record();
        assert!(!rec.dabs.is_empty());
        for d in &rec.dabs {
            assert!(
                d.radius <= 151.0,
                "radius {} exceeds the authored 150 px — clamp touched a stock brush",
                d.radius
            );
        }
    }

    /// PATCHES.md #19: the RASTER path clamps too, not just the record —
    /// with a tiny test budget, a stationary dab inks at most the budget's
    /// tile footprint. This is the O(r²)-stall guard itself.
    #[test]
    fn raster_dabs_are_clamped_by_the_tile_budget() {
        // Restore on unwind AND on assert-failure — a leaked 4-tile budget
        // would clamp every later stroke on this test thread.
        struct RestoreBudget;
        impl Drop for RestoreBudget {
            fn drop(&mut self) {
                set_dab_tile_budget(1024);
            }
        }
        set_dab_tile_budget(4);
        let _restore = RestoreBudget;

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/brushes/csp/real-g-pen.myb");
        let Ok(mut b) = MyBrush::load(&path) else {
            eprintln!("[probe] preset missing, skipping");
            return;
        };
        b.set_size_px(400.0); // radius ~200 → ~25-36 tiles/dab, over 4
        let mut doc = Document::new(512, 512);
        b.begin(&mut doc);
        for i in 0..30 {
            b.sample(
                &mut doc,
                PenSample {
                    x: 200.0 + i as f32 * 4.0,
                    y: 200.0,
                    pressure: 0.8,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: i as f64 * 8.0,
                },
            );
        }
        b.end(&mut doc);
        assert!(
            MyBrush::take_dab_clamp_count() >= 1,
            "the over-budget dabs must count as clamped under a 4-tile budget"
        );
        // Clamped to a ≤2×2-tile footprint per dab: the whole 120 px path
        // inks at most a 3×3 tile neighbourhood (unclamped ~200 px dabs
        // would touch ~4×4).
        let tiles = doc.active_layer().tiles().count();
        assert!(tiles <= 9, "{tiles} tiles inked — dab escaped the budget");
    }

    /// M4: the per-dab variant swap. Two tips armed → successive advance
    /// calls publish DIFFERENT active masks (the seeded rng visits both);
    /// no set armed → the pointers stand still, bit-identical to stock.
    #[test]
    fn tip_sets_swap_the_active_mask_per_dab() {
        use std::sync::Arc;
        let mk = |v: u8| Arc::new(TextureMask {
            name: format!("t{v}"),
            size: 4,
            data: Arc::new(vec![v; 16]),
        });
        let a = mk(10);
        let b = mk(200);
        let variants: Box<[(*const u8, i32)]> =
            vec![(a.data.as_ptr(), 4), (b.data.as_ptr(), 4)].into();
        set_tip_set_hook(variants.as_ptr(), variants.len(), 0.0, 0.0, false);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            mnc_brush_texture_advance();
            unsafe {
                seen.insert(*mnc_brush_texture_data());
            }
        }
        assert!(seen.len() >= 2, "both tips took their turn: {seen:?}");
        // Disarm: stock presets' path — the pointer must not move.
        set_tip_set_hook(std::ptr::null(), 0, 0.0, 0.0, false);
        let before = mnc_brush_texture_data();
        for _ in 0..8 {
            mnc_brush_texture_advance();
        }
        assert_eq!(mnc_brush_texture_data(), before, "no set, no swap");
    }

    /// M4: seeded stability — identical strokes through a tip-list brush
    /// stamp IDENTICALLY (the variant rng is fixed-seed per series;
    /// variation is between dabs, never between strokes).
    #[test]
    fn tip_list_strokes_are_seeded_bit_stable() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/brushes/csp/real-g-pen.myb");
        let Ok(_) = MyBrush::load(&path) else {
            eprintln!("[probe] preset missing, skipping");
            return;
        };
        // Build a preset in-memory is not the API; exercise via two loads of
        // the same list brush written to a temp root instead.
        let dir = std::env::temp_dir().join(format!("mn-tips-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let tex_dir = dir.join("textures");
        std::fs::create_dir_all(&tex_dir).unwrap();
        for (k, v) in [(0u8, 40u8), (1, 220)] {
            image::GrayImage::from_pixel(8, 8, image::Luma([v]))
                .save(tex_dir.join(format!("tip{k}.png")))
                .unwrap();
        }
        let base = MyBrush::load(&path).unwrap();
        let (settings, _tex) = (serde_json::to_value(&"").unwrap(), ());
        let _ = (settings, base);
        // Hand-write the preset: real-g-pen's settings + the list keys.
        let src = std::fs::read_to_string(&path).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&src).unwrap();
        v["mn-texture-anchor"] = serde_json::json!("dab");
        v["mn-texture-list"] = serde_json::json!(["tip0", "tip1"]);
        v["mn-variation"] = serde_json::json!(0.4);
        let preset = dir.join("mine").join("var.myb");
        std::fs::create_dir_all(dir.join("mine")).unwrap();
        std::fs::write(&preset, serde_json::to_string_pretty(&v).unwrap()).unwrap();

        let ink_of = || {
            let mut b = MyBrush::load(&preset).unwrap();
            // TAP: record AND rasterize — the hash covers real ink, and the
            // record pins the variant stream (mask/angle per dab).
            b.set_dab_recording(RecordMode::Tap);
            let mut doc = Document::new(512, 512);
            b.begin(&mut doc);
            for i in 0..40 {
                b.sample(
                    &mut doc,
                    PenSample {
                        x: 60.0 + i as f32 * 6.0,
                        y: 200.0,
                        pressure: 0.8,
                        tilt_x: 0.0,
                        tilt_y: 0.0,
                        t_ms: i as f64 * 8.0,
                    },
                );
            }
            b.end(&mut doc);
            let rec = b.take_dab_record();
            let mut racc = 0u64;
            for d in &rec.dabs {
                racc = racc
                    .wrapping_mul(31)
                    .wrapping_add((d.radius as u64) ^ ((d.tex_angle as i64 as u64) << 32));
            }
            let mut acc = 0u64;
            for (_, t) in doc.active_layer().tiles() {
                // Sum, not a position-dependent chain: tile iteration order
                // is not stable across runs, and order must not matter here.
                acc = acc.wrapping_add(t.alpha_sum() as u64);
            }
            (acc, racc)
        };
        let (a, ra) = ink_of();
        let (b, rb) = ink_of();
        assert_eq!(ra, rb, "the variant stream itself is seeded-stable");
        assert_eq!(a, b, "identical strokes, identical ink (seeded rng)");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
