//! `AppCmd` arms: the transform box (free/mesh/puppet), the
//! selection verbs, and colour picking.

use super::*;
use super::edit::{auto_note, subsample_path};

/// The TransformCommit arm's commit body, extracted so the paste path can
/// call it DIRECTLY (owner 2026-08-24: a paste commits onto its own new
/// layer immediately — no float, no handles under the Pen, nothing
/// following a layer switch). Assumes the caller already decided this
/// drag must stamp; the identity-cancel check is the arm's, not ours.
pub(super) fn commit_transform_drag(app: &mut App, drag: crate::app::TransformDrag) {
    // A LIFTED float off a stroke-recording layer: the records move and
    // the raster re-derives, so the ink stays editable after the move.
    // Pastes (never lifted) keep the raster path — they land on a fresh
    // raster layer anyway.
    if drag.clear_source
        && drag.create_in.is_none()
        && !drag.paste_new_layer
        && drag.mesh.is_none()
        && app.doc.active_layer().records_strokes()
    {
        let li = app.doc.active;
        let ok = transform_strokes(app, li, &drag.xform, drag.lift_selection.as_ref(), "Transform");
        app.set_status(if ok {
            "transform committed — the strokes moved with it"
        } else {
            "transform refused"
        });
        return;
    }
    // Paste-into-panel (owner HIGH): the fresh layer lands INSIDE the
    // frame folder as its topmost child and active, so the stamp below
    // hits it and the folder seal clips the art to the panel. The layer
    // add and the stamp record separately and are wrapped into ONE
    // "Paste" press at the end; a canceled float leaves nothing behind.
    let mut refused = false;
    // Paste into a selection (owner 2026-08-21). A paste is a float that
    // never lifted anything (`!clear_source`); with ants up it must arrive
    // MASKED to them. Two shapes, and exactly one of them fires per
    // commit — applying both would weight a feathered edge twice:
    //  · a paste that CREATES its layer gets a non-destructive layer mask
    //    built from the selection's coverage (the user can disable it to
    //    reveal the whole paste), or
    //  · a paste that stamps an existing layer is clamped to the coverage
    //    inside the commit's own op.
    let mut masked = false;
    // A paste that creates its layer records several steps (Structure
    // add, the stamp, maybe a reorder) — wrapped into ONE "Paste" press
    // below, now that structural ops record instead of clearing.
    let created = drag.create_in.is_some() || drag.paste_new_layer;
    let ops_before = app.doc.op_count();
    if let Some(folder) = drag.create_in {
        // Index captured at paste time; anything that reshuffled the
        // stack while the float was open must not silently redirect the
        // stamp.
        let ok = app.doc.layers.get(folder).is_some_and(|l| l.is_frame())
            && app.doc.add_layer_in_folder(folder, "Pasted").is_some();
        refused = !ok;
        // The mask rides the layer that was just created, so there is no
        // prior mask to restore and nothing to record — undoing the add's
        // Structure step takes the layer and its mask away together.
        if ok && !drag.clear_source {
            let m = app
                .doc
                .selection
                .as_ref()
                .and_then(|s| mn_core::fill_layer::mask_from_selection(&app.doc, s));
            if let Some(m) = m {
                let li = app.doc.active;
                app.doc.layers[li].mask = Some(m);
                app.renderer.invalidate();
                masked = true;
            }
        }
    } else if drag.paste_new_layer {
        // Owner 2026-08-24: no folder target — the paste still gets its
        // OWN layer, above the active one. No refusal mode:
        // add_layer_above always lands.
        app.doc.add_layer_above(app.doc.active, "Pasted");
        if !drag.clear_source {
            let m = app
                .doc
                .selection
                .as_ref()
                .and_then(|s| mn_core::fill_layer::mask_from_selection(&app.doc, s));
            if let Some(m) = m {
                let li = app.doc.active;
                app.doc.layers[li].mask = Some(m);
                app.renderer.invalidate();
                masked = true;
            }
        }
    }
    // The stamping case: no fresh layer to hang a mask on, so the
    // coverage is baked at commit (undoable in one step).
    let clamp = !drag.clear_source
        && drag.create_in.is_none()
        && !drag.paste_new_layer
        && app.doc.selection.is_some();
    masked |= clamp;
    if refused {
        app.set_status("transform refused — target folder is gone");
    } else {
        // commit_transform brackets its own single undo op. The source
        // clear (lifted floats only) uses the LIFT-TIME selection:
        // deselecting or re-lassoing while the float was open must not
        // change what gets erased.
        app.doc.set_op_label("Transform");
        // Row 53: a mesh drag resamples through the deformed quads and
        // hands the buffer to the commit's resampled seam; the affine
        // exists only to widen the destination loop past the source
        // rect when the lattice stretches outward.
        let (mesh_xf, mesh_buf) = match &drag.mesh {
            Some(m) => {
                let (dst, buf) = mn_core::mesh::warp_buffer(&drag.source, &m.pts, m.n);
                let xf = mn_core::mesh::cover_affine(drag.source.rect, &m.pts);
                (xf, Some((buf, dst)))
            }
            None => (drag.xform, None),
        };
        let ok = mn_core::transform::commit_transform(
            &mut app.doc,
            &drag.source,
            &mesh_xf,
            drag.lift_selection.as_ref(),
            drag.clear_source,
            clamp,
            mesh_buf
                .as_ref()
                .map(|(b, r)| (b.as_slice(), *r)),
            // `I-005`. A MESH drag already resampled through `warp_buffer`
            // and hands the finished buffer in above, so the kernel does
            // not reach it — the Tool Property row says so rather than
            // sitting there looking live (see `interp_row`).
            app.transform_interp,
        );
        // LM-009: a pure translation drags a LINKED mask with the art
        // (the hole stays over the same ink); scale/rotate/skew leave it
        // (mask resampling is a later cut). Raster masks are pixel grids
        // — the translation rounds. Its own mask-op undo group: the
        // dual-step convention (content + mask), same as the Object
        // tool's frame move.
        if ok {
            let li = app.doc.active;
            let pure_t = drag.xform.m == mn_core::Affine2::IDENTITY.m
                && (drag.xform.t[0] != 0.0 || drag.xform.t[1] != 0.0);
            // Lifted floats only: a PASTE translation moves pasted
            // pixels, not the layer's art, so the layer's mask must stay
            // where its ink is.
            if pure_t
                && drag.clear_source
                && let Some(l) = app.doc.layers.get_mut(li)
                && l.mask.is_some()
                && l.mask_linked
            {
                let dx = drag.xform.t[0].round() as i32;
                let dy = drag.xform.t[1].round() as i32;
                app.doc.mask_op_begin();
                if let Some(l) = app.doc.layers.get_mut(li)
                    && let Some(m) = &mut l.mask
                {
                    m.tiles = mn_core::doc::shift_tile_map(&m.tiles, dx, dy);
                    m.revision = mn_core::tile::next_revision();
                }
                app.doc.mask_op_end();
                app.renderer.invalidate();
            }
            // MT-034: where the pasted layer sits in the panel folder
            // (the palette dropdown set drag.order; Above is the default).
            if drag.order != crate::app::MaterialLayerOrder::Above
                && let Some(folder) = drag.create_in
                // add_layer_in_folder inserts AT the header index, so the
                // header moved to folder + 1 with the new layer.
                && app.doc.layers.get(folder + 1).is_some_and(|l| l.folder)
            {
                let folder = folder + 1;
                let li = app.doc.active;
                let to = match drag.order {
                    crate::app::MaterialLayerOrder::BottomOfPanel => {
                        Some(app.doc.children_range(folder).start)
                    }
                    crate::app::MaterialLayerOrder::Above => None,
                };
                if let Some(to) = to {
                    app.doc.move_layer(li, to);
                }
            }
        }
        app.set_status(match (ok, masked) {
            (true, true) if drag.paste_new_layer => {
                "pasted onto a new layer — masked by the selection"
            }
            (true, false) if drag.paste_new_layer => {
                "pasted onto a new layer — Ctrl+T or the Object tool to adjust"
            }
            (true, true) => "pasted into the selection — masked",
            (true, false) => "transform committed",
            (false, _) => "transform refused",
        });
        if created {
            let pushed = app.doc.op_count().saturating_sub(ops_before) as usize;
            if pushed > 1 {
                app.doc.wrap_recent("Paste", pushed.min(app.doc.undo_len()));
            }
        }
    }
}

