//! Colour UI: the Photoshop-style hue ring + SV square picker, the Color Set
//! grid with its named sets, the main/sub/transparent slots, and the HSV
//! helpers. `picker_sync` runs at the top of every frame (build) before
//! color_section reads the state.
//!
//! Three pieces of state live in `ui.data` rather than in `App`, because none
//! of them is part of a document and none belongs in an undo history: the
//! wheel's press-time part latch, which colour space the one-line readout is
//! showing, and the Color Set's selected swatch + search box. The named sets
//! themselves are cached there too — see [`SetStore`].

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
        let (tr, tr_resp) =
            ui.allocate_exact_size(egui::vec2(CHIP_SMALL, CHIP_SMALL), egui::Sense::click());
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
        egui::Stroke::new(2.0, theme::c().accent)
    } else if hovered {
        egui::Stroke::new(1.0, theme::c().text_strong)
    } else {
        egui::Stroke::new(1.0, theme::c().outline)
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
    // ONE tight run of controls under it — the single readout line, colour
    // chips, recent — with the leftover height left as clean panel. The rows
    // used to drift apart on the frame's 3pt rhythm plus their own add_space,
    // which with the full-width slots made the lower half read as unfinished
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
    p.circle_stroke(centre, r_out, egui::Stroke::new(1.0, theme::c().border));
    p.circle_stroke(centre, r_in, egui::Stroke::new(1.0, theme::c().border));

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
        egui::Stroke::new(1.0, theme::c().border),
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

    ui.add_space(5.0);
    readout_row(ui, app);
    ui.add_space(5.0);
    color_slots(ui, app);
    history_strip(ui, app);
}

/// Which numbers the one line under the wheel is showing. CSP spends ONE row
/// on `H 343 S 69 V 76` and a button at the palette's bottom-right that
/// cycles the colour space; ours used to stack an R/G/B row AND a hex row
/// always-on, which cost two rows and still had no HSV in it (parity audit
/// M6, 2026-08-22). The line IS the editor in every mode, so nothing became
/// unreachable — hex is one click further away, not gone.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Readout {
    #[default]
    Hsv,
    Rgb,
    Hex,
}

impl Readout {
    fn next(self) -> Self {
        match self {
            Self::Hsv => Self::Rgb,
            Self::Rgb => Self::Hex,
            Self::Hex => Self::Hsv,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Hsv => "HSV",
            Self::Rgb => "RGB",
            Self::Hex => "Hex",
        }
    }
}

fn readout_row(ui: &mut egui::Ui, app: &mut App) {
    let id = ui.id().with("mn.readout");
    let mode = ui.data_mut(|d| *d.get_temp_mut_or_default::<Readout>(id));
    ui.horizontal(|ui| {
        const BTN: f32 = 34.0;
        let w = (ui.available_width() - BTN - 8.0).max(46.0);
        match mode {
            Readout::Hsv => hsv_fields(ui, app, w),
            Readout::Rgb => rgb_fields(ui, app, w),
            Readout::Hex => hex_field(ui, app, w),
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let btn = egui::Button::new(
                egui::RichText::new(mode.label())
                    .size(9.5)
                    .color(theme::c().text_weak),
            )
            .min_size(egui::vec2(BTN, 16.0));
            if ui
                .add(btn)
                .on_hover_text("Colour space — click to cycle HSV → RGB → hex")
                .clicked()
            {
                ui.data_mut(|d| d.insert_temp(id, mode.next()));
            }
        });
    });
}

/// The width of one of three side-by-side value fields in `w` points of row,
/// allowing for each field's label and the gaps. The old 26pt floor
/// overflowed its labels onto the numbers in narrow panes.
fn field_w(w: f32) -> f32 {
    ((w - 3.0 * 6.0 - 3.0 * 8.0) / 3.0).clamp(18.0, 44.0)
}

fn dim(label: &str) -> egui::RichText {
    egui::RichText::new(label)
        .size(10.5)
        .color(theme::c().text_weak)
}

/// Push an edited colour: a held spinner is live, the release (which reports
/// no change of its own) is the commit — the same rule the wheel drag uses,
/// so one sweep leaves one entry in Recent rather than sixty.
fn push_edit(app: &mut App, rgb: [f32; 3], live: bool) {
    app.push_cmd(if live {
        AppCmd::SetSlotColorLive(rgb)
    } else {
        AppCmd::SetSlotColor(rgb)
    });
}

