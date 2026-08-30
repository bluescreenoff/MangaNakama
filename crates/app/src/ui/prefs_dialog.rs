//! The Preferences window (T3 rework, owner order 2026-08-21): tabbed like
//! CSP/Discord/VSCode, searchable, with a UI-size setting, opened from the
//! File menu. One metadata registry (`PREF_INDEX`) feeds the row labels,
//! their hover descriptions, the search, and the Ctrl+K palette — a row
//! cannot exist without its metadata, and every description ends in the
//! catch-terms/synonyms the owner asked search to never miss.

use super::theme;
use crate::app::App;
use mn_core::PageSetup;

/// The tab rail, in draw order. The Ctrl+K palette's "Preferences ▸ …"
/// rows come from this same array (`ui::quick`), so a rename here renames
/// them too instead of orphaning a highlight.
pub(crate) const TABS: [&str; 8] = [
    "Saving",
    "Drawing",
    "Canvas & view",
    "Interface",
    "Text",
    "History",
    "Performance",
    "Shortcuts",
];

/// One setting's metadata: `id` is stable (used by `App::prefs_focus` and
/// the palette), `tab` must be a `TABS` entry, and `desc` is the hover +
/// search haystack — it ENDS with catch terms, per the owner's rule.
pub(crate) struct PrefMeta {
    pub id: &'static str,
    pub tab: &'static str,
    pub label: &'static str,
    pub desc: &'static str,
}

