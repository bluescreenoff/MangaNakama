use super::super::*;
use crate::tile::FIX15_ONE;

/// FB-037: a common parent wraps both frame folders — headers and
/// shapes survive, both blocks deepen by one.
#[test]
fn common_parent_wraps_without_touching_originals() {
    let mut doc = Document::new(400, 400);
    let a = doc.add_frame_folder(
        "Frame 1",
        FrameSet::single_rect([0.0, 0.0, 180.0, 300.0], 4.0),
    );
    let b = doc.add_frame_folder(
        "Frame 2",
        FrameSet::single_rect([200.0, 0.0, 380.0, 300.0], 4.0),
    );
    let fa = doc.layers[a].frames().unwrap().clone();
    let fb = doc.layers[b].frames().unwrap().clone();
    let h = doc
        .group_frame_folders_common_parent(a, b)
        .expect("grouped");
    assert!(
        doc.layers[h].folder && !doc.layers[h].is_frame(),
        "a PLAIN parent"
    );
    assert_eq!(doc.layers[h].depth, 0);
    // Both frame folders survive verbatim, one level deeper.
    let kids = doc.children_range(h);
    let frames: Vec<_> = kids.clone().filter(|&i| doc.layers[i].is_frame()).collect();
    assert_eq!(frames.len(), 2, "both headers survive");
    for &fh in &frames {
        assert_eq!(doc.layers[fh].depth, 1);
    }
    assert!(
        frames
            .iter()
            .any(|&fh| doc.layers[fh].frames().unwrap().frames[0].bbox() == fa.frames[0].bbox())
            && frames
                .iter()
                .any(|&fh| doc.layers[fh].frames().unwrap().frames[0].bbox()
                    == fb.frames[0].bbox()),
        "shapes untouched"
    );
}

/// The r83 handoff flag: combining across an INTERVENING plain
/// folder block — the pooled children land inside the surviving
/// block and the intervening folder stays intact.
#[test]
fn combine_across_an_intervening_folder() {
    let mut doc = Document::new(400, 400);
    doc.add_frame_folder(
        "Frame 1",
        FrameSet::single_rect([0.0, 0.0, 180.0, 300.0], 4.0),
    );
    // A plain folder BETWEEN the two frame folders' blocks: insert
    // above the ROOT layer so it lands outside Frame 1's block.
    doc.add_folder_above(0, "notes");
    doc.add_frame_folder(
        "Frame 2",
        FrameSet::single_rect([200.0, 0.0, 380.0, 300.0], 4.0),
    );
    // Resolve by name — indices shifted with every insert.
    let a = doc
        .layers
        .iter()
        .position(|l| l.name == "Frame 1" && l.is_frame())
        .unwrap();
    let b = doc
        .layers
        .iter()
        .position(|l| l.name == "Frame 2" && l.is_frame())
        .unwrap();
    let plain = doc.layers.iter().position(|l| l.name == "notes").unwrap();
    let before_plain = doc.layers[plain].clone();
    let plain_kids = doc.children_range(plain).len();
    let h = doc.combine_frame_folders(a, b, false).expect("combined");
    // One frame folder fewer; both frames in the survivor; the plain
    // folder block untouched.
    assert_eq!(
        doc.layers.iter().filter(|l| l.is_frame()).count(),
        1,
        "one frame folder remains"
    );
    assert_eq!(doc.layers[h].frames().unwrap().frames.len(), 2);
    let p = doc.layers.iter().position(|l| l.name == "notes").unwrap();
    assert_eq!(doc.layers[p].depth, before_plain.depth);
    assert_eq!(
        doc.children_range(p).len(),
        plain_kids,
        "plain folder intact"
    );
}

