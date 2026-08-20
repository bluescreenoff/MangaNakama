//! Stroke entry taper — a `StrokeSink` decorator that ramps pressure up over
//! the first `length_px` of arc length, so strokes start thin the way CSP's
//! 入り (entry) taper does. The owner's Real G-Pen carries a CSP entry taper
//! the .myb import could not express; this is the native replacement.

use crate::doc::Document;
use crate::stroke::{PenSample, StrokeSink};

#[derive(Debug)]
pub struct Taper<S> {
    inner: S,
    /// Arc length over which the ramp runs; 0 disables the taper entirely.
    pub length_px: f32,
    /// Pressure factor at the very start of the stroke (0..1).
    pub min: f32,
    acc: f32,
    last: Option<(f32, f32)>,
}

impl<S> Taper<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            length_px: 0.0,
            min: 0.18,
            acc: 0.0,
            last: None,
        }
    }

    pub fn inner(&self) -> &S {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }
}

impl<S: StrokeSink> StrokeSink for Taper<S> {
    fn begin(&mut self, doc: &mut Document) {
        self.acc = 0.0;
        self.last = None;
        self.inner.begin(doc);
    }

    fn sample(&mut self, doc: &mut Document, s: PenSample) {
        if let Some((lx, ly)) = self.last {
            self.acc += ((s.x - lx).powi(2) + (s.y - ly).powi(2)).sqrt();
        }
        self.last = Some((s.x, s.y));
        let f = if self.length_px > 1.0 && self.acc < self.length_px {
            let m = self.min.clamp(0.0, 1.0);
            m + (1.0 - m) * (self.acc / self.length_px)
        } else {
            1.0
        };
        self.inner.sample(
            doc,
            PenSample {
                pressure: s.pressure * f,
                ..s
            },
        );
    }

    fn end(&mut self, doc: &mut Document) {
        self.inner.end(doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Probe(Vec<f32>);
    impl StrokeSink for Probe {
        fn begin(&mut self, _: &mut Document) {}
        fn sample(&mut self, _: &mut Document, s: PenSample) {
            self.0.push(s.pressure);
        }
        fn end(&mut self, _: &mut Document) {}
    }

    fn sample_at(x: f32) -> PenSample {
        PenSample {
            x,
            y: 0.0,
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: x as f64,
        }
    }

    #[test]
    fn ramps_over_the_configured_length_then_passes_through() {
        let mut doc = Document::new(64, 64);
        let mut t = Taper::new(Probe(Vec::new()));
        t.length_px = 100.0;
        t.min = 0.2;
        t.begin(&mut doc);
        for x in [0.0, 50.0, 100.0, 200.0] {
            t.sample(&mut doc, sample_at(x));
        }
        t.end(&mut doc);
        let p = &t.inner().0;
        assert!((p[0] - 0.2).abs() < 1e-4, "start at min: {p:?}");
        assert!((p[1] - 0.6).abs() < 1e-4, "midway: {p:?}");
        assert!((p[2] - 1.0).abs() < 1e-4, "ramp done: {p:?}");
        assert!((p[3] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn zero_length_is_exact_passthrough() {
        let mut doc = Document::new(64, 64);
        let mut t = Taper::new(Probe(Vec::new()));
        t.begin(&mut doc);
        t.sample(&mut doc, sample_at(0.0));
        t.sample(&mut doc, sample_at(3.0));
        assert!(t.inner().0.iter().all(|p| (*p - 1.0).abs() < 1e-6));
    }
}
