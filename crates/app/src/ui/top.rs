//! Top chrome: the menu bar + command row (top_bar), the document tab strip
//! and the segmented status bar. Everything here goes through
//! `app.push_cmd` — no direct state mutation except the doc-tab close flag.

use super::icons::Icon;
use super::theme;
use super::widgets::{icon_btn, item, paint_icon};
use crate::app::{App, CaptionCmd};
use crate::cmd::AppCmd;

// --- top bar ------------------------------------------------------------

/// A top-level menu-bar button with the platform-standard hover behaviour:
/// once any menu in the bar is open, sliding onto a sibling opens it without
/// another click. egui 0.36 only does this for submenus — top-level
/// `MenuButton`s toggle purely on click, so the bar remembers its members'
/// popup ids (one frame of lag, invisible to a hover check) and switches.
fn bar_menu(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    let resp = ui.menu_button(title, add).response;
    let id = egui::Popup::default_response_id(&resp);
    let ctx = resp.ctx.clone();
    let sibs_id = egui::Id::new("mn.menubar.popups");
    let mut sibs: Vec<egui::Id> = ctx.data(|d| d.get_temp(sibs_id).unwrap_or_default());
    if resp.hovered()
        && !egui::Popup::is_id_open(&ctx, id)
        && sibs
            .iter()
            .any(|&s| s != id && egui::Popup::is_id_open(&ctx, s))
    {
        // open_popup is exclusive: opening this one closes the sibling.
        egui::Popup::open_id(&ctx, id);
    }
    if !sibs.contains(&id) {
        sibs.push(id);
        ctx.data_mut(|d| d.insert_temp(sibs_id, sibs));
    }
}

