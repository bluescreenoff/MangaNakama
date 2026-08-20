use super::*;

pub(crate) fn pen_property(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(app.brush_name().to_owned())
                .size(11.5)
                .color(theme::TEXT_STRONG),
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
    brush_sliders(ui, app);
    group_caption(ui, "Dynamics");
    dynamics_editor(ui, app);
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
    painter.rect_filled(rect, 2.0, theme::FIELD);
    let mid_y = to_px((0.0, 0.0));
    if rect.top() < mid_y.y && mid_y.y < rect.bottom() {
        painter.line_segment(
            [pos2(rect.left(), mid_y.y), pos2(rect.right(), mid_y.y)],
            egui::Stroke::new(1.0, theme::OUTLINE),
        );
    }
    if x0 < 0.0 {
        let mid_x = to_px((0.0, 0.0));
        painter.line_segment(
            [pos2(mid_x.x, rect.top()), pos2(mid_x.x, rect.bottom())],
            egui::Stroke::new(1.0, theme::OUTLINE),
        );
    }
    if app.curve_edit_points.len() >= 2 {
        let stroke = egui::Stroke::new(2.0, theme::ACCENT);
        for pair in app.curve_edit_points.windows(2) {
            painter.line_segment([to_px(pair[0]), to_px(pair[1])], stroke);
        }
    }
    for (i, p) in app.curve_edit_points.iter().enumerate() {
        let pos = to_px(*p);
        let hot = app.curve_drag == Some(i);
        painter.circle_filled(pos, if hot { 5.0 } else { 3.5 }, theme::ACCENT);
        if hot {
            painter.circle_stroke(pos, 5.0, egui::Stroke::new(1.5, theme::TEXT_STRONG));
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

    let mut size = p.size;
    if ValueBar::new("Size", 0.25, 4.0)
        .log()
        .display_text(format!("{:.1} px", app.brush_radius() * 2.0))
        .show(ui, &mut size)
        .changed()
    {
        app.push_cmd(AppCmd::SetBrushSize(size));
    }

    let mut min = p.min_size;
    if ValueBar::new("Min size", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut min)
        .changed()
    {
        app.push_cmd(AppCmd::SetMinSize(min));
    }

    let mut op = p.opacity * 100.0;
    if ValueBar::new("Opacity", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut op)
        .changed()
    {
        app.push_cmd(AppCmd::SetOpacity(op / 100.0));
    }

    let mut stab = p.stabilizer * 100.0;
    if ValueBar::new("Stabilize", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut stab)
        .changed()
    {
        app.push_cmd(AppCmd::SetStabilizer(stab / 100.0));
    }

    correction_rows(ui, app);

    // Brush-size randomization (CSP 乱数) with the two things stock
    // libmypaint lacks: a pressure floor for the deviation (like Min size /
    // Min opacity) and a fixed-pixel mode whose deviation does not scale
    // with brush size.
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

    let mut rmin = p.random_min;
    if ValueBar::new("Min rand", 0.0, 100.0)
        .suffix("%")
        .show(ui, &mut rmin)
        .changed()
    {
        app.push_cmd(AppCmd::SetRandomMin(rmin));
    }

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

    // Krita-inspired dab modes (round 25): the tip selector swaps the dab
    // PROFILE — gaussian falloff (MyPaint classic) vs an exact AA disc
    // (Krita/CSP-style crisp ink edges). Presets can carry either.
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

    anti_alias_row(ui, app);
    interval_rows(ui, app);

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

    // Krita Wash (flow vs opacity): Build-up = stock per-dab compositing;
    // Wash = the whole stroke composites once at Opacity, Flow is the per-dab
    // alpha, and the optional blend mode applies at the commit.
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
        }
    })
    .response
    .on_hover_text(
        "Build-up: every dab composites into the layer (stock). Wash: the \
stroke accumulates in a buffer and composites ONCE — a single stroke can never \
exceed its Opacity however much it overlaps, like a wet marker. Flow = per-dab \
alpha; the blend mode is per-brush (Krita).",
    );

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
