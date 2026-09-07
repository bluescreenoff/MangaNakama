# Lane F — Critic 2 (final critic, pictures vs references)

Fresh eyes, 2026-09-07. I read no code and ran nothing. I judged the four in-scope panels —
shrunk **and** 1:1 `-crop` — against the reference pack, and I looked at the refs myself:
ref-12, ref-16, ref-17, ref-18, ref-19, ref-20, ref-21, ref-22.

The renders on disk are timestamped 18:51–18:54, well after critic 1's doc (16:32), so what
I looked at is post-fix work, and `METRICS.md` (18:54) is the post-fix numbers. Critic 1 was
quoting the *old* numbers; where I say a number changed, that is why.

For 1:1 checks I blew up seven regions 3× with nearest-neighbour (no resampling — every
pixel I describe is a real pixel):

| zoom | file | region (x,y,w,h at 1:1) |
|---|---|---|
| `urchin_z1` | `sea-urchin-flash-crop` | 170,180 200×200 (inner / hole edge) |
| `urchin_z2` | `sea-urchin-flash-crop` | 400,250 200×200 (outer fringe) |
| `tight_z1` | `sea-urchin-flash-tight-crop` | 240,200 200×200 |
| `tight_z2` | `sea-urchin-flash-tight-crop` | 0,300 180×180 |
| `solid_z1` | `solid-flash-crop` | 200,80 200×200 (core boundary) |
| `solid_z2` | `solid-flash-crop` | 180,330 200×200 (core boundary) |
| `solid_z3` | `solid-flash-crop` | 420,380 180×180 (outer sliver tips) |
| `off_z1` | `solid-flash-off-corner-crop` | 400,0 200×200 (core boundary) |

Harness reminder: a `-crop.png` is 600×600 px = 25.4 mm of paper. On a flash crop **left is
toward the hole, right is outward, and no panel frame is in shot** — so an end that stops
dead inside a crop is a real blunt end, never a frame clip.

---

## (a) Scores

| panel | score | one-line reason |
|---|---|---|
| `sea-urchin-flash` | **4 / 5** | Reads as a printed ウニフラ at arm's length. Real bundles, ~324 needle tips, hole σ 19.9 % vs outline σ 11.5 % — that is ref-12's 20/11 almost exactly, i.e. the hole is now rougher than the outline. Loses a point because the strokes feather *thin* into the hole instead of entering fat (ref-17/18), so the hole edge is a soft fade, not the hard black serration ref-12 has. |
| `sea-urchin-flash-tight` | **3 / 5** | Now a genuine small ウニフラ with a real usable hole instead of a 集中線 hitting a dot. But it is the one panel that breaks an explicit rubric ban: several bundles taper at **both** ends and float free of the ring as **leaf / petal blobs**, plus square-ended bars in open white. `caps 9`, the worst on the sheet. |
| `solid-flash` | **4 / 5** | A real ベタフラ now: solid black field, all four corners pure black (corner 100 %, ink 85/87/91/88), white burst punched through, sawtooth boundary, white side spiky, `frags 0`, 16 mm hole you could letter in. Loses a point on **density** — ~135 white tips against ref-21's 354, so the fringe reads as cracks in glass rather than ref-20/21's dandelion. |
| `solid-flash-off-corner` | **3 / 5** | Frame clipping is right (straight cut, full strength, no fade) and there is a good chewed core edge in the top-right. But the far half of the band is single white hairlines 25–50 mm long at constant width with wide black gaps, so it reads as rain / speed lines, and the bottom-left ~55 % of the panel is featureless black. |

### Why, panel by panel

**`sea-urchin-flash` — 4 / 5.** At print size this is the effect. Dark ring, white hole, no
petals, no compass circle, no ruled evenness. The bundle model landed hard: the METRICS
histogram is `4×4 5×7 6×6 7×7 8×6 9×10 10×3 12×1 13×1` with only three solitary strokes,
where before it was `1×209`. In `urchin_z2` (the outer fringe) I can see the thing critic 1
asked for: a pack of parallel slivers whose members end at staggered lengths so the pack's
silhouette comes to a point, then a wider gap, then the next pack. Every tip in that zoom
runs out to a single pixel. I found no blunt end anywhere in either urchin zoom.

