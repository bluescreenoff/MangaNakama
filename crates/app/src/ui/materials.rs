//! The Materials palette (TRIAGE 133 part 1), reshaped to CSP's Material
//! palette (`research/ui-parity-materials-menus-2026-08-22.md`, P0-1..P1-4):
//! a folder TREE on the left, a virtualized thumbnail grid on the right, a
//! material-information strip along the bottom, and everything that is not
//! search-or-sort behind a `≡` palette menu. Click selects; double-click
//! pastes the material as the move/scale float (the clipboard's stamp
//! path); with tiling on, one paste covers the canvas in N×N copies as a
//! single float — a mask to draw through.

use super::theme;
use crate::app::App;
use crate::app::materials::THUMB_STEPS;
use crate::cmd::AppCmd;

/// The tree column. Narrow on purpose: it is a navigation strip, and the
/// grid is what the palette is for.
const TREE_W: f32 = 128.0;
/// Below this the palette is all tree and no materials, so the tree hides
/// itself rather than squeezing the grid to one column.
const TREE_MIN_PALETTE_W: f32 = 300.0;
/// The paste-option row (Tile / Tone / size / order) needs about this much
/// to lay out without clipping. Narrower than that it lives only in the ≡
/// menu — the owner's dock is ~390px and the old row silently cut four
/// buttons off the right edge (owner report 2026-08-22).
const PASTE_ROW_MIN_W: f32 = 330.0;
/// The material-information strip's height — one thumbnail plus four short
/// lines beside it.
const INFO_H: f32 = 68.0;

pub(super) fn materials_palette(ui: &mut egui::Ui, app: &mut App) {
    header(ui, app);
    ui.separator();

    if app.materials.is_empty() {
        ui.weak("no materials yet");
        ui.weak(format!(
            "add PNGs to {} — or ≡ ▸ Add folder…",
            app.material_folders
                .first()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "the materials folder".into())
        ));
        return;
    }

    let order = visible_order(app);
    // The info strip is reserved BEFORE the body so it cannot be pushed off
    // the bottom by a long grid — it is the palette's only view of a
    // material's tags and folder, and a strip you have to scroll to is not one.
    let body_h = (ui.available_height() - INFO_H - 12.0).max(80.0);
    ui.allocate_ui(egui::vec2(ui.available_width(), body_h), |ui| {
        ui.horizontal_top(|ui| {
            if app.material_tree_show && ui.available_width() >= TREE_MIN_PALETTE_W {
                tree_column(ui, app, body_h);
                ui.separator();
            }
            grid(ui, app, &order);
        });
    });
    ui.separator();
    info_strip(ui, app);
}

// --- header (P0-3: search + sort, everything else behind ≡) --------------

fn header(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.material_search)
                .hint_text("search")
                .desired_width(84.0),
        )
        .on_hover_text("matches names and tags — right-click a material to tag it");
        let sort_label = if app.material_sort_uses {
            "most used"
        } else {
            "name"
        };
        egui::ComboBox::from_id_salt("mn.materials.sort")
            .width(72.0)
            .selected_text(sort_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.material_sort_uses, false, "name");
                ui.selectable_value(&mut app.material_sort_uses, true, "most used");
            });
        // CSP's palette menu sits at the palette's own corner, not in the
        // control flow — right-aligned so it stays put as the row narrows.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("≡", |ui| palette_menu(ui, app));
            thumb_size_button(ui, app);
        });
    });
    if ui.available_width() >= PASTE_ROW_MIN_W {
        ui.horizontal(|ui| paste_options(ui, app));
    }
}

/// CSP's hamburger: the commands that are not search-or-sort. Everything in
/// here is also reachable at a wider dock, except the folder commands —
/// those live ONLY here, which is the whole point of P0-3.
fn palette_menu(ui: &mut egui::Ui, app: &mut App) {
    ui.set_min_width(190.0);
    ui.checkbox(&mut app.material_tree_show, "Folder tree");
    ui.separator();
    if ui.button("Add folder…").clicked()
        && let Some(p) = rfd::FileDialog::new()
            .set_title("Add material folder")
            .pick_folder()
    {
        app.push_cmd(AppCmd::MaterialAddFolder(p));
    }
    if ui
        .button("Import folder…")
        .on_hover_text("copy that folder's images into the registered bank folder")
        .clicked()
        && let Some(p) = rfd::FileDialog::new()
            .set_title("Import images into the material bank")
            .pick_folder()
    {
        app.push_cmd(AppCmd::MaterialImportFolder(p));
    }
    if ui
        .button("Register layer as material")
        .on_hover_text("the active layer becomes an image material — a selection scopes it")
        .clicked()
    {
        app.push_cmd(AppCmd::MaterialRegisterLayer);
    }
    if ui.button("Rescan").clicked() {
        app.push_cmd(AppCmd::MaterialRescan);
    }
    ui.separator();
    ui.label(egui::RichText::new("Paste options").small().weak());
    ui.vertical(|ui| paste_options(ui, app));
}

