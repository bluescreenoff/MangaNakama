//! Modal dialogs and always-on-top windows: New Manga, Work Settings, the
//! Sub Tool Detail wrench window, and the F1 Diagnostics HUD.

use super::property::{Section, brush_sliders, prop_sections};
use super::theme::{self, ValueBar};
use crate::app::{App, PromoteDraft};
use crate::cmd::AppCmd;
use mn_core::PageSetup;

/// PM-043: the Shift+Enter split point for a field — the last space
/// before the midpoint, else the nearest CHARACTER boundary to the byte
/// midpoint (Japanese has no ASCII spaces, and the raw byte midpoint
/// lands inside a 3-byte kana/kanji two times in three, which the split
/// silently refused — audit G, 2026-08-19). None = nothing splittable.
pub(super) fn story_split_point(buf: &str) -> Option<usize> {
    if buf.len() < 2 {
        return None;
    }
    // Walk the byte midpoint DOWN to a boundary first — slicing at a
    // non-boundary panics, and a 3-byte kana midpoint is inside a char
    // two times in three.
    let mut mid = buf.len() / 2;
    while mid > 0 && !buf.is_char_boundary(mid) {
        mid -= 1;
    }
    let at_space = buf[..mid]
        .as_bytes()
        .iter()
        .rposition(|&b| b == b' ')
        .map(|sp| sp + 1)
        .filter(|&at| at > 0 && at < buf.len());
    if at_space.is_some() {
        return at_space;
    }
    // No space before the midpoint: the boundary nearest the midpoint,
    // preferring the later one on ties.
    buf.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(buf.len()))
        .filter(|&i| i > 0 && i < buf.len())
        .min_by_key(|&i| {
            let d = (i as i64 - mid as i64).abs();
            (d, std::cmp::Reverse(i))
        })
}

#[cfg(test)]
mod split_tests {
    use super::story_split_point;

    #[test]
    fn split_point_english_last_space_before_mid() {
        // The last space BEFORE the midpoint — not the last space of the
        // whole string (the old code split near the end). The head keeps
        // the space; the tail starts on a word.
        let at = story_split_point("one two three four").unwrap();
        let (head, tail) = "one two three four".split_at(at);
        assert_eq!(head, "one two ");
        assert_eq!(tail, "three four");
    }

    #[test]
    fn split_point_japanese_nearest_char_boundary() {
        // 5 kana = 15 bytes; the byte midpoint 7 lands INSIDE え (bytes
        // 6..9). The nearest boundary is 6 (or 9 at distance 2) — 6 wins.
        let at = story_split_point("あいうえお").unwrap();
        assert_eq!(at % 3, 0, "a character boundary, not a byte midpoint");
        let (head, tail) = "あいうえお".split_at(at);
        assert_eq!(head, "あい");
        assert_eq!(tail, "うえお");
    }

    #[test]
    fn split_point_refuses_unsplittable() {
        assert_eq!(story_split_point(""), None);
        // A single char: no interior boundary exists.
        assert_eq!(story_split_point("あ"), None);
        assert_eq!(story_split_point("a"), None);
    }
}

/// Sub Tool Detail — the wrench window: full-width controls for the current
/// sub tool, plus a reset back to the preset's own values.
pub(super) fn detail_window(ctx: &egui::Context, app: &mut App) {
    if !app.detail_open {
        return;
    }
    let mut open = app.detail_open;
    egui::Window::new("Sub Tool Detail")
        .open(&mut open)
        .default_width(300.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.strong(app.brush_name().to_owned());
            if let Some(i) = app.selected_preset {
                ui.weak(app.presets[i].1.display().to_string());
            }
            ui.separator();
            // TL-013: everything here still edits normally while the sub
            // tool is locked — it is the REMEMBERING that is frozen, and
            // this window is the same sub tool by another door, so it says
            // so rather than letting the snap-back arrive unannounced.
            if app.props_current.locked {
                ui.weak("locked — these values come back when you return to this sub tool");
            }
            brush_sliders(ui, app);
            ui.add_space(2.0);
            // Entry taper — the CSP 入り: strokes ramp from thin over this
            // length. Seeded from the preset's own CSP metadata.
            let p = app.props_current;
            let (mut tpx, mut tmin) = (p.taper_px, p.taper_min * 100.0);
            let mut changed = ValueBar::new("In taper", 0.0, 400.0)
                .step(1.0)
                .suffix(" px")
                .show(ui, &mut tpx)
                .changed();
            changed |= ValueBar::new("Taper min", 0.0, 100.0)
                .suffix("%")
                .show(ui, &mut tmin)
                .changed();
            if changed {
                app.push_cmd(AppCmd::SetTaper {
                    px: tpx,
                    min: tmin / 100.0,
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Reset to preset").clicked() {
                    if let Some(i) = app.selected_preset {
                        let p = app.presets[i].1.clone();
                        app.forget_current_props();
                        app.push_cmd(AppCmd::SelectBrush(p));
                    }
                }
                if app.eraser_active() {
                    ui.weak("erasing (transparent slot or eraser tool)");
                }
            });
        });
    app.detail_open = open;
}

/// The Tool Property FULL list (CSP: Tool Property ▸ detail window): every
/// section of the current context with its eye toggle — unchecked sections
/// disappear from the compact palette but stay fully editable here (owner
/// request, pics 6-7).
pub(super) fn property_detail_window(ctx: &egui::Context, app: &mut App) {
    if !app.prop_detail_open
        || matches!(
            app.tool,
            crate::cmd::Tool::Pen
                | crate::cmd::Tool::Eraser
                | crate::cmd::Tool::SelPen
                | crate::cmd::Tool::SelEraser
        )
    {
        return;
    }
    let mut open = app.prop_detail_open;
    egui::Window::new("Tool Property — full list")
        .open(&mut open)
        .default_width(290.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.weak("uncheck a category to hide it from the palette");
            ui.add_space(2.0);
            egui::ScrollArea::vertical()
                .max_height(520.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for Section { id, title, body } in prop_sections(app) {
                        let mut vis = !app.prop_hidden.contains(id);
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut vis, "").changed() {
                                if vis {
                                    app.prop_hidden.remove(id);
                                } else {
                                    app.prop_hidden.insert(id.to_owned());
                                }
                            }
                            ui.label(
                                egui::RichText::new(title.to_owned())
                                    .size(11.5)
                                    .color(super::theme::c().text_strong),
                            );
                        });
                        body(ui, app);
                        ui.add_space(3.0);
                        ui.separator();
                    }
                });
        });
    app.prop_detail_open = open;
}

// --- new document dialog ------------------------------------------------

