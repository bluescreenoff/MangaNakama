# Lane A report — perf: measure, then fix the two known offenders

## DONE / NEXT

- [x] **A0 slow-frame attribution line** — done, compiles clean.
- [x] **A1 measurements (BEFORE)** — done, release, real Intel UHD 620. Table below.
- [x] **A2 effect lines** — done (step 1 rewritten, step 2 was already in the app).
- [x] **A3 — STOPPED ON PURPOSE.** Cannot be made safe: the selection clamp, the alpha lock and the undo-op close all run AFTER the readback, inside the same op. Full reasoning below. Synchronous path left in place; the submit/wait/copy split shipped as diagnostics.
- [x] Gates: workspace check clean (zero warnings), mn-core genlines 41/41, mn-gpu parity all green.

### How to re-run
```
export PATH="$PWD/toolchain/w64devkit/bin:$HOME/.cargo/bin:$PATH"   # build.sh's C toolchain
CARGO_BUILD_JOBS=2 cargo test --release -p mn-app lag_hunt -- --ignored --test-threads=1 --nocapture
```
Release, not debug: the owner runs release, and debug is 10-30x off. Without the
toolchain on PATH, `mn-brush`'s build script cannot find a C compiler and nothing builds.

### Files touched so far
- `crates/app/src/app/diag.rs` — `note_cmd` / `note_doc` / `note_frame_parts`, `VariantName`.
- `crates/app/src/app.rs` — `App::render` timing split + `[doc]` line; `mod lag_hunt_tests`.
- `crates/app/src/cmd.rs` — `dispatch` head records the command name.
- `crates/gpu/src/dabs.rs`, `crates/gpu/src/lib.rs` — `ReadbackTiming` + `readback_timing()`.
- `crates/app/src/app/lag_hunt_tests.rs` — NEW, 5 `#[ignore]` measurements at B4 600 dpi.

---

## What A1 actually found

**The effect-line "crash" is not tile allocation. It is an O(length squared) scan.**
`genlines::segment` (and `fill_tooth`) scan the AXIS-ALIGNED bounding box of each stroke
and test every pixel in it. For a diagonal stroke that box is the whole page: a speed
line 8,000 px long and 3 px wide costs ~30 million pixel tests instead of ~30 thousand.
Multiply by a preset's few hundred lines and one render is tens of seconds.

The plan's step 1 ("render only touched tiles") is already true — `put()` only allocates
a tile when ink lands in it, and both scans already CLIP their box to the page. Clipping
to the page does not help when the stroke crosses the page.

**The pen readback is exactly what the plan said**: the milliseconds are the
`poll(wait)`, not the copy — 219.7 ms of a 306 ms readback, with the copy at 13 ms.
And that is HEADLESS, with no swapchain and therefore no vsync back-pressure at all.

---

## Measurements — BEFORE (release, Intel UHD 620 DX12, B4 600 dpi = 6070x8598, 12,825 tiles)

### Effect lines, each preset at its DEFAULT params
| preset | full page ms | tiles | MB | panel-sized ms | tiles |
|---|---|---|---|---|---|
| Stream line | **20,052** | 11,640 | 364 | 9,637 | 10,271 |
| Dense stream | **23,960** | 12,766 | 399 | 12,462 | 11,864 |
| Sparse stream | **8,644** | 8,018 | 251 | 4,329 | 5,903 |
| Perspective stream | **> 9 minutes** (run killed) | - | - | - | - |
| remaining 8 presets | not measured — the run had to be killed | | | |

One render. Not thirty. The owner's "crash" is Windows marking the window
not-responding while this runs on the UI thread, exactly as the plan predicted — but for
a different reason than the plan assumed.

The 30-regen drag row was not run BEFORE the fix: at 20 s per regen it is 10 minutes per
preset, 2 hours for the table. The single-render number already settles it.

### Compositor (same document, 12 layers, through `render_offscreen_vp`)
| op | wall ms | detail |
|---|---|---|
| `Document::composite_order` | 0.01 | 22 steps |
| first composite (cold) | 11,866 | 12,825 tiles, 68,558 uploads, full, gpu-side 11,309 ms |
| second composite (warm) | 83 | 0 tiles, 0 uploads |
| forced full rebuild (`invalidate`) | 4,053 | 12,825 tiles, 68,558 uploads, gpu-side 3,682 ms |

Matches the owner's log line `957.7 ms cpu-side (12825 tiles, 30 uploads, full rebuild)`.
A full rebuild of a B4 page is seconds. Out of Lane A's scope this round; recorded so it
is not mistaken for the effect-line problem.

