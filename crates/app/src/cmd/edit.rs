//! `AppCmd` arms: the clipboard (copy/cut/paste and the float it
//! opens), materials, and the fill family (live fills, dust, tone,
//! enclose/leftover/lasso).

use super::*;
use super::frames::{enclosing_folder, frame_index_for};
use super::history::{genlines_aim_point, genlines_new_layer};
use super::transform::{commit_transform_drag, selection_bbox};

/// The clipboard's operand (TRIAGE 131): the selection's bounds when one
/// exists, else the layer's populated bounds — canvas-clipped, lifted
/// selection-masked. `None` when there is nothing there.
fn lift_clipboard_source(app: &App) -> Option<mn_core::FloatSource> {
    let l = app.doc.active_layer();
    let rect = if let Some(sel) = &app.doc.selection {
        selection_bbox(sel)
    } else {
        l.tile_bounds()
            .map(|(x, y, w, h)| [x, y, x + w as i32, y + h as i32])
    }?;
    let rect = [
        rect[0].max(0),
        rect[1].max(0),
        rect[2].min(app.doc.size.0 as i32),
        rect[3].min(app.doc.size.1 as i32),
    ];
    if rect[0] >= rect[2] || rect[1] >= rect[3] {
        return None;
    }
    let src = mn_core::transform::lift_region(l, rect, app.doc.selection.as_ref());
    (!src.tiles.is_empty()).then_some(src)
}

/// Store a lifted source as the app's clipboard + the OS clipboard (DIB,
/// best-effort — other apps get the 8-bit copy, we keep the fix15 original).
fn store_clipboard(app: &mut App, src: mn_core::FloatSource) {
    let (bgra, w, h) = crate::clipboard::floatsource_to_bgra(&src);
    let os_ok = crate::clipboard::clipboard_set_dib(&bgra, w as usize, h as usize);
    let (rw, rh) = (src.rect[2] - src.rect[0], src.rect[3] - src.rect[1]);
    app.clipboard = Some(src);
    app.set_status(if os_ok {
        format!("copied {rw}×{rh} px")
    } else {
        format!("copied {rw}×{rh} px (OS clipboard unavailable)")
    });
}

/// Where a panel paste lands (owner HIGH 2026-08-18). `folder` names the
/// frame-folder header whose seal clips the art; `None` with a rect is the
/// selection-bbox rule — a centring target only, the stamp stays on the
/// active layer.
pub(crate) struct PasteTarget {
    pub folder: Option<usize>,
    /// True when the folder already owns the active layer (rule 1): the
    /// stamp goes onto the active layer, exactly as before, only aimed.
    pub owns_active: bool,
    /// Panel (or selection) rect, canvas px, `[x0, y0, x1, y1]`.
    pub rect: [f32; 4],
    /// Status-line name — the folder's layer name.
    pub label: String,
}

/// Paste-to-position target resolution (owner HIGH 2026-08-18), spec order:
/// 1. the frame folder OWNING the active layer (the common case — he is
///    drawing inside a panel and pastes; it goes in that panel),
/// 2. else the panel under the pointer,
/// 3. else the selection's bbox (aim only — no folder),
/// 4. else None → today's behaviour (source coords / view centre).
pub(crate) fn resolve_paste_target(
    doc: &mn_core::Document,
    active: usize,
    pointer_canvas: Option<(f32, f32)>,
) -> Option<PasteTarget> {
    // Rule 1: walk enclosing folders outward to the first frame folder.
    let mut f = enclosing_folder(doc, active);
    while let Some(i) = f {
        if doc.layers[i].is_frame() {
            let fi = frame_index_for(doc, i, active);
            // Invariant: a frame folder holds at least one frame, so `fi`
            // indexes something. An empty set would mean no panel to aim
            // at, which is rule 2's case, not a panic.
            if let Some(fr) = doc.layers[i].frames().and_then(|fs| fs.frames.get(fi)) {
                return Some(PasteTarget {
                    folder: Some(i),
                    owns_active: true,
                    rect: fr.bbox(),
                    label: doc.layers[i].name.clone(),
                });
            }
        }
        f = enclosing_folder(doc, i);
    }
    // Rule 2: the smallest panel containing the pointer, across all frame
    // folders (smallest so a nested re-division wins over its container).
    // Hidden and locked folders are NOT targets: pasting into a folder the
    // artist hid (to see the rough underneath) commits art that instantly
    // disappears, and a locked folder is one he explicitly protected.
    if let Some((px, py)) = pointer_canvas {
        let vis = doc.effective_visibility();
        let mut best: Option<(f32, usize, usize)> = None; // (area, folder, frame)
        for i in 0..doc.layers.len() {
            if !vis.get(i).copied().unwrap_or(true) || doc.layers[i].lock {
                continue;
            }
            let Some(fs) = doc.layers[i].frames() else {
                continue;
            };
            for (j, fr) in fs.frames.iter().enumerate() {
                let b = fr.bbox();
                if px >= b[0] && py >= b[1] && px < b[2] && py < b[3] {
                    let a = (b[2] - b[0]) * (b[3] - b[1]);
                    if best.is_none_or(|(ba, _, _)| a < ba) {
                        best = Some((a, i, j));
                    }
                }
            }
        }
        if let Some((_, i, j)) = best {
            let rect = doc.layers[i].frames().map(|fs| fs.frames[j].bbox())?;
            return Some(PasteTarget {
                folder: Some(i),
                owns_active: false,
                rect,
                label: doc.layers[i].name.clone(),
            });
        }
    }
    // Rule 3: a selection's bbox aims the paste; no folder, active layer.
    doc.selection
        .as_ref()
        .and_then(selection_bbox)
        .map(|r| PasteTarget {
            folder: None,
            owns_active: true,
            rect: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
            label: "selection".into(),
        })
}