pub(super) fn new_doc_window(ctx: &egui::Context, app: &mut App) {
    if !app.new_doc_open {
        return;
    }
    let mut open = app.new_doc_open;
    let mut create = false;
    let mut cancel = false;
    egui::Window::new("New Manga")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            let d = &mut app.new_doc_draft;
            let mm = |ui: &mut egui::Ui, v: &mut f32| {
                ui.add(
                    egui::DragValue::new(v)
                        .range(1.0..=3000.0)
                        .suffix(" mm")
                        .speed(0.5),
                );
            };
            egui::Grid::new("mn.newdoc")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    ui.label("Story");
                    ui.text_edit_singleline(&mut d.story);
                    ui.end_row();

                    ui.label("Preset");
                    egui::ComboBox::from_id_salt("mn.newdoc.preset")
                        .width(240.0)
                        .selected_text(d.setup.name.clone())
                        .show_ui(ui, |ui| {
                            for p in PageSetup::presets() {
                                if ui
                                    .selectable_label(d.setup.name == p.name, &p.name)
                                    .clicked()
                                {
                                    d.setup = p;
                                }
                            }
                        });
                    ui.end_row();

                    ui.label("Paper");
                    ui.horizontal(|ui| {
                        mm(ui, &mut d.setup.paper_mm.0);
                        mm(ui, &mut d.setup.paper_mm.1);
                    });
                    ui.end_row();

                    ui.label("DPI");
                    ui.add(egui::DragValue::new(&mut d.setup.dpi).range(0..=1200));
                    ui.end_row();

                    if d.setup.dpi > 0 {
                        ui.label("Trim (finish)");
                        ui.horizontal(|ui| {
                            mm(ui, &mut d.setup.trim_mm.0);
                            mm(ui, &mut d.setup.trim_mm.1);
                        });
                        ui.end_row();

                        ui.label("Bleed");
                        mm(ui, &mut d.setup.bleed_mm);
                        ui.end_row();

                        ui.label("Inner border");
                        ui.horizontal(|ui| {
                            mm(ui, &mut d.setup.inner_mm.0);
                            mm(ui, &mut d.setup.inner_mm.1);
                        });
                        ui.end_row();

                        ui.label("Inner offset");
                        ui.horizontal(|ui| {
                            mm(ui, &mut d.setup.inner_offset_mm.0);
                            mm(ui, &mut d.setup.inner_offset_mm.1);
                        });
                        ui.end_row();
                    }

                    ui.label("Pages");
                    ui.add(egui::DragValue::new(&mut d.pages).range(1..=200));
                    ui.end_row();

                    ui.label("Binding");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut d.binding_right, true, "Right (JP)");
                        ui.radio_value(&mut d.binding_right, false, "Left");
                    });
                    ui.end_row();

                    if d.setup.dpi > 0 {
                        ui.label("Frame folder");
                        ui.checkbox(
                            &mut d.frame_folder,
                            "Start pages with a frame border folder",
                        )
                        .on_hover_text(
                            "CSP-style: mask folder with a White layer and a draw layer inside",
                        );
                        ui.end_row();
                    }
                });
            let (w, h) = d.setup.paper_px();
            ui.weak(format!("{w} × {h} px per page"));
            ui.add_space(2.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("  Create  ").clicked() {
                    create = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if create {
        app.push_cmd(AppCmd::NewComicCreate);
        app.new_doc_open = false;
    } else {
        app.new_doc_open = open && !cancel;
    }
}

/// Work Settings: edit story/binding/page geometry after creation. Geometry
/// changes affect guides + new pages only — existing pixels stay untouched
/// unless "Resize existing pages…" hands them to the canvas-size dialog,
/// which is the one door that moves a work's page size after creation.
pub(super) fn work_settings_window(ctx: &egui::Context, app: &mut App) {
    if !app.work_settings_open {
        return;
    }
    let mut open = app.work_settings_open;
    let mut apply = false;
    let mut resize = false;
    let mut cancel = false;
    egui::Window::new("Work Settings")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            let d = &mut app.work_settings_draft;
            let mm = |ui: &mut egui::Ui, v: &mut f32| {
                ui.add(
                    egui::DragValue::new(v)
                        .range(1.0..=3000.0)
                        .suffix(" mm")
                        .speed(0.5),
                );
            };
            egui::Grid::new("mn.worksettings")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    ui.label("Story");
                    ui.text_edit_singleline(&mut d.story);
                    ui.end_row();

                    // M2: the publisher/printer target. Picking one
                    // RESTATES the draft's paper/trim/binding from the
                    // profile — through this dialog's own Apply, so the
                    // geometry consequences stay in the one door. None
                    // keeps everything hand-set.
                    ui.label("Publisher profile");
                    egui::ComboBox::from_id_salt("mn.worksettings.profile")
                        .width(240.0)
                        .selected_text(
                            d.profile
                                .as_ref()
                                .map_or("None".to_owned(), |p| p.name.clone()),
                        )
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(d.profile.is_none(), "None").clicked() {
                                d.profile = None;
                            }
                            for p in mn_core::profile::PublisherProfile::builtins() {
                                let on = d.profile.as_ref().is_some_and(|q| q.name == p.name);
                                if ui.selectable_label(on, &p.name).clicked() && !on {
                                    d.setup = p.setup.clone();
                                    d.binding_right = p.binding_right;
                                    d.profile = Some(p);
                                }
                            }
                        })
                        .response
                        .on_hover_text(
                            "a submission target: sets the paper geometry now, and \
                             gives preflight its norms (page-count multiple, screen \
                             ruling) and Export All a one-press output setup",
                        );
                    ui.end_row();

                    ui.label("Preset");
                    egui::ComboBox::from_id_salt("mn.worksettings.preset")
                        .width(240.0)
                        .selected_text(d.setup.name.clone())
                        .show_ui(ui, |ui| {
                            for p in PageSetup::presets() {
                                if ui
                                    .selectable_label(d.setup.name == p.name, &p.name)
                                    .clicked()
                                {
                                    d.setup = p;
                                }
                            }
                        });
                    ui.end_row();

                    ui.label("Paper");
                    ui.horizontal(|ui| {
                        mm(ui, &mut d.setup.paper_mm.0);
                        mm(ui, &mut d.setup.paper_mm.1);
                    });
                    ui.end_row();

                    ui.label("DPI");
                    ui.add(egui::DragValue::new(&mut d.setup.dpi).range(0..=1200));
                    ui.end_row();

                    if d.setup.dpi > 0 {
                        ui.label("Trim (finish)");
                        ui.horizontal(|ui| {
                            mm(ui, &mut d.setup.trim_mm.0);
                            mm(ui, &mut d.setup.trim_mm.1);
                        });
                        ui.end_row();

                        ui.label("Bleed");
                        mm(ui, &mut d.setup.bleed_mm);
                        ui.end_row();

                        ui.label("Inner border");
                        ui.horizontal(|ui| {
                            mm(ui, &mut d.setup.inner_mm.0);
                            mm(ui, &mut d.setup.inner_mm.1);
                        });
                        ui.end_row();

                        ui.label("Inner offset");
                        ui.horizontal(|ui| {
                            mm(ui, &mut d.setup.inner_offset_mm.0);
                            mm(ui, &mut d.setup.inner_offset_mm.1);
                        });
                        ui.end_row();
                    }

                    ui.label("Binding");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut d.binding_right, true, "Right (JP)");
                        ui.radio_value(&mut d.binding_right, false, "Left");
                    });
                    ui.end_row();

                    ui.label("Margin info");
                    ui.checkbox(
                        &mut d.print_margin_info,
                        "Print story + page number in margins",
                    )
                    .on_hover_text(
                        "Draws the story title and page number outside the trim on export",
                    );
                    ui.end_row();

                    // Print metadata (preflight inputs, TRIAGE 132).
                    ui.label("Expression");
                    ui.horizontal(|ui| {
                        ui.radio_value(
                            &mut d.expression,
                            mn_core::Expression::Mono,
                            "Mono (B&W)",
                        );
                        ui.radio_value(
                            &mut d.expression,
                            mn_core::Expression::Colour,
                            "Colour",
                        )
                        .on_hover_text(
                            "Mono flags colour pixels in the preflight — a B&W print cannot reproduce them",
                        );
                    });
                    ui.end_row();

                    ui.label("Spine");
                    ui.add(
                        egui::DragValue::new(&mut d.spine_mm)
                            .range(0.0..=60.0)
                            .suffix(" mm")
                            .speed(0.5),
                    )
                    .on_hover_text("Perfect-binding spine width — 0 = unset (preflight warns)");
                    ui.end_row();

                    ui.label("Cover page");
                    ui.horizontal(|ui| {
                        let mut has = d.cover.is_some();
                        let cb = ui.checkbox(&mut has, "Designate");
                        if cb.changed() {
                            d.cover = has.then_some(0);
                        }
                        if let Some(c) = &mut d.cover {
                            let pages = app.pages.len().max(1) as i64;
                            let mut v = (*c as i64).clamp(0, pages - 1);
                            ui.add(
                                egui::DragValue::new(&mut v)
                                    .range(1..=pages)
                                    .prefix("page ")
                                    .speed(0.1),
                            )
                            .on_hover_text(
                                "The cover page of the work (reading order) — preflight flags a multi-page work with none",
                            );
                            *c = v.clamp(0, pages - 1) as usize;
                        }
                    });
                    ui.end_row();
                });
            let (w, h) = d.setup.paper_px();
            ui.weak(format!("{w} × {h} px per page"));
            ui.weak(
                "Geometry changes affect guides and NEW pages; existing pages keep their pixels.",
            );
            if ui
                .button("Resize existing pages…")
                .on_hover_text(
                    "Apply these settings, then change the pixel size of the pages themselves — content is moved, never resampled",
                )
                .clicked()
            {
                resize = true;
            }
            ui.add_space(2.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("  Apply  ").clicked() {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if resize {
        // Apply FIRST: the size dialog seeds from the work's paper, so the
        // draft's geometry has to be the work's geometry by then (the queue
        // runs in order).
        app.push_cmd(AppCmd::WorkSettingsApply);
        app.push_cmd(AppCmd::OpenPageSize);
        app.work_settings_open = false;
    } else if apply {
        app.push_cmd(AppCmd::WorkSettingsApply);
        app.work_settings_open = false;
    } else {
        app.work_settings_open = open && !cancel;
    }
}

/// Batch Import (workflow audit #4): the files the picker returned, in
/// name order, mapped one-to-one onto consecutive pages from a chosen
/// start. The list is the preview — a row per file naming the page it will
/// land on — because the one thing that goes wrong here is an off-by-one
/// start page across twenty roughs.
pub(super) fn batch_import_window(ctx: &egui::Context, app: &mut App) {
    if !app.batch_import_open {
        return;
    }
    let mut open = app.batch_import_open;
    let (mut apply, mut cancel) = (false, false);
    let pages = app.pages.len();
    egui::Window::new("Batch Import Pages")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            let n = app.batch_import.files.len();
            ui.horizontal(|ui| {
                ui.label("Start at page");
                ui.add(
                    egui::DragValue::new(&mut app.batch_import.start)
                        .range(1..=pages.saturating_add(1))
                        .speed(1.0),
                );
                ui.weak(format!("of {pages}"));
            });
            ui.add_space(4.0);
            ui.label(format!("{n} file(s), in name order:"));
            let start = app.batch_import.start;
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .show(ui, |ui| {
                    for (i, p) in app.batch_import.files.iter().enumerate() {
                        let slot = start + i;
                        ui.weak(format!(
                            "page {slot}{} — {}",
                            if slot > pages { " (new)" } else { "" },
                            p.file_name().unwrap_or_default().to_string_lossy()
                        ));
                    }
                });
            ui.add_space(4.0);
            let added = (start + n).saturating_sub(1).saturating_sub(pages);
            ui.weak(format!(
                "Each image is scaled to fit and becomes that page's draft \
                 underlay — on screen, never in the export. {}",
                if added > 0 {
                    format!("{added} page(s) will be added at the end.")
                } else {
                    "No pages need to be added.".to_owned()
                }
            ));
            // Same wording contract as the batch-ops and canvas-size
            // dialogs: say what undo will not cover rather than implying
            // an undo that is not there.
            ui.weak(
                "Pages other than the open one are written directly — undo covers \
                 only the open page.",
            );
            ui.add_space(2.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("  Import  ").clicked() {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if apply {
        app.push_cmd(AppCmd::BatchImportApply);
        app.batch_import_open = false;
    } else {
        app.batch_import_open = open && !cancel;
    }
}

/// New work from this work (workflow audit §11): one number, the target
/// dpi, and a live readout of what it produces. The readout is the whole
/// dialog's job — "150 dpi" means nothing until it says the page will be
/// 1276×1795 px instead of 5102×7181, which is the reason to draw the
/// ネーム in a second work at all.
pub(super) fn promote_window(ctx: &egui::Context, app: &mut App) {
    if !app.promote_open {
        return;
    }
    let mut open = app.promote_open;
    let (mut apply, mut cancel) = (false, false);
    let pages = app.pages.len();
    let own_dpi = app.work_dpi();
    let setup = app.page.clone();
    egui::Window::new("New Work From This Work")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            let Some(mut setup) = setup else {
                ui.label(
                    "This work has no page setup to copy. Give it one in \
                     Page ▸ Work settings… first.",
                );
                if ui.button("Close").clicked() {
                    cancel = true;
                }
                return;
            };
            ui.horizontal(|ui| {
                ui.label("Resolution");
                ui.add(
                    egui::DragValue::new(&mut app.promote.dpi)
                        .range(PromoteDraft::MIN_DPI..=PromoteDraft::MAX_DPI)
                        .speed(1.0)
                        .suffix(" dpi"),
                );
                if ui
                    .button(format!("{} dpi (name)", PromoteDraft::NAME_DPI))
                    .clicked()
                {
                    app.promote.dpi = PromoteDraft::NAME_DPI;
                }
                if let Some(d) = own_dpi
                    && ui.button(format!("{d} dpi (same)")).clicked()
                {
                    app.promote.dpi = d;
                }
            });
            setup.dpi = app
                .promote
                .dpi
                .clamp(PromoteDraft::MIN_DPI, PromoteDraft::MAX_DPI);
            let (w, h) = setup.paper_px();
            ui.add_space(4.0);
            ui.weak(format!(
                "{pages} blank page(s), {w} × {h} px — same paper, same binding, \
                 same page order."
            ));
            ui.weak(
                "The pages keep this work's page identities, which is what lets \
                 Page ▸ Stamp name pages as drafts… put each of them back on the \
                 page it was drawn for. Save both works for that to survive a restart.",
            );
            ui.weak("It opens in a new tab — this work stays exactly as it is.");
            ui.add_space(2.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("  Create  ").clicked() {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if apply {
        app.push_cmd(AppCmd::PromoteNewWorkApply);
        app.promote_open = false;
    } else {
        app.promote_open = open && !cancel;
    }
}

/// Change Canvas Size: new pixel size + the CSP 3×3 anchor the existing
/// content pins to (基準位置). Structural — clears the undo history.
pub(super) fn canvas_size_window(ctx: &egui::Context, app: &mut App) {
    if !app.canvas_size_open {
        return;
    }
    let mut open = app.canvas_size_open;
    let mut apply = false;
    let mut cancel = false;
    egui::Window::new("Change Canvas Size")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            let d = &mut app.canvas_size_draft;
            let (cw, ch) = app.doc.size;
            egui::Grid::new("mn.canvassize")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    ui.label("Current");
                    ui.weak(format!("{cw} × {ch} px"));
                    ui.end_row();

                    ui.label("New size");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut d.w)
                                .range(1..=65535)
                                .suffix(" px")
                                .speed(1.0),
                        );
                        ui.add(
                            egui::DragValue::new(&mut d.h)
                                .range(1..=65535)
                                .suffix(" px")
                                .speed(1.0),
                        );
                    });
                    ui.end_row();

                    ui.label("Anchor");
                    // CSP's 3×3 基準位置 grid: which corner the content pins to.
                    ui.vertical(|ui| {
                        use mn_core::ResizeAnchor::*;
                        for row in [
                            [TopLeft, Top, TopRight],
                            [Left, Center, Right],
                            [BottomLeft, Bottom, BottomRight],
                        ] {
                            ui.horizontal(|ui| {
                                for a in row {
                                    let sel = d.anchor == a;
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new(if sel { "●" } else { "·" })
                                                    .size(12.0),
                                            )
                                            .min_size(egui::vec2(22.0, 18.0)),
                                        )
                                        .clicked()
                                    {
                                        d.anchor = a;
                                    }
                                }
                            });
                        }
                    });
                    ui.end_row();
                });
            if let Some(p) = app.page.as_ref().filter(|p| p.dpi > 0) {
                let (mw, mh) = (
                    d.w as f32 / p.dpi as f32 * 25.4,
                    d.h as f32 / p.dpi as f32 * 25.4,
                );
                ui.weak(format!("{mw:.1} × {mh:.1} mm"));
            }
            ui.weak("Content is not resampled; the undo history is cleared.");
            // The work-wide half: every other page is written DIRECTLY, so
            // the box has to say what undo will not cover (same wording
            // contract as the batch dialog).
            let n = app.pages.len();
            let d = &mut app.canvas_size_draft;
            ui.checkbox(&mut d.all_pages, "Apply to every page of the work")
                .on_hover_text(
                    "Also moves the work's default size, so pages added later match",
                );
            if d.all_pages {
                ui.weak(format!(
                    "{} other page(s) are resized and saved directly — that cannot be undone. Spreads take double the width.",
                    n.saturating_sub(1)
                ));
            }
            ui.add_space(2.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("  Apply  ").clicked() {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if apply {
        app.push_cmd(AppCmd::ResizeCanvasApply);
        app.canvas_size_open = false;
    } else {
        app.canvas_size_open = open && !cancel;
    }
}

/// Row 89 (BR-014–016): the global pen-pressure wizard. It listens to
/// the RAW pressures of strokes drawn while it is open (`push_batch`
/// copies them), graphs them grey, overlays the Stronger/Weaker
/// correction in accent, and Apply writes the curve to prefs — it then
/// bends EVERY tool's input, before any per-tool curve.
pub(super) fn pen_wizard_window(ctx: &egui::Context, app: &mut App) {
    if !app.pen_wizard_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Pen pressure")
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Draw a few strokes on the canvas — the grey line is the pen's raw pressure.");
            ui.add_space(4.0);
            let h = 96.0;
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
            let crv = mn_core::stroke::gamma_pressure_curve(app.pen_wizard_gamma);
            let yat = |p: f32| rect.bottom() - 4.0 - p.clamp(0.0, 1.0) * (rect.height() - 8.0);
            let paint = ui.painter_at(rect);
            for f in [0.0f32, 0.5, 1.0] {
                let y = yat(f);
                paint.line_segment(
                    [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                    egui::Stroke::new(0.5, theme::c().border),
                );
            }
            let n = app.pen_wizard_samples.len();
            if n >= 2 {
                let xat = |i: usize| rect.left() + i as f32 / (n - 1) as f32 * rect.width();
                let raw: Vec<egui::Pos2> = app
                    .pen_wizard_samples
                    .iter()
                    .enumerate()
                    .map(|(i, p)| egui::pos2(xat(i), yat(*p)))
                    .collect();
                paint.add(egui::Shape::line(
                    raw,
                    egui::Stroke::new(1.0, theme::c().text_weak),
                ));
                let corr: Vec<egui::Pos2> = app
                    .pen_wizard_samples
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        egui::pos2(xat(i), yat(mn_core::stroke::eval_pressure_curve(&crv, *p)))
                    })
                    .collect();
                paint.add(egui::Shape::line(
                    corr,
                    egui::Stroke::new(2.0, theme::c().accent),
                ));
            } else {
                paint.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "draw with the pen…",
                    egui::FontId::proportional(12.0),
                    theme::c().text_weak,
                );
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Stronger").clicked() {
                    app.pen_wizard_gamma = (app.pen_wizard_gamma / 1.25).max(0.25);
                }
                if ui.button("Weaker").clicked() {
                    app.pen_wizard_gamma = (app.pen_wizard_gamma * 1.25).min(4.0);
                }
                if ui.button("Reset").clicked() {
                    app.pen_wizard_gamma = 1.0;
                }
                ui.weak(format!("×{:.2}", app.pen_wizard_gamma));
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Apply to every tool").clicked() {
                    let pts = if (app.pen_wizard_gamma - 1.0).abs() < 1e-3 {
                        Vec::new()
                    } else {
                        crv.clone()
                    };
                    app.push_cmd(AppCmd::PenPressureCurveSet(pts));
                }
                if ui.button("Cancel").clicked() {
                    app.pen_wizard_open = false;
                }
            });
        });
    app.pen_wizard_open &= open;
}

