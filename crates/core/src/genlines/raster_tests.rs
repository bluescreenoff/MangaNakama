//! Perf lane 2 (2026-11-06): the rasterizer's sink changed from a
//! `HashMap<TileIdx, Tile>` looked up per inked pixel to a flat per-tile
//! bit grid that is turned into tiles once. The output must not move by a
//! single pixel, so every shipped preset is pinned here at numbers taken
//! from the HashMap code BEFORE the change, on a page that is not a
//! multiple of the tile size (the right and bottom tiles are partial, which
//! is where a clip change would show first) and with two gestures: one over
//! the whole page and one in an off-centre panel that runs off the page
//! edge.

use super::presets::builtin_presets;
use super::tests::fingerprint;

/// Small enough for a debug build, odd-sized on both axes on purpose.
const PAGE: (u32, u32) = (1000, 1403);

fn placed(i: usize, bounds: [f32; 4], dpi: u32) -> super::GenLinesSpec {
    let p = &builtin_presets()[i];
    let a = [
        (bounds[0] + bounds[2]) * 0.5,
        (bounds[1] + bounds[3]) * 0.5,
    ];
    let b = [a[0] + (bounds[2] - bounds[0]) / 6.0, a[1]];
    (p.opts)(dpi).place(p.kind, a, b, bounds, 0x51ED_5EED ^ i as u64)
}

/// `(preset name, full-page fingerprint, panel fingerprint)`, recorded from
/// the HashMap rasterizer (commit ba8201d) before the bit grid existed.
const GOLDEN: &[(&str, (usize, u64, u64), (usize, u64, u64))] = &[
    ("Stream line", (352, 194269, 9962289581590825978), (347, 144724, 13727875659090064715)),
    ("Dense stream", (352, 317648, 4201259618168586857), (352, 250945, 7884577936488149778)),
    ("Sparse stream", (352, 126377, 9634185979792630470), (325, 81584, 7598363907833373581)),
    ("Perspective stream", (340, 260887, 9514759352765576921), (330, 143715, 10801168896869668034)),
    ("Drip lines", (217, 102926, 15000923455441219491), (88, 49205, 17153334462064085045)),
    ("Saturated line", (351, 65370, 15281811041324296292), (106, 33260, 8019360910658854262)),
    ("Dense saturated line", (352, 90907, 6790143852227144441), (107, 44612, 11668203510389351645)),
    ("Dark burst", (352, 202848, 11496014516355956454), (108, 95929, 8292603950596650183)),
    ("Sea urchin flash", (34, 39584, 6879059035371027369), (14, 16728, 9593920868478306497)),
    ("Solid flash", (347, 1307999, 2600308566912565973), (150, 513579, 13489884220063221415)),
];

#[test]
fn every_preset_is_pixel_identical_to_the_hashmap_rasterizer() {
    let (w, h) = (PAGE.0 as f32, PAGE.1 as f32);
    let mut got = Vec::new();
    for (i, p) in builtin_presets().iter().enumerate() {
        let full = fingerprint(&placed(i, [0.0, 0.0, w, h], 150).render(PAGE));
        // A panel in the lower right whose box runs past the page edge.
        let panel = fingerprint(&placed(i, [w * 0.55, h * 0.6, w * 1.1, h * 1.05], 150).render(PAGE));
        got.push((p.name, full, panel));
    }
    for g in &got {
        println!("    (\"{}\", {:?}, {:?}),", g.0, g.1, g.2);
    }
    assert_eq!(got.len(), GOLDEN.len(), "one golden row per shipped preset");
    for (g, want) in got.iter().zip(GOLDEN) {
        assert_eq!(g, want, "{} moved — the rasterizer no longer draws the same pixels", g.0);
    }
}

/// The lag hunt's effect-line row, at the owner's page — B4 600 dpi — and
/// in this crate so it builds without the app. Prints ms and the
/// fingerprint, so a before/after run proves the full-size output too.
///
///     RUST_TEST_THREADS=1 cargo test -p mn-core --release -- --ignored genlines_b4_timing --nocapture
#[test]
#[ignore = "timing measurement — release only, B4/600 page"]
fn genlines_b4_timing() {
    const B4: (u32, u32) = (6070, 8598);
    let (w, h) = (B4.0 as f32, B4.1 as f32);
    println!("\n| preset | full page ms | panel ms | full fingerprint |");
    println!("|---|---|---|---|");
    for (i, p) in builtin_presets().iter().enumerate() {
        let full = placed(i, [0.0, 0.0, w, h], 600);
        let t = std::time::Instant::now();
        let map = full.render(B4);
        let full_ms = t.elapsed().as_secs_f32() * 1000.0;
        let panel = placed(i, [0.0, 0.0, w * 0.5, h * 0.5], 600);
        let t = std::time::Instant::now();
        let _ = panel.render(B4);
        let panel_ms = t.elapsed().as_secs_f32() * 1000.0;
        println!(
            "| {} | {:.0} | {:.0} | {:?} |",
            p.name,
            full_ms,
            panel_ms,
            fingerprint(&map)
        );
    }
    // The floor under every row above: turning a full page of bit masks
    // into real tiles, which each preset pays once for the tiles it inks.
    let mut mask = super::InkMask::new(B4);
    for y in (0..B4.1 as i32).step_by(64) {
        for x in (0..B4.0 as i32).step_by(64) {
            super::InkSink::put(&mut mask, x, y);
        }
    }
    let t = std::time::Instant::now();
    let tiles = mask.into_tiles().len();
    println!("| (tile once: {tiles} tiles) | {:.0} | | |", t.elapsed().as_secs_f32() * 1000.0);
}