pub(super) fn top_bar(ui: &mut egui::Ui, app: &mut App) {
    egui::MenuBar::new().ui(ui, |ui| {
        // The custom title bar (the native caption is removed via
        // WM_NCCALCSIZE in main.rs): the empty menu-bar space DRAGS the
        // window, double-click maximizes. Registered FIRST so every menu and
        // button added after it sits above it in the hit test — the strip only
        // ever sees the leftover space between them.
        let bar = ui.available_rect_before_wrap();
        let drag = ui.interact(
            bar,
            egui::Id::new("mn.caption.drag"),
            egui::Sense::click_and_drag(),
        );
        if drag.drag_started() {
            app.drag_window = true;
        }
        if drag.double_clicked() {
            app.caption_cmd = Some(CaptionCmd::ToggleMax);
        }

        bar_menu(ui, "File", |ui| {
            if item(ui, "New…", "Ctrl+N") {
                app.push_cmd(AppCmd::NewDoc);
                ui.close();
            }
            if item(ui, "New Tiling Pattern…", "") {
                app.push_cmd(AppCmd::NewPattern);
                ui.close();
            }
            if item(ui, "Open…", "Ctrl+O") {
                app.push_cmd(AppCmd::OpenOra);
                ui.close();
            }
            if item(ui, "Revert", "") {
                app.push_cmd(AppCmd::RevertFile);
                ui.close();
            }
            ui.menu_button("Recent", |ui| {
                if app.recent.is_empty() {
                    ui.weak("(empty)");
                }
                let recents = app.recent.clone();
                for p in recents {
                    let label = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.display().to_string());
                    if ui
                        .button(label)
                        .on_hover_text(p.display().to_string())
                        .clicked()
                    {
                        app.push_cmd(AppCmd::OpenOraPath(p.clone()));
                        ui.close();
                    }
                }
            });
            ui.separator();
            if item(ui, "Save", "Ctrl+S") {
                app.push_cmd(AppCmd::SaveOra);
                ui.close();
            }
            if item(ui, "Save As…", "Ctrl+Shift+S") {
                app.push_cmd(AppCmd::SaveOraAs);
                ui.close();
            }
            if item(ui, "Export Single File (.mnc)…", "") {
                app.push_cmd(AppCmd::ExportMnc);
                ui.close();
            }
            ui.separator();
            if item(ui, "Import Image as Layer…", "") {
                app.push_cmd(AppCmd::ImportImage);
                ui.close();
            }
            if item(ui, "Import Brushes (.abr, .gbr, .gih, .kpp, .sut)…", "") {
                app.push_cmd(AppCmd::ImportAbr);
                ui.close();
            }
            if item(ui, "Export PNG (this page)…", "") {
                app.push_cmd(AppCmd::ExportPng);
                ui.close();
            }
            if item(ui, "Export layered PSD (this page)…", "") {
                app.push_cmd(AppCmd::ExportPsd);
                ui.close();
            }
            if item(ui, "Export All Pages…", "") {
                app.push_cmd(AppCmd::ExportAllPages);
                ui.close();
            }
            if item(ui, "Export Text (script)…", "") {
                app.push_cmd(AppCmd::ExportText);
                ui.close();
            }
            if item(ui, "Export one image set per comp…", "") {
                app.push_cmd(AppCmd::CompExportAll);
                ui.close();
            }
            ui.separator();
            // CSP keeps Preferences under File; ours moved here from Edit
            // with the T3 rework (owner order 2026-08-21).
            if item(ui, "Preferences…", "") {
                app.push_cmd(AppCmd::OpenPrefs(None));
                ui.close();
            }
            if item(ui, "Pen pressure…", "") {
                app.push_cmd(AppCmd::PenPressureWizardOpen);
                ui.close();
            }
        });
        bar_menu(ui, "Edit", |ui| {
            if item(ui, "Undo", "Ctrl+Z") {
                app.push_cmd(AppCmd::Undo);
                ui.close();
            }
            if item(ui, "Redo", "Ctrl+Y") {
                app.push_cmd(AppCmd::Redo);
                ui.close();
            }
            if item(ui, "Clear undo history", "") {
                app.push_cmd(AppCmd::ClearHistory);
                ui.close();
            }
            ui.separator();
            if item(ui, "Cut", "Ctrl+X") {
                app.push_cmd(AppCmd::Cut);
                ui.close();
            }
            if item(ui, "Copy", "Ctrl+C") {
                app.push_cmd(AppCmd::Copy);
                ui.close();
            }
            if item(ui, "Paste into panel", "Ctrl+V") {
                app.push_cmd(AppCmd::Paste);
                ui.close();
            }
            if item(ui, "Paste in place", "Ctrl+Shift+V") {
                app.push_cmd(AppCmd::PasteInPlace);
                ui.close();
            }
            if item(ui, "Paste to shown position", "") {
                app.push_cmd(AppCmd::PasteShown);
                ui.close();
            }
            ui.separator();
            if item(ui, "Fill with drawing color", "Alt+Del") {
                app.push_cmd(AppCmd::FillSelection);
                ui.close();
            }
            if item(ui, "Clear", "Del") {
                app.push_cmd(AppCmd::ClearLayer);
                ui.close();
            }
            if item(ui, "Clear outside selection", "Shift+Del") {
                app.push_cmd(AppCmd::ClearOutside);
                ui.close();
            }
            ui.separator();
            if item(ui, "Transform", "Ctrl+T") {
                app.push_cmd(AppCmd::TransformStart);
                ui.close();
            }
            if item(ui, "Flip Horizontal", "") {
                app.push_cmd(AppCmd::TransformFlip { horizontal: true });
                ui.close();
            }
            if item(ui, "Flip Vertical", "") {
                app.push_cmd(AppCmd::TransformFlip { horizontal: false });
                ui.close();
            }
            ui.separator();
            if item(ui, "Change canvas size…", "") {
                app.push_cmd(AppCmd::OpenCanvasSize);
                ui.close();
            }
            let has_sel = app.doc.selection.as_ref().is_some_and(|s| !s.is_empty());
            if ui
                .add_enabled(
                    has_sel,
                    egui::Button::new("Crop to selection").shortcut_text(""),
                )
                .clicked()
            {
                app.push_cmd(AppCmd::CropSelection);
                ui.close();
            }
            if ui
                .add_enabled(
                    has_sel,
                    egui::Button::new("Register selection as brush tip").shortcut_text(""),
                )
                .clicked()
            {
                app.push_cmd(AppCmd::RegisterBrushFromSelection);
                ui.close();
            }
        });
        bar_menu(ui, "Selection", |ui| {
            if item(ui, "Select all", "Ctrl+A") {
                app.push_cmd(AppCmd::SelectAll);
                ui.close();
            }
            if item(ui, "Deselect", "Ctrl+D") {
                app.push_cmd(AppCmd::Deselect);
                ui.close();
            }
            if item(ui, "Reselect", "Ctrl+Shift+D") {
                app.push_cmd(AppCmd::Reselect);
                ui.close();
            }
            if item(ui, "Invert selected area", "Ctrl+Shift+I") {
                app.push_cmd(AppCmd::SelectInvert);
                ui.close();
            }
        });
        bar_menu(ui, "Layer", |ui| {
            if item(ui, "New layer", "") {
                app.push_cmd(AppCmd::AddLayer);
                ui.close();
            }
            if item(ui, "New vector layer", "") {
                app.push_cmd(AppCmd::AddVectorLayer);
                ui.close();
            }
            if item(ui, "Batch operations…", "") {
                app.push_cmd(AppCmd::BatchOpsOpen);
                ui.close();
            }
            if item(ui, "Align/Distribute…", "") {
                app.push_cmd(AppCmd::AlignOpen);
                ui.close();
            }
            if item(ui, "Generate effect lines…", "") {
                app.push_cmd(AppCmd::GenLines);
                ui.close();
            }
            if item(ui, "Edit effect lines…", "") {
                app.push_cmd(AppCmd::GenLinesEdit);
                ui.close();
            }
            if item(ui, "Convert brightness to opacity", "") {
                app.push_cmd(AppCmd::BrightnessToOpacity);
                ui.close();
            }
            ui.menu_button("Layer Mask", |ui| {
                if item(ui, "Mask selection (blank)", "") {
                    app.push_cmd(AppCmd::MaskSelection);
                    ui.close();
                }
                if item(ui, "Mask outside selection", "") {
                    app.push_cmd(AppCmd::MaskOutsideSelection);
                    ui.close();
                }
                ui.separator();
                if item(ui, "Apply mask to layer", "") {
                    app.push_cmd(AppCmd::MaskApply);
                    ui.close();
                }
                if item(ui, "Show mask area", "") {
                    app.push_cmd(AppCmd::MaskShowArea);
                    ui.close();
                }
                ui.separator();
                if item(ui, "Edit mask", "") {
                    app.push_cmd(AppCmd::MaskEdit);
                    ui.close();
                }
                {
                    // LM-009: linked (default) moves the mask with the layer;
                    // unlinked slides the art under a fixed window.
                    let linked =
                        app.doc.active_layer().mask.is_some() && app.doc.active_layer().mask_linked;
                    let has = app.doc.active_layer().mask.is_some();
                    let label = if !has {
                        "Link mask to layer (no mask)"
                    } else if linked {
                        "✓ Link mask to layer"
                    } else {
                        "Link mask to layer"
                    };
                    if item(ui, label, "") {
                        app.push_cmd(AppCmd::MaskLinkToggle);
                        ui.close();
                    }
                }
                ui.separator();
                if item(ui, "Toggle mask", "") {
                    app.push_cmd(AppCmd::MaskToggle);
                    ui.close();
                }
                if item(ui, "Clear mask (hide all)", "") {
                    app.push_cmd(AppCmd::MaskClear);
                    ui.close();
                }
                if item(ui, "Delete mask", "") {
                    app.push_cmd(AppCmd::MaskDelete);
                    ui.close();
                }
            });
            ui.menu_button("Ruler", |ui| {
                if item(ui, "Straight line…", "") {
                    app.push_cmd(AppCmd::RulerArm(crate::cmd::RulerKind::Line));
                    ui.close();
                }
                if item(ui, "Vanishing point…", "") {
                    app.push_cmd(AppCmd::RulerArm(crate::cmd::RulerKind::VanishingPoint));
                    ui.close();
                }
                if item(ui, "Perspective (1-point)…", "") {
                    app.push_cmd(AppCmd::RulerArm(crate::cmd::RulerKind::Perspective1));
                    ui.close();
                }
                if item(ui, "Perspective (2-point)…", "") {
                    app.push_cmd(AppCmd::RulerArm(crate::cmd::RulerKind::Perspective));
                    ui.close();
                }
                if item(ui, "Perspective (3-point)…", "") {
                    app.push_cmd(AppCmd::RulerArm(crate::cmd::RulerKind::Perspective3));
                    ui.close();
                }
                if item(ui, "Curve…", "") {
                    app.push_cmd(AppCmd::RulerArm(crate::cmd::RulerKind::Curve));
                    ui.close();
                }
                ui.separator();
                if item(ui, "Parallel line…", "") {
                    app.push_cmd(AppCmd::RulerArm(crate::cmd::RulerKind::Parallel));
                    ui.close();
                }
                if item(ui, "Concentric circles…", "") {
                    app.push_cmd(AppCmd::RulerArm(crate::cmd::RulerKind::Concentric));
                    ui.close();
                }
                if item(ui, "Symmetrical…", "") {
                    app.push_cmd(AppCmd::RulerArm(crate::cmd::RulerKind::Symmetric));
                    ui.close();
                }
                let sym = app
                    .doc
                    .rulers
                    .items
                    .iter()
                    .rev()
                    .find_map(|r| match r {
                        mn_core::Ruler::Symmetric { lines, .. } => Some(*lines),
                        _ => None,
                    })
                    .unwrap_or(app.symmetric_lines);
                if item(ui, &format!("Symmetry lines: {sym} (cycle)"), "") {
                    app.push_cmd(AppCmd::RulerSymmetricCount);
                    ui.close();
                }
                if item(ui, "Guide (horizontal)…", "") {
                    app.push_cmd(AppCmd::RulerArm(crate::cmd::RulerKind::GuideH));
                    ui.close();
                }
                if item(ui, "Guide (vertical)…", "") {
                    app.push_cmd(AppCmd::RulerArm(crate::cmd::RulerKind::GuideV));
                    ui.close();
                }
                ui.separator();
                let on = app.doc.rulers.on;
                if item(ui, if on { "Snap: ON" } else { "Snap: OFF" }, "") {
                    app.push_cmd(AppCmd::RulerSnapToggle);
                    ui.close();
                }
                let spec = app.doc.rulers.special_on;
                if item(
                    ui,
                    if spec {
                        "Special rulers: ON"
                    } else {
                        "Special rulers: OFF"
                    },
                    "",
                ) {
                    app.push_cmd(AppCmd::RulerSpecialSnapToggle);
                    ui.close();
                }
                if item(ui, "Delete all rulers", "") {
                    app.push_cmd(AppCmd::RulerClear);
                    ui.close();
                }
            });
            if item(ui, "New frame border folder", "") {
                app.push_cmd(AppCmd::NewFrameLayer);
                ui.close();
            }
            ui.menu_button("Combine frame folders", |ui| {
                if item(ui, "with next — keep shapes", "") {
                    app.push_cmd(AppCmd::FrameFoldersCombine {
                        merge_borders: false,
                    });
                    ui.close();
                }
                if item(ui, "with next — combine borders", "") {
                    app.push_cmd(AppCmd::FrameFoldersCombine {
                        merge_borders: true,
                    });
                    ui.close();
                }
                if item(ui, "with next — into a common folder", "") {
                    app.push_cmd(AppCmd::FrameFoldersGroup);
                    ui.close();
                }
            });
            ui.menu_button("New live layer", |ui| {
                if item(ui, "Fill (main colour)", "") {
                    let c = app.active_color();
                    app.push_cmd(AppCmd::NewLiveFill(mn_core::FillKind::Flat {
                        color: [c[0], c[1], c[2], 1.0],
                    }));
                    ui.close();
                }
                if item(ui, "Gradient (main → sub)", "") {
                    let (fg, bg) = (app.active_color(), app.sub_color);
                    let (w, h) = (app.doc.size.0 as f32, app.doc.size.1 as f32);
                    app.push_cmd(AppCmd::NewLiveFill(mn_core::FillKind::Gradient {
                        a: [w * 0.2, h * 0.5],
                        b: [w * 0.8, h * 0.5],
                        from: [fg[0], fg[1], fg[2], 1.0],
                        to: [bg[0], bg[1], bg[2], 1.0],
                        mid: app.grad_mid,
                        opts: app.grad_opts,
                    }));
                    ui.close();
                }
                if item(ui, "Tone (60 lpi dots)", "") {
                    app.push_cmd(AppCmd::NewLiveFill(mn_core::FillKind::Tone {
                        tone: mn_core::tone::ToneParams::default(),
                        density: 1.0,
                    }));
                    ui.close();
                }
            });
            if item(ui, "New folder", "") {
                app.push_cmd(AppCmd::AddFolder);
                ui.close();
            }
            if item(ui, "Duplicate layer", "") {
                app.push_cmd(AppCmd::DuplicateLayer);
                ui.close();
            }
            if item(ui, "Delete layer", "") {
                app.push_cmd(AppCmd::RemoveLayer);
                ui.close();
            }
            ui.separator();
            if item(ui, "Merge with layer below", "Ctrl+E") {
                app.push_cmd(AppCmd::MergeDown);
                ui.close();
            }
            if item(ui, "Merge visible to new layer", "Ctrl+Shift+E") {
                app.push_cmd(AppCmd::StampVisible);
                ui.close();
            }
            ui.separator();
            let a = app.doc.active;
            if app.doc.layers.get(a).is_some_and(|l| l.tone.is_some()) {
                if item(ui, "Remove tone (back to plain ink)", "") {
                    app.push_cmd(AppCmd::SetTone(None));
                    ui.close();
                }
            } else if app
                .doc
                .layers
                .get(a)
                .is_some_and(|l| !l.folder && !l.is_vector())
            {
                if item(ui, "Convert to tone layer…", "") {
                    app.push_cmd(AppCmd::SetTone(Some(mn_core::ToneParams::default())));
                    ui.close();
                }
            }
            ui.separator();
            let mut is_ref = app.doc.layers.get(a).is_some_and(|l| l.reference);
            let mut is_draft = app.doc.layers.get(a).is_some_and(|l| l.draft);
            if ui
                .add(egui::Checkbox::new(
                    &mut is_ref,
                    egui::RichText::new("Layer settings ▸ Reference layer").size(12.0),
                ))
                .changed()
            {
                app.push_cmd(AppCmd::SetLayerReference(a, is_ref));
                ui.close();
            }
            if ui
                .add(egui::Checkbox::new(
                    &mut is_draft,
                    egui::RichText::new("Layer settings ▸ Draft layer").size(12.0),
                ))
                .changed()
            {
                app.push_cmd(AppCmd::SetLayerDraft(a, is_draft));
                ui.close();
            }
        });
        // TRIAGE 101/102 — the blur family. Deliberately one small
        // self-contained block: the tonal-correction filters land in this same
        // menu and the two bodies just concatenate.
        bar_menu(ui, "Filter", |ui| {
            ui.menu_button("Blur", |ui| {
                // FL-010: the two no-dialog one-shots.
                if item(ui, "Blur", "") {
                    app.push_cmd(AppCmd::FilterApply(mn_core::Filter::Blur));
                    ui.close();
                }
                if item(ui, "Blur (strong)", "") {
                    app.push_cmd(AppCmd::FilterApply(mn_core::Filter::BlurStrong));
                    ui.close();
                }
                ui.separator();
                // FL-011 / FL-015: the two with parameters.
                if item(ui, "Gaussian blur…", "") {
                    app.push_cmd(AppCmd::FilterOpen(Some(mn_core::Filter::Gaussian {
                        sigma: 4.0,
                    })));
                    ui.close();
                }
                if item(ui, "Motion blur…", "") {
                    app.push_cmd(AppCmd::FilterOpen(Some(mn_core::Filter::Motion {
                        angle: 0.0,
                        length: 20.0,
                        dir: mn_core::MotionDir::Both,
                        mode: mn_core::MotionMode::Uniform,
                    })));
                    ui.close();
                }
                ui.separator();
                // FL-013.
                if item(ui, "Smoothing", "") {
                    app.push_cmd(AppCmd::FilterApply(mn_core::Filter::Smoothing));
                    ui.close();
                }
            });
            ui.menu_button("Effect", |ui| {
                // FL-033.
                if item(ui, "Mosaic…", "") {
                    app.push_cmd(AppCmd::FilterOpen(Some(mn_core::Filter::Mosaic {
                        cell: 8,
                    })));
                    ui.close();
                }
            });
        });
        // TC-002/003/004/005/006/007/011 (CSP 色調補正): whole-layer pixel
        // corrections. Each is one undo step across the selected layers,
        // clipped to the selection when there is one; the parameterised
        // ones open one shared dialog with a live canvas preview.
        bar_menu(ui, "Correction", |ui| {
            // TC-002/003 lead, as in CSP: the two corrections that shape a
            // scan before anything else touches it.
            if item(ui, "Levels…", "") {
                app.push_cmd(AppCmd::AdjustOpen(mn_core::Adjust::LEVELS));
                ui.close();
            }
            if item(ui, "Tone curve…", "") {
                app.push_cmd(AppCmd::AdjustOpen(mn_core::Adjust::TONE_CURVE));
                ui.close();
            }
            ui.separator();
            if item(ui, "Brightness/Contrast…", "") {
                app.push_cmd(AppCmd::AdjustOpen(mn_core::Adjust::BRIGHTNESS_CONTRAST));
                ui.close();
            }
            if item(ui, "Hue/Saturation/Luminosity…", "") {
                app.push_cmd(AppCmd::AdjustOpen(mn_core::Adjust::HUE_SATURATION));
                ui.close();
            }
            if item(ui, "Posterization…", "") {
                app.push_cmd(AppCmd::AdjustOpen(mn_core::Adjust::POSTERIZE));
                ui.close();
            }
            if item(ui, "Reverse gradient", "") {
                app.push_cmd(AppCmd::AdjustNow(mn_core::Adjust::Invert));
                ui.close();
            }
            ui.separator();
            if item(ui, "Binarization…", "") {
                app.push_cmd(AppCmd::AdjustOpen(mn_core::Adjust::BINARIZE));
                ui.close();
            }
        });
        // "Workspace" replaces the old Window menu (owner rename 2026-08-21):
        // one flat menu — palette toggles, layout reset, and the registered
        // workspaces — no submenu hunting for either half.
        bar_menu(ui, "Workspace", |ui| {
            if item(ui, "Command palette…", "Ctrl+K") {
                crate::ui::open_command_palette(app);
                ui.close();
            }
            ui.separator();
            ui.weak("closed palettes reopen beside Layers");
            for p in crate::ui::dock::ALL {
                let mut open = crate::ui::dock::is_open(app, p);
                if ui.checkbox(&mut open, p.title()).changed() {
                    // A real toggle (owner report 2026-08-22: the boxes
                    // wouldn't UNcheck): check reopens beside Layers,
                    // uncheck removes the palette wherever it lives.
                    if open {
                        crate::ui::dock::reopen(app, p);
                    } else {
                        crate::ui::dock::close_palette(app, p);
                    }
                    ui.close();
                }
            }
            let mut all_default = false;
            if ui.checkbox(&mut all_default, "Reset layout").changed() {
                app.dock = crate::ui::dock::default_tree();
                ui.close();
            }
            ui.separator();
            let mut apply = None;
            let mut delete = None;
            // Index-guarded: a workspace entry is variable-length (app.rs).
            let names: Vec<String> = app
                .workspaces
                .iter()
                .filter_map(|e| e.first().cloned())
                .collect();
            for n in names {
                ui.horizontal(|ui| {
                    let mark = if app.workspace_current == n {
                        "✓ "
                    } else {
                        "   "
                    };
                    if ui.button(format!("{mark}{n}")).clicked() {
                        apply = Some(n.clone());
                    }
                    if ui.small_button("✕").clicked() {
                        delete = Some(n);
                    }
                });
            }
            if app.workspaces.is_empty() {
                ui.weak("(no workspaces registered)");
            }
            if item(ui, "Register Workspace…", "") {
                app.workspace_open = true;
                ui.close();
            }
            if item(ui, "Reload workspace", "") {
                if !app.workspace_reload() {
                    app.set_status("no current workspace to reload");
                }
                ui.close();
            }
            if let Some(n) = apply {
                if app.workspace_apply(&n) {
                    app.set_status(format!("workspace: {n}"));
                }
            }
            if let Some(n) = delete {
                app.workspace_delete(&n);
            }
        });
        // "Manga" — the owner's rename (his screenshot's CSP menu is
        // Story; Manga reads better here and holds the reader).
        bar_menu(ui, "Manga", |ui| {
            if item(ui, "Reader — read the chapter", "") {
                app.push_cmd(AppCmd::ReaderOpen);
                ui.close();
            }
            if ui
                .add_enabled(
                    app.reader.visited && !app.reader.open,
                    egui::Button::new("Return to reader"),
                )
                .clicked()
            {
                app.push_cmd(AppCmd::ReaderReturn);
                ui.close();
            }
            ui.separator();
            if item(ui, "First page", "Ctrl+Home") {
                app.push_cmd(AppCmd::PageFirst);
                ui.close();
            }
            if item(ui, "Previous page", "Ctrl+PageUp") {
                app.push_cmd(AppCmd::PagePrev);
                ui.close();
            }
            if item(ui, "Go to Page…", "") {
                app.push_cmd(AppCmd::PageGoto);
                ui.close();
            }
            if item(ui, "Next page", "Ctrl+PageDown") {
                app.push_cmd(AppCmd::PageNext);
                ui.close();
            }
            if item(ui, "Last page", "Ctrl+End") {
                app.push_cmd(AppCmd::PageLast);
                ui.close();
            }
            ui.separator();
            if item(ui, "Story Editor…", "") {
                app.push_cmd(AppCmd::StoryEditor);
                ui.close();
            }
            ui.separator();
            if item(ui, "Combine with next page…", "") {
                app.push_cmd(AppCmd::PageCombineSpread);
                ui.close();
            }
            if item(ui, "Split spread…", "") {
                app.push_cmd(AppCmd::PageSplitSpread);
                ui.close();
            }
            ui.separator();
            if item(ui, "Add page", "") {
                app.push_cmd(AppCmd::AddPage);
                ui.close();
            }
            if item(ui, "Duplicate page", "") {
                app.push_cmd(AppCmd::DuplicatePage);
                ui.close();
            }
            if item(ui, "Delete page", "") {
                app.push_cmd(AppCmd::DeletePage);
                ui.close();
            }
            ui.separator();
            if item(ui, "Import file as page…", "") {
                app.push_cmd(AppCmd::ImportPage);
                ui.close();
            }
            if item(ui, "Replace page with file…", "") {
                app.push_cmd(AppCmd::ReplacePage);
                ui.close();
            }
            ui.separator();
            if item(ui, "Work settings…", "") {
                app.push_cmd(AppCmd::WorkSettings);
                ui.close();
            }
        });
        bar_menu(ui, "View", |ui| {
            if item(ui, "Fit to window", "Ctrl+0") {
                app.push_cmd(AppCmd::ZoomFit);
                ui.close();
            }
            if item(ui, "Zoom 100%", "Ctrl+1") {
                app.push_cmd(AppCmd::Zoom100);
                ui.close();
            }
            ui.separator();
            if item(
                ui,
                if app.touch_probe.enabled {
                    "Input probe: ON (pen/touch/mouse diagnostics)"
                } else {
                    "Input probe (pen/touch/mouse diagnostics)"
                },
                "",
            ) {
                app.touch_probe.enabled = !app.touch_probe.enabled;
                ui.close();
            }
            if item(
                ui,
                if app.frame_order_show {
                    "Panel reading order: SHOWN"
                } else {
                    "Panel reading order: hidden"
                },
                "",
            ) {
                app.frame_order_show = !app.frame_order_show;
                ui.close();
            }
            // TN-011: the pre-print sweep — every toned region of every
            // visible layer tinted, so a forgotten scrap of tone shows up
            // before the page does.
            if item(
                ui,
                if app.tone_show_area {
                    "✓ Show tone area"
                } else {
                    "Show tone area"
                },
                "",
            ) {
                app.push_cmd(AppCmd::ToneShowArea);
                ui.close();
            }
            // Touch gestures, one switch each (GS-008). They ship OFF and
            // there was no way to turn them on but hand-editing ui.txt with
            // the app closed — which is not a feature anyone will find.
            //
            // Individually switchable because a resting palm is also two
            // fingers: whoever finds that undo fires while their hand rests
            // needs to disable THAT gesture without losing the others.
            ui.menu_button("Touch gestures", |ui| {
                for (bit, label, tip) in [
                    (
                        crate::gesture::UNDO,
                        "Two-finger tap: undo",
                        "A quick tap with two fingers undoes. Panning and pinching are unaffected.",
                    ),
                    (
                        crate::gesture::REDO,
                        "Three-finger tap: redo",
                        "A quick tap with three fingers redoes.",
                    ),
                    (
                        crate::gesture::RESET_VIEW,
                        "Three-finger tap on the Navigator: reset rotation and flip",
                        "Puts a twisted or mirrored canvas back upright.",
                    ),
                ] {
                    let on = app.layout.touch_gestures & bit != 0;
                    if ui.selectable_label(on, label).on_hover_text(tip).clicked() {
                        app.layout.touch_gestures ^= bit;
                        app.set_status(if app.layout.touch_gestures & bit != 0 {
                            "touch gesture on"
                        } else {
                            "touch gesture off"
                        });
                    }
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(
                        "All off by default: a resting palm reads as two fingers,\n\
                         and a phantom undo mid-stroke costs more than the gesture saves.",
                    )
                    .size(10.0)
                    .color(theme::c().text_weak),
                );
            });
            ui.separator();
            // The step is the `rotate_step_deg` preference (15° shipped);
            // the two toolbar buttons and the `-` / F9 keys read the same one.
            let step = app.prefs.rotate_step_deg.to_radians();
            if item(ui, "Rotate clockwise", "F9") {
                app.push_cmd(AppCmd::RotateView(step));
                ui.close();
            }
            if item(ui, "Rotate counter-clockwise", "-") {
                app.push_cmd(AppCmd::RotateView(-step));
                ui.close();
            }
            // CV-035, the three resets. Rotation alone is the one you want
            // mid-drawing; the other two exist because "put the page back"
            // otherwise means remembering which of rotate, flip and zoom
            // you touched. Zoom and pan survive the middle one on purpose.
            if item(ui, "Reset rotation", "") {
                app.push_cmd(AppCmd::RotateReset);
                ui.close();
            }
            if item(ui, "Reset rotation and flip", "") {
                app.push_cmd(AppCmd::RotateFlipReset);
                ui.close();
            }
            if item(ui, "Reset view (upright, unmirrored, fitted)", "") {
                app.push_cmd(AppCmd::ViewReset);
                ui.close();
            }
            ui.separator();
            if item(ui, "Flip horizontal (view)", "Ctrl+9") {
                app.push_cmd(AppCmd::FlipView);
                ui.close();
            }
            if item(ui, "Flip vertical (view)", "Ctrl+Shift+9") {
                app.push_cmd(AppCmd::FlipViewV);
                ui.close();
            }
            ui.separator();
            // CV-041. Phrased as what it shows, ticked when shown, so the
            // menu never has to say "un-hide". Persisted (ui.txt), unlike
            // the Tab hides below it.
            let mut guides = !app.layout.guides_hidden;
            if ui
                .checkbox(&mut guides, "Crop marks and margins")
                .on_hover_text(
                    "Bleed, trim, inner border and safety margins. Hiding them\n\
                     changes nothing on the page — panels still snap to them,\n\
                     and they were never exported.",
                )
                .changed()
            {
                app.push_cmd(AppCmd::SetGuidesHidden(!guides));
                ui.close();
            }
            ui.separator();
            // UI-031/032. Menu items as well as keys: a hide whose only
            // way back is a key you have to remember is a trap, and the
            // top bar is also this window's title bar.
            if item(
                ui,
                if app.panels_hidden {
                    "Show palettes"
                } else {
                    "Hide palettes"
                },
                "Tab",
            ) {
                app.panels_hidden = !app.panels_hidden;
                ui.close();
            }
            if item(
                ui,
                if app.chrome_hidden {
                    "Show menu and status bars"
                } else {
                    "Hide menu and status bars"
                },
                "Shift+Tab",
            ) {
                app.chrome_hidden = !app.chrome_hidden;
                ui.close();
            }
            ui.separator();
            let mut mx = app.mirror_x;
            if ui
                .checkbox(&mut mx, "Symmetry X (mirror strokes)")
                .changed()
            {
                app.push_cmd(AppCmd::SetMirrorX(mx));
                ui.close();
            }
            let mut my = app.mirror_y;
            if ui
                .checkbox(&mut my, "Symmetry Y (mirror strokes)")
                .changed()
            {
                app.push_cmd(AppCmd::SetMirrorY(my));
                ui.close();
            }
            let mut wx = app.wrap_x;
            if ui
                .checkbox(&mut wx, "Tile X (wrap strokes at edges)")
                .changed()
            {
                app.push_cmd(AppCmd::SetWrapX(wx));
                ui.close();
            }
            let mut wy = app.wrap_y;
            if ui
                .checkbox(&mut wy, "Tile Y (wrap strokes at edges)")
                .changed()
            {
                app.push_cmd(AppCmd::SetWrapY(wy));
                ui.close();
            }
            ui.separator();
            // The user-facing --gpu-dabs switch (TODO #0.1): persisted in
            // ui.txt, takes effect from the next stroke on. Greyed out on
            // adapters without rgba16uint storage rather than a dead toggle.
            let mut gd = app.gpu_dabs;
            if ui
                .add_enabled(
                    app.renderer.gpu_dabs_supported(),
                    egui::Checkbox::new(&mut gd, "GPU inking"),
                )
                .on_disabled_hover_text(
                    "this adapter has no rgba16uint storage — cpu dab path only",
                )
                .changed()
            {
                app.push_cmd(AppCmd::SetGpuDabs(gd));
                ui.close();
            }
            ui.separator();
            paper_menu(ui, app);
        });
        // Help — LAST, where every app puts it (the owner missed it mid-bar).
        // The manual's door (MANUAL-PLAN: ships offline beside the exe and
        // opens from here; F1 stays the diagnostics HUD), and the feedback
        // window (GitHub issues + the dev's email).
        bar_menu(ui, "Help", |ui| {
            if item(
                ui,
                "Manual — the quirks, not a feature tour",
                "opens in your browser",
            ) {
                app.push_cmd(AppCmd::OpenManual);
                ui.close();
            }
            if item(
                ui,
                "Report Bug / Feature Request / Feedback…",
                "GitHub or email",
            ) {
                app.feedback_open = true;
                ui.close();
            }
            if item(ui, "Diagnostics (F1)", "log path, version, counters") {
                app.push_cmd(AppCmd::ToggleHud);
                ui.close();
            }
        });

        // --- the menu row ENDS at the menus --------------------------------
        //
        // The icon commands used to ride this row too, wedged between the
        // menus and the − □ × cluster. CSP gives its command bar a strip of
        // its own under the menus (`csp/060_pc_0001.png`, region 2) — ours is
        // `command_row`, added right after this bar.
        //
        // NARROW WINDOWS (owner report 2026-08-20): the caption cluster's
        // width is RESERVED up front so − □ × can never be pushed off the end
        // of the row, and the window title paints only where there is free
        // space for it (it used to paint at the bar centre unconditionally,
        // on top of whatever was there). WM_GETMINMAXINFO (main.rs) floors
        // the window so the menus + caption always fit.
        const CAPTION_W: f32 = 3.0 * 34.0 + 42.0; // − □ ×, separator, F1
        let flow_end = ui.cursor().min.x;

        // Window title — the doc tab names the page; this names the window
        // (and the taskbar keeps it too). It paints ONLY in free space:
        // bar-centred when the centre lies in the gap between the last
        // command widget and the caption cluster, else centred in that gap,
        // else not at all. Before this it painted at the bar centre
        // unconditionally, which at most widths sat ON TOP of the zoom and
        // rotate buttons (the owner's "ghost text over the menus").
        let cap_left = bar.right() - CAPTION_W;
        let title = app.desired_title();
        let galley = ui.fonts_mut(|f| {
            f.layout_no_wrap(
                title,
                egui::FontId::proportional(10.5),
                theme::c().text_weak.gamma_multiply(0.8),
            )
        });
        let (half_w, pad) = (galley.size().x * 0.5, 8.0);
        let (lo, hi) = (flow_end + pad, cap_left - pad);
        let c = bar.center();
        let cx = if c.x - half_w >= lo && c.x + half_w <= hi {
            Some(c.x)
        } else if hi - lo >= half_w * 2.0 {
            Some((lo + hi) * 0.5)
        } else {
            None
        };
        if let Some(cx) = cx {
            ui.painter().galley(
                egui::pos2(cx - half_w, c.y - galley.size().y * 0.5),
                galley,
                theme::c().text_weak.gamma_multiply(0.8),
            );
        }

        // The caption cluster, LAST and in a fixed right-anchored strip:
        // registered after everything else so it wins both the hit test and
        // the paint order — whatever else happens to the row, the window's
        // own controls are always where Windows puts them.
        let cap_rect =
            egui::Rect::from_min_max(egui::pos2(cap_left, bar.top()), bar.right_bottom());
        let mut cap = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(cap_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        caption_button(&mut cap, app, Caption::Close);
        caption_button(&mut cap, app, Caption::Max);
        caption_button(&mut cap, app, Caption::Min);
        cap.separator();
        let mut hud = app.hud_open;
        if cap
            .toggle_value(&mut hud, "F1")
            .on_hover_text("Diagnostics")
            .changed()
        {
            app.hud_open = hud;
        }
    });
    command_row(ui, app);
}

