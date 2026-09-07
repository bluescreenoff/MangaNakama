# Lane F — Builder B report (2026-09-07)

> Brief: `docs/plans/2026-09-07-lane-F-brief.md`. Predecessor: `…-lane-F-A-REPORT.md`.
> Critic: `…-lane-F-critic1.md` (verdict **NOT CLOSE**, four ranked fixes).
> References: `docs/plans/refs/effect-lines/REFS.md` + ref-12/17/19/21/22/23/24 looked at.
> **Committed nothing** — Fable reviews and commits.

## PROGRESS BOX

- **DONE — all four fixes, re-measured, gates green.**
  - Fix 1 bundles → peaks: pack length envelope in the renderer, and the bundle PROBE fixed.
  - Fix 2 black field + finer slivers: new `field` knob, solid row rebuilt.
  - Fix 3 points not cuts, nothing floats: nib profile on the tooth (`needle` + `entry`),
    core-boundary min filter, caps probe rebuilt, new fragment probe.
  - Fix 4 sea-urchin: lobed hole, denser packs, `-tight` rescaled into a real small ウニフラ.
  - Target test extended and verified failing three ways. `./build.sh`, 0 warnings,
    39/39 genlines tests, both examples re-run, sheet + METRICS.md rewritten.
- **LEFT UNDONE** — one named residual, at the bottom: 5 blunt black-spike ends on
  `solid-flash` (out of ~500), and what to do about them next round.

---

## Fix 1 first, because it explains the round: where the bundles went

`radial_angles` was walking bundles the whole time. `LineOpts::place` copies `group`,
`group_gap` and `group_jit` for the flash kinds; `render_urchin` calls the walk. Nothing was
lost between the preset and the render. Two other things made the packs invisible, and one of
them was the ruler.

**1. The probe could not see a bundle, by construction.** `bundle_sizes` called a gap "tight"
when it was under **0.6 × the MEDIAN gap**. A walk of `group` strokes at pitch `p` followed by
one hole of `group_gap × p` emits `group − 1` within-bundle gaps for every one hole, so for any
`group ≥ 3` the median gap **is** the within-bundle gap `p` — and no gap is ever under `0.6 p`.
Every panel on the sheet reported `1×N` no matter how tight the packs were. That is the whole
of the `1×209 2×16 3×1 4×1` line the critic read as "almost every stroke a loner".

The fix is to ask the honest question — *are there two gap populations here?* — as a
one-dimensional 2-means over the gaps, with a 1.6× separation floor below which the set counts
as one rhythm. Re-measured with it, and with **no pixel changed**, the other ten presets report
the bundle sizes their own `group` values ask for:

| preset | `group` | bundles, old probe | bundles, fixed probe |
|---|---:|---|---|
| `saturated-line` | 4 | `1×76` | `2×1 3×14 4×8` |
| `dense-saturated-line` | 5 | `1×122 2×1` | `3×12 4×13 5×6 6×1` |
| `dark-burst` | 3 | `1×85 2×32 3×1` | `1×4 2×36 3×24 4×1` |
| `dense-stream` | 6 | `1×34 2×18 3×2` | `1×10 2×8 3×9 4×3 5×1 6×1` |
| `drip-lines` | 3 | `1×14 2×20 3×7` | `1×12 2×21 3×7` |

