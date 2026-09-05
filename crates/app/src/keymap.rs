//! User key bindings — `keys.json` beside the exe (workflow-audit #5).
//!
//! The built-in table in `main.rs::shortcut` is a hardcoded match mined
//! from the owner's own CSP set, and it is well chosen — but frozen. A
//! working mangaka's key set is personal and evolves; CSP ships a whole
//! Shortcut Settings dialog for it. This is the file-only first step:
//!
//! ```json
//! { "ctrl+1": "Snap to rulers", "f2": "Cut" }
//! ```
//!
//! Chord on the left (modifiers in any order, `+`-joined, one key last),
//! on the right the label of a command from the command palette's
//! registry (`ui::quick::command_index`, matched case-insensitively).
//! The map is consulted BEFORE the built-in table, so a binding can
//! shadow a default — that is the point: the owner's CSP `Ctrl+1` is
//! ruler snap, ours was Zoom 100 %, and his muscle memory wins in his
//! own file. A chord matches its EXACT modifier set: binding `"p"` does
//! not fire on `Shift+P`.
//!
//! A chord can also name a TOOL TARGET instead of a command (owner ask
//! 2026-08-25) — a tool, one of its sub tool groups, or an exact sub tool:
//!
//! ```json
//! {
//!   "u": "tool: Frame border",
//!   "shift+u": "tool: Figure / Saturated line",
//!   "ctrl+shift+u": "tool: Figure / Direct draw / Ellipse",
//!   "j": ["tool: Fill", "tool: Auto select"]
//! }
//! ```
//!
//! The `tool:` prefix is what tells a target from a command label (a
//! palette command is free to be called anything). Names are the ones the
//! Sub Tool list shows, case-insensitively; a LIST binds several things to
//! one key and repeat-press cycles them in written order, which is how CSP
//! puts three tools on `U`. `crate::subtools` owns the model.
//!
//! A cycle may MIX the two kinds (owner ask 2026-09-05) — his `U` wants
//! Frame border, then Figure, then the straight-line ruler, and arming a
//! ruler is a command, not a tool:
//!
//! ```json
//! { "u": ["tool: Frame border", "tool: Figure", "Straight line ruler"] }
//! ```
//!
//! Which step a press runs is decided by looking at the app, never by a
//! stored index — see `main.rs::run_seq`.
//!
//! # Every key in one list
//!
//! The Shortcuts tab used to list keys.json rows only, so the built-in
//! keys (`U` = Frame border, `Ctrl+Z` = Undo) were invisible and looked
//! unbindable. [`effective_table`] merges the two hardcoded tables in
//! `main.rs` with the file into one list of [`Binding`]s, marked
//! [`Source::Default`] or [`Source::File`]; a file row replaces the
//! default of its chord and remembers what it shadowed.
//!
//! Commands that need a live layer index used to be unbindable — the door
//! is `AppCmd::ActiveLayer` now (layer colour, clip to below), which
//! resolves the row when the key is pressed rather than when it is read.
//!
//! Read once at startup, like `actions.json`. A missing file is silence;
//! a broken line is a startup status message naming the line, and every
//! other line still binds (the two-stage-parse lesson from actions.json:
//! one bad entry must not cost the whole file).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::cmd::AppCmd;
use crate::subtools::{Target, parse_target};

/// (ctrl, shift, alt, vk) — the exact shape `main.rs::shortcut` matches on.
pub(crate) type Chord = (bool, bool, bool, u16);

/// One thing a key press can do: run a palette command, or aim at a place
/// in the Sub Tool tree.
///
/// The two used to live in different worlds — a chord bound EITHER a
/// command OR a list of targets — which meant a cycle could hold tools
/// only, and the owner's `U` could not reach the straight-line ruler
/// because arming a ruler is a command (2026-09-05). Now a cycle is a list
/// of steps and the kinds mix freely.
#[derive(Clone, Debug)]
pub enum Step {
    Cmd(AppCmd),
    Target(Target),
}

