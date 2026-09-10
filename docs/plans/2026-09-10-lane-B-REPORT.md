# Lane B — split `crates/core/src/doc.rs` into `crates/core/src/doc/`

## DONE / NEXT
- **The split is COMPLETE and green.**
- Baseline (before): mn-core `840 passed; 0 failed; 2 ignored`, doctests `0 passed; 2 ignored`.
- After: mn-core `840 passed; 0 failed; 2 ignored`, doctests `0 passed; 2 ignored` — identical.
- `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets`: **exit 0, zero rustc warnings**
  (the only lines matching `warning:` are the pre-existing `mn-brush` build-script notes about
  `gcc.exe` probing, which appear on `master` too; the project C toolchain lives in
  `toolchain/w64devkit/bin` and must be on PATH, see `build.sh`).
- Content-equivalence proof: normalising both sides (strip indentation, drop blank lines, sort) and
  diffing the old `doc.rs` against the concatenation of all 19 new files yields ONLY the expected
  structural lines — 11 `impl Document {`, 11 `use super::*;`, 6 `use super::super::*;`, the `mod`
  declarations, minus the 6 removed `#[cfg(test)] mod X {` wrappers. **Not one line of code changed.**
- One forced visibility widening: `Document::take_op` private -> `pub(super)` (see Deviations 4).
- Cross-crate spot check `cargo test -p mn-app frames -- --test-threads=2`: **11 passed, 0 failed**
  (1015 filtered out). Zero warnings in that whole workspace build too.
- NEXT: nothing blocking. Lane B is done.
- NOT COMMITTED. New files are staged by name, `doc.rs` deletion staged, working tree left for review.

## Line counts

| file | lines |
|---|---|
| (before) `doc.rs` | 9293 |
| `doc/mod.rs` | 2249 |
| `doc/history.rs` | 649 |
| `doc/props.rs` | 607 |
| `doc/paint.rs` | 478 |
| `doc/layers.rs` | 470 |
| `doc/frames.rs` | 454 |
| `doc/masks.rs` | 383 |
| `doc/merge.rs` | 314 |
| `doc/derived.rs` | 261 |
| `doc/spill.rs` | 245 |
| `doc/vector_layers.rs` | 157 |
| `doc/resize.rs` | 141 |
| `doc/tests/mod.rs` | 11 |
| `doc/tests/tests.rs` | 1997 |
| `doc/tests/group_tests.rs` | 303 |
| `doc/tests/combine_tests.rs` | 201 |
| `doc/tests/op_count_tests.rs` | 193 |
| `doc/tests/mask_tests.rs` | 169 |
| `doc/tests/import_mask_tests.rs` | 54 |
| **total** | **9336** (+43 = the `mod`/`impl`/`use` scaffolding) |

## Deviations from the plan (and why)
1. **`ResizeAnchor` stayed in `mod.rs`** (with its `impl`). The plan's table put it in `doc/resize.rs`,
   but it is `pub` and callers outside the crate use `mn_core::doc::ResizeAnchor`; moving it would need
   a new `pub use`, which the plan forbids. Only the `impl Document` resize methods moved.
2. **`impl Layer { covers_canvas, is_uniform_white, extend_white }` stayed in `mod.rs`.** These are
   private inherent methods and `doc/resize.rs` calls them (lines 5625/5628/5696/5698 of the old file).
   An inherent `fn` is private to the module that writes the `impl`, so putting them in `paint.rs`
   would have broken `resize.rs`. Same reasoning kept `fill_layer_from_image*`, `bake_layer_into`,
   `tile_range`, `tile_count_for` in `mod.rs` (used from `layers.rs` and `merge.rs`).
3. **`FREEFORM_BATCH`, `paint_guard`, `over_pixel` went to `paint.rs`, not `resize.rs`.** They are only
   used by the gradient/fill code. The plan listed them under resize by line number; keeping them there
   would have forced visibility widening for no gain.
4. **One visibility widening, forced by the compiler:** `Document::take_op` is `pub(super)` instead of
   private, because `doc/frames.rs::regen_genlines` calls it. `pub(super)` = visible inside `doc` and
   its children only — narrower than `pub(crate)`.
5. **Imports:** each child file uses `use super::*;` (test files `use super::super::*;`) rather than a
   hand-picked import list. A glob of the parent is the standard idiom for a split `impl`, it re-exports
   nothing, and it cannot drift out of sync or emit unused-import warnings.
6. **`doc/tests/tests.rs` is 1997 lines**, over the plan's 1500 cap. It is the single pre-existing
   `mod tests` block, flat assertions only; splitting it further would mean inventing groupings, which
   is not a pure move. Flagged, not done.
7. **Test paths gained one level**: `doc::tests::*` is now `doc::tests::tests::*` etc., because the plan
   asked for a `doc/tests/` directory. Test *count* is unchanged; the *names* gained a `tests::` segment.

## Note on the git history
`git mv crates/core/src/doc.rs crates/core/src/doc/mod.rs` was the first step, so the rename is what
git recorded. `git diff --cached --stat -M` still prints it as a delete + adds, because `mod.rs` keeps
only 2249 of the old 9293 lines (24%) and the default rename threshold is 50%. Use
`git log --follow -- crates/core/src/doc/mod.rs` or `git diff -M20%` to see the rename.

## Commands used (for a resume)
```
export PATH="$PWD/toolchain/w64devkit/bin:$PATH"   # build.sh does this; mn-brush needs the C toolchain
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets
CARGO_BUILD_JOBS=2 cargo test  -p mn-core          -- --test-threads=2
CARGO_BUILD_JOBS=2 cargo test  -p mn-app frames    -- --test-threads=2
```
The split itself was mechanical: `/tmp/laneB/split.sh` cuts line ranges out of a copy of the original
(`/tmp/laneB/doc_orig.rs`) with `sed -n 'a,bp'`; nothing was retyped.
