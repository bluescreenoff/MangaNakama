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
    // CSP's frame styling (workflow walk #1, item 33): a selected frame's
    // border takes a main colour — black by default, black in old files.
    {
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
    // CSP's "Keep gutters aligned" (audit P0-4): All = dragging a border
    // brings the facing border of the panel across the gutter with it, so
    // the gap keeps its width. None = the one edge moves and the gutter
    // narrows.
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
    ui.weak(format!(
        "{} panel(s) — drag vertices/edges; the box scales",
        fs.frames.len()
    ));
}

// --- the SELECTED effect-line run (owner report 2026-08-23) -------------
//
// Tool Property edits the item you picked, and a generated 流線/集中線 set
// was the one item it refused to. These two sections edit the LAYER'S OWN
// `GenLinesSpec` — not the Figure tool's defaults, which is what the sub
// tool rows do — and commit through `GenLinesApplyTo` on the release edge
// of a drag. Never per frame: a regen re-rasterizes the whole layer, and
// a page-sized burst at 600 dpi is not a per-mouse-move operation.

/// The draft spec the widgets mutate, and the layer it belongs to.
fn gen_draft(app: &App) -> Option<(usize, mn_core::genlines::GenLinesSpec)> {
    let li = app.gen_sel?;
    let stored = app.doc.layers.get(li)?.genlines?;
    Some((li, app.gen_edit.unwrap_or(stored)))
}

/// Push the draft (drag release) or keep it (mid-drag). One helper so no
/// row can forget which of the two it is doing.
///
/// A frame in which NOTHING changed must not re-arm the draft: the canvas
/// handles write the same spec, and a stale draft left sitting here would
/// shadow their result the next time the palette drew.
fn gen_commit(
    app: &mut App,
    li: usize,
    spec: mn_core::genlines::GenLinesSpec,
    changed: bool,
    done: bool,
) {
    if done {
        app.gen_edit = None;
        if Some(spec) != app.doc.layers.get(li).and_then(|l| l.genlines) {
            app.push_cmd(AppCmd::GenLinesApplyTo { layer: li, spec });
        }
    } else if changed {
        app.gen_edit = Some(spec);
    }
}

/// A ValueBar row over one `f32` of the draft: returns (changed, done).
fn gen_bar(
    ui: &mut egui::Ui,
    label: &str,
    range: (f32, f32),
    decimals: usize,
    suffix: &str,
    v: &mut f32,
) -> (bool, bool) {
    let resp = ValueBar::new(label, range.0, range.1)
        .decimals(decimals)
        .suffix(suffix)
        .show(ui, v);
    (
        resp.changed(),
        resp.drag_stopped() || (resp.changed() && !resp.dragged()),
    )
}

