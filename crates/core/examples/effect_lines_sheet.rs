//! Renders every shipped effect-line sub tool to PNG so a critic can
//! judge the LOOK without launching the app (plan
//! `2026-09-06-effect-lines-parity`, A1.6).
//!
//! `cargo run -p mn-core --example effect_lines_sheet [-- <out-dir>]`
//! Default out-dir is `target/effect-lines/`. Writes, per preset:
//!
//! - `<preset>.png` — the whole 100 × 70 mm panel at 600 dpi, box-
//!   downscaled ×3 so a screen shows the block the way a printed page
//!   shows it (a 1:1 view of a 2362 px panel is a microscope, and every
//!   generated set looks fine under a microscope).
//! - `<preset>-crop.png` — a 600 × 600 px patch at 1:1. This is where
//!   the round caps, blunt ends and missing needles live; the downscale
//!   hides all three.
//!
//! plus `sheet.png`, every panel in a grid with its name burnt in.
//!
//! Deliberately an example rather than a `#[test]`, following
//! `gen_materials`: it WRITES files, which a test must never do. It
//! still asserts — a panel that inked nothing fails the run.
//!
//! Nothing here opens a window and nothing here is committed: the PNGs
//! are build output.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{GrayImage, Luma};
use mn_core::genlines::{GenLinesSpec, LineKind, LineOpts, builtin_presets};
use mn_core::tile::{TILE_SIZE, Tile, TileIdx};

/// The print resolution the presets are authored at (they are stated in
/// millimetres, so a dpi is what turns them into lines).
const DPI: u32 = 600;
/// The panel: 100 × 70 mm, a wide-ish middle-of-the-page frame.
const PANEL_MM: (f32, f32) = (100.0, 70.0);
/// How far the panel is shrunk for the overview PNG.
const SHRINK: u32 = 3;
/// The 1:1 patch side.
const CROP: u32 = 600;

fn mm(v: f32) -> f32 {
    v / 25.4 * DPI as f32
}

fn main() {
    let out = match std::env::args().nth(1) {
        Some(a) => PathBuf::from(a),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/effect-lines"),
    };
    std::fs::create_dir_all(&out).expect("create the out dir");

    let size = (mm(PANEL_MM.0) as u32, mm(PANEL_MM.1) as u32);
    let (w, h) = (size.0 as f32, size.1 as f32);
    let bounds = [0.0, 0.0, w, h];
    let at = |fx: f32, fy: f32| [w * fx, h * fy];

    let mut panels: Vec<(String, GrayImage)> = Vec::new();

    for (i, p) in builtin_presets().iter().enumerate() {
        let opts = (p.opts)(DPI);
        // A fixed drag per kind, so every row is judged on the same
        // gesture and two runs of this example are the same PNGs.
        let (a, b) = if p.name == "Drip lines" {
            // The drips hang off the panel's top EDGE — y = 0, not 2 %
            // down: in ref-09 every line touches the frame, and starting
            // the drag inside the panel left a white strip along the top
            // that read as "the set floats" (critic, round 1).
            (at(0.50, 0.0), at(0.50, 0.40))
        } else if p.kind.radial() {
            let c = at(0.55, 0.45);
            (c, [c[0] + w * 0.22, c[1]])
        } else {
            (at(0.15, 0.55), at(0.85, 0.45))
        };
        let spec = opts.place(p.kind, a, b, bounds, 1_000 + i as u64 * 7);
        write_panel(&out, p.name, &spec, size, &mut panels);
    }

    // The two off-panel centres, both on the Saturated line preset —
    // ref-11's left panel (a fan rising from below the frame) and
    // ref-10's right (a burst from beyond the top-right corner). A
    // centre outside the panel is the case a full circle plus clipping
    // gets wrong, so it gets its own picture.
    let sat = LineOpts::focus(DPI);
    let below = at(0.50, 1.20);
    write_panel(
        &out,
        "Saturated line - centre below",
        &LineOpts {
            // 170° of arc, aimed straight up into the panel.
            sweep_deg: 170.0,
            ..sat
        }
        .place(
            LineKind::Focus,
            below,
            [below[0], below[1] - w * 0.22],
            bounds,
            2_001,
        ),
        size,
        &mut panels,
    );
    let corner = at(1.10, -0.10);
    write_panel(
        &out,
        "Saturated line - centre off corner",
        &sat.place(
            LineKind::Focus,
            corner,
            [corner[0] - w * 0.16, corner[1] + h * 0.16],
            bounds,
            2_002,
        ),
        size,
        &mut panels,
    );

    write_sheet(&out.join("sheet.png"), &panels);
    println!(
        "[lines] {} panels + {} crops + sheet.png -> {}",
        panels.len(),
        panels.len(),
        out.display()
    );
}

