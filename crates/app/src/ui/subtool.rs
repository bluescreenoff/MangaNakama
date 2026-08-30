//! Sub Tool list: every tool's own sub-tool palette — brush presets with
//! live stroke previews for Pen/Eraser, mode rows for everything else.
//! `ensure_preview` honours the 1-per-frame budget set at the top of
//! `build` (startup trickles, never hitches).

use std::path::PathBuf;

use super::icons::Icon;
use super::preview;
use super::theme;
use super::widgets::{group_caption, paint_icon};
use crate::app::App;
use crate::cmd::{AppCmd, BalloonMode, FillMode, SelectMode, Tool};
// The group captions are DATA (`crate::subtools`), not literals typed here:
// a shortcut can name a tab, so the tab's name has to be somewhere both this
// file and the keymap can point at. Owner ask 2026-08-25.
use crate::subtools::group;

// --- sub tool list ------------------------------------------------------

/// Fetch (or lazily build) the stroke preview for one preset.
fn ensure_preview(
    app: &mut App,
    ctx: &egui::Context,
    path: &PathBuf,
) -> Option<egui::TextureHandle> {
    if let Some(entry) = app.brush_previews.get(path) {
        return entry.clone();
    }
    if app.preview_budget == 0 {
        // Not generated yet; keep the UI responsive and repaint soon.
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
        return None;
    }
    app.preview_budget -= 1;
    let tex = preview::generate(ctx, path);
    app.brush_previews.insert(path.clone(), tex.clone());
    tex
}

/// CSP's strict chain: every tool has its own Sub Tool list. Stroke tools
/// list the brush presets; the rest list their modes (fill referents, select
/// shapes, balloon shapes, frame cuts...), each remembering its own Tool
/// Property values.
pub(super) fn sub_tool_list(ui: &mut egui::Ui, app: &mut App) {
    match app.tool {
        // Only the two INK tools list brush presets. The selection pen and
        // eraser used to ride this list too, back when they were their own
        // strip cells; they are Selection sub tools now (2026-08-23), so
        // holding one shows the Selection list with that row lit — CSP's
        // shape, and the Tool Property panel still hands them the brush.
        Tool::Pen | Tool::Eraser => brush_sub_tools(ui, app),
        // Most mode lists are three or four rows, but Figure runs thirteen
        // over three captions and the tail was simply unreachable. The
        // brush list scrolls itself, so only this arm gets a ScrollArea —
        // wrapping the whole fn would nest two of them.
        _ => {
            egui::ScrollArea::vertical()
                .id_salt("mn.subtool.modes")
                .auto_shrink([false, false])
                .show(ui, |ui| mode_sub_tools(ui, app));
        }
    }
}

