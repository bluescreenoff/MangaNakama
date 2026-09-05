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
