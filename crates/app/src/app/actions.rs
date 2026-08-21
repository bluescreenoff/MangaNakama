//! Recordable action sequences (CSP オートアクション): record a run of
//! layer-management commands, store it, replay it — one palette press,
//! one undo press.
//!
//! Three rules carry the feature:
//!
//! * **Steps are their own narrow enum**, not `AppCmd`. `ActionStep` is the
//!   PERSISTED format (actions.json beside the exe), so it must stay small
//!   and stable — the 240-variant command enum is neither. Every step is
//!   index-free: it acts on whatever layer is active when it replays, which
//!   is also how CSP's auto actions read.
//! * **Replay goes through `cmd::dispatch`**, never straight at the
//!   `Document`. Each command arm carries its own cache doors (evictions,
//!   thumbnail resets, frame renumbering); a replay that bypassed dispatch
//!   would skip exactly those.
//! * **One run = one undo press.** A run containing a structural step
//!   (new layer / folder) has its history cleared by that step anyway, so
//!   the run ends by pushing ONE `UndoGroup::Structure` (the pre-run stack,
//!   `Arc`-cheap). A run of only non-structural steps instead bundles the
//!   groups it pushed into ONE `Compound` and the user's earlier history
//!   survives — clearing it for a tone tweak would be theft.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::app::App;
use crate::cmd::{self, AppCmd};

/// One replayable step. Everything acts on the ACTIVE layer at replay time.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ActionStep {
    NewRasterLayer,
    NewVectorLayer,
    NewFolder,
    NewFrameFolder,
    Rename(String),
    LayerColour(Option<[u8; 3]>),
    SubColour(Option<[u8; 3]>),
    Edge(Option<mn_core::EdgeParams>),
    Tone(Option<mn_core::ToneParams>),
    SelectAbove,
    SelectBelow,
    GaussianBlur(f32),
}

impl ActionStep {
    /// The palette's step label.
    pub fn label(&self) -> String {
        match self {
            ActionStep::NewRasterLayer => "New raster layer".into(),
            ActionStep::NewVectorLayer => "New vector layer".into(),
            ActionStep::NewFolder => "New folder".into(),
            ActionStep::NewFrameFolder => "New frame folder".into(),
            ActionStep::Rename(n) => format!("Rename to \"{n}\""),
            ActionStep::LayerColour(Some(_)) => "Layer colour on".into(),
            ActionStep::LayerColour(None) => "Layer colour off".into(),
            ActionStep::SubColour(Some(_)) => "Sub colour on".into(),
            ActionStep::SubColour(None) => "Sub colour off".into(),
            ActionStep::Edge(Some(e)) => format!("Border effect {: >2.0} px", e.width_px),
            ActionStep::Edge(None) => "Border effect off".into(),
            ActionStep::Tone(Some(_)) => "Screentone on".into(),
            ActionStep::Tone(None) => "Screentone off".into(),
            ActionStep::SelectAbove => "Select layer above".into(),
            ActionStep::SelectBelow => "Select layer below".into(),
            ActionStep::GaussianBlur(s) => format!("Gaussian blur σ {s:.1}"),
        }
    }

    /// Structural steps change the layer stack — they clear the undo
    /// history when replayed, which decides how the run wraps its undo.
    fn is_structural(&self) -> bool {
        matches!(
            self,
            ActionStep::NewRasterLayer
                | ActionStep::NewVectorLayer
                | ActionStep::NewFolder
                | ActionStep::NewFrameFolder
        )
    }

    /// Lower to the command it replays as. `active` is read at replay time,
    /// per step — a New-layer step moves it, and the next step follows.
    fn lower(&self, active: usize) -> AppCmd {
        match self {
            ActionStep::NewRasterLayer => AppCmd::AddLayer,
            ActionStep::NewVectorLayer => AppCmd::AddVectorLayer,
            ActionStep::NewFolder => AppCmd::AddFolder,
            ActionStep::NewFrameFolder => AppCmd::NewFrameLayer,
            ActionStep::Rename(n) => AppCmd::RenameLayer(active, n.clone()),
            ActionStep::LayerColour(c) => AppCmd::SetLayerColour(active, *c),
            ActionStep::SubColour(c) => AppCmd::SetLayerSubColour(active, *c),
            ActionStep::Edge(e) => AppCmd::SetEdge(active, *e),
            ActionStep::Tone(t) => AppCmd::SetTone(*t),
            ActionStep::SelectAbove => AppCmd::LayerAbove,
            ActionStep::SelectBelow => AppCmd::LayerBelow,
            ActionStep::GaussianBlur(sigma) => {
                AppCmd::FilterApply(mn_core::Filter::Gaussian { sigma: *sigma })
            }
        }
    }

