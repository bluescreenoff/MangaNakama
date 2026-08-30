//! MangaNakama GPU compositor (wgpu).
//!
//! # Tile upload strategy — **per-tile textures, not an atlas**
//!
//! Every populated `core::Tile` gets its own 64x64 `Rgba16Uint` texture plus a
//! bind group, keyed by `(layer_index, TileIdx)` and versioned by the tile's
//! `revision`. Uploads happen only when `tile.revision() > cached.revision`.
//!
//! Why not an atlas: an atlas buys fewer bind-group switches, but costs a
//! residency/eviction allocator, and every partial upload becomes a sub-rect
//! write with the 256-byte `bytes_per_row` alignment rule applied to offsets
//! inside the atlas rather than to a whole texture. A standalone 64x64 RGBA-u16
//! tile is 512 bytes per row — already aligned — so `write_texture` of a whole
//! tile is the simplest correct thing. At B4/600dpi the sparse tile count is in
//! the low thousands, which is fine for one draw call each. If profiling ever
//! says otherwise, the swap is local to this file — nothing outside knows how
//! tiles reach the GPU.
//!
//! # Two passes
//!
//! 1. **canvas pass** — the layer stack composited into a document-sized
//!    `Rgba8Unorm` texture, cleared to paper white. Incremental at tile
//!    granularity: a region whose tile changed is reset to paper and then
//!    rebuilt through *every* visible layer, bottom to top, each with its own
//!    blend state and opacity. See `update_canvas`.
//! 2. **present pass** — that texture drawn to the swapchain through the
//!    `Viewport` (pan/zoom/rotate), filtered, on a neutral grey backdrop.
//!
//! # Blend contract
//!
//! The three `Blend` modes are fixed-function blend states (built in
//! `assemble`, formulas in the comments there) and are mirrored exactly by
//! `mn_core::blend` on the CPU. `tests/composite.rs` renders synthetic documents
//! both ways and asserts they agree — if you change one side, change both.
//!
//! Known gaps: no tile atlas (see above), the eviction scan is O(cached tiles)
//! per frame, and layer opacity is applied per-fragment rather than with a blend
//! constant (exact, but it means one instance per layer per damaged tile).

use std::borrow::Cow;
use std::collections::HashMap;

use mn_core::{Document, Paper, TILE_SIZE, TileIdx};

mod dabs;
pub use dabs::{WASH_LAYER_KEY, dab_tiles};

mod kernel;
pub use kernel::{KERNEL_FLOOR_PX, Kernel, TileJob};

/// Canvas texture format. Non-sRGB so `render_offscreen` readback is exact.
const CANVAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Tile texture format: the fix15 data verbatim, scaled in the shader.
const TILE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Uint;
/// Isolation-buffer format (folder groups, clip scratch).
///
/// `Rgba8Unorm`, the same as the canvas, ON PURPOSE: `Rgba16Float` render
/// targets that are later sampled come back with stippled holes on this
/// laptop's 2020 Intel UHD 620 DX12 driver (reproduced 2026-08-14 — dashed
/// dropouts near tile seams, never on WARP, regardless of pass/submit
/// structure). The canvas format has been rendered-and-sampled every frame
/// since round 1 without a glitch. Cost: one extra 8-bit quantisation in
/// group intermediates — display is allowed to approximate
/// (docs/ARCHITECTURE.md); export composites exactly on the CPU.
const GROUP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Max recycled tile textures kept alive (32 KiB each — see `tile_pool`).
const TILE_POOL_CAP: usize = 2048;
/// One tile's footprint in a linear upload buffer.
///
/// 64 px * 4 ch * 2 bytes = 512 bytes per row, already a multiple of wgpu's
/// 256-byte `COPY_BYTES_PER_ROW_ALIGNMENT`, so a whole tile is one legal
/// copy with no per-row padding; 64 rows of that is 32 KiB. Being a multiple
/// of 256 also makes every batch slot a legal `copy_buffer_to_texture`
/// offset.
const TILE_UPLOAD_BYTES: usize = TILE_SIZE * TILE_SIZE * 4 * 2;
/// Tiles per staging buffer in [`flush_tile_uploads`].
///
/// Opening a page uploads ~1000 tiles; one buffer for all of them would be a
/// 30 MB transient allocation, so they go in batches. 256 tiles = 8 MiB,
/// which turns the worst frame's thousand driver allocations into four
/// without holding a page's pixels twice.
const UPLOAD_BATCH: usize = 256;
const TRANSPARENT: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};
/// Backdrop outside the page: darker than every UI surface (ui/theme.rs
/// WINDOW is 0x1f), so the artwork is the brightest thing on screen.
const BACKDROP: wgpu::Color = wgpu::Color {
    r: 0.086,
    g: 0.086,
    b: 0.094,
    a: 1.0,
};
/// PA-001: the clear colour under the stack, premultiplied. A hidden paper
/// clears to nothing at all — the canvas texture then carries real alpha and
/// the present pass shows the transparency checker through it.
fn paper_clear(paper: Paper) -> wgpu::Color {
    if !paper.visible {
        return TRANSPARENT;
    }
    let [r, g, b] = paper.colour;
    wgpu::Color {
        r: r as f64 / 255.0,
        g: g as f64 / 255.0,
        b: b as f64 / 255.0,
        a: 1.0,
    }
}

/// Canvas placement on screen.
///
/// The transform, canvas pixels -> screen (client) pixels, is
///
/// ```text
/// screen = pan + R(rotate_rad) * (canvas * zoom)
///
///          | cos -sin |
/// R(t)  =  | sin  cos |     (y-down screen space, so positive = clockwise)
/// ```
///
/// i.e. scale, then rotate about the canvas origin, then translate. `pan` is
/// therefore the screen position of the canvas's top-left corner *whatever the
/// rotation is*, which keeps the un-rotated behaviour (and every existing
/// caller) exactly as it was.
///
/// Rotating "around the view centre" is not a different transform, it is a
/// `pan` correction — use [`Viewport::rotate_around`] / [`Viewport::zoom_around`],
/// which keep the canvas point under a given screen point pinned.
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    /// Screen-pixel position of the canvas top-left corner.
    pub pan: [f32; 2],
    pub zoom: f32,
    pub rotate_rad: f32,
    /// View mirror (CSP's flip-view drawing check). Canvas x negates before
    /// the rotate+pan, so `pan` becomes the screen position of the top-RIGHT
    /// corner while flipped — callers never notice, they go through
    /// to_screen/to_canvas.
    pub flip_h: bool,
    /// The vertical half of the same check: canvas y negates before the
    /// rotate+pan. Both flips at once is a 180° point reflection, not a
    /// mirror — see [`Self::brush_view`], which is what the brush engine
    /// must be told.
    pub flip_v: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            pan: [0.0, 0.0],
            zoom: 1.0,
            rotate_rad: 0.0,
            flip_h: false,
            flip_v: false,
        }
    }
}

impl Viewport {
    /// Zoom guarded against divide-by-zero.
    #[inline]
    fn safe_zoom(&self) -> f32 {
        if self.zoom.abs() < 1e-6 {
            1e-6
        } else {
            self.zoom
        }
    }

    /// Screen (client) pixels -> canvas pixels. Inverse of [`Self::to_screen`].
    #[inline]
    pub fn to_canvas(&self, sx: f32, sy: f32) -> (f32, f32) {
        let z = self.safe_zoom();
        let (dx, dy) = (sx - self.pan[0], sy - self.pan[1]);
        let (cx, cy) = if self.rotate_rad == 0.0 {
            (dx / z, dy / z)
        } else {
            // R(-t) * d / zoom
            let (s, c) = self.rotate_rad.sin_cos();
            ((c * dx + s * dy) / z, (-s * dx + c * dy) / z)
        };
        (
            if self.flip_h { -cx } else { cx },
            if self.flip_v { -cy } else { cy },
        )
    }

    /// Canvas pixels -> screen (client) pixels.
    #[inline]
    pub fn to_screen(&self, cx: f32, cy: f32) -> (f32, f32) {
        let cx = if self.flip_h { -cx } else { cx };
        let cy = if self.flip_v { -cy } else { cy };
        let (x, y) = (cx * self.zoom, cy * self.zoom);
        if self.rotate_rad == 0.0 {
            return (x + self.pan[0], y + self.pan[1]);
        }
        let (s, c) = self.rotate_rad.sin_cos();
        (c * x - s * y + self.pan[0], s * x + c * y + self.pan[1])
    }

    /// Toggle the horizontal view mirror, keeping the canvas point under
    /// `screen` pinned and mirroring the view rotation so the page flips in
    /// place instead of swinging.
    pub fn flip_around(&mut self, screen: [f32; 2]) {
        self.flip_toggle_around(screen, true);
    }

    /// The vertical half: the same in-place toggle about a horizontal axis
    /// through `screen`.
    pub fn flip_v_around(&mut self, screen: [f32; 2]) {
        self.flip_toggle_around(screen, false);
    }

    /// Either flip. Mirroring the SCREEN image about an axis through the
    /// anchor is `M·R(t)·S·F` = `R(-t)·S·(M·F)`, because a mirror conjugates
    /// a rotation into its inverse — so both flips toggle their own field and
    /// negate the rotation, whatever the other flip is doing.
    fn flip_toggle_around(&mut self, screen: [f32; 2], horizontal: bool) {
        let anchor = self.to_canvas(screen[0], screen[1]);
        if horizontal {
            self.flip_h = !self.flip_h;
        } else {
            self.flip_v = !self.flip_v;
        }
        self.rotate_rad = wrap_angle(-self.rotate_rad);
        self.repin(anchor, screen);
    }

    /// Is the view MIRRORED (handedness reversed)? Both flips at once is a
    /// 180° rotation, which is not a mirror — the thing every "are we
    /// flipped?" consumer actually means.
    #[inline]
    pub fn mirrored(&self) -> bool {
        self.flip_h != self.flip_v
    }

    /// `(rotation_rad, mirrored)` for the brush engine's view compensation
    /// (vendor patch #12, which knows only a HORIZONTAL flip).
    ///
    /// The engine sees the linear part `R(t)·S·F` only, so any `(t', flip')`
    /// with the same linear map behaves identically. A vertical flip is a
    /// horizontal one turned half a circle — `diag(1,-1) = R(pi)·diag(-1,1)`
    /// — so `flip_v` costs `+pi` of rotation and flips the mirror bit, and
    /// H+V lands on a plain `t+pi` with no mirror at all. Skip this and a
    /// vertically flipped view feeds the C mirrored motion directions,
    /// which the direction-mapped dynamics render as subtly wrong dabs.
    #[inline]
    pub fn brush_view(&self) -> (f32, bool) {
        let rot = if self.flip_v {
            wrap_angle(self.rotate_rad + std::f32::consts::PI)
        } else {
            self.rotate_rad
        };
        (rot, self.mirrored())
    }

    /// Multiply the zoom, keeping the canvas point currently under `screen`
    /// pinned there. Pass the client-area centre for "zoom around the view
    /// centre", or the cursor for "zoom at the pointer".
    pub fn zoom_around(&mut self, screen: [f32; 2], factor: f32) {
        let anchor = self.to_canvas(screen[0], screen[1]);
        self.zoom = (self.zoom * factor).clamp(0.01, 64.0);
        self.repin(anchor, screen);
    }

    /// Set the zoom outright, keeping the canvas point under `screen` pinned.
    pub fn set_zoom_around(&mut self, screen: [f32; 2], zoom: f32) {
        let anchor = self.to_canvas(screen[0], screen[1]);
        self.zoom = zoom.clamp(0.01, 64.0);
        self.repin(anchor, screen);
    }

    /// Rotate by `delta_rad`, keeping the canvas point currently under `screen`
    /// pinned there.
    pub fn rotate_around(&mut self, screen: [f32; 2], delta_rad: f32) {
        let anchor = self.to_canvas(screen[0], screen[1]);
        self.rotate_rad = wrap_angle(self.rotate_rad + delta_rad);
        self.repin(anchor, screen);
    }

    /// Set an absolute rotation, keeping the canvas point under `screen` pinned.
    pub fn set_rotation_around(&mut self, screen: [f32; 2], rad: f32) {
        let anchor = self.to_canvas(screen[0], screen[1]);
        self.rotate_rad = wrap_angle(rad);
        self.repin(anchor, screen);
    }

    /// Move `pan` so that canvas point `anchor` lands on screen point `screen`.
    fn repin(&mut self, anchor: (f32, f32), screen: [f32; 2]) {
        let now = self.to_screen(anchor.0, anchor.1);
        self.pan[0] += screen[0] - now.0;
        self.pan[1] += screen[1] - now.1;
    }

    /// Centred fit of `doc_size` inside `surface`, no rotation. What the app
    /// wants on startup and on "fit to window".
    pub fn fit(doc_size: (u32, u32), surface: (u32, u32)) -> Self {
        let zoom = ((surface.0 as f32 / doc_size.0.max(1) as f32)
            .min(surface.1 as f32 / doc_size.1.max(1) as f32))
        .max(0.01);
        Self {
            pan: [
                (surface.0 as f32 - doc_size.0 as f32 * zoom) * 0.5,
                (surface.1 as f32 - doc_size.1 as f32 * zoom) * 0.5,
            ],
            zoom,
            rotate_rad: 0.0,
            flip_h: false,
            flip_v: false,
        }
    }

    /// The four canvas corners in screen pixels, in triangle-strip order:
    /// top-left, top-right, bottom-left, bottom-right.
    pub fn corners_screen(&self, doc_size: (u32, u32)) -> [[f32; 2]; 4] {
        let (w, h) = (doc_size.0 as f32, doc_size.1 as f32);
        let mut out = [[0.0f32; 2]; 4];
        for (i, (cx, cy)) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
            .into_iter()
            .enumerate()
        {
            let (x, y) = self.to_screen(cx, cy);
            out[i] = [x, y];
        }
        out
    }
}

/// Keep an angle in (-pi, pi] so it never drifts into float mush after a
/// thousand wheel clicks.
fn wrap_angle(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut a = a % TAU;
    if a > PI {
        a -= TAU;
    } else if a <= -PI {
        a += TAU;
    }
    a
}

/// Startup knobs, driven by the app's CLI flags.
#[derive(Clone, Copy, Debug, Default)]
pub struct GpuConfig {
    /// `--warp`: skip the hardware attempt, go straight to the software adapter.
    pub force_fallback: bool,
    /// `--novsync`: `AutoNoVsync` instead of `AutoVsync`.
    pub no_vsync: bool,
}

#[derive(Debug)]
pub struct GpuError(pub String);

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "gpu: {}", self.0)
    }
}
impl std::error::Error for GpuError {}

/// LP-016: the no-tint sentinel for QuadInstance.tint.
pub const TINT_NONE: u32 = 0xFFFF_FFFF;

/// Pack an RGB layer colour into the instance slot (0x00RRGGBB).
pub fn tint_pack(rgb: [u8; 3]) -> u32 {
    (rgb[0] as u32) << 16 | (rgb[1] as u32) << 8 | rgb[2] as u32
}

/// The neutral value of `QuadInstance.fx`: white sub colour, no reduce —
/// what every draw that is not a plain layer's own tile passes.
pub const FX_NONE: u32 = 0x00FF_FFFF;