/// Shape: what kind of set it is, how far it reaches, how it tapers.
pub(crate) fn sec_obj_genlines(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, mut s)) = gen_draft(app) else {
        return;
    };
    let (mut changed, mut done) = (false, false);

    // Kind, but only inside the RADIAL family: 集中線, ウニフラッシュ and
    // ベタフラッシュ read a, b, c, d identically (centre, r_in, r_out), so
    // switching between them is a re-render. A 流線 spec means something
    // else by the same four numbers, so it is not offered as a swap — that
    // would silently reinterpret the geometry.
    if s.radial() {
        ui.horizontal(|ui| {
            for (k, label) in [(0u8, "Saturated"), (1, "Urchin"), (2, "Solid")] {
                if ui.selectable_label(s.kind == k, label).clicked() && s.kind != k {
                    s.kind = k;
                    s.focus = true;
                    changed = true;
                    done = true;
                }
            }
        });
    }

    ui.horizontal(|ui| {
        ui.weak("colour");
        // White knockout lines over a black panel — the generator inked
        // black only until this existed.
        if ui.color_edit_button_srgb(&mut s.color).changed() {
            changed = true;
            done = true;
        }
        if s.color != [0, 0, 0] && ui.small_button("black").clicked() {
            s.color = [0, 0, 0];
            changed = true;
            done = true;
        }
    });

    let px_per_mm = app.mm_to_px(1.0).max(0.001);
    let flash = s.kind == 1 || s.kind == 2;
    let mut w_mm = s.width / px_per_mm;
    let (c, d) = gen_bar(
        ui,
        if flash { "Spike width" } else { "Width" },
        (0.02, if flash { 4.0 } else { 1.5 }),
        2,
        " mm",
        &mut w_mm,
    );
    if c {
        s.width = (w_mm * px_per_mm).max(0.5);
    }
    changed |= c;
    done |= d;

    if !flash {
        let (c, d) = gen_bar(ui, "Taper", (0.0, 1.0), 2, "", &mut s.taper);
        changed |= c;
        done |= d;
    }

    if s.radial() {
        // The hole, as a fraction of the reach — the same knob the sub
        // tool row arms, so the two mean one thing.
        let mut frac = if s.d > 0.0 { s.c / s.d } else { 0.0 };
        let (c, d) = gen_bar(ui, "Hollow centre", (0.0, 0.95), 2, "", &mut frac);
        if c {
            s.c = s.d * frac.clamp(0.0, 0.95);
        }
        changed |= c;
        done |= d;
        let (c2, d2) = gen_bar(ui, "Reach", (8.0, 6000.0), 0, " px", &mut s.d);
        if c2 {
            s.c = s.c.min((s.d - 4.0).max(0.0));
        }
        changed |= c2;
        done |= d2;
    } else {
        let (c, d) = gen_bar(ui, "Angle", (-180.0, 180.0), 1, "°", &mut s.a);
        changed |= c;
        done |= d;
        let (c2, d2) = gen_bar(ui, "Shortest", (8.0, 6000.0), 0, " px", &mut s.b);
        changed |= c2;
        done |= d2;
        let (c3, d3) = gen_bar(ui, "Longest", (8.0, 6000.0), 0, " px", &mut s.c);
        changed |= c3;
        done |= d3;
        if s.b > s.c {
            s.b = s.c;
        }
        // 流線 with a vanishing point: the subtle fan a perspective panel
        // wants. Off = pure parallel.
        let mut fan = s.converge.is_some();
        if ui
            .checkbox(&mut fan, "Fan toward a point")
            .on_hover_text("aims every run at one canvas point instead of running parallel")
            .changed()
        {
            s.converge = fan.then(|| {
                let a = crate::app::canvas_input::gen_anchor(&s, app.doc.size);
                [a[0], a[1] - app.doc.size.1 as f32 * 4.0]
            });
            changed = true;
            done = true;
        }
        if let Some(v) = &mut s.converge {
            ui.horizontal(|ui| {
                ui.label("Point");
                let a = ui.add(egui::DragValue::new(&mut v[0]).speed(4.0));
                let b = ui.add(egui::DragValue::new(&mut v[1]).speed(4.0));
                changed |= a.changed() || b.changed();
                done |= a.drag_stopped() || b.drag_stopped();
            });
        }
    }

    if ui
        .button("Reroll")
        .on_hover_text("the same parameters, a different draw of the random wobble")
        .clicked()
    {
        s.seed = s.seed.wrapping_add(1);
        changed = true;
        done = true;
    }
    gen_commit(app, li, s, changed, done);
}

