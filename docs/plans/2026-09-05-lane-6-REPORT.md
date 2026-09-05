# Lane 6 — item P: a balloon drawn over text joins the text's layer

## DONE / NEXT
- **DONE — everything.** Step 1 sizing (below), then the HONEST route: one vector kind
  `LayerKind::Speech(SpeechSet { texts, balloons, born })` replacing `Text` and `Balloon`.
  All seven acceptance points are done. `cargo check --workspace --all-targets` is clean with
  ZERO warnings. Nothing committed (Fable reviews and commits).
- **NEXT (for Fable, not blocking):** `docs/manual/text.html` says "balloon layer" / "text layer"
  in a couple of places and could mention that one layer now holds both — that file belongs to
  Lane 5, so I did not touch it.

---

## Step 1 — the size estimate, written BEFORE any edit

Grep counts, whole workspace, `.rs` only (`.git/kb022_fixed.rs` is a stray, ignored).

### Direct `LayerKind::Text` / `LayerKind::Balloon` matches — 55 total

| file | Text | Balloon |
|---|---|---|
| `crates/core/src/doc.rs` | 18 | 9 |
| `crates/core/src/align.rs` | 4 | 2 |
| `crates/core/src/resample_work_tests.rs` | 2 | 2 |
| `crates/core/src/ora.rs` | 1 | 1 |
| `crates/core/src/text.rs` | 1 (doc comment) | – |
| `crates/core/src/balloon.rs` | – | 1 (doc comment) |
| `crates/core/src/preflight.rs` | – | 1 |
| `crates/app/src/text_edit.rs` | 6 | – |
| `crates/app/src/ui/layers.rs` | 2 | 3 |
| `crates/app/src/app/layer_defaults.rs` | 2 | 1 |
| `crates/app/src/app/tests.rs` | 2 | – |

`crates/mcp` has **zero**: it is one `main.rs` of JSON tool descriptors that forwards to
`crates/app/src/remote.rs`, which only ever calls `texts()` / `balloons()` / `kind_label`.

### Accessor call sites — ~230, and none of them had to change

`set_texts` 42, `set_balloons` 22, `texts()` ~140, `balloons()` ~90, `is_text()` 18,
`is_balloon()` 21. Every one kept its name and signature; only the bodies (all in `doc.rs`)
changed. That is what made the honest route cheap.

### Estimate: ~5 hours — under a day, so: HONEST ROUTE

The expensive-sounding part (undo, MCP, palette, two tools) turned out to be one-file work,
because everything already went through the accessors. Actual time was close to the estimate;
the two things I did not predict were `SpeechSet.born` (below) and one test-helper fix.

---

## What shipped

### The kind
`crates/core/src/doc.rs`:

```rust
pub struct SpeechSet { pub texts: TextSet, pub balloons: BalloonSet, pub born: SpeechBorn }
pub enum SpeechBorn { Text, Balloon }
pub enum LayerKind { …, Frame(FrameSet), Speech(SpeechSet) }   // Text and Balloon are gone
```

- `texts()` / `balloons()` answer `Some` for EVERY speech layer, with an empty set for the half
  that is not there. `speech()` gives both.
- `is_text()` / `is_balloon()` answer by CONTENT, and a layer can be both.
- **`born` is the tiebreak for the EMPTY layer, and it is not optional.** Content cannot tell an
  empty text layer from an empty balloon layer, and empty balloon layers are a real feature:
  MCP's `layers.add_balloon` deliberately makes one for a script to fill on the next call
  (`remote_layers_add_balloon_lets_a_script_letter_from_zero` caught this).
- `SpeechSet::rasterize` = balloons first, then `TextSet::rasterize_over` lays the words on those
  tiles. One pass, one tile map. `rasterize_over` returns the base untouched (same `Arc`s) when
  there are no cached sprites, so a bubbles-only layer costs exactly what it did before.
- `BalloonSet` gained `Default` (= `new(DEFAULT_BORDER_PX)`, 6.0 px placeholder). Every path that
  puts the FIRST bubble on a layer overwrites it from Tool Property, and there is a test for that.

### The seven points

| # | point | status |
|---|---|---|
| 1 | BalloonAdd targeting | **DONE**, and the mirror too. A bubble whose body encloses a text on a visible, unlocked layer joins THAT layer (`mn_core::balloon::text_in`), beating the old rule. Words typed inside an existing bubble join the bubble's layer (new `mn_core::balloon::body_at`). |
| 2 | Rendering: balloons then texts, one pass | **DONE** — `SpeechSet::rasterize` / `TextSet::rasterize_over`. |
| 3 | Layers palette: one row, combined glyph | **DONE** — one row falls out of the merge; the thumbnail is the layer's own tiles so it shows both; new `Icon::Speech` = the Lucide `type` glyph wearing a small `message-circle` mark, via a new `Lucide::Marked(subject, mark)` (the old `Badged` is now `Marked(x, "plus")`). No new icon asset was needed. |
| 4 | Move / transform moves both | **DONE** — `Layer::translate_vectors` and `Layer::scale_geometry` have one Speech arm doing both sets; `align.rs::shift_target` now returns a `Vec<UndoGroup>` so a speech move records both halves. |
| 5 | Selection stays separate | **DONE** — `object_candidates_at` already walks texts and balloons in separate passes, so it yields `Text(li,0)` and `Balloon(li,0)` for the same `li`; `balloon_sel` / `text_sel` are unchanged. |
| 6 | MCP + palette commands take either kind | **DONE** — `balloon_layer_arg` and `texts.list` accept any speech layer (the accessors answer `Some`); `kind_label` says `text` / `balloon` / `speech`; `layers.list` rows gained `"texts"` and `"balloons"` counts; the `layers_list` tool description in `crates/mcp` says so. |
| 7 | Tests | **DONE** — see below. |

