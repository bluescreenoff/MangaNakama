//! The Shortcuts tab of the Preferences window: a friendly editor over
//! `keys.json`. The FILE is the truth and stays the truth — this tab
//! never keeps a second store, it loads the file's entries as rows and
//! one save is one rewritten file, applied live through the same loader
//! startup uses (no restart).
//!
//! # Every key is a row
//!
//! The list used to hold keys.json rows ONLY, so the built-in keys were
//! invisible: the owner pressed `U`, got the Frame tool, and could not
//! find `U` anywhere in the list (2026-09-05, "all keys you can press
//! should be in the shortcuts list and modifiable"). Now the list is
//! `keymap::effective_table` — the two built-in tables in `main.rs` merged
//! with the file. A built-in draws weakly with a "default" tag and writes
//! nothing; edit it and it becomes a File row that SHADOWS the built-in,
//! with a "default: …" hint and a ↺ that removes the file row again.
//!
//! # A row is a cycle
//!
//! A value cell is a strip of chips — one per step, each with × and ‹ ›.
//! Adding to a chord that already has a row APPENDS to its cycle instead
//! of re-aiming it (which is what the old Add box did, and what the owner
//! hit: "adding another u … just overwrites the previous one"). Re-aiming
//! is still one click away: drop the chip, add the new one.
//!
//! # Round-trip is the hard requirement
//!
//! Whatever the tab cannot edit richly still SURVIVES a save: `_`-prefixed
//! comment lanes, entries the loader only complains about, and any value
//! that is not a string or a list of strings render as PLAIN rows whose
//! value box holds raw JSON (the "as text" checkbox puts every row in that
//! mode). A plain row whose edit box is mid-garbage at save time keeps its
//! last VALID value and says so — never dropped, never mangled. Key order
//! after a save is the JSON object's (sorted); the entries' meaning is
//! what must not move, and it doesn't.
//!
//! # What the tab refuses (v1)
//!
//! It never offers a chord keys.json cannot spell — the capture field
//! maps only keys with canonical names (JP-layout OEM keys stay
//! unguessable, the recorded refusal), and says so when pressed. And a
//! handful of built-ins whose list label is not a palette command
//! ("Text / Balloon", "Hand (move)") are listed but frozen: the file has
//! no way to spell "do what that arm does", so the honest offer is to
//! bind the chord to something else, which shadows it.

use crate::app::App;
use crate::keymap::{self, Chord, Entry, Source};
use crate::subtools;

/// The tab's edit state. One instance lives on `App`; rows load the first
/// time the tab is shown and on Reload — the built-in tables always, the
/// keys.json entries merged over them.
#[derive(Default)]
pub(crate) struct State {
    rows: Vec<Row>,
    loaded: bool,
    status: String,
    search: String,
    /// The power-user escape hatch: every value cell as the JSON keys.json
    /// actually holds, instead of chips.
    raw_edit: bool,
    /// The captured chord, exactly as keys.json would spell it.
    cap_text: String,
    cap: Option<Chord>,
    cap_note: String,
}

/// One row of the merged table. `was_string` decides how the value box
/// edits: string values edit as the plain text they are, everything else
/// edits as raw JSON — and `loaded` is what a plain row saves when its
/// edit text does not currently parse.
///
/// A `Source::Default` row is a built-in, drawn weakly and saved by nobody;
/// the moment it is edited it becomes a `File` row and `shadows` remembers
/// what it replaced, so the ↺ can put it back.
struct Row {
    key: String,
    text: String,
    was_string: bool,
    loaded: serde_json::Value,
    source: Source,
    /// Can this row's value go into keys.json as it is? False for the
    /// handful of built-ins whose list label is not a palette command
    /// ("Text / Balloon"): listed, as the owner asked, but frozen.
    spellable: bool,
    shadows: Option<Vec<String>>,
}

impl State {
    /// Re-read keys.json and merge it over the built-in tables. A missing
    /// file is the empty map (the first save writes one); a broken file
    /// keeps the editor empty and shows the whole-file complaint, because
    /// a save from a half-read file would throw away what it could not
    /// read.
    fn reload_from_file(&mut self) {
        self.rows.clear();
        self.status.clear();
        let Some(p) = keymap::keys_file() else {
            self.status = "keys.json has no home beside the exe".into();
            return;
        };
        let Ok(text) = std::fs::read_to_string(&p) else {
            self.status = "no keys.json yet — the first save writes one".into();
            self.set_rows(Vec::new());
            return;
        };
        match keymap::parse_entries(&text) {
            Ok(entries) => self.set_rows(entries),
            Err(e) => self.status = format!("keys.json: {e}"),
        }
    }