### Ordinary document actions
| op | ms |
|---|---|
| layer visibility toggle | 0.00 |
| layer opacity change | 0.00 |
| `move_layer` | 1.81 |
| undo of a 200-dab stroke | 0.01 |
| redo | 0.00 |
| **`divide_frame_folder`** | **273** |
| `composite_order` | 0.00 |

Only `divide_frame_folder` is slow, and it is one click, not a drag. Noted, not fixed
(it derives a page-sized frame raster). The rest are free.

### Pen stroke-end readback, split three ways
| stroke | tiles | total ms | submit | poll(wait) | copy-out |
|---|---|---|---|---|---|
| short | 5 | 37.9 | 22.26 | 3.47 | 0.52 |
| medium | 64 | 57.3 | 2.55 | **20.57** | 7.48 |
| long | 317 | 306.4 | 8.69 | **219.74** | 13.33 |

The wait dominates and the copy does not. Headless, with an idle queue. In the live app
vsync keeps the queue busy, which is the owner's bimodal 4-9 ms / 40-64 ms.

---

## A2 — effect lines: what was done

**Step 1 (render only touched tiles): replaced with the real fix.** Tiles were already
only allocated where ink lands (`genlines::put`), and both scans already clipped to the
page. The actual defect was the SCAN: `segment` and `fill_tooth` test every pixel in the
stroke's axis-aligned box, which for a diagonal stroke is the whole page.

`crates/core/src/genlines.rs` now walks each stroke in chunks along its own axis
(`scan_chunks`), so the work is proportional to length x width instead of length squared.
It is not an approximation: every pixel within reach of the stroke still lands inside some
chunk's box, the boxes only ever shrink relative to the old one, and `put` is idempotent
so overlaps cost a repeated test. **The written pixel set is identical** — the pinned
`legacy_renders_are_bit_stable` test passes unchanged, along with all 39 genlines tests.

**Step 2 (coalesce during a drag): ALREADY DONE in the app — nothing to add.** The plan
assumed a slider drag fires `regen_genlines` many times a second. It does not:
- `crates/app/src/ui/property/effect_lines.rs` `gen_commit` holds the edit in
  `app.gen_edit` while the pointer is down and only pushes `GenLinesApplyTo` on the
  release edge.
- `crates/app/src/app/canvas_input.rs` `gen_drag` is updated on move (line 3495, the
  overlay draws it) and regenerated exactly once, in the pointer-UP handler (line 4020).

So the freeze was never thirty regens. It was ONE regen taking twenty seconds. A
`pending_genlines` field would have added state for a problem that does not exist.

**Step 3 (worker thread): not needed** if the after-numbers land under ~100 ms — see the
AFTER table below.

### Tests added (`crates/core/src/genlines.rs`, `mod scan_cost_tests`)
- `every_preset_renders_a_full_page_in_well_under_a_second_of_work` — every shipped
  preset on a 3000x4000 page under a 5 s wall budget. Safe as a wall-clock assertion only
  because the two sides are three orders of magnitude apart.
- `a_small_effect_touches_a_small_share_of_the_page` — one case per render kind.

### NOT fixed, and why: `LineKind::Solid` (ベタフラ / Solid flash)
It draws no strokes. It scans a filled disc and punches the teeth out of it, with an
`atan2` and a window of tooth tests per pixel, so its cost is the AREA of black it prints
— honest work, and never quadratic. Measured 17.8 s in a DEBUG build on 3000x4000, so
roughly 0.6-1.8 s in release; excluded from the budget test with that reasoning written
next to the `continue`. It is the one preset still slow enough to feel on a B4 page.
**Owner question at the end of this report.**

---

## A3 — STOPPED. The synchronous readback stays. Here is why.

The plan's diagnosis is correct and the measurement backs it (219 ms of a 306 ms readback
is the `poll(wait)`, 13 ms is the copy). The fix as specified cannot be made safe, and the
brief says to stop and say so rather than ship a maybe.

**`finish_gpu_dab_stroke` is not the end of the stroke.** It is called from
`App::end_stroke` at `app.rs:3832`, and THREE things run after it, on the pixels it just
wrote, before the undo op closes:

1. `self.doc.mask_stroke_to_selection()` (app.rs:3833) → `Document::mask_op_to_selection`
   (`crates/core/src/selection.rs:771`). This clamps the stroke to the active selection by
   walking `layer.recorded_tiles()` — the tiles recorded in the **currently open** undo op
   — and blending each against its pre-image.
2. `self.doc.mask_op_to_alpha()` (app.rs:3841), the transparent-pixel lock, the same shape.
3. The op itself closes at the bottom of `end_stroke` (`end_op_*` / `abort_op_restore`).

