//! `AppCmd` arms: print, and every path-taking open/save/export
//! (`.ora`, `.mnc`, `.psd`, `.png`, text dumps) plus autosave.

use super::*;

pub(super) fn run(app: &mut App, cmd: AppCmd, cmd_tail: CmdTail) {
    match cmd {
        AppCmd::Print => {
            app.print_open = true;
        }
        // Resolved to `PrintResult` by `main::pump_commands`; reaching here
        // means the shell never got the chance (headless, scripts).
        AppCmd::PrintGo => {}
        AppCmd::PrintResult { msg, warn } => {
            if warn {
                app.set_error(msg);
            } else {
                app.set_status(msg);
            }
        }
        AppCmd::ZoomPrintSize => {
            let mon = app.monitor_dpi();
            match crate::app::print::print_zoom(app.work_dpi(), mon) {
                Some(z) => {
                    let c = app.canvas_center();
                    let cur = app.viewport.zoom;
                    if cur > 0.0 {
                        app.viewport.zoom_around(c, z / cur);
                    }
                    app.set_status(format!(
                        "print size: 1 page mm = 1 screen mm ({}% — {} dpi page on a {:.0} dpi display)",
                        (z * 100.0).round() as i32,
                        app.work_dpi().unwrap_or(0),
                        mon
                    ));
                    app.mark_dirty();
                }
                // The honest refusal. Inventing 96 or 600 here would put a
                // page on screen at a size the owner would then measure
                // tone density against.
                None => app.set_error(
                    "print size needs a page dpi — this canvas is measured in pixels only \
                     (give the work a page setup in File ▸ Work settings)",
                ),
            }
        }
        AppCmd::ExportText => {}
        AppCmd::ExportTextPath(p) => {
            // A half-typed balloon is still the document's text: land it
            // before walking the stack.
            app.commit_text_edit();
            let body = app.script_dump();
            match std::fs::write(&p, body) {
                Ok(()) => app.set_status(format!("script -> {}", p.display())),
                Err(e) => app.set_error(format!("script export failed: {e}")),
            }
        }
        AppCmd::OpenOra
        | AppCmd::SaveOra
        | AppCmd::SaveOraAs
        | AppCmd::ExportPng
        | AppCmd::ExportMnc
        | AppCmd::SaveDuplicate => {
            // Unreachable in practice: `main::pump_commands` turns these into
            // their `*Path` forms. Reaching here means a path was not chosen.
        }
        AppCmd::OpenOraPath(p) => {
            app.commit_text_edit();
            let kind = if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mnc")) {
                mn_core::project::sniff_kind(&p)
            } else {
                mn_core::project::MncKind::Unknown
            };
            match kind {
                // The native multi-page format: a tiny index + side-by-side
                // page files in the same folder.
                mn_core::project::MncKind::WorkFolderIndex => {
                    match mn_core::project::load_folder(&p) {
                        Ok(wf) => match mn_core::project::bytes_to_doc(&wf.pages[0].bytes) {
                            Ok(doc) => {
                                let mn_core::project::WorkFolder {
                                    story,
                                    binding_right,
                                    setup,
                                    expression,
                                    spine_mm,
                                    cover,
                                    template_page,
                                    print_margin_info,
                                    print_crop_marks,
                                    profile,
                                    next_id,
                                    pages,
                                } = wf;
                                let n = pages.len();
                                let stored_uids: Vec<u64> =
                                    pages.iter().map(|fp| fp.uid).collect();
                                // A load lands in a NEW TAB unless the current
                                // document is an untouched blank (session.rs).
                                app.prepare_open_target();
                                app.doc = doc;
                                app.page = setup.filter(|s| s.has_guides());
                                app.story = story;
                                app.binding_right = binding_right;
                                app.expression = expression;
                                app.spine_mm = spine_mm;
                                app.cover = cover;
                                app.template_page = template_page;
                                app.print_margin_info = print_margin_info;
                                app.print_crop_marks = print_crop_marks;
                                app.profile = profile;
                                app.pages = pages
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, fp)| PageEntry {
                                        bytes: (i != 0).then_some(fp.bytes),
                                        thumb: None,
                                        // Overwritten by `adopt_page_uids`
                                        // below with the identity the work
                                        // was saved with (workflow audit
                                        // §11), where it recorded one.
                                        uid: PageEntry::next_uid(),
                                        id: fp.id,
                                        rev: fp.rev,
                                        saved_rev: fp.saved_rev,
                                        // The temp-autosave watermark is
                                        // per-TEMP-folder; a fresh open has
                                        // written nothing there.
                                        autosaved_rev: 0,
                                        exported_rev: fp.exported_rev,
                                        doc_rev: if i == 0 { app.doc.revision } else { 0 },
                                        blank: None,
                                        spread: false,
                                        preview_img: None,
                                        prev_tex: None,
                                        prev_tex_px: 0.0,
                                        prev_tex_rev: 0,
                                        canvas: None,
                                        pane_tex: None,
                                        pane_tex_px: 0.0,
                                        pane_tex_rev: 0,
                                        parked: None,
                                        parked_rev: 0,
                                    })
                                    .collect();
                                app.adopt_page_uids(&stored_uids);
                                app.page_index = 0;
                                let managed = app.page_file_names();
                                app.adopt_folder_state(next_id, managed);
                                app.renderer.invalidate();
                                app.layer_thumbs.clear();
                                app.fit_to_view();
                                app.set_doc_path(Some(p.clone()));
                                app.mark_saved();
                                app.note_recent(&p);
                                app.set_status(format!(
                                    "opened work folder {} ({n} pages)",
                                    p.display()
                                ));
                            }
                            Err(e) => app.set_error(format!("page 1 decode failed: {e}")),
                        },
                        Err(e) => app.set_error(format!("open failed: {e}")),
                    }
                }
                mn_core::project::MncKind::Comic => {
                    app.reset_folder_state();
                    match mn_core::project::load(&p) {
                        Ok(proj) => match mn_core::project::bytes_to_doc(&proj.pages[0]) {
                            Ok(doc) => {
                                app.prepare_open_target();
                                app.doc = doc;
                                // A fresh document cannot honour an armed
                                // mask-edit flag (audit H1).
                                app.disarm_mask_edit_if_unmasked();
                                app.page = proj.meta.setup.filter(|s| s.has_guides());
                                app.story = proj.meta.story;
                                app.binding_right = proj.meta.binding_right;
                                app.expression = proj.meta.expression;
                                app.spine_mm = proj.meta.spine_mm;
                                app.cover = proj.meta.cover;
                                app.template_page = proj.meta.template_page;
                                app.print_margin_info = proj.meta.print_margin_info;
                                app.print_crop_marks = proj.meta.print_crop_marks;
                                app.profile = proj.meta.profile.clone();
                                app.pages = proj
                                    .pages
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, b)| PageEntry {
                                        bytes: (i != 0).then_some(b),
                                        ..PageEntry::active()
                                    })
                                    .collect();
                                app.adopt_page_uids(&proj.meta.page_uids);
                                app.page_index = 0;
                                app.pages[0].doc_rev = app.doc.revision;
                                app.renderer.invalidate();
                                app.layer_thumbs.clear();
                                app.fit_to_view();
                                app.set_doc_path(Some(p.clone()));
                                app.mark_saved();
                                app.note_recent(&p);
                                app.set_status(format!(
                                    "opened {} ({} pages)",
                                    p.display(),
                                    app.pages.len()
                                ));
                            }
                            Err(e) => app.set_error(format!("page 1 decode failed: {e}")),
                        },
                        Err(e) => app.set_error(format!("open failed: {e}")),
                    }
                }
                mn_core::project::MncKind::Unknown => {
                    app.reset_folder_state();
                    match mn_core::ora::load(&p) {
                        Ok(doc) => {
                            app.prepare_open_target();
                            app.doc = doc;
                            app.disarm_mask_edit_if_unmasked();
                            // A bare ORA is a single page with no page-setup
                            // metadata, so guides are off rather than wrong.
                            app.page = None;
                            app.pages = vec![PageEntry::active()];
                            app.page_index = 0;
                            // Every layer index and tile in the cache belongs to
                            // the old document — exactly what invalidate() is for.
                            app.renderer.invalidate();
                            app.layer_thumbs.clear();
                            app.fit_to_view();
                            app.set_doc_path(Some(p.clone()));
                            app.mark_saved();
                            app.note_recent(&p);
                            app.set_status(format!(
                                "opened {} ({} layers)",
                                p.display(),
                                app.doc.layers.len()
                            ));
                        }
                        Err(e) => app.set_error(format!("open failed: {e}")),
                    }
                }
            }
        }
        AppCmd::SaveOraPath(p) => {
            // `work.mnc` = the work-folder flow (native). Anything else keeps
            // the legacy single-file / bare-ORA behaviour.
            let is_work_index = p
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("work.mnc"));
            if is_work_index {
                match app.save_work_folder(&p) {
                    Ok(msg) => {
                        app.set_doc_path(Some(p.clone()));
                        app.mark_saved();
                        app.note_recent(&p);
                        app.set_status(msg);
                    }
                    Err(e) => app.set_error(format!("save failed: {e}")),
                }
            } else {
                let is_mnc = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mnc"));
                if is_mnc {
                    match app.stash_current_page() {
                        Err(e) => app.set_error(e),
                        Ok(()) => {
                            let mut proj = mn_core::Project::new(
                                app.story.clone(),
                                app.page.clone(),
                                app.binding_right,
                            );
                            proj.meta.expression = app.expression;
                            proj.meta.spine_mm = app.spine_mm;
                            proj.meta.cover = app.cover;
                            proj.meta.template_page = app.template_page;
                            proj.meta.print_margin_info = app.print_margin_info;
                            proj.meta.print_crop_marks = app.print_crop_marks;
                            proj.meta.profile = app.profile.clone();
                            // Workflow audit §11: the pages' stable
                            // identities ride the file, so a work promoted
                            // from this one can still be stamped back onto
                            // it after both have been closed.
                            proj.meta.page_uids = app.page_uids();
                            proj.pages = app
                                .pages
                                .iter()
                                .map(|e| e.bytes.clone().unwrap_or_default())
                                .collect();
                            // The active page keeps living in `doc`, not in bytes.
                            app.pages[app.page_index].bytes = None;
                            match mn_core::project::save(&proj, &p) {
                                Ok(()) => {
                                    app.set_doc_path(Some(p.clone()));
                                    app.mark_saved();
                                    app.note_recent(&p);
                                    app.set_status(format!(
                                        "saved {} ({} pages)",
                                        p.display(),
                                        proj.pages.len()
                                    ));
                                }
                                Err(e) => app.set_error(format!("save failed: {e}")),
                            }
                        }
                    }
                } else {
                    match mn_core::ora::save(&app.doc, &p) {
                        Ok(()) => {
                            app.set_doc_path(Some(p.clone()));
                            app.mark_saved();
                            app.note_recent(&p);
                            if app.is_comic() {
                                app.set_status(format!(
                                    "saved CURRENT PAGE ONLY to {} — use .mnc for the whole comic",
                                    p.display()
                                ));
                            } else {
                                app.set_status(format!("saved {}", p.display()));
                            }
                        }
                        Err(e) => app.set_error(format!("save failed: {e}")),
                    }
                }
            }
            // A successful save makes any autosave shadowing this path stale
            // (PR-040). Leaving it behind means a crash months later offers
            // work the user already replaced — the one way a recovery prompt
            // can do harm.
            if app.doc_path.as_deref() == Some(p.as_path()) && !app.dirty() {
                crate::recovery::clear_sibling_autosave(&p);
                // This document has a real file now, so its never-saved
                // stash is superseded. Left behind, it would be offered
                // after some unrelated crash months later, described as
                // "newer than the file it belongs to" — which it is not.
                crate::recovery::clear_unsaved_stash(app.active_doc);
            }
        }
        AppCmd::ExportMncPath(p) => {
            // The portable single-file copy: never re-points the work at the
            // file, never marks the work clean.
            match app.stash_current_page() {
                Err(e) => app.set_error(e),
                Ok(()) => {
                    let mut proj = mn_core::Project::new(
                        app.story.clone(),
                        app.page.clone(),
                        app.binding_right,
                    );
                    proj.meta.expression = app.expression;
                    proj.meta.spine_mm = app.spine_mm;
                    proj.meta.cover = app.cover;
                    proj.meta.template_page = app.template_page;
                    proj.meta.print_margin_info = app.print_margin_info;
                    proj.meta.print_crop_marks = app.print_crop_marks;
                    proj.meta.profile = app.profile.clone();
                    proj.meta.page_uids = app.page_uids();
                    proj.pages = app
                        .pages
                        .iter()
                        .map(|e| e.bytes.clone().unwrap_or_default())
                        .collect();
                    // The active page keeps living in `doc`, not in bytes.
                    app.pages[app.page_index].bytes = None;
                    match mn_core::project::save(&proj, &p) {
                        Ok(()) => app.set_status(format!(
                            "exported single file {} ({} pages)",
                            p.display(),
                            proj.pages.len()
                        )),
                        Err(e) => app.set_error(format!("export failed: {e}")),
                    }
                }
            }
        }
        AppCmd::SaveDuplicatePath(p) => {
            // IO-003. Note what is NOT here, next to `SaveOraPath`: no
            // `set_doc_path`, no `mark_saved`, no `note_recent`, and no
            // autosave clearing. You are still in the original, still
            // dirty if you were, still crash-netted.
            let dirty = app.dirty();
            let path = app.doc_path.clone();
            match app.save_duplicate(&p) {
                Ok(msg) => app.set_status(msg),
                Err(e) => app.set_error(e),
            }
            debug_assert_eq!(app.doc_path, path, "a duplicate never moves the work");
            debug_assert_eq!(app.dirty(), dirty, "…and never marks it saved");
        }
        AppCmd::ExportPsd => {
            // Resolved to ExportPsdPath by `main::pump_commands`.
        }
        AppCmd::ExportPsdPath(p) => {
            app.refresh_tones();
            let file = match std::fs::File::create(&p) {
                Ok(f) => f,
                Err(e) => return app.set_error(format!("psd export failed: {e}")),
            };
            match mn_core::psd::save_psd(&app.doc, std::io::BufWriter::new(file)) {
                Ok(()) => app.set_status(format!(
                    "exported layered PSD ({} layers) -> {}",
                    app.doc.layers.len(),
                    p.display()
                )),
                Err(e) => app.set_error(format!("psd export failed: {e}")),
            }
        }
        AppCmd::ExportPngPath(p) => {
            app.refresh_tones();
            let (w, h) = app.doc.size;
            // PA-001: export on the paper COLOUR whatever the paper's eye
            // says. Hiding the paper is a hole-check, not an export mode —
            // and the transparency checker is screen furniture that must
            // never land in a PNG someone publishes.
            app.renderer.set_paper_override(Some(mn_core::Paper {
                visible: true,
                ..app.doc.paper
            }));
            // Export rules, not screen rules: the 下書き stays behind. Same
            // renderer the folder export, the previews, the reader and
            // print all use — without it the artist's rough shipped inside
            // the PNG, with the margin stamp printed on top of it.
            let img =
                crate::app::pages::render_offscreen_drafts_off(&mut app.renderer, &mut app.doc, w, h);
            app.renderer.set_paper_override(None);
            // The margin stamp rides this door too: the active page, full
            // paper, work pixels, no colour reduction — this door still has
            // no dpi, no colour reduction and no crop.
            let mut img = img;
            if app.print_margin_info {
                crate::app::export_stamp::stamp_margin_info(
                    app.text_engine.as_ref(),
                    &app.text_font,
                    &app.story,
                    &mut img,
                    app.page.as_ref(),
                    [0, 0, w, h],
                    1.0,
                    0,
                    mn_core::doc::LayerExpression::Colour,
                    &(app.page_index + 1).to_string(),
                );
            }
            // トンボ ride this door too — full paper at scale 1 is exactly
            // the geometry the marks want.
            if app.print_crop_marks {
                let marks =
                    mn_core::export::crop_marks(app.page.as_ref(), [0, 0, w, h], 1.0, (w, h));
                mn_core::export::apply_crop_marks(&mut img, &marks);
            }
            match img.save(&p) {
                Ok(()) => {
                    app.set_status(format!("exported {w}x{h} PNG -> {}", p.display()));
                    // This page's image just landed on disk, so the export
                    // reminder stops counting it (and starts counting the
                    // work at all, if this was its first export). After the
                    // status line: a stash that fails there has something
                    // to say and should not be talked over.
                    app.note_page_exported(app.page_index);
                }
                Err(e) => app.set_error(format!("png export failed: {e}")),
            }
        }


        AppCmd::Autosave => {
            // Background tabs first: they have no other way to be written,
            // and the tick used to ignore them entirely (a crash then took
            // their work with no recovery file to offer). Encoded from their
            // parked state, so this never disturbs the live document.
            let parked = app.autosave_parked();

            // Skip while clean or mid-stroke; never touches doc_path or the
            // dirty state (an autosave is not the user's save).
            let _ = parked;
            if app.dirty() && !app.drawing() {
                // Work-folder-backed works autosave IN PLACE, incrementally:
                // each changed page lands atomically (tmp+rename), the index
                // commits last. Nothing is rewritten for untouched pages —
                // that is the point of the folder format.
                let folder_index = app
                    .doc_path
                    .as_ref()
                    .filter(|p| {
                        p.file_name()
                            .is_some_and(|n| n.eq_ignore_ascii_case("work.mnc"))
                    })
                    .cloned();
                if let Some(p) = folder_index {
                    match app.save_work_folder(&p) {
                        Ok(msg) => app.set_status(format!("autosave: {msg}")),
                        Err(e) => app.set_error(format!("autosave failed: {e}")),
                    }
                } else if let Some(doc) = app.doc_path.clone() {
                    // A saved single-file work keeps its shadow BESIDE ITSELF
                    // — recovery ranks that shadow against the file it
                    // shadows, which a `%TEMP%` copy could not do.
                    if let Err(e) = app.stash_current_page() {
                        app.set_error(format!("autosave stash failed: {e}"));
                    } else {
                        let mut proj = mn_core::Project::new(
                            app.story.clone(),
                            app.page.clone(),
                            app.binding_right,
                        );
                        proj.meta.expression = app.expression;
                        proj.meta.spine_mm = app.spine_mm;
                        proj.meta.cover = app.cover;
                        proj.meta.template_page = app.template_page;
                        proj.meta.print_margin_info = app.print_margin_info;
                        proj.meta.print_crop_marks = app.print_crop_marks;
                        proj.meta.profile = app.profile.clone();
                        proj.meta.page_uids = app.page_uids();
                        proj.pages = app
                            .pages
                            .iter()
                            .map(|e| e.bytes.clone().unwrap_or_default())
                            .collect();
                        app.pages[app.page_index].bytes = None;
                        // Both spellings come from `recovery`, which is also
                        // what READS them back after a crash (PR-040) — a
                        // literal here and a literal there is how a recovery
                        // feature ends up hunting for a file nothing writes.
                        let path = crate::recovery::sibling_autosave(&doc);
                        match mn_core::project::save(&proj, &path) {
                            Ok(()) => app.set_status(format!("autosaved -> {}", path.display())),
                            Err(e) => app.set_error(format!("autosave failed: {e}")),
                        }
                    }
                } else {
                    // 05 item 1: a pathless work autosaves into an
                    // incremental TEMP work folder — only dirty pages
                    // re-encode, instead of one monolithic zip built from
                    // every page on the UI thread every tick.
                    let index = crate::app::unsaved_autosave_folder_for(app.active_doc);
                    match app.autosave_work_folder(&index) {
                        Ok(msg) => app.set_status(format!("autosave: {msg}")),
                        Err(e) => app.set_error(format!("autosave failed: {e}")),
                    }
                }
            }
        }
        other => return layers::run(app, other, cmd_tail),
    }
    run_cmd_tail(app, cmd_tail);
}