/// Paste (TRIAGE 131 + owner HIGH 2026-08-18): internal clipboard wins —
/// full fidelity plus the source coordinates Paste returns to, CSP-style;
/// an OS DIB has no coordinates and drops where the aim says. `Panel`
/// resolves a paste target (frame folder → pointer panel → selection →
/// old behaviour); `InPlace` is the pre-HIGH Ctrl+V verbatim;
/// `Shown` centres on the view.
#[derive(Clone, Copy, PartialEq)]
enum PasteWhere {
    Panel,
    InPlace,
    Shown,
}

fn paste_float(app: &mut App, where_: PasteWhere) {
    let target = if where_ == PasteWhere::Panel {
        // The pointer rule only fires over the canvas, not over a panel
        // that happens to sit on top of the page.
        let p = app.last_pointer;
        let pointer = (!app.shell.owns_pointer(p.0, p.1)).then(|| {
            let c = app.viewport.to_canvas(p.0 as f32, p.1 as f32);
            (c.0, c.1)
        });
        resolve_paste_target(&app.doc, app.doc.active, pointer)
    } else {
        None
    };
    // Owner 2026-08-24: a paste lands on its OWN new layer, committed
    // immediately — no float, no corner handles under the Pen, nothing
    // following a layer switch (the old guards stamped the active layer;
    // the active layer is never a paste target now, so lock/vector state
    // stops mattering).
    let aim = target.as_ref().map(|t| t.rect);
    let src = app.clipboard.clone().or_else(|| {
        let (bgra, w, h) = crate::clipboard::clipboard_get_dib()?;
        // An external paste with no aim drops centred on the view; with one,
        // seed the float at the target's corner so nothing clips away.
        let (vw, vh) = (w as i32 / 2, h as i32 / 2);
        let at = match aim {
            Some(r) => [r[0] as i32, r[1] as i32],
            None => {
                let c = app
                    .viewport
                    .to_canvas(app.canvas_center()[0], app.canvas_center()[1]);
                [c.0 as i32 - vw, c.1 as i32 - vh]
            }
        };
        Some(crate::clipboard::bgra_to_floatsource(
            &bgra,
            w,
            h,
            at,
            app.doc.size.0 as i32,
            app.doc.size.1 as i32,
        ))
    });
    let Some(src) = src.filter(|s| !s.tiles.is_empty()) else {
        app.set_status("clipboard is empty");
        return;
    };
    if where_ == PasteWhere::Shown && target.is_none() {
        open_float_drag(app, src, true);
    } else {
        open_float_aimed(app, src, target.as_ref());
    }
    // ...and commit it NOW. The float-open above is reused for its aim and
    // sizing math only; the drag never survives this call, so no handles
    // can appear under the Pen and no paste state can follow a layer
    // switch. Adjust afterwards with Ctrl+T or the Object tool.
    if let Some(mut drag) = app.transform_drag.take() {
        drag.paste_new_layer = true;
        // ANY folder target (owning or foreign): the new layer lands
        // inside it, so the folder seal clips the art to the panel.
        if let Some(t) = target.as_ref() {
            drag.create_in = t.folder;
        }
        commit_transform_drag(app, drag);
    }
}

/// The shared float-opening core (clipboard pastes AND material pastes,
/// TRIAGE 131/133): build the TransformDrag with `stamp_on_identity` and
/// optionally re-centre on the view. Layer guards are the CALLER's — this
/// takes a non-empty source and opens the move/scale/commit float.
fn open_float_drag(app: &mut App, src: mn_core::FloatSource, center_on_view: bool) {
    let r = src.rect;
    let preview_tex = crate::app::transform_preview(&src, 2048).map(|img| {
        app.shell
            .ctx
            .load_texture("mn.transform.preview", img, egui::TextureOptions::LINEAR)
    });
    let mut drag = crate::app::TransformDrag {
        source: src,
        xform: mn_core::Affine2::IDENTITY,
        bbox: [
            [r[0] as f32, r[1] as f32],
            [r[2] as f32, r[1] as f32],
            [r[2] as f32, r[3] as f32],
            [r[0] as f32, r[3] as f32],
        ],
        sx: 1.0,
        sy: 1.0,
        rad: 0.0,
        tx: 0.0,
        ty: 0.0,
        pivot_override: None,
        gesture: None,
        stamp_on_identity: true,
        // A paste: nothing was lifted off the layer, so the commit must not
        // clear the source rect (Copy is not Cut).
        clear_source: false,
        lift_selection: None,
        create_in: None,
        paste_new_layer: false,
        object_lift: false,
        order: crate::app::MaterialLayerOrder::Above,
        preview_tex,
        mesh: None,
    };
    if center_on_view {
        // Centre the float on the current view through the params model, so
        // the gestures stay consistent from here.
        let pivot = drag.pivot();
        let c = app
            .viewport
            .to_canvas(app.canvas_center()[0], app.canvas_center()[1]);
        drag.set_params(1.0, 1.0, 0.0, c.0 - pivot[0], c.1 - pivot[1]);
    }
    app.transform_drag = Some(drag);
    app.set_status("pasted — drag to move, Enter commits, Esc cancels");
    app.mark_dirty();
}

