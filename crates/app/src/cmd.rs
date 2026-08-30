//! Every user action in one enum, and one place that executes it.
//!
//! # Why the indirection
//!
//! Widgets and shortcuts never mutate state directly: they push an [`AppCmd`]
//! and [`dispatch`] is the single place that executes one. That kept the shell
//! compiling while the engine crates were still being built, and it is still
//! what makes "what does Ctrl+Z actually do" a one-file question.
//!
//! # Dialogs are not opened here
//!
//! `AppCmd::{OpenOra, SaveOra, SaveOraAs, ExportPng}` only *mean* "ask the user
//! for a path". `main::pump_commands` runs the native dialog while **no**
//! `&mut App` is alive (a modal dialog pumps the Win32 message queue, which
//! re-enters the wndproc) and re-issues the `*Path` variant. Opening a dialog
//! from inside `dispatch` would alias `&mut App` — see `main::with_app`.

use std::path::PathBuf;

use mn_brush::{AntiAlias, Interval, MyBrush};
use mn_core::{Balloon, BalloonSet, Blend, FrameSet, ResizeAnchor, Tail};

use crate::app::{App, Engine, EngineKind, PageEntry};
use mn_brush::{CurveDab, DynaDab, GridDab, HairyDab};

mod brush;
mod edit;
mod file_io;
mod frames;
mod history;
mod layers;
mod misc;
mod pages;
mod text;
mod tools;
mod transform;

// The tool enums moved to `cmd::tools` whole; re-exported so every existing
// `crate::cmd::Tool` (and friends) path still resolves.
pub use tools::*;

// Helpers that moved out to a domain module but are still addressed as
// `crate::cmd::…` from elsewhere in the crate.
pub use pages::default_export_stem;
pub use text::{BalloonPatch, TextPatch};
pub use transform::effective_sel_op;
pub(crate) use pages::is_spread_page;
pub(crate) use text::{
    balloons_add, balloons_patch, balloons_remove, texts_add, texts_patch, texts_remove,
};
pub(crate) use transform::{open_layer_transform, pick_color, transform_lift_rect};

// Doors only the test suite addresses as `crate::cmd::…` — every shipping
// caller resolves inside the module that owns the item, so an unconditional
// re-export would be an unused import in a release build.
#[cfg(test)]
pub(crate) use edit::{PasteTarget, open_float_aimed, resolve_paste_target};
#[cfg(test)]
pub(crate) use misc::manual_path;

/// Layer commands with no layer in them — the index is resolved at EXECUTE
/// time from the active row (keymap follow-up (a), 2026-08-29).
///
/// Every layer command carries a `usize`, which is right for a menu built on
/// a row and wrong for a keyboard shortcut: a chord is bound once, at load,
/// when there is no row to name yet. The owner's `Ctrl+B` and `Ctrl+Alt+G`
/// were unbindable for exactly that reason. These variants are the index-free
/// door; they delegate to the indexed commands, so undo, status lines and
/// action recording behave identically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveLayerCmd {
    /// CSP `layerpropuselayercolor` (the owner's Ctrl+B): the active layer's
    /// display colour on ⇄ off, keeping whichever tint it last carried.
    ToggleColour,
    /// CSP `layerchangebelowclip` (the owner's Ctrl+Alt+G).
    ToggleClip,
}

