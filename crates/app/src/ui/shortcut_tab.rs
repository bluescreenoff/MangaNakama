//! The Shortcuts tab of the Preferences window: a friendly editor over
//! `keys.json`. The FILE is the truth and stays the truth — this tab
//! never keeps a second store, it loads the file's entries as rows and
//! one save is one rewritten file, applied live through the same loader
//! startup uses (no restart).
//!
//! # Round-trip is the hard requirement
//!
//! Whatever the tab cannot edit richly still SURVIVES a save: `tool:`
//! targets and plain command rows are strings (edited as text), while
//! arrays (repeat-press cycles), `_`-prefixed comment lanes and entries
//! the loader only complains about render as PLAIN rows whose value box
//! holds raw JSON. A plain row whose edit box is mid-garbage at save
//! time keeps its last VALID value and says so — never dropped, never
//! mangled. Key order after a save is the JSON object's (sorted); the
//! entries' meaning is what must not move, and it doesn't.
//!
//! # What the tab refuses (v1)
//!
//! It never offers a chord keys.json cannot spell — the capture field
//! maps only keys with canonical names (JP-layout OEM keys stay
//! unguessable, the recorded refusal), and says so when pressed.

use crate::app::App;
use crate::keymap::{self, Entry};
use crate::subtools;

/// The tab's edit state. One instance lives on `App`; rows load from
/// keys.json the first time the tab is shown and on Reload.
#[derive(Default)]
pub(crate) struct State {
    rows: Vec<Row>,
    loaded: bool,
    status: String,
    search: String,
    /// The captured chord, exactly as keys.json would spell it.
    cap_text: String,
    cap: Option<(bool, bool, bool, u16)>,
    cap_note: String,
}

/// One keys.json entry as a row. `was_string` decides how the value box
/// edits: string values edit as the plain text they are, everything else
/// edits as raw JSON — and `loaded` is what a plain row saves when its
/// edit text does not currently parse.
struct Row {
    key: String,
    text: String,
    was_string: bool,
    loaded: serde_json::Value,
}

impl State {
    /// Re-read keys.json into rows. A missing file is the empty map (the
    /// first save writes one); a broken file keeps the editor empty and
    /// shows the whole-file complaint.
    fn reload_from_file(&mut self) {
        self.rows.clear();
        self.status.clear();
        let Some(p) = keymap::keys_file() else {
            self.status = "keys.json has no home beside the exe".into();
            return;
        };
        let Ok(text) = std::fs::read_to_string(&p) else {
            self.status = "no keys.json yet — the first save writes one".into();
            return;
        };
        match keymap::parse_entries(&text) {
            Ok(entries) => {
                self.rows = entries.into_iter().map(row_of).collect();
            }
            Err(e) => self.status = format!("keys.json: {e}"),
        }
    }

    /// The rows back to file text. `notes` collects the plain rows whose
    /// edit text does not parse (they keep their last valid value — the
    /// round-trip guarantee — and say so).
    fn serialize(&self) -> (String, Vec<String>) {
        let mut notes = Vec::new();
        let entries: Vec<Entry> = self
            .rows
            .iter()
            .map(|r| {
                let value = if r.was_string {
                    serde_json::Value::String(r.text.clone())
                } else {
                    match serde_json::from_str::<serde_json::Value>(&r.text) {
                        Ok(v) => v,
                        Err(_) => {
                            notes.push(format!("\"{}\" kept its last valid value", r.key));
                            r.loaded.clone()
                        }
                    }
                };
                Entry {
                    key: r.key.trim().to_owned(),
                    value,
                }
            })
            .collect();
        (keymap::serialize_entries(&entries).unwrap_or_default(), notes)
    }
}

fn row_of(e: Entry) -> Row {
    match &e.value {
        serde_json::Value::String(s) => Row {
            key: e.key,
            text: s.clone(),
            was_string: true,
            loaded: e.value,
        },
        v => Row {
            key: e.key,
            text: v.to_string(),
            was_string: false,
            loaded: e.value,
        },
    }
}

