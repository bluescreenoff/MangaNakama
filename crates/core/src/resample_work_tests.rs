//! `IO-060` (workflow audit §10) — `Document::resample_to`, the core half of
//! Edit ▸ Change work resolution.
//!
//! The three claims worth pinning are the three the op could quietly get
//! wrong: line art must SURVIVE the reduction (that is what the kernel row
//! is for), a tone layer must RE-DERIVE rather than be filtered, and every
//! px-space number in the document must move while every PHYSICAL one holds
//! still.

use std::collections::HashMap;
use std::sync::Arc;

use crate::balloon::{Balloon, BalloonSet, BalloonShape, BalloonTone, Tail};
use crate::doc::{Document, Layer, LayerKind};
use crate::frame::{Frame, FrameSet};
use crate::genlines::GenLinesSpec;
use crate::ruler::Ruler;
use crate::stroke_set::{StrokeSet, VectorStroke};
use crate::text::{TextItem, TextSet};
use crate::tile::{FIX15_ONE, TILE_SIZE, Tile, TileIdx};
use crate::tone::{ToneDensity, ToneParams, TonePattern};
use crate::transform::Interp;

fn ink(doc: &Document, li: usize, x: i32, y: i32) -> u16 {
    let ti = TileIdx::of_pixel(x, y);
    doc.layers[li]
        .tile(ti)
        .map(|t| {
            t.pixel(
                (x - ti.x * TILE_SIZE as i32) as usize,
                (y - ti.y * TILE_SIZE as i32) as usize,
            )[3]
        })
        .unwrap_or(0)
}

fn paint(doc: &mut Document, li: usize, x: i32, y: i32, a: u16) {
    let ti = TileIdx::of_pixel(x, y);
    let (ox, oy) = ti.origin();
    doc.layers[li]
        .tile_mut(ti)
        .set_pixel((x - ox) as usize, (y - oy) as usize, [0, 0, 0, a]);
}

