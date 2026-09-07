# Lane F — Builder C report (2026-09-07)

> Critic: `docs/plans/2026-09-07-lane-F-critic2.md` (verdict ONE MORE ROUND, two fixes).
> Predecessor: `…-lane-F-B-REPORT.md`. Start point: clean tree at `89b42f5`.
> **Commits nothing.**

## PROGRESS BOX

- [x] Read critic2, B-report, REFS, looked at ref-17/18/20/21/22, read the renderer.
- [x] Fix 1 — `sea-urchin-flash-tight`: root every bundle at the hole, kill the leaves.
- [x] Fix 2 — both solid flashes: 2–3× more, finer slivers dying in a band.
- [x] Re-pin the target test.
- [x] Gates + re-run both examples. ALL GREEN, report complete.

## Baseline (Builder B's committed numbers, `target/effect-lines/METRICS.md`)

| panel | /25mm | p95/p50 | acc % | caps | frags | corner % | bundles | white/hole mm | σ% | hi/lo | outer σ% |
|---|---:|---:|---:|---:|---:|---:|---|---:|---:|---|---:|
| `sea-urchin-flash` | 71.7 | 4.33 | 16 | 0 | 0 | – | `1×2 2×1 4×4 5×7 6×6 7×7 8×6 9×10 10×3 12×1 13×1` | 13.6 | 19.9 | 1.47/0.52 | 11.5 |
| `solid-flash` | 24.5 | 11.94 | 26 | 5 | 0 | 100 | `1×4 2×1 3×6 4×2 5×2 6×2 7×2 9×1 15×1 20×1 26×1` | 16.0 | 18.3 | 1.75/0.60 | – |
| `solid-flash-off-corner` | 16.2 | 17.26 | 27 | 5 | 0 | 0† | `1×4 2×1 3×2 4×1 9×2 15×1` | 35.5 | 26.1 | 1.83/0.37 | – |
| `sea-urchin-flash-tight` | 68.0 | 4.50 | 19 | **9** | 0 | – | `6×2 21×1 30×1 47×1 48×1` | 7.0 | 21.5 | 1.43/0.43 | 11.2 |

† the burst's own white core sits on that panel's top-right corner; the corner target is
asserted on the centred solid row only.

## Fix 1 — done (pass 1). What was actually wrong

Two independent causes, both visible in the code once you look for them.

**The floating leaves were the hole's own jitter running OUTWARD.** Builder B's walked
hole was `b_apex += r_in · core_jit · (0.65·lobe(θ) + 0.35·(2u−1))` — a smooth lobe plus a
per-bundle draw that ran BOTH ways — and on top of that a per-member
`r_apex += (rand−0.5)·SLIVER_IN·span`, also both ways. On `-tight` (r_in = 153 px,
core_jit 0.60) the per-bundle half alone is ±32 px = ±1.4 mm and the per-member half
±15.6 px, so two neighbouring packs could start 2.7 mm apart in radius with clean white
beside them. That is the critic's leaf, measured.

**The blunt ends were `entry` being a fraction of the LENGTH.** `Tooth::entry` 0.26 gives a
67 px ramp on the shipped urchin (span 260 px) and 34 px on `-tight` (span 130 px), while
the accent half-width is 2.09 × 4.5 = 9.4 px on BOTH. The caps probe wants the ink to keep
going for `stroke_half + 3` px past where the eroded core ends, and the core ends where the
ramp is 2 px wide, i.e. `2·E/hw` px from the tip: 14.4 px on the shipped row (passes),
7.2 px on `-tight` (fails). That is the whole of `caps 9`, and it is arithmetic, not taste.

### What changed

- The walked hole is now ONE smooth curve, `r_in · (1 + core_jit · HOLE_LOBE · lobe(θ))`,
  and BOTH jitters bite inward from it: `HOLE_BUMP` per bundle, `HOLE_CHEW` per stroke.
  Nothing can start outside the curve, so no pack can float and the raggedness that is left
  is at stroke scale (critic 2 asked for that on the shipped row too). `SLIVER_IN` is gone.
