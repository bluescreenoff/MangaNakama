# Effect lines — critic round 2 (2026-09-06)

Judged `target/effect-lines/*.png` (panel + 1:1 crop) against `refs/effect-lines/ref-07..11`.
No code read. Measured: perpendicular black-run widths in the 1-bit crops, strokes per
25.4 mm, rays crossing a 6 %-inset perimeter, 4×4 ink grid, and a parallelism residual.

## Scores (1–5) — PASS needs every cell ≥ 4

| preset | weight mix | length var | rhythm | tips | page test |
|---|---|---|---|---|---|
| dense-saturated-line | 5 | 4 | 4 | **3** | 4 |
| saturated-line | 4 | 4 | 4 | **3** | 4 |
| dark-burst | 5 | 4 | 4 | **2** | 4 |
| dense-stream | **3** | 4 | 4 | 4 | 4 |
| stream-line | **3** | 4 | **3** | 4 | **3** |
| perspective-stream | **3** | 4 | **3** | 4 | **3** |
| drip-lines | **2** | 4 | **2** | **3** | **2** |
| saturated-line-centre-below | **3** | 4 | **2** | **3** | **2** |
| sparse-stream | **2** | 4 | **2** | 4 | **2** |
| saturated-line-centre-off-corner | **3** | **3** | **1** | **2** | **1** |
| sea-urchin-flash *(parked)* | 2 | 1 | 1 | 2 | 1 |
| solid-flash *(parked)* | 1 | 1 | 1 | 2 | 1 |

**Overall: FAIL.** 0 of 10 non-parked presets pass. `saturated-line` and
`dense-saturated-line` are one axis short — both fail only on tips, and both fail it
for the same single reason (round caps). Fix that one thing and two presets pass.

## Round-1 defects: fixed / not

| round-1 item | status | evidence |
|---|---|---|
| 1 tips dissolve into dashes | **partly** — flashes fixed; dark-burst still dashes | in `dark-burst-crop` y≈225 two heavies merge and the white separator between them breaks into 6 white dashes; reads as a print fault |
| 2 hairline floor 1 px | **partly** — radial only | crop width p50: saturated 3 px, dense-sat 3 px, dark-burst 5 px (fixed); but stream-line 2, dense-stream 1, drip 2, perspective 2 — p5 still 1 px |
| 3 blunt inner ends | **FIXED** | `saturated-line` core at 5× shows needle points at many depths |
| 4 beading at shallow angles | **FIXED** | dense-sat right side y 170–250 is solid, clean staircase |
| 5 only two weight tiers | **partly** | continuum now in saturated / dense-sat / dark-burst. Streams + drips are still "wall of hairline + 1–3 strays" |
| 6 no bundling anywhere | **partly + regression** | real bundles in dense-stream and the radial sets. But sparse-stream and drip-lines got bundles of **exactly 2, at a near-constant pitch** — a comb, more mechanical than before |
| perspective: top-right void, zero ink on top edge | **FIXED, void moved** | top edge now inked; bottom-right 4×4 cell is 1.8 % ink vs 13.9 % max — the emptiest cell in the whole sheet |
| stream-line lower-half hole | **FIXED** | grid now 3.0–9.8 %, no hole |
| drip: nothing touches top edge, bottom 26 % blank | **FIXED** | all drips start at y = 0, longest reach y≈520/551 |
| saturated-line inner ends form a ring | **FIXED** | core rim is ragged, ends land over a wide radius band |
| dark-burst core is a compass circle | **FIXED** | rim is irregular now |
| off-corner "triple the ray count" | **NOT DONE** | 32 rays across the perimeter, unchanged; 6 strokes / 25 mm |
| centre-below "raise density ~2×" | **NOT DONE** | 6 → 7 strokes / 25 mm |

**New regression (round 1 did not have it): every heavy stroke that dies mid-field now ends
in a round blob cap.** A semicircular dome sitting on a still-5-to-10-px-wide wedge. Seen in
`saturated-line` (panel 150,465), `dense-saturated-line` (panel 40,290),
`saturated-line-centre-off-corner` (two in one 280 px window), `centre-below`, `dark-burst`.
Round 1 asked for tapers instead of square 1-px cuts; what arrived reads as a felt-tip dot,
not a G-pen exit. This single defect is why the three best presets fail.

## Per preset, worst first