/// The selection-combine op for one gesture: held modifiers OVERRIDE the
/// persistent 4-way mode (the owner's everyday path — Shift = add,
/// Alt = subtract, Shift+Alt = intersect).
pub fn effective_sel_op(
    shift: bool,
    alt: bool,
    persistent: mn_core::SelectionOp,
) -> mn_core::SelectionOp {
    match (shift, alt) {
        (true, true) => mn_core::SelectionOp::Intersect,
        (true, false) => mn_core::SelectionOp::Add,
        (false, true) => mn_core::SelectionOp::Subtract,
        (false, false) => persistent,
    }
}

/// The lift region for whole-layer/selection-wide ops (Transform, Flip):
/// the selection's bounds when one exists, else the layer's populated tile
/// bounds — canvas-clipped either way.
pub(crate) fn transform_lift_rect(app: &App) -> Option<[i32; 4]> {
    let l = app.doc.active_layer();
    // No selection: the box hugs the INK (CSP's bounding box sits on the
    // drawing). The tile-aligned bounds put the handles up to 63 px off
    // the art and the pivot — so the centre of every rotation and of every
    // standalone Flip — off its centre, which moved the art on a flip.
    let rect = if let Some(sel) = &app.doc.selection {
        selection_bbox(sel)
    } else {
        l.ink_bounds()
    };
    rect.map(|r| {
        [
            r[0].max(0),
            r[1].max(0),
            r[2].min(app.doc.size.0 as i32),
            r[3].min(app.doc.size.1 as i32),
        ]
    })
}

