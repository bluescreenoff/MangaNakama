# vendor/photocraft-psd

The PSD/PSB reader and writer from photocraft, vendored and trimmed to the
PSD core. MangaNakama's PSD import and export (`crates/core/src/psd/`) are
built on it.

| Item | Value |
|---|---|
| Repo | https://github.com/storytold/photocraft |
| Path upstream | `crates/psd` |
| Commit | `6326117c5394826a07afc1e897b9650699499381` |
| Commit date | 2026-11-05 21:04:59 UTC |
| Why this commit | the newest upstream commit that touches `crates/psd` |
| License | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`, verbatim from the upstream root) |

## Fetch method

No clone. List the tree, then fetch each blob:

```
gh api "repos/storytold/photocraft/git/trees/<commit>?recursive=1"
gh api "repos/storytold/photocraft/git/blobs/<sha>"   # .content is base64
```

Files were fetched into a scratch folder first, then copied here.

## Kept and dropped

Kept, verbatim except the patches below:

- `README.md` (upstream `crates/psd/README.md`), `LICENSE-MIT`, `LICENSE-APACHE`
- `src/`: `lib.rs`, `blend.rs`, `builder.rs`, `compression.rs` +
  `compression/tests.rs`, `descriptor.rs`, `error.rs`, `file.rs`,
  `filter_effects.rs`, `fixtures.rs`, `header.rs`, `image_data.rs`, `io.rs`,
  `layer.rs` + `layer/tests.rs`, `lazy.rs`, `pixels.rs`, `resources.rs`,
  `rows.rs`, `source.rs`, `tagged.rs`, `testgen.rs`, `tree.rs`
- `tests/it/`: `builder.rs`, `layer_block_order.rs`, `malformed.rs`,
  `merged_recovery.rs`, `roundtrip.rs`, `tree.rs`; `main.rs` rewritten to
  list only these
- `proptest-regressions/descriptor.txt`

`filter_effects.rs` is kept although the first plan listed it as dropped:
`tagged.rs` validates the `FEid`/`FXid` blocks (smart-filter caches, a PSD
block) through it, and it imports only kept modules.

Dropped (other Adobe formats, not PSD features): `src/abr.rs` + `abr/`,
`aco.rs`, `act.rs`, `ase.rs`, `asl.rs`, `atn.rs` + `atn/`, `codepage.rs`,
`csh.rs`, `grd.rs`, `hdr.rs`, `metadata.rs`, `path.rs`, `patterns.rs` +
`patterns/`, `shc.rs`, `slices.rs`, `tiff.rs` + `tiff/`. Tests dropped with
them: `tests/it/atn_contract.rs`, `atn_v12.rs`, `common/`, `corpus.rs`,
`corpus_atn.rs`, `swatch_malformed.rs`, `tests/csh.rs`, `tests/common/`,
`tests/fixtures/atn/`. Also dropped: `examples/`, `fuzz/`.

`tests/it/proptests.rs` is dropped too: it calls `grd`, `ase` and `aco`,
which are not vendored, so it does not compile. Per the vendoring rule it is
dropped whole, not edited. The property tests inside `src/descriptor.rs` and
`tests/it/malformed.rs` still run, so `proptest` stays a dev-dependency.

`Cargo.toml` is rewritten: upstream inherits everything from its workspace.
The `corpus` feature and the `encoding_rs` dependency are gone; `flate2`
uses its default (miniz_oxide) backend instead of upstream's `zlib-rs`.
`[lib] doctest = false`: the one doc example in `lib.rs` cannot link under
this toolchain (rustdoc does not get the `link-self-contained=yes` rustflag,
so it fails with `cannot find -lgcc_eh`); the same check runs in
`tests/it/builder.rs` and `roundtrip.rs`.

## Patches

Each one is also logged in `vendor/PATCHES.md`.

1. `src/lib.rs`: the `pub mod` lines of the dropped modules and `mod codepage;` deleted.
2. `src/file.rs` (`alpha_channel_names`): `crate::codepage::decode(name, encoding_rs::MACINTOSH)`
   became `crate::io::decode_legacy_name(name)` (UTF-8, else byte-as-char).
3. `src/lib.rs`: `#![allow(dead_code)]` with a comment. `dead_code` fires in
   `io.rs` (`utf16_units`, `Reader::new_little`, `append_with`) and `layer.rs`
   (`LayerInfo::write_unpadded`), helpers only the dropped modules used.
