//! Tool Property rows for the SELECTION family and the flood tools: the
//! Selection tool, Select layer, Auto select (the wand), the Fill tool with
//! its Remove-dust sub tool, and the eyedropper / hand guides.
//!
//! Lane B2 split every section body here into ROWS. The flood knobs are
//! written ONCE over `mn_core::FillOpts` and drawn by both the wand and the
//! bucket, so a knob cannot mean two things; each row takes its own copy of
//! the tool's options, edits one field and pushes it back
//! (`SetWandOpts` / `SetFillOpts` / `SetDustOpts`), which is exactly what the
//! whole-section bodies did once per frame.

use super::*;

// --- the Selection tool ----------------------------------------------------

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

pub(crate) fn sel_op_label(op: mn_core::SelectionOp) -> &'static str {
    match op {
        mn_core::SelectionOp::Replace => "New",
        mn_core::SelectionOp::Add => "Add",
        mn_core::SelectionOp::Subtract => "Subtract",
        mn_core::SelectionOp::Intersect => "Intersect",
    }
}

pub(crate) fn row_select_hint(ui: &mut egui::Ui, _app: &mut App) {
    ui.weak("drag inside a selection to move it");
}

pub(crate) fn row_select_actions(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        if ui.button("Deselect (Ctrl+D)").clicked() {
            app.push_cmd(AppCmd::Deselect);
        }
        if ui.button("Invert").clicked() {
            app.push_cmd(AppCmd::SelectInvert);
        }
    });
}

/// Quick Mask (SE round 2026-08-19): brushes edit the selection instead of
/// inking while this is on.
pub(crate) fn row_select_quick_mask(ui: &mut egui::Ui, app: &mut App) {
    if ui.checkbox(&mut app.quick_mask, "Quick mask").changed() {
        app.set_status(if app.quick_mask {
            "quick mask ON — pen adds to the selection, eraser subtracts"
        } else {
            "quick mask off — brushes ink again"
        });
    }
}

fn select_magnetic(app: &App) -> bool {
    app.select_mode == crate::cmd::SelectMode::Magnetic
}

/// L-001: the magnetic lasso's one knob. Photoshop calls it Width; CSP does
/// not expose it at all, and it is the difference between tracing a face
/// (small) and a whole figure against clean paper (large).
pub(crate) fn row_select_snap_range(ui: &mut egui::Ui, app: &mut App) {
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

pub(crate) const ROWS_SELECT: &[Row] = &[
    row("select.opts.hint", "Hint", row_select_hint),
    row("select.opts.op", "Combine mode", sel_op_row),
    row("select.opts.actions", "Deselect / Invert", row_select_actions),
    row("select.opts.quick_mask", "Quick mask", row_select_quick_mask),
    row_when(
        "select.opts.snap_range",
        "Snap range",
        row_select_snap_range,
        select_magnetic,
    ),
];

// --- S-001 Select layer ----------------------------------------------------
//
// The Exclude switches (CSP 選択しないレイヤー) are why the tool is usable on
// a finished page — without them the click lands on the topmost tone or the
// text layer every time.

pub(crate) fn row_pick_layer_hint(ui: &mut egui::Ui, _app: &mut App) {
    ui.weak("click a pixel — the Layer palette jumps to the layer that drew it");
    ui.separator();
    ui.weak("Do not select:");
}

pub(crate) fn row_pick_layer_draft(ui: &mut egui::Ui, app: &mut App) {
    ui.checkbox(&mut app.pick_exclude.draft, "Draft layers")
        .on_hover_text("the rough underdrawing (CSP 下書き)");
}

pub(crate) fn row_pick_layer_text(ui: &mut egui::Ui, app: &mut App) {
    ui.checkbox(&mut app.pick_exclude.text, "Text layers");
}

pub(crate) fn row_pick_layer_locked(ui: &mut egui::Ui, app: &mut App) {
    ui.checkbox(&mut app.pick_exclude.locked, "Locked layers");
}

pub(crate) fn row_pick_layer_fill(ui: &mut egui::Ui, app: &mut App) {
    ui.checkbox(&mut app.pick_exclude.fill, "Fill / tone layers")
        .on_hover_text("live fill, gradient and tone layers — the flats that cover everything");
}

pub(crate) const ROWS_PICK_LAYER: &[Row] = &[
    row("obj.picklayer.hint", "Hint", row_pick_layer_hint),
    row("obj.picklayer.draft", "Draft layers", row_pick_layer_draft),
    row("obj.picklayer.text", "Text layers", row_pick_layer_text),
    row("obj.picklayer.locked", "Locked layers", row_pick_layer_locked),
    row(
        "obj.picklayer.fill",
        "Fill / tone layers",
        row_pick_layer_fill,
    ),
];

// --- the flood knobs, written once over `FillOpts` -------------------------

fn fill_tolerance_ui(ui: &mut egui::Ui, o: &mut mn_core::FillOpts) -> bool {
    let mut tol = o.tolerance * 100.0;
    let changed = ValueBar::new("Tolerance", 0.0, 50.0)
        .suffix("%")
        .show(ui, &mut tol)
        .changed();
    o.tolerance = tol / 100.0;
    changed
}

/// The "Auto gap & fringe" switch: dialled by hand when off, measured from
/// the lineart when on. `measured` is the last measurement to read back,
/// where the tool keeps one.
fn fill_auto_ui(
    ui: &mut egui::Ui,
    o: &mut mn_core::FillOpts,
    measured: Option<mn_core::AutoFill>,
) -> bool {
    let changed = ui
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
    }
    changed
}

