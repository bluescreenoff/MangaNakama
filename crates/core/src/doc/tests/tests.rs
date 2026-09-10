use super::super::*;

/// Row 33: rasterizing a text layer keeps its RENDERED tiles and
/// drops the vector state; keep-original leaves the source beside
/// it; ONE structural undo restores the stack.
#[test]
fn convert_layer_rasterizes_and_undoes_in_one_step() {
    let mut doc = Document::new(128, 128);
    let t = crate::text::TextItem::new([10.0, 10.0], "Gothic".into(), 9.0, [0, 0, 0], true);
    let li = doc.add_text_layer("lettering", crate::text::TextSet { texts: vec![t] });
    assert!(matches!(doc.layers[li].kind, crate::doc::LayerKind::Speech(_)));
    let had_tiles = doc.layers[li].tiles().count() > 0;

    let ok = doc.convert_layer(
        li,
        true,
        Some(crate::doc::LayerExpression::Grey),
        None,
        true,
        Some("baked lettering".into()),
    );
    assert!(ok);
    assert!(matches!(doc.layers[li + 1].kind, crate::doc::LayerKind::Raster), "the copy is raster");
    assert!(matches!(doc.layers[li].kind, crate::doc::LayerKind::Speech(_)), "the original stays text");
    assert_eq!(doc.layers[li + 1].name, "baked lettering");
    assert_eq!(doc.layers[li + 1].expression, crate::doc::LayerExpression::Grey);
    assert_eq!(doc.layers[li + 1].tiles().count() > 0, had_tiles, "the rendered tiles came along");

    assert!(doc.undo(), "one undo");
    // A fresh document carries a base layer: the stack is back to
    // base + the original text layer.
    assert_eq!(doc.layers.len(), 2, "the copy is gone");
    assert!(matches!(doc.layers[1].kind, crate::doc::LayerKind::Speech(_)));

    // Replace mode: no new layer, the layer itself converts.
    let ok = doc.convert_layer(li, true, None, None, false, None);
    assert!(ok);
    assert!(matches!(doc.layers[li].kind, crate::doc::LayerKind::Raster));
}

/// Row 31: extraction keeps the DARK pixels as scaled-black lines
/// and drops the light ones; a fresh layer above; one undo.
#[test]
fn extract_lines_lifts_the_dark_ink() {
    let mut doc = Document::new(128, 128);
    let li = doc.add_layer("scan");
    let put = |doc: &mut Document, x: i32, y: i32, v: f32| {
        let idx = crate::tile::TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        let t = doc.layers[li].tile_mut(idx);
        let f = crate::blend::f32_to_fix15(v);
        t.set_pixel((x - ox) as usize, (y - oy) as usize, [f, f, f, crate::blend::f32_to_fix15(1.0)]);
    };
    put(&mut doc, 10, 10, 0.0); // a black line pixel
    put(&mut doc, 12, 10, 0.5); // mid grey
    put(&mut doc, 14, 10, 0.95); // paper
    let out = doc.extract_lines(li, 0.8).expect("lines extracted");
    assert_eq!(out, li + 1, "the new layer sits above");
    let get = |doc: &Document, x: i32, y: i32| -> u16 {
        let idx = crate::tile::TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        doc.layers[out]
            .tile_arc(idx)
            .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize)[3])
            .unwrap_or(0)
    };
    let black = get(&doc, 10, 10);
    let grey = get(&doc, 12, 10);
    assert_eq!(black, crate::blend::f32_to_fix15(1.0), "black is a full line");
    assert!(
        grey > crate::blend::f32_to_fix15(0.3) && grey < crate::blend::f32_to_fix15(0.4),
        "mid grey is a ~0.375 line: {grey}"
    );
    assert_eq!(get(&doc, 14, 10), 0, "paper dropped");
    assert!(doc.undo(), "one undo");
    assert_eq!(doc.layers.len(), 2, "the extraction layer is gone");
    assert_ne!(doc.layers[1].name, "Extracted lines");
}


use crate::tile::FIX15_ONE;

/// The ruler set is document state with ONE undo history: a recorded
/// gesture undoes to the exact set that was there before it, redoes to
/// the finished one, and a gesture that changed nothing records
/// nothing. Document-level, so it belongs to no layer.
#[test]
fn rulers_undo_and_redo_the_whole_set() {
    let mut doc = Document::new(64, 64);
    assert!(doc.rulers.items.is_empty(), "a fresh document has none");

    let before = doc.rulers.clone();
    doc.rulers.items.push(crate::ruler::Ruler::Line {
        a: [0.0, 0.0],
        b: [10.0, 0.0],
    });
    doc.rulers.on = true;
    assert!(doc.record_rulers(before, "Add ruler"));
    assert_eq!(doc.undo_labels(), ["Add ruler"]);

    // A gesture that ended where it started is not a step.
    let noop = doc.rulers.clone();
    assert!(!doc.record_rulers(noop, "Move ruler"));
    assert_eq!(doc.undo_labels().len(), 1);

    // Move it, then undo back to the exact geometry.
    let before = doc.rulers.clone();
    doc.rulers.items[0].translate([0.0, 25.0]);
    assert!(doc.record_rulers(before, "Move ruler"));
    assert!(doc.undo());
    assert_eq!(
        doc.rulers.items[0],
        crate::ruler::Ruler::Line {
            a: [0.0, 0.0],
            b: [10.0, 0.0]
        }
    );
    assert!(doc.redo(), "and redo puts the move back");
    assert_eq!(
        doc.rulers.items[0],
        crate::ruler::Ruler::Line {
            a: [0.0, 25.0],
            b: [10.0, 25.0]
        }
    );
    assert_eq!(doc.redo_labels(), Vec::<String>::new());

    // Undo the move AND the creation: back to nothing, snap switch and
    // all — the snapshot is the whole value.
    assert!(doc.undo());
    assert!(doc.undo());
    assert!(doc.rulers.items.is_empty());
    assert!(!doc.rulers.on);
}

/// Every mode survives a save/load, and no two share a name.
#[test]
fn every_blend_mode_round_trips_through_its_ora_name() {
    let mut seen = std::collections::HashSet::new();
    for b in Blend::ALL {
        let n = b.ora_name();
        assert!(seen.insert(n), "{n} is claimed by two modes ({b:?})");
        assert_eq!(Blend::from_ora_name(n), b, "{b:?} did not survive {n}");
        assert!(
            n.starts_with("svg:") || n.starts_with("mn:"),
            "{b:?} name {n} has no namespace"
        );
    }
    assert_eq!(seen.len(), Blend::ALL.len());
}

/// The brush-preset key spelling round-trips every mode too (the
/// `.myb` parser used to accept multiply/screen only), and keeps the
/// two legacy spellings plus the unknown-string fallback.
#[test]
fn every_blend_mode_round_trips_through_its_short_name() {
    for b in Blend::ALL {
        assert_eq!(Blend::from_short_name(b.short_name()), b, "{b:?}");
    }
    assert_eq!(Blend::from_short_name("multiply"), Blend::Multiply);
    assert_eq!(Blend::from_short_name("screen"), Blend::Screen);
    assert_eq!(Blend::from_short_name("from-a-newer-build"), Blend::Normal);
}

/// **Old files must load pixel-identically.** These fifteen names were
/// written into every .ora the owner saved before the part-3 modes
/// existed; they are a file format, not an implementation detail. A new
/// variant may append a name, never move one of these.
#[test]
fn the_pre_part3_ora_names_are_frozen() {
    for (b, n) in [
        (Blend::Normal, "svg:src-over"),
        (Blend::Multiply, "svg:multiply"),
        (Blend::Screen, "svg:screen"),
        (Blend::Darken, "svg:darken"),
        (Blend::Lighten, "svg:lighten"),
        (Blend::Add, "mn:add"),
        (Blend::Subtract, "mn:subtract"),
        (Blend::Overlay, "svg:overlay"),
        (Blend::SoftLight, "svg:soft-light"),
        (Blend::HardLight, "svg:hard-light"),
        (Blend::Difference, "svg:difference"),
        (Blend::Exclusion, "svg:exclusion"),
        (Blend::Hue, "svg:hue"),
        (Blend::Saturation, "svg:saturation"),
        (Blend::Color, "svg:color"),
    ] {
        assert_eq!(b.ora_name(), n, "{b:?} renamed — old files would shift");
        assert_eq!(Blend::from_ora_name(n), b, "{n} no longer loads as {b:?}");
    }
    // And the default is still Normal: an .ora with no composite-op, or
    // one we do not know, must not land on a part-3 mode.
    assert_eq!(Blend::default(), Blend::Normal);
    assert_eq!(Blend::from_ora_name("svg:plus-lighter"), Blend::Normal);
    assert_eq!(Blend::from_ora_name(""), Blend::Normal);
}

#[test]
fn gradient_paints_the_axis_and_clamps_outside() {
    let mut doc = Document::new(64, 8);
    // Ramp from black to white along x = 8..56.
    assert!(doc.paint_gradient(
        [8.0, 4.0],
        [56.0, 4.0],
        [0.0, 0.0, 0.0, 1.0],
        [1.0, 1.0, 1.0, 1.0]
    ));
    let px = |x: i32| -> u16 {
        let ti = TileIdx::of_pixel(x, 4);
        doc.layers[0]
            .tile(ti)
            .map(|t| t.pixel((x - ti.origin().0) as usize, 4)[0])
            .unwrap_or(0)
    };
    let alpha = |x: i32| -> u16 {
        let ti = TileIdx::of_pixel(x, 4);
        doc.layers[0]
            .tile(ti)
            .map(|t| t.pixel((x - ti.origin().0) as usize, 4)[3])
            .unwrap_or(0)
    };
    let one = FIX15_ONE as u16;
    assert_eq!(px(0), 0, "before the ramp: clamped to FROM (black)");
    assert_eq!(alpha(0), one, "clamped pixels are still opaque");
    assert_eq!(px(63), one, "beyond the ramp: clamped to TO (white)");
    assert_eq!(alpha(63), one);
    let near0 = px(9) as f32 / one as f32;
    let mid = px(32) as f32 / one as f32;
    let near1 = px(55) as f32 / one as f32;
    assert!(near0 < 0.2, "start dark ({near0})");
    assert!((mid - 0.5).abs() < 0.08, "midpoint half ({mid})");
    assert!(near1 > 0.8, "end bright ({near1})");
}