/// Tile / Tone / paste size / layer order. Rendered in BOTH the ≡ menu and
/// the header's slim second row — the row is the working surface when the
/// dock is wide enough, the menu is the guarantee that a narrow dock can
/// still reach them.
fn paste_options(ui: &mut egui::Ui, app: &mut App) {
    ui.checkbox(&mut app.material_tile, "Tile")
        .on_hover_text("paste covers the whole canvas in N×N copies — a mask to draw through");
    ui.checkbox(&mut app.material_tone, "Tone").on_hover_text(
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
        .width(100.0)
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
        .width(100.0)
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
}

/// P1-4's small/medium/large cycle. Changing the size invalidates every
/// decoded thumbnail — they are minted AT the display size, so a stale
/// cache would render 36px art in a 76px cell.
fn thumb_size_button(ui: &mut egui::Ui, app: &mut App) {
    let step = THUMB_STEPS
        .iter()
        .position(|&s| s == app.material_thumb_px)
        .unwrap_or(1);
    let label = ["S", "M", "L"][step.min(2)];
    if ui
        .small_button(label)
        .on_hover_text("thumbnail size — small / medium / large")
        .clicked()
    {
        app.material_thumb_px = THUMB_STEPS[(step + 1) % THUMB_STEPS.len()];
        app.material_thumbs.clear();
        app.material_thumb_lru.clear();
    }
}

// --- the folder tree (P0-1) ----------------------------------------------

/// CSP's region 3. The tree is pre-order with a `depth` per row, so a
/// collapsed branch is hidden by skipping rows until the depth drops back —
/// no recursion, no parent pointers.
fn tree_column(ui: &mut egui::Ui, app: &mut App, height: f32) {
    ui.allocate_ui_with_layout(
        egui::vec2(TREE_W, height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            egui::ScrollArea::both()
                .id_salt("mn.materials.tree")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let mut skip_below: Option<usize> = None;
                    for n in 0..app.material_tree.len() {
                        let (depth, closed, label, children, filter) = {
                            let node = &app.material_tree[n];
                            (
                                node.depth,
                                app.material_tree_closed.contains(&node.filter.id()),
                                format!("{} ({})", node.label, node.count),
                                node.children,
                                node.filter.clone(),
                            )
                        };
                        if let Some(d) = skip_below {
                            if depth > d {
                                continue;
                            }
                            skip_below = None;
                        }
                        if closed {
                            skip_below = Some(depth);
                        }
                        ui.horizontal(|ui| {
                            ui.add_space(depth as f32 * 8.0);
                            // A leaf gets the chevron's WIDTH but no button,
                            // so labels down one branch stay aligned.
                            if children {
                                let chev = if closed { "▸" } else { "▾" };
                                if ui.small_button(chev).clicked() {
                                    let id = filter.id();
                                    if !app.material_tree_closed.remove(&id) {
                                        app.material_tree_closed.insert(id);
                                    }
                                }
                            } else {
                                ui.add_space(14.0);
                            }
                            let sel = app.material_filter == filter;
                            if ui
                                .selectable_label(sel, egui::RichText::new(label).small())
                                .clicked()
                            {
                                app.material_filter = filter;
                                app.material_selected = None;
                            }
                        });
                    }
                });
        },
    );
}

// --- the grid (P0-2: virtualized, no per-frame item clone) ---------------

/// The filtered, sorted indices into `app.materials`. Indices, not items:
/// the old code cloned a whole `MaterialItem` per cell per frame.
fn visible_order(app: &App) -> Vec<usize> {
    // MT-012: the one box searches names AND tags — no second field.
    let search = app.material_search.to_lowercase();
    let mut order: Vec<usize> = (0..app.materials.len())
        .filter(|&i| {
            app.material_filter.accepts(&app.materials[i])
                && crate::app::materials::material_matches(&app.materials[i], &search)
        })
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
    order
}

