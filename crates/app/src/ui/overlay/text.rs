//! Text overlay: the wrap-box drag, the edited/selected box with its
//! handles, the multi-selection boxes, the caret and its selection
//! highlight. Moved here verbatim when `overlay.rs` was split by Z-order
//! band.

use super::super::theme;
use crate::app::App;
use crate::cmd::Tool;

/// Z-order band 11: everything the Text tool and the Object tool's text
/// selection draw.
pub(super) fn paint(
    ui: &egui::Ui,
    app: &App,
    painter: &egui::Painter,
    to_pt: &dyn Fn(f32, f32) -> egui::Pos2,
    ants: &dyn Fn(&[(f32, f32)], (f32, f32), egui::Color32),
) {
    // Text: wrap-box drag preview, the edited/selected box with its handles,
    // the caret, and the selection highlight.
    if let Some(crate::text_edit::TextGesture::Box { start, cur }) = &app.text_gesture {
        let col = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 160);
        ants(
            &[*start, (cur.0, start.1), *cur, (start.0, cur.1)],
            (0.0, 0.0),
            col,
        );
    }
    let rot_off = crate::app::ROTATE_STALK_SCREEN / app.viewport.zoom.max(0.01);
    let text_shown: Option<(mn_core::TextItem, bool)> = if let Some(d) = &app.text_obj_drag {
        Some((d.preview(), true))
    } else if app.tool == Tool::Object {
        // Rows 78/76: the multi-selection set — a thin accent box on
        // every member (the primary keeps its full affordances below).
        // While a group drag is live the boxes ride the delta, so the
        // whole set visibly moves together before anything commits.
        let gd = app
            .group_drag
            .as_ref()
            .map(|d| (d.cur.0 - d.start.0, d.cur.1 - d.start.1));
        for r in &app.object_multi {
            let bb: Option<[f32; 4]> = match *r {
                crate::app::ObjRef::Text(li, ti) => app
                    .doc
                    .layers
                    .get(li)
                    .and_then(|l| l.texts())
                    .and_then(|ts| ts.texts.get(ti))
                    .map(|t| {
                        let c = t.center();
                        [c[0] - t.size[0] * 0.5, c[1] - t.size[1] * 0.5,
                         c[0] + t.size[0] * 0.5, c[1] + t.size[1] * 0.5]
                    }),
                crate::app::ObjRef::Balloon(li, bi) => app
                    .doc
                    .layers
                    .get(li)
                    .and_then(|l| l.balloons())
                    .and_then(|bs| bs.balloons.get(bi))
                    .map(|b| b.bbox()),
                crate::app::ObjRef::Frame(li, fi) => {
                    // Whole-panel box — the folder's own frame polygon.
                    app.doc
                        .layers
                        .get(li)
                        .and_then(|l| l.frames())
                        .and_then(|fs| fs.frames.get(fi))
                        .map(|f| f.bbox())
                }
                crate::app::ObjRef::Gen(li) => {
                    // Focus runs carry a centre and an outer radius.
                    app.doc
                        .layers
                        .get(li)
                        .and_then(|l| l.genlines.clone())
                        .and_then(|s| {
                            (s.focus && s.d > 0.0).then(|| {
                                [s.a - s.d, s.b - s.d, s.a + s.d, s.b + s.d]
                            })
                        })
                }
            };
            if let Some(mut b) = bb {
                if let Some((dx, dy)) = gd {
                    b = [b[0] + dx, b[1] + dy, b[2] + dx, b[3] + dy];
                }
                painter.rect_stroke(
                    egui::Rect::from_min_max(to_pt(b[0], b[1]), to_pt(b[2], b[3])),
                    2.0,
                    egui::Stroke::new(1.5, theme::c().accent),
                    egui::StrokeKind::Middle,
                );
            }
        }
        app.text_sel.and_then(|(li, ti)| {
            Some((
                app.doc.layers.get(li)?.texts()?.texts.get(ti)?.clone(),
                true,
            ))
        })
    } else {
        app.edited_item().map(|i| (i.clone(), false))
    };
    if let Some((item, with_handles)) = text_shown {
        let mut pts: Vec<egui::Pos2> = item.corners().iter().map(|p| to_pt(p[0], p[1])).collect();
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
        painter.add(egui::Shape::line(
            pts,
            egui::Stroke::new(1.2, theme::c().accent),
        ));
        if with_handles {
            for (pos, h) in item.handles(rot_off) {
                let c = to_pt(pos[0], pos[1]);
                if h == mn_core::TextHandle::Rotate {
                    // Stem from the top edge to the lollipop.
                    let top = item.to_canvas([item.size[0] * 0.5, 0.0]);
                    painter.line_segment(
                        [to_pt(top[0], top[1]), c],
                        egui::Stroke::new(1.0, theme::c().accent),
                    );
                    painter.circle_filled(c, 4.0, egui::Color32::WHITE);
                    painter.circle_stroke(c, 4.0, egui::Stroke::new(1.2, theme::c().accent));
                } else {
                    let hrect = egui::Rect::from_center_size(c, egui::vec2(7.0, 7.0));
                    painter.rect_filled(hrect, 1.0, egui::Color32::WHITE);
                    painter.rect_stroke(
                        hrect,
                        1.0,
                        egui::Stroke::new(1.2, theme::c().accent),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
    }
    if let Some(ov) = app.text_caret_overlay() {
        for quad in &ov.selection {
            let pts: Vec<egui::Pos2> = quad.iter().map(|p| to_pt(p[0], p[1])).collect();
            painter.add(egui::Shape::convex_polygon(
                pts,
                egui::Color32::from_rgba_unmultiplied(110, 150, 240, 70),
                egui::Stroke::NONE,
            ));
        }
        // THE CARET BLINKS AND IS VISIBLE ON WHITE (owner report,
        // 2026-08-19: it was near-white — `from_gray(245)` — on a white
        // page, and it never blinked, so there was nothing to catch the eye
        // even where it did show).
        //
        // Two strokes, dark over light: a manga page is white where the
        // caret usually sits and black where the ink is, so a single colour
        // disappears against one of them. The halo is the same trick the
        // brush-size ring below uses.
        //
        // The blink is driven by egui's own clock (~1.06 s period, close to
        // the Windows default) and asks for a repaint at the next phase
        // change — without that request an idle window would freeze the
        // caret in whichever half it last drew, which is worse than not
        // blinking at all.
        let t = ui.input(|i| i.time);
        const BLINK: f64 = 1.06;
        let phase = t.rem_euclid(BLINK);
        let on = phase < BLINK * 0.5;
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(
                (if on { BLINK * 0.5 } else { BLINK }) - phase + 0.005,
            ));
        if on {
            let [a, b] = ov.caret;
            let (pa, pb) = (to_pt(a[0], a[1]), to_pt(b[0], b[1]));
            painter.line_segment(
                [pa, pb],
                egui::Stroke::new(3.2, egui::Color32::from_white_alpha(190)),
            );
            painter.line_segment(
                [pa, pb],
                egui::Stroke::new(1.4, egui::Color32::from_black_alpha(235)),
            );
        }
    }
}