#[test]
fn gradient_alpha_fades_to_transparent() {
    let mut doc = Document::new(32, 4);
    assert!(doc.paint_gradient(
        [0.0, 2.0],
        [31.0, 2.0],
        [0.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0, 0.0]
    ));
    let a = |x: i32| -> u16 {
        let ti = TileIdx::of_pixel(x, 2);
        doc.layers[0]
            .tile(ti)
            .map(|t| t.pixel((x - ti.origin().0) as usize, 2)[3])
            .unwrap_or(0)
    };
    let one = FIX15_ONE as u16;
    assert!(a(1) as f32 / (one as f32) > 0.85, "opaque start");
    assert!(a(30) as f32 / (one as f32) < 0.15, "transparent end");
}

#[test]
fn fill_polygon_covers_interior_not_exterior() {
    let mut doc = Document::new(64, 64);
    // A diamond around (32,32).
    let pts = [[32.0, 12.0], [52.0, 32.0], [32.0, 52.0], [12.0, 32.0]];
    assert!(doc.fill_polygon(&pts, [1.0, 0.0, 0.0], 1.0));
    let a = |x: i32, y: i32| -> u16 {
        let ti = TileIdx::of_pixel(x, y);
        doc.layers[0]
            .tile(ti)
            .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize)[3])
            .unwrap_or(0)
    };
    assert_eq!(a(32, 32), FIX15_ONE as u16, "centre filled");
    assert!(a(32, 30) > FIX15_ONE as u16 / 2, "inside mostly filled");
    assert_eq!(a(2, 2), 0, "far outside untouched");
    assert_eq!(a(2, 32), 0, "left of the diamond untouched");
}

/// `G-004`. Repeat tiles the ramp outside the drag; Reverse ping-pongs;
/// "do not draw" leaves the outside byte-untouched — and does not even
/// ALLOCATE those tiles, which is what keeps the undo step small.
#[test]
fn gradient_edge_process_repeats_and_declines() {
    use crate::gradient::{EdgeProcess, Ramp};
    let val = |doc: &Document, x: i32| -> Option<u16> {
        let ti = TileIdx::of_pixel(x, 2);
        doc.layers[0]
            .tile(ti)
            .map(|t| t.pixel((x - ti.origin().0) as usize, 2)[0])
    };

    // Ramp black→white across x 0..16 of a 64-wide canvas.
    let mut rep = Document::new(64, 4);
    let mut ramp = Ramp::two([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]);
    ramp.opts.edge = EdgeProcess::Repeat;
    assert!(rep.paint_gradient_ramp([0.0, 2.0], [16.0, 2.0], &ramp));
    // x = 4 is a quarter in; x = 20 and x = 36 are the same quarter of
    // the next tiles. Within a rounding step of each other.
    let a = val(&rep, 4).unwrap() as i32;
    let b = val(&rep, 20).unwrap() as i32;
    let c = val(&rep, 36).unwrap() as i32;
    assert!((a - b).abs() < 64 && (a - c).abs() < 64, "{a} {b} {c}");
    assert!(a < 12000, "and it is genuinely the dark quarter: {a}");

    let mut rev = Document::new(64, 4);
    ramp.opts.edge = EdgeProcess::Reverse;
    assert!(rev.paint_gradient_ramp([0.0, 2.0], [16.0, 2.0], &ramp));
    // Ping-pong about the drag's END (x = 16), and pixels are sampled at
    // their CENTRES: 20.5 folds to 11.5, so x = 20 mirrors x = 11 — not
    // x = 12, which is the off-by-one this assertion is here to pin.
    let m = val(&rev, 20).unwrap() as i32;
    let mirror = val(&rev, 11).unwrap() as i32;
    assert!((m - mirror).abs() < 64, "reverse mirrors: {m} vs {mirror}");
    assert_ne!(
        m,
        val(&rev, 12).unwrap() as i32,
        "and the fold really is half a pixel off the integer grid"
    );

    let mut blank = Document::new(256, 4);
    blank.layers[0].tile_mut(TileIdx::of_pixel(200, 2)); // pre-existing
    let before = blank.layers[0].tile_count();
    ramp.opts.edge = EdgeProcess::Blank;
    assert!(blank.paint_gradient_ramp([0.0, 2.0], [16.0, 2.0], &ramp));
    assert_eq!(
        val(&blank, 200),
        Some(0),
        "outside the drag: not drawn, not cleared"
    );
    assert!(val(&blank, 8).is_some_and(|v| v > 0), "inside still paints");
    assert!(
        blank.layers[0].tile_count() < before + 3,
        "a declined tile must not be allocated (undo pays for those)"
    );
}

/// The destructive tool writes PREMULTIPLIED pixels, like every other
/// paint op — and therefore agrees with the LIVE gradient layer built
/// from the same parameters. Before this, a mid-grey fading to
/// transparent was written straight beside a faded alpha, which reads
/// back as an over-bright colour.
#[test]
fn gradient_premultiplies_and_matches_the_live_layer() {
    use crate::fill_layer::FillKind;
    let grey = [0.5, 0.5, 0.5, 1.0];
    let clear = [0.5, 0.5, 0.5, 0.0];

    let mut baked = Document::new(64, 4);
    assert!(baked.paint_gradient([0.0, 2.0], [63.0, 2.0], grey, clear));

    let mut live = Document::new(64, 4);
    let li = live.add_fill_layer(
        FillKind::Gradient {
            a: [0.0, 2.0],
            b: [63.0, 2.0],
            from: grey,
            to: clear,
            mid: Default::default(),
            opts: Default::default(),
        },
        false,
    );
    live.refresh_derived(600);

    let ti = TileIdx::new(0, 0);
    let bt = baked.layers[0].tile(ti).expect("the ramp painted");
    let lt = live.layers[li].display_tile(ti).expect("the fill derived");
    for x in [1usize, 8, 16, 31, 47, 62] {
        let p = bt.pixel(x, 2);
        let q = lt.pixel(x, 2);
        for k in 0..3 {
            assert!(
                p[k] <= p[3],
                "x={x}: premultiplied means colour <= alpha, got {p:?}"
            );
            // Both paths quantize once; a rounding LSB apart is fine.
            assert!(
                (p[k] as i32 - q[k] as i32).abs() <= 2,
                "x={x} ch{k}: baked {p:?} vs live {q:?}"
            );
        }
        assert!((p[3] as i32 - q[3] as i32).abs() <= 2, "{p:?} {q:?}");
    }
}

#[test]
fn gradient_is_one_undo_step() {
    let mut doc = Document::new(32, 4);
    assert!(doc.paint_gradient(
        [0.0, 2.0],
        [31.0, 2.0],
        [1.0, 1.0, 1.0, 1.0],
        [0.0, 0.0, 0.0, 1.0]
    ));
    assert!(doc.undo());
    // Everything back to transparent.
    let ti = TileIdx::of_pixel(16, 2);
    let a = doc.layers[0]
        .tile(ti)
        .map(|t| t.pixel((16 - ti.origin().0) as usize, 2)[3])
        .unwrap_or(0);
    assert_eq!(a, 0, "undo restores the layer");
}

/// LM-009: a LINKED mask (the default) rides `translate_content`,
/// sub-tile accurate; an UNLINKED mask stays put — the art slides
/// underneath a fixed window.
#[test]
fn mask_link_rides_translate_and_unlink_stays() {
    let mut doc = Document::new(256, 256);
    // A mask with one full-coverage tile at (1,1) and a hole at local
    // (0,0) so the shift is observable per-pixel.
    let mut m = crate::doc::LayerMask {
        enabled: true,
        revision: crate::tile::next_revision(),
        tiles: std::collections::HashMap::new(),
        full: false,
    };
    let mut t = Tile::new_transparent();
    for y in 0..crate::tile::TILE_SIZE {
        for x in 0..crate::tile::TILE_SIZE {
            if !(x < 8 && y < 8) {
                t.set_pixel(x, y, [32768, 32768, 32768, 32768]);
            }
        }
    }
    m.tiles.insert(TileIdx::new(1, 1), std::sync::Arc::new(t));
    doc.layers[0].mask = Some(m);

    let cov = |d: &Document, x: i32, y: i32| -> u16 {
        let ti = TileIdx::of_pixel(x, y);
        d.layers[0]
            .mask
            .as_ref()
            .and_then(|m| m.tiles.get(&ti))
            .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize)[3])
            .unwrap_or(0)
    };
    assert_eq!(cov(&doc, 64, 64), 0, "the hole is at the tile corner");
    assert_eq!(cov(&doc, 80, 80), 32768);

    // Linked (default): +70,+30 sub-tile — the hole moves with it.
    doc.layers[0].translate_content(70, 30);
    assert_eq!(cov(&doc, 134, 94), 0, "the hole rode the shift");
    assert_eq!(cov(&doc, 150, 110), 32768, "coverage rode the shift");
    assert_eq!(cov(&doc, 64, 64), 0, "the vacated corner is empty");

    // Unlinked: an equal back-shift moves nothing about the mask.
    doc.layers[0].mask_linked = false;
    doc.layers[0].translate_content(-70, -30);
    assert_eq!(cov(&doc, 134, 94), 0, "unlinked: the mask stayed");
    assert_eq!(
        cov(&doc, 64, 64),
        0,
        "unlinked: untouched (still the shifted state)"
    );
}

/// The unlinked flag round-trips through ORA (absent = linked, so old
/// files load linked — CSP's default).
#[test]
fn mask_link_flag_round_trips() {
    let mut doc = Document::new(128, 128);
    let mut m = crate::doc::LayerMask::default();
    let mut t = Tile::new_transparent();
    t.set_pixel(0, 0, [32768, 32768, 32768, 32768]);
    m.tiles.insert(TileIdx::new(0, 0), std::sync::Arc::new(t));
    doc.layers[0].mask = Some(m);
    doc.layers[0].mask_linked = false;
    let mut buf = std::io::Cursor::new(Vec::new());
    crate::ora::save_to(&doc, &mut buf).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(buf.get_ref().clone())).unwrap();
    let mut s = String::new();
    std::io::Read::read_to_string(&mut z.by_name("stack.xml").unwrap(), &mut s).unwrap();
    assert!(s.contains("mnc-mask-unlinked=\"1\""), "{s}");
    let reloaded = crate::ora::load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
    assert!(!reloaded.layers[0].mask_linked, "unlinked survived");
    // A plain save/load without the attr stays linked.
    let mut b2 = std::io::Cursor::new(Vec::new());
    crate::ora::save_to(&Document::new(128, 128), &mut b2).unwrap();
    let r2 = crate::ora::load_from(std::io::Cursor::new(b2.into_inner())).unwrap();
    assert!(r2.layers[0].mask_linked, "absent attr = linked");
}

