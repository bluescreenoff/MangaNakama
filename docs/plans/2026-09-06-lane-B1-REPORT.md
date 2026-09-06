# Lane B1 — Tool Property rows, the settings window, text converted

> **DONE / NEXT**
> - **DONE: B1.1, B1.2, B1.3, B1.4 — the whole lane.** Gates green:
>   `cargo check --workspace --all-targets` **zero warnings**; all six new
>   `prop_rows` tests pass; the existing `prop_hidden` / ui.txt / workspace
>   tests pass (one assertion updated, see Deviations).
> - **NEXT: Fable reviews the diff and commits.** Then lane B2 converts
>   `frames_balloons.rs`, `tone.rs`, `select.rs`, `gradient.rs`, `rulers.rs`
>   to rows the same way — they all still draw as one-row sections today.
> - B1.5 (figure rows) was SKIPPED on instruction: A2 already landed, B2
>   converts that file.

## What shipped

### B1.1 — rows as data (`crates/app/src/ui/property.rs`)

```rust
struct Row     { id, label, body: fn(&mut Ui, &mut App), applies: fn(&App) -> bool }
struct Section { id, title, rows: &'static [Row], body: Option<fn>, applies: fn(&App) -> bool }
```

- A section with an EMPTY `rows` draws its `body` as ONE row — id = the
  section id, label = the section title. That is what keeps every panel lane
  B2 has not touched working unchanged, and it is why an unsplit section
  needs no `prop_hidden` migration (its id already IS its row id).
- Const constructors `sec` / `sec_when` / `sec_rows` replace the inline
  `Section { .. }` literals — Rust has no struct field defaults, so this is
  how `applies` gets one.
- `Section::applies` is used for real: the Figure tool's **Brush** and
  **Dynamics** sections now carry `applies = |app| !app.figure_mode.generates()`.
  With Stream line / Saturated line / either flash armed the generator places
  its own layer and never touches the brush, so those two editors were sitting
  in the palette doing nothing.
- `palette_rows(app)` is the ONE definition of "what the palette draws"
  (order applied, non-applying and hidden rows gone, empty sections dropped).
  `tool_property_body` calls it; `palette_row_ids` (test-only) flattens it, so
  a test cannot pass against a palette that does something else.

### B1.2 — order + visibility

- `app.prop_hidden: BTreeSet<String>` now holds **row** ids.
  `ui::property::hidden_from_line` decodes ui.txt's `prop_hidden=` AND runs
  `migrate_hidden`: an id naming a section this build has since SPLIT expands
  into that section's row ids and the section id is dropped. Unsplit ids and
  ids this build does not know are carried through untouched.
- `app.prop_order: BTreeMap<String, Vec<String>>`, keyed by CONTEXT id
  (`prop_context`): the tool's `{:?}` name, or `obj.picklayer` / `obj.text` /
  `obj.balloon` / `obj.gen` / `obj.frame` / `obj` under the Operation tool.
  The value is ONE flat list — sections and their rows interleaved. Sorting
  each group by position in that one list is correct because a section's rows
  always sit between it and the next section, and it makes the persisted form
  a single array instead of a nested structure.
- Degradation, both directions: an id in the stored list this build no longer
  has is ignored (nothing asks for it); an id this build has that the list
  lacks sorts to `usize::MAX` and, because the sort is STABLE, appends at the
  end in default order. Never silently disappears.
- Persisted as the one JSON line `prop_order=` in ui.txt (`gradients=`
  pattern): `layout.rs` gains the field, `note_prop_order`, the `to_body` line
  and the `apply_kv` arm; `app.rs` loads it in `App::new` and writes it in
  `sync_dock_layout`. An EMPTY map writes an EMPTY line, not `{}` — writing
  `{}` would differ from the default on every start, mark the layout dirty and
  rewrite ui.txt for a user who never reordered anything.
- Workspaces carry it: new field index **9**, on the END, beside
  `prop_hidden` at 5. Registering writes it, applying reads it.

### B1.3 — the settings window (`ui/dialogs.rs::property_detail_window`)

Preferences-shaped, deliberately copying `prefs_window`'s exact geometry:

- title `Tool Property settings — <context>` (the palette header's own
  context string), `ui.set_width(540.0)`, `anchor(CENTER_CENTER, (0, -30))`,
  not resizable.
- search box on top; a non-empty query draws a FLAT list of every row whose
  label matches, across all sections, with the section name above each. No
  ▲ ▼ in that mode — "up" has no meaning in a list that is not the order
  anything draws in.
- rail = 110 px column of section titles (click selects); a `▲ ▼ group` pair
  under the list moves the SELECTED section. Body = that section's rows: eye
  toggle, ▲ ▼ (only when the section has more than one row), the row's name,
  then the row's own live widget underneath.
