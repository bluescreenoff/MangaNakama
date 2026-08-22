use super::*;

pub(crate) fn sec_frame_tool(ui: &mut egui::Ui, app: &mut App) {
    use crate::cmd::FrameMode;
    if app.frame_mode.creates() {
        // CSP Rectangle-frame property block: border on/off + width,
        // fill inside (the White base layer).
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
        ui.checkbox(&mut app.frame_fill_inside, "Fill inside the frame")
            .on_hover_text("adds the White base layer that hides art below the panel");
    } else {
        let (mut lr, mut tb) = if app.frame_mode == FrameMode::DivideBorder {
            app.gutter_border_mm
        } else {
            app.gutter_folder_mm
        };
        let mut changed = false;
        if ValueBar::new("Gutter L/R", 0.0, 20.0)
            .decimals(1)
            .suffix(" mm")
            .show(ui, &mut lr)
            .changed()
        {
            changed = true;
        }
        if ValueBar::new("Gutter T/B", 0.0, 20.0)
            .decimals(1)
            .suffix(" mm")
            .show(ui, &mut tb)
            .changed()
        {
            changed = true;
        }
        if changed {
            if app.frame_mode == FrameMode::DivideBorder {
                app.gutter_border_mm = (lr, tb);
            } else {
                app.gutter_folder_mm = (lr, tb);
            }
        }
        // TRIAGE 128 (FB-026/FB-022): cutting a panel that already has art
        // in it has three answers and CSP makes you say which. Only the
        // folder-making sub tool asks — Divide frame border never touches
        // the layer structure, so the question does not arise there.
        if app.frame_mode == FrameMode::DivideFolder {
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
        // TRIAGE 129 (FB-023..025): equal division, and the tap that runs a
        // panel edge off the page.
        ui.separator();
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
        ui.checkbox(&mut app.frame_div_fit_side, "Fit to side direction")
            .on_hover_text("a tilted panel divides along its own slant, not true vertical");
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
    if ui.button("New frame border folder").clicked() {
        app.push_cmd(AppCmd::NewFrameLayer);
    }
}

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

pub(crate) fn sec_balloon_tail(ui: &mut egui::Ui, app: &mut App) {
    let mut t = app.balloon_tail_mm;
    if ValueBar::new("Tail width", 1.0, 20.0)
        .suffix(" mm")
        .show(ui, &mut t)
        .changed()
    {
        app.balloon_tail_mm = t;
    }
    // `B-005`/`B-006`: what the NEXT tail drag lands as.
    let (mut kind, mut bend) = (app.balloon_tail_kind, app.balloon_tail_bend);
    if balloon_tail_ui(ui, &mut kind, &mut bend).0 {
        app.balloon_tail_kind = kind;
        app.balloon_tail_bend = bend;
    }
}

pub(crate) fn sec_balloon_guide(ui: &mut egui::Ui, app: &mut App) {
    ui.weak(match app.balloon_mode {
        BalloonMode::Draw => "draw the bubble outline — a smooth pressure-aware curve",
        BalloonMode::Tail => "drag from inside a balloon out to the tip",
        _ => "drag out the bubble; O edits it afterwards",
    });
}

/// The Operation tool + a selected BALLOON: edit the bubble itself (the
/// owner's fix 7 — Tool Property edits the selected item).
pub(crate) fn sec_obj_balloon(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, bi)) = app.balloon_sel else {
        return;
    };
    let Some(bs) = app.doc.layers.get(li).and_then(|l| l.balloons()).cloned() else {
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
    // One undo step per drag, like Layer Property's buffer.
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

    // Drawn balloons: CSP's "correct line width" — a render-time multiplier
    // on the outline (Balloon::width_scale, applied at rasterize). The
    // recorded per-anchor pressure widths are DATA: the old implementation
    // rewrote them, saturating at 1.0, so scaling back down returned a flat
    // border instead of the original taper (auditor round 33). The bar is
    // ABSOLUTE — seeded from the balloon's current scale — and commits as
    // one undo step.
    let drawn = bs.balloons.get(bi).is_some_and(|b| match &b.shape {
        mn_core::BalloonShape::Polygon { widths, .. } => !widths.is_empty(),
        _ => false,
    });
    if drawn {
        let cur = bs.balloons[bi].width_scale;
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

    // ROADMAP good-first-issue #1: size the bubble around the lettering that
    // is already in it. One press, one undo step — it commits through the
    // same `BalloonCommit` as every row above.
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

    let anchors = bs.balloons.get(bi).map(|b| match &b.shape {
        mn_core::BalloonShape::Polygon { points, .. } => points.len(),
        _ => 0,
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

// --- balloon ink + tails (TRIAGE 81/82/83) ---------------------------------

/// The colour/opacity/screen controls, written ONCE (`B-001`–`004`, `C-04x`).
///
/// The Balloon tool shows them as the settings a new bubble is born with and
/// the Object tool shows them for the bubble already on the page, so the two
/// can never drift into meaning different things. Returns
/// `(changed_this_frame, finished)` — `finished` is the edge a buffered
/// caller commits on, so a bar drag is one undo step and not forty.
pub(crate) fn balloon_ink_ui(ui: &mut egui::Ui, ink: &mut mn_core::BalloonInk) -> (bool, bool) {
    let (mut changed, mut done) = (false, false);
    ui.horizontal(|ui| {
        ui.weak("line");
        if ui.color_edit_button_srgb(&mut ink.line_color).changed() {
            changed = true;
            done = true;
        }
        ui.weak("fill");
        if ui.color_edit_button_srgb(&mut ink.fill_color).changed() {
            changed = true;
            done = true;
        }
    });
    for (label, v) in [
        ("Line opacity", &mut ink.line_opacity),
        ("Fill opacity", &mut ink.fill_opacity),
    ] {
        let mut pct = *v * 100.0;
        let resp = ValueBar::new(label, 0.0, 100.0)
            .suffix(" %")
            .show(ui, &mut pct);
        if resp.changed() {
            *v = (pct / 100.0).clamp(0.0, 1.0);
            changed = true;
        }
        if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
            done = true;
        }
    }
    // Fill opacity 0 IS CSP's "fill inside frame" switched off — the outline
    // inks and the art behind shows through the bubble. There is no separate
    // checkbox because a fill you cannot see and a fill that is not there are
    // the same balloon.
    if ink.fill_opacity <= 0.0 {
        ui.weak("no fill — the art shows through the bubble");
    }

    // `C-04x`: a screened interior, the printed whisper/flashback bubble.
    let mut toned = ink.fill_tone.is_some();
    if ui
        .checkbox(&mut toned, "Screened fill")
        .on_hover_text("a halftone interior instead of flat paper")
        .changed()
    {
        ink.fill_tone = toned.then(mn_core::BalloonTone::default);
        changed = true;
        done = true;
    }
    if let Some(t) = &mut ink.fill_tone {
        let mut d = t.density * 100.0;
        let resp = ValueBar::new("Density", 0.0, 100.0)
            .suffix(" %")
            .show(ui, &mut d);
        if resp.changed() {
            t.density = (d / 100.0).clamp(0.0, 1.0);
            changed = true;
        }
        done |= resp.drag_stopped() || (resp.changed() && !resp.dragged());

        let mut cell = t.cell_px;
        let resp = ValueBar::new("Cell", 2.0, 40.0)
            .decimals(1)
            .suffix(" px")
            .show(ui, &mut cell);
        if resp.changed() {
            t.cell_px = cell.max(2.0);
            changed = true;
        }
        done |= resp.drag_stopped() || (resp.changed() && !resp.dragged());

        let mut ang = t.angle_deg;
        let resp = ValueBar::new("Angle", 0.0, 90.0)
            .suffix("°")
            .show(ui, &mut ang);
        if resp.changed() {
            t.angle_deg = ang;
            changed = true;
        }
        done |= resp.drag_stopped() || (resp.changed() && !resp.dragged());

        ui.horizontal(|ui| {
            ui.weak("pattern");
            egui::ComboBox::from_id_salt("mn.balloon.tone.pattern")
                .width(96.0)
                .selected_text(t.pattern.label())
                .show_ui(ui, |ui| {
                    for pat in mn_core::TonePattern::ALL {
                        if ui.selectable_label(t.pattern == pat, pat.label()).clicked() {
                            t.pattern = pat;
                            changed = true;
                            done = true;
                        }
                    }
                });
        });
        // The cell is stored in canvas px, so it does NOT re-flow when the
        // document dpi changes afterwards. Said out loud rather than hidden.
        ui.weak("cell is in canvas px — a later dpi change does not re-flow it");
    }
    (changed, done)
}

/// The tail SHAPE controls (`B-005`, `B-006`), also written once.
pub(crate) fn balloon_tail_ui(
    ui: &mut egui::Ui,
    kind: &mut mn_core::TailKind,
    bend: &mut f32,
) -> (bool, bool) {
    let (mut changed, mut done) = (false, false);
    ui.horizontal(|ui| {
        ui.weak("type");
        for k in mn_core::TailKind::ALL {
            if ui.selectable_label(*kind == k, k.label()).clicked() && *kind != k {
                *kind = k;
                changed = true;
                done = true;
            }
        }
    });
    let mut b = *bend;
    let resp = ValueBar::new("Bend", -0.6, 0.6)
        .decimals(2)
        .show(ui, &mut b);
    if resp.changed() {
        *bend = b.clamp(-0.6, 0.6);
        changed = true;
    }
    done |= resp.drag_stopped() || (resp.changed() && !resp.dragged());
    if *bend != 0.0 {
        ui.weak("the tail curves around the art instead of stabbing through it");
    }
    (changed, done)
}

/// Balloon tool: what a NEW bubble is inked with (`C-039`–`048`).
pub(crate) fn sec_balloon_ink(ui: &mut egui::Ui, app: &mut App) {
    let mut ink = app.balloon_ink;
    if balloon_ink_ui(ui, &mut ink).0 {
        app.balloon_ink = ink;
    }
    if ink != mn_core::BalloonInk::default() && ui.button("Back to black on white").clicked() {
        app.balloon_ink = mn_core::BalloonInk::default();
    }
}

/// The Operation tool + a selected BALLOON: repaint the bubble on the page.
/// Buffered through `App::ink_edit` so one bar drag is one undo step.
pub(crate) fn sec_obj_ink(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, bi)) = app.balloon_sel else {
        return;
    };
    let Some(bs) = app.doc.layers.get(li).and_then(|l| l.balloons()).cloned() else {
        return;
    };
    let Some(cur) = bs.balloons.get(bi).map(|b| b.ink()) else {
        return;
    };
    let mut ink = app.ink_edit.unwrap_or(cur);
    let (changed, done) = balloon_ink_ui(ui, &mut ink);
    if changed {
        app.ink_edit = Some(ink);
    }
    if done {
        if let Some(ink) = app.ink_edit.take() {
            let mut bs2 = bs.clone();
            bs2.balloons[bi].set_ink(ink);
            app.push_cmd(AppCmd::BalloonCommit {
                layer: li,
                balloons: bs2,
            });
            // The tool remembers what you just chose, so the next bubble
            // matches the one you have been styling.
            app.balloon_ink = ink;
        }
    }
}

/// The Operation tool + a selected BALLOON: the shape of its tails.
///
/// It edits the balloon, not one tail — the panel has no tail selection and a
/// bubble with two tails wants them matching. A hand-mixed pair (possible
/// only by editing two tails in turn) shows the tool's own setting rather
/// than picking one of them to call the truth.
pub(crate) fn sec_obj_tail(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, bi)) = app.balloon_sel else {
        return;
    };
    let Some(bs) = app.doc.layers.get(li).and_then(|l| l.balloons()).cloned() else {
        return;
    };
    let Some(b) = bs.balloons.get(bi) else { return };
    if b.tails.is_empty() {
        ui.weak("no tail yet — the Balloon tool's Tail mode drags one out");
        return;
    }
    let (mut kind, mut bend) = b
        .tail_style()
        .unwrap_or((app.balloon_tail_kind, app.balloon_tail_bend));
    if balloon_tail_ui(ui, &mut kind, &mut bend).1 {
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

/// The Operation tool + a selected PANEL: edit the frame border.
pub(crate) fn sec_obj_frame(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, _fi)) = app.object_sel else {
        return;
    };
    let Some(fs) = app.doc.layers.get(li).and_then(|l| l.frames()).cloned() else {
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
    ui.weak(format!(
        "{} panel(s) — drag vertices/edges; the box scales",
        fs.frames.len()
    ));
}

pub(crate) fn sec_obj_guide(ui: &mut egui::Ui, _app: &mut App) {
    ui.weak("click a text box, balloon or panel");
    ui.weak("drag moves; handles reshape; the blue box scales/rotates");
    ui.weak("Del removes the selected one");
}

pub(crate) fn sec_figure(ui: &mut egui::Ui, app: &mut App) {
    use crate::cmd::FigureMode;
    match app.figure_mode {
        FigureMode::Stream | FigureMode::Focus => {
            // The effect-line knobs: what the NEXT drag generates with.
            // Ranges mirror the generator dialog's (its clamp rationale —
            // giant counts/widths were a real UI hang — applies here too).
            let focus = app.figure_mode == FigureMode::Focus;
            let o = if focus {
                &mut app.figure_focus
            } else {
                &mut app.figure_stream
            };
            ui.horizontal(|ui| {
                ui.label("Lines");
                ui.add(egui::DragValue::new(&mut o.count).range(1..=512));
            });
            ui.horizontal(|ui| {
                ui.label("Width");
                ui.add(
                    egui::DragValue::new(&mut o.width)
                        .range(0.5..=40.0)
                        .speed(0.1),
                );
            });
            if focus {
                ui.horizontal(|ui| {
                    ui.label("Jitter");
                    ui.add(
                        egui::DragValue::new(&mut o.jitter)
                            .range(0.0..=1.0)
                            .speed(0.01),
                    )
                    .on_hover_text("angle, width and length wobble — 0 is a drafting tool's fan");
                });
                ui.horizontal(|ui| {
                    ui.label("Hollow centre");
                    ui.add(
                        egui::DragValue::new(&mut o.r_in_frac)
                            .range(0.0..=0.95)
                            .speed(0.01),
                    )
                    .on_hover_text("the empty middle, as a fraction of the dragged radius");
                });
            }
            ui.weak("each drag places its own layer — one undo press removes it");
        }
        _ => {
            ui.checkbox(&mut app.figure_fill, "Fill with drawing colour")
                .on_hover_text("closed shapes fill before the outline inks");
        }
    }
}

pub(crate) fn sec_figure_guide(ui: &mut egui::Ui, app: &mut App) {
    use crate::cmd::FigureMode;
    ui.weak(match app.figure_mode {
        FigureMode::Line => "drag start to end; Shift snaps to 45° steps",
        FigureMode::Rect => "drag corner to corner; Shift keeps it square",
        FigureMode::Ellipse => "drag the bounding box; Shift keeps it round",
        FigureMode::Polygon => "click vertices; the first one / Enter closes, Esc cancels",
        FigureMode::Stream => "drag along the motion — angle and length come from the drag",
        FigureMode::Focus => "drag from the convergence point out to the lines' reach",
    });
    match app.figure_mode {
        FigureMode::Stream | FigureMode::Focus => {
            ui.weak("adjust later: Object tool handles, or Layer ▸ effect lines");
        }
        _ => {
            ui.weak("inked with the active brush — Size/Opacity above apply");
        }
    }
}

// --- the gradient ramp editor (CSP's Edit gradient, `G-001`/`G-008`/
// `G-013`/`G-014`) -------------------------------------------------------
//
// Built INTO Tool Property rather than as a separate modal: the same rows
// then serve the gradient TOOL and a live gradient LAYER's parameters, and
// authoring a ramp with the canvas visible beats authoring it behind a
// dialog. The ends are pinned at the ends of the drag; interior stops are
// what you drag, which is the deviation from CSP worth knowing about.
