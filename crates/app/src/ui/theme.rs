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
use std::sync::RwLock;

// --- design tokens -------------------------------------------------------

/// Every colour the chrome is allowed to use, in one `Copy` bundle.
///
/// These were seventeen bare `pub const`s until themes arrived. The switch to
/// a struct read through [`c()`] is what lets the whole app repaint in
/// another palette without threading a `&Theme` through several hundred
/// painting functions — immediate mode redraws everything every frame
/// anyway, so a global read is all a theme switch needs to be.
///
/// **Canvas-semantic colours are NOT tokens.** The overlay's selection
/// marching ants, guide lines and ruler marks are meaning, not decoration:
/// they stay literal `Color32` in `overlay.rs`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Theme {
    /// Window chrome: menu bar, status bar, and the gutters between palettes.
    pub window: Color32,
    /// Palette bodies.
    pub panel: Color32,
    /// Palette title strips (a step darker than the body, Rebelle-style).
    pub header: Color32,
    /// Inset controls: slider troughs, list wells, text edits, combo boxes.
    pub field: Color32,
    /// Hovered rows/buttons.
    pub hover: Color32,
    /// Pressed/open widgets.
    pub active: Color32,
    /// Raised button faces — a hair lighter than `panel`, so a button reads
    /// as a button before it is hovered.
    pub button: Color32,
    /// The accent — selection, active tool, filled slider bars.
    pub accent: Color32,
    /// Accent-tinted fill for value bars (quieter than raw accent).
    pub accent_fill: Color32,
    /// Accent-tinted selected-row background.
    pub sel_row: Color32,
    /// The Layers palette's *editing* row. CSP paints it an unmissable blue;
    /// ours sits at CSP's weight rather than the original glare.
    pub sel_active: Color32,
    /// The 1px top/bottom edge lines on that active row.
    pub sel_edge: Color32,
    /// 1px seams between regions.
    pub border: Color32,
    /// Subtle outline on raised controls.
    pub outline: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub text_strong: Color32,
    /// Something did not happen and the user needs to notice. The status bar
    /// paints refusals in this instead of `text_weak` — a grey line at the
    /// bottom of the window is indistinguishable from no feedback at all
    /// (owner, 2026-08-19: "dragging a .txt does not seem to do much" — it
    /// did, and it said so, in grey).
    pub warn: Color32,
    /// The layer status column's reference-lighthouse mark: red, so the
    /// column reads at a glance without being deciphered (owner 2026-08-21).
    pub ref_mark: Color32,
    /// See [`Theme::ref_mark`] — the draft pencil's blue.
    pub draft_mark: Color32,
    /// Auto Actions' armed dot. "Armed" reads as red everywhere, so this one
    /// barely moves between themes.
    pub rec: Color32,

    // --- icon accent hues (owner order 2026-08-21: coloured icons
    // everywhere, "subtle, theme-fitting", with a Preferences off-switch).
    // Seven semantic roles, low saturation, tuned against `panel`. Icons
    // AND the Scratch-style Auto Action block categories draw from the
    // same seven so the app reads as one system.
    /// Things that make something new: new-layer plus badges, add-folder.
    pub hue_create: Color32,
    /// Things that destroy: delete, clear, remove.
    pub hue_destroy: Color32,
    /// Files and media: import/export, save, open, image.
    pub hue_media: Color32,
    /// Marks that ink the page: pen, brush, fill, figure, tone.
    pub hue_ink: Color32,
    /// Selection-family tools and ops: lasso, wand, select pens.
    pub hue_select: Color32,
    /// Layer kinds and layer ops: folders, vector/tone/text layer glyphs.
    pub hue_layer: Color32,
    /// Viewing and navigation: zoom, hand, rotate, fit.
    pub hue_nav: Color32,
}