#[test]
fn translate_content_shifts_pixels_sub_tile_accurately() {
    let mut doc = Document::new(256, 256);
    let alpha = |d: &Document, x: i32, y: i32| -> u16 {
        let ti = TileIdx::of_pixel(x, y);
        d.layers[0]
            .tile(ti)
            .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize)[3])
            .unwrap_or(0)
    };
    for y in 10..20 {
        for x in 10..20 {
            let ti = TileIdx::of_pixel(x, y);
            let (ox, oy) = ti.origin();
            doc.layers[0].tile_mut(ti).set_pixel(
                (x - ox) as usize,
                (y - oy) as usize,
                [1, 2, 3, FIX15_ONE as u16],
            );
        }
    }
    doc.layers[0].translate_content(37, -5);
    assert_eq!(alpha(&doc, 47, 5), FIX15_ONE as u16, "landed at +37,-5");
    assert_eq!(alpha(&doc, 56, 14), FIX15_ONE as u16, "far corner too");
    assert_eq!(alpha(&doc, 10, 10), 0, "source vacated");
    assert_eq!(alpha(&doc, 46, 5), 0, "no smear left of the box");
}

#[test]
fn translate_content_handles_negative_offsets() {
    let mut doc = Document::new(256, 256);
    let ti = TileIdx::of_pixel(3, 3);
    let (ox, oy) = ti.origin();
    doc.layers[0].tile_mut(ti).set_pixel(
        (3 - ox) as usize,
        (3 - oy) as usize,
        [0, 0, 0, FIX15_ONE as u16],
    );
    doc.layers[0].translate_content(-8, 0);
    let t2 = TileIdx::of_pixel(-5, 3);
    let got = doc.layers[0]
        .tile(t2)
        .map(|t| t.pixel((-5 - t2.origin().0) as usize, (3 - t2.origin().1) as usize));
    assert_eq!(
        got.unwrap()[3],
        FIX15_ONE as u16,
        "off-canvas tile holds it"
    );
}

#[test]
fn default_document_is_2048_square_with_one_layer() {
    let d = Document::default();
    assert_eq!(d.size, (2048, 2048));
    assert_eq!(d.layers.len(), 1);
    assert_eq!(d.active, 0);
    assert_eq!(d.tile_extent(), (32, 32));
    assert!(d.active_layer().is_empty());
}

#[test]
fn tile_mut_creates_and_bumps_revision() {
    let mut d = Document::default();
    let idx = TileIdx::new(1, 2);
    assert!(d.active_layer().tile(idx).is_none());

    let r0 = {
        let t = d.active_layer_mut().tile_mut(idx);
        t.set_pixel(3, 4, [0, 0, 0, FIX15_ONE as u16]);
        t.revision()
    };
    assert_eq!(d.active_layer().tile_count(), 1);
    assert_eq!(d.active_layer().tile(idx).unwrap().pixel(3, 4)[3], 32768);

    let r1 = d.active_layer_mut().tile_mut(idx).revision();
    assert!(r1 > r0, "second write must publish a newer revision");
}

#[test]
fn write_is_copy_on_write_against_snapshots() {
    let mut layer = Layer::new("L");
    let idx = TileIdx::new(0, 0);
    layer.tile_mut(idx).set_pixel(0, 0, [0, 0, 0, 32768]);

    // Undo would keep exactly this handle.
    let snapshot = layer.tile_arc(idx).unwrap().clone();
    assert_eq!(Arc::strong_count(&snapshot), 2);

    layer.tile_mut(idx).set_pixel(0, 0, [0, 0, 0, 0]);

    assert_eq!(
        snapshot.pixel(0, 0)[3],
        32768,
        "snapshot must not be mutated"
    );
    assert_eq!(layer.tile(idx).unwrap().pixel(0, 0)[3], 0);
    assert_eq!(
        Arc::strong_count(&snapshot),
        1,
        "make_mut must have unshared"
    );
}

/// Paint one pixel through the normal write path (what a brush does).
fn paint(doc: &mut Document, idx: TileIdx, v: u16) {
    doc.active_layer_mut()
        .tile_mut(idx)
        .set_pixel(0, 0, [v, v, v, v]);
}

fn px(doc: &Document, layer: usize, idx: TileIdx) -> Option<[u16; 4]> {
    doc.layers[layer].tile(idx).map(|t| t.pixel(0, 0))
}

#[test]
fn undo_redo_roundtrip_restores_pixel_state() {
    let mut doc = Document::default();
    let a = TileIdx::new(1, 1);
    let b = TileIdx::new(2, 1);

    // Stroke 1 creates a tile that did not exist (pre-image = None).
    doc.begin_op();
    paint(&mut doc, a, 100);
    assert!(doc.end_op());
    assert_eq!(px(&doc, 0, a).unwrap()[3], 100);

    // Stroke 2 overwrites `a` and creates `b`.
    doc.begin_op();
    paint(&mut doc, a, 200);
    paint(&mut doc, b, 200);
    assert!(doc.end_op());
    assert_eq!(doc.undo_len(), 2);

    assert!(doc.undo());
    assert_eq!(px(&doc, 0, a).unwrap()[3], 100, "tile restored to stroke 1");
    assert!(
        px(&doc, 0, b).is_none(),
        "tile that did not exist is removed"
    );

    assert!(doc.undo());
    assert!(px(&doc, 0, a).is_none(), "back to a blank layer");
    assert!(!doc.can_undo());
    assert!(!doc.undo());

    assert!(doc.redo());
    assert_eq!(px(&doc, 0, a).unwrap()[3], 100);
    assert!(doc.redo());
    assert_eq!(px(&doc, 0, a).unwrap()[3], 200);
    assert_eq!(px(&doc, 0, b).unwrap()[3], 200);
    assert!(!doc.redo());
}

#[test]
fn restored_tiles_carry_fresh_revisions() {
    let mut doc = Document::default();
    let a = TileIdx::new(0, 0);
    doc.begin_op();
    paint(&mut doc, a, 100);
    doc.end_op();
    let before = doc.max_revision();

    doc.begin_op();
    paint(&mut doc, a, 200);
    doc.end_op();
    doc.undo();

    assert_eq!(px(&doc, 0, a).unwrap()[3], 100);
    assert!(
        doc.max_revision() > before,
        "an undone tile must look new to the GPU cache"
    );
}

#[test]
fn new_op_clears_the_redo_stack() {
    let mut doc = Document::default();
    let a = TileIdx::new(0, 0);
    doc.begin_op();
    paint(&mut doc, a, 10);
    doc.end_op();
    doc.undo();
    assert!(doc.can_redo());

    doc.begin_op();
    paint(&mut doc, a, 20);
    doc.end_op();
    assert!(!doc.can_redo(), "a new op forks the history");
}

#[test]
fn empty_op_pushes_nothing_and_undo_stack_is_capped() {
    let mut doc = Document::default();
    doc.begin_op();
    assert!(!doc.end_op());
    assert!(!doc.can_undo());

    for i in 0..(crate::undo::UNDO_LIMIT + 25) {
        doc.begin_op();
        paint(&mut doc, TileIdx::new(0, 0), (i % 100) as u16 + 1);
        doc.end_op();
    }
    assert_eq!(doc.undo_len(), crate::undo::UNDO_LIMIT);
}

/// The depth is a preference now (`prefs.txt` `undo_depth=`), not a
/// constant: the cap follows whatever the document was told, and
/// LOWERING it frees the memory at once rather than on the next stroke.
#[test]
fn undo_depth_is_settable_and_lowering_it_trims_now() {
    let mut doc = Document::default();
    assert_eq!(doc.undo_limit(), crate::undo::UNDO_LIMIT, "default = today");

    doc.set_undo_limit(5);
    for i in 0..12 {
        doc.begin_op();
        paint(&mut doc, TileIdx::new(0, 0), i + 1);
        doc.end_op();
    }
    assert_eq!(doc.undo_len(), 5);

    // Raising it keeps what is there and simply allows more.
    doc.set_undo_limit(50);
    assert_eq!(doc.undo_len(), 5);
    for i in 0..20 {
        doc.begin_op();
        paint(&mut doc, TileIdx::new(0, 0), i + 100);
        doc.end_op();
    }
    assert_eq!(doc.undo_len(), 25);

    // Lowering trims immediately, oldest first, labels in lockstep.
    doc.set_undo_limit(3);
    assert_eq!(doc.undo_len(), 3);
    assert_eq!(doc.undo_labels().len(), 3);

    // A cleared history keeps the depth it was given.
    doc.clear_history();
    assert_eq!(doc.undo_limit(), 3);
}

#[test]
fn writes_outside_an_op_are_not_undoable() {
    let mut doc = Document::default();
    paint(&mut doc, TileIdx::new(0, 0), 42);
    assert!(!doc.can_undo());
}

#[test]
fn layer_ops() {
    let mut doc = Document::default();
    assert!(!doc.remove_layer(0), "the last layer cannot be removed");

    let i = doc.add_layer("Ink");
    assert_eq!((i, doc.active, doc.layers.len()), (1, 1, 2));
    assert_eq!(doc.layers[1].name, "Ink");

    paint(&mut doc, TileIdx::new(0, 0), 500);
    let dup = doc.duplicate_layer(1).unwrap();
    assert_eq!(dup, 2);
    assert_eq!(doc.layers[2].name, "Ink copy");
    assert_eq!(px(&doc, 2, TileIdx::new(0, 0)).unwrap()[3], 500);

    // Painting the copy must not touch the original (Arc COW).
    paint(&mut doc, TileIdx::new(0, 0), 900);
    assert_eq!(px(&doc, 1, TileIdx::new(0, 0)).unwrap()[3], 500);

    assert!(doc.move_layer(2, 0));
    assert_eq!(doc.layers[0].name, "Ink copy");
    assert_eq!(doc.active, 0, "the moved layer stays active");
    assert_eq!(doc.layers[1].name, "Layer 1");

    assert!(doc.rename_layer(1, "Paper"));
    assert_eq!(doc.layers[1].name, "Paper");
    assert!(!doc.rename_layer(9, "nope"));

    assert!(doc.set_active(2));
    assert!(!doc.set_active(3));

    let r = doc.revision;
    assert!(doc.set_layer_opacity(0, 2.0));
    assert_eq!(doc.layers[0].opacity, 1.0, "opacity is clamped");
    assert!(doc.set_layer_opacity(0, 0.25));
    assert!(doc.set_layer_blend(0, Blend::Multiply));
    assert!(doc.set_layer_visible(0, false));
    assert!(doc.revision > r, "presentation changes bump the revision");

    assert!(doc.remove_layer(0));
    assert_eq!(doc.layers.len(), 2);
    assert!(doc.active < doc.layers.len());
}