/// `H 343 S 69 V 76` — CSP's degrees and percents, editable in place. Reads
/// the PANEL's hue/saturation rather than re-deriving them from RGB, so the
/// numbers do not jump to zero when the colour passes through black or grey.
fn hsv_fields(ui: &mut egui::Ui, app: &mut App, w: f32) {
    let hsv = app.picker_hsv;
    let mut vals = [
        (hsv[0] * 360.0).round(),
        (hsv[1] * 100.0).round(),
        (hsv[2] * 100.0).round(),
    ];
    let (mut edited, mut live) = (false, false);
    let dw = field_w(w);
    for (i, label) in ["H", "S", "V"].iter().enumerate() {
        ui.label(dim(label));
        let max = if i == 0 { 359.0 } else { 100.0 };
        let r = ui.add_sized(
            [dw, 16.0],
            egui::DragValue::new(&mut vals[i])
                .range(0.0..=max)
                .speed(1.0),
        );
        edited |= r.changed() || r.drag_stopped();
        live |= r.dragged();
    }
    if edited {
        let hsv = [vals[0] / 360.0, vals[1] / 100.0, vals[2] / 100.0];
        app.picker_hsv = hsv;
        let rgb = hsv_to_rgb(hsv);
        app.picker_rgb_cache = rgb;
        push_edit(app, rgb, live);
    }
}

fn rgb_fields(ui: &mut egui::Ui, app: &mut App, w: f32) {
    let rgb = app.active_color();
    let mut vals = rgb.map(|c| (c * 255.0).round());
    let (mut edited, mut live) = (false, false);
    let dw = field_w(w);
    for (i, label) in ["R", "G", "B"].iter().enumerate() {
        ui.label(dim(label));
        let r = ui.add_sized(
            [dw, 16.0],
            egui::DragValue::new(&mut vals[i])
                .range(0.0..=255.0)
                .speed(1.0),
        );
        edited |= r.changed() || r.drag_stopped();
        live |= r.dragged();
    }
    if edited {
        push_edit(app, vals.map(|v| v / 255.0), live);
    }
}

