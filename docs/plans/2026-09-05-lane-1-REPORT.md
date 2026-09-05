# Lane 1 — Shortcuts: defaults as rows, cycles you can edit

## Done / next

- **Done:** A1, A2, B1, B2. `cargo check -p mn-app --all-targets` clean, zero
  warnings. `cargo test -p mn-app keymap::` 10/10 green,
  `cargo test -p mn-app shortcut_tab::` 11/11 green.
- **Not mine, left alone:** B3 (the default `U` cycle in `builtin_targets`) —
  still `&[Target::Tool(Tool::Frame)]`, waiting on Lane 2's `Tool::Ruler`.
- **Next:** nothing in this lane. Fable reviews and commits.

Files touched: `crates/app/src/keymap.rs`, `crates/app/src/ui/shortcut_tab.rs`,
`crates/app/src/main.rs` (only `builtin_targets`'s new twin, `builtin_chords`'s
neighbourhood, `shortcut` and two helpers beside it), `docs/manual/keys.html`.
**No file outside the lane was edited**, and none was needed.

---

## What changed, in order

### B2 — one shape for a binding (`keymap.rs`)

`Bind` was `Cmd(AppCmd) | Targets(Vec<Target>)`. It is now:

```rust
pub enum Step { Cmd(AppCmd), Target(Target) }
pub enum Bind { Seq(Vec<Step>) }
impl Bind { pub fn steps(&self) -> &[Step] }
```

A single command is a one-step `Seq`. `Keymap::parse` treats a JSON string as a
one-item list and an array as the list it looks like, then maps each item
through one new helper, `parse_step` (the `tool:` prefix picks the namespace,
exactly as before). One bad entry still costs the chord one complaint and the
chord stays unbound — a cycle with a hole in it would walk somewhere its owner
never wrote.

`main.rs::shortcut` now calls `run_seq(app, steps, repeat)`:

- one `Step::Cmd` → the old repeat family (`Undo | Redo | LayerAbove |
  LayerBelow` repeat on auto-repeat, nothing else does);
- otherwise never on auto-repeat;
- all steps are targets → `subtools::press` verbatim, so tool-only keys behave
  exactly as before;
- mixed → stateless walk: `step_is_current` (a target via `Target::matches`, a
  `RulerArm(k)` via `app.ruler_pending == Some(k)`, every other command never),
  run the step after it, step 0 if none is current.

**One judgement call to flag.** Several steps can be current at the same time:
arming a ruler does not take the tool out of your hand, so the tool step under
it still matches. I take the **last** match, not the first. That is what makes
the plan's acceptance lap come out right (Frame border → Figure → ruler armed →
Frame border); taking the first match breaks at the third press. It is pinned
by `a_mixed_cycle_advances_from_the_current_step`.

**Known rough edge, please read.** `app.ruler_pending` is sticky — nothing
clears it until a canvas drag creates the ruler. So on the *second* lap of a
mixed cycle the ruler step is still "current" and the walk parks on step 0
instead of moving on. I did not fix it: clearing `ruler_pending` from a
shortcut is a ruler-behaviour change in Lane 2's territory, and Lane 2 dissolves
the problem anyway — once the ruler is a `Tool`, `is_current` is
`tool == Ruler && ruler_mode == k`, which is mutually exclusive with the other
tool steps and the ambiguity disappears. B3's default `U` is all-targets, so it
never hits this.

### A1 — one table of every binding (`keymap.rs`, `main.rs`)

`main.rs` gained `builtin_target_rows() -> Vec<(Chord, &'static [Target])>`.
**Deviation from the plan, deliberate:** it is not a hand-written twin of the
`builtin_targets` match — it is a list of the sixteen tool vks that *calls* the
match. Same result, no duplicated values, and the only thing that can drift is
the vk list. The pin test walks the whole vk space in both directions and fails
if the match answers a chord the list forgot.

`keymap.rs` gained `Source { Default, File }`, `Binding`, and
`effective_table(file: &[Entry]) -> Vec<Binding>`.

```rust
pub(crate) struct Binding {
    chord: Chord,
    key: String,               // as keys.json spells it
    entries: Vec<String>,      // one per step: "tool: Frame border", "Undo"
    source: Source,
    spellable: bool,           // can `entries` go into keys.json as they are?
    shadows: Option<Vec<String>>,  // the default a File row replaced
    raw: Option<serde_json::Value>, // a value chips cannot cover, verbatim
}
```

**Two deviations from the plan's wording:**

1. The plan says `entries: Vec<Entry>`. `Entry` is already the name of a
   keys.json key/value pair in this module, so the steps are `Vec<String>` —
   each one the keys.json spelling of a step. That is also what makes "edit a
   default row → it becomes a file row" a copy, not a translation.
2. The plan says `effective_table(app)`. It needs nothing from `App`, and the
   tab has to merge its *unsaved* rows, not the file on disk — so it takes the
   entries. That also keeps the new tab tests app-free (no
   `headless_renderer`).

