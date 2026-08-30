//! `AppCmd` arms: undo/redo history, layer masks, filters and
//! tonal correction, layer comps, and the line generator.

use super::*;

/// Every layer mask's identity, for the undo/redo invalidation compare.
/// `Document::apply_group` stamps a fresh revision on any mask it restores,
/// so a moved coverage field always shows up here (and a mask that appeared
/// or vanished changes the shape of the vector).
fn mask_sig(app: &App) -> Vec<Option<(u64, bool)>> {
    app.doc
        .layers
        .iter()
        .map(|l| l.mask.as_ref().map(|m| (m.revision, m.enabled)))
        .collect()
}

/// Undo/redo can restore the whole ruler set (`UndoGroup::Rulers`). The
/// rulers ARE the geometry — the overlay and the snap read them straight,
/// so nothing is cached off them — with one exception: the symmetric
/// ruler's mirror twins hold its centre and axes, and they must be rebuilt
/// or the next stroke mirrors about the place the ruler used to be. The
/// in-flight gesture state goes too: a sticky snap lock and a live grab are
/// both INDICES into the set that was just replaced (the session.rs
/// pattern, where a tab switch drops them for the same reason).
fn resync_rulers(app: &mut App, before: &mn_core::Rulers) {
    if app.doc.rulers == *before {
        return;
    }
    app.ruler_lock = Default::default();
    app.ruler_move = None;
    app.rebuild_twins();
    app.mark_dirty();
}

/// The generator's NEW-LAYER half: render `spec`, add the layer that
/// CARRIES it, ink it, and wrap both records into ONE undo press. Returns
/// the layer's name, or `None` when the spec rendered nothing (the caller
/// says so — nothing is added).
///
/// Shared by the dialog's Generate and by the Materials bank's generator
/// materials, which is the point: a `.gen.json` material places the same
/// live, Object-tool-editable layer, never a decoded bitmap.
pub(super) fn genlines_new_layer(
    app: &mut App,
    spec: mn_core::genlines::GenLinesSpec,
    nest: Option<usize>,
) -> Option<&'static str> {
    let tiles = spec.render(app.doc.size);
    if tiles.is_empty() {
        return None;
    }
    let name = spec.layer_name();
    // Destination, two rules. (1) A gesture that STARTS inside a panel
    // places the layer INSIDE that panel's folder — the coverage mask
    // eats the protrusions past the border, which is the whole look of
    // printed 集中線 (owner, 2026-08-24: "they all make it to the panel
    // border, though the protrusions are not seen"). Deliberate, keyed
    // on the gesture — not on whichever layer happened to be selected.
    // (2) Everything else stays a page-level sheet, never SEALED inside
    // a frame folder by accident: `add_layer` inserts above the active
    // layer at its depth, and a frame folder leaves its draw layer
    // active — a burst drawn outside the panel window used to vanish
    // completely, a layer in the palette and nothing on the page (owner
    // repro 2026-08-22, Figure ▸ Saturated line). The loop climbs out of
    // nested frame folders; `add_layer_above` still hops clip runs.
    // (A folder with no children left cannot be nested into — inserting
    // "above the last child" would land OUTSIDE it; fall back to the
    // sheet rather than guess.)
    let anchor = match nest.filter(|&f| {
        app.doc
            .layers
            .get(f)
            .is_some_and(|l| l.folder && l.is_frame())
            && app.doc.block_range(f).len() > 1
    }) {
        // The block is [children…, header] — the header is the LAST
        // entry. Anchoring on `end - 1` (the header) inserts BELOW the
        // folder at its own depth; `end - 2` is the last child, and
        // inserting above it lands INSIDE, over the panel's art.
        Some(f) => app.doc.block_range(f).end - 2,
        None => {
            let mut anchor = app.doc.active;
            while let Some(f) = app.doc.enclosing_frame_folder(anchor) {
                anchor = f;
            }
            anchor
        }
    };
    app.doc.add_layer_above(anchor, name);
    app.doc.layers[app.doc.active].genlines = Some(spec);
    app.doc.begin_op();
    app.doc.set_op_label("Generate lines");
    let active = app.doc.active;
    for (idx, tile) in tiles {
        app.doc.layers[active].set_tile(idx, Some(tile));
    }
    app.doc.end_op();
    // One gesture, one press: the structural New-layer record and
    // the pixel op wrap together (structural ops record instead of
    // clearing since 2026-08-21).
    app.doc.wrap_recent("Generate lines", 2);
    app.mark_dirty();
    Some(name)
}

