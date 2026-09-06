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
