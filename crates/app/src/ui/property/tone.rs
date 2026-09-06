//! The Tone tool's Tool Property: the screen one click lays down, and how
//! that click finds the region. Widgets edit a COPY and push it back
//! (`AppCmd::SetToneOpts`) — nothing here writes app state directly, and
//! that is per ROW since lane B2: each row takes its own copy of
//! `app.tone_opts`, changes its one field and pushes.

use super::*;

/// One row over the Tone tool's options: copy, edit, push if it moved.
fn tone_row(
    ui: &mut egui::Ui,
    app: &mut App,
    f: impl FnOnce(&mut egui::Ui, &mut crate::cmd::ToneToolOpts) -> bool,
) {
    let mut o = app.tone_opts;
    if f(ui, &mut o) {
        app.push_cmd(AppCmd::SetToneOpts(o));
    }
}

// --- the screen itself -----------------------------------------------------
//
// Density is the live tone layer's own knob (`ToneDensity::Specified`), which
// is why the LP-008 density SOURCE is absent: a fill layer's window says
// where, the slider says how much.

pub(crate) fn row_tone_density(ui: &mut egui::Ui, app: &mut App) {
    tone_row(ui, app, |ui, o| {
        ValueBar::new("Density", 0.0, 1.0)
            .show(ui, &mut o.density)
            .changed()
    });
}

pub(crate) fn row_tone_frequency(ui: &mut egui::Ui, app: &mut App) {
    tone_row(ui, app, |ui, o| {
        ValueBar::new("Frequency", 5.0, 80.0)
            .suffix(" lpi")
            .show(ui, &mut o.tone.lpi)
            .changed()
    });
}

pub(crate) fn row_tone_angle(ui: &mut egui::Ui, app: &mut App) {
    tone_row(ui, app, |ui, o| {
        ValueBar::new("Angle", 0.0, 90.0)
            .suffix("°")
            .show(ui, &mut o.tone.angle_deg)
            .changed()
    });
}

pub(crate) fn row_tone_pattern(ui: &mut egui::Ui, app: &mut App) {
    tone_row(ui, app, |ui, o| {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.weak("pattern");
            egui::ComboBox::from_id_salt("mn.tone.tool.pattern")
                .width(96.0)
                .selected_text(o.tone.pattern.label())
                .show_ui(ui, |ui| {
                    for pat in mn_core::TonePattern::ALL {
                        if ui
                            .selectable_label(o.tone.pattern == pat, pat.label())
                            .clicked()
                        {
                            o.tone.pattern = pat;
                            changed = true;
                        }
                    }
                });
        });
        changed
    });
}

pub(crate) const ROWS_TONE: &[Row] = &[
    row("tone.screen.density", "Density", row_tone_density),
    row("tone.screen.frequency", "Frequency", row_tone_frequency),
    row("tone.screen.angle", "Angle", row_tone_angle),
    row("tone.screen.pattern", "Pattern", row_tone_pattern),
];

// --- how the click finds the enclosed area ---------------------------------
//
// The Fill tool's flood options, on the Tone tool's own copy of them.

pub(crate) fn row_tone_tolerance(ui: &mut egui::Ui, app: &mut App) {
    tone_row(ui, app, |ui, o| {
        let mut tol = o.region.tolerance * 100.0;
        let changed = ValueBar::new("Tolerance", 0.0, 50.0)
            .suffix("%")
            .show(ui, &mut tol)
            .changed();
        o.region.tolerance = tol / 100.0;
        changed
    });
}

pub(crate) fn row_tone_gap(ui: &mut egui::Ui, app: &mut App) {
    tone_row(ui, app, |ui, o| {
        let mut gap = o.region.gap_close_px as f32;
        let changed = ValueBar::new("Close gap", 0.0, 8.0)
            .step(1.0)
            .suffix(" px")
            .show(ui, &mut gap)
            .on_hover_text("seals breaks in the lineart so the tone cannot escape the area")
            .changed();
        o.region.gap_close_px = gap as u32;
        changed
    });
}

pub(crate) fn row_tone_area_scaling(ui: &mut egui::Ui, app: &mut App) {
    tone_row(ui, app, |ui, o| {
        area_scaling_row(ui, "mn.tone.expand", &mut o.region)
    });
}

pub(crate) fn row_tone_refer(ui: &mut egui::Ui, app: &mut App) {
    tone_row(ui, app, |ui, o| {
        let mut pick: Option<mn_core::FillRefer> = None;
        egui::ComboBox::from_id_salt("mn.tone.refer")
            .width(ui.available_width() - 8.0)
            .selected_text(refer_label(o.region.refer))
            .show_ui(ui, |ui| {
                for v in [
                    mn_core::FillRefer::All,
                    mn_core::FillRefer::Active,
                    mn_core::FillRefer::Reference,
                ] {
                    if ui
                        .selectable_label(o.region.refer == v, refer_label(v))
                        .clicked()
                    {
                        pick = Some(v);
                    }
                }
            });
        match pick {
            Some(v) => {
                o.region.refer = v;
                true
            }
            None => false,
        }
    });
}

pub(crate) fn row_tone_refer_drafts(ui: &mut egui::Ui, app: &mut App) {
    tone_row(ui, app, |ui, o| {
        ui.checkbox(&mut o.region.refer_drafts, "Refer draft layers")
            .changed()
    });
}

pub(crate) fn row_tone_refer_border(ui: &mut egui::Ui, app: &mut App) {
    tone_row(ui, app, |ui, o| {
        ui.checkbox(&mut o.region.refer_border, "Refer to image border")
            .on_hover_text("the page's outer edge counts as a drawn line")
            .changed()
    });
}

pub(crate) const ROWS_TONE_REGION: &[Row] = &[
    row("tone.region.tolerance", "Tolerance", row_tone_tolerance),
    row("tone.region.gap", "Close gap", row_tone_gap),
    row(
        "tone.region.area_scaling",
        "Area scaling",
        row_tone_area_scaling,
    ),
    row("tone.region.refer", "Refer", row_tone_refer),
    row(
        "tone.region.refer_drafts",
        "Refer draft layers",
        row_tone_refer_drafts,
    ),
    row(
        "tone.region.refer_border",
        "Refer to image border",
        row_tone_refer_border,
    ),
];

fn refer_label(r: mn_core::FillRefer) -> &'static str {
    match r {
        mn_core::FillRefer::All => "Refer: all layers",
        mn_core::FillRefer::Active => "Refer: editing layer",
        mn_core::FillRefer::Reference => "Refer: reference layer",
    }
}

pub(crate) fn sec_tone_guide(ui: &mut egui::Ui, _app: &mut App) {
    ui.weak("click an enclosed area — it becomes a tone layer you can still edit");
    ui.weak("`,` / `.` step the screen shape");
}