pub(crate) const PREF_INDEX: &[PrefMeta] = &[
    PrefMeta {
        id: "autosave_min",
        tab: "Saving",
        label: "Autosave",
        desc: "How often the recovery copy is written; Off stops the timer \
               entirely. Work folders save in place on this timer. \
               (autosave, backup, recovery, crash, save timer, minutes)",
    },
    PrefMeta {
        id: "autosave_every_op",
        tab: "Saving",
        label: "Also after every operation",
        desc: "Writes the recovery copy as soon as an operation finishes, \
               instead of waiting for the timer. Costs a save per operation \
               on a print-resolution page. (autosave, per-op, every stroke, \
               paranoid, recovery)",
    },
    PrefMeta {
        id: "export_reminder",
        tab: "Saving",
        label: "Remind about unexported pages",
        desc: "Shows a quiet count in the status bar when pages changed \
               since the last export wrote them; click it to open Export \
               All Pages. Silent until a work has been exported once. \
               (export reminder, unexported, stale pages, forgot to export)",
    },
    PrefMeta {
        id: "mouse_smooth_px",
        tab: "Drawing",
        label: "Mouse smoothing floor",
        desc: "Minimum smoothing applied to mouse strokes only — the pen \
               always uses the sub tool's own stabilizer. 0 turns the floor \
               off. (stabilizer, smoothing, jitter, shaky lines, mouse)",
    },
    PrefMeta {
        id: "smart_shape",
        tab: "Drawing",
        label: "Hold to create figures",
        desc: "The Figure ▸ Smart shape sub tool: draw freehand, hold the \
               pen still at the end of the stroke, and the wobble is \
               replaced by the clean line, curve or shape it was \
               approximating. Off leaves that sub tool drawing plain \
               freehand. (smart shape, hold to create figures, recognize, \
               recognise, snap, straighten, clean up, circle, ellipse, \
               rectangle, polygon, FG-020)",
    },
    PrefMeta {
        id: "smart_hold_ms",
        tab: "Drawing",
        label: "Hold time",
        desc: "How long the pen has to stand still before the shape is \
               recognized. Raise it if you pause mid-stroke to think — past \
               the hold the pen adjusts the recognized shape instead of \
               carrying the stroke on. (smart shape, hold duration, delay, \
               long press, wait, too fast, too slow, FG-020)",
    },
    PrefMeta {
        id: "smart_fit_tol",
        tab: "Drawing",
        label: "Recognition tolerance",
        desc: "How close the fit has to be before a stroke is swapped for a \
               figure, as a share of the shape's own size. Raise it if it \
               keeps refusing shapes you meant; the size floor and the \
               scribble refusals do not move, so cross-hatching and \
               scribbled-out mistakes are still never eaten. (smart shape, \
               tolerance, accuracy, confidence, strictness, sensitivity, \
               refuses, never triggers, FG-020)",
    },
    PrefMeta {
        id: "new_canvas",
        tab: "Canvas & view",
        label: "New canvas",
        desc: "Size of the blank canvas the app starts with, in pixels. \
               (new document, default size, width, height, blank page)",
    },
    PrefMeta {
        id: "new_preset",
        tab: "Canvas & view",
        label: "New Manga preset",
        desc: "The page preset the New Manga dialog opens on. Creating a \
               comic also remembers the preset it used. (paper size, B4, \
               B5, doujinshi, default preset, page setup)",
    },
    PrefMeta {
        id: "fit_margin",
        tab: "Canvas & view",
        label: "Fit margin",
        desc: "How much of the window the page fills when fit to view. \
               (zoom to fit, margin, fit page, breathing room)",
    },
    PrefMeta {
        id: "wheel_step",
        tab: "Canvas & view",
        label: "Wheel zoom step",
        desc: "Zoom factor per mouse-wheel notch. (scroll zoom, zoom speed, \
               wheel sensitivity)",
    },
    PrefMeta {
        id: "rotate_step_deg",
        tab: "Canvas & view",
        label: "View rotation step",
        desc: "Degrees per view-rotation keypress. (rotate canvas, rotation \
               angle, step)",
    },
    PrefMeta {
        id: "palette_icon_px",
        tab: "Canvas & view",
        label: "Layers palette icons",
        desc: "Size of the Layers palette's command buttons; the toggle \
               strip scales with it. (icons, scale, big, small, layers \
               buttons)",
    },
    PrefMeta {
        id: "theme",
        tab: "Interface",
        label: "Theme",
        desc: "The chrome's colour scheme. Applies immediately; only dark \
               themes ship for now. (dark, sepia, violet, colours, skin, \
               appearance)",
    },
    PrefMeta {
        id: "icon_colours",
        tab: "Interface",
        label: "Coloured icons",
        desc: "Tints each icon by what it does — a green plus on the \
               new-layer buttons, a red bin, one hue for the selection \
               family — in low-saturation shades taken from the current \
               theme. Off draws every glyph in plain chrome grey. \
               (colour, colors, icons, monochrome, tint, hue, greyscale)",
    },
    PrefMeta {
        id: "show_pose3d_materials",
        tab: "Interface",
        label: "Show 3D pose materials",
        desc: "The materials bank hides its 3D-pose thumbnails until you \
               turn this on — they cannot be placed on the page yet, and \
               a pile of things you cannot use is noise while drawing. \
               (3D, 3d, pose, poses, materials, hidden, hide, bank, \
               mannequin, figure)",
    },
    PrefMeta {
        id: "ui_scale",
        tab: "Interface",
        label: "UI size",
        desc: "Scales the whole interface — text, palettes, icons — without \
               touching the artwork's zoom. 100% is the window's own DPI. \
               (font size, ui scale, text too small, bigger interface, zoom, \
               dpi, readability)",
    },
    PrefMeta {
        id: "text_size_pt",
        tab: "Text",
        label: "New text size",
        desc: "Point size new text items start at. (font size, text tool, \
               default pt, lettering)",
    },
    PrefMeta {
        id: "recent_depth",
        tab: "Text",
        label: "Recent files kept",
        desc: "How many entries File ▸ Open Recent keeps. (recent, MRU, \
               history, file list)",
    },
    PrefMeta {
        id: "new_folder_through",
        tab: "Drawing",
        label: "New folders default to Through",
        desc: "A Through folder does not seal: its children blend against                everything beneath them on the page, exactly as if loose.                Off = the sealed default (CSP's default too). (folder,                through, blend, group, isolate, LF-003)",
    },
    PrefMeta {
        id: "undo_depth",
        tab: "History",
        label: "Undo depth",
        desc: "Undo steps kept per document. Deeper history uses more \
               memory; lowering it drops the oldest steps now. (undo, \
               history, ctrl+z, steps, memory)",
    },
    PrefMeta {
        id: "automation",
        tab: "Performance",
        label: "Automation server",
        desc: "Lets scripts and AI assistants drive the app over a \
               localhost-only socket — batch text edits, page renders — \
               with every change undoable. Off unless you turn it on; the \
               session token lives in automation.txt beside the exe. \
               (automation, MCP, API, socket, remote, scripting, Claude, \
               typesetting, JSON-RPC)",
    },
    PrefMeta {
        id: "gpu_inking",
        tab: "Performance",
        label: "GPU inking",
        desc: "Whether strokes rasterize on the graphics card. Decided by a \
               measurement on this machine; the manual switch lives in the \
               View menu under GPU inking. (gpu, speed, lag, slow brush, \
               hardware acceleration, performance)",
    },
    // Not a prefs.txt row: the tab itself. This entry is what makes the
    // window's search and the Ctrl+K palette reach the Shortcuts tab
    // ("shortcut settings — Setting"), and it satisfies the registry's
    // every-tab-has-rows rule; no tab fn renders it.
    PrefMeta {
        id: "shortcut_settings",
        tab: "Shortcuts",
        label: "Shortcut settings",
        desc: "Edits keys.json — your own chords for commands and tool \
               targets, ahead of the built-in keys. A save rewrites the \
               file and applies immediately; a chord shows what it already \
               does before you take it. (shortcuts, keys, hotkeys, key \
               bindings, keyboard, rebinding, chords, keymap)",
    },
];

