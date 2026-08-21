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

/// The RF-001 unit a reference click addresses: the layer itself, or —
/// when it is a folder — the folder plus its child run (every following
/// layer with a deeper depth, up to the first sibling that pops back).
fn reference_unit(doc: &mn_core::Document, i: usize) -> Vec<usize> {
    let Some(l) = doc.layers.get(i) else {
        return Vec::new();
    };
    if !l.folder {
        return vec![i];
    }
    let d = l.depth;
    let mut out = vec![i];
    for (j, m) in doc.layers.iter().enumerate().skip(i + 1) {
        if m.depth > d {
            out.push(j);
        } else {
            break;
        }
    }
    out
}

/// The clipboard's operand (TRIAGE 131): the selection's bounds when one
/// exists, else the layer's populated bounds — canvas-clipped, lifted
/// selection-masked. `None` when there is nothing there.
fn lift_clipboard_source(app: &App) -> Option<mn_core::FloatSource> {
    let l = app.doc.active_layer();
    let rect = if let Some(sel) = &app.doc.selection {
        selection_bbox(sel)
    } else {
        l.tile_bounds()
            .map(|(x, y, w, h)| [x, y, x + w as i32, y + h as i32])
    }?;
    let rect = [
        rect[0].max(0),
        rect[1].max(0),
        rect[2].min(app.doc.size.0 as i32),
        rect[3].min(app.doc.size.1 as i32),
    ];
    if rect[0] >= rect[2] || rect[1] >= rect[3] {
        return None;
    }
    let src = mn_core::transform::lift_region(l, rect, app.doc.selection.as_ref());
    (!src.tiles.is_empty()).then_some(src)
}

/// Store a lifted source as the app's clipboard + the OS clipboard (DIB,
/// best-effort — other apps get the 8-bit copy, we keep the fix15 original).
fn store_clipboard(app: &mut App, src: mn_core::FloatSource) {
    let (bgra, w, h) = crate::clipboard::floatsource_to_bgra(&src);
    let os_ok = crate::clipboard::clipboard_set_dib(&bgra, w as usize, h as usize);
    let (rw, rh) = (src.rect[2] - src.rect[0], src.rect[3] - src.rect[1]);
    app.clipboard = Some(src);
    app.set_status(if os_ok {
        format!("copied {rw}×{rh} px")
    } else {
        format!("copied {rw}×{rh} px (OS clipboard unavailable)")
    });
}

/// Where a panel paste lands (owner HIGH 2026-08-18). `folder` names the
/// frame-folder header whose seal clips the art; `None` with a rect is the
/// selection-bbox rule — a centring target only, the stamp stays on the
/// active layer.
pub(crate) struct PasteTarget {
    pub folder: Option<usize>,
    /// True when the folder already owns the active layer (rule 1): the
    /// stamp goes onto the active layer, exactly as before, only aimed.
    pub owns_active: bool,
    /// Panel (or selection) rect, canvas px, `[x0, y0, x1, y1]`.
    pub rect: [f32; 4],
    /// Status-line name — the folder's layer name.
    pub label: String,
}

/// The frame folder enclosing `i`, nearest first: children sit BELOW their
/// header in `layers`, so the block closes at the first folder above with
/// a smaller depth. Walks out through plain folders too (a subfoldered
/// panel is still inside the panel).
fn enclosing_folder(doc: &mn_core::Document, i: usize) -> Option<usize> {
    let d = doc.layers[i].depth;
    if d == 0 {
        return None;
    }
    (i + 1..doc.layers.len()).find(|&j| doc.layers[j].folder && doc.layers[j].depth < d)
}

/// Which frame of a multi-frame folder the paste aims at: the one holding
/// the active layer's content centre, else the first.
fn frame_index_for(doc: &mn_core::Document, folder: usize, active: usize) -> usize {
    let Some(fs) = doc.layers[folder].frames() else {
        return 0;
    };
    if fs.frames.len() < 2 {
        return 0;
    }
    let c = doc.layers[active]
        .tile_bounds()
        .map(|(x, y, w, h)| [x + w as i32 / 2, y + h as i32 / 2])
        .or_else(|| {
            doc.layers[folder]
                .tile_bounds()
                .map(|(x, y, w, h)| [x + w as i32 / 2, y + h as i32 / 2])
        });
    if let Some([cx, cy]) = c {
        for (i, f) in fs.frames.iter().enumerate() {
            let b = f.bbox();
            if cx as f32 >= b[0] && cy as f32 >= b[1] && (cx as f32) < b[2] && (cy as f32) < b[3] {
                return i;
            }
        }
    }
    0
}

/// Paste-to-position target resolution (owner HIGH 2026-08-18), spec order:
/// 1. the frame folder OWNING the active layer (the common case — he is
///    drawing inside a panel and pastes; it goes in that panel),
/// 2. else the panel under the pointer,
/// 3. else the selection's bbox (aim only — no folder),
/// 4. else None → today's behaviour (source coords / view centre).
pub(crate) fn resolve_paste_target(
    doc: &mn_core::Document,
    active: usize,
    pointer_canvas: Option<(f32, f32)>,
) -> Option<PasteTarget> {
    // Rule 1: walk enclosing folders outward to the first frame folder.
    let mut f = enclosing_folder(doc, active);
    while let Some(i) = f {
        if doc.layers[i].is_frame() {
            let fi = frame_index_for(doc, i, active);
            // Invariant: a frame folder holds at least one frame, so `fi`
            // indexes something. An empty set would mean no panel to aim
            // at, which is rule 2's case, not a panic.
            if let Some(fr) = doc.layers[i].frames().and_then(|fs| fs.frames.get(fi)) {
                return Some(PasteTarget {
                    folder: Some(i),
                    owns_active: true,
                    rect: fr.bbox(),
                    label: doc.layers[i].name.clone(),
                });
            }
        }
        f = enclosing_folder(doc, i);
    }
    // Rule 2: the smallest panel containing the pointer, across all frame
    // folders (smallest so a nested re-division wins over its container).
    // Hidden and locked folders are NOT targets: pasting into a folder the
    // artist hid (to see the rough underneath) commits art that instantly
    // disappears, and a locked folder is one he explicitly protected.
    if let Some((px, py)) = pointer_canvas {
        let vis = doc.effective_visibility();
        let mut best: Option<(f32, usize, usize)> = None; // (area, folder, frame)
        for i in 0..doc.layers.len() {
            if !vis.get(i).copied().unwrap_or(true) || doc.layers[i].lock {
                continue;
            }
            let Some(fs) = doc.layers[i].frames() else {
                continue;
            };
            for (j, fr) in fs.frames.iter().enumerate() {
                let b = fr.bbox();
                if px >= b[0] && py >= b[1] && px < b[2] && py < b[3] {
                    let a = (b[2] - b[0]) * (b[3] - b[1]);
                    if best.is_none_or(|(ba, _, _)| a < ba) {
                        best = Some((a, i, j));
                    }
                }
            }
        }
        if let Some((_, i, j)) = best {
            let rect = doc.layers[i].frames().map(|fs| fs.frames[j].bbox())?;
            return Some(PasteTarget {
                folder: Some(i),
                owns_active: false,
                rect,
                label: doc.layers[i].name.clone(),
            });
        }
    }
    // Rule 3: a selection's bbox aims the paste; no folder, active layer.
    doc.selection
        .as_ref()
        .and_then(selection_bbox)
        .map(|r| PasteTarget {
            folder: None,
            owns_active: true,
            rect: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
            label: "selection".into(),
        })
}

/// Paste (TRIAGE 131 + owner HIGH 2026-08-18): internal clipboard wins —
/// full fidelity plus the source coordinates Paste returns to, CSP-style;
/// an OS DIB has no coordinates and drops where the aim says. `Panel`
/// resolves a paste target (frame folder → pointer panel → selection →
/// old behaviour); `InPlace` is the pre-HIGH Ctrl+V verbatim;
/// `Shown` centres on the view.
#[derive(Clone, Copy, PartialEq)]
enum PasteWhere {
    Panel,
    InPlace,
    Shown,
}

fn paste_float(app: &mut App, where_: PasteWhere) {
    let target = if where_ == PasteWhere::Panel {
        // The pointer rule only fires over the canvas, not over a panel
        // that happens to sit on top of the page.
        let p = app.last_pointer;
        let pointer = (!app.shell.owns_pointer(p.0, p.1)).then(|| {
            let c = app.viewport.to_canvas(p.0 as f32, p.1 as f32);
            (c.0, c.1)
        });
        resolve_paste_target(&app.doc, app.doc.active, pointer)
    } else {
        None
    };
    // Stamping the active layer keeps today's guards; a target that creates
    // its own layer (a pointer panel the active layer is NOT in) skips them.
    let creates_layer = target
        .as_ref()
        .is_some_and(|t| !t.owns_active && t.folder.is_some());
    if !creates_layer {
        let l = app.doc.active_layer();
        if l.lock {
            app.set_status("layer is locked");
            return;
        }
        if l.is_vector() || l.folder {
            app.set_status("Paste targets raster layers");
            return;
        }
    }
    let aim = target.as_ref().map(|t| t.rect);
    let src = app.clipboard.clone().or_else(|| {
        let (bgra, w, h) = crate::clipboard::clipboard_get_dib()?;
        // An external paste with no aim drops centred on the view; with one,
        // seed the float at the target's corner so nothing clips away.
        let (vw, vh) = (w as i32 / 2, h as i32 / 2);
        let at = match aim {
            Some(r) => [r[0] as i32, r[1] as i32],
            None => {
                let c = app
                    .viewport
                    .to_canvas(app.canvas_center()[0], app.canvas_center()[1]);
                [c.0 as i32 - vw, c.1 as i32 - vh]
            }
        };
        Some(crate::clipboard::bgra_to_floatsource(
            &bgra,
            w,
            h,
            at,
            app.doc.size.0 as i32,
            app.doc.size.1 as i32,
        ))
    });
    let Some(src) = src.filter(|s| !s.tiles.is_empty()) else {
        app.set_status("clipboard is empty");
        return;
    };
    if where_ == PasteWhere::Shown && target.is_none() {
        open_float_drag(app, src, true);
    } else {
        open_float_aimed(app, src, target.as_ref());
    }
}

/// The shared float-opening core (clipboard pastes AND material pastes,
/// TRIAGE 131/133): build the TransformDrag with `stamp_on_identity` and
/// optionally re-centre on the view. Layer guards are the CALLER's — this
/// takes a non-empty source and opens the move/scale/commit float.
fn open_float_drag(app: &mut App, src: mn_core::FloatSource, center_on_view: bool) {
    let r = src.rect;
    let preview_tex = crate::app::transform_preview(&src, 2048).map(|img| {
        app.shell
            .ctx
            .load_texture("mn.transform.preview", img, egui::TextureOptions::LINEAR)
    });
    let mut drag = crate::app::TransformDrag {
        source: src,
        xform: mn_core::Affine2::IDENTITY,
        bbox: [
            [r[0] as f32, r[1] as f32],
            [r[2] as f32, r[1] as f32],
            [r[2] as f32, r[3] as f32],
            [r[0] as f32, r[3] as f32],
        ],
        sx: 1.0,
        sy: 1.0,
        rad: 0.0,
        tx: 0.0,
        ty: 0.0,
        pivot_override: None,
        gesture: None,
        stamp_on_identity: true,
        // A paste: nothing was lifted off the layer, so the commit must not
        // clear the source rect (Copy is not Cut).
        clear_source: false,
        lift_selection: None,
        create_in: None,
        order: crate::app::MaterialLayerOrder::Above,
        preview_tex,
    };
    if center_on_view {
        // Centre the float on the current view through the params model, so
        // the gestures stay consistent from here.
        let pivot = drag.pivot();
        let c = app
            .viewport
            .to_canvas(app.canvas_center()[0], app.canvas_center()[1]);
        drag.set_params(1.0, 1.0, 0.0, c.0 - pivot[0], c.1 - pivot[1]);
    }
    app.transform_drag = Some(drag);
    app.set_status("pasted — drag to move, Enter commits, Esc cancels");
    app.mark_dirty();
}

/// A paste with a resolved target (owner HIGH 2026-08-18): the float opens
/// centred on the panel rect, scaled uniformly DOWN to fit when oversized
/// (never up, never cropped), and — when the active layer is not already
/// inside the target folder — the commit creates the layer inside it so
/// the folder seal clips the art to the panel. Same float semantics as
/// ever: drag immediately, Enter commits, Esc cancels with nothing left.
/// r74's owner-approved paste sizing (uniform down-fit, topmost child) —
/// the clipboard path and every default caller.
pub(crate) fn open_float_aimed(
    app: &mut App,
    src: mn_core::FloatSource,
    target: Option<&PasteTarget>,
) {
    open_float_aimed_sized(
        app,
        src,
        target,
        crate::app::MaterialPasteSize::FitPanel,
        crate::app::MaterialLayerOrder::Above,
    );
}

/// The paste landing with EXPLICIT sizing/order (MT-032/034 — the
/// material palette's choices; the clipboard path keeps r74's defaults).
pub(crate) fn open_float_aimed_sized(
    app: &mut App,
    src: mn_core::FloatSource,
    target: Option<&PasteTarget>,
    size_mode: crate::app::MaterialPasteSize,
    order: crate::app::MaterialLayerOrder,
) {
    let create_in = target.filter(|t| !t.owns_active).and_then(|t| t.folder);
    open_float_drag(app, src, false);
    let Some(drag) = app.transform_drag.as_mut() else {
        return;
    };
    drag.create_in = create_in;
    drag.order = order;
    let status = if let Some(t) = target {
        let r = drag.source.rect;
        let (fw, fh) = ((r[2] - r[0]) as f32, (r[3] - r[1]) as f32);
        if fw > 0.0 && fh > 0.0 {
            // MT-032: one fit, five meanings (CSP's vocabulary, named
            // after the job). The default is r74's owner-approved
            // down-fit, verbatim.
            let (tw, th) = (t.rect[2] - t.rect[0], t.rect[3] - t.rect[1]);
            let (fx, fy) = (tw / fw, th / fh);
            let (mx, my) = match size_mode {
                crate::app::MaterialPasteSize::FitPanel => {
                    let s = fx.min(fy).min(1.0);
                    (s, s)
                }
                crate::app::MaterialPasteSize::AdjustAfter => (1.0, 1.0),
                crate::app::MaterialPasteSize::ExpandFull => {
                    let s = fx.max(fy);
                    (s, s)
                }
                crate::app::MaterialPasteSize::FitToScale => {
                    let s = fx.min(fy);
                    (s, s)
                }
                crate::app::MaterialPasteSize::ToDestination => (fx, fy),
            };
            let pivot = drag.pivot();
            let c = [(t.rect[0] + t.rect[2]) * 0.5, (t.rect[1] + t.rect[3]) * 0.5];
            drag.set_params(mx, my, 0.0, c[0] - pivot[0], c[1] - pivot[1]);
        }
        format!(
            "pasted into {} — drag to move, Enter commits, Esc cancels",
            t.label
        )
    } else {
        "pasted — drag to move, Enter commits, Esc cancels".into()
    };
    app.set_status(status);
}

/// The selection-combine op for one gesture: held modifiers OVERRIDE the
/// persistent 4-way mode (the owner's everyday path — Shift = add,
/// Alt = subtract, Shift+Alt = intersect).
pub fn effective_sel_op(
    shift: bool,
    alt: bool,
    persistent: mn_core::SelectionOp,
) -> mn_core::SelectionOp {
    match (shift, alt) {
        (true, true) => mn_core::SelectionOp::Intersect,
        (true, false) => mn_core::SelectionOp::Add,
        (false, true) => mn_core::SelectionOp::Subtract,
        (false, false) => persistent,
    }
}

/// The lift region for whole-layer/selection-wide ops (Transform, Flip):
/// the selection's bounds when one exists, else the layer's populated tile
/// bounds — canvas-clipped either way.
pub(crate) fn transform_lift_rect(app: &App) -> Option<[i32; 4]> {
    let l = app.doc.active_layer();
    let rect = if let Some(sel) = &app.doc.selection {
        selection_bbox(sel)
    } else {
        l.tile_bounds()
            .map(|(x, y, w, h)| [x, y, x + w as i32, y + h as i32])
    };
    rect.map(|r| {
        [
            r[0].max(0),
            r[1].max(0),
            r[2].min(app.doc.size.0 as i32),
            r[3].min(app.doc.size.1 as i32),
        ]
    })
}

/// The keeper's slot after a division (owner top item 2026-08-18): the
/// cut union when ALL its frames lie inside it (a pure division — the
/// common case: both halves share the slot and order inside it); None
/// when untouched panels remain outside, so the folder keeps ordering
/// globally by its own geometry.
/// The frame layer a frame command acts on: the active layer when it is one,
/// else the topmost frame layer in the stack (what `FrameDivide` has always
/// done — the new frame commands resolve it the same way so they never
/// disagree about which folder the artist means).
fn frame_target(app: &App) -> Option<usize> {
    if app.doc.active_layer().is_frame() {
        Some(app.doc.active)
    } else {
        app.doc.layers.iter().rposition(|l| l.is_frame())
    }
}

fn slot_for(frames: &[mn_core::Frame], cut: Option<[f32; 4]>) -> Option<[f32; 4]> {
    let c = cut?;
    const TOL: f32 = 2.0;
    let all_inside = frames.iter().all(|f| {
        let b = f.bbox();
        b[0] >= c[0] - TOL && b[1] >= c[1] - TOL && b[2] <= c[2] + TOL && b[3] <= c[3] + TOL
    });
    if all_inside { Some(c) } else { None }
}

/// Axis-aligned bounding box of a selection, [x0, y0, x1, y1]. The
/// COVERAGE decides: `outline` is one island of a multi-island mask and
/// is empty for a sub-half feather, so it cannot aim an operand rect.
fn selection_bbox(sel: &mn_core::Selection) -> Option<[i32; 4]> {
    sel.bounds()
}

/// One canvas resize through the whole app: end any stroke/edit, drop stale
/// view state (selection, transform float), run the core resize, and rebuild
/// every cache that is sized by the canvas.
fn apply_canvas_resize(app: &mut App, w: u32, h: u32, dx: i32, dy: i32) {
    app.end_stroke();
    app.commit_text_edit();
    app.transform_drag = None;
    app.last_selection = None;
    let old = app.doc.size;
    app.doc.resize_to(w, h, dx, dy);
    // Structural: the texture changes size and every cached thumb is stale.
    app.renderer.invalidate();
    app.layer_thumbs.clear();
    app.mark_pages_dirty();
    app.mark_dirty();
    app.set_status(format!(
        "canvas {w}×{h} (was {}×{}) — history cleared",
        old.0, old.1
    ));
}

/// Thin a freehand drag down to seed points at least `step` px apart, as
/// integer canvas pixels. The wand-family gestures (SE-020 shrink-select,
/// FI-003 enclose-and-fill) all want this: a pocket only needs ONE seed,
/// and both flood accumulators skip seeds that land in a pocket they
/// already hold, so the survivors cost nothing extra.
fn subsample_path(pts: &[(f32, f32)], step: f32) -> Vec<(i32, i32)> {
    let mut seeds: Vec<(i32, i32)> = Vec::new();
    let mut last: Option<(f32, f32)> = None;
    for &(x, y) in pts {
        if let Some((lx, ly)) = last
            && (x - lx).hypot(y - ly) < step
        {
            continue;
        }
        last = Some((x, y));
        seeds.push((x as i32, y as i32));
    }
    seeds
}

/// Left-strip tools, CSP order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Pen,
    Eraser,
    /// CSP 図形 Figure: inked shapes (line/rect/ellipse/polygon) through the
    /// active brush.
    Figure,
    /// CSP グラデ Gradient: drag a ramp between two colours.
    Gradient,
    Fill,
    Select,
    /// CSP 選択ペン Selection pen: paint selection coverage with the
    /// active brush — release ADDS to the selection (SE-022 combine).
    SelPen,
    /// CSP 選択消しゴム Selection eraser: same stroke, release SUBTRACTS.
    SelEraser,
    /// CSP Auto select (magic wand): click floods a region into a selection.
    Wand,
    /// CSP Operation ▸ Object: select/move/reshape frames and balloons.
    Object,
    /// Frame borders: divide folders/panels, drag out new rectangle frames.
    Frame,
    /// Speech balloons: drag out a body / draw one / attach a tail.
    Balloon,
    /// Text: click to place/edit a text box (T).
    Text,
    /// Eyedropper: click picks the colour under the cursor (I).
    Eyedrop,
    Pan,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Pen => "Pen",
            Tool::Eraser => "Eraser",
            Tool::Figure => "Figure",
            Tool::Gradient => "Gradient",
            Tool::Fill => "Fill",
            Tool::Select => "Select",
            Tool::SelPen => "Select pen",
            Tool::SelEraser => "Select eraser",
            Tool::Wand => "Auto select",
            Tool::Object => "Object",
            Tool::Frame => "Frame border",
            Tool::Balloon => "Balloon",
            Tool::Text => "Text",
            Tool::Eyedrop => "Eyedropper",
            Tool::Pan => "Move view",
        }
    }

    pub fn enabled(self) -> bool {
        true
    }

    /// Tools that paint strokes through the brush engine.
    pub fn strokes(self) -> bool {
        matches!(
            self,
            Tool::Pen | Tool::Eraser | Tool::SelPen | Tool::SelEraser
        )
    }
}

/// Figure-tool sub tools (CSP: Straight line / Rectangle / Ellipse /
/// Polygon — curve/continuous-curve deferred with the vector system).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FigureMode {
    Line,
    Rect,
    Ellipse,
    /// Click vertices; the first vertex / Enter closes, Esc cancels.
    Polygon,
}

impl FigureMode {
    pub fn label(self) -> &'static str {
        match self {
            FigureMode::Line => "Straight line",
            FigureMode::Rect => "Rectangle",
            FigureMode::Ellipse => "Ellipse",
            FigureMode::Polygon => "Polygon",
        }
    }
}

/// Gradient-tool colour modes (CSP's three).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GradMode {
    /// Main colour fades into the sub colour.
    FgToBg,
    /// Main colour fades out to transparent.
    FgToTransparent,
    /// Transparent fades into the main colour.
    TransparentToFg,
}

impl GradMode {
    pub fn label(self) -> &'static str {
        match self {
            GradMode::FgToBg => "Main → Sub",
            GradMode::FgToTransparent => "Main → Transparent",
            GradMode::TransparentToFg => "Transparent → Main",
        }
    }
}

/// What a pending ruler drag creates (TODO #3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RulerKind {
    Line,
    VanishingPoint,
    /// Part 2: a click-vertex polyline (double-click or Enter closes).
    Curve,
    /// Part 3 (RL-014): drag the direction — every stroke comes out
    /// parallel to it.
    Parallel,
    /// Part 3 (RL-019): drag from the centre — the drag length is the ring
    /// spacing.
    Concentric,
    /// Part 3 (RL-021): drag from the centre outward — centre at the
    /// press, first axis along the drag. Line count via the menu ladder.
    Symmetric,
    /// Part 4 (RL-060/061, P-001..010): drag the EYE LEVEL — the two
    /// ends become the horizon VPs of a 2-point set; strokes bind by
    /// direction to rays through either VP or the verticals.
    Perspective,
    /// One-point: drag FROM the vanishing point along the eye level — the
    /// press is the VP, the release the horizon handle (which tilts the
    /// horizontals and verticals when dragged later).
    Perspective1,
    /// Three-point: the same eye-level drag as the 2-point set, plus a
    /// third (vertical) VP placed off the horizon on the side the drag
    /// points to — left→right puts it below (high angle), right→left above
    /// (low angle). It is anchor 2, so the Object tool drags it home.
    Perspective3,
    /// Part 3 (RL-020): a click places a full-canvas horizontal/vertical
    /// guide at the clicked coordinate.
    GuideH,
    GuideV,
}

