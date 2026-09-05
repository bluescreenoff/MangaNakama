# Lane 5 report — save indicator (K), balloon pen (L, M, N, O)

## done / next  ← READ THIS FIRST ON RESUME

**DONE: K (partly — see the caveat, it is important).  NOT STARTED: O, L, M, N.**

| item | state |
|---|---|
| K — threaded save + "Saving…" pill + timing table | **built, compiles, 5 tests green**. But the measurement says the pill will barely be seen; see "K, the uncomfortable finding" below. One more change is wanted (option 1 layered on top of option 2). |
| O — thickness follows pressure | not started |
| L — live preview shows real width + colour | not started |
| M — sharp corners | not started |
| N — square vs circle control points | research DONE (below), drawing not started |

**Next edit, exactly:** item O — `crates/core/src/balloon.rs`, `BalloonSet::new`
(line ~1327, `pressure_width: false` → `true`) and the width curve in
`BalloonSet::rasterize`'s `border_of` closure (line ~1363, `0.35 + 0.65 * pr`),
plus a new `min_width` field on `BalloonSet` exposed as "Min size %" in
`crates/app/src/ui/property/frames_balloons.rs::sec_obj_balloon`.

Gate at the moment of stopping: `cargo check -p mn-app --all-targets` = **exit 0,
zero warnings**. Tests passing: `cargo test -p mn-app save_bg` (4) and
`cargo test -p mn-app every_blocking_file_command_is_timed` (1).

---

## K — was the save synchronous? Yes. What is it now?

**Before:** every arm in `crates/app/src/cmd/file_io.rs` did encode + disk write
inline on the UI thread. `AppCmd::SaveOraPath` → `mn_core::ora::save` /
`project::save` / `App::save_work_folder`, all blocking. So the Windows message
pump stopped for the whole save, which is exactly what makes Windows paint
"not responding" over a window, and it is also why any "saving…" widget drawn
in the same frame was never seen: no frame was ever produced.

**Now:** every save is two halves.

- The **encode** (document → bytes) stays on the UI thread. It reads the live
  layers, so it has to.
- The **write** (bytes → disk) goes to one background thread, process-wide, in
  the new file `crates/app/src/cmd/save_bg.rs`: one queue, one worker, so a
  second save while one is in flight queues behind it and two writes can never
  race each other's `rename`. Atomic `tmp` + `flush` + `fsync` + `rename`, same
  contract `mn_core::ora::write_atomic` keeps.
- The bookkeeping (`mark_saved`, `set_doc_path`, per-page `saved_rev`) runs
  optimistically the moment the bytes are handed over; if the write fails,
  `save_bg::poll_saves` puts the work **back to dirty** and shows the error.
- Drained once per UI frame from `ui::build`, and also at the end of every
  command (`cmd::run_cmd_tail`) so headless drivers still read the real
  "saved …" line.
- **No frames, no background.** `save_bg::submit` waits for the write when no
  UI frame loop is running (unit tests, `--e2e-workfolder`, the `--shot-*`
  drivers) — all of those dispatch a save and then immediately read the file
  back, and "the file is not there yet" is a bug, not a mode. In `cfg(test)`
  it is always synchronous, so parallel tests cannot make each other flaky.

### K, the uncomfortable finding — the write was never the problem

The timing test (`cargo test -p mn-app every_blocking_file_command_is_timed
-- --nocapture`, lives at the bottom of `cmd/file_io.rs`) measured the split on
the same work in the same run:

```
one page, 2024 x 2866 px, 4 layers:
  encode to .ora bytes ....... 14 424 ms   (stays on the UI thread)
  write those bytes to disk ...      1 ms   (now off it)
```

So threading the write removes **~0.007 %** of the freeze. The freeze is the
ENCODE — `mn_core::ora::save_to`'s per-layer `export::layer_image` +
`encode_png`. Two consequences the owner needs to know:

1. The work I did is correct and worth keeping (it is also what makes the pill
   possible at all), but on its own it will **not** stop "not responding".
2. **The pill as built will barely be visible**, because during the 14 s encode
   no frame is drawn. The plan's *option 1* has to be layered on top of option
   2: arm `saving`, let one frame draw the pill, and run the encode on the
   frame AFTER that. That is the first thing to add when K is resumed.
   The real cure is making the encode cheaper or incremental, which is a core
   (`crates/core/src/ora.rs`) change and belongs to its own item.

### The timing table (item K's addendum)

Three-page work, B4 at 200 dpi = 2024 × 2866 px per page, 4 layers, ink on every
page. Measured on the owner's laptop **while another agent lane was compiling**,
so treat these as an upper band with high variance (an earlier run of the same
table gave 5.4 s where this one gives 14.4 s). The ratios are stable; the
absolute numbers are not. His real pages are 600 dpi B4 = **9× these pixels**.

| command | ms (blocking, before) | over 1 s? | now |
|---|---|---|---|
| `SaveOraPath` bare `.ora` | 14 373 | yes | encode blocks, write threaded |
| `SaveOraPath` `.mnc` single file | 15 451 | yes | encode blocks, write threaded |
| `SaveOraPath` `work.mnc`, first save | 11 133 | yes | encode blocks, write threaded |
| `SaveOraPath` `work.mnc`, re-save, nothing dirty | 14 784 | yes | ditto — **and it should be near zero; see the leftover below** |
| `ExportMncPath` | 14 225 | yes | encode blocks, write threaded |
| `ExportPsdPath` | 11 506 | yes | encode blocks, write threaded |
| `ExportPngPath` | 754 | no | render+deflate block, write threaded |
| `SaveDuplicatePath` | 12 935 | yes | **still fully synchronous** — it lives in `app/save_duplicate.rs`, which this lane does not own |
| `ExportTextPath` | 7 074 | yes | **still synchronous.** It is a text dump that should cost milliseconds; the cost is upstream of the write (`commit_text_edit` / `script_dump`) and is worth its own look |
| `Autosave` | 0 (skipped: work was clean) | — | encode blocks, write threaded, and a tick is skipped while another write is in flight |
| `OpenOraPath` `.mnc`, 3 pages | 3 303 | yes | **read path, untouched by this lane** |

