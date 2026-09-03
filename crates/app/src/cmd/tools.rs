//! The tool enums: [`Tool`], [`SubTool`], the per-tool mode enums
//! and option structs, and [`ToolProps`]. Re-exported from `cmd`,
//! so every `crate::cmd::Tool` path still resolves.

use super::*;

/// Left-strip tools, CSP order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Pen,
    Eraser,
    /// CSP 図形 Figure: inked shapes (line/rect/ellipse/polygon) through the
    /// active brush.
    Figure,
    /// CSP グラデ Gradient: drag a ramp between two colours.
    Gradient,
    Fill,
    /// One-gesture screentone: ONE click floods the enclosed region under
    /// the cursor (the Fill tool's own region machinery, gap closing and
    /// all) and hands it to a LIVE tone layer as its window. It is its own
    /// tool rather than a Fill sub tool because the Fill sub tools all
    /// differ in how they AIM the same flood while sharing one Tool
    /// Property; this one shares the aiming and needs the tone parameters
    /// instead, which have no home in the Fill panel.
    Tone,
    Select,
    /// CSP 選択ペン Selection pen: paint selection coverage with the
    /// active brush — release ADDS to the selection (SE-022 combine).
    SelPen,
    /// CSP 選択消しゴム Selection eraser: same stroke, release SUBTRACTS.
    SelEraser,
    /// CSP Auto select (magic wand): click floods a region into a selection.
    Wand,
    /// CSP Operation ▸ Object: select/move/reshape frames and balloons.
    Object,
    /// Frame borders: divide folders/panels, drag out new rectangle frames.
    Frame,
    /// Speech balloons: drag out a body / draw one / attach a tail.
    Balloon,
    /// Text: click to place/edit a text box (T).
    Text,
    /// Eyedropper: click picks the colour under the cursor (I).
    Eyedrop,
    /// CSP 液化 Liquify: warp the raster layer directly with the pen —
    /// push/expand/pinch/push-sideways/twirl, Alt inverts, hold
    /// accumulates (`core::liquify`).
    Liquify,
    Pan,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Pen => "Pen",
            Tool::Eraser => "Eraser",
            Tool::Figure => "Figure",
            Tool::Gradient => "Gradient",
            Tool::Fill => "Fill",
            Tool::Tone => "Tone",
            Tool::Select => "Select",
            Tool::SelPen => "Select pen",
            Tool::SelEraser => "Select eraser",
            Tool::Wand => "Auto select",
            Tool::Object => "Object",
            Tool::Frame => "Frame border",
            Tool::Balloon => "Balloon",
            Tool::Text => "Text",
            Tool::Eyedrop => "Eyedropper",
            Tool::Liquify => "Liquify",
            Tool::Pan => "Move view",
        }
    }

    pub fn enabled(self) -> bool {
        true
    }

    /// Tools that paint strokes through the brush engine.
    pub fn strokes(self) -> bool {
        matches!(
            self,
            Tool::Pen | Tool::Eraser | Tool::SelPen | Tool::SelEraser
        )
    }
}

/// Figure-tool sub tools (CSP: Straight line / Rectangle / Ellipse /
/// Polygon — curve/continuous-curve deferred with the vector system).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FigureMode {
    Line,
    Rect,
    Ellipse,
    /// Click vertices; the first vertex / Enter closes, Esc cancels.
    Polygon,
    /// Rows 84/85 (C-014–022, U-001/002) 連続曲線: click a few points and
    /// Enter (or a double-click) inks a smooth spline THROUGH them with the
    /// current brush. The two-stage sibling of Polygon — same click-list
    /// gesture, same Enter/Esc, but the path is a Catmull-Rom curve and it
    /// is not closed. The mark a hand cannot make in one pass: a 900 px
    /// hair sweep, a cable, a curved speed line.
    Curve,
    /// Row 157 / `FG-002` 曲線 Curve: the TWO-STAGE arc. Drag the straight
    /// baseline, release, then move the pointer and the line bends to run
    /// through it; a click inks it. Distinct from [`FigureMode::Curve`],
    /// which is CSP's 連続曲線 *Continuous* curve (a click list) — this one
    /// is one segment and one bend, the quick swoosh you reach for mid-panel
    /// when a straight line is nearly right.
    Arc,
    /// CSP 流線 Stream line: drag along the motion — a fresh speed-line
    /// layer sweeps the canvas at that angle (the GenLines engine; the
    /// layer stays parameter-editable afterwards, unlike CSP's).
    Stream,
    /// CSP 集中線 Saturated line: drag from the convergence point outward —
    /// a fresh focus-line layer converging on the press point.
    Focus,
    /// ウニフラッシュ Sea urchin flash: same centre-out drag, but the rays
    /// are FILLED triangular spikes — the flash mat no line generator can
    /// make (pro-page audit 2026-08-22, #1 IMPOSSIBLE).
    Urchin,
    /// ベタフラッシュ Solid flash: the same teeth cut OUT of a solid ring.
    SolidFlash,
    /// Row 156 / `FG-020` **Smart shape**: draw freehand, then HOLD still at
    /// the end and the stroke becomes the clean figure it was approximating
    /// (`mn_core::shape_fit`). The one Figure sub tool whose gesture is a
    /// FREEHAND stroke rather than a drag or a click list — it inks live
    /// with the brush like the pen does, and the hold is what turns that
    /// ink into a figure. Releasing without holding leaves the stroke
    /// exactly as drawn, always.
    ///
    /// It is a sub tool rather than a preference on the Pen (which is where
    /// CSP puts it) on purpose: here it costs one registry row and cannot
    /// surprise a pen stroke, and `FG-020`'s own preference exists mainly
    /// to let CSP users turn the surprise OFF.
    Smart,
}

impl FigureMode {
    /// Do the Stream/Saturated/flash drags apply — the modes that place a
    /// generated layer instead of inking the active one? One predicate,
    /// because press, preview, release and the Tool Property all have to
    /// agree and a missed arm inks a frame layer.
    pub fn generates(self) -> bool {
        matches!(
            self,
            FigureMode::Stream | FigureMode::Focus | FigureMode::Urchin | FigureMode::SolidFlash
        )
    }

    /// The [`mn_core::genlines::GenLinesSpec`] `kind` this mode places.
    pub fn gen_kind(self) -> u8 {
        match self {
            FigureMode::Urchin => 1,
            FigureMode::SolidFlash => 2,
            _ => 0,
        }
    }

    /// Centre-out drags (everything but Stream, among the generators).
    pub fn radial(self) -> bool {
        matches!(
            self,
            FigureMode::Focus | FigureMode::Urchin | FigureMode::SolidFlash
        )
    }
}

