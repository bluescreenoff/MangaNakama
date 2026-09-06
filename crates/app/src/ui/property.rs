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
    let sections = palette_rows(app);
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
    for (s, rows) in sections {
        group_caption(ui, s.title);
        for r in rows {
            (r.body)(ui, app);
        }
    }
}

/// The active Transform's panel (TRIAGE 148 + row 130): flip buttons
/// (T-021), the numeric fields (TR-031-033: scale %, rotation deg, position)
/// and the 9-cell reference point picker (TR-003). Every field recomposes
/// the absolute params through `AppCmd::TransformUpdate` — the same
/// derivation the drag gestures use — so numbers and handles can never
/// disagree.
/// `I-005` — CSP Tool Settings ▸ Image settings ▸ **Interpolation method**.
///
/// Disabled, with the reason said out loud, while a MESH drag is up: the
/// mesh path resamples through `warp_buffer`'s own bilinear over the
/// deformed quads and hands the commit a finished buffer, so the kernel
/// never reaches those pixels. A live-looking dropdown that cannot change
/// anything is the export dialog's Resample row problem, and gets the export
/// dialog's answer.
fn interp_row(ui: &mut egui::Ui, app: &mut App) {
    use mn_core::transform::Interp;

    let mesh = app
        .transform_drag
        .as_ref()
        .is_some_and(|d| d.mesh.is_some());
    let current = app.transform_interp;
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.label("Interpolation");
        ui.add_enabled_ui(!mesh, |ui| {
            egui::ComboBox::from_id_salt("mn.transform.interp")
                .width(120.0)
                .selected_text(current.label())
                .show_ui(ui, |ui| {
                    let mut sel = current;
                    for i in Interp::ALL {
                        ui.selectable_value(&mut sel, i, i.label());
                    }
                    if sel != current {
                        pick = Some(sel);
                    }
                })
                .response
                .on_hover_text(if mesh {
                    "a mesh warp resamples through its own lattice — this \
                     kernel only applies to scale/rotate"
                } else {
                    "CSP's 補間方法, and it matters most when you SHRINK. \
                     Smooth edges (bilinear) reads two pixels per step, so \
                     below about half size a 1 px line can fall between the \
                     samples and disappear outright. High accuracy averages \
                     the whole area a pixel covers, so hairlines come through \
                     grey instead of broken — pick it for reducing lineart. \
                     Hard edges invents no colours at all (1-bit pages, pixel \
                     art); Clear edges sharpens an enlargement."
                });
        });
    });
    if let Some(i) = pick {
        app.push_cmd(AppCmd::SetTransformInterp(i));
    }
}

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
    interp_row(ui, app);
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
pub(super) fn context_title(app: &App) -> String {
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

mod balloon_ink;
mod effect_lines;
mod figure_lines;
mod frames_balloons;
mod gradient;
mod pen;
mod rulers;
mod select;
mod text;
mod tone;

pub(crate) use balloon_ink::*;
pub(crate) use effect_lines::*;
pub(crate) use figure_lines::*;
pub(crate) use frames_balloons::*;
pub(crate) use gradient::*;
pub(crate) use pen::*;
pub(crate) use rulers::*;
pub(crate) use select::*;
pub(crate) use text::*;
pub(crate) use tone::*;


// --- the row / section registry --------------------------------------------
//
// Lane B1 (`docs/plans/2026-09-06-effect-lines-parity.md`): the palette used
// to be a list of SECTIONS with one body each, and the wrench window could
// only hide a whole section. The unit is now the ROW — one control — so the
// window can hide and reorder them individually, which is what the owner
// asked for ("checkbox/eye icon click for each setting ... and you should be
// able to reorder them too").
//
// A section that has NOT been split yet keeps working: `rows` empty means
// "the section body IS my single row", with the section id as the row id.
// That is how frames/balloons, tone, select, gradient and rulers still draw
// while lane B2 converts them.

/// The "always applies" predicate. Rust has no struct field defaults, so the
/// const constructors below fill this in for rows and sections with no sub
/// tool condition.
fn always(_: &App) -> bool {
    true
}

/// The body of a split section, which has none of its own. Unreachable in
/// practice — `row_list` only falls back to `body` when `rows` is empty.
fn nothing(_: &mut egui::Ui, _: &mut App) {}

/// The Figure tool's BRUSH half only applies while the armed sub tool INKS.
/// With an effect-line preset armed (Stream line, Saturated line, either
/// flash) the generator never touches the brush, so Brush and Dynamics sat in
/// the palette taking space and doing nothing.
fn figure_inks(app: &App) -> bool {
    !app.figure_mode.generates()
}

/// One control of the Tool Property palette: the unit the settings window's
/// eye toggles and the up/down arrows move around.
#[derive(Clone, Copy)]
pub(crate) struct Row {
    /// Stable id, `<section id>.<name>`. SHIPPED API — it is what ui.txt's
    /// `prop_hidden=` and `prop_order=` lines carry, so renaming one silently
    /// un-hides a row the artist hid.
    pub id: &'static str,
    pub label: &'static str,
    pub body: fn(&mut egui::Ui, &mut App),
    /// False = the row means nothing for the armed sub tool: the palette
    /// skips it, the settings window greys it and says why.
    pub applies: fn(&App) -> bool,
}

const fn row(id: &'static str, label: &'static str, body: fn(&mut egui::Ui, &mut App)) -> Row {
    Row {
        id,
        label,
        body,
        applies: always,
    }
}

/// A row that only means something for SOME sub tools — the condition that
/// used to be an `if` around the control inside a section body. The palette
/// leaves it out; the settings window greys it and says why.
const fn row_when(
    id: &'static str,
    label: &'static str,
    body: fn(&mut egui::Ui, &mut App),
    applies: fn(&App) -> bool,
) -> Row {
    Row {
        id,
        label,
        body,
        applies,
    }
}

/// One named group of rows in the Tool Property palette.
#[derive(Clone, Copy)]
pub(crate) struct Section {
    pub id: &'static str,
    pub title: &'static str,
    /// The converted sections list their rows here. EMPTY = not split yet,
    /// and the section draws as ONE row (id = section id, label = title).
    pub rows: &'static [Row],
    pub body: Option<fn(&mut egui::Ui, &mut App)>,
    pub applies: fn(&App) -> bool,
}

impl Section {
    /// The rows this section draws — the whole-section fallback included, so
    /// callers never have to care whether a section has been split.
    pub(crate) fn row_list(&self) -> Vec<Row> {
        if self.rows.is_empty() {
            vec![Row {
                id: self.id,
                label: self.title,
                body: self.body.unwrap_or(nothing),
                applies: self.applies,
            }]
        } else {
            self.rows.to_vec()
        }
    }
}

/// An unsplit section: its body is its single row.
const fn sec(id: &'static str, title: &'static str, body: fn(&mut egui::Ui, &mut App)) -> Section {
    Section {
        id,
        title,
        rows: &[],
        body: Some(body),
        applies: always,
    }
}

/// An unsplit section that only applies to some sub tools.
const fn sec_when(
    id: &'static str,
    title: &'static str,
    body: fn(&mut egui::Ui, &mut App),
    applies: fn(&App) -> bool,
) -> Section {
    Section {
        id,
        title,
        rows: &[],
        body: Some(body),
        applies,
    }
}

/// A section that has been split into rows.
const fn sec_rows(id: &'static str, title: &'static str, rows: &'static [Row]) -> Section {
    Section {
        id,
        title,
        rows,
        body: None,
        applies: always,
    }
}

// --- the text panel, converted (B1.4) --------------------------------------

const ROWS_FONT: &[Row] = &[
    row("text.font.font", "Font", row_text_font),
    row("text.font.size", "Size", row_text_size),
];
const ROWS_DIR: &[Row] = &[
    row(
        "text.dir.vertical",
        "Vertical / horizontal",
        row_text_vertical,
    ),
    row("text.dir.auto_tcy", "Auto tate-chu-yoko", row_text_auto_tcy),
];
const ROWS_ALIGN: &[Row] = &[
    row("text.align.rows", "Rows", row_text_align_rows),
    row("text.align.in_frame", "In frame", row_text_in_frame),
];
const ROWS_SPACING: &[Row] = &[
    row(
        "text.spacing.line_mode",
        "Line spacing mode",
        row_text_line_mode,
    ),
    row("text.spacing.line", "Line spacing", row_text_line),
    row("text.spacing.letter", "Char space", row_text_letter),
];
const ROWS_STYLE: &[Row] = &[
    row(
        "text.style.marks",
        "Bold / italic / underline",
        row_text_marks,
    ),
    row("text.style.color", "Text colour", row_text_color),
];
const ROWS_RUBY: &[Row] = &[
    row("text.ruby.reading", "Reading", row_ruby_reading),
    row("text.ruby.size", "Reading size", row_ruby_size),
    row("text.ruby.gap", "Reading gap", row_ruby_gap),
    row("text.ruby.adjust", "Reading adjust", row_ruby_adjust),
    row("text.ruby.along", "Reading along", row_ruby_along),
];

const SEC_WORKSTYLE: Section = sec("text.workstyle", "Text style", sec_text_workstyle);
const SEC_FONT: Section = sec_rows("text.font", "Font", ROWS_FONT);
const SEC_DIR: Section = sec_rows("text.dir", "Direction", ROWS_DIR);
const SEC_ALIGN: Section = sec_rows("text.align", "Align", ROWS_ALIGN);
const SEC_SPACING: Section = sec_rows("text.spacing", "Spacing", ROWS_SPACING);
const SEC_STYLE: Section = sec_rows("text.style", "Style", ROWS_STYLE);
const SEC_EDGE: Section = sec("text.edge", "Edge", sec_text_edge);
const SEC_RUBY: Section = sec_rows("text.ruby", "Furigana", ROWS_RUBY);
const SEC_TEXT_GUIDE: Section = sec("text.guide", "Guide", sec_text_guide);

/// The text panel's DEFAULT order (B1.4, Fable's ruling from the owner's
/// example): Direction sits directly above Align, so the vertical/horizontal
/// switch and Left/Center/Right are neighbours. Style and Furigana, which
/// used to be wedged between them, moved below Spacing.
const TEXT_SECTIONS: &[Section] = &[
    SEC_WORKSTYLE,
    SEC_FONT,
    SEC_DIR,
    SEC_ALIGN,
    SEC_SPACING,
    SEC_STYLE,
    SEC_EDGE,
    SEC_RUBY,
    SEC_TEXT_GUIDE,
];

// --- the panels lane B2 converted ------------------------------------------
//
// The row arrays live beside the fns they call (`property/*.rs`); this is
// only where a section gets its id and its caption, so one file lists every
// section this build has.

const SEC_FRAME_TOOL: Section = sec_rows("frame.tool", "Frame", ROWS_FRAME_TOOL);
const SEC_BALLOON_INK: Section = sec_rows("balloon.ink", "Colour", ROWS_BALLOON_INK);
const SEC_BALLOON_TAIL: Section = sec_rows("balloon.tail", "Tail", ROWS_BALLOON_TAIL);
const SEC_OBJ_BALLOON: Section = sec_rows("obj.balloon", "Balloon", ROWS_OBJ_BALLOON);
const SEC_OBJ_INK: Section = sec_rows("obj.balloon.ink", "Colour", ROWS_OBJ_INK);
const SEC_OBJ_TAIL: Section = sec_rows("obj.balloon.tail", "Tail", ROWS_OBJ_TAIL);
const SEC_OBJ_FRAME: Section = sec_rows("obj.frame", "Frame border", ROWS_OBJ_FRAME);
const SEC_OBJ_GEN: Section = sec_rows("obj.gen", "Effect lines", ROWS_OBJ_GEN);
const SEC_OBJ_GEN_DENSITY: Section =
    sec_rows("obj.gen.density", "Density", ROWS_OBJ_GEN_DENSITY);
const SEC_FIGURE: Section = sec_rows("figure.opts", "Figure", ROWS_FIGURE);
const SEC_FIGURE_WOBBLE: Section = sec_rows("figure.wobble", "Wobble", ROWS_FIGURE_WOBBLE);
const SEC_TONE: Section = sec_rows("tone.screen", "Tone", ROWS_TONE);
const SEC_TONE_REGION: Section = sec_rows("tone.region", "Area detection", ROWS_TONE_REGION);
const SEC_SELECT: Section = sec_rows("select.opts", "Selection", ROWS_SELECT);
const SEC_PICK_LAYER: Section = sec_rows("obj.picklayer", "Select layer", ROWS_PICK_LAYER);
const SEC_WAND: Section = sec_rows("wand.opts", "Auto select", ROWS_WAND);
const SEC_FILL: Section = sec_rows("fill.opts", "Fill", ROWS_FILL);
const SEC_GRAD_INFO: Section = sec_rows("grad.info", "Gradient", ROWS_GRAD_INFO);
const SEC_GRAD_OPTS: Section = sec_rows("grad.opts", "Ramp", ROWS_GRAD_OPTS);
const SEC_GRAD_SET: Section = sec_rows("grad.set", "Gradient set", ROWS_GRAD_SET);
const SEC_RULER_TOOL: Section = sec_rows("ruler.tool", "Create ruler", ROWS_RULER_TOOL);
const SEC_RULER_SNAP: Section = sec_rows("ruler.snap", "Snapping", ROWS_RULER_SNAP);

/// Every section this build has SPLIT into rows — the migration table for
/// `prop_hidden`, which used to hold section ids. An unsplit section is
/// absent on purpose: its id already IS its row id, so there is nothing to
/// migrate and removing it would un-hide it.
const SPLIT_SECTIONS: &[Section] = &[
    SEC_FONT,
    SEC_DIR,
    SEC_ALIGN,
    SEC_SPACING,
    SEC_STYLE,
    SEC_RUBY,
    SEC_FRAME_TOOL,
    SEC_BALLOON_INK,
    SEC_BALLOON_TAIL,
    SEC_OBJ_BALLOON,
    SEC_OBJ_INK,
    SEC_OBJ_TAIL,
    SEC_OBJ_FRAME,
    SEC_OBJ_GEN,
    SEC_OBJ_GEN_DENSITY,
    SEC_FIGURE,
    SEC_FIGURE_WOBBLE,
    SEC_TONE,
    SEC_TONE_REGION,
    SEC_SELECT,
    SEC_PICK_LAYER,
    SEC_WAND,
    SEC_FILL,
    SEC_GRAD_INFO,
    SEC_GRAD_OPTS,
    SEC_GRAD_SET,
    SEC_RULER_TOOL,
    SEC_RULER_SNAP,
];

/// A `prop_hidden=` line written before lane B1 named SECTIONS. Expand each
/// such id into the row ids that section now has, so "I hid Furigana" still
/// means Furigana is hidden after the update rather than silently coming
/// back. Unsplit ids are left exactly as written.
pub(crate) fn migrate_hidden(hidden: &mut std::collections::BTreeSet<String>) {
    for s in SPLIT_SECTIONS {
        if hidden.remove(s.id) {
            for r in s.rows {
                hidden.insert(r.id.to_owned());
            }
        }
    }
}

/// Decode ui.txt's `prop_hidden=` (comma-joined ids), migration included.
pub(crate) fn hidden_from_line(line: &str) -> std::collections::BTreeSet<String> {
    let mut set: std::collections::BTreeSet<String> = line
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    migrate_hidden(&mut set);
    set
}

/// Decode ui.txt's `prop_order=` (one JSON object, context id → id list).
/// A line this build cannot read costs the ORDER and nothing else: every
/// panel then draws in its default order, which is recoverable, where a
/// half-parsed list is not.
pub(crate) fn order_from_json(line: &str) -> std::collections::BTreeMap<String, Vec<String>> {
    serde_json::from_str(line.trim()).unwrap_or_default()
}

/// The `prop_order=` line for the save side. An empty map writes an EMPTY
/// line rather than `{}`, so a user who never reordered anything does not
/// dirty ui.txt on every start.
pub(crate) fn order_to_json(map: &std::collections::BTreeMap<String, Vec<String>>) -> String {
    if map.is_empty() {
        String::new()
    } else {
        serde_json::to_string(map).unwrap_or_default()
    }
}

// --- order + visibility ----------------------------------------------------

/// The key `prop_order` is stored under: the tool, or — under the Operation
/// tool, whose palette swaps for the selected object — which KIND of object
/// is selected. Reordering the text rows while a text box is selected must
/// not reorder the balloon rows.
pub(crate) fn prop_context(app: &App) -> String {
    if app.tool == Tool::Object {
        return if app.object_mode == crate::cmd::ObjectMode::PickLayer {
            "obj.picklayer"
        } else if app.text_sel.is_some() {
            "obj.text"
        } else if app.balloon_sel.is_some() {
            "obj.balloon"
        } else if app
            .gen_sel
            .is_some_and(|li| app.doc.layers.get(li).is_some_and(|l| l.genlines.is_some()))
        {
            "obj.gen"
        } else if app.object_sel.is_some() {
            "obj.frame"
        } else {
            "obj"
        }
        .to_owned();
    }
    format!("{:?}", app.tool)
}

/// Where `id` sits in a stored order — `usize::MAX` for one that is not in
/// it, which a STABLE sort then leaves in default order at the end. That is
/// both halves of the rule at once: an id the artist never moved appends in
/// default order, and an id in the list that no longer exists is ignored
/// because nothing asks for it.
fn order_key(order: &[String], id: &str) -> usize {
    order.iter().position(|s| s == id).unwrap_or(usize::MAX)
}

/// The sections of the current context in the artist's stored order.
pub(crate) fn ordered_sections(app: &App) -> Vec<Section> {
    let mut v = prop_sections(app);
    if let Some(order) = app.prop_order.get(&prop_context(app)) {
        v.sort_by_key(|s| order_key(order, s.id));
    }
    v
}

/// One section's rows in the artist's stored order.
pub(crate) fn ordered_rows(app: &App, s: &Section) -> Vec<Row> {
    let mut v = s.row_list();
    if let Some(order) = app.prop_order.get(&prop_context(app)) {
        v.sort_by_key(|r| order_key(order, r.id));
    }
    v
}

/// What the compact palette draws: sections in the artist's order, each with
/// its rows in theirs, minus the rows an eye toggle hid and the rows that
/// mean nothing for the armed sub tool. A section left with no rows is
/// dropped entirely — a lone caption over nothing is what "hide it" was for.
///
/// ONE definition, called by `tool_property_body` and by the tests, so a
/// test can never pass against a palette that does something else.
pub(crate) fn palette_rows(app: &App) -> Vec<(Section, Vec<Row>)> {
    let mut out = Vec::new();
    for s in ordered_sections(app) {
        if !(s.applies)(app) {
            continue;
        }
        let rows: Vec<Row> = ordered_rows(app, &s)
            .into_iter()
            .filter(|r| (r.applies)(app) && !app.prop_hidden.contains(r.id))
            .collect();
        if !rows.is_empty() {
            out.push((s, rows));
        }
    }
    out
}

/// `palette_rows` flattened to the id sequence — the seam the order and
/// visibility tests drive without building a frame.
#[cfg(test)]
pub(crate) fn palette_row_ids(app: &App) -> Vec<&'static str> {
    palette_rows(app)
        .into_iter()
        .flat_map(|(_, rows)| rows.into_iter().map(|r| r.id))
        .collect()
}

