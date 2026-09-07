# Plan 2026-09-07: lean gauntlet for the flashes and the next manga effects

> **STATE: written 2026-09-07 by Fable, after the effect-lines gauntlet of 2026-09-06 (PASS 10/10, but
> ~1.17M subagent tokens over 4 builder + 4 critic rounds). Nothing built yet. NEXT: Lane F (flashes)
> with the lean loop below, then pick from the effects list with the owner.**

Owner, 2026-09-07: "our gauntlets were a bit too bloated ... make the flash effects (and look into
all kinds of other manga effects that could be useful to add) on a bit of a more minimized but still
good gauntlet".

## What was bloat last time, and the fix

| what happened | cost | fix |
|---|---|---|
| Fable found two bugs reading the base lane's diff, paid a builder round to fix them | 84k | base lane brief lists the known traps up front (half-pixel floor, sweep density) |
| critic 1 spent a third of its list on the harness (which PNGs are 1:1, which are shrunk) | ~40k + a round | the critic brief always states the harness format; the harness writes a `README.txt` beside the PNGs |
| the base lane eyeballed ("bundles show uneven holes") and was wrong; the biggest bug (half the runs off-panel) survived to critic 1 | one full round | the base lane MEASURES with a script and tunes to numeric targets before any critic looks |
| builders got 6–7 items per round, ran 200k+ each | most of the spend | ≤ 4 items per builder round, 2 rounds max |
| the two flashes were "parked" but still scored every round | critic attention | score only what is in scope |

**The lean loop, per effect kind (~4 agents, target ≤ 500k):**
1. **Builder A** — build (or retune) + extend the harness + run `effect_metrics` (below) + tune until every numeric target is met. Reports the metrics table. No critic yet.
2. **Critic 1** (fresh Opus, gets refs + PNGs + README + the rubric) — eyes only, for what numbers cannot see. ≤ 4 fixes, ranked.
3. **Builder B** — those ≤ 4 fixes, re-measure.
4. **Critic 2** — final verdict + ship note. Stop. A third round only if Fable judges the last verdict is one clear fix away.

**`effect_metrics`** — NEW `crates/core/examples/effect_metrics.rs` (or a helper module the sheet example
calls): for each rendered panel print strokes per 25 mm at the panel centre, perpendicular width
p5/p50/p95 in mm at a ring/band, round-cap count (the round-2 detector, re-implemented from the
gauntlet REPORT), ink % per quarter, bundle-size histogram, inner-end radius σ (radial). Targets per
preset live in a table in `presets.rs` tests so a regression fails CI, not a critic.

## Lane F — the flashes (ウニフラッシュ / ベタフラッシュ)

What the final critic said: every tooth identical and evenly spaced; sea-urchin's thickest stroke is
1.9× its median with zero accents; solid-flash's core is a perfect circle (its ring scan starts at
`r_in` whatever the teeth do, so `core_jit` cannot reach it); 65 round caps in the two of them.

Build (Builder A): `UrchinParams` gets `Mix` (accents = some teeth 2–4× wider at the base, per-tooth
length via `len_skew` + `core_jit` on the apex radius, angular bundles via `radial_angles`, `sweep`);
the solid variant's ring inner edge follows the jittered apex radii (a ragged blob, not a circle);
teeth tips floor at half a pixel (no dashing) but the tooth's BASE at the rim keeps a straight cut
(a flash base is clipped by the frame, not rounded). Reference: ref-08 top (the black wedges are
what a solid flash's teeth look like), plus find one real ウニフラッシュ page — ask the owner for a
scan before starting; a flash judged against nothing is what parked them.

## Effects worth adding next (owner picks; ranked by how often a Jump page uses them)

1. **カケアミ (kakeami, cross-hatched mesh)** — the shading workhorse; CSP ships it as brushes and
   materials. Generator: 2–4 layers of short parallel hatch strokes at rotated angles, density falloff
   from an edge or a point, stroke length/weight wobble. Placement: a lasso or a rectangle, feather.
2. **Curved speed lines along a stroke** (曲線流線) — ref-07's arc. Runs follow a drawn curve or a
   curve ruler instead of a straight direction; everything else the stream renderer already has.
3. **Rain / snow / falling lines** — drip lines are 80 % there; rain wants a slant, a length spread and
   two depths (near = heavy short, far = hairline long); snow is dots with size jitter.
4. **斜線 / 平行線 hatching for shadow** (single-direction hatch, tone-like) — the same walk as stream
   lines with short runs inside a region; pairs with kakeami.
5. **点描 / 砂目 stipple** — dot fields with density falloff; cheap, and the tone engine has half of it.
6. **Light rays with tone** (光線: white rays cut out of a gradient tone) — a focus set used as a mask
   over a tone layer; more of a compositing feature than a renderer.
7. **Impact burst** (ref-07 second panel's white star + black wedges) — dark-burst + a solid white hole
   with a spiky edge; probably a preset of Solid flash once Lane F lands.

Each of these gets the lean loop above, one at a time. Kakeami is the one to do first if the owner
agrees: it is the most-used effect that the app cannot make at all today.