/// The body of both "import an image as a layer" routes (File ▸ Import and
/// a dropped file), `draft` = land it as a 下書き layer.
///
/// **Workflow audit #3.** The import used to be a 1:1 pixel dump: native
/// size, centred, anything past the canvas edge silently clipped away by
/// [`mn_core::Document::add_layer_from_image`], and no way to move it
/// afterwards short of hunting for Transform. Three things changed:
///
/// * **Fit.** An image LARGER than the page shrinks to fit. Never enlarged
///   — scaling a small asset up is a guess, and the transform below is the
///   place to make that guess by hand.
/// * **Placement.** The transform gesture we already have is armed, so the
///   first thing the user does with the imported image is put it where it
///   goes; Enter commits, Esc cancels (which leaves the layer, not the
///   import, since the import is its own undo step).
/// * **Draft.** `draft` sets the flag, so a reference photo or a scanned
///   rough shows on screen and never reaches the export.
///
/// IO-043's selection-as-mask behaviour is untouched. When a selection IS
/// active the transform is deliberately NOT armed: the selection already
/// said where the image goes, and dragging lifted pixels out from under a
/// freshly built layer mask reads as a bug, not a feature.
/// Edit ▸ Transform / Flip on a STROKE-RECORDING layer (CSP transforms
/// vector layers; ours refused them until this): the recorded points go
/// through `xf` — all of them, or with `sel` the control points under the
/// ants — and the raster re-derives from the moved records, ONE undo step
/// (`end_op_vector_set` carries the record and the pixels together). The
/// float's own resample is never used: a replay of the moved strokes is
/// the vector layer's truth, and it keeps the line editable afterwards.
/// `false` = nothing moved (no records, or none under the selection).
pub(super) fn transform_strokes(
    app: &mut App,
    li: usize,
    xf: &mn_core::Affine2,
    sel: Option<&mn_core::selection::Selection>,
    label: &str,
) -> bool {
    let Some(before) = app.doc.layers.get(li).and_then(|l| l.strokes.clone()) else {
        return false;
    };
    let mut after = before.clone();
    let moved = after.transform(xf, |x, y| {
        sel.is_none_or(|s| s.coverage(x.floor() as i32, y.floor() as i32) > 0)
    });
    if !moved {
        return false;
    }
    app.doc.begin_op_on(li);
    app.doc.layers[li].strokes = Some(after);
    // The Object tool's stroke pick indexes the record it just replaced.
    app.vector_sel = None;
    app.rederive_vector_layer(li);
    app.doc.end_op_vector_set(before, label);
    app.renderer.invalidate();
    true
}

fn import_image_layer(app: &mut App, path: &std::path::Path, draft: bool) {
    let mut img = match image::open(path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            app.set_error(format!("import failed: {e}"));
            return;
        }
    };
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported".to_owned());
    let (iw, ih) = (img.width(), img.height());
    // I02: an asset that declares its own resolution (PNG `pHYs`, JPEG
    // JFIF density) keeps its PHYSICAL size here — a 350 dpi scan lands
    // BIGGER on a 600 dpi page, not smaller, which is what CSP does and
    // what the artist meant when they scanned at 350. Files that say
    // nothing, and works with no dpi, are untouched.
    let asset_dpi = app.scale_import_to_page_dpi(&mut img, path);
    let (sw, sh) = (img.width(), img.height());
    let (pw, ph) = app.doc.size;
    let fitted = if sw > pw || sh > ph {
        let s = (pw as f32 / sw as f32).min(ph as f32 / sh as f32);
        image::imageops::resize(
            &img,
            ((sw as f32 * s).round() as u32).max(1),
            ((sh as f32 * s).round() as u32).max(1),
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };
    let (fw, fh) = (fitted.width(), fitted.height());
    // IO-043: a selection turns the import into a masked import. Both
    // import routes — File ▸ Import ▸ Image and a dropped file — arrive
    // here, which is why the rule holds for every route without either of
    // them knowing about it.
    let (at, masked) = app.doc.add_layer_from_image_masked(name, &fitted);
    if draft {
        app.doc.set_layer_draft(at, true);
    }
    app.renderer.invalidate();
    let armed = !masked && {
        dispatch(app, AppCmd::TransformStart);
        app.transform_drag.is_some()
    };
    let mut s = if masked {
        format!("imported {iw}x{ih} — masked to the selection (delete the mask to see it all)")
    } else {
        format!("imported {iw}x{ih} as a layer")
    };
    if let Some(d) = asset_dpi.filter(|_| (sw, sh) != (iw, ih)) {
        s.push_str(&format!(
            " — the file says {d} dpi, so it came in at {sw}x{sh} for this {} dpi page",
            app.work_dpi().unwrap_or_default()
        ));
    }
    if (fw, fh) != (sw, sh) {
        s.push_str(&format!(" — scaled to {fw}x{fh} to fit the page"));
    }
    if draft {
        s.push_str(" — draft layer: on screen, never exported");
    }
    if armed {
        s.push_str(" — drag to place it, Enter commits, Esc cancels");
    }
    app.set_status(s);
}

