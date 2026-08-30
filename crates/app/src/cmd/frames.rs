//! `AppCmd` arms: frames (koma) — the frame layer, dividing,
//! drawing, combining and deleting panels.

use super::*;

/// The frame folder enclosing `i`, nearest first: children sit BELOW their
/// header in `layers`, so the block closes at the first folder above with
/// a smaller depth. Walks out through plain folders too (a subfoldered
/// panel is still inside the panel).
pub(super) fn enclosing_folder(doc: &mn_core::Document, i: usize) -> Option<usize> {
    let d = doc.layers[i].depth;
    if d == 0 {
        return None;
    }
    (i + 1..doc.layers.len()).find(|&j| doc.layers[j].folder && doc.layers[j].depth < d)
}

/// Which frame of a multi-frame folder the paste aims at: the one holding
/// the active layer's content centre, else the first.
pub(super) fn frame_index_for(doc: &mn_core::Document, folder: usize, active: usize) -> usize {
    let Some(fs) = doc.layers[folder].frames() else {
        return 0;
    };
    if fs.frames.len() < 2 {
        return 0;
    }
    let c = doc.layers[active]
        .tile_bounds()
        .map(|(x, y, w, h)| [x + w as i32 / 2, y + h as i32 / 2])
        .or_else(|| {
            doc.layers[folder]
                .tile_bounds()
                .map(|(x, y, w, h)| [x + w as i32 / 2, y + h as i32 / 2])
        });
    if let Some([cx, cy]) = c {
        for (i, f) in fs.frames.iter().enumerate() {
            let b = f.bbox();
            if cx as f32 >= b[0] && cy as f32 >= b[1] && (cx as f32) < b[2] && (cy as f32) < b[3] {
                return i;
            }
        }
    }
    0
}

/// The keeper's slot after a division (owner top item 2026-08-18): the
/// cut union when ALL its frames lie inside it (a pure division — the
/// common case: both halves share the slot and order inside it); None
/// when untouched panels remain outside, so the folder keeps ordering
/// globally by its own geometry.
/// The frame layer a frame command acts on: the active layer when it is one,
/// else the topmost frame layer in the stack (what `FrameDivide` has always
/// done — the new frame commands resolve it the same way so they never
/// disagree about which folder the artist means).
fn frame_target(app: &App) -> Option<usize> {
    if app.doc.active_layer().is_frame() {
        Some(app.doc.active)
    } else {
        app.doc.layers.iter().rposition(|l| l.is_frame())
    }
}

fn slot_for(frames: &[mn_core::Frame], cut: Option<[f32; 4]>) -> Option<[f32; 4]> {
    let c = cut?;
    const TOL: f32 = 2.0;
    let all_inside = frames.iter().all(|f| {
        let b = f.bbox();
        b[0] >= c[0] - TOL && b[1] >= c[1] - TOL && b[2] <= c[2] + TOL && b[3] <= c[3] + TOL
    });
    if all_inside { Some(c) } else { None }
}

