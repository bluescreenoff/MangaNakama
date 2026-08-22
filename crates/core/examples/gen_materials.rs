//! Regenerates the SHIPPED default materials from our own engines
//! (plan M7, `20-MEDIUMS.md`): the starter tone sheets come out of
//! `mn_core::tone`, the effect-line thumbnails out of
//! `mn_core::genlines`. Nothing here is drawn by hand and nothing is
//! copied from anywhere — that is the licensing rule the shipped bank
//! lives under (`DECISIONS 8.5`): our defaults must be OURS, and
//! generating them makes them correct by construction as a bonus.
//!
//! `cargo run -p mn-core --example gen_materials [-- <out-dir>]`
//! Default out-dir is the repo's `assets/materials`. Writes
//! `<out-dir>/tones/*.png` (+ its `tags.txt`) and refreshes the
//! `<stem>.png` thumbnail beside every `<stem>.gen.json` it finds.
//!
//! Deliberately an example rather than a `#[test]`, following
//! `mn-gpu`'s `offscreen` example: it WRITES files into the working
//! tree, which a test must never do. It still asserts — every sheet is
//! reopened from disk and its ink coverage checked against the density
//! it was asked for, so a broken screen fails the run instead of
//! shipping a wrong-density tone.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{GrayImage, Luma};
use mn_core::genlines::GenLinesSpec;
use mn_core::tile::{FIX15_ONE, TILE_SIZE, Tile, TileIdx};
use mn_core::tone::{ToneDensity, ToneParams, TonePattern, rasterize_tile};

/// Print resolution the sheets are authored at. A screentone is an
/// inch-relative thing (LPI), so a sheet without a dpi is meaningless —
/// 600 is the project's mono print default.
const DPI: u32 = 600;
/// Nominal sheet side. The real side is nudged to a whole number of
/// screen periods (see [`tile_side`]), so tiling a sheet across a page
/// does not cut a dot in half at every seam.
const NOMINAL: u32 = 1024;
/// Classic manga screen angle for both the dot and the line sheets.
const ANGLE: f32 = 45.0;

fn main() {
    let out = match std::env::args().nth(1) {
        Some(a) => PathBuf::from(a),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/materials"),
    };
    let tones = out.join("tones");
    std::fs::create_dir_all(&tones).expect("create the tones folder");

    let mut tags = String::new();
    let mut written = 0usize;

    // --- dot screens: the manga range, four grades each ----------------
    for lpi in [27.5f32, 42.5, 60.0] {
        for pct in [10u32, 20, 30, 50] {
            let p = ToneParams {
                pattern: TonePattern::Dots,
                lpi,
                angle_deg: ANGLE,
                density: ToneDensity::Specified(pct as f32 / 100.0),
                ..Default::default()
            };
            let name = format!("tone-dot-{}lpi-{pct}.png", trim(lpi));
            let side = tile_side(lpi);
            write_sheet(&tones, &name, &p, side, Ramp::Flat);
            check(&tones.join(&name), side, pct as f32 / 100.0);
            tags += &format!(
                "{name}=screentone, tone, dots, halftone, {pct}%, {} lpi, 45\n",
                trim(lpi)
            );
            written += 1;
        }
    }

    // --- line screens ---------------------------------------------------
    for lpi in [30.0f32, 60.0] {
        let pct = 50u32;
        let p = ToneParams {
            pattern: TonePattern::Lines,
            lpi,
            angle_deg: ANGLE,
            density: ToneDensity::Specified(pct as f32 / 100.0),
            ..Default::default()
        };
        let name = format!("tone-line-{}lpi-{pct}.png", trim(lpi));
        let side = tile_side(lpi);
        write_sheet(&tones, &name, &p, side, Ramp::Flat);
        check(&tones.join(&name), side, pct as f32 / 100.0);
        tags += &format!(
            "{name}=screentone, tone, lines, line screen, hatching, {pct}%, {} lpi, 45\n",
            trim(lpi)
        );
        written += 1;
    }

    // --- one noise (FM) sheet -------------------------------------------
    // No lattice, so no period to respect: the nominal side is exact.
    // 30 LPI rather than 60: the grain is a QUARTER of the cell, so 60
    // LPI at 600 dpi is 2.5 px of dust that prints as flat grey. 5 px
    // reads as the sand tone this sheet exists to be.
    {
        let p = ToneParams {
            pattern: TonePattern::Noise,
            lpi: 30.0,
            angle_deg: 0.0,
            density: ToneDensity::Specified(0.3),
            ..Default::default()
        };
        let name = "tone-noise-30lpi-30.png".to_string();
        write_sheet(&tones, &name, &p, NOMINAL, Ramp::Flat);
        check(&tones.join(&name), NOMINAL, 0.3);
        tags += &format!("{name}=screentone, tone, noise, grain, sand, random, FM, 30%\n");
        written += 1;
    }

    // --- one graded sheet ------------------------------------------------
    // The gradient tone every 効果 page wants: dot size follows a
    // left-to-right ramp, so the sheet fades from paper to solid. Not a
    // tiling sheet (a ramp has no period) — the side is the 60 LPI one
    // anyway so it sits beside its flat siblings at the same scale.
    {
        let p = ToneParams {
            pattern: TonePattern::Dots,
            lpi: 60.0,
            angle_deg: ANGLE,
            // The ramp IS the art here, so the dot follows the source.
            density: ToneDensity::ImageColour,
            ..Default::default()
        };
        let name = "tone-dot-60lpi-gradient.png".to_string();
        let side = tile_side(60.0);
        write_sheet(&tones, &name, &p, side, Ramp::Linear);
        tags += &format!("{name}=screentone, tone, gradient, graded, fade, ramp, dots, 60 lpi\n");
        written += 1;
    }

    std::fs::write(tones.join("tags.txt"), &tags).expect("write tags.txt");
    println!("[gen] {written} tone sheets -> {}", tones.display());

    // --- generator-material thumbnails -----------------------------------
    // The `.gen.json` IS the material; its same-stem PNG is only the
    // palette picture. Rendering that picture from the spec is the only
    // way it cannot lie about what a click will place — the pre-M7
    // thumbnails were hand-drawn and showed neither spec.
    let mut thumbs = 0usize;
    for spec_path in gen_specs(&out) {
        let text = std::fs::read_to_string(&spec_path).expect("read the spec");
        let spec: GenLinesSpec = serde_json::from_str(&text).expect("parse the spec");
        let png = spec_path.with_file_name(format!(
            "{}.png",
            spec_path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".gen.json"))
                .expect("a .gen.json name")
        ));
        write_thumb(&png, &spec);
        println!("[gen] thumbnail {}", png.display());
        thumbs += 1;
    }
    println!("[gen] {thumbs} generator thumbnails refreshed");
}

