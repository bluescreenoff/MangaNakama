use super::*;

/// Height of the colour bar itself; the handle gutter sits under it.
pub(crate) const BAR_H: f32 = 22.0;
/// How near (px) the pointer must be to grab a stop instead of adding one.
pub(crate) const BAR_GRAB: f32 = 7.0;

/// The two END colours the Gradient TOOL will paint with, given its sub
/// tool mode. Straight RGBA — the alpha is what the transparent modes vary.
pub(crate) fn tool_ends(app: &App) -> ([f32; 4], [f32; 4]) {
    let fg = app.active_color();
    let bg = app.sub_color;
    match app.grad_mode {
        crate::cmd::GradMode::FgToBg => ([fg[0], fg[1], fg[2], 1.0], [bg[0], bg[1], bg[2], 1.0]),
        crate::cmd::GradMode::FgToTransparent => {
            ([fg[0], fg[1], fg[2], 1.0], [fg[0], fg[1], fg[2], 0.0])
        }
        crate::cmd::GradMode::TransparentToFg => {
            ([fg[0], fg[1], fg[2], 0.0], [fg[0], fg[1], fg[2], 1.0])
        }
    }
}

pub(crate) fn col32(c: [f32; 4]) -> egui::Color32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    egui::Color32::from_rgba_unmultiplied(q(c[0]), q(c[1]), q(c[2]), q(c[3]))
}

/// The colour bar: the ramp as it will be painted, with a draggable handle
/// per interior stop. Click empty bar to add a stop there, drag a handle to
/// move it, drag it down out of the gutter to delete it. Returns whether
/// the ramp changed.
pub(crate) fn ramp_bar(
    ui: &mut egui::Ui,
    from: [f32; 4],
    to: [f32; 4],
    mid: &mut mn_core::MidStops,
    opts: &mn_core::RampOpts,
    sel: &mut Option<usize>,
    dragging: &mut bool,
) -> bool {
    let w = ui.available_width().max(60.0);
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(w, BAR_H + 11.0), egui::Sense::click_and_drag());
    let bar = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), BAR_H));
    let p = ui.painter().clone();

    // Checkerboard first: per-stop OPACITY (`G-013`) is invisible over a
    // flat panel colour, and opacity is half of what a manga fade is.
    let sq = 5.0;
    let mut y = bar.top();
    let mut row = 0;
    while y < bar.bottom() {
        let mut x = bar.left();
        let mut col = row % 2;
        while x < bar.right() {
            let cell = egui::Rect::from_min_max(
                egui::pos2(x, y),
                egui::pos2((x + sq).min(bar.right()), (y + sq).min(bar.bottom())),
            );
            p.rect_filled(
                cell,
                0.0,
                if col == 0 {
                    egui::Color32::from_gray(0x4a)
                } else {
                    egui::Color32::from_gray(0x33)
                },
            );
            x += sq;
            col ^= 1;
        }
        y += sq;
        row += 1;
    }

    let ramp = mn_core::Ramp::new(from, to, *mid, *opts);
    let n = bar.width().round().max(1.0) as i32;
    for i in 0..n {
        let t = (i as f32 + 0.5) / n as f32;
        let x = bar.left() + i as f32;
        p.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, bar.top()), egui::vec2(1.0, BAR_H)),
            0.0,
            col32(ramp.color_at(t)),
        );
    }
    p.rect_stroke(
        bar,
        0.0,
        egui::Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );

    let handle_x = |pos: f32| bar.left() + pos.clamp(0.0, 1.0) * bar.width();
    for (i, s) in mid.as_slice().iter().enumerate() {
        let x = handle_x(s.pos);
        let cy = bar.bottom() + 5.0;
        let picked = *sel == Some(i);
        p.circle_filled(egui::pos2(x, cy), 4.5, col32(s.color));
        p.circle_stroke(
            egui::pos2(x, cy),
            4.5,
            egui::Stroke::new(
                if picked { 2.0 } else { 1.0 },
                if picked {
                    theme::ACCENT
                } else {
                    theme::OUTLINE
                },
            ),
        );
    }

    let mut changed = false;
    if let Some(ptr) = resp.interact_pointer_pos() {
        let at = ((ptr.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
        if resp.drag_started() || resp.clicked() {
            let hit = mid
                .as_slice()
                .iter()
                .enumerate()
                .map(|(i, s)| (i, ((s.pos - at) * bar.width()).abs()))
                .filter(|(_, d)| *d <= BAR_GRAB)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i);
            match hit {
                Some(i) => *sel = Some(i),
                None => match mid.insert(mn_core::GradStop {
                    pos: at,
                    color: ramp.color_at(at),
                }) {
                    Some(i) => {
                        *sel = Some(i);
                        changed = true;
                    }
                    // Silently refusing would look like a dead click.
                    None => *sel = None,
                },
            }
            *dragging = resp.drag_started();
        }
        if *dragging && resp.dragged() {
            if let Some(i) = *sel {
                // CSP's delete gesture: drag the stop off the bar downwards.
                if ptr.y > rect.bottom() + 12.0 {
                    mid.remove(i);
                    *sel = None;
                    *dragging = false;
                    changed = true;
                } else if let Some(s) = mid.get_mut(i)
                    && s.pos != at
                {
                    s.pos = at;
                    changed = true;
                }
            }
        }
    }
    if resp.drag_stopped() && *dragging {
        *dragging = false;
        // Re-sort only once the gesture ends: a stop dragged past its
        // neighbour must not renumber itself out from under the drag.
        // Counted as a change so a live layer stores the sorted order —
        // otherwise `sel` would index an array the caller never kept.
        if let Some(i) = *sel {
            *sel = Some(mid.resort(i));
            changed = true;
        }
    }
    if mid.is_full() {
        // Said always, not only on the refused click: a line that appears
        // for one frame is a layout jump, and the cap is worth knowing
        // BEFORE you go looking for the stop that did not appear.
        ui.weak(format!(
            "{} stops is the maximum",
            mn_core::gradient::MAX_MID
        ));
    }
    changed
}