impl FigureMode {
    pub fn label(self) -> &'static str {
        match self {
            FigureMode::Line => "Straight line",
            FigureMode::Rect => "Rectangle",
            FigureMode::Ellipse => "Ellipse",
            FigureMode::Polygon => "Polygon",
            FigureMode::Curve => "Continuous curve",
            FigureMode::Arc => "Curve",
            FigureMode::Stream => "Stream line",
            FigureMode::Focus => "Saturated line",
            FigureMode::Urchin => "Sea urchin flash",
            FigureMode::SolidFlash => "Solid flash",
            FigureMode::Smart => "Smart shape",
        }
    }

    /// Row 157 / `FG-011`: does this sub tool offer "Adjust angle after
    /// fixed" — the optional second stage that spins the finished shape
    /// before it inks? Only the two DRAGGED closed shapes. A straight line's
    /// angle already came from the drag, the click-list gestures have no
    /// "after the size is fixed" moment, and the generators place a layer.
    pub fn can_adjust_angle(self) -> bool {
        matches!(self, FigureMode::Rect | FigureMode::Ellipse)
    }

    /// What Shift means for this sub tool's drag. CSP splits it: on the
    /// Straight line it "rotates in increments of 45 degrees", but on the
    /// Rectangle it makes "a perfect square" and on the Ellipse "a perfect
    /// circle" — a constraint on the two SIDES, not on the diagonal's
    /// angle.
    ///
    /// They are not the same operation and swapping one for the other is
    /// not a rounding difference: snapping a box's diagonal to the nearest
    /// 45° octant collapses the box whenever the drag lies nearer an axis
    /// than the diagonal. A 22° drag lands on 0°, and the "square" the
    /// artist asked for inks as a zero-height LINE (rendered proof:
    /// `g03-circle.png`, a flat bar where a circle was drawn).
    pub fn shift_keeps_aspect(self) -> bool {
        matches!(self, FigureMode::Rect | FigureMode::Ellipse)
    }

    /// What the History palette calls a FILLED figure — the one step the
    /// fill and the outline are folded into.
    pub fn undo_label(self) -> &'static str {
        match self {
            FigureMode::Line => "Line",
            FigureMode::Rect => "Rectangle",
            FigureMode::Ellipse => "Ellipse",
            FigureMode::Polygon => "Polygon",
            FigureMode::Curve | FigureMode::Arc => "Curve",
            FigureMode::Smart => "Shape",
            FigureMode::Stream => "Speed lines",
            FigureMode::Focus => "Focus lines",
            FigureMode::Urchin | FigureMode::SolidFlash => "Flash",
        }
    }
}

/// One placed point of an in-progress Polygon / Continuous-curve click list
/// (`FG-003`/`FG-004`), with the one bit `FG-016` needs on top of the
/// coordinate: is this a CORNER?
///
/// The flag lives on the point rather than in a parallel `Vec<bool>` beside
/// `App::figure_poly` on purpose. `FG-013` inserts and deletes points in the
/// middle of the list mid-draw, and every index-keyed side table would have
/// to be re-indexed at each of those edits — one missed site and the creases
/// silently walk to the wrong anchors, with nothing to catch it but the eye.
/// Carrying the bit inside the element makes that class of bug unwritable.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FigureAnchor {
    /// Canvas px.
    pub p: (f32, f32),
    /// `FG-016`: Alt+tap flips this. Meaningful only on the Continuous
    /// curve — a Polygon is corners all the way down — and only on INTERIOR
    /// anchors, since the spline's ends are one-sided already
    /// (`balloon::tessellate_open_corners`). Both no-ops are harmless, which
    /// is why the flag is allowed to be set anywhere: the artist marks the
    /// point they just placed, and it becomes interior on the next click.
    pub corner: bool,
}

impl FigureAnchor {
    /// A freshly clicked point: smooth, because the ordinary curve is the
    /// one people draw and the crease is the thing you ask for.
    pub fn new(x: f32, y: f32) -> Self {
        Self {
            p: (x, y),
            corner: false,
        }
    }
}

/// Row 157: what the SECOND stage of a two-stage figure gesture is steering.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FigureStage2Kind {
    /// `FG-002`: the pointer is a point ON the curve; the baseline bends
    /// through it.
    Bend,
    /// `FG-011`: the pointer is where the dragged corner should end up; the
    /// whole shape spins about its centre to follow.
    Angle,
}

/// Row 157: the live state of a figure gesture's SECOND stage — the size
/// drag is over, `a`/`b` are frozen, and the pointer now steers one more
/// parameter until a click (or Enter) commits and Esc throws it away.
///
/// It is a SEPARATE field from `App::figure_drag` rather than a variant
/// inside it, and the two are mutually exclusive by construction: the
/// release `take()`s the drag and only then may set this. Keeping them apart
/// means every mode that has no second stage — which is most of them, and
/// all of the generators — runs exactly the one-stage path it always did.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FigureStage2 {
    /// Where stage one's drag began, canvas px.
    pub a: (f32, f32),
    /// Where it ended. Frozen: stage two never resizes.
    pub b: (f32, f32),
    /// The live pointer. Seeded so that committing without moving reproduces
    /// stage one exactly (baseline midpoint for Bend, `b` for Angle).
    pub cur: (f32, f32),
    pub kind: FigureStage2Kind,
    /// Shift held on the last pointer move — snaps the Angle stage to 15°
    /// steps. Sampled on the move like the balloon object drag does, because
    /// a commit arriving by key has no modifier state of its own.
    pub shift: bool,
}

impl FigureStage2 {
    /// The Angle stage's rotation about the shape's centre, radians: the
    /// turn that carries the dragged corner `b` onto the pointer. Zero at
    /// the seeded `cur == b`, so a click that never moved inks the unrotated
    /// shape.
    pub fn angle(&self) -> f32 {
        let c = ((self.a.0 + self.b.0) * 0.5, (self.a.1 + self.b.1) * 0.5);
        let base = (self.b.1 - c.1).atan2(self.b.0 - c.0);
        let now = (self.cur.1 - c.1).atan2(self.cur.0 - c.0);
        let d = now - base;
        if self.shift {
            const STEP: f32 = std::f32::consts::PI / 12.0; // 15°
            (d / STEP).round() * STEP
        } else {
            d
        }
    }
}

/// Tool-side parameters for Figure ▸ Stream/Saturated line — what the NEXT
/// drag generates with (the drag itself supplies the geometry: center and
/// radius, or angle and length). One struct serves both modes; Stream
/// ignores `jitter`/`r_in_frac` (the speed renderer has no use for either)
/// and the radial modes ignore `taper`. `seed` bumps after every placement
/// so consecutive drags differ without losing determinism.
///
/// The flash modes share `figure_focus` with Saturated line: they are the
/// same centre-out gesture with the same four knobs, and every sub tool
/// row writes its own preset values on the way in.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FigureLineOpts {
    pub count: u32,
    pub width: f32,
    pub jitter: f32,
    /// Focus/flash only: the empty middle, as a fraction of the radius.
    pub r_in_frac: f32,
    /// Stream only: [`mn_core::genlines::SpeedLinesParams::taper`]. 0 is
    /// the pre-2026-08-22 look and stays the default — an existing drag
    /// must not change shape because the knob appeared.
    pub taper: f32,
    /// Radial: the angular gap in degrees ([`mn_core::genlines::
    /// GenLinesSpec::gap_deg`]). >0 drives `count`.
    pub gap_deg: f32,
    /// Stream: the spacing between runs in canvas px, and the bundling
    /// around it (see the same-named `GenLinesSpec` fields).
    pub gap_px: f32,
    pub group: u32,
    pub group_gap: f32,
    pub jit_gap: f32,
    pub jit_len: f32,
    pub jit_width: f32,
    pub seed: u64,
}

