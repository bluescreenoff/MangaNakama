//! The dab-path benchmark (P3 re-flip criterion, DECISIONS 8.9): measured
//! ms/stroke per engine class, CPU path vs GPU path, through the REAL app
//! stroke machinery (begin → push_batch → end, including the GPU flush,
//! readback, wash commit, and the smudge oracle's per-sample dispatch —
//! the costs that actually decide the flip, not an isolated kernel).
//!
//! `--bench-dabs` runs it in the shipped exe and writes the table to
//! `manganakama-bench.txt` beside the exe (the owner runs it on HIS
//! hardware — the criterion's whole point). The suite runs a 1-rep smoke
//! that checks STRUCTURE, never timings.

use std::time::Instant;

use mn_gpu::{GpuConfig, Renderer};

use crate::app::{App, Engine, EngineKind};

/// The engine classes the flip decides between — the everyday pen, the
/// strategic big-soft-tip case (dab cost scales with tip AREA), the
/// texture tip, the wash commit, and the smudge oracle path.
const CLASSES: &[(&str, &str)] = &[
    ("plain g-pen", "csp/real-g-pen.myb"),
    ("soft airbrush", "csp/soft-airbrush.myb"),
    ("textured pencil", "krita/textured-pencil.myb"),
    ("marker wash", "krita/marker-wash.myb"),
    ("blending knife (smudge)", "classic/blending_knife.myb"),
];

const SAMPLES: usize = 60;

fn preset_path(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/brushes")
        .join(rel)
}

fn stroke(app: &mut App, x0: f32) {
    app.begin_stroke(crate::app::PointerKind::Mouse);
    let batch: Vec<mn_core::PenSample> = (0..SAMPLES)
        .map(|i| mn_core::PenSample {
            x: x0 + i as f32 * 6.0,
            y: 1024.0,
            pressure: 0.8,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();
}

/// One timed pass: `reps` strokes per class, alternating x offsets,
/// returning per-class average ms. `gpu` selects the path.
fn timed_pass(app: &mut App, reps: usize, gpu: bool) -> Result<Vec<f64>, String> {
    app.gpu_dabs = gpu;
    let mut out = Vec::with_capacity(CLASSES.len());
    for (ci, (_, rel)) in CLASSES.iter().enumerate() {
        let b =
            mn_brush::MyBrush::load(&preset_path(rel)).map_err(|e| format!("preset {rel}: {e}"))?;
        *app.engine_mut() = Engine::new(EngineKind::My(Box::new(b)));
        let layer = app.doc.add_layer("bench");
        app.doc.set_active(layer);
        let mut total = 0f64;
        for r in 0..reps {
            let t0 = Instant::now();
            stroke(app, 100.0 + r as f32 * 40.0 + ci as f32 * 8.0);
            total += t0.elapsed().as_secs_f64() * 1000.0;
        }
        if gpu && !app.dab_path_last.starts_with("gpu") {
            return Err(format!(
                "class {rel} silently left the GPU path ({})",
                app.dab_path_last
            ));
        }
        out.push(total / reps.max(1) as f64);
    }
    Ok(out)
}

/// Run the benchmark and return the formatted table. One App for both
/// passes (same adapter, same document scale); CPU first so the GPU pass
/// inherits warm caches — the conservative direction for the GPU.
pub fn bench_dabs(cfg: GpuConfig, reps: usize) -> Result<String, String> {
    let renderer = Renderer::new_headless(cfg).map_err(|e| e.to_string())?;
    let mut app = App::new(renderer, (600, 400), 1.0);
    let cpu = timed_pass(&mut app, reps, false)?;
    let gpu = timed_pass(&mut app, reps, true)?;

    let mut table = String::new();
    table.push_str("MangaNakama dab-path benchmark — ms per stroke (avg)\n");
    table.push_str(&format!(
        "engine classes: {} | samples/stroke: {SAMPLES} | reps: {reps}\n",
        CLASSES.len()
    ));
    table.push_str("class                        cpu-ms    gpu-ms   gpu/cpu\n");
    table.push_str(&"-".repeat(58));
    table.push('\n');
    for ((name, _), (c, g)) in CLASSES.iter().zip(cpu.iter().zip(&gpu)) {
        let ratio = if *c > 0.0 { g / c } else { f64::INFINITY };
        table.push_str(&format!(
            "{name:<26} {c:>6.1}    {g:>6.1}   {ratio:>6.2}×\n"
        ));
    }
    Ok(table)
}

/// Write the table beside the exe (like ui.txt) so a run on the owner's
/// machine leaves the numbers behind.
pub fn bench_write(table: &str) -> Option<std::path::PathBuf> {
    let p = std::env::current_exe()
        .ok()?
        .parent()?
        .join("manganakama-bench.txt");
    std::fs::write(&p, table).ok()?;
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Structure smoke: every class produces a row, the GPU path really
    /// routed GPU for every class, both columns parse positive. Timings
    /// themselves are NEVER asserted — they are hardware properties, not
    /// correctness properties (the re-flip criterion reads them, humans
    /// decide).
    #[test]
    fn bench_dabs_produces_a_complete_table() {
        let cfg = GpuConfig {
            force_fallback: std::env::var("MN_WARP").is_ok(),
            no_vsync: false,
        };
        let table = match bench_dabs(cfg, 1) {
            Ok(t) => t,
            Err(e) => {
                // Distinguish "no adapter at all" (skip) from a real bench
                // failure (fail loudly — the table must never rot).
                if Renderer::new_headless(cfg).is_err() {
                    println!("[test] SKIP: no usable adapter");
                } else {
                    panic!("bench_dabs failed on a working adapter: {e}");
                }
                return;
            }
        };
        println!("{table}");
        for (name, _) in CLASSES {
            assert!(table.contains(name), "the {name} row is missing");
        }
        // Both timing columns exist and are positive: parse the data rows.
        for line in table.lines().skip(4) {
            if line.starts_with('-') || line.contains("class") || line.contains("engine") {
                continue;
            }
            let nums: Vec<f64> = line
                .split_whitespace()
                .filter_map(|t| t.trim_end_matches(['×', 'x']).parse().ok())
                .collect();
            assert!(
                nums.len() >= 2 && nums[0] > 0.0 && nums[1] > 0.0,
                "row lacks positive timings: {line}"
            );
        }
    }
}