/// The selected stop's numeric fields: position, opacity and colour, plus
/// the Main/Sub colour sources (`G-013`/`G-014`).
pub(crate) fn ramp_stop_fields(
    ui: &mut egui::Ui,
    mid: &mut mn_core::MidStops,
    sel: &mut Option<usize>,
    main: [f32; 3],
    sub: [f32; 3],
) -> bool {
    let Some(i) = (*sel).filter(|i| *i < mid.len()) else {
        ui.weak("click the bar to add a stop, drag one down to delete it");
        return false;
    };
    let mut changed = false;
    let mut s = *mid.get(i).expect("index checked above");
    let mut pos = s.pos * 100.0;
    if ValueBar::new("Position", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut pos)
        .changed()
    {
        s.pos = pos / 100.0;
        changed = true;
    }
    let mut op = s.color[3] * 100.0;
    if ValueBar::new("Opacity", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut op)
        .changed()
    {
        s.color[3] = op / 100.0;
        changed = true;
    }
    let mut delete = false;
    ui.horizontal(|ui| {
        let mut srgb = [
            (s.color[0].clamp(0.0, 1.0) * 255.0) as u8,
            (s.color[1].clamp(0.0, 1.0) * 255.0) as u8,
            (s.color[2].clamp(0.0, 1.0) * 255.0) as u8,
        ];
        if ui.color_edit_button_srgb(&mut srgb).changed() {
            for k in 0..3 {
                s.color[k] = srgb[k] as f32 / 255.0;
            }
            changed = true;
        }
        // `G-014`'s colour SOURCE, stamped rather than linked: the stop
        // takes the main/sub colour as it is now and keeps it. A live link
        // would make every palette click silently repaint old gradients.
        for (label, c) in [("Main", main), ("Sub", sub)] {
            if ui.small_button(label).clicked() {
                s.color[0] = c[0];
                s.color[1] = c[1];
                s.color[2] = c[2];
                changed = true;
            }
        }
        delete = ui.small_button("Delete").clicked();
    });
    if delete {
        mid.remove(i);
        *sel = None;
        return true;
    }
    if changed && let Some(d) = mid.get_mut(i) {
        *d = s;
    }
    changed
}

