use super::*;

pub(crate) fn sec_select(ui: &mut egui::Ui, app: &mut App) {
    ui.weak("drag inside a selection to move it");
    // SE-022: the persistent 4-way combine mode. Held modifiers override
    // it per gesture (Shift / Alt / Shift+Alt).
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
    let mut o = app.wand_opts;
    let mut tol = o.tolerance * 100.0;
    let mut changed = ValueBar::new("Tolerance", 0.0, 50.0)
        .suffix("%")
        .show(ui, &mut tol)
        .changed();
    o.tolerance = tol / 100.0;
    let mut gap = o.gap_close_px as f32;
    changed |= ValueBar::new("Close gap", 0.0, 8.0)
        .step(1.0)
        .suffix(" px")
        .show(ui, &mut gap)
        .changed();
    o.gap_close_px = gap as u32;
    changed |= area_scaling_row(ui, &mut o);
    if changed {
        app.push_cmd(AppCmd::SetWandOpts(o));
    }
}

/// FI-016's row, shared by the Fill and wand Tool Properties: CSP's SIGNED
/// area scaling. Positive tucks the region under the lineart, negative
/// pulls it back off the line.
pub(crate) fn area_scaling_row(ui: &mut egui::Ui, o: &mut mn_core::FillOpts) -> bool {
    let mut exp = o.expand_px as f32;
    let changed = ValueBar::new("Area scaling", -4.0, 4.0)
        .step(1.0)
        .suffix(" px")
        .show(ui, &mut exp)
        .on_hover_text("positive overfills under the lineart, negative underfills inside the area")
        .changed();
    // `as i32` truncates toward zero; the slider steps whole pixels, so
    // round first or -1 arrives as 0 on the way past.
    o.expand_px = exp.round() as i32;
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
    let mut gap = o.gap_close_px as f32;
    changed |= ValueBar::new("Close gap", 0.0, 8.0)
        .step(1.0)
        .suffix(" px")
        .show(ui, &mut gap)
        .changed();
    o.gap_close_px = gap as u32;
    changed |= area_scaling_row(ui, &mut o);
    // CSP's fill 参照 block: what the flood samples, and whether draft
    // layers count.
    let mut pick: Option<mn_core::FillRefer> = None;
    egui::ComboBox::from_id_salt("mn.fill.refer")
        .width(ui.available_width() - 8.0)
        .selected_text(match o.refer {
            mn_core::FillRefer::All => "Refer: all layers",
            mn_core::FillRefer::Active => "Refer: editing layer",
            mn_core::FillRefer::Reference => "Refer: reference layer",
        })
        .show_ui(ui, |ui| {
            for (v, label) in [
                (mn_core::FillRefer::All, "Refer: all layers"),
                (mn_core::FillRefer::Active, "Refer: editing layer"),
                (mn_core::FillRefer::Reference, "Refer: reference layer"),
            ] {
                if ui.selectable_label(o.refer == v, label).clicked() {
                    pick = Some(v);
                }
            }
        });
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
