//! GPU dab compositing — the P1 compute path (docs/design/GPU-DABS.md).
//!
//! `dab.wgsl` rasterizes recorded dabs (`core::dab::DabParams`, vendor patch
//! #11) directly into the tile textures: one workgroup per dirty 64×64 tile,
//! each thread owns a 4×4 pixel block register-resident (rgba16uint has no
//! read_write storage, so the pass samples a scratch COPY of the tile and
//! writes the original — the design doc's register trick with a WebGPU-legal
//! load path). The app flushes drained records per frame; stroke end reads
//! the touched tiles back into the CPU tiles (the only stall, one per
//! stroke) and marks the texture cache clean so the compositor keeps the
//! GPU state — while the canvas-side `canvas_shown` record deliberately
//! stays behind, so the composite still redraws the stroke's regions.
//!
//! Cursed-driver defense: every dispatched workgroup atomically bumps a
//! canary; the stroke-end readback compares it with the expected dispatch
//! count — on mismatch the caller repairs by re-rasterizing on CPU (worst
//! case = today's speed, never corruption).

use mn_core::TileIdx;
use std::collections::BTreeSet;

/// One dab as `dab.wgsl` sees it: the C dispatch math (per-mode opacities,
/// fix15 colour) precomputed on the CPU so the shader stays dumb integer
/// math.
///
/// **80 bytes** — 8 × f32 + 10 × u32 + i32 × 2 (texture crawl), no pad
/// (storage-buffer stride aligns to 4, so 80 is legal on both sides)
/// — and WGSL's `DabG` must agree to the byte or the array stride desyncs
/// and every dab after the first in a flush reads garbage. Round 28 lost
/// real time to exactly that (56 vs 64), which is why the number is
/// spelled out with its arithmetic. The two i32 slots were the first two
/// pad u32s before #0.1 — same layout, now carrying the per-dab texture
/// scroll; #10 amendment 2 appended the stamp sin/cos pair; the P4
/// colorize/posterize port appended its three u32s.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuDab {
    x: f32,
    y: f32,
    radius: f32,
    hardness: f32,
    aspect: f32,
    angle: f32,
    color_r: u32,
    color_g: u32,
    color_b: u32,
    color_a: u32,
    opa_normal: u32,
    opa_lock: u32,
    flags: u32,
    // Texture-tip crawl offset (mask px) this dab sees; 0 when off.
    tex_u: i32,
    tex_v: i32,
    /// Dab-anchored stamp rotation (#10 amendment 2) as CPU-precomputed
    /// sin/cos: GPU trig intrinsics are orders coarser than libm and broke
    /// the <=1 parity bar at rotated angles.
    tex_sn: f32,
    tex_cs: f32,
    /// Colorize / Posterize stamp opacities, fix15 (the P4 port):
    ///   opa_colorize  = colorize  * opaque * 32768
    ///   opa_posterize = posterize * opaque * 32768
    /// and the posterize level count (already clamped 1..=128 by the C).
    opa_colorize: u32,
    opa_posterize: u32,
    poster_num: u32,
}

impl GpuDab {
    fn from(p: &mn_core::dab::DabParams) -> Self {
        // The C dispatch (process_op) for the paint<1 branch — paint>0 dabs
        // never reach the GPU (MyBrush::gpu_ready routes them CPU-side).
        // `op->normal` folds in (1-colorize)(1-posterize); the LockAlpha
        // stamp opacity carries those factors too (brushmodes dispatch).
        let cp = (1.0 - p.colorize) * (1.0 - p.posterize);
        let normal = (1.0 - p.lock_alpha) * cp * p.opaque * (1.0 - p.paint);
        let lock = p.lock_alpha * p.opaque * cp * (1.0 - p.paint);
        let f15 = |v: f32| (v.clamp(0.0, 1.0) * 32768.0) as u32;
        Self {
            x: p.x,
            y: p.y,
            radius: p.radius,
            hardness: p.hardness,
            aspect: p.aspect_ratio,
            angle: p.angle,
            color_r: p.color[0] as u32,
            color_g: p.color[1] as u32,
            color_b: p.color[2] as u32,
            color_a: f15(p.alpha),
            opa_normal: f15(normal),
            opa_lock: f15(lock),
            // bit0: colour_a < 1 (Normal_and_Eraser); bit1: LockAlpha applies.
            flags: u32::from(p.alpha < 1.0)
                | (u32::from(p.lock_alpha > 0.0 && p.alpha != 0.0) << 1),
            tex_u: p.tex_off[0],
            tex_v: p.tex_off[1],
            tex_sn: (p.tex_angle / 360.0 * 2.0 * std::f32::consts::PI).sin(),
            tex_cs: (p.tex_angle / 360.0 * 2.0 * std::f32::consts::PI).cos(),
            opa_colorize: f15(p.colorize * p.opaque),
            opa_posterize: f15(p.posterize * p.opaque),
            poster_num: p.posterize_num.max(1) as u32,
        }
    }
}

