//! The Layers palette (the big one) + Layer Property: full stack rows
//! (eye, label rail, thumbnail, rename, folder blocks, drag-reorder via
//! the LayerDrag payload), blend/opacity strip, thumbnails over a
//! checkerboard, and the Layer Property palette body.

use super::icons::Icon;
use super::theme;
use super::theme::ValueBar;
use super::widgets::{group_caption, icon_btn, icon_btn_tint, paint_icon, px_mm_text};
use crate::app::{App, LayerFilterKind};
use crate::cmd::{AppCmd, Tool};
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

/// `LP-025` Tool navigation: the tools that MEAN something on this layer
/// type, in the order a page is made. Every entry is one the layer will
/// actually accept — the mapping is the guards read forwards:
/// `Layer::paintable` for the paint family, `App::live_fill_active` for the
/// two live kinds (a brush there edits the layer's window mask), and
/// `App::guard_frame_layer`'s refusals for the vector kinds, each of which
/// keeps its own editor plus the Object tool.
///
/// **Layer-agnostic tools are deliberately absent** — Select, Wand,
/// Eyedropper and Pan work the same on every layer in the stack, so listing
/// them here would put four cells in every list and teach nothing. This bar
/// answers "what is different about THIS layer", which is the only question
/// a per-type bar can answer.
pub(super) fn tools_for_layer(l: &mn_core::Layer) -> &'static [Tool] {
    if l.folder {
        // A frame folder is still divisible and still an object; a plain
        // folder holds no pixels at all.
        return if l.is_frame() {
            &[Tool::Frame, Tool::Object]
        } else {
            &[]
        };
    }
    match l.kind {
        LayerKind::Text(_) => &[Tool::Text, Tool::Object],
        // The balloon's text is edited in place, so T belongs here too.
        LayerKind::Balloon(_) => &[Tool::Balloon, Tool::Text, Tool::Object],
        LayerKind::Frame(_) => &[Tool::Frame, Tool::Object],
        // Live layers: a brush edits the WINDOW, and the parameters live in
        // Tool Property rather than in a tool of their own.
        LayerKind::Fill(_) | LayerKind::Correction(_) => &[Tool::Pen, Tool::Eraser],
        // Vector inking: strokes are captured (inking does not ask
        // `paintable`), and Object is what edits the control points. The
        // pixel ops that `paintable` refuses are correctly absent.
        LayerKind::Raster if l.records_strokes() => &[Tool::Pen, Tool::Eraser, Tool::Object],
        LayerKind::Raster => &[
            Tool::Pen,
            Tool::Eraser,
            Tool::Fill,
            Tool::Gradient,
            Tool::Tone,
            Tool::Figure,
            Tool::Liquify,
        ],
    }
}