/// Edge process, flip, dithering, start-from-centre, mixing mode and
/// mixing rate — `G-002`/`G-004`/`G-005`/`G-006`/`G-009`/`G-015`.
pub(crate) fn ramp_options(ui: &mut egui::Ui, opts: &mut mn_core::RampOpts, salt: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.weak("Edge");
        egui::ComboBox::from_id_salt(format!("mn.grad.edge.{salt}"))
            .selected_text(opts.edge.label())
            .show_ui(ui, |ui| {
                for e in mn_core::EdgeProcess::ALL {
                    changed |= ui.selectable_value(&mut opts.edge, e, e.label()).changed();
                }
            });
    })
    .response
    .on_hover_text("what the ramp does OUTSIDE the length you dragged");
    ui.horizontal(|ui| {
        ui.weak("Mixing");
        egui::ComboBox::from_id_salt(format!("mn.grad.mix.{salt}"))
            .selected_text(opts.mix.label())
            .show_ui(ui, |ui| {
                for m in mn_core::MixMode::ALL {
                    changed |= ui.selectable_value(&mut opts.mix, m, m.label()).changed();
                }
            });
    })
    .response
    .on_hover_text("Perceptual keeps a ramp's lightness even across the middle");
    // `G-010`. Shown only where it does something — CSP greys it out in the
    // other modes, and a control that is always dead is worse than absent.
    if opts.mix == mn_core::MixMode::Perceptual {
        let mut lv = opts.bright as f32;
        if ValueBar::new("Brightness", 0.0, mn_core::gradient::MAX_BRIGHT as f32)
            .step(1.0)
            .show(ui, &mut lv)
            .changed()
        {
            opts.bright = lv.round() as u8;
            changed = true;
        }
    }
    changed |= ui
        .checkbox(&mut opts.flip, "Flip")
        .on_hover_text("paint the ramp end-first, without re-dragging it")
        .changed();
    changed |= ui
        .checkbox(&mut opts.dither, "Dithering")
        .on_hover_text("ordered noise inside the ramp so print does not band")
        .changed();
    changed |= ui
        .checkbox(&mut opts.from_center, "Start from centre")
        .on_hover_text("the drag START is the middle of the ramp")
        .changed();
    let mut rate = opts.curve * 100.0;
    if ValueBar::new("Mixing rate", -100.0, 100.0)
        .suffix("%")
        .show(ui, &mut rate)
        .changed()
    {
        opts.curve = rate / 100.0;
        changed = true;
    }
    changed
}

pub(crate) fn sec_gradient_info(ui: &mut egui::Ui, app: &mut App) {
    ui.weak(format!(
        "{} (set in the Sub Tool list)",
        app.grad_mode.label()
    ));
    let (from, to) = tool_ends(app);
    let mut mid = app.grad_mid;
    let mut sel = app.grad_stop_sel;
    let mut dragging = app.grad_stop_drag;
    // The return value only matters to a LIVE layer, which has to push a
    // re-derive; the tool's ramp is plain app state, written back every
    // frame.
    ramp_bar(
        ui,
        from,
        to,
        &mut mid,
        &app.grad_opts,
        &mut sel,
        &mut dragging,
    );
    let (main, sub) = (app.active_color(), app.sub_color);
    ramp_stop_fields(ui, &mut mid, &mut sel, main, sub);
    app.grad_mid = mid;
    app.grad_stop_sel = sel;
    app.grad_stop_drag = dragging;
    // I-016 (CSP "Where to create") / NL-006's live switch: bake the ramp
    // into the layer's pixels, or spawn a gradient LAYER whose parameters
    // stay editable.
    ui.checkbox(&mut app.fill_live, "Create live layer")
        .on_hover_text("off: paint pixels · on: a gradient layer you can re-drag later");
}

pub(crate) fn sec_gradient_opts(ui: &mut egui::Ui, app: &mut App) {
    let mut opts = app.grad_opts;
    if ramp_options(ui, &mut opts, "tool") {
        app.grad_opts = opts;
    }
}