/// Selection sub-modes (CSP: Rectangle / Lasso sub tools).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SelectMode {
    Rect,
    Lasso,
    /// L-001/L-002 magnetic lasso: the outline snaps to the nearest strong
    /// edge as you trace, so a character is selected by roughly following
    /// its lineart instead of precisely. Click places an anchor, drag
    /// traces and anchors as it goes, Backspace takes the last anchor back,
    /// Enter (or a click on the first anchor) closes. See
    /// [`mn_core::magnetic`].
    Magnetic,
    /// SE-020 (CSP 選択範囲シュリンク): freehand drag through the empty
    // space — every closed area the path crosses floods to its barriers
    // (the flats grabber; see fill::magic_select_path).
    Shrink,
}

/// Fill-tool sub tools. CSP ships five; the difference between them is how
/// you AIM the same flood. Ours: the three 参照 variants are one Click mode
/// plus [`mn_core::FillRefer`], and the two path-aimed ones are here.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FillMode {
    /// The classic bucket — click an area, `FillOpts::refer` decides what
    /// counts as a wall (CSP's "Refer other layers" / "editing layer" /
    /// "reference layer" sub tools).
    #[default]
    Click,
    /// FI-003 Enclose and fill (CSP 囲って塗る): lasso roughly AROUND a
    /// messy region and every closed area inside it fills at once — the
    /// flatting workhorse. See `fill::enclose_and_fill`.
    Enclose,
    /// FI-004 Lasso fill (CSP 塗りつぶし・投げなわ): fill the lassoed shape
    /// ITSELF, boundaries ignored — colour blocking and shadow shapes.
    Lasso,
}

impl FillMode {
    pub fn label(self) -> &'static str {
        match self {
            FillMode::Click => "Fill",
            FillMode::Enclose => "Enclose and fill",
            FillMode::Lasso => "Lasso fill",
        }
    }
}

/// Frame-tool sub tools (CSP: Create frame / Cut frame border groups).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameMode {
    /// CSP "Rectangle frame": drag out a new rectangle frame border folder.
    Rect,
    /// CSP "Polyline frame": click vertices, close on the first one / Enter.
    Polyline,
    /// CSP "Frame border pen": freehand-draw the panel outline.
    Pen,
    /// CSP "Divide frame folder": the cut panel splits off into a NEW frame
    /// border folder with its own White + draw layer.
    DivideFolder,
    /// CSP "Divide frame border": the cut stays inside the same folder.
    DivideBorder,
}

impl FrameMode {
    pub fn creates(self) -> bool {
        matches!(self, FrameMode::Rect | FrameMode::Polyline | FrameMode::Pen)
    }
}

/// `FB-026`/`FB-022` (TRIAGE 128) — what happens to a panel's CONTENTS when
/// the panel is cut. CSP makes you say which, and so do we: cutting a panel
/// that already has art in it has three defensible answers and no default
/// that is right twice in a row.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DivideContents {
    /// CSP "Create empty folder": the new half gets a fresh White + empty
    /// draw layer. Our behaviour before this option existed, and the right
    /// one while you are still laying out the page — which is most of the
    /// time, so it stays the default.
    #[default]
    CreateEmpty,
    /// CSP "Duplicate layer": the new half gets a COPY of the folder's
    /// contents, so the drawing survives in both halves, each masked to its
    /// own shape. The answer when you cut a panel you have already drawn.
    Duplicate,
    /// CSP "Do not change": draw the border only — no new folder, the art
    /// and the layer structure are untouched. Identical in effect to the
    /// Divide-frame-border sub tool, reachable without switching sub tools.
    DoNotChange,
}

impl DivideContents {
    pub const ALL: [DivideContents; 3] = [
        DivideContents::CreateEmpty,
        DivideContents::Duplicate,
        DivideContents::DoNotChange,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DivideContents::CreateEmpty => "Create empty folder",
            DivideContents::Duplicate => "Duplicate layer",
            DivideContents::DoNotChange => "Do not change",
        }
    }
}

/// Move-tool sub tools (CSP Move: Hand / Rotate).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanMode {
    Hand,
    /// Drag rotates the view around the canvas-area centre.
    Rotate,
}

/// Eyedropper Tool Property (CSP: Reference / Average color / Show color
/// picker circle). Lives on `App` beside `fill_opts`/`wand_opts` rather than
/// riding `AppCmd::PickColor`, because it is sub-tool memory: the Alt+click
/// gesture from a brush must obey the same settings the tool does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EyedropOpts {
    /// Which layers the sample walks. Deliberately the FILL tool's 参照
    /// vocabulary (`mn_core::FillRefer`) — composite / editing layer /
    /// reference set — so the two tools cannot mean different things by
    /// "refer reference layer".
    pub refer: mn_core::FillRefer,
    /// Side of the averaging box, in canvas pixels (CSP's 1×1 / 2×2 / 3×3 /
    /// 5×5). 1 is the default and is the single-pixel pick, byte for byte.
    pub size: u32,
    /// Paint the picker ring under the pen while the Eyedropper tool is up.
    pub circle: bool,
}

impl Default for EyedropOpts {
    fn default() -> Self {
        Self {
            refer: mn_core::FillRefer::All,
            size: 1,
            circle: true,
        }
    }
}

/// Operation-tool sub tools (CSP 操作: Object / Select layer).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ObjectMode {
    /// Select and reshape frames, balloons and text boxes.
    #[default]
    Object,
    /// S-001 CSP レイヤー選択: click a pixel and the Layers palette jumps to
    /// whichever layer drew it. On a 200-layer page that is how you find
    /// things.
    PickLayer,
}

impl ObjectMode {
    pub fn label(self) -> &'static str {
        match self {
            ObjectMode::Object => "Object",
            ObjectMode::PickLayer => "Select layer",
        }
    }
}

/// S-001's exclusions (CSP's 選択しないレイヤー checkbox block): the kinds of
/// layer the pick must never land on. Defaults are CSP's — the four the
/// owner would otherwise keep clicking past on a finished page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PickExclude {
    /// Rough underdrawing (CSP 下書き).
    pub draft: bool,
    pub text: bool,
    pub locked: bool,
    /// LIVE fill/gradient/tone layers — the flats, which cover everything.
    pub fill: bool,
}

impl Default for PickExclude {
    fn default() -> Self {
        Self {
            draft: true,
            text: true,
            locked: true,
            fill: false,
        }
    }
}

/// Balloon-tool sub-modes (CSP: Ellipse / Rounded / Balloon pen / Balloon tail).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BalloonMode {
    Ellipse,
    Round,
    /// Freehand-drawn body, simplified to an editable polygon on release.
    Draw,
    /// Drag from inside a balloon out to the tail's tip.
    Tail,
}

impl BalloonMode {
    pub fn label(self) -> &'static str {
        match self {
            BalloonMode::Ellipse => "Ellipse balloon",
            BalloonMode::Round => "Rounded balloon",
            BalloonMode::Draw => "Balloon pen",
            BalloonMode::Tail => "Balloon tail",
        }
    }
}

/// CSP's three drawing-colour slots. `Transparent` is a *colour*, not a tool:
/// drawing with it erases using the current brush's own dab shape/texture
/// (spec section B finding #1 — the single highest-value CSP behaviour).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Main,
    Sub,
    Transparent,
}

/// Per-sub-tool settings, CSP's Tool Property memory model: "settings are saved
/// for the next time you use the sub tool". Keyed by preset path in
/// `App::props`; the active copy lives in `App::props_current`.
#[derive(Clone, Copy, Debug)]
pub struct ToolProps {
    /// The dab DIAMETER in canvas px — ABSOLUTE, the number artists think in
    /// and the one the `[`/`]` ladder steps through. Seeded from the preset's
    /// own size on a first encounter (`App::load_props_for`), which makes that
    /// size a DEFAULT and not a ceiling.
    ///
    /// Renamed from `size`, which was a 0.25..4 multiplier: a stored `2.0`
    /// means something completely different now, and a same-named field would
    /// have read the old meaning as 2 px without a word.
    pub size_px: f32,
    /// Absolute brush opacity 0..1 (libmypaint `opaque`).
    pub opacity: f32,
    /// Floor of the pressure→size curve, % of base radius. The owner's CSP
    /// G-Pen runs at 3%; MyPaint's `pen.myb` floor is much higher, which is
    /// exactly the "minimum is a bit high" complaint.
    pub min_size: f32,
    /// Pull-string stabilizer strength 0..1 (0 = off; the owner inks with 0).
    pub stabilizer: f32,
    /// The rest of CSP's Correction group — post correction, its speed/scale
    /// modulation, the sharp-angle exception, the stabilization mode and the
    /// entry/exit shaping. Defaults to all-off, and all-off is a byte-exact
    /// passthrough, so a sub tool from before this existed draws unchanged.
    pub correct: mn_core::stabilize::CorrectCfg,
    /// Brush-size randomization (CSP 乱数): deviation at full pressure.
    /// Unit depends on `random_abs`: log-radius 0..1 (scales with size) or
    /// canvas px (size-independent, the vendored hook).
    pub random: f32,
    /// Floor of the pressure→deviation curve, % of `random`.
    pub random_min: f32,
    /// Deviation is a fixed pixel amount instead of scaling with brush size.
    pub random_abs: bool,
    /// Entry taper: ramp length in px (0 = off) and starting pressure factor.
    /// Seeded from the preset's CSP metadata (Real G-Pen: 217 / 18.3%).
    pub taper_px: f32,
    pub taper_min: f32,
    /// Krita-style hard stamp dabs: exact AA discs instead of the gaussian
    /// hardness falloff (vendor/PATCHES.md). Off = stock behaviour.
    pub hard_dab: bool,
    /// Krita Scatter: each dab's centre jitters within `radius * this`.
    pub scatter: f32,
    /// Krita Wash: the stroke composites once at `opacity` instead of per
    /// dab (flow-vs-opacity semantics).
    pub wash: bool,
    /// Per-dab alpha inside a wash stroke (Krita: Flow). Only meaningful
    /// with `wash`; in build-up the single `opacity` slider is per-dab.
    pub flow: f32,
    /// Compositing mode of the wash commit (Krita: per-brush blending).
    pub brush_blend: Blend,
    /// Texture-tip mask: 0 = none, else 1.. into `App::texture_names`.
    pub texture: u16,
    /// Texture crawl per dab in mask px (0 = static pattern).
    pub texture_scroll: f32,
    /// Krita SKETCH engine: link strokes back to their recent history.
    pub sketch: bool,
    /// Max link distance in canvas px.
    pub sketch_dist: f32,
    /// Link attempts per sample, 0..1.
    pub sketch_density: f32,
    /// TL-013: this sub tool's settings are pinned. CSP's meaning, which is
    /// the useful one — a locked tool still ACCEPTS every change, it just
    /// never writes them down (`App::store_current_props`), so selecting it
    /// again restores the snapshot. Drop the size for one panel and your
    /// calibrated pen comes home by itself; refusing the edit outright
    /// would have made the lock something you have to keep switching off.
    ///
    /// Lives HERE, not on `App`, so the lock follows the sub tool the way
    /// every other Tool Property value does — locking the inking pen leaves
    /// the eraser free. Session-only, like `props` itself.
    pub locked: bool,
    /// CSP Stroke ▸ Interval (S-028): the gap between dabs. `AsPreset` — the
    /// default — leaves the preset's own spacing alone.
    pub interval: Interval,
    /// The gap the Fixed mode remembers while another mode is selected, so
    /// leaving Fixed and coming back does not lose the number.
    pub interval_px: f32,
    /// CSP Adjust brush density by gap (B-029). `None` = untouched, i.e. as
    /// the preset ships it — a tri-state for the same reason `Interval` has
    /// `AsPreset`: a plain `bool` default would have to guess, and guessing
    /// wrong silently re-inks every preset that disagrees with the guess.
    pub density_by_gap: Option<bool>,
    /// CSP Anti-aliasing (A-010): the four-level edge feather.
    pub anti_alias: AntiAlias,
}

impl Default for ToolProps {
    fn default() -> Self {
        Self {
            // Placeholder only: every real sub tool overwrites this from the
            // preset's own `base_size_px()` before a stroke can happen
            // (`App::new` seeds it, `load_props_for` seeds each later one).
            size_px: DEFAULT_SIZE_PX,
            opacity: 1.0,
            min_size: 0.0,
            stabilizer: 0.0,
            correct: mn_core::stabilize::CorrectCfg::default(),
            random: 0.0,
            random_min: 0.0,
            random_abs: false,
            taper_px: 0.0,
            taper_min: 0.18,
            hard_dab: false,
            scatter: 0.0,
            wash: false,
            flow: 1.0,
            brush_blend: Blend::Normal,
            texture: 0,
            texture_scroll: 0.0,
            sketch: false,
            sketch_dist: 40.0,
            // M1 re-tune: 0.6 restores the link rate the 0.3 default had
            // while rng01 only covered 0..0.5 (audit 2026-08-17).
            sketch_density: 0.6,
            locked: false,
            interval: Interval::AsPreset,
            interval_px: DEFAULT_INTERVAL_PX,
            density_by_gap: None,
            anti_alias: AntiAlias::AsPreset,
        }
    }
}

/// Gap the Fixed interval mode opens at. Only a starting point for a slider
/// the user then drags — it is not applied until they pick Fixed.
pub const DEFAULT_INTERVAL_PX: f32 = 2.0;

/// The settings the per-sensor curve editor exposes (Krita's dynamics
/// pickers, trimmed to the four a manga inker actually reaches for).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CurveSetting {
    #[default]
    Size,
    Opacity,
    Hardness,
    Smudge,
}

impl CurveSetting {
    pub const ALL: [CurveSetting; 4] = [
        CurveSetting::Size,
        CurveSetting::Opacity,
        CurveSetting::Hardness,
        CurveSetting::Smudge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CurveSetting::Size => "Size",
            CurveSetting::Opacity => "Opacity",
            CurveSetting::Hardness => "Hardness",
            CurveSetting::Smudge => "Smudge",
        }
    }

    pub fn from_index(i: u8) -> CurveSetting {
        Self::ALL[i as usize % Self::ALL.len() as usize]
    }

    /// libmypaint setting id. `None` would mean our vendored libmypaint
    /// lost a setting — not a real state, but the editor degrades calmly.
    pub fn setting_id(self) -> Option<i32> {
        let name = match self {
            CurveSetting::Size => "radius_logarithmic",
            CurveSetting::Opacity => "opaque_multiply",
            CurveSetting::Hardness => "hardness",
            CurveSetting::Smudge => "smudge",
        };
        mn_brush::settings::setting_id(name)
    }

    /// The editor's y axis range in RAW setting units. Size is logarithmic
    /// (ln of the radius factor: the owner's Real G-Pen curve bottoms near
    /// -3.5); the 0..1 settings get drag room past their ends.
    pub fn y_range(self) -> (f32, f32) {
        match self {
            CurveSetting::Size => (-4.0, 1.0),
            _ => (-0.25, 1.25),
        }
    }
}

/// The sensors a curve can respond to (Krita's per-sensor curves; libmypaint
/// already maps by all of these — the gap was the editor).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CurveSensor {
    #[default]
    Pressure,
    Speed,
    Direction,
    TiltX,
    TiltY,
    Random,
}

impl CurveSensor {
    pub const ALL: [CurveSensor; 6] = [
        CurveSensor::Pressure,
        CurveSensor::Speed,
        CurveSensor::Direction,
        CurveSensor::TiltX,
        CurveSensor::TiltY,
        CurveSensor::Random,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CurveSensor::Pressure => "Pressure",
            CurveSensor::Speed => "Speed",
            CurveSensor::Direction => "Direction",
            CurveSensor::TiltX => "Tilt X",
            CurveSensor::TiltY => "Tilt Y",
            CurveSensor::Random => "Random",
        }
    }

    pub fn from_index(i: u8) -> CurveSensor {
        Self::ALL[i as usize % Self::ALL.len() as usize]
    }

    pub fn input_id(self) -> Option<i32> {
        let name = match self {
            CurveSensor::Pressure => "pressure",
            CurveSensor::Speed => "speed1",
            CurveSensor::Direction => "direction",
            CurveSensor::TiltX => "declinationx",
            CurveSensor::TiltY => "declinationy",
            CurveSensor::Random => "random",
        };
        mn_brush::settings::input_id(name)
    }

    /// The editor's x axis range in raw input units (tilt is -1..1 after the
    /// MyBrush normalisation; speed is open-ended, clamped for display).
    pub fn x_range(self) -> (f32, f32) {
        match self {
            CurveSensor::Pressure | CurveSensor::Direction | CurveSensor::Random => (0.0, 1.0),
            CurveSensor::Speed => (0.0, 4.0),
            CurveSensor::TiltX | CurveSensor::TiltY => (-1.0, 1.0),
        }
    }
}