/// Pack the two *other* per-layer display effects into one instance word:
/// bits 0..24 the LP-017 SUB colour (white when unset, so [`FX_NONE`] is
/// literally "no sub colour"), bits 24.. the LP-022 expression reduce.
///
/// One word rather than two attributes because both shaders that read it
/// have to unpack it, and the pair is always set and cleared together.
pub fn fx_pack(sub: Option<[u8; 3]>, expr: mn_core::LayerExpression) -> u32 {
    let rgb = sub.map_or(0x00FF_FFFF, tint_pack);
    expr.sig() << 24 | rgb
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadInstance {
    /// x, y, w, h in canvas pixels
    rect: [f32; 4],
    /// 0 = opaque paper white, 1 = sample the bound tile texture
    mode: u32,
    /// Layer opacity, folded into the source in the fragment shader (all four
    /// premultiplied channels) — the CPU compositor does the same thing.
    opacity: f32,
    /// blend2 mode sentinel (blend_slot >= 16) — 0 for every
    /// fixed-function draw (blend2.wgsl location 3).
    blend_mode: u32,
    /// LP-016 layer colour, packed 0x00RRGGBB. 0xFFFFFFFF = no tint (the
    /// sentinel every non-tinted draw passes — blits must NOT re-tint
    /// pre-tinted group content; tiles.wgsl location 4).
    tint: u32,
    /// LP-017 sub colour + LP-022 expression reduce, packed by [`fx_pack`]
    /// (tiles.wgsl / blend2.wgsl location 5). [`FX_NONE`] on every draw that
    /// is not a plain layer's own tile, for the same reason as `tint`.
    fx: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CanvasUniform {
    size: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PresentUniform {
    /// Canvas corners in NDC: top-left, top-right (xy, zw).
    c01: [f32; 4],
    /// bottom-left, bottom-right (xy, zw).
    c23: [f32; 4],
    /// `[0]` = 1 when the target is an sRGB format and the shader must encode.
    flags: [u32; 4],
}

/// Which of the blend pipelines a draw needs.
///
/// `Darken`/`Lighten` have NO fixed-function state: wgpu validates that
/// min/max blend operations take factor `one` only, which kills the
/// `(1 - s.a)` destination fold. They composite through the blend2 shader
/// pass with the rest of part 2 and part 3.
/// Blend slots 0..=4 index the fixed-function pipeline arrays. Every other
/// mode goes through the blend2 SHADER pass instead — they still get
/// DISTINCT sentinel slots so `LayerSig` sees a blend change as a
/// presentation change (a collapsed slot would silently skip the rebuild).
/// The values ride the instance pad into blend2.wgsl — keep both sides in
/// lockstep.
const BLEND2_BASE: usize = 16;
/// **The order IS the wire format.** Each mode's slot is `16 + its index
/// here`, and that number rides the instance pad into `blend2.wgsl`, which
/// branches on the literal values. Append only; reordering this array
/// silently repaints every part-2/3 layer in the wrong mode.
///
/// Grouped the way the shader branches: separable operators first (slots
/// 16..22 and 26..34), then the nonseparable ones (23..25 and 35..37).
/// Slot 38 is Subtract, appended when it left the fixed-function
/// ReverseSubtract state (slot 4, retired): the premultiplied form painted
/// silent black over a transparent destination, the general frame keeps the
/// source there — see core::blend's Subtract arm.
const BLEND2_MODES: [mn_core::Blend; 23] = [
    mn_core::Blend::Darken,
    mn_core::Blend::Lighten,
    mn_core::Blend::Overlay,
    mn_core::Blend::SoftLight,
    mn_core::Blend::HardLight,
    mn_core::Blend::Difference,
    mn_core::Blend::Exclusion,
    mn_core::Blend::Hue,
    mn_core::Blend::Saturation,
    mn_core::Blend::Color,
    // Part 3 (CSP BM-004..028), separable:
    mn_core::Blend::ColorBurn,
    mn_core::Blend::LinearBurn,
    mn_core::Blend::ColorDodge,
    mn_core::Blend::GlowDodge,
    mn_core::Blend::VividLight,
    mn_core::Blend::LinearLight,
    mn_core::Blend::PinLight,
    mn_core::Blend::HardMix,
    mn_core::Blend::Divide,
    // Part 3, nonseparable:
    mn_core::Blend::DarkerColor,
    mn_core::Blend::LighterColor,
    mn_core::Blend::Luminosity,
    // Appended (slot 38): Subtract through the general separable frame.
    mn_core::Blend::Subtract,
];

fn blend_slot(b: mn_core::Blend) -> usize {
    match b {
        mn_core::Blend::Normal => 0,
        mn_core::Blend::Multiply => 1,
        mn_core::Blend::Screen => 2,
        mn_core::Blend::Add => 3,
        // Slot 4 (fixed-function ReverseSubtract) is RETIRED — Subtract now
        // rides blend2 via the array above. The slot-4 pipeline entries stay
        // so the array indexing never shifts; nothing selects them.
        _ => {
            let i = BLEND2_MODES.iter().position(|&m| m == b).unwrap_or(0);
            BLEND2_BASE + i
        }
    }
}

/// Per-layer presentation state the tile-revision path cannot see. When this
/// changes, the canvas is rebuilt from scratch — no call to `invalidate()`
/// needed for opacity/blend/visibility/reorder.
///
/// Tile COUNTS are deliberately absent: a stroke painting into an empty tile
/// grows a layer's count, and treating that as a presentation change forced a
/// full-canvas composite at nearly every stroke start (the round-17 pen-lag
/// fix). New tiles reach the compositor through the damaged set — they upload
/// and recomposite their own region; vanished tiles still force a full
/// rebuild through the eviction path below.
#[derive(Clone, Copy, PartialEq)]
struct LayerSig {
    /// Effective (ancestor-folders folded in) visibility.
    visible: bool,
    /// The layer's OWN opacity (folder opacity applies at its group blit).
    /// f32 bits: exact comparison, no epsilon games.
    opacity: u32,
    blend: usize,
    depth: u8,
    folder: bool,
    clip: bool,
    /// Screentone params, bit-exact — a param change re-derives the raster,
    /// so it must force a rebuild. **This is `ToneParams::sig()` and nothing
    /// else on purpose:** the old spelling listed three fields by hand, so
    /// every new tone field silently kept stale tiles on the canvas until
    /// someone remembered to widen the tuple. The signature now lives beside
    /// the params it covers (`core::tone`, guarded by `sig_covers_every_field`).
    tone: Option<[u32; 8]>,
    /// Border-effect params (`EdgeParams::sig`). Turning the effect OFF is
    /// the dangerous direction: the derived tiles simply stop being the
    /// display set and the painted tiles under them carry OLD revisions, so
    /// nothing is damaged and nothing re-uploads.
    edge: Option<[u32; 2]>,
    /// The exact words the shader reads for LP-016/017/022. These reach the
    /// fragment stage and touch no tile revision at all, so without them a
    /// layer colour could be switched on and the canvas would keep drawing
    /// the untinted tiles it already had. (LP-016's word was missing here
    /// until this round — found while adding the other two.)
    tint: u32,
    fx: u32,
    /// FB-overflow, both parts: the escape flag, the mask cap, and the
    /// draws-over set hashed into one word. Every one of these moves the
    /// layer to a different SEAT in `composite_order` (or splits it in two)
    /// without touching a single tile revision, so a canvas that did not
    /// watch them here would keep showing the old paint order until
    /// something else forced a rebuild.
    spill: u64,
}

/// Which raster of a layer a cached tile texture holds.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum TileVariant {
    /// The layer's display pixels, with an enabled layer mask already folded
    /// in (see the upload loop).
    Pixels,
    /// A frame folder's derived panel-coverage mask.
    Coverage,
    /// FB-overflow mask cap: the display pixels times ONE MINUS the layer
    /// mask — the half a breakout layer holds INSIDE the panel
    /// (`SpillPart::In`). Uploaded only for the layers `composite_order`
    /// splits in two, and only for tiles the mask actually covers (an absent
    /// mask tile holds nothing back).
    HeldIn,
}

/// Key of a cached tile texture: layer index, tile index, and which of the
/// layer's rasters it is.
type TileKey = (usize, TileIdx, TileVariant);

struct CachedTile {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    revision: u64,
}

/// A fresh tile texture + its compositor bind group. The usage flags cover
/// both jobs the texture can hold: sampled by the compositor (TEXTURE_
/// BINDING), uploaded from CPU tiles (COPY_DST), rasterized by the GPU-dab
/// compute path (STORAGE_BINDING) and read back at stroke end (COPY_SRC).
fn make_tile_texture(device: &wgpu::Device, bgl: &wgpu::BindGroupLayout) -> CachedTile {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mn.tile"),
        size: wgpu::Extent3d {
            width: TILE_SIZE as u32,
            height: TILE_SIZE as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TILE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mn.tile.bg"),
        layout: bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    });
    CachedTile {
        texture,
        bind_group,
        revision: 0,
    }
}

/// Push a batch of tile pixels to their textures through ONE staging buffer,
/// then clear the batch.
///
/// This replaces the obvious `queue.write_texture` per tile. That call is not
/// the thin memcpy it looks like: wgpu-core allocates AND maps a fresh
/// staging buffer inside every single one (`StagingBuffer::new`), so opening
/// a page paid ~1000 driver allocations plus ~1000 pending-write records.
/// One buffer of `n` tile slots plus `n` `copy_buffer_to_texture` commands
/// moves the same bytes with one allocation and one submit.
///
/// Submitted on its own encoder rather than folded into the composite
/// encoder on purpose: `update_canvas` can decide, after uploading, that it
/// has nothing to draw and return early (an invisible layer's tiles upload
/// but never become a region). Those uploads must still land — the cache
/// already recorded their revisions.
fn flush_tile_uploads(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    batch: &mut Vec<(wgpu::Texture, Cow<'_, [u16]>)>,
) {
    if batch.is_empty() {
        return;
    }
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mn.tile.staging"),
        size: (batch.len() * TILE_UPLOAD_BYTES) as u64,
        usage: wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: true,
    });
    {
        // Infallible in practice: the whole range of a buffer that was just
        // created `mapped_at_creation`, with no other view alive.
        let mut mapped = staging
            .slice(..)
            .get_mapped_range_mut()
            .expect("map a freshly created staging buffer");
        for (i, (_, data)) in batch.iter().enumerate() {
            let bytes: &[u8] = bytemuck::cast_slice(data);
            debug_assert_eq!(bytes.len(), TILE_UPLOAD_BYTES, "tile is not tile-sized");
            let off = i * TILE_UPLOAD_BYTES;
            // `.slice()`, not indexing: mapped memory can be write-combining,
            // so wgpu 30 only hands out a write-only view of it.
            mapped.slice(off..off + bytes.len()).copy_from_slice(bytes);
        }
    }
    staging.unmap();

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("mn.tile.upload"),
    });
    for (i, (texture, _)) in batch.iter().enumerate() {
        enc.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: (i * TILE_UPLOAD_BYTES) as u64,
                    bytes_per_row: Some((TILE_SIZE * 4 * 2) as u32),
                    rows_per_image: Some(TILE_SIZE as u32),
                },
            },
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: TILE_SIZE as u32,
                height: TILE_SIZE as u32,
                depth_or_array_layers: 1,
            },
        );
    }
    queue.submit([enc.finish()]);
    batch.clear();
}

/// One isolation buffer level (canvas-sized).
struct GroupTex {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Bind group for the group-blit pipelines (texture + sampler).
    blit_bg: wgpu::BindGroup,
}

struct Canvas {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    /// Level-0 view — the compositor's render target.
    view: wgpu::TextureView,
    size: (u32, u32),
    /// Bind group for the present pass (uniform + full-mip-chain view +
    /// sampler): the hardware picks the right level per zoom.
    present_bg: wgpu::BindGroup,
    /// Downsample chain: for each level 1..n, (that level's render-target
    /// view, a bind group sampling the level above).
    mip_chain: Vec<(wgpu::TextureView, wgpu::BindGroup)>,
}

struct SurfaceState {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

/// CPU-side cost of one `update_canvas` (tile uploads + composite encode +
/// submit). GPU execution time is not in here — measuring it would need
/// timestamp queries — but the CPU side is where a struggling
/// driver/adapter stalls first, and it needs no extra GPU work to observe.
/// The app aggregates these into the tester log ("passive GPU telemetry":
/// any user's log shows how their GPU fares without being asked anything).
#[derive(Clone, Copy, Default)]
pub struct FrameStats {
    /// Tiles whose pixels were written to GPU textures this frame.
    pub uploads: u32,
    /// Tile regions the composite redrew (0 = frame did no composite).
    pub composite_tiles: u32,
    /// The whole canvas was rebuilt (layer/paper/layout change, not damage).
    pub full: bool,
    /// Milliseconds spent CPU-side in upload + composite encode/submit.
    pub ms: f32,
}

impl FrameStats {
    /// Did this frame actually upload or composite anything?
    pub fn worked(&self) -> bool {
        self.uploads > 0 || self.composite_tiles > 0
    }
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    #[allow(dead_code)]
    adapter: wgpu::Adapter,
    adapter_info: wgpu::AdapterInfo,

    surface: Option<SurfaceState>,

    /// One pipeline per fixed-function `Blend` variant; index with
    /// `blend_slot` (every other mode takes the blend2 shader pass).
    tile_pipelines: [wgpu::RenderPipeline; 5],
    /// The same set, targeting `GROUP_FORMAT` (children drawing into an
    /// isolation buffer).
    tile_pipelines_group: [wgpu::RenderPipeline; 5],
    /// PA-001: the damaged-region reset, a straight replace (see
    /// `blend_replace`). Never used for layer content.
    tile_pipeline_reset: wgpu::RenderPipeline,
    /// Coverage multiply (`dst *= src.a`) into a group — frame-folder masks
    /// and clip-to-below bases. Group target only.
    mask_pipeline: wgpu::RenderPipeline,
    /// Coverage multiply sourced from the CLIP-BASE capture texture instead
    /// of a tile (clip-to-folder: the base is a canvas-sized group alpha,
    /// not a layer's tiles). Group target only, like `mask_pipeline`.
    mask_base_pipeline: wgpu::RenderPipeline,
    /// Blit an isolation buffer onto the canvas / an outer group, one per
    /// blend mode per target family.
    blit_pipelines: [wgpu::RenderPipeline; 5],
    blit_pipelines_group: [wgpu::RenderPipeline; 5],
    blit_bgl: wgpu::BindGroupLayout,
    tile_texture_bgl: wgpu::BindGroupLayout,
    tile_uniform_buf: wgpu::Buffer,
    tile_uniform_bg: wgpu::BindGroup,
    /// Bound for the paper-white reset quads, which sample no texture but still
    /// have to satisfy the bind group layout. Also the zero-coverage mask quad
    /// (wgpu zero-initialises textures, so its alpha reads 0).
    dummy_tile_bg: wgpu::BindGroup,
    /// Isolation buffers by level (1-based; index 0 unused). Canvas-sized,
    /// recreated with the canvas.
    groups: Vec<GroupTex>,
    /// Clip-to-folder base capture: a folder that serves as a clip base has
    /// its finished group (frame mask applied, before opacity/blend) copied
    /// here at close, because the member's own scratch reuses the folder's
    /// level and would clear it. One texture suffices — clip-run members sit
    /// directly above their folder, so captures never overlap. Canvas-sized,
    /// recreated with the canvas; `None` until a folder base first appears.
    clip_base: Option<GroupTex>,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,

    /// Present pipeline for the swapchain format.
    present_pipeline: wgpu::RenderPipeline,
    /// Present pipeline for `CANVAS_FORMAT`, used by `render_offscreen` when the
    /// swapchain format differs (an sRGB surface would otherwise be a format
    /// mismatch against the offscreen texture).
    present_pipeline_canvas: Option<wgpu::RenderPipeline>,
    /// True when the swapchain format is sRGB and the present shader has to
    /// pre-decode.
    surface_is_srgb: bool,
    present_bgl: wgpu::BindGroupLayout,
    present_uniform_buf: wgpu::Buffer,
    sampler: wgpu::Sampler,