4. `src/file.rs`: the unit test
   `pascal_channel_names_use_mac_roman_and_unicode_takes_precedence` gets
   `#[ignore = "..."]`. It asserts the Mac Roman decoding patch 2 removes.

## Re-pin procedure

1. Pick the new commit (the newest one touching `crates/psd`).
2. Fetch the same path list at that commit (method above) into a scratch folder.
3. Copy over these files; re-apply patches 1-4; check that no kept file now
   imports a dropped module (`grep -rn "crate::" src`).
4. Update the commit and date in this file and in `THIRD-PARTY.md`.
5. Run the vendored tests and the MangaNakama PSD tests:
   `cargo test -p photocraft-psd` and `cargo test -p mn-core psd`.

## Review notes (read before trusting, 2026-11-07)

Read: `builder.rs`, `pixels.rs`, `tree.rs`, `blend.rs`, the mask parts of
`layer.rs`, `file.rs` `from_bytes` / `to_bytes` / `layers`.

- `PsdBuilder::build` sets `merged_alpha` (negative layer count) only when the
  supplied composite is not fully opaque. An opaque composite is written as
  three channels with no alpha, which has no spare channel either, so the CSP
  "stocked selection" ghost cannot appear.
- `PsdBuilder` gives every entry (layer, group divider, group folder) one record
  in push order, with sequential `lyid` from 1. Record k = entry k.
- The builder writes groups with `clipping = 0` and no extra blocks;
  `GroupSpec` has no field for `lspf` or a mask.
- `Layer::rgba8()` reduces 16-bit samples with `(v * 255 + 32767) / 65535`,
  so 16-bit 32768 becomes 8-bit 128, not 127.5. Through 8 bits, a 16-bit value
  is exact only to one 8-bit step (about 129 fix15 units).
- A layer with no transparency channel decodes with alpha 255.
- `Layer::user_mask()` prefers the real user mask (channel -3) over the user
  mask (-2) and reports the matching default colour.
- `layer_tree_max_depth(n)` keeps a group nested n or more deep as a node but
  drops its children; it never recurses.
- `MAX_DECODED_BYTES` is 8 GiB on 64-bit: a corrupt size field fails with
  `LimitExceeded` / `OutOfMemory` instead of aborting.
- The version-info resource (1057) is written by `version_info_resource`
  with writer "Adobe Photoshop" (MangaNakama replaces the writer string).

## Real-file tests (local-only fixtures)

`crates/core/src/psd/tests.rs` `real_files_roundtrip` walks
`crates/core/tests/data/psd/*.psd|*.psb` (gitignored, never committed) and
skips when the folder is absent. Photoshop-made files, from the psd-tools
test corpus (MIT, `psd-tools/psd-tools` `tests/psd_files/` at commit
`b7d7723790bc63f59f70ab2d7b91d8ee3124758b`), one per kind:

| Kind | File |
|---|---|
| groups | `group.psd` |
| masks | `mask.psd` |
| clipping | `clipping-mask.psd` |
| 16-bit | `16bit5x5.psd` |
| text | `text.psd` |
| CMYK | `colormodes/4x4_8bit_cmyk.psd` (saved flat as `4x4_8bit_cmyk.psd`) |
| PSB | `2layers.psb` |

Each has a `<name>.expect.txt` beside it (`layers=`, `folders=`, `names=`
bottom-first with children before their folder, `notes=`), written from the
crate's own reading of the file (`print_real_file_expectations`, ignored
test). The Clip Studio slot (`csp-layered.psd`, `csp-flat.psd`) prints a
"missing" line until the owner saves those two files.