/// A paste with a resolved target (owner HIGH 2026-08-18): the float opens
/// centred on the panel rect, scaled uniformly DOWN to fit when oversized
/// (never up, never cropped), and — when the active layer is not already
/// inside the target folder — the commit creates the layer inside it so
/// the folder seal clips the art to the panel. Same float semantics as
/// ever: drag immediately, Enter commits, Esc cancels with nothing left.
/// r74's owner-approved paste sizing (uniform down-fit, topmost child) —
/// the clipboard path and every default caller.
pub(crate) fn open_float_aimed(
    app: &mut App,
    src: mn_core::FloatSource,
    target: Option<&PasteTarget>,
) {
    open_float_aimed_sized(
        app,
        src,
        target,
        crate::app::MaterialPasteSize::FitPanel,
        crate::app::MaterialLayerOrder::Above,
    );
}

/// The paste landing with EXPLICIT sizing/order (MT-032/034 — the
/// material palette's choices; the clipboard path keeps r74's defaults).
pub(crate) fn open_float_aimed_sized(
    app: &mut App,
    src: mn_core::FloatSource,
    target: Option<&PasteTarget>,
    size_mode: crate::app::MaterialPasteSize,
    order: crate::app::MaterialLayerOrder,
) {
    let create_in = target.filter(|t| !t.owns_active).and_then(|t| t.folder);
    open_float_drag(app, src, false);
    let Some(drag) = app.transform_drag.as_mut() else {
        return;
    };
    drag.create_in = create_in;
    drag.order = order;
    let status = if let Some(t) = target {
        let r = drag.source.rect;
        let (fw, fh) = ((r[2] - r[0]) as f32, (r[3] - r[1]) as f32);
        if fw > 0.0 && fh > 0.0 {
            // MT-032: one fit, five meanings (CSP's vocabulary, named
            // after the job). The default is r74's owner-approved
            // down-fit, verbatim.
            let (tw, th) = (t.rect[2] - t.rect[0], t.rect[3] - t.rect[1]);
            let (fx, fy) = (tw / fw, th / fh);
            let (mx, my) = match size_mode {
                crate::app::MaterialPasteSize::FitPanel => {
                    let s = fx.min(fy).min(1.0);
                    (s, s)
                }
                crate::app::MaterialPasteSize::AdjustAfter => (1.0, 1.0),
                crate::app::MaterialPasteSize::ExpandFull => {
                    let s = fx.max(fy);
                    (s, s)
                }
                crate::app::MaterialPasteSize::FitToScale => {
                    let s = fx.min(fy);
                    (s, s)
                }
                crate::app::MaterialPasteSize::ToDestination => (fx, fy),
            };
            let pivot = drag.pivot();
            let c = [(t.rect[0] + t.rect[2]) * 0.5, (t.rect[1] + t.rect[3]) * 0.5];
            drag.set_params(mx, my, 0.0, c[0] - pivot[0], c[1] - pivot[1]);
        }
        format!(
            "pasted into {} — drag to move, Enter commits, Esc cancels",
            t.label
        )
    } else {
        "pasted — drag to move, Enter commits, Esc cancels".into()
    };
    app.set_status(status);
}

/// Thin a freehand drag down to seed points at least `step` px apart, as
/// integer canvas pixels. The wand-family gestures (SE-020 shrink-select,
/// FI-003 enclose-and-fill) all want this: a pocket only needs ONE seed,
/// and both flood accumulators skip seeds that land in a pocket they
/// already hold, so the survivors cost nothing extra.
pub(super) fn subsample_path(pts: &[(f32, f32)], step: f32) -> Vec<(i32, i32)> {
    let mut seeds: Vec<(i32, i32)> = Vec::new();
    let mut last: Option<(f32, f32)> = None;
    for &(x, y) in pts {
        if let Some((lx, ly)) = last
            && (x - lx).hypot(y - ly) < step
        {
            continue;
        }
        last = Some((x, y));
        seeds.push((x as i32, y as i32));
    }
    seeds
}

