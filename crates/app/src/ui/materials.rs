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
use crate::app::materials::{MaterialFilter, MaterialType, THUMB_STEPS};
use crate::cmd::AppCmd;
use egui::vec2;

/// The glyph a type row/chip wears. `Other` deliberately gets none — an
/// untyped pile wearing a random glyph would be a lie.
fn material_type_icon(ty: MaterialType) -> Option<super::icons::Icon> {
    use super::icons::Icon;
    Some(match ty {
        MaterialType::Tone => Icon::Tone,
        MaterialType::PatternImage => Icon::Pattern,
        MaterialType::EffectLines => Icon::FocusLines,
        MaterialType::Balloon => Icon::Balloon,
        MaterialType::Pose3d => Icon::Pose3d,
        MaterialType::Other => return None,
    })
}

/// plans/05 item 6 (b): the chip rows under the search box — TYPE chips
/// (with live counts) and the bank's most common USER tags. Clicking a
/// chip filters; clicking the active chip again clears back to All.
fn chip_rows(ui: &mut egui::Ui, app: &mut App) {
    let show_pose3d = app.prefs.show_pose3d_materials;
    let search_active = !app.material_search.is_empty();
    ui.horizontal_wrapped(|ui| {
        for ty in [
            MaterialType::Tone,
            MaterialType::PatternImage,
            MaterialType::EffectLines,
            MaterialType::Balloon,
            MaterialType::Pose3d,
            MaterialType::Other,
        ] {
            if ty == MaterialType::Pose3d && !show_pose3d {
                continue;
            }
            let n = app
                .materials
                .iter()
                .filter(|m| m.material_type == ty)
                .count();
            if n == 0 {
                continue;
            }
            let sel = app.material_filter == MaterialFilter::Type(ty);
            if ui
                .selectable_label(sel, egui::RichText::new(format!("{} {n}", ty.label())).small())
                .clicked()
            {
                app.material_filter = if sel {
                    MaterialFilter::All
                } else {
                    MaterialFilter::Type(ty)
                };
                app.material_selected = None;
            }
        }
    });
    // The bank's top USER tags (system @tags never chip). One row, capped —
    // the chips are a shortcut, not a second tree.
    let mut counts: std::collections::HashMap<String, usize> = Default::default();
    for m in &app.materials {
        for t in m.tags.split([',', '\n']) {
            let t = t.trim();
            if !t.is_empty() && !t.starts_with('@') {
                *counts.entry(t.to_ascii_lowercase()).or_default() += 1;
            }
        }
    }
    let mut top: Vec<(String, usize)> = counts.into_iter().collect();
    top.sort_by(|a, b| (b.1, &a.0).cmp(&(a.1, &b.0)));
    top.truncate(10);
    if !top.is_empty() && !search_active {
        ui.horizontal_wrapped(|ui| {
            for (t, n) in top {
                let sel =
                    app.material_filter == MaterialFilter::Tag(t.clone());
                if ui
                    .selectable_label(sel, egui::RichText::new(format!("{t} {n}")).small())
                    .clicked()
                {
                    app.material_filter = if sel {
                        MaterialFilter::All
                    } else {
                        MaterialFilter::Tag(t.clone())
                    };
                    app.material_selected = None;
                }
            }
        });
    }
}

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
/// The strip with NOTHING selected: one weak line of prompt text. Reserving
/// the full `INFO_H` there spent 50px of grid on blank space.
const INFO_H_EMPTY: f32 = 20.0;

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
    let body_h = (ui.available_height() - info_height(app) - 12.0).max(80.0);
    ui.allocate_ui(egui::vec2(ui.available_width(), body_h), |ui| {
        ui.horizontal_top(|ui| {
            if app.material_tree_show && ui.available_width() >= TREE_MIN_PALETTE_W {
                tree_column(ui, app, body_h);
                ui.separator();
            }
            grid(ui, app, &order, body_h);
        });
    });
    ui.separator();
    info_strip(ui, app);
    // plans/05 item 6 piece 8: the CSP-style command strip — the
    // every-minute operations out of the ≡ menu, one click deep.
    ui.separator();
    bottom_bar(ui, app);
}

