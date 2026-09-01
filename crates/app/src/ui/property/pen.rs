use super::*;

pub(crate) fn pen_property(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(app.brush_name().to_owned())
                .size(11.5)
                .color(theme::c().text_strong),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_btn(
                ui,
                Icon::Wrench,
                15.0,
                app.detail_open,
                true,
                "Sub Tool Detail",
            )
            .clicked()
            {
                app.detail_open = !app.detail_open;
            }
            // BR-005: the rows eye — per sub tool, which sliders stay.
            let eye = icon_btn(
                ui,
                Icon::Eye,
                15.0,
                app.pen_rows_open,
                true,
                "Show or hide rows (per sub tool)",
            );
            // The open flag goes through a local so `open_bool`'s borrow
            // cannot cross the popup body's own `app` borrows.
            let mut rows_open = app.pen_rows_open;
            egui::Popup::from_response(&eye)
                .open_bool(&mut rows_open)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .width(170.0)
                .show(|ui| {
                    for (id, label) in PEN_ROW_IDS {
                        let mut on = app.pen_row_visible(id);
                        if ui.checkbox(&mut on, *label).changed() {
                            app.set_pen_row(id, on);
                        }
                    }
                });
            app.pen_rows_open = rows_open;
            // TL-013, CSP's own model: a locked sub tool still takes every
            // change — it just does not REMEMBER them. Leave it and come
            // back and the values below are the ones the padlock froze.
            // The lock lives in ToolProps, so it follows the sub tool:
            // locking the inking pen leaves the eraser free.
            let locked = app.props_current.locked;
            if icon_btn(
                ui,
                Icon::Lock,
                15.0,
                locked,
                true,
                if locked {
                    "Settings LOCKED — changes last until you leave this sub tool.\n\
                     Click to make the values below its own again."
                } else {
                    "Lock these settings — nudge them freely, they come back\n\
                     the next time you select this sub tool"
                },
            )
            .clicked()
            {
                app.push_cmd(AppCmd::SetToolLock(!locked));
            }
        });
    });
    ui.add_space(1.0);
    test_stroke_strip(ui, app);
    brush_sliders(ui, app);
    eraser_rows(ui, app);
    group_caption(ui, "Dynamics");
    dynamics_editor(ui, app);
}

/// Row 95 (BR-022): the Eraser's own option. Only the Eraser shows it —
/// the flag itself asks `eraser_active()` at stroke time, so a transparent
/// slot or a flipped stylus obeys it too, but a checkbox on the inking pen
/// saying "erase on every layer" would read as a threat.
fn eraser_rows(ui: &mut egui::Ui, app: &mut App) {
    if app.tool != crate::cmd::Tool::Eraser {
        return;
    }
    ui.checkbox(&mut app.erase_all_layers, "Erase on every layer")
        .on_hover_text(
            "One rub takes the same pixels off every visible, unlocked layer \
             that keeps pixels — sketch, flats and ink together — as ONE undo \
             press. Folders, vector and live layers are never touched.",
        );
    vector_eraser_row(ui, app);
}

/// CSP's Vector eraser modes, on the Eraser's own property page. A raster
/// layer erases touched pixels whatever this says (CSP: "when used on a
/// raster layer, vector erasers will only erase touched areas"), so the row
/// stays visible and the hover says who it speaks to rather than blinking
/// in and out as the artist walks the layer stack.
fn vector_eraser_row(ui: &mut egui::Ui, app: &mut App) {
    use mn_core::EraserMode as M;
    const MODES: &[(M, &str, &str)] = &[
        (
            M::Touched,
            "Touched",
            "Erase touched areas — only the part of the line the eraser \
             rubbed goes; rubbing through the middle leaves two lines.",
        ),
        (
            M::ToIntersection,
            "To intersection",
            "Erase up to intersection — the rub widens out to where the \
             line crosses another line on this layer (its own crossings \
             count), or to the line's ends if it crosses nothing. The \
             overshooting hatch mark, gone in one dab.\n\
             Refer all layers is not built: crossings are this layer's.",
        ),
        (
            M::WholeLine,
            "Whole line",
            "Whole line — every line the eraser touches goes entirely.",
        ),
    ];
    ui.horizontal(|ui| {
        ui.weak("Vector eraser").on_hover_text(
            "What an eraser stroke takes off a VECTOR layer. Raster layers \
             always erase what you touched.",
        );
        for (mode, label, hover) in MODES {
            ui.selectable_value(&mut app.vector_eraser_mode, *mode, *label)
                .on_hover_text(*hover);
        }
    });
}

/// BR-005: the gateable pen rows, in display order. The id is the
/// persistence key (ui.txt `hidden_rows`); the label is what the eye
/// popup shows. Rows NOT in this list are always visible.
const PEN_ROW_IDS: &[(&str, &str)] = &[
    ("size", "Size"),
    ("min_size", "Min size"),
    ("opacity", "Opacity"),
    ("stabilize", "Stabilize"),
    ("correction", "Correction"),
    ("randomize", "Randomize"),
    ("min_rand", "Min rand"),
    ("random_abs", "Fixed px"),
    ("tip", "Tip"),
    ("anti_alias", "Anti-alias"),
    ("interval", "Interval"),
    ("scatter", "Scatter"),
    ("sketch", "Sketch"),
    ("ink", "Ink"),
    ("flow", "Flow"),
    ("mixing", "Color mixing"),
    ("water_edge", "Watercolor edge"),
    ("jitter", "Color jitter"),
    ("texture", "Texture"),
    ("tip_flip", "Tip flip"),
];

