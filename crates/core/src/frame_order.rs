//! Panel READING ORDER (owner top item 2026-08-18): number frames by the
//! order a reader's eye takes, not the order they were created — a
//! computed property, recomputed whenever frames change. The bug it
//! fixes: `divide_frame_folder` names folders `Frame {n}` off a counter,
//! so the top-right panel of an RTL page — panel 1 to a reader — came
//! out "Frame 2" (CSP's own never-fixed grievance, his screenshot).
//!
//! The algorithm (owner correction, 2026-08-19 — GEOMETRY IS THE
//! AUTHORITY; the division tree is not):
//! 1. **One flat XY-cut over EVERY panel on the page** — the page as it
//!    IS now beats the page as it was BUILT. Slots encode history; he
//!    edits panels after building them.
//! 2. **Recursive XY-cut**: a horizontal line no item CROSSES (by more
//!    than the gutter tolerance) splits tiers, top first; else a
//!    vertical one splits columns, RIGHT group first for RTL. A line at
//!    shared edges cuts; a 1–2 px bleed never vetoes a human-obvious cut.
//! 3. **No cut exists** (interlocks): stable band sort, and the band is
//!    marked AMBIGUOUS — unless division SLOTS disambiguate it: members
//!    sharing a slot cluster (ordered by the slot's reading position)
//!    and a fully-resolved band clears its badge. Slots are the TIEBREAK
//!    here, never the anchor.
//! 4. **Validation, not precedence**: after the geometric order, folders
//!    sharing a slot whose panels ended up NON-ADJACENT are badged
//!    ambiguous — geometry and build history disagree; look at the page.
//! 5. **Spreads** cut on the page boundary first: the right page's items
//!    entirely, then the left's (binding-aware).

/// A [x0, y0, x1, y1] rect, canvas px.
pub type Rect4 = [f32; 4];

/// One panel in the computed reading order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelRef {
    /// The frame-folder header's layer index.
    pub layer: usize,
    /// Index into that folder's `FrameSet::frames`.
    pub frame: usize,
}

/// The computed order: panels in reading sequence, plus which positions
/// came from the ambiguous fallback (parallel to `panels`).
#[derive(Clone, Debug, Default)]
pub struct PanelOrder {
    pub panels: Vec<PanelRef>,
    pub ambiguous: Vec<bool>,
}

/// A frame folder participating in the order.
pub struct FolderInput<'a> {
    pub layer: usize,
    pub set: &'a crate::frame::FrameSet,
    /// Manual override: this folder's panels occupy reading position
    /// `pin` together, in their computed sub-order (1-based).
    pub pin: Option<u32>,
}

/// One ordering item: a panel, carrying its folder's division slot (the
/// tiebreak + validation signal — never the anchor).
#[derive(Clone)]
struct PanelItem {
    rect: Rect4,
    panel: PanelRef,
    slot: Option<Rect4>,
    /// Index into the caller's `folders` slice (for the scatter check).
    fi: usize,
}

