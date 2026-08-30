//! The shared tile-kernel seam: run a pixel kernel over tiles (or over one
//! flat region) on the GPU, with the CPU implementation as the reference and
//! the only correctness authority.
//!
//! Two consumers today, both of which used to crunch whole pages on one
//! thread:
//!
//! * **Correction layers** — `Document::refresh_corrections_with` lends this
//!   its per-tile derive, so a parameter drag re-runs the colour arithmetic
//!   on the GPU instead of in a scalar loop.
//! * **The blur family** — `Document::apply_filter_with` lends this the
//!   gathered halo buffer, and Gaussian / Blur / Blur (strong) / Smoothing
//!   run as one separable convolution per axis. The unsharp mask rides the
//!   same path for its blur half and combines on the CPU
//!   (`filter::run_split`), and radial / spin blur run as
//!   [`Kernel::Smear`] — the one arm here that samples rather than gathers.
//!
//! # Design decisions, and why
//!
//! **Compute, not a fullscreen fragment pass.** The dab path already proved
//! a compute pipeline works on the target hardware (`dabs.rs`, Intel UHD 620
//! / DX12), so this is the pattern with field evidence behind it. A fragment
//! pass would additionally have needed a render target per chunk and would
//! have inherited the 8192-texel dimension cap discussed below.
//!
//! **Storage buffers, not tile textures.** The compositor's tile cache is
//! rgba16uint textures, and reusing them here was the obvious move. It does
//! not survive contact with the sizes involved:
//!
//! * A B4 page at 600 dpi is 6070 × 8598. The height exceeds the 8192
//!   `max_texture_dimension_2d` this laptop's adapter reports, so a
//!   whole-page region simply cannot be one texture — while a buffer of the
//!   same pixels is bounded by `max_storage_buffer_binding_size` (128 MiB at
//!   the limits the project asks for), which chunking handles cleanly.
//! * Tile textures are pooled rather than freed because freeing and
//!   reallocating them makes that driver sample stale memory (see
//!   `Renderer::tile_pool`). Buffers sidestep the whole hazard: two grown-on-
//!   demand scratch buffers live for the process and are never reallocated in
//!   steady state.
//! * Readback is one `copy_buffer_to_buffer` + one map for a whole batch,
//!   against one `copy_texture_to_buffer` per tile.
//!
//! Pixels are packed exactly as the CPU holds them — premultiplied fix15
//! RGBA in u16, two channels per u32 — so the upload is a `bytemuck` cast of
//! the tile's own bytes, no conversion pass on either side.
//!
//! **Routing: always-GPU-when-available above a size floor, no measured
//! verdict.** The inking path has a per-adapter measured verdict
//! (`app::bench`) because a dab flush sits *inside* an interactive stroke:
//! there, a GPU that is merely "not much faster" still loses, because it adds
//! a submit and a readback to every frame of a latency-critical loop. A
//! kernel job has none of that shape — it is one upload, one dispatch chain
//! and one readback for an entire page, against a single-threaded per-pixel
//! CPU loop. So the rule here is:
//!
//! * the pipelines exist (compute is supported), **and**
//! * the adapter is not a software rasterizer (WARP runs the same scalar work
//!   through an emulator plus two copies — it loses, reliably), **and**
//! * the job is at least [`KERNEL_FLOOR_PX`] pixels.
//!
//! That is [`Renderer::kernels_preferred`], and it is a *judgement*, recorded
//! here so a future measurement can overturn it rather than having to
//! rediscover the reasoning.
//!
//! **Failure is never corruption.** Every workgroup bumps a canary; the
//! readback compares it against the number dispatched, exactly as the dab
//! path does for the same cursed-driver behaviour. Results are assembled
//! off to the side and handed over only when every chunk of a job passed, so
//! a mid-job failure returns "declined" with the caller's buffers untouched
//! and the CPU reference runs on original pixels.
//!
//! **Preview residency (investigated, not built).** The tempting design is
//! to leave a correction layer's derived tiles on the GPU during a drag and
//! read back only on commit. It does not fit *yet*, for a reason that has
//! nothing to do with this seam: `Layer::corr` tiles are consumed by the CPU
//! compositor too — export, `composite_pixel`, thumbnails, the fill/wand
//! samplers, and the next correction layer stacked above this one, which
//! derives from a CPU composite of everything below it. Residency would mean
//! either dual-pathing all of those or reading back anyway at the first one
//! that asks. What the investigation produced instead was a measurement of
//! where a drag's time actually goes, and it changed the plan: a large share
//! of an uncached derive is the *below-composite re-walk*, not the
//! arithmetic, so the seam alone would have left a drag re-compositing the
//! page every tick. Splitting the correction's freshness stamp
//! (`CorrDerived::src_stamp`) so a drag reuses cached sources is the other
//! half, and the two only pay off together — the numbers are in
//! `tests/kernel_bench.rs`.

