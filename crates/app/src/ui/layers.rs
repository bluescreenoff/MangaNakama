//! The Layers palette (the big one) + Layer Property: full stack rows
//! (eye, label rail, thumbnail, rename, folder blocks, drag-reorder via
//! the LayerDrag payload), blend/opacity strip, thumbnails over a
//! checkerboard, and the Layer Property palette body.

use super::icons::{self, Icon};
use super::theme;
use super::theme::ValueBar;
use super::widgets::{group_caption, icon_btn};
use crate::app::{App, LayerFilterKind};
use crate::cmd::AppCmd;
use mn_core::{Blend, FillKind, LayerKind};

// The picker order is OURS, not CSP's: parts 1, 2 and 3 in the order they
// shipped, appended never re-sorted, because a saved workspace and the
// owner's muscle memory both index into this list. So Color burn sits at the
// bottom rather than next to Color dodge — the search field is the way to
// reach a mode by name.
//
// 27 of CSP's 28. The missing one is Add (Glow): our Add is already the
// premultiplied, saturating add, which IS the stronger of CSP's two — see
// the deviation note in `mn_core::blend`.
pub(super) const BLENDS: [Blend; 27] = [
    Blend::Normal,
    Blend::Multiply,
    Blend::Screen,
    Blend::Add,
    Blend::Subtract,
    Blend::Darken,
    Blend::Lighten,
    Blend::Overlay,
    Blend::SoftLight,
    Blend::HardLight,
    Blend::Difference,
    Blend::Exclusion,
    Blend::Hue,
    Blend::Saturation,
    Blend::Color,
    Blend::ColorBurn,
    Blend::LinearBurn,
    Blend::ColorDodge,
    Blend::GlowDodge,
    Blend::VividLight,
    Blend::LinearLight,
    Blend::PinLight,
    Blend::HardMix,
    Blend::Divide,
    Blend::DarkerColor,
    Blend::LighterColor,
    Blend::Luminosity,
];

