# Effect lines — critic round 4 (FINAL) — 2026-09-06

Judged `target/effect-lines/*.png` against `refs/effect-lines/ref-07..11`. No code read.
My own probes (throwaway, outside the repo, deleted): perpendicular ink-integral run widths
bucketed by half, ink per quarter and per sixth, 32 px tile-ink percentiles on both the output
and the reference panels, per-component max thickness on the 1:1 crops, strokes per 25.4 mm,
runs touching row 0, and round-2's erode-and-march cap detector re-implemented from scratch.

## Scores (1–5) — PASS needs every cell ≥ 4

| preset | weight mix | length var | rhythm | tips | page test |
|---|---|---|---|---|---|
| dark-burst | 5 | 4 | 5 | 5 | 4 |
| saturated-line-centre-below | 4 | 4 | 5 | 5 | 4 |
| saturated-line | 4 | 4 | 5 | 5 | 4 |
| dense-saturated-line | 4 | 4 | 4 | 5 | 4 |
| saturated-line-centre-off-corner | 4 | 4 | 4 | 5 | 4 |
| stream-line | 4 | 4 | 4 | 4 | 4 |
| dense-stream | 4 | 4 | 4 | 4 | 4 |
| sparse-stream | 4 | 4 | 4 | 5 | 4 |
| perspective-stream | 4 | 4 | 4 | 4 | 4 |
| drip-lines | 4 | 4 | 4 | 5 | 4 |
| sea-urchin-flash *(parked)* | 2 | 1 | 2 | 4 | 2 |
| solid-flash *(parked)* | 1 | 1 | 1 | 2 | 1 |

**Overall: PASS — all 10 non-parked presets clear ≥ 4 on all five axes.**

**PASSING (all ten):** `dark-burst`, `saturated-line`, `dense-saturated-line`,
`saturated-line-centre-below`, `saturated-line-centre-off-corner`, `stream-line`,
`dense-stream`, `sparse-stream`, `perspective-stream`, `drip-lines`.

Thinnest margins, in order: `drip-lines` (page test), `saturated-line-centre-off-corner`
(page test), `perspective-stream` (nothing above 4 anywhere).

## Round-3 defects: verified on my own numbers

| round-3 item | status | my measurement |
|---|---|---|
| **rails all in one half** — stream-line 7/26 px, sparse-stream 6/22 | **FIXED** | thinner-half thickest as % of thicker half: stream-line **68 %** (19.3/28.3 px), sparse-stream **80 %** (26.3/21.0), drips L/R **95 %** (19.0/20.0). Builder claimed 68/70/95 — confirmed. |
| **drips float below the frame** (3 of 40 touched row 0) | **FIXED** | 69 runs sit on row 0; **81 %** of long column-runs start at row 0 and 81 % within 2 mm. (Builder's per-line count says 96 %; my per-column count weights fat lines, hence the gap. Visually every line is cut hard by the frame.) |
| drip density 0.40/mm, bottom-right 1.1 % ink | **FIXED** | **0.70/mm** at y=15 %, 0.63/mm mid-panel; bottom-right sixth **3.3 %**, panel ink 3.8 % → **8.0 %**. No blank patch left. |
| perspective bottom-right void (49 % of mean) | **FIXED** | bottom-right sixth **8.9 %** vs panel mean **11.1 % = 80 %**. Exactly the builder's number. Most even stream on the sheet (min/mean 80 %). |
| **the last rounded stub** in dark-burst | **FIXED** | my cap detector: **0 caps in all ten non-parked presets** (solid-flash 14, parked, out of scope). Round 2 had 24. |
| sliver dashing between merged wedges | unchanged, **accepted** | not scored, per the brief |

Weight mix on the 1:1 crops, thickest stroke ÷ median stroke: stream-line **12.0×**,
perspective **7.3×**, dense-stream **7.3×**, dark-burst **5.3×**, centre-below 5.2×,
off-corner 5.0×, drips 5.0×, saturated 3.0×, sparse 3.0×, dense-saturated 2.5×.
Parked `sea-urchin-flash` is **1.9× with zero strokes above twice the median** — the number
that says "machine", and the reason it is parked.

Page test against the references, 32 px tile-ink median (ignores solid SFX blacks):
ref-10 top 10.4 / ref-10 bottom 9.0 / ref-11 left 15.0 / ref-11 right 7.7 / ref-08 top 45.7 /
ref-09 drips 24.4 / ref-07 streaks 25.2. Ours: dense-saturated 10.2, centre-below 11.2,
saturated 6.5, dark-burst 23.4, off-corner **16.9**, dense-stream 9.7, drips **6.9**.

## Ship note (plain words, one line each)

- **dark-burst** — the closest match on the sheet; heavy wedges arrive in twos and threes with
  runs of hairlines between, same as ref-08's top panel. *Still noticeable:* the reference page
  is darker than ours, so it hits slightly softer.
- **saturated-line-centre-below** — real bundles with real white gaps, like ref-11 left.
  *Still noticeable:* the reference leaves a white breathing hole near the bottom-centre where
  lines stop short of the centre; ours packs lines right down to the edge.
- **saturated-line** — centre-in-panel focus lines, ragged core, needle tips, best clumping on
  the sheet. *Still noticeable:* about a third lighter than ref-10's top panel.
- **dense-saturated-line** — density is dead-on ref-10 top (10.2 vs 10.4). *Still noticeable:*
  the heavies are only 2.5× the hairlines, so it reads a touch more even than the reference.
- **saturated-line-centre-off-corner** — right shape, right tips, right bundling.
  *Still noticeable:* it is roughly twice as dense/dark as ref-10's bottom panel (72 lines per
  inch vs ~45), so at page size it reads more like a dark texture than a burst.
