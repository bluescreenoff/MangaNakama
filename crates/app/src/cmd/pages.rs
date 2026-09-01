//! `AppCmd` arms: new documents, the page list (add/delete/move/
//! duplicate/import/spread), work settings, canvas + page size,
//! resampling, and the all-pages export.

use super::*;
use super::transform::selection_bbox;

/// One canvas resize through the whole app: end any stroke/edit, drop stale
/// view state (selection, transform float), run the core resize, and rebuild
/// every cache that is sized by the canvas.
fn apply_canvas_resize(app: &mut App, w: u32, h: u32, dx: i32, dy: i32) {
    app.end_stroke();
    app.commit_text_edit();
    app.transform_drag = None;
    app.last_selection = None;
    let old = app.doc.size;
    app.doc.resize_to(w, h, dx, dy);
    // Structural: the texture changes size and every cached thumb is stale.
    app.renderer.invalidate();
    app.layer_thumbs.clear();
    app.mark_pages_dirty();
    app.mark_dirty();
    app.set_status(format!(
        "canvas {w}×{h} (was {}×{}) — history cleared",
        old.0, old.1
    ));
}

/// PM-051: the batch export's default file prefix — the work name, or
/// `page` for an unnamed work. This is the string the pre-options export
/// used, and keeping it here is what makes an untouched run byte-for-byte
/// identical to the old one.
pub fn default_export_stem(app: &App) -> String {
    if app.story.trim().is_empty() {
        "page".to_owned()
    } else {
        app.story.trim().to_owned()
    }
}

/// PM-055: is this page a two-page spread? The runtime `spread` flag when
/// it is still there, else the structural test — a canvas half again as
/// wide as a normal page. The flag is a session flag on the page entry
/// and does NOT survive a reload, so the width test is what keeps a
/// reopened work splitting correctly.
pub(crate) fn is_spread_page(d: &mn_core::Document, flagged: bool, normal_w: Option<u32>) -> bool {
    flagged || normal_w.is_some_and(|w| w > 0 && d.size.0 as f32 >= w as f32 * 1.5)
}