// --- tonal correction ---------------------------------------------------

/// A CSP-style −100..100 slider over a parameter stored as −1..1.
fn pct_row(ui: &mut egui::Ui, label: &str, v: &mut f32) {
    ui.label(label);
    let mut shown = *v * 100.0;
    if ui
        .add(egui::Slider::new(&mut shown, -100.0..=100.0).fixed_decimals(0))
        .changed()
    {
        *v = shown / 100.0;
    }
    ui.end_row();
}

/// TC-002: a level knob, shown on CSP's 0..255 scale over a 0..1 value.
fn level_row(ui: &mut egui::Ui, label: &str, v: &mut f32) {
    ui.label(label);
    let mut shown = *v * 255.0;
    if ui
        .add(egui::Slider::new(&mut shown, 0.0..=255.0).fixed_decimals(0))
        .changed()
    {
        *v = shown / 255.0;
    }
    ui.end_row();
}

/// TC-003: the tone-curve point editor.
///
/// Drag a handle; click empty space to add one; drag a handle out of the box
/// to delete it. The two END handles are pinned to x = 0 and x = 1 — the
/// evaluator clamps outside the control range, so an unpinned end would turn
/// a whole tail of the histogram flat without ever showing why.
///
/// The drag index lives in egui memory rather than on `App`: it is dead state
/// the moment this window closes, and nothing outside the widget reads it.
fn tone_curve_editor(ui: &mut egui::Ui, pts: &mut [[f32; 2]; mn_core::TONE_CURVE_MAX], n: &mut u8) {
    use super::theme;
    use egui::{Pos2, pos2};

    const PICK_RADIUS: f32 = 10.0;
    const SIDE: f32 = 224.0;
    // Keeps neighbours from stacking on one x (the evaluator would divide by
    // a zero-width span) and leaves a handle grabbable.
    const MIN_GAP: f32 = 0.02;

    let id = ui.make_persistent_id("mn.tone_curve.drag");
    let mut drag: Option<usize> = ui.data(|d| d.get_temp(id));

    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(SIDE, SIDE), egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let to_px = |p: [f32; 2]| -> Pos2 {
        pos2(
            rect.left() + p[0].clamp(0.0, 1.0) * rect.width(),
            rect.bottom() - p[1].clamp(0.0, 1.0) * rect.height(),
        )
    };
    let from_px = |pos: Pos2| -> [f32; 2] {
        [
            ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
            ((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0),
        ]
    };

    painter.rect_filled(rect, 2.0, theme::c().field);
    let grid = egui::Stroke::new(1.0, theme::c().outline);
    for k in 1..4 {
        let f = k as f32 / 4.0;
        let x = rect.left() + f * rect.width();
        let y = rect.top() + f * rect.height();
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], grid);
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], grid);
    }
    // The untouched diagonal, so "how far from nothing" is readable.
    painter.line_segment([to_px([0.0, 0.0]), to_px([1.0, 1.0])], grid);

    // The curve itself is drawn by SAMPLING `Adjust::map`, never by a second
    // copy of the interpolation — a preview that draws one curve and applies
    // another is the exact lie this codebase keeps `correct_tile` shared to
    // avoid.
    let shown = mn_core::Adjust::ToneCurve { pts: *pts, n: *n };
    let steps = SIDE as usize / 2;
    let line: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let x = i as f32 / steps as f32;
            to_px([x, shown.map([x; 3])[0]])
        })
        .collect();
    painter.add(egui::Shape::line(
        line,
        egui::Stroke::new(2.0, theme::c().accent),
    ));

    let count = (*n as usize).min(mn_core::TONE_CURVE_MAX);
    for (i, p) in pts[..count].iter().enumerate() {
        let pos = to_px(*p);
        let hot = drag == Some(i);
        painter.circle_filled(pos, if hot { 5.5 } else { 4.0 }, theme::c().accent);
        if hot {
            painter.circle_stroke(pos, 5.5, egui::Stroke::new(1.5, theme::c().text_strong));
        }
    }

    if let Some(pos) = resp.interact_pointer_pos() {
        let nearest = pts[..count]
            .iter()
            .enumerate()
            .map(|(i, p)| (i, to_px(*p).distance(pos)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if resp.drag_started() {
            drag = match nearest {
                Some((i, d)) if d <= PICK_RADIUS => Some(i),
                _ if count < mn_core::TONE_CURVE_MAX => {
                    // Insert sorted, never before the first end or after the
                    // last one.
                    let p = from_px(pos);
                    let at = pts[..count]
                        .partition_point(|q| q[0] < p[0])
                        .clamp(1, count.saturating_sub(1).max(1));
                    pts[at..=count].rotate_right(1);
                    pts[at] = p;
                    *n = count as u8 + 1;
                    Some(at)
                }
                _ => None,
            };
        } else if resp.dragged() {
            if let Some(i) = drag.filter(|&i| i < count) {
                let mut p = from_px(pos);
                // The ends own their x; the interior stays strictly between
                // its neighbours so the point order never inverts.
                if i == 0 {
                    p[0] = 0.0;
                } else if i == count - 1 {
                    p[0] = 1.0;
                } else {
                    let lo = pts[i - 1][0] + MIN_GAP;
                    let hi = pts[i + 1][0] - MIN_GAP;
                    p[0] = p[0].clamp(lo.min(hi), hi.max(lo));
                }
                pts[i] = p;
            }
        }
    }
    if resp.drag_stopped() {
        // Dragged out of the box = delete, and only an interior point can go
        // (dropping an end would unpin it).
        if let Some(i) = drag.filter(|&i| i > 0 && i + 1 < count) {
            let out = ui
                .ctx()
                .pointer_latest_pos()
                .is_none_or(|p| !rect.expand(6.0).contains(p));
            if out {
                pts[i..count].rotate_left(1);
                pts[count - 1] = [0.0, 0.0];
                *n = count as u8 - 1;
            }
        }
        drag = None;
    }
    ui.data_mut(|d| match drag {
        Some(i) => {
            d.insert_temp(id, i);
        }
        None => d.remove::<usize>(id),
    });
    resp.on_hover_text(
        "Drag a point; click empty space to add one; drag a point out of the \
box to delete it. The two ends are pinned to the left and right edges. The \
curve is monotone — it never overshoots between your points.",
    );
}

/// TC-004/005/006/011: the tonal-correction dialog. One window for all four
/// parameterised corrections — the open draft's variant picks the sliders,
/// so a new correction is a match arm and not a new dialog.
///
/// The preview is live on the real canvas: every frame this runs, the
/// document's pixels are brought in line with the sliders. See
/// `app/adjust.rs` for the rule that makes that safe.
pub(super) fn adjust_window(ctx: &egui::Context, app: &mut App) {
    let Some(mut adj) = app.adjust_draft else {
        return;
    };
    let mut open = true;
    let mut apply = false;
    let mut cancel = false;
    let live_mode = app.adjust_live.is_some();
    let mut live = app.adjust_preview.as_ref().is_some_and(|p| p.live)
        || app.adjust_live.as_ref().is_some_and(|l| l.live);
    egui::Window::new(adj.label())
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            ui.set_min_width(300.0);
            egui::Grid::new("mn.adjust")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| match &mut adj {
                    mn_core::Adjust::BrightnessContrast {
                        brightness,
                        contrast,
                    } => {
                        pct_row(ui, "Brightness", brightness);
                        pct_row(ui, "Contrast", contrast);
                    }
                    mn_core::Adjust::HueSaturation {
                        hue,
                        saturation,
                        luminosity,
                    } => {
                        ui.label("Hue");
                        ui.add(egui::Slider::new(hue, -180.0..=180.0).fixed_decimals(0));
                        ui.end_row();
                        pct_row(ui, "Saturation", saturation);
                        pct_row(ui, "Luminosity", luminosity);
                    }
                    mn_core::Adjust::Posterize { levels } => {
                        ui.label("Levels");
                        ui.add(egui::Slider::new(levels, 2..=20));
                        ui.end_row();
                    }
                    mn_core::Adjust::ColourBalance {
                        cyan_red,
                        magenta_green,
                        yellow_blue,
                    } => {
                        pct_row(ui, "Cyan ↔ Red", cyan_red);
                        pct_row(ui, "Magenta ↔ Green", magenta_green);
                        pct_row(ui, "Yellow ↔ Blue", yellow_blue);
                    }
                    mn_core::Adjust::GradientMap { stops, n } => {
                        // The ramp, inline: one row per live stop (colour
                        // + position), then add/remove. The Gradient
                        // tool's own ramp is NOT borrowed — a map wants
                        // its own palette, as CSP's dialog does.
                        ui.label("Ramp");
                        let mut preview = Vec::new();
                        for st in stops.iter().take(*n as usize) {
                            preview.push(egui::Color32::from_rgb(
                                (st[1].clamp(0.0, 1.0) * 255.0) as u8,
                                (st[2].clamp(0.0, 1.0) * 255.0) as u8,
                                (st[3].clamp(0.0, 1.0) * 255.0) as u8,
                            ));
                        }
                        let (_r, resp) = ui.allocate_exact_size(
                            egui::vec2(220.0, 14.0),
                            egui::Sense::hover(),
                        );
                        let p = ui.painter_at(resp.rect);
                        let w = resp.rect.width() / preview.len().max(1) as f32;
                        for (i, c) in preview.iter().enumerate() {
                            p.rect_filled(
                                egui::Rect::from_min_max(
                                    egui::pos2(resp.rect.left() + w * i as f32, resp.rect.top()),
                                    egui::pos2(
                                        resp.rect.left() + w * (i + 1) as f32,
                                        resp.rect.bottom(),
                                    ),
                                ),
                                0.0,
                                *c,
                            );
                        }
                        ui.end_row();
                        for i in 0..(*n as usize).min(mn_core::adjust::GRADIENT_MAP_MAX) {
                            ui.label(format!("Stop {}", i + 1));
                            ui.horizontal(|ui| {
                                let mut c = [
                                    (stops[i][1].clamp(0.0, 1.0) * 255.0) as u8,
                                    (stops[i][2].clamp(0.0, 1.0) * 255.0) as u8,
                                    (stops[i][3].clamp(0.0, 1.0) * 255.0) as u8,
                                ];
                                if ui.color_edit_button_srgb(&mut c).changed()
                                {
                                    stops[i][1] = c[0] as f32 / 255.0;
                                    stops[i][2] = c[1] as f32 / 255.0;
                                    stops[i][3] = c[2] as f32 / 255.0;
                                }
                                ui.add(
                                    egui::Slider::new(&mut stops[i][0], 0.0..=1.0)
                                        .fixed_decimals(2)
                                        .text("pos"),
                                );
                            });
                            ui.end_row();
                        }
                        ui.label("");
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(*n < mn_core::adjust::GRADIENT_MAP_MAX as u8, egui::Button::new("+ stop"))
                                .clicked()
                            {
                                let i = *n as usize;
                                stops[i] = [0.5, 0.5, 0.5, 0.5, 0.0];
                                *n += 1;
                            }
                            if ui
                                .add_enabled(*n > 2, egui::Button::new("− stop"))
                                .clicked()
                            {
                                *n -= 1;
                            }
                        });
                        ui.end_row();
                    }
                    mn_core::Adjust::Binarize { threshold } => {
                        ui.label("Threshold");
                        ui.add(egui::Slider::new(threshold, 0.0..=1.0).fixed_decimals(2));
                        ui.end_row();
                    }
                    mn_core::Adjust::Levels {
                        in_black,
                        in_white,
                        gamma,
                        out_black,
                        out_white,
                    } => {
                        ui.strong("Input");
                        ui.end_row();
                        level_row(ui, "Black point", in_black);
                        level_row(ui, "White point", in_white);
                        ui.label("Gamma");
                        ui.add(
                            egui::Slider::new(gamma, 0.1..=10.0)
                                .logarithmic(true)
                                .fixed_decimals(2),
                        );
                        ui.end_row();
                        ui.strong("Output");
                        ui.end_row();
                        level_row(ui, "Black point", out_black);
                        level_row(ui, "White point", out_white);
                    }
                    mn_core::Adjust::ToneCurve { pts, n } => {
                        ui.label("Curve");
                        tone_curve_editor(ui, pts, n);
                        ui.end_row();
                        ui.label("");
                        if ui.button("Reset curve").clicked() {
                            *pts = mn_core::Adjust::TONE_CURVE_REST;
                            *n = 2;
                        }
                        ui.end_row();
                    }
                    // Reverse gradient has no parameters and never opens
                    // this window (the menu applies it straight away).
                    mn_core::Adjust::Invert => {}
                });
            if matches!(adj, mn_core::Adjust::Binarize { .. }) {
                ui.weak("Transparent pixels stay transparent; alpha is not touched.");
            }
            // TC-013: the target set was fixed when the dialog opened.
            if live_mode {
                ui.weak("Edits this correction layer's parameters — nothing below is baked.");
            } else {
                let n = app
                    .adjust_preview
                    .as_ref()
                    .map_or(1, |p| p.targets.len().max(1));
                if n > 1 {
                    ui.weak(format!(
                        "Applies to the {n} selected layers, inside the selection if there is one."
                    ));
                } else {
                    ui.weak(
                        "Applies to the ACTIVE layer only, inside the selection if there is one.",
                    );
                }
            }
            ui.add_space(2.0);
            ui.checkbox(&mut live, "Preview").on_hover_text(
                "Off shows the layer untouched — the 'before' half, without closing this.",
            );
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("  Apply  ").clicked() {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    app.adjust_draft = Some(adj);
    if let Some(p) = app.adjust_preview.as_mut() {
        p.live = live;
    }
    if let Some(l) = app.adjust_live.as_mut() {
        l.live = live;
    }
    if apply {
        app.push_cmd(AppCmd::AdjustApply);
    } else if cancel || !open {
        app.push_cmd(AppCmd::AdjustCancel);
    } else {
        // The canvas renders before this pass, so a slider drag shows one
        // frame later at worst — `mark_dirty` inside guarantees that frame.
        app.adjust_preview_sync();
    }
}

