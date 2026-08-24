use super::headless_renderer;
use crate::app::App;

/// A text item at a chosen centre, with everything else at defaults.
fn dump_item(text: &str, cx: f32, cy: f32) -> mn_core::text::TextItem {
    let size = [80.0f32, 40.0f32];
    mn_core::text::TextItem {
        text: text.into(),
        runs: Vec::new(),
        pos: [cx - size[0] * 0.5, cy - size[1] * 0.5],
        size,
        auto_size: false,
        rotation: 0.0,
        font: "serif".into(),
        size_pt: 12.0,
        color: [0, 0, 0],
        outline_px: 0.0,
        outline_color: [255, 255, 255],
        vertical: true,
        align: Default::default(),
        frame_align: Default::default(),
        letter_spacing_pt: 0.0,
        line_spacing: Default::default(),
        ruby: Vec::new(),
        ruby_style: mn_core::text::RubyStyle::default(),
        tcy: Vec::new(),
        auto_tcy: 0,
        fonts: Vec::new(),
        style: None,
        cache: None,
    }
}

/// PM-053: the script dump groups by page, files each balloon into the
/// panel it sits in, numbers those panels in READING order (a
/// right-bound work meets the right panel first), sends items in no
/// panel to the tail, skips hidden text, and lands as UTF-8 with a BOM
/// and CRLF ends because it leaves the app for Windows tools.
#[test]
fn script_dump_reads_in_panel_order() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    assert!(app.binding_right, "the default work is right-bound");
    // Two panels side by side: the RIGHT one reads first.
    let panels = mn_core::FrameSet {
        frames: vec![
            mn_core::Frame::rect(10.0, 10.0, 290.0, 190.0),
            mn_core::Frame::rect(310.0, 10.0, 590.0, 190.0),
        ],
        border_px: 2.0,
        border_ruler: false,
        color: [0, 0, 0],
        slot: None,
        reading_pin: None,
    };
    app.doc.add_frame_folder("Frame 1", panels);
    app.doc.add_text_layer(
        "script",
        mn_core::TextSet {
            texts: vec![
                dump_item("left panel", 150.0, 100.0),
                dump_item("right panel", 450.0, 100.0),
                dump_item("loose sfx", 300.0, 350.0),
            ],
        },
    );
    // A hidden text layer is not in the handoff (it is not printed).
    app.doc.add_text_layer(
        "rough",
        mn_core::TextSet {
            texts: vec![dump_item("never exported", 450.0, 120.0)],
        },
    );
    app.doc.layers.last_mut().unwrap().visible = false;

    let body = app.script_dump();
    assert!(body.starts_with('\u{feff}'), "BOM for Windows tools");
    assert!(body.contains("\r\n"), "CRLF ends");
    assert!(!body.contains("never exported"), "hidden text is skipped");
    let at = |needle: &str| body.find(needle).unwrap_or_else(|| panic!("no {needle}"));
    assert!(at("== Page 1 ==") < at("-- Panel 1 --"));
    assert!(
        at("right panel") < at("left panel"),
        "right-bound: the right panel is Panel 1"
    );
    assert!(at("-- Panel 1 --") < at("right panel"));
    assert!(at("-- Panel 2 --") < at("left panel"));
    assert!(
        at("-- Outside panels --") > at("left panel"),
        "orphans come last"
    );
    assert!(at("loose sfx") > at("-- Outside panels --"));

    // Every page gets a marker even with nothing on it — the page
    // numbers have to stay countable.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddPage);
    let body = app.script_dump();
    assert!(body.contains("== Page 2 =="), "empty pages still marked");
}