/// Compute the reading order. `tol` is the cut tolerance in px (the
/// page's gutter); `spread` pages cut at `page_w / 2` first.
pub fn reading_order(
    folders: &[FolderInput<'_>],
    rtl: bool,
    spread: bool,
    page_w: f32,
    tol: f32,
) -> PanelOrder {
    let mut out = PanelOrder::default();
    if folders.is_empty() {
        return out;
    }

    // --- every panel, one flat cut (geometry is the authority) ----------
    let items: Vec<PanelItem> = folders
        .iter()
        .enumerate()
        .flat_map(|(fi, f)| {
            f.set
                .frames
                .iter()
                .enumerate()
                .map(move |(pi, p)| PanelItem {
                    rect: p.bbox(),
                    panel: PanelRef {
                        layer: f.layer,
                        frame: pi,
                    },
                    slot: f.set.slot,
                    fi,
                })
        })
        .collect();

    let mut ordered: Vec<(PanelItem, bool)> = Vec::new();
    // A spread's page order is ABSOLUTE: bands never cross the seam, or
    // the tiebreak sort could drag second-page panels across it.
    let mut seam = 0usize;
    if spread && page_w > 0.0 {
        let mid = page_w * 0.5;
        let (right, left): (Vec<PanelItem>, Vec<PanelItem>) =
            items.into_iter().partition(|it| cx(&it.rect) >= mid);
        let (first, second) = if rtl { (right, left) } else { (left, right) };
        xy_cut(first, tol, rtl, &mut ordered);
        seam = ordered.len();
        xy_cut(second, tol, rtl, &mut ordered);
    } else {
        xy_cut(items, tol, rtl, &mut ordered);
    }

    // --- slot tiebreak inside ambiguous bands ---------------------------
    // Consecutive ambiguous members ON ONE PAGE are one band: members
    // sharing a slot cluster (by the slot's reading position), and a band
    // whose sort is fully determined clears its badge. Slots never
    // reorder a band the geometry already resolved.
    let mut i = 0;
    while i < ordered.len() {
        if !ordered[i].1 {
            i += 1;
            continue;
        }
        let stop = if i < seam { seam } else { ordered.len() };
        let mut j = i;
        while j < stop && ordered[j].1 {
            j += 1;
        }
        slot_tiebreak(&mut ordered[i..j], rtl, tol);
        i = j;
    }

    // --- the scatter check: history vs geometry -------------------------
    // Folders sharing a slot whose panels are NOT adjacent in the final
    // order: the page moved on from the division; badge it rather than
    // silently trusting either story.
    for gi1 in 0..folders.len() {
        let Some(s1) = folders[gi1].set.slot else {
            continue;
        };
        for gi2 in (gi1 + 1)..folders.len() {
            let Some(s2) = folders[gi2].set.slot else {
                continue;
            };
            if !rect_close(s1, s2, tol) {
                continue;
            }
            // Positions of gi1's panels vs gi2's — a non-mate between
            // any of them is a scatter.
            let pos: Vec<Option<usize>> = ordered
                .iter()
                .map(|(it, _)| {
                    if it.fi == gi1 || it.fi == gi2 {
                        Some(it.fi)
                    } else {
                        None
                    }
                })
                .collect();
            let mates: Vec<usize> = pos.iter().flatten().copied().collect();
            let mut seen = [false, false];
            for &m in &mates {
                seen[if m == gi1 { 0 } else { 1 }] = true;
            }
            if seen[0] && seen[1] {
                // Scattered iff a NON-mate sits between the first and
                // last mate — alternation between the two folders is the
                // normal shape of a shared slot's run.
                let first = pos.iter().position(|p| p.is_some());
                let last = pos.iter().rposition(|p| p.is_some());
                let scattered = match (first, last) {
                    (Some(a), Some(b)) => pos[a..=b].iter().any(|p| p.is_none()),
                    _ => false,
                };
                if scattered {
                    for (it, amb) in ordered.iter_mut() {
                        if it.fi == gi1 || it.fi == gi2 {
                            *amb = true;
                        }
                    }
                }
            }
        }
    }

    // --- pins ------------------------------------------------------------
    let pins: Vec<(usize, u32)> = folders
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.pin.map(|p| (i, p)))
        .collect();
    let mut panels: Vec<PanelRef> = ordered.iter().map(|(it, _)| it.panel).collect();
    let mut ambiguous: Vec<bool> = ordered.iter().map(|(_, a)| *a).collect();
    if !pins.is_empty() {
        let pinned = apply_pins(&panels, &pins, folders);
        // apply_pins moves runs; ambiguity flags travel with their panels
        // by PanelRef identity.
        ambiguous = pinned
            .iter()
            .map(|pr| {
                ordered
                    .iter()
                    .find(|(it, _)| it.panel == *pr)
                    .map(|(_, a)| *a)
                    .unwrap_or(false)
            })
            .collect();
        panels = pinned;
    }
    out.panels = panels;
    out.ambiguous = ambiguous;
    out
}

