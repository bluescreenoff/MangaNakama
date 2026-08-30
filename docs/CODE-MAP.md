# Code map for contributors (human or AI)

This is not a folder tour — `docs/ARCHITECTURE.md` covers the crates and
the pinned tile/color model. This file is the list of **cross-file
invariants**: rules whose two halves live in different files, so a local
edit can be locally correct and still break the app *silently*. Audits
found that nearly every real defect in this codebase lived at one of
these seams, not inside an algorithm. Before changing an area, read its
entries; after changing one, say in the PR which entries you checked.

## The seam pattern (read this even if you skip the rest)

The recurring failure shapes, in order of how often they have shipped:

1. **A cache not invalidated through a door.** Page switch, document
   switch, tab switch, layer add/delete/move, undo — every one of these
   doors must reach every cache. New cache ⇒ enumerate the doors.
2. **State keyed by an index another subsystem shifts.** Layer indices,
   frame indices, page indices all move under reorder/insert/delete.
   Key long-lived state by identity or revision, or recompute.
3. **A flag that exists but doesn't gate the destructive path.** If you
   add a mode/badge/guard, grep for every path that should honor it.
4. **End conditions exempted from the interior's rule.** Stroke ends,
   first/last samples, edge endpoints, the last tile of a region — the
   interior's math must hold at the boundary too.

## Command dispatch (`cmd.rs` ⇄ `cmd/*.rs`)

- `cmd.rs` still owns the public seam: the `AppCmd` enum, the size/radius
  constants, and `dispatch` — the one entry every widget and shortcut goes
  through. The tool enums (`Tool`, `SubTool`, the mode enums, `ToolProps`)
  live in `cmd/tools.rs` and are re-exported whole, so every
  `crate::cmd::Tool` path still resolves; the same holds for the handful of
  helpers other modules address as `crate::cmd::…`. **Address them through
  `crate::cmd::`, not through the submodule** — the submodules are private
  and the re-export is what keeps a later re-shuffle from touching callers.
- The one `match cmd` is cut across ten domain modules (`history`, `pages`,
  `file_io`, `layers`, `frames`, `text`, `edit`, `transform`, `brush`,
  `misc`), each a `run(app, cmd, cmd_tail)` that matches its own arms and
  hands anything else to the next module in the chain. **A new `AppCmd`
  variant must be claimed by exactly one of them.** The compiler can no
  longer prove exhaustiveness across module walls, so an unclaimed variant
  reaches `misc`'s catch-all and panics there rather than doing nothing.
- `dispatch` runs a PROLOGUE (disarm one-shots, revert a live correction
  preview, snapshot dangling clips, tap the action recorder) and a TAIL
  (`sync_pages_palette`, the clip-change report, the recorded step). The
  tail travels with the command as `CmdTail` and a module runs it only
  where its `match` falls THROUGH: an arm's bare `return` still means
  "skip the tail", exactly as it did when this was one function. A new arm
  that returns early is opting out of the palette sync, the clip report and
  the action recording — which is sometimes right, but never accidental.

## Undo / operation recording (core `doc.rs` ⇄ everything that mutates)

- Every tile mutation must happen inside an armed op: `begin_op` arms
  the ACTIVE layer, `begin_op_on(li)` arms another one. Mutating a layer
  that is not the armed one silently produces an un-undoable change —
  undo then deletes or strands the art (this shipped once, as a
  folder-move undo that destroyed children's ink).
- **Never `mem::take` / swap a layer's tiles inside an op** — taking the
  tiles bypasses the copy-on-write recording. Write through the tile
  APIs so the pre-image is captured.
- Mask edits are recorded separately: `record_mask_change(layer, before,
  label)`. A structural op that also moves a mask must record both.
- A mask STROKE uses the bracket `mask_op_begin` / `mask_op_end`, and the
  begin half belongs next to `begin_op` in `begin_stroke` — the engine
  writes the mask's coverage tiles LIVE, per dab, so a snapshot taken at
  `end_stroke` is a snapshot of the finished stroke. `mask_op_end` pushes
  only when the coverage revision moved, so an empty gesture costs nothing.
- Undo is a DOOR: the GPU tile cache keys on the LAYER tile revision and
  folds the mask into the upload, so a mask restored over unchanged pixels
  needs `renderer.invalidate()` — `AppCmd::Undo`/`Redo` compare every
  layer's mask revision and call it. (A live-fill layer's derived raster is
  already self-healing: `fill_stamp` carries the mask revision.)
