//! Tool Property rows for PANELS and BALLOONS — the Frame tool, the Balloon
//! tool, and the Operation tool with a panel or a bubble selected.
//!
//! Lane B2 (`docs/plans/2026-09-06-effect-lines-parity.md`) split every
//! section body in here into ROWS: one control per [`Row`], so the settings
//! window can hide and reorder them one at a time. Nothing about what a
//! control DOES changed — a row that used to sit behind an `if` inside the
//! body now carries that condition as its `applies`, which is what lets the
//! window grey it and say why instead of the row simply not being there.
//!
//! The section state a row needs is recomputed per row (the selected
//! balloon, the document's px-per-mm): each is a handful of field reads, the
//! palette draws a dozen a frame, and it never shows up.
//!
//! This file was 1 540 lines before the round and the conversion added
//! rows, so it shed two subjects that were only ever here by accident:
//! `effect_lines.rs` / `figure_lines.rs` (a selected 集中線 set and the
//! Figure tool's knobs) and `balloon_ink.rs` (the colour/opacity/screen
//! controls, which are written once and drawn in two panels). Pure moves.

use super::*;

// --- Frame tool ------------------------------------------------------------

/// The Frame sub tools that CREATE a panel (rectangle, polyline, pen).
fn frame_creates(app: &App) -> bool {
    app.frame_mode.creates()
}

/// The Frame sub tools that CUT an existing panel.
fn frame_divides(app: &App) -> bool {
    !app.frame_mode.creates()
}

/// TRIAGE 128 (FB-026/FB-022): cutting a panel that already has art in it
/// has three answers and CSP makes you say which. Only the folder-making sub
/// tool asks — Divide frame border never touches the layer structure, so the
/// question does not arise there.
fn frame_divides_folder(app: &App) -> bool {
    app.frame_mode == crate::cmd::FrameMode::DivideFolder
}

/// CSP Rectangle-frame property block: border on/off and, when it is on, how
/// thick. One row because the width is the checkbox's own value — hiding
/// "Border" is meant to hide the whole question.
pub(crate) fn row_frame_border(ui: &mut egui::Ui, app: &mut App) {
    ui.checkbox(&mut app.frame_draw_border, "Draw border");
    if app.frame_draw_border {
        let mut b = app.frame_border_mm;
        if ValueBar::new("Border", 0.1, 3.0)
            .decimals(2)
            .display_text(px_mm_text(b, app.page_dpi()))
            .show(ui, &mut b)
            .changed()
        {
            app.frame_border_mm = b;
        }
    }
}

pub(crate) fn row_frame_fill_inside(ui: &mut egui::Ui, app: &mut App) {
    ui.checkbox(&mut app.frame_fill_inside, "Fill inside the frame")
        .on_hover_text("adds the White base layer that hides art below the panel");
}

/// The gap the cut leaves, per sub tool (border cuts and folder cuts keep
/// their own pair — the same drag means a different thing in each).
pub(crate) fn row_frame_gutter(ui: &mut egui::Ui, app: &mut App) {
    use crate::cmd::FrameMode;
    let (mut lr, mut tb) = if app.frame_mode == FrameMode::DivideBorder {
        app.gutter_border_mm
    } else {
        app.gutter_folder_mm
    };
    let mut changed = ValueBar::new("Gutter L/R", 0.0, 20.0)
        .decimals(1)
        .suffix(" mm")
        .show(ui, &mut lr)
        .changed();
    changed |= ValueBar::new("Gutter T/B", 0.0, 20.0)
        .decimals(1)
        .suffix(" mm")
        .show(ui, &mut tb)
        .changed();
    if changed {
        if app.frame_mode == FrameMode::DivideBorder {
            app.gutter_border_mm = (lr, tb);
        } else {
            app.gutter_folder_mm = (lr, tb);
        }
    }
}

pub(crate) fn row_frame_divide_contents(ui: &mut egui::Ui, app: &mut App) {
    ui.label("Contents of the new folder");
    for c in crate::cmd::DivideContents::ALL {
        if ui
            .selectable_label(app.frame_divide_contents == c, c.label())
            .clicked()
        {
            app.frame_divide_contents = c;
        }
    }
}