/// The default, and the palette every screenshot in `docs/` was taken in:
/// Photoshop's layered greys — chrome darkest, panel bodies a step lighter,
/// inset fields darker again — under a CSP-weight blue accent.
pub const DARK: Theme = Theme {
    window: Color32::from_rgb(0x1f, 0x1f, 0x22),
    panel: Color32::from_rgb(0x2a, 0x2a, 0x2e),
    header: Color32::from_rgb(0x24, 0x24, 0x27),
    field: Color32::from_rgb(0x1c, 0x1c, 0x1f),
    hover: Color32::from_rgb(0x35, 0x35, 0x3b),
    active: Color32::from_rgb(0x3e, 0x3e, 0x46),
    button: Color32::from_rgb(0x32, 0x32, 0x38),
    accent: Color32::from_rgb(0x4f, 0x8c, 0xd2),
    accent_fill: Color32::from_rgb(0x37, 0x5a, 0x84),
    sel_row: Color32::from_rgb(0x2e, 0x41, 0x59),
    sel_active: Color32::from_rgb(0x3c, 0x47, 0x59),
    sel_edge: Color32::from_rgb(0x5a, 0x6b, 0x86),
    border: Color32::from_rgb(0x15, 0x15, 0x17),
    outline: Color32::from_rgb(0x3c, 0x3c, 0x44),
    text: Color32::from_rgb(0xd4, 0xd4, 0xd8),
    text_weak: Color32::from_rgb(0x8e, 0x8e, 0x96),
    text_strong: Color32::from_rgb(0xf2, 0xf2, 0xf4),
    warn: Color32::from_rgb(0xe0, 0xa0, 0x4a),
    ref_mark: Color32::from_rgb(0xcf, 0x5d, 0x59),
    draft_mark: Color32::from_rgb(0x66, 0x9e, 0xd6),
    rec: Color32::from_rgb(0xe5, 0x4b, 0x4b),
    hue_create: Color32::from_rgb(0x6f, 0xae, 0x6f),
    hue_destroy: Color32::from_rgb(0xc9, 0x6a, 0x62),
    hue_media: Color32::from_rgb(0x6f, 0x9e, 0xc9),
    hue_ink: Color32::from_rgb(0xc9, 0xa0, 0x5e),
    hue_select: Color32::from_rgb(0x62, 0xb5, 0xae),
    hue_layer: Color32::from_rgb(0xa2, 0x88, 0xcf),
    hue_nav: Color32::from_rgb(0x8f, 0xa3, 0xb5),
};

/// The same greys pulled towards brown, under an amber accent — a warm
/// drawing room instead of a cold one. Long inking sessions on a bright
/// monitor are the case this is for.
pub const SEPIA: Theme = Theme {
    window: Color32::from_rgb(0x22, 0x1f, 0x1b),
    panel: Color32::from_rgb(0x2f, 0x2a, 0x24),
    header: Color32::from_rgb(0x28, 0x23, 0x1e),
    field: Color32::from_rgb(0x1e, 0x1b, 0x17),
    hover: Color32::from_rgb(0x3b, 0x35, 0x2d),
    active: Color32::from_rgb(0x46, 0x3f, 0x35),
    button: Color32::from_rgb(0x37, 0x31, 0x2a),
    accent: Color32::from_rgb(0xd0, 0x92, 0x4c),
    accent_fill: Color32::from_rgb(0x83, 0x5c, 0x30),
    sel_row: Color32::from_rgb(0x4a, 0x39, 0x25),
    sel_active: Color32::from_rgb(0x53, 0x43, 0x2c),
    sel_edge: Color32::from_rgb(0x83, 0x6c, 0x48),
    border: Color32::from_rgb(0x17, 0x14, 0x11),
    outline: Color32::from_rgb(0x44, 0x3d, 0x33),
    text: Color32::from_rgb(0xdf, 0xd7, 0xc9),
    text_weak: Color32::from_rgb(0x9a, 0x90, 0x82),
    text_strong: Color32::from_rgb(0xf6, 0xf0, 0xe4),
    // The accent is amber here, so the "that did not happen" colour has to
    // leave amber alone or refusals would read as ordinary chrome.
    warn: Color32::from_rgb(0xe8, 0x6a, 0x4c),
    ref_mark: Color32::from_rgb(0xd2, 0x66, 0x4f),
    draft_mark: Color32::from_rgb(0x74, 0x9d, 0xc0),
    rec: Color32::from_rgb(0xe5, 0x4b, 0x4b),
    // Cool hues muted a step further here — on brown chrome they pop more.
    hue_create: Color32::from_rgb(0x8a, 0xa6, 0x62),
    hue_destroy: Color32::from_rgb(0xc9, 0x6a, 0x4f),
    hue_media: Color32::from_rgb(0x7e, 0x97, 0xb3),
    hue_ink: Color32::from_rgb(0xcf, 0xa0, 0x50),
    hue_select: Color32::from_rgb(0x7f, 0xae, 0x9a),
    hue_layer: Color32::from_rgb(0xa9, 0x8f, 0xc0),
    hue_nav: Color32::from_rgb(0xa2, 0x9a, 0x8a),
};