- `entry` is stated PER TOOTH: the preset's fraction floored at `ENTRY_NIB` (7) half-widths
  of that tooth, capped at `ENTRY_MAX` (0.45) so the floor cannot turn a stroke into a
  spindle. On the shipped row every tooth is still 0.26 (the floor does not bite);
  on `-tight` the accents go to 0.45 and their march reach to 12.4 px.
- The `rand()` sequence is unchanged on the unwalked path: the `in_jit` draw still happens,
  it is just not spent on pushing the apex outward when the flash walks.

### Measured (pass 1, whole sheet re-measured)

| panel | caps | frags | hole σ% | outer σ% | /25mm | p95/p50 |
|---|---:|---:|---:|---:|---:|---:|
| `sea-urchin-flash` | 0 (was 0) | 0 | 22.6 (was 19.9) | 13.5 (was 11.5) | 73.6 | 4.33 |
| `sea-urchin-flash-tight` | **0 (was 9)** | 0 | 22.8 (was 21.5) | 13.5 (was 11.2) | 74.1 | 4.40 |

Every non-flash row is cell-for-cell identical to Builder B's table.

## Fix 2 — done. What was actually wrong, and the wall I hit

Three separate things, and only the first was the one the critic named.

**1. Density.** 380 teeth over the circle became ~270 after the walk's holes, and the wide
length spread meant only about half were still alive where the cut crosses — 24.5 strokes
per 25 mm, the critic's ~135 tips. Now 460 teeth → **354 emitted slivers, ref-21's own
number**, and the length spread is tighter so 42.4 of them cross the cut.

**2. The band.** `jit_len_out` 0.85 (times the renderer's 0.85 outer reach) let a sliver
lose 72 % of the span, so at any radius most had stopped and the survivors read as isolated
hairlines. 0.72 with `reach_frac` 1.58 against a 0.85 hole puts the whole fringe between
about 1.35× and 1.86× the core radius, with a solid black margin all round (corner ink 100).

**3. The blunt spike ends — Builder B's named fix, landed.** A ベタフラ's CUT now runs on
below its own fat end, FLARING to half the widest gap it has to close (`Tooth::flare_hw`),
so it merges with its neighbours, the black between them tapers to a point and disappears
into the white core, and every white sliver starts ON the core edge instead of 1.4 mm out in
the black. `caps` 5 → **0** on both solid rows, and the test's 12-cap budget is gone.

### The wall: a 1-bit raster cannot end a black wedge in a point

Removing the boundary's min-filter, as Builder B proposed, replaced 5 caps with **50
specks**. Diagnosed properly this time — I dumped every fragment's coordinates and read
them rather than reasoning from the picture — they are the last sub-pixel stretch of a black
wedge, printed as a dotted line. Four shape fixes were tried and measured:

| change | frags |
|---|---:|
| flare, linear ramp, boundary sharp | 50 |
| …squared ramp (closes steeply at the end) | 52 |
| …plus a floor on the black thread (`BLACK_MIN`, cuts capped at half their own gap) | 32 |
| …plus the boundary pushed 2 pitches clear again | 12 |
| `core_jit` 0.42 → 0.20 (a smoother core) | 10, **but white σ 18.8 → 14.5** |

That last row is the wall: the specks live where the black meets the *sloped* core boundary,
so every shape lever trades them against the core's own raggedness, which is the thing REFS
target 4 measures. The geometry cannot win it — a wedge that narrows to zero passes through
sub-pixel, and a hard-edged raster prints that as dots.

So the rule is **stated** instead of approximated. ref-20/21 and REFS target 5 both say a
ベタフラ's black is ONE mass; the renderer now floods from past `r_out` (where there are no
teeth and the field is solid by construction) and inks only the black that flood reached.
`frags` 0 on both solid rows with the raggedness intact. It is gated on `field > 1`, so a
saved flash — whose black stops with its teeth and is not a field — never runs it.

### Measured

| panel | /25mm | p95/p50 | acc % | caps | frags | corner % | white mm | σ% | hi/lo |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| `solid-flash` before | 24.5 | 11.94 | 26 | 5 | 0 | 100 | 16.0 | 18.3 | 1.75/0.60 |
| `solid-flash` after | **42.4** | 6.25 | 27 | **0** | 0 | 100 | 16.2 | 18.8 | 1.46/0.66 |
| `solid-flash-off-corner` before | 16.2 | 17.26 | 27 | 5 | 0 | – | 35.5 | 26.1 | 1.83/0.37 |
| `solid-flash-off-corner` after | **39.3** | 11.00 | 26 | **0** | 0 | – | 32.8 | 26.4 | 1.76/0.43 |

