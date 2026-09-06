//! Tool Property rows for the SELECTED effect-line set — the 流線 / 集中線
//! run already on the page, under the Operation tool.
//!
//! Split out of `frames_balloons.rs` in lane B2
//! (`docs/plans/2026-09-06-effect-lines-parity.md`): that file had grown to
//! 1 540 lines carrying two unrelated subjects. Pure move — the fns are the
//! ones that were there, cut into one `Row` per control. The Figure TOOL's
//! own knobs (what the next drag places) are in `figure_lines.rs`.
//!
//! Two rules survive the split and matter more than the layout:
//! - a placed set REGENERATES on the release edge of a drag, never per
//!   frame. A regen re-rasterizes the whole layer and a page-sized burst at
//!   600 dpi is not a per-mouse-move operation. Every row here reads the
//!   draft with [`gen_draft`] and hands it to [`gen_commit`], which is the
//!   one place that decision lives.
//! - a knob that means nothing for the armed sub tool (Sweep on a stream,
//!   Start on a burst) carries it as `applies`, so the settings window greys
//!   it and says why instead of the row silently not existing.

use super::*;

// --- the SELECTED effect-line run (owner report 2026-08-23) ----------------
//
// Tool Property edits the item you picked, and a generated set was the one
// item it refused to. These rows edit the LAYER'S OWN `GenLinesSpec` — not
// the Figure tool's defaults, which is what the sub tool rows do — and commit
// through `GenLinesApplyTo`.

/// The draft spec the widgets mutate, and the layer it belongs to.
fn gen_draft(app: &App) -> Option<(usize, mn_core::genlines::GenLinesSpec)> {
    let li = app.gen_sel?;
    let stored = app.doc.layers.get(li)?.genlines?;
    Some((li, app.gen_edit.unwrap_or(stored)))
}

/// Push the draft (drag release) or keep it (mid-drag). One helper so no row
/// can forget which of the two it is doing.
///
/// A frame in which NOTHING changed must not re-arm the draft: the canvas
/// handles write the same spec, and a stale draft left sitting here would
/// shadow their result the next time the palette drew.
fn gen_commit(
    app: &mut App,
    li: usize,
    spec: mn_core::genlines::GenLinesSpec,
    changed: bool,
    done: bool,
) {
    if done {
        app.gen_edit = None;
        if Some(spec) != app.doc.layers.get(li).and_then(|l| l.genlines) {
            app.push_cmd(AppCmd::GenLinesApplyTo { layer: li, spec });
        }
    } else if changed {
        app.gen_edit = Some(spec);
    }
}

/// What a row needs to know about the document while it edits the draft.
struct GenCtx {
    px_per_mm: f32,
    doc: (u32, u32),
}

/// One row over the selected set: read the draft, let the row edit it, commit
/// on the release edge. The shared state (the draft, the page's px-per-mm) is
/// recomputed per row — a handful of field reads.
fn gen_row(
    ui: &mut egui::Ui,
    app: &mut App,
    f: impl FnOnce(&mut egui::Ui, &GenCtx, &mut mn_core::genlines::GenLinesSpec) -> (bool, bool),
) {
    let Some((li, mut s)) = gen_draft(app) else {
        return;
    };
    let ctx = GenCtx {
        px_per_mm: app.mm_to_px(1.0).max(0.001),
        doc: app.doc.size,
    };
    let (changed, done) = f(ui, &ctx, &mut s);
    gen_commit(app, li, s, changed, done);
}

/// The density rows' draft, with one extra step: a set written before the
/// wobbles split carries ONE `jitter` and three zeros, and the renderer falls
/// back to it for each of them (`GenLinesSpec::jit`). STATE that on the draft
/// the moment the palette draws — otherwise moving the Position bar alone
/// would also move the length and the width wobble, because those two were
/// reading the same fallback number Position just overwrote. Writing them
/// changes no pixel (they are the values the renderer was already using) and
/// it only reaches the layer if the owner commits some edit anyway.
fn gen_density_row(
    ui: &mut egui::Ui,
    app: &mut App,
    f: impl FnOnce(&mut egui::Ui, &GenCtx, &mut mn_core::genlines::GenLinesSpec) -> (bool, bool),
) {
    gen_row(ui, app, |ui, ctx, s| {
        let single = s.jitter;
        if single > 0.0 {
            for v in [&mut s.jit_gap, &mut s.jit_len, &mut s.jit_width] {
                if *v <= 0.0 {
                    *v = single;
                }
            }
        }
        f(ui, ctx, s)
    });
}