/// Cool and dim: near-black plum chrome under a muted violet accent. The
/// darkest of the three — night work, and the one that makes the page glow
/// most.
pub const VIOLET: Theme = Theme {
    window: Color32::from_rgb(0x1c, 0x1a, 0x24),
    panel: Color32::from_rgb(0x26, 0x23, 0x30),
    header: Color32::from_rgb(0x20, 0x1d, 0x29),
    field: Color32::from_rgb(0x19, 0x16, 0x22),
    hover: Color32::from_rgb(0x32, 0x2e, 0x40),
    active: Color32::from_rgb(0x3b, 0x37, 0x4c),
    button: Color32::from_rgb(0x2e, 0x2a, 0x3a),
    accent: Color32::from_rgb(0x96, 0x7a, 0xdc),
    accent_fill: Color32::from_rgb(0x57, 0x46, 0x85),
    sel_row: Color32::from_rgb(0x36, 0x2e, 0x4f),
    sel_active: Color32::from_rgb(0x42, 0x3a, 0x5c),
    sel_edge: Color32::from_rgb(0x6d, 0x63, 0xa0),
    border: Color32::from_rgb(0x13, 0x11, 0x19),
    outline: Color32::from_rgb(0x3f, 0x3a, 0x52),
    text: Color32::from_rgb(0xd6, 0xd2, 0xe0),
    text_weak: Color32::from_rgb(0x91, 0x8d, 0xa0),
    text_strong: Color32::from_rgb(0xf2, 0xf0, 0xf8),
    warn: Color32::from_rgb(0xe0, 0xa0, 0x4a),
    ref_mark: Color32::from_rgb(0xd0, 0x60, 0x7e),
    draft_mark: Color32::from_rgb(0x7d, 0x8f, 0xe0),
    rec: Color32::from_rgb(0xe5, 0x4b, 0x4b),
    // hue_layer leans magenta here so it never reads as the violet accent.
    hue_create: Color32::from_rgb(0x7b, 0xb3, 0x83),
    hue_destroy: Color32::from_rgb(0xc9, 0x66, 0x7a),
    hue_media: Color32::from_rgb(0x7d, 0x9c, 0xe0),
    hue_ink: Color32::from_rgb(0xc3, 0x9a, 0x6a),
    hue_select: Color32::from_rgb(0x6a, 0xb3, 0xc0),
    hue_layer: Color32::from_rgb(0xb3, 0x89, 0xc9),
    hue_nav: Color32::from_rgb(0x8e, 0x93, 0xad),
};

/// The built-ins, in picker order. `dark` is first because it is the
/// default and the one every screenshot was taken in. A LIGHT theme is
/// deliberately absent: [`apply`]'s widget visuals are dark-tuned, and
/// shipping a light one without its own eye pass would ship a broken one.
pub const BUILT_INS: &[(&str, Theme)] = &[("dark", DARK), ("sepia", SEPIA), ("violet", VIOLET)];

/// An unknown name is the DEFAULT, never a panic and never a blank window:
/// `prefs.txt` is a text file people hand-edit, and a `theme=drak` typo must
/// cost a wrong colour scheme, not a start-up failure.
pub fn by_name(name: &str) -> Theme {
    if let Some((n, t)) = BUILT_INS.iter().find(|(n, _)| *n == name) {
        let _ = n;
        return *t;
    }
    // T1 step 3: a custom file beside the exe. A built-in name always
    // wins, so a hand-tweaked "dark.txt" cannot shadow the shipped dark.
    if let Some(dir) = themes_dir()
        && let Some(t) = load_custom(&dir, name)
    {
        return t;
    }
    DARK
}

