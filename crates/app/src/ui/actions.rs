//! The Auto Actions palette (CSP オートアクション): record a run of layer
//! commands into a named action, keep it, replay it as ONE undo press.
//! The model and the three rules that carry it live in `app::actions`.

use crate::app::App;
use crate::app::actions::Action;
use crate::cmd::AppCmd;

pub fn actions_palette(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        if ui.button("＋ action").clicked() {
            app.actions.push(Action {
                name: format!("Action {}", app.actions.len() + 1),
                steps: Vec::new(),
            });
            app.action_selected = Some(app.actions.len() - 1);
            app.actions_save();
        }
        if app.action_recording.is_some() {
            ui.colored_label(egui::Color32::from_rgb(0xe5, 0x4b, 0x4b), "● recording");
        }
    });
    ui.separator();
    if app.actions.is_empty() {
        ui.weak("no actions yet — ＋ action, press ● on it, then do things to layers");
        return;
    }
    let mut run: Option<usize> = None;
    let mut record: Option<usize> = None;
    let mut delete: Option<usize> = None;
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
                        app.actions_save();
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
                }
                if ui
                    .small_button("▶")
                    .on_hover_text("run — one undo takes the whole run back")
                    .clicked()
                {
                    run = Some(i);
                }
                if ui
                    .small_button(if recording { "■" } else { "●" })
                    .on_hover_text(if recording {
                        "stop recording"
                    } else {
                        "record layer commands into this action"
                    })
                    .clicked()
                {
                    record = Some(i);
                }
                if ui.small_button("✕").on_hover_text("delete action").clicked() {
                    delete = Some(i);
                }
            });

            // The open action's steps, with CSP's per-step checkbox — a
            // stored sequence can run with parts switched off.
            if selected {
                let mut step_delete: Option<usize> = None;
                let mut changed = false;
                for si in 0..app.actions[i].steps.len() {
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        let row = &mut app.actions[i].steps[si];
                        let label = row.step.label();
                        if ui.checkbox(&mut row.on, label).changed() {
                            changed = true;
                        }
                        if ui.small_button("✕").on_hover_text("remove step").clicked() {
                            step_delete = Some(si);
                        }
                    });
                }
                if let Some(si) = step_delete {
                    app.actions[i].steps.remove(si);
                    changed = true;
                }
                if changed {
                    app.actions_save();
                }
                if app.actions[i].steps.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.weak(if recording {
                            "recording — layer commands land here"
                        } else {
                            "empty — press ● and do things to layers"
                        });
                    });
                }
            }
        }
    });
    if let Some(i) = run {
        app.push_cmd(AppCmd::ActionRun(i));
    }
    if let Some(i) = record {
        app.push_cmd(AppCmd::ActionRecordToggle(i));
    }
    if let Some(i) = delete {
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
        app.actions_save();
    }
}