pub(super) fn run(app: &mut App, cmd: AppCmd, cmd_tail: CmdTail) {
    match cmd {
        // --- documents ----------------------------------------------------
        AppCmd::NewDoc => {
            app.new_doc_open = true;
            app.mark_dirty();
        }
        AppCmd::NewPattern => app.pattern_new(),
        AppCmd::PatternSaveMaterial => match app.pattern_save_material() {
            Some((path, stem)) => {
                app.set_status(format!(
                    "pattern \"{stem}\" saved to the material bank ({})",
                    path.display()
                ));
            }
            None => {
                app.set_error("pattern save failed: the tile is empty or the folder is unwritable")
            }
        },
        AppCmd::NewComicCreate => {
            app.commit_text_edit();
            // Direct-feel rule: bake any open float before the doc it lives
            // in is parked for the new comic.
            app.commit_open_float();
            // A new project opens in a NEW TAB (owner, 2026-08-19: "it would
            // be bad if I have art in the default canvas and making a new
            // manga deletes it"). This one line parks the current document;
            // everything below then builds the new one into the live fields
            // exactly as it did when there was only ever one.
            app.push_doc_slot();
            let d = app.new_doc_draft.clone();
            // Remember the preset actually used (owner, 2026-08-23): the next
            // New Manga opens on it. Only a known preset name is written —
            // hand-tweaked paper values keep the name they started from, and
            // an unknown name would just fall back to the default on read.
            if mn_core::PageSetup::presets()
                .iter()
                .any(|p| p.name == d.setup.name)
                && app.prefs.new_preset != d.setup.name
            {
                app.prefs.new_preset = d.setup.name.clone();
                app.prefs.mark_dirty();
            }
            let (w, h) = d.setup.paper_px();
            app.page = d.setup.has_guides().then(|| d.setup.clone());
            app.seed_frame_folder = d.frame_folder;
            // Binding BEFORE any page is seeded: every seeded frame's book
            // side (`page_is_right`) keys on it, and the old order built
            // page 1 under the PREVIOUS work's binding.
            app.binding_right = d.binding_right;
            app.doc = app.blank_page_doc_sized(w, h);
            app.story = d.story;
            // Facing pages get facing frames: one blank per BOOK SIDE,
            // picked by page number. THE FREEZE FIX (owner 2026-08-26):
            // encoding even one B4 600 dpi blank is a ~40 s ORA walk
            // (debug) that blocked the UI after Create — new pages now
            // carry the LAZY BLANK marker instead (PageEntry::blank), and
            // nothing encodes until a save actually needs page bytes.
            app.pages = vec![PageEntry::active()];
            for n in 2..=(d.pages.max(1) as usize) {
                let mut e = app.fresh_page(None, None);
                e.blank = Some((w, h, n));
                app.pages.push(e);
            }
            app.page_index = 0;
            // Page 1 is the same untouched template: mark it lazily blank
            // TOO and pre-stamp its doc_rev, so the FIRST switch away from
            // a fresh comic costs nothing (the live doc compares
            // unchanged, the marker survives, nothing encodes until the
            // page is actually drawn or the work is saved).
            app.pages[0].blank = Some((w, h, 1));
            app.pages[0].doc_rev = app.doc.revision;
            app.new_doc_open = false;
            app.set_doc_path(None);
            app.reset_folder_state();
            app.renderer.invalidate();
            app.layer_thumbs.clear();
            app.fit_to_view();
            app.mark_saved();
            app.mark_dirty();
        }

        // --- pages ----------------------------------------------------------
        AppCmd::SelectPage(i) => app.switch_page(i),
        AppCmd::OpenPageInPane(i) => {
            crate::ui::dock::open_page_pane(app, i);
            app.set_status(format!(
                "page {} opened in a pane — click it to edit",
                i + 1
            ));
        }
        // PM-021/022: navigation with end-of-chapter guards — switching
        // itself goes through switch_page (stash + decode + fit).
        AppCmd::PageFirst => app.switch_page(0),
        AppCmd::PageLast => app.switch_page(app.pages.len().saturating_sub(1)),
        AppCmd::PagePrev => {
            if app.page_index == 0 {
                app.set_status("first page");
            } else {
                app.switch_page(app.page_index - 1);
            }
        }
        AppCmd::PageNext => {
            if app.page_index + 1 >= app.pages.len() {
                app.set_status("last page");
            } else {
                app.switch_page(app.page_index + 1);
            }
        }
        AppCmd::PageGoto => {
            app.goto_page_value = (app.page_index + 1) as i32;
            app.goto_page_open = true;
        }
        AppCmd::PageGotoApply(n) => {
            app.goto_page_open = false;
            let n = n.clamp(1, app.pages.len().max(1));
            app.switch_page(n - 1);
        }
        AppCmd::RegisterBrushFromSelection => {
            // Sets its own status lines (success names the brush, failure
            // says what was missing).
            app.register_brush_from_selection();
        }
        AppCmd::MaterialRegisterLayer => match app.material_register_layer() {
            Some((p, name)) => {
                app.set_status(format!(
                    "registered \"{name}\" → {}",
                    p.parent()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default()
                ));
                app.mark_dirty();
            }
            None => app.set_status(
                "nothing to register — raster layer with content (a selection scopes it)",
            ),
        },
        AppCmd::MaterialImportFolder(src) => {
            let n = app.material_import_folder(&src);
            app.set_status(if n > 0 {
                format!("imported {n} material(s)")
            } else {
                "no new images found in that folder".into()
            });
        }
        AppCmd::StoryEditor => {
            app.story_open_refresh();
            app.set_status(format!(
                "Story Editor — {} text field(s)",
                app.story_bufs.len()
            ));
        }
        AppCmd::ReaderOpen => app.reader_open(),
        AppCmd::ReaderReturn => app.reader_return(),
        AppCmd::PageCombineSpread => {
            if app.page_index + 1 >= app.pages.len() {
                app.set_status("no next page to combine with");
            } else {
                app.spread_op = Some(crate::app::SpreadOp::Combine);
            }
        }
        AppCmd::PageCombineApply { gap, delete_empty } => {
            app.spread_op = None;
            let i = app.page_index;
            if i + 1 >= app.pages.len() {
                app.set_status("no next page to combine with");
                return;
            }
            let Some(b_bytes) = app.pages[i + 1].bytes.take() else {
                app.set_error("next page has no data");
                return;
            };
            let Ok(b_doc) = mn_core::project::bytes_to_doc(&b_bytes) else {
                app.pages[i + 1].bytes = Some(b_bytes);
                app.set_error("next page failed to decode");
                return;
            };
            let mut doc = mn_core::page::combine_spread(&app.doc, &b_doc, gap);
            if delete_empty {
                mn_core::page::drop_empty_raster_layers(&mut doc);
            }
            let id = app.pages[i].id;
            let entry = app.fresh_spread(None);
            app.pages.drain(i..=i + 1);
            app.pages.insert(i, entry);
            app.pages[i].id = id; // keep A's work-folder file identity
            app.page_index = i;
            // Same tab, same work: the rulers carry (see `adopt_page_doc`).
            app.adopt_page_doc(doc);
            app.pages[i].doc_rev = app.doc.revision;
            app.mark_pages_dirty();
            app.renderer.invalidate();
            app.layer_thumbs.clear();
            app.fit_to_view();
            app.set_status("spread combined — draw across the gutter");
            app.mark_dirty();
        }
        AppCmd::PageSplitSpread => {
            if app.doc.size.0 < 128 {
                app.set_status("page too narrow to split");
            } else {
                app.spread_op = Some(crate::app::SpreadOp::Split);
            }
        }
        AppCmd::PageSplitApply { gap, delete_empty } => {
            app.spread_op = None;
            let Some((mut l, mut r)) = mn_core::page::split_spread(&app.doc, gap) else {
                app.set_status("page too narrow to split");
                return;
            };
            if delete_empty {
                mn_core::page::drop_empty_raster_layers(&mut l);
                mn_core::page::drop_empty_raster_layers(&mut r);
            }
            let Ok(r_bytes) = mn_core::project::doc_to_bytes(&r) else {
                app.set_error("split page failed to encode");
                return;
            };
            let id = app.pages[app.page_index].id;
            let i = app.page_index;
            let mut right_entry = app.fresh_page(Some(r_bytes), None);
            right_entry.id = 0; // new file identity at the next folder save
            let left_entry = app.fresh_page(None, None);
            app.pages.drain(i..=i);
            app.pages.insert(i, left_entry);
            app.pages[i].id = id; // the left half keeps the spread's file
            app.pages.insert(i + 1, right_entry);
            app.page_index = i;
            app.adopt_page_doc(l);
            app.pages[i].doc_rev = app.doc.revision;
            app.mark_pages_dirty();
            app.renderer.invalidate();
            app.layer_thumbs.clear();
            app.fit_to_view();
            app.set_status("spread split into two pages");
            app.mark_dirty();
        }
        AppCmd::AddPage => {
            // Template page (tekno B2): when one is designated, its bytes
            // seed the new page — panel skeleton, guide layers, whatever
            // the artist built once. The ACTIVE page's bytes live in `doc`,
            // so it stashes first; any failure falls back to a blank.
            let seed = app
                .template_page
                .filter(|&t| t < app.pages.len())
                .and_then(|t| {
                    if t == app.page_index {
                        app.stash_current_page().ok()?;
                    }
                    app.pages[t].bytes.clone()
                });
            let from_template = seed.is_some();
            let blank = seed.or_else(|| mn_core::project::doc_to_bytes(&app.blank_page_doc()).ok());
            let at = app.page_index + 1;
            let e = app.fresh_page(blank, None);
            app.pages.insert(at, e);
            app.mark_pages_dirty();
            if from_template {
                app.set_status(format!("page {} added from the template page", at + 1));
            } else {
                app.set_status(format!("page {} added", at + 1));
            }
            app.switch_page(at);
        }
        AppCmd::DeletePage => {
            let n = app.pages.len();
            if n <= 1 {
                app.set_status("a comic keeps at least one page");
            } else {
                let cur = app.page_index;
                let target = if cur + 1 < n { cur + 1 } else { cur - 1 };
                app.switch_page(target);
                if app.page_index == target {
                    app.pages.remove(cur);
                    if app.page_index > cur {
                        app.page_index -= 1;
                    }
                    app.mark_pages_dirty();
                    app.set_status(format!("deleted page {}", cur + 1));
                    app.mark_dirty();
                }
            }
        }
        AppCmd::MovePage { from, to } => {
            let n = app.pages.len();
            if from < n && to < n && from != to {
                let e = app.pages.remove(from);
                app.pages.insert(to, e);
                let a = app.page_index;
                app.page_index = if a == from {
                    to
                } else if from < a && a <= to {
                    a - 1
                } else if to <= a && a < from {
                    a + 1
                } else {
                    a
                };
                app.mark_pages_dirty();
                app.mark_dirty();
            }
        }
        AppCmd::DuplicatePage => {
            // Serialize the live page so the copy is byte-exact.
            match app.stash_current_page() {
                Err(e) => app.set_error(e),
                Ok(()) => {
                    let cur = app.page_index;
                    let bytes = app.pages[cur].bytes.clone();
                    let thumb = app.pages[cur].thumb.clone();
                    let e = app.fresh_page(bytes, thumb);
                    app.pages.insert(cur + 1, e);
                    // Restore the active-page invariant (bytes live in `doc`).
                    app.pages[cur].bytes = None;
                    app.mark_pages_dirty();
                    app.set_status(format!("page {} duplicated", cur + 1));
                    app.mark_dirty();
                }
            }
        }
        AppCmd::ImportPage | AppCmd::ReplacePage | AppCmd::BatchImportPages => {
            // Resolved to their picked forms by `main::pump_commands`.
        }
        AppCmd::BatchImportPagesPicked(mut files) => {
            // NAME order is the page order. A stack of ネーム photos is
            // named for the chapter (p01, p02, …); the order the OS picker
            // happened to hand them back is not an order at all. Plain
            // lexicographic, lowercased — zero-padded names, which is how
            // every camera and scanner writes them, sort right.
            files.sort_by_key(|p| {
                p.file_name()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .unwrap_or_default()
            });
            app.batch_import.files = files;
            app.batch_import.start = app.page_index + 1;
            app.batch_import_open = true;
            app.mark_dirty();
        }
        AppCmd::BatchImportApply => {
            let s = app.batch_import_pages();
            app.set_status(s);
            app.mark_dirty();
        }
        // Workflow audit §11 — the ネーム promotion path.
        AppCmd::PromoteNewWork => {
            app.promote_open = true;
            app.mark_dirty();
        }
        AppCmd::PromoteNewWorkApply => {
            let s = app.promote_new_work();
            app.set_status(s);
            app.mark_dirty();
        }
        AppCmd::StampNamePages => {
            // Resolved to StampNamePagesPath by `main::pump_commands`.
        }
        AppCmd::StampNamePagesPath(p) => {
            let s = app.stamp_name_pages(&p);
            app.set_status(s);
            app.mark_dirty();
        }
        AppCmd::ImportAbr => {
            // Resolved to ImportAbrPath by `main::pump_commands`.
        }
        AppCmd::ImportAbrPath(p) => app.import_abr(&p),
        AppCmd::ImportPagePath(p) => match app.file_to_page_bytes(&p, app.next_page_number1()) {
            Err(e) => app.set_error(format!("import failed: {e}")),
            Ok((bytes, note)) => {
                let at = app.page_index + 1;
                let e = app.fresh_page(Some(bytes), None);
                app.pages.insert(at, e);
                app.mark_pages_dirty();
                // switch_page sets its own status, so say ours after it.
                app.switch_page(at);
                let mut s = format!("imported {} as page {}", p.display(), at + 1);
                if let Some(n) = note {
                    s.push_str(&format!(" — {n}"));
                }
                app.set_status(s);
            }
        },
        AppCmd::ReplacePagePath(p) => match app
            .file_to_page_bytes(&p, app.page_number1(app.page_index))
        {
            Err(e) => app.set_error(format!("replace failed: {e}")),
            Ok((bytes, note)) => match mn_core::project::bytes_to_doc(&bytes) {
                Err(e) => app.set_error(format!("replace decode failed: {e}")),
                Ok(doc) => {
                    app.commit_text_edit();
                    app.adopt_page_doc(doc);
                    let i = app.page_index;
                    // The page's content was swapped wholesale: give it a
                    // fresh revision so a folder save rewrites its file even
                    // though the decoded doc may carry a coincidental
                    // matching revision.
                    app.pages[i].rev = app.page_rev_next();
                    app.pages[i].doc_rev = app.doc.revision;
                    app.pages[i].bytes = None;
                    app.pages[i].thumb = None;
                    app.renderer.invalidate();
                    app.layer_thumbs.clear();
                    app.fit_to_view();
                    app.mark_pages_dirty();
                    let mut s = format!(
                        "page {} replaced with {}",
                        app.page_index + 1,
                        p.display()
                    );
                    if let Some(n) = note {
                        s.push_str(&format!(" — {n}"));
                    }
                    app.set_status(s);
                    app.mark_dirty();
                }
            },
        },
        AppCmd::WorkSettings => {
            app.work_settings_draft = crate::app::WorkSettingsDraft {
                setup: app
                    .page
                    .clone()
                    .unwrap_or_else(|| mn_core::PageSetup::presets().remove(0)),
                binding_right: app.binding_right,
                story: app.story.clone(),
                print_margin_info: app.print_margin_info,
                print_crop_marks: app.print_crop_marks,
                expression: app.expression,
                spine_mm: app.spine_mm,
                cover: app.cover,
                profile: app.profile.clone(),
            };
            app.work_settings_open = true;
            app.mark_dirty();
        }
        AppCmd::WorkSettingsApply => {
            let d = app.work_settings_draft.clone();
            app.story = d.story;
            app.binding_right = d.binding_right;
            app.print_margin_info = d.print_margin_info;
            app.print_crop_marks = d.print_crop_marks;
            app.expression = d.expression;
            app.spine_mm = d.spine_mm;
            app.cover = d.cover;
            app.profile = d.profile;
            // Metadata edits do not bump the doc revision — tell the
            // preflight cache by hand.
            app.preflight_stale = true;
            // Geometry: guides update immediately; existing page pixels stay.
            // New pages (AddPage) pick the new size up via blank_page_doc.
            if d.setup.has_guides() {
                app.page = Some(d.setup);
            }
            app.work_settings_open = false;
            app.mark_pages_dirty();
            app.set_status("work settings updated");
            app.mark_dirty();
        }
        AppCmd::OpenCanvasSize => {
            app.canvas_size_draft = crate::app::CanvasSizeDraft {
                w: app.doc.size.0,
                h: app.doc.size.1,
                anchor: ResizeAnchor::Center,
                all_pages: false,
            };
            app.canvas_size_open = true;
        }
        AppCmd::OpenPageSize => {
            // The work's paper is the target the Work Settings draft was
            // just applied to; a pixel preset has none, so the open page
            // stands in.
            let (w, h) = app
                .page
                .as_ref()
                .filter(|s| s.has_guides())
                .map(|s| s.paper_px())
                .unwrap_or(app.doc.size);
            app.canvas_size_draft = crate::app::CanvasSizeDraft {
                w,
                h,
                anchor: ResizeAnchor::Center,
                all_pages: true,
            };
            app.work_settings_open = false;
            app.canvas_size_open = true;
            app.mark_dirty();
        }
        AppCmd::ResizeCanvasApply => {
            let d = app.canvas_size_draft;
            let (w, h) = (d.w.max(1), d.h.max(1));
            let (dx, dy) = d.anchor.offsets(app.doc.size, (w, h));
            app.canvas_size_open = false;
            // What a NORMAL page is, for the spread test, BEFORE the resize
            // moves the answer (`is_spread_page`'s width rule).
            let normal_w = app
                .page
                .as_ref()
                .map(|s| s.paper_px().0)
                .unwrap_or(app.doc.size.0);
            apply_canvas_resize(app, w, h, dx, dy);
            if d.all_pages {
                // The work's DEFAULT page size moves too, or the next
                // AddPage re-introduces the old geometry (blank_page_doc
                // reads `page.paper_px()`).
                if let Some(s) = app.page.as_mut() {
                    s.set_paper_px(w, h);
                    app.preflight_stale = true;
                }
                let (done, failed) = app.resize_other_pages(w, h, d.anchor, Some(normal_w));
                let mut s = format!("canvas {w}×{h} — {done} other page(s) resized directly");
                if failed > 0 {
                    s.push_str(&format!(", {failed} could not be read"));
                }
                app.set_status(s);
            }
        }
        AppCmd::OpenResampleWork => {
            app.resample_work_draft = crate::app::ResampleWorkDraft {
                dpi: app.work_dpi().unwrap_or(600),
                ..crate::app::ResampleWorkDraft::default()
            };
            app.work_settings_open = false;
            app.resample_work_open = true;
            app.mark_dirty();
        }
        AppCmd::ResampleWorkApply => {
            let d = app.resample_work_draft;
            // THE DOOR THAT STANDS IN FOR UNDO. A whole-work resample is
            // not an undo step (see `App::resample_work`), so the file on
            // disk has to be the way back — which means there has to BE
            // one AND it has to be current. Both halves are load-bearing: a
            // freshly created comic reads as NOT dirty (the create path
            // syncs the saved revision) while having no file at all, so a
            // dirty-only guard would wave through exactly the case with
            // nothing to fall back to. One Ctrl+S and the artist proceeds.
            let saved = app.doc_path.clone().filter(|_| !app.dirty());
            if saved.is_none() {
                app.set_error(
                    "save the work first — changing the resolution cannot be undone, \
                     and the saved file is the only way back"
                        .to_string(),
                );
            } else {
                let back = saved
                    .as_ref()
                    .map(|p| format!(" — the {} on disk is still at the old resolution", {
                        p.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| p.display().to_string())
                    }))
                    .unwrap_or_default();
                app.resample_work_open = false;
                app.end_stroke();
                app.commit_text_edit();
                app.transform_drag = None;
                app.last_selection = None;
                // The run itself is CHUNKED — one page per frame, with a
                // progress window and a Cancel (see `App::resample_work_step`).
                // The command's job ends at starting it; the finishing status
                // line is composed by the last step, which is the only place
                // that knows how many pages actually landed.
                if let Err(e) = app.resample_work_begin(d.dpi, d.interp, back) {
                    app.set_error(format!("resolution unchanged: {e}"));
                }
            }
            app.mark_dirty();
        }
        AppCmd::CropSelection => {
            let bbox = app.doc.selection.as_ref().and_then(selection_bbox);
            match bbox {
                Some([x0, y0, x1, y1]) if x1 > x0 && y1 > y0 => {
                    apply_canvas_resize(app, (x1 - x0) as u32, (y1 - y0) as u32, -x0, -y0);
                }
                _ => app.set_status("crop needs a selection first (M / W)"),
            }
        }
        AppCmd::ExportAllPages => {
            // PM-050: the options window opens FIRST now (prefix, page
            // range, split spreads, script dump), and every field is
            // seeded so an untouched Export writes exactly the files it
            // has always written, under exactly the old names. The
            // FINISH is deliberately not reseeded: the prefix belongs to
            // the work, the finish belongs to where you send it, and
            // re-picking the same preset every page run is the annoyance
            // presets exist to remove.
            app.export_all_prefix = default_export_stem(app);
            app.export_all_from = 1;
            app.export_all_to = app.pages.len().max(1) as i32;
            app.export_all_open = true;
        }
        AppCmd::ExportAllPagesGo => {}
        AppCmd::ExportAllPreset(i) => {
            if let Some(p) = mn_core::export::PRINT_PRESETS.get(i) {
                app.set_export_finish(p.finish);
            }
        }
        AppCmd::ExportAllPagesPath(dir) => match app.stash_current_page() {
            Err(e) => app.set_error(e),
            Ok(()) => {
                // PM-051: an empty prefix falls back to the work name, so
                // clearing the field cannot produce files called "-p001".
                let prefix = {
                    let p = app.export_all_prefix.trim();
                    if p.is_empty() {
                        default_export_stem(app)
                    } else {
                        p.to_owned()
                    }
                };
                // PM-054: the range is 1-based inclusive and clamped; the
                // FILENAME keeps the page's true number, so exporting
                // 5..8 gives -p005..-p008 rather than renumbering from 1.
                let n = app.pages.len();
                let (first, last) = if app.export_all_range && n > 0 {
                    let a = app.export_all_from.clamp(1, n as i32) as usize;
                    let b = app.export_all_to.clamp(1, n as i32) as usize;
                    (a.min(b), a.max(b))
                } else {
                    (1, n)
                };
                let split = app.export_all_split;
                let want_text = app.export_all_text;
                let rtl = app.binding_right;
                // What a NORMAL page is, for the spread test: the work's
                // own paper when it has a page setup, else the narrowest
                // page in the work (cheap — stack.xml, no pixel decode).
                // A work whose pages are all one width therefore has no
                // spread by this measure and nothing splits, which is the
                // right refusal: there is no evidence to guess from.
                let normal_w = match app.page.as_ref().map(|s| s.paper_px().0) {
                    Some(w) => Some(w),
                    None if split => (0..app.pages.len())
                        .map(|i| app.reader_page_canvas(i).0)
                        .min(),
                    None => None,
                };
                let total = last.saturating_sub(first) + 1;
                // Print finishing.
                let scale = mn_core::export::finish_scale(app.export_all_dpi, app.work_dpi());
                // Runner-up 13 (`IO-030`): the artist's call, not ours.
                // `Frequency` (the default) derives the screen at the
                // WORK's dpi and lets it be resampled with everything else,
                // so 60 lpi prints as 60 lpi and the reduction may moiré.
                // `Dots` derives it at `work / scale`, so the reduction
                // lands each cell back at its work-pixel size — no beat,
                // and a printed screen coarsened to `lpi × scale`.
                // The exact-height fit derives its own per-page scale after
                // the crop, which this cannot see, so the dialog refuses
                // the choice there rather than screening against a number
                // that is not the one being applied.
                let dpi = mn_core::export::tone_export_dpi(
                    app.tone_dpi(),
                    if app.export_all_px_height > 0 {
                        1.0
                    } else {
                        scale
                    },
                    app.export_all_tone,
                );
                let colour = app.export_all_colour;
                // M2: crop + exact-height ride the finish when asked for;
                // Paper + 0 takes the exact old path, byte-identical.
                let crop = app.export_all_crop;
                let px_h = app.export_all_px_height;
                // Finding 7/9: the resample kernel and the container ride
                // the finish like everything else above. Photo + PNG is
                // the byte-identical old path.
                let resample = app.export_all_resample;
                let format = app.export_all_format;
                let quality = app.export_all_quality;
                let ext = format.ext();
                let setup = app.page.clone();
                // The margin stamp (Work Settings): borrows taken BEFORE
                // the closure so the loop below can keep `&mut app.renderer`
                // — the closure captures these, not `&app`.
                let stamp_on = app.print_margin_info;
                let marks_on = app.print_crop_marks;
                let stamp_engine = app.text_engine.as_ref();
                let stamp_font = app.text_font.clone();
                let stamp_story = app.story.clone();
                let finish = |img: image::RgbaImage, number: &str| -> image::RgbaImage {
                    let in_px = (img.width(), img.height());
                    let full = [0, 0, img.width(), img.height()];
                    let r = match &setup {
                        Some(s) => {
                            mn_core::export::crop_rect_px(s, (img.width(), img.height()), crop)
                        }
                        None => full,
                    };
                    let mut out = if r != full || px_h > 0 {
                        mn_core::export::finish_image_cropped(
                            img, r, scale, px_h, colour, resample,
                        )
                    } else {
                        mn_core::export::finish_image(img, scale, colour, resample)
                    };
                    if stamp_on {
                        // Spread halves share the entry's number — the
                        // file names say p0NNa/p0NNb, the stamp must agree
                        // with the file it lands in.
                        crate::app::export_stamp::stamp_margin_info(
                            stamp_engine,
                            &stamp_font,
                            &stamp_story,
                            &mut out,
                            setup.as_ref(),
                            r,
                            scale,
                            px_h,
                            colour,
                            number,
                        );
                    }
                    // トンボ, placed through the SAME geometry the finish
                    // applied — a crop that ate the margin leaves the
                    // marks with nowhere to go and draws none.
                    if marks_on {
                        let (out_px, eff, applied) =
                            mn_core::export::finish_geometry(in_px, r, scale, px_h);
                        let marks =
                            mn_core::export::crop_marks(setup.as_ref(), applied, eff, out_px);
                        mn_core::export::apply_crop_marks(&mut out, &marks);
                    }
                    out
                };
                let write = |img: &image::RgbaImage, path: &std::path::Path| {
                    mn_core::export::save_finished(img, path, format, quality, colour).is_ok()
                };
                let mut ok = 0usize;
                let mut files = 0usize;
                // Which pages this run actually wrote — the export
                // reminder's ledger. Collected rather than recorded in
                // place because the loop holds `app.pages` by reference,
                // and a page outside the range (or one that failed to
                // save) must keep whatever it had.
                let mut exported: Vec<usize> = Vec::new();
                for (i, e) in app.pages.iter().enumerate() {
                    if i + 1 < first || i + 1 > last {
                        continue;
                    }
                    let Some(b) = &e.bytes else { continue };
                    if let Ok(mut d) = mn_core::project::bytes_to_doc(b) {
                        // Tone layers export their derived rasters — the
                        // freshly decoded doc starts with none. Derive
                        // BEFORE any split: the tone screen is canvas-
                        // continuous, so halving first would restart the
                        // dot phase on the second half and the seam would
                        // show in print.
                        crate::app::refresh_derived_gpu(&mut d, &mut app.renderer, dpi);
                        // PM-055: gap 0 — the export must not swallow the
                        // seam. The gutter swallow is an EDIT-time choice
                        // (PM-031), not something a print run gets to do.
                        let halves = (split && is_spread_page(&d, e.spread, normal_w))
                            .then(|| mn_core::page::split_spread(&d, 0))
                            .flatten();
                        match halves {
                            Some((left, right)) => {
                                // `a` is the half a reader meets first —
                                // the RIGHT one in a right-bound work.
                                let (h1, h2) = if rtl { (right, left) } else { (left, right) };
                            for (tag, half) in [("a", &h1), ("b", &h2)] {
                                let img = finish(
                                    mn_core::export::composite_for_export(
                                        half,
                                        d.paper_export_background(),
                                    ),
                                    &(i + 1).to_string(),
                                );
                                    let path =
                                        dir.join(format!("{prefix}-p{:03}{tag}.{ext}", i + 1));
                                    if write(&img, &path) {
                                        files += 1;
                                    }
                                }
                                ok += 1;
                                exported.push(i);
                            }
                            None => {
                                let img = finish(
                                    mn_core::export::composite_for_export(
                                        &d,
                                        d.paper_export_background(),
                                    ),
                                    &(i + 1).to_string(),
                                );
                                let path = dir.join(format!("{prefix}-p{:03}.{ext}", i + 1));
                                if write(&img, &path) {
                                    ok += 1;
                                    files += 1;
                                    exported.push(i);
                                }
                            }
                        }
                    }
                }
                // Restore the active-page invariant (bytes live in `doc`).
                app.pages[app.page_index].bytes = None;
                // The pages this run wrote are now up to date; the ones it
                // skipped keep saying so in the status bar.
                for i in exported {
                    app.note_page_exported(i);
                }
                // PM-053 as CSP has it: the script rides along with the
                // image run when the toggle is on.
                let mut extra = String::new();
                if files != ok {
                    extra.push_str(&format!(" ({files} files)"));
                }
                // A finish that changed the pixels says so: a silent
                // downscale is the one export result nobody can spot by
                // looking at the file names.
                if scale < 1.0 {
                    extra.push_str(&format!(" @{}%", (scale * 100.0).round() as i32));
                    // Which kernel ran is invisible in a file listing and
                    // decides whether the hairlines are still there.
                    if resample.is_comic(colour) {
                        extra.push_str(" comic");
                    }
                }
                if let Some(n) = colour.ora_name() {
                    extra.push_str(&format!(" {n}"));
                }
                if format == mn_core::export::ExportFormat::Jpeg {
                    extra.push_str(&format!(" jpeg q{quality}"));
                }
                if want_text {
                    let body = app.script_dump();
                    let p = dir.join(format!("{prefix}-text.txt"));
                    extra.push_str(if std::fs::write(&p, body).is_ok() {
                        " + script"
                    } else {
                        " (script FAILED)"
                    });
                }
                app.set_status(format!(
                    "exported {ok}/{total} pages{extra} -> {}",
                    dir.display()
                ));
            }
        },
        other => return file_io::run(app, other, cmd_tail),
    }
    run_cmd_tail(app, cmd_tail);
}