/// The kernel row's whole point, at WORK scale: a 1 px hairline drawn at
/// 600 dpi must still be somewhere on the page at 350.
///
/// Composite: the canvas is the asked size, the line is present in EVERY
/// row of the reduced page (not just most), it landed where the geometry
/// says it should, and bilinear — the kernel an artist would get without
/// the dropdown — is measurably worse at the same job. The last clause is
/// what makes `HighAccuracy` the default for this dialog rather than a
/// preference.
#[test]
fn a_hairline_survives_the_work_resample_and_bilinear_is_worse() {
    // 1 px vertical hairlines 11 px apart across the whole page — what a
    // hatching block or a background of 効果線 actually is. The spacing is
    // deliberately NOT commensurate with the 7:12 reduction: at 12 px
    // every line would land at the same phase and the two kernels would
    // agree by arithmetic accident.
    const LINES: usize = 53;
    let build = || {
        let mut doc = Document::new(600, 600);
        for k in 0..LINES {
            let x = 6 + 11 * k as i32;
            for y in 0..600 {
                paint(&mut doc, 0, x, y, FIX15_ONE as u16);
            }
        }
        doc
    };
    // The PEAK alpha of every surviving line (runs of inked columns at one
    // row). Peak, not presence: on a mono page the export re-thresholds at
    // half, so a line that came through at 15 % is a line that is going to
    // be gone by the time the printer sees it.
    let peaks = |d: &Document, w: i32| -> Vec<u16> {
        let mut out = Vec::new();
        let mut cur: Option<u16> = None;
        for x in 0..=w {
            let a = if x < w { ink(d, 0, x, 100) } else { 0 };
            match (a > 0, &mut cur) {
                (true, Some(m)) => *m = (*m).max(a),
                (true, None) => cur = Some(a),
                (false, _) => {
                    if let Some(m) = cur.take() {
                        out.push(m);
                    }
                }
            }
        }
        out
    };
    let half = FIX15_ONE as u16 / 2;

    let mut hi = build();
    assert!(hi.resample_to(350, 350, Interp::HighAccuracy));
    assert_eq!(hi.size, (350, 350), "the canvas is the asked size");
    let hp = peaks(&hi, 350);
    assert_eq!(
        hp.len(),
        LINES,
        "high accuracy brings every hairline through the 600 -> 350 dpi shrink"
    );

    let mut bi = build();
    assert!(bi.resample_to(350, 350, Interp::Bilinear));
    let bp = peaks(&bi, 350);
    assert_eq!(bp.len(), LINES, "at this gentle a scale bilinear loses none either");

    // So the difference is not presence, it is STRENGTH — and on a mono
    // page strength is presence, because the export re-thresholds at half.
    // High accuracy conserves the line's ink into whatever boxes it falls
    // in, so the weakest line is a third of full ink; bilinear weights it
    // by however near a sample centre happened to land, so its weakest is
    // a fifth (measured: 10922/32768 vs 7021/32768). Relational assertions
    // with the numbers named — a kernel change that moves either says so.
    let (hmin, bmin) = (*hp.iter().min().unwrap(), *bp.iter().min().unwrap());
    assert!(
        hmin >= FIX15_ONE as u16 / 3,
        "high accuracy's weakest line keeps at least a third of its ink, got {hmin}"
    );
    assert!(
        bmin < hmin * 3 / 4,
        "bilinear's weakest line is markedly fainter ({bmin} vs {hmin}) — which \
         is why THIS dialog defaults to high accuracy even though the Transform \
         tool defaults to bilinear"
    );
    // And the spread: an even hatching block must stay even, not turn into
    // a moiré of strong and weak lines.
    let spread = |v: &[u16]| v.iter().max().unwrap() - v.iter().min().unwrap();
    assert!(
        spread(&hp) < spread(&bp) / 2,
        "high accuracy keeps the block EVEN ({} vs {}); bilinear's uneven \
         weights are what a hatching block reads as moiré",
        spread(&hp),
        spread(&bp)
    );
    let _ = half;
}

/// The tone-awareness §10 asks for: a screen is NOT scaled like a picture.
///
/// One inch of paper carries the same number of lines before and after —
/// the layer is still 50 lpi — while the pitch in PIXELS moves with the
/// resolution, and the ink coverage (the "30 % tone" number) does not
/// drift. Measured off the derived raster, not off the parameters, so a
/// resample that quietly baked the old dots would fail here.
#[test]
fn a_tone_layer_rescreens_at_the_new_dpi_at_the_same_frequency() {
    // Vertical lines (angle 0 ⇒ the screen axis is x), so one horizontal
    // scanline crosses exactly one line per cell.
    let params = ToneParams {
        pattern: TonePattern::Lines,
        lpi: 50.0,
        angle_deg: 0.0,
        density: ToneDensity::Specified(0.5),
        ..ToneParams::default()
    };
    let mut doc = Document::new(600, 600);
    for y in 0..600 {
        for x in 0..600 {
            paint(&mut doc, 0, x, y, FIX15_ONE as u16);
        }
    }
    doc.layers[0].tone = Some(params);

    // (lines counted across the whole width, ink coverage) at a row.
    let measure = |d: &Document, w: i32| -> (usize, f32) {
        let l = &d.layers[0];
        let read = |x: i32, y: i32| -> u16 {
            let ti = TileIdx::of_pixel(x, y);
            l.display_tile(ti)
                .map(|t| {
                    t.pixel(
                        (x - ti.x * TILE_SIZE as i32) as usize,
                        (y - ti.y * TILE_SIZE as i32) as usize,
                    )[3]
                })
                .unwrap_or(0)
        };
        let y = w / 2;
        let (mut runs, mut on, mut prev) = (0usize, 0usize, false);
        for x in 0..w {
            let lit = read(x, y) > FIX15_ONE as u16 / 2;
            if lit {
                on += 1;
                if !prev {
                    runs += 1;
                }
            }
            prev = lit;
        }
        (runs, on as f32 / w as f32)
    };

    doc.refresh_derived(600);
    let (runs600, cover600) = measure(&doc, 600);
    let lpi600 = runs600 as f32 / (600.0 / 600.0);
    assert!(
        (lpi600 - 50.0).abs() <= 2.0,
        "the 600 dpi page screens at 50 lpi, got {lpi600}"
    );

    assert!(doc.resample_to(350, 350, Interp::HighAccuracy));
    doc.refresh_derived(350);
    let (runs350, cover350) = measure(&doc, 350);
    let lpi350 = runs350 as f32 / (350.0 / 350.0);
    assert!(
        (lpi350 - 50.0).abs() <= 2.0,
        "and still 50 lpi at 350 dpi — the FREQUENCY is physical, got {lpi350}"
    );
    // The pitch in px is what moved: 12 px per cell at 600, 7 at 350.
    let (pitch600, pitch350) = (600.0 / runs600 as f32, 350.0 / runs350 as f32);
    assert!(
        pitch600 > pitch350 + 3.0,
        "the dot pitch shrank with the resolution ({pitch600} px -> {pitch350} px)"
    );
    assert!(
        (cover600 - cover350).abs() < 0.08,
        "density held: {cover600} -> {cover350}"
    );
    assert_eq!(
        doc.layers[0].tone.map(|t| t.lpi),
        Some(50.0),
        "the layer's own frequency was never rewritten"
    );
}

