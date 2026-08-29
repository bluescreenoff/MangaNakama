//! Tool/command icons drawn with the egui painter, on a unit square.
//!
//! Not glyphs: the bundled egui fonts carry no pictographs, so `👁`/`◉`/`✕` come
//! out as tofu on this machine (verified in `--screenshot` when the layer eye
//! was first tried). Everything here is font-independent geometry, which also
//! means it stays crisp at any DPI.

use super::theme::{self, Theme};
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Pen,
    Eraser,
    Fill,
    Select,
    /// Selection ▸ Selection pen (CSP 選択ペン): the marquee's corner Ls with
    /// a pen nib inside — the stroke paints selection, not ink.
    SelPen,
    /// Selection ▸ Erase selection (CSP 選択消し): the same marquee corners
    /// with an eraser slab inside.
    SelEraser,
    /// Object tool: transform-handle box (CSP Operation ▸ Object).
    Object,
    /// Frame (koma) divide tool: a panelled page.
    Frame,
    /// Speech balloon tool: a bubble with a tail.
    Balloon,
    /// Text tool: a capital A over a baseline.
    Text,
    /// Auto select (magic wand): a wand with sparkles.
    Wand,
    /// Eyedropper: a pipette at 45°.
    Eyedrop,
    /// Liquify (CSP 液化): two straight rails with a warped line
    /// between them — the grid-bend read of a warp tool.
    Liquify,
    Pan,
    Undo,
    Redo,
    RotateLeft,
    RotateRight,
    RotateReset,
    FlipH,
    FlipV,
    Eye,
    EyeOff,
    Plus,
    Duplicate,
    Trash,
    /// Layer palette-colour label chip.
    Label,
    /// Clip to layer below (CSP) — placeholder until masks land.
    Clip,
    /// Lock layer.
    Lock,
    /// Transparent-pixel lock: checkerboard with a small padlock.
    LockAlpha,
    /// Layer folder — placeholder until folders land.
    Folder,
    /// Merge with layer below.
    MergeDown,
    Wrench,
    /// Pop a docked section out into a floating window (or back).
    Swap,
    /// Palette-header collapse state.
    ZoomIn,
    ZoomOut,
    /// Fit page to window.
    ZoomFit,
    /// 100% zoom.
    Zoom100,
    Close,
    /// The reader: an open book (owner top item 2026-08-18).
    Book,
    /// Selection launcher: deselect (dashed rect).
    SelDeselect,
    /// Selection launcher: invert (half-filled square).
    SelInvert,
    /// Selection launcher: expand (arrows pushing outward).
    SelExpand,
    /// Selection launcher: shrink (arrows pressing inward).
    SelShrink,
    /// Selection launcher: clear outside (selection box, X beyond it).
    SelClearOutside,
    /// Selection launcher: move/transform (4-way arrows around a box).
    SelTransform,
    /// Selection launcher: crop the canvas to the selection.
    SelCrop,
    /// Selection launcher: cut the selection to the clipboard.
    SelCut,
    /// Selection launcher: copy the selection to the clipboard.
    SelCopy,
    /// Selection launcher: paste the clipboard as a float.
    SelPaste,
    /// Selection launcher: allow drawing outside the selection (a stroke
    /// escaping the selection box).
    SelDrawOutside,
    /// Layer flags: reference layer (what fill/wand refer to).
    Reference,
    /// Layer flags: draft layer (red-X, CSP 下書き).
    Draft,
    /// Layers palette: the row-filter funnel toggle.
    Funnel,
    /// Figure tool: a diagonal straight line between two endpoint dots.
    Figure,
    /// Figure ▸ rectangle.
    Rect,
    /// Figure ▸ ellipse.
    Ellipse,
    /// Figure ▸ polygon (click vertices).
    Poly,
    /// Figure ▸ curve (row 157 / `FG-002`): the two-stage arc — the straight
    /// baseline you drag, ghosted, with the bend it becomes over it.
    Arc,
    /// Gradient tool: a ramp square.
    Gradient,
    /// Vector layer (docs/VECTOR-INKING.md): a curve with its control points
    /// — the strokes are geometry beside the pixels.
    Vector,
    /// Tone layer (CSP トーンレイヤー): a halftone patch, dots ramping.
    Tone,
    /// The paper under the stack (PA-001): a sheet with a folded corner.
    Paper,
    /// File object (row 166): a sheet with an arrow coming INTO it — the
    /// picture arrives from outside and keeps arriving.
    FileObject,
    /// An OPEN layer folder (the palette rows' disclosure state — CSP
    /// draws open and closed folders differently and the owner asked for
    /// the same).
    FolderOpen,
    /// Frame border folder: a folder wearing panel divisions — the special
    /// icon CSP gives koma folders in the layer list.
    FrameFolder,
    /// New raster layer: a sheet with a corner plus. The bare `Plus` said
    /// nothing about WHAT it made (owner 2026-08-21); every palette
    /// "make one" button now wears its subject.
    NewLayer,
    /// New vector layer: the recorded-curve sheet with a corner plus.
    NewVector,
    /// New folder, corner plus.
    NewFolder,
    /// New frame border folder, corner plus.
    NewFrameFolder,
    /// Auto Actions: run the stored sequence (a play triangle).
    Play,
    /// Auto Actions: arm recording (a filled dot).
    Record,
    /// Auto Actions: stop recording (a filled square).
    Stop,
    /// Drag handle: two columns of grip dots — "this row moves".
    Grip,
    /// Figure ▸ Stream line (流線): three parallel motion strokes.
    StreamLines,
    /// Figure ▸ Saturated line (集中線): rays converging on an empty centre.
    FocusLines,
    /// Figure ▸ Sea urchin / Solid flash (ウニフラッシュ): a spiky burst.
    UrchinFlash,
    /// Materials palette: a pattern/texture material — 2×2 checker.
    Pattern,
    /// Materials palette: a 3D-pose material — a posing stick mannequin.
    Pose3d,
}

// --- accent roles --------------------------------------------------------

/// What an icon is *about*, in the seven hues `Theme` carries (owner order
/// 2026-08-21: coloured icons everywhere, subtle, with an off switch).
///
/// Seven and not seventy: the hues are a CATEGORY code, so any one palette
/// shows two or three of them at once and the eye can use them. The Auto
/// Action blocks draw from the same seven, so the app reads as one system.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconRole {
    Create,
    Destroy,
    Media,
    Ink,
    Select,
    Layer,
    Nav,
}

impl IconRole {
    /// This role's hue in `t` — the only place a role becomes a colour.
    pub fn hue(self, t: Theme) -> Color32 {
        match self {
            Self::Create => t.hue_create,
            Self::Destroy => t.hue_destroy,
            Self::Media => t.hue_media,
            Self::Ink => t.hue_ink,
            Self::Select => t.hue_select,
            Self::Layer => t.hue_layer,
            Self::Nav => t.hue_nav,
        }
    }
}