/// The canonical name for what `name` actually resolves to — what the picker
/// shows as selected when `prefs.txt` holds a name this build never heard of.
pub fn resolved_name(name: &str) -> &'static str {
    BUILT_INS
        .iter()
        .find(|(n, _)| *n == name)
        .map_or("dark", |(n, _)| *n)
}

static ACTIVE: RwLock<Theme> = RwLock::new(DARK);

/// The live palette. `Theme` is `Copy` and this is a read lock, so calling it
/// per widget is fine; in a tight painting loop, read it once into a local.
pub fn c() -> Theme {
    // A poisoned lock means a panic while a theme was being swapped. The
    // right answer to that is still "paint something", so take the value
    // that is in there rather than propagating the panic into every frame.
    ACTIVE.read().map_or(DARK, |g| *g)
}

/// Swap the live palette. Callers that hold an `egui::Context` should follow
/// this with [`apply`], which is what pushes the new tokens into egui's own
/// widget visuals.
pub fn set(t: Theme) {
    if let Ok(mut g) = ACTIVE.write() {
        *g = t;
    }
}

/// `set(by_name(name))` — the `prefs.txt` door.
pub fn set_by_name(name: &str) {
    set(by_name(name));
}

// --- custom theme files (T1 step 3) --------------------------------------

/// Every colour token, in file/Editor order — see [`token_names`]. The
/// name is the `k` in `themes/<name>.txt`'s `k=RRGGBB` lines. Radii are
/// NOT themeable — rounding is this app's build quality, not a colour
/// scheme's opinion.
pub fn token_get(t: &Theme, k: &str) -> Option<Color32> {
    Some(match k {
        "window" => t.window,
        "panel" => t.panel,
        "header" => t.header,
        "field" => t.field,
        "hover" => t.hover,
        "active" => t.active,
        "button" => t.button,
        "accent" => t.accent,
        "accent_fill" => t.accent_fill,
        "sel_row" => t.sel_row,
        "sel_active" => t.sel_active,
        "sel_edge" => t.sel_edge,
        "border" => t.border,
        "outline" => t.outline,
        "text" => t.text,
        "text_weak" => t.text_weak,
        "text_strong" => t.text_strong,
        "warn" => t.warn,
        "ref_mark" => t.ref_mark,
        "draft_mark" => t.draft_mark,
        "rec" => t.rec,
        "hue_create" => t.hue_create,
        "hue_destroy" => t.hue_destroy,
        "hue_media" => t.hue_media,
        "hue_ink" => t.hue_ink,
        "hue_select" => t.hue_select,
        "hue_layer" => t.hue_layer,
        "hue_nav" => t.hue_nav,
        _ => return None,
    })
}

pub fn token_set(t: &mut Theme, k: &str, v: Color32) -> bool {
    match k {
        "window" => t.window = v,
        "panel" => t.panel = v,
        "header" => t.header = v,
        "field" => t.field = v,
        "hover" => t.hover = v,
        "active" => t.active = v,
        "button" => t.button = v,
        "accent" => t.accent = v,
        "accent_fill" => t.accent_fill = v,
        "sel_row" => t.sel_row = v,
        "sel_active" => t.sel_active = v,
        "sel_edge" => t.sel_edge = v,
        "border" => t.border = v,
        "outline" => t.outline = v,
        "text" => t.text = v,
        "text_weak" => t.text_weak = v,
        "text_strong" => t.text_strong = v,
        "warn" => t.warn = v,
        "ref_mark" => t.ref_mark = v,
        "draft_mark" => t.draft_mark = v,
        "rec" => t.rec = v,
        "hue_create" => t.hue_create = v,
        "hue_destroy" => t.hue_destroy = v,
        "hue_media" => t.hue_media = v,
        "hue_ink" => t.hue_ink = v,
        "hue_select" => t.hue_select = v,
        "hue_layer" => t.hue_layer = v,
        "hue_nav" => t.hue_nav = v,
        _ => return false,
    }
    true
}

