//! The two "show me the area" tints — LM-008 mask area and TN-011 tone
//! area. Moved here verbatim when `overlay.rs` was split by Z-order band.

use crate::app::App;

/// Z-order band 14: the mask and tone tints, over the art and under the
/// transform preview.
pub(super) fn paint(app: &App, painter: &egui::Painter, to_pt: &dyn Fn(f32, f32) -> egui::Pos2) {
    // The two "show me the area" tints (LM-008 mask, TN-011 tone) draw the
    // same way: canvas-space quads, because the view rotates and mirrors so
    // an axis-aligned rect on screen would be wrong. One `Shape` per RUN of
    // pixels rather than one painter call per pixel — the mask version used
    // to emit a 1 px `line_segment` per pixel of every mask tile every frame,
    // which is a painter call per pixel on the UI thread.
    let quad = |x0: f32, y0: f32, x1: f32, y1: f32, col: egui::Color32| {
        egui::Shape::convex_polygon(
            vec![to_pt(x0, y0), to_pt(x1, y0), to_pt(x1, y1), to_pt(x0, y1)],
            col,
            egui::Stroke::NONE,
        )
    };
    // LM-008: Show Mask Area — a purple tint over the ACTIVE layer's
    // masked-off region (coverage tiles; absent tile = hidden too).
    if app.mask_show_area
        && let Some(m) = app.doc.active_layer().mask.as_ref().filter(|m| m.enabled)
    {
        let col = egui::Color32::from_rgba_premultiplied(60, 0, 90, 70);
        let mut shapes: Vec<egui::Shape> = Vec::new();
        for (idx, t) in &m.tiles {
            let (ox, oy) = idx.origin();
            for py in 0..64usize {
                // Run-length the row: a mask is mostly flat, so the typical
                // row is one or two runs instead of 64 shapes.
                let mut run: Option<usize> = None;
                for px in 0..=64usize {
                    let hidden = px < 64 && t.pixel(px, py)[3] < 32768;
                    match (hidden, run) {
                        (true, None) => run = Some(px),
                        (false, Some(s)) => {
                            shapes.push(quad(
                                ox as f32 + s as f32,
                                oy as f32 + py as f32,
                                ox as f32 + px as f32,
                                oy as f32 + py as f32 + 1.0,
                                col,
                            ));
                            run = None;
                        }
                        _ => {}
                    }
                }
            }
        }
        painter.extend(shapes);
    }

    // TN-011: Show Tone Area — a green tint over every toned region of the
    // WHOLE stack (not just the active layer: the row exists to catch the
    // scrap of tone you forgot on some layer before it prints). Granularity
    // is the 64 px tile, which is deliberate — a 3 px scrap tints its whole
    // tile, and a scrap you can see is the entire point. Costs one
    // `is_blank()` probe per derived tile and one quad per tile that has
    // ink — and `is_blank` short-circuits on the first non-zero halfword, so
    // the tiles that matter cost a handful of reads. (A tile that really is
    // blank does scan; only toned layers are walked, and only while the
    // toggle is on.)
    if app.tone_show_area {
        let col = egui::Color32::from_rgba_premultiplied(0, 70, 55, 60);
        let vis = app.doc.effective_visibility();
        let mut shapes: Vec<egui::Shape> = Vec::new();
        for (li, l) in app.doc.layers.iter().enumerate() {
            let toned = l.tone.is_some()
                || matches!(
                    l.kind,
                    mn_core::LayerKind::Fill(mn_core::FillKind::Tone { .. })
                );
            if !toned || !vis.get(li).copied().unwrap_or(false) || l.opacity <= 0.0 {
                continue;
            }
            for (idx, t) in l.display_tiles() {
                if t.is_blank() {
                    continue;
                }
                let (ox, oy) = idx.origin();
                shapes.push(quad(
                    ox as f32,
                    oy as f32,
                    ox as f32 + 64.0,
                    oy as f32 + 64.0,
                    col,
                ));
            }
        }
        painter.extend(shapes);
    }
}