/// A ValueBar row over one `f32` of the draft: returns (changed, done).
fn gen_bar(
    ui: &mut egui::Ui,
    label: &str,
    range: (f32, f32),
    decimals: usize,
    suffix: &str,
    v: &mut f32,
) -> (bool, bool) {
    let resp = ValueBar::new(label, range.0, range.1)
        .decimals(decimals)
        .suffix(suffix)
        .show(ui, v);
    (
        resp.changed(),
        resp.drag_stopped() || (resp.changed() && !resp.dragged()),
    )
}

// --- which knobs a selected set has ---------------------------------------

fn gen_spec(app: &App) -> Option<mn_core::genlines::GenLinesSpec> {
    gen_draft(app).map(|(_, s)| s)
}

/// A flash (ウニフラッシュ / ベタフラッシュ) has no stroke profile: its teeth
/// are filled wedges, counted and spread over the whole circle.
fn gen_is_flash(s: &mn_core::genlines::GenLinesSpec) -> bool {
    s.kind == 1 || s.kind == 2
}

fn gen_radial(app: &App) -> bool {
    gen_spec(app).is_some_and(|s| s.radial())
}

fn gen_stream(app: &App) -> bool {
    gen_spec(app).is_some_and(|s| !s.radial())
}

fn gen_line(app: &App) -> bool {
    gen_spec(app).is_some_and(|s| !gen_is_flash(&s))
}

fn gen_radial_line(app: &App) -> bool {
    gen_spec(app).is_some_and(|s| s.radial() && !gen_is_flash(&s))
}

fn gen_accented(app: &App) -> bool {
    gen_spec(app).is_some_and(|s| !gen_is_flash(&s) && s.accent_frac > 0.0)
}

fn gen_anchored(app: &App) -> bool {
    gen_spec(app).is_some_and(|s| !s.radial() && s.start_mode == 1)
}

fn gen_fanned(app: &App) -> bool {
    gen_spec(app).is_some_and(|s| !s.radial() && s.converge.is_some())
}

fn gen_by_gap(app: &App) -> bool {
    gen_spec(app).is_some_and(|s| if s.radial() { s.gap_deg > 0.0 } else { s.gap_px > 0.0 })
}

fn gen_by_count(app: &App) -> bool {
    gen_spec(app).is_some_and(|s| !(if s.radial() { s.gap_deg > 0.0 } else { s.gap_px > 0.0 }))
}

/// まとまり needs a GAP to count holes in, and a flash has none.
fn gen_gapped(app: &App) -> bool {
    gen_spec(app)
        .is_some_and(|s| !gen_is_flash(&s) && if s.radial() { s.gap_deg > 0.0 } else { s.gap_px > 0.0 })
}

fn gen_bundled(app: &App) -> bool {
    gen_gapped(app) && gen_spec(app).is_some_and(|s| s.group > 1)
}

// --- shape rows ------------------------------------------------------------

/// Kind, but only inside the RADIAL family: 集中線, ウニフラッシュ and
/// ベタフラッシュ read a, b, c, d identically (centre, r_in, r_out), so
/// switching between them is a re-render. A 流線 spec means something else by
/// the same four numbers, so it is not offered as a swap — that would
/// silently reinterpret the geometry.
pub(crate) fn row_obj_gen_kind(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        let mut hit = false;
        ui.horizontal(|ui| {
            for (k, label) in [(0u8, "Saturated"), (1, "Urchin"), (2, "Solid")] {
                if ui.selectable_label(s.kind == k, label).clicked() && s.kind != k {
                    s.kind = k;
                    s.focus = true;
                    hit = true;
                }
            }
        });
        (hit, hit)
    });
}

