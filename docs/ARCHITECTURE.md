# MangaNakama architecture (v3 — 2026-08-13)

Windows-only manga drawing app (scope: `ROADMAP.md` at the repository root).
Rust workspace + wgpu GPU compositing + libmypaint brush engine (vendored C) +
raw Win32 message loop (**no winit** — we own pen input end to end).
Target: `x86_64-pc-windows-gnu` (no MSVC assumed). C compiles with the
project-local w64devkit (`toolchain/w64devkit`, not committed — see SETUP.md).

## Crates

- `crates/core` (`mn-core`) — pure data/logic, no OS deps. Document, layers,
  tiles, **undo** (`begin_op`/`end_op` + tile snapshots), **stabilizer**
  (pull-string, a `StrokeSink` decorator), pressure curve, blend math,
  **ORA save/load**, PNG export. Unit-testable with plain `cargo test`.
- `crates/brush` (`mn-brush`) — libmypaint FFI (`build.rs` compiles
  `vendor/libmypaint` via `cc`), `.myb` preset loading (serde_json → setter
  calls), tiled surface backend writing into `core` tiles. `MyBrush` is the
  brush; `SimpleDab` survives only as the fallback when a preset will not load.
- `crates/gpu` (`mn-gpu`) — wgpu: device/adapter init, tile texture cache,
  layer compositor (opacity + Normal/Multiply/Screen), viewport transform,
  offscreen render-to-image (tests, export, `--screenshot`), and the GPU dab
  compute path (`dabs.rs` + `shaders/dab.wgsl`, behind `--gpu-dabs`;
  the CPU rasterizer stays the reference).
- `crates/app` (`mn-app`, bin `manganakama`) — Win32 window + message loop,
  WM_POINTER pen input, egui shell, tool state, wiring.

Dependency direction: `app → {brush, gpu, core}`, `brush → core`, `gpu → core`.
`core` depends on no OS-ish crate (image/zip/serde are fine).

## Tile + color model (pinned — do not renegotiate)

- Tile: **64×64 px, RGBA u16, premultiplied alpha, fix15** (libmypaint native:
  `1.0 == 1<<15 == 32768`). Zero conversion on the brush hot path.
- Canvas pixel space: origin top-left, y-down. Tile index = `floor(px / 64)`.
- Layer tiles are sparse: `HashMap<TileIdx, Arc<Tile>>`. Write path uses
  `Arc::make_mut` (copy-on-write) so undo snapshots are cheap Arc clones.
- Dirty tracking: per-tile monotonically increasing revision `u64`; GPU uploads
  only tiles whose revision is newer than its cache.
- Display may approximate fix15→unorm with a shader scale; **export/save paths
  convert exactly on CPU**.

## Contracts between crates (keep these signatures)

```rust
// core::doc — layer ops and presentation go through Document, not the Vec:
// the setters publish a revision the tile path cannot see.
Document::{begin_op, end_op, undo, redo, can_undo, clear_history}
Document::{add_layer, remove_layer, duplicate_layer, move_layer, set_active,
           set_layer_opacity, set_layer_blend, set_layer_visible}
pub enum Blend { Normal, Multiply, Screen }   // all fixed-function-blendable
Layer::{tile, tile_mut, set_tile, tiles, tile_bounds}

// core::stroke — the one seam between "what makes pixels" and everything else
pub struct PenSample { x, y, pressure, tilt_x, tilt_y: f32, t_ms: f64 }
pub trait StrokeSink { fn begin(&mut self, &mut Document);
                       fn sample(&mut self, &mut Document, PenSample);
                       fn end(&mut self, &mut Document); }
// core::stabilize::Stabilizer<S: StrokeSink> decorates a sink; strength 0 is an
// exact passthrough, and `end()` drains the string onto the last raw sample.

// brush::MyBrush implements StrokeSink via libmypaint. .myb parsed in Rust and
// applied with mypaint_brush_set_base_value / set_mapping_n / set_mapping_point;
// the vendored C is patched to drop json-c. EVERY patch is in vendor/PATCHES.md.

// gpu::Renderer / Viewport
Viewport::{to_canvas, to_screen, zoom_around, rotate_around, fit}
Renderer::{render, render_with_overlay, render_offscreen, invalidate, resize}
```

Compositing: per layer, draw its tiles as quads into a canvas texture with
fixed-function blend states (Normal/Multiply/Screen) and the layer opacity folded
into the source, then canvas → swapchain with the viewport transform. Per-layer
signatures (visible/opacity/blend/tile-count) are compared each frame, so
opacity/blend/visibility changes need **no** `invalidate()`; only structural
changes (add/remove/reorder/new document) do.

`render_with_overlay` is the app's hook: the compositor acquires and presents the
frame, and the closure paints egui into the same swapchain view first.

## Stroke path (app crate) — the two rules that bite

- A stroke is **always** `doc.begin_op()` → `StrokeSink::begin` → samples →
  `StrokeSink::end` → `doc.end_op()`. The op bracket is the whole of undo (every
  `tile_mut` in between snapshots its pre-image), and `end` must run before
  `end_op` because the stabilizer emits its last dabs there.