/// `Stream line` -> `stream-line`. The critic reads file names.
fn slug(name: &str) -> String {
    name.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Render one spec and write its overview PNG and its 1:1 crop.
fn write_panel(
    dir: &Path,
    name: &str,
    spec: &GenLinesSpec,
    size: (u32, u32),
    panels: &mut Vec<(String, GrayImage)>,
) {
    let full = raster(spec, size);
    let ink = full.pixels().filter(|p| p.0[0] < 128).count();
    assert!(ink > 0, "{name} inked nothing");

    let small = box_shrink(&full, SHRINK);
    let stem = slug(name);
    save(&small, &dir.join(format!("{stem}.png")));

    // The 1:1 patch, right of centre — where a radial set's rays have
    // spread far enough to read one at a time and a stream block is
    // still full of ends.
    let cx = ((size.0 as f32 * 0.62) as u32).min(size.0.saturating_sub(CROP));
    let cy = (size.1.saturating_sub(CROP)) / 2;
    let crop = image::imageops::crop_imm(&full, cx, cy, CROP, CROP).to_image();
    save(&crop, &dir.join(format!("{stem}-crop.png")));

    panels.push((name.to_string(), small));
}

/// Average every `n × n` block into one pixel — the honest reading of a
/// 1-bit page at a third of print scale: an output pixel's grey IS the
/// fraction of the block that carries ink.
///
/// `image`'s Triangle resample also produces greys, but its kernel
/// reaches ±3 source pixels at this ratio, so a single 1 px hairline
/// comes out as two soft half-tones rather than one honest 33 % grey —
/// the "hairline shows as dots" reading the critic took for a renderer
/// bug. A box filter is also what a printer's screen does.
fn box_shrink(src: &GrayImage, n: u32) -> GrayImage {
    let (w, h) = (src.width() / n, src.height() / n);
    let mut out = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0u32;
            for dy in 0..n {
                for dx in 0..n {
                    sum += src.get_pixel(x * n + dx, y * n + dy).0[0] as u32;
                }
            }
            out.put_pixel(x, y, Luma([(sum / (n * n)) as u8]));
        }
    }
    out
}

/// Spec -> paper-white / ink-black greyscale, the way the Materials
/// thumbnails do it: the generators write COVERAGE in alpha, the colour
/// channels are the layer's business.
fn raster(spec: &GenLinesSpec, size: (u32, u32)) -> GrayImage {
    let tiles: HashMap<TileIdx, Arc<Tile>> = spec.render(size);
    let mut img = GrayImage::from_pixel(size.0, size.1, Luma([255]));
    for (idx, t) in &tiles {
        let (ox, oy) = idx.origin();
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let (gx, gy) = (ox + x as i32, oy + y as i32);
                if gx < 0 || gy < 0 || gx >= size.0 as i32 || gy >= size.1 as i32 {
                    continue;
                }
                if t.pixel(x, y)[3] > 0 {
                    img.put_pixel(gx as u32, gy as u32, Luma([0]));
                }
            }
        }
    }
    img
}

/// All the panels in one grid, three across, each with its name burnt
/// in above it — the file the gauntlet critic actually opens.
fn write_sheet(path: &Path, panels: &[(String, GrayImage)]) {
    const COLS: u32 = 3;
    const PAD: u32 = 8;
    const LABEL: u32 = 30;
    let (cw, ch) = panels
        .first()
        .map(|(_, i)| i.dimensions())
        .unwrap_or((1, 1));
    let rows = panels.len().div_ceil(COLS as usize) as u32;
    let sw = COLS * (cw + PAD) + PAD;
    let sh = rows * (ch + LABEL + PAD) + PAD;
    let mut sheet = GrayImage::from_pixel(sw, sh, Luma([255]));
    for (i, (name, img)) in panels.iter().enumerate() {
        let cx = PAD + (i as u32 % COLS) * (cw + PAD);
        let cy = PAD + (i as u32 / COLS) * (ch + LABEL + PAD);
        text(&mut sheet, cx, cy + 4, &name.to_ascii_uppercase(), 4);
        image::imageops::replace(&mut sheet, img, cx as i64, (cy + LABEL) as i64);
    }
    save(&sheet, path);
}

/// A 3 × 5 bitmap font, uppercase only. Small on purpose: the sheet
/// needs labels, not typography, and pulling a font crate into `mn-core`
/// for twelve captions is the wrong trade.
fn glyph(c: char) -> [u8; 5] {
    match c {
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b010],
        'K' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b101, 0b111, 0b111, 0b111, 0b101],
        'O' => [0b010, 0b101, 0b101, 0b101, 0b010],
        'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'Q' => [0b010, 0b101, 0b101, 0b111, 0b011],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b011, 0b100, 0b010, 0b001, 0b110],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b011],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b110, 0b001, 0b010, 0b100, 0b111],
        '3' => [0b110, 0b001, 0b010, 0b001, 0b110],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b110, 0b001, 0b110],
        '6' => [0b011, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b110],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        _ => [0; 5],
    }
}

fn text(img: &mut GrayImage, x: u32, y: u32, s: &str, scale: u32) {
    let mut pen = x;
    for c in s.chars() {
        let g = glyph(c);
        for (ry, row) in g.iter().enumerate() {
            for rx in 0..3u32 {
                if row & (0b100 >> rx) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = (pen + rx * scale + dx, y + ry as u32 * scale + dy);
                        if px < img.width() && py < img.height() {
                            img.put_pixel(px, py, Luma([0]));
                        }
                    }
                }
            }
        }
        pen += 4 * scale;
    }
}

/// Maximum compression, same reason as `gen_materials`: these are big
/// high-frequency bitmaps and the default costs ~30 % more bytes.
fn save(img: &GrayImage, path: &Path) {
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
