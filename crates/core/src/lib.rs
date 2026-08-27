//! MangaNakama core: document/layer/tile model, undo, stroke pipeline, file I/O.
//!
//! No OS dependencies by contract — everything here is testable with plain
//! `cargo test`. See docs/ARCHITECTURE.md.
//!
//! Map of the crate:
//!
//! * [`tile`] — the 64x64 premultiplied fix15 tile and its revision counter.
//! * [`doc`] — `Document` / `Layer` / `Blend`, layer ops, undo bracketing.
//! * [`edge`] — the border effect and the distance transform text shares.
//! * [`undo`] — `UndoGroup` (per-op tile snapshots) and the two stacks.
//! * [`stroke`] — `PenSample` and the `StrokeSink` trait (the brush seam).
//! * [`stabilize`] — pull-string smoothing, a `StrokeSink` decorator.
//! * [`curve`] — pressure response curve.
//! * [`blend`] — the blend formulas, shared with the GPU compositor.
//! * [`export`] — exact CPU compositing + PNG export.
//! * [`ora`] — OpenRaster save/load.

pub mod adjust;
pub mod align;
pub mod balloon;
pub mod blend;
pub mod curve;
pub mod dab;
pub mod doc;
pub mod edge;
pub mod export;
pub mod fill;
pub mod fill_layer;
pub mod filter;
pub mod frame;
pub mod frame_order;
pub mod genlines;
pub mod gradient;
pub mod liquify;
pub mod mesh;
pub mod magnetic;
pub mod ora;
pub mod page;
pub mod palette;
pub mod preflight;
pub mod profile;
pub mod project;
pub mod psd;
pub mod ruler;
pub mod selection;
pub mod stabilize;
pub mod stroke;
pub mod stroke_set;
pub mod taper;
pub mod text;
pub mod tile;
pub mod tone;
pub mod transform;
pub mod undo;

pub use adjust::{Adjust, TONE_CURVE_MAX};
pub use balloon::{
    Balloon, BalloonHandle, BalloonInk, BalloonSet, BalloonShape, BalloonTone, Tail, TailGeom,
    TailKind,
};
pub use blend::{Rgba, blend_premul, expression_reduce, layer_colour_tint, scale_opacity};
pub use curve::PressureCurve;
pub use doc::{
    Blend, DEFAULT_SIZE, Document, Layer, LayerExpression, LayerKind, Paper, ResizeAnchor,
};
pub use edge::EdgeParams;
pub use export::Background;
pub use fill::{AutoFill, FillClose, FillOpts, FillRefer};
pub use fill_layer::FillKind;
pub use filter::{Filter, MotionDir, MotionMode, Raster, WaveDir};
pub use frame::{Frame, FrameSet};
pub use genlines::{FocusLinesParams, SpeedLinesParams, render_focus, render_speed};
pub use gradient::{
    EdgeProcess, GradStop, GradientSet, MidStops, MixMode, NamedRamp, Ramp, RampOpts,
};
pub use page::PageSetup;
pub use preflight::{PreflightFinding, PreflightLevel, run_page, run_work};
pub use project::{Expression, Project, ProjectMeta};
pub use ruler::{AnchorRole, CurveRuler, Ruler, RulerGrab, Rulers, SnapLock};
pub use selection::{SEL_ON, Selection, SelectionOp, selected};
pub use stabilize::Stabilizer;
pub use stroke::{PenSample, StrokeSink};
pub use stroke_set::{StrokeSet, VectorStroke};
pub use taper::Taper;
pub use text::{
    Align, FrameAlign, LineSpacing, RenderedText, StyleFlag, StyleRun, TextHandle, TextItem,
    TextSet,
};
pub use tile::{
    FIX15_ONE, TILE_CHANNELS, TILE_LEN, TILE_PIXELS, TILE_SIZE, Tile, TileIdx, next_revision,
};
pub use tone::{ToneDensity, ToneParams, TonePattern};
pub use transform::{Affine2, FloatSource};
pub use undo::{UNDO_LIMIT, UndoGroup};
