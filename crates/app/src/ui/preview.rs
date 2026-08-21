//! Sub Tool stroke previews: each `.myb` preset drawn as a real stroke, so
//! the list shows what a brush *does* — exactly what CSP's Sub Tool palette
//! and Rebelle's preset grid show. Generated lazily (one per frame, budgeted
//! by the caller) and cached for the session.
//!
//! The second half of the file is the LIVE test stroke: the same idea aimed
//! at the brush controls instead of the list — CSP makes you tune sixteen
//! collapsed parameter pages blind, so the strip re-inks the sample stroke
//! with the current preset *and its live overrides* as you drag.

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

/// Mint a display-size texture from a gray page preview, bilinear on the
/// CPU. Shared by the Pages palette's sharp cell tier and the docking-2
/// page panes (ui/dock.rs) — each keeps its OWN texture cache because they
/// want different sizes of the same page.
pub fn mint_gray_tex(
    ctx: &egui::Context,
    gray: &image::GrayImage,
    dw: u32,
    dh: u32,
    name: String,
) -> egui::TextureHandle {
    let (gw, gh) = gray.dimensions();
    let (dw, dh) = (dw.max(1), dh.max(1));
    let mut ci = egui::ColorImage::new(
        [dw as usize, dh as usize],
        vec![egui::Color32::WHITE; (dw * dh) as usize],
    );
    for y in 0..dh {
        // Bilinear source position of this display px.
        let sy = (y as f32 + 0.5) * gh as f32 / dh as f32 - 0.5;
        let y0 = sy.floor().max(0.0).min(gh as f32 - 1.0) as u32;
        let y1 = (y0 + 1).min(gh - 1);
        let fy = (sy - y0 as f32).clamp(0.0, 1.0);
        for x in 0..dw {
            let sx = (x as f32 + 0.5) * gw as f32 / dw as f32 - 0.5;
            let x0 = sx.floor().max(0.0).min(gw as f32 - 1.0) as u32;
            let x1 = (x0 + 1).min(gw - 1);
            let fx = (sx - x0 as f32).clamp(0.0, 1.0);
            let s = |xx: u32, yy: u32| gray.get_pixel(xx, yy)[0] as f32;
            let v = (s(x0, y0) * (1.0 - fx) * (1.0 - fy)
                + s(x1, y0) * fx * (1.0 - fy)
                + s(x0, y1) * (1.0 - fx) * fy
                + s(x1, y1) * fx * fy)
                .round() as u8;
            ci[(x as usize, y as usize)] = egui::Color32::from_gray(v);
        }
    }
    ctx.load_texture(name, ci, egui::TextureOptions::LINEAR)
}

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
        fill_layer(&mut doc, INK);
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