fn grid(ui: &mut egui::Ui, app: &mut App, order: &[usize]) {
    if order.is_empty() {
        ui.weak("nothing matches — clear the search, or pick All materials");
        return;
    }
    let thumb = app.material_thumb_px;
    // CSP's cell: thumbnail with the name centred UNDERNEATH it.
    let cell = egui::vec2(thumb + 16.0, thumb + 22.0);
    let gap = 4.0;
    let cols = (((ui.available_width() + gap) / (cell.x + gap)).floor() as usize).max(1);
    let rows = order.len().div_ceil(cols);
    egui::ScrollArea::vertical()
        .id_salt("mn.materials.grid")
        .auto_shrink([false, false])
        // show_rows lays out only the visible band. A uniform cell is what
        // buys it: row height is a constant, so egui can jump straight to
        // the right band instead of measuring 1200 rows.
        .show_rows(ui, cell.y + gap, rows, |ui, range| {
            ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
            for r in range {
                ui.horizontal(|ui| {
                    for c in 0..cols {
                        let Some(&i) = order.get(r * cols + c) else {
                            break;
                        };
                        material_cell(ui, app, i, cell);
                    }
                });
            }
        });
}

fn material_cell(ui: &mut egui::Ui, app: &mut App, i: usize, size: egui::Vec2) {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    // Everything read out of the item is a small copy taken ONCE — never
    // the item itself (P0-2: that clone was two Strings, a PathBuf and
    // possibly a GenLinesSpec, per cell per frame).
    let (path, name, tags, is_gen, thumb_path) = {
        let m = &app.materials[i];
        (
            m.path.clone(),
            m.name.clone(),
            m.tags.clone(),
            m.is_generator(),
            m.thumb_path(),
        )
    };
    let selected = app.material_selected == Some(i);

    if ui.is_rect_visible(rect) {
        if selected {
            ui.painter().rect_filled(rect, theme::R_CTRL, theme::SEL_ROW);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, theme::R_CTRL, theme::HOVER);
        }
        let thumb_px = app.material_thumb_px;
        let box_ = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.top() + 3.0 + thumb_px / 2.0),
            egui::vec2(thumb_px, thumb_px),
        );
        match app.material_thumbs.get(&path).cloned() {
            Some(t) => paint_thumb(ui, &t, box_),
            None => {
                // A generator material's picture is the PNG beside its
                // spec; the cache still keys on the material's own path.
                if let Some(t) = load_thumb(app, &thumb_path, thumb_px) {
                    app.material_thumbs.insert(path.clone(), t);
                } else {
                    ui.painter().rect_filled(box_, theme::R_CTRL, theme::FIELD);
                }
            }
        }
        // A generator material reads as different because it BEHAVES
        // differently: it places editable effect lines, not pixels.
        let text = if is_gen {
            format!("{name} (live)")
        } else {
            name.clone()
        };
        let galley = egui::WidgetText::from(egui::RichText::new(text).small()).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            size.x - 4.0,
            egui::TextStyle::Small,
        );
        let col = if selected {
            theme::TEXT_STRONG
        } else {
            theme::TEXT_WEAK
        };
        ui.painter().galley(
            egui::pos2(
                rect.center().x - galley.size().x / 2.0,
                rect.bottom() - galley.size().y - 2.0,
            ),
            galley,
            col,
        );
        app.material_thumb_touch(&path);
    }

    if resp.clicked() {
        // P1-2: a click SELECTS. Pasting on the first click meant a
        // material could never be inspected without being used.
        app.material_selected = Some(i);
    }
    if resp.double_clicked() {
        paste(app, &path);
    }
    // Built only while hovered: the info strip is where the full detail
    // lives now, so the other cells never format a line of it.
    let resp = if resp.hovered() {
        let what = if is_gen {
            "double-click to place LIVE effect lines (the Object tool re-aims them)"
        } else {
            "double-click to paste"
        };
        let hover = if tags.is_empty() {
            format!("{name} — {what}, right-click to tag")
        } else {
            format!("{name}\n{tags}\n{what}, right-click to tag")
        };
        resp.on_hover_text(hover)
    } else {
        resp
    };
    material_tag_menu(&resp, app, path, name, tags);
}

/// Paint a thumbnail INSIDE `box_` at its own aspect ratio. `paint_at`
/// stretches to the rect it is given, and a stretched 2000×400 sound effect
/// is unrecognisable in a 52px cell.
fn paint_thumb(ui: &egui::Ui, tex: &egui::TextureHandle, box_: egui::Rect) {
    let sz = tex.size_vec2();
    let k = (box_.width() / sz.x).min(box_.height() / sz.y).min(1.0);
    let fitted = egui::Rect::from_center_size(box_.center(), sz * k);
    egui::Image::from_texture(tex).paint_at(ui, fitted);
}

fn paste(app: &mut App, path: &std::path::Path) {
    let tile = app.material_tile;
    app.push_cmd(AppCmd::PasteMaterial {
        path: path.to_path_buf(),
        tile,
    });
}

// --- the material-information strip (P1-1) -------------------------------

