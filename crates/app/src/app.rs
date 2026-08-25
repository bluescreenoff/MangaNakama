//! Application state: document, brush, renderer, viewport, egui shell, tools.
//!
//! Deliberately free of `HWND`-poking beyond redraw requests — `main.rs` owns
//! the window and the message loop, this owns what a stroke *means*. UI widgets
//! never mutate state directly either: they push [`AppCmd`]s, which
//! `main::pump_commands` feeds to [`crate::cmd::dispatch`] once no `&mut App`
//! is alive (file dialogs re-enter the message loop).
//!
//! `impl App` is split across the `app/` children (`canvas_input`, `pages`,
//! `transform`, `view`, `layout` for its types) the same way `text_edit.rs`
//! already implements App methods outside this file.

mod abr;
pub mod actions;
mod adjust;
pub mod batch;
mod brush_manage;
pub(crate) mod canvas_input;
mod comps;
mod diag;
mod engine;
mod frames;
mod kpp_import;
/// TRIAGE 36 (`L-001`/`L-002` magnetic lasso) and 38 (`S-001` layer pick),
/// end to end through the real pointer path. Its own file so app.rs does not
/// grow another 200 lines of test.
#[cfg(test)]
mod lasso_tests;
mod layout;
mod make_brush;
pub mod materials;
mod pages;
pub mod pattern;
pub mod prefs;
pub(crate) mod reader;
mod session;
mod story;
mod sut_import;
pub(crate) mod tone_tool;
mod transform;
pub(crate) mod vector_edit;
mod view;
mod workspaces;

pub use adjust::AdjustPreview;
pub use layout::{ScreenRect, UiLayout, WinGeom, peek_win};
pub use prefs::Prefs;

pub use crate::cmd::RulerKind;
pub use pages::{CanvasSizeDraft, NewComicDraft, PageEntry, SpreadOp, WorkSettingsDraft};
pub use session::{DocSession, unsaved_autosave_folder_for, unsaved_autosave_path_for};
pub use transform::{
    ROTATE_STALK_SCREEN, TransformDrag, TransformGesture, TransformGrab, transform_preview,
};

use canvas_input::{BalloonObjDrag, ObjectDrag};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mn_brush::{CurveDab, DynaDab, GridDab, HairyDab, MyBrush, SimpleDab};
use mn_core::{
    Align, Blend, Document, FillOpts, FrameAlign, LineSpacing, PageSetup, PenSample, ResizeAnchor,
    Selection, Stabilizer, StrokeSink, Taper,
};
use mn_gpu::{Renderer, Viewport};

use crate::cmd::{
    AppCmd, BASE_MAX_RADIUS, BASE_MIN_RADIUS, BalloonMode, DivideContents, EyedropOpts, FigureMode,
    FillMode, FrameMode, GradMode, ObjectMode, PanMode, PickExclude, SelectMode, Slot, Tool,
    ToolProps,
};
use crate::shell::Shell;
use crate::text_edit::{TextBarDrag, TextEditState, TextGesture, TextObjDrag};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PointerKind {
    Pen,
    Mouse,
}

/// Who a press belongs to for as long as it is held. Decided once, on the
/// down-event, so a drag that starts on the canvas cannot be stolen by a panel
/// it wanders over (and vice versa).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owner {
    None,
    Canvas,
    Egui,
}

/// A caption-button click on the custom title bar (drawn by `ui::top`, run by
/// `main::pump_commands` — window ops must not happen inside the wndproc
/// borrow). Close is not here: it reuses `close_requested`'s save-prompt flow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptionCmd {
    Minimize,
    ToggleMax,
}

impl PointerKind {
    pub fn label(self) -> &'static str {
        match self {
            PointerKind::Pen => "pen",
            PointerKind::Mouse => "mouse",
        }
    }
}

use diag::StrokeStats;
/// Per-stroke console diagnostics. The owner's laptop pen stack is known-cursed; these
/// numbers are the first thing to look at when a stroke feels wrong.
pub use diag::{Diag, PenHealth};

/// What the message loop must do after a frame. (The cursor is not in here:
/// Win32 wants it set from `WM_SETCURSOR`, which reads `Shell::cursor`.)
pub struct FrameOutput {
    /// `Duration::ZERO` = repaint now, `Duration::MAX` = only on input.
    pub repaint_after: Duration,
}

pub use engine::{DabStrokeApp, Engine, EngineKind, preset_engine};
use engine::{StrokeTwin, TwinAxis, smudge_tile_oracle, symmetric_affines};

pub struct App {
    pub doc: Document,
    /// The brush, behind the stabilizer and the entry taper. Both decorators
    /// are exact passthroughs at their neutral settings, so this is always the
    /// same object chain whether smoothing/taper are on or not.
    pub brush: Stabilizer<Taper<Engine>>,
    /// Doc-space input resampling (owner pen-test 2026-08-17: strokes drawn
    /// zoomed-out came back polygonal — the input path never interpolated).
    /// Fed the raw canvas-space samples in `push_batch`, flushed before
    /// `end_stroke`; shape-preserving Catmull-Rom, ≤1 doc px spacing. See
    /// `input_path.rs`.
    pub input_resampler: crate::input_path::InputResampler,
    pub renderer: Renderer,
    pub viewport: Viewport,
    /// Startup fit-to-view is DEFERRED: `App::new` runs before the window
    /// is restored/maximized, so its fit is against the wrong size. While
    /// this is up, `render` refits whenever the canvas rect moved and
    /// stands down the first frame it holds still (see the comment there).
    pub startup_fit_pending: bool,
    startup_fit_last: (f32, f32),
    pub shell: Shell,

    // --- tool state (the UI reads these, commands write them) -------------
    pub tool: Tool,
    // CSP colour model: main / sub / transparent drawing-colour slots.
    pub main_color: [f32; 3],
    pub sub_color: [f32; 3],
    pub slot: Slot,
    /// Tool Property values of the *current* sub tool. CSP semantics: saved
    /// per sub tool — `props` is the memory, keyed by preset path.
    pub props_current: ToolProps,
    props: HashMap<PathBuf, ToolProps>,
    /// Latch for the per-dab tile-budget warning (PATCHES.md #19): the
    /// preset-key@size that last warned, so one clamped brush says so once
    /// instead of per stroke.
    dab_clamp_warned: Option<String>,
    /// Shipped texture-tip mask names (Tool Property's picker list).
    pub texture_names: Vec<String>,
    /// The brushes root the presets came from — masks load from under it.
    pub brushes_root: Option<PathBuf>,
    // --- curve editor (Krita per-sensor curves) -----------------------------
    /// Which dynamic the editor edits (Size / Opacity / …).
    pub curve_setting: crate::cmd::CurveSetting,
    /// Which sensor drives it (Pressure / Speed / Tilt / …).
    pub curve_sensor: crate::cmd::CurveSensor,
    /// The points being dragged (raw setting units); synced from the engine
    /// whenever no drag is live so the widget owns edits end-to-end.
    pub curve_edit_points: Vec<(f32, f32)>,
    /// Index of the point under drag, if any.
    pub curve_drag: Option<usize>,
    /// Per-sub-tool curve edits, replayed on preset switch (session memory;
    /// the preset files themselves are never rewritten).
    pub curve_overrides: HashMap<PathBuf, HashMap<(u8, u8), Vec<(f32, f32)>>>,
    /// Symmetry painting (Krita mirror): strokes reflect across the canvas
    /// centre's vertical / horizontal axis while these are on.
    pub mirror_x: bool,
    pub mirror_y: bool,
    /// Wrap-around tiling (Krita wrap mode): dabs near an edge continue on
    /// the opposite side — seamless border tiling for clothing/wall textures.
    pub wrap_x: bool,
    pub wrap_y: bool,
    /// The Color Set: colours the user chose and keeps, persisted in
    /// `swatches.txt`. Named when a `.gpl` import gave them names.
    pub swatches: Vec<mn_core::palette::Swatch>,
    /// Recently USED colours, newest first, at most [`COLOR_HISTORY_MAX`].
    /// The disposable half of the palette — automatic, bounded, and never
    /// the same thing as `swatches`, which are chosen and kept. Fed at the
    /// `SetSlotColor` choke point; persisted in `ui.txt`.
    pub color_history: Vec<[f32; 3]>,
    /// Manga page geometry for the guides overlay; `None` = plain canvas.
    pub page: Option<PageSetup>,
    // --- comic project ------------------------------------------------------
    /// All pages; `pages[page_index]` is the one decoded into `doc`.
    pub pages: Vec<PageEntry>,
    pub page_index: usize,
    pub story: String,
    pub binding_right: bool,
    /// New pages get a frame border folder (New Manga checkbox).
    pub seed_frame_folder: bool,
    pub new_doc_open: bool,
    pub new_doc_draft: NewComicDraft,
    /// Work Settings dialog: edit story/binding/page geometry after creation.
    pub work_settings_open: bool,
    pub work_settings_draft: WorkSettingsDraft,
    /// Change Canvas Size dialog (Edit menu): new size + CSP anchor.
    pub canvas_size_open: bool,
    /// Preferences window (Edit ▸ Preferences…).
    pub prefs_open: bool,
    /// What the window opened ON: a tab name ("Performance") or a row id
    /// from `ui::prefs_dialog::PREF_INDEX` ("undo_depth") — the window
    /// resolves either to a tab and lights the row. Cleared when it closes.
    pub prefs_focus: Option<&'static str>,
    /// The Preferences window's active tab (index into
    /// `ui::prefs_dialog::TABS`). Session state, not persisted.
    pub prefs_tab: usize,
    /// The Preferences window's live search text.
    pub prefs_search: String,
    /// The UI-size slider moved: `main::pump_commands` re-derives the
    /// effective pixels-per-point (window DPI × this) through the
    /// `dpi_changed` door. Same shape as `autosave_rearm`, same reason.
    pub ui_scale_apply: Option<f32>,
    /// TC-004/005/006/011: the open tonal-correction dialog's live
    /// parameters; `None` = no correction dialog open.
    pub adjust_draft: Option<mn_core::Adjust>,
    /// The pre-images the live preview overwrote. See `app/adjust.rs` — a
    /// preview writes real pixels outside the undo bracket.
    pub adjust_preview: Option<AdjustPreview>,
    /// PM-022: the Go to Page dialog state (1-based draft number).
    pub goto_page_open: bool,
    pub goto_page_value: i32,
    /// TRIAGE 143: the Combine/Split spread dialog state.
    pub spread_op: Option<crate::app::SpreadOp>,
    pub spread_gap: i32,
    pub spread_delete_empty: bool,
    /// PM-050/051/053/054/055: the Export All Pages options window. The
    /// defaults reproduce the pre-options run byte for byte — the prefix
    /// is SEEDED from the work name when the window opens, the range is
    /// off, and both toggles are off.
    pub export_all_open: bool,
    pub export_all_prefix: String,
    /// PM-054: false = every page; true = `export_all_from..=export_all_to`
    /// (1-based, inclusive, clamped on apply).
    pub export_all_range: bool,
    pub export_all_from: i32,
    pub export_all_to: i32,
    /// PM-055: a spread page leaves as two files (`…p003a` / `…p003b`,
    /// `a` = the half a reader meets first).
    pub export_all_split: bool,
    /// PM-053: also write the script dump beside the images.
    pub export_all_text: bool,
    /// Print finishing: output resolution, `0` = the work's own (no
    /// resample). Session-only, like every other field of this window.
    pub export_all_dpi: u32,
    /// Print finishing: the expression colour the exported files are
    /// reduced to. `Colour` = untouched, which is what the run did before
    /// finishing presets existed.
    pub export_all_colour: mn_core::LayerExpression,
    /// What rectangle leaves the building (M2): whole paper, trim+bleed
    /// (print plate), or trim only (web). Paper = the pre-profile export,
    /// byte-identical.
    pub export_all_crop: mn_core::export::ExportCrop,
    /// Exact output height in px, 0 = off. Wins over dpi; never upsamples.
    pub export_all_px_height: u32,
    /// TRIAGE 144: the Story Editor window + its per-page decoded docs
    /// (None = the active page, which edits the live document).
    pub story_open: bool,
    pub story_docs: Vec<Option<mn_core::Document>>,
    /// Edit buffers, flat and aligned with story_fields() enumeration.
    pub story_bufs: Vec<String>,
    /// LM-008: tint the active layer's masked-off region purple.
    pub mask_show_area: bool,
    /// TN-011: tint every toned region of the whole stack green, so leftover
    /// scraps of tone are visible before print. A view state, never exported.
    pub tone_show_area: bool,
    /// LM-004: strokes target the active layer's mask.
    pub mask_edit: bool,
    /// Quick Mask (SE round 2026-08-19): while on, Pen strokes ADD to the
    /// selection and Eraser strokes SUBTRACT — brushes edit the selection
    /// instead of inking (CSP's quick-mask affordance without the modal
    /// toggle dance: the ants are always live).
    pub quick_mask: bool,
    /// The in-flight selection-paint op (set at begin_stroke when the
    /// tool is the selection pen/eraser or Quick Mask is on; consumed at
    /// end_stroke's commit).
    sel_paint: Option<SelPaintOp>,
    /// Owner preview tier (2026-08-18): Pages-palette cell width target
    /// (the slider), the Fit-to-pane toggle, and the cell width the
    /// palette last DREW — the live thumb renders at that size. CSP's
    /// Page Manager "Fit to Navigator".
    pub pages_cell_w: f32,
    pub pages_fit: bool,
    pub pages_cell_px: f32,
    /// Cell height the live thumb texture was minted at (re-mint on drift).
    pub pages_thumb_px: f32,
    /// LRU order of decoded page previews (evict beyond 32).
    pub preview_order: std::collections::VecDeque<usize>,
    /// Owner top item (2026-08-18): the READER — read the chapter in-app.
    pub reader: reader::ReaderState,
    /// Owner top item (2026-08-18): panel READING ORDER — the cached
    /// computed order (Layers badge + on-canvas path) and its toggle.
    pub frame_order: Option<mn_core::frame_order::PanelOrder>,
    /// The document revision `frame_order` was computed at — it caches raw
    /// layer indices, so anything that shifts the stack invalidates it.
    pub frame_order_rev: u64,
    pub frame_order_show: bool,
    /// FB-039: the one-shot confirmation for deleting a frame
    /// folder's LAST frame (Delete arms, next Delete removes the folder
    /// WITH its layers). Cleared by any other command.
    pub frame_delete_armed: Option<(usize, usize)>,
    /// PM-044: the story editor's selected field (story_fields index)
    /// + the move/duplicate target page.
    pub story_sel: Option<usize>,
    pub story_move_to: usize,
    /// The window handle (0 in tests — fullscreen is state-only then).
    /// Set by main right after construction.
    pub hwnd: isize,
    /// TRIAGE 146 v1: named workspaces
    /// ([[name, dl, dr, lw, rw, ph, lcol, rcol]]).
    ///
    /// A `Vec<String>` per entry, NOT a fixed-size array: the entry was six
    /// fields until the column-collapse round, and `[String; 6]` makes serde
    /// REJECT the whole line the moment it grows or shrinks — every saved
    /// workspace would vanish on the version that added a field, silently
    /// (the parse falls back to `default()`). `workspaces.rs` reads every
    /// field through `Self::ws_field`, so short (older) entries simply keep
    /// this build's defaults for what they do not carry.
    pub workspaces: Vec<Vec<String>>,
    pub workspace_current: String,
    pub workspace_open: bool,
    pub workspace_draft: String,
    /// TRIAGE 139 v1: layer comps (runtime-only this round).
    pub comp_selected: Option<usize>,
    /// LC-007 multi-selection (sorted indices; Ctrl+click toggles,
    /// Shift+click ranges). Empty = none — LC-013's export treats empty
    /// as "all comps" (CSP's rule).
    pub comp_multi: Vec<usize>,
    /// Recordable action sequences (`app::actions`), loaded from
    /// actions.json beside the exe; saved on every change.
    pub actions: Vec<actions::Action>,
    /// Which action the palette has open (its steps show below the list).
    pub action_selected: Option<usize>,
    /// Recording target: dispatch appends recordable commands as steps
    /// while this is armed.
    pub action_recording: Option<usize>,
    /// True while a replay runs — the recorder must not eat the replay.
    pub action_running: bool,
    /// Inline rename in the Auto Actions palette (index, draft).
    pub action_renaming: Option<(usize, String)>,
    /// The "＋ step" picker: (action index, slot the pick inserts at).
    /// Runtime-only — a half-open menu is not worth persisting.
    pub action_picker: Option<(usize, usize)>,
    /// Which step has its inline parameter editor open (action, step).
    pub action_step_edit: Option<(usize, usize)>,
    /// LC-003: the presentation state captured just before the most recent
    /// comp application, as an unnamed comp — the same shape it restores,
    /// so it covers every property an apply can write, not just the eyes.
    pub comp_last_state: Option<mn_core::doc::LayerComp>,
    pub comp_name_draft: String,
    /// LC-006: layers added after a snapshot default to visible.
    pub comp_added_visible: bool,
    /// TRIAGE 101/102: the blur-family filter dialog. The pending `Filter`
    /// IS the dialog state — which variant it is decides which controls show,
    /// and its fields are the drafts. `None` = closed.
    pub filter_draft: Option<mn_core::Filter>,
    /// TRIAGE 140 v1: the speed/focus generator dialog state.
    pub gen_open: bool,
    /// True once the dialog fields hold REAL values (a first-open seed or
    /// a GenLinesEdit load). The old "seed when a == 0 && b == 0" test
    /// used a legal centre — a focus set converging on the top-left
    /// corner — as its uninitialized sentinel, so Edit ▸ effect lines on
    /// such a layer silently reset every parameter to the defaults (and
    /// the centre X could never be dragged to 0 while Y was 0).
    pub gen_inited: bool,
    pub gen_focus: bool,
    pub gen_a: f32,
    pub gen_b: f32,
    pub gen_c: f32,
    pub gen_d: f32,
    pub gen_count: u32,
    pub gen_width: f32,
    pub gen_jitter: f32,
    pub gen_seed: u64,
    /// SF-004/005: the Object tool's selected generated layer + the
    /// live driver drag (blue reference / shape handles).
    pub gen_sel: Option<usize>,
    pub gen_drag: Option<crate::app::canvas_input::GenLinesDrag>,
    /// Live drag of a tone layer's lattice (CSP "Move tone pattern").
    pub fill_lattice_drag: Option<crate::app::canvas_input::FillLatticeDrag>,
    /// Row 89: the GLOBAL pen-pressure correction, empty = identity.
    /// Applied to every sample in `push_batch`, before per-tool curves.
    pub global_pressure: Vec<[f32; 2]>,
    /// Row 42 (A-014, はみ出さない): brush strokes stay inside the
    /// reference set's ink (and frame-border folders) — the barrier is
    /// built per stroke in `begin_stroke`; False paints freely.
    pub anti_overflow: bool,
    /// The barrier cache (audit small, 2026-08-25): a full reference
    /// composite per stroke was ~71 MB + a 24 MB mask on the UI thread
    /// at B4/600dpi. The mask only changes when the reference set's own
    /// ink does, so it is cached against `(canvas size, reference layer
    /// indices, the newest tile revision among them)` — tile revisions
    /// are globally monotonic, so any edit, paste or undo inside a
    /// reference layer moves the key, and a paint stroke elsewhere
    /// never does.
    pub anti_overflow_cache: Option<(
        ((u32, u32), Vec<usize>, u64),
        Option<std::sync::Arc<mn_brush::AntiOverflowMask>>,
    )>,
    /// The pen-pressure wizard (BR-014–016): open flag, the
    /// Stronger/Weaker bend, and the raw pressures of strokes drawn
    /// while it listens.
    pub pen_wizard_open: bool,
    pub pen_wizard_gamma: f32,
    pub pen_wizard_samples: Vec<f32>,
    /// Tool Property's draft of the SELECTED run's spec while a bar is
    /// being dragged — the same buffering idiom as `border_edit`, and for
    /// a sharper reason: committing per frame would re-rasterize a
    /// page-sized effect-line layer on every mouse move.
    pub gen_edit: Option<mn_core::genlines::GenLinesSpec>,
    /// Quick Access (UI-050/052): the search field + pinned commands.
    pub quick_query: String,
    pub quick_pins: Vec<String>,
    /// The floating command palette (Ctrl+K, `ui/quick.rs`): open flag, its
    /// query, the highlighted row, and the labels run this session (most
    /// recent first — they lead the empty query). Session-only on purpose:
    /// a "recent" list restored from disk is not recent, it is history.
    pub cmdpal_open: bool,
    pub cmdpal_query: String,
    pub cmdpal_sel: usize,
    pub cmdpal_recent: Vec<String>,
    /// The overlay's rows, gathered when it OPENS (`ui::quick`): the index
    /// is thousands of rows with a material bank in it, and every
    /// index-keyed row (layer, page, action) must be as fresh as the press
    /// that summoned it. Empty while the overlay is closed.
    pub cmdpal_entries: Vec<crate::ui::quick::Entry>,
    /// PM-046: the find/replace row state.
    pub story_find: String,
    pub story_repl: String,
    pub story_ignore_case: bool,
    pub canvas_size_draft: CanvasSizeDraft,
    /// Print story info in page margins (round 14 feature).
    pub print_margin_info: bool,
    /// The work's expression colour (TRIAGE 132 preflight): Mono = B&W
    /// print; the colour-on-mono predicate keys off it.
    pub expression: mn_core::Expression,
    /// Perfect-binding spine width, mm (0 = unset). Preflight input.
    pub spine_mm: f32,
    /// Cover page designation — page index in reading order. Preflight input.
    pub cover: Option<usize>,
    /// Template page (tekno B2) — reading-order index whose bytes seed new
    /// pages instead of a blank. Set from the Pages palette's right-click;
    /// index-bound like `cover`.
    pub template_page: Option<usize>,
    /// Publisher/printer target (ROADMAP M2): preflight norms + export
    /// finish preselect. Picked in Work Settings; rides the work file.
    pub profile: Option<mn_core::profile::PublisherProfile>,
    /// Preflight palette (TRIAGE 132): cached findings + the staleness key
    /// (active doc revision + page index + a manual flag for work-metadata
    /// edits, which do not bump the doc revision).
    pub preflight_findings: Option<Vec<mn_core::PreflightFinding>>,
    pub preflight_rev: u64,
    pub preflight_page: usize,
    pub preflight_stale: bool,
    /// SL-001..004 (Search Layer): the Layers palette's row filter —
    /// name substring, layer type, and the two property narrowings worth
    /// their row. All-default = no filtering and the palette behaves
    /// exactly as it did before the filter existed.
    pub layer_search: String,
    pub layer_filter_kind: LayerFilterKind,
    /// SL-002 include: only layers the fill/wand samples.
    pub layer_filter_ref_only: bool,
    /// SL-003 exclude: hide 下書き rows once the finish starts.
    pub layer_filter_no_draft: bool,
    /// SL-003's manga row: only the frame folder holding the active layer
    /// (and that folder's own header). Matches nothing when the active
    /// layer sits in no frame folder — the count row says so.
    pub layer_filter_this_frame: bool,
    /// The filter row hides behind the palette's funnel button; closing
    /// resets every filter control so nothing narrows invisibly.
    pub layer_filter_open: bool,
    /// Only rows whose OWN label is exactly this standard swatch colour.
    pub layer_filter_label: Option<[u8; 3]>,
    /// Material bank (TRIAGE 133): folders (shipped starter first, then
    /// user-added), scanned items, lazy thumbnails, use counters, the
    /// palette's search + sort + tiling state.
    pub material_folders: Vec<std::path::PathBuf>,
    pub material_folder_names: Vec<String>,
    pub materials: Vec<crate::app::materials::MaterialItem>,
    pub material_thumbs: std::collections::HashMap<std::path::PathBuf, egui::TextureHandle>,
    pub material_uses: std::collections::BTreeMap<String, u64>,
    pub material_search: String,
    /// Sort the bank by name (false) or most-used (true) — MT-016.
    pub material_sort_uses: bool,
    /// Paste materials as an N×N canvas-covering tile (the owner's tiling
    /// ask: a mask to draw through).
    pub material_tile: bool,
    /// Tone pasted materials (MT-014): the material's ink renders as the
    /// document's screentone instead of greyscale pixels — what makes an
    /// arbitrary image printable on a mono page. The screen is the tone
    /// engine's own (60 LPI 45° dots, canvas-continuous).
    pub material_tone: bool,
    /// MT-032: the material paste-size mode (see [`MaterialPasteSize`]).
    pub material_size: MaterialPasteSize,
    /// MT-034: where a panel-targeted material paste's layer sits in the
    /// folder (see [`MaterialLayerOrder`]).
    pub material_order: MaterialLayerOrder,
    /// MT-012: the material whose tag editor is open in the bank's
    /// right-click menu, and the text being typed. `None` = no editor open;
    /// the buffer is re-seeded from the sidecar whenever the menu opens on a
    /// different material, so it can never write one material's tags onto
    /// another's.
    pub material_tag_edit: Option<(std::path::PathBuf, String)>,
    /// P0-1: the palette's folder tree, rebuilt by `materials_scan()` and
    /// never per frame (its counts are an O(dirs × items) sweep, and the
    /// owner's bank is 2399 files).
    pub material_tree: Vec<crate::app::materials::MaterialNode>,
    /// Which tree row the grid is showing.
    pub material_filter: crate::app::materials::MaterialFilter,
    /// Tree rows whose subtree is folded away, by `MaterialFilter::id()`.
    /// CLOSED rather than open, so the default — an absent id — is an
    /// expanded tree: a bank whose branches all start shut looks empty.
    pub material_tree_closed: std::collections::HashSet<String>,
    /// The tree column is showing. Off gives the grid the whole palette,
    /// which is what a narrow dock wants.
    pub material_tree_show: bool,
    /// P1-2: the SELECTED material (index into `materials`), which is not
    /// the pasted one — a click selects, a double-click applies. Cleared by
    /// every rescan, because the index means nothing across one.
    pub material_selected: Option<usize>,
    /// Thumbnail edge in px — the small/medium/large cycle. **Session-only,
    /// deliberately**: no `ui.txt` key fits it naturally (the material keys
    /// there are the folder list and the use counters), and inventing one
    /// means editing `layout.rs`, which other work owns this round.
    pub material_thumb_px: f32,
    /// Thumbnail-cache bookkeeping: path → the tick it was last drawn on,
    /// plus the tick counter. `material_thumbs` is capped, and this is what
    /// decides who goes — 2399 live GPU textures is a texture per file.
    pub material_thumb_lru: std::collections::HashMap<std::path::PathBuf, u64>,
    pub material_thumb_tick: u64,
    /// The References palette (`ui/refs.rs`): the persisted path list, the
    /// lazily loaded textures, and the open viewer windows. One field because
    /// the halves only make sense together — the list is what persists, the
    /// viewers and textures are the session.
    pub refs: crate::ui::refs::RefBank,
    /// Navigator (CV-036): sticky fit — keep re-fitting the page on every
    /// window resize until toggled off.
    pub fit_sticky: bool,
    /// Rulers (TODO #3): the pending CREATION (a Layer ▸ Ruler menu choice
    /// arms the next canvas drag). The set itself lives on the document
    /// (`app.doc.rulers`) so ruler edits ride the document's one undo
    /// history; this is the arming state of a menu, which does not.
    pub ruler_pending: Option<RulerKind>,