/// PM-050/051/054: the options window seeds from the work, and an
/// UNTOUCHED export writes exactly the files the pre-options export
/// wrote — same names, same count. That equality is the whole point of
/// the round; the range is checked on top of it.
#[test]
fn batch_export_defaults_are_unchanged() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    for _ in 0..2 {
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddPage);
    }
    assert_eq!(app.pages.len(), 3);

    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::ExportAllPages);
    assert!(app.export_all_open, "the options window opens first");
    assert_eq!(app.export_all_prefix, "page", "unnamed work -> page");
    assert_eq!((app.export_all_from, app.export_all_to), (1, 3));
    assert!(!app.export_all_range && !app.export_all_split && !app.export_all_text);

    let dir = std::env::temp_dir().join(format!("mn-batch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::ExportAllPagesPath(dir.clone()),
    );
    for n in 1..=3 {
        assert!(
            dir.join(format!("page-p{n:03}.png")).exists(),
            "page-p{n:03}.png — the pre-options name"
        );
    }
    assert!(
        app.pages[app.page_index].bytes.is_none(),
        "the active page's bytes go back to living in `doc`"
    );

    // PM-051 + PM-054: a prefix and a range. The filename keeps the
    // page's OWN number — exporting 2..3 must not renumber to 1..2.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.export_all_prefix = "ch01".into();
    app.export_all_range = true;
    app.export_all_from = 3;
    app.export_all_to = 2; // reversed on purpose: apply sorts them
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::ExportAllPagesPath(dir.clone()),
    );
    assert!(!dir.join("ch01-p001.png").exists(), "page 1 out of range");
    assert!(dir.join("ch01-p002.png").exists());
    assert!(dir.join("ch01-p003.png").exists());

    // PM-053 riding along with the image run.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.export_all_range = false;
    app.export_all_text = true;
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::ExportAllPagesPath(dir.clone()),
    );
    assert!(dir.join("ch01-text.txt").exists(), "the script rides along");
    std::fs::remove_dir_all(&dir).ok();
}

/// Print finishing: a preset fills all three finishing fields at once,
/// the picker finds its own name back, and moving ANY of them reads as
/// Custom — the picker stores no index, so there is nothing to go stale.
#[test]
fn export_preset_fills_the_draft_and_an_edit_reads_as_custom() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    let finish = |a: &App| mn_core::export::matching_preset(a.export_finish());
    assert_eq!(finish(&app), None, "a fresh app is Custom, not a preset");
    assert_eq!(app.export_all_dpi, 0);
    assert_eq!(app.export_all_colour, mn_core::LayerExpression::Colour);

    for (i, p) in mn_core::export::PRINT_PRESETS.iter().enumerate() {
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::ExportAllPreset(i));
        assert_eq!(app.export_all_dpi, p.finish.dpi, "{}", p.name);
        assert_eq!(app.export_all_colour, p.finish.colour, "{}", p.name);
        assert_eq!(app.export_all_split, p.finish.split_spreads, "{}", p.name);
        assert_eq!(finish(&app), Some(i), "{} reads its own name back", p.name);
    }

    // One edit per field, each on its own, each flipping to Custom.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::ExportAllPreset(0));
    app.export_all_dpi += 1;
    assert_eq!(finish(&app), None, "dpi edit -> Custom");
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::ExportAllPreset(0));
    app.export_all_colour = mn_core::LayerExpression::Grey;
    assert_eq!(finish(&app), None, "colour edit -> Custom");
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::ExportAllPreset(0));
    app.export_all_split = !app.export_all_split;
    assert_eq!(finish(&app), None, "split edit -> Custom");

    // An index the preset list does not have leaves the draft alone.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::ExportAllPreset(0));
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::ExportAllPreset(99));
    assert_eq!(finish(&app), Some(0), "a stale index is ignored, not fatal");
}

