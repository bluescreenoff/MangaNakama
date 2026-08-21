# Vendor patch log

Every modification to vendored third-party code gets a dated entry here, same
session it's made. Format: file, what changed, why, upstream-relevant or not.

## vendor/libmypaint (v1.6.1, cloned 2026-08-13)

### 2026-08-13 — json-c made optional (`MYPAINT_CONFIG_USE_JSON`)

**File:** `vendor/libmypaint/mypaint-brush.c` (5 hunks, all `#if` guards — no
upstream line was deleted or rewritten).

**What:** every reference to json-c is now inside
`#if MYPAINT_CONFIG_USE_JSON`, with an `#else` branch that provides a stub
`mypaint_brush_from_string()` returning `FALSE`. The guarded hunks are:

1. `#include <json.h>`
2. the `json_object *brush_json;` field of `struct MyPaintBrush`
3. `self->brush_json = json_object_new_object();` in `brush_new()`
4. `json_object_put(self->brush_json);` in `brush_free()`
5. the whole preset-parsing block (`obj_get`,
   `update_brush_setting_from_json_object`, `update_brush_from_json_object`,
   `mypaint_brush_from_string`)

`MYPAINT_CONFIG_USE_JSON` is defined to `0` in the generated `config.h`
(see below), so the stub branch is what we compile.

**Why:** json-c was libmypaint's only external library dependency, and it is
needed by exactly one entry point. MangaNakama parses `.myb` JSON in Rust with
`serde_json` and applies presets through the public setter API
(`mypaint_brush_set_base_value`, `set_mapping_n`, `set_mapping_point`), which is
what `mypaint_brush_from_string` does internally anyway. Vendoring and building
json-c under w64devkit for one function was not worth it.

**Upstream-relevant:** no (a downstream build choice, not a bug fix). The guards
are written so `-DMYPAINT_CONFIG_USE_JSON=1` restores stock behaviour if json-c
is ever vendored.

**Silent-failure note:** `mypaint_brush_from_string` now always returns `FALSE`.
Nothing in MangaNakama calls it. If a future agent does, it gets an empty brush
and no error — use `mn_brush::MyBrush::load` instead.

### 2026-08-13 — autotools-generated files replaced by `build.rs` codegen

**Files:** none in `vendor/` — this is a *note*, not a patch. The v1.6.1 **git
tag** ships neither `configure`'s `config.h` nor the two headers `generate.py`
produces from `brushsettings.json`. Rather than re-vendor the release tarball
(which ships them) or require Python at build time (this machine has no
`python` on PATH), `crates/brush/build.rs` writes all three into `OUT_DIR`
before compiling:

- `config.h` — `GETTEXT_PACKAGE`, `MYPAINT_CONFIG_USE_GLIB 0`,
  `MYPAINT_CONFIG_USE_JSON 0`; `HAVE_GETTEXT` deliberately left undefined so
  `mypaint-brush-settings.c` takes its no-i18n branch. No gettext, no
  GObject-introspection, no OpenMP (we never pass `-fopenmp`, and every
  `#include <omp.h>` upstream is already `#ifdef _OPENMP`-guarded).
- `mypaint-brush-settings-gen.h` — the three public enums
  (`MyPaintBrushInput`, `MyPaintBrushSetting`, `MyPaintBrushState`).
- `brushsettings-gen.h` — the two internal static info arrays.

Both headers are a faithful Rust port of `generate.py`'s output format. They are
included with `"quotes"`, and no copy exists inside `vendor/libmypaint`, so the
`OUT_DIR` copies are what the compiler finds.

**Why this way:** the same pass also emits `settings_gen.rs`, so the Rust
setting/input ids and the C enum come out of one source file in one run. That
makes "the Rust ids must match the C enum order" structurally true instead of
hand-maintained — the failure mode it prevents (applying a `.myb` value to the
wrong setting) produces no error, only a wrong-feeling brush.

**Upstream-relevant:** no.

### 2026-08-16 — absolute-pixel `radius_by_random` mode

**File:** `vendor/libmypaint/mypaint-brush.c` (2 hunks).