// --- diagnostics HUD ----------------------------------------------------

pub(super) fn hud(ctx: &egui::Context, app: &mut App) {
    let mut open = app.hud_open;
    egui::Window::new("Diagnostics")
        .open(&mut open)
        .default_pos(egui::pos2(300.0, 48.0))
        .default_width(330.0)
        .resizable(false)
        .show(ctx, |ui| {
            // The adapter line is long; wrap it instead of stretching the
            // window across the canvas.
            ui.set_max_width(330.0);
            ui.weak(app.renderer.adapter_line());
            ui.weak(format!(
                "MangaNakama {} ({})",
                env!("CARGO_PKG_VERSION"),
                env!("MN_BUILD_SHA")
            ));
            ui.separator();
            let present = app
                .renderer
                .present_mode()
                .map(|m| format!("{m:?}"))
                .unwrap_or_else(|| "-".into());
            let (sw, sh) = app.renderer.surface_size();
            let d = &app.diag;

            egui::Grid::new("mn.hud.grid")
                .num_columns(2)
                .spacing([12.0, 3.0])
                .show(ui, |ui| {
                    row(
                        ui,
                        "present",
                        &format!("{present} | {sw}x{sh} @ {:.2}x", app.shell.ppp),
                    );
                    row(
                        ui,
                        "frame",
                        &format!("{:.1} ms | {} painted", d.frame_ms, d.frames),
                    );
                    row(ui, "input", &format!("{:.0} events/s", d.events_per_sec));
                    // §4.12: pen-down to presented frame. `—` is honest and
                    // deliberate — the mouse fallback stamps a different
                    // clock, so there is nothing to subtract.
                    row(
                        ui,
                        "latency",
                        &match d.latency_ms {
                            Some(ms) => format!("{ms:.0} ms | max {:.0} ms", d.latency_max_ms),
                            None => "— (pen only)".to_owned(),
                        },
                    );
                    row(ui, "pointer", d.pointer);
                    // §4.1: `pressure 0.500` printed identically whether the
                    // pen was working perfectly at half pressure or not
                    // working at all. A diagnostic that cannot distinguish
                    // is not one.
                    row(
                        ui,
                        "pressure",
                        &format!(
                            "{:.3}{}",
                            d.last_pressure,
                            if app.pen.seen && !app.pen.pressure_reported {
                                "  (SUBSTITUTED — device reports no pressure)"
                            } else {
                                ""
                            }
                        ),
                    );
                    row(
                        ui,
                        "pen device",
                        &if app.pen.seen {
                            format!(
                                "pressure {} | tilt {} | {} report(s) dropped (not in contact){}",
                                if app.pen.pressure_reported {
                                    "yes"
                                } else {
                                    "NO"
                                },
                                if app.pen.tilt_reported { "yes" } else { "no" },
                                app.pen.dropped,
                                if app.pen.inverted { " | TAIL END" } else { "" },
                            )
                        } else {
                            "no pen seen this session".to_owned()
                        },
                    );
                    row(ui, "dab", &app.dab_path_last);
                    row(
                        ui,
                        "batches",
                        &format!(
                            "last {} | avg {:.1} | max {}",
                            d.last_batch, d.avg_batch, d.max_batch
                        ),
                    );
                    row(
                        ui,
                        "brush",
                        &format!(
                            "{} | {:.1} px radius | {:.1} px set{}",
                            app.brush_name(),
                            app.brush_radius(),
                            app.props_current.size_px,
                            if app.eraser_active() { " | eraser" } else { "" }
                        ),
                    );
                    row(
                        ui,
                        "props",
                        &format!(
                            "min {:.0}% | opacity {:.0}%",
                            app.props_current.min_size,
                            app.props_current.opacity * 100.0
                        ),
                    );
                    row(
                        ui,
                        "stabilizer",
                        &if app.props_current.stabilizer > 0.0 {
                            format!(
                                "{:.2} ({:.0} px string)",
                                app.props_current.stabilizer,
                                app.props_current.stabilizer * mn_core::stabilize::MAX_STRING_PX
                            )
                        } else {
                            "off".to_owned()
                        },
                    );
                    row(
                        ui,
                        "doc",
                        &format!(
                            "{}x{} | {} layer(s) | rev {}",
                            app.doc.size.0,
                            app.doc.size.1,
                            app.doc.layers.len(),
                            app.doc.revision
                        ),
                    );
                    row(
                        ui,
                        "view",
                        &format!(
                            "zoom {:.2} | rot {:.0}° | pan {:.0},{:.0}",
                            app.viewport.zoom,
                            app.viewport.rotate_rad.to_degrees(),
                            app.viewport.pan[0],
                            app.viewport.pan[1]
                        ),
                    );
                });

            // "Attach manganakama.log" is only actionable if the tester can
            // find it — and it is not always beside the exe (a read-only
            // install folder sends it to %LOCALAPPDATA%). Show the real
            // path, with a copy button so it can be pasted into an issue.
            ui.separator();
            match crate::testlog::path() {
                Some(p) => {
                    ui.horizontal(|ui| {
                        ui.weak("log");
                        if ui.small_button("copy path").clicked() {
                            ui.ctx().copy_text(p.display().to_string());
                        }
                    });
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(p.display().to_string())
                                .size(10.0)
                                .weak(),
                        )
                        .selectable(true)
                        .wrap(),
                    );
                }
                None => {
                    ui.weak("log: not writable here — report this");
                }
            }
        });
    app.hud_open = open;
}