/// What pressing the captured chord would displace: a same-chord row in
/// the file first (that is what a save re-aims), else the built-in the
/// binding would shadow.
fn conflict_note(
    chord: (bool, bool, bool, u16),
    rows: &[Row],
    builtins: &[((bool, bool, bool, u16), &str)],
) -> Option<String> {
    if let Some(r) = rows
        .iter()
        .find(|r| keymap::parse_chord(&r.key) == Some(chord))
    {
        return Some(format!("already bound here → {}", r.text));
    }
    builtins
        .iter()
        .find(|(c, _)| *c == chord)
        .map(|(_, label)| format!("shadows built-in: {label}"))
}

    /// The addable namespace: every palette command label, plus the `tool:`
/// target tree (`tool: Pen`, `tool: Figure / Direct draw`, `tool:
/// Figure / Direct draw / Ellipse`) — the same names `parse_target`
/// answers to, generated from the sub-tool registry itself.
fn addable() -> Vec<(String, &'static str)> {
    let mut v: Vec<(String, &'static str)> = crate::ui::quick::command_index()
        .into_iter()
        .map(|(label, _, _)| (label.to_owned(), "command"))
        .collect();
    // The nameable set is `nameable_tools` — the same list `parse_target`
    // resolves bare names against, so every row this builds is loadable.
    let tools: Vec<crate::cmd::Tool> = subtools::nameable_tools().collect();
    for t in tools {
        v.push((format!("tool: {}", t.label()), "tool"));
        for g in subtools::groups_of(t) {
            v.push((format!("tool: {} / {}", t.label(), g.name), "tool"));
            for &s in &g.subs {
                let sub = subtools::SubToolPath::of(s);
                v.push((
                    format!("tool: {} / {} / {}", t.label(), sub.group, s.label()),
                    "tool",
                ));
            }
        }
    }
    v
}

impl App {
    /// keys.json re-read LIVE (the Shortcuts tab's save tail — the same
    /// loader startup uses, one bad line one complaint, no restart).
    /// Returns the loader's complaints for the tab to surface; the
    /// startup status-line behavior is untouched.
    pub(crate) fn reload_keymap(&mut self) -> Vec<String> {
        self.keymap = keymap::Keymap::load_beside_exe();
        self.keymap.problems.clone()
    }
}

enum Action {
    Save,
    Reload,
}

/// The tab body. `app.shortcut_edit` holds the state; the save tail
/// borrows `app` only after the row editing is done (NLL keeps the two
/// borrows apart).
pub(super) fn tab(ui: &mut egui::Ui, app: &mut App) {
    let st = &mut app.shortcut_edit;
    if !st.loaded {
        st.reload_from_file();
        st.loaded = true;
    }
    let mut action: Option<Action> = None;

    // --- the rows --------------------------------------------------------
    ui.horizontal(|ui| {
        if ui.button("Save & apply").clicked() {
            action = Some(Action::Save);
        }
        if ui.small_button("Reload file").clicked() {
            action = Some(Action::Reload);
        }
    });
    if !st.status.is_empty() {
        ui.label(&st.status);
    }
    ui.add_space(4.0);
    let mut delete = None;
    egui::ScrollArea::vertical()
        .max_height(170.0)
        .show(ui, |ui| {
            for (i, r) in st.rows.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut r.key)
                            .desired_width(96.0)
                            .font(egui::TextStyle::Monospace),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut r.text)
                            .desired_width(ui.available_width() - 28.0)
                            .font(egui::TextStyle::Monospace),
                    )
                    .on_hover_text(if r.was_string {
                        "a command label or a tool: target — exactly what keys.json reads"
                    } else {
                        "raw JSON (a cycle array, a comment lane's value) — kept as written"
                    });
                    if ui.small_button("×").clicked() {
                        delete = Some(i);
                    }
                });
            }
        });
    if let Some(i) = delete {
        st.rows.remove(i);
    }

    // --- add a binding: capture the chord, pick what it runs -------------
    ui.add_space(6.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Add:");
        // The field edits the chord text directly — a captured press
        // writes it, a manual typing does too, and the loader's own
        // parser decides what is well-spelled.
        let cap_resp = ui.add(
            egui::TextEdit::singleline(&mut st.cap_text)
                .hint_text("press a key…")
                .desired_width(120.0)
                .font(egui::TextStyle::Monospace),
        );
        if cap_resp.has_focus() {
            let events = ui.input(|i| i.events.clone());
            let mods = ui.input(|i| i.modifiers);
            for ev in events {
                match ev {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        match vk_of_egui(key)
                            .map(|vk| (modifiers.ctrl, modifiers.shift, modifiers.alt, vk))
                        {
                            Some((c, s, a, vk)) => {
                                st.cap = Some((c, s, a, vk));
                                st.cap_text = keymap::chord_text(c, s, a, vk).unwrap_or_default();
                                st.cap_note = conflict_note(
                                    (c, s, a, vk),
                                    &st.rows,
                                    &crate::builtin_chords(),
                                )
                                .unwrap_or_else(|| "free".into());
                            }
                            None => {
                                st.cap_note = "that key has no keys.json spelling (v1: no OEM \
                                               / JP-layout guesses)"
                                    .into();
                            }
                        }
                    }
                    // `; ' / ` \ arrive as Text (the shell maps the other
                    // OEM keys to Key events); only bare, since a modified
                    // press means the Key arm above or a shortcut we
                    // should not eat.
                    egui::Event::Text(t) if !mods.ctrl && !mods.alt && t.chars().count() == 1 => {
                        let ch = t.chars().next().unwrap_or(' ');
                        if let Some(vk) = vk_of_char(ch)
                            && let (c, s, a) = (mods.ctrl, mods.shift, mods.alt)
                        {
                            st.cap = Some((c, s, a, vk));
                            st.cap_text = ch.to_string();
                            st.cap_note = conflict_note((c, s, a, vk), &st.rows, &crate::builtin_chords())
                                .unwrap_or_else(|| "free".into());
                        }
                    }
                    _ => {}
                }
            }
        } else if cap_resp.changed() {
            // Manual typing: re-resolve the chord and its conflicts; an
            // unparseable chord keeps the note honest.
            st.cap = keymap::parse_chord(&st.cap_text);
            st.cap_note = match st.cap {
                Some(c) => {
                    conflict_note(c, &st.rows, &crate::builtin_chords())
                        .unwrap_or_else(|| "free".into())
                }
                None if st.cap_text.trim().is_empty() => String::new(),
                None => "not a chord keys.json can read".into(),
            };
        }
        if !st.cap_note.is_empty() {
            ui.weak(&st.cap_note);
        }
    });
    ui.add(
        egui::TextEdit::singleline(&mut st.search)
            .hint_text("search commands and tool: targets…")
            .desired_width(f32::INFINITY),
    );
    let q = st.search.trim().to_lowercase();
    if !q.is_empty() {
        let hits: Vec<(String, &'static str)> = addable()
            .into_iter()
            .filter(|(label, _)| label.to_lowercase().contains(&q))
            .take(8)
            .collect();
        if hits.is_empty() {
            ui.weak("nothing to run by that name");
        }
        for (label, kind) in hits {
            let row = ui.selectable_label(false, format!("{label}   —   {kind}"));
            if row.clicked() {
                // A captured press wrote cap_text; manual typing may have
                // too — the file's own parser is the authority.
                if let Some(c) = keymap::parse_chord(&st.cap_text).or(st.cap) {
                    let text = keymap::chord_text(c.0, c.1, c.2, c.3).unwrap_or_default();
                    // Taking a chord the file already holds RE-AIMS that
                    // row (what the conflict note showed); a built-in
                    // just gains its shadow.
                    if let Some(existing) = st
                        .rows
                        .iter_mut()
                        .find(|r| keymap::parse_chord(&r.key) == Some(c))
                    {
                        existing.text = label.clone();
                        existing.was_string = true;
                        existing.loaded = serde_json::Value::String(label.clone());
                        st.status = format!("re-aimed \"{text}\" → {label}");
                    } else {
                        st.rows.push(Row {
                            key: text.clone(),
                            text: label.clone(),
                            was_string: true,
                            loaded: serde_json::Value::String(label),
                        });
                        st.status = format!("\"{text}\" will bind on save");
                    }
                } else {
                    st.status = "press (or type) a chord first".into();
                }
                st.search.clear();
            }
        }
    }

    // --- the deferred actions, after the row borrows are done ------------
    match action {
        Some(Action::Reload) => {
            app.shortcut_edit.reload_from_file();
        }
        Some(Action::Save) => {
            let (text, notes) = app.shortcut_edit.serialize();
            let path = keymap::keys_file();
            let written = path
                .as_ref()
                .map(|p| std::fs::write(p, &text).is_ok())
                .unwrap_or(false);
            let problems = app.reload_keymap();
            app.shortcut_edit.status = if !written {
                "could not write keys.json".into()
            } else if problems.is_empty() && notes.is_empty() {
                "saved · applied".into()
            } else {
                [problems, vec![notes.join("  ·  ")]].concat().join("  ·  ")
            };
        }
        None => {}
    }
}

