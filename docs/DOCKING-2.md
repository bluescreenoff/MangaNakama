# Docking 2 — the canvas is a pane

Decision (owner, 2026-08-21): the whole window becomes ONE dock tree and the
canvas is a tab/pane inside it, like any palette. The target layout the owner
sketched, three pages of one manga open at once:

    page 1 | tools / tool options | page 2 | layers list | page 3

Every open page is its own pane, interleaved freely with palette columns.

## What exists today (the starting point)

- Two `DockState<Palette>` columns (`App::dock_left` / `dock_right`), each a
  fixed-width side panel; floating palettes are extra surfaces of whichever
  column they were torn from (ui/dock.rs).
- The canvas is NOT a widget: it is the rect the panels leave free
  (`ui.rs` → `Shell::set_canvas_rect_points`), rendered by the wgpu
  compositor UNDER egui; `Shell::owns_pointer` routes pen input by position.
- Documents: `App::docs` is `Vec<Option<DocSession>>` in tab order, active
  slot `None` (its fields live inline on App); switching parks one session
  and installs another, resetting every layer-index-keyed cache
  (app/session.rs). One tab = one WORK; pages inside a work switch via
  `page_index`, with per-page preview textures keyed by `PageEntry::uid`.
- The doc tab strip is hand-drawn in ui/top.rs (`doc_tab`).
- Persistence: `ui.txt` `dock_left=` / `dock_right=` JSON + column widths +
  collapse flags; workspaces store the same per entry (variable-length,
  index-guarded).

## Core design

### One tree, two pane classes

```rust
enum Pane {
    Palette(Palette),               // exactly the 16 existing palettes
    Canvas { doc: u64, page: Option<u64> },  // runtime uids, see below
}
```

One `DockState<Pane>` (`App::dock`) fills everything between the top bar and
the status bar. Palette panes behave exactly as before. Canvas panes:

- **The live pane.** Exactly one canvas pane is LIVE: the one bound to the
  active doc slot + current `page_index`. Its body paints NOTHING (the wgpu
  canvas shows through the hole) and reports its rect via
  `set_canvas_rect_points`. Pen routing, zoom anchoring, the overlay and the
  selection launcher all keep working unchanged — they already key off that
  rect.
- **Inactive canvas panes** (phase 2) draw their page's preview texture
  scaled to fit, dimmed slightly, with the page label. A click ACTIVATES the
  pane: `switch_doc` and/or page-switch moves the live viewport there. One
  click activates, it never draws — clicking into an inactive document must
  not paint a dot (the owns_pointer principle, one level up).
  Rendering N live GPU viewports is deliberately out: one live document is a
  load-bearing invariant (parked pages are bytes, caches are per-document,
  and the target machine has an iGPU). Previews are honest and cheap; a
  higher-resolution parked render can upgrade them later without changing
  the model.

### Pane identity and reconciliation

`doc`/`page` are RUNTIME uids (a monotonic counter on App for doc slots; the
existing `PageEntry::uid` for pages), never slot indices — closing tab 0 must
not re-aim every canvas pane. `page: None` means "follows the work's current
page" (the default pane every document gets).

Serialization writes uids as ORDINALS (doc = tab order position, page = page
index) because uids do not survive a restart. On load, reconcile:

1. Collect canvas panes in tree order; bind them to open doc slots in order.
2. A pane whose ordinal has no doc/page any more is REMOVED from the tree.
3. A doc with no pane left gets one appended to the first canvas leaf
   (created center-root if none exists).

Deterministic, no orphans, and the invariant "every open doc has at least one
canvas pane; at least one canvas pane exists" holds after every load, close,
and drag — enforced in one `reconcile_canvas_panes` called from those three
places, not scattered.

### Never buriable

- Canvas panes: `closeable` = the doc-close flow (unsaved-changes prompt;
  the LAST canvas pane refuses to close — the app always shows a canvas).
  A page-view pane (phase 2, `page: Some`) closes freely; it is just a view.
- Canvas leaves accept only canvas tabs; palette leaves accept only palette
  tabs (vendored drop filter). Tabbing Layers OVER the canvas would bury it;
  mixing classes also makes the phase-2 "canvas leaf = the doc strip"
  reading incoherent. Splitting against either class is unrestricted — that
  is the owner's column layout.

### The doc strip dissolves (phase 2)