impl FigureLineOpts {
    /// The sub tool presets, in the units a manga tutorial states them in
    /// — an ANGULAR gap for 集中線 (CSP's own rule of thumb: ≈3° for a
    /// dense burst, ≈10° for a sparse one) and MILLIMETRES for a 流線
    /// spacing and every line width. Both are the units that mean the
    /// same thing on a 600 dpi B4 and a 72 dpi draft; a count and a pixel
    /// width are not, which is half of why the generated sets have never
    /// looked like the reference pages.
    ///
    /// `dpi` is the caller's `tone_dpi()` — the page's, or the manga
    /// standard 600 for a pixel canvas (at 96 a 0.2 mm line rounds to
    /// under one pixel and the whole set turns to hairline noise).
    fn from_mm(dpi: u32, width_mm: f32, gap_mm: f32) -> Self {
        let px = |mm: f32| mm / 25.4 * dpi as f32;
        Self {
            count: 60,
            width: px(width_mm).max(0.5),
            jitter: 0.0,
            r_in_frac: 0.0,
            taper: 0.5,
            gap_deg: 0.0,
            gap_px: px(gap_mm),
            group: 0,
            group_gap: 0.0,
            jit_gap: 0.0,
            jit_len: 0.0,
            jit_width: 0.0,
            seed: 1,
        }
    }

    // Default tapers are NOT 0: a flat-width effect line is the "flat
    // noise field" the pro-page audit flagged — printed 流線/集中線 thin
    // to needles. Tool defaults are free to be right (nothing saved
    // regenerates through them; the 0-means-legacy rule guards SPECS).
    pub fn stream_default() -> Self {
        Self::stream_dpi(600)
    }
    pub fn focus_default() -> Self {
        Self::focus_dpi(600)
    }

    /// 流線, the everyday one: 1 mm between runs, 0.20 mm wide, in
    /// bundles of 4 with a two-and-a-half-gap hole between bundles
    /// (まとまり — the single biggest quality lever a generated speed
    /// block has, and the thing the uniform scatter could not express).
    pub fn stream_dpi(dpi: u32) -> Self {
        Self {
            group: 4,
            group_gap: 2.5,
            jit_gap: 0.25,
            jit_len: 0.3,
            jit_width: 0.25,
            ..Self::from_mm(dpi, 0.20, 1.0)
        }
    }

    /// A tighter 流線 block: 0.6 mm gap, 0.15 mm lines, bundles of 6.
    pub fn dense_stream_dpi(dpi: u32) -> Self {
        Self {
            group: 6,
            group_gap: 2.0,
            jit_gap: 0.25,
            jit_len: 0.3,
            jit_width: 0.25,
            ..Self::from_mm(dpi, 0.15, 0.6)
        }
    }

    /// A sparse 流線 block — the same rule read the other way: gaps you
    /// can see between the runs, so no bundling on top of them.
    pub fn sparse_stream_dpi(dpi: u32) -> Self {
        Self {
            jit_gap: 0.3,
            jit_len: 0.35,
            jit_width: 0.25,
            ..Self::from_mm(dpi, 0.30, 2.5)
        }
    }

    /// 集中線: a 3.5° gap, 0.35 mm rays needling to the convergence, and
    /// a 40 % hole for the art.
    pub fn focus_dpi(dpi: u32) -> Self {
        Self {
            gap_deg: 3.5,
            gap_px: 0.0,
            taper: 0.6,
            r_in_frac: 0.40,
            jitter: 0.25,
            jit_gap: 0.25,
            jit_len: 0.25,
            jit_width: 0.3,
            ..Self::from_mm(dpi, 0.35, 0.0)
        }
    }

    /// The dense end of the 3°/10° rule: a 2° gap on 0.25 mm rays.
    pub fn dense_focus_dpi(dpi: u32) -> Self {
        Self {
            gap_deg: 2.0,
            ..Self::focus_dpi(dpi)
        }
    }

    /// A black burst: rays at a 1.2° gap, twice the weight, small hole.
    pub fn dark_burst_dpi(dpi: u32) -> Self {
        Self {
            gap_deg: 1.2,
            r_in_frac: 0.22,
            jitter: 0.5,
            width: (0.7 / 25.4 * dpi as f32).max(0.5),
            ..Self::focus_dpi(dpi)
        }
    }

    /// The two flash kinds ride `figure_focus` (same gesture, same
    /// knobs), but their `width` is a spike BASE in px and their teeth
    /// are counted, not gapped — so they keep the count-driven preset.
    pub fn flash_dpi(dpi: u32, count: u32, width_mm: f32, r_in_frac: f32) -> Self {
        Self {
            count,
            gap_deg: 0.0,
            jitter: 0.25,
            r_in_frac,
            taper: 0.0,
            ..Self::from_mm(dpi, width_mm, 0.0)
        }
    }

    /// Do two presets describe the same set? The seed rerolls on every
    /// placement, so it can never take part in "is this row armed".
    pub fn same_as(&self, other: &Self) -> bool {
        Self { seed: 0, ..*self } == Self { seed: 0, ..*other }
    }
}

/// Gradient-tool colour modes (CSP's three), plus `FI-050`'s freeform.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GradMode {
    /// Main colour fades into the sub colour.
    FgToBg,
    /// Main colour fades out to transparent.
    FgToTransparent,
    /// Transparent fades into the main colour.
    TransparentToFg,
    /// `FI-050`/`FI-051`: DRAWN guide lines instead of one straight drag —
    /// the main colour on the first, the sub colour on the second, and the
    /// ramp between them follows both shapes. A third line and up carries
    /// the main colour as it stands when that line is drawn, and the field
    /// blends by proximity. Takes several strokes, so it is the one gradient
    /// mode with a staged gesture ([`GradFree`]).
    Freeform,
}

impl GradMode {
    pub fn label(self) -> &'static str {
        match self {
            GradMode::FgToBg => "Main → Sub",
            GradMode::FgToTransparent => "Main → Transparent",
            GradMode::TransparentToFg => "Transparent → Main",
            GradMode::Freeform => "Freeform (drawn lines)",
        }
    }

    /// Does this mode take a two-stroke gesture rather than one drag?
    pub fn is_freeform(self) -> bool {
        self == GradMode::Freeform
    }
}

/// `FI-050`/`FI-051`: the live state of a freeform gradient's multi-stroke
/// gesture — the same shape as [`FigureStage2`], for the same reason. It is
/// a SEPARATE field from `App::grad_drag` and the two are mutually exclusive
/// by construction (the press picks one on `grad_mode`), so the three
/// one-drag modes run exactly the path they always did.
///
/// Nothing here is history: no pixels move until the gesture is COMMITTED
/// (Enter, or a click away from the last line), so Esc or a tool switch
/// throws the whole thing away with nothing to undo.
///
/// **The colour rides with the line.** Each guide records the colour it will
/// lay down at the moment it is drawn — guide 1 the main colour, guide 2 the
/// sub colour (so the two-line ramp is exactly what it always was), guide 3
/// and up the main colour as it stands then. That is what lets the artist
/// pick a colour, draw a line, pick another, draw another; and it is what
/// makes the overlay preview honest, since it draws each finished guide in
/// the colour that guide is actually carrying.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GradFree {
    /// The guides drawn so far, in order, each with its colour. Empty while
    /// the first stroke is still down.
    pub done: Vec<mn_core::freeform::ColourGuide>,
    /// The stroke under the pointer right now. Empty between strokes.
    pub cur: Vec<[f32; 2]>,
    /// Is a stroke in progress? Distinguishes "waiting for the next press"
    /// from "drawing", which `cur` alone cannot.
    pub drawing: bool,
}