/// Order one ambiguous band by its slots: members sharing a slot cluster
/// by the SLOT's reading position (slotless members keep band order,
/// sorted by their own position among the clusters). A band whose final
/// sort is total (no key ties) clears its ambiguity; ties stay flagged.
fn slot_tiebreak(band: &mut [(PanelItem, bool)], rtl: bool, tol: f32) {
    if band.len() < 2 || !band.iter().any(|(it, _)| it.slot.is_some()) {
        return; // nothing to tiebreak with — keep the band sort
    }
    // Reading position of the slot (or the panel itself when slotless) —
    // tier first, then right-first for RTL — then the PANEL's own
    // position as the within-cluster order. The slot alone excused every
    // tie inside a same-slot cluster (a Divide-equally grid shares ONE
    // slot), which cleared the badge on exactly the guesses it flags.
    let key = |it: &PanelItem| -> (f32, f32, f32, f32) {
        let r = it.slot.unwrap_or(it.rect);
        let sx = if rtl { -r[2] } else { r[0] };
        let px = if rtl { -it.rect[2] } else { it.rect[0] };
        (r[1], sx, it.rect[1], px)
    };
    let mut idx: Vec<usize> = (0..band.len()).collect();
    idx.sort_by(|&a, &b| {
        let (ka, kb) = (key(&band[a].0), key(&band[b].0));
        ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
    });
    let rearranged: Vec<(PanelItem, bool)> = idx.into_iter().map(|i| band[i].clone()).collect();
    // Total iff no two members genuinely tie: a shared slot is a
    // deliberate cluster, fine — but only while the panels' OWN
    // positions break the tie inside it.
    let mut total = true;
    for w in rearranged.windows(2) {
        let (ka, kb) = (key(&w[0].0), key(&w[1].0));
        let t = tol.max(2.0);
        let close = (ka.0 - kb.0).abs() <= t && (ka.1 - kb.1).abs() <= t;
        if close {
            let same_slot = match (w[0].0.slot, w[1].0.slot) {
                (Some(a), Some(b)) => rect_close(a, b, tol),
                _ => false,
            };
            if !same_slot {
                // Distinct slots (or slotless) at one key: ambiguous.
                total = false;
            } else if (ka.2 - kb.2).abs() <= t && (ka.3 - kb.3).abs() <= t {
                // Same slot AND same panel position: a genuine guess.
                total = false;
            }
        }
    }
    for (k, (it, _)) in rearranged.iter().enumerate() {
        band[k] = (it.clone(), !total);
    }
}

fn xy_cut(items: Vec<PanelItem>, tol: f32, rtl: bool, out: &mut Vec<(PanelItem, bool)>) {
    let items = items;
    if items.len() <= 1 {
        out.extend(items.into_iter().map(|it| (it, false)));
        return;
    }
    // Topmost horizontal cut that partitions into TWO NON-EMPTY sides.
    // A candidate some item can satisfy without either side receiving it
    // is real: a panel no taller than `tol` parks its centre at or past
    // every cut its top edge offers, so partitioning on the centre puts
    // it in the bottom side — an empty top side meant re-entering with
    // the IDENTICAL set and a stack overflow (audit C, 2026-08-19). A
    // degenerate cut now falls through to the next candidate and
    // ultimately to band_sort, which marks the group ambiguous — the
    // honest answer for a squashed panel.
    for y in cut_candidates(items.iter().map(|it| [it.rect[1], it.rect[3]]), tol) {
        let (top, bottom): (Vec<PanelItem>, Vec<PanelItem>) =
            items.iter().cloned().partition(|it| cy(&it.rect) < y);
        if top.is_empty() || bottom.is_empty() {
            continue;
        }
        xy_cut(top, tol, rtl, out);
        xy_cut(bottom, tol, rtl, out);
        return;
    }
    for x in cut_candidates(items.iter().map(|it| [it.rect[0], it.rect[2]]), tol) {
        let (right, left): (Vec<PanelItem>, Vec<PanelItem>) =
            items.iter().cloned().partition(|it| cx(&it.rect) >= x);
        if right.is_empty() || left.is_empty() {
            continue;
        }
        if rtl {
            xy_cut(right, tol, rtl, out);
            xy_cut(left, tol, rtl, out);
        } else {
            xy_cut(left, tol, rtl, out);
            xy_cut(right, tol, rtl, out);
        }
        return;
    }
    band_sort(items, tol, rtl, out);
}

