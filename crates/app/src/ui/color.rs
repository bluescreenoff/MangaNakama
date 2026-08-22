//! Colour UI: the Photoshop-style hue ring + SV square picker (the ONE
//! ui.data_mut user — the press-time part latch), the Color Set grid, the
//! main/sub/transparent slots, and the HSV helpers. `picker_sync` runs at
//! the top of every frame (build) before color_section reads the state.

use super::icons::{self, Icon};
use super::theme;
use super::widgets::icon_btn;
use crate::app::App;
use crate::cmd::{AppCmd, Slot};

pub(super) fn rgb32(rgb: [f32; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(
        (rgb[0] * 255.0) as u8,
        (rgb[1] * 255.0) as u8,
        (rgb[2] * 255.0) as u8,
    )
}

/// Chip metrics for [`color_slots`]: the main/sub square, how far the sub
/// chip sits behind it, and the transparent chip beside the pair.
const CHIP: f32 = 20.0;
const BEHIND: f32 = 11.0;
const CHIP_SMALL: f32 = 15.0;

/// CSP's main / sub / transparent drawing-colour slots: two SMALL chips with
/// the sub colour tucked behind and down-right of the main one, then the
/// transparent chip and the swap arrow beside them.
///
/// It used to paint three swatches that stretched the full palette width,
/// which in the Tool palette read as a footer bar rather than a colour
/// control, and in the Color palette as three loose slabs under the wheel
/// (owner, 2026-08-22). Same three click targets, same commands — only the
/// geometry changed. Shared by the Tool and Color palettes so the control is
/// literally the same object in both.
pub(super) fn color_slots(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        let (block, _) = ui.allocate_exact_size(
            egui::vec2(CHIP + BEHIND, CHIP + BEHIND),
            egui::Sense::hover(),
        );
        let main = egui::Rect::from_min_size(block.min, egui::vec2(CHIP, CHIP));
        let sub = main.translate(egui::vec2(BEHIND, BEHIND));
        // The sub chip is painted UNDER the main one, so only its lower-right
        // L is visible. Its hit area is the visible right strip and nothing
        // else, so neither chip can steal the other's click.
        let sub_hit = egui::Rect::from_min_max(egui::pos2(main.right(), sub.top()), sub.max);
        let id = ui.id().with("mn.slots");
        let sub_resp = ui
            .interact(sub_hit, id.with("sub"), egui::Sense::click())
            .on_hover_text("Sub colour (X swaps)");
        let main_resp = ui
            .interact(main, id.with("main"), egui::Sense::click())
            .on_hover_text("Main colour");

        let p = ui.painter();
        p.rect_filled(sub, 2.0, rgb32(app.sub_color));
        chip_stroke(p, sub, app.slot == Slot::Sub, sub_resp.hovered());
        p.rect_filled(main, 2.0, rgb32(app.main_color));
        chip_stroke(p, main, app.slot == Slot::Main, main_resp.hovered());

        if main_resp.clicked() {
            app.push_cmd(AppCmd::SetSlot(Slot::Main));
        }
        if sub_resp.clicked() {
            app.push_cmd(AppCmd::SetSlot(Slot::Sub));
        }

        ui.add_space(4.0);
        let (tr, tr_resp) = ui.allocate_exact_size(
            egui::vec2(CHIP_SMALL, CHIP_SMALL),
            egui::Sense::click(),
        );
        let p = ui.painter();
        icons::checkerboard(p, tr, 4.0);
        chip_stroke(p, tr, app.slot == Slot::Transparent, tr_resp.hovered());
        if tr_resp
            .on_hover_text("Transparent — erase with this brush (C)")
            .clicked()
        {
            app.push_cmd(AppCmd::SetSlot(Slot::Transparent));
        }

        if icon_btn(ui, Icon::Swap, 18.0, false, true, "Swap main/sub (X)").clicked() {
            app.push_cmd(AppCmd::SwapColors);
        }
    });
}

/// The chip outline: accent when the slot is the one being drawn with, a
/// bright hairline under the pointer, the panel's own outline otherwise.
fn chip_stroke(p: &egui::Painter, rect: egui::Rect, active: bool, hovered: bool) {
    let stroke = if active {
        egui::Stroke::new(2.0, theme::ACCENT)
    } else if hovered {
        egui::Stroke::new(1.0, theme::TEXT_STRONG)
    } else {
        egui::Stroke::new(1.0, theme::OUTLINE)
    };
    p.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
}

// --- color panel (Photoshop-style SV square + hue strip) -----------------

fn rgb_to_hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        (((g - b) / d).rem_euclid(6.0)) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max <= 0.0 { 0.0 } else { d / max };
    [h, s, max]
}

fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