Everything over 1 s that this lane did not fix, in one list:
`SaveDuplicatePath`, `ExportTextPath`, `OpenOraPath`, and — the big one — the
encode half of every row above.

### Two leftovers spotted while measuring (not fixed, not mine)

- **A clean re-save of a work folder still costs a full save** (14 784 ms with
  "0 rewritten"). `save_folder` correctly skips writing the page files, but the
  caller has already re-encoded every page to build the `WorkFolder`. The skip
  needs to happen one level up, before the encode.
- `ExportTextPath` at 7 s for a script dump.

### Files item K touched

Owned by this lane:
- `crates/app/src/cmd/file_io.rs` — every save arm split; the timing test.

**Outside the lane's list** (all uncontested by the other lanes' file lists —
flagging them as required):
- `crates/app/src/cmd/save_bg.rs` — **new file**, the queue/thread/pill/poll.
- `crates/app/src/cmd.rs` — one `pub(crate) mod save_bg;` + one `poll_saves`
  call in `run_cmd_tail`.
- `crates/app/src/ui.rs` — two calls in `build`: `poll_saves` and `saving_pill`.
  (The status line lives in `ui/top.rs`, which this lane was told not to touch,
  so the pill is drawn on the canvas overlay layer instead, as instructed.)
- `crates/app/src/app/page_files.rs` — `save_work_folder` /
  `autosave_work_folder` gained a `_via` twin so the disk write can be
  injected. No behaviour change on the synchronous path.

`crates/app/src/app.rs` was deliberately NOT touched (Lane 4 owns regions of it,
and Lane 2 is editing it): the save state lives in `save_bg`'s own module state
rather than as an `app.saving` field.

### Tests passing (item K)

- `cargo test -p mn-app save_bg` — 4 tests:
  - `a_write_in_flight_draws_the_saving_pill` (state set + one headless egui
    frame paints "Saving… work.mnc")
  - `the_background_writer_lands_the_bytes_and_reports_back`
  - `a_failed_write_comes_back_as_an_error`
  - `the_id_guess_matches_what_save_folder_assigns` (the pin that the page-id
    guess made on the UI thread equals what `save_folder` assigns)
- `cargo test -p mn-app every_blocking_file_command_is_timed` — the table above.

---

## N research (done up front, 10 min, EN + JP)

CSP's convention, confirmed in both languages:

- **Circle = smooth anchor (角なし), square = corner anchor (角あり).** JP source,
  exact wording 「角なし」の制御点は「〇」（丸）で表示され、「角あり」の制御点は「口」（四角）で表示されます:
  https://dentakumanga.com/clipstudiopaint-vectorlayer-controlpoint/
- **Toggle:** the Control point sub tool's 「角の切り替え」 ("Switch corner") —
  tapping a control point flips curve ↔ corner. EN tool guide:
  https://www.clip-studio.com/site/gd_en/csp/toolguide/csp_toolguide/100_reference/Controlpoint.htm
- **Modifier:** with the Figure/Bezier tools, **Alt+click an anchor** turns a
  curve into a corner (Alt+drag turns a corner back). With the Correct line
  tool it is Shift+Alt+click.
- Verdict: MangaNakama **already** binds Alt+click on a balloon anchor to
  `Balloon::toggle_anchor_corner`
  (`app/canvas_input.rs::balloon_anchor_edit`, reached from the Object tool),
  which matches CSP's Figure/Bezier spelling and is already undoable through
  `AppCmd::BalloonCommit`. So item N is only the SHAPE of the drawn handle
  plus a line in `docs/manual/text.html`.

---

## Causes already found for O, M, L (survey done, no code written)

- **O (uni-thickness).** `BalloonSet.pressure_width` is `false` at **every**
  construction site in the repo, including `BalloonSet::new` (`core/balloon.rs`
  ~1327), which is the one the balloon pen's fresh layer goes through
  (`cmd/text.rs::BalloonAdd`). So the default is off — cause (1) of the three
  the plan lists. Cause (2) is also real but smaller: the modulation is
  `border_px * (0.35 + 0.65 * pr)` (`rasterize`'s `border_of`, ~1363), i.e. a
  3× range with no min-size setting. Cause (3) is NOT the problem —
  `simplify_anchors` does carry each kept anchor's pen pressure.
- **M (corners not sharp).** `app/canvas_input.rs::finish_balloon_drag`,
  `BalloonMode::Draw` arm, builds `BalloonShape::Polygon { ..., corners:
  Vec::new() }` — the drawn balloon **never gets a single corner anchor**.
  `tessellate_closed` already honours a corner correctly (it zeroes both
  Catmull-Rom tangents there, which is a true kink), so M is purely the missing
  detection at release.
- **L (live preview).** `ui/overlay/frames.rs::balloon`, `BalloonMode::Draw`
  arm (~line 532): one `Shape::line` at 1.5 px in the accent colour, ignoring
  width, pressure and the tool's ink.
- **N (handles).** `ui/overlay/frames.rs::object` (~line 452): balloon shape
  handles are drawn as squares unconditionally, tail handles as circles.