#[derive(Clone, Debug)]
pub enum AppCmd {
    Undo,
    Redo,
    /// Bundle the newest `count` history steps into ONE labelled step
    /// (`Document::wrap_recent`) — the balloon-carry pairing's grouping:
    /// the bubble's commit and the lettering commits it carried land as
    /// one gesture, so one Ctrl+Z takes them back together. A command
    /// (not an inline call) because the commits it wraps ride this same
    /// FIFO queue ahead of it.
    HistoryWrapLast {
        label: String,
        count: usize,
    },
    /// Open the New Manga dialog (an egui window, not a native dialog).
    NewDoc,
    /// One-gesture tiling-pattern authoring: a square wrap-on canvas in a
    /// new tab + the Pattern Studio window (`app/pattern.rs`).
    NewPattern,
    /// Open the Align/Distribute window (TR-040).
    AlignOpen,
    /// Layer ▸ Hide all draft layers (CSP 5.0): a TOGGLE — first press
    /// hides every visible draft layer and remembers them, second press
    /// restores exactly those.
    HideDraftLayers,
    /// Edit ▸ Convert to drawing color (CSP): recolour the layer's ink
    /// to the main colour, coverage kept.
    ConvertToDrawingColor,
    /// Open the Outline selection window.
    OutlineOpen,
    /// Layer ▸ Convert layer… (row 33): rasterize / re-expression /
    /// re-blend / rename, keep-or-replace.
    ConvertOpen,
    /// Layer ▸ Rasterize frame folder (row 32): the frame folder owning
    /// the active layer — border becomes ink, the panel clip becomes
    /// child layer masks, children stay separate. One undo.
    FrameFolderRasterize,
    ConvertLayer {
        rasterize: bool,
        expression: Option<mn_core::doc::LayerExpression>,
        blend: Option<mn_core::Blend>,
        keep_original: bool,
        name: Option<String>,
    },
    /// Edit ▸ Advanced fill… (row 124): fill the selection (or layer)
    /// at an opacity, from the menu.
    AdvancedFillOpen,
    AdvancedFill {
        opacity: f32,
    },
    /// Layer ▸ Extract lines… (row 31): dark pixels → a fresh line
    /// layer above.
    ExtractOpen,
    ExtractLines {
        detection: f32,
    },
    /// Layer ▸ Convert to lines and tones… (row 154, `CL-001`–`014`):
    /// open the parameter window.
    LinesTonesOpen,
    /// …and run it: the active layer becomes a folder of lineart, ベタ and
    /// live tone layers. One structural undo.
    ConvertLinesTones {
        params: Box<mn_core::convert_lt::LinesTonesParams>,
    },
    /// Edit ▸ Outline selection… (CSP): stroke a band around the ants.
    OutlineSelection {
        width: f32,
        border: mn_core::filter::OutlineBorder,
        round: bool,
    },
    /// TR-041/044: align the selected layers — or, when the single
    /// selection is a text layer with 2+ items, its items against each
    /// other (TR-052).
    AlignLayers {
        mode: mn_core::align::AlignMode,
        base: mn_core::align::AlignBase,
    },
    /// TR-042: the chosen edges/centres become equally spaced.
    DistributeLayers {
        mode: mn_core::align::DistributeMode,
    },
    /// TR-043: equal gaps between the targets.
    SpaceLayers {
        mode: mn_core::align::SpacingMode,
    },
    /// Save the pattern canvas into the material bank under the studio's
    /// name.
    PatternSaveMaterial,
    /// Create from `App::new_doc_draft`.
    NewComicCreate,
    // --- pages --------------------------------------------------------------
    SelectPage(usize),
    /// Docking 2 phase 2: open (or focus) a page-view pane for page `i` —
    /// the page beside the canvas, click-to-edit (ui/dock.rs).
    OpenPageInPane(usize),
    /// PM-021: keyboard page navigation — first/last/previous/next.
    PageFirst,
    PageLast,
    PagePrev,
    PageNext,
    /// PM-022: open the Go to Page dialog.
    PageGoto,
    /// PM-022: apply the Go to Page dialog (1-based page number).
    PageGotoApply(usize),
    /// PM-030: combine the current page with the NEXT one into a spread
    /// (opens the dialog: gutter width + delete-empty).
    PageCombineSpread,
    PageCombineApply {
        gap: u32,
        delete_empty: bool,
    },
    /// PM-033: split the current (spread) page back into two pages.
    PageSplitSpread,
    /// PM-040: open the Story Editor (chapter-wide script).
    StoryEditor,
    /// Owner top item (2026-08-18): open the reader.
    ReaderOpen,
    /// Owner top item: return to the reader at the remembered screen.
    ReaderReturn,
    /// CV-003: jump the history to `keep` undo entries (undo/redo as
    /// needed; the History palette's click).
    HistoryTo {
        keep: usize,
    },
    /// EL-002: luminance → alpha on the active layer (scanned lineart).
    BrightnessToOpacity,
    /// TC-004/005/006/011: open the tonal-correction dialog seeded with
    /// this correction's defaults. Live preview until `AdjustApply`.
    AdjustOpen(mn_core::Adjust),
    /// Commit the open correction as one undo step.
    AdjustApply,
    /// Drop the preview and close the dialog.
    AdjustCancel,
    /// TC-007: a correction with no parameters — straight off the menu,
    /// no dialog, as in CSP.
    AdjustNow(mn_core::Adjust),
    /// TRIAGE 138 p1: LM-001 — the all-visible starter mask.
    MaskSelection,
    /// LM-002 — hide everything outside the selection.
    MaskOutsideSelection,
    /// LM-009: flip the active layer's mask link — linked (default) moves
    /// the mask with the layer; unlinked slides the art under a fixed mask.
    MaskLinkToggle,
    /// LM-007 — toggle the mask's effect.
    MaskToggle,
    /// LM-003 — remove the mask entirely.
    MaskDelete,
    /// LM-003 — keep the mask, empty its coverage.
    MaskClear,
    /// LM-006 — bake the mask into the layer pixels (one undo op).
    MaskApply,
    /// LM-008 — tint the masked-off region purple on canvas (toggle).
    MaskShowArea,
    /// TRIAGE 101/102: FL-010/011/013/015/033 — run a blur-family filter on
    /// the active layer as one undo step, clipped to the selection.
    FilterApply(mn_core::Filter),
    /// Open the parameter dialog for a filter, seeded with the value's own
    /// defaults (`None` closes it). The no-dialog one-shots skip this.
    FilterOpen(Option<mn_core::Filter>),
    /// LM-004 — strokes edit the active layer's mask instead of pixels.
    MaskEdit,
    /// TRIAGE 139 v1: apply layer comp `i`.
    CompApply(usize),
    /// LC-005: overwrite comp `i` (the row whose 💾 was clicked) with the
    /// layers' current presentation state — eyes, opacity, blend, layer
    /// colour (`LayerComp::capture`).
    CompSave(usize),
    /// TRIAGE 140 v1: open the speed/focus line generator dialog.
    GenLines,
    /// Apply the generator: one new layer of effect-line ink. Primitives
    /// only (the dialog owns the ergonomics; focus uses center/ri/ro,
    /// speed uses angle/len_min/len_max — a..d per kind).
    /// SF-004/005: reopen the generator dialog with the ACTIVE layer's
    /// stored params (refuses when it is not a generated layer).
    GenLinesEdit,
    GenLinesApply {
        focus: bool,
        a: f32,
        b: f32,
        c: f32,
        d: f32,
        count: u32,
        width: f32,
        jitter: f32,
        seed: u64,
    },
    /// Figure ▸ Stream/Saturated line release: ALWAYS a fresh layer, unlike
    /// `GenLinesApply` whose in-place regen keys on the active layer — the
    /// generated layer becomes active after the first drag, so the dialog
    /// rule would silently overwrite drag 1 with drag 2 (CSP places a new
    /// layer per drag; adjusting an existing one is the Object tool's job).
    ///
    /// Carries the whole spec rather than a field list: the generator has
    /// grown density, jitter, colour and handle attributes, and spelling
    /// each of them out twice is how `kind` came to be droppable by the
    /// dialog's own Apply (see `GenLinesApply`'s carry comment).
    ///
    /// The `Option<usize>` is the panel the gesture started in: `Some`
    /// (a frame-folder index) nests the layer INSIDE it — the coverage
    /// mask eats the protrusions past the border, the printed 集中線
    /// look (owner, 2026-08-24). `None` = the page-level sheet.
    GenLinesPlace(mn_core::genlines::GenLinesSpec, Option<usize>),
    /// The Tool Property editor's commit for an ALREADY PLACED run: the
    /// selected layer's own spec, regenerated in place. Unlike
    /// `GenLinesApply` it names its layer instead of keying on the active
    /// one, because the Object tool can select a run that is not active.
    /// One press per drag — the editor buffers and pushes on release, so
    /// this never runs per frame (regen re-rasterizes the whole layer).
    GenLinesApplyTo {
        layer: usize,
        spec: mn_core::genlines::GenLinesSpec,
    },
    /// CV-004: drop the whole undo history (frees memory, irreversible).
    ClearHistory,
    /// CV-005: reload the last-saved state of the current file.
    RevertFile,
    /// MT-020 raster half: register the active layer (selection-scoped) as
    /// an image material.
    MaterialRegisterLayer,
    /// ROADMAP "brushes without ceremony", capture half: the selection's ink
    /// on the active layer becomes a pure-stamp brush preset (group "mine")
    /// and is selected for immediate tuning.
    RegisterBrushFromSelection,
    /// Row 151's bulk half: copy a folder's images into the bank.
    MaterialImportFolder(PathBuf),
    PageSplitApply {
        gap: u32,
        delete_empty: bool,
    },
    AddPage,
    DeletePage,
    MovePage {
        from: usize,
        to: usize,
    },
    /// Duplicate the current page: its full ORA bytes copied in after itself.
    DuplicatePage,
    /// Import a file (.ora or image) as a NEW page after the current one
    /// (asks for a path first).
    ImportPage,
    ImportPagePath(PathBuf),
    /// Workflow audit #4 (CSP EX's File ▸ Import ▸ Batch import): pick N
    /// images, each becomes the draft underlay of one page (asks for the
    /// files first, then opens the dialog).
    BatchImportPages,
    BatchImportPagesPicked(Vec<PathBuf>),
    /// Run the Batch Import dialog's draft.
    BatchImportApply,
    /// Workflow audit §11, first half: open the "New work from this work…"
    /// dialog (the ネーム promotion's target dpi).
    PromoteNewWork,
    /// Build it — a second work, same pages and order, new dpi, in a new tab.
    PromoteNewWorkApply,
    /// Workflow audit §11, second half: stamp a ネーム work's pages into
    /// this work's pages as fitted draft underlays (asks for the `.mnc`).
    StampNamePages,
    StampNamePagesPath(PathBuf),
    /// Import a Photoshop brush set (.abr): sampled tips become
    /// `imported/` presets + texture masks (TRIAGE 151).
    ImportAbr,
    ImportAbrPath(PathBuf),
    /// Replace the CURRENT page's content with a file (.ora or image).
    ReplacePage,
    ReplacePagePath(PathBuf),
    /// Open the Work Settings dialog (edit story/binding/page geometry
    /// after creation).
    WorkSettings,
    /// Apply the Work Settings draft: story, binding, and page setup.
    /// Geometry changes only affect guides + new pages (existing page
    /// pixels are not resampled).
    WorkSettingsApply,
    /// Open the Change Canvas Size dialog (Edit menu).
    OpenCanvasSize,
    /// Same dialog from Work Settings, seeded with the work's paper size and
    /// the all-pages box already ticked — "make the existing pages match the
    /// settings I just applied".
    OpenPageSize,
    /// Apply the canvas-size draft: new size + the anchor the content pins to.
    ResizeCanvasApply,
    /// Open the Change Work Resolution dialog (`IO-060`, workflow audit
    /// §10). The verb CSP separates from Change Canvas Size: same paper,
    /// every pixel re-made at a new resolution.
    OpenResampleWork,
    /// Apply it — every page of the work, atomically, not undoable.
    ResampleWorkApply,
    /// Crop the canvas to the selection's bounding box (Edit ▸ Crop /
    /// Selection Launcher). Destructive: clears the undo history.
    CropSelection,
    /// Batch export: every page as a numbered full-res PNG into a folder.
    /// Opens the options window (PM-050/051/054/055) — the folder pick
    /// happens on `ExportAllPagesGo`.
    ExportAllPages,
    /// The options window's Export button: ask for the folder.
    ExportAllPagesGo,
    ExportAllPagesPath(PathBuf),
    /// Fill the export window's finishing draft from
    /// `mn_core::export::PRINT_PRESETS[i]`. Out of range is ignored — the
    /// preset list is data, and a stale index must not panic the pump.
    ExportAllPreset(usize),
    /// Open the Preferences window (Edit ▸ Preferences…). The payload is the
    /// section header to point at — the window has no tabs, so "open on the
    /// Performance page" is "open it with that header lit".
    OpenPrefs(Option<&'static str>),
    /// Row 89 (BR-014–016): open the global pen-pressure wizard.
    PenPressureWizardOpen,
    /// The wizard's Apply: replace the global correction curve (empty =
    /// back to the identity) and persist it in prefs — it then bends
    /// every tool's input before any per-tool curve.
    PenPressureCurveSet(Vec<[f32; 2]>),
    /// Workflow audit finding 8, File ▸ Print… — opens the size-policy
    /// pre-dialog; the Windows printer dialog comes on `PrintGo`.
    Print,
    /// The pre-dialog's Print button: composite the active page and hand it
    /// to `PrintDlgW`. Resolved by `main::resolve_dialog`, like every other
    /// command that opens a modal Win32 dialog, and it comes back as
    /// `PrintResult` — a print dialog pumps the message queue and no
    /// `&mut App` may be alive while it does.
    PrintGo,
    /// What the printer pipeline had to say, straight to the status bar.
    PrintResult { msg: String, warn: bool },
    /// Workflow audit finding 8, View ▸ Print size: set the viewport zoom so
    /// one page millimetre is one screen millimetre. A one-shot set, not a
    /// mode — it is a measurement, and the next wheel notch is allowed to
    /// end it.
    ZoomPrintSize,
    /// PM-053: write every text item in the chapter to a `.txt` in
    /// reading order (the translator/letterer handoff).
    ExportText,
    ExportTextPath(PathBuf),
    /// Ask for a path, then open (resolved to `OpenOraPath` by the message loop).
    OpenOra,
    /// Save to the current path, asking only if there is none.
    SaveOra,
    SaveOraAs,
    ExportPng,
    /// Layered PSD export (the studio hand-off; core::psd).
    ExportPsd,
    ExportPsdPath(PathBuf),
    /// Export the whole comic as a single-file `.mnc` (the work folder is the
    /// native format; this is the portable/copy form). Never changes
    /// `doc_path`.
    ExportMnc,
    ExportMncPath(PathBuf),
    /// `IO-003` File ▸ Save Duplicate…: a copy of the work on disk, and you
    /// stay in the original — `doc_path` and the dirty flag never move
    /// (`app/save_duplicate.rs`). Resolved to the `*Path` form by the
    /// message loop, like every other picker command.
    SaveDuplicate,
    SaveDuplicatePath(PathBuf),
    // --- path-resolved forms, issued by `main::pump_commands` --------------
    OpenOraPath(PathBuf),
    SaveOraPath(PathBuf),
    ExportPngPath(PathBuf),
    /// Import an image file as a new layer (asks for a path first).
    ImportImage,
    ImportImagePath(PathBuf),
    /// Same import, landing as a 下書き draft layer — the underlay you draw
    /// over and never print. A separate command (and a separate File-menu
    /// item) because the import route is a bare OS file picker with nowhere
    /// to hang a checkbox; see the note on [`import_image_layer`].
    ImportImageDraft,
    ImportImageDraftPath(PathBuf),
    /// Row 166 `FO-001`: import an image as a FILE OBJECT — a layer that
    /// keeps a link to the file and re-reads it when it changes. Third
    /// import door for the same reason the draft one is a second: the OS
    /// picker has nowhere to hang a checkbox.
    ImportFileObject,
    ImportFileObjectPath(PathBuf),
    /// `FO-008`: re-read every file object on this page whose source
    /// changed. The manual half of the update story (the automatic half is
    /// focus regain, `main.rs`).
    UpdateFileObjects,
    /// `FO-009`: repoint a file object at another file — the repair path
    /// for a broken link. `None` = the active layer.
    RelinkFileObject(Option<usize>),
    RelinkFileObjectPath(usize, PathBuf),
    // --- layers -----------------------------------------------------------
    AddLayer,
    /// Vector inking (docs/VECTOR-INKING.md): a raster layer that RECORDS
    /// its strokes as editable geometry beside the pixels.
    AddVectorLayer,
    /// Batch layer operations (app/batch.rs): open, apply, export.
    BatchOpsOpen,
    /// Recordable actions: replay the palette's action `idx` as one undo
    /// press (`app::actions`).
    ActionRun(usize),
    /// Arm/disarm live recording into action `idx`.
    ActionRecordToggle(usize),
    BatchApply,
    BatchExportPngs,
    BatchExportPngsPath(PathBuf),
    /// Delete the Object tool's selected recorded stroke (Del).
    VectorDelete {
        stroke: usize,
    },
    /// Row 169 (`E-001`…`E-007`, `VL-021`…`VL-027`): open/close Layer ▸ Line
    /// correction….
    LineCorrectOpen,
    /// Run one line-correction pass over the active vector layer's whole
    /// record — one undo press (`app/vector_edit.rs`).
    LineCorrect(crate::app::vector_edit::LineCorrect),
    /// New empty folder above the active layer (CSP layer-palette button).
    AddFolder,
    /// Expand/collapse a folder row in the Layers palette.
    ToggleFolderOpen(usize),
    RemoveLayer,
    DuplicateLayer,
    /// Drag-reorder: move layer `from` (a folder moves with its children) so
    /// its block lands at insertion gap `slot`, at nesting level `depth`.
    MoveLayer {
        from: usize,
        slot: usize,
        depth: u8,
    },
    RenameLayer(usize, String),
    SelectLayer(usize),
    /// TC-013 Ctrl+click on a palette row: toggle it in the multi-selection
    /// (toggling ON moves the editing pen there, like CSP).
    ToggleLayerMulti(usize),
    /// TC-013 Shift+click: multi-select the contiguous range between the
    /// active row and this one; the pen stays put.
    RangeLayerMulti(usize),
    /// LF-002: set a folder Through (children stop isolating) — presentation-
    /// only like visibility, composites live.
    SetFolderThrough(usize, bool),
    SetLayerOpacity(usize, f32),
    SetLayerBlend(usize, Blend),
    SetLayerVisible(usize, bool),
    /// CSP palette-colour label strip; `None` clears it.
    SetLayerLabel(usize, Option<[u8; 3]>),
    /// LP-016: set the layer colour (display tint; None = stock).
    SetLayerColour(usize, Option<[u8; 3]>),
    /// Clip to layer below (CSP クリッピング).
    SetLayerClip(usize, bool),
    /// The same family, aimed at whichever layer is active when it RUNS —
    /// see [`ActiveLayerCmd`]. What makes a layer command bindable to a key.
    ActiveLayer(ActiveLayerCmd),
    /// Ctrl+K's own command (keymap follow-up (b), 2026-08-29): opening the
    /// command palette was a direct `ui::` call in `main.rs`, which made the
    /// one chord in the app that finds every other command the only one
    /// `keys.json` could not move.
    CommandPalette,
    /// Set the ACTIVE layer's screentone params — `Some` converts it into a
    /// tone layer, `None` converts it back. Non-destructive either way (the
    /// painted pixels are the ink source and survive).
    SetTone(Option<mn_core::ToneParams>),
    /// `LP-001` Layer Property ▸ Save as default: bake the ACTIVE layer's
    /// presentation properties as the starting point for newly created
    /// layers of its TYPE. Written straight to `layer_defaults.txt` beside
    /// the exe — see `app::layer_defaults` for what is and is not saved.
    SaveLayerDefaults,
    /// `LP-001`: drop the saved default for the active layer's type, so new
    /// layers of it start stock again.
    ForgetLayerDefaults,
    /// Owner ruling 2026-08-30: whether saving a default for the active
    /// layer's TYPE carries an applied screentone with it. Remembered per
    /// type in `layer_defaults.txt`.
    SetLayerDefaultsIncludeTone(bool),
    /// TN-011 View ▸ Show Tone Area: tint every toned region on the canvas so
    /// leftover scraps of tone are visible before print. A view toggle — it
    /// touches no pixels and is never exported.
    ToneShowArea,
    /// Edit lock.
    SetLayerLock(usize, bool),
    /// Transparent-pixel lock.
    SetLayerLockAlpha(usize, bool),
    /// Mark as THE reference layer (exclusive; what Fill/Wand can refer to).
    SetLayerReference(usize, bool),
    /// RF-001 (owner spec): reference SOLO — clear every other layer's
    /// flag and set this one (the Layers panel's Alt+click).
    SetLayerReferenceSolo(usize),
    /// RF-001: clear the whole reference set.
    ClearReferences,
    // --- rulers (TODO #3) ---------------------------------------------------
    /// Arm the NEXT canvas drag to create this ruler kind (CSP's Layer ▸
    /// Ruler ▸ …-then-draw flow; no dedicated tool).
    RulerArm(RulerKind),
    /// Toggle ruler snapping (rulers stay drawn when off).
    RulerSnapToggle,
    /// Delete every ruler.
    RulerClear,
    /// Mark as a draft layer (excluded from fill refs + export).
    SetLayerDraft(usize, bool),
    /// FB-overflow: art bursts out of the panel — the layer composites
    /// above its frame folder's mask and border ink.
    SetLayerEscape(usize, bool),
    /// FB-overflow part 2: move a breakout layer's insertion marker to the
    /// layer at `.1` (`None` = back to "over my own panel only"). Paint
    /// order is a stack, so this covers everything BELOW that layer too —
    /// core stores the cascade as stable ids.
    SetLayerSpillSeat(usize, Option<usize>),
    /// Part 3 (RL-031): the special-ruler snap veto (parallel/concentric/
    /// guide/symmetric). The master `RulerSnapToggle` still gates all.
    RulerSpecialSnapToggle,
    /// Part 3 (RL-021): cycle the symmetric ruler's line count through the
    /// CSP ladder — applies to existing symmetric rulers AND the default
    /// for the next one created.
    RulerSymmetricCount,
    /// Row 149: bind every ruler to the active layer (Some) or make
    /// them all page-wide again (None). One undo step.
    RulerAttachAll(Option<usize>),
    // --- brush + colour ----------------------------------------------------
    SelectBrush(PathBuf),
    /// ROADMAP "brushes without ceremony", organise half: retitle a preset
    /// the artist owns. Edits the .myb's `"name"` only — the FILE name is
    /// the identity `preset_key` stores per-sub-tool sizes under, so it must
    /// not move.
    RenameBrush {
        path: PathBuf,
        name: String,
    },
    /// Copy a preset to the next free `<prefix>-N.myb` beside it, sharing
    /// the original's tip texture — the "start from this brush" gesture.
    DuplicateBrush(PathBuf),
    /// Brush shape presets (B-009..013) v1: save the SELECTED brush with
    /// its current Tool Property state as a new sub tool in `mine/`.
    BrushSaveCurrent,
    /// Remove a preset's .myb. The texture PNG stays (it can be shared) and
    /// there is no undo, which is why the status line names what went.
    DeleteBrush(PathBuf),
    /// The brush's dab DIAMETER in canvas px, absolute (`SIZE_PX_MIN`..
    /// `SIZE_PX_MAX`). RENAMED from `SetBrushSize`, which carried a 0.25..4
    /// multiplier — same shape of number, different meaning, so the old name
    /// had to go rather than silently read 2× as 2 px.
    SetBrushSizePx(f32),
    /// CSP Stroke ▸ Interval (S-028): the gap between dabs, either as a
    /// percent of tip diameter or as a literal pixel distance.
    SetInterval(Interval),
    /// CSP Adjust brush density by gap (B-029): compensate per-dab alpha for
    /// the dab count so the gap stops deciding the stroke's darkness.
    SetDensityByGap(bool),
    /// CSP Anti-aliasing (A-010): the four-level edge feather.
    SetAntiAlias(AntiAlias),
    /// CSP Ink ▸ Density of paint (I-010), 0..1.
    SetPaintDensity(f32),
    /// CSP Ink ▸ Color stretch (I-011), 0..1.
    SetColorStretch(f32),
    /// CSP Ink ▸ Mixing mode (I-014, triage rows 58 + 167): Standard
    /// (additive) vs Perceptual (spectral pigment). Routes the brush to the
    /// CPU dab path — see `MyBrush::set_color_mixing`.
    SetBrushMix(mn_core::BrushMix),
    /// CSP Image settings ▸ Interpolation method (I-005): the kernel the
    /// transform COMMIT resamples with.
    SetTransformInterp(mn_core::transform::Interp),
    /// CSP Advanced ▸ Watercolor edge (W-001..005, row 71): all four knobs
    /// in one variant — they are one effect, and a rim set half-way from a
    /// previous value is not a state anyone wants to see.
    SetWaterEdge(mn_core::edge::WaterEdge),
    /// CSP Ink ▸ Intensity of blur (I-013): the width, in the unit the
    /// companion `SetBlurAbs` selects.
    SetBlur(f32),
    /// ...and that unit: canvas px (pinned) instead of a multiple of the
    /// brush radius (scales with size).
    SetBlurAbs(bool),
    /// CSP Color jitter (C-010..012): the three amounts and the per-dab /
    /// per-stroke switch, in one variant — they are one row and one
    /// `ColorJitter` value on the engine side.
    SetColorJitter(mn_brush::ColorJitter),
    /// CSP 反転 (B-026/027): the tip's horizontal and vertical flip modes.
    /// Both axes together, for the same reason as the jitter amounts.
    SetTipFlip(mn_brush::TipFlip, mn_brush::TipFlip),
    /// Brush opacity 0..1.
    SetOpacity(f32),
    /// Pressure→size floor, 0..100 %.
    SetMinSize(f32),
    SetStabilizer(f32),
    /// The whole Correction group in one value (`C-027`, `C-029`–`C-033`,
    /// `S-023`–`S-027`). One variant rather than nine: the panel already has
    /// the current `CorrectCfg` in hand, so it sends a modified copy.
    SetCorrection(mn_core::stabilize::CorrectCfg),
    /// Brush-size randomization amount at full pressure (unit per `abs`).
    SetRandomization(f32),
    /// Pressure floor of the randomization, % of the amount.
    SetRandomMin(f32),
    /// Deviation as fixed pixels (true) vs scaling with brush size (false).
    SetRandomAbs(bool),
    /// Krita-style hard stamp dabs on/off (exact AA disc vs gaussian).
    SetHardDab(bool),
    /// Krita Scatter: dab centre jitter as a fraction of the radius.
    SetScatter(f32),
    /// Krita Wash mode on/off (flow-vs-opacity stroke compositing).
    SetWash(bool),
    /// Per-dab alpha inside a wash stroke (Krita: Flow), 0..1.
    SetFlow(f32),
    /// Compositing mode of the wash commit (Krita: per-brush blending).
    SetBrushBlend(Blend),
    /// CSP Ink output row (BM-029..035) - the brush-only commit
    /// behaviours (black/white burn, compare density, background,
    /// replace alpha).
    SetBrushDraw(mn_brush::BrushDraw),
    /// Texture-tip mask by `texture_names` index (0 = none).
    SetTexture(u16),
    /// Texture crawl per dab, mask px.
    SetTextureScroll(f32),
    /// B-031/032: stamped-tip rotation source (fixed / stroke / pen tilt).
    SetTextureRotate(mn_brush::TextureRotate),
    /// Krita SKETCH engine on/off (history-linking filaments).
    SetSketch(bool),
    SetSketchDistance(f32),
    SetSketchDensity(f32),
    /// Replace one setting's response curve for one sensor (Krita per-sensor
    /// curves). `setting`/`sensor` index `CurveSetting`/`CurveSensor`.
    SetCurve {
        setting: u8,
        sensor: u8,
        points: Vec<(f32, f32)>,
    },
    /// Symmetry painting: reflect strokes across the canvas centre's
    /// vertical (X) / horizontal (Y) axis (Krita mirror tools).
    SetMirrorX(bool),
    SetMirrorY(bool),
    /// Wrap-around tiling: dabs near an edge continue on the opposite side
    /// (Krita wrap mode) — seamless border tiling.
    SetWrapX(bool),
    SetWrapY(bool),
    /// In-app GPU dab switch (View menu, persisted as `gpu_dabs=` in
    /// ui.txt — TODO #0.1): strokes rasterize on the compute path where the
    /// brush and adapter allow. Replaces `--gpu-dabs` as the user-facing
    /// switch; the flag stays a startup override for the test bat/harness.
    SetGpuDabs(bool),
    /// LP-022 page half: display the whole canvas as monochrome — view
    /// state, never a composite or export.
    SetMonoPreview(bool),
    /// Set the colour of the active (non-transparent) slot, and record it
    /// in the Recent strip. This is the choke point every colour change
    /// should use — a new colour path gets the history for free.
    SetSlotColor([f32; 3]),
    /// The live half of a continuous colour drag (the wheel, the R/G/B
    /// spinners): sets the slot exactly like `SetSlotColor` but does NOT
    /// touch the history. The release of the drag sends the real
    /// `SetSlotColor`, so a two-second hue sweep leaves ONE history entry
    /// instead of flooding all ten with the same hue. Only reach for this
    /// from a widget that emits a value every frame while held.
    SetSlotColorLive([f32; 3]),
    /// Empty the Recent strip (CSP's Clear color history).
    ClearColorHistory,
    /// Copy the Recent colours that are not already in the Color Set into
    /// it (CSP's Register to color set) — promoting the disposable half to
    /// the kept half, on purpose, which is the only way swatches grow.
    AddHistoryToSwatches,
    SetSlot(Slot),
    /// Append a colour to the Color Set (persisted beside the exe).
    AddSwatch([f32; 3]),
    DeleteSwatch(usize),
    /// Import a GIMP/Krita `.gpl` palette into the Color Set (asks for a
    /// path first).
    ImportPalette,
    ImportPalettePath(PathBuf),
    /// `G-016`: import a gradient into the saved set (asks for a path
    /// first). GIMP `.ggr` — see `mn_core::gradient::import_ggr` for why
    /// that format and not CSP's `.cgs`.
    ImportGradient,
    ImportGradientPath(PathBuf),
    /// Swap main and sub colours (CSP `X`).
    SwapColors,
    /// Main to black, sub to white (CSP `F8`).
    ResetColors,
    SetTool(Tool),
    /// Pick a tool AND the sub tool inside it (the Ctrl+K palette's Sub Tool
    /// rows; see [`SubTool`]).
    SetSubTool(SubTool),
    /// Reopen a closed palette (Workspace menu / the command palette).
    PaletteOpen(crate::ui::dock::Palette),
    /// Owner item 2026-08-19: in the Object tool, pressing its key AGAIN
    /// cycles the selection through the stack under the pick point
    /// (Shift = backward). Selection only — no mutation, no undo.
    ObjectCycle(bool),
    /// The eye solo (RF-001's hover promise, made real r113): Alt+click a
    /// layer's eye hides every other layer; the second Alt+click restores
    /// the snapshot. Presentation state — no undo.
    SetLayerEyeSolo(usize),
    /// Help ▸ Manual: open docs/manual (shipped beside the exe) in the
    /// default browser.
    OpenManual,
    /// One manual TOPIC by file name (`"layers.html"`), for the command
    /// palette's `?` rows — same folder, same browser, one page in.
    OpenManualPage(&'static str),
    /// Apply a registered workspace by name (the Workspace menu's rows).
    WorkspaceApply(String),
    /// Pick a named work style: assign it to the selected text item if there
    /// is one, and make it the new-text default either way. The Tool
    /// Property picker and the command palette both run THIS, so the two
    /// cannot drift into meaning different things by "pick a style".
    TextStylePick(String),
    /// Help ▸ Diagnostics: the F1 HUD's menu twin.
    ToggleHud,
    // --- selection + fill ---------------------------------------------------
    SetSelectMode(SelectMode),
    Deselect,
    /// Ctrl+A.
    SelectAll,
    /// Ctrl+Shift+I — invert the current selection.
    SelectInvert,
    /// Selection Launcher — dilate the selection by px.
    SelectExpand(u32),
    /// SE-007 Selection Launcher — feather the edge by a box blur over
    /// the graduated coverage (the paint/fill weight path; the transform
    /// lift stays boolean per the round-50 record).
    SelectBlur(u32),
    /// Selection Launcher — erode the selection by px.
    SelectShrink(u32),
    /// Ctrl+Shift+D — restore the last deselected selection.
    Reselect,
    /// Alt+Delete — fill the selection (or the whole layer) with the active
    /// colour.
    FillSelection,
    /// Shift+Delete — clear everything outside the selection.
    ClearOutside,
    /// Auto-select wand click at a canvas position.
    MagicSelect(f32, f32, mn_core::SelectionOp),
    /// SE-020: the shrink-select drag's freehand path — seeds a union of
    // floods through the empty space (canvas-space points, subsampled in
    // the arm).
    MagicSelectPath {
        pts: Vec<(f32, f32)>,
        op: mn_core::SelectionOp,
    },
    /// SE-011: select the given layer's alpha (Ctrl+click a layer row),
    /// combined under `op` like every selection gesture.
    SelectFromLayer(usize, mn_core::SelectionOp),
    /// Eyedropper: pick the displayed (or active-layer) colour at a canvas
    /// position into the active slot.
    PickColor(f32, f32),
    /// Bucket fill at a canvas position with the active colour.
    Fill(f32, f32),
    SetFillOpts(mn_core::FillOpts),
    SetWandOpts(mn_core::FillOpts),
    /// Merge the active layer into the one below it (Ctrl+E).
    MergeDown,
    /// Stamp every visible layer onto a new layer above the active one
    /// (CSP Merge visible to new layer, Ctrl+Shift+E).
    StampVisible,
    /// Flatten the palette's multi-selection into one raster layer (CSP
    /// Merge selected layers, Shift+Alt+E). Index-free — it reads
    /// `Document::multi_targets` at dispatch time, so `keys.json` can bind
    /// it like any other chord.
    MergeSelected,
    /// Dissolve the folder the palette is pointing at: children step out,
    /// the header goes (CSP Release folder, Ctrl+Shift+G). Index-free for
    /// the same reason.
    ReleaseFolder,
    /// Move the active-layer cursor up/down the stack (Alt+] / Alt+[).
    LayerAbove,
    LayerBelow,
    // --- frames (koma) ------------------------------------------------------
    /// New frame layer from the page's default/inner border.
    NewFrameLayer,
    /// Divide every frame the drag segment crosses (canvas coords). The
    /// Frame-tool sub tool decides whether the cut spawns a new folder.
    FrameDivide {
        a: (f32, f32),
        b: (f32, f32),
    },
    /// Rectangle-frame sub tool: a new frame border folder from a drag.
    FrameRect {
        a: (f32, f32),
        b: (f32, f32),
    },
    /// Polyline-frame / frame-border-pen close: a new frame border folder
    /// from an arbitrary simple polygon.
    FramePoly {
        points: Vec<[f32; 2]>,
    },
    /// Object-tool edit commit: the layer's frames after a move/reshape.
    FrameCommit {
        layer: usize,
        frames: FrameSet,
    },
    /// Delete one frame polygon (Object tool + Del).
    FrameDelete {
        layer: usize,
        frame: usize,
    },
    /// `FB-030` (TRIAGE 129) "Extend to canvas edge": the panel edge nearest
    /// `at` runs out to the page — or stops flush on the panel facing it,
    /// which closes the gutter between the two. A TAP with a divide sub tool
    /// (CSP puts it on a triangle handle; a tap needs no handle to find).
    FrameExtendEdge {
        at: (f32, f32),
    },
    /// `FB-023`–`025` (TRIAGE 129) "Divide frame border equally": the frame
    /// under the active frame layer becomes `cols` x `rows` equal panels in
    /// one command, gutters from the divide sub tool's own Tool Property.
    /// `fit_to_side` = CSP's *Fit to Side Direction of Frame*.
    FrameDivideEqually {
        cols: usize,
        rows: usize,
        fit_to_side: bool,
    },
    /// `FB-053`/`FB-054` (TRIAGE 127): flip a frame folder between inked
    /// border and **border-as-ruler** — no ink, the outline snaps the pen so
    /// you ink the panel edge yourself with a real brush.
    FrameBorderRuler {
        layer: usize,
    },
    // --- balloons -----------------------------------------------------------
    /// New balloon from a balloon-tool drag. Lands on the active layer when it
    /// is a balloon layer, else on a fresh "Balloon N" layer at the top.
    BalloonAdd {
        balloon: Balloon,
    },
    /// Attach a tail to an existing balloon (Tail sub-mode drag).
    BalloonTailAdd {
        layer: usize,
        balloon: usize,
        tail: Tail,
    },
    /// Object-tool edit commit: the layer's balloons after a move/reshape.
    BalloonCommit {
        layer: usize,
        balloons: BalloonSet,
    },
    /// Delete one balloon (Object tool + Del).
    BalloonDelete {
        layer: usize,
        balloon: usize,
    },
    /// Rows 78/76: delete the Object tool's whole selection (the set
    /// plus the primary), texts and balloons, ONE undo step.
    ObjectMultiDelete,
    /// Rows 76/78's open half: translate every member of the Object
    /// tool's multi-selection (texts, balloons, effect-line runs, whole
    /// frame folders) by whole pixels — ONE undo step for the set.
    ObjectMultiMove {
        dx: i32,
        dy: i32,
    },
    // --- text ---------------------------------------------------------------
    /// Commit a text layer's items (Object-tool move/resize/rotate, or an
    /// editing session's single undo step).
    /// TX-styles: create/update a named work text style and re-style every
    /// current-page item carrying its name (one undo press).
    TextStyleUpsert(mn_core::text::TextStyle),
    /// TX-styles: forget a style; items keep their look, their reference
    /// clears (they become free-styled).
    TextStyleDelete(String),
    /// TX-styles: push the work's styles onto every OTHER page (saved
    /// directly — undo covers the open page only, like batch).
    TextStyleAllPages,
    /// TX-styles: attach (or detach, `None`) a style to one text item,
    /// restyling it to match.
    TextStyleAssign {
        layer: usize,
        item: usize,
        name: Option<String>,
    },
    TextCommit {
        layer: usize,
        texts: mn_core::TextSet,
    },
    /// Delete one text item (Object tool + Del).
    TextDelete {
        layer: usize,
        text: usize,
    },
    // --- tier 3 automation: batch text edits BY STABLE ID ------------------
    // (docs/AUTOMATION.md; archive TODO 2026-08-28.) The typesetting
    // primitives the remote socket speaks: set per-item content, direction
    // and alignment without opening the editor. The layer is still an index
    // (AppCmd convention); the ITEMS inside are addressed by the stable ids
    // the 2026-08-29 round minted, so a batch survives concurrent
    // insertions/deletions. Each command is one `set_texts` commit = one
    // undo press for the whole batch.
    /// Apply per-item field patches by id. Items whose id is absent are
    /// skipped, not an error — the reply's count says how many landed.
    /// (No UI producer yet — the socket calls the door fns directly for
    /// their counts; these variants are the auto-action/step surface,
    /// dead-code-allowed on the `write_tone_spec` precedent, tested via
    /// dispatch in `remote_tests`.)
    #[cfg_attr(not(test), allow(dead_code))]
    TextsPatch {
        layer: usize,
        patches: Vec<TextPatch>,
    },
    /// Append new items (id 0; the commit door mints). Fields not supplied
    /// on the wire follow `story_item_template` — the same defaults a new
    /// field created from the story script gets.
    #[cfg_attr(not(test), allow(dead_code))]
    TextsAdd {
        layer: usize,
        items: Vec<mn_core::TextItem>,
    },
    /// Remove items by id. Absent ids are skipped.
    #[cfg_attr(not(test), allow(dead_code))]
    TextsRemove {
        layer: usize,
        ids: Vec<u64>,
    },
    /// Clear the active layer's pixels (Delete) — selection-clipped when a
    /// selection exists. Vector layers refuse.
    ClearLayer,
    // --- clipboard (TRIAGE 131: CSP's Cut/Copy/Paste split) ----------------
    /// Copy the selection's content (whole layer when nothing is selected)
    /// to the internal clipboard + the OS clipboard as a DIB.
    Copy,
    /// Copy, then clear the lifted region in the same single undo step.
    Cut,
    /// Paste INTO THE PANEL (owner HIGH 2026-08-18): the frame folder
    /// owning the active layer, else the panel under the pointer, else the
    /// selection's bbox, else the old behaviour. The pasted layer is
    /// created inside the target folder so the panel clips it; the float
    /// opens centred on the panel, scaled down to fit.
    Paste,
    /// Paste in place: the pre-HIGH Ctrl+V verbatim (internal clipboard at
    /// its ORIGINAL coordinates; OS DIB at the view centre).
    PasteInPlace,
    /// Paste to shown position: like Paste, but the float is centred on the
    /// current view instead of returning to its source coordinates (the one
    /// you want when the source was another page).
    PasteShown,
    /// FB-035/036/038: combine the target frame folder with the NEXT
    /// sibling frame folder — children pool, frames concat, or the two
    /// adjacent single borders become one (`merge_borders`).
    FrameFoldersCombine {
        merge_borders: bool,
    },
    /// FB-037: wrap the target frame folder and the next sibling in a
    /// new common parent folder — originals survive untouched.
    FrameFoldersGroup,
    /// LC-008: apply the comp to EVERY page whose layer count matches
    /// (the text/no-text chapter case has identical structure per page).
    CompApplyAllPages(usize),
    /// LC-009: for each comp, apply it chapter-wide and export every page
    /// into <dir>/<comp>/ — one image set per version.
    CompExportAll,
    CompExportAllPath(std::path::PathBuf),
    /// New LIVE fill/gradient/tone layer (TRIAGE 137): parameters + a
    /// window mask instead of painted pixels. The current selection cuts
    /// the window; no selection = the whole canvas.
    NewLiveFill(mn_core::FillKind),
    /// Edit a live layer's parameters (Tool Property). Re-derives only —
    /// never structural, no history clear.
    ///
    /// ONE undo step per edit SESSION, not per slider tick: an edit with
    /// no session open records the whole stack as the pre-image, and the
    /// rest of the session only re-derives. See
    /// [`AppCmd::ParamEditSession`].
    SetFillParams(usize, mn_core::FillKind),
    /// The Tool Property panel reporting whether a pointer drag is live on
    /// a live layer's parameters: `Some(layer)` while the button is down,
    /// `None` when it comes up.
    ///
    /// Coalescing is opt-IN, and a drag is the only thing that opts in.
    /// Everything else that sets parameters — a gradient preset click, the
    /// Fill tool's live switch, the Object tool's lattice nudge — is one
    /// finished gesture and records its own step, so nudging the tone
    /// lattice twice takes two presses to unwind.
    ParamEditSession(Option<usize>),
    /// Row 105: new correction LAYER — the params live on the layer, the
    /// corrected page is derived, nothing below is baked. The current
    /// selection cuts the window mask; no selection = the whole canvas.
    NewCorrectionLayer(mn_core::Adjust),
    /// Reopen the ACTIVE correction layer's dialog to edit its params.
    /// Like `SetFillParams`, param edits are re-derives with no undo group
    /// (the live-layer convention).
    CorrectionEdit,
    /// Pick the Fill tool's sub tool (click / enclose / lasso).
    SetFillMode(FillMode),
    /// The one-gesture screentone (Tone tool): flood the enclosed region at
    /// this canvas point and give a new live tone layer that region as its
    /// window. Canvas coordinates.
    ToneRegion(f32, f32),
    /// The Tone tool's whole Tool Property, pushed as one value like
    /// `SetFillOpts` — widgets edit a copy and send it back.
    SetToneOpts(ToneToolOpts),
    /// FI-003 Enclose and fill: the Enclose drag's freehand path
    /// (canvas-space points, subsampled in the arm). Every closed area the
    /// path encloses takes the drawing colour in ONE undo step.
    EncloseFill {
        pts: Vec<(f32, f32)>,
    },
    /// FI-004 Lasso fill: the Lasso drag's freehand path. The shape itself
    /// is painted, boundaries ignored — one undo step.
    LassoFill {
        pts: Vec<(f32, f32)>,
    },
    /// Row 119 / FI-005 Leftover pen: the scrub's freehand path. Only the
    /// enclosed pockets under it that are STILL EMPTY fill; finished colour
    /// is never repainted. One undo step, like its two siblings.
    LeftoverFill {
        pts: Vec<(f32, f32)>,
    },
    /// Row 160 / RD-001: the Remove-dust drag's freehand path. The path
    /// closes into the WINDOW (intersected with any live selection) and
    /// every blob under the threshold inside it is cleaned — or selected,
    /// per `DustOpts::select`. One undo step.
    DustScrub {
        pts: Vec<(f32, f32)>,
    },
    /// The Remove-dust sub tool's whole Tool Property, pushed as one value
    /// like `SetToneOpts`.
    SetDustOpts(DustOpts),
    // --- materials (TRIAGE 133, part 1) ------------------------------------
    /// Paste an image material as the move/scale float, at natural size,
    /// centred on the view. `tile` covers the WHOLE canvas in N×N copies as
    /// one float instead (the owner's tiling ask — a mask to draw through).
    PasteMaterial {
        path: std::path::PathBuf,
        tile: bool,
    },
    /// Add a material folder (rfd pick happens in main.rs; the chosen path
    /// arrives here). Persists in ui.txt; the shipped starter folder is
    /// always index 0 and never persisted.
    MaterialAddFolder(std::path::PathBuf),
    /// Rescan every material folder.
    MaterialRescan,
    /// MT-012: set one material's tags (comma separated, `""` clears them).
    /// Writes the folder's `tags.txt` sidecar and refreshes the bank in
    /// place — no rescan, no restart.
    MaterialSetTags {
        path: std::path::PathBuf,
        tags: String,
    },
    // --- transform (Edit ▸ Transform: scale/rotate, Enter commits) ---------
    /// Begin a transform: lift the selection (or whole layer content) into a
    /// live floating source with a bounding box overlay.
    TransformStart,
    /// Row 53: Edit ▸ Transform ▸ Mesh — lift like Transform, then bend
    /// an n×n lattice; commit resamples through the deformed quads.
    TransformMeshStart,
    /// Row 54: Edit ▸ Transform ▸ Puppet Warp — the same lattice, but
    /// driven by PINS: click to drop one, drag to pull, Alt+click to
    /// remove; the lattice follows the pins and commit resamples.
    TransformPuppetStart,
    /// Commit the pending transform as ONE undo step.
    TransformCommit,
    /// Cancel: drop the floating source, nothing changes.
    TransformCancel,
    /// Update the pending transform with absolute params — the same math
    /// the drag gestures apply through `TransformDrag::set_params`. Used by
    /// the Tool Property numeric fields (TR-031–033); pointer gestures
    /// bypass the queue.
    TransformUpdate {
        sx: f32,
        sy: f32,
        rad: f32,
        tx: f32,
        ty: f32,
    },
    /// TR-019/T-021: flip horizontally/vertically about the pivot — a
    /// button during an active transform, or a standalone Edit ▸ Flip that
    /// lifts, mirrors and commits in one undo step.
    TransformFlip {
        horizontal: bool,
    },
    /// TR-003: set (`Some`) or reset to the source centre (`None`) the
    /// active transform's reference point.
    TransformSetPivot {
        pivot: Option<[f32; 2]>,
    },
    /// Entry-taper parameters (Sub Tool Detail).
    SetTaper {
        px: f32,
        min: f32,
    },
    /// Timer-driven safety save to `<file>.autosave.mnc` (or %TEMP%).
    Autosave,
    // --- view ---------------------------------------------------------------
    ZoomFit,
    Zoom100,
    /// Multiply zoom by the factor, anchored on the canvas-area centre.
    ZoomStep(f32),
    /// Rotate the view by the delta (radians), anchored on the centre.
    RotateView(f32),
    RotateReset,
    /// CV-035, the second of the three view resets: rotation AND mirror
    /// back to normal in one step, the view otherwise left alone (zoom and
    /// pan survive). The Navigator's three-finger tap runs through here.
    RotateFlipReset,
    /// CV-035, the third: the whole view back to how a page opens —
    /// upright, unmirrored, fitted. Order matters, so the fit runs LAST:
    /// `fit_to_view_sized` deliberately carries a mirror through a fit, and
    /// clearing the flip first is what makes the two fit paths agree.
    ViewReset,
    /// CV-021 (CSP's Window ▸ Canvas ▸ New Window), as a PANE: open — or
    /// focus — the SECOND live view of this page, with its own zoom and
    /// pan. View-only; the Canvas pane stays the one drawing surface
    /// (docs/DOCKING-2.md, and `Shell::owns_pointer` routes the pen by one
    /// canvas rect).
    OpenCanvasView,
    /// CV-041: hide the manuscript crop marks and margins WITHOUT deleting
    /// them. Unlike the Tab hides this one persists (`guides_hidden=` in
    /// ui.txt) — it is a workspace preference, not a panic button.
    SetGuidesHidden(bool),
    /// T-020: put the active transform back to the state it started from
    /// while STAYING in it — the identity params, so the float sits exactly
    /// where it was lifted. The reference point is a setting, not part of
    /// the transformation, and is left where the user put it.
    TransformReset,
    /// TL-013: pin (or release) the active sub tool's Tool Property values.
    /// Locking snapshots what is on the sliders now; unlocking makes the
    /// current values the sub tool's own.
    SetToolLock(bool),
    /// Mirror the view horizontally (the drawing-error check).
    FlipView,
    /// The same check about the other axis: flip the view vertically.
    /// Composes with [`AppCmd::FlipView`] — both on is a 180° turn.
    FlipViewV,
    // --- per-layer effects (TRIAGE 21/27/30) --------------------------------
    /// `LP-002`/`LP-003` border effect on the layer: `Some` grows an outline
    /// around the layer's own alpha, `None` removes it. Non-destructive both
    /// ways, one undo step per change.
    SetEdge(usize, Option<mn_core::EdgeParams>),
    /// `LP-017` two-tone SUB colour (the white end of the layer-colour ramp);
    /// `None` leaves the white end white. Presentation-only, like
    /// [`AppCmd::SetLayerColour`].
    SetLayerSubColour(usize, Option<[u8; 3]>),
    /// `LP-022` decrease-colour PREVIEW: display the layer as grey or 1-bit
    /// mono without converting a pixel. Screen only — never exported.
    SetLayerExpression(usize, mn_core::LayerExpression),
    /// Blend If: set (or clear, with `None`) the layer's gate — it shows only
    /// where the pixel it measures is in range. Which pixel (the composite
    /// BELOW it, or its own ink) and which value (brightness or one channel)
    /// are the gate's own two arms.
    ///
    /// Unlike the layer colour and the expression preview beside it, this is
    /// NOT display-only: it changes the exported page, so it is undoable.
    /// The property panel's bars fire every frame, so they coalesce through
    /// [`AppCmd::ParamEditSession`] exactly like the live-layer sliders —
    /// one press per drag. Folders are refused.
    SetLayerBlendIf(usize, Option<mn_core::BlendIf>),
    // --- paper (PA-001, TRIAGE 100 / OL-005) --------------------------------
    /// Toggle the paper's eye. View state, like a layer's eye: no undo, and
    /// no effect on what a PNG export writes. Off ⇒ the transparency checker
    /// shows through wherever the art is transparent.
    PaperToggle,
    /// Set the paper colour (undoable — it is what the page exports on).
    SetPaperColour([u8; 3]),
}

/// Base radii of the `SimpleDab` fallback engine — its shipped size, and the
/// min:max ratio `Engine::set_size_px` keeps. libmypaint presets carry their
/// own radius instead (see `MyBrush::set_size_px`).
pub const BASE_MIN_RADIUS: f32 = 1.0;
pub const BASE_MAX_RADIUS: f32 = 12.0;

/// The absolute Size range, as a dab DIAMETER in canvas px.
///
/// The ceiling deliberately sits PAST the `[`/`]` ladder's top rung (2000 px):
/// the bug this replaces was a 4× multiplier ceiling quietly capping the
/// ladder, so the clamp must never be the thing a `]` press runs into.
pub const SIZE_PX_MIN: f32 = 0.1;
pub const SIZE_PX_MAX: f32 = 5000.0;

/// Pre-seed placeholder for `ToolProps::default()` only — a real sub tool's
/// size comes from its preset (`Engine::base_size_px`).
pub const DEFAULT_SIZE_PX: f32 = 10.0;

pub fn dispatch(app: &mut App, cmd: AppCmd) {
    // `IO-060`: a work resample in flight owns the whole page set. Its
    // phase 1 has the open page stashed and a pending list keyed by page
    // INDEX, so a page turn, an undo or a save arriving between two pages
    // would install work built against a document set that no longer
    // exists. Commands are dropped rather than queued — a queue would
    // replay a dozen impatient clicks the moment the run finished — and
    // not silently: the progress window is on screen for the whole run
    // saying that the app takes nothing else until it is done, and
    // carrying the Cancel that ends it.
    if app.resample_job.is_some() {
        return;
    }
    // FB-039: the last-frame delete confirmation is one-shot — any other
    // command disarms it.
    if !matches!(cmd, AppCmd::FrameDelete { .. }) {
        app.frame_delete_armed = None;
    }
    // A live tonal-correction preview writes REAL pixels outside the undo
    // bracket. Nothing else may see them — a save would bake a correction
    // the history knows nothing about, an undo would restore around it, a
    // layer switch would leave the dialog pointed at the wrong layer. So
    // any other command reverts the preview and closes the dialog first.
    // (`begin_stroke` is the other door; it refuses instead.)
    if !matches!(
        cmd,
        AppCmd::AdjustOpen(_) | AppCmd::AdjustApply | AppCmd::AdjustCancel | AppCmd::AdjustNow(_)
    ) {
        app.adjust_preview_revert();
    }
    // A live-layer param drag emits one `SetFillParams` per frame so the
    // canvas re-derives while the slider moves; only the first records an
    // undo pre-image. ANY other command means the drag cannot still be
    // running, so the next param edit starts a fresh session — the belt to
    // `ParamEditSession(None)`'s braces.
    if !matches!(
        cmd,
        AppCmd::SetFillParams(..) | AppCmd::SetLayerBlendIf(..) | AppCmd::ParamEditSession(_)
    ) {
        app.param_session = None;
    }
    // docs/CLIPPING-SCENARIOS.md 5b: a structure edit that silences or
    // re-attaches someone's clip should SAY so, not just change the
    // picture. Snapshot which clipped layers are dangling; the tail of
    // this function compares and reports.
    let clip_before = clip_dangling_by_name(&app.doc);
    // Recordable actions: the recorder taps the stream HERE, before the
    // match consumes the command. Nothing records during a replay (a run
    // would eat itself), and index-carrying commands record only when
    // aimed at the ACTIVE layer — steps are index-free by design.
    let rec_step = if app.action_recording.is_some() && !app.action_running {
        crate::app::actions::ActionStep::from_cmd(&cmd, app.doc.active)
    } else {
        None
    };
    history::run(app, cmd, CmdTail { clip_before, rec_step });
}

/// The tail of [`dispatch`], carried down the chain of per-domain `run`
/// functions in `cmd::*`.
///
/// `dispatch` used to be one function with one `match`; an arm that wanted to
/// skip the tail work just wrote `return`. The match now lives in ten modules,
/// so the tail travels with the command: a module runs it after its `match`
/// falls through, and an arm's bare `return` still means "skip it".
struct CmdTail {
    clip_before: Vec<(String, bool)>,
    rec_step: Option<crate::app::actions::ActionStep>,
}

fn run_cmd_tail(app: &mut App, cmd_tail: CmdTail) {
    let CmdTail {
        clip_before,
        rec_step,
    } = cmd_tail;
    // The Pages palette follows the document (manga ⇒ present, plain image ⇒
    // closed) — after the command, so new/open/add/delete page all reconcile.
    app.sync_pages_palette();
    report_clip_changes(app, &clip_before);
    // Recordable actions: append the tapped step after the command ran, so
    // a recorded sequence reads in execution order.
    if let (Some(idx), Some(step)) = (app.action_recording, rec_step) {
        if let Some(a) = app.actions.get_mut(idx) {
            a.steps
                .push(crate::app::actions::StepRow { step, on: true });
            app.actions_save();
            app.mark_dirty();
        }
    }
}

/// docs/CLIPPING-SCENARIOS.md 5b support: `(name, dangling)` for every
/// clipped layer. Keyed by NAME on purpose — indices shift under the very
/// edits 5b reports on, and the report only fires for a name present on
/// both sides, so a rename can never false-positive.
fn clip_dangling_by_name(doc: &mn_core::Document) -> Vec<(String, bool)> {
    let bases = doc.clip_bases();
    doc.layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.clip && !l.folder)
        .map(|(i, l)| (l.name.clone(), bases[i].is_none()))
        .collect()
}

/// 5b: when the command changed whether some clip has a base, say so. The
/// first change wins the status line (it is a status line, not an audit);
/// duplicate layer names share a verdict, which is as good as a name key
/// gets. Pinned by `cmd_tests::a_structure_edit_that_silences_a_clip_reports_it`.
fn report_clip_changes(app: &mut App, before: &[(String, bool)]) {
    let after = clip_dangling_by_name(&app.doc);
    for (name, dangling) in &after {
        let Some((_, was)) = before.iter().find(|(n, _)| n == name) else {
            continue;
        };
        if dangling == was {
            continue;
        }
        if *dangling {
            app.set_status(format!(
                "\"{name}\": clip lost its base here — the flag is ignored (grey mark)"
            ));
        } else {
            app.set_status(format!("\"{name}\": clip re-attached to what sits below"));
        }
        return;
    }
}

/// docs/CLIPPING-SCENARIOS.md 5b: the status line speaks when an edit
/// silences or re-attaches an existing clip. (No test module existed in
/// this file before; the headless fixture copies `app::adjust::tests`.)
#[cfg(test)]
mod cmd_tests {
    use super::*;

