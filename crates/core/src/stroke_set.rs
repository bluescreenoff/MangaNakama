//! Vector inking, phase 1 (docs/VECTOR-INKING.md): the recorded-stroke
//! model. A layer with `strokes: Some(..)` is an ordinary raster layer
//! whose ink ALSO exists as editable geometry — drawing rasterizes through
//! the normal pipeline and records here; edits (later phases) re-derive
//! the raster by replaying.

use serde::{Deserialize, Serialize};

use crate::PenSample;

/// One recorded stroke: the pen samples as the stroke pipeline received
/// them (pre-engine, so a replay through the same engine — stabilizer,
/// taper, twins and all — reproduces the pipeline), plus what the app
/// needs to rebuild that engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorStroke {
    /// (x, y, pressure, tilt_x, tilt_y, t_ms) — `PenSample`, flattened for
    /// a compact, stable serialization.
    pub points: Vec<(f32, f32, f32, f32, f32, f64)>,
    /// Preset name (the brush picker's identity); resolution is app-side,
    /// and a missing preset replays with the default pen — degraded,
    /// never lost.
    pub preset: String,
    /// Absolute dab diameter at draw time, canvas px.
    pub size_px: f32,
    /// Straight RGB.
    pub color: [u8; 3],
    pub eraser: bool,
    /// The stabilizer strength the stroke was drawn under — the samples
    /// are captured BEFORE the pull-string, so a faithful replay re-runs
    /// it at the same strength. serde(default) keeps older sidecars
    /// loading (they replay unstabilized, degraded not lost).
    #[serde(default)]
    pub stabilizer: f32,
    /// Future re-width edits multiply here; 1.0 as drawn.
    pub width_scale: f32,
    /// The Tool Property values the stroke was drawn under, beyond the four
    /// above. Without them the replay rebuilds the engine from the `.myb`
    /// alone, so one control-point nudge re-inks the WHOLE layer at the
    /// preset's authored opacity/correction/taper instead of yours.
    /// `None` = a record written before this field existed: replay it the
    /// old way (preset values), degraded not lost.
    #[serde(default)]
    pub settings: Option<StrokeSettings>,
}

/// The per-stroke half of the Tool Property panel that the replay has to
/// restore. Deliberately not the whole panel: opacity, the Correction group
/// and the entry taper are the ones a mangaka SEES change (the audit's
/// "visible 90 %"); scatter, texture and the wash family stay preset-side
/// until a stroke needs them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StrokeSettings {
    /// Absolute brush opacity 0..1 — the engine's BASE opacity, i.e. the
    /// wash flow when the stroke was a wash one, exactly as the live path
    /// resolves it.
    pub opacity: f32,
    /// CSP's Correction group minus the stabilizer slider (post correction,
    /// the sharp-angle exception, the entry/exit shaping).
    pub correct: crate::stabilize::CorrectCfg,
    /// Entry taper: ramp length in px (0 = off) and its starting pressure.
    pub taper_px: f32,
    pub taper_min: f32,
    /// Row 71's watercolour edge. Rides the snapshot for the reason the
    /// whole snapshot exists: the rim is baked at stroke end, so a replay
    /// that rebuilt the engine from the `.myb` alone would re-ink the layer
    /// with the PRESET's rim (usually none) and quietly strip yours off
    /// every stroke the moment you nudge one control point.
    pub water_edge: crate::edge::WaterEdge,
}

impl Default for StrokeSettings {
    /// The neutral stroke: full opacity, no correction, no taper — the same
    /// numbers `Taper::new` and a fresh engine start from, so a snapshot
    /// missing a field replays as if it were not there.
    fn default() -> Self {
        Self {
            opacity: 1.0,
            correct: crate::stabilize::CorrectCfg::default(),
            taper_px: 0.0,
            taper_min: 0.18,
            water_edge: crate::edge::WaterEdge::default(),
        }
    }
}

