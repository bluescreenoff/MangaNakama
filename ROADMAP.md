# MangaNakama — roadmap

This is the public priority queue: what the app is, what already
works, what is next, what will never be built, and where a newcomer can start.
It is deliberately plain — no internal shorthand, no ticket numbers.

## What this is

A free, GPU-first drawing app for **making manga** on Windows: inking, panels,
screentone, balloons, Japanese text, multi-page chapters, and a built-in reader
so you can read your own chapter the way a reader will. Local files, no cloud,
no account, no subscription. Licensed MIT OR Apache-2.0.

The rule the project builds to: **everything that can run on the graphics card
runs on the graphics card.** That is a direction, not a finished claim — see
"next" below for the parts still on the CPU.

## What works today

**Drawing.** A libmypaint-based brush engine with hard-stamp tips, scatter,
wash (flow separated from opacity), texture tips, a sketch/hatching engine,
mirror symmetry and wrap-around tiling. Pen pressure comes from raw Win32
pointer messages — the app owns its tablet path rather than delegating it.
Stroke stabilization, input resampling, a per-sensor curve editor, and brush
import from four ecosystems — Photoshop `.abr` (dynamics translated as far as
they honestly map, tips stamped and rotating per dab), GIMP `.gbr`/`.gih`,
Krita `.kpp` (dynamics), and Clip Studio `.sut` (your own sub tools — read
directly, no export ceremony beyond CSP's own).

**Canvas and layers.** A tiled canvas at print resolution with a GPU
compositor (layer blending, group flattening, tile upload, display) and a
software-adapter fallback that is the *same* code path, not a second
implementation. Fifteen blend modes with the CPU and GPU results pinned equal
by tests. Layer masks, layer colour tint, reference layers, layer comps,
selections (rectangle, lasso, wand, brush-painted selection pen/eraser, quick
mask), transform and flip, and undo throughout. Layers multi-select in the
palette (Ctrl+click toggles, Shift+click ranges), and tonal correction —
levels, tone curve, brightness/contrast, hue/saturation, posterize, invert,
binarize — applies across every selected layer as a single dialog and a
single undo step. With a selection up, paste lands already masked to it:
a paste that stamps the active layer clamps to the ants (feather and all),
and a paste that arrives as its own layer wears the selection as a layer
mask you can remove. A layer above a sealed folder clips to the group's
combined ink, and clipping survives structure edits — the palette greys a
clip flag that lost its base and the status line says so.

**Manga.** Frame folders and panel division with automatic reading-order
numbering plus an on-canvas reading-path overlay; balloons with editable
splines and tails that can fit themselves to their lettering; screentone and
live tone/gradient fill layers; a material bank (tones, speed lines, focus
lines) with tiling and taggable search; a Pattern Studio — draw on a
wrap-around tile, watch it repeat live, save it as a material in one click;
perspective rulers with one, two or three vanishing points, all movable after
creation; text with vertical Japanese, per-range styling, furigana and mixed
fonts; a story editor; multi-page work folders with a page manager and
scalable page previews; print preflight; and a chapter reader with
right-to-left spreads, fullscreen, and an edit-and-return round trip.

**Files.** OpenRaster (`.ora`) as the native layered format, a single-file
`.mnc` for a whole comic, full-resolution PNG export per page or for the whole
chapter, layered PSD export for the studio hand-off (groups, all 27 blend
modes, clipping, Japanese layer names), crash recovery and autosave.

**Infrastructure.** 500+ tests. GPU tests run against the software adapter when
no hardware one is present, and skip rather than fail when there is no adapter
at all. A release workflow builds a zip that a tester can unzip and run.

## What is being built next

Roughly in order. None of this is promised by a date.

1. **GPU dabs, the last exclusions.** Wash, texture and smudge brushes ride
   the GPU path now, a benchmark harness exists (`--bench-dabs`), and the
   path turns itself on per machine from a measured verdict — a one-shot
   background benchmark decides, never a blanket default. Still CPU-only:
   smudge combined with wash (the sampler would have to read the in-flight
   wash buffer) and the spectral-paint / colorize / posterize presets.
2. **Recordable action sequences.** CSP Auto-Action parity: record a
   sequence of layer-management steps (new layer/folder/frame folder,
   rename, border effect, layer colour, tone, blur, …), store it, replay
   it as one undo step — a tab beside the Layers palette. The batch
   dialog covers the flat cases; recorded sequences are the real ask.
3. **A small scripting surface.** The batch half shipped (Layer > Batch
   operations: rename/renumber by pattern, apply/clear tone as one undo
   step, export a PNG per layer — scoped by all/folder/name-prefix).
   What remains is the scripting surface proper, if real use ever asks
   for more than the dialog covers.
4. **HDR / linear-light colour.**
5. **The manual, kept honest.** Static HTML beside the executable exists;
   its job is the quirks — the interlocks you would otherwise discover by
   having something silently do nothing — and it grows with every round.

## Explicitly out of scope

These are settled, not open questions. Please do not open PRs for them.

- **Animation**, 3D models and posing, generative/AI colouring, and a
  general filter grab-bag. This is a manga app, not a suite.
- **Mobile and tablet operating systems.** Windows only. The pen path is raw
  Win32 by design and is the thing that makes the app feel right.
- **Cloud anything.** Local files.
- **Writing `.clip` / `.cmc`.** No writer for either format exists in the open;
  only partial experimental readers. Layered PSD (above) is the practical
  hand-off instead.
- **Book/PDF batch export.** Deliberately dropped as scope creep.

## Good first issues

None open right now: the list of nine shipped, every one, and so did the
tenth (undo for ruler creation and moves — rulers moved onto the `Document`
so the one undo history owns them). New ones are noted here as the work
that finds them lands; each comes with the written reason it was deferred,
and asking in an issue gets you that reason with the answer.

## Picking something up

- Windows only. Follow `SETUP.md`, then `./build.sh`; run `./build.sh --test`
  before opening a PR and keep it green with zero warnings.
- Read `docs/ARCHITECTURE.md` (especially "Traps") first. Deferred items
  have a reason on record; that reason is usually the actual work.
- The CPU path is the reference and the GPU path is the destination: pixel
  features land with both, agreeing, and tests enforce it.
- Open an issue before starting anything large so nobody duplicates work.