// --- the gradient SET (`G-011`/`G-012`/`G-016`) --------------------------

/// A non-interactive strip of a ramp — the set's row preview, and the only
/// difference from `ramp_bar` is that nothing here can be grabbed.
pub(crate) fn ramp_preview(ui: &mut egui::Ui, ramp: &mn_core::Ramp, h: f32) {
    let w = ui.available_width().max(40.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let p = ui.painter();
    let n = rect.width().round().max(1.0) as i32;
    for i in 0..n {
        let t = (i as f32 + 0.5) / n as f32;
        p.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + i as f32, rect.top()),
                egui::vec2(1.0, h),
            ),
            0.0,
            col32(ramp.color_at(t)),
        );
    }
    p.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );
}

/// Persist the set — every list op calls this, so a crash between two edits
/// cannot lose a gradient the user registered ten minutes ago.
pub(crate) fn grad_set_save(app: &mut App) {
    let json = app.grad_set.to_json();
    app.layout.note_gradients(&json);
}

/// The ramp the Gradient TOOL would paint with right now, as a saveable
/// entry (the ends resolved from the sub tool mode).
pub(crate) fn tool_named_ramp(app: &App, name: String) -> mn_core::NamedRamp {
    let (from, to) = tool_ends(app);
    mn_core::NamedRamp {
        name,
        from,
        to,
        mid: app.grad_mid,
        opts: app.grad_opts,
    }
}

/// Apply a saved gradient. On a live gradient LAYER that is the whole ramp,
/// ends included — the layer owns its colours. On the TOOL the ends are the
/// palette's, so applying REWRITES main/sub and picks the sub tool mode
/// that reproduces the preset; otherwise "apply" would silently drop half
/// of what was saved. Documented in the manual as the quirk it is.
pub(crate) fn apply_named_ramp(app: &mut App, g: mn_core::NamedRamp) {
    let li = app.doc.active;
    // Read the ramp line out FIRST: the layer borrow has to be finished
    // before `push_cmd` can take `app` mutably.
    let line = match app.doc.layers.get(li).map(|l| &l.kind) {
        Some(mn_core::LayerKind::Fill(mn_core::FillKind::Gradient { a, b, .. })) => Some((*a, *b)),
        _ => None,
    };
    if let Some((a, b)) = line {
        let kind = mn_core::FillKind::Gradient {
            a,
            b,
            from: g.from,
            to: g.to,
            mid: g.mid,
            opts: g.opts,
        };
        app.push_cmd(AppCmd::SetFillParams(li, kind));
        app.set_status(format!("“{}” applied to the gradient layer", g.name));
        return;
    }
    app.grad_mid = g.mid;
    app.grad_opts = g.opts;
    let opaque = |c: [f32; 4]| c[3] > 0.5;
    let rgb = |c: [f32; 4]| [c[0], c[1], c[2]];
    match (opaque(g.from), opaque(g.to)) {
        (true, false) => {
            app.grad_mode = crate::cmd::GradMode::FgToTransparent;
            app.main_color = rgb(g.from);
        }
        (false, true) => {
            app.grad_mode = crate::cmd::GradMode::TransparentToFg;
            app.main_color = rgb(g.to);
        }
        _ => {
            app.grad_mode = crate::cmd::GradMode::FgToBg;
            app.main_color = rgb(g.from);
            app.sub_color = rgb(g.to);
        }
    }
    app.apply_draw_state();
    app.set_status(format!("“{}” applied — main/sub now its ends", g.name));
}