/// The LIVE test stroke: one S-curve with a synthesized pressure ramp, re-inked
/// with the current preset and its live overrides whenever any of them moves.
///
/// CSP's own answer to "what does this do?" is sixteen collapsed parameter
/// pages and a canvas to scribble on; this is the scribble, in the panel, on a
/// throwaway document — the artwork is never touched (see
/// [`crate::ui::preview::test_stroke_image`], which builds a FRESH engine per
/// render for the libmypaint-state reason recorded in docs/CODE-MAP.md).
pub(crate) fn test_stroke_strip(ui: &mut egui::Ui, app: &mut App) {
    let hidden = app.layout.test_stroke_hidden;
    ui.horizontal(|ui| {
        if icon_btn(
            ui,
            if hidden { Icon::EyeOff } else { Icon::Eye },
            13.0,
            false,
            true,
            if hidden {
                "Show the live test stroke"
            } else {
                "Hide the live test stroke"
            },
        )
        .clicked()
        {
            app.layout.note_test_stroke_hidden(!hidden);
        }
        ui.weak("Test stroke");
        // A brush wider than the strip is shown zoomed OUT, and says so — a
        // preview at an unstated scale is worse than no preview.
        let k = crate::ui::preview::test_stroke_scale(app.props_current.size_px);
        if k > 1 && !hidden {
            ui.weak(format!("1:{k}"));
        }
    });
    if hidden {
        return;
    }
    let w = ui.available_width();
    let tex = app.test_stroke_tex(w);
    let size = tex.size_vec2();
    ui.add(egui::Image::new(&tex).fit_to_exact_size(size))
        .on_hover_text(
            "The current brush on scrap paper: an S-curve with pressure \
ramping 0 → full → 0. Every control below re-inks it, so a value can be judged \
without drawing on the page. A brush too wide for the strip is zoomed out — \
the caption says by how much.",
        );
    ui.add_space(2.0);
}

