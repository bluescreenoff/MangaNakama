//! The visual system: one place that owns every colour, spacing and widget
//! finish, so the app reads as *one* professionally built surface instead of
//! egui defaults.
//!
//! Reference points (docs/design/CSP-UI-SPEC.md + the owner's screenshots):
//! CSP's structure and density, Rebelle's palette chrome (title strips, filled
//! value bars), Photoshop's layered grays — chrome darkest, panel bodies a
//! step lighter, inset fields darker again, and the pasteboard behind the page
//! darker than everything so the artwork is the brightest thing on screen.

use egui::{Color32, CornerRadius, Stroke};

// --- design tokens -------------------------------------------------------

/// Window chrome: menu bar, status bar, and the gutters between palettes.
pub const WINDOW: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x22);
/// Palette bodies.
pub const PANEL: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2e);
/// Palette title strips (a step darker than the body, Rebelle-style).
pub const HEADER: Color32 = Color32::from_rgb(0x24, 0x24, 0x27);
/// Inset controls: slider troughs, list wells, text edits, combo boxes.
pub const FIELD: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x1f);
/// Hovered rows/buttons.
pub const HOVER: Color32 = Color32::from_rgb(0x35, 0x35, 0x3b);
/// Pressed/open widgets.
pub const ACTIVE: Color32 = Color32::from_rgb(0x3e, 0x3e, 0x46);
/// The accent — selection, active tool, filled slider bars.
pub const ACCENT: Color32 = Color32::from_rgb(0x4f, 0x8c, 0xd2);
/// Accent-tinted fill for value bars (quieter than raw accent).
pub const ACCENT_FILL: Color32 = Color32::from_rgb(0x37, 0x5a, 0x84);
/// Accent-tinted selected-row background.
pub const SEL_ROW: Color32 = Color32::from_rgb(0x2e, 0x41, 0x59);
/// 1px seams between regions.
pub const BORDER: Color32 = Color32::from_rgb(0x15, 0x15, 0x17);
/// Subtle outline on raised controls.
pub const OUTLINE: Color32 = Color32::from_rgb(0x3c, 0x3c, 0x44);
pub const TEXT: Color32 = Color32::from_rgb(0xd4, 0xd4, 0xd8);
pub const TEXT_WEAK: Color32 = Color32::from_rgb(0x8e, 0x8e, 0x96);
pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf4);

/// Something did not happen and the user needs to notice. The status bar
/// paints refusals in this instead of TEXT_WEAK — a grey line at the bottom
/// of the window is indistinguishable from no feedback at all (owner,
/// 2026-08-19: "dragging a .txt does not seem to do much" — it did, and it
/// said so, in grey).
pub const WARN: Color32 = Color32::from_rgb(0xe0, 0xa0, 0x4a);

/// Corner rounding: palettes 4, controls 2.
pub const R_PANEL: u8 = 4;
pub const R_CTRL: u8 = 2;

// --- value bar -----------------------------------------------------------

/// The CSP/Rebelle property control: a filled horizontal bar with the label
/// inside on the left and the value inside on the right. Click or drag
/// anywhere on it to set the value.
pub struct ValueBar<'a> {
    label: &'a str,
    min: f32,
    max: f32,
    log: bool,
    step: f32,
    decimals: usize,
    suffix: &'a str,
    width: Option<f32>,
    /// Right-side text override (e.g. show pixels while editing a multiplier).
    display: Option<String>,
}

impl<'a> ValueBar<'a> {
    pub fn new(label: &'a str, min: f32, max: f32) -> Self {
        Self {
            label,
            min,
            max,
            log: false,
            step: 0.0,
            decimals: 0,
            suffix: "",
            width: None,
            display: None,
        }
    }

    pub fn display_text(mut self, s: String) -> Self {
        self.display = Some(s);
        self
    }
    pub fn log(mut self) -> Self {
        self.log = true;
        self
    }
    pub fn step(mut self, s: f32) -> Self {
        self.step = s;
        self
    }
    pub fn decimals(mut self, d: usize) -> Self {
        self.decimals = d;
        self
    }
    pub fn suffix(mut self, s: &'a str) -> Self {
        self.suffix = s;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }

    fn to_t(&self, v: f32) -> f32 {
        let t = if self.log {
            (v.max(1e-6).ln() - self.min.max(1e-6).ln()) / (self.max.ln() - self.min.max(1e-6).ln())
        } else {
            (v - self.min) / (self.max - self.min)
        };
        t.clamp(0.0, 1.0)
    }