/// Per-tile uniform, bound with a dynamic offset (stride 256 = the minimum
/// uniform buffer offset alignment).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TileUni {
    ox: i32,
    oy: i32,
    dab_count: u32,
    flags: u32,
    /// Texture-tip mask side length; 0 = no texture this flush (the shader
    /// gates its load on this, mirroring the C's `tex_size > 0` guard).
    tex_size: u32,
    _pad: [u32; 11],
}

const UNI_STRIDE: usize = 256;
const TILE_BYTES: usize = 64 * 64 * 4 * 2;
/// Seed value for a tile that has no CPU pixels yet. Explicit because a
/// recycled `tile_pool` texture is NOT blank — see `flush_dabs`.
const ZERO_TILE: [u16; 64 * 64 * 4] = [0; 64 * 64 * 4];

/// Tiles a dab touches — the C's `floor(floor(x ± r_fringe) / 64)` range,
/// with div_euclid because Rust `/` truncates toward zero (negative tile
/// coordinates would be wrong).
pub fn dab_tiles(d: &mn_core::dab::DabParams, stamp: bool) -> impl Iterator<Item = TileIdx> {
    // #10 amendment 3: an anchored stamp rotates a square — sqrt(2) reach.
    let fringe = if stamp {
        d.radius * std::f32::consts::SQRT_2 + 1.0
    } else {
        d.radius + 1.0
    };
    let x0 = (d.x - fringe).floor().div_euclid(64.0) as i32;
    let x1 = (d.x + fringe).floor().div_euclid(64.0) as i32;
    let y0 = (d.y - fringe).floor().div_euclid(64.0) as i32;
    let y1 = (d.y + fringe).floor().div_euclid(64.0) as i32;
    (y0..=y1).flat_map(move |ty| (x0..=x1).map(move |tx| TileIdx::new(tx, ty)))
}

/// The GPU-dab machinery, owned by the Renderer (`None` when the adapter's
/// rgba16uint tiles lack STORAGE_BINDING — then `--gpu-dabs` is inert).
pub struct DabGpu {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    params_buf: wgpu::Buffer,
    params_cap: usize,
    tile_uni_buf: wgpu::Buffer,
    tile_uni_cap: usize,
    /// Bumped by every dispatched workgroup (atomicAdd); copied to
    /// `canary_read` and compared against `dispatched_total` at stroke end.
    canary_buf: wgpu::Buffer,
    canary_read: wgpu::Buffer,
    /// Workgroups dispatched since the stroke began (the canary's expected
    /// value — kept outside `DabStroke` because the stroke is closed before
    /// the readback that checks it).
    dispatched_total: u32,
    /// TEST HOOK: skip exactly one compute dispatch of the next flush while
    /// still counting it — a faithful simulation of the cursed iGPU dropping
    /// a dispatch (the canary then fires and the caller repairs on CPU).
    /// Nothing in production arms this.
    debug_drop_next: bool,
    /// Recycled 64×64 scratch textures — the per-flush dst copies. Pooled
    /// like the tile textures: freeing and reallocating textures makes the
    /// cursed iGPU driver sample stale memory (see `Renderer::tile_pool`).
    scratch_pool: Vec<wgpu::Texture>,
    /// The uploaded texture-tip mask (R32Uint, one gray value per texel —
    /// u32 textures have no 8-bit loadable format) plus its cache key. The
    /// key is the mask DATA pointer + size: `Arc<TextureMask>` is stable for
    /// a loaded preset, so a whole stroke (and every stroke on the same
    /// brush) pays one upload.
    tex_cache: Option<(usize, u32, wgpu::Texture, wgpu::TextureView)>,
    /// 1×1 zero mask bound when no texture is active — the binding must
    /// always resolve; the shader never loads it (tex_size == 0 gates).
    tex_dummy: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// The active stroke (eviction-guard + readback set).
    pub(crate) stroke: Option<DabStroke>,
}