impl GradFree {
    /// Enough lines to paint? Two is the ramp, three and up the
    /// colour-per-guide field.
    pub fn ready(&self) -> bool {
        self.done.len() >= 2
    }
}

/// What a pending ruler drag creates (TODO #3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RulerKind {
    Line,
    VanishingPoint,
    /// Part 2: a click-vertex polyline (double-click or Enter closes).
    Curve,
    /// Part 3 (RL-014): drag the direction — every stroke comes out
    /// parallel to it.
    Parallel,
    /// CSP "Special ruler ▸ Radial line" (集中線): CLICK places the
    /// centre, and every stroke afterwards runs along the line through it
    /// — a continuum, unlike the vanishing point's ray fan.
    Radial,
    /// Part 3 (RL-019): CLICK places a free-radius ring ruler (CSP's own
    /// concentric circle ruler: the stroke keeps the radius it started
    /// at); DRAG instead and the drag length becomes a ring spacing to
    /// quantize onto.
    Concentric,
    /// Part 3 (RL-021): drag from the centre outward — centre at the
    /// press, first axis along the drag. Line count via the menu ladder.
    Symmetric,
    /// Part 4 (RL-060/061, P-001..010): drag the EYE LEVEL — the two
    /// ends become the horizon VPs of a 2-point set; strokes bind by
    /// direction to rays through either VP or the verticals.
    Perspective,
    /// One-point: drag FROM the vanishing point along the eye level — the
    /// press is the VP, the release the horizon handle (which tilts the
    /// horizontals and verticals when dragged later).
    Perspective1,
    /// Three-point: the same eye-level drag as the 2-point set, plus a
    /// third (vertical) VP placed off the horizon on the side the drag
    /// points to — left→right puts it below (high angle), right→left above
    /// (low angle). It is anchor 2, so the Object tool drags it home.
    Perspective3,
    /// Part 3 (RL-020): a click places a full-canvas horizontal/vertical
    /// guide at the clicked coordinate.
    GuideH,
    GuideV,
}

/// Selection sub-modes (CSP: Rectangle / Lasso sub tools).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SelectMode {
    Rect,
    /// CSP 楕円選択 Ellipse: the same diagonal drag as Rect, but the
    /// selection is the ellipse inscribed in it — the sub tool CSP ships
    /// beside Rectangle, and the shape a face, a moon or a spotlight is.
    Ellipse,
    Lasso,
    /// L-001/L-002 magnetic lasso: the outline snaps to the nearest strong
    /// edge as you trace, so a character is selected by roughly following
    /// its lineart instead of precisely. Click places an anchor, drag
    /// traces and anchors as it goes, Backspace takes the last anchor back,
    /// Enter (or a click on the first anchor) closes. See
    /// [`mn_core::magnetic`].
    Magnetic,
    /// SE-020 (CSP 選択範囲シュリンク): freehand drag through the empty
    // space — every closed area the path crosses floods to its barriers
    // (the flats grabber; see fill::magic_select_path).
    Shrink,
}

/// Fill-tool sub tools. CSP ships five; the difference between them is how
/// you AIM the same flood. Ours: the three 参照 variants are one Click mode
/// plus [`mn_core::FillRefer`], and the two path-aimed ones are here.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FillMode {
    /// The classic bucket — click an area, `FillOpts::refer` decides what
    /// counts as a wall (CSP's "Refer other layers" / "editing layer" /
    /// "reference layer" sub tools).
    #[default]
    Click,
    /// FI-003 Enclose and fill (CSP 囲って塗る): lasso roughly AROUND a
    /// messy region and every closed area inside it fills at once — the
    /// flatting workhorse. See `fill::enclose_and_fill`.
    Enclose,
    /// FI-004 Lasso fill (CSP 塗りつぶし・投げなわ): fill the lassoed shape
    /// ITSELF, boundaries ignored — colour blocking and shadow shapes.
    Lasso,
    /// Row 119 / FI-005 Leftover pen (CSP 塗り残し部分に塗る): scrub across
    /// finished colour and only the still-EMPTY enclosed pockets under the
    /// drag fill. A filter shaped like a brush — see `fill::leftover_fill`.
    Leftover,
    /// Row 160 / `RD-001`–`RD-009` Remove dust (CSP ゴミ取り): drag around a
    /// patch and every blob under the size threshold in it is cleaned —
    /// specks removed, or transparent pinholes plugged, per
    /// [`mn_core::DustMode`]. See `mn_core::dust`.
    ///
    /// **Placement call (2026-08-29).** CSP hangs this off a 線修正
    /// "Correct line" tool group we do not have, next to a 塗り残し部分に塗る
    /// (RD-005) we already ship as [`FillMode::Leftover`]. It lands here
    /// instead, for the house reason sub tools always fold: the gesture is
    /// the fill family's own freehand drag, the machinery is the fill
    /// subsystem's, and the Tool Property it needs is the Fill panel plus
    /// two rows. RD-007's "Select dust" folds one level further — it is
    /// the [`mn_core::DustMode`] rows plus this sub tool's "Select instead
    /// of cleaning" switch, not a fifth Selection sub tool, because the
    /// detection and the window are identical and only the verb differs.
    Dust,
}

impl FillMode {
    pub fn label(self) -> &'static str {
        match self {
            FillMode::Click => "Fill",
            FillMode::Enclose => "Enclose and fill",
            FillMode::Lasso => "Lasso fill",
            FillMode::Leftover => "Leftover pen",
            FillMode::Dust => "Remove dust",
        }
    }
}

/// The Remove-dust sub tool's Tool Property state (RD-002/RD-003/RD-009).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DustOpts {
    /// RD-002 "Dust size": the largest blob still counted as dust, in
    /// pixels of AREA — the unit LC-001's menu filter already uses, and the
    /// unit the row says out loud.
    pub max_px: u32,
    /// RD-003: which of the four definitions of "dust" this drag means.
    pub mode: mn_core::DustMode,
    /// RD-007: hand the detection back as a SELECTION instead of acting on
    /// it, so you can look before you delete.
    pub select: bool,
}

impl Default for DustOpts {
    fn default() -> Self {
        Self {
            // CSP ships ゴミ取り at a small default and so does our menu
            // filter — the same 5, spelled once more here because a tool's
            // Tool Property and a menu seed are separate promises
            // (`mn_core::Filter::REMOVE_DUST` is the menu's).
            max_px: 5,
            mode: mn_core::DustMode::default(),
            select: false,
        }
    }
}

/// The Tone tool's Tool Property: the screen it lays down, and how the
/// click finds the region to lay it on.
///
/// `region` is a [`mn_core::FillOpts`] on purpose — the gesture runs the
/// Fill tool's own `flood_region`, so tolerance / gap closing / area
/// scaling / 参照 mean exactly what they mean under the bucket. It is a
/// SEPARATE copy from `App::fill_opts`: the tone gesture wants gap closing
/// cranked up (sketch lines leak) far more often than a flat fill does.
#[derive(Clone, Copy, Debug)]
pub struct ToneToolOpts {
    /// The screen itself. `ToneParams::density` is not used: a live tone
    /// layer's coverage is `ToneDensity::Specified(density)`, written by
    /// `fill_layer::build_fill_tile`, so the field below is the one knob.
    pub tone: mn_core::tone::ToneParams,
    /// Ink coverage 0..=1 the screen prints at (CSP's tone density).
    pub density: f32,
    pub region: mn_core::FillOpts,
}

