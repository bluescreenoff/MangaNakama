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
Stroke stabilization, input resampling, a per-sensor curve editor, and import
of Photoshop `.abr` brush sets.

**Canvas and layers.** A tiled canvas at print resolution with a GPU
compositor (layer blending, group flattening, tile upload, display) and a
software-adapter fallback that is the *same* code path, not a second
implementation. Fifteen blend modes with the CPU and GPU results pinned equal
by tests. Layer masks, layer colour tint, reference layers, layer comps,
selections (rectangle, lasso, wand, brush-painted selection pen/eraser, quick
mask), transform and flip, and undo throughout.

**Manga.** Frame folders and panel division with automatic reading-order
numbering plus an on-canvas reading-path overlay; balloons with editable
splines and tails; screentone and live tone/gradient fill layers; a material
bank (tones, speed lines, focus lines) with tiling; text with vertical
Japanese, per-range styling, furigana and mixed fonts; a story editor;
multi-page work folders with a page manager and scalable page previews; print
preflight; and a chapter reader with right-to-left spreads, fullscreen, and an
edit-and-return round trip.

**Files.** OpenRaster (`.ora`) as the native layered format, a single-file
`.mnc` for a whole comic, full-resolution PNG export per page or for the whole
chapter, crash recovery and autosave.

**Infrastructure.** 500+ tests. GPU tests run against the software adapter when
no hardware one is present, and skip rather than fail when there is no adapter
at all. A release workflow builds a zip that a tester can unzip and run.

## What is being built next

Roughly in order. None of this is promised by a date.

1. **A Preferences dialog.** Several small settings currently have nowhere to
   live, so they are stuck behind this one missing window.
2. **GPU dab rasterization for the heavy brushes.** Dab cost scales with tip
   *area*, so big soft brushes are exactly where the graphics card should win —
   and today wash, texture and smudge brushes are excluded from the GPU path
   and fall back to the CPU. This also needs a benchmark harness: the GPU path
   should not become the default on anyone's machine without a measured number.
3. **Faithful brush imports.** Today `.abr` import keeps the sampled tip
   exact but deliberately resets the dynamics to a plain pressure brush.
   The goal is to close that gap as far as each format honestly allows:
   translate every dynamic that has an engine equivalent, grow the engine
   where a missing semantic is worth having natively (Photoshop's spacing
   and transfer behaviour, for instance), and extend import to more
   formats — GIMP `.gbr`/`.gih`, best-effort Clip Studio `.sut` (your own
   presets), and Krita `.kpp` for the engine features that exist here.
   Where a parameter cannot map, the import must say so instead of
   silently drawing differently. Includes a known wart: an imported tip's
   size is currently read from its padded bounding box, so extreme-aspect
   tips import oversized.
4. **Making a tiling pattern should be one gesture, not a ceremony.** The
   engine already tiles (wrap-around drawing, tiling materials); what is
   missing is the authoring path: draw something, see it tile live, save it
   as a material — without a register-this, crop-that, set-nine-options
   ritual. The benchmark is how many steps the equivalent takes in Clip
   Studio Paint (their own tutorial for it is a long numbered list); ours
   should be: draw, preview, name, done.
5. **Vector inking layers.** Strokes stored as editable geometry: control-point
   editing, width re-editing, and an eraser that trims a stroke at the
   intersection instead of deleting it.
6. **Layered PSD export.** Today's interchange is OpenRaster plus flat PNG,
   which is fine between open tools and not enough for a studio hand-off.
7. **Recordable actions, and a small scripting surface.** The real pain is
   batch operations over layers — rename, renumber, apply tone, export — not
   macro recording for its own sake.
8. **HDR / linear-light colour.**
9. **The manual, kept honest.** Static HTML beside the executable exists;
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

Each of these is real deferred work, deferred for a written reason — ask in
an issue before starting and the reason comes with the answer.

1. **Fit a balloon to its text.** The balloon and text models both exist; this
   needs a text-field target to size against.
2. **One-point and three-point perspective rulers.** The two-point ruler
   exists, and the "how many vanishing points" parameter pattern already exists
   on the symmetrical ruler — this is applying one to the other.
3. **Make rulers movable.** No ruler of any kind can be moved after you create
   it. A recorded gap for the whole ruler family.
4. **Absolute brush size per preset.** `[` and `]` step brush size through a
   ladder in real pixels, but the size slider is still a multiplier
   (0.25×–4×) on the preset's base size — so the slider's ceiling silently caps
   the ladder. Needs an absolute-pixel size field per sub-tool.
5. **Undo for mask strokes.** Painting on a layer mask bypasses the layer's
   operation recording, so it is not undoable. Medium: the work is the
   recording seam, not the brush.
6. **Undo for effect-line regeneration.** Regenerating replaces the layer's
   pixels wholesale, outside the undo bracket.
7. **Tags for materials.** The material bank has search but no tags; tags need
   a small per-folder sidecar file.

## Picking something up

- Windows only. Follow `SETUP.md`, then `./build.sh`; run `./build.sh --test`
  before opening a PR and keep it green with zero warnings.
- Read `docs/ARCHITECTURE.md` (especially "Traps") first. Deferred items
  have a reason on record; that reason is usually the actual work.
- The CPU path is the reference and the GPU path is the destination: pixel
  features land with both, agreeing, and tests enforce it.
- Open an issue before starting anything large so nobody duplicates work.