- One user gesture = one undo step. If your feature loops a per-item
  command, bracket it; N undo presses for one action is a bug (and a
  partial undo of a multi-part edit leaves mismatched state). For a
  multi-LAYER gesture the shape is: `begin_op_on(li)` … `end_op_take()`
  per layer, then ONE `push_compound` (see `apply_adjust_many`) — safe
  against index drift only because every index-shifting op clears the
  history.
- The palette multi-selection (`Document::layer_multi`) is index-keyed
  and rides the same door: `clear_history()` clears it. A new structural
  path that skips `clear_history` would leave BOTH the undo indices and
  the multi-selection stale — don't skip it.
- Recordable actions (`app::actions`) replay through `cmd::dispatch`,
  NEVER straight at the `Document` — the command arms carry the cache
  doors (evictions, thumbnail resets, frame renumbering) a bypass would
  skip. A structural run wraps in `UndoGroup::Structure` (whole-stack
  snapshot); its swap does NOT stamp tile revisions, and the cache
  uploads only on NEWER, so the Undo/Redo arms `renderer.invalidate()`
  when `next_undo_is_structure`/`next_redo_is_structure` says one is
  about to move. A new history-driving path must keep that peek-first
  shape.
- Vector layers (docs/VECTOR-INKING.md): a recorded stroke's pixels and
  its geometry ride ONE `UndoGroup::VectorStroke` — `end_op_vector_stroke`
  closes the op, never `end_op` + a separate record. Replay has two hard
  rules: enter at the ENGINE (`brush.begin/sample/end` — the captured
  samples are already canvas-space and resampled; `push_batch` would
  transform and resample them again), and on a FRESH engine from the
  recorded preset (libmypaint brush states persist across strokes by
  design; a same-engine replay starts mid-state and inks fatter —
  test-proven).

## Derived rasters (tone layers, frame borders, generated lines)

- Editing operates on the **source** tiles; everything that *reads*
  pixels (compositor upload, export, fill/wand sampling, eyedropper,
  clip bases) must go through `display_tiles()` / the display path, and
  `refresh_derived(dpi)` must run before any sampling or export.
  A new pixel consumer that reads `tiles()` directly will see stale or
  un-derived content only on tone/frame layers — the worst kind of
  sometimes-bug.

## Page identity (`app/pages.rs` ⇄ `core/project.rs` ⇄ every work writer)

- The page code sits in four files, split by responsibility: `app/pages.rs`
  (the page slots and the switch/stash/park machinery), `app/page_import.rs`
  (files coming IN as pages, and the underlay placement a draft scan lands
  under), `app/page_files.rs` (the work-folder save and its autosave twin)
  and `app/page_resize.rs` (the DPI resample and canvas resize that walk
  every page, parked or live). Every writer rule below applies across all
  four.
- `PageEntry::uid` is the page's identity — the key the park LRU, the
  reader's texture map and the MCP page lookups use, precisely because
  page INDICES move under reorder/insert/delete (seam pattern 2).
- It is **persisted** (`ProjectMeta::page_uids` for a single-file
  `.mnc`, `FolderPageMeta::uid` for a work folder). The ネーム promotion
  copies a work's identities into the promoted work, and the stamp back
  (`app/promote.rs`) matches on them; identities that do not survive a
  save would reduce that to page-ORDER matching, silently, months later.
  **A new writer of a work file must write them, and a new reader must
  adopt them through `App::adopt_page_uids`.**
- That adopt is also the only place the mint floor moves
  (`PageEntry::bump_uid_floor`). Adopting identities from a previous
  session without it lets the next `AddPage` mint one that a loaded page
  already holds — two pages of one work sharing an identity, which is
  what the uid exists to make impossible. `0` means "none recorded"
  (a work saved before the field existed) and always mints fresh.
- Two DIFFERENT works may legitimately hold the same identities — that
  is what the promotion produces — so nothing may key a cache on a uid
  across works. `forget_document_caches` clears the reader map on every
  tab switch; the park LRU is per doc slot.

## The frame reading-order cache (`app/frames.rs` ⇄ layer ops)

- `App::frame_order` is a cache of computed panel numbering, keyed by a
  document revision; `ensure_frame_order()` runs at the top of
  `App::render`. It holds **layer indices** — any new code path that
  adds/deletes/moves layers outside the normal command flow must bump
  the revision it checks, or badges land on the wrong panels.