/// The finish reaches the FILES: the output dpi resamples, and the mono
/// reduce runs after it, so a downscaled checkerboard lands 1-bit rather
/// than the mid-greys the filter produced.
#[test]
fn export_finish_resamples_then_thresholds() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    app.doc = mn_core::Document::new(64, 64);
    app.page = Some(mn_core::PageSetup::presets().remove(1)); // B4 600 dpi
    // A 1-px checkerboard of opaque black: any resample turns it grey.
    let li = app.doc.add_layer("ink");
    let tile = app.doc.layers[li].tile_mut(mn_core::TileIdx::new(0, 0));
    for y in 0..64u32 {
        for x in 0..64u32 {
            if (x + y) % 2 == 0 {
                tile.set_pixel(x as usize, y as usize, [0, 0, 0, mn_core::FIX15_ONE as u16]);
            }
        }
    }

    let dir = std::env::temp_dir().join(format!("mn-finish-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.export_all_dpi = 300; // half of the work's 600
    app.export_all_colour = mn_core::LayerExpression::Mono;
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::ExportAllPagesPath(dir.clone()),
    );
    let img = image::open(dir.join("page-p001.png")).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (32, 32), "300 dpi out of a 600 dpi work");
    assert!(
        img.pixels().all(|p| p.0[0] == 0 || p.0[0] == 255),
        "a resampled grey survived the 1-bit finish"
    );

    // Untouched finishing fields leave the file exactly as it was.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.export_all_dpi = 0;
    app.export_all_colour = mn_core::LayerExpression::Colour;
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::ExportAllPagesPath(dir.clone()),
    );
    let img = image::open(dir.join("page-p001.png")).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (64, 64), "0 dpi = the work's own size");
    std::fs::remove_dir_all(&dir).ok();
}

/// PM-055: a spread leaves as two files — `a` first for the reader —
/// and only when the toggle is on. Detection survives the runtime
/// flag being absent: a canvas half again as wide as the work's own
/// paper is a spread whatever the session thinks.
#[test]
fn batch_export_splits_spreads_when_asked() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let mut app = App::new(renderer, (600, 400), 1.0);
    // Page 1 is the work's NORMAL page, and the fixture has to say so:
    // `App::new`'s `(600, 400)` is the CLIENT size, and the document it
    // starts with is the New-canvas preference (2048² out of the box).
    // Leave that in place and page ONE is the widest page in the work,
    // so the structural test reads page one as the spread and halves it
    // while the real spread goes out whole — the heuristic behaving
    // correctly on a "work" whose pages were never one work.
    app.doc = mn_core::Document::new(600, 400);
    // Page 2 is a double-wide canvas with NO runtime spread flag.
    let wide = mn_core::Document::new(1200, 400);
    let bytes = mn_core::project::doc_to_bytes(&wide).unwrap();
    let e = app.fresh_page(Some(bytes), None);
    app.pages.push(e);
    assert!(!app.pages[1].spread, "no session flag on this one");

    let dir = std::env::temp_dir().join(format!("mn-split-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Toggle OFF: the wide page is one file, exactly as before.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::ExportAllPagesPath(dir.clone()),
    );
    assert!(dir.join("page-p002.png").exists());
    assert!(!dir.join("page-p002a.png").exists());

    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.export_all_split = true;
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::ExportAllPagesPath(dir.clone()),
    );
    assert!(
        dir.join("page-p001.png").exists(),
        "a normal page is untouched by the toggle"
    );
    assert!(!dir.join("page-p002.png").exists(), "the spread is halved");
    for tag in ["a", "b"] {
        let p = dir.join(format!("page-p002{tag}.png"));
        let img = image::open(&p).unwrap_or_else(|_| panic!("no {}", p.display()));
        assert_eq!(img.width(), 600, "each half is one page wide");
    }
    std::fs::remove_dir_all(&dir).ok();
}

// --- the unexported-pages reminder (owner ask 2026-08-22) ----------------

/// One undoable edit — the cheapest thing that moves a page's content.
fn edit(app: &mut App) {
    app.doc.begin_op();
    let li = app.doc.active;
    app.doc.layers[li]
        .tile_mut(mn_core::TileIdx::new(0, 0))
        .set_pixel(1, 1, [32768, 0, 0, 32768]);
    app.doc.end_op();
}

