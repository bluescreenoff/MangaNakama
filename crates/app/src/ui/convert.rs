//! Layer ▸ Convert layer… (row 33) and Layer ▸ Extract lines… (row 31)
//! — small windows over `Document::convert_layer` / `extract_lines`.

use crate::app::App;
use crate::cmd::AppCmd;
use mn_core::doc::LayerExpression;

pub(super) fn convert_window(ctx: &egui::Context, app: &mut App) {
    if !app.convert_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Convert Layer")
        .open(&mut open)
        .resizable(false)
        .default_pos(egui::pos2(320.0, 120.0))
        .show(ctx, |ui| {
            let li = app.doc.active;
            let rasterizable = app
                .doc
                .layers
                .get(li)
                .is_some_and(|l| !l.folder && !matches!(l.kind, mn_core::doc::LayerKind::Raster));
            ui.label(format!("“{}”", app.doc.active_layer().name));
            ui.separator();
            let mut rasterize = rasterizable;
            ui.add_enabled(rasterizable, egui::Checkbox::new(&mut rasterize, "Rasterize"))
                .on_hover_text(
                    "bake the rendered pixels and drop the vector state (text, balloon, vector layers)",
                );
            if !rasterizable {
                ui.small("the layer is already raster");
            }
            ui.horizontal(|ui| {
                ui.label("Expression colour");
                let mut e = app.convert_expr.unwrap_or(app.doc.active_layer().expression);
                egui::ComboBox::from_id_salt("mn.convert.expr")
                    .selected_text(expr_label(e))
                    .show_ui(ui, |ui| {
                        for (v, l) in [
                            (LayerExpression::Colour, "Colour"),
                            (LayerExpression::Grey, "Grey"),
                            (LayerExpression::Mono, "Mono"),
                        ] {
                            ui.selectable_value(&mut e, v, l);
                        }
                    });
                app.convert_expr = Some(e);
            });
            ui.checkbox(&mut app.convert_keep, "Keep original layer")
                .on_hover_text("the converted copy lands above the original; off = replace in place");
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut app.convert_name);
            });
            ui.separator();
            if ui.button("Convert").clicked() {
                app.push_cmd(AppCmd::ConvertLayer {
                    rasterize,
                    expression: app.convert_expr,
                    blend: None,
                    keep_original: app.convert_keep,
                    name: Some(app.convert_name.clone()),
                });
            }
            ui.small("one undo step");
        });
    if !open {
        app.convert_open = false;
    }
}

fn expr_label(e: LayerExpression) -> &'static str {
    match e {
        LayerExpression::Colour => "Colour",
        LayerExpression::Grey => "Grey",
        LayerExpression::Mono => "Mono",
    }
}

pub(super) fn extract_window(ctx: &egui::Context, app: &mut App) {
    if !app.extract_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Extract Lines")
        .open(&mut open)
        .resizable(false)
        .default_pos(egui::pos2(320.0, 120.0))
        .show(ctx, |ui| {
            ui.add(
                egui::Slider::new(&mut app.extract_detection, 0.05..=0.98)
                    .text("line detection")
                    .fixed_decimals(2),
            )
            .on_hover_text(
                "luma below this becomes line ink — darker pixels come out stronger lines",
            );
            if ui.button("Extract").clicked() {
                app.push_cmd(AppCmd::ExtractLines {
                    detection: app.extract_detection,
                });
            }
            ui.small("a fresh line layer lands above — one undo");
        });
    if !open {
        app.extract_open = false;
    }
}

