//! Input diagnostics and disclosure: the per-stroke stats line, the F1
//! HUD numbers, and what the pen device says about itself.

use super::*;

pub(crate) struct StrokeStats {
    pub(crate) kind: PointerKind,
    started: Instant,
    pub(crate) samples: u32,
    min_pressure: f32,
    max_pressure: f32,
    batches: u32,
    batch_total: u32,
    batch_max: u32,
    /// Client px the stroke started at, and the session's dropped-report
    /// total at that moment. Both are here for one purpose: a stroke that
    /// received ZERO samples has nothing else to say about itself, and
    /// "nothing was drawn and nothing was said" is the single most-viewed
    /// failure in the pen corpus (§4.2). `pen.dropped` now minus this tells
    /// the two silences apart — no input arrived, versus input arrived and
    /// the in-contact filter ate all of it.
    pub(crate) at: (i32, i32),
    pub(crate) dropped_at_start: u64,
}

impl StrokeStats {
    pub(crate) fn new(kind: PointerKind, at: (i32, i32), dropped_at_start: u64) -> Self {
        Self {
            kind,
            started: Instant::now(),
            samples: 0,
            min_pressure: f32::INFINITY,
            max_pressure: f32::NEG_INFINITY,
            batches: 0,
            batch_total: 0,
            batch_max: 0,
            at,
            dropped_at_start,
        }
    }

    pub(crate) fn note_batch(&mut self, n: usize) {
        self.batches += 1;
        self.batch_total += n as u32;
        self.batch_max = self.batch_max.max(n as u32);
    }

    pub(crate) fn note_sample(&mut self, p: f32) {
        self.samples += 1;
        self.min_pressure = self.min_pressure.min(p);
        self.max_pressure = self.max_pressure.max(p);
    }

    pub(crate) fn report(&self) {
        let secs = self.started.elapsed().as_secs_f64();
        let rate = if secs > 0.0 {
            self.samples as f64 / secs
        } else {
            0.0
        };
        let avg_batch = if self.batches > 0 {
            self.batch_total as f64 / self.batches as f64
        } else {
            0.0
        };
        println!(
            "[stroke] {:5} samples={:<5} {:.0}ms {:.0}/s  pressure {:.3}..{:.3}  \
             history: {} batches, avg {:.1}, max {}",
            self.kind.label(),
            self.samples,
            secs * 1000.0,
            rate,
            if self.min_pressure.is_finite() {
                self.min_pressure
            } else {
                0.0
            },
            if self.max_pressure.is_finite() {
                self.max_pressure
            } else {
                0.0
            },
            self.batches,
            avg_batch,
            self.batch_max,
        );
    }
}

/// Live numbers for the diagnostics HUD (F1). Rolling, not per stroke — this is
/// what tells a bad pen stack (bursty batches, 60 events/s, pressure quantised
/// to a handful of values) from a bad renderer.
pub struct Diag {
    pub events_per_sec: f32,
    events: u32,
    window: Instant,
    pub last_pressure: f32,
    pub pointer: &'static str,
    pub last_batch: usize,
    pub max_batch: usize,
    pub avg_batch: f32,
    batches: u32,
    batch_total: u64,
    pub frame_ms: f32,
    pub frames: u64,
    /// End-to-end input latency: the age, at present time, of the newest
    /// sample this frame showed. `None` = no pen sample yet, or the device's
    /// clock does not agree with `GetTickCount` (the mouse fallback stamps
    /// process-uptime ms, so it never does) — §4.12 wanted a number to
    /// defend, and a wrong number is worth less than an admitted blank.
    pub latency_ms: Option<f32>,
    pub latency_max_ms: f32,
    /// `dwTime` of the newest sample pushed since the last frame.
    last_sample_t_ms: f64,
}

impl Default for Diag {
    fn default() -> Self {
        Self {
            events_per_sec: 0.0,
            events: 0,
            window: Instant::now(),
            last_pressure: 0.0,
            pointer: "-",
            last_batch: 0,
            max_batch: 0,
            avg_batch: 0.0,
            batches: 0,
            batch_total: 0,
            frame_ms: 0.0,
            frames: 0,
            latency_ms: None,
            latency_max_ms: 0.0,
            last_sample_t_ms: 0.0,
        }
    }
}

impl Diag {
    pub(crate) fn note_batch(&mut self, kind: PointerKind, batch: &[PenSample]) {
        self.pointer = kind.label();
        self.last_batch = batch.len();
        self.max_batch = self.max_batch.max(batch.len());
        self.batches += 1;
        self.batch_total += batch.len() as u64;
        self.avg_batch = self.batch_total as f32 / self.batches as f32;
        self.events += batch.len() as u32;
        if let Some(s) = batch.last() {
            self.last_pressure = s.pressure;
            self.last_sample_t_ms = s.t_ms;
        }
        let secs = self.window.elapsed().as_secs_f32();
        if secs >= 0.5 {
            self.events_per_sec = self.events as f32 / secs;
            self.events = 0;
            self.window = Instant::now();
        }
    }

    pub(crate) fn note_frame(&mut self, dt: Duration) {
        self.frame_ms = dt.as_secs_f32() * 1000.0;
        self.frames += 1;
        // §4.12: pen-down to the frame that presents it, the number seven
        // years of latency threads never had. `PenSample::t_ms` is
        // `POINTER_INFO::dwTime` on the `GetTickCount` clock, so this is one
        // subtraction over counters that already existed. Outside a
        // plausible window (a wrapped tick count, a driver clock, or the
        // mouse fallback's uptime stamp) it reports nothing rather than a
        // number nobody could act on.
        if self.last_sample_t_ms > 0.0 {
            let age = crate::win32::tick_ms() - self.last_sample_t_ms;
            self.latency_ms = (0.0..2000.0).contains(&age).then_some(age as f32);
            if let Some(ms) = self.latency_ms {
                self.latency_max_ms = self.latency_max_ms.max(ms);
            }
        }
    }
}

/// What the pen device says about **itself**, as opposed to where it is.
///
/// Every field here exists because the corresponding failure is invisible
/// from the sample stream (`docs/CSP-PEN-TABLET-PAINS.md` §4.1–§4.2, §4.9).
/// Nothing in here changes where a dab lands; it is the disclosure layer,
/// and the whole design rule of the round is that a driver we have never
/// seen will fail in a way we did not predict — so the app's job is to stop
/// lying about it rather than to guess it.
#[derive(Default)]
pub struct PenHealth {
    /// A pen batch with real content has been seen this session.
    pub seen: bool,
    /// The device set `PEN_MASK_PRESSURE` on its newest report. When this is
    /// false every pressure in the stream is `input.rs`'s 0.5 SUBSTITUTE.
    pub pressure_reported: bool,
    pub tilt_reported: bool,
    /// The stylus is tail-end down (`PEN_FLAG_INVERTED`/`_ERASER`).
    pub inverted: bool,
    /// Session total of pointer reports the in-contact filter dropped.
    pub dropped: u64,
    /// `dropped` as it stood immediately BEFORE the newest report was folded
    /// in. The pen-down message is decoded, disclosed and only THEN
    /// dispatched to `canvas_down`, so without this the drops belonging to
    /// the very message that opened a stroke would land outside its own
    /// window — and a press-and-release that never registered contact is
    /// exactly the case where those are the only drops there are.
    pub(crate) dropped_at_last_report: u64,
    /// The tool to restore when the stylus flips back tip-down.
    pub(crate) tool_before_tail: Option<Tool>,
}
