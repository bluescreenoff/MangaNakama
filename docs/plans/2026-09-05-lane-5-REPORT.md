# Lane 5 report — save indicator (K), balloon pen (L, M, N, O)

## done / next  ← READ THIS FIRST ON RESUME

**ALL OF LANE 5 IS DONE: K (rounds 1 and 2), O, L, M, N, and both manual
pages.** Nothing is left for this lane. Not committed — the diff is in the
working tree for review.

| item | state |
|---|---|
| K r1 — background WRITE + "Saving…" pill | committed `abe1231` |
| K r2 — the encode itself off-thread, page skip, layer PNG cache, cheap thumbnail | **done.** Saves went from ~5 500 ms of frozen window to **1–170 ms**. |
| O — thickness follows pressure | **done.** Pressure is on for new balloon layers; the taper spans the whole width through a per-balloon `min_width` (CSP 最小値) with a "Min size %" bar in Tool Property. |
| L — live preview shows the real width and colour | **done.** The trail is a mesh ribbon at the outline's true width, taper and ink, with a 1 px accent centre-line; preview and commit share `BalloonSet::border_at` and a test pins them equal. |
| M — sharp corners | **done.** Corners are detected on the RAW trail (turn > `CORNER_TURN_DEG` = 55° over a 6 px chord each side), the trail is cut at them so simplification cannot drop them, and `tessellate_closed` already kinks there. |
| N — square vs circle control points | **done.** Corner anchors draw as squares, smooth ones as circles, per CSP (sources below). The Alt+click toggle already existed; it now has a test and a manual entry. |
| manual | `docs/manual/text.html` (balloon pen: pressure, Min size, live trail, spikes, the two grips) and `docs/manual/files.html` (saving in the background, the pill, the queue, a failed write, the free re-save). |

**If anything comes back to this lane**, the two obvious follow-ups are both
named below under "what is still over 1 s": `SaveDuplicatePath` (4.3 s, one
line now that `PageEncode` exists, but it lives in a file this lane does not
own) and `ExportTextPath` (2.8 s for a text dump, cost is upstream of the
write).

Gate: `cargo check --workspace --all-targets` = exit 0, **zero warnings**.
`cargo test -p mn-core` = **817 passed, 0 failed**. Every targeted app suite
below is green.

---

# K — the freeze

## Round 1 (committed): was the save synchronous? Yes.

Every arm in `cmd/file_io.rs` encoded *and* wrote on the UI thread, so the
Windows message pump stopped for the whole save — which is exactly what makes
Windows paint "not responding" over a window, and also why a "saving…" widget
drawn in the same frame was never seen: no frame was ever produced.

Round 1 moved the **write** to one background thread (`crates/app/src/cmd/save_bg.rs`:
one queue so writes never overlap, atomic tmp + fsync + rename, a failed write
puts the work back to dirty) and put a "Saving…" pill on the canvas overlay.

## Round 2: where the seconds actually were

Round 1's own timing test said the write was 1 ms and the encode was seconds, so
round 2 measured the encode stage by stage
(`cargo test -p mn-app the_ora_encode_cost_splits_by_stage -- --nocapture`).
One page, 2024 × 2866 px, 4 layers:

| stage | ms | share |
|---|---|---|
| `export::layer_image` — tiles → RGBA, per-pixel unpremultiply | 993 | 17 % |
| the four layer PNGs (`encode_png`) | 346 | 6 % |
| `export::composite` for `mergedimage.png` | 2 777 | 47 % |
| that merged image as PNG | 226 | 4 % |
| **`Thumbnails/thumbnail.png` — `imageops::resize(.., Triangle)`** | **1 556** | **26 %** |
| **whole `.ora` encode** | **5 925** | |
| the disk write of the result | 1 | 0.02 % |

And the measurement that decided the fix:

```
Document::clone (the snapshot the writer thread gets):   0 ms
the same encode, run on a background thread:          4 829 ms
```

`Document` is `Send`, and cloning one is **free** — every tile is an `Arc<Tile>`,
so a clone copies pointers, not pixels. That is the whole fix: snapshot on the
window thread, encode and write on the writer thread.

## What round 2 changed

1. **(d) The encode moved to the writer thread.** `save_bg::Write` grew `Ora`,
   `Psd` and `Project` variants and a `PageEncode` (a `Document` snapshot plus
   the GPU-rendered page preview). `App::snapshot_active_page` is
   `stash_current_page` minus `doc_to_bytes`: it lands the stroke and the text
   edit, renders the sharp preview (GPU — cannot leave the window thread),
   refreshes the palette thumbnail, advances the page revision, and hands back a
   snapshot.
