# Licences — the files under `assets/`

MangaNakama's own work is **MIT OR Apache-2.0**, at your option — see
`LICENSE-MIT` and `LICENSE-APACHE` at the repository root. Absent a separate
dedication, that covers the assets this project made, the same as its code.

One directory here is **not ours**: `classic/`, which is third-party material
under CC0-1.0. This file says, per directory, what is in it, where it came
from, and on what terms. Third-party files that live *outside* `assets/` are
recorded in [`THIRD-PARTY.md`](../../THIRD-PARTY.md) at the repository root.

| Path | What | Origin | Terms |
|---|---|---|---|
| `classic/` | 35 `.myb` presets | `mypaint/mypaint-brushes` | **CC0-1.0** (third party) |
| `csp/` | 9 `.myb` presets | this project | project licence |
| `krita/` | 9 `.myb` presets | this project | project licence |
| `textures/` | 3 `.png` dab masks | this project | project licence |
| `../materials/` | 6 `.png` starter materials | this project | project licence |
| `COPYING` | CC0-1.0 legal code | shipped with `classic/` | applies to `classic/` **only** |

---

## `classic/` — 35 MyPaint presets, CC0-1.0, not ours

Vendored from **`mypaint/mypaint-brushes`** (upstream path `brushes/classic/`)
in commit `052cc90`, "Vendor MyPaint classic brush presets (35 .myb, CC0)".
Each file still carries its upstream `parent_brush_name` (`classic/pen`,
`classic/charcoal`, …), which is what identifies the set.

Upstream's machine-readable `Licenses.dep5` puts `brushes/*` under
**CC0-1.0**, © 2011–2013 Martin Renold and the MyPaint Development Team, with
the policy line *"By policy, MyPaint's brush settings are released into the
public domain."* CC0 requires nothing of us; the credit above is courtesy, and
courtesy is the entire point of writing it down.

The GPL-2+ stanza in that same upstream file covers the mypaint-brushes
repository's packaging and build scripts. **None of those are here** — only
`.myb` settings files came across.

## `COPYING` — CC0-1.0 legal code, scoped to `classic/`

This is the licence file that arrived with the classic presets, byte-for-byte
the Creative Commons CC0 1.0 Universal legal code (it is also upstream's own
`COPYING`). It sits at the `brushes/` root for historical reasons only.

**Read it as covering `classic/` and nothing else.** It is not a public-domain
dedication of this project's own presets, and no file in `csp/`, `krita/` or
`textures/` is offered under CC0 by its presence. Moving it to
`classic/COPYING` would make the tree say that without needing this paragraph;
that move is left as a publish-day action because the build is not ours to
disturb right now.

## `csp/` — 9 presets, this project's own files

CLIP STUDIO PAINT default sub-tools' *parameter values*, re-expressed as libmypaint settings for a different engine.
An approximation on our engine, not a copy of anything — **no CELSYS bytes are
in this repository** in any form: no preset files, no rasters, no icons, no
manual text.

The tool names in the filenames and `description` fields ("Real G-Pen",
"milli-pen", "ink-gire fude pen") name the tool each preset was calibrated
*against* — descriptive reference, not a claim of origin. CLIP STUDIO PAINT is
a trademark of CELSYS, Inc.; this project is neither affiliated with nor
endorsed by CELSYS.

## `krita/` — 9 presets, this project's own files

Seven are hand-authored here for this project's own brush engines:
`curve-brush`, `dyna-spring`, `grid-dots`, `hairy-bristles`, `marker-wash`,
`sketch-pen`, `textured-pencil`. Two — `hard-ink` and `sketch-scatter` — are
the `csp/` dynamics with our Krita-style dab modes enabled, and carry the same
`cspmap.mjs` note in their `notes` field.

"Krita" in the group and preset names describes the *behaviour* they
demonstrate. **No Krita source and no Krita preset files are in this
repository**; everything taken from Krita is reimplemented behaviour, in our
Rust or in our own patches to libmypaint (ISC).
Krita is GPL-3.0-or-later and MangaNakama is not — that separation is
deliberate and load-bearing, not an oversight.

## `textures/` — 3 dab masks, ours

`grain.png`, `streaks.png`, `dots.png`: tileable grayscale masks, procedurally
generated for this project (commit `ecfe0f4`, *"Shipped masks (procedural,
ours, tileable)"*).

## `../materials/` — 6 starter materials, ours

Two screentone dot densities, a 45° line tone, a halftone gradient, speed
lines and focus lines — procedurally generated for this project (commit
`490dcb1`, *"SIX procedurally generated PNGs … pure code output, ours to ship
… no third-party assets"*). These are our own take on generic manga concepts,
not reproductions of any vendor's artwork.

---

## What this file used to say, and why it was wrong

Until this rewrite, this file was upstream mypaint-brushes' `Licenses.md`,
copied in with the presets. It summarises the MyPaint **application**:

> MyPaint is licensed under the terms of the GNU Public License, version 2.0
> or later. See the file COPYING in this folder.

In this repository every part of that was false. The neighbouring `COPYING` is
CC0-1.0, not GPL-2.0. The `Licenses.dep5` it named as "the master document"
was never vendored, so the pointer was dangling. And MangaNakama is MIT OR
Apache-2.0, so the file read as a GPL claim over a tree containing no GPL
work. The mismatch begins upstream — mypaint-brushes' own `COPYING` is CC0
while its `Licenses.md` describes a GPL one — but shipping it here made it
our error to correct.

## Not verified — do not read these as established

- **Which upstream revision** the 35 `classic/` presets came from. The licence
  is not in doubt (upstream's `brushes/*` has been CC0-1.0 throughout), but the
  provenance pin is missing: commit `052cc90` records no upstream commit or
  tag. Anyone re-vendoring should record it.
- **Whether those 35 files are byte-identical to upstream** or were modified on
  the way in. Not checked. CC0 permits modification either way; this only
  affects how precisely the origin can be described.
- **`textures/` and `../materials/`.** "Procedurally generated here" rests on
  the commit messages quoted above — the generator scripts are not in the
  repository, so it is an author's statement rather than something a reader can
  re-run. No third-party source has ever been identified for these files. If
  you need it stronger than an author's statement, regenerate them.

## Standing rule

**The project's own assets and third-party assets never share an
undifferentiated licence file.** Every directory under `assets/` is named
above with its own origin and terms. A new asset directory is not shipped
until it appears in this table; a `COPYING`-style file that arrives with
third-party material belongs in that material's own directory, scoped in
writing, never one level up where it appears to cover our work too.