fn row(ui: &mut egui::Ui, k: &str, v: &str) {
    ui.weak(k);
    ui.monospace(v);
    ui.end_row();
}

/// Help ▸ Report Bug / Feature Request / Feedback — the two doors to the
/// dev (GitHub issues, email), plus where the log lives, because a bug
/// report without `manganakama.log` usually needs a second round trip.
pub(super) fn feedback_window(ctx: &egui::Context, app: &mut App) {
    if !app.feedback_open {
        return;
    }
    const ISSUES: &str = "https://github.com/bluescreenoff/MangaNakama/issues";
    const MAIL: &str = "bluescreen.off@gmail.com";
    let mut open = app.feedback_open;
    egui::Window::new("Report Bug / Feature Request / Feedback")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
        .show(ctx, |ui| {
            ui.set_max_width(340.0);
            ui.label("Bugs, feature requests, or just impressions — both doors reach the dev:");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Open GitHub Issues").clicked() {
                    unsafe { crate::win32::shell_open(std::path::Path::new(ISSUES)) };
                }
                if ui.small_button("copy link").clicked() {
                    ui.ctx().copy_text(ISSUES.to_owned());
                    app.set_status("issues link copied");
                }
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if ui.button("Email the dev").clicked() {
                    unsafe {
                        crate::win32::shell_open(std::path::Path::new(&format!(
                            "mailto:{MAIL}?subject=MangaNakama feedback"
                        )))
                    };
                }
                if ui.small_button("copy address").clicked() {
                    ui.ctx().copy_text(MAIL.to_owned());
                    app.set_status("email address copied");
                }
                ui.weak(MAIL);
            });
            ui.add_space(6.0);
            ui.separator();
            // The log is the half of a bug report people forget. It is safe
            // to attach by design: no file paths, no names (testlog.rs).
            ui.weak(
                "For bugs, please attach manganakama.log — it names the build, \
                 your GPU and any crash, and carries nothing personal, so it is \
                 safe to post publicly.",
            );
            match crate::testlog::path() {
                Some(p) => {
                    ui.horizontal(|ui| {
                        ui.weak("log");
                        if ui.small_button("copy path").clicked() {
                            ui.ctx().copy_text(p.display().to_string());
                            app.set_status("log path copied");
                        }
                    });
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(p.display().to_string())
                                .size(10.0)
                                .weak(),
                        )
                        .selectable(true)
                        .wrap(),
                    );
                }
                None => {
                    ui.weak("log: nothing written yet this session — it appears beside the exe");
                }
            }
        });
    app.feedback_open = open;
}

/// PM-022: the Go to Page dialog — a number field (1-based), Go on Enter,
/// clamped on apply. CSP's "Specific Page".
pub(super) fn goto_page_window(ctx: &egui::Context, app: &mut App) {
    if !app.goto_page_open {
        return;
    }
    let mut open = app.goto_page_open;
    let mut go = false;
    let mut cancel = false;
    egui::Window::new("Go to Page")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Page");
                ui.add(
                    egui::DragValue::new(&mut app.goto_page_value)
                        .range(1..=(app.pages.len() as i32).max(1))
                        .speed(1),
                );
                ui.weak(format!("of {}", app.pages.len()));
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Go").clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    app.goto_page_open = open && !go && !cancel;
    if go {
        let n = app.goto_page_value.clamp(1, app.pages.len() as i32) as usize;
        app.push_cmd(crate::cmd::AppCmd::PageGotoApply(n));
    }
}

/// TRIAGE 143 (PM-030..033): the Combine/Split spread dialog — gutter
/// width (even, so the halves stay integer) and PM-032's delete-empty
/// toggle. The same three fields serve both operations.
pub(super) fn spread_window(ctx: &egui::Context, app: &mut App) {
    use crate::app::SpreadOp;
    let Some(op) = app.spread_op else {
        return;
    };
    let mut open = true;
    let mut apply = false;
    let mut cancel = false;
    let title = match op {
        SpreadOp::Combine => "Combine Pages into Spread",
        SpreadOp::Split => "Split Spread into Pages",
    };
    egui::Window::new(title)
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            egui::Grid::new("mn.spread")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    ui.label("Gutter (Gap)");
                    ui.add(
                        egui::DragValue::new(&mut app.spread_gap)
                            .range(0..=64)
                            .speed(1)
                            .suffix(" px"),
                    );
                    ui.end_row();
                    ui.label("Delete empty layers");
                    ui.checkbox(&mut app.spread_delete_empty, "");
                    ui.end_row();
                });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(match op {
                    SpreadOp::Combine => {
                        "The two pages become one wide canvas. Gutter pixels are \
                         discarded when the spread is split back (PM-031)."
                    }
                    SpreadOp::Split => {
                        "The gutter's pixels are discarded — art meant to survive \
                         must cross the gap-less boundary."
                    }
                })
                .weak()
                .size(11.0),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .button(match op {
                        SpreadOp::Combine => "Combine",
                        SpreadOp::Split => "Split",
                    })
                    .clicked()
                {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if !open || cancel {
        app.spread_op = None;
    }
    if apply {
        // Even gap only, so split halves stay integer px.
        let gap = (app.spread_gap.max(0) as u32) & !1;
        let de = app.spread_delete_empty;
        app.push_cmd(match op {
            SpreadOp::Combine => crate::cmd::AppCmd::PageCombineApply {
                gap,
                delete_empty: de,
            },
            SpreadOp::Split => crate::cmd::AppCmd::PageSplitApply {
                gap,
                delete_empty: de,
            },
        });
    }
}

/// Export-finish colour names. CSP's 表現色 wording, in English, with the
/// bit depth spelled out for the one that changes the file's nature.
fn colour_label(e: mn_core::LayerExpression) -> &'static str {
    match e {
        mn_core::LayerExpression::Colour => "Full colour",
        mn_core::LayerExpression::Grey => "Grey",
        mn_core::LayerExpression::Mono => "Monochrome (1-bit)",
    }
}

/// Finding 7: CSP calls this 処理方法; the labels say what each kernel is
/// FOR, because "Lanczos" tells a mangaka nothing about his hairlines.
fn resample_label(r: mn_core::export::Resample) -> &'static str {
    match r {
        mn_core::export::Resample::Comic => "Comic (keep hairlines)",
        mn_core::export::Resample::Photo => "Photo (smooth)",
    }
}

fn format_label(f: mn_core::export::ExportFormat) -> &'static str {
    match f {
        mn_core::export::ExportFormat::Png => "PNG (入稿 — lossless)",
        mn_core::export::ExportFormat::Jpeg => "JPEG (提出 — light)",
    }
}

fn crop_label(c: mn_core::export::ExportCrop) -> &'static str {
    match c {
        mn_core::export::ExportCrop::Paper => "Whole paper",
        mn_core::export::ExportCrop::TrimBleed => "Trim + bleed (print)",
        mn_core::export::ExportCrop::Trim => "Trim only (web)",
    }
}

