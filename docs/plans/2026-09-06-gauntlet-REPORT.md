# Gauntlet report — effect lines (Part A, after Lane A1)

Builder rounds against `docs/plans/2026-09-06-effect-lines-parity.md`. One agent at a time; the
critic scores the PNGs in `target/effect-lines/`. Nothing here is committed — Fable reviews the diff.

---

## Round 0 (fixes) — two renderer bugs A1 left behind

Not tuning. Two things the A1 report flagged as open questions, both wrong in the renderer rather
than in a preset number. Only `crates/core/src/genlines.rs` changed.

### Fix 1 — dotted needle tails

**Was:** `segment` tests a pixel centre against `hw × profile.width_at(t)`. With `taper 1` and a
needle exponent that half-width runs to 0, and once it drops under ~0.35 px the hard-edged test
catches only the odd pixel centre — so the last sixth of every tapered stroke printed as a DASHED
line. Plainly visible in `saturated-line-crop.png` and `stream-line-crop.png`.

**Now:** the ramped half-width is floored at half a pixel — `let hwt = (hw * prof.width_at(t)).max(0.5);`.
A pen needle is a 1 px line until it ends; the ramp now reaches "one pixel wide" and holds it to the
tip. The comment at the call site says why and names the trap.

**Bit stability:** both callers already floor `hw` itself at 0.5 (`(w * 0.5).max(0.5)` in
`render_focus`, `hw.max(0.5)` in `render_speed`), and a zero profile returns exactly `1.0`, so
`hwt == hw` on every layer saved before the parity round and the pinned rasters do not move.
`legacy_renders_are_bit_stable` and `pre_flash_specs_load_with_the_old_meaning` pass unchanged, no
fingerprint re-pinned. A layer saved WITH a taper > 0 does redraw — solid tails instead of dotted
ones. That is the intended fix, and it is stated in the comment.

**New test `needles_end_solid_not_dashed`:** one long ray, `taper 1` + `needle 1.2`, walked along its
axis from base to tip; the longest hole inside the inked run must be ≤ 1 px, and the run must cover
the whole ray. Verified it BITES: with the `.max(0.5)` removed the test fails, with it in place it
passes.

### Fix 2 — a sweep without bundles doubled the density

**Was:** `radial_angles` only walked when `gap_deg > 0 && group > 1`. A sweep with a gap but no
bundling fell through to the "spread `count` evenly over the arc" branch — and for a gap-driven set
`count` is `ray_count()`'s 360/gap. So a 3° fan swept to 170° drew all 120 rays inside 170°, at
double the asked-for density.

**Now:** the walk runs whenever `gap_deg > 0` and either a sweep or a bundle is asked for. `group <= 1`
is a walk with no holes: `group.max(1)` keeps the modulo off zero and the hole multiplier is 1×. The
even-spread branch is now only reached with `gap_deg == 0` + a sweep, where `count` is the stored ray
count and means what it says. `ray_count()` agrees for free — it already returns the walk length when
`radial_angles()` returns `Some`.

`gap_deg` set with NO sweep and NO bundle still returns `None` (the legacy even full circle), so
`gap_deg_derives_the_ray_count` is untouched.

**New test `sweep_keeps_the_gap`:** gap 3°, sweep 180° → 59–61 rays (60 steps, ±1 for whether the ray
landing on the far edge clears the float compare), not 120; and a ring sample measures 3° gaps with
nothing at 1.5°.

### Gates

```
cargo test -p mn-core genlines
test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 803 filtered out; finished in 38.32s

cargo check --workspace --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 56.73s
```

Warnings: **0**. 29 tests before, 31 now — the two new ones. Nothing re-pinned.

`cargo run -p mn-core --example effect_lines_sheet` re-run; `target/effect-lines/` is fresh
(12 panels + 12 crops + `sheet.png`).

### Eyeball check

- `saturated-line-crop.png` (1:1, 600 × 600): every ray is now an unbroken hairline all the way to
  its inner end. No dashes, no dotted tails anywhere in the crop. The one heavy accent stroke still
  reads as a wedge.
