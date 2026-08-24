//! The Auto Actions palette (CSP オートアクション): record a run of layer
//! commands into a named action, keep it, replay it as ONE undo press —
//! and EDIT it, which is the half CSP has and a recorder alone does not
//! (owner 2026-08-21: "auto actions still doesn't work like scratch
//! programming"). Steps can be added by hand from a picker of every kind,
//! retuned in place, dragged into a new order, duplicated and deleted.
//! The model and the four rules that carry it live in `app::actions`.
//!
//! Five shapes worth knowing before editing this file:
//!
//! * **One command bar, at the bottom, aimed at the SELECTED action** —
//!   ● record ▶ play ⧉ duplicate 🗑 delete. They used to repeat on every
//!   row, which put four icons of chrome beside every name in a ~200 px
//!   palette (parity audit T4: CSP has one bar). A row now carries only
//!   what is ABOUT that row: its run tick and its name.
//! * **Sets live in the file, not in `App`** (see `app::actions`, rule
//!   four). The combo asks `App::action_sets`, which reads actions.json, so
//!   the answer is cached in egui's temp store and re-asked only after a set
//!   verb — this palette is the only thing that moves sets.
//! * **A step is a Scratch BLOCK**, not a text row: a rounded frame filled
//!   with its category's hue, its parameters as live widgets inside the
//!   frame, and its on/off switch showing as the block going ghost-grey.
//!   The colours are the theme's icon hues, on purpose — see `category_hue`.
//!   v1 is the look; sequences are still flat, with no nesting or loops.
//! * **Every edit goes through the deferred `Pending`/`StepOp` values**, not
//!   straight at `app.actions` inside the row loop — a button that removed
//!   its own row mid-iteration would leave the rest of the frame indexing
//!   into a shorter list.
//! * **One save per frame.** `actions_save` writes the whole file, so the
//!   palette raises a `dirty` flag and writes once at the end instead of
//!   once per widget; drag-values commit on release (the layers-palette
//!   idiom), so a slider drag is one write, not one per frame.

use super::icons::Icon;
use super::theme::{self, Theme};
use super::widgets::{icon_btn, icon_btn_tint};
use crate::app::App;
use crate::app::actions::{Action, ActionStep, StepCategory};
use crate::cmd::AppCmd;

/// Payload for a step drag: (action index, step index). Actions do not drag
/// into each other — a step is only meaningful inside its own sequence.
#[derive(Clone, Copy)]
struct StepDrag(usize, usize);

/// Deferred whole-action edits (see the module note).
enum Pending {
    Run(usize),
    Record(usize),
    Delete(usize),
    Duplicate(usize),
}

/// Deferred set verbs. Same reason as [`Pending`], one level up: every one
/// of them rewrites `app.actions` from the file, so none may run while the
/// combo that raised it is still being laid out.
enum SetOp {
    Switch(String),
    New(String),
    Rename(String),
    Delete,
    Run,
}

/// Deferred step-list edits, applied to the open action after its rows.
enum StepOp {
    Insert(usize, ActionStep),
    Delete(usize),
    Duplicate(usize),
    /// (from, drop slot counted before removal)
    Move(usize, usize),
}

const BTN: f32 = 15.0;
// Recording red is `theme::c().rec` now. It stays red in every built-in
// theme — "armed" reads as red everywhere and nowhere else in the app — but
// it is a token rather than a private const, so a theme CAN move it.

// --- the Scratch block's colours ----------------------------------------
//
// Owner, 2026-08-21: auto actions "exactly like Scratch", and "we can use
// colors, we don't need to be scared of colors like clip studio". So a step
// is a rounded block in its category's colour, and the categories draw from
// the SAME seven icon hues the rest of the app uses — a block and the icon
// for the same idea are the same colour, which is what makes it read as one
// system instead of a second palette bolted on.
//
// v1 is the LOOK, not a language: sequences stay FLAT. Nesting, loops and
// conditionals would need a tree model, new drop-slot arithmetic and a new
// on-disk format, and the owner's real CSP auto actions are flat setup
// macros. Control flow is a later round if he asks for it.

/// The theme hue that owns a category.
///
/// * `Create` → `hue_create`, the same green as the new-layer plus badges.
/// * `Name` → `hue_media`, the blue this app uses for text-ish payloads;
///   a rename is a string slot, Scratch's variables family.
/// * `Style` → `hue_layer`, the violet of the layer-kind glyphs — palette
///   colour, border and tone are layer properties, not marks on the page.
/// * `Filter` → `hue_ink`, because a blur rewrites the ink itself. It is
///   the one destructive family and the only warm block in the list.
/// * `Navigate` → `hue_nav`, the neutral view/move hue. Selecting a layer
///   above or below changes nothing, and its block says so by staying grey.
fn category_hue(th: &Theme, cat: StepCategory) -> egui::Color32 {
    match cat {
        StepCategory::Create => th.hue_create,
        StepCategory::Name => th.hue_media,
        StepCategory::Style => th.hue_layer,
        StepCategory::Filter => th.hue_ink,
        StepCategory::Navigate => th.hue_nav,
    }
}

