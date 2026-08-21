//! Quick Access (TRIAGE 145, UI-050/052): a searchable palette of every
//! curated tool and command — type to filter, Enter (or click) runs, the
//! menu path shows in parentheses, and the ☆ pins a command into the
//! favorites row above (persisted in ui.txt). v1 deviations: the pin is a
//! button, not CSP's long-press (mouse-first); sets (UI-051), the tile/list
//! view modes (UI-053) and the settings dialog (UI-054) are deferred with
//! reasons — one flat set of pins first.
//!
//! The same index also feeds the **command palette** (Ctrl+K) at the bottom
//! of this file: the docked palette is the one you leave open, the overlay is
//! the one you summon. CSP's own answer to "too many clicks" is a hardware
//! remote; this is ours, and it reaches the brush presets too — half of what
//! anyone hunts for is a sub tool, not a menu item.

use std::path::PathBuf;

use super::theme;
use crate::app::App;
use crate::cmd::{AppCmd, SubTool, Tool};

/// The Preferences window's section headers, in the order it draws them —
/// the palette's "Preferences ▸ …" rows open the window with one of these
/// lit (`ui::dialogs::pref_head`). Renaming a header here without renaming
/// it there only costs the highlight, never the row.
const PREF_SECTIONS: [&str; 6] = [
    "Saving",
    "Drawing",
    "Canvas & view",
    "Text",
    "History",
    "Performance",
];

/// One searchable entry: what it is called, where it lives (the parenthetical
/// UI-052 shows), and what it runs. Curated — payload commands are named,
/// the rest are the enum's own units.
pub fn command_index() -> Vec<(&'static str, &'static str, AppCmd)> {
    use AppCmd::*;
    vec![
        ("Pen", "Tools (P)", SetTool(Tool::Pen)),
        ("Eraser", "Tools (E)", SetTool(Tool::Eraser)),
        ("Fill", "Tools (G)", SetTool(Tool::Fill)),
        ("Auto select (wand)", "Tools (W)", SetTool(Tool::Wand)),
        ("Select", "Tools (M)", SetTool(Tool::Select)),
        ("Object", "Tools (O)", SetTool(Tool::Object)),
        ("Frame border", "Tools (U)", SetTool(Tool::Frame)),
        ("Text / Balloon", "Tools (T)", SetTool(Tool::Text)),
        ("Eyedropper", "Tools (I)", SetTool(Tool::Eyedrop)),
        ("Hand", "Tools (H)", SetTool(Tool::Pan)),
        ("Rotate view", "Tools (R)", SetTool(Tool::Pan)),
        ("Undo", "Edit (Ctrl+Z)", Undo),
        ("Redo", "Edit (Ctrl+Y)", Redo),
        ("Cut", "Edit (Ctrl+X)", Cut),
        ("Copy", "Edit (Ctrl+C)", Copy),
        ("Paste", "Edit (Ctrl+V)", Paste),
        ("Paste to shown position", "Edit (Ctrl+Shift+V)", PasteShown),
        ("Fill with drawing color", "Edit (Alt+Del)", FillSelection),
        ("Clear", "Edit (Del)", ClearLayer),
        ("Clear outside selection", "Edit (Shift+Del)", ClearOutside),
        ("Transform", "Edit (Ctrl+T)", TransformStart),
        (
            "Flip Horizontal",
            "Edit",
            TransformFlip { horizontal: true },
        ),
        ("Flip Vertical", "Edit", TransformFlip { horizontal: false }),
        ("Select all", "Edit (Ctrl+A)", SelectAll),
        ("Deselect", "Edit (Ctrl+D)", Deselect),
        ("Invert selected area", "Edit (Ctrl+Shift+I)", SelectInvert),
        ("Clear undo history", "Edit", ClearHistory),
        ("New layer", "Layer", AddLayer),
        ("New folder", "Layer (Ctrl+G)", AddFolder),
        ("Duplicate layer", "Layer", DuplicateLayer),
        (
            "Straight line ruler",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Line),
        ),
        (
            "Vanishing point ruler",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::VanishingPoint),
        ),
        (
            "Perspective ruler (1-point)",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Perspective1),
        ),
        (
            "Perspective ruler (2-point)",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Perspective),
        ),
        (
            "Perspective ruler (3-point)",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Perspective3),
        ),
        (
            "Curve ruler",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Curve),
        ),
        (
            "Parallel line ruler",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Parallel),
        ),
        (
            "Concentric circle ruler",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Concentric),
        ),
        (
            "Symmetrical ruler",
            "Layer ▸ Ruler",
            RulerArm(crate::cmd::RulerKind::Symmetric),
        ),
        ("First page", "Page (Ctrl+Home)", PageFirst),
        ("Previous page", "Page (Ctrl+PageUp)", PagePrev),
        ("Next page", "Page (Ctrl+PageDown)", PageNext),
        ("Last page", "Page (Ctrl+End)", PageLast),
        ("Go to Page…", "Page", PageGoto),
        ("Add page", "Page", AddPage),
        ("Duplicate page", "Page", DuplicatePage),
        ("Story Editor…", "Page", StoryEditor),
        ("Combine with next page…", "Page", PageCombineSpread),
        ("Split spread…", "Page", PageSplitSpread),
        (
            "Register layer as material",
            "Material palette",
            MaterialRegisterLayer,
        ),
        (
            "Convert brightness to opacity",
            "Layer",
            BrightnessToOpacity,
        ),
        ("Revert to last save", "File", RevertFile),
        ("Export All Pages…", "File", ExportAllPages),
        ("Export Text (script)…", "File", ExportText),
        ("Save", "File (Ctrl+S)", SaveOra),
        ("Save As…", "File (Ctrl+Shift+S)", SaveOraAs),
        ("Open…", "File (Ctrl+O)", OpenOra),
        ("New…", "File (Ctrl+N)", NewDoc),
        ("Zoom fit", "View (Ctrl+0)", ZoomFit),
        ("Pixel size (100%)", "View (Ctrl+1)", Zoom100),
        ("Flip view", "View (Ctrl+9)", FlipView),
        ("Flip view vertically", "View (Ctrl+Shift+9)", FlipViewV),
        ("Reset rotation", "View", RotateReset),
        ("Reset rotation and flip", "View", RotateFlipReset),
        ("Reset view (upright, unmirrored, fitted)", "View", ViewReset),
        ("Hide crop marks and margins", "View", SetGuidesHidden(true)),
        ("Show crop marks and margins", "View", SetGuidesHidden(false)),
        ("Reset transformation", "Transform", TransformReset),
        ("Lock tool settings", "Tool Property", SetToolLock(true)),
        ("Unlock tool settings", "Tool Property", SetToolLock(false)),
    ]
}

