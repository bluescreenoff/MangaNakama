# Effect lines — critic round 3 (2026-09-06)

Judged `target/effect-lines/*.png` against `refs/effect-lines/ref-07..11`. No code read.
Measured, on my own probes: perpendicular ink-integral widths, strokes per 25.4 mm (the 1:1
crop is exactly 25.4 mm wide), skeleton stroke-ends and the half-width at each one (the
round-cap test), bundle-size histograms, gap CV, per-sixth thickest stroke, per-sixth ink,
per-line angle fit, and the same probes on the reference panels.

## Scores (1–5) — PASS needs every cell ≥ 4

| preset | weight mix | length var | rhythm | tips | page test |
|---|---|---|---|---|---|
| dense-saturated-line | 5 | 4 | 4 | 5 | 4 |
| dark-burst | 5 | 4 | 4 | 4 | 4 |
| saturated-line | 4 | 4 | 5 | 5 | 4 |
| saturated-line-centre-below | 4 | 4 | 4 | 5 | 4 |
| saturated-line-centre-off-corner | 4 | 4 | 4 | 5 | 4 |
| dense-stream | 4 | 4 | 4 | 4 | 4 |
| perspective-stream | 4 | 4 | 4 | 4 | 4 |
| stream-line | **3** | 4 | 4 | 4 | 4 |
| sparse-stream | **3** | 4 | 4 | 5 | 4 |
| drip-lines | **3** | 4 | 4 | **3** | **3** |
| sea-urchin-flash *(parked)* | 2 | 1 | 2 | 3 | 2 |
| solid-flash *(parked)* | 1 | 1 | 1 | 3 | 1 |

**Overall: FAIL — but 7 of 10 non-parked presets PASS.**

**PASSING:** `saturated-line`, `dense-saturated-line`, `dark-burst`,
`saturated-line-centre-below`, `saturated-line-centre-off-corner`, `dense-stream`,
`perspective-stream`.

**FAILING:** `stream-line`, `sparse-stream`, `drip-lines`. All three fail on the SAME axis —
weight mix — and for the same reason: the rails exist now, but they are all parked in one
region of the panel, so most of the paper is single-weight. `drip-lines` also fails tips
and page test.

## Round-2 defects: fixed / not

| round-2 item | status | my measurement |
|---|---|---|
| **round blob caps (the headline defect)** | **FIXED** | 89 interior stroke ends across the 10 crops; 88 end at half-width 1 px, i.e. a 1–2 px needle. One exception: `dark-burst-crop` (182,149), half-width 4 px — a rounded 8 px stub. 26 → 1. |
| off-corner "32 rays for a whole panel" | **FIXED** | 30–43 strokes / 25.4 mm in all six windows (ref-10 bottom reads 45 across its panel width); fan fills the frame corner to corner; 54 strokes now die inside the panel, so the frame-to-frame problem is gone too |
| centre-below density + no hairlines | **FIXED** | 29–38 strokes / 25.4 mm; crop width p5 = 1 px (round 2 measured 4 px) |
| exact-2 comb in sparse-stream / drips | **FIXED** | bundle sizes sparse 1/2/3/4, drips 1/2/4/6; gap CV 0.83 and 0.82 (a comb would sit near 0.1) |
| drip hard width step mid-length | **FIXED** | 0 jumps ≥ 1.5 px on the 8 long drips in the crop, after 5 px smoothing |
| perspective bottom-right void (1.8 % ink) | **PARTLY** | bottom-right sixth now 4.9 % against a panel mean of 9.9 % — exactly at the "half the mean" line, still the emptiest patch on the sheet |
| rails in the streams and drips, **scattered** | **PARTLY** | rails exist (max/median 8–25×) but see below — they are all in one half |
| streams "measurably parallel" | **PARTLY** | angle spread 0.9° across a 25 mm crop (sd 0.23–0.26°); `sparse-stream` only 0.27° |
| **drips all start at the top edge** | **REGRESSED** | 3 of ~40 drips touch row 0; only 16 of 40 are within 2 mm of it. ref-09: 21 of 21 start exactly on the frame line |
| dark-burst sliver dashing | unchanged, **accepted** | 17 of 171 wedge-pairs have a ≤ 3 px white gap. The brief accepts this |

## Per preset, worst first

**drip-lines (3,4,4,3,3)** — the only preset failing three axes.
1. *The curtain floats.* Only 3 of ~40 drips reach the top edge; the rest start 0–2.5 mm
   below it, each with a fine point aimed upward. ref-09 cuts every single line hard on the
   frame. Side by side this is the one difference you see instantly: theirs hangs from
   somewhere, ours hangs from nothing. Fix: force ~90 % of drips to overshoot row 0.
2. *No heavies on the right.* Thickest stroke per sixth: 23 / 26 / **10** across the top,
   15 / 13 / **6** across the bottom. The right third is hairline-only.
