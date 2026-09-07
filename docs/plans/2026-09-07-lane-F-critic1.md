# Lane F — Critic 1 (pictures vs references)

Fresh-eyes visual review, 2026-09-07. I did not read or run any code. I judged the four
in-scope panels (shrunk **and** 1:1 `-crop`) against the reference pack in
`docs/plans/refs/effect-lines/REFS.md`, and I looked at the refs myself:
ref-12, 13, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24.

For the 1:1 checks I zoomed three regions 3× with nearest-neighbour (no resampling, so
what I describe is real pixels):
`solid-flash-crop` (60,180)+200², `solid-flash-off-corner-crop` (120,230)+220²,
`sea-urchin-flash-crop` (0,150)+200².

Reminder of the harness: `<name>-crop.png` is 600×600 px = 25.4 × 25.4 mm of paper, taken
at 62 % of the panel width, vertically centred. So in every crop, **left = toward the
burst centre, right = outward**, and there is **no panel frame anywhere in the crop** — an
end that stops dead in a crop is a real blunt end, not a frame clip.

---

## Scores

| panel | score | one-line reason |
|---|---|---|
| `sea-urchin-flash` | **2 / 5** | Reads as a grey fuzzy donut of ruled hairlines. Its hole is *smoother* than its outline — that is the ref-16 密フラッシュ failure, the single sharpest fail in the pack. No bundle-peaks. Blunt square stroke ends and rectangular dashes at 1:1. |
| `sea-urchin-flash-tight` | **1 / 5** | Not a ウニフラ at all. It is a 集中線 (plain focus-line burst) converging on a ~10 px white dot. No usable hole, no heavy accents anywhere (nothing wider than 2 px), no bundles. |
| `solid-flash` | **2 / 5** | Right gross idea — black around a white burst — but ~40 teeth where ref-21 has ~354, so it reads as a chunky jagged star, not a flash. There is no solid black field: white slivers run all the way to all four panel edges. Floating black triangles and loose specks inside the hole. |
| `solid-flash-off-corner` | **1 / 5** | Reads as op-art diagonal stripes / a sunburst wallpaper, not a ベタフラ. ~24 strokes total. Giant multi-millimetre white bands running frame to frame, and a 9 mm stretch of dead-straight black/white boundary whose only texture is a uniform rasterisation staircase. |

### Why, panel by panel

**`sea-urchin-flash` — 2 / 5.**
Silhouette is fine at arm's length: a ring with a white middle, no petals, no perfect
circle. Two things kill it.

*The roughness is on the wrong side.* In the shrunk PNG (787 × 551) the hole edge is a
near-circle of radius ≈ 110 px around (400, 255) — even, tame, you could trace it with a
compass. Meanwhile the outer fringe is wild: lone lines shoot 40–60 px past their
neighbours at 1 o'clock (≈ 560, 110) and at 7 o'clock (≈ 300, 470). That is exactly the
picture REFS.md prints as the *wrong answer*, ref-16 密フラッシュ: "every second or third
ray is a long one shooting well past its neighbours" around a clean oval. ref-12, the
actual ウニフラ, is the opposite — chewed hole, comparatively tame outline. METRICS agrees
(hole σ 21.7 % vs outer σ 27.2 %) but the table does not shout that this inverts REFS
target 3.

*The band never reads black.* ref-12's ring is a solid dark annulus you read as a mass;
ours is 6–8 % ink and reads mid-grey, with white showing through everywhere. At 1:1 the
gaps between neighbouring strokes are 5–20× the stroke width. ref-22 says the gap inside a
bundle is about **one** sliver width. This is the "silently fell back to plain flash"
condition in the REFS bonus discriminator.

*At 1:1 the strokes are machine.* In the zoom of `sea-urchin-flash-crop` region
(0,150)–(200,350): the heavy accent bar running from ≈ (60, 225) to (197, 240) is
**constant width for its whole visible length** and its left end is **cut square** in open
white. There are two little black **rectangles** at ≈ (172,187)–(197,196) and
≈ (171,203)–(195,212), blunt on both ends. And the group at ≈ (25,270)→(200,315) is four
hairlines of identical weight and identical length at equal 1 px gaps — a comb, not a
peak. METRICS reports `caps 0`; that measurement is not catching these.