/// Krita per-sensor curve editor: pick a dynamic and the sensor that drives
/// it, then drag the response curve. Points live in RAW setting/input units
/// (Size's y is ln of the radius factor); the plot maps through the pair's
/// axis ranges, so what you drag is what libmypaint evaluates.
pub(crate) fn dynamics_editor(ui: &mut egui::Ui, app: &mut App) {
    use crate::cmd::{CurveSensor, CurveSetting};
    use egui::{Pos2, pos2};

    const PICK_RADIUS: f32 = 9.0;

    ui.horizontal(|ui| {
        let mut cs = app.curve_setting;
        egui::ComboBox::from_id_salt("curve-setting")
            .selected_text(cs.label())
            .width(84.0)
            .show_ui(ui, |ui| {
                for s in CurveSetting::ALL {
                    ui.selectable_value(&mut cs, s, s.label());
                }
            });
        let mut sn = app.curve_sensor;
        egui::ComboBox::from_id_salt("curve-sensor")
            .selected_text(sn.label())
            .width(86.0)
            .show_ui(ui, |ui| {
                for s in CurveSensor::ALL {
                    ui.selectable_value(&mut sn, s, s.label());
                }
            });
        let picked = cs != app.curve_setting || sn != app.curve_sensor;
        if picked {
            app.curve_setting = cs;
            app.curve_sensor = sn;
            app.curve_drag = None;
        }
    });

    // Between edits the engine is the truth; during a drag the widget owns
    // the points and commits once at release.
    if app.curve_drag.is_none() {
        if let (Some(sid), Some(iid)) =
            (app.curve_setting.setting_id(), app.curve_sensor.input_id())
        {
            app.curve_edit_points = app.engine().mapping(sid, iid);
        }
    }
    let (x0, x1) = app.curve_sensor.x_range();
    let (y0, y1) = app.curve_setting.y_range();
    let (xr, yr) = (x1 - x0, y1 - y0);

    let size = egui::vec2(ui.available_width().max(120.0), 108.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let to_px = |p: (f32, f32)| -> Pos2 {
        let nx = ((p.0 - x0) / xr).clamp(0.0, 1.0);
        let ny = ((p.1 - y0) / yr).clamp(0.0, 1.0);
        pos2(
            rect.left() + nx * rect.width(),
            rect.bottom() - ny * rect.height(),
        )
    };
    let from_px = |pos: Pos2| -> (f32, f32) {
        let nx = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let ny = ((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0);
        (x0 + nx * xr, y0 + ny * yr)
    };

    // Chrome: field, zero-lines, then the curve.
    painter.rect_filled(rect, 2.0, theme::c().field);
    let mid_y = to_px((0.0, 0.0));
    if rect.top() < mid_y.y && mid_y.y < rect.bottom() {
        painter.line_segment(
            [pos2(rect.left(), mid_y.y), pos2(rect.right(), mid_y.y)],
            egui::Stroke::new(1.0, theme::c().outline),
        );
    }
    if x0 < 0.0 {
        let mid_x = to_px((0.0, 0.0));
        painter.line_segment(
            [pos2(mid_x.x, rect.top()), pos2(mid_x.x, rect.bottom())],
            egui::Stroke::new(1.0, theme::c().outline),
        );
    }
    if app.curve_edit_points.len() >= 2 {
        let stroke = egui::Stroke::new(2.0, theme::c().accent);
        for pair in app.curve_edit_points.windows(2) {
            painter.line_segment([to_px(pair[0]), to_px(pair[1])], stroke);
        }
    }
    for (i, p) in app.curve_edit_points.iter().enumerate() {
        let pos = to_px(*p);
        let hot = app.curve_drag == Some(i);
        painter.circle_filled(pos, if hot { 5.0 } else { 3.5 }, theme::c().accent);
        if hot {
            painter.circle_stroke(pos, 5.0, egui::Stroke::new(1.5, theme::c().text_strong));
        }
    }

    // --- interactions ------------------------------------------------------
    let push_curve = |app: &mut App| {
        app.push_cmd(AppCmd::SetCurve {
            setting: app.curve_setting as u8,
            sensor: app.curve_sensor as u8,
            points: app.curve_edit_points.clone(),
        });
    };
    if let Some(pos) = resp.interact_pointer_pos() {
        let nearest = app
            .curve_edit_points
            .iter()
            .enumerate()
            .map(|(i, p)| (i, to_px(*p).distance(pos)))
            .min_by_key(|&(_, d)| d.to_bits());

        if resp.drag_started() {
            app.curve_drag = match nearest {
                Some((i, d)) if d <= PICK_RADIUS => Some(i),
                // Click on empty canvas inserts a point there (sorted).
                _ => {
                    let mut p = from_px(pos);
                    p.1 = p.1.clamp(y0, y1);
                    let at = app.curve_edit_points.partition_point(|q| q.0 < p.0);
                    app.curve_edit_points.insert(at, p);
                    Some(at)
                }
            };
        } else if resp.dragged() {
            if let Some(i) = app.curve_drag {
                let mut p = from_px(pos);
                // Keep x sorted: clamp between the neighbours (a hair of
                // the range each side so points can be pulled apart).
                let lo = i
                    .checked_sub(1)
                    .map(|j| app.curve_edit_points[j].0 + 0.01 * xr)
                    .unwrap_or(x0);
                let hi = app
                    .curve_edit_points
                    .get(i + 1)
                    .map(|q| q.0 - 0.01 * xr)
                    .unwrap_or(x1);
                p.0 = p.0.clamp(lo.min(hi), hi.max(lo));
                app.curve_edit_points[i] = p;
            }
        } else if resp.secondary_clicked() {
            if let Some((i, d)) = nearest
                && d <= PICK_RADIUS
                && app.curve_edit_points.len() > 1
            {
                app.curve_edit_points.remove(i);
                app.curve_drag = None;
                push_curve(app);
            }
        }
    }
    if resp.drag_stopped() {
        app.curve_drag = None;
        push_curve(app);
    }
    resp.on_hover_text(
        "Per-sensor curve (Krita): how the dynamic responds to the \
sensor. Drag points; click empty space to add one; right-click removes. Size's \
axis is logarithmic — -0.69 halves, +0.69 doubles. Empty = no response: click to \
start one (one point is a constant).",
    );
}

// --- section bodies ------------------------------------------------------

/// CSP's Correction group past the Stabilize slider: post correction and its
/// two modulations, the sharp-angle exception, the stabilization mode, and the
/// entry/exit shaping. Per sub tool, like everything else in this panel.
///
/// One command carries the whole group — the panel edits a copy of the current
/// [`mn_core::stabilize::CorrectCfg`] and sends it, so adding a dial here never
/// needs a new `AppCmd`.
pub(crate) fn correction_rows(ui: &mut egui::Ui, app: &mut App) {
    use mn_core::stabilize::{CorrectCfg, MAX_SE_PX, SeHow, StabMode};

    let cur = app.props_current.correct;
    let mut c: CorrectCfg = cur;

    group_caption(ui, "Correction");

    let mut post = c.post * 100.0;
    if ValueBar::new("Post correct", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut post)
        .changed()
    {
        c.post = post / 100.0;
    }
    ui.horizontal(|ui| {
        ui.checkbox(&mut c.post_by_speed, "By speed")
            .on_hover_text("A faster stroke gets a wider correction window.");
        ui.checkbox(&mut c.post_by_scale, "By scale").on_hover_text(
            "Hold the correction constant in SCREEN pixels instead of \
document pixels, so a line drawn zoomed out is corrected as hard as one \
drawn zoomed in. Leave this on if lines only look right to you at high zoom.",
        );
    });
    ui.checkbox(&mut c.sharp, "Sharp angles").on_hover_text(
        "Never smooth across a corner you drew on purpose — the correction \
stops at the angle and picks up on the far side of it.",
    );

    // Row 42 (A-014): CSP はみ出さない. Built from the reference set +
    // frame folders at each stroke's start.
    ui.checkbox(&mut app.anti_overflow, "Don't cross reference lines")
        .on_hover_text(
            "Scribble freely — the paint stops at the reference layers' ink (and \
frame borders): a blocked pixel is never painted, so flats stay inside \
the lineart. The stroke runs on the CPU while this is on.",
        );
    if app.anti_overflow {
        // A-016 (色余白): near-miss colours count as the line.
        ui.horizontal(|ui| {
            ui.weak("Colour margin");
            let mut m = app.anti_overflow_margin;
            let r = ui.add(
                egui::DragValue::new(&mut m)
                    .range(0..=128)
                    .suffix(" /255"),
            );
            let changed = r.changed();
            r.on_hover_text(
                "how far a colour may sit from the reference ink's own colour \
                 and still wall the stroke — the anti-aliased fringe and \
                 off-hue strokes fold into the line",
            );
            if changed {
                app.anti_overflow_margin = m;
            }
            // A-015 (ベクトルまで塗り): vector references wall at the
            // spline, not the rendered edge.
            let mut v = app.anti_overflow_vector_centreline;
            if ui
                .checkbox(&mut v, "Vector lines: centreline")
                .on_hover_text(
                    "a vector reference layer's strokes wall at their centre \
                     line — paint tucks to the middle of the line instead of \
                     stopping at its anti-aliased fringe",
                )
                .changed()
            {
                app.anti_overflow_vector_centreline = v;
            }
        });
    }

    ui.add_space(2.0);
    let mut by_speed = c.stab_by_speed;
    if ui
        .checkbox(&mut by_speed, "Stabilize by speed")
        .on_hover_text("Vary the Stabilize slider above with how fast the pen is moving.")
        .changed()
    {
        c.stab_by_speed = by_speed;
    }
    if c.stab_by_speed {
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut c.stab_mode,
                StabMode::IncreaseWhenSlow,
                "Longer when slow",
            );
            ui.selectable_value(
                &mut c.stab_mode,
                StabMode::ReduceWhenFast,
                "Shorter when fast",
            );
        });
    }

    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.weak("Start/end");
        ui.selectable_value(&mut c.se_how, SeHow::Length, "Length");
        ui.selectable_value(&mut c.se_how, SeHow::Fade, "Fade")
            .on_hover_text("Thin from full to the minimum over the Out length, then hold there.");
    });
    if c.se_how == SeHow::Length {
        let mut v = c.start_px;
        if ValueBar::new("In", 0.0, MAX_SE_PX)
            .step(1.0)
            .suffix(" px")
            .show(ui, &mut v)
            .changed()
        {
            c.start_px = v;
        }
    }
    let mut v = c.end_px;
    let out = ValueBar::new(
        if c.se_how == SeHow::Fade {
            "Fade over"
        } else {
            "Out"
        },
        0.0,
        MAX_SE_PX,
    )
    .step(1.0)
    .suffix(" px")
    .show(ui, &mut v);
    if out.changed() {
        c.end_px = v;
    }
    if c.se_how == SeHow::Length {
        out.on_hover_text(
            "The exit taper costs latency by construction: the last Out pixels \
of a stroke cannot be shaped until the pen lifts, so that much ink lands on \
release. Fade has no such cost.",
        );
    }
    let mut m = c.se_min * 100.0;
    if ValueBar::new("Start/end min", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut m)
        .changed()
    {
        c.se_min = m / 100.0;
    }
    ui.checkbox(&mut c.se_by_speed, "Start/end by speed")
        .on_hover_text("A slow stroke gets a shorter, weaker taper.");

    if c != cur {
        app.push_cmd(AppCmd::SetCorrection(c));
    }
}