#[test]
fn text_edits_are_undoable_and_warm_fills_caches() {
    use crate::text::{RenderedText, TextItem, TextSet};
    let sprite = |v: u8| {
        Arc::new(RenderedText {
            origin: [10, 10],
            size: [8, 8],
            rgba: (0..8 * 8).flat_map(|_| [v, v, v, 255]).collect(),
        })
    };
    let mut item = TextItem::new([10.0, 10.0], "Meiryo".into(), 12.0, [0, 0, 0], false);
    item.insert(0, "a");
    item.cache = Some(sprite(0));
    let one = TextSet { texts: vec![item] };

    let mut doc = Document::default();
    let li = doc.add_text_layer("Text 1", one);
    assert!(doc.layers[li].is_text() && doc.layers[li].is_vector());
    assert!(doc.layers[li].tile_count() > 0, "sprite rasterized");
    // The commit door minted the item's id; clones of the DOC's set are
    // how the app really edits, and they keep that identity.
    let one = doc.layers[li].texts().unwrap().clone();
    assert_ne!(one.texts[0].id, 0, "commit mints");

    let mut moved = one.clone();
    moved.texts[0].cache = Some(sprite(0));
    moved.texts[0].translate(30.0, 0.0);
    assert!(doc.set_texts(li, moved.clone()));
    assert_eq!(doc.layers[li].texts().unwrap(), &moved);
    assert!(doc.can_undo());
    doc.undo();
    assert_eq!(doc.layers[li].texts().unwrap(), &one, "vectors restored");
    doc.redo();
    assert_eq!(doc.layers[li].texts().unwrap(), &moved, "redo re-moves");
    assert!(
        !doc.set_texts(0, one.clone()),
        "raster layers have no texts"
    );

    // Warm: strip the cache (as an ORA load would) and refill it.
    let mut bare = moved.clone();
    bare.texts[0].cache = None;
    let LayerKind::Speech(sp) = &mut doc.layers[li].kind else {
        unreachable!()
    };
    sp.texts = bare;
    assert!(doc.warm_text_caches(li, |_| Some(sprite(7))));
    assert!(doc.layers[li].texts().unwrap().texts[0].cache.is_some());
    // Already-cached items are left alone.
    assert!(doc.warm_text_caches(li, |_| panic!("must not re-shape")));
}

/// Stable ids (the automation round): minted unique, copies are new
/// identities, and undo restores a deleted layer WITH its id — the
/// property an id-holding automation client depends on.
#[test]
fn stable_layer_ids_mint_survive_reorder_and_undo() {
    let mut doc = Document::default();
    doc.add_layer("2");
    doc.add_layer("3");
    let ids: Vec<u64> = doc.layers.iter().map(|l| l.id()).collect();
    assert!(ids.iter().all(|&i| i != 0), "every layer has a real id");
    let mut uniq = ids.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), ids.len(), "ids are unique");

    // Reorder: identity follows the layer, not the slot.
    let moved = ids[2];
    assert!(doc.move_layer(2, 0));
    assert_eq!(doc.layer_index_of(moved), Some(0));

    // Duplicate: the copy is a NEW identity.
    let src = doc.layers[0].id();
    let at = doc.duplicate_layer(0).unwrap();
    assert_ne!(doc.layers[at].id(), src);
    assert_eq!(doc.layer_index_of(src), Some(0), "original keeps its id");

    // Delete + undo: the SAME identity comes back.
    let gone = doc.layers[at].id();
    assert!(doc.remove_layer(at));
    assert_eq!(doc.layer_index_of(gone), None);
    assert!(doc.undo());
    assert_eq!(doc.layer_index_of(gone), Some(at), "undo restores the id");

    // Convert-keeping-original: the kept copy is a new identity too.
    let before = doc.layers[0].id();
    assert!(doc.convert_layer(0, true, None, None, true, None));
    assert_eq!(doc.layers[0].id(), before);
    assert_ne!(doc.layers[1].id(), before);
}

/// `ensure_ids` (the ORA loader's heal): zeros and duplicates remint —
/// first occurrence keeps the id — and the mint is lifted first, so a
/// healed id can never equal one the file also holds.
#[test]
fn ensure_ids_heals_zeros_and_duplicates() {
    use crate::text::{TextItem, TextSet};
    let mut doc = Document::default();
    doc.add_layer("2");
    doc.add_layer("3");
    let item = || TextItem::new([0.0, 0.0], "Meiryo".into(), 12.0, [0, 0, 0], false);
    doc.add_text_layer("T", TextSet { texts: vec![item(), item(), item()] });
    // Fake a hand-edited / pre-id file's stack, BEHIND the commit doors
    // (they would heal on the way in). Uniqueness is PER COLLECTION —
    // lookups are typed — so a layer and an item may share a number;
    // only siblings must differ.
    doc.layers[0].set_id(7);
    doc.layers[1].set_id(7);
    doc.layers[2].set_id(0);
    if let LayerKind::Speech(sp) = &mut doc.layers[3].kind {
        sp.texts.texts[0].id = 9;
        sp.texts.texts[1].id = 9;
        sp.texts.texts[2].id = 0;
    }
    doc.ensure_ids();
    let mut lids: Vec<u64> = doc.layers.iter().map(|l| l.id()).collect();
    assert_eq!(doc.layers[0].id(), 7, "first occurrence keeps the id");
    assert!(lids.iter().all(|&i| i != 0));
    let n = lids.len();
    lids.sort();
    lids.dedup();
    assert_eq!(lids.len(), n, "layers healed unique");
    let LayerKind::Speech(sp) = &doc.layers[3].kind else {
        unreachable!()
    };
    let iids: Vec<u64> = sp.texts.texts.iter().map(|t| t.id).collect();
    assert_eq!(iids[0], 9, "first occurrence keeps the id");
    assert!(
        iids[1] > 9 && iids[2] > 9,
        "duplicate and zero healed from past the file's max (9): {iids:?}"
    );
}

/// A frame folder with one child, plus `n` loose layers stacked above
/// it. Returns `(escapee, header, the loose indices bottom-first)`.
fn breakout_stack(n: usize) -> (Document, usize, usize, Vec<usize>) {
    let mut doc = Document::new(64, 64);
    let hdr = doc.add_frame_folder(
        "panel",
        crate::frame::FrameSet::single_rect([8.0, 8.0, 56.0, 56.0], 2.0),
    );
    let burst = hdr - 1;
    assert!(doc.set_layer_escape(burst, true));
    let above: Vec<usize> = (0..n)
        .map(|k| doc.add_layer_above(doc.layers.len() - 1, format!("up{k}")))
        .collect();
    (doc, burst, hdr, above)
}

/// Item 3, the invariant: paint order is a stack, so "draws over layer
/// N" HAS to mean "over everything below N". The set is stored, and the
/// resolved seat is the topmost member — never a hole in the middle.
#[test]
fn the_draws_over_set_only_ever_fills_downward() {
    let (mut doc, burst, hdr, up) = breakout_stack(3);
    assert_eq!(doc.spill_anchor(burst), Some(hdr), "default seat");
    assert_eq!(doc.spill_candidates(burst), up, "only the stack above");

    // Marking the TOP one covers the two below it as well.
    assert!(doc.set_layer_spill_seat(burst, Some(up[2])));
    let set = doc.layers[burst].draws_over.clone();
    for &j in &up {
        assert!(
            set.contains(&doc.layers[j].id()),
            "over {j} is implied by over {}",
            up[2]
        );
    }
    assert_eq!(doc.spill_anchor(burst), Some(up[2]));
    assert_eq!(doc.spill_seat(burst), Some(up[2]), "the marker's position");

    // Moving it down drops the ones above — the set is exactly the run.
    assert!(doc.set_layer_spill_seat(burst, Some(up[0])));
    assert_eq!(
        doc.layers[burst].draws_over,
        BTreeSet::from([doc.layers[up[0]].id()])
    );
    assert_eq!(doc.spill_anchor(burst), Some(up[0]));

    // …and back to the default seat.
    assert!(doc.set_layer_spill_seat(burst, None));
    assert!(doc.layers[burst].draws_over.is_empty());
    assert_eq!(doc.spill_anchor(burst), Some(hdr));

    // A layer that is not bursting out has no seat to move.
    assert!(!doc.set_layer_spill_seat(up[0], Some(up[2])));
}

/// The set is keyed on STABLE IDS, so a reorder or a delete leaves the
/// marker on the same art — the whole reason part 2 waited for ids.
#[test]
fn the_draws_over_set_survives_reorder_and_delete() {
    let (mut doc, burst, hdr, up) = breakout_stack(3);
    assert!(doc.set_layer_spill_seat(burst, Some(up[1])));
    let kept = doc.layers[up[1]].id();

    // Delete the layer between the marker and the panel: the marker
    // stays on the same art, one row lower.
    // (`burst` sits BELOW everything removed here, so its own index
    // never moves — the ids are what has to do the work.)
    assert!(doc.remove_layer(up[0]));
    let now = doc.layer_index_of(kept).expect("still there");
    assert_eq!(doc.spill_anchor(burst), Some(now), "same art, new index");
    assert!(now < up[1], "and it really did move");

    // Delete the marker itself: the seat falls back, never dangles.
    assert!(doc.remove_layer(now));
    assert!(doc.layer_index_of(kept).is_none());
    let seat = doc.spill_anchor(burst).expect("still a breakout");
    assert!(seat < doc.layers.len(), "the seat is a live index");
    assert_ne!(seat, hdr + 99, "sanity");
}