/// egui's Key → the Windows VK the shortcut table matches on. Only the
/// keys keys.json can spell (which is also exactly what the shell maps
/// to Key events); everything else is the caller's refusal.
fn vk_of_egui(k: egui::Key) -> Option<u16> {
    use egui::Key as K;
    Some(match k {
        K::A => 0x41,
        K::B => 0x42,
        K::C => 0x43,
        K::D => 0x44,
        K::E => 0x45,
        K::F => 0x46,
        K::G => 0x47,
        K::H => 0x48,
        K::I => 0x49,
        K::J => 0x4A,
        K::K => 0x4B,
        K::L => 0x4C,
        K::M => 0x4D,
        K::N => 0x4E,
        K::O => 0x4F,
        K::P => 0x50,
        K::Q => 0x51,
        K::R => 0x52,
        K::S => 0x53,
        K::T => 0x54,
        K::U => 0x55,
        K::V => 0x56,
        K::W => 0x57,
        K::X => 0x58,
        K::Y => 0x59,
        K::Z => 0x5A,
        K::Num0 => 0x30,
        K::Num1 => 0x31,
        K::Num2 => 0x32,
        K::Num3 => 0x33,
        K::Num4 => 0x34,
        K::Num5 => 0x35,
        K::Num6 => 0x36,
        K::Num7 => 0x37,
        K::Num8 => 0x38,
        K::Num9 => 0x39,
        K::F1 => 0x70,
        K::F2 => 0x71,
        K::F3 => 0x72,
        K::F4 => 0x73,
        K::F5 => 0x74,
        K::F6 => 0x75,
        K::F7 => 0x76,
        K::F8 => 0x77,
        K::F9 => 0x78,
        K::F10 => 0x79,
        K::F11 => 0x7A,
        K::F12 => 0x7B,
        K::ArrowUp => 0x26,
        K::ArrowDown => 0x28,
        K::ArrowLeft => 0x25,
        K::ArrowRight => 0x27,
        K::Escape => 0x1B,
        K::Tab => 0x09,
        K::Backspace => 0x08,
        K::Enter => 0x0D,
        K::Space => 0x20,
        K::Insert => 0x2D,
        K::Delete => 0x2E,
        K::Home => 0x24,
        K::End => 0x23,
        K::PageUp => 0x21,
        K::PageDown => 0x22,
        // The OEM keys the shell maps to Key events.
        K::Equals => 0xBB,
        K::Comma => 0xBC,
        K::Minus => 0xBD,
        K::Period => 0xBE,
        K::OpenBracket => 0xDB,
        K::CloseBracket => 0xDD,
        _ => return None,
    })
}