/// `60.0` -> `60`, `27.5` -> `27.5` — file names read like the tone
/// counter's labels, not like floats.
fn trim(lpi: f32) -> String {
    if (lpi - lpi.round()).abs() < 1e-3 {
        format!("{}", lpi.round() as i32)
    } else {
        format!("{lpi}")
    }
}

/// What drives each column's ink.
#[derive(Clone, Copy)]
enum Ramp {
    /// Opaque source everywhere; the density comes from the params.
    Flat,
    /// Left-to-right 0..1 source ink (the graded sheet).
    Linear,
}

/// Sheet side in px that is a whole number of screen periods.
///
/// A 45° square lattice of cell `C` repeats every `C·√2` along x and y
/// (rotating by 45° maps the axis period onto the diagonal). Cropping a
/// sheet anywhere else leaves a half dot at the seam, which is exactly
/// what you see when a tiled tone is pasted across a panel. The residual
/// error is sub-pixel: the period is irrational, so the best integer side
/// is off by at most half a pixel over the whole sheet.
fn tile_side(lpi: f32) -> u32 {
    let period = (DPI as f32 / lpi) * std::f32::consts::SQRT_2;
    let n = (NOMINAL as f32 / period).round().max(1.0);
    (n * period).round() as u32
}

/// Rasterize one tone sheet through the real screen and write it as an
/// 8-bit greyscale PNG (paper white, ink black — what a material bank
/// and every print path expect).
fn write_sheet(dir: &Path, name: &str, p: &ToneParams, side: u32, ramp: Ramp) {
    let mut img = GrayImage::from_pixel(side, side, Luma([255]));
    let mut src = Tile::new_transparent();
    for ty in (0..side).step_by(TILE_SIZE) {
        for tx in (0..side).step_by(TILE_SIZE) {
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let a = match ramp {
                        Ramp::Flat => FIX15_ONE as f32,
                        Ramp::Linear => {
                            (tx + x as u32).min(side - 1) as f32 / (side - 1) as f32
                                * FIX15_ONE as f32
                        }
                    };
                    src.set_pixel(x, y, [0, 0, 0, a as u16]);
                }
            }
            let out = rasterize_tile(&src, (tx as i32, ty as i32), p, DPI);
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let (gx, gy) = (tx + x as u32, ty + y as u32);
                    if gx >= side || gy >= side {
                        continue;
                    }
                    let a = out.pixel(x, y)[3] as u32;
                    img.put_pixel(gx, gy, Luma([(255 - a * 255 / FIX15_ONE) as u8]));
                }
            }
        }
    }
    save_png(&img, &dir.join(name));
}

