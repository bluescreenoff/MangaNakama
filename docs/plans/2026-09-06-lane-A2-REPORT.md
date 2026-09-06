# Lane A2 report — wire the app to the core presets + knobs in both panels

## done / next

**DONE — the whole lane.**

- [x] `cmd/tools.rs`: `pub use mn_core::genlines::LineOpts as FigureLineOpts`, the app copy and
      its seven `*_dpi` constructors deleted; new `FigureMode::line_kind` / `of_line_kind` and
      `arm_line_preset`
- [x] `app.rs`: the two `figure_*` fields take the core presets at 600 dpi
- [x] `canvas_input.rs::finish_figure_lines` is now the guard + the panel lookup + `opts.place(...)`;
      the spec literal is gone
- [x] `ui/subtool.rs`: the rows are `builtin_presets()`, drawn verbatim under the two captions
- [x] `ui/property/frames_balloons.rs`: `sec_figure` reshaped, new `sec_figure_wobble`
- [x] `ui/property/frames_balloons.rs`: `sec_obj_genlines` / `sec_obj_genlines_density` gained
      every new knob, and radial sets gained Bundle / Bundle gap / Bundle variety
- [x] `ui/property.rs`: one new `figure.wobble` section entry
- [x] tests: 3 new, all green; every existing test in the two owned files green
- [x] manual: `docs/manual/comic.html` ▸ Effect lines
- [x] gates: `cargo check --workspace --all-targets` **0 warnings**; `cargo test -p mn-core genlines`
      **38 passed**; core untouched (`git status crates/core/` empty)

**NEXT (Fable's calls, not mine)**
1. Review + commit. Nothing committed, nothing `git add`ed.
2. Three deviations and four open questions below — one of them (the tests.rs one-liner) is an
   edit outside my file list.
3. Then Lane A3 (user sub tools), then B1.

---

## What changed, per file

### `crates/app/src/cmd/tools.rs` (1601 → 1436 lines)

- The 170-line `FigureLineOpts` struct + `impl` (7 `*_dpi` constructors, `from_mm`, `same_as`)
  **deleted**. In its place one line:
  `pub use mn_core::genlines::{LineKind, LineOpts as FigureLineOpts, builtin_presets};`
  plus the doc block explaining where it went and why the alias survives.
- `FigureMode::line_kind() -> Option<LineKind>` and `FigureMode::of_line_kind(LineKind)` — the
  two-way map between the palette's eleven-member enum and core's four-member one.
- **NEW `pub fn arm_line_preset(app, kind, opts)`** — what a sub tool row's click does. Pulled out
  of the click closure because the two things that are easy to get wrong there (the seed must
  survive the pick; the three radial kinds share one holder) are untestable inside a closure, and
  the new `every_builtin_preset_row_arms_its_kind` test now drives exactly the code the row runs.
- `LinePreset` is deliberately NOT re-exported: nothing in the app names the type yet and an unused
  import is a warning. Lane A3 adds it back the moment `figure_presets` exists.

### `crates/app/src/app.rs` (2 lines + a comment)

`figure_stream: FigureLineOpts::stream(600)`, `figure_focus: FigureLineOpts::focus(600)`. 600 is
`tone_dpi()`'s own fallback and is what `App::new` can know — a sub tool row re-prices at the page's
dpi the moment one is picked, which is the behaviour that already existed.

### `crates/app/src/app/canvas_input.rs` (`finish_figure_lines`: 122 → 54 lines)

Everything geometric is `opts.place(kind, a, b, bounds, opts.seed)`. What is left is what genuinely
belongs to the app: the too-short-drag guidance, `panel_at` → `bounds`, and the seed bump. Lane A1's
six-line `..Default::default()` patch went with the literal it was patching.

### `crates/app/src/ui/subtool.rs` (the `Tool::Figure` arm, 79 → 60 lines)

One loop over `builtin_presets()`. The caption is emitted when the kind's group changes (Stream vs
everything else), so the list's own order IS the palette's order and the flashes stay in the 集中線
group. Icon and hover text per kind. Highlight is unchanged (`same_as` against the held opts).
Arming is `crate::cmd::arm_line_preset`.

### `crates/app/src/ui/property/frames_balloons.rs` (1096 → 1540 lines)

- New private helpers `line_knob` (one labelled `DragValue` row with mandatory hover text) and
  `tool_ray_count` (asks a throwaway `GenLinesSpec` for the bundle walk's own ray count instead of
  printing `360 / gap`, which over-counted by nearly half once bundling existed).
- `sec_figure` reshaped — see the row order below. Width is millimetres now, not pixels.
- **NEW `sec_figure_wobble`** — the eight wobbles. The old single "Jitter" row is gone.
- `sec_obj_genlines` gained Accents / Accent width / Entry / Needle / Sweep / Start from the line
  (+ Start wobble), in the same order as the tool panel.
