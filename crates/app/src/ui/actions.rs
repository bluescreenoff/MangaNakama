//! The Auto Actions palette (CSP オートアクション): record a run of layer
//! commands into a named action, keep it, replay it as ONE undo press —
//! and EDIT it, which is the half CSP has and a recorder alone does not
//! (owner 2026-08-21: "auto actions still doesn't work like scratch
//! programming"). Steps can be added by hand from a picker of every kind,
//! retuned in place, dragged into a new order, duplicated and deleted.
//! The model and the three rules that carry it live in `app::actions`.
//!
//! Two shapes worth knowing before editing this file:
//!
//! * **Every edit goes through the deferred `Pending`/`StepOp` values**, not
//!   straight at `app.actions` inside the row loop — a button that removed
//!   its own row mid-iteration would leave the rest of the frame indexing
//!   into a shorter list.
//! * **One save per frame.** `actions_save` writes the whole file, so the
//!   palette raises a `dirty` flag and writes once at the end instead of
//!   once per widget; drag-values commit on release (the layers-palette
//!   idiom), so a slider drag is one write, not one per frame.

use super::icons::Icon;
use super::theme;
use super::widgets::icon_btn;
use crate::app::App;
use crate::app::actions::{Action, ActionStep};
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

pub fn actions_palette(ui: &mut egui::Ui, app: &mut App) {
    let mut dirty = false;
    ui.horizontal(|ui| {
        if ui.button("＋ action").clicked() {
            app.actions.push(Action {
                name: format!("Action {}", app.actions.len() + 1),
                steps: Vec::new(),
            });
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
        ui.weak("no actions yet — ＋ action, then ＋ step (or press ● and do things to layers)");
        return;
    }
    let mut pending: Option<Pending> = None;
    let names: Vec<String> = app.actions.iter().map(|a| a.name.clone()).collect();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, name) in names.iter().enumerate() {
            let selected = app.action_selected == Some(i);
            let recording = app.action_recording == Some(i);

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
                let r = ui
                    .add(egui::Button::new(name).selected(selected))
                    .on_hover_text("click: show steps · double-click: rename");
                if r.double_clicked() {
                    app.action_renaming = Some((i, name.clone()));
                } else if r.clicked() {
                    app.action_selected = if selected { None } else { Some(i) };
                    app.action_picker = None;
                    app.action_step_edit = None;
                }
                if icon_btn(
                    ui,
                    Icon::Play,
                    BTN,
                    false,
                    true,
                    "run — one undo takes the whole run back",
                )
                .clicked()
                {
                    pending = Some(Pending::Run(i));
                }
                if icon_btn(
                    ui,
                    if recording { Icon::Stop } else { Icon::Record },
                    BTN,
                    recording,
                    true,
                    if recording {
                        "stop recording"
                    } else {
                        "record layer commands into this action"
                    },
                )
                .clicked()
                {
                    pending = Some(Pending::Record(i));
                }
                if icon_btn(ui, Icon::Duplicate, BTN, false, true, "duplicate action").clicked() {
                    pending = Some(Pending::Duplicate(i));
                }
                if icon_btn(ui, Icon::Trash, BTN, false, true, "delete action").clicked() {
                    pending = Some(Pending::Delete(i));
                }
            });

            if selected {
                dirty |= action_steps(ui, app, i, recording);
            }
        }
    });

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

    for si in 0..n {
        let row = ui
            .horizontal(|ui| {
                ui.add_space(10.0);
                // Drag handle. Only the grip drags, so the checkbox and the
                // label keep their plain clicks.
                let (gr, gresp) =
                    ui.allocate_exact_size(egui::vec2(8.0, BTN), egui::Sense::drag());
                super::icons::paint(
                    ui.painter(),
                    gr,
                    Icon::Grip,
                    if gresp.hovered() || gresp.dragged() {
                        theme::c().text
                    } else {
                        theme::c().text_weak
                    },
                );
                if gresp.drag_started() {
                    egui::DragAndDrop::set_payload(ui.ctx(), StepDrag(i, si));
                }
                let step_row = &mut app.actions[i].steps[si];
                if ui.checkbox(&mut step_row.on, "").changed() {
                    dirty = true;
                }
                let has_params = step_row.step.has_params();
                let label = step_row.step.label();
                let open = app.action_step_edit == Some((i, si));
                let lr = ui.add(egui::Button::new(label).selected(open).frame(has_params));
                let lr = if has_params {
                    lr.on_hover_text("click to edit this step")
                } else {
                    lr
                };
                if lr.clicked() && has_params {
                    app.action_step_edit = if open { None } else { Some((i, si)) };
                }
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
                "empty — ＋ step, or press ● and do things to layers"
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
        egui::Popup::from_response(picker_anchor.as_ref().unwrap_or(&add))
            .open_bool(&mut open)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .layout(egui::Layout::top_down_justified(egui::Align::Min))
            .width(190.0)
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
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for kind in ActionStep::kinds() {
                            if ui.selectable_label(false, kind.kind_label()).clicked() {
                                picked = Some(kind);
                            }
                        }
                    });
            });
        if !open {
            app.action_picker = None;
        }
        if let Some(step) = picked {
            let params = step.has_params();
            op = Some(StepOp::Insert(slot, step));
            app.action_picker = None;
            // A parameterized pick lands with its editor already open —
            // "Rename…" is useless until it says what to rename to.
            app.action_step_edit = params.then_some((i, slot.min(n)));
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

/// The inline editor for one step's parameters. Same fields the picker's
/// defaults land with — editing a step and adding one are the same widgets.
fn step_editor(ui: &mut egui::Ui, step: &mut ActionStep) -> bool {
    let mut dirty = false;
    match step {
        ActionStep::Rename(name) => {
            ui.horizontal(|ui| {
                ui.label("name");
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(120.0));
                // Text saves on release of focus, not per keystroke.
                if r.lost_focus() {
                    dirty = true;
                }
            });
        }
        ActionStep::LayerColour(c) => {
            ui.horizontal(|ui| dirty |= opt_colour(ui, "layer colour", c));
        }
        ActionStep::SubColour(c) => {
            ui.horizontal(|ui| dirty |= opt_colour(ui, "sub colour", c));
        }
        ActionStep::Edge(e) => {
            ui.horizontal(|ui| {
                let mut on = e.is_some();
                if ui.checkbox(&mut on, "border").changed() {
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
            });
        }
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
        ActionStep::GaussianBlur(sigma) => {
            ui.horizontal(|ui| {
                ui.label("σ");
                let r = ui.add(egui::DragValue::new(sigma).speed(0.1).range(0.1..=64.0));
                dirty |= commit(&r);
            });
        }
        // Parameterless: `has_params` keeps the editor from ever opening.
        _ => {}
    }
    dirty
}
