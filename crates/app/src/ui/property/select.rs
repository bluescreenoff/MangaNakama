use super::*;

/// SE-022's persistent 4-way combine mode, shared by every tool that MAKES a
/// selection. Held modifiers override it per gesture (Shift / Alt /
/// Shift+Alt). The wand went without it until 2026-08-23 — its selections
/// obeyed `sel_op` all along, but the only place to set it was a panel the
/// wand never shows.
pub(crate) fn sel_op_row(ui: &mut egui::Ui, app: &mut App) {
    egui::ComboBox::from_id_salt("mn.select.op")
        .selected_text(sel_op_label(app.sel_op))
        .show_ui(ui, |ui| {
            for op in [
                mn_core::SelectionOp::Replace,
                mn_core::SelectionOp::Add,
                mn_core::SelectionOp::Subtract,
                mn_core::SelectionOp::Intersect,
            ] {
                ui.selectable_value(&mut app.sel_op, op, sel_op_label(op));
            }
        });
    ui.weak("Shift adds · Alt subtracts · Shift+Alt intersects");
}

pub(crate) fn sec_select(ui: &mut egui::Ui, app: &mut App) {
    ui.weak("drag inside a selection to move it");
    sel_op_row(ui, app);
    ui.horizontal(|ui| {
        if ui.button("Deselect (Ctrl+D)").clicked() {
            app.push_cmd(AppCmd::Deselect);
        }
        if ui.button("Invert").clicked() {
            app.push_cmd(AppCmd::SelectInvert);
        }
    });
    // Quick Mask (SE round 2026-08-19): brushes edit the selection
    // instead of inking while this is on.
    if ui.checkbox(&mut app.quick_mask, "Quick mask").changed() {
        app.set_status(if app.quick_mask {
            "quick mask ON — pen adds to the selection, eraser subtracts"
        } else {
            "quick mask off — brushes ink again"
        });
    }
    // L-001: the magnetic lasso's one knob. Photoshop calls it Width; CSP
    // does not expose it at all, and it is the difference between tracing a
    // face (small) and a whole figure against clean paper (large).
    if app.select_mode == crate::cmd::SelectMode::Magnetic {
        ui.separator();
        let mut reach = app.magnetic_reach as f32;
        if ValueBar::new("Snap range", 4.0, 120.0)
            .step(1.0)
            .suffix(" px")
            .show(ui, &mut reach)
            .changed()
        {
            app.magnetic_reach = reach.round() as i32;
            if let Some(l) = app.magnetic.as_mut() {
                l.reach = app.magnetic_reach;
            }
        }
        ui.weak("drag along the line · Backspace undoes an anchor · Enter closes · Esc cancels");
    }
}

/// S-001 Select layer: the Exclude switches (CSP 選択しないレイヤー). These
/// are why the tool is usable on a finished page — without them the click
/// lands on the topmost tone or the text layer every time.
pub(crate) fn sec_pick_layer(ui: &mut egui::Ui, app: &mut App) {
    ui.weak("click a pixel — the Layer palette jumps to the layer that drew it");
    ui.separator();
    ui.weak("Do not select:");
    let ex = &mut app.pick_exclude;
    ui.checkbox(&mut ex.draft, "Draft layers")
        .on_hover_text("the rough underdrawing (CSP 下書き)");
    ui.checkbox(&mut ex.text, "Text layers");
    ui.checkbox(&mut ex.locked, "Locked layers");
    ui.checkbox(&mut ex.fill, "Fill / tone layers")
        .on_hover_text("live fill, gradient and tone layers — the flats that cover everything");
}

pub(crate) fn sel_op_label(op: mn_core::SelectionOp) -> &'static str {
    match op {
        mn_core::SelectionOp::Replace => "New",
        mn_core::SelectionOp::Add => "Add",
        mn_core::SelectionOp::Subtract => "Subtract",
        mn_core::SelectionOp::Intersect => "Intersect",
    }
}

pub(crate) fn sec_wand(ui: &mut egui::Ui, app: &mut App) {
    // The wand floods with the SAME `mn_core::fill` machinery the Fill tool
    // uses, off the same `FillOpts` — so every knob that machinery honours
    // belongs here too. Until 2026-08-23 the panel showed three of them and
    // the other four were set (or not) behind the artist's back.
    sel_op_row(ui, app);
    let mut o = app.wand_opts;
    let mut tol = o.tolerance * 100.0;
    let mut changed = ValueBar::new("Tolerance", 0.0, 50.0)
        .suffix("%")
        .show(ui, &mut tol)
        .changed();
    o.tolerance = tol / 100.0;
    changed |= auto_gap_block(ui, "mn.wand.expand", &mut o, None);
    changed |= refer_block(ui, "mn.wand.refer", &mut o);
    if changed {
        app.push_cmd(AppCmd::SetWandOpts(o));
    }
}