- **Never feed samples without `begin`.** `MyBrush::begin` resets libmypaint and
  the first sample goes in with `dtime = 10 s`: the `slow_tracking` smoothing in
  `mypaint-brush.c` runs *before* the reset branch, so with a small dtime the pen
  is planted next to the previous stroke's end and the line smears in from there
  (measured: a stroke at y=256 painted a box from (0,1) to (399,257)).
- Same reason the mouse fallback stamps a real `t_ms` (ms since process start):
  libmypaint divides by dtime, and a constant timestamp pins the smoothing.
- Tilt is degrees (−90..90) all the way to `MyBrush`, which normalises by 60
  internally. Do not pre-convert.

## Pen input (app crate) — the project-killer risk; follow exactly

- Raw Win32 via `windows-sys`. Per-monitor-v2 DPI aware.
- `WM_POINTERDOWN/UPDATE/UP` + `GetPointerPenInfoHistory` — history arrives
  **newest first, reverse it**. Pressure = `penInfo.pressure / 1024.0`.
- Handle `WM_TABLET_QUERYSYSTEMGESTURESTATUS` (0x02CC): return the
  `TABLET_DISABLE_PRESSANDHOLD`/flicks/palm flags so inking has no hold delay.
- `SetWindowFeedbackSetting`: disable all pen/touch visual feedback.
- Do **not** call `EnableMouseInPointer`. Mouse drawing fallback via classic
  `WM_MOUSE*` (constant pressure 0.5) so everything is testable without a pen.
- Message loop: idle-wait (`GetMessageW`), render on `WM_PAINT` when dirty.
  Commands are executed by `pump_commands` *outside* the wndproc — a file dialog
  pumps the message queue and would alias a live `&mut App`.
- Diagnostics HUD (egui overlay, F1): adapter, present mode, events/sec, last
  pressure, batch sizes, active brush, stabilizer. The owner's laptop pen stack is
  known-cursed (see dehook project) — the HUD is the diagnosis tool.

## Traps that cost real time (do not "clean up")

- **DX12 only** (`DEFAULT_BACKENDS`). `Backends::PRIMARY` also enables Vulkan,
  and this laptop's Intel UHD 620 Vulkan driver dies in `request_device` with
  `STATUS_ACCESS_VIOLATION`. The software fallback is DX12 WARP anyway;
  `WGPU_BACKEND=vulkan` still overrides.
- **The Intel DX12 driver (10.0.19041, 2020) intermittently DROPS ONE DRAW
  from a canvas rebuild frame** — reproduced 2026-08-14 on the round-10 code
  with a flat stack (render → add layer → render loses the new layer's tile
  in patches; position varies per run; WARP is exact every time). Pre-dates
  folders/groups; independent of quad-vs-scissor geometry, pass splitting,
  submit granularity and texture format. Consequences: composite agreement
  tests VERIFY ON WARP when hardware disagrees (`assert_agrees`), and quad
  edges are never trusted at NDC 0 — all tile/blit draws use one oversized
  triangle clipped by an integer scissor rect (the quad version reproducibly
  dropped its first pixel column on seams at NDC x == 0). If the owner ever
  reports paint vanishing in small patches after layer operations, it is this
  driver: `--warp` confirms, a driver update is the real fix.
- **GPU tests are serialised by a mutex.** Creating several DX12 WARP devices
  from parallel test threads crashes the process inside the rasteriser.
- **`rfd` without `common-controls-v6`.** That feature imports comctl32 v6,
  which needs a side-by-side assembly manifest we do not ship — the exe then
  fails to start at all, with a loader error and no message.
- **`.cargo/config.toml` forces `link-self-contained=yes`.** w64devkit's gcc on
  PATH makes rustc assume a full system mingw and stop shipping its own runtime;
  w64devkit has no `libgcc_eh.a`, so every link dies with `cannot find -lgcc_eh`.

## Testing

- `cargo test` (via `./build.sh --test`): 83 tests — core 49 (tiles, undo, blend,
  stabilizer, ORA round-trip, export), brush 17 (preset loading for all 35
  classics, synthetic strokes, eraser, the stroke-start regression, the
  stabilizer+undo path), gpu 9 + 5 (CPU-vs-GPU compositing, incremental redraw).
- GPU tests **skip, not fail**, when no adapter can be created.
- Synthetic stroke harness: scripted `PenSample`s → brush → doc → assert painted
  bounds/alpha. No golden pixels — brush output tunes over time.
- `manganakama.exe --screenshot PATH` renders one real frame (canvas + egui)
  offscreen: the only way an agent session can inspect the UI.
- Correctness never needs eyeballs; only *feel* does (the owner's pen test-drives).

## Build

`./build.sh` — prepends `toolchain/w64devkit/bin` to PATH, then cargo.
Debug builds for iteration; `./build.sh --release` for pen test-drives (dab
compositing is hot, debug Rust is 10×+ slower). `play/manganakama.exe` is the
release copy the owner actually draws with.

## Version policy

wgpu version = whatever the current `egui-wgpu` release pins (check before
adding), so the egui integration never forks the dependency tree.
egui latest stable; `windows-sys` latest; edition per `cargo new` default.