/// The whole context flattened — sections in order, each followed by its own
/// rows in order. This is what `prop_order` stores: sorting by position in
/// ONE flat list gives the right answer for both groups, because a section's
/// rows always sit between it and the next section.
pub(crate) fn flat_order(app: &App) -> Vec<String> {
    let mut out = Vec::new();
    for s in ordered_sections(app) {
        out.push(s.id.to_owned());
        if !s.rows.is_empty() {
            out.extend(ordered_rows(app, &s).iter().map(|r| r.id.to_owned()));
        }
    }
    out
}

/// Move one section (or one row inside its section) up or down by one, and
/// write the whole context's order down. `is_section` matters because an
/// unsplit section's id is ALSO its row id.
pub(crate) fn move_entry(app: &mut App, id: &str, is_section: bool, up: bool) {
    let ctx = prop_context(app);
    // The group this id belongs to, as ids, in current effective order.
    let group: Vec<String> = if is_section {
        ordered_sections(app)
            .iter()
            .map(|s| s.id.to_owned())
            .collect()
    } else {
        let Some(s) = ordered_sections(app)
            .into_iter()
            .find(|s| s.row_list().iter().any(|r| r.id == id))
        else {
            return;
        };
        ordered_rows(app, &s)
            .iter()
            .map(|r| r.id.to_owned())
            .collect()
    };
    let Some(pos) = group.iter().position(|s| s == id) else {
        return;
    };
    let other = if up {
        if pos == 0 {
            return;
        }
        pos - 1
    } else {
        if pos + 1 >= group.len() {
            return;
        }
        pos + 1
    };
    let mut flat = flat_order(app);
    let (Some(a), Some(b)) = (
        flat.iter().position(|s| *s == group[pos]),
        flat.iter().position(|s| *s == group[other]),
    ) else {
        return;
    };
    flat.swap(a, b);
    app.prop_order.insert(ctx, flat);
}