- `saturated-line-centre-below.png`: the fan is at its asked-for pitch — clumps of a few rays with
  uneven holes between them, the angular walk doing its job — not the packed even fan the old
  even-spread branch produced.

### Not done here

- Anti-aliasing (`segment` coverage + `recolor` × alpha) is still the proper fix for sub-pixel
  strokes and still sits in "Later / not this round". The floor is the pen-behaviour answer, not an
  AA substitute.
- No preset number was touched. Tuning is the gauntlet's job, from Round 1 on.
- `genlines.rs` is still over the plan's ~2 200 lines (A1's open question 2). Untouched — Fable's call.

---

## Round 1 (builder) — density, weight, ragged cores, drips

Critic verdict was FAIL, 0/12. Six items were asked for, in order. Measured before and after with a
throwaway probe example (rendered the same panels the harness does, counted strokes across a 25 mm
line and ink-run widths on the ring at r = 0.6 · reach). The probe is deleted; only `genlines.rs`,
`genlines/presets.rs` and `examples/effect_lines_sheet.rs` changed.

### 1. Where the density went — it was NOT `place` or the presets

The placed spec for `dense-stream` already carried `gap_px 14.17` (= 0.600 mm at 600 dpi),
`group 6`, `group_gap 2.0`, and `saturated-line` already carried `gap_deg 3.0`, `group 4`. Printed
both specs; nothing was dropped. The walk in `render_speed` also laid the runs out correctly.

The loss was one line further on — the ALONG-the-direction offset:

    rand() * (w.max(h) + len) - len - len * 0.5

It scatters a run's start over the canvas PLUS a whole run length, then shifts it back by another
half length. At any cross-section only about `len / (extent + len)` of the runs are present — for a
panel-crossing 流線 that is half of them, and the extra half-length shift throws the rest off the
left side. Measured: 14 strokes per 25 mm where the 0.6 mm gap and the bundle walk ask for ~36.

Fixed by fitting the run inside the canvas's along-extent instead: `a_lo + rand() * (extent − len)`.
Negative slack still covers the extent end to end, so a long run always crosses and a short one
lands anywhere inside — the reference's "many never cross the panel" without the bald half. Guarded
on `gap_px > 0` (the walk); the legacy scatter is untouched and still pinned bit for bit.

| stream preset | strokes per 25 mm, before → after |
|---|---|
| dense-stream | **14 → 34** (walk asks ~36) |
| perspective-stream | 11 → 21 |
| sparse-stream | 7 → 10 |
| stream-line | not captured before; 19 after (walk asks 18.2) |
| drip-lines | 9 → 9 (re-spent as pairs, see 6) |

New test `placed_presets_keep_their_density`: the placed dense-stream spec's `gap_px` is within
0.05 px of 0.6 mm at 600 dpi with `group 6 / group_gap 2.5`, and saturated-line's angular density
reaches the spec and produces 100–120 rays over the circle.

**Bundles made visible.** A 1.5× hole under a 0.35 position wobble is not a hole — the wobble was
half of it. Radial holes went to 3× (dense-saturated 2.5×) with the wobble down to 0.25–0.3; stream
holes are all ≥ 2.5×. Bigger holes cost rays, so the gaps were re-priced to keep the count:
focus 3.0° → **2.2°** (109 rays, not the 80 a 3° gap with a 3× hole would give), dense-focus
2.0° → 1.6°, dark-burst 1.5° → 1.0°.

> **Deviation from the brief, stated:** the brief's test text said "saturated-line has gap_deg 3".
> It is 2.2 now, for the reason above, and the test pins 2.2. Also `sparse-stream` and `drip-lines`
> got pair-bundles (group 2) they did not have — cross-cutting bug 6 was "no bundling anywhere" —
> with their gaps cut to 1.1 mm and 0.7 mm so the MEAN pitch stays what it was (2.5 mm each).