    /// Regenerates the canvas mip chain from level 0 (downsample.wgsl).
    mip_pipeline: wgpu::RenderPipeline,
    /// Canvas content changed since the mips were last rebuilt.
    mips_dirty: bool,
    canvas: Option<Canvas>,
    tiles: HashMap<TileKey, CachedTile>,
    /// GPU-dab compute path (P1); `None` when the adapter's rgba16uint
    /// textures lack STORAGE_BINDING (`--gpu-dabs` is then inert).
    dabs: Option<dabs::DabGpu>,
    /// The shared tile-kernel seam (correction derives, the blur family);
    /// `None` when the adapter has no compute shaders. See `kernel.rs`.
    kernels: Option<kernel::KernelGpu>,
    /// Canvas-side freshness: the revision each tile was last COMPOSITED at,
    /// keyed like the texture cache. The second half of the freshness
    /// question `CachedTile.revision` used to answer alone (the round-31
    /// display bug — the two meanings coincided for every CPU edit, which is
    /// why nothing caught them drifting apart): that field means "the TEXTURE
    /// matches the CPU tile" (skip upload); this map means "the CANVAS
    /// already shows this tile at this revision" (skip redraw). The GPU dab
    /// path is the one case that splits them: after a flush + stroke-end
    /// readback the texture is already correct (`mark_dab_tile_clean` ⇒ no
    /// upload) while the canvas has not shown the dabs yet (entries here stay
    /// behind ⇒ redraw required). Damage is DERIVED from this compare on
    /// every composite — never consumed from a one-shot set, so a second
    /// composite of the same canvas (Pages thumbnail then main view) cannot
    /// eat the first one's redraw.
    canvas_shown: HashMap<TileKey, u64>,
    /// What the last `update_canvas` cost (CPU-side), for the tester log's
    /// passive GPU telemetry. Zeroed when a frame had nothing to do.
    frame_stats: FrameStats,
    /// Blend part 2: the shader compositor pass for the modes fixed-function
    /// blending cannot express. Pipelines + bind group layouts; the SNAPSHOT
    /// texture (canvas-sized copy of the destination, taken between passes)
    /// lives with the canvas.
    blend2_tile_pipe: wgpu::RenderPipeline,
    blend2_blit_pipe: wgpu::RenderPipeline,
    blend2_tile_bgl: wgpu::BindGroupLayout,
    blend2_blit_bgl: wgpu::BindGroupLayout,
    /// (texture, tile-flavoured bg, blit-flavoured bg) — recreated with the
    /// canvas by `ensure_canvas`.
    snap: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// Recycled tile textures (with their bind groups). NOT an optimisation
    /// first: destroying tile textures and creating new ones in a later frame
    /// makes this laptop's Intel UHD 620 DX12 driver intermittently sample
    /// stale memory from the new texture (reproduced 2026-08-14: borders
    /// vanish nondeterministically on hardware, never on WARP; an empty
    /// flush submit does not fix it). Reusing the same `wgpu::Texture`
    /// objects avoids the free→realloc alias entirely — every upload is a
    /// full-tile `write_texture`, so contents never leak between uses.
    tile_pool: Vec<CachedTile>,
    /// Next canvas pass must clear + redraw everything.
    canvas_dirty_all: bool,
    /// Layer presentation state as of the last composite.
    layer_sig: Vec<LayerSig>,
    /// LP-022 page half: display EVERY layer as monochrome (1-bit look)
    /// on the canvas — display only, never a composite/export. Set by
    /// the app; entering the sig path so toggling it re-draws.
    pub mono_preview: bool,
    /// PA-001: the paper as of the last composite — changing it re-clears
    /// the whole canvas (it is the bottom of every region, not a layer).
    paper_sig: Paper,
    /// PA-001: forces a paper for this renderer, whatever the document says.
    /// `AppCmd::ExportPngPath` sets it so a page exports on its paper colour
    /// even while the editor is showing the transparency checker — the eye is
    /// a check you switch on, never an export mode. See
    /// `Document::paper_export_background`, the CPU path's half of the rule.
    paper_override: Option<Paper>,
}

impl Renderer {
    /// Build a renderer that presents into an existing Win32 window.
    ///
    /// # Safety
    /// `hwnd` must be a valid window handle that outlives this `Renderer`.
    pub unsafe fn new_windowed(
        hwnd: isize,
        hinstance: isize,
        width: u32,
        height: u32,
        cfg: GpuConfig,
    ) -> Result<Self, GpuError> {
        use raw_window_handle::{
            RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
        };

        let instance = new_instance();

        let mut wh = Win32WindowHandle::new(
            std::num::NonZeroIsize::new(hwnd).ok_or_else(|| GpuError("null HWND".into()))?,
        );
        wh.hinstance = std::num::NonZeroIsize::new(hinstance);

        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(RawDisplayHandle::Windows(WindowsDisplayHandle::new())),
            raw_window_handle: RawWindowHandle::Win32(wh),
        };
        // SAFETY: caller guarantees the HWND outlives the returned Renderer.
        let surface = unsafe { instance.create_surface_unsafe(target) }
            .map_err(|e| GpuError(format!("create_surface_unsafe: {e}")))?;

        let (adapter, device, queue) = request_gpu(&instance, Some(&surface), cfg)?;

        let caps = surface.get_capabilities(&adapter);
        let format = pick_surface_format(&caps);
        let present_mode = if cfg.no_vsync {
            wgpu::PresentMode::AutoNoVsync
        } else {
            wgpu::PresentMode::AutoVsync
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: width.max(1),
            height: height.max(1),
            present_mode,
            // Inking latency: ask for the shortest queue the backend allows.
            desired_maximum_frame_latency: 1,
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        println!(
            "[gpu] surface: {format:?} | present {present_mode:?} | {}x{}",
            config.width, config.height
        );

        let mut r = Self::assemble(adapter, device, queue, format)?;
        r.surface = Some(SurfaceState { surface, config });
        Ok(r)
    }

    /// Build a renderer with no swapchain — for `render_offscreen` and tests.
    pub fn new_headless(cfg: GpuConfig) -> Result<Self, GpuError> {
        let instance = new_instance();
        let (adapter, device, queue) = request_gpu(&instance, None, cfg)?;
        Self::assemble(adapter, device, queue, CANVAS_FORMAT)
    }

