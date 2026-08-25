//! Tool Property: the parameters of the SELECTED sub tool — and, under the
//! Operation tool, of the SELECTED OBJECT (text box / balloon / panel),
//! which is what CSP edits there. Everything renders through the SECTION
//! registry at the bottom: the compact palette shows only the sections left
//! checked in the full-properties window (the palette header's wrench, CSP's
//! Tool Property ▸ detail with its eye toggles); the window always shows all.

use super::icons::Icon;
use super::theme::{self, ValueBar};
use super::widgets::{group_caption, icon_btn, px_mm_text};
use crate::app::App;
use crate::cmd::{AppCmd, BalloonMode, Tool};

// --- tool property ------------------------------------------------------

/// CSP Tool Property: the parameters of the *selected tool*, saved per sub
/// tool for the stroke tools.
pub(super) fn tool_property(ui: &mut egui::Ui, app: &mut App) {
    // THE PALETTE SCROLLS (owner report, 2026-08-19: the Guide row at the
    // bottom was cut off with no scrollbar and no way to reach it). Wrapped
    // here, at the one entry point, so it covers the brush header and the
    // transform panel too — both can outgrow a short column just as easily,
    // and a control you cannot reach is the same bug whichever branch drew
    // it. `auto_shrink` off so the area claims the tab's full height rather
    // than hugging its content and never scrolling at all.
    egui::ScrollArea::vertical()
        .id_salt("mn.toolprop.scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| tool_property_body(ui, app));
}

fn tool_property_body(ui: &mut egui::Ui, app: &mut App) {
    // An active Transform OWNS the panel (CSP shows its fields in Tool
    // Property during a transform): flip buttons (T-021), the numeric
    // fields (TR-031–033) and the 9-cell reference point (TR-003).
    if app.transform_drag.is_some() {
        transform_property(ui, app);
        return;
    }
    // Brush tools keep their bespoke header (preset name + the Sub Tool
    // Detail wrench — that window IS the brush full list). The selection
    // pen/eraser are brush tools (the active brush paints the coverage).
    if matches!(
        app.tool,
        Tool::Pen | Tool::Eraser | Tool::SelPen | Tool::SelEraser
    ) {
        pen_property(ui, app);
        return;
    }
    let sections = prop_sections(app);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(context_title(app))
                .size(11.5)
                .color(theme::c().text_strong),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_btn(
                ui,
                Icon::Wrench,
                15.0,
                app.prop_detail_open,
                true,
                "Full property list — choose what shows in this palette",
            )
            .clicked()
            {
                app.prop_detail_open = !app.prop_detail_open;
            }
        });
    });
    ui.add_space(1.0);
    for s in &sections {
        if !app.prop_hidden.contains(s.id) {
            group_caption(ui, s.title);
            (s.body)(ui, app);
        }
    }
}