/// TRIAGE 129 (FB-023..025): the equal division's grid.
pub(crate) fn row_frame_grid(ui: &mut egui::Ui, app: &mut App) {
    let (mut cols, mut rows) = app.frame_div_grid;
    let mut c = cols as f32;
    let mut r = rows as f32;
    if ValueBar::new("Columns", 1.0, 12.0)
        .decimals(0)
        .show(ui, &mut c)
        .changed()
    {
        cols = (c.round() as usize).max(1);
    }
    if ValueBar::new("Rows", 1.0, 12.0)
        .decimals(0)
        .show(ui, &mut r)
        .changed()
    {
        rows = (r.round() as usize).max(1);
    }
    app.frame_div_grid = (cols, rows);
}

pub(crate) fn row_frame_fit_side(ui: &mut egui::Ui, app: &mut App) {
    ui.checkbox(&mut app.frame_div_fit_side, "Fit to side direction")
        .on_hover_text("a tilted panel divides along its own slant, not true vertical");
}

/// The whole grid in one command — and the tap that runs a panel edge off
/// the page, which is the other half of "divide" and has nowhere else to be
/// said.
pub(crate) fn row_frame_divide(ui: &mut egui::Ui, app: &mut App) {
    let (cols, rows) = app.frame_div_grid;
    if ui
        .button("Divide equally")
        .on_hover_text("the whole grid in one command; gutters come from the values above")
        .clicked()
    {
        app.push_cmd(AppCmd::FrameDivideEqually {
            cols,
            rows,
            fit_to_side: app.frame_div_fit_side,
        });
    }
    ui.weak("tap a panel edge (no drag) to run it to the page edge");
}

pub(crate) fn row_frame_new_folder(ui: &mut egui::Ui, app: &mut App) {
    if ui.button("New frame border folder").clicked() {
        app.push_cmd(AppCmd::NewFrameLayer);
    }
}

pub(crate) const ROWS_FRAME_TOOL: &[Row] = &[
    row_when("frame.tool.border", "Border", row_frame_border, frame_creates),
    row_when(
        "frame.tool.fill_inside",
        "Fill inside the frame",
        row_frame_fill_inside,
        frame_creates,
    ),
    row_when("frame.tool.gutter", "Gutter", row_frame_gutter, frame_divides),
    row_when(
        "frame.tool.contents",
        "Contents of the new folder",
        row_frame_divide_contents,
        frame_divides_folder,
    ),
    row_when(
        "frame.tool.grid",
        "Columns / rows",
        row_frame_grid,
        frame_divides,
    ),
    row_when(
        "frame.tool.fit_side",
        "Fit to side direction",
        row_frame_fit_side,
        frame_divides,
    ),
    row_when(
        "frame.tool.divide",
        "Divide equally",
        row_frame_divide,
        frame_divides,
    ),
    row(
        "frame.tool.new_folder",
        "New frame border folder",
        row_frame_new_folder,
    ),
];

pub(crate) fn sec_frame_guide(ui: &mut egui::Ui, app: &mut App) {
    use crate::cmd::FrameMode;
    ui.weak(match app.frame_mode {
        FrameMode::Rect => "drag out the new panel; it becomes a frame folder",
        FrameMode::Polyline => "click corners; first corner / Enter closes",
        FrameMode::Pen => "draw the panel outline; it closes itself",
        FrameMode::DivideFolder => "drag across a panel: the cut piece gets its own folder",
        _ => "drag across a panel to cut it; level drags snap straight",
    });
}

// --- Balloon tool ----------------------------------------------------------

/// One control, so it stays an unsplit section: its id already IS its row id
/// and an artist who hid it keeps it hidden.
pub(crate) fn sec_balloon_line(ui: &mut egui::Ui, app: &mut App) {
    let mut b = app.balloon_border_mm;
    if ValueBar::new("Line", 0.05, 2.0)
        .decimals(2)
        .suffix(" mm")
        .show(ui, &mut b)
        .changed()
    {
        app.balloon_border_mm = b;
    }
}

pub(crate) fn sec_balloon_guide(ui: &mut egui::Ui, app: &mut App) {
    ui.weak(match app.balloon_mode {
        BalloonMode::Draw => "draw the bubble outline — a smooth pressure-aware curve",
        BalloonMode::Tail => "drag from inside a balloon out to the tip",
        _ => "drag out the bubble; O edits it afterwards",
    });
}

// --- tail shape (`B-005`, `B-006`), also written once ----------------------