**saturated-line-centre-off-corner (3,3,1,2,1)** — worst in the sheet. 32 rays for a whole
panel; 6 strokes per 25 mm at 1:1 where ref-10's lower panel reads 47+ even at its lossy scan
resolution. The fan only opens about 60°, so the far-right and lower-left of the panel are
bare white. Nearly every line runs frame to frame, so there is no length rhythm. Two heavy
strokes end in obvious round blobs. Reads as a ruled vector starburst, not 集中線.
Needs: 3× the rays, fan opened to cover the panel, a third of the rays killed mid-field.

**sparse-stream (2,4,2,4,2)** — exactly one heavy stroke in the entire panel and it sits on
the top edge; everything else is 1–3 px. The bundles are all exactly two lines at an almost
constant pitch — a picket comb. Lines are parallel to within ~3 px over 23 mm (measured), so
there is no fan and no bow. Add rails, vary bundle size 1–5, vary the pitch.

**saturated-line-centre-below (3,4,2,3,2)** — density never moved from round 1 (7 strokes /
25 mm vs ref-11 left's ~0.65/mm curtain). The crop has **no hairlines at all** (p5 width
4 px) — it is heavy + medium only. Two heavies right of centre are near-identical twins
(same weight, same taper, ~15 px apart) and read as a duplicated line. Round caps on heavies.

**drip-lines (2,4,2,3,2)** — top-edge anchoring and length spread are genuinely fixed and
match ref-09. But every drip is the same 1–3 px hairline: zero heavies, where ref-09 has
several clearly bolder verticals. The pairs are a regular period across the whole panel.
One stroke changes width in a hard step mid-length (crop x≈530, y≈130) — looks like two
lines butt-jointed. Some top ends are blunt.

**perspective-stream (3,4,3,4,3)** — the vanishing knot resolves and there is finally one
12 px rail, but at 1:1 the crop is almost pure hairline; the rails are too few and clustered
near the top. Two heavies run exactly parallel 4 px apart for the crop's full width (a
doubled line). The lower-right quarter is nearly empty (1.8 % ink). One dead-straight,
uniform-weight line spans the whole panel at y≈100 and reads as a ruled border, not a streak.

**stream-line (3,4,3,4,3)** — all three heavy streaks sit in the top-left third; the bottom
half and the right half have none, so the panel reads lit-from-the-left (left quarter 7.3 %
ink, right quarter 3.0 %). Little bundling — an even sprinkle. Lines exactly parallel.

**dense-stream (3,4,4,4,4)** — the best stream. Real bundles-then-hole rhythm, density
(33 strokes / 25 mm) matches ref-07's top panel, clean fine starts mid-field. Only fails
weight: max 6 px against a 1 px hairline, and ref-07 puts solid rails ~10× the hairline
beside them. Add 2–4 rails and it passes.

**dark-burst (5,4,4,2,4)** — at panel scale the closest thing here to ref-08's top panel:
wedges 54 px beside 2 px hairlines, ragged core, holes. Fails on tips only, three ways:
(a) adjacent heavy wedges merge and the surviving white gap dashes out (crop y≈225);
(b) a heavy stroke makes a rectangular step-change in width mid-length (crop 180–350, y≈390);
(c) round blob caps.

**saturated-line (4,4,4,3,4) / dense-saturated-line (5,4,4,3,4)** — these two would pass a
Jump page. Weight continuum, staggered inner ends, clumps and uneven holes, needle points on
the short rays. The only thing stopping them is the round cap on the heavies.

**sea-urchin-flash / solid-flash — parked.** Both are still perfectly even angular spacing,
one taper profile, a compass-circle core. Scored, not asked to change this round.

## Top 5 fixes, ordered

1. **Kill the round line cap.** Heavy strokes must exit as a spindle point, like the
   hairlines already do. Unblocks `saturated-line` and `dense-saturated-line` outright.
2. **Density for the two off-centre focus sheets.** `off-corner` 32 → ~100 rays and open the
   fan to the whole panel; `centre-below` 54 → ~110, and give it hairlines (its crop has none).
3. **Rails for the streams and the drips.** One or two strokes ~10× the hairline in
   `dense-stream`, `stream-line`, `sparse-stream`, `perspective-stream`, `drip-lines` — and
   scatter them over the whole panel, not into one corner (stream-line's are all top-left).
4. **Break the exact-2 comb** in `sparse-stream` and `drip-lines` (bundle size 1–5, uneven
   pitch), and add a slow fan — the streams are measurably parallel to within 3 px over 23 mm.
5. **dark-burst tips:** stop adjacent heavy wedges from merging so the white separator dashes
   out, and remove the mid-stroke rectangular width step.