/// All tokens, file order — the Editor's row list and the writer's line
/// order.
pub fn token_names() -> [&'static str; 28] {
    [
        "window",
        "panel",
        "header",
        "field",
        "hover",
        "active",
        "button",
        "accent",
        "accent_fill",
        "sel_row",
        "sel_active",
        "sel_edge",
        "border",
        "outline",
        "text",
        "text_weak",
        "text_strong",
        "warn",
        "ref_mark",
        "draft_mark",
        "rec",
        "hue_create",
        "hue_destroy",
        "hue_media",
        "hue_ink",
        "hue_select",
        "hue_layer",
        "hue_nav",
    ]
}

/// A theme as `k=RRGGBB` lines, one per token (a whole theme, not a diff —
/// a folder of files IS the share mechanism).
pub fn to_body(t: &Theme) -> String {
    let mut s = String::new();
    for k in token_names() {
        let c = token_get(t, k).unwrap();
        s.push_str(&format!("{k}={:02x}{:02x}{:02x}\n", c.r(), c.g(), c.b()));
    }
    s
}

/// Parse a theme file over a base: known tokens override, unknown lines are
/// PRESERVED verbatim (prefs.txt semantics — a newer build's tokens must
/// survive this one's rewrite), malformed hex ignored.
pub fn from_body(base: Theme, text: &str) -> (Theme, Vec<String>) {
    let mut t = base;
    let mut unknown = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            unknown.push(line.to_owned());
            continue;
        };
        let hex = v.trim().trim_start_matches('#');
        if hex.len() == 6
            && let Ok(b) = u32::from_str_radix(hex, 16)
            && token_set(
                &mut t,
                k.trim(),
                Color32::from_rgb((b >> 16) as u8, ((b >> 8) & 0xff) as u8, (b & 0xff) as u8),
            )
        {
            continue;
        }
        unknown.push(line.to_owned());
    }
    (t, unknown)
}

/// Where custom themes live: a `themes/` folder beside the exe (the same
/// resolution rule as prefs.txt).
pub fn themes_dir() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("themes")))
}

/// Custom theme names on disk, sorted (the picker lists built-ins first,
/// then these).
pub fn custom_names_in(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("txt"))
                && let Some(stem) = p.file_stem()
            {
                out.push(stem.to_string_lossy().into_owned());
            }
        }
    }
    out.sort();
    out
}

/// One custom theme over DARK; missing/unreadable is None (the caller falls
/// back to the built-in rules).
pub fn load_custom(dir: &std::path::Path, name: &str) -> Option<Theme> {
    let text = std::fs::read_to_string(dir.join(format!("{name}.txt"))).ok()?;
    let (t, _) = from_body(DARK, &text);
    Some(t)
}

/// Write a custom theme (creates the folder). Returns false only when the
/// disk refused.
pub fn save_custom(dir: &std::path::Path, name: &str, t: &Theme) -> bool {
    let _ = std::fs::create_dir_all(dir);
    std::fs::write(dir.join(format!("{name}.txt")), to_body(t)).is_ok()
}

/// Corner rounding: palettes 4, controls 2. Not themed — rounding is this
/// app's build quality, not a colour scheme's opinion.
pub const R_PANEL: u8 = 4;
pub const R_CTRL: u8 = 2;

// --- value bar -----------------------------------------------------------