use mn_core::TileIdx;
use mn_core::adjust::{Adjust, TONE_CURVE_MAX, curve_tangents};
use mn_core::filter::{BoxPass, Smear};
use mn_core::tile::{TILE_LEN, TILE_PIXELS, TILE_SIZE};

/// Threads per workgroup — one pixel each. 256 is the downlevel-defaults
/// ceiling for `max_compute_invocations_per_workgroup`, so it is the largest
/// value that needs no limit negotiation.
const WG: usize = 256;

/// Below this many pixels, the upload + dispatch + readback round trip costs
/// more than the CPU loop it would replace. 2^18 px is 64 tiles — a 512×512
/// region. Small marquee blurs and small windowed corrections stay on the
/// CPU, which is also where they are already fast enough to be invisible.
pub const KERNEL_FLOOR_PX: usize = 1 << 18;

/// Tiles per pointwise batch. The same 256 as the compositor's
/// `UPLOAD_BATCH` and core's `DERIVE_BATCH`: 8 MB of pixels, one staging
/// write, one dispatch, one map.
const TILE_BATCH: usize = 256;

/// Ceiling on the pixels one region dispatch covers. Two buffers of `px * 8`
/// bytes must fit the adapter's storage-buffer binding limit and the
/// dispatch must fit `max_compute_workgroups_per_dimension`; 4 Mpx is well
/// inside both and holds GPU memory to 32 MB per buffer, which matters on an
/// integrated adapter sharing system RAM with the app. Clamped further at
/// runtime against the reported limits.
const REGION_CHUNK_PX: usize = 4 << 20;

/// Dynamic-offset stride of the uniform block — the 256-byte minimum
/// `min_uniform_buffer_offset_alignment`, which [`Params`] is padded to
/// exactly. One buffer holds every pass of a job, so a whole pass chain is
/// one encoder and one submit.
const UNI_STRIDE: usize = 256;

/// Which kernel to run — the "kernel id + params" half of a job.
#[derive(Clone, Copy, Debug)]
pub enum Kernel<'a> {
    /// The pointwise colour family: `correct_tile` + `Adjust::map`, per
    /// pixel, with optional per-pixel window coverage.
    Adjust(&'a Adjust),
    /// A chain of integer symmetric convolution passes, run along x in order
    /// and then along y in order — `Filter::separable_passes` produces
    /// these, and the arithmetic is bit-identical to the CPU's.
    Separable(&'a [BoxPass]),
    /// The smear family (radial / spin blur): one bilinear tap per sample
    /// matrix about a centre, uniformly averaged —
    /// `Filter::smear_samples` produces these.
    ///
    /// **Sampled from the storage buffer by hand, not through a texture
    /// sampler.** A `textureSampleLevel` variant is the obvious answer to
    /// "this one needs arbitrary source coordinates", and it is the wrong
    /// one here for three reasons already recorded in this module's header:
    /// the region can exceed `max_texture_dimension_2d` (a B4 page is 8598
    /// rows against this adapter's 8192 cap) while a buffer only has to
    /// clear the storage-binding limit; a texture would need its own bind
    /// group layout, an upload pass and a format that survives fix15, none
    /// of which the seam has; and the hardware filter is fixed-point (8-bit
    /// fractional weights on most parts), so it could not reproduce
    /// `sample_bilinear`'s f32 weights at all. Four manual taps in the
    /// shader are the SAME four the CPU takes, in the same order.
    Smear(&'a Smear),
}

/// One tile of a [`TileJob`]: its index, its `TILE_LEN` fix15 RGBA pixels,
/// and optional `TILE_PIXELS` coverage bytes.
pub type JobTile<'a> = (TileIdx, &'a [u16], Option<&'a [u8]>);

/// A job over a tile map: the source tiles, and which of them to hand back.
///
/// For [`Kernel::Adjust`] the tiles are independent and `src` may be any
/// set. For [`Kernel::Separable`] the tiles are assembled into their
/// bounding-box region first, so `src` must include the halo the kernel's
/// reach needs; tiles absent from the box read fully transparent, which is
/// the same convention `Raster` and the CPU box passes use.
pub struct TileJob<'a> {
    pub src: &'a [JobTile<'a>],
    /// Tiles to return, in this order. Empty = every tile of `src`, in order.
    pub out: &'a [TileIdx],
}

