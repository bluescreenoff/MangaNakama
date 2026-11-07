//! PSD import/export tests (plan 2026-11-06 §8.1).
//!
//! "Equal" means: metadata identical, pixels identical at 8-bit straight
//! RGBA over the whole canvas, masks identical as 8-bit coverage, opacity
//! within one 8-bit step.
//!
//! Quantisation note for the synthetic pixels: through one 8-bit pass, a
//! fix15 pixel comes back with the same 8-bit value unless its alpha rounds
//! to 1/255 (then the colour can move by one step); from the second pass on
//! every pixel is stable. The synthetic documents therefore use no alpha of
//! 1/255; real-file tests compare a second pass with a third.

use std::sync::Arc;

use photocraft_psd::{BlendMode, GroupSpec, LayerSpec, MaskSpec, PixelData, PsdBuilder, Rect, TaggedBlock};

use super::*;
use crate::blend::straight_u8_to_fix15;
use crate::doc::{Blend, Layer, LayerMask};
use crate::tile::{TILE_SIZE, Tile, TileIdx};

// ---------------------------------------------------------------- helpers

/// Fix15 premultiplied -> straight 8-bit, the export formulas (plan §4.3).
fn straight8(p: [u16; 4]) -> [u8; 4] {
    let a = p[3] as u32;
    if a == 0 {
        return [0, 0, 0, 0];
    }
    let un = |c: u16| (((c as u32 * 32768 / a).min(32768) * 255 + 16384) / 32768) as u8;
    [un(p[0]), un(p[1]), un(p[2]), ((a * 255 + 16384) / 32768) as u8]
}

/// A layer's displayed pixels as a full-canvas straight RGBA8 buffer.
fn flat8(size: (u32, u32), l: &Layer) -> Vec<u8> {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for (idx, t) in l.display_tiles() {
        let (ox, oy) = idx.origin();
        for py in 0..TILE_SIZE {
            for px in 0..TILE_SIZE {
                let (x, y) = (ox + px as i32, oy + py as i32);
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let o = ((y * w + x) * 4) as usize;
                out[o..o + 4].copy_from_slice(&straight8(t.pixel(px, py)));
            }
        }
    }
    out
}

/// A mask as full-canvas 8-bit coverage; an absent tile is 255 (visible).
fn mask8(size: (u32, u32), m: &LayerMask) -> Vec<u8> {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let mut out = vec![255u8; (w * h) as usize];
    for (idx, t) in &m.tiles {
        let (ox, oy) = idx.origin();
        for py in 0..TILE_SIZE {
            for px in 0..TILE_SIZE {
                let (x, y) = (ox + px as i32, oy + py as i32);
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let a = t.pixel(px, py)[3] as u32;
                out[(y * w + x) as usize] = ((a * 255 + 16384) / 32768) as u8;
            }
        }
    }
    out
}

fn assert_docs_equal_8bit(a: &Document, b: &Document, compare_masks: bool) {
    assert_eq!(a.size, b.size, "canvas size");
    let names = |d: &Document| d.layers.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    assert_eq!(a.layers.len(), b.layers.len(), "layer count: {:?} vs {:?}", names(a), names(b));
    for (i, (x, y)) in a.layers.iter().zip(&b.layers).enumerate() {
        let at = format!("layer {i} {:?}", x.name);
        assert_eq!(x.name, y.name, "{at}: name");
        assert_eq!(x.depth, y.depth, "{at}: depth");
        assert_eq!(x.folder, y.folder, "{at}: folder");
        if x.folder {
            assert_eq!(x.open, y.open, "{at}: open");
            assert_eq!(x.through, y.through, "{at}: through");
        }
        assert_eq!(x.visible, y.visible, "{at}: visible");
        assert_eq!(x.clip, y.clip, "{at}: clip");
        assert_eq!(x.blend, y.blend, "{at}: blend");
        assert_eq!(x.lock, y.lock, "{at}: lock");
        assert_eq!(x.lock_alpha, y.lock_alpha, "{at}: lock_alpha");
        assert!((x.opacity - y.opacity).abs() <= 1.0 / 255.0 + 1e-6, "{at}: opacity {} vs {}", x.opacity, y.opacity);
        assert!(flat8(a.size, x) == flat8(b.size, y), "{at}: pixels differ");
        if compare_masks {
            match (&x.mask, &y.mask) {
                (None, None) => {}
                (Some(m), Some(n)) => {
                    assert_eq!(m.enabled, n.enabled, "{at}: mask enabled");
                    assert!(mask8(a.size, m) == mask8(b.size, n), "{at}: mask coverage differs");
                }
                (m, n) => panic!("{at}: mask {} vs {}", m.is_some(), n.is_some()),
            }
        }
    }
}

