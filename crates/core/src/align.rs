//! TR-040..046 / row 152 — the Align/Distribute engine (CSP
//! `Align/Distribute` palette).
//!
//! Three families, exactly CSP's three rows of buttons:
//! * **Align** — one edge/centre of every target moves onto the base's
//!   same edge/centre.
//! * **Distribute** — the CHOSEN edges become equally spaced between the
//!   outermost targets (extremes stay put).
//! * **Distribute evenly** — the GAPS between targets become equal
//!   (different from Distribute when sizes differ).
//!
//! The base (TR-044) is what "onto" means: the targets' own union
//! (`Object`), the page (`Canvas`), the selection's bounding box
//! (`Selection area`), or `Auto` (selection when one exists, else the
//! page — the choice is NAMED, never silent). `Guide` is deferred: our
//! guides are ruler-anchored and "nearest guide" needs its own pass.
//!
//! Targets are LAYERS, measured by their CONTENT bounding box (TR-050:
//! the ink's outer edge, not the canvas — and for text/balloon layers
//! the RENDERED ink, which the layer's rasterized tiles already are),
//! or — when the single selection is a text layer with 2+ items — the
//! TEXT ITEMS inside it (TR-052, the JP board's loudest layer-cluster
//! ask). Deferred, with reasons in the triage ledger: vector layers
//! (shifting tiles desyncs the recorded geometry), fill/tone layers
//! (TR-051's mask-edge semantics), frame folders (TR-053 wants their
//! CONTENTS aligned), and the text-pixel / vector-path toggles.

use crate::doc::{Document, Layer, LayerKind};
use crate::tile::TileIdx;
use crate::undo::UndoGroup;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignMode {
    Left,
    HCenter,
    Right,
    Top,
    VCenter,
    Bottom,
}

impl AlignMode {
    pub const ALL: [AlignMode; 6] = [
        AlignMode::Left,
        AlignMode::HCenter,
        AlignMode::Right,
        AlignMode::Top,
        AlignMode::VCenter,
        AlignMode::Bottom,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            AlignMode::Left => "Align left edges",
            AlignMode::HCenter => "Align horizontal centers",
            AlignMode::Right => "Align right edges",
            AlignMode::Top => "Align top edges",
            AlignMode::VCenter => "Align vertical centers",
            AlignMode::Bottom => "Align bottom edges",
        }
    }
}

/// Distribute equalises the chosen EDGE/CENTRE positions; `Spacing`
/// equalises the gaps. Same six references as [`AlignMode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistributeMode {
    Left,
    HCenter,
    Right,
    Top,
    VCenter,
    Bottom,
}