/// CO-064: the field every palette on the internet is quoted in. Shows the
/// active colour as `rrggbb` (the `#` is the label, so it cannot be deleted
/// by accident) and accepts `#rrggbb`, `rrggbb`, `#rgb` or `rgb` on Enter
/// or on clicking away. **Text we cannot read exactly reverts** — clamping
/// `#ff00` to some nearby colour would be a guess, and a guess in a colour
/// field is worse than doing nothing.
fn hex_field(ui: &mut egui::Ui, app: &mut App, w: f32) {
    let current = mn_core::palette::hex_string(app.active_color())[1..].to_owned();
    let id = ui.id().with("mn.hex");
    // Outside an edit the field mirrors the colour, so the wheel drives it.
    if !ui.memory(|m| m.has_focus(id)) {
        app.hex_edit = current.clone();
    }
    ui.label(dim("#"));
    let done = ui
        .add_sized(
            [(w - 12.0).clamp(46.0, 88.0), 16.0],
            egui::TextEdit::singleline(&mut app.hex_edit)
                .id(id)
                .char_limit(7)
                .horizontal_align(egui::Align::Center),
        )
        .on_hover_text("Hex colour — #rrggbb, rrggbb or the 3-digit short form")
        .lost_focus();
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
                .color(theme::c().text_weak),
        );
        for rgb in &app.color_history {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, 2.0, rgb32(*rgb));
            let stroke = if resp.hovered() {
                egui::Stroke::new(1.5, theme::c().text_strong)
            } else {
                egui::Stroke::new(1.0, theme::c().outline)
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

/// One named Color Set. CSP's palette opens on a `[Standard color set ▾]`
/// combo and keeps as many sets as you make; ours had exactly one, implicit
/// and unnamed (parity audit M7, 2026-08-22).
#[derive(Clone, Debug, Default, PartialEq)]
struct NamedSet {
    name: String,
    /// EMPTY for the active set — see [`SetStore`].
    colors: Vec<mn_core::palette::Swatch>,
}

/// The named sets, as `color_sets.txt` beside `swatches.txt` holds them.
///
/// The active set's colours are deliberately NOT in this file. They stay in
/// `swatches.txt`, which is still the file `App` loads at boot and
/// `save_swatches` writes on every add/delete/import, so the two files can
/// never disagree about what is on screen. This one holds the set NAMES in
/// combo order, which of them is active, and the bodies of the sets that are
/// currently put away. Switching sets is therefore a swap: the live grid goes
/// into the old set's body, the new set's body comes out and is written to
/// `swatches.txt`.
///
/// ```text
/// current=Skin
/// [Standard]
/// #000000
/// #ffffff	Paper
/// [Skin]
/// ```
///
/// Junk lines are skipped and unrecognised `key=value` lines are kept
/// verbatim and written back, so a file touched by a newer build (or by hand)
/// survives a round trip through this one instead of quietly losing whatever
/// it did not understand.
#[derive(Clone, Debug, PartialEq)]
struct SetStore {
    sets: Vec<NamedSet>,
    current: usize,
    extra: Vec<String>,
}

impl Default for SetStore {
    /// One set called Standard: what every install that has never seen this
    /// file has, including the one that just seeded `swatches.txt` from the
    /// built-in CSP standard colours.
    fn default() -> Self {
        Self {
            sets: vec![NamedSet {
                name: "Standard".into(),
                colors: Vec::new(),
            }],
            current: 0,
            extra: Vec::new(),
        }
    }
}

impl SetStore {
    fn names(&self) -> Vec<String> {
        self.sets.iter().map(|s| s.name.clone()).collect()
    }

    /// Make set `i` the live one, swapping the grid's colours for its own.
    fn activate(&mut self, live: &mut Vec<mn_core::palette::Swatch>, i: usize) {
        if i >= self.sets.len() || i == self.current {
            return;
        }
        let cur = self.current;
        self.sets[cur].colors = std::mem::take(live);
        *live = std::mem::take(&mut self.sets[i].colors);
        self.current = i;
    }

    /// A new, empty set, named so it cannot collide, and made live.
    fn add(&mut self, live: &mut Vec<mn_core::palette::Swatch>) {
        let name = (2..)
            .map(|n| format!("Color set {n}"))
            .find(|n| !self.sets.iter().any(|s| &s.name == n))
            .unwrap_or_else(|| "Color set".to_owned());
        let cur = self.current;
        self.sets[cur].colors = std::mem::take(live);
        self.sets.push(NamedSet {
            name,
            colors: Vec::new(),
        });
        self.current = self.sets.len() - 1;
    }

    /// Drop the live set and fall to the one before it. Refused for the last
    /// set — a Color Set palette with no set in it has nothing to show.
    fn remove_current(&mut self, live: &mut Vec<mn_core::palette::Swatch>) {
        if self.sets.len() < 2 {
            return;
        }
        self.sets.remove(self.current);
        self.current = self.current.min(self.sets.len() - 1);
        *live = std::mem::take(&mut self.sets[self.current].colors);
    }
}

/// One `color_sets.txt` colour line: `#rrggbb`, or `#rrggbb<TAB>Name`, the
/// same shape `swatches.txt` uses so a set body can be pasted between them.
fn parse_set_line(l: &str) -> Option<mn_core::palette::Swatch> {
    let l = l.trim();
    let (hex, name) = match l.split_once(char::is_whitespace) {
        Some((h, n)) => (h, n.trim()),
        None => (l, ""),
    };
    Some(mn_core::palette::Swatch {
        rgb: mn_core::palette::parse_hex(hex)?,
        name: name.to_owned(),
    })
}

fn parse_sets(text: &str) -> SetStore {
    let mut sets: Vec<NamedSet> = Vec::new();
    let mut current_name = String::new();
    let mut extra = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if let Some(name) = l.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            sets.push(NamedSet {
                name: name.trim().to_owned(),
                colors: Vec::new(),
            });
        } else if let Some(sw) = parse_set_line(l) {
            match sets.last_mut() {
                Some(s) => s.colors.push(sw),
                None => continue,
            }
        } else if let Some((k, v)) = l.split_once('=') {
            match k.trim() {
                "current" => current_name = v.trim().to_owned(),
                _ => extra.push(l.to_owned()),
            }
        }
    }
    if sets.is_empty() {
        return SetStore {
            extra,
            ..SetStore::default()
        };
    }
    let current = sets
        .iter()
        .position(|s| s.name == current_name)
        .unwrap_or(0);
    // The live set's body is `swatches.txt`; whatever this file said about it
    // is stale by definition, so it is dropped rather than trusted.
    sets[current].colors.clear();
    SetStore {
        sets,
        current,
        extra,
    }
}

/// A name with the separators taken out — a `]` or a newline inside one would
/// turn a single set into two halves of junk.
fn clean_name(name: &str) -> String {
    let n = name.replace(['\n', '\r', '\t', '[', ']'], " ");
    match n.trim() {
        "" => "Color set".to_owned(),
        t => t.to_owned(),
    }
}