/// The command bar: CSP puts its icon commands on a slim strip of their own
/// UNDER the menus (`csp/060_pc_0001.png`, region 2) rather than beside them,
/// which is what lets a command bar exist at all on a narrow window. Ours
/// costs one slim row: 18 px icons, no separator rules of its own beyond the
/// cluster dividers.
///
/// Clusters are still added only if they fit — the row is full width now, so
/// this bites far later than it did on the shared row, but a very narrow
/// window drops rotate before zoom before undo rather than wrapping.
fn command_row(ui: &mut egui::Ui, app: &mut App) {
    const S: f32 = 18.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        // Empty command-row space drags the window like the menu row's does
        // (registered first, so every button below sits above it in the hit
        // test — same trick as the menu bar's caption strip).
        // Height-clamped: `available_rect_before_wrap` in an auto-sized panel
        // can report the rest of the window, and a drag region that tall
        // would swallow the canvas.
        let avail = ui.available_rect_before_wrap();
        let strip = egui::Rect::from_min_size(avail.min, egui::vec2(avail.width(), S + 4.0));
        let drag = ui.interact(
            strip,
            egui::Id::new("mn.cmdrow.drag"),
            egui::Sense::click_and_drag(),
        );
        if drag.drag_started() {
            app.drag_window = true;
        }
        if drag.double_clicked() {
            app.caption_cmd = Some(CaptionCmd::ToggleMax);
        }
        let fits = |ui: &egui::Ui, need: f32| -> bool { strip.right() - ui.cursor().min.x >= need };

        if fits(ui, 50.0) {
            if icon_btn(ui, Icon::Undo, S, false, true, "Undo (Ctrl+Z)").clicked() {
                app.push_cmd(AppCmd::Undo);
            }
            if icon_btn(ui, Icon::Redo, S, false, true, "Redo (Ctrl+Y)").clicked() {
                app.push_cmd(AppCmd::Redo);
            }
        }
        if fits(ui, 150.0) {
            ui.separator();
            if icon_btn(ui, Icon::ZoomOut, S, false, true, "Zoom out").clicked() {
                app.push_cmd(AppCmd::ZoomStep(1.0 / 1.25));
            }
            ui.label(
                egui::RichText::new(format!("{:>4.0}%", app.viewport.zoom * 100.0))
                    .monospace()
                    .size(10.5)
                    .color(theme::c().text_weak),
            );
            if icon_btn(ui, Icon::ZoomIn, S, false, true, "Zoom in").clicked() {
                app.push_cmd(AppCmd::ZoomStep(1.25));
            }
            if icon_btn(ui, Icon::ZoomFit, S, false, true, "Fit to window (Ctrl+0)").clicked() {
                app.push_cmd(AppCmd::ZoomFit);
            }
            if icon_btn(ui, Icon::Zoom100, S, false, true, "Zoom 100% (Ctrl+1)").clicked() {
                app.push_cmd(AppCmd::Zoom100);
            }
        }
        if fits(ui, 150.0) {
            ui.separator();
            let step = app.prefs.rotate_step_deg.to_radians();
            if icon_btn(ui, Icon::RotateLeft, S, false, true, "Rotate CCW (-)").clicked() {
                app.push_cmd(AppCmd::RotateView(-step));
            }
            if icon_btn(ui, Icon::RotateRight, S, false, true, "Rotate CW (F9)").clicked() {
                app.push_cmd(AppCmd::RotateView(step));
            }
            if icon_btn(ui, Icon::RotateReset, S, false, true, "Reset rotation").clicked() {
                app.push_cmd(AppCmd::RotateReset);
            }
            if icon_btn(
                ui,
                Icon::FlipH,
                S,
                app.viewport.flip_h,
                true,
                "Flip view horizontally (Ctrl+9)",
            )
            .clicked()
            {
                app.push_cmd(AppCmd::FlipView);
            }
            if icon_btn(
                ui,
                Icon::FlipV,
                S,
                app.viewport.flip_v,
                true,
                "Flip view vertically (Ctrl+Shift+9)",
            )
            .clicked()
            {
                app.push_cmd(AppCmd::FlipViewV);
            }
        }
    });
}