impl Default for ToneToolOpts {
    fn default() -> Self {
        Self {
            tone: mn_core::tone::ToneParams::default(),
            // A visible SCREEN, not a black fill: 100% coverage defeats the
            // point of one-click tone (readable halftone is the product).
            density: 0.4,
            // Lineart that a bucket fill would leak through is the normal
            // case for a tone gesture, so the default seals harder than
            // `FillOpts::default()`'s 2 px.
            region: mn_core::FillOpts {
                gap_close_px: 3,
                ..mn_core::FillOpts::default()
            },
        }
    }
}

/// Frame-tool sub tools (CSP: Create frame / Cut frame border groups).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameMode {
    /// CSP "Rectangle frame": drag out a new rectangle frame border folder.
    Rect,
    /// CSP "Polyline frame": click vertices, close on the first one / Enter.
    Polyline,
    /// CSP "Frame border pen": freehand-draw the panel outline.
    Pen,
    /// CSP "Divide frame folder": the cut panel splits off into a NEW frame
    /// border folder with its own White + draw layer.
    DivideFolder,
    /// CSP "Divide frame border": the cut stays inside the same folder.
    DivideBorder,
}

impl FrameMode {
    pub fn creates(self) -> bool {
        matches!(self, FrameMode::Rect | FrameMode::Polyline | FrameMode::Pen)
    }
}

/// `FB-026`/`FB-022` (TRIAGE 128) — what happens to a panel's CONTENTS when
/// the panel is cut. CSP makes you say which, and so do we: cutting a panel
/// that already has art in it has three defensible answers and no default
/// that is right twice in a row.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DivideContents {
    /// CSP "Create empty folder": the new half gets a fresh White + empty
    /// draw layer. Our behaviour before this option existed.
    CreateEmpty,
    /// CSP "Duplicate layer": the new half gets a COPY of the folder's
    /// contents, so the drawing survives in both halves, each masked to its
    /// own shape. The DEFAULT because it is the owner's own CSP setting
    /// (csp-tools.json `CutFrameCutFolderType = 0`) and it never loses
    /// work — an empty half is one Delete away, a lost half is not.
    #[default]
    Duplicate,
    /// CSP "Do not change": draw the border only — no new folder, the art
    /// and the layer structure are untouched. Identical in effect to the
    /// Divide-frame-border sub tool, reachable without switching sub tools.
    DoNotChange,
}

impl DivideContents {
    pub const ALL: [DivideContents; 3] = [
        DivideContents::CreateEmpty,
        DivideContents::Duplicate,
        DivideContents::DoNotChange,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DivideContents::CreateEmpty => "Create empty folder",
            DivideContents::Duplicate => "Duplicate layer",
            DivideContents::DoNotChange => "Do not change",
        }
    }
}

/// Move-tool sub tools (CSP Move: Hand / Rotate).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanMode {
    Hand,
    /// Drag rotates the view around the canvas-area centre.
    Rotate,
}

/// Eyedropper Tool Property (CSP: Reference / Average color / Show color
/// picker circle). Lives on `App` beside `fill_opts`/`wand_opts` rather than
/// riding `AppCmd::PickColor`, because it is sub-tool memory: the Alt+click
/// gesture from a brush must obey the same settings the tool does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EyedropOpts {
    /// Which layers the sample walks. Deliberately the FILL tool's 参照
    /// vocabulary (`mn_core::FillRefer`) — composite / editing layer /
    /// reference set — so the two tools cannot mean different things by
    /// "refer reference layer".
    pub refer: mn_core::FillRefer,
    /// Side of the averaging box, in canvas pixels (CSP's 1×1 / 2×2 / 3×3 /
    /// 5×5). 1 is the default and is the single-pixel pick, byte for byte.
    pub size: u32,
    /// Paint the picker ring under the pen while the Eyedropper tool is up.
    pub circle: bool,
}

impl Default for EyedropOpts {
    fn default() -> Self {
        Self {
            refer: mn_core::FillRefer::All,
            size: 1,
            circle: true,
        }
    }
}

/// Row 78 (CSP Operation ▸ Object ▸ Select): how a plain object CLICK
/// combines with the current selection. Shift-click is always Add.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SelectCombine {
    #[default]
    New,
    Add,
    Remove,
    Toggle,
}

impl SelectCombine {
    pub fn label(&self) -> &'static str {
        match self {
            SelectCombine::New => "New",
            SelectCombine::Add => "Add",
            SelectCombine::Remove => "Remove",
            SelectCombine::Toggle => "Toggle",
        }
    }
}

/// Operation-tool sub tools (CSP 操作: Object / Select layer).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ObjectMode {
    /// Select and reshape frames, balloons and text boxes.
    #[default]
    Object,
    /// S-001 CSP レイヤー選択: click a pixel and the Layers palette jumps to
    /// whichever layer drew it. On a 200-layer page that is how you find
    /// things.
    PickLayer,
}

impl ObjectMode {
    pub fn label(self) -> &'static str {
        match self {
            ObjectMode::Object => "Object",
            ObjectMode::PickLayer => "Select layer",
        }
    }
}

/// S-001's exclusions (CSP's 選択しないレイヤー checkbox block): the kinds of
/// layer the pick must never land on. Defaults are CSP's — the four the
/// owner would otherwise keep clicking past on a finished page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PickExclude {
    /// Rough underdrawing (CSP 下書き).
    pub draft: bool,
    pub text: bool,
    pub locked: bool,
    /// LIVE fill/gradient/tone layers — the flats, which cover everything.
    pub fill: bool,
}

impl Default for PickExclude {
    fn default() -> Self {
        Self {
            draft: true,
            text: true,
            locked: true,
            fill: false,
        }
    }
}

/// Balloon-tool sub-modes (CSP: Ellipse / Rounded / Balloon pen / Balloon tail).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BalloonMode {
    Ellipse,
    Round,
    /// Freehand-drawn body, simplified to an editable polygon on release.
    Draw,
    /// Drag from inside a balloon out to the tail's tip.
    Tail,
}

impl BalloonMode {
    pub fn label(self) -> &'static str {
        match self {
            BalloonMode::Ellipse => "Ellipse balloon",
            BalloonMode::Round => "Rounded balloon",
            BalloonMode::Draw => "Balloon pen",
            BalloonMode::Tail => "Balloon tail",
        }
    }
}

/// One row of the Sub Tool list, as a command: the TOOL and the sub tool
/// inside it, picked in one press. The list itself (`ui/subtool.rs`) sets
/// these same fields directly — this enum is the door the Ctrl+K palette
/// (and anything else that has only a command to push) uses, so a new sub
/// tool row wants an arm here too or it stays unreachable by search. The
/// brush presets are NOT here: they are their own rows, on `SelectBrush`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SubTool {
    /// The three 参照 rows, which also return to the click sub tool.
    FillRefer(mn_core::FillRefer),
    Fill(FillMode),
    /// The Tone tool's sub tools are the nine screen SHAPES — that is the
    /// choice you make before the click, and the rest of the screen (LPI,
    /// angle, density) is Tool Property.
    Tone(mn_core::tone::TonePattern),
    Wand(mn_core::FillRefer),
    Select(SelectMode),
    /// The Selection tool's PAINT sub tools (CSP 選択ペン / 選択消し). Not a
    /// mode enum: each is a fixed create-type that switches `Tool`, because
    /// the canvas stroke paths key off `Tool::SelPen`/`SelEraser`. A future
    /// "Selection pen (soft)" is one more variant here, one row there.
    SelectPen,
    SelectEraser,
    Frame(FrameMode),
    Balloon(BalloonMode),
    Text,
    Object(ObjectMode),
    Figure(FigureMode),
    Gradient(GradMode),
    Eyedrop(mn_core::FillRefer),
    /// E-016 平均色: the eyedropper's sample SIZE in pixels a side — its own
    /// group in the list, and its own rows so a shortcut can name one.
    EyedropSize(u32),
    Pan(PanMode),
}