fn meta(id: &str) -> Option<&'static PrefMeta> {
    PREF_INDEX.iter().find(|m| m.id == id)
}

/// The tab a focus string means: a tab name selects itself, a row id
/// selects the tab that owns the row.
fn tab_of(focus: &str) -> Option<usize> {
    TABS.iter()
        .position(|t| *t == focus)
        .or_else(|| meta(focus).and_then(|m| TABS.iter().position(|t| *t == m.tab)))
}

/// A row's label cell: registry label, hover description, lit when the
/// search / palette jumped here.
fn row_label(ui: &mut egui::Ui, focus: Option<&str>, id: &'static str) {
    let Some(m) = meta(id) else {
        ui.label(id);
        return;
    };
    let lit = focus == Some(m.id);
    let text = egui::RichText::new(m.label);
    ui.label(if lit {
        text.color(theme::c().accent)
    } else {
        text
    })
    .on_hover_text(m.desc);
}

/// The Autosave dropdown's labels — CSP's own range, plus Off.
fn autosave_label(min: u32) -> String {
    if min == 0 {
        "Off".to_owned()
    } else {
        format!("{min} min")
    }
}

/// Cross-widget effects a frame collects and applies after the window.
#[derive(Default)]
struct Fx {
    changed: bool,
    reset: bool,
    preset_pick: Option<String>,
    /// A theme to switch to before the window closes. `String`, not
    /// `&'static str`: custom themes (T1 step 3) have no static names.
    theme_pick: Option<String>,
}