/// The fallback: group by top edge (tolerance from the median height),
/// right edge descending for RTL. Marked ambiguous — never silently.
fn band_sort(mut items: Vec<PanelItem>, tol: f32, rtl: bool, out: &mut Vec<(PanelItem, bool)>) {
    let mut hs: Vec<f32> = items.iter().map(|it| it.rect[3] - it.rect[1]).collect();
    hs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let band_tol = (hs[hs.len() / 2] * 0.2).max(tol * 2.0);
    items.sort_by(|a, b| {
        let (ra, rb) = (&a.rect, &b.rect);
        let dy = ra[1] - rb[1];
        if dy.abs() > band_tol {
            return dy.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal);
        }
        let (ax, bx) = if rtl {
            (-ra[2], -rb[2])
        } else {
            (ra[0], rb[0])
        };
        ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
    });
    out.extend(items.into_iter().map(|it| (it, true)));
}

/// The cut-line candidates along a set of 1-D intervals, topmost first:
/// values where intervals end above, start below, and NOTHING crosses by
/// more than `tol` (a shared edge cuts; a 1–2 px bleed does not veto).
/// Returned as a list so a degenerate candidate (one that would not
/// partition into two non-empty sides) can fall through to the next.
fn cut_candidates(intervals: impl Iterator<Item = [f32; 2]>, tol: f32) -> Vec<f32> {
    let iv: Vec<[f32; 2]> = intervals
        .map(|[a, b]| if a <= b { [a, b] } else { [b, a] })
        .collect();
    let mut cands: Vec<f32> = iv.iter().flat_map(|[a, b]| [*a, *b]).collect();
    cands.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    cands.dedup_by(|a, b| (*a - *b).abs() <= tol);
    cands.retain(|&y| {
        let above = iv.iter().any(|[_, hi]| *hi <= y + tol);
        let below = iv.iter().any(|[lo, _]| *lo >= y - tol);
        let crossing = iv.iter().any(|[lo, hi]| *lo < y - tol && *hi > y + tol);
        above && below && !crossing
    });
    cands
}

