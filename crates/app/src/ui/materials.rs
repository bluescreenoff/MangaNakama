//! The Materials palette (TRIAGE 133 part 1): the bank as a dock tab —
//! folder-grouped thumbnails, search, name/most-used sort (MT-016's
//! frequency), the owner's tiling toggle, folder management. Click pastes
//! the material as the move/scale float (the clipboard's stamp path);
//! with tiling on, one click covers the canvas in N×N copies as a single
//! float — a mask to draw through.

use crate::app::App;
use crate::cmd::AppCmd;

const THUMB: f32 = 52.0;

pub(super) fn materials_palette(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.material_search)
                .hint_text("search")
                .desired_width(90.0),
        );
        let sort_label = if app.material_sort_uses {
            "most used"
        } else {
            "name"
        };
        egui::ComboBox::from_id_salt("mn.materials.sort")
            .width(76.0)
            .selected_text(sort_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.material_sort_uses, false, "name");
                ui.selectable_value(&mut app.material_sort_uses, true, "most used");
            });
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut app.material_tile, "Tile")
            .on_hover_text("paste covers the whole canvas in N×N copies — a mask to draw through");
        ui.checkbox(&mut app.material_tone, "Tone")
            .on_hover_text(
                "paste as the document's screentone (60 LPI 45° dots) — makes any image printable on a mono page",
            );
        // MT-032: CSP's paste-size vocabulary — five meanings of "paste
        // this at the right size", named after the job.
        let size_label = match app.material_size {
            crate::app::MaterialPasteSize::FitPanel => "fit panel",
            crate::app::MaterialPasteSize::AdjustAfter => "adjust after",
            crate::app::MaterialPasteSize::ExpandFull => "expand in full",
            crate::app::MaterialPasteSize::FitToScale => "fit to scale",
            crate::app::MaterialPasteSize::ToDestination => "to destination",
        };
        egui::ComboBox::from_id_salt("mn.materials.size")
            .width(104.0)
            .selected_text(size_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut app.material_size,
                    crate::app::MaterialPasteSize::FitPanel,
                    "fit panel",
                )
                .on_hover_text("uniform down-fit into the panel, never crop (the default)");
                ui.selectable_value(
                    &mut app.material_size,
                    crate::app::MaterialPasteSize::AdjustAfter,
                    "adjust after",
                )
                .on_hover_text("original size — drag/scale by hand after the paste");
                ui.selectable_value(
                    &mut app.material_size,
                    crate::app::MaterialPasteSize::ExpandFull,
                    "expand in full",
                )
                .on_hover_text("fill the panel, overflow cropped (backgrounds)");
                ui.selectable_value(
                    &mut app.material_size,
                    crate::app::MaterialPasteSize::FitToScale,
                    "fit to scale",
                )
                .on_hover_text("the whole material fits inside the panel (sound effects)");
                ui.selectable_value(
                    &mut app.material_size,
                    crate::app::MaterialPasteSize::ToDestination,
                    "to destination",
                )
                .on_hover_text("stretch to the panel rect exactly (patterns)");
            });
        // MT-034: where the pasted layer sits in the panel folder.
        let order_label = match app.material_order {
            crate::app::MaterialLayerOrder::Above => "top of panel",
            crate::app::MaterialLayerOrder::BottomOfPanel => "bottom of panel",
        };
        egui::ComboBox::from_id_salt("mn.materials.order")
            .width(104.0)
            .selected_text(order_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut app.material_order,
                    crate::app::MaterialLayerOrder::Above,
                    "top of panel",
                );
                ui.selectable_value(
                    &mut app.material_order,
                    crate::app::MaterialLayerOrder::BottomOfPanel,
                    "bottom of panel",
                );
            });
        if ui.small_button("＋ folder").clicked() {
            if let Some(p) = rfd::FileDialog::new()
                .set_title("Add material folder")
                .pick_folder()
            {
                app.push_cmd(AppCmd::MaterialAddFolder(p));
            }
        }
        if ui
            .small_button("import folder…")
            .on_hover_text("copy that folder's images into the registered bank folder")
            .clicked()
        {
            if let Some(p) = rfd::FileDialog::new()
                .set_title("Import images into the material bank")
                .pick_folder()
            {
                app.push_cmd(AppCmd::MaterialImportFolder(p));
            }
        }
        if ui
            .small_button("register layer")
            .on_hover_text("the active layer becomes an image material — a selection scopes it")
            .clicked()
        {
            app.push_cmd(AppCmd::MaterialRegisterLayer);
        }
        if ui.small_button("rescan").clicked() {
            app.push_cmd(AppCmd::MaterialRescan);
        }
    });
    ui.separator();

    if app.materials.is_empty() {
        ui.weak("no materials yet");
        ui.weak(format!(
            "add PNGs to {} — or ＋ folder",
            app.material_folders
                .first()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "the materials folder".into())
        ));
        return;
    }

    let search = app.material_search.to_lowercase();
    let mut order: Vec<usize> = (0..app.materials.len())
        .filter(|&i| app.materials[i].name.to_lowercase().contains(&search))
        .collect();
    if app.material_sort_uses {
        order.sort_by_key(|&i| {
            let uses = app
                .material_uses
                .get(&app.materials[i].path.display().to_string())
                .copied()
                .unwrap_or(0);
            std::cmp::Reverse(uses)
        });
    }

    // Group by folder (folder order is the bank's identity order).
    let folder_count = app.material_folder_names.len().max(1);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for f in 0..folder_count {
                let items: Vec<usize> = order
                    .iter()
                    .copied()
                    .filter(|&i| app.materials[i].folder == f)
                    .collect();
                if items.is_empty() {
                    continue;
                }
                ui.strong(
                    app.material_folder_names
                        .get(f)
                        .cloned()
                        .unwrap_or_else(|| "materials".into()),
                );
                egui::Grid::new(format!("mn.materials.grid.{f}"))
                    .num_columns(2)
                    .spacing([4.0, 4.0])
                    .show(ui, |ui| {
                        for (n, &i) in items.iter().enumerate() {
                            material_cell(ui, app, i);
                            if n % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });
                ui.add_space(4.0);
            }
        });
}