/// Row 55 (CSP liquify): the seven modes, strength and radius. Descriptions
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

const SEC_OBJ_GUIDE: Section = sec("obj.guide", "Guide", sec_obj_guide);

/// Every section of the CURRENT context, in DEFAULT order (the stored order
/// is applied by `ordered_sections`). The Operation tool swaps its whole
/// list for the selected object's editors — Tool Property edits the item
/// (owner's fix 7).
pub(super) fn prop_sections(app: &App) -> Vec<Section> {
    let mut v = prop_sections_for_tool(app);
    if matches!(app.doc.active_layer().kind, mn_core::LayerKind::Fill(_)) {
        v.insert(0, sec("live.fill", "Live layer", sec_live_fill));
    }
    v
}

fn prop_sections_for_tool(app: &App) -> Vec<Section> {
    match app.tool {
        Tool::Object => {
            // S-001: the layer pick is its own sub tool, and nothing else
            // in the Operation tool applies while it is the active one.
            if app.object_mode == crate::cmd::ObjectMode::PickLayer {
                vec![SEC_PICK_LAYER]
            } else if app.text_sel.is_some() {
                TEXT_SECTIONS.to_vec()
            } else if app.balloon_sel.is_some() {
                vec![SEC_OBJ_BALLOON, SEC_OBJ_INK, SEC_OBJ_TAIL, SEC_OBJ_GUIDE]
            } else if app
                .gen_sel
                .is_some_and(|li| app.doc.layers.get(li).is_some_and(|l| l.genlines.is_some()))
            {
                // Owner's fix 7 again, for the one object family that never
                // got it: with a run selected the palette said "click a
                // text box, balloon or panel" and offered nothing, so a
                // placed effect-line set could only be re-tuned by deleting
                // it and dragging a new one.
                vec![SEC_OBJ_GEN, SEC_OBJ_GEN_DENSITY, SEC_OBJ_GUIDE]
            } else if app.object_sel.is_some() {
                vec![SEC_OBJ_FRAME, SEC_OBJ_GUIDE]
            } else {
                vec![SEC_OBJ_GUIDE]
            }
        }
        Tool::Text => TEXT_SECTIONS.to_vec(),
        Tool::Balloon => vec![
            sec("balloon.line", "Balloon line", sec_balloon_line),
            SEC_BALLOON_INK,
            SEC_BALLOON_TAIL,
            sec("balloon.guide", "Guide", sec_balloon_guide),
        ],
        Tool::Frame => vec![
            SEC_FRAME_TOOL,
            sec("frame.guide", "Guide", sec_frame_guide),
        ],
        Tool::Fill => vec![
            SEC_FILL,
            sec("fill.guide", "Guide", sec_wand_guide),
        ],
        Tool::Tone => vec![
            SEC_TONE,
            SEC_TONE_REGION,
            sec("tone.guide", "Guide", sec_tone_guide),
        ],
        Tool::Wand => vec![
            SEC_WAND,
            sec("wand.guide", "Guide", sec_wand_guide),
        ],
        Tool::Select => vec![SEC_SELECT],
        Tool::Eyedrop => vec![sec("eyedrop.guide", "Guide", sec_eyedrop)],
        Tool::Liquify => vec![sec("liquify.opts", "Liquify", sec_liquify)],
        Tool::Pan => vec![sec("pan.guide", "Guide", sec_pan)],
        Tool::Ruler => vec![
            SEC_RULER_TOOL,
            SEC_RULER_SNAP,
            sec("ruler.guide", "Guide", sec_ruler_guide),
        ],
        Tool::Figure => vec![
            // Only while the armed sub tool INKS: an effect-line preset
            // generates its own layer and never touches the brush, so these
            // two used to sit here doing nothing (B1.1).
            sec_when("figure.brush", "Brush", brush_sliders, figure_inks),
            sec_when("figure.dynamics", "Dynamics", dynamics_editor, figure_inks),
            SEC_FIGURE,
            // The eight wobbles, split off the Figure section in the parity
            // round: nineteen numbers in one column is a wall nobody reads,
            // and "what is a line?" and "how much does the hand vary?" are
            // two different questions. Right after Figure, because it is the
            // second half of the same answer.
            SEC_FIGURE_WOBBLE,
            sec("figure.guide", "Guide", sec_figure_guide),
        ],
        Tool::Gradient => vec![
            SEC_GRAD_INFO,
            SEC_GRAD_OPTS,
            SEC_GRAD_SET,
            sec("grad.guide", "Guide", sec_gradient_guide),
        ],
        _ => Vec::new(),
    }
}