/// Every canvas pixel of layer 0 set to opaque `rgb`.
fn fill_layer(doc: &mut Document, rgb: [f32; 3]) {
    let ch = |c: f32| (c * 32768.0) as u16;
    let px = [ch(rgb[0]), ch(rgb[1]), ch(rgb[2]), 32768];
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

// --- the live test stroke -------------------------------------------------

/// Strip height in points. The width follows the palette column.
pub const TEST_H: usize = 56;
/// Clamps on the palette-driven width, so a torn-off palette cannot ask for a
/// 4000 px re-render on every slider tick.
const TEST_W_MIN: usize = 48;
const TEST_W_MAX: usize = 512;

/// Checker square edge in px, and its two greys.
const CHECK: usize = 8;
const CHECK_A: [f32; 3] = [1.0, 1.0, 1.0];
const CHECK_B: [f32; 3] = [0.86, 0.86, 0.88];
/// The band an ERASER cuts through — an eraser on empty paper draws nothing,
/// so the ink has to be there first and the *gap* is the preview.
const BAND: [f32; 3] = [0.12, 0.12, 0.14];

/// How many canvas px the strip packs into one preview px.
///
/// 1 = life size, and small brushes stay there. A manga brush at 600 dpi is
/// 100 px wide — wider than the whole strip — so past a point the sample is
/// ZOOMED OUT rather than clipped into a featureless bar: a preview you cannot
/// see the tip in is the problem this feature exists to fix. The caption prints
/// the factor, so the strip never quietly lies about scale.
pub fn test_stroke_scale(size_px: f32) -> usize {
    let ideal = size_px / (TEST_H as f32 * 0.42);
    (ideal.round().max(1.0) as usize).clamp(1, TEST_SCALE_MAX)
}

/// Ceiling on that zoom-out. Beyond it the brush really is enormous and a solid
/// band is the honest answer — and it bounds the per-frame render cost.
const TEST_SCALE_MAX: usize = 4;

/// Render the sample stroke with the current preset and its live overrides.
///
/// A **fresh** engine every call, never the live one. libmypaint brush state
/// persists across strokes by design, so a same-engine replay starts
/// mid-state and inks fatter — the recorded vector-replay trap in
/// docs/CODE-MAP.md, which applies to any off-document replay. The whole
/// chain is rebuilt (`Stabilizer<Taper<Engine>>`, exactly what the canvas
/// draws through) so the Correction and taper rows reach the preview too,
/// and the document is a throwaway: nothing here can touch the artwork.
pub fn test_stroke_image(
    preset: Option<&Path>,
    props: &crate::cmd::ToolProps,
    color: [f32; 3],
    eraser: bool,
    texture: Option<&std::sync::Arc<mn_brush::TextureMask>>,
    width: usize,
) -> egui::ColorImage {
    use crate::app::{Engine, EngineKind};
    use mn_core::{Stabilizer, Taper};

    let w = width.clamp(TEST_W_MIN, TEST_W_MAX);
    let h = TEST_H;

    let kind = match preset.and_then(|p| MyBrush::load(p).ok()) {
        Some(b) => EngineKind::My(Box::new(b)),
        None => EngineKind::Dab(mn_brush::SimpleDab::new()),
    };
    let mut engine = Engine::new(kind);
    engine.apply_props_all(props, texture);
    // Also unconditionally, so the fallback dab (whose `apply_props` has no
    // preset state to guard and returns early) still answers the Size slider.
    engine.set_size_px(props.size_px);
    engine.set_color(color);
    engine.set_eraser(eraser);

    let mut chain = Stabilizer::new(Taper::new(engine), props.stabilizer);
    chain.set_correction(props.correct);
    {
        let t = chain.inner_mut();
        t.length_px = props.taper_px;
        t.min = props.taper_min;
    }

    // The brush is inked at its TRUE size into a document `k` times the strip,
    // then box-downsampled: a real zoom-out, so every proportion the brush
    // cares about (interval, scatter, texture grain) survives it.
    let k = test_stroke_scale(props.size_px);
    let mut doc = Document::new((w * k) as u32, (h * k) as u32);
    if eraser {
        fill_layer(&mut doc, BAND);
    }
    test_s_curve(&mut chain, &mut doc, (w * k) as f32, (h * k) as f32);
    composite_checker(&doc, w, h, k)
}

/// The sample gesture: one S-curve with synthesized pressure ramping
/// 0 → 1 → 0, so tip shape, taper, texture and the pressure dynamics all read
/// in a single stroke.
fn test_s_curve<S: StrokeSink>(sink: &mut S, doc: &mut Document, w: f32, h: f32) {
    // Samples scale with the strip so the pen's apparent SPEED (and with it
    // every speed-mapped dynamic) does not change when the palette is
    // resized — the same reason the mouse fallback stamps a real `t_ms`.
    let n = (w / 2.0).clamp(64.0, 256.0) as u32;
    doc.begin_op();
    sink.begin(doc);
    for k in 0..=n {
        let t = k as f32 / n as f32;
        let x = w * (0.07 + 0.86 * t);
        let y = h * 0.5 + (t * std::f32::consts::TAU * 0.82).sin() * h * 0.22;
        // `sin(π)` lands a hair below zero in f32 and a negative base makes
        // `powf` return NaN — clamp first (see `stroke` above; a NaN once
        // reached libmypaint as a dab radius).
        let pressure = (t * std::f32::consts::PI).sin().max(0.0).powf(0.6);
        sink.sample(
            doc,
            PenSample {
                x,
                y,
                pressure,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: (k as f64) * 4.0,
            },
        );
    }
    sink.end(doc);
    doc.end_op();
}

/// Composite layer 0 over a checkerboard, box-downsampling `k`×`k` — white
/// paper that still shows where an eraser knocked a hole through the band.
fn composite_checker(doc: &Document, w: usize, h: usize, k: usize) -> egui::ColorImage {
    let layer = doc.layers.first();
    let sample = |x: i32, y: i32| -> [f32; 4] {
        let idx = TileIdx::of_pixel(x, y);
        let (ox, oy) = idx.origin();
        let p = layer
            .and_then(|l| l.tile(idx))
            .map(|t| t.pixel((x - ox) as usize, (y - oy) as usize))
            .unwrap_or([0; 4]);
        [
            p[0] as f32 / 32768.0,
            p[1] as f32 / 32768.0,
            p[2] as f32 / 32768.0,
            p[3] as f32 / 32768.0,
        ]
    };
    let weight = 1.0 / (k * k) as f32;
    let mut px = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let bg = if (x / CHECK + y / CHECK) % 2 == 0 {
                CHECK_A
            } else {
                CHECK_B
            };
            let mut acc = [0.0f32; 4];
            for dy in 0..k {
                for dx in 0..k {
                    let s = sample((x * k + dx) as i32, (y * k + dy) as i32);
                    for c in 0..4 {
                        acc[c] += s[c] * weight;
                    }
                }
            }
            // Premultiplied source over an opaque background.
            let inv = 1.0 - acc[3];
            let to8 = |v: f32, b: f32| ((v + b * inv) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            px.extend_from_slice(&[
                to8(acc[0], bg[0]),
                to8(acc[1], bg[1]),
                to8(acc[2], bg[2]),
                255,
            ]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([w, h], &px)
}

/// Cache slot id. The strip is ONE texture with ONE consumer, so it lives in
/// egui's own per-context store (the `ui/top.rs`, `ui/color.rs` idiom) rather
/// than as another `App` field — nothing about it outlives the session and
/// nothing else reads it.
fn test_stroke_id() -> egui::Id {
    egui::Id::new("mn.teststroke")
}

impl crate::app::App {
    /// The live test-stroke texture at `width` points, re-rendered only when
    /// something it depends on moved — which is at most once per frame, since
    /// the panel asks once per frame. Dragging a slider therefore costs one
    /// small CPU stroke per frame and nothing at all when the pointer rests.
    pub fn test_stroke_tex(&mut self, width: f32) -> egui::TextureHandle {
        let w = width.round().max(0.0) as usize;
        let preset = self
            .selected_preset
            .and_then(|i| self.presets.get(i).map(|(_, p)| p.clone()));
        // Everything the render depends on, as one string. A NEW brush
        // parameter joins this key just by existing (`ToolProps: Debug`), so
        // the cache cannot go stale behind a control nobody remembered to
        // list — the cache-not-invalidated-through-a-door seam in
        // docs/CODE-MAP.md, closed by construction rather than by discipline.
        let key = format!(
            "{w}|{preset:?}|{:?}|{}|{:?}",
            self.props_current,
            self.eraser_active(),
            self.active_color()
        );
        let id = test_stroke_id();
        if let Some((k, t)) = self
            .shell
            .ctx
            .data(|d| d.get_temp::<(String, egui::TextureHandle)>(id))
            && k == key
        {
            return t;
        }
        // The engine already holds the resolved texture mask for the current
        // props (`App::apply_props` loaded it), so the strip reads it there
        // rather than re-resolving the name and risking a different answer.
        let mask = self.engine().texture_mask().cloned();
        let img = test_stroke_image(
            preset.as_deref(),
            &self.props_current,
            self.active_color(),
            self.eraser_active(),
            mask.as_ref(),
            w,
        );
        // Named by the key's hash, not a fixed string: two live handles under
        // one name alias each other (the page-thumb trick), and equal params
        // never reach here anyway — they hit the branch above.
        let tex = self.shell.ctx.load_texture(
            format!("mn.teststroke.{:016x}", fnv1a(&key)),
            img,
            egui::TextureOptions::LINEAR,
        );
        self.shell
            .ctx
            .data_mut(|d| d.insert_temp(id, (key, tex.clone())));
        tex
    }
}

/// FNV-1a over the cache key — a texture NAME, never a correctness decision
/// (the string comparison above is the actual cache test).
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

#[cfg(test)]
mod test_stroke_tests {
    use super::*;
    use crate::cmd::ToolProps;

    fn pen() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes/classic/pen.myb")
    }

    /// Pixels darker than the lighter checker square — i.e. ink.
    fn inked(img: &egui::ColorImage) -> usize {
        img.pixels.iter().filter(|p| p.r() < 200).count()
    }

    /// The strip renders, at the size asked for, and it has ink on it. An
    /// all-background buffer would be "non-empty" and still say nothing.
    #[test]
    fn test_stroke_renders_a_non_empty_buffer() {
        let props = ToolProps {
            size_px: 8.0,
            ..Default::default()
        };
        let img = test_stroke_image(Some(&pen()), &props, [0.0, 0.0, 0.0], false, None, 180);
        assert_eq!(img.size, [180, TEST_H], "the strip is the size asked for");
        assert_eq!(img.pixels.len(), 180 * TEST_H, "and fully populated");
        assert!(
            inked(&img) > 100,
            "the sample stroke must actually ink ({} px)",
            inked(&img)
        );
    }

    /// The claim the whole feature rests on: the LIVE overrides reach the
    /// engine, so the strip answers the Size slider instead of showing the
    /// preset's shipped size forever. (Fails against a render that loads the
    /// preset and skips `apply_props_all`.)
    #[test]
    fn test_stroke_size_reaches_the_engine() {
        let thin = ToolProps {
            size_px: 4.0,
            ..Default::default()
        };
        let fat = ToolProps {
            size_px: 20.0,
            ..thin
        };
        let a = test_stroke_image(Some(&pen()), &thin, [0.0, 0.0, 0.0], false, None, 180);
        let b = test_stroke_image(Some(&pen()), &fat, [0.0, 0.0, 0.0], false, None, 180);
        assert_ne!(a.pixels, b.pixels, "two sizes must not render identically");
        assert!(
            inked(&b) > inked(&a) * 2,
            "the fatter brush must lay down much more ink ({} vs {})",
            inked(&b),
            inked(&a)
        );
    }

    /// A manga brush at 600 dpi is 100 px wide — wider than the strip. It must
    /// be zoomed OUT, not clipped into a featureless bar, or the preview cannot
    /// answer the question it was added for.
    #[test]
    fn test_stroke_zooms_out_a_brush_too_fat_for_the_strip() {
        assert_eq!(test_stroke_scale(6.0), 1, "small brushes stay life size");
        let k = test_stroke_scale(100.0);
        assert!(k > 1, "a 100 px brush must be zoomed out (got 1:{k})");

        let props = ToolProps {
            size_px: 100.0,
            ..Default::default()
        };
        let img = test_stroke_image(Some(&pen()), &props, [0.0, 0.0, 0.0], false, None, 180);
        assert!(inked(&img) > 500, "it still inks");
        let bare = img.pixels.iter().filter(|p| p.r() > 200).count();
        assert!(
            bare > img.pixels.len() / 5,
            "and the strip is not a solid bar — paper must still show ({bare} of {})",
            img.pixels.len()
        );
    }

    /// Erasers draw nothing on empty paper, so the strip inks a dark band and
    /// the stroke is the hole through it.
    #[test]
    fn test_stroke_eraser_knocks_a_hole_in_a_band() {
        let props = ToolProps {
            size_px: 10.0,
            ..Default::default()
        };
        let img = test_stroke_image(Some(&pen()), &props, [0.0, 0.0, 0.0], true, None, 180);
        let light = img.pixels.iter().filter(|p| p.r() > 200).count();
        assert!(
            light > 100,
            "the erased path must show the checker through ({light} px)"
        );
        assert!(
            inked(&img) > light,
            "and the band around it must survive ({} vs {light})",
            inked(&img)
        );
    }
}