/// File ▸ Preferences… — tab rail on the left (Discord), search box above
/// it (VSCode), grouped rows inside tabs (CSP). Every default is today's
/// constant, so a user who never opens this window sees no change at all.
pub(super) fn prefs_window(ctx: &egui::Context, app: &mut App) {
    if !app.prefs_open {
        return;
    }
    let mut open = app.prefs_open;
    // A palette/search jump owns the tab while it is lit; clicking the
    // rail hands the tab back to the user (and clears the highlight).
    if let Some(t) = app.prefs_focus.and_then(tab_of) {
        app.prefs_tab = t;
    }
    let mut fx = Fx::default();
    let autosave_before = app.prefs.autosave_min;
    let scale_before = app.prefs.ui_scale;
    let automation_before = app.prefs.automation;
    let preset_now = app.prefs.new_preset_setup().name;
    // Read before the window borrows `app.prefs`. Live rather than cached:
    // the background measurement can land mid-session.
    let gpu_line = crate::bench::state_line_for(app);

    egui::Window::new("Preferences")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            ui.set_width(540.0);

            // --- search ------------------------------------------------
            let search = ui.add(
                egui::TextEdit::singleline(&mut app.prefs_search)
                    .hint_text("Search settings…")
                    .desired_width(f32::INFINITY),
            );
            if search.changed() && !app.prefs_search.is_empty() {
                app.prefs_focus = None;
            }
            let q = app.prefs_search.trim().to_lowercase();
            if !q.is_empty() {
                // Ranked flat list across every tab: label prefix beats
                // label-contains beats description/tab-contains. The
                // description haystack is what makes "lasso"-style catch
                // terms land.
                let mut hits: Vec<(u8, &PrefMeta)> = PREF_INDEX
                    .iter()
                    .filter_map(|m| {
                        let label = m.label.to_lowercase();
                        let rank = if label.starts_with(&q) {
                            0
                        } else if label.contains(&q) {
                            1
                        } else if m.desc.to_lowercase().contains(&q)
                            || m.tab.to_lowercase().contains(&q)
                        {
                            2
                        } else {
                            return None;
                        };
                        Some((rank, m))
                    })
                    .collect();
                hits.sort_by_key(|(r, m)| (*r, m.label));
                ui.add_space(4.0);
                if hits.is_empty() {
                    ui.weak("no matching setting");
                }
                for (_, m) in hits.into_iter().take(12) {
                    let row = ui.selectable_label(false, format!("{}   —   {}", m.label, m.tab));
                    if row.on_hover_text(m.desc).clicked() {
                        app.prefs_focus = Some(m.id);
                        app.prefs_search.clear();
                        if let Some(t) = tab_of(m.id) {
                            app.prefs_tab = t;
                        }
                    }
                }
                footer(ui, app, &mut fx);
                return;
            }

            ui.add_space(4.0);
            // --- rail + body -------------------------------------------
            // The divider between them is painted AFTER the row, over the
            // row's real rect. A vertical `ui.separator()` here is a
            // feedback loop: the widget stretches to available_height —
            // which in this auto-sized window is last frame's height — and
            // the footer below then adds to it, so the window grew ~50pt
            // every frame until it ran off the desktop (owner report
            // 2026-08-29; latent since T3, masked while content was short).
            let mut divider_x = 0.0;
            let row = ui
                .horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(110.0);
                        for (i, t) in TABS.iter().enumerate() {
                            if ui.selectable_label(app.prefs_tab == i, *t).clicked() {
                                app.prefs_tab = i;
                                app.prefs_focus = None;
                            }
                        }
                    });
                    // The lane the Separator widget used to occupy — width
                    // only, so it cannot feed height back into the window.
                    let (lane, _) =
                        ui.allocate_exact_size(egui::vec2(6.0, 0.0), egui::Sense::hover());
                    divider_x = lane.center().x;
                    ui.vertical(|ui| {
                        ui.set_min_width(390.0);
                        ui.set_min_height(230.0);
                        let focus = app.prefs_focus;
                        match TABS[app.prefs_tab.min(TABS.len() - 1)] {
                            "Saving" => tab_saving(ui, app, focus, &mut fx),
                            "Drawing" => tab_drawing(ui, app, focus, &mut fx),
                            "Canvas & view" => {
                                tab_canvas(ui, app, focus, &preset_now, &mut fx)
                            }
                            "Interface" => tab_interface(ui, app, focus, &mut fx),
                            "Text" => tab_text(ui, app, focus, &mut fx),
                            "History" => tab_history(ui, app, focus, &mut fx),
                            "Shortcuts" => super::shortcut_tab::tab(ui, app),
                            _ => tab_performance(ui, app, focus, &mut fx, &gpu_line),
                        }
                    });
                })
                .response
                .rect;
            ui.painter().vline(
                divider_x,
                row.y_range(),
                ui.visuals().widgets.noninteractive.bg_stroke,
            );
            footer(ui, app, &mut fx);
        });

    if fx.reset {
        app.prefs.reset();
        app.new_doc_draft.setup = app.prefs.new_preset_setup();
        fx.theme_pick = Some(theme::resolved_name(&app.prefs.theme).to_owned());
    }
    // Unconditional, and after `reset`: the checkbox writes the preference
    // and this pushes it into the painters' global — Reset cannot leave
    // the switch and the setting disagreeing.
    super::icons::set_accents(app.prefs.icon_colours);
    if let Some(name) = fx.theme_pick {
        app.prefs.theme = name.clone();
        theme::set_by_name(&name);
        theme::apply(ctx);
        fx.changed = true;
    }
    if let Some(name) = fx.preset_pick {
        app.prefs.new_preset = name;
        app.new_doc_draft.setup = app.prefs.new_preset_setup();
        fx.changed = true;
    }
    if fx.changed {
        app.prefs.mark_dirty();
    }
    if app.prefs.autosave_min != autosave_before {
        app.autosave_rearm = Some(app.prefs.autosave_ms());
    }
    // UI size lands through the same door DPI changes use — the pump
    // multiplies the window DPI by the new scale (`main::pump_commands`).
    if (app.prefs.ui_scale - scale_before).abs() > 1e-4 {
        app.ui_scale_apply = Some(app.prefs.ui_scale);
    }
    // The automation socket opens/gates live through the pump, same
    // HWND-free indirection as the autosave timer above.
    if app.prefs.automation != automation_before {
        app.automation_apply = Some(app.prefs.automation);
    }
    app.prefs_open = open;
    if !open {
        app.prefs_focus = None;
        app.prefs_search.clear();
    }
}

fn footer(ui: &mut egui::Ui, _app: &App, fx: &mut Fx) {
    ui.add_space(6.0);
    ui.separator();
    if ui.button("Reset to defaults").clicked() {
        fx.reset = true;
    }
    ui.add_space(2.0);
    ui.weak(
        egui::RichText::new(format!(
            "Settings live in {} — deleting that file resets everything here.",
            crate::app::prefs::path_hint()
        ))
        .size(10.0),
    );
}