- `sec_obj_genlines_density` gained Bundle / Bundle gap **for radial sets too**, Bundle variety,
  Long bias, Outer length, Core stagger and Angle wobble.

### `crates/app/src/ui/property.rs` (one entry)

`Section { id: "figure.wobble", title: "Wobble", body: sec_figure_wobble }`.

### `crates/app/src/app/tests.rs` — ONE ASSERTION, OUTSIDE MY LIST. See Deviations.

### `docs/manual/comic.html` (+100 lines, the Effect lines section)

An updated lead-in quirk, a "Ten sub tool rows, in two groups" quirk that names all ten and says
right-click presets are Lane A3's work and not in yet, then `<h3>What each knob does</h3>` with one
plain-words row per knob in two tables (Figure, Wobble), then a quirk explaining why the flashes
show fewer rows. No new CSS — `<h3>` is already used unstyled in `keys.html`.

---

## The knob row order that shipped

**Tool Property ▸ Figure** (`figure.opts`, while a line sub tool is armed)

| # | row | shown for |
|---|---|---|
| 1 | Gap (° radial / mm stream) — or **Spikes** / **Lines** when the preset is count-driven | all |
| 2 | Bundle (size + hole ×) | non-flash, and only when a gap exists |
| 3 | Width (mm) / **Spike width** (mm) | all |
| 4 | Accents (fraction + up-to ×) | non-flash |
| 5 | Taper | non-flash |
| 6 | Entry | non-flash |
| 7 | Needle | non-flash |
| 8 | Hollow centre | radial (incl. flashes) |
| 9 | Sweep (0 = full circle) | radial, non-flash |
| 10 | Start: scatter / from the line (+ start wobble when anchored) | stream |
| 11 | Fan toward a point (on/off + how far) | stream |
| — | "each drag places its own layer" | all |

**Tool Property ▸ Wobble** (`figure.wobble`, new)

Position · Length · Long bias · Outer length (radial) · Width · Angle (stream) · Core stagger
(radial) · Bundle variety (only when bundling is actually in play).
Flash kinds see exactly: Position · Length · Core stagger.

**Object tool ▸ Effect lines** (`obj.gen`, a placed set selected)

kind picker (radial) · colour · Width/Spike width · Accents · Accent width · Taper · Entry · Needle ·
[radial: Hollow centre · Reach · Sweep] / [stream: Angle · Shortest · Longest · Start from the line ·
Start wobble · Fan toward a point · Point] · Reroll.

**Object tool ▸ Density** (`obj.gen.density`)

Space by angle / Space evenly · Gap or Lines · Bundle · Bundle gap · Bundle variety · Position ·
Length · Long bias · Outer length (radial) · Width · Core stagger (radial) · Angle wobble (stream).

---

## Tests

```
cargo check --workspace --all-targets      Finished. 0 warnings, 0 errors.
cargo test -p mn-core genlines             ok. 38 passed; 0 failed  (core untouched)
cargo test -p mn-app figure_stage_tests    ok. 15 passed; 0 failed  (13 old + 2 new)
cargo test -p mn-app gen_lines_object_tests ok. 4 passed; 0 failed  (3 old + 1 new)
cargo test -p mn-app figure_flash_drags_place_urchin_and_solid_layers   ok. 1 passed
cargo test -p mn-app figure_line_drags_place_fresh_effect_layers        ok. 1 passed
cargo test -p mn-app figure_lines_cross_the_panel_and_live_inside_it    ok. 1 passed
```

The last three are `app/tests.rs` placement tests that the change could plausibly have broken (one
of them needed the one-line edit below). Run one filter at a time, per the house rule.

**New:**

- `figure_drag_places_the_core_spec` — for all 10 shipped presets × 2 drags, the `GenLinesPlace`
  pushed by a real `finish_figure_drag` equals `LineOpts::place(...)` for the same inputs, **whole
  spec compared field for field**, not a spot check. Plus: the placed spec carries non-zero
  `accent_frac` / `needle` / `len_skew` on the line kinds (that is the fail-before-fix — the old
  struct literal wrote zeros there whatever the row said) and the seed bumped.
- `every_builtin_preset_row_arms_its_kind` — drives `arm_line_preset` for every preset: the mode
  matches the kind, the knobs land in the holder that generator reads, the OTHER holder is
  untouched, the reroll seed survives, and **exactly one row would highlight** afterwards (that last
  one is the real content — two presets whose numbers collapsed together would light two rows). Also
  asserts the two kind groups are contiguous, which is what makes two captions cover the list.
