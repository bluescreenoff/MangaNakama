//! `AppCmd` arms: print, and every path-taking open/save/export
//! (`.ora`, `.mnc`, `.psd`, `.png`, text dumps) plus autosave.
//!
//! **Item K (2026-09-05): every save here is now two halves.** The ENCODE
//! (document → bytes) stays on this thread, because it reads the live layers.
//! The WRITE (bytes → disk) goes to `cmd::save_bg`'s one background thread,
//! because it does not — and because it was the half that held the Windows
//! message pump long enough for the OS to paint "not responding" over the
//! window. See `save_bg.rs` for the queue, the pill and the poll.
//!
//! The bookkeeping (`mark_saved`, `set_doc_path`, per-page `saved_rev`) runs
//! HERE, optimistically, the moment the bytes are handed over.
//! `save_bg::poll_saves` takes it back if the write fails.

use super::*;

use crate::cmd::save_bg;

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
                // The encode + the bookkeeping run here; only the disk write
                // crosses to the writer thread (item K). `folder_page_ids`
                // answers the one question the write used to answer — which
                // id each page got — so the bookkeeping does not have to wait
                // for it.
                let label = p.display().to_string();
                let queued = app.save_work_folder_via(&p, |wf, encodes, dir, managed| {
                    let ids = save_bg::folder_page_ids(&wf);
                    save_bg::submit(
                        label,
                        true,
                        save_bg::Write::Folder {
                            wf: Box::new(wf),
                            encodes,
                            dir: dir.to_path_buf(),
                            managed: managed.to_vec(),
                            verb: "saved work folder",
                        },
                    );
                    Ok((ids, 0))
                });
                match queued {
                    Ok(_) => {
                        app.set_doc_path(Some(p.clone()));
                        app.mark_saved();
                        app.note_recent(&p);
                        // The real "saved …" line arrives from the writer
                        // thread through `save_bg::poll_saves`.
                        app.set_status(format!("saving {}…", p.display()));
                    }
                    Err(e) => app.set_error(format!("save failed: {e}")),
                }
            } else {
                let is_mnc = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mnc"));
                if is_mnc {
                    match app.project_pages_for_save() {
                        Err(e) => app.set_error(e),
                        Ok((page_bytes, encodes)) => {
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
                            proj.pages = page_bytes;
                            let n = proj.pages.len();
                            save_bg::submit(
                                p.display().to_string(),
                                true,
                                save_bg::Write::Project {
                                    path: p.clone(),
                                    proj: Box::new(proj),
                                    encodes,
                                },
                            );
                            app.set_doc_path(Some(p.clone()));
                            app.mark_saved();
                            app.note_recent(&p);
                            app.set_status(format!("saving {} ({n} pages)…", p.display()));
                        }
                    }
                } else {
                    // S03: a bare `.ora` has no page setup to reopen with,
                    // so the dpi rides the image element instead.
                    app.stamp_doc_dpi();
                    // A `Document::clone` is pointer copies (tiles are `Arc`),
                    // so the whole encode goes with it to the writer thread.
                    save_bg::submit(
                        p.display().to_string(),
                        true,
                        save_bg::Write::Ora {
                            path: p.clone(),
                            page: save_bg::PageEncode {
                                doc: Box::new(app.doc.clone()),
                                preview_png: None,
                            },
                        },
                    );
                    app.set_doc_path(Some(p.clone()));
                    app.mark_saved();
                    app.note_recent(&p);
                    if app.is_comic() {
                        app.set_status(format!(
                            "saving CURRENT PAGE ONLY to {} — use .mnc for the whole comic",
                            p.display()
                        ));
                    } else {
                        app.set_status(format!("saving {}…", p.display()));
                    }
                }
            }
            // A successful save makes any autosave shadowing this path stale
            // (PR-040). Leaving it behind means a crash months later offers
            // work the user already replaced — the one way a recovery prompt
            // can do harm.
            // Item K note: this now runs while the write is still in flight.
            // That is safe — if the write fails, `save_bg::poll_saves` marks
            // the work dirty again and the next autosave tick re-creates the
            // shadow this line just removed.
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
            match app.project_pages_for_save() {
                Err(e) => app.set_error(e),
                Ok((page_bytes, encodes)) => {
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
                    proj.pages = page_bytes;
                    let n = proj.pages.len();
                    save_bg::submit(
                        p.display().to_string(),
                        false,
                        save_bg::Write::Project {
                            path: p.clone(),
                            proj: Box::new(proj),
                            encodes,
                        },
                    );
                    app.set_status(format!(
                        "exporting single file {} ({n} pages)…",
                        p.display()
                    ));
                }
            }
        }
        AppCmd::SaveDuplicatePath(p) => {
            // IO-003. Note what is NOT here, next to `SaveOraPath`: no
            // `set_doc_path`, no `mark_saved`, no `note_recent`, and no
            // autosave clearing. You are still in the original, still
            // dirty if you were, still crash-netted.
            //
            // The copy is encoded and written on the writer thread like
            // every other save (2026-09-06); `save_duplicate` returns the
            // "writing…" line and `save_bg::poll_saves` says when it landed.
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
            // Snapshot AFTER the tone refresh, so the PSD carries the derived
            // halftones the screen is showing.
            save_bg::submit(
                p.display().to_string(),
                false,
                save_bg::Write::Psd {
                    path: p.clone(),
                    doc: Box::new(app.doc.clone()),
                },
            );
            app.set_status(format!(
                "exporting layered PSD ({} layers) -> {}",
                app.doc.layers.len(),
                p.display()
            ));
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
            // PNG deflate happens here (it is CPU, and the image is already
            // in memory); only the file write crosses to the writer thread.
            let mut buf = std::io::Cursor::new(Vec::new());
            match img.write_to(&mut buf, image::ImageFormat::Png) {
                Ok(()) => {
                    save_bg::submit(
                        p.display().to_string(),
                        false,
                        save_bg::Write::File {
                            path: p.clone(),
                            bytes: buf.into_inner(),
                        },
                    );
                    app.set_status(format!("exporting {w}x{h} PNG -> {}", p.display()));
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
            //
            // Item K adds a third skip: while the writer thread still has a
            // write in flight. An autosave is a tick, not a request — piling
            // ticks up behind a slow disk would make every later save wait for
            // work nobody asked for. The next tick catches what this one skips.
            let _ = parked;
            if app.dirty() && !app.drawing() && save_bg::queued() == 0 {
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
                    let label = p.display().to_string();
                    let queued = app.save_work_folder_via(&p, |wf, encodes, dir, managed| {
                        let ids = save_bg::folder_page_ids(&wf);
                        save_bg::submit(
                            label,
                            false,
                            save_bg::Write::Folder {
                                wf: Box::new(wf),
                                encodes,
                                dir: dir.to_path_buf(),
                                managed: managed.to_vec(),
                                verb: "autosaved work folder",
                            },
                        );
                        Ok((ids, 0))
                    });
                    match queued {
                        Ok(_) => app.set_status(format!("autosaving {}…", p.display())),
                        Err(e) => app.set_error(format!("autosave failed: {e}")),
                    }
                } else if let Some(doc) = app.doc_path.clone() {
                    // A saved single-file work keeps its shadow BESIDE ITSELF
                    // — recovery ranks that shadow against the file it
                    // shadows, which a `%TEMP%` copy could not do.
                    match app.project_pages_for_save() {
                        Err(e) => app.set_error(format!("autosave stash failed: {e}")),
                        Ok((page_bytes, encodes)) => {
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
                            proj.pages = page_bytes;
                            // Both spellings come from `recovery`, which is also
                            // what READS them back after a crash (PR-040) — a
                            // literal here and a literal there is how a recovery
                            // feature ends up hunting for a file nothing writes.
                            let path = crate::recovery::sibling_autosave(&doc);
                            save_bg::submit(
                                path.display().to_string(),
                                false,
                                save_bg::Write::Project {
                                    path: path.clone(),
                                    proj: Box::new(proj),
                                    encodes,
                                },
                            );
                            app.set_status(format!("autosaving -> {}", path.display()));
                        }
                    }
                } else {
                    // 05 item 1: a pathless work autosaves into an
                    // incremental TEMP work folder — only dirty pages
                    // re-encode, instead of one monolithic zip built from
                    // every page on the UI thread every tick.
                    let index = crate::app::unsaved_autosave_folder_for(app.active_doc);
                    let label = index.display().to_string();
                    let queued = app.autosave_work_folder_via(&index, |wf, encodes, dir| {
                        let ids = save_bg::folder_page_ids(&wf);
                        save_bg::submit(
                            label,
                            false,
                            save_bg::Write::Folder {
                                wf: Box::new(wf),
                                encodes,
                                dir: dir.to_path_buf(),
                                managed: Vec::new(),
                                verb: "autosaved work folder",
                            },
                        );
                        Ok((ids, 0))
                    });
                    match queued {
                        Ok(_) => app.set_status(format!("autosaving -> {}", index.display())),
                        Err(e) => app.set_error(format!("autosave failed: {e}")),
                    }
                }
            }
        }
        other => return layers::run(app, other, cmd_tail),
    }
    run_cmd_tail(app, cmd_tail);
}