2. **(c) Unchanged PAGES are skipped before they are copied.** A page already on
   disk at its current revision now contributes empty bytes and no job.
   `save_folder` already skipped *writing* it — but it had still been handed a
   full copy of its bytes, and the live page had still been re-encoded. The skip
   test is the exact negation of `save_folder`'s write test; a comment in
   `page_files.rs` says so, because getting them out of step writes a page empty.
3. **(c) Unchanged LAYERS are not re-encoded.** A PNG cache in `ora.rs` keyed by
   `(layer id, max tile revision, tile count, image size)`. Tile revisions come
   from one monotonic counter stamped on every write, so any paint moves the key;
   removing tiles cannot leave both the maximum and the count unchanged.
4. **The thumbnail.** `imageops::resize(.., Triangle)` → `imageops::thumbnail`.
   1 556 ms → ~30 ms, indistinguishable at 256 px, and it ran on every save.
5. **(b) PNG compression: measured, then left alone.** `image` 0.25's PNG default
   is *already* `CompressionType::Fast`. The alternatives are worse, not better:
   dropping the adaptive per-row filter made the encode **slower** (1 445 ms
   against 195 ms for a full page) and the file **73× bigger** — an unfiltered
   page of white is what deflate is worst at. Uncompressed is 41 ms but 23 MB a
   layer, a bad trade on a drive with 7 GB free. Nothing to change here, and now
   there is a comment in `encode_png` saying why so nobody re-litigates it.

## The timing table — before and after

Three-page work, B4 at 200 dpi (2024 × 2866 px a page), 4 layers, ink on every
page. **These are DEBUG builds** (`[profile.dev]` puts `opt-level = 0` on our own
crates); the owner's `play/` install is `--release`, where our own pixel loops
run several times faster while the PNG/deflate part does not move. Read the
column that matters — "blocks the pump" — as an upper bound.

`cargo test -p mn-app every_blocking_file_command_is_timed -- --nocapture`

| command | before (froze the window) | now blocks the pump | now in the background |
|---|---|---|---|
| `SaveOraPath` bare `.ora` | 5 413 | **1 ms** | 4 042 |
| `SaveOraPath` `.mnc` single file | 5 813 | **170 ms** | 4 144 |
| `SaveOraPath` `work.mnc`, first save | 5 490 | **65 ms** | 4 157 |
| `SaveOraPath` `work.mnc`, clean re-save | 5 700 | **80 ms** | 4 112 |
| `ExportMncPath` | 5 356 | **67 ms** | 4 144 |
| `ExportPsdPath` | 4 078 | **0 ms** | 4 288 |
| `ExportPngPath` | 293 | 301 ms | 3 |
| `SaveDuplicatePath` | 5 433 | 4 266 ms — **still blocking** | 0 |
| `ExportTextPath` | 2 825 | 2 801 ms — **still blocking** | 0 |
| `OpenOraPath` (.mnc, 3 pages) | 1 366 | 1 357 ms — **still blocking** | 0 |
| `Autosave` | — | 0 ms (skipped while a write is in flight) | — |

The 65–170 ms that remains on the window thread is the GPU page-preview render
plus the palette thumbnail, both of which need the renderer.

## What is still over 1 s and was NOT fixed

- **`SaveDuplicatePath`, 4.3 s.** It lives in `crates/app/src/app/save_duplicate.rs`,
  which this lane does not own. It is a one-line change now that `PageEncode`
  exists — say the word.
- **`ExportTextPath`, 2.8 s** for a plain text dump. The cost is upstream of the
  write (`commit_text_edit` / `script_dump`); worth its own look, not this lane's.
- **`OpenOraPath`, 1.4 s.** The read path, untouched here.
- The background half is still ~4 s a page, ~47 % of it `export::composite` for
  the `mergedimage.png` that the OpenRaster spec requires. It no longer freezes
  anything, so it was left alone: caching it would need a key over every layer's
  presentation state, and a stale merged image is a wrong-looking file.

## Tests

- `cargo test -p mn-app save_bg` — 4: `a_write_in_flight_draws_the_saving_pill`,
  `the_background_writer_lands_the_bytes_and_reports_back`,
  `a_failed_write_comes_back_as_an_error`,
  `the_id_guess_matches_what_save_folder_assigns`.
- `cargo test -p mn-app a_resave_of_an_unchanged_work_rewrites_no_page_files` —
  a clean re-save touches no page file (hashes + mtimes compared) and says
  "0 rewritten".
- `cargo test -p mn-app only_the_painted_layer_is_re_encoded` — an untouched
  document re-encodes **0** layer PNGs; after painting one layer it re-encodes
  exactly **1**.
