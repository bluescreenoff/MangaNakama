use super::super::*;

/// FB-035/036: combining two divide-siblings pools children under
/// one header; with `merge_borders` the two adjacent rects become
/// ONE frame at the union bbox; keep-shapes concatenates.
#[test]
fn combine_frame_folders_pools_and_merges() {
    let mut doc = Document::new(400, 400);
    let a = doc.add_frame_folder(
        "Frame 1",
        FrameSet::single_rect([16.0, 16.0, 200.0, 300.0], 4.0),
    );
    let b = doc.add_frame_folder(
        "Frame 2",
        FrameSet::single_rect([200.0, 16.0, 384.0, 300.0], 4.0),
    );
    let before = doc.layers.len();
    // Keep shapes: two frames, one header, all children pooled.
    let h = doc.combine_frame_folders(a, b, false).expect("combined");
    assert_eq!(doc.layers.len(), before - 1, "one header gone");
    assert!(doc.layers[h].is_frame());
    let fs = doc.layers[h].frames().unwrap();
    assert_eq!(fs.frames.len(), 2, "shapes kept");
    assert!(doc.children_range(h).len() >= 4, "children pooled");
    assert!(doc.layers.iter().filter(|l| l.is_frame()).count() == 1);

    // Combine-borders on adjacent siblings: one union-bbox frame.
    let mut doc2 = Document::new(400, 400);
    let a2 = doc2.add_frame_folder(
        "Frame 1",
        FrameSet::single_rect([16.0, 16.0, 200.0, 300.0], 4.0),
    );
    let b2 = doc2.add_frame_folder(
        "Frame 2",
        FrameSet::single_rect([200.0, 16.0, 384.0, 300.0], 4.0),
    );
    let h2 = doc2.combine_frame_folders(a2, b2, true).expect("combined");
    let fs2 = doc2.layers[h2].frames().unwrap();
    assert_eq!(fs2.frames.len(), 1, "one merged border");
    let u = fs2.frames[0].bbox();
    assert_eq!(u, [16.0, 16.0, 384.0, 300.0], "the union bbox");
    // Self-combine refuses (no meaningful merge with itself).
    let mut doc3 = Document::new(400, 400);
    let x = doc3.add_frame_folder("F", FrameSet::single_rect([0.0, 0.0, 100.0, 100.0], 4.0));
    assert!(doc3.combine_frame_folders(x, x, false).is_none());
    // Differing depths refuse (a nested child and a top-level folder).
    let y = doc3.add_frame_folder("G", FrameSet::single_rect([0.0, 0.0, 100.0, 100.0], 4.0));
    let _p = doc3
        .group_frame_folders_common_parent(x, y)
        .expect("grouped for the depth case");
    // `x` and `y` now nest at depth 1 inside the new parent.
    let z = doc3.add_frame_folder("H", FrameSet::single_rect([0.0, 0.0, 100.0, 100.0], 4.0));
    assert!(
        doc3.combine_frame_folders(x, z, false).is_none(),
        "differing depths refuse"
    );
}

/// The combine DESTROYS B's header, so everything the compositor reads
/// from a folder (visibility, opacity, blend, through, draft) and
/// everything that lives on the `FrameSet` rather than on a `Frame`
/// (border width, the ruler flag, the reading pin) went with it — B's
/// panels silently re-rendered wearing A's look. None of it pushes down:
/// a GROUP blend/opacity is not a per-child blend/opacity, a hidden
/// group is not a hidden child, and a `Frame` has no border of its own.
/// So a pair that disagrees refuses rather than restyling art.
#[test]
fn combine_frame_folders_refuses_to_silently_restyle_the_partner() {
    let build = || {
        let mut doc = Document::new(400, 400);
        let a = doc.add_frame_folder(
            "Frame 1",
            FrameSet::single_rect([16.0, 16.0, 200.0, 300.0], 4.0),
        );
        let b = doc.add_frame_folder(
            "Frame 2",
            FrameSet::single_rect([200.0, 16.0, 384.0, 300.0], 4.0),
        );
        (doc, a, b)
    };
    // Two folders that agree still combine — the divide-siblings case.
    let (mut doc, a, b) = build();
    assert!(doc.combine_frame_folders(a, b, false).is_some());

    let cases: [(&str, fn(&mut Layer)); 9] = [
        ("blend", |l| l.blend = Blend::Multiply),
        ("opacity", |l| l.opacity = 0.5),
        ("visibility", |l| l.visible = false),
        ("through", |l| l.through = true),
        ("draft", |l| l.draft = true),
        ("mask", |l| l.mask = Some(LayerMask::default())),
        ("border width", |l| {
            l.frames_mut().unwrap().border_px = 9.0;
        }),
        ("border ruler", |l| {
            l.frames_mut().unwrap().border_ruler = true;
        }),
        ("reading pin", |l| {
            l.frames_mut().unwrap().reading_pin = Some(3);
        }),
    ];
    for (what, edit) in cases {
        let (mut doc, a, b) = build();
        edit(&mut doc.layers[b]);
        assert!(
            doc.combine_frame_folders(a, b, false).is_none(),
            "B's {what} would have been dropped"
        );
        // The same disagreement refuses from either side.
        let (mut doc, a, b) = build();
        edit(&mut doc.layers[a]);
        assert!(
            doc.combine_frame_folders(a, b, false).is_none(),
            "A's {what} would have been forced onto B"
        );
    }
}