impl Icon {
    /// The hue family this glyph belongs to, or `None` for the glyphs that
    /// stay grey.
    ///
    /// **No wildcard arm, on purpose.** This match is the tripwire: a new
    /// `Icon` variant fails to COMPILE until someone has made the taste
    /// decision, which is stronger than any test could be.
    ///
    /// The `None` list is as deliberate as the coloured one. The eye column,
    /// the drag grip and the row locks repeat once per layer row — colour
    /// there is noise, not information. Reference/Draft already carry their
    /// own `ref_mark`/`draft_mark` red and blue. The Auto Action transport
    /// (play/record/stop) has the `rec` token. And the eraser is one solid
    /// slab with no sub-shape to accent: an all-amber slab is not subtle.
    pub fn accent_role(self) -> Option<IconRole> {
        use IconRole::*;
        Some(match self {
            // Marks on the page.
            Self::Pen
            | Self::Fill
            | Self::Text
            | Self::Balloon
            | Self::Frame
            | Self::Eyedrop
            | Self::Figure
            | Self::Rect
            | Self::Ellipse
            | Self::Poly
            | Self::Arc
            | Self::Gradient
            | Self::Liquify
            | Self::Tone
            | Self::StreamLines
            | Self::FocusLines
            | Self::UrchinFlash => Ink,
            // The selection family, launcher included.
            Self::Select
            | Self::SelPen
            | Self::SelEraser
            | Self::Wand
            | Self::Object
            | Self::SelDeselect
            | Self::SelInvert
            | Self::SelExpand
            | Self::SelShrink
            | Self::SelTransform
            | Self::SelCrop
            | Self::SelDrawOutside => Select,
            // Making something that was not there before.
            Self::Plus
            | Self::Duplicate
            | Self::NewLayer
            | Self::NewVector
            | Self::NewFolder
            | Self::NewFrameFolder => Create,
            // …and unmaking it. Two only: red is the loudest hue here, so it
            // is spent on the bin and on the one launcher op that erases.
            Self::Trash | Self::SelClearOutside => Destroy,
            // Files, clipboard, the reader, bank assets.
            Self::Book
            | Self::SelCut
            | Self::SelCopy
            | Self::SelPaste
            | Self::Pattern
            // A file object IS a file on the page — it belongs with the
            // other "content that came from outside" glyphs, not with the
            // layer kinds it sits beside in the palette.
            | Self::FileObject
            | Self::Pose3d => Media,
            // Layer kinds and layer-to-layer ops.
            Self::Folder
            | Self::FolderOpen
            | Self::FrameFolder
            | Self::Vector
            | Self::Paper
            | Self::MergeDown
            | Self::Clip => Layer,
            // Moving the eye, not the artwork.
            Self::Pan
            | Self::ZoomIn
            | Self::ZoomOut
            | Self::ZoomFit
            | Self::Zoom100
            | Self::RotateLeft
            | Self::RotateRight
            | Self::RotateReset
            | Self::FlipH
            | Self::FlipV => Nav,
            // Monochrome — see the doc comment above.
            Self::Eraser
            | Self::Undo
            | Self::Redo
            | Self::Eye
            | Self::EyeOff
            | Self::Label
            | Self::Lock
            | Self::LockAlpha
            | Self::Wrench
            | Self::Swap
            | Self::Close
            | Self::Reference
            | Self::Draft
            | Self::Funnel
            | Self::Play
            | Self::Record
            | Self::Stop
            | Self::Grip => return None,
        })
    }
}

/// The master switch (`icon_colours=` in `prefs.txt`). A global rather than a
/// threaded parameter for the same reason [`theme::c`] is one: every painting
/// function in the app would otherwise grow an argument it does not care
/// about, and immediate mode repaints from scratch anyway.
static ACCENTS: AtomicBool = AtomicBool::new(true);

/// Turn icon accents on or off, live. Called from `Prefs::load` at startup
/// and from the Preferences checkbox.
pub fn set_accents(on: bool) {
    ACCENTS.store(on, Ordering::Relaxed);
}

pub fn accents_on() -> bool {
    ACCENTS.load(Ordering::Relaxed)
}

/// The accent `icon` should be painted with right now: `None` means "draw it
/// monochrome", which is both the toggle-off answer and the answer for every
/// glyph that has no role.
///
/// This — not the painter — is what the toggle is tested through.
pub fn accent_for(icon: Icon) -> Option<Color32> {
    accent_of(icon, accents_on(), theme::c())
}

/// [`accent_for`] with its two globals passed in, so the rule is testable
/// without touching process-wide state.
fn accent_of(icon: Icon, on: bool, t: Theme) -> Option<Color32> {
    if !on {
        return None;
    }
    icon.accent_role().map(|r| r.hue(t))
}

/// Paint `icon` to fill `r` (which should be square-ish), in `color`.
///
/// Monochrome. Callers that want the accent go through [`paint_role`] — or,
/// better, through `widgets::icon_btn` / `widgets::paint_icon`, which consult
/// the toggle for them.
pub fn paint(p: &Painter, r: Rect, icon: Icon, color: Color32) {
    paint_role(p, r, icon, color, None);
}