**What:** an extern hook `float mnc_brush_radius_random_abs_px(void)`
(declared after `config.h`, implemented in Rust in
`crates/brush/src/mybrush.rs`) gates a second branch inside the
`radius_by_random` block of `prepare_draw_dab`. When the hook returns > 0,
the gaussian deviation the setting produces is interpreted as **canvas
pixels added to the current dab radius** instead of stock noise on the
log-radius. The alpha-conservation correction is kept for both branches.

**Why:** stock `radius_by_random` multiplies the radius (`exp(log + noise)`),
so the same setting looks calm on a 3 px brush and ragged on a 40 px one —
a feature request (round 19) asked for the variance to be size-independent,
with a pressure curve on top (the curve needs no C change: the setting value
is mapping-driven like any other).

**Upstream-relevant:** arguably (an "absolute random size" mode is a
reasonable upstream feature), but the extern-fn coupling is ours. The flag
is per-`stroke_to` call, set by the Rust brush from its own field before
each FFI entry, so brushes cannot leak modes into each other on the single
engine thread. Default 0 = stock behaviour, bit-for-bit.

---

## egui_dock 0.21.1 (vendored 2026-08-16, round 22 — "can't drag palettes")

Vendored to `vendor/egui_dock/` and wired via `[patch.crates-io]` in the
workspace root. All changes are marked `MN-PATCH` in the source. Context:
the app shows TWO sibling `DockArea`s (left/right palette columns, each with
its own `DockState`); upstream assumes one area per screen.

### 1. Global drag-and-drop payload keys (`show/mod.rs`, `leaf.rs`, `main_surface.rs`)

**What:** the `drag_data` / `hover_data` egui temp-memory payloads are keyed
by fixed global ids (`egui_dock::mn_dnd::*`) instead of per-`DockArea.id`.
`DragData` gained `owner: egui::Id` (which area started the drag) and
`HoverData` gained `owner: egui::Id` (which area published the hover). The
non-owning area re-inserts both payloads after its read instead of eating
them.

**Why:** with per-area keys a tab dragged in one column is invisible to the
other; worse, only the owning area's `try_initiate_tab_drag` keeps the drag
payload alive each frame, so the sibling's read consumed and dropped the
hover the owner's own leaves had just published — every drop degenerated to
the no-hover fallback. (Owner bug report: tabs wouldn't drag/dock at all.)

### 2. Owner guard (`show/mod.rs`)

**What:** only the `DockArea` whose id matches `DragData.owner` processes
the drag. Foreign payloads are restored untouched.

**Why:** with global keys both sibling areas see every drag; the foreign
one would index node/surface paths of a tree it does not have (panicked:
`index out of bounds` on window drags).

### 3. Release-over-no-leaf = tear-off (`show/mod.rs`)

**What:** on primary release with NO hover payload (pointer over the
canvas, the gap between columns, anywhere without a dock leaf), the drag
resolves to `TabDestination::Window` at the last hover position instead of
silently snapping back.

**Why:** upstream only ever drops onto a leaf, so "drag a tab out to float
it" was impossible outside the dock itself — the exact feature the app's
docking round advertised.

### 4. Foreign hover filter (`show/mod.rs`, `leaf.rs`)

**What:** the owner only accepts hovers published by its own tree
(`HoverData.owner`); leaves publish hover during ANY in-flight dock drag
(gated on the global drag payload existing), not just one started by their
own area.

**Why:** node indices do not translate between the two `DockState`s. A
foreign hover indexed by the owner panics; ignored, a cross-column release
now gracefully floats the tab (drop it back onto its own column — or onto
a drop button — to re-dock/merge).

### 5. Tab drag threshold 30px/6px → 8px/8px (`leaf.rs`)

**What:** `try_initiate_tab_drag`'s movement threshold lowered to egui's
own drag threshold.

**Why:** 30px horizontal on compact palette strips feels like the tab is
stuck; the owner reported tabs as "not draggable", and the long threshold
made small correction drags do nothing.

### Companion app-side change (not a patch)

`ui/dock.rs` gives each column its OWN `DockArea` id (`mn.dock.left/right`).
Both areas previously shared upstream's default id, so their tab widgets
collided in egui's widget store every frame — the left column's tabs
literally lost their hit rects to the right column's (egui even painted its
id-clash warning: a red line along the top of the old standing screenshot
baseline, gone in the new one).