    /// Default line count for the next symmetric ruler (RL-021); existing
    /// ones are re-counted through the menu ladder.
    pub symmetric_lines: u16,
    /// The drag start while a ruler creation is pending.
    pub ruler_drag: Option<[f32; 2]>,
    /// A ruler MOVE in progress (Object tool grabbed an anchor or a body).
    pub ruler_move: Option<canvas_input::RulerMove>,
    /// Part 2/4: the stroke-scoped sticky-snap lock (reset at
    /// begin_stroke) — the locked ruler, plus the perspective binding
    /// (anchor + the fixed family member).
    pub ruler_lock: mn_core::SnapLock,
    /// Part 2: the in-progress curve ruler polyline (click vertices).
    pub curve_pending: Option<Vec<[f32; 2]>>,
    /// Click-run detection (client px + time + how many presses deep the run
    /// is): curve-ruler finishing, double-click word select, triple-click line
    /// select, double-click-to-edit on the Object tool. Maintained by
    /// `App::click_run` — the window class carries no CS_DBLCLKS, so the run
    /// is timed by hand.
    pub last_click: Option<(f32, f32, std::time::Instant, u8)>,
    /// The surface size the last sticky-fit check saw (change = resize).
    pub nav_last_surface: (u32, u32),
    /// Navigator thumbnail cache: the texture + the doc revision it was
    /// rendered from (re-render only when the document moves).
    pub nav_thumb: Option<egui::TextureHandle>,
    pub nav_thumb_rev: u64,
    /// Sub Tool Detail floating window (the wrench).
    pub detail_open: bool,
    /// The Tool Property full-properties window (CSP: Tool Property >
    // sub-tool-detail with per-category eye toggles).
    pub prop_detail_open: bool,
    /// Tool Property sections hidden from the COMPACT palette (the eye
    /// toggles; the full window always shows everything). Persisted in
    /// ui.txt as `prop_hidden=`.
    pub prop_hidden: std::collections::BTreeSet<String>,
    /// Palette layout: widths, order, collapsed/floating sets. Persisted.
    pub layout: UiLayout,
    /// User preferences (`prefs.txt` beside the exe — deliberately NOT
    /// `ui.txt`, which is the file people delete to fix a wrecked dock).
    pub prefs: Prefs,
    /// Batch layer operations dialog state (`app/batch.rs`). Session-only.
    pub batch: batch::BatchOps,
    /// Pattern Studio window state (`app/pattern.rs`). Session-only.
    pub pattern: pattern::PatternStudio,
    /// Vector inking (docs/VECTOR-INKING.md): the in-flight stroke's
    /// captured samples — `Some` only between begin/end of a plain ink
    /// stroke on a recording layer.
    pub vector_capture: Option<Vec<PenSample>>,
    /// The Object tool's selected stroke INDEX on the ACTIVE layer
    /// (layer-scoped editing). Cleared on layer/tab switches and undo —
    /// indices shift under it (CODE-MAP seam #2).
    pub vector_sel: Option<usize>,
    /// A stroke drag in flight (`vector_edit.rs`).
    pub vector_drag: Option<vector_edit::VectorDrag>,
    /// The autosave interval changed: `main::pump_commands` re-arms the
    /// Win32 timer with this many ms (0 = kill it, autosave off). An App
    /// field rather than a direct `SetTimer` because this crate stays free
    /// of `HWND`-poking beyond redraw requests.
    pub autosave_rearm: Option<u32>,
    /// PR-041: the `Document::op_count` the last per-operation recovery
    /// save covered. The edge, not a flag: comparing counts is what makes
    /// the check idempotent, so `pump_commands` can ask every pass and
    /// only ever write once per operation.
    autosave_op_seen: u64,
    /// Brush stroke previews for the Sub Tool list, keyed by preset path.
    /// `None` = generation failed once, don't retry.
    pub brush_previews: HashMap<PathBuf, Option<egui::TextureHandle>>,
    /// Preview generations left this frame (reset in `ui::build`) — startup
    /// trickles one per frame instead of hitching.
    pub preview_budget: u32,
    /// Page-pane texture mints left this frame (docking 2 page views;
    /// reset in `ui::build`) — same trickle rule as the previews above.
    pub page_pane_budget: u32,
    /// Color panel HSV state; hue/saturation survive grayscale RGB values.
    pub picker_hsv: [f32; 3],
    pub picker_rgb_cache: [f32; 3],
    /// The HEX field's edit buffer. Tracks the active colour except while
    /// the field holds focus, when it is whatever the user has typed so far
    /// — which may not be a colour yet, and need not be.
    pub hex_edit: String,
    /// Per-layer thumbnail cache: (layer revision it was built at, texture).
    pub layer_thumbs: Vec<Option<(u64, egui::TextureHandle)>>,
    /// Inline layer rename in progress: (index, edit buffer).
    pub renaming: Option<(usize, String)>,
    /// The Layers palette's colour popup: `Some(row)` = the swatch/picker
    /// window is open for that row (owner 2026-08-21: set a layer's label
    /// colour with the picker right there, no click chain).
    pub layer_colour_pick: Option<usize>,
    /// PA-001: the Layers palette's Paper row is the highlighted one. The
    /// paper is NOT a layer, so this is a palette highlight beside
    /// `doc.active` rather than a second active-layer authority — nothing
    /// downstream ever sees "the active layer is the paper". Selecting any
    /// layer clears it (`AppCmd::SelectLayer` and the row click both do).
    pub paper_selected: bool,
    /// Doc revision at the last successful save/open/new — dirty tracking.
    saved_revision: u64,
    /// Structural page changes (add/delete/reorder) revisions can't see.
    pages_dirty: bool,
    /// Last window title applied (main polls `desired_title` against this).
    pub last_title: String,
    /// WM_CLOSE arrived; `main::pump_commands` runs the save prompt outside
    /// the wndproc borrow.
    pub close_requested: bool,
    /// A DOCUMENT close was asked for (the tab ×). Carries the tab index
    /// and is answered in `pump_commands`, where a save prompt can run — the
    /// wndproc cannot hold a modal dialog while `&mut App` is alive.
    pub close_doc_requested: Option<usize>,
    /// Caption-button request from the egui menu bar (custom title bar);
    /// executed by `main::pump_commands` outside the wndproc borrow.
    pub caption_cmd: Option<CaptionCmd>,
    /// The menu-bar drag strip asked to start a window move (WM_NCLBUTTONDOWN
    /// HTCAPTION); executed by `main::pump_commands` — the system move loop
    /// pumps messages, so it must not run inside WM_PAINT/render.
    pub drag_window: bool,
    /// IsZoomed(hwnd), tracked in WM_SIZE — picks the maximize vs restore
    /// glyph on the custom title bar.
    pub win_maximized: bool,
    /// `--gpu-dabs`: strokes rasterize on the GPU (P1). Per-stroke routing:
    /// wash/texture/exotic brushes fall back to the CPU reference path.
    pub gpu_dabs: bool,
    /// The live GPU dab stroke: the full drained dab list (kept for the
    /// canary-mismatch CPU repair), the tip mode, and HUD counters.
    pub dab_stroke: Option<DabStrokeApp>,
    /// "dab: gpu|cpu" + last readback ms — the F1 HUD line.
    pub dab_path_last: String,
    /// The smudge oracle's ctx allocation (`Box<(*mut Renderer, usize)>`
    /// as raw — renderer + stroke layer), installed for GPU smudge strokes
    /// (#0.1 part 3) and freed in `finish_gpu_dab_stroke`. Raw pointer, not
    /// a field borrow: the oracle fires inside `brush.sample`, where
    /// `self.brush` is already mutably borrowed.
    pub dab_smudge_ctx: Option<*mut core::ffi::c_void>,
    /// The internal image clipboard (TRIAGE 131): the last Cut/Copy's
    /// lifted pixels, full fix15 fidelity plus the original coordinates
    /// (Paste returns to them; the OS DIB clipboard is the lossy fallback
    /// for OTHER apps' content coming in).
    pub clipboard: Option<mn_core::FloatSource>,
    /// Tab: hide every palette for a clean drawing view (CSP's Tab).
    pub panels_hidden: bool,
    /// UI-032, Shift+Tab: hide the top bar and the status bar too — canvas
    /// and nothing else.
    ///
    /// **Neither hide is persisted, on purpose** (CSP's own anti-lockout
    /// design): a restart always comes back with the chrome. That matters
    /// more here than in CSP, because the top bar IS this window's title
    /// bar — the drag strip and the – □ × buttons live in it (main.rs
    /// `nc_calc_size` gives the window no native caption). While it is
    /// hidden the ways back are Shift+Tab, Esc, Alt+Space and Alt+F4.
    pub chrome_hidden: bool,
    pub recent: Vec<PathBuf>,
    pub select_mode: SelectMode,
    /// The persistent selection-combine mode (SE-022): the 4-way Tool
    /// Settings choice a held modifier OVERRIDES (Shift=Add, Alt=Subtract,
    /// Shift+Alt=Intersect — the owner's everyday path).
    pub sel_op: mn_core::SelectionOp,
    pub fill_opts: FillOpts,
    /// What `fill_opts.auto` measured on the last fill — the Tool Property
    /// shows it in place of the two numeric rows it drives. Session-only:
    /// it describes one click's artwork, not a setting.
    pub fill_auto: Option<mn_core::AutoFill>,
    /// Fill-tool sub tool: click / FI-003 enclose-and-fill / FI-004 lasso fill.
    pub fill_mode: FillMode,
    /// In-progress Enclose/Lasso fill drag, canvas coords. Same shape as
    /// `select_drag` — the overlay traces it while the pen is down.
    pub fill_drag: Option<Vec<(f32, f32)>>,
    /// Auto-select wand parameters (its own Tool Property, CSP-style).
    pub wand_opts: FillOpts,
    /// The Tone tool's Tool Property: the screen to lay down, plus its own
    /// copy of the flood options the click uses to find the region.
    pub tone_opts: crate::cmd::ToneToolOpts,
    /// Frame-tool sub tool (divide-into-folder / divide-in-place / rectangle).
    pub frame_mode: FrameMode,
    /// Move-tool sub tool (hand pans, rotate spins the view).
    pub pan_mode: PanMode,
    /// Eyedropper Tool Property: which layers it samples, how big a box it
    /// averages, and whether the picker ring draws (E-014/016/017).
    pub eyedrop_opts: EyedropOpts,
    /// What Ctrl+Shift+D restores (stashed on Deselect).
    pub last_selection: Option<Selection>,
    /// Selection Launcher expand/shrink amount (px), remembered between uses.
    pub sel_px: u32,
    /// doc.revision the active page's Pages-panel thumbnail was built at
    /// (0 = never; bumps once per content revision, panels and all).
    pub pages_thumb_rev: u64,
    /// Fresh-name counter for live page thumbnails (see `thumb_of_current`).
    pub pages_thumb_seq: u64,
    /// In-progress view-rotate drag (Rotate sub tool): last pointer angle
    /// around the canvas-area centre.
    rotate_drag: Option<f32>,
    /// In-progress selection drag, canvas coords (rect: [anchor, current];
    /// lasso: the polyline so far).
    pub select_drag: Option<Vec<(f32, f32)>>,
    /// In-progress move of the selected pixels: (start, current) canvas coords.
    pub select_moving: Option<((f32, f32), (f32, f32))>,
    /// L-001/L-002: the magnetic-lasso trace in progress. Owns its edge-cost
    /// cache, so dropping it frees the lot — always clear it rather than
    /// leaving a stale trace behind a tool switch.
    pub magnetic: Option<mn_core::magnetic::Lasso>,
    /// Magnetic lasso snap range, px (Tool Property). How far off the cursor
    /// the wire may wander looking for an edge.
    pub magnetic_reach: i32,
    /// Divide-tool drag, canvas coords (start, current — current is axis-snapped).
    pub frame_drag: Option<((f32, f32), (f32, f32))>,
    /// Operation-tool sub tool: reshape objects, or S-001's layer pick.
    pub object_mode: ObjectMode,
    /// S-001: the layer kinds the pick refuses to land on.
    pub pick_exclude: PickExclude,
    /// Object tool: the selected frame as (layer, frame) indices.
    pub object_sel: Option<(usize, usize)>,
    /// Object tool: live move/reshape drag.
    pub object_drag: Option<ObjectDrag>,
    /// Cut-tool gutter widths, mm, (horizontal, vertical) — one pair per cut
    /// sub tool, seeded from the owner's CSP install (70/230 px and 40/54 px
    /// at 600 dpi).
    pub gutter_folder_mm: (f32, f32),
    pub gutter_border_mm: (f32, f32),
    /// Object tool, CSP "Keep gutters aligned": with it on (All), dragging a
    /// panel border moves the FACING border of the panel across the gutter
    /// too, so the gap keeps its width. Off (None) resizes the one edge and
    /// lets the gutter narrow. Session state — CSP does not persist it either.
    pub gutter_align_all: bool,
    /// Create-frame options (CSP Rectangle-frame Tool Property).
    pub frame_border_mm: f32,
    pub frame_draw_border: bool,
    pub frame_fill_inside: bool,
    /// Polyline-frame vertices placed so far (canvas coords).
    pub frame_poly: Option<Vec<(f32, f32)>>,
    /// Frame-border-pen freehand trail (canvas coords).
    pub frame_pen: Option<Vec<(f32, f32)>>,
    /// TRIAGE 128 (`FB-026`/`FB-022`): what a cut does to the panel's
    /// CONTENTS. A divide-tool Tool Property, sticky like every other one.
    pub frame_divide_contents: DivideContents,
    /// TRIAGE 129 (`FB-023`–`025`): the equal-division grid the "Divide
    /// equally" button applies, and whether it runs along the panel's own
    /// slant (CSP *Fit to Side Direction of Frame*).
    pub frame_div_grid: (usize, usize),
    pub frame_div_fit_side: bool,
    /// TRIAGE 127 (`FB-053`/`FB-054`): the curve rulers currently derived
    /// from border-as-ruler frame folders. Kept so the sync can retract
    /// exactly what it added and never eat a hand-drawn curve ruler.
    pub frame_rulers: Vec<mn_core::CurveRuler>,
    /// Balloon-tool sub-mode (Tool Property buttons).
    pub balloon_mode: BalloonMode,
    /// Figure-tool sub-mode (line / rect / ellipse / polygon).
    pub figure_mode: FigureMode,
    /// Figure ▸ fill the closed shape with the drawing colour.
    pub figure_fill: bool,
    /// In-progress figure drag, canvas coords `[anchor, current]`
    /// (line/rect/ellipse).
    pub figure_drag: Option<((f32, f32), (f32, f32))>,
    /// Figure ▸ Polygon: the placed vertices so far.
    pub figure_poly: Option<Vec<(f32, f32)>>,
    /// Figure ▸ Stream line: parameters the next drag generates with
    /// (session-only, like every other Tool Property knob here).
    pub figure_stream: crate::cmd::FigureLineOpts,
    /// Figure ▸ Saturated line: same, for the focus-line drags.
    pub figure_focus: crate::cmd::FigureLineOpts,
    /// Gradient-tool sub-mode (which two colours the ramp spans).
    pub grad_mode: GradMode,
    /// `G-008`/`G-013`/`G-014`: the interior colour stops the Tool Property
    /// colour bar authors. The two END colours still come from `grad_mode`.
    pub grad_mid: mn_core::MidStops,
    /// `G-002`/`G-004`/`G-005`/`G-006`/`G-009`/`G-015`: flip, edge process,
    /// dithering, start-from-centre, mixing mode and mixing rate.
    pub grad_opts: mn_core::RampOpts,
    /// Which interior stop the TOOL's colour bar has selected (index into
    /// `grad_mid`), and whether the pointer is dragging it right now.
    pub grad_stop_sel: Option<usize>,
    pub grad_stop_drag: bool,
    /// The same, for the LIVE gradient layer's bar. Separate because both
    /// bars render in the same frame whenever the Gradient tool is active
    /// on a gradient layer — one shared index would let a drag on either
    /// bar move a stop on the other.
    pub grad_live_sel: Option<usize>,
    pub grad_live_drag: bool,
    /// `G-011`/`G-012`: the saved gradient set and which entry the panel
    /// has highlighted. Persisted through `ui.txt`'s `gradients=` line.
    pub grad_set: mn_core::GradientSet,
    pub grad_set_sel: usize,
    /// NL-006's switch (TRIAGE 137): Fill/Gradient create or retarget a
    /// LIVE layer instead of painting. Session-only v1 (not persisted per
    /// sub tool yet — recorded in DECISIONS 8.50).
    pub fill_live: bool,
    /// In-progress gradient ramp drag, canvas coords `[start, end]`.
    pub grad_drag: Option<((f32, f32), (f32, f32))>,
    /// In-progress balloon-tool drag, canvas coords. Ellipse/Round/Tail keep
    /// `[anchor, current]`; Draw appends the freehand trail.
    pub balloon_drag: Option<Vec<[f32; 3]>>,
    /// Object tool: the selected balloon as (layer, balloon) indices.
    pub balloon_sel: Option<(usize, usize)>,
    /// Object tool: live balloon move/reshape drag.
    pub balloon_obj_drag: Option<BalloonObjDrag>,
    /// KB-020 (TRIAGE 172, owner HIGH): a live Ctrl+Alt+drag brush-size
    /// gesture — (screen-x anchor, multiplier at press).
    pub size_drag: Option<(f32, f32)>,
    /// KB-022: a Ctrl+drag temporary Object grab from a drawing tool —
    /// the drags run through the tool-independent handlers; cleared at
    /// release. The tool never changes.
    pub temp_object: bool,
    /// The eye-solo's restore snapshot (RF-001/r113): the visibility
    /// vector as it was before the solo press. Cleared on page switch.
    pub eye_solo_backup: Option<Vec<bool>>,
    /// Object tool: the canvas point the current selection was picked at
    /// (set on every click-select; the cycle's anchor). Falls back to the
    /// selection's bbox centre when the selection did not come from a
    /// click.
    pub object_pick: Option<(f32, f32)>,
    /// Balloon outline width (mm) for NEW balloon layers, and tail base width
    /// (mm) for new tails — Tool Property. Defaults are guesses at CSP's;
    /// owner should check.
    pub balloon_border_mm: f32,
    pub balloon_tail_mm: f32,
    /// `C-039`–`048` "create balloon options": the colours, opacities and
    /// screened fill a NEW bubble is born with. Defaults are black-on-white,
    /// so a session that never opens the section draws what it always drew.
    pub balloon_ink: mn_core::BalloonInk,
    /// `B-005`/`B-006` for NEW tails: the shape (spoken wedge / thought
    /// chain / shout spike) and how far the tail bows sideways.
    pub balloon_tail_kind: mn_core::TailKind,
    pub balloon_tail_bend: f32,
    /// Buffered balloon ink while an Object-tool bar is being dragged (one
    /// undo step per interaction, like `border_edit`).
    pub ink_edit: Option<mn_core::BalloonInk>,
    /// Layer Property border-thickness drag in progress (mm) — committed as
    /// ONE undo step when the drag ends.
    pub border_edit: Option<f32>,
    /// Buffered "correct line width" multiplier while the Object-tool row is
    /// being dragged (one undo step per interaction, like `border_edit`).
    pub width_edit: Option<f32>,
    /// Buffered tone params while a Layer-Property control is being dragged
    /// (one undo step per interaction, like `border_edit`).
    pub tone_edit: Option<mn_core::ToneParams>,
    /// Buffered border-effect params (`LP-003`) while the width bar is being
    /// dragged — same one-undo-step-per-interaction rule as `tone_edit`.
    pub edge_edit: Option<mn_core::EdgeParams>,
    // --- text ---------------------------------------------------------------
    /// DirectWrite engine; `None` when init failed (text tool then refuses
    /// with a status line instead of crashing).
    pub text_engine: Option<mn_text::TextEngine>,
    /// Live editing session (T tool). See `text_edit.rs` for the model.
    pub text_edit: Option<TextEditState>,
    /// T-tool press gesture (caret drag / box drag).
    pub text_gesture: Option<TextGesture>,
    /// Object tool: selected text as (layer, item) indices.
    pub text_sel: Option<(usize, usize)>,
    /// Object tool: live text move/resize/rotate drag.
    pub text_obj_drag: Option<TextObjDrag>,
    /// Object tool: Tool-Property value-bar drag on the selected text item
    /// (size/char-space/line/edge) — live preview, ONE undo step on release
    /// (auditor round 34). None while a text edit session is up.
    pub text_bar_drag: Option<TextBarDrag>,
    /// Transform modal: live transform drag (Enter commits, Esc cancels).
    pub transform_drag: Option<TransformDrag>,
    /// Transform Tool Property "Keep aspect ratio" (CSP 縦横比固定, on by
    /// default): corner and side handles scale both axes by one ratio.
    /// Shift does the same for a single drag.
    pub transform_keep_aspect: bool,
    /// New-text defaults (Tool Property). Font resolves to 源暎アンチック v5
    /// when installed; size in pt at the document dpi; manga default vertical.
    pub text_font: String,
    pub text_size_pt: f32,
    pub text_vertical: bool,
    /// Tool Property furigana input buffer (TX-062). Session-only: a reading
    /// belongs to the characters it annotates, not to the tool.
    pub text_ruby: String,
    /// Edge (フチ) width in mm for new text; 0 = off.
    pub text_outline_mm: f32,
    pub text_outline_color: [u8; 3],
    /// Round-34 typography defaults for new text (CSP Text Tool parity);
    /// seeded into every `start_new_text` item and applied live to the item
    /// under edit / Object selection by Tool Property.
    pub text_align: Align,
    pub text_frame_align: FrameAlign,
    pub text_letter_pt: f32,
    pub text_line: LineSpacing,
    /// TX-styles: the work style newly created text will carry.
    pub text_style_new: Option<String>,
    /// TX-styles editor window + its working copy (dropped on close).
    pub text_styles_open: bool,
    pub styles_draft: Vec<mn_core::text::TextStyle>,
    /// Auto 縦中横 for new text (TX-062): the longest alphanumeric run that
    /// stands upright by itself, 0 = off. 2 out of the box — a page number
    /// or a 22時 is the case this exists for, and CSP ships the same number.
    pub text_auto_tcy: u8,
    /// Font-list panel state (CSP Font list: search + recently used).
    pub font_search: String,
    pub font_picker_open: bool,
    /// Recently used font families, newest first, max 10 (CSP's Font list).
    pub recent_fonts: Vec<String>,
    /// False after the pointer leaves the window / pen leaves hover range —
    /// hides the brush cursor ring instead of parking it at the last spot.
    pub pointer_visible: bool,
    /// (display name, path) — names come from the .myb JSON when present
    /// (the CSP imports carry theirs, Japanese included).
    pub presets: Vec<(String, PathBuf)>,
    pub selected_preset: Option<usize>,
    /// The preset whose Rename box is open in the Sub Tool list's
    /// right-click menu, and the name being typed. Re-seeded whenever the
    /// menu opens on a different preset, so a half-typed name can never land
    /// on the next brush you right-click.
    pub brush_rename_edit: Option<(PathBuf, String)>,
    /// Pen and Eraser keep SEPARATE sub tools (owner order): each remembers
    /// its own preset across tool switches.
    pub pen_preset: Option<usize>,
    pub eraser_preset: Option<usize>,
    pub hud_open: bool,
    /// Help ▸ Report Bug / Feature Request — the feedback window.
    pub feedback_open: bool,
    pub doc_path: Option<PathBuf>,
    /// One line of feedback for the top bar (last save/open/brush switch, or the
    /// error that stopped one).
    pub status: String,
    /// The status line is a REFUSAL, not news — painted in the warning
    /// colour. Set by `set_error`, cleared by `set_status`.
    pub status_warn: bool,
    pub diag: Diag,