pub(crate) struct DabStroke {
    pub layer: usize,
    /// WASH MODE (#0.1): the stroke rasterizes into a dedicated SENTINEL
    /// layer key in the tile cache instead of a document layer — the
    /// off-canvas wash buffer's GPU twin. Seeding reads the CPU wash
    /// buffer's layer 0 (blank ⇒ zero-seeded, correct by construction);
    /// the stroke-end readback feeds the existing `commit_wash` math.
    pub wash: bool,
    /// Every tile any flush of this stroke has written.
    pub touched: BTreeSet<TileIdx>,
}

/// Tile-cache layer key for the wash buffer — no document layer can own
/// it. Public since the P4 wash+smudge round: the app's smudge oracle
/// passes it as the layer so `readback_dab_tile` serves the IN-FLIGHT
/// wash accumulation, which is what the CPU path's sampler reads too.
pub const WASH_LAYER_KEY: usize = usize::MAX;

impl DabGpu {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mn.dabs"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/dab.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mn.dab.bgl"),
            entries: &[
                // The tile's pre-dab pixels (the scratch copy).
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // The tile itself (write-only storage).
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: crate::TILE_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                // This flush's dab list.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Per-tile constants (dynamic offset).
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // The canary.
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // The texture-tip mask (#0.1). R32Uint so `textureLoad`
                // works — u32 textures have no 8-bit formats. Always bound;
                // a 1×1 zero dummy when no mask is active.
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pll = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mn.dab.pll"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("mn.dab.pipe"),
            layout: Some(&pll),
            module: &shader,
            entry_point: None,
            compilation_options: Default::default(),
            cache: None,
        });

        let make_buf = |usage: wgpu::BufferUsages, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mn.dab.buf"),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            pipeline,
            bgl,
            params_buf: make_buf(
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                256 * std::mem::size_of::<GpuDab>() as u64,
            ),
            params_cap: 256,
            tile_uni_buf: make_buf(
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                UNI_STRIDE as u64,
            ),
            tile_uni_cap: 1,
            canary_buf: make_buf(
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                4,
            ),
            canary_read: make_buf(
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                4,
            ),
            dispatched_total: 0,
            debug_drop_next: false,
            scratch_pool: Vec::new(),
            tex_cache: None,
            tex_dummy: None,
            stroke: None,
        }
    }

    fn scratch_texture(&mut self, device: &wgpu::Device) -> wgpu::Texture {
        self.scratch_pool.pop().unwrap_or_else(|| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mn.dab.scratch"),
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: crate::TILE_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        })
    }

    fn ensure_params_cap(&mut self, device: &wgpu::Device, n: usize) {
        if n <= self.params_cap {
            return;
        }
        let cap = n.max(self.params_cap * 2);
        self.params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mn.dab.buf"),
            size: (cap * std::mem::size_of::<GpuDab>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.params_cap = cap;
    }

    fn ensure_uni_cap(&mut self, device: &wgpu::Device, n: usize) {
        if n <= self.tile_uni_cap {
            return;
        }
        let cap = n.max(self.tile_uni_cap * 2);
        self.tile_uni_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mn.dab.uni"),
            size: (cap * UNI_STRIDE) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.tile_uni_cap = cap;
    }

    /// Create + upload the texture-tip mask as R32Uint (gray u8 widened to
    /// u32 texels — `textureLoad` has no 8-bit u32 formats).
    fn make_mask_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
        size: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mn.dab.tex"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let wide: Vec<u32> = data.iter().map(|&g| g as u32).collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&wide),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        let v = t.create_view(&wgpu::TextureViewDescriptor::default());
        (t, v)
    }

    /// Make sure this flush's mask view exists: upload on first sight of a
    /// mask (keyed by data pointer + size — stable per loaded
    /// `Arc<TextureMask>`) and lazily create the 1×1 zero dummy. The view is
    /// then read immutably by the caller — kept separate so the mutable
    /// borrow ends here.
    fn ensure_mask(
        dg: &mut DabGpu,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mask: Option<(&[u8], u32)>,
    ) {
        if let Some((data, size)) = mask {
            let key = (data.as_ptr() as usize, size);
            if dg
                .tex_cache
                .as_ref()
                .is_none_or(|(p, s, _, _)| (*p, *s) != key)
            {
                let (t, v) = Self::make_mask_texture(device, queue, data, size);
                dg.tex_cache = Some((key.0, key.1, t, v));
            }
        } else if dg.tex_dummy.is_none() {
            let (t, v) = Self::make_mask_texture(device, queue, &[0], 1);
            dg.tex_dummy = Some((t, v));
        }
    }
}

