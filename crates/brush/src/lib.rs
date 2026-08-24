//! MangaNakama brush engine.
//!
//! Two `core::StrokeSink` implementations live here — that trait is the seam
//! between "what makes pixels" and everything else, keep it clean:
//!
//! - [`MyBrush`] — the real one. libmypaint v1.6.1, vendored and compiled by
//!   `build.rs`, painting through a tiled surface straight into `core::Tile`
//!   buffers (`surface.rs`), driven by MyPaint `.myb` presets parsed in Rust
//!   (`mybrush.rs`). See `vendor/PATCHES.md` for what we changed in the C.
//! - [`SimpleDab`] — the placeholder round-dab brush from the walking skeleton
//!   (`dab.rs`). Kept until the shell finishes switching over; it depends on
//!   nothing here.
//!
//! Neither type is `Send`/`Sync`: libmypaint holds mutable stroke state and our
//! surface hands raw tile pointers to C. Single-threaded use is a contract with
//! the app crate.

mod abr;
mod abr_desc;
mod cpu_raster;
mod dab;
mod ffi;
mod gbr;
mod kpp;
mod mybrush;
pub mod sqlite_ro;
mod surface;
pub mod sut;
pub mod todb;

/// Brush setting/input ids, generated from `brushsettings.json` at build time.
pub mod settings;

pub use abr::{AbrSet, AbrTip, parse_abr, parse_abr_file, parse_abr_set};
pub use abr_desc::{AbrPresetInfo, BrushKind, Control, DynGroup, by_sample_id};
pub use cpu_raster::rasterize_dabs;
pub use gbr::{GbrBrush, parse_gbr, parse_gih, parse_gimp_brush_file};
pub use kpp::{KppPreset, parse_kpp, parse_kpp_file};

pub use dab::{CurveDab, DynaDab, GridDab, HairyDab, SimpleDab};
pub use mn_core::dab::{DabParams, DabRecord};
pub use mybrush::{
    AntiAlias, BrushError, BrushLibrary, DENSITY_BY_GAP_DEFAULT, Interval, MyBrush, RecordMode,
    SketchParams, TextureMask, commit_wash, load_texture,
};
pub use surface::{TileOracle, set_tile_oracle};

/// Row 42 / A-014 (CSP はみ出さない, "do not cross lines of the reference
/// layer"): a per-pixel paint barrier shared by every engine, built once
/// per stroke from the reference set's ink plus frame-border folders
/// (the owner's widened referent ruling). `allow` is canvas-wide, one byte
/// per pixel: 255 = paint freely, 0 = the reference's ink — a blocked
/// pixel is never painted, so a scribble stays inside the lines.
#[derive(Clone, Debug)]
pub struct AntiOverflowMask {
    /// Canvas width in pixels (the stride of `allow`).
    pub w: usize,
    /// 255 = paintable, 0 = blocked (reference ink).
    pub allow: Vec<u8>,
}

impl AntiOverflowMask {
    /// True when `(x, y)` is the reference's ink and must stay unpainted.
    /// Off-canvas coordinates read as blocked — safer than painting past
    /// an edge the barrier was meant to close.
    pub fn blocked(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 {
            return true;
        }
        let i = y as usize * self.w + x as usize;
        self.allow.get(i).is_none_or(|a| *a == 0)
    }
}

