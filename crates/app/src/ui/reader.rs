//! The reader overlay (owner top item, 2026-08-18): read the chapter
//! in-app — spread layout, turn-by-click zones, the options strip, and
//! the Edit-this-page round trip. Two-tier paint: the preview-tier
//! texture lands instantly, the sharp pass replaces it a frame or two
//! later (App::reader_frame).

use super::theme;
use crate::app::App;
use crate::app::reader::ReaderMode;

pub(super) fn reader_overlay(ui: &mut egui::Ui, app: &mut App) {
    let bg = [
        egui::Color32::BLACK,
        egui::Color32::from_gray(24),
        egui::Color32::from_gray(232),
    ][(app.reader.opts.bg % 3) as usize];
    let frame = egui::Frame::new().fill(bg);

    egui::Panel::bottom("mn.reader.bar")
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(theme::c().field)
                .inner_margin(egui::Margin::symmetric(8, 4)),
        )
        .show(ui, |ui| reader_bar(ui, app));

    // Reader v2: the flags side panel (notes + jump — the
    // proofreading pass's collected list).
    if app.reader.show_flags {
        egui::Panel::left("mn.reader.flags")
            .exact_size(236.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::c().field)
                    .inner_margin(egui::Margin::symmetric(8, 6)),
            )
            .show(ui, |ui| reader_flags_panel(ui, app));
    }

    egui::CentralPanel::default()
        .frame(frame)
        .show(ui, |ui| reader_pages(ui, app));
}