### Not reached: "`/25mm` roughly triples"

42.4 is 1.7×, not 3×. Tripling needs ~800 teeth on this shape, where the pitch at the hole
is 2.8 px and the black thread between two slivers is 1.8 px before any jitter — under the
raster. The critic's other number, the one tied to a measurement, IS met: 354 emitted
slivers against ref-21's 354 and REFS' 150–350 band. His ~135 came from converting /25mm to
tips, which undercounts by exactly the slivers that have already ended at the cut radius.

## Final table — the four flash panels

BEFORE = Builder B's committed run (`89b42f5`). AFTER = this round. Same harness, same
100 × 70 mm panel at 600 dpi, same probes (`metrics.rs` is **byte-identical to HEAD** — no
ruler was moved this round).

| panel | /25mm | p95/p50 | acc % | caps | frags | corner % | ink % TL/TR/BL/BR | white/hole mm | σ% | hi/lo | outer σ% |
|---|---:|---:|---:|---:|---:|---:|---|---:|---:|---|---:|
| `sea-urchin-flash` before | 71.7 | 4.33 | 16 | 0 | 0 | – | 6.8/8.1/3.4/8.5 | 13.6 | 19.9 | 1.47/0.52 | 11.5 |
| `sea-urchin-flash` after | 73.6 | 4.33 | 15 | **0** | **0** | – | 7.3/9.7/4.6/9.3 | 11.9 | 22.6 | 1.54/0.48 | 13.5 |
| `sea-urchin-flash-tight` before | 68.0 | 4.50 | 19 | **9** | 0 | – | 1.5/2.7/0.5/1.8 | 7.0 | 21.5 | 1.43/0.43 | 11.2 |
| `sea-urchin-flash-tight` after | 74.1 | 4.40 | 16 | **0** | **0** | – | 1.5/3.3/0.5/2.3 | 6.0 | 22.8 | 1.63/0.49 | 13.5 |
| `solid-flash` before | 24.5 | 11.94 | 26 | **5** | 0 | 100 | 85.5/87.4/90.6/87.5 | 16.0 | 18.3 | 1.75/0.60 | – |
| `solid-flash` after | **42.4** | 6.25 | 27 | **0** | **0** | 100 | 86.6/88.4/91.1/88.9 | 16.2 | 18.8 | 1.46/0.66 | – |
| `solid-flash-off-corner` before | 16.2 | 17.26 | 27 | **5** | 0 | 0† | 99.8/67.0/100.0/97.3 | 35.5 | 26.1 | 1.83/0.37 | – |
| `solid-flash-off-corner` after | **39.3** | 11.00 | 26 | **0** | **0** | 0† | 99.9/71.4/100.0/98.6 | 32.8 | 26.4 | 1.76/0.43 | – |

† the burst's own white core sits on that panel's top-right corner, which is the point of
the panel (ref-19). The corner target is asserted on the centred solid row only.

**Every non-flash preset is pixel-identical.** All ten rows — `/25mm`, the three width
percentiles, `p95/p50`, `acc %`, `caps`, `frags`, `corner %`, the quarter inks, the bundle
histogram and the hole numbers — match Builder B's table cell for cell.

## Files changed

| file | what |
|---|---|
| `crates/core/src/genlines.rs` | `HOLE_LOBE`/`HOLE_BUMP`/`HOLE_CHEW` replace the two-way hole jitter and `SLIVER_IN`; `ENTRY_NIB`/`ENTRY_MAX` and a per-tooth entry fraction; `Tooth::flare_hw` + the flare branch in `hit` + `FLARE_SLOPE`; `tooth_reach` accounts for a flared cut; `BLACK_MIN` and the two caps on the sorted teeth (a base pulled out to where the pitch can carry it, a cut capped at half its own gap); the solid scan keeps only the black its field flood reached. |
| `crates/core/src/genlines/presets.rs` | `Solid flash` retuned: count 380 → 460, `width_frac` 0.34 → 0.40, `reach_frac` 1.70 → 1.58, `jit_len_out` 0.85 → 0.72, `group_gap` 3.5 → 2.8, `len_skew` 0.15 → 0.05. Target test: caps budget 12 → 0 on every row, a new sliver-density pin on the solid rows, and the harness's tight burst added as a third row. |
| `crates/core/examples/common/mod.rs` | `Solid flash - off corner`: count 840 → 1120 (the same pitch in px as the shipped row, since the burst is 2.4× the radius), `reach_frac` 1.9 → 1.58. |