/// The common brush parameters, shared by Tool Property and Sub Tool Detail.
pub(crate) fn brush_sliders(ui: &mut egui::Ui, app: &mut App) {
    let p = app.props_current;

    // Absolute px, not a multiplier of the preset: the bar's range covers the
    // whole `[`/`]` ladder, so the two controls can no longer disagree about
    // how big the brush is allowed to get. The readout is the engine's own
    // dab diameter rather than the pending slider value.
    if app.pen_row_visible("size") {
    let mut size = p.size_px;
    if ValueBar::new("Size", 1.0, 2000.0)
        .log()
        .display_text(format!("{:.1} px", app.brush_radius() * 2.0))
        .show(ui, &mut size)
        .changed()
    {
        app.push_cmd(AppCmd::SetBrushSizePx(size));
    }
    }


    if app.pen_row_visible("min_size") {
    let mut min = p.min_size;
    if ValueBar::new("Min size", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut min)
        .changed()
    {
        app.push_cmd(AppCmd::SetMinSize(min));
    }
    }


    if app.pen_row_visible("opacity") {
    let mut op = p.opacity * 100.0;
    if ValueBar::new("Opacity", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut op)
        .changed()
    {
        app.push_cmd(AppCmd::SetOpacity(op / 100.0));
    }
    }


    if app.pen_row_visible("stabilize") {
    let mut stab = p.stabilizer * 100.0;
    if ValueBar::new("Stabilize", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut stab)
        .changed()
    {
        app.push_cmd(AppCmd::SetStabilizer(stab / 100.0));
    }
    }


    if app.pen_row_visible("correction") {
        correction_rows(ui, app);
    }

    // Brush-size randomization (CSP 乱数) with the two things stock
    // libmypaint lacks: a pressure floor for the deviation (like Min size /
    // Min opacity) and a fixed-pixel mode whose deviation does not scale
    // with brush size.
    if app.pen_row_visible("randomize") {
    let mut rand = if p.random_abs {
        p.random
    } else {
        p.random * 100.0
    };
    let rand_changed = if p.random_abs {
        ValueBar::new("Randomize", 0.0, 16.0)
            .decimals(1)
            .suffix(" px")
            .show(ui, &mut rand)
            .changed()
    } else {
        ValueBar::new("Randomize", 0.0, 100.0)
            .suffix("%")
            .show(ui, &mut rand)
            .changed()
    };
    if rand_changed {
        let v = if p.random_abs { rand } else { rand / 100.0 };
        app.push_cmd(AppCmd::SetRandomization(v));
    }
    }


    if app.pen_row_visible("min_rand") {
    let mut rmin = p.random_min;
    if ValueBar::new("Min rand", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut rmin)
        .changed()
    {
        app.push_cmd(AppCmd::SetRandomMin(rmin));
    }
    }


    if app.pen_row_visible("random_abs") {
    let mut abs = p.random_abs;
    if ui
        .checkbox(&mut abs, "Fixed px (size-independent)")
        .on_hover_text(
            "Randomization normally scales with brush size — good at small sizes, \
             coarse at large ones. Fixed px keeps the deviation constant; Min rand \
             gives it a pressure curve like brush size and opacity.",
        )
        .changed()
    {
        app.push_cmd(AppCmd::SetRandomAbs(abs));
    }
    }


    // Krita-inspired dab modes (round 25): the tip selector swaps the dab
    // PROFILE — gaussian falloff (MyPaint classic) vs an exact AA disc
    // (Krita/CSP-style crisp ink edges). Presets can carry either.
    if app.pen_row_visible("tip") {
    ui.horizontal(|ui| {
        ui.weak("Tip");
        let mut hard = app.props_current.hard_dab;
        ui.selectable_value(&mut hard, false, "Gaussian");
        ui.selectable_value(&mut hard, true, "Hard");
        if hard != app.props_current.hard_dab {
            app.push_cmd(AppCmd::SetHardDab(hard));
        }
    })
    .response
    .on_hover_text(
        "Gaussian: the classic soft MyPaint falloff. Hard: an exact \
anti-aliased disc — CSP/Krita pen-crisp edges, the preset's hardness is ignored.",
    );

    }

    if app.pen_row_visible("anti_alias") {
        anti_alias_row(ui, app);
    }
    if app.pen_row_visible("interval") {
        interval_rows(ui, app);
    }

    if app.pen_row_visible("scatter") {
    let mut sc = app.props_current.scatter;
    let sc_resp = ValueBar::new("Scatter", 0.0, 2.0)
        .decimals(2)
        .suffix("×r")
        .show(ui, &mut sc);
    if sc_resp.changed() {
        app.push_cmd(AppCmd::SetScatter(sc));
    }
    sc_resp.on_hover_text(
        "Krita Scatter: each dab lands within radius×this of the \
stroke path — sketchy, sprayed lines at higher values.",
    );
    }


    // Krita SKETCH engine (round 27), gated as one row.
    if app.pen_row_visible("sketch") {
    // Krita SKETCH engine (round 27): the stroke links back to its own
    // recent history — scribbles knot into hatching webs.
    let mut sk = app.props_current.sketch;
    if ui.checkbox(&mut sk, "Sketch").changed() {
        app.push_cmd(AppCmd::SetSketch(sk));
    }
    ui.response().on_hover_text(
        "Krita's sketch engine: while drawing, the stroke \
connects to points it recently passed nearby — loops and scribbles knot into a \
hatching web instead of a clean line. Distance = how far back it connects.",
    );
    if app.props_current.sketch {
        let mut dist = app.props_current.sketch_dist;
        if ValueBar::new("Link dist", 5.0, 150.0)
            .suffix(" px")
            .show(ui, &mut dist)
            .changed()
        {
            app.push_cmd(AppCmd::SetSketchDistance(dist));
        }
        let mut dens = app.props_current.sketch_density * 100.0;
        if ValueBar::new("Link rate", 0.0, 100.0)
            .suffix("%")
            .show(ui, &mut dens)
            .changed()
        {
            app.push_cmd(AppCmd::SetSketchDensity(dens / 100.0));
        }
    }
    }

    // Krita Wash (flow vs opacity): Build-up = stock per-dab compositing;
    // Wash = the whole stroke composites once at Opacity, Flow is the per-dab
    // alpha, and the optional blend mode applies at the commit.
    if app.pen_row_visible("ink") {
    ui.horizontal(|ui| {
        ui.weak("Ink");
        let mut wash = app.props_current.wash;
        ui.selectable_value(&mut wash, false, "Build-up");
        ui.selectable_value(&mut wash, true, "Wash");
        if wash != app.props_current.wash {
            app.push_cmd(AppCmd::SetWash(wash));
        }
        if app.props_current.wash {
            let mut blend = app.props_current.brush_blend;
            // The wash commit runs `mn_core::blend::blend_premul` for
            // whatever mode it is handed (`mn_brush::commit_wash`, and the
            // GPU stroke path reaches the SAME function after readback), so
            // the drawing-side list has no reason to be shorter than the
            // layer-side one — the old three-item list was a UI stub, not a
            // capability limit. One source of truth: the layer picker's own
            // array and its own names.
            egui::ComboBox::from_id_salt("brush-blend")
                .selected_text(crate::ui::layers::blend_name(blend))
                .width(84.0)
                .show_ui(ui, |ui| {
                    for b in crate::ui::layers::BLENDS {
                        ui.selectable_value(&mut blend, b, crate::ui::layers::blend_name(b));
                    }
                });
            if blend != app.props_current.brush_blend {
                app.push_cmd(AppCmd::SetBrushBlend(blend));
            }
            // CSP's Ink output (Advanced Tool Settings ▸ Ink): the
            // brush-only commit behaviours layered over the blend.
            let mut draw = app.props_current.brush_draw;
            egui::ComboBox::from_id_salt("brush-draw")
                .selected_text(draw_label(draw))
                .width(110.0)
                .show_ui(ui, |ui| {
                    for d in [
                        mn_brush::BrushDraw::Normal,
                        mn_brush::BrushDraw::BlackBurn,
                        mn_brush::BrushDraw::WhiteBurn,
                        mn_brush::BrushDraw::CompareDensity,
                        mn_brush::BrushDraw::Background,
                        mn_brush::BrushDraw::ReplaceAlpha,
                    ] {
                        ui.selectable_value(&mut draw, d, draw_label(d));
                    }
                })
                .response
                .on_hover_text(
                    "black/white burn darken or lighten only existing ink · \
                     compare density paints only where the stroke is denser · \
                     background paints underneath · replace alpha overlays and \
                     takes over the coverage",
                );
            if draw != app.props_current.brush_draw {
                app.push_cmd(AppCmd::SetBrushDraw(draw));
            }
        }
    })
    .response
    .on_hover_text(
        "Build-up: every dab composites into the layer (stock). Wash: the \
stroke accumulates in a buffer and composites ONCE — a single stroke can never \
exceed its Opacity however much it overlaps, like a wet marker. Flow = per-dab \
alpha; the blend mode is per-brush (Krita).",
    );

    }

    if app.pen_row_visible("flow") {
    if app.props_current.wash {
        let mut flow = app.props_current.flow * 100.0;
        let f_resp = ValueBar::new("Flow", 0.0, 100.0)
            .suffix("%")
            .show(ui, &mut flow);
        if f_resp.changed() {
            app.push_cmd(AppCmd::SetFlow(flow / 100.0));
        }
        f_resp.on_hover_text(
            "Per-dab alpha inside the wash stroke (Krita: Flow). Low \
flow = faint dabs that stack up to the stroke's Opacity, never past it.",
        );
    }
    }


    // CSP's Ink colour-mixing trio (I-010/011/013) and Color jitter
    // (C-010..012) — each gated as one row, like the sketch and texture
    // groups above.
    if app.pen_row_visible("mixing") {
        mixing_rows(ui, app);
    }
    if app.pen_row_visible("water_edge") {
        water_edge_rows(ui, app);
    }
    if app.pen_row_visible("jitter") {
        jitter_rows(ui, app);
    }

    // Krita texture tips — gated as one row (texture + crawl + angle).
    if app.pen_row_visible("texture") {
    // Krita texture tips: a grayscale mask multiplies the dab, anchored to
    // the canvas — paper grain / tone that does not wash out.
    ui.horizontal(|ui| {
        ui.weak("Texture");
        let current = app.props_current.texture;
        let label = if current == 0 {
            "None".to_owned()
        } else {
            app.texture_names
                .get(current as usize - 1)
                .cloned()
                .unwrap_or_else(|| "?".into())
        };
        // Names cloned out so the popup closure never borrows `app` against
        // the command push after it.
        let names = app.texture_names.clone();
        let mut pick = None;
        egui::ComboBox::from_id_salt("brush-texture")
            .selected_text(label)
            .width(104.0)
            .show_ui(ui, |ui| {
                let mut sel = current;
                ui.selectable_value(&mut sel, 0, "None");
                for (i, name) in names.iter().enumerate() {
                    ui.selectable_value(&mut sel, i as u16 + 1, name);
                }
                if sel != current {
                    pick = Some(sel);
                }
            });
        if let Some(sel) = pick {
            app.push_cmd(AppCmd::SetTexture(sel));
        }
    })
    .response
    .on_hover_text(
        "A grayscale mask multiplies every dab (Krita texture tips). Ours is \
anchored to the CANVAS, so the grain stays put and overlapping strokes keep the \
texture — reads as paper tooth, not noise.",
    );

    if app.props_current.texture > 0 {
        let mut crawl = app.props_current.texture_scroll;
        let t_resp = ValueBar::new("Crawl", 0.0, 16.0)
            .decimals(1)
            .suffix(" px")
            .show(ui, &mut crawl);
        if t_resp.changed() {
            app.push_cmd(AppCmd::SetTextureScroll(crawl));
        }
        t_resp.on_hover_text(
            "Texture crawl per dab (Krita: offset per dab). 0 = the \
pattern is fixed; higher = the grain drifts as you draw, spray-like.",
        );
        // B-031/032: what a stamped tip's rotation follows. The label says
        // "ink" nowhere near it — this is the flat-nib question: a chisel
        // tip that never turns reads wrong in every direction but one.
        ui.horizontal(|ui| {
            ui.weak("Tip angle");
            let mut r = app.props_current.texture_rotate;
            for (v, l) in [
                (mn_brush::TextureRotate::Fixed, "Fixed"),
                (mn_brush::TextureRotate::Direction, "Stroke"),
                (mn_brush::TextureRotate::Tilt, "Pen tilt"),
            ] {
                if ui
                    .selectable_label(r == v, l)
                    .clicked()
                    && r != v
                {
                    r = v;
                }
            }
            if r != app.props_current.texture_rotate {
                app.push_cmd(AppCmd::SetTextureRotate(r));
            }
        })
        .response
        .on_hover_text(
            "what the stamped tip rotates with: a fixed base angle, the \
             stroke's direction, or the pen's physical tilt bearing",
        );
    }
    }

    // B-026/027: the tip's own mirroring. Its own eye row rather than part
    // of the texture group, because it is the one setting an artist reaches
    // for AFTER the tip is chosen and never touches again.
    if app.pen_row_visible("tip_flip") {
        tip_flip_rows(ui, app);
    }
}

