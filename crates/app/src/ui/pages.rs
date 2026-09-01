//! The Pages palette (CSP EX page manager): spread-grouped thumbnails,
//! lazy per-frame thumb decode, click-to-switch, drag reorder.

use super::icons::Icon;
use super::theme;
use super::widgets::icon_btn;
use crate::app::App;
use crate::cmd::AppCmd;

#[derive(Clone, Copy)]
struct PageDrag(usize);

// --- pages palette ------------------------------------------------------

/// The dock tab body (the old chrome wrapper is gone — ui/dock.rs).
pub(super) fn pages_palette(ui: &mut egui::Ui, app: &mut App) {
    pages_body(ui, app);
}

// --- preflight palette (TRIAGE 132, part 2) -----------------------------

/// The print-preflight sidebar: CSP's binding-list Confirm column as a
/// live palette. Recomputes when the cache key trips (palette opened, page
/// switched, the active page's revision moved, work metadata edited);
/// "Re-check" forces it. Non-active pages decode from their stashed ORA
/// bytes — only the page being edited can change under the cache.
pub(super) fn preflight_palette(ui: &mut egui::Ui, app: &mut App) {
    let findings = app.preflight_cached();
    let errors = findings
        .iter()
        .filter(|f| f.level == mn_core::PreflightLevel::Error)
        .count();
    let warns = findings.len() - errors;

    ui.horizontal(|ui| {
        if errors > 0 {
            ui.colored_label(
                egui::Color32::from_rgb(196, 74, 74),
                format!("{errors} error{}", if errors > 1 { "s" } else { "" }),
            );
        }
        if warns > 0 {
            ui.colored_label(
                egui::Color32::from_rgb(196, 158, 46),
                format!("{warns} warning{}", if warns > 1 { "s" } else { "" }),
            );
        }
        if findings.is_empty() {
            ui.colored_label(
                egui::Color32::from_rgb(96, 168, 96),
                format!(
                    "all checks pass ({} page{})",
                    app.pages.len(),
                    if app.pages.len() > 1 { "s" } else { "" }
                ),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Re-check").clicked() {
                app.preflight_findings = Some(app.run_preflight());
            }
        });
    });
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for f in &findings {
                ui.horizontal(|ui| {
                    let (glyph, color) = match f.level {
                        mn_core::PreflightLevel::Error => {
                            ("✕", egui::Color32::from_rgb(196, 74, 74))
                        }
                        mn_core::PreflightLevel::Warn => {
                            ("⚠", egui::Color32::from_rgb(196, 158, 46))
                        }
                    };
                    ui.colored_label(color, glyph);
                    ui.add(egui::Label::new(&f.message).wrap().selectable(false));
                });
            }
        });
}