/// Pins: each pinned folder's panels move as one run (computed sub-order
/// kept) to its pinned slot; unpinned panels keep their sequence in the
/// remaining slots. Pins clamp into range; equal pins resolve in value
/// order (stable).
fn apply_pins(
    panels: &[PanelRef],
    pins: &[(usize, u32)],
    folders: &[FolderInput<'_>],
) -> Vec<PanelRef> {
    let n = panels.len();
    let mut result: Vec<Option<PanelRef>> = vec![None; n];
    let pinned_layers: Vec<usize> = pins.iter().map(|(i, _)| folders[*i].layer).collect();
    let mut sorted: Vec<(usize, u32)> = pins.to_vec();
    sorted.sort_by_key(|(_, p)| *p);
    for (fi, pin) in sorted {
        let run: Vec<PanelRef> = panels
            .iter()
            .filter(|p| p.layer == folders[fi].layer)
            .copied()
            .collect();
        if run.is_empty() {
            continue;
        }
        let mut slot = ((pin as usize).saturating_sub(1)).min(n.saturating_sub(run.len()));
        for p in run {
            while slot < n && result[slot].is_some() {
                slot += 1;
            }
            if slot < n {
                result[slot] = Some(p);
                slot += 1;
            }
        }
    }
    let mut rest = panels.iter().filter(|p| !pinned_layers.contains(&p.layer));
    (0..n)
        .filter_map(|i| result[i].take().or_else(|| rest.next().copied()))
        .collect()
}

fn rect_close(a: Rect4, b: Rect4, tol: f32) -> bool {
    (a[0] - b[0]).abs() <= tol
        && (a[1] - b[1]).abs() <= tol
        && (a[2] - b[2]).abs() <= tol
        && (a[3] - b[3]).abs() <= tol
}

/// Is `a` inside `b`, strictly by more than tol on at least one axis
/// (equal rects are NOT within — they group instead)?
#[allow(dead_code)] // kept: the containment test the slot validation may want again
fn rect_within(a: Rect4, b: Rect4, tol: f32) -> bool {
    a[0] >= b[0] - tol
        && a[1] >= b[1] - tol
        && a[2] <= b[2] + tol
        && a[3] <= b[3] + tol
        && (b[2] - b[0] - (a[2] - a[0]) > tol || b[3] - b[1] - (a[3] - a[1]) > tol)
}

#[allow(dead_code)] // kept: slot-tooling helper
fn area(r: Rect4) -> f32 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

fn cx(r: &Rect4) -> f32 {
    (r[0] + r[2]) * 0.5
}
fn cy(r: &Rect4) -> f32 {
    (r[1] + r[3]) * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Frame, FrameSet};

    // The table leaks its FrameSets on purpose: FolderInput borrows and
    // the table is process-lifetime anyway.
    pub(crate) fn folder(
        layer: usize,
        pin: Option<u32>,
        slot: Option<Rect4>,
        rects: &[[f32; 4]],
    ) -> FolderInput<'static> {
        let set = FrameSet {
            frames: rects
                .iter()
                .map(|r| Frame::rect(r[0], r[1], r[2], r[3]))
                .collect(),
            border_px: 2.0,
            slot,
            reading_pin: None,
            border_ruler: false,
        };
        FolderInput {
            layer,
            set: Box::leak(Box::new(set)),
            pin,
        }
    }

    /// The reading sequence as "L{layer}F{frame}" tokens.
    pub(crate) fn seq(o: &PanelOrder) -> Vec<String> {
        o.panels
            .iter()
            .map(|p| format!("L{}F{}", p.layer, p.frame))
            .collect()
    }

    const TOL: f32 = 2.0;

    /// Audit C, 2026-08-19: a panel no taller than `tol` accepted a cut
    /// its own top edge offered and then parked every item's centre in
    /// the bottom side — xy_cut re-entered with the IDENTICAL set and
    /// the process died of stack overflow (reproduced: 0xc00000fd, both
    /// below). The guard: a cut must partition into two non-empty sides;
    /// degenerate panels fall through to band_sort and are marked
    /// ambiguous — the honest answer for a squashed panel.
    #[test]
    fn collapsed_panel_terminates() {
        let f = [
            folder(0, None, None, &[[0.0, 0.0, 400.0, 0.0]]), // collapsed
            folder(1, None, None, &[[0.0, 0.0, 400.0, 400.0]]),
        ];
        let o = reading_order(&f, true, false, 400.0, 2.0);
        assert_eq!(o.panels.len(), 2, "terminates with every panel placed");
        assert!(
            o.ambiguous.iter().any(|a| *a),
            "a degenerate band is marked ambiguous, never guessed"
        );
    }

    #[test]
    fn thin_panel_at_gutter_tolerance_terminates() {
        // 50 px panel under tol 71 — exactly what a 3 mm gutter at 600
        // dpi passes from renumber_frames after a carry squashed it.
        let f = [
            folder(0, None, None, &[[0.0, 0.0, 400.0, 50.0]]),
            folder(1, None, None, &[[0.0, 50.0, 400.0, 400.0]]),
        ];
        let o = reading_order(&f, true, false, 400.0, 71.0);
        assert_eq!(o.panels.len(), 2, "terminates with every panel placed");
    }

    /// The spec's layout table — each row a hand-checked layout and its
    /// exact expected sequence. This table is where the feature is won.
    #[test]
    fn layout_table() {
        // Standard 2x2 grid, RTL: TR, TL, BR, BL (his screenshot's page).
        let f = [
            folder(0, None, None, &[[0.0, 0.0, 200.0, 100.0]]), // TL
            folder(1, None, None, &[[200.0, 0.0, 400.0, 100.0]]), // TR
            folder(2, None, None, &[[0.0, 100.0, 200.0, 200.0]]), // BL
            folder(3, None, None, &[[200.0, 100.0, 400.0, 200.0]]), // BR
        ];
        let o = reading_order(&f, true, false, 400.0, TOL);
        assert_eq!(seq(&o), ["L1F0", "L0F0", "L3F0", "L2F0"], "2x2 RTL");
        assert!(
            o.ambiguous.iter().all(|a| !a),
            "a plain grid is never ambiguous"
        );

        // T-shape: full-width top over two bottom panels, RTL.
        let f = [
            folder(0, None, None, &[[0.0, 100.0, 200.0, 200.0]]), // BL
            folder(1, None, None, &[[0.0, 0.0, 400.0, 100.0]]),   // band
            folder(2, None, None, &[[200.0, 100.0, 400.0, 200.0]]), // BR
        ];
        let o = reading_order(&f, true, false, 400.0, TOL);
        assert_eq!(seq(&o), ["L1F0", "L2F0", "L0F0"], "T: band, BR, BL");

        // L-shape: tall left column, right column split in two, RTL.
        let f = [
            folder(0, None, None, &[[0.0, 0.0, 150.0, 200.0]]), // left col
            folder(1, None, None, &[[150.0, 0.0, 300.0, 100.0]]), // R top
            folder(2, None, None, &[[150.0, 100.0, 300.0, 200.0]]), // R bottom
        ];
        let o = reading_order(&f, true, false, 300.0, TOL);
        assert_eq!(
            seq(&o),
            ["L1F0", "L2F0", "L0F0"],
            "L: R top, R bottom, left col"
        );

        // The middle panel surrounded by four, OVERLAPPING both side
        // columns: no cut line exists → band sort + AMBIGUOUS (never
        // silently guessed), but the clean top and bottom bands still
        // read first and last.
        let f = [
            folder(0, None, None, &[[0.0, 0.0, 400.0, 90.0]]), // top band
            folder(1, None, None, &[[0.0, 90.0, 150.0, 310.0]]), // left col
            folder(2, None, None, &[[140.0, 90.0, 260.0, 310.0]]), // middle (overlaps both)
            folder(3, None, None, &[[250.0, 90.0, 400.0, 310.0]]), // right col
            folder(4, None, None, &[[0.0, 310.0, 400.0, 400.0]]), // bottom band
        ];
        let o = reading_order(&f, true, false, 400.0, TOL);
        assert_eq!(o.panels.len(), 5);
        assert_eq!(seq(&o)[0], "L0F0", "the clean top band reads first");
        assert_eq!(seq(&o)[4], "L4F0", "the clean bottom band reads last");
        assert_eq!(
            seq(&o)[1..4],
            ["L3F0", "L2F0", "L1F0"],
            "the interlocked trio band-sorts right-first"
        );
        assert!(
            o.ambiguous[1..4].iter().all(|a| *a),
            "the surrounded-middle interlock is marked ambiguous"
        );
        assert!(
            !o.ambiguous[0] && !o.ambiguous[4],
            "the bands are not ambiguous"
        );

        // Divide-then-divide-again: the SLOT anchor. Dividing the
        // top-right panel gives two halves sharing slot S; dividing the
        // right half again gives a pair sharing slot S2 ⊂ S. The pair
        // orders INSIDE S2 (right-first), S2 inside S, S at its page
        // position — the halves can never scatter.
        let s: Rect4 = [200.0, 0.0, 400.0, 100.0];
        let s2: Rect4 = [300.0, 0.0, 400.0, 100.0];
        let f = [
            folder(0, None, None, &[[0.0, 0.0, 200.0, 100.0]]), // TL
            // first division: the kept left half (slot S)
            folder(1, None, Some(s), &[[200.0, 0.0, 300.0, 100.0]]),
            // second division of the right half: both pieces (slot S2)
            folder(2, None, Some(s2), &[[300.0, 0.0, 350.0, 100.0]]),
            folder(3, None, Some(s2), &[[350.0, 0.0, 400.0, 100.0]]),
            folder(4, None, None, &[[0.0, 100.0, 400.0, 200.0]]), // bottom band
        ];
        let o = reading_order(&f, true, false, 400.0, TOL);
        assert_eq!(
            seq(&o),
            ["L3F0", "L2F0", "L1F0", "L0F0", "L4F0"],
            "S2's pair inside S2, S's kept half, then TL, then the band"
        );
        assert!(o.ambiguous.iter().all(|a| !a), "slots never fall back");

        // Two-page spread, RTL: the right page's panels entirely first.
        let f = [
            folder(0, None, None, &[[0.0, 0.0, 200.0, 200.0]]), // left page
            folder(1, None, None, &[[200.0, 0.0, 400.0, 200.0]]), // right page
        ];
        let o = reading_order(&f, true, true, 400.0, TOL);
        assert_eq!(seq(&o), ["L1F0", "L0F0"], "spread RTL: right page first");

        // LTR page: the same 2x2 grid reads TL, TR, BL, BR.
        let f = [
            folder(0, None, None, &[[0.0, 0.0, 200.0, 100.0]]),
            folder(1, None, None, &[[200.0, 0.0, 400.0, 100.0]]),
            folder(2, None, None, &[[0.0, 100.0, 200.0, 200.0]]),
            folder(3, None, None, &[[200.0, 100.0, 400.0, 200.0]]),
        ];
        let o = reading_order(&f, false, false, 400.0, TOL);
        assert_eq!(seq(&o), ["L0F0", "L1F0", "L2F0", "L3F0"], "LTR 2x2");

        // Manual pin: he fixes a wrong order; the pinned folder moves as
        // one run to its slot and everything else keeps its sequence.
        let f = [
            folder(0, Some(1), None, &[[0.0, 0.0, 200.0, 100.0]]), // pinned first
            folder(1, None, None, &[[200.0, 0.0, 400.0, 100.0]]),
            folder(2, None, None, &[[0.0, 100.0, 400.0, 200.0]]),
        ];
        let o = reading_order(&f, true, false, 400.0, TOL);
        assert_eq!(
            seq(&o),
            ["L0F0", "L1F0", "L2F0"],
            "the pin overrules the computed right-first"
        );
    }

    /// Gutter tolerance: a 1 px bleed must not veto a human-obvious cut,
    /// and adjacent tiers with NO gutter (shared edge) still cut.
    #[test]
    fn tolerance_bridges_bleeds_and_shared_edges() {
        let f = [
            folder(0, None, None, &[[0.0, 0.0, 400.0, 101.0]]), // 1px bleed over
            folder(1, None, None, &[[0.0, 100.0, 400.0, 200.0]]),
        ];
        let o = reading_order(&f, true, false, 400.0, 2.0);
        assert_eq!(seq(&o), ["L0F0", "L1F0"], "1px bleed tiers still cut");
        assert!(o.ambiguous.iter().all(|a| !a));

        let f = [
            folder(0, None, None, &[[0.0, 0.0, 400.0, 100.0]]),
            folder(1, None, None, &[[0.0, 100.0, 400.0, 200.0]]), // shared edge
        ];
        let o = reading_order(&f, true, false, 400.0, 2.0);
        assert_eq!(
            seq(&o),
            ["L0F0", "L1F0"],
            "shared-edge tiers cut at the edge"
        );
    }
}