// Op ids — must match `kernel.wgsl`'s `OP_*`.
const OP_BRIGHTNESS: u32 = 0;
const OP_HUESAT: u32 = 1;
const OP_POSTERIZE: u32 = 2;
const OP_INVERT: u32 = 3;
const OP_BINARIZE: u32 = 4;
const OP_LEVELS: u32 = 5;
const OP_TONECURVE: u32 = 6;
const OP_BALANCE: u32 = 7;
const OP_GRADMAP: u32 = 8;

/// The uniform block, **exactly 256 bytes** so it is its own dynamic-offset
/// stride. Field offsets must match `Params` in `kernel.wgsl`: ten `u32`s
/// and two pad words (48 B), then two `vec4`s, then the point table, then
/// the tail pad. Every `vec4` lands on a 16-byte boundary, so Rust's
/// `repr(C)` layout and WGSL's alignment rules agree without implicit
/// padding — which is what makes `Pod` legal here. A field inserted in the
/// wrong place desyncs both sides silently; the parity tests are what would
/// catch it.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    op: u32,
    count: u32,
    w: u32,
    h: u32,
    axis: u32,
    taps: u32,
    cov_base: u32,
    n: u32,
    denom: u32,
    k_base: u32,
    _pad0: [u32; 2],
    a: [f32; 4],
    b: [f32; 4],
    pts: [[f32; 4]; TONE_CURVE_MAX],
    _pad1: [[f32; 4]; 3],
}

const _: () = assert!(std::mem::size_of::<Params>() == UNI_STRIDE);

impl Params {
    /// Pack an [`Adjust`] into the op id + scalar slots the shader reads.
    ///
    /// The tone curve's Fritsch–Carlson tangents are computed here, on the
    /// CPU, by `mn_core::adjust::curve_tangents` — the same function
    /// `curve_eval` uses — so the limiter has exactly one implementation in
    /// the tree and the shader only evaluates the Hermite segment.
    fn adjust(adj: &Adjust) -> Self {
        let mut p = Self::zeroed();
        match *adj {
            Adjust::BrightnessContrast {
                brightness,
                contrast,
            } => {
                p.op = OP_BRIGHTNESS;
                p.a = [brightness, contrast, 0.0, 0.0];
            }
            Adjust::HueSaturation {
                hue,
                saturation,
                luminosity,
            } => {
                p.op = OP_HUESAT;
                p.a = [hue, saturation, luminosity, 0.0];
            }
            Adjust::Posterize { levels } => {
                p.op = OP_POSTERIZE;
                // The CPU clamps to 2..=256 and then works in f32; do the
                // clamp on the same integer it does.
                p.a = [levels.clamp(2, 256) as f32, 0.0, 0.0, 0.0];
            }
            Adjust::Invert => p.op = OP_INVERT,
            Adjust::Binarize { threshold } => {
                p.op = OP_BINARIZE;
                p.a = [threshold, 0.0, 0.0, 0.0];
            }
            Adjust::Levels {
                in_black,
                in_white,
                gamma,
                out_black,
                out_white,
            } => {
                p.op = OP_LEVELS;
                p.a = [in_black, in_white, gamma, out_black];
                p.b = [out_white, 0.0, 0.0, 0.0];
            }
            Adjust::ToneCurve { pts, n } => {
                p.op = OP_TONECURVE;
                let n = (n as usize).min(TONE_CURVE_MAX);
                p.n = n as u32;
                let m = curve_tangents(&pts[..n]);
                for i in 0..n {
                    p.pts[i] = [pts[i][0], pts[i][1], m[i], 0.0];
                }
            }
            Adjust::ColourBalance {
                cyan_red,
                magenta_green,
                yellow_blue,
            } => {
                p.op = OP_BALANCE;
                p.a = [cyan_red, magenta_green, yellow_blue, 0.0];
            }
            Adjust::GradientMap { stops, n } => {
                p.op = OP_GRADMAP;
                let n = (n as usize).min(TONE_CURVE_MAX);
                p.n = n as u32;
                // The CPU sorts by position before sampling; do it here so
                // the shader's walk is a plain forward scan.
                let mut live: Vec<[f32; 5]> = stops[..n].to_vec();
                live.sort_by(|x, y| x[0].partial_cmp(&y[0]).unwrap_or(std::cmp::Ordering::Equal));
                for (i, s) in live.iter().enumerate() {
                    p.pts[i] = [s[0], s[1], s[2], s[3]];
                }
            }
        }
        p
    }