pub(crate) fn row_obj_gen_color(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        let mut hit = false;
        ui.horizontal(|ui| {
            ui.weak("colour");
            // White knockout lines over a black panel — the generator inked
            // black only until this existed.
            if ui.color_edit_button_srgb(&mut s.color).changed() {
                hit = true;
            }
            if s.color != [0, 0, 0] && ui.small_button("black").clicked() {
                s.color = [0, 0, 0];
                hit = true;
            }
        });
        (hit, hit)
    });
}

pub(crate) fn row_obj_gen_width(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, c, s| {
        let flash = gen_is_flash(s);
        let mut w_mm = s.width / c.px_per_mm;
        let (changed, done) = gen_bar(
            ui,
            if flash { "Spike width" } else { "Width" },
            (0.02, if flash { 4.0 } else { 1.5 }),
            2,
            " mm",
            &mut w_mm,
        );
        if changed {
            s.width = (w_mm * c.px_per_mm).max(0.5);
        }
        (changed, done)
    });
}

/// The weight MIX (parity round): a few heavy strokes among the hairlines.
/// One weight everywhere is the flat noise field.
pub(crate) fn row_obj_gen_accents(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        gen_bar(ui, "Accents", (0.0, 1.0), 2, "", &mut s.accent_frac)
    });
}

pub(crate) fn row_obj_gen_accent_width(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        // 0 reads as 1 in the renderer, so a set placed before accents
        // existed must not jump to 10× the moment the bar is touched.
        let mut mul = s.accent_mul.max(1.0);
        let (changed, done) = gen_bar(ui, "Accent width", (1.0, 10.0), 1, " \u{d7}", &mut mul);
        if changed {
            s.accent_mul = mul;
        }
        (changed, done)
    });
}

pub(crate) fn row_obj_gen_taper(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        gen_bar(ui, "Taper", (0.0, 1.0), 2, "", &mut s.taper)
    });
}

pub(crate) fn row_obj_gen_entry(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        gen_bar(ui, "Entry", (0.0, 1.0), 2, "", &mut s.entry)
    });
}

pub(crate) fn row_obj_gen_needle(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        gen_bar(ui, "Needle", (0.0, 3.0), 2, "", &mut s.needle)
    });
}

/// The hole, as a fraction of the reach — the same knob the sub tool row
/// arms, so the two mean one thing.
pub(crate) fn row_obj_gen_hollow(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        let mut frac = if s.d > 0.0 { s.c / s.d } else { 0.0 };
        let (changed, done) = gen_bar(ui, "Hollow centre", (0.0, 0.95), 2, "", &mut frac);
        if changed {
            s.c = s.d * frac.clamp(0.0, 0.95);
        }
        (changed, done)
    });
}

pub(crate) fn row_obj_gen_reach(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        let (changed, done) = gen_bar(ui, "Reach", (8.0, 6000.0), 0, " px", &mut s.d);
        if changed {
            s.c = s.c.min((s.d - 4.0).max(0.0));
        }
        (changed, done)
    });
}

/// 0 = the full circle; anything else is a fan that wide, centred on the
/// direction the placing drag went.
pub(crate) fn row_obj_gen_sweep(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        gen_bar(ui, "Sweep", (0.0, 360.0), 0, "\u{b0}", &mut s.sweep_deg)
    });
}

pub(crate) fn row_obj_gen_angle(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        gen_bar(ui, "Angle", (-180.0, 180.0), 1, "\u{b0}", &mut s.a)
    });
}

pub(crate) fn row_obj_gen_shortest(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        let r = gen_bar(ui, "Shortest", (8.0, 6000.0), 0, " px", &mut s.b);
        if s.b > s.c {
            s.b = s.c;
        }
        r
    });
}

pub(crate) fn row_obj_gen_longest(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        let r = gen_bar(ui, "Longest", (8.0, 6000.0), 0, " px", &mut s.c);
        if s.b > s.c {
            s.b = s.c;
        }
        r
    });
}