#[test]
fn group_common_parent_splices_separated_siblings() {
    // Audit E, 2026-08-19: the old guard REFUSED any non-adjacent
    // sibling pair as "not siblings" (the condition tested for
    // separation), and its insert position would have left the lower
    // block outside the parent. Separated siblings group by splicing
    // the lower block adjacent to the higher one (CSP semantics: the
    // selection moves to the highest position; intervening layers
    // stay put, below both). Indices MOVE in a splice — re-find by
    // name after the call.
    let mut doc = Document::new(400, 400);
    let a = doc.add_frame_folder(
        "Frame 1",
        FrameSet::single_rect([16.0, 16.0, 184.0, 300.0], 4.0),
    );
    let b0 = doc.add_frame_folder(
        "Frame 2",
        FrameSet::single_rect([216.0, 16.0, 384.0, 300.0], 4.0),
    );
    // A plain TOP-LEVEL layer BETWEEN the two blocks (add_layer would
    // land inside Frame 1's block — the active layer is its draw
    // layer — so insert explicitly at Frame 2's block start).
    let mut between = Layer::new("bg");
    between.depth = 0;
    doc.layers.insert(b0 - 2, between);
    let b = b0 + 1;
    let h = doc
        .group_frame_folders_common_parent(a, b)
        .expect("grouped");
    assert!(
        doc.layers[h].folder && !doc.layers[h].is_frame(),
        "a plain parent"
    );
    let kids = doc.children_range(h);
    assert_eq!(kids.len(), 6, "both folders' whole blocks inside");
    let headers: Vec<usize> = kids
        .clone()
        .filter(|&i| doc.layers[i].is_frame() && doc.layers[i].folder)
        .collect();
    assert_eq!(headers.len(), 2, "both frame headers are children");
    assert!(headers.iter().all(|&i| doc.layers[i].depth == 1));
    let bg = doc
        .layers
        .iter()
        .position(|l| l.name == "bg")
        .expect("the plain layer survives");
    assert_eq!(doc.layers[bg].depth, 0, "the plain layer stays outside");
    assert!(bg < kids.start, "the moved block crossed it, staying above");
    assert_eq!(doc.active, h, "the new parent header is the selection");
}

#[test]
fn combine_refuses_folders_in_different_parents() {
    // Audit H: equal depth is not parenthood — folders in DIFFERENT
    // parents must not combine (the merged folder would land in one
    // parent and silently empty the other). Grouping inserts layers,
    // so the headers are re-found by name before each call.
    let hdr =
        |doc: &Document, name: &str| doc.layers.iter().position(|l| l.name == name).unwrap();
    let mut doc = Document::new(400, 400);
    for n in ["A", "B", "C", "D"] {
        doc.add_frame_folder(n, FrameSet::single_rect([0.0, 0.0, 100.0, 100.0], 4.0));
    }
    doc.group_frame_folders_common_parent(hdr(&doc, "A"), hdr(&doc, "B"))
        .expect("P1");
    doc.group_frame_folders_common_parent(hdr(&doc, "C"), hdr(&doc, "D"))
        .expect("P2");
    assert!(
        doc.combine_frame_folders(hdr(&doc, "A"), hdr(&doc, "C"), false)
            .is_none(),
        "same depth, different parents — refused"
    );
    assert!(
        doc.group_frame_folders_common_parent(hdr(&doc, "A"), hdr(&doc, "C"))
            .is_none(),
        "same depth, different parents — refused (group)"
    );
    assert!(
        doc.combine_frame_folders(hdr(&doc, "A"), hdr(&doc, "B"), false)
            .is_some(),
        "true siblings still combine"
    );
}