    /// The recorder's half: which commands become a step. Index-carrying
    /// commands record only when aimed at the ACTIVE layer — a step is
    /// index-free, so recording a click on some other row would silently
    /// change its meaning. Returns `None` for everything unrecordable.
    pub fn from_cmd(cmd: &AppCmd, active: usize) -> Option<ActionStep> {
        Some(match cmd {
            AppCmd::AddLayer => ActionStep::NewRasterLayer,
            AppCmd::AddVectorLayer => ActionStep::NewVectorLayer,
            AppCmd::AddFolder => ActionStep::NewFolder,
            AppCmd::NewFrameLayer => ActionStep::NewFrameFolder,
            AppCmd::RenameLayer(i, n) if *i == active => ActionStep::Rename(n.clone()),
            AppCmd::SetLayerColour(i, c) if *i == active => ActionStep::LayerColour(*c),
            AppCmd::SetLayerSubColour(i, c) if *i == active => ActionStep::SubColour(*c),
            AppCmd::SetEdge(i, e) if *i == active => ActionStep::Edge(*e),
            AppCmd::SetTone(t) => ActionStep::Tone(*t),
            AppCmd::LayerAbove => ActionStep::SelectAbove,
            AppCmd::LayerBelow => ActionStep::SelectBelow,
            AppCmd::FilterApply(mn_core::Filter::Gaussian { sigma }) => {
                ActionStep::GaussianBlur(*sigma)
            }
            _ => return None,
        })
    }
}

/// One step row: CSP's per-step checkbox, so a stored sequence can run
/// with parts switched off without editing it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StepRow {
    pub step: ActionStep,
    #[serde(default = "on_default")]
    pub on: bool,
}

fn on_default() -> bool {
    true
}

/// A named, stored sequence.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Action {
    pub name: String,
    pub steps: Vec<StepRow>,
}

/// User-global storage, its own file on purpose: `ui.txt` is the file the
/// manual tells people to delete to fix a wrecked dock layout (and the
/// `--warp` harness deletes it), and authored actions must survive both.
fn actions_path() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("actions.json"))
}

impl App {
    pub fn actions_load(&mut self) {
        // Unit tests share one exe directory and run in parallel: a real
        // file there leaks one test's recording into another test's
        // `App::new` (it did — the suite caught what solo runs could not).
        if cfg!(test) {
            return;
        }
        let Some(p) = actions_path() else { return };
        let Ok(text) = std::fs::read_to_string(p) else {
            return;
        };
        if let Ok(list) = serde_json::from_str::<Vec<Action>>(&text) {
            self.actions = list;
        }
    }

    /// Write-on-change: the file is a few KB and a lost recording is the
    /// kind of loss nobody re-does happily.
    pub fn actions_save(&self) {
        if cfg!(test) {
            return; // see `actions_load`
        }
        let Some(p) = actions_path() else { return };
        if let Ok(text) = serde_json::to_string_pretty(&self.actions) {
            let _ = std::fs::write(p, text);
        }
    }

    /// Replay `idx`'s enabled steps as ONE undo press. See the module note
    /// for the structural / non-structural split.
    pub fn action_run(&mut self, idx: usize) {
        let Some(action) = self.actions.get(idx) else {
            return;
        };
        let name = action.name.clone();
        let steps: Vec<ActionStep> = action
            .steps
            .iter()
            .filter(|r| r.on)
            .map(|r| r.step.clone())
            .collect();
        if steps.is_empty() {
            self.set_status("this action has no enabled steps");
            return;
        }
        let structural = steps.iter().any(ActionStep::is_structural);
        let before = structural.then(|| (self.doc.layers.clone(), self.doc.active));
        let depth_before = self.doc.undo_labels().len();
        // The recorder must not eat the replay (recording while running an
        // existing action is how CSP composes them — v1 keeps it simple and
        // records nothing during a run).
        self.action_running = true;
        for s in &steps {
            let cmd = s.lower(self.doc.active);
            cmd::dispatch(self, cmd);
        }
        self.action_running = false;
        match before {
            Some((layers, active)) => {
                // The structural steps cleared the history mid-run; the
                // snapshot pair (pre-run stack in the group, post-run stack
                // live) supersedes whatever else the run pushed.
                self.doc.push_structure(&name, layers, active);
            }
            None => {
                // Non-structural run: bundle what it pushed, keep the
                // user's earlier history. Newest-first member order is the
                // undo order (Compound swaps members in sequence).
                let n = self.doc.undo_labels().len().saturating_sub(depth_before);
                self.doc.wrap_recent(&name, n);
            }
        }
        self.set_status(format!("\"{name}\" ran — one undo takes it all back"));
        self.mark_dirty();
    }