/// What a chord does: its steps, in written order. ONE step and the press
/// simply runs it; SEVERAL and the press runs the step after whichever one
/// the app is standing on — `main.rs::run_seq` owns that rule, and for an
/// all-target sequence it is exactly `subtools::press`.
#[derive(Clone, Debug)]
pub enum Bind {
    Seq(Vec<Step>),
}

impl Bind {
    pub fn steps(&self) -> &[Step] {
        let Bind::Seq(s) = self;
        s
    }
}

#[derive(Default)]
pub struct Keymap {
    binds: HashMap<Chord, Bind>,
    /// Human-readable load complaints, surfaced once as a status line.
    pub problems: Vec<String>,
}

impl Keymap {
    /// `keys.json` beside the exe — same home as `actions.json`.
    pub fn load_beside_exe() -> Keymap {
        let Some(path) = keys_path() else {
            return Keymap::default();
        };
        Keymap::load_from(&path)
    }

    /// The loader behind [`Self::load_beside_exe`], path in the open —
    /// the Shortcut settings tab's save-then-apply goes through here in
    /// tests, where "beside the exe" is the build tree.
    pub fn load_from(path: &std::path::Path) -> Keymap {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Keymap::default(); // no file = no bindings, silently
        };
        Keymap::parse(&text)
    }

    /// Parse the file's text. Public for tests — no disk, no exe path.
    pub fn parse(text: &str) -> Keymap {
        let mut map = Keymap::default();
        let index = crate::ui::quick::command_index();
        let table: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                map.problems.push(format!("keys.json: not valid JSON ({e})"));
                return map;
            }
        };
        let Some(obj) = table.as_object() else {
            map.problems
                .push("keys.json: expected an object of \"chord\": \"command\"".into());
            return map;
        };
        for (chord_s, cmd_v) in obj {
            // An underscore key is a comment lane (JSON has no comments).
            if chord_s.starts_with('_') {
                continue;
            }
            let Some(chord) = parse_chord(chord_s) else {
                map.problems.push(format!("keys.json: unknown key \"{chord_s}\""));
                continue;
            };
            // A LIST is a cycle, and its entries may be of either kind
            // (2026-09-05). A single value is a one-step cycle.
            let items: Vec<&serde_json::Value> = match cmd_v.as_array() {
                Some(a) if a.is_empty() => {
                    map.problems
                        .push(format!("keys.json: \"{chord_s}\" — an empty cycle"));
                    continue;
                }
                Some(a) => a.iter().collect(),
                None => vec![cmd_v],
            };
            // One bad entry costs the whole chord ONE complaint, and the
            // chord stays unbound: a cycle with a hole in it would walk
            // somewhere its owner never wrote.
            let mut steps = Vec::with_capacity(items.len());
            let mut bad = None;
            for item in items {
                match item.as_str() {
                    Some(s) => match parse_step(s, &index) {
                        Ok(step) => steps.push(step),
                        Err(e) => bad = Some(e),
                    },
                    None => bad = Some("must name a command or a tool: target".to_owned()),
                }
                if bad.is_some() {
                    break;
                }
            }
            match bad {
                Some(e) => map
                    .problems
                    .push(format!("keys.json: \"{chord_s}\" — {e}")),
                None => {
                    map.binds.insert(chord, Bind::Seq(steps));
                }
            }
        }
        map
    }

    pub fn lookup(&self, ctrl: bool, shift: bool, alt: bool, vk: u16) -> Option<&Bind> {
        self.binds.get(&(ctrl, shift, alt, vk))
    }
}

/// One entry of a chord's right-hand side → one step.
///
/// `tool:` says "a place in the Sub Tool tree", which is a namespace of its
/// own — a palette command is free to be called anything, so the two cannot
/// be told apart by content. The `Err` is what the user reads in the status
/// bar, so it names the thing it could not find.
fn parse_step(want: &str, index: &[(&'static str, &'static str, AppCmd)]) -> Result<Step, String> {
    if want.trim_start().to_ascii_lowercase().starts_with("tool:") {
        return parse_target(want).map(Step::Target);
    }
    index
        .iter()
        .find(|(label, _, _)| label.eq_ignore_ascii_case(want))
        .map(|(_, _, cmd)| Step::Cmd(cmd.clone()))
        .ok_or_else(|| {
            format!("no command called \"{want}\" — the palette (Ctrl+K) knows the names")
        })
}