/// The CSP property row (`csp/150_tools_0008.png`): a plain
/// "Label ......... value" text line with a THIN slider track underneath it.
/// Click or drag anywhere on the row to set the value.
///
/// It used to be a full-height accent-FILLED bar with the label inside, which
/// at 100% painted a solid blue row — "Opacity 100%" read as a *selected*
/// row rather than a slider at maximum, and Tool Property looked like two
/// highlighted rows on every launch (parity P0-2). The interaction is
/// unchanged; only the paint is.
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
        /// The slider track's thickness — CSP's is a hairline under the
        /// label line, not a bar the row is made of.
        const TRACK_H: f32 = 3.0;
        // The row keeps its old height: it is the hit area (drag anywhere),
        // and every consumer's list rhythm is measured against it.
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
        let hot = resp.hovered() || resp.dragged();
        let th = c();
        let p = ui.painter();
        // The row itself is not a control surface any more — only the hover
        // wash says "this is draggable".
        if hot {
            p.rect_filled(rect, CornerRadius::same(R_CTRL), th.hover);
        }
        // The track: a hairline strip along the bottom edge. Full width, so
        // the drag mapping (pointer x → t) is exactly the one the row
        // already had.
        let track = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - TRACK_H),
            rect.right_bottom(),
        );
        let cr = CornerRadius::same(1);
        // The GROOVE is painted first and always, across the whole width —
        // CSP never hides it, and a zeroed parameter ("Stabilize 0%", "In
        // 0 px") has no fill at all, so the groove is the only thing saying
        // the row is draggable (parity M1). It is `outline`, not the `field`
        // trough grey: three pixels of 0x1c on a 0x2a panel is a black
        // hairline flush with the row below it, which reads as a seam, not a
        // control — an empty bar looked like static text.
        p.rect_filled(track, cr, th.outline);
        let t = self.to_t(*v);
        let fill_w = t * track.width();
        if fill_w > 0.5 {
            let fill = if hot { th.accent } else { th.accent_fill };
            let clip = egui::Rect::from_min_max(
                track.min,
                egui::pos2(track.left() + fill_w, track.bottom()),
            );
            p.with_clip_rect(clip).rect_filled(track, cr, fill);
        }
        // Label left, value right, both on the text line ABOVE the track.
        let text_y = rect.top() + (rect.height() - TRACK_H) * 0.5;
        let font = egui::FontId::proportional(11.0);
        p.text(
            egui::pos2(rect.left() + 4.0, text_y),
            egui::Align2::LEFT_CENTER,
            self.label,
            font.clone(),
            th.text,
        );
        let value_text = self
            .display
            .unwrap_or_else(|| format!("{:.*}{}", self.decimals, v, self.suffix));
        p.text(
            egui::pos2(rect.right() - 4.0, text_y),
            egui::Align2::RIGHT_CENTER,
            value_text,
            font,
            th.text_strong,
        );
        resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
    }
}