/// The palette body: pinned favorites + the search field + live results.
pub fn quick_palette(ui: &mut egui::Ui, app: &mut App) {
    // The overlay's own door, for anyone who never learns the chord.
    if ui
        .small_button("Command palette…  Ctrl+K")
        .on_hover_text("the same search, floating over the canvas — brushes included")
        .clicked()
    {
        open_command_palette(app);
    }
    // Favorites row (UI-050): click runs, ✕ unpins.
    if !app.quick_pins.is_empty() {
        ui.horizontal_wrapped(|ui| {
            let pins = app.quick_pins.clone();
            for key in pins {
                if let Some((label, _where, cmd)) = find_entry(&key) {
                    if ui.small_button(label).clicked() {
                        app.push_cmd(cmd);
                    }
                    // The unpin cross rides the button's hover text.
                    if ui
                        .small_button("✕")
                        .on_hover_text(format!("unpin {label}"))
                        .clicked()
                    {
                        app.quick_pins.retain(|k| k != &key);
                        app.layout.note_quick_pins(&app.quick_pins.join("\u{1f}"));
                    }
                } else {
                    app.quick_pins.retain(|k| k != &key);
                    app.layout.note_quick_pins(&app.quick_pins.join("\u{1f}"));
                }
            }
        });
        ui.separator();
    }
    ui.text_edit_singleline(&mut app.quick_query);
    let q = app.quick_query.trim().to_lowercase();
    let hits: Vec<(usize, &'static str, &'static str)> = if q.is_empty() {
        Vec::new()
    } else {
        command_index()
            .into_iter()
            .enumerate()
            .filter(|(_, (label, wher, _))| {
                label.to_lowercase().contains(&q) || wher.to_lowercase().contains(&q)
            })
            .map(|(i, (label, wher, _))| (i, label, wher))
            .take(12)
            .collect()
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        if q.is_empty() {
            ui.weak("type to search every tool and command");
            return;
        }
        if hits.is_empty() {
            ui.weak("no matches");
            return;
        }
        for (i, label, wher) in hits {
            ui.horizontal(|ui| {
                let row = egui::RichText::new(label).color(egui::Color32::WHITE);
                if ui
                    .add(egui::Button::new(row).fill(egui::Color32::TRANSPARENT))
                    .on_hover_text(wher)
                    .clicked()
                {
                    let (_, _, cmd) = command_index()[i].clone();
                    app.push_cmd(cmd);
                }
                ui.weak(format!("({wher})"));
                let pinned = app.quick_pins.iter().any(|k| k == label);
                let star = if pinned { "★" } else { "☆" };
                if ui
                    .small_button(star)
                    .on_hover_text("pin into Quick Access")
                    .clicked()
                {
                    if pinned {
                        app.quick_pins.retain(|k| k != label);
                    } else {
                        app.quick_pins.push(label.to_string());
                    }
                    app.layout.note_quick_pins(&app.quick_pins.join("\u{1f}"));
                }
            });
        }
    });
}