// --- the merged table: the defaults and the file in ONE list -------------

/// Where a row of the merged table comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Source {
    /// One of the two hardcoded tables in `main.rs` — the app's own keys.
    Default,
    /// A `keys.json` row. It REPLACES the default row of that chord.
    File,
}

/// One chord and everything it runs, whichever table it came from — the
/// model behind the Shortcuts tab's list.
///
/// The owner's complaint (2026-09-05) was that `U` opens the Frame tool and
/// yet is nowhere in the Shortcuts list: the built-ins were invisible, so
/// "modifiable" was not even a question you could ask. Every chord the app
/// answers is a row here now, and a row is a CYCLE — the value is a list of
/// steps, drawn as chips.
pub(crate) struct Binding {
    pub chord: Chord,
    /// The chord as text: a File row keeps the file's own spelling, a
    /// default row gets [`chord_text`]'s canonical one.
    pub key: String,
    /// One string per step, each spelled the way keys.json spells it
    /// (`"tool: Frame border"`, `"Undo"`). Empty when `raw` is set.
    pub entries: Vec<String>,
    pub source: Source,
    /// Can `entries` go into keys.json as they are? A few default rows are
    /// display labels for arms that are not palette commands at all
    /// ("Text / Balloon", "Hand (move)"): they are LISTED — that was the
    /// ask — but the tab cannot turn them into a file row by copying the
    /// label, so it does not offer to.
    pub spellable: bool,
    /// A File row's shadowed default, for the "default: …" hint and the ↺
    /// button that puts it back.
    pub shadows: Option<Vec<String>>,
    /// A file value the tab cannot show as chips (`42`, a nested array):
    /// kept VERBATIM and edited as raw JSON, exactly as before.
    pub raw: Option<serde_json::Value>,
}

/// Every binding the app answers to, defaults and file merged, sorted the
/// way the tab lists them.
///
/// `file` is the keys.json entries as the tab currently holds them (not
/// what is on disk — the tab's unsaved edits must show in the same list).
/// Entries whose key is not a chord at all (a `_`-comment lane, a
/// misspelled modifier) have no place in a chord-keyed table and are the
/// caller's to keep: `parse_chord` is the same filter this uses.
pub(crate) fn effective_table(file: &[Entry]) -> Vec<Binding> {
    let index = crate::ui::quick::command_index();
    let named = |c: Chord| chord_text(c.0, c.1, c.2, c.3).unwrap_or_default();
    let mut out: Vec<Binding> = Vec::new();
    // Targets FIRST: the bare tool letters are in both built-in tables, and
    // the target table is the one that can spell itself back into keys.json.
    for (chord, targets) in crate::builtin_target_rows() {
        out.push(Binding {
            chord,
            key: named(chord),
            entries: targets.iter().map(target_text).collect(),
            source: Source::Default,
            spellable: true,
            shadows: None,
            raw: None,
        });
    }
    for (chord, label) in crate::builtin_chords() {
        if out.iter().any(|b| b.chord == chord) {
            continue;
        }
        out.push(Binding {
            chord,
            key: named(chord),
            entries: vec![label.to_owned()],
            source: Source::Default,
            spellable: index.iter().any(|(l, _, _)| l.eq_ignore_ascii_case(label)),
            shadows: None,
            raw: None,
        });
    }
    for e in file {
        let Some(chord) = parse_chord(&e.key) else {
            continue;
        };
        let (entries, raw) = match entries_of(&e.value) {
            Some(v) => (v, None),
            None => (Vec::new(), Some(e.value.clone())),
        };
        let row = Binding {
            chord,
            key: e.key.clone(),
            entries,
            source: Source::File,
            spellable: true,
            shadows: None,
            raw,
        };
        // A keys.json line SHADOWS the default of its chord — one row, with
        // the default kept as the hint (and the ↺ that restores it).
        match out.iter().position(|b| b.chord == chord) {
            Some(i) => {
                let default = std::mem::replace(&mut out[i], row);
                out[i].shadows = Some(default.entries);
            }
            None => out.push(row),
        }
    }
    out.sort_by(|a, b| sort_rank(a).cmp(&sort_rank(b)));
    out
}

