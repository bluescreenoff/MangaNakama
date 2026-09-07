//! The NUMBERS for every panel `effect_lines_sheet` draws — the table a
//! preset is tuned against, so a builder round never again ships a guess
//! (plan `2026-09-07-lean-gauntlet-effects`, "the lean loop").
//!
//! `cargo run --release -p mn-core --example effect_metrics [-- <out-file>]`
//! Prints a markdown table on stdout and, given a path, writes the same
//! text there. Run it with `--release`: it erodes and labels a
//! 2362 × 1653 panel fourteen times, which is seconds released and
//! minutes not.
//!
//! What the columns mean is in `mn_core::genlines::metrics`; the short
//! version is printed under the table so a critic never has to open the
//! source.
//!
//! Writes no images and opens no window.

mod common;

use std::fmt::Write as _;

use mn_core::genlines::metrics;

use common::{DPI, PANEL_MM, panel_size, panels, slug};

fn main() {
    let size = panel_size();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "# Effect-line metrics — {} × {} mm at {DPI} dpi ({} × {} px)\n",
        PANEL_MM.0, PANEL_MM.1, size.0, size.1
    );
    let _ = writeln!(
        s,
        "| panel | cut @ mm | /25mm | w p5 | w p50 | w p95 | p95/p50 | acc % | caps | frags | \
         corner % | ink % TL/TR/BL/BR | bundles | white/hole mm | σ% | hi/lo × | outer σ% |"
    );
    let _ = writeln!(
        s,
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|---:|---:|---|---:|"
    );

    // Same filter the sheet takes, for the same reason: tuning one row
    // should not cost the whole table.
    let only = std::env::var("MN_PANELS").unwrap_or_default();
    let t0 = std::time::Instant::now();
    for (name, spec) in panels() {
        if !only.is_empty() && !slug(&name).contains(&only) {
            continue;
        }
        let t = std::time::Instant::now();
        let m = metrics::measure(&spec, size, DPI);
        eprintln!("[metrics] {name} {:.1}s", t.elapsed().as_secs_f32());
        let ratio = if m.w_p50 > 0.0 {
            format!("{:.2}", m.w_p95 / m.w_p50)
        } else {
            "-".into()
        };
        let opt = |v: Option<f32>, d: usize| match v {
            Some(x) => format!("{x:.*}", d),
            None => "-".to_string(),
        };
        let hilo = match (m.hole_hi_ratio, m.hole_lo_ratio) {
            (Some(a), Some(b)) => format!("{a:.2}/{b:.2}"),
            _ => "-".into(),
        };
        let bundles = if m.bundles.is_empty() {
            "-".to_string()
        } else {
            m.bundles
                .iter()
                .map(|(size, n)| format!("{size}×{n}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let _ = writeln!(
            s,
            "| `{}` | {} | {:.1} | {:.2} | {:.2} | {:.2} | {} | {:.0} | {} | {} | {:.0} | \
             {:.1}/{:.1}/{:.1}/{:.1} | {} | {} | {} | {} | {} |",
            slug(&name),
            m.r_meas_mm
                .map_or("line".to_string(), |r| format!("r {r:.0}")),
            m.per_25mm,
            m.w_p5,
            m.w_p50,
            m.w_p95,
            ratio,
            m.accent_pct,
            m.caps,
            m.frags,
            m.corner_ink,
            m.quarter_ink[0],
            m.quarter_ink[1],
            m.quarter_ink[2],
            m.quarter_ink[3],
            bundles,
            opt(m.hole_mean_mm, 1),
            opt(m.hole_rel(), 1),
            hilo,
            opt(m.outer_rel(), 1),
        );
    }

    let _ = write!(
        s,
        "\n\
        - **cut @** — where the cross-section was taken: `r N` = a circle N mm from the\n\
        \x20 burst's centre (the ring's mid radius, pulled in if that circle is mostly off\n\
        \x20 the panel), `line` = straight across the panel's middle, square to the runs.\n\
        - **/25mm** — complete strokes the cut crossed per 25 mm of ON-PANEL cut. Strokes\n\
        \x20 the frame cut in half are not counted: they are not a width measurement.\n\
        - **w p5/p50/p95** — stroke width across that cut, in mm. For the solid flash the\n\
        \x20 \"stroke\" is the BLACK spike between two cut teeth — the mark on the paper.\n\
        - **p95/p50** — the weight mix in one number. A printed set is 2.5× or more; 1.9×\n\
        \x20 with nothing above twice the median is what the last critic called \"machine\".\n\
        - **acc %** — share of strokes at least twice the median width.\n\
        - **caps** — ends that stop dead instead of running out to a point (erode to the\n\
        \x20 cores of strokes ≥ 5 px, march outward from each core's own end through the\n\
        \x20 full ink). Frame-cut ends do not count, and neither do BURIED ends — one\n\
        \x20 whose neighbourhood is ≥ 62 % ink sits inside a mass, where there is no end\n\
        \x20 to see. Target 0.\n\
        - **frags** — marks belonging to nothing: black blobs of ≤ 20 px (dirt on the\n\
        \x20 scan) on any row, plus — on a solid flash, whose black is meant to be ONE\n\
        \x20 field — every black island floating clear of it. Target 0.\n\
        - **corner %** — ink in the EMPTIEST of four 12 % × 12 % corner boxes. ref-20/21\n\
        \x20 are an all-black rectangle with a white burst punched through it, so a solid\n\
        \x20 flash reads 100 here. On the other rows it is just a reading.\n\
        - **bundles** — `size×count`: runs of strokes whose gaps fall in the SMALL of the\n\
        \x20 two gap populations on the cut (1-D 2-means; two centroids within 1.6× means\n\
        \x20 one rhythm and every stroke reports as its own). All `1×` = a comb.\n\
        - **white/hole mm + σ%** — 2160 rays out of the centre. Every row but the solid\n\
        \x20 flash reports where each ray FIRST meets ink — the hole of a 集中線 or a\n\
        \x20 ウニフラ (ref-12's = 20 %). The solid flash reports the outer radius of the\n\
        \x20 WHITE region connected to the middle, core plus slivers, because that is the\n\
        \x20 shape the effect IS and the one REFS measures on ref-20/21 (σ 20–31 %).\n\
        - **hi/lo ×** — that sweep's longest and shortest reach over its median. REFS\n\
        \x20 target 4 for a solid flash: 1.5–2.0 and 0.55–0.65.\n\
        - **outer σ%** — the same sweep's LAST ink, i.e. the outer silhouette. `-` means\n\
        \x20 the silhouette is the panel frame, not the effect, so there is nothing to\n\
        \x20 judge. REFS target 3: a ウニフラ's hole must be ROUGHER than its outline.\n\
        \n\
        Rendered in {:.1} s.\n",
        t0.elapsed().as_secs_f32()
    );

    print!("{s}");
    if let Some(p) = std::env::args().nth(1) {
        std::fs::write(&p, &s).expect("write the metrics table");
        eprintln!("[metrics] -> {p}");
    }
}