impl DistributeMode {
    pub const ALL: [DistributeMode; 6] = [
        DistributeMode::Left,
        DistributeMode::HCenter,
        DistributeMode::Right,
        DistributeMode::Top,
        DistributeMode::VCenter,
        DistributeMode::Bottom,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            DistributeMode::Left => "Distribute left edges",
            DistributeMode::HCenter => "Distribute horizontal centers",
            DistributeMode::Right => "Distribute right edges",
            DistributeMode::Top => "Distribute top edges",
            DistributeMode::VCenter => "Distribute vertical centers",
            DistributeMode::Bottom => "Distribute bottom edges",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpacingMode {
    Horizontal,
    Vertical,
}

impl SpacingMode {
    pub fn label(&self) -> &'static str {
        match self {
            SpacingMode::Horizontal => "Distribute horizontal spacing",
            SpacingMode::Vertical => "Distribute vertical spacing",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignBase {
    Object,
    Canvas,
    Selection,
    Auto,
}

impl AlignBase {
    pub const ALL: [AlignBase; 4] = [
        AlignBase::Object,
        AlignBase::Canvas,
        AlignBase::Selection,
        AlignBase::Auto,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            AlignBase::Object => "Alignment object",
            AlignBase::Canvas => "Canvas",
            AlignBase::Selection => "Selection area",
            AlignBase::Auto => "Auto",
        }
    }
}

/// Resolve the base rectangle. `object` is the union of the targets'
/// boxes; `sel` the selection's bounds when one exists. `None` = an
/// honest no-op (an explicit `Selection` base with no selection) — the
/// caller reports it rather than quietly aligning to nothing.
/// The returned name reports what `Auto` picked (TR-044: "Auto: Canvas").
pub fn base_rect(
    base: AlignBase,
    doc_size: (u32, u32),
    object: Option<[f32; 4]>,
    sel: Option<[f32; 4]>,
) -> Option<([f32; 4], &'static str)> {
    match base {
        AlignBase::Object => object.map(|r| (r, "Alignment object")),
        AlignBase::Canvas => Some(([0.0, 0.0, doc_size.0 as f32, doc_size.1 as f32], "Canvas")),
        AlignBase::Selection => sel.map(|r| (r, "Selection area")),
        AlignBase::Auto => sel
            .map(|r| (r, "Auto: Selection area"))
            .or_else(|| Some(([0.0, 0.0, doc_size.0 as f32, doc_size.1 as f32], "Auto: Canvas"))),
    }
}

/// The translation that puts `mode`'s reference of `bb` onto `base`'s.
/// Single-axis by definition: aligning left edges moves x ONLY (CSP —
/// a vertical align is a different button).
pub fn align_delta(mode: AlignMode, bb: [f32; 4], base: [f32; 4]) -> [f32; 2] {
    match mode {
        AlignMode::Left => [base[0] - bb[0], 0.0],
        AlignMode::Right => [base[2] - bb[2], 0.0],
        AlignMode::HCenter => [((base[0] + base[2]) - (bb[0] + bb[2])) * 0.5, 0.0],
        AlignMode::Top => [0.0, base[1] - bb[1]],
        AlignMode::Bottom => [0.0, base[3] - bb[3]],
        AlignMode::VCenter => [0.0, ((base[1] + base[3]) - (bb[1] + bb[3])) * 0.5],
    }
}

/// Whether `mode` works on the x axis (false = y).
fn is_x(m: DistributeMode) -> bool {
    matches!(
        m,
        DistributeMode::Left | DistributeMode::HCenter | DistributeMode::Right
    )
}

fn edge_pos(m: DistributeMode, b: [f32; 4]) -> f32 {
    match m {
        DistributeMode::Left => b[0],
        DistributeMode::HCenter => (b[0] + b[2]) * 0.5,
        DistributeMode::Right => b[2],
        DistributeMode::Top => b[1],
        DistributeMode::VCenter => (b[1] + b[3]) * 0.5,
        DistributeMode::Bottom => b[3],
    }
}

/// Distribute (TR-042): the chosen reference edges become equally
/// spaced between the outermost targets, WHICH STAY PUT. One delta per
/// box. Fewer than three targets is a no-op (CSP requires 3+).
pub fn distribute_deltas(mode: DistributeMode, boxes: &[[f32; 4]]) -> Vec<[f32; 2]> {
    let n = boxes.len();
    if n < 3 {
        return vec![[0.0, 0.0]; n];
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        edge_pos(mode, boxes[a])
            .partial_cmp(&edge_pos(mode, boxes[b]))
            .unwrap()
    });
    let first = edge_pos(mode, boxes[order[0]]);
    let last = edge_pos(mode, boxes[order[n - 1]]);
    let mut out = vec![[0.0, 0.0]; n];
    for (slot, &i) in order.iter().enumerate() {
        let t = slot as f32 / (n - 1) as f32;
        let d = first + (last - first) * t - edge_pos(mode, boxes[i]);
        out[i] = if is_x(mode) { [d, 0.0] } else { [0.0, d] };
    }
    out
}

/// Distribute evenly (TR-043): equal GAPS along the axis. The
/// outermost targets stay put; the middle ones close up or spread out
/// so every gap is `(span - Σsizes) / (n-1)`. Fewer than three is a
/// no-op.
pub fn spacing_deltas(mode: SpacingMode, boxes: &[[f32; 4]]) -> Vec<[f32; 2]> {
    let n = boxes.len();
    if n < 3 {
        return vec![[0.0, 0.0]; n];
    }
    let x_axis = matches!(mode, SpacingMode::Horizontal);
    let lo = |b: [f32; 4]| if x_axis { b[0] } else { b[1] };
    let hi = |b: [f32; 4]| if x_axis { b[2] } else { b[3] };
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| lo(boxes[a]).partial_cmp(&lo(boxes[b])).unwrap());
    let first_lo = lo(boxes[order[0]]);
    let last_hi = hi(boxes[order[n - 1]]);
    let sizes: f32 = order.iter().map(|&i| hi(boxes[i]) - lo(boxes[i])).sum();
    let gap = ((last_hi - first_lo) - sizes) / (n - 1) as f32;
    let mut out = vec![[0.0, 0.0]; n];
    let mut cursor = first_lo;
    for &i in &order {
        let d = cursor - lo(boxes[i]);
        out[i] = if x_axis { [d, 0.0] } else { [0.0, d] };
        cursor = cursor + (hi(boxes[i]) - lo(boxes[i])) + gap;
    }
    out
}

/// A layer's CONTENT bounding box (TR-050): the union of alpha>0
/// pixels over the populated tiles — the ink's outer edge, not the
/// canvas. For text and balloon layers the tiles are the RENDERED
/// vectors, so the box is pixel-accurate for them too. Fill/tone
/// layers keep no pixel map of their own (the raster is derived) and
/// answer `None`.
pub fn content_bbox(l: &Layer) -> Option<[f32; 4]> {
    let mut r: Option<[f32; 4]> = None;
    for (idx, t) in l.tiles() {
        let (ox, oy) = idx.origin();
        let d = t.data();
        for py in 0..crate::tile::TILE_SIZE {
            let row = py * crate::tile::TILE_SIZE;
            for px in 0..crate::tile::TILE_SIZE {
                if d[(row + px) * 4 + 3] == 0 {
                    continue;
                }
                let (x, y) = (ox + px as i32, oy + py as i32);
                r = Some(match r {
                    None => [x as f32, y as f32, x as f32 + 1.0, y as f32 + 1.0],
                    Some(m) => [
                        m[0].min(x as f32),
                        m[1].min(y as f32),
                        m[2].max(x as f32 + 1.0),
                        m[3].max(y as f32 + 1.0),
                    ],
                });
            }
        }
    }
    r
}

impl Document {
    /// The layers an align/distribute gesture targets: the palette's
    /// multi-selection, plus the active layer when it is not already in
    /// it, filtered to kinds this engine moves (raster, text, balloon),
    /// visible and unlocked. Frame folders, fill/tone layers, vector
    /// layers and hidden/locked layers are refused with reasons in the
    /// returned notes.
    fn align_targets(&self) -> (Vec<usize>, Vec<String>) {
        let picks = self.multi_targets();
        let mut out = Vec::new();
        let mut notes = Vec::new();
        for li in picks {
            let Some(l) = self.layers.get(li) else {
                continue;
            };
            if l.folder {
                notes.push(format!("{} is a folder", l.name));
                continue;
            }
            if !l.visible {
                notes.push(format!("{} is hidden", l.name));
                continue;
            }
            if l.lock {
                notes.push(format!("{} is locked", l.name));
                continue;
            }
            if l.strokes.is_some() {
                notes.push(format!("{} is a vector layer", l.name));
                continue;
            }
            match &l.kind {
                LayerKind::Frame(_) => notes.push(format!("{} is a frame folder layer", l.name)),
                LayerKind::Fill(_) => {
                    notes.push(format!("{} is a live fill/tone layer", l.name))
                }
                _ => out.push(li),
            }
        }
        (out, notes)
    }