/// The palette's bottom command bar: paste, register, folder ops. The ≡
/// keeps the full set; these four are the ones a working session touches.
fn bottom_bar(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        let selected = app.material_selected.is_some();
        if ui
            .add_enabled(selected, egui::Button::new("Paste").small())
            .clicked()
        {
            if let Some(i) = app.material_selected
                && let Some(m) = app.materials.get(i)
            {
                let path = m.path.clone();
                paste(app, &path);
            }
        }
        if ui
            .small_button("Register layer")
            .on_hover_text("the active layer becomes an image material — a selection scopes it")
            .clicked()
        {
            app.push_cmd(AppCmd::MaterialRegisterLayer);
        }
        if ui
            .small_button("Add folder…")
            .clicked()
            && let Some(p) = rfd::FileDialog::new()
                .set_title("Add material folder")
                .pick_folder()
        {
            app.push_cmd(AppCmd::MaterialAddFolder(p));
        }
        if ui.small_button("Rescan").clicked() {
            app.push_cmd(AppCmd::MaterialRescan);
        }
    });
}

/// What the information strip costs the grid this frame. It is a fixed
/// reservation either way — a strip you have to scroll to is not a strip —
/// but the empty state is one line, not four.
fn info_height(app: &App) -> f32 {
    if app.material_selected.is_some_and(|i| i < app.materials.len()) {
        INFO_H
    } else {
        INFO_H_EMPTY
    }
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
    // plans/05 item 6 (b): the type + tag chip rows under search.
    chip_rows(ui, app);
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
    // A TONE material has no paste geometry to choose: it fills the page
    // (or the selection) as a live tone layer, and its screen is
    // canvas-absolute — scaling a tone would scale the dots, which is the
    // bug the live path exists to kill. Grey the two combos out rather
    // than leaving controls armed that cannot do anything.
    let tone_material = app
        .material_selected
        .and_then(|i| app.materials.get(i))
        .is_some_and(|m| m.tone_spec().is_some());
    let row = ui.add_enabled_ui(!tone_material, |ui| paste_geometry(ui, app));
    if tone_material {
        row.response
            .on_hover_text("a tone fills the page or the selection");
    }
}

