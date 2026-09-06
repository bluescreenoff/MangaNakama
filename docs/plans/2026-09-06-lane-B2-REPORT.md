# Lane B2 — the remaining Tool Property panels converted to rows

> **DONE / NEXT**
> - **DONE: the whole lane.** `frames_balloons.rs` (+ a 4-way file split),
>   `tone.rs`, `select.rs`, `gradient.rs`, `rulers.rs`, the section lists in
>   `property.rs`, and `prop_rows_tests.rs` extended.
>   **165 rows over 22 newly split sections** (`SPLIT_SECTIONS` now lists 28,
>   B1's six text ones included). The sections left unsplit are the guides
>   and the one-control ones, plus `live.fill` — see Deviations 5.
> - Gates: `cargo check --workspace --all-targets` **zero warnings** after
>   every file. Tests: `prop_rows` 8/8 (2 new), `figure_stage_tests` 15/15,
>   `gen_lines_object_tests` 4/4, `tone_tool` 2/2, `basics_qa_fill_tests`
>   16/16, `grad_free_tests` 10/10, `ruler_undo_tests` 7/7.
> - **NEXT: Fable reviews the diff and commits.** Nothing is left open in
>   this lane. Open questions at the bottom.

Lane B1 (`docs/plans/2026-09-06-lane-B1-REPORT.md`) built the row model and
converted `text.rs`. Every other panel still drew as ONE row per section.
This lane splits them: one `Row` per control, and the `if` that used to wrap
a control inside a section body becomes that row's `applies`, so the settings
window can grey it and say why. No behaviour change.

## Where the row arrays live

`property.rs` keeps only the section list — id, caption, and which array of
rows. The arrays (`ROWS_*`) sit beside the fns they call, in the panel's own
file. B1 put the text arrays in `property.rs`; doing the same for 165 more
rows would have taken that file past 1 400 lines for nothing. It is 932 now.

New in `property.rs`: `row_when(id, label, body, applies)`, the constructor
for a row that only applies to some sub tools; and every newly split section
is registered in `SPLIT_SECTIONS`, which is what makes an old `prop_hidden=`
line naming a SECTION expand into that section's row ids on load. **Section
ids are unchanged**, so an artist who hid a section keeps it hidden.

## The rows, per file

### `frames_balloons.rs` + the three files it shed — 96 rows

| section | id | rows |
|---|---|---|
| Frame | `frame.tool` | 8 |
| Colour (Balloon tool) | `balloon.ink` | 9 |
| Tail (Balloon tool) | `balloon.tail` | 3 |
| Balloon (Object) | `obj.balloon` | 6 |
| Colour (Object) | `obj.balloon.ink` | 8 |
| Tail (Object) | `obj.balloon.tail` | 3 |
| Frame border (Object) | `obj.frame` | 4 |
| Effect lines (Object) | `obj.gen` | 19 |
| Density (Object) | `obj.gen.density` | 13 |
| Figure | `figure.opts` | 15 |
| Wobble | `figure.wobble` | 8 |
| unsplit | `balloon.line`, `balloon.guide`, `frame.guide`, `figure.guide`, `obj.guide` | — |

### `tone.rs` — 10 rows

`tone.screen` 4 (density, frequency, angle, pattern) · `tone.region` 6
(tolerance, close gap, area scaling, refer, refer drafts, refer border) ·
`tone.guide` unsplit.

### `select.rs` — 35 rows

`select.opts` 5 · `obj.picklayer` 5 · `wand.opts` 10 · `fill.opts` 15 ·
`wand.guide` / `fill.guide` / `eyedrop.guide` / `pan.guide` unsplit.

`fill.opts` carries three families in one section, as it always did: the
Remove-dust sub tool's three rows (`applies` = the Dust sub tool), the flood
knobs (`applies` = a sub tool that actually runs a flood, i.e. not Lasso and
not Dust), and the live-layer switch (`applies` = the click bucket). Those
were `if … return` at the top of the old body.

### `gradient.rs` — 16 rows

`grad.info` 4 (sub tool mode, the ramp bar, the selected stop, live layer) ·
`grad.opts` 7 (edge, mixing, brightness, flip, dithering, start from centre,
mixing rate) · `grad.set` 5 · `grad.guide` unsplit.

### `rulers.rs` — 8 rows

`ruler.tool` 5 (hint, vanishing points, symmetry lines, re-count, ring
spacing — the last four `applies` per ruler kind) · `ruler.snap` 3 ·
`ruler.guide` unsplit.

## Order changes

**None.** Every row sits where its control sat in the body it came out of.
Nothing looked wrong enough to move, and the one thing that did (Direction
next to Align) was B1's job and is already done.

## The file split

`frames_balloons.rs` was 1 540 lines before the round and rows only add. It
is now four files, all pure moves of whole fns:

| file | lines | holds |
|---|---|---|
| `frames_balloons.rs` | 709 | the Frame tool, the Balloon tool's line/tail/guide, the selected balloon and panel, the Operation guide |
| `balloon_ink.rs` | 388 | the colour / opacity / screened-fill controls, written once and drawn in both ink panels |
| `effect_lines.rs` | 740 | the SELECTED 流線 / 集中線 set (`obj.gen`, `obj.gen.density`) |
| `figure_lines.rs` | 736 | the Figure tool's own knobs and the wobbles (`figure.opts`, `figure.wobble`) |

The brief asked for TWO files under ~900 lines each. Two could not hold it —
the pair would have been ~1 080 and ~1 470 — so the extra cuts follow seams
that were already there: the ink block is the one thing the Balloon tool and
the Operation tool literally share, and a placed set and the tool's defaults
are edited in different panels at different times. Every property file is now
under 900 lines except `pen.rs` (1 390), which this lane does not touch.

## How the shared per-section state is handled

The rule from the brief — recompute per row, each row commits itself.

- **A selected effect-line set** — `gen_row` recomputes the draft
  (`gen_draft`) and the page's px-per-mm for every row and commits through
  the same `gen_commit`. **The regen-on-release-only rule is intact**: a row
  that changes mid-drag parks the draft in `app.gen_edit`, and only the
  release edge pushes `GenLinesApplyTo`.
- **The density rows** go through `gen_density_row`, which additionally
  restates the legacy single `jitter` onto the three split wobbles before the
  row draws — exactly what the old combined body did at the top, and it has
  to stay per row or moving Position alone would drag Length and Width with
  it (they read the same fallback number).
- **The Figure tool's knobs** are plain app state, so `fig_row` only picks
  `figure_focus` or `figure_stream` by the armed sub tool.
- **The balloon ink** has two wrappers over one set of control fns:
  `ink_tool_row` (writes `app.balloon_ink`) and `ink_obj_row` (buffers
  through `app.ink_edit`, commits on the release edge, one undo step per
  drag). Unchanged behaviour, one row at a time.
- **The flood knobs** (`FillOpts`) are written once and wrapped by
  `wand_row` / `fill_row`, which copy the tool's options, let the row edit
  one field and push `SetWandOpts` / `SetFillOpts`. Same for
  `tone_row` (`SetToneOpts`), `dust_row` (`SetDustOpts`) and
  `grad_opts_row`.

## Tests

| filter | result |
|---|---|
| `prop_rows` | **8 passed** (6 existing + 2 new) |
| `figure_stage_tests` | 15 passed |
| `gen_lines_object_tests` | 4 passed |
| `tone_tool` | 2 passed |
| `basics_qa_fill_tests` | 16 passed |
| `grad_free_tests` | 10 passed |
| `ruler_undo_tests` | 7 passed |

`cargo check --workspace --all-targets` — **zero warnings**, run after every
file. No app window was ever launched; every test is headless. Nothing was
committed or `git add`ed.

### The two new tests

- **`every_context_has_unique_row_ids`** — walks all 18 tools, the Figure
  tool's five generating/inking sub tools, and each Operation-tool context
  (`obj.picklayer`, `obj.text`, `obj.balloon`, `obj.gen`, `obj.frame`), and
  asserts no id appears twice. It checks the FULL id list (`flat_order`), not
  the visible palette: a row that does not apply still owns its id, and that
  is the id space `prop_order` writes down. A duplicate would make two rows
  jump to one position and one eye toggle hide both.
- **`effect_line_rows_apply_per_kind`** — Stream armed: no `Sweep`, no
  `Hollow centre`, no radial wobbles; Focus armed: no `Start`, no `Fan`, no
  angular wobble; either flash: no stroke-profile rows at all (taper, entry,
  needle, accents, width wobble) but the hollow centre and core stagger stay;
  an inking sub tool: no wobbles, and its own Fill / Adjust-angle rows
  instead.

## Deviations, and why

1. **Four files instead of two** (see the split table above) — the ~900-line
   bar in the brief could not be met with two.
2. **`prop_rows_tests.rs:non_applying_sections_are_skipped`** asserted
   `ids.contains("figure.opts")`. `figure.opts` is a split section now, so it
   is no longer a row id; the assertion moved to `figure.opts.width`. It
   still says the same thing.
3. **`the_settings_window_settles_on_screen` grew from 2 contexts to 11.**
   Not in the brief, but this lane introduced the risk it now covers: the
   settings window draws rows that do NOT apply (greyed), so a row body that
   assumed the `if` around it would panic there and nowhere else. All 11
   contexts build and settle.
4. **The Wobble section's "only the effect-line sub tools wobble" line is
   gone.** Its own comment said "B1 will grey this out properly
   (`Row::applies`); until then say why it is empty" — that is now what
   happens: with an inking sub tool armed the whole section drops out of the
   palette, and the settings window greys its rows with "not for this sub
   tool".
5. **`live.fill` was left UNSPLIT** although it lives in `gradient.rs`. Two
   reasons, both about behaviour: it is the one section also drawn by the
   Layer Property window (`ui/layers/property.rs:194` calls `sec_live_fill`),
   and its undo-coalescing session (`AppCmd::ParamEditSession`) is opened and
   closed off ONE `changed` flag spanning the whole body. Splitting it would
   have changed how a live gradient/tone layer's slider drags land in the
   undo stack, which is not a mechanical edit. Its id is unchanged.
6. **`ramp_options` still exists as a whole block** for the same reason —
   `live.fill` draws it. It is now seven one-control fns plus a fn that calls
   them in order; the Gradient tool's Ramp section uses the seven.
7. **Three rows are a sentence, not a control**: `obj.balloon.tail.none`
   ("no tail yet…"), `obj.balloon.hint` (the anchor count) and
   `obj.frame.hint` (the panel count), plus `fill.opts.hint`,
   `ruler.tool.hint`, `select.opts.hint`, `obj.picklayer.hint`,
   `grad.info.mode` and `figure.opts.hint`. They were text inside the bodies;
   as rows they can be switched off, which is what an experienced user wants.
   Without `obj.balloon.tail.none` a tail-less bubble would show an empty
   section (dropped from the palette) and the explanation would vanish.

## Open questions for Fable

1. **`pen.rs` (1 390 lines) is untouched**, as instructed — the brush tools
   keep their own Sub Tool Detail system. It is now the only property file
   over 900 lines, and joining the row model is already on the plan's
   "Later / not this round" list.
2. **`live.fill` and the four "one control" sections stay unsplit.** If you
   want `live.fill` in the row model, it needs a decision about the param
   session first (one session per row, or a section-level session the rows
   report into) — that is a real design call, not a mechanical split.
3. **Nothing tests which BUTTON is wired to which call** — `ui::dialogs` is
   private to `ui`, the same limitation B1 wrote down. The effect of every
   control is covered; the widget wiring is eye-test only.
4. **Eye test worth doing** (nothing is blocked on it): arm each Figure sub
   tool and confirm the palette shows exactly the knobs that kind has; open
   the wrench on the Fill tool with Remove dust armed and confirm the flood
   rows are greyed rather than missing.