- **stream-line** — heavy rails now run top to bottom instead of piling in the lower half, and
  at 2° of lean it reads drawn rather than ruled. *Still noticeable:* the right third of the
  panel carries about half the ink of the left third.
- **dense-stream** — bundles-then-hole rhythm at ref-07's density (9.7 vs ~15 for the cleanest
  reference band). *Still noticeable:* same left-heavy fade, and the hairline field between
  rails is the most uniform thing on the sheet.
- **sparse-stream** — genuinely sparse, two long spindles that swell and run off the frame.
  *Still noticeable:* it jumps hairline → rail with few mid-weights in between.
- **perspective-stream** — the vanishing-point fade is correct (widest ink furthest from the
  point) and the dead corner is filled. *Still noticeable:* nothing specific — it is simply the
  most "even" of the ten, which is the least hand-drawn quality it has.
- **drip-lines** — every line is now cut hard by the top frame, twice as many of them, and the
  bold verticals sit left, middle and right. *Still noticeable:* it is about a third as inky as
  ref-09 and it empties out toward the bottom (12.2 % ink up top, 3.8 % below) where the
  reference holds full density the whole way down. This is the one preset a reader could put
  next to its reference and call thin.
- **sea-urchin-flash / solid-flash** *(parked)* — every ray identical, evenly spaced. They look
  like a computer drew them because nothing varies.

## What a future round should do, ranked

1. **Preset numbers only — `drip-lines` density and fade.** Roughly double the ink and flatten
   the top-to-bottom falloff. Biggest visible gap against any reference anywhere on the sheet.
2. **Preset numbers only — `saturated-line-centre-off-corner` density down.** ~45 lines per inch
   and ~10 % ink to sit on ref-10's bottom panel instead of doubling it.
3. **Renderer — anti-aliasing / grey output.** Already known and accepted, but with the caps,
   rails and anchoring all fixed, hard 1-bit edges are now the largest remaining "a machine made
   this" tell once a page is shrunk for screen. It also dissolves the accepted sliver-dashing
   artefact for free.
4. **Renderer — a white relief hole for centres outside the panel.** `centre-below` has none;
   ref-11 left has an obvious one. Needs an inner stop measured from the off-panel centre, not
   from the frame.
5. **Preset numbers — the right-third fade in all three streams** (right third 55–60 % of the
   left). Defensible against ref-07, which is also uneven, so this is taste, not a defect.
6. **Preset numbers — mid-weights in `sparse-stream`** and a heavier tail in
   `dense-saturated-line`, both of which currently jump from hairline straight to rail.
7. **Out of scope unless unparked:** the two flash kinds need per-ray variation (length, pitch,
   taper) before they are worth scoring at all.