/// A file value as chips: a plain string is a one-step cycle, an array of
/// strings is the cycle it looks like. Anything else is a raw row.
fn entries_of(v: &serde_json::Value) -> Option<Vec<String>> {
    match v {
        serde_json::Value::String(s) => Some(vec![s.clone()]),
        serde_json::Value::Array(a) if !a.is_empty() => {
            a.iter().map(|x| x.as_str().map(str::to_owned)).collect()
        }
        _ => None,
    }
}

/// The list's order: bare keys first (letters A..Z before the digits and
/// punctuation), then the function keys, then everything with a modifier —
/// which is roughly the order a hand finds them in.
fn sort_rank(b: &Binding) -> (u8, u8, u16, &str) {
    let (ctrl, shift, alt, vk) = b.chord;
    let class = match (ctrl || shift || alt, vk) {
        (true, _) => 2,
        (false, 0x70..=0x7B) => 1,
        (false, _) => 0,
    };
    let letter = u8::from(!(0x41..=0x5A).contains(&vk));
    (class, letter, vk, &b.key)
}

/// A target back to its keys.json spelling — [`parse_target`]'s inverse,
/// and the same names `ui::shortcut_tab::addable` offers, so a default row
/// the tab turns into a file row loads back as the same target.
pub(crate) fn target_text(t: &Target) -> String {
    match t {
        Target::Tool(tool) => format!("tool: {}", tool.label()),
        Target::Group(tool, g) => format!("tool: {} / {g}", tool.label()),
        Target::SubTool(p) => format!("tool: {} / {} / {}", p.tool.label(), p.group, p.sub.label()),
    }
}

fn keys_path() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("keys.json"))
}

/// The path keys.json lives at, for the Shortcut settings tab's save.
pub(crate) fn keys_file() -> Option<PathBuf> {
    keys_path()
}

/// The reverse of [`vk_of`]: the canonical key name for a virtual-key
/// code, or `None` for one keys.json cannot spell (JP-layout OEM keys
/// are deliberately unnamed — a chord the file can't express must not be
/// offered by anything that builds one). Called only when a chord is
/// rendered as text, so the small allocation is free.
pub(crate) fn name_of_vk(vk: u16) -> Option<String> {
    Some(match vk {
        0x41..=0x5A => char::from(b'a' + (vk as u8 - 0x41)).to_string(),
        0x30..=0x39 => char::from(b'0' + (vk as u8 - 0x30)).to_string(),
        0x70..=0x7B => format!("f{}", vk - 0x70 + 1),
        0x20 => "space".into(),
        0x09 => "tab".into(),
        0x0D => "enter".into(),
        0x1B => "esc".into(),
        0x08 => "backspace".into(),
        0x2E => "del".into(),
        0x2D => "ins".into(),
        0x24 => "home".into(),
        0x23 => "end".into(),
        0x21 => "pageup".into(),
        0x22 => "pagedown".into(),
        0x26 => "up".into(),
        0x28 => "down".into(),
        0x25 => "left".into(),
        0x27 => "right".into(),
        0xDB => "[".into(),
        0xDD => "]".into(),
        0xBC => ",".into(),
        0xBE => ".".into(),
        0xBA => ";".into(),
        0xDE => "'".into(),
        0xBD => "-".into(),
        0xBB => "=".into(),
        0xBF => "/".into(),
        0xC0 => "`".into(),
        0xDC => "\\".into(),
        _ => return None,
    })
}

/// The chord as keys.json spells it: modifiers in canonical order
/// (Ctrl+Shift+Alt+), key name last — `parse_chord` is its inverse.
pub(crate) fn chord_text(ctrl: bool, shift: bool, alt: bool, vk: u16) -> Option<String> {
    let key = name_of_vk(vk)?;
    let mut s = String::new();
    for (on, m) in [(ctrl, "ctrl+"), (shift, "shift+"), (alt, "alt+")] {
        if on {
            s.push_str(m);
        }
    }
    s.push_str(&key);
    Some(s)
}