/// A three-page work in a fresh temp folder to export into.
fn work_and_dir(tag: &str, renderer: mn_gpu::Renderer) -> (App, std::path::PathBuf) {
    let mut app = App::new(renderer, (600, 400), 1.0);
    for _ in 0..2 {
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddPage);
    }
    let dir = std::env::temp_dir().join(format!("mn-unexported-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (app, dir)
}

/// The reminder's core arithmetic: an edit after an export makes the page
/// count, and re-exporting clears it. The LIVE page counts before it
/// stashes (its `rev` only moves then) and still counts after — the two
/// halves of the same page, not two different dirty systems.
#[test]
fn an_edit_after_an_export_makes_the_page_count_as_unexported() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let (mut app, dir) = work_and_dir("edit", renderer);

    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::ExportAllPagesPath(dir.clone()),
    );
    assert!(
        app.pages.iter().all(|e| e.exported_rev > 0),
        "every page was written, so every page is recorded"
    );
    assert_eq!(app.unexported_pages(), 0, "fresh off an export: nothing");

    edit(&mut app);
    assert_eq!(
        app.unexported_pages(),
        1,
        "the page being drawn on counts before it stashes"
    );
    app.stash_current_page().unwrap();
    let e = &app.pages[app.page_index];
    assert!(
        e.rev > e.exported_rev,
        "the stash gave it a fresh revision, above the exported one"
    );
    assert_eq!(app.unexported_pages(), 1, "…and it still counts once");

    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::ExportAllPagesPath(dir.clone()),
    );
    assert_eq!(app.unexported_pages(), 0, "re-exporting clears it");
    std::fs::remove_dir_all(&dir).ok();
}

/// PM-054's range is the reminder's sharpest edge: an export of pages 2..3
/// must clear those two and leave page 1 saying it is still out of date.
/// Marking the whole work would be the silent failure this feature exists
/// to prevent.
#[test]
fn an_export_range_clears_only_the_pages_it_wrote() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let (mut app, dir) = work_and_dir("range", renderer);
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::ExportAllPagesPath(dir.clone()),
    );
    assert_eq!(app.unexported_pages(), 0);

    // Every page edited, through the page-switch door that stashes.
    for i in 0..3 {
        app.switch_page(i);
        edit(&mut app);
    }
    assert_eq!(app.unexported_pages(), 3, "all three moved");

    app.export_all_range = true;
    app.export_all_from = 2;
    app.export_all_to = 3;
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::ExportAllPagesPath(dir.clone()),
    );
    assert_eq!(app.unexported_pages(), 1, "page 1 was not in the range");
    assert!(
        app.pages[0].rev > app.pages[0].exported_rev,
        "and it is page 1 specifically"
    );
    for i in 1..3 {
        assert!(
            app.pages[i].rev <= app.pages[i].exported_rev,
            "page {} was written by this run",
            i + 1
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// A work nobody has ever exported must stay silent however much it is
/// edited: the reminder is for re-exports, and a chip on every new work
/// would be nagging rather than reminding.
#[test]
fn a_work_that_was_never_exported_stays_quiet() {
    let Some(renderer) = headless_renderer() else {
        return;
    };
    let (mut app, dir) = work_and_dir("quiet", renderer);
    for i in 0..3 {
        app.switch_page(i);
        edit(&mut app);
    }
    assert!(
        app.pages.iter().all(|e| e.exported_rev == 0),
        "nothing has been exported"
    );
    assert_eq!(app.unexported_pages(), 0, "so there is nothing to say");

    // One single-page Export PNG is enough to start the ledger — for that
    // page only, and the rest of the work then reports honestly.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::ExportPngPath(dir.join("one.png")),
    );
    assert!(app.pages[app.page_index].exported_rev > 0);
    assert_eq!(
        app.unexported_pages(),
        2,
        "the two pages that export never wrote"
    );
    std::fs::remove_dir_all(&dir).ok();
}