/// Row 160 / `RD-001`–`RD-003`, `RD-007` — the Remove-dust drag.
///
/// The freehand path closes into a polygon, and that polygon is the tool's
/// WINDOW. An existing selection is not thrown away: the window is the
/// intersection of the two, so ants you set still bound the tool exactly
/// the way they bound a fill. The window is installed as the document's
/// selection only for the duration of the op — that is what makes
/// `apply_filter`'s tile gather, its selection clip and its single undo
/// group do all the work here (`mn_core::dust`), and the real selection
/// goes back afterwards.
///
/// The detection runs FIRST, as a query, for two reasons: the status line
/// can then say how much it found, and a drag that finds nothing leaves no
/// undo step behind — the fill family's `paint_region` rule.
fn dust_scrub(app: &mut App, pts: &[(f32, f32)]) {
    app.refresh_tones();
    let o = app.dust_opts;
    let drag = mn_core::Selection::from_polygon(&app.doc, pts);
    let window = match &app.doc.selection {
        Some(cur) if !drag.is_empty() => {
            cur.combine(&drag, &app.doc, mn_core::SelectionOp::Intersect)
        }
        _ => drag,
    };
    if window.is_empty() {
        app.set_status("that drag enclosed nothing — circle the patch to clean");
        return;
    }
    let keep = app.doc.selection.replace(window);
    let found = app.doc.dust_selection(o.mode, o.max_px);
    let Some(found) = found else {
        app.doc.selection = keep;
        app.set_status(format!(
            "nothing under {} px of {} in there",
            o.max_px,
            if o.mode.detects_gaps() { "gap" } else { "dust" }
        ));
        app.mark_dirty();
        return;
    };
    let n = count_coverage(&found);
    if o.select {
        // RD-007: look before you delete. The find REPLACES the selection
        // (RD-008's New/Add/Subtract/Intersect row is `S-002` — absent
        // house-wide, not skipped here specifically).
        app.doc.selection = Some(found);
        app.doc.touch();
        app.set_status(format!(
            "{n} px of {} selected — Delete clears it",
            if o.mode.detects_gaps() { "gaps" } else { "dust" }
        ));
        app.mark_dirty();
        return;
    }
    let color = app.active_color();
    let ok = app.doc.apply_filter(mn_core::Filter::Dust {
        max_px: o.max_px,
        mode: o.mode,
        color,
    });
    app.doc.selection = keep;
    app.set_status(if ok {
        format!("{}: {n} px", o.mode.label())
    } else {
        "that layer will not take pixel edits".into()
    });
    app.mark_dirty();
}

/// How many pixels a selection actually covers — the honest number for a
/// status line, read off the coverage inside its own bounds.
fn count_coverage(sel: &mn_core::Selection) -> u32 {
    let Some([x0, y0, x1, y1]) = sel.bounds() else {
        return 0;
    };
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if mn_core::selected(sel.coverage(x, y)) {
                n += 1;
            }
        }
    }
    n
}

/// The auto-fill tail of a fill's status line. With auto off it is empty;
/// with auto on it names what was measured, because a fill that silently
/// chooses its own numbers teaches the user nothing and cannot be argued
/// with when it gets one wrong.
pub(super) fn auto_note(opts: &mn_core::FillOpts, auto: Option<mn_core::AutoFill>) -> String {
    if !opts.auto {
        return String::new();
    }
    match auto {
        Some(a) => format!(
            " · auto: lines ~{:.0} px → close gap {} px, area {:+} px",
            a.line_px, a.gap_close_px, a.expand_px
        ),
        None => format!(
            " · auto found no lines to measure — kept close gap {} px, area {:+} px",
            opts.gap_close_px, opts.expand_px
        ),
    }
}