/// The active Transform's panel (TRIAGE 148 + row 130): flip buttons
/// (T-021), the numeric fields (TR-031-033: scale %, rotation deg, position)
/// and the 9-cell reference point picker (TR-003). Every field recomposes
/// the absolute params through `AppCmd::TransformUpdate` — the same
/// derivation the drag gestures use — so numbers and handles can never
/// disagree.
fn transform_property(ui: &mut egui::Ui, app: &mut App) {
    ui.label(
        egui::RichText::new("Transform")
            .size(11.5)
            .color(theme::c().text_strong),
    );
    ui.add_space(3.0);
    ui.horizontal(|ui| {
        if ui.button("⇋ Flip H").clicked() {
            app.push_cmd(AppCmd::TransformFlip { horizontal: true });
        }
        if ui.button("⇵ Flip V").clicked() {
            app.push_cmd(AppCmd::TransformFlip { horizontal: false });
        }
        // T-020: start this transform over without leaving it. Esc also
        // undoes everything, but Esc drops the float and the selection with
        // it — this keeps both and only clears the numbers.
        if ui
            .button("Reset")
            .on_hover_text(
                "Back to the size, angle and position it was lifted at.\n\
                 Stays in the transform (Esc would leave it).",
            )
            .clicked()
        {
            app.push_cmd(AppCmd::TransformReset);
        }
    });
    ui.add_space(3.0);
    // Copy the live params out, let the widgets mutate the copies, then
    // push the recomposition — `push_cmd` needs &mut app, so no borrow of
    // the drag may outlive the reads.
    let (pivot, rect) = {
        let d = app.transform_drag.as_ref().unwrap();
        (d.pivot(), d.source.rect)
    };
    let (mut sx, mut sy, mut rot) = {
        let d = app.transform_drag.as_ref().unwrap();
        (d.sx * 100.0, d.sy * 100.0, d.rad.to_degrees())
    };
    let (mut px, mut py) = {
        let d = app.transform_drag.as_ref().unwrap();
        (pivot[0] + d.tx, pivot[1] + d.ty)
    };
    let field = |ui: &mut egui::Ui, label: &str, v: &mut f32, sp: f32, sfx: &str| -> bool {
        let mut hit = false;
        ui.horizontal(|ui| {
            ui.label(format!("{label:<12}"));
            if ui
                .add(egui::DragValue::new(v).speed(sp).suffix(sfx))
                .changed()
            {
                hit = true;
            }
        });
        hit
    };
    group_caption(ui, "Scale / Rotation");
    let c1 = field(ui, "Scale X", &mut sx, 1.0, "%");
    let c2 = field(ui, "Scale Y", &mut sy, 1.0, "%");
    let c3 = field(ui, "Rotation", &mut rot, 1.0, "°");
    // CSP 縦横比固定, on by default: corner and side handles scale both axes
    // by one ratio. (Shift does the same for a single drag.)
    let mut keep = app.transform_keep_aspect;
    if ui
        .checkbox(&mut keep, "Keep aspect ratio")
        .on_hover_text(
            "Handles scale width and height together.\n\
             Hold Shift while dragging to do it for one drag.",
        )
        .changed()
    {
        app.transform_keep_aspect = keep;
    }
    group_caption(ui, "Position");
    let c4 = field(ui, "X", &mut px, 1.0, "");
    let c5 = field(ui, "Y", &mut py, 1.0, "");
    if c1 || c2 || c3 || c4 || c5 {
        app.push_cmd(AppCmd::TransformUpdate {
            sx: sx / 100.0,
            sy: sy / 100.0,
            rad: rot.to_radians(),
            tx: px - pivot[0],
            ty: py - pivot[1],
        });
    }
    ui.add_space(3.0);
    group_caption(ui, "Reference point");
    // 9 cells over the UNtransformed source rect; the centre cell resets
    // to the default pivot.
    let cell = |col: usize, row: usize| -> [f32; 2] {
        [
            rect[0] as f32 + (rect[2] - rect[0]) as f32 * col as f32 * 0.5,
            rect[1] as f32 + (rect[3] - rect[1]) as f32 * row as f32 * 0.5,
        ]
    };
    ui.horizontal(|ui| {
        for row in 0..3 {
            ui.vertical(|ui| {
                for col in 0..3 {
                    let p = cell(col, row);
                    let is_center = row == 1 && col == 1;
                    let on = !is_center
                        && app
                            .transform_drag
                            .as_ref()
                            .is_some_and(|d| d.pivot_override == Some(p));
                    if ui
                        .small_button(if on { "◆" } else { "·" })
                        .on_hover_text("Reference point (rotation/flip centre)")
                        .clicked()
                    {
                        app.push_cmd(AppCmd::TransformSetPivot {
                            pivot: if is_center { None } else { Some(p) },
                        });
                    }
                }
            });
        }
    });
}

/// The palette header's context: the selected object when there is one, else
/// the tool (CSP's Tool Property titles itself after the selection).
fn context_title(app: &App) -> String {
    if app.tool == Tool::Object {
        if let Some((li, _)) = app.text_sel {
            if let Some(l) = app.doc.layers.get(li) {
                return format!("Text — {}", l.name);
            }
        }
        if let Some((li, _)) = app.balloon_sel {
            if let Some(l) = app.doc.layers.get(li) {
                return format!("Balloon — {}", l.name);
            }
        }
        if let Some(li) = app.gen_sel {
            if let Some(l) = app.doc.layers.get(li) {
                return format!("Effect lines — {}", l.name);
            }
        }
        if let Some((li, _)) = app.object_sel {
            if let Some(l) = app.doc.layers.get(li) {
                return format!("Frame — {}", l.name);
            }
        }
    }
    format!("{:?} tool", app.tool)
}

mod frames_balloons;
mod gradient;
mod pen;
mod select;
mod text;
mod tone;

pub(crate) use frames_balloons::*;
pub(crate) use gradient::*;
pub(crate) use pen::*;
pub(crate) use select::*;
pub(crate) use text::*;
pub(crate) use tone::*;

// --- the section registry --------------------------------------------------

/// One named, toggleable group of the Tool Property palette (the compact
/// palette hides unchecked sections; the full-properties window shows all).
pub(crate) struct Section {
    pub id: &'static str,
    pub title: &'static str,
    pub body: fn(&mut egui::Ui, &mut App),
}