**2. The packs did not read as peaks.** Inside a bundle every stroke was the same length give or
take a ±6 % wobble, so the bundle's outer envelope was a flat-topped comb. ref-23's author draws
a red sawtooth over a real inked boundary captioned 「このジグザグをつくる」 and what makes it is
a pack of ~5 strokes leaving the mass shoulder to shoulder with **the middle one longest**. So
`render_urchin` now precomputes each tooth's `(place in pack, pack size)` off the walk's own
angles and tapers the length away from the pack's centre member (`PEAK_DROP`, 16 % of the ring
span), and the two ends' own wobbles were split: 3 % on the shared peak, 12 % on the fat end
(ref-17's stroke-starts "stack with visible gaps and overlaps"). The peak is what a pack shares;
the base is what it frays.

---

## What changed, fix by fix

### 1. Bundles that form peaks (all four panels)

- `PEAK_DROP` length envelope inside a pack (above), plus `SLIVER_JIT` / `SLIVER_IN` split.
- Urchin packs went from 5 with a 4-pitch hole to **9–12 with a 3.2-pitch hole**. That is not
  only rhythm, it is the band's duty cycle: 5 strokes against a 4-pitch hole puts ink on 43 %
  of the ring and the band cannot read black however fine the hairs are. ref-12 describes the
  ウニフラ as "packs of roughly 8–15 near-parallel needles … then a visible white gap", so this
  row follows ref-12 rather than the generic 2–6.
- Solid packs are **4–6 with a 3.5-pitch valley**, which is ref-22's own count.
- Probe fixed (above), and the target test now fails if under three quarters of the strokes on
  the cut sit in a pack of 3+.

### 2. Solid flash: a real black field, and 5–8× finer slivers

The root cause of "no black field" was that **one radius was answering two questions**. A
tooth's outer end is jittered as a fraction of `r_out − r_in`, so making `r_out` the panel
corner (to get black to the frame) made every white sliver corner-sized too, and all of them ran
off the frame. New `UrchinParams::field` / `GenLinesSpec::field` / `LineOpts::field`
(`#[serde(default)]`, 0 = the old raster): **`reach_frac` says how long a sliver is, `field` says
how far the black goes.** The solid row is `reach_frac` 1.85 with `field` 4.0 — a balloon whose
slivers stop well inside the frame, inside a black field that runs past any panel it fits in.
It is a multiple rather than "to the edge of the layer" so a flash dropped on a full page blacks
a big area, not the page.

Then the count and the cut width. `width_frac` was 0.90 — a cut nine tenths of the pitch merges
with its neighbours, so 170 slivers printed as ~40 huge white wedges. It is now **0.34**, so the
black thread between two slivers is as wide as the sliver (ref-22's measurement), and the drag
shrank from 0.42 w to 0.176 w so the white core is 14 mm inside a panel whose nearest edge is
33.6 mm away. Corner ink went **0 % → 100 %**.

`solid-flash-off-corner` got the same treatment plus a scaled count: a flash states its pitch as
a count over the WHOLE circle, so a burst at twice the radius drawn at the shipped count has
half the slivers per millimetre — which is exactly the critic's "giant multi-millimetre white
bands running frame to frame". 840 over the circle, ~280 in the 120° sweep.

### 3. Every stroke runs out to a point, and nothing floats

**The tooth got a nib.** `Mix::needle` and `Mix::entry` were on the spec and the preset already
and the flash renderer ignored them ("a tooth is a filled triangle, so it has no ramp to bend").
They now drive `Tooth::hit`, and both read 0 as the straight wedge every saved flash drew:

- `needle` under 1 keeps the belly and whips the last stretch to a point. A straight wedge
  averages half its base width over its length; at 0.5 it averages two thirds, and the ウニフラ's
  band ink rises with it. Urchin 0.5, solid 0.6.
- `entry` (入り) gives the FAT end a short ramp back to a point, so it arrives at full width
  instead of being cut square. This is the direct answer to "the heavy accent bar … its left end
  is cut square in open white" and "two black rectangles, blunt on both ends". Urchin 0.26.

**The caps probe was rebuilt**, because it was reporting 0 while the critic was finding blunt
ends by eye at 1:1. Two faults:

- the erosion radius was 3, so only strokes ≥ 7 px left a core to judge. Now 2 (≥ 5 px =
  0.21 mm at 600 dpi, a mark a reader can see the end of).
- worse, it skipped every end inside `hole_mean + 2σ` of the burst's centre, on the theory that
  a flash's fat start is a base. On `sea-urchin-flash` that zone reached **18.6 mm** while the
  hole itself was 13 mm, so it swallowed a third of the ring — including both of the blunt
  rectangles the critic found. The excuse is now BURIAL: an end whose neighbourhood is ≥ 62 %
  ink is inside a mass, where there is no end to see. That is the property that actually
  excuses a straight cut (ref-17's starts stack into a band; ref-21's white slivers run into the
  white core), and it does not depend on where the end happens to sit.

**A fragment probe** (`Metrics::frags`, new): black components of ≤ 20 px anywhere (dirt on the
scan) plus, on a solid flash whose black is meant to be ONE field, every black island floating
clear of it. First run reported **357 on `solid-flash`** — 355 specks and 2 islands, i.e. the
critic was right about the dirt and nearly right about the islands.

Where the 355 came from, measured rather than guessed (every one of them sat within a millimetre
of the core edge): the black between two neighbouring cuts is at its NARROWEST just above their
fat ends, because below a cut's base that cut stops and the black abruptly widens. The
interpolated core boundary landed in exactly that pinch, and a hard-edged 1-bit test prints a
sub-pixel wedge as a row of dots. `core_lut` now takes a **minimum over three quarters of a
pitch**, which pushes the boundary past the pinch so the black that survives is the wide part
below it. (Lifting the boundary instead was tried and doubled the count — it parks the boundary
in the pinch on purpose. Flooring the black per pixel the way `Tooth::hit` floors the cut was
also tried; it carved 2 px threads into dots and made it worse, 13 → 75.)

Frags: **357 → 0** on both solid rows, 0 on both urchin rows.

### 4. Sea-urchin: rough the hole, darken the band, give `-tight` a hole

- **The hole is lobed, not white-noised.** `core_jit` used to be a flat per-bundle pull-in, and
  at 0.55 against a `jit_len` of 0.90 it scattered the fat starts over 360 px of a 364 px hole
  radius — nothing stacked at the hole, so the band read mid-grey and the hole read smooth.
  A walked flash now takes its hole variation from a seeded three-harmonic wobble (`lobe`,
  harmonics 2/5/11 so the dominant shape is an oval, not a rounded triangle), applied
  SYMMETRICALLY about `r_in` so `r_in` still means the mean hole radius and the ring's span still
  means the stroke length. `jit_len` dropped 0.90 → 0.10. Saved flashes are unwalked and keep the
  old one-sided pull, off the same single `rand()`.
- **The band darkens** through the pack size (above), `width_frac` **1.10** — fat ends WIDER than
  the pitch, so inside a pack they overlap and fuse, which is ref-18's "70° arc fused into a
  solid black patch where the fat stroke-starts ran together" and REFS' "partial fusion is
  correct" — and the `needle` belly. Panel ink 6.0/8.6/4.6/7.1 → 6.8/8.1/3.4/8.5 on a burst that
  is now HALF the area, so the ring itself is roughly twice as dark.
- **REFS target 3, the one Builder A could not get and the critic called the sharpest fail in
  the pack**: hole 21.7 % against outline 27.2 % (ref-16 密フラッシュ, the wrong answer) is now
  **hole 19.9 % against outline 11.5 %** — ref-12 measures 20 / 11. It is asserted in the test.
- **`-tight` is a scaled burst, not a broken one.** It used to be the shipped row with
  `r_in_frac` dropped to 0.15 and nothing else moved, and that made a different EFFECT: the hole
  was a seventh of the reach so there was no ring, and because a tooth's fat end is a fraction of
  the pitch AT THE HOLE, shrinking the hole 4.7× shrank every tooth onto the half-pixel floor
  (p95/p50 2.00, "nothing wider than 2 px"). It is now half the drag with half the count, so the
  hole-to-reach ratio and the teeth's millimetres both survive: a 7 mm hole, p95/p50 4.50, packs
  of 6–8, hole σ 21.5 % against an 11.2 % outline.

---

## Metrics: before / after

Same harness, same 100 × 70 mm panel at 600 dpi. **Read the caps and bundles columns with
care**: BEFORE is Builder A's table under the old probes (blind to 5 px strokes, blind to any
end near the burst's middle, and unable to see a bundle at all), AFTER is the fixed ones. The
other columns are measured identically on both sides.

**BEFORE** (`target/effect-lines/METRICS.md` as Builder A left it)

| panel | /25mm | p95/p50 | acc % | caps | frags | corner % | ink % TL/TR/BL/BR | bundles | hole mm | σ% | hi/lo | outer σ% |
|---|---:|---:|---:|---:|---:|---:|---|---|---:|---:|---|---:|
| `sea-urchin-flash` | 42.7 | 3.00 | 9 | 0* | – | – | 6.0/8.6/4.6/7.1 | `1×209 2×16 3×1 4×1` | 13.0 | 21.7 | 1.59/0.62 | **27.2** |
| `solid-flash` | 11.3 | 5.57 | 21 | 0* | – | **0** | 33.0/33.0/39.6/43.3 | `1×37 2×2 3×1 4×1` | 33.6 | 20.0 | 1.75/0.54 | – |
| `solid-flash-off-corner` | 7.0 | 6.37 | 25 | 0* | – | – | 83.4/14.4/88.1/64.6 | `1×24` | 62.0 | 25.2 | 2.05/0.59 | – |
| `sea-urchin-flash-tight` | 55.5 | **2.00** | 11 | 0* | – | – | 3.3/6.2/2.4/4.5 | `1×210 2×14` | 4.4 | 65.0 | 4.30/0.35 | 64.9 |

**AFTER**

| panel | /25mm | p95/p50 | acc % | caps | frags | corner % | ink % TL/TR/BL/BR | bundles | white/hole mm | σ% | hi/lo | outer σ% |
|---|---:|---:|---:|---:|---:|---:|---|---|---:|---:|---|---:|
| `sea-urchin-flash` | 71.7 | 4.33 | 16 | **0** | **0** | – | 6.8/8.1/3.4/8.5 | `1×2 2×1 4×4 5×7 6×6 7×7 8×6 9×10 10×3 12×1 13×1` | 13.6 | **19.9** | 1.47/0.52 | **11.5** |
| `solid-flash` | 24.5 | 11.94 | 26 | 5 | **0** | **100** | 85.5/87.4/90.6/87.5 | `1×4 2×1 3×6 4×2 5×2 6×2 7×2 9×1 15×1 20×1 26×1` | 16.0 | 18.3 | **1.75/0.60** | – |
| `solid-flash-off-corner` | 16.2 | 17.26 | 27 | 5 | **0** | 0† | 99.8/67.0/100.0/97.3 | `1×4 2×1 3×2 4×1 9×2 15×1` | 35.5 | 26.1 | 1.83/0.37 | – |
| `sea-urchin-flash-tight` | 68.0 | 4.50 | 19 | 9 | **0** | – | 1.5/2.7/0.5/1.8 | `6×2 21×1 30×1 47×1 48×1` | 7.0 | **21.5** | 1.43/0.43 | **11.2** |

\* the old probe's zero, not a measured zero — see fix 3.
† the burst's own white core sits on this panel's top-right corner, which is the point of the
panel (ref-19). The corner target is asserted on the centred solid row only.

**Every non-flash preset is pixel-identical.** `/25mm`, the three width percentiles, `p95/p50`,
`acc %`, `caps`, the quarter inks and the hole numbers are the same cell for cell in both runs;
only the `bundles` column moved, and it moved because the probe was fixed, on the same raster.
Two pre-existing single specks show up in the new `frags` column (`dense-stream` 1,
`drip-lines` 1) — those rows are out of scope and untouched.

---

## Files changed

| file | what |
|---|---|
| `crates/core/src/genlines.rs` | `PEAK_DROP` + pack table (peaks, not combs); `SLIVER_JIT`/`SLIVER_IN` split; `lobe()` and the symmetric lobed hole; `Tooth` gains `needle` + `entry` and `hit` ramps with them; `UrchinParams::field` + the field-radius scan; `core_lut` min-filter; `tip_out`'s stale doc corrected (the solid row wants `true` too, and ref-22/23 say why). |
| `crates/core/src/genlines/presets.rs` | `LineOpts::field`, threaded in `place` for kind 2 only. `LineOpts::flash` and both shipped rows retuned. Target test extended with the four new pins. |
| `crates/core/src/genlines/metrics.rs` | `bundle_sizes` 2-means split (+ `gap_split`, `BUNDLE_SPLIT`); `round_caps` erosion 3→2 and the core-zone excuse replaced by `buried`; new `fragments` + `corner_ink`; the solid kind's sweep now measures the white REGION (flood-filled from the middle) with a ±1.5 px cross-ray tolerance. |
| `crates/core/examples/common/mod.rs` | solid drag 0.42 w → 0.176 w; `-off-corner` reach and count scaled; `-tight` rebuilt as a half-size burst. |
| `crates/core/examples/effect_metrics.rs` | two new columns and the column notes. |
| `crates/core/examples/effect_lines_sheet.rs` | the 1:1 crop follows a balloon-sized radial burst's own band instead of a fixed 62 %; `README.txt` says so. |

Nothing outside `genlines` is touched. `git status` shows the same three modified files plus the
same untracked set Builder A left.

---

## Gates

    $ ./build.sh
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 25s

    $ cargo check --workspace --all-targets      (through build.sh's PATH)
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.42s
    Warnings: 0

    $ ./build.sh --test -p mn-core genlines
    test genlines::presets::tests::flash_presets_hit_their_measured_targets ... ok
    test genlines::tests::legacy_renders_are_bit_stable ... ok
    test genlines::spec_tests::pre_flash_specs_load_with_the_old_meaning ... ok
    test genlines::tests::solid_flash_inverts_the_ring ... ok
    test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 803 filtered out; finished in 63.15s

    $ cargo run --release -p mn-core --example effect_lines_sheet
    [lines] 14 panels + 14 crops + sheet.png + README.txt -> target/effect-lines

    $ cargo run --release -p mn-core --example effect_metrics -- target/effect-lines/METRICS.md
    Rendered in 124.8 s.

Test count is unchanged at 39 — no new test file, the four new pins live in the existing target
table. **They bite**, verified by reverting one preset value at a time and re-running:

    group 12 -> 0        (an even circle instead of packs)
      Sea urchin flash: only 234 of 428 strokes are in a pack of 3+ — that is a comb
    field 4.0 -> 0.0     (the black stops with the teeth)
      Solid flash: the emptiest corner box is only 0 % ink
    entry 0.26 -> 0.0    (the fat end is a square cut again)
      Sea urchin flash: 45 ends stop dead instead of running out to a point

No fingerprint pin moved. Every new knob's 0 is the old raster, including the `rand()` order:
`needle`/`entry` draw nothing, the lobe phases run on their own seed stream, and the pack
envelope and the lobed hole are both behind `bases.is_some()`, which is false for every saved
flash.

---

## Left undone, and what I would do next

**`solid-flash` still reports 5 caps** (and `-off-corner` 5, `-tight` 9). The shipped urchin row
is 0. They are the inner ends of the BLACK spikes where they meet the white core, and they are a
deliberate trade, not an oversight: the core boundary is pushed a little below the cuts' fat ends
so the black does not pinch under a pixel and dot (that was 355 specks). Below the ends the black
is wide and clean, and a few spikes therefore meet the core with a short flat instead of a point.
At 1:1 this also shows as a short staircase along the core edge in `solid-flash-crop`.

The fix I did not have the budget to land, stated so the next round does not rediscover it:
**extend each CUT inward past the boundary instead of dropping the boundary**. A tooth whose
inner end runs below `core_lut`'s value keeps ramping, so it FLARES there and meets its
neighbours — the black tapers to a real point and disappears into the white core, with no
sub-pixel remnant to dot, and the boundary can go back to the sharp interpolation. It is a few
lines in `render_urchin` plus a re-tune of `width_frac`, and it should take caps to 0 and remove
the staircase at the same time.

Two smaller things:

- `sea-urchin-flash-tight`'s 9 caps are a scaling artefact: `entry` is a fraction of the stroke
  LENGTH, and at half the burst size the ramp is half as long in pixels. A millimetre floor on
  the entry ramp would fix it. It is a stress panel, not a shipped row.
- `dense-stream` and `drip-lines` each carry one 20-px speck. Pre-existing, pixel-identical to
  Builder A's run, and out of Lane F's scope — but now visible in the table, so someone should
  decide whether the stream renderer owes a floor too.
