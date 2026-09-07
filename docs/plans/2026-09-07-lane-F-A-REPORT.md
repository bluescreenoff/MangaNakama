# Lane F — Builder A report (2026-09-07)

> Brief: `docs/plans/2026-09-07-lane-F-brief.md`. Parent: `2026-09-07-lean-gauntlet-effects.md`.
> References: `docs/plans/refs/effect-lines/REFS.md` (ref-12…24, arrived mid-round).
> **Committed nothing** — Fable reviews and commits.

## PROGRESS BOX

- **DONE** — all of it.
  - `effect_metrics` built, calibrated against the previous rounds' own numbers, BEFORE table taken.
  - Renderer: the four defects, the sweep, and the reference pack's bundle model.
  - Both flash presets retuned; three tuning passes; AFTER table taken.
  - Numeric target table as a `#[test]` in `presets.rs` — passes, and bites (see below).
  - Harness: `README.txt`, two extra flash panels, a shared panel list, a `MN_PANELS` filter.
  - Gates: `./build.sh`, `cargo check --workspace --all-targets` (0 warnings),
    `cargo test -p mn-core genlines` (39 passed), both examples re-run.
- **NEXT (for Critic 1 / Builder B)** — one measured target missed, listed at the bottom.

## The headline

`solid-flash` before and after, same panel, same harness:

| | before | after |
|---|---|---|
| what it was | 84 % black with 64 hairline scratches and a **compass-circle** hole | a ragged white burst on a black field, white spikes in bundles, sawtooth boundary |
| round caps | **63** | **0** |
| hole edge σ | **0.07 mm** (a circle, to within a pixel) | **6.7 mm = 20.0 %** of its mean (ref-21 measures 20 %) |
| stroke p95/p50 | 1.29 | **5.57** |

`sea-urchin-flash`: 1.17 → **3.00**, caps 3 → **0**, hole σ 4.13 mm → 21.7 % of its mean
(ref-12 measures 20 %).

## What I changed, and why (four defects + the pack)

1. **Every tooth identical and evenly spaced.** The flash now walks the SAME angular bundle
   walk the focus kind uses (`radial_angles`), and the teeth of one bundle **share their
   bundle's apex and tip radii** so the pack reads as ONE peak built of slivers. That last part
   is not in the brief; it is the coordinator's mid-round correction and the pack's
   highest-confidence fact — CSP's own tool labels its spikes 2本/3本/4本 with a まとまり
   setting (ref-24), a close-up of an inked analog flash resolves each spike into 4–10 slivers
   (ref-22), a manga school budgets 4–6 lines per big peak (ref-23). Shipped as bundles of 3–5
   with a 4× hole. Per-tooth width accents and `len_skew` ride on top.
2. **Thickest stroke 1.9× the median, zero accents.** `UrchinParams` now carries `Mix`, so
   `accent_frac`/`accent_mul`/`len_skew` work exactly as they do for the other two renderers.
3. **The solid core was a perfect circle.** The ring scan's inner edge is now a 2048-bin table
   interpolated between the two neighbouring teeth's apex radii. Written as `a + (b − a)·t`, so
   with every apex equal it returns *exactly* `r_in` and the old raster is untouched.
4. **65 round caps.** Both rows' tips are needles that end in a point; the fat ends are straight
   cuts by design (ref-19 shows a real inked flash whose bases the panel border cuts dead
   straight, and the brief says the same). Caps are now 0 on every panel on the sheet.
5. **`sweep`** for an off-panel centre, plus — the solid kind's own version of the problem — the
   RING is clipped to the swept arc, or a burst aimed from outside the panel blacks the whole
   page on the way past. New panel `solid-flash-off-corner` is the picture of it.

Three things the pack forced that the brief did not have:

- **The teeth were upside down.** Every source describes the same construction: a **fat start
  and a thin whip-out**; ベタフラ is that stroke whose fat starts fuse into black, ウニフラ is the
  same stroke whose fat starts *don't quite* fuse and leave a chewed hole. Ours were pointed at
  the hole and blunt at the rim — the "polar zoom filter" the round-4 critic saw, and where its
  round caps came from. New `tip_out` flips them; `false` is the old geometry, bit for bit.