fn tab_saving(ui: &mut egui::Ui, app: &mut App, focus: Option<&str>, fx: &mut Fx) {
    let p = &mut app.prefs;
    egui::Grid::new("mn.prefs.saving")
        .num_columns(2)
        .spacing([10.0, 5.0])
        .show(ui, |ui| {
            row_label(ui, focus, "autosave_min");
            egui::ComboBox::from_id_salt("mn.prefs.autosave")
                .width(110.0)
                .selected_text(autosave_label(p.autosave_min))
                .show_ui(ui, |ui| {
                    for m in [0u32, 5, 10, 15, 30, 60] {
                        if ui
                            .selectable_label(p.autosave_min == m, autosave_label(m))
                            .clicked()
                            && p.autosave_min != m
                        {
                            p.autosave_min = m;
                            fx.changed = true;
                        }
                    }
                });
            ui.end_row();

            // PR-041. A second row rather than an "Every operation" entry in
            // the dropdown above: the two are not alternatives — with both
            // on you get whichever comes first — and a dropdown cannot say
            // that. Greys out when Autosave is Off, because Off has to mean
            // off.
            let timer_on = p.autosave_min != 0;
            row_label(ui, focus, "autosave_every_op");
            fx.changed |= ui
                .add_enabled(timer_on, egui::Checkbox::new(&mut p.autosave_every_op, ""))
                .changed();
            ui.end_row();

            row_label(ui, focus, "new_folder_through");
            fx.changed |= ui
                .add(egui::Checkbox::new(&mut p.new_folder_through, ""))
                .changed();
            ui.end_row();

            row_label(ui, focus, "export_reminder");
            fx.changed |= ui
                .add(egui::Checkbox::new(&mut p.export_reminder, ""))
                .changed();
            ui.end_row();
        });
    ui.weak(
        "Work folders save in place on this timer; everything else gets a \
         separate recovery copy. Off stops the timer entirely — including \
         the per-operation save, which is the same write on a different \
         trigger.",
    );
}

fn tab_drawing(ui: &mut egui::Ui, app: &mut App, focus: Option<&str>, fx: &mut Fx) {
    let p = &mut app.prefs;
    egui::Grid::new("mn.prefs.drawing")
        .num_columns(2)
        .spacing([10.0, 5.0])
        .show(ui, |ui| {
            row_label(ui, focus, "mouse_smooth_px");
            fx.changed |= ui
                .add(
                    egui::DragValue::new(&mut p.mouse_smooth_px)
                        .range(0.0..=mn_core::stabilize::MAX_STRING_PX)
                        .speed(0.25)
                        .suffix(" px"),
                )
                .changed();
            ui.end_row();

            // Row 156 / `FG-020`. The two knobs grey out with the switch:
            // a hold time for a hold that never happens is a lie.
            row_label(ui, focus, "smart_shape");
            fx.changed |= ui.checkbox(&mut p.smart_shape, "").changed();
            ui.end_row();

            let on = p.smart_shape;
            row_label(ui, focus, "smart_hold_ms");
            fx.changed |= ui
                .add_enabled(
                    on,
                    egui::DragValue::new(&mut p.smart_hold_ms)
                        .range(
                            crate::app::prefs::SMART_HOLD_MS_MIN
                                ..=crate::app::prefs::SMART_HOLD_MS_MAX,
                        )
                        .speed(10.0)
                        .suffix(" ms"),
                )
                .changed();
            ui.end_row();

            row_label(ui, focus, "smart_fit_tol");
            let mut pct = p.smart_fit_tol * 100.0;
            if ui
                .add_enabled(
                    on,
                    egui::DragValue::new(&mut pct)
                        .range(
                            (crate::app::prefs::SMART_FIT_TOL_MIN * 100.0)
                                ..=(crate::app::prefs::SMART_FIT_TOL_MAX * 100.0),
                        )
                        .speed(0.1)
                        .fixed_decimals(1)
                        .suffix(" %"),
                )
                .changed()
            {
                p.smart_fit_tol = pct / 100.0;
                fx.changed = true;
            }
            ui.end_row();
        });
    ui.weak(
        "Mouse smoothing is mouse strokes only — the pen always uses the sub \
         tool's own stabilizer, and 0 turns the floor off. Hold to create \
         figures is the Figure ▸ Smart shape sub tool: past the hold, keeping \
         the pen down and dragging sizes and turns the recognized shape, and \
         Shift makes it regular. A stroke the recognizer will not explain is \
         always left exactly as drawn.",
    );
}