/// A covered layer living inside somebody ELSE's sealed folder lifts to
/// that folder's header: inside the group the burst would be clipped by
/// the very panel it is spilling over.
#[test]
fn a_covered_layer_inside_a_sealed_folder_lifts_to_its_header() {
    let mut doc = Document::new(64, 64);
    let hdr = doc.add_frame_folder(
        "lower",
        crate::frame::FrameSet::single_rect([8.0, 32.0, 56.0, 56.0], 2.0),
    );
    let burst = hdr - 1;
    assert!(doc.set_layer_escape(burst, true));
    let up = doc.add_frame_folder(
        "upper",
        crate::frame::FrameSet::single_rect([8.0, 8.0, 56.0, 24.0], 2.0),
    );
    let inside = up - 1;
    assert!(doc.set_layer_spill_seat(burst, Some(inside)));
    assert_eq!(
        doc.spill_anchor(burst),
        Some(up),
        "lifted out of the upper panel's seal to its header"
    );
}

/// One toggle, one undo press — the set lives on `Layer`, so the
/// Structure snapshot carries it.
#[test]
fn a_draws_over_toggle_is_one_undo_press() {
    let (mut doc, burst, hdr, up) = breakout_stack(2);
    assert!(doc.set_layer_spill_seat(burst, Some(up[0])));
    assert!(doc.set_layer_spill_seat(burst, Some(up[1])));
    assert_eq!(doc.spill_anchor(burst), Some(up[1]));

    doc.undo();
    assert_eq!(doc.spill_anchor(burst), Some(up[0]), "one press, one step");
    doc.undo();
    assert!(doc.layers[burst].draws_over.is_empty());
    assert_eq!(doc.spill_anchor(burst), Some(hdr), "back to the default");
    doc.redo();
    assert_eq!(doc.spill_anchor(burst), Some(up[0]), "redo restores it");

    // Setting the value it already has is not a step.
    let before = doc.spill_anchor(burst);
    assert!(!doc.set_layer_spill_seat(burst, Some(up[0])));
    assert_eq!(doc.spill_anchor(burst), before);
}

/// The shared walk: a mask-capped breakout appears TWICE (held-in at
/// its own seat, spilled at the anchor's), an uncapped one once, and
/// every other layer exactly once.
#[test]
fn composite_order_splits_only_a_mask_capped_breakout() {
    let (mut doc, burst, hdr, up) = breakout_stack(1);
    let steps = doc.composite_order();
    assert_eq!(steps.len(), doc.layers.len(), "uncapped: one step each");
    let seat = steps.iter().position(|s| s.layer == burst).unwrap();
    let hdr_at = steps.iter().position(|s| s.layer == hdr).unwrap();
    assert_eq!(seat, hdr_at + 1, "right after its frame folder header");
    assert_eq!(steps[seat].depth, doc.layers[hdr].depth);
    assert!(steps.iter().all(|s| s.part == SpillPart::All));

    // Give it a mask: now it is two steps, and only two.
    doc.layers[burst].tile_mut(TileIdx::new(0, 0));
    assert!(doc.mask_selection_blank(burst));
    let steps = doc.composite_order();
    assert_eq!(steps.len(), doc.layers.len() + 1);
    let parts: Vec<SpillPart> = steps
        .iter()
        .filter(|s| s.layer == burst)
        .map(|s| s.part)
        .collect();
    assert_eq!(parts, vec![SpillPart::In, SpillPart::Out]);
    let held = steps.iter().position(|s| s.part == SpillPart::In).unwrap();
    let out = steps.iter().position(|s| s.part == SpillPart::Out).unwrap();
    assert!(held < out, "the held half walks in place, first");
    assert_eq!(steps[held].depth, doc.layers[burst].depth, "its own depth");

    // And the seat still moves with the set.
    assert!(doc.set_layer_spill_seat(burst, Some(up[0])));
    let steps = doc.composite_order();
    let out = steps.iter().position(|s| s.part == SpillPart::Out).unwrap();
    let anchor = steps.iter().position(|s| s.layer == up[0]).unwrap();
    assert_eq!(out, anchor + 1, "right after the layer it draws over");
}

/// Item ids: the commit doors mint fresh (id 0) and cloned (duplicate
/// id) items, existing items keep their identity across edits, and
/// `index_of_id` tracks an item through a reorder of its set.
#[test]
fn text_and_balloon_item_ids_mint_and_stay_stable() {
    use crate::balloon::{Balloon, BalloonSet};
    use crate::text::{TextItem, TextSet};
    let mut doc = Document::default();
    let item = TextItem::new([0.0, 0.0], "Meiryo".into(), 12.0, [0, 0, 0], false);
    let li = doc.add_text_layer("T", TextSet { texts: vec![item] });
    let ts = doc.layers[li].texts().unwrap().clone();
    let id0 = ts.texts[0].id;
    assert_ne!(id0, 0);

    // A cloned item (duplicate id) and a fresh one (id 0) both mint;
    // the original keeps its id.
    let mut edited = ts.clone();
    let mut dup = edited.texts[0].clone();
    dup.pos = [50.0, 50.0];
    edited.texts.push(dup);
    edited
        .texts
        .push(TextItem::new([9.0, 9.0], "Meiryo".into(), 12.0, [0, 0, 0], false));
    assert!(doc.set_texts(li, edited));
    let got = doc.layers[li].texts().unwrap();
    assert_eq!(got.texts[0].id, id0, "original identity kept");
    let all: Vec<u64> = got.texts.iter().map(|t| t.id).collect();
    assert!(all.iter().all(|&i| i != 0));
    let mut uniq = all.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), all.len(), "minted unique");
    assert_eq!(got.index_of_id(id0), Some(0));

    // Balloons: same contract through their door.
    let bli = doc.add_balloon_layer(
        "B",
        BalloonSet {
            balloons: vec![Balloon::default(), Balloon::default()],
            border_px: 4.0,
            pressure_width: false,
        },
    );
    let bs = doc.layers[bli].balloons().unwrap().clone();
    assert!(bs.balloons.iter().all(|b| b.id != 0));
    assert_ne!(bs.balloons[0].id, bs.balloons[1].id);
    // Reorder inside the set: the id still finds the item.
    let follow = bs.balloons[0].id;
    let mut swapped = bs.clone();
    swapped.balloons.swap(0, 1);
    assert!(doc.set_balloons(bli, swapped));
    assert_eq!(
        doc.layers[bli].balloons().unwrap().index_of_id(follow),
        Some(1)
    );
}

#[test]
fn structural_layer_ops_record_and_the_history_survives() {
    // The 2026-08-21 model: a structural op pushes a Structure snapshot
    // instead of clearing. Undo is LIFO, so the paint group recorded
    // BEFORE the add is still valid once the add's swap has restored
    // the one-layer stack it was recorded against.
    let mut doc = Document::default();
    doc.begin_op();
    paint(&mut doc, TileIdx::new(0, 0), 10);
    doc.end_op();
    assert!(doc.can_undo());
    doc.add_layer("2");
    assert_eq!(doc.layers.len(), 2);
    assert_eq!(doc.undo_len(), 2, "the paint step survived the add");
    assert!(doc.next_undo_is_structure(), "the add is on top");
    assert!(doc.undo(), "undo the add");
    assert_eq!(doc.layers.len(), 1, "the new layer is gone");
    assert!(doc.undo(), "…then undo the paint on the restored stack");
    assert!(doc.layers[0].tiles().next().is_none(), "paint took back");
    assert!(doc.redo() && doc.redo(), "the whole chain replays forward");
    assert_eq!(doc.layers.len(), 2);
}

/// The full LIFO round trip across several structural shapes: every op
/// undoes in reverse order and redoes forward, and pixel groups
/// recorded between structural steps restore into the right layers.
#[test]
fn structural_undo_round_trips_a_mixed_chain() {
    let mut doc = Document::default();
    doc.begin_op();
    paint(&mut doc, TileIdx::new(0, 0), 10);
    doc.end_op();
    doc.add_layer("2");
    doc.begin_op();
    paint(&mut doc, TileIdx::new(1, 0), 20);
    doc.end_op();
    let dup = doc.duplicate_layer(doc.active).unwrap();
    assert_eq!(doc.layers.len(), 3);
    assert!(doc.remove_layer(dup));
    assert_eq!(doc.layers.len(), 2);
    assert_eq!(doc.undo_len(), 5);
    for _ in 0..5 {
        assert!(doc.undo());
    }
    assert_eq!(doc.layers.len(), 1, "back to the single empty layer");
    assert!(
        doc.layers[0].tiles().next().is_none(),
        "first paint undone last"
    );
    assert!(!doc.can_undo());
    for _ in 0..5 {
        assert!(doc.redo());
    }
    assert_eq!(doc.layers.len(), 2, "dup redone, then its removal");
    assert!(
        doc.layers[1].tiles().next().is_some(),
        "second paint replayed into the re-added layer"
    );
}

/// Merge-down is destructive on the lower layer's pixels; the Structure
/// snapshot holds the pre-merge tiles by Arc, so undo restores both
/// layers exactly.
#[test]
fn merge_down_undoes_to_both_layers() {
    let mut doc = Document::default();
    doc.begin_op();
    paint(&mut doc, TileIdx::new(0, 0), 10);
    doc.end_op();
    doc.add_layer("upper");
    doc.begin_op();
    paint(&mut doc, TileIdx::new(0, 0), 200);
    doc.end_op();
    assert!(doc.merge_down(1));
    assert_eq!(doc.layers.len(), 1);
    assert!(doc.undo(), "undo the merge");
    assert_eq!(doc.layers.len(), 2, "upper layer is back");
    assert_eq!(
        px(&doc, 0, TileIdx::new(0, 0)).unwrap()[0],
        10,
        "lower layer's pre-merge pixels restored"
    );
}

/// The multi-selection is index-keyed: a structural op still clears it
/// even though the history now survives.
#[test]
fn structural_ops_clear_the_multi_selection_not_the_history() {
    let mut doc = Document::default();
    doc.add_layer("2");
    doc.toggle_multi(0);
    assert_eq!(doc.multi_targets().len(), 2);
    doc.add_layer("3");
    assert_eq!(
        doc.multi_targets().len(),
        1,
        "selection cleared back to the active row alone"
    );
    assert!(doc.can_undo(), "history kept");
}

#[test]
fn tile_bounds_are_tile_aligned() {
    let mut layer = Layer::new("L");
    assert!(layer.tile_bounds().is_none());
    layer.tile_mut(TileIdx::new(1, 2));
    layer.tile_mut(TileIdx::new(3, 2));
    assert_eq!(layer.tile_bounds(), Some((64, 128, 192, 64)));
}