/// Every pixel-space number moves; every physical one does not. One
/// composite assertion because the failure mode is a FORGOTTEN field, and a
/// per-field test would only ever catch the fields somebody remembered.
#[test]
fn the_resample_scales_px_geometry_and_leaves_physical_units_alone() {
    let mut doc = Document::new(600, 600);

    let mut frame = Layer::new("frames");
    frame.kind = LayerKind::Frame(FrameSet {
        frames: vec![Frame {
            points: vec![[100.0, 200.0], [400.0, 200.0], [400.0, 500.0]],
        }],
        border_px: 12.0,
        slot: Some([0.0, 0.0, 600.0, 600.0]),
        reading_pin: None,
        border_ruler: false,
        color: [0, 0, 0],
    });
    doc.layers.push(frame);

    let mut balloon = Layer::new("balloons");
    balloon.kind = LayerKind::Balloon(BalloonSet {
        balloons: vec![Balloon {
            shape: BalloonShape::Ellipse {
                center: [300.0, 240.0],
                radii: [120.0, 60.0],
            },
            tails: vec![Tail {
                base: [300.0, 300.0],
                tip: [340.0, 400.0],
                width: 20.0,
                bend: 0.25,
                ..Tail::default()
            }],
            fill_tone: Some(BalloonTone {
                cell_px: 10.0,
                ..BalloonTone::default()
            }),
            ..Balloon::default()
        }],
        border_px: 6.0,
        pressure_width: false,
    });
    doc.layers.push(balloon);

    let mut item = TextItem::new([120.0, 360.0], "Mincho".into(), 12.0, [0, 0, 0], true);
    item.size = [240.0, 120.0];
    item.letter_spacing_pt = 1.5;
    item.outline_px = 8.0;
    let mut text = Layer::new("text");
    text.kind = LayerKind::Text(TextSet {
        texts: vec![item],
        ..TextSet::default()
    });
    doc.layers.push(text);

    let mut inked = Layer::new("vector ink");
    inked.strokes = Some(StrokeSet {
        strokes: vec![VectorStroke {
            points: vec![(100.0, 100.0, 1.0, 0.0, 0.0, 0.0)],
            preset: "pen".into(),
            size_px: 24.0,
            color: [0, 0, 0],
            eraser: false,
            stabilizer: 0.0,
            width_scale: 1.0,
            settings: None,
        }],
    });
    inked.tone = Some(ToneParams {
        offset: [8.0, 16.0],
        ..ToneParams::default()
    });
    inked.edge = Some(crate::edge::EdgeParams {
        width_px: 10.0,
        ..crate::edge::EdgeParams::default()
    });
    inked.genlines = Some(GenLinesSpec {
        focus: true,
        a: 300.0,
        b: 300.0,
        c: 100.0,
        d: 400.0,
        width: 6.0,
        ..GenLinesSpec::default()
    });
    doc.layers.push(inked);

    doc.rulers.items.push(Ruler::Line {
        a: [60.0, 60.0],
        b: [540.0, 540.0],
    });
    doc.rulers.items.push(Ruler::Guide {
        horizontal: true,
        pos: 300.0,
    });

    // 600 -> 300 dpi: an exact half, so every expected number is exact and
    // a wrong-axis or forgotten-field bug cannot hide in rounding.
    assert!(doc.resample_to(300, 300, Interp::HighAccuracy));
    assert_eq!(doc.size, (300, 300));

    let LayerKind::Frame(fs) = &doc.layers[1].kind else {
        panic!("frame layer")
    };
    assert_eq!(fs.frames[0].points[1], [200.0, 100.0]);
    assert_eq!(fs.border_px, 6.0, "panel border is px");
    assert_eq!(fs.slot, Some([0.0, 0.0, 300.0, 300.0]));

    let LayerKind::Balloon(bs) = &doc.layers[2].kind else {
        panic!("balloon layer")
    };
    let b = &bs.balloons[0];
    let BalloonShape::Ellipse { center, radii } = b.shape else {
        panic!("ellipse")
    };
    assert_eq!(center, [150.0, 120.0]);
    assert_eq!(radii, [60.0, 30.0], "the RADII scale, not just the centre");
    assert_eq!(b.tails[0].tip, [170.0, 200.0]);
    assert_eq!(b.tails[0].width, 10.0);
    assert_eq!(
        b.tails[0].bend, 0.25,
        "bend is a fraction of the tail's own length — dimensionless"
    );
    assert_eq!(
        b.fill_tone.map(|t| t.cell_px),
        Some(5.0),
        "a balloon's screen cell is stored in PX and cannot re-flow itself"
    );
    assert_eq!(bs.border_px, 3.0);

    let LayerKind::Text(ts) = &doc.layers[3].kind else {
        panic!("text layer")
    };
    let t = &ts.texts[0];
    assert_eq!(t.pos, [60.0, 180.0]);
    assert_eq!(t.size, [120.0, 60.0], "the wrap box is px");
    assert_eq!(t.outline_px, 4.0, "フチ width is px");
    assert_eq!(
        t.size_pt, 12.0,
        "TYPE IS PHYSICAL: 12 pt prints 12 pt at either resolution"
    );
    assert_eq!(t.letter_spacing_pt, 1.5, "and so is its spacing");
    assert!(
        t.cache.is_none(),
        "the sprite was shaped at the old dpi and must be re-shaped"
    );

    let ink = &doc.layers[4];
    let st = &ink.strokes.as_ref().unwrap().strokes[0];
    assert_eq!((st.points[0].0, st.points[0].1), (50.0, 50.0));
    assert_eq!(st.size_px, 12.0, "the recorded nib width is px");
    assert_eq!(st.points[0].2, 1.0, "pressure is 0..1, not a length");
    assert_eq!(ink.tone.map(|t| t.offset), Some([4.0, 8.0]));
    assert_eq!(ink.edge.map(|e| e.width_px), Some(5.0));
    let g = ink.genlines.as_ref().unwrap();
    assert_eq!((g.a, g.b, g.c, g.d), (150.0, 150.0, 50.0, 200.0));
    assert_eq!(g.width, 3.0);

    assert_eq!(
        doc.rulers.items[0],
        Ruler::Line {
            a: [30.0, 30.0],
            b: [270.0, 270.0]
        },
        "a perspective grid built for the page stays on the page"
    );
    assert_eq!(
        doc.rulers.items[1],
        Ruler::Guide {
            horizontal: true,
            pos: 150.0
        }
    );
}