- `cargo test -p mn-app every_blocking_file_command_is_timed` — the table above.
- `cargo test -p mn-app the_ora_encode_cost_splits_by_stage` — the stage table.
- Regression suites re-run green: `surface_file_tests` (13), `export_and_script_tests`
  (16), `promote_tests` (7), `document_tab_tests` (6), `page_switch_park_tests` (4),
  `parked_document_tests` (3), `unsaved_across_tabs_tests` (3),
  `autosave_folder_tests` (2), `save_duplicate_tests` (2), `open_in_tab_tests` (2),
  `mn-core ora` (50), `mn-core project` (10).

## Files this lane has touched (K)

Owned: `crates/app/src/cmd/file_io.rs`.

Outside the lane's original list — all uncontested by the other lanes, flagged
as required (round 2 adds the last two):
- `crates/app/src/cmd/save_bg.rs` — new file: queue, thread, pill, poll.
- `crates/app/src/cmd.rs` — `pub(crate) mod save_bg;` + one poll in `run_cmd_tail`.
- `crates/app/src/ui.rs` — two calls in `build` (poll + pill).
- `crates/app/src/app/page_files.rs` — the `_via` twins, `snapshot_active_page`,
  `page_bytes_for_folder_save`, `project_pages_for_save`.
- **`crates/core/src/ora.rs`** — the layer PNG cache, the cheap thumbnail, the
  compression comment.

`crates/app/src/app.rs` was deliberately not touched: the save state lives in
`save_bg`'s own module state rather than as an `app.saving` field.

---

## N research (10 min, EN + JP)

- **Circle = smooth anchor (角なし), square = corner anchor (角あり).** JP source,
  exact wording 「角なし」の制御点は「〇」（丸）で表示され、「角あり」の制御点は「口」（四角）で表示されます:
  https://dentakumanga.com/clipstudiopaint-vectorlayer-controlpoint/
- **Toggle:** the Control point sub tool's 「角の切り替え」 ("Switch corner") —
  tapping a control point flips curve ↔ corner. EN tool guide:
  https://www.clip-studio.com/site/gd_en/csp/toolguide/csp_toolguide/100_reference/Controlpoint.htm
- **Modifier:** with the Figure/Bezier tools, **Alt+click an anchor** turns a
  curve into a corner (Alt+drag turns a corner back). Correct line tool:
  Shift+Alt+click.
- MangaNakama **already** binds Alt+click on a balloon anchor to
  `Balloon::toggle_anchor_corner` (`app/canvas_input.rs::balloon_anchor_edit`,
  from the Object tool), undoable through `AppCmd::BalloonCommit`. So N is only
  the SHAPE of the drawn handle plus a line in `docs/manual/text.html`.

# O, L, M, N — the balloon pen

## O. The line was uniform because pressure was off everywhere

