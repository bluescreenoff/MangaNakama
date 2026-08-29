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
  frame folder header at the header's depth. A new walk that iterates
  the raw layer list re-clips escaped art on one path only — a parity
  break that only shows on pages using the feature
  (`cpu_matches_gpu_with_an_escaped_frame_child`).
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
- Selection coverage is weighted, not boolean, and selection ops go
  through coverage-based bounds — a new op that derives its region from
  one outline loop breaks multi-island selections.
- A PASTE with a selection active lands MASKED to it (owner 2026-08-21),
  and the halves live apart: `cmd.rs`'s `TransformCommit` picks the shape —
  a paste that CREATES its layer gets a non-destructive layer mask built
  from the coverage (`fill_layer::mask_from_selection`), a paste that
  stamps an existing layer passes `mask_to_selection: true` into
  `commit_transform`, which clamps inside the commit's own op. Exactly one
  may fire per commit: both would weight a feathered edge twice. The tell
  for "this is a paste" is `clear_source == false` — clamping a LIFTED
  float (Transform, Flip) would erase the selection's own art the moment it
  moved. The mask reads the LIVE selection at commit, not a lift-time
  snapshot; only the source CLEAR needs to mirror the lift.

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
  routes a generator through `genlines_new_layer` in `cmd.rs` (never a
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
- **A sub tool row exists in three places at once** (`cmd.rs`
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

## Tests and process

- `./build.sh --test`, zero warnings, before every PR. GPU tests run on
  WARP when no hardware adapter exists; WARP tests serialize behind a
  mutex — don't remove it.
- **A fix ships with a test that failed against the old code** — run it
  both ways and say so in the PR. A test that never failed pins nothing.
- The owner draws with `play/manganakama.exe` (a standalone copy).
  Never point him at `target/`'s exe: his open file handle kills the
  next build.
