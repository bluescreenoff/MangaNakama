//! The Figure tool's effect-line knobs — what the NEXT drag places.
//!
//! Split out of `effect_lines.rs` in lane B2 when the row conversion took
//! that file past 1 400 lines: the SELECTED set (there) and the tool's own
//! defaults (here) are edited in different panels, by different people, at
//! different times. Pure move — the fns are the ones that were there.
//!
//! Ranges mirror the generator dialog's (its clamp rationale — giant
//! counts/widths were a real UI hang — applies here too), and every width is
//! stated in MILLIMETRES like the object panel: a millimetre means the same
//! thing on a 600 dpi B4 and a 72 dpi draft, and a pixel does not.

use super::*;

/// One `label  value` row of the effect-line knobs.
///
/// A helper rather than a dozen copies of the same four lines: the Figure
/// panel is a column of near-identical numbers, and the only thing that makes
/// such a column readable is that EVERY value says what it does on the page
/// when you hover it. Hand-written rows are rows where that gets left out.
fn line_knob(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    speed: f64,
    suffix: &str,
    hint: &str,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::DragValue::new(v)
                .range(range)
                .speed(speed)
                .suffix(suffix),
        )
        .on_hover_text(hint);
    });
}

/// How many rays the CURRENT radial knobs would draw — the bundle walk's own
/// answer, not `360 / gap`.
///
/// Once bundling exists the two are different numbers: a bundle of 4 with a
/// 3x hole spends 7 gaps on 4 rays, so the old hint over-counted by nearly
/// half and the owner would have sized the burst against a lie. The walk
/// lives on [`mn_core::genlines::GenLinesSpec`], so ask a throwaway spec
/// carrying just the fields it reads.
fn tool_ray_count(o: &crate::cmd::FigureLineOpts) -> u32 {
    mn_core::genlines::GenLinesSpec {
        focus: true,
        count: o.count,
        gap_deg: o.gap_deg,
        sweep_deg: o.sweep_deg,
        group: o.group,
        group_gap: o.group_gap,
        group_jit: o.group_jit,
        seed: o.seed,
        ..Default::default()
    }
    .ray_count()
}

/// What the armed Figure sub tool is, for the `applies` predicates and the
/// row bodies alike.
struct FigCtx {
    radial: bool,
    flash: bool,
    px_per_mm: f32,
}

/// One row over the Figure tool's own knobs. These are plain app state (the
/// settings the NEXT drag is placed with), so there is no draft and no commit
/// edge — a stroke of the bar IS the new default.
fn fig_row(
    ui: &mut egui::Ui,
    app: &mut App,
    f: impl FnOnce(&mut egui::Ui, &FigCtx, &mut crate::cmd::FigureLineOpts),
) {
    let m = app.figure_mode;
    let ctx = FigCtx {
        radial: m.radial(),
        flash: m.gen_kind() != 0,
        px_per_mm: app.mm_to_px(1.0).max(0.001),
    };
    let o = if ctx.radial {
        &mut app.figure_focus
    } else {
        &mut app.figure_stream
    };
    f(ui, &ctx, o);
}

/// The knobs the armed sub tool would place with.
fn fig_opts(app: &App) -> &crate::cmd::FigureLineOpts {
    if app.figure_mode.radial() {
        &app.figure_focus
    } else {
        &app.figure_stream
    }
}

fn fig_generates(app: &App) -> bool {
    app.figure_mode.generates()
}

fn fig_draws(app: &App) -> bool {
    !app.figure_mode.generates()
}

/// The two DRAGGED closed shapes (row 157 / FG-011): on a straight line the
/// drag already IS the angle, and the click-list gestures have no "size is
/// fixed now" moment to hang it on.
fn fig_adjust_angle(app: &App) -> bool {
    !app.figure_mode.generates() && app.figure_mode.can_adjust_angle()
}

fn fig_line(app: &App) -> bool {
    app.figure_mode.generates() && app.figure_mode.gen_kind() == 0
}

fn fig_radial(app: &App) -> bool {
    app.figure_mode.generates() && app.figure_mode.radial()
}

fn fig_stream(app: &App) -> bool {
    app.figure_mode.generates() && !app.figure_mode.radial()
}

fn fig_radial_line(app: &App) -> bool {
    fig_radial(app) && app.figure_mode.gen_kind() == 0
}