- The ordering itself (`core/frame_order.rs`) has a contract: **geometry
  is the authority, division slots are only a tiebreak/validation
  signal, and an unresolved order is BADGED ambiguous — never silently
  guessed.** If your change makes some layout resolve "cleanly", prove
  the resolution is deterministic geometry, not a lucky sort order.
- Spread pages order absolutely (right page entirely first for RTL).
  No later pass may move a panel across the page seam.

## Brush engine (app ⇄ `mn-brush` ⇄ vendored libmypaint C)

- The vendored C is patched; every change carries a numbered entry in
  `vendor/PATCHES.md` and an `MN-PATCH` marker in the source. No entry,
  no edit.
- Vendor hooks that carry per-stroke modes are **thread-local and
  re-stated per `stroke_to`** — a process global races under the
  parallel test runner and clobbers a stroke mid-draw. Copy the
  existing pattern (hard-dab / scatter hooks) for any new mode.
- The NaN-pressure guard in `MyBrush::stroke_to` drops non-finite
  samples at the FFI boundary. Do not remove it: the corruption it
  prevents surfaces later, inside an unrelated preset's heap.
- The view transform reaches the C through `Viewport::brush_view()`, never
  the raw viewport fields: patch #12 knows only a HORIZONTAL mirror, so a
  vertical flip is handed to it as the equivalent mirror-plus-half-turn
  (and H+V as a plain half turn, which is not a mirror at all). A new view
  axis that skips this still paints — with every direction-mapped dynamic
  reading the mirrored angle.
- `mn-brush`'s CPU rasterizer is pinned ≤1 quantum against the C
  reference by tests; the GPU dab path is pinned against the CPU path.
  Chain of custody: C reference → CPU → GPU. Break one link and the
  parity tests are testing nothing.