impl crate::Renderer {
    /// Whether the adapter's rgba16uint tiles support storage binding (the
    /// P1 gate; false ⇒ `--gpu-dabs` is ignored and CPU rasterizes).
    pub fn gpu_dabs_supported(&self) -> bool {
        self.dabs.is_some()
    }

    /// A GPU dab stroke starts on `layer`: opens the stroke state
    /// (eviction-guard + readback set) and zeroes the canary.
    pub fn begin_dab_stroke(&mut self, layer: usize) {
        let Some(dg) = &mut self.dabs else { return };
        self.queue.write_buffer(&dg.canary_buf, 0, &[0, 0, 0, 0]);
        dg.dispatched_total = 0;
        dg.stroke = Some(DabStroke {
            layer,
            wash: false,
            touched: BTreeSet::new(),
        });
    }

    /// A GPU WASH stroke begins (#0.1): same machinery, sentinel layer key.
    pub fn begin_wash_dab_stroke(&mut self) {
        let Some(dg) = &mut self.dabs else { return };
        self.queue.write_buffer(&dg.canary_buf, 0, &[0, 0, 0, 0]);
        dg.dispatched_total = 0;
        dg.stroke = Some(DabStroke {
            layer: WASH_LAYER_KEY,
            wash: true,
            touched: BTreeSet::new(),
        });
    }

    /// TEST HOOK — arm a simulated driver dispatch-drop for the next
    /// `flush_dabs` (one compute dispatch skipped, still counted, so the
    /// stroke-end canary fires exactly as it does on the cursed iGPU).
    /// Production never calls this; tests use it to drive the app's real
    /// CPU-repair path deterministically.
    pub fn debug_drop_next_flush(&mut self) {
        if let Some(dg) = &mut self.dabs {
            dg.debug_drop_next = true;
        }
    }

    /// Rasterize one frame's drained dab record into the tile textures.
    /// `hard_dab` is the brush's tip mode (a shader flag; per-stroke const).
    ///
    /// `doc` is needed to guarantee the **seeding invariant**: a dab reads the
    /// tile it paints onto, so the destination texture must hold that tile's
    /// current CPU pixels before the dispatch. Two ways it would not, both of
    /// which corrupted the canvas before this argument existed:
    ///
    /// * A tile with artwork on it that the compositor has not cached yet
    ///   (`self.tiles` misses) got a **fresh, zero** texture — the dabs landed
    ///   on transparent black and the stroke-end readback wrote that back over
    ///   the CPU tile, **erasing whatever was already drawn there**.
    /// * Worse, a miss usually recycles from `tile_pool`, whose textures still
    ///   hold *a different tile's* pixels. `tile_pool`'s own contract (see its
    ///   field docs) is that "every upload is a full-tile `write_texture`, so
    ///   contents never leak between uses" — this path used a pooled texture
    ///   with no upload at all, so unrelated artwork was resurrected into
    ///   whatever tile the pen happened to touch.
    ///
    /// Re-uploading mid-stroke is safe and does not eat earlier flushes: BYPASS
    /// never touches CPU tiles, so a tile's CPU revision is frozen for the
    /// duration of the stroke and only the first flush that touches it seeds.
    pub fn flush_dabs(
        &mut self,
        doc: &mn_core::Document,
        dabs: &[mn_core::dab::DabParams],
        hard_dab: bool,
        texture: Option<(&[u8], u32, bool)>,
    ) {
        self.flush_dabs_impl(doc, None, dabs, hard_dab, texture);
    }