/// CSP's Layer Property: what the *active layer* is. Frame layers expose the
/// border thickness (one undo step per drag); raster layers get blend +
/// opacity, handy when the Layers palette is collapsed or floated away.
pub(super) fn layer_property(ui: &mut egui::Ui, app: &mut App) {
    let i = app.doc.active;
    let Some(l) = app.doc.layers.get(i) else {
        return;
    };
    let name = l.name.clone();
    let (blend, opacity) = (l.blend, l.opacity);
    let frames = l.frames().cloned();
    let balloons = l.balloons().cloned();
    let tone = l.tone;

    ui.label(
        egui::RichText::new(name)
            .size(11.5)
            .color(theme::TEXT_STRONG),
    );
    let (mut reference, mut draft) = (l.reference, l.draft);
    match (frames, balloons) {
        (Some(fs), _) => {
            group_caption(ui, "Frame border");
            let px_per_mm = app.mm_to_px(1.0).max(0.001);
            let mut mm = app.border_edit.unwrap_or(fs.border_px / px_per_mm);
            let resp = ValueBar::new("Thickness", 0.1, 3.0)
                .decimals(2)
                .suffix(" mm")
                .show(ui, &mut mm);
            if resp.changed() {
                app.border_edit = Some(mm);
            }
            // Commit once, when the interaction ends — not per drag tick.
            if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                if let Some(mm) = app.border_edit.take() {
                    let mut fs = fs.clone();
                    fs.border_px = (mm * px_per_mm).max(0.5);
                    app.push_cmd(AppCmd::FrameCommit {
                        layer: i,
                        frames: fs,
                    });
                }
            }
            ui.weak(format!("{} panel(s) — U divides, O edits", fs.frames.len()));
        }
        (_, Some(bs)) => {
            group_caption(ui, "Balloon line");
            let px_per_mm = app.mm_to_px(1.0).max(0.001);
            let mut mm = app.border_edit.unwrap_or(bs.border_px / px_per_mm);
            let resp = ValueBar::new("Thickness", 0.05, 2.0)
                .decimals(2)
                .suffix(" mm")
                .show(ui, &mut mm);
            if resp.changed() {
                app.border_edit = Some(mm);
            }
            if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                if let Some(mm) = app.border_edit.take() {
                    let mut bs = bs.clone();
                    bs.border_px = (mm * px_per_mm).max(0.5);
                    app.push_cmd(AppCmd::BalloonCommit {
                        layer: i,
                        balloons: bs,
                    });
                }
            }
            ui.weak(format!(
                "{} balloon(s) — W adds, O edits",
                bs.balloons.len()
            ));
        }
        (None, None) if app.doc.layers[i].is_text() => {
            group_caption(ui, "Text");
            let n = app.doc.layers[i]
                .texts()
                .map(|t| t.texts.len())
                .unwrap_or(0);
            ui.weak(format!("{n} text box(es) — T types, O moves/rotates"));
        }
        (None, None) => {
            group_caption(ui, "Layer");
            ui.horizontal(|ui| {
                let mut pick = None;
                egui::ComboBox::from_id_salt("mn.blend.prop")
                    .width(88.0)
                    .selected_text(blend_name(blend))
                    .show_ui(ui, |ui| {
                        for b in BLENDS {
                            if ui.selectable_label(blend == b, blend_name(b)).clicked() {
                                pick = Some(b);
                            }
                        }
                    });
                if let Some(b) = pick {
                    app.push_cmd(AppCmd::SetLayerBlend(i, b));
                }
                let mut pct = opacity * 100.0;
                if ValueBar::new("", 0.0, 100.0)
                    .suffix("%")
                    .width(ui.available_width())
                    .show(ui, &mut pct)
                    .changed()
                {
                    app.push_cmd(AppCmd::SetLayerOpacity(i, pct / 100.0));
                }
            });

            // CSP Layer Property ▸ Effect ▸ Tone: the non-destructive
            // conversion. Painting keeps working on the ink source; the
            // screen follows.
            group_caption(ui, "Effect");
            let mut effect_pick = None;
            egui::ComboBox::from_id_salt("mn.effect")
                .width(120.0)
                .selected_text(match tone {
                    Some(_) => "Tone",
                    None => "None",
                })
                .show_ui(ui, |ui| {
                    if ui.selectable_label(tone.is_none(), "None").clicked() {
                        effect_pick = Some(false);
                    }
                    if ui.selectable_label(tone.is_some(), "Tone").clicked() {
                        effect_pick = Some(true);
                    }
                });
            if let Some(want_tone) = effect_pick {
                app.push_cmd(AppCmd::SetTone(if want_tone {
                    Some(mn_core::ToneParams::default())
                } else {
                    None
                }));
            }
            if let Some(cur) = tone {
                let mut p = app.tone_edit.unwrap_or(cur);
                // Combos and the posterize switch are DISCRETE: one click is
                // the whole edit, so they commit at once. The bars drag, so
                // they coalesce through `app.tone_edit` and commit on release
                // (one undo step per drag, not one per frame).
                let mut discrete = false;
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("mn.tone.pattern")
                        .width(88.0)
                        .selected_text(p.pattern.label())
                        .show_ui(ui, |ui| {
                            for pat in mn_core::TonePattern::ALL {
                                if ui.selectable_label(p.pattern == pat, pat.label()).clicked() {
                                    p.pattern = pat;
                                    discrete = true;
                                }
                            }
                        });
                    ui.weak(format!("{} LPI at {}°", p.lpi, p.angle_deg));
                });
                let mut bars: Vec<egui::Response> = Vec::new();
                bars.push(
                    ValueBar::new("Frequency", 5.0, 80.0)
                        .decimals(1)
                        .suffix(" LPI")
                        .show(ui, &mut p.lpi),
                );
                bars.push(
                    ValueBar::new("Angle", 0.0, 90.0)
                        .decimals(0)
                        .suffix("°")
                        .show(ui, &mut p.angle_deg),
                );

                // LP-008 density source. "Specified" is CSP's Fill-layer-only
                // option; we allow it on any tone layer too, because "screen
                // the region I painted at a flat 40 %" is the same engine and
                // the same want.
                const DENSITIES: [mn_core::ToneDensity; 3] = [
                    mn_core::ToneDensity::ImageColour,
                    mn_core::ToneDensity::ImageBrightness,
                    mn_core::ToneDensity::Specified(0.4),
                ];
                egui::ComboBox::from_id_salt("mn.tone.density")
                    .width(160.0)
                    .selected_text(p.density.label())
                    .show_ui(ui, |ui| {
                        for d in DENSITIES {
                            let picked = std::mem::discriminant(&p.density)
                                == std::mem::discriminant(&d);
                            if ui.selectable_label(picked, d.label()).clicked() && !picked {
                                p.density = d;
                                discrete = true;
                            }
                        }
                    });
                if let mn_core::ToneDensity::Specified(d) = p.density {
                    let mut pct = d * 100.0;
                    let r = ValueBar::new("Density", 0.0, 100.0)
                        .decimals(0)
                        .suffix("%")
                        .show(ui, &mut pct);
                    p.density = mn_core::ToneDensity::Specified(pct / 100.0);
                    bars.push(r);
                }

                // LP-014 / TN-009: the lattice origin. ±32 px covers several
                // cells at every frequency the Frequency bar offers, and the
                // whole point is a nudge of a few px.
                bars.push(
                    ValueBar::new("Dot position X", -32.0, 32.0)
                        .decimals(1)
                        .suffix(" px")
                        .show(ui, &mut p.offset[0]),
                );
                bars.push(
                    ValueBar::new("Dot position Y", -32.0, 32.0)
                        .decimals(1)
                        .suffix(" px")
                        .show(ui, &mut p.offset[1]),
                );

                // LP-010 posterization.
                let mut post_on = p.posterize.is_some();
                if ui
                    .checkbox(
                        &mut post_on,
                        egui::RichText::new("Posterize the density").size(12.0),
                    )
                    .on_hover_text("flatten the density ramp into a few steps")
                    .changed()
                {
                    p.posterize = post_on.then_some(4);
                    discrete = true;
                }
                if let Some(n) = p.posterize {
                    let mut steps = n as f32;
                    let r = ValueBar::new("Steps", 2.0, 20.0)
                        .decimals(0)
                        .step(1.0)
                        .show(ui, &mut steps);
                    p.posterize = Some(steps.round().clamp(2.0, 20.0) as u8);
                    bars.push(r);
                }

                let dragging = bars.iter().any(|r| r.dragged());
                let changed = bars.iter().any(|r| r.changed());
                let released = bars.iter().any(|r| r.drag_stopped());
                if changed {
                    app.tone_edit = Some(p);
                }
                if discrete {
                    app.tone_edit = None;
                    app.push_cmd(AppCmd::SetTone(Some(p)));
                } else if released || (changed && !dragging) {
                    if let Some(p) = app.tone_edit.take() {
                        app.push_cmd(AppCmd::SetTone(Some(p)));
                    }
                }
            }
        }
    }

    // CSP Layer Settings, on every layer kind: the reference layer fill/wand
    // can be told to sample, and the draft flag (hidden from fill refs and
    // export, still shown on screen).
    ui.add_space(3.0);
    group_caption(ui, "Layer settings");
    // RF-001 (owner spec): click toggles THIS layer independently (a folder
    // row toggles its whole child run — the unit); Alt+click SOLOs it; the
    // count + clear-all sit beside it.
    ui.horizontal(|ui| {
        let alt = ui.input(|i| i.modifiers.alt);
        let cb = ui
            .checkbox(&mut reference, "Reference layer")
            .on_hover_text(
                "fill/wand sample the reference set (folders toggle as one unit) — Alt+click solos this layer",
            );
        if cb.changed() {
            if alt && reference {
                app.push_cmd(AppCmd::SetLayerReferenceSolo(i));
            } else {
                app.push_cmd(AppCmd::SetLayerReference(i, reference));
            }
        }
        let n = app.doc.reference_layers().len();
        if n > 0 {
            ui.weak(format!("{n} ref"));
            if ui
                .small_button("✕")
                .on_hover_text("clear every reference layer")
                .clicked()
            {
                app.push_cmd(AppCmd::ClearReferences);
            }
        }
    });
    if ui
        .checkbox(&mut draft, "Draft layer")
        .on_hover_text("shown on screen; excluded from fill references and PNG export")
        .changed()
    {
        app.push_cmd(AppCmd::SetLayerDraft(i, draft));
    }

    // FB-overflow: only offered where it means something — a non-folder
    // layer living inside a frame folder.
    if !app.doc.layers[i].folder && app.doc.enclosing_frame_folder(i).is_some() {
        let mut esc = app.doc.layers[i].escape_frame;
        if ui
            .checkbox(&mut esc, "Burst out of the panel")
            .on_hover_text(
                "this layer draws OVER the frame border and outside the panel mask — \
                 the art overflows, the panel stays editable, the layer stays in its folder",
            )
            .changed()
        {
            app.push_cmd(AppCmd::SetLayerEscape(i, esc));
        }
    }

    // LP-016 Layer colour: draw black, DISPLAY as the chosen colour (the
    // draft/two-tone workflow — non-destructive; pixels stay black).
    let cur = app.doc.layers.get(i).and_then(|l| l.layer_colour);
    ui.horizontal(|ui| {
        let mut on = cur.is_some();
        if ui
            .checkbox(&mut on, "Layer colour")
            .on_hover_text("the layer's dark ink displays in this colour — non-destructive")
            .changed()
        {
            let c = if on {
                cur.or(Some(LAYER_TINTS[0]))
            } else {
                None
            };
            app.push_cmd(AppCmd::SetLayerColour(i, c));
        }
        if on {
            for &t in LAYER_TINTS.iter() {
                let lit = cur == Some(t);
                let visuals = egui::Button::new(
                    egui::RichText::new("■").color(egui::Color32::from_rgb(t[0], t[1], t[2])),
                );
                let visuals = if lit { visuals.small() } else { visuals };
                if ui
                    .add(visuals)
                    .on_hover_text(format!("#{:02x}{:02x}{:02x}", t[0], t[1], t[2]))
                    .clicked()
                {
                    app.push_cmd(AppCmd::SetLayerColour(i, Some(t)));
                }
            }
        }
    });

    // LP-017 SUB colour: the OTHER end of the same ramp. The main colour
    // replaces the layer's black, this replaces its WHITE — the two-tone
    // rough-draft pair. Meaningless on its own, so the row only exists once
    // a layer colour is set.
    if cur.is_some() {
        let sub = app.doc.layers.get(i).and_then(|l| l.layer_sub_colour);
        ui.horizontal(|ui| {
            let mut on = sub.is_some();
            if ui
                .checkbox(&mut on, "Sub colour")
                .on_hover_text(
                    "the layer's WHITE end displays in this colour — off leaves it white",
                )
                .changed()
            {
                let c = if on { sub.or(Some(LAYER_TINTS[3])) } else { None };
                app.push_cmd(AppCmd::SetLayerSubColour(i, c));
            }
            if on && let Some(c) = tint_chips(ui, &LAYER_TINTS, sub) {
                app.push_cmd(AppCmd::SetLayerSubColour(i, Some(c)));
            }
        });
    }

    // LP-002/LP-003 Border effect ▸ Edge: grow an outline round the layer's
    // OWN alpha. The white keyline that lets a character read against a tone,
    // which today is hand-inked. Non-destructive, and it follows the drawing
    // — draw more and the outline is round the new ink on the next frame.
    //
    // On a PLAIN folder the same effect is FB-knockout ("Knock out behind"):
    // the outline grows from the union of the children's ink and lies just
    // beneath the group — the hand-painted white mat, automated. Frame
    // folders are excluded (`Document::set_edge` refuses them): their close
    // already owns a panel mask and border ink.
    if !app
        .doc
        .layers
        .get(i)
        .is_some_and(|l| l.folder && l.is_frame())
    {
        let is_folder = app.doc.layers.get(i).is_some_and(|l| l.folder);
        let edge = app.doc.layers.get(i).and_then(|l| l.edge);
        ui.horizontal(|ui| {
            let mut on = edge.is_some();
            let (label, tip) = if is_folder {
                (
                    "Knock out behind",
                    "a mat grown round everything in this folder, laid just under \
                     the group — the white backing behind balloons and SFX, redrawn \
                     as the art changes",
                )
            } else {
                (
                    "Border effect",
                    "an outline grown round this layer's own alpha — nothing is painted",
                )
            };
            if ui
                .checkbox(&mut on, label)
                .on_hover_text(tip)
                .changed()
            {
                app.edge_edit = None;
                app.push_cmd(AppCmd::SetEdge(i, on.then(|| edge.unwrap_or_default())));
            }
            if let Some(e) = edge
                && let Some(c) = tint_chips(ui, &EDGE_TINTS, Some(e.colour))
            {
                app.push_cmd(AppCmd::SetEdge(
                    i,
                    Some(mn_core::EdgeParams { colour: c, ..e }),
                ));
            }
        });
        if let Some(e) = edge {
            let mut p = app.edge_edit.unwrap_or(e);
            let resp = ValueBar::new("Thickness", 0.0, mn_core::edge::WIDTH_MAX)
                .decimals(1)
                .suffix(" px")
                .show(ui, &mut p.width_px);
            if resp.changed() {
                app.edge_edit = Some(p);
            }
            // Commit once, when the interaction ends. Every change re-derives
            // the whole layer's outline, so a per-frame commit would be both
            // an undo-stack spill and a visible stutter.
            if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                if let Some(p) = app.edge_edit.take() {
                    app.push_cmd(AppCmd::SetEdge(i, Some(p)));
                }
            }
        }
    }

    // LP-022 decrease-colour PREVIEW: ask the screen "would this page hold up
    // in 1-bit?" without converting anything. Deliberately NOT CSP's
    // irreversible per-layer expression colour (TRIAGE 147, owner-ruled
    // `low`) — no pixel changes, the export composite ignores it, and the
    // setting survives a save so the answer stays one click away.
    let expr = app
        .doc
        .layers
        .get(i)
        .map(|l| l.expression)
        .unwrap_or_default();
    let mut expr_pick = None;
    ui.horizontal(|ui| {
        ui.label("Preview as");
        egui::ComboBox::from_id_salt("mn.layer.expression")
            .width(104.0)
            .selected_text(expression_name(expr))
            .show_ui(ui, |ui| {
                for e in [
                    mn_core::LayerExpression::Colour,
                    mn_core::LayerExpression::Grey,
                    mn_core::LayerExpression::Mono,
                ] {
                    if ui
                        .selectable_label(expr == e, expression_name(e))
                        .clicked()
                    {
                        expr_pick = Some(e);
                    }
                }
            })
            .response
            .on_hover_text("display only — nothing is converted, and the export ignores it");
    });
    if let Some(e) = expr_pick {
        app.push_cmd(AppCmd::SetLayerExpression(i, e));
    }
}