fn pages_body(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        if icon_btn(ui, Icon::Plus, 15.0, false, true, "Add page after this one").clicked() {
            app.push_cmd(AppCmd::AddPage);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_btn(ui, Icon::Trash, 15.0, false, true, "Delete this page").clicked() {
                app.push_cmd(AppCmd::DeletePage);
            }
        });
    });
    ui.add_space(1.0);

    let n = app.pages.len();
    // Decode at most one missing thumbnail per frame (it's just the embedded
    // ORA thumbnail PNG — cheap, but not free at 60fps). Preview-texture
    // mints get their own budget: the decode + bilinear downscale is the
    // expensive half of the preview tier.
    let mut thumb_budget = 1;
    let mut prev_budget = 1;
    let mut drop: Option<(usize, usize)> = None;
    let binding_right = app.binding_right;

    // CSP EX spread layout: the cover alone, then facing pairs in binding
    // order — right-bound (JP) shows [3|2], [5|4]…: the earlier page of a
    // spread is the right-hand page. THE reader's pairing fn, not a copy —
    // the palette has no spread-offset toggle, so it always passes false.
    let groups = crate::app::reader::spread_groups(n, binding_right, false);

    let aspect = {
        let (w, h) = app.doc.size;
        (h.max(1) as f32 / w.max(1) as f32).clamp(0.5, 2.5)
    };

    egui::ScrollArea::vertical()
        .id_salt("mn.pages.scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let avail = ui.available_width();
            let pad = 5.0;
            let gap = 3.0;
            // Owner preview tier / CSP Fit to Navigator: cells follow the
            // slider, or the pane width when Fit is on; the chosen width is
            // remembered so the live thumb renders at the size actually
            // shown (app.pages_cell_px).
            //
            // ONE cell size for the whole palette, in every mode: Fit divides
            // by the FULL spread (2), never by the pages this row happens to
            // hold, so the lone cover is a normal-sized page sitting in its
            // half of the row instead of a stretched one. `spread_groups`
            // hands back a fixed `[Option; 2]` per row for the same reason.
            let max_tw = ((avail - 2.0 * pad - gap) * 0.5).max(34.0);
            let tw = if app.pages_fit {
                max_tw
            } else {
                app.pages_cell_w.clamp(34.0, max_tw)
            };
            if !app.pages_fit {
                app.pages_cell_w = tw;
            }
            app.pages_cell_px = tw;
            let th = tw * aspect;
            let label_h = 13.0;

            for group in &groups {
                let (box_rect, _) = ui.allocate_exact_size(
                    egui::vec2(avail, th + label_h + 2.0 * pad),
                    egui::Sense::hover(),
                );
                let p = ui.painter();
                // CSP-style spread container: a darker well with a border.
                p.rect_filled(box_rect, 3.0, theme::c().field.gamma_multiply(0.75));
                p.rect_stroke(
                    box_rect,
                    3.0,
                    egui::Stroke::new(1.0, theme::c().border),
                    egui::StrokeKind::Inside,
                );

                for (cell, page) in group.iter().enumerate() {
                    let x0 = box_rect.left() + pad + cell as f32 * (tw + gap);
                    let cell_rect = egui::Rect::from_min_size(
                        egui::pos2(x0, box_rect.top() + pad),
                        egui::vec2(tw, th),
                    );
                    let Some(i) = *page else {
                        continue; // empty half of the cover/last spread
                    };

                    if app.pages[i].thumb.is_none() && thumb_budget > 0 {
                        if let Some(b) = &app.pages[i].bytes {
                            thumb_budget -= 1;
                            if let Some(img) = mn_core::project::page_thumb(b) {
                                let ci = egui::ColorImage::from_rgba_unmultiplied(
                                    [img.width() as usize, img.height() as usize],
                                    img.as_raw(),
                                );
                                app.pages[i].thumb = Some(ui.ctx().load_texture(
                                    format!("mn.page.{i}"),
                                    ci,
                                    egui::TextureOptions::LINEAR,
                                ));
                            }
                        }
                    }

                    // Owner preview tier: once the cell outgrows the 256px
                    // ORA thumbnail (portrait pages go soft past ~240px of
                    // cell height), mint the display-size texture from the
                    // sharp 1600px preview — bilinear on the CPU, budgeted
                    // one page per frame. Until it lands the existing
                    // texture keeps SCALING (scale first, sharpen after);
                    // with a 1600px base the upgrade is rarely visible.
                    let want_sharp = th > 240.0 && app.pages[i].bytes.is_some();
                    if want_sharp
                        && prev_budget > 0
                        && (app.pages[i].prev_tex_rev != app.pages[i].rev
                            || app.pages[i].prev_tex.is_none()
                            || (th - app.pages[i].prev_tex_px).abs()
                                > app.pages[i].prev_tex_px * 0.25)
                        && let Some(gray) = app.preview_for(i)
                    {
                        prev_budget -= 1;
                        app.pages[i].prev_tex = Some(super::preview::mint_gray_tex(
                            ui.ctx(),
                            &gray,
                            tw.round().max(1.0) as u32,
                            th.round().max(1.0) as u32,
                            format!("mn.page.prev.{i}"),
                        ));
                        app.pages[i].prev_tex_px = th;
                        app.pages[i].prev_tex_rev = app.pages[i].rev;
                    }

                    let selected = i == app.page_index;
                    let id = egui::Id::new(("mn.page.cell", i));
                    let resp = ui.interact(cell_rect, id, egui::Sense::click_and_drag());
                    let p = ui.painter();
                    // The sharp preview texture wins once the cell is big
                    // enough to need it; otherwise the ordinary thumb
                    // (active page: the live display-size render).
                    let tex = if want_sharp {
                        app.pages[i]
                            .prev_tex
                            .as_ref()
                            .or(app.pages[i].thumb.as_ref())
                    } else {
                        app.pages[i].thumb.as_ref()
                    };
                    // PAPER FIRST, art over it. Owner report 2026-08-22:
                    // "page 1 renders much bigger than pages 2-3". The cells
                    // are the same size — what differed was the BACKING. The
                    // active page's thumb is a live `render_offscreen`, which
                    // composites over the canvas paper, so it fills the cell
                    // edge to edge; a stashed page's thumb is the ORA
                    // `mergedimage` taken over a TRANSPARENT background, and
                    // on a blank frame-folder page the only opaque pixels are
                    // the folder's white fill — clipped to the 180x270 mm
                    // inner frame. That 2:3 patch floating on the dark well
                    // read as a smaller page. Painting the paper here backs
                    // every cell the same way, whichever tier drew the art.
                    p.rect_filled(cell_rect, 2.0, egui::Color32::WHITE);
                    match tex {
                        Some(t) => {
                            p.image(
                                t.id(),
                                cell_rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                        }
                        None if selected => {
                            p.text(
                                cell_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "editing",
                                egui::FontId::proportional(9.0),
                                egui::Color32::from_gray(110),
                            );
                        }
                        None => {}
                    }
                    let stroke = if selected {
                        egui::Stroke::new(2.0, theme::c().accent)
                    } else if resp.hovered() {
                        egui::Stroke::new(1.0, theme::c().text_weak)
                    } else {
                        egui::Stroke::new(1.0, theme::c().border)
                    };
                    p.rect_stroke(cell_rect, 2.0, stroke, egui::StrokeKind::Outside);
                    p.text(
                        egui::pos2(cell_rect.center().x, cell_rect.bottom() + 2.0),
                        egui::Align2::CENTER_TOP,
                        format!("{}", i + 1),
                        egui::FontId::proportional(10.0),
                        if selected {
                            theme::c().text_strong
                        } else {
                            theme::c().text_weak
                        },
                    );

                    if resp.clicked() && !selected {
                        app.push_cmd(AppCmd::SelectPage(i));
                    }
                    // The template page wears a quiet corner mark so "why
                    // did my new page come out pre-panelled" has a visible
                    // answer.
                    if app.template_page == Some(i) {
                        ui.painter().text(
                            cell_rect.left_top() + egui::vec2(3.0, 2.0),
                            egui::Align2::LEFT_TOP,
                            "T",
                            egui::FontId::proportional(9.0),
                            theme::c().accent,
                        );
                    }
                    // Docking 2 phase 2: a page beside the canvas.
                    resp.context_menu(|ui| {
                        if ui.button("Open in a pane").clicked() {
                            app.push_cmd(AppCmd::OpenPageInPane(i));
                            ui.close();
                        }
                        // Template page (tekno B2): new pages clone this
                        // page's bytes instead of starting blank.
                        if app.template_page == Some(i) {
                            if ui.button("Stop using as template").clicked() {
                                app.template_page = None;
                                app.mark_dirty();
                                app.set_status("new pages start blank again");
                                ui.close();
                            }
                        } else if ui.button("Use as template for new pages").clicked() {
                            app.template_page = Some(i);
                            app.mark_dirty();
                            app.set_status(format!(
                                "page {} is the template — every new page starts as a copy of it",
                                i + 1
                            ));
                            ui.close();
                        }
                    });
                    if resp.drag_started() {
                        egui::DragAndDrop::set_payload(ui.ctx(), PageDrag(i));
                    }
                    if resp.dnd_hover_payload::<PageDrag>().is_some() {
                        // Reading order runs right→left in a right-bound book:
                        // the pointer on the reading-start side drops BEFORE
                        // this page.
                        let start_side = ui.ctx().pointer_interact_pos().is_some_and(|p| {
                            let right_half = p.x > cell_rect.center().x;
                            if binding_right {
                                right_half
                            } else {
                                !right_half
                            }
                        });
                        let slot = if start_side { i } else { i + 1 };
                        let x = if start_side == binding_right {
                            cell_rect.right() + 1.0
                        } else {
                            cell_rect.left() - 1.0
                        };
                        ui.painter().vline(
                            x,
                            cell_rect.y_range(),
                            egui::Stroke::new(2.0, theme::c().accent),
                        );
                        if let Some(from) = resp.dnd_release_payload::<PageDrag>() {
                            drop = Some((from.0, slot));
                        }
                    }
                }
                ui.add_space(3.0);
            }
        });
    // CSP Page Manager "Fit to Navigator" (owner preview tier): − / slider
    // / + for the cell size, and a Fit toggle that tracks the pane width
    // as it is dragged. Touching any size control leaves Fit mode.
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        if icon_btn(
            ui,
            Icon::Book,
            15.0,
            false,
            true,
            "Reader — read the chapter",
        )
        .clicked()
        {
            app.push_cmd(AppCmd::ReaderOpen);
        }
        let mut cw = app.pages_cell_w;
        if ui.small_button("−").clicked() {
            app.pages_fit = false;
            cw = (cw / 1.15).max(34.0);
        }
        let before = cw;
        ui.add_enabled(
            !app.pages_fit,
            egui::Slider::new(&mut cw, 34.0..=480.0).text("size"),
        );
        if cw != before {
            app.pages_fit = false;
        }
        if ui.small_button("+").clicked() {
            app.pages_fit = false;
            cw = (cw * 1.15).min(480.0);
        }
        app.pages_cell_w = cw;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new("Fit to pane").selected(app.pages_fit))
                .clicked()
            {
                app.pages_fit = !app.pages_fit;
            }
        });
    });

    if let Some((from, slot)) = drop {
        let to = if from < slot { slot - 1 } else { slot };
        if to != from {
            app.push_cmd(AppCmd::MovePage { from, to });
        }
    }
}