**`sea-urchin-flash-tight` — 1 / 5.**
Everything converges to a ~10 px white point at ≈ (430, 245) in the shrunk PNG. The
densest, blackest part of the image is the middle. A flash is フキダシの一種 — a speech
balloon — so the middle is the one place that must stay open and readable (REFS, "What the
two effects actually are"). Here it is the opposite. Every stroke is a hairline: METRICS
w p50 0.04 mm, p95 0.08 mm, p95/p50 = **2.00**, which is the number REFS' own note calls
"machine". Not one heavy accent in the whole panel. At 1:1 the crop is a fan of perfectly
ruled, perfectly even lines with no taper you can see. This is `dense-saturated-line` with
a smaller radius, not a tight urchin flash.

**`solid-flash` — 2 / 5.**
The good part: there is a fat usable white hole (33.6 mm), its boundary is a zigzag not a
circle, and the black teeth point the right way — fat outward, needle inward — which
matches ref-22 (white slivers widest at the hole, narrowing to a hair going out).

The bad part is count and field. METRICS says 11.3 strokes per 25 mm and bundles
`1×37 2×2 3×1 4×1`, i.e. about 40 teeth. ref-21 measures **354** white spike tips on a
comparable shape. At 40, each tooth is a huge triangle and each white sliver is a huge
white triangle, so the whole thing reads as a cracked-window star rather than a burst.
And there is **no black field**: corner ink is 33/33/40/43 %, and in the shrunk PNG white
stripes reach all four panel edges. ref-20 and ref-21 both have solid black corners — the
black runs unbroken to the frame and the white spikes die well short of it. Without that
margin the panel does not read "black panel with a hole punched in it", it reads
"black-and-white pinwheel".

At 1:1, region (60,180)–(260,380): there is an isolated black speck of about 2 × 1 px at
≈ (134, 260) and a 3 px hook at ≈ (185, 335), both floating in clean white. The wedge whose
needle apex is at ≈ (143, 297) has its **fat outer end cut square at x ≈ 190**, in the
middle of white space — it is a detached triangular island that never joins the black mass.
The wedge above it, ending at x ≈ 193, y ≈ 245, does the same. Nothing floats in ref-20 or
ref-21; every black tooth is rooted in the black field.

**`solid-flash-off-corner` — 1 / 5.**
METRICS `1×24` — twenty-four strokes for a whole panel. The result is a set of white
diagonal bands several millimetres wide crossing the panel corner to corner. It does not
read as a burst with an off-panel centre; it reads as a striped background. Compare ref-19,
where the flash also sits off-centre: even the part of it that lands on white still shows
60–80 individual needle-sharp wedges, and the black fuses seamlessly into the panel's own
black at top-left.

The clipping behaviour looks right — where teeth meet the frame they are cut on a straight
line at full strength, no fade, which is REFS target 5. But at 1:1, region
(120,230)–(340,450), the black/white boundary runs **dead straight** from ≈ (135, 445) to
≈ (340, 295) — that is 9 mm of paper with no incident — and its only texture is a uniform
3–4 px rasterisation staircase, the same tooth repeated a hundred times. That regular comb
is the loudest "generated" tell in the whole sheet. In ref-21/22 you never get 9 mm of
straight boundary; a peak lands every ~2 mm.

### Things numbers miss

- **Repeating rhythm.** In the shrunk `sea-urchin-flash` the same motif — one heavy dark
  wedge with a pale fan beside it — recurs at roughly even angular spacing around the ring
  (~20–24 times). It reads as a stamp repeated, not as a hand working around a circle.
- **Blunt ends that `caps 0` did not catch** (see the 1:1 findings above): square-ended
  bars and rectangles in open white in `sea-urchin-flash-crop`, square-ended islands in
  `solid-flash-crop`.
- **Speck debris.** Isolated 1–3 px black dots inside the white hole of `solid-flash`.
  On paper these read as dirt on the scan.
- **Bundle structure is absent everywhere, and METRICS already says so.** The bundles
  column is `1×209 2×16 3×1 4×1` (urchin), `1×210 2×14` (tight), `1×37 2×2 3×1 4×1`
  (solid), `1×24` (off-corner). Read plainly: almost every stroke is a loner. REFS calls
  the bundle model "the highest-confidence structural fact in the whole pack" (three
  independent sources: ref-24's まとまり=3 with 2本/3本/4本 labels, ref-22's 4–10 slivers
  per visual spike, egaco's 4–6 lines per big peak).

---

## Fixes — at most 4, ranked by how much they move the score

### 1. Make bundles that form PEAKS, not lone rays (all four panels)

**What is wrong.** Strokes are placed one at a time. METRICS' bundles column shows 209 /
210 / 37 / 24 solitary strokes across the four panels. Where a cluster does happen — e.g.
`sea-urchin-flash-crop` at ≈ (25,270)→(200,315) — its members are the same weight and the
**same length**, sitting at equal gaps, so the cluster's outer envelope is a flat-topped
stripe. Your eye reads a comb.

**What the refs do.** ref-23 is a professional drawing a red sawtooth over the boundary
with the caption 「このジグザグをつくる」 — *make this zigzag*. Look at what makes it: a
pack of ~5 strokes leaves the black mass shoulder-to-shoulder, and **the middle one is the
longest**, so the pack's tip is a triangle. Then a wider gap, then the next pack, shorter.
The zigzag comes from length variation *inside* a pack, not from spacing the rays out.
ref-24 states the parameters outright: まとまり (grouping) = 3, 乱れ (randomness) on, and
the example is hand-labelled 2本 / 3本 / 4本.

**Do this.** Generate M bundles of a random **2–6** strokes. Inside a bundle the gap
between neighbouring strokes ≈ one stroke width (ref-22). Between bundles the gap is
**3–8×** that. Give each bundle a peak length and taper its members' lengths away from the
centre member (longest in the middle, shortest at the edges) so the bundle silhouette is a
triangle. Vary peak length bundle to bundle — big peaks 4–6 strokes, small peaks 2–3
(egaco). Target ≈ 40 peaks around the whole ring (ref-23), which at 2–6 strokes each lands
tip count in the 150–350 band REFS asks for.

**How you will know it worked.** The METRICS bundles column stops being dominated by
`1×…`. Visually, the outer edge of `sea-urchin-flash` stops being "lone lines shooting past
their neighbours" and becomes a row of little triangles.

---

### 2. `solid-flash` + `solid-flash-off-corner`: give it back a black field, and put 5–8× more, finer white slivers in the band

**What is wrong.** Two separate faults with the same root: too few, too fat alternations.

*(a) No black field.* Corner ink is 33/33/40/43 % on `solid-flash`; in the shrunk PNG
white stripes reach all four panel edges. `solid-flash-off-corner` is worse — white bands
several mm wide run the panel corner to corner (ink 83/14/88/65, and the 14 % top-right is
the hole, so the "black" side is stripes).

*(b) Too few teeth.* ~40 on `solid-flash` (11.3 per 25 mm), ~24 on `solid-flash-off-corner`
(7.0 per 25 mm).

**What the refs do.** ref-20 is an all-black rectangle with a white burst punched through —
the black runs to the panel edges and the corners are **pure black**. ref-21 the same: the
white core is the middle ~45 % of the panel width, the spike band fills most of the rest,
and there is still an unbroken black margin of roughly 10–15 % of the panel width all round
plus fully solid corners. ref-21 measures **354** white tips; ref-22's close-up shows each
visual spike resolving into **4–10 parallel white slivers**.

**Do this.** Make the white slivers taper to nothing before they reach the frame, so the
outer 10–15 % of the panel (and every corner) is unbroken black. Then raise the alternation
count in the band by about 5–8×: aim for ~250–350 white tips on `solid-flash`, and enough
on `solid-flash-off-corner` that no white band is more than ~1 mm wide where it crosses the
frame. Keep the current direction — white widest at the hole, narrowing monotonically
outward (ref-22) — just make each sliver much thinner.

**Side benefit.** This also fixes the 9 mm serrated straight boundary in
`solid-flash-off-corner-crop` (135,445)→(340,295). With ~40 peaks around the ring the
boundary never runs straight long enough for the 1-bit staircase to become a visible comb;
the raggedness comes from the arrangement of many clean slivers, which is how the refs do
it, rather than from noise on one long edge.

---

### 3. Every stroke must run out to a point, and nothing may float

**What is wrong (all four panels, only visible at 1:1).** METRICS reports `caps 0`, but
the crops are full of blunt square ends in open white — no frame anywhere near them:

- `sea-urchin-flash-crop`, the heavy bar ≈ (60,225)→(197,240): constant width its whole
  length, **left end cut square**. Also two black **rectangles**, blunt at both ends, at
  ≈ (172,187)–(197,196) and ≈ (171,203)–(195,212).
- `solid-flash-crop`: the wedge with its needle apex at ≈ (143,297) has its fat end
  **cut square at x ≈ 190**, floating free in white — a detached triangular island. Same
  for the wedge ending ≈ (193,245).
- `solid-flash-crop`: loose black specks at ≈ (134,260) (about 2 × 1 px) and ≈ (185,335).

**What the refs do.** REFS target 2: width tapers monotonically from a fat base to a
**single-pixel** tip, base:tip at least 8:1. ref-13, ref-17 and ref-21 have no blunt ends
anywhere. Where ref-17/18 *do* break up, it is dry-brush dashes — irregular, tapering,
clearly ink running out — not clean rectangles with machine-square ends. And in ref-20/21
every black tooth is rooted in the black mass; nothing floats.

**Do this.** Taper every stroke to 1 px at its far end. Root every black tooth in the black
field (or clip it on the frame) — no free-floating triangles. Drop connected components
below ~6 px. If you want the hand-inked dash break-up from ref-17, make the dashes
irregular in length and tapered at their ends, not rectangles.

---

### 4. Fix the sea-urchin family: rough the HOLE, darken the band, and give `-tight` an actual hole

**What is wrong.**
*(a) `sea-urchin-flash` has it backwards.* The hole is a near-circle of radius ≈ 110 px
around (400,255) in the shrunk PNG while the outline is wild (METRICS 21.7 % inner vs
27.2 % outer). REFS target 3 says the hole must be **rougher than, or at least comparable
to,** the outline — ref-12 measures 20 % inner / 11 % outer; ref-16, the wrong answer,
measures 12 % inner and reads as a clean ellipse. As rendered we are on the ref-16 side.
*(b) The band reads grey.* 6–8 % ink, big white gaps everywhere; ref-12's ring is a black
mass.
*(c) `sea-urchin-flash-tight` has no hole* — everything converges on a ~10 px dot at
≈ (430,245) — and no weight mix at all (p95/p50 = 2.00, nothing wider than 2 px).

**What the refs do.** ref-17: every stroke is **entered fat right at the hole and whipped
out thin**, so the fat starts stack against the hole edge with visible gaps and overlaps —
a fuzzy band 4–6 stroke-widths thick, not a boundary. ref-18 goes further: one 70° arc of
the ring has **fused into a solid black patch** where the fat starts ran together, and REFS
says that partial fusion is correct, the state between ウニフラ and ベタフラ.

**Do this.** Push the weight into the inner end: fat base at the hole, whip out to 1 px.
That both roughens the hole (the stacked fat starts chew it) and darkens the band near the
hole where ref-12 is blackest. Jitter each bundle's start radius so the hole edge varies by
≥ 15 % of the mean radius, and let one arc of bundles start close enough to fuse. Tame the
outer edge at the same time — stop single strokes shooting 40–60 px past their neighbours;
let the bundle envelope, not the individual stroke, set the silhouette. For `-tight`,
target the same shape at a smaller scale: keep a real white hole (usable for text, per
フキダシの一種) and carry the same weight mix — right now it has neither.

---

## Verdict

**NOT CLOSE** — two of the four panels are currently a different effect entirely
(`sea-urchin-flash-tight` is a 集中線, `solid-flash-off-corner` is diagonal stripes), and
the bundle model that three independent references agree on is absent from all four.