/// One keys.json entry, VERBATIM — the Shortcut settings tab's model.
/// Whatever the loader does with an entry, the editor keeps it: arrays,
/// `tool:` targets, `_`-prefixed comment lanes, and entries the loader
/// only complains about all ride a save untouched (rendered as plain
/// raw-text rows when the tab cannot edit them richly).
pub(crate) struct Entry {
    pub key: String,
    pub value: serde_json::Value,
}

/// The file's entries, in the object's (sorted) key order. `Err` is the
/// whole-file case the loader also refuses: not JSON, or not an object.
pub(crate) fn parse_entries(text: &str) -> Result<Vec<Entry>, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not valid JSON ({e})"))?;
    let obj = v.as_object().ok_or("expected an object of \"chord\": \"command\"")?;
    Ok(obj
        .into_iter()
        .map(|(k, val)| Entry {
            key: k.clone(),
            value: val.clone(),
        })
        .collect())
}

/// The entries back to file text — one save, one rewritten keys.json.
/// Key order is serde_json's (sorted): the file is REWRITTEN, entries
/// are never dropped or reordered in meaning, and `_`-comments survive
/// as the rows they are.
pub(crate) fn serialize_entries(entries: &[Entry]) -> Option<String> {
    let mut m = serde_json::Map::new();
    for e in entries {
        m.insert(e.key.clone(), e.value.clone());
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(m)).ok()
}

/// `"ctrl+shift+1"` → the exact-modifier chord. Case-insensitive;
/// modifiers in any order; the LAST token is the key. `chord_text` is
/// its inverse.
pub(crate) fn parse_chord(s: &str) -> Option<Chord> {
    let mut ctrl = false;
    let mut shift = false;
    let mut alt = false;
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    let (key, mods) = parts.split_last()?;
    for m in mods {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "shift" => shift = true,
            "alt" => alt = true,
            _ => return None,
        }
    }
    Some((ctrl, shift, alt, vk_of(key)?))
}