/// CSP Ink ▸ **Mixing mode** (`I-014`, triage rows 58 + 167): the pigment
/// model the other rows in this block operate under.
///
/// FIRST in the group, above Paint density, because it is the only row here
/// that changes what mixing MEANS — the sliders are amounts, this is the
/// model. It is also the only Tool Property in the panel that reroutes the
/// stroke off the GPU, and the tooltip says so: an artist who notices their
/// big brush got heavier deserves to know which switch did it rather than
/// filing it under "the app got slow today".
fn mix_mode_row(ui: &mut egui::Ui, app: &mut App) {
    use mn_core::BrushMix;

    let current = app.props_current.brush_mix;
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.weak("Mixing");
        for m in BrushMix::ALL {
            if ui.selectable_label(current == m, m.label()).clicked() && current != m {
                pick = Some(m);
            }
        }
    })
    .response
    .on_hover_text(
        "CSP's Mixing mode. Standard mixes colours the way a screen adds \
light, which is why blending two strong colours drifts toward grey mud. \
Paint mixes them the way pigment does — blue over yellow makes green, and a \
blend keeps its colour instead of dulling. It also changes the colour a \
smudge picks up off the canvas, and it steers Color jitter.\n\nPaint is \
drawn on the CPU (the GPU brush path has no pigment model), so a very large \
brush can feel heavier in this mode.",
    );
    if let Some(m) = pick {
        app.push_cmd(AppCmd::SetBrushMix(m));
    }
}