/// Density: the gap between lines, the bundling, and the wobble.
pub(crate) fn sec_obj_genlines_density(ui: &mut egui::Ui, app: &mut App) {
    let Some((li, mut s)) = gen_draft(app) else {
        return;
    };
    let (mut changed, mut done) = (false, false);
    let px_per_mm = app.mm_to_px(1.0).max(0.001);

    if s.radial() {
        // GAP, not count: a manga tutorial sizes a 集中線 in degrees
        // (≈3° dense, ≈10° sparse) and that number means the same thing
        // whatever the page is. The count is still shown, because every
        // set placed before this was made of one.
        let mut by_gap = s.gap_deg > 0.0;
        if ui
            .checkbox(&mut by_gap, "Space by angle")
            .on_hover_text("a gap in degrees instead of a total count — 3° dense, 10° sparse")
            .changed()
        {
            s.gap_deg = if by_gap {
                360.0 / s.count.max(1) as f32
            } else {
                0.0
            };
            if !by_gap {
                s.count = s.ray_count();
            }
            changed = true;
            done = true;
        }
        if by_gap {
            let (c, d) = gen_bar(ui, "Gap", (0.5, 30.0), 2, "°", &mut s.gap_deg);
            changed |= c;
            done |= d;
            ui.weak(format!("{} lines", s.ray_count()));
        } else {
            let mut n = s.count as f32;
            let (c, d) = gen_bar(ui, "Lines", (1.0, 512.0), 0, "", &mut n);
            if c {
                s.count = (n.round() as u32).max(1);
            }
            changed |= c;
            done |= d;
        }
    } else {
        let mut by_gap = s.gap_px > 0.0;
        if ui
            .checkbox(&mut by_gap, "Space evenly")
            .on_hover_text(
                "walk the block at a fixed gap instead of scattering runs at random — \
                 the random scatter clumps, which is what makes a generated set read as noise",
            )
            .changed()
        {
            s.gap_px = if by_gap { px_per_mm } else { 0.0 };
            changed = true;
            done = true;
        }
        if by_gap {
            let mut gap_mm = s.gap_px / px_per_mm;
            let (c, d) = gen_bar(ui, "Gap", (0.1, 10.0), 2, " mm", &mut gap_mm);
            if c {
                s.gap_px = (gap_mm * px_per_mm).max(0.25);
            }
            changed |= c;
            done |= d;

            // まとまり — bundles with a hole between them. CSP's own
            // biggest quality lever for a speed block.
            let mut n = s.group as f32;
            let (c, d) = gen_bar(ui, "Bundle", (0.0, 16.0), 0, "", &mut n);
            if c {
                s.group = n.round() as u32;
            }
            changed |= c;
            done |= d;
            if s.group > 1 {
                let (c, d) = gen_bar(ui, "Bundle gap", (1.0, 8.0), 1, " ×", &mut s.group_gap);
                changed |= c;
                done |= d;
            } else {
                ui.weak("0 or 1 = one even block, no bundles");
            }
        } else {
            let mut n = s.count as f32;
            let (c, d) = gen_bar(ui, "Lines", (1.0, 512.0), 0, "", &mut n);
            if c {
                s.count = (n.round() as u32).max(1);
            }
            changed |= c;
            done |= d;
        }
    }

    // The three wobbles. 0 means "use the single old Jitter", so the row
    // shows that value rather than pretending the set has none.
    let single = s.jitter;
    for (label, v, hint) in [
        ("Position", &mut s.jit_gap, "how far each line strays"),
        ("Length", &mut s.jit_len, "how much the lengths vary"),
        ("Width", &mut s.jit_width, "how much the weights vary"),
    ] {
        let mut shown = if *v > 0.0 { *v } else { single };
        let resp = ValueBar::new(label, 0.0, 1.0)
            .decimals(2)
            .show(ui, &mut shown);
        if resp.changed() {
            *v = shown;
            changed = true;
        }
        done |= resp.drag_stopped() || (resp.changed() && !resp.dragged());
        resp.on_hover_text(hint);
    }
    gen_commit(app, li, s, changed, done);
}

pub(crate) fn sec_obj_guide(ui: &mut egui::Ui, _app: &mut App) {
    ui.weak("click a text box, balloon, panel or effect-line set");
    ui.weak("drag moves; handles reshape; the blue box scales/rotates");
    ui.weak("Del removes the selected one");
}