/// A speed-lines spec keeps an ANGLE in the same slot a focus spec keeps a
/// centre X. Multiplying all four fields would silently rotate every speed
/// set on the page, and the rotation would only show up the next time
/// somebody reopened the dialog.
#[test]
fn a_speed_lines_angle_is_not_scaled_like_a_length() {
    let mut doc = Document::new(600, 600);
    doc.layers[0].genlines = Some(GenLinesSpec {
        focus: false,
        kind: 0,
        a: 30.0, // degrees
        b: 100.0,
        c: 200.0,
        width: 8.0,
        gap_px: 12.0,
        ..GenLinesSpec::default()
    });
    assert!(doc.resample_to(300, 300, Interp::HighAccuracy));
    let g = doc.layers[0].genlines.as_ref().unwrap();
    assert_eq!(g.a, 30.0, "the angle is degrees, not pixels");
    assert_eq!((g.b, g.c), (50.0, 100.0), "the LENGTHS scale");
    assert_eq!((g.width, g.gap_px), (4.0, 6.0));
}

/// Enlarging is the same op with the ratio the other way up, and it must
/// not silently clip: art parked OUTSIDE the old canvas (a margin sketch,
/// a balloon hanging off the trim) scales with the page instead of being
/// trimmed away by a bound nobody asked for.
#[test]
fn the_resample_is_bounded_by_content_not_by_the_canvas() {
    let mut doc = Document::new(200, 200);
    // A blob well outside the canvas, to the right.
    for y in 300..310 {
        for x in 300..310 {
            paint(&mut doc, 0, x, y, FIX15_ONE as u16);
        }
    }
    assert!(doc.resample_to(400, 400, Interp::Bilinear));
    assert_eq!(doc.size, (400, 400));
    assert!(
        ink(&doc, 0, 610, 610) > 0,
        "off-canvas art doubled with everything else instead of vanishing"
    );
}

