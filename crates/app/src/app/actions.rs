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
//! * **One run = one undo press.** Every step records — structural ones
//!   (new layer / folder) as `UndoGroup::Structure` snapshots since the
//!   2026-08-21 structural-undo round — so the run bundles whatever it
//!   pushed into ONE `Compound` via `wrap_recent`, and the user's earlier
//!   history always survives.

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

    /// One of every kind, with the defaults a hand-added step starts from —
    /// the "＋ step" picker's menu, in palette order. Adding a variant to
    /// `ActionStep` without adding it here hides it from the editor, which
    /// is what the `every_kind_is_offered_by_the_picker` test guards.
    pub fn kinds() -> Vec<ActionStep> {
        vec![
            ActionStep::NewRasterLayer,
            ActionStep::NewVectorLayer,
            ActionStep::NewFolder,
            ActionStep::NewFrameFolder,
            ActionStep::Rename("Layer".into()),
            ActionStep::LayerColour(Some([0x2a, 0x6f, 0xf4])),
            ActionStep::SubColour(Some([0xf4, 0x6f, 0x2a])),
            ActionStep::Edge(Some(mn_core::EdgeParams::default())),
            ActionStep::Tone(Some(mn_core::ToneParams::default())),
            ActionStep::SelectAbove,
            ActionStep::SelectBelow,
            ActionStep::GaussianBlur(4.0),
        ]
    }

    /// Does this step carry anything to edit? Parameterless steps (new
    /// layer, select above/below) get no inline editor, so the palette
    /// doesn't offer a click that opens an empty box.
    pub fn has_params(&self) -> bool {
        matches!(
            self,
            ActionStep::Rename(_)
                | ActionStep::LayerColour(_)
                | ActionStep::SubColour(_)
                | ActionStep::Edge(_)
                | ActionStep::Tone(_)
                | ActionStep::GaussianBlur(_)
        )
    }

    /// The picker's menu label: the label of the step at its defaults, minus
    /// the parameter readout ("Rename…" not "Rename to \"Layer\"").
    pub fn kind_label(&self) -> &'static str {
        match self {
            ActionStep::NewRasterLayer => "New raster layer",
            ActionStep::NewVectorLayer => "New vector layer",
            ActionStep::NewFolder => "New folder",
            ActionStep::NewFrameFolder => "New frame folder",
            ActionStep::Rename(_) => "Rename…",
            ActionStep::LayerColour(_) => "Layer colour…",
            ActionStep::SubColour(_) => "Sub colour…",
            ActionStep::Edge(_) => "Border effect…",
            ActionStep::Tone(_) => "Screentone…",
            ActionStep::SelectAbove => "Select layer above",
            ActionStep::SelectBelow => "Select layer below",
            ActionStep::GaussianBlur(_) => "Gaussian blur…",
        }
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

/// The step-list editing verbs. Pure `Vec` moves, no document contact — the
/// palette (`ui::actions`) is the only caller, and every one of them is
/// followed by `actions_save`. Slots are clamped rather than panicking: the
/// UI computes them from pointer positions and a drag that lands one row
/// past the end must nudge the step to the end, not take the app down.
impl Action {
    /// Insert `step` at slot `at` (`steps.len()` = append), switched on.
    pub fn insert_step(&mut self, at: usize, step: ActionStep) {
        let at = at.min(self.steps.len());
        self.steps.insert(at, StepRow { step, on: true });
    }

    pub fn remove_step(&mut self, at: usize) {
        if at < self.steps.len() {
            self.steps.remove(at);
        }
    }

    /// Copy step `at` in directly below itself — the row the user just
    /// tuned is the row they want two of.
    pub fn duplicate_step(&mut self, at: usize) {
        if let Some(row) = self.steps.get(at).cloned() {
            self.steps.insert(at + 1, row);
        }
    }

    /// Move step `from` into gap `to`, where gaps are counted BEFORE the
    /// removal — the layers-palette drop-slot convention, so the drop line
    /// the user saw is where the step lands. Returns whether anything moved.
    pub fn move_step(&mut self, from: usize, to: usize) -> bool {
        if from >= self.steps.len() || to == from || to == from + 1 {
            return false;
        }
        let row = self.steps.remove(from);
        let to = if to > from { to - 1 } else { to };
        self.steps.insert(to.min(self.steps.len()), row);
        true
    }