    /// Move one target by whole pixels: raster ink tile-by-tile, text
    /// and balloon layers through their vector state + re-rasterize
    /// (shifting their tiles alone would desync from the vectors).
    /// Returns the undo member when anything moved.
    fn shift_target(&mut self, li: usize, d: [f32; 2]) -> Option<UndoGroup> {
        let (dx, dy) = (d[0].round() as i32, d[1].round() as i32);
        if dx == 0 && dy == 0 {
            return None;
        }
        let size = self.size;
        let kind = self.layers.get(li)?.kind.clone();
        match kind {
            LayerKind::Text(ts) => {
                let mut ts = ts;
                for t in &mut ts.texts {
                    t.pos = [t.pos[0] + d[0], t.pos[1] + d[1]];
                }
                let before = match &mut self.layers.get_mut(li)?.kind {
                    LayerKind::Text(cur) => std::mem::replace(cur, ts.clone()),
                    _ => return None,
                };
                let raster = ts.rasterize(size);
                self.layers.get_mut(li)?.replace_tiles(raster);
                Some(UndoGroup::Texts {
                    layer: li,
                    texts: before,
                })
            }
            LayerKind::Balloon(bs) => {
                let mut bs = bs;
                for b in &mut bs.balloons {
                    b.translate(d[0], d[1]);
                }
                let before = match &mut self.layers.get_mut(li)?.kind {
                    LayerKind::Balloon(cur) => std::mem::replace(cur, bs.clone()),
                    _ => return None,
                };
                let raster = bs.rasterize(size);
                self.layers.get_mut(li)?.replace_tiles(raster);
                Some(UndoGroup::Balloons {
                    layer: li,
                    balloons: before,
                })
            }
            _ => {
                // Raster ink: snapshot, clear, write — inside one op
                // bracket so the pre-images are one undo member.
                let src: Vec<(TileIdx, std::sync::Arc<crate::tile::Tile>)> =
                    self.layers[li].tiles().map(|(i, t)| (i, t.clone())).collect();
                if src.is_empty() {
                    return None;
                }
                self.begin_op_on(li);
                let (w, h) = (self.size.0 as i32, self.size.1 as i32);
                for (idx, t) in &src {
                    let (ox, oy) = idx.origin();
                    let d8 = t.data();
                    for py in 0..crate::tile::TILE_SIZE {
                        let row = py * crate::tile::TILE_SIZE;
                        for px in 0..crate::tile::TILE_SIZE {
                            let p = {
                                let o = (row + px) * 4;
                                [d8[o], d8[o + 1], d8[o + 2], d8[o + 3]]
                            };
                            if p[3] == 0 {
                                continue;
                            }
                            let (x, y) = (ox + px as i32, oy + py as i32);
                            // Clear the source, then write the dest —
                            // the snapshot supplies the pixel, so the
                            // order inside one tile pair cannot lose ink.
                            let lt = self.layers[li].tile_mut(TileIdx::of_pixel(x, y));
                            lt.set_pixel(
                                (x - ox) as usize,
                                (y - oy) as usize,
                                [0, 0, 0, 0],
                            );
                            let (nx, ny) = (x + dx, y + dy);
                            if nx < 0 || ny < 0 || nx >= w || ny >= h {
                                continue;
                            }
                            let ni = TileIdx::of_pixel(nx, ny);
                            let (nox, noy) = ni.origin();
                            let nt = self.layers[li].tile_mut(ni);
                            nt.set_pixel((nx - nox) as usize, (ny - noy) as usize, p);
                        }
                    }
                }
                self.end_op_take()
            }
        }
    }