/// The "Auto gap & fringe" switch and the two rows it drives: dialled by hand
/// when off, measured from the lineart when on. `measured` is the last
/// measurement to read back, where the tool keeps one.
fn auto_gap_block(
    ui: &mut egui::Ui,
    salt: &str,
    o: &mut mn_core::FillOpts,
    measured: Option<mn_core::AutoFill>,
) -> bool {
    let mut changed = ui
        .checkbox(&mut o.auto, "Auto gap & fringe")
        .on_hover_text(
            "measure the lineart's own thickness at each click instead of dialling \
             gap closing and area scaling by hand",
        )
        .changed();
    if o.auto {
        match measured {
            Some(a) => {
                ui.weak(format!("Close gap: {} px — measured", a.gap_close_px));
                ui.weak(format!("Area scaling: {:+} px — measured", a.expand_px));
                ui.weak(format!("lines read ~{:.0} px thick", a.line_px));
            }
            None => {
                ui.weak("Close gap and area scaling: measured at the next click");
            }
        }
    } else {
        let mut gap = o.gap_close_px as f32;
        changed |= ValueBar::new("Close gap", 0.0, 8.0)
            .step(1.0)
            .suffix(" px")
            .show(ui, &mut gap)
            .changed();
        o.gap_close_px = gap as u32;
        changed |= area_scaling_row(ui, salt, o);
    }
    // Row 40/120: CSP 半透明を透明にする — the antialiased skirt is
    // fillable, the flat runs under the fringe to the line's dark core,
    // and the 1 px halo dies.
    changed |= ui
        .checkbox(
            &mut o.semi_transparent_paper,
            "Semi-transparent is fillable",
        )
        .on_hover_text(
            "treat the antialiased fringe of the lineart as paper — the fill \
             runs under it to the dark core and no light halo survives \
             against the flat",
        )
        .changed();
    changed
}

/// CSP's 参照 block: what the flood samples, whether draft layers count, and
/// whether the page rim walls it in. Shared by Fill and Auto select — one
/// `FillOpts`, one set of rows.
fn refer_block(ui: &mut egui::Ui, salt: &str, o: &mut mn_core::FillOpts) -> bool {
    let label = |v: mn_core::FillRefer| match v {
        mn_core::FillRefer::All => "Refer: all layers",
        mn_core::FillRefer::Active => "Refer: editing layer",
        mn_core::FillRefer::Reference => "Refer: reference layer",
    };
    let mut pick: Option<mn_core::FillRefer> = None;
    egui::ComboBox::from_id_salt(salt)
        .width(ui.available_width() - 8.0)
        .selected_text(label(o.refer))
        .show_ui(ui, |ui| {
            for v in [
                mn_core::FillRefer::All,
                mn_core::FillRefer::Active,
                mn_core::FillRefer::Reference,
            ] {
                if ui.selectable_label(o.refer == v, label(v)).clicked() {
                    pick = Some(v);
                }
            }
        });
    let mut changed = false;
    if let Some(v) = pick {
        o.refer = v;
        changed = true;
    }
    changed |= ui
        .checkbox(&mut o.refer_drafts, "Refer draft layers")
        .changed();
    // FI-022: the page's own perimeter joins the lineart as a wall.
    changed |= ui
        .checkbox(&mut o.refer_border, "Refer to image border")
        .on_hover_text("the page's outer edge counts as a drawn line, so a fill cannot get out")
        .changed();
    changed
}