/// A block's body: the hue mixed INTO the panel rather than painted raw, so
/// `text` stays readable on every one of them in every theme (a raw hue at
/// full strength is a highlighter pen, not a UI). `on = false` is the
/// ghosted state that replaces a ticked/unticked checkbox as the thing you
/// actually see: a switched-off step keeps its shape and loses its colour.
fn block_fill(th: &Theme, cat: StepCategory, on: bool) -> egui::Color32 {
    let t = if on { 0.34 } else { 0.08 };
    th.panel.lerp_to_gamma(category_hue(th, cat), t)
}

fn block_stroke(th: &Theme, cat: StepCategory, on: bool) -> egui::Stroke {
    let hue = category_hue(th, cat);
    egui::Stroke::new(1.0, hue.gamma_multiply(if on { 0.85 } else { 0.3 }))
}

fn block_text(th: &Theme, on: bool) -> egui::Color32 {
    if on {
        th.text_strong
    } else {
        th.text_weak.gamma_multiply(0.85)
    }
}

/// The block frame itself — one shape for the step list and the palette, so
/// a block you drag out of the palette looks like the block you dropped.
fn block_frame(th: &Theme, cat: StepCategory, on: bool) -> egui::Frame {
    egui::Frame::new()
        .fill(block_fill(th, cat, on))
        .stroke(block_stroke(th, cat, on))
        .corner_radius(theme::R_PANEL)
        .inner_margin(egui::Margin::symmetric(5, 2))
}

/// The set combo and its verbs — CSP's `Default ▾` at the top of the
/// palette. The sets other than the open one are on disk (`app::actions`,
/// rule four), so the names are asked for once and cached in egui's temp
/// store; every verb here clears the cache on its way out.
fn set_row(ui: &mut egui::Ui, app: &mut App) {
    let cache = egui::Id::new("mn.action.sets");
    let draft_id = egui::Id::new("mn.action.set.draft");
    let (names, current) = match ui.data(|d| d.get_temp::<(Vec<String>, String)>(cache)) {
        Some(v) => v,
        None => {
            let v = app.action_sets();
            ui.data_mut(|d| d.insert_temp(cache, v.clone()));
            v
        }
    };
    let mut op: Option<SetOp> = None;
    ui.horizontal(|ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new("set")
                    .color(theme::c().text_weak)
                    .size(10.0),
            )
            .selectable(false),
        );
        egui::ComboBox::from_id_salt("mn.action.set")
            .selected_text(current.as_str())
            .width(120.0)
            .show_ui(ui, |ui| {
                for n in &names {
                    if ui.selectable_label(*n == current, n).clicked() && *n != current {
                        op = Some(SetOp::Switch(n.clone()));
                    }
                }
                ui.separator();
                // The two naming verbs open a draft row under the combo
                // rather than a dialog — the same inline idiom the action
                // rows rename with.
                if ui.button("New set…").clicked() {
                    ui.data_mut(|d| d.insert_temp(draft_id, (true, String::new())));
                }
                if ui.button("Rename set…").clicked() {
                    ui.data_mut(|d| d.insert_temp(draft_id, (false, current.clone())));
                }
                if ui
                    .add_enabled(names.len() > 1, egui::Button::new("Delete set"))
                    .on_disabled_hover_text("the last set stays — it is where new actions go")
                    .clicked()
                {
                    op = Some(SetOp::Delete);
                }
            });
        if icon_btn_tint(
            ui,
            Icon::Play,
            BTN,
            false,
            !app.actions.is_empty(),
            "run every ticked action in this set, top to bottom — one undo takes it all back",
            Some(theme::c().hue_create),
        )
        .clicked()
        {
            op = Some(SetOp::Run);
        }
    });
    if let Some((new_set, mut text)) = ui.data(|d| d.get_temp::<(bool, String)>(draft_id)) {
        let resp = ui
            .horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(if new_set { "new set" } else { "rename" })
                            .color(theme::c().text_weak)
                            .size(10.0),
                    )
                    .selectable(false),
                );
                ui.text_edit_singleline(&mut text)
            })
            .inner;
        let (enter, escape) = ui.input(|inp| {
            (
                inp.key_pressed(egui::Key::Enter),
                inp.key_pressed(egui::Key::Escape),
            )
        });
        if enter {
            op = Some(if new_set {
                SetOp::New(text)
            } else {
                SetOp::Rename(text)
            });
            ui.data_mut(|d| d.remove::<(bool, String)>(draft_id));
        } else if escape || resp.lost_focus() {
            ui.data_mut(|d| d.remove::<(bool, String)>(draft_id));
        } else {
            resp.request_focus();
            ui.data_mut(|d| d.insert_temp(draft_id, (new_set, text)));
        }
    }
    if let Some(op) = op {
        match op {
            SetOp::Switch(n) => app.action_set_switch(&n),
            SetOp::New(n) => app.action_set_new(n.trim()),
            SetOp::Rename(n) => app.action_set_rename(n.trim()),
            SetOp::Delete => app.action_set_delete(),
            SetOp::Run => app.action_run_set(),
        }
        // The list may have moved under us — ask the file again next frame.
        ui.data_mut(|d| d.remove::<(Vec<String>, String)>(cache));
    }
}