/// The index entry a pin key refers to (keys are labels; the index is the
/// source of truth — a renamed command simply drops its stale pins).
fn find_entry(key: &str) -> Option<(&'static str, &'static str, AppCmd)> {
    command_index()
        .into_iter()
        .find(|(label, _, _)| *label == key)
}

// --- command palette (Ctrl+K) -------------------------------------------

/// Rows the overlay shows at once. Ten is the whole point of the feature:
/// a list you read, not a list you scroll.
const PALETTE_ROWS: usize = 10;
/// How many labels the session remembers for the empty-query ordering.
const PALETTE_RECENTS: usize = 12;

/// One runnable row: what it is called, where it lives (weak text on the
/// right) and the command it pushes. Brush rows carry `SelectBrush` — the
/// very command the Sub Tool list pushes, so a pick made here and a pick
/// made there are the same event.
#[derive(Clone)]
pub struct Entry {
    pub label: String,
    pub path: &'static str,
    pub cmd: AppCmd,
}

/// Everything the palette can run: `command_index()` first, then one row per
/// brush preset, then the rest of the Sub Tool list, the user's own auto
/// actions, the Preferences sections and the palettes the Workspace menu
/// reopens. Taking the presets and action names as arguments rather than
/// reading `App` is what makes the whole search testable.
///
/// Half of what anyone hunts for is not a menu item: it is a sub tool, an
/// action he recorded last week, or the palette he closed by accident.
pub fn palette_entries(presets: &[(String, PathBuf)], actions: &[String]) -> Vec<Entry> {
    command_index()
        .into_iter()
        .map(|(label, path, cmd)| Entry {
            label: label.to_owned(),
            path,
            cmd,
        })
        .chain(presets.iter().map(|(name, p)| Entry {
            label: name.clone(),
            path: "Sub Tool ▸ Brush",
            cmd: AppCmd::SelectBrush(p.clone()),
        }))
        // Every non-brush sub tool, switching the TOOL as well as the mode —
        // picking "Lasso" from the palette must leave you holding the
        // Selection tool, exactly as clicking that row does.
        .chain(SubTool::ALL.iter().map(|&s| Entry {
            label: s.label().to_owned(),
            path: s.path(),
            cmd: AppCmd::SetSubTool(s),
        }))
        // Auto actions are index-keyed (`ActionRun`), so these rows are built
        // fresh from the list every time the overlay opens — a renamed or
        // deleted action must never leave a row pointing at its old slot.
        .chain(actions.iter().enumerate().map(|(i, name)| Entry {
            label: name.clone(),
            path: "Auto Action",
            cmd: AppCmd::ActionRun(i),
        }))
        .chain(PREF_SECTIONS.iter().map(|&s| Entry {
            label: s.to_owned(),
            path: "Preferences",
            cmd: AppCmd::OpenPrefs(Some(s)),
        }))
        .chain(super::dock::ALL.iter().map(|&p| Entry {
            label: format!("{} palette", p.title()),
            path: "Workspace",
            cmd: AppCmd::PaletteOpen(p),
        }))
        .collect()
}

