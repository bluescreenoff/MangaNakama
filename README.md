# MangaNakama

A free, GPU-first manga drawing app for Windows — an open alternative for
mangaka who can't justify Clip Studio PAINT's price tag, or who are tired of
a CPU-bound pipeline that makes basic operations (selected-area rotation, big
brushes, tone layers) feel like the 2000s. CSP has spent decades adding
feature bloat without touching its graphics/video-card integration; we put
the GPU at the center of everything instead. **Everything that can run on the
GPU, runs on the GPU** is the rule we build to — and it is a direction, not a
finished claim. What is actually on the GPU today: the whole compositor
(layer blending, group flattening, tile upload, display), plus brush dab
rasterization behind the `--gpu-dabs` flag. Gradients, transforms and filters
still run on the CPU; they are the queue.

Current state: active early development (the full test suite runs on
every push by CI — `.github/workflows/ci.yml`, windows-latest with the
suite on WARP). Inking-ready with a libmypaint engine carrying
Krita-inspired modes (hard-stamp tips, scatter, wash/flow-vs-opacity,
texture tips, symmetry, wrap-around tiling, a sketch/hatching engine);
CSP-style panels and frame folders with reading-order numbering;
screentone and live tone/gradient layers; brush-painted selections
(selection pen/eraser, Quick Mask); balloons and a story editor; a
built-in chapter reader (read your own chapter the way a reader sees
it); layer comps; multi-page work folders; undo everywhere.
Contributions welcome — see `CONTRIBUTING.md` and the review workflow
below.

## Download (no toolchain, no build)

Grab the latest zip from this repository's **Releases** page, unzip it
anywhere, and run `manganakama.exe`. Keep `assets/` and `manual/` beside the
exe — it looks for its brushes and Help ▸ Manual right next to itself.
Windows 10/11, 64-bit; nothing to install, nothing touches the registry, and
deleting the folder is the whole uninstall.

Early builds, so: if something breaks, `manganakama.log` beside the exe
names the build and records the last crash — attach it to an issue with a
line about what you were drawing.

## Build

Windows only (Win32 + pen input, no winit — we own the tablet path).

```
git clone <repo>
cd MangaNakama
./build.sh            # debug build (prepends toolchain/w64devkit to PATH)
./build.sh --release  # pen-friendly build; copy to play/ to use
./build.sh --test     # the full suite
```

`toolchain/w64devkit` (not committed) provides the GCC toolchain — see
`SETUP.md` in the repo. Everything is plain `cargo` once it's on PATH via
`./build.sh`.

## Testers

You don't need to build — grab a release exe, draw, and report. The app
writes **`manganakama.log` next to the exe**: it records the build you are
running, your graphics adapter, whether strokes ran on the GPU or CPU, any
`CANARY REPAIR` lines (the automatic recovery when a driver drops GPU work —
pixels stay correct; the line is still useful telemetry), and — if the app
ever dies on you — a `!!! PANIC` line saying where. Each session block ends
with `=== exited cleanly ===`; a block **without** that line is a crash,
which is exactly what we want to see.

If the app's own folder is read-only (an install under Program Files), the
log goes to `%LOCALAPPDATA%\MangaNakama\` instead. **Press F1 — the
Diagnostics window shows the real path with a copy button.**

**Attach `manganakama.log` when you open an issue** — ideally reproduce the
problem right before attaching so the relevant session is the last block in
the file. A screenshot or short video of the misbehavior helps too.

> **Send that one file, not the folder.** The log is deliberately free of
> anything personal — no file paths, no document names, no user name — so it
> is safe to post publicly. Its neighbours are not: `recent.txt` holds the
> full paths of files you have opened (which include your Windows user name),
> and `ui.txt` holds your layout. Nobody needs those to fix a bug.

Heavy things worth hammering on:

1. **Rotation + zoom** — rotate the canvas (touch gesture / toolbar), zoom
   deep in and out, draw through all of it; report any lag spikes or
   vanishing strokes.
2. **GPU brush path** — run `manganakama.exe --gpu-dabs` and compare pen
   feel against the default; F1 shows the live dab path (gpu/cpu + readback
   ms). If the status ever reads `gpu → cpu repair!`, that's the canary
   defense: please attach the log.
3. **Software-renderer mode** — `--warp` forces the CPU-simulated GPU; if
   something looks different between normal and `--warp` runs, that's a
   driver bug worth an issue.
4. **Long, fast strokes** — pages of hatching, fast diagonal slashes,
   pressure ramps; look for lag, gaps, or seam artifacts at tile borders.
5. **Symmetry + tiling** — View ▸ Symmetry X/Y and Tile X/Y: draw across
   canvas edges with tiling on, mirror with X+Y active.
6. **Tone layers** — Layer ▸ Convert to tone layer; keep drawing inside one,
   change frequency/angle, undo through it, save + reload.
7. **Transform + undo chains** — Ctrl+T, scale/rotate, Enter, then Ctrl+Z
   repeatedly; the canvas must restore exactly.
8. **Multi-page works** — create a comic work folder, switch pages while
   drawing, autosave (15 min), close + reopen; check page files survive.

Issue template: what you did, what you saw, what you expected, the log file,
and your GPU (the log's first line has it).

## For contributors

- Read `docs/ARCHITECTURE.md` (especially "Traps") before changing
  anything — it records why the code is shaped this way.
- Windows-only, Git Bash for scripts; build via `./build.sh`, never bare
  `cargo` (linking needs the w64devkit PATH).
- The CPU path is the reference, the GPU path is the destination: pixel
  features land with both, agreeing (tests enforce it).
- Vendored code (`vendor/`) carries numbered patches documented in
  `vendor/PATCHES.md`.
- GPU tests verify on WARP when hardware misbehaves; the test suite skips,
  never fails, when no adapter exists.

## License

MIT OR Apache-2.0, at your option — see [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE). Vendored components keep their own
licenses (`vendor/libmypaint` is ISC; see `assets/brushes/Licenses.md`).
