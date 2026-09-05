# Plan 2026-09-05: one shortcut system, Ruler as a tool, frame-folder fixes, lag, touch rotate

> **STATE 2026-09-05 (evening, owner paused all agents).** Pushed through `abe1231`: Lanes 1, 3, 4 done; Lane 2 done except `docs/manual/rulers.html` + the CODE-MAP paragraph; Lane 5 item K landed (background write, pill, exit flush) but the REAL freeze is the .ora encode (14 s for 3 pages, write 1 ms) — K resumes with: per-layer PNG encode cost (compression level, encode only layers whose revision changed, snapshot Arc tiles and encode off-thread), re-save of unchanged pages skipped, then O, L, M, N. Lane 6 not started. Resume = one agent at a time (owner rule for this session): resume Lane 5 first from its report, then Lane 2 docs, then Lane 6. Lucide icons, zoomed thumbnails and the default U cycle were done by Fable directly.

Written by Fable (main chat) for Opus implementation lanes. Owner asks from the 2026-09-05 session,
in his words where it matters. Each item: what the owner saw, the cause on the page, the exact change,
files, tests, acceptance. Lanes own DISJOINT files; a lane that needs a file outside its list stops
and reports instead of editing it.

House rules for every lane (from AGENTS.md + standing owner protocol):
- Save work to disk as you go; keep a running "done / next" note at the top of your report file
  `docs/plans/2026-09-05-lane-<N>-REPORT.md`. Never hold a round's work only in conversation.
- NO full local `./build.sh --test`. Local gate = `cargo check --workspace --all-targets` with ZERO
  warnings + the targeted tests named per item (`cargo test -p mn-app <name>`). CI is the full gate.
- Cargo's target lock serialises builds across lanes; waiting on it is normal. Never clear `target/`.
- Never launch the app window. Headless tests only (`headless_renderer()` pattern in `app/tests.rs`).
  If a screenshot is needed use the offscreen `--shot-*` path in `screenshot.rs`.
- Do not commit. Fable reviews the diff per lane and commits.
- Owner's machine is RAM-starved and he is drawing in the app while you work: keep builds to
  `cargo check` + targeted tests, nothing else.

---

## LANE 1 — Shortcuts: defaults as rows, cycles you can edit (items A, B)

Files owned: `crates/app/src/keymap.rs`, `crates/app/src/ui/shortcut_tab.rs`,
`crates/app/src/main.rs` (only `builtin_chords`, `builtin_targets`, `shortcut`), `docs/manual/keys.html`.

### What the owner saw
"u is the shortcut for the frame tool but I don't see it in the shortcuts list. All keys you can press
should be in the shortcuts list and modifiable." And: "I added another u to also have it do tool Figure,
but adding another one to get u to cycle to straight line ruler as well just overwrites the previous one."

### Cause
- The Shortcuts tab (`ui/shortcut_tab.rs::tab`) lists only `keys.json` rows. Built-ins live in two
  hardcoded tables in `main.rs`: `builtin_chords()` (commands, with labels) and `builtin_targets()`
  (the tool letters P/B/G/M/W/O/F/V/U/T/I/H/R/E/J/D). They only surface as the grey
  "shadows built-in: …" note in the Add box (`conflict_note`).
- `keys.json` is a map, one key one value. A cycle is a JSON array (`"u": ["tool: Frame border",
  "tool: Figure"]`) that the tab can only produce by hand-editing the row text. The Add flow
  *re-aims* an existing row (`existing.text = label`) instead of appending.
- `keymap::Bind` is `Cmd(AppCmd) | Targets(Vec<Target>)`: a cycle may hold tool targets ONLY.
  "Straight line ruler" is a palette command, so it cannot join a cycle at all today ("bad cycle").
- A `keys.json` entry SHADOWS the built-in for that chord; it never adds to it.