/// Where a focus-line generator material converges: the pointer when it is
/// over the canvas (the same paste-to-position gesture an image material
/// gets), the document centre otherwise — a degenerate view (headless, or
/// a shell that has not laid out) must not aim off-canvas.
pub(super) fn genlines_aim_point(app: &App) -> (f32, f32) {
    let p = app.last_pointer;
    if !app.shell.owns_pointer(p.0, p.1) {
        let c = app.viewport.to_canvas(p.0 as f32, p.1 as f32);
        if c.0 >= 0.0 && c.1 >= 0.0 && c.0 < app.doc.size.0 as f32 && c.1 < app.doc.size.1 as f32 {
            return c;
        }
    }
    (app.doc.size.0 as f32 * 0.5, app.doc.size.1 as f32 * 0.5)
}

pub(super) fn run(app: &mut App, cmd: AppCmd, cmd_tail: CmdTail) {
    match cmd {
        // --- history ------------------------------------------------------
        // No `renderer.invalidate()`: undo stamps a fresh revision on every
        // tile it restores, and the tile cache evicts on revision.
        AppCmd::HistoryWrapLast { label, count } => {
            // No-op when the stack is shorter than promised (a count
            // mismatch would be a bug elsewhere; undo never breaks).
            app.doc.wrap_recent(label.as_str(), count);
            app.mark_dirty();
        }
        AppCmd::Undo => {
            app.commit_text_edit();
            // Ctrl+Z with a floating paste/material still live: the float
            // is not in history yet, so "undo" means take the placement
            // back — NOT silently rewind something older underneath it
            // (owner 2026-08-21: dragged a material in, Ctrl+Z "did
            // nothing").
            if app.transform_drag.take().is_some() {
                app.set_status("placement taken back");
                app.mark_dirty();
                return;
            }
            // A tone-param undo can flip a layer back to non-tone: the GPU
            // tile cache then holds derived rasters newer than the source
            // tiles, which the revision compare would keep. Evict on change.
            let tones_before: Vec<_> = app.doc.layers.iter().map(|l| l.tone).collect();
            let masks_before = mask_sig(app);
            let rulers_before = app.doc.rulers.clone();
            // A Structure swap (recordable-action run) restores tiles at
            // their OLD revisions — the cache uploads only on newer, so the
            // swap needs the full rebuild plus the index-keyed resets a
            // layer-stack change implies.
            let structural = app.doc.next_undo_is_structure();
            if app.doc.undo() {
                resync_rulers(app, &rulers_before);
                // Vector selection indexes into a set undo just reshaped.
                app.vector_sel = None;
                app.vector_drag = None;
                for (li, (l, was)) in app.doc.layers.iter().zip(&tones_before).enumerate() {
                    if l.tone != *was {
                        app.renderer.evict_layer(li);
                    }
                }
                // LM-004: the GPU tile cache keys on the LAYER tile revision
                // and folds the mask into the upload, so a mask that moved
                // over unchanged pixels needs the full rebuild — the same
                // door every other mask edit goes through.
                if structural || mask_sig(app) != masks_before {
                    app.renderer.invalidate();
                }
                if structural {
                    app.layer_thumbs.clear();
                }
                // Undo can remove the active layer's mask (e.g. undo of its
                // creation) — audit H1: armed mask-edit must not survive it.
                app.disarm_mask_edit_if_unmasked();
                app.mark_dirty();
            }
        }
        AppCmd::Redo => {
            app.commit_text_edit();
            let tones_before: Vec<_> = app.doc.layers.iter().map(|l| l.tone).collect();
            let masks_before = mask_sig(app);
            let rulers_before = app.doc.rulers.clone();
            let structural = app.doc.next_redo_is_structure();
            if app.doc.redo() {
                resync_rulers(app, &rulers_before);
                app.vector_sel = None;
                app.vector_drag = None;
                for (li, (l, was)) in app.doc.layers.iter().zip(&tones_before).enumerate() {
                    if l.tone != *was {
                        app.renderer.evict_layer(li);
                    }
                }
                if structural || mask_sig(app) != masks_before {
                    app.renderer.invalidate();
                }
                if structural {
                    app.layer_thumbs.clear();
                }
                app.disarm_mask_edit_if_unmasked();
                app.mark_dirty();
            }
        }

        AppCmd::MaskSelection => {
            let li = app.doc.active;
            if app.doc.mask_selection_blank(li) {
                app.renderer.invalidate();
                app.set_status("mask created — all visible (LM-001 starter)");
                app.mark_dirty();
            } else {
                app.set_status("mask applies to raster layers with content");
            }
        }
        AppCmd::MaskOutsideSelection => {
            let li = app.doc.active;
            if app.doc.selection.is_none() {
                app.set_status("no selection — the whole layer would be hidden; refusing");
                return;
            }
            if app.doc.mask_outside_selection(li) {
                app.renderer.invalidate();
                app.set_status("mask outside selection — the rest is hidden");
                app.mark_dirty();
            } else {
                app.set_status("mask applies to raster layers with content");
            }
        }
        AppCmd::MaskLinkToggle => {
            let li = app.doc.active;
            let flipped = matches!(app.doc.layers.get(li), Some(l) if l.mask.is_some()) && {
                app.doc.layers[li].mask_linked = !app.doc.layers[li].mask_linked;
                true
            };
            if flipped {
                // Persisted state (`mnc-mask-unlinked`): the touch is what
                // gets it saved when the toggle is the session's last act.
                app.doc.touch();
                app.set_status(if app.doc.layers[li].mask_linked {
                    "mask linked — moves with the layer"
                } else {
                    "mask unlinked — art slides underneath a fixed mask"
                });
                app.mark_dirty();
            } else {
                app.set_status("that layer has no mask to link");
            }
        }
        AppCmd::MaskToggle => {
            let li = app.doc.active;
            let on = app
                .doc
                .layers
                .get(li)
                .and_then(|l| l.mask.as_ref())
                .is_some_and(|m| !m.enabled);
            if app.doc.mask_set_enabled(li, on) {
                app.renderer.invalidate();
                app.set_status(if on { "mask ON" } else { "mask OFF (kept)" });
                app.mark_dirty();
            }
        }
        AppCmd::MaskDelete => {
            let li = app.doc.active;
            if app.doc.mask_delete(li) {
                app.disarm_mask_edit_if_unmasked();
                app.renderer.invalidate();
                app.set_status("mask deleted");
                app.mark_dirty();
            }
        }
        AppCmd::MaskClear => {
            let li = app.doc.active;
            if app.doc.mask_clear(li) {
                app.renderer.invalidate();
                app.set_status("mask cleared — all hidden (the mask itself kept)");
                app.mark_dirty();
            }
        }
        AppCmd::MaskApply => {
            let li = app.doc.active;
            app.doc.set_op_label("Apply mask");
            if app.doc.mask_apply_bake(li) {
                // The bake ends by deleting the mask (audit H1: disarm).
                app.disarm_mask_edit_if_unmasked();
                app.renderer.invalidate();
                app.set_status("mask baked into the layer — the mask is gone");
                app.mark_dirty();
            } else {
                app.set_status("no enabled mask to apply");
            }
        }
        AppCmd::MaskEdit => {
            // Mask strokes land in the MASK, which the stroke replay never
            // touches — so a recording layer edits its mask like any other
            // raster layer, and this asks for that directly rather than
            // through `paintable()` (which now refuses one).
            let l = app.doc.active_layer();
            let ok = l.mask.is_some() && !l.folder && !l.is_vector();
            if !app.mask_edit && !ok {
                app.set_status("edit-mask needs a masked raster layer");
                return;
            }
            app.set_mask_edit(!app.mask_edit);
            app.set_status(if app.mask_edit {
                "editing the MASK — draw any colour to reveal, erase to hide"
            } else {
                "editing the LAYER again"
            });
            app.mark_dirty();
        }
        AppCmd::MaskShowArea => {
            app.mask_show_area = !app.mask_show_area;
            app.set_status(if app.mask_show_area {
                "mask area shown (purple tint over the hidden region)"
            } else {
                "mask area hidden"
            });
            app.mark_dirty();
        }
        AppCmd::FilterOpen(f) => {
            app.filter_draft = f;
        }
        AppCmd::FilterApply(f) => {
            app.filter_draft = None;
            // The blur family goes through the shared GPU kernel seam when
            // the adapter and the size floor allow; `apply_filter_with`
            // falls back to `Filter::run` — the reference — the moment the
            // closure returns false, including on a dispatch canary failure
            // mid-job. Everything else (the warps, dust, morphology) has no
            // kernel here yet and takes the false arm every time.
            let crate::app::App { doc, renderer, .. } = &mut *app;
            let ran = doc.apply_filter_with(f, &mut |f, buf| {
                let Some(passes) = f.separable_passes() else {
                    return false;
                };
                renderer.kernels_preferred(buf.w * buf.h)
                    && renderer.run_region_kernel(
                        mn_gpu::Kernel::Separable(&passes),
                        &mut buf.px,
                        buf.w,
                        buf.h,
                    )
            });
            if ran {
                app.set_status(format!("{} applied", f.label()));
                app.mark_dirty();
            } else {
                // Every refusal reason at once: the layer will not take
                // pixels, or there are none, or the marquee misses it, or the
                // parameters are a no-op. Nothing was pushed onto undo.
                app.set_status(format!(
                    "{} did nothing — needs an unlocked raster layer with pixels inside the selection",
                    f.label()
                ));
            }
        }
        AppCmd::BrightnessToOpacity => {
            let li = app.doc.active;
            app.doc.set_op_label("Brightness → opacity");
            if app.doc.convert_brightness_to_opacity(li) {
                app.set_status("brightness converted to opacity — white is now transparent");
                app.mark_dirty();
            } else {
                app.set_status("nothing to convert (raster layer with content)");
            }
        }
        AppCmd::AdjustOpen(a) => app.adjust_begin(a),
        AppCmd::AdjustApply => app.adjust_commit(),
        AppCmd::AdjustCancel => {
            app.adjust_preview_revert();
        }
        AppCmd::AdjustNow(a) => {
            app.adjust_draft = Some(a);
            app.adjust_commit();
        }
        AppCmd::CompApply(i) => {
            if app.comp_apply(i) {
                let n = app
                    .doc
                    .comps
                    .get(i)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                app.set_status(format!("layer comp applied: {n}"));
            }
        }
        AppCmd::CompSave(i) => {
            // Named, because this is destructive to a snapshot: say WHICH
            // one was replaced (the old status claimed success even when
            // nothing was selected and nothing happened).
            let name = app.doc.comps.get(i).map(|c| c.name.clone());
            if app.comp_save(i)
                && let Some(n) = name
            {
                app.set_status(format!(
                    "layer comp \"{n}\" overwritten with the layers' current state"
                ));
            } else {
                app.set_status("no comp at that row to overwrite");
            }
        }
        AppCmd::GenLines => {
            app.gen_open = true;
        }
        AppCmd::GenLinesEdit => {
            let Some(g) = app.doc.active_layer().genlines else {
                app.set_status("the active layer was not generated — Layer menu > Generate effect lines for a new one");
                return;
            };
            app.gen_focus = g.focus;
            app.gen_a = g.a;
            app.gen_b = g.b;
            app.gen_c = g.c;
            app.gen_d = g.d;
            app.gen_count = g.count;
            app.gen_width = g.width;
            app.gen_jitter = g.jitter;
            app.gen_seed = g.seed;
            // The loaded values ARE the layer's parameters — the dialog's
            // first-open seeding must not replace them (a (0,0) centre is
            // legal, not "uninitialized").
            app.gen_inited = true;
            app.gen_open = true;
            app.set_status("editing the layer's own parameters — Apply regenerates in place");
        }
        AppCmd::GenLinesApply {
            focus,
            a,
            b,
            c,
            d,
            count,
            width,
            jitter,
            seed,
        } => {
            app.gen_open = false;
            // The dialog only knows the original nine parameters, so it
            // must CARRY everything else the layer holds rather than
            // rebuild it: applying it to a flash layer would otherwise
            // silently turn the spikes back into plain focus lines, with
            // the raster following on the same press. Carrying the WHOLE
            // spec (`..carry`) means the density round's attributes are
            // covered by that rule automatically, instead of the dialog
            // resetting a gap or a colour it has never heard of.
            let carry = app.doc.active_layer().genlines.unwrap_or_default();
            let spec = mn_core::genlines::GenLinesSpec {
                focus,
                a,
                b,
                c,
                d,
                count,
                width,
                jitter,
                seed,
                // The dialog counts lines, so a gap-driven layer edited
                // there becomes count-driven — otherwise its Lines field
                // would be a no-op that silently did nothing.
                gap_deg: 0.0,
                ..carry
            };
            // SF-004/005: re-applying on the layer the params came from
            // regenerates IN PLACE (the layer keeps name/stack/blend);
            // everything else generates a fresh layer as before.
            if app.doc.active_layer().genlines.is_some() {
                let li = app.doc.active;
                // Spec-on-success (audit F): the regen stores the new spec
                // only when it rendered something, and a failed one leaves
                // both halves alone — the stored parameters must always
                // describe the pixels on screen.
                if app.doc.regen_genlines(li, spec) {
                    app.set_status("effect lines regenerated");
                    app.mark_dirty();
                } else {
                    app.set_status("generator produced nothing — widen the parameters");
                }
                return;
            }
            match genlines_new_layer(app, spec, None) {
                Some(name) => app.set_status(format!("{name} generated — {count} lines")),
                None => app.set_status("generator produced nothing — widen the parameters"),
            }
        }
        AppCmd::GenLinesPlace(spec, nest) => match genlines_new_layer(app, spec, nest) {
            Some(name) => app.set_status(format!(
                "{name} placed — the Object tool's handles (or Layer ▸ effect lines) adjust it"
            )),
            None => app.set_status("no lines landed on the canvas — drag further out"),
        },
        AppCmd::GenLinesApplyTo { layer, spec } => {
            // Spec-on-success, exactly like the dialog's Apply: a regen
            // that renders nothing leaves BOTH halves alone, so the
            // stored parameters always describe the pixels on screen.
            if app.doc.regen_genlines(layer, spec) {
                app.set_status("effect lines regenerated");
                app.mark_dirty();
            } else {
                app.set_status("generator produced nothing — widen the parameters");
            }
        }
        AppCmd::HistoryTo { keep } => {
            app.commit_text_edit();
            let tones_before: Vec<_> = app.doc.layers.iter().map(|l| l.tone).collect();
            let masks_before = mask_sig(app);
            app.vector_sel = None;
            app.vector_drag = None;
            let mut steps = 0usize;
            while app.doc.undo_len() > keep && app.doc.undo() {
                steps += 1;
            }
            while app.doc.undo_len() < keep && app.doc.redo() {
                steps += 1;
            }
            if steps > 0 {
                for (li, (l, was)) in app.doc.layers.iter().zip(&tones_before).enumerate() {
                    if l.tone != *was {
                        app.renderer.evict_layer(li);
                    }
                }
                // Scrubbing the History palette crosses the same two doors a
                // single Undo does: a restored mask needs the upload rebuild,
                // and a mask that scrubbed away must not leave mask-edit armed.
                if mask_sig(app) != masks_before {
                    app.renderer.invalidate();
                }
                app.disarm_mask_edit_if_unmasked();
                app.mark_dirty();
            }
        }
        AppCmd::ClearHistory => {
            app.doc.clear_history();
            app.set_status("undo history cleared");
        }
        AppCmd::RevertFile => match app.doc_path.clone() {
            Some(p) if p.exists() => {
                app.push_cmd(AppCmd::OpenOraPath(p));
                app.set_status("reverted to the last save");
            }
            _ => app.set_status("nothing saved to revert to"),
        },

        other => return pages::run(app, other, cmd_tail),
    }
    run_cmd_tail(app, cmd_tail);
}