/// ref-09's drips: every run begins on ONE line instead of scattering along
/// the direction.
pub(crate) fn row_obj_gen_start(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        let mut anchored = s.start_mode == 1;
        if ui
            .checkbox(&mut anchored, "Start from the line")
            .on_hover_text(
                "every run begins on the reference line and hangs off it — drips from a panel's \
                 top edge, rain, a curtain. Off, they start anywhere along the direction",
            )
            .changed()
        {
            s.start_mode = u8::from(anchored);
            return (true, true);
        }
        (false, false)
    });
}

pub(crate) fn row_obj_gen_start_wobble(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        gen_bar(ui, "Start wobble", (0.0, 1.0), 2, "", &mut s.jit_start)
    });
}

/// 流線 with a vanishing point: the subtle fan a perspective panel wants.
/// Off = pure parallel.
pub(crate) fn row_obj_gen_fan(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, c, s| {
        let mut fan = s.converge.is_some();
        if ui
            .checkbox(&mut fan, "Fan toward a point")
            .on_hover_text("aims every run at one canvas point instead of running parallel")
            .changed()
        {
            s.converge = fan.then(|| {
                let a = crate::app::canvas_input::gen_anchor(s, c.doc);
                [a[0], a[1] - c.doc.1 as f32 * 4.0]
            });
            return (true, true);
        }
        (false, false)
    });
}

pub(crate) fn row_obj_gen_fan_point(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        let (mut changed, mut done) = (false, false);
        if let Some(v) = &mut s.converge {
            ui.horizontal(|ui| {
                ui.label("Point");
                let a = ui.add(egui::DragValue::new(&mut v[0]).speed(4.0));
                let b = ui.add(egui::DragValue::new(&mut v[1]).speed(4.0));
                changed |= a.changed() || b.changed();
                done |= a.drag_stopped() || b.drag_stopped();
            });
        }
        (changed, done)
    });
}

pub(crate) fn row_obj_gen_reroll(ui: &mut egui::Ui, app: &mut App) {
    gen_row(ui, app, |ui, _c, s| {
        if ui
            .button("Reroll")
            .on_hover_text("the same parameters, a different draw of the random wobble")
            .clicked()
        {
            s.seed = s.seed.wrapping_add(1);
            return (true, true);
        }
        (false, false)
    });
}

/// Shape: what kind of set it is, how far it reaches, how a line is shaped.
///
/// Row order mirrors the Figure tool's own panel — Width, Accents, Taper,
/// Entry, Needle, then the geometry — so tuning a placed set and arming the
/// next drag are the same motion in the same order. The gap, the bundling and
/// the wobbles are the Density section.
pub(crate) const ROWS_OBJ_GEN: &[Row] = &[
    row_when("obj.gen.kind", "Kind", row_obj_gen_kind, gen_radial),
    row("obj.gen.color", "Colour", row_obj_gen_color),
    row("obj.gen.width", "Width", row_obj_gen_width),
    row_when("obj.gen.accents", "Accents", row_obj_gen_accents, gen_line),
    row_when(
        "obj.gen.accent_width",
        "Accent width",
        row_obj_gen_accent_width,
        gen_accented,
    ),
    row_when("obj.gen.taper", "Taper", row_obj_gen_taper, gen_line),
    row_when("obj.gen.entry", "Entry", row_obj_gen_entry, gen_line),
    row_when("obj.gen.needle", "Needle", row_obj_gen_needle, gen_line),
    row_when(
        "obj.gen.hollow",
        "Hollow centre",
        row_obj_gen_hollow,
        gen_radial,
    ),
    row_when("obj.gen.reach", "Reach", row_obj_gen_reach, gen_radial),
    row_when("obj.gen.sweep", "Sweep", row_obj_gen_sweep, gen_radial_line),
    row_when("obj.gen.angle", "Angle", row_obj_gen_angle, gen_stream),
    row_when(
        "obj.gen.shortest",
        "Shortest",
        row_obj_gen_shortest,
        gen_stream,
    ),
    row_when(
        "obj.gen.longest",
        "Longest",
        row_obj_gen_longest,
        gen_stream,
    ),
    row_when("obj.gen.start", "Start", row_obj_gen_start, gen_stream),
    row_when(
        "obj.gen.start_wobble",
        "Start wobble",
        row_obj_gen_start_wobble,
        gen_anchored,
    ),
    row_when(
        "obj.gen.fan",
        "Fan toward a point",
        row_obj_gen_fan,
        gen_stream,
    ),
    row_when(
        "obj.gen.fan_point",
        "Fan point",
        row_obj_gen_fan_point,
        gen_fanned,
    ),
    row("obj.gen.reroll", "Reroll", row_obj_gen_reroll),
];

