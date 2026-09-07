//! Renders every shipped effect-line sub tool to PNG so a critic can
//! judge the LOOK without launching the app (plan
//! `2026-09-06-effect-lines-parity`, A1.6).
//!
//! `cargo run -p mn-core --example effect_lines_sheet [-- <out-dir>]`
//! Default out-dir is `target/effect-lines/`. The panel list itself lives
//! in `examples/common/mod.rs`, shared with `effect_metrics` so the table
//! and the pictures can never describe different sets. Writes, per panel:
//!
//! - `<panel>.png` — the whole 100 × 70 mm panel at 600 dpi, box-
//!   downscaled ×3 so a screen shows the block the way a printed page
//!   shows it (a 1:1 view of a 2362 px panel is a microscope, and every
//!   generated set looks fine under a microscope).
//! - `<panel>-crop.png` — a 600 × 600 px patch at 1:1. This is where
//!   the round caps, blunt ends and missing needles live; the downscale
//!   hides all three.
//!
//! plus `sheet.png`, every panel in a grid with its name burnt in, and
//! `README.txt` — which file is 1:1 and which is shrunk, in writing,
//! because a critic that has to guess spends a third of its round on it
//! (lean-loop plan, "what was bloat last time").
//!
//! Deliberately an example rather than a `#[test]`, following
//! `gen_materials`: it WRITES files, which a test must never do. It
//! still asserts — a panel that inked nothing fails the run.
//!
//! Nothing here opens a window and nothing here is committed: the PNGs
//! are build output.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{GrayImage, Luma};
use mn_core::genlines::GenLinesSpec;
use mn_core::tile::{TILE_SIZE, Tile, TileIdx};

use common::{DPI, PANEL_MM, panel_size, panels, slug};

/// How far the panel is shrunk for the overview PNG.
const SHRINK: u32 = 3;
/// The 1:1 patch side.
const CROP: u32 = 600;

fn main() {
    let out = match std::env::args().nth(1) {
        Some(a) => PathBuf::from(a),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/effect-lines"),
    };
    std::fs::create_dir_all(&out).expect("create the out dir");

    // Tuning one preset should not cost a whole sheet: `MN_PANELS` is a
    // substring of the slug, and a filtered run leaves `sheet.png` and
    // `README.txt` alone rather than writing a partial one.
    let only = std::env::var("MN_PANELS").unwrap_or_default();
    let size = panel_size();
    let mut sheet_panels: Vec<(String, GrayImage)> = Vec::new();
    for (name, spec) in panels() {
        if !only.is_empty() && !slug(&name).contains(&only) {
            continue;
        }
        write_panel(&out, &name, &spec, size, &mut sheet_panels);
    }
    if !only.is_empty() {
        println!("[lines] {} filtered panels -> {}", sheet_panels.len(), out.display());
        return;
    }

    write_sheet(&out.join("sheet.png"), &sheet_panels);
    write_readme(&out.join("README.txt"), size, &sheet_panels);
    println!(
        "[lines] {} panels + {} crops + sheet.png + README.txt -> {}",
        sheet_panels.len(),
        sheet_panels.len(),
        out.display()
    );
}

/// What the critic reads first: which PNG is at print scale and which is
/// not. Round 1 of the last gauntlet spent a third of its list working
/// this out from the pixels, and got it wrong.
fn write_readme(path: &Path, size: (u32, u32), panels: &[(String, GrayImage)]) {
    let (sw, sh) = panels.first().map(|(_, i)| i.dimensions()).unwrap_or((0, 0));
    let mut s = String::new();
    s.push_str("MangaNakama - effect-line render sheet\n");
    s.push_str("=====================================\n\n");
    s.push_str(&format!(
        "Panel      : {} x {} mm at {DPI} dpi = {} x {} px.\n",
        PANEL_MM.0, PANEL_MM.1, size.0, size.1
    ));
    s.push_str(&format!(
        "             1 mm = {:.1} px, 1 px = {:.4} mm.\n\n",
        DPI as f32 / 25.4,
        25.4 / DPI as f32
    ));
    s.push_str(&format!(
        "<name>.png       SHRUNK x{SHRINK} (box filter, so a grey pixel IS the ink\n\
         \x20                fraction of its {SHRINK}x{SHRINK} block). {sw} x {sh} px.\n\
         \x20                This is roughly how the panel reads on a printed page.\n\n"
    ));
    s.push_str(&format!(
        "<name>-crop.png  1:1, NO resampling. A {CROP} x {CROP} px patch =\n\
         \x20                {:.1} x {:.1} mm of paper, vertically centred. Judge\n\
         \x20                tips, caps and stroke weights here; the shrunk PNG\n\
         \x20                hides all three.\n\
         \x20                Taken at x = 62 % of the panel width, EXCEPT for a\n\
         \x20                radial set that ends inside the frame (both flashes,\n\
         \x20                which are balloon-sized): there it straddles the band,\n\
         \x20                at the burst centre plus 0.6 of its reach. So on a\n\
         \x20                flash crop, LEFT is toward the hole and RIGHT is\n\
         \x20                outward, and no panel frame is in shot - an end that\n\
         \x20                stops dead in a crop is a real blunt end.\n\n",
        CROP as f32 * 25.4 / DPI as f32,
        CROP as f32 * 25.4 / DPI as f32
    ));
    s.push_str(
        "sheet.png        every shrunk panel in a 3-wide grid with its name burnt in.\n\n",
    );
    s.push_str("Ink is 1-bit black on white. There is no anti-aliasing anywhere in\n");
    s.push_str("this pipeline - the generators write full coverage or nothing - so a\n");
    s.push_str("grey pixel in a shrunk PNG is the box filter, never the renderer.\n\n");
    s.push_str("Panels, in sheet order:\n");
    for (name, _) in panels {
        s.push_str(&format!("  {:<36} {}.png\n", name, slug(name)));
    }
    s.push_str("\nNumbers for every panel: run\n");
    s.push_str("  cargo run --release -p mn-core --example effect_metrics\n");
    std::fs::write(path, s).expect("write README.txt");
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
    //
    // …unless the effect is a BALLOON that ends inside the panel, which
    // since Lane F both flashes are. 62 % of the panel width is a fixed
    // spot, and `sea-urchin-flash-tight` is 24 mm across — the crop
    // landed almost entirely on blank paper. For a radial set whose own
    // outer radius stops short of the frame, take the patch across its
    // band instead: the burst's centre plus 0.6 of its reach, which puts
    // the hole edge, the band and the outer fringe all in one 25 mm
    // square.
    let cx = {
        let by_frac = (size.0 as f32 * 0.62) as u32;
        let x = if spec.radial() && spec.a + spec.d < size.0 as f32 {
            (spec.a + spec.d * 0.6 - CROP as f32 * 0.5).max(0.0) as u32
        } else {
            by_frac
        };
        x.min(size.0.saturating_sub(CROP))
    };
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