    fn headless() -> Option<App> {
        let renderer = mn_gpu::Renderer::new_headless(mn_gpu::GpuConfig {
            force_fallback: std::env::var("MN_WARP").is_ok(),
            no_vsync: false,
        })
        .ok()?;
        Some(App::new(renderer, (1280, 860), 1.0))
    }

    #[test]
    fn a_structure_edit_that_silences_a_clip_reports_it() {
        let Some(mut app) = headless() else {
            println!("[test] SKIP: no usable adapter");
            return;
        };
        // A folder with ink-bearing potential below a clipped "Shade":
        // clip-to-folder makes the header the base, so flipping the folder
        // Through is exactly the edit that silences the clip (2a note).
        let hi = app.doc.add_folder_above(0, "F");
        app.doc.add_layer_in_folder(hi, "in").unwrap();
        let hi = hi + 1;
        let top = app.doc.add_layer_above(hi, "Shade");
        assert!(app.doc.set_layer_clip(top, true));
        assert_eq!(app.doc.clip_bases()[top], Some(hi), "fixture: clip is live");

        dispatch(&mut app, AppCmd::SetFolderThrough(hi, true));
        assert!(
            app.status.contains("Shade") && app.status.contains("lost its base"),
            "silencing reported: {}",
            app.status
        );
        dispatch(&mut app, AppCmd::SetFolderThrough(hi, false));
        assert!(
            app.status.contains("Shade") && app.status.contains("re-attached"),
            "re-attach reported: {}",
            app.status
        );
    }
}
