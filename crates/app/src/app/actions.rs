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

/// Which family a step belongs to. Purely DERIVED from the variant and
/// never serialized — `actions.json` is unchanged by this type existing, so
/// re-categorising a step is a UI decision and not a file migration.
///
/// It buys two things: the Scratch-style block colour (each category owns
/// one of the theme's seven icon hues, so a block and the icon for the same
/// idea are the same colour), and the step palette's sections.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum StepCategory {
    /// Makes something that was not there: raster/vector layers, folders,
    /// frame folders.
    Create,
    /// Says what the layer is called.
    Name,
    /// How the layer *shows* without its pixels changing: palette colour,
    /// sub colour, border effect, screentone.
    Style,
    /// Rewrites pixels — the one family that touches the art itself.
    Filter,
    /// Moves the active layer. Changes nothing on the page.
    Navigate,
}

impl StepCategory {
    /// Palette order. `Create` first because every useful macro starts by
    /// making the layer it is about to set up.
    pub const ALL: [StepCategory; 5] = [
        StepCategory::Create,
        StepCategory::Name,
        StepCategory::Style,
        StepCategory::Filter,
        StepCategory::Navigate,
    ];

    /// The section caption in the step palette.
    pub fn label(self) -> &'static str {
        match self {
            StepCategory::Create => "Create",
            StepCategory::Name => "Name",
            StepCategory::Style => "Style",
            StepCategory::Filter => "Filter",
            StepCategory::Navigate => "Navigate",
        }
    }
}

impl ActionStep {
    /// This step's block colour family. See [`StepCategory`] — derived, so
    /// adding a variant without adding it here will not compile.
    pub fn category(&self) -> StepCategory {
        match self {
            ActionStep::NewRasterLayer
            | ActionStep::NewVectorLayer
            | ActionStep::NewFolder
            | ActionStep::NewFrameFolder => StepCategory::Create,
            ActionStep::Rename(_) => StepCategory::Name,
            ActionStep::LayerColour(_)
            | ActionStep::SubColour(_)
            | ActionStep::Edge(_)
            | ActionStep::Tone(_) => StepCategory::Style,
            ActionStep::GaussianBlur(_) => StepCategory::Filter,
            ActionStep::SelectAbove | ActionStep::SelectBelow => StepCategory::Navigate,
        }
    }

    /// The block's leading words: the label MINUS the parameter readout,
    /// because in a Scratch block the parameters follow it as live widgets
    /// ("rename to [SFX]", not "Rename to \"SFX\"" as flat text). Steps with
    /// no parameters read the same either way.
    pub fn block_label(&self) -> &'static str {
        match self {
            ActionStep::NewRasterLayer => "new raster layer",
            ActionStep::NewVectorLayer => "new vector layer",
            ActionStep::NewFolder => "new folder",
            ActionStep::NewFrameFolder => "new frame folder",
            ActionStep::Rename(_) => "rename to",
            ActionStep::LayerColour(_) => "layer colour",
            ActionStep::SubColour(_) => "sub colour",
            ActionStep::Edge(_) => "border effect",
            ActionStep::Tone(_) => "screentone",
            ActionStep::SelectAbove => "select layer above",
            ActionStep::SelectBelow => "select layer below",
            ActionStep::GaussianBlur(_) => "gaussian blur",
        }
    }

    /// Do this step's parameters fit INSIDE the block, on one line? The
    /// Scratch shape wants them there. `Tone` is the exception — a pattern
    /// combo plus two drag-values is two rows wide — so it keeps the older
    /// click-to-open framed editor underneath its block.
    pub fn inline_params(&self) -> bool {
        self.has_params() && !matches!(self, ActionStep::Tone(_))
    }

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
    /// An action THIS build could not read — a step kind from a newer
    /// version — kept exactly as it was found on disk so that opening the
    /// file in an older build and saving does not delete it.
    ///
    /// `#[serde(skip)]`, so it is not a field in the format: on save the raw
    /// value REPLACES the whole action (see [`serialize_actions`]). `None`
    /// for every action this build made itself.
    #[serde(skip)]
    pub unknown: Option<Box<serde_json::Value>>,
}