/// The two paste-geometry combos — size and layer order.
fn paste_geometry(ui: &mut egui::Ui, app: &mut App) {
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
                            // The TYPE rows wear their glyph (plans/05
                            // item 6): a halftone patch, a checker, rays, a
                            // bubble, a mannequin — the eye finds the kind
                            // before it reads the word.
                            if let MaterialFilter::Type(ty) = &filter {
                                if let Some(icon) = material_type_icon(*ty) {
                                    let (rect, _) =
                                        ui.allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
                                    let col = ui.visuals().widgets.inactive.fg_stroke.color;
                                    super::icons::paint(ui.painter(), rect, icon, col);
                                }
                            }
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
    // MT-012: the one box searches names AND tags — no second field. The
    // hidden type never reaches the grid (owner-locked: hidden by default,
    // a setting unhides — plans/05 item 6).
    let show_pose3d = app.prefs.show_pose3d_materials;
    let search = app.material_search.to_lowercase();
    let mut order: Vec<usize> = (0..app.materials.len())
        .filter(|&i| {
            (show_pose3d
                || app.materials[i].material_type
                    != crate::app::materials::MaterialType::Pose3d)
                && app.material_filter.accepts(&app.materials[i])
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

/// The cell box for a thumbnail of `thumb` px: CSP's cell is the thumbnail
/// with the name centred UNDERNEATH it.
fn cell_size(thumb: f32) -> egui::Vec2 {
    egui::vec2(thumb + 16.0, thumb + 22.0)
}

/// How many cells fit across `avail` points. Never zero: a pane too narrow
/// for one whole cell shows one clipped column rather than nothing.
fn grid_cols(avail: f32, cell_w: f32, gap: f32) -> usize {
    (((avail + gap) / (cell_w + gap)).floor() as usize).max(1)
}

/// `height` is threaded in EXPLICITLY, exactly as the tree column's is, and
/// that is the whole fix for the one-row grid the owner hit on 2026-08-22:
/// inside `horizontal_top` a child `Ui`'s `max_rect` is only
/// `interact_size.y` tall (egui sizes a horizontal row from its default item
/// height and lets the contents overflow it). A `ScrollArea` that sizes
/// itself from `available_height()` in there therefore came out ~20px tall —
/// it drew the one row that fits and left the rest of the pane blank, with
/// no scrollbar to hint that 2400 materials were hiding under it.
fn grid(ui: &mut egui::Ui, app: &mut App, order: &[usize], height: f32) {
    if order.is_empty() {
        ui.weak("nothing matches — clear the search, or pick All materials");
        return;
    }
    let cell = cell_size(app.material_thumb_px);
    let gap = 4.0;
    let cols = grid_cols(ui.available_width(), cell.x, gap);
    let rows = order.len().div_ceil(cols);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            egui::ScrollArea::vertical()
                .id_salt("mn.materials.grid")
                .auto_shrink([false, false])
                // show_rows lays out only the visible band. A uniform cell is
                // what buys it: row height is a constant, so egui can jump
                // straight to the right band instead of measuring 1200 rows.
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
        },
    );
}

/// One grid cell's widget id. Stable across frames and derived from the bank
/// index alone, so a test can read the cell back out of the context after a
/// pass and prove which cells were actually laid out.
pub(super) fn cell_id(i: usize) -> egui::Id {
    egui::Id::new(("mn.materials.cell", i))
}

fn material_cell(ui: &mut egui::Ui, app: &mut App, i: usize, size: egui::Vec2) {
    let (_, rect) = ui.allocate_space(size);
    // click_and_drag, not click, for TWO reasons. The drop below is one. The
    // other is that a torn-off palette is an `egui::Window` with no title bar,
    // so its whole body moves the window: against a click-only cell egui's
    // hit test handed the DRAG to the window behind it and the owner could
    // only shove the palette around (report 2026-08-22). A widget that senses
    // drag on top of the window wins that hit test, so the window stays put —
    // no dock-side change needed. Docked panes never had the problem: a dock
    // tab drag starts on the tab, not in the body.
    let resp = ui.interact(rect, cell_id(i), egui::Sense::click_and_drag());
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
    // plans/05 item 6: `@`-prefixed SYSTEM tags (@type=…) never reach the
    // user — the hover, the info strip and the tag editor speak user tags.
    let user_tags = crate::app::materials::MaterialType::user_tags(&tags);

    if ui.is_rect_visible(rect) {
        if selected {
            ui.painter()
                .rect_filled(rect, theme::R_CTRL, theme::c().sel_row);
        } else if resp.hovered() {
            ui.painter()
                .rect_filled(rect, theme::R_CTRL, theme::c().hover);
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
                    ui.painter()
                        .rect_filled(box_, theme::R_CTRL, theme::c().field);
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
            theme::c().text_strong
        } else {
            theme::c().text_weak
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
    // P1-3: drag a cell onto the canvas to place the material THERE.
    if resp.drag_started() {
        app.material_selected = Some(i);
        app.set_status("drop it on the canvas to place it there");
    }
    if resp.dragged() {
        drag_ghost(ui, app, &path);
    }
    if resp.drag_stopped() {
        // `app.last_pointer` is the win32 truth about where the button came
        // up, and `owns_pointer` is the same "is this the canvas?" test the
        // pen router uses. Released back over the UI: the drag was a shove,
        // not a drop — the selection it already made is the whole result.
        let (px, py) = app.last_pointer;
        if !app.shell.owns_pointer(px, py) {
            // No new door: `PasteMaterial` already aims at `last_pointer`
            // (Ctrl+V's paste-to-position rule), and a generator material's
            // `genlines_aim_point` reads the very same field — so the drop
            // point IS the paste point / the point the focus lines converge
            // on, for free.
            paste(app, &path);
        }
    }
    // Built only while hovered: the info strip is where the full detail
    // lives now, so the other cells never format a line of it.
    let resp = if resp.hovered() {
        let what = if is_gen {
            "double-click (or drag onto the canvas) to place LIVE effect lines (the Object tool re-aims them)"
        } else {
            "double-click to paste, or drag onto the canvas to paste there"
        };
        let hover = if user_tags.is_empty() {
            format!("{name} — {what}, right-click to tag")
        } else {
            format!("{name}\n{user_tags}\n{what}, right-click to tag")
        };
        resp.on_hover_text(hover)
    } else {
        resp
    };
    material_tag_menu(&resp, app, path, name, user_tags);
}

/// A translucent copy of the thumbnail under the pointer while a cell is
/// dragged, so the gesture reads as carrying something. Painted straight onto
/// the tooltip layer: `layer_painter` registers no *area*, which matters —
/// an area under the pointer would make `owns_pointer` call the drop "UI"
/// and the paste would never fire. Nothing is drawn if the thumbnail has not
/// been decoded yet; a ghost is a nicety, not the feature.
fn drag_ghost(ui: &egui::Ui, app: &App, path: &std::path::Path) {
    let (Some(pos), Some(tex)) = (ui.ctx().pointer_latest_pos(), app.material_thumbs.get(path))
    else {
        return;
    };
    let sz = tex.size_vec2();
    let k = (app.material_thumb_px / sz.x.max(sz.y)).min(1.0);
    let rect = egui::Rect::from_min_size(pos + egui::vec2(10.0, 10.0), sz * k);
    ui.ctx()
        .layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("mn.materials.drag"),
        ))
        .image(
            tex.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::from_white_alpha(180),
        );
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
        // One line, and `info_height` reserved exactly one line for it — the
        // strip never takes grid height it is not using.
        ui.weak("click a material to see its details");
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
                        ui.painter()
                            .rect_filled(rect, theme::R_CTRL, theme::c().field);
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
                    egui::RichText::new({
                        let u = crate::app::materials::MaterialType::user_tags(&tags);
                        if u.is_empty() {
                            "no tags — right-click the cell to add some".to_owned()
                        } else {
                            u
                        }
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
            // plans/05 item 6c: the material's OWN paste settings + tone
            // numbers. Untagged controls read the palette globals (the
            // header behaviour every bank used before); touching one writes
            // this material's @paste tags; the ⟲ strips them again.
            ui.separator();
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("paste").small().weak());
                let mut p = app
                    .material_paste_selected()
                    .unwrap_or_default();
                let is_tone_material = app
                    .material_selected
                    .and_then(|i| app.materials.get(i))
                    .is_some_and(|m| m.tone_spec().is_some());
                let mut changed = false;
                ui.horizontal(|ui| {
                    let mut tile = p.tile.unwrap_or(app.material_tile);
                    ui.add_enabled_ui(!is_tone_material, |ui| {
                        if ui.checkbox(&mut tile, "Tile").changed() {
                            p.tile = Some(tile);
                            changed = true;
                        }
                        let mut tone = p.tone.unwrap_or(app.material_tone);
                        if ui.checkbox(&mut tone, "Tone").changed() {
                            p.tone = Some(tone);
                            changed = true;
                        }
                        let mut size = p.size.unwrap_or(app.material_size);
                        let label = paste_size_word(size);
                        egui::ComboBox::from_id_salt("mn.mat.own.size")
                            .width(84.0)
                            .selected_text(label)
                            .show_ui(ui, |ui| {
                                for (w, v) in PASTE_SIZE_WORDS {
                                    changed |= ui
                                        .selectable_value(&mut size, v, w)
                                        .changed();
                                }
                            });
                        if size != p.size.unwrap_or(app.material_size) {
                            p.size = Some(size);
                            changed = true;
                        }
                    });
                });
                if changed {
                    app.material_set_paste_selected(p);
                }
                ui.horizontal(|ui| {
                    if p.any() && ui.small_button("⟲ defaults").clicked() {
                        app.material_set_paste_selected(Default::default());
                    }
                    if !p.any() {
                        ui.label(egui::RichText::new("using the palette defaults").small().weak());
                    }
                });
                // A tone material's NUMBERS, editable in place: the info
                // pane is what makes the 2399 bank's tones live-usable
                // (plans/05 item 6 piece 7).
                if is_tone_material {
                    ui.separator();
                    tone_settings(ui, app, &name);
                }
            });
        });
    });
}