    fn assemble(
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Result<Self, GpuError> {
        let adapter_info = adapter.get_info();

        let tile_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mn.tiles"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/tiles.wgsl").into()),
        });
        let present_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mn.present"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/present.wgsl").into()),
        });

        let tile_uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mn.tile.uniform"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                // The group-blit fragment shader reads the canvas size.
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tile_texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mn.tile.texture"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let tile_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mn.tile.layout"),
            bind_group_layouts: &[Some(&tile_uniform_bgl), Some(&tile_texture_bgl)],
            immediate_size: 0,
        });

        // ---- the three blend states --------------------------------------
        //
        // `s` = source (a tile texel, already scaled by layer opacity in the
        // fragment shader), `d` = destination (the canvas). Everything is
        // PREMULTIPLIED. These are the same equations as `mn_core::blend`, and
        // `tests/composite.rs` renders both ways and asserts they agree.
        //
        // The canvas is always cleared to OPAQUE paper white, so `d.a == 1`
        // throughout — that is what lets Multiply fit in two fixed-function
        // terms (its general premultiplied form has three).
        use wgpu::{BlendComponent as Bc, BlendFactor as Bf, BlendOperation as Bo, BlendState};

        // Normal (svg:src-over)
        //   rgb: s.rgb * 1 + d.rgb * (1 - s.a)
        //   a  : s.a   * 1 + d.a   * (1 - s.a)
        // Opaque source (the paper-white reset quad) degenerates to a replace.
        let blend_normal = BlendState {
            color: Bc {
                src_factor: Bf::One,
                dst_factor: Bf::OneMinusSrcAlpha,
                operation: Bo::Add,
            },
            alpha: Bc {
                src_factor: Bf::One,
                dst_factor: Bf::OneMinusSrcAlpha,
                operation: Bo::Add,
            },
        };

        // PA-001: the damaged-region reset is a REPLACE — src wins outright,
        // the destination is discarded. With opaque paper this is bit-for-bit
        // what `blend_normal` already did (an opaque source covers everything
        // anyway); with the paper HIDDEN it is the only state that can put
        // transparency *back* into a region, which src-over cannot do —
        // blending a zero-alpha source leaves the old pixels standing.
        let blend_replace = BlendState {
            color: Bc {
                src_factor: Bf::One,
                dst_factor: Bf::Zero,
                operation: Bo::Add,
            },
            alpha: Bc {
                src_factor: Bf::One,
                dst_factor: Bf::Zero,
                operation: Bo::Add,
            },
        };

        // Multiply (svg:multiply), with d.a == 1
        //   general: s.rgb*d.rgb + s.rgb*(1 - d.a) + d.rgb*(1 - s.a)
        //   d.a==1 : s.rgb*d.rgb                   + d.rgb*(1 - s.a)
        //   rgb: s.rgb * Dst + d.rgb * (1 - s.a)
        //   a  : s.a   * 1   + d.a   * (1 - s.a)   = 1 when d.a == 1
        let blend_multiply = BlendState {
            color: Bc {
                src_factor: Bf::Dst,
                dst_factor: Bf::OneMinusSrcAlpha,
                operation: Bo::Add,
            },
            alpha: Bc {
                src_factor: Bf::One,
                dst_factor: Bf::OneMinusSrcAlpha,
                operation: Bo::Add,
            },
        };

        // Screen (svg:screen)
        //   s.rgb + d.rgb - s.rgb*d.rgb  (exact for premultiplied, any d.a)
        //   rgb: s.rgb * (1 - Dst) + d.rgb * 1
        //   a  : s.a   * 1         + d.a   * (1 - s.a)
        let blend_screen = BlendState {
            color: Bc {
                src_factor: Bf::OneMinusDst,
                dst_factor: Bf::One,
                operation: Bo::Add,
            },
            alpha: Bc {
                src_factor: Bf::One,
                dst_factor: Bf::OneMinusSrcAlpha,
                operation: Bo::Add,
            },
        };

        // Add (round 27, owner image = CSP's blend list): OUR operator
        // (mn:add), defined directly on premultiplied values so CPU and GPU
        // agree at every alpha (see core::blend — the straight-colour SVG
        // form would diverge on translucent sources).
        //   Add:      out.rgb = min(s.rgb + d.rgb, 1)
        // Subtract's ReverseSubtract state below is RETIRED (slot 4 kept
        // only for array alignment): the premultiplied form painted black
        // over a transparent destination, so Subtract moved to blend2.
        let blend_alpha_src_over = Bc {
            src_factor: Bf::One,
            dst_factor: Bf::OneMinusSrcAlpha,
            operation: Bo::Add,
        };
        let blend_add = BlendState {
            color: Bc {
                src_factor: Bf::One,
                dst_factor: Bf::One,
                operation: Bo::Add,
            },
            alpha: blend_alpha_src_over,
        };
        let blend_subtract = BlendState {
            color: Bc {
                src_factor: Bf::One,
                dst_factor: Bf::One,
                operation: Bo::ReverseSubtract,
            },
            alpha: blend_alpha_src_over,
        };

        let tile_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Uint32,
                    offset: 16,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 20,
                    shader_location: 2,
                },
                // The blend2 mode sentinel (blend_slot ≥ 16) rides the pad;
                // 0 for every fixed-function draw.
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Uint32,
                    offset: 24,
                    shader_location: 3,
                },
                // LP-016 layer colour (tiles.wgsl location 4).
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Uint32,
                    offset: 28,
                    shader_location: 4,
                },
                // LP-017 sub colour + LP-022 reduce, packed (location 5).
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Uint32,
                    offset: 32,
                    shader_location: 5,
                },
            ],
        };

        let make_tile_pipeline = |label: &str, blend: BlendState, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&tile_layout),
                vertex: wgpu::VertexState {
                    module: &tile_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(tile_vertex_layout.clone())],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &tile_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // Order must match `blend_slot`.
        let tile_pipelines = [
            make_tile_pipeline("mn.tile.normal", blend_normal, CANVAS_FORMAT),
            make_tile_pipeline("mn.tile.multiply", blend_multiply, CANVAS_FORMAT),
            make_tile_pipeline("mn.tile.screen", blend_screen, CANVAS_FORMAT),
            make_tile_pipeline("mn.tile.add", blend_add, CANVAS_FORMAT),
            make_tile_pipeline("mn.tile.subtract", blend_subtract, CANVAS_FORMAT),
        ];
        // PA-001: `DrawKind::Reset` only.
        let tile_pipeline_reset = make_tile_pipeline("mn.tile.reset", blend_replace, CANVAS_FORMAT);
        let tile_pipelines_group = [
            make_tile_pipeline("mn.tile.g.normal", blend_normal, GROUP_FORMAT),
            make_tile_pipeline("mn.tile.g.multiply", blend_multiply, GROUP_FORMAT),
            make_tile_pipeline("mn.tile.g.screen", blend_screen, GROUP_FORMAT),
            make_tile_pipeline("mn.tile.g.add", blend_add, GROUP_FORMAT),
            make_tile_pipeline("mn.tile.g.subtract", blend_subtract, GROUP_FORMAT),
        ];

        // Coverage multiply: out = dst * src.a, all four channels. The mask /
        // clip-base texel arrives premultiplied white, so its alpha IS the
        // coverage; drawing the dummy (zero-initialised) texture zeroes the
        // region — an absent mask tile means "outside every panel".
        let blend_mask = BlendState {
            color: Bc {
                src_factor: Bf::Zero,
                dst_factor: Bf::SrcAlpha,
                operation: Bo::Add,
            },
            alpha: Bc {
                src_factor: Bf::Zero,
                dst_factor: Bf::SrcAlpha,
                operation: Bo::Add,
            },
        };
        let mask_pipeline = make_tile_pipeline("mn.tile.mask", blend_mask, GROUP_FORMAT);

        // Group blit: sample an isolation buffer and blend it as one source.
        let group_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mn.group"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/group.wgsl").into()),
        });
        let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mn.blit.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mn.blit.layout"),
            bind_group_layouts: &[Some(&tile_uniform_bgl), Some(&blit_bgl)],
            immediate_size: 0,
        });
        let make_blit_pipeline = |label: &str, blend: BlendState, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&blit_layout),
                vertex: wgpu::VertexState {
                    module: &group_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(tile_vertex_layout.clone())],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &group_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // FIVE entries, indexed by blend_slot, exactly like tile_pipelines:
        // the layers palette offers every fixed mode for FOLDERS too, and a
        // folder blit indexes this array directly — with only the first
        // three, setting a folder to Add was an index-out-of-bounds panic
        // (two clicks in the UI). Slot 4 is the retired Subtract state,
        // kept for alignment; nothing selects it.
        let blit_pipelines = [
            make_blit_pipeline("mn.blit.normal", blend_normal, CANVAS_FORMAT),
            make_blit_pipeline("mn.blit.multiply", blend_multiply, CANVAS_FORMAT),
            make_blit_pipeline("mn.blit.screen", blend_screen, CANVAS_FORMAT),
            make_blit_pipeline("mn.blit.add", blend_add, CANVAS_FORMAT),
            make_blit_pipeline("mn.blit.subtract", blend_subtract, CANVAS_FORMAT),
        ];
        let blit_pipelines_group = [
            make_blit_pipeline("mn.blit.g.normal", blend_normal, GROUP_FORMAT),
            make_blit_pipeline("mn.blit.g.multiply", blend_multiply, GROUP_FORMAT),
            make_blit_pipeline("mn.blit.g.screen", blend_screen, GROUP_FORMAT),
            make_blit_pipeline("mn.blit.g.add", blend_add, GROUP_FORMAT),
            make_blit_pipeline("mn.blit.g.subtract", blend_subtract, GROUP_FORMAT),
        ];
        // Clip-to-folder: coverage multiply like `mask_pipeline`, but the
        // source is the canvas-sized clip-base capture (group.wgsl sampling,
        // instance opacity 1), not a tile. Clip scratches are groups, so one
        // GROUP_FORMAT flavour is enough.
        let mask_base_pipeline = make_blit_pipeline("mn.mask.base", blend_mask, GROUP_FORMAT);

        // Blend part 2 — the shader compositor pass. The mode value rides
        // the instance pad (shader location 3); the destination arrives as a
        // SNAPSHOT texture copied between passes (a pass cannot read its own
        // target). REPLACE blend: the shader computes the full RGBA.
        let blend2_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mn.blend2"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blend2.wgsl").into()),
        });
        let blend2_tile_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mn.blend2.tile.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let blend2_blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mn.blend2.blit.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let make_blend2_pipeline =
            |label: &str,
             entry: &str,
             layout: &wgpu::PipelineLayout,
             format: wgpu::TextureFormat| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &blend2_shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[Some(tile_vertex_layout.clone())],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &blend2_shader,
                        entry_point: Some(entry),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(BlendState::REPLACE),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            };
        let blend2_tile_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mn.blend2.tile.layout"),
            bind_group_layouts: &[Some(&tile_uniform_bgl), Some(&blend2_tile_bgl)],
            immediate_size: 0,
        });
        let blend2_blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mn.blend2.blit.layout"),
            bind_group_layouts: &[Some(&tile_uniform_bgl), Some(&blend2_blit_bgl)],
            immediate_size: 0,
        });
        // Canvas and group targets are both Rgba8Unorm, so one pipeline of
        // each flavour serves every target.
        let blend2_tile_pipe = make_blend2_pipeline(
            "mn.blend2.tile",
            "fs_tile",
            &blend2_tile_layout,
            GROUP_FORMAT,
        );
        let blend2_blit_pipe = make_blend2_pipeline(
            "mn.blend2.blit",
            "fs_blit",
            &blend2_blit_layout,
            GROUP_FORMAT,
        );

        // Mip downsample: full-target triangle, blit_bgl-shaped bind group
        // (texture + sampler), one pass per level.
        let downsample_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mn.downsample"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/downsample.wgsl").into()),
        });
        let mip_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mn.mip.layout"),
            bind_group_layouts: &[Some(&blit_bgl)],
            immediate_size: 0,
        });
        let mip_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mn.mip.pipeline"),
            layout: Some(&mip_layout),
            vertex: wgpu::VertexState {
                module: &downsample_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &downsample_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: CANVAS_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let present_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mn.present.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    // The fragment stage reads `flags.x` (the sRGB switch), so
                    // this uniform is visible to both stages.
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let present_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mn.present.layout"),
            bind_group_layouts: &[Some(&present_bgl)],
            immediate_size: 0,
        });
        let make_present_pipeline = |label: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&present_layout),
                vertex: wgpu::VertexState {
                    module: &present_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &present_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let present_pipeline = make_present_pipeline("mn.present.pipeline", surface_format);
        // `render_offscreen` always targets CANVAS_FORMAT; a pipeline's target
        // format must match its attachment, so an sRGB swapchain needs a second
        // one rather than a runtime surprise.
        let present_pipeline_canvas = if surface_format == CANVAS_FORMAT {
            None
        } else {
            Some(make_present_pipeline(
                "mn.present.pipeline.canvas",
                CANVAS_FORMAT,
            ))
        };

        let tile_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mn.tile.uniform.buf"),
            size: std::mem::size_of::<CanvasUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tile_uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mn.tile.uniform.bg"),
            layout: &tile_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: tile_uniform_buf.as_entire_binding(),
            }],
        });
        let present_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mn.present.uniform.buf"),
            size: std::mem::size_of::<PresentUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let instance_cap = 256usize;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mn.instances"),
            size: (instance_cap * std::mem::size_of::<QuadInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Paper-white reset quads sample nothing, but group 1 still has to be
        // bound with *something* the layout accepts.
        let dummy_tile_bg = {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mn.tile.dummy"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TILE_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mn.tile.dummy.bg"),
                layout: &tile_texture_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            })
        };

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mn.canvas.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // Trilinear across the canvas mip chain — the zoomed-out view
            // blends two area averages instead of shimmering on level 0.
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        // P1 gate: the GPU-dab compute path needs rgba16uint STORAGE_BINDING
        // on tile textures. Unsupported adapters keep the CPU raster path.
        let tff = adapter.get_texture_format_features(TILE_FORMAT);
        let dabs = if tff
            .allowed_usages
            .contains(wgpu::TextureUsages::STORAGE_BINDING)
        {
            Some(dabs::DabGpu::new(&device))
        } else {
            println!("[gpu] rgba16uint storage unsupported — GPU dabs disabled (CPU path)");
            None
        };

        // The kernel seam needs only compute + storage BUFFERS, which is a
        // weaker ask than the dab path's storage TEXTURES — an adapter can
        // legitimately have one and not the other, so this gate is its own.
        let kernels = if adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS)
        {
            Some(kernel::KernelGpu::new(&device))
        } else {
            println!("[gpu] no compute shaders — kernel seam disabled (CPU corrections/filters)");
            None
        };

        Ok(Self {
            device,
            queue,
            adapter,
            adapter_info,
            surface: None,
            tile_pipelines,
            tile_pipelines_group,
            mask_pipeline,
            mask_base_pipeline,
            blit_pipelines,
            blit_pipelines_group,
            blit_bgl,
            tile_texture_bgl,
            tile_uniform_buf,
            tile_uniform_bg,
            dummy_tile_bg,
            groups: Vec::new(),
            clip_base: None,
            instance_buf,
            instance_cap,
            present_pipeline,
            present_pipeline_canvas,
            surface_is_srgb: surface_format.is_srgb(),
            present_bgl,
            present_uniform_buf,
            sampler,
            mip_pipeline,
            mips_dirty: true,
            canvas: None,
            tiles: HashMap::new(),
            dabs,
            kernels,
            canvas_shown: HashMap::new(),
            frame_stats: FrameStats::default(),
            blend2_tile_pipe,
            blend2_blit_pipe,
            blend2_tile_bgl,
            blend2_blit_bgl,
            snap: None,
            tile_pool: Vec::new(),
            canvas_dirty_all: true,
            layer_sig: Vec::new(),
            mono_preview: false,
            paper_sig: Paper::default(),
            paper_override: None,
            tile_pipeline_reset,
        })
    }

    /// PA-001: render as if the document's paper were this one.
    ///
    /// Set it around an export so the page composites on its paper colour
    /// even while the editor is showing the transparency checker, then clear
    /// it. The eye is a check you switch on to find holes in your flats; it
    /// must never be the reason a page ships with a transparent background,
    /// and a checker must never reach a PNG. Changing it re-clears the canvas.
    pub fn set_paper_override(&mut self, paper: Option<Paper>) {
        if self.paper_override != paper {
            self.paper_override = paper;
            self.canvas_dirty_all = true;
        }
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    /// One-line human-readable adapter identity for the console + (later) HUD.
    ///
    /// Also the KEY the measured GPU-inking verdict is filed under, so its
    /// exact bytes matter: DX12 reports an empty `driver_info`, and joining
    /// it unconditionally left a trailing space on every Windows launch.
    /// The stored copy is trimmed when read, so the two never compared
    /// equal — the measured default could never apply. Build the driver
    /// half conditionally and there is no stray space to lose.
    pub fn adapter_line(&self) -> String {
        let i = &self.adapter_info;
        let driver = match (i.driver.trim(), i.driver_info.trim()) {
            ("", "") => "unknown".to_string(),
            (d, "") => d.to_string(),
            ("", info) => info.to_string(),
            (d, info) => format!("{d} {info}"),
        };
        format!(
            "{} | backend {:?} | type {:?} | driver {}",
            i.name, i.backend, i.device_type, driver
        )
    }

    pub fn present_mode(&self) -> Option<wgpu::PresentMode> {
        self.surface.as_ref().map(|s| s.config.present_mode)
    }

    pub fn surface_size(&self) -> (u32, u32) {
        self.surface
            .as_ref()
            .map(|s| (s.config.width, s.config.height))
            .unwrap_or((0, 0))
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if let Some(s) = &mut self.surface {
            if s.config.width == width && s.config.height == height {
                return;
            }
            s.config.width = width;
            s.config.height = height;
            s.surface.configure(&self.device, &s.config);
        }
    }

    /// Drop every cached tile texture and force a full canvas rebuild.
    ///
    /// The blunt hammer. Most cases do not need it: tile uploads are revision
    /// driven, tiles that disappear (undo, layer delete) are evicted
    /// automatically, and layer opacity/blend/visibility/order changes are
    /// detected by comparing a per-layer signature. Call it after loading a
    /// document, or whenever you want to be certain.
    pub fn invalidate(&mut self) {
        let pool = &mut self.tile_pool;
        pool.extend(self.tiles.drain().map(|(_, t)| t));
        pool.truncate(TILE_POOL_CAP);
        self.layer_sig.clear();
        self.canvas_dirty_all = true;
    }

    /// Drop the cached tiles of one layer index and force a redraw. Cheaper than
    /// [`Renderer::invalidate`] when you know what changed; correct to call at
    /// any time.
    pub fn evict_layer(&mut self, layer: usize) {
        let pool = &mut self.tile_pool;
        self.tiles.retain(|(li, _, _), t| {
            let keep = *li != layer;
            if !keep && pool.len() < TILE_POOL_CAP {
                // Recycled, not destroyed — see `tile_pool`.
                pool.push(CachedTile {
                    texture: t.texture.clone(),
                    bind_group: t.bind_group.clone(),
                    revision: 0,
                });
            }
            keep
        });
        self.canvas_dirty_all = true;
    }

    /// Number of tile textures currently resident. Diagnostics/HUD.
    pub fn cached_tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Composite `doc` and present it through `vp`.
    ///
    /// Contract signature (docs/ARCHITECTURE.md). Recoverable surface errors are
    /// swallowed on purpose so the Win32 message loop never has to care.
    pub fn render(&mut self, doc: &Document, vp: &Viewport) {
        let Some(surface_size) = self
            .surface
            .as_ref()
            .map(|s| (s.config.width, s.config.height))
        else {
            return;
        };
        if surface_size.0 == 0 || surface_size.1 == 0 {
            return;
        }

        self.update_canvas(doc);

        let frame = {
            use wgpu::CurrentSurfaceTexture as Cst;
            let s = self.surface.as_ref().unwrap();
            match s.surface.get_current_texture() {
                Cst::Success(f) => Some(f),
                // Suboptimal still presents; reconfiguring next frame is enough.
                Cst::Suboptimal(f) => {
                    self.canvas_dirty_all = true;
                    Some(f)
                }
                Cst::Lost | Cst::Outdated => {
                    s.surface.configure(&self.device, &s.config);
                    None
                }
                Cst::Timeout | Cst::Occluded => None,
                other => {
                    eprintln!("[gpu] surface acquire failed: {other:?}");
                    None
                }
            }
        };
        let Some(frame) = frame else { return };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.present_into(doc, vp, &view, surface_size);
        // wgpu 30 moved presentation onto the queue; dropping the frame
        // un-presented would silently discard it.
        self.queue.present(frame);
    }

    /// Like [`Renderer::render`], but hands the swapchain view to `overlay`
    /// after the canvas has been drawn and before the frame is presented, so a
    /// UI layer (the app's egui pass) lands in the same frame.
    ///
    /// `overlay` gets `(device, queue, encoder, swapchain view, size in px)`.
    /// It may `queue.submit` its own buffers; those land before this method's
    /// encoder, which is submitted right after the closure returns.
    ///
    /// Additive on purpose: it reuses `update_canvas` + `present_into` and
    /// changes nothing about the existing paths (see docs/ARCHITECTURE.md —
    /// `crates/gpu` is owned by the compositor agent, this is the app's hook).
    pub fn render_with_overlay(
        &mut self,
        doc: &Document,
        vp: &Viewport,
        overlay: impl FnOnce(
            &wgpu::Device,
            &wgpu::Queue,
            &mut wgpu::CommandEncoder,
            &wgpu::TextureView,
            (u32, u32),
        ),
    ) {
        let Some(surface_size) = self
            .surface
            .as_ref()
            .map(|s| (s.config.width, s.config.height))
        else {
            return;
        };
        if surface_size.0 == 0 || surface_size.1 == 0 {
            return;
        }

        self.update_canvas(doc);

        let frame = {
            use wgpu::CurrentSurfaceTexture as Cst;
            let s = self.surface.as_ref().unwrap();
            match s.surface.get_current_texture() {
                Cst::Success(f) => Some(f),
                Cst::Suboptimal(f) => {
                    self.canvas_dirty_all = true;
                    Some(f)
                }
                Cst::Lost | Cst::Outdated => {
                    s.surface.configure(&self.device, &s.config);
                    None
                }
                Cst::Timeout | Cst::Occluded => None,
                other => {
                    eprintln!("[gpu] surface acquire failed: {other:?}");
                    None
                }
            }
        };
        let Some(frame) = frame else { return };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.present_into(doc, vp, &view, surface_size);

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mn.overlay"),
            });
        overlay(&self.device, &self.queue, &mut enc, &view, surface_size);
        self.queue.submit([enc.finish()]);

        self.queue.present(frame);
    }

    /// The wgpu device, for code that renders its own pass into our frames
    /// (the app's egui painter).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The wgpu queue, same purpose as [`Renderer::device`].
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// What the last frame's `update_canvas` cost (see [`FrameStats`]).
    pub fn frame_stats(&self) -> FrameStats {
        self.frame_stats
    }

    /// Format of the swapchain (or the canvas format when headless) — an
    /// overlay pipeline must be created for this target format.
    pub fn output_format(&self) -> wgpu::TextureFormat {
        self.surface
            .as_ref()
            .map(|s| s.config.format)
            .unwrap_or(CANVAS_FORMAT)
    }

    /// Render the document to an image, no window involved. Used by the
    /// `offscreen` example and (later) export.
    pub fn render_offscreen(&mut self, doc: &Document, w: u32, h: u32) -> image::RgbaImage {
        let vp = Viewport::fit(doc.size, (w.max(1), h.max(1)));
        self.render_offscreen_vp(doc, &vp, w, h)
    }

    /// Same, but through a caller-supplied viewport — the `--screenshot`
    /// harness uses the app's real one so canvas and egui overlay line up.
    pub fn render_offscreen_vp(
        &mut self,
        doc: &Document,
        vp: &Viewport,
        w: u32,
        h: u32,
    ) -> image::RgbaImage {
        let w = w.max(1);
        let h = h.max(1);
        self.update_canvas(doc);
        if std::env::var("MN_DUMP_CANVAS").is_ok() {
            if let Some(c) = &self.canvas {
                // Fixed name: overwritten per composite, so the file left
                // behind is the LAST (usually the screenshot's) composite.
                let img =
                    read_texture_rgba(&self.device, &self.queue, &c.texture, c.size.0, c.size.1);
                let _ = img.save("canvas-dump.png");
            }
        }

        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mn.offscreen"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CANVAS_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());

        // At w/h == the document size the fit viewport is an exact 1:1 blit,
        // which is what the CPU-vs-GPU blend agreement test relies on.
        // CANVAS_FORMAT is not sRGB, so no encoding in the shader.
        self.present_into_format(doc, vp, &view, (w, h), false, true);

        read_texture_rgba(&self.device, &self.queue, &target, w, h)
    }

    /// The present pass onto the swapchain, shared by `render` and
    /// `render_with_overlay`.
    fn present_into(
        &mut self,
        doc: &Document,
        vp: &Viewport,
        target: &wgpu::TextureView,
        target_size: (u32, u32),
    ) {
        let srgb = self.surface_is_srgb;
        self.present_into_format(doc, vp, target, target_size, srgb, false);
    }

    /// Walk the downsample chain once after canvas content changed. Each pass
    /// is one oversized triangle into the next level; skipped entirely while
    /// the canvas is clean, so panning/zooming costs nothing extra.
    fn regen_mips_if_dirty(&mut self) {
        if !std::mem::take(&mut self.mips_dirty) {
            return;
        }
        let Some(canvas) = &self.canvas else { return };
        if canvas.mip_chain.is_empty() {
            return;
        }
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mn.mips"),
            });
        for (target, src_bg) in &canvas.mip_chain {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mn.mip.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&self.mip_pipeline);
            rp.set_bind_group(0, src_bg, &[]);
            rp.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
    }

    /// The present pass. `canvas_format_target` selects the pipeline built for
    /// `CANVAS_FORMAT` (offscreen) instead of the swapchain one.
    fn present_into_format(
        &mut self,
        doc: &Document,
        vp: &Viewport,
        target: &wgpu::TextureView,
        target_size: (u32, u32),
        srgb_target: bool,
        canvas_format_target: bool,
    ) {
        self.regen_mips_if_dirty();
        let Some(canvas) = &self.canvas else { return };

        // Canvas -> screen for all four corners (this is where rotation lands),
        // then screen -> NDC. Doing it on the CPU keeps the vertex shader a
        // lookup and costs four sin/cos per frame.
        let (sw, sh) = (target_size.0 as f32, target_size.1 as f32);
        let c = vp.corners_screen(doc.size);
        let ndc = |p: [f32; 2]| [p[0] / sw * 2.0 - 1.0, 1.0 - p[1] / sh * 2.0];
        let (tl, tr, bl, br) = (ndc(c[0]), ndc(c[1]), ndc(c[2]), ndc(c[3]));
        // PA-001: the checker is the present pass's job, and it is gated on
        // the SAME paper the canvas pass used (`paper_override` included) —
        // otherwise an export would composite the art over a checker.
        let paper = self.paper_override.unwrap_or(doc.paper);
        let uni = PresentUniform {
            c01: [tl[0], tl[1], tr[0], tr[1]],
            c23: [bl[0], bl[1], br[0], br[1]],
            flags: [srgb_target as u32, (!paper.visible) as u32, 0, 0],
        };
        self.queue
            .write_buffer(&self.present_uniform_buf, 0, bytemuck::bytes_of(&uni));

        let pipeline = if canvas_format_target {
            self.present_pipeline_canvas
                .as_ref()
                .unwrap_or(&self.present_pipeline)
        } else {
            &self.present_pipeline
        };

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mn.present"),
            });
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mn.present.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Clear values are written through the same format
                        // conversion as the shader output, so the backdrop needs
                        // the same pre-decode on an sRGB target.
                        load: wgpu::LoadOp::Clear(if srgb_target {
                            srgb_decode_color(BACKDROP)
                        } else {
                            BACKDROP
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(pipeline);
            rp.set_bind_group(0, &canvas.present_bg, &[]);
            rp.draw(0..4, 0..1);
        }
        self.queue.submit([enc.finish()]);
    }

    /// Upload changed tiles and (re)composite them into the canvas texture.
    ///
    /// # Strategy: damaged-region recomposite of the whole stack
    ///
    /// The unit of work is a **tile-sized region of the canvas**. A region is
    /// damaged when any layer's tile at that index was re-uploaded. Each damaged
    /// region is then rebuilt from scratch: paper white, then every visible
    /// layer's tile at that index, bottom to top, each through its own blend
    /// pipeline at its own opacity. That is correct for any number of layers
    /// with any blend modes, and still only touches the tiles you painted —
    /// inking a 2048² document does not redraw the whole page.
    ///
    /// A full rebuild (clear to paper + every populated region) happens when the
    /// canvas is resized, a tile disappears, layer presentation state changes,
    /// or `invalidate()` was called.
    fn update_canvas(&mut self, doc: &Document) {
        // Telemetry: zeroed now, filled at the end when work happened, so a
        // frame that early-returns honestly reports "did nothing".
        self.frame_stats = FrameStats::default();
        let t0 = std::time::Instant::now();
        self.ensure_canvas(doc);
        let Some(canvas) = &self.canvas else { return };
        let canvas_view = canvas.view.clone();

        // Layer presentation state the tile revisions cannot express. Folder
        // visibility cascades onto children (core::doc folders); a folder's
        // opacity applies once, at its group blit.
        let vis = doc.effective_visibility();
        let bases = doc.clip_bases();
        // LP-022 page half: the mono preview forces every layer's expression.
        let mono = self.mono_preview;
        let sig: Vec<LayerSig> = doc
            .layers
            .iter()
            .zip(&vis)
            .enumerate()
            .map(|(li, (l, v))| LayerSig {
                visible: *v,
                opacity: l.opacity.to_bits(),
                blend: blend_slot(l.blend),
                depth: l.depth,
                folder: l.folder,
                clip: l.clip,
                tone: l.tone.map(|t| t.sig()),
                edge: l.edge.map(|e| e.sig()),
                tint: l.layer_colour.map_or(crate::TINT_NONE, crate::tint_pack),
                fx: crate::fx_pack(
                    l.layer_sub_colour,
                    if mono {
                        mn_core::LayerExpression::Mono
                    } else {
                        l.expression
                    },
                ),
                spill: {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    l.escape_frame.hash(&mut h);
                    l.draws_over.hash(&mut h);
                    // The resolved seat, not just the wish: a structural edit
                    // elsewhere can move it with this layer untouched.
                    doc.spill_anchor(li).hash(&mut h);
                    l.breakout_mask().map(|m| m.revision).hash(&mut h);
                    h.finish()
                },
            })
            .collect();
        if sig != self.layer_sig {
            self.canvas_dirty_all = true;
            self.layer_sig = sig;
        }

        // PA-001: the paper is the bottom of every damaged region, so a new
        // colour (or the eye flipping) invalidates the whole canvas — no tile
        // revision moved, and nothing else would notice.
        let paper = self.paper_override.unwrap_or(doc.paper);
        if paper != self.paper_sig {
            self.canvas_dirty_all = true;
            self.paper_sig = paper;
        }

        // --- upload ---------------------------------------------------------
        // Invisible layers are uploaded too: toggling one back on must not need
        // a texture upload pass, and the sig check above already forces the
        // redraw. Frame-folder coverage masks upload like pixels, keyed apart.
        let mut damaged: std::collections::BTreeSet<TileIdx> = Default::default();
        let mut present: std::collections::HashSet<TileKey> = Default::default();
        let mut regions_all: std::collections::BTreeSet<TileIdx> = Default::default();
        let mut uploads: u32 = 0;
        // Tiles waiting on a staging buffer, flushed every `UPLOAD_BATCH` and
        // once more when the walk ends. Holds texture handles (cheap clones)
        // and either a borrowed tile slice or a masked copy of one.
        let mut batch: Vec<(wgpu::Texture, Cow<[u16]>)> = Vec::with_capacity(UPLOAD_BATCH);

        for (li, layer) in doc.layers.iter().enumerate() {
            // FB-overflow mask cap: only a layer the shared walk really does
            // split needs the complement raster — `breakout_mask` alone would
            // also fire on a stale flag with no sealed frame folder above it.
            let cap = layer
                .breakout_mask()
                .filter(|_| doc.spill_anchor(li).is_some());
            let pixel_tiles = layer
                .display_tiles()
                .iter()
                .map(|(idx, t)| (*idx, t, TileVariant::Pixels));
            let mask_tiles = layer
                .mask_tiles()
                .into_iter()
                .flat_map(|m| m.iter().map(|(idx, t)| (*idx, t, TileVariant::Coverage)));
            let held_tiles = cap.into_iter().flat_map(|m| {
                layer
                    .display_tiles()
                    .iter()
                    .filter(|(idx, _)| m.tiles.contains_key(idx))
                    .map(|(idx, t)| (*idx, t, TileVariant::HeldIn))
            });
            for (idx, tile, variant) in pixel_tiles.chain(mask_tiles).chain(held_tiles) {
                let key: TileKey = (li, idx, variant);
                present.insert(key);
                if variant == TileVariant::Pixels && vis[li] && layer.opacity > 0.0 {
                    regions_all.insert(idx);
                }
                // Two independent freshness questions (this split IS the
                // round-31 fix): does the TEXTURE need this tile's pixels
                // (upload), and has the CANVAS composite shown this revision
                // yet (redraw)? A CPU edit bumps the tile revision, making
                // both stale together; the GPU dab path splits them —
                // mark_dab_tile_clean kills the upload while `canvas_shown`
                // stays behind, so the stroke region redraws from the
                // already-correct texture.
                let needs_upload = match self.tiles.get(&key) {
                    Some(c) => c.revision < tile.revision(),
                    None => true,
                };
                let needs_redraw = self
                    .canvas_shown
                    .get(&key)
                    .is_none_or(|r| *r < tile.revision());
                if needs_upload || needs_redraw {
                    damaged.insert(idx);
                }
                if !needs_upload {
                    continue;
                }

                let device = &self.device;
                let bgl = &self.tile_texture_bgl;
                let pool = &mut self.tile_pool;
                let entry = self.tiles.entry(key).or_insert_with(|| {
                    if let Some(mut t) = pool.pop() {
                        t.revision = 0;
                        return t;
                    }
                    make_tile_texture(device, bgl)
                });

                // LM-005: fold an enabled layer mask into the UPLOADED
                // pixels (no shader change; the texture then shows the
                // masked content, which is also what the smudge oracle
                // samples — noted in the round-64 handoff). Mask edits go
                // through invalidate() (full rebuild), so the content
                // revision alone still gates this upload correctly.
                //
                // `Cow` because the common case has no mask: those pixels go
                // to the staging buffer straight from the tile, and the copy
                // it used to make (964 of them on a page open) is gone.
                // The HeldIn variant is the same fold against ONE MINUS the
                // coverage — the exact complement the CPU compositor applies,
                // so the two halves of a capped spill never double-blend.
                let fold = |md: &[u16], invert: bool| -> Vec<u16> {
                    let td = tile.data();
                    let mut out = vec![0u16; td.len()];
                    for p in 0..td.len() / 4 {
                        let a = (md[p * 4 + 3] as u32).min(32768);
                        let cov = if invert { 32768 - a } else { a };
                        for c in 0..4 {
                            out[p * 4 + c] = (td[p * 4 + c] as u32 * cov / 32768) as u16;
                        }
                    }
                    out
                };
                let upload: Cow<[u16]> = match variant {
                    TileVariant::Coverage => Cow::Borrowed(tile.data()),
                    TileVariant::HeldIn => match cap.and_then(|m| m.tiles.get(&idx)) {
                        Some(mt) => Cow::Owned(fold(mt.data(), true)),
                        // Unreachable: `held_tiles` only yields covered tiles.
                        None => Cow::Borrowed(tile.data()),
                    },
                    TileVariant::Pixels => layer
                        .mask
                        .as_ref()
                        .filter(|m| m.enabled)
                        .and_then(|m| m.tiles.get(&idx))
                        .map(|mt| Cow::Owned(fold(mt.data(), false)))
                        .unwrap_or(Cow::Borrowed(tile.data())),
                };
                batch.push((entry.texture.clone(), upload));
                entry.revision = tile.revision();
                uploads += 1;
                if batch.len() >= UPLOAD_BATCH {
                    flush_tile_uploads(&self.device, &self.queue, &mut batch);
                }
            }
        }
        flush_tile_uploads(&self.device, &self.queue, &mut batch);
        // Everything up to here — canvas sizing, the layer walk, the tile
        // uploads — so the batching above stays honest. Reported under
        // MN_DEBUG_PASSES below, next to the composite script it precedes.
        let upload_ms = t0.elapsed().as_secs_f32() * 1000.0;

        // --- evict tiles that no longer exist --------------------------------
        // Undo can delete a tile and layer removal can delete a whole layer's
        // worth; neither shows up as a revision bump, so compare key sets.
        // Evicted textures go to the pool, not to the driver (see tile_pool).
        // A live GPU dab stroke pins its touched tiles: they hold stroke
        // pixels the CPU-side `present` set cannot know about yet (BYPASS
        // records without rasterizing), and the per-flush dispatches are not
        // replayable — eviction would lose dabs.
        if let Some(st) = self.dabs.as_ref().and_then(|d| d.stroke.as_ref()) {
            for idx in &st.touched {
                present.insert((st.layer, *idx, TileVariant::Pixels));
                // The live regions stay damaged for the WHOLE stroke, derived
                // from stroke state: BYPASS freezes the CPU tiles mid-stroke,
                // so no revision bumps while flushes write the textures ahead.
                // (This replaces the one-shot `gpu_dab_dirty` the flush used
                // to extend — a set the first consumer CLEARED, after which
                // later composites of the same canvas went stale.)
                damaged.insert(*idx);
                // A full rebuild draws `regions_all`, which is derived from
                // CPU tiles — without this, a mid-stroke full composite (sig
                // change, resize) would skip the flush-only tiles entirely.
                if st.layer < vis.len() && vis[st.layer] {
                    regions_all.insert(*idx);
                }
            }
        }
        let before = self.tiles.len();
        if before != present.len() {
            let pool = &mut self.tile_pool;
            self.tiles.retain(|k, t| {
                let keep = present.contains(k);
                if !keep && pool.len() < TILE_POOL_CAP {
                    pool.push(CachedTile {
                        texture: t.texture.clone(),
                        bind_group: t.bind_group.clone(),
                        revision: 0,
                    });
                }
                keep
            });
            if self.tiles.len() != before {
                self.canvas_dirty_all = true;
            }
        }

        // GPU-dab regions need no special casing here: their damage falls out
        // of the `canvas_shown` compare above (mark_dab_tile_clean keeps the
        // texture fresh without touching this side) and the live-stroke block.

        let full = self.canvas_dirty_all;
        let regions: Vec<TileIdx> = if full {
            regions_all.into_iter().collect()
        } else {
            damaged.into_iter().collect()
        };
        if regions.is_empty() && !full {
            // An upload implies damage implies a region, so uploads == 0 here
            // and the zeroed stats above are already the truth.
            return;
        }
        // Content is about to change: the mip chain regenerates at present.
        self.mips_dirty = true;

        let uni = CanvasUniform {
            size: [doc.size.0 as f32, doc.size.1 as f32],
            _pad: [0.0, 0.0],
        };
        self.queue
            .write_buffer(&self.tile_uniform_buf, 0, bytemuck::bytes_of(&uni));

        // Isolation buffers: one per nesting level actually used this frame.
        let max_level = doc
            .layers
            .iter()
            .map(|l| l.depth as usize + usize::from(l.folder || l.clip) + 1)
            .max()
            .unwrap_or(1);
        self.ensure_groups(doc, max_level);
        // Clip-to-folder: which folder headers serve as a clip base this
        // frame (visibility does not matter here — a hidden folder base
        // still needs the zero-coverage path below to agree with the CPU).
        let folder_base: Vec<bool> = {
            let mut fb = vec![false; doc.layers.len()];
            for b in bases.iter().flatten() {
                if doc.layers[*b].folder {
                    fb[*b] = true;
                }
            }
            fb
        };
        if folder_base.iter().any(|&b| b) {
            self.ensure_clip_base(doc);
        }
        // LF-002 Through: real depth → effective target depth, the same
        // collapse core's composite computes. A through-folder maps its
        // child depth onto its own effective depth (children blend as if
        // loose); a normal folder seals one level deeper.
        let mut collapse: Vec<usize> = (0..=max_level + 1).collect();
        for l in &doc.layers {
            if l.folder {
                let e = collapse[l.depth as usize];
                collapse[l.depth as usize + 1] = if l.through { e } else { e + 1 };
            }
        }

        // --- build the composite script --------------------------------------
        //
        // Layers at depth d draw into target level d (0 = canvas). A folder
        // header at depth d multiplies level d+1 by its coverage mask (frame
        // folders), blits it onto level d with the folder's opacity/blend,
        // then draws its own raster (the border ink). A clip layer draws into
        // a scratch level, multiplies by its base layer's alpha, and blits
        // with its own opacity/blend. Every target switch is a render pass; a
        // group's first pass after being consumed clears it.
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Target {
            Canvas,
            Group(usize),
            /// The clip-to-folder capture texture — only ever the target of a
            /// clear-only pass (an empty folder base zeroes it; a live one is
            /// filled by `capture_base`'s encoder copy instead).
            ClipBase,
        }
        #[derive(Clone, Copy)]
        enum DrawKind {
            /// Paper-white reset quad (canvas, incremental mode only).
            Reset,
            /// A cached tile through the blend pipeline for this target.
            Tile(TileKey),
            /// Coverage multiply: dst *= alpha of this tile (`None` = the
            /// zero-initialised dummy — zero coverage).
            Mask(Option<TileKey>),
            /// Coverage multiply from the canvas-sized clip-base capture
            /// (clip-to-folder: the base alpha is a whole group, not tiles).
            MaskBase,
            /// Blend group `level` onto this target.
            Blit(usize),
        }
        struct Draw {
            instance: u32,
            blend: usize,
            kind: DrawKind,
        }
        struct Pass {
            target: Target,
            clear: Option<wgpu::Color>,
            draws: Vec<Draw>,
            /// Blend part 2: copy the target into the snapshot texture
            /// before this pass begins — the shader compositor needs the
            /// destination and a pass cannot read its own target. Snapshot
            /// passes never merge with neighbours.
            snapshot: bool,
            /// Clip-to-folder: copy group `level` into the clip-base capture
            /// before this pass begins (same encoder-op timing as
            /// `snapshot`). Set on an empty pass pushed at the folder's
            /// close, while the group still holds the finished content.
            capture_base: Option<usize>,
        }

        let mut passes: Vec<Pass> = Vec::new();
        let mut instances: Vec<QuadInstance> = Vec::new();
        let mut needs_clear = vec![true; max_level + 1];
        let mut drew_into = vec![false; max_level + 1];
        let mut first_canvas = true;

        let rect_of = |idx: &TileIdx| {
            let (ox, oy) = idx.origin();
            [ox as f32, oy as f32, TILE_SIZE as f32, TILE_SIZE as f32]
        };
        let target_of = |depth: usize| {
            if depth == 0 {
                Target::Canvas
            } else {
                Target::Group(depth)
            }
        };

        let never_reuse = std::env::var("MN_SPLIT_PASSES").is_ok();
        macro_rules! open_pass {
            ($target:expr) => {{
                let t = $target;
                let reuse = !never_reuse
                    && matches!(passes.last(), Some(p) if p.target == t && !p.snapshot)
                    && match t {
                        Target::Canvas => true,
                        Target::Group(l) => !needs_clear[l],
                        Target::ClipBase => false,
                    };
                if !reuse {
                    let clear = match t {
                        Target::Canvas => {
                            let c = if first_canvas && full {
                                Some(paper_clear(paper))
                            } else {
                                None
                            };
                            c
                        }
                        Target::Group(l) => {
                            let c = if needs_clear[l] { Some(TRANSPARENT) } else { None };
                            needs_clear[l] = false;
                            c
                        }
                        Target::ClipBase => Some(TRANSPARENT),
                    };
                    if t == Target::Canvas {
                        first_canvas = false;
                    }
                    passes.push(Pass {
                        target: t,
                        clear,
                        draws: Vec::new(),
                        snapshot: false,
                        capture_base: None,
                    });
                }
                passes.last_mut().unwrap()
            }};
        }
        // A blend2 pass: NEVER reuses (each snapshot must see the target as
        // of exactly this point in the layer order) and never gets appended
        // to by later normal draws reusing it as "last pass" — the reuse
        // check above rejects snapshot passes. It CAN absorb further draws
        // pushed through open_snap_pass! itself (same exotic layer), which
        // is correct: they all read the same snapshot.
        macro_rules! open_snap_pass {
            ($target:expr) => {{
                let t = $target;
                if !matches!(passes.last(), Some(p) if p.target == t && p.snapshot) {
                    let clear = match t {
                        Target::Canvas => {
                            let c = if first_canvas && full {
                                Some(paper_clear(paper))
                            } else {
                                None
                            };
                            c
                        }
                        Target::Group(l) => {
                            let c = if needs_clear[l] { Some(TRANSPARENT) } else { None };
                            needs_clear[l] = false;
                            c
                        }
                        Target::ClipBase => Some(TRANSPARENT),
                    };
                    if t == Target::Canvas {
                        first_canvas = false;
                    }
                    passes.push(Pass {
                        target: t,
                        clear,
                        draws: Vec::new(),
                        snapshot: true,
                        capture_base: None,
                    });
                }
                passes.last_mut().unwrap()
            }};
        }

        // Incremental mode: damaged canvas regions reset to paper first.
        if !full {
            let pass = open_pass!(Target::Canvas);
            for idx in &regions {
                pass.draws.push(Draw {
                    instance: instances.len() as u32,
                    blend: 0,
                    kind: DrawKind::Reset,
                });
                instances.push(QuadInstance {
                    // PA-001: the reset quad IS the paper, so it carries the
                    // paper colour in the tint slot and the paper's eye in
                    // the opacity slot (0 = hidden, and the replace blend
                    // state makes that a real hole rather than a no-op).
                    tint: crate::tint_pack(paper.colour),
                    // ...and no layer effect: the paper is not a layer.
                    fx: crate::FX_NONE,
                    rect: rect_of(idx),
                    mode: 0,
                    opacity: if paper.visible { 1.0 } else { 0.0 },
                    blend_mode: 0,
                });
            }
        } else {
            // Full mode: the first canvas pass clears to paper even when the
            // bottom of the stack lives in a group.
            open_pass!(Target::Canvas);
        }

        // FB-overflow: the SHARED walk — escaped layers re-seat above their
        // anchor at the anchor's depth, and a mask-capped one appears TWICE
        // (the halves differ only in which texture variant they sample).
        // Disagreeing with the CPU compositor here is a parity break.
        for step in doc.composite_order() {
            let li = step.layer;
            let layer = &doc.layers[li];
            if !vis[li] {
                continue;
            }
            let d = step.depth as usize;
            let variant = match step.part {
                mn_core::SpillPart::In => TileVariant::HeldIn,
                _ => TileVariant::Pixels,
            };
            // LF-002 Through: same collapse mapping as core's composite —
            // a through-folder's children draw into the folder's own
            // effective target (as if loose); normal folders seal.
            let cd = collapse[d];

            if layer.folder {
                if layer.through {
                    // No seal: no group close, no mask clip, no group blit.
                    // The header's own raster (border ink) still draws.
                    if layer.opacity > 0.0 && layer.tile_count() > 0 {
                        let pass = open_pass!(target_of(cd));
                        for idx in &regions {
                            if layer.tile(*idx).is_none() {
                                continue;
                            }
                            pass.draws.push(Draw {
                                instance: instances.len() as u32,
                                blend: 0,
                                kind: DrawKind::Tile((li, *idx, TileVariant::Pixels)),
                            });
                            instances.push(QuadInstance {
                                tint: crate::TINT_NONE,
                                fx: crate::FX_NONE,
                                rect: rect_of(idx),
                                mode: 1,
                                opacity: layer.opacity.clamp(0.0, 1.0),
                                blend_mode: 0,
                            });
                        }
                        if cd > 0 {
                            drew_into[cd] = true;
                        }
                    }
                    continue;
                }
                let lvl = cd + 1;
                // FB-knockout: the folder's derived mat lies on the page
                // just beneath the group — drawn into the parent target
                // BEFORE the group blit, scaled by the folder's opacity
                // (mirrors the CPU compositor's step 0). The mat IS this
                // folder's display raster, so the Pixels texture
                // key already holds it.
                if layer.edge.is_some() && layer.opacity > 0.0 {
                    let mat_tiles: Vec<TileIdx> = layer
                        .edge_tiles()
                        .map(|m| {
                            regions
                                .iter()
                                .copied()
                                .filter(|idx| m.contains_key(idx))
                                .collect()
                        })
                        .unwrap_or_default();
                    if !mat_tiles.is_empty() {
                        let pass = open_pass!(target_of(cd));
                        for idx in &mat_tiles {
                            pass.draws.push(Draw {
                                instance: instances.len() as u32,
                                blend: 0,
                                kind: DrawKind::Tile((li, *idx, TileVariant::Pixels)),
                            });
                            instances.push(QuadInstance {
                                tint: crate::TINT_NONE,
                                fx: crate::FX_NONE,
                                rect: rect_of(idx),
                                mode: 1,
                                opacity: layer.opacity.clamp(0.0, 1.0),
                                blend_mode: 0,
                            });
                        }
                        if d > 0 {
                            drew_into[cd] = true;
                        }
                    }
                }
                if drew_into[lvl] {
                    if layer.mask_tiles().is_some() {
                        let pass = open_pass!(Target::Group(lvl));
                        for idx in &regions {
                            let key = (li, *idx, TileVariant::Coverage);
                            let bind = self.tiles.contains_key(&key).then_some(key);
                            pass.draws.push(Draw {
                                instance: instances.len() as u32,
                                blend: 0,
                                kind: DrawKind::Mask(bind),
                            });
                            instances.push(QuadInstance {
                                tint: crate::TINT_NONE,
                                fx: crate::FX_NONE,
                                rect: rect_of(idx),
                                mode: 1,
                                opacity: 1.0,
                                blend_mode: 0,
                            });
                        }
                    }
                    // Clip-to-folder: capture the finished group (frame mask
                    // applied, before opacity/blend — the raw-display-alpha
                    // rule layer bases follow) before anything reuses or
                    // clears this level. The copy runs before the pass that
                    // carries it; the pass itself may absorb the blit draws.
                    if folder_base[li] {
                        passes.push(Pass {
                            target: target_of(cd),
                            clear: None,
                            draws: Vec::new(),
                            snapshot: false,
                            capture_base: Some(lvl),
                        });
                    }
                    if layer.opacity > 0.0 {
                        let slot = blend_slot(layer.blend);
                        let pass = if slot >= BLEND2_BASE {
                            open_snap_pass!(target_of(cd))
                        } else {
                            open_pass!(target_of(cd))
                        };
                        for idx in &regions {
                            pass.draws.push(Draw {
                                instance: instances.len() as u32,
                                blend: slot,
                                kind: DrawKind::Blit(lvl),
                            });
                            instances.push(QuadInstance {
                                tint: crate::TINT_NONE,
                                fx: crate::FX_NONE,
                                rect: rect_of(idx),
                                mode: 1,
                                opacity: layer.opacity.clamp(0.0, 1.0),
                                blend_mode: slot as u32,
                            });
                        }
                        if d > 0 {
                            drew_into[cd] = true;
                        }
                    }
                    needs_clear[lvl] = true;
                    drew_into[lvl] = false;
                } else if folder_base[li] {
                    // Nothing drew into the group, so there is nothing to
                    // copy — and the group texture may hold stale content
                    // (it is cleared lazily). An empty folder base means
                    // zero ink: zero the capture with a clear-only pass.
                    passes.push(Pass {
                        target: Target::ClipBase,
                        clear: Some(TRANSPARENT),
                        draws: Vec::new(),
                        snapshot: false,
                        capture_base: None,
                    });
                }
                // The header's own raster (frame border ink), Normal blend.
                if layer.opacity > 0.0 && layer.tile_count() > 0 {
                    let pass = open_pass!(target_of(cd));
                    for idx in &regions {
                        if layer.tile(*idx).is_none() {
                            continue;
                        }
                        pass.draws.push(Draw {
                            instance: instances.len() as u32,
                            blend: 0,
                            kind: DrawKind::Tile((li, *idx, TileVariant::Pixels)),
                        });
                        instances.push(QuadInstance {
                            tint: crate::TINT_NONE,
                            fx: crate::FX_NONE,
                            rect: rect_of(idx),
                            mode: 1,
                            opacity: layer.opacity.clamp(0.0, 1.0),
                            blend_mode: 0,
                        });
                    }
                    if d > 0 {
                        drew_into[cd] = true;
                    }
                }
                continue;
            }

            // The tile_count guard is gone: a live GPU dab stroke can hold
            // flush-only textures for a layer whose CPU tile count is still
            // zero (BYPASS) — the per-region check below and the `any` flag
            // make the empty case a no-op either way.
            if layer.opacity <= 0.0 {
                continue;
            }

            if let Some(base) = bases[li] {
                // Clip layer: scratch = layer pixels × base alpha, then blit.
                let lvl = cd + 1;
                let touched: Vec<TileIdx> = regions
                    .iter()
                    .copied()
                    .filter(|idx| match variant {
                        // The held-in half only exists where the cap mask
                        // does; elsewhere the whole tile went out.
                        TileVariant::HeldIn => self.tiles.contains_key(&(li, *idx, variant)),
                        _ => layer.tile(*idx).is_some(),
                    })
                    .collect();
                if touched.is_empty() {
                    continue;
                }
                {
                    let pass = open_pass!(Target::Group(lvl));
                    for idx in &touched {
                        pass.draws.push(Draw {
                            instance: instances.len() as u32,
                            blend: 0,
                            kind: DrawKind::Tile((li, *idx, variant)),
                        });
                        instances.push(QuadInstance {
                            tint: crate::TINT_NONE,
                            fx: crate::FX_NONE,
                            rect: rect_of(idx),
                            mode: 1,
                            opacity: 1.0,
                            blend_mode: 0,
                        });
                    }
                    for idx in &touched {
                        // Clip-to-folder: a folder base masks from the
                        // canvas-sized capture; a hidden folder never
                        // composited its children, so its ink is zero
                        // coverage (the dummy tile), matching the CPU walk
                        // which skips hidden folders entirely.
                        let kind = if doc.layers[base].folder {
                            if vis[base] {
                                DrawKind::MaskBase
                            } else {
                                DrawKind::Mask(None)
                            }
                        } else {
                            let key = (base, *idx, TileVariant::Pixels);
                            DrawKind::Mask(self.tiles.contains_key(&key).then_some(key))
                        };
                        pass.draws.push(Draw {
                            instance: instances.len() as u32,
                            blend: 0,
                            kind,
                        });
                        instances.push(QuadInstance {
                            tint: crate::TINT_NONE,
                            fx: crate::FX_NONE,
                            rect: rect_of(idx),
                            mode: 1,
                            opacity: 1.0,
                            blend_mode: 0,
                        });
                    }
                }
                let slot = blend_slot(layer.blend);
                let pass = if slot >= BLEND2_BASE {
                    open_snap_pass!(target_of(cd))
                } else {
                    open_pass!(target_of(cd))
                };
                for idx in &touched {
                    pass.draws.push(Draw {
                        instance: instances.len() as u32,
                        blend: slot,
                        kind: DrawKind::Blit(lvl),
                    });
                    instances.push(QuadInstance {
                        tint: crate::TINT_NONE,
                        fx: crate::FX_NONE,
                        rect: rect_of(idx),
                        mode: 1,
                        opacity: layer.opacity.clamp(0.0, 1.0),
                        blend_mode: slot as u32,
                    });
                }
                if d > 0 {
                    drew_into[cd] = true;
                }
                needs_clear[lvl] = true;
                drew_into[lvl] = false;
                continue;
            }

            // Plain layer.
            let slot = blend_slot(layer.blend);
            let pass = if slot >= BLEND2_BASE {
                open_snap_pass!(target_of(cd))
            } else {
                open_pass!(target_of(cd))
            };
            let mut any = false;
            for idx in &regions {
                // DISPLAY tiles, not painted ones: the border effect (LP-002)
                // throws outline into tiles that hold no source pixels at
                // all, and those are exactly the tiles the upload loop above
                // uploaded.
                if variant == TileVariant::HeldIn {
                    // The held-in half exists only where the cap mask has a
                    // tile — everywhere else the whole tile spilled out.
                    if !self.tiles.contains_key(&(li, *idx, variant)) {
                        continue;
                    }
                } else if layer.display_tile(*idx).is_none() {
                    // A GPU dab flush may have materialised this tile's
                    // texture before any CPU tile exists (BYPASS, live
                    // preview) — the composite samples the TEXTURE, so a
                    // cached entry is drawable too.
                    if !self.tiles.contains_key(&(li, *idx, variant)) {
                        continue;
                    }
                }
                any = true;
                pass.draws.push(Draw {
                    instance: instances.len() as u32,
                    blend: slot,
                    kind: DrawKind::Tile((li, *idx, variant)),
                });
                instances.push(QuadInstance {
                    // LP-016/017/022: the plain-layer draw is the only one
                    // carrying per-layer display maths. Group blits must NOT
                    // re-apply them — their content is already tinted.
                    tint: layer
                        .layer_colour
                        .map_or(crate::TINT_NONE, crate::tint_pack),
                    fx: crate::fx_pack(
                        layer.layer_sub_colour,
                        if self.mono_preview {
                            mn_core::LayerExpression::Mono
                        } else {
                            layer.expression
                        },
                    ),
                    rect: rect_of(idx),
                    mode: 1,
                    opacity: layer.opacity.clamp(0.0, 1.0),
                    blend_mode: slot as u32,
                });
            }
            if any && cd > 0 {
                drew_into[cd] = true;
            }
        }

        self.ensure_instance_capacity(instances.len().max(1));
        if !instances.is_empty() {
            self.queue
                .write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&instances));
        }

        // --- execute ---------------------------------------------------------
        if std::env::var("MN_DEBUG_PASSES").is_ok() {
            eprintln!(
                "[gpu] script: full={} regions={} instances={} uploads={uploads} ({upload_ms:.1} ms)",
                full,
                regions.len(),
                instances.len()
            );
            for (i, p) in passes.iter().enumerate() {
                let t = match p.target {
                    Target::Canvas => "canvas".to_string(),
                    Target::Group(l) => format!("group{l}"),
                    Target::ClipBase => "clipbase".to_string(),
                };
                eprintln!(
                    "[gpu]   pass {i}: {t} clear={} draws={}",
                    p.clear.is_some(),
                    p.draws.len()
                );
            }
        }
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mn.canvas"),
            });
        for pass in &passes {
            // Clip-to-folder: capture the named group into the clip-base
            // texture before this pass (same encoder-op timing as the
            // snapshot copy below — queue order guarantees the copy sees
            // every prior pass, i.e. the finished group).
            if let (Some(lvl), Some(cb)) = (pass.capture_base, &self.clip_base) {
                enc.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.groups[lvl - 1].texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &cb.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: doc.size.0.max(1),
                        height: doc.size.1.max(1),
                        depth_or_array_layers: 1,
                    },
                );
            }
            // Blend part 2: snapshot the destination before a shader-composite
            // pass (copies are encoder ops — legal between passes, never in
            // one). Queue order guarantees the copy sees every prior pass.
            if pass.snapshot {
                let Some((snap_tex, _snap_view)) = &self.snap else {
                    continue;
                };
                let src = match pass.target {
                    Target::Canvas => &self.canvas.as_ref().unwrap().texture,
                    Target::Group(l) => &self.groups[l - 1].texture,
                    // Clear-only target; never a snapshot pass.
                    Target::ClipBase => unreachable!("clip-base pass is never blend2"),
                };
                enc.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: src,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: snap_tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: doc.size.0.max(1),
                        height: doc.size.1.max(1),
                        depth_or_array_layers: 1,
                    },
                );
            }
            let view = match pass.target {
                Target::Canvas => &canvas_view,
                Target::Group(l) => &self.groups[l - 1].view,
                Target::ClipBase => {
                    // Guaranteed by ensure_clip_base before the script built
                    // any ClipBase pass; skip defensively if it ever isn't.
                    match &self.clip_base {
                        Some(cb) => &cb.view,
                        None => continue,
                    }
                }
            };
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mn.canvas.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: match pass.clear {
                            Some(c) => wgpu::LoadOp::Clear(c),
                            None => wgpu::LoadOp::Load,
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_bind_group(0, &self.tile_uniform_bg, &[]);
            rp.set_vertex_buffer(0, self.instance_buf.slice(..));
            let on_canvas = pass.target == Target::Canvas;
            let (cw, ch) = (doc.size.0.max(1), doc.size.1.max(1));
            for d in &pass.draws {
                // Integer scissor does the clipping (see tiles.wgsl: quad
                // edges at NDC 0 misrasterize on the Intel DX12 driver).
                let r = instances[d.instance as usize].rect;
                let x = (r[0].max(0.0) as u32).min(cw - 1);
                let y = (r[1].max(0.0) as u32).min(ch - 1);
                let w = (r[2] as u32).min(cw - x).max(1);
                let h = (r[3] as u32).min(ch - y).max(1);
                rp.set_scissor_rect(x, y, w, h);
                match d.kind {
                    DrawKind::Reset => {
                        // PA-001: replace, not src-over — see `blend_replace`.
                        rp.set_pipeline(&self.tile_pipeline_reset);
                        rp.set_bind_group(1, &self.dummy_tile_bg, &[]);
                    }
                    DrawKind::Tile(key) if d.blend >= BLEND2_BASE => {
                        // Blend part 2: shader composite against the snapshot.
                        // Bind group built per draw (tile view + snap view —
                        // a bg must fill its whole layout; exotic layers are
                        // rare, so this is not a hot path).
                        let Some(cached) = self.tiles.get(&key) else {
                            eprintln!("[gpu] MISSING tile cache entry {key:?}");
                            continue;
                        };
                        let Some((_, snap_view)) = &self.snap else {
                            continue;
                        };
                        let tile_view = cached
                            .texture
                            .create_view(&wgpu::TextureViewDescriptor::default());
                        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("mn.blend2.draw.bg"),
                            layout: &self.blend2_tile_bgl,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(&tile_view),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::TextureView(snap_view),
                                },
                            ],
                        });
                        rp.set_pipeline(&self.blend2_tile_pipe);
                        rp.set_bind_group(1, &bg, &[]);
                    }
                    DrawKind::Tile(key) => {
                        let pipes = if on_canvas {
                            &self.tile_pipelines
                        } else {
                            &self.tile_pipelines_group
                        };
                        rp.set_pipeline(&pipes[d.blend]);
                        let Some(cached) = self.tiles.get(&key) else {
                            eprintln!("[gpu] MISSING tile cache entry {key:?}");
                            continue;
                        };
                        rp.set_bind_group(1, &cached.bind_group, &[]);
                    }
                    DrawKind::Mask(key) => {
                        rp.set_pipeline(&self.mask_pipeline);
                        match key.and_then(|k| self.tiles.get(&k)) {
                            Some(cached) => rp.set_bind_group(1, &cached.bind_group, &[]),
                            None => rp.set_bind_group(1, &self.dummy_tile_bg, &[]),
                        }
                    }
                    DrawKind::MaskBase => {
                        // ensure_clip_base ran before the script emitted this.
                        let Some(cb) = &self.clip_base else {
                            eprintln!("[gpu] MISSING clip-base capture");
                            continue;
                        };
                        rp.set_pipeline(&self.mask_base_pipeline);
                        rp.set_bind_group(1, &cb.blit_bg, &[]);
                    }
                    DrawKind::Blit(level) if d.blend >= BLEND2_BASE => {
                        let Some((_, snap_view)) = &self.snap else {
                            continue;
                        };
                        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("mn.blend2.blitdraw.bg"),
                            layout: &self.blend2_blit_bgl,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(
                                        &self.groups[level - 1].view,
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 2,
                                    resource: wgpu::BindingResource::TextureView(snap_view),
                                },
                            ],
                        });
                        rp.set_pipeline(&self.blend2_blit_pipe);
                        rp.set_bind_group(1, &bg, &[]);
                    }
                    DrawKind::Blit(level) => {
                        let pipes = if on_canvas {
                            &self.blit_pipelines
                        } else {
                            &self.blit_pipelines_group
                        };
                        rp.set_pipeline(&pipes[d.blend]);
                        rp.set_bind_group(1, &self.groups[level - 1].blit_bg, &[]);
                    }
                }
                rp.draw(0..3, d.instance..d.instance + 1);
            }
        }
        self.queue.submit([enc.finish()]);

        // Publish canvas-side freshness: the composite just executed, so the
        // canvas reflects every present tile at its CURRENT revision. A full
        // rebuild clears first — stale entries from a previous document or
        // layer layout must never suppress a redraw (the map is keyed by
        // layer INDEX, which is only unique within one document). Incremental
        // composites rewrite all present keys: an unchanged key is by
        // definition not damaged, so this only drift-heals entries whose
        // recorded revision was above the current one (possible only after a
        // document switch, which forces a full rebuild anyway).
        if full {
            self.canvas_shown.clear();
        }
        for (li, layer) in doc.layers.iter().enumerate() {
            for (idx, t) in layer.display_tiles() {
                self.canvas_shown.insert((li, *idx, TileVariant::Pixels), t.revision());
                // FB-overflow mask cap: the held-in half rides the same
                // source revision. Without its own entry the key would read
                // as never-shown and keep its region damaged every frame.
                if self.tiles.contains_key(&(li, *idx, TileVariant::HeldIn)) {
                    self.canvas_shown
                        .insert((li, *idx, TileVariant::HeldIn), t.revision());
                }
            }
            if let Some(masks) = layer.mask_tiles() {
                for (idx, t) in masks.iter() {
                    self.canvas_shown
                        .insert((li, *idx, TileVariant::Coverage), t.revision());
                }
            }
        }
        self.canvas_dirty_all = false;
        self.frame_stats = FrameStats {
            uploads,
            composite_tiles: regions.len() as u32,
            full,
            ms: t0.elapsed().as_secs_f32() * 1000.0,
        };
    }

    /// Make sure isolation buffers exist for levels `1..=max_level` at the
    /// canvas size (they are dropped with the canvas on resize).
    fn ensure_groups(&mut self, doc: &Document, max_level: usize) {
        while self.groups.len() < max_level {
            let size = (doc.size.0.max(1), doc.size.1.max(1));
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mn.group"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: GROUP_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let blit_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mn.group.blit.bg"),
                layout: &self.blit_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.groups.push(GroupTex {
                texture,
                view,
                blit_bg,
            });
        }
    }

    fn ensure_canvas(&mut self, doc: &Document) {
        let size = (doc.size.0.max(1), doc.size.1.max(1));
        if self.canvas.as_ref().map(|c| c.size) == Some(size) {
            return;
        }
        // Full mip chain: the zoomed-out view samples an area average.
        let mip_levels = 32 - size.0.max(size.1).leading_zeros();
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mn.canvas"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CANVAS_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        // Level 0 alone is the compositor's render target; the present pass
        // binds the whole chain.
        let level_view = |lvl: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: lvl,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let view = level_view(0);
        let full_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut mip_chain = Vec::new();
        for lvl in 1..mip_levels {
            let src = level_view(lvl - 1);
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mn.mip.bg"),
                layout: &self.blit_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            mip_chain.push((level_view(lvl), bg));
        }
        let present_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mn.present.bg"),
            layout: &self.present_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.present_uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&full_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.canvas = Some(Canvas {
            texture,
            view,
            size,
            present_bg,
            mip_chain,
        });
        self.mips_dirty = true;
        // The blend2 destination snapshot (canvas-sized; copied between
        // passes whenever a part-2-mode layer composites). Bind groups are
        // built per draw at execution — a bg must fill its whole layout, and
        // the tile/group half changes per draw anyway.
        let snap = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mn.blend2.snap"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: GROUP_FORMAT,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let snap_view = snap.create_view(&wgpu::TextureViewDescriptor::default());
        self.snap = Some((snap, snap_view));
        // Isolation buffers are canvas-sized; rebuild them lazily at the new
        // size.
        self.groups.clear();
        self.clip_base = None;
        self.canvas_dirty_all = true;
    }

    /// Make sure the clip-to-folder base capture exists at the canvas size.
    /// Called only when the frame actually has a folder serving as a clip
    /// base, so documents without the feature never pay for the texture.
    fn ensure_clip_base(&mut self, doc: &Document) {
        if self.clip_base.is_some() {
            return;
        }
        let size = (doc.size.0.max(1), doc.size.1.max(1));
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mn.clipbase"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: GROUP_FORMAT,
            // COPY_DST for the group capture; RENDER_ATTACHMENT for the
            // zero-clear pass an empty folder base needs.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let blit_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mn.clipbase.bg"),
            layout: &self.blit_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.clip_base = Some(GroupTex {
            texture,
            view,
            blit_bg,
        });
    }

    fn ensure_instance_capacity(&mut self, needed: usize) {
        if needed <= self.instance_cap {
            return;
        }
        let cap = needed.next_power_of_two();
        self.instance_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mn.instances"),
            size: (cap * std::mem::size_of::<QuadInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_cap = cap;
    }
}