### 2. Harness downscale — the premise was wrong, changed anyway

Measured every PNG's grey-level count before touching anything. **The ×3 panels were already
256-level greyscale** (`image`'s Triangle resample); the files the critic measured as "2 grey
levels" are the **1:1 crops**, which are 1-bit *by design* — hard-edged ink at print scale.

Triangle is still the wrong kernel here: at a 3:1 ratio it reaches ±3 source pixels, so one 1 px
hairline comes out as two soft half-tones instead of one honest grey. Swapped for an exact 3×3 box
average (an output pixel's grey IS the block's ink coverage). Panels now carry exactly 10 levels —
0/9 to 9/9 — which is the correct quantization for a 3×3 block of a 1-bit source, not a loss.

### 3. Sub-pixel tips in the flashes

`Tooth::hit` had no half-pixel floor, so both flash kinds dotted out at the apex where `segment`
(fixed in round 0) does not. Same one-line floor applied. `urchin_flash_spikes_are_filled_wedges`
and `solid_flash_inverts_the_ring` pass unchanged; the change only adds pixels within half a pixel
of a spike's own axis, and no flash kind carries a fingerprint pin.

### 4. Hairline weight and a continuum

The 1 px median was the PROFILE, not the nominal width: `taper 1` + `needle 1.2` drives the ramp to
zero, so at r = 0.6 · reach a saturated-line ray was drawing at 49 % of its nominal half-width.

- `taper` 1.0 → **0.9** (in `from_mm`, so every preset), `needle` → **0.8–1.0** (≤ 1 keeps weight
  along the stroke and needles only at the end).
- base widths up: saturated 0.30 → 0.35 mm, dense-saturated 0.25 → 0.30, dense-stream 0.15 → 0.17,
  drip 0.12 → 0.16. `jit_width` ≤ 0.4 everywhere, so the thinnest nominal line is ≥ 0.10 mm.
- accents are a SPREAD now: `1.5 .. accent_mul` drawn uniformly instead of a fixed multiplier
  (guarded by `accent_frac > 0`, documented on the field), with `accent_frac` 0.18–0.25 on the
  radial rows and 0.12–0.15 on the streams.

Width histogram, perpendicular runs on the ring r = 0.6 · reach, px at 600 dpi (0.042 mm per px):

| preset | before p10/p50/p90/max | after p10/p50/p90/max |
|---|---|---|
| saturated-line | 1.2 / **2.8** / 5.0 / 28.0 | 3.5 / **4.5** / 6.5 / 18.8 |
| dense-saturated-line | 1.2 / **2.2** / 4.5 / 14.0 | 3.0 / **4.0** / 10.2 / 20.2 |
| dark-burst | 3.5 / 6.5 / 22.0 / 44.8 | 5.0 / 7.0 / 27.2 / 63.2 |

p50 on saturated-line went 0.118 mm → 0.189 mm; p10 (the hairline end) 0.05 mm → 0.148 mm, past the
0.08–0.15 mm a G-pen hairline holds. Perspective-stream's heaviest stroke went 2 px → 13 px, so it
has rails beside the hairlines for the first time.

### 5. The core is a clean circle

New spec field `core_jit` (`#[serde(default)]`, 0 = today, its own `rand()` guard) on both
`FocusLinesParams` and `UrchinParams`: the inner end drops BELOW `r_in` by up to `core_jit · r_in`,
clamped at 0. `length_jitter` could only ever pull ends OUTWARD, so with a length skew a crowd of
rays landed on `r_in` exactly — the compass circle the critic saw. Presets 0.3 (dark-burst 0.35),
flashes 0.3, and the flash jitters raised to angle 0.35 / length 0.5.

New test `inner_ends_stagger_not_ring`, std-dev of per-ray inner-end radii on a 150 px hole:
**0.5 px** with core_jit 0 → **12.1 px** with core_jit 0.3 alone → **17.4 px** as shipped
(core_jit 0.3 + length jitter), past the 0.1 · r_in the eye needs. One-sided uniform noise can never
reach 0.1 · r_in on its own (0.3/√12 = 0.087), which is why the test states both halves.

### 6. Drip lines

Harness drag starts at y = 0 of the panel, `jit_start` 0.15 → 0.10, `jit_len` 0.7 → 0.8,
`len_skew` 0 → 0.3. The lengths needed a renderer change too: `place` gave an anchored stream the
panel DIAGONAL, so `jit_len` had to eat 45 % before a run even stopped inside the panel and the
bottom quarter stayed blank. `start_mode 1` now gets the distance from the reference line to the far
edge — the panel height for top-edge drips. Ink 79 k → 92 k, depths run from ~1/8 to the full
height, every line touches the top edge. `sparse-stream` got `accent_frac 0.12` as asked.

### 7. Gates

    cargo test -p mn-core genlines
    test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 803 filtered out; finished in 58.19s

    cargo check --workspace --all-targets
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 40.65s

Warnings: **0**. 31 tests before, 33 now (`placed_presets_keep_their_density`,
`inner_ends_stagger_not_ring`). `legacy_renders_are_bit_stable` and
`pre_flash_specs_load_with_the_old_meaning` pass UNCHANGED — no fingerprint re-pinned.
`cargo run -p mn-core --example effect_lines_sheet` re-run; `target/effect-lines/` is fresh.

### Eyeball check (Read tool, honest one-liners)

- `sheet.png` — twelve panels; the three streams read as bundles-then-hole with a few black rails
  through them, the three bursts have real wedges against hairlines, and both flashes still look
  machine-made next to the rest.
- `saturated-line-crop.png` — hairlines, mediums and two heavy wedges in the same 600 px patch,
  every end a solid needle, no dashes anywhere; the ray angles still clump a little too evenly.
- `dense-stream.png` — genuinely dense now, a fine grey field with black strokes cutting through it
  and visible white lanes between bundles; the fan is still perfectly parallel.
- `dark-burst-crop.png` — the best of the set: 2 mm black wedges interleaved with hairlines, no
  periodicity I can see; a few wedges merge into slabs at the rim.
- `drip-lines.png` — every line hangs off the top edge, depths from an eighth to the full height,
  pairs visible; the whole set still reads a touch evenly spaced and uniformly thin.

### Not done, out of scope (stated)

- **Rebuilding the flash kinds** and **anti-aliasing** — both explicitly out of scope. The
  consequence to name: `solid-flash`'s white core is still a perfect circle, because the solid
  variant's ring scan starts at `r_in` whatever the teeth do, so `core_jit` cannot reach it. That
  needs the ring scan rewritten. `sea-urchin-flash` widths are still near-uniform (11–15 px) for the
  same reason — the tooth width is a clamped constant.
- Perspective-stream's vanishing knot still sits off the panel and the top-right stays empty; it was
  not on this round's list.
- `genlines.rs` is ~2 600 lines. Still Fable's call.

---

## Round 2 (builder) — round caps, off-centre density, rails, broken combs

Critic verdict was FAIL again, 0/10, with `saturated-line` and `dense-saturated-line` failing on
ONE axis (tips) for one reason. Seven items, in the brief's order. Measured before and after with a
throwaway probe example (rendered the same panels the harness does; the probe is deleted). Only
`genlines.rs`, `genlines/presets.rs` and `examples/effect_lines_sheet.rs` changed.

Three new spec fields, all `#[serde(default)]`, all guarding their own `rand()`: `jit_len_out`,
`group_jit`, `jit_angle`. Nothing re-pinned.

### 1. The round cap — the defect that failed the three best presets

`segment` finds a pixel's distance to the SEGMENT, so both ends are half-disc caps. A focus ray's
outer end pulled in by up to `jit_len/2` of the span (0.45 of it at `jit_len` 0.9), and the placed
reach is only the far corner plus a margin — so a heavy accent's outer end regularly landed inside
the frame and printed a semicircular blob.

Two answers, both needed:

- **`jit_len_out` 0.15** (new field; 0 = "use `jit_len` for both ends", the old raster; the `rand()`
  is drawn either way, only the multiplier changes, so the sequence never moves). Nearly every outer
  end is now past the frame where the border hides it.
- **`entry` 0.25** on the focus family. `segment`'s `a` IS the outer end for focus rays (the
  endpoints are swapped so the taper aims at the convergence), so `entry` already applies there —
  confirmed by reading the call, and pinned by the new test. On a ray that does run off the page the
  ramp is spent off-page and invisible.