// --- caption buttons (custom title bar) -----------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Caption {
    Min,
    Max,
    Close,
}

/// Windows 10 caption hit zone: flat on the bar, quiet glyph, hover fill —
/// red for close, the theme hover grey otherwise. Clicks become App flags;
/// `main::pump_commands` does the actual ShowWindow outside the wndproc.
fn caption_button(ui: &mut egui::Ui, app: &mut App, kind: Caption) {
    let h = ui.available_height().max(19.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(34.0, h), egui::Sense::click());
    let hover = resp.hovered();
    let bg = if hover && kind == Caption::Close {
        egui::Color32::from_rgb(0xe8, 0x11, 0x23)
    } else if hover {
        theme::c().hover
    } else {
        theme::c().window
    };
    let color = if hover {
        theme::c().text_strong
    } else {
        theme::c().text_weak
    };
    let p = ui.painter();
    if hover {
        p.rect_filled(rect, 0.0, bg);
    }
    let c = rect.center();
    let s = |dx: f32, dy: f32| egui::pos2(c.x + dx, c.y + dy);
    match kind {
        Caption::Min => {
            p.line_segment([s(-5.0, 3.5), s(5.0, 3.5)], egui::Stroke::new(1.5, color));
        }
        Caption::Max if !app.win_maximized => {
            let r = egui::Rect::from_center_size(c, egui::vec2(10.0, 9.0));
            p.rect_stroke(
                r,
                0.0,
                egui::Stroke::new(1.2, color),
                egui::StrokeKind::Inside,
            );
            // Thicker top edge — the classic maximize glyph.
            p.line_segment(
                [
                    egui::pos2(r.left(), r.top() + 0.6),
                    egui::pos2(r.right(), r.top() + 0.6),
                ],
                egui::Stroke::new(2.2, color),
            );
        }
        Caption::Max => {
            // Restore: the back window's two edges behind a filled front one.
            let back =
                egui::Rect::from_min_size(egui::pos2(c.x - 4.5, c.y - 4.5), egui::vec2(9.0, 9.0));
            let front =
                egui::Rect::from_min_size(egui::pos2(c.x - 1.5, c.y - 1.5), egui::vec2(9.0, 9.0));
            p.line_segment(
                [back.left_top(), back.right_top()],
                egui::Stroke::new(1.2, color),
            );
            p.line_segment(
                [back.left_top(), back.left_bottom()],
                egui::Stroke::new(1.2, color),
            );
            p.rect_filled(front, 0.0, bg);
            p.rect_stroke(
                front,
                0.0,
                egui::Stroke::new(1.2, color),
                egui::StrokeKind::Inside,
            );
        }
        Caption::Close => {
            p.line_segment([s(-4.5, -4.5), s(4.5, 4.5)], egui::Stroke::new(1.3, color));
            p.line_segment([s(4.5, -4.5), s(-4.5, 4.5)], egui::Stroke::new(1.3, color));
        }
    }
    if resp.clicked() {
        match kind {
            Caption::Close => app.close_requested = true,
            Caption::Min => app.caption_cmd = Some(CaptionCmd::Minimize),
            Caption::Max => app.caption_cmd = Some(CaptionCmd::ToggleMax),
        }
    }
}

