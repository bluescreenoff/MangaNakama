# Effect survey 2026-09-07 — what a Jump page actually uses

> **STATE: DONE.** 24 real published pages read, one by one, right to left. Images live in
> `docs/plans/refs/effect-survey/` — that folder is git-excluded on purpose (copyrighted
> scans, never commit them). This doc checks the "Effects worth adding next" list in
> `2026-09-07-lean-gauntlet-effects.md` against what the pages actually do.

## How the sample was picked

24 pages, half from fight chapters and half from quiet/talking chapters, so the tally is not
just "action manga uses action lines":

| series | chapter | pages read |
|---|---|---|
| Chainsaw Man | 33 (talk + a beat) | 3 |
| Chainsaw Man | 52 (quiet) | 2 |
| Jujutsu Kaisen | 136 (fight) | 2 |
| Jujutsu Kaisen | 90 (fight aftermath) | 2 |
| One Piece | 1000 (fight) | 2 |
| Kagurabachi | 25 (fight) | 2 |
| Dandadan | 60 (quiet) | 2 |
| Sakamoto Days | 105 (quiet + comedy) | 2 |
| Demon Slayer | 194 (fight) | 2 |
| Bleach | 500 (fight + reaction montage) | 3 |
| Naruto | 476 (fight) | 2 |

Pages were read from published chapters; the images stay local and untracked (copyright).

**Right-to-left proof.** One Piece ch 1000, third tier: the sentence *"SOMEONE MUST HAVE KEPT
THIS JOURNAL SAFE… …FROM THE BURNING CASTLE…"* is split across two panels — first half in the
RIGHTMOST panel, second half in the panel to its LEFT. Read left-to-right you get "…from the
burning castle… someone must have kept this journal safe…", which is backwards. The same tier
does it again with "IT CONTAINS EVERYTHING THERE IS TO KNOW… …ABOUT ODEN'S GRAND LIFE". Bleach
500 does the cross-tier version: the last panel of tier 1 (leftmost) says "SO ICHIGO'S…" and
the first panel of tier 2 (rightmost) finishes it with "…ON HIS WAY, HUH!".

---

## (a) Tally — effect → how many of the 24 pages show it → is it in the app?

"Brush-able by hand" means the artist can draw it with the pens we already ship; it is a
missing *convenience*, not a missing *capability*.