#[test]
fn frame_edits_are_undoable_and_rerasterize() {
    use crate::frame::FrameSet;
    let mut doc = Document::new(256, 256);
    let one_frame = FrameSet::single_rect([64.0, 64.0, 192.0, 192.0], 4.0);
    let li = doc.add_frame_layer("Frame 1", one_frame.clone());
    assert!(doc.layers[li].is_frame());
    assert!(
        doc.can_undo(),
        "adding the layer records one structural step"
    );
    let raster_before = doc.layers[li].tile_count();
    assert!(raster_before > 0);

    // Divide the panel — one undoable step.
    let mut divided = one_frame.clone();
    let (a, b) = divided.frames[0]
        .split([128.0, 0.0], [128.0, 256.0], 8.0)
        .unwrap();
    divided.frames = vec![a, b];
    assert!(doc.set_frames(li, divided.clone()));
    assert_eq!(doc.layers[li].frames().unwrap().frames.len(), 2);

    assert!(doc.undo());
    assert_eq!(
        doc.layers[li].frames().unwrap(),
        &one_frame,
        "vectors restored"
    );
    assert!(doc.redo());
    assert_eq!(
        doc.layers[li].frames().unwrap(),
        &divided,
        "redo re-divides"
    );

    // Guards: no painting semantics change, but merge refuses frames and
    // set_frames refuses raster layers.
    assert!(!doc.merge_down(li), "frame layers never merge");
    assert!(
        !doc.set_frames(0, one_frame),
        "raster layers have no frames"
    );
}

/// Row 32: rasterizing a frame folder keeps what is losable-for-free
/// and gives up only the frame OBJECT — the header's border ink
/// becomes a plain raster layer, the panel clip becomes the
/// children's layer masks, the children stay separate, and ONE undo
/// restores the folder.
#[test]
fn rasterize_frame_folder_keeps_children_and_clips_by_mask() {
    let mut doc = Document::new(256, 256);
    let fs = FrameSet {
        frames: vec![crate::Frame::rect(32.0, 32.0, 224.0, 224.0)],
        border_px: 4.0,
        slot: None,
        reading_pin: None,
        border_ruler: false,
        color: [0, 0, 0],
    };
    let hi = doc.add_frame_folder("Frame 1", fs);
    // add_frame_folder lands [White, Layer 1, header] with the draw
    // layer active; children sit BELOW the header index.
    let kids: Vec<usize> = doc.children_range(hi).collect();
    assert_eq!(kids.len(), 2, "white + draw layer");
    assert!(doc.layers[hi].folder && doc.layers[hi].is_frame());
    assert!(doc.layers[hi].mask_tiles().is_some_and(|m| !m.is_empty()));

    assert!(doc.rasterize_frame_folder(hi));
    let h = &doc.layers[hi];
    assert!(!h.folder, "the folder is gone");
    assert!(matches!(h.kind, LayerKind::Raster), "the header is plain ink now");
    assert!(h.tile_count() > 0, "the border ink raster survives");
    for k in &kids {
        let c = &doc.layers[*k];
        assert_eq!(c.depth, h.depth, "child hoisted loose beside the ink");
        assert!(
            c.mask.as_ref().is_some_and(|m| !m.tiles.is_empty()),
            "the panel clip rides as a layer mask"
        );
    }
    // add_frame_folder pushed its own structural step — the rasterize
    // is exactly ONE more.
    assert_eq!(doc.undo_labels().len(), 2, "setup + ONE rasterize step");
    assert!(doc.undo());
    let h = &doc.layers[hi];
    assert!(h.folder && h.is_frame(), "undo restores the frame folder");
    for k in &kids {
        assert!(
            doc.layers[*k].mask.is_none(),
            "undo takes the clip back into the folder"
        );
    }

    // Only frame folders: a plain layer is refused untouched.
    let plain = doc.add_layer("plain");
    assert!(!doc.rasterize_frame_folder(plain));
}

#[test]
fn balloon_edits_are_undoable_and_rerasterize() {
    use crate::balloon::{Balloon, BalloonSet, BalloonShape};
    let mut doc = Document::new(256, 256);
    let one = BalloonSet {
        balloons: vec![Balloon {
            shape: BalloonShape::Ellipse {
                center: [128.0, 128.0],
                radii: [60.0, 40.0],
            },
            tails: Vec::new(),

            ..Default::default()
        }],
        border_px: 4.0,
        pressure_width: false,
    };
    let li = doc.add_balloon_layer("Balloon 1", one);
    assert!(doc.layers[li].is_balloon() && doc.layers[li].is_vector());
    assert!(doc.layers[li].tile_count() > 0);
    // Post-mint state — clones of it keep identity (text test's twin).
    let one = doc.layers[li].balloons().unwrap().clone();
    assert_ne!(one.balloons[0].id, 0, "commit mints");

    let mut moved = one.clone();
    moved.balloons[0].translate(30.0, 0.0);
    assert!(doc.set_balloons(li, moved.clone()));
    assert_eq!(doc.layers[li].balloons().unwrap(), &moved);

    assert!(doc.undo());
    assert_eq!(doc.layers[li].balloons().unwrap(), &one, "vectors restored");
    assert!(doc.redo());
    assert_eq!(doc.layers[li].balloons().unwrap(), &moved, "redo re-moves");

    assert!(!doc.merge_down(li), "balloon layers never merge");
    assert!(!doc.set_balloons(0, one), "raster layers have no balloons");
}

#[test]
fn frame_folder_structure_and_white_fill_share_one_tile() {
    let mut doc = Document::new(256, 256);
    let fs = FrameSet::single_rect([32.0, 32.0, 224.0, 224.0], 4.0);
    let hi = doc.add_frame_folder("Frame 1", fs.clone());
    // Stack bottom→top: Layer 1(0), White(1), Layer 1(1), Frame 1 header.
    assert_eq!(doc.layers.len(), 4);
    assert_eq!(hi, 3);
    assert!(doc.layers[3].folder && doc.layers[3].is_frame());
    assert_eq!(doc.layers[3].depth, 0);
    assert_eq!(doc.layers[1].name, "White");
    assert_eq!(doc.layers[1].depth, 1);
    assert_eq!(doc.layers[2].depth, 1);
    assert_eq!(doc.active, 2, "the draw layer is active");
    assert_eq!(doc.children_range(3), 1..3);
    assert_eq!(doc.block_range(3), 1..4);

    // The white fill really is one shared allocation.
    let whites: Vec<_> = doc.layers[1].tiles().map(|(_, a)| Arc::as_ptr(a)).collect();
    assert_eq!(whites.len(), 16);
    assert!(
        whites.iter().all(|p| *p == whites[0]),
        "all tiles share one Arc"
    );
    assert_eq!(
        doc.layers[1].tile(TileIdx::new(0, 0)).unwrap().pixel(5, 5),
        [FIX15_ONE as u16; 4]
    );

    // Painting on the header or merging through the mask is refused.
    assert!(!doc.layers[3].paintable());
    assert!(doc.layers[2].paintable());
    assert!(!doc.merge_down(3), "a FRAME folder never merges (its vectors)");
    assert!(!doc.merge_down(1), "White sits over a depth boundary");
}

#[test]
fn frame_folder_without_fill_has_no_white_child() {
    let mut doc = Document::new(128, 128);
    let fs = FrameSet::single_rect([16.0, 16.0, 112.0, 112.0], 2.0);
    let hi = doc.add_frame_folder_with("Frame 1", fs, false);
    assert_eq!(doc.children_range(hi).len(), 1, "just the draw layer");
    assert!(doc.layers.iter().all(|l| l.name != "White"));
}

#[test]
fn divide_frame_folder_spawns_a_sibling_folder_with_children() {
    let mut doc = Document::new(256, 256);
    let fs = FrameSet::single_rect([32.0, 32.0, 224.0, 224.0], 4.0);
    let hi = doc.add_frame_folder("Frame 1", fs.clone());
    // Split the panel in half by hand and hand the pieces over.
    let keep = FrameSet::single_rect([32.0, 32.0, 224.0, 120.0], 4.0);
    let off = FrameSet::single_rect([32.0, 136.0, 224.0, 224.0], 4.0);
    // `above = false`: the split-off half is the LOWER panel, so it reads
    // second and belongs below the original's block. (Which half reads
    // first is the app's call — `cmd::frames::reads_earlier`.)
    let new_hi = doc
        .divide_frame_folder(hi, keep.clone(), off.clone(), false)
        .unwrap();

    // Stack bottom→top: Layer 1, [White, Layer 1, Frame 2], [White, Layer 1, Frame 1].
    assert_eq!(doc.layers.len(), 7);
    assert_eq!(new_hi, 3);
    let new = &doc.layers[new_hi];
    assert!(new.folder && new.is_frame());
    assert_eq!(new.frames().unwrap().frames, off.frames);
    assert_eq!(doc.children_range(new_hi), 1..3);
    assert_eq!(doc.layers[1].name, "White");
    assert_eq!(doc.active, 2, "new folder's draw layer active");
    // The original folder kept its piece and its own children.
    let orig = doc
        .layers
        .iter()
        .rposition(|l| l.name == "Frame 1")
        .unwrap();
    assert_eq!(doc.layers[orig].frames().unwrap().frames, keep.frames);
    assert_eq!(doc.children_range(orig), 4..6);
    // Both folders re-derived isolation masks.
    assert!(doc.layers[orig].mask_tiles().is_some());
    assert!(doc.layers[new_hi].mask_tiles().is_some());

    // A flat (non-folder) frame layer refuses.
    let mut flat = Document::new(64, 64);
    let fi = flat.add_frame_layer("F", FrameSet::single_rect([8.0, 8.0, 56.0, 56.0], 2.0));
    assert!(
        flat.divide_frame_folder(
            fi,
            FrameSet::single_rect([8.0, 8.0, 56.0, 30.0], 2.0),
            FrameSet::single_rect([8.0, 34.0, 56.0, 56.0], 2.0),
            false
        )
        .is_none()
    );
}