/// Fill `x0..x1 × y0..y1` of `l` with a straight 8-bit colour (`f` per pixel).
fn fill(l: &mut Layer, x0: i32, y0: i32, x1: i32, y1: i32, f: impl Fn(i32, i32) -> [u8; 4]) {
    let t = TILE_SIZE as i32;
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = TileIdx::new(x.div_euclid(t), y.div_euclid(t));
            let (ox, oy) = idx.origin();
            l.tile_mut(idx).set_pixel((x - ox) as usize, (y - oy) as usize, straight_u8_to_fix15(f(x, y)));
        }
    }
}

/// A mask with coverage `cov(x, y)` (8-bit) in the given tiles only.
fn mask_with(tiles: &[TileIdx], enabled: bool, cov: impl Fn(i32, i32) -> u8) -> LayerMask {
    let mut m = LayerMask { enabled, ..Default::default() };
    for &idx in tiles {
        let (ox, oy) = idx.origin();
        let mut tile = Tile::new_transparent();
        for py in 0..TILE_SIZE {
            for px in 0..TILE_SIZE {
                let c = (cov(ox + px as i32, oy + py as i32) as u32 * 32768 / 255) as u16;
                tile.set_pixel(px, py, [c, c, c, c]);
            }
        }
        m.tiles.insert(idx, Arc::new(tile));
    }
    m
}

/// The synthetic page (plan §8.1 #13): 3+ raster layers with soft alpha, an
/// open folder and a closed pass-through one, a clip, 5 blend modes,
/// Japanese names. `full` adds what only the new writer carries: a mask,
/// lock flags and the resolution.
fn synthetic(full: bool) -> Document {
    let mut doc = Document::new(200, 150);
    let mut layers = Vec::new();

    let mut l = Layer::new("ベタ");
    l.blend = Blend::Multiply;
    l.opacity = 0.8;
    fill(&mut l, 5, 5, 60, 40, |_, _| [20, 30, 40, 255]);
    layers.push(l);

    let mut l = Layer::new("線画");
    l.depth = 1;
    l.blend = Blend::Screen;
    // Soft alpha across a tile edge; alpha 2..=255 (see the module note).
    fill(&mut l, 50, 10, 140, 20, |x, y| [(x * 2) as u8, (y * 9) as u8, 200, (2 + (x - 50) * 253 / 89) as u8]);
    if full {
        l.lock_alpha = true;
    }
    layers.push(l);

    let mut l = Layer::new("影");
    l.depth = 1;
    l.clip = true;
    l.blend = Blend::Overlay;
    fill(&mut l, 60, 12, 100, 30, |_, _| [200, 100, 50, 128]);
    layers.push(l);

    let mut l = Layer::new("顔");
    l.folder = true;
    l.open = true;
    l.opacity = 0.6;
    layers.push(l);

    let mut l = Layer::new("効果");
    l.depth = 1;
    l.blend = Blend::Difference;
    l.visible = false;
    fill(&mut l, 0, 60, 130, 140, |x, _| [255, 255, (x % 256) as u8, 255]);
    if full {
        l.lock = true;
        // Hidden left of x = 32 in tile (0,1), half coverage in (1,1),
        // everything else unmasked.
        l.mask = Some(mask_with(&[TileIdx::new(0, 1), TileIdx::new(1, 1)], true, |x, _| if x < 32 { 0 } else { 128 }));
    }
    layers.push(l);

    let mut l = Layer::new("閉じた folder");
    l.folder = true;
    l.open = false;
    l.through = true;
    layers.push(l);

    let mut l = Layer::new("トーン");
    l.blend = Blend::LinearBurn;
    l.opacity = 0.5;
    fill(&mut l, 150, 100, 200, 150, |x, y| [(x + y) as u8, 0, 99, 255]);
    // A blank tile: it must not widen the exported rectangle.
    l.tile_mut(TileIdx::new(0, 0));
    layers.push(l);

    doc.layers = layers;
    doc.ensure_ids();
    doc.active = doc.layers.len() - 1;
    if full {
        doc.dpi = Some(600);
    }
    doc
}

fn legacy_bytes(doc: &Document) -> Vec<u8> {
    let mut buf = Vec::new();
    crate::psd_legacy::save_psd(doc, &mut buf).unwrap();
    buf
}