fn reader_pages(ui: &mut egui::Ui, app: &mut App) {
    let screens = app.reader_screens();
    if screens == 0 {
        return;
    }
    let screen = app.reader.screen.min(screens - 1);
    let cells = app.reader_screen_pages(screen);
    let doc_asp = app.doc.size.1.max(1) as f32 / app.doc.size.0.max(1) as f32;
    // Cell aspect: the landed texture knows combined-spread widths; the
    // doc aspect is the pre-texture stand-in.
    let aspects: Vec<f32> = cells
        .iter()
        .map(|&c| {
            c.and_then(|i| {
                app.reader_tex(i)
                    .map(|(_, _, (w, h), _)| (*h).max(1) as f32 / (*w).max(1) as f32)
            })
            .unwrap_or(doc_asp)
        })
        .collect();
    let present = cells.iter().flatten().count().max(1);
    let gap = 10.0;

    let fit_width = app.reader.opts.fit_width;
    let avail = ui.available_rect_before_wrap();
    let numbers_h = if app.reader.opts.numbers { 16.0 } else { 0.0 };
    // Fit-page: solve the common height so the whole spread fits; the
    // per-cell width follows from its aspect. Fit-width: cells fill the
    // width (ScrollArea scrolls the overflow vertically).
    let inv_sum: f32 = aspects.iter().sum();
    let area_h = (avail.height() - numbers_h - 8.0).max(40.0);
    let (cell_h, _) = if fit_width {
        let w = ((avail.width() - gap * (present as f32 - 1.0)) / present as f32).max(40.0);
        let h = w * (aspects.iter().sum::<f32>() / present as f32);
        (h, w)
    } else {
        let h_by_w = (avail.width() - gap * (present as f32 - 1.0)) / inv_sum.max(0.01);
        (area_h.min(h_by_w), 0.0)
    };

    // 1:1 (the tone-moiré check): each cell at its page's TRUE canvas
    // size — one canvas px per screen px; the sharp pass renders native,
    // so the tone you squint at is the real rasterized one.
    let zoom = app.reader.opts.zoom_100;
    let cell_px: Vec<(f32, f32)> = if zoom {
        cells
            .iter()
            .map(|&c| {
                c.map(|i| {
                    let (w, h) = app.reader_page_canvas(i);
                    (w as f32, h as f32)
                })
                .unwrap_or((0.0, 0.0))
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut page_rects: Vec<(usize, egui::Rect)> = Vec::new();
    // `area` is passed in explicitly: the scrolling branches allocate their
    // content block FIRST, which advances the cursor — reading
    // available_rect_before_wrap() after that painted the pages BELOW the
    // scrollable content (a blank pane at any scroll position).
    let mut draw = |ui: &mut egui::Ui, app: &mut App, area: egui::Rect, vh: f32| {
        let total_w: f32 = (0..cells.len())
            .map(|slot| {
                if zoom {
                    cell_px[slot].0
                } else {
                    vh / aspects[slot]
                }
            })
            .sum::<f32>()
            + gap * (present as f32 - 1.0);
        let mut x = area.left() + (area.width() - total_w).max(0.0) * 0.5;
        let y = area.top() + 4.0;
        for (slot, &c) in cells.iter().enumerate() {
            let (cw, ch) = if zoom {
                cell_px[slot]
            } else {
                (vh / aspects[slot], vh)
            };
            let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cw, ch));
            x += cw + gap;
            let Some(i) = c else { continue };
            page_rects.push((i, rect));
            let p = ui.painter();
            p.rect_filled(rect, 2.0, egui::Color32::WHITE);
            if let Some((_, _, _, t)) = app.reader_tex(i) {
                p.image(
                    t.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            p.rect_stroke(
                rect,
                2.0,
                egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
                egui::StrokeKind::Outside,
            );
            // Reader v2: a flagged page carries an orange corner flag.
            if app.reader.flags.contains_key(&i) {
                let a = rect.right_top();
                let c = egui::Color32::from_rgb(255, 140, 0);
                p.add(egui::Shape::convex_polygon(
                    vec![a, egui::pos2(a.x - 14.0, a.y), egui::pos2(a.x, a.y + 14.0)],
                    c,
                    egui::Stroke::NONE,
                ));
            }
            if app.reader.opts.numbers {
                p.text(
                    egui::pos2(rect.center().x, rect.bottom() + 2.0),
                    egui::Align2::CENTER_TOP,
                    format!("{}/{}", i + 1, app.pages.len()),
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_gray(160),
                );
            }
            // Edit this page: a small hover affordance on the page's corner.
            let btn_rect = egui::Rect::from_min_size(
                egui::pos2(rect.right() - 52.0, rect.top() + 4.0),
                egui::vec2(48.0, 18.0),
            );
            let resp = ui.put(btn_rect, egui::Button::new("Edit").small());
            if resp.clicked() {
                app.reader_edit_page(i);
            }
        }
    };

    if zoom {
        // Pannable both ways: 1:1 shows a crop of a native-size page.
        egui::ScrollArea::both()
            .id_salt("mn.reader.zoom")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let max_h = cell_px.iter().map(|s| s.1).fold(0.0, f32::max);
                let total_w =
                    cell_px.iter().map(|s| s.0).sum::<f32>() + gap * (present as f32 - 1.0);
                let h = max_h + numbers_h + 12.0;
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(total_w, h), egui::Sense::hover());
                draw(ui, app, rect, 0.0);
            });
    } else if fit_width {
        egui::ScrollArea::vertical()
            .id_salt("mn.reader.scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let h = cell_h + numbers_h + 12.0;
                let (rect, _) = ui
                    .allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
                draw(ui, app, rect, cell_h);
            });
    } else {
        let rect = ui.available_rect_before_wrap();
        draw(ui, app, rect, cell_h);
    }

    // The sharp pass renders to the size actually displayed (first cell;
    // cells share the height).
    if let Some(&(i, r)) = page_rects.first() {
        let _ = i;
        app.reader.frame_px = (r.width(), r.height());
    }

    // Click zones: outer thirds of the page area turn pages, in the
    // reading direction.
    let resp = ui.interact(
        egui::Rect::from_min_max(avail.min, egui::pos2(avail.right(), avail.bottom())),
        egui::Id::new("mn.reader.zones"),
        egui::Sense::click(),
    );
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = (p.x - avail.left()) / avail.width().max(1.0);
            if t < 0.3 {
                app.reader_turn(app.reader_left_delta());
            } else if t > 0.7 {
                app.reader_turn(-app.reader_left_delta());
            }
        }
    }
}

