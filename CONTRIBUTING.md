# Contributing to MangaNakama

Short version: build with `./build.sh`, run `./build.sh --test` before
opening a PR, and read the docs the repo keeps before changing code —
they record *why* everything is shaped the way it is.

## Setup

Follow [`SETUP.md`](SETUP.md): the Rust **GNU** toolchain (no Visual
Studio needed) plus the portable w64devkit C toolchain (not committed —
SETUP.md step 2 fetches it). Windows only: the app owns its Win32 +
pen-input path.

## The rules that matter

1. **Build via `./build.sh`, never bare `cargo`** — the vendored
   libmypaint C needs w64devkit's gcc/ar/dlltool on PATH, and the GNU
   link needs `link-self-contained` (`.cargo/config.toml` explains why).
2. **`./build.sh --test` green, zero warnings** — the full suite,
   including GPU/CPU parity checks. GPU tests run on WARP when no hardware
   adapter is present; CI does exactly that on every push.
3. **The CPU path is the reference; the GPU path is the destination.**
   Pixel features land with both, agreeing — tests enforce it.
4. **Docs live with the code they describe.** Before touching an area,
   read `docs/ARCHITECTURE.md` (especially "Traps"); if your change
   moves knowledge, update the doc in the same commit.
5. Vendored code (`vendor/`) carries numbered patches, documented in
   `vendor/PATCHES.md` — never edit vendored files without a patch-note
   entry.

## PR checklist

- [ ] `./build.sh --test` passes locally (0 warnings)
- [ ] New behavior has a test that fails without the change
- [ ] Docs updated if the change moves architecture or decisions
- [ ] No new dependencies without a note on why

## Reporting bugs (non-builders)

You don't need to build: grab a release exe and see the "Testers"
section of the README — attach `manganakama.log` (next to the exe, or
`%LOCALAPPDATA%\MangaNakama\` when that folder is read-only; press F1 for
the exact path). It names the build, your GPU adapter, recovery events and
any crash. Attach **that file alone** — it holds nothing personal, but its
neighbour `recent.txt` holds the full paths of files you have opened.
