//! Sub Tool stroke previews: each `.myb` preset drawn as a real stroke, so
//! the list shows what a brush *does* — exactly what CSP's Sub Tool palette
//! and Rebelle's preset grid show. Generated lazily (one per frame, budgeted
//! by the caller) and cached for the session.

use std::path::Path;

use mn_brush::MyBrush;
use mn_core::{Document, PenSample, StrokeSink, TileIdx};

use super::theme;

/// Preview texture size in points (row thumbnails).
pub const PREVIEW_W: usize = 64;
pub const PREVIEW_H: usize = 18;

// Rendered 2x and box-downsampled, so thin strokes stay smooth.
const CW: u32 = (PREVIEW_W * 2) as u32;
const CH: u32 = (PREVIEW_H * 2) as u32;

/// Light ink on the panel's dark field.
const INK: [f32; 3] = [0.84, 0.84, 0.87];

/// Render one preset's preview, or `None` when the preset will not load.
pub fn generate(ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
    let img = generate_image(path)?;
    Some(ctx.load_texture(
        format!("mn.brush.preview.{}", path.display()),
        img,
        egui::TextureOptions::LINEAR,
    ))
}

/// The pure-CPU half: brush → tiny document → composited image.
pub fn generate_image(path: &Path) -> Option<egui::ColorImage> {
    let mut b = MyBrush::load(path).ok()?;
    // Normalize: whatever the preset's real size, the preview stroke should
    // read as an elegant line, not a bar. The multiplier is unclamped.
    let r = b.radius_px().clamp(0.1, 400.0);
    b.set_size_multiplier((CH as f32 * 0.15) / r);
    b.set_color_rgb(INK);

    let mut doc = Document::new(CW, CH);
    stroke(&mut b, &mut doc);

    // Erasers leave nothing on an empty layer: give them opaque ink to cut
    // through, then the gap *is* the preview (what Krita does).
    if max_alpha(&doc) < 2000 {
        fill_layer(&mut doc);
        stroke(&mut b, &mut doc);
    }

    Some(composite(&doc))
}

/// An S-curve with a pressure ramp — enough to show tip shape, taper and
/// texture in one gesture.
fn stroke(b: &mut MyBrush, doc: &mut Document) {
    const N: u32 = 96;
    doc.begin_op();
    b.begin(doc);
    for k in 0..=N {
        let t = k as f32 / N as f32;
        let x = CW as f32 * (0.06 + 0.88 * t);
        let y = CH as f32 * 0.5 + (t * std::f32::consts::TAU * 0.82).sin() * CH as f32 * 0.16;
        // Ease in and out so pressure-dynamic tips read. `sin(π)` lands a hair
        // *below* zero in f32 and a negative base makes `powf` return NaN —
        // clamp first (a NaN here once reached libmypaint as a NaN dab radius).
        let pressure = (t * std::f32::consts::PI).sin().max(0.0).powf(0.6);
        b.sample(
            doc,
            PenSample {
                x,
                y,
                pressure,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: (k as f64) * 3.5,
            },
        );
    }
    b.end(doc);
    doc.end_op();
}

fn max_alpha(doc: &Document) -> u16 {
    let Some(layer) = doc.layers.first() else {
        return 0;
    };
    let mut m = 0u16;
    for (_, tile) in layer.tiles() {
        for y in 0..mn_core::TILE_SIZE {
            for x in 0..mn_core::TILE_SIZE {
                m = m.max(tile.pixel(x, y)[3]);
            }
        }
    }
    m
}

/// Every canvas pixel of layer 0 set to opaque ink.
fn fill_layer(doc: &mut Document) {
    let ch = |c: f32| (c * 32768.0) as u16;
    let px = [ch(INK[0]), ch(INK[1]), ch(INK[2]), 32768];
    let (w, h) = doc.size;
    let Some(layer) = doc.layers.first_mut() else {
        return;
    };
    let ts = mn_core::TILE_SIZE as i32;
    for ty in 0..(h as i32 + ts - 1) / ts {
        for tx in 0..(w as i32 + ts - 1) / ts {
            let idx = TileIdx::of_pixel(
                tx * mn_core::TILE_SIZE as i32,
                ty * mn_core::TILE_SIZE as i32,
            );
            let tile = layer.tile_mut(idx);
            for y in 0..mn_core::TILE_SIZE {
                for x in 0..mn_core::TILE_SIZE {
                    tile.set_pixel(x, y, px);
                }
            }
        }
    }
}

/// Composite layer 0 over the theme's field colour, 2x2 box downsample.
fn composite(doc: &Document) -> egui::ColorImage {
    let bg = theme::FIELD;
    let (br, bg_, bb) = (bg.r() as f32, bg.g() as f32, bg.b() as f32);
    let layer = doc.layers.first();
    let sample = |x: i32, y: i32| -> [f32; 4] {
        let Some(layer) = layer else { return [0.0; 4] };
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        let p = layer
            .tile(idx)
            .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
            .unwrap_or([0; 4]);
        [
            p[0] as f32 / 32768.0,
            p[1] as f32 / 32768.0,
            p[2] as f32 / 32768.0,
            p[3] as f32 / 32768.0,
        ]
    };
    let mut px = Vec::with_capacity(PREVIEW_W * PREVIEW_H * 4);
    for oy in 0..PREVIEW_H {
        for ox in 0..PREVIEW_W {
            let mut acc = [0.0f32; 4];
            for dy in 0..2 {
                for dx in 0..2 {
                    let s = sample((ox * 2 + dx) as i32, (oy * 2 + dy) as i32);
                    for c in 0..4 {
                        acc[c] += s[c] * 0.25;
                    }
                }
            }
            // Premultiplied source over opaque background.
            let inv = 1.0 - acc[3];
            let to8 = |v: f32| (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            px.extend_from_slice(&[
                to8(acc[0] + br / 255.0 * inv),
                to8(acc[1] + bg_ / 255.0 * inv),
                to8(acc[2] + bb / 255.0 * inv),
                255,
            ]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([PREVIEW_W, PREVIEW_H], &px)
}