fn fill_gap_ui(ui: &mut egui::Ui, o: &mut mn_core::FillOpts) -> bool {
    let mut gap = o.gap_close_px as f32;
    let changed = ValueBar::new("Close gap", 0.0, 8.0)
        .step(1.0)
        .suffix(" px")
        .show(ui, &mut gap)
        .changed();
    o.gap_close_px = gap as u32;
    changed
}

/// Row 40/120: CSP 半透明を透明にする — the antialiased skirt is fillable, the
/// flat runs under the fringe to the line's dark core, and the 1 px halo dies.
fn fill_semi_ui(ui: &mut egui::Ui, o: &mut mn_core::FillOpts) -> bool {
    ui.checkbox(
        &mut o.semi_transparent_paper,
        "Semi-transparent is fillable",
    )
    .on_hover_text(
        "treat the antialiased fringe of the lineart as paper — the fill \
         runs under it to the dark core and no light halo survives \
         against the flat",
    )
    .changed()
}

/// C-005: 対象色, the Target-colour dropdown — which pixel classes the flood
/// treats as fillable; everything else walls it.
fn fill_target_ui(ui: &mut egui::Ui, salt: &str, o: &mut mn_core::FillOpts) -> bool {
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
    let changed = close != o.close;
    o.close = close;
    changed
}

/// C-005 labels. "and transparent" spelled out — the pairing is the feature
/// (white-on-white lineart work needs the transparent half).
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

fn refer_label(v: mn_core::FillRefer) -> &'static str {
    match v {
        mn_core::FillRefer::All => "Refer: all layers",
        mn_core::FillRefer::Active => "Refer: editing layer",
        mn_core::FillRefer::Reference => "Refer: reference layer",
    }
}

/// CSP's 参照 block: what the flood samples. Shared by Fill and Auto select —
/// one `FillOpts`, one set of rows.
fn refer_pick_ui(ui: &mut egui::Ui, salt: &str, o: &mut mn_core::FillOpts) -> bool {
    let mut pick: Option<mn_core::FillRefer> = None;
    egui::ComboBox::from_id_salt(salt)
        .width(ui.available_width() - 8.0)
        .selected_text(refer_label(o.refer))
        .show_ui(ui, |ui| {
            for v in [
                mn_core::FillRefer::All,
                mn_core::FillRefer::Active,
                mn_core::FillRefer::Reference,
            ] {
                if ui.selectable_label(o.refer == v, refer_label(v)).clicked() {
                    pick = Some(v);
                }
            }
        });
    match pick {
        Some(v) => {
            o.refer = v;
            true
        }
        None => false,
    }
}

fn refer_drafts_ui(ui: &mut egui::Ui, o: &mut mn_core::FillOpts) -> bool {
    ui.checkbox(&mut o.refer_drafts, "Refer draft layers")
        .changed()
}