/// One selectable mode row: tool icon + name, same shape as the brush rows so
/// the palette reads uniformly.
fn mode_row(ui: &mut egui::Ui, selected: bool, icon: Icon, name: &str) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 22.0), egui::Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 2.0, theme::c().sel_row);
        p.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(2.0, rect.height())),
            0.0,
            theme::c().accent,
        );
    } else if resp.hovered() {
        p.rect_filled(rect, 2.0, theme::c().hover);
    }
    let ir = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 15.0, rect.center().y),
        egui::vec2(15.0, 15.0),
    );
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
    let color = if selected {
        theme::c().text_strong
    } else {
        theme::c().text
    };
    let galley = super::widgets::ellipsis(
        ui,
        name,
        egui::FontId::proportional(11.5),
        color,
        (rect.right() - 4.0 - (rect.left() + 28.0)).max(10.0),
    );
    p.galley(
        egui::pos2(rect.left() + 28.0, rect.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
    resp
}

/// The Selection tool's PAINT sub tools, folded in from the tool strip
/// (owner, 2026-08-23: "select pen duplicates the G-pen with the same
/// icon"). CSP files 選択ペン / 選択消し as Selection sub tools with a fixed
/// create-type; ours carry theirs as a `Tool`, which is what the canvas
/// stroke paths already key off. A table so the growth path is one line: a
/// soft selection pen would be a third row here and a third `SubTool`
/// variant, nothing else.
const SEL_PAINT: [(&str, &str, Icon, Tool); 2] = [
    (
        "Selection pen",
        "paint with the active brush — the stroke ADDS to the selection",
        Icon::SelPen,
        Tool::SelPen,
    ),
    (
        "Erase selection",
        "the same stroke, subtracting from the selection",
        Icon::SelEraser,
        Tool::SelEraser,
    ),
];

fn mode_sub_tools(ui: &mut egui::Ui, app: &mut App) {
    use crate::cmd::{FrameMode, PanMode};
    match app.tool {
        Tool::Fill => {
            // CSP's Fill sub-tool list, in its order: three click-aimed
            // 参照 rows, then the two path-aimed ones. Picking a 参照 row
            // also returns to the click sub tool — that is what those rows
            // ARE in CSP.
            group_caption(ui, group::FILL);
            let click = app.fill_mode == FillMode::Click;
            let refer = app.fill_opts.refer;
            let mut pick_refer: Option<mn_core::FillRefer> = None;
            if mode_row(
                ui,
                click && refer == mn_core::FillRefer::All,
                Icon::Fill,
                "Refer other layers",
            )
            .clicked()
            {
                pick_refer = Some(mn_core::FillRefer::All);
            }
            if mode_row(
                ui,
                click && refer == mn_core::FillRefer::Active,
                Icon::Fill,
                "Refer editing layer only",
            )
            .clicked()
            {
                pick_refer = Some(mn_core::FillRefer::Active);
            }
            if mode_row(
                ui,
                click && refer == mn_core::FillRefer::Reference,
                Icon::Fill,
                "Refer reference layer",
            )
            .clicked()
            {
                pick_refer = Some(mn_core::FillRefer::Reference);
            }
            if let Some(r) = pick_refer {
                app.fill_opts.refer = r;
                app.push_cmd(AppCmd::SetFillMode(FillMode::Click));
            }
            if mode_row(
                ui,
                app.fill_mode == FillMode::Enclose,
                Icon::Wand,
                "Enclose and fill",
            )
            .on_hover_text("drag right around a messy region — every closed area inside it fills")
            .clicked()
            {
                app.push_cmd(AppCmd::SetFillMode(FillMode::Enclose));
            }
            if mode_row(
                ui,
                app.fill_mode == FillMode::Lasso,
                Icon::Select,
                "Lasso fill",
            )
            .on_hover_text("drag a shape and it is painted as drawn — lines are ignored")
            .clicked()
            {
                app.push_cmd(AppCmd::SetFillMode(FillMode::Lasso));
            }
            // Row 119 / FI-005: the after-flatting pass. Same family, same
            // freehand drag, opposite question — not "what did I enclose"
            // but "what did I MISS".
            if mode_row(
                ui,
                app.fill_mode == FillMode::Leftover,
                Icon::Fill,
                "Leftover pen",
            )
            .on_hover_text(
                "scrub across a finished flat — only the enclosed spots still \
                 empty fill; colour you already laid down is never repainted",
            )
            .clicked()
            {
                app.push_cmd(AppCmd::SetFillMode(FillMode::Leftover));
            }
            // Row 160 / RD-001: CSP's 線修正 group folded in here — same
            // freehand drag, and the thing it cleans up is what the three
            // rows above leave behind.
            if mode_row(ui, app.fill_mode == FillMode::Dust, Icon::Fill, "Remove dust")
                .on_hover_text(
                    "drag around a patch — specks smaller than the size row are cleared, \
                     or the pinholes inside a flat are plugged",
                )
                .clicked()
            {
                app.push_cmd(AppCmd::SetFillMode(FillMode::Dust));
            }
        }
        Tool::Tone => {
            // The nine screen shapes — the choice made BEFORE the click.
            group_caption(ui, group::TONE);
            let mut pick = None;
            for pat in mn_core::tone::TonePattern::ALL {
                if mode_row(
                    ui,
                    app.tone_opts.tone.pattern == pat,
                    Icon::Tone,
                    pat.label(),
                )
                .clicked()
                {
                    pick = Some(pat);
                }
            }
            if let Some(p) = pick {
                let mut o = app.tone_opts;
                o.tone.pattern = p;
                app.push_cmd(AppCmd::SetToneOpts(o));
            }
        }
        Tool::Wand => {
            group_caption(ui, group::AUTO_SELECT);
            let refer = app.wand_opts.refer;
            if mode_row(
                ui,
                refer == mn_core::FillRefer::All,
                Icon::Wand,
                "Refer all layers",
            )
            .clicked()
            {
                app.wand_opts.refer = mn_core::FillRefer::All;
            }
            if mode_row(
                ui,
                refer == mn_core::FillRefer::Active,
                Icon::Wand,
                "Refer editing layer only",
            )
            .clicked()
            {
                app.wand_opts.refer = mn_core::FillRefer::Active;
            }
            if mode_row(
                ui,
                refer == mn_core::FillRefer::Reference,
                Icon::Wand,
                "Refer reference layer",
            )
            .clicked()
            {
                app.wand_opts.refer = mn_core::FillRefer::Reference;
            }
        }
        Tool::Select | Tool::SelPen | Tool::SelEraser => {
            group_caption(ui, group::SELECTION);
            // The four SHAPE sub tools only light while the shape tool is
            // the one in hand: holding the selection pen must not also show
            // "Rectangle" selected, or the list stops saying where you are.
            let m = app.select_mode;
            let shape = app.tool == Tool::Select;
            if mode_row(
                ui,
                shape && m == SelectMode::Rect,
                Icon::Select,
                "Rectangle",
            )
            .clicked()
            {
                app.push_cmd(AppCmd::SetSelectMode(SelectMode::Rect));
            }
            if mode_row(ui, shape && m == SelectMode::Lasso, Icon::Select, "Lasso").clicked() {
                app.push_cmd(AppCmd::SetSelectMode(SelectMode::Lasso));
            }
            if mode_row(
                ui,
                shape && m == SelectMode::Magnetic,
                Icon::Select,
                "Magnetic lasso",
            )
            .on_hover_text(
                "trace roughly along the lineart and the outline snaps to it — Backspace undoes an anchor, Enter closes",
            )
            .clicked()
            {
                app.push_cmd(AppCmd::SetSelectMode(SelectMode::Magnetic));
            }
            if mode_row(
                ui,
                shape && m == SelectMode::Shrink,
                Icon::Wand,
                "Shrink selection",
            )
            .on_hover_text(
                "drag across the empty space — every closed area the path crosses is selected",
            )
            .clicked()
            {
                app.push_cmd(AppCmd::SetSelectMode(SelectMode::Shrink));
            }
            for (name, hint, icon, tool) in SEL_PAINT {
                if mode_row(ui, app.tool == tool, icon, name)
                    .on_hover_text(hint)
                    .clicked()
                {
                    app.push_cmd(AppCmd::SetTool(tool));
                }
            }
        }
        Tool::Frame => {
            let m = app.frame_mode;
            let mut pick: Option<FrameMode> = None;
            group_caption(ui, group::CREATE_FRAME);
            if mode_row(ui, m == FrameMode::Rect, Icon::Object, "Rectangle frame").clicked() {
                pick = Some(FrameMode::Rect);
            }
            if mode_row(ui, m == FrameMode::Polyline, Icon::Frame, "Polyline frame")
                .on_hover_text("click corners, close on the first one (Enter closes, Esc cancels)")
                .clicked()
            {
                pick = Some(FrameMode::Polyline);
            }
            if mode_row(ui, m == FrameMode::Pen, Icon::Pen, "Frame border pen")
                .on_hover_text("draw the panel outline freehand")
                .clicked()
            {
                pick = Some(FrameMode::Pen);
            }
            group_caption(ui, group::CUT_FRAME);
            if mode_row(
                ui,
                m == FrameMode::DivideFolder,
                Icon::Frame,
                "Divide frame folder",
            )
            .on_hover_text("each cut panel becomes its own frame folder (CSP)")
            .clicked()
            {
                pick = Some(FrameMode::DivideFolder);
            }
            if mode_row(
                ui,
                m == FrameMode::DivideBorder,
                Icon::Frame,
                "Divide frame border",
            )
            .on_hover_text("the cut stays inside the same folder")
            .clicked()
            {
                pick = Some(FrameMode::DivideBorder);
            }
            if let Some(p) = pick {
                app.frame_mode = p;
                app.frame_poly = None;
                app.frame_pen = None;
            }
        }
        Tool::Balloon => {
            group_caption(ui, group::BALLOON);
            for m in [
                BalloonMode::Ellipse,
                BalloonMode::Round,
                BalloonMode::Draw,
                BalloonMode::Tail,
            ] {
                if mode_row(ui, app.balloon_mode == m, Icon::Balloon, m.label()).clicked() {
                    app.balloon_mode = m;
                }
            }
        }
        Tool::Text => {
            group_caption(ui, group::TEXT);
            mode_row(ui, true, Icon::Text, "Text");
        }
        Tool::Object => {
            group_caption(ui, group::OPERATION);
            use crate::cmd::ObjectMode;
            let om = app.object_mode;
            if mode_row(ui, om == ObjectMode::Object, Icon::Object, "Object").clicked() {
                app.object_mode = ObjectMode::Object;
            }
            // S-001: the pick is an OPERATION sub tool in CSP, not a layer
            // palette button — you are pointing at the page, not at a list.
            if mode_row(
                ui,
                om == ObjectMode::PickLayer,
                Icon::Eyedrop,
                "Select layer",
            )
            .on_hover_text("click a pixel and the Layer palette jumps to whichever layer drew it")
            .clicked()
            {
                app.object_mode = ObjectMode::PickLayer;
            }
        }
        Tool::Figure => {
            // CSP's Figure tool is three sub tool groups (直接描画 / 流線 /
            // 集中線) — same shape here (owner order 2026-08-22). The line
            // groups' extra rows are PRESETS: picking one arms the mode and
            // writes its parameters; the knobs stay editable in Tool
            // Property (a tweaked set simply highlights no row, like a
            // modified brush preset).
            group_caption(ui, group::DIRECT_DRAW);
            use crate::cmd::FigureMode;
            for (m, icon) in [
                (FigureMode::Line, Icon::Figure),
                (FigureMode::Rect, Icon::Rect),
                (FigureMode::Ellipse, Icon::Ellipse),
                (FigureMode::Polygon, Icon::Poly),
                // Row 157 / FG-002: the two-stage arc sits next to the
                // straight line it starts life as.
                (FigureMode::Arc, Icon::Arc),
                // Rows 84/85: the curve rides in Direct draw because that
                // is what it does — it inks with your brush, unlike the
                // generator groups below.
                (FigureMode::Curve, Icon::Vector),
                // Row 156 / FG-020: also Direct draw — it inks the active
                // layer with your brush; the hold is what makes it a figure.
                (FigureMode::Smart, Icon::SmartShape),
            ] {
                if mode_row(ui, app.figure_mode == m, icon, m.label()).clicked() {
                    app.figure_mode = m;
                    app.figure_poly = None;
                    app.figure_stage2 = None;
                    app.smart_shape = None;
                }
            }
            // The preset rows carry a WHOLE `FigureLineOpts` now, built in
            // mm and degrees at the page's dpi (see the constructors): a
            // row that only wrote count and width could not express the
            // gap, bundling and split jitters the density round added,
            // and a half-written preset is how the sets drifted.
            let dpi = app.tone_dpi();
            use crate::cmd::FigureLineOpts as FLO;
            group_caption(ui, group::STREAM_LINE);
            for (label, opts) in [
                ("Stream line", FLO::stream_dpi(dpi)),
                ("Dense stream", FLO::dense_stream_dpi(dpi)),
                ("Sparse stream", FLO::sparse_stream_dpi(dpi)),
            ] {
                let on = app.figure_mode == FigureMode::Stream && app.figure_stream.same_as(&opts);
                if mode_row(ui, on, Icon::StreamLines, label)
                    .on_hover_text("drag along the motion — a fresh speed-line layer each drag")
                    .clicked()
                {
                    app.figure_mode = FigureMode::Stream;
                    app.figure_poly = None;
                    app.figure_stream = FLO {
                        seed: app.figure_stream.seed,
                        ..opts
                    };
                }
            }
            group_caption(ui, group::SATURATED_LINE);
            for (label, opts) in [
                ("Saturated line", FLO::focus_dpi(dpi)),
                ("Dense saturated line", FLO::dense_focus_dpi(dpi)),
                ("Dark burst", FLO::dark_burst_dpi(dpi)),
            ] {
                let on = app.figure_mode == FigureMode::Focus && app.figure_focus.same_as(&opts);
                if mode_row(ui, on, Icon::FocusLines, label)
                    .on_hover_text(
                        "drag from the convergence point outward — a fresh focus-line layer each drag",
                    )
                    .clicked()
                {
                    app.figure_mode = FigureMode::Focus;
                    app.figure_poly = None;
                    app.figure_focus = FLO {
                        seed: app.figure_focus.seed,
                        ..opts
                    };
                }
            }
            // ウニフラッシュ, the pro-page audit's #1 IMPOSSIBLE — same
            // group because it is the same centre-out gesture on the same
            // knobs, only the rays are filled spikes. `width` reads as the
            // spike base width in px here, so the rows carry values that
            // suit a flash rather than a hairline fan.
            for (label, mode, opts) in [
                (
                    "Sea urchin flash",
                    FigureMode::Urchin,
                    FLO::flash_dpi(dpi, 64, 0.85, 0.3),
                ),
                (
                    "Solid flash",
                    FigureMode::SolidFlash,
                    FLO::flash_dpi(dpi, 64, 0.95, 0.45),
                ),
            ] {
                let on = app.figure_mode == mode && app.figure_focus.same_as(&opts);
                if mode_row(ui, on, Icon::UrchinFlash, label)
                    .on_hover_text(
                        "drag from the flash's centre outward — a fresh flash layer each drag",
                    )
                    .clicked()
                {
                    app.figure_mode = mode;
                    app.figure_poly = None;
                    app.figure_focus = FLO {
                        seed: app.figure_focus.seed,
                        ..opts
                    };
                }
            }
        }
        Tool::Gradient => {
            group_caption(ui, group::GRADIENT);
            use crate::cmd::GradMode;
            for m in [
                GradMode::FgToBg,
                GradMode::FgToTransparent,
                GradMode::TransparentToFg,
                GradMode::Freeform,
            ] {
                let row = mode_row(ui, app.grad_mode == m, Icon::Gradient, m.label());
                let row = if m == GradMode::Freeform {
                    row.on_hover_text(
                        "draw TWO lines: the first takes the main colour, the second the sub \
                         colour, and the ramp between them follows both shapes. Enter (or a \
                         click away from them) paints. Draw a THIRD line and up and each one \
                         carries the main colour as it stands when you draw it, with the \
                         colour blending by proximity to every line. Backspace takes the last \
                         line back, Esc cancels.",
                    )
                } else {
                    row
                };
                if row.clicked() {
                    app.grad_mode = m;
                    app.grad_free = None;
                }
            }
        }
        Tool::Eyedrop => {
            // E-014 参照: the same three referents the fill tool offers, in
            // the same order, so the two palettes read as one idea.
            group_caption(ui, group::EYEDROPPER);
            let refer = app.eyedrop_opts.refer;
            for (v, label, hint) in [
                (
                    mn_core::FillRefer::All,
                    "Pick displayed color",
                    "the colour you see, every visible layer flattened",
                ),
                (
                    mn_core::FillRefer::Active,
                    "Pick color from layer",
                    "the editing layer's own ink, over paper white",
                ),
                (
                    mn_core::FillRefer::Reference,
                    "Pick from reference layers",
                    "the reference set only, even where its eyes are off",
                ),
            ] {
                if mode_row(ui, refer == v, Icon::Eyedrop, label)
                    .on_hover_text(hint)
                    .clicked()
                {
                    app.eyedrop_opts.refer = v;
                }
            }
            // E-016 average colour: a single pixel of anti-aliased ink is a
            // colour nobody can see on the page.
            // Rows straight from the registry — these four have `SubTool`
            // variants, so the palette, the command palette and a keys.json
            // binding all reach the same four things by the same names.
            group_caption(ui, group::AVERAGE_COLOR);
            let mut pick = None;
            for &s in crate::subtools::rows(Tool::Eyedrop, group::AVERAGE_COLOR) {
                if mode_row(ui, crate::subtools::is_lit(app, s), Icon::Eyedrop, s.label()).clicked()
                {
                    pick = Some(s);
                }
            }
            if let Some(s) = pick {
                app.push_cmd(AppCmd::SetSubTool(s));
            }
        }
        Tool::Liquify => {
            // The seven modes live in Tool Property, one flat radio list
            // — no sub-tool shapes to mirror here.
        }
        Tool::Pan => {
            group_caption(ui, group::MOVE);
            if mode_row(ui, app.pan_mode == PanMode::Hand, Icon::Pan, "Hand").clicked() {
                app.pan_mode = PanMode::Hand;
            }
            if mode_row(
                ui,
                app.pan_mode == PanMode::Rotate,
                Icon::RotateRight,
                "Rotate",
            )
            .on_hover_text("drag spins the view around the canvas centre")
            .clicked()
            {
                app.pan_mode = PanMode::Rotate;
            }
        }
        Tool::Pen | Tool::Eraser => {
            unreachable!("brush list handles these")
        }
    }
}

/// The preset list, grouped CSP-style with a real stroke preview per row:
/// the owner's imported CSP brushes first, then the MyPaint classics.
fn brush_sub_tools(ui: &mut egui::Ui, app: &mut App) {
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("mn.presets")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if app.presets.is_empty() {
                ui.weak("no assets/brushes/**/*.myb found");
                return;
            }
            let group_of = |p: &std::path::Path| -> &'static str {
                match p
                    .parent()
                    .and_then(|d| d.file_name())
                    .and_then(|n| n.to_str())
                {
                    // The artist's own captures lead the list.
                    Some("mine") => "Mine",
                    Some("csp") => "CSP",
                    Some("krita") => "Krita",
                    Some("classic") => "Classic",
                    Some("imported") => "Imported",
                    _ => "Other",
                }
            };
            for group in ["Mine", "CSP", "Krita", "Classic", "Imported", "Other"] {
                let rows: Vec<(usize, String, PathBuf)> = app
                    .presets
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, p))| group_of(p) == group)
                    .map(|(i, (name, p))| (i, name.clone(), p.clone()))
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                group_caption(ui, group);
                // Only the artist's own groups are editable: the shipped
                // ones come back with the next build, so a rename there
                // would not stick and a delete would not come back at all.
                let owned = matches!(group, "Mine" | "Imported");
                for (i, name, path) in rows {
                    let tex = ensure_preview(app, ui.ctx(), &path);
                    let selected = app.selected_preset == Some(i);
                    let resp = subtool_row(ui, selected, tex.as_ref(), &name);
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                    if owned {
                        organise_menu(&resp, app, &name, &path);
                    }
                }
                ui.add_space(4.0);
            }
        });
    if let Some(i) = clicked {
        let p = app.presets[i].1.clone();
        app.push_cmd(AppCmd::SelectBrush(p));
    }
}
/// The organise half of "brushes without ceremony": Rename / Duplicate /
/// Delete on the preset row itself, since clicking the row already means
/// "use this brush" and a properties pane for three verbs is the ceremony.
/// Rename is an inline box — Enter (or the button) applies, Esc drops it.
fn organise_menu(resp: &egui::Response, app: &mut App, name: &str, path: &PathBuf) {
    resp.context_menu(|ui| {
        ui.set_min_width(170.0);
        // Seed (or re-seed) from the name on disk, so the box can never
        // carry one brush's half-typed name onto the next one.
        let buf = app
            .brush_rename_edit
            .get_or_insert_with(|| (path.clone(), name.to_owned()));
        if buf.0 != *path {
            *buf = (path.clone(), name.to_owned());
        }
        let edit = ui.add(
            egui::TextEdit::singleline(&mut buf.1)
                .hint_text("brush name")
                .desired_width(160.0),
        );
        let entered = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if entered || ui.button("Rename").clicked() {
            let name = std::mem::take(&mut buf.1);
            app.brush_rename_edit = None;
            app.push_cmd(AppCmd::RenameBrush {
                path: path.clone(),
                name,
            });
            ui.close();
            return;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.brush_rename_edit = None;
            ui.close();
            return;
        }
        ui.separator();
        // Only on the SELECTED row: the folded state belongs to the brush
        // Tool Property is currently editing.
        let is_selected = app
            .selected_preset
            .and_then(|i| app.presets.get(i))
            .is_some_and(|(_, p)| p == path);
        if is_selected
            && ui
                .button("Save current settings as brush")
                .on_hover_text(
                    "the brush as TUNED right now — size, wash, ink, texture — \
                     becomes a new sub tool in Mine",
                )
                .clicked()
        {
            app.push_cmd(AppCmd::BrushSaveCurrent);
            ui.close();
        }
        if ui
            .button("Duplicate")
            .on_hover_text("a copy beside it to retune — the original stays as it is")
            .clicked()
        {
            app.push_cmd(AppCmd::DuplicateBrush(path.clone()));
            ui.close();
        }
        if ui
            .button("Delete")
            .on_hover_text("removes the .myb file — there is no undo for this")
            .clicked()
        {
            app.push_cmd(AppCmd::DeleteBrush(path.clone()));
            ui.close();
        }
    });
}

