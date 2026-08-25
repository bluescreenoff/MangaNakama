//! The Align/Distribute window (TR-040..046): chrome over
//! `mn_core::align` — CSP's three rows of buttons and the five-way
//! alignment base (Guide deferred with the row).

use crate::app::App;
use crate::cmd::AppCmd;
use mn_core::align::{AlignBase, AlignMode, DistributeMode, SpacingMode};

/// The layers this palette can act on — the engine's own target rule
/// (multi-selection + active, kinds it moves, visible, unlocked),
/// mirrored so the buttons never promise an op that would refuse.
fn layer_targets(app: &App) -> usize {
    app.doc
        .multi_targets()
        .into_iter()
        .filter(|&i| {
            app.doc.layers.get(i).is_some_and(|l| {
                !l.folder && l.visible && !l.lock && l.strokes.is_none()
                    && !matches!(
                        l.kind,
                        mn_core::doc::LayerKind::Frame(_) | mn_core::doc::LayerKind::Fill(_)
                    )
            })
        })
        .count()
}

/// TR-052: the single selected layer is a text layer with 2+ items —
/// the buttons then align the ITEMS against each other, not layers.
fn item_mode(app: &App) -> Option<usize> {
    let t = app.doc.multi_targets();
    if t.len() != 1 {
        return None;
    }
    app.doc
        .layers
        .get(t[0])
        .filter(|l| l.texts().is_some_and(|ts| ts.texts.len() >= 2))
        .map(|_| t[0])
}

fn item_count(app: &App) -> usize {
    item_mode(app)
        .and_then(|li| app.doc.layers.get(li))
        .and_then(|l| l.texts())
        .map(|ts| ts.texts.len())
        .unwrap_or(0)
}

pub(super) fn align_window(ctx: &egui::Context, app: &mut App) {
    if !app.align_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Align/Distribute")
        .open(&mut open)
        .resizable(false)
        .default_pos(egui::pos2(320.0, 120.0))
        .show(ctx, |ui| {
            let items = item_mode(app).is_some();
            let (n, what) = if let Some(li) = item_mode(app) {
                let _ = li;
                (item_count(app), "text items on the selected layer")
            } else {
                (layer_targets(app), "selected layers")
            };
            ui.label(format!("Targets: {n} {what}"));
            ui.separator();

            ui.label("Alignment base");
            ui.horizontal(|ui| {
                for b in AlignBase::ALL {
                    if ui.selectable_label(app.align_base == b, b.label()).clicked() {
                        app.align_base = b;
                    }
                }
            });
            ui.separator();

            // Align: 2+ targets against the Object base, or any count
            // against the page/selection (TR-044: "centre this on the
            // page" works on a single selection).
            let can_align = n >= 2 || (n >= 1 && app.align_base != AlignBase::Object);
            ui.label("Align");
            ui.horizontal_wrapped(|ui| {
                for m in AlignMode::ALL {
                    let btn = egui::Button::new(short_align(m));
                    let resp = ui.add_enabled(can_align, btn);
                    if resp.clicked() {
                        app.push_cmd(AppCmd::AlignLayers {
                            mode: m,
                            base: app.align_base,
                        });
                    }
                }
            });
            ui.separator();

            let can_distribute = n >= 3;
            ui.label("Distribute (equal edge spacing)");
            ui.horizontal_wrapped(|ui| {
                for m in DistributeMode::ALL {
                    let resp = ui.add_enabled(can_distribute, egui::Button::new(short_dist(m)));
                    if resp.clicked() {
                        app.push_cmd(AppCmd::DistributeLayers { mode: m });
                    }
                }
            });
            ui.separator();

            ui.label("Distribute evenly (equal gaps)");
            ui.horizontal(|ui| {
                let h = ui.add_enabled(
                    can_distribute,
                    egui::Button::new("↔ horizontal gaps"),
                );
                if h.clicked() {
                    app.push_cmd(AppCmd::SpaceLayers {
                        mode: SpacingMode::Horizontal,
                    });
                }
                let v = ui.add_enabled(can_distribute, egui::Button::new("↕ vertical gaps"));
                if v.clicked() {
                    app.push_cmd(AppCmd::SpaceLayers {
                        mode: SpacingMode::Vertical,
                    });
                }
            });
            if !can_distribute {
                ui.small("distribute needs 3+ targets — the outer two stay put");
            }
            if items {
                ui.small(
                    "one text layer selected: the buttons align its ITEMS against each other",
                );
            }
        });
    if !open {
        app.align_open = false;
    }
}

fn short_align(m: AlignMode) -> &'static str {
    match m {
        AlignMode::Left => "⇤ left",
        AlignMode::HCenter => "↔ center",
        AlignMode::Right => "right ⇥",
        AlignMode::Top => "⇧ top",
        AlignMode::VCenter => "↕ center",
        AlignMode::Bottom => "bottom ⇩",
    }
}

fn short_dist(m: DistributeMode) -> &'static str {
    match m {
        DistributeMode::Left => "⇤ left",
        DistributeMode::HCenter => "↔ center",
        DistributeMode::Right => "right ⇥",
        DistributeMode::Top => "⇧ top",
        DistributeMode::VCenter => "↕ center",
        DistributeMode::Bottom => "bottom ⇩",
    }
}