/// The blocking-command timing table (item K's addendum, 2026-09-05). The
/// owner reported Windows painting "not responding" over the window during
/// saves, which is what happens when the message pump does not run for ~5 s.
/// This measures every path in this file that writes or reads a whole work,
/// on a three-page work, and prints one `[time]` line each. It asserts
/// nothing about wall clock — a laptop under load has no stable budget —
/// it exists so the numbers in the lane report can be re-measured.
#[cfg(test)]
mod blocking_time_tests {
    use super::*;
    use std::time::Instant;

    fn ms(t: Instant) -> f64 {
        t.elapsed().as_secs_f64() * 1000.0
    }

    /// A three-page work with real ink on every page, at a dpi that keeps
    /// this test honest without asking a 15.8 GB laptop for a 600 dpi B4.
    fn three_page_work(dpi: u32) -> Option<App> {
        let mut app = App::new(crate::app::headless_renderer()?, (1280, 860), 1.0);
        app.new_doc_draft.setup.dpi = dpi;
        app.new_doc_draft.pages = 3;
        app.new_doc_draft.story = "Timing".into();
        dispatch(&mut app, AppCmd::NewComicCreate);
        for p in 0..3 {
            dispatch(&mut app, AppCmd::SelectPage(p));
            let (w, h) = app.doc.size;
            let li = app.doc.active;
            for k in 0..400u32 {
                let x = (w as f32 * 0.15 + k as f32 * 3.0) as i32;
                let y = (h as f32 * 0.5 + (k as f32 * 0.11).sin() * 60.0) as i32;
                for dy in -6..6 {
                    for dx in -6..6 {
                        let (px, py) = (x + dx, y + dy);
                        if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                            let t = app.doc.layers[li]
                                .tile_mut(mn_core::tile::TileIdx::new(px / 64, py / 64));
                            t.set_pixel((px % 64) as usize, (py % 64) as usize, [0, 0, 0, 32767]);
                        }
                    }
                }
            }
            // Pixels written straight into the tiles do not advance the
            // document revision, and the page stash skips a page whose
            // revision has not moved — so without this the later `.mnc`
            // doors would encode three EMPTY pages and time nothing.
            app.doc.touch();
            app.mark_pages_dirty();
        }
        dispatch(&mut app, AppCmd::SelectPage(0));
        Some(app)
    }

    #[test]
    fn every_blocking_file_command_is_timed() {
        // 200 dpi B4 ≈ 1930×2730 px per page. The owner draws at 600 dpi,
        // which is 9× the pixels — read every number below as "×9 for his
        // page" (PNG deflate is close to linear in pixel count).
        let Some(mut app) = three_page_work(200) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("mn-lane5-time-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        println!(
            "[time] page {:?} px, {} pages, {} layers",
            app.doc.size,
            app.pages.len(),
            app.doc.layers.len()
        );

        let cases: Vec<(&str, AppCmd)> = vec![
            ("SaveOraPath .ora (one page)", AppCmd::SaveOraPath(dir.join("t.ora"))),
            ("SaveOraPath .mnc (single file)", AppCmd::SaveOraPath(dir.join("t.mnc"))),
            (
                "SaveOraPath work.mnc (work folder, first save)",
                AppCmd::SaveOraPath(dir.join("wf").join("work.mnc")),
            ),
            (
                "SaveOraPath work.mnc (work folder, re-save, nothing dirty)",
                AppCmd::SaveOraPath(dir.join("wf").join("work.mnc")),
            ),
            ("ExportMncPath", AppCmd::ExportMncPath(dir.join("e.mnc"))),
            ("ExportPngPath", AppCmd::ExportPngPath(dir.join("e.png"))),
            ("ExportPsdPath", AppCmd::ExportPsdPath(dir.join("e.psd"))),
            ("SaveDuplicatePath", AppCmd::SaveDuplicatePath(dir.join("dup.mnc"))),
            ("ExportTextPath", AppCmd::ExportTextPath(dir.join("script.txt"))),
            ("Autosave", AppCmd::Autosave),
            ("OpenOraPath (.mnc, 3 pages)", AppCmd::OpenOraPath(dir.join("t.mnc"))),
        ];
        // Measure the two halves apart. `submit` waits for the writer when no
        // UI frame loop is running, which every unit test relies on — so this
        // one test turns the background on (under the shared lock) to see what
        // the message pump would really have paid.
        let _serial = save_bg::test_lock();
        save_bg::pretend_frames_are_running(true);
        for (name, cmd) in cases {
            let t = Instant::now();
            dispatch(&mut app, cmd);
            let blocked = ms(t);
            let t = Instant::now();
            save_bg::flush();
            let background = ms(t);
            crate::cmd::save_bg::poll_saves(&mut app);
            println!(
                "[time] {name}: BLOCKS THE PUMP {blocked:.0} ms, background {background:.0} ms — {}",
                app.status
            );
        }
        save_bg::pretend_frames_are_running(false);

        // The SPLIT the fix rests on: of that total, how much is the encode
        // (stays on the UI thread — it reads the live layers) and how much is
        // the disk write (now on the writer thread)? Measured on the same
        // work, through the same core calls the arms above use.
        let t = Instant::now();
        let one_page = mn_core::project::doc_to_bytes(&app.doc).expect("encode the page");
        let enc = ms(t);
        let t = Instant::now();
        std::fs::write(dir.join("split.bin"), &one_page).expect("write the bytes");
        println!(
            "[time] SPLIT one page ({} KB): encode {enc:.0} ms + write {:.0} ms — BOTH now on the writer thread",
            one_page.len() / 1024,
            ms(t)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }


    /// Item K round 2 (c), rule one: a save must not rewrite what it already
    /// wrote. An untouched work re-saves without re-encoding or re-writing a
    /// single page file — pinned by hashing the files, which no other test
    /// running in parallel can disturb.
    #[test]
    fn a_resave_of_an_unchanged_work_rewrites_no_page_files() {
        let Some(mut app) = three_page_work(72) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("mn-lane5-skip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let index = dir.join("wf").join("work.mnc");
        std::fs::create_dir_all(index.parent().unwrap()).expect("the folder");
        let folder = index.parent().unwrap().to_path_buf();

        dispatch(&mut app, AppCmd::SaveOraPath(index.clone()));
        println!("[skip] first save: {}", app.status);
        let snapshot = |dir: &std::path::Path| -> Vec<(String, u64, Vec<u8>)> {
            let mut v: Vec<(String, u64, Vec<u8>)> = std::fs::read_dir(dir)
                .expect("read the folder")
                .flatten()
                .map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let bytes = std::fs::read(e.path()).unwrap_or_default();
                    let mtime = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0);
                    (name, mtime, bytes)
                })
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v
        };
        let before = snapshot(&folder);
        assert!(
            before.iter().any(|(n, _, _)| n.ends_with(".ora")),
            "the first save wrote page files: {:?}",
            before.iter().map(|(n, _, _)| n).collect::<Vec<_>>()
        );

        dispatch(&mut app, AppCmd::SaveOraPath(index.clone()));
        println!("[skip] clean re-save: {}", app.status);
        let after = snapshot(&folder);
        let pages_before: Vec<_> = before.iter().filter(|(n, _, _)| n.ends_with(".ora")).collect();
        let pages_after: Vec<_> = after.iter().filter(|(n, _, _)| n.ends_with(".ora")).collect();
        assert_eq!(pages_before, pages_after, "no page file was touched again");
        assert!(
            app.status.contains("0 rewritten"),
            "the save says so too: {}",
            app.status
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Rule two: when one layer IS painted, only that layer is re-encoded.
    /// The counter is per thread, so this runs the encode here rather than on
    /// the writer thread — the cache it is testing is the same one either way.
    #[test]
    fn only_the_painted_layer_is_re_encoded() {
        let Some(mut app) = three_page_work(72) else {
            return;
        };
        let n = mn_core::ora::layer_png_encodes;
        // Warm: building the work already encoded these pages once, on this
        // thread, which is itself the first half of the claim.
        let mark = n();
        mn_core::project::doc_to_bytes(&app.doc).expect("first encode");
        println!("[cache] encode of an already-encoded page: {} layer PNGs", n() - mark);

        let mark = n();
        mn_core::project::doc_to_bytes(&app.doc).expect("second encode");
        assert_eq!(n() - mark, 0, "an untouched document re-encodes nothing");

        // Paint into ONE layer.
        let li = app.doc.active;
        let t = app.doc.layers[li].tile_mut(mn_core::tile::TileIdx::new(1, 1));
        for k in 0..64 {
            t.set_pixel(k, k, [0, 0, 0, 32767]);
        }
        let mark = n();
        mn_core::project::doc_to_bytes(&app.doc).expect("third encode");
        let dirty = n() - mark;
        println!("[cache] after painting one layer: {dirty} layer PNGs");
        assert_eq!(dirty, 1, "only the painted layer misses the cache");
    }

    /// Item K round 2: WHERE the .ora encode's seconds go. One `[stage]` line
    /// per layer, then the totals. This is the measurement the compression /
    /// caching / off-thread decisions rest on, so it prints rather than
    /// asserts — a laptop under load has no stable budget.
    #[test]
    fn the_ora_encode_cost_splits_by_stage() {
        use image::ImageEncoder;
        use image::codecs::png::{CompressionType, FilterType, PngEncoder};
        let Some(app) = three_page_work(200) else {
            return;
        };
        let (mut t_pix, mut t_cur, mut t_nofilt, mut t_raw) = (0.0, 0.0, 0.0, 0.0);
        let (mut n_cur, mut n_nofilt, mut n_raw) = (0usize, 0usize, 0usize);
        for (i, layer) in app.doc.layers.iter().enumerate() {
            let t = Instant::now();
            let Some((img, _, _)) = mn_core::export::layer_image(layer) else {
                println!("[stage] layer {i} {:?}: empty", layer.name);
                continue;
            };
            let pix = ms(t);
            let (w, h) = (img.width(), img.height());

            // 1. What ships today: image's PNG writer (Fast + Adaptive filter).
            let t = Instant::now();
            let mut cur = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut cur), image::ImageFormat::Png)
                .expect("png");
            let cur_ms = ms(t);

            // 2. Same compression, no per-row filter search.
            let t = Instant::now();
            let mut nf = Vec::new();
            PngEncoder::new_with_quality(&mut nf, CompressionType::Fast, FilterType::NoFilter)
                .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
                .expect("png");
            let nf_ms = ms(t);

            // 3. No compression at all (the floor: pure pixel plumbing).
            let t = Instant::now();
            let mut raw = Vec::new();
            PngEncoder::new_with_quality(
                &mut raw,
                CompressionType::Uncompressed,
                FilterType::NoFilter,
            )
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
            .expect("png");
            let raw_ms = ms(t);

            println!(
                "[stage] layer {i} {:?} {w}x{h}: pixels {pix:.0} ms | png now {cur_ms:.0} ms ({} KB) | png nofilter {nf_ms:.0} ms ({} KB) | png raw {raw_ms:.0} ms ({} KB)",
                layer.name,
                cur.len() / 1024,
                nf.len() / 1024,
                raw.len() / 1024
            );
            t_pix += pix;
            t_cur += cur_ms;
            t_nofilt += nf_ms;
            t_raw += raw_ms;
            n_cur += cur.len();
            n_nofilt += nf.len();
            n_raw += raw.len();
        }
        println!(
            "[stage] TOTAL one page: pixels {t_pix:.0} ms + png {t_cur:.0} ms = {:.0} ms (the rest of the .ora save is the zip)",
            t_pix + t_cur
        );
        // The other half of the .ora: `mergedimage.png` (the OpenRaster
        // spec's flattened preview) and the thumbnail derived from it.
        let t = Instant::now();
        let merged = mn_core::export::composite(&app.doc, mn_core::export::Background::Transparent);
        let comp_ms = ms(t);
        let t = Instant::now();
        let mut mp = Vec::new();
        merged
            .write_to(&mut std::io::Cursor::new(&mut mp), image::ImageFormat::Png)
            .expect("png");
        let mpng_ms = ms(t);
        let t = Instant::now();
        let thumb = image::imageops::resize(&merged, 256, 360, image::imageops::FilterType::Triangle);
        let th_ms = ms(t);
        let _ = thumb;
        println!(
            "[stage] mergedimage.png: composite {comp_ms:.0} ms + png {mpng_ms:.0} ms ({} KB) + thumbnail resize {th_ms:.0} ms = {:.0} ms",
            mp.len() / 1024,
            comp_ms + mpng_ms + th_ms
        );
        // And the whole door, for the arithmetic to close.
        let t = Instant::now();
        let all = mn_core::project::doc_to_bytes(&app.doc).expect("encode");
        println!(
            "[stage] WHOLE .ora encode: {:.0} ms ({} KB)",
            ms(t),
            all.len() / 1024
        );
        // Item K round 2 (d): can the whole encode move to the writer thread?
        // Only if a Document can be SNAPSHOT cheaply and sent. Tiles are
        // `Arc<Tile>`, so a clone is refcount bumps and hash maps, not pixels.
        fn assert_send<T: Send>() {}
        assert_send::<mn_core::Document>();
        let t = Instant::now();
        let snap = app.doc.clone();
        println!(
            "[stage] Document::clone (the snapshot the writer thread would get): {:.0} ms, {} layers",
            ms(t),
            snap.layers.len()
        );
        let t = Instant::now();
        let h = std::thread::spawn(move || mn_core::project::doc_to_bytes(&snap));
        let bytes = h.join().expect("join").expect("encode");
        println!(
            "[stage] the same encode ON A THREAD: {:.0} ms ({} KB)",
            ms(t),
            bytes.len() / 1024
        );
        println!(
            "[stage] alternatives: png nofilter {t_nofilt:.0} ms ({} KB vs {} KB), png uncompressed {t_raw:.0} ms ({} KB)",
            n_nofilt / 1024,
            n_cur / 1024,
            n_raw / 1024
        );
    }
}
