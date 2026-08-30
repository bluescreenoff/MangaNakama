//! The Transform tool's overlay: the source veil, the floated preview (mesh
//! or affine), the bbox with its handles, the rotate stalk and the
//! reference point. Moved here verbatim when `overlay.rs` was split by
//! Z-order band.

use super::super::theme;
use super::{dim_readout, draw_dim_readout, extent_of};
use crate::app::App;

/// Z-order band 15: the transform preview and its affordances.
///
/// Returns TRUE when the mesh path took its early exit — the one place this
/// overlay stops painting before the end, so the caller returns too and the
/// bands after this one stay unpainted exactly as they were.
pub(super) fn paint(
    app: &App,
    painter: &egui::Painter,
    canvas_pts: egui::Rect,
    to_pt: &dyn Fn(f32, f32) -> egui::Pos2,
    cursor_pt: &dyn Fn() -> egui::Pos2,
) -> bool {
    // Transform: veil the vacated source region, float the transformed
    // preview over it, then the bbox + corner handles on top.
    if let Some(drag) = &app.transform_drag {
        // IO-081: the TRANSFORMED bounds, live, while a handle is held —
        // the affine box measures along its own (rotated) edges, so a
        // rotated float still reports the size it will commit at, not its
        // screen-aligned envelope. A mesh has no such edges: its lattice's
        // AABB is the honest answer. Drawn at each of this block's two
        // exits, because the mesh path returns early.
        let readout = drag.gesture.is_some().then(|| {
            let (w, h) = match &drag.mesh {
                Some(m) => extent_of(m.pts.iter().map(|p| (p[0], p[1]))),
                None => {
                    let d = |a: [f32; 2], b: [f32; 2]| (b[0] - a[0]).hypot(b[1] - a[1]);
                    (d(drag.bbox[0], drag.bbox[1]), d(drag.bbox[1], drag.bbox[2]))
                }
            };
            dim_readout(w, h, app.work_dpi())
        });
        let r = drag.source.rect;
        let veil = [
            to_pt(r[0] as f32, r[1] as f32),
            to_pt(r[2] as f32, r[1] as f32),
            to_pt(r[2] as f32, r[3] as f32),
            to_pt(r[0] as f32, r[3] as f32),
        ];
        if !drag.is_identity() || drag.mesh.is_some() {
            // Dim what is being lifted out — reads as "this region is
            // floating now" without touching the layer pixels.
            painter.add(egui::Shape::convex_polygon(
                veil.to_vec(),
                egui::Color32::from_white_alpha(110),
                egui::Stroke::NONE,
            ));
        }
        // Row 53 — mesh mode: the preview renders through the DEFORMED
        // quads (one textured triangle pair per cell), with the lattice
        // lines and draggable points on top. The affine path is untouched
        // below.
        if let Some(m) = &drag.mesh {
            if let Some(tex) = &drag.preview_tex {
                let k = (m.n - 1) as f32;
                let mut mesh = egui::Mesh {
                    texture_id: tex.id(),
                    indices: Vec::new(),
                    vertices: Vec::new(),
                };
                for cj in 0..m.n - 1 {
                    for ci in 0..m.n - 1 {
                        let corner = |i: usize, j: usize| {
                            let p = m.pts[j * m.n + i];
                            let uv = egui::pos2(i as f32 / k, j as f32 / k);
                            (
                                egui::epaint::Vertex {
                                    pos: to_pt(p[0], p[1]),
                                    uv,
                                    color: egui::Color32::WHITE,
                                },
                                [p[0], p[1]],
                            )
                        };
                        let (v00, p00) = corner(ci, cj);
                        let (v10, p10) = corner(ci + 1, cj);
                        let (v01, p01) = corner(ci, cj + 1);
                        let (v11, p11) = corner(ci + 1, cj + 1);
                        let b = mesh.vertices.len() as u32;
                        mesh.vertices.extend_from_slice(&[v00, v10, v01, v11]);
                        mesh.indices.extend_from_slice(&[b, b + 1, b + 2, b + 1, b + 3, b + 2]);
                        // The lattice lines, straight from the same corners.
                        for (a, q) in [(p00, p10), (p00, p01), (p10, p11), (p01, p11)] {
                            painter.line_segment(
                                [to_pt(a[0], a[1]), to_pt(q[0], q[1])],
                                egui::Stroke::new(1.0, theme::c().accent),
                            );
                        }
                    }
                }
                painter.add(egui::Shape::Mesh(mesh.into()));
            }
            let active = match drag.gesture.as_ref().map(|g| g.grab) {
                Some(crate::app::TransformGrab::MeshPoint(i)) => Some(i),
                _ => None,
            };
            for (i, p) in m.pts.iter().enumerate() {
                let half = if active == Some(i) { 4.5 } else { 3.0 };
                let rect = egui::Rect::from_center_size(
                    to_pt(p[0], p[1]),
                    egui::vec2(half * 2.0, half * 2.0),
                );
                painter.circle_filled(to_pt(p[0], p[1]), half, theme::c().accent);
                painter.circle_stroke(
                    to_pt(p[0], p[1]),
                    half,
                    egui::Stroke::new(1.0, egui::Color32::WHITE),
                );
                let _ = rect;
            }
            // PUPPET (row 54): pearls at the pins' current spots, a thin
            // line from where each was dropped.
            let active_pin = match drag.gesture.as_ref().map(|g| g.grab) {
                Some(crate::app::TransformGrab::PuppetPin(i)) => Some(i),
                _ => None,
            };
            for (i, pin) in m.pins.iter().enumerate() {
                if pin.orig != pin.cur {
                    painter.line_segment(
                        [to_pt(pin.orig[0], pin.orig[1]), to_pt(pin.cur[0], pin.cur[1])],
                        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(140)),
                    );
                }
                let r = if active_pin == Some(i) { 5.0 } else { 4.0 };
                painter.circle_filled(to_pt(pin.cur[0], pin.cur[1]), r, egui::Color32::WHITE);
                painter.circle_stroke(
                    to_pt(pin.cur[0], pin.cur[1]),
                    r,
                    egui::Stroke::new(2.0, theme::c().accent),
                );
            }
            if let Some(text) = readout {
                draw_dim_readout(&painter, canvas_pts, cursor_pt(), text);
            }
            // The one early exit: TRUE tells the caller to stop the overlay
            // here, which is what the bare `return` did before the split.
            return true;
        }
        if let Some(tex) = &drag.preview_tex {
            let pts: Vec<egui::Pos2> = drag.bbox.iter().map(|c| to_pt(c[0], c[1])).collect();
            let uvs = [
                egui::pos2(0.0, 0.0),
                egui::pos2(1.0, 0.0),
                egui::pos2(1.0, 1.0),
                egui::pos2(0.0, 1.0),
            ];
            let mesh = egui::Mesh {
                texture_id: tex.id(),
                indices: vec![0, 1, 2, 0, 2, 3],
                vertices: pts
                    .iter()
                    .zip(uvs)
                    .map(|(pos, uv)| egui::epaint::Vertex {
                        pos: *pos,
                        uv,
                        color: egui::Color32::WHITE,
                    })
                    .collect(),
            };
            painter.add(egui::Shape::Mesh(mesh.into()));
        }
        let pts: Vec<egui::Pos2> = drag.bbox.iter().map(|c| to_pt(c[0], c[1])).collect();
        let mut line = pts.clone();
        line.push(pts[0]);
        painter.add(egui::Shape::line(
            line,
            egui::Stroke::new(1.5, theme::c().accent),
        ));
        // Corner handles (squares, CSP-style); the grabbed one is larger.
        let active_corner = match drag.gesture.as_ref().map(|g| g.grab) {
            Some(crate::app::TransformGrab::Corner(i)) => Some(i),
            _ => None,
        };
        for (i, pt) in pts.iter().enumerate() {
            let half = if active_corner == Some(i) { 5.0 } else { 4.0 };
            let rect = egui::Rect::from_center_size(*pt, egui::vec2(half * 2.0, half * 2.0));
            painter.rect_filled(rect, 1.0, theme::c().accent);
            painter.rect_stroke(
                rect,
                1.0,
                egui::Stroke::new(1.0, egui::Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }
        // TR-004: edge-midpoint handles (one-axis scale), smaller and
        // white-filled so they read differently from the corners.
        let active_edge = match drag.gesture.as_ref().map(|g| g.grab) {
            Some(crate::app::TransformGrab::Edge(i)) => Some(i),
            _ => None,
        };
        for i in 0..4 {
            let (a, b) = (drag.bbox[i], drag.bbox[(i + 1) % 4]);
            let m = egui::pos2((a[0] + b[0]) as f32 * 0.5, (a[1] + b[1]) as f32 * 0.5);
            let m = to_pt(m.x, m.y);
            let half = if active_edge == Some(i) { 4.5 } else { 3.5 };
            let rect = egui::Rect::from_center_size(m, egui::vec2(half * 2.0, half * 2.0));
            painter.rect_filled(rect, 1.0, egui::Color32::WHITE);
            painter.rect_stroke(
                rect,
                1.0,
                egui::Stroke::new(1.0, theme::c().accent),
                egui::StrokeKind::Inside,
            );
        }
        // The rotate stalk, same lollipop as frames/balloons/text: rotation
        // is THIS handle (plus dragging outside the box), never a corner.
        // Its point comes from the hit test's own helper.
        let stalk = drag.stalk_point(app.viewport.zoom);
        let stalk = to_pt(stalk[0], stalk[1]);
        let top = egui::pos2(
            (drag.bbox[0][0] + drag.bbox[1][0]) * 0.5,
            (drag.bbox[0][1] + drag.bbox[1][1]) * 0.5,
        );
        let grabbed_stalk = matches!(
            drag.gesture.as_ref().map(|g| g.grab),
            Some(crate::app::TransformGrab::Rotate)
        );
        painter.line_segment(
            [to_pt(top.x, top.y), stalk],
            egui::Stroke::new(1.2, theme::c().accent),
        );
        let r = if grabbed_stalk { 5.5 } else { 4.5 };
        painter.circle_filled(stalk, r, theme::c().panel);
        painter.circle_stroke(stalk, r, egui::Stroke::new(1.4, theme::c().accent));
        // TR-003: the reference point — a cross with a centre dot.
        let pv = drag.pivot();
        let pv = to_pt(pv[0], pv[1]);
        let arm = 8.0;
        for d in [egui::vec2(1.0, 0.0), egui::vec2(0.0, 1.0)] {
            painter.line_segment(
                [pv - d * arm, pv + d * arm],
                egui::Stroke::new(1.5, theme::c().accent),
            );
        }
        painter.circle_filled(pv, 2.0, theme::c().accent);
        if let Some(text) = readout {
            draw_dim_readout(&painter, canvas_pts, cursor_pt(), text);
        }
    }
    false
}
