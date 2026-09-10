# Plan 2026-09-10: lag hunt (effect lines crash, pen readback stall) + doc.rs split

Owner (2026-09-10): "a lot of various manganakama actions trigger lag and stuff", "speedlines/radial
crash manganakama from slowness maybe the length is drawn too much", "refactor doc.rs a bit".
Budget for the whole round: **1M tokens across all agents + main chat**. Be token-frugal: read the
plan, read only the files it names, no re-deriving the codebase.

Machine rules (every agent): `CARGO_BUILD_JOBS=2` on every cargo command, `-- --test-threads=2` on
tests, NO full `./build.sh --test`, NEVER launch the app exe with a window (headless tests only;
the owner may be watching a movie). Free RAM is ~3 GB — one build at a time; waiting on the cargo
target lock is normal. Stage BY NAME, never `git add -A`. Save work to disk as you go and keep a
running "done / next" box at the top of your report file.

Evidence already in hand (`play/manganakama.log`, owner's real sessions on the Intel UHD 620):

```
gpu | 10 tiles, readback 63.6 ms      gpu | 2 tiles, readback 3.8 ms
gpu | 23 tiles, readback 49.1 ms      gpu | 8 tiles, readback 9.4 ms
[gpu] slow composite: 99.2 ms cpu-side (3 tiles, 0 uploads)
[gpu] slow composite: 957.7 ms cpu-side (12825 tiles, 30 uploads, full rebuild)
[gpu] composite session: 118 frames (2 full), avg 72.9 ms, max 957.7 ms, 80 slow >50ms
```

Readback time is BIMODAL (4-9 ms or 40-64 ms) and not proportional to tile count. 12,825 tiles =
a B4 600 dpi page. So: the wait is contention on the GPU queue, not the copy itself.

---

## LANE A — perf: measure, then fix the two known offenders (Opus)

Owns: `crates/app/src/app/diag.rs`, `crates/app/src/app.rs` (render + finish_gpu_dab_stroke only),
`crates/gpu/src/dabs.rs`, `crates/gpu/src/lib.rs` (FrameStats + timing only), `crates/core/src/genlines.rs`
and `crates/core/src/genlines/`, `crates/app/src/app/canvas_input.rs` (~4035 regen call),
`crates/app/src/cmd/text.rs` (~537 regen call), `crates/app/src/cmd/history.rs` (regen calls),
a new `crates/app/src/app/lag_hunt_tests.rs`. Does NOT touch `crates/core/src/doc.rs` (Lane B owns it;
`Document::regen_genlines` at doc.rs:3870 stays as is — work around it from the callers).
Report: `docs/plans/2026-09-10-lane-A-REPORT.md`.

### A0. Slow-frame attribution line (small, do first, ~30 min)
`diag.rs` already logs `[gpu] slow composite` (compositor CPU ms only) and `note_frame(dt)` knows the
whole-frame time. Add: when a whole frame exceeds 50 ms, ONE rate-limited log line (reuse the
`comp_slow_logged` pattern, same 1-per-few-seconds limit) that says WHERE the time went:
`[frame] slow: 212 ms | cmds: FrameDivide | ui 3 ms | composite 190 ms (12 tiles, full) | doc 6070x8598, 14 layers | tool Pen`.
To get the parts: time `ctx.run_ui` and `render_with_overlay` separately inside `App::render`
(app.rs:3062-3231, two `Instant`s), and have `main.rs::pump_commands` / `dispatch` push the names of
commands dispatched since the last frame into a small `Vec<&'static str>` on diag (use the enum
variant name via a tiny `fn name(&self) -> &'static str` on the command enum, or `format!("{:?}")`
truncated to the variant — pick whichever needs fewer lines; the command enum lives in
`crates/app/src/cmd/`). Also log the doc size + layer count once per doc open in the session banner
area (find where `testlog::begin_session` runs — main.rs ~623; add a `[doc] WxH dpi N layers` line
where the document is opened/created, one line per open). Purpose: from now on, every real session of
the owner's is a lag report we can read.

### A1. Measure (the HPC rule: numbers before fixes). `lag_hunt_tests.rs`, all `#[ignore]`, headless
Build the App the way `crates/app/src/app.rs:5033 headless_renderer()` + the existing test helpers
do (see how `actions.rs:945` or `frames.rs:212` builds an App). Document: **B4 600 dpi = 6070×8598**,
12 layers (raster ×8, one frame folder with 4 panels via `add_frame_folder` + `divide_frame_folder`,
one text layer, one tone layer). Time with `Instant`, print a table, write it to
`docs/plans/2026-09-10-lane-A-REPORT.md` under "Measurements":

| op | how |
|---|---|
| effect lines: `render_speed`, `render_focus`, `render_urchin` (genlines.rs:1030/951/1465) at each preset's DEFAULT params, full page | direct call, ms + tiles returned + peak alloc (tiles × 16 KB) |
| same at a typical panel-sized region if the params allow a bbox/centre+radius | direct call |
| `Document::composite_order` + a full compositor rebuild (`renderer` full=true path) | through `App::render` once with `canvas_dirty_all` forced |
| `finish_gpu_dab_stroke` readback: 2, 10, 40 tiles, with and without a full composite submitted immediately before | split the timer inside `dabs.rs::readback_dab_tiles` into submit / `poll(wait)` / copy-out and print all three |
| layer visibility toggle, opacity change, move_layer, undo/redo of a 200-dab stroke, `divide_frame_folder` | through the command queue if a helper exists, else direct `Document` calls |

Run with `CARGO_BUILD_JOBS=2 cargo test -p mn-app lag_hunt -- --ignored --test-threads=1 --nocapture`.
Headless uses the real Intel adapter here (no `MN_WARP`), which is what the owner has. Record the
numbers BEFORE any fix and again AFTER each fix in the report.

### A2. Effect lines: stop rendering the whole page per nudge (the crash)
**Cause (verify with A1's number first):** `Document::regen_genlines` (doc.rs:3870) calls
`spec.render(self.size)` — full-page rasterization on the UI thread — and it is invoked from
`canvas_input.rs:4035` (handle drag) and `cmd/text.rs:537` (property panel) on every change. On a
B4 600 dpi page each call is tens of millions of pixels; a slider drag fires it many times a second;
Windows declares the window not responding; the owner sees a "crash".
**Fix, in this order, stop when the drag is smooth in A1's timing:**
1. **Render only touched tiles.** Inside `genlines.rs` `render_*`: compute the effect's bounding box
   from its params (centre + outer radius for focus/urchin; the band for speed lines; clamp to page)
   and only allocate/rasterize tiles that intersect it. Tiles outside are simply absent from the
   returned map (regen already treats absent as cleared). If a render kind already does this, say so
   and move on.
2. **Coalesce during a drag.** At the two live-edit call sites, regen at most once per frame (a
   `pending_genlines: Option<(usize, spec)>` on App consumed at the top of `App::render`), and at
   full quality only on release; while the pointer is down, render at a `preview` quality if the
   params have a line-count / AA knob (a stride of 4 on line count is fine; it is a preview). If no
   such knob exists, skip the preview idea and rely on 1 + coalescing.
3. **Only if 1+2 still exceed ~100 ms per regen at B4 600 dpi:** move the render to a worker thread
   (`std::thread::spawn`, generation counter, latest result wins, applied on the UI thread through the
   existing `regen_genlines` door; a stale generation is dropped). Keep undo semantics: the op is
   recorded when the result is APPLIED, exactly as today.
**Tests:** a unit test per render kind asserting the returned tile set is bounded by the bbox (count
< full-page tile count for a small effect); an existing effect test suite exists (`effect_metrics`,
`genlines` tests) — run the targeted ones. **Acceptance:** A1 table shows a default speed-lines regen at
B4 600 dpi under 100 ms; dragging a handle (simulated: 30 regen calls in a loop) under 1 s total.

### A3. Pen: the end-of-stroke readback must not block behind the queue
**Cause (verify with A1's split timer):** `dabs.rs:861 readback_dab_tiles` does `map_async` then
`device.poll(PollType::wait_indefinitely())` on the UI thread. `poll(wait)` returns only when every
previously submitted command buffer is done — including the last composite of a big page — so the pen
freezes for 40-64 ms when the queue is busy. `desired_maximum_frame_latency: 1` + vsync makes the
queue busy often.
**Fix:** make the readback asynchronous, plain path only (the wash-commit and canary-repair branches in
`app.rs:4118 finish_gpu_dab_stroke` stay synchronous; they are rarer and need the bytes now):
- `readback_dab_tiles` splits into `begin_readback(layer, tiles) -> PendingReadback { buffers, tiles,
  layer, tile_revisions }` (submit + map_async, NO wait) and `try_finish_readback(&mut Pending) ->
  Option<(px, canary_ok)>` (calls `device.poll(PollType::Poll)`, returns `None` if not mapped yet).
- App keeps `pending_readback: Option<PendingReadback>`; `App::render` calls `try_finish` once per frame
  before the compositor runs and applies the tiles exactly as the current `else if canary_ok` branch
  does (copy into doc tiles, `mark_dab_tile_clean`).
- **Safety rules (this is his live chapter; silent corruption is the failure to avoid):**
  (a) if a NEW stroke begins on any layer while a readback is pending, finish it synchronously first
  (`poll(wait)`), same cost as today, never worse; (b) if the document's tile revision for any pending
  tile changed since `begin_readback` (undo, fill, another tool), DROP the readback and route through
  the existing canary-repair CPU path (rasterize from `all_dabs`) so the doc is right; (c) undo/redo,
  save, export, page switch, layer removal: flush pending synchronously first (one helper
  `flush_pending_readback()` called from those commands' entry — find them in `cmd/history.rs`,
  `cmd/file_io.rs`, `cmd/pages.rs`, `cmd/layers.rs`; keep the call one line each).
- Log line changes to `gpu | N tiles, readback queued` at stroke end and `gpu | N tiles, readback
  landed after K frames, M ms` when applied, so the owner's log shows the change.
**Tests:** the mn-gpu parity suite (kept deliberately; ~2 min) must pass; add a headless test that
begins a stroke, ends it, renders 3 frames, and asserts the doc tiles equal a synchronous readback
of the same stroke; and one that undoes before the readback lands and asserts the doc equals the
pre-stroke pixels. **Acceptance:** A1's readback row shows the stroke-end call under 2 ms with a busy
queue.

### A4. Not in scope (say so in the report if tempted)
No `[profile.release]` work (Fable did it: thin LTO, see git log). No `-march`. No SIMD rewrite of
libmypaint. No changes to `doc.rs`.

---

## LANE B — split `crates/core/src/doc.rs` (9,293 lines) into a `doc/` module directory (Opus)

Owns: `crates/core/src/doc.rs` → `crates/core/src/doc/*.rs` only. Nothing else. Pure move: zero
behaviour change, zero signature change, zero visibility widening where avoidable (child modules can
see the parent's private items, so `impl Document` blocks in child files keep working on private
fields without `pub(crate)` edits). Report: `docs/plans/2026-09-10-lane-B-REPORT.md`.

Layout (line ranges are today's doc.rs):

| new file | takes |
|---|---|
| `doc/mod.rs` | everything not listed below: the type definitions (Blend, LayerExpression, SpeechSet, LayerMask, CompositeStep, Layer, Paper, LayerComp, Document + their small `impl`s at 83-1967, `Default`s), `Document::new` and the accessor cluster 1969-2377 minus the mask methods, `mod` declarations |
| `doc/masks.rs` | `impl Document` mask methods 2002-2271 (`record_rulers`… `mask_clear`, `convert_brightness_to_opacity`), `mask_op_to_alpha` 5517 |
| `doc/history.rs` | 2378-3022 (undo/redo/op stack, `swap_group`) |
| `doc/layers.rs` | 3023-3689 minus the frame items: folder ranges, add/remove/duplicate/move layer, from_image |
| `doc/frames.rs` | 3216-3387 (`add_frame_folder*`, `divide_frame_folder*`), 3665-3943 (`add_frame_layer`, `combine_frame_folders`, `group_frame_folders_common_parent`, `regen_genlines`, `set_frames`) |
| `doc/derived.rs` | 3944-4198 (`set_tone`, `set_edge`, `refresh_derived*`, `refresh_folder_edge`) |
| `doc/vector_layers.rs` | 4199-4367 (balloon + text layer doors) |
| `doc/merge.rs` | 4368-4658 (`merge_down`… `set_tone_many`) |
| `doc/props.rs` | 4659-5262 (active/multi-select, opacity, blend, visibility, colour, expression, blend-if, clip, lock, reference, draft, escape) |
| `doc/spill.rs` | 5263-5516 (`enclosing_frame_folder`, spill seats, `composite_order`, `effective_drafts`) |
| `doc/resize.rs` | 5567-5777 (`ResizeAnchor`, `resize_to`, `resample_to`, `resize_canvas`, `trim_outside`, `paint_guard`, `over_pixel`) |
| `doc/paint.rs` | 5779-6220 (gradient paints, `fill_polygon`) and the `impl Layer` 6221-6352 |
| `doc/tests/mod.rs` + one file per module | the five `#[cfg(test)] mod` blocks 6353-9293 (`import_mask_tests`, `mask_tests`, `tests`, `combine_tests`, `group_tests`, `op_count_tests`) as `#[cfg(test)] mod x;` children; each file's `use super::*;` becomes `use super::super::*;` (or re-export `use crate::doc::*` in `tests/mod.rs`) |

Rules:
- Use `git mv crates/core/src/doc.rs crates/core/src/doc/mod.rs` first, then cut blocks out into
  the sibling files, so git history follows the file.
- Section banners (`// ---- undo --`) move with their code. Doc comments stay attached.
- `use` lines: each new file gets only the imports it needs; do not `pub use` anything new from
  `mod.rs`; external paths `mn_core::doc::X` must all still resolve (grep `doc::` across crates for the
  public names and confirm each still lives at the same path — the types stay in `mod.rs`, so they do).
- If a private helper is used from two new files, leave it in `mod.rs`.
- Gate: `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets` with zero warnings (an unused
  import in a moved file is a warning — fix, don't allow), then
  `CARGO_BUILD_JOBS=2 cargo test -p mn-core -- --test-threads=2` (core only; it is the crate you touched),
  then `cargo test -p mn-app frames -- --test-threads=2` as a spot check across the crate boundary.
- Acceptance: the build is warning-free, the core tests pass with the same count as before the move
  (record before/after counts in the report), `mod.rs` is under ~2,600 lines, no file over 1,500
  except `mod.rs`, and `git diff --stat` shows the move as renames + cuts, not a rewrite.

---

## Merge order (Fable)
Lane B first (mechanical, one crate), then Lane A (touches app + gpu + genlines; only its regen call
sites reference `Document::regen_genlines`, whose path does not change). The two lanes' files are
disjoint, so both may run at once; cargo's target lock serializes their builds.

## Already done by Fable in this round
- `[profile.release] lto = "thin"` in the workspace `Cargo.toml` (HPC step 1, the safe half; no
  `codegen-units = 1` because it roughly doubles the owner's local release build time, no
  `target-cpu` because the exe ships to strangers' machines).