fn rgba16(w: u32, h: u32, f: impl Fn(u32, u32) -> [u16; 4]) -> PixelData {
    PixelData::Rgba16((0..h).flat_map(|y| (0..w).map(move |x| (x, y))).flat_map(|(x, y)| f(x, y)).collect())
}

// ---------------------------------------------------------------- import

/// #8: the importer reads what the legacy writer wrote — files CSP has
/// already opened — back into the same page.
#[test]
fn import_reads_the_legacy_writer() {
    let doc = synthetic(false);
    let back = load_from_bytes(&legacy_bytes(&doc)).unwrap();
    assert_docs_equal_8bit(&doc, &back.doc, false);
    assert!(back.notes.is_empty(), "{:?}", back.notes);
    assert_eq!(back.doc.dpi, None);
}

/// #9: a 16-bit file from the crate's builder, every structure we map.
#[test]
fn import_builder_file_with_groups_masks_clipping_16bit() {
    let (w, h) = (128u32, 128u32);
    let mut b = PsdBuilder::new(w, h).depth(16);
    let mut hidden = LayerSpec::new("hidden", 0, 0, 2, 1, rgba16(2, 1, |x, _| if x == 0 { [65535, 0, 0, 65535] } else { [32768, 0, 0, 65535] }));
    hidden.visible = false;
    b.push_layer(hidden);

    b.begin_group(GroupSpec { name: "open".into(), blend_mode: BlendMode::Normal, opacity: 255, visible: true, open: true });
    let mut base = LayerSpec::new("base", 8, 8, 8, 8, rgba16(8, 8, |_, _| [0, 65535, 0, 65535]));
    base.mask = Some(MaskSpec { rect: Rect::from_xywh(10, 10, 4, 4), data: vec![255; 16], default_color: 0, disabled: false });
    b.push_layer(base);
    let mut clipped = LayerSpec::new("clipped", 8, 8, 4, 4, rgba16(4, 4, |_, _| [0, 0, 65535, 65535]));
    clipped.clipping = true;
    clipped.fill_opacity = Some(128);
    b.push_layer(clipped);
    b.end_group().unwrap();

    b.begin_group(GroupSpec { name: "closed".into(), blend_mode: BlendMode::Multiply, opacity: 128, visible: true, open: false });
    let mut m255 = LayerSpec::new("masked255", 64, 64, 16, 16, rgba16(16, 16, |_, _| [65535, 65535, 65535, 65535]));
    m255.mask = Some(MaskSpec { rect: Rect::from_xywh(70, 70, 4, 4), data: vec![0; 16], default_color: 255, disabled: true });
    b.push_layer(m255);
    b.end_group().unwrap();

    b.begin_group(GroupSpec::new("through"));
    let mut diss = LayerSpec::new("dissolve", 0, 100, 2, 2, rgba16(2, 2, |_, _| [65535, 0, 65535, 65535]));
    diss.blend_mode = BlendMode::Dissolve;
    b.push_layer(diss);
    b.end_group().unwrap();

    let imp = load_from_bytes(&b.to_bytes().unwrap()).unwrap();
    let d = &imp.doc;
    let got: Vec<(&str, u8, bool)> = d.layers.iter().map(|l| (l.name.as_str(), l.depth, l.folder)).collect();
    assert_eq!(
        got,
        vec![
            ("hidden", 0, false),
            ("base", 1, false),
            ("clipped", 1, false),
            ("open", 0, true),
            ("masked255", 1, false),
            ("closed", 0, true),
            ("dissolve", 1, false),
            ("through", 0, true),
        ]
    );
    let [hid, base, clipped, open, m255, closed, diss, through] = &d.layers[..] else { unreachable!() };
    assert!(!hid.visible && base.visible);
    assert!(clipped.clip && !base.clip);
    assert!((clipped.opacity - 128.0 / 255.0).abs() < 1e-6, "fill opacity folds into opacity");
    assert!(open.open && !open.through && open.blend == Blend::Normal);
    assert!(!closed.open && !closed.through && closed.blend == Blend::Multiply);
    assert!((closed.opacity - 128.0 / 255.0).abs() < 1e-6);
    assert!(through.through && through.blend == Blend::Normal);
    assert_eq!(diss.blend, Blend::Normal);

    // 16-bit samples: 65535 is exactly full; 32768 is half, but only to one
    // 8-bit step, because the crate reduces to 8-bit first (VENDOR.md).
    let p0 = hid.tile(TileIdx::new(0, 0)).unwrap().pixel(0, 0);
    assert_eq!(p0, [32768, 0, 0, 32768]);
    let p1 = hid.tile(TileIdx::new(0, 0)).unwrap().pixel(1, 0);
    assert!((i32::from(p1[0]) - 16384).abs() <= 129, "16-bit 32768 -> {} (one 8-bit step is 128.5)", p1[0]);
    assert_eq!(p1[3], 32768);

    // default 0: hidden outside its rectangle, so every canvas tile is held.
    let mb = base.mask.as_ref().unwrap();
    assert!(mb.enabled);
    assert_eq!(mb.tiles.len(), 4, "128x128 canvas = 2x2 tiles");
    let cov = mask8((w, h), mb);
    assert_eq!(cov[(11 * w + 11) as usize], 255);
    assert_eq!(cov[(9 * w + 9) as usize], 0);
    assert_eq!(cov[(100 * w + 100) as usize], 0);
    // default 255: tiles over its rectangle only; disabled rides across.
    let mm = m255.mask.as_ref().unwrap();
    assert!(!mm.enabled);
    assert_eq!(mm.tiles.keys().copied().collect::<Vec<_>>(), vec![TileIdx::new(1, 1)]);
    let cov = mask8((w, h), mm);
    assert_eq!(cov[(71 * w + 71) as usize], 0);
    assert_eq!(cov[(66 * w + 66) as usize], 255, "outside the rect inside the same tile");
    assert_eq!(cov[(10 * w + 10) as usize], 255);

    let dissolve_notes = imp.notes.iter().filter(|n| n.contains("Dissolve")).count();
    assert_eq!(dissolve_notes, 1, "{:?}", imp.notes);
}