All canvas panes tabbed together in one center leaf ARE the document tab
strip — egui_dock's tab bar shows label + dirty dot + close, exactly what
`doc_tab` hand-draws today. Clicking a canvas tab = `switch_doc`. Dragging
one out into its own leaf = the owner's side-by-side layout. `doc_tab` and
its top panel are deleted. "Open page N in a pane" lands in the Pages
palette context menu (and later: drag a page thumbnail into the tree).

### Side collapse → edge flap (owner sketch, part 15)

In a free tree "left column" is defined as: at the root-most horizontal
split whose one child contains the live canvas pane and the other does not,
the non-canvas child on that side. Collapse stores the split fraction and
sets it to ~0; expand restores it. No restructuring, so nothing is lost.
UI per the owner's sketch: expanded → a small chevron in that side's own
header row (zero extra space); collapsed → NO strip, just a ~24×48px
rounded flap hugging the window edge near the top corner, floating over the
canvas, subtle at idle, full opacity on hover, click = expand. If the tree
has no such split (canvas mixed to both edges), the flap simply is not
offered on that side.

### Persistence and migration

- New key `dock_tree=` (one JSON line) + `dock_flap=` for stored collapse
  fractions. Old keys `dock_left=`/`dock_right=`/`left_w`/`right_w`/
  `*_collapsed` stay READABLE forever (ui.txt keys are shipped API): when
  `dock_tree=` is absent, MIGRATE — build root = [left subtree | canvas |
  right subtree], fractions from the stored widths against the window width.
  Column-internal splits migrate by walking the old tree's leaves top-to-
  bottom and rebuilding with `split_below` at the recorded fractions;
  floating surfaces copy across verbatim. Anything unreadable falls back to
  the default tree (a stale ui.txt must never wedge startup).
- The 6 saved workspaces migrate the same way, LAZILY at apply time (the
  entries are already variable-length and index-guarded; a new field carries
  the tree, absent = migrate from the old fields in the entry).
- Default tree = today's default columns around a center canvas leaf.

### Input gating (phase 2 guardrail)

Canvas shortcuts (space-pan, brackets, Tab, zoom keys) currently gate on
"egui does not want the keyboard". With multiple canvas panes visible that
stays correct — they all aim at the LIVE pane, and there is exactly one.
What must gate per-pane is the POINTER: only the live pane's rect is canvas
for `owns_pointer`; inactive canvas panes are ordinary egui widgets (their
click-to-activate is an egui button in the pane body).

### Vendored egui_dock patches needed (PATCHES.md entries)

- #15: per-tab body fill override (`TabViewer::tab_body_fill(&Tab) ->
  Option<Color32>`) — the live canvas pane's body must be TRANSPARENT so the
  wgpu canvas shows through.
- #16: per-leaf drop filter by tab class (canvas leaves take only canvas
  tabs, palette leaves only palettes). Builds on patch #4's foreign-hover
  machinery.
- (already shipped: #14 float-window drag, #1–7b.)

## Phases

Each phase ships pushed, suite green, on its own.

1. **One tree.** `DockState<Pane>` replaces both columns; ONE canvas pane
   (`page: None`, follows active doc); doc strip stays (drawn INSIDE the
   canvas pane body, above the hole). Migration + workspaces + Tab/Shift+Tab
   behavior + Workspace menu reopen/reset updated. Vendor patches #15/#16.
   Known limit from part 14 (collapsing hides torn-off floats) dies here:
   floats are surfaces of the ONE tree, nothing hides them.
2. **Panes are pages.** Canvas panes per doc + per page, preview rendering
   for inactive panes, click-to-activate, doc strip deleted, Pages palette
   "open in pane", close flows. Side collapse → edge flap.
3. **Polish round** after owner eye test: drag a page thumbnail into the
   tree, preview resolution upgrade for large inactive panes, per-pane view
   state (remember zoom/pan per pane) if the owner asks.

## Traps to carry forward

- Two sibling DockAreas clobbering widget ids goes away (one area), but the
  DockArea id must stay stable across frames.
- `forget_document_caches` runs on BOTH sides of every activation — the
  click-to-activate path must go through `switch_doc`, never install a
  session by hand.
- A collapsed side's stored fraction must never be persisted as the live
  fraction (the `note_widths` strip-width rule, reborn).
- egui_dock 0.21's `Style::default()` is LIGHT — always derive from
  `Style::from_egui` (the round "some parts are white now").
- The reader still replaces the whole UI while open; it bypasses the tree
  entirely, unchanged.