### Change
A1. **One table of every binding.** Add `keymap::Binding { chord, entries: Vec<Entry>, source: Default | File }`
    and a function `keymap::effective_table(app) -> Vec<Binding>` that merges: every `builtin_chords()`
    row and every `builtin_targets()` row as `source: Default`, then every `keys.json` row as
    `source: File` (a file row for a chord REPLACES the default row of that chord in the list, and shows
    a small "default: …" hint so the user can see what it shadowed). Give `builtin_targets` a labelled
    twin (`builtin_target_rows() -> Vec<(Chord, &'static [Target])>`) rather than parsing the match; keep
    the existing `builtin_targets` match as the fast path and add a pin test that the two agree
    (`every_builtin_target_row_is_the_match`).
A2. **The tab draws the merged table**, sorted: bare letters first (A..Z), then function keys, then
    modifier chords. Default rows are drawn in the weak text colour with a "default" tag; editing a
    default row's text or key turns it into a File row (writes to `keys.json` on save). A "↺" button on a
    File row that shadows a default restores the default (removes the file row). Keep the raw-text edit
    for power users; the round-trip test `the_round_trip_preserves_every_entry` must still pass.
B1. **Cycles as first-class rows.** A binding with several entries draws as ONE row whose value cell is
    a horizontal list of chips ("Frame border" "Figure" "Straight line ruler"), each chip with × (remove)
    and ‹ › (reorder). The Add flow: if the captured chord already has a row, the picked label is
    APPENDED to that row's cycle (status: `"u" now cycles 3`), not re-aimed. Re-aim stays available as
    the chip × plus add.
B2. **Mixed cycles.** Change `Bind` to `Bind::Seq(Vec<Step>)` with `enum Step { Cmd(AppCmd), Target(Target) }`
    (a single command = a one-step Seq; keep `Bind::Cmd`/`Bind::Targets` constructors only if it
    shortens the diff). `main.rs::shortcut` executes a Seq like this: if every step is a Target, exactly
    the current `subtools::press` cycle. Otherwise: stateless cycle too — find the step that is
    "current" (a Target is current per `subtools::is_current`; a `RulerArm(k)` command is current when
    `app.ruler_pending == Some(k)`; any other command is never current) and run the NEXT step; if none
    is current, run step 0. Repeat presses on auto-repeat never advance (mirror the existing `!repeat`
    gate). `keymap::parse` accepts arrays mixing `tool:` targets and command labels; the
    one-bad-entry-one-complaint rule stands.
B3. **Default U becomes a cycle**: `0x55 => [Figure, Frame border, <Ruler>]` — but ONLY after Lane 2
    lands `Tool::Ruler`. Until then leave U as is; Fable adds the third step at merge.

### Tests (targeted, `cargo test -p mn-app <name>`)
- keymap: `a_chord_can_name_a_tool_target` (unchanged), new `a_cycle_may_mix_commands_and_targets`,
  new `every_builtin_target_row_is_the_match`, `garbage_files_degrade_to_empty`.
- shortcut_tab: `the_round_trip_preserves_every_entry`, `every_builtin_chord_still_consumes`,
  `a_saved_file_rebinds_without_restart`, new `adding_to_a_bound_chord_appends_to_its_cycle`,
  new `restoring_a_shadowed_default_removes_the_file_row`.
- main: `a_binding_reaches_the_command_queue`; new `a_mixed_cycle_advances_from_the_current_step`.

### Acceptance
Open Preferences ▸ Shortcuts: U is listed (default, "tool: Frame border"). Press U in Add, pick
"tool: Figure": the U row now shows two chips. Pick "Straight line ruler": three chips. Save. In the
app, U cycles Frame border → Figure → straight ruler armed → Frame border. Ctrl+Z is listed as a default.
`docs/manual/keys.html` gets a paragraph: defaults are listed, cycles are chips, the file is still
`keys.json`.

---

## LANE 2 — Ruler is a tool (item C)

Files owned: `crates/app/src/cmd/tools.rs`, `crates/app/src/subtools.rs`, `crates/app/src/ui/subtool.rs`,
`crates/app/src/app/canvas_input.rs` (ruler parts only), `crates/app/src/ui/top.rs` (Ruler menu),
`crates/app/src/ui/quick.rs` (ruler rows), `crates/app/src/app/ruler_undo_tests.rs`, `docs/manual/rulers.html`,
`docs/CODE-MAP.md` (the sub tool four-place paragraph). NOT `main.rs`, NOT `keymap.rs`.