/// A resample is not an undo step, and the history it leaves behind must
/// not be one either: an undo stack recorded against 600 dpi tiles cannot
/// be replayed onto a 350 dpi canvas.
#[test]
fn the_resample_clears_the_history_and_the_selection() {
    let mut doc = Document::new(600, 600);
    doc.begin_op();
    paint(&mut doc, 0, 10, 10, FIX15_ONE as u16);
    doc.end_op();
    assert!(doc.can_undo(), "there is something to undo before the op");
    doc.selection = Some(crate::selection::Selection::from_rect(&doc, 0.0, 0.0, 100.0, 100.0));

    assert!(doc.resample_to(350, 350, Interp::HighAccuracy));
    assert!(!doc.can_undo(), "the history cannot survive a resolution change");
    assert!(doc.selection.is_none(), "nor can the marching ants");
}

/// The no-op and the refusals, so the dialog's guards and the core's agree.
#[test]
fn the_resample_refuses_a_degenerate_target() {
    let mut doc = Document::new(600, 600);
    assert!(doc.resample_to(600, 600, Interp::Bilinear), "same size is a no-op");
    assert_eq!(doc.size, (600, 600));
    assert!(doc.resample_to(0, 0, Interp::Bilinear), "clamped to 1x1, not a panic");
    assert_eq!(doc.size, (1, 1));
}

/// `resample_tile_map` on an empty layer must not invent tiles — a blank
/// layer costs nothing, which is what keeps a 20-page resample liveable.
#[test]
fn resampling_an_empty_tile_map_allocates_nothing() {
    let empty: HashMap<TileIdx, Arc<Tile>> = HashMap::new();
    assert!(
        crate::transform::resample_tile_map(&empty, 0.5, 0.5, Interp::HighAccuracy).is_empty()
    );
}