/// Key name → Windows virtual-key code. US-layout names for the OEM
/// punctuation — the same keys the built-in table binds. JP-layout
/// specials (`@`, the JP `;`) shift VKs per layout and are deliberately
/// not guessed here; a name this table cannot place is a load problem
/// the user sees, not a silently wrong key.
fn vk_of(name: &str) -> Option<u16> {
    let n = name.to_ascii_lowercase();
    let b = n.as_bytes();
    match (n.as_str(), b) {
        (_, [c @ b'a'..=b'z']) => Some((c - b'a') as u16 + 0x41),
        (_, [c @ b'0'..=b'9']) => Some((c - b'0') as u16 + 0x30),
        ("f1", _) => Some(0x70),
        ("f2", _) => Some(0x71),
        ("f3", _) => Some(0x72),
        ("f4", _) => Some(0x73),
        ("f5", _) => Some(0x74),
        ("f6", _) => Some(0x75),
        ("f7", _) => Some(0x76),
        ("f8", _) => Some(0x77),
        ("f9", _) => Some(0x78),
        ("f10", _) => Some(0x79),
        ("f11", _) => Some(0x7A),
        ("f12", _) => Some(0x7B),
        ("space", _) => Some(0x20),
        ("tab", _) => Some(0x09),
        ("enter" | "return", _) => Some(0x0D),
        ("esc" | "escape", _) => Some(0x1B),
        ("backspace", _) => Some(0x08),
        ("del" | "delete", _) => Some(0x2E),
        ("ins" | "insert", _) => Some(0x2D),
        ("home", _) => Some(0x24),
        ("end", _) => Some(0x23),
        ("pageup" | "pgup", _) => Some(0x21),
        ("pagedown" | "pgdn", _) => Some(0x22),
        ("up", _) => Some(0x26),
        ("down", _) => Some(0x28),
        ("left", _) => Some(0x25),
        ("right", _) => Some(0x27),
        ("[", _) => Some(0xDB),
        ("]", _) => Some(0xDD),
        (",", _) => Some(0xBC),
        (".", _) => Some(0xBE),
        (";", _) => Some(0xBA),
        ("'", _) => Some(0xDE),
        ("-" | "minus", _) => Some(0xBD),
        ("=" | "plus", _) => Some(0xBB),
        ("/", _) => Some(0xBF),
        ("`", _) => Some(0xC0),
        ("\\", _) => Some(0xDC),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_parse_exactly() {
        assert_eq!(parse_chord("ctrl+1"), Some((true, false, false, 0x31)));
        assert_eq!(
            parse_chord("Shift+Alt+E"),
            Some((false, true, true, 0x45))
        );
        assert_eq!(parse_chord("f2"), Some((false, false, false, 0x71)));
        assert_eq!(parse_chord("ctrl+["), Some((true, false, false, 0xDB)));
        assert_eq!(parse_chord("super+x"), None, "unknown modifier");
        assert_eq!(parse_chord("ctrl+@"), None, "JP-layout key, not guessed");
    }

    /// One bad line costs one line — the rest of the file still binds,
    /// and every complaint names its line's text.
    #[test]
    fn a_broken_entry_does_not_cost_the_file() {
        let m = Keymap::parse(
            r#"{
                "_comment": "owner keys",
                "ctrl+1": "Snap to rulers",
                "ctrl+7": "No Such Command",
                "hyper+9": "Undo",
                "f2": "cut"
            }"#,
        );
        assert!(m.lookup(true, false, false, 0x31).is_some(), "ctrl+1 bound");
        assert!(
            matches!(
                m.lookup(false, false, false, 0x71).map(Bind::steps),
                Some([Step::Cmd(AppCmd::Cut)])
            ),
            "f2 bound, label case-insensitive"
        );
        assert_eq!(m.problems.len(), 2, "two complaints, no more: {:?}", m.problems);
    }

    /// The whole point: a binding shadows a built-in, but only on its
    /// EXACT modifier set.
    #[test]
    fn lookup_is_exact_on_modifiers() {
        let m = Keymap::parse(r#"{ "p": "Undo" }"#);
        assert!(m.lookup(false, false, false, 0x50).is_some());
        assert!(
            m.lookup(false, true, false, 0x50).is_none(),
            "Shift+P is not P"
        );
    }

    /// The hook in `main.rs::shortcut`: a bound bare key pushes its
    /// command and consumes the press before the built-in table sees it
    /// (Q is deliberately a key the built-in table does not use).
    /// Modifier chords are untestable here — `shortcut` reads the REAL
    /// keyboard through `sync_modifiers`.
    #[test]
    fn a_binding_reaches_the_command_queue() {
        let Some(renderer) = crate::app::headless_renderer() else {
            return;
        };
        let mut app = crate::app::App::new(renderer, (900, 700), 1.0);
        app.keymap = Keymap::parse(r#"{ "q": "Undo" }"#);
        assert!(app.keymap.problems.is_empty(), "{:?}", app.keymap.problems);
        assert!(crate::shortcut(&mut app, 0x51, false), "the press consumed");
        assert!(
            matches!(app.cmds.back(), Some(AppCmd::Undo)),
            "the bound command queued"
        );
        // Unbound keys still fall through to the built-in table.
        let depth = app.cmds.len();
        assert!(crate::shortcut(&mut app, 0x50, false), "P is a built-in");
        assert!(app.cmds.len() > depth);
    }

    /// The targeting half: a chord can name a tool, a sub tool group or an
    /// exact sub tool, and a LIST of them is a repeat-press cycle. Same
    /// one-bad-line-costs-one-line discipline as the command half.
    #[test]
    fn a_chord_can_name_a_tool_target() {
        use crate::cmd::{FigureMode, SubTool, Tool};
        use crate::subtools::{SubToolPath, group};
        let m = Keymap::parse(
            r#"{
                "u": "tool: Frame border",
                "shift+u": "TOOL: figure / saturated line",
                "ctrl+u": "tool: Figure / Direct draw / Ellipse",
                "j": ["tool: Fill", "tool: Auto select"],
                "k": ["tool: Fill", "tool: Nope"],
                "l": [],
                "n": "tool: Nope"
            }"#,
        );
        let aimed = |vk: u16, c: bool, s: bool| -> Vec<Target> {
            m.lookup(c, s, false, vk)
                .map(Bind::steps)
                .unwrap_or_default()
                .iter()
                .filter_map(|st| match st {
                    Step::Target(t) => Some(*t),
                    Step::Cmd(_) => None,
                })
                .collect()
        };
        assert_eq!(
            aimed(0x55, false, false),
            [Target::Tool(Tool::Frame)],
            "a bare tool"
        );
        assert_eq!(
            aimed(0x55, false, true),
            [Target::Group(Tool::Figure, group::SATURATED_LINE)],
            "a group, case-insensitively"
        );
        assert_eq!(
            aimed(0x55, true, false),
            [Target::SubTool(SubToolPath::of(SubTool::Figure(
                FigureMode::Ellipse
            )))],
            "an exact sub tool"
        );
        assert_eq!(aimed(0x4A, false, false).len(), 2, "a cycle keeps its order");
        assert!(m.lookup(false, false, false, 0x4B).is_none(), "bad cycle");
        assert!(m.lookup(false, false, false, 0x4C).is_none(), "empty cycle");
        assert!(m.lookup(false, false, false, 0x4E).is_none(), "bad target");
        assert_eq!(m.problems.len(), 3, "one each, no more: {:?}", m.problems);
    }

    /// A cycle may hold commands and targets side by side (owner ask
    /// 2026-09-05): his `U` wants Frame border, Figure, and the
    /// straight-line ruler, and the last of those is a palette command.
    /// One bad entry still costs the whole chord exactly one complaint.
    #[test]
    fn a_cycle_may_mix_commands_and_targets() {
        use crate::cmd::{RulerKind, Tool};
        let m = Keymap::parse(
            r#"{
                "u": ["tool: Frame border", "tool: Figure", "Straight line ruler"],
                "q": ["Undo", "tool: Nope"],
                "r": ["Undo", 42]
            }"#,
        );
        let steps = m.lookup(false, false, false, 0x55).map(Bind::steps);
        assert!(
            matches!(
                steps,
                Some(
                    [
                        Step::Target(Target::Tool(Tool::Frame)),
                        Step::Target(Target::Tool(Tool::Figure)),
                        Step::Cmd(AppCmd::RulerArm(RulerKind::Line)),
                    ]
                )
            ),
            "three steps, in written order: {steps:?}"
        );
        assert!(m.lookup(false, false, false, 0x51).is_none(), "one bad target");
        assert!(m.lookup(false, false, false, 0x52).is_none(), "not a string");
        assert_eq!(m.problems.len(), 2, "one each: {:?}", m.problems);
    }

    /// The pin behind the merged table's default rows: `builtin_target_rows`
    /// is derived FROM `builtin_targets`, so their values cannot drift —
    /// what can drift is the key list, so this walks the whole vk space and
    /// fails if the match answers a chord no row lists.
    #[test]
    fn every_builtin_target_row_is_the_match() {
        let rows = crate::builtin_target_rows();
        assert!(rows.len() > 10, "the tool letters: {}", rows.len());
        for ((ctrl, shift, alt, vk), targets) in &rows {
            assert!(!ctrl && !alt, "{vk:#x}: the target table is Ctrl/Alt-free");
            assert_eq!(
                crate::builtin_targets(*vk, *shift),
                Some(*targets),
                "row {vk:#x} is not what the match answers"
            );
        }
        for vk in 0u16..=0xFF {
            for shift in [false, true] {
                let Some(t) = crate::builtin_targets(vk, shift) else {
                    continue;
                };
                assert!(
                    rows.iter().any(|((.., v), r)| *v == vk && *r == t),
                    "{vk:#x} (shift {shift}) aims at {t:?}, but no row lists it — \
                     add it to builtin_target_rows or the Shortcuts tab hides it"
                );
            }
        }
    }

    /// The merged table is every chord the app answers: the built-in tool
    /// letters and command chords as `Default` rows, a keys.json line
    /// REPLACING the default of its chord and remembering what it shadowed.
    #[test]
    fn the_merged_table_lists_defaults_and_the_file() {
        let file = parse_entries(r#"{ "u": ["tool: Figure"], "_c": "note", "hyper+9": "Undo" }"#)
            .expect("parses");
        let table = effective_table(&file);
        let row = |k: &str| table.iter().find(|b| b.key == k).expect(k);
        // A built-in that was invisible before — the owner's complaint.
        assert_eq!(row("ctrl+z").entries, ["Undo"]);
        assert_eq!(row("ctrl+z").source, Source::Default);
        assert_eq!(row("f").entries, ["tool: Figure"], "a default tool letter");
        // The file's own row, in place of the default it shadows.
        let u = row("u");
        assert_eq!(u.source, Source::File);
        assert_eq!(u.entries, ["tool: Figure"]);
        // What U's default holds is `builtin_targets`' business (a cycle,
        // since 2026-09-05); what THIS test pins is that the file row keeps
        // it as the hint the ↺ button restores.
        let shadowed = u.shadows.as_deref().expect("it knows what it hid");
        assert!(
            shadowed.contains(&"tool: Frame border".to_owned()),
            "{shadowed:?}"
        );
        assert_eq!(table.iter().filter(|b| b.key == "u").count(), 1, "one row");
        // Keys that are not chords at all are the caller's to keep.
        assert!(!table.iter().any(|b| b.key == "_c" || b.key == "hyper+9"));
        // Sorted: bare letters, then function keys, then modifier chords.
        let pos = |k: &str| table.iter().position(|b| b.key == k).expect(k);
        assert!(pos("u") < pos("f8"), "letters before the function keys");
        assert!(pos("f8") < pos("ctrl+z"), "modifier chords last");
    }

    /// A mixed cycle walks with no stored index: each press runs the step
    /// after whichever one the app is standing on. This is the acceptance
    /// lap — Frame border, Figure, ruler armed, back to Frame border.
    #[test]
    fn a_mixed_cycle_advances_from_the_current_step() {
        use crate::cmd::{RulerKind, Tool, dispatch};
        let Some(renderer) = crate::app::headless_renderer() else {
            return;
        };
        let mut app = crate::app::App::new(renderer, (600, 400), 1.0);
        app.layout = crate::app::UiLayout::default();
        app.keymap = Keymap::parse(
            r#"{ "q": ["tool: Frame border", "tool: Figure", "Straight line ruler"] }"#,
        );
        assert!(app.keymap.problems.is_empty(), "{:?}", app.keymap.problems);
        let press = |app: &mut crate::app::App| {
            assert!(crate::shortcut(app, 0x51, false), "the press consumed");
            while let Some(c) = app.cmds.pop_front() {
                dispatch(app, c);
            }
        };
        press(&mut app);
        assert_eq!(app.tool, Tool::Frame, "step 0, nothing was current");
        press(&mut app);
        assert_eq!(app.tool, Tool::Figure, "the step after the current one");
        press(&mut app);
        assert_eq!(
            app.ruler_pending,
            Some(RulerKind::Line),
            "a command step runs like any other"
        );
        press(&mut app);
        assert_eq!(app.tool, Tool::Frame, "round the lap");
        // Auto-repeat never advances a cycle.
        assert!(crate::shortcut(&mut app, 0x51, true), "still consumed");
        assert!(app.cmds.is_empty(), "a held key queues nothing");
    }

    /// Garbage at the top level degrades to an empty map plus a complaint —
    /// never a panic at startup.
    #[test]
    fn garbage_files_degrade_to_empty() {
        for text in ["not json at all", "[1,2,3]", "42"] {
            let m = Keymap::parse(text);
            assert!(m.binds.is_empty());
            assert_eq!(m.problems.len(), 1, "{text}");
        }
    }
}