    /// The file's entries → the rows the tab draws: the merged table, then
    /// whatever the file holds that is not a chord at all (`_`-comment
    /// lanes, a misspelled modifier). Those still round-trip.
    fn set_rows(&mut self, entries: Vec<Entry>) {
        let merged = keymap::effective_table(&entries);
        self.rows = merged.into_iter().map(row_of_binding).collect();
        self.rows.extend(
            entries
                .into_iter()
                .filter(|e| keymap::parse_chord(&e.key).is_none())
                .map(row_of),
        );
    }

    /// The FILE half of the rows — the built-ins are not the file's to
    /// write. `notes` collects the plain rows whose edit text does not
    /// parse (they keep their last valid value — the round-trip guarantee
    /// — and say so).
    fn file_entries(&self) -> (Vec<Entry>, Vec<String>) {
        let mut notes = Vec::new();
        let entries = self
            .rows
            .iter()
            .filter(|r| r.source == Source::File)
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
        (entries, notes)
    }

    /// The rows back to file text — one save, one rewritten keys.json.
    fn serialize(&self) -> (String, Vec<String>) {
        let (entries, notes) = self.file_entries();
        (keymap::serialize_entries(&entries).unwrap_or_default(), notes)
    }

    /// Re-merge after a STRUCTURAL change (a row deleted, a default
    /// restored): the built-in that was hidden has to come back, and it
    /// only exists in `effective_table`. Never called while typing — it
    /// rebuilds the value boxes from the file's values.
    fn remerge(&mut self) {
        let (entries, _) = self.file_entries();
        self.set_rows(entries);
    }

    /// The save gate for the round-trip guarantee's other half: two rows
    /// edited onto the SAME key (or a key emptied) would collapse in the
    /// JSON object and silently drop a binding. `Some` is the complaint;
    /// the save refuses instead of writing.
    fn key_complaint(&self) -> Option<String> {
        let mut seen = std::collections::BTreeSet::new();
        for r in self.rows.iter().filter(|r| r.source == Source::File) {
            let k = r.key.trim();
            if k.is_empty() {
                return Some("a row's chord is empty — give it a key or delete the row".into());
            }
            if !seen.insert(k) {
                return Some(format!(
                    "two rows share \"{k}\" — a save would drop one; merge or delete first"
                ));
            }
        }
        None
    }
}

fn row_of(e: Entry) -> Row {
    let was_string = e.value.is_string();
    Row {
        key: e.key,
        text: match &e.value {
            serde_json::Value::String(s) => s.clone(),
            v => v.to_string(),
        },
        was_string,
        loaded: e.value,
        source: Source::File,
        spellable: true,
        shadows: None,
    }
}

fn row_of_binding(b: keymap::Binding) -> Row {
    let value = match (b.raw, b.entries.as_slice()) {
        (Some(v), _) => v,
        (None, [one]) => serde_json::Value::String(one.clone()),
        (None, many) => serde_json::Value::Array(
            many.iter()
                .map(|s| serde_json::Value::String(s.clone()))
                .collect(),
        ),
    };
    Row {
        source: b.source,
        spellable: b.spellable,
        shadows: b.shadows,
        ..row_of(Entry { key: b.key, value })
    }
}

/// A row's value as chips: a string is a one-step cycle, an array of
/// strings is the cycle it looks like, anything else has no chips and
/// edits as raw JSON. Derived from the edit text every time, so the chips
/// and the raw box can never drift apart.
fn chips_of(r: &Row) -> Option<Vec<String>> {
    if r.was_string {
        return Some(vec![r.text.clone()]);
    }
    serde_json::from_str::<Vec<String>>(&r.text)
        .ok()
        .filter(|v| !v.is_empty())
}