fn subtool_row(
    ui: &mut egui::Ui,
    selected: bool,
    tex: Option<&egui::TextureHandle>,
    name: &str,
) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 22.0), egui::Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 2.0, theme::c().sel_row);
        p.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(2.0, rect.height())),
            0.0,
            theme::c().accent,
        );
    } else if resp.hovered() {
        p.rect_filled(rect, 2.0, theme::c().hover);
    }
    let ir = egui::Rect::from_min_size(
        egui::pos2(
            rect.left() + 6.0,
            rect.center().y - preview::PREVIEW_H as f32 * 0.5,
        ),
        egui::vec2(preview::PREVIEW_W as f32, preview::PREVIEW_H as f32),
    );
    match tex {
        Some(t) => {
            p.image(
                t.id(),
                ir,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            p.rect_filled(ir, 2.0, theme::c().field);
        }
    }
    p.rect_stroke(
        ir,
        2.0,
        egui::Stroke::new(1.0, theme::c().border),
        egui::StrokeKind::Inside,
    );
    let tx = ir.right() + 7.0;
    let color = if selected {
        theme::c().text_strong
    } else {
        theme::c().text
    };
    let galley = super::widgets::ellipsis(
        ui,
        name,
        egui::FontId::proportional(11.5),
        color,
        (rect.right() - 4.0 - tx).max(10.0),
    );
    p.galley(
        egui::pos2(tx, rect.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
    resp
}