    /// TR-041: align the selected layers (`mode`'s reference onto the
    /// base's). One undo step, whatever the mix of kinds. Returns a
    /// status line; aligning nothing is reported, never silent.
    pub fn align_layers(&mut self, mode: AlignMode, base: AlignBase) -> String {
        let (targets, notes) = self.align_targets();
        let boxes: Vec<[f32; 4]> = targets
            .iter()
            .filter_map(|&li| self.layers.get(li).and_then(content_bbox))
            .collect();
        if targets.is_empty() || boxes.is_empty() {
            return "nothing to align — select layers with content".into();
        }
        if base == AlignBase::Object && targets.len() < 2 {
            return "aligning to the objects needs 2+ selected".into();
        }
        let object = union(&boxes);
        let sel = self.selection.as_ref().and_then(|s| s.bounds()).map(|b| {
            [
                b[0] as f32,
                b[1] as f32,
                b[2] as f32 + 1.0,
                b[3] as f32 + 1.0,
            ]
        });
        let Some((base_r, base_name)) = base_rect(base, self.size, Some(object), sel) else {
            return "align to selection needs a selection first".into();
        };
        let mut members = Vec::new();
        let mut moved = 0usize;
        for &li in targets.iter() {
            let Some(bb) = self.layers.get(li).and_then(content_bbox) else {
                continue;
            };
            if let Some(g) = self.shift_target(li, align_delta(mode, bb, base_r)) {
                members.push(g);
                moved += 1;
            }
        }
        let label = format!("Align · {}", mode.label());
        self.push_compound(&label, members);
        format!(
            "aligned {} layer{} to {} — {}{}",
            moved,
            if moved == 1 { "" } else { "s" },
            base_name,
            label,
            if notes.is_empty() {
                String::new()
            } else {
                format!(" (skipped: {})", notes.join(", "))
            }
        )
    }