/// CSP Ink ▸ Density of paint / Color stretch / Intensity of blur
/// (I-010/011/013) — the three knobs of colour mixing, which is one
/// behaviour and reads as one block.
///
/// Stretch and blur only appear below full density, for the reason the row
/// itself states: with neat paint nothing is picked up, so there is nothing
/// to stretch or blur and two live-looking sliders would be lying.
pub(crate) fn mixing_rows(ui: &mut egui::Ui, app: &mut App) {
    mix_mode_row(ui, app);
    let mut density = app.props_current.paint_density * 100.0;
    let d_resp = ValueBar::new("Paint", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut density);
    if d_resp.changed() {
        app.push_cmd(AppCmd::SetPaintDensity(density / 100.0));
    }
    d_resp.on_hover_text(
        "CSP's Density of paint: how much of YOUR colour a dab lays down, \
against how much it picks up off the canvas underneath. 100 % is neat ink \
and is what every pen ships with. Lower it and the brush starts mixing with \
what is already there — watercolour, blending, a smudge tool.",
    );
    if app.props_current.paint_density >= 1.0 {
        return;
    }

    let mut stretch = app.props_current.color_stretch * 100.0;
    let s_resp = ValueBar::new("Stretch", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut stretch);
    if s_resp.changed() {
        app.push_cmd(AppCmd::SetColorStretch(stretch / 100.0));
    }
    s_resp.on_hover_text(
        "CSP's Color stretch: how far the colour picked up at the start of \
a stroke is dragged along it. 0 % re-reads the canvas at every dab (a short \
smear); 100 % carries the first colour the whole way.",
    );

    let abs = app.props_current.blur_abs;
    let mut blur = app.props_current.blur;
    let b_resp = if abs {
        ValueBar::new("Blur", 1.0, 200.0)
            .log()
            .decimals(1)
            .suffix(" px")
            .show(ui, &mut blur)
    } else {
        ValueBar::new("Blur", 0.05, 20.0)
            .log()
            .decimals(2)
            .suffix("×r")
            .show(ui, &mut blur)
    };
    if b_resp.changed() {
        app.push_cmd(AppCmd::SetBlur(blur));
    }
    b_resp.on_hover_text(
        "CSP's Intensity of blur: how wide an area the running colour is \
picked up from. Narrow reads as a smear, wide reads as a blur. The unit is a \
multiple of the brush radius, so it follows the Size slider — unless you pin \
it below.",
    );
    let mut pinned = abs;
    if ui
        .checkbox(&mut pinned, "Blur pinned to px")
        .on_hover_text(
            "Pin the blur width to a canvas-pixel number that does NOT \
follow the Size slider (CSP's fixed mode). The number keeps its face value \
and changes meaning when you switch, exactly like Randomize's Fixed px.",
        )
        .changed()
    {
        app.push_cmd(AppCmd::SetBlurAbs(pinned));
    }
}

