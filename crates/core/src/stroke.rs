//! Stroke pipeline types. Contract from docs/ARCHITECTURE.md — exact shapes.
//!
//! `PenSample`s flow: Win32 WM_POINTER (app) -> [stabilizer, later] ->
//! `StrokeSink` (brush) -> `Document` tiles.

use crate::doc::Document;

/// One pen/mouse sample in **canvas pixel space** (not screen space — the app
/// applies the viewport transform before handing samples over).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PenSample {
    pub x: f32,
    pub y: f32,
    /// 0..1. Windows reports 0..1024; the app divides by 1024.0.
    pub pressure: f32,
    /// Degrees, -90..90, from `POINTER_PEN_INFO::tiltX` (passthrough for now).
    pub tilt_x: f32,
    /// Degrees, -90..90, from `POINTER_PEN_INFO::tiltY`.
    pub tilt_y: f32,
    /// Milliseconds, monotonic within a stroke.
    pub t_ms: f64,
}

/// Anything that turns `PenSample`s into pixels. `brush::SimpleDab` today,
/// `brush::MyBrush` (libmypaint) once the FFI lands — the app only ever sees
/// this trait, so swapping engines touches one line.
pub trait StrokeSink {
    fn begin(&mut self, doc: &mut Document);
    fn sample(&mut self, doc: &mut Document, s: PenSample);
    fn end(&mut self, doc: &mut Document);
}