pub(super) fn run(app: &mut App, cmd: AppCmd, cmd_tail: CmdTail) {
    match cmd {
        // --- frames (koma) --------------------------------------------------
        AppCmd::NewFrameLayer => {
            // The 基本枠 is mirrored per book side — `inner_rect_px()` alone is
            // the RIGHT-page orientation and put a left page's folder 2×offset
            // off the purple guides it is drawn against.
            let right = app.current_page_right().unwrap_or(true);
            let rect = app
                .page
                .as_ref()
                .filter(|p| p.has_guides())
                .map(|p| p.inner_rect_px_on(right))
                .unwrap_or_else(|| {
                    let (w, h) = (app.doc.size.0 as f32, app.doc.size.1 as f32);
                    [w * 0.08, h * 0.08, w * 0.92, h * 0.92]
                });
            // Same width the frame create sub tools use (owner's CSP value);
            // this arm used to hardcode CSP's factory 0.8 mm instead.
            let border = app.mm_to_px(app.frame_border_mm).max(2.0);
            let n = app.doc.layers.iter().filter(|l| l.is_frame()).count() + 1;
            app.doc
                .add_frame_folder(format!("Frame {n}"), FrameSet::single_rect(rect, border));
            app.renderer.invalidate();
            app.renumber_frames();
            app.set_status(
                "frame folder added — draw inside it, U divides panels, its White layer hides art below",
            );
            app.mark_dirty();
        }
        AppCmd::FrameDivide { a, b } => {
            // The folder the cut LANDS in is the one whose panel the drag
            // actually crossed — after the first folder-divide the active
            // layer is a draw layer, and "topmost frame layer" sent every
            // later cut to the newest folder regardless of where the pen
            // was (owner-class bug, 2026-08-23 audit). Active still wins
            // when it was crossed too, so nothing changes mid-panel.
            let crosses = |l: &mn_core::Layer| {
                l.is_frame()
                    && l.frames().is_some_and(|fs| {
                        fs.frames
                            .iter()
                            .any(|f| f.segment_touches([a.0, a.1], [b.0, b.1]))
                    })
            };
            let li = if app.doc.active_layer().is_frame() && crosses(app.doc.active_layer()) {
                Some(app.doc.active)
            } else {
                app.doc.layers.iter().rposition(crosses)
            }
            // Nothing crossed: fall back to the old resolution so the
            // "drag across a panel" status below still has a folder to say
            // it about.
            .or_else(|| {
                if app.doc.active_layer().is_frame() {
                    Some(app.doc.active)
                } else {
                    app.doc.layers.iter().rposition(|l| l.is_frame())
                }
            });
            let Some(li) = li else {
                app.set_status("no frame layer — Layer > New frame border folder first");
                return;
            };
            let mut fs = app.doc.layers[li].frames().expect("is_frame").clone();
            // Gutter width blends the two Tool Property values by cut angle:
            // a horizontal cut separates rows (vertical interval), a vertical
            // cut separates columns (horizontal interval). Each cut sub tool
            // keeps its own pair (the owner's CSP values).
            let (g_h, g_v) = if app.frame_mode == FrameMode::DivideBorder {
                app.gutter_border_mm
            } else {
                app.gutter_folder_mm
            };
            let ang = (b.1 - a.1).atan2(b.0 - a.0);
            let gutter = app.mm_to_px(g_v) * ang.cos().abs() + app.mm_to_px(g_h) * ang.sin().abs();
            // CSP "Divide frame folder": the far side of every cut splits off
            // into ONE new frame border folder. TRIAGE 128: what that folder
            // gets is the artist's call — "Do not change" declines the folder
            // entirely and just draws the border.
            let as_folder = app.frame_mode == FrameMode::DivideFolder
                && app.doc.layers[li].folder
                && app.frame_divide_contents != DivideContents::DoNotChange;
            let mut keep = Vec::with_capacity(fs.frames.len() + 1);
            let mut split_off = Vec::new();
            let mut cuts = 0usize;
            // Reading-order provenance (owner top item 2026-08-18): the
            // union of the CUT panels is the slot both halves order
            // inside — division siblings can never scatter.
            let mut cut_union: Option<[f32; 4]> = None;
            for f in fs.frames.drain(..) {
                if f.segment_touches([a.0, a.1], [b.0, b.1]) {
                    let bb = f.bbox();
                    cut_union = Some(match cut_union {
                        None => bb,
                        Some(u) => [
                            u[0].min(bb[0]),
                            u[1].min(bb[1]),
                            u[2].max(bb[2]),
                            u[3].max(bb[3]),
                        ],
                    });
                    if let Some((p, q)) = f.split([a.0, a.1], [b.0, b.1], gutter) {
                        keep.push(p);
                        if as_folder {
                            split_off.push(q);
                        } else {
                            keep.push(q);
                        }
                        cuts += 1;
                        continue;
                    }
                }
                keep.push(f);
            }
            if cuts == 0 {
                app.set_status("drag across a panel to divide it");
            } else if as_folder && !split_off.is_empty() {
                fs.frames = keep;
                fs.slot = slot_for(&fs.frames, cut_union);
                let mut new_fs = fs.clone();
                new_fs.frames = split_off;
                new_fs.slot = cut_union;
                let dup = app.frame_divide_contents == DivideContents::Duplicate;
                let done = if dup {
                    app.doc.divide_frame_folder_dup(li, fs, new_fs)
                } else {
                    app.doc.divide_frame_folder(li, fs, new_fs)
                };
                if done.is_some() {
                    app.renderer.invalidate();
                    app.layer_thumbs.clear();
                    app.renumber_frames();
                    let what = if dup {
                        "with a copy of its art"
                    } else {
                        "empty"
                    };
                    app.set_status(format!(
                        "divided into a new frame folder, {what} ({cuts} cut(s))"
                    ));
                } else {
                    app.set_error("frame folder divide failed");
                }
                app.mark_dirty();
            } else {
                fs.frames = keep;
                fs.slot = slot_for(&fs.frames, cut_union);
                app.doc.set_frames(li, fs);
                app.doc.set_active(li);
                app.renumber_frames();
                app.set_status(format!("divided {cuts} panel(s)"));
                app.mark_dirty();
            }
        }
        AppCmd::FrameRect { a, b } => {
            let (w, h) = ((b.0 - a.0).abs(), (b.1 - a.1).abs());
            if w < 8.0 || h < 8.0 {
                app.set_status("drag out the frame's size");
                return;
            }
            let mut rect = [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)];
            // CSP's rectangle-frame magnet (manual, snapping): a dragged
            // edge within reach of the 基本枠 lands ON it — panels are
            // drawn against the guides and hand alignment drifts by
            // pixels. Same guide the New Frame Layer arm and the canvas
            // guides use (book-side aware); far-off edges stay put.
            let right = app.current_page_right().unwrap_or(true);
            let guide = app
                .page
                .as_ref()
                .filter(|p| p.has_guides())
                .map(|p| p.inner_rect_px_on(right))
                .unwrap_or([
                    app.doc.size.0 as f32 * 0.08,
                    app.doc.size.1 as f32 * 0.08,
                    app.doc.size.0 as f32 * 0.92,
                    app.doc.size.1 as f32 * 0.92,
                ]);
            let tol = (app.doc.size.0.min(app.doc.size.1) as f32 * 0.01).clamp(8.0, 32.0);
            for k in 0..4 {
                if (rect[k] - guide[k]).abs() <= tol {
                    rect[k] = guide[k];
                }
            }
            let border = if app.frame_draw_border {
                app.mm_to_px(app.frame_border_mm).max(1.0)
            } else {
                0.0
            };
            let n = app.doc.layers.iter().filter(|l| l.is_frame()).count() + 1;
            app.doc.add_frame_folder_with(
                format!("Frame {n}"),
                FrameSet::single_rect(rect, border),
                app.frame_fill_inside,
            );
            app.renderer.invalidate();
            app.renumber_frames();
            app.set_status("frame folder added");
            app.mark_dirty();
        }
        AppCmd::FramePoly { points } => {
            let f = mn_core::Frame { points };
            if f.points.len() < 3 || f.area() < mn_core::frame::MIN_FRAME_AREA {
                app.set_status("draw a bigger panel shape");
                return;
            }
            if !f.is_simple() {
                app.set_status("that outline crosses itself — try again");
                return;
            }
            let border = if app.frame_draw_border {
                app.mm_to_px(app.frame_border_mm).max(1.0)
            } else {
                0.0
            };
            let n = app.doc.layers.iter().filter(|l| l.is_frame()).count() + 1;
            app.doc.add_frame_folder_with(
                format!("Frame {n}"),
                mn_core::FrameSet {
                    frames: vec![f],
                    border_px: border,
                    slot: None,
                    reading_pin: None,
                    border_ruler: false,
                    color: [0, 0, 0],
                },
                app.frame_fill_inside,
            );
            app.renderer.invalidate();
            app.renumber_frames();
            app.set_status("frame folder added");
            app.mark_dirty();
        }
        AppCmd::FrameCommit { layer, frames } => {
            if app.doc.set_frames(layer, frames) {
                app.mark_dirty();
            }
        }
        AppCmd::FrameFoldersCombine { merge_borders } => {
            // Target: the Object tool's selected frame's folder, else the
            // frame folder owning the active layer. Then the next sibling
            // frame folder in stack order.
            let target = if app.tool == Tool::Object
                && let Some((li, _fi)) = app.object_sel
                && let Some(l) = app.doc.layers.get(li)
                && l.is_frame()
            {
                Some(li)
            } else {
                // Walk ancestors outward to the first frame folder (the
                // paste-target walk, one layer up).
                let mut f = enclosing_folder(&app.doc, app.doc.active);
                while let Some(i) = f
                    && !(app.doc.layers[i].folder && app.doc.layers[i].is_frame())
                {
                    f = enclosing_folder(&app.doc, i);
                }
                f
            };
            let Some(a) = target else {
                app.set_status("no frame folder to combine");
                return;
            };
            let depth = app.doc.layers[a].depth;
            let block_end = app.doc.block_range(a).end;
            let next = (block_end..app.doc.layers.len()).find(|&i| {
                app.doc.layers[i].is_frame()
                    && app.doc.layers[i].folder
                    && app.doc.layers[i].depth == depth
            });
            let Some(b) = next else {
                app.set_status("no sibling frame folder below to combine with");
                return;
            };
            match app.doc.combine_frame_folders(a, b, merge_borders) {
                Some(h) => {
                    app.doc.active = h.saturating_sub(1).max(0).min(h);
                    app.object_sel = None;
                    app.renumber_frames();
                    app.set_status(if merge_borders {
                        "frame folders combined — borders merged"
                    } else {
                        "frame folders combined — shapes kept"
                    });
                    app.mark_dirty();
                }
                None => app.set_status(
                    "those folders cannot combine — they must be siblings and \
                     agree on eye, opacity, blend, border and reading pin",
                ),
            }
        }
        AppCmd::FrameFoldersGroup => {
            // FB-037: same target resolution as the combine, but the
            // partner keeps its own header — a plain parent wraps both.
            let target = if app.tool == Tool::Object
                && let Some((li, _fi)) = app.object_sel
                && let Some(l) = app.doc.layers.get(li)
                && l.is_frame()
            {
                Some(li)
            } else {
                let mut f = enclosing_folder(&app.doc, app.doc.active);
                while let Some(i) = f
                    && !(app.doc.layers[i].folder && app.doc.layers[i].is_frame())
                {
                    f = enclosing_folder(&app.doc, i);
                }
                f
            };
            let Some(a) = target else {
                app.set_status("no frame folder to group");
                return;
            };
            let depth = app.doc.layers[a].depth;
            let block_end = app.doc.block_range(a).end;
            let next = (block_end..app.doc.layers.len()).find(|&i| {
                app.doc.layers[i].is_frame()
                    && app.doc.layers[i].folder
                    && app.doc.layers[i].depth == depth
            });
            let Some(b) = next else {
                app.set_status("no sibling frame folder below to group with");
                return;
            };
            match app.doc.group_frame_folders_common_parent(a, b) {
                Some(h) => {
                    let _ = h;
                    app.object_sel = None;
                    app.set_status("common folder created — originals kept");
                    app.mark_dirty();
                }
                None => app.set_status("those folders cannot group (not siblings)"),
            }
        }
        AppCmd::FrameDelete { layer, frame } => {
            // FB-039: deleting a border is silent; deleting the folder's
            // LAST frame takes its art with it — a one-shot confirm, the
            // status line the ask. (Any other command disarms.)
            if let Some(fs) = app.doc.layers.get(layer).and_then(|l| l.frames()) {
                if fs.frames.len() == 1 {
                    if app.frame_delete_armed == Some((layer, frame)) {
                        let name = app.doc.layers[layer].name.clone();
                        if app.doc.remove_layer(layer) {
                            app.object_sel = None;
                            app.set_status(format!("\"{name}\" and its layers deleted"));
                            app.renumber_frames();
                            app.mark_dirty();
                            return;
                        }
                    }
                    app.frame_delete_armed = Some((layer, frame));
                    app.set_status(
                        "that is the folder's last frame — Delete again to remove the folder AND its layers",
                    );
                    return;
                }
                let mut fs = fs.clone();
                if frame < fs.frames.len() {
                    fs.frames.remove(frame);
                    app.doc.set_frames(layer, fs);
                    app.object_sel = None;
                    app.set_status("frame deleted");
                    app.renumber_frames();
                    app.mark_dirty();
                }
            }
        }
        AppCmd::FrameExtendEdge { at } => {
            // TRIAGE 129 / FB-030. The tap picks the panel edge nearest it,
            // generously (a fingertip on a tablet is not a pixel).
            let Some(li) = frame_target(app) else {
                app.set_status("no frame layer — Layer > New frame border folder first");
                return;
            };
            let mut fs = app.doc.layers[li].frames().expect("is_frame").clone();
            let p = [at.0, at.1];
            let tol = (20.0 / app.viewport.zoom.max(0.01)).max(10.0);
            let hit = fs
                .frames
                .iter()
                .enumerate()
                .filter_map(|(fi, f)| f.edge_near(p, tol).map(|ei| (fi, ei)))
                .next();
            let Some((fi, ei)) = hit else {
                app.set_status("tap ON a panel edge to run it to the page edge");
                return;
            };
            let canvas = (app.doc.size.0 as f32, app.doc.size.1 as f32);
            let bleed = app.mm_to_px(3.0).max(4.0);
            let before = fs.frames[fi].bbox();
            if !fs.extend_to_edge(fi, ei, canvas, bleed) {
                app.set_status("that edge is already out");
                return;
            }
            let closed = fs.frames[fi].bbox() != before && fs.frames.len() > 1;
            app.doc.set_frames(li, fs);
            app.renumber_frames();
            app.set_status(if closed {
                "edge extended — it stops on the next panel, or runs off the page"
            } else {
                "edge extended to the page"
            });
            app.mark_dirty();
        }
        AppCmd::FrameDivideEqually {
            cols,
            rows,
            fit_to_side,
        } => {
            // TRIAGE 129 / FB-023..025. The cheap half of the pair, and the
            // one a page layout actually starts from.
            let Some(li) = frame_target(app) else {
                app.set_status("no frame layer — Layer > New frame border folder first");
                return;
            };
            let mut fs = app.doc.layers[li].frames().expect("is_frame").clone();
            // Which panel: the Object tool's selection when it is on this
            // layer, else the only one there is. Never a guess.
            let fi = match app.object_sel {
                Some((l, f)) if l == li && f < fs.frames.len() => f,
                _ if fs.frames.len() == 1 => 0,
                _ => {
                    app.set_status("pick the panel to divide with the Object tool first");
                    return;
                }
            };
            // The pair the ACTIVE cut sub tool owns — this always read the
            // tight divide-border gutters (1.69/2.29 mm), so "divide into
            // tiers" with the folder sub tool ignored his 9.74 mm 上下 value.
            let (gx, gy) = if app.frame_mode == FrameMode::DivideBorder {
                app.gutter_border_mm
            } else {
                app.gutter_folder_mm
            };
            let cells = fs.frames[fi].divide_equally(
                cols,
                rows,
                app.mm_to_px(gx),
                app.mm_to_px(gy),
                fit_to_side,
            );
            let Some(cells) = cells else {
                app.set_status("that division does not fit this panel");
                return;
            };
            let slot = fs.frames[fi].bbox();
            let n = cells.len();
            fs.frames.splice(fi..fi + 1, cells);
            fs.slot = slot_for(&fs.frames, Some(slot));
            app.doc.set_frames(li, fs);
            app.object_sel = None;
            app.renumber_frames();
            app.set_status(format!("divided into {n} panels ({cols} x {rows})"));
            app.mark_dirty();
        }
        AppCmd::FrameBorderRuler { layer } => {
            // TRIAGE 127 / FB-053-054.
            let Some(fs) = app.doc.layers.get(layer).and_then(|l| l.frames()) else {
                app.set_status("that layer has no frames");
                return;
            };
            let mut fs = fs.clone();
            fs.border_ruler = !fs.border_ruler;
            let on = fs.border_ruler;
            app.doc.set_frames(layer, fs);
            app.sync_frame_rulers();
            app.renderer.invalidate();
            app.layer_thumbs.clear();
            app.set_status(if on {
                "border off — the panel outline is a ruler now; ink it with a pen"
            } else {
                "border back on"
            });
            app.mark_dirty();
        }
        other => return text::run(app, other, cmd_tail),
    }
    run_cmd_tail(app, cmd_tail);
}