fn tab_canvas(
    ui: &mut egui::Ui,
    app: &mut App,
    focus: Option<&str>,
    preset_now: &str,
    fx: &mut Fx,
) {
    let p = &mut app.prefs;
    egui::Grid::new("mn.prefs.canvas")
        .num_columns(2)
        .spacing([10.0, 5.0])
        .show(ui, |ui| {
            row_label(ui, focus, "new_canvas");
            ui.horizontal(|ui| {
                fx.changed |= ui
                    .add(
                        egui::DragValue::new(&mut p.new_canvas.0)
                            .range(1..=65535)
                            .suffix(" px"),
                    )
                    .changed();
                fx.changed |= ui
                    .add(
                        egui::DragValue::new(&mut p.new_canvas.1)
                            .range(1..=65535)
                            .suffix(" px"),
                    )
                    .changed();
            });
            ui.end_row();

            row_label(ui, focus, "new_preset");
            egui::ComboBox::from_id_salt("mn.prefs.preset")
                .width(240.0)
                .selected_text(preset_now.to_owned())
                .show_ui(ui, |ui| {
                    for s in PageSetup::presets() {
                        if ui.selectable_label(preset_now == s.name, &s.name).clicked()
                            && preset_now != s.name
                        {
                            fx.preset_pick = Some(s.name.clone());
                        }
                    }
                });
            ui.end_row();

            row_label(ui, focus, "fit_margin");
            let mut pct = p.fit_margin * 100.0;
            if ui
                .add(
                    egui::DragValue::new(&mut pct)
                        .range(80.0..=100.0)
                        .speed(0.25)
                        .fixed_decimals(0)
                        .suffix(" %"),
                )
                .changed()
            {
                p.fit_margin = pct / 100.0;
                fx.changed = true;
            }
            ui.end_row();

            row_label(ui, focus, "wheel_step");
            fx.changed |= ui
                .add(
                    egui::DragValue::new(&mut p.wheel_step)
                        .range(1.02..=1.50)
                        .speed(0.005)
                        .fixed_decimals(2)
                        .prefix("×"),
                )
                .changed();
            ui.end_row();

            row_label(ui, focus, "rotate_step_deg");
            fx.changed |= ui
                .add(
                    egui::DragValue::new(&mut p.rotate_step_deg)
                        .range(1.0..=90.0)
                        .speed(0.5)
                        .fixed_decimals(0)
                        .suffix(" °"),
                )
                .changed();
            ui.end_row();

            row_label(ui, focus, "palette_icon_px");
            fx.changed |= ui
                .add(
                    egui::DragValue::new(&mut p.palette_icon_px)
                        .range(14.0..=32.0)
                        .speed(0.25)
                        .fixed_decimals(0)
                        .suffix(" px"),
                )
                .changed();
            ui.end_row();
        });
    ui.weak("New canvas and preset apply to the next document you create.");
}

fn tab_interface(ui: &mut egui::Ui, app: &mut App, focus: Option<&str>, fx: &mut Fx) {
    let p = &mut app.prefs;
    egui::Grid::new("mn.prefs.interface")
        .num_columns(2)
        .spacing([10.0, 5.0])
        .show(ui, |ui| {
            // Three words in a row rather than a dropdown: with three
            // built-ins, a combo hides two of them behind a click, and the
            // whole point of a theme is that you try them.
            row_label(ui, focus, "theme");
            let now = theme::resolved_name(&p.theme);
            ui.horizontal(|ui| {
                for (name, _) in theme::BUILT_INS {
                    if ui.selectable_label(now == *name, *name).clicked() && now != *name {
                        fx.theme_pick = Some((*name).to_owned());
                    }
                }
                // T1 step 3: custom themes beside the built-ins — a folder
                // of files IS the share mechanism. A file named like a
                // built-in is skipped here: by_name prefers the built-in,
                // and a chip that cannot win is a lie.
                if let Some(dir) = theme::themes_dir() {
                    for name in theme::custom_names_in(&dir)
                        .into_iter()
                        .filter(|n| !theme::BUILT_INS.iter().any(|(b, _)| b == n))
                    {
                        let is_now = p.theme == name;
                        if ui.selectable_label(is_now, &name).clicked() && !is_now {
                            fx.theme_pick = Some(name);
                        }
                    }
                }
            });
            ui.end_row();

            row_label(ui, focus, "icon_colours");
            fx.changed |= ui.checkbox(&mut p.icon_colours, "").changed();
            ui.end_row();

            row_label(ui, focus, "show_pose3d_materials");
            fx.changed |= ui.checkbox(&mut p.show_pose3d_materials, "").changed();
            ui.end_row();

            row_label(ui, focus, "ui_scale");
            let mut pct = p.ui_scale * 100.0;
            if ui
                .add(
                    egui::Slider::new(&mut pct, 75.0..=175.0)
                        .fixed_decimals(0)
                        .suffix(" %"),
                )
                .changed()
            {
                p.ui_scale = pct / 100.0;
                fx.changed = true;
            }
            ui.end_row();
        });
    ui.weak(
        "Theme and icons apply immediately; UI size applies when you release \
         the slider. Only dark themes ship as built-ins — customise below.",
    );
    theme_editor(ui, p, fx);
    // The 3D-poses toggle re-derives the bank's tree live (the branch and
    // counts appear/vanish); items did not change, so no rescan/decode.
    // Detected AFTER the grid: `p`'s borrow ends at its last use above.
    let wants = app.prefs.show_pose3d_materials;
    let has = app.material_tree.iter().any(|n| {
        matches!(
            n.filter,
            crate::app::materials::MaterialFilter::Type(
                crate::app::materials::MaterialType::Pose3d
            )
        )
    });
    if wants != has {
        app.rebuild_material_tree();
    }
}