// --- document tab strip --------------------------------------------------

/// The strip's height — the canvas pane body (ui/dock.rs) reserves exactly
/// this above the canvas hole.
pub(super) const DOC_TAB_H: f32 = 25.0;

/// The document tab strip — ONE TAB PER OPEN DOCUMENT since 2026-08-19.
///
/// It used to draw a single tab whose × set `close_requested`, i.e. quit the
/// application. The owner's words: *"clicking x on the canvas for the only
/// page in a manga closes the whole window — that whole kind of behavior
/// seems dumb."* Now × closes that DOCUMENT, and only the last one standing
/// falls back to the app's own close flow (which still asks about unsaved
/// work), because an editor with no document open has nothing to show.
pub(super) fn doc_tab(ui: &mut egui::Ui, app: &mut App) {
    let h = DOC_TAB_H;
    let tabs = app.doc_tabs();
    let active = app.active_doc.min(tabs.len().saturating_sub(1));
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());

    // Tabs share the strip when there are many, exactly like the palette tab
    // bar does — a fixed width would push the later ones off the edge.
    let avail = rect.width() - 12.0;
    let font = egui::FontId::proportional(11.5);
    let mut switch_to: Option<usize> = None;
    let mut close_at: Option<usize> = None;
    let mut x = rect.left() + 6.0;

    for (i, (label, dirty)) in tabs.iter().enumerate() {
        let galley =
            ui.fonts_mut(|f| f.layout_no_wrap(label.clone(), font.clone(), theme::c().text));
        let dot_w = if *dirty { 11.0 } else { 0.0 };
        let want = galley.size().x + 12.0 + dot_w + 22.0;
        let share = (avail / tabs.len() as f32).max(60.0);
        let tab_w = want.min(share.max(want.min(share)));
        let tab = egui::Rect::from_min_size(
            egui::pos2(x, rect.top() + 3.0),
            egui::vec2(tab_w.min(rect.right() - x - 4.0).max(24.0), h - 3.0),
        );
        if tab.width() < 24.0 {
            break;
        }
        let is_active = i == active;
        let close = egui::Rect::from_center_size(
            egui::pos2(tab.right() - 13.0, tab.center().y),
            egui::vec2(14.0, 14.0),
        );
        let tresp = ui.interact(tab, ui.id().with(("mn.tab", i)), egui::Sense::click());
        let cresp = ui.interact(
            close,
            ui.id().with(("mn.tab.close", i)),
            egui::Sense::click(),
        );

        let p = ui.painter();
        let radius = egui::CornerRadius {
            nw: 5,
            ne: 5,
            sw: 0,
            se: 0,
        };
        p.rect_filled(
            tab,
            radius,
            if is_active {
                theme::c().panel
            } else if tresp.hovered() {
                theme::c().hover
            } else {
                theme::c().header
            },
        );
        // Accent edge along the top of the ACTIVE tab, Photoshop-style.
        if is_active {
            p.rect_filled(
                egui::Rect::from_min_size(tab.min, egui::vec2(tab.width(), 2.0)),
                radius,
                theme::c().accent,
            );
        }
        let ty = tab.center().y + 0.5;
        // Clip the label to what is left after the × and the dirty dot, so a
        // narrow strip truncates instead of painting over its own controls.
        let text_room = (tab.width() - 20.0 - dot_w - 8.0).max(0.0);
        p.with_clip_rect(egui::Rect::from_min_size(
            egui::pos2(tab.left() + 8.0, tab.top()),
            egui::vec2(text_room, tab.height()),
        ))
        .galley(
            egui::pos2(tab.left() + 8.0, ty - galley.size().y * 0.5),
            galley.clone(),
            if is_active {
                theme::c().text
            } else {
                theme::c().text_weak
            },
        );
        if *dirty {
            let dx =
                (tab.left() + 8.0 + galley.size().x.min(text_room) + 6.0).min(close.left() - 5.0);
            p.circle_filled(egui::pos2(dx, ty), 2.5, theme::c().text_weak);
        }
        if cresp.hovered() {
            p.rect_filled(close, 2.0, theme::c().hover);
        }
        paint_icon(
            p,
            close.shrink(3.5),
            Icon::Close,
            if cresp.hovered() {
                theme::c().text_strong
            } else {
                theme::c().text_weak
            },
        );

        if cresp.clicked() {
            close_at = Some(i);
        } else if tresp.clicked() && !is_active {
            switch_to = Some(i);
        }
        x = tab.right() + 2.0;
    }

    // Acted on after the loop: both of these move documents around, and
    // doing that while iterating the tab list would read stale labels.
    if let Some(i) = close_at {
        // NEVER close a document with unsaved work from here. The prompt
        // lives in `pump_commands` (it needs a message loop and no `&mut
        // App` alive), so a dirty tab hands the whole thing over: switch to
        // it so the question is about something visible, then request the
        // app-close flow, which asks per document and — on "No" — discards
        // only that one and carries on.
        //
        // This arm shipped without the check for one round and destroyed a
        // dirty tab on a single click, which is the same hole the quit path
        // had. Two agents found it independently; the lesson is that adding
        // a second way to close a document means auditing every way.
        app.close_doc_requested = Some(i);
    } else if let Some(i) = switch_to {
        app.switch_doc(i);
    }
}

