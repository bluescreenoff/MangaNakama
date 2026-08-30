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

/// The Preferences window's tabs, straight from the window's own array —
/// the palette's "Preferences ▸ …" rows open the window on one of these,
/// and the per-setting rows below them come from the same registry the
/// window renders, so neither can drift.
const PREF_SECTIONS: [&str; 7] = super::prefs_dialog::TABS;

/// One searchable entry: what it is called, where it lives (the parenthetical
/// UI-052 shows), and what it runs. Curated — payload commands are named,
/// the rest are the enum's own units.
pub fn command_index() -> Vec<(&'static str, &'static str, AppCmd)> {
    use AppCmd::*;
    vec![
        ("Pen", "Tools (P)", SetTool(Tool::Pen)),
        ("Eraser", "Tools (E)", SetTool(Tool::Eraser)),
        ("Fill", "Tools (G)", SetTool(Tool::Fill)),
        ("Tone (one-click screentone)", "Tools", SetTool(Tool::Tone)),
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
        // The two index-free layer verbs (keymap follow-up (a)). They are
        // here so `keys.json` can NAME them — a palette label is the only
        // handle that file has, and these two aim at the active row rather
        // than at a row the binding chose when it was read.
        (
            "Layer colour on/off",
            "Layer",
            ActiveLayer(crate::cmd::ActiveLayerCmd::ToggleColour),
        ),
        (
            "Clip to layer below",
            "Layer",
            ActiveLayer(crate::cmd::ActiveLayerCmd::ToggleClip),
        ),
        // Follow-up (b): the palette's own door, so Ctrl+K is rebindable
        // like every other chord. Searching for it from inside itself is a
        // no-op, which is the honest behaviour for "open what is open".
        ("Command palette", "Edit (Ctrl+K)", CommandPalette),
        // TRIAGE 109's "Apply to all (create merged layer)": flatten a copy on
        // top, originals untouched. Named for both things people call it, so
        // "flatten" finds it as well as "merge".
        (
            "Merge visible to new layer (flatten a copy)",
            "Layer (Ctrl+Shift+E)",
            StampVisible,
        ),
        // Wave 1's two missing layer commands. Named for what people
        // search: "combine" finds the merge, "ungroup" finds the release.
        // No built-in chord on purpose — they are index-free commands
        // acting on the palette's selection, so `keys.json` is the door
        // (CSP's own Shift+Alt+E / Ctrl+Shift+G, if you want them).
        (
            "Merge selected layers (combine the palette selection)",
            "Layer",
            MergeSelected,
        ),
        (
            "Release folder (ungroup, children step out)",
            "Layer",
            ReleaseFolder,
        ),
        // Row 169: the vector layer's tidy-up window (delete short lines,
        // connect, simplify, adjust width). The four passes take their
        // thresholds from the window, so the palette door is the window.
        (
            "Line correction (simplify, connect, delete short lines)…",
            "Layer",
            LineCorrectOpen,
        ),
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
        // Index-free snap toggles — palette rows so keys.json can bind
        // them (the owner's CSP Ctrl+1 / Ctrl+2 are exactly these).
        ("Snap to rulers", "Layer ▸ Ruler", RulerSnapToggle),
        (
            "Snap to special rulers (parallel, guide, symmetry)",
            "Layer ▸ Ruler",
            RulerSpecialSnapToggle,
        ),
        // Row 109's other half — the Correction menu, whole. Seeds are the
        // menu's own `Adjust` defaults; the parameterised ones open the
        // shared correction dialog, Reverse gradient runs on the spot.
        ("Levels", "Correction", AdjustOpen(mn_core::Adjust::LEVELS)),
        (
            "Tone curve",
            "Correction",
            AdjustOpen(mn_core::Adjust::TONE_CURVE),
        ),
        (
            "Brightness/Contrast",
            "Correction",
            AdjustOpen(mn_core::Adjust::BRIGHTNESS_CONTRAST),
        ),
        (
            "Hue/Saturation/Luminosity",
            "Correction",
            AdjustOpen(mn_core::Adjust::HUE_SATURATION),
        ),
        (
            "Posterization",
            "Correction",
            AdjustOpen(mn_core::Adjust::POSTERIZE),
        ),
        (
            "Colour balance",
            "Correction",
            AdjustOpen(mn_core::Adjust::COLOUR_BALANCE),
        ),
        (
            "Gradient map",
            "Correction",
            AdjustOpen(mn_core::Adjust::GRADIENT_MAP),
        ),
        // Named for both things people call it, like the flatten row above.
        (
            "Reverse gradient (invert colours)",
            "Correction",
            AdjustNow(mn_core::Adjust::Invert),
        ),
        (
            "Binarization",
            "Correction",
            AdjustOpen(mn_core::Adjust::BINARIZE),
        ),
        // Row 105 — correction LAYERS, the live spelling of the rows above.
        (
            "Levels correction layer",
            "Layer ▸ New correction layer",
            NewCorrectionLayer(mn_core::Adjust::LEVELS),
        ),
        (
            "Tone curve correction layer",
            "Layer ▸ New correction layer",
            NewCorrectionLayer(mn_core::Adjust::TONE_CURVE),
        ),
        (
            "Brightness/Contrast correction layer",
            "Layer ▸ New correction layer",
            NewCorrectionLayer(mn_core::Adjust::BRIGHTNESS_CONTRAST),
        ),
        (
            "Hue/Saturation correction layer",
            "Layer ▸ New correction layer",
            NewCorrectionLayer(mn_core::Adjust::HUE_SATURATION),
        ),
        (
            "Posterization correction layer",
            "Layer ▸ New correction layer",
            NewCorrectionLayer(mn_core::Adjust::POSTERIZE),
        ),
        (
            "Colour balance correction layer",
            "Layer ▸ New correction layer",
            NewCorrectionLayer(mn_core::Adjust::COLOUR_BALANCE),
        ),
        (
            "Gradient map correction layer",
            "Layer ▸ New correction layer",
            NewCorrectionLayer(mn_core::Adjust::GRADIENT_MAP),
        ),
        (
            "Invert correction layer (reverse gradient)",
            "Layer ▸ New correction layer",
            NewCorrectionLayer(mn_core::Adjust::Invert),
        ),
        (
            "Binarization correction layer",
            "Layer ▸ New correction layer",
            NewCorrectionLayer(mn_core::Adjust::BINARIZE),
        ),
        (
            "Correction layer settings (edit parameters)",
            "Layer",
            CorrectionEdit,
        ),
        // TRIAGE 109 — the Filter menu, whole. The seeds match the menu's own
        // (`ui::top`); a filter with parameters opens its dialog seeded the
        // same way from either door, and the no-dialog pair runs on the spot.
        ("Blur", "Filter ▸ Blur", FilterApply(mn_core::Filter::Blur)),
        (
            "Blur (strong)",
            "Filter ▸ Blur",
            FilterApply(mn_core::Filter::BlurStrong),
        ),
        (
            "Smoothing",
            "Filter ▸ Blur",
            FilterApply(mn_core::Filter::Smoothing),
        ),
        (
            "Gaussian blur…",
            "Filter ▸ Blur",
            FilterOpen(Some(mn_core::Filter::GAUSSIAN)),
        ),
        (
            "Motion blur…",
            "Filter ▸ Blur",
            FilterOpen(Some(mn_core::Filter::MOTION)),
        ),
        (
            "Radial blur…",
            "Filter ▸ Blur",
            FilterOpen(Some(mn_core::Filter::RADIAL_BLUR)),
        ),
        (
            "Spin blur…",
            "Filter ▸ Blur",
            FilterOpen(Some(mn_core::Filter::SPIN_BLUR)),
        ),
        (
            "Unsharp mask…",
            "Filter ▸ Sharpen",
            FilterOpen(Some(mn_core::Filter::UNSHARP)),
        ),
        (
            "Pinch…",
            "Filter ▸ Distort",
            FilterOpen(Some(mn_core::Filter::PINCH)),
        ),
        (
            "Ripple…",
            "Filter ▸ Distort",
            FilterOpen(Some(mn_core::Filter::RIPPLE)),
        ),
        (
            "Wave…",
            "Filter ▸ Distort",
            FilterOpen(Some(mn_core::Filter::WAVE)),
        ),
        (
            "Twirl…",
            "Filter ▸ Distort",
            FilterOpen(Some(mn_core::Filter::TWIRL)),
        ),
        (
            "Adjust line width…",
            "Filter ▸ Line correction",
            FilterOpen(Some(mn_core::Filter::LINE_WIDTH)),
        ),
        (
            "Remove dust…",
            "Filter ▸ Line correction",
            FilterOpen(Some(mn_core::Filter::REMOVE_DUST)),
        ),
        (
            "Mosaic…",
            "Filter ▸ Effect",
            FilterOpen(Some(mn_core::Filter::MOSAIC)),
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
            "Register selection as brush tip",
            "Edit",
            RegisterBrushFromSelection,
        ),
        (
            "Convert brightness to opacity",
            "Layer",
            BrightnessToOpacity,
        ),
        // Row 166 file objects. The update row is the manual half of
        // `FO-008` (the automatic half is focus regain); relink is
        // `FO-009`, the repair path for a broken link.
        (
            "Import Image as File Object…",
            "File ▸ Import",
            ImportFileObject,
        ),
        ("Update file objects", "File", UpdateFileObjects),
        ("Relink file object…", "File", RelinkFileObject(None)),
        ("Revert to last save", "File", RevertFile),
        ("Export All Pages…", "File", ExportAllPages),
        ("Export Text (script)…", "File", ExportText),
        ("Save", "File (Ctrl+S)", SaveOra),
        ("Save As…", "File (Ctrl+Shift+S)", SaveOraAs),
        // IO-003: the send-it move — a copy on disk, you stay in the file
        // you were in. "Copy" in the label because that is what people
        // search for.
        ("Save Duplicate (a copy, stay in this file)…", "File", SaveDuplicate),
        ("Open…", "File (Ctrl+O)", OpenOra),
        ("New…", "File (Ctrl+N)", NewDoc),
        // The §8 print pair — rows so `keys.json` can NAME them, and the
        // default Ctrl+P lives in the built-in table.
        ("Print…", "File (Ctrl+P)", Print),
        ("Print size (1:1 on paper)", "View", ZoomPrintSize),
        ("Zoom fit", "View (Ctrl+0)", ZoomFit),
        ("Pixel size (100%)", "View (Ctrl+1)", Zoom100),
        ("Flip view", "View (Ctrl+9)", FlipView),
        ("Flip view vertically", "View (Ctrl+Shift+9)", FlipViewV),
        ("Reset rotation", "View", RotateReset),
        ("Reset rotation and flip", "View", RotateFlipReset),
        (
            "Reset view (upright, unmirrored, fitted)",
            "View",
            ViewReset,
        ),
        ("New view of this page", "View", OpenCanvasView),
        ("Hide crop marks and margins", "View", SetGuidesHidden(true)),
        (
            "Show crop marks and margins",
            "View",
            SetGuidesHidden(false),
        ),
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

/// What KIND of thing a row is. Only the sigil filter reads it (`>` for
/// commands, `@` for layers…), so a new row kind that nothing narrows can
/// share an existing kind — but a kind the user can name deserves its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Menu commands, sub tools, brushes, actions, settings — everything
    /// that DOES something rather than naming a thing in the document.
    Command,
    Layer,
    Material,
    Page,
    Manual,
}

/// The VSCode-style prefixes, in the order the hint line prints them. Each
/// narrows the list to one kind; the rest of the query then searches inside
/// it, and a bare sigil ("@" alone) lists that kind whole.
pub const SIGILS: [(char, Kind, &str); 5] = [
    ('>', Kind::Command, "commands"),
    ('@', Kind::Layer, "layers"),
    ('#', Kind::Material, "materials"),
    (':', Kind::Page, "page 12"),
    ('?', Kind::Manual, "manual"),
];

/// The manual's topics (docs/manual/*.html, shipped beside the exe). Kept
/// here as titles rather than parsed out of the HTML: the palette must be
/// searchable with no files on disk, and a missing page says so when it is
/// opened (`cmd::manual_page_path`) instead of vanishing from the search.
const MANUAL_TOPICS: [(&str, &str); 9] = [
    ("Manual — start here", "index.html"),
    ("Comic — pages, spreads, panels", "comic.html"),
    ("Drawing & selections", "drawing.html"),
    ("Files & diagnostics", "files.html"),
    ("Keys — every shortcut", "keys.html"),
    ("Layers & masks", "layers.html"),
    ("Pages & reading order", "pages.html"),
    ("Rulers & perspective", "rulers.html"),
    ("Text & objects", "text.html"),
];

/// One runnable row: what it is called, where it lives (weak text on the
/// right) and the command it pushes. Brush rows carry `SelectBrush` — the
/// very command the Sub Tool list pushes, so a pick made here and a pick
/// made there are the same event.
#[derive(Clone)]
pub struct Entry {
    pub label: String,
    /// Where it lives — the weak right-hand text, and half the haystack.
    /// A `String` because material rows name their own folder.
    pub path: String,
    pub kind: Kind,
    pub cmd: AppCmd,
}

impl Entry {
    fn new(label: impl Into<String>, path: impl Into<String>, kind: Kind, cmd: AppCmd) -> Entry {
        Entry {
            label: label.into(),
            path: path.into(),
            kind,
            cmd,
        }
    }
}

/// Everything the rows are built FROM, borrowed from `App` at open time. A
/// struct rather than eight arguments: the palette keeps growing, and each
/// new kind should cost one field and one `chain`, not a new signature at
/// every call site. Pure input ⇒ the whole index stays testable with no App.
#[derive(Default)]
pub struct PaletteInput<'a> {
    pub presets: &'a [(String, PathBuf)],
    pub actions: &'a [String],
    /// (name, folder label, file, tile) — the Materials palette's own click.
    pub materials: &'a [(String, String, PathBuf, bool)],
    /// The current page's layers, in document order (the index IS the row).
    pub layers: &'a [String],
    pub pages: usize,
    pub recent_files: &'a [PathBuf],
    pub styles: &'a [String],
    pub workspaces: &'a [String],
}