fn tail_kind_ui(ui: &mut egui::Ui, kind: &mut mn_core::TailKind) -> (bool, bool) {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.weak("type");
        for k in mn_core::TailKind::ALL {
            if ui.selectable_label(*kind == k, k.label()).clicked() && *kind != k {
                *kind = k;
                changed = true;
            }
        }
    });
    (changed, changed)
}

fn tail_bend_ui(ui: &mut egui::Ui, bend: &mut f32) -> (bool, bool) {
    let mut b = *bend;
    let resp = ValueBar::new("Bend", -0.6, 0.6).decimals(2).show(ui, &mut b);
    let changed = resp.changed();
    if changed {
        *bend = b.clamp(-0.6, 0.6);
    }
    if *bend != 0.0 {
        ui.weak("the tail curves around the art instead of stabbing through it");
    }
    (
        changed,
        resp.drag_stopped() || (changed && !resp.dragged()),
    )
}

/// Balloon tool: the width the NEXT tail drag lands at.
pub(crate) fn row_balloon_tail_width(ui: &mut egui::Ui, app: &mut App) {
    let mut t = app.balloon_tail_mm;
    if ValueBar::new("Tail width", 1.0, 20.0)
        .suffix(" mm")
        .show(ui, &mut t)
        .changed()
    {
        app.balloon_tail_mm = t;
    }
}

/// `B-005`/`B-006`: what the NEXT tail drag lands as.
fn tail_tool_row(
    ui: &mut egui::Ui,
    app: &mut App,
    f: impl FnOnce(&mut egui::Ui, &mut mn_core::TailKind, &mut f32) -> (bool, bool),
) {
    let (mut kind, mut bend) = (app.balloon_tail_kind, app.balloon_tail_bend);
    if f(ui, &mut kind, &mut bend).0 {
        app.balloon_tail_kind = kind;
        app.balloon_tail_bend = bend;
    }
}

pub(crate) fn row_balloon_tail_kind(ui: &mut egui::Ui, app: &mut App) {
    tail_tool_row(ui, app, |ui, k, _b| tail_kind_ui(ui, k));
}

pub(crate) fn row_balloon_tail_bend(ui: &mut egui::Ui, app: &mut App) {
    tail_tool_row(ui, app, |ui, _k, b| tail_bend_ui(ui, b));
}

pub(crate) const ROWS_BALLOON_TAIL: &[Row] = &[
    row("balloon.tail.width", "Tail width", row_balloon_tail_width),
    row("balloon.tail.kind", "Tail type", row_balloon_tail_kind),
    row("balloon.tail.bend", "Bend", row_balloon_tail_bend),
];

// --- the selected balloon (Operation tool) ---------------------------------

/// The selected bubble's set, cloned so the borrow ends before `push_cmd`.
fn obj_balloon(app: &App) -> Option<(usize, usize, mn_core::BalloonSet)> {
    let (li, bi) = app.balloon_sel?;
    let bs = app.doc.layers.get(li)?.balloons()?.clone();
    Some((li, bi, bs))
}

/// CSP's 最小値 only means anything with the pressure toggle on.
fn obj_balloon_pressure(app: &App) -> bool {
    obj_balloon(app).is_some_and(|(_, bi, bs)| bs.pressure_width && bi < bs.balloons.len())
}

/// CSP's "correct line width" is for HAND-DRAWN bubbles: it scales the
/// recorded per-anchor pressure widths at render time, and a shape without
/// them has nothing to scale.
fn obj_balloon_drawn(app: &App) -> bool {
    obj_balloon(app).is_some_and(|(_, bi, bs)| {
        bs.balloons.get(bi).is_some_and(|b| match &b.shape {
            mn_core::BalloonShape::Polygon { widths, .. } => !widths.is_empty(),
            _ => false,
        })
    })
}

fn obj_balloon_has_tail(app: &App) -> bool {
    obj_balloon(app).is_some_and(|(_, bi, bs)| bs.balloons.get(bi).is_some_and(|b| !b.tails.is_empty()))
}

fn obj_balloon_no_tail(app: &App) -> bool {
    obj_balloon(app).is_some_and(|(_, bi, bs)| bs.balloons.get(bi).is_some_and(|b| b.tails.is_empty()))
}