const PASTE_SIZE_WORDS: [(&str, crate::app::MaterialPasteSize); 5] = [
    ("fit panel", crate::app::MaterialPasteSize::FitPanel),
    ("adjust after", crate::app::MaterialPasteSize::AdjustAfter),
    ("expand in full", crate::app::MaterialPasteSize::ExpandFull),
    ("fit to scale", crate::app::MaterialPasteSize::FitToScale),
    ("to destination", crate::app::MaterialPasteSize::ToDestination),
];

fn paste_size_word(s: crate::app::MaterialPasteSize) -> &'static str {
    PASTE_SIZE_WORDS
        .iter()
        .find(|(_, v)| *v == s)
        .map(|(w, _)| *w)
        .unwrap_or("fit panel")
}

/// Density / frequency / angle for the selected TONE material, written
/// straight into its `.tone.json` (write_tone_spec's production debut).
fn tone_settings(ui: &mut egui::Ui, app: &mut App, name: &str) {
    let Some(i) = app.material_selected else { return };
    let Some(m) = app.materials.get(i) else { return };
    let Some(mut spec) = m.tone_spec() else { return };
    ui.label(egui::RichText::new("tone").small().weak());
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut pct = spec.density * 100.0;
        if ui
            .add(
                egui::DragValue::new(&mut pct)
                    .range(1.0..=100.0)
                    .speed(1.0)
                    .fixed_decimals(0)
                    .suffix(" %"),
            )
            .changed()
        {
            spec.density = (pct / 100.0).clamp(0.01, 1.0);
            spec.tone.density = mn_core::tone::ToneDensity::Specified(spec.density);
            changed = true;
        }
        if ui
            .add(
                egui::DragValue::new(&mut spec.tone.lpi)
                    .range(5.0..=80.0)
                    .speed(0.5)
                    .fixed_decimals(1)
                    .suffix(" lpi"),
            )
            .changed()
        {
            changed = true;
        }
        if ui
            .add(
                egui::DragValue::new(&mut spec.tone.angle_deg)
                    .range(0.0..=360.0)
                    .speed(1.0)
                    .fixed_decimals(0)
                    .suffix(" °"),
            )
            .changed()
        {
            changed = true;
        }
    });
    if changed
        && let Some(dir) = m.path.parent()
        && let Some(_) =
            crate::app::materials::write_tone_spec(dir, name, &spec)
    {
        // The bank entry follows the sidecar in place — a full rescan
        // would throw away every decoded thumbnail for one number.
        if let Some(m) = app.materials.get_mut(i) {
            m.kind = crate::app::materials::MaterialKind::Tone(spec);
        }
        app.set_status(format!("tone {name}: {} % @ {:.1} lpi", (spec.density * 100.0) as u32, spec.tone.lpi));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_too_narrow_for_a_cell_still_shows_one_column() {
        assert_eq!(grid_cols(220.0, 68.0, 4.0), 3);
        assert_eq!(grid_cols(71.0, 68.0, 4.0), 1);
        assert_eq!(grid_cols(0.0, 68.0, 4.0), 1);
    }

    /// The empty strip is one line, not four: with nothing selected it used
    /// to reserve the full `INFO_H` and the grid paid for the blank space.
    #[test]
    fn the_information_strip_only_reserves_what_it_draws() {
        assert!(INFO_H_EMPTY < INFO_H);
    }

    /// Owner report 2026-08-22: the palette drew ONE row of thumbnails and
    /// then dead space, docked or floating. The grid's `ScrollArea` sized
    /// itself from `available_height()` inside `horizontal_top`, where a
    /// child `Ui`'s `max_rect` is one `interact_size.y` tall — so a 700px
    /// pane got a ~20px scroll viewport. Both halves of the fix are asserted
    /// here: the grid fills the pane, and a cell senses DRAG (against a
    /// click-only cell egui's hit test handed the drag to the `egui::Window`
    /// behind it, and dragging a thumbnail moved the whole palette instead of
    /// carrying the material to the canvas).
    #[test]
    fn the_grid_fills_the_pane_and_cells_carry_the_drag() {
        let Some(renderer) = crate::app::headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (360, 700), 1.0);
        let mat_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/materials");
        assert!(
            mat_dir.join("tones/tone-dot-60lpi-10.png").is_file(),
            "the starter materials must ship in assets/materials"
        );
        if app.materials.is_empty() {
            app.material_folders[0] = mat_dir;
            app.materials_scan();
        }
        let n = app.materials.len();
        assert!(n >= 12, "the starter bank must overflow one row: {n} items");

        let ctx = app.shell.ctx.clone();
        let raw = app.shell.begin((360, 700));
        let mut out = ctx.run_ui(raw, |ui| materials_palette(ui, &mut app));
        // No GPU pass in this test, so the thumbnail uploads are ours to drop.
        out.textures_delta.clear();

        let laid_out = (0..n)
            .filter(|&i| ctx.read_response(cell_id(i)).is_some())
            .count();
        let cols = grid_cols(360.0 - TREE_W, cell_size(app.material_thumb_px).x, 4.0);
        assert!(
            laid_out > cols,
            "a 700px-tall pane laid out {laid_out} cells across {cols} columns — that is one row"
        );

        let cell = ctx.read_response(cell_id(0)).expect("cell 0 was laid out");
        assert!(
            cell.sense.senses_drag(),
            "a cell must sense drag, or the floating palette swallows the gesture and moves itself"
        );
        assert!(cell.sense.senses_click(), "a cell still selects on click");
    }
}