    /// Filled by UI widgets and shortcuts, drained by `main::pump_commands`.
    pub cmds: VecDeque<AppCmd>,

    /// Press routing, decided at button/pen down.
    pub mouse_owner: Owner,
    pub pen_owner: Owner,
    /// Last known client-pixel mouse position (for `WM_SETCURSOR`, which does
    /// not carry one).
    pub last_pointer: (i32, i32),
    /// What the pen device reports about itself — see [`PenHealth`]. Fed by
    /// [`App::note_pen_report`] from the `WM_POINTER` arm.
    pub pen: PenHealth,

    stroke: Option<StrokeStats>,
    /// Active touch contacts (id → client px). Touch never draws — it pans and
    /// pinch-zooms the view, which also makes resting palms harmless.
    pub touch: HashMap<u32, (f32, f32)>,
    pub touch_probe: TouchProbe,
    /// The two-finger twist accumulator (2026-08-19 feel fix): rotation is
    /// inert until the cumulative twist passes a threshold, then live for
    /// the whole gesture, and the quarter-snap DERIVES the displayed angle
    /// from the raw one instead of writing it back — an absolute-set snap
    /// is what pinned slow twists at the quarters (the owner's "one stable
    /// finger + circling" gesture was the only escape; see
    /// research/touch-rotation.md).
    touch_twist: TouchTwist,
    /// Client-pixel anchor + viewport pan at the start of a pan drag.
    pan_drag: Option<([f32; 2], [f32; 2])>,
    pub space_down: bool,
    /// Set when something changed and a WM_PAINT should be requested.
    needs_redraw: bool,
    /// Monotonic clock feeding per-page content revisions (work-folder
    /// skip-write hints). Kept at or above `doc.revision`.
    page_clock: u64,
    /// Work-folder bookkeeping: next free page id (0 = not folder-backed) and
    /// the file names the last index/save managed (cleanup set).
    folder_next_id: u32,
    folder_managed: Vec<String>,
    /// Open documents, in TAB ORDER. The ACTIVE slot is `None` because its
    /// contents live inline on this struct (see `app::session`); every other
    /// slot holds a parked `DocSession`. Empty until the first tab op, which
    /// is why `doc_count()` floors at 1.
    pub docs: Vec<Option<DocSession>>,
    /// Which slot of `docs` is the live one.
    pub active_doc: usize,
    /// The one dock tree (ui/dock.rs, docs/DOCKING-2.md): palettes AND the
    /// canvas pane. Floating palettes live inside it as window surfaces.
    pub dock: crate::ui::dock::DockTree,
}

/// The two-finger twist accumulator (see [`App::touch_twist`]). All the
/// state the 2026-08-19 rotate-feel fix needs; reset on every contact-set
/// change (down/up), consumed by `touch_move`.
#[derive(Default)]
struct TouchTwist {
    /// Viewport rotation when the pair formed — the raw angle's anchor.
    start_rad: f32,
    /// Raw (unsnapped) rotation accumulated since the pair formed.
    raw: f32,
    /// Rotation is inert until the cumulative twist passes
    /// [`TOUCH_TWIST_THRESHOLD`], then live for the rest of the gesture.
    live: bool,
    /// The quarter the snap currently holds (None = free rotation).
    holding: Option<f32>,
}

impl TouchTwist {
    fn reset(&mut self, cur: f32) {
        *self = TouchTwist {
            start_rad: cur,
            ..Default::default()
        };
    }
}

/// Cumulative twist that flips rotation live for the gesture — pinch
/// noise stays under it (the OpenLayers PinchRotate pattern, tuned for a
/// canvas rather than a map's conservative 17°).
const TOUCH_TWIST_THRESHOLD: f32 = 4.0f32.to_radians();
/// The displayed angle snaps to a quarter within [`SNAP_ENGAGE`] of it
/// and holds until the raw angle passes [`SNAP_RELEASE`] — hysteresis, so
/// the boundary never dithers and leaving the magnet is always possible
/// at ANY twist speed.
const SNAP_ENGAGE: f32 = 2.5f32.to_radians();
const SNAP_RELEASE: f32 = 4.0f32.to_radians();

/// The in-flight selection-paint direction: what the stroke does to the
/// selection at release (selection pen adds, eraser subtracts; Quick Mask
/// maps Pen/Eraser the same way).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SelPaintOp {
    Add,
    Subtract,
}

/// SL-001: the Layers palette filter's layer-type row. `All` is the
/// off position — the palette is unfiltered and nothing about it moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LayerFilterKind {
    #[default]
    All,
    /// Painted layers: not a folder, not one of the vector kinds.
    Raster,
    /// Any folder header (frame folders included).
    Folder,
    /// Frame (koma) layers — the folder header carries the FrameSet.
    Frame,
    Balloon,
    Text,
}

impl LayerFilterKind {
    pub const ALL: [LayerFilterKind; 6] = [
        LayerFilterKind::All,
        LayerFilterKind::Raster,
        LayerFilterKind::Folder,
        LayerFilterKind::Frame,
        LayerFilterKind::Balloon,
        LayerFilterKind::Text,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LayerFilterKind::All => "all types",
            LayerFilterKind::Raster => "raster",
            LayerFilterKind::Folder => "folder",
            LayerFilterKind::Frame => "frame",
            LayerFilterKind::Balloon => "balloon",
            LayerFilterKind::Text => "text",
        }
    }
}

/// MT-032 (CSP's paste-size vocabulary, named after the job): what
/// "paste this at the right size" means for a material. The DEFAULT
/// keeps r74's owner-approved down-fit verbatim; the rest are additive
/// choices in the Material palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MaterialPasteSize {
    /// r74's spec: uniform down-fit into the panel, never crop, never
    /// scale up.
    #[default]
    FitPanel,
    /// CSP 貼り付け後に調整: original size, centred on the target, the
    /// transform drag armed.
    AdjustAfter,
    /// CSP 全体に拡大 (backgrounds): uniform COVER — fills the target,
    /// overflow cropped by the panel seal.
    ExpandFull,
    /// CSP スケールに合わせる (sound effects): uniform CONTAIN, up or
    /// down — the whole material stays visible inside the panel.
    FitToScale,
    /// Stretch to the destination rect exactly (non-uniform — for
    /// patterns where the seam matters more than the proportion).
    ToDestination,
}

/// MT-034: where a pasted material's layer sits inside the panel folder.
/// Only panel-targeted pastes that CREATE a layer (the pointer's panel,
/// not the one owning the active layer — r74's rule 2) apply this; leaving
/// the folder would defeat the panel mask, so page-level top/bottom are
/// NOT offered. ("Under the active layer" is unreachable by construction:
/// when the active layer lives in the target panel, r74's rule 1 stamps
/// it in place and no layer is created.)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MaterialLayerOrder {
    /// The current behavior: topmost child of the panel folder.
    #[default]
    Above,
    /// The panel folder's bottom child.
    BottomOfPanel,
}

/// Owner-correction probe (2026-08-19): two-finger rotate is reported
/// broken on his TOUCHSCREEN. These are wndproc-entry counters — if
/// touch stays ZERO while he touches the screen, the driver never
/// delivers WM_POINTER touch events (pen-proximity palm rejection is
/// the prime suspect: test with the pen well away from the glass);
/// if they climb but nothing rotates, the bug is ours, downstream.
#[derive(Default)]
pub struct TouchProbe {
    pub enabled: bool,
    /// [down, update, up] per device class.
    pub pen: [u32; 3],
    pub touch: [u32; 3],
    pub mouse: [u32; 3],
    pub other: [u32; 3],
    pub last: String,
}

impl TouchProbe {
    pub fn bump(&mut self, dev: usize, mi: usize) {
        let slot = match dev {
            0 => &mut self.pen,
            1 => &mut self.touch,
            2 => &mut self.mouse,
            _ => &mut self.other,
        };
        slot[mi] += 1;
        self.last = format!(
            "{} {}",
            ["down", "update", "up"][mi],
            ["pen", "touch", "mouse", "other"][dev.min(3)]
        );
    }
}