/// T1 step 3: the theme EDITOR. The pickers bind to the LIVE palette — an
/// edit previews immediately (immediate mode = free); "Save as…" writes
/// `themes/<name>.txt` beside the exe and switches to it; Reset reloads
/// the theme's source of truth (built-in or file). No in-app share
/// ecosystem — a folder of files IS the share mechanism.
fn theme_editor(ui: &mut egui::Ui, p: &mut crate::app::Prefs, fx: &mut Fx) {
    ui.collapsing("Customise theme…", |ui| {
        let mut t = theme::c();
        let mut edited = false;
        egui::Grid::new("mn.theme.editor")
            .num_columns(2)
            .striped(true)
            .min_col_width(96.0)
            .show(ui, |ui| {
                for k in theme::token_names() {
                    ui.label(egui::RichText::new(k).small());
                    let mut rgb = [
                        theme::token_get(&t, k).unwrap().r(),
                        theme::token_get(&t, k).unwrap().g(),
                        theme::token_get(&t, k).unwrap().b(),
                    ];
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        let c = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                        theme::token_set(&mut t, k, c);
                        edited = true;
                    }
                    ui.end_row();
                }
            });
        if edited {
            theme::set(t);
            theme::apply(ui.ctx());
        }
        ui.horizontal(|ui| {
            ui.label("save as");
            ui.add(
                egui::TextEdit::singleline(&mut p.theme_save_name)
                    .hint_text("my theme")
                    .desired_width(110.0),
            );
            let name = p.theme_save_name.trim().to_owned();
            let ok = !name.is_empty()
                && !name.contains(['/', '\\', ':'])
                && !theme::BUILT_INS.iter().any(|(b, _)| *b == name);
            if ui
                .add_enabled(ok, egui::Button::new("Save as…"))
                .on_disabled_hover_text(
                    "a name without path separators; built-in names are reserved",
                )
                .clicked()
                && let Some(dir) = theme::themes_dir()
                && theme::save_custom(&dir, &name, &theme::c())
            {
                p.theme = name.clone();
                theme::set_by_name(&name);
                theme::apply(ui.ctx());
                p.theme_save_name = String::new();
                fx.changed = true;
            }
            if ui.button("Reset").clicked() {
                theme::set_by_name(&p.theme);
                theme::apply(ui.ctx());
            }
        });
        ui.weak(
            "custom themes live in themes\\ beside the app — copy the file \
             to share one; a file named like a built-in is ignored",
        );
    });
}

fn tab_text(ui: &mut egui::Ui, app: &mut App, focus: Option<&str>, fx: &mut Fx) {
    let p = &mut app.prefs;
    egui::Grid::new("mn.prefs.text")
        .num_columns(2)
        .spacing([10.0, 5.0])
        .show(ui, |ui| {
            row_label(ui, focus, "text_size_pt");
            fx.changed |= ui
                .add(
                    egui::DragValue::new(&mut p.text_size_pt)
                        .range(4.0..=72.0)
                        .speed(0.25)
                        .fixed_decimals(1)
                        .suffix(" pt"),
                )
                .changed();
            ui.end_row();

            row_label(ui, focus, "recent_depth");
            fx.changed |= ui
                .add(egui::DragValue::new(&mut p.recent_depth).range(1..=32))
                .changed();
            ui.end_row();
        });
}

fn tab_history(ui: &mut egui::Ui, app: &mut App, focus: Option<&str>, fx: &mut Fx) {
    let p = &mut app.prefs;
    egui::Grid::new("mn.prefs.history")
        .num_columns(2)
        .spacing([10.0, 5.0])
        .show(ui, |ui| {
            row_label(ui, focus, "undo_depth");
            fx.changed |= ui
                .add(
                    egui::DragValue::new(&mut p.undo_depth)
                        .range(50..=5000)
                        .speed(5.0)
                        .suffix(" steps"),
                )
                .changed();
            ui.end_row();
        });
    ui.weak("Deeper history uses more memory. Lowering it drops the oldest steps now.");
}