// --- density rows ----------------------------------------------------------

/// GAP, not count: a manga tutorial sizes a 集中線 in degrees (≈3° dense,
/// ≈10° sparse) and that number means the same thing whatever the page is.
/// A stream walks its block at a fixed pitch instead of scattering, because
/// the random scatter clumps — which is what makes a generated set read as
/// noise. The count is still offered, because every set placed before this
/// was made of one.
pub(crate) fn row_obj_gen_space_mode(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, c, s| {
        let radial = s.radial();
        let mut by_gap = if radial { s.gap_deg > 0.0 } else { s.gap_px > 0.0 };
        let hit = ui
            .checkbox(
                &mut by_gap,
                if radial {
                    "Space by angle"
                } else {
                    "Space evenly"
                },
            )
            .on_hover_text(if radial {
                "a gap in degrees instead of a total count — 3° dense, 10° sparse"
            } else {
                "walk the block at a fixed gap instead of scattering runs at random — \
                 the random scatter clumps, which is what makes a generated set read as noise"
            })
            .changed();
        if hit {
            if radial {
                s.gap_deg = if by_gap {
                    360.0 / s.count.max(1) as f32
                } else {
                    0.0
                };
                if !by_gap {
                    s.count = s.ray_count();
                }
            } else {
                s.gap_px = if by_gap { c.px_per_mm } else { 0.0 };
            }
        }
        (hit, hit)
    });
}

pub(crate) fn row_obj_gen_gap(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, c, s| {
        if s.radial() {
            let r = gen_bar(ui, "Gap", (0.5, 30.0), 2, "\u{b0}", &mut s.gap_deg);
            ui.weak(format!("{} lines", s.ray_count()));
            r
        } else {
            let mut gap_mm = s.gap_px / c.px_per_mm;
            let (changed, done) = gen_bar(ui, "Gap", (0.1, 10.0), 2, " mm", &mut gap_mm);
            if changed {
                s.gap_px = (gap_mm * c.px_per_mm).max(0.25);
            }
            (changed, done)
        }
    });
}

pub(crate) fn row_obj_gen_count(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, _c, s| {
        let mut n = s.count as f32;
        let (changed, done) = gen_bar(ui, "Lines", (1.0, 512.0), 0, "", &mut n);
        if changed {
            s.count = (n.round() as u32).max(1);
        }
        (changed, done)
    });
}

/// まとまり — bundles with a hole between them.
pub(crate) fn row_obj_gen_bundle(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, _c, s| {
        let mut n = s.group as f32;
        let (changed, done) = gen_bar(ui, "Bundle", (0.0, 16.0), 0, "", &mut n);
        if changed {
            s.group = n.round() as u32;
        }
        if s.group <= 1 {
            ui.weak("0 or 1 = one even block, no bundles");
        }
        (changed, done)
    });
}

pub(crate) fn row_obj_gen_bundle_gap(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, _c, s| {
        gen_bar(ui, "Bundle gap", (1.0, 8.0), 1, " \u{d7}", &mut s.group_gap)
    });
}

pub(crate) fn row_obj_gen_bundle_variety(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, _c, s| {
        gen_bar(ui, "Bundle variety", (0.0, 1.0), 2, "", &mut s.group_jit)
    });
}

/// One 0..1 wobble bar with its hover text.
fn gen_wobble(ui: &mut egui::Ui, label: &str, hint: &str, v: &mut f32) -> (bool, bool) {
    let resp = ValueBar::new(label, 0.0, 1.0).decimals(2).show(ui, v);
    let changed = resp.changed();
    let done = resp.drag_stopped() || (changed && !resp.dragged());
    resp.on_hover_text(hint);
    (changed, done)
}