Under the GPU dab path (BYPASS) the CPU tiles are untouched for the whole stroke. They
become recorded — and their pre-images captured — **only** when `finish_gpu_dab_stroke`
writes the readback into them. So if the write is deferred to a later frame:

- `recorded_tiles()` is empty when the selection clamp runs, the clamp is a no-op, and the
  deferred write then lands **unclamped**: ink outside the selection, permanently, with no
  error and no visual cue at the moment it happens.
- The same for the transparent-pixel lock.
- The op has already closed, so the write has **no pre-image**. The stroke is not
  undoable, and the next undo restores a state that never contained it.

**The plan's safety rule (b) cannot catch this.** Rule (b) drops the readback if a pending
tile's revision changed. Here nothing changes a revision — the clamp had nothing to clamp,
precisely because the pixels are not there yet. The corruption is invisible to the rule
that was supposed to guard it.

Making it safe means deferring the selection clamp, the alpha lock AND the undo-op close
across frames, i.e. holding an undo op open between frames while autosave, the command
queue and every other tool run. That is a much larger change to the undo bracket than
"make the readback async", it is on the owner's live chapter, and it is not what this lane
was scoped to do. **Stopped deliberately. `readback_dab_tiles` is unchanged.**

### What A3 DID ship (diagnostics only, no behaviour change)
- `mn_gpu::ReadbackTiming` + `Renderer::readback_timing()` — the submit / wait / copy split.
- The `[dab]` log line now reads
  `gpu | 23 tiles, readback 49.1 ms (submit 2.1, wait 44.3, copy 2.7)`, so the owner's next
  session log says whether the pen is queued behind the compositor or not.

### If this is picked up later, the shape that IS safe
Keep the write synchronous, but stop the queue being busy at stroke end — e.g. do not
submit the composite on the frame a stroke ends, or give the dab path its own queue
submission ordering. That attacks the `wait` without moving the pixels out of the op.

---

## Gates

| gate | result |
|---|---|
| `cargo check --workspace --all-targets` | **clean, zero warnings** |
| `cargo test -p mn-core genlines -- --test-threads=2` | **41 passed**, incl. `legacy_renders_are_bit_stable` (39 before + the 2 new) |
| `cargo test -p mn-gpu -- --test-threads=2` (parity suite) | **all passed** — 13 + 30 + 3 + 13 + 4 + 20, 0 failed |
| AFTER measurements (release) | see below |

### Disk note for whoever picks this up
F: filled up mid-round (the release build is ~2 GB on top of debug's ~6 GB, with two
lanes building). To finish I deleted, in this order, only regenerable build caches:
`target/debug/incremental` (1.7 GB — `build.sh` sets `CARGO_INCREMENTAL=0` and its own
comment says these have corrupted twice and no documented workflow uses them), the six
`mn-gpu` integration-test exes in `target/debug/deps` (1.5 GB, after the parity suite had
already passed), and `target/{debug,release}/examples`. No source, no target dir wholesale,
nothing of Lane B's. F: is still only ~1.7 GB free — **worth telling the owner.**

---

## Measurements — AFTER (same machine, same release settings, same page)

### Effect lines, default params, full B4 600 dpi page
| preset | BEFORE ms | AFTER ms | speedup | tiles | MB of tiles |
|---|---|---|---|---|---|
| Stream line | 20,052 | **5,125** | 3.9x | 11,640 | 364 |
| Dense stream | 23,960 | **6,515** | 3.7x | 12,766 | 399 |
| Sparse stream | 8,644 | **3,486** | 2.5x | 8,018 | 251 |
| Perspective stream | **> 540,000** (killed) | **8,887** | **> 60x** | 11,643 | 364 |
| Drip lines | not reached | 614 | | 6,271 | 196 |
| Saturated line | not reached | 1,542 | | 6,178 | 193 |
| Dense saturated line | not reached | 2,113 | | 8,444 | 264 |
| Dark burst | not reached | 3,983 | | 9,520 | 298 |
| Sea urchin flash | not reached | **248** (834 before `fill_tooth` was fixed too) | 3.4x | 771 | 24 |
| Solid flash | not reached | 15,527 | **unchanged** | 12,389 | 387 |

The preset that hung for over nine minutes now takes nine seconds. Nothing hangs the
window indefinitely any more, and `legacy_renders_are_bit_stable` says every existing
effect-line layer still regenerates to the same pixels.

### The plan's acceptance number was NOT met, and here is the arithmetic
The plan asked for "a default speed-lines regen at B4 600 dpi under 100 ms". That is not
reachable by bounding the scan, because **these presets ink almost the whole page.**
`Dense stream` fills 12,766 of the page's 12,825 tiles — 399 MB of tile data.

