# Lane 2 — Ruler is a tool (item C) — REPORT

## PAUSED 2026-09-05 (coordinator asked all agents to stop). Tree compiles.
`cargo check -p mn-app --all-targets` — clean, ZERO warnings, at the pause.
Nothing half-edited. Resume by re-reading this file + `git status`.

## done / next

DONE — C1 `Tool::Ruler` (label "Ruler"), strip cell after Eyedropper and
       before Move view, new `Icon::Ruler` glyph, `strokes()` stays false
       (it is not in the `strokes()` list), `GROUPS` third block 6 → 7.
DONE — C2 `SubTool::Ruler(RulerKind)` × 12 + `subtools::group::CREATE_RULER`.
       All four CODE-MAP places: `SubTool::ALL` block, `group_of` arm,
       `apply_state` arm, `is_current` arm.
DONE — C3 `App::ruler_mode: RulerKind` (default `Line`), `App::ruler_arm()`,
       `AppCmd::RulerArm(k)` now = `SetSubTool(SubTool::Ruler(k))` + the
       one-shot `ruler_pending` + the status line.
DONE — C4 Tool Property: new `crates/app/src/ui/property/rulers.rs`
       (`sec_ruler_tool` / `sec_ruler_snap` / `sec_ruler_guide`), wired as
       the `Tool::Ruler` arm of `prop_sections`.
DONE — C5 targets verified by test (see below); no keymap change.
DONE — C6 memory: the row rides `is_current`/`apply_state`, so `note_memory`
       and `restore_from_memory` carry `ruler_mode` like any other mode.
DONE — C7 target strings, below. main.rs untouched.
DONE — extra, same lane: the Ruler MENU (`ui/top.rs`) now walks
       `RulerKind::ALL` instead of twelve hand-typed rows, so the menu, the
       Sub Tool list and a `keys.json` target offer the same rows in the
       same order under the same names. Both ladder rows (ring spacing,
       symmetry lines) kept, under the row they re-count.

NEXT, in order:
1. `docs/manual/rulers.html` — say rulers are a tool now (left strip ▸ Ruler,
   Sub Tool ▸ Create ruler, twelve rows; the Layer ▸ Ruler menu still works
   and selects the tool). NOT STARTED.
2. `docs/CODE-MAP.md` — the "sub tool row exists in three places" paragraph
   (line ~498): add the ruler rows as an example and note that `is_current`
   for a ruler row also asks which TOOL is in hand (the one exception, and
   why). NOT STARTED.
3. Three test runs not yet made (one filter per invocation):
   `cargo test -p mn-app a_mixed_cycle_advances_from_the_current_step`
   (keymap, Lane 1's — must still pass, `ruler_pending` is deliberately
   still set), `cargo test -p mn-app ui::tools::tests` (strip counts +
   unique glyph), `cargo test -p mn-app the_perspective_family_is_in_the_palette`
   (quick.rs palette rows still resolve).
4. Optional, only if Fable wants it: `ui/quick.rs` has no palette row for
   the two GUIDE kinds (it never did). Everything else already routes.

## Tests that PASS as of the pause
- `subtools::tests::the_registry_holds_every_row_once` (extended: the Create
  ruler tab IS `RulerKind::ALL`, in order)
- `subtools::tests::a_ruler_row_reports_current` (new)
- `app::ruler_undo_tests::the_ruler_tool_drag_creates_a_line_ruler` (new)
- `cargo test -p mn-app ruler` — 24 tests, all green (every pre-existing
  ruler test in `ruler_undo_tests.rs`, `app/tests.rs`, `tone_round_tests`,
  `smart_shape_tests`, `tab_switch_state_tests`)