impl VectorStroke {
    pub fn samples(&self) -> impl Iterator<Item = PenSample> + '_ {
        self.points
            .iter()
            .map(|&(x, y, pressure, tilt_x, tilt_y, t_ms)| PenSample {
                x,
                y,
                pressure,
                tilt_x,
                tilt_y,
                t_ms,
            })
    }

    pub fn from_samples(
        samples: &[PenSample],
        preset: &str,
        size_px: f32,
        color: [u8; 3],
        eraser: bool,
    ) -> Self {
        VectorStroke {
            points: samples
                .iter()
                .map(|s| (s.x, s.y, s.pressure, s.tilt_x, s.tilt_y, s.t_ms))
                .collect(),
            preset: preset.to_string(),
            size_px,
            color,
            eraser,
            stabilizer: 0.0,
            width_scale: 1.0,
            settings: None,
        }
    }
}

/// A vector layer's recorded strokes, draw order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StrokeSet {
    pub strokes: Vec<VectorStroke>,
}

impl StrokeSet {
    /// The vector eraser's TRIM (docs/VECTOR-INKING.md phase 3, Clip
    /// Studio's "erase up to intersection"): every stroke the eraser path
    /// touches (within `radius`) loses the touched span EXTENDED to the
    /// nearest crossings with OTHER strokes — or to its own ends. A
    /// remainder shorter than two samples vanishes; a middle cut splits
    /// the stroke in two. Returns whether anything changed.
    ///
    /// v1 grain: cuts land on the sample nearest the crossing, not the
    /// exact intersection coordinate — samples are input-density (a couple
    /// of px apart), so the error is subpixel-ish and the replay stays
    /// sample-faithful.
    pub fn trim(&mut self, eraser: &[(f32, f32)], radius: f32) -> bool {
        if eraser.is_empty() || self.strokes.is_empty() {
            return false;
        }
        // Which sample indices of stroke `si` cross any OTHER stroke.
        let crossings = |strokes: &[VectorStroke], si: usize| -> Vec<usize> {
            let s = &strokes[si];
            let mut out = Vec::new();
            for (a, w) in s.points.windows(2).enumerate() {
                let seg_a = ([w[0].0, w[0].1], [w[1].0, w[1].1]);
                'others: for (oi, o) in strokes.iter().enumerate() {
                    if oi == si {
                        continue;
                    }
                    for v in o.points.windows(2) {
                        if segments_cross(seg_a, ([v[0].0, v[0].1], [v[1].0, v[1].1])) {
                            out.push(a);
                            break 'others;
                        }
                    }
                }
            }
            out
        };
        let touched = |s: &VectorStroke| -> Vec<bool> {
            s.points
                .iter()
                .map(|p| {
                    eraser.len() == 1 && {
                        let e = eraser[0];
                        (p.0 - e.0).hypot(p.1 - e.1) <= radius
                    } || eraser.windows(2).any(|w| {
                        seg_point_dist([w[0].0, w[0].1], [w[1].0, w[1].1], [p.0, p.1]) <= radius
                    })
                })
                .collect()
        };