- **A tooth's width cannot be stated in millimetres.** It has to be a fraction of the angular
  pitch *where its fat end sits*, because whether neighbouring fat ends fuse IS the difference
  between the two effects. New `width_frac`; 0 = the old px `width`.
- **A flash is a balloon, not a burst.** Every source calls it フキダシの一種 and every reference
  is a balloon-sized object with its own silhouette. The old placement ran `r_out` to the
  panel's far corner, so the effect had no outline at all. New `reach_frac` (0 = the old corner
  reach) gives the urchin a drag-sized balloon. **The solid row deliberately keeps the corner
  reach**: ref-20/21 are an all-black field with a white burst punched through it, and there
  "everything you read as the flash is the WHITE shape".

## Metrics: BEFORE / AFTER

Same harness, same panel (100 × 70 mm at 600 dpi), same measuring code. Full tables are in
`target/effect-lines/METRICS.md`; the flash rows:

**BEFORE**

| panel | /25mm | w p5 | w p50 | w p95 | p95/p50 | acc % | caps | ink % TL/TR/BL/BR | hole mm ± σ |
|---|---:|---:|---:|---:|---:|---:|---:|---|---|
| `sea-urchin-flash` | 6.6 | 0.42 | 0.49 | 0.57 | **1.17** | **0** | **3** | 11.9/10.9/12.4/12.0 | 6.5 ± 4.13 |
| `solid-flash` | 6.3 | 2.52 | 3.39 | 4.38 | **1.29** | **0** | **63** | 84.6/78.9/86.0/83.8 | 9.9 ± **0.07** |
| `solid-flash-off-corner` | 3.1 | 5.50 | 6.96 | 8.70 | 1.25 | 0 | 7 | 92.5/92.2/93.2/93.3 | 28.7 ± 15.86 |
| `sea-urchin-flash-tight` | 6.9 | 0.44 | 0.49 | 0.57 | 1.17 | 0 | 4 | 12.6/13.1/12.7/13.1 | 2.9 ± 0.80 |

**AFTER**

| panel | cut @ | /25mm | w p5 | w p50 | w p95 | p95/p50 | acc % | caps | ink % TL/TR/BL/BR | bundles | hole mm | hole σ% | hi/lo × | outer σ% |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---|---:|---:|---|---:|
| `sea-urchin-flash` | r 23 | 42.7 | 0.04 | 0.08 | 0.25 | **3.00** | **9** | **0** | 6.0/8.6/4.6/7.1 | 1×209 2×16 3×1 4×1 | 13.0 | **21.7** | 1.59/0.62 | 27.2 |
| `solid-flash` | r 51 | 11.3 | 0.53 | 0.85 | 4.72 | **5.57** | **21** | **0** | 33.0/33.0/39.6/43.3 | 1×37 2×2 3×1 4×1 | 33.6 | **20.0** | **1.75**/0.54 | – |
| `solid-flash-off-corner` | r 90 | 7.0 | 0.85 | 1.31 | 8.36 | 6.37 | 25 | **0** | 83.4/14.4/88.1/64.6 | 1×24 | 62.0 | 25.2 | 2.05/0.59 | – |
| `sea-urchin-flash-tight` | r 17 | 55.5 | 0.02 | 0.04 | 0.08 | 2.00 | 11 | **0** | 3.3/6.2/2.4/4.5 | 1×210 2×14 | 4.4 | 65.0 | 4.30/0.35 | 64.9 |

**The other ten presets are byte-for-byte the same numbers before and after** (I diffed the two
runs; every cell matches, caps 0 throughout). Nothing outside the two flashes moved.

Targets, scored:

| target | source | urchin | solid |
|---|---|---|---|
| accents ≥ 8 % of strokes | brief | 9 % ✅ | 21 % ✅ |
| base-width p95/p50 ≥ 2.5 | brief | 3.00 ✅ | 5.57 ✅ |
| round caps 0 | brief | 0 ✅ | 0 ✅ |
| hole/inner-edge σ ≥ 0.4 mm | brief | 2.8 mm ✅ | 6.7 mm ✅ |
| fattest ≥ 4× thinnest | REFS 2 | 6.3× ✅ | 8.9× ✅ |
| hole σ ≥ 15 % of mean | REFS 3 | 21.7 % ✅ | – |
| hole rougher than the outline | REFS 3 | 21.7 vs 27.2 ❌ | – (no outline) |
| white region σ 20–31 % | REFS 4 | – | 20.0 % ✅ |
| longest white spike 1.5–2.0× median | REFS 4 | – | 1.75 ✅ |
| shortest 0.55–0.65× median | REFS 4 | – | 0.54 ⚠️ (0.01 out) |

### The one I did not get, and what I measured

**REFS target 3's second half — "the hole must be rougher than the outline".** Hole 21.7 %,
outline 27.2 %. Three tuning passes:

1. `jit_len` 0.55→0.75, `jit_len_out` 0.22→0.10, `core_jit` 0.35→0.45: hole 13.6→16.3, outline
   25.4 (up, not down).
2. `jit_len` 0.90, `jit_len_out` 0.05, `core_jit` 0.55: hole 20.5→21.7, outline 26.6→27.2.
3. `jit_len` 1.0, `core_jit` 0.75: hole **31.8**, outline **32.1** — the gap closes, but only by
   making the hole rougher than the reference's own 20 %, and p95/p50 fell 3.00→2.75. Reverted.

**Why it resists, measured not guessed:** the outer number is the last-ink radius per ray, and
our tips are one pixel. A ray has to pass within half a pixel of a hair's axis to reach its tip,
so most rays report a radius well short of the real fringe — the σ is grazing-ray noise as much
as shape. ref-12 measures 11 % because its needles are ~1.1° apart; ours are 1.5° and, unlike
ref-12's, carry accents, and a fat hair reads longer than a hairline at the same length. Two
honest ways forward for Builder B, neither of them a knob I could turn this round: raise the
count until the fringe is continuous (costs the 8:1 base-to-tip ratio at this hole radius), or
measure the outline on the eroded silhouette rather than per ray.

Also worth Critic 1 knowing: **`sea-urchin-flash-tight`** (`r_in_frac` 0.15, the crowded case
the brief asked for) is at hole σ 65 % and a 4.30× longest reach — far outside every reference
band. That is what the same preset does when the hole shrinks to a fifth of the drag: the fat
ends have no room and the shape stops being a ring. It is a stress panel, not a shipped row,
and I left it un-tuned on purpose so the failure mode is visible.

## Reference correction Critic 1 should know about

`REFS.md` (ref-13) says the canonical CSP ベタフラッシュ has **no hole at all** — a filled black
balloon with knocked-out lettering; the *holed* member of the family is 密フラッシュ. Per the
coordinator's ruling, kind 2 keeps its hole (that is where the art and the text go). What the
row now renders is ref-20/21's form: black field, white burst, ragged core — which has a hole in
the sense that matters (a clear middle you can letter into) without pretending to be ref-13.

## Files changed

| file | what |
|---|---|
| `crates/core/src/genlines.rs` | `UrchinParams` gains `width_frac`, `jit_len_out`, `mix`, `group`/`group_gap`/`group_jit`, `sweep_deg`/`sweep_center_deg`, `tip_out`, and `Default`. `Tooth` gains `ang` and takes either orientation. `render_urchin` rewritten: bundle walk, shared bundle peaks, accents, split length jitter, sweep-clipped ring, binary-search candidate lookup (`tooth_reach` / `candidate_window`), ragged core table (`core_lut`). `GenLinesSpec` gains `width_frac` + `tip_out` (both `#[serde(default)]`) and threads all of it through `render`. |
| `crates/core/src/genlines/presets.rs` | `LineOpts` gains `width_frac`, `tip_out`, `reach_frac`. `place` honours them and lets the flashes bundle. `LineOpts::flash` and both shipped rows rebuilt against the pack. NEW test `flash_presets_hit_their_measured_targets`. |
| `crates/core/src/genlines/metrics.rs` | **NEW.** The measuring code, used by both the example and that test. |
| `crates/core/examples/common/mod.rs` | **NEW.** The panel list, shared by the two examples so the table and the pictures can never describe different sets. Adds the brief's two flash panels. |
| `crates/core/examples/effect_metrics.rs` | **NEW.** Prints the markdown table. |
| `crates/core/examples/effect_lines_sheet.rs` | Uses the shared list; writes `README.txt`; `MN_PANELS` filter. |