/// The OEM keys that reach egui as TEXT instead (`; ' / ` \) — the same
/// five vk_of names, reached the only way the shell delivers them.
fn vk_of_char(c: char) -> Option<u16> {
    Some(match c {
        ';' => 0xBA,
        '\'' => 0xDE,
        '/' => 0xBF,
        '`' => 0xC0,
        '\\' => 0xDC,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE hard requirement: every entry — arrays, `tool:` targets,
    /// `_`-comment lanes, entries the loader only complains about —
    /// survives a load→edit-nothing→save→load round trip with its
    /// meaning intact, and the loader binds exactly what it bound
    /// before.
    #[test]
    fn the_round_trip_preserves_every_entry() {
        let text = r#"{
            "_comment": "owner keys",
            "ctrl+1": "Snap to rulers",
            "u": "tool: Frame border",
            "j": ["tool: Fill", "tool: Auto select"],
            "hyper+9": "Undo",
            "ctrl+7": "No Such Command",
            "odd": 42
        }"#;
        let entries = keymap::parse_entries(text).expect("parses");
        let rows: Vec<Row> = entries.into_iter().map(row_of).collect();
        let st = State {
            rows,
            ..Default::default()
        };
        let (saved, notes) = st.serialize();
        assert!(notes.is_empty(), "untouched rows never need notes: {notes:?}");

        // Meaning, both directions: the entries are all still there with
        // the same values, and the loader binds the same chords.
        let back = keymap::parse_entries(&saved).expect("saved text reparses");
        assert_eq!(back.len(), 7, "nothing dropped: {saved}");
        let get = |k: &str| {
            back.iter()
                .find(|e| e.key == k)
                .map(|e| e.value.to_string())
                .unwrap_or_default()
        };
        assert_eq!(get("_comment"), r#""owner keys""#);
        assert_eq!(get("j"), r#"["tool: Fill","tool: Auto select"]"#);
        assert_eq!(get("odd"), "42");
        assert_eq!(get("ctrl+1"), r#""Snap to rulers""#);
        let before = keymap::Keymap::parse(text);
        let after = keymap::Keymap::parse(&saved);
        assert_eq!(before.problems.len(), after.problems.len());
        assert!(
            after.lookup(true, false, false, 0x31).is_some()
                && after.lookup(false, false, false, 0x4A).is_some(),
            "the good bindings still bind through the saved file"
        );
    }

    /// A plain row mid-garbage at save time keeps its last valid value —
    /// the round-trip guarantee is stronger than the edit box's state.
    #[test]
    fn a_broken_raw_edit_keeps_its_last_valid_value() {
        let text = r#"{ "j": ["tool: Fill", "tool: Auto select"] }"#;
        let entries = keymap::parse_entries(text).unwrap();
        let mut rows: Vec<Row> = entries.into_iter().map(row_of).collect();
        rows[0].text = "[tool: Fill".to_owned(); // no longer JSON
        let (saved, notes) = State {
            rows,
            ..Default::default()
        }
        .serialize();
        assert_eq!(notes.len(), 1, "the note names the row: {notes:?}");
        assert!(
            saved.contains("tool: Fill"),
            "the cycle entry survived: {saved}"
        );
    }

    /// The conflict lookup names what a chord already does: a file row
    /// first, else the built-in a binding would shadow.
    #[test]
    fn the_conflict_lookup_names_both_sides() {
        let rows = vec![Row {
            key: "ctrl+1".into(),
            text: "Snap to rulers".into(),
            was_string: true,
            loaded: serde_json::Value::String("Snap to rulers".into()),
        }];
        let builtins = crate::builtin_chords();
        let note = conflict_note((true, false, false, 0x31), &rows, &builtins).unwrap();
        assert!(note.contains("already bound here"), "{note}");
        let note = conflict_note((true, false, false, 0x5A), &rows, &builtins).unwrap();
        assert!(note.contains("shadows built-in: Undo"), "{note}");
        assert!(conflict_note((false, false, false, 0x51), &rows, &builtins).is_none());
    }

    /// The pin behind the "shadows built-in" hints: every chord in
    /// `builtin_chords` actually consumes on a fresh app, so the table
    /// cannot drift into lying.
    #[test]
    fn every_builtin_chord_still_consumes() {
        let Some(renderer) = crate::app::headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (600, 400), 1.0);
        app.keymap = keymap::Keymap::default(); // no user shadowing
        for ((c, s, a, vk), label) in crate::builtin_chords() {
            let mods = app.shell.test_modifiers.take();
            app.shell.test_modifiers = Some(egui::Modifiers {
                ctrl: c,
                shift: s,
                alt: a,
                ..Default::default()
            });
            let consumed = crate::shortcut(&mut app, vk, false);
            app.shell.test_modifiers = mods;
            assert!(consumed, "the table says {label} holds {vk:#x} — it doesn't");
        }
    }

    /// Live apply without restart: a saved file re-binds, through the
    /// same door the save tail uses, and the press reaches the queue.
    #[test]
    fn a_saved_file_rebinds_without_restart() {
        let dir = std::env::temp_dir().join(format!("mn-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("keys.json");

        std::fs::write(&path, r#"{ "q": "Undo" }"#).unwrap();
        let m = keymap::Keymap::load_from(&path);
        assert!(m.lookup(false, false, false, 0x51).is_some(), "first binding");

        // The tab's save: edit rows, serialize, write…
        let entries =
            keymap::parse_entries(r#"{ "q": "Redo", "f2": "tool: Frame border" }"#).unwrap();
        std::fs::write(&path, keymap::serialize_entries(&entries).unwrap()).unwrap();
        // …and apply through the loader, no restart:
        let applied = keymap::Keymap::load_from(&path);
        assert_eq!(applied.problems.len(), 0, "{:?}", applied.problems);
        assert!(matches!(
            applied.lookup(false, false, false, 0x51),
            Some(keymap::Bind::Cmd(crate::cmd::AppCmd::Redo))
        ));
        assert!(applied.lookup(false, false, false, 0x71).is_some());

        // A bad hand-edited line still binds the rest (loader authority,
        // unchanged by the dialog's existence).
        std::fs::write(&path, r#"{ "q": "Redo", "hyper+9": "Undo" }"#).unwrap();
        let m = keymap::Keymap::load_from(&path);
        assert!(m.lookup(false, false, false, 0x51).is_some());
        assert_eq!(m.problems.len(), 1, "{:?}", m.problems);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The chord text is `parse_chord`'s inverse on both directions, so
    /// the capture field can only ever build well-spelled chords.
    #[test]
    fn chord_text_round_trips() {
        for (c, s, a, vk) in [
            (true, false, false, 0x31),
            (false, true, true, 0x45),
            (false, false, false, 0x71),
            (true, false, false, 0xDB),
            (true, true, true, 0x20),
        ] {
            let text = keymap::chord_text(c, s, a, vk).unwrap();
            assert_eq!(keymap::parse_chord(&text), Some((c, s, a, vk)), "{text}");
        }
        assert!(keymap::chord_text(false, false, false, 0xC1).is_none(), "unspellable");
    }

    /// The addable namespace walks the real registry: tools, groups and
    /// exact sub tools, every one parseable by `parse_target`.
    #[test]
    fn the_addable_namespace_is_all_parseable() {
        let all = addable();
        assert!(all.len() > 40, "commands + the tool tree: {}", all.len());
        for (label, kind) in &all {
            if *kind == "tool" {
                assert!(
                    subtools::parse_target(label).is_ok(),
                    "{label} does not parse"
                );
            }
        }
        assert!(all.iter().any(|(l, _)| l == "tool: Figure / Direct draw / Ellipse"));
    }
}
