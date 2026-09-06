//! The balloon INK rows — colour, opacity and the screened interior — for the
//! Balloon tool (what a new bubble is born with) and for the Operation tool
//! (the bubble already on the page).
//!
//! Split out of `frames_balloons.rs` in lane B2 when the row conversion took
//! that file past 1 000 lines. Every control is written ONCE
//! (`B-001`–`004`, `C-04x`) and drawn twice, so the two panels cannot drift
//! into meaning different things; the only difference is the commit. The
//! tool's rows are plain app state, the object's are buffered through
//! `App::ink_edit` and pushed on the release edge of a bar drag, so one drag
//! is one undo step and not forty.
//!
//! Each control fn returns `(changed_this_frame, finished)` — `finished` is
//! that release edge.

use super::*;

// The colour/opacity/screen controls are written ONCE (`B-001`–`004`,
// `C-04x`) and drawn twice: the Balloon tool shows them as the settings a new
// bubble is born with, the Operation tool for the bubble already on the page.
// Each fn returns `(changed_this_frame, finished)` — `finished` is the edge
// the buffered (Operation-tool) caller commits on, so a bar drag is one undo
// step and not forty.

fn ink_colors(ui: &mut egui::Ui, ink: &mut mn_core::BalloonInk) -> (bool, bool) {
    let (mut changed, mut done) = (false, false);
    ui.horizontal(|ui| {
        ui.weak("line");
        if ui.color_edit_button_srgb(&mut ink.line_color).changed() {
            changed = true;
            done = true;
        }
        ui.weak("fill");
        if ui.color_edit_button_srgb(&mut ink.fill_color).changed() {
            changed = true;
            done = true;
        }
    });
    (changed, done)
}

fn ink_opacity_bar(ui: &mut egui::Ui, label: &str, v: &mut f32) -> (bool, bool) {
    let mut pct = *v * 100.0;
    let resp = ValueBar::new(label, 0.0, 100.0)
        .suffix(" %")
        .show(ui, &mut pct);
    let changed = resp.changed();
    if changed {
        *v = (pct / 100.0).clamp(0.0, 1.0);
    }
    (
        changed,
        resp.drag_stopped() || (changed && !resp.dragged()),
    )
}

fn ink_line_opacity(ui: &mut egui::Ui, ink: &mut mn_core::BalloonInk) -> (bool, bool) {
    ink_opacity_bar(ui, "Line opacity", &mut ink.line_opacity)
}

/// Fill opacity 0 IS CSP's "fill inside frame" switched off — the outline
/// inks and the art behind shows through the bubble. There is no separate
/// checkbox because a fill you cannot see and a fill that is not there are
/// the same balloon.
fn ink_fill_opacity(ui: &mut egui::Ui, ink: &mut mn_core::BalloonInk) -> (bool, bool) {
    let r = ink_opacity_bar(ui, "Fill opacity", &mut ink.fill_opacity);
    if ink.fill_opacity <= 0.0 {
        ui.weak("no fill — the art shows through the bubble");
    }
    r
}

/// `C-04x`: a screened interior, the printed whisper/flashback bubble.
fn ink_screened(ui: &mut egui::Ui, ink: &mut mn_core::BalloonInk) -> (bool, bool) {
    let mut toned = ink.fill_tone.is_some();
    if ui
        .checkbox(&mut toned, "Screened fill")
        .on_hover_text("a halftone interior instead of flat paper")
        .changed()
    {
        ink.fill_tone = toned.then(mn_core::BalloonTone::default);
        return (true, true);
    }
    (false, false)
}

fn ink_tone_density(ui: &mut egui::Ui, ink: &mut mn_core::BalloonInk) -> (bool, bool) {
    let Some(t) = &mut ink.fill_tone else {
        return (false, false);
    };
    let mut d = t.density * 100.0;
    let resp = ValueBar::new("Density", 0.0, 100.0)
        .suffix(" %")
        .show(ui, &mut d);
    let changed = resp.changed();
    if changed {
        t.density = (d / 100.0).clamp(0.0, 1.0);
    }
    (
        changed,
        resp.drag_stopped() || (changed && !resp.dragged()),
    )
}

fn ink_tone_cell(ui: &mut egui::Ui, ink: &mut mn_core::BalloonInk) -> (bool, bool) {
    let Some(t) = &mut ink.fill_tone else {
        return (false, false);
    };
    let mut cell = t.cell_px;
    let resp = ValueBar::new("Cell", 2.0, 40.0)
        .decimals(1)
        .suffix(" px")
        .show(ui, &mut cell);
    let changed = resp.changed();
    if changed {
        t.cell_px = cell.max(2.0);
    }
    (
        changed,
        resp.drag_stopped() || (changed && !resp.dragged()),
    )
}