    /// WASH variant (#0.1): seed from the CPU wash `buf` (its layer 0) and
    /// write the sentinel layer key — the buffer is blank, so every tile
    /// zero-seeds, exactly like a wash stroke starting on empty paper.
    pub fn flush_wash_dabs(
        &mut self,
        buf: &mn_core::Document,
        dabs: &[mn_core::dab::DabParams],
        hard_dab: bool,
        texture: Option<(&[u8], u32, bool)>,
    ) {
        self.flush_dabs_impl(buf, Some(()), dabs, hard_dab, texture);
    }

    fn flush_dabs_impl(
        &mut self,
        doc: &mn_core::Document,
        wash: Option<()>,
        dabs: &[mn_core::dab::DabParams],
        hard_dab: bool,
        texture: Option<(&[u8], u32, bool)>,
    ) {
        if dabs.is_empty() {
            return;
        }
        let Some(dg) = &mut self.dabs else { return };
        if dg.stroke.is_none() {
            return;
        }
        let wash = wash.is_some() && dg.stroke.as_ref().unwrap().wash;
        let layer = if wash {
            WASH_LAYER_KEY
        } else {
            dg.stroke.as_ref().unwrap().layer
        };

        // Clamp to the canvas exactly like the CPU reference does: the brush
        // surface hands off-canvas dabs a scratch tile and drops the writes
        // (crates/brush/src/surface.rs) rather than growing the layer past the
        // document bounds. Without this the GPU path materialises tiles the CPU
        // path never would, and the stroke-end readback commits them.
        let (ex, ey) = doc.tile_extent();
        let stamp = texture.is_some_and(|(_, _, a)| a);
        let dirty: BTreeSet<TileIdx> = dabs
            .iter()
            .flat_map(|d| dab_tiles(d, stamp))
            .filter(|i| i.x >= 0 && i.y >= 0 && i.x < ex && i.y < ey)
            .collect();
        if dirty.is_empty() {
            return;
        }
        let gpu_dabs: Vec<GpuDab> = dabs.iter().map(GpuDab::from).collect();
        dg.ensure_params_cap(&self.device, gpu_dabs.len());
        self.queue
            .write_buffer(&dg.params_buf, 0, bytemuck::cast_slice(&gpu_dabs));
        dg.ensure_uni_cap(&self.device, dirty.len());

        // Every dirty tile needs a cache entry holding the tile's CURRENT
        // pixels (see the seeding invariant above) plus a scratch texture to
        // load them from.
        let device = &self.device;
        let tex_size = texture.map(|(_, s, _)| s).unwrap_or(0);
        // #10 amendment 2: dab-anchored stamp mode, a per-flush tile flag
        // (bit 1; bit 0 stays hard-dab).
        let tex_anchor_dab = texture.is_some_and(|(_, _, a)| a);
        DabGpu::ensure_mask(dg, device, &self.queue, texture.map(|(d, s, _)| (d, s)));
        // Clone the handle (refcounted) so the dg borrow ends here — the
        // loop below needs &mut dg for scratch textures.
        let mask_view = if texture.is_some() {
            dg.tex_cache.as_ref().unwrap().3.clone()
        } else {
            dg.tex_dummy.as_ref().unwrap().1.clone()
        };
        let cpu_layer = if wash {
            doc.layers.first()
        } else {
            doc.layers.get(layer)
        };
        let mut dst_textures = Vec::with_capacity(dirty.len());
        let mut scratch_textures = Vec::with_capacity(dirty.len());
        let mut bgs = Vec::with_capacity(dirty.len());
        let mut uni = Vec::with_capacity(dirty.len());
        for &idx in &dirty {
            let key = (layer, idx, crate::TileVariant::Pixels);
            let cpu_tile = cpu_layer.and_then(|l| l.tile(idx));
            let cpu_rev = cpu_tile.map(|t| t.revision()).unwrap_or(0);
            let seeded = self.tiles.contains_key(&key);
            let entry = self.tiles.entry(key).or_insert_with(|| {
                // The pool first (its textures carry the same usage flags);
                // the usage now includes STORAGE_BINDING | COPY_SRC.
                if let Some(mut t) = self.tile_pool.pop() {
                    t.revision = 0;
                    return t;
                }
                crate::make_tile_texture(device, &self.tile_texture_bgl)
            });
            // Seed on first touch, and whenever the CPU tile moved on without
            // the compositor having uploaded it (undo, a CPU-path stroke, a
            // fill). A tile with no CPU pixels seeds to zero — never to
            // whatever the pooled texture last held.
            if !seeded || entry.revision != cpu_rev {
                let data: &[u16] = cpu_tile.map(|t| t.data()).unwrap_or(&ZERO_TILE);
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &entry.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(data),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(64 * 4 * 2),
                        rows_per_image: Some(64),
                    },
                    wgpu::Extent3d {
                        width: 64,
                        height: 64,
                        depth_or_array_layers: 1,
                    },
                );
                entry.revision = cpu_rev;
            }
            dst_textures.push(entry.texture.clone());
            let scratch = dg.scratch_texture(device);
            scratch_textures.push(scratch.clone());
            let scratch_view = scratch.create_view(&wgpu::TextureViewDescriptor::default());
            let dst_view = entry
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            bgs.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mn.dab.bg"),
                layout: &dg.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&scratch_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&dst_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: dg.params_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &dg.tile_uni_buf,
                            offset: 0,
                            size: wgpu::BufferSize::new(UNI_STRIDE as u64),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: dg.canary_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&mask_view),
                    },
                ],
            }));
            uni.push(TileUni {
                ox: idx.x * 64,
                oy: idx.y * 64,
                dab_count: gpu_dabs.len() as u32,
                flags: u32::from(hard_dab) | (u32::from(tex_anchor_dab) << 1),
                tex_size,
                _pad: [0; 11],
            });
        }
        for (i, u) in uni.iter().enumerate() {
            self.queue.write_buffer(
                &dg.tile_uni_buf,
                (i * UNI_STRIDE) as u64,
                bytemuck::bytes_of(u),
            );
        }

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mn.dab"),
            });
        // Copies first, then one compute pass — a pass cannot interleave
        // with copies, and every dispatch must see its tile's prior pixels.
        for (dst, scratch) in dst_textures.iter().zip(&scratch_textures) {
            enc.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: dst,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: scratch,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
            );
        }
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("mn.dab.pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&dg.pipeline);
            // Test hook: a "dropped" dispatch is never submitted but still
            // counted below — indistinguishable from the driver bug the
            // canary exists for (real dispatches run, the count expects one
            // more than ran).
            let drop_first = std::mem::take(&mut dg.debug_drop_next);
            for (i, bg) in bgs.iter().enumerate() {
                if drop_first && i == 0 {
                    continue;
                }
                pass.set_bind_group(0, bg, &[(i * UNI_STRIDE) as u32]);
                pass.dispatch_workgroups(1, 1, 1);
            }
        }
        self.queue.submit(Some(enc.finish()));

        dg.scratch_pool.extend(scratch_textures);
        dg.dispatched_total += bgs.len() as u32;
        dg.stroke
            .as_mut()
            .unwrap()
            .touched
            .extend(dirty.iter().copied());
        // No damage bookkeeping here: the live-stroke regions stay damaged via
        // `update_canvas`'s stroke-state block, and post-readback damage falls
        // out of the `canvas_shown` compare (mark_dab_tile_clean refreshes the
        // texture cache only, never the canvas side). The mips do refresh.
        self.mips_dirty = true;
    }

    /// After a wash commit (#0.1): drop the sentinel layer's cache entries —
    /// they belong to no document layer, are never composited, and would
    /// otherwise linger until eviction.
    pub fn drop_wash_tiles(&mut self) {
        self.tiles.retain(|k, _| k.0 != WASH_LAYER_KEY);
    }

    /// Read ONE dab-cache tile back into CPU memory (#0.1 part 3, the
    /// smudge sampler's oracle): `None` when the stroke never touched the
    /// tile (no cache entry — the CPU tile is then current). No canary
    /// here: the stroke-end readback remains the single canary check.
    pub fn readback_dab_tile(&self, layer: usize, idx: TileIdx) -> Option<Vec<u16>> {
        if self.dabs.is_none() {
            return None;
        }
        let entry = self.tiles.get(&(layer, idx, crate::TileVariant::Pixels))?;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mn.dab.rb1"),
            size: TILE_BYTES as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mn.dab.rb1"),
            });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &entry.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(64 * 4 * 2),
                    rows_per_image: Some(64),
                },
            },
            wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(enc.finish()));
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let data = slice.get_mapped_range().expect("map dab tile").to_vec();
        buf.unmap();
        Some(bytemuck::cast_slice(&data).to_vec())
    }

    /// Stroke end: closes the stroke, returns its layer key, whether it was
    /// a WASH stroke (sentinel key — the caller runs the wash commit), and
    /// the touched tiles (the readback set; empty when no stroke ran).
    pub fn end_dab_stroke(&mut self) -> Option<(usize, bool, Vec<TileIdx>)> {
        let dg = self.dabs.as_mut()?;
        let st = dg.stroke.take()?;
        Some((st.layer, st.wash, st.touched.into_iter().collect()))
    }

    /// Read the stroke's tiles back into CPU memory plus the canary — the
    /// design's single map per stroke. `canary_ok == false` ⇒ a dispatch was
    /// dropped (the cursed-driver trap): the caller repairs on CPU.
    pub fn readback_dab_tiles(
        &mut self,
        layer: usize,
        tiles: &[TileIdx],
    ) -> (Vec<(TileIdx, Vec<u16>)>, bool) {
        if tiles.is_empty() {
            return (Vec::new(), true);
        }
        let n = tiles.len();
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mn.dab.read"),
            size: (n * TILE_BYTES) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mn.dab.rb"),
            });
        for (i, &idx) in tiles.iter().enumerate() {
            let Some(entry) = self.tiles.get(&(layer, idx, crate::TileVariant::Pixels)) else {
                continue;
            };
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &entry.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: (i * TILE_BYTES) as u64,
                        bytes_per_row: Some(64 * 4 * 2),
                        rows_per_image: Some(64),
                    },
                },
                wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
            );
        }
        let (canary_buf, canary_read, expected) = {
            let dg = self.dabs.as_ref().unwrap();
            (&dg.canary_buf, &dg.canary_read, dg.dispatched_total)
        };
        enc.copy_buffer_to_buffer(canary_buf, 0, canary_read, 0, 4);
        self.queue.submit(Some(enc.finish()));

        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let cslice = canary_read.slice(..);
        cslice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let canary = {
            let bytes = cslice.get_mapped_range().expect("map canary");
            u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        };
        let data = slice.get_mapped_range().expect("map dab readback").to_vec();
        buf.unmap();
        canary_read.unmap();

        let mut out = Vec::with_capacity(n);
        for (i, &idx) in tiles.iter().enumerate() {
            let px: Vec<u16> = data[i * TILE_BYTES..(i + 1) * TILE_BYTES]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            out.push((idx, px));
        }
        (out, canary == expected)
    }

    /// After the CPU tile is authoritative again (readback write or CPU
    /// repair), sync the TEXTURE cache so update_canvas does not re-upload
    /// what the GPU already holds. Deliberately does NOT touch
    /// `canvas_shown` — the canvas has not shown these pixels yet, and that
    /// gap (upload-fresh + redraw-required) is exactly what drives the
    /// post-stroke recomposite.
    pub fn mark_dab_tile_clean(&mut self, layer: usize, idx: TileIdx, rev: u64) {
        if let Some(c) = self.tiles.get_mut(&(layer, idx, crate::TileVariant::Pixels)) {
            c.revision = rev;
        }
    }
}