    fn zeroed() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

/// Flatten a pass chain's integer half-kernels into the weights buffer, and
/// return each pass's base index alongside its reach. `None` for a chain
/// with an empty or zero-denominator pass (a caller bug, never a shape this
/// seam should invent a meaning for).
fn flatten_weights(passes: &[BoxPass]) -> Option<(Vec<u32>, Vec<(u32, u32)>)> {
    if passes.is_empty() {
        return None;
    }
    let mut w = Vec::new();
    let mut meta = Vec::with_capacity(passes.len());
    for p in passes {
        if p.half.is_empty() || p.denom == 0 {
            return None;
        }
        meta.push((w.len() as u32, p.half.len() as u32));
        w.extend_from_slice(&p.half);
    }
    Some((w, meta))
}

/// A scratch storage buffer that grows on demand and is never shrunk —
/// reallocating per drag frame is the churn the tile pool exists to avoid.
struct Slot {
    label: &'static str,
    usage: wgpu::BufferUsages,
    buf: Option<wgpu::Buffer>,
    cap: u64,
}

impl Slot {
    fn new(label: &'static str, usage: wgpu::BufferUsages) -> Self {
        Self {
            label,
            usage,
            buf: None,
            cap: 0,
        }
    }

    fn get(&mut self, device: &wgpu::Device, need: u64) -> &wgpu::Buffer {
        if self.cap < need {
            let cap = need.max(self.cap * 2);
            self.buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: cap,
                usage: self.usage,
                mapped_at_creation: false,
            }));
            self.cap = cap;
        }
        self.buf.as_ref().expect("slot buffer just ensured")
    }
}

/// Which entry point a dispatch runs. One bind group layout serves all
/// three; only the shader function differs.
#[derive(Clone, Copy)]
enum Pipe {
    Adjust,
    Sep,
    Smear,
}

/// The kernel machinery, owned by the Renderer. `None` when the adapter has
/// no compute shaders.
pub struct KernelGpu {
    adjust_pipe: wgpu::ComputePipeline,
    sep_pipe: wgpu::ComputePipeline,
    smear_pipe: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    canary_buf: wgpu::Buffer,
    canary_read: wgpu::Buffer,
    /// The two ping-pong pixel buffers, the per-pass uniform block (one
    /// `UNI_STRIDE` slot per pass), the separable weights, and the mappable
    /// readback staging.
    a: Slot,
    b: Slot,
    params: Slot,
    weights: Slot,
    read: Slot,
    /// TEST HOOK: skip exactly one dispatch of the next job while still
    /// counting it — a faithful simulation of a driver dropping work, so the
    /// canary fires and the caller falls back to the CPU. Production never
    /// arms this.
    debug_fail_next: bool,
    /// TEST HOOK: shrink the region chunk so a test-sized raster still gets
    /// banded. Without it the band path only runs at page scale, where the
    /// suite cannot afford to go — and a halo off-by-one shows up as a seam
    /// nobody sees until a real B4 blur.
    debug_chunk_px: Option<usize>,
}

impl KernelGpu {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mn.kernel"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/kernel.wgsl").into()),
        });
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mn.kernel.bgl"),
            entries: &[
                storage(0, true),
                storage(1, false),
                storage(2, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        // One bind group serves every pass of a chain; the
                        // offset picks the pass. Four storage bindings is the
                        // downlevel ceiling, so the params could not become a
                        // fifth one.
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage(4, true),
            ],
        });
        let pll = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mn.kernel.pll"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipe = |entry: &'static str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("mn.kernel.pipe"),
                layout: Some(&pll),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let mk = |usage, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mn.kernel.buf"),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            adjust_pipe: pipe("adjust_main"),
            sep_pipe: pipe("sep_main"),
            smear_pipe: pipe("smear_main"),
            bgl,
            canary_buf: mk(
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                4,
            ),
            canary_read: mk(
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                4,
            ),
            a: Slot::new(
                "mn.kernel.a",
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            ),
            b: Slot::new(
                "mn.kernel.b",
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            ),
            params: Slot::new(
                "mn.kernel.uni",
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            weights: Slot::new(
                "mn.kernel.weights",
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            read: Slot::new(
                "mn.kernel.read",
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            ),
            debug_fail_next: false,
            debug_chunk_px: None,
        }
    }
}

impl crate::Renderer {
    /// Whether the kernel pipelines exist at all (compute is supported).
    /// Tests call the `run_*` entry points directly on this alone, so they
    /// still exercise the shaders under the software adapter.
    pub fn kernels_supported(&self) -> bool {
        self.kernels.is_some()
    }

    /// A software rasterizer (WARP / lavapipe), which runs the same scalar
    /// work an emulator's worth slower and pays two buffer copies on top.
    /// Every batch-kernel routing decision declines these.
    pub fn adapter_is_software(&self) -> bool {
        self.adapter_info().device_type == wgpu::DeviceType::Cpu
    }

