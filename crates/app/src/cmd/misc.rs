//! `AppCmd` arms: colour slots and palettes, tool/sub-tool
//! selection, file objects, the view (zoom/rotate/flip), per-layer
//! effects, and paper.

use super::*;

/// The status line after either view flip. Both flags read together: with
/// the vertical flip in, "view back to normal" is only true when NEITHER
/// axis is flipped, and H+V is a half turn rather than a mirror.
fn flip_status(vp: &mn_gpu::Viewport) -> &'static str {
    match (vp.flip_h, vp.flip_v) {
        (true, true) => "view turned 180° — mirrored both ways is a half turn",
        (true, false) => "view mirrored — the classic drawing-error check",
        (false, true) => "view flipped vertically — the same check, upside down",
        (false, false) => "view back to normal",
    }
}

/// Where the manual lives, for Help ▸ Manual: `manual/index.html` beside
/// the running exe (the shipped layout); a dev build falls back to the
/// repository's docs/manual via the compiled manifest dir.
pub(crate) fn manual_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let shipped = exe.parent()?.join("manual").join("index.html");
    if shipped.exists() {
        return Some(shipped);
    }
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual/index.html");
    dev.exists().then_some(dev)
}

/// One manual TOPIC, beside the index — whichever of the two folders above
/// actually exists, so the palette's `?` rows follow the manual home rather
/// than guessing a second time. A missing file says so instead of handing
/// the shell a path that opens nothing.
pub(crate) fn manual_page_path(file: &str) -> Option<std::path::PathBuf> {
    let page = manual_path()?.with_file_name(file);
    page.exists().then_some(page)
}