/// The bubble's outline width, in mm. One undo step per drag, like Layer
/// Property's buffer.
pub(crate) fn row_obj_balloon_width(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, _bi, bs)) = obj_balloon(app) else {
        return;
    };
    let px_per_mm = app.mm_to_px(1.0).max(0.001);
    let mut mm = app.border_edit.unwrap_or(bs.border_px / px_per_mm);
    let resp = ValueBar::new("Line", 0.05, 2.0)
        .decimals(2)
        .suffix(" mm")
        .show(ui, &mut mm);
    if resp.changed() {
        app.border_edit = Some(mm);
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        if let Some(mm) = app.border_edit.take() {
            let mut bs2 = bs.clone();
            bs2.border_px = (mm * px_per_mm).max(0.5);
            app.push_cmd(AppCmd::BalloonCommit {
                layer: li,
                balloons: bs2,
            });
        }
    }
}

pub(crate) fn row_obj_balloon_pressure(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, _bi, bs)) = obj_balloon(app) else {
        return;
    };
    let mut pw = bs.pressure_width;
    if ui
        .checkbox(&mut pw, "Line follows pen pressure")
        .on_hover_text("drawn bubbles: a light hand inks a thinner outline")
        .changed()
    {
        let mut bs2 = bs.clone();
        bs2.pressure_width = pw;
        app.push_cmd(AppCmd::BalloonCommit {
            layer: li,
            balloons: bs2,
        });
    }
}

/// CSP's 最小値: how thin the outline gets where the pen was weightless, as
/// a percentage of the line width.
pub(crate) fn row_obj_balloon_min_size(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, bi, bs)) = obj_balloon(app) else {
        return;
    };
    if !bs.pressure_width || bi >= bs.balloons.len() {
        return;
    }
    // The in-progress drag value lives in egui's own scratch memory rather
    // than on `App`: one undo step per drag, and no third `*_edit` field on
    // the app for a bar that exists in one panel.
    let key = egui::Id::new(("mn.balloon.min-size", li, bi));
    let held: Option<f32> = ui.data(|d| d.get_temp(key));
    let mut pct = held.unwrap_or(bs.balloons[bi].min_width * 100.0);
    let resp = ValueBar::new("Min size", 0.0, 100.0)
        .suffix(" %")
        .show(ui, &mut pct);
    if resp.changed() {
        ui.data_mut(|d| d.insert_temp(key, pct));
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        ui.data_mut(|d| d.remove::<f32>(key));
        let mut bs2 = bs.clone();
        bs2.balloons[bi].min_width = (pct / 100.0).clamp(0.0, 1.0);
        app.push_cmd(AppCmd::BalloonCommit {
            layer: li,
            balloons: bs2,
        });
    }
}

/// Drawn balloons: CSP's "correct line width" — a render-time multiplier on
/// the outline (`Balloon::width_scale`, applied at rasterize). The recorded
/// per-anchor pressure widths are DATA: the old implementation rewrote them,
/// saturating at 1.0, so scaling back down returned a flat border instead of
/// the original taper (auditor round 33). The bar is ABSOLUTE — seeded from
/// the balloon's current scale — and commits as one undo step.
pub(crate) fn row_obj_balloon_correct_width(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, bi, bs)) = obj_balloon(app) else {
        return;
    };
    let Some(cur) = bs.balloons.get(bi).map(|b| b.width_scale) else {
        return;
    };
    let mut m = app.width_edit.unwrap_or(cur);
    let resp = ValueBar::new("Correct width", 0.25, 4.0)
        .suffix(" ×")
        .show(ui, &mut m);
    if resp.changed() {
        app.width_edit = Some(m);
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        if let Some(m) = app.width_edit.take() {
            let mut bs2 = bs.clone();
            bs2.balloons[bi].width_scale = m.clamp(0.25, 4.0);
            app.push_cmd(AppCmd::BalloonCommit {
                layer: li,
                balloons: bs2,
            });
        }
    }
}

/// ROADMAP good-first-issue #1: size the bubble around the lettering that is
/// already in it. One press, one undo step.
pub(crate) fn row_obj_balloon_fit(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, bi, _bs)) = obj_balloon(app) else {
        return;
    };
    if ui
        .button("Fit to text")
        .on_hover_text(
            "resize the bubble around the lettering inside it; the tail, the style and a \
             hand-drawn outline's own shape are kept",
        )
        .clicked()
    {
        app.fit_balloon_to_text(li, bi);
    }
}