#[test]
fn folder_visibility_cascades_and_clip_bases_resolve() {
    let mut doc = Document::new(128, 128);
    let fs = FrameSet::single_rect([16.0, 16.0, 112.0, 112.0], 2.0);
    let hi = doc.add_frame_folder("F", fs);
    assert!(doc.effective_visibility().iter().all(|v| *v));

    doc.set_layer_visible(hi, false);
    let eff = doc.effective_visibility();
    assert!(!eff[1] && !eff[2], "children hidden with the folder");
    assert!(eff[0], "the root layer below is untouched");
    doc.set_layer_visible(hi, true);

    // Clip chain: draw layer clips to White; a second clipped layer above
    // resolves to the same base. Folders refuse the flag.
    assert!(doc.set_layer_clip(2, true));
    let li = doc.add_layer_in_folder(hi, "Tone").unwrap();
    assert!(doc.set_layer_clip(li, true));
    let bases = doc.clip_bases();
    assert_eq!(bases[2], Some(1), "draw layer clips to White");
    assert_eq!(bases[li], Some(1), "the chain shares one base");
    assert!(
        !doc.set_layer_clip(doc.layers.len() - 1, true),
        "folder refuses"
    );
    // The bottom root layer has nothing below it at its depth.
    assert!(doc.set_layer_clip(0, true));
    assert_eq!(doc.clip_bases()[0], None, "no base -> flag ignored");
}

/// Recordable actions: `push_structure` makes a replayed run — however
/// many structural and pixel steps it contained — ONE undo press, and
/// redo re-applies the run's whole result.
#[test]
fn structure_snapshot_is_one_undo_press_for_a_whole_run() {
    let mut doc = Document::new(64, 64);
    doc.begin_op();
    doc.layers[0].tile_mut(TileIdx::new(0, 0)).data_mut()[0] = 123;
    doc.end_op();
    let before = doc.layers.clone();
    let active_before = doc.active;
    // The "run": a structural step (clears history) then a pixel step
    // (pushes its own group) — both superseded by the snapshot pair.
    doc.add_layer("SFX");
    let li = doc.active;
    doc.begin_op();
    doc.layers[li].tile_mut(TileIdx::new(0, 0)).data_mut()[0] = 77;
    doc.end_op();
    doc.push_structure("Create SFX Layer", before, active_before);
    assert_eq!(doc.undo_labels().len(), 1, "the run's step stands alone");
    assert_eq!(doc.undo_labels()[0], "Create SFX Layer");
    assert!(doc.undo());
    assert_eq!(doc.layers.len(), 1, "stack restored wholesale");
    assert_eq!(doc.active, active_before);
    assert_eq!(
        doc.layers[0].tile_arc(TileIdx::new(0, 0)).unwrap().data()[0],
        123,
        "pre-run pixels intact"
    );
    assert!(doc.redo());
    assert_eq!(doc.layers.len(), 2, "redo re-applies the run");
    assert_eq!(doc.layers[1].name, "SFX");
    assert_eq!(
        doc.layers[1].tile_arc(TileIdx::new(0, 0)).unwrap().data()[0],
        77,
        "run-created pixels return"
    );
}

/// docs/CLIPPING-SCENARIOS.md 2a: a layer above a folder clips to the
/// folder's combined ink — the header is a valid base now, and a clip
/// run above it shares that base. A THROUGH folder has no isolated
/// composite to take an alpha from, so it still breaks the chain.
#[test]
fn clip_above_a_folder_resolves_to_the_folder() {
    let mut doc = Document::new(64, 64);
    let hi = doc.add_folder_above(0, "F");
    doc.add_layer_in_folder(hi, "in").unwrap();
    let hi = hi + 1; // the child inserted below shifted the header up
    let top = doc.add_layer_above(hi, "Shade");
    assert!(doc.set_layer_clip(top, true));
    let above = doc.add_layer("Shade 2");
    assert!(doc.set_layer_clip(above, true));
    let bases = doc.clip_bases();
    assert_eq!(bases[top], Some(hi), "folder header is the base");
    assert_eq!(
        bases[above],
        Some(hi),
        "the run resolves through the member"
    );
    assert!(doc.set_folder_through(hi, true));
    let bases = doc.clip_bases();
    assert_eq!(bases[top], None, "a through folder breaks the chain");
    assert_eq!(bases[above], None, "for the whole run");
}

/// docs/CLIPPING-SCENARIOS.md #1/#2: a new layer added from the base —
/// or from any member — of a clip run lands ABOVE the run, never inside
/// it. Inside, the run would silently re-base onto the new empty layer
/// and every clipped member would go invisible.
#[test]
fn new_layer_hops_above_the_clip_run() {
    let mut doc = Document::new(64, 64);
    let base = doc.add_layer("Base");
    let c1 = doc.add_layer("Clip 1");
    doc.set_layer_clip(c1, true);
    let c2 = doc.add_layer("Clip 2");
    doc.set_layer_clip(c2, true);
    doc.set_active(base);
    let n = doc.add_layer("Pasted");
    assert_eq!(n, c2 + 1, "insert hops above the whole run");
    let bases = doc.clip_bases();
    assert_eq!(bases[c1], Some(base), "run keeps its base");
    assert_eq!(bases[c2], Some(base));
    // From a mid-run member: same landing spot.
    doc.set_active(c1);
    let m = doc.add_layer("Mid add");
    assert_eq!(m, c2 + 1, "mid-run insert lands just above the run");
    assert_eq!(doc.clip_bases()[c1], Some(base));
}

#[test]
fn alpha_lock_masks_the_open_op_and_locks_guard_merge() {
    let mut doc = Document::new(128, 128);
    // Base state: alpha ramp 0 / half / full across three pixels.
    let idx = TileIdx::new(0, 0);
    doc.layers[0].tile_mut(idx).set_pixel(0, 0, [0, 0, 0, 0]);
    doc.layers[0]
        .tile_mut(idx)
        .set_pixel(1, 0, [8192, 8192, 8192, 16384]);
    doc.layers[0]
        .tile_mut(idx)
        .set_pixel(2, 0, [0, 0, 32768, 32768]);

    doc.begin_op();
    // Paint opaque red over all three.
    for x in 0..3 {
        doc.layers[0]
            .tile_mut(idx)
            .set_pixel(x, 0, [32768, 0, 0, 32768]);
    }
    doc.mask_op_to_alpha();
    doc.end_op();

    let t = doc.layers[0].tile(idx).unwrap();
    assert_eq!(t.pixel(0, 0)[3], 0, "alpha 0 keeps nothing");
    assert_eq!(t.pixel(1, 0)[3], 16384, "alpha is preserved EXACTLY");
    assert_eq!(
        t.pixel(2, 0),
        [32768, 0, 0, 32768],
        "opaque takes full paint"
    );
    // Half-alpha pixel now shows pure red at its original opacity: the
    // opaque stroke replaces the colour (sa = 1), alpha stays 0.5.
    assert_eq!(t.pixel(1, 0), [16384, 0, 0, 16384]);

    // A brand-new tile under alpha lock evaporates.
    let idx2 = TileIdx::new(1, 1);
    doc.begin_op();
    doc.layers[0]
        .tile_mut(idx2)
        .set_pixel(0, 0, [32768, 0, 0, 32768]);
    doc.mask_op_to_alpha();
    doc.end_op();
    assert!(doc.layers[0].tile(idx2).is_none());

    // Locked layers refuse merges.
    doc.add_layer("top");
    doc.set_layer_lock(1, true);
    assert!(!doc.merge_down(1));
    doc.set_layer_lock(1, false);
    assert_eq!(
        doc.merge_down_refusal(1),
        None,
        "unlocked again, the merge is allowed (a clipped layer bakes what it shows — see the app suite)"
    );
}

#[test]
fn folder_blocks_move_remove_and_duplicate_together() {
    let mut doc = Document::new(128, 128);
    let fs = FrameSet::single_rect([16.0, 16.0, 112.0, 112.0], 2.0);
    doc.add_frame_folder("F", fs);
    doc.rename_layer(0, "Base");
    // [Base, White, Layer 1, F]

    // Move the folder block below Base: slot 0.
    assert!(doc.move_block_to_slot(3, 0, 0));
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["White", "Layer 1", "F", "Base"]);
    assert_eq!(doc.children_range(2), 0..2, "children travelled with it");
    assert_eq!(doc.active, 1, "active stayed on the draw layer");

    // Refuse dropping a folder into itself.
    assert!(!doc.move_block_to_slot(2, 1, 1));

    // Duplicate copies the whole block.
    let at = doc.duplicate_layer(2).unwrap();
    assert_eq!(doc.layers.len(), 7);
    assert_eq!(doc.layers[at].name, "F copy");
    assert!(doc.layers[at].folder);
    assert_eq!(doc.children_range(at).len(), 2);

    // Remove takes children along; the last block cannot be removed.
    assert!(doc.remove_layer(at));
    assert_eq!(doc.layers.len(), 4);
    assert!(doc.remove_layer(3), "Base can go");
    assert!(
        !doc.remove_layer(2),
        "removing the only remaining block would empty the doc"
    );
}

#[test]
fn depth_normalization_clamps_orphans() {
    let mut doc = Document::new(64, 64);
    doc.add_layer("A");
    // Force an invalid depth by hand, then run any structural op.
    doc.layers[1].depth = 3;
    doc.add_layer_above(1, "B");
    assert_eq!(doc.layers[1].depth, 0, "no folder above — depth clamped");
    assert_eq!(doc.layers[2].depth, 0);
}

#[test]
fn add_layer_in_folder_and_folder_above() {
    let mut doc = Document::new(64, 64);
    let fi = doc.add_folder_above(0, "Folder");
    assert!(doc.layers[fi].folder && doc.layers[fi].open);
    let li = doc.add_layer_in_folder(fi, "Inner").unwrap();
    assert_eq!(doc.layers[li].depth, 1);
    assert_eq!(doc.children_range(fi + 1), li..li + 1);
    assert!(doc.add_layer_in_folder(0, "nope").is_none());
}