- Texture tips have TWO anchor modes (PATCHES.md #10): canvas-anchored
  grain (default) and the dab-anchored STAMP, whose per-dab rotation is
  its own unfolded channel (`tex_angle`, snapshotted into the op and the
  record like the crawl offsets) — never the elliptical angle, which
  folds mod 180. The stamp sample is bilinear with identical arithmetic
  in C, `cpu_raster` and `dab.wgsl`, and the GPU reads CPU-precomputed
  sin/cos (GPU trig is too coarse for the parity bar). Changing any one
  of the three sampling paths without the other two breaks parity at
  rotation boundaries only — run `gpu_dab_parity_dab_anchored_stamps`.
- GPU dabs default by MEASUREMENT (`bench::resolve_auto`): an explicit
  choice (the `--gpu-dabs` flag or a `gpu_dabs=` line ui.txt actually
  carries — `gpu_dabs_explicit` is the tri-state) always wins; else a
  `gpu-verdict.txt` matching THIS adapter's fingerprint; else off + a
  one-shot `--bench-verdict` child measures for the next launch. An auto
  path must never overwrite the user's key — the verdict file and ui.txt
  are separate authorities on purpose. **The tri-state is the ABSENCE of
  the `gpu_dabs=` line, so `to_body` must not write the key until the user
  has actually chosen** (`note_gpu_dabs`, the View-menu toggle, is the only
  thing that makes it explicit). Writing it unconditionally is a silent
  one-way door: the first clean exit forges "he chose off", startup honours
  the forgery, the measurement child is never spawned again, and the
  measured default becomes unreachable on that machine. That shipped once —
  the owner's own ui.txt carried a `gpu_dabs=0` he never typed. Whatever
  decided is stated in ONE place, `bench::state_line`, which Preferences and
  the startup log both print; a new authority must appear there too or the
  feature goes invisible again.
- **Brush size is ONE absolute number in canvas px** (`ToolProps::size_px`,
  a dab diameter): the Size control, the `[`/`]` ladder and the live drag
  all write it, and only `Engine::set_size_px` converts. A second size
  model — a multiplier, a per-control clamp — puts a ceiling on the others
  without saying so, which is exactly what shipped before. The engine
  re-derives from the radius the preset shipped (`base_radius_log`, natural
  log), so setting the same size twice must equal setting it once; a setter
  that scales what it currently holds compounds on every slider tick. The
  preset's own size is the DEFAULT a sub tool seeds from, never a ceiling —
  which is exactly what the persistence rule leans on: ui.txt's
  `sub_tool_size_px=` stores ONLY the sub tools whose size the user moved off
  that default (keyed by the preset's path relative to the brushes root, so a
  moved install keeps them). An untouched sub tool has no entry and still
  seeds from its preset, so updating a preset moves its size; "back to the
  preset" DELETES the entry instead of writing today's default down.

## GPU compositor (`mn-gpu` ⇄ core CPU compositing)

- **The CPU path is the reference; the GPU path must agree at every
  alpha.** Blend operators are defined on premultiplied values exactly
  as the fixed-function states compute them — an operator that is only
  correct for straight color diverges on translucent sources.
- **Both compositors walk `Document::composite_order()`, never
  `doc.layers` directly.** The order is identity except FB-overflow
  ("Burst out of the panel"): an escaped child re-seats just above its
  `spill_anchor` — its own frame folder header by default, or higher
  when the layer's `draws_over` set (stable ids) names something above.
  A `CompositeStep` also carries a `SpillPart`: a breakout layer with an
  ENABLED layer mask appears TWICE, `In` (× `1 − mask`) at its own seat
  where the panel still clips it and `Out` (× mask) at the escaped seat.
  The GPU serves the `In` half from a third tile-texture variant
  (`TileVariant::HeldIn`) because it folds masks into uploads. A new
  walk that iterates the raw layer list re-clips escaped art on one path
  only — a parity break that only shows on pages using the feature
  (`cpu_matches_gpu_with_an_escaped_frame_child`,
  `cpu_matches_gpu_with_a_mask_capped_breakout`).
- **`LayerSig` must carry anything that moves a layer's SEAT.** The
  escape flag, the draws-over set and the resolved anchor move no tile
  revision at all, so `LayerSig::spill` is the only thing that damages
  the canvas when they change; without it the hardware path serves a
  stale composite (verified by disabling the field — the re-seat step of
  the parity test then fails on hardware).
- The laptop's 2020 Intel DX12 driver randomly drops one draw per
  rebuild frame. This is NOT a regression; agreement tests auto-verify
  on WARP, and the dab path has a canary counter + CPU-replay repair.
  Do not "fix" mysterious single-draw losses by restructuring draws.
- Windows-10-era WARP (driver 10.0.19041.x) LOSES THE DEVICE executing
  the blend2 shader pass — `MN_WARP=1` cannot test the shader blend
  modes on such machines (fixed-function modes work; CI's newer WARP
  runs everything). A lost device hands back invalid resources with no
  error of its own: the symptom is "buffer is invalid" far downstream.
  The device-lost callback now names it — believe that line, don't
  chase the buffer.
- Explicit bind-group layouts are REQUIRED for compute — auto-derived
  layouts produce silent no-op dispatches. Rust and WGSL structs must
  agree to the byte (padding included).

## The tile-kernel seam (`gpu/kernel.rs` + `kernel.wgsl` ⇄ core `adjust.rs`, `filter.rs`)

- **`filter.rs` is a directory module, and `Filter::reach` now lives in a
  DIFFERENT FILE from the kernels it describes.** `filter.rs` keeps the enum,
  the menu seeds, `reach`, `run`, `is_identity`, `separable_passes`, the
  gather/scatter and `Document::apply_filter*`; the kernels are
  `filter/blur.rs` (the box/Gaussian family, motion, radial, spin, unsharp),
  `filter/distort.rs` (FL-020..023) and `filter/lines.rs` (LC-001 dust,
  LC-002 line width). The halo rule is unchanged and is now a cross-file
  one: an arm of `reach` and the kernel it names must share arithmetic
  (`gaussian_reach`, `motion_span`, `wave_amplitude`, `dust_max`,
  `line_width_radius` are exported from the kernel files for exactly that).
  Understate `reach` and the outermost written pixels read halo, i.e. a
  seam at the region edge.
- `core/transform.rs` split the same way: `transform/resample.rs` holds
  `I-005`'s samplers (`Interp`, nearest/bilinear/bicubic/area) and
  `resample_tile_map`; `transform.rs` keeps the float lift/clear and
  `commit_transform`. `Interp` and `resample_tile_map` keep their old
  `mn_core::transform::…` paths through re-exports.
- **The CPU function is the specification, and every caller keeps it as
  the fallback.** `Document::refresh_corrections_with` and
  `Document::apply_filter_with` take a kernel the caller lends; it may
  decline at any moment (unsupported adapter, below the size floor, a
  dispatch canary failure mid-job) and the CPU reference then runs. Both
  entry points assemble GPU results OFF TO THE SIDE and hand them over
  only when the whole job passed — a declined job must leave the
  caller's pixels untouched, or the fallback filters already-filtered
  pixels (`a_dropped_dispatch_declines_and_leaves_the_caller_untouched`).
- **A GPU-derived correction tile carries the same freshness keys as a
  CPU one.** The stamps and per-tile `(max-rev, count)` keys are written
  identically whoever computed the pixels; a tile that did not carry
  them would re-derive every frame, or never.
- **`kernel.wgsl` transcribes core, it does not reinvent it.** The colour
  ops mirror `Adjust::map` expression for expression, and the tone
  curve's Fritsch–Carlson tangents come from `adjust::curve_tangents` on
  the CPU so the limiter has ONE implementation. Editing either side
  alone breaks parity; `gpu/tests/kernel_parity.rs` is what catches it.
- **The blur family is a CHAIN of integer passes, not one composite
  kernel.** `Filter::separable_passes` mirrors `box_radii` pass for
  pass, because each CPU pass re-zero-pads its own output and throws
  away the ink the previous pass pushed past the buffer edge. Folding
  the three boxes into one wide kernel is the same operator in the
  interior and a different one within `3 × reach` of every border —
  measured at 4015/32768 before the chain replaced it. The integer
  weights are also what make GPU parity exact rather than a tolerance.
- **Routing is a judgement, not a measurement** (unlike inking's
  per-adapter verdict): GPU when compute exists, the adapter is not a
  software rasterizer, and the job clears `KERNEL_FLOOR_PX`. The reason
  the dab verdict does not apply is written out in `kernel.rs`'s module
  docs — a kernel job is one upload/dispatch/readback for a whole page,
  not a stall inside an interactive stroke.
- Storage BUFFERS, not tile textures: a B4/600 page is 8598 px tall and
  exceeds this adapter's 8192 `max_texture_dimension_2d`. Region jobs
  chunk into horizontal bands with a halo equal to the summed pass
  reach; a band must read ORIGINAL pixels, which is the other reason the
  output is assembled separately.

## Input (`app/canvas_input.rs`, `input.rs`, `win32.rs`)

- Pen input is raw `WM_POINTER` history batches; mouse is one sample
  per `WM_MOUSEMOVE`. Mouse strokes get a smoothing floor at
  `begin_stroke`; pen strokes must NOT (the owner's presets ink with
  stabilizer 0 on purpose).
- Hit tests use screen-px tolerances divided by zoom. If you gate a
  hit test with a pre-check, the pre-check must use the SAME tolerance
  as the test it gates — a zero-tolerance gate in front of a 10 px hit
  test creates a band where the press does the wrong thing.
- Ink must never land on frame layers (`guard_frame_layer`); selection
  strokes are exempt (they paint the doc's scratch).
- Rulers are movable with the Object tool, and a ruler's geometry IS the
  ruler — a move needs no invalidation to change what the pen snaps to.
  The one exception is the SYMMETRIC ruler: the mirror twins are engines
  built from its centre and axes, so a move must `rebuild_twins()` or the
  next stroke mirrors about where the ruler used to be. Rulers park with
  their document, so an in-flight move index is cleared on a tab switch
  (`forget_document_caches`) like every other armed gesture.
- The ruler SET lives on the `Document` (`doc.rulers`) so the document's
  one undo history owns it: create / move / clear each record ONE
  `UndoGroup::Rulers` whole-set snapshot (`Document::record_rulers`), and
  a restore clears `ruler_lock`/`ruler_move` and rebuilds the twins
  (`cmd::resync_rulers`). Two rules ride with it. Rulers are **session
  only** — no encoder writes them, so any path that re-decodes the page
  the tab is already editing must go through `App::adopt_page_doc` or a
  page switch silently throws the set away. And the frame-PUBLISHED curve
  rulers are derived: `sync_frame_rulers` writes `doc.rulers` directly and
  must never record a step (its retract-by-value bookkeeping,
  `App::frame_rulers`, stays on the App and stays in step because ruler
  and frame snapshots interleave on the same history).
- **Smart Shape (row 156 / `FG-020`) presses Undo on the user's behalf**, so
  its interlock is a seam across three files. The gesture inks a real
  freehand stroke; if the hold recognizes a figure, `finish_smart_shape`
  (`app/canvas_input.rs`) undoes that stroke and inks the figure in its
  place, which is ONE net history entry only because two things hold at
  once. First, the undo goes through `cmd::dispatch(AppCmd::Undo)`, never
  `Document::undo` — the command arm carries the cache-invalidation doors.
  Second, the replacement ink must CLEAR the redo branch (`history::push_
  labeled`, both the raster and `end_op_vector_stroke` paths do); close that
  op with `push_undo_keep_redo` instead and a Ctrl+Y would paint the wobble
  back on top of the clean figure, silently and much later
  (`smart_shape_tests::the_swap_leaves_nothing_to_redo`).
  The interlock that decides whether to undo at all is `Document::op_count`,
  NOT `undo_len`: `undo_len` stops moving once the depth cap is reached, so
  a `undo_len == before + 1` test would refuse the swap for every user with
  a deep session — and a check that was merely wrong in the other direction
  would undo somebody else's operation.
- **Smart Shape's post-hold drag (`FG-021`) turns the pen off.** Once the
  hold has produced a figure (`SmartShape::armed`), pointer travel ADJUSTS
  that figure instead of continuing the stroke — so `canvas_move` and
  `canvas_up` must both skip `push_batch` while it is armed, or the pen
  keeps laying ink for a stroke that is about to be taken back off the page.
  The overlay and the commit stay in step for free because both read the one
  accessor, `SmartShape::preview`, which returns the ADJUSTED shape when
  there is one. Note the deliberate consequence: a hold that lands on a real
  shape can no longer be escaped by drawing on — the `smart_hold_ms`
  preference is the way out, and a hold that recognized NOTHING still
  disarms on the next move.
- **A work resample (`IO-060`) in flight is a modal state.** Phase 1 is
  chunked one page per frame (`App::resample_work_step`, driven from
  `App::render`) so the progress window can be painted at all, and its
  pending list is keyed by page INDEX. `cmd::dispatch` and `canvas_down`
  therefore refuse everything while `App::resample_job` is `Some`: a page
  turn or an undo arriving between two pages would install work built
  against a document set that no longer exists. Cancel is safe because
  phase 1 writes nothing into the work; phase 2 installs whole, inside one
  frame, so there is no half-installed state to cancel into. Abandoning a
  run must restore `pages[page_index].bytes = None` — the stash phase 1
  needed is the only mark it leaves.
- Selection coverage is weighted, not boolean, and selection ops go
  through coverage-based bounds — a new op that derives its region from
  one outline loop breaks multi-island selections.
- A PASTE with a selection active lands MASKED to it (owner 2026-08-21),
  and the halves live apart: `cmd/transform.rs`'s `TransformCommit` picks the shape —
  a paste that CREATES its layer gets a non-destructive layer mask built
  from the coverage (`fill_layer::mask_from_selection`), a paste that
  stamps an existing layer passes `mask_to_selection: true` into
  `commit_transform`, which clamps inside the commit's own op. Exactly one
  may fire per commit: both would weight a feathered edge twice. The tell
  for "this is a paste" is `clear_source == false` — clamping a LIFTED
  float (Transform, Flip) would erase the selection's own art the moment it
  moved. The mask reads the LIVE selection at commit, not a lift-time
  snapshot; only the source CLEAR needs to mirror the lift.
- **Colour mixing has ONE home: `core/src/mix.rs`** (rows 58 + 167).
  `MixMode` (gradient `G-009`) and the Oklab conversions live there and
  `gradient.rs` re-exports them; the brush's `BrushMix` (`I-014`) lives
  there too. They are not the same math on purpose — interpolating two
  authored colours is a lerp in a chosen space, mixing WET PIGMENT is
  subtractive, and the brush hands the second one to libmypaint's vendored
  spectral code (`paint_mode`, PATCHES.md #21) rather than re-deriving it.
  Do not add a third copy of Oklab.
- **`BrushMix::Perceptual` reroutes the stroke.** `dab.wgsl` has no
  pigment model, so `MyBrush::set_color_mixing` sets the `exotic` flag with
  the mode and `gpu_ready()` goes false. Any future brush mode the shader
  cannot express must do the same, in its own setter — routing decided
  anywhere else is a wrong blend that never errors.
- **`I-005` interpolation is scoped to the TRANSFORM commit.**
  `transform::Interp` reaches `commit_transform` only. The mesh/puppet path
  resamples through `mesh::warp_buffer`'s own bilinear and the Tool
  Property row disables itself there; export has its own kernel row
  (`export::Resample`, Comic/Photo) and `import_image_layer` is fixed at
  Lanczos3. Three resample seams, three separate choices — deliberately,
  because they answer to different dialogs.

## Text (`crates/text`)

- The ONLY COM-capable crate (DirectWrite); UTF-16 end-to-end. Do not
  introduce COM elsewhere, and do not round-trip text through UTF-8
  inside the text path.

## UI chrome (`app/ui/*`)

- Colors come from theme tokens in `ui/theme.rs`, read as `theme::c().<token>`
  — a hardcoded color in ui code will be wrong in one theme and invisible in
  review. There are three built-in themes (`dark` the default, `sepia`,
  `violet`), chosen by `theme=` in `prefs.txt` and switched live in
  Preferences ▸ Interface: pick with `theme::set*`, then call `theme::apply`
  so egui's own widget visuals follow. Canvas-SEMANTIC colours (marching
  ants, guides, ruler marks in `ui/overlay.rs`) are meaning, not decoration,
  and stay literal on purpose.
- **`ui/overlay.rs` is a directory module and its Z-ORDER lives in exactly
  one place** — `canvas_overlay`'s list of band calls. Each band
  (`overlay/page.rs`, `rulers.rs`, `selection.rs`, `frames.rs`, `tools.rs`,
  `text.rs`, `areas.rs`, `transform.rs`, `readouts.rs`) paints only what it
  is handed: the painter, `to_pt`, `cursor_pt` and the `ants` closure are
  built once in the entry and lent down, so a band cannot quietly re-derive
  a different screen mapping. A new overlay goes in the band it belongs to
  and gets ONE call in that list; adding a second call site for the same
  band is how a Z-order bug ships. `transform::paint` returns `true` for its
  mesh early-exit — the caller returns too, which is what keeps the reading
  order and the magnetic lasso unpainted in that state.
- `ui/layers.rs` is a directory module too: `layers/rows.rs` (stack rows,
  drag-drop, row menus, thumbnail caches), `layers/property.rs` (the Layer
  Property panel), plus the existing `breakout.rs`/`blendif.rs` sections.
  `layers.rs` itself keeps only what BOTH halves read — `BLENDS`,
  `blend_name`, `tools_for_layer`, `LAYER_TINTS` — and the pane entries
  (`layer_section`, `layer_property`) are re-exported from there, so
  `dock.rs` and `batch.rs` still see one module.
- **egui_dock must be styled from `Style::from_egui`, never
  `Style::default()`** — 0.21's default is a LIGHT style, and using it paints
  white tab bodies over a dark app (owner bug report 2026-08-16). `dock.rs`
  `dock_style` does this correctly and re-derives per frame, which is also
  why a live theme switch reaches the dock for free.
- `vendor/egui_dock` is patched (see PATCHES.md); dock ids, DnD payload
  keys, and the tear-off behavior are ours. Upstream-updating the crate
  without replaying the patches breaks palette drag silently.
- Persistence is `ui.txt` key=value lines. A persisted value must be
  mode-independent or persist its context with it (a bare index whose
  meaning depends on session-only state restores wrong). When a key
  changes meaning, RENAME it so stale values are ignored, not misread.
- Material TAGS live in a per-folder `tags.txt` sidecar
  (`app/materials.rs`, `MaterialTags`), not in `ui.txt` — the tags belong
  to the folder, so copying a material folder copies them and a rescan
  re-reads them. It is a shipped format: `<file name>=<comma, separated,
  tags>`, and BOTH kinds of unknown content survive a rewrite — a line
  this build cannot read, and an entry naming a file not in the folder
  right now (the owner's own tags must outlive a rescan or an unmounted
  drive). Clearing the last tag deletes the file, so "cleared" and "never
  tagged" are the same folder on disk.
- A material folder holds TWO kinds of material (`app/materials.rs`,
  `MaterialKind`): images, and generator materials — a `<stem>.gen.json`
  holding a serialized `GenLinesSpec`, whose same-stem PNG is only its
  thumbnail and must never scan as a second material. `PasteMaterial`
  routes a generator through `genlines_new_layer` in `cmd/history.rs` (never a
  bitmap decode), so the placed layer carries `Layer.genlines` and the
  Object tool edits it from the first click; that helper is also the
  dialog's Generate path, and it owns the `wrap_recent("Generate lines",
  2)` that keeps one placement at ONE undo press.
- A palette column can be COLLAPSED to an 18pt strip (`ui.rs`,
  `left_collapsed=` / `right_collapsed=`). Whatever writes a column's width
  down must skip a collapsed side — `UiLayout::note_widths` and the resize
  handles both do — or the strip's width becomes the stored column width and
  the column is a permanent sliver after the next launch. Collapsing also
  hides that column's torn-off FLOATING palettes: egui_dock only draws window
  surfaces from `DockArea::show_inside`, which a collapsed column skips.
- **A sub tool row exists in three places at once** (`cmd/tools.rs`
  `SubTool::ALL` ⇄ `subtools.rs` ⇄ `ui/subtool.rs`). `SubTool::ALL` is the
  enumeration; `subtools::group_of` files each row under a group caption and
  `subtools::registry` derives the tool → groups → rows tree from those two.
  The Sub Tool palette draws its captions from `subtools::group::*`
  constants, and `subtools::is_current` is the REVERSE of the row's
  `apply_state` arm. So: a new row needs an `ALL` entry, a `group_of` arm, an
  `apply_state` arm and an `is_current` arm. Miss `ALL` and the row is
  invisible to Ctrl+K, to `keys.json` and to the memory while still drawing
  fine; miss `is_current` and the row can be reached but never reports that
  you are standing on it, which silently breaks the shortcut CYCLE (it starts
  from the top every press) and the ui.txt memory (the group stops being
  written down). `subtools::tests::the_registry_holds_every_row_once` catches
  the first, nothing catches the second but review.
- The shortcut MEMORY (`sub_tool_last=` in ui.txt) is a SNAPSHOT of live
  state taken beside the save (`subtools::note_memory`, called from
  `ui::build` and `WM_DESTROY`), never a hook on the switch. That is
  deliberate: the mode fields move from a dozen places (`,`/`.`, Tool
  Property, the palette, a shortcut) and a snapshot cannot fall behind them.
  It MERGES, because a tool has one mode field and the tab you are not in
  can only be remembered by the file. The playback
  (`subtools::restore_from_memory`) runs once in `main`, NOT in `App::new` —
  in `App::new` the test suite would inherit the developer's own ui.txt.
- A workspace entry (`app/workspaces.rs`) is a VARIABLE-LENGTH `Vec<String>`,
  never a fixed-size array, and every field is read through `ws_field`. A
  `[String; N]` makes serde reject the whole line the day the entry grows —
  and the parse is an `unwrap_or_default()`, so every saved workspace would
  vanish without a word. New fields go on the END only.

## File objects (core `file_object.rs` ⇄ `ora.rs` ⇄ `app.rs` ⇄ `layers.rs`)

- `LayerKind::FileObject` is the ONE non-`Raster` kind whose pixels live in
  the layer's ordinary `tiles` instead of a derived cache. Everything good
  about the feature depends on that: the ORA save writes the raster for
  free, a broken link keeps its last picture, and an older build opens the
  page as a plain raster showing the right image. Move those pixels into a
  derive cache and all three break at once, silently.
- Consequence in the ORA LOADER: the file-object arm must sit AFTER the
  frames/balloons/texts/fill/correction chain — the same place `genlines`
  sits — never inside it. Inside the chain the layer PNG is never decoded,
  so a page opened on a machine without the source comes up BLANK, which is
  exactly the case the saved raster exists for. `mnc-fileobj` carries the
  recipe only.
- `FileObject::missing` is `#[serde(skip)]` on purpose: it is a fact about
  this machine right now. Persisting it would tell the studio desktop that
  a background it can see perfectly well is gone.
- The brush guard is NOT `paintable()`. `paintable()` (false here, via
  `is_vector()`) refuses fill/gradient/transform/filter/clear/merge, but the
  MyPaint engine writes tiles directly and never asks it — so the stroke
  refusal is an explicit arm in `App::begin_stroke`, beside the maskless
  live-layer refusal. A new derived kind needs BOTH.
- A refresh records no undo (external truth, CSP's rule) and calls
  `Document::touch()` only when something changed — an idle alt-tab must not
  dirty the document, because `WM_SETFOCUS` runs the refresh on every one.
- The palette row is the only place a file object announces itself
  (`row_glyph` → `Icon::FileObject`, red + "file missing" when the link is
  broken). There is no modal at load time, by design: the mark IS the
  notification.

## Tests and process

- `./build.sh --test`, zero warnings, before every PR. GPU tests run on
  WARP when no hardware adapter exists; WARP tests serialize behind a
  mutex — don't remove it.
- **A fix ships with a test that failed against the old code** — run it
  both ways and say so in the PR. A test that never failed pins nothing.
- The owner draws with `play/manganakama.exe` (a standalone copy).
  Never point him at `target/`'s exe: his open file handle kills the
  next build.