impl App {
    pub fn new(renderer: Renderer, client: (u32, u32), ppp: f32) -> Self {
        let prefs = Prefs::load();
        // BEFORE `Shell::new`, which is what calls `theme::apply` on the
        // fresh context: set the palette first and the first frame is
        // already in the user's theme, with no flash of the default.
        crate::ui::theme::set_by_name(&prefs.theme);
        // Same reason, same frame: the icon accent switch is read by the
        // painters, not passed to them.
        crate::ui::icons::set_accents(prefs.icon_colours);
        // `Document::default()` is `DEFAULT_SIZE`; the preference IS that
        // constant until the owner changes it in the panel.
        let doc = Document::new(prefs.new_canvas.0, prefs.new_canvas.1);
        let new_doc_draft = NewComicDraft {
            setup: prefs.new_preset_setup(),
            ..NewComicDraft::default()
        };
        let shell = Shell::new(&renderer, ppp);
        let layout = UiLayout::load();
        // Docking 2: the single tree, else the one-time migration of the
        // legacy two-column layout, else the default. The legacy keys stay
        // on disk untouched (downgrade safety); this build only writes
        // `dock_tree=` from here on.
        let dock = if !layout.dock_tree.is_empty() {
            crate::ui::dock::from_json_tree(&layout.dock_tree)
        } else if !layout.dock_left.is_empty() || !layout.dock_right.is_empty() {
            let win_w = crate::app::layout::WinGeom::parse(&layout.win)
                .map_or(1280.0, |g| (g.w as f32 / ppp).max(400.0));
            // A side the old build never saved migrates as its default.
            let l = if layout.dock_left.is_empty() {
                crate::ui::dock::to_json(&crate::ui::dock::default_left())
            } else {
                layout.dock_left.clone()
            };
            let r = if layout.dock_right.is_empty() {
                crate::ui::dock::to_json(&crate::ui::dock::default_right())
            } else {
                layout.dock_right.clone()
            };
            crate::ui::dock::merge_columns(&l, &r, layout.left_w, layout.right_w, win_w)
                .unwrap_or_else(crate::ui::dock::default_tree)
        } else {
            crate::ui::dock::default_tree()
        };
        let presets = scan_presets();
        println!("[ui] {} brush presets found", presets.len());
        let root = brushes_root();
        let texture_names = scan_textures(root.as_deref());
        let (engine, selected_preset) = default_engine(&presets);
        println!("[ui] brush: {}", engine.name());
        // Strength 0 = exact passthrough; the slider raises it.
        let brush = Stabilizer::new(Taper::new(engine), 0.0);

        let mut app = Self {
            doc,
            brush,
            input_resampler: crate::input_path::InputResampler::new(),
            renderer,
            viewport: Viewport::default(),
            startup_fit_pending: true,
            startup_fit_last: (0.0, 0.0),
            shell,
            tool: Tool::Pen,
            main_color: [0.0, 0.0, 0.0],
            sub_color: [1.0, 1.0, 1.0],
            slot: Slot::Main,
            props_current: ToolProps::default(),
            props: HashMap::new(),
            dab_clamp_warned: None,
            texture_names,
            brushes_root: root,
            curve_setting: Default::default(),
            curve_sensor: Default::default(),
            curve_edit_points: Vec::new(),
            curve_drag: None,
            curve_overrides: HashMap::new(),
            mirror_x: false,
            mirror_y: false,
            wrap_x: false,
            wrap_y: false,
            swatches: load_swatches().unwrap_or_else(default_swatches),
            color_history: layout
                .color_history
                .iter()
                .filter_map(|h| mn_core::palette::parse_hex(h))
                .collect(),
            page: None,
            pages: vec![PageEntry::active()],
            page_index: 0,
            story: String::new(),
            binding_right: true,
            seed_frame_folder: true,
            new_doc_open: false,
            new_doc_draft,
            work_settings_open: false,
            work_settings_draft: WorkSettingsDraft::default(),
            canvas_size_open: false,
            prefs_open: false,
            prefs_focus: None,
            prefs_tab: 0,
            prefs_search: String::new(),
            ui_scale_apply: None,
            adjust_draft: None,
            adjust_preview: None,
            goto_page_open: false,
            goto_page_value: 1,
            spread_op: None,
            spread_gap: 0,
            spread_delete_empty: true,
            export_all_open: false,
            export_all_prefix: String::new(),
            export_all_range: false,
            export_all_from: 1,
            export_all_to: 1,
            export_all_split: false,
            export_all_text: false,
            export_all_dpi: 0,
            export_all_colour: mn_core::LayerExpression::Colour,
            export_all_crop: mn_core::export::ExportCrop::Paper,
            export_all_px_height: 0,
            mask_show_area: false,
            tone_show_area: false,
            mask_edit: false,
            quick_mask: false,
            sel_paint: None,
            pages_cell_w: 100.0,
            pages_fit: false,
            pages_cell_px: 112.0,
            pages_thumb_px: 0.0,
            preview_order: Default::default(),
            reader: Default::default(),
            frame_order: None,
            frame_order_rev: 0,
            frame_order_show: false,
            frame_delete_armed: None,
            story_sel: None,
            story_move_to: 1,
            hwnd: 0,
            workspaces: serde_json::from_str(&layout.workspaces).unwrap_or_default(),
            workspace_current: layout.workspace_current.clone(),
            workspace_draft: String::new(),
            workspace_open: false,
            comp_selected: None,
            comp_multi: Vec::new(),
            actions: Vec::new(),
            action_selected: None,
            action_recording: None,
            action_running: false,
            action_renaming: None,
            action_picker: None,
            action_step_edit: None,
            comp_last_state: None,
            comp_name_draft: String::new(),
            comp_added_visible: true,
            filter_draft: None,
            gen_open: false,
            gen_inited: false,
            gen_focus: true,
            gen_a: 0.0,
            gen_b: 0.0,
            gen_c: 0.0,
            gen_d: 0.0,
            gen_count: 48,
            gen_width: 5.0,
            gen_jitter: 0.4,
            gen_seed: 1,
            gen_sel: None,
            gen_edit: None,
            object_pick: None,
            eye_solo_backup: None,
            size_drag: None,
            temp_object: false,
            gen_drag: None,
            fill_lattice_drag: None,
            global_pressure: crate::app::prefs::parse_pressure_curve(&prefs.pressure_curve)
                .unwrap_or_default(),
            pen_wizard_open: false,
            pen_wizard_gamma: 1.0,
            pen_wizard_samples: Vec::new(),
            anti_overflow_cache: None,            anti_overflow: false,
            quick_query: String::new(),
            quick_pins: layout
                .quick_pins
                .split('')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            cmdpal_open: false,
            cmdpal_query: String::new(),
            cmdpal_sel: 0,
            cmdpal_recent: Vec::new(),
            cmdpal_entries: Vec::new(),
            story_open: false,
            story_docs: Vec::new(),
            story_bufs: Vec::new(),
            story_find: String::new(),
            story_repl: String::new(),
            story_ignore_case: false,
            canvas_size_draft: CanvasSizeDraft {
                w: prefs.new_canvas.0,
                h: prefs.new_canvas.1,
                anchor: ResizeAnchor::Center,
                all_pages: false,
            },
            print_margin_info: false,
            expression: mn_core::Expression::Mono,
            spine_mm: 0.0,
            cover: None,
            template_page: None,
            profile: None,
            preflight_findings: None,
            preflight_rev: 0,
            preflight_page: 0,
            preflight_stale: false,
            material_folders: {
                let mut v = Vec::new();
                if let Some(d) = Self::materials_default_folder() {
                    v.push(d);
                }
                v.extend(layout.material_folders.iter().map(std::path::PathBuf::from));
                v
            },
            material_folder_names: Vec::new(),
            materials: Vec::new(),
            material_thumbs: std::collections::HashMap::new(),
            material_uses: serde_json::from_str(&layout.material_uses).unwrap_or_default(),
            layer_search: String::new(),
            layer_filter_kind: LayerFilterKind::All,
            layer_filter_ref_only: false,
            layer_filter_no_draft: false,
            layer_filter_this_frame: false,
            layer_filter_open: false,
            layer_filter_label: None,
            material_search: String::new(),
            material_sort_uses: false,
            material_tile: false,
            material_tone: false,
            material_size: MaterialPasteSize::default(),
            material_order: MaterialLayerOrder::default(),
            material_tag_edit: None,
            material_tree: Vec::new(),
            material_filter: crate::app::materials::MaterialFilter::All,
            material_tree_closed: std::collections::HashSet::new(),
            material_tree_show: true,
            material_selected: None,
            material_thumb_px: crate::app::materials::THUMB_STEPS[1],
            material_thumb_lru: std::collections::HashMap::new(),
            material_thumb_tick: 0,
            refs: crate::ui::refs::RefBank::from_layout(&layout.references),
            fit_sticky: false,
            ruler_pending: None,
            symmetric_lines: 2,
            ruler_drag: None,
            ruler_move: None,
            ruler_lock: Default::default(),
            curve_pending: None,
            last_click: None,
            nav_last_surface: (0, 0),
            nav_thumb: None,
            nav_thumb_rev: 0,
            detail_open: false,
            prop_detail_open: false,
            prop_hidden: std::collections::BTreeSet::new(),
            dock,
            brush_previews: HashMap::new(),
            preview_budget: 0,
            page_pane_budget: 0,
            picker_hsv: [0.0, 0.0, 0.0],
            picker_rgb_cache: [0.0, 0.0, 0.0],
            hex_edit: String::new(),
            layer_thumbs: Vec::new(),
            renaming: None,
            layer_colour_pick: None,
            paper_selected: false,
            saved_revision: 0,
            pages_dirty: false,
            last_title: String::new(),
            close_requested: false,
            close_doc_requested: None,
            caption_cmd: None,
            drag_window: false,
            win_maximized: false,
            gpu_dabs: false,
            dab_stroke: None,
            dab_path_last: "cpu".into(),
            dab_smudge_ctx: None,
            clipboard: None,
            panels_hidden: false,
            recent: load_recent_n(prefs.recent_depth),
            chrome_hidden: false,
            select_mode: SelectMode::Rect,
            sel_op: mn_core::SelectionOp::Replace,
            fill_opts: FillOpts::default(),
            fill_auto: None,
            fill_mode: FillMode::Click,
            fill_drag: None,
            wand_opts: FillOpts::default(),
            tone_opts: crate::cmd::ToneToolOpts::default(),
            frame_mode: FrameMode::DivideFolder,
            pan_mode: PanMode::Hand,
            eyedrop_opts: EyedropOpts::default(),
            last_selection: None,
            sel_px: 2,
            pages_thumb_rev: 0,
            pages_thumb_seq: 0,
            rotate_drag: None,
            select_drag: None,
            select_moving: None,
            magnetic: None,
            magnetic_reach: mn_core::magnetic::DEFAULT_REACH,
            frame_drag: None,
            object_mode: ObjectMode::default(),
            pick_exclude: PickExclude::default(),
            object_sel: None,
            object_drag: None,
            // The owner's CSP values at his 600 dpi workspace: divide-folder
            // 70/230 px, divide-border 40/54 px (research/csp-tools.json).
            gutter_folder_mm: (2.96, 9.74),
            gutter_border_mm: (1.69, 2.29),
            gutter_align_all: true,
            // His Rectangle-frame border is 15 px at 600 dpi.
            frame_border_mm: 0.64,
            frame_draw_border: true,
            frame_fill_inside: true,
            frame_poly: None,
            frame_pen: None,
            frame_divide_contents: DivideContents::default(),
            // 2x2 is the commonest first cut of a manga page.
            frame_div_grid: (2, 2),
            frame_div_fit_side: false,
            frame_rulers: Vec::new(),
            balloon_mode: BalloonMode::Ellipse,
            figure_mode: FigureMode::Line,
            figure_fill: false,
            figure_drag: None,
            figure_poly: None,
            figure_stream: crate::cmd::FigureLineOpts::stream_default(),
            figure_focus: crate::cmd::FigureLineOpts::focus_default(),
            grad_mode: GradMode::FgToBg,
            grad_mid: Default::default(),
            grad_opts: Default::default(),
            grad_stop_sel: None,
            grad_stop_drag: false,
            grad_live_sel: None,
            grad_live_drag: false,
            // Empty `gradients=` means a ui.txt written before the set
            // existed (or a first run): seed the starter gradients. `[]` is
            // a user who deleted them all and gets to keep that.
            grad_set: if layout.gradients.trim().is_empty() {
                mn_core::GradientSet::starter()
            } else {
                mn_core::GradientSet::from_json(&layout.gradients)
            },
            grad_set_sel: 0,
            fill_live: false,
            grad_drag: None,
            balloon_drag: None,
            balloon_sel: None,
            balloon_obj_drag: None,
            balloon_border_mm: 0.3,
            balloon_tail_mm: 4.0,
            balloon_ink: mn_core::BalloonInk::default(),
            balloon_tail_kind: mn_core::TailKind::default(),
            balloon_tail_bend: 0.0,
            ink_edit: None,
            border_edit: None,
            width_edit: None,
            tone_edit: None,
            edge_edit: None,
            text_engine: None,
            text_edit: None,
            text_gesture: None,
            text_sel: None,
            text_obj_drag: None,
            text_bar_drag: None,
            transform_drag: None,
            transform_keep_aspect: true,
            text_font: String::new(),
            text_size_pt: prefs.text_size_pt,
            text_vertical: true,
            text_ruby: String::new(),
            text_outline_mm: 0.0,
            text_outline_color: [255, 255, 255],
            text_align: Align::default(),
            text_frame_align: FrameAlign::default(),
            text_letter_pt: 0.0,
            text_line: LineSpacing::default(),
            text_style_new: None,
            text_styles_open: false,
            styles_draft: Vec::new(),
            text_auto_tcy: 2,
            font_search: String::new(),
            font_picker_open: false,
            recent_fonts: layout.recent_fonts.clone(),
            layout,
            prefs,
            batch: batch::BatchOps::default(),
            pattern: pattern::PatternStudio::default(),
            vector_capture: None,
            vector_sel: None,
            vector_drag: None,
            autosave_rearm: None,
            autosave_op_seen: 0,
            pointer_visible: false,
            pen_preset: selected_preset,
            eraser_preset: None,
            presets,
            selected_preset,
            brush_rename_edit: None,
            hud_open: false,
            feedback_open: false,
            doc_path: None,
            status: String::new(),
            status_warn: false,
            diag: Diag::default(),
            cmds: VecDeque::new(),
            mouse_owner: Owner::None,
            pen_owner: Owner::None,
            last_pointer: (0, 0),
            pen: PenHealth::default(),
            stroke: None,
            touch: HashMap::new(),
            touch_twist: TouchTwist::default(),
            touch_probe: TouchProbe::default(),
            pan_drag: None,
            space_down: false,
            page_clock: 0,
            folder_next_id: 0,
            docs: Vec::new(),
            active_doc: 0,
            folder_managed: Vec::new(),
            needs_redraw: true,
        };
        app.materials_scan();
        // The Eraser tool's own default sub tool: the owner's CSP hard eraser,
        // else anything with "eraser" in it, else it shares the pen's brush.
        app.eraser_preset = app
            .presets
            .iter()
            .position(|(_, p)| p.ends_with("csp/eraser-hard.myb"))
            .or_else(|| {
                app.presets
                    .iter()
                    .position(|(n, _)| n.to_lowercase().contains("eraser"))
            });
        // Seed the Tool Property panel from the default brush's own readings —
        // except the SIZE, which the startup sub tool takes from ui.txt when
        // the user set one last session (`seed_size_px`), so the first stroke
        // after a relaunch is the size he left the brush at.
        app.props_current.size_px = match app.selected_preset {
            Some(i) => {
                let p = app.presets[i].1.clone();
                app.seed_size_px(&p)
            }
            None => app.engine().base_size_px(),
        };
        app.props_current.opacity = app.engine().base_opacity();
        app.props_current.min_size = app.engine().size_min_pct();
        {
            let (r, rm, ra) = app.engine().randomization();
            app.props_current.random = r;
            app.props_current.random_min = rm;
            app.props_current.random_abs = ra;
        }
        if let Some((px, min)) = app.engine().taper_hint() {
            app.props_current.taper_px = px;
            app.props_current.taper_min = min;
        }
        app.apply_props();
        app.apply_draw_state();
        app.viewport = fitted_viewport(&app.doc, client, app.prefs.fit_margin);
        app.saved_revision = app.doc.revision;
        // DirectWrite comes up once; failure downgrades the text tool to a
        // status message instead of taking the app down.
        match mn_text::TextEngine::new() {
            Ok(e) => {
                app.text_font = e.default_family();
                println!(
                    "[ui] text engine: {} families, default \"{}\"",
                    e.families().len(),
                    app.text_font
                );
                app.text_engine = Some(e);
            }
            Err(e) => eprintln!("[ui] text engine init failed: {e}"),
        }
        // The startup doc is a plain image — no Pages palette until a manga
        // shows up (dispatch re-syncs on every doc/page command).
        app.sync_pages_palette();
        // Tool Property section visibility from ui.txt.
        let hidden = app.layout.prop_hidden.clone();
        app.prop_hidden = hidden
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_owned())
            .collect();
        // Recorded action sequences (actions.json beside the exe).
        app.actions_load();
        app
    }

    // --- dirty tracking + window title -------------------------------------

    pub fn dirty(&self) -> bool {
        self.doc.revision != self.saved_revision || self.pages_dirty
    }

    /// Call after a successful save/open/new.
    pub fn mark_saved(&mut self) {
        self.saved_revision = self.doc.revision;
        self.pages_dirty = false;
    }

    pub fn mark_pages_dirty(&mut self) {
        self.pages_dirty = true;
    }

    pub fn desired_title(&self) -> String {
        let name = self
            .doc_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_owned());
        format!(
            "{}{name} - MangaNakama",
            if self.dirty() { "*" } else { "" }
        )
    }

    /// Remember a successfully opened/saved file at the top of the MRU list.
    pub fn note_recent(&mut self, p: &Path) {
        self.recent.retain(|q| q != p);
        self.recent.insert(0, p.to_owned());
        self.recent.truncate(self.prefs.recent_depth);
        save_recent(&self.recent);
    }

    /// The engine at the bottom of the stabilizer→taper chain.
    pub fn engine(&self) -> &Engine {
        self.brush.inner().inner()
    }

    pub fn engine_mut(&mut self) -> &mut Engine {
        self.brush.inner_mut().inner_mut()
    }

    /// Engine-effective eraser: the Eraser tool, the transparent slot
    /// (CSP: transparent is a drawing colour, and it erases with the current
    /// brush's own dab shape), or the stylus flipped tail-end down.
    pub fn eraser_active(&self) -> bool {
        self.tool == Tool::Eraser || self.slot == Slot::Transparent || self.pen.inverted
    }

    /// The colour strokes draw with right now (main unless Sub is selected).
    pub fn active_color(&self) -> [f32; 3] {
        if self.slot == Slot::Sub {
            self.sub_color
        } else {
            self.main_color
        }
    }

    /// Record the Recent strip into `ui.txt` as hex, newest first. It lives
    /// there rather than in `swatches.txt` because that file is the user's
    /// PALETTE — chosen, hand-editable, worth backing up — and disposable
    /// churn has no business inside it.
    pub fn note_color_history(&mut self) {
        let hex: Vec<String> = self
            .color_history
            .iter()
            .map(|c| mn_core::palette::hex_string(*c))
            .collect();
        self.layout.note_color_history(&hex);
    }

    /// Push colour + eraser mode into the engine. Cheap — call on any change
    /// to tool, slot, or the slot colours.
    pub fn apply_draw_state(&mut self) {
        let (color, eraser) = (self.active_color(), self.eraser_active());
        let e = self.engine_mut();
        e.set_color(color);
        e.set_eraser(eraser);
    }

    /// Push the current Tool Property values into the engine.
    pub fn apply_props(&mut self) {
        let p = self.props_current;
        self.brush.set_strength(p.stabilizer);
        self.brush.set_correction(p.correct);
        let t = self.brush.inner_mut();
        t.length_px = p.taper_px;
        t.min = p.taper_min;
        // Texture tip first (needs `self` reads; the engine borrow below
        // would conflict): index 0 = none; 1.. names into `texture_names`.
        let texture_mask = if p.texture > 0 {
            self.brushes_root.as_deref().and_then(|root| {
                self.texture_names
                    .get(p.texture as usize - 1)
                    .and_then(|n| mn_brush::load_texture(root, n))
            })
        } else {
            None
        };
        self.engine_mut().apply_props_all(&p, texture_mask.as_ref());
    }

    /// Rebuild the stroke twins from the current preset: one per active axis
    /// combination (X, Y, both — up to three fresh engines). Per axis, wrap
    /// tiling wins if it and mirror are somehow both on (the UI keeps them
    /// exclusive). The twins then replay `apply_props`/`apply_draw_state`
    /// through the per-kind guarded path, so a copy is always exactly the
    /// brush being painted with. Called on every toggle and preset switch.
    ///
    /// Part 3: a live symmetric ruler OWNS the twin set while special
    /// rulers are on — the 2N-1 dihedral images replace the axis twins
    /// (the more specific mechanism wins; the top-bar mirror/wrap
    /// checkboxes come back when the ruler is deleted or special-snap is
    /// toggled off).
    pub fn rebuild_twins(&mut self) {
        let sym = self
            .doc
            .rulers
            .items
            .iter()
            .find(|r| matches!(r, mn_core::Ruler::Symmetric { .. }));
        let twins = if let (Some(r), true) = (sym, self.doc.rulers.special_active()) {
            let affines = symmetric_affines(r);
            let mut kinds: Vec<EngineKind> = Vec::with_capacity(affines.len());
            for _ in 0..affines.len() {
                kinds.push(self.make_twin_kind());
            }
            affines
                .into_iter()
                .zip(kinds)
                .map(|(xf, kind)| StrokeTwin {
                    kind,
                    x: None,
                    y: None,
                    xf: Some(xf),
                })
                .collect()
        } else {
            self.axis_twins()
        };
        self.engine_mut().set_twins(twins);
        // Fresh twins default to record-mode Off; a LIVE GPU dab stroke has
        // the main engine in Bypass — align them (audit H1, third-order:
        // mixed modes meant CPU twin ink that the stroke-end readback then
        // overwrote with GPU content in shared tiles).
        let mode = if self.dab_stroke.is_some() {
            mn_brush::RecordMode::Bypass
        } else {
            mn_brush::RecordMode::Off
        };
        self.engine_mut().set_dab_recording_all(mode);
        self.apply_props();
        self.apply_draw_state();
    }

    /// The checkbox twins (top-bar mirror X/Y, wrap X/Y): one fresh engine
    /// per active axis combination, up to three.
    fn axis_twins(&mut self) -> Vec<StrokeTwin> {
        let x_mode = if self.wrap_x {
            Some(TwinAxis::Wrap)
        } else if self.mirror_x {
            Some(TwinAxis::Mirror)
        } else {
            None
        };
        let y_mode = if self.wrap_y {
            Some(TwinAxis::Wrap)
        } else if self.mirror_y {
            Some(TwinAxis::Mirror)
        } else {
            None
        };
        [(x_mode, None), (None, y_mode), (x_mode, y_mode)]
            .into_iter()
            .filter(|&(x, y)| x.is_some() || y.is_some())
            .map(|(x, y)| StrokeTwin {
                kind: self.make_twin_kind(),
                x,
                y,
                xf: None,
            })
            .collect()
    }

    /// A fresh engine of the current preset/props — the same brush the
    /// user paints with (a twin is a copy, never a fallback... unless the
    /// preset cannot reload, same as the old inline closure).
    fn make_twin_kind(&mut self) -> EngineKind {
        match self.engine().kind() {
            EngineKind::My(_) => {
                let path = self
                    .selected_preset
                    .and_then(|i| self.presets.get(i).map(|(_, p)| p.clone()));
                match path.and_then(|p| MyBrush::load(&p).ok()) {
                    Some(b) => EngineKind::My(Box::new(b)),
                    None => EngineKind::Dab(SimpleDab::new()),
                }
            }
            EngineKind::Dab(d) => EngineKind::Dab(d.clone()),
            EngineKind::Grid(g) => EngineKind::Grid(g.twin()),
            EngineKind::Hairy(h) => EngineKind::Hairy(h.twin()),
            EngineKind::Curve(c) => EngineKind::Curve(c.twin()),
            EngineKind::Dyna(y) => EngineKind::Dyna(y.twin()),
        }
    }

    /// CSP keeps Tool Property per sub tool: remember the outgoing preset's
    /// values before a switch...
    ///
    /// ...unless the sub tool is LOCKED (TL-013), and that exception IS the
    /// feature. A locked tool takes every nudge live — you can still drop
    /// the size for one panel — but the switch away does not write them
    /// down, so `load_props_for` hands back the snapshot on the way in.
    /// The calibrated pen comes home by itself.
    pub fn store_current_props(&mut self) {
        if self.props_current.locked {
            return;
        }
        self.snapshot_current_props();
    }

    /// Write the live values into the per-sub-tool memory unconditionally —
    /// TL-013's snapshot. The only path that overwrites a LOCKED entry, so
    /// that locking (and unlocking) is the one deliberate way to move the
    /// values a locked tool returns to.
    pub fn snapshot_current_props(&mut self) {
        if let Some(i) = self.selected_preset {
            let path = self.presets[i].1.clone();
            self.props.insert(path.clone(), self.props_current);
            self.note_sub_tool_size(&path);
        }
    }

    /// Write this sub tool's SIZE into `ui.txt` (`sub_tool_size_px=`), or drop
    /// the entry when it is back at the preset's own size. Called from the
    /// same place the session memory is written, so the persisted size obeys
    /// TL-013 for free: a LOCKED tool never reaches `snapshot_current_props`
    /// on a switch, so the nudge you took for one panel is not what the next
    /// launch restores — the snapshot is.
    ///
    /// The size only, not the whole `ToolProps`. Everything else there is
    /// either a reading of the preset (which must follow a preset update) or
    /// session state by design (`locked`); the size is the one value the
    /// owner re-dials every launch, which is why it is the one persisted.
    fn note_sub_tool_size(&mut self, path: &Path) {
        // The current engine IS this preset's, so its shipped size is the
        // default to compare against. Callers run before the engine swap.
        let base = self.engine().base_size_px();
        let px = self.props_current.size_px;
        let key = self.preset_key(path);
        let moved = (px - base).abs() > 1e-3;
        self.layout.note_sub_tool_size(&key, moved.then_some(px));
    }

    /// A sub tool's stable identity for `ui.txt`: its preset path RELATIVE to
    /// the brushes root, `/`-separated — so moving the install, or running the
    /// same assets from `cargo run` instead of the shipped exe, keeps the
    /// sizes. A preset from outside the root (none ship that way) falls back
    /// to its file name.
    pub fn preset_key(&self, path: &Path) -> String {
        let rel = self
            .brushes_root
            .as_deref()
            .and_then(|root| path.strip_prefix(root).ok())
            .or_else(|| path.file_name().map(Path::new))
            .unwrap_or(path);
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// The size a sub tool STARTS at: the one the user last set for it
    /// (persisted in `ui.txt`), else the preset's own shipped size. An
    /// untouched sub tool has no stored entry, so a preset whose size changes
    /// moves it — the CODE-MAP rule that the preset size is a DEFAULT.
    fn seed_size_px(&self, path: &Path) -> f32 {
        self.layout
            .sub_tool_size_px
            .get(&self.preset_key(path))
            .copied()
            .unwrap_or_else(|| self.engine().base_size_px())
    }

    /// Drop the stored Tool Property values for the current preset, so the
    /// next `SelectBrush` re-seeds from the preset's own honest readings.
    /// Releases the lock with them: "back to the preset" cannot leave a
    /// padlock hanging over values that are no longer the ones it froze.
    pub fn forget_current_props(&mut self) {
        if let Some(i) = self.selected_preset {
            let path = self.presets[i].1.clone();
            self.props.remove(&path);
            // ...including the persisted size: "back to the preset" that only
            // lasted until the next launch would be the same bug in slow
            // motion.
            let key = self.preset_key(&path);
            self.layout.note_sub_tool_size(&key, None);
        }
        self.props_current.locked = false;
    }

    /// ...and restore the incoming one's after it. First encounter seeds from
    /// the preset's own honest readings, so the panel shows what the brush
    /// actually does instead of imaginary defaults.
    pub fn load_props_for(&mut self, path: &Path) {
        let seed_size = self.seed_size_px(path);
        self.props_current = self.props.get(path).copied().unwrap_or_else(|| {
            let e = self.engine();
            let (taper_px, taper_min) = e.taper_hint().unwrap_or((0.0, 0.18));
            let (random, random_min, random_abs) = e.randomization();
            // Wash presets: Opacity is the stroke-level value the preset
            // carries, Flow its per-dab `opaque`; texture resolves by name.
            let wash = e.wash();
            let texture = e
                .texture_name()
                .and_then(|n| self.texture_names.iter().position(|t| t == n))
                .map_or(0, |i| i as u16 + 1);
            ToolProps {
                // The preset's own size is the DEFAULT, not a ceiling: from
                // here the ladder and the Size control move it anywhere in
                // SIZE_PX_MIN..SIZE_PX_MAX. A size the user set in an earlier
                // SESSION overrides that default (ui.txt `sub_tool_size_px=`);
                // one he never touched still comes from the preset.
                size_px: seed_size,
                opacity: if wash {
                    e.wash_opacity()
                } else {
                    e.base_opacity()
                },
                min_size: e.size_min_pct(),
                stabilizer: 0.0,
                // No preset on disk carries CSP's Correction group (the .myb
                // format has nowhere to put it), so a first encounter always
                // seeds it off — which is also what keeps every existing
                // brush drawing exactly as it did.
                correct: mn_core::stabilize::CorrectCfg::default(),
                random,
                random_min,
                random_abs,
                taper_px,
                taper_min,
                hard_dab: e.hard_dab(),
                scatter: e.scatter(),
                wash,
                flow: e.base_opacity(),
                brush_blend: e.wash_blend(),
                texture,
                texture_scroll: e.texture_scroll(),
                sketch: e.sketch().is_some(),
                sketch_dist: e.sketch().map_or(40.0, |s| s.distance),
                // 0.6, not the pre-M1-fix 0.3: the halved rng doubled the
                // effective link rate, so presets + defaults authored then
                // read at half strength now (audit M1 re-tune).
                sketch_density: e.sketch().map_or(0.6, |s| s.density),
                // TL-013: a sub tool met for the first time is never locked.
                locked: false,
                // The feel rows seed as UNTOUCHED, not as a reading: a
                // preset's own spacing/feather has no CSP mode to report,
                // and inventing one would apply it back on the next
                // `apply_props` and change how the brush draws.
                interval: mn_brush::Interval::AsPreset,
                interval_px: crate::cmd::DEFAULT_INTERVAL_PX,
                density_by_gap: None,
                anti_alias: mn_brush::AntiAlias::AsPreset,
            }
        });
    }

    /// The dpi print units convert against (96 when the document has no page
    /// setup — pixel presets still get sane gutter/border defaults). ONE
    /// fallback, shared by `mm_to_px` and the px/mm readouts, so a border
    /// cannot be measured against one dpi and printed against another.
    pub fn page_dpi(&self) -> u32 {
        self.page
            .as_ref()
            .map(|p| p.dpi)
            .filter(|d| *d > 0)
            .unwrap_or(96)
    }

    /// Millimetres to canvas px at the page's dpi.
    pub fn mm_to_px(&self, mm: f32) -> f32 {
        mm / 25.4 * self.page_dpi() as f32
    }

    /// The DPI tone screens rasterize against: the page's print dpi, or the
    /// manga standard 600 for pixel canvases (at 96 a 60 LPI screen would be
    /// sub-pixel noise).
    pub fn tone_dpi(&self) -> u32 {
        self.page
            .as_ref()
            .map(|p| p.dpi)
            .filter(|d| *d > 0)
            .unwrap_or(600)
    }

    /// The work's OWN print resolution, or `None` for a pixel canvas.
    /// Unlike `tone_dpi` this must not invent 600: a canvas with no dpi
    /// has nothing for an output dpi to be relative to, and guessing here
    /// would silently downscale every export from such a work.
    pub fn work_dpi(&self) -> Option<u32> {
        self.page.as_ref().map(|p| p.dpi).filter(|d| *d > 0)
    }

    /// The export-all window's finishing draft, gathered from the three
    /// fields that hold it. The picker derives its selection from this.
    pub fn export_finish(&self) -> mn_core::export::ExportFinish {
        mn_core::export::ExportFinish {
            dpi: self.export_all_dpi,
            colour: self.export_all_colour,
            split_spreads: self.export_all_split,
        }
    }

    /// Fill the finishing draft from a preset. Every field stays editable
    /// afterwards — an edit simply stops matching and the picker reads
    /// "Custom".
    pub fn set_export_finish(&mut self, f: mn_core::export::ExportFinish) {
        self.export_all_dpi = f.dpi;
        self.export_all_colour = f.colour;
        self.export_all_split = f.split_spreads;
    }

    /// Re-derive every tone layer's halftone raster. The render loop calls
    /// this every frame; sampling/export commands call it before compositing.
    pub fn refresh_tones(&mut self) {
        let dpi = self.tone_dpi();
        self.doc.refresh_derived(dpi);
    }

    /// Remember the document's path (the title bar polls `desired_title`).
    pub fn set_doc_path(&mut self, path: Option<PathBuf>) {
        self.doc_path = path;
    }

    /// Hand the serialized dock tree to the layout for persistence (called
    /// at the end of every UI frame; string compare is the change check).
    pub fn sync_dock_layout(&mut self) {
        let t = crate::ui::dock::to_json_tree(&self.dock);
        self.layout.note_dock_tree(&t);
        let hidden: Vec<&str> = self.prop_hidden.iter().map(|s| s.as_str()).collect();
        self.layout.note_prop_hidden(&hidden.join(","));
    }

    /// One line for the top bar (and the console, which is the log).
    /// True while a selection-paint stroke is live (the overlay's preview).
    pub fn sel_paint_active(&self) -> bool {
        self.sel_paint.is_some()
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        println!("[cmd] {m}");
        self.status = m;
        self.status_warn = false;
        self.needs_redraw = true;
    }

    /// Same, for something that failed. Never a panic: a bad file must not take
    /// the drawing down with it.
    /// A refusal or a failure. Same line as `set_status`, painted in
    /// [`ui::theme::c().warn`] — the whole difference between "nothing happened"
    /// and "nothing happened, and here is why" is whether the eye lands on
    /// it.
    pub fn set_error(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        eprintln!("[cmd] {m}");
        self.status = m;
        self.status_warn = true;
        self.needs_redraw = true;
    }

    pub fn drawing(&self) -> bool {
        self.stroke.is_some()
    }

    pub fn panning(&self) -> bool {
        self.pan_drag.is_some()
    }

    /// Record what one `WM_POINTER` pen batch said about the device, and
    /// disclose it. Pure bookkeeping on the sample path — it never moves a
    /// dab — but it is what turns the corpus's two invisible failures into
    /// reportable ones: an artist can now read off which side of the driver
    /// boundary lost their pressure (§4.1) instead of guessing for nine
    /// years, which is what 124 threads did.
    pub fn note_pen_report(&mut self, b: &crate::input::PenBatch) {
        // A report the OS could no longer describe carries no facts — every
        // `WM_POINTERUP` produces one — so it must not be allowed to CLEAR
        // facts we already hold. (Without this, each pen-up would announce
        // that pressure had stopped being reported.)
        if b.reports == 0 {
            return;
        }
        self.pen.dropped_at_last_report = self.pen.dropped;
        self.pen.dropped += (b.reports - b.samples.len()) as u64;
        self.pen.tilt_reported = b.tilt_reported;
        let was = std::mem::replace(&mut self.pen.pressure_reported, b.pressure_reported);
        if !self.pen.seen {
            self.pen.seen = true;
            // The input receipt: one line, once, the first time a pen is
            // seen. `pen · pressure: NOT REPORTED · 0 of 7 in contact` hands
            // over in three seconds what the corpus never established.
            let msg = format!(
                "pen · pressure: {} · tilt: {} · first batch: {} of {} report(s) in contact",
                Self::pressure_word(b.pressure_reported),
                if b.tilt_reported {
                    "reported"
                } else {
                    "not reported"
                },
                b.samples.len(),
                b.reports,
            );
            crate::testlog::line(&format!("[pen] {msg}"));
            if b.pressure_reported {
                self.set_status(msg);
            } else {
                self.set_error(msg);
            }
        } else if was != b.pressure_reported {
            // The more valuable half of §3.3: pressure that WORKED and then
            // stopped, mid-session. Silent until now, and unfalsifiable from
            // inside the app.
            let msg = format!("pen pressure: {}", Self::pressure_word(b.pressure_reported));
            crate::testlog::line(&format!("[pen] {msg}"));
            if b.pressure_reported {
                self.set_status(msg);
            } else {
                self.set_error(msg);
            }
        }
        self.set_pen_inverted(b.inverted);
    }

    fn pressure_word(reported: bool) -> &'static str {
        if reported {
            "reported"
        } else {
            "NOT REPORTED — 0.5 substituted"
        }
    }

    /// The stylus flipped end-over-end (`PEN_FLAG_INVERTED`/`_ERASER`).
    ///
    /// **Design call, this round.** The tail SELECTS THE ERASER TOOL rather
    /// than applying eraser behaviour to the current brush, and flipping
    /// back restores the tool that was standing before. Two reasons. First,
    /// the four *accepted* requests behind §4.9 all ask for the same thing —
    /// that the eraser end keep its own size and settings — and `SetTool`
    /// already carries a per-tool preset memory, so the tool route gets that
    /// for free where a hidden brush-mode flag would have to reinvent it.
    /// Second, a visible toolbar change is disclosure, and this round exists
    /// because latched state that nothing displays is how a pen path lies.
    ///
    /// The engine's eraser mode is set here **synchronously** as well:
    /// `SetTool` is a queued command, and a tail that touches down without
    /// hovering first would otherwise ink one stroke with the pen before the
    /// queue drains. Never applied mid-stroke — a flip cannot change the
    /// tool under a live line.
    pub fn set_pen_inverted(&mut self, inverted: bool) {
        if inverted == self.pen.inverted || self.drawing() {
            return;
        }
        self.pen.inverted = inverted;
        self.apply_draw_state();
        if inverted {
            if self.tool != Tool::Eraser {
                self.pen.tool_before_tail = Some(self.tool);
                self.push_cmd(AppCmd::SetTool(Tool::Eraser));
            }
            self.set_status("stylus tail — erasing");
        } else if let Some(t) = self.pen.tool_before_tail.take() {
            // Only if the tail's own tool is still standing: switching tools
            // by hand while inverted is the user's decision, not ours to
            // undo behind their back.
            if self.tool == Tool::Eraser {
                self.push_cmd(AppCmd::SetTool(t));
                self.set_status("stylus tip — back to the previous tool");
            }
        }
        self.mark_dirty();
    }

    /// Release every latched input state, and say so if anything was live.
    ///
    /// Focus loss and capture loss mean the same thing — the events that
    /// would have ENDED the gesture in progress are being delivered
    /// somewhere else — so the gesture ends here rather than surviving into
    /// the next one. Two corpus shapes close on this:
    ///
    /// - §4.6: hold space to pan, Alt-Tab, release space in the other
    ///   window, come back. `space_down` stayed true and every pen-down
    ///   panned instead of drawing, permanently. That is the corpus's "the
    ///   pen works everywhere except inside the app" (38 threads) with our
    ///   own name on it.
    /// - §4.4: a focus steal mid-stroke left the undo bracket OPEN until the
    ///   next pen-down closed it, attributing a stroke to the wrong gesture.
    ///   It self-healed, which is exactly why nobody would ever report it.
    pub fn cancel_input_latches(&mut self, why: &str) {
        let live = self.drawing() || self.panning() || self.rotating() || self.space_down;
        if live {
            // Said BEFORE the stroke is ended, so a stroke that has its own
            // verdict to deliver (the zero-sample receipt) still gets the
            // last word on the status line.
            self.set_status(format!("input released — {why}"));
        }
        if self.drawing() {
            self.end_stroke();
        }
        self.end_pan();
        self.end_rotate();
        // Same class, same fix: a live brush-size drag (Ctrl+Alt) left armed
        // resizes the brush on the next pointer move over the canvas.
        self.size_drag = None;
        self.space_down = false;
        self.pen_owner = Owner::None;
        self.mouse_owner = Owner::None;
        // The touch contact map: lose focus mid-pinch and the WM_POINTERUP
        // goes elsewhere — the phantom contact stays, and the next
        // one-finger pan sees touch.len() == 2 and pinch-rotates against a
        // stationary ghost. (main.rs already cancels TAPS here; the map is
        // the other half of the same state.)
        self.touch.clear();
        // And the KB-020 temp-Object grab + the drags it arms: Ctrl+drag a
        // balloon on the Pen tool, Alt-Tab mid-drag, come back — the armed
        // drag would move the balloon on the next pen pass instead of
        // drawing.
        self.temp_object = false;
        self.object_drag = None;
        self.balloon_obj_drag = None;
        self.text_obj_drag = None;
        self.gen_drag = None;
        self.mark_dirty();
    }

    pub fn take_redraw(&mut self) -> bool {
        std::mem::take(&mut self.needs_redraw)
    }

    /// Force a redraw from outside the input handlers (tool switches, layer
    /// toggles, commands).
    pub fn mark_dirty(&mut self) {
        self.needs_redraw = true;
    }

    pub fn push_cmd(&mut self, cmd: AppCmd) {
        self.cmds.push_back(cmd);
    }

    /// Cursor for the canvas area (egui supplies its own over the panels).
    pub fn canvas_cursor(&self) -> egui::CursorIcon {
        if self.space_down || self.tool == Tool::Pan {
            if self.panning() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            }
        } else if let Some(c) = self.transform_cursor() {
            c
        } else if self.tool == Tool::Text {
            egui::CursorIcon::Text
        } else {
            egui::CursorIcon::Crosshair
        }
    }

    /// While a Transform float is up, the cursor says what the handle under
    /// the pointer WILL do — the same answer `transform_down` will give,
    /// through the same `hit_test`, so the two can never disagree.
    fn transform_cursor(&self) -> Option<egui::CursorIcon> {
        let drag = self.transform_drag.as_ref()?;
        let grab = match drag.gesture {
            Some(g) => g.grab,
            None => {
                let (px, py) = self.last_pointer;
                let (cx, cy) = self.viewport.to_canvas(px as f32, py as f32);
                // Alt never changes the ICON (Alt-inside is Pivot, which is
                // Move's cursor; Alt on a handle keeps the handle), so the
                // hover probe does not need the live modifier state.
                drag.hit_test([cx, cy], self.viewport.zoom, false)
            }
        };
        let dragging = drag.gesture.is_some();
        // Screen-space direction of a canvas vector: the box rotates, and so
        // does the VIEW, so the arrow must be picked after both.
        let sector = |a: [f32; 2], b: [f32; 2], n: f32| -> i32 {
            let (ax, ay) = self.viewport.to_screen(a[0], a[1]);
            let (bx, by) = self.viewport.to_screen(b[0], b[1]);
            let step = std::f32::consts::TAU / n;
            (((by - ay).atan2(bx - ax) / step).round() as i32).rem_euclid(n as i32)
        };
        Some(match grab {
            TransformGrab::Corner(i) => {
                // Along the diagonal out of the opposite corner.
                if matches!(
                    sector(drag.bbox[(i + 2) % 4], drag.bbox[i], 8.0),
                    0 | 1 | 4 | 5
                ) {
                    egui::CursorIcon::ResizeNwSe
                } else {
                    egui::CursorIcon::ResizeNeSw
                }
            }
            TransformGrab::Edge(i) => {
                let mid = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                let from = mid(drag.bbox[(i + 2) % 4], drag.bbox[(i + 3) % 4]);
                let to = mid(drag.bbox[i], drag.bbox[(i + 1) % 4]);
                if sector(from, to, 4.0) % 2 == 0 {
                    egui::CursorIcon::ResizeHorizontal
                } else {
                    egui::CursorIcon::ResizeVertical
                }
            }
            TransformGrab::Rotate if dragging => egui::CursorIcon::Grabbing,
            TransformGrab::Rotate => egui::CursorIcon::Grab,
            TransformGrab::Move | TransformGrab::Pivot => egui::CursorIcon::Move,
        })
    }

    /// Build the egui frame, then draw canvas + UI into one swapchain frame.
    pub fn render(&mut self) -> FrameOutput {
        self.needs_redraw = false;
        let t0 = Instant::now();
        let size = self.renderer.surface_size();

        // Startup fit (owner report 2026-08-20): App::new fits the canvas
        // against the CREATION-time client size, but the window is
        // restored/maximized AFTER that — so a full-screen window opened on
        // a page fitted for 1280×860 and showed a small square at ~28%.
        // While the flag is up, refit whenever the canvas rect moved since
        // the last fit. The flag stays up until the FIRST canvas
        // interaction (pen/mouse down, wheel, touch — canvas_input clears
        // it), NOT until the size "settles": the maximize restore can land
        // several frames after two identical small frames, and a
        // settle-latch stood down right before it (the owner reproduced
        // exactly that). Once the user touches the view, the fit never
        // fights them again — a later manual resize keeps their view.
        if self.startup_fit_pending {
            let r = self.shell.canvas_rect_px();
            let sz = (r.width(), r.height());
            if sz.0.is_finite()
                && sz.1.is_finite()
                && sz.0 >= 1.0
                && sz.1 >= 1.0
                && sz != self.startup_fit_last
            {
                self.startup_fit_last = sz;
                self.fit_to_view();
            }
        }

        // The undo-depth preference, applied at the frame head rather than
        // at the nine places a Document is swapped into `self.doc` (open,
        // page turn, tab switch, session restore, …). A missed site there
        // would be a silent failure — the setting quietly not applying to
        // the document you are actually drawing on — and this compare is
        // one integer. A parked document picks the depth up the moment it
        // becomes the active one, which is the only moment it matters.
        if self.doc.undo_limit() != self.prefs.undo_depth {
            self.doc.set_undo_limit(self.prefs.undo_depth);
        }

        // Panel reading order, same frame-head reasoning as the line above:
        // the cache holds raw layer indices, and a dozen commands that are
        // not "frame commands" shift them (see `ensure_frame_order`).
        self.ensure_frame_order();

        // Tone layers first: their derived halftone rasters are what every
        // composite below displays (cheap — per-tile revision compare).
        self.refresh_tones();

        // GPU dab flush before anything composites this frame.
        self.flush_gpu_dabs();

        // Keep the ACTIVE page's Pages-panel thumbnail live (CSP): one
        // small offscreen composite — panels, balloons, text, everything —
        // per content revision, before the UI builds so this frame already
        // shows it. Other pages keep their stashed thumbs. Single-page
        // plain images skip it when the palette is closed (their auto-hide
        // is `sync_pages_palette`); a visible Pages palette always thumbs.
        // Owner preview tier: also re-mint when the pane's cell size drifted
        // >25% from what the texture was built at (Fit-to-pane / slider).
        let aspect = self.doc.size.1.max(1) as f32 / self.doc.size.0.max(1) as f32;
        let want_h = (self.pages_cell_px.clamp(112.0, 1200.0) * aspect).clamp(112.0, 1600.0);
        let size_drift = self.pages_thumb_px <= 0.0
            || (want_h - self.pages_thumb_px).abs() > self.pages_thumb_px * 0.25;
        if (self.doc.revision != self.pages_thumb_rev || size_drift)
            && (self.pages.len() > 1
                || crate::ui::dock::is_open(self, crate::ui::dock::Palette::Pages))
        {
            let t = self.thumb_of_current();
            self.pages[self.page_index].thumb = Some(t);
            self.pages_thumb_rev = self.doc.revision;
        }

        // Reader one-per-frame work: preview placeholders for the current
        // screen + one sharp render (current screen first, then the
        // neighbours as prefetch). Owner top item 2026-08-18.
        self.reader_frame();

        // The context is an Arc handle, so cloning it frees `self` to be
        // borrowed mutably by the UI closure.
        let ctx = self.shell.ctx.clone();
        let raw = self.shell.begin(size);
        let mut out = ctx.run_ui(raw, |ui| crate::ui::build(ui, self));
        let repaint_after = self.shell.end(&out);
        let jobs = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);

        {
            let Self {
                renderer,
                shell,
                doc,
                viewport,
                ..
            } = self;
            let vp = *viewport;
            let deltas = &mut out.textures_delta;
            renderer.render_with_overlay(doc, &vp, move |device, queue, enc, view, size| {
                shell.paint(device, queue, enc, view, size, &jobs, deltas);
            });
        }
        // Only legal after the frame has been submitted.
        self.shell.free(&mut out.textures_delta);

        let fs = self.renderer.frame_stats();
        self.diag.note_composite(&fs);
        self.diag.note_frame(t0.elapsed());
        FrameOutput { repaint_after }
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        let (ow, oh) = self.renderer.surface_size();
        self.renderer.resize(w, h);
        // Keep whatever was at the client centre at the centre — maximizing
        // used to leave the page stranded at the old window's fit position.
        self.viewport.pan[0] += (w as f32 - ow as f32) * 0.5;
        self.viewport.pan[1] += (h as f32 - oh as f32) * 0.5;
        self.needs_redraw = true;
    }

    /// Window DPI changed: rescale the UI and keep the canvas the same
    /// apparent size (the client rect grew/shrank by the same ratio).
    pub fn dpi_changed(&mut self, ppp: f32) {
        let old = self.shell.ppp;
        if !(ppp.is_finite() && ppp > 0.0) || (ppp - old).abs() < 1e-4 {
            return;
        }
        let k = ppp / old;
        self.shell.set_ppp(ppp);
        self.viewport.zoom *= k;
        self.viewport.pan = [self.viewport.pan[0] * k, self.viewport.pan[1] * k];
        self.needs_redraw = true;
    }

    // --- strokes ---------------------------------------------------------

    /// Arm/disarm mask editing (LM-004). Single choke point: the flag rides
    /// EVERY MyPaint engine (main + symmetry twins) — never set `mask_edit`
    /// directly.
    pub fn set_mask_edit(&mut self, on: bool) {
        self.mask_edit = on;
        self.brush.inner_mut().inner_mut().set_mask_mode_all(on);
    }

    /// True while the active layer is a LIVE fill layer (TRIAGE 137):
    /// every brush stroke edits its WINDOW mask — the same path and the
    /// same alpha-scale semantics as mask-edit (LM-005), armed implicitly.
    fn live_fill_active(&self) -> bool {
        matches!(self.doc.active_layer().kind, mn_core::LayerKind::Fill(_))
    }

    /// Audit H1 (rounds 50-68): leave mask-edit mode when the active layer
    /// no longer carries a mask — selection change, delete, bake, undo/redo
    /// and file open can all move the ground under the armed flag, and an
    /// armed flag over a maskless layer aborted the process inside the C
    /// tile callback. The surface degrades to dropped dabs as the backstop;
    /// this keeps the flag (and the status line) honest on known paths.
    pub fn disarm_mask_edit_if_unmasked(&mut self) {
        if self.mask_edit && self.doc.active_layer().mask.is_none() {
            self.set_mask_edit(false);
            self.set_status("mask gone — editing the layer again");
        }
    }

    /// The cached anti-overflow barrier (audit small, 2026-08-25): the
    /// mask is a full reference-set composite, so it is rebuilt ONLY
    /// when the reference set's own key moved — `(canvas size,
    /// reference layer indices, newest tile revision among them)`. Tile
    /// revisions are globally monotonic, so any edit, paste or undo
    /// inside a reference layer changes the key and a paint stroke
    /// anywhere else does not. A cache hit hands back the SAME Arc.
    pub(crate) fn cached_anti_overflow_mask(
        &mut self,
    ) -> Option<std::sync::Arc<mn_brush::AntiOverflowMask>> {
        let refs = self.doc.reference_layers();
        let rev = refs
            .iter()
            .filter_map(|&li| self.doc.layers.get(li))
            .flat_map(|l| l.tiles())
            .map(|(_, t)| t.revision())
            .max()
            .unwrap_or(0);
        let key = (self.doc.size, refs, rev);
        match &self.anti_overflow_cache {
            Some((k, m)) if *k == key => m.clone(),
            _ => {
                let m = mn_core::fill::anti_overflow_barrier(&self.doc)
                    .map(|(w, allow)| std::sync::Arc::new(mn_brush::AntiOverflowMask { w, allow }));
                self.anti_overflow_cache = Some((key, m.clone()));
                m
            }
        }
    }

    /// Open the undo op *and* the brush stroke. Both halves are mandatory:
    /// `begin_op` is what makes the stroke undoable (every `tile_mut` in between
    /// snapshots itself), and `StrokeSink::begin` is what snaps libmypaint's
    /// position smoothing to the pen — feeding samples without it smears a line
    /// in from wherever the previous stroke ended (see `MyBrush`'s
    /// `FIRST_SAMPLE_DTIME`).
    pub fn begin_stroke(&mut self, kind: PointerKind) {
        if self.stroke.is_some() {
            self.end_stroke();
        }
        // A live tonal-correction preview owns the layer's pixels until it
        // is applied or cancelled (see app/adjust.rs). Painting into them
        // would be reverted by the very next Apply, so the stroke is
        // refused — the same chokepoint the mask-edit disarm uses, for the
        // same reason: every stroke in the app enters here.
        if self.adjust_preview.is_some() {
            self.set_status("finish the correction first — Apply or Cancel the dialog");
            return;
        }
        // Mouse strokes get a FLOOR of pull-string smoothing. The mouse
        // delivers one sample per WM_MOUSEMOVE (60–125 Hz) while pen history
        // batches arrive dense; with the owner's smoothing-off presets every
        // dab lands exactly on the raw input points, so mouse ink reads as
        // short straight segments between sparse reports instead of a curve
        // (owner report 2026-08-16). The floor is in SCREEN px, converted to
        // canvas px at stroke start so the smoothing feels the same at any
        // zoom; pen keeps the sub tool's own setting (the owner inks with 0).
        // The floor itself is the `mouse_smooth_px` preference — 12 px
        // shipped, and 0 turns it off (the mouse then takes the sub tool's
        // stabilizer verbatim, exactly like the pen).
        let floor_px = self.prefs.mouse_smooth_px;
        let radius_px = if kind == PointerKind::Mouse {
            (self.props_current.stabilizer * mn_core::stabilize::MAX_STRING_PX)
                .max(floor_px / self.viewport.zoom.max(0.01))
        } else {
            self.props_current.stabilizer * mn_core::stabilize::MAX_STRING_PX
        };
        self.brush
            .set_strength(radius_px / mn_core::stabilize::MAX_STRING_PX);
        // The correction stage reads zoom for `C-033` (its window is held in
        // SCREEN px) and to judge pen speed in screen px. Sampled ONCE per
        // stroke, like the mouse floor above: a wheel-zoom mid-stroke must not
        // resize the smoothing window under a half-emitted sample.
        self.brush.set_zoom(self.viewport.zoom);
        // F5 (audit r69-78): disarm mask-edit at the ONE place every
        // stroke enters — the nine command-site calls stay as harmless
        // belt-and-braces, but no future layer-switch path can route
        // around this one.
        self.disarm_mask_edit_if_unmasked();
        let live = self.live_fill_active();
        if live && self.doc.active_layer().mask.is_none() {
            self.set_status("live layer has no window mask — make one from a selection first");
            return;
        }
        // Selection paint (SE round 2026-08-19): the selection pen/eraser
        // and Quick Mask route the stroke into the DOCUMENT's selection
        // scratch — the engine paints coverage with full brush fidelity,
        // the overlay previews the ants per frame, release commits through
        // SE-022's combine. Exclusive with mask strokes by construction.
        self.sel_paint = match self.tool {
            crate::cmd::Tool::SelPen => Some(SelPaintOp::Add),
            crate::cmd::Tool::SelEraser => Some(SelPaintOp::Subtract),
            crate::cmd::Tool::Pen if self.quick_mask => Some(SelPaintOp::Add),
            crate::cmd::Tool::Eraser if self.quick_mask => Some(SelPaintOp::Subtract),
            _ => None,
        };
        let sel_paint = self.sel_paint.is_some();
        if sel_paint {
            self.doc.sel_scratch = mn_core::doc::LayerMask {
                tiles: std::collections::HashMap::new(),
                enabled: true,
                revision: 0,
            };
        }
        self.brush
            .inner_mut()
            .inner_mut()
            .set_sel_mode_all(sel_paint);
        self.brush
            .inner_mut()
            .inner_mut()
            .set_mask_mode_all((self.mask_edit || live) && !sel_paint);
        // LM-004: the mask stroke's undo bracket opens HERE, next to
        // `begin_op` and for the same reason. The engine writes the mask's
        // coverage tiles LIVE, per dab (see `mn-brush`'s surface callback) —
        // a snapshot taken at `end_stroke` would already contain the stroke,
        // so undo restored the stroke it was meant to remove. `mask_op_end`
        // pushes the group only if the coverage revision actually moved, so
        // an aborted or empty stroke still spends no undo step.
        if (self.mask_edit || live) && !sel_paint {
            self.doc.mask_op_begin();
        }
        // Vector inking phase 1 (docs/VECTOR-INKING.md): a plain ink stroke
        // on a recording layer captures its post-snap samples beside the
        // pixels — `end_stroke` closes both into ONE undo group. Mirror and
        // wrap strokes deliberately do NOT record: their twin-engine halves
        // are not in the captured samples, so an edit re-render would drop
        // the mirrored ink — leaving them out of the record is the honest
        // half (the manual says so).
        self.vector_capture = (!(self.mask_edit || live)
            && !sel_paint
            && !(self.mirror_x || self.mirror_y || self.wrap_x || self.wrap_y)
            && self.doc.active_layer().strokes.is_some())
        .then(Vec::new);
        self.input_resampler.reset();
        self.doc.begin_op();
        self.brush.begin(&mut self.doc);
        // GPU dab stroke (P1): route the engine's rasterization to the
        // compute path. Wash/texture/exotic brushes and unsupported adapters
        // keep the CPU path (the reference, per the GPU-everything directive).
        // The record mode is a FUNCTION OF THE BRANCH, never a latch: the
        // engine object survives Tool Property edits (wash/texture toggles
        // mutate it in place), so a stroke that armed Bypass and a later
        // brush change that disqualifies the GPU path must disarm it here —
        // audit H1: with the latch, the engine kept recording without ever
        // rasterizing, and the pen went silent until a sub-tool switch.
        self.dab_stroke = None;
        // Rulers part 2: the sticky lock is stroke-scoped.
        self.ruler_lock = Default::default();
        // Row 42 (A-014, はみ出さない): build the stroke's anti-overflow
        // barrier — the REFERENCE SET composite only (owner ruling
        // 2026-08-25: frame folders clip their own children themselves
        // and never wall the page) — and hand it to every engine. None
        // paints freely, exactly as before. The barrier is cached
        // against the reference set's own tile revisions: a stroke on a
        // paint layer reuses it, the first stroke after editing the
        // reference rebuilds it.
        let anti_mask = if self.anti_overflow && !self.mask_edit && !live && !sel_paint {
            self.cached_anti_overflow_mask()
        } else {
            None
        };
        self.brush
            .inner_mut()
            .inner_mut()
            .set_anti_overflow_all(anti_mask.clone());
        let gpu_path = self.gpu_dabs
            && !self.mask_edit // LM-004: mask strokes are CPU (the GPU path writes layer tiles)
            && !live // a live layer's strokes ARE mask strokes (TRIAGE 137)
            && !sel_paint // selection strokes are CPU too (same reason — they write the scratch)
            // Row 42: the GPU dab path writes layer tiles with no barrier
            // — anti-overflow strokes run CPU.
            && anti_mask.is_none()
            && self.renderer.gpu_dabs_supported()
            && self.brush.inner().inner().gpu_dab_ready();
        self.brush
            .inner_mut()
            .inner_mut()
            .set_dab_recording_all(if gpu_path {
                mn_brush::RecordMode::Bypass
            } else {
                mn_brush::RecordMode::Off
            });
        if gpu_path {
            let wash = self.brush.inner().inner().wash();
            // Smudge needs the sampler fed from the GPU tile cache: the
            // stroke's layer for the oracle, and per-sample dispatch. A
            // WASH stroke's sampler would read the in-flight wash buffer
            // (WASH_LAYER_KEY) — the wiring below is ready, but
            // `gpu_ready` still routes wash+smudge CPU: the C sampler has
            // PER-DAB visibility of the stroke's own paint and a batched
            // GPU path measurably drifts (see MyBrush::gpu_ready).
            let smudge = self.brush.inner().inner().smudge();
            if wash {
                self.renderer.begin_wash_dab_stroke();
            } else {
                self.renderer.begin_dab_stroke(self.doc.active);
            }
            if smudge {
                let key = if wash {
                    mn_gpu::WASH_LAYER_KEY
                } else {
                    self.doc.active
                };
                let rptr: *mut mn_gpu::Renderer = &mut self.renderer;
                let ctx = Box::into_raw(Box::new((rptr, key))) as *mut core::ffi::c_void;
                self.dab_smudge_ctx = Some(ctx);
                mn_brush::set_tile_oracle(Some((smudge_tile_oracle, ctx)));
            }
            self.dab_stroke = Some(DabStrokeApp {
                all_dabs: Vec::new(),
                hard: self.brush.inner().inner().hard_dab_main(),
                flushes: 0,
                wash,
                smudge,
            });
            self.dab_path_last = "gpu".into();
            self.diag.dab_gpu_strokes += 1;
        } else {
            self.dab_path_last = if self.gpu_dabs { "cpu (routed)" } else { "cpu" }.into();
            if self.gpu_dabs {
                self.diag.dab_cpu_routed += 1;
                // Why the CPU path took over (wash/texture/exotic brush) —
                // the tester log's routing line.
                crate::testlog::line(&format!(
                    "[dab] cpu (routed; brush={})",
                    self.brush.inner().inner().name(),
                ));
            }
        }
        // The baseline is the count from BEFORE the report being handled: a
        // pen-down whose whole batch was filtered out must own those drops,
        // and it is decoded before it is dispatched here.
        // PATCHES.md #19: drain any stale clamp count here (a path that
        // abandoned a stroke without `end_stroke`) so the end-of-stroke
        // warning fires only on this stroke's own clamps.
        let _ = MyBrush::take_dab_clamp_count();
        self.stroke = Some(StrokeStats::new(
            kind,
            self.last_pointer,
            self.pen.dropped_at_last_report,
        ));
    }

    /// Feed one batch of client-space samples (a `GetPointerPenInfoHistory`
    /// batch, or a single mouse move). Coordinates are converted to canvas
    /// space here, so the brush never sees the viewport.
    pub fn push_batch(&mut self, batch: &[PenSample]) {
        if self.stroke.is_none() || batch.is_empty() {
            return;
        }
        // Row 89 (BR-014–016): the wizard listens to the RAW tablet
        // pressures; the global correction curve then bends every sample
        // BEFORE any per-tool curve sees it — global first, for every
        // tool at once. Identity (empty curve) costs one length check.
        if self.pen_wizard_open {
            self.pen_wizard_samples
                .extend(batch.iter().map(|s| s.pressure));
            let keep = self.pen_wizard_samples.len().saturating_sub(4096);
            self.pen_wizard_samples.drain(0..keep);
        }
        let corrected: Vec<PenSample>;
        let batch: &[PenSample] = if self.global_pressure.is_empty() {
            batch
        } else {
            corrected = batch
                .iter()
                .map(|s| {
                    let mut s = *s;
                    s.pressure =
                        mn_core::stroke::eval_pressure_curve(&self.global_pressure, s.pressure);
                    s
                })
                .collect();
            &corrected
        };
        // View-compensated speed/direction inputs (vendor patch #12) — per
        // batch, so wheel-zoom/rotation mid-stroke stays correct. Rotation
        // goes in RAW RADIANS: the C applies DEGREES() itself, and our
        // viewport's screen = R(rotate_rad)·canvas means the C's
        // dir_angle + viewrotation arithmetic lands in true screen space.
        // The mirror rides along for patch #12's flip extension (the flip
        // has already negated rotate_rad). It comes from `brush_view()`,
        // NOT from `flip_h`: the C knows only a horizontal mirror, so a
        // vertical flip is handed to it as the equivalent mirror-plus-half-
        // turn, and H+V as a plain half turn with no mirror at all. At
        // 100%/0°/unflipped this is bit-identical to the stock entry point.
        let (view_rot, view_mirrored) = self.viewport.brush_view();
        self.brush
            .inner_mut()
            .inner_mut()
            .set_view(self.viewport.zoom, view_rot, view_mirrored);
        let kind = self
            .stroke
            .as_ref()
            .map(|s| s.kind)
            .unwrap_or(PointerKind::Mouse);
        self.diag.note_batch(kind, batch);
        if let Some(s) = &mut self.stroke {
            s.note_batch(batch.len());
        }
        for s in batch {
            let (cx, cy) = self.viewport.to_canvas(s.x, s.y);
            if let Some(st) = &mut self.stroke {
                st.note_sample(s.pressure);
            }
            // Doc-space resampling BEFORE the engine: the polygonal-at-low-
            // zoom fix (input_path.rs). Shape-preserving; dense input (the
            // 100%-zoom case) passes through untouched.
            for r in self.input_resampler.push(PenSample { x: cx, y: cy, ..*s }) {
                // TODO #3: ruler snapping — post-resampler, pre-stabilizer,
                // so the pen slides along the ruler like Krita/CSP. Sticky
                // (part 2): the first snapped sample locks the ruler for the
                // whole stroke; crossing rulers cannot flicker mid-stroke.
                let snapped = self
                    .doc
                    .rulers
                    .snap_sticky([r.x, r.y], &mut self.ruler_lock);
                let r = PenSample {
                    x: snapped[0],
                    y: snapped[1],
                    ..r
                };
                if let Some(cap) = &mut self.vector_capture {
                    cap.push(r);
                }
                self.brush.sample(&mut self.doc, r);
                // Smudge strokes dispatch per sample (NOT per frame): the
                // C's get_color must see sample N's dabs when sample N+1
                // fires it — on CPU that visibility comes from end_atomic's
                // tile processing, and per-frame flushes would leave whole
                // frames of ink invisible to the sampler. Inlined (not
                // `flush_gpu_dabs`, whose `&mut self` cannot rebase the
                // resampler's live field borrow) — field-disjoint places
                // only, same discipline as the `sample` call above.
                if self.dab_stroke.as_ref().is_some_and(|st| st.smudge) {
                    let dabs = self.brush.inner_mut().inner_mut().drain_dab_records();
                    if !dabs.is_empty() {
                        let hard = self.dab_stroke.as_ref().map(|s| s.hard).unwrap_or(false);
                        let wash = self.dab_stroke.as_ref().map(|s| s.wash).unwrap_or(false);
                        let tex = self.brush.inner().inner().texture_flush();
                        if wash {
                            // Wash+smudge (P4): per-sample dispatch into the
                            // wash sentinel, so the oracle's readback shows
                            // the sampler every prior dab of this stroke.
                            if let Some(buf) = self.brush.inner().inner().wash_buffer() {
                                self.renderer.flush_wash_dabs(buf, &dabs, hard, tex);
                            }
                        } else {
                            self.renderer.flush_dabs(&self.doc, &dabs, hard, tex);
                        }
                        if let Some(st) = &mut self.dab_stroke {
                            st.all_dabs.extend(dabs);
                            st.flushes += 1;
                        }
                    }
                }
            }
        }
        self.needs_redraw = true;
    }

    /// The direct-feel rule (owner, 2026-08-26, applied at scale): a
    /// transform float is MODAL state — it must never outlive the context
    /// it was opened in. Any layer/page/document switch calls this FIRST,
    /// so the float bakes where it was lifted (an identity float cancels
    /// itself inside the arm) instead of stamping into whatever document
    /// or layer happens to be active at commit time.
    pub fn commit_open_float(&mut self) {
        if self.transform_drag.is_some() {
            crate::cmd::dispatch(self, AppCmd::TransformCommit);
        }
    }

    pub fn end_stroke(&mut self) {
        if self.stroke.is_none() {
            return;
        }
        self.doc.set_op_label("Stroke");
        // LM-004: the bracket opened in `begin_stroke` (the snapshot has to
        // predate the first dab); it closes below, after the tail dabs.
        let mask_stroke = self.mask_edit || self.live_fill_active();
        // The resampler's tail first: its last points must flow through the
        // stabilizer + engine before `end` drains the pull-string, landing
        // inside the still-open undo op like every other dab. Ruler snapping
        // applies here too — the tail is ordinary input (found by draining
        // the dab record AFTER stroke end instead of working around the old
        // finish-order bug: the tail dabs were visibly off the ruler).
        for r in self.input_resampler.flush() {
            let snapped = self
                .doc
                .rulers
                .snap_sticky([r.x, r.y], &mut self.ruler_lock);
            let r = PenSample {
                x: snapped[0],
                y: snapped[1],
                ..r
            };
            if let Some(cap) = &mut self.vector_capture {
                cap.push(r);
            }
            self.brush.sample(&mut self.doc, r);
        }
        // Order matters: the stabilizer drains its remaining string inside
        // `end`, so those last dabs must still land inside the open undo op —
        // and the selection mask must clamp them before the op closes.
        self.brush.end(&mut self.doc);
        // PATCHES.md #19: some dab(s) exceeded the per-dab tile budget and
        // were clamped — an imported tip whose stored "size" is not pixels.
        // Warn once per brush+size (re-picking the brush or moving the size
        // slider re-arms it); the stroke itself is already safe.
        if MyBrush::take_dab_clamp_count() > 0 {
            let key = format!(
                "{}@{:.0}",
                self.selected_preset
                    .and_then(|i| self.presets.get(i).map(|(_, p)| self.preset_key(p)))
                    .unwrap_or_default(),
                self.props_current.size_px
            );
            if self.dab_clamp_warned.as_deref() != Some(key.as_str()) {
                self.set_status("brush size clamped — the imported size looks wrong");
                self.dab_clamp_warned = Some(key);
            }
        }
        self.finish_gpu_dab_stroke();
        self.doc.mask_stroke_to_selection();
        // Transparent-pixel lock clamps ONCE at stroke end (not per batch —
        // the attenuation would compound; see Document::mask_op_to_alpha).
        if self
            .doc
            .op_layer_index()
            .is_some_and(|li| self.doc.layers[li].lock_alpha)
        {
            self.doc.mask_op_to_alpha();
        }
        // Vector inking: close the op as ONE pixels-plus-record group when
        // this stroke captured (docs/VECTOR-INKING.md); otherwise stock.
        match self.vector_capture.take() {
            // Phase 3: the ERASER on a vector layer TRIMS geometry (up to
            // the neighbouring intersections) instead of recording an
            // eraser stroke. The live raster erase already happened inside
            // the op; the re-derive replaces it with the trimmed truth —
            // and an eraser that touched no stroke reverts to the op's own
            // pre-images and spends nothing.
            Some(samples) if !samples.is_empty() && self.eraser_active() => {
                let li = self.doc.active;
                let before = self.doc.layers[li].strokes.clone().unwrap_or_default();
                let path: Vec<(f32, f32)> = samples.iter().map(|s| (s.x, s.y)).collect();
                let radius = (self.props_current.size_px / 2.0).max(1.0);
                let changed = self.doc.layers[li]
                    .strokes
                    .as_mut()
                    .is_some_and(|set| set.trim(&path, radius));
                if changed {
                    self.vector_sel = None; // indices just restructured
                    self.rederive_vector_layer(li);
                    self.doc.end_op_vector_set(before, "Trim strokes");
                    self.set_status("strokes trimmed to their crossings");
                } else {
                    self.doc.abort_op_restore();
                }
                self.renderer.invalidate();
            }
            Some(samples) if !samples.is_empty() => {
                let preset = self
                    .selected_preset
                    .map(|i| self.preset_key(&self.presets[i].1.clone()))
                    .unwrap_or_default();
                let c = self.active_color();
                let mut stroke = mn_core::VectorStroke::from_samples(
                    &samples,
                    &preset,
                    self.props_current.size_px,
                    [
                        (c[0] * 255.0).round() as u8,
                        (c[1] * 255.0).round() as u8,
                        (c[2] * 255.0).round() as u8,
                    ],
                    self.eraser_active(),
                );
                // The samples are captured BEFORE the pull-string — the
                // replay must re-run it at the same strength.
                stroke.stabilizer = self.props_current.stabilizer;
                self.doc.end_op_vector_stroke(stroke);
                // Reaching for the Object tool right after inking means "edit
                // THAT stroke": select the record just pushed (the newest on
                // the layer) so a selection left over from an earlier click
                // cannot light up the wrong stroke. A layer that turned out
                // not to record selects nothing. Undo/redo/trim/delete keep
                // clearing the selection themselves, so the index can never
                // outlive its stroke.
                self.vector_sel = self
                    .doc
                    .layers
                    .get(self.doc.active)
                    .and_then(|l| l.strokes.as_ref())
                    .and_then(|set| set.strokes.len().checked_sub(1));
            }
            _ => {
                self.doc.end_op();
            }
        }
        if mask_stroke {
            self.doc.mask_op_end();
            // The mask's revision changed per tile write — the upload fold
            // needs a rebuild (mask edits are command-frequency anyway).
            self.renderer.invalidate();
        }
        // Selection paint (SE round 2026-08-19): commit the scratch into
        // the selection through SE-022's combine. NOT an undo step — CSP
        // parity: selections are not in the undo history.
        if let Some(op) = self.sel_paint.take() {
            self.brush.inner_mut().inner_mut().set_sel_mode_all(false);
            let scratch = std::mem::replace(
                &mut self.doc.sel_scratch,
                mn_core::doc::LayerMask {
                    tiles: std::collections::HashMap::new(),
                    enabled: true,
                    revision: 0,
                },
            );
            if !scratch.tiles.is_empty() {
                let painted = mn_core::selection::Selection::from_mask_field(&self.doc, &scratch);
                let next = match (self.doc.selection.take(), op) {
                    (Some(cur), SelPaintOp::Add) => {
                        cur.combine(&painted, &self.doc, mn_core::SelectionOp::Add)
                    }
                    (Some(cur), SelPaintOp::Subtract) => {
                        cur.combine(&painted, &self.doc, mn_core::SelectionOp::Subtract)
                    }
                    (None, SelPaintOp::Add) => painted,
                    (None, SelPaintOp::Subtract) => mn_core::selection::Selection::default(),
                };
                let n = next.is_empty();
                self.doc.selection = if n { None } else { Some(next) };
                self.doc.touch();
                self.set_status(match op {
                    SelPaintOp::Add => "painted selection — added",
                    SelPaintOp::Subtract => "painted selection — subtracted",
                });
            }
        }
        // Row 42: the barrier is stroke-scoped — disarm for whatever the
        // next stroke is (begin_stroke re-arms from the toggle).
        self.brush
            .inner_mut()
            .inner_mut()
            .set_anti_overflow_all(None);
        if let Some(s) = self.stroke.take() {
            s.report();
            if s.samples == 0 {
                // §4.2/§5.4 — the corpus's signature failure: the app looks
                // alive and produces nothing. `read_pen_batch` drops every
                // sample that is not `POINTER_FLAG_INCONTACT`, which is
                // correct (it removed a blob at every stroke start) and is
                // also exactly how a driver that signals contact through
                // pressure alone gets silenced by us. The counters for
                // saying so already existed; only the sentence was missing.
                let dropped = self.pen.dropped.saturating_sub(s.dropped_at_start);
                let msg = if dropped > 0 {
                    format!(
                        "stroke drew nothing — 0 in-contact samples at ({}, {}); \
                         {dropped} pen report(s) dropped as not-in-contact",
                        s.at.0, s.at.1
                    )
                } else {
                    format!(
                        "stroke drew nothing — no {} samples arrived at ({}, {})",
                        s.kind.label(),
                        s.at.0,
                        s.at.1
                    )
                };
                crate::testlog::line(&format!("[stroke] {msg}"));
                self.set_error(msg);
            }
        }
        self.needs_redraw = true;
    }

    /// Per-frame GPU dab flush: rasterize everything the engines recorded
    /// since the last frame into the tile textures (render calls this before
    /// the composite so the same frame shows the dabs).
    pub fn flush_gpu_dabs(&mut self) {
        if self.dab_stroke.is_none() {
            return;
        }
        let dabs = self.brush.inner_mut().inner_mut().drain_dab_records();
        if dabs.is_empty() {
            return;
        }
        let hard = self.dab_stroke.as_ref().map(|s| s.hard).unwrap_or(false);
        let wash = self.dab_stroke.as_ref().map(|s| s.wash).unwrap_or(false);
        let tex = self.brush.inner().inner().texture_flush();
        if wash {
            // #0.1: seed/read from the CPU wash buffer (blank under BYPASS —
            // zero-seed, wet semantics identical to the CPU path). A live
            // wash stroke always has one (created at `begin`); the
            // `unwrap_or` is only for the impossible window.
            let Some(buf) = self.brush.inner().inner().wash_buffer() else {
                return;
            };
            self.renderer.flush_wash_dabs(buf, &dabs, hard, tex);
        } else {
            self.renderer.flush_dabs(&self.doc, &dabs, hard, tex);
        }
        if let Some(st) = &mut self.dab_stroke {
            st.all_dabs.extend(dabs);
            st.flushes += 1;
        }
    }

    /// Stroke end for the GPU path: final flush, the single readback, CPU
    /// tiles authoritative again, cache marked clean — or, when the canary
    /// disagrees with the dispatched workgroup count (the cursed-driver
    /// defense), a CPU repair from the full dab list. Runs INSIDE the still-
    /// open undo op: BYPASS never touched the CPU tiles, so the pre-image
    /// captured at this write IS the pre-stroke state.
    fn finish_gpu_dab_stroke(&mut self) {
        // The None-guard comes FIRST: a CPU stroke with recording armed (the
        // ruler-snap tests, future oracle consumers) owns its records — a
        // drain here would silently discard them. Hit twice (rounds 42 and
        // 46, both worked around); audit 36–48 §2 ordered the fix. Leaving
        // the mode armed is safe: `begin_stroke` resets it and clears the
        // record, so nothing leaks into the next stroke.
        let Some(mut st) = self.dab_stroke.take() else {
            return;
        };
        // Drain the last dabs FIRST, then disarm — `set_dab_recording`
        // clears the record, so disarming before the drain would wipe
        // exactly the dabs we want. An aborted GPU stroke must never leave
        // Bypass armed (audit H1's early-return trap).
        let dabs = self.brush.inner_mut().inner_mut().drain_dab_records();
        self.brush
            .inner_mut()
            .inner_mut()
            .set_dab_recording_all(mn_brush::RecordMode::Off);
        // Tear the smudge oracle down FIRST — before any path below can
        // return — so no later readonly fetch (a twin's end_atomic, a
        // following stroke's get_color on this thread) can reach through a
        // freed ctx.
        if st.smudge {
            mn_brush::set_tile_oracle(None);
            if let Some(ctx) = self.dab_smudge_ctx.take() {
                // Drop the Box<(*mut Renderer, layer)>; the renderer pointer
                // inside is never dereferenced here.
                unsafe { drop(Box::from_raw(ctx as *mut (*mut mn_gpu::Renderer, usize))) };
            }
        }
        if !dabs.is_empty() {
            let tex = self.brush.inner().inner().texture_flush();
            if st.wash {
                // `end` left the buffer alive under BYPASS (the GPU owns the
                // commit). The fallback blank exists only for the impossible
                // window — and matters because an early return here would
                // strand the renderer's stroke state: `end_dab_stroke` below
                // must run on every path.
                let blank;
                let buf = match self.brush.inner().inner().wash_buffer() {
                    Some(b) => b,
                    None => {
                        blank = mn_core::Document::new(self.doc.size.0, self.doc.size.1);
                        &blank
                    }
                };
                self.renderer.flush_wash_dabs(buf, &dabs, st.hard, tex);
            } else {
                self.renderer.flush_dabs(&self.doc, &dabs, st.hard, tex);
            }
            st.all_dabs.extend(dabs);
            st.flushes += 1;
        }
        let Some((layer, wash, tiles)) = self.renderer.end_dab_stroke() else {
            return;
        };
        let t0 = Instant::now();
        let (px, canary_ok) = self.renderer.readback_dab_tiles(layer, &tiles);
        if wash {
            // #0.1 wash commit: the readback IS the buffer content — assemble
            // it into a scratch doc and run the CPU `commit_wash` math
            // unchanged (wet semantics identical by construction; the GPU
            // only replaced the per-dab rasterization). Runs inside the
            // still-open op bracket like every stroke end. The sentinel
            // cache entries are dropped afterwards — they belong to no
            // document layer and would otherwise linger until eviction.
            if canary_ok {
                let mut scratch = mn_core::Document::new(self.doc.size.0, self.doc.size.1);
                for (idx, data) in &px {
                    let tile = scratch.active_layer_mut().tile_mut(*idx);
                    tile.data_mut()[..data.len()].copy_from_slice(data);
                }
                let (opacity, blend, erase) = self.brush.inner().inner().wash_commit_params();
                mn_brush::commit_wash(&scratch, &mut self.doc, opacity, blend, erase);
            } else {
                // Canary repair for wash: replay the dabs into a scratch
                // buffer with the Rust rasterizer, then the same commit.
                eprintln!(
                    "[gpu-dabs] canary mismatch after {} flushes — CPU repair (wash, {} dabs)",
                    st.flushes,
                    st.all_dabs.len()
                );
                let mut scratch = mn_core::Document::new(self.doc.size.0, self.doc.size.1);
                let tex = self.brush.inner().inner().texture_flush();
                mn_brush::rasterize_dabs(&mut scratch, 0, &st.all_dabs, st.hard, tex);
                let (opacity, blend, erase) = self.brush.inner().inner().wash_commit_params();
                mn_brush::commit_wash(&scratch, &mut self.doc, opacity, blend, erase);
                self.dab_path_last = "gpu → cpu repair!".into();
            }
            self.renderer.drop_wash_tiles();
            self.brush.inner_mut().inner_mut().take_wash_buffer();
        } else if canary_ok {
            for (idx, data) in px {
                let tile = self.doc.layers[layer].tile_mut(idx);
                let rev = tile.revision();
                tile.data_mut()[..data.len()].copy_from_slice(&data);
                self.renderer.mark_dab_tile_clean(layer, idx, rev);
            }
        } else {
            // A dispatch was dropped: the GPU pixels are untrustworthy, so
            // they never reach the CPU tiles — re-rasterize the whole stroke
            // from the record instead (worst case = today's speed).
            // NO mark_dab_tile_clean here (auditor round 33): the textures
            // still hold the INCOMPLETE GPU pixels — the dropped dispatch's
            // dabs are missing from them — so claiming texture==CPU at the
            // repaired revision froze needs_upload off forever and the
            // canvas kept showing the incomplete stroke while the document
            // was fine. The ordinary revision compare uploads the repair.
            eprintln!(
                "[gpu-dabs] canary mismatch after {} flushes — CPU repair ({} dabs)",
                st.flushes,
                st.all_dabs.len()
            );
            crate::testlog::line(&format!(
                "[dab] CANARY REPAIR after {} flushes ({} dabs) — adapter dropped a dispatch",
                st.flushes,
                st.all_dabs.len()
            ));
            self.diag.dab_canary_repairs += 1;
            let tex = self.brush.inner().inner().texture_flush();
            mn_brush::rasterize_dabs(&mut self.doc, layer, &st.all_dabs, st.hard, tex);
            self.dab_path_last = "gpu → cpu repair!".into();
        }
        if canary_ok {
            self.dab_path_last = format!(
                "gpu | {} tiles, readback {:.1} ms",
                tiles.len(),
                t0.elapsed().as_secs_f32() * 1000.0
            );
            crate::testlog::line(&self.dab_path_last);
        }
    }

    // --- touch: one finger pans, two fingers pinch-zoom --------------------

    pub fn touch_down(&mut self, id: u32, x: f32, y: f32) {
        // First touch = the user owns the view now (see App::render's
        // deferred startup fit).
        self.startup_fit_pending = false;
        self.touch.insert(id, (x, y));
        // Any contact-set change restarts the twist gesture (a new pair,
        // a third finger, a lift): the accumulator anchors to NOW.
        self.touch_twist.reset(self.viewport.rotate_rad);
    }

    pub fn touch_up(&mut self, id: u32) {
        self.touch.remove(&id);
        self.touch_twist.reset(self.viewport.rotate_rad);
    }

    pub fn touch_move(&mut self, id: u32, x: f32, y: f32) {
        let Some(&(ox0, oy0)) = self.touch.get(&id) else {
            return;
        };
        match self.touch.len() {
            1 => {
                self.viewport.pan[0] += x - ox0;
                self.viewport.pan[1] += y - oy0;
            }
            _ => {
                // Pinch against the other finger (each finger's message sees
                // the other as static; the two half-updates compose). Anchor
                // on the pair midpoint so zoom + drag feel like one gesture.
                let other = self.touch.iter().find(|(k, _)| **k != id).map(|(_, v)| *v);
                let Some((bx, by)) = other else { return };
                let d0 = ((ox0 - bx).powi(2) + (oy0 - by).powi(2)).sqrt().max(1.0);
                let d1 = ((x - bx).powi(2) + (y - by).powi(2)).sqrt().max(1.0);
                let m0 = [(ox0 + bx) * 0.5, (oy0 + by) * 0.5];
                let m1 = [(x + bx) * 0.5, (y + by) * 0.5];
                self.viewport.zoom_around(m0, (d1 / d0).clamp(0.5, 2.0));
                // Two-finger ROTATE rides the same pair (pinch-twist is ONE
                // gesture — Procreate/CSP/Qt never gate rotate against
                // zoom). The interleaved half-updates compose exactly (each
                // half-rotation is θ/2; the half-zooms cos(θ/2)·1/cos(θ/2)
                // cancel), so the pair angle's per-event delta feeds the
                // accumulator unchanged. What changed in 2026-08-19 is
                // everything AROUND it (research/touch-rotation.md):
                //  - an activation threshold + LATCH keeps touch-noise from
                //    rotating during a pure pinch (OpenLayers' pattern);
                //  - the 90° snap became engage/release hysteresis over the
                //    RAW angle — the displayed angle is DERIVED every event
                //    and never written back, so slow twists can always
                //    leave a quarter (the old absolute-set pinned them).
                let a0 = (oy0 - by).atan2(ox0 - bx);
                let a1 = (y - by).atan2(x - bx);
                // Wrap to (−π, π]: atan2 has a branch cut at ±π, and a
                // finger pair passing through level jumps the raw delta by
                // ~2π. Unwrapped, that tripped the activation latch during
                // a pure pinch and popped a held quarter loose
                // ((target − q).abs() suddenly ≥ SNAP_RELEASE) — the exact
                // complaints r95 was written to fix, back through a door
                // r95 never touched.
                let mut delta = a1 - a0;
                if delta > std::f32::consts::PI {
                    delta -= std::f32::consts::TAU;
                } else if delta < -std::f32::consts::PI {
                    delta += std::f32::consts::TAU;
                }
                let g = &mut self.touch_twist;
                g.raw += delta;
                if !g.live && g.raw.abs() > TOUCH_TWIST_THRESHOLD {
                    g.live = true;
                }
                if g.live {
                    let quarter = std::f32::consts::FRAC_PI_2;
                    let target = g.start_rad + g.raw;
                    match g.holding {
                        Some(q) => {
                            if (target - q).abs() >= SNAP_RELEASE {
                                g.holding = None;
                            }
                        }
                        None => {
                            let q = (target / quarter).round() * quarter;
                            if (target - q).abs() < SNAP_ENGAGE {
                                g.holding = Some(q);
                            }
                        }
                    }
                    let shown = g.holding.unwrap_or(target);
                    self.viewport.set_rotation_around(m0, shown);
                }
                self.viewport.pan[0] += m1[0] - m0[0];
                self.viewport.pan[1] += m1[1] - m0[1];
            }
        }
        self.touch.insert(id, (x, y));
        self.needs_redraw = true;
    }

    /// Close the in-progress polyline frame into a panel (Enter / click on
    /// the first vertex).
    pub fn finish_frame_poly(&mut self) {
        if let Some(pts) = self.frame_poly.take() {
            if pts.len() >= 3 {
                let points: Vec<[f32; 2]> = pts.iter().map(|p| [p.0, p.1]).collect();
                self.push_cmd(AppCmd::FramePoly { points });
            } else {
                self.set_status("a panel needs at least 3 corners");
            }
            self.needs_redraw = true;
        }
    }

    pub fn cancel_frame_poly(&mut self) {
        if self.frame_poly.take().is_some() {
            self.set_status("polyline frame cancelled");
            self.needs_redraw = true;
        }
    }

    /// Fold a freshly drawn selection shape into the document under the
    /// active combine mode (SE-022: the modifier held at release overrides
    /// the persistent Tool Property choice). EVERY selection source goes
    /// through here — rectangle, lasso, magnetic lasso — so New / Add /
    /// Subtract / Intersect mean the same thing whichever one drew it.
    pub fn commit_selection(&mut self, s: Selection) {
        if s.is_empty() {
            self.push_cmd(AppCmd::Deselect);
            return;
        }
        let m = self.shell.sync_modifiers();
        let op = crate::cmd::effective_sel_op(m.shift, m.alt, self.sel_op);
        let combined = match &self.doc.selection {
            Some(cur) if op != mn_core::SelectionOp::Replace => cur.combine(&s, &self.doc, op),
            _ => s,
        };
        if combined.is_empty() {
            // Subtracted away to nothing: an empty Selection means
            // "everything", so a real deselect instead.
            self.push_cmd(AppCmd::Deselect);
        } else {
            self.doc.selection = Some(combined);
            self.doc.touch();
        }
        self.needs_redraw = true;
    }

    /// L-001: close the magnetic outline (Enter, or a click back on the
    /// first anchor) and fold it into the selection.
    pub fn magnetic_close(&mut self) {
        let Some(mut lasso) = self.magnetic.take() else {
            return;
        };
        let poly = lasso.close(&self.doc);
        if poly.len() < 3 {
            self.set_status("trace further round before closing the lasso");
            self.needs_redraw = true;
            return;
        }
        let s = Selection::from_polygon(&self.doc, &poly);
        self.commit_selection(s);
    }

    pub fn magnetic_cancel(&mut self) {
        if self.magnetic.take().is_some() {
            self.set_status("magnetic lasso cancelled");
            self.needs_redraw = true;
        }
    }

    /// L-002 Backspace: take the last anchor back. At the FIRST anchor there
    /// is nothing left to undo, so the trace cancels — one key, no dead end
    /// where Backspace silently does nothing.
    pub fn magnetic_undo_anchor(&mut self) {
        let (px, py) = self.last_pointer;
        let (cx, cy) = self.viewport.to_canvas(px as f32, py as f32);
        let at = (cx.round() as i32, cy.round() as i32);
        let popped = match self.magnetic.as_mut() {
            Some(l) => l.undo_anchor(),
            None => return,
        };
        if !popped {
            self.magnetic_cancel();
            return;
        }
        let left = match self.magnetic.as_mut() {
            Some(l) => {
                // Re-aim from the anchor that is now last, so the preview is
                // live again without waiting for the next pointer move.
                l.track(&self.doc, at);
                l.anchors().len()
            }
            None => 0,
        };
        self.set_status(format!("anchor removed — {left} placed"));
        self.needs_redraw = true;
    }

    /// S-001: which layer drew the pixel under the cursor. Topmost first, so
    /// the answer is what the eye sees; hidden layers, folders and the
    /// `pick_exclude` kinds never win. `None` = nothing eligible has ink
    /// there.
    pub fn layer_at(&self, cx: i32, cy: i32) -> Option<usize> {
        /// A click should land on ink you can SEE, not on the 2% fringe of
        /// an antialiased line one layer up. ~10% of full alpha.
        const PICK_MIN_ALPHA: u16 = 3277;
        let vis = self.doc.effective_visibility();
        let drafts = self.doc.effective_drafts();
        let idx = mn_core::tile::TileIdx::of_pixel(cx, cy);
        let (ox, oy) = idx.origin();
        for i in (0..self.doc.layers.len()).rev() {
            let l = &self.doc.layers[i];
            if !vis[i] || l.folder {
                continue;
            }
            let ex = &self.pick_exclude;
            if (ex.draft && drafts[i])
                || (ex.text && l.is_text())
                || (ex.locked && l.lock)
                // "Fill layer" means both spellings of the flats: a LIVE
                // fill/gradient/tone layer, and a raster carrying tone
                // parameters. The switch is about what covers the page, not
                // about which of the two ways it was made.
                || (ex.fill
                    && (matches!(l.kind, mn_core::LayerKind::Fill(_)) || l.tone.is_some()))
            {
                continue;
            }
            let Some(t) = l.display_tile(idx) else {
                continue;
            };
            if t.pixel((cx - ox) as usize, (cy - oy) as usize)[3] >= PICK_MIN_ALPHA {
                return Some(i);
            }
        }
        None
    }

    /// S-001 click: jump the Layers palette to whichever layer owns the
    /// clicked pixel.
    pub fn pick_layer_at(&mut self, cx: f32, cy: f32) {
        let (x, y) = (cx.floor() as i32, cy.floor() as i32);
        if x < 0 || y < 0 || x >= self.doc.size.0 as i32 || y >= self.doc.size.1 as i32 {
            return;
        }
        match self.layer_at(x, y) {
            Some(i) => {
                let name = self.doc.layers[i].name.clone();
                self.push_cmd(AppCmd::SelectLayer(i));
                self.set_status(format!("layer: {name}"));
            }
            None => self.set_status("no layer draws there (check the Exclude switches)"),
        }
        self.needs_redraw = true;
    }

    /// `,` / `.` — step through the active tool's Sub Tool list, CSP-style.
    pub fn step_subtool(&mut self, forward: bool) {
        let dir: i32 = if forward { 1 } else { -1 };
        let cycle =
            |cur: usize, n: usize| -> usize { ((cur as i32 + dir).rem_euclid(n as i32)) as usize };
        match self.tool {
            Tool::Pen | Tool::Eraser => {
                if self.presets.is_empty() {
                    return;
                }
                let cur = self.selected_preset.unwrap_or(0);
                let next = cycle(cur, self.presets.len());
                let p = self.presets[next].1.clone();
                self.push_cmd(AppCmd::SelectBrush(p));
            }
            // ONE cycle over the whole Selection list: the four shapes, then
            // the two paint sub tools folded in from the strip (2026-08-23).
            // Stepping onto or off a paint row switches the tool, so `,`/`.`
            // walks exactly the rows `ui/subtool.rs` draws.
            Tool::Select | Tool::SelPen | Tool::SelEraser => {
                const SHAPES: [SelectMode; 4] = [
                    SelectMode::Rect,
                    SelectMode::Lasso,
                    SelectMode::Magnetic,
                    SelectMode::Shrink,
                ];
                let cur = match self.tool {
                    Tool::SelPen => SHAPES.len(),
                    Tool::SelEraser => SHAPES.len() + 1,
                    _ => SHAPES
                        .iter()
                        .position(|m| *m == self.select_mode)
                        .unwrap_or(0),
                };
                match cycle(cur, SHAPES.len() + 2) {
                    4 => self.push_cmd(AppCmd::SetTool(Tool::SelPen)),
                    5 => self.push_cmd(AppCmd::SetTool(Tool::SelEraser)),
                    i => {
                        if self.tool != Tool::Select {
                            self.push_cmd(AppCmd::SetTool(Tool::Select));
                        }
                        self.select_mode = SHAPES[i];
                    }
                }
                self.magnetic = None;
            }
            Tool::Object => {
                self.object_mode = match self.object_mode {
                    ObjectMode::Object => ObjectMode::PickLayer,
                    ObjectMode::PickLayer => ObjectMode::Object,
                };
                self.set_status(self.object_mode.label());
            }
            Tool::Fill => {
                // The stepper walks the Fill tool's SUB TOOLS. It used to
                // flip the 参照 pair instead, back when refer was the only
                // axis the tool had; with FI-003/FI-004 landed, refer is a
                // parameter of the Click sub tool (strip rows + the Tool
                // Property dropdown, both one click) and the key belongs to
                // the three aiming modes.
                const M: [FillMode; 3] = [FillMode::Click, FillMode::Enclose, FillMode::Lasso];
                let cur = M.iter().position(|m| *m == self.fill_mode).unwrap_or(0);
                self.fill_mode = M[cycle(cur, M.len())];
                self.fill_drag = None;
                self.set_status(self.fill_mode.label());
            }
            Tool::Tone => {
                // The Tone tool's sub tools are the screen shapes.
                const M: [mn_core::tone::TonePattern; 9] = mn_core::tone::TonePattern::ALL;
                let cur = M
                    .iter()
                    .position(|p| *p == self.tone_opts.tone.pattern)
                    .unwrap_or(0);
                self.tone_opts.tone.pattern = M[cycle(cur, M.len())];
                self.set_status(self.tone_opts.tone.pattern.label());
            }
            Tool::Wand => {
                // The wand's sub tools are its three 参照 rows — all three,
                // since the Sub Tool list lists all three (the old pair flip
                // could never reach "refer reference layer" from the key).
                const M: [mn_core::FillRefer; 3] = [
                    mn_core::FillRefer::All,
                    mn_core::FillRefer::Active,
                    mn_core::FillRefer::Reference,
                ];
                let cur = M
                    .iter()
                    .position(|r| *r == self.wand_opts.refer)
                    .unwrap_or(0);
                self.wand_opts.refer = M[cycle(cur, M.len())];
            }
            Tool::Balloon => {
                const M: [BalloonMode; 4] = [
                    BalloonMode::Ellipse,
                    BalloonMode::Round,
                    BalloonMode::Draw,
                    BalloonMode::Tail,
                ];
                let cur = M.iter().position(|m| *m == self.balloon_mode).unwrap_or(0);
                self.balloon_mode = M[cycle(cur, M.len())];
            }
            Tool::Frame => {
                const M: [FrameMode; 5] = [
                    FrameMode::Rect,
                    FrameMode::Polyline,
                    FrameMode::Pen,
                    FrameMode::DivideFolder,
                    FrameMode::DivideBorder,
                ];
                let cur = M.iter().position(|m| *m == self.frame_mode).unwrap_or(0);
                self.frame_mode = M[cycle(cur, M.len())];
                self.frame_poly = None;
                self.frame_pen = None;
            }
            Tool::Eyedrop => {
                // E-014: three referents now, so the stepper walks them
                // (the `,`/`.` direction counts, unlike the old flip).
                const M: [mn_core::FillRefer; 3] = [
                    mn_core::FillRefer::All,
                    mn_core::FillRefer::Active,
                    mn_core::FillRefer::Reference,
                ];
                let cur = M
                    .iter()
                    .position(|m| *m == self.eyedrop_opts.refer)
                    .unwrap_or(0);
                self.eyedrop_opts.refer = M[cycle(cur, M.len())];
            }
            Tool::Figure => {
                // Direct-draw shapes only: cycling through Stream/Saturated
                // line too would make the shortcut a six-stop tour where a
                // stray press generates a layer — those two are a deliberate
                // sub-tool-list (or Ctrl+K) pick.
                const M: [FigureMode; 4] = [
                    FigureMode::Line,
                    FigureMode::Rect,
                    FigureMode::Ellipse,
                    FigureMode::Polygon,
                ];
                let cur = M.iter().position(|m| *m == self.figure_mode).unwrap_or(0);
                self.figure_mode = M[cycle(cur, M.len())];
                self.figure_drag = None;
                self.figure_poly = None;
            }
            Tool::Gradient => {
                const M: [GradMode; 3] = [
                    GradMode::FgToBg,
                    GradMode::FgToTransparent,
                    GradMode::TransparentToFg,
                ];
                let cur = M.iter().position(|m| *m == self.grad_mode).unwrap_or(0);
                self.grad_mode = M[cycle(cur, M.len())];
                self.grad_drag = None;
            }
            Tool::Pan => {
                self.pan_mode = match self.pan_mode {
                    PanMode::Hand => PanMode::Rotate,
                    PanMode::Rotate => PanMode::Hand,
                };
            }
            // Object has its own arm above (S-001 gave it two sub tools).
            Tool::Text => {}
        }
        self.needs_redraw = true;
    }

    /// `[` / `]` step the brush's pixel DIAMETER through the CSP-style ladder
    /// below. Owner ask 2026-08-17: the old ×1.15 percentage stepping drifted
    /// sizes onto ugly values; CSP's feel is round numbers with gradiated
    /// steps.
    ///
    /// The rung IS the value now — the Size control edits the same absolute
    /// px, so nothing between here and the engine can cap it (it used to be
    /// squeezed back through a 0.25..4 multiplier, and a 10 px preset could
    /// therefore never reach the ladder's upper half).
    pub fn step_brush_size(&mut self, up: bool) {
        let target = size_rung(self.brush_radius() * 2.0, up);
        self.push_cmd(AppCmd::SetBrushSizePx(target));
        let text = if target >= 9.95 {
            format!("brush size: {:.0} px", target)
        } else {
            format!("brush size: {:.1} px", target)
        };
        self.set_status(text);
    }

    /// Base dab radius of the current brush, in canvas px — HUD + size readout.
    /// Pressure/speed dynamics move around it, so it is a scale, not a promise.
    pub fn brush_radius(&self) -> f32 {
        self.engine().radius_px()
    }

    pub fn brush_name(&self) -> &str {
        self.engine().name()
    }
}