// --- status bar ---------------------------------------------------------

pub(super) fn status_bar(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        let seg = |ui: &mut egui::Ui, v: String| {
            ui.label(
                egui::RichText::new(v)
                    .monospace()
                    .size(10.5)
                    .color(theme::c().text),
            );
            ui.separator();
        };
        // M12: the zoom and rotation cells are CONTROLS, not printout —
        // CSP docks a whole strip of them under the canvas. Every action
        // routes through the existing view commands; the bar computes
        // nothing of its own.
        ui.spacing_mut().item_spacing.x = 3.0;
        if ui.small_button("−").on_hover_text("zoom out").clicked() {
            let s = app.prefs.wheel_step.max(1.02);
            app.push_cmd(AppCmd::ZoomStep(1.0 / s));
        }
        let mut zoom_pct = app.viewport.zoom * 100.0;
        let zr = ui.add(
            egui::DragValue::new(&mut zoom_pct)
                .range(1.0..=6400.0)
                .speed(1.0)
                .fixed_decimals(0)
                .suffix("%"),
        );
        if zr.changed() && app.viewport.zoom > 0.0 {
            app.push_cmd(AppCmd::ZoomStep((zoom_pct / 100.0) / app.viewport.zoom));
        }
        zr.on_hover_text("drag or type; double-click to type an exact zoom");
        if ui.small_button("＋").on_hover_text("zoom in").clicked() {
            app.push_cmd(AppCmd::ZoomStep(app.prefs.wheel_step.max(1.02)));
        }
        if ui
            .small_button("fit")
            .on_hover_text("fit the page in the window")
            .clicked()
        {
            app.push_cmd(AppCmd::ZoomFit);
        }
        ui.separator();
        let mut rot_deg = app.viewport.rotate_rad.to_degrees();
        let rr = ui.add(
            egui::DragValue::new(&mut rot_deg)
                .range(-180.0..=180.0)
                .speed(0.5)
                .fixed_decimals(1)
                .suffix("°"),
        );
        if rr.changed() {
            app.push_cmd(AppCmd::RotateView(
                (rot_deg - app.viewport.rotate_rad.to_degrees()).to_radians(),
            ));
        }
        rr.on_hover_text("view rotation — the page, not the art");
        if app.viewport.rotate_rad.abs() > 1e-4
            && ui
                .small_button("0°")
                .on_hover_text("reset rotation")
                .clicked()
        {
            app.push_cmd(AppCmd::RotateReset);
        }
        ui.separator();
        if app.viewport.flip_h {
            seg(ui, "mirror".to_owned());
        }
        if app.viewport.flip_v {
            seg(ui, "mirror V".to_owned());
        }
        if app.mirror_x || app.mirror_y {
            seg(
                ui,
                format!(
                    "symmetry {}{}",
                    if app.mirror_x { "X" } else { "" },
                    if app.mirror_y { "Y" } else { "" }
                ),
            );
        }
        if app.wrap_x || app.wrap_y {
            seg(
                ui,
                format!(
                    "tile {}{}",
                    if app.wrap_x { "X" } else { "" },
                    if app.wrap_y { "Y" } else { "" }
                ),
            );
        }
        if app.is_comic() {
            seg(ui, format!("p {}/{}", app.page_index + 1, app.pages.len()));
        }
        let (w, h) = app.doc.size;
        let size_txt = match &app.page {
            Some(p) => format!("{w}x{h}px @{}dpi — {}", p.dpi, p.name),
            None => format!("{w}x{h}px"),
        };
        ui.label(
            egui::RichText::new(size_txt)
                .size(10.5)
                .color(theme::c().text_weak),
        );
        if !app.status.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new(app.status.clone()).size(10.5).color(
                if app.status_warn {
                    theme::c().warn
                } else {
                    theme::c().text_weak
                },
            ));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Right-to-left: what is added first sits furthest right, so
            // the layer name and brush keep the corner they have always
            // had and the reminder chip lands to their left.
            let layer = app
                .doc
                .layers
                .get(app.doc.active)
                .map(|l| l.name.clone())
                .unwrap_or_default();
            ui.label(egui::RichText::new(layer).size(10.5).color(theme::c().text));
            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    // The honest dab diameter, full stop — the old "×2.00"
                    // tail was the multiplier model that capped the ladder.
                    "{}  {:.1}px{}",
                    app.brush_name(),
                    app.brush_radius() * 2.0,
                    if app.eraser_active() { "  [erase]" } else { "" }
                ))
                .size(10.5)
                .color(theme::c().text),
            );
            unexported_chip(ui, app);
        });
    });
}