#[cfg(test)]
mod owner_correction_tests {
    use super::tests::{folder, seq};
    use super::*;

    /// OWNER CORRECTION (2026-08-19): geometry is the authority, the
    /// division tree is not. Two folders share a slot (division history)
    /// but were MOVED apart with a third panel between them: the numbers
    /// follow the page as it IS, and the scatter is BADGED, not silently
    /// resolved by history.
    #[test]
    fn geometry_beats_slot_history_and_badges_the_disagreement() {
        // Clean three-tier page. The slot pair got separated by editing.
        let s: Rect4 = [0.0, 0.0, 400.0, 300.0];
        let f = [
            folder(0, None, Some(s), &[[0.0, 0.0, 400.0, 100.0]]), // tier 1
            folder(1, None, None, &[[0.0, 100.0, 400.0, 200.0]]),  // tier 2
            folder(2, None, Some(s), &[[0.0, 200.0, 400.0, 300.0]]), // tier 3
        ];
        let o = reading_order(&f, true, false, 400.0, 2.0);
        assert_eq!(seq(&o), ["L0F0", "L1F0", "L2F0"], "geometry: top to bottom");
        assert!(
            o.ambiguous[0] && o.ambiguous[2],
            "the separated slot pair is badged — history and geometry disagree"
        );
        assert!(!o.ambiguous[1], "the innocent middle panel is not badged");
    }