/// The startup brush: the owner's converted CSP Real G-Pen, then classic
/// `pen.myb`, then `SimpleDab` — each fallback with a line saying so, because
/// a silent downgrade would be very confusing to draw with.
///
/// Returns the engine and the index it occupies in `presets` (so the panel
/// shows the right row selected).
fn default_engine(presets: &[(String, PathBuf)]) -> (Engine, Option<usize>) {
    for want in ["csp/real-g-pen.myb", "classic/pen.myb"] {
        let Some(idx) = presets.iter().position(|(_, p)| p.ends_with(want)) else {
            continue;
        };
        match MyBrush::load(&presets[idx].1) {
            Ok(b) => return (Engine::new(EngineKind::My(Box::new(b))), Some(idx)),
            Err(e) => {
                eprintln!("[ui] default brush {want} failed to load: {e}");
            }
        }
    }
    eprintln!("[ui] falling back to the placeholder dab brush");
    (Engine::new(EngineKind::Dab(SimpleDab::new())), None)
}

/// The MRU list lives next to the exe as plain lines — no config framework.
fn recent_path() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("recent.txt"))
}

/// `pub(crate)` for the crash-recovery scan: the MRU is where the documents
/// whose autosaves might need offering back are named (PR-040). `n` is the
/// `recent_depth` preference — 8 until someone changes it.
pub(crate) fn load_recent_n(n: usize) -> Vec<PathBuf> {
    recent_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| {
            s.lines()
                .filter(|l| !l.trim().is_empty())
                .take(n)
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

fn save_recent(list: &[PathBuf]) {
    if let Some(p) = recent_path() {
        let body: String = list.iter().map(|q| format!("{}\n", q.display())).collect();
        let _ = std::fs::write(p, body);
    }
}

/// The Color Set lives next to the exe as `#rrggbb` lines — no config
/// framework, same policy as `recent.txt`.
fn swatches_path() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("swatches.txt"))
}