pub(crate) fn sec_gradient_set(ui: &mut egui::Ui, app: &mut App) {
    if app.grad_set.is_empty() {
        ui.weak("no saved gradients");
    } else {
        let sel = app.grad_set_sel.min(app.grad_set.len() - 1);
        app.grad_set_sel = sel;
        let names: Vec<String> = app.grad_set.items.iter().map(|g| g.name.clone()).collect();
        egui::ComboBox::from_id_salt("mn.grad.set")
            .width(ui.available_width())
            .selected_text(names[sel].clone())
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut app.grad_set_sel, i, n);
                }
            });
        let sel = app.grad_set_sel;
        ramp_preview(ui, &app.grad_set.items[sel].ramp(), 14.0);
        // Rename in place: a set of six "Gradient 4"s is a set you cannot
        // use, and there is nowhere else in the UI a name could be edited.
        let mut name = app.grad_set.items[sel].name.clone();
        if ui
            .add(egui::TextEdit::singleline(&mut name).desired_width(ui.available_width()))
            .changed()
        {
            app.grad_set.items[sel].name = name;
            grad_set_save(app);
        }
        ui.horizontal(|ui| {
            if ui
                .button("Apply")
                .on_hover_text("load this ramp into the tool (or the active gradient layer)")
                .clicked()
            {
                let g = app.grad_set.items[sel].clone();
                apply_named_ramp(app, g);
            }
            if ui
                .small_button("Replace")
                .on_hover_text("overwrite this entry with the ramp as it is now")
                .clicked()
            {
                let keep = app.grad_set.items[sel].name.clone();
                app.grad_set.items[sel] = tool_named_ramp(app, keep);
                grad_set_save(app);
            }
            if ui.small_button("Copy").clicked() {
                app.grad_set_sel = app.grad_set.duplicate(sel).unwrap_or(sel);
                grad_set_save(app);
            }
        });
        ui.horizontal(|ui| {
            if ui.small_button("Up").clicked() {
                app.grad_set_sel = app.grad_set.move_by(sel, -1);
                grad_set_save(app);
            }
            if ui.small_button("Down").clicked() {
                app.grad_set_sel = app.grad_set.move_by(sel, 1);
                grad_set_save(app);
            }
            if ui.small_button("Delete").clicked() {
                app.grad_set.items.remove(sel);
                app.grad_set_sel = sel.saturating_sub(1);
                grad_set_save(app);
            }
        });
    }
    ui.horizontal(|ui| {
        if ui
            .button("Add")
            .on_hover_text("save the ramp as it is now as a new entry")
            .clicked()
        {
            let name = app.grad_set.free_name("Gradient");
            let g = tool_named_ramp(app, name);
            app.grad_set.items.push(g);
            app.grad_set_sel = app.grad_set.len() - 1;
            grad_set_save(app);
        }
        if ui
            .small_button("Import…")
            .on_hover_text("a GIMP .ggr gradient file")
            .clicked()
        {
            app.push_cmd(AppCmd::ImportGradient);
        }
    });
}