    fn from_t(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        let mut v = if self.log {
            (self.min.max(1e-6).ln() + t * (self.max.ln() - self.min.max(1e-6).ln())).exp()
        } else {
            self.min + t * (self.max - self.min)
        };
        if self.step > 0.0 {
            v = (v / self.step).round() * self.step;
        }
        v.clamp(self.min, self.max)
    }

    pub fn show(self, ui: &mut egui::Ui, v: &mut f32) -> egui::Response {
        let h = 17.0;
        let w = self.width.unwrap_or_else(|| ui.available_width());
        let (rect, mut resp) =
            ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click_and_drag());
        if resp.dragged() || resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let t = (pos.x - rect.left()) / rect.width().max(1.0);
                let nv = self.from_t(t);
                if nv != *v {
                    *v = nv;
                    resp.mark_changed();
                }
            }
        }
        let p = ui.painter();
        let cr = CornerRadius::same(3);
        p.rect_filled(rect, cr, FIELD);
        let t = self.to_t(*v);
        let fill_w = t * rect.width();
        if fill_w > 0.5 {
            let hot = resp.hovered() || resp.dragged();
            let fill = if hot {
                Color32::from_rgb(0x40, 0x69, 0x9c)
            } else {
                ACCENT_FILL
            };
            let clip =
                egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + fill_w, rect.bottom()));
            p.with_clip_rect(clip).rect_filled(rect, cr, fill);
        }
        p.rect_stroke(
            rect,
            cr,
            Stroke::new(1.0, OUTLINE),
            egui::StrokeKind::Inside,
        );
        let font = egui::FontId::proportional(11.0);
        p.text(
            egui::pos2(rect.left() + 6.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            self.label,
            font.clone(),
            TEXT,
        );
        let value_text = self
            .display
            .unwrap_or_else(|| format!("{:.*}{}", self.decimals, v, self.suffix));
        p.text(
            egui::pos2(rect.right() - 6.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            value_text,
            font,
            TEXT_STRONG,
        );
        resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
    }
}

/// Everything `egui::Style`, in one pass over the context.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|s| {
        // CSP density: palettes are lists of small rows, ~40 visible at a
        // glance. Tight spacing, small type.
        s.spacing.item_spacing = egui::vec2(4.0, 3.0);
        s.spacing.button_padding = egui::vec2(5.0, 2.5);
        s.spacing.interact_size.y = 17.0;
        s.spacing.slider_width = 92.0;
        s.spacing.menu_margin = egui::Margin::same(4);

        use egui::FontFamily::Proportional;
        use egui::TextStyle::*;
        s.text_styles
            .insert(Body, egui::FontId::new(12.0, Proportional));
        s.text_styles
            .insert(Button, egui::FontId::new(12.0, Proportional));
        s.text_styles
            .insert(Small, egui::FontId::new(10.5, Proportional));
        s.text_styles
            .insert(Heading, egui::FontId::new(12.5, Proportional));

        let v = &mut s.visuals;
        v.panel_fill = WINDOW;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(R_PANEL + 2);
        v.window_shadow = egui::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        v.popup_shadow = egui::Shadow {
            offset: [0, 3],
            blur: 10,
            spread: 0,
            color: Color32::from_black_alpha(110),
        };
        v.extreme_bg_color = FIELD; // text edits, scroll wells
        v.faint_bg_color = HEADER; // striped rows
        v.selection.bg_fill = SEL_ROW;
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.override_text_color = Some(TEXT);

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = PANEL;
        w.noninteractive.weak_bg_fill = PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, OUTLINE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_WEAK);
        w.inactive.bg_fill = HOVER; // slider handles etc.
        w.inactive.weak_bg_fill = Color32::from_rgb(0x32, 0x32, 0x38); // buttons
        w.inactive.bg_stroke = Stroke::new(1.0, Color32::TRANSPARENT);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.bg_fill = ACTIVE;
        w.hovered.weak_bg_fill = HOVER;
        w.hovered.bg_stroke = Stroke::new(1.0, OUTLINE);
        w.hovered.fg_stroke = Stroke::new(1.2, TEXT_STRONG);
        w.active.bg_fill = ACCENT;
        w.active.weak_bg_fill = ACTIVE;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.2, TEXT_STRONG);
        w.open.bg_fill = ACTIVE;
        w.open.weak_bg_fill = ACTIVE;
        w.open.bg_stroke = Stroke::new(1.0, OUTLINE);
        w.open.fg_stroke = Stroke::new(1.0, TEXT_STRONG);
        for wv in [
            &mut w.noninteractive,
            &mut w.inactive,
            &mut w.hovered,
            &mut w.active,
            &mut w.open,
        ] {
            wv.corner_radius = CornerRadius::same(R_CTRL);
        }
    });
}
