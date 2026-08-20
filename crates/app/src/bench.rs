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

// ---------------------------------------------------------------------------
// The measured auto-default (ROADMAP: "the GPU path should not become the
// default on anyone's machine without a measured number").
// ---------------------------------------------------------------------------

/// One measured decision for one adapter.
#[derive(Debug, Clone, PartialEq)]
pub struct DabVerdict {
    /// `Renderer::adapter_line()` — name, backend, type, driver. A driver
    /// update or a GPU swap changes it, which is exactly when the number
    /// must be re-measured.
    pub fingerprint: String,
    pub on: bool,
    /// The ratios, one line, for the log and the verdict file's own record.
    pub summary: String,
}

/// The criterion: GPU becomes the default only when the strategic class —
/// the soft airbrush, where dab cost scales with tip AREA — is clearly
/// faster (< 0.9×), and NO class regresses badly (all < 1.3×). Ratios are
/// gpu/cpu, aligned with [`CLASSES`]. Deliberately conservative: a wash
/// that ties and an airbrush that wins flips ON; a knife that pays 1.5×
/// vetoes even a 2× airbrush win.
pub fn decide(ratios: &[f64]) -> bool {
    ratios.len() == CLASSES.len()
        && ratios.iter().all(|r| r.is_finite() && *r < 1.3)
        && ratios[1] < 0.9
}

/// Measure and decide, on the adapter `cfg` selects. `reps` low (3) — this
/// runs once per adapter, in a child process, not in anyone's way.
pub fn quick_verdict(cfg: GpuConfig, reps: usize) -> Result<DabVerdict, String> {
    let renderer = Renderer::new_headless(cfg).map_err(|e| e.to_string())?;
    let fingerprint = renderer.adapter_line();
    let mut app = App::new(renderer, (600, 400), 1.0);
    let cpu = timed_pass(&mut app, reps, false)?;
    let gpu = timed_pass(&mut app, reps, true)?;
    let ratios: Vec<f64> = cpu
        .iter()
        .zip(&gpu)
        .map(|(c, g)| if *c > 0.0 { g / c } else { f64::INFINITY })
        .collect();
    let on = decide(&ratios);
    let summary = CLASSES
        .iter()
        .zip(&ratios)
        .map(|((name, _), r)| format!("{name} {r:.2}x"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(DabVerdict {
        fingerprint,
        on,
        summary,
    })
}

/// `gpu-verdict.txt` beside the exe: line 1 `fingerprint`, line 2 `on`/`off`,
/// line 3 the summary (a record for humans; never parsed).
pub fn verdict_path() -> Option<std::path::PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("gpu-verdict.txt"))
}

pub fn store_verdict(v: &DabVerdict) {
    if let Some(p) = verdict_path() {
        store_verdict_at(&p, v);
    }
}

fn store_verdict_at(p: &std::path::Path, v: &DabVerdict) {
    let _ = std::fs::write(
        p,
        format!(
            "{}\n{}\n{}\n",
            v.fingerprint,
            if v.on { "on" } else { "off" },
            v.summary
        ),
    );
}

/// The stored `(fingerprint, on)`, if any. A malformed file reads as absent
/// (the next launch re-measures — a corrupt verdict must never decide).
pub fn load_verdict() -> Option<(String, bool)> {
    load_verdict_at(&verdict_path()?)
}

fn load_verdict_at(p: &std::path::Path) -> Option<(String, bool)> {
    let text = std::fs::read_to_string(p).ok()?;
    let mut lines = text.lines();
    let fp = lines.next()?.trim().to_string();
    let on = match lines.next()?.trim() {
        "on" => true,
        "off" => false,
        _ => return None,
    };
    if fp.is_empty() { None } else { Some((fp, on)) }
}

/// The startup resolution, pure so it is testable. Returns
/// `(gpu_dabs_on, spawn_measurement)`.
///
/// - An EXPLICIT choice (the `--gpu-dabs` flag, or a `gpu_dabs=` key the
///   user's ui.txt actually carries) always wins and never re-measures.
/// - Otherwise a stored verdict for THIS adapter applies.
/// - Otherwise: off for now, and the measurement child should run so the
///   NEXT launch has its number.
pub fn resolve_auto(
    explicit: Option<bool>,
    verdict: Option<(String, bool)>,
    fingerprint: &str,
) -> (bool, bool) {
    if let Some(on) = explicit {
        return (on, false);
    }
    match verdict {
        Some((fp, on)) if fp == fingerprint => (on, false),
        _ => (false, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The criterion, pinned: the airbrush must clearly win, nothing may
    /// badly lose, and malformed input never flips ON.
    #[test]
    fn the_flip_criterion_is_conservative()
    {
        let n = CLASSES.len();
        let mut wins = vec![0.8; n];
        assert!(decide(&wins), "a clean sweep flips on");
        wins[1] = 0.95; // the strategic class merely ties
        assert!(!decide(&wins), "an airbrush that only ties does not flip");
        wins[1] = 0.5;
        wins[4] = 1.5; // one class regresses badly
        assert!(!decide(&wins), "a bad regression vetoes a big win");
        assert!(!decide(&vec![0.5; n - 1]), "wrong arity never flips");
        let mut nan = vec![0.5; n];
        nan[0] = f64::NAN;
        assert!(!decide(&nan), "a NaN ratio never flips");
    }

    /// The startup resolution: explicit beats verdict beats measurement.
    #[test]
    fn resolution_order_explicit_verdict_measure() {
        let fp = "Some GPU | Vulkan | driver 1.2";
        let stored = Some((fp.to_string(), true));
        // Explicit user choice wins, never re-measures.
        assert_eq!(resolve_auto(Some(false), stored.clone(), fp), (false, false));
        assert_eq!(resolve_auto(Some(true), None, fp), (true, false));
        // No explicit choice: this adapter's verdict applies.
        assert_eq!(resolve_auto(None, stored, fp), (true, false));
        assert_eq!(
            resolve_auto(None, Some((fp.into(), false)), fp),
            (false, false)
        );
        // A verdict for ANOTHER adapter (driver update, new GPU) re-measures.
        assert_eq!(
            resolve_auto(None, Some(("old driver".into(), true)), fp),
            (false, true)
        );
        // Nothing stored: stay off, measure.
        assert_eq!(resolve_auto(None, None, fp), (false, true));
    }

    /// The verdict file: round trips, and a corrupt file reads as absent.
    #[test]
    fn verdict_file_round_trips_and_rejects_garbage() {
        let dir = std::env::temp_dir().join(format!("mn-verdict-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("gpu-verdict.txt");
        let v = DabVerdict {
            fingerprint: "Adapter X | Dx12 | driver 31.0".into(),
            on: true,
            summary: "plain g-pen 1.01x, soft airbrush 0.42x".into(),
        };
        store_verdict_at(&p, &v);
        assert_eq!(load_verdict_at(&p), Some((v.fingerprint.clone(), true)));
        std::fs::write(&p, "Adapter X | Dx12 | driver 31.0\nmaybe\n").unwrap();
        assert_eq!(load_verdict_at(&p), None, "a corrupt flag never decides");
        std::fs::write(&p, "\non\n").unwrap();
        assert_eq!(load_verdict_at(&p), None, "an empty fingerprint never decides");
        std::fs::remove_dir_all(&dir).ok();
    }

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