/// A row of colour chips; returns the one clicked. The chip currently in use
/// draws small, which is how the LP-016 row above shows the active pick.
fn tint_chips(ui: &mut egui::Ui, chips: &[[u8; 3]], current: Option<[u8; 3]>) -> Option<[u8; 3]> {
    let mut picked = None;
    for &t in chips {
        let b = egui::Button::new(
            egui::RichText::new("■").color(egui::Color32::from_rgb(t[0], t[1], t[2])),
        );
        let b = if current == Some(t) { b.small() } else { b };
        if ui
            .add(b)
            .on_hover_text(format!("#{:02x}{:02x}{:02x}", t[0], t[1], t[2]))
            .clicked()
        {
            picked = Some(t);
        }
    }
    picked
}

/// LP-022 preview names. "Mono" says 1-bit out loud because the whole point
/// of the preview is the aliased edge, and a user who reads it as "greyscale"
/// would draw the wrong conclusion from what they see.
fn expression_name(e: mn_core::LayerExpression) -> &'static str {
    match e {
        mn_core::LayerExpression::Colour => "Colour",
        mn_core::LayerExpression::Grey => "Grey",
        mn_core::LayerExpression::Mono => "Mono (1-bit)",
    }
}

/// Border-effect chips. White is first because the white keyline round a
/// character sitting on a tone is the reason the effect exists at all.
const EDGE_TINTS: [[u8; 3]; 4] = [
    [0xff, 0xff, 0xff], // white — the keyline
    [0x00, 0x00, 0x00], // black
    [0xe5, 0x4b, 0x4b], // red
    [0x2a, 0x6f, 0xf4], // blue
];

/// The Layer-colour chip set (CSP's default two-tone palette).
const LAYER_TINTS: [[u8; 3]; 8] = [
    [0x2a, 0x6f, 0xf4], // blue
    [0xe5, 0x4b, 0x4b], // red
    [0x3f, 0xb2, 0x5e], // green
    [0xf2, 0xb8, 0x1c], // amber
    [0x9b, 0x59, 0xd0], // purple
    [0xe8, 0x7e, 0xb5], // pink
    [0x26, 0xc6, 0xc9], // cyan
    [0x8a, 0x8f, 0x98], // grey
];

#[derive(Clone, Copy)]
struct LayerDrag(usize);

// --- the row filter (SL-001..004, CSP's Search Layer) --------------------

/// A resolved snapshot of the palette's filter controls — owned, so the
/// row loop can hold it while `app` is borrowed mutably. Built only when
/// something is actually narrowing; `None` means the palette behaves
/// exactly as it did before the filter existed.
pub(crate) struct LayerFilter {
    /// Lower-cased name substring (SL-004); empty = no name test.
    needle: String,
    kind: LayerFilterKind,
    ref_only: bool,
    no_draft: bool,
    /// SL-003: the active layer's frame folder block (header included).
    frame_scope: Option<std::ops::Range<usize>>,
    /// The scope was ASKED for — with `frame_scope` None that means the
    /// active layer sits in no frame folder, and nothing matches. Kept
    /// separate so the count row can say why instead of showing "0 of N"
    /// with no explanation.
    frame_scope_wanted: bool,
}

impl LayerFilter {
    /// True when this row survives the filter. Index is the DATA index,
    /// so every row action (eye, rename, select, delete) keeps working
    /// on the layer it names.
    pub(crate) fn passes(&self, doc: &mn_core::Document, i: usize) -> bool {
        let Some(l) = doc.layers.get(i) else {
            return false;
        };
        if !self.needle.is_empty() && !l.name.to_lowercase().contains(&self.needle) {
            return false;
        }
        let kind_ok = match self.kind {
            LayerFilterKind::All => true,
            // "Raster" is the leftover: a painted layer is one that is
            // neither a folder nor one of the three vector kinds.
            LayerFilterKind::Raster => {
                !l.folder && !l.is_frame() && !l.is_balloon() && !l.is_text()
            }
            LayerFilterKind::Folder => l.folder,
            LayerFilterKind::Frame => l.is_frame(),
            LayerFilterKind::Balloon => l.is_balloon(),
            LayerFilterKind::Text => l.is_text(),
        };
        if !kind_ok {
            return false;
        }
        if self.ref_only && !l.reference {
            return false;
        }
        if self.no_draft && l.draft {
            return false;
        }
        match &self.frame_scope {
            Some(r) => r.contains(&i),
            None => !self.frame_scope_wanted,
        }
    }
}

/// The frame folder enclosing `active` (or `active` itself when it IS
/// one). Walks parents, so a raster deep inside a panel's sub-folder
/// still finds its koma.
fn active_frame_folder(doc: &mn_core::Document, active: usize) -> Option<usize> {
    let mut i = active;
    loop {
        if doc.layers.get(i).is_some_and(|l| l.folder && l.is_frame()) {
            return Some(i);
        }
        // `enclosing_folder` only ever looks upward (i+1..), so this
        // terminates.
        i = doc.enclosing_folder(i)?;
    }
}

/// Read the palette's filter controls. `None` = nothing is narrowing.
pub(crate) fn build_filter(app: &App) -> Option<LayerFilter> {
    let needle = app.layer_search.trim().to_lowercase();
    let wanted = app.layer_filter_this_frame;
    if needle.is_empty()
        && app.layer_filter_kind == LayerFilterKind::All
        && !app.layer_filter_ref_only
        && !app.layer_filter_no_draft
        && !wanted
    {
        return None;
    }
    Some(LayerFilter {
        needle,
        kind: app.layer_filter_kind,
        ref_only: app.layer_filter_ref_only,
        no_draft: app.layer_filter_no_draft,
        frame_scope: wanted
            .then(|| active_frame_folder(&app.doc, app.doc.active))
            .flatten()
            .map(|f| app.doc.block_range(f)),
        frame_scope_wanted: wanted,
    })
}

// --- layers -------------------------------------------------------------

/// CSP label-colour cycle for the layer rail (colours from the owner's
/// reference screenshot). The Label command icon steps through, then off.
const LABEL_COLORS: [[u8; 3]; 6] = [
    [0x58, 0x6b, 0xf0], // blue
    [0xe5, 0x4b, 0x4b], // red
    [0xf0, 0x8a, 0x3c], // orange
    [0xf2, 0x9a, 0x8a], // salmon
    [0x4b, 0xc4, 0x62], // green
    [0x8a, 0x2f, 0x2f], // dark red
];

// CSP's stack is two text lines tall: "100 % Normal" over the name. The
// extra height is what buys the palette its legibility (owner order
// 2026-08-21: "first do exactly what clip studio does").
const LAYER_ROW_H: f32 = 44.0;

/// The active row's fill. CSP paints the editing row a saturated blue that
/// is unmissable in a 60-layer stack; `theme::SEL_ROW` (kept for the
/// multi-selection) was the "which row is lit?" squint the owner reported.
const SEL_ACTIVE: egui::Color32 = egui::Color32::from_rgb(0x2f, 0x5e, 0x99);