fn hsv32(hsv: [f32; 3]) -> egui::Color32 {
    rgb32(hsv_to_rgb(hsv))
}

/// Keep the panel's HSV state in step with the active colour, preserving hue
/// and saturation through grayscale values (where RGB forgets them).
pub(super) fn picker_sync(app: &mut App) {
    let rgb = app.active_color();
    if rgb != app.picker_rgb_cache {
        let [h, s, v] = rgb_to_hsv(rgb);
        if v > 1e-4 && s > 1e-4 {
            app.picker_hsv[0] = h;
        }
        if v > 1e-4 {
            app.picker_hsv[1] = s;
        }
        app.picker_hsv[2] = v;
        app.picker_rgb_cache = rgb;
    }
}

pub(super) fn color_section(ui: &mut egui::Ui, app: &mut App) {
    // CSP's default Color Wheel: hue ring with the SV square inscribed, then
    // ONE tight run of controls under it — value row, hex, colour chips,
    // recent — with the leftover height left as clean panel. The rows used to
    // drift apart on the frame's 3pt rhythm plus their own add_space, which
    // with the full-width slots made the lower half read as unfinished
    // (owner, 2026-08-22: "feels ugly like there's some missing space").
    ui.spacing_mut().item_spacing.y = 2.0;
    let mut hsv = app.picker_hsv;
    let w = ui.available_width();
    // The wheel grows with the palette (CSP's does); the old 172 ceiling left
    // a floated Color palette with a small wheel adrift in a wide panel.
    let side = w.clamp(110.0, 220.0);
    let (all, _) = ui.allocate_exact_size(egui::vec2(w, side), egui::Sense::hover());
    let centre = egui::pos2(all.center().x, all.top() + side * 0.5);
    let r_out = side * 0.5 - 1.0;
    let r_in = r_out * 0.80;
    // Square inscribed in the inner circle, with a whisper of clearance.
    let half = r_in / std::f32::consts::SQRT_2 * 0.94;
    let sq = egui::Rect::from_center_size(centre, egui::vec2(half * 2.0, half * 2.0));

    // One interact region; which part a drag drives is decided at the press
    // and remembered, so a wobbly hue drag cannot fall into the square.
    let id = ui.id().with("mn.wheel");
    let resp = ui.interact(all, id, egui::Sense::click_and_drag());
    let part_id = id.with("part");
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos() {
        let d = pos - centre;
        let dist = d.length();
        let part: u8 = if resp.drag_started() || resp.clicked() {
            let p = if dist >= r_in * 0.98 && dist <= r_out * 1.15 {
                1 // ring
            } else if sq.contains(pos) {
                2 // square
            } else {
                0
            };
            ui.data_mut(|m| m.insert_temp(part_id, p));
            p
        } else if resp.dragged() || resp.drag_stopped() {
            // The release frame is included so the value the user let go on
            // is the one that reaches the history — see the push below.
            ui.data(|m| m.get_temp::<u8>(part_id)).unwrap_or(0)
        } else {
            0
        };
        match part {
            1 => {
                hsv[0] = (d.y.atan2(d.x) / std::f32::consts::TAU)
                    .rem_euclid(1.0)
                    .min(0.9999);
                changed = true;
            }
            2 => {
                hsv[1] = ((pos.x - sq.left()) / sq.width()).clamp(0.0, 1.0);
                hsv[2] = (1.0 - (pos.y - sq.top()) / sq.height()).clamp(0.0, 1.0);
                changed = true;
            }
            _ => {}
        }
    }

    let p = ui.painter();
    // Hue ring: 48 segments, two triangles each, hue interpolated per vertex.
    const SEG: usize = 48;
    let mut ring = egui::Mesh::default();
    for k in 0..=SEG {
        let t = k as f32 / SEG as f32;
        let a = t * std::f32::consts::TAU;
        let (s, c) = a.sin_cos();
        let col = hsv32([t.min(0.9999), 1.0, 1.0]);
        ring.colored_vertex(centre + egui::vec2(c * r_in, s * r_in), col);
        ring.colored_vertex(centre + egui::vec2(c * r_out, s * r_out), col);
    }
    for k in 0..SEG {
        let a = (k * 2) as u32;
        ring.add_triangle(a, a + 1, a + 2);
        ring.add_triangle(a + 1, a + 3, a + 2);
    }
    p.add(egui::Shape::mesh(ring));
    p.circle_stroke(centre, r_out, egui::Stroke::new(1.0, theme::BORDER));
    p.circle_stroke(centre, r_in, egui::Stroke::new(1.0, theme::BORDER));

    // SV square: v * mix(white, hue, s) is bilinear in RGB, so a small grid
    // mesh renders it exactly (up to triangle interpolation, invisible at 12²).
    const N: usize = 12;
    let mut mesh = egui::Mesh::default();
    for j in 0..=N {
        for i in 0..=N {
            let s = i as f32 / N as f32;
            let v = 1.0 - j as f32 / N as f32;
            mesh.colored_vertex(
                egui::pos2(
                    sq.left() + s * sq.width(),
                    sq.top() + (1.0 - v) * sq.height(),
                ),
                hsv32([hsv[0], s, v]),
            );
        }
    }
    for j in 0..N {
        for i in 0..N {
            let a = (j * (N + 1) + i) as u32;
            let b = a + 1;
            let c = a + (N + 1) as u32;
            let d = c + 1;
            mesh.add_triangle(a, b, c);
            mesh.add_triangle(b, d, c);
        }
    }
    p.add(egui::Shape::mesh(mesh));
    p.rect_stroke(
        sq,
        0.0,
        egui::Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Outside,
    );

    // Handles: a ring marker on the wheel, a circle in the square.
    let ha = hsv[0] * std::f32::consts::TAU;
    let (hs, hc) = ha.sin_cos();
    let hm = centre + egui::vec2(hc, hs) * (r_in + r_out) * 0.5;
    p.circle_stroke(
        hm,
        (r_out - r_in) * 0.42,
        egui::Stroke::new(1.6, egui::Color32::WHITE),
    );
    p.circle_stroke(
        hm,
        (r_out - r_in) * 0.42 + 1.0,
        egui::Stroke::new(1.0, egui::Color32::BLACK),
    );
    let hp = egui::pos2(
        sq.left() + hsv[1] * sq.width(),
        sq.top() + (1.0 - hsv[2]) * sq.height(),
    );
    p.circle_stroke(hp, 4.5, egui::Stroke::new(1.2, egui::Color32::BLACK));
    p.circle_stroke(hp, 3.5, egui::Stroke::new(1.4, egui::Color32::WHITE));

    if changed {
        app.picker_hsv = hsv;
        let rgb = hsv_to_rgb(hsv);
        app.picker_rgb_cache = rgb;
        // Mid-drag values are live-only; the release commits. Otherwise one
        // hue sweep would fill the whole Recent strip with itself.
        app.push_cmd(if resp.dragged() {
            AppCmd::SetSlotColorLive(rgb)
        } else {
            AppCmd::SetSlotColor(rgb)
        });
    }

    // RGB readout.
    ui.add_space(5.0);
    let rgb = app.active_color();
    let mut vals = [
        (rgb[0] * 255.0).round(),
        (rgb[1] * 255.0).round(),
        (rgb[2] * 255.0).round(),
    ];
    let mut edited = false;
    let mut live = false;
    ui.horizontal(|ui| {
        // Fit the three R/G/B fields to whatever the column gives — the old
        // 26pt floor overflowed its labels onto the numbers in narrow panes.
        let dw = ((ui.available_width() - 6.0 * 4.0 - 3.0 * 8.0) / 3.0).clamp(18.0, 44.0);
        for (label, v) in ["R", "G", "B"].iter().zip(vals.iter_mut()) {
            ui.label(
                egui::RichText::new(*label)
                    .size(10.5)
                    .color(theme::TEXT_WEAK),
            );
            let r = ui.add_sized(
                [dw, 16.0],
                egui::DragValue::new(v).range(0.0..=255.0).speed(1.0),
            );
            // Same rule as the wheel: a held spinner is live, the release
            // (which reports no change of its own) is the commit.
            edited |= r.changed() || r.drag_stopped();
            live |= r.dragged();
        }
    });
    if edited {
        let rgb = [vals[0] / 255.0, vals[1] / 255.0, vals[2] / 255.0];
        app.push_cmd(if live {
            AppCmd::SetSlotColorLive(rgb)
        } else {
            AppCmd::SetSlotColor(rgb)
        });
    }

    hex_field(ui, app, rgb);
    ui.add_space(5.0);
    color_slots(ui, app);
    history_strip(ui, app);
}

