# Third-party material in this repository

MangaNakama's own code and assets are **MIT OR Apache-2.0**, at your option
(`LICENSE-MIT` / `LICENSE-APACHE`). This file records everything that is
**not ours**: what it is, where it came from, and on what terms. Assets under
`assets/` have their own per-directory record in
[`assets/brushes/Licenses.md`](assets/brushes/Licenses.md).

Nothing listed here imposes copyleft on the project.

## Vendored source (in this tree, patched)

| Path | What | License | Notes |
|---|---|---|---|
| `vendor/libmypaint` | libmypaint v1.6.1 — the brush engine | **ISC**, © 2008–2011 Martin Renold (`vendor/libmypaint/COPYING`) | The MyPaint *application* is GPLv2+; this *library* is ISC. Our patches are logged in `vendor/PATCHES.md`. |
| `vendor/libmypaint/fastapprox/` | fast approximate math | Own permissive notice, © Paul Mineiro | Notice kept inside the vendored tree. |
| `vendor/egui_dock` | egui_dock 0.21 — the docking system | **MIT**, © 2022 Adam Gąsior (`vendor/egui_dock/LICENSE`) | Our patches are marked `MN-PATCH` and logged in `vendor/PATCHES.md`. |

## Vendored assets

| Path | What | License | Notes |
|---|---|---|---|
| `assets/brushes/classic/` | 35 MyPaint classic `.myb` presets | **CC0-1.0** (public domain), from `mypaint/mypaint-brushes` | `assets/brushes/COPYING` is the CC0 legal code, scoped to this directory. Details in `assets/brushes/Licenses.md`. |

## Crates.io dependencies (fetched at build time, not in this tree)

The direct dependencies — `wgpu`, `egui`, `egui-wgpu`, `image`, `zip`,
`serde`, `serde_json`, `quick-xml`, `rfd`, `bytemuck`, `pollster`,
`raw-window-handle`, `windows`, `windows-sys`, `windows-numerics`, `cc` —
are all published under the Rust ecosystem's standard permissive terms
(**MIT OR Apache-2.0**, or equivalently permissive; `rfd` is MIT). Their
license texts ship inside each crate as fetched by cargo. None impose
copyleft.

## Toolchain (never committed)

`toolchain/w64devkit` is downloaded per `SETUP.md`, is gitignored, and has
never been in history. w64devkit itself is ISC.

## Deliberately absent

- No CELSYS / CLIP STUDIO assets, code, or documentation ship in this
  repository.
- The ABR parser's real-file test fixture
  (`crates/brush/tests/data/abr_v6_sample.abr`) is third-party brush data:
  it is **local-only and gitignored** — the test skips silently where the
  file is absent, and a synthetic fixture pins the same container walk.