/// PM-050/051/053/054/055: the Export All Pages options — file prefix,
/// page range, split spreads, and CSP's "write text to file" toggle. The
/// name preview is the point of the dialog: the owner can SEE that the
/// defaults still write `<work>-p001.png` before he commits to a folder.
///
/// ROADMAP "print-finishing presets": the Finish picker on top writes the
/// output dpi, the expression colour and split-spreads in one pick. It
/// stores no index — the selection is DERIVED from those three fields
/// (`matching_preset`), so any edit below reads as "Custom" for free and
/// there is no stale index to invalidate.
pub(super) fn export_all_window(ctx: &egui::Context, app: &mut App) {
    if !app.export_all_open {
        return;
    }
    let mut open = true;
    let mut go = false;
    let mut cancel = false;
    let pages = app.pages.len().max(1) as i32;
    let mut preset_pick: Option<usize> = None;
    let presets = mn_core::export::PRINT_PRESETS;
    let active = mn_core::export::matching_preset(app.export_finish());
    let work_dpi = app.work_dpi();
    let page_px = app.doc.size;
    egui::Window::new("Export All Pages")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            egui::Grid::new("mn.exportall")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    // The finishing preset leads: it is the one control
                    // that moves the others.
                    ui.label("Finish");
                    egui::ComboBox::from_id_salt("mn.exportall.preset")
                        .width(240.0)
                        .selected_text(match active {
                            Some(i) => presets[i].name,
                            None => "Custom",
                        })
                        .show_ui(ui, |ui| {
                            for (i, p) in presets.iter().enumerate() {
                                if ui
                                    .selectable_label(active == Some(i), p.name)
                                    .on_hover_text(p.note)
                                    .clicked()
                                {
                                    preset_pick = Some(i);
                                }
                            }
                        })
                        .response
                        .on_hover_text(
                            "Custom is not a choice you make — it is what the picker \
                             reads once you edit a control below",
                        );
                    ui.end_row();

                    // M2: one press fills every knob below from the work's
                    // publisher profile — dpi, colour, split, crop, exact
                    // height. Only shown when a profile is picked.
                    if let Some(p) = app.profile.clone() {
                        ui.label("Profile");
                        if ui
                            .button(format!("Use \"{}\"", p.name))
                            .on_hover_text(
                                "fills the output settings from the profile picked \
                                 in Work Settings; every field stays editable after",
                            )
                            .clicked()
                        {
                            app.export_all_dpi = p.export.dpi;
                            app.export_all_colour = p.export.colour;
                            app.export_all_split = p.export.split_spreads;
                            app.export_all_crop = p.export.crop;
                            app.export_all_px_height = p.export.px_height;
                        }
                        ui.end_row();
                    }

                    ui.label("File prefix");
                    ui.add(
                        egui::TextEdit::singleline(&mut app.export_all_prefix).desired_width(160.0),
                    )
                    .on_hover_text("empty falls back to the work name");
                    ui.end_row();

                    ui.label("Page range");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut app.export_all_range, "");
                        ui.add_enabled(
                            app.export_all_range,
                            egui::DragValue::new(&mut app.export_all_from)
                                .range(1..=pages)
                                .speed(1),
                        );
                        ui.weak("to");
                        ui.add_enabled(
                            app.export_all_range,
                            egui::DragValue::new(&mut app.export_all_to)
                                .range(1..=pages)
                                .speed(1),
                        );
                        ui.weak(format!("of {}", app.pages.len()));
                    });
                    ui.end_row();

                    ui.label("Split spreads");
                    ui.checkbox(&mut app.export_all_split, "").on_hover_text(
                        "a spread page leaves as two files — a is the half a reader meets first",
                    );
                    ui.end_row();

                    ui.label("Write text to file");
                    ui.checkbox(&mut app.export_all_text, "")
                        .on_hover_text("the whole chapter's dialogue, in reading order, as a .txt");
                    ui.end_row();

                    ui.label("Output");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut app.export_all_dpi)
                                .range(0..=1200)
                                .speed(10)
                                .suffix(" dpi"),
                        )
                        .on_hover_text("0 = the work's own resolution, no resample");
                        match work_dpi {
                            Some(d) => ui.weak(format!("work is {d} dpi")),
                            None => ui.weak("no page setup — nothing to scale against"),
                        };
                    });
                    ui.end_row();

                    ui.label("Colour");
                    egui::ComboBox::from_id_salt("mn.exportall.colour")
                        .width(160.0)
                        .selected_text(colour_label(app.export_all_colour))
                        .show_ui(ui, |ui| {
                            for e in [
                                mn_core::LayerExpression::Colour,
                                mn_core::LayerExpression::Grey,
                                mn_core::LayerExpression::Mono,
                            ] {
                                ui.selectable_value(&mut app.export_all_colour, e, colour_label(e));
                            }
                        })
                        .response
                        .on_hover_text(
                            "the finish is applied AFTER the resample, so a 1-bit \
                             export is 1-bit at the size it ships",
                        );
                    ui.end_row();

                    // Finding 7: the kernel only bites on a MONO finish, so
                    // the control is dead unless the finish is mono — and
                    // it says WHY rather than just greying out.
                    let mono = app.export_all_colour == mn_core::LayerExpression::Mono;
                    ui.label("Resample");
                    ui.add_enabled_ui(mono, |ui| {
                        egui::ComboBox::from_id_salt("mn.exportall.resample")
                            .width(160.0)
                            .selected_text(resample_label(app.export_all_resample))
                            .show_ui(ui, |ui| {
                                for r in [
                                    mn_core::export::Resample::Comic,
                                    mn_core::export::Resample::Photo,
                                ] {
                                    ui.selectable_value(
                                        &mut app.export_all_resample,
                                        r,
                                        resample_label(r),
                                    );
                                }
                            })
                            .response
                            .on_hover_text(if mono {
                                "CSP's 処理方法. Comic area-averages the ink and \
                                 re-thresholds so a 1 px line survives the shrink; \
                                 Photo is the smooth filter, which dissolves \
                                 hairlines into grey and then loses them at the \
                                 1-bit threshold."
                            } else {
                                "only a 1-bit finish has a threshold to bias — \
                                 grey and colour always resample smoothly"
                            });
                    });
                    ui.end_row();

                    // Finding 9: 入稿 vs 提出. The quality knob keeps its
                    // value while PNG is picked; it just has nothing to do.
                    ui.label("Format");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("mn.exportall.format")
                            .width(160.0)
                            .selected_text(format_label(app.export_all_format))
                            .show_ui(ui, |ui| {
                                for f in [
                                    mn_core::export::ExportFormat::Png,
                                    mn_core::export::ExportFormat::Jpeg,
                                ] {
                                    ui.selectable_value(
                                        &mut app.export_all_format,
                                        f,
                                        format_label(f),
                                    );
                                }
                            })
                            .response
                            .on_hover_text(
                                "PNG is the file a printer gets. JPEG is the copy \
                                 you send an editor — small, phone-openable; a \
                                 1-bit finish becomes a GREY jpeg of the \
                                 thresholded page, since JPEG cannot hold 1 bit.",
                            );
                        ui.add_enabled(
                            app.export_all_format == mn_core::export::ExportFormat::Jpeg,
                            egui::DragValue::new(&mut app.export_all_quality)
                                .range(1..=100)
                                .speed(1)
                                .prefix("q"),
                        )
                        .on_hover_text("85 is the 提出 norm: no visible ringing at reading size");
                    });
                    ui.end_row();

                    // M2: what rectangle leaves the building. Needs a page
                    // setup — a pixel canvas has no trim to cut to.
                    let has_setup = app.page.is_some();
                    ui.label("Crop");
                    ui.add_enabled_ui(has_setup, |ui| {
                        egui::ComboBox::from_id_salt("mn.exportall.crop")
                            .width(160.0)
                            .selected_text(crop_label(app.export_all_crop))
                            .show_ui(ui, |ui| {
                                for c in [
                                    mn_core::export::ExportCrop::Paper,
                                    mn_core::export::ExportCrop::TrimBleed,
                                    mn_core::export::ExportCrop::Trim,
                                ] {
                                    ui.selectable_value(&mut app.export_all_crop, c, crop_label(c));
                                }
                            })
                            .response
                            .on_hover_text(
                                "whole paper (as always), trim + bleed (what a \
                                 printer wants on the plate), or trim only (what \
                                 a reader sees — the web crop)",
                            );
                    });
                    ui.end_row();

                    ui.label("Exact height");
                    ui.add(
                        egui::DragValue::new(&mut app.export_all_px_height)
                            .range(0..=20000)
                            .speed(8)
                            .suffix(" px"),
                    )
                    .on_hover_text(
                        "0 = off. Wins over dpi when set — a web target speced \
                         in pixels means those pixels. Never upsamples.",
                    );
                    ui.end_row();
                });
            ui.add_space(4.0);
            let prefix = {
                let p = app.export_all_prefix.trim();
                if p.is_empty() {
                    crate::cmd::default_export_stem(app)
                } else {
                    p.to_owned()
                }
            };
            // One source for the extension: `ExportFormat::ext` is what the
            // writer uses too, so the preview cannot promise `.png` and
            // deliver `.jpg`.
            let ext = app.export_all_format.ext();
            let sample = if app.export_all_split {
                format!("{prefix}-p001.{ext} · a spread: {prefix}-p003a.{ext} + {prefix}-p003b.{ext}")
            } else {
                format!("{prefix}-p001.{ext}, {prefix}-p002.{ext}, …")
            };
            ui.label(egui::RichText::new(sample).weak().size(11.0));
            // Say the finish in pixels. "350 dpi" means nothing until you
            // can see what it does to this page, and a request the work
            // cannot honour (no upsampling) has to be visible BEFORE the
            // folder pick, not inferred from the files afterwards.
            let scale = mn_core::export::finish_scale(app.export_all_dpi, work_dpi);
            let out_px = (
                ((page_px.0 as f32 * scale).round() as u32).max(1),
                ((page_px.1 as f32 * scale).round() as u32).max(1),
            );
            let mut size_line = format!(
                "this page {}×{} px → {}×{} px",
                page_px.0, page_px.1, out_px.0, out_px.1
            );
            if app.export_all_dpi > 0 && work_dpi.is_some_and(|d| app.export_all_dpi > d) {
                size_line.push_str(" — the work is coarser than that; export never upsamples");
            }
            ui.label(egui::RichText::new(size_line).weak().size(11.0));
            ui.label(
                egui::RichText::new(
                    "Page numbers in the filename are the page's own — exporting 5 to 8 \
                     writes p005 to p008, it does not renumber.",
                )
                .weak()
                .size(11.0),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Export…").clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if let Some(i) = preset_pick {
        app.push_cmd(AppCmd::ExportAllPreset(i));
    }
    app.export_all_open = open && !go && !cancel;
    if go {
        app.push_cmd(crate::cmd::AppCmd::ExportAllPagesGo);
    }
}