/// FI-016's row, shared by the Fill, wand and Tone Tool Properties: CSP's
/// SIGNED area scaling, plus P0-4's 拡縮方法 — the SHAPE it scales in.
/// Positive tucks the region under the lineart, negative pulls it back off
/// the line. `salt` keeps the three panels' combos from sharing an egui id.
pub(crate) fn area_scaling_row(ui: &mut egui::Ui, salt: &str, o: &mut mn_core::FillOpts) -> bool {
    use mn_core::fill::ExpandMode;
    let mut exp = o.expand_px as f32;
    let mut changed = ValueBar::new("Area scaling", -4.0, 4.0)
        .step(1.0)
        .suffix(" px")
        .show(ui, &mut exp)
        .on_hover_text("positive overfills under the lineart, negative underfills inside the area")
        .changed();
    // `as i32` truncates toward zero; the slider steps whole pixels, so
    // round first or -1 arrives as 0 on the way past.
    o.expand_px = exp.round() as i32;
    let label = |m: ExpandMode| match m {
        ExpandMode::Rect => "Square",
        ExpandMode::Round => "Round",
        ExpandMode::ToDarkest => "To darkest pixel",
    };
    let mut pick: Option<ExpandMode> = None;
    egui::ComboBox::from_id_salt(salt)
        .width(ui.available_width() - 8.0)
        .selected_text(label(o.expand_mode))
        .show_ui(ui, |ui| {
            for m in [ExpandMode::Rect, ExpandMode::Round, ExpandMode::ToDarkest] {
                if ui.selectable_label(o.expand_mode == m, label(m)).clicked() {
                    pick = Some(m);
                }
            }
        });
    ui.weak(match o.expand_mode {
        ExpandMode::Rect => "grows the same distance in every direction, corners included",
        ExpandMode::Round => "rounds the corners off — a disc, not a square",
        ExpandMode::ToDarkest => "grows to the darkest pixel of the line and stops there",
    });
    if let Some(m) = pick {
        o.expand_mode = m;
        changed = true;
    }
    changed
}

pub(crate) fn sec_wand_guide(ui: &mut egui::Ui, _app: &mut App) {
    ui.weak("click an area to select it — G fills, Delete clears");
}

pub(crate) fn sec_fill(ui: &mut egui::Ui, app: &mut App) {
    // FI-004 fills the drawn shape itself: tolerance, gap closing, area
    // scaling and 参照 all describe a FLOOD, and it does not run one.
    if app.fill_mode == crate::cmd::FillMode::Lasso {
        ui.weak("drag a shape and it is painted as drawn — lines are ignored");
        ui.weak("combines with an active selection like any fill");
        return;
    }
    if app.fill_mode == crate::cmd::FillMode::Enclose {
        ui.weak("drag right around the areas to fill — everything closed inside goes");
    }
    let mut o = app.fill_opts;
    let mut tol = o.tolerance * 100.0;
    let mut changed = ValueBar::new("Tolerance", 0.0, 50.0)
        .suffix("%")
        .show(ui, &mut tol)
        .changed();
    o.tolerance = tol / 100.0;
    // ROADMAP: the fill that measures gap and fringe itself. Opt-in — the
    // two rows it drives go read-only underneath, showing what the last
    // fill actually measured, so the numbers stay learnable.
    changed |= auto_gap_block(ui, "mn.fill.expand", &mut o, app.fill_auto);
    changed |= refer_block(ui, "mn.fill.refer", &mut o);
    // NL-006's switch (TRIAGE 137): fill a LIVE layer instead of painting.
    // Enclose-and-fill paints pockets, not a window — the live model has no
    // shape for it, so the switch stays with the click sub tool.
    if app.fill_mode == crate::cmd::FillMode::Click {
        ui.checkbox(&mut app.fill_live, "Create live layer");
    }
    if changed {
        app.push_cmd(AppCmd::SetFillOpts(o));
    }
}

pub(crate) fn sec_eyedrop(ui: &mut egui::Ui, app: &mut App) {
    ui.weak(match app.eyedrop_opts.refer {
        mn_core::FillRefer::All => "click picks the colour you see (all layers)",
        mn_core::FillRefer::Active => "click picks the active layer's own colour",
        mn_core::FillRefer::Reference => "click picks the reference layers only",
    });
    if app.eyedrop_opts.refer == mn_core::FillRefer::Reference
        && app.doc.reference_layers().is_empty()
    {
        ui.weak("no reference layer marked — picks fall back to what you see");
    }
    let n = app.eyedrop_opts.size;
    if n > 1 {
        ui.weak(format!(
            "averaged over {n} × {n} px, in linear light — the colour the area reads as"
        ));
    }
    ui.checkbox(&mut app.eyedrop_opts.circle, "Show color picker circle")
        .on_hover_text("the ring under the pen: what a click would take, over the current colour");
    ui.weak("Alt+click does this from any drawing tool");
}

pub(crate) fn sec_pan(ui: &mut egui::Ui, app: &mut App) {
    ui.weak(match app.pan_mode {
        crate::cmd::PanMode::Hand => "drag to pan; space does this from any tool",
        crate::cmd::PanMode::Rotate => "drag to spin the view; R steps 15°",
    });
}

// --- text sections (Text tool AND Operation + text selection) ------------