/// The `color_sets.txt` text. Split from the write so the format round-trips
/// under test without touching the disk.
fn sets_body(store: &SetStore) -> String {
    let mut out = String::new();
    if let Some(s) = store.sets.get(store.current) {
        out.push_str(&format!("current={}\n", clean_name(&s.name)));
    }
    for line in &store.extra {
        out.push_str(line);
        out.push('\n');
    }
    for (i, s) in store.sets.iter().enumerate() {
        out.push_str(&format!("[{}]\n", clean_name(&s.name)));
        if i == store.current {
            continue;
        }
        for c in &s.colors {
            let hex = mn_core::palette::hex_string(c.rgb);
            let name = c.name.replace(['\n', '\r', '\t'], " ");
            match name.trim() {
                "" => out.push_str(&format!("{hex}\n")),
                n => out.push_str(&format!("{hex}\t{n}\n")),
            }
        }
    }
    out
}

fn sets_path() -> Option<std::path::PathBuf> {
    Some(
        std::env::current_exe()
            .ok()?
            .parent()?
            .join("color_sets.txt"),
    )
}

fn load_sets() -> SetStore {
    sets_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse_sets(&t))
        .unwrap_or_default()
}

fn save_sets(store: &SetStore) {
    if let Some(p) = sets_path() {
        let _ = std::fs::write(p, sets_body(store));
    }
}

/// What the set combo asked for, applied after the row is built (it needs the
/// swatch list the row is borrowing).
#[derive(Clone, Copy)]
enum SetAct {
    Switch(usize),
    New,
    Delete,
}

/// A foot button: CSP puts add / replace / delete under the grid, acting on
/// the swatch you have selected.
fn foot_btn(ui: &mut egui::Ui, label: &str, enabled: bool, tip: &str) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).size(10.5)).min_size(egui::vec2(0.0, 17.0)),
    )
    .on_hover_text(tip)
}