3. *Density.* 0.40 drips per mm against ref-09's ~0.9 per mm; ink 3.8 % against ref-09's
   24 %. Bottom-right sixth is 1.1 % ink — nearly blank paper.
4. One drip near x = 57 (preview) begins as a fat rounded teardrop, not a point.

**sparse-stream (3,4,4,5,4)** — the tips are the best on the sheet: both rails are long
spindles that swell and run off the frame. But the thickest stroke in the **top half** is
6 px, in the **bottom half** 22 px. The rails are two near-twins stacked ~25 mm apart in the
same corner of the panel. On a page the top two thirds read as one uniform weight. Move one
rail above the midline and the preset passes.

**stream-line (3,4,4,4,4)** — same defect, mirrored. Round 2 said the heavies were all
top-left; they are now all **bottom**: thickest per sixth 7 / 7 / 6 on top, 26 / 24 / 13
below. The builder's own [3,0,3] was measured across left/middle/right thirds, which hides
the vertical split. Ink 3.9 %–11.8 % across the sixths (3.0×). Add 2 rails to the top half.

**perspective-stream (4,4,4,4,4) — passes, thinnest margin.** Rails now in all six windows
(width 25 / 22 / 14 / 15 / 15 / 10 px per sixth — the fade toward the vanishing point is
correct, the widest ink is furthest from it). Gap CV 0.54 is the most even of the ten, and
the bottom-right is still the lightest patch on the sheet (4.9 % vs 9.9 % mean). It clears
the bar; it is the first thing to break if anything else moves.

**dense-stream (4,4,4,4,4) — passes.** Bundles-then-hole rhythm, 25–28 strokes / 25.4 mm,
rails 8× the hairline in five of six windows. Mild left-to-right ink gradient (14.0 % → 6.3 %).

**dark-burst (5,4,4,4,4) — passes.** Closest thing on the sheet to ref-08's top panel:
wedges 43 px beside 2 px hairlines, ragged core, ink 22–30 % evenly across all six windows.
Costs a point on tips for the one rounded stub at (182,149). The dashed white slivers are the
brief's accepted artefact and I did not score them.

**saturated-line (4,4,5,5,4) / dense-saturated-line (5,4,4,5,4) — pass.** Every interior
end is a needle (16/16 and 24/24). `saturated-line` has the highest gap CV on the sheet
(0.84) — real clumps and real holes. Weight range max/median 6.5 and 6.0 against ref-10
top's 5.2. Only 4.8 % / 7.2 % of strokes are rails against ref-10's 13 % — a shade shy, not
a defect.

**saturated-line-centre-below (4,4,4,5,4) / -off-corner (4,4,4,5,4) — pass, and these are
the round's biggest jump** (1s and 2s in round 2). Both fill the frame, both carry rails in
all six windows, both are almost perfectly even in ink (centre-below 9.8–13.3 %,
off-corner 10.6–16.1 %) with 11 and 17 strokes dying mid-crop. Matches ref-11 left (14.0 %
ink) and ref-10 bottom (10.9 %).

**sea-urchin-flash / solid-flash — parked, scored only.** Gap CV 0.33 and even angular
pitch; one taper profile; sea-urchin's max/median is 3.0 with 0.2 % rails.

## Taste, not defects — with the number that would settle each

- **Stream fan.** 0.9° of angle spread per 25 mm. ref-07's streaks are near-parallel too, so
  this is a call, not a fault. If you want it unmistakably hand-ruled, target 2–3° per panel.
- **Drip density.** Ours 0.40/mm, ref-09 ~0.9/mm. The brief calls ref-09 "sparse", so either
  reading is defensible. To match the reference, double the count.
- **dark-burst blackness.** 22–30 % ink against ref-08 top's 48.7 % (inflated by the SFX
  kanji inside my box). Darker is available; the current level already reads as a page.

## Top 5 fixes, ordered

1. **Anchor the drips to the top edge.** ~90 % should overshoot row 0 instead of 3 of 40.
   Single biggest visual gap against a reference anywhere on the sheet.
2. **Spread the rails vertically in `stream-line` and `sparse-stream`.** Target: no half of
   the panel whose thickest stroke is under 40 % of the other half's (now 7 vs 26, and 6 vs 22).
   This alone flips both presets to PASS.
3. **Give `drip-lines` heavies in the right third** (thickest there 10 px vs 26 px on the
   left) and lift its bottom-right sixth off 1.1 % ink.
4. **Kill the last rounded stub** in `dark-burst` at crop (182,149), half-width 4 px — the
   only survivor of round 2's 26 caps.
5. **Top up `perspective-stream`'s bottom-right** (4.9 % vs 9.9 % mean) so the one passing
   preset with no margin gets some.