/// Layer ▸ Convert to lines and tones… (row 154, `CL-001`–`014`): the
/// render→page bridge's dialog. Every control maps to one CL row; the
/// preset pool (`CL-002`) and the multi-node levels bars (`CL-003`/`012`)
/// are not here — see the module note in `mn_core::convert_lt`.
pub(super) fn lines_tones_window(ctx: &egui::Context, app: &mut App) {
    if !app.lt_open {
        return;
    }
    let mut open = true;
    let mut run = false;
    egui::Window::new("Convert to Lines and Tones")
        .open(&mut open)
        .resizable(false)
        .default_pos(egui::pos2(320.0, 120.0))
        .show(ctx, |ui| {
            let is_folder = app.doc.active_layer().folder;
            ui.label(format!("“{}”", app.doc.active_layer().name));
            if is_folder {
                ui.small("pick a layer, not a folder");
            }
            ui.separator();
            let p = &mut app.lt_params;

            ui.label("Lines");
            ui.add(
                egui::Slider::new(&mut p.strength, 0.0..=1.0)
                    .text("strength")
                    .fixed_decimals(2),
            )
            .on_hover_text("how eagerly edges are detected — higher finds fainter ones");
            ui.add(egui::Slider::new(&mut p.width, 0..=3).text("thickness px"))
                .on_hover_text("the detected ink is grown by this many pixels");
            ui.add(egui::Slider::new(&mut p.join, 0..=3).text("line density"))
                .on_hover_text("joins broken runs without fattening solid ones");
            ui.horizontal(|ui| {
                ui.label("directions").on_hover_text(
                    "an arrow that is off still detects edges facing that way, weakly",
                );
                for (i, name) in ["up", "right", "down", "left"].iter().enumerate() {
                    if ui.selectable_label(p.directions[i], *name).clicked() {
                        p.directions[i] = !p.directions[i];
                    }
                }
            });
            let mut post_on = p.posterize.is_some();
            if ui
                .checkbox(&mut post_on, "Posterize before extracting")
                .on_hover_text("flatten the source into bands so edges land on band boundaries")
                .changed()
            {
                p.posterize = post_on.then_some(6);
            }
            if let Some(n) = p.posterize.as_mut() {
                ui.add(egui::Slider::new(n, 2..=16).text("levels"));
            }

            ui.separator();
            let mut beta_on = p.black_fill.is_some();
            if ui
                .checkbox(&mut beta_on, "Black fill (ベタ)")
                .on_hover_text("darker than the threshold becomes solid black on its own layer")
                .changed()
            {
                p.black_fill = beta_on.then_some(0.15);
            }
            if let Some(v) = p.black_fill.as_mut() {
                ui.add(
                    egui::Slider::new(v, 0.0..=0.6)
                        .text("black threshold")
                        .fixed_decimals(2),
                );
            }
            let lo = p.black_fill.unwrap_or(0.0);

            ui.separator();
            let mut tone_on = p.tone.is_some();
            if ui
                .checkbox(&mut tone_on, "Tone")
                .on_hover_text("off = lines only")
                .changed()
            {
                p.tone = tone_on.then(mn_core::convert_lt::ToneOutput::default);
            }
            if let Some(t) = p.tone.as_mut() {
                let mut reset = ui
                    .add(
                        egui::Slider::new(&mut t.bands, 1..=mn_core::convert_lt::MAX_BANDS)
                            .text("tone bands"),
                    )
                    .changed();
                reset |= ui
                    .add(
                        egui::Slider::new(&mut t.white_point, 0.5..=1.0)
                            .text("white point")
                            .fixed_decimals(2),
                    )
                    .on_hover_text("lighter than this stays paper — no dots")
                    .changed();
                if reset || t.densities.len() != t.bands as usize {
                    t.densities = mn_core::convert_lt::band_densities(t.bands, lo, t.white_point);
                }
                ui.horizontal(|ui| {
                    ui.label("densities").on_hover_text(
                        "ink per band, darkest first — editable, and re-derived when the \
                         band count or white point moves",
                    );
                    for d in t.densities.iter_mut() {
                        ui.add(
                            egui::DragValue::new(d)
                                .speed(0.01)
                                .range(0.0..=1.0)
                                .fixed_decimals(2),
                        );
                    }
                });
                ui.checkbox(&mut t.grayscale, "Grayscale")
                    .on_hover_text("flat grey instead of dots — type/angle/frequency stop mattering");
                ui.add_enabled_ui(!t.grayscale, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("type");
                        egui::ComboBox::from_id_salt("mn.lt.tone.pattern")
                            .width(84.0)
                            .selected_text(t.params.pattern.label())
                            .show_ui(ui, |ui| {
                                for pat in mn_core::TonePattern::ALL {
                                    ui.selectable_value(&mut t.params.pattern, pat, pat.label());
                                }
                            });
                        ui.add(
                            egui::DragValue::new(&mut t.params.lpi)
                                .speed(0.5)
                                .range(5.0..=80.0)
                                .suffix(" LPI"),
                        );
                        ui.add(
                            egui::DragValue::new(&mut t.params.angle_deg)
                                .speed(1.0)
                                .range(0.0..=90.0)
                                .suffix("°"),
                        );
                    });
                });
            }

            ui.separator();
            ui.checkbox(&mut p.keep_original, "Keep original layer")
                .on_hover_text("kept but hidden; off = the source layer is deleted");
            if ui
                .add_enabled(!is_folder, egui::Button::new("Convert"))
                .clicked()
            {
                run = true;
            }
            ui.small("lines, ベタ and live tone layers land in a folder above — one undo");
        });
    if run {
        let params = Box::new(app.lt_params.clone());
        app.push_cmd(AppCmd::ConvertLinesTones { params });
    }
    if !open {
        app.lt_open = false;
    }
}

pub(super) fn advfill_window(ctx: &egui::Context, app: &mut App) {
    if !app.advfill_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Advanced Fill")
        .open(&mut open)
        .resizable(false)
        .default_pos(egui::pos2(320.0, 120.0))
        .show(ctx, |ui| {
            let target = if app.doc.selection.is_some() {
                "the selection"
            } else {
                "the whole layer"
            };
            ui.label(format!("Fills {target} with the main colour."));
            ui.add(
                egui::Slider::new(&mut app.advfill_opacity, 0.01..=1.0)
                    .text("opacity")
                    .fixed_decimals(2),
            )
            .on_hover_text("a src-over — the ink under the fill blends through");
            if ui.button("Fill").clicked() {
                app.push_cmd(AppCmd::AdvancedFill {
                    opacity: app.advfill_opacity,
                });
            }
            ui.small("one undo step");
        });
    if !open {
        app.advfill_open = false;
    }
}