/// Everything `egui::Style`, in one pass over the context. Call it again
/// after [`set`] — this is what a live theme switch costs.
pub fn apply(ctx: &egui::Context) {
    let t = c();
    // Every built-in is a DARK theme (see [`BUILT_INS`]): egui's own dark
    // preference is the right base for all of them, and the tokens below
    // then overwrite everything that shows.
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
        v.panel_fill = t.window;
        v.window_fill = t.panel;
        v.window_stroke = Stroke::new(1.0, t.border);
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
        v.extreme_bg_color = t.field; // text edits, scroll wells
        v.faint_bg_color = t.header; // striped rows
        v.selection.bg_fill = t.sel_row;
        v.selection.stroke = Stroke::new(1.0, t.accent);
        v.hyperlink_color = t.accent;
        v.override_text_color = Some(t.text);

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = t.panel;
        w.noninteractive.weak_bg_fill = t.panel;
        w.noninteractive.bg_stroke = Stroke::new(1.0, t.outline);
        w.noninteractive.fg_stroke = Stroke::new(1.0, t.text_weak);
        w.inactive.bg_fill = t.hover; // slider handles etc.
        w.inactive.weak_bg_fill = t.button; // buttons
        w.inactive.bg_stroke = Stroke::new(1.0, Color32::TRANSPARENT);
        w.inactive.fg_stroke = Stroke::new(1.0, t.text);
        w.hovered.bg_fill = t.active;
        w.hovered.weak_bg_fill = t.hover;
        w.hovered.bg_stroke = Stroke::new(1.0, t.outline);
        w.hovered.fg_stroke = Stroke::new(1.2, t.text_strong);
        w.active.bg_fill = t.accent;
        w.active.weak_bg_fill = t.active;
        w.active.bg_stroke = Stroke::new(1.0, t.accent);
        w.active.fg_stroke = Stroke::new(1.2, t.text_strong);
        w.open.bg_fill = t.active;
        w.open.weak_bg_fill = t.active;
        w.open.bg_stroke = Stroke::new(1.0, t.outline);
        w.open.fg_stroke = Stroke::new(1.0, t.text_strong);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The load-bearing test of the themes round: `dark` is the seventeen
    /// constants this file shipped before `Theme` existed, to the byte.
    ///
    /// Spelled out as literals ON PURPOSE — asserting `DARK.accent ==
    /// DARK.accent` would pass no matter what drifted. If one of these ever
    /// fails, the refactor that was supposed to change nothing changed the
    /// look of every screenshot in `docs/`, and the fix is the token, not
    /// this test.
    #[test]
    fn dark_is_byte_identical_to_the_pre_theme_constants() {
        let rgb = |c: Color32| (c.r(), c.g(), c.b());
        let t = DARK;
        assert_eq!(rgb(t.window), (0x1f, 0x1f, 0x22), "WINDOW");
        assert_eq!(rgb(t.panel), (0x2a, 0x2a, 0x2e), "PANEL");
        assert_eq!(rgb(t.header), (0x24, 0x24, 0x27), "HEADER");
        assert_eq!(rgb(t.field), (0x1c, 0x1c, 0x1f), "FIELD");
        assert_eq!(rgb(t.hover), (0x35, 0x35, 0x3b), "HOVER");
        assert_eq!(rgb(t.active), (0x3e, 0x3e, 0x46), "ACTIVE");
        assert_eq!(rgb(t.accent), (0x4f, 0x8c, 0xd2), "ACCENT");
        assert_eq!(rgb(t.accent_fill), (0x37, 0x5a, 0x84), "ACCENT_FILL");
        assert_eq!(rgb(t.sel_row), (0x2e, 0x41, 0x59), "SEL_ROW");
        assert_eq!(rgb(t.border), (0x15, 0x15, 0x17), "BORDER");
        assert_eq!(rgb(t.outline), (0x3c, 0x3c, 0x44), "OUTLINE");
        assert_eq!(rgb(t.text), (0xd4, 0xd4, 0xd8), "TEXT");
        assert_eq!(rgb(t.text_weak), (0x8e, 0x8e, 0x96), "TEXT_WEAK");
        assert_eq!(rgb(t.text_strong), (0xf2, 0xf2, 0xf4), "TEXT_STRONG");
        assert_eq!(rgb(t.warn), (0xe0, 0xa0, 0x4a), "WARN");
        // The five shadow tokens folded in from `layers.rs` and `actions.rs`,
        // which were private consts rather than `theme::` constants.
        assert_eq!(rgb(t.ref_mark), (0xcf, 0x5d, 0x59), "layers.rs REF_MARK");
        assert_eq!(
            rgb(t.draft_mark),
            (0x66, 0x9e, 0xd6),
            "layers.rs DRAFT_MARK"
        );
        assert_eq!(
            rgb(t.sel_active),
            (0x3c, 0x47, 0x59),
            "layers.rs SEL_ACTIVE"
        );
        assert_eq!(rgb(t.sel_edge), (0x5a, 0x6b, 0x86), "layers.rs SEL_EDGE");
        assert_eq!(rgb(t.rec), (0xe5, 0x4b, 0x4b), "actions.rs REC");
        // The button face was a bare literal inside `apply` itself.
        assert_eq!(rgb(t.button), (0x32, 0x32, 0x38), "apply's button fill");
        assert_eq!((R_PANEL, R_CTRL), (4, 2), "the radii are not themed");
    }

    /// A name from a newer build, or a typo in a hand-edited `prefs.txt`,
    /// must cost a wrong colour scheme — never a panic and never a start-up
    /// that dies before the window appears.
    #[test]
    fn an_unknown_theme_name_is_dark() {
        assert_eq!(by_name("drak"), DARK);
        assert_eq!(by_name(""), DARK);
        assert_eq!(by_name("light"), DARK, "there is no light theme yet");
        assert_eq!(resolved_name("whatever a 2027 build called it"), "dark");
        assert_eq!(resolved_name("violet"), "violet");
    }

    /// Every built-in must actually be a distinct, complete theme: a variant
    /// that forgot to move a token would ship as `dark` with two colours
    /// changed, which is worse than not shipping it.
    #[test]
    fn the_built_ins_are_named_distinct_and_dark() {
        assert_eq!(BUILT_INS[0].0, "dark", "the default is first in the picker");
        for (name, t) in BUILT_INS {
            assert_eq!(by_name(name), *t, "{name} must be reachable by its name");
            // Dark-tuned widget visuals (see `apply`): the panel body has to
            // stay darker than the text on it, in every one of them.
            let lum = |c: Color32| c.r() as u32 + c.g() as u32 + c.b() as u32;
            assert!(lum(t.panel) < lum(t.text), "{name}: panel is not dark");
            assert!(
                lum(t.window) < lum(t.panel),
                "{name}: chrome is not the darkest surface"
            );
            // [`ValueBar`] paints its groove in `outline` on a `panel` body,
            // and an empty bar is groove ONLY. If a future palette lets the
            // two converge, every zeroed slider in Tool Property goes back to
            // looking like a line of static text (parity M1).
            assert!(
                lum(t.outline) >= lum(t.panel) + 24,
                "{name}: the ValueBar groove does not read against the panel"
            );
        }
        assert_ne!(SEPIA, DARK);
        assert_ne!(VIOLET, DARK);
        assert_ne!(SEPIA, VIOLET);
    }

    /// `set` then `c` is the whole switching mechanism. Restores `dark`,
    /// because the global outlives the test.
    #[test]
    fn setting_a_theme_changes_what_c_returns() {
        set_by_name("sepia");
        assert_eq!(c(), SEPIA);
        set_by_name("nonsense");
        assert_eq!(c(), DARK, "an unknown name falls back rather than sticking");
        set(DARK);
    }

    /// T1 step 3: a theme file round-trips EVERY token — a saved custom is
    /// byte-faithful to what the editor showed, unknown lines survive a
    /// load-and-rewrite (prefs.txt semantics), and a custom file resolves
    /// by name while a built-in name always wins.
    #[test]
    fn theme_files_round_trip_every_token_and_keep_unknown_lines() {
        // A modified DARK: every token nudged, so a lost one is visible.
        let mut t = DARK;
        for (i, k) in token_names().iter().enumerate() {
            let base = token_get(&t, k).unwrap();
            let nudge = egui::Color32::from_rgb(
                (base.r() ^ (i as u8)).max(1),
                (base.g() ^ (i as u8).wrapping_mul(3)).max(1),
                (base.b() ^ (i as u8).wrapping_mul(7)).max(1),
            );
            assert!(token_set(&mut t, k, nudge), "{k} is a real token");
        }
        let body = to_body(&t);
        let (back, unknown) = from_body(DARK, &format!("# a comment\n{body}"));
        assert!(unknown == vec!["# a comment".to_owned()], "{unknown:?}");
        for k in token_names() {
            assert_eq!(
                token_get(&back, k),
                token_get(&t, k),
                "{k} survived the round trip"
            );
        }
        // Malformed lines are kept, not applied.
        let (_, unknown) = from_body(DARK, "accent=xyz\nnot-a-pair\n");
        assert_eq!(unknown.len(), 2, "garbage preserved verbatim");
    }

    /// The custom-file life cycle in a scratch dir: save, scan, load, and
    /// the built-in shadowing rule.
    #[test]
    fn custom_theme_files_save_scan_and_load() {
        let dir = std::env::temp_dir().join(format!("mn-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut t = SEPIA;
        assert!(token_set(
            &mut t,
            "accent",
            egui::Color32::from_rgb(1, 2, 3)
        ));
        assert!(save_custom(&dir, "mine", &t));
        assert_eq!(custom_names_in(&dir), vec!["mine".to_owned()]);
        let got = load_custom(&dir, "mine").expect("the file loads");
        assert_eq!(
            token_get(&got, "accent"),
            Some(egui::Color32::from_rgb(1, 2, 3))
        );
        assert_eq!(
            token_get(&got, "panel"),
            Some(SEPIA.panel),
            "a saved file carries the WHOLE theme — every token came along"
        );
        // A hand-written PARTIAL file: named tokens apply, the rest are
        // the DARK base.
        std::fs::write(dir.join("partial.txt"), "accent=010203\n").unwrap();
        let part = load_custom(&dir, "partial").expect("partial loads");
        assert_eq!(
            token_get(&part, "accent"),
            Some(egui::Color32::from_rgb(1, 2, 3))
        );
        assert_eq!(
            token_get(&part, "panel"),
            Some(DARK.panel),
            "the base fills the gaps"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