fn ink_tone_angle(ui: &mut egui::Ui, ink: &mut mn_core::BalloonInk) -> (bool, bool) {
    let Some(t) = &mut ink.fill_tone else {
        return (false, false);
    };
    let mut ang = t.angle_deg;
    let resp = ValueBar::new("Angle", 0.0, 90.0).suffix("°").show(ui, &mut ang);
    let changed = resp.changed();
    if changed {
        t.angle_deg = ang;
    }
    (
        changed,
        resp.drag_stopped() || (changed && !resp.dragged()),
    )
}

fn ink_tone_pattern(ui: &mut egui::Ui, ink: &mut mn_core::BalloonInk) -> (bool, bool) {
    let Some(t) = &mut ink.fill_tone else {
        return (false, false);
    };
    let (mut changed, mut done) = (false, false);
    ui.horizontal(|ui| {
        ui.weak("pattern");
        egui::ComboBox::from_id_salt("mn.balloon.tone.pattern")
            .width(96.0)
            .selected_text(t.pattern.label())
            .show_ui(ui, |ui| {
                for pat in mn_core::TonePattern::ALL {
                    if ui.selectable_label(t.pattern == pat, pat.label()).clicked() {
                        t.pattern = pat;
                        changed = true;
                        done = true;
                    }
                }
            });
    });
    // The cell is stored in canvas px, so it does NOT re-flow when the
    // document dpi changes afterwards. Said out loud rather than hidden.
    ui.weak("cell is in canvas px — a later dpi change does not re-flow it");
    (changed, done)
}

/// The Balloon TOOL's ink rows: plain app state, written back on change.
fn ink_tool_row(
    ui: &mut egui::Ui,
    app: &mut App,
    f: impl FnOnce(&mut egui::Ui, &mut mn_core::BalloonInk) -> (bool, bool),
) {
    let mut ink = app.balloon_ink;
    if f(ui, &mut ink).0 {
        app.balloon_ink = ink;
    }
}

/// The bubble under the Operation tool, with the in-flight edit applied.
fn obj_ink_now(app: &App) -> Option<mn_core::BalloonInk> {
    let (li, bi) = app.balloon_sel?;
    let cur = app.doc.layers.get(li)?.balloons()?.balloons.get(bi)?.ink();
    Some(app.ink_edit.unwrap_or(cur))
}

fn obj_ink_toned(app: &App) -> bool {
    obj_ink_now(app).is_some_and(|i| i.fill_tone.is_some())
}

fn tool_ink_toned(app: &App) -> bool {
    app.balloon_ink.fill_tone.is_some()
}

fn tool_ink_touched(app: &App) -> bool {
    app.balloon_ink != mn_core::BalloonInk::default()
}

/// The Operation tool + a selected BALLOON: repaint the bubble on the page.
/// Buffered through `App::ink_edit` so one bar drag is one undo step.
fn ink_obj_row(
    ui: &mut egui::Ui,
    app: &mut App,
    f: impl FnOnce(&mut egui::Ui, &mut mn_core::BalloonInk) -> (bool, bool),
) {
    let Some((li, bi)) = app.balloon_sel else {
        return;
    };
    let Some(bs) = app.doc.layers.get(li).and_then(|l| l.balloons()).cloned() else {
        return;
    };
    let Some(cur) = bs.balloons.get(bi).map(|b| b.ink()) else {
        return;
    };
    let mut ink = app.ink_edit.unwrap_or(cur);
    let (changed, done) = f(ui, &mut ink);
    if changed {
        app.ink_edit = Some(ink);
    }
    if done {
        if let Some(ink) = app.ink_edit.take() {
            let mut bs2 = bs.clone();
            bs2.balloons[bi].set_ink(ink);
            app.push_cmd(AppCmd::BalloonCommit {
                layer: li,
                balloons: bs2,
            });
            // The tool remembers what you just chose, so the next bubble
            // matches the one you have been styling.
            app.balloon_ink = ink;
        }
    }
}

pub(crate) fn row_balloon_ink_colors(ui: &mut egui::Ui, app: &mut App) {
    ink_tool_row(ui, app, ink_colors);
}
pub(crate) fn row_balloon_ink_line_opacity(ui: &mut egui::Ui, app: &mut App) {
    ink_tool_row(ui, app, ink_line_opacity);
}
pub(crate) fn row_balloon_ink_fill_opacity(ui: &mut egui::Ui, app: &mut App) {
    ink_tool_row(ui, app, ink_fill_opacity);
}
pub(crate) fn row_balloon_ink_screened(ui: &mut egui::Ui, app: &mut App) {
    ink_tool_row(ui, app, ink_screened);
}
pub(crate) fn row_balloon_ink_density(ui: &mut egui::Ui, app: &mut App) {
    ink_tool_row(ui, app, ink_tone_density);
}
pub(crate) fn row_balloon_ink_cell(ui: &mut egui::Ui, app: &mut App) {
    ink_tool_row(ui, app, ink_tone_cell);
}
pub(crate) fn row_balloon_ink_angle(ui: &mut egui::Ui, app: &mut App) {
    ink_tool_row(ui, app, ink_tone_angle);
}
pub(crate) fn row_balloon_ink_pattern(ui: &mut egui::Ui, app: &mut App) {
    ink_tool_row(ui, app, ink_tone_pattern);
}