const SEC_WORKSTYLE: Section = Section {
    id: "text.workstyle",
    title: "Text style",
    body: sec_text_workstyle,
};
const SEC_FONT: Section = Section {
    id: "text.font",
    title: "Font",
    body: sec_text_font,
};
const SEC_DIR: Section = Section {
    id: "text.dir",
    title: "Direction",
    body: sec_text_dir,
};
const SEC_STYLE: Section = Section {
    id: "text.style",
    title: "Style",
    body: sec_text_style,
};
const SEC_RUBY: Section = Section {
    id: "text.ruby",
    title: "Furigana",
    body: sec_text_ruby,
};
const SEC_ALIGN: Section = Section {
    id: "text.align",
    title: "Align",
    body: sec_text_align,
};
const SEC_SPACING: Section = Section {
    id: "text.spacing",
    title: "Spacing",
    body: sec_text_spacing,
};
const SEC_EDGE: Section = Section {
    id: "text.edge",
    title: "Edge",
    body: sec_text_edge,
};
const SEC_TEXT_GUIDE: Section = Section {
    id: "text.guide",
    title: "Guide",
    body: sec_text_guide,
};
/// Row 55 (CSP 液化): the seven modes, strength and radius. Descriptions
/// carry the Alt-invert and hold-accumulate rules — the two things a
/// CSP user expects and a new user would never find.
fn sec_liquify(ui: &mut egui::Ui, app: &mut App) {
    ui.vertical(|ui| {
        for m in mn_core::liquify::LiquifyMode::ALL {
            if ui
                .radio(app.liquify_mode == m, m.label())
                .on_hover_text(match m {
                    mn_core::liquify::LiquifyMode::Push => {
                        "the ink follows the pen. Alt reverses the direction"
                    }
                    mn_core::liquify::LiquifyMode::Expand => {
                        "bulge outward from the stroke; HOLD to keep growing. Alt = pinch"
                    }
                    mn_core::liquify::LiquifyMode::Pinch => {
                        "scrunch inward; HOLD to keep shrinking. Alt = expand"
                    }
                    mn_core::liquify::LiquifyMode::PushLeft => {
                        "shift the ink to the left of your stroke's direction"
                    }
                    mn_core::liquify::LiquifyMode::PushRight => {
                        "shift the ink to the right of your stroke's direction"
                    }
                    mn_core::liquify::LiquifyMode::TwirlCw => {
                        "rotate about the pen; HOLD to keep turning. Alt reverses"
                    }
                    mn_core::liquify::LiquifyMode::TwirlCcw => {
                        "rotate the other way; HOLD to keep turning. Alt reverses"
                    }
                })
                .clicked()
            {
                app.liquify_mode = m;
            }
        }
        let sr = ui.add(
            egui::Slider::new(&mut app.liquify_strength, 0.0..=1.0).text("strength"),
        );
        sr.on_hover_text(
            "how far one touch moves the ink; for expand/pinch/twirl this is also the hold speed — Alt inverts any mode",
        );
        ui.add(egui::Slider::new(&mut app.liquify_radius, 4.0..=300.0).text("radius px"))
            .on_hover_text("the brush disc's radius in canvas pixels");
        ui.weak("drag to warp · hold to accumulate (expand/pinch/twirl) · Alt inverts · one undo per gesture");
    });
}

const SEC_OBJ_GUIDE: Section = Section {
    id: "obj.guide",
    title: "Guide",
    body: sec_obj_guide,
};

/// Every section of the CURRENT context, in palette order. The Operation
/// tool swaps its whole list for the selected object's editors — Tool
/// Property edits the item (owner's fix 7).
pub(super) fn prop_sections(app: &App) -> Vec<Section> {
    let mut v = prop_sections_for_tool(app);
    if matches!(app.doc.active_layer().kind, mn_core::LayerKind::Fill(_)) {
        v.insert(
            0,
            Section {
                id: "live.fill",
                title: "Live layer",
                body: sec_live_fill,
            },
        );
    }
    v
}