/// The per-type marker a palette row carries beside its thumbnail, CSP's
/// layer-type glyph. `None` = a plain raster layer: the common case stays
/// bare so the marked kinds are the ones that catch the eye.
///
/// Most-specific first, because the kinds overlap in storage: a frame folder
/// is a folder AND a frame, a tone is either painted ink screened (`tone`) or
/// a LIVE fill layer's parameters, and vector inking is a `strokes` set
/// recorded BESIDE an ordinary raster — so it is the last test before bare.
pub(crate) fn row_glyph(l: &mn_core::Layer) -> Option<Icon> {
    if l.folder {
        return Some(if l.is_frame() {
            Icon::Frame
        } else {
            Icon::Folder
        });
    }
    if l.is_text() {
        return Some(Icon::Text);
    }
    if l.is_balloon() {
        return Some(Icon::Balloon);
    }
    if l.is_frame() {
        return Some(Icon::Frame);
    }
    if l.tone.is_some() || matches!(l.kind, LayerKind::Fill(FillKind::Tone { .. })) {
        return Some(Icon::Tone);
    }
    if matches!(l.kind, LayerKind::Fill(_)) {
        return Some(Icon::Fill);
    }
    if l.strokes.is_some() {
        return Some(Icon::Vector);
    }
    None
}