pub(super) fn run(app: &mut App, cmd: AppCmd, cmd_tail: CmdTail) {
    match cmd {
        AppCmd::ClearLayer => {
            app.doc.set_op_label("Clear");
            let l = app.doc.active_layer();
            if l.lock {
                app.set_status("layer is locked");
            } else if l.is_vector() {
                app.set_status("Delete clears raster layers — this one is derived from vectors");
            } else if l.records_strokes() {
                // Zeroing the tiles would look like it worked and then hand
                // every stroke back at the next re-derive.
                app.set_status("this layer's ink is recorded — erase the strokes, or delete the layer");
            } else {
                let tiles: Vec<_> = l.tiles().map(|(i, _)| i).collect();
                if tiles.is_empty() {
                    app.set_status("layer is already empty");
                } else {
                    app.doc.begin_op();
                    let li = app.doc.active;
                    for idx in tiles {
                        app.doc.layers[li].tile_mut(idx).data_mut().fill(0);
                    }
                    // Outside a selection the pre-images come back — same
                    // clipping path strokes use.
                    app.doc.mask_op_to_selection();
                    app.doc.end_op();
                    app.set_status(if app.doc.selection.is_some() {
                        "selection cleared"
                    } else {
                        "layer cleared"
                    });
                    app.mark_dirty();
                }
            }
        }

        AppCmd::Copy => match lift_clipboard_source(app) {
            None => app.set_status("nothing to copy"),
            Some(src) => store_clipboard(app, src),
        },
        AppCmd::Cut => {
            let l = app.doc.active_layer();
            if l.lock {
                app.set_status("layer is locked");
            } else if l.is_vector() || l.records_strokes() || l.folder {
                app.set_status("Cut applies to raster layers");
            } else {
                match lift_clipboard_source(app) {
                    None => app.set_status("nothing to cut"),
                    Some(src) => {
                        // Erase exactly the fraction `lift_region` took —
                        // the shared weighted clear (ONE implementation with
                        // `commit_transform`, so the lift/clear pair cannot
                        // drift) — as ONE undo step.
                        let (r, sel) = (src.rect, app.doc.selection.clone());
                        app.doc.begin_op();
                        mn_core::transform::clear_lifted(
                            &mut app.doc.layers[app.doc.active],
                            r,
                            sel.as_ref(),
                        );
                        app.doc.end_op();
                        store_clipboard(app, src);
                        app.mark_dirty();
                    }
                }
            }
        }
        AppCmd::Paste => paste_float(app, PasteWhere::Panel),
        AppCmd::PasteInPlace => paste_float(app, PasteWhere::InPlace),
        AppCmd::PasteShown => paste_float(app, PasteWhere::Shown),
        AppCmd::CompApplyAllPages(i) => {
            let Some(c) = app.doc.comps.get(i).cloned() else {
                return;
            };
            if let Err(e) = app.stash_current_page() {
                app.set_error(e);
                return;
            }
            let (mut ok, mut skip) = (0usize, 0usize);
            let mut updated: Vec<(usize, Vec<u8>)> = Vec::new();
            for (pi, e) in app.pages.iter().enumerate() {
                let Some(b) = &e.bytes else { continue };
                match mn_core::project::bytes_to_doc(b) {
                    Ok(mut d) => {
                        if d.layers.len() == c.vis.len() {
                            c.apply_to(&mut d.layers, None);
                            if let Ok(nb) = mn_core::project::doc_to_bytes(&d) {
                                updated.push((pi, nb));
                                ok += 1;
                            } else {
                                skip += 1;
                            }
                        } else {
                            skip += 1;
                        }
                    }
                    Err(_) => skip += 1,
                }
            }
            // Rewritten bytes get a fresh page rev and a dropped thumbnail,
            // or the Pages panel and the rev-keyed sharp preview keep
            // serving the pre-comp look (batch agent's finding, this wave).
            for (pi, nb) in updated {
                let rev = app.page_rev_next();
                let e = &mut app.pages[pi];
                e.bytes = Some(nb);
                e.rev = rev;
                e.doc_rev = 0;
                e.thumb = None;
            }
            // Restore the active-page invariant (bytes live in `doc`).
            app.pages[app.page_index].bytes = None;
            // SELF-AUDIT (Opus 0ee84f8's named blind spot): the LIVE doc
            // is what the owner sees AND what the next save writes — the
            // loop above only touched the stashed bytes, so the comp
            // silently evaporated from the active page on the next save.
            // Apply it here too, same strict structure check.
            if app.doc.layers.len() == c.vis.len() {
                c.apply_to(&mut app.doc.layers, None);
                app.doc.touch();
                app.mark_dirty();
            }
            app.mark_pages_dirty();
            app.set_status(format!(
                "comp \"{}\" applied to {ok} pages ({skip} skipped — structure mismatch)",
                c.name
            ));
        }
        AppCmd::CompExportAll => {}
        AppCmd::CompExportAllPath(dir) => {
            if app.doc.comps.is_empty() {
                app.set_status("no comps to export — save one first (Layer menu)");
                return;
            }
            let comps = app.doc.comps.clone();
            let stem = if app.story.trim().is_empty() {
                "page".to_owned()
            } else {
                app.story.trim().to_owned()
            };
            let dpi = app.tone_dpi();
            if let Err(e) = app.stash_current_page() {
                app.set_error(e);
                return;
            }
            let mut report = Vec::new();
            // Pages this run wrote at least one image of (see the export
            // reminder). A comp set is a VARIANT of the page rather than
            // the page itself, but it is still that page's art leaving the
            // app, so it counts as an export.
            let mut exported: Vec<usize> = Vec::new();
            // LC-013: a multi-selection (LC-007) exports ONLY those
            // comps; empty selection = everything (CSP's rule).
            let sel = app.comp_multi.clone();
            for (ci, c) in comps.iter().enumerate() {
                if !sel.is_empty() && !sel.contains(&ci) {
                    continue;
                }
                // LC-008 inside the export: every page takes the comp (or
                // keeps its own flags when the structure does not match).
                let sub = dir.join(c.name.replace(
                    |ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
                    "_",
                ));
                let _ = std::fs::create_dir_all(&sub);
                let mut ok = 0usize;
                for (i, e) in app.pages.iter().enumerate() {
                    let Some(b) = &e.bytes else { continue };
                    let Ok(mut d) = mn_core::project::bytes_to_doc(b) else {
                        continue;
                    };
                    if d.layers.len() == c.vis.len() {
                        c.apply_to(&mut d.layers, None);
                    }
                    crate::app::refresh_derived_gpu(&mut d, &mut app.renderer, dpi);
                    let img =
                        mn_core::export::composite_for_export(&d, d.paper_export_background());
                    if img
                        .save(sub.join(format!("{stem}-p{:03}.png", i + 1)))
                        .is_ok()
                    {
                        ok += 1;
                        exported.push(i);
                    }
                }
                report.push(format!("{}: {ok}", c.name));
            }
            app.pages[app.page_index].bytes = None;
            // Idempotent: a page written once per comp is recorded once per
            // comp at the same revision.
            for i in exported {
                app.note_page_exported(i);
            }
            let scope = if sel.is_empty() {
                format!("all {} comps", comps.len())
            } else {
                format!("{} of {} comps", report.len(), comps.len())
            };
            app.set_status(format!(
                "exported {scope} -> {} ({})",
                dir.display(),
                report.join(", ")
            ));
        }
        AppCmd::NewLiveFill(kind) => {
            let from_sel = app.doc.selection.is_some();
            // LP-001: a saved fill/tone default is creation INPUT, not a
            // patch afterwards — the layer's derived-raster stamp is taken
            // inside `add_fill_layer`, so changing the parameters after it
            // would leave the stamp describing a picture nobody asked for.
            let kind = app.layer_defaults.fill_kind(kind);
            // Say WHY the new layer is new when it landed on top of another
            // live layer: the Gradient tool used to overwrite the one you
            // were standing on, so a stack where a replacement used to
            // happen needs a word (friction 6).
            let over = matches!(
                app.doc.active_layer().kind,
                mn_core::LayerKind::Fill(mn_core::FillKind::Tone { .. })
            );
            let li = app.doc.add_fill_layer(kind, from_sel);
            app.apply_layer_defaults(li);
            app.refresh_tones();
            app.set_status(if over {
                "live layer ABOVE the tone — a tone layer is never overwritten; parameters in Tool Property"
            } else {
                "live layer — any brush edits its window; parameters in Tool Property"
            });
            app.mark_dirty();
        }
        AppCmd::SetFillParams(li, kind) => {
            if let Some(l) = app.doc.layers.get(li)
                && matches!(l.kind, mn_core::LayerKind::Fill(cur) if cur != kind)
            {
                // ONE undo step for the whole drag. The params live in
                // `Layer.kind` and the derived raster beside them, so the
                // stack snapshot `record_structure` takes carries BOTH —
                // undo restores the old numbers and the old dots in one
                // swap, no re-derive needed. Ticks inside an open session
                // skip this: the pre-image already on the stack is the
                // pre-SESSION state, which is exactly what Ctrl+Z owes the
                // user. (The drag's FIRST tick lands before the panel has
                // reported the session, so it is the one that records.)
                if app.param_session != Some(li) {
                    let before = app.doc.stack_snapshot();
                    let active = app.doc.active;
                    app.doc.record_structure("Layer parameters", before, active);
                }
                app.doc.layers[li].kind = mn_core::LayerKind::Fill(kind);
                // Persisted state (`mnc-fill`): without the touch, a retint
                // as the session's last action was discarded with no
                // unsaved-changes prompt.
                app.doc.touch();
                app.refresh_tones();
                app.set_status("live layer parameters updated");
                app.mark_dirty();
            }
        }
        AppCmd::ParamEditSession(s) => app.param_session = s,
        AppCmd::NewCorrectionLayer(adj) => {
            let from_sel = app.doc.selection.is_some();
            let li = app.doc.add_correction_layer(adj, from_sel);
            // LP-001: presentation only — the Adjust itself is dialog
            // state, not a style default (see `app::layer_defaults`).
            app.apply_layer_defaults(li);
            app.refresh_tones();
            app.set_status(
                "correction layer — everything below renders through it; any brush edits its window",
            );
            app.mark_dirty();
            // The parameterised kinds go straight into their dialog, CSP
            // style; Invert has nothing to ask.
            if !matches!(adj, mn_core::Adjust::Invert) {
                app.push_cmd(AppCmd::CorrectionEdit);
            }
        }
        AppCmd::CorrectionEdit => app.adjust_begin_live(),
        AppCmd::SetFillMode(m) => {
            app.fill_mode = m;
            app.set_status(match m {
                FillMode::Click => "fill: click an area",
                FillMode::Enclose => "enclose and fill: drag around the areas to fill",
                FillMode::Lasso => "lasso fill: drag the shape to paint",
                FillMode::Leftover => {
                    "leftover pen: scrub across the flat — only the enclosed spots still empty fill"
                }
                FillMode::Dust => "remove dust: drag around the patch to clean",
            });
        }
        AppCmd::SetDustOpts(o) => {
            app.dust_opts = o;
        }
        AppCmd::DustScrub { pts } => dust_scrub(app, &pts),
        AppCmd::ToneRegion(x, y) => crate::app::tone_tool::tone_region(app, x, y),
        AppCmd::SetToneOpts(o) => {
            app.tone_opts = o;
        }
        AppCmd::EncloseFill { pts } => {
            app.refresh_tones();
            // Same subsampling as the SE-020 shrink drag: one seed every
            // ~4 px of travel is plenty, and enclosed_pockets skips seeds
            // that land in a pocket it already has.
            let seeds = subsample_path(&pts, 4.0);
            let color = app.active_color();
            // One measurement for the whole lasso, from its first seed —
            // the pockets and the outer set must agree on the numbers.
            let first = seeds.first().copied().unwrap_or((0, 0));
            let (opts, auto) = mn_core::fill::resolve_auto(&app.doc, first, &app.fill_opts);
            if app.fill_opts.auto {
                app.fill_auto = auto;
            }
            let (n, pockets) = mn_core::fill::enclose_and_fill(&mut app.doc, &seeds, color, &opts);
            app.set_status(if n > 0 {
                format!(
                    "{pockets} closed areas filled ({n} px){}",
                    auto_note(&app.fill_opts, auto)
                )
            } else {
                "nothing enclosed — drag right around the areas to fill".into()
            });
            app.mark_dirty();
        }
        AppCmd::LeftoverFill { pts } => {
            app.refresh_tones();
            // Denser seeding than the enclose lasso: the holes this tool
            // exists for are a few pixels wide, and a seed every 4 px of
            // travel would scrub straight over them. `leftover_fill` drops
            // every seed standing on finished colour, so the extra points
            // cost nothing on the flat — one flood per distinct pocket.
            let seeds = subsample_path(&pts, 1.0);
            let color = app.active_color();
            let first = seeds.first().copied().unwrap_or((0, 0));
            let (opts, auto) = mn_core::fill::resolve_auto(&app.doc, first, &app.fill_opts);
            if app.fill_opts.auto {
                app.fill_auto = auto;
            }
            let (n, pockets) = mn_core::fill::leftover_fill(&mut app.doc, &seeds, color, &opts);
            app.set_status(if n > 0 {
                format!(
                    "{pockets} leftover spot(s) filled ({n} px){}",
                    auto_note(&app.fill_opts, auto)
                )
            } else {
                "nothing left over under that drag — it only fills enclosed spots that are still empty".into()
            });
            app.mark_dirty();
        }
        AppCmd::LassoFill { pts } => {
            app.refresh_tones();
            let color = app.active_color();
            let path: Vec<[f32; 2]> = pts.iter().map(|&(x, y)| [x, y]).collect();
            app.doc.set_op_label("Lasso fill");
            if app.doc.fill_polygon(&path, color, 1.0) {
                app.set_status("lasso filled");
            } else {
                app.set_status("lasso fill needs a raster layer (unlocked)");
            }
            app.mark_dirty();
        }
        AppCmd::PasteMaterial { path, tile } => {
            // plans/05 item 6c: the material's OWN paste settings win, the
            // palette's globals are the fallback for untagged materials.
            let own = app
                .materials
                .iter()
                .find(|m| m.path == path)
                .map(|m| crate::app::materials::MaterialPaste::from_tags(&m.tags))
                .unwrap_or_default();
            let tile = own.tile.unwrap_or(tile);
            let tone = own.tone.unwrap_or(app.material_tone);
            let size = own.size.unwrap_or(app.material_size);
            let order = own.order.unwrap_or(app.material_order);
            // A GENERATOR material (`<name>.gen.json`) places LIVE: a new
            // layer carrying the spec, with Object-tool handles from the
            // first click. No bitmap is decoded on this path — the whole
            // point is that the placed lines stay re-aimable. Focus lines
            // converge where you clicked; the rest is what the material
            // stores (a material is reusable, a position is not).
            if let Some(mut spec) = crate::app::materials::read_gen_spec(&path) {
                if spec.radial() {
                    (spec.a, spec.b) = genlines_aim_point(app);
                }
                app.material_note_use(&path);
                match genlines_new_layer(app, spec, None) {
                    Some(name) => app.set_status(format!(
                        "{name} placed live — the Object tool edits the handles"
                    )),
                    None => app.set_status(
                        "that generator material produced nothing on this canvas — Layer ▸ Edit effect lines to widen it",
                    ),
                }
                return;
            }
            // A TONE material places a LIVE TONE LAYER, never pixels.
            //
            // CSP's model, which our engine already implements: a tone is a
            // fill layer plus a mask, and the SCREEN — frequency, angle,
            // density — is canvas-absolute. It does not scale with the area
            // it covers. Pasting a tone sheet as a raster float broke both
            // halves of that: resizing the float resized the DOTS (owner
            // report), and a sheet dropped on a page covered one rectangle
            // instead of the page. So: fill the selection, or the whole
            // canvas when there is none, with parameters that stay editable
            // in Layer Property a week later.
            if let Some(spec) = app.material_tone_spec(&path) {
                app.material_note_use(&path);
                // `add_fill_layer` cuts the window from the selection and
                // records ONE structural undo step — the same single press
                // the Tone tool's gesture costs.
                let from_sel = app.doc.selection.is_some();
                let i = app.doc.add_fill_layer(
                    mn_core::FillKind::Tone {
                        tone: spec.tone,
                        density: spec.density,
                    },
                    from_sel,
                );
                app.refresh_tones();
                app.renderer.invalidate();
                // What actually happened, not what was asked for: an empty
                // selection yields no mask, and that is the whole page.
                let where_ = if app.doc.layers[i].mask.is_some() {
                    "the selection"
                } else {
                    "the page"
                };
                app.set_status(format!(
                    "tone filled {where_} — Frequency/Density/Angle in Layer Property"
                ));
                app.mark_dirty();
                return;
            }
            // Same paste-to-position rule as Ctrl+V (owner HIGH): a tone
            // dropped into its panel is the same gesture. The tiling
            // variant stays canvas-wide by design — no aiming there.
            let target = if tile {
                None
            } else {
                let p = app.last_pointer;
                let pointer = (!app.shell.owns_pointer(p.0, p.1)).then(|| {
                    let c = app.viewport.to_canvas(p.0 as f32, p.1 as f32);
                    (c.0, c.1)
                });
                resolve_paste_target(&app.doc, app.doc.active, pointer)
            };
            let creates_layer = target
                .as_ref()
                .is_some_and(|t| !t.owns_active && t.folder.is_some());
            if !creates_layer {
                let l = app.doc.active_layer();
                if l.lock {
                    app.set_status("layer is locked");
                    return;
                }
                if l.is_vector() || l.records_strokes() || l.folder {
                    app.set_status("Material pastes target raster layers");
                    return;
                }
            }
            let Ok(img) = image::open(&path) else {
                app.set_status(format!("material failed to load: {}", path.display()));
                return;
            };
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            // BGRA byte order (the clipboard module's conversion contract).
            let mut bgra = rgba.into_raw();
            for px in bgra.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            let (cw, ch) = (app.doc.size.0 as usize, app.doc.size.1 as usize);
            let mut src = if tile {
                // The owner's tiling: one float covering the whole canvas in
                // N×N copies — usable as a mask to draw through.
                let mut tiled = vec![0u8; cw * ch * 4];
                for y in 0..ch {
                    let sy = y % h as usize;
                    let srow = &bgra[sy as usize * w as usize * 4..];
                    for x in 0..cw {
                        let sx = x % w as usize;
                        tiled[(y * cw + x) * 4..(y * cw + x) * 4 + 4]
                            .copy_from_slice(&srow[sx * 4..sx * 4 + 4]);
                    }
                }
                crate::clipboard::bgra_to_floatsource(
                    &tiled,
                    cw as u32,
                    ch as u32,
                    [0, 0],
                    cw as i32,
                    ch as i32,
                )
            } else {
                // Aiming at a panel seeds the float at the panel corner so
                // nothing clips away; open_float_aimed re-centres it.
                let mut c = target
                    .as_ref()
                    .map(|t| (t.rect[0] + w as f32 * 0.5, t.rect[1] + h as f32 * 0.5))
                    .unwrap_or_else(|| {
                        app.viewport
                            .to_canvas(app.canvas_center()[0], app.canvas_center()[1])
                    });
                if c.0 < 0.0
                    || c.1 < 0.0
                    || c.0 >= app.doc.size.0 as f32
                    || c.1 >= app.doc.size.1 as f32
                {
                    // A degenerate view (headless tests, a shell that has
                    // not laid out yet) — paste at the document centre
                    // rather than fully off-canvas.
                    c = (app.doc.size.0 as f32 * 0.5, app.doc.size.1 as f32 * 0.5);
                }
                let at = [c.0 as i32 - w as i32 / 2, c.1 as i32 - h as i32 / 2];
                crate::clipboard::bgra_to_floatsource(
                    &bgra,
                    w,
                    h,
                    at,
                    app.doc.size.0 as i32,
                    app.doc.size.1 as i32,
                )
            };
            if src.tiles.is_empty() {
                app.set_status("material is empty or fully off-canvas");
                return;
            }
            // MT-014 Toning: the material's ink renders as the document's
            // screentone — the tone engine's own raster (canvas-continuous
            // screen, ink coverage from the source pixels), so an arbitrary
            // image becomes printable on a mono page.
            if tone {
                let p = mn_core::ToneParams::default();
                let dpi = app.tone_dpi();
                let mut toned = std::collections::HashMap::new();
                for (idx, t) in &src.tiles {
                    let out = mn_core::tone::rasterize_tile(t, idx.origin(), &p, dpi);
                    if out.alpha_sum() > 0 {
                        toned.insert(*idx, std::sync::Arc::new(out));
                    }
                }
                src.tiles = toned;
                if src.tiles.is_empty() {
                    app.set_status("material tones to nothing (no ink)");
                    return;
                }
            }
            app.material_note_use(&path);
            let n = if tile { " (tiled)" } else { "" };
            let t = if tone { " (toned)" } else { "" };
            let into = target
                .as_ref()
                .map(|tg| format!(" into {}", tg.label))
                .unwrap_or_default();
            open_float_aimed_sized(app, src, target.as_ref(), size, order);
            app.set_status(format!(
                "material {} pasted{n}{t}{into} — drag to move, Enter to commit",
                path.file_stem()
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default()
            ));
        }
        AppCmd::MaterialAddFolder(p) => {
            if app.material_folders.iter().any(|f| *f == p) {
                app.set_status("folder already in the bank");
                return;
            }
            app.material_folders.push(p.clone());
            app.materials_scan();
            app.layout.note_materials(
                &app.user_material_folders(),
                &serde_json::to_string(&app.material_uses).unwrap_or_default(),
            );
            app.set_status(format!(
                "material folder added — {} items",
                app.materials.len()
            ));
        }
        AppCmd::MaterialRescan => {
            app.materials_scan();
            app.set_status(format!("rescanned — {} materials", app.materials.len()));
        }
        AppCmd::MaterialSetTags { path, tags } => {
            let name = path
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !app.material_set_user_tags(&path, &tags) {
                app.set_status(format!(
                    "could not write {} beside {name} — is the folder read-only?",
                    crate::app::materials::TAGS_FILE
                ));
                return;
            }
            let now = tags.trim();
            app.set_status(if now.is_empty() {
                format!("cleared \"{name}\"'s tags")
            } else {
                format!("tagged \"{name}\": {now}")
            });
        }
        other => return transform::run(app, other, cmd_tail),
    }
    run_cmd_tail(app, cmd_tail);
}