/// CSP's region 6, kept to what we actually know about a material:
/// thumbnail, name, where it lives, its tags, how often it has been used.
/// The Paste button is here because a double-click in the grid is the only
/// other way to apply one, and a discoverable palette needs a visible verb.
fn info_strip(ui: &mut egui::Ui, app: &mut App) {
    let Some(i) = app.material_selected.filter(|&i| i < app.materials.len()) else {
        ui.allocate_ui(egui::vec2(ui.available_width(), INFO_H), |ui| {
            ui.weak("click a material to see its details");
        });
        return;
    };
    let (path, name, is_gen, thumb_path, tags, folder, rel) = {
        let m = &app.materials[i];
        (
            m.path.clone(),
            m.name.clone(),
            m.is_generator(),
            m.thumb_path(),
            m.tags.clone(),
            m.folder,
            m.rel.clone(),
        )
    };
    let uses = app
        .material_uses
        .get(&path.display().to_string())
        .copied()
        .unwrap_or(0);
    let mut where_it_lives = app
        .material_folder_names
        .get(folder)
        .cloned()
        .unwrap_or_else(|| "materials".into());
    if !rel.as_os_str().is_empty() {
        where_it_lives.push('/');
        where_it_lives.push_str(&rel.display().to_string().replace('\\', "/"));
    }

    ui.allocate_ui(egui::vec2(ui.available_width(), INFO_H), |ui| {
        ui.horizontal_top(|ui| {
            // Sized from the grid's own thumbnail, so the ONE cached
            // texture per material is always minted at the size both
            // places draw it.
            let side = app.material_thumb_px.min(INFO_H - 12.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
            match app.material_thumbs.get(&path).cloned() {
                Some(t) => paint_thumb(ui, &t, rect),
                // The selected material may have been scrolled far enough
                // away to be evicted; the strip is not allowed to go blank
                // just because the grid forgot it.
                None => match load_thumb(app, &thumb_path, app.material_thumb_px) {
                    Some(t) => {
                        paint_thumb(ui, &t, rect);
                        app.material_thumbs.insert(path.clone(), t);
                    }
                    None => {
                        ui.painter().rect_filled(rect, theme::R_CTRL, theme::FIELD);
                    }
                },
            }
            app.material_thumb_touch(&path);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                let title = if is_gen {
                    format!("{name} (live)")
                } else {
                    name.clone()
                };
                ui.label(egui::RichText::new(title).strong());
                ui.label(egui::RichText::new(where_it_lives).small().weak());
                ui.label(
                    egui::RichText::new(if tags.is_empty() {
                        "no tags — right-click the cell to add some".to_owned()
                    } else {
                        tags
                    })
                    .small(),
                );
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("used ×{uses}")).small().weak());
                    if ui.small_button("Paste").clicked() {
                        paste(app, &path);
                    }
                });
            });
        });
    });
}

/// MT-012's editor: the bank's per-item actions live on the cell itself, so
/// tags go on the same cell's right-click menu rather than a properties
/// pane nobody would find. One comma-separated line — Enter or the button
/// writes the folder's sidecar, Esc/click-away drops it.
fn material_tag_menu(
    resp: &egui::Response,
    app: &mut App,
    path: std::path::PathBuf,
    name: String,
    tags: String,
) {
    resp.context_menu(|ui| {
        ui.set_min_width(190.0);
        ui.label(egui::RichText::new(&name).small());
        // Seed (or re-seed) the buffer from what is on disk. Re-seeding on a
        // different path is what stops the menu from carrying one material's
        // half-typed tags onto the next one you right-click.
        let buf = app
            .material_tag_edit
            .get_or_insert_with(|| (path.clone(), tags.clone()));
        if buf.0 != path {
            *buf = (path.clone(), tags.clone());
        }
        let edit = ui.add(
            egui::TextEdit::singleline(&mut buf.1)
                .hint_text("tags, comma separated")
                .desired_width(180.0),
        );
        let entered = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let saved = ui.button("Save tags").clicked();
        if entered || saved {
            let tags = std::mem::take(&mut buf.1);
            app.material_tag_edit = None;
            app.push_cmd(AppCmd::MaterialSetTags { path, tags });
            ui.close();
        }
    });
}

/// Decode one thumbnail at the CURRENT display size. Minting at the display
/// size rather than the largest is why the size cycle clears the cache: a
/// 300-texture cache at 76px is already 6.9 MB of VRAM.
fn load_thumb(app: &mut App, path: &std::path::Path, px: f32) -> Option<egui::TextureHandle> {
    let img = image::open(path).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let scale = (px / w.max(h) as f32).min(1.0);
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