    /// The app's routing predicate — see the module docs' "Routing" note.
    /// A job of `px` pixels should go to the GPU when the pipelines exist,
    /// the adapter is real silicon, and the job clears [`KERNEL_FLOOR_PX`].
    pub fn kernels_preferred(&self, px: usize) -> bool {
        self.kernels.is_some() && !self.adapter_is_software() && px >= KERNEL_FLOOR_PX
    }

    /// TEST HOOK — make the next kernel job drop one dispatch (still counted),
    /// so its canary fails and the job declines. The fallback path is only
    /// worth having if it is exercised; this is how.
    pub fn debug_fail_next_kernel(&mut self) {
        if let Some(k) = &mut self.kernels {
            k.debug_fail_next = true;
        }
    }

    /// The seam, tile flavour: run `kernel` over `job`'s tiles and return the
    /// requested ones, or `None` to decline (unsupported, unexpressible, or a
    /// dispatch canary failure). Declining is always legal — the caller runs
    /// the CPU reference.
    pub fn run_tile_kernel(
        &mut self,
        kernel: Kernel<'_>,
        job: &TileJob<'_>,
    ) -> Option<Vec<Vec<u16>>> {
        if self.kernels.is_none() || job.src.is_empty() {
            return None;
        }
        if job.src.iter().any(|(_, px, cov)| {
            px.len() != TILE_LEN || cov.is_some_and(|c| c.len() != TILE_PIXELS)
        }) {
            return None;
        }
        match kernel {
            Kernel::Adjust(adj) => self.tile_adjust(adj, job),
            // Both neighbourhood kernels want one flat region, not tiles.
            Kernel::Separable(_) | Kernel::Smear(_) => self.tile_region(kernel, job),
        }
    }

    /// The seam, region flavour: run `kernel` in place over `w * h` fix15
    /// RGBA pixels. Returns false — leaving `px` **untouched** — when it
    /// declines, so the caller's CPU reference sees original pixels.
    pub fn run_region_kernel(
        &mut self,
        kernel: Kernel<'_>,
        px: &mut [u16],
        w: usize,
        h: usize,
    ) -> bool {
        match self.region_result(kernel, px, w, h) {
            Some(out) => {
                px.copy_from_slice(&out);
                true
            }
            None => false,
        }
    }

    // ------------------------------------------------------------ pointwise --

    fn tile_adjust(&mut self, adj: &Adjust, job: &TileJob<'_>) -> Option<Vec<Vec<u16>>> {
        let base = Params::adjust(adj);
        let any_cov = job.src.iter().any(|(_, _, c)| c.is_some());
        // Built in `src` order; the index map is only assembled when the
        // caller asked for a different `out` set. The correction derive —
        // the hot consumer — wants "these tiles back in this order" and so
        // never pays for the map at all.
        let mut done: Vec<Vec<u16>> = Vec::with_capacity(job.src.len());

        for batch in job.src.chunks(TILE_BATCH) {
            let n = batch.len();
            let pixels = n * TILE_LEN;
            // Coverage rides in the SAME storage buffer, past the pixels.
            // Three storage bindings is the downlevel ceiling minus one, and
            // the fourth slot is spoken for by the separable weights — but
            // coverage appends here for free, so it never needed one.
            // Two bytes per u16 word, little-endian, which puts byte `p` of
            // the run at coverage index `p` exactly as the shader reads it.
            let cov_u16 = if any_cov { n * TILE_PIXELS / 2 } else { 0 };
            let mut up = vec![0u16; pixels + cov_u16];
            for (i, (_, tile, _)) in batch.iter().enumerate() {
                up[i * TILE_LEN..(i + 1) * TILE_LEN].copy_from_slice(tile);
            }
            if any_cov {
                for (i, (_, _, cov)) in batch.iter().enumerate() {
                    let dst = &mut up
                        [pixels + i * TILE_PIXELS / 2..pixels + (i + 1) * TILE_PIXELS / 2];
                    match cov {
                        // A tile with no window is fully covered; the
                        // shader's 255 branch is the byte-identical one.
                        None => dst.fill(u16::MAX),
                        Some(c) => {
                            for (q, word) in dst.iter_mut().enumerate() {
                                *word = c[q * 2] as u16 | ((c[q * 2 + 1] as u16) << 8);
                            }
                        }
                    }
                }
            }

            let mut params = base;
            params.count = (n * TILE_PIXELS) as u32;
            // In u32 units — what the shader indexes `src` by.
            params.cov_base = if any_cov { (pixels / 2) as u32 } else { 0 };
            let out = self.dispatch(&up, &[], &[params], Pipe::Adjust, pixels)?;
            for i in 0..n {
                done.push(out[i * TILE_LEN..(i + 1) * TILE_LEN].to_vec());
            }
        }

        if job.out.is_empty() {
            return Some(done);
        }
        let at: std::collections::HashMap<TileIdx, usize> = job
            .src
            .iter()
            .enumerate()
            .map(|(i, (idx, _, _))| (*idx, i))
            .collect();
        job.out
            .iter()
            .map(|i| at.get(i).map(|&n| done[n].clone()))
            .collect()
    }