### What the owner saw
"Ruler doesn't seem to be a tool in the tool box for some reason, how come?" He wants CSP's U cycle:
Figure → Frame border → Ruler.

### Cause
Rulers predate the sub tool registry. They are commands: `AppCmd::RulerArm(RulerKind)` from the Ruler
menu (`ui/top.rs`) and the palette (`ui/quick.rs`, labels "Straight line ruler", "Vanishing point
ruler", "Perspective ruler (1/2/3-point)", "Curve ruler", "Parallel ruler", "Radial line ruler",
"Concentric circle ruler", "Symmetrical ruler", guides). `RulerArm` sets `app.ruler_pending` and the next
canvas drag creates the ruler (`canvas_input.rs` ~3800–3920). There is no `Tool::Ruler`, so no left-strip
icon, no Sub Tool group, no `tool:` target.

### Change
C1. Add `Tool::Ruler` (label "Ruler") to the left strip, placed after `Frame` in CSP order
    (CSP's strip ends … Frame border, Balloon? no: CSP order is … Text, Frame border, Correct line,
    Balloon, Ruler; put Ruler LAST before Pan). `Tool::strokes()` false. One icon (a ruler glyph in the
    existing icon style; look at how `Tool::Frame` gets its glyph and copy the pattern).
C2. Add `SubTool::Ruler(RulerKind)` rows and the group constant `group::CREATE_RULER = "Create ruler"`.
    Rows, in this order and with these labels (CSP's own names): Straight line, Curve, Parallel line,
    Radial line, Concentric circle, Vanishing point, Perspective 1-point, Perspective 2-point,
    Perspective 3-point, Symmetrical, Guide horizontal, Guide vertical. CODE-MAP's four-place rule:
    `SubTool::ALL` entry + `subtools::group_of` arm + `apply_state` arm + `is_current` arm, for EVERY
    row. `is_current` for a ruler row = `app.tool == Tool::Ruler && app.ruler_mode == kind`.
C3. State: `app.ruler_mode: RulerKind` (the tool's remembered sub tool, default Line). While
    `app.tool == Tool::Ruler`, a canvas drag creates a ruler of `ruler_mode` — the SAME code path
    `RulerArm` uses today, so refactor: `RulerArm(k)` becomes "select Tool::Ruler with mode k"
    (keeps the menu and palette working, and keeps Lane 1's `is_current` rule for `RulerArm` simple:
    current when tool is Ruler and mode is k). `ruler_pending` may stay as the internal "armed for one
    drag" if the Ruler tool needs it; if it becomes dead, delete it.
C4. Tool Property panel for Ruler: whatever the current per-kind options are (the concentric `dr`,
    perspective point counts) move from the menu into the property panel; the menu keeps working.
C5. `subtools::nameable_tools()` picks the new tool up automatically → `tool: Ruler`,
    `tool: Ruler / Create ruler`, `tool: Ruler / Create ruler / Straight line` are valid `keys.json`
    targets with no keymap change. Verify with `subtools::tests::the_registry_holds_every_row_once`.
C6. Memory: `subtools::note_memory` / `restore_from_memory` carry `ruler_mode` like any other tool mode.
C7. Do NOT edit `main.rs`. Report the exact target strings so Fable can add the U default cycle.

### Tests
- `subtools::tests::the_registry_holds_every_row_once`; new `a_ruler_row_reports_current`.
- `ruler_undo_tests.rs`: every existing test still passes when driven through the tool instead of
  `RulerArm` (add one new test `the_ruler_tool_drag_creates_a_line_ruler` that selects the tool via
  `subtools::press(&[Target::SubTool(Straight line path)])` then drags).
- `shortcut_tab::the_addable_namespace_is_all_parseable` (Lane 1's file, but the test reads the
  registry: run it, do not edit it).

### Acceptance
Left strip shows a Ruler tool. Its Sub Tool list shows "Create ruler" with the twelve rows. Clicking
"Straight line" then dragging on canvas makes a line ruler, undoable, same as the menu today.
`keys.json` with `"k": "tool: Ruler / Create ruler / Curve"` selects the Curve ruler. The Ruler menu still
works. `docs/manual/rulers.html` says rulers are a tool now.

---

## LANE 3 — Frame folders: lag, list order, blue veil, layer click (items D, E, F, G)

Files owned: `crates/app/src/ui/overlay/page.rs`, `crates/core/src/doc.rs` (`divide_frame_folder`,
`divide_frame_folder_dup` only), `crates/app/src/cmd/frames.rs`, `crates/app/src/app/frames.rs`,
`crates/app/src/ui/layers/rows.rs` (the plain-click arm only), `crates/app/src/cmd/layers.rs`
(`SelectLayer` only), `crates/app/src/app/tests.rs` (new tests appended at the end), `docs/manual/layers.html`.

### D. "MangaNakama froze for a bit and almost crashed just from dividing a panel" / "dragging one frame folder lagged"
**Cause (hypothesis to VERIFY first, then fix):** the frame-focus veil in `ui/overlay/page.rs`
(~line 117 on) is drawn every frame while a frame folder is the active layer, as ONE `rect_filled`
PER SCREEN ROW (a scanline even-odd fill, `y += 1.0`). A 1000-row canvas with one slanted cut is
~2000 egui shapes per frame, more with more panels, and it is exactly the state the owner was in for
both lags (a frame folder active: right after a divide, and while dragging a folder in the list).
**Verify:** add a debug counter (or a test) that counts shapes the veil emits for a 600×400 canvas with
two panels and a slanted cut; record the number in your report. If the veil is NOT the cost, profile
`FrameDivide` (`cmd/frames.rs`) and `derive_frame_raster` (`core/doc.rs`) with `std::time::Instant`
prints in a headless test and report before touching anything else.
**Fix:** replace the per-row rects with ONE `egui::Mesh`: sort the distinct y-levels of all panel
vertices (plus the page top/bottom); within each y-band every polygon edge is a straight line, so the
even-odd crossings at the band's top and bottom pair up into trapezoids; emit each "outside" trapezoid
as two triangles. Clamp to the visible area as today. Result: O(vertices × panels) triangles instead of
O(rows). Keep the even-odd semantics (concave panels still work). Use `painter.add(egui::Shape::mesh(m))`.

### E. "Frame 2 (the bottom one) is on the layer list higher than frame 1, which is counterintuitive"
**Cause:** `core::doc::divide_frame_folder` always inserts the new folder's block at
`children_range(index).start` (the new block lands on one fixed side of the original), and the
reading-order renumbering (`app/frames.rs::frame_pos`) then names them by geometry, so the list order
and the numbers disagree whenever the split-off half reads FIRST.
**Change:** after computing both halves' reading positions (use the same order `frame_pos` uses:
top-to-bottom rows, right-to-left within a row — this is a manga app, RTL), place the new block so that
the list reads top = reading position 1: the folder that reads EARLIER sits HIGHER in the list. Concretely
in `divide_frame_folder` / `_dup`: decide `above: bool` from the two `FrameSet.slot` boxes (row first,
then RTL), and insert at `children_range(index).end` (above the original block) when the new folder reads
earlier, else at `.start` as today. Both `keep` and `split_off` slots are already computed by the caller
(`cmd/frames.rs`, `slot_for` / `cut_union`) — pass what you need. The undo record must still restore the
exact pre-divide stack (`record_structure` does; add the assertion to the test).
**Do not** reorder existing folders on load or on rename; this is divide-time placement only.

### F. "The blue overlay only hits when I click on the frame folder; it should apply whenever I'm on any layer in the frame folder"
**Cause:** the veil condition is `app.doc.active_layer().is_frame()`.
**Change:** condition becomes "the active layer is a frame folder OR is inside one": use
`cmd::frames::enclosing_folder(doc, active)` (already exists, used by `cmd/edit.rs`) to find the frame
folder; draw the veil for THAT folder's frames. Keep the veil off for layers outside any frame folder.
Do F after D, since F makes the veil show far more often.

### G. "If I have multiple layers selected and single click one of them, it should reselect just that one"
**Cause:** `rows.rs` plain-click arm pushes `SelectLayer(i)` only `if !selected`, and `SelectLayer` in
`cmd/layers.rs` does not touch `doc.layer_multi`.
**Change:** (1) `SelectLayer(i)` clears `doc.layer_multi` (plain click = single selection; Ctrl/Shift
paths are separate commands and keep their behaviour). Check the other callers of `SelectLayer` (grep)
and confirm none relies on multi surviving; the pin test below documents the new rule. (2) In `rows.rs`,
a plain click on a row that is already active but has a non-empty `layer_multi` also pushes
`SelectLayer(i)` so the selection collapses to that row.

### Tests (append to `app/tests.rs`)
- `the_frame_veil_is_one_mesh_not_a_rect_per_row` (shape count bound, e.g. < 64 shapes for two panels
  and one slanted cut).
- `a_divide_lists_the_earlier_reading_panel_higher` (top/bottom cut: the top panel's folder is the
  higher row; left/right cut: the RIGHT panel's folder is higher). Also undo restores the stack.
- `the_veil_shows_for_a_layer_inside_the_frame_folder`.
- `a_plain_click_collapses_a_multi_selection` (multi of 3, `SelectLayer(one)`, `layer_multi` empty,
  active = one).
- Existing: every test in `app/tests.rs` that mentions `FrameDivide` (grep) must still pass.

### Acceptance
Divide a panel top/bottom: Frame 1 (top panel) is the higher row, badge 1. Select "Layer 1" inside
Frame 1: the blue veil shows outside Frame 1's panel. Divide and drag folders with no stutter (report the
shape count before/after). Ctrl-click three layers, plain-click one: only that one is selected.

Note for the owner (not a change): the numbered badge on a frame folder is the COMPUTED reading position
(`frame_pos`); the name "Frame N" is renumbered only while it is still a default name. They agree until
a folder is renamed or pinned; then the badge is the one that stays true. Keep both.

---

## LANE 4 — Process priority, touch rotate activation (items H, I)

Files owned: `crates/app/src/shell.rs` (priority call at window creation), a new
`crates/app/src/priority.rs`, `crates/app/src/app.rs` (the `TOUCH_TWIST_THRESHOLD` block and
`touch_move` ~1371–1450 and ~4197–4260 only), `crates/app/src/app/tests.rs` (the two-finger tests
~6900–7160 only; Lane 3 appends at the END of the same file — do not touch anything after line 7200 and
do not reformat), `docs/manual/drawing.html`.

### H. "Even the G-pen is laggy while two Claude sessions run. Is our priority low? Should we raise it in all builds?"
**Answer to record in the report:** CPU contention is only part of it; the machine is RAM-starved
(15.8 GB, two agent sessions + cargo). A higher priority class helps against CPU contention, not
against paging. It is still worth doing, and it is safe at ABOVE_NORMAL (never HIGH/REALTIME: those
starve the digitizer driver and the compositor, which makes pen input WORSE).
**Change:** `priority.rs`: `pub fn raise_for_interactive()` calling
`SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS)` and
`SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL)` on the thread that pumps input and
renders (the shell's window thread). Log the outcome once to the status/log line ("priority:
above-normal" or the `GetLastError`). Windows-only `#[cfg(windows)]`; a no-op elsewhere. Call it from
`shell.rs` where the window is created (NOT `main.rs`, Lane 1 owns that). Make it a preference only if
one already exists for similar system knobs; otherwise always on. Also: if the brush/dab worker threads
exist (`crates/brush` or the tile callback in `app.rs` ~3211), leave THEIR priority alone — raising the
UI thread above them is the point.

### I. "Two-finger rotate is too sensitive: my zoom gesture's imperfection is read as a rotate. Make rotating harder to start."
**Cause:** `app.rs` `TOUCH_TWIST_THRESHOLD = 4°` cumulative twist latch (2026-08-19 fix, with
`SNAP_ENGAGE`/`SNAP_RELEASE` for the quarter snap). Four degrees is below a normal pinch's wobble.
**Change:** two gates, both must pass before the rotate latch engages:
1. Cumulative twist ≥ `TOUCH_TWIST_THRESHOLD` raised to **12°**.
2. **Twist must dominate the pinch**: track cumulative |log(scale change)| since the gesture started;
   engage only if `twist_rad > k * pinch_log` with k chosen so a pinch of ±25 % with ±3° wobble never
   engages, and a pure twist with < 5 % scale drift engages by 12°. (Suggested: engage requires
   `twist_deg >= 12 && twist_deg >= 40 * |ln(scale_ratio)|`; tune with the tests.)
   Once latched, behaviour is exactly today's (the accumulated twist is applied on engage so nothing is
   lost; the snap logic untouched). Pan and zoom keep tracking from the first event as today.
**Tests:** `two_finger_slow_twist_rotates` (must still reach the quarter within tolerance: the latch
applies the accumulated twist), `two_finger_pinch_noise_does_not_rotate` (strengthen the wobble to ±3°
and the pinch to ±25 %), `two_finger_pinch_twist_is_one_gesture`, `two_finger_rotate_snaps_to_quarters`,
`two_finger_snap_holds_then_releases_at_any_speed`, new `a_wobbly_zoom_never_starts_a_rotate`.
Document the new feel in `docs/manual/drawing.html` (one sentence).

---

## Merge order (Fable)
Lane 3 and Lane 4 first (his felt pain today), reviewed and committed separately. Then Lane 1 and Lane 2
together; after both land, Fable sets the U default to `[Figure, Frame border, Ruler / Create ruler /
Straight line]` in `main.rs::builtin_targets` and adds the pin test. Push after each commit; CI is the gate.

---

## LANE 5 — Save-in-progress indicator, balloon pen live preview (items K, L)

Files owned: `crates/app/src/cmd/file_io.rs`, `crates/app/src/ui/overlay/frames.rs` (the `balloon`
band only), `crates/app/src/ui/top.rs` (ONLY if a title-bar/status indicator is added there; Lane 2 edits
the Ruler menu in the same file, so touch nothing but the indicator), `crates/app/src/ui/status.rs` or
wherever the status line lives, `docs/manual/files.html`, `docs/manual/text.html` (balloon section).

### K. "When a project is saving there should be some indicator; right now it displays nothing"
**First establish the fact:** is `AppCmd::SaveOra` / project save synchronous on the UI thread
(`cmd/file_io.rs` ~59 on)? Report it. If it is synchronous, the UI cannot repaint during the save, so any
"saving…" widget drawn in the same frame is never seen. Two acceptable answers, pick by cost:
1. **Cheap, honest:** before the blocking write, set `app.saving = Some(label)` and request ONE repaint
   that draws the indicator, then perform the write on the NEXT frame (a two-phase command: `SaveOra`
   arms, the following `ui::build` sees `saving` set, draws the indicator, and runs the write after
   drawing). Clear `saving` when done; show "saved · <path>" in the status line for ~2 s.
2. **Proper:** encode the `.ora` (or project) into bytes on the UI thread (fast: it is already in
   memory), hand the bytes to a `std::thread` that writes and fsyncs, and keep `app.saving` set until the
   thread reports back through a channel polled in `ui::build`. Writes never overlap: a second save while
   one is in flight queues behind it. Errors surface in the status line. Do NOT move the encode off-thread
   (it reads live layers).
The indicator: a small pill at the top-right of the canvas ("Saving…" with a spinner) plus the document
tab's title suffix; never a modal (a modal steals the pen mid-stroke).

### L. "The balloon pen should show the expected final thickness and brush colour while drawing"
**Cause:** `ui/overlay/frames.rs::balloon`, `BalloonMode::Draw` arm: the live trail is a 1.5 px
accent-coloured `Shape::line`, regardless of the width the balloon will be rasterized with.
**Change:** draw the trail as the outline the commit will produce: for each recorded point use the same
per-anchor pressure width the balloon commit records (find where the Draw commit builds its `Balloon`
from `balloon_drag`: `canvas_input.rs` ~3722 and `preview()` ~525; reuse that function so preview and
commit cannot drift), scaled by the tool's `width_scale`, in the tool's `line_color` at `line_opacity`,
converted to screen px through the view zoom. Emit it as one `egui::Mesh` ribbon (a quad per segment,
widths interpolated) rather than per-segment lines so joins do not flicker. Keep a 1 px accent
centre-line on top so the trail is visible over black ink. Tail and Ellipse/Round previews: same
treatment if it is a one-line change, otherwise leave.
**Tests:** a headless test that the preview builder and the commit builder produce the same anchor widths
for one recorded drag (`the_balloon_pen_preview_matches_its_commit`).

### M, N, O. Balloon pen: corners not sharp, one kind of control point, uniform thickness
Owner (with a screenshot of a drawn balloon whose intended spikes came out rounded): "the articulation
points that decide corners and sharp parts for bubbles are not sharp enough; I don't see 2 types of grip
points like CSP, circles for curvy ones and squares for the sharp corners (google it, EN and JP); the
balloon pen seems uni-thickness which is no fun."

**Facts on the page.** `core/src/balloon.rs` `BalloonShape::Drawn { points, widths, corners }` already
carries a per-anchor pressure `widths` and a per-anchor `corners: Vec<bool>` (CSP's corner-anchor flag),
tessellated by `tessellate_closed`. `BalloonSet.pressure_width: bool` gates the pressure modulation.
`canvas_input.rs` ~3722 records `[x, y, pressure]` per pen sample during the Draw drag; the release
fits those into anchors (find the fit: grep `BalloonMode::Draw` / `corners` in `canvas_input.rs` and
`cmd/`). So all three complaints are tuning and UI, not new data.

**M. Corners.** Find where `corners[i]` is decided at release. Owner's screenshot: a hand-drawn spike
(direction change well over 90° within a few px) came out as a rounded bump. Change: detect corners on
the RAW sample trail before simplification (turning angle over a short window, e.g. the angle between
the incoming and outgoing 6 px chords > 55° = corner; tune against the screenshot's shape), keep the
corner sample as an anchor with `corners[i] = true`, and make `tessellate_closed` honour a corner as a
true kink (the spline breaks tangent continuity there: both adjacent segments end exactly at the anchor
with no rounding). Print the turning-angle threshold as a constant with a comment. Test: a synthetic
star-shaped trail (10 spikes) produces 10 corner anchors and the tessellated outline passes within 0.5 px
of each spike tip.

**N. Two kinds of control points.** Research first (web, EN + JP, 10 minutes max, cite the URL in the
report): how CSP draws balloon/vector control points for corner vs curve anchors (CSP terms: 制御点,
角 / 曲線; the Object tool's 「制御点の切り替え」). Then: in the Object tool's balloon handle drawing
(find where `BalloonHandle` handles are painted, `ui/overlay/*`), draw corner anchors as SQUARES and
smooth anchors as CIRCLES, same size as today's handles. Add the toggle CSP has: with the Object tool,
Alt+click (or whatever CSP uses, per your research) on an anchor flips `corners[i]`, undoable through the
existing balloon edit command path. Document in `docs/manual/text.html`.

**O. Thickness.** Check `BalloonSet.pressure_width` default and the modulation curve in the rasterizer
(`sdf_w`, `pressure_width`). Owner says the result is uniform. Likely causes, check in order: (1) the
default is off; (2) the modulation maps pressure 0..1 to a width range too narrow to see (e.g. 0.8..1.0
of `width`); (3) the pen samples' pressure is not reaching `widths` at release (dense batches averaged
to ~0.5). Fix the actual cause; the target feel is CSP's balloon pen: width follows pressure over the
full brush-size range with the tool's min-size setting, like the G-pen does. Expose "Min size %" in the
balloon Tool Property if it is not there. The live preview (item L) must show the same widths.

Files for M/N/O: `crates/core/src/balloon.rs`, the balloon parts of `crates/app/src/app/canvas_input.rs`
(Lane 2 owns the RULER parts of the same file; do not touch those regions), the balloon handle overlay
in `crates/app/src/ui/overlay/`, the balloon Tool Property panel, `docs/manual/text.html`. Tests in
`crates/core` next to the balloon tests, plus one app-level test for the corner toggle.

---

## LANE 6 — A balloon drawn over text joins the text's layer (item P) — STRUCTURAL, runs LAST

Owner: "I drew a balloon with the balloon pen over text but it just added the balloon as a new layer
above the text layer instead of combining them: visually in the layer list, and functionally with the
K move tool. They should still be separately selectable via the Text tool or the Object tool."

**Facts.** `core/src/doc.rs` has two vector kinds, `LayerKind::Balloon(BalloonSet)` and
`LayerKind::Text(TextSet)`. `cmd/text.rs::run` `AppCmd::BalloonAdd` puts a new balloon on the active
balloon layer, else the topmost visible unlocked balloon layer, else a fresh balloon layer (surface pass
2026-09-02). It never looks at text layers. CSP's model: a text layer holds BOTH its text and its
balloon; drawing a balloon over text adds the balloon to that text's layer, and the layer move tool moves
both. `canvas_input.rs::fit_balloon_to_text` and commit 1f5f0cf ("Text clicked inside a balloon gets a
box wrapped to that bubble") already relate the two across layers.

**Approach (recommended, the least structural change that gives CSP's behaviour):** let a TEXT layer
carry balloons. `TextSet` gains `#[serde(default)] balloons: BalloonSet` (old files load unchanged; a
`Balloon` layer stays a valid kind for balloons with no text). Then:
1. **BalloonAdd targeting.** If the new balloon's body (its `sdf` ≤ 0) contains the anchor of a text on
   a visible, unlocked text layer, the balloon goes into THAT layer's `TextSet.balloons` and the layer
   becomes active. Otherwise today's rule. Same in reverse: a text placed inside a balloon that lives on
   a balloon layer moves that balloon onto the text's layer? NO: simpler and CSP-true, the new TEXT goes
   onto the balloon's layer, which means a Balloon layer must also be able to hold texts. So the honest
   version of this step is: **one vector kind `Speech { texts, balloons }`** replacing both `Text` and
   `Balloon`, with `is_text()` / `is_balloon()` answering by content, and the `.ora` loader mapping the two
   old kinds into it. Do it that way if the crate's `set_texts` / `set_balloons` / undo / rasterize paths
   can be unified in one pass with the tests green; otherwise ship the `TextSet.balloons` half first and
   report the remaining half. Fable decides after reading the report's size estimate.
2. **Rendering.** The layer rasterizes its balloons, then its texts on top (today's two layers, one pass).
3. **Layers palette.** One row; the thumbnail shows both; the kind glyph is the text glyph with a small
   balloon mark when the layer carries balloons.
4. **Move / transform** on the layer (K, Ctrl+T, the layer-move drag) translates both sets
   (`Document` ~1247 already translates vectors per kind; extend to both fields).
5. **Selection stays separate.** Object tool: click picks a balloon or a text on the same layer as today
   (`balloon_sel` / `text_sel` are `(layer, idx)` already). Text tool: click edits the text. Balloon
   tail drags, tone, handles: unchanged, keyed by the layer they live on.
6. **MCP + palette commands** (`balloons_add`, `texts_add`, `layers_list` in `crates/mcp` and
   `cmd/text.rs`) keep working with a layer index of either kind; add the new field to whatever
   `doc_info`/`layers_list` report.
7. **Tests:** `text_and_balloon_item_ids_mint_and_stay_stable` still passes; new: a balloon drawn around
   a text lands on the text's layer; moving that layer moves both; Object tool can still pick each; an
   old `.ora` with separate text and balloon layers loads with both layers intact (no auto-merge on load).

**Size:** the largest item in this plan (core kind, .ora, undo, rasterize, palette, two tools, MCP).
Estimate a day of agent time. Runs after Lanes 1–5 are merged; its own lane, no file sharing.

**K, addendum (owner, later the same day): "I'm getting 'not responding' all the time on saves and
other basic actions."** That is Windows marking the window after ~5 s without the message pump
running. So option 2 (write on a thread) is REQUIRED for K, not optional, and the report must list
every other command that blocks the pump for more than ~1 s (grep the save/export/print/import paths in
`cmd/file_io.rs` and time them in a headless test on a 3-page work). Anything over 1 s gets the same
treatment or a progress pill. Do not add a modal.