    /// A copy under its own name, for the palette's duplicate button.
    pub fn duplicated(&self) -> Action {
        Action {
            name: format!("{} copy", self.name),
            steps: self.steps.clone(),
        }
    }
}

/// User-global storage, its own file on purpose: `ui.txt` is the file the
/// manual tells people to delete to fix a wrecked dock layout (and the
/// `--warp` harness deletes it), and authored actions must survive both.
fn actions_path() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("actions.json"))
}

/// Starter actions for a fresh install (no actions.json yet) — the owner's
/// own CSP macros, translated (TOP-15 #12: "we can just have a default auto
/// action for this"). A step the recipe only sometimes wants ships with its
/// checkbox OFF; deleting or rewriting them is fine, they never come back
/// once the file exists.
pub fn default_actions() -> Vec<Action> {
    let on = |step: ActionStep| StepRow { step, on: true };
    let off = |step: ActionStep| StepRow { step, on: false };
    vec![
        Action {
            name: "Create SFX layer".into(),
            steps: vec![
                on(ActionStep::NewRasterLayer),
                on(ActionStep::Rename("SFX".into())),
                // White 3 px — the legible-over-art edge.
                on(ActionStep::Edge(Some(mn_core::EdgeParams::default()))),
                off(ActionStep::LayerColour(Some([0x2a, 0x6f, 0xf4]))),
            ],
        },
        Action {
            name: "Create tone layer".into(),
            steps: vec![
                on(ActionStep::NewRasterLayer),
                on(ActionStep::Rename("Tone".into())),
                on(ActionStep::Tone(Some(mn_core::ToneParams::default()))),
            ],
        },
        Action {
            name: "Create blue rough layer".into(),
            steps: vec![
                on(ActionStep::NewRasterLayer),
                on(ActionStep::Rename("Rough".into())),
                on(ActionStep::LayerColour(Some([0x2a, 0x6f, 0xf4]))),
            ],
        },
    ]
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
            // First run: seed the palette with the starter macros so the
            // tab demonstrates itself. Not saved until something changes,
            // so a user who deletes actions.json to reset gets them back.
            if self.actions.is_empty() {
                self.actions = default_actions();
            }
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
        let ops_before = self.doc.op_count();
        // The recorder must not eat the replay (recording while running an
        // existing action is how CSP composes them — v1 keeps it simple and
        // records nothing during a run).
        self.action_running = true;
        for s in &steps {
            let cmd = s.lower(self.doc.active);
            cmd::dispatch(self, cmd);
        }
        self.action_running = false;
        // Every step records now — structural ones as Structure snapshots
        // (2026-08-21) — so ONE bundling path: wrap what the run pushed
        // into a single press, keeping the user's earlier history. Counted
        // by the ops tally, not stack depth: the depth stops moving when
        // the cap trims the oldest entries mid-run. Newest-first member
        // order is the undo order (Compound swaps members in sequence).
        let pushed = (self.doc.op_count().saturating_sub(ops_before)) as usize;
        self.doc.wrap_recent(&name, pushed.min(self.doc.undo_len()));
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

    /// Step names, for readable assertions on the editing verbs.
    fn names(a: &Action) -> Vec<String> {
        a.steps.iter().map(|r| r.step.label()).collect()
    }

    fn three() -> Action {
        action(
            vec![
                (ActionStep::NewRasterLayer, true),
                (ActionStep::Rename("A".into()), true),
                (ActionStep::SelectBelow, false),
            ],
            "Edit me",
        )
    }

    #[test]
    fn insert_puts_the_step_at_the_slot_and_switches_it_on() {
        let mut a = three();
        a.insert_step(1, ActionStep::NewFolder);
        assert_eq!(names(&a)[1], "New folder");
        assert_eq!(a.steps.len(), 4);
        assert!(a.steps[1].on, "a hand-added step starts enabled");
        // Past the end appends instead of panicking.
        a.insert_step(99, ActionStep::SelectAbove);
        assert_eq!(names(&a).last().unwrap(), "Select layer above");
        assert_eq!(a.steps.len(), 5);
    }

    #[test]
    fn remove_and_duplicate_hit_the_right_row() {
        let mut a = three();
        a.duplicate_step(1);
        assert_eq!(
            names(&a),
            vec![
                "New raster layer",
                "Rename to \"A\"",
                "Rename to \"A\"",
                "Select layer below"
            ],
            "the copy sits directly below its original"
        );
        a.remove_step(0);
        assert_eq!(a.steps.len(), 3);
        assert_eq!(names(&a)[0], "Rename to \"A\"");
        // Out-of-range verbs are no-ops, not panics.
        a.remove_step(9);
        a.duplicate_step(9);
        assert_eq!(a.steps.len(), 3);
    }

    #[test]
    fn duplicate_step_carries_the_checkbox() {
        let mut a = three();
        a.duplicate_step(2); // the OFF row
        assert!(!a.steps[3].on, "a disabled step duplicates disabled");
    }

    /// Drop slots are counted before the removal (the layers-palette
    /// convention), so the line the user saw is where the step lands.
    #[test]
    fn move_step_uses_pre_removal_drop_slots() {
        let mut a = three();
        assert!(a.move_step(0, 3), "first row to the end");
        assert_eq!(
            names(&a),
            vec!["Rename to \"A\"", "Select layer below", "New raster layer"]
        );
        assert!(a.move_step(2, 0), "last row to the top");
        assert_eq!(
            names(&a),
            vec!["New raster layer", "Rename to \"A\"", "Select layer below"]
        );
        // Both no-op slots: onto itself, and into the gap just below itself.
        assert!(!a.move_step(1, 1));
        assert!(!a.move_step(1, 2));
        assert!(!a.move_step(9, 0), "out-of-range source moves nothing");
        assert_eq!(names(&a), names(&three()), "no-ops left the order alone");
    }

    #[test]
    fn duplicated_action_copies_the_steps_under_a_new_name() {
        let a = three();
        let b = a.duplicated();
        assert_eq!(b.name, "Edit me copy");
        assert_eq!(b.steps, a.steps);
    }

    /// The picker offers EVERY variant: a step kind missing from `kinds()`
    /// is unreachable by hand, which is the whole point of the editor.
    #[test]
    fn every_kind_is_offered_by_the_picker() {
        let kinds = ActionStep::kinds();
        // One entry per variant, compared by kind label so the parameter
        // defaults are free to change.
        let mut labels: Vec<&str> = kinds.iter().map(|k| k.kind_label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), kinds.len(), "no kind listed twice");
        assert_eq!(kinds.len(), 12, "one entry per ActionStep variant");
        // Parameterized kinds carry usable defaults (an editor opens on
        // them; a `None` default would open on nothing).
        for k in &kinds {
            match k {
                ActionStep::Rename(n) => assert!(!n.is_empty()),
                ActionStep::LayerColour(c) | ActionStep::SubColour(c) => assert!(c.is_some()),
                ActionStep::Edge(e) => assert!(e.is_some()),
                ActionStep::Tone(t) => assert!(t.is_some()),
                ActionStep::GaussianBlur(s) => assert!(*s > 0.0),
                _ => assert!(!k.has_params(), "{}: unlisted parameters", k.label()),
            }
        }
    }

    /// The starter macros a fresh install ships with: every one runnable
    /// as-is (a name, at least one ENABLED step) and every one starting
    /// from a new layer so replay never scribbles on existing art.
    #[test]
    fn default_actions_are_runnable_and_start_on_a_new_layer() {
        let defaults = super::default_actions();
        assert!(!defaults.is_empty());
        for a in &defaults {
            assert!(!a.name.trim().is_empty());
            assert!(a.steps.iter().any(|r| r.on), "{}: nothing enabled", a.name);
            assert_eq!(
                a.steps.first().map(|r| &r.step),
                Some(&ActionStep::NewRasterLayer),
                "{}: must begin on its own fresh layer",
                a.name
            );
            for r in &a.steps {
                if let ActionStep::Rename(n) = &r.step {
                    assert!(!n.trim().is_empty(), "{}: empty rename", a.name);
                }
            }
        }
    }
}