    // ----------------------------------------------------------- neighbourhood --

    /// A neighbourhood kernel over a tile map: assemble the tiles' bounding
    /// box into one region, run the region path, slice the wanted tiles back
    /// out. Tiles the caller did not supply read transparent — the same
    /// "outside is transparent" convention `Raster` has.
    fn tile_region(&mut self, kernel: Kernel<'_>, job: &TileJob<'_>) -> Option<Vec<Vec<u16>>> {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for (i, _, _) in job.src {
            x0 = x0.min(i.x);
            y0 = y0.min(i.y);
            x1 = x1.max(i.x + 1);
            y1 = y1.max(i.y + 1);
        }
        let (tw, th) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let (w, h) = (tw * TILE_SIZE, th * TILE_SIZE);
        // Guard the assembly before allocating it: a sparse map spanning a
        // huge box would otherwise materialise the whole box.
        if w.checked_mul(h)? > REGION_CHUNK_PX * 16 {
            return None;
        }
        let mut region = vec![0u16; w * h * 4];
        for (idx, tile, _) in job.src {
            let (cx, cy) = (
                (idx.x - x0) as usize * TILE_SIZE,
                (idx.y - y0) as usize * TILE_SIZE,
            );
            for row in 0..TILE_SIZE {
                let s = row * TILE_SIZE * 4;
                let d = ((cy + row) * w + cx) * 4;
                region[d..d + TILE_SIZE * 4].copy_from_slice(&tile[s..s + TILE_SIZE * 4]);
            }
        }
        let out = self.region_result(kernel, &region, w, h)?;

        let wanted: Vec<TileIdx> = if job.out.is_empty() {
            job.src.iter().map(|(i, _, _)| *i).collect()
        } else {
            job.out.to_vec()
        };
        wanted
            .iter()
            .map(|idx| {
                if idx.x < x0 || idx.y < y0 || idx.x >= x1 || idx.y >= y1 {
                    return None;
                }
                let (cx, cy) = (
                    (idx.x - x0) as usize * TILE_SIZE,
                    (idx.y - y0) as usize * TILE_SIZE,
                );
                let mut t = vec![0u16; TILE_LEN];
                for row in 0..TILE_SIZE {
                    let s = ((cy + row) * w + cx) * 4;
                    let d = row * TILE_SIZE * 4;
                    t[d..d + TILE_SIZE * 4].copy_from_slice(&out[s..s + TILE_SIZE * 4]);
                }
                Some(t)
            })
            .collect()
    }

