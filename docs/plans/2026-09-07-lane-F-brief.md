# Lane F brief — the flashes (ウニフラッシュ / ベタフラッシュ), lean loop

> Written 2026-09-07 by Fable. Parent plan: `2026-09-07-lean-gauntlet-effects.md` (read its
> "lean loop" and "Lane F" sections first). One Opus agent at a time. Fable reviews and commits.

## What exists today (read these before touching anything)

- `crates/core/src/genlines.rs` — `UrchinParams` (~line 318), `Tooth` (~350), `render_urchin`
  (~1030). Filled variant = `fill_tooth` per tooth. Solid variant = one ring scan between `r_in`
  and `r_out` that inks every pixel no neighbouring tooth covers. `core_jit` moves only the
  filled variant's apexes; the solid ring's inner edge is a perfect circle at `r_in`.
- `Mix` (~line 108): `accent_frac`, `accent_mul`, `entry`, `needle`, `len_skew`. Focus and
  stream lines honour it. The urchin renderer ignores it entirely.
- `GenLinesSpec` (~line 2606) already carries the density-round fields (`#[serde(default)]`,
  0 = old meaning). `GenLinesSpec::render` (~2876) builds `UrchinParams` for kinds 1 and 2.
  Find where and thread the new knobs through the SAME fields the focus kind uses — do not add
  fields the spec already has under another name.
- `crates/core/src/genlines/presets.rs` — `LineOpts::flash` (~451) and the two presets
  "Sea urchin flash" / "Solid flash" (~713). `LineOpts::place` turns a drag into a spec.
- `crates/core/examples/effect_lines_sheet.rs` — the harness. Renders every builtin preset into
  `target/effect-lines/` as one panel PNG each (`<slug>.png`, shrunk) plus a 1:1 crop
  (`<slug>-crop.png`) and a contact sheet. Run: `cargo run -p mn-core --example effect_lines_sheet`.
- `docs/plans/2026-09-06-gauntlet-REPORT.md` — last loop's report. Section "Round caps across
  the sheet" (~line 565) has the round-cap detector recipe. The final critic's flash verdict is
  in `2026-09-06-gauntlet-critic-round4.md`.
- Refs: `docs/plans/refs/effect-lines/` (git-excluded, copyrighted). `ref-08` top = black wedges
  that look like a solid flash's teeth. A research agent is adding real flash refs as
  `ref-12-…` onward with a `REFS.md` beside them; use them if they are there when you start,
  and at the latest the critic will have them.

## The defects the last critic named (all four are the job)

1. Every tooth identical and evenly spaced. → per-tooth width accents (`accent_frac`,
   `accent_mul`: some teeth 2–4× wider at the base), per-tooth length via `len_skew` +
   `length_jitter`, angular bundling (teeth cluster in groups with wider gaps between groups,
   the way focus lines bundle — reuse whatever the focus kind does for its bundles).
2. Sea-urchin's thickest stroke is 1.9× its median with zero accents. → target: p95/p50 of base
   width ≥ 2.5, at least 8 % of teeth are accents.
3. Solid flash's core is a perfect circle. → the ring's inner edge must follow the jittered apex
   radii per sector (a ragged blob). The ring scan has to consult the per-sector apex, not `r_in`.
4. 65 round caps between the two. → the tooth's BASE at the rim stays a straight cut (a flash
   base is clipped by the frame, not rounded); tips floor at half a pixel so they do not dash
   (the floor already exists in `Tooth::hit`, keep it). Round-cap count target: 0 on both.

Also: `sweep` for an off-panel centre (the same arc-only sweep the focus kind got in round 2) so
a flash aimed from outside the panel does not spend 64 teeth on the arc the panel cannot see.

## Builder A (this round)

1. Build the four fixes + sweep. Keep `pre_flash_specs_load_with_the_old_meaning` green: a spec
   with the new fields at 0 renders bit-for-bit what it did (there is a fingerprint pin for the
   urchin at ~line 1581 — if the pin must move because the geometry changed with all knobs at 0,
   say so in the report and why; prefer a design where 0 = old).
2. NEW `crates/core/examples/effect_metrics.rs` (or a helper module the sheet example calls):
   for each rendered panel print, as a markdown table:
   - strokes (teeth) per 25 mm at the ring's mid radius,
   - tooth base width p5/p50/p95 in mm, measured at the rim,
   - round-cap count (the round-2 detector, re-implemented from the REPORT recipe),
   - ink % per quarter of the panel,
   - bundle-size histogram (runs of teeth whose gap is < 0.6× the median gap),
   - inner-end (apex) radius σ, in mm.
   Run it on ALL presets, not just the flashes — it is the regression tool for the next effects
   too. Make it fast enough to run in CI (< 30 s).
3. Numeric targets, put them in a `#[test]` table in `presets.rs` so a regression fails CI:
   - urchin: accents ≥ 8 % of teeth, base-width p95/p50 ≥ 2.5, round caps 0, apex σ ≥ 0.4 mm
     at 600 dpi on the harness drag.
   - solid: same accents + caps, and the inner edge is NOT a circle: the inner-edge radius over
     360° has σ ≥ 0.4 mm.
   Tune the two presets until every target is met. Report the metrics table before/after.
4. Harness: write `target/effect-lines/README.txt` stating which PNGs are 1:1 and which are
   shrunk and by how much, the panel size in mm and px, and the dpi. The critic reads this
   first. Add two extra flash panels: a solid flash aimed from an off-panel corner, and an
   urchin with a small hole (r_in_frac 0.15) — a tight burst is where the teeth crowd.
5. Gates, all three, paste the tail of each into the report:
   `./build.sh` (zero warnings), `cargo test -p mn-core genlines`, the sheet + metrics examples.
6. Report: `docs/plans/2026-09-07-lane-F-A-REPORT.md`. Write it INCREMENTALLY (progress note at
   the top: done / next) — API disconnects kill agents mid-round, and a resume must be able to
   read the disk and continue. Commit nothing; Fable commits.

## Critic 1 (after Builder A) — eyes only, ≤ 4 fixes, ranked

Gets: the refs folder + REFS.md, the PNGs + README.txt, the metrics table, and this rubric:
- Does the sea-urchin read as a printed ウニフラッシュ (needles of many weights, some heavy accents,
  groups with breathing room, apexes not on one circle)?
- Does the solid flash read as ベタフラッシュ (a ragged black blob that breaks into outward
  spikes, hole is where the art goes, no perfect-circle edge anywhere)?
- Anything the numbers cannot see (a moiré, a "ruled vector" tell, a tooth that reads as a petal).
Score only the flash panels. Do not re-score the other presets.

## Builder B — the ≤ 4 fixes, re-measure, same gates, `lane-F-B-REPORT.md`.
## Critic 2 — final verdict + ship note. Stop.

## Traps (learned the expensive way)

- Half-pixel floor: any hard-edged test narrower than ~0.35 px prints dots. Keep the floor.
- `f32::clamp` panics on min > max; a tiny flash must not abort through wndproc.
- The solid scan tests only neighbouring sectors' teeth; bundling that moves a tooth more than
  one sector over breaks that — either widen the neighbour window to what the bundling can reach,
  or bundle by re-spacing sector angles (a per-tooth angle table) and look up by binary search.
- No windows, no popups: examples write PNGs only. Never open a viewer.
- Do not "fix" unrelated presets. Score and touch only what is in scope.
