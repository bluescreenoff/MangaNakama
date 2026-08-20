//! Modal dialogs and always-on-top windows: New Comic, Work Settings, the
//! Sub Tool Detail wrench window, and the F1 Diagnostics HUD.

use super::property::{Section, brush_sliders, prop_sections};
use super::theme::ValueBar;
use crate::app::App;
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
                                    .color(super::theme::TEXT_STRONG),
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
    egui::Window::new("New Comic")
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
/// changes affect guides + new pages only — existing pixels stay untouched.
pub(super) fn work_settings_window(ctx: &egui::Context, app: &mut App) {
    if !app.work_settings_open {
        return;
    }
    let mut open = app.work_settings_open;
    let mut apply = false;
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
        app.push_cmd(AppCmd::WorkSettingsApply);
        app.work_settings_open = false;
    } else {
        app.work_settings_open = open && !cancel;
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

// --- preferences --------------------------------------------------------

/// A group header inside the Preferences window. Five of these instead of
/// tabs: ten rows fit in one column, and a tab bar over ten rows is a
/// filing system for a drawer with three things in it.
fn pref_head(ui: &mut egui::Ui, text: &str) {
    ui.add_space(7.0);
    ui.label(
        egui::RichText::new(text)
            .strong()
            .color(super::theme::TEXT_STRONG),
    );
    ui.add_space(2.0);
}

/// The Autosave dropdown's labels — CSP's own range, plus Off.
fn autosave_label(min: u32) -> String {
    if min == 0 {
        "Off".to_owned()
    } else {
        format!("{min} min")
    }
}

/// Edit ▸ Preferences…. **One window, five headers, no tabs, no tree, no
/// search box** — the ten values that are genuinely user preferences rather
/// than architecture constants (docs/design/PREFERENCES-SPEC.md §5 is the
/// list of what deliberately stays hardcoded, and it is the load-bearing
/// half). Revisit the shape if this ever passes ~20 rows, and treat that as
/// a warning rather than a milestone.
///
/// Every default is today's constant, so a user who never opens this window
/// sees no change at all.
pub(super) fn prefs_window(ctx: &egui::Context, app: &mut App) {
    if !app.prefs_open {
        return;
    }
    let mut open = app.prefs_open;
    let mut changed = false;
    let mut reset = false;
    let mut preset_pick: Option<String> = None;
    let autosave_before = app.prefs.autosave_min;
    let preset_now = app.prefs.new_preset_setup().name;

    egui::Window::new("Preferences")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            ui.set_max_width(430.0);
            let p = &mut app.prefs;

            pref_head(ui, "Saving");
            egui::Grid::new("mn.prefs.saving")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    ui.label("Autosave");
                    egui::ComboBox::from_id_salt("mn.prefs.autosave")
                        .width(110.0)
                        .selected_text(autosave_label(p.autosave_min))
                        .show_ui(ui, |ui| {
                            for m in [0u32, 5, 10, 15, 30, 60] {
                                if ui
                                    .selectable_label(p.autosave_min == m, autosave_label(m))
                                    .clicked()
                                    && p.autosave_min != m
                                {
                                    p.autosave_min = m;
                                    changed = true;
                                }
                            }
                        });
                    ui.end_row();

                    // PR-041. A second row rather than an "Every operation"
                    // entry in the dropdown above: the two are not
                    // alternatives — with both on you get whichever comes
                    // first — and a dropdown cannot say that.
                    //
                    // It greys out when Autosave is Off, because Off has to
                    // mean off. A user who turns autosave off (a stalling
                    // network drive, a huge page) and then finds MORE writes
                    // happening has been lied to by the control he reached
                    // for, and the setting doing nothing invisibly would be
                    // the same lie one layer down.
                    let timer_on = p.autosave_min != 0;
                    ui.label("Also after every operation");
                    changed |= ui
                        .add_enabled(
                            timer_on,
                            egui::Checkbox::new(&mut p.autosave_every_op, ""),
                        )
                        .on_hover_text(
                            "Writes the recovery copy as soon as an operation finishes, \
                             instead of waiting for the timer. Costs a save per operation \
                             on a print-resolution page.",
                        )
                        .changed();
                    ui.end_row();
                });
            ui.weak(
                "Work folders save in place on this timer; everything else gets a \
                 separate recovery copy. Off stops the timer entirely — including \
                 the per-operation save, which is the same write on a different \
                 trigger.",
            );

            pref_head(ui, "Drawing");
            egui::Grid::new("mn.prefs.drawing")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    ui.label("Mouse smoothing floor");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut p.mouse_smooth_px)
                                .range(0.0..=mn_core::stabilize::MAX_STRING_PX)
                                .speed(0.25)
                                .suffix(" px"),
                        )
                        .changed();
                    ui.end_row();
                });
            ui.weak(
                "Mouse strokes only — the pen always uses the sub tool's own \
                 stabilizer. 0 turns the floor off.",
            );

            pref_head(ui, "Canvas & view");
            egui::Grid::new("mn.prefs.canvas")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    ui.label("New canvas");
                    ui.horizontal(|ui| {
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut p.new_canvas.0)
                                    .range(1..=65535)
                                    .suffix(" px"),
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut p.new_canvas.1)
                                    .range(1..=65535)
                                    .suffix(" px"),
                            )
                            .changed();
                    });
                    ui.end_row();

                    ui.label("New Comic preset");
                    egui::ComboBox::from_id_salt("mn.prefs.preset")
                        .width(240.0)
                        .selected_text(preset_now.clone())
                        .show_ui(ui, |ui| {
                            for s in PageSetup::presets() {
                                if ui.selectable_label(preset_now == s.name, &s.name).clicked()
                                    && preset_now != s.name
                                {
                                    preset_pick = Some(s.name.clone());
                                }
                            }
                        });
                    ui.end_row();

                    ui.label("Fit margin");
                    let mut pct = p.fit_margin * 100.0;
                    if ui
                        .add(
                            egui::DragValue::new(&mut pct)
                                .range(80.0..=100.0)
                                .speed(0.25)
                                .fixed_decimals(0)
                                .suffix(" %"),
                        )
                        .changed()
                    {
                        p.fit_margin = pct / 100.0;
                        changed = true;
                    }
                    ui.end_row();

                    ui.label("Wheel zoom step");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut p.wheel_step)
                                .range(1.02..=1.50)
                                .speed(0.005)
                                .fixed_decimals(2)
                                .prefix("×"),
                        )
                        .changed();
                    ui.end_row();

                    ui.label("View rotation step");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut p.rotate_step_deg)
                                .range(1.0..=90.0)
                                .speed(0.5)
                                .fixed_decimals(0)
                                .suffix(" °"),
                        )
                        .changed();
                    ui.end_row();
                });
            ui.weak("New canvas and preset apply to the next document you create.");

            pref_head(ui, "Text");
            egui::Grid::new("mn.prefs.text")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    ui.label("New text size");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut p.text_size_pt)
                                .range(4.0..=72.0)
                                .speed(0.25)
                                .fixed_decimals(1)
                                .suffix(" pt"),
                        )
                        .changed();
                    ui.end_row();

                    ui.label("Recent files kept");
                    changed |= ui
                        .add(egui::DragValue::new(&mut p.recent_depth).range(1..=32))
                        .changed();
                    ui.end_row();
                });

            pref_head(ui, "History");
            egui::Grid::new("mn.prefs.history")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    ui.label("Undo depth");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut p.undo_depth)
                                .range(50..=5000)
                                .speed(5.0)
                                .suffix(" steps"),
                        )
                        .changed();
                    ui.end_row();
                });
            ui.weak("Deeper history uses more memory. Lowering it drops the oldest steps now.");

            ui.add_space(4.0);
            ui.separator();
            if ui.button("Reset to defaults").clicked() {
                reset = true;
            }
            ui.add_space(2.0);
            ui.weak(
                egui::RichText::new(format!(
                    "Settings live in {} — deleting that file resets everything here.",
                    crate::app::prefs::path_hint()
                ))
                .size(10.0),
            );
        });

    if reset {
        app.prefs.reset();
        app.new_doc_draft.setup = app.prefs.new_preset_setup();
    }
    if let Some(name) = preset_pick {
        app.prefs.new_preset = name;
        // Take effect NOW, not just on the next launch — the draft the New
        // Comic dialog reuses is built once, at startup.
        app.new_doc_draft.setup = app.prefs.new_preset_setup();
        changed = true;
    }
    if changed {
        app.prefs.mark_dirty();
    }
    // The autosave timer lives in the message loop; `pump_commands` re-arms
    // it from here (0 = kill it).
    if app.prefs.autosave_min != autosave_before {
        app.autosave_rearm = Some(app.prefs.autosave_ms());
    }
    app.prefs_open = open;
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
    let mut live = app.adjust_preview.as_ref().is_some_and(|p| p.live);
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
                    mn_core::Adjust::Binarize { threshold } => {
                        ui.label("Threshold");
                        ui.add(egui::Slider::new(threshold, 0.0..=1.0).fixed_decimals(2));
                        ui.end_row();
                    }
                    // Reverse gradient has no parameters and never opens
                    // this window (the menu applies it straight away).
                    mn_core::Adjust::Invert => {}
                });
            if matches!(adj, mn_core::Adjust::Binarize { .. }) {
                ui.weak("Transparent pixels stay transparent; alpha is not touched.");
            }
            ui.weak("Applies to the ACTIVE layer only, inside the selection if there is one.");
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