/// Default backend set.
///
/// **DX12 only, deliberately.** `Backends::PRIMARY` also enables Vulkan, and on
/// this laptop's Intel UHD 620 Vulkan driver `request_device` dies with a hard
/// `STATUS_ACCESS_VIOLATION` (reproduced 2026-08-13; the identical document
/// renders byte-identically on Dx12 hardware and on Dx12 WARP). This is a
/// Windows-only app, the software fallback is DX12 WARP anyway, and DX12 exists
/// on every Windows 10+ machine. `WGPU_BACKEND=vulkan` still overrides for the
/// home PC.
const DEFAULT_BACKENDS: wgpu::Backends = wgpu::Backends::DX12;

fn new_instance() -> wgpu::Instance {
    // `InstanceDescriptor` is not `Default` in wgpu 30 (it holds a boxed display
    // handle), so spell the fields out.
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::from_env().unwrap_or(DEFAULT_BACKENDS),
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::from_env_or_default(),
        display: None,
    })
}

/// Adapter policy from docs/ARCHITECTURE.md: ask for HighPerformance first, and
/// on failure retry with `force_fallback_adapter` (DX12 WARP on Windows) so the
/// no-GPU laptop runs the exact same code path minus the fast bits.
fn request_gpu(
    instance: &wgpu::Instance,
    compatible_surface: Option<&wgpu::Surface<'static>>,
    cfg: GpuConfig,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), GpuError> {
    let ask = |fallback: bool| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: fallback,
            compatible_surface,
            ..Default::default()
        }))
    };

    let adapter = if cfg.force_fallback {
        println!("[gpu] --warp: requesting software (fallback) adapter directly");
        ask(true).map_err(|e| GpuError(format!("no fallback adapter: {e}")))?
    } else {
        match ask(false) {
            Ok(a) => a,
            Err(e) => {
                println!("[gpu] no hardware adapter ({e}); retrying force_fallback_adapter");
                ask(true)
                    .map_err(|e2| GpuError(format!("no adapter (hw: {e}) (fallback: {e2})")))?
            }
        }
    };

    let info = adapter.get_info();
    // Log-only twin of `adapter_line` (which cannot be called before the
    // Renderer exists) — same match, same rule: no trailing space when
    // driver_info is empty. That shape caused the fingerprint bug.
    let driver = match (info.driver.trim(), info.driver_info.trim()) {
        ("", "") => "unknown".to_string(),
        (d, "") => d.to_string(),
        ("", i) => i.to_string(),
        (d, i) => format!("{d} {i}"),
    };
    println!(
        "[gpu] adapter: {} | backend {:?} | type {:?} | driver {driver}",
        info.name, info.backend, info.device_type
    );

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("mn.device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .map_err(|e| GpuError(format!("request_device: {e}")))?;

    device.on_uncaptured_error(std::sync::Arc::new(|e| {
        eprintln!("[gpu] uncaptured error: {e}")
    }));
    // A lost device fails SILENTLY otherwise: every later create_* hands
    // back an invalid resource and the first visible symptom is a baffling
    // "buffer is invalid" far from the cause (seen on the 19041 WARP).
    device.set_device_lost_callback(|reason, msg| {
        eprintln!("[gpu] DEVICE LOST ({reason:?}): {msg}");
    });

    Ok((adapter, device, queue))
}