Two things keep it off 5. First, **the weight is in the wrong half of the stroke.** In
`urchin_z1` — the inner edge — the slivers narrow to needles going *left*, toward the hole.
ref-17 is explicit and ref-18 shows it: a hand-inked urchin is *entered fat at the hole and
whipped out thin*, so the fat starts stack against the hole and chew it into a hard-edged
serration. Ours fades into the hole instead, which is why the hole edge reads soft where
ref-12's reads like a saw. The σ number is right; the *character* of the roughness is at
bundle scale (whole packs starting 3–5 mm apart in radius) rather than at stroke scale.
Second, at roughly 2–3 o'clock the ring thins to near-transparent for about 30° — visible
even at sheet scale — so the annulus does not read as one continuous mass the way ref-12's
does.

**`sea-urchin-flash-tight` — 3 / 5.** Critic 1's complaint is fixed: there is a real white
hole (7.0 mm) instead of a 10 px dot, and the weight mix is no longer flat (p95/p50 4.50,
was 2.00). `tight_z2` shows textbook construction — bundles fused solid at the base, fraying
into needles at the tip, envelope coming to a peak. That part is ref-17/18 quality.

The failure is in `tight_z1`. At real (335,205)–(413,250) there is a black mass that is
**pointed at both ends and fat in the middle** — a leaf — and its inner end sits about
2.8 mm clear of the hole edge, floating in white, rooted in nothing. There is another at
about (295,290)–(360,320) and a third around (245,335)–(300,370). The rubric bans petal
shapes outright, and REFS calls double-tapered spindles the ref-15 *plain-flash* signature.
In the same zoom, three short bars at about (340,218)–(403,223), (365,231)–(430,236) and
(340,240)–(395,244) have **square left ends in open white**. `caps 9` is METRICS agreeing.

Cause is legible from the picture: the bundle's length taper is being applied symmetrically
(shortest members at both ends of the pack, or start radius jittered as far as the peak
length), so instead of a triangle rooted at the hole you get a free-floating lens.

**`solid-flash` — 4 / 5.** Everything critic 1 asked for landed. Corner ink went 33/33/40/43
→ 85/87/91/88 with `corner 100`, so the black now runs unbroken to all four frame edges and
the panel reads "black field with a hole punched in it", which is ref-20 exactly. `frags 0` —
the floating triangular islands and the 2×1 px specks in the hole are gone. `hi/lo 1.75/0.60`
sits inside REFS target 4 (1.5–2.0 / 0.55–0.65). In `solid_z3` every outer white sliver ends
in a one-pixel point. The boundary is a zigzag with real peaks, and the white side has its
own peaks — egaco's acceptance test passes.

What is left is **too few, too long slivers**. 24.5 strokes per 25 mm on an r=22 mm ring is
about 135 white tips; ref-21 measures 354 on a comparable shape and REFS asks for 150–350.
The consequence is visible without measuring: in ref-20 and ref-22 the fringe is packed
shoulder to shoulder with almost no black showing between slivers near the core, and the
spike band is only ~20–25 % of the total radius. Ours has wide black gaps between slivers
and the slivers run most of the way to the frame, so the silhouette reads as a cracked
window rather than a burst. σ 18.3 % is also just under REFS' 20–31 % band.

Second, smaller: **some white slivers start blunt out in the black.** In `solid_z1` the
sliver beginning at about real (300,128) has a square left end while the white core's edge
at that height is at about x = 267 — so it starts 1.4 mm out in the black with a 3 px
vertical cut. Same at about (310,175) and (315,540). In ref-22 every sliver is widest right
at the hole and narrows monotonically outward; none begins in mid-air. `caps 5` is METRICS
catching these. Third: between peaks the core boundary has untextured runs — `solid_z2` has
a dead-horizontal white/black edge about 63 px = 2.7 mm long at real y ≈ 378, x = 227–290,
and `solid_z1` has a clean curved run of ~60 px = 2.5 mm with nothing but a 1 px staircase
on it. In ref-21 a peak lands every ~2 mm and the valleys are chewed too.

**`solid-flash-off-corner` — 3 / 5.** Real progress: it is no longer op-art stripes on white,
it is a black field with a white burst coming in from off the top-right corner, corners
99.8/67.0/100.0/97.3, and where slivers meet the frame they are cut on a straight line at
full strength with no fade — REFS target 5, confirmed by eye at 1:1. `off_z1` shows a
genuinely good chewed sawtooth on the core's lower-left edge around real (510,100)–(560,140),
fine-grained and irregular.

