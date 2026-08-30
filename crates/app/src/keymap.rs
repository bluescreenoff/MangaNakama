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
//! Sub Tool list shows, case-insensitively; a LIST binds several targets to
//! one key and repeat-press cycles them in written order, which is how CSP
//! puts three tools on `U`. `crate::subtools` owns the model.
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
type Chord = (bool, bool, bool, u16);

/// What a chord does: run one command, or aim at one or more tool targets
/// (several = a repeat-press cycle, `subtools::press`).
#[derive(Clone, Debug)]
pub enum Bind {
    Cmd(AppCmd),
    Targets(Vec<Target>),
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
            // A LIST is a cycle: every entry must be a target, since
            // cycling needs to know which one you are standing on and only
            // a target can answer that.
            if let Some(list) = cmd_v.as_array() {
                let mut targets = Vec::new();
                let mut bad = None;
                for item in list {
                    match item.as_str().map(parse_target) {
                        Some(Ok(t)) => targets.push(t),
                        Some(Err(e)) => bad = Some(e),
                        None => bad = Some("a cycle entry must be a string".to_owned()),
                    }
                }
                match (bad, targets.is_empty()) {
                    (Some(e), _) => map
                        .problems
                        .push(format!("keys.json: \"{chord_s}\" — {e}")),
                    (None, true) => map
                        .problems
                        .push(format!("keys.json: \"{chord_s}\" — an empty cycle")),
                    (None, false) => {
                        map.binds.insert(chord, Bind::Targets(targets));
                    }
                }
                continue;
            }
            let Some(want) = cmd_v.as_str() else {
                map.problems
                    .push(format!("keys.json: \"{chord_s}\" must name a command"));
                continue;
            };
            // `tool:` says "a place in the Sub Tool tree", which is a
            // namespace of its own — a palette command may be called
            // anything, so the two cannot be told apart by content.
            if want.trim_start().to_ascii_lowercase().starts_with("tool:") {
                match parse_target(want) {
                    Ok(t) => {
                        map.binds.insert(chord, Bind::Targets(vec![t]));
                    }
                    Err(e) => map
                        .problems
                        .push(format!("keys.json: \"{chord_s}\" — {e}")),
                }
                continue;
            }
            let found = index
                .iter()
                .find(|(label, _, _)| label.eq_ignore_ascii_case(want));
            match found {
                Some((_, _, cmd)) => {
                    map.binds.insert(chord, Bind::Cmd(cmd.clone()));
                }
                None => map.problems.push(format!(
                    "keys.json: no command called \"{want}\" — the palette (Ctrl+K) knows the names"
                )),
            }
        }
        map
    }

    pub fn lookup(&self, ctrl: bool, shift: bool, alt: bool, vk: u16) -> Option<&Bind> {
        self.binds.get(&(ctrl, shift, alt, vk))
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
                m.lookup(false, false, false, 0x71),
                Some(Bind::Cmd(AppCmd::Cut))
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
        assert!(
            matches!(
                m.lookup(false, false, false, 0x55),
                Some(Bind::Targets(t)) if t[..] == [Target::Tool(Tool::Frame)]
            ),
            "a bare tool"
        );
        assert!(
            matches!(
                m.lookup(false, true, false, 0x55),
                Some(Bind::Targets(t))
                    if t[..] == [Target::Group(Tool::Figure, group::SATURATED_LINE)]
            ),
            "a group, case-insensitively"
        );
        assert!(
            matches!(
                m.lookup(true, false, false, 0x55),
                Some(Bind::Targets(t))
                    if t[..] == [Target::SubTool(SubToolPath::of(SubTool::Figure(
                        FigureMode::Ellipse
                    )))]
            ),
            "an exact sub tool"
        );
        assert!(
            matches!(m.lookup(false, false, false, 0x4A), Some(Bind::Targets(t)) if t.len() == 2),
            "a cycle keeps its written order"
        );
        assert!(m.lookup(false, false, false, 0x4B).is_none(), "bad cycle");
        assert!(m.lookup(false, false, false, 0x4C).is_none(), "empty cycle");
        assert!(m.lookup(false, false, false, 0x4E).is_none(), "bad target");
        assert_eq!(m.problems.len(), 3, "one each, no more: {:?}", m.problems);
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