/// Read-only on purpose — the switch itself is the View menu's, and
/// duplicating a control here would create a second place for it to
/// disagree. What was missing was never a control: it was any way to SEE
/// which authority decided.
fn tab_performance(
    ui: &mut egui::Ui,
    app: &mut App,
    focus: Option<&str>,
    fx: &mut Fx,
    gpu_line: &str,
) {
    row_label(ui, focus, "gpu_inking");
    ui.label(gpu_line);
    ui.weak(
        "Inking moves to the GPU only where a measurement on this machine says \
         it is faster; on many laptops the CPU wins and it stays off. Set it \
         by hand in the View menu, under GPU inking.",
    );
    ui.add_space(8.0);
    ui.separator();
    egui::Grid::new("mn.prefs.automation")
        .num_columns(2)
        .spacing([10.0, 5.0])
        .show(ui, |ui| {
            row_label(ui, focus, "automation");
            fx.changed |= ui
                .add(egui::Checkbox::new(&mut app.prefs.automation, ""))
                .changed();
            ui.end_row();
        });
    ui.weak(
        "A localhost-only command socket for scripts and AI assistants. \
         Clients read the port and session token from automation.txt beside \
         the exe; every remote edit lands in the normal undo history. \
         docs/AUTOMATION.md has the protocol.",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Preferences window must settle, not grow. A vertical
    /// `ui.separator()` between the rail and the body stretched to
    /// available_height — last frame's window height in an auto-sized
    /// window — and the footer added below it, so the window gained ~50pt
    /// every frame until it ran off the desktop (owner report 2026-08-29).
    /// Every tab pumps a few real frames; the window rect must stop moving
    /// and stay inside the screen.
    #[test]
    fn prefs_window_settles_on_screen_every_tab() {
        let Some(renderer) = crate::app::headless_renderer() else {
            return;
        };
        let (w, h) = (1280u32, 860u32);
        let mut app = crate::app::App::new(renderer, (w, h), 1.0);
        app.prefs_open = true;
        let ctx = app.shell.ctx.clone();
        // The window is the only Middle-order area this frame builds.
        let window_rect = |ctx: &egui::Context| -> egui::Rect {
            ctx.memory(|m| {
                m.areas()
                    .visible_layer_ids()
                    .iter()
                    .filter(|l| l.order == egui::Order::Middle)
                    .filter_map(|l| m.area_rect(l.id))
                    .next()
                    .expect("the Preferences window is visible")
            })
        };
        for tab in 0..TABS.len() {
            app.prefs_tab = tab;
            let mut last = egui::Rect::NOTHING;
            for i in 0..6 {
                let raw = app.shell.begin((w, h));
                let mut out = ctx.run_ui(raw, |ui| crate::ui::build(ui, &mut app));
                out.textures_delta.clear();
                let r = window_rect(&ctx);
                // Two frames to settle (anchor + first auto-size), then still.
                if i >= 3 {
                    assert_eq!(
                        r, last,
                        "tab {tab} ({}) frame {i}: the window is still moving",
                        TABS[tab]
                    );
                }
                last = r;
            }
            assert!(
                last.top() >= 0.0 && last.bottom() <= h as f32,
                "tab {tab} ({}) settled off-screen: {last:?}",
                TABS[tab]
            );
        }
    }

    /// The registry is the single spell-out: ids unique, every tab real,
    /// every description carrying catch terms (ends with a parenthetical).
    #[test]
    fn the_registry_is_coherent() {
        let mut seen = std::collections::HashSet::new();
        for m in PREF_INDEX {
            assert!(seen.insert(m.id), "duplicate id {}", m.id);
            assert!(
                TABS.contains(&m.tab),
                "{} names unknown tab {}",
                m.id,
                m.tab
            );
            assert!(
                m.desc.trim_end().ends_with(')'),
                "{}'s description must end in (catch, terms)",
                m.id
            );
        }
        // Every tab has at least one row (Performance's is the info row).
        for t in TABS {
            assert!(PREF_INDEX.iter().any(|m| m.tab == t), "tab {t} has no rows");
        }
    }

    /// The owner's acceptance case for search: a catch term that appears in
    /// no label still finds its row.
    #[test]
    fn search_catch_terms_land() {
        let hit = |q: &str| {
            PREF_INDEX
                .iter()
                .find(|m| m.label.to_lowercase().contains(q) || m.desc.to_lowercase().contains(q))
                .map(|m| m.id)
        };
        assert_eq!(hit("text too small"), Some("ui_scale"));
        assert_eq!(hit("crash"), Some("autosave_min"));
        assert_eq!(hit("ctrl+z"), Some("undo_depth"));
        assert_eq!(hit("lag"), Some("gpu_inking"));
        assert_eq!(hit("through"), Some("new_folder_through"), "row 19's words are findable");
    }

    /// Focus strings resolve: tab names to themselves, row ids to their
    /// owning tab, garbage to nothing.
    #[test]
    fn focus_resolves_to_a_tab() {
        assert_eq!(tab_of("Performance"), Some(6));
        assert_eq!(tab_of("undo_depth"), Some(5));
        assert_eq!(tab_of("ui_scale"), Some(3));
        assert_eq!(tab_of("nonsense"), None);
    }
}