`BalloonSet.pressure_width` was `false` at every construction site in the
repo, `BalloonSet::new` included — and `BalloonSet::new` is the one the
balloon pen reaches through when the page has no balloon layer yet
(`cmd/text.rs`'s `BalloonAdd`). So the owner's bubbles were literally drawn
with the taper switched off. Cause (1) of the plan's three; cause (3) was ruled
out — `simplify_anchors` does carry each anchor's pen pressure.

Cause (2) was real but smaller: the width curve was `border_px * (0.35 + 0.65 * pr)`,
a 3× range with no min-size setting, which reads as near-uniform on a page.

**Changed:** `BalloonSet::new` turns pressure ON. A new per-balloon
`min_width` (CSP's 最小値) replaces the hardcoded 0.35, and the curve is
`border_px * (min + (1 - min) * pressure)`. Two defaults, deliberately: a
balloon drawn in this build starts at `MIN_WIDTH_DEFAULT` = 0.12 (a taper you
can see), a balloon loaded from an older file gets 0.35 through
`#[serde(default = "legacy_min_width")]` — so **nothing already on disk
changes shape when it is reopened**. The field went on `Balloon` rather than
`BalloonSet` on purpose: `Balloon` has a hand-written `Default` and every
literal in the tree already spells `..Default::default()`, so adding it cost
zero churn in ten test files.

"Min size %" is a bar in the balloon Tool Property (Object tool + a selected
balloon), shown only when the pressure toggle is on. Its in-progress drag value
lives in egui's scratch memory rather than as a third `*_edit` field on `App`
— one undo step per drag, and no edit to `app.rs`.

## L. The live trail now shows the mark you are making

`ui/overlay/frames.rs::balloon`, `BalloonMode::Draw` drew a flat 1.5 px accent
hairline whatever the balloon was going to be. It now draws a **mesh ribbon**
(one quad per segment, widths lerped, one draw call so joins cannot flicker) at
the outline's true half-width per sample, in the tool's line colour at its
opacity, with a 1 px accent centre-line on top so the trail stays findable over
black art.

The anti-drift rule: **there is one function that answers "how thick"** —
`BalloonSet::border_at(i, pressure)`. The rasterizer calls it; the preview
calls it, on a probe set built the same way `BalloonAdd` picks its target
layer. `the_balloon_pen_preview_matches_its_commit` drives a real pen drag,
captures the probe mid-drag, and asserts the preview's width equals the
committed balloon's at every recorded anchor.

## M. Spikes came out rounded because nothing looked for corners

`canvas_input.rs::finish_balloon_drag` built `BalloonShape::Polygon { ...,
corners: Vec::new() }` — a drawn balloon never got a single corner anchor.
`tessellate_closed` already handled corners correctly (it zeroes both
Catmull-Rom tangents there, which is a true kink), so the whole bug was the
missing detection.

**The threshold: `CORNER_TURN_DEG = 55.0`,** measured between the chord
arriving at a sample and the chord leaving it, each `CORNER_CHORD_PX = 6.0`
long. 55° is above the wobble of a hand drawing a smooth arc and below a
deliberate spike; the owner's screenshot was a turn well past 90°. Both are
named constants with the reasoning next to them.

Detection runs on the **raw** samples, before simplification — a spike's
sharpness is *how fast the direction turned*, and the anchors no longer know
that. The new `balloon::drawn_anchors` then cuts the trail at each corner and
simplifies each run separately, so a corner sample is a run endpoint and
Douglas-Peucker can neither drop it nor move it. The trail is treated as
closed, so a spike the artist happened to start drawing on is found like any
other.

## N. Two kinds of control point

Research first, EN + JP, both sources cited:

- **Circle = smooth anchor (角なし), square = corner anchor (角あり).** JP, exact
  wording 「角なし」の制御点は「〇」（丸）で表示され、「角あり」の制御点は「口」（四角）で表示されます:
  <https://dentakumanga.com/clipstudiopaint-vectorlayer-controlpoint/>
- **The toggle** is the Control point sub tool's 「角の切り替え」 ("Switch
  corner"): tapping a control point flips curve ↔ corner. EN tool guide:
  <https://www.clip-studio.com/site/gd_en/csp/toolguide/csp_toolguide/100_reference/Controlpoint.htm>
- **The modifier**, with the Figure/Bezier tools, is **Alt+click** on an anchor
  (Alt+drag turns a corner back into a curve); the Correct line tool uses
  Shift+Alt+click.

MangaNakama already bound Alt+click on a balloon anchor to
`Balloon::toggle_anchor_corner` (`canvas_input.rs::balloon_anchor_edit`, from
the Object tool, undoable through `AppCmd::BalloonCommit`) — which is CSP's
Figure/Bezier spelling. So N was only the drawn SHAPE: corner anchors are
squares, smooth anchors are circles. Ellipse and rounded-rect grips are resize
handles rather than anchors, so they keep the square every other transform
handle wears; tail handles are unchanged.

## Tests (O, L, M, N)

`cargo test -p mn-core <name>`:
- `the_balloon_outline_follows_pen_pressure` — a light hand inks less than
  half the outline of a heavy one, and with the toggle off the two are equal.
- `min_size_sets_how_far_the_taper_goes`
- `a_fresh_balloon_layer_follows_the_pen`
- `an_old_balloon_keeps_its_old_min_size` — a balloon deserialised without the
  field still gets 0.35.
- `a_drawn_star_keeps_all_ten_of_its_spikes` — every tip is a corner anchor
  and the tessellated outline passes within 0.5 px of it.
- `a_smooth_loop_gets_no_corners`
- `the_corner_threshold_is_the_line_between_smooth_and_kinked` — an octagon
  (45° a vertex) stays smooth, a hexagon (60°) kinks at all six.

`cargo test -p mn-app <name>`:
- `the_balloon_pen_preview_matches_its_commit`
- `alt_click_switches_an_anchor_between_corner_and_smooth` — exactly one
  anchor changes, and one undo puts it back.

Re-run green after these changes: `cargo test -p mn-core` (817),
`surface_text_tests` (18), `balloon` filter across the app (19),
`balloon_fit_tests` (4), `balloon_carries_text_tests` (5),
`remote_tests` (8), `tcy_button_tests` (4).

## Files touched for O, L, M, N

- `crates/core/src/balloon.rs` — `Balloon::min_width`, `BalloonSet::border_at`,
  `BalloonSet::new`, `raw_corners`, `drawn_anchors`, the constants, the tests.
- `crates/app/src/app/canvas_input.rs` — the BALLOON release path only
  (`finish_balloon_drag`); the ruler regions were not touched.
- `crates/app/src/ui/overlay/frames.rs` — the balloon band (live ribbon) and
  the balloon handle shapes in the object band, plus the two tests.
- `crates/app/src/ui/property/frames_balloons.rs` — the "Min size %" bar.
- `docs/manual/text.html`, `docs/manual/files.html`.