/// Write a greyscale PNG at maximum compression. `GrayImage::save` uses
/// the encoder's default, and these sheets are 1 MP of high-frequency
/// screen — the default costs ~30 % more bytes in a repo whose whole
/// `assets/` tree is smaller than one careless tone folder.
fn save_png(img: &GrayImage, path: &Path) {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::{ExtendedColorType, ImageEncoder};
    let f = std::fs::File::create(path).expect("create the PNG");
    PngEncoder::new_with_quality(
        std::io::BufWriter::new(f),
        CompressionType::Best,
        FilterType::Adaptive,
    )
    .write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        ExtendedColorType::L8,
    )
    .expect("encode the PNG");
}

/// Reopen a written sheet and assert it is the tone it claims to be:
/// right size, and mean ink within a few percent of the requested
/// density. The screen is area-exact by construction (`tone.rs`), so a
/// miss here means the sheet on disk is not what the engine renders.
fn check(path: &Path, side: u32, density: f32) {
    let img = image::open(path).expect("the sheet reopens").to_luma8();
    assert_eq!(img.dimensions(), (side, side), "{}", path.display());
    let ink: f64 = img.pixels().map(|p| 1.0 - p.0[0] as f64 / 255.0).sum();
    let mean = ink / (side as f64 * side as f64);
    assert!(
        (mean - density as f64).abs() < 0.04,
        "{} asked for {density:.2} ink, carries {mean:.3}",
        path.display()
    );
}

/// Every `*.gen.json` under `dir`, recursively (the bank scan walks
/// subfolders, so this must too).
fn gen_specs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(gen_specs(&p));
        } else if p.file_name().is_some_and(|n| {
            n.to_str()
                .is_some_and(|n| n.to_ascii_lowercase().ends_with(".gen.json"))
        }) {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Render a generator spec's palette thumbnail.
///
/// The canvas is sized from the spec's own reach so the whole figure is
/// in frame, then box-downsampled to thumbnail size — which is also
/// where the anti-aliasing comes from, since the generators ink hard
/// edges on purpose.
fn write_thumb(png: &Path, spec: &GenLinesSpec) {
    // Focus lines and both flashes are RADIAL: (a, b) is a centre and
    // (c, d) a radius pair. Speed lines are the one strip layout.
    let radial = spec.focus || spec.kind == 1 || spec.kind == 2;
    let (size, thumb, mut spec) = if radial {
        let r = spec.d.max(spec.c + 1.0);
        let side = (r * 2.0).round() as u32;
        ((side, side), (512u32, 512u32), *spec)
    } else {
        // Speed lines scatter across whatever canvas they are given; a
        // 2:1 strip reads as motion, a square reads as noise.
        let w = (spec.c.max(spec.b) * 1.4).round() as u32;
        ((w, w / 2), (512u32, 256u32), *spec)
    };
    if radial {
        // The stored centre is overwritten by the click on placement
        // (`PasteMaterial`), so the thumbnail shows the centred case.
        spec.a = size.0 as f32 * 0.5;
        spec.b = size.1 as f32 * 0.5;
    }
    let tiles: HashMap<TileIdx, Arc<Tile>> = spec.render(size);
    let mut full = GrayImage::from_pixel(size.0, size.1, Luma([255]));
    for (idx, t) in &tiles {
        let (ox, oy) = idx.origin();
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let (gx, gy) = (ox + x as i32, oy + y as i32);
                if gx < 0 || gy < 0 || gx >= size.0 as i32 || gy >= size.1 as i32 {
                    continue;
                }
                // The generators write COVERAGE in alpha (the colour
                // channels are the layer's business); ink where alpha is.
                if t.pixel(x, y)[3] > 0 {
                    full.put_pixel(gx as u32, gy as u32, Luma([0]));
                }
            }
        }
    }
    let small = image::imageops::resize(
        &full,
        thumb.0,
        thumb.1,
        image::imageops::FilterType::Triangle,
    );
    save_png(&small, png);
}