    /// The slot as TIEBREAK: an interlock the geometry cannot cut is
    /// resolved when two of its members share a division slot — they
    /// cluster by the slot's reading position and the badge clears.
    #[test]
    fn slot_resolves_an_interlock_the_geometry_cannot() {
        // C spans the whole band vertically AND crosses the pair's shared
        // x=150 edge, so no cut line exists. Band sort alone reads
        // C first (its top edge is higher). The shared slot [0,0,300,160]
        // reads at the TOP tier with a righter edge than C — the pair
        // clusters first and the band resolves.
        let s: Rect4 = [0.0, 0.0, 300.0, 160.0];
        let f = [
            folder(0, None, Some(s), &[[0.0, 60.0, 150.0, 160.0]]), // A left
            folder(1, None, Some(s), &[[150.0, 60.0, 300.0, 160.0]]), // B right
            folder(2, None, None, &[[0.0, 0.0, 200.0, 200.0]]),     // C spans
        ];
        let o = reading_order(&f, true, false, 400.0, 2.0);
        assert_eq!(
            seq(&o),
            ["L1F0", "L0F0", "L2F0"],
            "the slot clusters B, A before C"
        );
        assert!(
            o.ambiguous.iter().all(|a| !a),
            "the slot resolved the band: {:?}",
            o.ambiguous
        );
    }