        let mut result: Vec<VectorStroke> = Vec::with_capacity(self.strokes.len());
        let mut changed = false;
        for si in 0..self.strokes.len() {
            let s = &self.strokes[si];
            let touch = touched(s);
            if !touch.iter().any(|&t| t) {
                result.push(s.clone());
                continue;
            }
            changed = true;
            let cuts = crossings(&self.strokes, si);
            // Erase mask: every touched run, widened to the neighbouring
            // crossings (or the ends).
            let n = s.points.len();
            let mut erase = vec![false; n];
            let mut i = 0;
            while i < n {
                if touch[i] {
                    let run_end = (i..n).take_while(|&j| touch[j]).last().unwrap_or(i);
                    // Nearest crossing strictly below the run start; the
                    // span keeps [0..=lo] alive, so erase from lo+1.
                    let lo = cuts.iter().rev().find(|&&c| c < i).map_or(0, |&c| c + 1);
                    // Nearest crossing at/after the run end: samples up to
                    // and including the crossing segment's start die.
                    let hi = cuts.iter().find(|&&c| c >= run_end).map_or(n - 1, |&c| c);
                    for e in erase.iter_mut().take(hi + 1).skip(lo) {
                        *e = true;
                    }
                    i = hi + 1;
                } else {
                    i += 1;
                }
            }
            // Kept intervals (≥ 2 samples) become strokes.
            let mut start: Option<usize> = None;
            for j in 0..=n {
                let keep = j < n && !erase[j];
                match (start, keep) {
                    (None, true) => start = Some(j),
                    (Some(a), false) => {
                        if j - a >= 2 {
                            let mut part = s.clone();
                            part.points = s.points[a..j].to_vec();
                            result.push(part);
                        }
                        start = None;
                    }
                    _ => {}
                }
            }
        }
        if changed {
            self.strokes = result;
        }
        changed
    }

    /// Serialize for the `.ora` sidecar entry (`data/layerN.strokes.json`).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{\"strokes\":[]}".into())
    }

    /// Parse a sidecar. A malformed one reads as EMPTY-but-present: the
    /// layer stays a vector layer (its raster is intact), the record is
    /// gone — degraded, loudly loggable, never a load failure.
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }
}

/// Proper segment intersection (touching endpoints count — a T-junction is
/// a junction).
fn segments_cross(a: ([f32; 2], [f32; 2]), b: ([f32; 2], [f32; 2])) -> bool {
    let d = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let (d1, d2) = (d(b.0, b.1, a.0), d(b.0, b.1, a.1));
    let (d3, d4) = (d(a.0, a.1, b.0), d(a.0, a.1, b.1));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    let on = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| {
        d(p, q, r).abs() < 1e-6
            && r[0] >= p[0].min(q[0]) - 1e-6
            && r[0] <= p[0].max(q[0]) + 1e-6
            && r[1] >= p[1].min(q[1]) - 1e-6
            && r[1] <= p[1].max(q[1]) + 1e-6
    };
    on(b.0, b.1, a.0) || on(b.0, b.1, a.1) || on(a.0, a.1, b.0) || on(a.0, a.1, b.1)
}