    /// TR-042 (edges/centres equally spaced) and TR-043 (equal gaps):
    /// 3+ targets, extremes stay put. One undo step.
    pub fn distribute_layers(&mut self, mode: DistributeMode) -> String {
        let (targets, notes) = self.align_targets();
        let pairs: Vec<(usize, [f32; 4])> = targets
            .iter()
            .filter_map(|&li| Some((li, self.layers.get(li).and_then(content_bbox)?)))
            .collect();
        if pairs.len() < 3 {
            return "distribute needs 3+ selected layers with content".into();
        }
        let boxes: Vec<[f32; 4]> = pairs.iter().map(|p| p.1).collect();
        let deltas = distribute_deltas(mode, &boxes);
        self.apply_deltas(&pairs, &deltas, &notes, &format!("Distribute · {}", mode.label()))
    }

    pub fn space_layers(&mut self, mode: SpacingMode) -> String {
        let (targets, notes) = self.align_targets();
        let pairs: Vec<(usize, [f32; 4])> = targets
            .iter()
            .filter_map(|&li| Some((li, self.layers.get(li).and_then(content_bbox)?)))
            .collect();
        if pairs.len() < 3 {
            return "distribute evenly needs 3+ selected layers with content".into();
        }
        let boxes: Vec<[f32; 4]> = pairs.iter().map(|p| p.1).collect();
        let deltas = spacing_deltas(mode, &boxes);
        self.apply_deltas(
            &pairs,
            &deltas,
            &notes,
            &format!("Distribute evenly · {}", mode.label()),
        )
    }

    fn apply_deltas(
        &mut self,
        pairs: &[(usize, [f32; 4])],
        deltas: &[[f32; 2]],
        notes: &[String],
        label: &str,
    ) -> String {
        let mut members = Vec::new();
        for (&(li, _), d) in pairs.iter().zip(deltas) {
            if let Some(g) = self.shift_target(li, *d) {
                members.push(g);
            }
        }
        // The extremes staying put is PART of the operation, so the
        // count is the targets, not the movers (an already-even spread
        // is a completed distribute, not a failure).
        let n = pairs.len();
        self.push_compound(label, members);
        format!(
            "distributed {} layer{} — the outer two stayed put{}",
            n,
            if n == 1 { "" } else { "s" },
            if notes.is_empty() {
                String::new()
            } else {
                format!(" (skipped: {})", notes.join(", "))
            }
        )
    }