    /// A shared slot excuses a tie only when the panels' OWN positions
    /// break it: two same-slot cells stacked at the same spot (a
    /// Divide-equally grid after a drag mishap) are a genuine guess and
    /// keep the badge. The old key was the slot alone, so a same-slot
    /// cluster excused every tie — "never silently guessed" broken on
    /// exactly the layouts the badge exists for.
    #[test]
    fn stacked_same_slot_cells_stay_badged() {
        let s: Rect4 = [0.0, 0.0, 300.0, 160.0];
        let f = [
            // Two cells of one division sitting on the SAME rect.
            folder(0, None, Some(s), &[[150.0, 60.0, 300.0, 160.0]]),
            folder(1, None, Some(s), &[[150.0, 60.0, 300.0, 160.0]]),
            // The spanner that defeats every cut line.
            folder(2, None, None, &[[0.0, 0.0, 200.0, 200.0]]),
        ];
        let o = reading_order(&f, true, false, 400.0, 2.0);
        assert_eq!(o.panels.len(), 3);
        let badged = o
            .panels
            .iter()
            .zip(&o.ambiguous)
            .filter(|(p, _)| p.layer < 2)
            .all(|(_, a)| *a);
        assert!(
            badged,
            "identical same-slot cells are a guess — badge kept: {:?}",
            o.ambiguous
        );
    }

    /// Spread pages are ordered ABSOLUTELY (right page entirely first
    /// for RTL): an ambiguous band ending the first page and one opening
    /// the second must never merge into a single band, or the tiebreak
    /// sort drags second-page panels across the seam.
    #[test]
    fn ambiguous_bands_never_merge_across_the_spread_seam() {
        let s: Rect4 = [100.0, 0.0, 300.0, 80.0];
        let f = [
            // RIGHT page (x >= 400): an unresolvable pair, low on the page.
            folder(0, None, None, &[[500.0, 300.0, 700.0, 380.0]]),
            folder(1, None, None, &[[500.0, 300.0, 700.0, 380.0]]),
            // LEFT page: a slotted pair at the top — its key reads
            // "above" the right pair's, which is exactly the bait.
            folder(2, None, Some(s), &[[100.0, 0.0, 300.0, 80.0]]),
            folder(3, None, Some(s), &[[100.0, 0.0, 300.0, 80.0]]),
        ];
        let o = reading_order(&f, true, true, 800.0, 2.0);
        let layers: Vec<usize> = o.panels.iter().map(|p| p.layer).collect();
        assert!(
            layers[0..2].contains(&0) && layers[0..2].contains(&1),
            "the right page reads entirely first: {layers:?}"
        );
    }
}