/// One `swatches.txt` line: `#rrggbb`, or `#rrggbb<TAB>Name` since the
/// `.gpl` import started keeping the names its palettes carry. Junk lines
/// return `None` and are skipped — a hand-edited file must never fail.
fn parse_swatch_line(l: &str) -> Option<mn_core::palette::Swatch> {
    let l = l.trim();
    let (hex, name) = match l.split_once(char::is_whitespace) {
        Some((h, n)) => (h, n.trim()),
        None => (l, ""),
    };
    Some(mn_core::palette::Swatch {
        rgb: mn_core::palette::parse_hex(hex)?,
        name: name.to_owned(),
    })
}

fn load_swatches() -> Option<Vec<mn_core::palette::Swatch>> {
    let text = swatches_path().and_then(|p| std::fs::read_to_string(p).ok())?;
    let out: Vec<mn_core::palette::Swatch> = text.lines().filter_map(parse_swatch_line).collect();
    (!out.is_empty()).then_some(out)
}

/// The `swatches.txt` text for a Color Set. Split from the write so the
/// format round-trips under test without touching the disk.
fn swatches_body(sw: &[mn_core::palette::Swatch]) -> String {
    sw.iter()
        .map(|c| {
            let hex = mn_core::palette::hex_string(c.rgb);
            // A name must not carry the separators: a newline inside one
            // would split a single swatch into two lines of junk.
            let name = c.name.replace(['\n', '\r', '\t'], " ");
            match name.trim() {
                "" => format!("{hex}\n"),
                n => format!("{hex}\t{n}\n"),
            }
        })
        .collect()
}

