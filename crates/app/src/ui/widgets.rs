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
    // The one place ~57 buttons ask what colour their icon's subject is. A
    // caller-supplied `tint` WINS: it means "this button and the mark it
    // flips are the same thing", which is a stronger statement than the
    // icon's own category. A disabled button is grey all through — a
    // coloured glyph on a dead button reads as available.
    let accent = match (enabled, tint) {
        (false, _) | (_, Some(_)) => None,
        (true, None) => icons::accent_for(icon).map(|a| {
            if selected || resp.hovered() {
                a
            } else {
                // Same idle rule as `tint` above, for the same reason.
                a.gamma_multiply(0.8)
            }
        }),
    };
    let mut glyph = rect.shrink(size * 0.18);
    if pressed {
        glyph = glyph.translate(egui::vec2(0.0, 0.75));
    }
    icons::paint_role(ui.painter(), glyph, icon, color, accent);
    resp.on_hover_text(tip)
}

/// `icons::paint` for the places that draw a glyph THEMSELVES — layer rows,
/// document tabs, sub tool rows — rather than through [`icon_btn`]. Same
/// base colour as before, plus the icon's accent when colours are on.
///
/// It exists so the toggle has two doors in the whole app instead of a
/// dozen: a direct `icons::paint` call would silently stay monochrome
/// forever and nobody would notice which of the fifty glyphs was the stale
/// one.
pub(super) fn paint_icon(p: &egui::Painter, r: egui::Rect, icon: Icon, base: egui::Color32) {
    // A hair back from full: these glyphs sit beside weak-grey row text, and
    // a saturated mark next to `text_weak` reads as the loudest thing in a
    // list of forty rows.
    let accent = icons::accent_for(icon).map(|a| a.gamma_multiply(0.85));
    icons::paint_role(p, r, icon, base, accent);
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

/// Title-case a section caption: "Frame border" → "Frame Border". CSP titles
/// its property sections; we used to SHOUT them, which is the loudest
/// typography in a palette of quiet grey rows (nit N1).
///
/// Normalising HERE and not at the ~30 call sites keeps the source strings
/// readable and covers the two dynamic callers (the property section table
/// and sub tool preset GROUP FOLDER NAMES, which the artist types).
///
/// Anything with a non-ASCII character is returned untouched: a Japanese
/// group name has no case, and `to_uppercase` on one is at best a no-op.
/// Apostrophes do not start a word, so "Artist's" stays "Artist's".
fn title_case(s: &str) -> String {
    if !s.is_ascii() {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut at_word_start = true;
    for ch in s.chars() {
        out.push(if at_word_start {
            ch.to_ascii_uppercase()
        } else {
            ch.to_ascii_lowercase()
        });
        at_word_start = !(ch.is_ascii_alphanumeric() || ch == '\'');
    }
    out
}

/// Small title-case caption with a rule to the right — a list section label,
/// not a widget.
pub(super) fn group_caption(ui: &mut egui::Ui, label: &str) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 15.0), egui::Sense::hover());
    let font = egui::FontId::proportional(9.5);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(title_case(label), font, theme::c().text_weak));
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
    use super::{px_mm_text, title_case};

    /// The captions the palettes actually pass, plus the two shapes that
    /// would be damaged by a naive `split_whitespace`.
    #[test]
    fn captions_are_title_case_and_leave_japanese_alone() {
        assert_eq!(title_case("Frame border"), "Frame Border");
        assert_eq!(title_case("LAYER SETTINGS"), "Layer Settings");
        assert_eq!(title_case("Scale / Rotation"), "Scale / Rotation");
        // A slash or a hyphen starts a word; an apostrophe does not.
        assert_eq!(title_case("gutter l/r"), "Gutter L/R");
        assert_eq!(title_case("artist's own"), "Artist's Own");
        // Nothing to case: separators and numbers come back as they went in.
        assert_eq!(title_case("---"), "---");
        assert_eq!(title_case("2 up"), "2 Up");
        // Non-ASCII is never touched, so a Japanese preset group folder keeps
        // its name and its ASCII neighbours in the same string keep theirs.
        assert_eq!(title_case("ペン入れ"), "ペン入れ");
        assert_eq!(title_case("my ペン group"), "my ペン group");
        assert_eq!(title_case(""), "");
    }

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
