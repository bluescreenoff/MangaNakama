# Plan 2026-09-06: effect lines that look like a printed page + a Tool Property settings box

> **STATE 2026-09-06: Lane A1 DONE (3afcd12). Gauntlet DONE, 4 critic rounds, final verdict PASS 10/10 live presets (flashes parked) — see docs/plans/2026-09-06-gauntlet-REPORT.md + critic-round1..4.md; shipped presets have MORE knobs than the A1 table (jit_len_out, group_jit, jit_angle, core_jit, start_back): presets.rs is the truth. Lanes A2 + A3 DONE. NEXT: B1 (row model + settings window + text), then B2.**
> Lanes run ONE AT A TIME in the order below. Each lane keeps a `docs/plans/2026-09-06-lane-<X>-REPORT.md` with a done / next box at the top, updated as it works.

Owner asks, 2026-09-06, his words:
- "manganakama effect lines in general need to be way way better, like even the default one needs to
  look way better compared to professional manga" and "effect lines needs huge work maybe have opus do
  some loops on this compare how real manga look".
- "i should be able to make a new subtool from default that does this by varying settings ... from
  right click duplicate subtool and rename or something".
- Effect lines "doesn't seem to have a grouping setting" (earlier the same day).
- "for the subtool settings ... there should probably be a button that opens the full settings box
  similar to how the preferences box works and in there you can checkbox/eye icon click for each
  setting on whether or not you want to show it in the subtool settings and you should be able to
  reorder them too but for all tools you should reorder the default ones to make sense e.g. for fonts
  the horizontal/vertical ordering and cetered/left/right aligned should be next to each other".

House rules for every lane (AGENTS.md + standing owner protocol):
- Save work to disk as you go; keep the done / next box in your report file current. Never hold a
  round's work only in conversation.
- NO full local `./build.sh --test`. Local gate = `cargo check --workspace --all-targets` with ZERO
  warnings + the targeted tests named per lane (`cargo test -p mn-core <name>` / `-p mn-app <name>`).
  CI is the full gate. `CARGO_INCREMENTAL=0`, jobs=2 pinned — leave them.
- Never launch the app window. Headless only. The render harness (Lane A1) writes PNGs; nothing pops.
- Do not commit. Fable reviews the diff per lane and commits.
- Owner's machine is RAM-starved and F: is ~5 GB free: `cargo check` + targeted tests only.
- The reference scans live in `docs/plans/refs/effect-lines/` — LOCAL ONLY, git-excluded
  (copyrighted pages). Never `git add` them, never copy them anywhere tracked.

---

## Part A — the look

### What is there today (facts, file:line)

- Generator: `crates/core/src/genlines.rs`. `render_focus` (~276) draws `count` rays from `r_out` to
  `r_in` through `segment` (~212), which is a constant-width capsule whose half-width ramps linearly
  to `hw·(1−taper)` at the `b` end. `render_speed` (~314) scatters or walks runs across the canvas
  normal; grouping (`group`/`group_gap`, まとまり) exists ONLY in that walk. `render_urchin` (~413)
  draws filled teeth. `GenLinesSpec` (~1218) is the saved, re-editable spec (`#[serde(default)]`
  on every post-v1 field; 0 = legacy meaning; `legacy_renders_are_bit_stable` ~645 pins it).
- Tool knobs: `crates/app/src/cmd/tools.rs::FigureLineOpts` (334) + the mm/degree preset
  constructors (358–480: `stream_dpi`, `dense_stream_dpi`, `sparse_stream_dpi`, `focus_dpi`,
  `dense_focus_dpi`, `dark_burst_dpi`, `flash_dpi`). `same_as` (489) is how a sub tool row knows it
  is the armed preset.
- Sub tool rows: `crates/app/src/ui/subtool.rs` `Tool::Figure` arm (~440–560), hard-coded preset
  lists. No duplicate / rename / delete on them (brushes have `organise_menu`, ~745).
- Placement: `crates/app/src/app/canvas_input.rs::finish_figure_lines` (~2358) turns the drag +
  `FigureLineOpts` into a `GenLinesSpec` and pushes `AppCmd::GenLinesPlace`. The rays' reach is the
  panel's farthest corner; a stream's runs cross the whole panel (`len_min == len_max == cross`).