/// TRIAGE 144 (PM-040/045/046/047): the Story Editor — every visible text
/// field in the chapter, page-grouped, editable inline. Edits write
/// through per field (active page = one undo step; other pages re-encode
/// their ORA bytes). Hidden layers are not shown (PM-047).
pub(super) fn story_window(ctx: &egui::Context, app: &mut App) {
    if !app.story_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Story Editor")
        .open(&mut open)
        .default_width(360.0)
        .default_height(480.0)
        .show(ctx, |ui| {
            // PM-046: find & replace.
            ui.horizontal(|ui| {
                ui.label("Find");
                ui.text_edit_singleline(&mut app.story_find);
                ui.label("Replace");
                ui.text_edit_singleline(&mut app.story_repl);
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut app.story_ignore_case, "Ignore case");
                if ui.button("Replace all").clicked() {
                    let (f, o) = app.story_replace_all(
                        &app.story_find.clone(),
                        &app.story_repl.clone(),
                        app.story_ignore_case,
                    );
                    app.set_status(format!("replaced {o} occurrence(s) in {f} field(s)"));
                    app.story_rebuffer();
                }
                // PM-045: restyle every field to the Text tool's settings.
                if ui
                    .button("Apply text-tool style to all")
                    .on_hover_text(
                        "PM-045 — font, size, vertical, outline, spacing from the Text tool",
                    )
                    .clicked()
                {
                    let n = app.story_apply_tool_style();
                    app.set_status(format!("style applied to {n} field(s)"));
                    app.story_rebuffer();
                }
            });
            ui.separator();
            // The script: page-grouped fields.
            let fields = app.story_fields();
            // PM-044: carry the selected field to another page — move or
            // duplicate, without opening either page.
            {
                let n = app.pages.len();
                let sel = app.story_sel.is_some();
                // The field's own page is not a legal target for either
                // action (same-page moves are refused in story_move_field;
                // hiding it here keeps the combo honest).
                let own_page = app
                    .story_sel
                    .and_then(|fi| fields.get(fi).map(|&(p, _, _)| p));
                ui.horizontal(|ui| {
                    ui.weak("field →page");
                    egui::ComboBox::from_id_salt("mn.story.move.to")
                        .width(64.0)
                        .selected_text(format!("{}", (app.story_move_to + 1).min(n)))
                        .show_ui(ui, |ui| {
                            for q in 0..n {
                                if Some(q) == own_page {
                                    continue;
                                }
                                ui.selectable_value(
                                    &mut app.story_move_to,
                                    q,
                                    format!("{}", q + 1),
                                );
                            }
                        });
                    if ui
                        .add_enabled(sel, egui::Button::new("Move"))
                        .on_disabled_hover_text("click a field first")
                        .clicked()
                    {
                        if let Some(fi) = app.story_sel
                            && let Some(&(p, l, i)) = fields.get(fi)
                        {
                            if app.story_move_field(p, l, i, app.story_move_to, false) {
                                app.set_status("field moved");
                            } else {
                                app.set_status(
                                    "the field could not move — pick a decoded target page",
                                );
                            }
                            app.story_sel = None;
                            app.story_rebuffer();
                        }
                    }
                    if ui
                        .add_enabled(sel, egui::Button::new("Duplicate"))
                        .on_disabled_hover_text("click a field first")
                        .clicked()
                    {
                        if let Some(fi) = app.story_sel
                            && let Some(&(p, l, i)) = fields.get(fi)
                        {
                            if app.story_move_field(p, l, i, app.story_move_to, true) {
                                app.set_status("field duplicated");
                            } else {
                                app.set_status(
                                    "the field could not duplicate — pick a decoded target page",
                                );
                            }
                            // story_sel is a POSITION in the page-ascending
                            // field list: duplicating onto an earlier page
                            // shifts every later index, so a kept selection
                            // would aim the next Move at the wrong field —
                            // and Move deletes its source. Drop it, exactly
                            // like Move does.
                            app.story_sel = None;
                            app.story_rebuffer();
                        }
                    }
                });
            }
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut page = usize::MAX;
                let mut do_new: Option<usize> = None;
                for (fi, &(p, l, i)) in fields.iter().enumerate() {
                    if p != page {
                        page = p;
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("Page {}", p + 1))
                                    .size(11.5)
                                    .strong(),
                            );
                            // PM-042: a new field from the script side —
                            // the matching text layer appears on canvas.
                            if ui.small_button("+ field").clicked() {
                                do_new = Some(p);
                            }
                        });
                        ui.add_space(2.0);
                    }
                    let structural = ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("L{}", l + 1)).weak().size(10.0));
                        let mut buf = app.story_bufs.get(fi).cloned().unwrap_or_default();
                        let resp = ui
                            .add(egui::TextEdit::singleline(&mut buf).desired_width(f32::INFINITY));
                        if resp.clicked() || resp.has_focus() {
                            app.story_sel = Some(fi);
                        }
                        if resp.changed() {
                            if let Some(b) = app.story_bufs.get_mut(fi) {
                                *b = buf.clone();
                            }
                            app.story_set_text(p, l, i, &buf);
                        }
                        // PM-043: Shift+Enter splits the field at the
                        // last space BEFORE the midpoint (v1: this egui
                        // wrapper exposes no caret position — recorded);
                        // with no space (Japanese, the primary script)
                        // the NEAREST CHARACTER BOUNDARY to the byte
                        // midpoint — the raw byte midpoint lands inside
                        // a 3-byte kana/kanji two times in three and
                        // the split silently refused (audit G,
                        // 2026-08-19). Backspace at the very start
                        // (empty buffer) merges into the previous field.
                        let shift_enter = ui.input(|i| {
                            i.modifiers.shift && i.key_pressed(egui::Key::Enter) && resp.has_focus()
                        });
                        let mut structural = false;
                        if shift_enter && buf.len() > 1 {
                            match story_split_point(&buf) {
                                Some(at) => {
                                    if app.story_split_field(p, l, i, at) {
                                        app.story_sel = None;
                                        app.story_rebuffer();
                                        structural = true;
                                    } else {
                                        app.set_status("the field could not split");
                                    }
                                }
                                None => app.set_status("the field is too short to split"),
                            }
                        }
                        let bs_at_start =
                            ui.input(|i| i.key_pressed(egui::Key::Backspace) && resp.has_focus());
                        if bs_at_start && buf.is_empty() && app.story_merge_field(p, l, i) {
                            app.story_sel = None;
                            app.story_rebuffer();
                            structural = true;
                        }
                        structural
                    });
                    // A split/merge changed the field list and re-indexed
                    // the buffers; the captured `fields` triples are stale
                    // from here. Stop the walk — next frame re-derives.
                    if structural.inner {
                        break;
                    }
                }
                if let Some(p) = do_new {
                    app.story_new_field(p);
                    app.story_sel = None;
                    app.story_rebuffer();
                }
            });
        });
    if !open {
        app.story_open = false;
        app.story_docs.clear();
        app.story_bufs.clear();
    }
}

/// TRIAGE 140 v1: the speed/focus line generator — kind toggle, sliders,
/// seed, Generate. The params map per kind (focus: centre + inner/outer
/// radius; speed: angle + length range); jitter drives focus only.
pub(super) fn gen_lines_window(ctx: &egui::Context, app: &mut App) {
    if !app.gen_open {
        return;
    }
    let mut open = true;
    let mut apply = false;
    let mut cancel = false;
    egui::Window::new("Generate Effect Lines")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut app.gen_focus, true, "Focus lines (集中線)");
                ui.selectable_value(&mut app.gen_focus, false, "Speed lines (流線)");
            });
            ui.separator();
            let (w, h) = app.doc.size;
            egui::Grid::new("mn.genlines")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    if app.gen_focus {
                        let (hw, hh) = (w as f32 * 0.5, h as f32 * 0.5);
                        if !app.gen_inited {
                            app.gen_a = hw;
                            app.gen_b = hh;
                            app.gen_c = (hw.min(hh) * 0.35).max(16.0);
                            app.gen_d = (hw.min(hh) * 1.3).max(64.0);
                            app.gen_inited = true;
                        }
                        ui.label("Centre X / Y");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut app.gen_a)
                                    .range(0.0..=w as f32)
                                    .speed(1.0),
                            );
                            ui.add(
                                egui::DragValue::new(&mut app.gen_b)
                                    .range(0.0..=h as f32)
                                    .speed(1.0),
                            );
                        });
                        ui.end_row();
                        ui.label("Inner / outer radius");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut app.gen_c)
                                    .range(0.0..=w as f32)
                                    .speed(1.0),
                            );
                            ui.add(
                                egui::DragValue::new(&mut app.gen_d)
                                    .range(4.0..=w as f32 * 2.0)
                                    .speed(1.0),
                            );
                        });
                        ui.end_row();
                        ui.label("Jitter (angle/width/length)");
                        ui.add(
                            egui::DragValue::new(&mut app.gen_jitter)
                                .range(0.0..=1.0)
                                .speed(0.01),
                        );
                        ui.end_row();
                    } else {
                        if !app.gen_inited {
                            app.gen_b = w as f32 * 0.2;
                            app.gen_c = w as f32 * 0.6;
                            app.gen_inited = true;
                        }
                        ui.label("Angle (°)");
                        ui.add(
                            egui::DragValue::new(&mut app.gen_a)
                                .range(-180.0..=180.0)
                                .speed(1.0)
                                .suffix("°"),
                        );
                        ui.end_row();
                        ui.label("Length min / max");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut app.gen_b)
                                    .range(8.0..=w as f32 * 2.0)
                                    .speed(1.0),
                            );
                            ui.add(
                                egui::DragValue::new(&mut app.gen_c)
                                    .range(8.0..=w as f32 * 2.0)
                                    .speed(1.0),
                            );
                        });
                        ui.end_row();
                    }
                    ui.label("Count");
                    ui.add(
                        egui::DragValue::new(&mut app.gen_count)
                            .range(1..=512)
                            .speed(1),
                    );
                    ui.end_row();
                    ui.label("Width (px)");
                    ui.add(
                        egui::DragValue::new(&mut app.gen_width)
                            .range(0.5..=64.0)
                            .speed(0.5),
                    );
                    ui.end_row();
                    ui.label("Seed");
                    ui.add(
                        egui::DragValue::new(&mut app.gen_seed)
                            .range(0..=u64::MAX)
                            .speed(1),
                    );
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Generate").clicked() {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if !open || cancel {
        app.gen_open = false;
    }
    if apply {
        app.push_cmd(crate::cmd::AppCmd::GenLinesApply {
            focus: app.gen_focus,
            a: app.gen_a,
            b: app.gen_b,
            c: app.gen_c,
            d: app.gen_d,
            count: app.gen_count,
            width: app.gen_width,
            jitter: app.gen_jitter,
            seed: app.gen_seed,
        });
    }
}