impl SubTool {
    /// Every sub tool the list offers, in the list's own order.
    pub const ALL: &'static [SubTool] = {
        use mn_core::FillRefer::{Active, All, Reference};
        use mn_core::tone::TonePattern as P;
        &[
            SubTool::FillRefer(All),
            SubTool::FillRefer(Active),
            SubTool::FillRefer(Reference),
            SubTool::Fill(FillMode::Enclose),
            SubTool::Fill(FillMode::Lasso),
            SubTool::Fill(FillMode::Leftover),
            SubTool::Fill(FillMode::Dust),
            SubTool::Tone(P::Dots),
            SubTool::Tone(P::Lines),
            SubTool::Tone(P::Square),
            SubTool::Tone(P::Ellipse),
            SubTool::Tone(P::Lozenge),
            SubTool::Tone(P::Cross),
            SubTool::Tone(P::Noise),
            SubTool::Tone(P::Asterisk),
            SubTool::Tone(P::Star),
            SubTool::Wand(All),
            SubTool::Wand(Active),
            SubTool::Wand(Reference),
            SubTool::Select(SelectMode::Rect),
            SubTool::Select(SelectMode::Ellipse),
            SubTool::Select(SelectMode::Lasso),
            SubTool::Select(SelectMode::Magnetic),
            SubTool::Select(SelectMode::Shrink),
            SubTool::SelectPen,
            SubTool::SelectEraser,
            SubTool::Frame(FrameMode::Rect),
            SubTool::Frame(FrameMode::Polyline),
            SubTool::Frame(FrameMode::Pen),
            SubTool::Frame(FrameMode::DivideFolder),
            SubTool::Frame(FrameMode::DivideBorder),
            SubTool::Balloon(BalloonMode::Ellipse),
            SubTool::Balloon(BalloonMode::Round),
            SubTool::Balloon(BalloonMode::Draw),
            SubTool::Balloon(BalloonMode::Tail),
            SubTool::Text,
            SubTool::Object(ObjectMode::Object),
            SubTool::Object(ObjectMode::PickLayer),
            SubTool::Figure(FigureMode::Line),
            SubTool::Figure(FigureMode::Rect),
            SubTool::Figure(FigureMode::Ellipse),
            SubTool::Figure(FigureMode::Polygon),
            SubTool::Figure(FigureMode::Arc),
            SubTool::Figure(FigureMode::Curve),
            SubTool::Figure(FigureMode::Smart),
            SubTool::Figure(FigureMode::Stream),
            SubTool::Figure(FigureMode::Focus),
            SubTool::Figure(FigureMode::Urchin),
            SubTool::Figure(FigureMode::SolidFlash),
            SubTool::Gradient(GradMode::FgToBg),
            SubTool::Gradient(GradMode::FgToTransparent),
            SubTool::Gradient(GradMode::TransparentToFg),
            SubTool::Gradient(GradMode::Freeform),
            SubTool::Eyedrop(All),
            SubTool::Eyedrop(Active),
            SubTool::Eyedrop(Reference),
            SubTool::EyedropSize(1),
            SubTool::EyedropSize(2),
            SubTool::EyedropSize(3),
            SubTool::EyedropSize(5),
            SubTool::Pan(PanMode::Hand),
            SubTool::Pan(PanMode::Rotate),
        ]
    };

    /// The tool this sub tool lives under — picking it switches tools too.
    pub fn tool(self) -> Tool {
        match self {
            SubTool::FillRefer(_) | SubTool::Fill(_) => Tool::Fill,
            SubTool::Tone(_) => Tool::Tone,
            SubTool::Wand(_) => Tool::Wand,
            SubTool::Select(_) => Tool::Select,
            SubTool::SelectPen => Tool::SelPen,
            SubTool::SelectEraser => Tool::SelEraser,
            SubTool::Frame(_) => Tool::Frame,
            SubTool::Balloon(_) => Tool::Balloon,
            SubTool::Text => Tool::Text,
            SubTool::Object(_) => Tool::Object,
            SubTool::Figure(_) => Tool::Figure,
            SubTool::Gradient(_) => Tool::Gradient,
            SubTool::Eyedrop(_) | SubTool::EyedropSize(_) => Tool::Eyedrop,
            SubTool::Pan(_) => Tool::Pan,
        }
    }

    /// The Sub Tool list's own row text.
    pub fn label(self) -> &'static str {
        use mn_core::FillRefer::{Active, All, Reference};
        match self {
            SubTool::FillRefer(All) => "Refer other layers",
            SubTool::FillRefer(Active) => "Refer editing layer only",
            SubTool::FillRefer(Reference) => "Refer reference layer",
            SubTool::Fill(FillMode::Click) => "Fill",
            SubTool::Fill(FillMode::Enclose) => "Enclose and fill",
            SubTool::Fill(FillMode::Lasso) => "Lasso fill",
            SubTool::Fill(FillMode::Leftover) => "Leftover pen",
            SubTool::Fill(FillMode::Dust) => "Remove dust",
            SubTool::Tone(p) => p.label(),
            SubTool::Wand(All) => "Refer all layers",
            SubTool::Wand(Active) => "Refer editing layer only",
            SubTool::Wand(Reference) => "Refer reference layer",
            SubTool::Select(SelectMode::Rect) => "Rectangle",
            SubTool::Select(SelectMode::Ellipse) => "Ellipse",
            SubTool::Select(SelectMode::Lasso) => "Lasso",
            SubTool::Select(SelectMode::Magnetic) => "Magnetic lasso",
            SubTool::Select(SelectMode::Shrink) => "Shrink selection",
            SubTool::SelectPen => "Selection pen",
            SubTool::SelectEraser => "Erase selection",
            SubTool::Frame(FrameMode::Rect) => "Rectangle frame",
            SubTool::Frame(FrameMode::Polyline) => "Polyline frame",
            SubTool::Frame(FrameMode::Pen) => "Frame border pen",
            SubTool::Frame(FrameMode::DivideFolder) => "Divide frame folder",
            SubTool::Frame(FrameMode::DivideBorder) => "Divide frame border",
            SubTool::Balloon(m) => m.label(),
            SubTool::Text => "Text",
            SubTool::Object(m) => m.label(),
            SubTool::Figure(m) => m.label(),
            SubTool::Gradient(m) => m.label(),
            SubTool::Eyedrop(All) => "Pick displayed color",
            SubTool::Eyedrop(Active) => "Pick color from layer",
            SubTool::Eyedrop(Reference) => "Pick from reference layers",
            // The list's own row text, spelled out rather than formatted:
            // these labels are what `keys.json` and the palette match on.
            SubTool::EyedropSize(2) => "2 × 2",
            SubTool::EyedropSize(3) => "3 × 3",
            SubTool::EyedropSize(5) => "5 × 5",
            SubTool::EyedropSize(_) => "1 × 1 (one pixel)",
            SubTool::Pan(PanMode::Hand) => "Hand",
            SubTool::Pan(PanMode::Rotate) => "Rotate",
        }
    }

    /// Where the row lives, for the palette's weak right-hand text — the
    /// Sub Tool list's own group caption, so searching the GROUP name
    /// ("selection", "balloon") lists that tool's whole family.
    pub fn path(self) -> &'static str {
        match self {
            SubTool::FillRefer(_) | SubTool::Fill(_) => "Sub Tool ▸ Fill",
            SubTool::Tone(_) => "Sub Tool ▸ Tone",
            SubTool::Wand(_) => "Sub Tool ▸ Auto select",
            SubTool::Select(_) | SubTool::SelectPen | SubTool::SelectEraser => {
                "Sub Tool ▸ Selection"
            }
            SubTool::Frame(_) => "Sub Tool ▸ Frame border",
            SubTool::Balloon(_) => "Sub Tool ▸ Balloon",
            SubTool::Text => "Sub Tool ▸ Text",
            SubTool::Object(_) => "Sub Tool ▸ Operation",
            SubTool::Figure(_) => "Sub Tool ▸ Figure",
            SubTool::Gradient(_) => "Sub Tool ▸ Gradient",
            SubTool::Eyedrop(_) | SubTool::EyedropSize(_) => "Sub Tool ▸ Eyedropper",
            SubTool::Pan(_) => "Sub Tool ▸ Move",
        }
    }
}