/// Density is stated as a GAP where the preset says so — a count field the
/// generator ignores is worse than no field, and the gap is the unit a
/// tutorial uses anyway. The flashes COUNT their teeth (their `width` is a
/// spike base and the renderer clamps neighbours), so they keep the count.
fn fig_gapped(app: &App) -> bool {
    if !fig_line(app) {
        return false;
    }
    let o = fig_opts(app);
    if app.figure_mode.radial() {
        o.gap_deg > 0.0
    } else {
        o.gap_px > 0.0
    }
}

fn fig_counted(app: &App) -> bool {
    fig_generates(app) && !fig_gapped(app)
}

fn fig_bundled(app: &App) -> bool {
    fig_gapped(app) && fig_opts(app).group > 1
}

pub(crate) fn row_figure_gap(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, c, o| {
        if c.radial {
            line_knob(
                ui,
                "Gap",
                &mut o.gap_deg,
                0.5..=30.0,
                0.05,
                "\u{b0}",
                "degrees between neighbouring rays — about 2° for a dense burst, 10° for a sparse one",
            );
            ui.weak(format!("{} lines", tool_ray_count(o)));
        } else {
            let mut gap_mm = o.gap_px / c.px_per_mm;
            ui.horizontal(|ui| {
                ui.label("Gap");
                if ui
                    .add(
                        egui::DragValue::new(&mut gap_mm)
                            .range(0.1..=10.0)
                            .speed(0.02)
                            .suffix(" mm"),
                    )
                    .on_hover_text(
                        "how far apart the runs sit — they walk the block at this pitch \
                         instead of scattering",
                    )
                    .changed()
                {
                    o.gap_px = (gap_mm * c.px_per_mm).max(0.25);
                }
            });
        }
    });
}

pub(crate) fn row_figure_count(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, c, o| {
        ui.horizontal(|ui| {
            ui.label(if c.flash { "Spikes" } else { "Lines" });
            ui.add(egui::DragValue::new(&mut o.count).range(1..=512))
                .on_hover_text(if c.flash {
                    "how many teeth the flash has, all the way round"
                } else {
                    "how many lines in total"
                });
        });
    });
}

/// まとまり — bundles with a hole between them, in px for a stream and in
/// DEGREES for a burst (the owner's missing grouping setting, 2026-09-06).
pub(crate) fn row_figure_bundle(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        ui.horizontal(|ui| {
            ui.label("Bundle");
            ui.add(egui::DragValue::new(&mut o.group).range(0..=16))
                .on_hover_text(
                    "まとまり — how many lines sit together before a hole. 0 or 1 is one \
                     even comb, which is the thing that reads as machine-made",
                );
            if o.group > 1 {
                ui.add(
                    egui::DragValue::new(&mut o.group_gap)
                        .range(1.0..=8.0)
                        .speed(0.1)
                        .suffix(" \u{d7}"),
                )
                .on_hover_text("how wide the hole after a bundle is, counted in gaps");
            }
        });
    });
}

pub(crate) fn row_figure_width(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, c, o| {
        let mut w_mm = o.width / c.px_per_mm;
        ui.horizontal(|ui| {
            ui.label(if c.flash { "Spike width" } else { "Width" });
            if ui
                .add(
                    egui::DragValue::new(&mut w_mm)
                        // A flash's width is the spike BASE, so it needs a
                        // range a hairline never does.
                        .range(0.02..=if c.flash { 4.0 } else { 1.5 })
                        .speed(0.01)
                        .suffix(" mm"),
                )
                .on_hover_text(if c.flash {
                    "how wide each spike is where it meets the rim"
                } else {
                    "how thick a line is at its heaviest point"
                })
                .changed()
            {
                o.width = (w_mm * c.px_per_mm).max(0.5);
            }
        });
    });
}

/// The weight MIX — the single biggest thing between a generated set and a
/// printed one. A hand puts a few heavy strokes among the hairlines; one
/// weight everywhere is the flat noise field the pro-page audit flagged.
pub(crate) fn row_figure_accents(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        ui.horizontal(|ui| {
            ui.label("Accents");
            ui.add(
                egui::DragValue::new(&mut o.accent_frac)
                    .range(0.0..=1.0)
                    .speed(0.01),
            )
            .on_hover_text("what share of the lines are drawn heavy — 0.15 is about one in seven");
            ui.add(
                egui::DragValue::new(&mut o.accent_mul)
                    .range(1.0..=10.0)
                    .speed(0.05)
                    .suffix(" \u{d7}"),
            )
            .on_hover_text(
                "the HEAVIEST an accent gets, as a multiple of the width. Each accent \
                 draws its own between 1.5× and this, so the set is a continuum rather \
                 than two weights",
            );
        });
    });
}