/// How well one entry answers `q` (already trimmed and lowercased), lower is
/// better; `None` is "not a match at all". The ladder is deliberate: a
/// prefix beats a word start beats a substring beats the menu path beats a
/// scattered-letter fuzzy hit, so typing `pen` puts the Pen tool above
/// "Perspective ruler" without any per-command tuning.
fn palette_score(e: &Entry, q: &str) -> Option<u32> {
    let label = e.label.to_lowercase();
    if label.starts_with(q) {
        return Some(0);
    }
    if label.split_whitespace().any(|w| w.starts_with(q)) {
        return Some(1);
    }
    if label.contains(q) {
        return Some(2);
    }
    if e.path.to_lowercase().contains(q) {
        return Some(3);
    }
    // Fuzzy last resort: the query's letters in order, anywhere ("dupl" or
    // "dpl" both find "Duplicate layer").
    let mut rest = label.chars();
    if q.chars().all(|c| rest.any(|h| h == c)) {
        return Some(4);
    }
    None
}

/// The palette's whole search, as a pure function: indices into `entries`,
/// best first, at most `limit`. An EMPTY query is not "no results" — it is
/// the recents list, most recent first, then the index's own order, which is
/// what makes Ctrl+K, Enter a repeat of the last thing you did.
pub fn palette_filter(
    entries: &[Entry],
    query: &str,
    recents: &[String],
    limit: usize,
) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let mut hits: Vec<(u32, usize, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let score = if q.is_empty() {
                0
            } else {
                palette_score(e, &q)?
            };
            let recent = recents
                .iter()
                .position(|r| *r == e.label)
                .unwrap_or(usize::MAX);
            Some((score, recent, i))
        })
        .collect();
    hits.sort_unstable();
    hits.into_iter().take(limit).map(|(_, _, i)| i).collect()
}

/// Summon the overlay (Ctrl+K, and the docked palette's header button).
pub fn open_command_palette(app: &mut App) {
    app.cmdpal_open = true;
    app.cmdpal_query.clear();
    app.cmdpal_sel = 0;
    app.mark_dirty();
}

fn close_command_palette(app: &mut App) {
    app.cmdpal_open = false;
    app.cmdpal_query.clear();
    app.cmdpal_sel = 0;
    app.mark_dirty();
}

/// The floating overlay itself. Drawn from `ui::build` after the dialogs, so
/// it sits over the canvas and the palettes both.
pub fn command_palette(ctx: &egui::Context, app: &mut App) {
    if !app.cmdpal_open {
        return;
    }
    // The navigation keys are read BEFORE the field is built. A focused
    // `TextEdit` reacts to arrows and Enter but does not drain the frame's
    // event queue, so both halves see the same press — reading them after
    // would work too, but this keeps the decision above the drawing.
    let (up, down, enter, esc) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if esc {
        close_command_palette(app);
        return;
    }

    let action_names: Vec<String> = app.actions.iter().map(|a| a.name.clone()).collect();
    let entries = palette_entries(&app.presets, &action_names);
    let hits = palette_filter(
        &entries,
        &app.cmdpal_query,
        &app.cmdpal_recent,
        PALETTE_ROWS,
    );
    if hits.is_empty() {
        app.cmdpal_sel = 0;
    } else {
        let n = hits.len();
        if down {
            app.cmdpal_sel += 1;
        }
        if up {
            app.cmdpal_sel += n - 1; // wrap backwards without underflowing
        }
        app.cmdpal_sel %= n; // wraps, and re-clamps a selection the filter shortened
    }
    let mut run: Option<Entry> = hits
        .get(app.cmdpal_sel)
        .filter(|_| enter)
        .map(|&i| entries[i].clone());

    let width = (ctx.content_rect().width() * 0.5).clamp(340.0, 560.0);
    egui::Area::new(egui::Id::new("mn.cmdpal"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            let shadow = ui.style().visuals.window_shadow;
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::same(8))
                .shadow(shadow)
                .show(ui, |ui| {
                    ui.set_width(width);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut app.cmdpal_query)
                            .hint_text("Run a command or pick a brush…")
                            .desired_width(f32::INFINITY),
                    );
                    // Focus on open — and back again if a click let it go,
                    // but never stolen from whatever else the user focused.
                    if ctx.memory(|m| m.focused().is_none()) {
                        resp.request_focus();
                    }
                    if resp.changed() {
                        app.cmdpal_sel = 0;
                    }
                    ui.add_space(4.0);
                    if hits.is_empty() {
                        ui.weak("no matches");
                    }
                    for (row, &i) in hits.iter().enumerate() {
                        if palette_row(ui, &entries[i], row == app.cmdpal_sel).clicked() {
                            run = Some(entries[i].clone());
                        }
                    }
                    ui.add_space(4.0);
                    ui.weak("↑ ↓ move   Enter run   Esc close   —   Ctrl+K opens this");
                });
        });

    if let Some(e) = run {
        app.cmdpal_recent.retain(|l| *l != e.label);
        app.cmdpal_recent.insert(0, e.label.clone());
        app.cmdpal_recent.truncate(PALETTE_RECENTS);
        close_command_palette(app);
        // Dispatch, never mutate: the command arms carry the cache doors.
        app.push_cmd(e.cmd);
    }
}

