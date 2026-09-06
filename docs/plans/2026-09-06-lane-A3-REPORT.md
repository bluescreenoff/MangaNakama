# Lane A3 report — user sub tools for effect lines

## done / next

**DONE — the whole lane, plus the dpi follow-up Fable ruled on (open question 3).**

### Follow-up round (dpi recorded before any rows exist on disk)

- [x] `UserLinePreset` gained `dpi: u32`, `#[serde(default = "default_preset_dpi")]` ⇒ 600, so a
      row written in the hour before this field existed still loads
- [x] `UserLinePreset::repriced(page_dpi)` — scales EXACTLY the three canvas-pixel fields
      (`width`, `gap_px`, `start_back`, each verified against its field doc in
      `crates/core/src/genlines/presets.rs` and against `place`); degrees, fractions, multiples
      and counts are left alone. Same dpi, or a 0 on either side, returns the opts untouched
- [x] Add / Duplicate / Update stamp `app.tone_dpi()`; Update moves the row's dpi with the knobs
- [x] `ui/subtool.rs` hands `repriced(dpi)` to the row, so arming, the `same_as` highlight and a
      Duplicate of a Mine row all agree on one set of numbers
- [x] new test `mine_rows_reprice_to_the_page_dpi`; the five existing ones still green
- [x] `docs/manual/comic.html`: the `why` tail rewritten — it said a Mine row keeps the exact
      widths you saved, which this round made untrue
- [x] gates re-run: `cargo check --workspace --all-targets` **0 warnings**;
      `cargo test -p mn-app figure_presets` **6 passed**; `figure_stage_tests` **15 passed**

Fable's other three rulings are recorded and needed no code: (1) two rows lit after a Duplicate —
accepted, self-resolves; (3) the `dispatch` hop — accepted as is; (4) `"<name> tuned"` — kept.

### The lane itself

