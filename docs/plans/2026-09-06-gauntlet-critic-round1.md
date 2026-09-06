# Effect lines — critic round 1 (2026-09-06)

Judged: `target/effect-lines/*.png` (panel + 1:1 crop) against
`docs/plans/refs/effect-lines/ref-07..11`. Read-only pass, no code seen.

## What makes the references look hand-drawn

- **ref-07 top (流線):** lines come in *bundles* — 4–8 packed almost touching,
  then a white gap several line-widths wide. Not evenly sprinkled. Lines start
  and stop at scattered points; many never cross the panel. Slight bow/wobble.
- **ref-08 2nd panel (perspective):** solid black rails ~20× the hairline width
  sit right beside hairlines, all aimed at one visible vanishing knot. Ink flecks.
- **ref-09 (drip):** every line *starts flush on the top edge*. Depths run from
  1/8 to full height. Pairs and triples nearly touch, then a wide hole. Ends
  thin to a point.
- **ref-10 / ref-11 (集中線):** the white core is a **ragged blob**, never a
  circle — inner ends are staggered ±30% of the core radius. Heavies come in
  pairs/triples and widen toward the frame. Many lines die mid-field.

## Scores (1–5) — PASS needs every cell ≥4

| preset | weight mix | length var | rhythm | tips | page test |
|---|---|---|---|---|---|
| dark-burst | 5 | 3 | 3 | 3 | 3 |
| saturated-line-centre-below | 4 | 4 | 3 | 4 | 3 |
| saturated-line | 3 | 3 | 2 | 2 | 2 |
| dense-saturated-line | 2 | 4 | 3 | 2 | 2 |
| drip-lines | 3 | 3 | 2 | 4 | 2 |
| stream-line | 2 | 4 | 2 | 2 | 2 |
| dense-stream | 2 | 4 | 2 | 2 | 2 |
| saturated-line-centre-off-corner | 2 | 4 | 2 | 2 | 2 |
| sparse-stream | 1 | 4 | 2 | 2 | 2 |
| perspective-stream | 1 | 3 | 2 | 1 | 1 |
| sea-urchin-flash | 2 | 1 | 1 | 1 | 1 |
| solid-flash | 1 | 1 | 1 | 1 | 1 |

**Overall: FAIL.** 0 of 12 presets pass. Two (dark-burst,
saturated-line-centre-below) are within one round of passing.

## Cross-cutting bugs (fix these once, 9 presets improve)

1. **Tapered tips dissolve into dashes.** Output is 1-bit (measured: exactly 2
   grey levels in every file). Where a taper drops below 1 px the threshold
   turns the tip into a row of separated dots. Clearly visible in
   `sea-urchin-flash-crop` (left edge), `solid-flash-crop` (white slits near the
   core), `saturated-line-centre-off-corner`. A line must never thin below
   ~2 px @600 dpi; stop the taper there and end it, or supersample then threshold.
2. **Hairlines are 1 px @600 dpi = 0.042 mm.** Measured perpendicular width
   p50 = 1 px for saturated-line, dense-saturated-line, perspective-stream.
   That is under print reliability (a G-pen hairline is 0.08–0.15 mm ≈ 2–4 px).
   Raise the floor to 2 px and let the range go up from there.
3. **Blunt inner ends.** Every non-heavy line ends in a square 1-px cut with no
   taper (see `saturated-line-crop` x60–300 / `dense-stream-crop` top-left).
   The reference has needle points on both ends of the short lines.
4. **Beading at shallow angles.** Near-horizontal lines alternate 1 px / 2 px
   runs, so they read as an uneven grey-to-black dotted rope instead of one
   stroke (`dense-saturated-line-crop` right side, y≈180–220).
5. **Only two weight tiers.** Almost every set is "a wall of 1 px" plus 1–3
   isolated fat lines. References have a continuum, and the heavies arrive in
   pairs/triples, not as lone strays.
6. **No bundling anywhere.** Nothing in the whole sheet shows 4+ lines packed
   nearly touching followed by a wide hole. That single trait is the loudest
   hand-drawn cue in ref-07 and ref-09.

## Per preset, worst first

**solid-flash (1,1,1,1,1)** — perfect-circle hole, mathematically even slits,
identical widths. Reads as vector clip-art. Needs: irregular hand-cut core
(oval/blob, not a circle), slit widths varying 3×, slit inner ends staggered so
the core edge is jagged, some slits stopping short of the frame. Also the
dashed-tip bug.

**sea-urchin-flash (2,1,1,1,1)** — every line the same length, the same taper
profile, at the same angular spacing; core is a perfect circle. Reads as a
"polar zoom" filter. The real thing (visible in ref-07 2nd panel) is a small
dense ball of *short irregular* spikes around a white blob, plus loose flecks
outside. Fully rebuild.

**perspective-stream (1,3,2,1,1)** — no heavy strokes at all (max 3 px vs
hairline 1 px); ref-08 has solid rails beside hairlines. Vanishing point never
resolves — every line is clipped before convergence, and the top-right quadrant
is a large empty void. Top edge has zero ink (bbox starts at y=24/551). Add
2–4 solid rails, push the convergence knot into frame, fill the top-right.

**sparse-stream (1,4,2,2,2)** — measured widths p10=2, p90=4, max=4: effectively
uniform. Add heavies. Runs are evenly scattered; bundle them.

**saturated-line-centre-off-corner (2,4,2,2,2)** — only ~35 lines for a whole
panel where ref-10 right has 200+; one heavy line total. Lines near the centre
break into dashes. Triple the count, add heavies in pairs.

**stream-line / dense-stream (2,4,2,2,2)** — "dense" is not dense: 8 strokes per
25 mm at 1:1 vs 40+ in ref-07. Both are an even sprinkle with no bundles, and
`stream-line` has one hole covering most of the lower half that reads as
unfinished rather than as rhythm. All angles are exactly parallel — add a slow
fan and a slight bow.

**drip-lines (3,3,2,4,2)** — tips are the best in the set, but it floats: bbox
is x[18,785] y[14,406] of 787×551, so **no line touches the top edge** (ref-09:
all of them do) and the bottom 26% of the panel is blank. Spacing is combed —
28 lines, gap CV 0.47, minimum gap still ~1.9 mm; ref-09 has near-touching
pairs. Anchor every line to y=0, let a few reach the bottom, cluster the rest.

**dense-saturated-line (2,4,3,2,2)** — good inner-end staggering. But max width
in the 1:1 crop is 5 px, so at print scale there are no heavies at all; the
core still reads as a circle; shallow lines bead.

**saturated-line (3,3,2,2,2)** — inner ends form a visible ring around a clean
circular core; spread them ±30% so the boundary is a ragged blob. Angular
spacing is even — clump it. Most outer ends reach the frame; kill a third of
them mid-field. Heavies are lone strays, not pairs.

**saturated-line-centre-below (4,4,3,4,3)** — closest to passing with
dark-burst. Clean spindle tips, real wedges, well-scattered inner ends. Only
problems: too sparse (6 strokes per 25 mm at 1:1), and two large voids in the
upper half (around x 200–320 and x 580–760) that read as missing rather than
rhythmic. Raise density ~2×.

**dark-burst (5,3,3,3,3)** — best of the set; the wedge/hairline interleave
genuinely resembles ref-08. Remaining: the white core is a near-perfect circle
with an evenly scalloped rim; every line runs core-to-edge with none dying
mid-field; the heavy/thin alternation is close to periodic; hairline inner ends
are still blunt.