### 6. Root node claims the full available rect (`show/mod.rs`, 2026-08-16)

**What:** `allocate_area_for_root_node` now `ui.allocate_rect`s the FULL
`available_rect_before_wrap()` instead of the dock-area-padded `rect` (the
tree still lays out inside the padded rect).

**Why:** a resizable host `egui::Panel` re-stores its size every frame from
the content-sized `Frame::show` response rect. Upstream's padded allocation
made the dock report a min size 1pt narrower than the panel's allocation, so
each stored size was 1pt smaller than the last — a palette column dragged
wider visibly ratcheted closed, 1pt per repaint, until it hit `min_size`
(owner report 2026-08-16, reproduced and verdict-pinned by
`--e2e-paneresize`). Claiming the full rect makes the stored size a fixed
point. Upstream-relevant: yes, any resizable-panel host has this ratchet.

### 7. Tab titles ellipsize when the strip squeezes them (`show/leaf.rs`, 2026-08-16)

**What:** `tab_title` builds the title galley with `TextWrapMode::Truncate`
bounded to `preferred_width - close_button - spacing` instead of
`f32::INFINITY` wrap.

**Why:** upstream centers the FULL galley in the tab's text rect, so whenever
`fill_tab_bar` divides a narrow bar between tabs, titles stick out over the
close button and the neighbouring tab (owner report 2026-08-16, pic 2 —
"Sub Tool" over the ×). With this + `fill_tab_bar = true` app-side, squeezed
tabs read "Tool Prope…" like every desktop tab bar.

### 7b. Close × on the active tab only (`show/leaf.rs`, round 28)