/// CO-064: the field every palette on the internet is quoted in. Shows the
/// active colour as `rrggbb` (the `#` is the label, so it cannot be deleted
/// by accident) and accepts `#rrggbb`, `rrggbb`, `#rgb` or `rgb` on Enter
/// or on clicking away. **Text we cannot read exactly reverts** — clamping
/// `#ff00` to some nearby colour would be a guess, and a guess in a colour
/// field is worse than doing nothing.
fn hex_field(ui: &mut egui::Ui, app: &mut App, rgb: [f32; 3]) {
    let current = mn_core::palette::hex_string(rgb)[1..].to_owned();
    let id = ui.id().with("mn.hex");
    // Outside an edit the field mirrors the colour, so the wheel drives it.
    if !ui.memory(|m| m.has_focus(id)) {
        app.hex_edit = current.clone();
    }
    let mut done = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("#")
                .size(10.5)
                .color(theme::TEXT_WEAK),
        );
        let w = (ui.available_width() - 6.0).clamp(46.0, 78.0);
        done = ui
            .add_sized(
                [w, 16.0],
                egui::TextEdit::singleline(&mut app.hex_edit)
                    .id(id)
                    .char_limit(7)
                    .horizontal_align(egui::Align::Center),
            )
            .on_hover_text("Hex colour — #rrggbb, rrggbb or the 3-digit short form")
            .lost_focus();
    });
    if done {
        match mn_core::palette::parse_hex(&app.hex_edit) {
            Some(c) => app.push_cmd(AppCmd::SetSlotColor(c)),
            None => app.hex_edit = current,
        }
    }
}