Three faults. (1) **Density collapses with distance.** Out past ~12 mm from the core the band
is single white hairlines, 2 px wide, constant width, 25–50 mm long, separated by 3–8 mm of
black. `off_z1`'s top-left quadrant is five such lines and nothing else. That is rain, not a
flash fringe; ref-19's off-centre flash still shows 60–80 individually visible spikes packed
into a band. (2) **A bald patch on the core boundary**: from about real (555,0) down to
(530,100) in the crop the black/white edge is a smooth arc for ~100 px = 4.2 mm carrying only
a 1 px rasterisation staircase — no peak, no texture. (3) `lo 0.37` is well under REFS' 0.55–0.65
floor, and compositionally the bottom-left ~55 % of the panel is empty black with nothing in
it, so the panel does not really exercise the effect.

### Things the numbers miss

- **Petal blobs are invisible to `frags`.** `frags` counts black blobs ≤ 20 px plus, on a
  solid flash, islands off the main field. A 80 × 45 px leaf floating clear of the urchin
  ring is neither, so `sea-urchin-flash-tight` reports `frags 0` while carrying at least
  three of them.
- **Same σ, different roughness.** `sea-urchin-flash` hits ref-12's 20 % hole σ, but ours is
  low-frequency (bundle start radii scattered over 3–5 mm) where ref-12's is high-frequency
  (serration at one stroke width). Both measure 20 %; only one looks inked.
- **Pinholes inside the black masses.** `tight_z1` and `tight_z2` show isolated single white
  pixels inside fused bundles — e.g. real (307,260), (275,317), (358,380). At 600 dpi these
  are 0.04 mm and will not print, so this is a note, not a fault, but a metric that ever
  counts white components will trip on them.
- **Staircase is no longer the loudest tell.** Critic 1's 9 mm dead-straight serrated
  boundary in `solid-flash-off-corner` is gone; the longest untextured run I could find on
  any panel is 4.2 mm, and it is a smooth arc rather than a repeating comb. The loudest
  remaining "generated" tell is the *uniformity of sliver width* — on both solid flashes the
  white slivers hold one constant width for most of their length and only taper in the last
  few pixels, where ref-22's narrow the whole way.
- **No moiré.** I looked for it where bundles cross at shallow angles in the urchin band and
  found none at either 1:1 or shrunk.

---

## (b) Critic 1's four fixes

| # | fix | verdict | why |
|---|---|---|---|
| 1 | Bundles that form peaks, not lone rays | **LANDED** | METRICS bundle histograms went `1×209` → `4×4 5×7 6×6 7×7 8×6 9×10 10×3 12×1 13×1` (urchin) and `1×24` → `1×4 2×1 3×2 4×1 9×2 15×1` (off-corner); at 1:1 in `urchin_z2` and `tight_z2` the packs visibly come to a triangular point from length variation inside the pack. |
| 2 | Solid flash: black field back, 5–8× more/finer white slivers | **PARTIAL** | The field landed completely (corner 33 % → 100 %, all four corners pure black, white no longer reaches any frame edge). The density did not: ~135 white tips against ref-21's 354 and REFS' 150–350 floor, so the fringe is still coarse and the slivers still run most of the way to the frame instead of dying in a band. |
| 3 | Every stroke runs out to a point, nothing floats | **PARTIAL** | Floating islands and specks are gone (`frags 0` on both solid flashes, confirmed by eye) and every tip I zoomed on the urchin and on `solid_z3` is one pixel. But `caps` went 0 → 5/9/5 under the tightened definition and the ends are really there: square-ended bars in open white in `tight_z1`, blunt sliver starts 1.4 mm out in the black in `solid_z1`. |
| 4 | Rough the hole, darken the band, give `-tight` a hole | **PARTIAL** | Hole/outline inversion is fixed — 19.9 % inner vs 11.5 % outer on the urchin, matching ref-12's 20/11, and `-tight` has a real 7.0 mm hole with a real weight mix (p95/p50 2.00 → 4.50). Not done: the strokes still feather *thin* into the hole rather than entering fat as ref-17/18 require, so the hole edge is a soft fade rather than a hard serration, and no arc anywhere fuses solid the way ref-18's lower-left does. |

---

## (c) What remains — 2 fixes, ranked

### Fix 1 — `sea-urchin-flash-tight`: root every bundle at the hole, kill the leaves