/// Everything the palette can run. `command_index()` first, then the things
/// that only exist in THIS document or THIS install: brushes, sub tools,
/// actions, settings, palettes, materials, layers, pages, recent files, work
/// styles, manual topics and saved workspaces.
///
/// Half of what anyone hunts for is not a menu item — it is a layer he named
/// twenty minutes ago, a tone in the bank, or the page he was on before.
/// Everything index-keyed (layers, pages, actions, materials) is rebuilt
/// each time the overlay OPENS, so a row can never point at a slot that has
/// since moved.
pub fn palette_entries(input: &PaletteInput) -> Vec<Entry> {
    let cmd_row = |label: &str, path: &'static str, cmd: AppCmd| {
        Entry::new(label.to_owned(), path, Kind::Command, cmd)
    };
    command_index()
        .into_iter()
        .map(|(label, path, cmd)| cmd_row(label, path, cmd))
        .chain(
            input
                .presets
                .iter()
                .map(|(name, p)| cmd_row(name, "Sub Tool ▸ Brush", AppCmd::SelectBrush(p.clone()))),
        )
        // Every non-brush sub tool, switching the TOOL as well as the mode —
        // picking "Lasso" from the palette must leave you holding the
        // Selection tool, exactly as clicking that row does.
        .chain(
            SubTool::ALL
                .iter()
                .map(|&s| cmd_row(s.label(), s.path(), AppCmd::SetSubTool(s))),
        )
        .chain(
            input
                .actions
                .iter()
                .enumerate()
                .map(|(i, name)| cmd_row(name, "Auto Action", AppCmd::ActionRun(i))),
        )
        .chain(
            PREF_SECTIONS
                .iter()
                .map(|&s| cmd_row(s, "Preferences", AppCmd::OpenPrefs(Some(s)))),
        )
        // Every individual setting, from the window's own registry — the
        // palette can NAVIGATE to a row ("undo depth" jumps there, lit);
        // inline editing here is explicitly out of scope.
        .chain(
            super::prefs_dialog::PREF_INDEX
                .iter()
                .map(|m| cmd_row(m.label, "Setting", AppCmd::OpenPrefs(Some(m.id)))),
        )
        .chain(super::dock::ALL.iter().map(|&p| {
            cmd_row(
                &format!("{} palette", p.title()),
                "Workspace ▸ Palette",
                AppCmd::PaletteOpen(p),
            )
        }))
        .chain(
            input
                .workspaces
                .iter()
                .map(|n| cmd_row(n, "Workspace ▸ Saved", AppCmd::WorkspaceApply(n.clone()))),
        )
        // The material bank, on the Materials palette's own click command —
        // including its Tile checkbox, so the two paths paste the same way.
        .chain(input.materials.iter().map(|(name, folder, path, tile)| {
            Entry::new(
                name.clone(),
                format!("Material ▸ {folder}"),
                Kind::Material,
                AppCmd::PasteMaterial {
                    path: path.clone(),
                    tile: *tile,
                },
            )
        }))
        .chain(input.layers.iter().enumerate().map(|(i, name)| {
            Entry::new(name.clone(), "Layer", Kind::Layer, AppCmd::SelectLayer(i))
        }))
        // Pages are 1-based everywhere the user can see them.
        .chain((1..=input.pages).map(|n| {
            Entry::new(
                format!("Page {n}"),
                "Pages",
                Kind::Page,
                AppCmd::PageGotoApply(n),
            )
        }))
        .chain(input.recent_files.iter().map(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string());
            Entry::new(
                name,
                "File ▸ Recent",
                Kind::Command,
                AppCmd::OpenOraPath(p.clone()),
            )
        }))
        .chain(
            input
                .styles
                .iter()
                .map(|n| cmd_row(n, "Text style", AppCmd::TextStylePick(n.clone()))),
        )
        .chain(MANUAL_TOPICS.iter().map(|&(title, file)| {
            Entry::new(
                title,
                "Help ▸ Manual",
                Kind::Manual,
                AppCmd::OpenManualPage(file),
            )
        }))
        .collect()
}