- **the divider is PAINTED** (`allocate_exact_size(vec2(6, 0))` then
  `painter().vline` over the row's rect), not `ui.separator()` — the
  prefs-window feedback loop that grew the window ~50 pt a frame until it
  walked off the desktop. There is now a test that pins this.
- a row whose `applies` is false is drawn with `add_enabled_ui(false, ..)`
  (greyed, and a disabled widget cannot fire a side effect) plus the note
  `not for this sub tool`.
- footer: **Show all** (clears hidden for every row of THIS context),
  **Default order** (removes this context's `prop_order` entry), **Close**,
  and one plain-words line explaining the eye. The old "uncheck a category to
  hide it from the palette" line is gone.
- the palette header wrench still toggles it, unchanged.

### B1.4 — `text.rs` converted to rows

Sections and rows, in the new DEFAULT order:

| section | rows |
|---|---|
| `text.workstyle` Text style | (unsplit — one control) |
| `text.font` Font | `text.font.font`, `text.font.size` |
| `text.dir` Direction | `text.dir.vertical`, `text.dir.auto_tcy` |
| `text.align` Align | `text.align.rows`, `text.align.in_frame` |
| `text.spacing` Spacing | `text.spacing.line_mode`, `text.spacing.line`, `text.spacing.letter` |
| `text.style` Style | `text.style.marks`, `text.style.color` |
| `text.edge` Edge | (unsplit) |
| `text.ruby` Furigana | `text.ruby.reading`, `.size`, `.gap`, `.adjust`, `.along` |
| `text.guide` Guide | (unsplit) |

Was: Workstyle, Font, Direction, **Style, Furigana**, Align, Spacing, Edge,
Guide. Direction now sits directly above Align, which is the owner's example
("the horizontal/vertical ordering and centered/left/right aligned should be
next to each other"). **Char space moved from Font into Spacing** — it is the
same question as line spacing, and Font is now exactly "which face, how big".
The Text tool and the Operation tool's text context share ONE list
(`TEXT_SECTIONS`), so they cannot drift.

`text_state(app)` is recomputed per row, as ruled — three field reads off the
selected item.

## Tests — `cargo test -p mn-app <name>`, one filter at a time

| test | result |
|---|---|
| `hidden_section_id_migrates_to_its_rows` | ok |
| `prop_order_round_trips_through_ui_txt` | ok |
| `palette_draws_rows_in_stored_order` | ok |
| `default_text_order_puts_direction_before_align` | ok |
| `non_applying_sections_are_skipped` | ok |
| `the_settings_window_settles_on_screen` (extra, see below) | ok |
| existing: `workspace_entries_migrate_from_the_old_six_field_shape` | ok |
| existing: `workspaces_register_apply_reload_delete` | ok |
| existing filter `prop` (14 tests incl. the 5 new) | 14 passed |
| existing filter `ui_txt` (3) / `layout::` (14) | all passed |

`cargo check --workspace --all-targets` — **zero warnings**.
No app window was ever launched; every test is headless.

## Choices worth naming

- **Eye icon: `Icon::Eye` / `Icon::EyeOff` exist** (`ui/icons.rs` 53/54,
  113/114) and are what the window uses — not a checkbox. Lit = showing.
- **The ▲ ▼ arrows are text buttons.** There is no up/down arrow in
  `icons.rs`, and `icons.rs` is not in this lane's owned files. The plan
  specifies "▲ ▼" literally, so that is what shipped; adding a real
  `Icon::ArrowUp`/`ArrowDown` is a one-line follow-up for B2 if wanted.

## Deviations, and why

1. **`text.edge` and `text.workstyle` / `text.guide` were left UNSPLIT.**
   The plan lists Edge with a row named `edge`. A section whose single row is
   its whole body is byte-for-byte the "empty `rows`" case, so splitting it
   would have cost a migration entry and bought nothing. Their ids are
   unchanged, which also means an artist who hid Edge before this lane keeps
   it hidden.
2. **`crates/app/src/app/tests.rs:4445` changed from `len() == 9` to `== 10`.**
   The workspace entry grew by one field (`prop_order` at index 9) — the
   number IS the point of that test, so it had to move. Nothing else in that
   file was touched.
3. **`ui.rs`: `mod property` → `pub(crate) mod property`.** `App::new`,
   `sync_dock_layout` and `workspaces.rs` all need
   `ui::property::{hidden_from_line, order_from_json, order_to_json}`. Same
   treatment `shortcut_tab` already has, and no item inside it was widened.
4. **`workspace_apply` now writes the LIVE `app.prop_hidden` / `app.prop_order`,
   not just `layout.*` — a small bug fix inside an owned file.**
   `sync_dock_layout` copies `app.prop_hidden` into `layout.prop_hidden` at the
   end of every frame, so the old apply (which only set the layout copy) was
   undone one frame later: a workspace's Tool Property visibility never
   actually arrived. Present since UI-060. Adding `prop_order` the same broken
   way would have shipped a second dead feature.
5. **One extra test, `the_settings_window_settles_on_screen`.** Not in the
   brief's list. It builds a real headless frame for every section of the Text
   context and of Figure-with-Stream-armed and asserts the window stops
   moving and stays on screen. The prefs feedback loop is explicitly called
   out in the plan as having shipped once; a window shaped like that one
   should carry the test that would have caught it. (It settles on frame 4,
   not 3 — the anchor plus a fractional content height. Size is identical
   from frame 3 on, so there is no growth.)

## Open questions for Fable

1. **`prop_detail_sec` is a plain index into the current context's section
   list.** Switching tools while the window is open silently lands you on a
   different group (it is clamped, never out of range). A per-context memory
   would be nicer; it did not seem worth a field.
2. **Nothing tests which BUTTON is wired to which call** — `ui::dialogs` is
   private to `ui`, same limitation lane A3 wrote down. The effect of every
   control is covered (`move_entry`, `prop_hidden`, `palette_rows`); the
   widget wiring is eye-test only.
3. **`sec_text_line_mode` no longer early-returns** the way the old combined
   Spacing body did, so on the frame you switch Auto→% the value bar under it
   can draw one frame of the previous value when a text item is selected
   (`apply_text_prop` goes through the command pump). Invisible in practice;
   say the word if you want the row to skip a frame after a mode change.
4. **`docs/manual/drawing.html`** got the paragraph, under a new heading
   "Choosing what Tool Property shows", just above "Locking a sub tool" —
   that is where the wrench and the brush `Sub Tool Detail` window are already
   explained, and the new text says plainly that the two are different windows.