/// The ACTIVE LIVE LAYER's parameters (TRIAGE 137): edit the fill a week
/// later — colour, ramp endpoints, or tone density/pattern/frequency/angle.
/// Every change is a re-derive, never a repaint.
pub(crate) fn sec_live_fill(ui: &mut egui::Ui, app: &mut App) {
    let li = app.doc.active;
    let Some(mn_core::LayerKind::Fill(k)) = app.doc.layers.get(li).map(|l| &l.kind) else {
        return;
    };
    let mut kind = *k;
    let mut changed = false;
    // Read out before the borrow of `kind` starts; written back after.
    let (main, sub) = (app.active_color(), app.sub_color);
    let mut live_sel = app.grad_live_sel;
    let mut live_drag = app.grad_live_drag;
    let mut rgb = |ui: &mut egui::Ui, c: &mut [f32; 4]| {
        let mut srgb = [
            (c[0].clamp(0.0, 1.0) * 255.0) as u8,
            (c[1].clamp(0.0, 1.0) * 255.0) as u8,
            (c[2].clamp(0.0, 1.0) * 255.0) as u8,
        ];
        if ui.color_edit_button_srgb(&mut srgb).changed() {
            c[0] = srgb[0] as f32 / 255.0;
            c[1] = srgb[1] as f32 / 255.0;
            c[2] = srgb[2] as f32 / 255.0;
            changed = true;
        }
    };
    match &mut kind {
        mn_core::FillKind::Flat { color } => {
            ui.horizontal(|ui| {
                ui.weak("colour");
                rgb(ui, color);
            });
        }
        mn_core::FillKind::Gradient {
            a,
            b,
            from,
            to,
            mid,
            opts,
        } => {
            ui.horizontal(|ui| {
                ui.weak("from");
                rgb(ui, from);
                ui.weak("to");
                rgb(ui, to);
            });
            // I-016's whole point: a gradient LAYER's ramp is editable a
            // week later, stops and options included — the same editor the
            // tool uses, so nothing is authorable in one place only.
            let mut sel = live_sel;
            let mut dragging = live_drag;
            changed |= ramp_bar(ui, *from, *to, mid, opts, &mut sel, &mut dragging);
            changed |= ramp_stop_fields(ui, mid, &mut sel, main, sub);
            changed |= ramp_options(ui, opts, "live");
            live_sel = sel;
            live_drag = dragging;
            ui.weak(format!(
                "ramp ({:.0},{:.0}) → ({:.0},{:.0}) — drag the Gradient tool to re-set",
                a[0], a[1], b[0], b[1]
            ));
        }
        mn_core::FillKind::Tone { tone, density } => {
            let mut d = *density;
            changed |= ValueBar::new("Density", 0.0, 1.0)
                .show(ui, &mut d)
                .changed();
            *density = d;
            let mut lpi = tone.lpi;
            changed |= ValueBar::new("Frequency", 5.0, 80.0)
                .suffix(" lpi")
                .show(ui, &mut lpi)
                .changed();
            tone.lpi = lpi;
            let mut ang = tone.angle_deg;
            changed |= ValueBar::new("Angle", 0.0, 90.0)
                .suffix("°")
                .show(ui, &mut ang)
                .changed();
            tone.angle_deg = ang;
            ui.horizontal(|ui| {
                ui.weak("pattern");
                egui::ComboBox::from_id_salt("mn.fill.tone.pattern")
                    .width(96.0)
                    .selected_text(tone.pattern.label())
                    .show_ui(ui, |ui| {
                        for pat in mn_core::TonePattern::ALL {
                            if ui
                                .selectable_label(tone.pattern == pat, pat.label())
                                .clicked()
                            {
                                tone.pattern = pat;
                                changed = true;
                            }
                        }
                    });
            });
            // LP-014 / TN-009: the lattice origin, so two live tone layers
            // at the same frequency and angle can be nudged out of moiré.
            // (Posterization is deliberately absent here — CSP does not show
            // it on Fill layers, and a flat density has no ramp to quantize.)
            let mut ox = tone.offset[0];
            changed |= ValueBar::new("Dot position X", -32.0, 32.0)
                .decimals(1)
                .suffix(" px")
                .show(ui, &mut ox)
                .changed();
            tone.offset[0] = ox;
            let mut oy = tone.offset[1];
            changed |= ValueBar::new("Dot position Y", -32.0, 32.0)
                .decimals(1)
                .suffix(" px")
                .show(ui, &mut oy)
                .changed();
            tone.offset[1] = oy;
        }
    }
    ui.weak("any brush edits the window mask");
    app.grad_live_sel = live_sel;
    app.grad_live_drag = live_drag;
    if changed {
        app.push_cmd(AppCmd::SetFillParams(li, kind));
    }
}

pub(crate) fn sec_gradient_guide(ui: &mut egui::Ui, app: &mut App) {
    ui.weak("drag the ramp line — colour follows it");
    ui.weak("the ramp spans the whole canvas (or selection) perpendicular to the line");
    ui.weak(match app.grad_opts.edge {
        mn_core::EdgeProcess::Clamp => "outside the drag: the end colour holds",
        mn_core::EdgeProcess::Repeat => "outside the drag: the ramp tiles",
        mn_core::EdgeProcess::Reverse => "outside the drag: the ramp ping-pongs",
        mn_core::EdgeProcess::Blank => "outside the drag: nothing is painted",
    });
    if app.grad_opts.flip || app.grad_opts.from_center {
        ui.weak("Flip and Start from centre change the DRAG, not the bar above");
    }
}