pub(crate) fn row_obj_gen_pos_wobble(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, _c, s| {
        gen_wobble(
            ui,
            "Position",
            "how far each line strays from the even spacing",
            &mut s.jit_gap,
        )
    });
}

pub(crate) fn row_obj_gen_len_wobble(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, _c, s| {
        gen_wobble(ui, "Length", "how much the lengths differ", &mut s.jit_len)
    });
}

pub(crate) fn row_obj_gen_long_bias(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, _c, s| {
        gen_wobble(
            ui,
            "Long bias",
            "which way the length wobble leans — 1 makes most lines long with a few short ones",
            &mut s.len_skew,
        )
    });
}

pub(crate) fn row_obj_gen_outer_len(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, _c, s| {
        gen_wobble(
            ui,
            "Outer length",
            "the same wobble on the FAR end of a ray — keep it small or the ends stop inside the panel",
            &mut s.jit_len_out,
        )
    });
}

pub(crate) fn row_obj_gen_width_wobble(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, _c, s| {
        gen_wobble(
            ui,
            "Width",
            "how much the weights differ from line to line",
            &mut s.jit_width,
        )
    });
}

pub(crate) fn row_obj_gen_core_jit(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, _c, s| {
        gen_wobble(
            ui,
            "Core stagger",
            "how ragged the edge of the hollow centre is — 0 is a compass circle",
            &mut s.core_jit,
        )
    });
}

/// A stream's own hand wobble: degrees off the drag's direction, so it does
/// not belong in the 0..1 column above.
pub(crate) fn row_obj_gen_angle_wobble(ui: &mut egui::Ui, app: &mut App) {
    gen_density_row(ui, app, |ui, _c, s| {
        gen_bar(ui, "Angle wobble", (0.0, 5.0), 2, "\u{b0}", &mut s.jit_angle)
    });
}

/// Density: the gap between lines, the bundling, and the wobbles.
///
/// The bundling is offered for the RADIAL kinds too since the parity round —
/// the owner's "effect lines doesn't seem to have a grouping setting", which
/// was true: まとまり only ever existed in the speed walk, so a 集中線 came
/// out at one even pitch whatever you did to it.
pub(crate) const ROWS_OBJ_GEN_DENSITY: &[Row] = &[
    row(
        "obj.gen.density.space_mode",
        "Spacing mode",
        row_obj_gen_space_mode,
    ),
    row_when("obj.gen.density.gap", "Gap", row_obj_gen_gap, gen_by_gap),
    row_when(
        "obj.gen.density.count",
        "Lines",
        row_obj_gen_count,
        gen_by_count,
    ),
    row_when(
        "obj.gen.density.bundle",
        "Bundle",
        row_obj_gen_bundle,
        gen_gapped,
    ),
    row_when(
        "obj.gen.density.bundle_gap",
        "Bundle gap",
        row_obj_gen_bundle_gap,
        gen_bundled,
    ),
    row_when(
        "obj.gen.density.bundle_variety",
        "Bundle variety",
        row_obj_gen_bundle_variety,
        gen_bundled,
    ),
    row(
        "obj.gen.density.position",
        "Position wobble",
        row_obj_gen_pos_wobble,
    ),
    row(
        "obj.gen.density.length",
        "Length wobble",
        row_obj_gen_len_wobble,
    ),
    row_when(
        "obj.gen.density.long_bias",
        "Long bias",
        row_obj_gen_long_bias,
        gen_line,
    ),
    row_when(
        "obj.gen.density.outer_length",
        "Outer length",
        row_obj_gen_outer_len,
        gen_radial_line,
    ),
    row_when(
        "obj.gen.density.width",
        "Width wobble",
        row_obj_gen_width_wobble,
        gen_line,
    ),
    row_when(
        "obj.gen.density.core",
        "Core stagger",
        row_obj_gen_core_jit,
        gen_radial,
    ),
    row_when(
        "obj.gen.density.angle",
        "Angle wobble",
        row_obj_gen_angle_wobble,
        gen_stream,
    ),
];