- `object_panel_edits_radial_bundle` — places a focus set, applies a clean even 3° spec through
  `GenLinesApplyTo`, measures the ray pitch on a ring (core's own `ray_gaps` idea, re-derived here
  against a real layer's pixels) and asserts one even pitch; then applies `group 4, group_gap 3` and
  asserts tight 3° runs with 9° holes; then undoes and asserts the even pitch is back. One undo
  press, as before.

---

## Deviations

1. **I edited `crates/app/src/app/tests.rs` — one assertion.** `figure_flash_drags_place_urchin_and_solid_layers`
   pinned `app.figure_stream.taper == 0.5`; the core preset the gauntlet tuned is 0.9, so that test
   went red the moment the default came from core. I changed it to read the number from the preset
   (`FigureLineOpts::stream(600).taper`) plus a `> 0.0` assert, so a future tuning round moves one
   number and not two, and left a comment saying why. `tests.rs` is not on my list; a knowingly red
   test seemed worse than one line. **Revert it and the test fails — it is not optional, only the
   FORM of the fix is.**

2. **`sec_figure_wobble` on a non-generator sub tool draws one weak line, not nothing.** The section
   registry draws caption-then-body unconditionally, so an empty body would show a bare "Wobble"
   caption over nothing whenever the Figure tool is on Rectangle. It says "only the effect-line sub
   tools wobble — a drawn figure is ruled" instead. B1's `Row::applies` is the proper fix and it
   will delete this line.

3. **The Wobble section sits after Figure and BEFORE Guide, not before Dynamics.** The brief said
   "after Figure and before Dynamics", but the shipped order in `property.rs` is Brush → Dynamics →
   Figure → Guide, so Dynamics is already above Figure. Putting Wobble between Figure and Dynamics
   would mean MOVING Dynamics, which is a second change to a file I was told to touch once. Wobble
   went directly after Figure. **Say the word and I will reorder Figure's four sections properly —
   it is a one-line move.**

4. *(not a deviation, a note)* **The legacy `jitter` is handled differently in the two panels, on
   purpose.** On the TOOL side the Position wobble writes `jitter` alongside `jit_gap`, as briefed —
   `place` copies `jitter` straight through, so that keeps the saved-file fallback honest. On the
   OBJECT side I did **not** do that, because it is a silent-data trap there: an old set carries one
   `jitter` and three zeros, and the renderer falls back to it for each of the three. Writing
   `jitter := jit_gap` when the owner moves Position alone would ALSO have moved the length and
   width wobble, which were reading the fallback Position just overwrote. Instead the density
   section states the three splits from `jitter` on the draft the moment it draws them. That writes
   the values the renderer was already using, so it changes no pixel, and it only reaches the layer
   if the owner commits some edit anyway. After it, every bar shows what the renderer uses and
   editing one moves exactly one.

---

## Open questions for Fable

1. **`every_builtin_preset_row_arms_its_kind` does not click a real widget.** `ui/subtool.rs`
   declares `mod subtool;` privately inside `ui.rs`, and `sub_tool_list` is `pub(super)`, so nothing
   under `crate::app` can call it — driving the actual row would need `crates/app/src/ui.rs`, which
   is outside my list, so **I stopped rather than editing it**. The test drives
   `arm_line_preset`, which is the whole body of the row's click closure, so what is NOT covered is
   exactly: the caption emission, the icon choice, and that the click handler is wired to the loop
   variable. The repo has a working headless-egui pattern (`app/surface_layers_tests.rs:173`,
   `ctx.run_ui`) if you want the real thing — it needs one word (`pub(crate) mod subtool;`) and one
   more (`pub(crate) fn mode_sub_tools`). Your call.

2. **The Figure section order.** Deviation 3. Related: with Wobble added, the Figure tool's palette
   is Brush · Dynamics · Figure · Wobble · Guide, and Brush/Dynamics do nothing at all while a line
   sub tool is armed (`applies` = `!generates()`, per the B1 plan). Today they still draw. That is
   B1's job, but it means the palette currently opens on two dead sections when the owner picks
   Saturated line — worth knowing before he eye-tests this.

3. **`frames_balloons.rs` is 1540 lines** (was 1096), essentially all logic, and Lane B2 is going to
   rewrite every `sec_*` in it into row lists. It is over the house tripwire. I did NOT split it:
   splitting now would make your review diff for this lane much harder to read, and B2 is going to
   move the same code again. Flagging it so the decision is yours and on the record.

4. **A flash's `jitter`, `jit_gap` and `jit_len` are all non-zero in the shipped presets**, so the
   Wobble section shows Position and Length with real numbers on a flash and they work. But
   `render_urchin` never reads `jit_width`, and the flash presets do not set it — so if a future row
   sets it, nothing happens. Not a bug today (the panel hides Width wobble on flashes); noting it
   because the *object* panel hides it by `flash`, not by "the renderer reads it", and those two
   could drift.

5. **Not verified by eye.** The plan's A2 acceptance is "pick each preset row, drag, see a set that
   matches the harness PNG for that preset". I never launched the window (house rule), and the tests
   prove the SPEC matches `place` rather than that the raster matches `target/effect-lines/*.png`.
   If you want that closed before A3, the cheap version is a headless test that renders a placed
   layer and compares its ink fingerprint against `LineOpts::place(...).render(...)` — say so and I
   will add it.