    /// Arm or disarm recording into `idx`. Recording appends live from the
    /// dispatch tail (`cmd::dispatch`), one step per recordable command.
    pub fn action_record_toggle(&mut self, idx: usize) {
        if self.action_recording == Some(idx) {
            self.action_recording = None;
            let n = self.actions.get(idx).map_or(0, |a| a.steps.len());
            self.set_status(format!("recording stopped — {n} steps"));
            self.actions_save();
        } else if idx < self.actions.len() {
            self.action_recording = Some(idx);
            self.set_status(
                "recording — layer commands land as steps (new layer/folder, rename, \
                 colour, border, tone, blur, select above/below)",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::{AppCmd, dispatch};
    use mn_core::TileIdx;

    fn headless() -> Option<App> {
        let renderer = mn_gpu::Renderer::new_headless(mn_gpu::GpuConfig {
            force_fallback: std::env::var("MN_WARP").is_ok(),
            no_vsync: false,
        })
        .ok()?;
        Some(App::new(renderer, (1280, 860), 1.0))
    }

    fn action(steps: Vec<(ActionStep, bool)>, name: &str) -> Action {
        Action {
            name: name.into(),
            steps: steps
                .into_iter()
                .map(|(step, on)| StepRow { step, on })
                .collect(),
        }
    }

    #[test]
    fn a_structural_action_replays_as_one_undo_press() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        app.actions.push(action(
            vec![
                (ActionStep::NewRasterLayer, true),
                (ActionStep::Rename("SFX 1".into()), true),
                (ActionStep::Edge(Some(mn_core::EdgeParams::default())), true),
                (ActionStep::Rename("skipped".into()), false), // box unticked
            ],
            "Create SFX Layer",
        ));
        let n0 = app.doc.layers.len();
        dispatch(&mut app, AppCmd::ActionRun(0));
        assert_eq!(app.doc.layers.len(), n0 + 1, "the run created its layer");
        let li = app.doc.active;
        assert_eq!(app.doc.layers[li].name, "SFX 1", "renamed — not 'skipped'");
        assert!(app.doc.layers[li].edge.is_some(), "border effect landed");
        assert_eq!(app.doc.undo_labels().len(), 1, "ONE step for the run");
        assert_eq!(app.doc.undo_labels()[0], "Create SFX Layer");
        dispatch(&mut app, AppCmd::Undo);
        assert_eq!(app.doc.layers.len(), n0, "one press took it all back");
        dispatch(&mut app, AppCmd::Redo);
        assert_eq!(app.doc.layers.len(), n0 + 1, "redo re-applies the run");
        assert_eq!(app.doc.layers[app.doc.layers.len() - 1].name, "SFX 1");
    }

    #[test]
    fn a_non_structural_action_keeps_the_earlier_history() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        // An earlier undoable step that must SURVIVE the run.
        let li = app.doc.active;
        app.doc.begin_op();
        app.doc.layers[li].tile_mut(TileIdx::new(0, 0)).data_mut()[0] = 55;
        assert!(app.doc.end_op());
        app.actions.push(action(
            vec![(ActionStep::Edge(Some(mn_core::EdgeParams::default())), true)],
            "Keyline",
        ));
        dispatch(&mut app, AppCmd::ActionRun(0));
        assert!(app.doc.layers[li].edge.is_some());
        assert_eq!(
            app.doc.undo_labels().len(),
            2,
            "the stroke is still undoable under the run"
        );
        assert_eq!(app.doc.undo_labels()[1], "Keyline");
        dispatch(&mut app, AppCmd::Undo);
        assert!(app.doc.layers[li].edge.is_none(), "run undone");
        assert_eq!(
            app.doc.layers[li].tile_arc(TileIdx::new(0, 0)).unwrap().data()[0],
            55,
            "the earlier stroke survives"
        );
    }

    #[test]
    fn recording_taps_only_recordable_commands() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        app.actions.push(action(vec![], "Rec"));
        dispatch(&mut app, AppCmd::ActionRecordToggle(0));
        dispatch(&mut app, AppCmd::AddLayer);
        let li = app.doc.active;
        dispatch(&mut app, AppCmd::RenameLayer(li, "X".into()));
        dispatch(&mut app, AppCmd::Zoom100); // not a layer command
        dispatch(&mut app, AppCmd::ActionRecordToggle(0));
        let steps: Vec<ActionStep> =
            app.actions[0].steps.iter().map(|r| r.step.clone()).collect();
        assert_eq!(
            steps,
            vec![ActionStep::NewRasterLayer, ActionStep::Rename("X".into())],
            "execution order, zoom ignored"
        );
        dispatch(&mut app, AppCmd::AddLayer);
        assert_eq!(app.actions[0].steps.len(), 2, "stopped means stopped");
    }
}