/// sRGB EOTF (encoded -> linear), matching `srgb_to_linear` in present.wgsl.
/// Used for clear colours, which the hardware encodes just like shader output.
fn srgb_decode_color(c: wgpu::Color) -> wgpu::Color {
    fn ch(v: f64) -> f64 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    wgpu::Color {
        r: ch(c.r),
        g: ch(c.g),
        b: ch(c.b),
        a: c.a,
    }
}

/// Prefer a non-sRGB swapchain format: the canvas is authored in plain unorm and
/// the present shader does no colour conversion, so an sRGB view would double-
/// encode. When only sRGB formats are offered we take one and the present shader
/// pre-decodes instead (`PresentUniform::flags.x`).
fn pick_surface_format(caps: &wgpu::SurfaceCapabilities) -> wgpu::TextureFormat {
    caps.formats
        .iter()
        .copied()
        .find(|f| !f.is_srgb())
        .or_else(|| caps.formats.first().copied())
        .unwrap_or(wgpu::TextureFormat::Bgra8Unorm)
}

fn read_texture_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    w: u32,
    h: u32,
) -> image::RgbaImage {
    const ALIGN: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let unpadded = w * 4;
    let padded = unpadded.div_ceil(ALIGN) * ALIGN;

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mn.readback"),
        size: (padded * h) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("mn.readback"),
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);

    let slice = buffer.slice(..);
    // Keep the map result: swallowing it (`|_| {}`) turns a real WARP/device
    // error into a baffling "buffer is invalid" at get_mapped_range.
    let map_result = std::sync::Arc::new(std::sync::Mutex::new(None));
    let map_result_cb = map_result.clone();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        *map_result_cb.lock().unwrap() = Some(r);
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    if let Some(Err(e)) = map_result.lock().unwrap().take() {
        panic!("map readback buffer: map_async failed: {e:?}");
    }

    let mut out = image::RgbaImage::new(w, h);
    {
        let view = slice.get_mapped_range().expect("map readback buffer");
        for y in 0..h {
            let src = (y * padded) as usize;
            let dst = (y * unpadded) as usize;
            out.as_mut()[dst..dst + unpadded as usize]
                .copy_from_slice(&view[src..src + unpadded as usize]);
        }
    }
    buffer.unmap();
    out
}