/// Write a cycle back into a row. A DEFAULT row becomes the user's the
/// moment it is edited, and remembers the built-in it now shadows.
fn rewrite(r: &mut Row, chips: Vec<String>) {
    if r.source == Source::Default {
        r.shadows = chips_of(r);
        r.source = Source::File;
    }
    match chips.as_slice() {
        [one] => {
            r.was_string = true;
            r.text = one.clone();
        }
        many => {
            r.was_string = false;
            r.text = serde_json::to_string(many).unwrap_or_default();
        }
    }
    r.loaded = serde_json::from_str(&r.text)
        .unwrap_or_else(|_| serde_json::Value::String(r.text.clone()));
}

/// A chip's caption: the `tool:` prefix is machinery, not information.
fn chip_label(s: &str) -> &str {
    let t = s.trim();
    match t.len() >= 5 && t[..5].eq_ignore_ascii_case("tool:") {
        true => t[5..].trim_start(),
        false => t,
    }
}

fn cycle_text(entries: &[String]) -> String {
    entries
        .iter()
        .map(|s| chip_label(s))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// What the captured chord already does. Every chord the app answers is a
/// row now — built-ins included — so there is one lookup, not two, and the
/// note says what picking a command will DO: append, never overwrite.
fn conflict_note(chord: Chord, rows: &[Row]) -> Option<String> {
    let r = rows
        .iter()
        .find(|r| keymap::parse_chord(&r.key) == Some(chord))?;
    let what = chips_of(r).map_or_else(|| r.text.clone(), |c| cycle_text(&c));
    Some(match r.source {
        Source::Default => format!("built-in: {what} — picking adds to it"),
        Source::File => format!("already bound: {what} — picking adds to it"),
    })
}

/// Bind `label` to `chord`: a chord that already has a row APPENDS to its
/// cycle (owner 2026-09-05: "adding another u to also have it do tool
/// Figure … just overwrites the previous one"). Re-aiming is still there —
/// it is the chip's × plus this.
fn add_to_chord(st: &mut State, chord: Chord, label: &str) {
    let text = keymap::chord_text(chord.0, chord.1, chord.2, chord.3).unwrap_or_default();
    let Some(i) = st
        .rows
        .iter()
        .position(|r| keymap::parse_chord(&r.key) == Some(chord))
    else {
        st.rows.push(row_of(Entry {
            key: text.clone(),
            value: serde_json::Value::String(label.to_owned()),
        }));
        st.status = format!("\"{text}\" will bind on save");
        return;
    };
    let r = &mut st.rows[i];
    let Some(mut chips) = chips_of(r) else {
        st.status = format!("\"{text}\" holds raw JSON — edit that row as text");
        return;
    };
    // A built-in whose label is not a palette command cannot be repeated in
    // the file, so the new binding shadows it outright — which is exactly
    // what a keys.json line has always done.
    if r.source == Source::Default && !r.spellable {
        let shadowed = chips;
        rewrite(r, vec![label.to_owned()]);
        r.shadows = Some(shadowed);
        st.status = format!("\"{text}\" shadows the built-in");
        return;
    }
    chips.push(label.to_owned());
    let n = chips.len();
    rewrite(r, chips);
    st.status = match n {
        1 => format!("\"{text}\" will bind on save"),
        n => format!("\"{text}\" now cycles {n}"),
    };
}

/// The ↺ on a row that shadows a built-in: drop the file row and let the
/// default come back. A keys.json line is the only thing hiding it, so
/// removing the line IS the restore.
fn restore_default(st: &mut State, i: usize) {
    if i >= st.rows.len() {
        return;
    }
    let key = st.rows[i].key.clone();
    st.rows.remove(i);
    st.remerge();
    st.status = format!("\"{key}\" back to its built-in");
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

/// What a click on a row asked for, applied after the row loop so the
/// rows can be borrowed mutably while they draw.
enum RowAct {
    Delete,
    Restore,
    ChipDrop(usize),
    ChipMove(usize, usize),
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
        ui.checkbox(&mut st.raw_edit, "as text")
            .on_hover_text("every value as the JSON keys.json holds — for the rows chips cannot cover");
    });
    if !st.status.is_empty() {
        ui.label(&st.status);
    }
    ui.weak(
        "Grey rows are the built-in keys; change one and it becomes yours (↺ puts it back). \
         A row with several chips is a cycle: press the key again to step along it.",
    );
    ui.add_space(4.0);
    let raw_edit = st.raw_edit;
    let mut act: Option<(usize, RowAct)> = None;
    egui::ScrollArea::vertical()
        .max_height(200.0)
        .show(ui, |ui| {
            for (i, r) in st.rows.iter_mut().enumerate() {
                ui.horizontal(|ui| row_ui(ui, i, r, raw_edit, &mut act));
            }
        });
    if let Some((i, a)) = act {
        apply_row_act(st, i, a);
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
                                st.cap_note = conflict_note((c, s, a, vk), &st.rows)
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
                            st.cap_note = conflict_note((c, s, a, vk), &st.rows)
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
                Some(c) => conflict_note(c, &st.rows).unwrap_or_else(|| "free".into()),
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
                match keymap::parse_chord(&st.cap_text).or(st.cap) {
                    Some(c) => add_to_chord(st, c, &label),
                    None => st.status = "press (or type) a chord first".into(),
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
            if let Some(complaint) = app.shortcut_edit.key_complaint() {
                app.shortcut_edit.status = complaint;
                return;
            }
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

/// One row: the chord, then its cycle as chips (or raw JSON), then the
/// tag that says where it came from.
fn row_ui(ui: &mut egui::Ui, i: usize, r: &mut Row, raw_edit: bool, act: &mut Option<(usize, RowAct)>) {
    // A built-in the file cannot spell is shown and not edited: binding
    // that chord in the Add box below shadows it, which is the honest door.
    let frozen = r.source == Source::Default && !r.spellable;
    if frozen {
        ui.add_sized(
            [88.0, 18.0],
            egui::Label::new(egui::RichText::new(&r.key).monospace().weak()),
        );
    } else if ui
        .add(
            egui::TextEdit::singleline(&mut r.key)
                .desired_width(88.0)
                .font(egui::TextStyle::Monospace),
        )
        .on_hover_text("the chord, as keys.json spells it")
        .changed()
    {
        r.source = Source::File;
    }
    let chips = (!raw_edit && !frozen).then(|| chips_of(r)).flatten();
    match chips {
        Some(chips) => {
            let n = chips.len();
            for (c, label) in chips.iter().enumerate() {
                if n > 1
                    && ui
                        .add_enabled(c > 0, egui::Button::new("‹").small())
                        .clicked()
                {
                    *act = Some((i, RowAct::ChipMove(c, c - 1)));
                }
                ui.label(chip_label(label));
                if n > 1
                    && ui
                        .add_enabled(c + 1 < n, egui::Button::new("›").small())
                        .clicked()
                {
                    *act = Some((i, RowAct::ChipMove(c, c + 1)));
                }
                if ui
                    .small_button("×")
                    .on_hover_text("drop this from the key")
                    .clicked()
                {
                    *act = Some((i, RowAct::ChipDrop(c)));
                }
            }
        }
        None if frozen => {
            ui.weak(&r.text);
        }
        None => {
            ui.add(
                egui::TextEdit::singleline(&mut r.text)
                    .desired_width(ui.available_width() - 80.0)
                    .font(egui::TextStyle::Monospace),
            )
            .on_hover_text(if r.was_string {
                "a command label or a tool: target — exactly what keys.json reads"
            } else {
                "raw JSON (a cycle array, a comment lane's value) — kept as written"
            });
        }
    }
    match (r.source, &r.shadows) {
        (Source::Default, _) => {
            ui.weak("default").on_hover_text(if frozen {
                "a built-in with no keys.json spelling — bind the chord below to shadow it"
            } else {
                "a built-in; change it and it becomes your own keys.json row"
            });
        }
        (Source::File, Some(d)) => {
            ui.weak(format!("default: {}", cycle_text(d)));
            if ui
                .small_button("↺")
                .on_hover_text("put the built-in back")
                .clicked()
            {
                *act = Some((i, RowAct::Restore));
            }
        }
        (Source::File, None) => {
            if ui
                .small_button("×")
                .on_hover_text("delete this binding")
                .clicked()
            {
                *act = Some((i, RowAct::Delete));
            }
        }
    }
}

fn apply_row_act(st: &mut State, i: usize, a: RowAct) {
    match a {
        RowAct::Delete => {
            st.rows.remove(i);
            st.remerge();
        }
        RowAct::Restore => restore_default(st, i),
        RowAct::ChipDrop(c) => {
            let Some(mut chips) = chips_of(&st.rows[i]) else {
                return;
            };
            chips.remove(c);
            // The last chip gone means the row does nothing: a row that
            // shadows a built-in goes back to the built-in, the rest go.
            if chips.is_empty() {
                restore_default(st, i);
            } else {
                rewrite(&mut st.rows[i], chips);
            }
        }
        RowAct::ChipMove(from, to) => {
            let Some(mut chips) = chips_of(&st.rows[i]) else {
                return;
            };
            if from < chips.len() && to < chips.len() {
                chips.swap(from, to);
                rewrite(&mut st.rows[i], chips);
            }
        }
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

    fn file_row(key: &str, value: &str) -> Row {
        row_of(Entry {
            key: key.to_owned(),
            value: serde_json::Value::String(value.to_owned()),
        })
    }

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
        let mut st = State::default();
        st.set_rows(entries);
        let (saved, notes) = st.serialize();
        assert!(notes.is_empty(), "untouched rows never need notes: {notes:?}");

        // Meaning, both directions: the entries are all still there with
        // the same values, and the loader binds the same chords. The
        // built-in rows the list now shows write nothing.
        let back = keymap::parse_entries(&saved).expect("saved text reparses");
        assert_eq!(back.len(), 7, "nothing dropped, nothing added: {saved}");
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

    /// The conflict lookup names what a chord already does — built-ins
    /// included, since they are rows now — and says that picking a
    /// command ADDS to it rather than replacing it.
    #[test]
    fn the_conflict_lookup_names_both_sides() {
        let mut st = State::default();
        st.set_rows(keymap::parse_entries(r#"{ "ctrl+1": "Snap to rulers" }"#).unwrap());
        let note = conflict_note((true, false, false, 0x31), &st.rows).unwrap();
        assert!(note.contains("already bound: Snap to rulers"), "{note}");
        let note = conflict_note((true, false, false, 0x5A), &st.rows).unwrap();
        assert!(note.contains("built-in: Undo"), "{note}");
        let note = conflict_note((false, false, false, 0x55), &st.rows).unwrap();
        // U's built-in is a CYCLE (Figure → Frame border → Ruler), and the
        // note names the whole of it — that is what "picking adds to it"
        // means.
        assert!(note.contains("built-in: "), "{note}");
        assert!(note.contains("Frame border"), "{note}");
        // Q is bound by nothing, in either table.
        assert!(conflict_note((false, false, false, 0x51), &st.rows).is_none());
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
            applied
                .lookup(false, false, false, 0x51)
                .map(keymap::Bind::steps),
            Some([keymap::Step::Cmd(crate::cmd::AppCmd::Redo)])
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

    /// Two rows edited onto ONE key would collapse in the JSON object and
    /// silently drop a binding — the save refuses instead (the round-trip
    /// guarantee's other half). An emptied key refuses too.
    #[test]
    fn a_duplicate_or_empty_key_refuses_to_save() {
        let dup = State {
            rows: vec![file_row("ctrl+1", "Undo"), file_row("ctrl+1", "Undo")],
            ..Default::default()
        };
        let c = dup.key_complaint().expect("a duplicate complains");
        assert!(c.contains("ctrl+1"), "{c}");
        let empty = State {
            rows: vec![file_row("ctrl+1", "Undo"), file_row("  ", "Undo")],
            ..Default::default()
        };
        assert!(empty.key_complaint().is_some(), "an empty key complains");
        let fine = State {
            rows: vec![file_row("ctrl+1", "Undo"), file_row("ctrl+2", "Undo")],
            ..Default::default()
        };
        assert!(fine.key_complaint().is_none(), "distinct keys save");
    }

    /// The owner's exact complaint: adding a second thing to `U` used to
    /// overwrite the first. It APPENDS now, built-in included, and the
    /// three-step cycle it builds loads back as three steps.
    #[test]
    fn adding_to_a_bound_chord_appends_to_its_cycle() {
        let mut st = State::default();
        st.set_rows(Vec::new()); // no file: the built-ins alone
        let u = (false, false, false, 0x55);
        let row = |st: &State| -> Vec<String> {
            chips_of(st.rows.iter().find(|r| r.key == "u").expect("a U row")).unwrap()
        };
        // U's built-in is itself a cycle since 2026-09-05 (Figure → Frame
        // border → Ruler), so the default is READ rather than spelled: this
        // test is about APPENDING, and hard-coding the table's current
        // contents here is what made it fail the day the table grew.
        let default = row(&st);
        assert!(
            default.contains(&"tool: Frame border".to_owned()),
            "U's built-in, listed: {default:?}"
        );
        let n = default.len();

        add_to_chord(&mut st, u, "tool: Text");
        assert_eq!(row(&st).len(), n + 1);
        assert_eq!(row(&st)[..n], default[..], "the default rides in front");
        assert_eq!(row(&st)[n], "tool: Text");
        assert!(st.status.contains(&format!("cycles {}", n + 1)), "{}", st.status);
        add_to_chord(&mut st, u, "Straight line ruler");
        assert_eq!(row(&st).len(), n + 2, "a command joins the cycle too");
        assert!(st.status.contains(&format!("cycles {}", n + 2)), "{}", st.status);

        // The row is the user's now, and remembers what it shadows.
        let r = st.rows.iter().find(|r| r.key == "u").unwrap();
        assert_eq!(r.source, Source::File);
        assert_eq!(r.shadows.as_deref(), Some(&default[..]));
        // And it survives the save as the cycle the loader reads.
        let (saved, notes) = st.serialize();
        assert!(notes.is_empty(), "{notes:?}");
        let m = keymap::Keymap::parse(&saved);
        assert!(m.problems.is_empty(), "{:?}", m.problems);
        assert_eq!(
            m.lookup(false, false, false, 0x55)
                .map(|b| b.steps().len()),
            Some(n + 2)
        );

        // A free chord still starts a row of its own.
        add_to_chord(&mut st, (false, false, false, 0x51), "Undo");
        assert_eq!(row_text(&st, "q"), "Undo");
    }

    fn row_text(st: &State, key: &str) -> String {
        st.rows
            .iter()
            .find(|r| r.key == key)
            .map(|r| r.text.clone())
            .unwrap_or_default()
    }

    /// ↺ on a row that shadows a built-in: the file row goes, the built-in
    /// comes back into the list, and the save stops mentioning it.
    #[test]
    fn restoring_a_shadowed_default_removes_the_file_row() {
        let mut st = State::default();
        st.set_rows(keymap::parse_entries(r#"{ "u": "tool: Figure" }"#).unwrap());
        let i = st.rows.iter().position(|r| r.key == "u").expect("a U row");
        assert_eq!(st.rows[i].source, Source::File);
        assert!(st.rows[i].shadows.is_some(), "it knows what it hid");
        assert!(st.serialize().0.contains("Figure"));

        restore_default(&mut st, i);
        let r = st.rows.iter().find(|r| r.key == "u").expect("still listed");
        assert_eq!(r.source, Source::Default, "the built-in is back");
        // Whatever U's built-in cycle holds today, ↺ puts THAT back — the
        // point is that the file row is gone, not what the table says.
        assert!(r.text.contains("tool: Frame border"), "{}", r.text);
        assert!(r.shadows.is_none());
        let (saved, _) = st.serialize();
        assert!(!saved.contains("Figure"), "the file row is gone: {saved}");
    }

    /// Dropping the last chip is the same restore, and reordering chips
    /// turns a built-in row into the user's own without losing the hint.
    #[test]
    fn chip_edits_reorder_and_restore() {
        let mut st = State::default();
        st.set_rows(keymap::parse_entries(r#"{ "u": ["tool: Figure", "Undo"] }"#).unwrap());
        let i = st.rows.iter().position(|r| r.key == "u").unwrap();
        apply_row_act(&mut st, i, RowAct::ChipMove(0, 1));
        assert_eq!(chips_of(&st.rows[i]).unwrap(), ["Undo", "tool: Figure"]);
        apply_row_act(&mut st, i, RowAct::ChipDrop(0));
        assert_eq!(chips_of(&st.rows[i]).unwrap(), ["tool: Figure"]);
        apply_row_act(&mut st, i, RowAct::ChipDrop(0));
        let r = st.rows.iter().find(|r| r.key == "u").expect("still listed");
        assert_eq!(r.source, Source::Default, "an empty row is the built-in");
    }
}