fn material_cell(ui: &mut egui::Ui, app: &mut App, i: usize) {
    let item = app.materials[i].clone();
    let path = item.path.clone();
    let tile = app.material_tile;
    let uses = app
        .material_uses
        .get(&path.display().to_string())
        .copied()
        .unwrap_or(0);

    // Lazy thumbnail: decoded once on first display, cached by path. The
    // name-only button renders the frame the decode lands on.
    let thumb = app.material_thumbs.get(&path).cloned();
    let label = if uses > 0 {
        format!("{}\n×{uses}", item.name)
    } else {
        item.name.clone()
    };
    let btn = match &thumb {
        Some(t) => egui::Button::image_and_text(
            egui::Image::from_texture(t).max_size(egui::vec2(THUMB, THUMB)),
            egui::RichText::new(label).small(),
        ),
        None => egui::Button::new(egui::RichText::new(label).small()),
    };
    let resp = ui
        .add(btn)
        .on_hover_text(format!("{} — click to paste", item.name));
    if thumb.is_none() && ui.is_rect_visible(resp.rect) {
        if let Some(t) = load_thumb(app, &path) {
            app.material_thumbs.insert(path.clone(), t);
        }
    }
    if resp.clicked() {
        app.push_cmd(AppCmd::PasteMaterial { path, tile });
    }
}

fn load_thumb(app: &mut App, path: &std::path::Path) -> Option<egui::TextureHandle> {
    let img = image::open(path).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let scale = (THUMB / w.max(h) as f32).min(1.0);
    let (tw, th) = (
        ((w as f32 * scale) as u32).max(1),
        ((h as f32 * scale) as u32).max(1),
    );
    let resized = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
    let ci = egui::ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], resized.as_raw());
    Some(app.shell.ctx.load_texture(
        format!("mn.material.{}", path.display()),
        ci,
        egui::TextureOptions::LINEAR,
    ))
}