/// Open the Transform float for layer `li`'s content over the canvas-
/// clipped lift rect `r` — the shared body of TransformStart and the
/// Object tool's raster-ink fallback (owner 2026-08-24: the Object tool
/// can grab e.g. the lineart and drag it immediately). False = nothing
/// liftable there. The caller owns the guards (lock / raster) and the
/// status line.
pub(crate) fn open_layer_transform(app: &mut App, li: usize, r: [i32; 4]) -> bool {
    let src = {
        let l = &app.doc.layers[li];
        mn_core::transform::lift_region(l, r, app.doc.selection.as_ref())
    };
    if src.tiles.is_empty() {
        return false;
    }
    // The overlay preview is uploaded once, here; the drag then only
    // moves the quad (GPU-drawn).
    let preview_tex = crate::app::transform_preview(&src, 2048).map(|img| {
        app.shell
            .ctx
            .load_texture("mn.transform.preview", img, egui::TextureOptions::LINEAR)
    });
    app.transform_drag = Some(crate::app::TransformDrag {
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
        stamp_on_identity: false,
        // A genuine lift off the layer: commit clears the source,
        // weighted by the selection as it stands right now.
        clear_source: true,
        lift_selection: app.doc.selection.clone(),
        create_in: None,
        paste_new_layer: false,
        object_lift: false,
        order: crate::app::MaterialLayerOrder::Above,
        preview_tex,
        mesh: None,
    });
    true
}

/// Axis-aligned bounding box of a selection, [x0, y0, x1, y1]. The
/// COVERAGE decides: `outline` is one island of a multi-island mask and
/// is empty for a sub-half feather, so it cannot aim an operand rect.
pub(super) fn selection_bbox(sel: &mn_core::Selection) -> Option<[i32; 4]> {
    sel.bounds()
}

/// CO-023, the eyedropper's half of the Color Set: when the user has asked
/// for it, a picked colour joins the set. Returns the tail to hang on the
/// pick's status line, so the palette never grows without saying so.
///
/// Three things keep this from turning the set into landfill. It is OFF by
/// default (the Recent strip already remembers picks, and forgets them
/// again — that is the right home for automatic colours). It de-duplicates,
/// so sampling the same ink twenty times adds one swatch. And it stops at
/// [`crate::app::SWATCH_CAP`] rather than growing without end — the `+`
/// button and a `.gpl` import are deliberate acts and are never refused,
/// but nothing that happens behind the user gets to fill his palette.
fn register_picked(app: &mut App, rgb: [f32; 3]) -> &'static str {
    use crate::app::PickReg;
    match crate::app::pick_registration(app.layout.auto_swatch, &app.swatches, rgb) {
        PickReg::Off => "",
        PickReg::Duplicate => " — already in the Color Set",
        PickReg::Full => " — Color Set full, not added",
        PickReg::Added => {
            app.swatches
                .push(mn_core::palette::Swatch::new(mn_core::palette::quantize8(
                    rgb,
                )));
            crate::app::save_swatches(&app.swatches);
            " — added to the Color Set"
        }
    }
}

/// The eyedropper's whole sample (E-014 + E-016): the box the size covers,
/// taken from the layers the Reference row names, averaged in linear light.
/// `None` when the pick itself is off-canvas.
///
/// Also called from the overlay each paint to colour the picker ring (E-017),
/// so it must stay a per-pick cost — the composite branch is one tile walk for
/// the whole box, the other two are direct tile reads.
pub(crate) fn pick_color(
    doc: &mn_core::Document,
    x: i32,
    y: i32,
    opts: EyedropOpts,
) -> Option<[u8; 3]> {
    use mn_core::FillRefer;
    let (x0, y0, w, h) = mn_core::export::sample_box(doc.size, x, y, opts.size)?;
    // The reference SET (RF-001), even where the layers' own eyes are off.
    // Nothing marked: fall back to what you see, exactly as the fill tool
    // does (`fill::flood_region`) — a silent empty pick would be worse.
    let refs = match opts.refer {
        FillRefer::Reference => doc.reference_layers(),
        _ => Vec::new(),
    };
    if opts.refer == FillRefer::All || (opts.refer == FillRefer::Reference && refs.is_empty()) {
        return mn_core::export::composite_pixel_avg(doc, x, y, opts.size);
    }
    let mut samples = Vec::with_capacity((w * h) as usize);
    for py in y0..y0 + h as i32 {
        for px in x0..x0 + w as i32 {
            samples.push(match opts.refer {
                FillRefer::Active => layer_pixel_over_white(doc.active_layer(), px, py),
                _ => layers_pixel_over_white(doc, &refs, px, py),
            });
        }
    }
    mn_core::export::average_srgb(&samples)
}

/// One layer's own colour at a canvas pixel, over white (the eyedropper's
/// "pick from layer" sub tool). Bounds are the caller's business —
/// `sample_box` has already clipped the box to the canvas.
fn layer_pixel_over_white(layer: &mn_core::Layer, x: i32, y: i32) -> [u8; 3] {
    let idx = mn_core::TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    let p = layer
        .display_tile(idx)
        .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
        .unwrap_or([0; 4]);
    // Premultiplied fix15 over white.
    let a = p[3] as u32;
    let ch = |c: u16| -> u8 {
        let v = c as u32 + (32768 - a);
        ((v.min(32768) * 255 + 16384) / 32768) as u8
    };
    [ch(p[0]), ch(p[1]), ch(p[2])]
}