**Measured** (probe: erode the panel to strokes at least 6 px wide, take each heavy blob's outermost
point, march outward in the full ink — a needle keeps inking, a cap stops within a couple of pixels):

| preset | heavy strokes ending inside the frame | of which ROUND CAPS, round 1 | round 2 |
|---|---|---|---|
| saturated-line | 1 | **5** | **0** |
| dense-saturated-line | 1 | **1** | **0** |
| dark-burst | 19 | **15** | **0** |
| centre-below | 3 | **4** | **0** |
| centre-off-corner | 2 | **2** | **0** |

26 round caps across the sheet → **0**. At 1:1 in `saturated-line-crop.png` and `dark-burst-crop.png`
there is no semicircular end anywhere.

New test `outer_ends_are_needles_not_caps`: one 24 px ray, measured perpendicular to its own axis.
Without `entry` the ink is at least 10 px half-height at its own last pixel (the cap this test exists
to catch); with `entry` 0.25 it is 2 px or less there and back to 8 px a quarter of the way in. It
BITES both ways.

### 2. The two off-centre curtains are their own look now, not `saturated-line` re-aimed

A centre INSIDE the panel spends its rays over 360° and the panel sees all of them; a centre outside
spends them over 360° of which the panel sees ~79°, so the shipped 2.2° gap printed 32 rays for a
whole page. The fix is arithmetic: sweep only the arc the panel occupies and buy the pitch back.