**What:** a tab's close × renders only when the tab is ACTIVE or hovered
(last frame's hover, read from `ctx().read_response(id)` before layout);
inactive tabs at rest show no ×.

**Why:** with one × per tab, an ellipsized strip reads as a row of stray ×s
(owner report 2026-08-17: "two Xs close buttons horizontally in a row" under
the window's own close). CSP shows the × on the active tab only; the
hover-reveal keeps background tabs directly closable. At rest exactly one ×
per strip — the active tab's.

## vendor/libmypaint — round 25 (2026-08-16): Krita-inspired dab modes

### 8. Hard stamp dabs (`mypaint-tiled-surface.c`)

**What:** `render_dab_mask`'s per-pixel opacity becomes, when the
`mnc_brush_hard_dab()` hook returns non-zero, `clamp(radius*(1-rr)+0.5, 0, 1)`
— an exact anti-aliased disc (coverage by pixel distance from the edge) —
instead of the two-segment gaussian hardness falloff.

**Why:** Krita/CSP pens ink with hard bitmap-ish tips; a gaussian cannot
reach that edge profile at any hardness. This is the missing "bitmap dab
tip" the csp/Real G-Pen import noted as unmapped. Opt-in: the Rust side
(threads-local flag, re-stated per stroke_to) leaves every stock preset
pixel-identical when off — pinned by a regression test.

**Upstream-relevant:** arguably — a `dab profile` option upstream lacks.

### 9. Scatter (`mypaint-brush.c`)

**What:** in `prepare_draw_dab`, when `mnc_brush_scatter() > 0`, each dab's
centre jitters by a uniform point in a disc of `radius*scatter` (angle
uniform, radius sqrt for area-uniform) around the stroke path.

**Why:** Krita's Scatter — sprayed/sketchy strokes. Distinct from
`offset_by_random` (which shifts the whole stroke gaussianly).
Opt-in exactly like #8.

### 10. Texture tips (`mypaint-tiled-surface.c` + `mypaint-brush.c`, round 26)

**What:** when `mnc_brush_texture_size() > 0`, `render_dab_mask` multiplies
every dab pixel's opacity (gaussian or hard profile) by a grayscale mask
sampled in **canvas space** — `(canvas_px + scroll) % tex_size`, wrapping.
`render_dab_mask` gained `tile_tx/tile_ty` params (both in-tree call sites
updated, plus the `tiled-surface-private.h` declaration) precisely so the
mask can be sampled canvas-anchored without a surface back-pointer.
`prepare_draw_dab` calls `mnc_brush_texture_advance()` once per dab; the
Rust side owns the accumulator and publishes the offset — advancing there
(not in `render_dab_mask`, which runs once per dab×tile) keeps a
multi-tile dab seamless.

**Why:** Krita's texture tips / gbr masked dabs. DELIBERATE DEVIATION:
Krita anchors the pattern per dab, which makes overlapping dabs fill the
pattern in along a stroke; we anchor to the canvas, so the mask reads as
stable paper grain / tone — the manga-useful behaviour. The optional crawl
(`mn-texture-scroll`, mask px per dab, fixed diagonal direction) restores
the per-dab spray feel when wanted.

**Upstream-relevant:** arguably, minus the extern-fn coupling.

**#0.1 AMENDMENT (2026-08-17, GPU texture tips):** the crawl offset is now a
**draw-time snapshot carried in the op** (`OperationDataDrawDab.tex_dx/dy`,
filled in `draw_dab_internal` right before the #11 record tap;
`render_dab_mask` takes the offsets as parameters instead of reading the
accumulator). Latent bug this fixes: the op queue defers mask rendering to
tile-process time (end_atomic), where the accumulator has advanced past the
dab — every dab of one `stroke_to` rendered at the batch's FINAL offset, so
the crawl was per-SAMPLE, not the per-dab this patch documents. The GPU
record reads the same thread-locals at the same point, so CPU and GPU agree
by construction; the pure-Rust repair rasterizer (`cpu_raster`) applies the
same multiply from `DabParams::tex_off`. `get_color`'s call site reads the
accumulator directly (it runs immediately, at draw time). Pin:
`gpu_dab_parity_texture_tips` (≤1/channel; repair path 0) — before the fix
the GPU's true per-dab offsets vs the CPU's per-sample render diverged at
32756/3022 channels.

**AMENDMENT 2 (2026-08-21, dab-anchored stamps + per-dab rotation):**
`mnc_brush_texture_anchor_dab()` selects a second sampling mode in
`render_dab_mask`: the mask covers the DAB'S bounding square (a stamped
tip, the Photoshop/Krita behaviour this patch originally deviated from)
instead of wrapping over the canvas. The stamp ROTATES per dab by its own
angle channel: `mypaint-brush.c` hands the Rust side the UNFOLDED stroke
direction beside the crawl advance (`mnc_brush_texture_stamp`), Rust
computes base ± direction and publishes it, and `draw_dab_internal`
snapshots it into the op (`tex_angle`) exactly like the crawl offsets —
because `ACTUAL_ELLIPTICAL_DAB_ANGLE` folds mod 180 (right for a
symmetric ellipse, useless for a stamp: 0 and 180 would render the
same). Sampling is BILINEAR with texel centres at +0.5, the identical
arithmetic in all three implementations — nearest sampling let a 1-ulp
trig skew flip whole texels at rotation boundaries, and the GPU ships
CPU-precomputed sin/cos in the record (GPU trig intrinsics are orders
coarser than libm; `GpuDab` grew to 68 bytes for the pair). Outside its
square the stamp is over (opacity 0), never wrapped. `.myb` keys:
`mn-texture-anchor: "dab"`, `mn-texture-rotate: "direction"`,
`mn-texture-angle` (degrees). Pins: `dab_anchored_stamps_rotate_with_the_dab`
(0 vs 180 genuinely differ; direction mode mirrors with the stroke) and
`gpu_dab_parity_dab_anchored_stamps` (C → CPU repair → GPU ≤1 quantum on
a curved stroke with per-dab angles).

**AMENDMENT 3 (2026-08-21, pure stamp):** in dab-anchored mode the tip mask
**IS the coverage** — `render_dab_mask` no longer multiplies the radial
profile (or the hard-dab disc) into an anchored stamp; opacity is the
bilinear tip sample alone. The owner's `.abr` eye test proved the old
compose wrong: every stamp rendered as a giant disc with tip texture only
at its edges, because the profile dominated (Photoshop/CSP treat a sampled
tip as the dab's SHAPE, not as a texture on a disc). With the profile gone
the stamp's corners must survive rotation, so every `radius + 1` fringe
becomes `radius * sqrt(2) + 1` **in anchored mode only** (`render_dab_mask`
bbox, `draw_dab_internal` tile queue, `update_dirty_bbox`; mirrored in
`cpu_raster::dab_tiles`/`rasterize_dabs` and `dab.wgsl`'s cull +
`gpu::dabs::dab_tiles` — all three implementations change together or the
parity tests catch it). `get_color`'s smudge-sampling kernel keeps the
disc fringe (a sampling weight, not visible ink). Companion import fix:
`.abr`/`.sut` default sizes cap at `MAX_DEFAULT_PX` (300) with an honest
"authored at N px" note — Painter-style sets author kilo-pixel tips and a
brush that *selects* at 985 px reads as broken. Pins:
`anchored_stamp_mask_is_the_coverage` (corner at ~1.24 r inks full, one px
outside the square is dry) plus the amendment-2 parity pair still ≤1.

### 11. Record mode for GPU dab compositing (`mypaint-tiled-surface.c`, round 27)

**What:** `draw_dab_internal` gains a tap after its early-outs and clamps
and before the per-tile op queue: when `mnc_record_dab_mode()` returns 1
(TAP) it calls `mnc_record_dab(...)` with exactly the clamped, converted
values the rasterizer sees (colours already fix15) and rasterizes normally;
mode 2 (BYPASS) records and skips the queue entirely (the P1 compute path
rasterizes from the record instead); mode 0 is stock, bit-for-bit.

**Why:** the GPU-dabs design, phase 0 — the seam between "brush dynamics" and
"dab rasterization" is exactly the op-queue hand-off; recording there gives
the GPU path the same ordered dab list the CPU consumes, per tile. Rust side:
`MyBrush::{set_dab_recording, take_dab_record}` + `DabParams`/`DabRecord`
(pub — the P1 `mn-gpu` compute consumes them); the touched-tile range math
mirrors the C's `floor(floor(x ± r_fringe) / 64)` with `div_euclid`.

**Upstream-relevant:** no (a downstream acceleration seam).

### 12. View-aware legacy stroke entry (`mypaint-brush.c` + `mypaint-brush.h`, round 31)

**What:** `mypaint_brush_stroke_to_view` — the LEGACY `stroke_to` (identical
internal flags: `legacy=TRUE, linear=FALSE`, so dab counting and paint mode
stay bit-for-bit unchanged) with `viewzoom`/`viewrotation` surfaced as
arguments instead of the hardcoded 1.0/0.0.

**Why:** speed-dependent inputs (`SPEED1`/`SPEED2`, offset-by-speed,
direction filters) are computed as `step_dxy/dtime * VIEWZOOM` inside
`update_states_and_setting_values`. Through the plain legacy entry the zoom
is always 1.0, so drawing at 25% zoom makes the engine see 4× the document
velocity — every speed-mapped dynamic (velocity→Size in the owner's
milli-pen, speed→opacity in classic presets) fires as if the pen moved 4×
faster, and zoomed-out strokes come out bumpy/jagged when inspected at 100%
(owner report 2026-08-17). Direction inputs are likewise corrected for view
rotation. Rust seam: `MyBrush::set_view(zoom, rotation_rad)`, published per
input batch by the app (`App::push_batch`), fanned to symmetry/wrap twins.

**UNIT CORRECTION (auditor, 2026-08-17):** `viewrotation` is
**RADIANS**, not degrees. The C applies `DEGREES()` to the argument itself
(`update_states_and_setting_values`: `mod_arith(DEGREES(step_viewrotation)
+ 180.0, 360.0) - 180.0`), and MyPaint's own caller passes `tdw.rotation`,
radians — upstream's "@viewrotation: View rotation in degrees" docstring
(the one this patch's header originally copied) is an upstream
documentation bug. Our first wiring passed `.to_degrees()` and fed 15° of
canvas rotation in as ~859 "radians"; every direction-mapped input
(`DIRECTION`, `DIRECTION_ANGLE`, `TILT_ASCENSION`,
`ACTUAL_ELLIPTICAL_DAB_ANGLE`) was angled wrong on rotated canvases
(rotation 0 unaffected). Fixed by passing `viewport.rotate_rad` raw, sign
included: our viewport's `screen = R(rotate_rad)·canvas` matches the C's
`dir_angle + viewrotation` arithmetic directly (pinned by
`direction_inputs_are_view_rotation_compensated` in mn-brush).

**Upstream-relevant:** functionally yes — it is `stroke_to_2`'s view
compensation without the non-legacy dab-count switch; upstream would
probably just tell callers to migrate to `_2`.

**FLIP EXTENSION (round 34, auditor item b):** `viewflip` (a `gboolean`,
last arg) mirrors the motion-direction inputs under a horizontally flipped
view. DERIVATION: the flip maps a doc angle θ to screen angle π−θ+r (r =
the flipped viewport's stored rotation — `Viewport::flip_around` negates
it while mirroring), and a constant `viewrotation` offset cannot express
an angle-DEPENDENT map. Negating the DX component of the direction state
vectors is exactly the 180−θ reflection, after which the existing
`+ viewrotation` arithmetic carries the rest with r raw — the same
raw-rotation rule as the rotation half. Scope: `DIRECTION` +
`DIRECTION_ANGLE` only (motion direction); `TILT_ASCENSION` and
`ACTUAL_ELLIPTICAL_DAB_ANGLE` stay rotation-compensated but not
mirror-compensated — the pen's device-space ascension does not mirror
when the VIEW does (known limit, recorded). `viewflip=FALSE` is
bit-identical to the pre-extension signature. Rust seam:
`MyBrush::set_view(zoom, rotation_rad, flip_h)`; pinned by
`direction_inputs_are_view_flip_compensated` in mn-brush (same-SCREEN
path at flip off vs on; without the negation the mirrored doc motion
reads ≈180° through a steep DIRECTION→Size curve and the dabs collapse
~5×).

### 13. RLE dab-mask buffer sized for its true worst case (`mypaint-tiled-surface.c`, 2026-08-21)

Upstream sizes the per-tile RLE opacity mask at
`TILE^2 + 2*TILE` `uint16_t`s, which assumes long runs — fine for every
smooth radial profile, and wrong the day #10 amendment 2 let a texture
mask BE the coverage. The encoding costs one entry per inked pixel plus
two per zero-run; a tip texture with hard black speckle (the owner's
不気味線 改 .sut) alternates ink/zero often enough that a single tile
needs up to `3/2 * TILE^2 + 2` entries. The overflow ran off the end of
a STACK array in `process_tile`/`get_color`, corrupting the operation
queue — observed as `end_atomic` claiming ~268 million dirty tiles and
grinding for minutes ("app stops responding at the first dabs"), then
STATUS_ACCESS_VIOLATION. Both local `mask` arrays now use the worst-case
bound (16 KB of stack, still trivial). `rr_mask` keeps the old size: its
writes are index-bounded, not run-length. Pinned by
`spotty_sut_tip_strokes_without_queue_corruption` in mn-app (real
fixture, skip-if-absent), verified crashing against the old bound.

### Companion Rust side (`crates/brush/src/mybrush.rs`)

`set_hard_dab`/`set_scatter` + readbacks; thread-local hook state per the
round-20 lesson (a process-global raced under parallel test runners).
Presets carry the modes as TOP-LEVEL `.myb` keys (`"mn-hard-dab": true`,
`"mn-scatter": 0.5`) so stock files stay untouched; `assets/brushes/krita/`
holds the two ports of the owner's G-Pen dynamics for A/B.

Round 26 additions: `set_wash` (flow-vs-opacity stroke buffer — pure Rust,
no C change) with `mn-wash`/`mn-wash-opacity`/`mn-brush-blend` keys;
`set_texture`/`set_texture_scroll` with `mn-texture`/`mn-texture-scroll`
(mask PNGs under `assets/brushes/textures/`); and the public
`mapping`/`set_mapping` pair (any setting × any input — the per-sensor
curve editor's seam).