Nothing else in the tree is touched. No preset outside the two flashes changed.

## Gates

    $ ./build.sh
       Compiling mn-core / mn-mcp / mn-gpu / mn-text / mn-brush / mn-app
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 45s

    $ cargo check --workspace --all-targets
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 12s
    Warnings: 0

    $ cargo test -p mn-core genlines
    test genlines::presets::tests::flash_presets_hit_their_measured_targets ... ok
    test genlines::presets::tests::place_matches_the_app_drag_maths ... ok
    test genlines::presets::tests::builtin_presets_all_render_without_panic ... ok
    test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 803 filtered out; finished in 75.21s

**The new test BITES** (project rule: a test is verified failing against the old code). Two
separate reversions, each run and then undone:

    tip_out: true -> false   (the pre-Lane-F tooth orientation)
      panicked: Sea urchin flash: round caps  (left == right failed)
    group: 5 -> 0            (rays instead of bundles)
      panicked: Sea urchin flash: round caps
    restored: test result: ok. 1 passed

38 tests before, 39 now. `legacy_renders_are_bit_stable`,
`pre_flash_specs_load_with_the_old_meaning`, `sea_urchin_ring_and_hole`,
`solid_flash_inverts_the_ring` and `place_matches_the_app_drag_maths` all pass **unchanged** —
no fingerprint re-pinned, no pin moved. Every new knob's zero is the old raster, including the
`rand()` order: the apex draw, the sliver wobble and the accent roll are each behind their own
guard, so an absent field does not even shift the random sequence.

    $ cargo run --release -p mn-core --example effect_lines_sheet
    [lines] 14 panels + 14 crops + sheet.png + README.txt -> target/effect-lines

    $ cargo run --release -p mn-core --example effect_metrics -- target/effect-lines/METRICS.md
    Rendered in 161.1 s.

**On the brief's "< 30 s in CI":** the CI *test* is 1.9 s released, 2 s in the debug suite — that
is the thing that fails a build, and it is well inside budget. The full 14-panel example is
161 s, and 150 s of that is RENDERING, not measuring: `saturated-line-centre-off-corner` alone
takes 43 s to rasterize (1285 rays × a per-ray bbox scan in `segment`) against 84 ms to measure.
The sheet example, which does no measuring at all, takes 8 minutes for the same reason. So the
example is a developer tool that costs what drawing the sheet costs; making it CI-cheap means
speeding up `segment` for the focus kind, which is not Lane F's to touch. Stated rather than
papered over.

## Notes for whoever reads the pictures

- `target/effect-lines/README.txt` says which PNG is 1:1 and which is shrunk ×3, the panel in mm
  and px, and the dpi. Read it first.
- `MN_PANELS=<substring>` on either example renders/measures only the matching rows — three
  minutes instead of eight while tuning.
- The four flash panels are `sea-urchin-flash`, `solid-flash`, `solid-flash-off-corner`
  (the sweep), `sea-urchin-flash-tight` (the crowded stress case).
- The round-cap probe is calibrated, not asserted: re-implemented from the round-2 recipe, it
  independently reproduces the previous rounds' published counts — 0 on all ten non-flash
  presets, 3 on the old `sea-urchin-flash`, 63 on the old `solid-flash` (round 3 published
  0 / 3 / 62). It skips ends inside a flash's core band, because a fat start is a base and
  ref-19 shows a real inked flash whose bases are cut dead straight.