/// How well one entry answers `q` (already trimmed and lowercased), lower is
/// better; `None` is "not a match at all". The ladder is deliberate: a
/// prefix beats a word start beats a substring beats the menu path beats a
/// scattered-letter fuzzy hit, so typing `pen` puts the Pen tool above
/// "Perspective ruler" without any per-command tuning.
fn palette_score(e: &Entry, q: &str) -> Option<u32> {
    text_score(&e.label, &e.path, q)
}

/// The ladder above as a pure text function, so a second picker can rank the
/// same way this one does without inventing its own rules (the Auto Action
/// step palette's search box, `ui::actions`). `path` is the secondary field
/// a hit may come from — there, the step's category.
pub(super) fn text_score(label: &str, path: &str, q: &str) -> Option<u32> {
    let label = label.to_lowercase();
    if label.starts_with(q) {
        return Some(0);
    }
    if label.split_whitespace().any(|w| w.starts_with(q)) {
        return Some(1);
    }
    if label.contains(q) {
        return Some(2);
    }
    if path.to_lowercase().contains(q) {
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

/// The prefix hint the overlay prints under an empty query, built FROM the
/// table the filter reads — a hint that can disagree with the behaviour is
/// worse than none.
fn sigil_hint() -> String {
    SIGILS
        .iter()
        .map(|(c, _, what)| format!("{c} {what}"))
        .collect::<Vec<_>>()
        .join("    ")
}

/// A leading sigil and the query behind it. `">"` alone is a filter with an
/// empty query — the whole kind, which is what VSCode does and what makes
/// the prefixes discoverable: press one, see what is in there.
fn split_sigil(q: &str) -> (Option<Kind>, &str) {
    let mut chars = q.chars();
    match chars.next() {
        Some(c) => match SIGILS.iter().find(|(s, _, _)| *s == c) {
            Some(&(_, kind, _)) => (Some(kind), chars.as_str().trim_start()),
            None => (None, q),
        },
        None => (None, q),
    }
}

/// `:12` is a page JUMP, not a search: the digits are matched against the
/// page number itself, exact first, then the numbers that start with them
/// (`:1` offers 1, 10, 11…). Anything non-numeric after the colon falls
/// back to the ordinary text search over "Page 12".
fn page_score(label: &str, q: &str) -> Option<u32> {
    let n = label.strip_prefix("page ")?;
    if n == q {
        Some(0)
    } else if n.starts_with(q) {
        Some(1)
    } else {
        None
    }
}

/// The palette's whole search, as a pure function: indices into `entries`,
/// best first, at most `limit`. Two things are not plain text search. An
/// EMPTY query is the recents list, most recent first, then the index's own
/// order — that is what makes Ctrl+K, Enter a repeat of the last thing you
/// did. And a leading SIGIL narrows to one kind before any scoring happens,
/// so `@` is the layer list of this page and `:7` is page 7.
pub fn palette_filter(
    entries: &[Entry],
    query: &str,
    recents: &[String],
    limit: usize,
) -> Vec<usize> {
    let trimmed = query.trim();
    let (kind, rest) = split_sigil(trimmed);
    let q = rest.trim().to_lowercase();
    let numeric_page =
        kind == Some(Kind::Page) && !q.is_empty() && q.chars().all(|c| c.is_ascii_digit());
    let mut hits: Vec<(u32, usize, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            if kind.is_some_and(|k| k != e.kind) {
                return None;
            }
            let score = if numeric_page {
                page_score(&e.label.to_lowercase(), &q)?
            } else if q.is_empty() {
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

/// Gather the whole index from `App`. ONCE PER OPEN, not per keystroke:
/// with the material bank and a 200-layer page in it this is thousands of
/// rows, and rebuilding it under the typing would put an allocation storm
/// between a key and its letter. It is also the freshness rule — every
/// index-keyed row (layer, page, action, material) is only as old as this
/// press of Ctrl+K.
fn gather_entries(app: &App) -> Vec<Entry> {
    let actions: Vec<String> = app.actions.iter().map(|a| a.name.clone()).collect();
    let materials: Vec<(String, String, PathBuf, bool)> = app
        .materials
        .iter()
        .map(|m| {
            let folder = app
                .material_folder_names
                .get(m.folder)
                .cloned()
                .unwrap_or_else(|| "Materials".to_owned());
            (m.name.clone(), folder, m.path.clone(), app.material_tile)
        })
        .collect();
    let layers: Vec<String> = app.doc.layers.iter().map(|l| l.name.clone()).collect();
    let styles: Vec<String> = app.doc.text_styles.iter().map(|s| s.name.clone()).collect();
    let workspaces: Vec<String> = app.workspaces.iter().map(|e| e[0].clone()).collect();
    palette_entries(&PaletteInput {
        presets: &app.presets,
        actions: &actions,
        materials: &materials,
        layers: &layers,
        pages: app.pages.len(),
        recent_files: &app.recent,
        styles: &styles,
        workspaces: &workspaces,
    })
}

/// Summon the overlay (Ctrl+K, and the docked palette's header button).
pub fn open_command_palette(app: &mut App) {
    app.cmdpal_open = true;
    app.cmdpal_query.clear();
    app.cmdpal_sel = 0;
    app.cmdpal_entries = gather_entries(app);
    app.mark_dirty();
}

fn close_command_palette(app: &mut App) {
    app.cmdpal_open = false;
    app.cmdpal_query.clear();
    app.cmdpal_sel = 0;
    // The bank can be thousands of rows; nothing needs them while closed.
    app.cmdpal_entries = Vec::new();
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

    // Gathered at open (`gather_entries`) and borrowed out for the frame:
    // the drawing closure needs `&mut app` for the query field. Put back
    // below, before anything that can close the overlay.
    let entries = std::mem::take(&mut app.cmdpal_entries);
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
                .fill(theme::c().panel)
                .stroke(egui::Stroke::new(1.0, theme::c().border))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::same(8))
                .shadow(shadow)
                .show(ui, |ui| {
                    ui.set_width(width);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut app.cmdpal_query)
                            .hint_text("Search everything — command, brush, layer, page, material…")
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
                    // The sigils are only discoverable if something says
                    // them, and the empty query is the one moment there is
                    // room — once he is typing, the rows are the answer.
                    if app.cmdpal_query.trim().is_empty() {
                        ui.weak(sigil_hint());
                    }
                    ui.weak("↑ ↓ move   Enter run   Esc close   —   Ctrl+K opens this");
                });
        });
    app.cmdpal_entries = entries;

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
        p.rect_filled(rect, 3.0, theme::c().sel_row);
    } else if resp.hovered() {
        p.rect_filled(rect, 3.0, theme::c().hover);
    }
    let color = if selected {
        theme::c().text_strong
    } else {
        theme::c().text
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
        &e.path,
        egui::FontId::proportional(10.5),
        theme::c().text_weak,
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

    /// Row 109's other half: the Correction menu's nine rows are all
    /// findable, each seeded exactly as the menu seeds it — the dialog
    /// ones open, Reverse gradient runs on the spot.
    #[test]
    fn the_correction_menu_is_in_the_palette() {
        use mn_core::Adjust as A;
        let idx = command_index();
        let row = |label: &str| {
            idx.iter()
                .find(|(l, _, _)| *l == label)
                .unwrap_or_else(|| panic!("no palette row for {label}"))
                .clone()
        };
        for (label, want) in [
            ("Levels", A::LEVELS),
            ("Tone curve", A::TONE_CURVE),
            ("Brightness/Contrast", A::BRIGHTNESS_CONTRAST),
            ("Hue/Saturation/Luminosity", A::HUE_SATURATION),
            ("Posterization", A::POSTERIZE),
            ("Colour balance", A::COLOUR_BALANCE),
            ("Gradient map", A::GRADIENT_MAP),
            ("Binarization", A::BINARIZE),
        ] {
            let (_, wher, cmd) = row(label);
            assert_eq!(wher, "Correction", "menu path ({label})");
            assert!(
                matches!(cmd, AppCmd::AdjustOpen(a) if a == want),
                "{label}: dialog door, menu seed — got {cmd:?}"
            );
        }
        let (_, _, cmd) = row("Reverse gradient (invert colours)");
        assert!(
            matches!(cmd, AppCmd::AdjustNow(A::Invert)),
            "invert runs with no dialog, like the menu: {cmd:?}"
        );
    }

    /// TRIAGE 109: the Filter menu was reachable by mouse only — not one of
    /// its fifteen rows had a palette entry, so "gaussian" found nothing. Each
    /// one-shot runs on the spot, each parameterised one opens its dialog
    /// seeded exactly as the menu seeds it.
    #[test]
    fn the_filter_menu_is_in_the_palette() {
        use mn_core::Filter as F;
        let idx = command_index();
        let row = |label: &str| {
            idx.iter()
                .find(|(l, _, _)| *l == label)
                .unwrap_or_else(|| panic!("no palette row for {label}"))
                .clone()
        };
        // The no-dialog ones apply immediately — a dialog here would be a
        // second click the menu does not ask for.
        for (label, want) in [
            ("Blur", F::Blur),
            ("Blur (strong)", F::BlurStrong),
            ("Smoothing", F::Smoothing),
        ] {
            let (_, wher, cmd) = row(label);
            assert_eq!(wher, "Filter ▸ Blur");
            assert!(
                matches!(cmd, AppCmd::FilterApply(f) if f == want),
                "{label} must apply at once, not {cmd:?}"
            );
        }
        // The parameterised ones open the shared dialog on the menu's seed.
        //
        // These stay written OUT, deliberately, now that the seeds are
        // `Filter`'s own consts: comparing a const against itself would
        // assert nothing. Spelled here, this loop is the byte-pin — the one
        // place that says what the numbers ARE, so a slipped decimal in
        // `filter.rs` fails a test instead of shipping a different dialog.
        for (label, wher, want) in [
            ("Gaussian blur…", "Filter ▸ Blur", F::Gaussian { sigma: 4.0 }),
            (
                "Motion blur…",
                "Filter ▸ Blur",
                F::Motion {
                    angle: 0.0,
                    length: 20.0,
                    dir: mn_core::MotionDir::Both,
                    mode: mn_core::MotionMode::Uniform,
                },
            ),
            (
                "Radial blur…",
                "Filter ▸ Blur",
                F::RadialBlur { strength: 0.3 },
            ),
            ("Spin blur…", "Filter ▸ Blur", F::SpinBlur { angle_deg: 20.0 }),
            (
                "Unsharp mask…",
                "Filter ▸ Sharpen",
                F::Unsharp {
                    radius: 2.0,
                    amount: 1.0,
                },
            ),
            ("Pinch…", "Filter ▸ Distort", F::Pinch { amount: 0.4 }),
            (
                "Ripple…",
                "Filter ▸ Distort",
                F::Ripple {
                    amplitude: 8.0,
                    wavelength: 48.0,
                },
            ),
            (
                "Wave…",
                "Filter ▸ Distort",
                F::Wave {
                    amplitude: 8.0,
                    wavelength: 48.0,
                    dir: mn_core::WaveDir::Horizontal,
                },
            ),
            ("Twirl…", "Filter ▸ Distort", F::Twirl { angle_deg: 90.0 }),
            (
                "Adjust line width…",
                "Filter ▸ Line correction",
                F::LineWidth { delta: 1 },
            ),
            (
                "Remove dust…",
                "Filter ▸ Line correction",
                F::RemoveDust { max_px: 5 },
            ),
            ("Mosaic…", "Filter ▸ Effect", F::Mosaic { cell: 8 }),
        ] {
            let (_, w, cmd) = row(label);
            assert_eq!(w, wher, "{label} keeps its submenu");
            assert!(
                matches!(cmd, AppCmd::FilterOpen(Some(f)) if f == want),
                "{label} must open seeded, not {cmd:?}"
            );
        }
        // …and the family answers its own name in the search.
        let entries = all_entries();
        let blurs = labels(&entries, &palette_filter(&entries, "blur", &[], 20));
        assert!(blurs.len() >= 6, "the blur family is findable {blurs:?}");
    }

    /// TRIAGE 109's "Apply to all (create merged layer)" — flatten a copy on
    /// top. The command existed with a chord and a menu row; the palette is
    /// where anyone who knows neither goes looking, and "flatten" is the word
    /// half of them will type.
    #[test]
    fn merge_visible_is_reachable_by_name() {
        let label = "Merge visible to new layer (flatten a copy)";
        let (_, wher, cmd) = find_entry(label).expect("the stamp-visible row");
        assert!(wher.contains("Ctrl+Shift+E"), "the chord rides along ({wher})");
        assert!(matches!(cmd, AppCmd::StampVisible), "{cmd:?}");
        let entries = all_entries();
        for q in ["flatten", "merge visible"] {
            let hits = labels(&entries, &palette_filter(&entries, q, &[], 5));
            assert_eq!(hits.first().map(String::as_str), Some(label), "{q}: {hits:?}");
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

    /// A document's worth of everything else, so the whole index can be
    /// built with no App: two materials in two folders, three layers, four
    /// pages, a recent file, a work style and a saved workspace.
    fn fake_materials() -> Vec<(String, String, PathBuf, bool)> {
        vec![
            (
                "Dots 10%".to_owned(),
                "Tones".to_owned(),
                PathBuf::from("mat/dots10.png"),
                false,
            ),
            (
                "Brick wall".to_owned(),
                "Backgrounds".to_owned(),
                PathBuf::from("mat/brick.png"),
                false,
            ),
        ]
    }

    fn all_entries() -> Vec<Entry> {
        let presets = fake_presets();
        let actions = fake_actions();
        let materials = fake_materials();
        let layers = ["Rough".to_owned(), "Ink".to_owned(), "Tone 60L".to_owned()];
        let recent = [PathBuf::from(r"C:\work\ch03.mnc")];
        let styles = ["Dialogue".to_owned(), "Thought".to_owned()];
        let workspaces = ["Inking".to_owned()];
        palette_entries(&PaletteInput {
            presets: &presets,
            actions: &actions,
            materials: &materials,
            layers: &layers,
            pages: 4,
            recent_files: &recent,
            styles: &styles,
            workspaces: &workspaces,
        })
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
                assert_eq!(
                    s,
                    crate::cmd::SubTool::Select(crate::cmd::SelectMode::Lasso)
                );
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
        assert!(
            family.len() >= 4,
            "the balloon family is findable {family:?}"
        );
        let magnetic = labels(&entries, &palette_filter(&entries, "magnetic", &[], 5));
        assert_eq!(magnetic, vec!["Magnetic lasso".to_owned()]);
    }

    /// The selection pen and eraser lost their strip cells on 2026-08-23 and
    /// became Selection sub tools. The palette is the door that has to stay
    /// open, or removing the cells would simply have removed the tools —
    /// they were unreachable by search before the fold-in, so this row of
    /// the fix is the whole reason the `SubTool` variants exist.
    #[test]
    fn palette_entries_reach_the_selection_paint_tools() {
        let entries = all_entries();
        for (label, tool) in [
            ("Selection pen", Tool::SelPen),
            ("Erase selection", Tool::SelEraser),
        ] {
            let row = entries
                .iter()
                .find(|e| e.label == label)
                .unwrap_or_else(|| panic!("{label} must be searchable"));
            assert_eq!(row.path, "Sub Tool ▸ Selection", "it files under Selection");
            match row.cmd {
                AppCmd::SetSubTool(s) => assert_eq!(s.tool(), tool, "{label} carries its tool"),
                ref other => panic!("{label} must push SetSubTool, not {other:?}"),
            }
        }
        // Searching the group name lists the whole family — the four shapes
        // plus the two paint rows.
        let family = labels(&entries, &palette_filter(&entries, "selection", &[], 40));
        assert!(
            family.iter().any(|l| l == "Selection pen"),
            "the group search finds it too {family:?}"
        );
    }

    /// The user's own auto actions are runnable from the palette, on the
    /// SAME command the Auto Actions palette's ▶ pushes — index-keyed, so
    /// the rows are built from today's list, not remembered.
    #[test]
    fn palette_entries_carry_the_auto_actions() {
        let entries = all_entries();
        let hits = palette_filter(&entries, "tone a flat", &[], PALETTE_ROWS);
        // The action's own name wins the top row; a long command label whose
        // letters happen to spell the query is a fuzzy straggler below it,
        // which is the ladder working rather than a competitor.
        let named = labels(&entries, &hits);
        assert_eq!(named.first().map(String::as_str), Some("Tone a flat"));
        let row = &entries[hits[0]];
        assert_eq!(row.path, "Auto Action");
        assert!(matches!(row.cmd, AppCmd::ActionRun(0)), "{:?}", row.cmd);
        // The second action keeps its own index — an off-by-one here runs
        // the wrong sequence at the user's layers.
        let second = entries
            .iter()
            .find(|e| e.label == "Panel setup")
            .expect("the second action");
        assert!(
            matches!(second.cmd, AppCmd::ActionRun(1)),
            "{:?}",
            second.cmd
        );
        // No actions recorded: no rows, and nothing else changes.
        let presets = fake_presets();
        let bare = palette_entries(&PaletteInput {
            presets: &presets,
            ..PaletteInput::default()
        });
        assert!(bare.iter().all(|e| e.path != "Auto Action"));
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
        // And each individual setting row jumps to its own registry id —
        // "undo depth" in the palette lands on that row, lit.
        for m in super::super::prefs_dialog::PREF_INDEX {
            let row = entries
                .iter()
                .find(|e| e.label == m.label && e.path == "Setting")
                .unwrap_or_else(|| panic!("Setting ▸ {} has no row", m.label));
            match row.cmd {
                AppCmd::OpenPrefs(Some(s)) => assert_eq!(s, m.id, "jumps to its own row"),
                ref other => panic!("{} must open Preferences, not {other:?}", m.label),
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
            assert_eq!(row.path, "Workspace ▸ Palette");
            match row.cmd {
                AppCmd::PaletteOpen(q) => assert_eq!(q, p, "the row reopens ITS palette"),
                ref other => panic!("{want} must push PaletteOpen, not {other:?}"),
            }
        }
        let hist = labels(
            &entries,
            &palette_filter(&entries, "history palette", &[], 5),
        );
        assert_eq!(hist, vec!["History palette".to_owned()]);
    }

    /// Round 2: the rows that describe THIS document or THIS install —
    /// materials, layers, pages, recent files, work styles, manual topics
    /// and saved workspaces. Each one carries the command its own palette
    /// or menu already runs; a second door that means something slightly
    /// different is the bug this whole file exists to avoid.
    #[test]
    fn palette_entries_carry_the_document_and_the_install() {
        let entries = all_entries();
        let row = |label: &str| {
            entries
                .iter()
                .find(|e| e.label == label)
                .unwrap_or_else(|| panic!("no row labelled {label}"))
                .clone()
        };

        let dots = row("Dots 10%");
        assert_eq!(dots.path, "Material ▸ Tones", "the folder names the row");
        assert_eq!(dots.kind, Kind::Material);
        match &dots.cmd {
            AppCmd::PasteMaterial { path, tile } => {
                assert_eq!(path, &PathBuf::from("mat/dots10.png"));
                assert!(!tile, "the bank's own Tile checkbox rides along");
            }
            other => panic!("a material row must paste, not {other:?}"),
        }

        let ink = row("Ink");
        assert_eq!((ink.path.as_str(), ink.kind), ("Layer", Kind::Layer));
        assert!(matches!(ink.cmd, AppCmd::SelectLayer(1)), "{:?}", ink.cmd);

        let p3 = row("Page 3");
        assert_eq!((p3.path.as_str(), p3.kind), ("Pages", Kind::Page));
        assert!(
            matches!(p3.cmd, AppCmd::PageGotoApply(3)),
            "pages are 1-based where the user can see them ({:?})",
            p3.cmd
        );
        assert!(
            !entries.iter().any(|e| e.label == "Page 5"),
            "four pages means four rows"
        );

        let recent = row("ch03.mnc");
        assert_eq!(recent.path, "File ▸ Recent");
        assert!(
            matches!(&recent.cmd, AppCmd::OpenOraPath(p) if p.ends_with("ch03.mnc")),
            "{:?}",
            recent.cmd
        );

        let style = row("Dialogue");
        assert_eq!(style.path, "Text style");
        assert!(
            matches!(&style.cmd, AppCmd::TextStylePick(n) if n == "Dialogue"),
            "{:?}",
            style.cmd
        );

        let ws = row("Inking");
        assert_eq!(ws.path, "Workspace ▸ Saved");
        assert!(matches!(&ws.cmd, AppCmd::WorkspaceApply(n) if n == "Inking"));

        // Every manual topic is a row, and each names a page that exists in
        // the repository's docs/manual — a title with no file behind it is
        // a row that opens nothing.
        for (title, file) in MANUAL_TOPICS {
            let m = row(title);
            assert_eq!((m.path.as_str(), m.kind), ("Help ▸ Manual", Kind::Manual));
            assert!(matches!(m.cmd, AppCmd::OpenManualPage(f) if f == file));
            let on_disk =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual/");
            assert!(
                on_disk.join(file).exists(),
                "{file} is missing from docs/manual"
            );
        }
    }

    /// The VSCode prefixes. A sigil narrows to ONE kind before any scoring,
    /// a bare sigil is that kind whole (which is how you discover what is in
    /// there), and `:12` is a page jump rather than a text search.
    #[test]
    fn sigils_narrow_the_search_by_kind() {
        let entries = all_entries();
        // "@" alone = this page's layers, in document order.
        let layers = labels(&entries, &palette_filter(&entries, "@", &[], 20));
        assert_eq!(
            layers,
            vec!["Rough".to_owned(), "Ink".to_owned(), "Tone 60L".to_owned()]
        );
        // …and searching inside the kind still works.
        assert_eq!(
            labels(&entries, &palette_filter(&entries, "@ton", &[], 20)),
            vec!["Tone 60L".to_owned()]
        );
        // "#" is the material bank only — "brick" as a plain query could
        // also reach a brush or a command; behind the sigil it cannot.
        let mats = labels(&entries, &palette_filter(&entries, "#", &[], 20));
        assert_eq!(mats.len(), 2, "{mats:?}");
        assert!(
            palette_filter(&entries, "#lasso", &[], 20).is_empty(),
            "a sub tool is not a material, whatever it is called"
        );
        // ">" is commands, and a layer named "Ink" must not answer it.
        let cmds = labels(&entries, &palette_filter(&entries, ">ink", &[], 20));
        assert!(!cmds.contains(&"Ink".to_owned()), "{cmds:?}");
        assert!(
            cmds.iter().all(|l| entries
                .iter()
                .any(|e| e.label == *l && e.kind == Kind::Command)),
            "{cmds:?}"
        );
        // "?" is the manual.
        let help = labels(&entries, &palette_filter(&entries, "?keys", &[], 5));
        assert_eq!(help, vec!["Keys — every shortcut".to_owned()]);
        // A sigil-less query still searches everything.
        let both = labels(&entries, &palette_filter(&entries, "ink", &[], 20));
        assert!(both.contains(&"Ink".to_owned()), "{both:?}");
        assert!(both.contains(&"Rough ink".to_owned()), "{both:?}");
        // The hint line is built from the table the filter reads.
        let hint = sigil_hint();
        for (c, _, what) in SIGILS {
            assert!(hint.contains(c) && hint.contains(what), "{hint}");
        }
    }

    /// `:N` is a jump: the digits match the page NUMBER, exact first, then
    /// the numbers that start with them — the same thing VSCode's `:12`
    /// does with a line, and nothing like a fuzzy search over "Page 12".
    #[test]
    fn the_colon_form_jumps_to_a_page() {
        let many: Vec<String> = Vec::new();
        let entries = palette_entries(&PaletteInput {
            pages: 14,
            layers: &["Page smudge".to_owned()],
            actions: &many,
            ..PaletteInput::default()
        });
        assert_eq!(
            labels(&entries, &palette_filter(&entries, ":3", &[], 10)),
            vec!["Page 3".to_owned()],
            "one page has a 3 in it"
        );
        // ":1" leads with page 1, then 10..14 — the prefix ladder.
        let ones = labels(&entries, &palette_filter(&entries, ":1", &[], 10));
        assert_eq!(ones.first().map(String::as_str), Some("Page 1"), "{ones:?}");
        assert_eq!(ones.len(), 6, "1, 10, 11, 12, 13, 14 ({ones:?})");
        // A layer whose name contains "page" is not a page jump.
        assert!(!ones.contains(&"Page smudge".to_owned()));
        // ":" alone is the page list; a non-number after it is a text
        // search inside the page rows, which finds nothing useful but must
        // not panic or leak other kinds.
        assert_eq!(
            palette_filter(&entries, ":", &[], 20).len(),
            14,
            "a bare colon lists the pages"
        );
        assert!(palette_filter(&entries, ":smudge", &[], 20).is_empty());
        // Out of range says nothing rather than jumping somewhere near.
        assert!(palette_filter(&entries, ":99", &[], 10).is_empty());
    }

    /// Brushes are half the reason the overlay exists: they must be in the
    /// searchable set, findable by name, and run the SAME command the Sub
    /// Tool list pushes — a second brush-picking path would be a second
    /// place to keep in step.
    #[test]
    fn palette_entries_carry_the_brush_presets() {
        let entries = all_entries();
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
        let hits = labels(
            &entries,
            &palette_filter(&entries, "eras", &[], PALETTE_ROWS),
        );
        assert!(hits.contains(&"Eraser".to_owned()), "{hits:?}");
        let rulers = labels(&entries, &palette_filter(&entries, "ruler", &[], 20));
        assert!(
            rulers.len() >= 5,
            "the ruler family is reachable {rulers:?}"
        );
        // A menu path is searchable too — "Ruler" as a *path* fragment.
        let by_path = labels(&entries, &palette_filter(&entries, "layer ▸", &[], 20));
        assert!(!by_path.is_empty(), "menu paths are part of the haystack");
        // Ladder: "pen" is a prefix of "Pen" and only a fuzzy/word hit
        // elsewhere, so the tool wins the top row.
        let pen = labels(
            &entries,
            &palette_filter(&entries, "pen", &[], PALETTE_ROWS),
        );
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
        let hits = labels(
            &entries,
            &palette_filter(&entries, "", &recents, PALETTE_ROWS),
        );
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