/// FI-022: the page's own perimeter joins the lineart as a wall.
fn refer_border_ui(ui: &mut egui::Ui, o: &mut mn_core::FillOpts) -> bool {
    ui.checkbox(&mut o.refer_border, "Refer to image border")
        .on_hover_text("the page's outer edge counts as a drawn line, so a fill cannot get out")
        .changed()
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

// --- Auto select (the wand) ------------------------------------------------
//
// The wand floods with the SAME `mn_core::fill` machinery the Fill tool uses,
// off the same `FillOpts` — so every knob that machinery honours belongs here
// too. Until 2026-08-23 the panel showed three of them and the other four
// were set (or not) behind the artist's back.

fn wand_row(
    ui: &mut egui::Ui,
    app: &mut App,
    f: impl FnOnce(&mut egui::Ui, &mut mn_core::FillOpts) -> bool,
) {
    let mut o = app.wand_opts;
    if f(ui, &mut o) {
        app.push_cmd(AppCmd::SetWandOpts(o));
    }
}

/// The two measured rows go read-only under the Auto switch, so they are not
/// knobs while it is on.
fn wand_manual(app: &App) -> bool {
    !app.wand_opts.auto
}

pub(crate) fn row_wand_tolerance(ui: &mut egui::Ui, app: &mut App) {
    wand_row(ui, app, fill_tolerance_ui);
}
pub(crate) fn row_wand_auto(ui: &mut egui::Ui, app: &mut App) {
    wand_row(ui, app, |ui, o| fill_auto_ui(ui, o, None));
}
pub(crate) fn row_wand_gap(ui: &mut egui::Ui, app: &mut App) {
    wand_row(ui, app, fill_gap_ui);
}
pub(crate) fn row_wand_area_scaling(ui: &mut egui::Ui, app: &mut App) {
    wand_row(ui, app, |ui, o| {
        area_scaling_row(ui, "mn.wand.expand", o)
    });
}
pub(crate) fn row_wand_semi(ui: &mut egui::Ui, app: &mut App) {
    wand_row(ui, app, fill_semi_ui);
}
pub(crate) fn row_wand_target(ui: &mut egui::Ui, app: &mut App) {
    wand_row(ui, app, |ui, o| {
        fill_target_ui(ui, "mn.wand.expand", o)
    });
}
pub(crate) fn row_wand_refer(ui: &mut egui::Ui, app: &mut App) {
    wand_row(ui, app, |ui, o| refer_pick_ui(ui, "mn.wand.refer", o));
}
pub(crate) fn row_wand_refer_drafts(ui: &mut egui::Ui, app: &mut App) {
    wand_row(ui, app, refer_drafts_ui);
}
pub(crate) fn row_wand_refer_border(ui: &mut egui::Ui, app: &mut App) {
    wand_row(ui, app, refer_border_ui);
}

pub(crate) const ROWS_WAND: &[Row] = &[
    row("wand.opts.op", "Combine mode", sel_op_row),
    row("wand.opts.tolerance", "Tolerance", row_wand_tolerance),
    row("wand.opts.auto", "Auto gap & fringe", row_wand_auto),
    row_when("wand.opts.gap", "Close gap", row_wand_gap, wand_manual),
    row_when(
        "wand.opts.area_scaling",
        "Area scaling",
        row_wand_area_scaling,
        wand_manual,
    ),
    row(
        "wand.opts.semi",
        "Semi-transparent is fillable",
        row_wand_semi,
    ),
    row("wand.opts.target", "Target colour", row_wand_target),
    row("wand.opts.refer", "Refer", row_wand_refer),
    row(
        "wand.opts.refer_drafts",
        "Refer draft layers",
        row_wand_refer_drafts,
    ),
    row(
        "wand.opts.refer_border",
        "Refer to image border",
        row_wand_refer_border,
    ),
];

pub(crate) fn sec_wand_guide(ui: &mut egui::Ui, _app: &mut App) {
    ui.weak("click an area to select it — G fills, Delete clears");
}

// --- the Fill tool ---------------------------------------------------------

fn fill_row(
    ui: &mut egui::Ui,
    app: &mut App,
    f: impl FnOnce(&mut egui::Ui, &mut mn_core::FillOpts) -> bool,
) {
    let mut o = app.fill_opts;
    if f(ui, &mut o) {
        app.push_cmd(AppCmd::SetFillOpts(o));
    }
}

/// The sub tools that run a FLOOD. FI-004 fills the drawn shape itself and
/// dust removal runs no flood at all, so tolerance, gap closing, area scaling
/// and 参照 would be knobs that do nothing under either.
fn fill_floods(app: &App) -> bool {
    !matches!(
        app.fill_mode,
        crate::cmd::FillMode::Lasso | crate::cmd::FillMode::Dust
    )
}

fn fill_manual(app: &App) -> bool {
    fill_floods(app) && !app.fill_opts.auto
}

fn fill_dusts(app: &App) -> bool {
    app.fill_mode == crate::cmd::FillMode::Dust
}

/// NL-006's switch (TRIAGE 137) belongs to the CLICK sub tool alone:
/// enclose-and-fill paints pockets, not a window, and the live model has no
/// shape for that.
fn fill_clicks(app: &App) -> bool {
    app.fill_mode == crate::cmd::FillMode::Click
}

/// What this sub tool does, in one line — the gesture, not a setting.
pub(crate) fn row_fill_hint(ui: &mut egui::Ui, app: &mut App) {
    use crate::cmd::FillMode;
    match app.fill_mode {
        FillMode::Lasso => {
            ui.weak("drag a shape and it is painted as drawn — lines are ignored");
            ui.weak("combines with an active selection like any fill");
        }
        FillMode::Dust => {
            ui.weak("drag around the patch to clean — the drag is the window");
        }
        FillMode::Enclose => {
            ui.weak("drag right around the areas to fill — everything closed inside goes");
        }
        // Row 119: the leftover pen runs the same flood, so every knob below
        // means what it means under the bucket — only the seeds and the mask
        // differ, and both are decided by what is already painted.
        FillMode::Leftover => {
            ui.weak("scrub across the flat — only enclosed spots still empty fill");
        }
        _ => {}
    }
}

pub(crate) fn row_fill_tolerance(ui: &mut egui::Ui, app: &mut App) {
    fill_row(ui, app, fill_tolerance_ui);
}

/// ROADMAP: the fill that measures gap and fringe itself. Opt-in — the two
/// rows it drives go read-only underneath, showing what the last fill
/// actually measured, so the numbers stay learnable.
pub(crate) fn row_fill_auto(ui: &mut egui::Ui, app: &mut App) {
    let measured = app.fill_auto;
    fill_row(ui, app, |ui, o| fill_auto_ui(ui, o, measured));
}

pub(crate) fn row_fill_gap(ui: &mut egui::Ui, app: &mut App) {
    fill_row(ui, app, fill_gap_ui);
}
pub(crate) fn row_fill_area_scaling(ui: &mut egui::Ui, app: &mut App) {
    fill_row(ui, app, |ui, o| {
        area_scaling_row(ui, "mn.fill.expand", o)
    });
}
pub(crate) fn row_fill_semi(ui: &mut egui::Ui, app: &mut App) {
    fill_row(ui, app, fill_semi_ui);
}
pub(crate) fn row_fill_target(ui: &mut egui::Ui, app: &mut App) {
    fill_row(ui, app, |ui, o| {
        fill_target_ui(ui, "mn.fill.expand", o)
    });
}
pub(crate) fn row_fill_refer(ui: &mut egui::Ui, app: &mut App) {
    fill_row(ui, app, |ui, o| refer_pick_ui(ui, "mn.fill.refer", o));
}
pub(crate) fn row_fill_refer_drafts(ui: &mut egui::Ui, app: &mut App) {
    fill_row(ui, app, refer_drafts_ui);
}
pub(crate) fn row_fill_refer_border(ui: &mut egui::Ui, app: &mut App) {
    fill_row(ui, app, refer_border_ui);
}

/// Fill a LIVE layer instead of painting. Default off; the Gradient tool
/// carries its own `gradient_live`, which ships on.
pub(crate) fn row_fill_live(ui: &mut egui::Ui, app: &mut App) {
    ui.checkbox(&mut app.fill_live, "Create live layer");
}

/// Leak repair's door where the eyes already are (owner UX call 2026-08-31):
/// a fill that leaked is fixed from HERE, one tap — the palette rows stay for
/// keys.json. Greyed with the arm's own refusal as the hover, so the button
/// can never lie about why.
pub(crate) fn row_fill_repair(ui: &mut egui::Ui, app: &mut App) {
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

// --- Row 160: the Remove-dust sub tool ------------------------------------

fn dust_row(
    ui: &mut egui::Ui,
    app: &mut App,
    f: impl FnOnce(&mut egui::Ui, &mut crate::cmd::DustOpts) -> bool,
) {
    let mut o = app.dust_opts;
    if f(ui, &mut o) {
        app.push_cmd(AppCmd::SetDustOpts(o));
    }
}

/// RD-002. The unit is AREA and the row says so: "5 px" here is a blob of
/// five connected pixels, not a five-pixel-wide one, and the same number
/// means the same thing in the Filter menu's Remove dust.
pub(crate) fn row_dust_size(ui: &mut egui::Ui, app: &mut App) {
    dust_row(ui, app, |ui, o| {
        let mut size = o.max_px as f32;
        let changed = ValueBar::new("Dust size", 1.0, 64.0)
            .step(1.0)
            .suffix(" px")
            .show(ui, &mut size)
            .on_hover_text(
                "the AREA of a blob, not its width — a blob of this many connected \
                 pixels or fewer counts as dust",
            )
            .changed();
        o.max_px = (size.round() as u32).max(1);
        changed
    });
}

/// RD-003: the four definitions of "dust". With Select instead of cleaning
/// on, the two gap rows detect the same pixels (RD-009's 3-way).
pub(crate) fn row_dust_mode(ui: &mut egui::Ui, app: &mut App) {
    dust_row(ui, app, |ui, o| {
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
        let changed = mode != o.mode;
        o.mode = mode;
        ui.weak(match o.mode {
            mn_core::DustMode::OnTransparency => "isolated ink floating in emptiness",
            mn_core::DustMode::OnWhite => "blobs darker than the paper — cleaned back to white",
            mn_core::DustMode::GapsSurrounding => {
                "transparent pinholes inside a flat — what a bucket fill leaves"
            }
            mn_core::DustMode::GapsForeground => "the same pinholes, in the current colour",
        });
        changed
    });
}

/// RD-007 Select dust, folded in: same detection, same window, and it hands
/// back marching ants instead of editing pixels.
pub(crate) fn row_dust_select(ui: &mut egui::Ui, app: &mut App) {
    dust_row(ui, app, |ui, o| {
        ui.checkbox(&mut o.select, "Select instead of cleaning")
            .on_hover_text("hands back a selection of what it found, so you can look before deleting")
            .changed()
    });
}

pub(crate) const ROWS_FILL: &[Row] = &[
    row("fill.opts.hint", "Hint", row_fill_hint),
    row_when("fill.opts.dust_size", "Dust size", row_dust_size, fill_dusts),
    row_when("fill.opts.dust_mode", "Dust mode", row_dust_mode, fill_dusts),
    row_when(
        "fill.opts.dust_select",
        "Select instead of cleaning",
        row_dust_select,
        fill_dusts,
    ),
    row_when(
        "fill.opts.tolerance",
        "Tolerance",
        row_fill_tolerance,
        fill_floods,
    ),
    row_when(
        "fill.opts.auto",
        "Auto gap & fringe",
        row_fill_auto,
        fill_floods,
    ),
    row_when("fill.opts.gap", "Close gap", row_fill_gap, fill_manual),
    row_when(
        "fill.opts.area_scaling",
        "Area scaling",
        row_fill_area_scaling,
        fill_manual,
    ),
    row_when(
        "fill.opts.semi",
        "Semi-transparent is fillable",
        row_fill_semi,
        fill_floods,
    ),
    row_when(
        "fill.opts.target",
        "Target colour",
        row_fill_target,
        fill_floods,
    ),
    row_when("fill.opts.refer", "Refer", row_fill_refer, fill_floods),
    row_when(
        "fill.opts.refer_drafts",
        "Refer draft layers",
        row_fill_refer_drafts,
        fill_floods,
    ),
    row_when(
        "fill.opts.refer_border",
        "Refer to image border",
        row_fill_refer_border,
        fill_floods,
    ),
    row_when(
        "fill.opts.live",
        "Create live layer",
        row_fill_live,
        fill_clicks,
    ),
    row_when(
        "fill.opts.repair",
        "Repair last fill",
        row_fill_repair,
        fill_floods,
    ),
];

// --- the two guides --------------------------------------------------------

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