/// #10: a file with no layer records opens as one "Background" layer.
#[test]
fn import_flat_file_becomes_background() {
    let (w, h) = (70u32, 3u32);
    let px = |x: u32, y: u32| -> [u8; 4] { if x == 69 { [0, 0, 0, 0] } else { [(x * 3) as u8, (y * 80) as u8, 7, 255] } };
    let mut b = PsdBuilder::new(w, h);
    b.composite(PixelData::Rgba8((0..h).flat_map(|y| (0..w).map(move |x| (x, y))).flat_map(|(x, y)| px(x, y)).collect()));
    let imp = load_from_bytes(&b.to_bytes().unwrap()).unwrap();
    assert_eq!(imp.doc.layers.len(), 1);
    let l = &imp.doc.layers[0];
    assert_eq!(l.name, "Background");
    let got = flat8((w, h), l);
    let want: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).flat_map(|(x, y)| px(x, y)).collect();
    assert!(got == want, "background pixels");
}

/// #11: text and effects layers arrive as pixels; one note per kind, names
/// untouched.
#[test]
fn import_notes_count_unsupported() {
    let mut b = PsdBuilder::new(8, 8);
    let mut text = LayerSpec::new("テスト", 0, 0, 1, 1, PixelData::Rgba8(vec![1, 2, 3, 255]));
    text.extra_blocks.push(TaggedBlock::new(*b"TySh", vec![0; 8]));
    b.push_layer(text);
    let mut fx = LayerSpec::new("glow", 0, 0, 1, 1, PixelData::Rgba8(vec![1, 2, 3, 255]));
    fx.extra_blocks.push(TaggedBlock::new(*b"lfx2", vec![0; 8]));
    b.push_layer(fx);
    let imp = load_from_bytes(&b.to_bytes().unwrap()).unwrap();
    assert_eq!(imp.notes.len(), 2, "{:?}", imp.notes);
    assert!(imp.notes.iter().any(|n| n.contains("text")));
    assert!(imp.notes.iter().any(|n| n.contains("effects")));
    let names: Vec<&str> = imp.doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["テスト", "glow"]);
}

/// #12: nesting past 250 folders is an error, never a silent drop.
#[test]
fn import_depth_cap_errors() {
    let nest = |n: usize| {
        let mut b = PsdBuilder::new(4, 4);
        for i in 0..n {
            b.begin_group(GroupSpec::new(format!("g{i}")));
        }
        b.push_layer(LayerSpec::new("deep", 0, 0, 1, 1, PixelData::Rgba8(vec![9, 9, 9, 255])));
        for _ in 0..n {
            b.end_group().unwrap();
        }
        b.to_bytes().unwrap()
    };
    let err = load_from_bytes(&nest(251)).err().expect("251 levels must fail");
    assert!(err.0.contains("250"), "{err}");
    // The limit itself still opens, every layer present.
    let ok = load_from_bytes(&nest(250)).unwrap();
    assert_eq!(ok.doc.layers.len(), 251);
    assert_eq!(ok.doc.layers[0].depth, 250);
}
