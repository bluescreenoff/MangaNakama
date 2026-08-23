//! The Tone tool's Tool Property: the screen one click lays down, and how
//! that click finds the region. Widgets edit a COPY and push it back
//! (`AppCmd::SetToneOpts`) — nothing here writes app state directly.

use super::*;

/// The screen itself. Density is the live tone layer's own knob
/// (`ToneDensity::Specified`), which is why the LP-008 density SOURCE is
/// absent: a fill layer's window says where, the slider says how much.
pub(crate) fn sec_tone(ui: &mut egui::Ui, app: &mut App) {
    let mut o = app.tone_opts;
    let mut changed = ValueBar::new("Density", 0.0, 1.0)
        .show(ui, &mut o.density)
        .changed();
    changed |= ValueBar::new("Frequency", 5.0, 80.0)
        .suffix(" lpi")
        .show(ui, &mut o.tone.lpi)
        .changed();
    changed |= ValueBar::new("Angle", 0.0, 90.0)
        .suffix("°")
        .show(ui, &mut o.tone.angle_deg)
        .changed();
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
    if changed {
        app.push_cmd(AppCmd::SetToneOpts(o));
    }
}

/// How the click finds the enclosed area — the Fill tool's flood options,
/// on the Tone tool's own copy of them.
pub(crate) fn sec_tone_region(ui: &mut egui::Ui, app: &mut App) {
    let mut o = app.tone_opts;
    let mut tol = o.region.tolerance * 100.0;
    let mut changed = ValueBar::new("Tolerance", 0.0, 50.0)
        .suffix("%")
        .show(ui, &mut tol)
        .changed();
    o.region.tolerance = tol / 100.0;
    let mut gap = o.region.gap_close_px as f32;
    changed |= ValueBar::new("Close gap", 0.0, 8.0)
        .step(1.0)
        .suffix(" px")
        .show(ui, &mut gap)
        .on_hover_text("seals breaks in the lineart so the tone cannot escape the area")
        .changed();
    o.region.gap_close_px = gap as u32;
    changed |= area_scaling_row(ui, "mn.tone.expand", &mut o.region);
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
    if let Some(v) = pick {
        o.region.refer = v;
        changed = true;
    }
    changed |= ui
        .checkbox(&mut o.region.refer_drafts, "Refer draft layers")
        .changed();
    changed |= ui
        .checkbox(&mut o.region.refer_border, "Refer to image border")
        .on_hover_text("the page's outer edge counts as a drawn line")
        .changed();
    if changed {
        app.push_cmd(AppCmd::SetToneOpts(o));
    }
}

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
