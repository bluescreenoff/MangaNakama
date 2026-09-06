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