`crates/core/src/genlines/metrics.rs` is byte-identical to HEAD — the numbers moved because
the pictures moved, not because the ruler did.

## Gates

    $ ./build.sh
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 3m 48s

    $ cargo check --workspace --all-targets      (through build.sh's PATH)
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 02s
    Warnings: 0

    $ ./build.sh --test -p mn-core genlines
    test genlines::presets::tests::flash_presets_hit_their_measured_targets ... ok
    test genlines::spec_tests::pre_flash_specs_load_with_the_old_meaning ... ok
    test genlines::tests::legacy_renders_are_bit_stable ... ok
    test genlines::tests::solid_flash_inverts_the_ring ... ok
    test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 803 filtered out; finished in 112.87s

    $ cargo run --release -p mn-core --example effect_lines_sheet
    [lines] 14 panels + 14 crops + sheet.png + README.txt -> target/effect-lines

    $ cargo run --release -p mn-core --example effect_metrics -- target/effect-lines/METRICS.md
    Rendered in 200.7 s.

Test count unchanged at 39. Both examples write PNGs and exit; nothing opened a window.

## Left undone

- **`/25mm` on `solid-flash` is 1.7×, not the 3× the critic asked for.** Explained above: at
  600 dpi the black thread between two slivers is the binding constraint, and the count
  target that IS tied to a measurement (354 slivers vs ref-21's 354) is met. If a future
  round wants 3×, it has to come from a bigger balloon or a higher dpi, not from a knob.
- **`solid-flash-off-corner`'s far band still opens to ~1.5–1.7 mm gaps** at the outer end
  of the crop, right on the critic's limit rather than inside it, and its bundle histogram
  (`1×13 2×8 …`, 72 % of strokes in a pack of 3+) is under the 75 % the shipped rows hold.
  Its angle wobble is a bigger share of a pitch than the shipped row's because the sweep
  spends the count over a 120° arc. A `jit_gap` tightening on that row would fix both; it is
  a harness row, not a shipped preset, and it is not pinned.
- **Compositionally the off-corner panel is still ~55 % featureless black.** That is what a
  shorter band costs on a burst aimed from outside the frame, and it is ref-19's own look —
  but the critic flagged it and it is still true.
- **`sea-urchin-flash`'s white core reads a touch small** (11.9 mm mean, was 13.6): the
  per-stroke chew eats inward from the lobe curve, so the mean hole radius comes down by
  about `core_jit · HOLE_CHEW / 2`. If a round wants the old hole size back, raise
  `r_in_frac` rather than shrinking the chew — the chew is what makes the serration.
- `dense-stream` and `drip-lines` still carry one 20-px speck each. Pre-existing,
  pixel-identical to Builders A and B, out of scope both rounds.

## The new pins bite — verified by reverting one value at a time

    ENTRY_NIB 7.0 -> 0.0        (the nib's 入り goes back to a pure fraction of the length)
      Sea urchin flash - tight: 1 ends stop dead instead of running out to a point

    Solid flash count 460 -> 380 (Builder B's density)
      Solid flash: 35.4 white slivers per 25 mm of the cut, wanted 40-70

Both fail on the row and for the reason the fix was made. The third change — the flare —
has no single value that switches it off (its slope floors at a pixel), so it is evidenced
by measurement instead: `caps` 5 -> 0 on both solid rows, which is what the tightened
`assert_eq!(m.caps, 0)` now holds.

## Sanity on scope

`git status` shows exactly three modified files plus this report. Nothing outside
`genlines` and the two examples is touched, `metrics.rs` is byte-identical to HEAD, and
**nothing is committed** — Fable reviews and commits.
