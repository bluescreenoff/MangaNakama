//! The Layer Property panel (`LP-…`): the per-type sections, the tool
//! navigation strip and the save-as-default footer. Moved here verbatim
//! when `layers.rs` was split; the pane entry still calls
//! [`super::layer_property`], which is this module's `layer_property`
//! re-exported from the parent.

use super::super::icons::Icon;
use super::super::theme;
use super::super::theme::ValueBar;
use super::super::widgets::{group_caption, icon_btn, px_mm_text};
use super::{BLENDS, LAYER_TINTS, blend_name, blendif, breakout, tools_for_layer};
use crate::app::App;
use crate::cmd::{AppCmd, Tool};

/// The strip icon for a tool — the same table the tool palette draws from,
/// so a tool never wears two glyphs.
pub(crate) fn tool_icon(t: Tool) -> Option<Icon> {
    super::super::tools::STRIP_TOOLS
        .iter()
        .find(|(tool, _)| *tool == t)
        .map(|(_, icon)| *icon)
}

/// `LP-025`'s bar. Clicking pushes the same `SetTool` the tool palette
/// pushes, so sub-tool memory (each tool's remembered brush) behaves
/// identically whichever surface you reach the tool from.
fn tool_nav(ui: &mut egui::Ui, app: &mut App) {
    let Some(l) = app.doc.layers.get(app.doc.active) else {
        return;
    };
    let tools = tools_for_layer(l);
    if tools.is_empty() {
        ui.weak("folders organise layers — pick a layer inside to draw");
        return;
    }
    let cur = app.tool;
    let mut pick = None;
    ui.horizontal(|ui| {
        for &t in tools {
            let Some(icon) = tool_icon(t) else { continue };
            if icon_btn(ui, icon, 20.0, cur == t, true, t.label()).clicked() {
                pick = Some(t);
            }
        }
    });
    if let Some(t) = pick {
        app.push_cmd(AppCmd::SetTool(t));
    }
}