/// One result row: label left, menu path weak on the right.
fn palette_row(ui: &mut egui::Ui, e: &Entry, selected: bool) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 20.0), egui::Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 3.0, theme::SEL_ROW);
    } else if resp.hovered() {
        p.rect_filled(rect, 3.0, theme::HOVER);
    }
    let color = if selected {
        theme::TEXT_STRONG
    } else {
        theme::TEXT
    };
    p.text(
        egui::pos2(rect.left() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &e.label,
        egui::FontId::proportional(12.0),
        color,
    );
    p.text(
        egui::pos2(rect.right() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        e.path,
        egui::FontId::proportional(10.5),
        theme::TEXT_WEAK,
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index is the pin-key space: unique labels, every label
    /// resolves back to its entry, and the 178-vote point works —
    /// "ruler" finds every ruler.
    #[test]
    fn index_is_a_sound_key_space_and_searches() {
        let idx = command_index();
        assert!(
            idx.len() >= 50,
            "a real palette, not a stub ({})",
            idx.len()
        );
        let mut labels: Vec<&str> = idx.iter().map(|(l, _, _)| *l).collect();
        let n = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), n, "labels unique — pin keys are stable");
        for (l, _, _) in idx.iter().take(8) {
            assert!(
                find_entry(l).is_some_and(|(l2, _, _)| l2 == *l),
                "{l} resolves"
            );
        }
        let rulers: Vec<_> = idx
            .iter()
            .filter(|(l, _, _)| l.to_lowercase().contains("ruler"))
            .collect();
        assert!(
            rulers.len() >= 5,
            "the ruler family is findable ({rulers:?})"
        );
        assert!(
            idx.iter()
                .any(|(l, w, _)| *l == "Undo" && w.contains("Edit")),
            "menu paths ride along (UI-052's parenthetical)"
        );
    }

    /// Issue #5: the palette listed line/vanishing-point/curve/parallel/
    /// concentric/symmetric but no perspective ruler at all — the three
    /// kinds the Ruler menu offers were unreachable by search.
    #[test]
    fn the_perspective_family_is_in_the_palette() {
        let idx = command_index();
        for kind in [
            crate::cmd::RulerKind::Perspective1,
            crate::cmd::RulerKind::Perspective,
            crate::cmd::RulerKind::Perspective3,
        ] {
            let hit = idx
                .iter()
                .find(|(_, _, c)| matches!(c, AppCmd::RulerArm(k) if *k == kind));
            let Some((label, wher, _)) = hit else {
                panic!("{kind:?} has no palette row");
            };
            assert!(
                label.to_lowercase().contains("perspective"),
                "typing 'perspective' must find it ({label})"
            );
            assert_eq!(*wher, "Layer ▸ Ruler", "same menu path as its siblings");
        }
    }

    /// Two fake presets, enough to prove the brush half without an App.
    fn fake_presets() -> Vec<(String, PathBuf)> {
        vec![
            ("Kabura pen".to_owned(), PathBuf::from("csp/kabura.myb")),
            ("Rough ink".to_owned(), PathBuf::from("classic/rough.myb")),
        ]
    }

    /// Two named actions, the same shape the Auto Actions palette holds.
    fn fake_actions() -> Vec<String> {
        vec!["Tone a flat".to_owned(), "Panel setup".to_owned()]
    }

    fn all_entries() -> Vec<Entry> {
        palette_entries(&fake_presets(), &fake_actions())
    }

    fn labels(entries: &[Entry], hits: &[usize]) -> Vec<String> {
        hits.iter().map(|&i| entries[i].label.clone()).collect()
    }

    /// The sub tool half beyond the brushes: every row of every tool's Sub
    /// Tool list is reachable, and running one switches the TOOL as well as
    /// the mode — a "Lasso" that left you holding the Fill tool would be a
    /// worse answer than not listing it.
    #[test]
    fn palette_entries_carry_every_sub_tool() {
        let entries = all_entries();
        let lasso = entries
            .iter()
            .find(|e| e.label == "Lasso" && e.path == "Sub Tool ▸ Selection")
            .expect("the Selection tool's Lasso row");
        match lasso.cmd {
            AppCmd::SetSubTool(s) => {
                assert_eq!(s.tool(), Tool::Select, "the pick carries its tool");
                assert_eq!(s, crate::cmd::SubTool::Select(crate::cmd::SelectMode::Lasso));
            }
            ref other => panic!("a sub tool row must push SetSubTool, not {other:?}"),
        }
        // Every tool with a sub tool list is represented, and the group name
        // is searchable on its own ("balloon" lists the balloon family).
        for path in [
            "Sub Tool ▸ Fill",
            "Sub Tool ▸ Auto select",
            "Sub Tool ▸ Selection",
            "Sub Tool ▸ Frame border",
            "Sub Tool ▸ Balloon",
            "Sub Tool ▸ Operation",
            "Sub Tool ▸ Figure",
            "Sub Tool ▸ Gradient",
            "Sub Tool ▸ Eyedropper",
            "Sub Tool ▸ Move",
        ] {
            assert!(entries.iter().any(|e| e.path == path), "{path} has rows");
        }
        let family = labels(&entries, &palette_filter(&entries, "balloon", &[], 20));
        assert!(family.len() >= 4, "the balloon family is findable {family:?}");
        let magnetic = labels(&entries, &palette_filter(&entries, "magnetic", &[], 5));
        assert_eq!(magnetic, vec!["Magnetic lasso".to_owned()]);
    }

    /// The user's own auto actions are runnable from the palette, on the
    /// SAME command the Auto Actions palette's ▶ pushes — index-keyed, so
    /// the rows are built from today's list, not remembered.
    #[test]
    fn palette_entries_carry_the_auto_actions() {
        let entries = all_entries();
        let hits = palette_filter(&entries, "tone a flat", &[], PALETTE_ROWS);
        assert_eq!(labels(&entries, &hits), vec!["Tone a flat".to_owned()]);
        let row = &entries[hits[0]];
        assert_eq!(row.path, "Auto Action");
        assert!(matches!(row.cmd, AppCmd::ActionRun(0)), "{:?}", row.cmd);
        // The second action keeps its own index — an off-by-one here runs
        // the wrong sequence at the user's layers.
        let second = entries
            .iter()
            .find(|e| e.label == "Panel setup")
            .expect("the second action");
        assert!(matches!(second.cmd, AppCmd::ActionRun(1)), "{:?}", second.cmd);
        // No actions recorded: no rows, and nothing else changes.
        let bare = palette_entries(&fake_presets(), &[]);
        assert!(bare.iter().all(|e| e.path != "Auto Action"));
        assert_eq!(bare.len() + 2, entries.len());
    }

    /// Settings and palettes: each Preferences section opens the window ON
    /// itself, and every palette the Workspace menu reopens is reachable by
    /// name — the two things you cannot reach when the palette you need is
    /// the one you closed.
    #[test]
    fn palette_entries_jump_to_settings_and_palettes() {
        let entries = all_entries();
        for sec in PREF_SECTIONS {
            let row = entries
                .iter()
                .find(|e| e.label == sec && e.path == "Preferences")
                .unwrap_or_else(|| panic!("Preferences ▸ {sec} has no row"));
            match row.cmd {
                AppCmd::OpenPrefs(Some(s)) => assert_eq!(s, sec, "opens on its own section"),
                ref other => panic!("{sec} must open Preferences, not {other:?}"),
            }
        }
        // Typing the window's name lists its sections (the path is searched;
        // a fuzzy straggler or two below them is the ladder working).
        let prefs = labels(&entries, &palette_filter(&entries, "preferences", &[], 20));
        assert_eq!(&prefs[..PREF_SECTIONS.len()], &PREF_SECTIONS, "{prefs:?}");
        // Every dockable palette, on the command the Workspace menu runs.
        for p in super::super::dock::ALL {
            let want = format!("{} palette", p.title());
            let row = entries
                .iter()
                .find(|e| e.label == want)
                .unwrap_or_else(|| panic!("{want} has no row"));
            assert_eq!(row.path, "Workspace");
            match row.cmd {
                AppCmd::PaletteOpen(q) => assert_eq!(q, p, "the row reopens ITS palette"),
                ref other => panic!("{want} must push PaletteOpen, not {other:?}"),
            }
        }
        let hist = labels(&entries, &palette_filter(&entries, "history palette", &[], 5));
        assert_eq!(hist, vec!["History palette".to_owned()]);
    }

    /// Brushes are half the reason the overlay exists: they must be in the
    /// searchable set, findable by name, and run the SAME command the Sub
    /// Tool list pushes — a second brush-picking path would be a second
    /// place to keep in step.
    #[test]
    fn palette_entries_carry_the_brush_presets() {
        let presets = fake_presets();
        let entries = palette_entries(&presets, &fake_actions());
        assert!(
            entries.len() > command_index().len(),
            "the presets are appended, not replacing the commands"
        );
        let hits = palette_filter(&entries, "kabura", &[], PALETTE_ROWS);
        assert_eq!(labels(&entries, &hits), vec!["Kabura pen".to_owned()]);
        let brush = &entries[hits[0]];
        assert_eq!(brush.path, "Sub Tool ▸ Brush", "row says where it lives");
        match &brush.cmd {
            AppCmd::SelectBrush(p) => assert_eq!(p, &PathBuf::from("csp/kabura.myb")),
            other => panic!("a brush row must push SelectBrush, not {other:?}"),
        }
    }

    /// Substring and menu-path matching, and the score ladder: an exact
    /// prefix outranks a mid-word hit for the same query.
    #[test]
    fn palette_filter_matches_labels_and_menu_paths() {
        let entries = all_entries();
        let hits = labels(&entries, &palette_filter(&entries, "eras", &[], PALETTE_ROWS));
        assert!(hits.contains(&"Eraser".to_owned()), "{hits:?}");
        let rulers = labels(&entries, &palette_filter(&entries, "ruler", &[], 20));
        assert!(rulers.len() >= 5, "the ruler family is reachable {rulers:?}");
        // A menu path is searchable too — "Ruler" as a *path* fragment.
        let by_path = labels(&entries, &palette_filter(&entries, "layer ▸", &[], 20));
        assert!(!by_path.is_empty(), "menu paths are part of the haystack");
        // Ladder: "pen" is a prefix of "Pen" and only a fuzzy/word hit
        // elsewhere, so the tool wins the top row.
        let pen = labels(&entries, &palette_filter(&entries, "pen", &[], PALETTE_ROWS));
        assert_eq!(pen.first().map(String::as_str), Some("Pen"), "{pen:?}");
        assert!(
            palette_filter(&entries, "zzzznotathing", &[], PALETTE_ROWS).is_empty(),
            "a miss is a miss — no fuzzy match on nonsense"
        );
    }

    /// The empty query is the recents list, most recent first — Ctrl+K then
    /// Enter repeats the last thing you ran. Everything else follows in the
    /// index's own order, so the list is never empty.
    #[test]
    fn palette_filter_leads_with_recents_on_an_empty_query() {
        let entries = all_entries();
        let recents = vec!["Redo".to_owned(), "Kabura pen".to_owned()];
        let hits = labels(&entries, &palette_filter(&entries, "", &recents, PALETTE_ROWS));
        assert_eq!(hits.len(), PALETTE_ROWS, "the empty query still fills rows");
        assert_eq!(&hits[..2], &recents[..], "most recent first, in order");
        assert_eq!(
            hits[2], "Pen",
            "then the index's own order, from the top ({hits:?})"
        );
        // A recent still sorts first inside a filtered query.
        let by_query = labels(
            &entries,
            &palette_filter(&entries, "pen", &recents, PALETTE_ROWS),
        );
        assert_eq!(by_query.first().map(String::as_str), Some("Pen"));
        // Whitespace is not a query.
        assert_eq!(
            labels(&entries, &palette_filter(&entries, "   ", &recents, 2)),
            recents,
            "a field holding only spaces is still the empty query"
        );
    }
}