Read the cheapest row as the floor: `Drip lines` writes 6,271 tiles in 614 ms, i.e. about
0.1 ms per tile including its scan. At that rate the 12,766-tile presets cost ~1.3 s in
ink alone. 100 ms would need roughly 10x more, and the 10x is not in the scan geometry —
it is in how the ink is stored:

- `genlines::put` does a **HashMap lookup per inked pixel** (`TileIdx::of_pixel` + `entry`).
  Tens of millions of hashes per render.
- The map holds `Tile` **by value at 32 KB each**, so every growth rehash memcpys hundreds
  of megabytes, and the final `into_iter().map(Arc::new)` copies all of it again.
- It is single-threaded, and the strokes are independent.

Getting to 100 ms means writing spans into per-tile buffers instead of pixels into a
HashMap, and rasterizing strokes in parallel. That is a rasterizer rewrite, it is a
different risk class from a scan bound, and the brief's A4 rules it out for this round.
**Recorded, not attempted.**

### `Solid flash` (ベタフラ) is unchanged at 15.5 s — different bug, still open
It draws no strokes, so `scan_chunks` never touches it. It scans a filled disc — here
12,389 tiles, effectively the whole page — and for EVERY pixel does an `atan2`, a
core-radius lookup and a window of `Tooth::hit` tests before deciding black or white
(`render_urchin`'s `solid` branch, `genlines.rs` ~1890-1950). The cost is
`page_pixels x teeth_in_window`. Bounding it is a real fix but a different one:
precompute per-angle-bin spans instead of testing per pixel. **Not attempted this round.**

---

## Everything that changed, by file

| file | what |
|---|---|
| `crates/core/src/genlines.rs` | `scan_chunks` + chunked scanning in `segment` and `fill_tooth`; `MAX_SCAN_CHUNKS`; new `mod scan_cost_tests` (2 tests) |
| `crates/app/src/app/diag.rs` | `Diag::note_cmd` / `note_doc` / `note_frame_parts`, `VariantName`, 3 fields, 3 consts |
| `crates/app/src/app.rs` | `App::render`: `[doc]` line, ui/composite `Instant`s, `note_frame_parts`; `finish_gpu_dab_stroke`: readback log line now carries the submit/wait/copy split; `#[cfg(test)] mod lag_hunt_tests` |
| `crates/app/src/cmd.rs` | `dispatch` records the command name for the next slow-frame line |
| `crates/gpu/src/dabs.rs` | `ReadbackTiming` + timing in `readback_dab_tiles` + `readback_timing()` |
| `crates/gpu/src/lib.rs` | `readback_timing` field + `pub use dabs::ReadbackTiming` |
| `crates/app/src/app/lag_hunt_tests.rs` | NEW — 5 `#[ignore]` measurements |
| `docs/plans/2026-09-10-lane-A-REPORT.md` | NEW — this file |

Nothing staged, nothing committed, `crates/core/src/doc.rs` untouched.

## Questions for the owner

1. **Effect lines still take 3-9 seconds on a B4 600 dpi page** (down from 20 s to
   9+ minutes). The window no longer looks dead, but it is not instant. Getting to
   "instant" means rewriting how effect lines store their ink (spans into tiles instead of
   a hash lookup per pixel, and threads). Is that worth its own round?
2. **`Solid flash` (ベタフラ) still takes 15 seconds.** Separate cause, described above.
   Does he use it? If yes it is the next thing to fix; if no it can wait.
3. **The pen readback stays synchronous.** Making it async would have risked ink landing
   outside the selection with no way to notice. Fine to leave slow?
4. **F: has about 1.7 GB free.** Two release builds and the round is stuck.

## One near-miss worth knowing about (fixed)

A `perl -0pi` one-liner I used to insert a field into `crates/gpu/src/lib.rs` contained a
`\x{2014}` escape. That switches perl's output to character mode, so it re-encoded the
file's EXISTING UTF-8 as if it were latin-1 and turned every em dash and Japanese
character in the comments into mojibake — a 6-line change showing up as a 244-line diff.
Caught by reading `git diff --stat`, fixed by `git checkout` on that file and re-applying
the three edits properly. `crates/gpu/src/lib.rs` is now +6/-2 and mojibake-free, and the
`mn-gpu` parity suite was re-run green against the final tree. No other file was touched
by a perl one-liner carrying a non-ASCII replacement, and all seven files were checked.

Lesson for the next agent on this repo: do not use `perl -0pi` with non-ASCII in the
replacement on these sources. They are full of Japanese and em dashes.