/// CSP's Layer Property: what the *active layer* is. Frame layers expose the
/// border thickness (one undo step per drag); raster layers get blend +
/// opacity, handy when the Layers palette is collapsed or floated away.
pub(crate) fn layer_property(ui: &mut egui::Ui, app: &mut App) {
    let i = app.doc.active;
    let Some(l) = app.doc.layers.get(i) else {
        return;
    };
    let name = l.name.clone();
    let (blend, opacity) = (l.blend, l.opacity);
    let frames = l.frames().cloned();
    let balloons = l.balloons().cloned();
    let tone = l.tone;
    // LIVE layers (fill / gradient / tone) carry their picture as
    // parameters, not pixels — a different block below.
    let live = match l.kind {
        mn_core::LayerKind::Fill(k) => Some(k),
        _ => None,
    };

    ui.label(
        egui::RichText::new(name)
            .size(11.5)
            .color(theme::c().text_strong),
    );
    let (mut reference, mut draft) = (l.reference, l.draft);
    // LP-025, first thing under the name: what this layer type is for.
    tool_nav(ui, app);
    match (frames, balloons) {
        (Some(fs), _) => {
            group_caption(ui, "Frame border");
            let px_per_mm = app.mm_to_px(1.0).max(0.001);
            let mut mm = app.border_edit.unwrap_or(fs.border_px / px_per_mm);
            let resp = ValueBar::new("Thickness", 0.1, 3.0)
                .decimals(2)
                .display_text(px_mm_text(mm, app.page_dpi()))
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

            // A LIVE layer's screen IS its parameters, so this palette shows
            // those parameters — the same rows the Tool Property draws, from
            // the same function, so the two doors cannot drift and a drag
            // through either is one undo press. CSP's Tone layers page sends
            // you exactly here: "You can change the detailed settings of the
            // tone layer in the Layer Properties palette."
            //
            // The raster Effect ▸ Tone combo is deliberately NOT offered here:
            // it screens the layer's PAINTED pixels, and `Document::set_tone`
            // refuses every non-raster kind, so on a live layer the combo was
            // a dead control that reported "None" about a layer made of dots.
            if let Some(k) = live {
                group_caption(
                    ui,
                    match k {
                        mn_core::FillKind::Tone { .. } => "Tone",
                        mn_core::FillKind::Gradient { .. } => "Gradient",
                        mn_core::FillKind::Flat { .. } => "Fill",
                    },
                );
                super::super::property::sec_live_fill(ui, app);
            } else {
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
                                    if ui.selectable_label(p.pattern == pat, pat.label()).clicked()
                                    {
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

    // FB-overflow, both parts (the flag, the mask cap, the seat) — see
    // `breakout.rs` for why the seat is one marker and not per-row ticks.
    breakout::section(ui, app, i);

    // Blend If: show this layer only where the composite BELOW it is in a
    // brightness range. Its own file — see `blendif.rs` for the scope ruling
    // (one arm, not the split-channel matrix) and the undo coalescing.
    blendif::section(ui, app, i);

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
                let c = if on {
                    sub.or(Some(LAYER_TINTS[3]))
                } else {
                    None
                };
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
            if ui.checkbox(&mut on, label).on_hover_text(tip).changed() {
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
            // LP-004: the solid keyline, or a pale stain rim whose colour
            // comes from the layer's own ink.
            let mut style_pick = None;
            ui.horizontal(|ui| {
                ui.weak("Style");
                egui::ComboBox::from_id_salt("mn.edge.style")
                    .selected_text(match p.style {
                        mn_core::edge::EdgeStyle::Solid => "Outline",
                        mn_core::edge::EdgeStyle::Watercolour => "Watercolour",
                    })
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for (v, l) in [
                            (mn_core::edge::EdgeStyle::Solid, "Outline"),
                            (mn_core::edge::EdgeStyle::Watercolour, "Watercolour"),
                        ] {
                            if ui.selectable_label(p.style == v, l).clicked() && p.style != v {
                                style_pick = Some(v);
                            }
                        }
                    })
                    .response
                    .on_hover_text(
                        "watercolour: a paler rim whose colour is sampled from the \
                         layer's own ink — a stain, not a line",
                    );
            });
            if let Some(v) = style_pick {
                p.style = v;
            }
            if resp.changed() || style_pick.is_some() {
                app.edge_edit = Some(p);
            }
            // Commit once, when the interaction ends. Every change re-derives
            // the whole layer's outline, so a per-frame commit would be both
            // an undo-stack spill and a visible stutter.
            if resp.drag_stopped() || (resp.changed() && !resp.dragged()) || style_pick.is_some()
            {
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
                    if ui.selectable_label(expr == e, expression_name(e)).clicked() {
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

    // LP-001 Save as default. CSP files this under the Layer Properties
    // palette MENU; we have no palette-level menu (the ≡ in this app is a
    // per-ROW popup in the stack list, and putting a whole-type action
    // there would read as "this row"), so it lives at the foot of the panel
    // whose contents it saves — visible state instead of a hidden item,
    // with what is stored spelled out under the buttons.
    defaults_section(ui, app, i);
}

/// The `LP-001` footer: save, forget, and a line saying what is stored for
/// this layer type. Only for the types whose creation path reads a default
/// (`layer_defaults::applies_to`) — offering it on a text or balloon layer
/// would save something nothing ever reads.
fn defaults_section(ui: &mut egui::Ui, app: &mut App, i: usize) {
    use crate::app::layer_defaults as ld;
    let Some(key) = app.doc.layers.get(i).map(ld::kind_key) else {
        return;
    };
    if !ld::applies_to(key) {
        return;
    }
    let saved = app.layer_defaults.summary(key);
    ui.add_space(3.0);
    group_caption(ui, "New-layer defaults");
    ui.horizontal(|ui| {
        if ui
            .small_button("Save as default")
            .on_hover_text(format!(
                "new {} start with this layer's blend, opacity and effects.\n\
                 Not its name, visibility, locks, clipping, reference or draft flag.\n{}",
                ld::kind_label(key),
                ld::path_hint()
            ))
            .clicked()
        {
            app.push_cmd(AppCmd::SaveLayerDefaults);
        }
        if saved.is_some()
            && ui
                .small_button("Forget")
                .on_hover_text(format!("new {} start stock again", ld::kind_label(key)))
                .clicked()
        {
            app.push_cmd(AppCmd::ForgetLayerDefaults);
        }
        // Owner ruling 2026-08-30. Only where a tone can land at all — on
        // a folder or a vector layer `apply` refuses tones outright, so the
        // switch would be a control over nothing.
        let toneable = app
            .doc
            .layers
            .get(i)
            .is_some_and(|l| !l.folder && !l.is_vector());
        if toneable {
            let mut inc = app.layer_defaults.include_tone(key);
            if ui
                .checkbox(&mut inc, "Include tone")
                .on_hover_text(format!(
                    "on: an applied screentone is part of the saved default, so new {0} \
                     come out already screened (Clip Studio's behaviour).\n\
                     off: the default is blend, opacity and effects only — the tone stays \
                     on this layer.\n\
                     Remembered per layer type, in {1}",
                    ld::kind_label(key),
                    ld::path_hint()
                ))
                .changed()
            {
                app.push_cmd(AppCmd::SetLayerDefaultsIncludeTone(inc));
            }
        }
    });
    match saved {
        Some(what) => ui.weak(format!("saved for {}: {what}", ld::kind_label(key))),
        None => ui.weak(format!("new {} start stock", ld::kind_label(key))),
    };
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
