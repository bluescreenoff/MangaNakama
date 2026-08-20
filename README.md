# MangaNakama

A free, GPU-first manga drawing app for Windows: inking, panels, screentone,
balloons, vertical Japanese text, multi-page chapters, and a built-in reader
so you can read your own chapter the way a reader will. Local files, no
cloud, no account. MIT OR Apache-2.0. Early development, moving fast —
[`ROADMAP.md`](ROADMAP.md) is what works today and what comes next.

*(Shameless plug: the dev draws too —
**[read my Webtoon here](https://mangadex.org/title/6a08b268-e032-4198-a53a-ef705e592dc3/tekno)**.
This app exists so making it hurts less.)*

## Why this exists

An open alternative for mangaka who can't justify Clip Studio PAINT's price
tag — or who are tired of a CPU-bound pipeline that makes big brushes, tone
layers and selected-area rotation feel like the 2000s. **Everything that can
run on the GPU, runs on the GPU** is the rule this app is built to — the
"More" fold at the bottom continues this in detail.

## Download

Grab the zip from the [Releases](../../releases) page, unzip it anywhere, run
`manganakama.exe` — keep `assets/` and `manual/` beside it. Windows 10/11,
64-bit; no installer, nothing touches the registry, deleting the folder is
the uninstall. If something breaks, open an issue and attach
`manganakama.log` from beside the exe: it names the build, your GPU, and any
crash, and it is deliberately free of anything personal, so it is safe to
post. (Attach that one file — its neighbour `recent.txt` holds full paths
with your Windows user name in them.)

## Build & contribute

Windows only. `git clone`, follow [`SETUP.md`](SETUP.md) (Rust GNU toolchain
plus one portable GCC — no Visual Studio), then:

```
./build.sh            # debug build
./build.sh --release  # the build you draw with
./build.sh --test     # the full suite — green with zero warnings before a PR
```

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) (TL;DR in its first line) and
`docs/ARCHITECTURE.md` — especially its **"Traps"** section — before
changing anything. `ROADMAP.md` ends with a list of good first issues.

---

<details>
<summary><b>More: what's under the hood, and the tester's guide</b></summary>

### Why this exists, in more detail

The GPU rule is a direction, not a finished claim. On the GPU today: the
whole compositor (layer blending, group flattening, tile upload, display)
plus brush dab rasterization behind `--gpu-dabs`. Gradients, transforms and
filters are still CPU; they are the queue.

Inking runs on a libmypaint engine with Krita-inspired modes (hard-stamp
tips, scatter, wash/flow-vs-opacity, texture tips, symmetry, wrap-around
tiling, a sketch/hatching engine). Manga structure is CSP-style: panels and
frame folders with reading-order numbering, screentone and live tone/
gradient layers, balloons, a story editor, layer comps, multi-page work
folders, and the reader. The full test suite runs on every push
(`.github/workflows/ci.yml`, windows-latest, GPU tests on the WARP software
adapter). The pen path is raw Win32 pointer input — no winit; we own the
tablet path end to end.

### Tester's guide

You don't need to build — grab a release exe, draw, and report. The log
(`manganakama.log` beside the exe, or `%LOCALAPPDATA%\MangaNakama\` if the
folder is read-only — press **F1**, the Diagnostics window shows the real
path with a copy button) records the build, the graphics adapter, whether
strokes ran GPU or CPU, any `CANARY REPAIR` lines (automatic recovery when
a driver drops GPU work — pixels stay correct, the line is telemetry), and
a `!!! PANIC` line if the app ever dies. A session block that does not end
with `=== exited cleanly ===` is a crash — exactly what we want to see.
Reproduce the problem right before attaching so the relevant session is the
last block.

Heavy things worth hammering on:

1. **Rotation + zoom** — rotate the canvas, zoom deep in and out, draw
   through all of it; report lag spikes or vanishing strokes.
2. **GPU brush path** — run with `--gpu-dabs`, compare pen feel against the
   default; F1 shows the live dab path. A `gpu → cpu repair!` status is the
   canary defense firing — please attach the log.
3. **Software-renderer mode** — `--warp` forces the CPU-simulated GPU; any
   difference between normal and `--warp` runs is a driver bug worth an
   issue.
4. **Long, fast strokes** — pages of hatching, fast diagonal slashes,
   pressure ramps; look for lag, gaps, or seams at tile borders.
5. **Symmetry + tiling** — View ▸ Symmetry X/Y and Tile X/Y: draw across
   canvas edges with tiling on, mirror with X+Y active.
6. **Tone layers** — Layer ▸ Convert to tone layer; draw inside one, change
   frequency/angle, undo through it, save + reload.
7. **Transform + undo chains** — Ctrl+T, scale/rotate, Enter, then Ctrl+Z
   repeatedly; the canvas must restore exactly.
8. **Multi-page works** — create a comic work folder, switch pages while
   drawing, autosave, close + reopen; page files must survive.

Issue shape: what you did, what you saw, what you expected, the log file,
and your GPU (the log's first line has it).

### License

MIT OR Apache-2.0, at your option — see [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE). Vendored components keep their own
licenses (`vendor/libmypaint` is ISC); the full third-party inventory is
[`THIRD-PARTY.md`](THIRD-PARTY.md).

</details>