/// CSP Advanced ▸ Watercolor edge (W-001..005) — the bleed rim.
///
/// One width bar plus three that only appear once it is on, the mixing
/// group's rule and for the same reason: with no rim there is nothing for
/// opacity, darkness or blur to act on, and three live-looking sliders that
/// cannot change a pixel are a lie the panel tells.
pub(crate) fn water_edge_rows(ui: &mut egui::Ui, app: &mut App) {
    let e = app.props_current.water_edge;
    let mut next = e;
    let mut width = e.px;
    let w_resp = ValueBar::new("Wet edge", 0.0, mn_core::edge::WIDTH_MAX)
        .decimals(1)
        .suffix(" px")
        .show(ui, &mut width);
    if w_resp.changed() {
        next.px = width;
    }
    w_resp.on_hover_text(
        "CSP's Watercolor edge: a darker bleed rim laid just OUTSIDE the \
stroke when you lift the pen, the way pigment pools at the edge of a wet \
wash. 0 px is off. It is baked into the pixels, like CSP's — the \
non-destructive version of the same look is the layer's Border effect.",
    );
    if e.on() {
        let mut op = e.opacity * 100.0;
        let o_resp = ValueBar::new("Wet op", 0.0, 100.0)
            .suffix("%")
            .show(ui, &mut op);
        if o_resp.changed() {
            next.opacity = op / 100.0;
        }
        o_resp.on_hover_text("How strong the rim is. Higher reads darker.");

        let mut dark = e.darkness * 100.0;
        let d_resp = ValueBar::new("Wet dark", 0.0, 100.0)
            .suffix("%")
            .show(ui, &mut dark);
        if d_resp.changed() {
            next.darkness = dark / 100.0;
        }
        d_resp.on_hover_text(
            "CSP's Darkness: drains the colour out of the rim toward grey \
and takes its brightness down with it. 0 % keeps the rim the colour you \
drew with.",
        );

        let mut blur = e.blur_px;
        let b_resp = ValueBar::new("Wet blur", 0.0, mn_core::edge::WIDTH_MAX)
            .decimals(1)
            .suffix(" px")
            .show(ui, &mut blur);
        if b_resp.changed() {
            next.blur_px = blur;
        }
        b_resp.on_hover_text(
            "Softens the rim's outer boundary over this distance instead of \
one pixel. It fades the edge inward; it does not make the rim wider.",
        );
    }
    if next != e {
        app.push_cmd(AppCmd::SetWaterEdge(next));
    }
}

/// CSP Color jitter (C-010..012): hue / saturation / brightness wander, so a
/// stroke is not one flat value — the difference between a foliage brush and
/// a stamped one.
pub(crate) fn jitter_rows(ui: &mut egui::Ui, app: &mut App) {
    let j = app.props_current.jitter;
    let mut next = j;
    let mut hue = j.hue * 100.0;
    let h_resp = ValueBar::new("Jit hue", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut hue);
    if h_resp.changed() {
        next.hue = hue / 100.0;
    }
    h_resp.on_hover_text(
        "How far the colour is allowed to wander around the wheel: 100 % is \
a half turn either way. A few percent is what keeps foliage, hair and \
texture from reading as one flat fill.",
    );
    let mut sat = j.sat * 100.0;
    if ValueBar::new("Jit sat", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut sat)
        .changed()
    {
        next.sat = sat / 100.0;
    }
    let mut bri = j.bri * 100.0;
    if ValueBar::new("Jit bright", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut bri)
        .changed()
    {
        next.bri = bri / 100.0;
    }
    if !j.is_off() {
        ui.horizontal(|ui| {
            ui.weak("Jitter");
            let mut per_dab = j.per_dab;
            ui.selectable_value(&mut per_dab, true, "Along stroke");
            ui.selectable_value(&mut per_dab, false, "Per stroke");
            next.per_dab = per_dab;
        })
        .response
        .on_hover_text(
            "Along stroke: the colour keeps moving as you draw — grain, the \
natural-texture look. Per stroke: one colour per stroke, so each stroke is \
slightly different from the last but internally even.\n\nAlong stroke re-draws \
per input sample rather than per dab: the dab colour is decided inside \
libmypaint's own loop, which we do not patch.",
        );
    }
    if next != j {
        app.push_cmd(AppCmd::SetColorJitter(next));
    }
}

/// CSP 反転 (B-026/027): flip the brush tip left-right / up-down.
///
/// `On reverse` is the mode with the story: an asymmetric tip drawn
/// right-to-left comes out backwards, and this mirrors it per dab so a
/// stroke reads the same in both directions.
pub(crate) fn tip_flip_rows(ui: &mut egui::Ui, app: &mut App) {
    use mn_brush::TipFlip;

    let (mut h, mut v) = (app.props_current.tip_flip_h, app.props_current.tip_flip_v);
    let (was_h, was_v) = (h, v);
    let row = |ui: &mut egui::Ui, label: &str, cur: &mut TipFlip| {
        ui.horizontal(|ui| {
            ui.weak(label);
            for mode in TipFlip::ALL {
                if ui.selectable_label(*cur == mode, mode.label()).clicked() {
                    *cur = mode;
                }
            }
        });
    };
    row(ui, "Flip ↔", &mut h);
    row(ui, "Flip ↕", &mut v);
    ui.response().on_hover_text(
        "Mirror the brush TIP image. Always is a permanent mirror; Random \
re-rolls per dab; On reverse mirrors only while the stroke runs backwards \
along that axis — which is what stops a chisel or dry-brush tip from looking \
inside-out when you draw right to left.",
    );
    if app.props_current.texture == 0 && (h != TipFlip::Off || v != TipFlip::Off) {
        ui.weak("no tip texture — nothing to mirror");
    }
    if (h, v) != (was_h, was_v) {
        app.push_cmd(AppCmd::SetTipFlip(h, v));
    }
}

/// CSP Tool Settings ▸ Anti-aliasing (A-010): four levels, plus the "as the
/// preset ships it" state that keeps an untouched brush drawing unchanged.
pub(crate) fn anti_alias_row(ui: &mut egui::Ui, app: &mut App) {
    use mn_brush::AntiAlias;

    let current = app.props_current.anti_alias;
    // Hard dab stamps an exact disc and reads no hardness at all (see the
    // Hard dab row's own note), so the feather this picks never reaches the
    // pixels there — only the small radius growth that comes with it does.
    // Saying so beats a px readout that looks like it is doing something.
    let hard = app.props_current.hard_dab;
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.weak("Anti-alias");
        egui::ComboBox::from_id_salt("brush-antialias")
            .selected_text(aa_label(current))
            .width(96.0)
            .show_ui(ui, |ui| {
                let mut sel = current;
                ui.selectable_value(&mut sel, AntiAlias::AsPreset, aa_label(AntiAlias::AsPreset));
                for level in AntiAlias::LEVELS {
                    ui.selectable_value(&mut sel, level, aa_label(level));
                }
                if sel != current {
                    pick = Some(sel);
                }
            });
        if hard {
            ui.weak("hard dab ignores it");
        } else {
            ui.weak(format!("{:.2} px", app.engine().anti_alias_px()));
        }
    })
    .response
    .on_hover_text(
        "How soft the dab's edge is allowed to be (CSP's four levels). It is \
a MINIMUM feather in pixels: a tip already softer than this keeps its own \
edge, a harder one is softened to reach it — and the dab grows a little so \
the visible width does not change. As preset leaves the brush exactly as it \
was authored. Hard dab draws its own exact disc edge, so this row does \
nothing while that is on.",
    );
    if let Some(aa) = pick {
        app.push_cmd(AppCmd::SetAntiAlias(aa));
    }
}

