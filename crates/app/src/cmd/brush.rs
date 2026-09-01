//! `AppCmd` arms: brush selection and every brush parameter.

use super::*;

pub(super) fn run(app: &mut App, cmd: AppCmd, cmd_tail: CmdTail) {
    match cmd {
        AppCmd::SetTaper { px, min } => {
            app.props_current.taper_px = px.clamp(0.0, 500.0);
            app.props_current.taper_min = min.clamp(0.0, 1.0);
            let t = app.brush.inner_mut();
            t.length_px = app.props_current.taper_px;
            t.min = app.props_current.taper_min;
            app.mark_dirty();
        }

        // --- brush --------------------------------------------------------
        AppCmd::SelectBrush(p) => {
            // TODO #7, the `mn-engine` preset key: a procedural preset
            // builds its own engine instead of the MyPaint one (per-sub-tool
            // identities without new preset formats).
            //
            // ONE reader, shared with the sub tool PREVIEW
            // (`app::preset_engine`). This arm used to keep a second
            // copy of the name list and "dot" was missing from it, so the Dot
            // Pen previewed as the row-96 pixel pen and then inked as a plain
            // MyPaint brush off a preset file that carries no libmypaint
            // settings — a soft, pressure-tapered line, which is the one
            // thing that sub tool exists not to draw.
            let engine_kind = mn_brush::preset_engine_key(&p);
            if let Some(kind) = crate::app::preset_engine(&p) {
                app.store_current_props();
                app.selected_preset = app.presets.iter().position(|(_, q)| *q == p);
                *app.engine_mut() = Engine::new(kind);
                app.load_props_for(&p);
                app.apply_props();
                app.apply_draw_state();
                // The replaced engine took the symmetry twins with it —
                // this arm returned before the MyBrush arm's rebuild, so
                // switching to hairy/curve/dyna/grid silently inked one
                // half of a mirrored drawing.
                app.rebuild_twins();
                app.set_status(match engine_kind.as_deref() {
                    Some("hairy") => "hairy engine: bristle fan",
                    Some("curve") => "curve engine: scallop arches",
                    Some("dyna") => "dyna engine: spring tip",
                    Some("dot") => "dot pen: one whole pixel per dab",
                    _ => "grid engine: lattice dots",
                });
                app.mark_dirty();
                return;
            }
            match MyBrush::load(&p) {
                Ok(b) => {
                    // CSP model: the outgoing sub tool keeps its settings, the
                    // incoming one restores its own (or starts from defaults).
                    app.store_current_props();
                    app.selected_preset = app.presets.iter().position(|(_, q)| *q == p);
                    *app.engine_mut() = Engine::new(EngineKind::My(Box::new(b)));
                    app.load_props_for(&p);
                    app.apply_props();
                    app.apply_draw_state();
                    // This sub tool's curve edits, replayed on the fresh engine
                    // (they live only in the session, never in the preset file).
                    let overrides: Vec<((u8, u8), Vec<(f32, f32)>)> = app
                        .curve_overrides
                        .get(&p)
                        .map(|m| m.iter().map(|(k, v)| (*k, v.clone())).collect())
                        .unwrap_or_default();
                    for ((s, n), pts) in overrides {
                        let (cs, sn) = (CurveSetting::from_index(s), CurveSensor::from_index(n));
                        if let (Some(sid), Some(iid)) = (cs.setting_id(), sn.input_id()) {
                            app.engine_mut().set_mapping(sid, iid, &pts);
                        }
                    }
                    // The replaced engine took the symmetry twins with it.
                    // Unconditional: rebuild_twins derives the whole twin
                    // set (mirror/wrap AND the symmetric ruler's, which
                    // the old mirror-only guard missed) from current state
                    // and is a cheap no-op when none apply.
                    app.rebuild_twins();
                    // TL-013: the one moment a locked tool's snap-back
                    // happens is right here, so the line that names the
                    // brush is the line that has to mention it.
                    let name = app.brush_name().to_owned();
                    let lock = if app.props_current.locked {
                        " (locked — settings restored)"
                    } else {
                        ""
                    };
                    app.set_status(format!("brush: {name}{lock}"));
                }
                Err(e) => app.set_error(format!("brush {} failed: {e}", p.display())),
            }
        }
        // All three set their own status lines, and each re-checks that the
        // path is a preset the artist owns before touching the disk.
        AppCmd::RenameBrush { path, name } => app.rename_brush(path, name),
        AppCmd::DuplicateBrush(p) => app.duplicate_brush(p),
        AppCmd::BrushSaveCurrent => app.save_current_brush(),
        AppCmd::DeleteBrush(p) => app.delete_brush(p),
        AppCmd::SetBrushSizePx(px) => {
            let px = if px.is_finite() { px } else { DEFAULT_SIZE_PX };
            app.props_current.size_px = px.clamp(SIZE_PX_MIN, SIZE_PX_MAX);
            let px = app.props_current.size_px;
            app.engine_mut().set_size_px(px);
            app.mark_dirty();
        }
        AppCmd::SetInterval(iv) => {
            app.props_current.interval = iv;
            // Fixed remembers its gap across a trip through the relative
            // modes; the other modes leave the remembered number alone.
            if let Interval::FixedPx(px) = iv {
                app.props_current.interval_px = px;
            }
            app.engine_mut().set_interval(iv);
            app.mark_dirty();
        }
        AppCmd::SetDensityByGap(on) => {
            app.props_current.density_by_gap = Some(on);
            app.engine_mut().set_density_by_gap(on);
            app.mark_dirty();
        }
        AppCmd::SetAntiAlias(aa) => {
            app.props_current.anti_alias = aa;
            app.engine_mut().set_anti_alias(aa);
            app.mark_dirty();
        }
        AppCmd::SetPaintDensity(d) => {
            app.props_current.paint_density = d.clamp(0.0, 1.0);
            let v = app.props_current.paint_density;
            app.engine_mut().set_paint_density(v);
            app.mark_dirty();
        }
        AppCmd::SetColorStretch(v) => {
            app.props_current.color_stretch = v.clamp(0.0, 1.0);
            let v = app.props_current.color_stretch;
            app.engine_mut().set_color_stretch(v);
            app.mark_dirty();
        }
        AppCmd::SetBrushMix(m) => {
            app.props_current.brush_mix = m;
            app.engine_mut().set_color_mixing(m);
            app.mark_dirty();
        }
        AppCmd::SetTransformInterp(i) => {
            // No `mark_dirty`: nothing has been resampled yet. The kernel is
            // read once, at commit — changing it mid-drag costs nothing and
            // re-rendering the overlay would only redraw the SAME GPU-sampled
            // preview (see `interp_row`).
            app.transform_interp = i;
        }
        AppCmd::SetWaterEdge(e) => {
            app.engine_mut().set_water_edge(e);
            // Read the clamped value back so the panel shows what the engine
            // actually holds, the `set_paint_density` habit.
            app.props_current.water_edge = app.engine().water_edge();
            app.mark_dirty();
        }
        AppCmd::SetBlur(v) => {
            app.props_current.blur = v;
            let (v, abs) = (app.props_current.blur, app.props_current.blur_abs);
            app.engine_mut().set_blur(v, abs);
            app.mark_dirty();
        }
        AppCmd::SetBlurAbs(abs) => {
            // The number keeps its face value and changes UNIT, exactly as
            // the randomization pair does: 2 stops meaning "twice the
            // radius" and starts meaning "2 px". Re-pushing it is what makes
            // the switch visible in the stroke instead of only in the label.
            app.props_current.blur_abs = abs;
            let v = app.props_current.blur;
            app.engine_mut().set_blur(v, abs);
            app.mark_dirty();
        }
        AppCmd::SetColorJitter(j) => {
            app.props_current.jitter = j;
            app.engine_mut().set_color_jitter(j);
            app.mark_dirty();
        }
        AppCmd::SetTipFlip(h, v) => {
            app.props_current.tip_flip_h = h;
            app.props_current.tip_flip_v = v;
            app.engine_mut().set_tip_flip(h, v);
            app.mark_dirty();
        }
        AppCmd::SetOpacity(o) => {
            app.props_current.opacity = o.clamp(0.0, 1.0);
            let (v, wash) = (app.props_current.opacity, app.props_current.wash);
            let e = app.engine_mut();
            // In wash mode the slider is the STROKE opacity; Flow owns the
            // per-dab alpha (Krita's Opacity/Flow pair).
            if wash {
                e.set_wash_opacity(v);
            } else {
                e.set_base_opacity(v);
            }
            app.mark_dirty();
        }
        AppCmd::SetMinSize(pct) => {
            app.props_current.min_size = pct.clamp(0.0, 100.0);
            let v = app.props_current.min_size;
            app.engine_mut().set_size_min_pct(v);
            app.mark_dirty();
        }
        AppCmd::SetStabilizer(v) => {
            app.props_current.stabilizer = v.clamp(0.0, 1.0);
            let s = app.props_current.stabilizer;
            app.brush.set_strength(s);
            app.mark_dirty();
        }
        AppCmd::SetCorrection(c) => {
            app.props_current.correct = c.sanitized();
            let c = app.props_current.correct;
            app.brush.set_correction(c);
            app.mark_dirty();
        }
        AppCmd::SetRandomization(v) => {
            let p = &mut app.props_current;
            p.random = if p.random_abs {
                v.max(0.0)
            } else {
                v.clamp(0.0, 1.0)
            };
            let (r, m, a) = (p.random, p.random_min, p.random_abs);
            app.engine_mut().set_randomization(r, m, a);
            app.mark_dirty();
        }
        AppCmd::SetRandomMin(pct) => {
            app.props_current.random_min = pct.clamp(0.0, 100.0);
            let p = app.props_current;
            app.engine_mut()
                .set_randomization(p.random, p.random_min, p.random_abs);
            app.mark_dirty();
        }
        AppCmd::SetRandomAbs(abs) => {
            // Unit change: keep the *look* by converting the amount between
            // log-radius (≈ proportional) and px around the current radius.
            let radius = app.engine().radius_px();
            let p = &mut app.props_current;
            p.random = if abs {
                // log-units → px: a deviation of L on radius r ≈ r·L px.
                (p.random * radius).clamp(0.0, 16.0)
            } else {
                (p.random / radius.max(1.0)).clamp(0.0, 1.0)
            };
            p.random_abs = abs;
            let (r, m, a) = (p.random, p.random_min, p.random_abs);
            app.engine_mut().set_randomization(r, m, a);
            app.mark_dirty();
        }

        AppCmd::SetHardDab(on) => {
            app.props_current.hard_dab = on;
            app.engine_mut().set_hard_dab(on);
            app.mark_dirty();
        }
        AppCmd::SetScatter(v) => {
            let sc = v.clamp(0.0, 4.0);
            app.props_current.scatter = sc;
            app.engine_mut().set_scatter(sc);
            app.mark_dirty();
        }
        AppCmd::SetWash(on) => {
            app.props_current.wash = on;
            let p = app.props_current;
            // Toggling re-applies the full pair: in wash, `opacity` becomes
            // the stroke-level value and `flow` takes over the per-dab knob.
            let e = app.engine_mut();
            if on {
                e.set_flow(p.flow);
                e.set_wash(true, p.opacity, p.brush_blend);
            } else {
                e.set_wash(false, 1.0, Blend::Normal);
                e.set_base_opacity(p.opacity);
            }
            app.mark_dirty();
        }
        AppCmd::SetFlow(v) => {
            app.props_current.flow = v.clamp(0.0, 1.0);
            let f = app.props_current.flow;
            app.engine_mut().set_flow(f);
            app.mark_dirty();
        }
        AppCmd::SetBrushBlend(b) => {
            app.props_current.brush_blend = b;
            let (on, op) = (app.props_current.wash, app.props_current.opacity);
            let e = app.engine_mut();
            e.set_wash_blend(b);
            e.set_wash(on, op, b);
            app.mark_dirty();
        }
        AppCmd::SetBrushDraw(d) => {
            // The ink output is a wash-COMMIT behaviour: choosing one
            // turns the buffer on (and Normal turns no such trick — it
            // just restores the plain blend), the same contract the
            // blend picker applies to itself.
            app.props_current.brush_draw = d;
            let wants_wash = d != mn_brush::BrushDraw::Normal;
            if wants_wash {
                app.props_current.wash = true;
            }
            let p = app.props_current;
            let e = app.engine_mut();
            e.set_wash_draw(d);
            if wants_wash {
                e.set_flow(p.flow);
                e.set_wash(true, p.opacity, p.brush_blend);
            }
            app.mark_dirty();
        }
        AppCmd::SetTexture(idx) => {
            app.props_current.texture = idx.min(app.texture_names.len() as u16);
            let p = app.props_current;
            let mask = if p.texture > 0 {
                app.brushes_root.as_deref().and_then(|root| {
                    app.texture_names
                        .get(p.texture as usize - 1)
                        .and_then(|n| mn_brush::load_texture(root, n))
                })
            } else {
                None
            };
            app.engine_mut().set_texture_mask(mask);
            app.mark_dirty();
        }
        AppCmd::SetTextureScroll(v) => {
            app.props_current.texture_scroll = v.clamp(0.0, 64.0);
            let s = app.props_current.texture_scroll;
            app.engine_mut().set_texture_scroll(s);
            app.mark_dirty();
        }
        AppCmd::SetTextureRotate(r) => {
            app.props_current.texture_rotate = r;
            app.engine_mut().set_texture_rotate(r);
            app.mark_dirty();
        }
        AppCmd::SetSketch(on) => {
            app.props_current.sketch = on;
            let p = app.props_current;
            app.engine_mut()
                .set_sketch(p.sketch.then_some(mn_brush::SketchParams {
                    distance: p.sketch_dist,
                    density: p.sketch_density,
                }));
            app.mark_dirty();
        }
        AppCmd::SetSketchDistance(v) => {
            app.props_current.sketch_dist = v.clamp(2.0, 500.0);
            let p = app.props_current;
            app.engine_mut()
                .set_sketch(p.sketch.then_some(mn_brush::SketchParams {
                    distance: p.sketch_dist,
                    density: p.sketch_density,
                }));
            app.mark_dirty();
        }
        AppCmd::SetSketchDensity(v) => {
            app.props_current.sketch_density = v.clamp(0.0, 1.0);
            let p = app.props_current;
            app.engine_mut()
                .set_sketch(p.sketch.then_some(mn_brush::SketchParams {
                    distance: p.sketch_dist,
                    density: p.sketch_density,
                }));
            app.mark_dirty();
        }
        AppCmd::SetCurve {
            setting,
            sensor,
            points,
        } => {
            let (cs, sn) = (
                CurveSetting::from_index(setting),
                CurveSensor::from_index(sensor),
            );
            let Some(sid) = cs.setting_id() else { return };
            let Some(iid) = sn.input_id() else { return };
            app.engine_mut().set_mapping(sid, iid, &points);
            // Session memory per sub tool, like ToolProps — the preset's own
            // file is never rewritten.
            if let Some(i) = app.selected_preset {
                let key = (setting, sensor);
                let entry = app
                    .curve_overrides
                    .entry(app.presets[i].1.clone())
                    .or_default();
                if points.is_empty() {
                    entry.remove(&key);
                } else {
                    entry.insert(key, points);
                }
            }
            app.mark_dirty();
        }

        AppCmd::SetMirrorX(on) => {
            app.mirror_x = on;
            if on {
                app.wrap_x = false;
            }
            app.rebuild_twins();
            app.mark_dirty();
        }
        AppCmd::SetMirrorY(on) => {
            app.mirror_y = on;
            if on {
                app.wrap_y = false;
            }
            app.rebuild_twins();
            app.mark_dirty();
        }
        AppCmd::SetWrapX(on) => {
            app.wrap_x = on;
            if on {
                app.mirror_x = false;
            }
            app.rebuild_twins();
            app.mark_dirty();
        }
        AppCmd::SetWrapY(on) => {
            app.wrap_y = on;
            if on {
                app.mirror_y = false;
            }
            app.rebuild_twins();
            app.mark_dirty();
        }
        AppCmd::SetGpuDabs(on) => {
            // A pure app preference, not document state: no mark_dirty, no
            // redraw — the next stroke's begin branch reads it live (the
            // audit-H1 function-of-the-branch rule), so flipping between
            // strokes is safe by construction.
            if on && !app.renderer.gpu_dabs_supported() {
                app.set_status("gpu dabs: this adapter can't — staying on the cpu path");
                return;
            }
            app.gpu_dabs = on;
            app.layout.note_gpu_dabs(on);
            app.set_status(if on {
                "gpu dabs: on — strokes rasterize on the gpu"
            } else {
                "gpu dabs: off — cpu dab path"
            });
        }
        AppCmd::SetMonoPreview(on) => {
            // View state, not document state: the canvas re-composites with
            // every layer forced to the 1-bit look, and nothing else — no
            // pixel changes, no export change, no dirty flag.

            app.layout.note_mono_preview(on);
            app.mark_dirty();
            app.set_status(if on {
                "monochrome preview on — display only, exports stay in colour"
            } else {
                "monochrome preview off"
            });
        }
        other => return misc::run(app, other, cmd_tail),
    }
    run_cmd_tail(app, cmd_tail);
}