fn prop_sections_for_tool(app: &App) -> Vec<Section> {
    match app.tool {
        Tool::Object => {
            // S-001: the layer pick is its own sub tool, and nothing else
            // in the Operation tool applies while it is the active one.
            if app.object_mode == crate::cmd::ObjectMode::PickLayer {
                vec![Section {
                    id: "obj.picklayer",
                    title: "Select layer",
                    body: sec_pick_layer,
                }]
            } else if app.text_sel.is_some() {
                vec![
                    SEC_WORKSTYLE,
                    SEC_FONT,
                    SEC_DIR,
                    SEC_STYLE,
                    SEC_RUBY,
                    SEC_ALIGN,
                    SEC_SPACING,
                    SEC_EDGE,
                    SEC_TEXT_GUIDE,
                ]
            } else if app.balloon_sel.is_some() {
                vec![
                    Section {
                        id: "obj.balloon",
                        title: "Balloon",
                        body: sec_obj_balloon,
                    },
                    Section {
                        id: "obj.balloon.ink",
                        title: "Colour",
                        body: sec_obj_ink,
                    },
                    Section {
                        id: "obj.balloon.tail",
                        title: "Tail",
                        body: sec_obj_tail,
                    },
                    SEC_OBJ_GUIDE,
                ]
            } else if app
                .gen_sel
                .is_some_and(|li| app.doc.layers.get(li).is_some_and(|l| l.genlines.is_some()))
            {
                // Owner's fix 7 again, for the one object family that never
                // got it: with a run selected the palette said "click a
                // text box, balloon or panel" and offered nothing, so a
                // placed effect-line set could only be re-tuned by deleting
                // it and dragging a new one.
                vec![
                    Section {
                        id: "obj.gen",
                        title: "Effect lines",
                        body: sec_obj_genlines,
                    },
                    Section {
                        id: "obj.gen.density",
                        title: "Density",
                        body: sec_obj_genlines_density,
                    },
                    SEC_OBJ_GUIDE,
                ]
            } else if app.object_sel.is_some() {
                vec![
                    Section {
                        id: "obj.frame",
                        title: "Frame border",
                        body: sec_obj_frame,
                    },
                    SEC_OBJ_GUIDE,
                ]
            } else {
                vec![SEC_OBJ_GUIDE]
            }
        }
        Tool::Text => vec![
            SEC_WORKSTYLE,
            SEC_FONT,
            SEC_DIR,
            SEC_STYLE,
            SEC_RUBY,
            SEC_ALIGN,
            SEC_SPACING,
            SEC_EDGE,
            SEC_TEXT_GUIDE,
        ],
        Tool::Balloon => vec![
            Section {
                id: "balloon.line",
                title: "Balloon line",
                body: sec_balloon_line,
            },
            Section {
                id: "balloon.ink",
                title: "Colour",
                body: sec_balloon_ink,
            },
            Section {
                id: "balloon.tail",
                title: "Tail",
                body: sec_balloon_tail,
            },
            Section {
                id: "balloon.guide",
                title: "Guide",
                body: sec_balloon_guide,
            },
        ],
        Tool::Frame => vec![
            Section {
                id: "frame.tool",
                title: "Frame",
                body: sec_frame_tool,
            },
            Section {
                id: "frame.guide",
                title: "Guide",
                body: sec_frame_guide,
            },
        ],
        Tool::Fill => vec![
            Section {
                id: "fill.opts",
                title: "Fill",
                body: sec_fill,
            },
            Section {
                id: "fill.guide",
                title: "Guide",
                body: sec_wand_guide,
            },
        ],
        Tool::Tone => vec![
            Section {
                id: "tone.screen",
                title: "Tone",
                body: sec_tone,
            },
            Section {
                id: "tone.region",
                title: "Area detection",
                body: sec_tone_region,
            },
            Section {
                id: "tone.guide",
                title: "Guide",
                body: sec_tone_guide,
            },
        ],
        Tool::Wand => vec![
            Section {
                id: "wand.opts",
                title: "Auto select",
                body: sec_wand,
            },
            Section {
                id: "wand.guide",
                title: "Guide",
                body: sec_wand_guide,
            },
        ],
        Tool::Select => vec![Section {
            id: "select.opts",
            title: "Selection",
            body: sec_select,
        }],
        Tool::Eyedrop => vec![Section {
            id: "eyedrop.guide",
            title: "Guide",
            body: sec_eyedrop,
        }],
        Tool::Liquify => vec![Section {
            id: "liquify.opts",
            title: "Liquify",
            body: sec_liquify,
        }],
        Tool::Pan => vec![Section {
            id: "pan.guide",
            title: "Guide",
            body: sec_pan,
        }],
        Tool::Figure => vec![
            Section {
                id: "figure.brush",
                title: "Brush",
                body: brush_sliders,
            },
            Section {
                id: "figure.dynamics",
                title: "Dynamics",
                body: dynamics_editor,
            },
            Section {
                id: "figure.opts",
                title: "Figure",
                body: sec_figure,
            },
            Section {
                id: "figure.guide",
                title: "Guide",
                body: sec_figure_guide,
            },
        ],
        Tool::Gradient => vec![
            Section {
                id: "grad.info",
                title: "Gradient",
                body: sec_gradient_info,
            },
            Section {
                id: "grad.opts",
                title: "Ramp",
                body: sec_gradient_opts,
            },
            Section {
                id: "grad.set",
                title: "Gradient set",
                body: sec_gradient_set,
            },
            Section {
                id: "grad.guide",
                title: "Guide",
                body: sec_gradient_guide,
            },
        ],
        _ => Vec::new(),
    }
}