/// The strip icon for a tool — the same table the tool palette draws from,
/// so a tool never wears two glyphs.
fn tool_icon(t: Tool) -> Option<Icon> {
    super::tools::STRIP_TOOLS
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
                            let picked =
                                std::mem::discriminant(&p.density) == std::mem::discriminant(&d);
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

/// The Layer-colour chip set (CSP's default two-tone palette). The FIRST
/// entry is also what `AppCmd::ActiveLayer(ToggleColour)` turns a layer on
/// with, so the keyboard and the palette checkbox agree on "on".
pub(crate) const LAYER_TINTS: [[u8; 3]; 8] = [
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
    /// Only rows wearing exactly this palette colour — their OWN label,
    /// not one inherited from a folder (the inherited tint is display
    /// convenience; the label is what the artist assigned).
    label: Option<[u8; 3]>,
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
        if let Some(c) = self.label
            && l.label != Some(c)
        {
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
        && app.layer_filter_label.is_none()
        && !wanted
    {
        return None;
    }
    Some(LayerFilter {
        needle,
        kind: app.layer_filter_kind,
        ref_only: app.layer_filter_ref_only,
        no_draft: app.layer_filter_no_draft,
        label: app.layer_filter_label,
        frame_scope: wanted
            .then(|| active_frame_folder(&app.doc, app.doc.active))
            .flatten()
            .map(|f| app.doc.block_range(f)),
        frame_scope_wanted: wanted,
    })
}

// --- layers -------------------------------------------------------------

/// CSP's standard palette-colour set for the layer rail (colours from the
/// owner's reference screenshot). Offered as swatches in the colour popup
/// beside a full picker (owner 2026-08-21: no click chain, the wheel is
/// right there).
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

/// Folder rows run THINNER than layer rows — CSP does this and the owner
/// called it out: a folder is a heading, not content, and the shorter row
/// makes the stack's structure readable at a glance.
const FOLDER_ROW_H: f32 = 34.0;

/// The status column between the colour rail and the tree gutter: the
/// reference / draft marks live HERE, in their own fixed slot (CSP gives
/// layer status its own column; the owner asked for the same instead of
/// the old right-edge badges).
const FLAG_COL_W: f32 = 16.0;

/// Header strip: the palette-colour chip and the numeric opacity field that
/// sits beside the slider (CSP's `[colour ▾][Normal ▾][slider][100 ⌃⌄]`).
const CHIP_W: f32 = 15.0;
const SPIN_W: f32 = 44.0;

/// The mask thumbnail beside the layer thumbnail — CSP's second image cell,
/// present only on masked rows.
const MASK_THUMB: f32 = 20.0;

/// The row's right-edge menu column (the hover-visible ≡). Reserved on
/// EVERY row even though the glyph only paints on hover: a column that
/// appeared under the pointer would reflow the name mid-read.
const ROW_MENU_W: f32 = 15.0;

// The status marks (`ref_mark`/`draft_mark`), the active row's fill
// (`sel_active`) and its 1px edge lines (`sel_edge`) used to be four private
// consts here. They are Theme fields now, so a theme can move them together
// with everything else — see `ui/theme.rs`.

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

/// One palette row's worth of layer state, snapshotted before the row loop
/// so the painter can hold it while `app` is borrowed mutably.
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
    /// `Some(enabled)` when the layer carries a mask (LM-001..009). The row
    /// paints a second thumbnail for it — CSP's mask cell — and a cross
    /// through it when the mask is kept but switched off.
    mask: Option<bool>,
}

/// The row's ≡ menu: the per-row half of the layer commands, all of them
/// indexed at THIS row rather than the active layer, so a menu opened on a
/// row you have not selected still does what it says. The ones that only
/// exist as active-layer commands (duplicate, merge, delete, the mask
/// family) select the row first — commands run in queue order.
fn row_menu(ui: &mut egui::Ui, app: &mut App, i: usize, row: &Row) {
    let select_first = |app: &mut App| {
        if app.doc.active != i {
            app.push_cmd(AppCmd::SelectLayer(i));
        }
    };
    if ui.button("Rename…").clicked() {
        app.renaming = Some((i, row.name.clone()));
        ui.close();
    }
    if ui.button("Duplicate layer").clicked() {
        select_first(app);
        app.push_cmd(AppCmd::DuplicateLayer);
        ui.close();
    }
    if ui.button("Merge with layer below").clicked() {
        select_first(app);
        app.push_cmd(AppCmd::MergeDown);
        ui.close();
    }
    ui.separator();
    let mark = |on: bool, s: &str| if on { format!("✓ {s}") } else { s.to_owned() };
    if ui.button(mark(row.clip, "Clip to layer below")).clicked() {
        app.push_cmd(AppCmd::SetLayerClip(i, !row.clip));
        ui.close();
    }
    if ui
        .button(mark(row.lock_alpha, "Lock transparent pixels"))
        .clicked()
    {
        app.push_cmd(AppCmd::SetLayerLockAlpha(i, !row.lock_alpha));
        ui.close();
    }
    if ui.button(mark(row.lock, "Lock layer")).clicked() {
        app.push_cmd(AppCmd::SetLayerLock(i, !row.lock));
        ui.close();
    }
    ui.separator();
    if ui.button(mark(row.reference, "Reference layer")).clicked() {
        app.push_cmd(AppCmd::SetLayerReference(i, !row.reference));
        ui.close();
    }
    if ui.button(mark(row.draft, "Draft layer")).clicked() {
        app.push_cmd(AppCmd::SetLayerDraft(i, !row.draft));
        ui.close();
    }
    if ui.button("Layer colour…").clicked() {
        app.layer_colour_pick = Some(i);
        ui.close();
    }
    ui.separator();
    match row.mask {
        None => {
            if ui.button("Create mask (all visible)").clicked() {
                select_first(app);
                app.push_cmd(AppCmd::MaskSelection);
                ui.close();
            }
        }
        Some(enabled) => {
            if ui.button("Edit mask").clicked() {
                select_first(app);
                if !app.mask_edit {
                    app.push_cmd(AppCmd::MaskEdit);
                }
                ui.close();
            }
            if ui
                .button(if enabled {
                    "Mask off (keep)"
                } else {
                    "Mask on"
                })
                .clicked()
            {
                select_first(app);
                app.push_cmd(AppCmd::MaskToggle);
                ui.close();
            }
            if ui.button("Delete mask").clicked() {
                select_first(app);
                app.push_cmd(AppCmd::MaskDelete);
                ui.close();
            }
        }
    }
    ui.separator();
    if ui.button("Delete layer").clicked() {
        select_first(app);
        app.push_cmd(AppCmd::RemoveLayer);
        ui.close();
    }
}

pub(super) fn layer_section(ui: &mut egui::Ui, app: &mut App) {
    // Top strip: the active layer's blend + opacity, exactly CSP's layout.
    let active = app.doc.active;
    if let Some(l) = app.doc.layers.get(active) {
        let (blend, opacity, through, is_folder) = (l.blend, l.opacity, l.through, l.folder);
        let label = l.label;
        ui.horizontal(|ui| {
            // CSP's header strip is [palette-colour ▾][Normal ▾][opacity
            // slider][100 ⌃⌄]. The widths are budgeted from the space the
            // palette actually has: docked at ~200 pt the blend combo gives
            // ground so the numeric field keeps its digits.
            let avail = ui.available_width();
            let sp = ui.spacing().item_spacing.x;
            let combo_w = if avail < 226.0 { 62.0 } else { 88.0 };
            let bar_w = (avail - CHIP_W - combo_w - SPIN_W - sp * 3.0).max(26.0);

            // The palette-colour chip: the active layer's label colour, or a
            // hollow well when it has none. It opens the SAME one-click
            // Layer-colour window as the Label button and a row's colour
            // cell — no second command, no second list of swatches.
            let (chip, chip_resp) =
                ui.allocate_exact_size(egui::vec2(CHIP_W, 15.0), egui::Sense::click());
            match label {
                Some([r, g, b]) => {
                    ui.painter()
                        .rect_filled(chip, 2.0, egui::Color32::from_rgb(r, g, b));
                }
                None => {
                    ui.painter().rect_filled(chip, 2.0, theme::c().field);
                    ui.painter().line_segment(
                        [chip.left_bottom(), chip.right_top()],
                        egui::Stroke::new(1.0, theme::c().text_weak),
                    );
                }
            }
            ui.painter().rect_stroke(
                chip,
                2.0,
                egui::Stroke::new(1.0, theme::c().border),
                egui::StrokeKind::Inside,
            );
            if chip_resp
                .on_hover_text("Palette colour — swatches + picker")
                .clicked()
            {
                app.layer_colour_pick = match app.layer_colour_pick {
                    Some(_) => None,
                    None => Some(active),
                };
            }

            let mut pick = None;
            let mut flip_through: Option<bool> = None;
            egui::ComboBox::from_id_salt("mn.blend.active")
                .width(combo_w)
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
            // Slider AND spinner over one value: CSP's header carries both,
            // and the number is the half that lets you type 63 instead of
            // hunting for it on a 40 px track.
            let mut pct = opacity * 100.0;
            let bar = ValueBar::new("", 0.0, 100.0)
                .suffix("%")
                .width(bar_w)
                .show(ui, &mut pct);
            let spin = ui.add_sized(
                [SPIN_W, 17.0],
                egui::DragValue::new(&mut pct)
                    .range(0.0..=100.0)
                    .speed(0.5)
                    .max_decimals(0)
                    .suffix(" %"),
            );
            if bar.changed() || spin.changed() {
                app.push_cmd(AppCmd::SetLayerOpacity(active, pct / 100.0));
            }
        });
    }

    // Toggle strip + ONE command row, CSP's layout. Sizes ride the
    // `palette_icon_px` preference (owner 2026-08-21: bigger by default,
    // adjustable in Preferences ▸ Interface).
    let cmd_s = app.prefs.palette_icon_px;
    let s = (cmd_s * 0.8).max(14.0);
    let (a_clip, a_lock, a_lock_alpha, a_reference, a_draft) = app
        .doc
        .layers
        .get(active)
        .map(|l| (l.clip, l.lock, l.lock_alpha, l.reference, l.draft))
        .unwrap_or((false, false, false, false, false));
    ui.horizontal(|ui| {
        let label_tip = "Palette colour — swatches + picker (also: click a row's status cell)";
        if icon_btn(ui, Icon::Label, s, false, true, label_tip).clicked() {
            app.layer_colour_pick = match app.layer_colour_pick {
                Some(_) => None,
                None => Some(active),
            };
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
        // These two wear the same hues as the marks they set in the rows'
        // status column, so the header toggle and the row mark are visibly
        // the same flag (owner 2026-08-22).
        if icon_btn_tint(
            ui,
            Icon::Reference,
            s,
            a_reference,
            true,
            "Reference layer",
            Some(theme::c().ref_mark),
        )
        .clicked()
        {
            app.push_cmd(AppCmd::SetLayerReference(active, !a_reference));
        }
        if icon_btn_tint(
            ui,
            Icon::Draft,
            s,
            a_draft,
            true,
            "Draft layer",
            Some(theme::c().draft_mark),
        )
        .clicked()
        {
            app.push_cmd(AppCmd::SetLayerDraft(active, !a_draft));
        }
    });
    ui.horizontal(|ui| {
        // Every "make one" button wears its subject + a corner plus (owner
        // 2026-08-21: the bare + said nothing about what it made).
        if icon_btn(ui, Icon::NewLayer, cmd_s, false, true, "New layer").clicked() {
            app.push_cmd(AppCmd::AddLayer);
        }
        // Same command as Layer ▸ New vector layer, and the Vector glyph the
        // resulting row carries.
        if icon_btn(
            ui,
            Icon::NewVector,
            cmd_s,
            false,
            true,
            "New vector layer — strokes record as editable geometry",
        )
        .clicked()
        {
            app.push_cmd(AppCmd::AddVectorLayer);
        }
        if icon_btn(
            ui,
            Icon::NewFrameFolder,
            cmd_s,
            false,
            true,
            "New frame border folder",
        )
        .clicked()
        {
            app.push_cmd(AppCmd::NewFrameLayer);
        }
        if icon_btn(ui, Icon::NewFolder, cmd_s, false, true, "New folder").clicked() {
            app.push_cmd(AppCmd::AddFolder);
        }
        if icon_btn(ui, Icon::Duplicate, cmd_s, false, true, "Duplicate layer").clicked() {
            app.push_cmd(AppCmd::DuplicateLayer);
        }
        if icon_btn(
            ui,
            Icon::MergeDown,
            cmd_s,
            false,
            true,
            "Merge with layer below (Ctrl+E)",
        )
        .clicked()
        {
            app.push_cmd(AppCmd::MergeDown);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_btn(ui, Icon::Trash, cmd_s, false, true, "Delete layer").clicked() {
                app.push_cmd(AppCmd::RemoveLayer);
            }
            // The funnel: the filter row hides behind it (audit leftover —
            // most sessions never filter, so the row was dead height).
            // Closing RESETS every control: a filter narrowing the list
            // from behind a closed funnel would read as lost layers.
            if icon_btn(
                ui,
                Icon::Funnel,
                cmd_s,
                app.layer_filter_open,
                true,
                "Filter rows (closing clears the filter)",
            )
            .clicked()
            {
                app.layer_filter_open = !app.layer_filter_open;
                if !app.layer_filter_open {
                    app.layer_search.clear();
                    app.layer_filter_kind = LayerFilterKind::All;
                    app.layer_filter_ref_only = false;
                    app.layer_filter_no_draft = false;
                    app.layer_filter_this_frame = false;
                    app.layer_filter_label = None;
                }
            }
        });
    });

    // SL-001..004 (CSP's Search Layer, folded into the palette that
    // already lists the stack rather than a second window listing it
    // again): name substring beside a type + property dropdown. Same
    // shape as the Material palette's search row.
    if app.layer_filter_open {
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
                    // T5c: the two property narrowings wear the hues of the
                    // marks they hunt for (the row status column, the header
                    // toggles and these now agree).
                    ui.checkbox(
                        &mut app.layer_filter_ref_only,
                        egui::RichText::new("reference only").color(theme::c().ref_mark),
                    );
                    ui.checkbox(
                        &mut app.layer_filter_no_draft,
                        egui::RichText::new("hide drafts").color(theme::c().draft_mark),
                    );
                    ui.checkbox(&mut app.layer_filter_this_frame, "this frame folder")
                        .on_hover_text(
                            "only the koma folder holding the active layer — the one that earns its keep on a 200-layer page",
                        );
                    ui.separator();
                    // Layer colour as a filter: the six standard swatches.
                    // A custom-picked label matches none of them — the
                    // swatch is the query, exact by design.
                    ui.horizontal(|ui| {
                        for c in LABEL_COLORS {
                            let sel = app.layer_filter_label == Some(c);
                            let (r, resp) = ui.allocate_exact_size(
                                egui::vec2(16.0, 16.0),
                                egui::Sense::click(),
                            );
                            ui.painter().rect_filled(
                                r,
                                3.0,
                                egui::Color32::from_rgb(c[0], c[1], c[2]),
                            );
                            if sel {
                                ui.painter().rect_stroke(
                                    r,
                                    3.0,
                                    egui::Stroke::new(2.0, theme::c().text_strong),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            if resp
                                .on_hover_text("only rows with this palette colour")
                                .clicked()
                            {
                                app.layer_filter_label = if sel { None } else { Some(c) };
                            }
                        }
                    });
                });
        });
        ui.add_space(1.0);
    }

    refresh_layer_thumbs(ui.ctx(), app);
    let mask_thumbs = refresh_mask_thumbs(ui.ctx(), app);

    // The stack, top-first: CSP rows — eye | label strip | pen | thumbnail |
    // "100 % Normal" over the name. Rows drag to reorder.
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
            tone_lpi: l.tone.map(|t| t.lpi).or(match &l.kind {
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
            mask: l.mask.as_ref().map(|m| m.enabled),
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

        // Folder rows run thinner than layer rows (CSP; owner 2026-08-21).
        let row_h = if row.folder {
            FOLDER_ROW_H
        } else {
            LAYER_ROW_H
        };

        // Inline rename keeps the row's full height and puts the field on
        // the name line — the old branch let the TextEdit allocate its own
        // ~20 px and the row visibly collapsed (owner bug 2026-08-21).
        // Escape cancels; Enter or clicking away commits.
        if matches!(&app.renaming, Some((ri, _)) if *ri == i) {
            let (rrect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), row_h),
                egui::Sense::hover(),
            );
            if ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
                app.renaming = None;
                continue;
            }
            let Some((_, text)) = &mut app.renaming else {
                unreachable!()
            };
            let field = egui::Rect::from_center_size(
                rrect.center(),
                egui::vec2(rrect.width() - 12.0, 22.0),
            );
            let resp = ui.put(field, egui::TextEdit::singleline(text));
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
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, row_h), sense);
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
        // The status column: reference / draft marks in their own fixed
        // slot (owner 2026-08-21, matching CSP's per-layer status column)
        // instead of the old right-edge badges.
        let flag_cell = egui::Rect::from_min_max(
            egui::pos2(pen_cell.right(), rect.top()),
            egui::pos2(pen_cell.right() + FLAG_COL_W, rect.bottom()),
        );
        let pen_col = flag_cell.right();
        let eye = ui
            .interact(eye_cell, resp.id.with("eye"), egui::Sense::click())
            .on_hover_text("show/hide — Alt+click solos this layer");
        let colour_cell = ui
            .interact(pen_cell, resp.id.with("colour"), egui::Sense::click())
            .on_hover_text("layer colour…");
        // The row's ≡ menu lives in a reserved right-edge column. The
        // response is taken EVERY frame (not only while hovered) — the
        // pointer leaves the row the moment it enters the open menu, and a
        // response that stopped existing would take the menu with it.
        let menu_rect =
            egui::Rect::from_min_max(egui::pos2(rect.right() - ROW_MENU_W, rect.top()), rect.max);
        let menu_resp = ui.interact(menu_rect, resp.id.with("rowmenu"), egui::Sense::click());
        let menu_open =
            egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&menu_resp));
        // Discoverability (r102 audit): the row's two power gestures had
        // no surface — hover carries them now.
        let resp = resp.on_hover_text(
            "Ctrl+click: add/remove from the selected layers · Shift+click: select range · \
             Ctrl+click the thumbnail: selection from this layer's ink · Alt+click: clip to \
             layer below · double-click: rename",
        );

        let p = ui.painter();
        // The highlight starts AFTER the rail/status/indent gutter (audit:
        // flooding the empty status column read as two meaningless blue
        // bars; CSP keeps the whole left gutter dark on the selected row).
        let fill_rect = egui::Rect::from_min_max(egui::pos2(pen_col, rect.top()), rect.max);
        if selected {
            p.rect_filled(fill_rect, 0.0, theme::c().sel_active);
            p.hline(
                fill_rect.x_range(),
                rect.top() + 0.5,
                egui::Stroke::new(1.0, theme::c().sel_edge),
            );
            p.hline(
                fill_rect.x_range(),
                rect.bottom() - 0.5,
                egui::Stroke::new(1.0, theme::c().sel_edge),
            );
        } else if multi {
            p.rect_filled(fill_rect, 0.0, theme::c().sel_row);
        } else if resp.hovered() || menu_resp.hovered() {
            // The ≡ column is its own widget, so it takes the hover off the
            // row — without this the row un-highlights as the pointer
            // reaches the menu it is about to open.
            p.rect_filled(fill_rect, 0.0, theme::c().hover);
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
                    theme::c().text_strong
                };
                (dim, egui::Color32::from_rgb(r, g, b), ic)
            }
            None => (theme::c().field, theme::c().panel, theme::c().text),
        };
        p.rect_filled(eye_cell, 0.0, cell_a);
        p.rect_filled(pen_cell, 0.0, cell_b);
        for x in [
            eye_cell.left(),
            eye_cell.right(),
            pen_cell.right(),
            flag_cell.right(),
        ] {
            p.vline(x, rect.y_range(), egui::Stroke::new(1.0, theme::c().border));
        }
        // Status column: the reference mark above the draft mark, one alone
        // centres. A layer with neither stays EMPTY — the round before this
        // drew a stroked box there as CSP's affordance, but the cell has no
        // click of its own (the marks are set from the header buttons and the
        // row's context menu), so the box read as a checkbox that did nothing.
        if row.reference || row.draft {
            let fs = egui::vec2(12.0, 12.0);
            let cx = flag_cell.center().x;
            match (row.reference, row.draft) {
                (true, true) => {
                    let up = egui::pos2(cx, cy - 7.0);
                    let dn = egui::pos2(cx, cy + 7.0);
                    paint_icon(
                        p,
                        egui::Rect::from_center_size(up, fs),
                        Icon::Reference,
                        theme::c().ref_mark,
                    );
                    paint_icon(
                        p,
                        egui::Rect::from_center_size(dn, fs),
                        Icon::Draft,
                        theme::c().draft_mark,
                    );
                }
                (true, false) => paint_icon(
                    p,
                    egui::Rect::from_center_size(egui::pos2(cx, cy), fs),
                    Icon::Reference,
                    theme::c().ref_mark,
                ),
                (false, true) => paint_icon(
                    p,
                    egui::Rect::from_center_size(egui::pos2(cx, cy), fs),
                    Icon::Draft,
                    theme::c().draft_mark,
                ),
                (false, false) => unreachable!(),
            }
        }
        let eye_r = egui::Rect::from_center_size(
            egui::pos2(eye_cell.center().x, cy),
            egui::vec2(15.0, 15.0),
        );
        paint_icon(
            p,
            eye_r.shrink(1.5),
            if row.visible { Icon::Eye } else { Icon::EyeOff },
            if row.visible {
                rail_icon
            } else {
                rail_icon.gamma_multiply(0.4)
            },
        );
        // Editing-target pen on the active row; a plain CHECK on rows that
        // are multi-selected but not the target (CSP's status column split:
        // the pen row is where ink lands, checked rows merely ride along).
        if selected {
            let pr = egui::Rect::from_center_size(
                egui::pos2(pen_cell.center().x, cy),
                egui::vec2(13.0, 13.0),
            );
            paint_icon(p, pr, Icon::Pen, rail_icon);
        } else if multi {
            let c = egui::pos2(pen_cell.center().x, cy);
            p.line(
                vec![
                    egui::pos2(c.x - 4.5, c.y + 0.5),
                    egui::pos2(c.x - 1.5, c.y + 3.5),
                    egui::pos2(c.x + 4.5, c.y - 3.5),
                ],
                egui::Stroke::new(1.8, rail_icon),
            );
        }

        // Indent nested rows; folders get a disclosure triangle in the gutter.
        // Nested rows also carry CSP's tree guide lines — one vertical under
        // each ancestor's triangle column.
        let indent = row.depth as f32 * 12.0;
        for d in 1..=row.depth as usize {
            let gx = pen_col + 8.0 + (d - 1) as f32 * 12.0;
            p.vline(
                gx,
                rect.y_range(),
                egui::Stroke::new(1.0, theme::c().border),
            );
        }
        let mut disclose: Option<egui::Rect> = None;
        if row.folder {
            // A thin chevron, tucked close to the folder glyph (audit: the
            // filled 13px triangle was too heavy and sat too far away).
            let dr = egui::Rect::from_center_size(
                egui::pos2(pen_col + 7.0 + indent, cy),
                egui::vec2(12.0, 12.0),
            );
            let c = dr.center();
            let ch = egui::Color32::from_rgb(0x9a, 0x9a, 0x9a);
            let stroke = egui::Stroke::new(1.5, if selected { theme::c().text_strong } else { ch });
            let pts = if row.open {
                vec![
                    egui::pos2(c.x - 4.0, c.y - 2.0),
                    egui::pos2(c.x, c.y + 2.5),
                    egui::pos2(c.x + 4.0, c.y - 2.0),
                ]
            } else {
                vec![
                    egui::pos2(c.x - 2.0, c.y - 4.0),
                    egui::pos2(c.x + 2.5, c.y),
                    egui::pos2(c.x - 2.0, c.y + 4.0),
                ]
            };
            p.line(pts, stroke);
            disclose = Some(dr);
        }
        let thumb_left = pen_col + 7.0 + indent + if row.folder { 12.0 } else { 0.0 };

        // Thumbnail slot, 32 px wide. Layer rows: the content thumb on a
        // checker well (CSP-size — after the colour rail, the second thing
        // that tells rows apart at a glance). FOLDER rows carry a BIG
        // folder glyph instead — no content composite, no transparency
        // checker, no border box (owner 2026-08-21, CSP's look): frame
        // folders wear their own panelled-folder icon, plain folders flip
        // open/closed with the disclosure state.
        let thumb_h = if row.folder { 26.0 } else { 32.0 };
        // Pixel-snap the slot: a fractional row centre puts the 32 px
        // bitmap on half-pixels and bilinear sampling smears a 1 px lip
        // along one edge (the audit's "thumbnail lip").
        let ppp = p.ctx().pixels_per_point();
        let snap = |v: f32| (v * ppp).round() / ppp;
        let tr = egui::Rect::from_min_size(
            egui::pos2(snap(thumb_left), snap(cy - thumb_h * 0.5)),
            egui::vec2(32.0, thumb_h),
        );
        if row.folder {
            let icon = if row.is_frame {
                Icon::FrameFolder
            } else if row.open {
                Icon::FolderOpen
            } else {
                Icon::Folder
            };
            let ir = egui::Rect::from_center_size(tr.center(), egui::Vec2::splat(22.0));
            paint_icon(
                p,
                ir,
                icon,
                if selected {
                    theme::c().text_strong
                } else {
                    theme::c().text
                },
            );
        } else {
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
                None => {
                    p.rect_filled(tr, 2.0, theme::c().field);
                }
            }
            p.rect_stroke(
                tr,
                2.0,
                egui::Stroke::new(1.0, theme::c().border),
                egui::StrokeKind::Inside,
            );
        }
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
                    theme::c().text_weak
                } else {
                    egui::Color32::from_rgb(0xe5, 0x4b, 0x4b)
                },
            );
        }

        // CSP's SECOND image cell: the layer mask, beside the layer thumb.
        // Grey-on-dark coverage (light = the layer shows through), an accent
        // ring while strokes are landing on the mask instead of the pixels
        // (LM-004), and a red cross when the mask is kept but switched OFF —
        // the state that otherwise looks exactly like no mask at all. The
        // cell only exists on masked rows, so an ordinary stack loses no
        // name width to it.
        let mut mask_rect = None;
        if let Some(enabled) = row.mask {
            let mr = egui::Rect::from_min_size(
                egui::pos2(snap(tr.right() + 5.0), snap(cy - MASK_THUMB * 0.5)),
                egui::Vec2::splat(MASK_THUMB),
            );
            match mask_thumbs.get(i).and_then(|t| t.as_ref()) {
                Some(t) => {
                    p.image(
                        t.id(),
                        mr,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                None => {
                    p.rect_filled(mr, 2.0, theme::c().field);
                }
            }
            let armed = selected && app.mask_edit;
            p.rect_stroke(
                mr,
                2.0,
                egui::Stroke::new(
                    if armed { 2.0 } else { 1.0 },
                    if armed {
                        theme::c().accent
                    } else {
                        theme::c().border
                    },
                ),
                egui::StrokeKind::Inside,
            );
            if !enabled {
                let off = theme::c().ref_mark;
                let s = egui::Stroke::new(1.6, off);
                p.line_segment([mr.left_top(), mr.right_bottom()], s);
                p.line_segment([mr.left_bottom(), mr.right_top()], s);
            }
            mask_rect = Some(mr);
        }

        // Two text lines, CSP's layout: the meta ("100 % Normal" — or
        // "85.0 LPI" on a toned row) small on top, the NAME big underneath,
        // instead of a name and a far-away right-aligned meta sharing one
        // cramped line. The type glyph leads the meta line (CSP's slot).
        let (y_meta, y_name) = if row.folder {
            // The thinner folder row keeps both lines, tighter.
            (rect.top() + 9.0, rect.bottom() - 11.0)
        } else {
            (rect.top() + 12.0, rect.bottom() - 14.0)
        };
        let text_x = mask_rect.map_or(tr.right() + 8.0, |mr| mr.right() + 6.0);

        // The ≡ itself: three hairlines, painted only while the row (or the
        // menu it opened) is under the pointer, CSP's row menu.
        if resp.hovered() || menu_resp.hovered() || menu_open {
            let mc = menu_rect.center();
            let col = if menu_open || menu_resp.hovered() {
                theme::c().text_strong
            } else {
                theme::c().text_weak
            };
            for dy in [-3.5, 0.0, 3.5] {
                p.hline(
                    (mc.x - 4.5)..=(mc.x + 4.5),
                    mc.y + dy,
                    egui::Stroke::new(1.2, col),
                );
            }
        }

        // Right-edge flag on the meta line: the locks only — reference and
        // draft moved into the status column on the left. Everything on this
        // edge stops short of the reserved ≡ column.
        let mut fx = menu_rect.left() - 8.0;
        if row.lock || row.lock_alpha {
            let lr = egui::Rect::from_center_size(egui::pos2(fx, y_meta), egui::vec2(12.0, 12.0));
            paint_icon(
                p,
                lr,
                if row.lock {
                    Icon::Lock
                } else {
                    Icon::LockAlpha
                },
                theme::c().text_weak,
            );
            fx -= 13.0;
        }

        let mut meta_x = text_x;
        // Folders carry their glyph as the big thumbnail-slot icon; only
        // non-folder kinds still mark the meta line.
        if let Some(icon) = row.glyph.filter(|_| !row.folder) {
            let fr = egui::Rect::from_center_size(
                egui::pos2(meta_x + 6.0, y_meta),
                egui::vec2(12.0, 12.0),
            );
            paint_icon(
                p,
                fr,
                icon,
                if selected {
                    theme::c().text_strong
                } else {
                    theme::c().text_weak
                },
            );
            meta_x = fr.right() + 4.0;
        }
        // A narrow column shortens the blend name to its initial rather
        // than ellipsizing every single row (audit: "100 % No…" repeated
        // down the stack reads as a rendering bug).
        let narrow = rect.width() < 215.0;
        let bname = |b: Blend| -> String {
            let full = blend_name(b);
            if narrow {
                full.chars().next().unwrap_or('N').to_string()
            } else {
                full.to_owned()
            }
        };
        let meta = match row.tone_lpi {
            Some(lpi) if row.blend == Blend::Normal => format!("{lpi:.1} LPI"),
            Some(lpi) => format!("{lpi:.1} LPI · {}", bname(row.blend)),
            None => format!("{:.0} % {}", row.opacity * 100.0, bname(row.blend)),
        };
        let meta_col = if selected {
            theme::c().text
        } else {
            theme::c().text_weak
        };
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
        let mut name_right = menu_rect.left() - 5.0;
        if row.folder
            && row.is_frame
            && let Some((pos, amb, pinned)) = app.frame_pos(i)
        {
            let br = egui::Rect::from_center_size(
                egui::pos2(menu_rect.left() - 11.0, y_name),
                egui::vec2(16.0, 13.0),
            );
            name_right = br.left() - 6.0;
            let bg = if amb {
                egui::Color32::from_rgb(196, 158, 46)
            } else if pinned {
                theme::c().accent
            } else {
                theme::c().field
            };
            p.rect_filled(br, 3.0, bg);
            p.rect_stroke(
                br,
                3.0,
                egui::Stroke::new(1.0, theme::c().border),
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
                    theme::c().text_strong
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
                let ruler = app.doc.layers[i].frames().is_some_and(|fs| fs.border_ruler);
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
            theme::c().text_weak
        } else if selected {
            theme::c().text_strong
        } else {
            theme::c().text
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
            egui::Stroke::new(1.0, theme::c().border),
        );

        let disclose_clicked = disclose.is_some_and(|dr| {
            ui.interact(dr, resp.id.with("fold"), egui::Sense::click())
                .clicked()
        });
        // LM-004 from the palette: the mask cell is the mask's SELECT — it
        // arms mask editing on this row, and clicking the armed one hands
        // the brush back to the pixels.
        let mask_clicked = mask_rect.is_some_and(|mr| {
            ui.interact(mr, resp.id.with("mask"), egui::Sense::click())
                .on_hover_text("edit this mask (click again to go back to the layer)")
                .clicked()
        });
        egui::Popup::menu(&menu_resp).show(|ui| row_menu(ui, app, i, row));
        if colour_cell.clicked() {
            app.layer_colour_pick = Some(i);
        }
        if mask_clicked {
            if selected && app.mask_edit {
                app.push_cmd(AppCmd::MaskEdit);
            } else {
                if !selected {
                    app.push_cmd(AppCmd::SelectLayer(i));
                }
                // Already armed on another row: selecting this one keeps the
                // arm (it has a mask), so a second toggle would disarm it.
                if !app.mask_edit {
                    app.push_cmd(AppCmd::MaskEdit);
                }
            }
        } else if eye.clicked() {
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
        } else if resp.clicked() && ui.input(|i| i.modifiers.alt) {
            // Walk #4 (CSP's gesture is Alt+click the line between rows;
            // the row body is the honest egui equivalent — the EYE cell's
            // Alt stays the solo, and Ctrl+Alt belongs to the selection
            // op combos): toggle clip to layer below, same command the
            // palette icon and the row menu push.
            let clipped = app.doc.layers.get(i).is_some_and(|l| l.clip);
            app.push_cmd(AppCmd::SetLayerClip(i, !clipped));
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
                .hline(rect.x_range(), y, egui::Stroke::new(2.0, theme::c().accent));
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
            p.rect_filled(rect, 0.0, theme::c().sel_active);
        } else if resp.hovered() {
            p.rect_filled(rect, 2.0, theme::c().hover);
        }
        // Same rail geometry as a layer row: eye cell | pen cell | content.
        let eye_cell =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 22.0, rect.bottom()));
        let pen_cell = egui::Rect::from_min_max(
            egui::pos2(eye_cell.right(), rect.top()),
            egui::pos2(eye_cell.right() + 20.0, rect.bottom()),
        );
        p.rect_filled(eye_cell, 0.0, theme::c().field);
        p.rect_filled(pen_cell, 0.0, theme::c().panel);
        for x in [eye_cell.left(), eye_cell.right(), pen_cell.right()] {
            p.vline(x, rect.y_range(), egui::Stroke::new(1.0, theme::c().border));
        }
        let eye_r = egui::Rect::from_center_size(
            egui::pos2(eye_cell.center().x, cy),
            egui::vec2(15.0, 15.0),
        );
        paint_icon(p, eye_r.shrink(1.5), Icon::Eye, theme::c().text_weak);
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
            egui::Stroke::new(1.0, theme::c().border),
            egui::StrokeKind::Inside,
        );
        let text_col = if selected {
            theme::c().text_strong
        } else {
            theme::c().text_weak
        };
        // Two lines like a layer row: the sheet glyph + role on top, the
        // name underneath.
        let gr = egui::Rect::from_center_size(
            egui::pos2(tr.right() + 14.0, rect.top() + 12.0),
            egui::vec2(12.0, 12.0),
        );
        paint_icon(p, gr, Icon::Paper, theme::c().text_weak);
        p.text(
            egui::pos2(gr.right() + 4.0, rect.top() + 12.0),
            egui::Align2::LEFT_CENTER,
            "the page's ground",
            egui::FontId::proportional(10.0),
            theme::c().text_weak,
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

    // The layer-colour popup: CSP's standard swatches PLUS the full picker
    // right in the window — no second click to reach the wheel (owner
    // 2026-08-21). Opened from a row's status cell or the Label button.
    if let Some(ci) = app.layer_colour_pick {
        if ci >= app.doc.layers.len() {
            app.layer_colour_pick = None;
        } else {
            let mut open = true;
            let name = app.doc.layers[ci].name.clone();
            egui::Window::new(format!("Layer colour — {name}"))
                .id(egui::Id::new("mn.layercolour"))
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    let cur = app.doc.layers[ci].label;
                    ui.horizontal(|ui| {
                        for c in LABEL_COLORS {
                            let (r, resp) = ui
                                .allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
                            ui.painter().rect_filled(
                                r,
                                3.0,
                                egui::Color32::from_rgb(c[0], c[1], c[2]),
                            );
                            if cur == Some(c) {
                                ui.painter().rect_stroke(
                                    r,
                                    3.0,
                                    egui::Stroke::new(2.0, theme::c().text_strong),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            if resp.clicked() {
                                app.push_cmd(AppCmd::SetLayerLabel(ci, Some(c)));
                            }
                        }
                        if ui.button("Off").clicked() {
                            app.push_cmd(AppCmd::SetLayerLabel(ci, None));
                        }
                    });
                    ui.add_space(4.0);
                    let base = cur.unwrap_or(LABEL_COLORS[0]);
                    let mut c32 = egui::Color32::from_rgb(base[0], base[1], base[2]);
                    if egui::color_picker::color_picker_color32(
                        ui,
                        &mut c32,
                        egui::color_picker::Alpha::Opaque,
                    ) {
                        app.push_cmd(AppCmd::SetLayerLabel(ci, Some([c32.r(), c32.g(), c32.b()])));
                    }
                });
            if !open {
                app.layer_colour_pick = None;
            }
        }
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

/// Mask thumbnails, keyed by the mask's revision. Mask revisions come from
/// the GLOBAL tile counter (`mn_core::next_revision`), so a key can never
/// mean two different masks — which is why this cache lives in egui memory
/// keyed by content instead of by row index like `layer_thumbs`, and why it
/// survives a reorder without a single invalidation call.
#[derive(Clone, Default)]
struct MaskThumbs(std::collections::HashMap<u64, egui::TextureHandle>);

/// The per-row mask thumbnail for every layer (index-aligned, `None` on the
/// unmasked ones). Rebuilds only what changed; drops the entries whose masks
/// are gone, so a session of mask edits does not pile up textures.
fn refresh_mask_thumbs(ctx: &egui::Context, app: &App) -> Vec<Option<egui::TextureHandle>> {
    let id = egui::Id::new("mn.layer.maskthumbs");
    let old = ctx
        .data(|d| d.get_temp::<MaskThumbs>(id))
        .unwrap_or_default();
    let mut fresh = MaskThumbs::default();
    let out: Vec<Option<egui::TextureHandle>> = (0..app.doc.layers.len())
        .map(|i| {
            let rev = app.doc.layers[i].mask.as_ref()?.revision;
            let tex = match old.0.get(&rev) {
                Some(t) => t.clone(),
                None => ctx.load_texture(
                    format!("mn.layer.mask.{rev}"),
                    mask_thumb_image(&app.doc, i),
                    egui::TextureOptions::LINEAR,
                ),
            };
            fresh.0.insert(rev, tex.clone());
            Some(tex)
        })
        .collect();
    ctx.data_mut(|d| d.insert_temp(id, fresh));
    out
}

/// A mask as a small grey-on-dark image: the coverage IS the picture (light
/// = the layer shows through). An absent tile is UNMASKED, i.e. fully
/// visible — the same rule the compositors and the bake use, and the reason
/// a fresh LM-001 mask reads as a plain light square.
fn mask_thumb_image(doc: &mn_core::Document, li: usize) -> egui::ColorImage {
    const T: usize = 20;
    let (w, h) = doc.size;
    let mask = doc.layers[li].mask.as_ref();
    let mut px = Vec::with_capacity(T * T * 4);
    for ty in 0..T {
        for tx in 0..T {
            let cx = ((tx as f32 + 0.5) / T as f32 * w as f32) as i32;
            let cy = ((ty as f32 + 0.5) / T as f32 * h as f32) as i32;
            let idx = mn_core::TileIdx::of_pixel(cx, cy);
            let (ox, oy) = idx.origin();
            let cov = mask
                .and_then(|m| m.tiles.get(&idx))
                .map(|t| t.pixel((cx - ox) as usize, (cy - oy) as usize)[3])
                .unwrap_or(32768)
                .min(32768) as u32;
            let mix = |lo: u32, hi: u32| ((lo * (32768 - cov) + hi * cov) / 32768) as u8;
            px.extend_from_slice(&[mix(0x14, 0xd6), mix(0x14, 0xd6), mix(0x18, 0xda), 255]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([T, T], &px)
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
            label: None,
            frame_scope: None,
            frame_scope_wanted: false,
        }
    }

    /// The colour filter matches a row's OWN label exactly — an unlabelled
    /// row and a differently-labelled row both drop.
    #[test]
    fn filter_matches_by_label_colour() {
        let mut d = stack();
        let blue = LABEL_COLORS[0];
        let red = LABEL_COLORS[1];
        d.layers[0].label = Some(blue);
        let mut f = plain(LayerFilterKind::All, "");
        f.label = Some(blue);
        let hits: Vec<usize> = (0..d.layers.len()).filter(|&i| f.passes(&d, i)).collect();
        assert_eq!(hits, vec![0], "only the blue-labelled row");
        f.label = Some(red);
        assert_eq!(
            (0..d.layers.len()).filter(|&i| f.passes(&d, i)).count(),
            0,
            "no red rows anywhere"
        );
    }

    fn empty_balloons() -> mn_core::BalloonSet {
        mn_core::BalloonSet {
            balloons: Vec::new(),
            border_px: 4.0,
            pressure_width: false,
        }
    }

    /// One layer of every kind the bar has to answer for.
    fn one_of_each() -> Vec<(&'static str, mn_core::Layer)> {
        let l = |k: LayerKind| {
            let mut l = mn_core::Layer::new("x");
            l.kind = k;
            l
        };
        let mut folder = mn_core::Layer::new("folder");
        folder.folder = true;
        let mut frame_folder = l(LayerKind::Frame(FrameSet::single_rect(
            [0.0, 0.0, 8.0, 8.0],
            2.0,
        )));
        frame_folder.folder = true;
        let mut vector = mn_core::Layer::new("vector");
        vector.strokes = Some(Default::default());
        vec![
            ("raster", mn_core::Layer::new("raster")),
            ("vector", vector),
            ("folder", folder),
            ("frame-folder", frame_folder),
            (
                "frame",
                l(LayerKind::Frame(FrameSet::single_rect(
                    [0.0, 0.0, 8.0, 8.0],
                    2.0,
                ))),
            ),
            ("balloon", l(LayerKind::Balloon(empty_balloons()))),
            ("text", l(LayerKind::Text(TextSet::default()))),
            (
                "fill",
                l(LayerKind::Fill(FillKind::Flat { color: [0.0; 4] })),
            ),
            (
                "tone",
                l(LayerKind::Fill(FillKind::Tone {
                    tone: mn_core::ToneParams::default(),
                    density: 0.4,
                })),
            ),
            (
                "correction",
                l(LayerKind::Correction(mn_core::Adjust::Invert)),
            ),
        ]
    }

    /// LP-025: the bar's contents per layer type. The table is asserted
    /// where it is a taste call, but the load-bearing half is the invariant
    /// underneath it — a tool is listed only where the app would actually
    /// take it, so the bar can never advertise a tool the guards refuse.
    #[test]
    fn tool_nav_lists_only_tools_the_layer_accepts() {
        for (name, l) in one_of_each() {
            let tools = tools_for_layer(&l);
            let mut seen = tools.to_vec();
            seen.sort_by_key(|t| format!("{t:?}"));
            seen.dedup();
            assert_eq!(seen.len(), tools.len(), "{name}: a tool listed twice");
            for &t in tools {
                assert!(
                    tool_icon(t).is_some(),
                    "{name}: {t:?} has no strip icon to draw"
                );
            }

            // Brushes: exactly the three cases that take one — a plain
            // raster (`paintable`), a stroke-recording raster (inking is
            // captured), and the live kinds (the brush edits the window).
            let live = matches!(l.kind, LayerKind::Fill(_) | LayerKind::Correction(_));
            let takes_brush = l.paintable() || l.records_strokes() || live;
            assert_eq!(
                tools.contains(&Tool::Pen),
                takes_brush,
                "{name}: the pen must be listed exactly where a stroke lands"
            );
            assert_eq!(tools.contains(&Tool::Eraser), takes_brush, "{name}");

            // The raster-edit family is `paintable`'s own list: everything
            // else re-derives its pixels and would lose the edit.
            for t in [Tool::Fill, Tool::Gradient, Tool::Tone, Tool::Liquify] {
                assert!(
                    !tools.contains(&t) || l.paintable(),
                    "{name}: {t:?} needs a paintable layer"
                );
            }
            // Object edits geometry, so it is offered exactly where there
            // is geometry to edit.
            let has_geometry = l.is_vector() || l.records_strokes();
            assert!(
                !tools.contains(&Tool::Object) || has_geometry,
                "{name}: nothing for the Object tool to grab"
            );
            // Text only where text lives.
            assert!(
                !tools.contains(&Tool::Text) || l.is_text() || l.is_balloon(),
                "{name}: the text tool would make a new layer here"
            );
            // Layer-agnostic tools are never listed (see the doc comment).
            for t in [Tool::Select, Tool::Wand, Tool::Eyedrop, Tool::Pan] {
                assert!(!tools.contains(&t), "{name}: {t:?} works everywhere");
            }
            // Only a plain folder gets an empty bar; every other type has
            // at least one tool to offer.
            assert_eq!(
                tools.is_empty(),
                name == "folder",
                "{name}: {tools:?} — only a plain folder holds nothing"
            );
        }

        // The two taste calls worth pinning by name.
        let mut balloon = mn_core::Layer::new("b");
        balloon.kind = LayerKind::Balloon(empty_balloons());
        assert_eq!(
            tools_for_layer(&balloon).to_vec(),
            vec![Tool::Balloon, Tool::Text, Tool::Object],
            "a balloon's text is edited in place, so T belongs on its bar"
        );
        let mut ff = mn_core::Layer::new("ff");
        ff.folder = true;
        ff.kind = LayerKind::Frame(FrameSet::single_rect([0.0, 0.0, 8.0, 8.0], 2.0));
        assert_eq!(
            tools_for_layer(&ff).to_vec(),
            vec![Tool::Frame, Tool::Object],
            "a frame folder is still divisible"
        );
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

    /// The mask cell's picture IS the coverage, and the rule that decides
    /// what an EMPTY mask looks like is the compositor's: an absent tile is
    /// unmasked, i.e. fully visible. So a fresh LM-001 mask reads as a light
    /// square (the layer shows through) and a zero-coverage tile reads dark.
    /// Get this backwards and every masked row shows the mask inverted.
    #[test]
    fn mask_thumb_paints_coverage_light_on_dark() {
        let mut d = Document::new(64, 64);
        d.layers[0].mask = Some(mn_core::doc::LayerMask {
            tiles: std::collections::HashMap::new(),
            enabled: true,
            revision: mn_core::next_revision(),
        });
        let img = mask_thumb_image(&d, 0);
        assert_eq!(img.pixels.len(), 20 * 20);
        assert!(
            img.pixels.iter().all(|c| c.r() > 0xc0),
            "no tiles = unmasked = light"
        );

        let m = d.layers[0].mask.as_mut().unwrap();
        // A transparent tile is zero coverage — the mask hides that region.
        m.tiles.insert(
            mn_core::TileIdx { x: 0, y: 0 },
            std::sync::Arc::new(mn_core::Tile::new_transparent()),
        );
        let img = mask_thumb_image(&d, 0);
        assert!(
            img.pixels.iter().all(|c| c.r() < 0x30),
            "zero coverage = hidden = dark"
        );
    }

    /// SL-003's manga row: the scope is the frame folder BLOCK — header
    /// plus every child — and the walk finds it from a layer nested
    /// inside, not only from the header itself.
    #[test]
    fn frame_folder_scope_covers_the_block() {
        let mut d = Document::new(200, 200);
        let hdr = d.add_frame_folder(
            "Frame 1",
            FrameSet::single_rect([10.0, 10.0, 90.0, 90.0], 2.0),
        );
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
            label: None,
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
            label: None,
            frame_scope: None,
            frame_scope_wanted: true,
        };
        assert!(!f.passes(&flat, 0));
    }
}