/// The inked pixels of a map sink, sorted, for comparing two scans.
fn inked(m: &std::collections::HashMap<crate::tile::TileIdx, crate::tile::Tile>) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    for (k, t) in m {
        let (ox, oy) = k.origin();
        for y in 0..crate::tile::TILE_SIZE {
            for x in 0..crate::tile::TILE_SIZE {
                if t.pixel(x, y)[3] > 0 {
                    out.push((ox + x as i32, oy + y as i32));
                }
            }
        }
    }
    out.sort_unstable();
    out
}

/// The row scan [`super::segment`] is held to the chunk scan it replaced,
/// pixel for pixel, on strokes chosen to be awkward: any angle (and the
/// exact axes), hairlines and fat ones, every profile knob, ends off the
/// page on every side, and a page that is not a whole number of tiles.
#[test]
fn row_scan_matches_the_chunk_scan() {
    let size = (300u32, 257u32);
    let mut seed = 0x5EED_0F_11E5u64;
    let mut r = |lo: f32, hi: f32| lo + super::rand(&mut seed) * (hi - lo);
    let mut checked = 0;
    for i in 0..400 {
        let a = [r(-80.0, 380.0), r(-80.0, 340.0)];
        let mut b = [r(-80.0, 380.0), r(-80.0, 340.0)];
        // Every tenth stroke exactly horizontal, vertical or very short.
        match i % 10 {
            0 => b[1] = a[1],
            1 => b[0] = a[0],
            2 => b = [a[0] + r(-3.0, 3.0), a[1] + r(-3.0, 3.0)],
            _ => {}
        }
        let hw = if i % 3 == 0 { r(0.0, 1.0) } else { r(0.5, 24.0) };
        let prof = super::Profile {
            taper: if i % 4 == 0 { 0.0 } else { r(0.0, 1.3) },
            entry: if i % 5 < 2 { 0.0 } else { r(0.0, 0.6) },
            needle: if i % 2 == 0 { 0.0 } else { r(0.2, 2.5) },
        };
        let mut rows = std::collections::HashMap::new();
        let mut chunks = std::collections::HashMap::new();
        super::segment(&mut rows, a, b, hw, prof, size);
        super::segment_chunks(&mut chunks, a, b, hw, prof, size);
        let (x, y) = (inked(&rows), inked(&chunks));
        assert_eq!(x, y, "stroke {i}: {a:?} → {b:?}, hw {hw}, {prof:?}");
        checked += usize::from(!y.is_empty());
    }
    assert!(checked > 200, "most strokes must reach the page ({checked})");
}

/// The solid kind's own corners, which the one shipped preset does not
/// reach: no field (so no despeckle), a swept arc, a centre near a page
/// corner (the flood box clipped by the page), the legacy width path, an
/// inverted tooth, bundles with a ragged core. Pinned at fingerprints taken
/// from the per-pixel scan (commit 5893c17) before the row scan existed.
fn solid_variants() -> Vec<(&'static str, super::UrchinParams)> {
    use super::{Mix, UrchinParams};
    let base = UrchinParams {
        center: [430.0, 610.0],
        r_in: 90.0,
        r_out: 260.0,
        count: 140,
        width: 6.0,
        width_frac: 0.55,
        angle_jitter: 0.4,
        length_jitter: 0.5,
        solid: true,
        seed: 11,
        ..Default::default()
    };
    vec![
        ("no field", base.clone()),
        ("field 4", UrchinParams { field: 4.0, ..base.clone() }),
        (
            "swept, field 3",
            UrchinParams {
                field: 3.0,
                sweep_deg: 140.0,
                sweep_center_deg: 30.0,
                ..base.clone()
            },
        ),
        (
            "corner, bundles, ragged core",
            UrchinParams {
                center: [60.0, 1350.0],
                field: 1.5,
                group: 5,
                group_gap: 2.8,
                group_jit: 0.6,
                core_jit: 0.4,
                jit_len_out: 0.7,
                mix: Mix {
                    needle: 0.6,
                    entry: 0.1,
                    ..Default::default()
                },
                ..base.clone()
            },
        ),
        (
            "legacy width",
            UrchinParams {
                width_frac: 0.0,
                width: 9.0,
                ..base.clone()
            },
        ),
        (
            "tip out, field 2",
            UrchinParams {
                tip_out: true,
                field: 2.0,
                ..base.clone()
            },
        ),
        (
            "off page",
            UrchinParams {
                center: [-150.0, 500.0],
                field: 4.0,
                ..base
            },
        ),
    ]
}

const GOLDEN_SOLID: &[(&str, (usize, u64, u64))] = &[
    ("no field", (67, 163202, 15314340380099935216)),
    ("field 4", (350, 1356230, 9624883242645359390)),
    ("swept, field 3", (170, 573202, 10374741291137620039)),
    ("corner, bundles, ragged core", (46, 143016, 7603092316824107955)),
    ("legacy width", (67, 93648, 13160660166343323177)),
    ("tip out, field 2", (223, 765616, 16885660788358395811)),
    ("off page", (282, 1088038, 9395007924750315913)),
];

#[test]
fn solid_flash_corners_are_pixel_identical_to_the_pixel_scan() {
    let mut got = Vec::new();
    for (name, p) in solid_variants() {
        got.push((name, fingerprint(&super::render_urchin(&p, PAGE))));
    }
    for g in &got {
        println!("    (\"{}\", {:?}),", g.0, g.1);
    }
    assert_eq!(got.len(), GOLDEN_SOLID.len(), "one golden row per variant");
    for (g, want) in got.iter().zip(GOLDEN_SOLID) {
        assert_eq!(g, want, "{} moved", g.0);
    }
}
