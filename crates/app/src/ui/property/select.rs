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
    // C-005: 対象色, the Target-colour dropdown — which pixel classes the
    // flood treats as fillable; everything else walls it.
    let mut close = o.close;
    ui.horizontal(|ui| {
        ui.weak("Target colour");
        egui::ComboBox::from_id_salt(format!("{salt}.close"))
            .selected_text(close_label(close))
            .width(170.0)
            .show_ui(ui, |ui| {
                for v in [
                    mn_core::FillClose::AllColours,
                    mn_core::FillClose::OnlyTransparent,
                    mn_core::FillClose::NotTransparent,
                    mn_core::FillClose::OnlyBlack,
                    mn_core::FillClose::NotBlack,
                    mn_core::FillClose::WhiteAndTransparent,
                    mn_core::FillClose::NotWhiteAndTransparent,
                ] {
                    ui.selectable_value(&mut close, v, close_label(v));
                }
            })
            .response
            .on_hover_text(
                "what counts as fillable: only the chosen class of pixels \
                 fills, everything else is a wall (all colours = the \
                 tolerance decides, as always)",
            );
    });
    changed |= close != o.close;
    o.close = close;
    changed
}

/// C-005 labels. "and transparent" spelled out — the pairing is the
/// feature (white-on-white lineart work needs the transparent half).
fn close_label(v: mn_core::FillClose) -> &'static str {
    match v {
        mn_core::FillClose::AllColours => "All colours",
        mn_core::FillClose::OnlyTransparent => "Only transparent",
        mn_core::FillClose::NotTransparent => "Other than transparent",
        mn_core::FillClose::OnlyBlack => "Only black",
        mn_core::FillClose::NotBlack => "Other than black",
        mn_core::FillClose::WhiteAndTransparent => "White and transparent",
        mn_core::FillClose::NotWhiteAndTransparent => "Other than white and transparent",
    }
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
    // Row 160 / RD-002/RD-003/RD-007. Nothing below this block applies:
    // dust removal runs no flood, so tolerance / gap closing / area scaling
    // / 参照 would all be knobs that do nothing.
    if app.fill_mode == crate::cmd::FillMode::Dust {
        sec_dust(ui, app);
        return;
    }
    if app.fill_mode == crate::cmd::FillMode::Enclose {
        ui.weak("drag right around the areas to fill — everything closed inside goes");
    }
    // Row 119: the leftover pen runs the same flood, so every knob below
    // means what it means under the bucket — only the seeds and the mask
    // differ, and both are decided by what is already painted.
    if app.fill_mode == crate::cmd::FillMode::Leftover {
        ui.weak("scrub across the flat — only enclosed spots still empty fill");
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
    // This is the BUCKET's switch alone (default off); the Gradient tool
    // carries its own `gradient_live`, which ships on.
    if app.fill_mode == crate::cmd::FillMode::Click {
        ui.checkbox(&mut app.fill_live, "Create live layer");
    }
    if changed {
        app.push_cmd(AppCmd::SetFillOpts(o));
    }
    // Leak repair's door where the eyes already are (owner UX call
    // 2026-08-31): a fill that leaked is fixed from HERE, one tap — the
    // palette rows stay for keys.json. Greyed with the arm's own refusal
    // as the hover, so the button can never lie about why.
    ui.add_space(4.0);
    if let Some(r) = &app.fill_repair {
        let kind = if r.virtual_barrier {
            "virtual barrier"
        } else {
            "real ink"
        };
        ui.horizontal(|ui| {
            ui.weak(format!("repair armed ({kind}) — draw the closing stroke"));
            if ui.small_button("Cancel").clicked() {
                app.cancel_fill_repair();
            }
        });
    } else {
        let ok = app.fill_repairable();
        ui.horizontal(|ui| {
            let b = ui.add_enabled(ok.is_ok(), egui::Button::new("Repair last fill"));
            let b = match ok {
                Ok(_) => b.on_hover_text(
                    "leaked through a gap? this undoes the fill and waits for ONE stroke \
                     across the gap — a barrier only the fill sees; release re-runs the \
                     fill from the same click, and one undo takes it all back",
                ),
                Err(e) => b.on_disabled_hover_text(e),
            };
            if b.clicked() {
                app.push_cmd(AppCmd::ArmFillRepair {
                    virtual_barrier: true,
                });
            }
            let b2 = ui
                .add_enabled(ok.is_ok(), egui::Button::new("as ink"))
                .on_hover_text("the same repair, but the closing stroke stays as real ink");
            if b2.clicked() {
                app.push_cmd(AppCmd::ArmFillRepair {
                    virtual_barrier: false,
                });
            }
        });
    }
}

/// Row 160 — the Remove-dust sub tool's Tool Property: RD-002's one
/// threshold, RD-003's Mode row, and RD-007 folded in as a switch.
fn sec_dust(ui: &mut egui::Ui, app: &mut App) {
    ui.weak("drag around the patch to clean — the drag is the window");
    let mut o = app.dust_opts;
    // RD-002. The unit is AREA and the row says so: "5 px" here is a blob
    // of five connected pixels, not a five-pixel-wide one, and the same
    // number means the same thing in the Filter menu's Remove dust.
    let mut size = o.max_px as f32;
    let mut changed = ValueBar::new("Dust size", 1.0, 64.0)
        .step(1.0)
        .suffix(" px")
        .show(ui, &mut size)
        .on_hover_text(
            "the AREA of a blob, not its width — a blob of this many connected \
             pixels or fewer counts as dust",
        )
        .changed();
    o.max_px = (size.round() as u32).max(1);
    // RD-003: the four definitions of "dust". With the switch below on,
    // the two gap rows detect the same pixels (RD-009's 3-way).
    let mut mode = o.mode;
    egui::ComboBox::from_id_salt("mn.dust.mode")
        .width(ui.available_width() - 8.0)
        .selected_text(if o.select {
            mode.select_label()
        } else {
            mode.label()
        })
        .show_ui(ui, |ui| {
            for m in mn_core::DustMode::ALL {
                let text = if o.select { m.select_label() } else { m.label() };
                ui.selectable_value(&mut mode, m, text);
            }
        });
    changed |= mode != o.mode;
    o.mode = mode;
    ui.weak(match o.mode {
        mn_core::DustMode::OnTransparency => "isolated ink floating in emptiness",
        mn_core::DustMode::OnWhite => "blobs darker than the paper — cleaned back to white",
        mn_core::DustMode::GapsSurrounding => {
            "transparent pinholes inside a flat — what a bucket fill leaves"
        }
        mn_core::DustMode::GapsForeground => "the same pinholes, in the current colour",
    });
    // RD-007 Select dust, folded in: same detection, same window, and it
    // hands back marching ants instead of editing pixels.
    changed |= ui
        .checkbox(&mut o.select, "Select instead of cleaning")
        .on_hover_text("hands back a selection of what it found, so you can look before deleting")
        .changed();
    if changed {
        app.push_cmd(AppCmd::SetDustOpts(o));
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