/// LP-002/LP-003. The property the whole feature rests on: the keyline is
/// grown from the layer's OWN alpha and lands in tiles the layer never
/// painted — the dilation is not pointwise, and a compositor that only
/// walked source tiles would clip the outline at every tile edge. Also
/// the non-destructive half: the pixels survive and one undo restores.
#[test]
fn border_effect_rings_the_ink_across_a_tile_edge() {
    use crate::edge::EdgeParams;
    let mut doc = Document::new(256, 256);
    // One opaque pixel in the LAST column of tile (0,0), so a 6 px
    // keyline round it has to spill into tile (1,0).
    doc.begin_op();
    doc.active_layer_mut()
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(63, 32, [0, 0, 0, FIX15_ONE as u16]);
    doc.end_op();
    let src = doc
        .active_layer()
        .tile(TileIdx::new(0, 0))
        .cloned()
        .unwrap();

    assert!(doc.set_edge(
        0,
        Some(EdgeParams {
            width_px: 6.0,
            colour: [255, 255, 255],
            ..EdgeParams::default()
        })
    ));
    doc.refresh_derived(600);

    // Canvas (66,32) is 3 px from the ink, in the NEXT tile along.
    let right = doc.layers[0]
        .display_tile(TileIdx::new(1, 0))
        .cloned()
        .expect("the neighbour tile is displayed even with no source in it");
    assert_eq!(right.pixel(2, 32), [32768, 32768, 32768, 32768]);
    assert_eq!(right.pixel(60, 60), [0; 4], "and it stops well short");
    assert!(
        doc.layers[0].tile(TileIdx::new(1, 0)).is_none(),
        "no SOURCE tile was invented to hold the outline"
    );
    // The ink shows through the ring, and the painted tile is untouched.
    assert_eq!(
        doc.layers[0]
            .display_tile(TileIdx::new(0, 0))
            .unwrap()
            .pixel(63, 32),
        [0, 0, 0, 32768],
        "the ink itself is still black"
    );
    assert_eq!(
        doc.layers[0].tile(TileIdx::new(0, 0)).unwrap().data(),
        src.data(),
        "the painted pixels never changed"
    );

    // One undo step, and the layer is exactly the drawing again.
    assert!(doc.undo());
    assert!(doc.layers[0].edge.is_none());
    assert!(
        doc.layers[0].display_tile(TileIdx::new(1, 0)).is_none(),
        "no outline left over in the neighbour tile"
    );
    assert_eq!(
        doc.layers[0]
            .display_tile(TileIdx::new(0, 0))
            .unwrap()
            .data(),
        src.data()
    );
}

/// The other half of TRIAGE 27: the outline must FOLLOW the layer. New
/// ink on an already-outlined layer gets its own keyline on the next
/// refresh — this is what the `edge_stamp` early-out is allowed to skip
/// and must not skip wrongly. Folders have no alpha of their own and are
/// refused outright.
#[test]
fn border_effect_follows_new_ink_and_refuses_folders() {
    use crate::edge::EdgeParams;
    let mut doc = Document::new(256, 256);
    doc.begin_op();
    doc.active_layer_mut()
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(10, 10, [0, 0, 0, FIX15_ONE as u16]);
    doc.end_op();
    let p = EdgeParams {
        width_px: 6.0,
        colour: [255, 255, 255],
        ..Default::default()
    };
    assert!(doc.set_edge(0, Some(p)));
    doc.refresh_derived(600);
    assert!(
        doc.layers[0].display_tile(TileIdx::new(2, 2)).is_none(),
        "a tile the outline cannot reach is never derived"
    );

    // Draw somewhere new; refresh; the new mark is ringed too.
    doc.begin_op();
    doc.active_layer_mut()
        .tile_mut(TileIdx::new(2, 2))
        .set_pixel(10, 10, [0, 0, 0, FIX15_ONE as u16]);
    doc.end_op();
    doc.refresh_derived(600);
    let t = doc.layers[0]
        .display_tile(TileIdx::new(2, 2))
        .cloned()
        .expect("the new ink's tile is derived");
    assert_eq!(t.pixel(14, 10), [32768, 32768, 32768, 32768]);
    assert_eq!(t.pixel(10, 10), [0, 0, 0, 32768], "the new ink is intact");
    // …and the OLD mark still has its ring (the cache reuse is not a leak).
    assert_eq!(
        doc.layers[0]
            .display_tile(TileIdx::new(0, 0))
            .unwrap()
            .pixel(14, 10),
        [32768, 32768, 32768, 32768]
    );

    // Setting the same params again is a no-op — no undo step is spent.
    assert!(!doc.set_edge(0, Some(p)));
    // FB-knockout: a PLAIN folder accepts (the group mat); a FRAME
    // folder refuses — its close already owns a mask + border ink.
    let fi = doc.add_folder_above(0, "Folder");
    assert!(doc.set_edge(fi, Some(p)), "plain folder takes the mat");
    assert!(doc.set_edge(fi, None));
    let hi = doc.add_frame_folder(
        "F",
        crate::frame::FrameSet::single_rect([1.0, 1.0, 9.0, 9.0], 2.0),
    );
    assert!(!doc.set_edge(hi, Some(p)), "frame folder refuses");
}