pub(super) fn layer_section(ui: &mut egui::Ui, app: &mut App) {
    // Top strip: the active layer's blend + opacity, exactly CSP's layout.
    let active = app.doc.active;
    if let Some(l) = app.doc.layers.get(active) {
        let (blend, opacity, through, is_folder) = (l.blend, l.opacity, l.through, l.folder);
        ui.horizontal(|ui| {
            let mut pick = None;
            let mut flip_through: Option<bool> = None;
            egui::ComboBox::from_id_salt("mn.blend.active")
                .width(88.0)
                .selected_text(if through {
                    "Through".to_owned()
                } else {
                    blend_name(blend).to_owned()
                })
                .show_ui(ui, |ui| {
                    // LF-002: folders list Through first — it is not a blend
                    // mode but the seal's OFF switch (the stored blend waits
                    // underneath for when Through is turned off again).
                    if is_folder {
                        if ui.selectable_label(through, "Through (no seal)").clicked() {
                            flip_through = Some(true);
                        }
                        ui.separator();
                    }
                    for b in BLENDS {
                        if ui
                            .selectable_label(!through && blend == b, blend_name(b))
                            .clicked()
                        {
                            pick = Some(b);
                        }
                    }
                });
            if let Some(b) = pick {
                app.push_cmd(AppCmd::SetFolderThrough(active, false));
                app.push_cmd(AppCmd::SetLayerBlend(active, b));
            }
            if flip_through == Some(true) {
                app.push_cmd(AppCmd::SetFolderThrough(active, true));
            }
            let mut pct = opacity * 100.0;
            if ValueBar::new("", 0.0, 100.0)
                .suffix("%")
                .width(ui.available_width())
                .show(ui, &mut pct)
                .changed()
            {
                app.push_cmd(AppCmd::SetLayerOpacity(active, pct / 100.0));
            }
        });
    }

    // Two command-icon rows, CSP's layout.
    let s = 16.0;
    let (a_clip, a_lock, a_lock_alpha, a_reference, a_draft) = app
        .doc
        .layers
        .get(active)
        .map(|l| (l.clip, l.lock, l.lock_alpha, l.reference, l.draft))
        .unwrap_or((false, false, false, false, false));
    ui.horizontal(|ui| {
        let label_tip = "Palette colour — cycles through CSP's set, then off";
        if icon_btn(ui, Icon::Label, s, false, true, label_tip).clicked() {
            let cur = app.doc.layers.get(active).and_then(|l| l.label);
            let next = match cur {
                None => Some(LABEL_COLORS[0]),
                Some(c) => LABEL_COLORS
                    .iter()
                    .position(|x| *x == c)
                    .and_then(|i| LABEL_COLORS.get(i + 1))
                    .copied(),
            };
            app.push_cmd(AppCmd::SetLayerLabel(active, next));
        }
        if icon_btn(ui, Icon::Clip, s, a_clip, true, "Clip to layer below").clicked() {
            app.push_cmd(AppCmd::SetLayerClip(active, !a_clip));
        }
        if icon_btn(
            ui,
            Icon::LockAlpha,
            s,
            a_lock_alpha,
            true,
            "Lock transparent pixels",
        )
        .clicked()
        {
            app.push_cmd(AppCmd::SetLayerLockAlpha(active, !a_lock_alpha));
        }
        if icon_btn(ui, Icon::Lock, s, a_lock, true, "Lock layer").clicked() {
            app.push_cmd(AppCmd::SetLayerLock(active, !a_lock));
        }
        if icon_btn(ui, Icon::Reference, s, a_reference, true, "Reference layer").clicked() {
            app.push_cmd(AppCmd::SetLayerReference(active, !a_reference));
        }
        if icon_btn(ui, Icon::Draft, s, a_draft, true, "Draft layer").clicked() {
            app.push_cmd(AppCmd::SetLayerDraft(active, !a_draft));
        }
    });
    ui.horizontal(|ui| {
        if icon_btn(ui, Icon::Plus, s, false, true, "New layer").clicked() {
            app.push_cmd(AppCmd::AddLayer);
        }
        // Same command as Layer ▸ New vector layer, and the same glyph the
        // resulting row carries.
        if icon_btn(
            ui,
            Icon::Vector,
            s,
            false,
            true,
            "New vector layer — strokes record as editable geometry",
        )
        .clicked()
        {
            app.push_cmd(AppCmd::AddVectorLayer);
        }
        if icon_btn(ui, Icon::Frame, s, false, true, "New frame border folder").clicked() {
            app.push_cmd(AppCmd::NewFrameLayer);
        }
        if icon_btn(ui, Icon::Folder, s, false, true, "New folder").clicked() {
            app.push_cmd(AppCmd::AddFolder);
        }
        if icon_btn(ui, Icon::Duplicate, s, false, true, "Duplicate layer").clicked() {
            app.push_cmd(AppCmd::DuplicateLayer);
        }
        if icon_btn(
            ui,
            Icon::MergeDown,
            s,
            false,
            true,
            "Merge with layer below (Ctrl+E)",
        )
        .clicked()
        {
            app.push_cmd(AppCmd::MergeDown);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_btn(ui, Icon::Trash, s, false, true, "Delete layer").clicked() {
                app.push_cmd(AppCmd::RemoveLayer);
            }
        });
    });

    // SL-001..004 (CSP's Search Layer, folded into the palette that
    // already lists the stack rather than a second window listing it
    // again): name substring beside a type + property dropdown. Same
    // shape as the Material palette's search row.
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 94.0).clamp(44.0, 150.0);
        ui.add(
            egui::TextEdit::singleline(&mut app.layer_search)
                .hint_text("filter")
                .desired_width(w),
        )
        .on_hover_text("show only layers whose name contains this (SL-004)");
        egui::ComboBox::from_id_salt("mn.layers.filter")
            .width(84.0)
            .selected_text(app.layer_filter_kind.label())
            .show_ui(ui, |ui| {
                for k in LayerFilterKind::ALL {
                    ui.selectable_value(&mut app.layer_filter_kind, k, k.label());
                }
                ui.separator();
                ui.checkbox(&mut app.layer_filter_ref_only, "reference only");
                ui.checkbox(&mut app.layer_filter_no_draft, "hide drafts");
                ui.checkbox(&mut app.layer_filter_this_frame, "this frame folder")
                    .on_hover_text(
                        "only the koma folder holding the active layer — the one that earns its keep on a 200-layer page",
                    );
            });
    });
    ui.add_space(1.0);

    refresh_layer_thumbs(ui.ctx(), app);

    // The stack, top-first: CSP rows — eye | label strip | pen | thumbnail |
    // "100 % Normal" over the name. Rows drag to reorder.
    struct Row {
        name: String,
        visible: bool,
        opacity: f32,
        blend: Blend,
        /// The palette colour the rail strip paints — the layer's OWN label,
        /// or, for a folder without one, the colour it inherits (PC-002).
        strip: Option<[u8; 3]>,
        is_frame: bool,
        /// The layer-type marker (`row_glyph`); `None` = plain raster.
        glyph: Option<Icon>,
        /// A toned row's screen frequency: the meta line reads "85.0 LPI"
        /// where a plain row reads "100 % Normal" (CSP's tone rows).
        tone_lpi: Option<f32>,
        depth: u8,
        folder: bool,
        open: bool,
        clip: bool,
        /// The clip flag is set but resolves to NO base (`clip_bases`) — the
        /// flag is being ignored and the row should say so, not lie red
        /// (docs/CLIPPING-SCENARIOS.md 5a).
        clip_dangling: bool,
        lock: bool,
        lock_alpha: bool,
        reference: bool,
        draft: bool,
    }
    let clip_bases = app.doc.clip_bases();
    let rows: Vec<Row> = app
        .doc
        .layers
        .iter()
        .enumerate()
        .map(|(i, l)| Row {
            name: l.name.clone(),
            visible: l.visible,
            opacity: l.opacity,
            blend: l.blend,
            // PC-002: a folder with no colour of its own shows the topmost
            // one from inside it (the rule + its edge cases live on
            // `Document::palette_colour`).
            strip: app.doc.palette_colour(i),
            is_frame: l.is_frame(),
            glyph: row_glyph(l),
            tone_lpi: l
                .tone
                .map(|t| t.lpi)
                .or(match &l.kind {
                    LayerKind::Fill(FillKind::Tone { tone, .. }) => Some(tone.lpi),
                    _ => None,
                }),
            depth: l.depth,
            folder: l.folder,
            open: l.open,
            clip: l.clip,
            clip_dangling: l.clip && !l.folder && clip_bases[i].is_none(),
            lock: l.lock,
            lock_alpha: l.lock_alpha,
            reference: l.reference,
            draft: l.draft,
        })
        .collect();
    // Rows inside a collapsed folder are hidden (top-first walk).
    let mut row_hidden = vec![false; rows.len()];
    {
        let mut hide_deeper: Option<u8> = None;
        for i in (0..rows.len()).rev() {
            let r = &rows[i];
            if let Some(d) = hide_deeper {
                if r.depth > d {
                    row_hidden[i] = true;
                    continue;
                }
                hide_deeper = None;
            }
            if r.folder && !r.open {
                hide_deeper = Some(r.depth);
            }
        }
    }
    // SL-001..004: which rows the filter leaves standing. A FILTERED
    // list is FLAT — collapsed folders no longer hide their children,
    // because a filter whose only match sits inside a shut folder reads
    // as a broken filter. The ACTIVE row always shows: a stack where you
    // cannot see the layer your pen is on is worse than one extra row.
    let filter = build_filter(app);
    let filtering = filter.is_some();
    let row_shown: Vec<bool> = match &filter {
        Some(f) => (0..rows.len())
            .map(|i| i == active || f.passes(&app.doc, i))
            .collect(),
        None => row_hidden.iter().map(|h| !h).collect(),
    };
    if let Some(f) = &filter {
        let shown = row_shown.iter().filter(|b| **b).count();
        ui.horizontal(|ui| {
            ui.weak(format!("{shown} of {}", rows.len()));
            if f.frame_scope_wanted && f.frame_scope.is_none() {
                ui.weak("· active layer is in no frame folder");
            } else {
                // The data-loss shape this closes: with rows missing, a
                // drop line drawn between two VISIBLE rows lands the
                // layer somewhere the user cannot see. Reordering is off
                // until the filter is cleared, and it says so rather
                // than silently ignoring the drag.
                ui.weak("· reorder off");
            }
            if ui.small_button("clear").clicked() {
                app.layer_search.clear();
                app.layer_filter_kind = LayerFilterKind::All;
                app.layer_filter_ref_only = false;
                app.layer_filter_no_draft = false;
                app.layer_filter_this_frame = false;
            }
        });
    }

    let mut drop: Option<(usize, usize, u8)> = None;
    ui.spacing_mut().item_spacing.y = 1.0;

    for (i, row) in rows.iter().enumerate().rev() {
        if !row_shown[i] {
            continue;
        }
        let selected = i == active;
        // TC-013: multi-selected rows share the selection fill; the editing
        // pen (below) still marks only the active row, like CSP.
        let multi = selected || app.doc.layer_multi.contains(&i);

        // Inline rename replaces the row with a text edit.
        if matches!(&app.renaming, Some((ri, _)) if *ri == i) {
            let Some((_, text)) = &mut app.renaming else {
                unreachable!()
            };
            let resp = ui.text_edit_singleline(text);
            let done = resp.lost_focus() || ui.input(|inp| inp.key_pressed(egui::Key::Enter));
            if done {
                let (_, text) = app.renaming.take().unwrap();
                if !text.trim().is_empty() {
                    app.push_cmd(AppCmd::RenameLayer(i, text.trim().to_owned()));
                }
            } else {
                resp.request_focus();
            }
            continue;
        }

        let w = ui.available_width();
        let sense = if filtering {
            egui::Sense::click()
        } else {
            egui::Sense::click_and_drag()
        };
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, LAYER_ROW_H), sense);
        let cy = rect.center().y;

        // CSP rail: two full-height cells — eye | editing pen — both FILLED
        // with the layer's palette colour, so a coloured stack reads as
        // solid blocks down the left edge and an uncoloured one stays dark.
        let eye_cell =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 22.0, rect.bottom()));
        let pen_cell = egui::Rect::from_min_max(
            egui::pos2(eye_cell.right(), rect.top()),
            egui::pos2(eye_cell.right() + 20.0, rect.bottom()),
        );
        let pen_col = pen_cell.right();
        let eye = ui
            .interact(eye_cell, resp.id.with("eye"), egui::Sense::click())
            .on_hover_text("show/hide — Alt+click solos this layer");
        // Discoverability (r102 audit): the row's two power gestures had
        // no surface — hover carries them now.
        let resp = resp.on_hover_text(
            "Ctrl+click: add/remove from the selected layers · Shift+click: select range · \
             Ctrl+click the thumbnail: selection from this layer's ink · double-click: rename",
        );

        let p = ui.painter();
        if selected {
            p.rect_filled(rect, 0.0, SEL_ACTIVE);
        } else if multi {
            p.rect_filled(rect, 0.0, theme::SEL_ROW);
        } else if resp.hovered() {
            p.rect_filled(rect, 0.0, theme::HOVER);
        }
        // The rail cells paint over the row fill. With a palette colour the
        // eye cell takes the dimmed shade and the pen cell the full one
        // (CSP's pair); rail icons flip dark when the colour is light.
        let (cell_a, cell_b, rail_icon) = match row.strip {
            Some([r, g, b]) => {
                let dim = egui::Color32::from_rgb(
                    (r as f32 * 0.72) as u8,
                    (g as f32 * 0.72) as u8,
                    (b as f32 * 0.72) as u8,
                );
                let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
                let ic = if lum > 140.0 {
                    egui::Color32::from_rgb(0x16, 0x16, 0x18)
                } else {
                    theme::TEXT_STRONG
                };
                (dim, egui::Color32::from_rgb(r, g, b), ic)
            }
            None => (theme::FIELD, theme::PANEL, theme::TEXT),
        };
        p.rect_filled(eye_cell, 0.0, cell_a);
        p.rect_filled(pen_cell, 0.0, cell_b);
        for x in [eye_cell.left(), eye_cell.right(), pen_cell.right()] {
            p.vline(x, rect.y_range(), egui::Stroke::new(1.0, theme::BORDER));
        }
        let eye_r = egui::Rect::from_center_size(
            egui::pos2(eye_cell.center().x, cy),
            egui::vec2(15.0, 15.0),
        );
        icons::paint(
            p,
            eye_r.shrink(1.5),
            if row.visible { Icon::Eye } else { Icon::EyeOff },
            if row.visible {
                rail_icon
            } else {
                rail_icon.gamma_multiply(0.4)
            },
        );
        // Editing-target pen on the active row (CSP's second rail column).
        if selected {
            let pr = egui::Rect::from_center_size(
                egui::pos2(pen_cell.center().x, cy),
                egui::vec2(13.0, 13.0),
            );
            icons::paint(p, pr, Icon::Pen, rail_icon);
        }

        // Indent nested rows; folders get a disclosure triangle in the gutter.
        // Nested rows also carry CSP's tree guide lines — one vertical under
        // each ancestor's triangle column.
        let indent = row.depth as f32 * 12.0;
        for d in 1..=row.depth as usize {
            let gx = pen_col + 8.0 + (d - 1) as f32 * 12.0;
            p.vline(gx, rect.y_range(), egui::Stroke::new(1.0, theme::BORDER));
        }
        let mut disclose: Option<egui::Rect> = None;
        if row.folder {
            let dr = egui::Rect::from_center_size(
                egui::pos2(pen_col + 8.0 + indent, cy),
                egui::vec2(13.0, 13.0),
            );
            let c = dr.center();
            let tri = if row.open {
                vec![
                    egui::pos2(c.x - 4.0, c.y - 2.0),
                    egui::pos2(c.x + 4.0, c.y - 2.0),
                    egui::pos2(c.x, c.y + 3.5),
                ]
            } else {
                vec![
                    egui::pos2(c.x - 2.0, c.y - 4.0),
                    egui::pos2(c.x + 3.5, c.y),
                    egui::pos2(c.x - 2.0, c.y + 4.0),
                ]
            };
            p.add(egui::Shape::convex_polygon(
                tri,
                if selected {
                    theme::TEXT_STRONG
                } else {
                    theme::TEXT_WEAK
                },
                egui::Stroke::NONE,
            ));
            disclose = Some(dr);
        }
        let thumb_left = pen_col + 7.0 + indent + if row.folder { 14.0 } else { 0.0 };

        // Thumbnail on a checker well, 32 px (CSP-size — after the colour
        // rail, the second thing that tells rows apart at a glance); a plain
        // folder shows the folder glyph until its composite generates.
        let tr = egui::Rect::from_min_size(
            egui::pos2(thumb_left, cy - 16.0),
            egui::vec2(32.0, 32.0),
        );
        let thumb = app
            .layer_thumbs
            .get(i)
            .and_then(|o| o.as_ref())
            .map(|(_, t)| t.clone());
        match &thumb {
            Some(t) => {
                p.image(
                    t.id(),
                    tr,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None if row.folder => {
                // Fallback until the composite thumbnail generates (1/frame).
                p.rect_filled(tr, 2.0, theme::FIELD);
                icons::paint(
                    p,
                    tr.shrink(5.0),
                    Icon::Folder,
                    if selected {
                        theme::TEXT_STRONG
                    } else {
                        theme::TEXT
                    },
                );
            }
            None => {
                p.rect_filled(tr, 2.0, theme::FIELD);
            }
        }
        p.rect_stroke(
            tr,
            2.0,
            egui::Stroke::new(1.0, theme::BORDER),
            egui::StrokeKind::Inside,
        );
        // Clip marker: CSP's red bar down the left edge of the thumbnail.
        // A DANGLING flag (set, but no valid base below — the compositor
        // ignores it) dims to grey so "why is this suddenly unclipped"
        // is answerable at a glance (docs/CLIPPING-SCENARIOS.md 5a).
        if row.clip {
            p.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(tr.left() - 4.0, tr.top()),
                    egui::pos2(tr.left() - 1.5, tr.bottom()),
                ),
                0.0,
                if row.clip_dangling {
                    theme::TEXT_WEAK
                } else {
                    egui::Color32::from_rgb(0xe5, 0x4b, 0x4b)
                },
            );
        }

        // Two text lines, CSP's layout: the meta ("100 % Normal" — or
        // "85.0 LPI" on a toned row) small on top, the NAME big underneath,
        // instead of a name and a far-away right-aligned meta sharing one
        // cramped line. The type glyph leads the meta line (CSP's slot).
        let y_meta = rect.top() + 12.0;
        let y_name = rect.bottom() - 14.0;
        let text_x = tr.right() + 8.0;

        // Right-edge flags on the meta line, CSP order: lock / draft / ref.
        let mut fx = rect.right() - 11.0;
        if row.lock || row.lock_alpha {
            let lr = egui::Rect::from_center_size(egui::pos2(fx, y_meta), egui::vec2(12.0, 12.0));
            icons::paint(
                p,
                lr,
                if row.lock {
                    Icon::Lock
                } else {
                    Icon::LockAlpha
                },
                theme::TEXT_WEAK,
            );
            fx -= 13.0;
        }
        if row.draft {
            let dr = egui::Rect::from_center_size(egui::pos2(fx, y_meta), egui::vec2(12.0, 12.0));
            icons::paint(p, dr, Icon::Draft, theme::TEXT_WEAK);
            fx -= 13.0;
        }
        if row.reference {
            let rr = egui::Rect::from_center_size(egui::pos2(fx, y_meta), egui::vec2(12.0, 12.0));
            icons::paint(p, rr, Icon::Reference, theme::TEXT_WEAK);
            fx -= 13.0;
        }

        let mut meta_x = text_x;
        if let Some(icon) = row.glyph {
            let fr = egui::Rect::from_center_size(
                egui::pos2(meta_x + 6.0, y_meta),
                egui::vec2(12.0, 12.0),
            );
            icons::paint(
                p,
                fr,
                icon,
                if selected {
                    theme::TEXT_STRONG
                } else {
                    theme::TEXT_WEAK
                },
            );
            meta_x = fr.right() + 4.0;
        }
        let meta = match row.tone_lpi {
            Some(lpi) if row.blend == Blend::Normal => format!("{lpi:.1} LPI"),
            Some(lpi) => format!("{lpi:.1} LPI · {}", blend_name(row.blend)),
            None => format!("{:.0} % {}", row.opacity * 100.0, blend_name(row.blend)),
        };
        let meta_col = if selected { theme::TEXT } else { theme::TEXT_WEAK };
        let mut mjob = egui::text::LayoutJob::simple(
            meta,
            egui::FontId::proportional(10.0),
            meta_col,
            f32::INFINITY,
        );
        mjob.wrap = egui::text::TextWrapping::truncate_at_width((fx + 5.0 - meta_x).max(10.0));
        let mgalley = ui.fonts_mut(|f| f.layout_job(mjob));
        p.galley(
            egui::pos2(meta_x, y_meta - mgalley.size().y * 0.5),
            mgalley,
            meta_col,
        );

        // Panel reading order (owner top item 2026-08-18): a numbered
        // badge on frame folders — the COMPUTED position (renumbering
        // only touches default `Frame N` names, so a hand-named folder
        // still shows its number here). "?" = ambiguous layout; the dot
        // marker = manually pinned. Right-click for the pin actions.
        // It rides the name line's right edge; the name ellipsizes first.
        let mut name_right = rect.right() - 8.0;
        if row.folder
            && row.is_frame
            && let Some((pos, amb, pinned)) = app.frame_pos(i)
        {
            let br = egui::Rect::from_center_size(
                egui::pos2(rect.right() - 14.0, y_name),
                egui::vec2(16.0, 13.0),
            );
            name_right = br.left() - 6.0;
            let bg = if amb {
                egui::Color32::from_rgb(196, 158, 46)
            } else if pinned {
                theme::ACCENT
            } else {
                theme::FIELD
            };
            p.rect_filled(br, 3.0, bg);
            p.rect_stroke(
                br,
                3.0,
                egui::Stroke::new(1.0, theme::BORDER),
                egui::StrokeKind::Inside,
            );
            p.text(
                br.center(),
                egui::Align2::CENTER_CENTER,
                if amb {
                    "?".to_owned()
                } else {
                    format!("{pos}")
                },
                egui::FontId::proportional(9.5),
                if amb || pinned {
                    egui::Color32::BLACK
                } else {
                    theme::TEXT_STRONG
                },
            );
            if pinned {
                p.circle_filled(
                    egui::pos2(br.right() - 1.5, br.top() + 1.5),
                    2.0,
                    egui::Color32::WHITE,
                );
            }
            let bresp = ui.interact(br, resp.id.with("framepos"), egui::Sense::click());
            bresp.context_menu(|ui| {
                if ui.button("Read earlier").clicked() {
                    app.frame_pin_step(i, -1);
                    ui.close();
                }
                if ui.button("Read later").clicked() {
                    app.frame_pin_step(i, 1);
                    ui.close();
                }
                if ui
                    .add_enabled(pinned, egui::Button::new("Automatic order"))
                    .clicked()
                {
                    app.frame_pin_clear(i);
                    ui.close();
                }
                // TRIAGE 127 (FB-053/054): the per-frame Draw-border switch,
                // on the folder's own row where CSP puts it.
                ui.separator();
                let ruler = app.doc.layers[i]
                    .frames()
                    .is_some_and(|fs| fs.border_ruler);
                let label = if ruler {
                    "Draw border (ink it again)"
                } else {
                    "Border off — outline becomes a ruler"
                };
                if ui.button(label).clicked() {
                    app.push_cmd(AppCmd::FrameBorderRuler { layer: i });
                    ui.close();
                }
            });
        }

        // The name line. A draft row's name dims (CSP greys the 下書き
        // layer); narrow columns ELLIPSIZE the name, never wrap it.
        let name_color = if row.draft {
            theme::TEXT_WEAK
        } else if selected {
            theme::TEXT_STRONG
        } else {
            theme::TEXT
        };
        let mut job = egui::text::LayoutJob::simple(
            row.name.clone(),
            egui::FontId::proportional(12.5),
            name_color,
            f32::INFINITY,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width((name_right - text_x).max(14.0));
        let name_galley = ui.fonts_mut(|f| f.layout_job(job));
        p.galley(
            egui::pos2(text_x, y_name - name_galley.size().y * 0.5),
            name_galley,
            name_color,
        );
        p.hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0, theme::BORDER),
        );

        let disclose_clicked = disclose.is_some_and(|dr| {
            ui.interact(dr, resp.id.with("fold"), egui::Sense::click())
                .clicked()
        });
        if eye.clicked() {
            if ui.input(|i| i.modifiers.alt) {
                // RF-001's promise (the hover said so since r102; the
                // behaviour arrives r113): Alt+click SOLOs the layer,
                // second press restores.
                app.push_cmd(AppCmd::SetLayerEyeSolo(i));
            } else {
                app.push_cmd(AppCmd::SetLayerVisible(i, !row.visible));
            }
        } else if disclose_clicked {
            app.push_cmd(AppCmd::ToggleFolderOpen(i));
        } else if resp.double_clicked() {
            app.renaming = Some((i, row.name.clone()));
        } else if resp.clicked() && ui.input(|i| i.modifiers.ctrl) {
            // SE-011 vs TC-013, CSP's own split: Ctrl+click on the THUMBNAIL
            // selects the layer's alpha (modifiers combine with the current
            // selection like every other selection gesture); Ctrl+click
            // anywhere else on the row toggles it in the multi-selection.
            let on_thumb = ui
                .ctx()
                .pointer_interact_pos()
                .is_some_and(|pos| tr.contains(pos));
            if on_thumb {
                let m = ui.input(|i| i.modifiers);
                let op = crate::cmd::effective_sel_op(m.shift, m.alt, app.sel_op);
                app.push_cmd(AppCmd::SelectFromLayer(i, op));
            } else {
                app.push_cmd(AppCmd::ToggleLayerMulti(i));
            }
        } else if resp.clicked() && ui.input(|i| i.modifiers.shift) {
            // TC-013: range-select between the active row and this one.
            app.push_cmd(AppCmd::RangeLayerMulti(i));
        } else if resp.clicked() {
            // `SelectLayer` clears the Paper highlight, but a click on the
            // row that is ALREADY active pushes no command — so clear it
            // here too, or the paper stays lit beside the active layer.
            app.paper_selected = false;
            if !selected {
                app.push_cmd(AppCmd::SelectLayer(i));
            }
        }
        if !filtering && resp.drag_started() {
            egui::DragAndDrop::set_payload(ui.ctx(), LayerDrag(i));
        }
        if !filtering && resp.dnd_hover_payload::<LayerDrag>().is_some() {
            let above = ui
                .ctx()
                .pointer_interact_pos()
                .is_some_and(|p| p.y < rect.center().y);
            // Display is top-first (data reversed): dropping above the row of
            // data index i inserts at slot i+1, below it at slot i. Dropping
            // directly under an OPEN folder header drops *into* it (topmost
            // child); under a closed one, below its whole block.
            let (slot, depth) = if above {
                (i + 1, row.depth)
            } else if row.folder && row.open {
                (i, row.depth + 1)
            } else if row.folder {
                (app.doc.children_range(i).start, row.depth)
            } else {
                (i, row.depth)
            };
            let y = if above { rect.top() } else { rect.bottom() };
            ui.painter()
                .hline(rect.x_range(), y, egui::Stroke::new(2.0, theme::ACCENT));
            if let Some(from) = resp.dnd_release_payload::<LayerDrag>() {
                drop = Some((from.0, slot, depth));
            }
        }
    }

    // Paper row: the canvas' white ground, CSP-style at the bottom of the
    // stack. It SELECTS like a layer row (highlight + active row) and nothing
    // more — the paper is not a layer, so `doc.active` never points at it and
    // no downstream code has to know about this.
    {
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, LAYER_ROW_H), egui::Sense::click());
        let resp = resp.on_hover_text("the page's ground — View ▸ Paper sets its colour");
        if resp.clicked() {
            app.paper_selected = true;
        }
        let selected = app.paper_selected;
        let [pr, pg, pb] = app.doc.paper.colour;
        let cy = rect.center().y;
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, 0.0, SEL_ACTIVE);
        } else if resp.hovered() {
            p.rect_filled(rect, 2.0, theme::HOVER);
        }
        // Same rail geometry as a layer row: eye cell | pen cell | content.
        let eye_cell =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 22.0, rect.bottom()));
        let pen_cell = egui::Rect::from_min_max(
            egui::pos2(eye_cell.right(), rect.top()),
            egui::pos2(eye_cell.right() + 20.0, rect.bottom()),
        );
        p.rect_filled(eye_cell, 0.0, theme::FIELD);
        p.rect_filled(pen_cell, 0.0, theme::PANEL);
        for x in [eye_cell.left(), eye_cell.right(), pen_cell.right()] {
            p.vline(x, rect.y_range(), egui::Stroke::new(1.0, theme::BORDER));
        }
        let eye_r = egui::Rect::from_center_size(
            egui::pos2(eye_cell.center().x, cy),
            egui::vec2(15.0, 15.0),
        );
        icons::paint(p, eye_r.shrink(1.5), Icon::Eye, theme::TEXT_WEAK);
        let tr = egui::Rect::from_min_size(
            egui::pos2(pen_cell.right() + 7.0, cy - 16.0),
            egui::vec2(32.0, 32.0),
        );
        // The swatch is the paper's ACTUAL colour, so a cream page reads
        // cream here too.
        p.rect_filled(tr, 2.0, egui::Color32::from_rgb(pr, pg, pb));
        p.rect_stroke(
            tr,
            2.0,
            egui::Stroke::new(1.0, theme::BORDER),
            egui::StrokeKind::Inside,
        );
        let text_col = if selected {
            theme::TEXT_STRONG
        } else {
            theme::TEXT_WEAK
        };
        // Two lines like a layer row: the sheet glyph + role on top, the
        // name underneath.
        let gr = egui::Rect::from_center_size(
            egui::pos2(tr.right() + 14.0, rect.top() + 12.0),
            egui::vec2(12.0, 12.0),
        );
        icons::paint(p, gr, Icon::Paper, theme::TEXT_WEAK);
        p.text(
            egui::pos2(gr.right() + 4.0, rect.top() + 12.0),
            egui::Align2::LEFT_CENTER,
            "the page's ground",
            egui::FontId::proportional(10.0),
            theme::TEXT_WEAK,
        );
        p.text(
            egui::pos2(tr.right() + 8.0, rect.bottom() - 14.0),
            egui::Align2::LEFT_CENTER,
            "Paper",
            egui::FontId::proportional(12.5),
            text_col,
        );
    }

    if let Some((from, slot, depth)) = drop {
        app.push_cmd(AppCmd::MoveLayer { from, slot, depth });
    }
}