    /// The region path proper: returns the filtered pixels, or `None`.
    /// Never writes through `px`.
    fn region_result(
        &mut self,
        kernel: Kernel<'_>,
        px: &[u16],
        w: usize,
        h: usize,
    ) -> Option<Vec<u16>> {
        self.kernels.as_ref()?;
        if w == 0 || h == 0 || px.len() != w * h * 4 {
            return None;
        }
        let chunk_px = self.region_chunk_px();
        let mut out = vec![0u16; px.len()];

        match kernel {
            Kernel::Adjust(adj) => {
                let base = Params::adjust(adj);
                // Pixels are independent: straight linear chunking.
                for start in (0..w * h).step_by(chunk_px) {
                    let n = chunk_px.min(w * h - start);
                    let mut params = base;
                    params.count = n as u32;
                    let got = self.dispatch(
                        &px[start * 4..(start + n) * 4],
                        &[],
                        &[params],
                        Pipe::Adjust,
                        n * 4,
                    )?;
                    out[start * 4..(start + n) * 4].copy_from_slice(&got);
                }
            }
            Kernel::Separable(passes) => {
                let (weights, meta) = flatten_weights(passes)?;
                // The halo the vertical chain needs: every pass's reach
                // stacks, because pass k reads pass k-1's output.
                let reach: usize = meta.iter().map(|(_, taps)| *taps as usize - 1).sum();
                // Horizontal bands with that halo above and below. The
                // horizontal chain needs no halo (the full width is present
                // and a row's h-result depends only on that row); the
                // vertical chain reads the halo rows, and rows past the
                // region's real edges are absent and therefore transparent,
                // which is exactly `box_v`'s convention. Bands must read
                // ORIGINAL pixels, which is why the result is assembled
                // separately rather than written back in place.
                let band_rows = (chunk_px / w).saturating_sub(2 * reach);
                if band_rows == 0 {
                    return None;
                }
                for y0 in (0..h).step_by(band_rows) {
                    let y1 = (y0 + band_rows).min(h);
                    let g0 = y0.saturating_sub(reach);
                    let g1 = (y1 + reach).min(h);
                    let bh = g1 - g0;
                    let up = &px[g0 * w * 4..g1 * w * 4];
                    // Every pass along x in order, then every pass along y
                    // in order — `gaussian`'s shape, which is what makes the
                    // result bit-identical rather than merely close.
                    let mut plan: Vec<Params> = Vec::with_capacity(meta.len() * 2);
                    for axis in 0..2u32 {
                        for (i, (k_base, taps)) in meta.iter().enumerate() {
                            let mut p = Params::zeroed();
                            p.count = (w * bh) as u32;
                            p.w = w as u32;
                            p.h = bh as u32;
                            p.axis = axis;
                            p.taps = *taps;
                            p.denom = passes[i].denom;
                            p.k_base = *k_base;
                            plan.push(p);
                        }
                    }
                    let band = self.dispatch(up, &weights, &plan, Pipe::Sep, w * bh * 4)?;
                    let keep = (y0 - g0) * w * 4;
                    out[y0 * w * 4..y1 * w * 4]
                        .copy_from_slice(&band[keep..keep + (y1 - y0) * w * 4]);
                }
            }
            Kernel::Smear(s) => {
                if s.mats.is_empty() {
                    return None;
                }
                // NOT banded, and it cannot be: a smear's taps land anywhere
                // in the region (a corner pixel's arc reaches the far side),
                // so there is no halo width that would make a band
                // self-sufficient — the whole source has to be resident.
                // Above the chunk ceiling this therefore declines and the CPU
                // reference runs, which is the honest answer for a full-page
                // smear: 52 Mpx of source is 417 MB and no adapter here will
                // bind it.
                if w * h > chunk_px {
                    return None;
                }
                let mut p = Params::zeroed();
                p.count = (w * h) as u32;
                p.w = w as u32;
                p.h = h as u32;
                p.n = s.mats.len() as u32;
                p.a = [s.centre[0], s.centre[1], 0.0, 0.0];
                // The matrices ride the weights buffer as raw f32 bits: four
                // storage bindings is the downlevel ceiling and all four are
                // spoken for, so a fifth for coefficients was never
                // available. `bitcast<f32>` on the shader side.
                let coef: Vec<u32> = s.mats.iter().flatten().map(|v| v.to_bits()).collect();
                let got = self.dispatch(px, &coef, &[p], Pipe::Smear, w * h * 4)?;
                out.copy_from_slice(&got);
            }
        }
        Some(out)
    }

    /// TEST HOOK — force the region band size, so a test-sized raster
    /// exercises the halo bookkeeping the page-sized path would.
    pub fn debug_region_chunk_px(&mut self, px: Option<usize>) {
        if let Some(k) = &mut self.kernels {
            k.debug_chunk_px = px;
        }
    }

    /// Largest region one dispatch may cover on this adapter.
    fn region_chunk_px(&self) -> usize {
        let l = self.device.limits();
        self.kernels
            .as_ref()
            .and_then(|k| k.debug_chunk_px)
            .unwrap_or(REGION_CHUNK_PX)
            .min(l.max_storage_buffer_binding_size as usize / 8)
            .min(l.max_buffer_size as usize / 8)
            .min(l.max_compute_workgroups_per_dimension as usize * WG)
            .max(1)
    }

    // ------------------------------------------------------------- dispatch --