/// TRIAGE 101/102: the blur-family parameter dialog (FL-011 Gaussian, FL-015
/// Motion, FL-033 Mosaic). One window for all three — the pending `Filter`
/// variant picks the rows, so adding a fourth is a match arm.
///
/// **No live preview, by omission not oversight.** CSP previews these on
/// canvas; ours applies on Apply and you judge it with Ctrl+Z in your hand.
/// A preview needs a whole scratch-composite path that does not exist yet,
/// and shipping the filter without one beats not shipping it. The manual says
/// so on the Layers page.
pub(super) fn filter_window(ctx: &egui::Context, app: &mut App) {
    use mn_core::filter::{MAX_SIGMA, MotionDir, MotionMode, WaveDir};
    let Some(mut draft) = app.filter_draft else {
        return;
    };
    let mut open = true;
    let mut apply = false;
    let mut cancel = false;
    egui::Window::new(draft.label())
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            egui::Grid::new("mn.filter")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| match &mut draft {
                    mn_core::Filter::Gaussian { sigma } => {
                        ui.label("Strength");
                        ui.add(
                            egui::DragValue::new(sigma)
                                .range(1.0..=MAX_SIGMA as f64)
                                .speed(0.1)
                                .suffix(" px"),
                        );
                        ui.end_row();
                    }
                    mn_core::Filter::Motion {
                        angle,
                        length,
                        dir,
                        mode,
                    } => {
                        ui.label("Angle");
                        ui.add(
                            egui::DragValue::new(angle)
                                .range(-360.0..=360.0)
                                .speed(1.0)
                                .suffix("°"),
                        );
                        ui.end_row();
                        ui.label("Length");
                        ui.add(
                            egui::DragValue::new(length)
                                .range(1.0..=1000.0)
                                .speed(1.0)
                                .suffix(" px"),
                        );
                        ui.end_row();
                        ui.label("Direction");
                        ui.horizontal(|ui| {
                            ui.selectable_value(dir, MotionDir::Both, "Both");
                            ui.selectable_value(dir, MotionDir::Forward, "Forward");
                            ui.selectable_value(dir, MotionDir::Backward, "Backward");
                        });
                        ui.end_row();
                        ui.label("Mode");
                        ui.horizontal(|ui| {
                            ui.selectable_value(mode, MotionMode::Uniform, "Box");
                            ui.selectable_value(mode, MotionMode::Taper, "Smooth");
                        });
                        ui.end_row();
                    }
                    mn_core::Filter::RadialBlur { strength } => {
                        ui.label("Strength");
                        ui.add(
                            egui::Slider::new(strength, 0.02..=0.95)
                                .fixed_decimals(2)
                                .text("zoom"),
                        )
                        .on_hover_text("each pixel smears inward along its ray from the centre");
                        ui.end_row();
                    }
                    mn_core::Filter::SpinBlur { angle_deg } => {
                        ui.label("Angle");
                        ui.add(
                            egui::Slider::new(angle_deg, 1.0..=180.0)
                                .fixed_decimals(0)
                                .text("°"),
                        )
                        .on_hover_text("each pixel smears along its arc, both directions");
                        ui.end_row();
                    }
                    mn_core::Filter::Unsharp { radius, amount } => {
                        ui.label("Radius");
                        ui.add(
                            egui::DragValue::new(radius)
                                .range(1.0..=50.0)
                                .speed(0.1)
                                .suffix(" px"),
                        )
                        .on_hover_text("the blur that gets subtracted — how wide the edge halo is");
                        ui.end_row();
                        ui.label("Amount");
                        ui.add(
                            egui::Slider::new(amount, 0.1..=5.0)
                                .fixed_decimals(2)
                                .text("×"),
                        )
                        .on_hover_text("how much of the original-minus-blur difference is added back");
                        ui.end_row();
                    }
                    mn_core::Filter::Pinch { amount } => {
                        ui.label("Amount");
                        ui.add(
                            egui::Slider::new(amount, -0.95..=0.95)
                                .fixed_decimals(2)
                                .text("pinch ⇠ ⇢ bulge"),
                        )
                        .on_hover_text(
                            "positive squeezes toward the centre, negative bulges out",
                        );
                        ui.end_row();
                    }
                    mn_core::Filter::Ripple {
                        amplitude,
                        wavelength,
                    } => {
                        ui.label("Amplitude");
                        ui.add(
                            egui::DragValue::new(amplitude)
                                .range(-256.0..=256.0)
                                .speed(0.5)
                                .suffix(" px"),
                        );
                        ui.end_row();
                        ui.label("Wavelength");
                        ui.add(
                            egui::DragValue::new(wavelength)
                                .range(2.0..=512.0)
                                .speed(1.0)
                                .suffix(" px"),
                        )
                        .on_hover_text("the spacing of the rings, measured out from the centre");
                        ui.end_row();
                    }
                    mn_core::Filter::Wave {
                        amplitude,
                        wavelength,
                        dir,
                    } => {
                        ui.label("Amplitude");
                        ui.add(
                            egui::DragValue::new(amplitude)
                                .range(-256.0..=256.0)
                                .speed(0.5)
                                .suffix(" px"),
                        );
                        ui.end_row();
                        ui.label("Wavelength");
                        ui.add(
                            egui::DragValue::new(wavelength)
                                .range(2.0..=512.0)
                                .speed(1.0)
                                .suffix(" px"),
                        );
                        ui.end_row();
                        ui.label("Direction");
                        ui.horizontal(|ui| {
                            for d in [WaveDir::Horizontal, WaveDir::Vertical] {
                                ui.selectable_value(dir, d, d.label());
                            }
                        })
                        .response
                        .on_hover_text("which way the rows or columns slide");
                        ui.end_row();
                    }
                    mn_core::Filter::Twirl { angle_deg } => {
                        ui.label("Angle");
                        ui.add(
                            egui::Slider::new(angle_deg, -720.0..=720.0)
                                .fixed_decimals(0)
                                .text("°"),
                        )
                        .on_hover_text("strongest at the centre, nothing at the rim");
                        ui.end_row();
                    }
                    mn_core::Filter::RemoveDust { max_px } => {
                        ui.label("Speck size");
                        ui.add(
                            egui::DragValue::new(max_px)
                                .range(1..=256)
                                .speed(1.0)
                                .suffix(" px"),
                        )
                        .on_hover_text(
                            "the AREA of a blob, not its width — a blob of this many \
                             connected pixels or fewer is cleared",
                        );
                        ui.end_row();
                    }
                    mn_core::Filter::LineWidth { delta } => {
                        ui.label("Width");
                        ui.add(
                            egui::Slider::new(delta, -32..=32).text("thin ⇠ ⇢ thick"),
                        )
                        .on_hover_text("how many pixels the ink grows, or shrinks by");
                        ui.end_row();
                    }
                    mn_core::Filter::Mosaic { cell } => {
                        ui.label("Cell size");
                        ui.add(
                            egui::DragValue::new(cell)
                                .range(2..=512)
                                .speed(1.0)
                                .suffix(" px"),
                        );
                        ui.end_row();
                    }
                    // The one-shots have no parameters and never open this.
                    _ => {}
                });
            ui.weak(
                "Applies to the active layer, inside the selection. No preview — undo to compare.",
            );
            ui.add_space(2.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("  Apply  ").clicked() {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    // Write the edited draft back before acting on it, or a drag this frame is
    // lost the moment the window closes.
    app.filter_draft = Some(draft);
    if apply {
        app.push_cmd(AppCmd::FilterApply(draft));
    } else if !open || cancel {
        app.filter_draft = None;
    }
}

/// TRIAGE 146 (UI-060): register the current layout under a name.
pub(super) fn workspace_window(ctx: &egui::Context, app: &mut App) {
    if !app.workspace_open {
        return;
    }
    let mut open = true;
    let mut ok = false;
    let mut cancel = false;
    egui::Window::new("Register Workspace")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut app.workspace_draft);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Register").clicked() {
                    ok = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if !open || cancel {
        app.workspace_open = false;
    }
    if ok {
        let name = app.workspace_draft.trim().to_string();
        app.workspace_open = false;
        if name.is_empty() {
            app.set_status("workspace needs a name");
        } else {
            app.workspace_register(&name);
            app.set_status(format!("workspace registered: {name}"));
        }
    }
}

/// TX-styles: the work's named text styles. Edits live in a draft; Apply
/// commits one style through `TextStyleUpsert` (this page reflows, one
/// undo press); the footer button pushes every style to every page.
pub(super) fn text_styles_window(ctx: &egui::Context, app: &mut App) {
    use mn_core::text::{LineSpacing, PT_PER_Q, TextStyle};
    if !app.text_styles_open {
        if !app.styles_draft.is_empty() {
            app.styles_draft.clear();
        }
        return;
    }
    if app.styles_draft.is_empty() {
        app.styles_draft = if app.doc.text_styles.is_empty() {
            TextStyle::defaults()
        } else {
            app.doc.text_styles.clone()
        };
    }
    let mut draft = std::mem::take(&mut app.styles_draft);
    let mut open = true;
    let mut apply: Option<usize> = None;
    let mut delete: Option<usize> = None;
    let mut all_pages = false;
    let mut add = false;
    let tool_font = app.text_font.clone();
    egui::Window::new("Text styles")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            ui.weak(
                "Every text can follow a named style. Apply re-styles this page \
                 (one undo); the bottom button re-styles the whole work.",
            );
            for (i, s) in draft.iter_mut().enumerate() {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(96.0));
                    let font_label = if s.font.is_empty() {
                        "font: (keep each text's)".to_owned()
                    } else {
                        format!("font: {}", s.font)
                    };
                    let fresp = ui.button(font_label).on_hover_text(
                        "click: pin the text tool's current font — right-click: unpin",
                    );
                    if fresp.clicked() {
                        s.font = tool_font.clone();
                    }
                    if fresp.secondary_clicked() {
                        s.font.clear();
                    }
                    if ui
                        .button("Apply")
                        .on_hover_text("save + reflow this page")
                        .clicked()
                    {
                        apply = Some(i);
                    }
                    if ui.small_button("✕").clicked() {
                        delete = Some(i);
                    }
                });
                ui.horizontal(|ui| {
                    let mut q = s.size_pt / PT_PER_Q;
                    if theme::ValueBar::new("Size", 8.0, 60.0)
                        .decimals(1)
                        .suffix(" Q")
                        .width(150.0)
                        .show(ui, &mut q)
                        .changed()
                    {
                        s.size_pt = q * PT_PER_Q;
                    }
                    ui.weak(format!("= {:.1} pt", s.size_pt));
                    let spacing_label = match s.line_spacing {
                        LineSpacing::Auto => "line: auto".to_owned(),
                        LineSpacing::Percent(p) => format!("line: {p:.0} %"),
                        LineSpacing::Pt(p) => format!("line: {p:.1} pt"),
                    };
                    egui::ComboBox::from_id_salt(("mn.style.line", i))
                        .width(96.0)
                        .selected_text(spacing_label)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(false, "auto").clicked() {
                                s.line_spacing = LineSpacing::Auto;
                            }
                            for p in [125.0, 150.0, 175.0, 200.0] {
                                if ui.selectable_label(false, format!("{p:.0} %")).clicked() {
                                    s.line_spacing = LineSpacing::Percent(p);
                                }
                            }
                        });
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("New style").clicked() {
                    add = true;
                }
                if ui
                    .button("Re-style every page in the work")
                    .on_hover_text(
                        "saves all styles, then re-styles every other page DIRECTLY — \
                         undo covers this page only",
                    )
                    .clicked()
                {
                    all_pages = true;
                }
            });
        });
    if let Some(i) = apply {
        let s = draft[i].clone();
        if s.name.trim().is_empty() {
            app.set_status("a style needs a name");
        } else {
            app.push_cmd(crate::cmd::AppCmd::TextStyleUpsert(s));
        }
    }
    if let Some(i) = delete {
        let s = draft.remove(i);
        if !s.name.trim().is_empty() {
            app.push_cmd(crate::cmd::AppCmd::TextStyleDelete(s.name));
        }
    }
    if add {
        draft.push(TextStyle {
            name: format!("Style {}", draft.len() + 1),
            font: String::new(),
            size_pt: 20.0 * PT_PER_Q,
            color: [0, 0, 0],
            outline_px: 0.0,
            outline_color: [255, 255, 255],
            letter_spacing_pt: 0.0,
            line_spacing: LineSpacing::Percent(150.0),
        });
    }
    if all_pages {
        for s in &draft {
            if !s.name.trim().is_empty() {
                app.push_cmd(crate::cmd::AppCmd::TextStyleUpsert(s.clone()));
            }
        }
        app.push_cmd(crate::cmd::AppCmd::TextStyleAllPages);
    }
    app.styles_draft = draft;
    if !open {
        app.text_styles_open = false;
        app.styles_draft.clear();
    }
}
