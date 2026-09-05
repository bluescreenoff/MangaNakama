# Lane 3 report — frame folders: lag, list order, blue veil, layer click

## Done / next
- [x] **D** — the veil is ONE mesh, not a rect per screen row. Hypothesis VERIFIED with a
      shape count before rewriting (numbers below). `ui/overlay/page.rs::veil_mesh`.
- [x] **E** — a divide puts the folder you read FIRST on the higher row.
      `core/doc.rs::divide_frame_folder{,_dup}` take `above: bool`;
      `cmd/frames.rs::reads_earlier` decides it.
- [x] **F** — the veil shows for any layer INSIDE a frame folder
      (`ui/overlay/page.rs::veil_folder`). Landed after D, as the plan required.
- [x] **G** — a plain click collapses a multi selection (`ui/layers/rows.rs`).
- [x] Gate: `cargo check --workspace --all-targets` — **zero errors, zero warnings**.
      All targeted tests pass (list at the bottom).
- **next:** nothing. Not committed, per the brief — Fable reviews and commits.

## Item D — the veil really WAS the cost (measured, not assumed)

Scene: 600x400 canvas, two panels, one SLANTED cut (a straight cut would make every
scanline identical and flatter the old code).

| | egui shapes pushed per frame |
|---|---|
| **before** (one `rect_filled` per screen row) | **800** |
| **after** (one `egui::Mesh`) | **1** shape / 26 triangles |

800 = 400 visible rows x the two gaps beside the panels. It was rebuilt and re-pushed
EVERY frame for as long as a frame folder was the active layer — exactly the state the
owner was in for both lags (right after a divide; while dragging a folder in the list).
On his real window (~1000 pt tall) that is ~2000–3000 shapes a frame, more with more
panels. Item F makes that state much more common, which is why D had to land first.
No profiling of `FrameDivide` / `derive_frame_raster` was needed — the hypothesis held.

How the mesh works: cut the visible area into horizontal bands at every polygon vertex's
`y`. Inside a band no vertex exists, so every polygon edge crossing it is one straight
segment, and the even-odd spans at the band's top and bottom pair up into trapezoids (two
triangles each). O(vertices x panels) instead of O(rows). The two ends are sorted
independently, which is the same pairing as following each edge while panels do not
overlap, and degrades gracefully if two ever cross inside a band.

Correctness is pinned, not assumed: `the_mesh_veil_covers_exactly_what_the_scanlines_did`
samples a 60x40 grid over an L-shaped (CONCAVE) panel plus a plain one and asserts the mesh
covers a point exactly when the even-odd rule says it is outside the panels.

Bonus: the old fill started at `ceil(top)` and stepped 1 px, so it left a sub-pixel sliver
at the top of the canvas. The mesh covers the area exactly.

## Item E — how the side is decided