/// CSP's three drawing-colour slots. `Transparent` is a *colour*, not a tool:
/// drawing with it erases using the current brush's own dab shape/texture
/// (spec section B finding #1 — the single highest-value CSP behaviour).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Main,
    Sub,
    Transparent,
}

/// Per-sub-tool settings, CSP's Tool Property memory model: "settings are saved
/// for the next time you use the sub tool". Keyed by preset path in
/// `App::props`; the active copy lives in `App::props_current`.
#[derive(Clone, Copy, Debug)]
pub struct ToolProps {
    /// The dab DIAMETER in canvas px — ABSOLUTE, the number artists think in
    /// and the one the `[`/`]` ladder steps through. Seeded from the preset's
    /// own size on a first encounter (`App::load_props_for`), which makes that
    /// size a DEFAULT and not a ceiling.
    ///
    /// Renamed from `size`, which was a 0.25..4 multiplier: a stored `2.0`
    /// means something completely different now, and a same-named field would
    /// have read the old meaning as 2 px without a word.
    pub size_px: f32,
    /// Absolute brush opacity 0..1 (libmypaint `opaque`).
    pub opacity: f32,
    /// Floor of the pressure→size curve, % of base radius. The owner's CSP
    /// G-Pen runs at 3%; MyPaint's `pen.myb` floor is much higher, which is
    /// exactly the "minimum is a bit high" complaint.
    pub min_size: f32,
    /// Pull-string stabilizer strength 0..1 (0 = off; the owner inks with 0).
    pub stabilizer: f32,
    /// The rest of CSP's Correction group — post correction, its speed/scale
    /// modulation, the sharp-angle exception, the stabilization mode and the
    /// entry/exit shaping. Defaults to all-off, and all-off is a byte-exact
    /// passthrough, so a sub tool from before this existed draws unchanged.
    pub correct: mn_core::stabilize::CorrectCfg,
    /// Brush-size randomization (CSP 乱数): deviation at full pressure.
    /// Unit depends on `random_abs`: log-radius 0..1 (scales with size) or
    /// canvas px (size-independent, the vendored hook).
    pub random: f32,
    /// Floor of the pressure→deviation curve, % of `random`.
    pub random_min: f32,
    /// Deviation is a fixed pixel amount instead of scaling with brush size.
    pub random_abs: bool,
    /// Entry taper: ramp length in px (0 = off) and starting pressure factor.
    /// Seeded from the preset's CSP metadata (Real G-Pen: 217 / 18.3%).
    pub taper_px: f32,
    pub taper_min: f32,
    /// Krita-style hard stamp dabs: exact AA discs instead of the gaussian
    /// hardness falloff (vendor/PATCHES.md). Off = stock behaviour.
    pub hard_dab: bool,
    /// Krita Scatter: each dab's centre jitters within `radius * this`.
    pub scatter: f32,
    /// Krita Wash: the stroke composites once at `opacity` instead of per
    /// dab (flow-vs-opacity semantics).
    pub wash: bool,
    /// Per-dab alpha inside a wash stroke (Krita: Flow). Only meaningful
    /// with `wash`; in build-up the single `opacity` slider is per-dab.
    pub flow: f32,
    /// Compositing mode of the wash commit (Krita: per-brush blending).
    pub brush_blend: Blend,
    /// CSP Ink output (BM-029..035): the brush-only commit behaviours
    /// (black/white burn, compare density, background, replace alpha).
    /// Applies beside `brush_blend` at the wash commit; wash turns itself
    /// on with the choice, exactly as the blend picker does.
    pub brush_draw: mn_brush::BrushDraw,
    /// Texture-tip mask: 0 = none, else 1.. into `App::texture_names`.
    pub texture: u16,
    /// Texture crawl per dab in mask px (0 = static pattern).
    pub texture_scroll: f32,
    /// B-031/032: what a stamped tip's rotation follows.
    pub texture_rotate: mn_brush::TextureRotate,
    /// Krita SKETCH engine: link strokes back to their recent history.
    pub sketch: bool,
    /// Max link distance in canvas px.
    pub sketch_dist: f32,
    /// Link attempts per sample, 0..1.
    pub sketch_density: f32,
    /// TL-013: this sub tool's settings are pinned. CSP's meaning, which is
    /// the useful one — a locked tool still ACCEPTS every change, it just
    /// never writes them down (`App::store_current_props`), so selecting it
    /// again restores the snapshot. Drop the size for one panel and your
    /// calibrated pen comes home by itself; refusing the edit outright
    /// would have made the lock something you have to keep switching off.
    ///
    /// Lives HERE, not on `App`, so the lock follows the sub tool the way
    /// every other Tool Property value does — locking the inking pen leaves
    /// the eraser free. Session-only, like `props` itself.
    pub locked: bool,
    /// CSP Stroke ▸ Interval (S-028): the gap between dabs. `AsPreset` — the
    /// default — leaves the preset's own spacing alone.
    pub interval: Interval,
    /// The gap the Fixed mode remembers while another mode is selected, so
    /// leaving Fixed and coming back does not lose the number.
    pub interval_px: f32,
    /// CSP Adjust brush density by gap (B-029). `None` = untouched, i.e. as
    /// the preset ships it — a tri-state for the same reason `Interval` has
    /// `AsPreset`: a plain `bool` default would have to guess, and guessing
    /// wrong silently re-inks every preset that disagrees with the guess.
    pub density_by_gap: Option<bool>,
    /// CSP Anti-aliasing (A-010): the four-level edge feather.
    pub anti_alias: AntiAlias,
    /// CSP Ink ▸ Density of paint (I-010), 0..1: how much of the DRAWING
    /// colour a dab lays down against how much it picked up off the canvas.
    /// 1.0 = neat paint, and every preset that has never heard of colour
    /// mixing reads 1.0 — so the row is a no-op until it is moved.
    pub paint_density: f32,
    /// CSP Ink ▸ Color stretch (I-011), 0..1: how far the picked-up pigment
    /// is dragged along the stroke.
    pub color_stretch: f32,
    /// CSP Ink ▸ Mixing mode (I-014, triage rows 58 + 167): additive
    /// (Standard) or spectral pigment (Perceptual). Standard is what every
    /// preset that has never heard of the row already does, and the row is
    /// a no-op until it is moved — the `paint_density` rule.
    pub brush_mix: mn_core::BrushMix,
    /// CSP Ink ▸ Intensity of blur (I-013): how wide the running colour is
    /// sampled from. A multiple of the brush radius, unless `blur_abs`.
    pub blur: f32,
    /// The blur width is a canvas-pixel number that does NOT follow the
    /// Size slider (CSP's other mode) — the `random_abs` pattern exactly,
    /// including the unit of `blur` changing with it.
    pub blur_abs: bool,
    /// CSP Color jitter (C-010..012): hue/sat/brightness wander.
    pub jitter: mn_brush::ColorJitter,
    /// CSP 反転 (B-026/027): the brush tip's flip modes, per axis. Only a
    /// TEXTURE tip has an image to mirror; on a plain round dab the rows
    /// sit inert (and say so).
    pub tip_flip_h: mn_brush::TipFlip,
    pub tip_flip_v: mn_brush::TipFlip,
    /// CSP Advanced ▸ Watercolor edge (W-001..005, row 71): the bleed rim
    /// baked outside a finished stroke. Width 0 = off, which is where every
    /// preset that has never asked for it sits.
    pub water_edge: mn_core::edge::WaterEdge,
}

