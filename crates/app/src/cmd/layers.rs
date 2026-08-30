//! `AppCmd` arms: the layer stack (add/remove/merge/folders),
//! per-layer properties, layer conversion + alignment, and rulers.

use super::*;
use super::frames::enclosing_folder;

/// The RF-001 unit a reference click addresses: the layer itself, or —
/// when it is a folder — the folder plus its child run (every following
/// layer with a deeper depth, up to the first sibling that pops back).
fn reference_unit(doc: &mn_core::Document, i: usize) -> Vec<usize> {
    let Some(l) = doc.layers.get(i) else {
        return Vec::new();
    };
    if !l.folder {
        return vec![i];
    }
    let d = l.depth;
    let mut out = vec![i];
    for (j, m) in doc.layers.iter().enumerate().skip(i + 1) {
        if m.depth > d {
            out.push(j);
        } else {
            break;
        }
    }
    out
}

pub(super) fn run(app: &mut App, cmd: AppCmd, cmd_tail: CmdTail) {
    match cmd {
        AppCmd::HideDraftLayers => {
            if let Some(saved) = app.draft_visibility.take() {
                let mut n = 0usize;
                for li in saved {
                    if app
                        .doc
                        .layers
                        .get(li)
                        .is_some_and(|l| l.draft && !l.visible)
                    {
                        app.doc.set_layer_visible(li, true);
                        n += 1;
                    }
                }
                app.set_status(format!(
                    "restored {n} draft layer{}",
                    if n == 1 { "" } else { "s" }
                ));
            } else {
                let hidden: Vec<usize> = (0..app.doc.layers.len())
                    .filter(|&li| {
                        app.doc.layers[li].draft && app.doc.layers[li].visible
                    })
                    .collect();
                for li in &hidden {
                    app.doc.set_layer_visible(*li, false);
                }
                let n = hidden.len();
                app.draft_visibility = Some(hidden);
                app.set_status(format!(
                    "hid {n} draft layer{} — the command again restores them",
                    if n == 1 { "" } else { "s" }
                ));
            }
            app.mark_dirty();
        }
        AppCmd::ConvertToDrawingColor => {
            let c = app.main_color;
            let status = app.doc.convert_to_drawing_colour(c);
            app.set_status(status);
            app.mark_dirty();
        }
        AppCmd::ConvertOpen => {
            app.convert_name = app.doc.active_layer().name.clone();
            app.convert_expr = None;
            app.convert_open = !app.convert_open;
            app.mark_dirty();
        }
        AppCmd::FrameFolderRasterize => {
            // Target: the active layer when it IS a frame folder, else the
            // frame folder owning it (the same walk FrameFoldersCombine
            // uses — pasting into a panel targets the folder, so does
            // this).
            let target = if app.doc.active_layer().folder && app.doc.active_layer().is_frame() {
                Some(app.doc.active)
            } else {
                let mut f = enclosing_folder(&app.doc, app.doc.active);
                while let Some(i) = f
                    && !(app.doc.layers[i].folder && app.doc.layers[i].is_frame())
                {
                    f = enclosing_folder(&app.doc, i);
                }
                f
            };
            let Some(li) = target else {
                app.set_status("no frame folder to rasterize");
                return;
            };
            if app.doc.rasterize_frame_folder(li) {
                app.set_status(
                    "frame rasterized — the border is ink, the panel is a mask, \
                     the layers stayed separate (one undo)",
                );
                app.mark_dirty();
            } else {
                app.set_status("that layer is not a frame folder");
            }
        }
        AppCmd::ConvertLayer {
            rasterize,
            expression,
            blend,
            keep_original,
            name,
        } => {
            let li = app.doc.active;
            let refused = app.doc.layers.get(li).is_some_and(|l| l.folder);
            if refused {
                app.set_status("convert layer: pick a layer, not a folder");
            } else if app.doc.convert_layer(li, rasterize, expression, blend, keep_original, name) {
                app.set_status("layer converted — one undo");
                app.renderer.invalidate();
                app.mark_dirty();
            } else {
                app.set_status("nothing to convert");
            }
        }
        AppCmd::AdvancedFillOpen => {
            app.advfill_open = !app.advfill_open;
            app.mark_dirty();
        }
        AppCmd::AdvancedFill { opacity } => {
            app.doc.set_op_label("Advanced fill");
            let color = app.active_color();
            if app.doc.fill_selection_opacity(color, opacity) {
                app.set_status(if app.doc.selection.is_some() {
                    "selection filled"
                } else {
                    "layer filled"
                });
                app.mark_dirty();
            } else {
                app.set_status("this layer cannot be filled (vector/folder/locked)");
            }
        }
        AppCmd::ExtractOpen => {
            app.extract_open = !app.extract_open;
            app.mark_dirty();
        }
        AppCmd::ExtractLines { detection } => {
            let li = app.doc.active;
            match app.doc.extract_lines(li, detection) {
                Some(_) => {
                    app.set_status("lines extracted to a new layer — one undo");
                    app.renderer.invalidate();
                    app.mark_dirty();
                }
                None => app.set_status("nothing to extract — the layer has no ink"),
            }
        }
        AppCmd::LinesTonesOpen => {
            app.lt_open = !app.lt_open;
            app.mark_dirty();
        }
        AppCmd::ConvertLinesTones { params } => {
            let li = app.doc.active;
            if app.doc.layers.get(li).is_some_and(|l| l.folder) {
                app.set_status("convert to lines and tones: pick a layer, not a folder");
                return;
            }
            let dpi = app.tone_dpi();
            match app.doc.convert_to_lines_and_tones(li, &params, dpi) {
                Some(_) => {
                    app.set_status(if params.keep_original {
                        "converted to lines and tones — the materials are in a folder, \
                         the source is hidden (one undo)"
                    } else {
                        "converted to lines and tones — the materials are in a folder, \
                         the source is removed (one undo)"
                    });
                    app.renderer.invalidate();
                    app.mark_dirty();
                }
                None => app.set_status("nothing to convert — the layer has no ink"),
            }
        }
        AppCmd::OutlineOpen => {
            app.outline_open = !app.outline_open;
            app.mark_dirty();
        }
        AppCmd::OutlineSelection { width, border, round } => {
            let c = app.main_color;
            let status = app.doc.outline_selection(width, border, round, c);
            app.set_status(status);
            app.mark_dirty();
        }
        AppCmd::AlignOpen => {
            app.align_open = !app.align_open;
            app.mark_dirty();
        }
        AppCmd::AlignLayers { mode, base } => {
            // TR-052: one text layer selected → align its ITEMS against
            // each other; anything else aligns layers.
            let single_text = app.doc.multi_targets().len() == 1
                && app
                    .doc
                    .layers
                    .get(app.doc.multi_targets()[0])
                    .and_then(|l| l.texts())
                    .is_some_and(|ts| ts.texts.len() >= 2);
            let status = if single_text {
                app.doc.align_text_items(app.doc.active, mode)
            } else {
                app.doc.align_layers(mode, base)
            };
            app.set_status(status);
            app.mark_dirty();
        }
        AppCmd::DistributeLayers { mode } => {
            let single_text = app.doc.multi_targets().len() == 1
                && app
                    .doc
                    .layers
                    .get(app.doc.multi_targets()[0])
                    .and_then(|l| l.texts())
                    .is_some_and(|ts| ts.texts.len() >= 3);
            let status = if single_text {
                app.doc.distribute_text_items(app.doc.active, mode)
            } else {
                app.doc.distribute_layers(mode)
            };
            app.set_status(status);
            app.mark_dirty();
        }
        AppCmd::SpaceLayers { mode } => {
            let single_text = app.doc.multi_targets().len() == 1
                && app
                    .doc
                    .layers
                    .get(app.doc.multi_targets()[0])
                    .and_then(|l| l.texts())
                    .is_some_and(|ts| ts.texts.len() >= 3);
            let status = if single_text {
                app.doc.space_text_items(app.doc.active, mode)
            } else {
                app.doc.space_layers(mode)
            };
            app.set_status(status);
            app.mark_dirty();
        }
        AppCmd::OpenPrefs(section) => {
            app.prefs_open = true;
            app.prefs_focus = section;
            app.mark_dirty();
        }
        AppCmd::PenPressureWizardOpen => {
            app.pen_wizard_open = true;
            // Open on the correction that is IN FORCE, not on ×1.00: the
            // only authorable curve is y = x^γ, so reading the stored
            // curve at x = 0.5 recovers its γ exactly (ln y / ln 0.5).
            // Opening blind invited an Apply that silently wiped the
            // stored correction (audit verdict 2, 2026-08-25).
            let y = mn_core::stroke::eval_pressure_curve(&app.global_pressure, 0.5);
            app.pen_wizard_gamma = if app.global_pressure.is_empty() {
                1.0
            } else {
                (-y.clamp(1e-4, 1.0 - 1e-4).ln() / std::f32::consts::LN_2).clamp(0.25, 4.0)
            };
            app.pen_wizard_samples.clear();
            app.set_status("draw a few strokes, then Stronger/Weaker until the line feels right");
        }
        AppCmd::PenPressureCurveSet(pts) => {
            app.global_pressure = pts.clone();
            app.prefs.pressure_curve = if pts.is_empty() {
                String::new()
            } else {
                crate::app::prefs::pressure_curve_string(&pts)
            };
            app.prefs.mark_dirty();
            app.pen_wizard_open = false;
            app.set_status(if pts.is_empty() {
                "pen pressure correction off — raw tablet pressure"
            } else {
                "pen pressure correction applied to every tool"
            });
        }

        // --- layers -------------------------------------------------------
        // Structural ops (add/remove/reorder) shift layer indices, which the
        // tile cache keys on and `UndoGroup` records — hence invalidate() here,
        // and hence `Document` clearing the history itself.
        AppCmd::AddLayer => {
            app.commit_text_edit();
            let n = app.doc.layers.len() + 1;
            let name = format!("Layer {n}");
            // CSP: a new layer lands *inside* the active folder, else above
            // the active layer as its sibling.
            let active = app.doc.active;
            let made = if app
                .doc
                .layers
                .get(active)
                .is_some_and(|l| l.folder && l.open)
            {
                app.doc.add_layer_in_folder(active, name)
            } else {
                Some(app.doc.add_layer(name))
            };
            // LP-001: the type's saved default, inside the add's own undo
            // step (see `App::apply_layer_defaults`).
            if let Some(li) = made {
                app.apply_layer_defaults(li);
            }
            app.renderer.invalidate();
            app.mark_dirty();
        }
        AppCmd::AddVectorLayer => {
            app.commit_text_edit();
            let n = app
                .doc
                .layers
                .iter()
                .filter(|l| l.strokes.is_some())
                .count()
                + 1;
            let li = app.doc.add_layer(format!("Vector {n}"));
            app.doc.layers[li].strokes = Some(mn_core::StrokeSet::default());
            // After the strokes set, so `kind_key` reads "vector" and not
            // "raster" — the type is what the default is filed under.
            app.apply_layer_defaults(li);
            app.doc.set_active(li);
            app.renderer.invalidate();
            app.set_status("vector layer: strokes record as editable geometry");
            app.mark_dirty();
        }
        AppCmd::BatchOpsOpen => {
            app.batch.open = true;
        }
        AppCmd::BatchApply => {
            let s = app.batch_apply();
            app.set_status(s);
        }
        AppCmd::BatchExportPngs => {
            // Resolved to BatchExportPngsPath by `main::pump_commands`.
        }
        AppCmd::BatchExportPngsPath(dir) => {
            let s = app.batch_export_pngs(&dir);
            app.set_status(s);
        }
        AppCmd::ActionRun(idx) => {
            app.action_run(idx);
        }
        AppCmd::ActionRecordToggle(idx) => {
            app.action_record_toggle(idx);
        }
        AppCmd::VectorDelete { stroke } => {
            let li = app.doc.active;
            let Some(before) = app.doc.layers[li].strokes.clone() else {
                return;
            };
            if stroke >= before.strokes.len() {
                return;
            }
            app.doc.begin_op_on(li);
            app.doc.layers[li]
                .strokes
                .as_mut()
                .expect("checked above")
                .strokes
                .remove(stroke);
            app.vector_sel = None;
            app.rederive_vector_layer(li);
            app.doc.end_op_vector_set(before, "Delete stroke");
            app.renderer.invalidate();
            app.set_status("stroke deleted");
            app.mark_dirty();
        }
        AppCmd::LineCorrectOpen => {
            app.line_correct_open = !app.line_correct_open;
        }
        AppCmd::LineCorrect(op) => {
            app.commit_text_edit();
            app.line_correct(op);
        }
        AppCmd::AddFolder => {
            app.commit_text_edit();
            let n = app.doc.layers.iter().filter(|l| l.folder).count() + 1;
            let li = app
                .doc
                .add_folder_above(app.doc.active, format!("Folder {n}"));
            // LP-001, before the Through preference below: both are the
            // same idea (a new layer of this type starts like this), and
            // both ride inside the add's single undo step.
            app.apply_layer_defaults(li);
            // LF-003 (row 19): the preference makes Through the default.
            // Set on the fresh folder directly — presentation state, the
            // same door the palette's Through checkbox uses.
            if app.prefs.new_folder_through {
                let li = app.doc.active;
                app.doc.set_folder_through(li, true);
            }
            app.renderer.invalidate();
            app.mark_dirty();
        }
        AppCmd::ToggleFolderOpen(i) => {
            let open = app.doc.layers.get(i).map(|l| l.open).unwrap_or(true);
            if app.doc.set_folder_open(i, !open) {
                app.mark_dirty();
            }
        }
        AppCmd::RemoveLayer => {
            app.commit_text_edit();
            let i = app.doc.active;
            if app.doc.remove_layer(i) {
                app.object_sel = None;
                app.text_sel = None;
                // The active index moved onto whatever now sits there —
                // audit H1: disarm if it carries no mask.
                app.disarm_mask_edit_if_unmasked();
                app.renderer.invalidate();
                app.renumber_frames();
                app.mark_dirty();
            }
        }
        AppCmd::DuplicateLayer => {
            app.commit_text_edit();
            if app.doc.duplicate_layer(app.doc.active).is_some() {
                app.renderer.invalidate();
                app.mark_dirty();
            }
        }
        AppCmd::MoveLayer { from, slot, depth } => {
            app.commit_text_edit();
            if app.doc.move_block_to_slot(from, slot, depth) {
                app.object_sel = None;
                app.text_sel = None;
                app.renderer.invalidate();
                app.layer_thumbs.clear();
                app.mark_dirty();
            }
        }
        AppCmd::MergeDown => {
            app.commit_text_edit();
            let i = app.doc.active;
            let tone_side = app
                .doc
                .layers
                .get(i)
                .zip(app.doc.layers.get(i.wrapping_sub(1)))
                .is_some_and(|(a, b)| a.tone.is_some() || b.tone.is_some());
            if tone_side {
                app.set_status(
                    "merge refuses tone layers — remove the tone first (it is non-destructive)",
                );
            } else if app.doc.merge_down(i) {
                app.object_sel = None;
                // The merged layer is gone; the active index moved (H1).
                app.disarm_mask_edit_if_unmasked();
                app.renderer.invalidate();
                app.layer_thumbs.clear();
                app.set_status("merged with layer below");
                app.mark_dirty();
            } else if app.doc.layers.get(i).is_some_and(|l| l.is_frame()) {
                app.set_status("frame layers keep their vectors — they never merge");
            }
        }
        AppCmd::MergeSelected => {
            app.commit_text_edit();
            let targets = app.doc.multi_targets();
            if targets.len() < 2 {
                app.set_status(
                    "select the layers first — Ctrl+click or Shift+click rows in the palette",
                );
                return;
            }
            // The same refusal Merge-down gives, checked first so the
            // status can say WHICH thing stopped it (a tone layer is
            // non-destructive; converting it back is one click).
            if targets
                .iter()
                .any(|&i| app.doc.layers[i].tone.is_some())
            {
                app.set_status(
                    "merge refuses tone layers — remove the tone first (it is non-destructive)",
                );
            } else if app.doc.merge_selected(&targets) {
                app.object_sel = None;
                app.vector_sel = None;
                // Layers left the stack; the active index moved (H1).
                app.disarm_mask_edit_if_unmasked();
                app.renderer.invalidate();
                app.layer_thumbs.clear();
                app.set_status(format!("{} layers merged into one", targets.len()));
                app.mark_dirty();
            } else {
                app.set_status(
                    "that selection will not merge — folders, frames, balloons, text, \
                     vector layers, locked or clipped rows, and rows in different folders \
                     all refuse (same rule as merge down)",
                );
            }
        }
        AppCmd::ReleaseFolder => {
            app.commit_text_edit();
            // Target: the active layer when it IS a folder, else the folder
            // holding it — the same walk the frame-folder commands use, so
            // "release" works from a child row without hunting for the
            // header.
            let a = app.doc.active;
            let target = if app.doc.layers[a].folder {
                Some(a)
            } else {
                enclosing_folder(&app.doc, a)
            };
            let Some(li) = target else {
                app.set_status("no folder to release");
                return;
            };
            let lossless = app.doc.folder_release_is_lossless(li);
            if app.doc.release_folder(li) {
                app.object_sel = None;
                app.vector_sel = None;
                app.disarm_mask_edit_if_unmasked();
                app.renderer.invalidate();
                app.layer_thumbs.clear();
                app.set_status(if lossless {
                    "folder released — its layers stepped out, order kept (one undo)"
                } else {
                    "folder released — its own opacity/blend/mask could not come with it, \
                     so the page looks different; Ctrl+Z if that was not what you wanted"
                });
                app.mark_dirty();
            } else if app.doc.layers[li].is_frame() {
                app.set_status(
                    "a frame folder's header holds the panel — use Layer ▸ Rasterize frame folder",
                );
            } else {
                app.set_status("that folder will not release (locked, or it is the last layer)");
            }
        }

        AppCmd::RenameLayer(i, name) => {
            if app.doc.rename_layer(i, name) {
                app.mark_dirty();
            }
        }
        AppCmd::SelectLayer(i) => {
            app.commit_text_edit();
            // The direct-feel rule: a transform float belongs to the layer
            // it was lifted from — switching layers mid-float would retarget
            // its commit's clear+stamp to the WRONG layer. Bake it first.
            if app.transform_drag.is_some() && i != app.doc.active {
                app.commit_open_float();
            }
            // PA-001: picking a layer un-picks the Paper row, whichever way
            // the pick arrived (palette row, shortcut, another command).
            app.paper_selected = false;
            if app.doc.set_active(i) {
                // Audit H1: armed mask-edit must not survive onto a layer
                // that has no mask.
                app.disarm_mask_edit_if_unmasked();
                // A stroke index is only meaningful on the layer it was
                // picked on — carried across, it would light an unrelated
                // stroke on the next vector layer with enough strokes.
                app.vector_sel = None;
                app.mark_dirty();
            }
        }
        AppCmd::ToggleLayerMulti(i) => {
            app.commit_text_edit();
            app.paper_selected = false;
            // Toggling ON (or the active row OFF) moves the editing target,
            // so the same hygiene as SelectLayer applies.
            if app.doc.toggle_multi(i) {
                app.disarm_mask_edit_if_unmasked();
                app.vector_sel = None;
                app.mark_dirty();
            }
        }
        AppCmd::RangeLayerMulti(i) => {
            app.commit_text_edit();
            app.paper_selected = false;
            if app.doc.range_multi(i) {
                app.mark_dirty();
            }
        }
        // Opacity / blend / visibility need no invalidate: the compositor keeps
        // a per-layer signature and rebuilds the canvas when one changes.
        AppCmd::SetLayerOpacity(i, v) => {
            if app.doc.set_layer_opacity(i, v) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerBlend(i, b) => {
            if app.doc.set_layer_blend(i, b) {
                app.mark_dirty();
            }
        }
        AppCmd::SetFolderThrough(i, on) => {
            if app.doc.set_folder_through(i, on) {
                app.set_status(if on {
                    "folder Through — its layers now blend with the page beneath"
                } else {
                    "folder sealed (Normal)"
                });
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerVisible(i, v) => {
            if app.doc.set_layer_visible(i, v) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerLabel(i, l) => {
            if app.doc.set_layer_label(i, l) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerColour(i, c) => {
            if app.doc.set_layer_colour(i, c) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerBlendIf(i, gate) => {
            // Undoable, unlike its neighbours: the gate decides what the
            // EXPORTED page holds, not just what the screen shows.
            //
            // ONE undo step per drag, the `SetFillParams` shape: the
            // pre-image is a whole-stack snapshot taken on the first tick,
            // and ticks inside an open session skip it because the snapshot
            // already on the stack is the pre-SESSION state — which is what
            // Ctrl+Z owes the hand that dragged the slider. (The drag's
            // first tick lands before the panel reports the session, so it
            // is the one that records.)
            //
            // The change test comes first so a tick that moved nothing —
            // the bar re-emitting the same value, a reset on an already
            // reset gate — leaves no do-nothing step in the History palette.
            let gate = gate.map(|g| g.normalized());
            let changes = app
                .doc
                .layers
                .get(i)
                .is_some_and(|l| !l.folder && l.blend_if != gate);
            if changes {
                if app.param_session != Some(i) {
                    let before = app.doc.stack_snapshot();
                    let active = app.doc.active;
                    app.doc.record_structure("Blend if", before, active);
                }
                app.doc.set_layer_blend_if(i, gate);
                app.mark_dirty();
            }
        }
        // The index-free door (keymap follow-up (a)): resolve the row HERE,
        // at execute time, then hand straight to the indexed command so
        // there is one implementation of each verb.
        AppCmd::ActiveLayer(c) => {
            let i = app.doc.active;
            let aimed = app.doc.layers.get(i).map(|l| match c {
                // Off keeps nothing to come back to, so ON re-uses the tint
                // the layer last displayed — the Layer palette's own rule.
                ActiveLayerCmd::ToggleColour => AppCmd::SetLayerColour(
                    i,
                    match l.layer_colour {
                        Some(_) => None,
                        None => Some(crate::ui::LAYER_TINTS[0]),
                    },
                ),
                ActiveLayerCmd::ToggleClip => AppCmd::SetLayerClip(i, !l.clip),
            });
            match aimed {
                Some(c) => dispatch(app, c),
                None => app.set_status("no layer"),
            }
        }
        AppCmd::CommandPalette => crate::ui::open_command_palette(app),
        AppCmd::SetLayerClip(i, v) => {
            if app.doc.set_layer_clip(i, v) {
                app.set_status(if v {
                    "clipped to the layer below"
                } else {
                    "clip removed"
                });
                app.mark_dirty();
            } else {
                app.set_status("folders cannot clip — their group already isolates");
            }
        }
        AppCmd::SetTone(tone) => {
            let i = app.doc.active;
            match app.doc.set_tone(i, tone) {
                true => {
                    // Derived rasters (or their absence) are newer than
                    // whatever the GPU cache holds for this layer.
                    app.renderer.evict_layer(i);
                    app.refresh_tones();
                    // TN-009: while the lattice is off its origin, SAY so —
                    // the art has not moved and nothing else on screen tells
                    // you which of two tone layers you just nudged.
                    app.set_status(match tone {
                        Some(t) if t.offset != [0.0, 0.0] => format!(
                            "tone lattice at ({:+.1}, {:+.1}) px — the art stays put; nudge it to break moiré against another tone layer",
                            t.offset[0], t.offset[1]
                        ),
                        Some(_) => "tone layer — paint grey/black ink, the screen follows; Layer Property tunes it".to_string(),
                        None => "tone removed — painted ink back to plain pixels".to_string(),
                    });
                    app.mark_dirty();
                }
                false => {
                    if app
                        .doc
                        .layers
                        .get(i)
                        .is_some_and(|l| l.folder || l.is_vector())
                    {
                        app.set_status("folders and vector layers cannot be tones");
                    }
                }
            }
        }
        AppCmd::SaveLayerDefaults => {
            use crate::app::layer_defaults as ld;
            let i = app.doc.active;
            let Some(l) = app.doc.layers.get(i) else {
                return;
            };
            let key = ld::kind_key(l);
            if !ld::applies_to(key) {
                return app.set_status(format!(
                    "{} are made by their own tool, out of that tool's settings — there is no new-layer default to save",
                    ld::kind_label(key)
                ));
            }
            app.layer_defaults.capture(l);
            app.layer_defaults.save_if_dirty();
            let what = app.layer_defaults.summary(key).unwrap_or_default();
            app.set_status(format!(
                "new {} will start like this one — {what}",
                ld::kind_label(key)
            ));
        }
        AppCmd::ForgetLayerDefaults => {
            use crate::app::layer_defaults as ld;
            let i = app.doc.active;
            let Some(l) = app.doc.layers.get(i) else {
                return;
            };
            let key = ld::kind_key(l);
            if !app.layer_defaults.has(key) {
                return;
            }
            app.layer_defaults.forget(key);
            app.layer_defaults.save_if_dirty();
            app.set_status(format!("new {} start stock again", ld::kind_label(key)));
        }
        AppCmd::SetLayerDefaultsIncludeTone(on) => {
            use crate::app::layer_defaults as ld;
            let i = app.doc.active;
            let Some(l) = app.doc.layers.get(i) else {
                return;
            };
            let key = ld::kind_key(l);
            app.layer_defaults.set_include_tone(key, on);
            app.layer_defaults.save_if_dirty();
            app.set_status(if on {
                format!(
                    "saving a {} default will include its screentone",
                    ld::kind_label(key).trim_end_matches('s')
                )
            } else {
                format!(
                    "saving a {} default will leave the screentone out",
                    ld::kind_label(key).trim_end_matches('s')
                )
            });
        }
        AppCmd::ToneShowArea => {
            app.tone_show_area = !app.tone_show_area;
            app.set_status(if app.tone_show_area {
                "tone area shown (green tint over every toned region — a print check, not part of the art)"
            } else {
                "tone area hidden"
            });
            app.mark_dirty();
        }
        AppCmd::SetLayerLock(i, v) => {
            if app.doc.set_layer_lock(i, v) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerLockAlpha(i, v) => {
            if app.doc.set_layer_lock_alpha(i, v) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerReference(i, v) => {
            // RF-001: independent toggle; a FOLDER row toggles its whole
            // child run (the folder is one unit).
            let targets = reference_unit(&app.doc, i);
            let mut any = false;
            for &t in &targets {
                any |= app.doc.set_layer_reference(t, v);
            }
            if any {
                let n = app.doc.reference_layers().len();
                app.set_status(if n > 0 {
                    format!(
                        "{n} reference layer{} — fill/wand refer to them",
                        if n > 1 { "s" } else { "" }
                    )
                } else if v {
                    "reference layer set — fill/wand can refer to it".into()
                } else {
                    "reference layer cleared".into()
                });
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerReferenceSolo(i) => {
            if app.doc.set_layer_reference_solo(i) {
                app.set_status("reference solo — every other layer cleared");
                app.mark_dirty();
            }
        }
        AppCmd::ClearReferences => {
            if app.doc.reference_layers().is_empty() {
                app.set_status("no reference layers set");
            } else {
                app.doc.clear_references();
                app.set_status("reference layers cleared");
                app.mark_dirty();
            }
        }
        AppCmd::RulerArm(kind) => {
            app.ruler_pending = Some(kind);
            app.set_status(match kind {
                RulerKind::Line => "drag on the canvas to draw a line ruler",
                RulerKind::VanishingPoint => "drag from the vanishing point to set its first ray",
                RulerKind::Perspective => {
                    "drag the eye level — both ends become vanishing points; strokes aim at either VP or run vertical"
                }
                RulerKind::Perspective1 => {
                    "drag from the vanishing point along the eye level; strokes aim at it, or run along/across the horizon"
                }
                RulerKind::Perspective3 => {
                    "drag the eye level — a third vanishing point lands on the side you dragged toward; drag it where you want it"
                }
                RulerKind::Curve => "click the curve's corners — double-click (or Enter) to finish",
                RulerKind::Parallel => "drag the direction — every stroke comes out parallel to it",
                RulerKind::Concentric => "drag from the centre — the length sets the ring spacing",
                RulerKind::Symmetric => {
                    "drag from the symmetry centre outward — the drag sets the first axis"
                }
                RulerKind::GuideH => "click where the horizontal guide goes",
                RulerKind::GuideV => "click where the vertical guide goes",
            });
        }
        AppCmd::RulerSnapToggle => {
            app.doc.rulers.on = !app.doc.rulers.on;
            app.rebuild_twins();
            app.set_status(if app.doc.rulers.on {
                "ruler snapping ON"
            } else {
                "ruler snapping OFF (rulers stay drawn)"
            });
            app.mark_dirty();
        }
        AppCmd::RulerSpecialSnapToggle => {
            app.doc.rulers.special_on = !app.doc.rulers.special_on;
            app.rebuild_twins();
            app.set_status(if app.doc.rulers.special_on {
                "special rulers ON (parallel/concentric/guide/symmetry)"
            } else {
                "special rulers OFF (line/curve/vanishing-point rulers unaffected)"
            });
            app.mark_dirty();
        }
        AppCmd::RulerSymmetricCount => {
            // CSP's ladder, cycled. Applies to every symmetric ruler (you
            // keep one) and to the default for the next created.
            const LADDER: [u16; 7] = [2, 3, 4, 6, 8, 12, 16];
            let cur = app
                .doc
                .rulers
                .items
                .iter()
                .rev()
                .find_map(|r| match r {
                    mn_core::Ruler::Symmetric { lines, .. } => Some(*lines),
                    _ => None,
                })
                .unwrap_or(app.symmetric_lines);
            let next = LADDER
                .iter()
                .position(|&n| n == cur)
                .map(|i| LADDER[(i + 1) % LADDER.len()])
                .unwrap_or(2);
            let mut changed = 0;
            for r in &mut app.doc.rulers.items {
                if let mn_core::Ruler::Symmetric { lines, .. } = r {
                    *lines = next;
                    changed += 1;
                }
            }
            app.symmetric_lines = next;
            app.rebuild_twins();
            app.set_status(if changed > 0 {
                format!("symmetric rulers: {next} lines")
            } else {
                format!("symmetry line count: {next} (creates at this count)")
            });
            app.mark_dirty();
        }
        AppCmd::RulerAttachAll(layer) => {
            let before = app.doc.rulers.clone();
            let n = app.doc.rulers.items.len();
            app.doc.rulers.set_all_attach(layer);
            app.ruler_lock = Default::default();
            app.ruler_move = None;
            app.rebuild_twins();
            app.doc.record_rulers(before, "Attach rulers");
            match layer {
                Some(l) => {
                    let name = app
                        .doc
                        .layers
                        .get(l)
                        .map(|l| l.name.clone())
                        .unwrap_or_default();
                    app.set_status(format!("{n} rulers attached to \"{name}\" — one undo"));
                }
                None => app.set_status(format!("{n} rulers page-wide again — one undo")),
            }
        }
        AppCmd::RulerClear => {
            let before = app.doc.rulers.clone();
            app.doc.rulers.items.clear();
            // The curve rulers go too (issue #3) — but only the hand-made
            // ones. A panel border published as a ruler is the FRAME's
            // property, and `sync_frame_rulers` retracts by value against
            // `frame_rulers`; dropping those here would desync that
            // bookkeeping (they would vanish now and never be retracted).
            app.doc.rulers.curves = app.frame_rulers.clone();
            app.ruler_pending = None;
            // A live sticky lock indexing into the cleared set would fall
            // to snap_locked's else (unsnapped) — safe, but stale; drop it
            // (round-47 handoff item 1).
            app.ruler_lock = Default::default();
            // Same for a live move (its index is into the cleared set).
            app.ruler_move = None;
            app.rebuild_twins();
            // One gesture, one step — and the step's pre-image holds the
            // hand-made curves the clear dropped, so undo brings exactly
            // those back. The frame-published ones never left, so undo
            // cannot double them.
            app.doc.record_rulers(before, "Delete rulers");
            app.set_status(if app.frame_rulers.is_empty() {
                "rulers cleared"
            } else {
                "rulers cleared — panel-border rulers stay with their frames"
            });
            app.mark_dirty();
        }
        AppCmd::SetLayerDraft(i, v) => {
            if app.doc.set_layer_draft(i, v) {
                app.set_status(if v {
                    "draft layer: shown on screen, skipped by fill refs and export"
                } else {
                    "draft flag removed"
                });
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerEscape(i, v) => {
            if app.doc.set_layer_escape(i, v) {
                app.set_status(if v {
                    "bursts out of the panel: drawn over the frame border, outside the mask"
                } else {
                    "back inside the panel"
                });
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerSpillSeat(i, top) => {
            // The seat is not a tile edit — no revision moves — so the
            // renderer only notices through its layer signature. It does
            // (mn_gpu::LayerSig::spill); the invalidate is belt and braces
            // for the incremental path.
            if app.doc.set_layer_spill_seat(i, top) {
                let name = top
                    .and_then(|t| app.doc.layers.get(t))
                    .map(|l| l.name.clone());
                app.set_status(match name {
                    Some(n) => format!("the burst now draws over “{n}” and everything below it"),
                    None => "the burst draws over its own panel only".to_owned(),
                });
                app.renderer.invalidate();
                app.mark_dirty();
            }
        }

        other => return frames::run(app, other, cmd_tail),
    }
    run_cmd_tail(app, cmd_tail);
}