/// The one command bar, at the foot of the palette: the four verbs that act
/// on the SELECTED action, greyed when there is nothing to aim them at. They
/// used to repeat on every row — four icons of chrome beside every name, in
/// a palette that docks at ~200 px (parity audit T4).
fn command_bar(ui: &mut egui::Ui, app: &App) -> Option<Pending> {
    let mut pending: Option<Pending> = None;
    let sel = app.action_selected.filter(|&i| i < app.actions.len());
    // Everything but delete needs an action this build can read: the rest
    // would write over a verbatim-carried copy (`Action::is_readable`).
    let live = sel.is_some_and(|i| app.actions[i].is_readable());
    let recording = matches!((app.action_recording, sel), (Some(r), Some(s)) if r == s);
    ui.horizontal(|ui| {
        // The two transport buttons carry colour, the two edit buttons do
        // not: run is the green "makes something happen" hue, record and
        // stop are the `rec` red that means armed everywhere in this app.
        if icon_btn_tint(
            ui,
            if recording { Icon::Stop } else { Icon::Record },
            BTN,
            recording,
            live,
            if recording {
                "stop recording"
            } else {
                "record layer commands into the selected action"
            },
            Some(theme::c().rec),
        )
        .clicked()
        {
            pending = sel.map(Pending::Record);
        }
        if icon_btn_tint(
            ui,
            Icon::Play,
            BTN,
            false,
            live,
            "run the selected action — one undo takes the whole run back",
            Some(theme::c().hue_create),
        )
        .clicked()
        {
            pending = sel.map(Pending::Run);
        }
        if icon_btn(
            ui,
            Icon::Duplicate,
            BTN,
            false,
            live,
            "duplicate the selected action",
        )
        .clicked()
        {
            pending = sel.map(Pending::Duplicate);
        }
        if icon_btn(
            ui,
            Icon::Trash,
            BTN,
            false,
            sel.is_some(),
            "delete the selected action",
        )
        .clicked()
        {
            pending = sel.map(Pending::Delete);
        }
        if sel.is_none() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new("select an action")
                        .color(theme::c().text_weak)
                        .size(10.0),
                )
                .selectable(false),
            );
        }
    });
    pending
}