    /// TR-052 (the 285-vote thread) for TEXT: with one text layer
    /// selected, align/distribute its ITEMS against each other —
    /// `Object` base only (the union of the items' boxes), gaps and
    /// edges exactly as the layer families. One undo step.
    pub fn align_text_items(&mut self, li: usize, mode: AlignMode) -> String {
        let Some(l) = self.layers.get(li) else {
            return String::new();
        };
        let Some(ts) = l.texts() else {
            return String::new();
        };
        let boxes: Vec<[f32; 4]> = ts.texts.iter().map(item_box).collect();
        if boxes.len() < 2 {
            return "aligning items needs 2+ text items on the layer".into();
        }
        let base = union(&boxes);
        let mut ts = ts.clone();
        for (t, bb) in ts.texts.iter_mut().zip(&boxes) {
            let d = align_delta(mode, *bb, base);
            t.pos = [t.pos[0] + d[0], t.pos[1] + d[1]];
        }
        let status = format!("aligned {} text items against each other", ts.texts.len());
        let before = match &mut self.layers.get_mut(li).unwrap().kind {
            LayerKind::Text(cur) => std::mem::replace(cur, ts.clone()),
            _ => unreachable!("checked above"),
        };
        let raster = ts.rasterize(self.size);
        self.layers.get_mut(li).unwrap().replace_tiles(raster);
        self.push_compound(
            &format!("Align items · {}", mode.label()),
            vec![UndoGroup::Texts {
                layer: li,
                texts: before,
            }],
        );
        status
    }

    pub fn distribute_text_items(&mut self, li: usize, mode: DistributeMode) -> String {
        let Some(l) = self.layers.get(li) else {
            return String::new();
        };
        let Some(ts) = l.texts() else {
            return String::new();
        };
        let boxes: Vec<[f32; 4]> = ts.texts.iter().map(item_box).collect();
        if boxes.len() < 3 {
            return "distributing items needs 3+ text items on the layer".into();
        }
        let deltas = distribute_deltas(mode, &boxes);
        self.apply_item_deltas(li, &deltas, &format!("Distribute items · {}", mode.label()))
    }

    pub fn space_text_items(&mut self, li: usize, mode: SpacingMode) -> String {
        let Some(l) = self.layers.get(li) else {
            return String::new();
        };
        let Some(ts) = l.texts() else {
            return String::new();
        };
        let boxes: Vec<[f32; 4]> = ts.texts.iter().map(item_box).collect();
        if boxes.len() < 3 {
            return "distributing items needs 3+ text items on the layer".into();
        }
        let deltas = spacing_deltas(mode, &boxes);
        self.apply_item_deltas(
            li,
            &deltas,
            &format!("Distribute items evenly · {}", mode.label()),
        )
    }