/// The early-out's sharp edge. One op that PRUNES an emptied tile and
/// creates another leaves the source-tile COUNT unchanged, and where the
/// ink vanished the neighbourhood's newest revision DROPS — so a cache
/// keyed on the count alone still reads "derived after the newest
/// source" and keeps a ghost outline round ink that is gone. The stamp
/// hashes the tile SET for exactly this.
#[test]
fn border_effect_does_not_ghost_when_one_tile_is_traded_for_another() {
    use crate::edge::EdgeParams;
    let mut doc = Document::new(512, 512);
    doc.begin_op();
    doc.active_layer_mut()
        .tile_mut(TileIdx::new(0, 0))
        .set_pixel(10, 10, [0, 0, 0, FIX15_ONE as u16]);
    doc.end_op();
    assert!(doc.set_edge(
        0,
        Some(EdgeParams {
            width_px: 4.0,
            colour: [255, 255, 255],
            ..Default::default()
        })
    ));
    doc.refresh_derived(600);
    assert_eq!(
        doc.layers[0]
            .display_tile(TileIdx::new(0, 0))
            .unwrap()
            .pixel(13, 10)[3],
        32768
    );

    // Same count, different set.
    let mut moved = Tile::new_transparent();
    moved.set_pixel(10, 10, [0, 0, 0, FIX15_ONE as u16]);
    doc.layers[0].set_tile(TileIdx::new(0, 0), None);
    doc.layers[0].set_tile(TileIdx::new(4, 4), Some(Arc::new(moved)));
    doc.refresh_derived(600);
    assert!(
        doc.layers[0].display_tile(TileIdx::new(0, 0)).is_none(),
        "the outline left with the ink it belonged to"
    );
    assert_eq!(
        doc.layers[0]
            .display_tile(TileIdx::new(4, 4))
            .expect("the ink's new home is outlined")
            .pixel(13, 10)[3],
        32768
    );
}

/// LP-017 + LP-022 are presentation switches: they change what is shown,
/// spend no undo step (like visibility), and reject a no-op.
#[test]
fn sub_colour_and_expression_are_presentation_switches() {
    let mut doc = Document::new(64, 64);
    let before = doc.undo_len();
    assert!(doc.set_layer_sub_colour(0, Some([1, 2, 3])));
    assert!(doc.set_layer_expression(0, LayerExpression::Mono));
    assert_eq!(doc.layers[0].layer_sub_colour, Some([1, 2, 3]));
    assert_eq!(doc.layers[0].expression, LayerExpression::Mono);
    assert!(
        !doc.set_layer_expression(0, LayerExpression::Mono),
        "setting what is already set is not a change"
    );
    assert_eq!(doc.undo_len(), before, "no undo step is spent");
    assert!(doc.set_layer_sub_colour(0, None));
    assert!(doc.set_layer_expression(0, LayerExpression::Colour));
    // Out-of-range indices are a false, not a panic.
    assert!(!doc.set_layer_sub_colour(99, None));
    assert!(!doc.set_layer_expression(99, LayerExpression::Grey));
    assert!(!doc.set_edge(99, None));
}