/// The reference SET at one canvas pixel, composited bottom→top over white —
/// the single-pixel twin of `fill.rs`'s canvas-sized `layers_over_white`, so
/// the eyedropper and the fill tool sample the same stack the same way.
/// `indices` must be in stack order (bottom first), which is what
/// `Document::reference_layers` returns.
fn layers_pixel_over_white(doc: &mn_core::Document, indices: &[usize], x: i32, y: i32) -> [u8; 3] {
    let idx = mn_core::TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    // Accumulate in fix15 straight RGB on white paper, quantize once.
    let mut acc = [32768u32; 3];
    for &li in indices {
        let Some(layer) = doc.layers.get(li) else {
            continue;
        };
        let Some(tile) = layer.display_tile(idx) else {
            continue;
        };
        let p = tile.pixel((x - ox) as usize, (y - oy) as usize);
        let inv = 32768 - p[3] as u32;
        for c in 0..3 {
            acc[c] = p[c] as u32 + acc[c] * inv / 32768;
        }
    }
    std::array::from_fn(|c| ((acc[c] * 255 + 16384) / 32768) as u8)
}

pub(super) fn run(app: &mut App, cmd: AppCmd, cmd_tail: CmdTail) {
    match cmd {
        AppCmd::TransformStart => {
            let l = app.doc.active_layer();
            if l.lock {
                app.set_status("layer is locked");
            } else if l.is_vector() || l.folder {
                app.set_status("Transform applies to raster and vector-ink layers");
            } else {
                // A stroke-recording layer lifts its raster for the preview
                // like any other; the COMMIT moves the records and
                // re-derives (`transform_strokes`), so the ink stays
                // editable — CSP transforms vector layers the same way.
                // Source rect: the selection's bounds when one exists, else
                // the layer's populated tile bounds (shared with Flip).
                match transform_lift_rect(app) {
                    Some(r) if r[0] < r[2] && r[1] < r[3] => {
                        if open_layer_transform(app, app.doc.active, r) {
                            app.set_status(
                                "transform: drag inside to move, corners to scale, outside to rotate — Enter commits, Esc cancels",
                            );
                            app.mark_dirty();
                        } else {
                            app.set_status("nothing to transform");
                        }
                    }
                    _ => app.set_status("nothing to transform"),
                }
            }
        }
        AppCmd::TransformMeshStart => {
            let l = app.doc.active_layer();
            if l.lock {
                app.set_status("layer is locked");
            } else if l.is_vector() || l.records_strokes() || l.folder {
                app.set_status("Mesh transform applies to raster layers");
            } else {
                match transform_lift_rect(app) {
                    Some(r) if r[0] < r[2] && r[1] < r[3] => {
                        if open_layer_transform(app, app.doc.active, r) {
                            let drag = app.transform_drag.as_mut().unwrap();
                            drag.mesh = Some(crate::app::MeshLattice {
                                n: 5,
                                pts: mn_core::mesh::identity_lattice(drag.source.rect, 5),
                                pins: Vec::new(),
                                puppet: false,
                            });
                            app.set_status(
                                "mesh transform: drag the lattice points, drag between them to move all — Enter commits, Esc cancels",
                            );
                            app.mark_dirty();
                        } else {
                            app.set_status("nothing to transform");
                        }
                    }
                    _ => app.set_status("nothing to transform"),
                }
            }
        }
        AppCmd::TransformPuppetStart => {
            let l = app.doc.active_layer();
            if l.lock {
                app.set_status("layer is locked");
            } else if l.is_vector() || l.records_strokes() || l.folder {
                app.set_status("Puppet warp applies to raster layers");
            } else {
                match transform_lift_rect(app) {
                    Some(r) if r[0] < r[2] && r[1] < r[3] => {
                        if open_layer_transform(app, app.doc.active, r) {
                            let drag = app.transform_drag.as_mut().unwrap();
                            drag.mesh = Some(crate::app::MeshLattice {
                                n: 5,
                                pts: mn_core::mesh::identity_lattice(drag.source.rect, 5),
                                pins: Vec::new(),
                                puppet: true,
                            });
                            app.set_status(
                                "puppet warp: click to drop a pin, drag to pull, Alt+click a pin to remove — Enter commits, Esc cancels",
                            );
                            app.mark_dirty();
                        } else {
                            app.set_status("nothing to warp");
                        }
                    }
                    _ => app.set_status("nothing to warp"),
                }
            }
        }
        AppCmd::TransformCommit => {
            app.doc.set_op_label("Transform");
            if let Some(drag) = app.transform_drag.take() {
                // A mesh drag's "nothing moved" is the LATTICE's identity,
                // not the affine's (the affine holds identity throughout).
                let mesh_moved = drag.mesh.as_ref().is_some_and(|m| {
                    !mn_core::mesh::lattice_is_identity(drag.source.rect, m.n, &m.pts)
                });
                if drag.is_identity() && !mesh_moved && !drag.stamp_on_identity {
                    // Nothing moved — drop the float without an undo step.
                    app.set_status("transform canceled");
                } else {
                    commit_transform_drag(app, drag);
                }
                app.mark_dirty();
            }
        }
        AppCmd::TransformCancel => {
            if app.transform_drag.take().is_some() {
                app.set_status("transform canceled");
                app.mark_dirty();
            }
        }
        AppCmd::TransformUpdate {
            sx,
            sy,
            rad,
            tx,
            ty,
        } => {
            if let Some(drag) = &mut app.transform_drag {
                drag.set_params(sx, sy, rad, tx, ty);
                app.mark_dirty();
            }
        }
        AppCmd::TransformFlip { horizontal } => {
            // In an active transform: a flip BUTTON (T-021). Standalone
            // (TRIAGE 130): lift, mirror about the region centre, commit —
            // one undo step, selection-bounded like every whole-layer op.
            if let Some(drag) = &mut app.transform_drag {
                drag.flip(horizontal);
                app.set_status("flipped about the reference point");
                app.mark_dirty();
            } else {
                let l = app.doc.active_layer();
                if l.lock {
                    app.set_status("layer is locked");
                } else if l.is_vector() || l.folder {
                    app.set_status("Flip applies to raster and vector-ink layers");
                } else {
                    let rect = transform_lift_rect(app);
                    let valid = rect.is_some_and(|r| r[0] < r[2] && r[1] < r[3]);
                    match (rect, valid) {
                        (Some(r), true) if l.records_strokes() => {
                            let pivot = [(r[0] + r[2]) as f32 * 0.5, (r[1] + r[3]) as f32 * 0.5];
                            let (sx, sy) = if horizontal { (-1.0, 1.0) } else { (1.0, -1.0) };
                            let xform =
                                mn_core::Affine2::scale_rotate_around(pivot, sx, sy, 0.0, [0.0, 0.0]);
                            let li = app.doc.active;
                            let sel = app.doc.selection.clone();
                            let ok = transform_strokes(app, li, &xform, sel.as_ref(), "Flip");
                            app.set_status(match (ok, horizontal) {
                                (true, true) => "flipped horizontally — the strokes with it",
                                (true, false) => "flipped vertically — the strokes with it",
                                (false, _) => "nothing to flip",
                            });
                            app.mark_dirty();
                        }
                        (Some(r), true) => {
                            let src =
                                mn_core::transform::lift_region(l, r, app.doc.selection.as_ref());
                            if src.tiles.is_empty() {
                                app.set_status("nothing to flip");
                            } else {
                                let pivot =
                                    [(r[0] + r[2]) as f32 * 0.5, (r[1] + r[3]) as f32 * 0.5];
                                let xform = if horizontal {
                                    mn_core::Affine2::scale_rotate_around(
                                        pivot,
                                        -1.0,
                                        1.0,
                                        0.0,
                                        [0.0, 0.0],
                                    )
                                } else {
                                    mn_core::Affine2::scale_rotate_around(
                                        pivot,
                                        1.0,
                                        -1.0,
                                        0.0,
                                        [0.0, 0.0],
                                    )
                                };
                                app.doc.set_op_label("Flip");
                                // Lift and commit are one atomic action
                                // here, so the live selection IS the
                                // lift-time selection.
                                let sel = app.doc.selection.take();
                                let ok = mn_core::transform::commit_transform(
                                    &mut app.doc,
                                    &src,
                                    &xform,
                                    sel.as_ref(),
                                    true,
                                    false, // a lifted flip, not a paste
                                    None,
                                    // A flip is a ±1 scale: every sampled
                                    // position is integral and every kernel
                                    // degenerates to the same permutation
                                    // (`flip_is_an_exact_pixel_permutation`).
                                    // Pinned to Bilinear so the row cannot
                                    // make Edit ▸ Flip resample at all.
                                    mn_core::transform::Interp::Bilinear,
                                );
                                app.doc.selection = sel;
                                app.set_status(if ok {
                                    if horizontal {
                                        "flipped horizontally"
                                    } else {
                                        "flipped vertically"
                                    }
                                } else {
                                    "flip refused"
                                });
                                app.mark_dirty();
                            }
                        }
                        _ => app.set_status("nothing to flip"),
                    }
                }
            }
        }
        AppCmd::TransformSetPivot { pivot } => {
            if let Some(drag) = &mut app.transform_drag {
                match pivot {
                    Some(p) => drag.set_pivot(p),
                    // Reset to the source centre: deriving about the centre
                    // and clearing the override are the same transform.
                    None => {
                        let r = drag.source.rect;
                        let c = [(r[0] + r[2]) as f32 * 0.5, (r[1] + r[3]) as f32 * 0.5];
                        drag.set_pivot(c);
                        drag.pivot_override = None;
                    }
                }
                app.mark_dirty();
            }
        }


        // --- selection + fill -----------------------------------------------
        AppCmd::SetSelectMode(m) => {
            app.select_mode = m;
            // Leaving Magnetic mid-trace would leave an orphan outline on
            // the overlay with no gesture left to close it.
            app.magnetic = None;
            if m == SelectMode::Magnetic {
                app.set_status(
                    "magnetic lasso: trace along the lineart — Backspace undoes an anchor, Enter closes",
                );
            }
            app.mark_dirty();
        }
        AppCmd::Deselect => {
            if let Some(s) = app.doc.selection.take() {
                // Ctrl+Shift+D brings it back.
                app.last_selection = Some(s);
                app.doc.touch();
                app.mark_dirty();
            }
        }
        AppCmd::SelectAll => {
            app.doc.selection = Some(mn_core::Selection::all(&app.doc));
            app.doc.touch();
            app.set_status("all selected");
            app.mark_dirty();
        }
        AppCmd::SelectInvert => match app.doc.selection.take() {
            Some(s) => {
                let inv = s.inverted(&app.doc);
                app.doc.selection = (!inv.is_empty()).then_some(inv);
                app.doc.touch();
                app.set_status("selection inverted");
                app.mark_dirty();
            }
            None => app.set_status("nothing selected to invert"),
        },
        AppCmd::SelectBlur(px) => match app.doc.selection.take() {
            Some(s) => {
                let b = s.blur(&app.doc, px);
                // A blur wide enough to push EVERY pixel under half leaves
                // a live selection with no ants and no launcher — it still
                // masks the brush at partial strength, so say so rather
                // than let the canvas go quietly unpaintable.
                let hidden = !b.is_empty() && !b.has_visible_outline();
                app.doc.selection = Some(b);
                app.doc.touch();
                if hidden {
                    app.set_error(format!(
                        "blurred by {px} px: coverage is under 50% everywhere, so the marching ants are hidden — the selection still masks painting at partial strength (Ctrl+D clears it)"
                    ));
                } else {
                    app.set_status(format!("selection border blurred by {px} px"));
                }
                app.mark_dirty();
            }
            None => app.set_status("nothing selected to blur"),
        },
        AppCmd::SelectExpand(px) => match app.doc.selection.take() {
            Some(s) => {
                app.doc.selection = Some(s.grow(&app.doc, px));
                app.doc.touch();
                app.set_status(format!("selection expanded by {px} px"));
                app.mark_dirty();
            }
            None => app.set_status("nothing selected to expand"),
        },
        AppCmd::SelectShrink(px) => match app.doc.selection.take() {
            Some(s) => {
                let e = s.shrink(&app.doc, px);
                let gone = e.is_empty();
                app.doc.selection = (!gone).then_some(e);
                app.doc.touch();
                app.set_status(if gone {
                    "selection shrunk out of existence".to_string()
                } else {
                    format!("selection shrunk by {px} px")
                });
                app.mark_dirty();
            }
            None => app.set_status("nothing selected to shrink"),
        },
        AppCmd::Reselect => match app.last_selection.take() {
            Some(s) => {
                app.doc.selection = Some(s);
                app.doc.touch();
                app.set_status("reselected");
                app.mark_dirty();
            }
            None => app.set_status("no previous selection"),
        },
        AppCmd::FillSelection => {
            // NL-006's live switch (TRIAGE 137): with the Tool Property's
            // "live layer" on, Fill targets the live model — retargeting the
            // active live layer when there is one, else creating a new one.
            if app.fill_live {
                let color = app.active_color();
                let kind = mn_core::FillKind::Flat {
                    color: [color[0], color[1], color[2], 1.0],
                };
                let li = app.doc.active;
                if matches!(app.doc.layers[li].kind, mn_core::LayerKind::Fill(_)) {
                    app.push_cmd(AppCmd::SetFillParams(li, kind));
                } else {
                    app.push_cmd(AppCmd::NewLiveFill(kind));
                }
                return;
            }
            app.doc.set_op_label("Fill");
            let color = app.active_color();
            if app.doc.fill_selection(color) {
                app.set_status(if app.doc.selection.is_some() {
                    "selection filled"
                } else {
                    "layer filled"
                });
            } else {
                app.set_status("this layer cannot be filled (vector/folder/locked)");
            }
            app.mark_dirty();
        }
        AppCmd::SelectFromLayer(i, op) => {
            app.refresh_tones();
            let s = mn_core::Selection::from_layer_alpha(&app.doc, i);
            if s.is_empty() {
                app.set_status("that layer has no opaque pixels to select");
                app.mark_dirty();
                return;
            }
            let combined = match &app.doc.selection {
                Some(cur) if op != mn_core::SelectionOp::Replace => cur.combine(&s, &app.doc, op),
                _ => s,
            };
            if combined.is_empty() {
                app.doc.selection = None;
                app.set_status("selection combined away — deselected");
            } else {
                app.doc.selection = Some(combined);
                app.set_status("selected the layer's opacity");
            }
            app.doc.touch();
            app.mark_dirty();
        }
        AppCmd::ClearOutside => {
            app.doc.set_op_label("Clear outside");
            if app.doc.selection.is_none() {
                app.set_status("no selection — everything would go");
            } else if app.doc.clear_outside_selection() {
                app.set_status("cleared outside the selection");
            } else {
                app.set_status("this layer cannot be cleared (vector/folder/locked)");
            }
            app.mark_dirty();
        }
        AppCmd::MagicSelectPath { pts, op } => {
            app.refresh_tones();
            let seeds = subsample_path(&pts, 4.0);
            match mn_core::fill::magic_select_path(&app.doc, &seeds, &app.wand_opts) {
                Some((s, floods)) if !s.is_empty() => {
                    let combined = match &app.doc.selection {
                        Some(cur) if op != mn_core::SelectionOp::Replace => {
                            cur.combine(&s, &app.doc, op)
                        }
                        _ => s,
                    };
                    if combined.is_empty() {
                        app.doc.selection = None;
                        app.doc.touch();
                        app.set_status("selection subtracted away — deselected");
                    } else {
                        app.doc.selection = Some(combined);
                        app.doc.touch();
                        app.set_status(format!(
                            "{floods} closed areas selected — G fills them, Delete clears"
                        ));
                    }
                }
                _ => app.set_status("drag across the empty space inside the drawing"),
            }
            app.mark_dirty();
        }
        AppCmd::MagicSelect(x, y, op) => {
            app.refresh_tones();
            match mn_core::fill::magic_select(&app.doc, (x as i32, y as i32), &app.wand_opts) {
                Some(s) if !s.is_empty() => {
                    let combined = match &app.doc.selection {
                        Some(cur) if op != mn_core::SelectionOp::Replace => {
                            cur.combine(&s, &app.doc, op)
                        }
                        _ => s,
                    };
                    if combined.is_empty() {
                        // Subtracted away to nothing: an empty Selection
                        // means "everything", so deselect instead.
                        app.doc.selection = None;
                        app.doc.touch();
                        app.set_status("selection subtracted away — deselected");
                    } else {
                        app.doc.selection = Some(combined);
                        app.doc.touch();
                        app.set_status("area selected — G fills it, Delete clears it");
                    }
                }
                _ => app.set_status("nothing to select there"),
            }
            app.mark_dirty();
        }
        AppCmd::PickColor(x, y) => {
            app.refresh_tones();
            let (xi, yi) = (x as i32, y as i32);
            let opts = app.eyedrop_opts;
            match pick_color(&app.doc, xi, yi, opts) {
                Some([r, g, b]) => {
                    let rgb = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0];
                    dispatch(app, AppCmd::SetSlotColor(rgb));
                    let tail = register_picked(app, rgb);
                    // Name the mode when it is not the plain one: a 5×5
                    // average that silently disagrees with the pixel under
                    // the pen is the kind of thing you blame on the tool.
                    let mut note = String::new();
                    if opts.size > 1 {
                        note += &format!(" ({0}×{0} average)", opts.size);
                    }
                    if opts.refer == mn_core::FillRefer::Reference
                        && app.doc.reference_layers().is_empty()
                    {
                        note += " (no reference layer marked — took what you see)";
                    }
                    app.set_status(format!("picked #{r:02x}{g:02x}{b:02x}{note}{tail}"));
                }
                None => app.set_status("outside the canvas"),
            }
        }
        AppCmd::Fill(x, y) => {
            // A new fill supersedes any armed repair — the click queued
            // before this dispatch already released the gesture.
            app.fill_repair = None;
            app.refresh_tones();
            let color = app.active_color();
            let opts = app.fill_opts;
            let (n, auto) = mn_core::fill::bucket_fill_measured(
                &mut app.doc,
                (x as i32, y as i32),
                color,
                &opts,
            );
            if opts.auto {
                app.fill_auto = auto;
            }
            if n > 0 {
                // Leak repair (app/fill_repair.rs): remember the fill at
                // its commit point — seed, settings, layer, page. Only
                // the CLICK family lands here; lasso/enclose gestures
                // have no seed and must not overwrite a repairable one.
                app.last_fill = Some(crate::app::fill_repair::LeakFill {
                    layer_id: app.doc.layers[app.doc.active].id(),
                    page_uid: app.pages.get(app.page_index).map(|p| p.uid).unwrap_or(0),
                    seed: (x, y),
                    color,
                    opts,
                    op_label: "Fill",
                    // The label alone is not identity — Alt+Del also
                    // pushes "Fill". Depth + label together are (the
                    // stack is uncapped).
                    undo_len: app.doc.undo_len(),
                });
                app.set_status(format!("filled {n} px{}", auto_note(&opts, auto)));
            } else {
                // Every other fill door says so when it writes nothing —
                // enclose, leftover, lasso and the Tone tool all do. The
                // plain click used to go silent, and a bucket that does
                // nothing without a word reads as a broken tool. The two
                // ways to get here on a real page are clicking ON the line
                // instead of beside it, and re-clicking an area that is
                // already this colour.
                app.set_status(if !app.doc.active_layer().paintable() {
                    "this layer takes no paint — pick a raster layer to fill"
                } else if app.doc.selection.is_some() {
                    "nothing filled — the selection clipped the whole area away"
                } else {
                    "nothing filled — click inside the page"
                });
            }
            app.mark_dirty();
        }
        AppCmd::ArmFillRepair { virtual_barrier } => {
            app.arm_fill_repair(virtual_barrier);
        }
        AppCmd::SetFillOpts(o) => {
            app.fill_opts = o;
        }
        AppCmd::SetWandOpts(o) => {
            app.wand_opts = o;
        }
        AppCmd::StampVisible => {
            app.commit_text_edit();
            app.refresh_tones();
            let img = mn_core::export::composite(&app.doc, mn_core::Background::Transparent);
            let n = app.doc.layers.len() + 1;
            app.doc.add_layer_from_image(format!("Merged {n}"), &img);
            app.renderer.invalidate();
            app.set_status("visible layers stamped onto a new layer");
            app.mark_dirty();
        }
        AppCmd::LayerAbove => {
            let i = app.doc.active + 1;
            if app.doc.set_active(i) {
                app.mark_dirty();
            }
        }
        AppCmd::LayerBelow => {
            let i = app.doc.active.wrapping_sub(1);
            if app.doc.set_active(i) {
                app.mark_dirty();
            }
        }
        AppCmd::ImportImage | AppCmd::ImportImageDraft => {}
        AppCmd::ImportImagePath(p) => import_image_layer(app, &p, false),
        AppCmd::ImportImageDraftPath(p) => import_image_layer(app, &p, true),

        other => return brush::run(app, other, cmd_tail),
    }
    run_cmd_tail(app, cmd_tail);
}