pub(crate) fn row_figure_taper(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, c, o| {
        line_knob(
            ui,
            "Taper",
            &mut o.taper,
            0.0..=1.0,
            0.01,
            "",
            if c.radial {
                "how far a ray thins toward the convergence — 0 is a flat bar, 1 runs out to nothing"
            } else {
                "how far a line thins toward its tail — 0 is a flat bar, 1 runs out to nothing"
            },
        );
    });
}

pub(crate) fn row_figure_entry(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        line_knob(
            ui,
            "Entry",
            &mut o.entry,
            0.0..=1.0,
            0.01,
            "",
            "入り — how much of the START also ramps up from a point. With Taper that \
             makes a spindle (thin, thick, thin); 0 leaves a blunt cap",
        );
    });
}

pub(crate) fn row_figure_needle(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        line_knob(
            ui,
            "Needle",
            &mut o.needle,
            0.0..=3.0,
            0.05,
            "",
            "the SHAPE of the thinning. 1 is a straight wedge; above 1 it thins fast and \
             then runs a long needle; below 1 it keeps a belly and ends abruptly",
        );
    });
}

pub(crate) fn row_figure_hollow(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        line_knob(
            ui,
            "Hollow centre",
            &mut o.r_in_frac,
            0.0..=0.95,
            0.01,
            "",
            "the empty middle the art sits in, as a fraction of how far you drag",
        );
    });
}

pub(crate) fn row_figure_sweep(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        line_knob(
            ui,
            "Sweep",
            &mut o.sweep_deg,
            0.0..=360.0,
            1.0,
            "\u{b0}",
            "0 = a full circle. Anything else is a FAN that wide, aimed the way you \
             drag — what you want when the burst's centre is off the panel",
        );
    });
}

/// ref-09's ゴ… drips: every run hangs off ONE line instead of scattering
/// along the direction. Scattered, half of them would float in mid-air.
pub(crate) fn row_figure_start(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        ui.horizontal(|ui| {
            ui.label("Start");
            let mut anchored = o.start_mode == 1;
            if ui
                .selectable_label(!anchored, "scatter")
                .on_hover_text("runs begin anywhere along the drag's direction")
                .clicked()
            {
                anchored = false;
            }
            if ui
                .selectable_label(anchored, "from the line")
                .on_hover_text(
                    "every run begins on the line you drag from and hangs off it — drips \
                     from a panel's top edge, rain, a curtain",
                )
                .clicked()
            {
                anchored = true;
            }
            o.start_mode = u8::from(anchored);
            if anchored {
                ui.add(
                    egui::DragValue::new(&mut o.jit_start)
                        .range(0.0..=1.0)
                        .speed(0.01),
                )
                .on_hover_text(
                    "how far the starts stagger off that line, as a share of the run's \
                     own length",
                );
            }
        });
    });
}

/// 流線 with a vanishing point: the perspective streaks of a block converging
/// on an impact instead of sliding past it.
pub(crate) fn row_figure_fan(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        ui.horizontal(|ui| {
            ui.label("Fan toward a point");
            let mut fan = o.converge_far > 0.0;
            if ui
                .checkbox(&mut fan, "")
                .on_hover_text(
                    "aims every run at one point beyond the drag instead of running parallel",
                )
                .changed()
            {
                o.converge_far = if fan { 2.5 } else { 0.0 };
            }
            if fan {
                ui.add(
                    egui::DragValue::new(&mut o.converge_far)
                        .range(0.2..=10.0)
                        .speed(0.05)
                        .suffix(" \u{d7}"),
                )
                .on_hover_text(
                    "how far past the drag's end that point sits, in drag lengths. \
                     Small = a hard perspective, large = a gentle fan",
                );
            }
        });
    });
}

pub(crate) fn row_figure_hint(ui: &mut egui::Ui, _app: &mut App) {
    ui.weak("each drag places its own layer — one undo press removes it");
}

pub(crate) fn row_figure_fill(ui: &mut egui::Ui, app: &mut App) {
    ui.checkbox(&mut app.figure_fill, "Fill with drawing colour")
        .on_hover_text("closed shapes fill before the outline inks");
}

pub(crate) fn row_figure_adjust_angle(ui: &mut egui::Ui, app: &mut App) {
    ui.checkbox(&mut app.figure_adjust_angle, "Adjust angle after fixed")
        .on_hover_text("release sets the size, then the pointer spins it — click inks it");
}