    /// Upload `up` (and `weights`), run `plan`'s passes in order — ping-
    /// ponging between the two scratch buffers — read `out_len` `u16`s back
    /// from wherever the last pass landed, and verify the canary. `None` on
    /// any failure; nothing the caller owns has been touched.
    ///
    /// `up` is fix15 `u16` verbatim: it goes to the GPU as a byte cast of
    /// itself, which is already the shader's packing (see the note by the
    /// former `pack` helper).
    ///
    /// One encoder and one submit for the whole chain: the per-pass uniform
    /// block is addressed by dynamic offset (the dab path's trick), so
    /// rewriting a shared buffer between passes — which would have forced a
    /// submit per pass — never arises.
    fn dispatch(
        &mut self,
        up: &[u16],
        weights: &[u32],
        plan: &[Params],
        pipe: Pipe,
        out_len: usize,
    ) -> Option<Vec<u16>> {
        if plan.is_empty() {
            return None;
        }
        let bytes = (up.len() * 2) as u64;
        let out_bytes = (out_len * 2) as u64;
        let device = &self.device;
        let k = self.kernels.as_mut()?;

        let a = k.a.get(device, bytes.max(out_bytes)).clone();
        let b = k.b.get(device, bytes.max(out_bytes)).clone();
        let read = k.read.get(device, out_bytes).clone();
        let uni = k
            .params
            .get(device, (plan.len() * UNI_STRIDE) as u64)
            .clone();
        // Always bound, even for the pointwise family that never reads it —
        // a bind group has to resolve every entry in its layout.
        let wbuf = k.weights.get(device, (weights.len().max(1) * 4) as u64).clone();

        // `u16` → bytes is always alignment-safe (u32 would not be: a
        // `Vec<u16>`'s allocation is only 2-aligned).
        self.queue
            .write_buffer(&a, 0, bytemuck::cast_slice::<u16, u8>(up));
        self.queue.write_buffer(&k.canary_buf, 0, &[0, 0, 0, 0]);
        if !weights.is_empty() {
            self.queue
                .write_buffer(&wbuf, 0, bytemuck::cast_slice(weights));
        }
        for (i, p) in plan.iter().enumerate() {
            self.queue
                .write_buffer(&uni, (i * UNI_STRIDE) as u64, bytemuck::bytes_of(p));
        }

        // One bind group per ping-pong direction; the dynamic offset picks
        // the pass.
        let bind = |src: &wgpu::Buffer, dst: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mn.kernel.bg"),
                layout: &k.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: src.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: dst.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: k.canary_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &uni,
                            offset: 0,
                            size: wgpu::BufferSize::new(UNI_STRIDE as u64),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wbuf.as_entire_binding(),
                    },
                ],
            })
        };
        let bg_ab = bind(&a, &b);
        let bg_ba = bind(&b, &a);

        let drop_one = std::mem::take(&mut k.debug_fail_next);
        let mut expected = 0u32;
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mn.kernel"),
        });
        for (i, p) in plan.iter().enumerate() {
            let groups = (p.count as usize).div_ceil(WG) as u32;
            expected += groups;
            let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("mn.kernel.pass"),
                timestamp_writes: None,
            });
            cp.set_pipeline(match pipe {
                Pipe::Adjust => &k.adjust_pipe,
                Pipe::Sep => &k.sep_pipe,
                Pipe::Smear => &k.smear_pipe,
            });
            cp.set_bind_group(
                0,
                if i % 2 == 0 { &bg_ab } else { &bg_ba },
                &[(i * UNI_STRIDE) as u32],
            );
            // The dropped dispatch is still counted above, so the canary
            // comes up short exactly as it does when a driver eats one.
            if !(drop_one && i == 0) {
                cp.dispatch_workgroups(groups, 1, 1);
            }
        }

        // Pass i writes b for even i, a for odd — so the chain ends in b
        // when it had an odd number of passes.
        let final_buf = if plan.len() % 2 == 1 { &b } else { &a };
        enc.copy_buffer_to_buffer(final_buf, 0, &read, 0, out_bytes);
        enc.copy_buffer_to_buffer(&k.canary_buf, 0, &k.canary_read, 0, 4);
        self.queue.submit(Some(enc.finish()));

        let slice = read.slice(..out_bytes);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let cslice = k.canary_read.slice(..);
        cslice.map_async(wgpu::MapMode::Read, |_| {});
        if device.poll(wgpu::PollType::wait_indefinitely()).is_err() {
            return None;
        }
        // Both ranges are read and BOTH buffers unmapped before any early
        // return: a buffer left mapped would poison every later job.
        let canary = cslice
            .get_mapped_range()
            .ok()
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        // `pod_collect_to_vec` rather than `cast_slice`: a mapped range is
        // not guaranteed 2-aligned, and this copy has to happen anyway.
        let data = slice
            .get_mapped_range()
            .ok()
            .map(|m| bytemuck::pod_collect_to_vec::<u8, u16>(&m));
        read.unmap();
        k.canary_read.unmap();
        if canary != Some(expected) {
            return None;
        }
        data
    }
}

// No pack/unpack step exists on purpose. The shader's "two channels per
// u32" layout IS the CPU's `[u16]` layout on a little-endian machine —
// `src[p*2] = r | g<<16` is exactly the bytes of `[r, g]` — so tiles and
// rasters upload as a byte cast of themselves. An earlier revision packed
// into a `Vec<u32>` and unpacked on the way back, which cost two full-size
// allocations plus two copies per band: ~300 MB of churn for one full-width
// B4 gaussian band, on a laptop that shares its RAM with the GPU.