pub(crate) fn row_balloon_ink_reset(ui: &mut egui::Ui, app: &mut App) {
    if ui.button("Back to black on white").clicked() {
        app.balloon_ink = mn_core::BalloonInk::default();
    }
}

/// Balloon tool: what a NEW bubble is inked with (`C-039`–`048`).
pub(crate) const ROWS_BALLOON_INK: &[Row] = &[
    row(
        "balloon.ink.colors",
        "Line / fill colour",
        row_balloon_ink_colors,
    ),
    row(
        "balloon.ink.line_opacity",
        "Line opacity",
        row_balloon_ink_line_opacity,
    ),
    row(
        "balloon.ink.fill_opacity",
        "Fill opacity",
        row_balloon_ink_fill_opacity,
    ),
    row(
        "balloon.ink.screened",
        "Screened fill",
        row_balloon_ink_screened,
    ),
    row_when(
        "balloon.ink.density",
        "Screen density",
        row_balloon_ink_density,
        tool_ink_toned,
    ),
    row_when(
        "balloon.ink.cell",
        "Screen cell",
        row_balloon_ink_cell,
        tool_ink_toned,
    ),
    row_when(
        "balloon.ink.angle",
        "Screen angle",
        row_balloon_ink_angle,
        tool_ink_toned,
    ),
    row_when(
        "balloon.ink.pattern",
        "Screen pattern",
        row_balloon_ink_pattern,
        tool_ink_toned,
    ),
    row_when(
        "balloon.ink.reset",
        "Back to black on white",
        row_balloon_ink_reset,
        tool_ink_touched,
    ),
];

pub(crate) fn row_obj_ink_colors(ui: &mut egui::Ui, app: &mut App) {
    ink_obj_row(ui, app, ink_colors);
}
pub(crate) fn row_obj_ink_line_opacity(ui: &mut egui::Ui, app: &mut App) {
    ink_obj_row(ui, app, ink_line_opacity);
}
pub(crate) fn row_obj_ink_fill_opacity(ui: &mut egui::Ui, app: &mut App) {
    ink_obj_row(ui, app, ink_fill_opacity);
}
pub(crate) fn row_obj_ink_screened(ui: &mut egui::Ui, app: &mut App) {
    ink_obj_row(ui, app, ink_screened);
}
pub(crate) fn row_obj_ink_density(ui: &mut egui::Ui, app: &mut App) {
    ink_obj_row(ui, app, ink_tone_density);
}
pub(crate) fn row_obj_ink_cell(ui: &mut egui::Ui, app: &mut App) {
    ink_obj_row(ui, app, ink_tone_cell);
}
pub(crate) fn row_obj_ink_angle(ui: &mut egui::Ui, app: &mut App) {
    ink_obj_row(ui, app, ink_tone_angle);
}
pub(crate) fn row_obj_ink_pattern(ui: &mut egui::Ui, app: &mut App) {
    ink_obj_row(ui, app, ink_tone_pattern);
}

pub(crate) const ROWS_OBJ_INK: &[Row] = &[
    row(
        "obj.balloon.ink.colors",
        "Line / fill colour",
        row_obj_ink_colors,
    ),
    row(
        "obj.balloon.ink.line_opacity",
        "Line opacity",
        row_obj_ink_line_opacity,
    ),
    row(
        "obj.balloon.ink.fill_opacity",
        "Fill opacity",
        row_obj_ink_fill_opacity,
    ),
    row(
        "obj.balloon.ink.screened",
        "Screened fill",
        row_obj_ink_screened,
    ),
    row_when(
        "obj.balloon.ink.density",
        "Screen density",
        row_obj_ink_density,
        obj_ink_toned,
    ),
    row_when(
        "obj.balloon.ink.cell",
        "Screen cell",
        row_obj_ink_cell,
        obj_ink_toned,
    ),
    row_when(
        "obj.balloon.ink.angle",
        "Screen angle",
        row_obj_ink_angle,
        obj_ink_toned,
    ),
    row_when(
        "obj.balloon.ink.pattern",
        "Screen pattern",
        row_obj_ink_pattern,
        obj_ink_toned,
    ),
];
