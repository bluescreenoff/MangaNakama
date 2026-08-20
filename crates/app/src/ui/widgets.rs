//! Shared chrome and small widgets: the panel frame, the icon button, the
//! menu item, the group caption. (The docked-palette wrapper with its
//! collapse/float header was replaced by the docking system — ui/dock.rs.)

use super::icons::{self, Icon};
use super::theme;

pub(super) fn chrome_frame(margin: egui::Margin) -> egui::Frame {
    egui::Frame::new().fill(theme::WINDOW).inner_margin(margin)
}

// --- shared little widgets ----------------------------------------------

/// A square icon button: quietly raised, accent-filled when selected, and a
/// real pressed state (fill darkens, the glyph dips a hair) so clicks feel
/// mechanical, CSP-style.
pub(super) fn icon_btn(
    ui: &mut egui::Ui,
    icon: Icon,
    size: f32,
    selected: bool,
    enabled: bool,
    tip: &str,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), sense);
    let pressed = enabled && resp.is_pointer_button_down_on();
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 3.0, theme::SEL_ROW);
        p.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
    } else if pressed {
        p.rect_filled(rect, 3.0, theme::ACTIVE);
        p.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0, theme::BORDER),
            egui::StrokeKind::Inside,
        );
    } else if enabled && resp.hovered() {
        p.rect_filled(rect, 3.0, theme::HOVER);
        p.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0, theme::OUTLINE),
            egui::StrokeKind::Inside,
        );
    }
    let color = if !enabled {
        theme::TEXT_WEAK.gamma_multiply(0.55)
    } else if selected || resp.hovered() {
        theme::TEXT_STRONG
    } else {
        theme::TEXT
    };
    let mut glyph = rect.shrink(size * 0.18);
    if pressed {
        glyph = glyph.translate(egui::vec2(0.0, 0.75));
    }
    icons::paint(ui.painter(), glyph, icon, color);
    resp.on_hover_text(tip)
}

pub(super) fn item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    ui.add(egui::Button::new(label).shortcut_text(shortcut))
        .clicked()
}

/// One-line, ellipsized text galley ("Nam…"). Row labels in narrow dock
/// columns must truncate — wrapping or overflowing runs them over
/// neighbouring rows and widgets (owner report 2026-08-16, pic 2).
/// Returns the galley; the caller paints it (their painter borrow outlives).
pub(super) fn ellipsis(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, f32::INFINITY);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(8.0));
    ui.fonts_mut(|f| f.layout_job(job))
}

/// Small uppercase caption with a rule to the right — a list section label,
/// not a widget.
pub(super) fn group_caption(ui: &mut egui::Ui, label: &str) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 15.0), egui::Sense::hover());
    let font = egui::FontId::proportional(9.5);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(label.to_uppercase(), font, theme::TEXT_WEAK));
    let p = ui.painter();
    p.galley(
        egui::pos2(rect.left() + 2.0, rect.center().y - galley.size().y * 0.5),
        galley.clone(),
        theme::TEXT_WEAK,
    );
    p.hline(
        egui::Rangef::new(rect.left() + galley.size().x + 8.0, rect.right() - 2.0),
        rect.center().y,
        egui::Stroke::new(1.0, theme::OUTLINE),
    );
}