pub fn actions_palette(ui: &mut egui::Ui, app: &mut App) {
    let mut dirty = false;
    set_row(ui, app);
    ui.horizontal(|ui| {
        if ui.button("＋ action").clicked() {
            app.actions
                .push(Action::named(format!("Action {}", app.actions.len() + 1)));
            app.action_selected = Some(app.actions.len() - 1);
            app.action_picker = None;
            app.action_step_edit = None;
            dirty = true;
        }
        if app.action_recording.is_some() {
            let (r, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
            super::icons::paint(ui.painter(), r, Icon::Record, theme::c().rec);
            ui.colored_label(theme::c().rec, "recording");
        }
    });
    ui.separator();
    if app.actions.is_empty() {
        ui.weak(
            "no actions in this set yet — ＋ action, then ＋ step (or select it and press ● \
             in the bar below, and do things to layers)",
        );
        return;
    }
    let mut pending: Option<Pending> = None;
    // (name, this build can read it). An action carried verbatim out of a
    // newer version's file gets one greyed row that can be selected and
    // binned, and nothing else — see `Action::is_readable`.
    let rows: Vec<(String, bool)> = app
        .actions
        .iter()
        .map(|a| (a.name.clone(), a.is_readable()))
        .collect();
    // Room kept for the command bar: the scroll area would otherwise eat the
    // whole docked height and push the four verbs off the bottom edge.
    let bar = BTN + 2.0 * ui.spacing().item_spacing.y + 6.0;
    let room = (ui.available_height() - bar).max(64.0);
    egui::ScrollArea::vertical()
        .max_height(room)
        .show(ui, |ui| {
            for (i, (name, readable)) in rows.iter().enumerate() {
                let readable = *readable;
                let selected = app.action_selected == Some(i);
                let recording = app.action_recording == Some(i);

                if !readable {
                    ui.horizontal(|ui| {
                        // No tick: an action this build cannot read is not going
                        // to be run by a set either. The space keeps its name
                        // aligned with the rows above and below it.
                        ui.add_space(BTN + 4.0);
                        let r = ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(format!("{name}  ·  newer version"))
                                        .color(theme::c().text_weak)
                                        .italics(),
                                )
                                .selected(selected),
                            )
                            .on_hover_text(
                                "this action uses a step this build does not know. It is kept \
                             exactly as it was found and written back untouched — open the \
                             file in a newer MangaNakama to edit it, or select it and use \
                             the bin below to drop it.",
                            );
                        if r.clicked() {
                            app.action_selected = if selected { None } else { Some(i) };
                            app.action_picker = None;
                            app.action_step_edit = None;
                        }
                    });
                    continue;
                }

                // Inline rename replaces the row, the layer-palette idiom.
                if matches!(&app.action_renaming, Some((ri, _)) if *ri == i) {
                    let Some((_, text)) = &mut app.action_renaming else {
                        unreachable!()
                    };
                    let resp = ui.text_edit_singleline(text);
                    let done =
                        resp.lost_focus() || ui.input(|inp| inp.key_pressed(egui::Key::Enter));
                    if done {
                        let (_, text) = app.action_renaming.take().unwrap();
                        if !text.trim().is_empty() {
                            app.actions[i].name = text.trim().to_owned();
                            dirty = true;
                        }
                    } else {
                        resp.request_focus();
                    }
                    continue;
                }

                ui.horizontal(|ui| {
                    // CSP's first per-action checkbox, and the only per-row
                    // control left: is this action IN when the set runs (the ▶
                    // beside the set combo)? CSP's second checkbox — show the
                    // step's settings dialog while replaying — has nothing to
                    // switch here: no step kind opens a dialog, so the box would
                    // be a dead control on every row.
                    let mut run = app.actions[i].run;
                    if ui
                        .checkbox(&mut run, "")
                        .on_hover_text("include this action when the whole set runs")
                        .changed()
                    {
                        app.actions[i].run = run;
                        dirty = true;
                    }
                    let r = ui
                        .add(egui::Button::new(name).selected(selected))
                        .on_hover_text(
                            "click: select, and show its steps · double-click: rename\n\
                         the bar at the bottom acts on the selected action",
                        );
                    if r.double_clicked() {
                        app.action_renaming = Some((i, name.clone()));
                    } else if r.clicked() {
                        app.action_selected = if selected { None } else { Some(i) };
                        app.action_picker = None;
                        app.action_step_edit = None;
                    }
                    if recording {
                        let (r, _) =
                            ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                        super::icons::paint(ui.painter(), r, Icon::Record, theme::c().rec);
                    }
                });

                if selected {
                    dirty |= action_steps(ui, app, i, recording);
                }
            }
        });
    ui.separator();
    if let Some(p) = command_bar(ui, app) {
        pending = Some(p);
    }

    match pending {
        Some(Pending::Run(i)) => app.push_cmd(AppCmd::ActionRun(i)),
        Some(Pending::Record(i)) => app.push_cmd(AppCmd::ActionRecordToggle(i)),
        Some(Pending::Duplicate(i)) => {
            // Appended, never inserted: an insert would shift the recording
            // and selection indices of every action below it.
            let copy = app.actions[i].duplicated();
            app.actions.push(copy);
            app.action_selected = Some(app.actions.len() - 1);
            app.action_step_edit = None;
            app.action_picker = None;
            dirty = true;
        }
        Some(Pending::Delete(i)) => {
            if app.action_recording == Some(i) {
                app.action_recording = None;
            }
            app.actions.remove(i);
            app.action_selected = match app.action_selected {
                Some(s) if s == i => None,
                Some(s) if s > i => Some(s - 1),
                other => other,
            };
            if let Some(r) = app.action_recording
                && r > i
            {
                app.action_recording = Some(r - 1);
            }
            app.action_step_edit = None;
            app.action_picker = None;
            dirty = true;
        }
        None => {}
    }
    if dirty {
        app.actions_save();
    }
}