#[cfg(test)]
mod viewport_tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn close2(a: (f32, f32), b: (f32, f32)) -> bool {
        close(a.0, b.0) && close(a.1, b.1)
    }

    #[test]
    fn unrotated_transform_is_unchanged() {
        // Regression guard: adding rotation must not move anything at 0 rad.
        let vp = Viewport {
            pan: [30.0, -12.0],
            zoom: 2.5,
            ..Default::default()
        };
        for (cx, cy) in [(0.0, 0.0), (100.0, 40.0), (-7.0, 3.5)] {
            let s = vp.to_screen(cx, cy);
            assert!(close2(s, (cx * 2.5 + 30.0, cy * 2.5 - 12.0)), "{s:?}");
            assert!(close2(vp.to_canvas(s.0, s.1), (cx, cy)));
        }
    }

    #[test]
    fn quarter_turn_maps_x_axis_onto_y_axis() {
        // Screen space is y-down, so a positive angle turns clockwise on screen.
        let vp = Viewport {
            pan: [0.0, 0.0],
            zoom: 1.0,
            rotate_rad: FRAC_PI_2,
            ..Default::default()
        };
        assert!(close2(vp.to_screen(10.0, 0.0), (0.0, 10.0)));
        assert!(close2(vp.to_screen(0.0, 10.0), (-10.0, 0.0)));
        assert!(close2(vp.to_canvas(0.0, 10.0), (10.0, 0.0)));
    }

    #[test]
    fn rotation_composes_with_pan_and_zoom_and_inverts() {
        let vp = Viewport {
            pan: [640.0, 360.0],
            zoom: 3.0,
            rotate_rad: 0.7,
            ..Default::default()
        };
        for (cx, cy) in [(0.0, 0.0), (128.0, 64.0), (-40.0, 900.0)] {
            let s = vp.to_screen(cx, cy);
            assert!(
                close2(vp.to_canvas(s.0, s.1), (cx, cy)),
                "roundtrip {cx},{cy}"
            );
        }
        // Scale is applied before rotation, so distances scale by zoom exactly.
        let a = vp.to_screen(0.0, 0.0);
        let b = vp.to_screen(10.0, 0.0);
        let d = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        assert!(close(d, 30.0), "expected 10 px * zoom 3, got {d}");
    }

    #[test]
    fn rotate_and_zoom_around_pin_the_anchor() {
        let centre = [512.0f32, 384.0];
        let mut vp = Viewport {
            pan: [100.0, 50.0],
            zoom: 1.5,
            rotate_rad: 0.2,
            ..Default::default()
        };
        let before = vp.to_canvas(centre[0], centre[1]);

        vp.rotate_around(centre, 0.9);
        let after = vp.to_screen(before.0, before.1);
        assert!(
            close2(after, (centre[0], centre[1])),
            "rotate moved the anchor: {after:?}"
        );

        vp.zoom_around(centre, 2.0);
        let after = vp.to_screen(before.0, before.1);
        assert!(
            close2(after, (centre[0], centre[1])),
            "zoom moved the anchor: {after:?}"
        );
        assert!(close(vp.zoom, 3.0));

        // A different anchor point must still work.
        let corner = [0.0f32, 0.0];
        let pinned = vp.to_canvas(corner[0], corner[1]);
        vp.set_rotation_around(corner, -1.2);
        assert!(close2(vp.to_screen(pinned.0, pinned.1), (0.0, 0.0)));
        assert!(close(vp.rotate_rad, -1.2));
    }

    #[test]
    fn flip_mirrors_and_inverts_and_pins_its_anchor() {
        let mut vp = Viewport {
            pan: [200.0, 100.0],
            zoom: 2.0,
            rotate_rad: 0.3,
            ..Default::default()
        };
        let centre = [512.0f32, 384.0];
        let anchor = vp.to_canvas(centre[0], centre[1]);

        vp.flip_around(centre);
        assert!(vp.flip_h);
        // The anchor point stays put through the flip.
        let s = vp.to_screen(anchor.0, anchor.1);
        assert!(
            close2(s, (centre[0], centre[1])),
            "flip moved the anchor: {s:?}"
        );
        // Round-trips still hold while mirrored.
        for (cx, cy) in [(0.0, 0.0), (128.0, 64.0), (-40.0, 900.0)] {
            let sc = vp.to_screen(cx, cy);
            assert!(
                close2(vp.to_canvas(sc.0, sc.1), (cx, cy)),
                "flip roundtrip {cx},{cy}"
            );
        }
        // A point right of the anchor lands left of it on screen.
        let right = vp.to_screen(anchor.0 + 10.0, anchor.1);
        assert!(right.0 < centre[0], "mirror reverses x: {right:?}");

        // Flipping back restores the original transform.
        vp.flip_around(centre);
        assert!(!vp.flip_h);
        assert!(close(vp.rotate_rad, 0.3));
        let s = vp.to_screen(anchor.0, anchor.1);
        assert!(close2(s, (centre[0], centre[1])));
    }

    /// The vertical half of the flip (ROADMAP good-first-issue #1). Same
    /// shape as the horizontal test above, one axis over: round trips hold,
    /// the anchor stays put, DOWN becomes UP, and x is left alone.
    #[test]
    fn flip_v_mirrors_the_other_axis_and_pins_its_anchor() {
        let mut vp = Viewport {
            pan: [200.0, 100.0],
            zoom: 2.0,
            rotate_rad: 0.3,
            ..Default::default()
        };
        let centre = [512.0f32, 384.0];
        let anchor = vp.to_canvas(centre[0], centre[1]);

        vp.flip_v_around(centre);
        assert!(vp.flip_v && !vp.flip_h);
        let s = vp.to_screen(anchor.0, anchor.1);
        assert!(
            close2(s, (centre[0], centre[1])),
            "flip moved the anchor: {s:?}"
        );
        for (cx, cy) in [(0.0, 0.0), (128.0, 64.0), (-40.0, 900.0)] {
            let sc = vp.to_screen(cx, cy);
            assert!(
                close2(vp.to_canvas(sc.0, sc.1), (cx, cy)),
                "flip_v roundtrip {cx},{cy}"
            );
        }
        // Unrotated so the axes read straight: below the anchor lands above.
        let mut flat = Viewport {
            pan: [200.0, 100.0],
            zoom: 2.0,
            flip_v: true,
            ..Default::default()
        };
        let a = flat.to_screen(50.0, 50.0);
        let below = flat.to_screen(50.0, 60.0);
        assert!(below.1 < a.1, "mirror reverses y: {below:?} vs {a:?}");
        let right = flat.to_screen(60.0, 50.0);
        assert!(right.0 > a.0, "x is untouched: {right:?} vs {a:?}");

        // zoom_around still pins its anchor while flipped (the transform
        // helpers all go through to_canvas/to_screen).
        let pinned = flat.to_canvas(centre[0], centre[1]);
        flat.zoom_around(centre, 2.0);
        assert!(
            close2(flat.to_screen(pinned.0, pinned.1), (centre[0], centre[1])),
            "zoom_around moved the anchor under a vertical flip"
        );
        assert!(close(flat.zoom, 4.0));

        // Flipping back restores the original transform.
        vp.flip_v_around(centre);
        assert!(!vp.flip_v);
        assert!(close(vp.rotate_rad, 0.3));
        assert!(close2(
            vp.to_screen(anchor.0, anchor.1),
            (centre[0], centre[1])
        ));
    }

    /// H+V composed is a 180° POINT reflection, not a mirror: both axes
    /// reverse, round trips hold, and the view is not `mirrored()`.
    #[test]
    fn both_flips_compose_into_a_point_reflection() {
        let mut vp = Viewport {
            pan: [200.0, 100.0],
            zoom: 2.0,
            ..Default::default()
        };
        let centre = [512.0f32, 384.0];
        vp.flip_around(centre);
        vp.flip_v_around(centre);
        assert!(vp.flip_h && vp.flip_v);
        assert!(!vp.mirrored(), "two flips cancel the handedness reversal");

        let a = vp.to_screen(50.0, 50.0);
        for (dx, dy) in [(10.0f32, 0.0f32), (0.0, 10.0), (7.0, -3.0)] {
            let p = vp.to_screen(50.0 + dx, 50.0 + dy);
            let plain = Viewport {
                pan: vp.pan,
                zoom: vp.zoom,
                rotate_rad: vp.rotate_rad,
                ..Default::default()
            };
            let q = plain.to_screen(50.0, 50.0);
            let q2 = plain.to_screen(50.0 - dx, 50.0 - dy);
            assert!(
                close2((p.0 - a.0, p.1 - a.1), (q2.0 - q.0, q2.1 - q.1)),
                "H+V must negate the offset: {dx},{dy}"
            );
        }
        for (cx, cy) in [(0.0, 0.0), (128.0, 64.0), (-40.0, 900.0)] {
            let sc = vp.to_screen(cx, cy);
            assert!(
                close2(vp.to_canvas(sc.0, sc.1), (cx, cy)),
                "H+V roundtrip {cx},{cy}"
            );
        }
        let pinned = vp.to_canvas(centre[0], centre[1]);
        vp.zoom_around(centre, 0.5);
        assert!(
            close2(vp.to_screen(pinned.0, pinned.1), (centre[0], centre[1])),
            "zoom_around moved the anchor under H+V"
        );
    }

    /// `brush_view()` must hand patch #12 a `(rotation, mirror)` pair whose
    /// linear map IS the viewport's own — that equivalence is the whole
    /// reason the brush needs no vertical-flip flag of its own.
    #[test]
    fn brush_view_reproduces_the_real_transform() {
        for &rot in &[0.0f32, 0.3, -1.9, FRAC_PI_2] {
            for &(fh, fv) in &[(false, false), (true, false), (false, true), (true, true)] {
                let vp = Viewport {
                    pan: [17.0, -4.0],
                    zoom: 2.0,
                    rotate_rad: rot,
                    flip_h: fh,
                    flip_v: fv,
                };
                let (brot, bmirror) = vp.brush_view();
                // The pair, as a viewport the brush COULD have been given.
                let equiv = Viewport {
                    pan: vp.pan,
                    zoom: vp.zoom,
                    rotate_rad: brot,
                    flip_h: bmirror,
                    flip_v: false,
                };
                let o = vp.to_screen(0.0, 0.0);
                let oe = equiv.to_screen(0.0, 0.0);
                for (cx, cy) in [(10.0, 0.0), (0.0, 10.0), (-3.0, 7.0)] {
                    let d = vp.to_screen(cx, cy);
                    let de = equiv.to_screen(cx, cy);
                    assert!(
                        close2((d.0 - o.0, d.1 - o.1), (de.0 - oe.0, de.1 - oe.1)),
                        "brush_view diverges at rot {rot} flips {fh}/{fv}"
                    );
                }
                assert!(brot.abs() <= PI + 1e-6, "brush rotation stays wrapped");
            }
        }
    }

    #[test]
    fn angles_stay_wrapped() {
        let mut vp = Viewport::default();
        for _ in 0..20 {
            vp.rotate_around([0.0, 0.0], 1.0);
        }
        assert!(vp.rotate_rad.abs() <= PI + 1e-6, "{}", vp.rotate_rad);
        assert!(close(wrap_angle(3.0 * PI), PI) || close(wrap_angle(3.0 * PI), -PI));
        assert!(close(wrap_angle(0.5), 0.5));
    }

    #[test]
    fn fit_centres_the_page() {
        let vp = Viewport::fit((1000, 500), (500, 500));
        assert!(close(vp.zoom, 0.5));
        assert!(close2((vp.pan[0], vp.pan[1]), (0.0, 125.0)));
        // 1:1 when the target matches the document — what render_offscreen uses.
        let vp = Viewport::fit((256, 256), (256, 256));
        assert!(close(vp.zoom, 1.0));
        assert!(close2((vp.pan[0], vp.pan[1]), (0.0, 0.0)));
    }

    #[test]
    fn corners_follow_the_rotation() {
        let vp = Viewport {
            pan: [10.0, 20.0],
            zoom: 1.0,
            rotate_rad: FRAC_PI_2,
            ..Default::default()
        };
        let c = vp.corners_screen((100, 50));
        // top-left is the pan point whatever the rotation
        assert!(close2((c[0][0], c[0][1]), (10.0, 20.0)));
        // top-right (100,0) rotates to (0,100) + pan
        assert!(close2((c[1][0], c[1][1]), (10.0, 120.0)));
        // bottom-left (0,50) rotates to (-50,0) + pan
        assert!(close2((c[2][0], c[2][1]), (-40.0, 20.0)));
        assert!(close2((c[3][0], c[3][1]), (-40.0, 120.0)));
    }

    #[test]
    fn blend_slots_are_distinct_and_ordered() {
        let all = [
            mn_core::Blend::Normal,
            mn_core::Blend::Multiply,
            mn_core::Blend::Screen,
            mn_core::Blend::Add,
            // Subtract left slot 4 for blend2 (the transparent-dest fix);
            // the BLEND2_MODES loop below covers it now.
        ];
        for (i, b) in all.iter().enumerate() {
            assert_eq!(blend_slot(*b), i, "{b:?}");
        }
        // The part-2 family rides DISTINCT sentinel slots (16..) so LayerSig
        // sees blend changes; the blend2 shader consumes the same values.
        for (i, b) in BLEND2_MODES.iter().enumerate() {
            assert_eq!(blend_slot(*b), BLEND2_BASE + i, "{b:?}");
        }
    }

    #[test]
    fn srgb_decode_matches_the_shader() {
        let c = srgb_decode_color(wgpu::Color {
            r: 0.5,
            g: 0.04,
            b: 1.0,
            a: 1.0,
        });
        assert!((c.r - 0.2140).abs() < 1e-3, "{}", c.r);
        assert!((c.g - 0.04 / 12.92).abs() < 1e-6, "linear segment");
        assert!((c.b - 1.0).abs() < 1e-6);
        assert!((c.a - 1.0).abs() < 1e-9, "alpha is never encoded");
    }
}