/// The unexported-pages reminder (owner ask 2026-08-22): "I fixed two
/// panels and forgot to re-export".
///
/// Deliberately the quietest thing on the bar — weak text, no fill, no
/// warn colour. It is a memory aid, not a problem: nothing is broken, the
/// files on disk are simply older than the pages. It says nothing until
/// the work has been exported at least once (a work that has never left
/// the app has nothing to be reminded about), and nothing at all when the
/// preference is off.
fn unexported_chip(ui: &mut egui::Ui, app: &mut App) {
    if !app.prefs.export_reminder {
        return;
    }
    let n = app.unexported_pages();
    if n == 0 {
        return;
    }
    ui.separator();
    let text = if n == 1 {
        "1 page unexported".to_owned()
    } else {
        format!("{n} pages unexported")
    };
    let hit = ui
        .add(
            egui::Label::new(
                egui::RichText::new(text)
                    .size(10.5)
                    .color(theme::c().text_weak),
            )
            .sense(egui::Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(
            "These pages changed since the last export wrote them. \
             Click to open Export All Pages.",
        );
    if hit.clicked() {
        app.push_cmd(AppCmd::ExportAllPages);
    }
}

// --- paper (PA-001, TRIAGE 100 / CSP OL-005) -----------------------------

/// CSP's paper is a *layer* at the bottom of the stack; ours is a document
/// property with the same two knobs, because a row you cannot draw on, move,
/// delete or reorder is not a layer — it is the page.
///
/// Two knobs, and they are deliberately different in kind:
/// - **Show paper** is view state, like a layer's eye. Not undoable, and it
///   does not change one pixel of an exported PNG. Off, the canvas shows the
///   transparency checker under the art — which is how you find the missed
///   spot in a flat fill that is invisible against white.
/// - **The colour** is document content. It is what an empty page composites
///   to and what an export writes, so a cream page prints cream. Undoable.
///
/// The colours are a preset list rather than a picker: menus and colour
/// popups nest badly, and the artist already has a full picker in the Color
/// palette — hence "Use the current drawing colour", which covers everything
/// the presets do not.
fn paper_menu(ui: &mut egui::Ui, app: &mut App) {
    ui.menu_button("Paper", |ui| {
        let mut on = app.doc.paper.visible;
        if ui
            .checkbox(&mut on, "Show paper")
            .on_hover_text(
                "Off shows the transparency checker where the page is transparent — \
                 a check for holes in your flats. It never changes an export.",
            )
            .changed()
        {
            app.push_cmd(AppCmd::PaperToggle);
            ui.close();
        }
        ui.separator();
        let cur = app.doc.paper.colour;
        for (name, rgb) in PAPER_PRESETS {
            let mark = if cur == *rgb { "✓ " } else { "   " };
            if item(ui, &format!("{mark}{name}"), "") {
                app.push_cmd(AppCmd::SetPaperColour(*rgb));
                ui.close();
            }
        }
        ui.separator();
        if item(ui, "Use the current drawing colour", "") {
            let c = app.main_color;
            app.push_cmd(AppCmd::SetPaperColour([
                (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            ]));
            ui.close();
        }
        ui.separator();
        ui.label(
            egui::RichText::new(
                "The colour is part of the document and exports.\n\
                 The eye is not, and never does.",
            )
            .size(10.0)
            .color(theme::c().text_weak),
        );
    });
}

/// Grounds an inker actually asks for: white, the two warm papers a scan or a
/// printed page lands on, and two greys for judging values (black lineart on
/// mid grey is the classic way to see whether your darks are carrying).
const PAPER_PRESETS: &[(&str, [u8; 3])] = &[
    ("White", [255, 255, 255]),
    ("Cream", [250, 243, 224]),
    ("Newsprint", [235, 227, 206]),
    ("Light grey", [214, 214, 216]),
    ("Mid grey", [128, 128, 130]),
    ("Black", [0, 0, 0]),
];