    fn apply_item_deltas(&mut self, li: usize, deltas: &[[f32; 2]], label: &str) -> String {
        let Some(l) = self.layers.get(li) else {
            return String::new();
        };
        let Some(ts) = l.texts() else {
            return String::new();
        };
        let mut ts = ts.clone();
        for (t, d) in ts.texts.iter_mut().zip(deltas) {
            t.pos = [t.pos[0] + d[0], t.pos[1] + d[1]];
        }
        let status = format!("distributed {} text items", ts.texts.len());
        let before = match &mut self.layers.get_mut(li).unwrap().kind {
            LayerKind::Text(cur) => std::mem::replace(cur, ts.clone()),
            _ => unreachable!("checked above"),
        };
        let raster = ts.rasterize(self.size);
        self.layers.get_mut(li).unwrap().replace_tiles(raster);
        self.push_compound(
            label,
            vec![UndoGroup::Texts {
                layer: li,
                texts: before,
            }],
        );
        status
    }
}

/// The axis-aligned extent of one text item — the ROTATED corners, so a
/// tilted shout measures its true footprint (the same rule the balloon
/// fit uses for the lettering it wraps).
fn item_box(t: &crate::text::TextItem) -> [f32; 4] {
    let c = t.center();
    let (sn, cs) = t.rotation.sin_cos();
    let mut r = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for (ex, ey) in [
        (0.0, 0.0),
        (t.size[0], 0.0),
        (0.0, t.size[1]),
        (t.size[0], t.size[1]),
    ] {
        let (x, y) = (t.pos[0] + ex - c[0], t.pos[1] + ey - c[1]);
        let (rx, ry) = (c[0] + x * cs - y * sn, c[1] + x * sn + y * cs);
        r[0] = r[0].min(rx);
        r[1] = r[1].min(ry);
        r[2] = r[2].max(rx);
        r[3] = r[3].max(ry);
    }
    r
}

fn union(boxes: &[[f32; 4]]) -> [f32; 4] {
    let mut r = boxes[0];
    for b in &boxes[1..] {
        r[0] = r[0].min(b[0]);
        r[1] = r[1].min(b[1]);
        r[2] = r[2].max(b[2]);
        r[3] = r[3].max(b[3]);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::f32_to_fix15;
    use crate::doc::Document;
    use crate::tile::TILE_SIZE;

    /// Ink a solid rect onto a raster layer (premultiplied opaque black).
    fn ink(doc: &mut Document, li: usize, x0: i32, y0: i32, x1: i32, y1: i32) {
        for y in y0..y1 {
            for x in x0..x1 {
                let idx = TileIdx::of_pixel(x, y);
                let (ox, oy) = idx.origin();
                let t = doc.layers[li].tile_mut(idx);
                let o = ((y - oy) as usize * TILE_SIZE + (x - ox) as usize) * 4;
                let f = f32_to_fix15(0.0);
                let d = t.data_mut();
                d[o] = f;
                d[o + 1] = f;
                d[o + 2] = f;
                d[o + 3] = f32_to_fix15(1.0);
            }
        }
    }

    #[test]
    fn align_math_hits_the_base_reference() {
        let bb = [10.0, 20.0, 30.0, 60.0];
        let base = [0.0, 0.0, 100.0, 200.0];
        // Left edges: 10 → 0.
        assert_eq!(align_delta(AlignMode::Left, bb, base), [-10.0, 0.0]);
        // H centers: 20 → 50.
        assert_eq!(align_delta(AlignMode::HCenter, bb, base), [30.0, 0.0]);
        // Bottom edges: 60 → 200.
        assert_eq!(align_delta(AlignMode::Bottom, bb, base), [0.0, 140.0]);
    }

    #[test]
    fn auto_base_names_its_choice() {
        let page = [0.0, 0.0, 400.0, 600.0];
        assert_eq!(
            base_rect(AlignBase::Auto, (400, 600), Some([9.0; 4]), None),
            Some((page, "Auto: Canvas"))
        );
        let sel = [10.0, 10.0, 90.0, 90.0];
        assert_eq!(
            base_rect(AlignBase::Auto, (400, 600), Some([9.0; 4]), Some(sel)),
            Some((sel, "Auto: Selection area"))
        );
        assert_eq!(
            base_rect(AlignBase::Selection, (400, 600), None, None),
            None,
            "an explicit selection base with no selection is an honest no-op"
        );
    }

    #[test]
    fn distribute_spaces_edges_and_keeps_the_outer_two() {
        // Centers 5, 10, 60 → evenly 5, 32.5, 60; the middle moves +22.5.
        let boxes = [[0.0, 0.0, 10.0, 10.0], [5.0, 0.0, 15.0, 10.0], [55.0, 0.0, 65.0, 10.0]];
        let d = distribute_deltas(DistributeMode::HCenter, &boxes);
        assert_eq!(d[0], [0.0, 0.0], "the leftmost stays");
        assert_eq!(d[1], [22.5, 0.0], "the middle centres onto 32.5");
        assert_eq!(d[2], [0.0, 0.0], "the rightmost stays");
        assert_eq!(
            distribute_deltas(DistributeMode::Top, &boxes[..2]),
            vec![[0.0, 0.0]; 2],
            "fewer than three is a no-op"
        );
    }

    #[test]
    fn spacing_equalises_gaps_not_edges() {
        // Widths 10/20/10 spanning 0..=60: gaps (60-40)/2 = 10 →
        // lefts at 0, 20, 50. The middle (left 5) moves to 20; the
        // last (left 55) is 5 PAST its equal-gap slot — the CSP
        // difference between TR-042 and TR-043 in one number.
        let boxes = [[0.0, 0.0, 10.0, 10.0], [5.0, 0.0, 25.0, 10.0], [50.0, 0.0, 60.0, 10.0]];
        let d = spacing_deltas(SpacingMode::Horizontal, &boxes);
        assert_eq!(d[0], [0.0, 0.0]);
        assert_eq!(d[1], [15.0, 0.0]);
        assert_eq!(d[2], [0.0, 0.0], "the outermost target stays put");
        // And the gaps really are equal afterwards.
        let moved: Vec<[f32; 4]> = boxes
            .iter()
            .zip(&d)
            .map(|(b, d)| [b[0] + d[0], b[1], b[2] + d[0], b[3]])
            .collect();
        assert_eq!(moved[1][0] - moved[0][2], moved[2][0] - moved[1][2]);
    }

    #[test]
    fn a_layer_aligns_by_its_content_and_one_undo_rewinds_it() {
        let mut doc = Document::new(200, 200);
        let a = doc.add_layer("a");
        let b = doc.add_layer("b");
        ink(&mut doc, a, 10, 10, 20, 20);
        ink(&mut doc, b, 100, 100, 120, 120);
        // The palette's real selection gesture: one Ctrl+click on `a`
        // pulls the old active `b` into the multi-selection with it.
        doc.toggle_multi(a);

        let status = doc.align_layers(AlignMode::VCenter, AlignBase::Canvas);
        assert!(status.contains("aligned 2 layers to Canvas"), "{status}");
        // a's centre y (15) → page centre (100): ink now 10..20 at y 95..105.
        let bb_a = content_bbox(&doc.layers[a]).unwrap();
        assert!((bb_a[1] - 95.0).abs() < 1.0 && (bb_a[3] - 105.0).abs() < 1.0);
        // One undo rewinds BOTH layers (a Compound).
        assert!(doc.undo());
        assert_eq!(
            content_bbox(&doc.layers[a]).unwrap(),
            [10.0, 10.0, 20.0, 20.0],
            "undo put the ink back"
        );
    }

    #[test]
    fn three_layers_distribute_by_content_edges() {
        let mut doc = Document::new(300, 100);
        let layers: Vec<usize> = (0..3).map(|_| doc.add_layer("l")).collect();
        ink(&mut doc, layers[0], 0, 0, 10, 10);
        ink(&mut doc, layers[1], 100, 50, 110, 60);
        ink(&mut doc, layers[2], 250, 20, 260, 30);
        // l2 is active already; each Ctrl+click pulls the previous
        // active into the multi — two clicks select all three.
        doc.toggle_multi(layers[0]);
        doc.toggle_multi(layers[1]);

        let status = doc.distribute_layers(DistributeMode::Left);
        assert!(status.contains("distributed 3 layers"), "{status}");
        let lefts: Vec<f32> = layers
            .iter()
            .map(|&l| content_bbox(&doc.layers[l]).unwrap()[0])
            .collect();
        assert_eq!(
            lefts[1] - lefts[0],
            lefts[2] - lefts[1],
            "left edges evenly spaced: {lefts:?}"
        );
        assert_eq!(lefts[0], 0.0, "the leftmost stayed");
        assert_eq!(lefts[2], 250.0, "the rightmost stayed");
    }

    #[test]
    fn text_items_align_against_each_other_within_one_layer() {
        let mut doc = Document::new(400, 400);
        let mk = |x: f64, y: f64| crate::text::TextItem::new(
            [x as f32, y as f32],
            "Gothic".into(),
            9.0,
            [0, 0, 0],
            true,
        );
        let mut t0 = mk(10.0, 10.0);
        t0.size = [40.0, 20.0];
        let mut t1 = mk(100.0, 300.0);
        t1.size = [40.0, 20.0];
        let li = doc.add_text_layer(
            "lettering",
            crate::text::TextSet {
                texts: vec![t0, t1],
            },
        );
        let status = doc.align_text_items(li, AlignMode::Left);
        assert!(status.contains("aligned 2 text items"), "{status}");
        let ts = doc.layers[li].texts().unwrap();
        assert_eq!(ts.texts[0].pos[0], 10.0, "the leftmost item is the reference");
        assert_eq!(ts.texts[1].pos[0], 10.0, "the other moved onto it");
        assert_eq!(ts.texts[1].pos[1], 300.0, "only the x axis moved");
        assert!(doc.undo(), "one undo rewinds the item align");
    }
}