/// The effect-line knobs for the NEXT drag: SHAPE and density.
///
/// The wobbles are their own section — eleven shape knobs and eight wobbles
/// in one column is a wall of numbers nobody reads, and they answer two
/// different questions ("what is a line?" and "how much does the hand
/// vary?"). Ranges mirror the generator dialog's (its clamp rationale —
/// giant counts/widths were a real UI hang — applies here too), and every
/// width is stated in MILLIMETRES like the object panel: a millimetre means
/// the same thing on a 600 dpi B4 and a 72 dpi draft, and a pixel does not.
pub(crate) const ROWS_FIGURE: &[Row] = &[
    row_when("figure.opts.gap", "Gap", row_figure_gap, fig_gapped),
    row_when(
        "figure.opts.count",
        "Lines / spikes",
        row_figure_count,
        fig_counted,
    ),
    row_when("figure.opts.bundle", "Bundle", row_figure_bundle, fig_gapped),
    row_when("figure.opts.width", "Width", row_figure_width, fig_generates),
    row_when("figure.opts.accents", "Accents", row_figure_accents, fig_line),
    row_when("figure.opts.taper", "Taper", row_figure_taper, fig_line),
    row_when("figure.opts.entry", "Entry", row_figure_entry, fig_line),
    row_when("figure.opts.needle", "Needle", row_figure_needle, fig_line),
    row_when(
        "figure.opts.hollow",
        "Hollow centre",
        row_figure_hollow,
        fig_radial,
    ),
    row_when(
        "figure.opts.sweep",
        "Sweep",
        row_figure_sweep,
        fig_radial_line,
    ),
    row_when("figure.opts.start", "Start", row_figure_start, fig_stream),
    row_when(
        "figure.opts.fan",
        "Fan toward a point",
        row_figure_fan,
        fig_stream,
    ),
    row_when("figure.opts.hint", "Hint", row_figure_hint, fig_generates),
    row_when(
        "figure.opts.fill",
        "Fill with drawing colour",
        row_figure_fill,
        fig_draws,
    ),
    row_when(
        "figure.opts.adjust_angle",
        "Adjust angle after fixed",
        row_figure_adjust_angle,
        fig_adjust_angle,
    ),
];

// --- how much the HAND varies: the wobbles, for the next drag --------------
//
// Their own section since the parity round. Before it there was ONE "Jitter"
// row that wrote all four wobble fields to the same number, which was worse
// than nothing: the shipped presets deliberately set them apart (a printed
// set wants a lot of length wobble and almost no angular wobble), so touching
// that row flattened the preset the owner had just picked and the panel gave
// no way back.
//
// The legacy single `jitter` is written from the POSITION wobble. It is not a
// knob any more — it is the fallback a file saved before the split reads
// (`GenLinesSpec::jit`), and keeping it equal to the position wobble is the
// only value that cannot surprise an older build.

pub(crate) fn row_wobble_position(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        ui.horizontal(|ui| {
            ui.label("Position");
            if ui
                .add(
                    egui::DragValue::new(&mut o.jit_gap)
                        .range(0.0..=1.0)
                        .speed(0.01),
                )
                .on_hover_text(
                    "how far each line strays from where the even spacing would put it, as a \
                     share of the gap. 0 is a drafting tool",
                )
                .changed()
            {
                // The fallback for a build that predates the split wobbles.
                o.jitter = o.jit_gap;
            }
        });
    });
}

pub(crate) fn row_wobble_length(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        line_knob(
            ui,
            "Length",
            &mut o.jit_len,
            0.0..=1.0,
            0.01,
            "",
            "how much the lengths differ. 0 makes every line reach exactly as far as its \
             neighbour, which is the clean ring or the wall of equal streaks that gives a set away",
        );
    });
}

pub(crate) fn row_wobble_long_bias(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        line_knob(
            ui,
            "Long bias",
            &mut o.len_skew,
            0.0..=1.0,
            0.01,
            "",
            "which way the length wobble leans. 0 draws short and long equally often; 1 makes \
             most lines long with a few short ones, the way a reference page looks",
        );
    });
}

pub(crate) fn row_wobble_outer_length(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        line_knob(
            ui,
            "Outer length",
            &mut o.jit_len_out,
            0.0..=1.0,
            0.01,
            "",
            "the same wobble on the FAR end of a ray. Keep it small — a ray that stops \
             inside the panel shows its own blunt end instead of being cut by the border",
        );
    });
}