/// CO-042: the colours recently *used*, newest first — automatic, bounded
/// and disposable, which is exactly what the Color Set below is not. Hidden
/// when empty rather than showing an empty row that explains nothing.
fn history_strip(ui: &mut egui::Ui, app: &mut App) {
    if app.color_history.is_empty() {
        return;
    }
    let size = 14.0;
    let mut pick = None;
    let mut cmd = None;
    ui.add_space(3.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
        ui.label(
            egui::RichText::new("Recent")
                .size(9.5)
                .color(theme::TEXT_WEAK),
        );
        for rgb in &app.color_history {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, 2.0, rgb32(*rgb));
            let stroke = if resp.hovered() {
                egui::Stroke::new(1.5, theme::TEXT_STRONG)
            } else {
                egui::Stroke::new(1.0, theme::OUTLINE)
            };
            p.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
            let resp = resp.on_hover_text(mn_core::palette::hex_string(*rgb));
            if resp.clicked() {
                pick = Some(*rgb);
            }
            resp.context_menu(|ui| {
                if ui.button("Add all to Color Set").clicked() {
                    cmd = Some(AppCmd::AddHistoryToSwatches);
                    ui.close();
                }
                if ui.button("Clear recent colours").clicked() {
                    cmd = Some(AppCmd::ClearColorHistory);
                    ui.close();
                }
            });
        }
    });
    if let Some(rgb) = pick {
        app.push_cmd(AppCmd::SetSlotColor(rgb));
    }
    if let Some(c) = cmd {
        app.push_cmd(c);
    }
}

// --- color set ----------------------------------------------------------

pub(super) fn swatch_grid(ui: &mut egui::Ui, app: &mut App) {
    let size = 17.0;
    let mut pick = None;
    let mut del = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
        for (i, sw) in app.swatches.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, 2.0, rgb32(sw.rgb));
            let stroke = if resp.hovered() {
                egui::Stroke::new(1.5, theme::TEXT_STRONG)
            } else {
                egui::Stroke::new(1.0, theme::OUTLINE)
            };
            p.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
            // An imported palette's own name for the colour, when it has
            // one; the hex otherwise, which is what you would look up.
            let hex = mn_core::palette::hex_string(sw.rgb);
            let resp = resp.on_hover_text(if sw.name.is_empty() {
                hex
            } else {
                format!("{}  ({hex})", sw.name)
            });
            if resp.clicked() {
                pick = Some(sw.rgb);
            }
            resp.context_menu(|ui| {
                if ui.button("Delete swatch").clicked() {
                    del = Some(i);
                    ui.close();
                }
            });
        }
        // Add the current colour as a new swatch.
        if icon_btn(ui, Icon::Plus, size, false, true, "Add current colour").clicked() {
            app.push_cmd(AppCmd::AddSwatch(app.active_color()));
        }
        // Import a GIMP/Krita .gpl palette (appended, persisted).
        if icon_btn(
            ui,
            Icon::Folder,
            size,
            false,
            true,
            "Import palette (.gpl)…",
        )
        .clicked()
        {
            app.push_cmd(AppCmd::ImportPalette);
        }
    });
    // CO-023. Off by default and said out loud, because the alternative —
    // a palette that fills itself while you work — is the failure mode.
    ui.add_space(3.0);
    let mut auto = app.layout.auto_swatch;
    if ui
        .checkbox(
            &mut auto,
            egui::RichText::new("Add picked colours").size(10.5),
        )
        .on_hover_text(
            "Colours taken with the eyedropper join this set (duplicates are ignored).\n\
             Off by default: the Color palette's Recent strip already remembers your \
             picks and forgets them again, which is what you want for most of them.",
        )
        .changed()
    {
        app.layout.note_auto_swatch(auto);
    }
    if let Some(rgb) = pick {
        app.push_cmd(AppCmd::SetSlotColor(rgb));
    }
    if let Some(i) = del {
        app.push_cmd(AppCmd::DeleteSwatch(i));
    }
}