/// The engine a preset asks for through its optional `mn-engine` key
/// (`"grid"` / `"hairy"` / `"curve"` / `"dyna"`), or `None` for an ordinary
/// MyPaint `.myb`.
///
/// `.myb` is JSON with an open schema, so a procedural sub-tool identity rides
/// in one extra key instead of a second preset format. The SNIFF lives here,
/// below the app, because several readers need the same answer — the live
/// engine, the Sub Tool swatch, the property panel's test strip — and a reader
/// that sniffed it its own way is exactly how a preset ends up drawing as one
/// brush and previewing as another. Returns the raw name (not an engine) so
/// this crate does not need to know the app's `EngineKind`.
pub fn preset_engine_key(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let j: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(j.get("mn-engine")?.as_str()?.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mn_core::{Document, PenSample, StrokeSink, TILE_LEN, TILE_SIZE, TileIdx};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn sample(x: f32, y: f32, p: f32, t: f64) -> PenSample {
        PenSample {
            x,
            y,
            pressure: p,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: t,
        }
    }

    /// Synthetic stroke harness: scripted PenSamples -> brush -> doc.
    /// No GPU, no window.
    #[test]
    fn synthetic_stroke_paints_the_tiles_it_crosses() {
        let mut doc = Document::default();
        let mut brush = SimpleDab::new();

        // Horizontal stroke across three tile columns of row 1:
        // y = 100 -> tile row 1; x 100..300 -> tile cols 1,2,3,4.
        brush.begin(&mut doc);
        for i in 0..=20 {
            let x = 100.0 + i as f32 * 10.0;
            brush.sample(&mut doc, sample(x, 100.0, 1.0, i as f64 * 8.0));
        }
        brush.end(&mut doc);

        let layer = doc.active_layer();
        assert!(layer.tile_count() > 0, "stroke painted nothing");

        for tx in 1..=4 {
            let idx = TileIdx::new(tx, 1);
            let tile = layer
                .tile(idx)
                .unwrap_or_else(|| panic!("expected tile {idx:?} to exist"));
            assert!(
                !tile.is_blank(),
                "tile {idx:?} exists but is fully transparent"
            );
            assert!(tile.alpha_sum() > 0, "tile {idx:?} has no alpha");
        }

        // Nowhere near the stroke -> no tile allocated at all (sparse layers).
        assert!(layer.tile(TileIdx::new(20, 20)).is_none());
        assert!(layer.tile(TileIdx::new(1, 10)).is_none());
    }

    #[test]
    fn pressure_drives_radius_and_alpha() {
        let brush = SimpleDab::new();
        assert!((brush.radius_for(0.0) - 1.0).abs() < 1e-6);
        assert!((brush.radius_for(1.0) - 12.0).abs() < 1e-6);
        assert!(brush.radius_for(0.5) > brush.radius_for(0.1));
        assert!(brush.alpha_for(1.0) > brush.alpha_for(0.2));

        // Light stroke must cover fewer pixels than a heavy one.
        let mut light = Document::default();
        let mut heavy = Document::default();
        for (doc, p) in [(&mut light, 0.05f32), (&mut heavy, 1.0f32)] {
            let mut b = SimpleDab::new();
            b.begin(doc);
            b.sample(doc, sample(512.0, 512.0, p, 0.0));
            b.sample(doc, sample(560.0, 512.0, p, 10.0));
            b.end(doc);
        }
        let a_light = light
            .active_layer()
            .tile(TileIdx::of_pixel(512, 512))
            .unwrap()
            .alpha_sum();
        let a_heavy = heavy
            .active_layer()
            .tile(TileIdx::of_pixel(512, 512))
            .unwrap()
            .alpha_sum();
        assert!(a_heavy > a_light * 4, "heavy={a_heavy} light={a_light}");
    }

    #[test]
    fn output_stays_premultiplied_and_in_range() {
        let mut doc = Document::default();
        let mut brush = SimpleDab::new();
        brush.color = [1.0, 0.0, 0.0];
        brush.begin(&mut doc);
        for i in 0..30 {
            brush.sample(&mut doc, sample(300.0 + i as f32, 300.0, 1.0, i as f64));
        }
        brush.end(&mut doc);

        let tile = doc
            .active_layer()
            .tile(TileIdx::of_pixel(300, 300))
            .unwrap();
        for px in tile.data().chunks_exact(4) {
            let a = px[3];
            assert!(a <= 32768, "alpha {a} out of fix15 range");
            for (c, &v) in px[..3].iter().enumerate() {
                assert!(
                    v <= a.max(1) + 1,
                    "channel {c}={v} exceeds alpha {a}: not premultiplied"
                );
            }
        }
    }

    #[test]
    fn stroke_clips_to_the_canvas_without_panicking() {
        let mut doc = Document::new(128, 128);
        let mut brush = SimpleDab::new();
        brush.begin(&mut doc);
        for x in [-50.0f32, -5.0, 0.0, 64.0, 127.0, 200.0] {
            brush.sample(&mut doc, sample(x, 64.0, 1.0, 0.0));
        }
        brush.end(&mut doc);
        // 128x128 -> tiles (0,0)..(1,1) only.
        for (idx, _) in doc.active_layer().tiles() {
            assert!(
                (0..2).contains(&idx.x) && (0..2).contains(&idx.y),
                "escaped: {idx:?}"
            );
        }
    }

    // ---------------------------------------------------------------- MyBrush

    fn preset_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes/classic")
    }

    fn preset(name: &str) -> PathBuf {
        preset_dir().join(name)
    }

    /// Audit M1 (docs/AUDIT-2026-08-17-opus.md): `rng01` shifted by 33, which
    /// leaves 31 bits over a `u32::MAX` divisor — the result never exceeded
    /// 0.5. The sketch engine gates link density on `rng01() < density` and
    /// picks link targets with `rng01() * history.len()`, so the old range
    /// doubled the effective density and confined every target to the older
    /// half of the history ring. Asserts the range is actually used.
    #[test]
    fn rng01_covers_the_whole_unit_interval() {
        let mut b = MyBrush::load(&preset("pen.myb")).expect("pen.myb");
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let mut above_half = 0;
        for _ in 0..10_000 {
            let v = b.rng01();
            assert!((0.0..=1.0).contains(&v), "rng01 out of range: {v}");
            lo = lo.min(v);
            hi = hi.max(v);
            if v > 0.5 {
                above_half += 1;
            }
        }
        assert!(hi > 0.99, "never reached the top of the range (max {hi})");
        assert!(
            lo < 0.01,
            "never reached the bottom of the range (min {lo})"
        );
        // Uniform over 0..1 => ~5000; the old 0..0.5 bug gives exactly 0.
        assert!(
            (4000..6000).contains(&above_half),
            "not uniform over 0..1: {above_half}/10000 samples above 0.5"
        );
    }

    /// The vertical alpha profile across a horizontal stroke: how many rows
    /// are PARTIALLY inked (between 15% and 85% of the stroke's peak row
    /// alpha). A crisper edge profile = fewer partial rows.
    fn partial_edge_rows(doc: &Document, x_probe: i32) -> usize {
        let mut peak = 0u32;
        let mut rows = [0u32; 1024];
        for y in 0..1024i32 {
            let idx = TileIdx::of_pixel(x_probe, y);
            let Some(t) = doc.active_layer().tile(idx) else {
                continue;
            };
            let v = t.pixel(
                (x_probe - idx.origin().0) as usize,
                (y - idx.origin().1) as usize,
            )[3];
            rows[y as usize] = v as u32;
            peak = peak.max(v as u32);
        }
        if peak == 0 {
            return usize::MAX;
        }
        rows.iter()
            .filter(|&&v| v > peak * 15 / 100 && v < peak * 85 / 100)
            .count()
    }

    /// Krita-style hard stamp dabs: the edge profile must be measurably
    /// crisper than the gaussian falloff at the same radius — that is the
    /// whole point of the mode (CSP pen-crisp ink).
    #[test]
    fn hard_dab_edges_are_sharper_than_gaussian() {
        let mut gauss = Document::new(1024, 1024);
        let mut g = MyBrush::load(&preset("pen.myb")).unwrap();
        straight_stroke(&mut g, &mut gauss, 512.0, 1.0);
        let mut hard = Document::new(1024, 1024);
        let mut h = MyBrush::load(&preset("pen.myb")).unwrap();
        h.set_hard_dab(true);
        straight_stroke(&mut h, &mut hard, 512.0, 1.0);

        let pg = partial_edge_rows(&gauss, 200);
        let ph = partial_edge_rows(&hard, 200);
        assert!(
            ph < pg,
            "hard edge not crisper: partial rows hard={ph} gauss={pg}"
        );
        // And the mode OFF reproduces stock pixels (regression pin): loading
        // the same preset twice without the flag paints identically.
        let mut again = Document::new(1024, 1024);
        let mut g2 = MyBrush::load(&preset("pen.myb")).unwrap();
        straight_stroke(&mut g2, &mut again, 512.0, 1.0);
        assert_eq!(total_alpha(&gauss), total_alpha(&again));
    }

    /// Krita Scatter: dabs land around the path, so the stroke's painted
    /// band grows with the scatter amount.
    #[test]
    fn scatter_widens_the_stroke_band() {
        let plain = {
            let mut d = Document::new(1024, 1024);
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            straight_stroke(&mut b, &mut d, 512.0, 1.0);
            d
        };
        let scattered = {
            let mut d = Document::new(1024, 1024);
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            b.set_scatter(1.5);
            straight_stroke(&mut b, &mut d, 512.0, 1.0);
            d
        };
        let (_, y0, _, y1) = painted_bbox(&plain).unwrap();
        let (_, s0, _, s1) = painted_bbox(&scattered).unwrap();
        assert!(
            s1 - s0 >= (y1 - y0) + 3,
            "scatter did not widen the band: plain {} vs scattered {}",
            y1 - y0,
            s1 - s0
        );
    }

    /// The round-25 Krita-ported presets carry the new top-level keys and the
    /// loader picks them up.
    #[test]
    fn krita_presets_load_with_modes() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes/krita");
        let hard = MyBrush::load(&dir.join("hard-ink.myb")).unwrap();
        assert!(hard.hard_dab(), "hard-ink should enable hard dabs");
        assert_eq!(hard.scatter(), 0.0);
        let sketch = MyBrush::load(&dir.join("sketch-scatter.myb")).unwrap();
        assert!(sketch.hard_dab());
        assert!((sketch.scatter() - 0.5).abs() < 1e-6);
        // The M1 re-tune: this preset's 0.3 was authored while `rng01`
        // covered only 0..0.5, so the density gate fired at double its
        // setting (effective 0.6). The RNG fix (audit 2026-08-17) made the
        // stored value literal, halving the web — the preset re-pins to
        // 0.6 to restore the authored link rate.
        let pen = MyBrush::load(&dir.join("sketch-pen.myb")).unwrap();
        let s = pen.sketch().expect("sketch-pen must carry mn-sketch");
        assert!((s.distance - 40.0).abs() < 1e-6);
        assert!(
            (s.density - 0.6).abs() < 1e-6,
            "M1 re-tune regressed: {}",
            s.density
        );
        // Stock presets must stay untouched: no key, no mode.
        let pen = MyBrush::load(&preset("pen.myb")).unwrap();
        assert!(!pen.hard_dab() && pen.scatter() == 0.0);
    }

    /// Bounding box of every pixel with non-zero alpha, in canvas coordinates.
    fn painted_bbox(doc: &Document) -> Option<(i32, i32, i32, i32)> {
        let mut bb: Option<(i32, i32, i32, i32)> = None;
        for (idx, tile) in doc.active_layer().tiles() {
            let (ox, oy) = idx.origin();
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    if tile.pixel(x, y)[3] == 0 {
                        continue;
                    }
                    let (px, py) = (ox + x as i32, oy + y as i32);
                    bb = Some(match bb {
                        None => (px, py, px, py),
                        Some((x0, y0, x1, y1)) => (x0.min(px), y0.min(py), x1.max(px), y1.max(py)),
                    });
                }
            }
        }
        bb
    }

    fn total_alpha(doc: &Document) -> u64 {
        doc.active_layer().tiles().map(|(_, t)| t.alpha_sum()).sum()
    }

    /// Max per-channel delta between two docs over the union of their tiles
    /// (missing tiles read as transparent). The P1 parity bar: ≤ 1.
    fn max_channel_diff(a: &Document, b: &Document) -> u32 {
        let mut tiles: std::collections::BTreeSet<TileIdx> = Default::default();
        for (idx, _) in a.active_layer().tiles() {
            tiles.insert(idx);
        }
        for (idx, _) in b.active_layer().tiles() {
            tiles.insert(idx);
        }
        let zero = vec![0u16; TILE_LEN];
        let mut max: u32 = 0;
        for &idx in &tiles {
            let pa = a
                .active_layer()
                .tile(idx)
                .map(|t| t.data())
                .unwrap_or(&zero);
            let pb = b
                .active_layer()
                .tile(idx)
                .map(|t| t.data())
                .unwrap_or(&zero);
            for (x, y) in pa.iter().zip(pb.iter()) {
                max = max.max(x.abs_diff(*y) as u32);
            }
        }
        max
    }

    /// The Rust CPU mirror (`cpu_raster`) vs the vendored C rasterizer
    /// through a TAP record: the GPU-dabs canary-repair path re-rasterizes
    /// exactly this way, so the mirror must land within the parity
    /// tolerance of the real C (same u32 blend math; the f32 mask
    /// re-derived — ≤1/32765 per channel).
    #[test]
    fn cpu_raster_mirror_matches_the_c() {
        let mut doc_a = Document::new(512, 512);
        let mut a = MyBrush::load(&preset("pen.myb")).unwrap();
        a.set_dab_recording(RecordMode::Tap);
        straight_stroke(&mut a, &mut doc_a, 200.0, 0.8);
        let rec = a.take_dab_record();
        assert!(!rec.dabs.is_empty(), "tap recorded nothing");

        let mut doc_b = Document::new(512, 512);
        crate::rasterize_dabs(&mut doc_b, 0, &rec.dabs, false, None);
        let max = max_channel_diff(&doc_a, &doc_b);
        assert!(max <= 1, "cpu mirror drifted: max channel diff {max}");
        assert!(total_alpha(&doc_b) > 0, "mirror painted nothing");
    }

    /// The mirror's eraser arm: erasing arrives as colour_a < 1, the
    /// Normal_and_Eraser blend, over existing ink.
    #[test]
    fn cpu_raster_mirror_matches_the_c_eraser() {
        let ink = |doc: &mut Document| {
            let tile = doc.layers[0].tile_mut(TileIdx::new(2, 3));
            for px in tile.data_mut().chunks_exact_mut(4) {
                px[0] = 20000;
                px[1] = 15000;
                px[2] = 10000;
                px[3] = 32768;
            }
        };
        let mut doc_a = Document::new(512, 512);
        ink(&mut doc_a);
        let mut a = MyBrush::load(&preset("pen.myb")).unwrap();
        a.set_eraser(true);
        a.set_dab_recording(RecordMode::Tap);
        // Stroke through the inked tile (y = 3*64..4*64, x = 128..405).
        straight_stroke(&mut a, &mut doc_a, 220.0, 0.9);
        let rec = a.take_dab_record();
        assert!(!rec.dabs.is_empty());

        let mut doc_b = Document::new(512, 512);
        ink(&mut doc_b);
        crate::rasterize_dabs(&mut doc_b, 0, &rec.dabs, false, None);
        let max = max_channel_diff(&doc_a, &doc_b);
        assert!(
            max <= 1,
            "cpu mirror eraser drifted: max channel diff {max}"
        );
    }

    /// Straight horizontal stroke at constant pressure. Enough samples that
    /// `slow_tracking` (0.65 in pen.myb) has caught up well before the end.
    fn straight_stroke(brush: &mut MyBrush, doc: &mut Document, y: f32, pressure: f32) {
        brush.begin(doc);
        for i in 0..=60 {
            let x = 100.0 + i as f32 * 5.0;
            brush.sample(doc, sample(x, y, pressure, i as f64 * 8.0));
        }
        brush.end(doc);
    }

    /// Our generated ids are handed straight to the C setters, so if they ever
    /// disagreed with the C enum, presets would load into the wrong settings
    /// with no error at all. Ask the C itself.
    #[test]
    fn generated_ids_match_the_c_enum() {
        for (i, name) in settings::SETTING_NAMES.iter().enumerate() {
            let c = std::ffi::CString::new(*name).unwrap();
            let id = unsafe { ffi::mypaint_brush_setting_from_cname(c.as_ptr()) };
            assert_eq!(id, i as i32, "setting {name:?} id mismatch");
        }
        for (i, name) in settings::INPUT_NAMES.iter().enumerate() {
            let c = std::ffi::CString::new(*name).unwrap();
            let id = unsafe { ffi::mypaint_brush_input_from_cname(c.as_ptr()) };
            assert_eq!(id, i as i32, "input {name:?} id mismatch");
        }
        // Named constants must point at the same places.
        assert_eq!(
            settings::setting_id("radius_logarithmic"),
            Some(settings::setting::RADIUS_LOGARITHMIC)
        );
        assert_eq!(
            settings::setting_id("eraser"),
            Some(settings::setting::ERASER)
        );
        assert_eq!(
            settings::input_id("pressure"),
            Some(settings::input::PRESSURE)
        );
        assert_eq!(settings::setting_id("no_such_setting"), None);
    }

    #[test]
    fn loads_the_inking_stars() {
        for f in ["pen.myb", "kabura.myb", "pointy_ink.myb"] {
            let b = MyBrush::load(&preset(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
            assert_eq!(b.name(), f.trim_end_matches(".myb"));
            // Preset values actually landed (stock default for this is 2.0).
            assert!(
                (b.base_value(settings::setting::RADIUS_LOGARITHMIC) - 2.0).abs() > 1e-6,
                "{f}: radius_logarithmic still at the stock default"
            );
        }

        // Mapping points, not just base values: pen.myb maps pressure and
        // speed1 onto radius_logarithmic with 2 points each.
        let pen = MyBrush::load(&preset("pen.myb")).unwrap();
        assert_eq!(
            pen.mapping_n(
                settings::setting::RADIUS_LOGARITHMIC,
                settings::input::PRESSURE
            ),
            2
        );
        assert_eq!(
            pen.mapping_n(
                settings::setting::RADIUS_LOGARITHMIC,
                settings::input::SPEED1
            ),
            2
        );
        assert_eq!(
            pen.mapping_n(
                settings::setting::RADIUS_LOGARITHMIC,
                settings::input::STROKE
            ),
            0
        );
    }

    #[test]
    fn every_classic_preset_loads() {
        let found = BrushLibrary::scan(&preset_dir());
        assert_eq!(
            found.len(),
            35,
            "expected 35 classic presets, got {}",
            found.len()
        );
        assert!(found.iter().any(|(n, _)| n == "pen"));
        assert!(found.iter().any(|(n, _)| n == "kabura"));
        // Display names are de-underscored.
        assert!(found.iter().any(|(n, _)| n == "marker fat"));

        for (name, path) in &found {
            MyBrush::load(path).unwrap_or_else(|e| panic!("{name} ({path:?}): {e}"));
        }
    }

    /// Every shipped preset (CSP imports included) must survive a stroke on a
    /// preview-sized document — this is exactly what the Sub Tool palette's
    /// stroke previews do (`mn-app` `ui/preview.rs`).
    #[test]
    fn every_preset_strokes_on_a_tiny_document() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes");
        let found = BrushLibrary::scan(&root);
        assert!(
            found.len() >= 40,
            "expected all presets, got {}",
            found.len()
        );
        for (name, path) in &found {
            eprintln!("stroking {name}");
            // `mn-engine` sub-tools (TODO #7) exercise their own engine;
            // MyBrush::load skips the key, so stroke each engine directly.
            let engine = preset_engine_key(path);
            let samples: Vec<PenSample> = (0..40)
                .map(|i| PenSample {
                    x: 10.0 + i as f32 * 2.8,
                    y: 18.0,
                    pressure: 1.0,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: i as f64 * 8.0,
                })
                .collect();
            let mut special: Option<Box<dyn StrokeSink>> = match engine.as_deref() {
                Some("grid") => {
                    let mut g = GridDab::default();
                    g.pitch = g.pitch.min(12.0);
                    Some(Box::new(g))
                }
                Some("hairy") => Some(Box::new(HairyDab::default())),
                Some("curve") => Some(Box::new(CurveDab::default())),
                Some("dyna") => Some(Box::new(DynaDab::default())),
                _ => None,
            };
            if let Some(eng) = special.as_deref_mut() {
                let mut doc = Document::new(128, 36);
                doc.begin_op();
                eng.begin(&mut doc);
                for s in samples {
                    eng.sample(&mut doc, s);
                }
                eng.end(&mut doc);
                doc.end_op();
                continue;
            }
            let mut b = MyBrush::load(path).unwrap_or_else(|e| panic!("{name}: {e}"));
            let r = b.radius_px().clamp(0.1, 400.0);
            b.set_size_multiplier(9.4 / r);
            b.set_color_rgb([0.84, 0.84, 0.87]);
            let mut doc = Document::new(128, 36);
            doc.begin_op();
            b.begin(&mut doc);
            for k in 0..=96u32 {
                let t = k as f32 / 96.0;
                let x = 128.0 * (0.06 + 0.88 * t);
                let y = 18.0 + (t * std::f32::consts::TAU * 0.82).sin() * 6.0;
                b.sample(
                    &mut doc,
                    sample(
                        x,
                        y,
                        (t * std::f32::consts::PI).sin().max(0.0).powf(0.6),
                        k as f64 * 3.5,
                    ),
                );
            }
            // Regression guard: non-finite samples must be dropped at the FFI
            // boundary, not become NaN dab radii inside libmypaint.
            b.sample(&mut doc, sample(64.0, 18.0, f32::NAN, 400.0));
            b.sample(&mut doc, sample(f32::NAN, 18.0, 0.5, 404.0));
            b.sample(&mut doc, sample(64.0, f32::INFINITY, 0.5, 408.0));
            b.end(&mut doc);
            doc.end_op();
        }
    }

    #[test]
    fn rejects_junk_presets() {
        let dir = std::env::temp_dir().join("mn-brush-tests");
        std::fs::create_dir_all(&dir).unwrap();

        let bad_json = dir.join("bad.myb");
        std::fs::write(&bad_json, "{ not json").unwrap();
        assert!(matches!(MyBrush::load(&bad_json), Err(BrushError::Json(_))));

        let wrong_version = dir.join("v2.myb");
        std::fs::write(&wrong_version, r#"{"version": 2, "settings": {}}"#).unwrap();
        assert!(matches!(
            MyBrush::load(&wrong_version),
            Err(BrushError::Format(_))
        ));

        assert!(matches!(
            MyBrush::load(&dir.join("does-not-exist.myb")),
            Err(BrushError::Io(_))
        ));
    }

    #[test]
    fn mybrush_paints_into_document_tiles() {
        let mut doc = Document::new(512, 512);
        let mut brush = MyBrush::load(&preset("pen.myb")).unwrap();
        brush.set_color_rgb([0.0, 0.0, 0.0]);
        straight_stroke(&mut brush, &mut doc, 256.0, 1.0);

        let layer = doc.active_layer();
        assert!(layer.tile_count() > 0, "libmypaint painted nothing");
        assert!(
            layer.tiles().any(|(_, t)| !t.is_blank()),
            "tiles exist but every one is fully transparent"
        );
        assert!(total_alpha(&doc) > 0);

        // Every tile we allocated must be inside the canvas and carry ink.
        for (idx, tile) in layer.tiles() {
            assert!(
                (0..8).contains(&idx.x) && (0..8).contains(&idx.y),
                "tile {idx:?} escaped the 512x512 canvas"
            );
            assert!(!tile.is_blank(), "allocated a blank tile at {idx:?}");
        }

        // Premultiplied fix15, in range — the format the GPU compositor and the
        // ORA/PNG export paths both assume.
        for (_, tile) in layer.tiles() {
            for px in tile.data().chunks_exact(4) {
                let a = px[3];
                assert!(a <= 32768, "alpha {a} out of fix15 range");
                for (c, &v) in px[..3].iter().enumerate() {
                    assert!(v <= a.max(1) + 1, "channel {c}={v} exceeds alpha {a}");
                }
            }
        }
    }

    /// Regression, and the one bug this whole crate could most easily ship
    /// silently: libmypaint applies `slow_tracking` smoothing *before* it
    /// honours a reset, so a stroke can start by dragging ink in from wherever
    /// the brush state last was. Measured on the first cut: a stroke along
    /// y=256 painted a bounding box of (0, 1)-(399, 257) — a diagonal smear
    /// across a quarter of the canvas. See `FIRST_SAMPLE_DTIME`.
    #[test]
    fn a_stroke_starts_where_the_pen_touched_down() {
        let mut doc = Document::new(512, 512);
        let mut brush = MyBrush::load(&preset("pen.myb")).unwrap();

        // First stroke: brush state is fresh, i.e. sitting at the origin.
        straight_stroke(&mut brush, &mut doc, 256.0, 1.0);
        let (x0, y0, _, y1) = painted_bbox(&doc).expect("first stroke painted nothing");
        assert!(x0 >= 90, "ink left of the stroke start (x=100): x0={x0}");
        assert!(
            (240..=272).contains(&y0) && (240..=272).contains(&y1),
            "first stroke smeared off its line: y {y0}..{y1}"
        );

        // Second stroke somewhere else: brush state now holds the end of the
        // first one, so this catches the same bug in its milder form.
        let mut doc = Document::new(512, 512);
        straight_stroke(&mut brush, &mut doc, 64.0, 1.0);
        let (x0, y0, _, y1) = painted_bbox(&doc).expect("second stroke painted nothing");
        assert!(x0 >= 90, "ink left of the stroke start (x=100): x0={x0}");
        assert!(
            (48..=80).contains(&y0) && (48..=80).contains(&y1),
            "second stroke smeared in from the previous stroke: y {y0}..{y1}"
        );
    }

    /// The app's whole stroke path in miniature: `Stabilizer` decorating
    /// `MyBrush`, bracketed by `Document::begin_op`/`end_op`, then undone.
    ///
    /// Two seams at once. (1) The decorator must forward `begin` — if it does
    /// not, libmypaint never gets its reset and the *second* stroke smears in
    /// from where the first one ended (the `FIRST_SAMPLE_DTIME` bug, one layer
    /// up). (2) The brush knows nothing about undo, so the op bracket is the
    /// only thing making a stroke undoable.
    #[test]
    fn stabilized_stroke_is_one_undoable_op() {
        let brush = MyBrush::load(&preset("pen.myb")).unwrap();
        let mut sink = mn_core::Stabilizer::new(brush, 0.5);

        let stroke = |sink: &mut mn_core::Stabilizer<MyBrush>, doc: &mut Document, y: f32| {
            doc.begin_op();
            sink.begin(doc);
            for i in 0..=60 {
                sink.sample(doc, sample(100.0 + i as f32 * 5.0, y, 1.0, i as f64 * 8.0));
            }
            sink.end(doc);
            doc.end_op()
        };

        let mut doc = Document::new(512, 512);
        assert!(
            stroke(&mut sink, &mut doc, 256.0),
            "the stroke recorded no tiles"
        );
        let painted = total_alpha(&doc);
        assert!(painted > 0, "stabilized stroke painted nothing");

        // A second stroke elsewhere: this is the one that catches a decorator
        // swallowing `begin`.
        let mut doc2 = Document::new(512, 512);
        assert!(stroke(&mut sink, &mut doc2, 64.0));
        let (x0, y0, _, y1) = painted_bbox(&doc2).expect("second stroke painted nothing");
        assert!(x0 >= 90, "ink left of the stroke start (x=100): x0={x0}");
        assert!(
            (48..=80).contains(&y0) && (48..=80).contains(&y1),
            "stroke smeared off its line (begin not forwarded?): y {y0}..{y1}"
        );

        // Undo is one step for the whole stroke, drain included.
        assert_eq!(doc.undo_len(), 1);
        assert!(doc.undo());
        assert_eq!(total_alpha(&doc), 0, "undo left ink behind");
        assert!(doc.redo());
        assert_eq!(
            total_alpha(&doc),
            painted,
            "redo did not restore the stroke"
        );
    }

    #[test]
    fn heavier_pressure_paints_a_wider_stroke() {
        let mut light = Document::new(512, 512);
        let mut heavy = Document::new(512, 512);

        let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
        straight_stroke(&mut b, &mut light, 256.0, 0.2);
        let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
        straight_stroke(&mut b, &mut heavy, 256.0, 0.9);

        let (_, ly0, _, ly1) = painted_bbox(&light).expect("pressure 0.2 painted nothing");
        let (_, hy0, _, hy1) = painted_bbox(&heavy).expect("pressure 0.9 painted nothing");
        let (lw, hw) = (ly1 - ly0, hy1 - hy0);
        assert!(
            hw > lw,
            "pressure 0.9 stroke ({hw}px wide) is not wider than pressure 0.2 ({lw}px)"
        );
        assert!(
            total_alpha(&heavy) > total_alpha(&light),
            "heavier pressure laid down less ink"
        );
    }

    #[test]
    fn eraser_removes_alpha_it_previously_laid_down() {
        let mut doc = Document::new(512, 512);
        let mut brush = MyBrush::load(&preset("pen.myb")).unwrap();

        straight_stroke(&mut brush, &mut doc, 256.0, 1.0);
        let painted = total_alpha(&doc);
        assert!(painted > 0, "nothing to erase");

        brush.set_eraser(true);
        // Fatter than the ink stroke so the eraser covers it.
        brush.set_size_multiplier(3.0);
        straight_stroke(&mut brush, &mut doc, 256.0, 1.0);

        let after = total_alpha(&doc);
        assert!(
            after < painted,
            "eraser did not reduce alpha: {painted} -> {after}"
        );
    }

    #[test]
    fn size_multiplier_is_logarithmic_and_does_not_compound() {
        let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
        let base = b.base_value(settings::setting::RADIUS_LOGARITHMIC);

        b.set_size_multiplier(2.0);
        let doubled = b.base_value(settings::setting::RADIUS_LOGARITHMIC);
        assert!(
            (doubled - (base + 2.0f32.ln())).abs() < 1e-5,
            "expected base + ln(2), got {doubled} from {base}"
        );

        // Re-derived from the preset value, never accumulated.
        b.set_size_multiplier(2.0);
        assert!((b.base_value(settings::setting::RADIUS_LOGARITHMIC) - doubled).abs() < 1e-5);

        b.set_size_multiplier(1.0);
        assert!((b.base_value(settings::setting::RADIUS_LOGARITHMIC) - base).abs() < 1e-5);

        // Nonsense multipliers must not produce exp(-inf)/NaN radii.
        b.set_size_multiplier(0.0);
        assert!(
            b.base_value(settings::setting::RADIUS_LOGARITHMIC)
                .is_finite()
        );
        b.set_size_multiplier(f32::NAN);
        assert!(
            b.base_value(settings::setting::RADIUS_LOGARITHMIC)
                .is_finite()
        );
    }

    #[test]
    fn strokes_off_canvas_do_not_grow_the_layer() {
        let mut doc = Document::new(128, 128);
        let mut brush = MyBrush::load(&preset("pen.myb")).unwrap();

        brush.begin(&mut doc);
        for (i, x) in [-500.0f32, -60.0, 0.0, 64.0, 127.0, 400.0, 5000.0]
            .into_iter()
            .enumerate()
        {
            brush.sample(&mut doc, sample(x, 64.0, 1.0, i as f64 * 8.0));
        }
        brush.end(&mut doc);

        // 128x128 -> tiles (0,0)..(1,1) only.
        for (idx, _) in doc.active_layer().tiles() {
            assert!(
                (0..2).contains(&idx.x) && (0..2).contains(&idx.y),
                "off-canvas dab allocated tile {idx:?}"
            );
        }
    }

    /// A stroke that never moves, and one with a backwards timestamp: both are
    /// things a real pen produces, and both feed a division by dtime inside
    /// libmypaint.
    #[test]
    fn degenerate_input_does_not_panic() {
        let mut doc = Document::new(256, 256);
        let mut brush = MyBrush::load(&preset("kabura.myb")).unwrap();

        brush.begin(&mut doc);
        for _ in 0..10 {
            brush.sample(&mut doc, sample(128.0, 128.0, 0.5, 0.0));
        }
        brush.sample(&mut doc, sample(128.0, 128.0, 0.5, -50.0));
        brush.end(&mut doc);
    }

    #[test]
    fn smudge_preset_reads_the_canvas_without_escaping_it() {
        let mut doc = Document::new(256, 256);
        // Lay down ink first so there is something to smudge.
        let mut pen = MyBrush::load(&preset("pen.myb")).unwrap();
        pen.set_color_rgb([1.0, 0.0, 0.0]);
        pen.begin(&mut doc);
        for i in 0..=20 {
            pen.sample(
                &mut doc,
                sample(60.0 + i as f32 * 4.0, 128.0, 1.0, i as f64 * 8.0),
            );
        }
        pen.end(&mut doc);
        let tiles_after_ink = doc.active_layer().tile_count();
        assert!(tiles_after_ink > 0);

        // smudge.myb exercises the read-only tile path (libmypaint get_color).
        let mut smudge = MyBrush::load(&preset("smudge.myb")).unwrap();
        smudge.begin(&mut doc);
        for i in 0..=20 {
            smudge.sample(
                &mut doc,
                sample(60.0 + i as f32 * 4.0, 128.0, 0.8, i as f64 * 8.0),
            );
        }
        smudge.end(&mut doc);

        for (idx, _) in doc.active_layer().tiles() {
            assert!(
                (0..4).contains(&idx.x) && (0..4).contains(&idx.y),
                "smudge escaped the canvas at {idx:?}"
            );
        }
    }

    /// Krita Wash (flow vs opacity): a single stroke composites ONCE at the
    /// stroke opacity, so going over the same spot forever cannot push the
    /// laid-down alpha past that opacity. Build-up (stock) reaches full
    /// opacity instead. Same brush, same overlapping samples.
    #[test]
    fn wash_stroke_never_exceeds_its_opacity() {
        let stationary = |wash: bool| {
            let mut doc = Document::new(256, 256);
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            b.set_color_rgb([0.0, 0.0, 0.0]);
            if wash {
                b.set_wash(true, 0.5, mn_core::Blend::Normal);
            }
            doc.begin_op();
            b.begin(&mut doc);
            // Tiny wiggle, same two pixels: distance-driven dab spacing
            // (pen.myb has no dabs_per_second) keeps stamping over the spot.
            for i in 0..200 {
                let x = 128.0 + (i % 2) as f32 * 0.5;
                b.sample(&mut doc, sample(x, 128.0, 1.0, i as f64 * 33.0));
            }
            b.end(&mut doc);
            doc.end_op();
            let t = doc
                .active_layer()
                .tile(TileIdx::of_pixel(128, 128))
                .expect("stroke painted nothing");
            u32::from(t.pixel((128 % 64) as usize, (128 % 64) as usize)[3])
        };
        let buildup = stationary(false);
        let wash = stationary(true);
        let one = 32768u32;
        assert!(
            buildup > one * 90 / 100,
            "build-up should reach ~full opacity, got {buildup}"
        );
        assert!(
            wash <= one * 55 / 100 && wash >= one * 40 / 100,
            "wash stroke should saturate at its 50% stroke opacity, got {wash}"
        );
    }

    /// Erasing through a wash stroke: the buffer records the dab coverage,
    /// the commit subtracts `stroke_opacity x mask` — saturating at the stroke
    /// opacity, never erasing more than that in one stroke.
    #[test]
    fn wash_erase_saturates_at_stroke_opacity() {
        let mut doc = Document::new(256, 256);
        let mut base = MyBrush::load(&preset("pen.myb")).unwrap();
        base.set_color_rgb([0.0, 0.0, 0.0]);
        straight_stroke(&mut base, &mut doc, 128.0, 1.0);
        let px = TileIdx::of_pixel(200, 128);
        let before = doc
            .active_layer()
            .tile(px)
            .unwrap()
            .pixel((200 % 64) as usize, (128 % 64) as usize)[3];

        let mut eraser = MyBrush::load(&preset("pen.myb")).unwrap();
        eraser.set_eraser(true);
        eraser.set_size_multiplier(2.0);
        eraser.set_wash(true, 0.5, mn_core::Blend::Normal);
        straight_stroke(&mut eraser, &mut doc, 128.0, 1.0);
        let after = doc
            .active_layer()
            .tile(px)
            .unwrap()
            .pixel((200 % 64) as usize, (128 % 64) as usize)[3];

        let expected = before as f32 * 0.5;
        assert!(
            (after as f32 - expected).abs() < before as f32 * 0.2,
            "wash erase should remove ~half, {before} -> {after}"
        );
    }

    /// The wash commit honours Krita's per-brush blend mode: multiply over
    /// solid red with blue ink must go dark; normal wash goes blue.
    #[test]
    fn wash_commit_honours_the_blend_mode() {
        let paint_over_red = |blend: mn_core::Blend| {
            let mut doc = Document::new(256, 256);
            let mut base = MyBrush::load(&preset("pen.myb")).unwrap();
            base.set_color_rgb([1.0, 0.0, 0.0]);
            straight_stroke(&mut base, &mut doc, 128.0, 1.0);
            let mut over = MyBrush::load(&preset("pen.myb")).unwrap();
            over.set_color_rgb([0.0, 0.0, 1.0]);
            over.set_wash(true, 1.0, blend);
            straight_stroke(&mut over, &mut doc, 128.0, 1.0);
            let t = doc
                .active_layer()
                .tile(TileIdx::of_pixel(200, 128))
                .unwrap();
            t.pixel((200 % 64) as usize, (128 % 64) as usize)
        };
        let normal = paint_over_red(mn_core::Blend::Normal);
        let multiply = paint_over_red(mn_core::Blend::Multiply);
        assert!(
            u32::from(multiply[2]) < u32::from(normal[2]) * 9 / 10,
            "multiply must kill the blue over red: b normal={} multiply={}",
            normal[2],
            multiply[2]
        );
    }

    /// A wash stroke is still one undoable op (the commit runs inside the
    /// caller's bracket), and turning wash off again reproduces stock pixels.
    #[test]
    fn wash_is_one_undo_and_off_is_stock() {
        let mut doc = Document::new(256, 256);
        let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
        b.set_wash(true, 0.8, mn_core::Blend::Normal);
        doc.begin_op();
        b.begin(&mut doc);
        for i in 0..=30 {
            b.sample(
                &mut doc,
                sample(60.0 + i as f32 * 4.0, 128.0, 1.0, i as f64 * 8.0),
            );
        }
        b.end(&mut doc);
        doc.end_op();
        assert_eq!(doc.undo_len(), 1, "wash stroke must be one op");
        assert!(doc.undo());
        assert_eq!(total_alpha(&doc), 0, "undo left wash ink behind");
        assert!(doc.redo());
        assert!(total_alpha(&doc) > 0);

        // Off is stock: same preset, wash off, must match a fresh load on
        // untouched documents. Both brushes get one PRIOR stroke first —
        // libmypaint carries a little filtered-speed state across `reset()`,
        // so "first stroke ever" and "second stroke" differ slightly in stock
        // behaviour too (true with or without wash).
        let (mut d1, mut d2) = (Document::new(256, 256), Document::new(256, 256));
        let (mut warm1, mut warm2) = (Document::new(256, 256), Document::new(256, 256));
        let mut b2 = MyBrush::load(&preset("pen.myb")).unwrap();
        b.set_wash(false, 1.0, mn_core::Blend::Normal);
        straight_stroke(&mut b2, &mut warm2, 100.0, 1.0);
        straight_stroke(&mut b, &mut warm1, 100.0, 1.0);
        straight_stroke(&mut b, &mut d1, 200.0, 1.0);
        straight_stroke(&mut b2, &mut d2, 200.0, 1.0);
        assert_eq!(total_alpha(&d1), total_alpha(&d2));
    }

    /// Presets carry the wash keys top-level; stock presets stay build-up.
    #[test]
    fn wash_presets_load_with_modes() {
        let dir = std::env::temp_dir().join("mn-brush-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("wash-test.myb");
        std::fs::write(
            &p,
            r#"{"version": 3, "settings": {}, "mn-wash": true,
                "mn-wash-opacity": 0.65, "mn-brush-blend": "multiply"}"#,
        )
        .unwrap();
        let b = MyBrush::load(&p).unwrap();
        assert!(b.wash());
        assert!((b.wash_opacity() - 0.65).abs() < 1e-6);
        assert_eq!(b.wash_blend(), mn_core::Blend::Multiply);
        let pen = MyBrush::load(&preset("pen.myb")).unwrap();
        assert!(!pen.wash() && pen.wash_blend() == mn_core::Blend::Normal);
    }

    /// The curve editor's engine contract: mapping points round-trip through
    /// the C setters for ANY setting x input, and an edited pressure→size
    /// curve visibly changes the stroke.
    #[test]
    fn mapping_roundtrips_through_the_ffi() {
        let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
        let (sid, pid) = (
            settings::setting::RADIUS_LOGARITHMIC,
            settings::input::PRESSURE,
        );
        let original = b.mapping(sid, pid);
        assert_eq!(original.len(), b.mapping_n(sid, pid) as usize);

        let edited = vec![(0.0f32, -1.0f32), (0.4, -0.2), (1.0, 0.3)];
        b.set_mapping(sid, pid, &edited);
        let read = b.mapping(sid, pid);
        assert_eq!(read.len(), edited.len());
        for (r, e) in read.iter().zip(&edited) {
            assert!(
                (r.0 - e.0).abs() < 1e-5 && (r.1 - e.1).abs() < 1e-5,
                "{r:?} vs {e:?}"
            );
        }

        // Non-pressure sensors work through the same seam.
        let (oid, vid) = (settings::setting::OPAQUE_MULTIPLY, settings::input::SPEED1);
        assert!(b.mapping(oid, vid).is_empty());
        b.set_mapping(oid, vid, &[(0.0, 0.2), (4.0, 1.0)]);
        assert_eq!(b.mapping(oid, vid).len(), 2);
    }

    /// A curve that halves the dab at every pressure must halve the stroke
    /// band — the editor edits real dynamics, not display state.
    #[test]
    fn editing_the_pressure_curve_changes_the_stroke() {
        let width = |curve: Option<&[(f32, f32)]>| {
            let mut doc = Document::new(512, 512);
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            if let Some(pts) = curve {
                b.set_mapping(
                    settings::setting::RADIUS_LOGARITHMIC,
                    settings::input::PRESSURE,
                    pts,
                );
            }
            straight_stroke(&mut b, &mut doc, 256.0, 1.0);
            let (_, y0, _, y1) = painted_bbox(&doc).unwrap();
            y1 - y0
        };
        let full = width(Some(&[(0.0, 0.0), (1.0, 0.0)]));
        let half = width(Some(&[(0.0, 0.5f32.ln()), (1.0, 0.5f32.ln())]));
        assert!(
            full > half * 3 / 2,
            "half-radius curve must narrow the stroke: full={full} half={half}"
        );
    }

    /// PATCHES.md #10 amendment 2: DAB-ANCHORED stamps. A half-ink mask in
    /// dab mode inks each dab's own left half — so a stroke carries ink past
    /// its left end (trailing dab halves) and stops dry before its right
    /// end. A 180° stamp angle mirrors that — genuinely, because the stamp
    /// angle is its own UNFOLDED channel (the elliptical angle folds mod
    /// 180 and would render 0 and 180 identically). Canvas-anchored grain
    /// can do neither: its pattern is fixed to the page, not the dab.
    #[test]
    fn dab_anchored_stamps_rotate_with_the_dab() {
        let half_ink = || {
            let size = 64usize;
            let mut data = vec![0u8; size * size];
            for y in 0..size {
                for x in 0..size / 2 {
                    data[y * size + x] = 255; // left half ink, right half dry
                }
            }
            Arc::new(TextureMask {
                name: "half".into(),
                size: size as u32,
                data: Arc::new(data),
            })
        };
        let stroke_at = |angle: f32| -> Document {
            let mut doc = Document::new(512, 512);
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            b.set_hard_dab(true);
            b.set_texture(Some(half_ink()));
            b.set_texture_anchor_dab(true);
            b.set_base_value(
                settings::setting_id("radius_logarithmic").unwrap(),
                16f32.ln(),
            );
            b.set_texture_angle_deg(angle);
            straight_stroke(&mut b, &mut doc, 256.0, 1.0);
            doc
        };
        // Alpha in a window just past each end of the stroke's dab centres
        // (centres run x = 100..400, radius 16).
        let window = |doc: &Document, x0: i32, x1: i32| -> u64 {
            let mut sum = 0u64;
            for x in x0..x1 {
                for y in 248..264 {
                    let idx = TileIdx::of_pixel(x, y);
                    if let Some(t) = doc.active_layer().tile(idx) {
                        sum += u64::from(
                            t.pixel((x - idx.origin().0) as usize, (y - idx.origin().1) as usize)
                                [3],
                        );
                    }
                }
            }
            sum
        };
        let d0 = stroke_at(0.0);
        assert!(
            window(&d0, 86, 98) > 0,
            "angle 0: trailing (left) dab halves ink past the first centre"
        );
        assert_eq!(
            window(&d0, 403, 415),
            0,
            "angle 0: the dry right halves leave the far end blank"
        );
        let d180 = stroke_at(180.0);
        assert_eq!(
            window(&d180, 86, 98),
            0,
            "angle 180: the stamp mirrored — left end dry"
        );
        assert!(
            window(&d180, 403, 415) > 0,
            "angle 180: ink now runs past the right end"
        );

        // Direction mode: the stamp turns WITH the stroke. The same brush
        // drawn rightward vs leftward flips which end carries the trailing
        // ink — the calligraphy-nib behaviour a folded angle cannot give.
        let directional = |leftward: bool| -> Document {
            let mut doc = Document::new(512, 512);
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            b.set_hard_dab(true);
            b.set_texture(Some(half_ink()));
            b.set_texture_anchor_dab(true);
            b.set_texture_rotate_direction(true);
            b.set_base_value(
                settings::setting_id("radius_logarithmic").unwrap(),
                16f32.ln(),
            );
            b.begin(&mut doc);
            for i in 0..=60 {
                let x = if leftward {
                    400.0 - i as f32 * 5.0
                } else {
                    100.0 + i as f32 * 5.0
                };
                b.sample(&mut doc, sample(x, 256.0, 1.0, i as f64 * 8.0));
            }
            b.end(&mut doc);
            doc
        };
        let right = directional(false); // direction ≈ 0°: trailing = left
        let left = directional(true); // direction ≈ 180°: trailing = right
        assert!(window(&right, 86, 98) > 0 && window(&right, 403, 415) == 0);
        assert!(window(&left, 86, 98) == 0 && window(&left, 403, 415) > 0);
    }

    /// Krita texture tips (vendor/PATCHES.md #10): the mask multiplies the
    /// dab profile, so a half-black mask halves the inked band and leaves
    /// dry stripes INSIDE the stroke — visible structure, not just less ink.
    #[test]
    fn texture_tips_modulate_the_dab() {
        let stripes = || {
            let size = 64;
            let mut data = vec![0u8; size * size];
            for y in 0..size {
                for x in 0..size / 2 {
                    data[y * size + x] = 255; // left half ink, right half dry
                }
            }
            Arc::new(TextureMask {
                name: "test-stripes".into(),
                size: size as u32,
                data: Arc::new(data),
            })
        };
        let (mut plain_doc, mut tex_doc) = (Document::new(512, 512), Document::new(512, 512));
        let mut plain = MyBrush::load(&preset("pen.myb")).unwrap();
        let mut tex = MyBrush::load(&preset("pen.myb")).unwrap();
        plain.set_hard_dab(true);
        tex.set_hard_dab(true);
        tex.set_texture(Some(stripes()));
        straight_stroke(&mut plain, &mut plain_doc, 256.0, 1.0);
        straight_stroke(&mut tex, &mut tex_doc, 256.0, 1.0);

        assert!(
            total_alpha(&tex_doc) < total_alpha(&plain_doc) * 3 / 4,
            "half-dry mask must cut the ink well below plain"
        );

        // Structure, not just level: inside the plain stroke's band there
        // must be inked AND dry columns in the textured one (the stripe
        // pattern rides across the whole stroke).
        let (x0, _, x1, _) = painted_bbox(&tex_doc).unwrap();
        let col_alpha = |doc: &Document, x: i32| -> u64 {
            let mut sum = 0;
            for y in 200..312 {
                let idx = TileIdx::of_pixel(x, y);
                if let Some(t) = doc.active_layer().tile(idx) {
                    sum += u64::from(
                        t.pixel((x - idx.origin().0) as usize, (y - idx.origin().1) as usize)[3],
                    );
                }
            }
            sum
        };
        let cols: Vec<u64> = (x0..=x1).map(|x| col_alpha(&tex_doc, x)).collect();
        assert!(
            cols.iter().any(|&c| c > 0) && cols.iter().any(|&c| c == 0),
            "expected inked and dry stripes inside the band, got {cols:?}"
        );

        // Off is stock: texture removed, the very next stroke must match a
        // fresh load's SECOND stroke (both warmed once — libmypaint carries
        // speed-filter state across reset; see the wash twin of this).
        let (mut again, mut warm, mut fresh_doc) = (
            Document::new(512, 512),
            Document::new(512, 512),
            Document::new(512, 512),
        );
        let mut fresh = MyBrush::load(&preset("pen.myb")).unwrap();
        fresh.set_hard_dab(true);
        tex.set_texture(None);
        straight_stroke(&mut tex, &mut warm, 256.0, 1.0);
        straight_stroke(&mut fresh, &mut warm, 256.0, 1.0);
        straight_stroke(&mut tex, &mut again, 256.0, 1.0);
        straight_stroke(&mut fresh, &mut fresh_doc, 256.0, 1.0);
        assert_eq!(total_alpha(&again), total_alpha(&fresh_doc));
    }

    /// A preset carrying "mn-texture" resolves the mask from textures/ beside
    /// the preset groups and applies the crawl step; the shipped masks load.
    #[test]
    fn texture_presets_load_their_masks() {
        // Ship-shape structure: <tmp>/group/x.myb + <tmp>/textures/grain.png.
        let tmp = std::env::temp_dir().join("mn-brush-texture-test");
        let group = tmp.join("group");
        std::fs::create_dir_all(&group).unwrap();
        std::fs::create_dir_all(tmp.join("textures")).unwrap();
        let src =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brushes/textures/grain.png");
        std::fs::copy(&src, tmp.join("textures/grain.png")).unwrap();
        let p = group.join("textured.myb");
        std::fs::write(
            &p,
            r#"{"version": 3, "settings": {}, "mn-texture": "grain",
                "mn-texture-scroll": 2.5}"#,
        )
        .unwrap();

        let b = MyBrush::load(&p).unwrap();
        let mask = b.texture().expect("grain.png must resolve");
        assert_eq!(mask.name, "grain");
        assert_eq!(mask.size, 128);
        assert!((b.texture_scroll() - 2.5).abs() < 1e-6);
        // The shipped set loads as masks (square, in range).
        for name in ["grain", "streaks", "dots"] {
            let src = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/brushes/textures")
                .join(format!("{name}.png"));
            let img = image::open(&src).unwrap().to_luma8();
            assert_eq!(img.dimensions(), (128, 128), "{name}");
        }
        // A name that does not resolve keeps the brush usable, untextured.
        let p2 = group.join("missing.myb");
        std::fs::write(
            &p2,
            r#"{"version": 3, "settings": {}, "mn-texture": "nope"}"#,
        )
        .unwrap();
        assert!(MyBrush::load(&p2).unwrap().texture().is_none());
    }

    /// The crawl actually moves: with a big per-dab step the stroke must
    /// differ from the static pattern (the mask drifts across the dabs).
    #[test]
    fn texture_scroll_moves_the_pattern() {
        let stripes = || {
            let size = 64;
            let mut data = vec![0u8; size * size];
            for y in 0..size {
                for x in 0..size / 2 {
                    data[y * size + x] = 255;
                }
            }
            Arc::new(TextureMask {
                name: "s".into(),
                size: size as u32,
                data: Arc::new(data),
            })
        };
        let stroke = |scroll: f32| -> u64 {
            let mut doc = Document::new(512, 512);
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            b.set_hard_dab(true);
            b.set_texture(Some(stripes()));
            b.set_texture_scroll(scroll);
            straight_stroke(&mut b, &mut doc, 256.0, 1.0);
            total_alpha(&doc)
        };
        assert_ne!(stroke(0.0), stroke(8.0), "pattern must crawl with the step");
    }

    /// Krita SKETCH engine: looping strokes link back to their own history,
    /// so the web paints visibly more ink than the plain path.
    #[test]
    fn sketch_links_add_ink() {
        let loops = |sketch: Option<SketchParams>| -> u64 {
            let mut doc = Document::new(512, 512);
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            b.set_sketch(sketch);
            b.begin(&mut doc);
            // Two loops of a circle (radius 40): plenty of near-history.
            for i in 0..120 {
                let a = i as f32 / 60.0 * std::f32::consts::TAU;
                b.sample(
                    &mut doc,
                    sample(
                        256.0 + a.cos() * 40.0,
                        256.0 + a.sin() * 40.0,
                        1.0,
                        i as f64 * 16.0,
                    ),
                );
            }
            b.end(&mut doc);
            total_alpha(&doc)
        };
        let plain = loops(None);
        let sketched = loops(Some(SketchParams {
            distance: 60.0,
            density: 1.0,
        }));
        assert!(
            sketched > plain + plain / 4,
            "sketch must add linking ink: plain={plain} sketched={sketched}"
        );

        // Off is stock: removing the mode leaves a plain-painting brush.
        let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
        b.set_sketch(Some(SketchParams {
            distance: 60.0,
            density: 1.0,
        }));
        b.set_sketch(None);
        let mut doc = Document::new(512, 512);
        straight_stroke(&mut b, &mut doc, 256.0, 1.0);
        let mut fresh = MyBrush::load(&preset("pen.myb")).unwrap();
        let mut again = Document::new(512, 512);
        straight_stroke(&mut fresh, &mut again, 256.0, 1.0);
        assert_eq!(total_alpha(&doc), total_alpha(&again));
    }

    /// Presets carry the sketch keys top-level, stock presets stay plain.
    #[test]
    fn sketch_presets_load_with_modes() {
        let dir = std::env::temp_dir().join("mn-brush-sketch-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("sketch.myb");
        std::fs::write(
            &p,
            r#"{"version": 3, "settings": {}, "mn-sketch": true,
                "mn-sketch-distance": 55, "mn-sketch-density": 0.4}"#,
        )
        .unwrap();
        let b = MyBrush::load(&p).unwrap();
        let s = b.sketch().expect("mn-sketch must load");
        assert!((s.distance - 55.0).abs() < 1e-6);
        assert!((s.density - 0.4).abs() < 1e-6);
        assert!(
            MyBrush::load(&preset("pen.myb"))
                .unwrap()
                .sketch()
                .is_none()
        );
    }

    /// GPU-dabs P0 (docs/design/GPU-DABS.md, PATCHES.md #11): TAP mode
    /// records every dab the CPU rasterizes — same pixels as stock, a full
    /// dab list, and a touched-tile set that covers everything painted.
    #[test]
    fn dab_record_tap_changes_nothing_and_captures_everything() {
        let stroke = |record: RecordMode| -> (u64, DabRecord) {
            let mut doc = Document::new(512, 512);
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            b.set_dab_recording(record);
            straight_stroke(&mut b, &mut doc, 256.0, 1.0);
            let rec = b.take_dab_record();
            (total_alpha(&doc), rec)
        };
        let (plain, _) = stroke(RecordMode::Off);
        let (tapped, rec) = stroke(RecordMode::Tap);
        assert_eq!(plain, tapped, "tap mode must not change pixels");
        assert!(!rec.dabs.is_empty(), "no dabs recorded");
        assert!(
            rec.dabs.iter().all(|d| d.radius >= 0.1),
            "early-out dabs leaked in"
        );

        // The touched-tile set must cover every tile that actually got ink.
        let doc = {
            let mut d = Document::new(512, 512);
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            straight_stroke(&mut b, &mut d, 256.0, 1.0);
            d
        };
        let inked: std::collections::BTreeSet<(i32, i32)> = doc
            .active_layer()
            .tiles()
            .filter(|(_, t)| !t.is_blank())
            .map(|(i, _)| (i.x, i.y))
            .collect();
        assert!(
            inked
                .iter()
                .all(|t| rec.tiles.contains(&TileIdx::new(t.0, t.1))),
            "record misses painted tiles: inked {inked:?} vs recorded {:?}",
            rec.tiles
        );

        // BYPASS records but rasterizes nothing — the P1 seam, pinned.
        let (bare, rec2) = stroke(RecordMode::Bypass);
        assert_eq!(bare, 0, "bypass must not paint on the CPU path");
        assert!(!rec2.dabs.is_empty(), "bypass must still record");
    }

    /// View-zoom compensation (vendor patch #12): the same SCREEN motion at
    /// different zooms must drive the speed inputs identically, so the dab
    /// radii in document space agree. Owner report 2026-08-17: strokes drawn
    /// zoomed-out came out jagged/bumpy when zoomed back in — through the
    /// plain legacy entry the C saw 1/zoom times the document velocity and
    /// every speed-mapped dynamic (here a hard velocity→Size, the owner's
    /// milli-pen shape) fired as if the pen moved that much faster.
    #[test]
    fn speed_inputs_are_view_zoom_compensated() {
        // One brush, one motion script; two runs at zoom 1.0 and 0.25 with
        // positions scaled into document space (exactly what
        // `App::push_batch`'s `viewport.to_canvas` does live).
        let run = |zoom: f32| -> Vec<f32> {
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            // Hard velocity→Size: log-radius -3 over the SPEED1 input range,
            // so a 4× velocity error is a ~40% radius error — far past the
            // tolerance.
            b.set_mapping(
                settings::setting::RADIUS_LOGARITHMIC,
                settings::input::SPEED1,
                &[(0.0, 0.0), (8.0, -3.0)],
            );
            b.set_dab_recording(RecordMode::Tap);
            b.set_view(zoom, 0.0, false);
            let mut doc = Document::default();
            b.begin(&mut doc);
            // Same screen cadence: 3 px per 8 ms at 125 Hz, 150 samples.
            for i in 0..150 {
                let t = i as f32 / 149.0;
                let (sx, sy) = (100.0 + t * 450.0, 200.0 + (t * 7.0).sin() * 40.0);
                b.sample(&mut doc, sample(sx / zoom, sy / zoom, 0.8, i as f64 * 8.0));
            }
            b.end(&mut doc);
            b.take_dab_record().dabs.iter().map(|d| d.radius).collect()
        };

        let wide = run(1.0);
        let zoomed = run(0.25);
        assert!(wide.len() > 30, "too few dabs to compare");
        // Skip the ramp-in while the speed filters settle, then compare mean
        // radius: view-compensated speeds mean equal radii in doc px.
        let mean = |v: &[f32]| v.iter().skip(20).sum::<f32>() / (v.len() - 20) as f32;
        let (m1, m2) = (mean(&wide), mean(&zoomed));
        assert!(
            (m1 - m2).abs() / m1 < 0.05,
            "zoomed-out stroke radii drifted: 100% mean {m1} vs 25% mean {m2} \
             (speed inputs not view-compensated?)"
        );
    }

    /// Patch #12's ROTATION half (auditor review of f741b26, 2026-08-17):
    /// the C applies `DEGREES()` to the viewrotation argument itself — it
    /// wants RADIANS (upstream's "@viewrotation: in degrees" docstring is
    /// a doc bug; MyPaint's own caller passes `tdw.rotation`). Same-SCREEN
    /// motion at two view rotations must feed the engine the SAME
    /// direction input. Observed through a steep DIRECTION→Size curve
    /// because the recorded dab ANGLE alone cannot pin the SIGN: with a 1:1
    /// direction→angle mapping a wrong sign cancels in
    /// setting-minus-viewrotation (the dab still tracks the canvas), while
    /// the DIRECTION input — what the hand feels — is off by 2×rotation.
    /// Our viewport's screen = R(rotate_rad)·canvas makes the raw
    /// +rotate_rad the correct argument; this test discriminates both the
    /// old `.to_degrees()` unit bug and a negated sign.
    #[test]
    fn direction_inputs_are_view_rotation_compensated() {
        let run = |rot: f32| -> Vec<f32> {
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            // Steep DIRECTION→Size over the input's 0..180 wrap: the steady
            // screen angle ≈0° maps to ≈0; a −2×rot error (wrong sign)
            // reads ≈120° and collapses the dabs ~5×.
            b.set_mapping(
                settings::setting::RADIUS_LOGARITHMIC,
                settings::input::DIRECTION,
                &[(0.0, 0.0), (180.0, -2.5)],
            );
            b.set_dab_recording(RecordMode::Tap);
            b.set_view(1.0, rot, false);
            let mut doc = Document::default();
            b.begin(&mut doc);
            let (s, c) = rot.sin_cos();
            for i in 0..150 {
                let t = i as f32 / 149.0;
                // The SAME screen-space path both runs (+x drift with a
                // slight wobble so the direction filter has work), fed as
                // document coordinates through the viewport's own
                // to_canvas rotation: canvas = R(-rot)·screen.
                let (dx, dy) = (t * 450.0, (t * 7.0).sin() * 40.0);
                b.sample(
                    &mut doc,
                    sample(
                        100.0 + c * dx + s * dy,
                        200.0 - s * dx + c * dy,
                        0.8,
                        i as f64 * 8.0,
                    ),
                );
            }
            b.end(&mut doc);
            b.take_dab_record().dabs.iter().map(|d| d.radius).collect()
        };

        let flat = run(0.0);
        let rotated = run(std::f32::consts::FRAC_PI_6); // 30°
        assert!(flat.len() > 30, "too few dabs to compare");
        let mean = |v: &[f32]| v.iter().skip(20).sum::<f32>() / (v.len() - 20) as f32;
        let (m1, m2) = (mean(&flat), mean(&rotated));
        assert!(
            (m1 - m2).abs() / m1 < 0.05,
            "rotated-view stroke radii drifted: 0° mean {m1} vs 30° mean {m2} \
             (direction inputs not view-rotation compensated — units or sign?)"
        );
    }

    /// Patch #12's flip extension (auditor item b, round 34): under a
    /// horizontally mirrored view the SAME screen-space motion must map to
    /// the SAME direction input — the C negates the direction vectors' DX
    /// (the 180−θ reflection), then the raw +viewrotation arithmetic carries
    /// the rest. Shaped exactly like the rotation test above: the DIRECTION
    /// input cannot be read directly, so a steep DIRECTION→Size curve makes
    /// the RADIUS carry the burden. Without the flip compensation the
    /// mirrored doc motion reads ≈180° through the curve and the dabs
    /// collapse.
    #[test]
    fn direction_inputs_are_view_flip_compensated() {
        let run = |flip: bool| -> Vec<f32> {
            let mut b = MyBrush::load(&preset("pen.myb")).unwrap();
            b.set_mapping(
                settings::setting::RADIUS_LOGARITHMIC,
                settings::input::DIRECTION,
                &[(0.0, 0.0), (180.0, -2.5)],
            );
            b.set_dab_recording(RecordMode::Tap);
            b.set_view(1.0, 0.0, flip);
            let mut doc = Document::default();
            b.begin(&mut doc);
            for i in 0..150 {
                let t = i as f32 / 149.0;
                // The SAME screen path both runs, at a steady OFF-AXIS angle
                // (45° + a slight perpendicular wobble so the direction
                // filter has work): DIRECTION is mod-180, so a path on the
                // 0/180 axis would be mirror-invariant BY ACCIDENT and the
                // test could not discriminate. The flipped run's doc
                // coordinates go through the viewport's own mirrored
                // to_canvas at rotation 0: doc = S·screen (x negates).
                let a = t * 320.0;
                let w = (t * 7.0).sin() * 30.0;
                let (sx, sy) = (a - w, a + w); // 45° + wobble
                b.sample(
                    &mut doc,
                    sample(
                        if flip { 100.0 - sx } else { 100.0 + sx },
                        200.0 + sy,
                        0.8,
                        i as f64 * 8.0,
                    ),
                );
            }
            b.end(&mut doc);
            b.take_dab_record().dabs.iter().map(|d| d.radius).collect()
        };

        let normal = run(false);
        let flipped = run(true);
        assert!(normal.len() > 30, "too few dabs to compare");
        let mean = |v: &[f32]| v.iter().skip(20).sum::<f32>() / (v.len() - 20) as f32;
        let (m1, m2) = (mean(&normal), mean(&flipped));
        assert!(
            (m1 - m2).abs() / m1 < 0.05,
            "flipped-view stroke radii drifted: normal mean {m1} vs flipped mean {m2} \
             (direction inputs not view-flip compensated — the mirror must negate \
             the direction vectors' DX before the +viewrotation arithmetic)"
        );
    }
}
