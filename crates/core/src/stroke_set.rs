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
}

impl VectorStroke {
    pub fn samples(&self) -> impl Iterator<Item = PenSample> + '_ {
        self.points.iter().map(|&(x, y, pressure, tilt_x, tilt_y, t_ms)| PenSample {
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
        }
    }
}

/// A vector layer's recorded strokes, draw order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StrokeSet {
    pub strokes: Vec<VectorStroke>,
}

impl StrokeSet {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(doc.active_layer().strokes.as_ref().unwrap().strokes.len(), 1);
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
        let stroke =
            VectorStroke::from_samples(
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
}