/// The step-list editing verbs. Pure `Vec` moves, no document contact — the
/// palette (`ui::actions`) is the only caller, and every one of them is
/// followed by `actions_save`. Slots are clamped rather than panicking: the
/// UI computes them from pointer positions and a drag that lands one row
/// past the end must nudge the step to the end, not take the app down.
impl Action {
    /// An empty action under `name` — the "＋ action" button's constructor.
    pub fn named(name: impl Into<String>) -> Action {
        Action {
            name: name.into(),
            steps: Vec::new(),
            unknown: None,
        }
    }

    /// Can this build show and run this action? `false` = it came out of a
    /// newer version's file and is being carried, not understood: the
    /// palette greys it and offers nothing but delete, because every other
    /// verb (rename, record, edit a step) would write over the verbatim copy.
    pub fn is_readable(&self) -> bool {
        self.unknown.is_none()
    }

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

    /// A copy under its own name, for the palette's duplicate button. Never
    /// carries `unknown`: the palette does not offer duplicate on an
    /// unreadable action, and a copy of one would be a second verbatim blob
    /// under a name the file does not agree with.
    pub fn duplicated(&self) -> Action {
        Action {
            name: format!("{} copy", self.name),
            steps: self.steps.clone(),
            unknown: None,
        }
    }
}

/// Read the whole file, in TWO stages: the outer array first, then each
/// action on its own.
///
/// One `serde_json::from_str::<Vec<Action>>` used to do it, which meant ONE
/// step kind this build had never heard of failed the entire parse and every
/// action in the file went unread. It looked survivable — the in-memory list
/// was simply left alone — but the next edit called `actions_save`, and that
/// wrote the empty-ish in-memory list over the user's file. Per-action
/// parsing keeps the readable ones and carries the rest verbatim.
///
/// `None` = the text is not even an array (a truncated or hand-mangled
/// file); the caller keeps what it already has rather than clearing.
pub fn parse_actions(text: &str) -> Option<Vec<Action>> {
    let raw: Vec<serde_json::Value> = serde_json::from_str(text).ok()?;
    Some(
        raw.into_iter()
            .map(|v| {
                serde_json::from_value::<Action>(v.clone()).unwrap_or(Action {
                    name: v
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unreadable action")
                        .to_owned(),
                    steps: Vec::new(),
                    unknown: Some(Box::new(v)),
                })
            })
            .collect(),
    )
}