pub(crate) fn sec_figure(ui: &mut egui::Ui, app: &mut App) {
    match app.figure_mode {
        m if m.generates() => {
            // The effect-line knobs: what the NEXT drag generates with.
            // Ranges mirror the generator dialog's (its clamp rationale —
            // giant counts/widths were a real UI hang — applies here too).
            let radial = m.radial();
            let flash = m.gen_kind() != 0;
            let px_per_mm = app.mm_to_px(1.0).max(0.001);
            let o = if radial {
                &mut app.figure_focus
            } else {
                &mut app.figure_stream
            };
            // Density is stated as a GAP where the preset says so — a
            // count field that the generator ignores is worse than no
            // field, and the gap is the unit a tutorial uses anyway.
            if !flash && radial && o.gap_deg > 0.0 {
                ui.horizontal(|ui| {
                    ui.label("Gap");
                    ui.add(
                        egui::DragValue::new(&mut o.gap_deg)
                            .range(0.5..=30.0)
                            .speed(0.1)
                            .suffix("°"),
                    )
                    .on_hover_text("degrees between rays — 3° dense, 10° sparse");
                });
                ui.weak(format!("{} lines", (360.0 / o.gap_deg).ceil() as u32));
            } else if !flash && !radial && o.gap_px > 0.0 {
                let mut gap_mm = o.gap_px / px_per_mm;
                ui.horizontal(|ui| {
                    ui.label("Gap");
                    if ui
                        .add(
                            egui::DragValue::new(&mut gap_mm)
                                .range(0.1..=10.0)
                                .speed(0.02)
                                .suffix(" mm"),
                        )
                        .on_hover_text("spacing between runs — they walk the block evenly")
                        .changed()
                    {
                        o.gap_px = (gap_mm * px_per_mm).max(0.25);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Bundle");
                    ui.add(egui::DragValue::new(&mut o.group).range(0..=16))
                        .on_hover_text("まとまり — runs per bundle, with a hole between bundles");
                    if o.group > 1 {
                        ui.add(
                            egui::DragValue::new(&mut o.group_gap)
                                .range(1.0..=8.0)
                                .speed(0.1)
                                .suffix(" ×"),
                        )
                        .on_hover_text("how wide the hole is, in gaps");
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    ui.label(if flash { "Spikes" } else { "Lines" });
                    ui.add(egui::DragValue::new(&mut o.count).range(1..=512));
                });
            }
            ui.horizontal(|ui| {
                ui.label(if flash { "Spike width" } else { "Width" });
                // A flash's width is the spike BASE, so it needs a range
                // a line never does; the renderer still caps it at the
                // gap between neighbours so the teeth cannot merge.
                ui.add(
                    egui::DragValue::new(&mut o.width)
                        .range(if flash { 1.0..=200.0 } else { 0.5..=40.0 })
                        .speed(0.1),
                )
                .on_hover_text(if flash {
                    "how wide each spike is at the rim, in pixels"
                } else {
                    "line thickness in pixels"
                });
            });
            // Stream tails and Focus rays both taper (a printed 集中線
            // needles at the convergence); the flash kinds' teeth carry
            // their own shape and get no knob.
            if !flash {
                ui.horizontal(|ui| {
                    ui.label("Taper");
                    ui.add(
                        egui::DragValue::new(&mut o.taper)
                            .range(0.0..=1.0)
                            .speed(0.01),
                    )
                    .on_hover_text(if radial {
                        "how far each ray thins toward the convergence — 0 flat, 1 a needle point"
                    } else {
                        "how far each line thins toward its tail — 0 is a flat run, 1 a needle point"
                    });
                });
            }
            // ONE wobble knob, writing all four fields. The presets set
            // the three split jitters to different values (a printed set
            // wants a lot of length wobble and little angular wobble),
            // and a split jitter overrides the single one — so a Jitter
            // row that wrote only `jitter` would look live and do
            // nothing. Turning this evens them out, deliberately.
            ui.horizontal(|ui| {
                ui.label("Jitter");
                let mut j = o.jit_gap.max(o.jitter);
                if ui
                    .add(egui::DragValue::new(&mut j).range(0.0..=1.0).speed(0.01))
                    .on_hover_text("angle, width and length wobble — 0 is a drafting tool's fan")
                    .changed()
                {
                    o.jitter = j;
                    o.jit_gap = j;
                    o.jit_len = j;
                    o.jit_width = j;
                }
            });
            if radial {
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
        FigureMode::Urchin => "drag from the flash's centre out to the spikes' reach",
        FigureMode::SolidFlash => "drag from the hole's centre out to the ring's rim",
    });
    if app.figure_mode.generates() {
        ui.weak("adjust later: Object tool handles, or Layer ▸ effect lines");
    } else {
        ui.weak("inked with the active brush — Size/Opacity above apply");
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