pub fn save_swatches(sw: &[mn_core::palette::Swatch]) {
    if let Some(p) = swatches_path() {
        let _ = std::fs::write(p, swatches_body(sw));
    }
}

/// How many recently used colours the Color palette's Recent strip keeps —
/// the same cap the font list's recently-used row uses.
pub const COLOR_HISTORY_MAX: usize = 10;

/// Where the Color Set stops growing on its own. Only the automatic paths
/// (auto-registered eyedropper picks, Add all recent) respect it; the `+`
/// button and a `.gpl` import are deliberate acts and are never refused.
pub const SWATCH_CAP: usize = 256;

/// Push a used colour onto the front of the history, de-duplicated at the
/// 8-bit precision it displays at and bounded to [`COLOR_HISTORY_MAX`].
/// Re-using an older colour moves it to the front rather than adding a
/// second copy. A free function so the rule is testable without a GPU.
pub fn push_color_history(list: &mut Vec<[f32; 3]>, rgb: [f32; 3]) {
    let q = mn_core::palette::quantize8(rgb);
    list.retain(|c| *c != q);
    list.insert(0, q);
    list.truncate(COLOR_HISTORY_MAX);
}

/// What happens to an eyedropped colour (CO-023). Pure, so the rule and
/// both of its refusals are testable without a device or a disk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickReg {
    /// The switch is off — the default, and where most users should leave
    /// it: the Recent strip already remembers picks, and forgets them.
    Off,
    /// Already in the set. Sampling the same ink twenty times adds one.
    Duplicate,
    /// The set is at [`SWATCH_CAP`]. Nothing that happens behind the user
    /// gets to keep filling the half of the palette he curates.
    Full,
    Added,
}