The harness variants now carry their own opts — a `curtain(sweep, gap, width_mm)` closure — with
`jit_width` 0.5, `accent_frac` 0.15, `accent_mul` 6 and `len_skew` 0.25. The **width comes down to
0.16 mm**: 40 strokes in 25 mm is a 0.63 mm pitch, and at 0.35 mm that is over half the paper inked
before a single accent, so the hairlines the critic asked for could not exist at the shipped nib.
Both drags are also longer, so the hole reaches the near frame edge instead of knotting just outside
it.

| variant | sweep / gap | strokes per 25.4 mm at the panel centre | width p5 |
|---|---|---|---|
| centre-below | 170° / 0.42° | **7 → 40** (target at least 40) | 4 px → **1 px** (target 2.5 or less) |
| centre-off-corner | 110° / 0.28° | **6 → 43** (target at least 40) | — → **1 px** |

Rails: 10 and 8 strokes at 3× the median width or more, spread [5,2,3] and [2,2,4] across the panel's
thirds. `len_skew` 0.25 (against the in-panel preset's 0.6) is what gives the length rhythm — the
long bias put almost every inner end on the hole radius, which for an off-panel centre is off the
panel, so every ray ran frame to frame.

**Deviation from the brief, stated:** the brief suggested gap 0.9° for off-corner and 1.0° for
centre-below. Those gaps and the "at least 40 strokes per 25 mm" target contradict each other: at the
off-corner panel centre (r = 1730 px) a 0.9° gap with bundles of 4 and a 3× hole is 14 strokes per
25 mm, not 40. The gaps here are solved from the target instead.

### 3. Rails in the streams and the drips

`accent_mul` 8 on the whole stream family (accents draw uniformly from 1.5..mul, so it is a
continuum up to ~1.6 mm, not a second fixed weight), `accent_frac` 0.15 / dense 0.12 /
sparse **0.22** / drips **0.28**.

**Deviation, stated:** the brief said sparse 0.15 and drips 0.08. A fraction has to be read against
the count it applies to. `sparse-stream` puts ~21 runs on a panel and `drip-lines` ~43, and half the
accent spread lands mild against a thin nib — measured, 0.15 on the drips was TWO visible rails.
0.22 and 0.28 measure as three and eight.

| preset | runs on a full cross-panel scan | p50 | top widths, px | rails at 3× median or more, per third |
|---|---|---|---|---|
| stream-line | 47 | 2 | 11, 11, 11, 10, 8, 8 | 6 — [3, 0, 3] |
| dense-stream | 78 | 2 | 11, 9, 9, 8, 7, 7 | 7 — [2, 4, 1] |
| sparse-stream | 21 | 4 | 18, 15, 12 | 3 — [1, 0, 2] |
| perspective-stream | 53 | 2 | 21, 16, 11, 10, 8, 8 | 7 — [2, 3, 2] |
| drip-lines | 43 | 3 | 23, 21, 18, 17, 15, 14 | 8 — [3, 2, 3] |

**The accent roll is NOT correlated with position** — checked in the code, not inferred: `Mix::accent`
is called once per line inside the render loop, drawing from the shared stream, and the number of
`rand()`s per line varies (an accent costs one extra), so the sequence cannot lock to the walk's
period. The clustering the critic saw was small-N luck: `stream-line` had ~7 accents on the whole
panel. With the new fractions the per-third counts above are the evidence. Only `stream-line` is
still lopsided (3/0/3, an empty middle third).

### 4. `group_jit` — the exact-2 comb is gone

New shared helper `walk_step(group, group_gap, group_jit, seed)`: the next bundle's size is
`group − round(rand·group_jit·(group−1))`, clamped to 1..=group, and the hole after it is
`group_gap·(1 + (rand−0.5)·group_jit)`. `group_jit` 0 returns `(group, group_gap)` and draws NO
random number, so both walks are bit-identical without it. The speed walk moved into its own
`walk_offsets` so the test can read the bundle sizes back off it.

Both walks use it. The radial one takes a `seed` and runs a stream of its OWN (seeded from the spec
seed with a decorrelating splash): `GenLinesSpec::ray_count` calls `radial_angles` outside any render
to size the set, so the walk cannot draw from the per-ray sequence or the two would disagree about
how many rays there are.

Presets: streams `group 5, group_jit 0.7`; sparse `group 4, group_gap 6, group_jit 0.8` (was the
exact pair); drips `group 3, group_gap 6, group_jit 0.9` (was the exact pair); radial `group_jit 0.5`.

New test `bundle_sizes_vary`: with `group_jit` 0.8 over a 512 px band the speed walk's bundles come
in at least 3 distinct sizes and so do the radial walk's, and with `group_jit` 0 every bundle is
exactly `group`.

### 5. `jit_angle`, and the perspective void

New field, degrees, per-run direction wobble of `± jit_angle/2` behind its own guard: 1.0° on
stream/dense/sparse, 0.5° on drips, **0 on perspective** (the convergence already fans every run;
a second wobble only softens the vanishing point).

The perspective void was the along-extent fit from round 1 solving in the WRONG BASIS. A converged
run does not travel along the shared direction, so its start was solved for a direction it never
took and it stopped short. Now the fit runs along the run's OWN direction: a provisional base at the
middle of the extent gives the direction, the canvas extent is projected onto THAT, and the result is
converted back into the (normal, direction) basis the base point is built in. With `converge` None
the conversion is a division by exactly 1 and the whole thing collapses to the line it replaced, bit
for bit — the guard is still `gap_px > 0`.

**perspective-stream ink per quarter:** critic measured the bottom-right at 1.8 % against a 13.9 %
maximum. Now TL 11.1 / TR 8.0 / BL 7.5 / **BR 5.8**, mean 8.1. Target was "no quarter below half the
mean" (4.05) — met, with the emptiest quarter at 72 % of the mean.

### 6. dark-burst tips — one fixed, one explained with a measurement

**Merging heavies.** `Mix::accent` now refuses an accent immediately after an accent: the roll's
`rand()` still happens on every line so no other random number moves, only the draw is refused. New
test `accents_never_land_on_neighbours`. Honest result: it did NOT move the sliver count. Walking
five arcs (r = 300…1100) and counting 1–2 px white gaps sitting between two wedges of 8 px or more
gives **4 slivers out of 156 wedges** now against 3 out of 171 before — noise, both ways. The cap is
still right (two touching 40 px wedges are a slab), but it is not what makes the sliver.

**The rectangular width step.** Measured directly: walk 720 rays outward from r = 200 to 1400 and
count places where the ink width across the ray jumps 8 px or more in one 4 px radial step.

| dark-burst variant | hard width steps | ink |
|---|---|---|
| shipped | **128** (biggest 22 px) | 27.8 % |
| same, gap ×3 so no two rays can overlap | **27** (biggest 19 px) | 9.5 % |
| taper 1.0 + needle 0.5 (a true point at the tip) | 158 | 31.4 % |
| taper 1.0 + needle 0.5, gap ×3 | 44 | 5.5 % |

So the step is **ray OVERLAP, not a profile fault**: removing overlap removes 79 % of them, and the
count tracks ink density almost linearly across every variant. At a 1° pitch a 41 px accent spans
3.4 ray slots, so one stroke's end sitting inside another's body is geometric. The candidate fixes
were tried and are worse: a true-point profile ADDS steps (it carries more weight further, so more
overlap), and `needle` 1.4 halves them only by cutting the ink from 27.8 % to 17.9 % — which throws
away the weight mix the critic scored 5/5. Left alone, deliberately, and stated.

### 7. Gates

    cargo test -p mn-core genlines
    test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 803 filtered out; finished in 63.70s

    cargo check --workspace --all-targets
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 47.94s

Warnings: **0**. 33 tests before, 36 now (`outer_ends_are_needles_not_caps`, `bundle_sizes_vary`,
`accents_never_land_on_neighbours`). `legacy_renders_are_bit_stable` and
`pre_flash_specs_load_with_the_old_meaning` pass UNCHANGED — no fingerprint re-pinned.
`cargo run -p mn-core --example effect_lines_sheet` re-run; `target/effect-lines/` is fresh.

### Eyeball check (Read tool, honest one-liners)

- `saturated-line-crop.png` — not one round end left: every stroke that dies mid-field comes to a
  fine point, and hairlines, mediums and two black rails share the same 600 px patch. The ray angles
  still clump a shade too evenly on the left side.
- `dark-burst-crop.png` — 2 mm wedges against hairlines, ragged core, no blobs; on the right at
  y ≈ 180 two heavy wedges do still nearly merge and the white sliver between them dashes, which is
  the one thing item 6 could not remove without draining the black out of it.
- `stream-line.png` — the rails are scattered top, middle and bottom now instead of piled in one
  corner, and the lines are visibly not parallel; the bundling is still the weakest thing here, it
  reads more as an even sprinkle than as clumps.
- `sparse-stream.png` — the picket fence is gone: singles, pairs and triples with holes you cannot
  find a period in. Both black rails sit in the lower half, so the top two thirds are still all one
  weight.
- `saturated-line-centre-off-corner.png` — was the worst panel in the sheet and is now a real
  curtain: it fills the frame corner to corner, has hairlines against rails, and you can see strokes
  stopping mid-field. It is still slightly too uniform across the panel for a Jump page.
- `perspective-stream.png` — the vanishing knot resolves on the right edge and every quarter now
  carries ink, with rails at the top and through the bottom-left; the far side by the vanishing
  point is still the thinnest part of the panel.

### Not done, out of scope (stated)

- The flash kinds (parked) and anti-aliasing. The sliver in `dark-burst-crop` is an AA problem
  wearing a geometry costume: a 1 px white gap between two hard-edged blacks can only be a dashed
  line until `segment` writes coverage instead of a hit test.
- `genlines.rs` is ~3 030 lines. Still Fable's call.