- [x] `cmd/tools.rs`: `UserLinePreset`, `user_presets_from_json` / `_to_json`, `unique_preset_name`,
      `held_line_opts`, and `run_figure_preset` (the four commands' bodies)
- [x] `cmd.rs`: the four `FigurePreset*` variants + the hop in `dispatch` that runs them before the
      history bracket
- [x] `app.rs`: `figure_presets` + `figure_preset_rename` fields, seeded from the layout line
- [x] `app/layout.rs`: the `figure_presets=` line — field, default, `note_figure_presets`,
      `to_body`, `apply_kv`
- [x] `ui/subtool.rs`: the two groups walked explicitly, a `Mine` caption per group, new
      `line_preset_row` + `line_preset_menu`
- [x] `app/figure_presets_tests.rs` (5 tests) + its `mod` line in `app.rs`
- [x] `docs/manual/comic.html`: A2's "not in yet" line replaced with the real paragraph
- [x] gates: `cargo check --workspace --all-targets` **0 warnings**; the 5 new tests green;
      `figure_stage_tests` **15 passed**; `layout::tests` **14 passed**; `cmd_tests` **1 passed**

**NEXT (Fable's calls, not mine)**
1. Review + commit. Nothing committed, nothing `git add`ed. The diff is exactly the six owned
   files plus the two new ones.
2. One deviation and three open questions below. The first open question (two rows lighting at
   once right after a Duplicate) is a real UX wrinkle the brief's `same_as` rule produces on
   purpose — worth a ruling before the owner eye-tests.
3. Then B1.

**Pre-flight fact checked (the thing I was told to stop on):** `mn_core::genlines::LineOpts` derives
`Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize` with `#[serde(default)]` at the
container, and `LineKind` derives `Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize`.
No core edit was needed; `git status crates/core/` is empty.

---

## What changed, per file

### `crates/app/src/cmd/tools.rs` (+153)

Five new items, all after `arm_line_preset`:

- **`pub fn held_line_opts(app, kind) -> FigureLineOpts`** — the read mirror of `arm_line_preset`'s
  write: which of the two holders (`figure_stream` / `figure_focus`) this kind's generator actually
  reads. "Save current settings" and "Update from current" are both one call to it, and the row
  highlight in `subtool.rs` now goes through it too instead of open-coding the same `if radial`.
- **`pub struct UserLinePreset { name: String, kind: LineKind, opts: LineOpts }`**, `Serialize` +
  `Deserialize`. Documented next to it: a shipped `LinePreset` is a `&'static str` + a
  `fn(dpi) -> LineOpts` because a millimetre is not a pixel until a page says so; a user row is
  born on a page whose dpi is known, so it stores the PRICED numbers, not the recipe. See open
  question 3.
- **`user_presets_from_json` / `user_presets_to_json`** — tolerant in, compact out.
- **`unique_preset_name(list, base) -> Option<String>`** — `base` free ⇒ `base`, else `base 2`,
  `base 3`, … The plan's two Duplicate spellings fall out of this one rule: a BUILTIN duplicate asks
  for `"<name> copy"` (free ⇒ stays), a MINE duplicate asks for its own name (taken by definition
  ⇒ becomes `"<name> 2"`). Empty after trimming ⇒ `None`, which every caller reads as "do nothing".
- **`run_figure_preset(app, cmd)`** — the four arms plus a private `note_figure_presets(app)` that
  every one of them calls, so a fifth verb cannot forget the ui.txt write and leave a row that
  vanishes at the next start.

Rows are addressed **by name, never by index**: a command sits in `app.cmds` for a frame before it
runs, and an index would be pointed at whatever moved into that slot meanwhile.

### `crates/app/src/cmd.rs` (+43)

The four variants (`FigurePresetAdd { name, kind, opts }`, `FigurePresetRename { from, to }`,
`FigurePresetUpdate { name, opts }`, `FigurePresetDelete(String)`), and in `dispatch` a
`matches!` guard that hands all four to `tools::run_figure_preset` and returns — **before**
`history::run` and before `run_cmd_tail`.

Why there and not as a new link in the module chain: `cmd/tools.rs` is a types file with no `run`,
and adding one to the chain means editing `misc.rs`'s catch-all, which is outside my file list. The
early hop is also the honest description of these commands — they cannot touch a layer, a page or a
clip, so there is no undo step to record and no tail work to run. And the thing that WOULD be wrong
is an undo press giving back a row you renamed instead of the mark you just drew. The brush presets
they copy are not undo steps either.

### `crates/app/src/app.rs` (+22)

`pub figure_presets: Vec<crate::cmd::UserLinePreset>` (seeded from `layout.figure_presets`),
`pub figure_preset_rename: Option<(String, String)>` (row name, typed name — the
`brush_rename_edit` shape), and the `mod figure_presets_tests;` line beside the other test modules.

### `crates/app/src/app/layout.rs` (+29)

The `gradients` pattern, five spots: the field, the `Default`, `note_figure_presets`,
`to_body` (a new `figure_presets={}` line at the end, `.replace('\n', "")` like `gradients`), and
`apply_kv` (`"figure_presets" if !line.contains('\n')`). The line is kept as WRITTEN and decoded
where it is used — this module knows nothing about `LineOpts` and should not start now, so the
"junk ⇒ empty list" rule lives in `cmd/tools.rs`.

### `crates/app/src/ui/subtool.rs` (+241 / −43, the `Tool::Figure` arm)

A2 walked `builtin_presets()` once and emitted a caption whenever the kind's group changed. That
cannot answer "have I passed the last builtin of this group yet", which is exactly where the Mine
rows go, so the arm now walks the **two groups explicitly** (`for stream_group in [true, false]`)
and filters the shipped list inside each. Same order on screen, same two captions.

Two new fns:

- **`line_preset_row(ui, app, name, kind, opts, mine)`** — one row, shipped or the artist's own.
  Identical draw, identical arming (`arm_line_preset`), identical highlight (`same_as`), because a
  row you made is a sub tool and not a bookmark. It carries the body A2 had inline.
- **`line_preset_menu(resp, app, name, kind, opts, mine, armed)`** — `organise_menu`'s shape:
  inline rename box (Mine only; Enter or the Rename button applies, Esc drops), a separator, then
  `Save current settings as sub tool` (armed rows only), `Update from current` (Mine + armed),
  `Duplicate` (every row), `Delete` (Mine only). Every item has hover text.

The `Mine` caption is a literal, matching `brush_sub_tools`'s own `"Mine"` — the
`crate::subtools::group` constants exist so a SHORTCUT can name a tab, and this is not a tab.

### `docs/manual/comic.html` (+21 / −4)

A2's `<span class="why">…is the next piece of work and is not in yet</span>` is gone. In its place a
new "Make your own rows" quirk: the five verbs in plain words, what Enter and Esc do, and a `why`
tail covering the three things that will otherwise look like bugs — shipped rows cannot be renamed
or deleted (they come back with the next version), Delete does not change the settings in hand, and
a Mine row keeps the exact widths you saved while a shipped row restates its millimetres for
whatever page you have open.

---

## Tests

```
cargo check --workspace --all-targets   Finished. 0 warnings, 0 errors.
cargo test -p mn-app figure_presets     ok. 6 passed; 0 failed
cargo test -p mn-app figure_stage_tests ok. 15 passed; 0 failed   (A2's, unchanged)
cargo test -p mn-app layout::           ok. 14 passed; 0 failed   (to_body/apply_kv changed)
cargo test -p mn-app cmd_tests          ok. 1 passed; 0 failed    (dispatch changed)
```

One filter at a time, `CARGO_INCREMENTAL=0`, `--jobs 2`, `RUST_TEST_THREADS=2`. No release build,
no app window.

The five new ones, and what each actually pins:

- **`user_preset_round_trips_through_ui_txt`** — the plan's acceptance minus the mouse. Duplicate
  `Saturated line` → Rename to "Ref 08 wedges" → arm it and tune `accent_frac` to 0.77 → Update from
  current → assert the rename reached `layout.figure_presets` → `to_body` → assert exactly ONE
  `figure_presets=` line and that it starts `[{` → `from_body` → `user_presets_from_json` → the
  whole row is equal, kind and every knob → and it then ARMS through `arm_line_preset` and would
  highlight. That is the real save→load seam the layout tests use, so the run never touches the
  ui.txt beside the test exe.
- **`duplicate_of_a_builtin_lands_in_mine`** — `Dark burst copy`, then `copy 2`, `copy 3`, `copy 4`;
  the shipped list untouched; no two rows share a name; an all-space Add is refused; an all-space
  Rename leaves the working row's name alone.
- **`save_current_captures_the_tuned_knobs`** — arm `Stream line`, move three knobs across both
  panel halves (`gap_px`, `accent_mul`, `jit_len`), save: the row holds the TUNED struct field for
  field, not the shipped numbers; saving does not disturb what is in hand; Update keeps the ROW's
  reroll seed rather than adopting the tool's; and all three verbs aimed at a name nobody has are
  no-ops rather than panics or new rows.
- **`deleting_the_armed_preset_keeps_the_knobs`** — arm a Mine flash row, delete it: the row is
  gone, `figure_mode`, `figure_focus` AND `figure_stream` are byte-identical, and the now-empty list
  persists as `[]` (so the next start cannot resurrect it from a stale line).
- **`mine_rows_reprice_to_the_page_dpi`** (follow-up round) — duplicate `Saturated line` at 600,
  drop the page to 300: the width halves while `gap_deg`, `count`, `group`, `group_gap`, `taper`,
  `jit_len`, `accent_mul` and `sweep_deg` do not move. Then, for all TEN shipped presets at 2×, the
  whole repriced struct is compared to `{ width×2 (floored at 0.5 px), gap_px×2, start_back×2, ..o }`
  — which is the assertion that will fail the day someone adds a new pixel field to `LineOpts` and
  forgets `repriced`. Plus: same dpi is a no-op, a 0 on either side passes through, the row lights
  on the 300 dpi page against its REPRICED opts and not against its stored ones, a Duplicate here
  stores dpi 300 and round-trips back to the original width at 600, and Update moves the row's dpi
  with the knobs.
- **`garbage_figure_presets_line_loads_empty`** — seven junk forms (`""`, spaces, prose, `"[{"`,
  `"{}"`, a row missing `kind`, a `kind` this build does not have) all give an empty list; the rest
  of ui.txt survives a bad line (`left_w` / `right_w` still parse); a row with UNKNOWN extra fields
  from a newer build still loads and takes `LineOpts`' defaults for what it did not carry; and
  encode/decode round-trip a real list on one line.

Each test clears `app.figure_presets` first: `App::new` reads the ui.txt beside the test exe, which
the parallel runner shares, and a developer's own saved sub tools must not decide whether they pass.

---

## Deviations

1. **The commands run from `dispatch` rather than as a new link in the `cmd::*` `run` chain.** The
   chain is `history → … → misc`, and `misc`'s catch-all is an `unreachable!` that names any
   unclaimed variant. Joining it properly means editing `misc.rs`, which is outside my file list;
   the brief said the execution lives in `cmd.rs` + `cmd/tools.rs`, so it does — a `matches!` guard
   in `dispatch` that hands the four over and returns. It is documented there with the reasoning
   (no undo bracket, no document touch, no tail work). **If you would rather they rode the chain,
   it is `other => tools::run(app, other, cmd_tail)` in `misc.rs` plus a `run` wrapper here.**

2. *(a naming call, not a deviation)* **"Save current settings as sub tool" names its row
   `"<name> tuned"`,** not `"<name> copy"`. The brief did not say. The brush equivalent uses
   `"<name> copy"`, but Duplicate is right underneath it in the same menu and already produces
   exactly that, so two adjacent verbs would differ only by a silently appended `2`. Say the word
   and it is one string.

---

## Open questions for Fable

1. **Right after a Duplicate, TWO rows light up.** The highlight rule is the brief's (`same_as`
   against the held knobs), and a fresh duplicate is by definition the same set as its parent — so
   `Saturated line` and `Saturated line copy` are both lit until you retune one. It is honest (they
   ARE the same numbers) and it self-resolves the moment the copy is edited, which is the next thing
   anyone does. The alternative is an identity — `app.figure_row: Option<String>` set by the click —
   but that changes A2's shipped model ("a tweaked set highlights NO row"), so I did not invent it.
   **Your call; it is the one thing here the owner will see immediately.**

2. **The menu widget itself is still untested, same wall as A2's open question 1.**
   `ui/subtool.rs` is a private `mod` inside `ui.rs` and `sub_tool_list` is `pub(super)`, so nothing
   under `crate::app` can drive the real rows. The tests cover the whole *effect* of every menu item
   (they call the same `held_line_opts` and dispatch the same commands the closures do); what is NOT
   covered is which button is wired to which command, plus the caption placement. One word in
   `ui.rs` (`pub(crate) mod subtool;`) plus one on `mode_sub_tools` would let a `ctx.run_ui` test
   click the actual rows — the pattern exists at `app/surface_layers_tests.rs:173`.

3. ~~**A Mine row stores PRICED numbers; a shipped row stores a recipe.**~~ **CLOSED — Fable ruled
   "record the dpi now", and it is in.** `UserLinePreset` carries `dpi: u32` and `repriced(page_dpi)`
   scales `width`, `gap_px` and `start_back` by `page / stored`. Those three are the complete set of
   canvas-pixel fields in `LineOpts`; every other field is a degree, a fraction, a multiple or a
   count, and `place` treats none of them as pixels. The `mine_rows_reprice_to_the_page_dpi` test
   pins the whole struct for all ten shipped presets at 2×, which is what will catch a NEW pixel
   field being added to `LineOpts` later without being added to `repriced` — that is the one way
   this can silently rot.

4. **Not verified by eye.** No window was launched (house rule). What is proven is the state
   machine and the persistence; what is not is that the Mine caption lands where I think it does in
   the palette and that the context menu is not clipped by the Sub Tool list's `ScrollArea`. The
   cheap close is the `pub(crate) mod subtool` from question 2 plus a `--warp` screenshot.