| # | effect | seen on | in the app? |
|---|---|---|---|
| 1 | Flat screentone fill (grey areas) | 22 / 24 | **yes** — `tone.rs` |
| 2 | Single-direction hatching 斜線 (shading + whole backgrounds) | 23 / 24 | **no generator**; brush-able |
| 3 | Hand-lettered sound FX | 13 / 24 | out of scope (text/brush) |
| 4 | Tone gradient (soft light→dark tone, vignettes) | 11 / 24 | **yes** — `gradient.rs` painted under a tone layer |
| 5 | Straight speed lines 流線 | 9 / 24 | **yes** — Stream presets |
| 6 | Cross-hatch mesh カケアミ | 8 / 24 | **no**; brush-able |
| 7 | Spiky "shout/flash" balloon フラッシュ吹き出し | 8 / 24 | **no** — balloons are Ellipse / RoundRect / hand-drawn Polygon only |
| 8 | Sweat drops 汗 | 8 / 24 | **no**; brush-able (really a stamp/material) |
| 9 | Focus lines 集中線 | 7 / 24 | **yes** — Focus presets |
| 10 | Smoke / dust / steam clouds | 7 / 24 | **no**; brush-able |
| 11 | Debris, rubble, floating flecks | 7 / 24 | **no**; tedious by hand |
| 12 | Blood spatter / drips / liquid drips | 6 / 24 | **no**; tedious by hand |
| 13 | Aura (flame tongues, fuzzy "hairy" burst, concentric swirl) | 6 / 24 | **partly** — the fuzzy burst is an Urchin variant; flames are art |
| 14 | Vertical-line gloom background 縦線 | 4 / 24 | **yes, unlabelled** — a Stream preset at 90° hanging off the top edge |
| 15 | Flash: white ウニフラ starburst / black ベタフラ teeth | 4 / 24 | **yes** (Lane F rebuild in progress) |
| 16 | Stipple / noise texture 点描・砂目 | 4 / 24 | **mostly** — tone `Noise` pattern; hand stipple is brush-able |
| 17 | Motion smear / after-image (blurred trail) | 3 / 24 | **no** |
| 18 | Cracked-surface texture (ground, bark, walls) | 3 / 24 | **no**; brush-able |
| 19 | Perspective grid / vanishing-point ruled lines | 3 / 24 | **yes** — `ruler.rs` |
| 20 | White spatter knocked out of black キリテン | 2 / 24 | **no** |
| 21 | Blush / embarrassment hatch strokes on cheeks | 2 / 24 | brush-able |
| 22 | Shockwave / impact ring | 1 / 24 | **no** (but it is Kagurabachi's signature move) |
| 23 | Glass gloss / reflection stripes | 1 / 24 | brush-able |
| 24 | Anger / irritation cross mark | 1 / 24 | stamp/material |
| — | **rain / snow** | **0 / 24** | — |
| — | **light rays cut out of tone (光線)** | **0 / 24** | — |

**The headline number:** hatching of some kind is on 23 of 24 pages. Speed lines and focus
lines together are on about 13. Rain and light rays are on none.

---

## (b) Every effect NOT in the app — what it is, what a generator needs, how hard

Difficulty is judged against `genlines.rs`, which is a *seeded, parametric segment painter*:
you give it a centre or a direction, it walks an angle or an offset in steps, jitters each
run's length/width/angle, marks a few runs as heavy "accents", and rasterizes hard-edged ink
into tiles. Anything that is "a lot of similar strokes laid out by a rule" is close to free.
Anything that needs a *region mask*, a *path*, or *soft edges* is a new capability.

### 1. Region hatching 斜線 — single direction, inside a shape, fading out (S–M)
**Looks like:** a patch of short parallel strokes, all leaning the same way, packed tight
where the shadow is dark and thinning out to nothing at the lit edge. On Chainsaw Man 52 it
*is* the artwork — stairs, walls, moss. On Bleach it fills half of every panel.
**Generator inputs:** a lasso or a rectangle (the region), one angle (a drag), and a density
falloff — either "dense at this edge" or "dense near this point".
**Why it's cheap:** the stroke walk, gap jitter, bundling, taper and accents already exist in
`SpeedLinesParams`. The two new pieces are (i) clip runs to a mask, (ii) let the walk's gap
vary across the region instead of being constant. **S–M.**

### 2. Kakeami カケアミ — cross-hatch mesh (M)
**Looks like:** #1 done 2–4 times over the same patch at rotated angles, so it reads as grey
made of visible pen strokes rather than as tone. Used for gloom, heavy shading, and any
background that should not look mechanical.
**Generator inputs:** same as #1 plus "how many layers" and the angle between them.
**Difficulty:** it is literally #1 in a loop with a rotation, so build #1 first and kakeami is
a preset on top. **M** only because the layers must not land in a moiré grid — each layer
needs its own seed and a small angle wobble.
**Owner's hunch is half right:** a brush *can* lay kakeami down, but a brush cannot make the
density follow a shape. The value here is the falloff, not the strokes.

### 3. Spatter / scatter generator — one tool, five effects (S)
**Looks like:** in Jujutsu Kaisen 136 and 90, a cloud of little white blobs knocked out of a
solid-black panel (キリテン kiriten). In Kagurabachi, black ink flecks around a sword and
small chips of floating rubble. In One Piece, blood.
**Generator inputs:** a lasso or a drag (the area), density, size range, and a shape knob
(round dot / elongated chip / irregular splat) plus black-or-white.
**Difficulty: S.** It is `rand()` in a bbox with a size distribution — far simpler than the
line walk. This single generator covers blood spatter, ink spatter, debris flecks, kiriten,
dust motes and star fields, which is four separate rows of the tally above.
**Note:** there is no spray/airbrush/scatter brush anywhere in the codebase today, so nothing
covers this by hand either.

### 4. Flash balloon フラッシュ吹き出し — the spiky shout bubble (S)
**Looks like:** a speech bubble whose outline is a ring of sharp teeth instead of a smooth
oval — "!!", "WHY, YOU…!!", "WHO THE HELL ARE YOU?!". On 8 of 24 pages, including quiet ones.
**Generator inputs:** the same two-point drag that already makes an ellipse balloon, plus
tooth count, tooth depth and jitter.
**Difficulty: S**, and it does not touch `genlines.rs` at all — it is a fourth
`BalloonShape` variant beside `Ellipse`, `RoundRect` and `Polygon`, generated as a star
polygon with jittered radii. It reuses the whole existing balloon pipeline (fill, outline,
tail, text fitting) for free.

### 5. White polarity on the line tools (S — and it is the biggest multiplier here)
**Looks like:** speed lines drawn *white on black* (JJK 136), a white starburst punched
through a black panel (Demon Slayer 194), white "shine" rays behind a happy face (Sakamoto
Days 105), and トーンフラッシュ — focus lines carved out of a grey tone instead of inked on
top of it.
**What's needed:** `GenLinesSpec` already has a `color` field and a test called
*"a white run on a black page is the knockout"*. But `LineOpts` — the struct the tool and the
presets actually use — has no colour field, and the UI never sets one. So the renderer can
already do this and the tool cannot reach it.
**Difficulty: S.** One field, one UI row, a handful of presets. It roughly doubles what the
four existing line generators can produce.

### 6. Curved speed lines along a stroke 曲線流線 (M)
**Looks like:** the streak block bends with the swing instead of running dead straight.
Naruto 476's background streaks bow with the arm; Kagurabachi 25's shockwave arcs.
**Generator inputs:** a drawn curve (or a curve ruler) instead of a single angle; runs sit
perpendicular to it at each step.
**Difficulty: M** — the walk becomes "arc-length along a path" instead of "offset along a
line", and every run needs the local tangent. Everything downstream is unchanged.

### 7. Shockwave / impact ring (S–M)
**Looks like:** a big ellipse ring, filled with short radial teeth, marking where a blow
displaced the air. Kagurabachi uses it constantly; it is that series' visual signature.
**Generator inputs:** a drag for the ellipse plus ring thickness and tooth density; optionally
a partial arc rather than the full ring.
**Difficulty: S–M.** It is `UrchinParams` with (i) an elliptical instead of circular radius
and (ii) a sweep limit — and `sweep_deg` already exists on the focus params. Do it *after*
Lane F lands so it inherits the fixed teeth.

### 8. Fuzzy / "hairy" aura burst (S, after Lane F)
**Looks like:** a black halo made of hundreds of hair-fine tapered strokes with a ragged
outer edge — the aura around Tanjiro in Demon Slayer 194, and the furred edges of its impact
splats. Not the same as ウニフラ: the teeth are hair-thin, wildly uneven in length, and the
outer boundary is a blob, not a circle.
**Generator inputs:** the same centre-out drag as the flash.
**Difficulty: S** *if* Lane F ships `len_skew`, `core_jit` on the apex and per-tooth width
jitter, because then this is a preset, not code.

### 9. Motion smear / after-image (M–L)
**Looks like:** a swung fist whose trailing edge is a grey blur (Chainsaw Man 33), or a
doubled ghost outline.
**Generator inputs:** a selection plus a direction and a length.
**Difficulty: M–L**, and it belongs in `filter.rs` (directional blur on a copied selection),
not in `genlines.rs`. Check whether the existing filter framework already gets most of the way.

### 10. Smoke / dust clouds (L) — recommend NOT building
**Looks like:** the outlined puff-clouds in One Piece 1000, cigarette smoke in Chainsaw Man,
steam off an oden pot in Sakamoto Days.
**Why not:** a convincing cloud is a *drawing decision* — the outline's rhythm is the whole
effect. A generator produces bubble-wrap. This is a brush + material job.

### 11. Small change, real payoff: name the vertical-gloom preset
The 縦線 background (tight vertical lines hanging off the panel's top edge, used for dread)
appears on 4 of 24 pages and the Stream generator can *already* make it — `start_mode = 1`
with a 90° angle and a top-edge anchor. It just is not a preset, so nobody will find it.
**Difficulty: trivial**, one row in `builtin_presets()`.

---

## (c) Ranked "add next" — top 5

1. **White / knock-out polarity on all four line generators.** The renderer already does it
   and the tool cannot reach it. One field on `LineOpts` and a UI row unlocks white speed
   lines, white shine rays, and トーンフラッシュ — three real page effects — with almost no
   new code. Best payoff-per-line in the whole list.
2. **Region hatching (斜線, single direction, density falloff), then kakeami on top of it.**
   Hatching is on 23 of 24 pages; it is the single most-used mark in manga after tone. Build
   the one-direction version first and kakeami becomes a preset with a rotation loop. The bit
   a brush genuinely cannot do is the density following a shape.
3. **Spatter / scatter generator.** One small tool retires four tally rows — blood spatter,
   ink flecks, floating debris, and kiriten (white dots knocked out of black). Nothing in the
   app covers it today, not even a brush, and doing it by hand is pure tedium.
4. **Flash balloon shape.** On 8 of 24 pages including quiet ones, and it is a *balloon*
   change, not a genlines change — a new `BalloonShape` variant that inherits fill, outline,
   tail and text fitting for free. Cheapest visible win on the list.
5. **Shockwave / impact ring + fuzzy aura burst, as two presets after Lane F.** Both are
   `UrchinParams` with a sweep limit, an elliptical radius, or hair-fine jittered teeth.
   Almost no new code if Lane F delivers the tooth mix it promises — so schedule them as the
   thing that *proves* Lane F was general enough.

---

## (d) What I would drop or demote from the old list

| old item | verdict | why |
|---|---|---|
| **カケアミ kakeami — "do this first"** | **demote to second, and build hatching first** | Kakeami is 8/24; plain one-direction hatching is 23/24 and is kakeami's own building block. Doing kakeami first means writing the general case to get the special one. The owner's hunch that a brush covers it is half right — a brush covers the strokes, not the density falloff. |
| **Rain / snow / falling lines** | **drop for now** | 0 / 24 pages. It is a weather effect, not a manga effect: it shows up when it rains, which in a Jump fight arc is a handful of pages a year. Drip lines already cover the "hanging verticals" shape. |
| **Light rays with tone (光線)** | **drop for now** | 0 / 24 pages, and the plan itself already calls it "more of a compositing feature than a renderer". Item 1 above (white polarity) delivers the useful 90 % of it — white rays cut out of a tone — without building a masking pipeline. |
| **Stipple / 点描** | **demote** | 4 / 24, and the tone engine's `Noise` pattern plus a gradient already covers the machine-made version. The hand-stippled version (Dandadan's oil portrait) is deliberately irregular — it *wants* to be brush work. |
| **Impact burst** | **keep, but merge** | The plan already guessed it is "probably a preset of Solid flash". Confirmed on the pages: it is the same shape family as the shockwave ring and the fuzzy aura. Bundle all three as one post-Lane-F preset round rather than three separate lanes. |
| **Curved speed lines along a stroke** | **keep, unchanged** | Real and repeatedly visible (Naruto's bowed streaks, Kagurabachi's arcs). Middle of the pack — worth doing, just not before hatching. |

## Things the old list missed entirely

White/knock-out polarity (#1 above), the spatter generator (#3), the flash balloon shape (#4),
the shockwave ring, the fuzzy aura, motion smear, and the free 縦線 preset. Of those, the
first three are the ones I would actually schedule.