/// PM-050/051/053/054/055: the Export All Pages options — file prefix,
/// page range, split spreads, and CSP's "write text to file" toggle. The
/// name preview is the point of the dialog: the owner can SEE that the
/// defaults still write `<work>-p001.png` before he commits to a folder.
pub(super) fn export_all_window(ctx: &egui::Context, app: &mut App) {
    if !app.export_all_open {
        return;
    }
    let mut open = true;
    let mut go = false;
    let mut cancel = false;
    let pages = app.pages.len().max(1) as i32;
    egui::Window::new("Export All Pages")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            egui::Grid::new("mn.exportall")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    ui.label("File prefix");
                    ui.add(
                        egui::TextEdit::singleline(&mut app.export_all_prefix)
                            .desired_width(160.0),
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
                    ui.checkbox(&mut app.export_all_split, "")
                        .on_hover_text(
                            "a spread page leaves as two files — a is the half a reader meets first",
                        );
                    ui.end_row();

                    ui.label("Write text to file");
                    ui.checkbox(&mut app.export_all_text, "")
                        .on_hover_text("the whole chapter's dialogue, in reading order, as a .txt");
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
            let sample = if app.export_all_split {
                format!("{prefix}-p001.png · a spread: {prefix}-p003a.png + {prefix}-p003b.png")
            } else {
                format!("{prefix}-p001.png, {prefix}-p002.png, …")
            };
            ui.label(egui::RichText::new(sample).weak().size(11.0));
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
    use mn_core::filter::{MAX_SIGMA, MotionDir, MotionMode};
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
            ui.weak("Applies to the active layer, inside the selection. No preview — undo to compare.");
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
