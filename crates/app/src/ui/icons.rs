//! Tool/command icons drawn with the egui painter, on a unit square.
//!
//! Not glyphs: the bundled egui fonts carry no pictographs, so `👁`/`◉`/`✕` come
//! out as tofu on this machine (verified in `--screenshot` when the layer eye
//! was first tried). Everything here is font-independent geometry, which also
//! means it stays crisp at any DPI.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Pen,
    Eraser,
    Fill,
    Select,
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
    /// Gradient tool: a ramp square.
    Gradient,
    /// Vector layer (docs/VECTOR-INKING.md): a curve with its control points
    /// — the strokes are geometry beside the pixels.
    Vector,
    /// Tone layer (CSP トーンレイヤー): a halftone patch, dots ramping.
    Tone,
    /// The paper under the stack (PA-001): a sheet with a folded corner.
    Paper,
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
    /// Palette-column collapse (`ui.rs`): a chevron pointing at the screen
    /// edge the column folds towards, or back at the canvas once it is
    /// folded away. Geometry, not `<`/`>` — the bundled fonts have no
    /// pictographs and the owner's rule is no raw ascii glyphs in the UI.
    ChevronLeft,
    ChevronRight,
}

/// Paint `icon` to fill `r` (which should be square-ish), in `color`.
pub fn paint(p: &Painter, r: Rect, icon: Icon, color: Color32) {
    let w = r.width().min(r.height());
    let line = Stroke::new((w * 0.11).clamp(1.0, 2.2), color);
    let thin = Stroke::new((w * 0.075).clamp(0.8, 1.4), color);

    match icon {
        Icon::Pen => {
            // Nib triangle at the bottom-left, barrel up to the top-right.
            p.add(Shape::convex_polygon(
                poly(r, &[(0.08, 0.92), (0.40, 0.78), (0.24, 0.60)]),
                color,
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
            p.circle_filled(pt(r, 0.90, 0.74), w * 0.10, color);
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
            p.extend(Shape::dashed_line(&pts, thin, w * 0.16, w * 0.13));
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
                    color,
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
            p.line(poly(r, &[(0.12, 0.48), (0.88, 0.48)]), thin);
            p.line(poly(r, &[(0.54, 0.14), (0.54, 0.48)]), thin);
        }
        Icon::Text => {
            // Capital A over a baseline — the classic text-tool glyph.
            p.line(poly(r, &[(0.22, 0.74), (0.50, 0.14), (0.78, 0.74)]), line);
            p.line(poly(r, &[(0.34, 0.52), (0.66, 0.52)]), thin);
            p.line(poly(r, &[(0.14, 0.88), (0.86, 0.88)]), thin);
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
                color,
                Stroke::NONE,
            ));
        }
        Icon::Wand => {
            // Wand shaft with a star tip and two sparkles.
            p.line(poly(r, &[(0.34, 0.34), (0.88, 0.88)]), line);
            for (x, y, s) in [(0.26, 0.26, 0.16), (0.66, 0.14, 0.08), (0.14, 0.66, 0.08)] {
                p.line(poly(r, &[(x - s, y), (x + s, y)]), thin);
                p.line(poly(r, &[(x, y - s), (x, y + s)]), thin);
            }
        }
        Icon::Eyedrop => {
            // Pipette: bulb at the top-right, tip to the bottom-left, one drop.
            p.line(poly(r, &[(0.72, 0.28), (0.24, 0.76)]), line);
            p.circle_filled(pt(r, 0.76, 0.24), w * 0.15, color);
            p.line(poly(r, &[(0.60, 0.16), (0.84, 0.40)]), line);
            p.add(Shape::convex_polygon(
                poly(r, &[(0.24, 0.76), (0.32, 0.84), (0.12, 0.88)]),
                color,
                Stroke::NONE,
            ));
        }
        Icon::Pan => {
            // Palm + three fingers + thumb: CSP's grab tool, minus the detail.
            let cr = egui::CornerRadius::same((w * 0.12) as u8);
            p.rect_filled(rect(r, 0.26, 0.46, 0.78, 0.92), cr, color);
            for x in [0.28, 0.44, 0.60] {
                p.rect_filled(rect(r, x, 0.22, x + 0.14, 0.56), cr, color);
            }
            p.rect_filled(rect(r, 0.10, 0.54, 0.28, 0.74), cr, color);
        }
        Icon::Undo => arc_arrow(p, r, (0.50, 0.54), 0.30, 20.0, -200.0, line, color),
        Icon::Redo => arc_arrow(p, r, (0.50, 0.54), 0.30, 160.0, 380.0, line, color),
        Icon::RotateLeft => arc_arrow(p, r, (0.50, 0.50), 0.32, 60.0, -240.0, line, color),
        Icon::RotateRight => arc_arrow(p, r, (0.50, 0.50), 0.32, 120.0, 420.0, line, color),
        Icon::RotateReset => {
            p.circle_stroke(pt(r, 0.50, 0.54), w * 0.30, thin);
            p.circle_filled(pt(r, 0.50, 0.16), w * 0.11, color);
        }
        Icon::FlipH => {
            p.extend(Shape::dashed_line(
                &poly(r, &[(0.50, 0.08), (0.50, 0.92)]),
                thin,
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
                thin,
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
            p.line(poly(r, &[(0.50, 0.12), (0.50, 0.88)]), line);
            p.line(poly(r, &[(0.12, 0.50), (0.88, 0.50)]), line);
        }
        Icon::Duplicate => {
            p.rect_stroke(
                rect(r, 0.30, 0.30, 0.90, 0.90),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.rect_filled(rect(r, 0.10, 0.10, 0.66, 0.66), 1.0, color);
        }
        Icon::Trash => {
            p.line(poly(r, &[(0.14, 0.26), (0.86, 0.26)]), line);
            p.line(poly(r, &[(0.38, 0.14), (0.62, 0.14)]), thin);
            p.add(Shape::convex_polygon(
                poly(r, &[(0.22, 0.36), (0.78, 0.36), (0.70, 0.92), (0.30, 0.92)]),
                color,
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
            p.rect_filled(rect(r, 0.08, 0.76, 0.92, 0.92), 1.0, color);
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
            p.add(Shape::convex_polygon(
                poly(r, &[(0.10, 0.24), (0.42, 0.24), (0.50, 0.36), (0.10, 0.36)]),
                color,
                Stroke::NONE,
            ));
            p.rect_filled(rect(r, 0.10, 0.34, 0.90, 0.80), 1.0, color);
        }
        Icon::FolderOpen => {
            // Tab + a hint of the back panel, with the front flap tilted
            // open — the classic "this folder is expanded" silhouette.
            p.add(Shape::convex_polygon(
                poly(r, &[(0.08, 0.22), (0.36, 0.22), (0.44, 0.33), (0.08, 0.33)]),
                color,
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
                color,
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
            p.line(poly(r, &[(0.80, 0.60), (0.80, 0.98)]), line);
            p.line(poly(r, &[(0.61, 0.79), (0.99, 0.79)]), line);
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
            p.line(poly(r, &[(0.80, 0.60), (0.80, 0.98)]), line);
            p.line(poly(r, &[(0.61, 0.79), (0.99, 0.79)]), line);
        }
        Icon::NewFolder => {
            // Square-shouldered tab (audit: the slanted tab rounded to a
            // lump at button size).
            p.rect_filled(rect(r, 0.06, 0.18, 0.38, 0.30), 0.5, color);
            p.rect_filled(rect(r, 0.06, 0.28, 0.74, 0.72), 0.5, color);
            p.line(poly(r, &[(0.80, 0.60), (0.80, 0.98)]), line);
            p.line(poly(r, &[(0.61, 0.79), (0.99, 0.79)]), line);
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
            p.line(poly(r, &[(0.80, 0.60), (0.80, 0.98)]), line);
            p.line(poly(r, &[(0.61, 0.79), (0.99, 0.79)]), line);
        }
        Icon::MergeDown => {
            p.line(poly(r, &[(0.50, 0.10), (0.50, 0.52)]), line);
            p.add(Shape::convex_polygon(
                poly(r, &[(0.50, 0.72), (0.30, 0.50), (0.70, 0.50)]),
                color,
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
            p.line(poly(r, &[(0.28, 0.42), (0.56, 0.42)]), thin);
            if icon == Icon::ZoomIn {
                p.line(poly(r, &[(0.42, 0.28), (0.42, 0.56)]), thin);
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
            p.line(poly(r, &[(0.26, 0.44), (0.26, 0.30), (0.40, 0.30)]), thin);
            p.line(poly(r, &[(0.74, 0.56), (0.74, 0.70), (0.60, 0.70)]), thin);
        }
        Icon::Zoom100 => {
            // "1:1"
            p.line(poly(r, &[(0.22, 0.30), (0.22, 0.74)]), line);
            p.circle_filled(pt(r, 0.48, 0.42), w * 0.06, color);
            p.circle_filled(pt(r, 0.48, 0.64), w * 0.06, color);
            p.line(poly(r, &[(0.74, 0.30), (0.74, 0.74)]), line);
        }
        Icon::Close => {
            p.line(poly(r, &[(0.22, 0.22), (0.78, 0.78)]), line);
            p.line(poly(r, &[(0.78, 0.22), (0.22, 0.78)]), line);
        }
        Icon::Book => {
            // An open book: two page outlines around a spine (the reader).
            p.line(poly(r, &[(0.50, 0.28), (0.50, 0.76)]), line);
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
            let seg = |a: egui::Pos2, b: egui::Pos2| p.line_segment([a, b], thin);
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
            p.rect_filled(rect(r, 0.18, 0.18, 0.50, 0.82), 0.0, color);
        }
        Icon::SelExpand => {
            p.rect_stroke(
                rect(r, 0.30, 0.30, 0.70, 0.70),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.line(poly(r, &[(0.50, 0.24), (0.50, 0.06)]), thin);
            p.line(poly(r, &[(0.50, 0.76), (0.50, 0.94)]), thin);
            p.line(poly(r, &[(0.24, 0.50), (0.06, 0.50)]), thin);
            p.line(poly(r, &[(0.76, 0.50), (0.94, 0.50)]), thin);
        }
        Icon::SelShrink => {
            p.rect_stroke(
                rect(r, 0.14, 0.14, 0.86, 0.86),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.line(poly(r, &[(0.50, 0.14), (0.50, 0.34)]), thin);
            p.line(poly(r, &[(0.50, 0.86), (0.50, 0.66)]), thin);
            p.line(poly(r, &[(0.14, 0.50), (0.34, 0.50)]), thin);
            p.line(poly(r, &[(0.86, 0.50), (0.66, 0.50)]), thin);
        }
        Icon::SelClearOutside => {
            p.rect_stroke(
                rect(r, 0.12, 0.28, 0.72, 0.88),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            // X just outside the box's top-right corner.
            p.line(poly(r, &[(0.76, 0.12), (0.94, 0.30)]), thin);
            p.line(poly(r, &[(0.94, 0.12), (0.76, 0.30)]), thin);
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
            p.line(
                poly(r, &[(0.24, 0.76), (0.50, 0.56), (0.86, 0.20)]),
                line,
            );
            p.circle_filled(pt(r, 0.86, 0.20), w * 0.09, color);
        }
        Icon::SelTransform => {
            p.rect_stroke(
                rect(r, 0.40, 0.40, 0.60, 0.60),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.line(poly(r, &[(0.50, 0.34), (0.50, 0.08)]), thin);
            p.line(poly(r, &[(0.50, 0.66), (0.50, 0.92)]), thin);
            p.line(poly(r, &[(0.34, 0.50), (0.08, 0.50)]), thin);
            p.line(poly(r, &[(0.66, 0.50), (0.92, 0.50)]), thin);
        }
        Icon::SelCrop => {
            // Two overlapping corner brackets — the classic crop glyph.
            p.line(poly(r, &[(0.32, 0.08), (0.32, 0.68), (0.92, 0.68)]), line);
            p.line(poly(r, &[(0.08, 0.32), (0.68, 0.32), (0.68, 0.92)]), line);
        }
        Icon::SelCut => {
            // Scissors: two blades crossing, two ring handles.
            p.line(poly(r, &[(0.20, 0.12), (0.72, 0.78)]), thin);
            p.line(poly(r, &[(0.20, 0.78), (0.72, 0.12)]), thin);
            p.circle_stroke(pt(r, 0.18, 0.86), w * 0.09, thin);
            p.circle_stroke(pt(r, 0.18, 0.14), w * 0.09, thin);
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
                thin,
            );
        }
        Icon::SelPaste => {
            // A clipboard: board, clip on top, a dropped patch on it.
            p.rect_stroke(
                rect(r, 0.16, 0.14, 0.84, 0.90),
                1.0,
                thin,
                egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                rect(r, 0.38, 0.06, 0.62, 0.20),
                1.0,
                thin,
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
            p.circle_filled(poly(r, &[(0.20, 0.80)])[0], w * 0.09, color);
            p.circle_filled(poly(r, &[(0.80, 0.20)])[0], w * 0.09, color);
        }
        Icon::Rect => {
            p.rect_stroke(
                rect(r, 0.18, 0.24, 0.82, 0.76),
                0.0,
                line,
                egui::StrokeKind::Inside,
            );
        }
        Icon::Ellipse => {
            p.circle_stroke(r.center(), w * 0.30, line);
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
            p.add(egui::Shape::closed_line(pts, line));
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
            p.rect_stroke(g, 0.0, thin, egui::StrokeKind::Inside);
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
            for (x, y) in [(0.12, 0.80), (0.88, 0.72)] {
                p.rect_filled(
                    Rect::from_center_size(pt(r, x, y), Vec2::splat(w * 0.22)),
                    0.0,
                    color,
                );
            }
        }
        Icon::Tone => {
            // 3x3 halftone grid, dots growing toward the bottom-right so the
            // glyph reads as a density ramp rather than a dice face.
            for (row, y) in [0.22_f32, 0.50, 0.78].into_iter().enumerate() {
                for (col, x) in [0.22_f32, 0.50, 0.78].into_iter().enumerate() {
                    let t = (row + col) as f32 / 4.0;
                    p.circle_filled(pt(r, x, y), w * (0.055 + 0.085 * t), color);
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
            p.line(poly(r, &[(0.62, 0.10), (0.62, 0.30), (0.82, 0.30)]), thin);
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
        Icon::ChevronLeft => {
            p.line(poly(r, &[(0.62, 0.16), (0.32, 0.50), (0.62, 0.84)]), line);
        }
        Icon::ChevronRight => {
            p.line(poly(r, &[(0.38, 0.16), (0.68, 0.50), (0.38, 0.84)]), line);
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
fn arc_arrow(
    p: &Painter,
    r: Rect,
    centre: (f32, f32),
    rad: f32,
    a0: f32,
    a1: f32,
    stroke: Stroke,
    color: Color32,
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
        color,
        Stroke::NONE,
    ));
}

/// The transparent-colour tile: a small checkerboard, exactly what CSP draws in
/// the third colour slot.
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