/// The open action's step list: rows, the "＋ step" picker, and the inline
/// parameter editors. Returns whether anything needs saving.
fn action_steps(ui: &mut egui::Ui, app: &mut App, i: usize, recording: bool) -> bool {
    let mut dirty = false;
    let mut op: Option<StepOp> = None;
    let n = app.actions[i].steps.len();
    // The widget the open picker hangs off: a row's ＋ if that row aimed it,
    // otherwise the ＋ step button at the bottom (filled in below).
    let mut picker_anchor: Option<egui::Response> = None;

    let th = theme::c();
    for si in 0..n {
        let row = ui
            .horizontal(|ui| {
                ui.add_space(8.0);
                let on = app.actions[i].steps[si].on;
                let cat = app.actions[i].steps[si].step.category();
                // The three list verbs keep a fixed column at the right
                // edge and the block takes what is left. A docked palette
                // is ~200 px wide and a block with parameter slots in it is
                // wider than that: laid out left-to-right the verbs were
                // pushed off the edge with nothing to say they existed
                // (caught in `--shot-dock` at the shipped width), and
                // letting the whole row wrap put a ragged half-empty line
                // under most steps. Reserved space + a block that wraps
                // INSIDE its own frame keeps the column tidy at any width.
                let verbs = 3.0 * (BTN + ui.spacing().item_spacing.x) + 2.0;
                let room = (ui.available_width() - verbs).max(90.0);
                // The block. Everything that IS the step lives inside the
                // rounded frame — grip, switch, label, parameter slots —
                // and only the list verbs sit outside it, so the blocks
                // themselves stay clean.
                ui.allocate_ui_with_layout(
                    egui::vec2(room, 0.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        block_frame(&th, cat, on).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            // Wrapped INSIDE the frame: a block too wide for the
                            // palette grows a second line of its own rather than
                            // running out past the edge.
                            ui.horizontal_wrapped(|ui| {
                                // Drag handle. Only the grip drags, so the switch and
                                // the parameter widgets keep their plain clicks.
                                let (gr, gresp) = ui
                                    .allocate_exact_size(egui::vec2(8.0, BTN), egui::Sense::drag());
                                super::icons::paint(
                                    ui.painter(),
                                    gr,
                                    Icon::Grip,
                                    if gresp.hovered() || gresp.dragged() {
                                        th.text_strong
                                    } else {
                                        block_text(&th, on).gamma_multiply(0.7)
                                    },
                                );
                                if gresp.drag_started() {
                                    egui::DragAndDrop::set_payload(ui.ctx(), StepDrag(i, si));
                                }
                                let step_row = &mut app.actions[i].steps[si];
                                if ui
                                    .checkbox(&mut step_row.on, "")
                                    .on_hover_text("run this step (off = the block greys out)")
                                    .changed()
                                {
                                    dirty = true;
                                }
                                let txt = block_text(&th, on);
                                if step_row.step.inline_params() {
                                    // Scratch shape: the words, then the live slots.
                                    ui.label(
                                        egui::RichText::new(step_row.step.block_label()).color(txt),
                                    );
                                    dirty |= step_inline(ui, &mut step_row.step);
                                } else if step_row.step.has_params() {
                                    // Too wide to sit in the block (ToneParams): the
                                    // block is a button that opens the framed editor
                                    // underneath it, which is what it always was.
                                    let open = app.action_step_edit == Some((i, si));
                                    let label = step_row.step.label();
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new(label).color(txt),
                                            )
                                            .selected(open),
                                        )
                                        .on_hover_text("click to edit this step")
                                        .clicked()
                                    {
                                        app.action_step_edit =
                                            if open { None } else { Some((i, si)) };
                                    }
                                } else {
                                    ui.label(
                                        egui::RichText::new(step_row.step.block_label()).color(txt),
                                    );
                                }
                            });
                        });
                    },
                );
                let plus = icon_btn(ui, Icon::Plus, BTN, false, true, "insert a step here");
                if plus.clicked() {
                    app.action_picker = Some((i, si));
                }
                if app.action_picker == Some((i, si)) {
                    picker_anchor = Some(plus);
                }
                if icon_btn(ui, Icon::Duplicate, BTN, false, true, "duplicate step").clicked() {
                    op = Some(StepOp::Duplicate(si));
                }
                if icon_btn(ui, Icon::Trash, BTN, false, true, "remove step").clicked() {
                    op = Some(StepOp::Delete(si));
                }
            })
            .response;

        // Drop target: the whole row. Above its middle = the gap before it,
        // below = the gap after — the layers-palette convention, and the
        // one `Action::move_step` is written against.
        if let Some(d) = row.dnd_hover_payload::<StepDrag>()
            && d.0 == i
        {
            let above = ui
                .ctx()
                .pointer_interact_pos()
                .is_some_and(|p| p.y < row.rect.center().y);
            let slot = if above { si } else { si + 1 };
            let y = if above {
                row.rect.top()
            } else {
                row.rect.bottom()
            };
            ui.painter().hline(
                row.rect.x_range(),
                y,
                egui::Stroke::new(2.0, theme::c().accent),
            );
            if row.dnd_release_payload::<StepDrag>().is_some() {
                op = Some(StepOp::Move(d.1, slot));
            }
        }

        // The inline parameter editor, indented under its row.
        if app.action_step_edit == Some((i, si)) {
            ui.horizontal(|ui| {
                ui.add_space(26.0);
                egui::Frame::new()
                    .fill(theme::c().header)
                    .inner_margin(egui::Margin::same(4))
                    .corner_radius(theme::R_CTRL)
                    .show(ui, |ui| {
                        dirty |= step_editor(ui, &mut app.actions[i].steps[si].step);
                    });
            });
        }
    }

    if n == 0 {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.weak(if recording {
                "recording — layer commands land here"
            } else {
                "empty — ＋ step, or press ● below and do things to layers"
            });
        });
    }

    // Append control + the picker it opens. The picker also opens from a
    // row's ＋, which aims it at that row's slot instead of the end.
    let add = ui
        .horizontal(|ui| {
            ui.add_space(14.0);
            let r = ui.small_button("＋ step");
            if r.clicked() {
                app.action_picker = match app.action_picker {
                    Some((a, s)) if a == i && s == n => None,
                    _ => Some((i, n)),
                };
            }
            r
        })
        .inner;
    // The picker is a popup on its own layer, not a frame in the row flow:
    // laid out inline it inherited the enclosing HORIZONTAL layout, so the
    // entries ran sideways over each other and off the palette edge. A popup
    // stacks them, flips itself to stay on screen, and closes on Escape or a
    // click outside — the menu behaviour the rest of the app has.
    if let Some((pi, slot)) = app.action_picker
        && pi == i
    {
        let mut picked: Option<ActionStep> = None;
        let mut open = true;
        // The search text lives in egui's temp store, not in `App`: it is
        // scratch for one open menu, and a half-typed query is not state
        // worth persisting or threading through the app struct. It resets
        // whenever the picker is aimed somewhere new.
        let qid = egui::Id::new("mn.action.picker.query");
        let owner_id = egui::Id::new("mn.action.picker.owner");
        let mut q: String = ui.data(|d| d.get_temp(qid)).unwrap_or_default();
        let fresh = ui.data(|d| d.get_temp::<(usize, usize)>(owner_id)) != Some((pi, slot));
        if fresh {
            q.clear();
        }
        egui::Popup::from_response(picker_anchor.as_ref().unwrap_or(&add))
            .open_bool(&mut open)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .layout(egui::Layout::top_down_justified(egui::Align::Min))
            .width(215.0)
            .show(|ui| {
                ui.label(
                    egui::RichText::new(if slot >= n {
                        "add step at the end".to_owned()
                    } else {
                        format!("insert step at {}", slot + 1)
                    })
                    .color(theme::c().text_weak)
                    .size(10.0),
                );
                let search = ui.add(
                    egui::TextEdit::singleline(&mut q)
                        .hint_text("search steps")
                        .desired_width(f32::INFINITY),
                );
                // Focus once, on open: requesting it every frame would fight
                // the mouse for the clicks in the list below.
                if fresh {
                    search.request_focus();
                }
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        let th = theme::c();
                        let mut hits = 0usize;
                        for cat in StepCategory::ALL {
                            // Matching on the category name too, so "style"
                            // lists the whole family (quick.rs's ladder —
                            // one ranking for every picker in the app).
                            let kinds: Vec<ActionStep> = ActionStep::kinds()
                                .into_iter()
                                .filter(|k| k.category() == cat)
                                .filter(|k| {
                                    super::quick::text_score(
                                        k.kind_label(),
                                        cat.label(),
                                        &q.trim().to_lowercase(),
                                    )
                                    .is_some()
                                })
                                .collect();
                            if kinds.is_empty() {
                                continue;
                            }
                            hits += kinds.len();
                            category_caption(ui, &th, cat);
                            for kind in kinds {
                                if palette_block(ui, &th, &kind).clicked() {
                                    picked = Some(kind);
                                }
                            }
                            ui.add_space(3.0);
                        }
                        if hits == 0 {
                            ui.weak("no step matches that");
                        }
                    });
            });
        ui.data_mut(|d| {
            d.insert_temp(qid, q);
            d.insert_temp(owner_id, (pi, slot));
        });
        if !open {
            app.action_picker = None;
        }
        if let Some(step) = picked {
            // An inline pick lands with its slots already on screen, so
            // there is nothing to open. Only the framed fallback (tone)
            // still needs its editor unfolded — "Screentone…" is useless
            // until it says which pattern.
            let framed = step.has_params() && !step.inline_params();
            op = Some(StepOp::Insert(slot, step));
            app.action_picker = None;
            app.action_step_edit = framed.then_some((i, slot.min(n)));
        }
    }

    match op {
        Some(StepOp::Insert(at, step)) => {
            app.actions[i].insert_step(at, step);
            dirty = true;
        }
        Some(StepOp::Delete(at)) => {
            app.actions[i].remove_step(at);
            app.action_step_edit = None;
            dirty = true;
        }
        Some(StepOp::Duplicate(at)) => {
            app.actions[i].duplicate_step(at);
            app.action_step_edit = None;
            dirty = true;
        }
        Some(StepOp::Move(from, to)) => {
            dirty |= app.actions[i].move_step(from, to);
            app.action_step_edit = None;
        }
        None => {}
    }
    dirty
}