`core/doc.rs` now only PLACES the block: `above` means insert at `index + 1` (one past the
header = above the original's whole block, since children sit BELOW their header);
otherwise `children_range(index).start`, as before.

Reading order needs the binding side, which core does not have, so the app decides:
`cmd/frames.rs::reads_earlier(a, b, rtl)` — rows top to bottom first, right to left inside
a row for a manga binding; the axis that decides is the one the two halves' centres are
furthest apart on, so a slanted cut still gets a deterministic answer. `FrameDivide` now
tracks each half's OWN bounding box (`kept_union` / `off_union`) instead of reusing
`cut_union`, which spans both halves, so untouched panels in the same folder cannot skew
the comparison.

Divide-time placement only — nothing reorders existing folders on load or on rename.
The undo record is unchanged, and the test asserts undo restores the exact pre-divide
stack (name, depth and folder flag of every layer).

## Item F

The condition was `app.doc.active_layer().is_frame()`. It is now
`veil_folder(&app.doc, app.doc.active)`, which walks OUT of the active layer to the frame
folder containing it (the header itself, a draw layer inside it, or a layer in a
sub-folder inside it) and returns `None` for a layer in no frame folder — so the veil
still lifts when you leave the koma. The veil is drawn for THAT folder's frames.

`veil_folder` is a six-line copy of `ui::layers::rows::active_frame_folder`. It is not a
call: both are private to their own module, and `cmd::frames::enclosing_folder` (what the
plan suggested) is `pub(super)` inside `cmd`, so `ui::overlay::page` cannot see it either.
Six lines beat three visibility changes across files this lane does not own.

## Item G — half of it was already true

`Document::set_active` has ALWAYS cleared `layer_multi`, and its doc comment already said
"a plain selection collapses the palette multi-selection". So `AppCmd::SelectLayer` was
never the bug and needed no change. The bug was entirely in `ui/layers/rows.rs`: the
plain-click arm pushed `SelectLayer(i)` only `if !selected`, so clicking the row that was
already the editing target pushed no command at all and the Ctrl-built selection survived.
The condition is now `if !selected || !app.doc.layer_multi.is_empty()`.

Checked every other `SelectLayer` caller (`app.rs::pick_layer_at`, the row menu's
`select_first`, the mask cell in `rows.rs`, the Ctrl+K palette in `ui/quick.rs`, and the
tests): all of them mean "make this the one layer". Nothing relies on multi surviving.

## Files needed but did not own

- `crates/core/src/doc.rs` **tests** — adding `above` forced two call sites inside
  `doc::tests::divide_frame_folder_spawns_a_sibling_folder_with_children` to be updated.
  Both pass `false`, which is the pre-existing placement, so no assertion changed. Lane 3
  owns `doc.rs` for the two divide functions only; this is the same change's tail.
- `crates/app/src/ui.rs` and `ui/overlay.rs` — **NOT edited**. `mod overlay` and `mod page`
  are private, so `app/tests.rs` cannot name `veil_mesh`. The three veil tests therefore
  live in `page.rs`'s own `mod tests` rather than `app/tests.rs`. Same crate, same
  `cargo test -p mn-app` run, so the gate is unchanged.
- `crates/app/src/ui/icons/svg.rs` — another lane's new file. It broke the `mn-app` build
  for ~20 minutes (`p.ctx().style()` does not exist in egui 0.36). NOT touched; waited it
  out, then the gate ran clean.

## Note for Fable (not mine to act on)

Commit `22eca79` (Lane 4) says the two-finger test was strengthened, but
`crates/app/src/app/tests.rs` is **not in that commit's file list** — Lane 4's hunks around
lines 7021–7099 are still unstaged in the shared working tree, sitting beside Lane 3's
appended block at the end of the same file. Worth splitting when you stage Lane 3.

## Tests

New, in `crates/app/src/ui/overlay/page.rs` (`mod tests`):
- `ui::overlay::page::tests::the_frame_veil_is_one_mesh_not_a_rect_per_row` — PASS
- `ui::overlay::page::tests::the_mesh_veil_covers_exactly_what_the_scanlines_did` — PASS
- `ui::overlay::page::tests::the_veil_shows_for_a_layer_inside_the_frame_folder` — PASS

New, appended at the END of `crates/app/src/app/tests.rs` (nothing before line 11347
touched, nothing reformatted):
- `app::tests::a_divide_lists_the_earlier_reading_panel_higher` — PASS
  (level cut: the TOP panel's folder is the higher row and badges 1; undo restores the
  stack; vertical cut: RTL, the RIGHT panel's folder is the higher row and badges 1.
  Covers both `divide_frame_folder` via `CreateEmpty` and `divide_frame_folder_dup` via
  the default `Duplicate`.)
- `app::tests::a_plain_click_collapses_a_multi_selection` — PASS

Existing, re-run and still passing:
- `mn-core`: `doc::tests::divide_frame_folder_spawns_a_sibling_folder_with_children`
- `mn-app`: `app::basics_qa_panels_tests::qa_dividing_a_panel_leaves_the_gutter_the_tool_property_asks_for`,
  `app::basics_qa_panels_tests::qa_a_second_cut_crosses_the_first_into_four_panels`,
  `app::tone_round_tests::divide_contents_decides_what_the_new_half_gets`,
  `app::tone_round_tests::divide_equally_lays_the_grid_in_one_command`,
  `app::tone_round_tests::tapping_a_panel_edge_extends_it_and_dragging_still_divides`,
  `app::surface_e2e_tests::e2e_a_short_manga_walks_the_whole_pipeline`,
  `app::frames::tests::frames_renumber_by_reading_order_not_creation`

## What the owner should look at (eye test, does not block)

1. Divide a panel top/bottom — the TOP half's folder should be the higher row, badge 1.
2. Divide left/right — the RIGHT half's folder should be the higher row, badge 1.
3. Click "Layer 1" inside a frame folder — the blue veil should stay on.
4. Divide, and drag frame folders around the list — the stutter should be gone.
5. Ctrl-click three layers, then plain-click one — only that one stays lit.

## Docs

`docs/manual/layers.html`: the plain-click sentence now says "any row... including the row
that holds the pen", plus two new quirk boxes — the veil following the folder rather than
the header row, and divides placing the earlier-reading panel higher.