pub fn pick_registration(
    auto: bool,
    swatches: &[mn_core::palette::Swatch],
    picked: [f32; 3],
) -> PickReg {
    let q = mn_core::palette::quantize8(picked);
    if !auto {
        PickReg::Off
    } else if swatches.iter().any(|s| s.rgb == q) {
        PickReg::Duplicate
    } else if swatches.len() >= SWATCH_CAP {
        PickReg::Full
    } else {
        PickReg::Added
    }
}

/// CSP's "Standard color set", sampled from the owner's palette screenshot
/// (docs/design/reference-2026-08-14/31.png) row by row. The transparent
/// swatch is omitted — transparency is a colour *slot* here, not a swatch.
fn default_swatches() -> Vec<mn_core::palette::Swatch> {
    const HEX: [u32; 39] = [
        // blacks → dark greys
        0x000000, 0xFFFFFF, 0x0C0C0C, 0x191919, 0x262626, 0x333333, 0x4C4C4C,
        // greys → white
        0x666666, 0x7F7F7F, 0x999999, 0xB3B3B3, 0xCCCCCC, 0xD9D9D9, 0xE6E6E6, 0xF3F3F3,
        // primaries + dark blue-greys
        0xFF0000, 0xFFFF00, 0x00FF00, 0x00FFFF, 0x0000FF, 0xFF00FF, 0x2A2C30, 0x464B55,
        // slate blues + browns
        0x798BA8, 0xA7B0C8, 0xC8CFE4, 0x36322D, 0x56493D, 0x6B5745, 0xB58F7B, 0xC0A292,
        // pastels
        0xFF9999, 0xFFBC99, 0xFFDB99, 0xFFFF99, 0xCBFF99, 0x99FFA9, 0x99FFE9, 0x99DBFF,
    ];
    HEX.iter()
        .map(|v| {
            mn_core::palette::Swatch::new([
                ((v >> 16) & 0xff) as f32 / 255.0,
                ((v >> 8) & 0xff) as f32 / 255.0,
                (v & 0xff) as f32 / 255.0,
            ])
        })
        .collect()
}

/// CSP-style brush-size ladder for `[` / `]`: round numbers, fine steps at
/// pen widths, coarser as sizes grow. The owner's examples pin the shape:
/// `]` on 1 px → 2, on 100 → 120, on 120 → 150.
const SIZE_RUNGS: [f32; 40] = [
    1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 25.0, 30.0,
    35.0, 40.0, 45.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 120.0, 150.0, 180.0, 200.0, 250.0,
    300.0, 400.0, 500.0, 600.0, 800.0, 1000.0, 1200.0, 1500.0, 2000.0,
];

/// Next/previous ladder rung for a diameter. Off-rung values snap past
/// themselves (3.7 steps to 4 / 3); beyond the ladder the steps stay
/// gradiated (×1.25 up, ÷1.25 down, floor 1 px).
fn size_rung(cur: f32, up: bool) -> f32 {
    if up {
        SIZE_RUNGS
            .iter()
            .copied()
            .find(|&r| r > cur + 1e-3)
            .unwrap_or((cur * 1.25).max(1.0))
    } else {
        SIZE_RUNGS
            .iter()
            .copied()
            .rev()
            .find(|&r| r < cur - 1e-3)
            .unwrap_or((cur / 1.25).max(1.0))
    }
}

/// The fit against an arbitrary screen rect — `origin` is its top-left in
/// client px. `fitted_viewport` is this against the whole surface. `margin`
/// is the `fit_margin` preference (see `fitted_viewport` for its history).
fn fitted_viewport_in(doc: &Document, origin: [f32; 2], size: (f32, f32), margin: f32) -> Viewport {
    let zoom =
        ((size.0 / doc.size.0 as f32).min(size.1 / doc.size.1 as f32) * margin).clamp(0.05, 8.0);
    Viewport {
        pan: [
            origin[0] + (size.0 - doc.size.0 as f32 * zoom) * 0.5,
            origin[1] + (size.1 - doc.size.1 as f32 * zoom) * 0.5,
        ],
        zoom,
        rotate_rad: 0.0,
        flip_h: false,
        flip_v: false,
    }
}

fn fitted_viewport(doc: &Document, client: (u32, u32), margin: f32) -> Viewport {
    // Shipped margin 0.98, not 0.90 (owner, 2026-08-19: the startup canvas
    // "opens too small — should open a bit bigger so it's easier to draw
    // initially without having to zoom"). A tenth of the window spent on
    // margin is a lot when the palettes have already taken their share. He
    // has now had an opinion about this number twice, so it is a preference
    // (`fit_margin`) rather than a third edit here — as is the DEFAULT
    // canvas being 2048², which was the other lever and was his call.
    let zoom = ((client.0 as f32 / doc.size.0 as f32).min(client.1 as f32 / doc.size.1 as f32)
        * margin)
        .clamp(0.05, 8.0);
    Viewport {
        pan: [
            (client.0 as f32 - doc.size.0 as f32 * zoom) * 0.5,
            (client.1 as f32 - doc.size.1 as f32 * zoom) * 0.5,
        ],
        zoom,
        rotate_rad: 0.0,
        flip_h: false,
        flip_v: false,
    }
}

/// `assets/brushes/**/*.myb` as (display name, path), sorted. Looked up next
/// to the exe first (a shipped build), then from the working directory
/// (`cargo run` at the repo root), then two levels up from the exe
/// (`target/debug/`). Names come from `BrushLibrary`, which prefers the .myb
/// JSON `"name"` field over the file stem.
fn scan_presets() -> Vec<(String, PathBuf)> {
    brushes_root()
        .map(|root| mn_brush::BrushLibrary::scan(&root))
        .unwrap_or_default()
}

/// The assets/brushes directory next to whichever root actually contains
/// presets — also where texture-tip masks live (`textures/*.png`).
pub(crate) fn brushes_root() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        roots.push(dir.join("assets/brushes"));
        roots.push(dir.join("../../assets/brushes"));
        roots.push(dir.join("../../../assets/brushes"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd.join("assets/brushes"));
    }
    roots
        .into_iter()
        .find(|root| root.is_dir() && !mn_brush::BrushLibrary::scan(root).is_empty())
}

/// The shipped texture-tip masks by name (the Tool Property picker's list).
fn scan_textures(root: Option<&Path>) -> Vec<String> {
    let Some(dir) = root.map(|r| r.join("textures")) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")))
                .then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .flatten()
        })
        .collect();
    names.sort();
    names
}

/// A headless renderer for a test — `None` when this machine has no usable
/// adapter, which is the shared "skip this test" signal.
///
/// The skip is CONDITIONAL on purpose. Nearly every test in this crate opens
/// by asking for one of these and returning early when it cannot have one, so
/// an unconditional skip means the whole app suite PASSES having tested
/// nothing the moment an adapter goes missing — and a green run that means
/// nothing is worse than a red one, because nobody looks at it.
///
/// CI runs with `MN_WARP` set and CI always has the software adapter, so
/// there a missing adapter is a broken build, not an environment to tolerate:
/// it panics. Without `MN_WARP` — a developer's machine, where the adapter
/// really can be absent — the skip stands.
#[cfg(test)]
pub(crate) fn headless_renderer() -> Option<mn_gpu::Renderer> {
    let warp = std::env::var("MN_WARP").is_ok();
    match mn_gpu::Renderer::new_headless(mn_gpu::GpuConfig {
        force_fallback: warp,
        no_vsync: false,
    }) {
        Ok(r) => Some(r),
        Err(e) => {
            assert!(
                !warp,
                "MN_WARP is set, so the software adapter must work: {e}"
            );
            println!("[test] SKIP: no usable adapter");
            None
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod grid_engine_tests;

/// Documents, tabs, and what "new" means.
///
/// Every test here is deliberately FRUGAL: a new comic defaults to Shueisha
/// A at 600 dpi, which is a ~6000×8600 canvas, and two of those alive at
/// once put the software (WARP) adapter out of memory on a normal machine —
/// observed, not feared. So the drafts run at 72 dpi and the apps are built
/// one at a time.
#[cfg(test)]
mod new_document_tests;

#[cfg(test)]
mod document_tab_tests;

#[cfg(test)]
mod text_editor_undo_tests;

#[cfg(test)]
mod open_in_tab_tests;

#[cfg(test)]
mod unsaved_across_tabs_tests;

/// 05 item 1 — the pathless-work autosave is an incremental TEMP work
/// folder. Same frugality rule as `new_document_tests`.
#[cfg(test)]
mod autosave_folder_tests;

/// r125's data-loss fixes, pinned. Same frugality rule as
/// `new_document_tests`: 72 dpi drafts, one App at a time.
#[cfg(test)]
mod parked_document_tests;

/// What a tab click has to forget, and the one thing it must not.
#[cfg(test)]
mod tab_switch_state_tests;

#[cfg(test)]
mod tcy_button_tests;

/// PM-050..055: the batch-export options and the script dump.
#[cfg(test)]
mod export_and_script_tests;

#[cfg(test)]
mod tone_round_tests;

/// ROADMAP "further out": one-gesture screentone — the structure one click
/// produces, the single undo press it costs, and the gap closing it
/// inherits from the fill machinery.
#[cfg(test)]
mod tone_gesture_tests;

#[cfg(test)]
mod view_reset_and_tool_lock_tests;

/// ROADMAP good-first-issue #1: the vertical view flip, and the brush
/// compensation it has to reach.
#[cfg(test)]
mod view_flip_tests;

/// Vector inking phase 1: recording, one-step undo, faithful replay.
#[cfg(test)]
mod vector_layer_tests;

/// E-014/E-016: the eyedropper's referent and its averaging box, driven
/// through `dispatch` the way a click drives them. `cmd.rs` has no test
/// module of its own, so command behaviour is tested from here.
#[cfg(test)]
mod eyedropper_tests;

/// TRIAGE 134 (JP #4) — a turned balloon takes its lettering with it, and the
/// lettering is still lettering afterwards.
#[cfg(test)]
mod balloon_carries_text_tests;

/// ROADMAP good-first-issue #1 — "Fit a balloon to its text": which text it
/// pairs with, and the single undo step the reshape costs.
#[cfg(test)]
mod balloon_fit_tests;

/// ROADMAP good-first-issue: LM-004's mask-stroke undo bracket — the
/// snapshot has to predate the first dab, and undo/redo has to reach the
/// compositor.
#[cfg(test)]
mod mask_stroke_undo_tests;

/// ROADMAP "further out": changing a work's page size after creation —
/// the anchor, the all-pages bytes round trip, and the default new pages
/// inherit. Same frugality rule as `new_document_tests`: 72 dpi drafts.
#[cfg(test)]
mod page_size_tests;

/// ROADMAP good-first-issue: ruler creation/move/clear on the document's
/// one undo history — and the two things that must NOT be steps (the
/// frame-published sync) or lost (a page switch).
#[cfg(test)]
mod ruler_undo_tests;

/// Owner report 2026-08-23: a tone material must place a LIVE tone layer
/// filling the page (or the selection), never a resizable raster float
/// whose scale would resize the dots.
#[cfg(test)]
mod material_tone_tests;

/// Owner report 2026-08-23: an effect-line run you cannot re-select, and
/// driver handles that were off the page when you could.
#[cfg(test)]
mod gen_lines_object_tests;
