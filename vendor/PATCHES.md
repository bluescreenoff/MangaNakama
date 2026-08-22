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

### 14. Dragging a floating window's tab strip MOVES the window (`show/mod.rs`, `show/leaf.rs`, `show/window_surface.rs`, `state.rs`, 2026-08-21)

**What:** a tab drag whose SOURCE surface is a window (not the main surface)
now drives that window's existing `WindowState` — position += the frame's
pointer delta, payload kept alive, `reset_drag()` on release — instead of
falling through to patch #3's tear-off. Pieces:

- `float_drag_moves_window` (show/mod.rs) decides per frame. It returns
  `false` — i.e. upstream behaviour — for a main-surface drag, for a hover
  over another surface (re-docking the float into a column), for a hover over
  a DIFFERENT node of the same window, and for a tab-strip hover on its own
  leaf when that leaf has more than one tab (a reorder). Everything else,
  including "the pointer is over nothing at all", moves the window.
- `drag_move_window` re-reads the window's CURRENT rect from egui memory each
  frame (`Memory::area_rect`) rather than accumulating: `create_window`'s
  `constrain_to(window_bounds)` clamps a fast drag at the screen edge, and an
  accumulator would keep running past the clamp and leave the window stuck
  until the pointer came all the way back. The id it reads under is now
  `window_surface::window_area_id`, extracted from `show_window_surface` so
  the two spellings cannot drift (they would drift silently — the window
  would simply never move).
- `State::float_move` (state.rs) tells the leaf renderer to skip the drag
  ghost's `transform_layer_shapes`: the whole window already follows the
  pointer, so offsetting the tab by the same accumulated delta on top carried
  it away at twice the speed. Cleared by `reset_drag`.

**Why:** the app floats palettes as `Surface::Window` entries with
`title_bar(false)` and `fill_tab_bar = true`, so the tab strip IS the
window's title bar — grabbing it must feel like grabbing a title bar. It
didn't: every such drag ended in patch #3's no-hover fallback
(`TabDestination::Window` → `detach_tab`), which builds a BRAND NEW surface at
the pointer with a reset position and size while `move_tab`/`detach_tab`
collect the old one. The owner screenshotted the result on 2026-08-21 — a tab
torn out of its own window, the window's geometry gone.

**Upstream-relevant:** arguably yes for any host that turns the window title
bar off; the `owner`-id plumbing it sits beside is ours.

### 16. Per-tab "may these share a tab bar" filter (`tab_viewer.rs`, `show/mod.rs`, 2026-08-21)

**What:** new `TabViewer::can_tab_into(&self, tab, dst_tabs) -> bool`
(default `true` = stock behaviour), consulted at the DROP COMMIT in
`show/mod.rs`: a resolved `TabDestination::Node(_, Insert|Append)` whose
destination leaf refuses the dragged tab is REWRITTEN — to a
`TabDestination::Window` at the pointer when `allowed_in_windows` says the
tab may float (dropping a palette over the canvas pane keeps behaving like
patch #3's tear-off), else to `None` (the canvas pane snaps back). Split
destinations, window drops and surface drops are never filtered. Patch #3's
release-over-no-leaf tear-off is also gated on `allowed_in_windows` now: a
tab the viewer barred from windows never floats from any path.

**Why:** docking-2 makes the canvas a pane in the same tree as the
palettes. A palette tabbed OVER the canvas leaf would bury the drawing
surface behind a tab; a canvas tab joining a palette stack is equally
wrong. Splitting beside either is exactly the layout feature, so only
tab-bar joins are vetoed. Known cosmetic gap, accepted: the hover overlay
still highlights the vetoed tab-bar target; the veto sits at the commit,
where source and destination leaves can both be read immutably.

**Upstream-relevant:** plausibly (a generic "tab classes" feature), though
upstream would likely want the overlay suppressed too.

### 17. Tree grafting + window absorption (`tree/mod.rs`, `dock_state/mod.rs`, 2026-08-21)

**What:** `Tree::graft_at(at, &Tree)` — overwrite the subtree rooted at a
node with a copy of another tree (heap-index-mapped copy; clears the old
subtree first; grows the node vec by whole levels like `split()`); and
`DockState::absorb_windows(DockState)` — move another state's floating
`Surface::Window` entries across intact (tree, position and size).

**Why:** the docking-2 ui.txt migration merges the two legacy dock COLUMNS
into the single tree. `split()` only accepts leaf nodes as the new child,
so there was no way to place an existing tree beside another; rebuilding
by walking leaves would drop nested split geometry the user made. Node
rects are stale after a graft — recomputed on the next laid-out frame,
same as after deserialization.

**Upstream-relevant:** yes in principle (tree composition is a real gap),
but the API would need bikeshedding upstream.

### 18. Drop-position UX: half-of-title insertion + root-edge split zones (`show/leaf.rs`, `show/mod.rs`, `drag_and_drop.rs`, `dock_state/mod.rs`, 2026-08-22)

**What:** three related drop fixes, one owner report ("I can't drop a pane
as the RIGHTMOST tab — it always lands in the middle; and I can't drag a
pane to the far left of the screen to make a new column").

1. `title_insert_half` (`show/leaf.rs`): hovering a tab title during a drag
   picks the insertion side from the hovered HALF — left half inserts
   before that tab, right half after it (index+1). The stored indicator
   rect is the chosen half. Needed because our tab bars are FILLED: titles
   stretch across the whole bar, so there is no empty-bar area to drop past
   the last tab, and upstream's "insert before the hovered tab" could never
   produce the last slot.
2. `move_tab` `TabInsert::Insert` (`dock_state/mod.rs`): same-node indices
   now mean PRE-removal positions (index decremented when the source sat
   left of it). Without this a same-node move right landed one slot short,
   and dropping a tab on its own right half swapped instead of no-op.
3. Root-edge drop zones (`drag_and_drop.rs` `edge_split_zone`/`EDGE_ZONE_W`,
   `show/mod.rs`, `DockState::move_tab_to_root_split`): during a dock drag,
   24 pt strips along the dock area's left/right edges are drop targets
   that split the ROOT — the dragged tab becomes a brand-new OUTERMOST
   column (fraction 0.2 for the new side, set explicitly because
   `Tree::split`'s fraction is by-position — the recorded 2026-08-21 trap).
   Takes precedence over leaf hovers and patch-#14 float-window moves.

**Tests:** `title_halves_pick_the_insertion_side`,
`same_node_insert_uses_pre_removal_positions`,
`root_split_makes_an_outermost_column`, `edge_zones_are_the_outer_strips_only`.

**Upstream-relevant:** 1 and 2 yes (filled tab bars exist upstream via
`fill_tab_bar`; the same dead-slot bug applies); 3 is arguably a feature PR.

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

**Amendment (P4 colorize/posterize port, 2026-08-21):** the tap gained
three trailing args — `op->colorize`, `op->posterize`, `op->posterize_num`
(the latter already `CLAMP(ROUND(num*100), 1, 128)` by `process_op`) — so
the recorded `DabParams` can drive the GPU Color/Posterize stamps. Only the
spectral `paint` mode still routes CPU-side via `exotic`.

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
