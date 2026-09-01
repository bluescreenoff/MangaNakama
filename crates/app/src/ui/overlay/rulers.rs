//! The ruler families (TODO #3): line, vanishing point, the perspective
//! sets, the part-3 specials and the curve rulers, plus their Object-tool
//! handles. Moved here verbatim when `overlay.rs` was split by Z-order band.

use super::super::theme;
use crate::app::App;
use crate::cmd::Tool;

/// Z-order band 4: the rulers, over the page and under the selection.
pub(super) fn paint(
    ui: &egui::Ui,
    app: &App,
    painter: &egui::Painter,
    to_pt: &dyn Fn(f32, f32) -> egui::Pos2,
) {
    // TODO #3: rulers — cyan guide lines, clipped to the canvas. The gate
    // asks about EVERY family: a set holding only curve rulers (or only a
    // half-clicked one) drew nothing while this read `items` alone.
    if !app.doc.rulers.items.is_empty()
        || !app.doc.rulers.curves.is_empty()
        || app.curve_pending.is_some()
    {
        let col = egui::Color32::from_rgb(0, 200, 220);
        let far = 1.0e5;
        // Part 4 and the 1-/3-point variants share their furniture: the
        // eye level drawn strong through two points, and a vanishing point
        // marked by a faint 15° ray fan plus a cross.
        let eye_level = |a: [f32; 2], b: [f32; 2]| {
            let d = [b[0] - a[0], b[1] - a[1]];
            let n = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1.0);
            let u = [d[0] / n, d[1] / n];
            painter.line_segment(
                [
                    to_pt(a[0] - u[0] * far, a[1] - u[1] * far),
                    to_pt(b[0] + u[0] * far, b[1] + u[1] * far),
                ],
                egui::Stroke::new(1.5, col),
            );
        };
        // The continuum rulers draw SAMPLES of their family, not the
        // family: every angle (every radius) is reachable, so the marks
        // are faint to say "these are not the only ones".
        let faint = egui::Color32::from_rgba_unmultiplied(0, 200, 220, 60);
        let vp_mark = |vp: [f32; 2]| {
            for k in 0..24 {
                let ang = k as f32 * std::f32::consts::TAU / 24.0;
                let dd = [ang.cos(), ang.sin()];
                painter.line_segment(
                    [
                        to_pt(vp[0] + dd[0] * 24.0, vp[1] + dd[1] * 24.0),
                        to_pt(vp[0] + dd[0] * 400.0, vp[1] + dd[1] * 400.0),
                    ],
                    egui::Stroke::new(0.5, faint),
                );
            }
            let p = to_pt(vp[0], vp[1]);
            painter.line_segment(
                [egui::pos2(p.x - 5.0, p.y), egui::pos2(p.x + 5.0, p.y)],
                egui::Stroke::new(1.5, col),
            );
            painter.line_segment(
                [egui::pos2(p.x, p.y - 5.0), egui::pos2(p.x, p.y + 5.0)],
                egui::Stroke::new(1.5, col),
            );
        };
        // Row 149: attached rulers draw only on their layer.
        let active_items = app.doc.rulers.for_layer(app.doc.active).items;
        for r in &active_items {
            match *r {
                mn_core::Ruler::Line { a, b } => {
                    let d = [b[0] - a[0], b[1] - a[1]];
                    let n = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1.0);
                    let u = [d[0] / n, d[1] / n];
                    let p0 = [a[0] - u[0] * far, a[1] - u[1] * far];
                    let p1 = [a[0] + u[0] * far, a[1] + u[1] * far];
                    painter.line_segment(
                        [to_pt(p0[0], p0[1]), to_pt(p1[0], p1[1])],
                        egui::Stroke::new(1.0, col),
                    );
                }
                mn_core::Ruler::VanishingPoint { c, rays, angle0 } => {
                    let n = rays.max(1);
                    for i in 0..n {
                        let ang = angle0 + i as f32 * std::f32::consts::TAU / n as f32;
                        let d = [ang.cos(), ang.sin()];
                        painter.line_segment(
                            [
                                to_pt(c[0], c[1]),
                                to_pt(c[0] + d[0] * far, c[1] + d[1] * far),
                            ],
                            egui::Stroke::new(1.0, col),
                        );
                    }
                }
                // Part 4: the eye level strong, faint 15° guide fans from
                // each VP, and a cross on each VP.
                mn_core::Ruler::Perspective { a, b } => {
                    eye_level(a, b);
                    vp_mark(a);
                    vp_mark(b);
                }
                // One-point: the eye level runs through the single VP; the
                // far end is a HANDLE (a ring, not a cross — it is not a
                // vanishing point, it only tilts the horizon).
                mn_core::Ruler::Perspective1 { vp, h } => {
                    eye_level(vp, h);
                    vp_mark(vp);
                    painter.circle_stroke(to_pt(h[0], h[1]), 3.0, egui::Stroke::new(1.5, col));
                }
                // Three-point: the horizon pair plus the vertical VP off
                // it — all three are real vanishing points, so all three
                // get the fan and the cross.
                mn_core::Ruler::Perspective3 { a, b, z } => {
                    eye_level(a, b);
                    vp_mark(a);
                    vp_mark(b);
                    vp_mark(z);
                }
                // Part 3 special rulers. Parallel renders its direction
                // segment extended; concentric draws its rings (capped);
                // guides span the canvas; symmetric draws its N axes.
                mn_core::Ruler::Parallel { a, b } => {
                    let d = [b[0] - a[0], b[1] - a[1]];
                    let n = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1.0);
                    let u = [d[0] / n, d[1] / n];
                    painter.line_segment(
                        [
                            to_pt(a[0] - u[0] * 40.0, a[1] - u[1] * 40.0),
                            to_pt(b[0] + u[0] * 40.0, b[1] + u[1] * 40.0),
                        ],
                        egui::Stroke::new(2.0, col),
                    );
                }
                // The radial line ruler: a continuum, so it draws exactly
                // the faint sample fan a vanishing point draws, plus the
                // cross on the centre that IS the ruler's handle.
                mn_core::Ruler::Radial { c } => vp_mark(c),
                mn_core::Ruler::Concentric { c, dr } => {
                    // A FREE ring ruler (dr <= 0) has no spacing to draw:
                    // the rings shown are a fixed-pitch sample, drawn
                    // faint so they do not read as the only radii.
                    let free = dr <= 0.0;
                    let step = if free { 64.0 } else { dr };
                    let stroke = egui::Stroke::new(1.0, if free { faint } else { col });
                    let reach = ((c[0]).abs().max(c[0]) + (c[1]).abs().max(c[1]) + 2048.0).max(step);
                    for k in 1..=(reach / step.max(1.0)) as usize {
                        let r = k as f32 * step;
                        painter.circle_stroke(
                            to_pt(c[0], c[1]),
                            (r * app.viewport.zoom).max(1.0),
                            stroke,
                        );
                    }
                    // The centre mark.
                    painter.circle_stroke(to_pt(c[0], c[1]), 3.0, egui::Stroke::new(1.5, col));
                }
                mn_core::Ruler::Guide { horizontal, pos } => {
                    let (p0, p1) = if horizontal {
                        ([-1.0e4, pos], [1.0e4, pos])
                    } else {
                        ([pos, -1.0e4], [pos, 1.0e4])
                    };
                    painter.line_segment(
                        [to_pt(p0[0], p0[1]), to_pt(p1[0], p1[1])],
                        egui::Stroke::new(1.0, col),
                    );
                }
                mn_core::Ruler::Symmetric { c, lines, angle0 } => {
                    let n = lines.max(1);
                    for k in 0..n {
                        let ang = angle0 + k as f32 * std::f32::consts::PI / n as f32;
                        let d = [ang.cos(), ang.sin()];
                        painter.line_segment(
                            [
                                to_pt(c[0] - d[0] * far, c[1] - d[1] * far),
                                to_pt(c[0] + d[0] * far, c[1] + d[1] * far),
                            ],
                            egui::Stroke::new(1.0, col),
                        );
                    }
                    painter.circle_stroke(to_pt(c[0], c[1]), 3.0, egui::Stroke::new(1.5, col));
                }
            }
        }
        // Part 2: curve rulers (their finite path, not infinite lines) and
        // the in-progress curve being clicked out.
        for c in &app.doc.rulers.curves {
            for w in c.pts.windows(2) {
                painter.line_segment(
                    [to_pt(w[0][0], w[0][1]), to_pt(w[1][0], w[1][1])],
                    egui::Stroke::new(1.5, col),
                );
            }
        }
        if let Some(pts) = &app.curve_pending {
            for w in pts.windows(2) {
                painter.line_segment(
                    [to_pt(w[0][0], w[0][1]), to_pt(w[1][0], w[1][1])],
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 220, 235)),
                );
            }
        }
        // Rulers are movable with the Object tool, so under that tool they
        // wear the same handle every other on-canvas anchor does (white
        // square, accent outline). A guide has no anchor — its whole line
        // is the grab — so it shows none, which is the honest affordance.
        if app.tool == Tool::Object {
            let outline = egui::Stroke::new(1.2, theme::c().accent);
            let square = |c: egui::Pos2| {
                let r = egui::Rect::from_center_size(c, egui::vec2(7.0, 7.0));
                painter.rect_filled(r, 1.0, egui::Color32::WHITE);
                painter.rect_stroke(r, 1.0, outline, egui::StrokeKind::Inside);
            };
            // M3 phase A: the handle's SHAPE says what it is before the
            // label is read — a vanishing point is a diamond (the set
            // aims at it), the 1-pt set's horizon handle a circle (it
            // only tilts the eye level), every other anchor the plain
            // square every on-canvas affordance wears.
            let handle = |p: [f32; 2], role: mn_core::AnchorRole| {
                let c = to_pt(p[0], p[1]);
                match role {
                    mn_core::AnchorRole::Vp(_) | mn_core::AnchorRole::VerticalVp => {
                        let d = 5.5;
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                c + egui::vec2(0.0, -d),
                                c + egui::vec2(d, 0.0),
                                c + egui::vec2(0.0, d),
                                c + egui::vec2(-d, 0.0),
                            ],
                            egui::Color32::WHITE,
                            outline,
                        ));
                    }
                    mn_core::AnchorRole::Horizon => {
                        painter.circle(c, 4.0, egui::Color32::WHITE, outline);
                    }
                    _ => square(c),
                }
            };
            // …and the small tag spells it out ("VP1", "eye level"). Theme
            // type, set to the handle's RIGHT so it never covers the mark
            // it names — the labels teach the set, they don't compete with
            // the art.
            let font = egui::TextStyle::Small.resolve(ui.style());
            for r in &active_items {
                for (p, role) in r.anchors_with_roles() {
                    handle(p, role);
                    painter.text(
                        to_pt(p[0], p[1]) + egui::vec2(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        role.tag(),
                        font.clone(),
                        theme::c().accent,
                    );
                }
            }
            // A curve ruler's vertices are just path points — no role to
            // name, so they keep the plain square.
            for c in &app.doc.rulers.curves {
                for p in c.anchors() {
                    square(to_pt(p[0], p[1]));
                }
            }
        }
    }
}
