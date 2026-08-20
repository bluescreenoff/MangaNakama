# Vector inking layers — design

Status: DESIGN. Nothing here is built; this document exists so the build
starts from decisions instead of rediscovering them. Read
`docs/CODE-MAP.md` first — almost every constraint below is one of its
seams applied to a new layer kind.

## What it is

A layer whose content is **strokes as editable geometry**: move a control
point, re-edit a stroke's width after the fact, and erase by trimming a
stroke at the intersection instead of deleting it. The ROADMAP's one-line
scope, unchanged.

## The four decisions

### 1. It is a `Layer` FIELD, and the raster is the layer's own tiles

`Layer::strokes: Option<StrokeSet>` beside `tone`/`genlines`/`edge` —
the non-destructive-mode pattern this codebase already runs on (a
`LayerKind` variant was the first draft; the field is smaller and, like
tone layers, convertible both ways by dropping the record). The visible
pixels are the layer's ORDINARY painted tiles: drawing on a vector
layer rasterizes through the normal stroke pipeline exactly as today,
and the stroke is RECORDED beside the pixels. Only an EDIT (move, trim,
re-width) re-derives — clear and replay, damage-limited later. Every
compositor/undo/export path therefore works unchanged in phase 1.

### 2. Rendering replays the REAL brush engine

The derived raster is produced by replaying each stroke's recorded
samples through `MyBrush` with the stroke's own preset — not by a second
"vector renderer" with its own ink look. One ink, by construction; the
CPU/GPU parity chain keeps applying because nothing new makes pixels.
Cost is controlled the way tone layers control it: per-stroke damage
(a moved control point re-renders that stroke's bounding tiles, layered
over the other strokes' cached raster), revision-keyed.

The trap this dodges deliberately: a polyline-fill renderer would be
10× faster and produce ink that does not match the raster brushes —
the manga inker would see the seam instantly. Faithful-first; speed by
damage-limiting, not by a second implementation.

### 3. The stroke stores SAMPLES, not dabs

```
Stroke {
    // The pen samples as captured (x, y, pressure), post-stabilizer —
    // what the artist actually drew, at input resolution.
    points: Vec<(f32, f32, f32)>,
    // The preset by name + the settings snapshot that drew it (a preset
    // edited later must not silently re-ink old strokes).
    preset: String,
    settings_snapshot: ...,
    color: [u8; 3],
    width_scale: f32,   // the re-width edit multiplies pressure->size
}
```

Samples, because every editing operation is sample-space: control-point
move = displace samples under a falloff, re-width = scale the pressure
channel or `width_scale`, trim = cut the sample list at a geometric
intersection. Dabs are a rendering artifact and stay one.

### 4. Vector layers SERIALIZE (unlike rulers)

`.ora`/`.mnc` carry the stroke set as a custom layer attribute (JSON, the
`mnc-*` attribute idiom, new key so old builds ignore it) AND the layer's
rendered raster as its ordinary content — so any other OpenRaster app
opens the file and sees the ink, and a round trip through a foreign app
loses editability, never pixels. The load path prefers the stroke data
and re-derives.

## Editing surface, v1

- **Object tool** selects a stroke (hit = distance to the polyline within
  the zoom-scaled tolerance every other handle uses); its control points
  show as the standard handles. Drag a point = local deform (raised-cosine
  falloff over a radius, so a nib stroke stays smooth); drag the body =
  translate.
- **Width tool** (a sub tool): drag along a stroke scales `width_scale`
  locally (same falloff), the ROADMAP's "width re-editing".
- **Vector eraser**: a stroke with the eraser sub tool on a vector layer
  computes segment intersections between the eraser path and each stroke
  polyline and TRIMS — splitting a stroke into two at the cut. Whole-
  stroke delete is the same tool with a modifier (match CSP's three modes:
  trim-to-intersection is the one artists actually mean).
- Drawing on a vector layer records the stroke (the normal pipeline runs;
  `end_stroke` stores samples + renders into the derived raster instead of
  committing tiles).

## Undo

Whole-set snapshots would copy every stroke on every gesture; a page of
ink is thousands of samples. Per-gesture groups instead:
`UndoGroup::VectorStroke { layer, index, before: Option<Stroke>,
after: Option<Stroke> }` (add = None→Some, delete = Some→None, edit =
Some→Some, trim = one before → the commands records two entries in one
labelled step). One gesture one step, as always.

## Phasing (each lands green on its own)

1. **Core model + derived rendering**: `StrokeSet`, `LayerKind::Vector`,
   replay-rendering into the derived raster, drawing records strokes,
   ORA round trip. No editing yet — the layer draws and saves.
2. **Object-tool selection + move/translate + undo.**
3. **The vector eraser (trim at intersection)** — the headline.
4. **Width re-edit + polish** (stroke smoothing on move, simplify).

## Traps already visible from here

- The derived-raster cache doors (CODE-MAP seam #1): page switch, tab
  switch, undo, DPI change all must reach the vector cache. Enumerate at
  phase 1, not after the first sometimes-bug.
- Replay determinism: MyBrush uses a per-stroke RNG seed — the seed must
  be stored per stroke or scatter/jitter presets re-roll on every
  re-render (a moved control point would reshuffle the whole stroke's
  grain). Seed rides the Stroke.
- `settings_snapshot` keeps imported/retuned presets honest, but it must
  be the SMALL myb-relevant subset, not a texture bitmap — textures load
  by name through the existing loader; a missing texture degrades exactly
  like a missing preset texture does today (usable, untextured).
- Frame layers refuse ink (`guard_frame_layer`); vector layers must make
  the same call about non-brush tools (fill, wand) explicitly — silently
  doing nothing is the failure mode the manual exists to document, so
  prefer refusing with a status line.