**What is wrong.** Some bundles taper at both ends and float clear of the ring, so they read
as leaves / petals rather than spikes. Concretely, in `sea-urchin-flash-tight-crop.png`
(600 × 600, 1:1) there is a black mass spanning about (335,205)–(413,250) that is pointed at
its left end, fat in the middle and pointed at its right end, and whose left end sits roughly
2.8 mm outside the hole edge with clean white all around it. Two more of the same at about
(295,290)–(360,320) and (245,335)–(300,370). Separately, three short bars in the same crop —
about (340,218)–(403,223), (365,231)–(430,236), (340,240)–(395,244) — have **square left
ends** sitting in open white; METRICS' `caps 9` on this panel is counting them.

**What the refs do.** ref-17 and ref-18: every stroke starts *on* the hole edge, fat, and
whips outward to a hair. Nothing in either image is pointed at its inner end and nothing
floats detached in the white between the hole and the ring. ref-15 — the *negative*
reference — is precisely a field of double-tapered spindles, which is what these leaves are.

**Do this.** Make every bundle member start at the hole radius (with only small jitter,
smaller than the hole's own roughness) and taper monotonically outward only. Apply the
bundle's length taper to the **outer** end alone, never symmetrically. Then: no stroke may
have a square end anywhere except where the panel frame cuts it. If dry-brush break-up is
wanted (ref-17's dashes), the dashes must be irregular in length and tapered, not rectangles.

**How you will know it worked.** `caps` on `sea-urchin-flash-tight` goes to 0, and by eye
every black mass in the crop touches the hole at its left end — nothing floats in the white
annulus between hole and ring.

### Fix 2 — both solid flashes: 2–3× more white slivers, dying in a band

**What is wrong.** `solid-flash` cuts 24.5 strokes per 25 mm at r = 22 mm, which is about
135 white tips around the ring; ref-21 measures **354** and REFS target 1 asks for 150–350.
`solid-flash-off-corner` is worse — 16.2 per 25 mm, roughly 60 tips over the arc that is
actually on the panel — and its slivers are 25–50 mm long at constant 2 px width with 3–8 mm
of black between them, so past ~12 mm out it reads as rain. Secondary symptoms that come from
the same root: white σ 18.3 % on `solid-flash` is just under REFS' 20–31 % band, the
untextured runs on the core boundary (2.7 mm horizontal shelf at about (227,378)–(290,378) in
`solid-flash-crop`; 4.2 mm smooth arc from about (555,0) to (530,100) in
`solid-flash-off-corner-crop`), and blunt sliver starts out in the black (e.g. about (300,128)
in `solid-flash-crop`, starting 1.4 mm clear of the core edge).

**What the refs do.** In ref-20 and ref-22 the fringe is packed shoulder to shoulder — near
the core there is almost no black visible between slivers, and the gap only opens up as the
slivers thin outward. The spike band is only about 20–25 % of the total radius in ref-21: the
slivers **die well short of the frame** and leave a solid black margin, they do not streak
across the panel. Each sliver is widest at the hole and narrows the whole way, and every one
of them starts on the core edge.

**Do this.** Raise the sliver count by roughly 2–3× on `solid-flash` (target ~250–350 tips)
and enough on `solid-flash-off-corner` that no black gap between neighbouring slivers exceeds
about 1.5 mm anywhere inside the band. Shorten the band: slivers should reach roughly 1.3–1.5×
the core radius, not to the frame, so a solid black margin survives all round (keep the
corners at 100 %, which already works). Start every sliver on the core boundary — no blunt
starts in open black. Taper width monotonically from the hole outward rather than holding one
width and pointing only at the end.

**How you will know it worked.** `/25mm` on `solid-flash` roughly triples, white σ climbs
into 20–31 %, `caps` goes to 0, and by eye the panel stops reading as cracks in glass and
starts reading as ref-20's fringe: a dense white ruff around the core with clean black beyond it.

---

## (d) Verdict

**ONE MORE ROUND** — and a short one. All five REFS targets are now substantially met on
`sea-urchin-flash` and `solid-flash`, and the two weak panels are weak in *degree*, not in
kind, which is a completely different position from critic 1's "two of the four are a
different effect entirely". The two fixes above are parameter-and-taper work, not another
rebuild: root the bundles at the hole and stop the symmetric taper (kills the petals and the
square ends), then raise sliver density and shorten the band on the solid pair. Nothing
structural is wrong any more.