pub(super) fn swatch_grid(ui: &mut egui::Ui, app: &mut App) {
    let store_id = ui.id().with("mn.sets");
    let query_id = ui.id().with("mn.sw.query");
    let sel_id = ui.id().with("mn.sw.sel");
    let (names, cur) = ui.data_mut(|d| {
        let s = d.get_temp_mut_or_insert_with(store_id, load_sets);
        (s.names(), s.current)
    });

    // Header: the set combo, then the search box (CSP's own header row).
    let mut query = ui
        .data_mut(|d| d.get_temp::<String>(query_id))
        .unwrap_or_default();
    let mut act = None;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("mn.colorset.name")
            .width((ui.available_width() - 84.0).clamp(70.0, 150.0))
            .selected_text(
                egui::RichText::new(names.get(cur).map(String::as_str).unwrap_or("Color set"))
                    .size(10.5),
            )
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    if ui
                        .selectable_label(i == cur, egui::RichText::new(n).size(10.5))
                        .clicked()
                    {
                        act = Some(SetAct::Switch(i));
                    }
                }
                ui.separator();
                if ui
                    .button(egui::RichText::new("New colour set").size(10.5))
                    .clicked()
                {
                    act = Some(SetAct::New);
                }
                if ui
                    .add_enabled(
                        names.len() > 1,
                        egui::Button::new(egui::RichText::new("Delete this colour set").size(10.5)),
                    )
                    .on_hover_text("The colours in it go with it; the other sets are untouched")
                    .clicked()
                {
                    act = Some(SetAct::Delete);
                }
            });
        if ui
            .add(
                egui::TextEdit::singleline(&mut query)
                    .hint_text(dim("Search"))
                    .desired_width((ui.available_width() - 2.0).max(40.0)),
            )
            .on_hover_text("Show only swatches whose name or hex contains this")
            .changed()
        {
            ui.data_mut(|d| d.insert_temp(query_id, query.clone()));
        }
    });
    if let Some(a) = act {
        ui.data_mut(|d| {
            let s = d.get_temp_mut_or_insert_with(store_id, load_sets);
            match a {
                SetAct::Switch(i) => s.activate(&mut app.swatches, i),
                SetAct::New => s.add(&mut app.swatches),
                SetAct::Delete => s.remove_current(&mut app.swatches),
            }
            save_sets(s);
            // A swatch index means nothing in a set you just left.
            d.insert_temp(sel_id, usize::MAX);
        });
        crate::app::save_swatches(&app.swatches);
    }

    let mut sel = ui
        .data_mut(|d| d.get_temp::<usize>(sel_id))
        .unwrap_or(usize::MAX);
    if sel >= app.swatches.len() {
        sel = usize::MAX;
    }
    let q = query.trim().to_lowercase();
    let live = mn_core::palette::quantize8(app.active_color());
    let size = 17.0;
    let mut pick = None;
    let mut del = None;
    let mut new_sel = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
        for (i, sw) in app.swatches.iter().enumerate() {
            // An imported palette's own name for the colour, when it has
            // one; the hex otherwise, which is what you would look up — and
            // both are what the search box matches against.
            let hex = mn_core::palette::hex_string(sw.rgb);
            if !q.is_empty() && !sw.name.to_lowercase().contains(&q) && !hex.contains(&q) {
                continue;
            }
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, 2.0, rgb32(sw.rgb));
            let stroke = if i == sel {
                egui::Stroke::new(2.0, theme::c().accent)
            } else if resp.hovered() {
                egui::Stroke::new(1.5, theme::c().text_strong)
            } else if sw.rgb == live {
                // The colour you are drawing with, wherever it sits.
                egui::Stroke::new(1.5, theme::c().accent)
            } else {
                egui::Stroke::new(1.0, theme::c().outline)
            };
            p.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
            let resp = resp.on_hover_text(if sw.name.is_empty() {
                hex
            } else {
                format!("{}  ({hex})", sw.name)
            });
            if resp.clicked() {
                pick = Some(sw.rgb);
                new_sel = Some(i);
            }
            resp.context_menu(|ui| {
                if ui.button("Delete swatch").clicked() {
                    del = Some(i);
                    ui.close();
                }
            });
        }
    });

    // Foot: what CSP puts under its grid, on the selected swatch.
    ui.add_space(3.0);
    let mut replace = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if foot_btn(ui, "Add", true, "Add the current colour as a new swatch").clicked() {
            app.push_cmd(AppCmd::AddSwatch(app.active_color()));
        }
        if foot_btn(
            ui,
            "Replace",
            sel != usize::MAX,
            "Put the current colour in the selected swatch (click a swatch to select it)",
        )
        .clicked()
        {
            replace = true;
        }
        if foot_btn(
            ui,
            "Delete",
            sel != usize::MAX,
            "Delete the selected swatch",
        )
        .clicked()
        {
            del = Some(sel);
        }
        // Import a GIMP/Krita .gpl palette (appended to this set, persisted).
        if icon_btn(
            ui,
            Icon::Folder,
            17.0,
            false,
            true,
            "Import palette (.gpl)…",
        )
        .clicked()
        {
            app.push_cmd(AppCmd::ImportPalette);
        }
    });
    if replace && sel < app.swatches.len() {
        // The name came from the palette file that supplied the OLD colour,
        // so it goes with it rather than mislabelling the new one.
        app.swatches[sel] = mn_core::palette::Swatch::new(app.active_color());
        crate::app::save_swatches(&app.swatches);
    }

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
    if let Some(i) = new_sel {
        ui.data_mut(|d| d.insert_temp(sel_id, i));
    }
    if let Some(rgb) = pick {
        app.push_cmd(AppCmd::SetSlotColor(rgb));
    }
    if let Some(i) = del {
        app.push_cmd(AppCmd::DeleteSwatch(i));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mn_core::palette::Swatch;

    fn sw(hex: &str) -> Swatch {
        Swatch::new(mn_core::palette::parse_hex(hex).expect("test hex"))
    }

    /// The readout button walks all three colour spaces and comes home, so
    /// no mode is a one-way door.
    #[test]
    fn readout_cycles_through_every_colour_space() {
        let m = Readout::default();
        assert_eq!(m, Readout::Hsv);
        assert_eq!(m.next(), Readout::Rgb);
        assert_eq!(m.next().next(), Readout::Hex);
        assert_eq!(m.next().next().next(), Readout::Hsv);
    }

    /// `color_sets.txt` round-trips names, order, bodies and which set is
    /// live — and the live set's body is NOT in it, because `swatches.txt`
    /// holds that and two copies would eventually disagree.
    #[test]
    fn color_sets_file_round_trips() {
        let mut store = SetStore::default();
        let mut live = vec![sw("#ff0000"), sw("#00ff00")];
        store.add(&mut live); // "Color set 2", now live and empty
        assert_eq!(store.names(), ["Standard", "Color set 2"]);
        assert!(live.is_empty(), "a new set starts empty");
        live.push(Swatch {
            rgb: mn_core::palette::parse_hex("#123456").expect("hex"),
            name: "Ink".into(),
        });

        let body = sets_body(&store);
        assert!(body.starts_with("current=Color set 2\n"), "{body}");
        assert!(body.contains("[Standard]\n#ff0000\n#00ff00\n"), "{body}");
        assert!(
            !body.contains("#123456"),
            "the live set's colours live in swatches.txt: {body}"
        );

        let back = parse_sets(&body);
        assert_eq!(back.names(), ["Standard", "Color set 2"]);
        assert_eq!(back.current, 1);
        assert_eq!(back.sets[0].colors, vec![sw("#ff0000"), sw("#00ff00")]);
        assert!(back.sets[1].colors.is_empty());
        assert_eq!(back, store);
    }

    /// A named body keeps its name through the file, the way a `.gpl`
    /// import's names have to.
    #[test]
    fn a_put_away_set_keeps_its_colour_names() {
        let mut store = SetStore::default();
        let mut live = vec![Swatch {
            rgb: mn_core::palette::parse_hex("#123456").expect("hex"),
            name: "Deep blue".into(),
        }];
        store.add(&mut live);
        let back = parse_sets(&sets_body(&store));
        assert_eq!(back.sets[0].colors[0].name, "Deep blue");
    }

    /// Keys this build does not know survive being read and written — the
    /// file is shared with future builds and with whoever edits it by hand.
    /// Junk lines are skipped rather than fatal, same policy as
    /// `swatches.txt`.
    #[test]
    fn unknown_keys_survive_and_junk_is_skipped() {
        let text = "current=Skin\nfuture_key=7\n[Standard]\nnot a colour\n#000000\n[Skin]\n";
        let store = parse_sets(text);
        assert_eq!(store.names(), ["Standard", "Skin"]);
        assert_eq!(store.current, 1);
        assert_eq!(store.sets[0].colors, vec![sw("#000000")]);
        let body = sets_body(&store);
        assert!(body.contains("future_key=7\n"), "{body}");
        assert!(!body.contains("not a colour"), "{body}");
        assert_eq!(parse_sets(&body), store);
    }

    /// An empty or missing file is one set called Standard — the state every
    /// install that has never switched sets is in.
    #[test]
    fn no_file_means_one_standard_set() {
        let store = parse_sets("");
        assert_eq!(store.names(), ["Standard"]);
        assert_eq!(store, SetStore::default());
    }

    /// Switching swaps the grid's colours with the set's, both ways, and
    /// nothing is lost on the trip out and back.
    #[test]
    fn switching_sets_swaps_the_live_grid() {
        let mut store = SetStore::default();
        let mut live = vec![sw("#ff0000")];
        store.add(&mut live);
        live.push(sw("#0000ff"));

        store.activate(&mut live, 0);
        assert_eq!(store.current, 0);
        assert_eq!(live, vec![sw("#ff0000")]);

        store.activate(&mut live, 1);
        assert_eq!(live, vec![sw("#0000ff")]);
        // And a switch to the set already live is a no-op, not a wipe.
        store.activate(&mut live, 1);
        assert_eq!(live, vec![sw("#0000ff")]);
    }

    /// Deleting takes the set and its colours; the last set cannot be
    /// deleted, because an empty combo has nothing to offer.
    #[test]
    fn deleting_a_set_falls_back_and_never_empties_the_combo() {
        let mut store = SetStore::default();
        let mut live = vec![sw("#ff0000")];
        store.add(&mut live);
        live.push(sw("#0000ff"));

        store.remove_current(&mut live);
        assert_eq!(store.names(), ["Standard"]);
        assert_eq!(
            live,
            vec![sw("#ff0000")],
            "the survivor's colours come back"
        );

        store.remove_current(&mut live);
        assert_eq!(store.names(), ["Standard"], "the last set stays");
        assert_eq!(live, vec![sw("#ff0000")]);
    }

    /// A set name carrying the file's own separators cannot split one set
    /// into two halves of junk.
    #[test]
    fn set_names_cannot_break_the_file() {
        let mut store = SetStore::default();
        store.sets[0].name = "we[ir]d\nname".into();
        let back = parse_sets(&sets_body(&store));
        assert_eq!(back.names(), ["we ir d name"]);
        assert_eq!(back.current, 0);
    }
}