/// A palette section head: the category's colour chip, then its name.
fn category_caption(ui: &mut egui::Ui, th: &Theme, cat: StepCategory) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(6.0, 10.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(r, theme::R_CTRL, category_hue(th, cat));
        ui.label(
            egui::RichText::new(cat.label())
                .color(th.text_weak)
                .size(10.0),
        );
    });
}

/// One block in the step palette: the same shape and colour it will have
/// once it is in the list, so picking is "drag this thing there" even when
/// the click does the moving. The hover ring is painted OVER the block
/// rather than swapping its fill — immediate mode does not know the block is
/// hovered until after it has been laid out, and a fill that lags one frame
/// behind the pointer flickers.
fn palette_block(ui: &mut egui::Ui, th: &Theme, kind: &ActionStep) -> egui::Response {
    let cat = kind.category();
    let inner = block_frame(th, cat, true)
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(kind.kind_label()).color(block_text(th, true)),
                )
                .selectable(false),
            );
        })
        .response;
    let r = inner.interact(egui::Sense::click());
    if r.hovered() {
        ui.painter().rect_stroke(
            r.rect,
            theme::R_PANEL,
            egui::Stroke::new(1.5, th.accent),
            egui::StrokeKind::Inside,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    r
}

/// The parameter slots that live INSIDE a block, on its one line — the
/// thing that makes a block read as a sentence ("rename to [SFX]") the way
/// a Scratch block does, instead of as a row of text with a dialog behind
/// it. Wide editors fall back to [`step_editor`] under the block.
fn step_inline(ui: &mut egui::Ui, step: &mut ActionStep) -> bool {
    let mut dirty = false;
    match step {
        ActionStep::Rename(name) => {
            let r = ui.add(egui::TextEdit::singleline(name).desired_width(74.0));
            // Text saves on release of focus, not per keystroke.
            if r.lost_focus() {
                dirty = true;
            }
        }
        ActionStep::LayerColour(c) | ActionStep::SubColour(c) => {
            dirty |= opt_colour(ui, "", c);
        }
        ActionStep::Edge(e) => {
            let mut on = e.is_some();
            if ui.checkbox(&mut on, "").changed() {
                *e = on.then(mn_core::EdgeParams::default);
                dirty = true;
            }
            if let Some(p) = e.as_mut() {
                let r = ui.add(
                    egui::DragValue::new(&mut p.width_px)
                        .speed(0.1)
                        .range(0.0..=mn_core::edge::WIDTH_MAX)
                        .suffix(" px"),
                );
                dirty |= commit(&r);
                if ui.color_edit_button_srgb(&mut p.colour).changed() {
                    dirty = true;
                }
            }
        }
        ActionStep::GaussianBlur(sigma) => {
            ui.label("σ");
            let r = ui.add(egui::DragValue::new(sigma).speed(0.1).range(0.1..=64.0));
            dirty |= commit(&r);
        }
        // `inline_params` is the gate: tone keeps the framed editor, and
        // the parameterless kinds are all words and no slots.
        _ => {}
    }
    dirty
}

/// Commit a drag-value edit ONCE, on release — a slider that saved every
/// frame would rewrite actions.json sixty times a second (layers.rs idiom).
fn commit(r: &egui::Response) -> bool {
    r.drag_stopped() || (r.changed() && !r.dragged())
}

/// An optional palette colour: a checkbox for on/off plus its swatch.
fn opt_colour(ui: &mut egui::Ui, label: &str, c: &mut Option<[u8; 3]>) -> bool {
    let mut dirty = false;
    let mut on = c.is_some();
    if ui.checkbox(&mut on, label).changed() {
        *c = on.then_some([0x2a, 0x6f, 0xf4]);
        dirty = true;
    }
    if let Some(rgb) = c.as_mut()
        && ui.color_edit_button_srgb(rgb).changed()
    {
        dirty = true;
    }
    dirty
}

/// The framed editor under a block, for parameters too wide to sit in one:
/// today that is `Tone` alone (a pattern combo plus two drag-values is two
/// rows). Anything else that reaches here — a future kind whose
/// `inline_params` says no — falls back to the inline slots rather than
/// rendering an empty box, so the widgets are written once.
fn step_editor(ui: &mut egui::Ui, step: &mut ActionStep) -> bool {
    let mut dirty = false;
    match step {
        ActionStep::Tone(t) => {
            let mut on = t.is_some();
            if ui.checkbox(&mut on, "screentone").changed() {
                *t = on.then(mn_core::ToneParams::default);
                dirty = true;
            }
            if let Some(p) = t.as_mut() {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("mn.action.tone.pattern")
                        .width(84.0)
                        .selected_text(p.pattern.label())
                        .show_ui(ui, |ui| {
                            for pat in mn_core::TonePattern::ALL {
                                if ui.selectable_label(p.pattern == pat, pat.label()).clicked() {
                                    p.pattern = pat;
                                    dirty = true;
                                }
                            }
                        });
                });
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::DragValue::new(&mut p.lpi)
                            .speed(0.5)
                            .range(5.0..=80.0)
                            .suffix(" LPI"),
                    );
                    dirty |= commit(&r);
                    let r = ui.add(
                        egui::DragValue::new(&mut p.angle_deg)
                            .speed(1.0)
                            .range(0.0..=90.0)
                            .suffix("°"),
                    );
                    dirty |= commit(&r);
                });
            }
        }
        _ => {
            ui.horizontal(|ui| dirty |= step_inline(ui, step));
        }
    }
    dirty
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Distance in plain RGB. Crude on purpose: the bar is "would a person
    /// call these two blocks the same colour", not a perceptual model.
    fn apart(a: egui::Color32, b: egui::Color32) -> i32 {
        let d = |x: u8, y: u8| (x as i32 - y as i32).abs();
        d(a.r(), b.r()) + d(a.g(), b.g()) + d(a.b(), b.b())
    }

    /// Every category is coloured, and no two categories collide, in every
    /// built-in theme. A theme that painted Style and Filter the same would
    /// turn the whole point of the Scratch look — read the sequence by
    /// colour, not by reading it — back into a list of grey rows.
    #[test]
    fn every_category_has_its_own_colour_in_every_theme() {
        for (name, th) in theme::BUILT_INS {
            let hues: Vec<(StepCategory, egui::Color32)> = StepCategory::ALL
                .iter()
                .map(|&c| (c, category_hue(th, c)))
                .collect();
            for (i, (ca, a)) in hues.iter().enumerate() {
                for (cb, b) in &hues[i + 1..] {
                    assert!(
                        apart(*a, *b) >= 40,
                        "{name}: {} and {} are the same colour ({a:?} vs {b:?})",
                        ca.label(),
                        cb.label()
                    );
                }
                // The fill is the hue mixed into the panel, so a block must
                // still be visible AGAINST the panel it sits on...
                let fill = block_fill(th, *ca, true);
                assert!(
                    apart(fill, th.panel) >= 12,
                    "{name}: the {} block vanishes into the panel",
                    ca.label()
                );
                // ...and the text on it must not vanish into the block.
                assert!(
                    apart(fill, block_text(th, true)) >= 120,
                    "{name}: the {} block's label is unreadable on it",
                    ca.label()
                );
                // Ghosted is dimmer than live, or the checkbox state is
                // invisible now that the block IS the checkbox.
                let off = block_fill(th, *ca, false);
                assert!(
                    apart(off, th.panel) < apart(fill, th.panel),
                    "{name}: a switched-off {} block does not read as off",
                    ca.label()
                );
            }
        }
    }

    /// Every step kind reaches a colour through its category — the other
    /// half of the model's category tripwire.
    #[test]
    fn every_step_kind_reaches_a_block_colour() {
        for kind in ActionStep::kinds() {
            let hue = category_hue(&theme::DARK, kind.category());
            assert!(
                hue != egui::Color32::TRANSPARENT,
                "{}: no block colour",
                kind.label()
            );
        }
    }

    /// The palette's search is the command palette's ladder, so the step
    /// picker ranks the way Ctrl+K does — including finding a whole family
    /// by its category name.
    #[test]
    fn the_step_search_finds_kinds_by_name_and_by_category() {
        let hit = |q: &str| -> Vec<&'static str> {
            ActionStep::kinds()
                .into_iter()
                .filter(|k| {
                    super::super::quick::text_score(k.kind_label(), k.category().label(), q)
                        .is_some()
                })
                .map(|k| k.kind_label())
                .collect()
        };
        assert_eq!(hit("rename"), vec!["Rename…"]);
        assert!(hit("folder").contains(&"New folder"));
        // The category name lists its whole family.
        let create = hit("create");
        assert_eq!(create.len(), 4, "the Create family: {create:?}");
        assert!(hit("zzzz").is_empty(), "a miss is a miss");
        assert_eq!(hit("").len(), ActionStep::kinds().len(), "empty = all");
    }
}