/// The other half of [`parse_actions`]: unreadable actions go back out with
/// their contents intact, in their original slot.
///
/// The array is assembled from each action's OWN pretty rendering rather
/// than by serializing one `Vec<serde_json::Value>`, and that is not a
/// flourish: `serde_json::Value` holds its object keys in a `BTreeMap`, so a
/// trip through it silently re-sorts every field alphabetically — `"on"`
/// before `"step"`, `"colour"` before `"width_px"`. Nothing would break, but
/// every existing actions.json would be rewritten head to toe on the first
/// save. `the_on_disk_format_did_not_move` is the test that caught it.
pub fn serialize_actions(actions: &[Action]) -> Option<String> {
    if actions.is_empty() {
        return Some("[]".to_owned());
    }
    let mut items = Vec::with_capacity(actions.len());
    for a in actions {
        let text = match &a.unknown {
            Some(v) => serde_json::to_string_pretty(v).ok()?,
            None => serde_json::to_string_pretty(a).ok()?,
        };
        // Two spaces on every line: exactly how `to_string_pretty` indents
        // the members of an array.
        items.push(
            text.lines()
                .map(|l| format!("  {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    Some(format!("[\n{}\n]", items.join(",\n")))
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
    const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
    vec![
        Action {
            name: "Create SFX layer".into(),
            unknown: None,
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
            unknown: None,
            steps: vec![
                on(ActionStep::NewRasterLayer),
                on(ActionStep::Rename("Tone".into())),
                on(ActionStep::Tone(Some(mn_core::ToneParams::default()))),
            ],
        },
        Action {
            name: "Create blue rough layer".into(),
            unknown: None,
            steps: vec![
                on(ActionStep::NewRasterLayer),
                on(ActionStep::Rename("Rough".into())),
                on(ActionStep::LayerColour(Some([0x2a, 0x6f, 0xf4]))),
            ],
        },
        // Whatever you draw on this one comes out WHITE, whichever brush
        // colour is loaded — the layer-colour pair maps the black end AND
        // the white end to white (`blend::layer_colour_tint`). That is why
        // both chips are set: a white SUB colour alone is a documented
        // no-op ("an explicit white sub == no sub, everywhere", LP-017), so
        // the main chip is the step doing the work.
        Action {
            name: "Create white cover layer".into(),
            unknown: None,
            steps: vec![
                on(ActionStep::NewRasterLayer),
                on(ActionStep::Rename("White".into())),
                on(ActionStep::LayerColour(Some(WHITE))),
                on(ActionStep::SubColour(Some(WHITE))),
            ],
        },
        // The rough-work bin: a folder to drop drafts into, tinted blue so
        // it reads as not-final in the Layers palette. (CSP's macro also
        // flips the folder's DRAFT flag; there is no draft step kind yet,
        // so the colour carries the meaning until there is one.)
        Action {
            name: "Create draft folder".into(),
            unknown: None,
            steps: vec![
                on(ActionStep::NewFolder),
                on(ActionStep::Rename("Draft".into())),
                on(ActionStep::LayerColour(Some([0x66, 0x9e, 0xd6]))),
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
        // Per-action parsing, NOT one `Vec<Action>`: see `parse_actions`.
        if let Some(list) = parse_actions(&text) {
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
        if let Some(text) = serialize_actions(&self.actions) {
            let _ = std::fs::write(p, text);
        }
    }

    /// Replay `idx`'s enabled steps as ONE undo press. See the module note
    /// for the structural / non-structural split.
    pub fn action_run(&mut self, idx: usize) {
        let Some(action) = self.actions.get(idx) else {
            return;
        };
        if !action.is_readable() {
            self.set_status("that action is from a newer version — it is kept as-is, not run");
            return;
        }
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
        } else if self.actions.get(idx).is_some_and(|a| !a.is_readable()) {
            // Recording into a verbatim-carried action would append steps
            // that the save path then throws away (it writes the raw copy).
            self.set_status("that action is from a newer version — it cannot be recorded into");
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
            unknown: None,
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
            app.doc.layers[li]
                .tile_arc(TileIdx::new(0, 0))
                .unwrap()
                .data()[0],
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
        let steps: Vec<ActionStep> = app.actions[0]
            .steps
            .iter()
            .map(|r| r.step.clone())
            .collect();
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
        // Every kind lands in a category (the Scratch block's colour), and
        // no category is empty — an empty section would print a coloured
        // caption over nothing.
        for cat in StepCategory::ALL {
            assert!(
                kinds.iter().any(|k| k.category() == cat),
                "{}: no step kind in this category",
                cat.label()
            );
        }
        for k in &kinds {
            assert!(
                StepCategory::ALL.contains(&k.category()),
                "{}: category outside ALL — the palette would never show it",
                k.label()
            );
            assert!(
                !k.block_label().is_empty(),
                "{}: no block label to put in the block",
                k.label()
            );
            // Inline parameters are a subset of "has parameters": a step
            // with nothing to edit must not claim an inline editor slot.
            assert!(
                !k.inline_params() || k.has_params(),
                "{}: inline editor for a step with no parameters",
                k.label()
            );
        }
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
    fn default_actions_are_runnable_and_start_by_making_something() {
        let defaults = super::default_actions();
        assert!(!defaults.is_empty());
        for a in &defaults {
            assert!(!a.name.trim().is_empty());
            assert!(a.steps.iter().any(|r| r.on), "{}: nothing enabled", a.name);
            // Every starter macro's first step CREATES its own target
            // (layer or folder), so a replay can never scribble on the art
            // that happens to be selected.
            assert_eq!(
                a.steps.first().map(|r| r.step.category()),
                Some(StepCategory::Create),
                "{}: must begin by making its own layer or folder",
                a.name
            );
            // "Action 4" shipped in a screenshot once (parity audit T4).
            // A starter macro says what it is for.
            assert!(
                !a.name.starts_with("Action "),
                "{}: placeholder name in the shipped set",
                a.name
            );
            for r in &a.steps {
                if let ActionStep::Rename(n) = &r.step {
                    assert!(!n.trim().is_empty(), "{}: empty rename", a.name);
                }
            }
        }
    }

    /// A file written by a NEWER build carries step kinds this one has never
    /// heard of. It used to cost the whole file (see `parse_actions`): the
    /// one `Vec<Action>` parse failed and nothing loaded. The unreadable
    /// action is now carried verbatim and everything around it still works.
    #[test]
    fn one_unreadable_action_no_longer_costs_the_whole_file() {
        let text = r#"[
          {"name":"Good","steps":[{"step":"NewRasterLayer","on":true}]},
          {"name":"From the future","steps":[{"step":{"WarpMesh":{"rows":4}},"on":true}]},
          {"name":"Also good","steps":[{"step":"SelectAbove","on":true}]}
        ]"#;
        // The shape of the bug, pinned: the OLD one-shot parse still fails
        // on this exact text, so this test fails against the old code.
        assert!(
            serde_json::from_str::<Vec<Action>>(text).is_err(),
            "the whole-file parse is supposed to choke here — that is the bug"
        );

        let list = parse_actions(text).expect("the outer array is fine");
        assert_eq!(list.len(), 3, "nothing dropped out of the middle");
        assert!(list[0].is_readable() && list[2].is_readable());
        assert_eq!(list[0].steps.len(), 1);
        assert_eq!(list[2].steps[0].step, ActionStep::SelectAbove);

        let odd = &list[1];
        assert!(!odd.is_readable(), "the future action is flagged, not run");
        assert_eq!(odd.name, "From the future", "it keeps its name for the UI");
        assert!(odd.steps.is_empty(), "no half-read steps to edit or replay");

        // And a save round-trips it: opening the file in this build and
        // pressing anything must not delete the user's work.
        let out = serialize_actions(&list).expect("serializes");
        let again = parse_actions(&out).expect("round-trips");
        assert_eq!(again.len(), 3);
        assert!(
            out.contains("WarpMesh"),
            "the unknown step survived the save"
        );
        assert_eq!(again[1].name, "From the future");
        assert!(!again[1].is_readable());
        assert_eq!(again[0], list[0], "the readable ones are unchanged");
    }

    /// Same file, no unknowns: the bytes must be what the old
    /// `to_string_pretty(&Vec<Action>)` wrote, or every user's actions.json
    /// churns on first save.
    #[test]
    fn the_on_disk_format_did_not_move() {
        let list = default_actions();
        let ours = serialize_actions(&list).unwrap();
        let old = serde_json::to_string_pretty(&list).unwrap();
        assert_eq!(ours, old, "serialize_actions changed the format");
        assert!(
            !ours.contains("unknown"),
            "the carry-field leaked into the format"
        );
        assert_eq!(parse_actions(&ours).unwrap(), list, "round-trip");
        // An empty list is `[]`, not a two-line array of nothing.
        assert_eq!(serialize_actions(&[]).unwrap(), "[]");
    }

    /// Not an array at all (truncated write, hand-editing). The caller keeps
    /// what it has rather than being handed an empty list to save over the
    /// file with.
    #[test]
    fn a_mangled_file_reads_as_nothing_at_all_not_as_zero_actions() {
        assert!(parse_actions("").is_none());
        assert!(parse_actions("[{\"name\":\"half").is_none());
        assert!(parse_actions("{}").is_none());
        assert_eq!(parse_actions("[]").unwrap().len(), 0);
    }

    /// The unreadable ones are carried, not operated on: every verb the
    /// palette hides is also refused by the model.
    #[test]
    fn an_unreadable_action_refuses_to_run_or_record() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        let mut odd = Action::named("From the future");
        odd.unknown = Some(Box::new(serde_json::json!({"name":"From the future"})));
        app.actions.push(odd);
        let n0 = app.doc.layers.len();
        app.action_run(0);
        assert_eq!(app.doc.layers.len(), n0, "nothing ran");
        assert_eq!(app.doc.undo_labels().len(), 0, "and nothing was pushed");
        app.action_record_toggle(0);
        assert_eq!(app.action_recording, None, "the recorder stayed disarmed");
    }
}