### `.ora` format change, and how old files load

**The only change: `mnc-texts` and `mnc-balloons` may now ride the SAME `<layer>` element.**
No new attribute, no version bump.

- Save: write `mnc-balloons` when the layer holds balloons OR was born a balloon layer; write
  `mnc-texts` when it holds texts OR was born a text layer. So every speech layer writes at least
  one attribute (an empty one must not come back a plain raster), and an unmerged text or balloon
  layer's file is byte-for-byte what it always was.
- Load: a layer with either attribute becomes `Speech`; the missing half loads empty; `born` comes
  from which attribute rode the element (both present ⇒ born Text, the CSP reading).
- **An old file with a text layer and a balloon layer loads as TWO layers, each holding one half.
  Nothing is ever merged on load.** Pinned by `an_old_files_separate_text_and_balloon_layers_stay_two_layers`.

---

## Tests that pass

New:
- `mn-core`: `ora::tests::an_old_files_separate_text_and_balloon_layers_stay_two_layers`,
  `ora::tests::an_empty_speech_layer_comes_back_the_half_it_was_made_for`.
- `mn-app` (new file `crates/app/src/app/speech_layer_tests.rs`):
  `a_balloon_drawn_around_a_text_lands_on_the_texts_layer`,
  `a_balloon_drawn_away_from_any_text_keeps_the_old_rule`,
  `moving_the_speech_layer_moves_the_words_and_the_bubble`,
  `the_object_tool_still_picks_the_text_and_the_bubble_apart`,
  `words_typed_inside_a_bubble_join_the_bubbles_layer`.

Existing suites, all green:
`mn-core ora` 53/53 · `mn-core balloon` 56/56 (incl. **`text_and_balloon_item_ids_mint_and_stay_stable`**)
· `mn-core project` 10/10 · `mn-core resample_work` 8/8 · `mn-core align` 10/10 ·
`mn-app surface_text_tests` 18/18 · `mn-app surface_file_tests` 13/13 ·
`mn-app export_and_script_tests` 16/16 · `mn-app save_bg` 4/4 · `mn-app balloon` 19/19 ·
`mn-app remote_tests` 8/8 · `mn-app story` 12/12 · `mn-app layers` 43/43 ·
`mn-app surface_e2e_tests` 1/1 · `mn-app lucide` 1/1 · `mn-mcp` 2/2.

### Two flakes seen, neither caused by item P
1. `app::surface_file_tests::f06_autosave_and_recovery` failed once inside the full
   `surface_file_tests` run, at two DIFFERENT assertions on two runs, then passed 13/13 on a
   re-run of the same binary, and passes alone. It is a race against Lane 5's off-thread save
   writer (`AppCmd::Autosave` returns before the writer has landed the file). f06 does not create
   a single text or balloon layer, so item P cannot reach it. Worth a `wait for the writer` step
   in that test — Lane 5's territory.
2. Running a WIDE filter (`cargo test -p mn-app text`, 55 GPU tests at once) loses the device:
   `[gpu] DEVICE LOST (Unknown): Out of memory` on the Intel UHD 620, and whichever two tests were
   mid-readback panic. A different pair each run; all pass under the narrower named filters. This
   is the reason the house rule says targeted filters, not full runs.

---

## Files touched
`crates/core/src/`: `doc.rs`, `text.rs`, `balloon.rs`, `align.rs`, `ora.rs`, `preflight.rs`,
`lib.rs`, `resample_work_tests.rs`.
`crates/app/src/`: `app.rs` (mod decl only), `app/layer_defaults.rs`, `app/tests.rs`,
`app/speech_layer_tests.rs` (new), `cmd/text.rs`, `remote.rs`, `text_edit.rs`,
`text_edit/surface_text_tests.rs`, `ui/icons.rs`, `ui/icons/svg.rs`, `ui/layers.rs`,
`ui/layers/property.rs`, `ui/layers/rows.rs`.
`crates/mcp/src/main.rs` (one tool description). `docs/CODE-MAP.md` (a new "Speech layers"
section — a different part of the file from Lane 2's sub-tool paragraph).

## Traps a later agent should know
- `texts().is_some()` no longer means "this layer has words in it" — it means "this is a speech
  layer". Two places in the codebase meant the first thing and had to be changed
  (`surface_text_tests::text_layers`, `ui/layers/property.rs`'s balloon block). If a future bug
  looks like "a plain text layer is being treated as a balloon layer", this is why.
- Any seam that installs HALF a speech layer (`set_texts`, `set_balloons`, both undo arms,
  `align.rs`) must re-derive the raster from BOTH halves, or the other half vanishes from the page
  while staying in the model. All current seams do.