/// [`paint`] with one detail of the glyph lifted into `accent`.
///
/// The silhouette stays `base`: what takes the accent is the plus badge, the
/// folder tab, the arrowhead, the marker line — the part that says what the
/// icon DOES. The exceptions are the glyphs that are a single thin outline or
/// a scatter of small dots, where tinting the whole mark is what reads (a
/// dashed marquee, a halftone patch); each match arm below decides for
/// itself. `accent: None` is exactly today's monochrome paint.
pub fn paint_role(p: &Painter, r: Rect, icon: Icon, base: Color32, accent: Option<Color32>) {
    let color = base;
    let w = r.width().min(r.height());
    let line = Stroke::new((w * 0.11).clamp(1.0, 2.2), color);
    let thin = Stroke::new((w * 0.075).clamp(0.8, 1.4), color);
    // `a` falls back to the base colour, so every arm below can be written
    // once and simply draws grey-on-grey when accents are off.
    let a = accent.unwrap_or(base);
    let a_line = Stroke::new(line.width, a);
    let a_thin = Stroke::new(thin.width, a);

    match icon {
        Icon::Pen => {
            // Nib triangle at the bottom-left, barrel up to the top-right.
            // The nib is the ink: barrel grey, tip coloured.
            p.add(Shape::convex_polygon(
                poly(r, &[(0.08, 0.92), (0.40, 0.78), (0.24, 0.60)]),
                a,
                Stroke::NONE,
            ));
            p.line(poly(r, &[(0.30, 0.70), (0.88, 0.12)]), line);
        }
        Icon::Eraser => {
            p.add(Shape::convex_polygon(
                poly(r, &[(0.10, 0.64), (0.46, 0.16), (0.90, 0.44), (0.54, 0.92)]),
                color,
                Stroke::NONE,
            ));
        }
        Icon::Fill => {
            // Tilted bucket + a drop falling off the lip.
            p.add(Shape::convex_polygon(
                poly(r, &[(0.46, 0.10), (0.84, 0.48), (0.46, 0.86), (0.08, 0.48)]),
                color,
                Stroke::NONE,
            ));
            p.circle_filled(pt(r, 0.90, 0.74), w * 0.10, a);
        }
        Icon::Select => {
            let pts = poly(
                r,
                &[
                    (0.12, 0.18),
                    (0.88, 0.18),
                    (0.88, 0.82),
                    (0.12, 0.82),
                    (0.12, 0.18),
                ],
            );
            // A marquee IS its dashes — the whole thin outline takes the hue.
            p.extend(Shape::dashed_line(&pts, a_thin, w * 0.16, w * 0.13));
        }
        Icon::SelPen | Icon::SelEraser => {
            // Shared base, so the pair reads as one family: the marquee's
            // four corner Ls (the same shorthand `SelDeselect` uses), hue on
            // the marquee because the marquee is the selection. The TOOL
            // sitting inside stays base-coloured — it is the create-type,
            // not the subject.
            let q = rect(r, 0.05, 0.05, 0.95, 0.95);
            let (s, e) = (q.min, q.max);
            let (kx, ky) = (q.width() * 0.30, q.height() * 0.30);
            for (from, to) in [
                (s, Pos2::new(s.x + kx, s.y)),
                (s, Pos2::new(s.x, s.y + ky)),
                (Pos2::new(e.x - kx, s.y), Pos2::new(e.x, s.y)),
                (Pos2::new(e.x, s.y), Pos2::new(e.x, s.y + ky)),
                (Pos2::new(e.x, e.y - ky), e),
                (Pos2::new(e.x - kx, e.y), e),
                (Pos2::new(s.x, e.y - ky), Pos2::new(s.x, e.y)),
                (Pos2::new(s.x, e.y), Pos2::new(s.x + kx, e.y)),
            ] {
                p.line_segment([from, to], a_thin);
            }
            if icon == Icon::SelPen {
                // `Icon::Pen`'s nib + barrel, shrunk to sit inside the box.
                p.add(Shape::convex_polygon(
                    poly(r, &[(0.22, 0.78), (0.44, 0.68), (0.34, 0.52)]),
                    color,
                    Stroke::NONE,
                ));
                p.line(poly(r, &[(0.38, 0.61), (0.76, 0.24)]), line);
            } else {
                // `Icon::Eraser`'s slab at ~60%, so the two sit at one weight.
                p.add(Shape::convex_polygon(
                    poly(r, &[(0.24, 0.62), (0.46, 0.33), (0.72, 0.50), (0.50, 0.79)]),
                    color,
                    Stroke::NONE,
                ));
            }
        }
        Icon::Object => {
            p.rect_stroke(
                rect(r, 0.20, 0.20, 0.80, 0.80),
                0.0,
                thin,
                egui::StrokeKind::Inside,
            );
            for (x, y) in [(0.20, 0.20), (0.80, 0.20), (0.20, 0.80), (0.80, 0.80)] {
                p.rect_filled(
                    Rect::from_center_size(pt(r, x, y), Vec2::splat(w * 0.18)),
                    0.0,
                    a,
                );
            }
        }
        Icon::Frame => {
            // A page cut into three panels — the koma layout at a glance.
            p.rect_stroke(
                rect(r, 0.12, 0.14, 0.88, 0.86),
                0.0,
                line,
                egui::StrokeKind::Inside,
            );
            // The koma divisions are what this tool draws.
            p.line(poly(r, &[(0.12, 0.48), (0.88, 0.48)]), a_thin);
            p.line(poly(r, &[(0.54, 0.14), (0.54, 0.48)]), a_thin);
        }
        Icon::Text => {
            // Capital A over a baseline — the classic text-tool glyph.
            p.line(poly(r, &[(0.22, 0.74), (0.50, 0.14), (0.78, 0.74)]), line);
            p.line(poly(r, &[(0.34, 0.52), (0.66, 0.52)]), thin);
            p.line(poly(r, &[(0.14, 0.88), (0.86, 0.88)]), a_thin);
        }
        Icon::Balloon => {
            // Speech bubble: ellipse outline + a tail poking out bottom-left.
            let c = pt(r, 0.50, 0.42);
            let (rx, ry) = (w * 0.36, w * 0.28);
            let pts: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let t = i as f32 / 24.0 * std::f32::consts::TAU;
                    Pos2::new(c.x + rx * t.cos(), c.y + ry * t.sin())
                })
                .collect();
            p.add(Shape::line(pts, thin));
            p.add(Shape::convex_polygon(
                poly(r, &[(0.34, 0.62), (0.52, 0.66), (0.24, 0.92)]),
                a,
                Stroke::NONE,
            ));
        }
        Icon::Wand => {
            // Wand shaft with a star tip and two sparkles; the sparkles are
            // the "auto" in auto-select.
            p.line(poly(r, &[(0.34, 0.34), (0.88, 0.88)]), line);
            for (x, y, s) in [(0.26, 0.26, 0.16), (0.66, 0.14, 0.08), (0.14, 0.66, 0.08)] {
                p.line(poly(r, &[(x - s, y), (x + s, y)]), a_thin);
                p.line(poly(r, &[(x, y - s), (x, y + s)]), a_thin);
            }
        }
        Icon::Eyedrop => {
            // Pipette: bulb at the top-right, tip to the bottom-left, one drop.
            p.line(poly(r, &[(0.72, 0.28), (0.24, 0.76)]), line);
            p.circle_filled(pt(r, 0.76, 0.24), w * 0.15, color);
            p.line(poly(r, &[(0.60, 0.16), (0.84, 0.40)]), line);
            p.add(Shape::convex_polygon(
                poly(r, &[(0.24, 0.76), (0.32, 0.84), (0.12, 0.88)]),
                a,
                Stroke::NONE,
            ));
        }
        Icon::Liquify => {
            // Two straight rails, one warped traveller between them.
            p.line(poly(r, &[(0.22, 0.12), (0.22, 0.88)]), line);
            p.line(poly(r, &[(0.78, 0.12), (0.78, 0.88)]), line);
            p.line(
                poly(r, &[(0.50, 0.12), (0.38, 0.32), (0.62, 0.56), (0.50, 0.88)]),
                line,
            );
        }
        Icon::Pan => {
            // Palm + three fingers + thumb: CSP's grab tool, minus the
            // detail. Solid all through, so the whole hand takes the hue —
            // `hue_nav` is barely off grey, which is the point.
            let cr = egui::CornerRadius::same((w * 0.12) as u8);
            p.rect_filled(rect(r, 0.26, 0.46, 0.78, 0.92), cr, a);
            for x in [0.28, 0.44, 0.60] {
                p.rect_filled(rect(r, x, 0.22, x + 0.14, 0.56), cr, a);
            }
            p.rect_filled(rect(r, 0.10, 0.54, 0.28, 0.74), cr, a);
        }
        Icon::Undo => arc_arrow(p, r, (0.50, 0.54), 0.30, 20.0, -200.0, line, color),
        Icon::Redo => arc_arrow(p, r, (0.50, 0.54), 0.30, 160.0, 380.0, line, color),
        Icon::RotateLeft => arc_arrow(p, r, (0.50, 0.50), 0.32, 60.0, -240.0, line, a),
        Icon::RotateRight => arc_arrow(p, r, (0.50, 0.50), 0.32, 120.0, 420.0, line, a),
        Icon::RotateReset => {
            p.circle_stroke(pt(r, 0.50, 0.54), w * 0.30, thin);
            p.circle_filled(pt(r, 0.50, 0.16), w * 0.11, a);
        }
        Icon::FlipH => {
            p.extend(Shape::dashed_line(
                &poly(r, &[(0.50, 0.08), (0.50, 0.92)]),
                a_thin,
                w * 0.14,
                w * 0.12,
            ));
            p.add(Shape::convex_polygon(
                poly(r, &[(0.42, 0.18), (0.42, 0.82), (0.08, 0.50)]),
                color,
                Stroke::NONE,
            ));
            p.add(Shape::convex_polygon(
                poly(r, &[(0.58, 0.18), (0.58, 0.82), (0.92, 0.50)]),
                Color32::TRANSPARENT,
                thin,
            ));
        }
        // FlipH a quarter turn over: dashed axis across, solid half on top.
        Icon::FlipV => {
            p.extend(Shape::dashed_line(
                &poly(r, &[(0.08, 0.50), (0.92, 0.50)]),
                a_thin,
                w * 0.14,
                w * 0.12,
            ));
            p.add(Shape::convex_polygon(
                poly(r, &[(0.18, 0.42), (0.82, 0.42), (0.50, 0.08)]),
                color,
                Stroke::NONE,
            ));
            p.add(Shape::convex_polygon(
                poly(r, &[(0.18, 0.58), (0.82, 0.58), (0.50, 0.92)]),
                Color32::TRANSPARENT,
                thin,
            ));
        }
        Icon::Plus => {
            // Nothing but the plus, so the plus is the accent.
            p.line(poly(r, &[(0.50, 0.12), (0.50, 0.88)]), a_line);
            p.line(poly(r, &[(0.12, 0.50), (0.88, 0.50)]), a_line);
        }
        Icon::Duplicate => {
            // The COPY (the outlined sheet behind) is the new thing.
            p.rect_stroke(
                rect(r, 0.30, 0.30, 0.90, 0.90),
                1.0,
                a_thin,
                egui::StrokeKind::Inside,
            );
            p.rect_filled(rect(r, 0.10, 0.10, 0.66, 0.66), 1.0, color);
        }
        Icon::Trash => {
            // The one glyph that takes the hue whole: a red bin is a
            // convention older than this app, and the button it sits on is
            // the one worth hesitating over.
            p.line(poly(r, &[(0.14, 0.26), (0.86, 0.26)]), a_line);
            p.line(poly(r, &[(0.38, 0.14), (0.62, 0.14)]), a_thin);
            p.add(Shape::convex_polygon(
                poly(r, &[(0.22, 0.36), (0.78, 0.36), (0.70, 0.92), (0.30, 0.92)]),
                a,
                Stroke::NONE,
            ));
        }
        Icon::Label => {
            // A colour chip with a dropdown nub — the palette-colour
            // CONTROL, matching CSP's (audit: the old two offset chips
            // read as "duplicate").
            p.rect_filled(rect(r, 0.08, 0.18, 0.64, 0.74), 1.0, color);
            p.add(Shape::convex_polygon(
                poly(r, &[(0.72, 0.38), (0.96, 0.38), (0.84, 0.56)]),
                color,
                Stroke::NONE,
            ));
        }
        Icon::Clip => {
            // Clip to the layer below: a small inset layer hooking DOWN
            // into the full-width base (audit: the bent arrow read as a
            // set-square).
            p.rect_stroke(
                rect(r, 0.36, 0.08, 0.90, 0.36),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.line(poly(r, &[(0.20, 0.22), (0.20, 0.56)]), line);
            p.add(Shape::convex_polygon(
                poly(r, &[(0.20, 0.70), (0.09, 0.52), (0.31, 0.52)]),
                color,
                Stroke::NONE,
            ));
            // The base layer being clipped TO is the coloured one.
            p.rect_filled(rect(r, 0.08, 0.76, 0.92, 0.92), 1.0, a);
        }
        Icon::Lock => {
            p.rect_filled(rect(r, 0.22, 0.48, 0.78, 0.90), 1.0, color);
            let n = 10;
            let arc: Vec<Pos2> = (0..=n)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / n as f32);
                    pt(r, 0.50 + 0.20 * a.cos(), 0.48 + 0.26 * a.sin())
                })
                .collect();
            p.line(arc, line);
        }
        Icon::LockAlpha => {
            // ONE big transparency checker + a padlock over its corner
            // (audit: two 5px checker blobs were mush at toggle size).
            p.rect_stroke(
                rect(r, 0.06, 0.06, 0.64, 0.64),
                0.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.rect_filled(rect(r, 0.06, 0.06, 0.35, 0.35), 0.0, color);
            p.rect_filled(rect(r, 0.35, 0.35, 0.64, 0.64), 0.0, color);
            p.rect_filled(rect(r, 0.50, 0.64, 0.96, 0.96), 1.0, color);
            let n = 8;
            let arc: Vec<Pos2> = (0..=n)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / n as f32);
                    pt(r, 0.73 + 0.14 * a.cos(), 0.64 + 0.18 * a.sin())
                })
                .collect();
            p.line(arc, thin);
        }
        Icon::Folder => {
            // Folder tabs are the accent all through this family.
            p.add(Shape::convex_polygon(
                poly(r, &[(0.10, 0.24), (0.42, 0.24), (0.50, 0.36), (0.10, 0.36)]),
                a,
                Stroke::NONE,
            ));
            p.rect_filled(rect(r, 0.10, 0.34, 0.90, 0.80), 1.0, color);
        }
        Icon::FolderOpen => {
            // Tab + a hint of the back panel, with the front flap tilted
            // open — the classic "this folder is expanded" silhouette.
            p.add(Shape::convex_polygon(
                poly(r, &[(0.08, 0.22), (0.36, 0.22), (0.44, 0.33), (0.08, 0.33)]),
                a,
                Stroke::NONE,
            ));
            p.line(poly(r, &[(0.08, 0.33), (0.08, 0.78), (0.20, 0.78)]), thin);
            p.add(Shape::convex_polygon(
                poly(r, &[(0.22, 0.44), (0.94, 0.44), (0.80, 0.80), (0.08, 0.80)]),
                color,
                Stroke::NONE,
            ));
        }
        Icon::FrameFolder => {
            // A folder whose front is a panelled page: filled tab, outlined
            // body, koma divisions inside — unmistakably "the panel folder".
            p.add(Shape::convex_polygon(
                poly(r, &[(0.10, 0.18), (0.40, 0.18), (0.48, 0.29), (0.10, 0.29)]),
                a,
                Stroke::NONE,
            ));
            p.rect_stroke(
                rect(r, 0.10, 0.29, 0.90, 0.84),
                0.0,
                line,
                egui::StrokeKind::Inside,
            );
            p.line(poly(r, &[(0.10, 0.56), (0.90, 0.56)]), thin);
            p.line(poly(r, &[(0.53, 0.29), (0.53, 0.56)]), thin);
        }
        Icon::NewLayer => {
            // A sheet with two content lines; the corner plus says "make one".
            p.rect_stroke(
                rect(r, 0.10, 0.08, 0.70, 0.76),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.line(poly(r, &[(0.20, 0.30), (0.60, 0.30)]), thin);
            p.line(poly(r, &[(0.20, 0.46), (0.60, 0.46)]), thin);
            // The corner plus badge is the accent on every New-* glyph; the
            // subject it hangs off stays grey.
            p.line(poly(r, &[(0.80, 0.60), (0.80, 0.98)]), a_line);
            p.line(poly(r, &[(0.61, 0.79), (0.99, 0.79)]), a_line);
        }
        Icon::NewVector => {
            // A bare S-curve with square node handles + the corner plus
            // (audit: inside a bounding box the mark was illegible; the
            // curve IS the subject).
            let pts: Vec<Pos2> = (0..=14)
                .map(|i| {
                    let t = i as f32 / 14.0;
                    let u = 1.0 - t;
                    let x = u * u * 0.10 + 2.0 * u * t * 0.42 + t * t * 0.68;
                    let y = u * u * 0.70 + 2.0 * u * t * 0.02 + t * t * 0.52;
                    pt(r, x, y)
                })
                .collect();
            p.add(Shape::line(pts, line));
            for (x, y) in [(0.10, 0.70), (0.68, 0.52)] {
                p.rect_filled(
                    Rect::from_center_size(pt(r, x, y), Vec2::splat(w * 0.18)),
                    0.0,
                    color,
                );
            }
            // The corner plus badge is the accent on every New-* glyph; the
            // subject it hangs off stays grey.
            p.line(poly(r, &[(0.80, 0.60), (0.80, 0.98)]), a_line);
            p.line(poly(r, &[(0.61, 0.79), (0.99, 0.79)]), a_line);
        }
        Icon::NewFolder => {
            // Square-shouldered tab (audit: the slanted tab rounded to a
            // lump at button size).
            p.rect_filled(rect(r, 0.06, 0.18, 0.38, 0.30), 0.5, color);
            p.rect_filled(rect(r, 0.06, 0.28, 0.74, 0.72), 0.5, color);
            // The corner plus badge is the accent on every New-* glyph; the
            // subject it hangs off stays grey.
            p.line(poly(r, &[(0.80, 0.60), (0.80, 0.98)]), a_line);
            p.line(poly(r, &[(0.61, 0.79), (0.99, 0.79)]), a_line);
        }
        Icon::NewFrameFolder => {
            // The panelled FOLDER (the frame-folder row glyph, shrunk) +
            // corner plus — the plain panelled page shared its silhouette
            // with New-layer (audit).
            p.add(Shape::convex_polygon(
                poly(r, &[(0.06, 0.12), (0.30, 0.12), (0.37, 0.22), (0.06, 0.22)]),
                color,
                Stroke::NONE,
            ));
            p.rect_stroke(
                rect(r, 0.06, 0.22, 0.74, 0.76),
                0.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.line(poly(r, &[(0.06, 0.50), (0.74, 0.50)]), thin);
            p.line(poly(r, &[(0.42, 0.22), (0.42, 0.50)]), thin);
            // The corner plus badge is the accent on every New-* glyph; the
            // subject it hangs off stays grey.
            p.line(poly(r, &[(0.80, 0.60), (0.80, 0.98)]), a_line);
            p.line(poly(r, &[(0.61, 0.79), (0.99, 0.79)]), a_line);
        }
        Icon::MergeDown => {
            p.line(poly(r, &[(0.50, 0.10), (0.50, 0.52)]), line);
            p.add(Shape::convex_polygon(
                poly(r, &[(0.50, 0.72), (0.30, 0.50), (0.70, 0.50)]),
                a,
                Stroke::NONE,
            ));
            p.line(poly(r, &[(0.14, 0.88), (0.86, 0.88)]), line);
        }
        Icon::Wrench => {
            p.circle_stroke(pt(r, 0.32, 0.32), r.width() * 0.20, line);
            p.line(poly(r, &[(0.44, 0.44), (0.86, 0.86)]), line);
        }
        Icon::Swap => {
            p.line(poly(r, &[(0.16, 0.36), (0.78, 0.36)]), thin);
            p.add(Shape::convex_polygon(
                poly(r, &[(0.90, 0.36), (0.72, 0.24), (0.72, 0.48)]),
                color,
                Stroke::NONE,
            ));
            p.line(poly(r, &[(0.22, 0.66), (0.84, 0.66)]), thin);
            p.add(Shape::convex_polygon(
                poly(r, &[(0.10, 0.66), (0.28, 0.54), (0.28, 0.78)]),
                color,
                Stroke::NONE,
            ));
        }
        Icon::ZoomIn | Icon::ZoomOut => {
            p.circle_stroke(pt(r, 0.42, 0.42), w * 0.28, line);
            p.line(poly(r, &[(0.64, 0.64), (0.90, 0.90)]), line);
            // The lens is grey; the + / − inside it carries the hue.
            p.line(poly(r, &[(0.28, 0.42), (0.56, 0.42)]), a_thin);
            if icon == Icon::ZoomIn {
                p.line(poly(r, &[(0.42, 0.28), (0.42, 0.56)]), a_thin);
            }
        }
        Icon::ZoomFit => {
            p.rect_stroke(
                rect(r, 0.12, 0.16, 0.88, 0.84),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            // Inward arrows at two corners.
            p.line(poly(r, &[(0.26, 0.44), (0.26, 0.30), (0.40, 0.30)]), a_thin);
            p.line(poly(r, &[(0.74, 0.56), (0.74, 0.70), (0.60, 0.70)]), a_thin);
        }
        Icon::Zoom100 => {
            // "1:1" — the colon is the accent, the ones stay grey.
            p.line(poly(r, &[(0.22, 0.30), (0.22, 0.74)]), line);
            p.circle_filled(pt(r, 0.48, 0.42), w * 0.06, a);
            p.circle_filled(pt(r, 0.48, 0.64), w * 0.06, a);
            p.line(poly(r, &[(0.74, 0.30), (0.74, 0.74)]), line);
        }
        Icon::Close => {
            p.line(poly(r, &[(0.22, 0.22), (0.78, 0.78)]), line);
            p.line(poly(r, &[(0.78, 0.22), (0.22, 0.78)]), line);
        }
        Icon::Book => {
            // An open book: two page outlines around a spine (the reader).
            // The spine is the accent.
            p.line(poly(r, &[(0.50, 0.28), (0.50, 0.76)]), a_line);
            p.line(
                poly(
                    r,
                    &[
                        (0.50, 0.28),
                        (0.42, 0.24),
                        (0.20, 0.26),
                        (0.20, 0.70),
                        (0.44, 0.70),
                        (0.50, 0.76),
                    ],
                ),
                line,
            );
            p.line(
                poly(
                    r,
                    &[
                        (0.50, 0.28),
                        (0.58, 0.24),
                        (0.80, 0.26),
                        (0.80, 0.70),
                        (0.56, 0.70),
                        (0.50, 0.76),
                    ],
                ),
                line,
            );
        }
        Icon::Eye | Icon::EyeOff => {
            let lid = |up: bool| -> Vec<Pos2> {
                (0..=10)
                    .map(|i| {
                        let t = i as f32 / 10.0;
                        let bulge =
                            (t * std::f32::consts::PI).sin() * if up { -0.30 } else { 0.26 };
                        pt(r, 0.06 + t * 0.88, 0.50 + bulge)
                    })
                    .collect()
            };
            p.line(lid(true), thin);
            p.line(lid(false), thin);
            p.circle_filled(pt(r, 0.50, 0.50), w * 0.17, color);
            if icon == Icon::EyeOff {
                p.line(poly(r, &[(0.10, 0.88), (0.90, 0.12)]), line);
            }
        }
        Icon::SelDeselect => {
            // Dashed rect drawn as four corner Ls — gaps read as dashes.
            let q = rect(r, 0.18, 0.20, 0.82, 0.80);
            let (s, e) = (q.min, q.max);
            let seg = |from: egui::Pos2, to: egui::Pos2| p.line_segment([from, to], a_thin);
            let (kx, ky) = (q.width() * 0.30, q.height() * 0.30);
            seg(s, egui::pos2(s.x + kx, s.y));
            seg(s, egui::pos2(s.x, s.y + ky));
            seg(egui::pos2(e.x - kx, s.y), e);
            seg(e, egui::pos2(e.x, s.y + ky));
            seg(e, egui::pos2(e.x, e.y - ky));
            seg(egui::pos2(e.x, e.y - ky), egui::pos2(e.x - kx, e.y));
            seg(egui::pos2(s.x, e.y - ky), egui::pos2(s.x, e.y));
            seg(egui::pos2(s.x + kx, e.y), egui::pos2(s.x + kx, e.y));
        }
        Icon::SelInvert => {
            p.rect_stroke(
                rect(r, 0.18, 0.18, 0.82, 0.82),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            // The half that stays selected.
            p.rect_filled(rect(r, 0.18, 0.18, 0.50, 0.82), 0.0, a);
        }
        Icon::SelExpand => {
            // Box grey, the direction arms coloured — the same rule as
            // Shrink and Transform, so the three read as one row.
            p.rect_stroke(
                rect(r, 0.30, 0.30, 0.70, 0.70),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.line(poly(r, &[(0.50, 0.24), (0.50, 0.06)]), a_thin);
            p.line(poly(r, &[(0.50, 0.76), (0.50, 0.94)]), a_thin);
            p.line(poly(r, &[(0.24, 0.50), (0.06, 0.50)]), a_thin);
            p.line(poly(r, &[(0.76, 0.50), (0.94, 0.50)]), a_thin);
        }
        Icon::SelShrink => {
            p.rect_stroke(
                rect(r, 0.14, 0.14, 0.86, 0.86),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.line(poly(r, &[(0.50, 0.14), (0.50, 0.34)]), a_thin);
            p.line(poly(r, &[(0.50, 0.86), (0.50, 0.66)]), a_thin);
            p.line(poly(r, &[(0.14, 0.50), (0.34, 0.50)]), a_thin);
            p.line(poly(r, &[(0.86, 0.50), (0.66, 0.50)]), a_thin);
        }
        Icon::SelClearOutside => {
            p.rect_stroke(
                rect(r, 0.12, 0.28, 0.72, 0.88),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            // X just outside the box's top-right corner — the erasing half
            // of the glyph, so it is the half that goes red.
            p.line(poly(r, &[(0.76, 0.12), (0.94, 0.30)]), a_thin);
            p.line(poly(r, &[(0.94, 0.12), (0.76, 0.30)]), a_thin);
        }
        Icon::Funnel => {
            // The classic funnel: wide mouth tapering into a stem.
            p.line(poly(r, &[(0.12, 0.18), (0.88, 0.18)]), thin);
            p.line(poly(r, &[(0.12, 0.18), (0.42, 0.54), (0.42, 0.86)]), thin);
            p.line(poly(r, &[(0.88, 0.18), (0.58, 0.54), (0.58, 0.86)]), thin);
        }
        Icon::SelDrawOutside => {
            // Selection box with a pen stroke crossing its border: the
            // stroke starts inside, escapes past the top-right edge, and
            // ends in a nib dot outside.
            p.rect_stroke(
                rect(r, 0.12, 0.30, 0.68, 0.88),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            // The stroke that escapes is the whole point of the icon.
            p.line(poly(r, &[(0.24, 0.76), (0.50, 0.56), (0.86, 0.20)]), a_line);
            p.circle_filled(pt(r, 0.86, 0.20), w * 0.09, a);
        }
        Icon::SelTransform => {
            p.rect_stroke(
                rect(r, 0.40, 0.40, 0.60, 0.60),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.line(poly(r, &[(0.50, 0.34), (0.50, 0.08)]), a_thin);
            p.line(poly(r, &[(0.50, 0.66), (0.50, 0.92)]), a_thin);
            p.line(poly(r, &[(0.34, 0.50), (0.08, 0.50)]), a_thin);
            p.line(poly(r, &[(0.66, 0.50), (0.92, 0.50)]), a_thin);
        }
        Icon::SelCrop => {
            // Two overlapping corner brackets — the classic crop glyph. One
            // bracket coloured reads as the new edge closing in.
            p.line(poly(r, &[(0.32, 0.08), (0.32, 0.68), (0.92, 0.68)]), line);
            p.line(poly(r, &[(0.08, 0.32), (0.68, 0.32), (0.68, 0.92)]), a_line);
        }
        Icon::SelCut => {
            // Scissors: two blades crossing, two ring handles. The handles
            // take the hue — coloured blades read as a painted X.
            p.line(poly(r, &[(0.20, 0.12), (0.72, 0.78)]), thin);
            p.line(poly(r, &[(0.20, 0.78), (0.72, 0.12)]), thin);
            p.circle_stroke(pt(r, 0.18, 0.86), w * 0.09, a_thin);
            p.circle_stroke(pt(r, 0.18, 0.14), w * 0.09, a_thin);
        }
        Icon::SelCopy => {
            // Two stacked sheets, the front one offset.
            p.rect_stroke(
                rect(r, 0.14, 0.26, 0.62, 0.74),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.line(
                poly(
                    r,
                    &[
                        (0.38, 0.26),
                        (0.38, 0.10),
                        (0.86, 0.10),
                        (0.86, 0.58),
                        (0.62, 0.58),
                    ],
                ),
                a_thin,
            );
        }
        Icon::SelPaste => {
            // A clipboard: board, clip on top, a dropped patch on it. The
            // clip is the accent.
            p.rect_stroke(
                rect(r, 0.16, 0.14, 0.84, 0.90),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                rect(r, 0.38, 0.06, 0.62, 0.20),
                1.0,
                a_thin,
                egui::StrokeKind::Inside,
            );
            p.line(
                poly(
                    r,
                    &[
                        (0.30, 0.42),
                        (0.70, 0.42),
                        (0.70, 0.76),
                        (0.30, 0.76),
                        (0.30, 0.42),
                    ],
                ),
                thin,
            );
        }
        Icon::Reference => {
            // CSP's beacon/lighthouse: tapered tower, lamp, two rays —
            // unique silhouette (audit: the framed picture read as
            // "image", not "reference").
            p.add(Shape::convex_polygon(
                poly(r, &[(0.42, 0.36), (0.58, 0.36), (0.68, 0.92), (0.32, 0.92)]),
                color,
                Stroke::NONE,
            ));
            p.rect_filled(rect(r, 0.38, 0.16, 0.62, 0.34), 1.0, color);
            p.line(poly(r, &[(0.30, 0.22), (0.12, 0.12)]), thin);
            p.line(poly(r, &[(0.70, 0.22), (0.88, 0.12)]), thin);
        }
        Icon::Draft => {
            // A pencil over a lined page — 下書き. (Audit: the old red
            // diagonal cross read as "delete", and was the only red in
            // the palette, on a benign toggle.)
            p.rect_stroke(
                rect(r, 0.08, 0.12, 0.62, 0.88),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.line(poly(r, &[(0.18, 0.34), (0.50, 0.34)]), thin);
            p.line(poly(r, &[(0.18, 0.50), (0.44, 0.50)]), thin);
            p.line(poly(r, &[(0.52, 0.76), (0.90, 0.26)]), line);
            p.add(Shape::convex_polygon(
                poly(r, &[(0.42, 0.90), (0.58, 0.83), (0.48, 0.70)]),
                color,
                Stroke::NONE,
            ));
        }
        Icon::Figure => {
            // A ruled diagonal with endpoint grips.
            p.line(poly(r, &[(0.20, 0.80), (0.80, 0.20)]), line);
            p.circle_filled(poly(r, &[(0.20, 0.80)])[0], w * 0.09, a);
            p.circle_filled(poly(r, &[(0.80, 0.20)])[0], w * 0.09, a);
        }
        // The three figure primitives are nothing BUT their outline, so the
        // outline is what takes the ink hue.
        Icon::Rect => {
            p.rect_stroke(
                rect(r, 0.18, 0.24, 0.82, 0.76),
                0.0,
                a_line,
                egui::StrokeKind::Inside,
            );
        }
        Icon::Ellipse => {
            p.circle_stroke(r.center(), w * 0.30, a_line);
        }
        Icon::Poly => {
            // A five-point star outline reads as "click vertices".
            let c = r.center();
            let mut pts = Vec::new();
            for k in 0..10 {
                let a = (k as f32) * std::f32::consts::TAU / 10.0 - std::f32::consts::FRAC_PI_2;
                let rad = if k % 2 == 0 { w * 0.34 } else { w * 0.15 };
                pts.push(Pos2::new(c.x + a.cos() * rad, c.y + a.sin() * rad));
            }
            p.add(egui::Shape::closed_line(pts, a_line));
        }
        Icon::Arc => {
            // The gesture itself, in one glyph: the dragged baseline drawn
            // faint, and the curve it bends into drawn solid, sharing both
            // endpoints. Distinct from `Vector` (which wears square control
            // grips — this tool has none, which is its whole point).
            let ends = [(0.14, 0.72), (0.86, 0.72)];
            p.add(Shape::line(
                vec![pt(r, ends[0].0, ends[0].1), pt(r, ends[1].0, ends[1].1)],
                a_thin,
            ));
            let bend: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let t = i as f32 / 16.0;
                    let u = 1.0 - t;
                    // Control point solved so the arc peaks at (0.50, 0.22).
                    let x = u * u * ends[0].0 + 2.0 * u * t * 0.50 + t * t * ends[1].0;
                    let y = u * u * ends[0].1 + 2.0 * u * t * (2.0 * 0.22 - 0.72) + t * t * ends[1].1;
                    pt(r, x, y)
                })
                .collect();
            p.add(Shape::line(bend, line));
            for (x, y) in ends {
                p.circle_filled(pt(r, x, y), w * 0.09, a);
            }
        }
        Icon::Gradient => {
            // A ramp square: dark to light, top-left to bottom-right.
            let g = rect(r, 0.20, 0.22, 0.80, 0.78);
            let steps = 10;
            for k in 0..steps {
                let t0 = k as f32 / steps as f32;
                let t1 = (k + 1) as f32 / steps as f32;
                let strip = Rect::from_min_max(
                    Pos2::new(
                        g.left() + (g.right() - g.left()) * t0,
                        g.top() + (g.bottom() - g.top()) * t0,
                    ),
                    Pos2::new(
                        g.left() + (g.right() - g.left()) * t1 + 0.5,
                        g.top() + (g.bottom() - g.top()) * t1 + 0.5,
                    ),
                );
                let v = (70.0 + 150.0 * t0) as u8;
                p.rect_filled(strip, 0.0, Color32::from_gray(v));
            }
            // The ramp itself is greyscale BY DEFINITION; only its frame can
            // carry the hue.
            p.rect_stroke(g, 0.0, a_thin, egui::StrokeKind::Inside);
        }
        Icon::Vector => {
            // A quadratic through (0.12,0.80) → (0.88,0.72) pulled up by a
            // handle at (0.50,0.04), with square grips on the two ends: the
            // "this layer records geometry" marker.
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let t = i as f32 / 16.0;
                    let u = 1.0 - t;
                    let x = u * u * 0.12 + 2.0 * u * t * 0.50 + t * t * 0.88;
                    let y = u * u * 0.80 + 2.0 * u * t * 0.04 + t * t * 0.72;
                    pt(r, x, y)
                })
                .collect();
            p.add(Shape::line(pts, line));
            // The control points are what makes it a vector layer.
            for (x, y) in [(0.12, 0.80), (0.88, 0.72)] {
                p.rect_filled(
                    Rect::from_center_size(pt(r, x, y), Vec2::splat(w * 0.22)),
                    0.0,
                    a,
                );
            }
        }
        Icon::Tone => {
            // 3x3 halftone grid, dots growing toward the bottom-right so the
            // glyph reads as a density ramp rather than a dice face.
            for (row, y) in [0.22_f32, 0.50, 0.78].into_iter().enumerate() {
                for (col, x) in [0.22_f32, 0.50, 0.78].into_iter().enumerate() {
                    let t = (row + col) as f32 / 4.0;
                    // Nine small dots: tinting them all is the only reading.
                    p.circle_filled(pt(r, x, y), w * (0.055 + 0.085 * t), a);
                }
            }
        }
        Icon::Paper => {
            // Sheet outline with the top-right corner turned down.
            p.add(Shape::closed_line(
                poly(
                    r,
                    &[
                        (0.20, 0.10),
                        (0.62, 0.10),
                        (0.82, 0.30),
                        (0.82, 0.90),
                        (0.20, 0.90),
                    ],
                ),
                thin,
            ));
            // The turned-down corner is the accent.
            p.line(poly(r, &[(0.62, 0.10), (0.62, 0.30), (0.82, 0.30)]), a_thin);
        }
        Icon::FileObject => {
            // The Paper sheet, pushed right to leave room for the arrow
            // that keeps feeding it.
            p.add(Shape::closed_line(
                poly(
                    r,
                    &[
                        (0.42, 0.10),
                        (0.76, 0.10),
                        (0.92, 0.26),
                        (0.92, 0.90),
                        (0.42, 0.90),
                    ],
                ),
                thin,
            ));
            p.line(poly(r, &[(0.76, 0.10), (0.76, 0.26), (0.92, 0.26)]), thin);
            // The link: an arrow entering the sheet from outside.
            p.line(poly(r, &[(0.06, 0.50), (0.42, 0.50)]), a_thin);
            p.line(poly(r, &[(0.28, 0.36), (0.42, 0.50), (0.28, 0.64)]), a_thin);
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(
                poly(r, &[(0.24, 0.12), (0.86, 0.50), (0.24, 0.88)]),
                color,
                Stroke::NONE,
            ));
        }
        Icon::Record => {
            p.circle_filled(pt(r, 0.5, 0.5), w * 0.34, color);
        }
        Icon::Stop => {
            p.rect_filled(rect(r, 0.20, 0.20, 0.80, 0.80), 1.0, color);
        }
        Icon::Grip => {
            for x in [0.34, 0.66] {
                for y in [0.22, 0.44, 0.66, 0.88] {
                    p.circle_filled(pt(r, x, y), w * 0.075, color);
                }
            }
        }
        Icon::StreamLines => {
            // Three parallel motion strokes, staggered like a speed-line
            // panel; the middle one longest.
            p.line(poly(r, &[(0.10, 0.30), (0.72, 0.30)]), thin);
            // The lead stroke carries the hue; the two behind it stay grey.
            p.line(poly(r, &[(0.20, 0.52), (0.92, 0.52)]), a_line);
            p.line(poly(r, &[(0.10, 0.74), (0.64, 0.74)]), thin);
        }
        Icon::FocusLines => {
            // Eight rays converging on an empty centre — the donut a
            // saturated-line drag places.
            let c = r.center();
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::TAU / 8.0 + 0.2;
                let (s, co) = a.sin_cos();
                p.line(
                    vec![
                        Pos2::new(c.x + co * w * 0.18, c.y + s * w * 0.18),
                        Pos2::new(c.x + co * w * 0.42, c.y + s * w * 0.42),
                    ],
                    a_thin,
                );
            }
        }
        Icon::UrchinFlash => {
            // Eight FILLED spikes, needle-pointed at the hole — the shape
            // itself is the difference from FocusLines, so the icon draws
            // wedges rather than strokes.
            let c = r.center();
            for k in 0..8 {
                let ang = k as f32 * std::f32::consts::TAU / 8.0 + 0.2;
                let (s, co) = ang.sin_cos();
                let (nx, ny) = (-s, co);
                let tip = w * 0.46;
                let hw = w * 0.09;
                p.add(Shape::convex_polygon(
                    vec![
                        Pos2::new(c.x + co * w * 0.14, c.y + s * w * 0.14),
                        Pos2::new(c.x + co * tip + nx * hw, c.y + s * tip + ny * hw),
                        Pos2::new(c.x + co * tip - nx * hw, c.y + s * tip - ny * hw),
                    ],
                    a,
                    Stroke::NONE,
                ));
            }
        }
        Icon::Pattern => {
            // 2x2 checker inside a sheet outline: the two diagonal quadrants
            // filled, so the glyph reads at palette sizes (a 1px outline
            // checker closes up).
            let (b, m, e) = (0.14_f32, 0.50_f32, 0.86_f32);
            p.add(Shape::closed_line(
                poly(r, &[(b, b), (e, b), (e, e), (b, e)]),
                thin,
            ));
            p.add(Shape::convex_polygon(
                poly(r, &[(b, b), (m, b), (m, m), (b, m)]),
                a,
                Stroke::NONE,
            ));
            p.add(Shape::convex_polygon(
                poly(r, &[(m, m), (e, m), (e, e), (m, e)]),
                a,
                Stroke::NONE,
            ));
        }
        Icon::Pose3d => {
            // A posing mannequin: head, torso, one arm raised — the gesture
            // reads even at chip size, which is where this glyph lives.
            p.circle_filled(pt(r, 0.50, 0.20), w * 0.10, color);
            p.line(poly(r, &[(0.50, 0.32), (0.50, 0.64)]), line);
            p.line(poly(r, &[(0.22, 0.36), (0.50, 0.42), (0.78, 0.30)]), thin);
            p.line(poly(r, &[(0.50, 0.64), (0.30, 0.88)]), thin);
            p.line(poly(r, &[(0.50, 0.64), (0.72, 0.86)]), thin);
        }
    }
}

// --- geometry helpers ---------------------------------------------------

fn pt(r: Rect, x: f32, y: f32) -> Pos2 {
    r.min + Vec2::new(r.width() * x, r.height() * y)
}

fn poly(r: Rect, pts: &[(f32, f32)]) -> Vec<Pos2> {
    pts.iter().map(|&(x, y)| pt(r, x, y)).collect()
}

fn rect(r: Rect, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::from_min_max(pt(r, x0, y0), pt(r, x1, y1))
}

/// An arc from `a0` to `a1` degrees (y-down, so increasing = clockwise on
/// screen) with a filled arrowhead on the `a1` end, pointing along the tangent.
///
/// `head` is separate from `stroke.color` so the rotate icons can put their
/// accent on the arrowhead alone; pass the stroke's own colour for a
/// monochrome arrow.
fn arc_arrow(
    p: &Painter,
    r: Rect,
    centre: (f32, f32),
    rad: f32,
    a0: f32,
    a1: f32,
    stroke: Stroke,
    head: Color32,
) {
    const N: usize = 24;
    let at = |deg: f32| {
        let a = deg.to_radians();
        pt(r, centre.0 + rad * a.cos(), centre.1 + rad * a.sin())
    };
    let pts: Vec<Pos2> = (0..=N)
        .map(|i| at(a0 + (a1 - a0) * i as f32 / N as f32))
        .collect();
    p.line(pts.clone(), stroke);

    // Tangent at the end, in the direction of travel.
    let end = a1.to_radians();
    let dir = if a1 > a0 { 1.0 } else { -1.0 };
    let t = Vec2::new(-end.sin() * dir, end.cos() * dir);
    let n = Vec2::new(t.y, -t.x);
    let last = *pts.last().unwrap();
    let s = r.width().min(r.height());
    p.add(Shape::convex_polygon(
        vec![
            last + t * s * 0.20,
            last + n * s * 0.13,
            last - n * s * 0.13,
        ],
        head,
        Stroke::NONE,
    ));
}

/// The transparent-colour tile: a small checkerboard, exactly what CSP draws in
/// the third colour slot.
///
/// **Its white and grey are NOT theme tokens and must not become any.** This
/// is the same rule `theme.rs` states for the overlay: a checkerboard means
/// "no pixels here", a convention every drawing app on earth paints in the
/// same two greys, and a sepia-tinted one would read as a *colour* rather
/// than as transparency.
pub fn checkerboard(p: &Painter, r: Rect, cell: f32) {
    p.rect_filled(r, 0, Color32::WHITE);
    let (mut y, mut row) = (r.top(), 0);
    while y < r.bottom() {
        let mut x = r.left();
        let mut col = row;
        while x < r.right() {
            if col % 2 == 0 {
                let c = Rect::from_min_max(
                    egui::pos2(x, y),
                    egui::pos2((x + cell).min(r.right()), (y + cell).min(r.bottom())),
                );
                p.rect_filled(c, 0, Color32::from_gray(190));
            }
            x += cell;
            col += 1;
        }
        y += cell;
        row += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The toggle's contract, tested where it is DECIDED rather than where
    /// it is painted: off means every icon resolves to no accent, including
    /// the ones that have a role. A painter test would prove nothing extra
    /// and would need a live egui context.
    #[test]
    fn colours_off_means_no_accent_for_anything() {
        for icon in [
            Icon::Pen,
            Icon::Trash,
            Icon::NewLayer,
            Icon::SelInvert,
            Icon::Folder,
            Icon::ZoomIn,
            Icon::Book,
            Icon::Grip,
        ] {
            assert_eq!(accent_of(icon, false, theme::DARK), None, "{icon:?}");
        }
        // …and on, the same list is exactly "has a role or does not".
        for icon in [Icon::Pen, Icon::Trash, Icon::NewLayer, Icon::Book] {
            assert!(accent_of(icon, true, theme::DARK).is_some(), "{icon:?}");
        }
        assert_eq!(
            accent_of(Icon::Grip, true, theme::DARK),
            None,
            "a roleless icon stays grey even with colours on"
        );
    }

    /// The hue comes from the LIVE theme, not from a literal — a coloured
    /// icon in the sepia palette must be sepia's ink, not dark's.
    #[test]
    fn the_accent_follows_the_theme() {
        assert_eq!(
            accent_of(Icon::Pen, true, theme::DARK),
            Some(theme::DARK.hue_ink)
        );
        assert_eq!(
            accent_of(Icon::Pen, true, theme::SEPIA),
            Some(theme::SEPIA.hue_ink)
        );
        assert_ne!(theme::DARK.hue_ink, theme::SEPIA.hue_ink);
    }

    /// The taste decisions, spot-checked where getting them wrong would be
    /// actively misleading: the bin is the red one, "new" is the green one,
    /// and the selection launcher is one family rather than a rainbow.
    #[test]
    fn the_roles_say_what_the_icons_do() {
        assert_eq!(Icon::Trash.accent_role(), Some(IconRole::Destroy));
        assert_eq!(Icon::NewFolder.accent_role(), Some(IconRole::Create));
        assert_eq!(Icon::Pen.accent_role(), Some(IconRole::Ink));
        for icon in [
            Icon::SelDeselect,
            Icon::SelInvert,
            Icon::SelExpand,
            Icon::SelShrink,
            Icon::SelTransform,
            Icon::SelDrawOutside,
            // The two paint sub tools: their marquee takes the hue, the nib
            // and the slab inside stay grey. (The bare `Eraser` is roleless
            // precisely because it has no sub-shape to accent; this one
            // does — the box around it.)
            Icon::SelPen,
            Icon::SelEraser,
        ] {
            assert_eq!(icon.accent_role(), Some(IconRole::Select), "{icon:?}");
        }
        // The one launcher op that erases pixels breaks that family on
        // purpose.
        assert_eq!(Icon::SelClearOutside.accent_role(), Some(IconRole::Destroy));
        // Reference and Draft carry `ref_mark`/`draft_mark` from their
        // callers; an accent here would fight the tint they are given.
        assert_eq!(Icon::Reference.accent_role(), None);
        assert_eq!(Icon::Draft.accent_role(), None);
    }

    /// The global is a plain switch, and it defaults ON (owner: coloured
    /// icons are the shipped look, monochrome is the opt-out).
    #[test]
    fn the_global_switch_round_trips() {
        assert!(accents_on(), "colours ship on");
        set_accents(false);
        assert!(!accents_on());
        set_accents(true);
    }
}