pub(super) fn run(app: &mut App, cmd: AppCmd, cmd_tail: CmdTail) {
    match cmd {
        // --- colour slots ---------------------------------------------------
        AppCmd::SetSlotColor(rgb) => {
            dispatch(app, AppCmd::SetSlotColorLive(rgb));
            crate::app::push_color_history(&mut app.color_history, rgb);
            app.note_color_history();
        }
        AppCmd::SetSlotColorLive(rgb) => {
            // Picking a colour while Transparent is active returns to Main —
            // CSP does the same (you chose a colour; you mean to draw with it).
            if app.slot == Slot::Transparent {
                app.slot = Slot::Main;
            }
            match app.slot {
                Slot::Sub => app.sub_color = rgb,
                _ => app.main_color = rgb,
            }
            app.apply_draw_state();
            app.mark_dirty();
        }
        AppCmd::ClearColorHistory => {
            app.color_history.clear();
            app.note_color_history();
            app.set_status("recent colours cleared");
            app.mark_dirty();
        }
        AppCmd::AddHistoryToSwatches => {
            let before = app.swatches.len();
            // The history stays as it was: this copies, it does not move.
            for rgb in app.color_history.clone() {
                if app.swatches.len() >= crate::app::SWATCH_CAP {
                    break;
                }
                if !app.swatches.iter().any(|s| s.rgb == rgb) {
                    app.swatches.push(mn_core::palette::Swatch::new(rgb));
                }
            }
            let n = app.swatches.len() - before;
            if n > 0 {
                crate::app::save_swatches(&app.swatches);
            }
            app.set_status(match n {
                0 => "every recent colour is already in the Color Set".to_string(),
                1 => "1 colour added to the Color Set".to_string(),
                n => format!("{n} colours added to the Color Set"),
            });
            app.mark_dirty();
        }
        AppCmd::SetSlot(s) => {
            app.slot = s;
            app.apply_draw_state();
            app.mark_dirty();
        }
        AppCmd::ToggleTransparentSlot => {
            let next = if app.slot == Slot::Transparent {
                Slot::Main
            } else {
                Slot::Transparent
            };
            dispatch(app, AppCmd::SetSlot(next));
        }
        AppCmd::AddSwatch(rgb) => {
            app.swatches.push(mn_core::palette::Swatch::new(rgb));
            crate::app::save_swatches(&app.swatches);
            app.mark_dirty();
        }
        AppCmd::DeleteSwatch(i) => {
            if i < app.swatches.len() {
                app.swatches.remove(i);
                crate::app::save_swatches(&app.swatches);
            }
            app.mark_dirty();
        }
        AppCmd::ImportPalette => {}
        AppCmd::ImportPalettePath(p) => match std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|t| mn_core::palette::parse_gpl(&t))
        {
            Ok(cols) => {
                let n = cols.len();
                let name = p.file_stem().map(|s| s.to_string_lossy().into_owned());
                // Names come through: a palette whose swatches are called
                // "skin — shadow" is worthless as anonymous squares, and
                // the parser has always returned them.
                app.swatches.extend(cols);
                crate::app::save_swatches(&app.swatches);
                app.set_status(match name {
                    Some(nm) => format!("imported {n} colours from {nm}.gpl"),
                    None => format!("imported {n} colours"),
                });
                app.mark_dirty();
            }
            Err(e) => app.set_error(format!("palette import: {e}")),
        },
        AppCmd::ImportGradient => {}
        AppCmd::ImportGradientPath(p) => match std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|t| mn_core::gradient::import_ggr(&t))
        {
            Ok(mut g) => {
                g.name = app.grad_set.free_name(&g.name);
                let name = g.name.clone();
                app.grad_set.items.push(g);
                app.grad_set_sel = app.grad_set.len() - 1;
                app.layout.note_gradients(&app.grad_set.to_json());
                app.set_status(format!("imported gradient “{name}”"));
                app.mark_dirty();
            }
            Err(e) => app.set_error(format!("gradient import: {e}")),
        },
        AppCmd::SwapColors => {
            std::mem::swap(&mut app.main_color, &mut app.sub_color);
            app.apply_draw_state();
            app.mark_dirty();
        }
        AppCmd::ResetColors => {
            app.main_color = [0.0, 0.0, 0.0];
            app.sub_color = [1.0, 1.0, 1.0];
            app.apply_draw_state();
            app.mark_dirty();
        }
        AppCmd::SetTool(t) => {
            if t.enabled() {
                if t != Tool::Text {
                    app.commit_text_edit();
                }
                // A live transform float (pasted material, floating
                // selection) owns the canvas over every tool, so leaving
                // it armed across a switch turns the pen into a
                // move-the-material tool (owner 2026-08-21). Switching
                // away COMMITS the placement — the Object tool keeps it
                // live for further nudging.
                if t != Tool::Object && app.tool != t && app.transform_drag.is_some() {
                    dispatch(app, AppCmd::TransformCommit);
                }
                let old = app.tool;
                app.tool = t;
                // Pen and Eraser are separate sub tools (owner order): each
                // remembers its own brush across switches.
                if old != t {
                    match old {
                        Tool::Pen => app.pen_preset = app.selected_preset,
                        Tool::Eraser => app.eraser_preset = app.selected_preset,
                        _ => {}
                    }
                    let want = match t {
                        Tool::Pen => app.pen_preset,
                        Tool::Eraser => app.eraser_preset,
                        _ => None,
                    };
                    if let Some(i) = want {
                        if app.selected_preset != Some(i) && i < app.presets.len() {
                            let p = app.presets[i].1.clone();
                            app.push_cmd(AppCmd::SelectBrush(p));
                        }
                    }
                }
                // Owner item (2026-08-19, top of the text arc): switching
                // from Text to Object hands him the BALLOON under the
                // selected text — CSP's behaviour, the part he likes. Falls
                // back to keeping the text when no balloon contains it.
                if t == Tool::Object
                    && old == Tool::Text
                    && let Some((li, ti)) = app.text_sel
                    && let Some(c) = app
                        .doc
                        .layers
                        .get(li)
                        .and_then(|l| l.texts())
                        .and_then(|ts| ts.texts.get(ti))
                        .map(|it| it.center())
                {
                    let mut handover = None;
                    for lj in (0..app.doc.layers.len()).rev() {
                        let l = &app.doc.layers[lj];
                        if !l.visible {
                            continue;
                        }
                        if let Some(bs) = l.balloons() {
                            for bi in (0..bs.balloons.len()).rev() {
                                if bs.balloons[bi].contains(c) {
                                    handover = Some((lj, bi));
                                    break;
                                }
                            }
                        }
                        if handover.is_some() {
                            break;
                        }
                    }
                    if let Some((lj, bi)) = handover {
                        app.text_sel = None;
                        app.balloon_sel = Some((lj, bi));
                        app.object_pick = Some((c[0], c[1]));
                        app.set_status("balloon selected — O cycles the stack under it");
                    }
                }
                app.frame_drag = None;
                app.frame_poly = None;
                app.frame_pen = None;
                // Row 157: a figure waiting on its second stage has no
                // gesture left to commit it once the tool changes, and it
                // would keep painting its preview over the new tool.
                app.figure_stage2 = None;
                // FI-050: same for a freeform gradient waiting on its second
                // guide line — the tool that would draw it is gone.
                app.grad_free = None;
                // L-001: a half-traced magnetic outline has no gesture left
                // to close it once the tool changes — and it holds an edge
                // cache, so dropping it frees that too.
                app.magnetic = None;
                app.object_drag = None;
                app.text_gesture = None;
                app.text_obj_drag = None;
                if t != Tool::Object {
                    app.object_sel = None;
                    app.text_sel = None;
                }
                app.apply_draw_state();
                app.mark_dirty();
            }
        }

        AppCmd::ObjectCycle(forward) => app.object_cycle(forward),
        AppCmd::SetLayerEyeSolo(i) => {
            if app.doc.only_visible(i) && app.eye_solo_backup.is_some() {
                let b = app.eye_solo_backup.take().unwrap();
                app.doc.restore_visibility(&b);
                app.set_status("visibility restored");
            } else if let Some(b) = app.doc.set_layer_visibility_solo(i) {
                app.eye_solo_backup = Some(b);
                app.set_status("solo — Alt+click the eye again to restore");
            } else {
                app.set_status("no such layer");
            }
            app.mark_dirty();
        }
        AppCmd::ToggleHud => {
            app.hud_open = !app.hud_open;
            app.mark_dirty();
        }
        AppCmd::OpenManual => match manual_path() {
            Some(p) => unsafe { crate::win32::shell_open(&p) },
            None => app.set_status(
                "manual not found — docs/manual/ lives beside the exe (manual/index.html)",
            ),
        },
        AppCmd::OpenManualPage(file) => match manual_page_path(file) {
            Some(p) => unsafe { crate::win32::shell_open(&p) },
            None => app.set_status(format!("manual page not found — manual/{file}")),
        },
        AppCmd::WorkspaceApply(name) => {
            if app.workspace_apply(&name) {
                app.set_status(format!("workspace: {name}"));
            } else {
                app.set_status(format!("no workspace named \"{name}\""));
            }
            app.mark_dirty();
        }
        AppCmd::TextStylePick(name) => {
            // No early return: dispatch does bookkeeping after the match
            // (pages palette, clip report, action recording) that every
            // command owes it.
            match app.doc.text_styles.iter().find(|s| s.name == name).cloned() {
                None => app.set_status(format!("no work style named \"{name}\"")),
                Some(style) => {
                    // The selected text item follows the style; the recorded
                    // step is the assignment's own, so this stays one press.
                    if let Some((layer, item)) = crate::text_edit::property_target(app) {
                        dispatch(
                            app,
                            AppCmd::TextStyleAssign {
                                layer,
                                item,
                                name: Some(name.clone()),
                            },
                        );
                    }
                    // Either way it becomes what the NEXT text box is typed in.
                    if !style.font.is_empty() {
                        app.text_font = style.font.clone();
                    }
                    app.text_size_pt = style.size_pt;
                    app.text_letter_pt = style.letter_spacing_pt;
                    app.text_line = style.line_spacing;
                    app.text_style_new = Some(name);
                    app.mark_dirty();
                }
            }
        }

        // The Sub Tool list's rows, as one command. The tool switch goes
        // through `SetTool` and the two modes that have their own commands
        // through those, so a palette pick and a click in the list run the
        // same code — including the status lines and the mid-gesture
        // cleanups those arms carry. The state half lives in
        // `subtools::apply_state`, which the startup memory restore replays
        // WITHOUT the tool switch (owner ask 2026-08-25).
        AppCmd::SetSubTool(s) => {
            dispatch(app, AppCmd::SetTool(s.tool()));
            crate::subtools::apply_state(app, s);
            app.mark_dirty();
        }
        // `,` / `.` as a command, so the two default chords are rebindable
        // and the walk is reachable from Ctrl+K. `step_subtool` queues its
        // own command, which `run_cmd_tail` drains like any other.
        AppCmd::StepSubTool(fwd) => app.step_subtool(fwd),
        AppCmd::CloseWindow => app.close_requested = true,
        AppCmd::PaletteOpen(p) => {
            // Reopening an open palette moves nothing, which from the
            // command palette looks like the press was swallowed — say so.
            if crate::ui::dock::is_open(app, p) {
                app.set_status(format!("{} is already open", p.title()));
            } else {
                crate::ui::dock::reopen(app, p);
            }
            app.mark_dirty();
        }


        // --- row 166 file objects (app/file_object.rs) ----------------------
        // `ImportFileObject` / `RelinkFileObject` are the picker's cue and
        // never reach here with anything to do — `main::pump_commands`
        // turns them into the `…Path` forms (and drops them on cancel), the
        // way every other file command in this file works.
        AppCmd::ImportFileObject | AppCmd::RelinkFileObject(_) => {}
        AppCmd::ImportFileObjectPath(p) => app.import_file_object(&p),
        AppCmd::RelinkFileObjectPath(li, p) => app.relink_file_object(li, &p),
        AppCmd::UpdateFileObjects => app.update_file_objects(),

        // --- view -----------------------------------------------------------
        AppCmd::ZoomFit => app.fit_to_view(),
        AppCmd::Zoom100 => {
            let c = app.canvas_center();
            let z = app.viewport.zoom;
            if z > 0.0 {
                app.viewport.zoom_around(c, 1.0 / z);
            }
            app.mark_dirty();
        }
        AppCmd::ZoomStep(f) => {
            let c = app.canvas_center();
            app.viewport.zoom_around(c, f);
            app.mark_dirty();
        }
        AppCmd::ZoomIn => {
            app.zoom_ladder_step(true);
            app.mark_dirty();
        }
        AppCmd::ZoomOut => {
            app.zoom_ladder_step(false);
            app.mark_dirty();
        }
        AppCmd::ZoomTo(z) => {
            let c = app.canvas_center();
            app.viewport.set_zoom_around(c, z);
            app.set_status(format!("zoom {}%", (app.viewport.zoom * 100.0).round()));
            app.mark_dirty();
        }
        AppCmd::RotateView(d) => {
            let c = app.canvas_center();
            app.viewport.rotate_around(c, d);
            app.mark_dirty();
        }
        AppCmd::RotateViewStep(cw) => {
            let step = app.prefs.rotate_step_deg.to_radians();
            dispatch(app, AppCmd::RotateView(if cw { step } else { -step }));
        }
        AppCmd::RotateViewTo(deg) => {
            let c = app.canvas_center();
            app.viewport.set_rotation_around(c, deg.to_radians());
            // Report the angle the VIEWPORT settled on, not the one asked
            // for: it wraps to (-180, 180], so 270 reads back as -90 and
            // the status line must not claim otherwise.
            app.set_status(format!(
                "view rotated {}°",
                app.viewport.rotate_rad.to_degrees().round()
            ));
            app.mark_dirty();
        }
        AppCmd::RotateReset => {
            let c = app.canvas_center();
            app.viewport.set_rotation_around(c, 0.0);
            app.mark_dirty();
        }
        AppCmd::RotateFlipReset => {
            let c = app.canvas_center();
            // Each flip is a TOGGLE, so it is only sent when that axis is
            // flipped; unflipping also mirrors the rotation, so they go
            // first and the absolute rotation reset lands on top.
            if app.viewport.flip_h {
                app.viewport.flip_around(c);
            }
            if app.viewport.flip_v {
                app.viewport.flip_v_around(c);
            }
            app.viewport.set_rotation_around(c, 0.0);
            app.set_status("view reset — upright and unmirrored");
            app.mark_dirty();
        }
        AppCmd::ViewReset => {
            let c = app.canvas_center();
            if app.viewport.flip_h {
                app.viewport.flip_around(c);
            }
            if app.viewport.flip_v {
                app.viewport.flip_v_around(c);
            }
            app.viewport.set_rotation_around(c, 0.0);
            app.fit_to_view();
            app.set_status("view reset — upright, unmirrored, fitted");
            app.mark_dirty();
        }
        AppCmd::OpenCanvasView => {
            let had = crate::ui::dock::canvas_view_open(app);
            crate::ui::dock::open_canvas_view(app);
            app.set_status(if had {
                "second view focused"
            } else {
                "second view opened — the whole page, live, with its own zoom"
            });
        }
        AppCmd::SetGuidesHidden(hidden) => {
            app.layout.note_guides_hidden(hidden);
            app.set_status(if hidden {
                "crop marks and margins hidden — the page is unchanged"
            } else {
                "crop marks and margins shown"
            });
            app.mark_dirty();
        }
        AppCmd::NextDoc(forward) => {
            // `docs` is dense (one slot per tab, `ensure_slots` keeps at
            // least one), so the neighbour is arithmetic. One tab open is a
            // no-op that SAYS so, rather than a key that looks broken.
            let n = app.docs.len().max(1);
            if n < 2 {
                app.set_status("only one work is open");
            } else {
                let i = if forward {
                    (app.active_doc + 1) % n
                } else {
                    (app.active_doc + n - 1) % n
                };
                if app.switch_doc(i) {
                    app.set_status(format!("work {} of {n}", i + 1));
                }
            }
        }
        AppCmd::SetGrid { on, mm, div } => {
            app.layout.note_grid(on, mm, div);
            // The refusal has to speak, because the guard is invisible: a
            // grid whose lines would land closer than GRID_MIN_PX apart is
            // dropped, and a silent drop reads as "the grid is broken".
            let ruled = !crate::app::grid_lines(
                app.doc.size,
                app.page_dpi(),
                app.layout.grid_mm,
                app.layout.grid_div,
            )
            .is_empty();
            if !app.layout.grid_on {
                app.set_status("grid off");
            } else if ruled {
                app.set_status(format!(
                    "grid on — {} mm cells, {} divisions",
                    app.layout.grid_mm, app.layout.grid_div
                ));
            } else {
                app.set_error(format!(
                    "{} mm cut into {} is under {} px on this page — nothing ruled; \
                     use bigger cells or fewer divisions",
                    app.layout.grid_mm,
                    app.layout.grid_div,
                    crate::app::GRID_MIN_PX
                ));
            }
            app.mark_dirty();
        }
        AppCmd::TransformReset => {
            if let Some(drag) = &mut app.transform_drag {
                drag.reset();
                app.set_status("transform reset — still transforming");
                app.mark_dirty();
            }
        }
        AppCmd::SetToolLock(on) => {
            app.props_current.locked = on;
            // Locking TAKES the snapshot: whatever is on the sliders now is
            // what returning to this sub tool restores. Unlocking writes
            // too, so today's drift becomes the new normal on the way out
            // instead of being silently thrown away by the next switch.
            app.snapshot_current_props();
            app.set_status(if on {
                "tool settings locked — change them freely; they come back when you return to this sub tool"
            } else {
                "tool settings unlocked — the values on the sliders are now this sub tool's own"
            });
            app.mark_dirty();
        }
        AppCmd::FlipView => {
            let c = app.canvas_center();
            app.viewport.flip_around(c);
            app.set_status(flip_status(&app.viewport));
            app.mark_dirty();
        }
        AppCmd::FlipViewV => {
            let c = app.canvas_center();
            app.viewport.flip_v_around(c);
            app.set_status(flip_status(&app.viewport));
            app.mark_dirty();
        }
        // --- per-layer effects (TRIAGE 21/27/30) ---------------------------
        AppCmd::SetEdge(i, edge) => {
            if app.doc.set_edge(i, edge) {
                // The derived outline (or its absence) is newer than anything
                // the GPU cache holds for this layer — and the effect writes
                // into tiles the layer never painted, so a stale cache would
                // leave a ring floating with nothing inside it.
                app.renderer.evict_layer(i);
                app.refresh_tones();
                app.set_status(match edge {
                    Some(e) => format!(
                        "border effect — {:.1} px outline round the layer's own alpha; the painted pixels are untouched",
                        e.width()
                    ),
                    None => "border effect off — the drawing is exactly as it was".to_string(),
                });
                app.mark_dirty();
            } else if app.doc.layers.get(i).is_some_and(|l| l.folder) {
                app.set_status("folders have no alpha of their own to outline");
            }
        }
        AppCmd::SetLayerSubColour(i, c) => {
            if app.doc.set_layer_sub_colour(i, c) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerExpression(i, e) => {
            if app.doc.set_layer_expression(i, e) {
                app.set_status(match e {
                    mn_core::LayerExpression::Colour => "layer displayed in colour",
                    mn_core::LayerExpression::Grey => "layer previewed as grey — display only",
                    mn_core::LayerExpression::Mono => {
                        "layer previewed as 1-bit mono — display only, nothing is converted"
                    }
                });
                app.mark_dirty();
            }
        }

        // --- paper (PA-001) -------------------------------------------------
        AppCmd::PaperToggle => {
            let on = !app.doc.paper.visible;
            if app.doc.set_paper_visible(on) {
                app.set_status(if on {
                    "paper shown"
                } else {
                    "paper hidden — the checker is where the page is transparent (a check; export is unaffected)"
                });
                app.mark_dirty();
            }
        }
        AppCmd::SetPaperColour(c) => {
            if app.doc.set_paper_colour(c) {
                let [r, g, b] = c;
                app.set_status(format!(
                    "paper #{r:02x}{g:02x}{b:02x} — the page exports on it"
                ));
                app.mark_dirty();
            }
        }
        // The chain of `run`s is the one `match` this file was cut out
        // of: every `AppCmd` is claimed by exactly one module. The
        // compiler cannot prove that across module walls any more, so a
        // variant nobody claims says so here instead of doing nothing.
        other => unreachable!("AppCmd claimed by no cmd module: {other:?}"),
    }
    run_cmd_tail(app, cmd_tail);
}