Merge order: target rows first (the bare tool letters are in *both* built-in
tables and the target row is the one that can spell itself back), then
`builtin_chords` rows for chords not already present, then the file — a file row
replaces the default of its chord in place and keeps it in `shadows`. Sorted
bare letters → other bare keys → function keys → modifier chords.

New helper `keymap::target_text` is `parse_target`'s inverse and produces the
same strings `shortcut_tab::addable()` offers, so a default row turned into a
file row loads back as the same target.

### A2 + B1 — the tab (`ui/shortcut_tab.rs`)

- The list is `effective_table` over the tab's own rows. Built-ins draw weakly
  with a **default** tag; editing the key or the value flips the row to
  `Source::File` and records what it now shadows.
- A File row that shadows a default shows `default: …` and a **↺** that removes
  the file row (`restore_default`); a File row that shadows nothing keeps its
  **×**.
- Value cells are **chips**, one per step, with `×` and `‹ ›`. Chips are derived
  from the row's edit text every frame (`chips_of`) and written back through
  `rewrite`, so the chip view and the raw text can never drift apart.
- The raw-text edit is kept two ways: rows chips cannot cover (a `_`-comment
  lane, `42`) still edit as raw JSON, and a new **as text** checkbox puts every
  row in that mode.
- **Add appends.** `add_to_chord` pushes the picked label onto the chord's
  existing cycle instead of re-aiming it, status `"u" now cycles 3`. Re-aiming
  is the chip `×` plus an add. A free chord still starts a fresh row.
- `conflict_note` lost its `builtins` argument: built-ins are rows now, so
  there is one lookup instead of two, and it says "picking adds to it".
- Entries whose key is not a chord at all (`_comment`, `hyper+9`) are not in the
  merged table; the tab appends them as raw File rows so they round-trip.
- Only `Source::File` rows are serialized and only they are checked for
  duplicate keys.

---

## Finding for Fable: ~30 built-in chords cannot be spelled in `keys.json`

`builtin_chords` labels are display hints, not palette command names, and for
many of them **the palette has no command at all**: `MergeDown`, `Reselect`,
`SwapColors`, `PasteInPlace`, `StampVisible` (it exists under a different
label), the brush-size/opacity steps, the sub-tool steps, `Zoom100` (palette
calls it "Pixel size (100%)"), the view-rotate steps, `FillSelection`,
`ClearOutside`, `LayerAbove`/`LayerBelow`, `NextDoc`, `FlipView`/`FlipViewV`…
39 of the 65 labels have no exact palette twin; subtracting the tool letters
(covered by target rows) leaves roughly **30 command rows**.

Those rows are `spellable: false`: listed, tagged **default**, and frozen — the
tab says "a built-in with no keys.json spelling — bind the chord below to
shadow it", and shadowing still works. I did **not** fuzzy-match labels
("Open" → "Open…"): a wrong guess would silently bind the wrong command.

The honest fix is to add the missing entries to
`ui/quick.rs::command_index()` — which also makes them runnable from Ctrl+K,
a win on its own. `ui/quick.rs` is not Lane 1's file, so I stopped here.

---

## Tests

`cargo test -p mn-app keymap::` — 10 passed:

- existing: `chords_parse_exactly`, `a_broken_entry_does_not_cost_the_file`,
  `lookup_is_exact_on_modifiers`, `a_binding_reaches_the_command_queue`,
  `a_chord_can_name_a_tool_target`, `garbage_files_degrade_to_empty`
- new: `a_cycle_may_mix_commands_and_targets`,
  `every_builtin_target_row_is_the_match`,
  `the_merged_table_lists_defaults_and_the_file`,
  `a_mixed_cycle_advances_from_the_current_step`

`cargo test -p mn-app shortcut_tab::` — 11 passed:

- existing: `the_round_trip_preserves_every_entry`,
  `a_broken_raw_edit_keeps_its_last_valid_value`,
  `the_conflict_lookup_names_both_sides`, `every_builtin_chord_still_consumes`,
  `a_saved_file_rebinds_without_restart`, `chord_text_round_trips`,
  `the_addable_namespace_is_all_parseable`,
  `a_duplicate_or_empty_key_refuses_to_save`
- new: `adding_to_a_bound_chord_appends_to_its_cycle`,
  `restoring_a_shadowed_default_removes_the_file_row`,
  `chip_edits_reorder_and_restore`

`a_mixed_cycle_advances_from_the_current_step` is the plan's "main:" test; it
lives in `keymap.rs` beside `a_binding_reaches_the_command_queue`, which is the
existing `main.rs`-driving test.

## Owner eye test (nothing blocks on it)

Preferences ▸ Shortcuts: the list should now open with grey **default** rows —
`u` → Frame border, `ctrl+z` → Undo. Press `u` in the Add box (the note should
read "built-in: Frame border — picking adds to it"), search `Figure`, click
`tool: Figure`: the `u` row turns into two chips with a `default: Frame border`
hint and a ↺. Add "Straight line ruler": three chips. Save & apply, then press
`u` on the canvas three times — Frame border, Figure, ruler armed. (The fourth
press is the sticky-`ruler_pending` edge described above; it goes away with
Lane 2.)
