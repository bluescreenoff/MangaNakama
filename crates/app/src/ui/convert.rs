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