pub(crate) fn row_obj_balloon_hint(ui: &mut egui::Ui, app: &mut App) {
    let anchors = obj_balloon(app).and_then(|(_, bi, bs)| {
        bs.balloons.get(bi).map(|b| match &b.shape {
            mn_core::BalloonShape::Polygon { points, .. } => points.len(),
            _ => 0,
        })
    });
    if let Some(n) = anchors.filter(|&n| n > 0) {
        ui.weak(format!(
            "{n} anchors — drag to reshape; Ctrl+click an edge adds one, \
             Ctrl+click an anchor or tail deletes it, Alt+click toggles corner"
        ));
    } else {
        ui.weak("drag the handles to reshape; Ctrl+click a tail deletes it");
    }
}

/// The Operation tool + a selected BALLOON: edit the bubble itself (the
/// owner's fix 7 — Tool Property edits the selected item).
pub(crate) const ROWS_OBJ_BALLOON: &[Row] = &[
    row("obj.balloon.width", "Line", row_obj_balloon_width),
    row(
        "obj.balloon.pressure",
        "Line follows pen pressure",
        row_obj_balloon_pressure,
    ),
    row_when(
        "obj.balloon.min_size",
        "Min size",
        row_obj_balloon_min_size,
        obj_balloon_pressure,
    ),
    row_when(
        "obj.balloon.correct_width",
        "Correct width",
        row_obj_balloon_correct_width,
        obj_balloon_drawn,
    ),
    row("obj.balloon.fit", "Fit to text", row_obj_balloon_fit),
    row("obj.balloon.hint", "Handles", row_obj_balloon_hint),
];

/// The Operation tool + a selected BALLOON: the shape of its tails.
///
/// It edits the balloon, not one tail — the panel has no tail selection and a
/// bubble with two tails wants them matching. A hand-mixed pair (possible
/// only by editing two tails in turn) shows the tool's own setting rather
/// than picking one of them to call the truth.
fn tail_obj_row(
    ui: &mut egui::Ui,
    app: &mut App,
    f: impl FnOnce(&mut egui::Ui, &mut mn_core::TailKind, &mut f32) -> (bool, bool),
) {
    let Some((li, bi, bs)) = obj_balloon(app) else {
        return;
    };
    let Some(b) = bs.balloons.get(bi) else { return };
    if b.tails.is_empty() {
        return;
    }
    let (mut kind, mut bend) = b
        .tail_style()
        .unwrap_or((app.balloon_tail_kind, app.balloon_tail_bend));
    if f(ui, &mut kind, &mut bend).1 {
        let mut bs2 = bs.clone();
        bs2.balloons[bi].set_tail_style(kind, bend);
        app.push_cmd(AppCmd::BalloonCommit {
            layer: li,
            balloons: bs2,
        });
        app.balloon_tail_kind = kind;
        app.balloon_tail_bend = bend;
    }
}

pub(crate) fn row_obj_tail_none(ui: &mut egui::Ui, _app: &mut App) {
    ui.weak("no tail yet — the Balloon tool's Tail mode drags one out");
}

pub(crate) fn row_obj_tail_kind(ui: &mut egui::Ui, app: &mut App) {
    tail_obj_row(ui, app, |ui, k, _b| tail_kind_ui(ui, k));
}

pub(crate) fn row_obj_tail_bend(ui: &mut egui::Ui, app: &mut App) {
    tail_obj_row(ui, app, |ui, _k, b| tail_bend_ui(ui, b));
}

pub(crate) const ROWS_OBJ_TAIL: &[Row] = &[
    row_when(
        "obj.balloon.tail.none",
        "No tail yet",
        row_obj_tail_none,
        obj_balloon_no_tail,
    ),
    row_when(
        "obj.balloon.tail.kind",
        "Tail type",
        row_obj_tail_kind,
        obj_balloon_has_tail,
    ),
    row_when(
        "obj.balloon.tail.bend",
        "Bend",
        row_obj_tail_bend,
        obj_balloon_has_tail,
    ),
];

// --- the selected panel (Operation tool) -----------------------------------

/// The selected panel's frame set, cloned so the borrow ends before
/// `push_cmd`.
fn obj_frame(app: &App) -> Option<(usize, mn_core::FrameSet)> {
    let (li, _fi) = app.object_sel?;
    Some((li, app.doc.layers.get(li)?.frames()?.clone()))
}