#[derive(Clone, Debug)]
pub enum AppCmd {
    Undo,
    Redo,
    /// Open the New Comic dialog (an egui window, not a native dialog).
    NewDoc,
    /// One-gesture tiling-pattern authoring: a square wrap-on canvas in a
    /// new tab + the Pattern Studio window (`app/pattern.rs`).
    NewPattern,
    /// Save the pattern canvas into the material bank under the studio's
    /// name.
    PatternSaveMaterial,
    /// Create from `App::new_doc_draft`.
    NewComicCreate,
    // --- pages --------------------------------------------------------------
    SelectPage(usize),
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
    /// LC-005: overwrite the selected comp with the current visibility.
    /// Overwrite comp `i` (the row whose 💾 was clicked) with the current
    /// visibility.
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
    /// CV-004: drop the whole undo history (frees memory, irreversible).
    ClearHistory,
    /// CV-005: reload the last-saved state of the current file.
    RevertFile,
    /// MT-020 raster half: register the active layer (selection-scoped) as
    /// an image material.
    MaterialRegisterLayer,
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
    /// Apply the canvas-size draft: new size + the anchor the content pins to.
    ResizeCanvasApply,
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
    /// Open the Preferences window (Edit ▸ Preferences…).
    OpenPrefs,
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
    // --- path-resolved forms, issued by `main::pump_commands` --------------
    OpenOraPath(PathBuf),
    SaveOraPath(PathBuf),
    ExportPngPath(PathBuf),
    /// Import an image file as a new layer (asks for a path first).
    ImportImage,
    ImportImagePath(PathBuf),
    // --- layers -----------------------------------------------------------
    AddLayer,
    /// Vector inking (docs/VECTOR-INKING.md): a raster layer that RECORDS
    /// its strokes as editable geometry beside the pixels.
    AddVectorLayer,
    /// Batch layer operations (app/batch.rs): open, apply, export.
    BatchOpsOpen,
    BatchApply,
    BatchExportPngs,
    BatchExportPngsPath(PathBuf),
    /// Delete the Object tool's selected recorded stroke (Del).
    VectorDelete { stroke: usize },
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
    /// Set the ACTIVE layer's screentone params — `Some` converts it into a
    /// tone layer, `None` converts it back. Non-destructive either way (the
    /// painted pixels are the ink source and survive).
    SetTone(Option<mn_core::ToneParams>),
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
    /// Part 3 (RL-031): the special-ruler snap veto (parallel/concentric/
    /// guide/symmetric). The master `RulerSnapToggle` still gates all.
    RulerSpecialSnapToggle,
    /// Part 3 (RL-021): cycle the symmetric ruler's line count through the
    /// CSP ladder — applies to existing symmetric rulers AND the default
    /// for the next one created.
    RulerSymmetricCount,
    // --- brush + colour ----------------------------------------------------
    SelectBrush(PathBuf),
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
    /// Texture-tip mask by `texture_names` index (0 = none).
    SetTexture(u16),
    /// Texture crawl per dab, mask px.
    SetTextureScroll(f32),
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
    // --- text ---------------------------------------------------------------
    /// Commit a text layer's items (Object-tool move/resize/rotate, or an
    /// editing session's single undo step).
    TextCommit {
        layer: usize,
        texts: mn_core::TextSet,
    },
    /// Delete one text item (Object tool + Del).
    TextDelete {
        layer: usize,
        text: usize,
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
    SetFillParams(usize, mn_core::FillKind),
    /// Pick the Fill tool's sub tool (click / enclose / lasso).
    SetFillMode(FillMode),
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

/// PM-051: the batch export's default file prefix — the work name, or
/// `page` for an unnamed work. This is the string the pre-options export
/// used, and keeping it here is what makes an untouched run byte-for-byte
/// identical to the old one.
pub fn default_export_stem(app: &App) -> String {
    if app.story.trim().is_empty() {
        "page".to_owned()
    } else {
        app.story.trim().to_owned()
    }
}

/// PM-055: is this page a two-page spread? The runtime `spread` flag when
/// it is still there, else the structural test — a canvas half again as
/// wide as a normal page. The flag is a session flag on the page entry
/// and does NOT survive a reload, so the width test is what keeps a
/// reopened work splitting correctly.
fn is_spread_page(d: &mn_core::Document, flagged: bool, normal_w: Option<u32>) -> bool {
    flagged || normal_w.is_some_and(|w| w > 0 && d.size.0 as f32 >= w as f32 * 1.5)
}

/// The status line after either view flip. Both flags read together: with
/// the vertical flip in, "view back to normal" is only true when NEITHER
/// axis is flipped, and H+V is a half turn rather than a mirror.
fn flip_status(vp: &mn_gpu::Viewport) -> &'static str {
    match (vp.flip_h, vp.flip_v) {
        (true, true) => "view turned 180° — mirrored both ways is a half turn",
        (true, false) => "view mirrored — the classic drawing-error check",
        (false, true) => "view flipped vertically — the same check, upside down",
        (false, false) => "view back to normal",
    }
}

/// Every layer mask's identity, for the undo/redo invalidation compare.
/// `Document::apply_group` stamps a fresh revision on any mask it restores,
/// so a moved coverage field always shows up here (and a mask that appeared
/// or vanished changes the shape of the vector).
fn mask_sig(app: &App) -> Vec<Option<(u64, bool)>> {
    app.doc
        .layers
        .iter()
        .map(|l| l.mask.as_ref().map(|m| (m.revision, m.enabled)))
        .collect()
}

/// Undo/redo can restore the whole ruler set (`UndoGroup::Rulers`). The
/// rulers ARE the geometry — the overlay and the snap read them straight,
/// so nothing is cached off them — with one exception: the symmetric
/// ruler's mirror twins hold its centre and axes, and they must be rebuilt
/// or the next stroke mirrors about the place the ruler used to be. The
/// in-flight gesture state goes too: a sticky snap lock and a live grab are
/// both INDICES into the set that was just replaced (the session.rs
/// pattern, where a tab switch drops them for the same reason).
fn resync_rulers(app: &mut App, before: &mn_core::Rulers) {
    if app.doc.rulers == *before {
        return;
    }
    app.ruler_lock = Default::default();
    app.ruler_move = None;
    app.rebuild_twins();
    app.mark_dirty();
}

pub fn dispatch(app: &mut App, cmd: AppCmd) {
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
    match cmd {
        // --- history ------------------------------------------------------
        // No `renderer.invalidate()`: undo stamps a fresh revision on every
        // tile it restores, and the tile cache evicts on revision.
        AppCmd::Undo => {
            app.commit_text_edit();
            // A tone-param undo can flip a layer back to non-tone: the GPU
            // tile cache then holds derived rasters newer than the source
            // tiles, which the revision compare would keep. Evict on change.
            let tones_before: Vec<_> = app.doc.layers.iter().map(|l| l.tone).collect();
            let masks_before = mask_sig(app);
            let rulers_before = app.doc.rulers.clone();
            if app.doc.undo() {
                resync_rulers(app, &rulers_before);
                // Vector selection indexes into a set undo just reshaped.
                app.vector_sel = None;
                app.vector_drag = None;
                for (li, (l, was)) in app.doc.layers.iter().zip(&tones_before).enumerate() {
                    if l.tone != *was {
                        app.renderer.evict_layer(li);
                    }
                }
                // LM-004: the GPU tile cache keys on the LAYER tile revision
                // and folds the mask into the upload, so a mask that moved
                // over unchanged pixels needs the full rebuild — the same
                // door every other mask edit goes through.
                if mask_sig(app) != masks_before {
                    app.renderer.invalidate();
                }
                // Undo can remove the active layer's mask (e.g. undo of its
                // creation) — audit H1: armed mask-edit must not survive it.
                app.disarm_mask_edit_if_unmasked();
                app.mark_dirty();
            }
        }
        AppCmd::Redo => {
            app.commit_text_edit();
            let tones_before: Vec<_> = app.doc.layers.iter().map(|l| l.tone).collect();
            let masks_before = mask_sig(app);
            let rulers_before = app.doc.rulers.clone();
            if app.doc.redo() {
                resync_rulers(app, &rulers_before);
                app.vector_sel = None;
                app.vector_drag = None;
                for (li, (l, was)) in app.doc.layers.iter().zip(&tones_before).enumerate() {
                    if l.tone != *was {
                        app.renderer.evict_layer(li);
                    }
                }
                if mask_sig(app) != masks_before {
                    app.renderer.invalidate();
                }
                app.disarm_mask_edit_if_unmasked();
                app.mark_dirty();
            }
        }

        AppCmd::MaskSelection => {
            let li = app.doc.active;
            if app.doc.mask_selection_blank(li) {
                app.renderer.invalidate();
                app.set_status("mask created — all visible (LM-001 starter)");
                app.mark_dirty();
            } else {
                app.set_status("mask applies to raster layers with content");
            }
        }
        AppCmd::MaskOutsideSelection => {
            let li = app.doc.active;
            if app.doc.selection.is_none() {
                app.set_status("no selection — the whole layer would be hidden; refusing");
                return;
            }
            if app.doc.mask_outside_selection(li) {
                app.renderer.invalidate();
                app.set_status("mask outside selection — the rest is hidden");
                app.mark_dirty();
            } else {
                app.set_status("mask applies to raster layers with content");
            }
        }
        AppCmd::MaskLinkToggle => {
            let li = app.doc.active;
            let flipped = matches!(app.doc.layers.get(li), Some(l) if l.mask.is_some()) && {
                app.doc.layers[li].mask_linked = !app.doc.layers[li].mask_linked;
                true
            };
            if flipped {
                // Persisted state (`mnc-mask-unlinked`): the touch is what
                // gets it saved when the toggle is the session's last act.
                app.doc.touch();
                app.set_status(if app.doc.layers[li].mask_linked {
                    "mask linked — moves with the layer"
                } else {
                    "mask unlinked — art slides underneath a fixed mask"
                });
                app.mark_dirty();
            } else {
                app.set_status("that layer has no mask to link");
            }
        }
        AppCmd::MaskToggle => {
            let li = app.doc.active;
            let on = app
                .doc
                .layers
                .get(li)
                .and_then(|l| l.mask.as_ref())
                .is_some_and(|m| !m.enabled);
            if app.doc.mask_set_enabled(li, on) {
                app.renderer.invalidate();
                app.set_status(if on { "mask ON" } else { "mask OFF (kept)" });
                app.mark_dirty();
            }
        }
        AppCmd::MaskDelete => {
            let li = app.doc.active;
            if app.doc.mask_delete(li) {
                app.disarm_mask_edit_if_unmasked();
                app.renderer.invalidate();
                app.set_status("mask deleted");
                app.mark_dirty();
            }
        }
        AppCmd::MaskClear => {
            let li = app.doc.active;
            if app.doc.mask_clear(li) {
                app.renderer.invalidate();
                app.set_status("mask cleared — all hidden (the mask itself kept)");
                app.mark_dirty();
            }
        }
        AppCmd::MaskApply => {
            let li = app.doc.active;
            app.doc.set_op_label("Apply mask");
            if app.doc.mask_apply_bake(li) {
                // The bake ends by deleting the mask (audit H1: disarm).
                app.disarm_mask_edit_if_unmasked();
                app.renderer.invalidate();
                app.set_status("mask baked into the layer — the mask is gone");
                app.mark_dirty();
            } else {
                app.set_status("no enabled mask to apply");
            }
        }
        AppCmd::MaskEdit => {
            let ok = app.doc.active_layer().mask.is_some() && app.doc.active_layer().paintable();
            if !app.mask_edit && !ok {
                app.set_status("edit-mask needs a masked raster layer");
                return;
            }
            app.set_mask_edit(!app.mask_edit);
            app.set_status(if app.mask_edit {
                "editing the MASK — draw any colour to reveal, erase to hide"
            } else {
                "editing the LAYER again"
            });
            app.mark_dirty();
        }
        AppCmd::MaskShowArea => {
            app.mask_show_area = !app.mask_show_area;
            app.set_status(if app.mask_show_area {
                "mask area shown (purple tint over the hidden region)"
            } else {
                "mask area hidden"
            });
            app.mark_dirty();
        }
        AppCmd::FilterOpen(f) => {
            app.filter_draft = f;
        }
        AppCmd::FilterApply(f) => {
            app.filter_draft = None;
            if app.doc.apply_filter(f) {
                app.set_status(format!("{} applied", f.label()));
                app.mark_dirty();
            } else {
                // Every refusal reason at once: the layer will not take
                // pixels, or there are none, or the marquee misses it, or the
                // parameters are a no-op. Nothing was pushed onto undo.
                app.set_status(format!(
                    "{} did nothing — needs an unlocked raster layer with pixels inside the selection",
                    f.label()
                ));
            }
        }
        AppCmd::BrightnessToOpacity => {
            let li = app.doc.active;
            app.doc.set_op_label("Brightness → opacity");
            if app.doc.convert_brightness_to_opacity(li) {
                app.set_status("brightness converted to opacity — white is now transparent");
                app.mark_dirty();
            } else {
                app.set_status("nothing to convert (raster layer with content)");
            }
        }
        AppCmd::AdjustOpen(a) => app.adjust_begin(a),
        AppCmd::AdjustApply => app.adjust_commit(),
        AppCmd::AdjustCancel => {
            app.adjust_preview_revert();
        }
        AppCmd::AdjustNow(a) => {
            app.adjust_draft = Some(a);
            app.adjust_commit();
        }
        AppCmd::CompApply(i) => {
            if app.comp_apply(i) {
                let n = app
                    .doc
                    .comps
                    .get(i)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                app.set_status(format!("layer comp applied: {n}"));
            }
        }
        AppCmd::CompSave(i) => {
            // Named, because this is destructive to a snapshot: say WHICH
            // one was replaced (the old status claimed success even when
            // nothing was selected and nothing happened).
            let name = app.doc.comps.get(i).map(|c| c.name.clone());
            if app.comp_save(i)
                && let Some(n) = name
            {
                app.set_status(format!(
                    "layer comp \"{n}\" overwritten with current visibility"
                ));
            } else {
                app.set_status("no comp at that row to overwrite");
            }
        }
        AppCmd::GenLines => {
            app.gen_open = true;
        }
        AppCmd::GenLinesEdit => {
            let Some(g) = app.doc.active_layer().genlines else {
                app.set_status("the active layer was not generated — Layer menu > Generate effect lines for a new one");
                return;
            };
            app.gen_focus = g.focus;
            app.gen_a = g.a;
            app.gen_b = g.b;
            app.gen_c = g.c;
            app.gen_d = g.d;
            app.gen_count = g.count;
            app.gen_width = g.width;
            app.gen_jitter = g.jitter;
            app.gen_seed = g.seed;
            // The loaded values ARE the layer's parameters — the dialog's
            // first-open seeding must not replace them (a (0,0) centre is
            // legal, not "uninitialized").
            app.gen_inited = true;
            app.gen_open = true;
            app.set_status("editing the layer's own parameters — Apply regenerates in place");
        }
        AppCmd::GenLinesApply {
            focus,
            a,
            b,
            c,
            d,
            count,
            width,
            jitter,
            seed,
        } => {
            app.gen_open = false;
            let spec = mn_core::genlines::GenLinesSpec {
                focus,
                a,
                b,
                c,
                d,
                count,
                width,
                jitter,
                seed,
            };
            // SF-004/005: re-applying on the layer the params came from
            // regenerates IN PLACE (the layer keeps name/stack/blend);
            // everything else generates a fresh layer as before.
            if app.doc.active_layer().genlines.is_some() {
                let li = app.doc.active;
                // Spec-on-success (audit F): the regen stores the new spec
                // only when it rendered something, and a failed one leaves
                // both halves alone — the stored parameters must always
                // describe the pixels on screen.
                if app.doc.regen_genlines(li, spec) {
                    app.set_status("effect lines regenerated");
                    app.mark_dirty();
                } else {
                    app.set_status("generator produced nothing — widen the parameters");
                }
                return;
            }
            let size = app.doc.size;
            let tiles = if focus {
                mn_core::genlines::render_focus(
                    &mn_core::FocusLinesParams {
                        center: [a, b],
                        r_in: c,
                        r_out: d,
                        count: count.max(1),
                        width,
                        angle_jitter: jitter,
                        width_jitter: jitter,
                        length_jitter: jitter,
                        seed,
                    },
                    size,
                )
            } else {
                mn_core::genlines::render_speed(
                    &mn_core::SpeedLinesParams {
                        angle_deg: a,
                        count: count.max(1),
                        len_min: b,
                        len_max: c,
                        width,
                        seed,
                    },
                    size,
                )
            };
            if tiles.is_empty() {
                app.set_status("generator produced nothing — widen the parameters");
                return;
            }
            let name = if focus { "Focus lines" } else { "Speed lines" };
            app.doc.add_layer(name);
            app.doc.layers[app.doc.active].genlines = Some(spec);
            app.doc.begin_op();
            app.doc.set_op_label("Generate lines");
            let active = app.doc.active;
            for (idx, tile) in tiles {
                app.doc.layers[active].set_tile(idx, Some(tile));
            }
            app.doc.end_op();
            app.set_status(format!("{name} generated — {count} lines"));
            app.mark_dirty();
        }
        AppCmd::HistoryTo { keep } => {
            app.commit_text_edit();
            let tones_before: Vec<_> = app.doc.layers.iter().map(|l| l.tone).collect();
            let masks_before = mask_sig(app);
            app.vector_sel = None;
            app.vector_drag = None;
            let mut steps = 0usize;
            while app.doc.undo_len() > keep && app.doc.undo() {
                steps += 1;
            }
            while app.doc.undo_len() < keep && app.doc.redo() {
                steps += 1;
            }
            if steps > 0 {
                for (li, (l, was)) in app.doc.layers.iter().zip(&tones_before).enumerate() {
                    if l.tone != *was {
                        app.renderer.evict_layer(li);
                    }
                }
                // Scrubbing the History palette crosses the same two doors a
                // single Undo does: a restored mask needs the upload rebuild,
                // and a mask that scrubbed away must not leave mask-edit armed.
                if mask_sig(app) != masks_before {
                    app.renderer.invalidate();
                }
                app.disarm_mask_edit_if_unmasked();
                app.mark_dirty();
            }
        }
        AppCmd::ClearHistory => {
            app.doc.clear_history();
            app.set_status("undo history cleared");
        }
        AppCmd::RevertFile => match app.doc_path.clone() {
            Some(p) if p.exists() => {
                app.push_cmd(AppCmd::OpenOraPath(p));
                app.set_status("reverted to the last save");
            }
            _ => app.set_status("nothing saved to revert to"),
        },

        // --- documents ----------------------------------------------------
        AppCmd::NewDoc => {
            app.new_doc_open = true;
            app.mark_dirty();
        }
        AppCmd::NewPattern => app.pattern_new(),
        AppCmd::PatternSaveMaterial => match app.pattern_save_material() {
            Some((path, stem)) => {
                app.set_status(format!(
                    "pattern \"{stem}\" saved to the material bank ({})",
                    path.display()
                ));
            }
            None => {
                app.set_error("pattern save failed: the tile is empty or the folder is unwritable")
            }
        },
        AppCmd::NewComicCreate => {
            app.commit_text_edit();
            // A new project opens in a NEW TAB (owner, 2026-08-19: "it would
            // be bad if I have art in the default canvas and making a new
            // manga deletes it"). This one line parks the current document;
            // everything below then builds the new one into the live fields
            // exactly as it did when there was only ever one.
            app.push_doc_slot();
            let d = app.new_doc_draft.clone();
            let (w, h) = d.setup.paper_px();
            app.page = d.setup.has_guides().then(|| d.setup.clone());
            app.seed_frame_folder = d.frame_folder;
            app.doc = app.blank_page_doc_sized(w, h);
            app.story = d.story;
            app.binding_right = d.binding_right;
            let blank = mn_core::project::doc_to_bytes(&app.doc).ok();
            app.pages = vec![PageEntry::active()];
            for _ in 1..d.pages.max(1) {
                let e = app.fresh_page(blank.clone(), None);
                app.pages.push(e);
            }
            app.page_index = 0;
            app.new_doc_open = false;
            app.set_doc_path(None);
            app.reset_folder_state();
            app.renderer.invalidate();
            app.layer_thumbs.clear();
            app.fit_to_view();
            app.mark_saved();
            app.mark_dirty();
        }

        // --- pages ----------------------------------------------------------
        AppCmd::SelectPage(i) => app.switch_page(i),
        // PM-021/022: navigation with end-of-chapter guards — switching
        // itself goes through switch_page (stash + decode + fit).
        AppCmd::PageFirst => app.switch_page(0),
        AppCmd::PageLast => app.switch_page(app.pages.len().saturating_sub(1)),
        AppCmd::PagePrev => {
            if app.page_index == 0 {
                app.set_status("first page");
            } else {
                app.switch_page(app.page_index - 1);
            }
        }
        AppCmd::PageNext => {
            if app.page_index + 1 >= app.pages.len() {
                app.set_status("last page");
            } else {
                app.switch_page(app.page_index + 1);
            }
        }
        AppCmd::PageGoto => {
            app.goto_page_value = (app.page_index + 1) as i32;
            app.goto_page_open = true;
        }
        AppCmd::PageGotoApply(n) => {
            app.goto_page_open = false;
            let n = n.clamp(1, app.pages.len().max(1));
            app.switch_page(n - 1);
        }
        AppCmd::MaterialRegisterLayer => match app.material_register_layer() {
            Some((p, name)) => {
                app.set_status(format!(
                    "registered \"{name}\" → {}",
                    p.parent()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default()
                ));
                app.mark_dirty();
            }
            None => app.set_status(
                "nothing to register — raster layer with content (a selection scopes it)",
            ),
        },
        AppCmd::MaterialImportFolder(src) => {
            let n = app.material_import_folder(&src);
            app.set_status(if n > 0 {
                format!("imported {n} material(s)")
            } else {
                "no new images found in that folder".into()
            });
        }
        AppCmd::StoryEditor => {
            app.story_open_refresh();
            app.set_status(format!(
                "Story Editor — {} text field(s)",
                app.story_bufs.len()
            ));
        }
        AppCmd::ReaderOpen => app.reader_open(),
        AppCmd::ReaderReturn => app.reader_return(),
        AppCmd::PageCombineSpread => {
            if app.page_index + 1 >= app.pages.len() {
                app.set_status("no next page to combine with");
            } else {
                app.spread_op = Some(crate::app::SpreadOp::Combine);
            }
        }
        AppCmd::PageCombineApply { gap, delete_empty } => {
            app.spread_op = None;
            let i = app.page_index;
            if i + 1 >= app.pages.len() {
                app.set_status("no next page to combine with");
                return;
            }
            let Some(b_bytes) = app.pages[i + 1].bytes.take() else {
                app.set_error("next page has no data");
                return;
            };
            let Ok(b_doc) = mn_core::project::bytes_to_doc(&b_bytes) else {
                app.pages[i + 1].bytes = Some(b_bytes);
                app.set_error("next page failed to decode");
                return;
            };
            let mut doc = mn_core::page::combine_spread(&app.doc, &b_doc, gap);
            if delete_empty {
                mn_core::page::drop_empty_raster_layers(&mut doc);
            }
            let id = app.pages[i].id;
            let entry = app.fresh_spread(None);
            app.pages.drain(i..=i + 1);
            app.pages.insert(i, entry);
            app.pages[i].id = id; // keep A's work-folder file identity
            app.page_index = i;
            // Same tab, same work: the rulers carry (see `adopt_page_doc`).
            app.adopt_page_doc(doc);
            app.pages[i].doc_rev = app.doc.revision;
            app.mark_pages_dirty();
            app.renderer.invalidate();
            app.layer_thumbs.clear();
            app.fit_to_view();
            app.set_status("spread combined — draw across the gutter");
            app.mark_dirty();
        }
        AppCmd::PageSplitSpread => {
            if app.doc.size.0 < 128 {
                app.set_status("page too narrow to split");
            } else {
                app.spread_op = Some(crate::app::SpreadOp::Split);
            }
        }
        AppCmd::PageSplitApply { gap, delete_empty } => {
            app.spread_op = None;
            let Some((mut l, mut r)) = mn_core::page::split_spread(&app.doc, gap) else {
                app.set_status("page too narrow to split");
                return;
            };
            if delete_empty {
                mn_core::page::drop_empty_raster_layers(&mut l);
                mn_core::page::drop_empty_raster_layers(&mut r);
            }
            let Ok(r_bytes) = mn_core::project::doc_to_bytes(&r) else {
                app.set_error("split page failed to encode");
                return;
            };
            let id = app.pages[app.page_index].id;
            let i = app.page_index;
            let mut right_entry = app.fresh_page(Some(r_bytes), None);
            right_entry.id = 0; // new file identity at the next folder save
            let left_entry = app.fresh_page(None, None);
            app.pages.drain(i..=i);
            app.pages.insert(i, left_entry);
            app.pages[i].id = id; // the left half keeps the spread's file
            app.pages.insert(i + 1, right_entry);
            app.page_index = i;
            app.adopt_page_doc(l);
            app.pages[i].doc_rev = app.doc.revision;
            app.mark_pages_dirty();
            app.renderer.invalidate();
            app.layer_thumbs.clear();
            app.fit_to_view();
            app.set_status("spread split into two pages");
            app.mark_dirty();
        }
        AppCmd::AddPage => {
            let blank = mn_core::project::doc_to_bytes(&app.blank_page_doc()).ok();
            let at = app.page_index + 1;
            let e = app.fresh_page(blank, None);
            app.pages.insert(at, e);
            app.mark_pages_dirty();
            app.set_status(format!("page {} added", at + 1));
            app.switch_page(at);
        }
        AppCmd::DeletePage => {
            let n = app.pages.len();
            if n <= 1 {
                app.set_status("a comic keeps at least one page");
            } else {
                let cur = app.page_index;
                let target = if cur + 1 < n { cur + 1 } else { cur - 1 };
                app.switch_page(target);
                if app.page_index == target {
                    app.pages.remove(cur);
                    if app.page_index > cur {
                        app.page_index -= 1;
                    }
                    app.mark_pages_dirty();
                    app.set_status(format!("deleted page {}", cur + 1));
                    app.mark_dirty();
                }
            }
        }
        AppCmd::MovePage { from, to } => {
            let n = app.pages.len();
            if from < n && to < n && from != to {
                let e = app.pages.remove(from);
                app.pages.insert(to, e);
                let a = app.page_index;
                app.page_index = if a == from {
                    to
                } else if from < a && a <= to {
                    a - 1
                } else if to <= a && a < from {
                    a + 1
                } else {
                    a
                };
                app.mark_pages_dirty();
                app.mark_dirty();
            }
        }
        AppCmd::DuplicatePage => {
            // Serialize the live page so the copy is byte-exact.
            match app.stash_current_page() {
                Err(e) => app.set_error(e),
                Ok(()) => {
                    let cur = app.page_index;
                    let bytes = app.pages[cur].bytes.clone();
                    let thumb = app.pages[cur].thumb.clone();
                    let e = app.fresh_page(bytes, thumb);
                    app.pages.insert(cur + 1, e);
                    // Restore the active-page invariant (bytes live in `doc`).
                    app.pages[cur].bytes = None;
                    app.mark_pages_dirty();
                    app.set_status(format!("page {} duplicated", cur + 1));
                    app.mark_dirty();
                }
            }
        }
        AppCmd::ImportPage | AppCmd::ReplacePage => {
            // Resolved to their *Path forms by `main::pump_commands`.
        }
        AppCmd::ImportAbr => {
            // Resolved to ImportAbrPath by `main::pump_commands`.
        }
        AppCmd::ImportAbrPath(p) => app.import_abr(&p),
        AppCmd::ImportPagePath(p) => match app.file_to_page_bytes(&p) {
            Err(e) => app.set_error(format!("import failed: {e}")),
            Ok(bytes) => {
                let at = app.page_index + 1;
                let e = app.fresh_page(Some(bytes), None);
                app.pages.insert(at, e);
                app.mark_pages_dirty();
                app.set_status(format!("imported {} as page {}", p.display(), at + 1));
                app.switch_page(at);
            }
        },
        AppCmd::ReplacePagePath(p) => match app.file_to_page_bytes(&p) {
            Err(e) => app.set_error(format!("replace failed: {e}")),
            Ok(bytes) => match mn_core::project::bytes_to_doc(&bytes) {
                Err(e) => app.set_error(format!("replace decode failed: {e}")),
                Ok(doc) => {
                    app.commit_text_edit();
                    app.adopt_page_doc(doc);
                    let i = app.page_index;
                    // The page's content was swapped wholesale: give it a
                    // fresh revision so a folder save rewrites its file even
                    // though the decoded doc may carry a coincidental
                    // matching revision.
                    app.pages[i].rev = app.page_rev_next();
                    app.pages[i].doc_rev = app.doc.revision;
                    app.pages[i].bytes = None;
                    app.pages[i].thumb = None;
                    app.renderer.invalidate();
                    app.layer_thumbs.clear();
                    app.fit_to_view();
                    app.mark_pages_dirty();
                    app.set_status(format!(
                        "page {} replaced with {}",
                        app.page_index + 1,
                        p.display()
                    ));
                    app.mark_dirty();
                }
            },
        },
        AppCmd::WorkSettings => {
            app.work_settings_draft = crate::app::WorkSettingsDraft {
                setup: app
                    .page
                    .clone()
                    .unwrap_or_else(|| mn_core::PageSetup::presets().remove(0)),
                binding_right: app.binding_right,
                story: app.story.clone(),
                print_margin_info: app.print_margin_info,
                expression: app.expression,
                spine_mm: app.spine_mm,
                cover: app.cover,
            };
            app.work_settings_open = true;
            app.mark_dirty();
        }
        AppCmd::WorkSettingsApply => {
            let d = app.work_settings_draft.clone();
            app.story = d.story;
            app.binding_right = d.binding_right;
            app.print_margin_info = d.print_margin_info;
            app.expression = d.expression;
            app.spine_mm = d.spine_mm;
            app.cover = d.cover;
            // Metadata edits do not bump the doc revision — tell the
            // preflight cache by hand.
            app.preflight_stale = true;
            // Geometry: guides update immediately; existing page pixels stay.
            // New pages (AddPage) pick the new size up via blank_page_doc.
            if d.setup.has_guides() {
                app.page = Some(d.setup);
            }
            app.work_settings_open = false;
            app.mark_pages_dirty();
            app.set_status("work settings updated");
            app.mark_dirty();
        }
        AppCmd::OpenCanvasSize => {
            app.canvas_size_draft = crate::app::CanvasSizeDraft {
                w: app.doc.size.0,
                h: app.doc.size.1,
                anchor: ResizeAnchor::Center,
            };
            app.canvas_size_open = true;
        }
        AppCmd::ResizeCanvasApply => {
            let d = app.canvas_size_draft;
            let (dx, dy) = d.anchor.offsets(app.doc.size, (d.w.max(1), d.h.max(1)));
            app.canvas_size_open = false;
            apply_canvas_resize(app, d.w.max(1), d.h.max(1), dx, dy);
        }
        AppCmd::CropSelection => {
            let bbox = app.doc.selection.as_ref().and_then(selection_bbox);
            match bbox {
                Some([x0, y0, x1, y1]) if x1 > x0 && y1 > y0 => {
                    apply_canvas_resize(app, (x1 - x0) as u32, (y1 - y0) as u32, -x0, -y0);
                }
                _ => app.set_status("crop needs a selection first (M / W)"),
            }
        }
        AppCmd::ExportAllPages => {
            // PM-050: the options window opens FIRST now (prefix, page
            // range, split spreads, script dump), and every field is
            // seeded so an untouched Export writes exactly the files it
            // has always written, under exactly the old names.
            app.export_all_prefix = default_export_stem(app);
            app.export_all_from = 1;
            app.export_all_to = app.pages.len().max(1) as i32;
            app.export_all_open = true;
        }
        AppCmd::ExportAllPagesGo => {}
        AppCmd::ExportAllPagesPath(dir) => match app.stash_current_page() {
            Err(e) => app.set_error(e),
            Ok(()) => {
                // PM-051: an empty prefix falls back to the work name, so
                // clearing the field cannot produce files called "-p001".
                let prefix = {
                    let p = app.export_all_prefix.trim();
                    if p.is_empty() {
                        default_export_stem(app)
                    } else {
                        p.to_owned()
                    }
                };
                // PM-054: the range is 1-based inclusive and clamped; the
                // FILENAME keeps the page's true number, so exporting
                // 5..8 gives -p005..-p008 rather than renumbering from 1.
                let n = app.pages.len();
                let (first, last) = if app.export_all_range && n > 0 {
                    let a = app.export_all_from.clamp(1, n as i32) as usize;
                    let b = app.export_all_to.clamp(1, n as i32) as usize;
                    (a.min(b), a.max(b))
                } else {
                    (1, n)
                };
                let split = app.export_all_split;
                let want_text = app.export_all_text;
                let rtl = app.binding_right;
                // What a NORMAL page is, for the spread test: the work's
                // own paper when it has a page setup, else the narrowest
                // page in the work (cheap — stack.xml, no pixel decode).
                // A work whose pages are all one width therefore has no
                // spread by this measure and nothing splits, which is the
                // right refusal: there is no evidence to guess from.
                let normal_w = match app.page.as_ref().map(|s| s.paper_px().0) {
                    Some(w) => Some(w),
                    None if split => (0..app.pages.len())
                        .map(|i| app.reader_page_canvas(i).0)
                        .min(),
                    None => None,
                };
                let total = last.saturating_sub(first) + 1;
                let dpi = app.tone_dpi();
                let mut ok = 0usize;
                let mut files = 0usize;
                for (i, e) in app.pages.iter().enumerate() {
                    if i + 1 < first || i + 1 > last {
                        continue;
                    }
                    let Some(b) = &e.bytes else { continue };
                    if let Ok(mut d) = mn_core::project::bytes_to_doc(b) {
                        // Tone layers export their derived rasters — the
                        // freshly decoded doc starts with none. Derive
                        // BEFORE any split: the tone screen is canvas-
                        // continuous, so halving first would restart the
                        // dot phase on the second half and the seam would
                        // show in print.
                        d.refresh_derived(dpi);
                        // PM-055: gap 0 — the export must not swallow the
                        // seam. The gutter swallow is an EDIT-time choice
                        // (PM-031), not something a print run gets to do.
                        let halves = (split && is_spread_page(&d, e.spread, normal_w))
                            .then(|| mn_core::page::split_spread(&d, 0))
                            .flatten();
                        match halves {
                            Some((left, right)) => {
                                // `a` is the half a reader meets first —
                                // the RIGHT one in a right-bound work.
                                let (h1, h2) = if rtl { (right, left) } else { (left, right) };
                                for (tag, half) in [("a", &h1), ("b", &h2)] {
                                    let img = mn_core::export::composite_for_export(
                                        half,
                                        d.paper_export_background(),
                                    );
                                    let path = dir.join(format!("{prefix}-p{:03}{tag}.png", i + 1));
                                    if img.save(&path).is_ok() {
                                        files += 1;
                                    }
                                }
                                ok += 1;
                            }
                            None => {
                                let img = mn_core::export::composite_for_export(
                                    &d,
                                    d.paper_export_background(),
                                );
                                let path = dir.join(format!("{prefix}-p{:03}.png", i + 1));
                                if img.save(&path).is_ok() {
                                    ok += 1;
                                    files += 1;
                                }
                            }
                        }
                    }
                }
                // Restore the active-page invariant (bytes live in `doc`).
                app.pages[app.page_index].bytes = None;
                // PM-053 as CSP has it: the script rides along with the
                // image run when the toggle is on.
                let mut extra = String::new();
                if files != ok {
                    extra.push_str(&format!(" ({files} files)"));
                }
                if want_text {
                    let body = app.script_dump();
                    let p = dir.join(format!("{prefix}-text.txt"));
                    extra.push_str(if std::fs::write(&p, body).is_ok() {
                        " + script"
                    } else {
                        " (script FAILED)"
                    });
                }
                app.set_status(format!(
                    "exported {ok}/{total} pages{extra} -> {}",
                    dir.display()
                ));
            }
        },
        AppCmd::OpenPrefs => {
            app.prefs_open = true;
            app.mark_dirty();
        }
        AppCmd::ExportText => {}
        AppCmd::ExportTextPath(p) => {
            // A half-typed balloon is still the document's text: land it
            // before walking the stack.
            app.commit_text_edit();
            let body = app.script_dump();
            match std::fs::write(&p, body) {
                Ok(()) => app.set_status(format!("script -> {}", p.display())),
                Err(e) => app.set_error(format!("script export failed: {e}")),
            }
        }
        AppCmd::OpenOra
        | AppCmd::SaveOra
        | AppCmd::SaveOraAs
        | AppCmd::ExportPng
        | AppCmd::ExportMnc => {
            // Unreachable in practice: `main::pump_commands` turns these into
            // their `*Path` forms. Reaching here means a path was not chosen.
        }
        AppCmd::OpenOraPath(p) => {
            app.commit_text_edit();
            let kind = if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mnc")) {
                mn_core::project::sniff_kind(&p)
            } else {
                mn_core::project::MncKind::Unknown
            };
            match kind {
                // The native multi-page format: a tiny index + side-by-side
                // page files in the same folder.
                mn_core::project::MncKind::WorkFolderIndex => {
                    match mn_core::project::load_folder(&p) {
                        Ok(wf) => match mn_core::project::bytes_to_doc(&wf.pages[0].bytes) {
                            Ok(doc) => {
                                let mn_core::project::WorkFolder {
                                    story,
                                    binding_right,
                                    setup,
                                    expression,
                                    spine_mm,
                                    cover,
                                    next_id,
                                    pages,
                                } = wf;
                                let n = pages.len();
                                // A load lands in a NEW TAB unless the current
                                // document is an untouched blank (session.rs).
                                app.prepare_open_target();
                                app.doc = doc;
                                app.page = setup.filter(|s| s.has_guides());
                                app.story = story;
                                app.binding_right = binding_right;
                                app.expression = expression;
                                app.spine_mm = spine_mm;
                                app.cover = cover;
                                app.pages = pages
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, fp)| PageEntry {
                                        bytes: (i != 0).then_some(fp.bytes),
                                        thumb: None,
                                        uid: PageEntry::next_uid(),
                                        id: fp.id,
                                        rev: fp.rev,
                                        saved_rev: fp.saved_rev,
                                        doc_rev: if i == 0 { app.doc.revision } else { 0 },
                                        spread: false,
                                        preview_img: None,
                                        prev_tex: None,
                                        prev_tex_px: 0.0,
                                        prev_tex_rev: 0,
                                        canvas: None,
                                    })
                                    .collect();
                                app.page_index = 0;
                                let managed = app.page_file_names();
                                app.adopt_folder_state(next_id, managed);
                                app.renderer.invalidate();
                                app.layer_thumbs.clear();
                                app.fit_to_view();
                                app.set_doc_path(Some(p.clone()));
                                app.mark_saved();
                                app.note_recent(&p);
                                app.set_status(format!(
                                    "opened work folder {} ({n} pages)",
                                    p.display()
                                ));
                            }
                            Err(e) => app.set_error(format!("page 1 decode failed: {e}")),
                        },
                        Err(e) => app.set_error(format!("open failed: {e}")),
                    }
                }
                mn_core::project::MncKind::Comic => {
                    app.reset_folder_state();
                    match mn_core::project::load(&p) {
                        Ok(proj) => match mn_core::project::bytes_to_doc(&proj.pages[0]) {
                            Ok(doc) => {
                                app.prepare_open_target();
                                app.doc = doc;
                                // A fresh document cannot honour an armed
                                // mask-edit flag (audit H1).
                                app.disarm_mask_edit_if_unmasked();
                                app.page = proj.meta.setup.filter(|s| s.has_guides());
                                app.story = proj.meta.story;
                                app.binding_right = proj.meta.binding_right;
                                app.expression = proj.meta.expression;
                                app.spine_mm = proj.meta.spine_mm;
                                app.cover = proj.meta.cover;
                                app.pages = proj
                                    .pages
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, b)| PageEntry {
                                        bytes: (i != 0).then_some(b),
                                        ..PageEntry::active()
                                    })
                                    .collect();
                                app.page_index = 0;
                                app.pages[0].doc_rev = app.doc.revision;
                                app.renderer.invalidate();
                                app.layer_thumbs.clear();
                                app.fit_to_view();
                                app.set_doc_path(Some(p.clone()));
                                app.mark_saved();
                                app.note_recent(&p);
                                app.set_status(format!(
                                    "opened {} ({} pages)",
                                    p.display(),
                                    app.pages.len()
                                ));
                            }
                            Err(e) => app.set_error(format!("page 1 decode failed: {e}")),
                        },
                        Err(e) => app.set_error(format!("open failed: {e}")),
                    }
                }
                mn_core::project::MncKind::Unknown => {
                    app.reset_folder_state();
                    match mn_core::ora::load(&p) {
                        Ok(doc) => {
                            app.prepare_open_target();
                            app.doc = doc;
                            app.disarm_mask_edit_if_unmasked();
                            // A bare ORA is a single page with no page-setup
                            // metadata, so guides are off rather than wrong.
                            app.page = None;
                            app.pages = vec![PageEntry::active()];
                            app.page_index = 0;
                            // Every layer index and tile in the cache belongs to
                            // the old document — exactly what invalidate() is for.
                            app.renderer.invalidate();
                            app.layer_thumbs.clear();
                            app.fit_to_view();
                            app.set_doc_path(Some(p.clone()));
                            app.mark_saved();
                            app.note_recent(&p);
                            app.set_status(format!(
                                "opened {} ({} layers)",
                                p.display(),
                                app.doc.layers.len()
                            ));
                        }
                        Err(e) => app.set_error(format!("open failed: {e}")),
                    }
                }
            }
        }
        AppCmd::SaveOraPath(p) => {
            // `work.mnc` = the work-folder flow (native). Anything else keeps
            // the legacy single-file / bare-ORA behaviour.
            let is_work_index = p
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("work.mnc"));
            if is_work_index {
                match app.save_work_folder(&p) {
                    Ok(msg) => {
                        app.set_doc_path(Some(p.clone()));
                        app.mark_saved();
                        app.note_recent(&p);
                        app.set_status(msg);
                    }
                    Err(e) => app.set_error(format!("save failed: {e}")),
                }
            } else {
                let is_mnc = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mnc"));
                if is_mnc {
                    match app.stash_current_page() {
                        Err(e) => app.set_error(e),
                        Ok(()) => {
                            let mut proj = mn_core::Project::new(
                                app.story.clone(),
                                app.page.clone(),
                                app.binding_right,
                            );
                            proj.meta.expression = app.expression;
                            proj.meta.spine_mm = app.spine_mm;
                            proj.meta.cover = app.cover;
                            proj.pages = app
                                .pages
                                .iter()
                                .map(|e| e.bytes.clone().unwrap_or_default())
                                .collect();
                            // The active page keeps living in `doc`, not in bytes.
                            app.pages[app.page_index].bytes = None;
                            match mn_core::project::save(&proj, &p) {
                                Ok(()) => {
                                    app.set_doc_path(Some(p.clone()));
                                    app.mark_saved();
                                    app.note_recent(&p);
                                    app.set_status(format!(
                                        "saved {} ({} pages)",
                                        p.display(),
                                        proj.pages.len()
                                    ));
                                }
                                Err(e) => app.set_error(format!("save failed: {e}")),
                            }
                        }
                    }
                } else {
                    match mn_core::ora::save(&app.doc, &p) {
                        Ok(()) => {
                            app.set_doc_path(Some(p.clone()));
                            app.mark_saved();
                            app.note_recent(&p);
                            if app.is_comic() {
                                app.set_status(format!(
                                    "saved CURRENT PAGE ONLY to {} — use .mnc for the whole comic",
                                    p.display()
                                ));
                            } else {
                                app.set_status(format!("saved {}", p.display()));
                            }
                        }
                        Err(e) => app.set_error(format!("save failed: {e}")),
                    }
                }
            }
            // A successful save makes any autosave shadowing this path stale
            // (PR-040). Leaving it behind means a crash months later offers
            // work the user already replaced — the one way a recovery prompt
            // can do harm.
            if app.doc_path.as_deref() == Some(p.as_path()) && !app.dirty() {
                crate::recovery::clear_sibling_autosave(&p);
                // This document has a real file now, so its never-saved
                // stash is superseded. Left behind, it would be offered
                // after some unrelated crash months later, described as
                // "newer than the file it belongs to" — which it is not.
                crate::recovery::clear_unsaved_stash(app.active_doc);
            }
        }
        AppCmd::ExportMncPath(p) => {
            // The portable single-file copy: never re-points the work at the
            // file, never marks the work clean.
            match app.stash_current_page() {
                Err(e) => app.set_error(e),
                Ok(()) => {
                    let mut proj = mn_core::Project::new(
                        app.story.clone(),
                        app.page.clone(),
                        app.binding_right,
                    );
                    proj.meta.expression = app.expression;
                    proj.meta.spine_mm = app.spine_mm;
                    proj.meta.cover = app.cover;
                    proj.pages = app
                        .pages
                        .iter()
                        .map(|e| e.bytes.clone().unwrap_or_default())
                        .collect();
                    // The active page keeps living in `doc`, not in bytes.
                    app.pages[app.page_index].bytes = None;
                    match mn_core::project::save(&proj, &p) {
                        Ok(()) => app.set_status(format!(
                            "exported single file {} ({} pages)",
                            p.display(),
                            proj.pages.len()
                        )),
                        Err(e) => app.set_error(format!("export failed: {e}")),
                    }
                }
            }
        }
        AppCmd::ExportPsd => {
            // Resolved to ExportPsdPath by `main::pump_commands`.
        }
        AppCmd::ExportPsdPath(p) => {
            app.refresh_tones();
            let file = match std::fs::File::create(&p) {
                Ok(f) => f,
                Err(e) => return app.set_error(format!("psd export failed: {e}")),
            };
            match mn_core::psd::save_psd(&app.doc, std::io::BufWriter::new(file)) {
                Ok(()) => app.set_status(format!(
                    "exported layered PSD ({} layers) -> {}",
                    app.doc.layers.len(),
                    p.display()
                )),
                Err(e) => app.set_error(format!("psd export failed: {e}")),
            }
        }
        AppCmd::ExportPngPath(p) => {
            app.refresh_tones();
            let (w, h) = app.doc.size;
            // PA-001: export on the paper COLOUR whatever the paper's eye
            // says. Hiding the paper is a hole-check, not an export mode —
            // and the transparency checker is screen furniture that must
            // never land in a PNG someone publishes.
            app.renderer.set_paper_override(Some(mn_core::Paper {
                visible: true,
                ..app.doc.paper
            }));
            let img = app.renderer.render_offscreen(&app.doc, w, h);
            app.renderer.set_paper_override(None);
            match img.save(&p) {
                Ok(()) => app.set_status(format!("exported {w}x{h} PNG -> {}", p.display())),
                Err(e) => app.set_error(format!("png export failed: {e}")),
            }
        }

        // --- layers -------------------------------------------------------
        // Structural ops (add/remove/reorder) shift layer indices, which the
        // tile cache keys on and `UndoGroup` records — hence invalidate() here,
        // and hence `Document` clearing the history itself.
        AppCmd::AddLayer => {
            app.commit_text_edit();
            let n = app.doc.layers.len() + 1;
            let name = format!("Layer {n}");
            // CSP: a new layer lands *inside* the active folder, else above
            // the active layer as its sibling.
            let active = app.doc.active;
            if app
                .doc
                .layers
                .get(active)
                .is_some_and(|l| l.folder && l.open)
            {
                app.doc.add_layer_in_folder(active, name);
            } else {
                app.doc.add_layer(name);
            }
            app.renderer.invalidate();
            app.mark_dirty();
        }
        AppCmd::AddVectorLayer => {
            app.commit_text_edit();
            let n = app
                .doc
                .layers
                .iter()
                .filter(|l| l.strokes.is_some())
                .count()
                + 1;
            let li = app.doc.add_layer(format!("Vector {n}"));
            app.doc.layers[li].strokes = Some(mn_core::StrokeSet::default());
            app.doc.set_active(li);
            app.renderer.invalidate();
            app.set_status("vector layer: strokes record as editable geometry");
            app.mark_dirty();
        }
        AppCmd::BatchOpsOpen => {
            app.batch.open = true;
        }
        AppCmd::BatchApply => {
            let s = app.batch_apply();
            app.set_status(s);
        }
        AppCmd::BatchExportPngs => {
            // Resolved to BatchExportPngsPath by `main::pump_commands`.
        }
        AppCmd::BatchExportPngsPath(dir) => {
            let s = app.batch_export_pngs(&dir);
            app.set_status(s);
        }
        AppCmd::VectorDelete { stroke } => {
            let li = app.doc.active;
            let Some(before) = app.doc.layers[li].strokes.clone() else {
                return;
            };
            if stroke >= before.strokes.len() {
                return;
            }
            app.doc.begin_op_on(li);
            app.doc.layers[li]
                .strokes
                .as_mut()
                .expect("checked above")
                .strokes
                .remove(stroke);
            app.vector_sel = None;
            app.rederive_vector_layer(li);
            app.doc.end_op_vector_set(before, "Delete stroke");
            app.renderer.invalidate();
            app.set_status("stroke deleted");
            app.mark_dirty();
        }
        AppCmd::AddFolder => {
            app.commit_text_edit();
            let n = app.doc.layers.iter().filter(|l| l.folder).count() + 1;
            app.doc
                .add_folder_above(app.doc.active, format!("Folder {n}"));
            app.renderer.invalidate();
            app.mark_dirty();
        }
        AppCmd::ToggleFolderOpen(i) => {
            let open = app.doc.layers.get(i).map(|l| l.open).unwrap_or(true);
            if app.doc.set_folder_open(i, !open) {
                app.mark_dirty();
            }
        }
        AppCmd::RemoveLayer => {
            app.commit_text_edit();
            let i = app.doc.active;
            if app.doc.remove_layer(i) {
                app.object_sel = None;
                app.text_sel = None;
                // The active index moved onto whatever now sits there —
                // audit H1: disarm if it carries no mask.
                app.disarm_mask_edit_if_unmasked();
                app.renderer.invalidate();
                app.renumber_frames();
                app.mark_dirty();
            }
        }
        AppCmd::DuplicateLayer => {
            app.commit_text_edit();
            if app.doc.duplicate_layer(app.doc.active).is_some() {
                app.renderer.invalidate();
                app.mark_dirty();
            }
        }
        AppCmd::MoveLayer { from, slot, depth } => {
            app.commit_text_edit();
            if app.doc.move_block_to_slot(from, slot, depth) {
                app.object_sel = None;
                app.text_sel = None;
                app.renderer.invalidate();
                app.layer_thumbs.clear();
                app.mark_dirty();
            }
        }
        AppCmd::MergeDown => {
            app.commit_text_edit();
            let i = app.doc.active;
            let tone_side = app
                .doc
                .layers
                .get(i)
                .zip(app.doc.layers.get(i.wrapping_sub(1)))
                .is_some_and(|(a, b)| a.tone.is_some() || b.tone.is_some());
            if tone_side {
                app.set_status(
                    "merge refuses tone layers — remove the tone first (it is non-destructive)",
                );
            } else if app.doc.merge_down(i) {
                app.object_sel = None;
                // The merged layer is gone; the active index moved (H1).
                app.disarm_mask_edit_if_unmasked();
                app.renderer.invalidate();
                app.layer_thumbs.clear();
                app.set_status("merged with layer below");
                app.mark_dirty();
            } else if app.doc.layers.get(i).is_some_and(|l| l.is_frame()) {
                app.set_status("frame layers keep their vectors — they never merge");
            }
        }
        // --- frames (koma) --------------------------------------------------
        AppCmd::NewFrameLayer => {
            let rect = app
                .page
                .as_ref()
                .filter(|p| p.has_guides())
                .map(|p| p.inner_rect_px())
                .unwrap_or_else(|| {
                    let (w, h) = (app.doc.size.0 as f32, app.doc.size.1 as f32);
                    [w * 0.08, h * 0.08, w * 0.92, h * 0.92]
                });
            // CSP's default frame border is 0.8 mm thick.
            let border = app.mm_to_px(0.8).max(2.0);
            let n = app.doc.layers.iter().filter(|l| l.is_frame()).count() + 1;
            app.doc
                .add_frame_folder(format!("Frame {n}"), FrameSet::single_rect(rect, border));
            app.renderer.invalidate();
            app.renumber_frames();
            app.set_status(
                "frame folder added — draw inside it, U divides panels, its White layer hides art below",
            );
            app.mark_dirty();
        }
        AppCmd::FrameDivide { a, b } => {
            let li = if app.doc.active_layer().is_frame() {
                Some(app.doc.active)
            } else {
                app.doc.layers.iter().rposition(|l| l.is_frame())
            };
            let Some(li) = li else {
                app.set_status("no frame layer — Layer > New frame border folder first");
                return;
            };
            let mut fs = app.doc.layers[li].frames().expect("is_frame").clone();
            // Gutter width blends the two Tool Property values by cut angle:
            // a horizontal cut separates rows (vertical interval), a vertical
            // cut separates columns (horizontal interval). Each cut sub tool
            // keeps its own pair (the owner's CSP values).
            let (g_h, g_v) = if app.frame_mode == FrameMode::DivideBorder {
                app.gutter_border_mm
            } else {
                app.gutter_folder_mm
            };
            let ang = (b.1 - a.1).atan2(b.0 - a.0);
            let gutter = app.mm_to_px(g_v) * ang.cos().abs() + app.mm_to_px(g_h) * ang.sin().abs();
            // CSP "Divide frame folder": the far side of every cut splits off
            // into ONE new frame border folder. TRIAGE 128: what that folder
            // gets is the artist's call — "Do not change" declines the folder
            // entirely and just draws the border.
            let as_folder = app.frame_mode == FrameMode::DivideFolder
                && app.doc.layers[li].folder
                && app.frame_divide_contents != DivideContents::DoNotChange;
            let mut keep = Vec::with_capacity(fs.frames.len() + 1);
            let mut split_off = Vec::new();
            let mut cuts = 0usize;
            // Reading-order provenance (owner top item 2026-08-18): the
            // union of the CUT panels is the slot both halves order
            // inside — division siblings can never scatter.
            let mut cut_union: Option<[f32; 4]> = None;
            for f in fs.frames.drain(..) {
                if f.segment_touches([a.0, a.1], [b.0, b.1]) {
                    let bb = f.bbox();
                    cut_union = Some(match cut_union {
                        None => bb,
                        Some(u) => [
                            u[0].min(bb[0]),
                            u[1].min(bb[1]),
                            u[2].max(bb[2]),
                            u[3].max(bb[3]),
                        ],
                    });
                    if let Some((p, q)) = f.split([a.0, a.1], [b.0, b.1], gutter) {
                        keep.push(p);
                        if as_folder {
                            split_off.push(q);
                        } else {
                            keep.push(q);
                        }
                        cuts += 1;
                        continue;
                    }
                }
                keep.push(f);
            }
            if cuts == 0 {
                app.set_status("drag across a panel to divide it");
            } else if as_folder && !split_off.is_empty() {
                fs.frames = keep;
                fs.slot = slot_for(&fs.frames, cut_union);
                let mut new_fs = fs.clone();
                new_fs.frames = split_off;
                new_fs.slot = cut_union;
                let dup = app.frame_divide_contents == DivideContents::Duplicate;
                let done = if dup {
                    app.doc.divide_frame_folder_dup(li, fs, new_fs)
                } else {
                    app.doc.divide_frame_folder(li, fs, new_fs)
                };
                if done.is_some() {
                    app.renderer.invalidate();
                    app.layer_thumbs.clear();
                    app.renumber_frames();
                    let what = if dup {
                        "with a copy of its art"
                    } else {
                        "empty"
                    };
                    app.set_status(format!(
                        "divided into a new frame folder, {what} ({cuts} cut(s))"
                    ));
                } else {
                    app.set_error("frame folder divide failed");
                }
                app.mark_dirty();
            } else {
                fs.frames = keep;
                fs.slot = slot_for(&fs.frames, cut_union);
                app.doc.set_frames(li, fs);
                app.doc.set_active(li);
                app.renumber_frames();
                app.set_status(format!("divided {cuts} panel(s)"));
                app.mark_dirty();
            }
        }
        AppCmd::FrameRect { a, b } => {
            let (w, h) = ((b.0 - a.0).abs(), (b.1 - a.1).abs());
            if w < 8.0 || h < 8.0 {
                app.set_status("drag out the frame's size");
                return;
            }
            let rect = [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)];
            let border = if app.frame_draw_border {
                app.mm_to_px(app.frame_border_mm).max(1.0)
            } else {
                0.0
            };
            let n = app.doc.layers.iter().filter(|l| l.is_frame()).count() + 1;
            app.doc.add_frame_folder_with(
                format!("Frame {n}"),
                FrameSet::single_rect(rect, border),
                app.frame_fill_inside,
            );
            app.renderer.invalidate();
            app.renumber_frames();
            app.set_status("frame folder added");
            app.mark_dirty();
        }
        AppCmd::FramePoly { points } => {
            let f = mn_core::Frame { points };
            if f.points.len() < 3 || f.area() < mn_core::frame::MIN_FRAME_AREA {
                app.set_status("draw a bigger panel shape");
                return;
            }
            if !f.is_simple() {
                app.set_status("that outline crosses itself — try again");
                return;
            }
            let border = if app.frame_draw_border {
                app.mm_to_px(app.frame_border_mm).max(1.0)
            } else {
                0.0
            };
            let n = app.doc.layers.iter().filter(|l| l.is_frame()).count() + 1;
            app.doc.add_frame_folder_with(
                format!("Frame {n}"),
                mn_core::FrameSet {
                    frames: vec![f],
                    border_px: border,
                    slot: None,
                    reading_pin: None,
                    border_ruler: false,
                },
                app.frame_fill_inside,
            );
            app.renderer.invalidate();
            app.renumber_frames();
            app.set_status("frame folder added");
            app.mark_dirty();
        }
        AppCmd::FrameCommit { layer, frames } => {
            if app.doc.set_frames(layer, frames) {
                app.mark_dirty();
            }
        }
        AppCmd::FrameFoldersCombine { merge_borders } => {
            // Target: the Object tool's selected frame's folder, else the
            // frame folder owning the active layer. Then the next sibling
            // frame folder in stack order.
            let target = if app.tool == Tool::Object
                && let Some((li, _fi)) = app.object_sel
                && let Some(l) = app.doc.layers.get(li)
                && l.is_frame()
            {
                Some(li)
            } else {
                // Walk ancestors outward to the first frame folder (the
                // paste-target walk, one layer up).
                let mut f = enclosing_folder(&app.doc, app.doc.active);
                while let Some(i) = f
                    && !(app.doc.layers[i].folder && app.doc.layers[i].is_frame())
                {
                    f = enclosing_folder(&app.doc, i);
                }
                f
            };
            let Some(a) = target else {
                app.set_status("no frame folder to combine");
                return;
            };
            let depth = app.doc.layers[a].depth;
            let block_end = app.doc.block_range(a).end;
            let next = (block_end..app.doc.layers.len()).find(|&i| {
                app.doc.layers[i].is_frame()
                    && app.doc.layers[i].folder
                    && app.doc.layers[i].depth == depth
            });
            let Some(b) = next else {
                app.set_status("no sibling frame folder below to combine with");
                return;
            };
            match app.doc.combine_frame_folders(a, b, merge_borders) {
                Some(h) => {
                    app.doc.active = h.saturating_sub(1).max(0).min(h);
                    app.object_sel = None;
                    app.renumber_frames();
                    app.set_status(if merge_borders {
                        "frame folders combined — borders merged"
                    } else {
                        "frame folders combined — shapes kept"
                    });
                    app.mark_dirty();
                }
                None => app.set_status(
                    "those folders cannot combine — they must be siblings and \
                     agree on eye, opacity, blend, border and reading pin",
                ),
            }
        }
        AppCmd::FrameFoldersGroup => {
            // FB-037: same target resolution as the combine, but the
            // partner keeps its own header — a plain parent wraps both.
            let target = if app.tool == Tool::Object
                && let Some((li, _fi)) = app.object_sel
                && let Some(l) = app.doc.layers.get(li)
                && l.is_frame()
            {
                Some(li)
            } else {
                let mut f = enclosing_folder(&app.doc, app.doc.active);
                while let Some(i) = f
                    && !(app.doc.layers[i].folder && app.doc.layers[i].is_frame())
                {
                    f = enclosing_folder(&app.doc, i);
                }
                f
            };
            let Some(a) = target else {
                app.set_status("no frame folder to group");
                return;
            };
            let depth = app.doc.layers[a].depth;
            let block_end = app.doc.block_range(a).end;
            let next = (block_end..app.doc.layers.len()).find(|&i| {
                app.doc.layers[i].is_frame()
                    && app.doc.layers[i].folder
                    && app.doc.layers[i].depth == depth
            });
            let Some(b) = next else {
                app.set_status("no sibling frame folder below to group with");
                return;
            };
            match app.doc.group_frame_folders_common_parent(a, b) {
                Some(h) => {
                    let _ = h;
                    app.object_sel = None;
                    app.set_status("common folder created — originals kept");
                    app.mark_dirty();
                }
                None => app.set_status("those folders cannot group (not siblings)"),
            }
        }
        AppCmd::FrameDelete { layer, frame } => {
            // FB-039: deleting a border is silent; deleting the folder's
            // LAST frame takes its art with it — a one-shot confirm, the
            // status line the ask. (Any other command disarms.)
            if let Some(fs) = app.doc.layers.get(layer).and_then(|l| l.frames()) {
                if fs.frames.len() == 1 {
                    if app.frame_delete_armed == Some((layer, frame)) {
                        let name = app.doc.layers[layer].name.clone();
                        if app.doc.remove_layer(layer) {
                            app.object_sel = None;
                            app.set_status(format!("\"{name}\" and its layers deleted"));
                            app.renumber_frames();
                            app.mark_dirty();
                            return;
                        }
                    }
                    app.frame_delete_armed = Some((layer, frame));
                    app.set_status(
                        "that is the folder's last frame — Delete again to                          remove the folder AND its layers",
                    );
                    return;
                }
                let mut fs = fs.clone();
                if frame < fs.frames.len() {
                    fs.frames.remove(frame);
                    app.doc.set_frames(layer, fs);
                    app.object_sel = None;
                    app.set_status("frame deleted");
                    app.renumber_frames();
                    app.mark_dirty();
                }
            }
        }
        AppCmd::FrameExtendEdge { at } => {
            // TRIAGE 129 / FB-030. The tap picks the panel edge nearest it,
            // generously (a fingertip on a tablet is not a pixel).
            let Some(li) = frame_target(app) else {
                app.set_status("no frame layer — Layer > New frame border folder first");
                return;
            };
            let mut fs = app.doc.layers[li].frames().expect("is_frame").clone();
            let p = [at.0, at.1];
            let tol = (20.0 / app.viewport.zoom.max(0.01)).max(10.0);
            let hit = fs
                .frames
                .iter()
                .enumerate()
                .filter_map(|(fi, f)| f.edge_near(p, tol).map(|ei| (fi, ei)))
                .next();
            let Some((fi, ei)) = hit else {
                app.set_status("tap ON a panel edge to run it to the page edge");
                return;
            };
            let canvas = (app.doc.size.0 as f32, app.doc.size.1 as f32);
            let bleed = app.mm_to_px(3.0).max(4.0);
            let before = fs.frames[fi].bbox();
            if !fs.extend_to_edge(fi, ei, canvas, bleed) {
                app.set_status("that edge is already out");
                return;
            }
            let closed = fs.frames[fi].bbox() != before && fs.frames.len() > 1;
            app.doc.set_frames(li, fs);
            app.renumber_frames();
            app.set_status(if closed {
                "edge extended — it stops on the next panel, or runs off the page"
            } else {
                "edge extended to the page"
            });
            app.mark_dirty();
        }
        AppCmd::FrameDivideEqually {
            cols,
            rows,
            fit_to_side,
        } => {
            // TRIAGE 129 / FB-023..025. The cheap half of the pair, and the
            // one a page layout actually starts from.
            let Some(li) = frame_target(app) else {
                app.set_status("no frame layer — Layer > New frame border folder first");
                return;
            };
            let mut fs = app.doc.layers[li].frames().expect("is_frame").clone();
            // Which panel: the Object tool's selection when it is on this
            // layer, else the only one there is. Never a guess.
            let fi = match app.object_sel {
                Some((l, f)) if l == li && f < fs.frames.len() => f,
                _ if fs.frames.len() == 1 => 0,
                _ => {
                    app.set_status("pick the panel to divide with the Object tool first");
                    return;
                }
            };
            let (gx, gy) = app.gutter_border_mm;
            let cells = fs.frames[fi].divide_equally(
                cols,
                rows,
                app.mm_to_px(gx),
                app.mm_to_px(gy),
                fit_to_side,
            );
            let Some(cells) = cells else {
                app.set_status("that division does not fit this panel");
                return;
            };
            let slot = fs.frames[fi].bbox();
            let n = cells.len();
            fs.frames.splice(fi..fi + 1, cells);
            fs.slot = slot_for(&fs.frames, Some(slot));
            app.doc.set_frames(li, fs);
            app.object_sel = None;
            app.renumber_frames();
            app.set_status(format!("divided into {n} panels ({cols} x {rows})"));
            app.mark_dirty();
        }
        AppCmd::FrameBorderRuler { layer } => {
            // TRIAGE 127 / FB-053-054.
            let Some(fs) = app.doc.layers.get(layer).and_then(|l| l.frames()) else {
                app.set_status("that layer has no frames");
                return;
            };
            let mut fs = fs.clone();
            fs.border_ruler = !fs.border_ruler;
            let on = fs.border_ruler;
            app.doc.set_frames(layer, fs);
            app.sync_frame_rulers();
            app.renderer.invalidate();
            app.layer_thumbs.clear();
            app.set_status(if on {
                "border off — the panel outline is a ruler now; ink it with a pen"
            } else {
                "border back on"
            });
            app.mark_dirty();
        }
        AppCmd::BalloonAdd { balloon } => {
            let li = if app.doc.active_layer().is_balloon() {
                Some(app.doc.active)
            } else {
                None
            };
            let selected = match li {
                Some(li) => {
                    let mut bs = app.doc.layers[li].balloons().expect("is_balloon").clone();
                    bs.balloons.push(balloon);
                    let last = bs.balloons.len() - 1;
                    app.doc.set_balloons(li, bs);
                    (li, last)
                }
                None => {
                    // Fresh layer per balloon, CSP-style; border from Tool
                    // Property. Structural op — clears history, like frames.
                    let border = app.mm_to_px(app.balloon_border_mm).max(2.0);
                    let n = app.doc.layers.iter().filter(|l| l.is_balloon()).count() + 1;
                    let mut bs = BalloonSet::new(border);
                    bs.balloons.push(balloon);
                    let li = app.doc.add_balloon_layer(format!("Balloon {n}"), bs);
                    app.renderer.invalidate();
                    (li, 0)
                }
            };
            // The fresh balloon is SELECTED (CSP selects a drawn object) —
            // O's handles and the Tool Property rows apply to it immediately.
            app.balloon_sel = Some(selected);
            app.set_status("balloon added — O edits it, Tail mode attaches a tail");
            app.mark_dirty();
        }
        AppCmd::BalloonTailAdd {
            layer,
            balloon,
            tail,
        } => {
            if let Some(bs) = app.doc.layers.get(layer).and_then(|l| l.balloons()) {
                let mut bs = bs.clone();
                if let Some(b) = bs.balloons.get_mut(balloon) {
                    b.tails.push(tail);
                    app.doc.set_balloons(layer, bs);
                    app.set_status("tail attached");
                    app.mark_dirty();
                }
            }
        }
        AppCmd::BalloonCommit { layer, balloons } => {
            if app.doc.set_balloons(layer, balloons) {
                app.mark_dirty();
            }
        }
        AppCmd::BalloonDelete { layer, balloon } => {
            if let Some(bs) = app.doc.layers.get(layer).and_then(|l| l.balloons()) {
                let mut bs = bs.clone();
                if balloon < bs.balloons.len() {
                    bs.balloons.remove(balloon);
                    app.doc.set_balloons(layer, bs);
                    app.balloon_sel = None;
                    app.set_status("balloon deleted");
                    app.mark_dirty();
                }
            }
        }

        AppCmd::TextCommit { layer, texts } => {
            if app.doc.set_texts(layer, texts) {
                app.mark_dirty();
            }
        }
        AppCmd::TextDelete { layer, text } => {
            app.cancel_text_edit();
            if let Some(ts) = app.doc.layers.get(layer).and_then(|l| l.texts()) {
                let mut ts = ts.clone();
                if text < ts.texts.len() {
                    ts.texts.remove(text);
                    app.warm_texts(layer);
                    app.doc.set_texts(layer, ts);
                    app.text_sel = None;
                    app.set_status("text deleted");
                    app.mark_dirty();
                }
            }
        }
        AppCmd::ClearLayer => {
            app.doc.set_op_label("Clear");
            let l = app.doc.active_layer();
            if l.lock {
                app.set_status("layer is locked");
            } else if l.is_vector() {
                app.set_status("Delete clears raster layers — this one is derived from vectors");
            } else {
                let tiles: Vec<_> = l.tiles().map(|(i, _)| i).collect();
                if tiles.is_empty() {
                    app.set_status("layer is already empty");
                } else {
                    app.doc.begin_op();
                    let li = app.doc.active;
                    for idx in tiles {
                        app.doc.layers[li].tile_mut(idx).data_mut().fill(0);
                    }
                    // Outside a selection the pre-images come back — same
                    // clipping path strokes use.
                    app.doc.mask_op_to_selection();
                    app.doc.end_op();
                    app.set_status(if app.doc.selection.is_some() {
                        "selection cleared"
                    } else {
                        "layer cleared"
                    });
                    app.mark_dirty();
                }
            }
        }

        AppCmd::Copy => match lift_clipboard_source(app) {
            None => app.set_status("nothing to copy"),
            Some(src) => store_clipboard(app, src),
        },
        AppCmd::Cut => {
            let l = app.doc.active_layer();
            if l.lock {
                app.set_status("layer is locked");
            } else if l.is_vector() || l.folder {
                app.set_status("Cut applies to raster layers");
            } else {
                match lift_clipboard_source(app) {
                    None => app.set_status("nothing to cut"),
                    Some(src) => {
                        // Erase exactly the fraction `lift_region` took —
                        // the shared weighted clear (ONE implementation with
                        // `commit_transform`, so the lift/clear pair cannot
                        // drift) — as ONE undo step.
                        let (r, sel) = (src.rect, app.doc.selection.clone());
                        app.doc.begin_op();
                        mn_core::transform::clear_lifted(
                            &mut app.doc.layers[app.doc.active],
                            r,
                            sel.as_ref(),
                        );
                        app.doc.end_op();
                        store_clipboard(app, src);
                        app.mark_dirty();
                    }
                }
            }
        }
        AppCmd::Paste => paste_float(app, PasteWhere::Panel),
        AppCmd::PasteInPlace => paste_float(app, PasteWhere::InPlace),
        AppCmd::PasteShown => paste_float(app, PasteWhere::Shown),
        AppCmd::CompApplyAllPages(i) => {
            let Some(c) = app.doc.comps.get(i).cloned() else {
                return;
            };
            if let Err(e) = app.stash_current_page() {
                app.set_error(e);
                return;
            }
            let (mut ok, mut skip) = (0usize, 0usize);
            for e in app.pages.iter_mut() {
                let Some(b) = &e.bytes else { continue };
                match mn_core::project::bytes_to_doc(b) {
                    Ok(mut d) => {
                        if d.layers.len() == c.vis.len() {
                            for (l, v) in d.layers.iter_mut().zip(&c.vis) {
                                l.visible = *v;
                            }
                            if let Ok(nb) = mn_core::project::doc_to_bytes(&d) {
                                e.bytes = Some(nb);
                                ok += 1;
                            } else {
                                skip += 1;
                            }
                        } else {
                            skip += 1;
                        }
                    }
                    Err(_) => skip += 1,
                }
            }
            // Restore the active-page invariant (bytes live in `doc`).
            app.pages[app.page_index].bytes = None;
            // SELF-AUDIT (Opus 0ee84f8's named blind spot): the LIVE doc
            // is what the owner sees AND what the next save writes — the
            // loop above only touched the stashed bytes, so the comp
            // silently evaporated from the active page on the next save.
            // Apply it here too, same strict structure check.
            if app.doc.layers.len() == c.vis.len() {
                for (l, v) in app.doc.layers.iter_mut().zip(&c.vis) {
                    l.visible = *v;
                }
                app.doc.touch();
                app.mark_dirty();
            }
            app.mark_pages_dirty();
            app.set_status(format!(
                "comp \"{}\" applied to {ok} pages ({skip} skipped — structure mismatch)",
                c.name
            ));
        }
        AppCmd::CompExportAll => {}
        AppCmd::CompExportAllPath(dir) => {
            if app.doc.comps.is_empty() {
                app.set_status("no comps to export — save one first (Layer menu)");
                return;
            }
            let comps = app.doc.comps.clone();
            let stem = if app.story.trim().is_empty() {
                "page".to_owned()
            } else {
                app.story.trim().to_owned()
            };
            let dpi = app.tone_dpi();
            if let Err(e) = app.stash_current_page() {
                app.set_error(e);
                return;
            }
            let mut report = Vec::new();
            // LC-013: a multi-selection (LC-007) exports ONLY those
            // comps; empty selection = everything (CSP's rule).
            let sel = app.comp_multi.clone();
            for (ci, c) in comps.iter().enumerate() {
                if !sel.is_empty() && !sel.contains(&ci) {
                    continue;
                }
                // LC-008 inside the export: every page takes the comp (or
                // keeps its own flags when the structure does not match).
                let sub = dir.join(c.name.replace(
                    |ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
                    "_",
                ));
                let _ = std::fs::create_dir_all(&sub);
                let mut ok = 0usize;
                for (i, e) in app.pages.iter().enumerate() {
                    let Some(b) = &e.bytes else { continue };
                    let Ok(mut d) = mn_core::project::bytes_to_doc(b) else {
                        continue;
                    };
                    if d.layers.len() == c.vis.len() {
                        for (l, v) in d.layers.iter_mut().zip(&c.vis) {
                            l.visible = *v;
                        }
                    }
                    d.refresh_derived(dpi);
                    let img =
                        mn_core::export::composite_for_export(&d, d.paper_export_background());
                    if img
                        .save(sub.join(format!("{stem}-p{:03}.png", i + 1)))
                        .is_ok()
                    {
                        ok += 1;
                    }
                }
                report.push(format!("{}: {ok}", c.name));
            }
            app.pages[app.page_index].bytes = None;
            let scope = if sel.is_empty() {
                format!("all {} comps", comps.len())
            } else {
                format!("{} of {} comps", report.len(), comps.len())
            };
            app.set_status(format!(
                "exported {scope} -> {} ({})",
                dir.display(),
                report.join(", ")
            ));
        }
        AppCmd::NewLiveFill(kind) => {
            let from_sel = app.doc.selection.is_some();
            app.doc.add_fill_layer(kind, from_sel);
            app.refresh_tones();
            app.set_status("live layer — any brush edits its window; parameters in Tool Property");
            app.mark_dirty();
        }
        AppCmd::SetFillParams(li, kind) => {
            if let Some(l) = app.doc.layers.get_mut(li)
                && matches!(l.kind, mn_core::LayerKind::Fill(_))
            {
                l.kind = mn_core::LayerKind::Fill(kind);
                // Persisted state (`mnc-fill`): without the touch, a retint
                // as the session's last action was discarded with no
                // unsaved-changes prompt.
                app.doc.touch();
                app.refresh_tones();
                app.set_status("live layer parameters updated");
                app.mark_dirty();
            }
        }
        AppCmd::SetFillMode(m) => {
            app.fill_mode = m;
            app.set_status(match m {
                FillMode::Click => "fill: click an area",
                FillMode::Enclose => "enclose and fill: drag around the areas to fill",
                FillMode::Lasso => "lasso fill: drag the shape to paint",
            });
        }
        AppCmd::EncloseFill { pts } => {
            app.refresh_tones();
            // Same subsampling as the SE-020 shrink drag: one seed every
            // ~4 px of travel is plenty, and enclosed_pockets skips seeds
            // that land in a pocket it already has.
            let seeds = subsample_path(&pts, 4.0);
            let color = app.active_color();
            let (n, pockets) =
                mn_core::fill::enclose_and_fill(&mut app.doc, &seeds, color, &app.fill_opts);
            app.set_status(if n > 0 {
                format!("{pockets} closed areas filled ({n} px)")
            } else {
                "nothing enclosed — drag right around the areas to fill".into()
            });
            app.mark_dirty();
        }
        AppCmd::LassoFill { pts } => {
            app.refresh_tones();
            let color = app.active_color();
            let path: Vec<[f32; 2]> = pts.iter().map(|&(x, y)| [x, y]).collect();
            app.doc.set_op_label("Lasso fill");
            if app.doc.fill_polygon(&path, color, 1.0) {
                app.set_status("lasso filled");
            } else {
                app.set_status("lasso fill needs a raster layer (unlocked)");
            }
            app.mark_dirty();
        }
        AppCmd::PasteMaterial { path, tile } => {
            // Same paste-to-position rule as Ctrl+V (owner HIGH): a tone
            // dropped into its panel is the same gesture. The tiling
            // variant stays canvas-wide by design — no aiming there.
            let target = if tile {
                None
            } else {
                let p = app.last_pointer;
                let pointer = (!app.shell.owns_pointer(p.0, p.1)).then(|| {
                    let c = app.viewport.to_canvas(p.0 as f32, p.1 as f32);
                    (c.0, c.1)
                });
                resolve_paste_target(&app.doc, app.doc.active, pointer)
            };
            let creates_layer = target
                .as_ref()
                .is_some_and(|t| !t.owns_active && t.folder.is_some());
            if !creates_layer {
                let l = app.doc.active_layer();
                if l.lock {
                    app.set_status("layer is locked");
                    return;
                }
                if l.is_vector() || l.folder {
                    app.set_status("Material pastes target raster layers");
                    return;
                }
            }
            let Ok(img) = image::open(&path) else {
                app.set_status(format!("material failed to load: {}", path.display()));
                return;
            };
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            // BGRA byte order (the clipboard module's conversion contract).
            let mut bgra = rgba.into_raw();
            for px in bgra.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            let (cw, ch) = (app.doc.size.0 as usize, app.doc.size.1 as usize);
            let mut src = if tile {
                // The owner's tiling: one float covering the whole canvas in
                // N×N copies — usable as a mask to draw through.
                let mut tiled = vec![0u8; cw * ch * 4];
                for y in 0..ch {
                    let sy = y % h as usize;
                    let srow = &bgra[sy as usize * w as usize * 4..];
                    for x in 0..cw {
                        let sx = x % w as usize;
                        tiled[(y * cw + x) * 4..(y * cw + x) * 4 + 4]
                            .copy_from_slice(&srow[sx * 4..sx * 4 + 4]);
                    }
                }
                crate::clipboard::bgra_to_floatsource(
                    &tiled,
                    cw as u32,
                    ch as u32,
                    [0, 0],
                    cw as i32,
                    ch as i32,
                )
            } else {
                // Aiming at a panel seeds the float at the panel corner so
                // nothing clips away; open_float_aimed re-centres it.
                let mut c = target
                    .as_ref()
                    .map(|t| (t.rect[0] + w as f32 * 0.5, t.rect[1] + h as f32 * 0.5))
                    .unwrap_or_else(|| {
                        app.viewport
                            .to_canvas(app.canvas_center()[0], app.canvas_center()[1])
                    });
                if c.0 < 0.0
                    || c.1 < 0.0
                    || c.0 >= app.doc.size.0 as f32
                    || c.1 >= app.doc.size.1 as f32
                {
                    // A degenerate view (headless tests, a shell that has
                    // not laid out yet) — paste at the document centre
                    // rather than fully off-canvas.
                    c = (app.doc.size.0 as f32 * 0.5, app.doc.size.1 as f32 * 0.5);
                }
                let at = [c.0 as i32 - w as i32 / 2, c.1 as i32 - h as i32 / 2];
                crate::clipboard::bgra_to_floatsource(
                    &bgra,
                    w,
                    h,
                    at,
                    app.doc.size.0 as i32,
                    app.doc.size.1 as i32,
                )
            };
            if src.tiles.is_empty() {
                app.set_status("material is empty or fully off-canvas");
                return;
            }
            // MT-014 Toning: the material's ink renders as the document's
            // screentone — the tone engine's own raster (canvas-continuous
            // screen, ink coverage from the source pixels), so an arbitrary
            // image becomes printable on a mono page.
            if app.material_tone {
                let p = mn_core::ToneParams::default();
                let dpi = app.tone_dpi();
                let mut toned = std::collections::HashMap::new();
                for (idx, t) in &src.tiles {
                    let out = mn_core::tone::rasterize_tile(t, idx.origin(), &p, dpi);
                    if out.alpha_sum() > 0 {
                        toned.insert(*idx, std::sync::Arc::new(out));
                    }
                }
                src.tiles = toned;
                if src.tiles.is_empty() {
                    app.set_status("material tones to nothing (no ink)");
                    return;
                }
            }
            app.material_note_use(&path);
            let n = if tile { " (tiled)" } else { "" };
            let t = if app.material_tone { " (toned)" } else { "" };
            let into = target
                .as_ref()
                .map(|tg| format!(" into {}", tg.label))
                .unwrap_or_default();
            open_float_aimed_sized(
                app,
                src,
                target.as_ref(),
                app.material_size,
                app.material_order,
            );
            app.set_status(format!(
                "material {} pasted{n}{t}{into} — drag to move, Enter to commit",
                path.file_stem()
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default()
            ));
        }
        AppCmd::MaterialAddFolder(p) => {
            if app.material_folders.iter().any(|f| *f == p) {
                app.set_status("folder already in the bank");
                return;
            }
            app.material_folders.push(p.clone());
            app.materials_scan();
            app.layout.note_materials(
                &app.user_material_folders(),
                &serde_json::to_string(&app.material_uses).unwrap_or_default(),
            );
            app.set_status(format!(
                "material folder added — {} items",
                app.materials.len()
            ));
        }
        AppCmd::MaterialRescan => {
            app.materials_scan();
            app.set_status(format!("rescanned — {} materials", app.materials.len()));
        }
        AppCmd::MaterialSetTags { path, tags } => {
            let name = path
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !app.material_set_tags(&path, &tags) {
                app.set_status(format!(
                    "could not write {} beside {name} — is the folder read-only?",
                    crate::app::materials::TAGS_FILE
                ));
                return;
            }
            let now = tags.trim();
            app.set_status(if now.is_empty() {
                format!("cleared \"{name}\"'s tags")
            } else {
                format!("tagged \"{name}\": {now}")
            });
        }
        AppCmd::TransformStart => {
            let l = app.doc.active_layer();
            if l.lock {
                app.set_status("layer is locked");
            } else if l.is_vector() || l.folder {
                app.set_status("Transform applies to raster layers");
            } else {
                // Source rect: the selection's bounds when one exists, else
                // the layer's populated tile bounds (shared with Flip).
                match transform_lift_rect(app) {
                    None => app.set_status("nothing to transform"),
                    Some(r) if r[0] >= r[2] || r[1] >= r[3] => {
                        app.set_status("nothing to transform")
                    }
                    Some(r) => {
                        let src = mn_core::transform::lift_region(l, r, app.doc.selection.as_ref());
                        if src.tiles.is_empty() {
                            app.set_status("nothing to transform");
                        } else {
                            // The overlay preview is uploaded once, here; the
                            // drag then only moves the quad (GPU-drawn).
                            let preview_tex =
                                crate::app::transform_preview(&src, 2048).map(|img| {
                                    app.shell.ctx.load_texture(
                                        "mn.transform.preview",
                                        img,
                                        egui::TextureOptions::LINEAR,
                                    )
                                });
                            app.transform_drag = Some(crate::app::TransformDrag {
                                source: src,
                                xform: mn_core::Affine2::IDENTITY,
                                bbox: [
                                    [r[0] as f32, r[1] as f32],
                                    [r[2] as f32, r[1] as f32],
                                    [r[2] as f32, r[3] as f32],
                                    [r[0] as f32, r[3] as f32],
                                ],
                                sx: 1.0,
                                sy: 1.0,
                                rad: 0.0,
                                tx: 0.0,
                                ty: 0.0,
                                pivot_override: None,
                                gesture: None,
                                stamp_on_identity: false,
                                // A genuine lift off the layer: commit
                                // clears the source, weighted by the
                                // selection as it stands RIGHT NOW.
                                clear_source: true,
                                lift_selection: app.doc.selection.clone(),
                                create_in: None,
                                order: crate::app::MaterialLayerOrder::Above,
                                preview_tex,
                            });
                            app.set_status(
                                "transform: drag inside to move, corners to scale, outside to rotate — Enter commits, Esc cancels",
                            );
                            app.mark_dirty();
                        }
                    }
                }
            }
        }
        AppCmd::TransformCommit => {
            app.doc.set_op_label("Transform");
            if let Some(drag) = app.transform_drag.take() {
                if drag.is_identity() && !drag.stamp_on_identity {
                    // Nothing moved — drop the float without an undo step.
                    app.set_status("transform canceled");
                } else {
                    // Paste-into-panel (owner HIGH): the fresh layer lands
                    // INSIDE the frame folder as its topmost child and
                    // active, so the stamp below hits it and the folder
                    // seal clips the art to the panel. Structural like
                    // every layer-list change (history clears); Esc before
                    // this point leaves nothing behind.
                    // (MT-034 note: the pre-paste active is not needed — BelowActive
                    // turned out unreachable under r74 rules and was cut.)
                    let mut refused = false;
                    if let Some(folder) = drag.create_in {
                        // Index captured at paste time; anything that
                        // reshuffled the stack while the float was open must
                        // not silently redirect the stamp.
                        let ok = app.doc.layers.get(folder).is_some_and(|l| l.is_frame())
                            && app.doc.add_layer_in_folder(folder, "Pasted").is_some();
                        refused = !ok;
                    }
                    if refused {
                        app.set_status("transform refused — target folder is gone");
                    } else {
                        // commit_transform brackets its own single undo op.
                        // The source clear (lifted floats only) uses the
                        // LIFT-TIME selection: deselecting or re-lassoing
                        // while the float was open must not change what
                        // gets erased.
                        app.doc.set_op_label("Transform");
                        let ok = mn_core::transform::commit_transform(
                            &mut app.doc,
                            &drag.source,
                            &drag.xform,
                            drag.lift_selection.as_ref(),
                            drag.clear_source,
                            None, // CPU resample; GPU path is a follow-up
                        );
                        // LM-009: a pure translation drags a LINKED mask
                        // with the art (the hole stays over the same ink);
                        // scale/rotate/skew leave it (mask resampling is a
                        // later cut). Raster masks are pixel grids — the
                        // translation rounds. Its own mask-op undo group:
                        // the dual-step convention (content + mask), same
                        // as the Object tool's frame move.
                        if ok {
                            let li = app.doc.active;
                            let pure_t = drag.xform.m == mn_core::Affine2::IDENTITY.m
                                && (drag.xform.t[0] != 0.0 || drag.xform.t[1] != 0.0);
                            // Lifted floats only: a PASTE translation moves
                            // pasted pixels, not the layer's art, so the
                            // layer's mask must stay where its ink is.
                            if pure_t
                                && drag.clear_source
                                && let Some(l) = app.doc.layers.get_mut(li)
                                && l.mask.is_some()
                                && l.mask_linked
                            {
                                let dx = drag.xform.t[0].round() as i32;
                                let dy = drag.xform.t[1].round() as i32;
                                app.doc.mask_op_begin();
                                if let Some(l) = app.doc.layers.get_mut(li)
                                    && let Some(m) = &mut l.mask
                                {
                                    m.tiles = mn_core::doc::shift_tile_map(&m.tiles, dx, dy);
                                    m.revision = mn_core::tile::next_revision();
                                }
                                app.doc.mask_op_end();
                                app.renderer.invalidate();
                            }
                            // MT-034: where the pasted layer sits in the
                            // panel folder (only rule-2 pastes create a
                            // layer; the palette dropdown set drag.order).
                            if drag.order != crate::app::MaterialLayerOrder::Above
                                && let Some(folder) = drag.create_in
                                // add_layer_in_folder inserts AT the header index,
                                // so the header moved to folder + 1 with the new layer.
                                && app.doc.layers.get(folder + 1).is_some_and(|l| l.folder)
                            {
                                let folder = folder + 1;
                                let li = app.doc.active;
                                let to = match drag.order {
                                    crate::app::MaterialLayerOrder::BottomOfPanel => {
                                        Some(app.doc.children_range(folder).start)
                                    }
                                    crate::app::MaterialLayerOrder::Above => None,
                                };
                                if let Some(to) = to {
                                    app.doc.move_layer(li, to);
                                }
                            }
                        }
                        app.set_status(if ok {
                            "transform committed"
                        } else {
                            "transform refused"
                        });
                    }
                }
                app.mark_dirty();
            }
        }
        AppCmd::TransformCancel => {
            if app.transform_drag.take().is_some() {
                app.set_status("transform canceled");
                app.mark_dirty();
            }
        }
        AppCmd::TransformUpdate {
            sx,
            sy,
            rad,
            tx,
            ty,
        } => {
            if let Some(drag) = &mut app.transform_drag {
                drag.set_params(sx, sy, rad, tx, ty);
                app.mark_dirty();
            }
        }
        AppCmd::TransformFlip { horizontal } => {
            // In an active transform: a flip BUTTON (T-021). Standalone
            // (TRIAGE 130): lift, mirror about the region centre, commit —
            // one undo step, selection-bounded like every whole-layer op.
            if let Some(drag) = &mut app.transform_drag {
                drag.flip(horizontal);
                app.set_status("flipped about the reference point");
                app.mark_dirty();
            } else {
                let l = app.doc.active_layer();
                if l.lock {
                    app.set_status("layer is locked");
                } else if l.is_vector() || l.folder {
                    app.set_status("Flip applies to raster layers");
                } else {
                    let rect = transform_lift_rect(app);
                    let valid = rect.is_some_and(|r| r[0] < r[2] && r[1] < r[3]);
                    match (rect, valid) {
                        (Some(r), true) => {
                            let src =
                                mn_core::transform::lift_region(l, r, app.doc.selection.as_ref());
                            if src.tiles.is_empty() {
                                app.set_status("nothing to flip");
                            } else {
                                let pivot =
                                    [(r[0] + r[2]) as f32 * 0.5, (r[1] + r[3]) as f32 * 0.5];
                                let xform = if horizontal {
                                    mn_core::Affine2::scale_rotate_around(
                                        pivot,
                                        -1.0,
                                        1.0,
                                        0.0,
                                        [0.0, 0.0],
                                    )
                                } else {
                                    mn_core::Affine2::scale_rotate_around(
                                        pivot,
                                        1.0,
                                        -1.0,
                                        0.0,
                                        [0.0, 0.0],
                                    )
                                };
                                app.doc.set_op_label("Flip");
                                // Lift and commit are one atomic action
                                // here, so the live selection IS the
                                // lift-time selection.
                                let sel = app.doc.selection.take();
                                let ok = mn_core::transform::commit_transform(
                                    &mut app.doc,
                                    &src,
                                    &xform,
                                    sel.as_ref(),
                                    true,
                                    None,
                                );
                                app.doc.selection = sel;
                                app.set_status(if ok {
                                    if horizontal {
                                        "flipped horizontally"
                                    } else {
                                        "flipped vertically"
                                    }
                                } else {
                                    "flip refused"
                                });
                                app.mark_dirty();
                            }
                        }
                        _ => app.set_status("nothing to flip"),
                    }
                }
            }
        }
        AppCmd::TransformSetPivot { pivot } => {
            if let Some(drag) = &mut app.transform_drag {
                match pivot {
                    Some(p) => drag.set_pivot(p),
                    // Reset to the source centre: deriving about the centre
                    // and clearing the override are the same transform.
                    None => {
                        let r = drag.source.rect;
                        let c = [(r[0] + r[2]) as f32 * 0.5, (r[1] + r[3]) as f32 * 0.5];
                        drag.set_pivot(c);
                        drag.pivot_override = None;
                    }
                }
                app.mark_dirty();
            }
        }

        AppCmd::SetTaper { px, min } => {
            app.props_current.taper_px = px.clamp(0.0, 500.0);
            app.props_current.taper_min = min.clamp(0.0, 1.0);
            let t = app.brush.inner_mut();
            t.length_px = app.props_current.taper_px;
            t.min = app.props_current.taper_min;
            app.mark_dirty();
        }
        AppCmd::Autosave => {
            // Background tabs first: they have no other way to be written,
            // and the tick used to ignore them entirely (a crash then took
            // their work with no recovery file to offer). Encoded from their
            // parked state, so this never disturbs the live document.
            let parked = app.autosave_parked();

            // Skip while clean or mid-stroke; never touches doc_path or the
            // dirty state (an autosave is not the user's save).
            let _ = parked;
            if app.dirty() && !app.drawing() {
                // Work-folder-backed works autosave IN PLACE, incrementally:
                // each changed page lands atomically (tmp+rename), the index
                // commits last. Nothing is rewritten for untouched pages —
                // that is the point of the folder format.
                let folder_index = app
                    .doc_path
                    .as_ref()
                    .filter(|p| {
                        p.file_name()
                            .is_some_and(|n| n.eq_ignore_ascii_case("work.mnc"))
                    })
                    .cloned();
                if let Some(p) = folder_index {
                    match app.save_work_folder(&p) {
                        Ok(msg) => app.set_status(format!("autosave: {msg}")),
                        Err(e) => app.set_error(format!("autosave failed: {e}")),
                    }
                } else if let Err(e) = app.stash_current_page() {
                    app.set_error(format!("autosave stash failed: {e}"));
                } else {
                    let mut proj = mn_core::Project::new(
                        app.story.clone(),
                        app.page.clone(),
                        app.binding_right,
                    );
                    proj.meta.expression = app.expression;
                    proj.meta.spine_mm = app.spine_mm;
                    proj.meta.cover = app.cover;
                    proj.pages = app
                        .pages
                        .iter()
                        .map(|e| e.bytes.clone().unwrap_or_default())
                        .collect();
                    app.pages[app.page_index].bytes = None;
                    // Both spellings come from `recovery`, which is also what
                    // READS them back after a crash (PR-040) — a literal here
                    // and a literal there is how a recovery feature ends up
                    // hunting for a file nothing writes.
                    let path = app
                        .doc_path
                        .as_ref()
                        .map(|p| crate::recovery::sibling_autosave(p))
                        .unwrap_or_else(|| crate::app::unsaved_autosave_path_for(app.active_doc));
                    match mn_core::project::save(&proj, &path) {
                        Ok(()) => app.set_status(format!("autosaved -> {}", path.display())),
                        Err(e) => app.set_error(format!("autosave failed: {e}")),
                    }
                }
            }
        }
        AppCmd::RenameLayer(i, name) => {
            if app.doc.rename_layer(i, name) {
                app.mark_dirty();
            }
        }
        AppCmd::SelectLayer(i) => {
            app.commit_text_edit();
            // PA-001: picking a layer un-picks the Paper row, whichever way
            // the pick arrived (palette row, shortcut, another command).
            app.paper_selected = false;
            if app.doc.set_active(i) {
                // Audit H1: armed mask-edit must not survive onto a layer
                // that has no mask.
                app.disarm_mask_edit_if_unmasked();
                // A stroke index is only meaningful on the layer it was
                // picked on — carried across, it would light an unrelated
                // stroke on the next vector layer with enough strokes.
                app.vector_sel = None;
                app.mark_dirty();
            }
        }
        // Opacity / blend / visibility need no invalidate: the compositor keeps
        // a per-layer signature and rebuilds the canvas when one changes.
        AppCmd::SetLayerOpacity(i, v) => {
            if app.doc.set_layer_opacity(i, v) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerBlend(i, b) => {
            if app.doc.set_layer_blend(i, b) {
                app.mark_dirty();
            }
        }
        AppCmd::SetFolderThrough(i, on) => {
            if app.doc.set_folder_through(i, on) {
                app.set_status(if on {
                    "folder Through — its layers now blend with the page beneath"
                } else {
                    "folder sealed (Normal)"
                });
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerVisible(i, v) => {
            if app.doc.set_layer_visible(i, v) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerLabel(i, l) => {
            if app.doc.set_layer_label(i, l) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerColour(i, c) => {
            if app.doc.set_layer_colour(i, c) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerClip(i, v) => {
            if app.doc.set_layer_clip(i, v) {
                app.set_status(if v {
                    "clipped to the layer below"
                } else {
                    "clip removed"
                });
                app.mark_dirty();
            } else {
                app.set_status("folders cannot clip — their group already isolates");
            }
        }
        AppCmd::SetTone(tone) => {
            let i = app.doc.active;
            match app.doc.set_tone(i, tone) {
                true => {
                    // Derived rasters (or their absence) are newer than
                    // whatever the GPU cache holds for this layer.
                    app.renderer.evict_layer(i);
                    app.refresh_tones();
                    // TN-009: while the lattice is off its origin, SAY so —
                    // the art has not moved and nothing else on screen tells
                    // you which of two tone layers you just nudged.
                    app.set_status(match tone {
                        Some(t) if t.offset != [0.0, 0.0] => format!(
                            "tone lattice at ({:+.1}, {:+.1}) px — the art stays put; nudge it to break moiré against another tone layer",
                            t.offset[0], t.offset[1]
                        ),
                        Some(_) => "tone layer — paint grey/black ink, the screen follows; Layer Property tunes it".to_string(),
                        None => "tone removed — painted ink back to plain pixels".to_string(),
                    });
                    app.mark_dirty();
                }
                false => {
                    if app
                        .doc
                        .layers
                        .get(i)
                        .is_some_and(|l| l.folder || l.is_vector())
                    {
                        app.set_status("folders and vector layers cannot be tones");
                    }
                }
            }
        }
        AppCmd::ToneShowArea => {
            app.tone_show_area = !app.tone_show_area;
            app.set_status(if app.tone_show_area {
                "tone area shown (green tint over every toned region — a print check, not part of the art)"
            } else {
                "tone area hidden"
            });
            app.mark_dirty();
        }
        AppCmd::SetLayerLock(i, v) => {
            if app.doc.set_layer_lock(i, v) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerLockAlpha(i, v) => {
            if app.doc.set_layer_lock_alpha(i, v) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerReference(i, v) => {
            // RF-001: independent toggle; a FOLDER row toggles its whole
            // child run (the folder is one unit).
            let targets = reference_unit(&app.doc, i);
            let mut any = false;
            for &t in &targets {
                any |= app.doc.set_layer_reference(t, v);
            }
            if any {
                let n = app.doc.reference_layers().len();
                app.set_status(if n > 0 {
                    format!(
                        "{n} reference layer{} — fill/wand refer to them",
                        if n > 1 { "s" } else { "" }
                    )
                } else if v {
                    "reference layer set — fill/wand can refer to it".into()
                } else {
                    "reference layer cleared".into()
                });
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerReferenceSolo(i) => {
            if app.doc.set_layer_reference_solo(i) {
                app.set_status("reference solo — every other layer cleared");
                app.mark_dirty();
            }
        }
        AppCmd::ClearReferences => {
            if app.doc.reference_layers().is_empty() {
                app.set_status("no reference layers set");
            } else {
                app.doc.clear_references();
                app.set_status("reference layers cleared");
                app.mark_dirty();
            }
        }
        AppCmd::RulerArm(kind) => {
            app.ruler_pending = Some(kind);
            app.set_status(match kind {
                RulerKind::Line => "drag on the canvas to draw a line ruler",
                RulerKind::VanishingPoint => "drag from the vanishing point to set its first ray",
                RulerKind::Perspective => {
                    "drag the eye level — both ends become vanishing points; strokes aim at either VP or run vertical"
                }
                RulerKind::Perspective1 => {
                    "drag from the vanishing point along the eye level; strokes aim at it, or run along/across the horizon"
                }
                RulerKind::Perspective3 => {
                    "drag the eye level — a third vanishing point lands on the side you dragged toward; drag it where you want it"
                }
                RulerKind::Curve => "click the curve's corners — double-click (or Enter) to finish",
                RulerKind::Parallel => "drag the direction — every stroke comes out parallel to it",
                RulerKind::Concentric => "drag from the centre — the length sets the ring spacing",
                RulerKind::Symmetric => {
                    "drag from the symmetry centre outward — the drag sets the first axis"
                }
                RulerKind::GuideH => "click where the horizontal guide goes",
                RulerKind::GuideV => "click where the vertical guide goes",
            });
        }
        AppCmd::RulerSnapToggle => {
            app.doc.rulers.on = !app.doc.rulers.on;
            app.rebuild_twins();
            app.set_status(if app.doc.rulers.on {
                "ruler snapping ON"
            } else {
                "ruler snapping OFF (rulers stay drawn)"
            });
            app.mark_dirty();
        }
        AppCmd::RulerSpecialSnapToggle => {
            app.doc.rulers.special_on = !app.doc.rulers.special_on;
            app.rebuild_twins();
            app.set_status(if app.doc.rulers.special_on {
                "special rulers ON (parallel/concentric/guide/symmetry)"
            } else {
                "special rulers OFF (line/curve/vanishing-point rulers unaffected)"
            });
            app.mark_dirty();
        }
        AppCmd::RulerSymmetricCount => {
            // CSP's ladder, cycled. Applies to every symmetric ruler (you
            // keep one) and to the default for the next created.
            const LADDER: [u16; 7] = [2, 3, 4, 6, 8, 12, 16];
            let cur = app
                .doc
                .rulers
                .items
                .iter()
                .rev()
                .find_map(|r| match r {
                    mn_core::Ruler::Symmetric { lines, .. } => Some(*lines),
                    _ => None,
                })
                .unwrap_or(app.symmetric_lines);
            let next = LADDER
                .iter()
                .position(|&n| n == cur)
                .map(|i| LADDER[(i + 1) % LADDER.len()])
                .unwrap_or(2);
            let mut changed = 0;
            for r in &mut app.doc.rulers.items {
                if let mn_core::Ruler::Symmetric { lines, .. } = r {
                    *lines = next;
                    changed += 1;
                }
            }
            app.symmetric_lines = next;
            app.rebuild_twins();
            app.set_status(if changed > 0 {
                format!("symmetric rulers: {next} lines")
            } else {
                format!("symmetry line count: {next} (creates at this count)")
            });
            app.mark_dirty();
        }
        AppCmd::RulerClear => {
            let before = app.doc.rulers.clone();
            app.doc.rulers.items.clear();
            // The curve rulers go too (issue #3) — but only the hand-made
            // ones. A panel border published as a ruler is the FRAME's
            // property, and `sync_frame_rulers` retracts by value against
            // `frame_rulers`; dropping those here would desync that
            // bookkeeping (they would vanish now and never be retracted).
            app.doc.rulers.curves = app.frame_rulers.clone();
            app.ruler_pending = None;
            // A live sticky lock indexing into the cleared set would fall
            // to snap_locked's else (unsnapped) — safe, but stale; drop it
            // (round-47 handoff item 1).
            app.ruler_lock = Default::default();
            // Same for a live move (its index is into the cleared set).
            app.ruler_move = None;
            app.rebuild_twins();
            // One gesture, one step — and the step's pre-image holds the
            // hand-made curves the clear dropped, so undo brings exactly
            // those back. The frame-published ones never left, so undo
            // cannot double them.
            app.doc.record_rulers(before, "Delete rulers");
            app.set_status(if app.frame_rulers.is_empty() {
                "rulers cleared"
            } else {
                "rulers cleared — panel-border rulers stay with their frames"
            });
            app.mark_dirty();
        }
        AppCmd::SetLayerDraft(i, v) => {
            if app.doc.set_layer_draft(i, v) {
                app.set_status(if v {
                    "draft layer: shown on screen, skipped by fill refs and export"
                } else {
                    "draft flag removed"
                });
                app.mark_dirty();
            }
        }

        // --- brush --------------------------------------------------------
        AppCmd::SelectBrush(p) => {
            // TODO #7, the `mn-engine` preset key: grid/hairy/curve/dyna
            // build their own engine instead of the MyPaint one
            // (per-sub-tool identities without new preset formats).
            let engine_kind = std::fs::read_to_string(&p).ok().and_then(|text| {
                serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|j| {
                        j.get("mn-engine")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned)
                    })
            });
            let special = match engine_kind.as_deref() {
                Some("grid") => Some(EngineKind::Grid(GridDab::default())),
                Some("hairy") => Some(EngineKind::Hairy(HairyDab::default())),
                Some("curve") => Some(EngineKind::Curve(CurveDab::default())),
                Some("dyna") => Some(EngineKind::Dyna(DynaDab::default())),
                _ => None,
            };
            if let Some(kind) = special {
                app.store_current_props();
                app.selected_preset = app.presets.iter().position(|(_, q)| *q == p);
                *app.engine_mut() = Engine::new(kind);
                app.load_props_for(&p);
                app.apply_props();
                app.apply_draw_state();
                // The replaced engine took the symmetry twins with it —
                // this arm returned before the MyBrush arm's rebuild, so
                // switching to hairy/curve/dyna/grid silently inked one
                // half of a mirrored drawing.
                app.rebuild_twins();
                app.set_status(match engine_kind.as_deref() {
                    Some("hairy") => "hairy engine: bristle fan",
                    Some("curve") => "curve engine: scallop arches",
                    Some("dyna") => "dyna engine: spring tip",
                    _ => "grid engine: lattice dots",
                });
                app.mark_dirty();
                return;
            }
            match MyBrush::load(&p) {
                Ok(b) => {
                    // CSP model: the outgoing sub tool keeps its settings, the
                    // incoming one restores its own (or starts from defaults).
                    app.store_current_props();
                    app.selected_preset = app.presets.iter().position(|(_, q)| *q == p);
                    *app.engine_mut() = Engine::new(EngineKind::My(Box::new(b)));
                    app.load_props_for(&p);
                    app.apply_props();
                    app.apply_draw_state();
                    // This sub tool's curve edits, replayed on the fresh engine
                    // (they live only in the session, never in the preset file).
                    let overrides: Vec<((u8, u8), Vec<(f32, f32)>)> = app
                        .curve_overrides
                        .get(&p)
                        .map(|m| m.iter().map(|(k, v)| (*k, v.clone())).collect())
                        .unwrap_or_default();
                    for ((s, n), pts) in overrides {
                        let (cs, sn) = (CurveSetting::from_index(s), CurveSensor::from_index(n));
                        if let (Some(sid), Some(iid)) = (cs.setting_id(), sn.input_id()) {
                            app.engine_mut().set_mapping(sid, iid, &pts);
                        }
                    }
                    // The replaced engine took the symmetry twins with it.
                    // Unconditional: rebuild_twins derives the whole twin
                    // set (mirror/wrap AND the symmetric ruler's, which
                    // the old mirror-only guard missed) from current state
                    // and is a cheap no-op when none apply.
                    app.rebuild_twins();
                    // TL-013: the one moment a locked tool's snap-back
                    // happens is right here, so the line that names the
                    // brush is the line that has to mention it.
                    let name = app.brush_name().to_owned();
                    let lock = if app.props_current.locked {
                        " (locked — settings restored)"
                    } else {
                        ""
                    };
                    app.set_status(format!("brush: {name}{lock}"));
                }
                Err(e) => app.set_error(format!("brush {} failed: {e}", p.display())),
            }
        }
        AppCmd::SetBrushSizePx(px) => {
            let px = if px.is_finite() { px } else { DEFAULT_SIZE_PX };
            app.props_current.size_px = px.clamp(SIZE_PX_MIN, SIZE_PX_MAX);
            let px = app.props_current.size_px;
            app.engine_mut().set_size_px(px);
            app.mark_dirty();
        }
        AppCmd::SetInterval(iv) => {
            app.props_current.interval = iv;
            // Fixed remembers its gap across a trip through the relative
            // modes; the other modes leave the remembered number alone.
            if let Interval::FixedPx(px) = iv {
                app.props_current.interval_px = px;
            }
            app.engine_mut().set_interval(iv);
            app.mark_dirty();
        }
        AppCmd::SetDensityByGap(on) => {
            app.props_current.density_by_gap = Some(on);
            app.engine_mut().set_density_by_gap(on);
            app.mark_dirty();
        }
        AppCmd::SetAntiAlias(aa) => {
            app.props_current.anti_alias = aa;
            app.engine_mut().set_anti_alias(aa);
            app.mark_dirty();
        }
        AppCmd::SetOpacity(o) => {
            app.props_current.opacity = o.clamp(0.0, 1.0);
            let (v, wash) = (app.props_current.opacity, app.props_current.wash);
            let e = app.engine_mut();
            // In wash mode the slider is the STROKE opacity; Flow owns the
            // per-dab alpha (Krita's Opacity/Flow pair).
            if wash {
                e.set_wash_opacity(v);
            } else {
                e.set_base_opacity(v);
            }
            app.mark_dirty();
        }
        AppCmd::SetMinSize(pct) => {
            app.props_current.min_size = pct.clamp(0.0, 100.0);
            let v = app.props_current.min_size;
            app.engine_mut().set_size_min_pct(v);
            app.mark_dirty();
        }
        AppCmd::SetStabilizer(v) => {
            app.props_current.stabilizer = v.clamp(0.0, 1.0);
            let s = app.props_current.stabilizer;
            app.brush.set_strength(s);
            app.mark_dirty();
        }
        AppCmd::SetCorrection(c) => {
            app.props_current.correct = c.sanitized();
            let c = app.props_current.correct;
            app.brush.set_correction(c);
            app.mark_dirty();
        }
        AppCmd::SetRandomization(v) => {
            let p = &mut app.props_current;
            p.random = if p.random_abs {
                v.max(0.0)
            } else {
                v.clamp(0.0, 1.0)
            };
            let (r, m, a) = (p.random, p.random_min, p.random_abs);
            app.engine_mut().set_randomization(r, m, a);
            app.mark_dirty();
        }
        AppCmd::SetRandomMin(pct) => {
            app.props_current.random_min = pct.clamp(0.0, 100.0);
            let p = app.props_current;
            app.engine_mut()
                .set_randomization(p.random, p.random_min, p.random_abs);
            app.mark_dirty();
        }
        AppCmd::SetRandomAbs(abs) => {
            // Unit change: keep the *look* by converting the amount between
            // log-radius (≈ proportional) and px around the current radius.
            let radius = app.engine().radius_px();
            let p = &mut app.props_current;
            p.random = if abs {
                // log-units → px: a deviation of L on radius r ≈ r·L px.
                (p.random * radius).clamp(0.0, 16.0)
            } else {
                (p.random / radius.max(1.0)).clamp(0.0, 1.0)
            };
            p.random_abs = abs;
            let (r, m, a) = (p.random, p.random_min, p.random_abs);
            app.engine_mut().set_randomization(r, m, a);
            app.mark_dirty();
        }

        AppCmd::SetHardDab(on) => {
            app.props_current.hard_dab = on;
            app.engine_mut().set_hard_dab(on);
            app.mark_dirty();
        }
        AppCmd::SetScatter(v) => {
            let sc = v.clamp(0.0, 4.0);
            app.props_current.scatter = sc;
            app.engine_mut().set_scatter(sc);
            app.mark_dirty();
        }
        AppCmd::SetWash(on) => {
            app.props_current.wash = on;
            let p = app.props_current;
            // Toggling re-applies the full pair: in wash, `opacity` becomes
            // the stroke-level value and `flow` takes over the per-dab knob.
            let e = app.engine_mut();
            if on {
                e.set_flow(p.flow);
                e.set_wash(true, p.opacity, p.brush_blend);
            } else {
                e.set_wash(false, 1.0, Blend::Normal);
                e.set_base_opacity(p.opacity);
            }
            app.mark_dirty();
        }
        AppCmd::SetFlow(v) => {
            app.props_current.flow = v.clamp(0.0, 1.0);
            let f = app.props_current.flow;
            app.engine_mut().set_flow(f);
            app.mark_dirty();
        }
        AppCmd::SetBrushBlend(b) => {
            app.props_current.brush_blend = b;
            let (on, op) = (app.props_current.wash, app.props_current.opacity);
            let e = app.engine_mut();
            e.set_wash_blend(b);
            e.set_wash(on, op, b);
            app.mark_dirty();
        }
        AppCmd::SetTexture(idx) => {
            app.props_current.texture = idx.min(app.texture_names.len() as u16);
            let p = app.props_current;
            let mask = if p.texture > 0 {
                app.brushes_root.as_deref().and_then(|root| {
                    app.texture_names
                        .get(p.texture as usize - 1)
                        .and_then(|n| mn_brush::load_texture(root, n))
                })
            } else {
                None
            };
            app.engine_mut().set_texture_mask(mask);
            app.mark_dirty();
        }
        AppCmd::SetTextureScroll(v) => {
            app.props_current.texture_scroll = v.clamp(0.0, 64.0);
            let s = app.props_current.texture_scroll;
            app.engine_mut().set_texture_scroll(s);
            app.mark_dirty();
        }
        AppCmd::SetSketch(on) => {
            app.props_current.sketch = on;
            let p = app.props_current;
            app.engine_mut()
                .set_sketch(p.sketch.then_some(mn_brush::SketchParams {
                    distance: p.sketch_dist,
                    density: p.sketch_density,
                }));
            app.mark_dirty();
        }
        AppCmd::SetSketchDistance(v) => {
            app.props_current.sketch_dist = v.clamp(2.0, 500.0);
            let p = app.props_current;
            app.engine_mut()
                .set_sketch(p.sketch.then_some(mn_brush::SketchParams {
                    distance: p.sketch_dist,
                    density: p.sketch_density,
                }));
            app.mark_dirty();
        }
        AppCmd::SetSketchDensity(v) => {
            app.props_current.sketch_density = v.clamp(0.0, 1.0);
            let p = app.props_current;
            app.engine_mut()
                .set_sketch(p.sketch.then_some(mn_brush::SketchParams {
                    distance: p.sketch_dist,
                    density: p.sketch_density,
                }));
            app.mark_dirty();
        }
        AppCmd::SetCurve {
            setting,
            sensor,
            points,
        } => {
            let (cs, sn) = (
                CurveSetting::from_index(setting),
                CurveSensor::from_index(sensor),
            );
            let Some(sid) = cs.setting_id() else { return };
            let Some(iid) = sn.input_id() else { return };
            app.engine_mut().set_mapping(sid, iid, &points);
            // Session memory per sub tool, like ToolProps — the preset's own
            // file is never rewritten.
            if let Some(i) = app.selected_preset {
                let key = (setting, sensor);
                let entry = app
                    .curve_overrides
                    .entry(app.presets[i].1.clone())
                    .or_default();
                if points.is_empty() {
                    entry.remove(&key);
                } else {
                    entry.insert(key, points);
                }
            }
            app.mark_dirty();
        }

        AppCmd::SetMirrorX(on) => {
            app.mirror_x = on;
            if on {
                app.wrap_x = false;
            }
            app.rebuild_twins();
            app.mark_dirty();
        }
        AppCmd::SetMirrorY(on) => {
            app.mirror_y = on;
            if on {
                app.wrap_y = false;
            }
            app.rebuild_twins();
            app.mark_dirty();
        }
        AppCmd::SetWrapX(on) => {
            app.wrap_x = on;
            if on {
                app.mirror_x = false;
            }
            app.rebuild_twins();
            app.mark_dirty();
        }
        AppCmd::SetWrapY(on) => {
            app.wrap_y = on;
            if on {
                app.mirror_y = false;
            }
            app.rebuild_twins();
            app.mark_dirty();
        }
        AppCmd::SetGpuDabs(on) => {
            // A pure app preference, not document state: no mark_dirty, no
            // redraw — the next stroke's begin branch reads it live (the
            // audit-H1 function-of-the-branch rule), so flipping between
            // strokes is safe by construction.
            if on && !app.renderer.gpu_dabs_supported() {
                app.set_status("gpu dabs: this adapter can't — staying on the cpu path");
                return;
            }
            app.gpu_dabs = on;
            app.layout.note_gpu_dabs(on);
            app.set_status(if on {
                "gpu dabs: on — strokes rasterize on the gpu"
            } else {
                "gpu dabs: off — cpu dab path"
            });
        }
        // --- colour slots ---------------------------------------------------
        AppCmd::SetSlotColor(rgb) => {
            dispatch(app, AppCmd::SetSlotColorLive(rgb));
            crate::app::push_color_history(&mut app.color_history, rgb);
            app.note_color_history();
        }
        AppCmd::SetSlotColorLive(rgb) => {
            // Picking a colour while Transparent is active returns to Main —
            // CSP does the same (you chose a colour; you mean to draw with it).
            if app.slot == Slot::Transparent {
                app.slot = Slot::Main;
            }
            match app.slot {
                Slot::Sub => app.sub_color = rgb,
                _ => app.main_color = rgb,
            }
            app.apply_draw_state();
            app.mark_dirty();
        }
        AppCmd::ClearColorHistory => {
            app.color_history.clear();
            app.note_color_history();
            app.set_status("recent colours cleared");
            app.mark_dirty();
        }
        AppCmd::AddHistoryToSwatches => {
            let before = app.swatches.len();
            // The history stays as it was: this copies, it does not move.
            for rgb in app.color_history.clone() {
                if app.swatches.len() >= crate::app::SWATCH_CAP {
                    break;
                }
                if !app.swatches.iter().any(|s| s.rgb == rgb) {
                    app.swatches.push(mn_core::palette::Swatch::new(rgb));
                }
            }
            let n = app.swatches.len() - before;
            if n > 0 {
                crate::app::save_swatches(&app.swatches);
            }
            app.set_status(match n {
                0 => "every recent colour is already in the Color Set".to_string(),
                1 => "1 colour added to the Color Set".to_string(),
                n => format!("{n} colours added to the Color Set"),
            });
            app.mark_dirty();
        }
        AppCmd::SetSlot(s) => {
            app.slot = s;
            app.apply_draw_state();
            app.mark_dirty();
        }
        AppCmd::AddSwatch(rgb) => {
            app.swatches.push(mn_core::palette::Swatch::new(rgb));
            crate::app::save_swatches(&app.swatches);
            app.mark_dirty();
        }
        AppCmd::DeleteSwatch(i) => {
            if i < app.swatches.len() {
                app.swatches.remove(i);
                crate::app::save_swatches(&app.swatches);
            }
            app.mark_dirty();
        }
        AppCmd::ImportPalette => {}
        AppCmd::ImportPalettePath(p) => match std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|t| mn_core::palette::parse_gpl(&t))
        {
            Ok(cols) => {
                let n = cols.len();
                let name = p.file_stem().map(|s| s.to_string_lossy().into_owned());
                // Names come through: a palette whose swatches are called
                // "skin — shadow" is worthless as anonymous squares, and
                // the parser has always returned them.
                app.swatches.extend(cols);
                crate::app::save_swatches(&app.swatches);
                app.set_status(match name {
                    Some(nm) => format!("imported {n} colours from {nm}.gpl"),
                    None => format!("imported {n} colours"),
                });
                app.mark_dirty();
            }
            Err(e) => app.set_error(format!("palette import: {e}")),
        },
        AppCmd::ImportGradient => {}
        AppCmd::ImportGradientPath(p) => match std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|t| mn_core::gradient::import_ggr(&t))
        {
            Ok(mut g) => {
                g.name = app.grad_set.free_name(&g.name);
                let name = g.name.clone();
                app.grad_set.items.push(g);
                app.grad_set_sel = app.grad_set.len() - 1;
                app.layout.note_gradients(&app.grad_set.to_json());
                app.set_status(format!("imported gradient “{name}”"));
                app.mark_dirty();
            }
            Err(e) => app.set_error(format!("gradient import: {e}")),
        },
        AppCmd::SwapColors => {
            std::mem::swap(&mut app.main_color, &mut app.sub_color);
            app.apply_draw_state();
            app.mark_dirty();
        }
        AppCmd::ResetColors => {
            app.main_color = [0.0, 0.0, 0.0];
            app.sub_color = [1.0, 1.0, 1.0];
            app.apply_draw_state();
            app.mark_dirty();
        }
        AppCmd::SetTool(t) => {
            if t.enabled() {
                if t != Tool::Text {
                    app.commit_text_edit();
                }
                let old = app.tool;
                app.tool = t;
                // Pen and Eraser are separate sub tools (owner order): each
                // remembers its own brush across switches.
                if old != t {
                    match old {
                        Tool::Pen => app.pen_preset = app.selected_preset,
                        Tool::Eraser => app.eraser_preset = app.selected_preset,
                        _ => {}
                    }
                    let want = match t {
                        Tool::Pen => app.pen_preset,
                        Tool::Eraser => app.eraser_preset,
                        _ => None,
                    };
                    if let Some(i) = want {
                        if app.selected_preset != Some(i) && i < app.presets.len() {
                            let p = app.presets[i].1.clone();
                            app.push_cmd(AppCmd::SelectBrush(p));
                        }
                    }
                }
                // Owner item (2026-08-19, top of the text arc): switching
                // from Text to Object hands him the BALLOON under the
                // selected text — CSP's behaviour, the part he likes. Falls
                // back to keeping the text when no balloon contains it.
                if t == Tool::Object
                    && old == Tool::Text
                    && let Some((li, ti)) = app.text_sel
                    && let Some(c) = app
                        .doc
                        .layers
                        .get(li)
                        .and_then(|l| l.texts())
                        .and_then(|ts| ts.texts.get(ti))
                        .map(|it| it.center())
                {
                    let mut handover = None;
                    for lj in (0..app.doc.layers.len()).rev() {
                        let l = &app.doc.layers[lj];
                        if !l.visible {
                            continue;
                        }
                        if let Some(bs) = l.balloons() {
                            for bi in (0..bs.balloons.len()).rev() {
                                if bs.balloons[bi].contains(c) {
                                    handover = Some((lj, bi));
                                    break;
                                }
                            }
                        }
                        if handover.is_some() {
                            break;
                        }
                    }
                    if let Some((lj, bi)) = handover {
                        app.text_sel = None;
                        app.balloon_sel = Some((lj, bi));
                        app.object_pick = Some((c[0], c[1]));
                        app.set_status("balloon selected — O cycles the stack under it");
                    }
                }
                app.frame_drag = None;
                app.frame_poly = None;
                app.frame_pen = None;
                // L-001: a half-traced magnetic outline has no gesture left
                // to close it once the tool changes — and it holds an edge
                // cache, so dropping it frees that too.
                app.magnetic = None;
                app.object_drag = None;
                app.text_gesture = None;
                app.text_obj_drag = None;
                if t != Tool::Object {
                    app.object_sel = None;
                    app.text_sel = None;
                }
                app.apply_draw_state();
                app.mark_dirty();
            }
        }

        AppCmd::ObjectCycle(forward) => app.object_cycle(forward),
        AppCmd::SetLayerEyeSolo(i) => {
            if app.doc.only_visible(i) && app.eye_solo_backup.is_some() {
                let b = app.eye_solo_backup.take().unwrap();
                app.doc.restore_visibility(&b);
                app.set_status("visibility restored");
            } else if let Some(b) = app.doc.set_layer_visibility_solo(i) {
                app.eye_solo_backup = Some(b);
                app.set_status("solo — Alt+click the eye again to restore");
            } else {
                app.set_status("no such layer");
            }
            app.mark_dirty();
        }
        AppCmd::ToggleHud => {
            app.hud_open = !app.hud_open;
            app.mark_dirty();
        }
        AppCmd::OpenManual => match manual_path() {
            Some(p) => unsafe { crate::win32::shell_open(&p) },
            None => app.set_status(
                "manual not found — docs/manual/ lives beside the exe (manual/index.html)",
            ),
        },

        // --- selection + fill -----------------------------------------------
        AppCmd::SetSelectMode(m) => {
            app.select_mode = m;
            // Leaving Magnetic mid-trace would leave an orphan outline on
            // the overlay with no gesture left to close it.
            app.magnetic = None;
            if m == SelectMode::Magnetic {
                app.set_status(
                    "magnetic lasso: trace along the lineart — Backspace undoes an anchor, Enter closes",
                );
            }
            app.mark_dirty();
        }
        AppCmd::Deselect => {
            if let Some(s) = app.doc.selection.take() {
                // Ctrl+Shift+D brings it back.
                app.last_selection = Some(s);
                app.doc.touch();
                app.mark_dirty();
            }
        }
        AppCmd::SelectAll => {
            app.doc.selection = Some(mn_core::Selection::all(&app.doc));
            app.doc.touch();
            app.set_status("all selected");
            app.mark_dirty();
        }
        AppCmd::SelectInvert => match app.doc.selection.take() {
            Some(s) => {
                let inv = s.inverted(&app.doc);
                app.doc.selection = (!inv.is_empty()).then_some(inv);
                app.doc.touch();
                app.set_status("selection inverted");
                app.mark_dirty();
            }
            None => app.set_status("nothing selected to invert"),
        },
        AppCmd::SelectBlur(px) => match app.doc.selection.take() {
            Some(s) => {
                let b = s.blur(&app.doc, px);
                // A blur wide enough to push EVERY pixel under half leaves
                // a live selection with no ants and no launcher — it still
                // masks the brush at partial strength, so say so rather
                // than let the canvas go quietly unpaintable.
                let hidden = !b.is_empty() && !b.has_visible_outline();
                app.doc.selection = Some(b);
                app.doc.touch();
                if hidden {
                    app.set_error(format!(
                        "blurred by {px} px: coverage is under 50% everywhere, so the marching ants are hidden — the selection still masks painting at partial strength (Ctrl+D clears it)"
                    ));
                } else {
                    app.set_status(format!("selection border blurred by {px} px"));
                }
                app.mark_dirty();
            }
            None => app.set_status("nothing selected to blur"),
        },
        AppCmd::SelectExpand(px) => match app.doc.selection.take() {
            Some(s) => {
                app.doc.selection = Some(s.grow(&app.doc, px));
                app.doc.touch();
                app.set_status(format!("selection expanded by {px} px"));
                app.mark_dirty();
            }
            None => app.set_status("nothing selected to expand"),
        },
        AppCmd::SelectShrink(px) => match app.doc.selection.take() {
            Some(s) => {
                let e = s.shrink(&app.doc, px);
                let gone = e.is_empty();
                app.doc.selection = (!gone).then_some(e);
                app.doc.touch();
                app.set_status(if gone {
                    "selection shrunk out of existence".to_string()
                } else {
                    format!("selection shrunk by {px} px")
                });
                app.mark_dirty();
            }
            None => app.set_status("nothing selected to shrink"),
        },
        AppCmd::Reselect => match app.last_selection.take() {
            Some(s) => {
                app.doc.selection = Some(s);
                app.doc.touch();
                app.set_status("reselected");
                app.mark_dirty();
            }
            None => app.set_status("no previous selection"),
        },
        AppCmd::FillSelection => {
            // NL-006's live switch (TRIAGE 137): with the Tool Property's
            // "live layer" on, Fill targets the live model — retargeting the
            // active live layer when there is one, else creating a new one.
            if app.fill_live {
                let color = app.active_color();
                let kind = mn_core::FillKind::Flat {
                    color: [color[0], color[1], color[2], 1.0],
                };
                let li = app.doc.active;
                if matches!(app.doc.layers[li].kind, mn_core::LayerKind::Fill(_)) {
                    app.push_cmd(AppCmd::SetFillParams(li, kind));
                } else {
                    app.push_cmd(AppCmd::NewLiveFill(kind));
                }
                return;
            }
            app.doc.set_op_label("Fill");
            let color = app.active_color();
            if app.doc.fill_selection(color) {
                app.set_status(if app.doc.selection.is_some() {
                    "selection filled"
                } else {
                    "layer filled"
                });
            } else {
                app.set_status("this layer cannot be filled (vector/folder/locked)");
            }
            app.mark_dirty();
        }
        AppCmd::SelectFromLayer(i, op) => {
            app.refresh_tones();
            let s = mn_core::Selection::from_layer_alpha(&app.doc, i);
            if s.is_empty() {
                app.set_status("that layer has no opaque pixels to select");
                app.mark_dirty();
                return;
            }
            let combined = match &app.doc.selection {
                Some(cur) if op != mn_core::SelectionOp::Replace => cur.combine(&s, &app.doc, op),
                _ => s,
            };
            if combined.is_empty() {
                app.doc.selection = None;
                app.set_status("selection combined away — deselected");
            } else {
                app.doc.selection = Some(combined);
                app.set_status("selected the layer's opacity");
            }
            app.doc.touch();
            app.mark_dirty();
        }
        AppCmd::ClearOutside => {
            app.doc.set_op_label("Clear outside");
            if app.doc.selection.is_none() {
                app.set_status("no selection — everything would go");
            } else if app.doc.clear_outside_selection() {
                app.set_status("cleared outside the selection");
            } else {
                app.set_status("this layer cannot be cleared (vector/folder/locked)");
            }
            app.mark_dirty();
        }
        AppCmd::MagicSelectPath { pts, op } => {
            app.refresh_tones();
            let seeds = subsample_path(&pts, 4.0);
            match mn_core::fill::magic_select_path(&app.doc, &seeds, &app.wand_opts) {
                Some((s, floods)) if !s.is_empty() => {
                    let combined = match &app.doc.selection {
                        Some(cur) if op != mn_core::SelectionOp::Replace => {
                            cur.combine(&s, &app.doc, op)
                        }
                        _ => s,
                    };
                    if combined.is_empty() {
                        app.doc.selection = None;
                        app.doc.touch();
                        app.set_status("selection subtracted away — deselected");
                    } else {
                        app.doc.selection = Some(combined);
                        app.doc.touch();
                        app.set_status(format!(
                            "{floods} closed areas selected — G fills them, Delete clears"
                        ));
                    }
                }
                _ => app.set_status("drag across the empty space inside the drawing"),
            }
            app.mark_dirty();
        }
        AppCmd::MagicSelect(x, y, op) => {
            app.refresh_tones();
            match mn_core::fill::magic_select(&app.doc, (x as i32, y as i32), &app.wand_opts) {
                Some(s) if !s.is_empty() => {
                    let combined = match &app.doc.selection {
                        Some(cur) if op != mn_core::SelectionOp::Replace => {
                            cur.combine(&s, &app.doc, op)
                        }
                        _ => s,
                    };
                    if combined.is_empty() {
                        // Subtracted away to nothing: an empty Selection
                        // means "everything", so deselect instead.
                        app.doc.selection = None;
                        app.doc.touch();
                        app.set_status("selection subtracted away — deselected");
                    } else {
                        app.doc.selection = Some(combined);
                        app.doc.touch();
                        app.set_status("area selected — G fills it, Delete clears it");
                    }
                }
                _ => app.set_status("nothing to select there"),
            }
            app.mark_dirty();
        }
        AppCmd::PickColor(x, y) => {
            app.refresh_tones();
            let (xi, yi) = (x as i32, y as i32);
            let opts = app.eyedrop_opts;
            match pick_color(&app.doc, xi, yi, opts) {
                Some([r, g, b]) => {
                    let rgb = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0];
                    dispatch(app, AppCmd::SetSlotColor(rgb));
                    let tail = register_picked(app, rgb);
                    // Name the mode when it is not the plain one: a 5×5
                    // average that silently disagrees with the pixel under
                    // the pen is the kind of thing you blame on the tool.
                    let mut note = String::new();
                    if opts.size > 1 {
                        note += &format!(" ({0}×{0} average)", opts.size);
                    }
                    if opts.refer == mn_core::FillRefer::Reference
                        && app.doc.reference_layers().is_empty()
                    {
                        note += " (no reference layer marked — took what you see)";
                    }
                    app.set_status(format!("picked #{r:02x}{g:02x}{b:02x}{note}{tail}"));
                }
                None => app.set_status("outside the canvas"),
            }
        }
        AppCmd::Fill(x, y) => {
            app.refresh_tones();
            let color = app.active_color();
            let opts = app.fill_opts;
            let n = mn_core::fill::bucket_fill(&mut app.doc, (x as i32, y as i32), color, &opts);
            if n > 0 {
                app.set_status(format!("filled {n} px"));
            }
            app.mark_dirty();
        }
        AppCmd::SetFillOpts(o) => {
            app.fill_opts = o;
        }
        AppCmd::SetWandOpts(o) => {
            app.wand_opts = o;
        }
        AppCmd::StampVisible => {
            app.commit_text_edit();
            app.refresh_tones();
            let img = mn_core::export::composite(&app.doc, mn_core::Background::Transparent);
            let n = app.doc.layers.len() + 1;
            app.doc.add_layer_from_image(format!("Merged {n}"), &img);
            app.renderer.invalidate();
            app.set_status("visible layers stamped onto a new layer");
            app.mark_dirty();
        }
        AppCmd::LayerAbove => {
            let i = app.doc.active + 1;
            if app.doc.set_active(i) {
                app.mark_dirty();
            }
        }
        AppCmd::LayerBelow => {
            let i = app.doc.active.wrapping_sub(1);
            if app.doc.set_active(i) {
                app.mark_dirty();
            }
        }
        AppCmd::ImportImage => {}
        AppCmd::ImportImagePath(p) => match image::open(&p) {
            Ok(img) => {
                let img = img.to_rgba8();
                let name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Imported".to_owned());
                let (iw, ih) = (img.width(), img.height());
                // IO-043: a selection turns the import into a masked
                // import. Both import routes — File ▸ Import ▸ Image and a
                // dropped file — arrive here, which is why the rule holds
                // for every route without either of them knowing about it.
                let (_, masked) = app.doc.add_layer_from_image_masked(name, &img);
                app.renderer.invalidate();
                app.set_status(if masked {
                    format!("imported {iw}x{ih} — masked to the selection (delete the mask to see it all)")
                } else {
                    format!("imported {iw}x{ih} as a layer")
                });
            }
            Err(e) => app.set_error(format!("import failed: {e}")),
        },

        // --- view -----------------------------------------------------------
        AppCmd::ZoomFit => app.fit_to_view(),
        AppCmd::Zoom100 => {
            let c = app.canvas_center();
            let z = app.viewport.zoom;
            if z > 0.0 {
                app.viewport.zoom_around(c, 1.0 / z);
            }
            app.mark_dirty();
        }
        AppCmd::ZoomStep(f) => {
            let c = app.canvas_center();
            app.viewport.zoom_around(c, f);
            app.mark_dirty();
        }
        AppCmd::RotateView(d) => {
            let c = app.canvas_center();
            app.viewport.rotate_around(c, d);
            app.mark_dirty();
        }
        AppCmd::RotateReset => {
            let c = app.canvas_center();
            app.viewport.set_rotation_around(c, 0.0);
            app.mark_dirty();
        }
        AppCmd::RotateFlipReset => {
            let c = app.canvas_center();
            // Each flip is a TOGGLE, so it is only sent when that axis is
            // flipped; unflipping also mirrors the rotation, so they go
            // first and the absolute rotation reset lands on top.
            if app.viewport.flip_h {
                app.viewport.flip_around(c);
            }
            if app.viewport.flip_v {
                app.viewport.flip_v_around(c);
            }
            app.viewport.set_rotation_around(c, 0.0);
            app.set_status("view reset — upright and unmirrored");
            app.mark_dirty();
        }
        AppCmd::ViewReset => {
            let c = app.canvas_center();
            if app.viewport.flip_h {
                app.viewport.flip_around(c);
            }
            if app.viewport.flip_v {
                app.viewport.flip_v_around(c);
            }
            app.viewport.set_rotation_around(c, 0.0);
            app.fit_to_view();
            app.set_status("view reset — upright, unmirrored, fitted");
            app.mark_dirty();
        }
        AppCmd::SetGuidesHidden(hidden) => {
            app.layout.note_guides_hidden(hidden);
            app.set_status(if hidden {
                "crop marks and margins hidden — the page is unchanged"
            } else {
                "crop marks and margins shown"
            });
            app.mark_dirty();
        }
        AppCmd::TransformReset => {
            if let Some(drag) = &mut app.transform_drag {
                drag.reset();
                app.set_status("transform reset — still transforming");
                app.mark_dirty();
            }
        }
        AppCmd::SetToolLock(on) => {
            app.props_current.locked = on;
            // Locking TAKES the snapshot: whatever is on the sliders now is
            // what returning to this sub tool restores. Unlocking writes
            // too, so today's drift becomes the new normal on the way out
            // instead of being silently thrown away by the next switch.
            app.snapshot_current_props();
            app.set_status(if on {
                "tool settings locked — change them freely; they come back when you return to this sub tool"
            } else {
                "tool settings unlocked — the values on the sliders are now this sub tool's own"
            });
            app.mark_dirty();
        }
        AppCmd::FlipView => {
            let c = app.canvas_center();
            app.viewport.flip_around(c);
            app.set_status(flip_status(&app.viewport));
            app.mark_dirty();
        }
        AppCmd::FlipViewV => {
            let c = app.canvas_center();
            app.viewport.flip_v_around(c);
            app.set_status(flip_status(&app.viewport));
            app.mark_dirty();
        }
        // --- per-layer effects (TRIAGE 21/27/30) ---------------------------
        AppCmd::SetEdge(i, edge) => {
            if app.doc.set_edge(i, edge) {
                // The derived outline (or its absence) is newer than anything
                // the GPU cache holds for this layer — and the effect writes
                // into tiles the layer never painted, so a stale cache would
                // leave a ring floating with nothing inside it.
                app.renderer.evict_layer(i);
                app.refresh_tones();
                app.set_status(match edge {
                    Some(e) => format!(
                        "border effect — {:.1} px outline round the layer's own alpha; the painted pixels are untouched",
                        e.width()
                    ),
                    None => "border effect off — the drawing is exactly as it was".to_string(),
                });
                app.mark_dirty();
            } else if app.doc.layers.get(i).is_some_and(|l| l.folder) {
                app.set_status("folders have no alpha of their own to outline");
            }
        }
        AppCmd::SetLayerSubColour(i, c) => {
            if app.doc.set_layer_sub_colour(i, c) {
                app.mark_dirty();
            }
        }
        AppCmd::SetLayerExpression(i, e) => {
            if app.doc.set_layer_expression(i, e) {
                app.set_status(match e {
                    mn_core::LayerExpression::Colour => "layer displayed in colour",
                    mn_core::LayerExpression::Grey => "layer previewed as grey — display only",
                    mn_core::LayerExpression::Mono => {
                        "layer previewed as 1-bit mono — display only, nothing is converted"
                    }
                });
                app.mark_dirty();
            }
        }

        // --- paper (PA-001) -------------------------------------------------
        AppCmd::PaperToggle => {
            let on = !app.doc.paper.visible;
            if app.doc.set_paper_visible(on) {
                app.set_status(if on {
                    "paper shown"
                } else {
                    "paper hidden — the checker is where the page is transparent (a check; export is unaffected)"
                });
                app.mark_dirty();
            }
        }
        AppCmd::SetPaperColour(c) => {
            if app.doc.set_paper_colour(c) {
                let [r, g, b] = c;
                app.set_status(format!(
                    "paper #{r:02x}{g:02x}{b:02x} — the page exports on it"
                ));
                app.mark_dirty();
            }
        }
    }

    // The Pages palette follows the document (manga ⇒ present, plain image ⇒
    // closed) — after the command, so new/open/add/delete page all reconcile.
    app.sync_pages_palette();
}

/// CO-023, the eyedropper's half of the Color Set: when the user has asked
/// for it, a picked colour joins the set. Returns the tail to hang on the
/// pick's status line, so the palette never grows without saying so.
///
/// Three things keep this from turning the set into landfill. It is OFF by
/// default (the Recent strip already remembers picks, and forgets them
/// again — that is the right home for automatic colours). It de-duplicates,
/// so sampling the same ink twenty times adds one swatch. And it stops at
/// [`crate::app::SWATCH_CAP`] rather than growing without end — the `+`
/// button and a `.gpl` import are deliberate acts and are never refused,
/// but nothing that happens behind the user gets to fill his palette.
fn register_picked(app: &mut App, rgb: [f32; 3]) -> &'static str {
    use crate::app::PickReg;
    match crate::app::pick_registration(app.layout.auto_swatch, &app.swatches, rgb) {
        PickReg::Off => "",
        PickReg::Duplicate => " — already in the Color Set",
        PickReg::Full => " — Color Set full, not added",
        PickReg::Added => {
            app.swatches
                .push(mn_core::palette::Swatch::new(mn_core::palette::quantize8(
                    rgb,
                )));
            crate::app::save_swatches(&app.swatches);
            " — added to the Color Set"
        }
    }
}

/// The eyedropper's whole sample (E-014 + E-016): the box the size covers,
/// taken from the layers the Reference row names, averaged in linear light.
/// `None` when the pick itself is off-canvas.
///
/// Also called from the overlay each paint to colour the picker ring (E-017),
/// so it must stay a per-pick cost — the composite branch is one tile walk for
/// the whole box, the other two are direct tile reads.
pub(crate) fn pick_color(
    doc: &mn_core::Document,
    x: i32,
    y: i32,
    opts: EyedropOpts,
) -> Option<[u8; 3]> {
    use mn_core::FillRefer;
    let (x0, y0, w, h) = mn_core::export::sample_box(doc.size, x, y, opts.size)?;
    // The reference SET (RF-001), even where the layers' own eyes are off.
    // Nothing marked: fall back to what you see, exactly as the fill tool
    // does (`fill::flood_region`) — a silent empty pick would be worse.
    let refs = match opts.refer {
        FillRefer::Reference => doc.reference_layers(),
        _ => Vec::new(),
    };
    if opts.refer == FillRefer::All || (opts.refer == FillRefer::Reference && refs.is_empty()) {
        return mn_core::export::composite_pixel_avg(doc, x, y, opts.size);
    }
    let mut samples = Vec::with_capacity((w * h) as usize);
    for py in y0..y0 + h as i32 {
        for px in x0..x0 + w as i32 {
            samples.push(match opts.refer {
                FillRefer::Active => layer_pixel_over_white(doc.active_layer(), px, py),
                _ => layers_pixel_over_white(doc, &refs, px, py),
            });
        }
    }
    mn_core::export::average_srgb(&samples)
}

/// One layer's own colour at a canvas pixel, over white (the eyedropper's
/// "pick from layer" sub tool). Bounds are the caller's business —
/// `sample_box` has already clipped the box to the canvas.
fn layer_pixel_over_white(layer: &mn_core::Layer, x: i32, y: i32) -> [u8; 3] {
    let idx = mn_core::TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    let p = layer
        .display_tile(idx)
        .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
        .unwrap_or([0; 4]);
    // Premultiplied fix15 over white.
    let a = p[3] as u32;
    let ch = |c: u16| -> u8 {
        let v = c as u32 + (32768 - a);
        ((v.min(32768) * 255 + 16384) / 32768) as u8
    };
    [ch(p[0]), ch(p[1]), ch(p[2])]
}

/// The reference SET at one canvas pixel, composited bottom→top over white —
/// the single-pixel twin of `fill.rs`'s canvas-sized `layers_over_white`, so
/// the eyedropper and the fill tool sample the same stack the same way.
/// `indices` must be in stack order (bottom first), which is what
/// `Document::reference_layers` returns.
fn layers_pixel_over_white(doc: &mn_core::Document, indices: &[usize], x: i32, y: i32) -> [u8; 3] {
    let idx = mn_core::TileIdx::of_pixel(x, y);
    let (ox, oy) = idx.origin();
    // Accumulate in fix15 straight RGB on white paper, quantize once.
    let mut acc = [32768u32; 3];
    for &li in indices {
        let Some(layer) = doc.layers.get(li) else {
            continue;
        };
        let Some(tile) = layer.display_tile(idx) else {
            continue;
        };
        let p = tile.pixel((x - ox) as usize, (y - oy) as usize);
        let inv = 32768 - p[3] as u32;
        for c in 0..3 {
            acc[c] = p[c] as u32 + acc[c] * inv / 32768;
        }
    }
    std::array::from_fn(|c| ((acc[c] * 255 + 16384) / 32768) as u8)
}

/// Where the manual lives, for Help ▸ Manual: `manual/index.html` beside
/// the running exe (the shipped layout); a dev build falls back to the
/// repository's docs/manual via the compiled manifest dir.
pub(crate) fn manual_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let shipped = exe.parent()?.join("manual").join("index.html");
    if shipped.exists() {
        return Some(shipped);
    }
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual/index.html");
    dev.exists().then_some(dev)
}