impl Default for ToolProps {
    fn default() -> Self {
        Self {
            // Placeholder only: every real sub tool overwrites this from the
            // preset's own `base_size_px()` before a stroke can happen
            // (`App::new` seeds it, `load_props_for` seeds each later one).
            size_px: DEFAULT_SIZE_PX,
            opacity: 1.0,
            min_size: 0.0,
            stabilizer: 0.0,
            correct: mn_core::stabilize::CorrectCfg::default(),
            random: 0.0,
            random_min: 0.0,
            random_abs: false,
            taper_px: 0.0,
            taper_min: 0.18,
            hard_dab: false,
            scatter: 0.0,
            wash: false,
            flow: 1.0,
            brush_blend: Blend::Normal,
            brush_draw: mn_brush::BrushDraw::Normal,
            texture: 0,
            texture_rotate: mn_brush::TextureRotate::Fixed,
            texture_scroll: 0.0,
            sketch: false,
            sketch_dist: 40.0,
            // M1 re-tune: 0.6 restores the link rate the 0.3 default had
            // while rng01 only covered 0..0.5 (audit 2026-08-17).
            sketch_density: 0.6,
            locked: false,
            interval: Interval::AsPreset,
            interval_px: DEFAULT_INTERVAL_PX,
            density_by_gap: None,
            anti_alias: AntiAlias::AsPreset,
            // The ink group's neutral state: neat paint, so stretch and
            // blur have nothing to act on and the three rows cannot change
            // a pixel until the artist reaches for them.
            paint_density: 1.0,
            color_stretch: 0.5,
            brush_mix: mn_core::BrushMix::Standard,
            blur: 1.0,
            blur_abs: false,
            jitter: mn_brush::ColorJitter {
                hue: 0.0,
                sat: 0.0,
                bri: 0.0,
                per_dab: true,
            },
            tip_flip_h: mn_brush::TipFlip::Off,
            tip_flip_v: mn_brush::TipFlip::Off,
            // Width 0 — off, and byte-exact off (see `apply_stroke_rim`).
            water_edge: mn_core::edge::WaterEdge::default(),
        }
    }
}

/// Gap the Fixed interval mode opens at. Only a starting point for a slider
/// the user then drags — it is not applied until they pick Fixed.
pub const DEFAULT_INTERVAL_PX: f32 = 2.0;

/// The settings the per-sensor curve editor exposes (Krita's dynamics
/// pickers, trimmed to the four a manga inker actually reaches for).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CurveSetting {
    #[default]
    Size,
    Opacity,
    Hardness,
    Smudge,
}

impl CurveSetting {
    pub const ALL: [CurveSetting; 4] = [
        CurveSetting::Size,
        CurveSetting::Opacity,
        CurveSetting::Hardness,
        CurveSetting::Smudge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CurveSetting::Size => "Size",
            CurveSetting::Opacity => "Opacity",
            CurveSetting::Hardness => "Hardness",
            CurveSetting::Smudge => "Smudge",
        }
    }

    pub fn from_index(i: u8) -> CurveSetting {
        Self::ALL[i as usize % Self::ALL.len() as usize]
    }

    /// libmypaint setting id. `None` would mean our vendored libmypaint
    /// lost a setting — not a real state, but the editor degrades calmly.
    pub fn setting_id(self) -> Option<i32> {
        let name = match self {
            CurveSetting::Size => "radius_logarithmic",
            CurveSetting::Opacity => "opaque_multiply",
            CurveSetting::Hardness => "hardness",
            CurveSetting::Smudge => "smudge",
        };
        mn_brush::settings::setting_id(name)
    }

    /// The editor's y axis range in RAW setting units. Size is logarithmic
    /// (ln of the radius factor: the owner's Real G-Pen curve bottoms near
    /// -3.5); the 0..1 settings get drag room past their ends.
    pub fn y_range(self) -> (f32, f32) {
        match self {
            CurveSetting::Size => (-4.0, 1.0),
            _ => (-0.25, 1.25),
        }
    }
}

/// The sensors a curve can respond to (Krita's per-sensor curves; libmypaint
/// already maps by all of these — the gap was the editor).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CurveSensor {
    #[default]
    Pressure,
    Speed,
    Direction,
    TiltX,
    TiltY,
    Random,
}

impl CurveSensor {
    pub const ALL: [CurveSensor; 6] = [
        CurveSensor::Pressure,
        CurveSensor::Speed,
        CurveSensor::Direction,
        CurveSensor::TiltX,
        CurveSensor::TiltY,
        CurveSensor::Random,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CurveSensor::Pressure => "Pressure",
            CurveSensor::Speed => "Speed",
            CurveSensor::Direction => "Direction",
            CurveSensor::TiltX => "Tilt X",
            CurveSensor::TiltY => "Tilt Y",
            CurveSensor::Random => "Random",
        }
    }

    pub fn from_index(i: u8) -> CurveSensor {
        Self::ALL[i as usize % Self::ALL.len() as usize]
    }

    pub fn input_id(self) -> Option<i32> {
        let name = match self {
            CurveSensor::Pressure => "pressure",
            CurveSensor::Speed => "speed1",
            CurveSensor::Direction => "direction",
            CurveSensor::TiltX => "declinationx",
            CurveSensor::TiltY => "declinationy",
            CurveSensor::Random => "random",
        };
        mn_brush::settings::input_id(name)
    }

    /// The editor's x axis range in raw input units (tilt is -1..1 after the
    /// MyBrush normalisation; speed is open-ended, clamped for display).
    pub fn x_range(self) -> (f32, f32) {
        match self {
            CurveSensor::Pressure | CurveSensor::Direction | CurveSensor::Random => (0.0, 1.0),
            CurveSensor::Speed => (0.0, 4.0),
            CurveSensor::TiltX | CurveSensor::TiltY => (-1.0, 1.0),
        }
    }
}