pub(crate) fn aa_label(aa: mn_brush::AntiAlias) -> &'static str {
    use mn_brush::AntiAlias;
    match aa {
        AntiAlias::AsPreset => "As preset",
        AntiAlias::Off => "None",
        AntiAlias::Weak => "Weak",
        AntiAlias::Middle => "Middle",
        AntiAlias::Strong => "Strong",
    }
}

/// CSP Advanced ▸ Stroke ▸ Interval (S-028) and its companion toggle,
/// Brush tip ▸ Adjust brush density by gap (B-029). They ship together
/// because the gap is what the compensation compensates FOR.
pub(crate) fn interval_rows(ui: &mut egui::Ui, app: &mut App) {
    use mn_brush::Interval;

    let current = app.props_current.interval;
    let remembered_px = app.props_current.interval_px;
    let gap_px = app.engine().dab_gap_px();
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.weak("Interval");
        egui::ComboBox::from_id_salt("brush-interval")
            .selected_text(interval_label(current))
            .width(96.0)
            .show_ui(ui, |ui| {
                for choice in [
                    Interval::AsPreset,
                    Interval::Percent(Interval::NARROW_PCT),
                    Interval::Percent(Interval::NORMAL_PCT),
                    Interval::Percent(Interval::WIDE_PCT),
                    Interval::FixedPx(remembered_px),
                ] {
                    let on = same_interval_mode(current, choice);
                    if ui.selectable_label(on, interval_label(choice)).clicked() && !on {
                        pick = Some(choice);
                    }
                }
            });
        ui.weak(if gap_px.is_finite() {
            format!("{gap_px:.2} px")
        } else {
            "on the clock".to_owned()
        });
    })
    .response
    .on_hover_text(
        "How far apart the dabs a stroke is stamped from sit. Too wide and \
the line reads as a row of beads; too narrow and it is slower and builds up \
darker — which is what the density toggle below is for. Narrow/Normal/Wide \
are fractions of the tip, so they hold when you resize the brush; Fixed is a \
pixel distance that does not. The number on the right is the real gap at the \
current size.",
    );

    if let Interval::FixedPx(_) = current {
        let mut px = remembered_px;
        let resp = ValueBar::new("Gap", Interval::MIN_PX, Interval::MAX_PX)
            .log()
            .decimals(2)
            .suffix(" px")
            .show(ui, &mut px);
        if resp.changed() {
            pick = Some(Interval::FixedPx(px));
        }
        resp.on_hover_text(
            "The literal gap between dabs, in canvas pixels. Unlike the \
three relative settings this does NOT follow the Size slider.",
        );
    }

    // The toggle reads the engine while the user has not touched it, so a
    // preset that ships the compensation off shows off.
    let mut on = app
        .props_current
        .density_by_gap
        .unwrap_or_else(|| app.engine().density_by_gap());
    if ui
        .checkbox(&mut on, "Adjust density by gap")
        .on_hover_text(
            "Keeps the interval from deciding how dark the stroke comes \
out: each dab is made fainter in proportion to how many of them land on a \
pixel, so tightening the gap smooths the line instead of blackening it. Off \
is raw build-up — every dab paints at full strength and overlaps add up. \
This is also the cure for periodic banding: with it on you can narrow the \
interval until the rings disappear and pay nothing in density.",
        )
        .changed()
    {
        app.push_cmd(AppCmd::SetDensityByGap(on));
    }

    if let Some(iv) = pick {
        app.push_cmd(AppCmd::SetInterval(iv));
    }
}

/// Whether two interval values are the same CSP *mode* (the dropdown marks a
/// row selected by mode, not by the number riding inside it).
pub(crate) fn same_interval_mode(a: mn_brush::Interval, b: mn_brush::Interval) -> bool {
    use mn_brush::Interval;
    match (a, b) {
        (Interval::AsPreset, Interval::AsPreset) => true,
        (Interval::FixedPx(_), Interval::FixedPx(_)) => true,
        (Interval::Percent(x), Interval::Percent(y)) => (x - y).abs() < 0.01,
        _ => false,
    }
}

pub(crate) fn interval_label(iv: mn_brush::Interval) -> String {
    use mn_brush::Interval;
    match iv {
        Interval::AsPreset => "As preset".to_owned(),
        Interval::Percent(p) if (p - Interval::NARROW_PCT).abs() < 0.01 => "Narrow".to_owned(),
        Interval::Percent(p) if (p - Interval::NORMAL_PCT).abs() < 0.01 => "Normal".to_owned(),
        Interval::Percent(p) if (p - Interval::WIDE_PCT).abs() < 0.01 => "Wide".to_owned(),
        Interval::Percent(p) => format!("{p:.0} % of tip"),
        Interval::FixedPx(_) => "Fixed".to_owned(),
    }
}

/// CSP Ink output names (BM-029..035). "Ink" instead of "blend" in the
/// labels because that is the question the row answers — what the ink
/// DOES when it lands, not how two layers mix.
pub(crate) fn draw_label(d: mn_brush::BrushDraw) -> &'static str {
    match d {
        mn_brush::BrushDraw::Normal => "Over",
        mn_brush::BrushDraw::BlackBurn => "Black burn",
        mn_brush::BrushDraw::WhiteBurn => "White burn",
        mn_brush::BrushDraw::CompareDensity => "Compare density",
        mn_brush::BrushDraw::Background => "Background (under)",
        mn_brush::BrushDraw::ReplaceAlpha => "Replace alpha",
    }
}