/// PC-002: the folder wears its topmost child's palette colour, unless it
/// has one of its own. The nesting case is the one worth pinning — the
/// scan is flat, and it must still give the recursive answer.
#[test]
fn a_folder_shows_the_topmost_palette_colour_from_inside_it() {
    const RED: [u8; 3] = [200, 40, 40];
    const BLUE: [u8; 3] = [40, 60, 200];
    const GREEN: [u8; 3] = [40, 200, 60];

    // `add_layer_in_folder` inserts AT the header's index (new topmost
    // child), so the header shifts up by one each time.
    let mut doc = Document::new(64, 64);
    let mut outer = doc.add_folder_above(0, "Outer");
    let lower = doc.add_layer_in_folder(outer, "lower").unwrap();
    outer += 1;
    let upper = doc.add_layer_in_folder(outer, "upper").unwrap();
    outer += 1;
    assert!(doc.layers[outer].folder && doc.layers[upper].depth == 1);

    // Nothing labelled: no colour invented.
    assert_eq!(doc.palette_colour(outer), None, "empty of labels: bare");

    doc.set_layer_label(lower, Some(RED));
    assert_eq!(doc.palette_colour(outer), Some(RED), "the only one there");
    doc.set_layer_label(upper, Some(BLUE));
    assert_eq!(
        doc.palette_colour(outer),
        Some(BLUE),
        "TOPmost child wins, not the first one found from the bottom"
    );

    // The folder's own colour beats anything inside it (CSP skips the
    // inheritance entirely once the folder has one).
    doc.set_layer_label(outer, Some(GREEN));
    assert_eq!(doc.palette_colour(outer), Some(GREEN));
    doc.set_layer_label(outer, None);

    // Neither neighbour outside the folder leaks in.
    let above = doc.add_layer_above(outer, "above");
    doc.set_layer_label(above, Some(GREEN));
    doc.set_layer_label(0, Some(GREEN));
    assert_eq!(doc.palette_colour(above), Some(GREEN), "its own label");
    assert_eq!(
        doc.palette_colour(outer),
        Some(BLUE),
        "no leak from outside"
    );

    // Nesting: an inner folder with no colour of its own is transparent to
    // the flat scan — the outer folder still finds the label further in,
    // which is the answer recursion would have given.
    let mut doc = Document::new(64, 64);
    let outer = doc.add_folder_above(0, "Outer");
    let child = doc.add_layer_in_folder(outer, "child").unwrap();
    let inner = doc.add_folder_above(child, "Inner");
    let deep = doc.add_layer_in_folder(inner, "deep").unwrap();
    let (inner, outer) = (inner + 1, outer + 3);
    assert!(doc.layers[inner].folder && doc.layers[outer].folder);
    assert_eq!(doc.layers[deep].depth, 2, "two levels in");
    doc.set_layer_label(deep, Some(RED));
    assert_eq!(doc.palette_colour(inner), Some(RED), "one level up");
    assert_eq!(doc.palette_colour(outer), Some(RED), "and two");
}

#[test]
fn blend_variants_exist() {
    // Present-but-unused by design; keeps the enum from being "added later"
    // in a way that churns match arms across three crates.
    let all = [Blend::Normal, Blend::Multiply, Blend::Screen];
    assert_eq!(all[0], Blend::default());
}

#[test]
fn resize_grow_center_pins_content_to_the_middle() {
    let mut doc = Document::new(128, 128);
    let put = |doc: &mut Document, x: i32, y: i32| {
        let ti = TileIdx::of_pixel(x, y);
        let (ox, oy) = ti.origin();
        doc.layers[0].tile_mut(ti).set_pixel(
            (x - ox) as usize,
            (y - oy) as usize,
            [1, 2, 3, FIX15_ONE as u16],
        );
    };
    let get = |doc: &Document, x: i32, y: i32| -> Option<[u16; 4]> {
        let ti = TileIdx::of_pixel(x, y);
        doc.layers[0]
            .tile(ti)
            .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize))
    };
    put(&mut doc, 10, 10);
    doc.begin_op();
    put(&mut doc, 11, 10);
    doc.end_op();
    assert!(doc.can_undo());

    doc.resize_canvas(256, 256, ResizeAnchor::Center);
    assert_eq!(doc.size, (256, 256));
    // (256-128)/2 = +64: the pixel moved with the content.
    assert_eq!(get(&doc, 74, 74).unwrap()[3], FIX15_ONE as u16);
    assert!(get(&doc, 10, 10).is_none(), "source vacated");
    assert!(!doc.can_undo(), "structural: history cleared");
    assert!(doc.selection.is_none());
}

#[test]
fn crop_offsets_and_trims_outside_tiles() {
    let mut doc = Document::new(256, 256);
    let put = |doc: &mut Document, x: i32, y: i32| {
        let ti = TileIdx::of_pixel(x, y);
        let (ox, oy) = ti.origin();
        doc.layers[0].tile_mut(ti).set_pixel(
            (x - ox) as usize,
            (y - oy) as usize,
            [9, 9, 9, FIX15_ONE as u16],
        );
    };
    let get = |doc: &Document, x: i32, y: i32| -> Option<[u16; 4]> {
        let ti = TileIdx::of_pixel(x, y);
        doc.layers[0]
            .tile(ti)
            .map(|t| t.pixel((x - ti.origin().0) as usize, (y - ti.origin().1) as usize))
    };
    put(&mut doc, 130, 130); // tile (2, 2) — survives the crop
    put(&mut doc, 200, 200); // tile (3, 3) — fully outside 64x64, dropped

    // Crop to the 64x64 area whose top-left is (128, 128).
    doc.resize_to(64, 64, -128, -128);
    assert_eq!(doc.size, (64, 64));
    assert_eq!(
        get(&doc, 2, 2).unwrap()[3],
        FIX15_ONE as u16,
        "kept pixel at origin"
    );
    assert!(
        doc.layers[0].tile(TileIdx::new(3, 3)).is_none(),
        "outside tile trimmed"
    );
}

#[test]
fn resize_translates_frame_vectors_and_reextends_white() {
    let mut doc = Document::new(256, 256);
    let fs = FrameSet::single_rect([32.0, 32.0, 224.0, 224.0], 4.0);
    let hi = doc.add_frame_folder("Frame 1", fs);
    let white = doc.layers.iter().position(|l| l.name == "White").unwrap();

    // Grow top-left-anchored: content stays, new paper appears right/below.
    doc.resize_canvas(512, 512, ResizeAnchor::TopLeft);
    assert_eq!(doc.size, (512, 512));
    // The frame moved with the content (dx = dy = 0 here, but the
    // rasters re-derived at the larger size and White re-extended).
    assert_eq!(
        doc.layers[hi].frames().unwrap().frames[0].points[0],
        [32.0, 32.0]
    );
    let far = TileIdx::new(7, 7);
    assert_eq!(
        doc.layers[white].tile(far).map(|t| t.pixel(1, 1)),
        Some([FIX15_ONE as u16; 4]),
        "White covers the grown corner"
    );
    assert!(doc.layers[hi].mask_tiles().is_some(), "mask re-derived");

    // Shrink + move: vectors follow the offset.
    doc.resize_to(256, 256, -64, -64);
    assert_eq!(
        doc.layers[hi].frames().unwrap().frames[0].points[0],
        [-32.0, -32.0]
    );
}

#[test]
fn anchor_offsets_match_the_nine_positions() {
    use ResizeAnchor::*;
    let old = (100u32, 100u32);
    assert_eq!(TopLeft.offsets(old, (300, 50)), (0, 0));
    assert_eq!(Center.offsets(old, (300, 50)), (100, -25));
    assert_eq!(BottomRight.offsets(old, (300, 50)), (200, -50));
    assert_eq!(Top.offsets(old, (300, 50)), (100, 0));
    assert_eq!(Left.offsets(old, (300, 50)), (0, -25));
}

#[test]
fn tone_conversion_is_nondestructive_and_undoable() {
    use crate::tone::ToneParams;
    let mut doc = Document::new(256, 256);
    // A half-ink block in the middle tile — non-blank through the screen
    // at any phase (32×32 at ~50 % coverage), and guaranteed DIFFERENT
    // from the uniform source (full ink rasterizes to itself: 100 %
    // coverage is solid black by design).
    doc.begin_op();
    {
        let t = doc.active_layer_mut().tile_mut(TileIdx::new(2, 2));
        for y in 16..48 {
            for x in 16..48 {
                t.set_pixel(x, y, [0, 0, 0, (FIX15_ONE / 2) as u16]);
            }
        }
    }
    doc.end_op();
    let src_arc = doc
        .active_layer()
        .tile(TileIdx::new(2, 2))
        .cloned()
        .unwrap();

    assert!(doc.set_tone(0, Some(ToneParams::default())));
    doc.refresh_derived(600);
    let shown = doc.layers[0]
        .display_tile(TileIdx::new(2, 2))
        .cloned()
        .unwrap();
    assert!(!shown.is_blank(), "tone raster derived");
    assert_ne!(shown.data(), src_arc.data(), "display differs from source");
    // The SOURCE is untouched — that is the non-destructive half.
    assert_eq!(
        doc.active_layer().tile(TileIdx::new(2, 2)).unwrap().data(),
        src_arc.data()
    );

    // Undo restores plain pixels as one step; redo re-tons.
    assert!(doc.undo());
    assert!(doc.layers[0].tone.is_none());
    assert_eq!(
        doc.layers[0]
            .display_tile(TileIdx::new(2, 2))
            .unwrap()
            .data(),
        src_arc.data(),
        "after undo the display is the plain source again"
    );
    assert!(doc.redo());
    doc.refresh_derived(600);
    assert!(doc.layers[0].tone.is_some());

    // Vector layers and folders refuse conversion.
    doc.add_balloon_layer("B", crate::balloon::BalloonSet::new(2.0));
    assert!(!doc.set_tone(1, Some(ToneParams::default())));
}

#[test]
fn tone_raster_follows_new_source_edits() {
    use crate::tone::ToneParams;
    let mut doc = Document::new(256, 256);
    doc.active_layer_mut()
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(10, 10, [0, 0, 0, FIX15_ONE as u16]);
    assert!(doc.set_tone(0, Some(ToneParams::default())));
    doc.refresh_derived(600);
    let rev1 = doc.layers[0]
        .display_tile(TileIdx::new(0, 0))
        .unwrap()
        .revision();

    // Paint more ink on the source (as a stroke would): the derived tile
    // must re-derive with a newer revision; untouched tiles don't.
    doc.active_layer_mut()
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(12, 12, [0, 0, 0, FIX15_ONE as u16]);
    doc.refresh_derived(600);
    let rev2 = doc.layers[0]
        .display_tile(TileIdx::new(0, 0))
        .unwrap()
        .revision();
    assert!(rev2 > rev1, "dirty source re-derives");

    // A clean refresh is a no-op (same revision object).
    doc.refresh_derived(600);
    assert_eq!(
        doc.layers[0]
            .display_tile(TileIdx::new(0, 0))
            .unwrap()
            .revision(),
        rev2
    );

    // Param change re-derives everything (map dropped).
    let mut p = ToneParams::default();
    p.lpi = 42.5;
    assert!(doc.set_tone(0, Some(p)));
    doc.refresh_derived(600);
    assert!(
        doc.layers[0]
            .display_tile(TileIdx::new(0, 0))
            .unwrap()
            .revision()
            > rev2
    );
}