pub(crate) fn row_obj_frame_border(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, fs)) = obj_frame(app) else {
        return;
    };
    let px_per_mm = app.mm_to_px(1.0).max(0.001);
    let mut mm = app.border_edit.unwrap_or(fs.border_px / px_per_mm);
    let resp = ValueBar::new("Border", 0.1, 3.0)
        .decimals(2)
        .display_text(px_mm_text(mm, app.page_dpi()))
        .show(ui, &mut mm);
    if resp.changed() {
        app.border_edit = Some(mm);
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        if let Some(mm) = app.border_edit.take() {
            let mut fs2 = fs.clone();
            fs2.border_px = (mm * px_per_mm).max(0.5);
            app.push_cmd(AppCmd::FrameCommit {
                layer: li,
                frames: fs2,
            });
        }
    }
}

/// CSP's frame styling (workflow walk #1, item 33): a selected frame's border
/// takes a main colour — black by default, black in old files.
pub(crate) fn row_obj_frame_color(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, fs)) = obj_frame(app) else {
        return;
    };
    let mut rgb = fs.color;
    let resp = ui.color_edit_button_srgb(&mut rgb);
    if (resp.changed() && !resp.dragged()) || resp.drag_stopped() {
        let mut fs2 = fs.clone();
        fs2.color = rgb;
        app.push_cmd(AppCmd::FrameCommit {
            layer: li,
            frames: fs2,
        });
    }
}

/// CSP's "Keep gutters aligned" (audit P0-4): All = dragging a border brings
/// the facing border of the panel across the gutter with it, so the gap keeps
/// its width. None = the one edge moves and the gutter narrows.
pub(crate) fn row_obj_frame_gutter_align(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label("Keep gutters aligned");
        egui::ComboBox::from_id_salt("mn.obj.frame.gutter_align")
            .selected_text(if app.gutter_align_all { "All" } else { "None" })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.gutter_align_all, false, "None");
                ui.selectable_value(&mut app.gutter_align_all, true, "All");
            });
    })
    .response
    .on_hover_text("moves the neighbouring panel's facing border too, keeping the gutter width");
}

pub(crate) fn row_obj_frame_hint(ui: &mut egui::Ui, app: &mut App) {
    let Some((_li, fs)) = obj_frame(app) else {
        return;
    };
    ui.weak(format!(
        "{} panel(s) — drag vertices/edges; the box scales",
        fs.frames.len()
    ));
}

/// The Operation tool + a selected PANEL: edit the frame border.
pub(crate) const ROWS_OBJ_FRAME: &[Row] = &[
    row("obj.frame.border", "Border", row_obj_frame_border),
    row("obj.frame.color", "Border colour", row_obj_frame_color),
    row(
        "obj.frame.gutter_align",
        "Keep gutters aligned",
        row_obj_frame_gutter_align,
    ),
    row("obj.frame.hint", "Panels", row_obj_frame_hint),
];

// --- the Operation tool's own guide ----------------------------------------

pub(crate) fn sec_obj_guide(ui: &mut egui::Ui, app: &mut App) {
    // Row 78 (CSP Operation ▸ Object ▸ Select): the four-way combine.
    ui.label("Select");
    ui.horizontal(|ui| {
        for m in [
            crate::cmd::SelectCombine::New,
            crate::cmd::SelectCombine::Add,
            crate::cmd::SelectCombine::Remove,
            crate::cmd::SelectCombine::Toggle,
        ] {
            if ui
                .selectable_label(app.object_combine == m, m.label())
                .on_hover_text(match m {
                    crate::cmd::SelectCombine::New => {
                        "each click starts a fresh selection (Shift-click adds anyway)"
                    }
                    crate::cmd::SelectCombine::Add => {
                        "clicks stack objects into a multi selection"
                    }
                    crate::cmd::SelectCombine::Remove => {
                        "clicking a selected object drops it from the selection"
                    }
                    crate::cmd::SelectCombine::Toggle => {
                        "clicking flips membership — selected objects deselect"
                    }
                })
                .clicked()
            {
                app.object_combine = m;
            }
        }
    });
    let n = app.object_multi.len();
    if n > 0 {
        ui.weak(format!(
            "{n} more object{} in the selection — Del removes all",
            if n == 1 { "" } else { "s" }
        ));
    }
    ui.weak("click a text box, balloon, panel or effect-line set");
    ui.weak("drag moves; handles reshape; the blue box scales/rotates");
    ui.weak("Del removes the selected one");
}