fn seg_point_dist(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    let (abx, aby) = (b[0] - a[0], b[1] - a[1]);
    let len2 = abx * abx + aby * aby;
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len2).clamp(0.0, 1.0)
    };
    (p[0] - (a[0] + abx * t)).hypot(p[1] - (a[1] + aby * t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(x0: f32, y0: f32, x1: f32, y1: f32, n: usize) -> VectorStroke {
        let pts: Vec<PenSample> = (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                PenSample {
                    x: x0 + (x1 - x0) * t,
                    y: y0 + (y1 - y0) * t,
                    pressure: 1.0,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: i as f64,
                }
            })
            .collect();
        VectorStroke::from_samples(&pts, "pen", 8.0, [0, 0, 0], false)
    }

    /// The headline behaviour: a horizontal stroke crossed by two verticals
    /// — the eraser touches the middle span, and exactly the between-the-
    /// crossings piece dies; the outer pieces survive as TWO strokes.
    #[test]
    fn trim_erases_up_to_the_neighbouring_intersections() {
        let mut set = StrokeSet {
            strokes: vec![
                line(0.0, 50.0, 300.0, 50.0, 61), // horizontal, every 5px
                line(100.0, 0.0, 100.0, 100.0, 21),
                line(200.0, 0.0, 200.0, 100.0, 21),
            ],
        };
        // Touch the horizontal at x≈150 only.
        assert!(set.trim(&[(150.0, 50.0)], 6.0));
        assert_eq!(
            set.strokes.len(),
            4,
            "middle span died, two pieces + verticals"
        );
        // Piece 1 ends at the first crossing (x≈100), piece 2 begins at the
        // second (x≈200) — sample-grain tolerance.
        let xs: Vec<(f32, f32)> = set.strokes[..2]
            .iter()
            .map(|s| (s.points.first().unwrap().0, s.points.last().unwrap().0))
            .collect();
        assert!(xs[0].0 <= 0.5 && (xs[0].1 - 100.0).abs() <= 6.0, "{xs:?}");
        assert!((xs[1].0 - 200.0).abs() <= 6.0 && xs[1].1 >= 299.0, "{xs:?}");
        // The verticals are untouched.
        assert_eq!(set.strokes[2].points.len(), 21);
        assert_eq!(set.strokes[3].points.len(), 21);
    }

    /// No crossings: the touched stroke dies to its ENDS — the whole
    /// stroke vanishes (the overshooting-hatch case with nothing to stop
    /// at).
    #[test]
    fn trim_without_intersections_takes_the_whole_stroke() {
        let mut set = StrokeSet {
            strokes: vec![line(0.0, 50.0, 300.0, 50.0, 61)],
        };
        assert!(set.trim(&[(150.0, 50.0)], 6.0));
        assert!(set.strokes.is_empty());
        // And an eraser that touches nothing changes nothing.
        let mut set = StrokeSet {
            strokes: vec![line(0.0, 50.0, 300.0, 50.0, 61)],
        };
        assert!(!set.trim(&[(150.0, 500.0)], 6.0));
        assert_eq!(set.strokes.len(), 1);
    }

    /// One drawn stroke = ONE undo step that takes back pixels AND record
    /// together (a half-undo would leave geometry describing missing ink).
    #[test]
    fn a_recorded_stroke_undoes_pixels_and_geometry_together() {
        let mut doc = crate::Document::new(128, 128);
        doc.active_layer_mut().strokes = Some(StrokeSet::default());
        const W: u16 = crate::FIX15_ONE as u16;
        doc.begin_op();
        doc.active_layer_mut()
            .tile_mut(crate::TileIdx::new(0, 0))
            .set_pixel(3, 4, [W, W, W, W]);
        let stroke = VectorStroke::from_samples(
            &[PenSample {
                x: 3.0,
                y: 4.0,
                pressure: 1.0,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: 0.0,
            }],
            "pen",
            10.0,
            [0, 0, 0],
            false,
        );
        assert!(doc.end_op_vector_stroke(stroke.clone()));
        assert_eq!(
            doc.active_layer().strokes.as_ref().unwrap().strokes.len(),
            1
        );
        let inked = |d: &crate::Document| {
            d.active_layer()
                .tile(crate::TileIdx::new(0, 0))
                .is_some_and(|t| t.pixel(3, 4)[3] > 0)
        };
        assert!(inked(&doc));

        assert!(doc.undo());
        assert!(!inked(&doc), "undo takes the ink");
        assert_eq!(
            doc.active_layer().strokes.as_ref().unwrap().strokes.len(),
            0,
            "…and the record, in the same step"
        );
        assert!(doc.redo());
        assert!(inked(&doc));
        assert_eq!(
            doc.active_layer().strokes.as_ref().unwrap().strokes,
            vec![stroke]
        );

        // An empty gesture spends nothing.
        doc.begin_op();
        assert!(!doc.end_op_vector_stroke(VectorStroke::from_samples(
            &[],
            "pen",
            10.0,
            [0, 0, 0],
            false
        )));
    }

    /// The `.ora` sidecar: a vector layer's record survives the round trip
    /// (as `data/layerN.strokes.json` beside the rendered PNG); ordinary
    /// layers stay ordinary.
    #[test]
    fn the_record_rides_the_ora_file() {
        let mut doc = crate::Document::new(128, 128);
        doc.add_layer("plain");
        doc.add_layer("vector");
        let li = doc.layers.len() - 1;
        let stroke = VectorStroke::from_samples(
            &[PenSample {
                x: 1.0,
                y: 2.0,
                pressure: 0.5,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: 8.0,
            }],
            "csp/real-g-pen",
            14.0,
            [1, 2, 3],
            false,
        );
        doc.layers[li].strokes = Some(StrokeSet {
            strokes: vec![stroke.clone()],
        });

        let mut buf = std::io::Cursor::new(Vec::new());
        crate::ora::save_to(&doc, &mut buf).unwrap();
        let back = crate::ora::load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        assert_eq!(
            back.layers[li].strokes.as_ref().unwrap().strokes,
            vec![stroke]
        );
        assert!(back.layers[li - 1].strokes.is_none());
    }

    #[test]
    fn strokes_round_trip_through_json() {
        let s = VectorStroke::from_samples(
            &[
                PenSample {
                    x: 1.5,
                    y: 2.5,
                    pressure: 0.75,
                    tilt_x: 0.1,
                    tilt_y: -0.2,
                    t_ms: 16.0,
                },
                PenSample {
                    x: 3.0,
                    y: 4.0,
                    pressure: 1.0,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: 32.0,
                },
            ],
            "csp/real-g-pen",
            12.5,
            [10, 20, 30],
            false,
        );
        let set = StrokeSet {
            strokes: vec![s.clone()],
        };
        let back = StrokeSet::from_json(&set.to_json());
        assert_eq!(back, set);
        assert_eq!(back.strokes[0].samples().count(), 2);
        // Garbage degrades to empty, never an error.
        assert_eq!(StrokeSet::from_json("not json").strokes.len(), 0);
    }

    /// Back-compat for the settings snapshot: a sidecar written before it
    /// existed has no `settings` key, loads clean, and comes back as `None`
    /// — which the replay reads as "keep the preset's own numbers", i.e.
    /// exactly what that file's ink was derived with when it was written.
    #[test]
    fn a_sidecar_without_settings_loads_and_keeps_the_old_replay() {
        let old = r#"{"strokes":[{"points":[[10.0,20.0,0.9,0.0,0.0,0.0],
            [30.0,20.0,0.9,0.0,0.0,8.0]],"preset":"pen","size_px":8.0,
            "color":[0,0,0],"eraser":false,"stabilizer":0.25,
            "width_scale":1.0}]}"#;
        let set = StrokeSet::from_json(old);
        assert_eq!(set.strokes.len(), 1, "the old sidecar still parses");
        let s = &set.strokes[0];
        assert_eq!(s.settings, None, "no snapshot: the replay stays as it was");
        assert!((s.stabilizer - 0.25).abs() < 1e-6, "the old fields survive");

        // And a snapshot round-trips through the same sidecar.
        let mut set = set;
        set.strokes[0].settings = Some(StrokeSettings {
            opacity: 0.4,
            correct: crate::stabilize::CorrectCfg {
                post: 0.5,
                sharp: true,
                ..Default::default()
            },
            taper_px: 200.0,
            taper_min: 0.1,
            water_edge: crate::edge::WaterEdge {
                px: 3.0,
                opacity: 0.4,
                darkness: 0.2,
                blur_px: 1.0,
            },
        });
        assert_eq!(StrokeSet::from_json(&set.to_json()), set);
    }

    /// Row 71: a snapshot written before the rim field existed replays with
    /// the rim OFF, not with whatever the preset happens to carry — the
    /// `serde(default)` on `StrokeSettings` plus `WaterEdge`'s own off
    /// default, which is the pair that keeps every existing sidecar drawing
    /// the ink it drew.
    #[test]
    fn a_settings_snapshot_without_a_rim_replays_without_one() {
        let old = r#"{"strokes":[{"points":[[1.0,2.0,1.0,0.0,0.0,0.0]],
            "preset":"pen","size_px":8.0,"color":[0,0,0],"eraser":false,
            "stabilizer":0.0,"width_scale":1.0,
            "settings":{"opacity":0.4,"taper_px":12.0}}]}"#;
        let set = StrokeSet::from_json(old);
        let cfg = set.strokes[0].settings.expect("the snapshot parses");
        assert!((cfg.opacity - 0.4).abs() < 1e-6, "its own fields survive");
        assert!(!cfg.water_edge.on(), "and the rim it never had is off");
    }
}