fn reader_bar(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        let screens = app.reader_screens().max(1);
        let cur = app.reader.screen;
        if ui
            .add_enabled(cur > 0, egui::Button::new("◀").small())
            .clicked()
        {
            app.reader_turn(-1);
        }
        if ui
            .add_enabled(cur + 1 < screens, egui::Button::new("▶").small())
            .clicked()
        {
            app.reader_turn(1);
        }
        ui.separator();
        let dbl = app.reader.opts.mode == ReaderMode::Double;
        if ui
            .add(
                egui::Button::new(if dbl { "Double" } else { "Single" })
                    .small()
                    .selected(dbl),
            )
            .clicked()
        {
            app.reader.opts.mode = if dbl {
                ReaderMode::Single
            } else {
                ReaderMode::Double
            };
            app.reader.screen = 0;
        }
        if ui
            .add(
                egui::Button::new(if app.reader.opts.rtl {
                    "← RTL"
                } else {
                    "LTR →"
                })
                .small(),
            )
            .clicked()
        {
            app.reader.opts.rtl = !app.reader.opts.rtl;
        }
        if ui
            .add(
                egui::Button::new("Shift pair")
                    .small()
                    .selected(app.reader.opts.offset),
            )
            .clicked()
        {
            app.reader.opts.offset = !app.reader.opts.offset;
            app.reader.screen = 0;
        }
        if ui
            .add(
                egui::Button::new(if app.reader.opts.fit_width {
                    "Fit width"
                } else {
                    "Fit page"
                })
                .small()
                .selected(app.reader.opts.fit_width),
            )
            .clicked()
        {
            app.reader.opts.fit_width = !app.reader.opts.fit_width;
        }
        if ui
            .add(
                egui::Button::new("1:1")
                    .small()
                    .selected(app.reader.opts.zoom_100),
            )
            .on_hover_text("tone moiré check — one canvas px per screen px (key: 1); drag pans")
            .clicked()
        {
            app.reader_toggle_zoom();
        }
        if ui
            .add(
                egui::Button::new("n°")
                    .small()
                    .selected(app.reader.opts.numbers),
            )
            .clicked()
        {
            app.reader.opts.numbers = !app.reader.opts.numbers;
        }
        if ui.small_button("bg").clicked() {
            app.reader.opts.bg = (app.reader.opts.bg + 1) % 3;
        }
        // Reader v2: the flags list (F flags the current spread).
        if ui
            .add(
                egui::Button::new(format!("⚑ {}", app.reader.flags.len()))
                    .small()
                    .selected(app.reader.show_flags),
            )
            .on_hover_text("flagged pages — F flags the spread you are reading")
            .clicked()
        {
            app.reader.show_flags = !app.reader.show_flags;
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new("Exit").small()).clicked() {
                app.reader_close();
            }
            if ui
                .add(
                    egui::Button::new(if app.reader.fullscreen {
                        "Windowed"
                    } else {
                        "F11 Full"
                    })
                    .small(),
                )
                .clicked()
            {
                app.reader_toggle_fullscreen();
            }
            let cells = app.reader_screen_pages(cur);
            let mut n = 0;
            for (slot, &c) in cells.iter().enumerate() {
                let Some(i) = c else { continue };
                n += 1;
                let label = if cells.iter().flatten().count() > 1 {
                    format!("Edit {}", if slot == 0 { "left" } else { "right" })
                } else {
                    "Edit this page".to_owned()
                };
                let _ = n;
                if ui.add(egui::Button::new(label).small()).clicked() {
                    app.reader_edit_page(i);
                }
            }
            ui.label(format!("{}/{}", cur + 1, screens));
        });
    });
}

/// Reader v2: one row per flagged page — the note field, Go, unflag.
/// Walk it after the pass: every "this hand is wrong" is one click away.
fn reader_flags_panel(ui: &mut egui::Ui, app: &mut App) {
    ui.strong("Flags");
    uiweak_hint(ui, "F flags the spread you are reading");
    let mut pages: Vec<usize> = app.reader.flags.keys().copied().collect();
    pages.sort_unstable();
    if pages.is_empty() {
        ui.weak("nothing flagged");
        return;
    }
    let mut jump = None;
    let mut unflag = None;
    for p in pages {
        ui.horizontal(|ui| {
            ui.label(format!("p{}", p + 1));
            if ui.small_button("Go").clicked() {
                jump = Some(p);
            }
            if ui.small_button("✕").clicked() {
                unflag = Some(p);
            }
        });
        let mut buf = app.reader.flags.get(&p).cloned().unwrap_or_default();
        let r = ui.add(
            egui::TextEdit::singleline(&mut buf)
                .desired_width(f32::INFINITY)
                .hint_text("what is wrong here?"),
        );
        if r.changed() {
            app.reader_set_note(p, &buf);
        }
        ui.add_space(2.0);
    }
    if let Some(p) = jump {
        app.reader_goto_page(p);
    }
    if let Some(p) = unflag {
        app.reader_unflag(p);
    }
}

fn uiweak_hint(ui: &mut egui::Ui, text: &str) {
    ui.weak(text);
}
