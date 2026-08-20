//! Headless render-path check: synthetic stroke -> tiles -> GPU -> PNG.
//!
//! `cargo run -p mn-gpu --example offscreen [--warp]`
//! Writes `target/test-out/offscreen.png` and asserts the result is neither
//! blank paper nor all backdrop. Deliberately an example, not a `#[test]`, so
//! `cargo test` stays GPU-free (docs/ARCHITECTURE.md: correctness never needs a
//! device, only *feel* does).

use mn_brush::SimpleDab;
use mn_core::{Document, PenSample, StrokeSink};
use mn_gpu::{GpuConfig, Renderer};

fn main() {
    let force_fallback = std::env::args().any(|a| a == "--warp");

    let mut doc = Document::new(512, 512);
    let mut brush = SimpleDab::new();
    brush.begin(&mut doc);
    for i in 0..=60 {
        let t = i as f32 / 60.0;
        brush.sample(
            &mut doc,
            PenSample {
                x: 60.0 + t * 392.0,
                y: 256.0 + (t * std::f32::consts::TAU).sin() * 120.0,
                pressure: 0.2 + 0.8 * t,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: i as f64 * 8.0,
            },
        );
    }
    brush.end(&mut doc);
    println!("[test] painted {} tiles", doc.active_layer().tile_count());

    let mut r = Renderer::new_headless(GpuConfig {
        force_fallback,
        no_vsync: false,
    })
    .expect("headless renderer");
    println!("[test] adapter: {}", r.adapter_line());

    let img = r.render_offscreen(&doc, 512, 512);

    let mut ink = 0usize;
    let mut paper = 0usize;
    for p in img.pixels() {
        let [r, g, b, _] = p.0;
        if r > 240 && g > 240 && b > 240 {
            paper += 1;
        } else if r < 128 && g < 128 && b < 128 && r.abs_diff(41) > 8 {
            ink += 1;
        }
    }
    println!("[test] paper px {paper}, ink px {ink}");

    let dir = std::path::Path::new("target/test-out");
    std::fs::create_dir_all(dir).ok();
    let path = dir.join("offscreen.png");
    img.save(&path).expect("write png");
    println!("[test] wrote {}", path.display());

    assert!(paper > 10_000, "canvas did not render as paper");
    assert!(ink > 500, "stroke did not reach the GPU");
    println!("[test] OK");
}