/// Rebuild stale per-layer thumbnails (32x32, sampled over a checkerboard so
/// transparency reads CSP-style).
fn refresh_layer_thumbs(ctx: &egui::Context, app: &mut App) {
    let n = app.doc.layers.len();
    app.layer_thumbs.resize_with(n, || None);
    for i in 0..n {
        // A folder's thumbnail is its children composited, so its cache key
        // must move when any child's content does, not just its own raster.
        let rev = {
            let l = &app.doc.layers[i];
            let mut r = l.max_revision();
            if l.folder {
                for k in app.doc.children_range(i) {
                    r = r.max(app.doc.layers[k].max_revision());
                }
            }
            r
        };
        let stale = app.layer_thumbs[i].as_ref().is_none_or(|(r, _)| *r != rev);
        if !stale {
            continue;
        }
        let img = layer_thumb_image(&app.doc, i);
        let tex = ctx.load_texture(
            format!("mn.layer.thumb.{i}"),
            img,
            egui::TextureOptions::LINEAR,
        );
        app.layer_thumbs[i] = Some((rev, tex));
    }
}

fn layer_thumb_image(doc: &mn_core::Document, li: usize) -> egui::ColorImage {
    const TW: usize = 32;
    const TH: usize = 32;
    let (w, h) = doc.size;
    let layer = &doc.layers[li];
    // A folder shows its visible children composited (CSP folder thumbs);
    // the folder's own raster (a frame folder's border ink) draws on top.
    let mut srcs: Vec<usize> = if layer.folder {
        doc.children_range(li).collect()
    } else {
        vec![li]
    };
    srcs.push(li);
    let mut px = Vec::with_capacity(TW * TH * 4);
    for ty in 0..TH {
        for tx in 0..TW {
            let cx = ((tx as f32 + 0.5) / TW as f32 * w as f32) as i32;
            let cy = ((ty as f32 + 0.5) / TH as f32 * h as f32) as i32;
            let idx = mn_core::TileIdx::of_pixel(cx, cy);
            let (ox, oy) = idx.origin();
            // Composite the stack bottom-up in premultiplied space.
            let mut acc = [0.0f32; 4];
            for &si in srcs.iter().rev() {
                let l = &doc.layers[si];
                if si != li && !l.visible {
                    continue;
                }
                let p = l
                    .tile(idx)
                    .map(|t| t.pixel((cx - ox) as usize, (cy - oy) as usize))
                    .unwrap_or([0; 4]);
                let sa = p[3] as f32 / 32768.0;
                for c in 0..3 {
                    acc[c] = p[c] as f32 / 32768.0 + acc[c] * (1.0 - sa);
                }
                acc[3] = sa + acc[3] * (1.0 - sa);
            }
            let a = (acc[3] * 32768.0).round() as u32;
            let bg: u32 = if ((tx / 5) + (ty / 5)) % 2 == 0 {
                214
            } else {
                176
            };
            let ch = |c: f32| {
                (((c * 32768.0).round() as u32 * 255 + (32768 - a) * bg + 16384) / 32768) as u8
            };
            px.extend_from_slice(&[ch(acc[0]), ch(acc[1]), ch(acc[2]), 255]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([TW, TH], &px)
}

pub(super) fn blend_name(b: Blend) -> &'static str {
    match b {
        Blend::Normal => "Normal",
        Blend::Multiply => "Multiply",
        Blend::Screen => "Screen",
        Blend::Darken => "Darken",
        Blend::Lighten => "Lighten",
        Blend::Add => "Add",
        Blend::Subtract => "Subtract",
        Blend::Overlay => "Overlay",
        Blend::SoftLight => "Soft light",
        Blend::HardLight => "Hard light",
        Blend::Difference => "Difference",
        Blend::Exclusion => "Exclusion",
        Blend::Hue => "Hue",
        Blend::Saturation => "Saturation",
        Blend::Color => "Color",
        Blend::ColorBurn => "Color burn",
        Blend::LinearBurn => "Linear burn",
        Blend::ColorDodge => "Color dodge",
        Blend::GlowDodge => "Glow dodge",
        Blend::VividLight => "Vivid light",
        Blend::LinearLight => "Linear light",
        Blend::PinLight => "Pin light",
        Blend::HardMix => "Hard mix",
        Blend::Divide => "Divide",
        Blend::DarkerColor => "Darker color",
        Blend::LighterColor => "Lighter color",
        // CSP's label. Photoshop/SVG call the same operator Luminosity;
        // the enum and the ORA name use theirs, the owner sees CSP's.
        Blend::Luminosity => "Brightness",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mn_core::{Document, FrameSet, TextSet};

    fn plain(kind: LayerFilterKind, needle: &str) -> LayerFilter {
        LayerFilter {
            needle: needle.to_owned(),
            kind,
            ref_only: false,
            no_draft: false,
            frame_scope: None,
            frame_scope_wanted: false,
        }
    }

    /// A stack with one of each thing the filter can name.
    fn stack() -> Document {
        let mut d = Document::new(200, 200);
        d.layers[0].name = "rough sketch".into();
        d.layers[0].draft = true;
        d.add_text_layer("Dialogue", TextSet { texts: Vec::new() });
        d.layers.last_mut().unwrap().reference = true;
        d
    }

    /// SL-004 + SL-001: the name test is a case-insensitive substring and
    /// the type test names the layer kinds, not their storage.
    #[test]
    fn filter_matches_by_name_and_type() {
        let d = stack();
        let f = plain(LayerFilterKind::All, "DIALOG".to_lowercase().as_str());
        let hits: Vec<usize> = (0..d.layers.len()).filter(|&i| f.passes(&d, i)).collect();
        assert_eq!(hits.len(), 1, "one name match");
        assert!(d.layers[hits[0]].is_text());

        let f = plain(LayerFilterKind::Text, "");
        assert_eq!(
            (0..d.layers.len()).filter(|&i| f.passes(&d, i)).count(),
            1,
            "one text layer"
        );
        let f = plain(LayerFilterKind::Raster, "");
        let hits: Vec<usize> = (0..d.layers.len()).filter(|&i| f.passes(&d, i)).collect();
        assert_eq!(hits, vec![0], "raster = neither folder nor vector kind");
    }

    /// SL-002/SL-003: the two property narrowings, and the fact that they
    /// AND with the rest rather than replacing it.
    #[test]
    fn filter_narrows_by_property() {
        let d = stack();
        let mut f = plain(LayerFilterKind::All, "");
        f.no_draft = true;
        assert!(!f.passes(&d, 0), "the draft row is excluded");
        assert!(f.passes(&d, 1));

        let mut f = plain(LayerFilterKind::All, "");
        f.ref_only = true;
        assert!(!f.passes(&d, 0));
        assert!(f.passes(&d, 1), "the reference row survives");

        // AND, not OR: a reference row whose name misses still fails.
        let mut f = plain(LayerFilterKind::All, "zzz");
        f.ref_only = true;
        assert!(!f.passes(&d, 1));
    }

    /// The owner's r-round eye test: every row type must LOOK different.
    /// Pins the marker each kind resolves to, and the two overlaps that
    /// decide the order — a frame FOLDER is both, and a tone/vector layer
    /// is an ordinary raster with something recorded beside it.
    #[test]
    fn every_layer_kind_gets_its_own_glyph() {
        let mut d = Document::new(200, 200);
        assert_eq!(row_glyph(&d.layers[0]), None, "a plain raster stays bare");

        let li = d.add_layer("Vector 1");
        d.layers[li].strokes = Some(mn_core::StrokeSet::default());
        assert_eq!(row_glyph(&d.layers[li]), Some(Icon::Vector));

        let li = d.add_layer("Tone 1");
        d.layers[li].tone = Some(mn_core::tone::ToneParams::default());
        assert_eq!(row_glyph(&d.layers[li]), Some(Icon::Tone));
        // A tone that also records strokes is still a TONE: the screen is
        // what the row shows on the canvas.
        d.layers[li].strokes = Some(mn_core::StrokeSet::default());
        assert_eq!(row_glyph(&d.layers[li]), Some(Icon::Tone));

        let li = d.add_layer("Flat");
        d.layers[li].kind = LayerKind::Fill(FillKind::Flat {
            color: [0.0, 0.0, 0.0, 1.0],
        });
        assert_eq!(row_glyph(&d.layers[li]), Some(Icon::Fill));
        // A LIVE fill layer whose parameters ARE a screentone reads as one.
        d.layers[li].kind = LayerKind::Fill(FillKind::Tone {
            tone: mn_core::tone::ToneParams::default(),
            density: 0.5,
        });
        assert_eq!(row_glyph(&d.layers[li]), Some(Icon::Tone));

        d.add_text_layer("Dialogue", TextSet { texts: Vec::new() });
        assert_eq!(row_glyph(d.layers.last().unwrap()), Some(Icon::Text));
        d.add_balloon_layer(
            "Bubbles",
            mn_core::BalloonSet {
                balloons: Vec::new(),
                border_px: 2.0,
                pressure_width: false,
            },
        );
        assert_eq!(row_glyph(d.layers.last().unwrap()), Some(Icon::Balloon));

        // A frame folder is a folder AND a frame — the koma marker wins.
        let hdr = d.add_frame_folder("Frame 1", FrameSet::single_rect([1.0, 1.0, 9.0, 9.0], 2.0));
        assert_eq!(row_glyph(&d.layers[hdr]), Some(Icon::Frame));
        let plain = d.add_folder_above(hdr, "Group");
        assert_eq!(row_glyph(&d.layers[plain]), Some(Icon::Folder));
    }

    /// SL-003's manga row: the scope is the frame folder BLOCK — header
    /// plus every child — and the walk finds it from a layer nested
    /// inside, not only from the header itself.
    #[test]
    fn frame_folder_scope_covers_the_block() {
        let mut d = Document::new(200, 200);
        let hdr = d.add_frame_folder("Frame 1", FrameSet::single_rect([10.0, 10.0, 90.0, 90.0], 2.0));
        // add_frame_folder leaves the folder's own draw layer active.
        let inside = d.active;
        assert!(inside < hdr, "the draw layer is a child of the header");
        assert_eq!(
            active_frame_folder(&d, inside),
            Some(hdr),
            "walked up from a child"
        );
        assert_eq!(
            active_frame_folder(&d, hdr),
            Some(hdr),
            "the header is its own folder"
        );

        let block = d.block_range(hdr);
        let f = LayerFilter {
            needle: String::new(),
            kind: LayerFilterKind::All,
            ref_only: false,
            no_draft: false,
            frame_scope: Some(block.clone()),
            frame_scope_wanted: true,
        };
        for i in 0..d.layers.len() {
            assert_eq!(
                f.passes(&d, i),
                block.contains(&i),
                "layer {i} inside the block?"
            );
        }

        // Asked for a scope, no frame folder above the active layer: the
        // filter matches NOTHING, deliberately — the count row explains.
        let flat = Document::new(64, 64);
        assert_eq!(active_frame_folder(&flat, 0), None);
        let f = LayerFilter {
            needle: String::new(),
            kind: LayerFilterKind::All,
            ref_only: false,
            no_draft: false,
            frame_scope: None,
            frame_scope_wanted: true,
        };
        assert!(!f.passes(&flat, 0));
    }
}
