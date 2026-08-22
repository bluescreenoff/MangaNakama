//! Shared chrome and small widgets: the panel frame, the icon button, the
//! menu item, the group caption. (The docked-palette wrapper with its
//! collapse/float header was replaced by the docking system — ui/dock.rs.)

use super::icons::{self, Icon};
use super::theme;

pub(super) fn chrome_frame(margin: egui::Margin) -> egui::Frame {
    egui::Frame::new()
        .fill(theme::c().window)
        .inner_margin(margin)
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
    icon_btn_tint(ui, icon, size, selected, enabled, tip, None)
}

/// [`icon_btn`] with the glyph in a caller's colour. For toggles whose subject
/// already carries a hue elsewhere in the palette — the Layers reference/draft
/// marks — so the button and the mark it flips read as one thing. `None` is
/// the plain theme text.
pub(super) fn icon_btn_tint(
    ui: &mut egui::Ui,
    icon: Icon,
    size: f32,
    selected: bool,
    enabled: bool,
    tip: &str,
    tint: Option<egui::Color32>,
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
        p.rect_filled(rect, 3.0, theme::c().sel_row);
        p.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0, theme::c().accent),
            egui::StrokeKind::Inside,
        );
    } else if pressed {
        p.rect_filled(rect, 3.0, theme::c().active);
        p.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0, theme::c().border),
            egui::StrokeKind::Inside,
        );
    } else if enabled && resp.hovered() {
        p.rect_filled(rect, 3.0, theme::c().hover);
        p.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0, theme::c().outline),
            egui::StrokeKind::Inside,
        );
    }
    let color = if !enabled {
        theme::c().text_weak.gamma_multiply(0.55)
    } else if selected || resp.hovered() {
        tint.unwrap_or(theme::c().text_strong)
    } else {
        // Idle a tinted glyph sits back at the palette's own weight: two
        // coloured toggles must not out-shout the grey ones beside them.
        tint.map_or(theme::c().text, |c| c.gamma_multiply(0.8))
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
    let galley =
        ui.fonts_mut(|f| f.layout_no_wrap(label.to_uppercase(), font, theme::c().text_weak));
    let p = ui.painter();
    p.galley(
        egui::pos2(rect.left() + 2.0, rect.center().y - galley.size().y * 0.5),
        galley.clone(),
        theme::c().text_weak,
    );
    p.hline(
        egui::Rangef::new(rect.left() + galley.size().x + 8.0, rect.right() - 2.0),
        rect.center().y,
        egui::Stroke::new(1.0, theme::c().outline),
    );
}

/// A border thickness in BOTH units (owner, 2026-08-21). The value is EDITED
/// in mm — that is what a printer and a frame folder speak — but CSP shows
/// the same border as "15", meaning pixels at the page's dpi, and a mm-only
/// readout leaves nothing to compare against the app the owner is coming
/// from. Pure (mm + dpi in, one string out) so the rounding is testable:
/// thick borders read as whole pixels, sub-10 px ones keep a decimal, since
/// "2 px" and "2.4 px" are a visible difference at 96 dpi.
pub(super) fn px_mm_text(mm: f32, dpi: u32) -> String {
    let px = mm / 25.4 * dpi.max(1) as f32;
    let px = if px >= 10.0 {
        format!("{px:.0}")
    } else {
        format!("{px:.1}")
    };
    format!("{px} px · {mm:.2} mm")
}

#[cfg(test)]
mod tests {
    use super::px_mm_text;

    /// The owner's own numbers: CSP's "15" and our "0.64 mm" are the same
    /// border at 600 dpi, and the label says both.
    #[test]
    fn px_mm_text_says_both_units() {
        assert_eq!(px_mm_text(0.64, 600), "15 px · 0.64 mm");
        // 0.8 mm is our own default frame border (cmd.rs), CSP's 19 px.
        assert_eq!(px_mm_text(0.8, 600), "19 px · 0.80 mm");
        // A pixel canvas has no page setup: 96 dpi, where the same border is
        // thin enough that the decimal is the whole information.
        assert_eq!(px_mm_text(0.64, 96), "2.4 px · 0.64 mm");
        // Degenerate dpi must not divide by zero or print "inf px".
        assert_eq!(px_mm_text(1.0, 0), "0.0 px · 1.00 mm");
    }
}