pub(crate) fn row_wobble_width(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        line_knob(
            ui,
            "Width",
            &mut o.jit_width,
            0.0..=1.0,
            0.01,
            "",
            "how much the weights differ from line to line, on top of the accents",
        );
    });
}

pub(crate) fn row_wobble_angle(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        line_knob(
            ui,
            "Angle",
            &mut o.jit_angle,
            0.0..=5.0,
            0.05,
            "\u{b0}",
            "how far each run leans off the drag's direction. A degree or two reads as \
             hand-ruled; 0 is dead parallel and reads as a filter",
        );
    });
}

pub(crate) fn row_wobble_core(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        line_knob(
            ui,
            "Core stagger",
            &mut o.core_jit,
            0.0..=1.0,
            0.01,
            "",
            "how ragged the edge of the hollow centre is. 0 puts every inner end on one perfect \
             circle, which is the compass look",
        );
    });
}

pub(crate) fn row_wobble_bundle_variety(ui: &mut egui::Ui, app: &mut App) {
    fig_row(ui, app, |ui, _c, o| {
        line_knob(
            ui,
            "Bundle variety",
            &mut o.group_jit,
            0.0..=1.0,
            0.01,
            "",
            "how much the bundle sizes and the holes between them vary. 0 repeats one unit at \
             one pitch, and the eye finds the period — a picket fence",
        );
    });
}

/// A stream's angular wobble is the one wobble a burst does not have: a ray's
/// direction IS its place in the fan, so leaning it off is the same knob as
/// Position.
fn fig_stream_line(app: &App) -> bool {
    fig_stream(app) && app.figure_mode.gen_kind() == 0
}

pub(crate) const ROWS_FIGURE_WOBBLE: &[Row] = &[
    row_when(
        "figure.wobble.position",
        "Position",
        row_wobble_position,
        fig_generates,
    ),
    row_when(
        "figure.wobble.length",
        "Length",
        row_wobble_length,
        fig_generates,
    ),
    row_when(
        "figure.wobble.long_bias",
        "Long bias",
        row_wobble_long_bias,
        fig_line,
    ),
    row_when(
        "figure.wobble.outer_length",
        "Outer length",
        row_wobble_outer_length,
        fig_radial_line,
    ),
    row_when("figure.wobble.width", "Width", row_wobble_width, fig_line),
    row_when(
        "figure.wobble.angle",
        "Angle",
        row_wobble_angle,
        fig_stream_line,
    ),
    row_when(
        "figure.wobble.core",
        "Core stagger",
        row_wobble_core,
        fig_radial,
    ),
    row_when(
        "figure.wobble.bundle_variety",
        "Bundle variety",
        row_wobble_bundle_variety,
        fig_bundled,
    ),
];

pub(crate) fn sec_figure_guide(ui: &mut egui::Ui, app: &mut App) {
    use crate::cmd::FigureMode;
    ui.weak(match app.figure_mode {
        FigureMode::Line => "drag start to end; Shift snaps to 45° steps",
        FigureMode::Rect if app.figure_adjust_angle => {
            "drag corner to corner, release, then spin it — click inks it"
        }
        FigureMode::Ellipse if app.figure_adjust_angle => {
            "drag the bounding box, release, then spin it — click inks it"
        }
        FigureMode::Rect => "drag corner to corner; Shift keeps it square",
        FigureMode::Ellipse => "drag the bounding box; Shift keeps it round",
        FigureMode::Polygon => {
            "click vertices; the first one / Enter closes, Backspace takes one back, Esc cancels"
        }
        FigureMode::Arc => "drag the straight line, release, then bend it — click inks it",
        FigureMode::Curve => {
            "click along the curve; Enter (or the last point twice) inks it, Backspace takes one back"
        }
        FigureMode::Smart => {
            "draw freehand, then HOLD still at the end — the stroke becomes the shape it \
             was approximating; release without holding and it stays as drawn"
        }
        FigureMode::Stream => "drag along the motion — angle and length come from the drag",
        FigureMode::Focus => "drag from the convergence point out to the lines' reach",
        FigureMode::Urchin => "drag from the flash's centre out to the spikes' reach",
        FigureMode::SolidFlash => "drag from the hole's centre out to the ring's rim",
    });
    if app.figure_mode.generates() {
        ui.weak("adjust later: Object tool handles, or Layer ▸ effect lines");
    } else {
        ui.weak("inked with the active brush — Size/Opacity above apply");
    }
}