- `cargo test -p mn-app subtools::tests` — 12 tests, all green
- `ui::shortcut_tab::tests::the_addable_namespace_is_all_parseable` (read
  only, Lane 1's file — not edited)

## Target strings (C7 — for Fable's U default cycle)
- `tool: Ruler`
- `tool: Ruler / Create ruler`
- `tool: Ruler / Create ruler / <row>`, the twelve rows in list order:
  1. Straight line
  2. Curve
  3. Parallel line
  4. Radial line
  5. Concentric circle
  6. Vanishing point
  7. Perspective 1-point
  8. Perspective 2-point
  9. Perspective 3-point
  10. Symmetrical
  11. Guide horizontal
  12. Guide vertical
- The U cycle's third step: `tool: Ruler / Create ruler / Straight line`
- When that lands, flip `Tool::Ruler => ""` to `"U"` in
  `crates/app/src/ui/tools.rs::tool_key` (the strip tooltip).

## What happened to `ruler_pending`
KEPT, narrowed to the ONE-SHOT half, and nobody reads it directly any more
except the two writers. `App::ruler_arm()` (in `app/canvas_input.rs`) is the
single reading:

    Some(ruler_mode)  while Tool::Ruler is in hand
    ruler_pending     otherwise

`RulerArm(k)` still sets `ruler_pending = Some(k)` on top of selecting the
tool, ON PURPOSE: that keeps `main.rs::run_seq`'s `step_is_current`
predicate (`app.ruler_pending == Some(*k)`) true, so **main.rs needed no
change** and Lane 1's `keymap::tests::a_mixed_cycle_advances_from_the_
current_step` (which asserts `app.ruler_pending == Some(RulerKind::Line)`)
still compiles and passes. The release clears `ruler_pending` where it used
to `take()` it, so a menu pick is still spent by one gesture while the TOOL
stays in your hand and builds a second ruler on the next drag.

## Files touched outside the lane's list (unavoidable, all small)
- `crates/app/src/app.rs` — the `ruler_mode` field + its init + ONE
  `step_subtool` match arm the compiler demands (Lane 4 already landed).
- `crates/app/src/cmd/layers.rs` — the `RulerArm` arm only (Lane 3 landed).
- `crates/app/src/ui/tools.rs` + `crates/app/src/ui/icons.rs` — the strip
  cell and its glyph. C1 is unimplementable without them.
- `crates/app/src/ui/property.rs` — `mod rulers;` + the `Tool::Ruler` arm of
  `prop_sections`. (Lane 5 owns the BALLOON sections in
  `property/frames_balloons.rs`; not touched.)
No balloon code touched anywhere. No `main.rs`, no `keymap.rs`, no
`shortcut_tab.rs`.

## One judgement call to flag for review
`is_current(SubTool::Ruler(k))` is `app.tool == Tool::Ruler && app.ruler_mode
== k` — exactly as the plan writes it, but it is the ONE row kind that asks
which tool is in hand, which the module doc says rows do not do. Reason in
the comment: a ruler row is an ARMED gesture, not a carried setting. The
ui.txt memory is unaffected because `note_memory` snapshots every frame and
merges, so the row is written down while the tool IS in hand.

---

# RESUME 2026-09-06 — queue from the coordinator

## done / next
- DONE item 1: `docs/manual/rulers.html` + `docs/CODE-MAP.md`
- DONE item 2: SaveDuplicate 4266→160 ms, ExportText 2801→13 ms; OpenOra shape reported, NOT done
- DONE item 3: all 32 unspellable chords resolve now (18 new palette rows + 14 aliases + 7 new AppCmds)
- NOTHING LEFT in the queue. Not committed.

## Item 1 — the two owed docs (DONE)
`docs/manual/rulers.html`: new intro ("Ruler is a tool", strip position,
Create ruler, the tool stays in hand) + a new "The Ruler tool" section with
the twelve rows in list order, the U cycle, the three `keys.json` target
spellings, `,`/`.`, where the per-kind numbers moved (Tool Property), and
what the Layer ▸ Ruler menu still is (a shortcut into the tool; a menu pick
arms ONE gesture, the tool arms every one). Also corrected the stale line in
"The creation drags" that told the reader a refused short drag means going
back to the menu — true only for a menu pick now.

`docs/CODE-MAP.md`: two bullets after the "sub tool row exists in three
places" paragraph — (a) the Ruler rows as the worked example, naming
`RulerKind::ALL` as the canonical order+labels walked by three places and
the pin test that holds them together, plus WHY `is_current` for a ruler row
is the one tool-aware arm; (b) `ruler_pending` vs `ruler_mode`, with
`App::ruler_arm()` as the single reading and why `RulerArm` still sets
`ruler_pending` (main.rs's `step_is_current` predicate).

## Item 2 — the three file commands that still blocked the window thread

Measured the same way Lane 5 did:
`cargo test -p mn-app every_blocking_file_command_is_timed -- --nocapture`
(debug build, 3-page work, B4 at 200 dpi = 2024 × 2866 px, 4 layers).

| command | Lane 5's number | now blocks the pump |
|---|---|---|
| `SaveDuplicatePath` (.mnc) | 4 266 ms | **160 ms** |
| `ExportTextPath` | 2 801 ms | **13 ms** |
| `OpenOraPath` (.mnc, 3 pages) | 1 357 ms | 1 857 ms — NOT DONE, see below |

### SaveDuplicate — DONE, Lane 5's pattern exactly
`app/save_duplicate.rs` now submits all three of its branches to
`cmd::save_bg` instead of encoding and writing inline: bare `.ora` →
`Write::Ora` with a `Document::clone` snapshot; `.mnc` →
`project_pages_for_save()` + `Write::Project`; work folder →
`save_work_folder_via` + `Write::Folder`, the same closure `SaveOraPath`
uses. `was_a_save` is **false** on all three — a duplicate that fails must
not re-dirty the work, because the work never changed. The ledger
borrow/give-back is untouched and still runs on the window thread, because
`save_bg::folder_page_ids` answers the only question the write used to.

Two side effects worth knowing:
- `App::save_work_folder` (the synchronous twin) had no callers left and is
  GONE. Every folder write is now `save_work_folder_via` → `save_bg`. A
  second entry point that encoded on the window thread is exactly how this
  command stayed a freeze after the others stopped being one.
- `save_bg::Write::Folder` gained `verb: &'static str`, because three
  callers submit an identical job and only they know what to call it.
  "saved work folder" / "autosaved work folder" / "duplicate written to".
  Saying "saved" after a Save Duplicate is the one that actually misleads.
The 160 ms that remains is the GPU page preview + palette thumbnail, both of
which need the renderer — same floor every other `.mnc` save has.

### ExportText — the cost was NOT the write, and not a thread
2.8 s for a plain text file, and the write is a `fs::write` of ~2 KB.
**Cause:** `App::script_dump` (`app/story.rs`) walks every page for its text
items and its panel rectangles, and reached the non-active pages through
`mn_core::project::bytes_to_doc` — a FULL page load: a PNG decoded per
layer, every frame raster re-derived, every balloon re-rasterized. All of it
thrown away a moment later. Both things a script dump reads (`mnc-texts` and
`mnc-frames`) ride `stack.xml`, which is one zip entry.
**Fix:** `ora::load_meta_from` / `project::bytes_to_doc_meta` — the same
loader with the pixel work skipped (no layer PNG, no frame derive, no
balloon rasterize, no mask, no stroke sidecar). `script_dump` uses it.
2 801 ms → **13 ms**, and no thread, no pill, no new failure mode.

**TRAP, recorded for whoever finds `load_meta_from` next.** The Story
Editor's `story_refresh` decodes every page the same way and looks like the
same fix — it is NOT. `story_docs` documents are EDITED and re-encoded
(`story.rs` `doc_to_bytes` → `pages[p].bytes`), so loading them without
pixels would write that page back to disk BLANK, silently. The rule is in
the function's doc comment: metadata-only is for passes that ASK a page
something, never for one that will save it.

### OpenOra — NOT DONE. The shape, as instructed.
Backgrounding a LOAD is not the same size as backgrounding a save, and it is
past "medium":
1. The three branches of `AppCmd::OpenOraPath` (work folder / single-file
   `.mnc` / bare `.ora`) each end in a ~40-line INSTALL block that writes
   ~20 `App` fields (`prepare_open_target`, `doc`, `pages`,
   `adopt_page_uids`, `adopt_folder_state`, `renderer.invalidate`,
   `fit_to_view`, `set_doc_path`, `mark_saved`, `note_recent`). All of that
   must move out of the arm into functions the poll can call later. A save
   had nothing to install; this is the whole difference.
2. The install must run at ARRIVAL, not at submit: `prepare_open_target`
   decides whether the file lands in a new tab, and the answer can change
   while the load runs.
3. New states nothing handles today: a second open while one is in flight, a
   tab closed under a load, a save queued onto the path being read.
4. `save_bg`'s "no frames, no background" rule has to be mirrored, or every
   test that opens a file and asserts on `app.doc` in the next line breaks.
5. It is also the least valuable of the three: 1.4–1.9 s is under the ~5 s
   "not responding" threshold, it happens once at the start of a session
   rather than in the middle of drawing, and it is a DEBUG number — the
   owner's `play/` build is `--release`, where our own pixel loops run
   several times faster.
Recommendation: leave it, or take it as its own lane with the install-block
refactor as step one.

## Item 3 — every default chord is now spellable in keys.json

**Where it stood.** Lane 1 made every built-in a ROW, and marked 32 of them
`spellable: false` — listed, shadowable, but not re-aimable, because
`keymap::parse` resolves a label through `ui::quick::command_index()` and
those labels had no row there. `spellable` is COMPUTED from that index
(`keymap.rs:298`), so the freeze lifts by itself the moment a label resolves:
**no change was needed in `keymap.rs` or `shortcut_tab.rs`** for the
un-freezing. (Their "a built-in with no keys.json spelling" branches stay, as
the guard rail for the next built-in that arrives without a command.)

**What the 32 split into.**
1. **18 rows the palette simply did not have** — added to `command_index()`
   proper, so they are Ctrl+K-runnable too, which was the "win on its own":
   Reselect, Paste in place, Merge down, Layer above, Layer below, Close
   document, Swap colours, Reset colours, Transparent colour slot, Rotate
   view left/right, Brush size down/up, Brush opacity down/up, Previous/Next
   sub tool, Delete / clear layer.
2. **14 commands the palette already had under a different label** (Save As…
   vs "Save As", "Pixel size (100%)" vs "Zoom 100 %", …) — these went into a
   new `shortcut_aliases()`. `command_index()` = `palette_commands()` +
   aliases, and the two SEARCH UIs (Ctrl+K and the Quick Access picker) call
   `palette_commands()`. So the keys.json namespace grew and the palette did
   not sprout 14 near-duplicate rows. Renaming the existing rows instead was
   not an option: a palette label IS a keys.json binding name, and renaming
   one silently unbinds whatever key a user put on it. Fuzzy matching
   ("Open" → "Open…") was rejected for Lane 1's reason — a wrong guess binds
   the wrong command.

**Seven new `AppCmd`s, and why each had to exist.** Six of those 18 rows had
no command because the key reads LIVE STATE, and a row carrying a baked value
would have changed behaviour the moment it was rebound:
`StepBrushSize(bool)` and `StepBrushOpacity(bool)` (the ladder / the 5 %
step), `StepSubTool(bool)` (`,`/`.`), `RotateViewStep(bool)` (the step is the
`rotate_step_deg` PREFERENCE, not 15°), `ToggleTransparentSlot` (C toggles,
it does not set), `CloseWindow` (Ctrl+W only arms `close_requested`; the
message loop runs the save prompt). The seventh is `DeleteOrClear`: the Del
key is a CHAIN (in the Object tool it deletes the picked text / balloon /
panel / vector / multi-selection, else it clears the layer), and binding
"Delete / clear layer" to plain `ClearLayer` would have silently dropped the
object half. Arms live in `cmd/brush.rs`, `cmd/misc.rs`, `cmd/edit.rs`.

**Duplication I am leaving behind, on purpose, with a pin.**
`main::shortcut` still has the Del chain inline; `cmd::edit::
delete_or_clear_target` is the same decision as a function. `main.rs` is not
this lane's file, so the two coexist and
`cmd::edit::delete_key_tests::the_delete_command_is_the_del_key_verbatim`
drives the real key press and the command in three states and asserts they
answer the same. **When `main.rs` is next open: point its Del arm (and the
`[`/`]`, `,`/`.`, `-`/F9, C and Ctrl+W arms) at the new commands and delete
the twins.** Each is a one-line change.

**Three failing tests I found and fixed — they were RED at HEAD, before I
touched anything.** `abe1231` (Fable's U default cycle) changed U's built-in
from `tool: Frame border` to the three-step cycle, and four of Lane 1's tests
hard-coded the old value: `shortcut_tab::adding_to_a_bound_chord_appends_to_
its_cycle`, `::the_conflict_lookup_names_both_sides`,
`::restoring_a_shadowed_default_removes_the_file_row`, and
`keymap::the_merged_table_lists_defaults_and_the_file`. I fixed the
EXPECTATIONS only, and made them READ the default instead of spelling it, so
the next change to the table cannot break them the same way. That is more
than the "one-line change" that was allowed in those two files — flagging it
explicitly. CI was red without it.

### Tests
- new `ui::quick::builtin_chord_tests::every_builtin_chord_label_resolves_through_the_palette`
  — the pin that was asked for: every `builtin_chords()` label resolves
  through `command_index()`, tool letters exempted (they spell themselves as
  `tool: …` targets). It prints the missing labels when it fails.
- new `ui::quick::new_command_tests::the_new_shortcut_commands_are_claimed_and_do_their_job`
  — `cmd::misc`'s chain ends in `unreachable!("AppCmd claimed by no cmd
  module")`, so a variant with no arm COMPILES and panics the first time it
  runs. This runs all six and checks each did its job.
- new `cmd::edit::delete_key_tests::the_delete_command_is_the_del_key_verbatim`
- green: `shortcut_tab::` (11), `keymap::` (10), `ui::quick` (16),
  `save_bg` (4), `surface_file_tests` (13), `save_duplicate_tests` (2),
  `export_and_script_tests` (16), `mn-core a_meta_load_brings_the_vectors_and_no_pixels`.
- `cargo check --workspace --all-targets`: clean, ZERO warnings.