- Effect lines ARE editable objects already: Object tool handles (`gen_handle_points` ~135,
  `GenLinesDrag`), `app.gen_sel` / `gen_edit`, `AppCmd::GenLinesApplyTo`, and the Tool Property
  sections `sec_obj_genlines` / `sec_obj_genlines_density` in
  `crates/app/src/ui/property/frames_balloons.rs` (~575–920). The earlier stub's "no post-placement
  object" was wrong.
- Tool Property while placing: `sec_figure` in the same file (~920–1050).
- Materials bank thumbnails come from `crates/core/examples/gen_materials.rs` (renders specs to PNG
  with the `image` crate — the pattern for the harness below).

### Why the sets look generated, against the reference pages

Read alongside the scans in `docs/plans/refs/effect-lines/`:

| ref | what the page does | what we cannot do today |
|---|---|---|
| ref-07 top panel (horizontal streaks) | three weights in one set: many hairlines, some medium, a FEW heavy black strokes; each stroke is a spindle (thin–thick–thin); lengths differ a lot; strokes come in bands with uneven holes | one weight (width jitter only THINS, never thickens); round cap at the base, needle only at the tail; every run crosses the panel at ~the same length |
| ref-08 top panel (radial around the old man) | heavy black WEDGES (1–3 mm at the rim, needle at the centre) mixed with hairlines; inner ends scattered over a wide band, most rays long, some short; rays bunched, not evenly spaced | no accent population; inner-end jitter capped at half the span and uniform; no bundling in angle space (the owner's "no grouping setting") |
| ref-08 second panel, ref-07 second panel (perspective streaks into an impact point) | long tapered runs converging on one point, again mixed weights and lengths | `converge` exists; weights and lengths as above |
| ref-09 (ゴ… drip lines) | sparse THIN verticals hanging from the top edge, each a different length, no bundles | runs scatter along the direction; no "start at the reference line" mode |
| ref-10, ref-11 (集中線 sheets, centre on or off the panel) | long tapered wedges, big length variation, fuzzy inner ring, a burst that only fills a sector when the centre sits outside | length distribution uniform and narrow; no sweep limit (off-panel centres work by clipping, on-panel partial bursts do not exist) |

So the gap is five things, all in the renderer: (1) a weight MIX, (2) a proper stroke profile with an
entry taper and a needle exponent, (3) a skewed length distribution, (4) grouping in angle space,
(5) runs that hang off a start line. Plus presets that use them and a harness that lets a critic
see the result without the app.

### LANE A1 — renderer + presets in core + PNG harness (`mn-core` only)

Files owned: `crates/core/src/genlines.rs`, NEW `crates/core/src/genlines/presets.rs` (or a
`presets` module inside genlines.rs if the file split is awkward — your call, but keep genlines.rs
under ~2 200 lines), NEW `crates/core/examples/effect_lines_sheet.rs`, `crates/core/Cargo.toml`
only if a dev-dependency is missing. Nothing under `crates/app`.

#### A1.1 New spec fields (all `#[serde(default)]`, 0/false = exactly today's raster)

Add to `GenLinesSpec` AND thread through `FocusLinesParams` / `SpeedLinesParams` (both get the same
new fields; `UrchinParams` gets none — the flashes keep their shape):

| field | type | meaning | legacy |
|---|---|---|---|
| `accent_frac` | f32 0..1 | fraction of lines drawn as ACCENTS | 0 = none |
| `accent_mul` | f32 ≥1 | an accent's width = `width × accent_mul` (before `jit_width`) | 0 → treated as 1 |
| `entry` | f32 0..1 | fraction of the length at the BASE end that tapers IN from a needle (入り), giving a spindle when combined with `taper` | 0 = round cap as today |
| `needle` | f32 | exponent on the taper ramp: `hw(t) = hw·(1 − taper·t)^k` with `k = needle`; 0 → k = 1 (today's straight wedge); >1 thins fast then runs a long thin needle; <1 keeps a belly | 0 |
| `len_skew` | f32 0..1 | skews the per-line length draw toward LONG lines: use `u = rand(); u = u.powf(1.0 + 3.0·len_skew)` wherever a length/inner-end jitter is drawn | 0 = uniform |
| `sweep_deg` | f32 | radial only: rays are drawn only within `hand_deg ± sweep_deg/2`; the walk/scatter covers just that arc | 0 = full 360° |
| `start_mode` | u8 | stream only: 0 = scatter along the direction (today); 1 = every run STARTS on the reference line (the line through `anchor` perpendicular to the direction) offset by `jit_start`, and runs `len` in the direction | 0 |
| `jit_start` | f32 0..1 | stream, `start_mode 1`: start offset along the direction as a fraction of `len`, drawn per run | 0 |

Also for radial kinds, READ the existing `group` / `group_gap` / `jit_gap`: the same bundle walk
`render_speed` does, in ANGLE space over the sweep: `group` rays at `gap_deg`, then a hole of
`group_gap × gap_deg`. `ray_count()` becomes "how many rays the walk over the sweep produces" when
`gap_deg > 0`; keep the count path when `gap_deg == 0`. `group 0/1` = no bundling = today's ray set.

Rules that keep every saved file identical:
- Every new `rand()` call sits behind a `> 0` guard on its own field, like the density round did,
  so the draw sequence is untouched when the fields are absent. `legacy_renders_are_bit_stable` and
  `pre_flash_specs_load_with_the_old_meaning` must pass UNCHANGED.
- `GenLinesSpec::scale` scales nothing new (all new fields are fractions/degrees/flags).
- `recolor` unchanged.

#### A1.2 Stroke profile in `segment`

Replace the single `taper` ramp with a profile fn `fn width_at(t, taper, entry, needle) -> f32`
(t = 0 at the base `a`, 1 at the tip `b`):
- exit: `(1 − taper·t).max(0).powf(k)`;
- entry: if `entry > 0` and `t < entry`, multiply by `(t / entry).powf(0.7)` (a fast-in ramp from a
  point);
- `taper 0, entry 0, needle 0` returns exactly `1.0` for every t — the bit-stable path.
Keep the bbox clip and the capsule distance test. Accents just pass a bigger `hw`.

#### A1.3 The accent draw

Per line, after the width jitter: `if accent_frac > 0 && rand() < accent_frac { hw *= accent_mul.max(1) }`.
Accents are decided per LINE, so bundles get a heavy stroke inside them the way a hand does it.

#### A1.4 Presets move to core

New `pub struct LineOpts` in core = today's `FigureLineOpts` fields + the new ones above (`count,
width, jitter, r_in_frac, taper, gap_deg, gap_px, group, group_gap, jit_gap, jit_len, jit_width,
seed` + `accent_frac, accent_mul, entry, needle, len_skew, sweep_deg, start_mode, jit_start,
converge_far: f32` (stream: 0 = parallel; >0 = aim at a point `converge_far × drag length` beyond the
drag's end along the drag, the "Perspective stream" preset)). `Clone, Copy, PartialEq, Debug,
Serialize, Deserialize`. `same_as` moves with it.

New `pub enum LineKind { Stream, Focus, Urchin, Solid }` with `radial()`, `gen_kind()` (mirrors
`FigureMode::gen_kind`).

New `pub struct LinePreset { pub name: &'static str, pub kind: LineKind, pub opts: fn(u32) -> LineOpts }`
and `pub fn builtin_presets() -> &'static [LinePreset]` in this order (the sub tool rows will draw
it verbatim; the Materials thumbnails may use it later):

Stream group: `Stream line`, `Dense stream`, `Sparse stream`, `Perspective stream` (NEW),
`Drip lines` (NEW). Saturated group: `Saturated line`, `Dense saturated line`, `Dark burst`,
`Sea urchin flash`, `Solid flash`.

Starting values (Fable's call; the gauntlet loop tunes them, and the tuned numbers are what ships):

| preset | gap | group / hole / jit_gap | width mm / jit_width | accent frac / mul | taper / entry / needle | jit_len / len_skew | other |
|---|---|---|---|---|---|---|---|
| Stream line | 1.0 mm | 4 / 2.5 / 0.25 | 0.20 / 0.4 | 0.08 / 3 | 1 / 0.35 / 1.2 | 0.5 / 0.4 | start scatter |
| Dense stream | 0.6 mm | 6 / 2.0 / 0.25 | 0.15 / 0.4 | 0.06 / 3 | 1 / 0.3 / 1.2 | 0.4 / 0.4 | |
| Sparse stream | 2.5 mm | 0 / – / 0.3 | 0.30 / 0.3 | 0.1 / 2.5 | 1 / 0.4 / 1.0 | 0.6 / 0.3 | |
| Perspective stream | 1.0 mm | 4 / 2.5 / 0.25 | 0.20 / 0.4 | 0.1 / 3 | 1 / 0.3 / 1.2 | 0.6 / 0.5 | converge_far 2.5 |
| Drip lines | 2.5 mm | 0 / – / 0.4 | 0.12 / 0.3 | 0 / 1 | 1 / 0 / 1.5 | 0.7 / 0 | start_mode 1, jit_start 0.15 |
| Saturated line | 3.0° | 4 / 1.5 / 0.35 | 0.30 / 0.5 | 0.12 / 4 | 1 / 0 / 1.2 | 0.9 / 0.6 | hole 0.35 |
| Dense saturated | 2.0° | 5 / 1.3 / 0.35 | 0.25 / 0.5 | 0.10 / 3.5 | 1 / 0 / 1.2 | 0.9 / 0.6 | hole 0.35 |
| Dark burst | 1.5° | 3 / 1.2 / 0.4 | 0.50 / 0.5 | 0.30 / 4 | 1 / 0 / 1.0 | 0.9 / 0.7 | hole 0.2 |
| Sea urchin flash / Solid flash | unchanged (count 64, 0.85 / 0.95 mm, hole 0.3 / 0.45) | | | | | | |

#### A1.5 Placement geometry in core

Move the maths of `finish_figure_lines` into
`pub fn LineOpts::place(&self, kind: LineKind, a: [f32;2], b: [f32;2], bounds: [f32;4], seed: u64) -> GenLinesSpec`
— identical output to today for the existing fields (the app lane swaps its body for this call), plus:
`sweep_deg`, `start_mode`, `jit_start`, accents etc. copied through; `converge` = the far point when
`converge_far > 0`; `anchor` = for `start_mode 1` the DRAG START (the runs hang off the line through
it), else the drag midpoint as today; `hand_deg` = the drag direction as today. Test it against the
numbers `finish_figure_lines` produces today (write the test from the current code BEFORE changing
anything: `place_matches_the_app_drag_maths`).

#### A1.6 The harness: `cargo run -p mn-core --example effect_lines_sheet [-- <out-dir>]`

Default out-dir `target/effect-lines/`. For every builtin preset render ONE panel at 600 dpi,
panel 100 × 70 mm (2362 × 1654 px), with a fixed drag per kind:
- stream kinds: drag from (15 %, 55 %) to (85 %, 45 %) of the panel (slightly rising, left to right);
  Drip lines: drag from (50 %, 2 %) straight down to (50 %, 40 %).
- radial kinds: centre (55 %, 45 %), drag radius 22 % of the panel width.
Also render two "off-panel centre" saturated variants: centre at (50 %, 120 %) with `sweep_deg 170`
pointing up (ref-11 left), and centre at (110 %, −10 %) (ref-10 right).
Write `<preset>.png` (the panel, downscaled ×3 to ~787 px wide, plus a 1:1 crop of a 600 × 600 px
patch at the centre-right saved as `<preset>-crop.png`, because the critic must see line ENDS at
print scale), and `sheet.png` = all panels in a grid with the preset name burnt in (any 8 px
bitmap font or just the file name; a name in the file name is enough if burning text is a hassle).
Deterministic (fixed seeds). Print the out-dir at the end. This is the artifact the gauntlet judges.

#### A1 tests (`cargo test -p mn-core <name>`)
- unchanged and must still pass: `legacy_renders_are_bit_stable`, `pre_flash_specs_load_with_the_old_meaning`,
  `speed_lines_grouping_leaves_holes`, `gap_deg_derives_the_ray_count`, every other genlines test.
- new: `radial_grouping_leaves_angular_holes` (mirror of the stream one: measure ink angles on a
  ring, expect tight gaps × group then a hole), `sweep_limits_the_arc` (no ink outside the arc),
  `accents_are_wider_than_the_rest` (two width populations in a ring sample),
  `entry_taper_starts_at_a_point` (ink width near the base grows from ~1 px), `needle_exponent_thins_faster`,
  `len_skew_biases_long` (mean inner-end radius smaller than with skew 0),
  `anchored_runs_start_on_the_reference_line`, `place_matches_the_app_drag_maths`,
  `builtin_presets_all_render_without_panic`.

#### A1 acceptance
`cargo check --workspace --all-targets` zero warnings; the example writes the sheet; the new tests pass;
the pinned ones are untouched (do not re-pin a fingerprint — if one changes, a guard is missing).

---

### GAUNTLET LOOP (after A1) — "have opus do some loops on this"

Run by Fable with the `gauntlet-loop` skill. One agent at a time. Builder and critic alternate.

- **Builder** (Opus): edits ONLY the preset numbers in `presets.rs` and, if a look is unreachable
  by numbers, the renderer in `genlines.rs` (new knob behind a default, same rules as A1). Reruns
  the example. Writes what it changed and why in `docs/plans/2026-09-06-gauntlet-REPORT.md`.
- **Critic** (Opus, FRESH context, gets: the five reference scans, the current `target/effect-lines/`
  PNGs, this table, nothing else). Manga reads RIGHT TO LEFT (panels, then balloons) — irrelevant to
  a line set but stated in every page-reading brief by house rule. Scores each preset 1–5 on:
  weight mix (hairlines AND a few heavy strokes present?), length variation (inner ends scattered,
  not a clean ring / not all crossing), rhythm (bundles and uneven holes, nothing mechanical), tip
  shape (needles/spindles, no round caps or blunt ends visible in the crop), page test ("would this
  pass in a Jump chapter next to the reference?"). Pass = every preset ≥ 4 on every axis. Else a
  ranked "what is still off" list per preset, concrete ("Saturated line: inner ends still form a
  visible ring — spread them more; no heavy wedges seen").
- Reference pairing: Saturated line / Dense saturated ↔ ref-10 left, ref-11 right; Dark burst ↔
  ref-08 top, ref-11 left; Stream line / Dense / Sparse ↔ ref-07 top; Perspective stream ↔ ref-08
  second panel, ref-07 second panel; Drip lines ↔ ref-09; off-panel variants ↔ ref-11 left, ref-10 right.
- Max 4 rounds. Fable reviews each critic verdict before the next builder round and stops early if
  the critic is grading noise.

---

### LANE A2 — wire the app to the core presets + knobs in both panels

Files owned: `crates/app/src/cmd/tools.rs` (the `FigureLineOpts` block only), `crates/app/src/app.rs`
(the two `figure_*` fields' types only), `crates/app/src/app/canvas_input.rs` (`finish_figure_lines`
only), `crates/app/src/ui/subtool.rs` (the `Tool::Figure` arm only), `crates/app/src/ui/property/frames_balloons.rs`
(`sec_figure`, `sec_obj_genlines`, `sec_obj_genlines_density` only), `crates/app/src/app/figure_stage_tests.rs`
/ `gen_lines_object_tests.rs` for tests, `docs/manual/` page for effect lines.

- `pub use mn_core::genlines::LineOpts as FigureLineOpts;` — delete the app copy and its constructors.
  `finish_figure_lines` = compute `bounds` (panel or page, as today) and call `opts.place(kind, a, b, bounds, seed)`.
- Sub tool rows: draw `builtin_presets()` in order under the two group captions (`Stream line`,
  `Saturated line`); the flash rows stay in the Saturated group as today. Same arming behaviour.
- `sec_figure` (the knobs for the NEXT drag), rows in THIS order, one `ui.horizontal` per row, hover
  text on every value: Gap (° or mm) · Bundle (count + hole ×) · Width (mm, not px — use `mm_to_px`
  like the object panel) · Accents (frac + ×) · Taper · Entry · Needle · Length wobble + Long bias
  (`jit_len`, `len_skew`) · Position wobble (`jit_gap`) · Width wobble · Hollow centre (radial) ·
  Sweep (radial) · Start: scatter / from the line (+ start wobble) (stream) · Fan toward a point
  (`converge_far`) (stream) · seed hint line. The single "Jitter" row goes away (the split wobbles
  replace it; the legacy `jitter` field is written as `jit_gap` for saved-file fallback).
- `sec_obj_genlines` / `sec_obj_genlines_density` (the SELECTED set): expose the same new knobs
  through `gen_bar` in the same order, committing through `gen_commit` as today. Radial sets show
  Bundle / hole now (the owner's missing grouping setting).
- Manual page: one paragraph per knob, plain words.

Tests: `figure_stage_tests` still pass; new `figure_drag_places_the_core_spec` (drag → `GenLinesPlace`
spec equals `place(...)`), `object_panel_edits_radial_bundle` (a `GenLinesApplyTo` with `group 4` on
a focus spec regenerates with holes — reuse the angular-holes measurement from A1 through the doc).

Acceptance: pick each preset row, drag, see a set that matches the harness PNG for that preset; the
Object tool re-selects it and every knob edits live; `cargo check` zero warnings.

---

### LANE A3 — user sub tools for effect lines (duplicate / rename / delete / save current)

Files owned: `crates/app/src/ui/subtool.rs` (`Tool::Figure` arm), `crates/app/src/app/layout.rs`
(one new `figure_presets=` line, the `gradients=` pattern at 174 / 459 / 517 / 558 / 670),
`crates/app/src/app.rs` (one field), `crates/app/src/cmd.rs` + `crates/app/src/cmd/tools.rs` (the new
commands), a new `crates/app/src/app/figure_presets_tests.rs`.

- `app.figure_presets: Vec<UserLinePreset { name: String, kind: LineKind, opts: LineOpts }>`,
  persisted as one JSON line in ui.txt (`figure_presets=`), loaded like `gradients`.
- Rows: after the builtin rows of each group, a `Mine` caption with the user rows of that group's
  kinds. Right-click on ANY effect-line row (builtin or mine) → context menu with the brush
  `organise_menu` shape: inline rename box (mine only), `Duplicate` (builtin → a new Mine row
  "<name> copy" carrying the row's opts; mine → "<name> 2"), `Save current settings as sub tool`
  (only on the ARMED row: the knobs as tuned now become a new Mine row), `Update from current`
  (mine + armed: overwrite the row's opts), `Delete` (mine only). Commands: `FigurePresetAdd`,
  `FigurePresetRename`, `FigurePresetUpdate`, `FigurePresetDelete` — plain state edits, not undo
  steps (brush presets are not either).
- A user row arms exactly like a builtin (`same_as` highlight).

Tests: `user_preset_round_trips_through_ui_txt`, `duplicate_of_a_builtin_lands_in_mine`,
`save_current_captures_the_tuned_knobs`, `deleting_the_armed_preset_keeps_the_knobs`.

Acceptance: right-click Saturated line → Duplicate → rename it "Ref 08 wedges" → tweak accents →
Update from current → restart (headless test: reload layout) → the row is back with the knobs.

---

## Part B — Tool Property: one settings box, per-setting eye toggles, reordering

### What is there today
- `crates/app/src/ui/property.rs`: `Section { id, title, body: fn }` registry (~300), the per-tool
  lists in `prop_sections_for_tool` (434–680), the palette body `tool_property_body` (32–75) which
  draws sections in list order and skips ids in `app.prop_hidden`. The wrench button (~55) toggles
  `app.prop_detail_open`.
- `crates/app/src/ui/dialogs.rs::property_detail_window` (147–195): a plain window listing every
  section with a checkbox = "hide from the palette". Section-level only, no ordering, not styled
  like Preferences.
- Persistence: `prop_hidden` in ui.txt via `crates/app/src/app/layout.rs` (61, 316, 517, 586) and
  workspaces (`workspaces.rs` 53 / 106). NOTE the brush tools have a SEPARATE system (`detail_open`,
  `Sub Tool Detail`, `pen.rs` sliders hidden by label — `app/tests.rs:4424` pins one). Leave it alone
  in this round.
- Preferences window shape to copy: `crates/app/src/ui/prefs_dialog.rs::prefs_window` (search box,
  left tab rail 110 px, body, painted divider — read the comment about the vertical separator
  feedback loop before touching layout; footer with Reset).

### LANE B1 — the row model, the window, text + figure converted

Files owned: `crates/app/src/ui/property.rs`, `crates/app/src/ui/property/text.rs`,
`crates/app/src/ui/dialogs.rs` (`property_detail_window` only), `crates/app/src/app/layout.rs` (one new
`prop_order=` line + `prop_hidden` semantics), `crates/app/src/app/workspaces.rs` (carry the new
line), `crates/app/src/app.rs` (fields), `crates/app/src/app/tests.rs` (new tests at the end only).
NOT `frames_balloons.rs` (Lane A2 is in it) — its sections stay whole-section rows for now: a section
with no `rows` list is drawn as ONE row whose label is the section title, so the window still lists
it with an eye toggle. That keeps B1 shippable before B2.

B1.1 **Rows as data.** `pub(crate) struct Row { id: &'static str, label: &'static str, body: fn(&mut Ui, &mut App), applies: fn(&App) -> bool }`
(`applies` default = always; a row that does not apply to the current sub tool is skipped in the
palette and drawn greyed in the window with "not for this sub tool"). `Section { id, title, rows: &'static [Row] }`;
keep a `body` fallback for unconverted sections (`rows` empty → the section's `body` IS its single row,
id = section id).

B1.2 **Order + visibility state.** `app.prop_hidden: BTreeSet<String>` now holds ROW ids (a hidden
section id from an old ui.txt hides every row of that section — migrate on load: if an id matches a
section, insert its rows' ids). `app.prop_order: BTreeMap<String /*context id*/, Vec<String /*section or row ids*/>>`
persisted as one JSON line `prop_order=` in ui.txt (`gradients=` pattern). Context id = the tool
name, or `obj.text` / `obj.balloon` / `obj.gen` / `obj.frame` under the Object tool. The palette
draws sections in stored order then rows in stored order; ids not in the stored list append in
default order; ids in the list that no longer exist are ignored.

B1.3 **The window** (`property_detail_window`), styled like Preferences: title "Tool Property settings —
<context>", 540 wide, search box on top (filters rows by label across sections), left rail = the
sections of this context (click = jump; ▲ ▼ buttons on the selected rail entry reorder SECTIONS),
right body = the selected section's rows: each row = eye toggle (an eye icon from `icons.rs` if one
exists, else the checkbox — say which in the report) + ▲ ▼ + the row's live widget, greyed with a
note when `applies` is false. Footer: "Show all" (clears hidden for this context), "Default order"
(clears `prop_order` for this context), Close. The palette header wrench keeps opening it. The
"uncheck a category" line goes.

B1.4 **Convert `text.rs`** to rows. Rows and DEFAULT order (Fable's ruling; the owner's example was
direction next to alignment):
- Text style (workstyle picker) → Font: `font`, `size` → Direction: `vertical` (縦書き/横書き),
  `auto_tcy` → Align: `rows` (Left/Center/Right or Top/Center/Bottom), `in_frame` → Spacing:
  `line_mode`, `line`, `letter` → Style: (its rows as they are: bold/italic/etc.) → Edge: `edge` →
  Furigana: `reading`, `size`, `gap`, `adjust`, `along` → Guide.
So the section order becomes Workstyle, Font, Direction, Align, Spacing, Style, Edge, Furigana,
Guide (today: Workstyle, Font, Direction, Style, Furigana, Align, Spacing, Edge, Guide). Same order
for the Object-tool text context. `text_state(app)` may be recomputed per row (it is cheap).

B1.5 **Figure**: rows `figure.brush.*` (the brush sliders stay one row each if `brush_sliders`
already iterates a slider list — else one row), `figure.opts.*` = the A2 knob rows (B1 lands BEFORE
A2 finishes? No — B1 runs after A2; convert `sec_figure` into rows then, in frames_balloons.rs, as
the ONE exception to "not that file", coordinated by Fable). `applies`: Brush and Dynamics rows are
`!figure_mode.generates()`; every effect-line knob row is `generates()` plus its kind check.

Tests (`cargo test -p mn-app <name>`): `hidden_section_id_migrates_to_its_rows`,
`prop_order_round_trips_through_ui_txt`, `palette_draws_rows_in_stored_order` (headless: build the
section list, apply an order, assert the id sequence), `default_text_order_puts_direction_before_align`,
existing `prop_hidden` tests unchanged.

Acceptance: wrench → a Preferences-shaped window; hide "In frame" → it leaves the palette; move
Direction above Font → the palette follows; restart → both stick. Default text palette: Font, then
縦書き/横書き, then Left/Center/Right on the next row.

### LANE B2 — convert the remaining panels (after A2 and B1)

One lane per file, sequential: `frames_balloons.rs` (balloon, frame, figure, object variants),
`tone.rs`, `select.rs`, `gradient.rs`, `rulers.rs`. Mechanical: split each `sec_*` body into row fns,
list them in the section, keep the existing default order except where the row order is obviously
wrong (say so in the report). No behaviour change. Tests: each file's existing tests + the palette
order test extended to every context (`every_context_has_unique_row_ids`).

---

## Later / not this round
- Anti-aliased effect lines (coverage in `segment`; `recolor` must then multiply by alpha).
- Curved speed lines along a ruler / stroke (ref-07's arc).
- Materials-bank thumbnails regenerated from `builtin_presets()` (`gen_materials.rs`).
- The brush `Sub Tool Detail` window joining the row model.
