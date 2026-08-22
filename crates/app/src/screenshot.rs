//! `--screenshot PATH`: render one full frame (canvas + egui) offscreen, write
//! a PNG, exit. No window, no message loop.
//!
//! This exists because the UI cannot be inspected any other way from a headless
//! agent session, and it is the seed of a future UI-verification harness. It
//! deliberately reuses the *real* `ui::build` and `Shell::paint`, so it fails
//! the same way the app would.
//!
//! The compositor's `render_offscreen` owns its texture and hands back an image,
//! so the two layers are combined on the CPU: canvas image, then the egui pass
//! rendered to a transparent texture of the same size, composited source-over
//! (egui's output is premultiplied).

use std::path::Path;

use mn_core::PenSample;
use mn_gpu::{GpuConfig, Renderer};

use crate::app::{App, PointerKind};

pub fn run(
    cfg: GpuConfig,
    out: &Path,
    size: (u32, u32),
    shot_transform: bool,
    shot_selection: bool,
    shot_framefocus: bool,
    shot_tone: bool,
    shot_dock: bool,
    // --shot-hero: a README-grade shot — diagnostics window closed,
    // content only. Everything else about the frame is the real app.
    shot_hero: bool,
    gpu_dabs: bool,
) -> Result<(), String> {
    let (w, h) = size;
    let renderer = Renderer::new_headless(cfg).map_err(|e| e.to_string())?;
    println!("[app] adapter: {}", renderer.adapter_line());

    let mut app = App::new(renderer, (w, h), 1.0);
    // Without this the harness could never see the GPU dab path at all: `run`
    // is called from `main` BEFORE `app.gpu_dabs` is assigned there, so
    // `--screenshot --gpu-dabs` silently rendered the CPU path and any GPU-dab
    // regression was invisible to the only UI harness an agent session has.
    app.gpu_dabs = gpu_dabs && app.renderer.gpu_dabs_supported();
    println!(
        "[app] gpu-dabs: requested={gpu_dabs} enabled={}",
        app.gpu_dabs
    );
    // Everything the shot is meant to prove renders, including the HUD —
    // except a hero shot, whose audience is a README reader, not an agent.
    app.hud_open = !shot_hero;
    // The hero shot builds its OWN document (a blank manuscript) and leaves
    // this harness's demo page — strokes, balloon, tone, self-checks — to the
    // agent-facing shots. Capture itself is shared, deliberately: the README
    // image must come out of the same path the app renders with.
    if shot_hero {
        hero_doc(&mut app);
        let img = capture(&mut app, w, h, true)?;
        img.save(out).map_err(|e| format!("png write: {e}"))?;
        println!("[app] screenshot {}x{} -> {}", w, h, out.display());
        return Ok(());
    }
    // Round 7/10: a frame border FOLDER cut into three panels — proves the
    // koma raster (white gutter + borders), the folder rows (header + White +
    // draw layer) and Layer Property. The strokes go in AFTER it exists so
    // they land on the draw layer inside the folder (the White layer hides
    // anything below the folder — that is its job).
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::NewFrameLayer);
    demo_strokes(&mut app);
    // Self-check (round 19): the demo strokes must land on the tile grid where
    // they were aimed. Round 15's "renders ~1000px off" anomaly was this exact
    // harness feeding canvas-space samples into `push_batch`, which applies
    // `viewport.to_canvas` itself — a double transform. Pinned here so a
    // coordinate-space regression in the harness fails loudly.
    {
        let b = app
            .doc
            .active_layer()
            .tile_bounds()
            .expect("demo strokes painted something");
        let (lo, hi) = DEMO_FOOTPRINT;
        // tile_bounds is (x, y, w, h), tile-aligned.
        let (x1, y1) = (b.0 + b.2 as i32, b.1 + b.3 as i32);
        let ok = b.0 >= lo.0 - TILE && b.1 >= lo.1 - TILE && x1 <= hi.0 + TILE && y1 <= hi.1 + TILE;
        println!("[shot] stroke tile_bounds={b:?} expected≈{lo:?}..{hi:?} ok={ok}");
        // Content fingerprint of the stroked layer. Lets a caller tell a
        // DOCUMENT difference (rasterization/readback) apart from a DISPLAY
        // difference (compositor cache) when comparing two runs — the question
        // that matters first when GPU and CPU dab output disagree.
        let l = app.doc.active_layer();
        let mut tiles: Vec<_> = l.tiles().map(|(i, _)| i).collect();
        tiles.sort_by_key(|i| (i.y, i.x));
        let (mut sum, mut alpha) = (0u64, 0u64);
        for i in &tiles {
            if let Some(t) = l.tile(*i) {
                for (k, v) in t.data().iter().enumerate() {
                    sum = sum.wrapping_mul(31).wrapping_add((*v as u64) ^ (k as u64));
                    if k % 4 == 3 {
                        alpha += *v as u64;
                    }
                }
            }
        }
        println!(
            "[shot] layer fingerprint: tiles={} alpha_sum={alpha} hash={sum:016x}",
            tiles.len()
        );
    }
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::FrameDivide {
            a: (0.0, 1024.0),
            b: (2048.0, 1024.0),
        },
    );
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::FrameDivide {
            a: (1024.0, 0.0),
            b: (1024.0, 1000.0),
        },
    );
    // Round 8: a speech balloon with a tail in the top-left panel — proves the
    // balloon raster (white fill + outline + merged tail) and the balloon row.
    crate::cmd::dispatch(
        &mut app,
        crate::cmd::AppCmd::BalloonAdd {
            balloon: mn_core::Balloon {
                shape: mn_core::BalloonShape::Ellipse {
                    center: [560.0, 660.0],
                    radii: [220.0, 150.0],
                },
                tails: vec![mn_core::Tail {
                    base: [620.0, 760.0],
                    tip: [800.0, 970.0],
                    width: 90.0,
                    ..Default::default()
                }],
                ..Default::default()
            },
        },
    );
    // Round 9: vertical JP text with an edge, typed through the REAL editing
    // pipeline (session, WM_CHAR path, Enter, Esc commit) — proves DirectWrite
    // shaping, the sprite raster, auto-size and the text-layer row.
    if app.text_engine.is_some() {
        app.text_size_pt = 40.0;
        app.text_outline_mm = 0.5;
        crate::cmd::dispatch(
            &mut app,
            crate::cmd::AppCmd::SetTool(crate::cmd::Tool::Text),
        );
        app.text_tool_down(1560.0, 360.0, false);
        app.text_tool_up(1560.0, 360.0);
        for unit in "こんにちは".encode_utf16().collect::<Vec<u16>>() {
            app.text_char(unit);
        }
        app.text_key(0x0D, false, false); // Enter
        for unit in "世界！".encode_utf16().collect::<Vec<u16>>() {
            app.text_char(unit);
        }
        app.text_key(0x1B, false, false); // Esc commits (one undo step)
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetTool(crate::cmd::Tool::Pen));
    }
    // A second page so the Pages panel (and page-2's thumbnail) is in shot,
    // then back to page 1 so the strokes are too — a full switch round-trip.
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::AddPage);
    crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SelectPage(0));
    // Fingerprint of the DECODED page-1 doc: together with the pre-encode
    // print above this splits "document corrupted by the encode/stash path"
    // (this fingerprint drifts) from "display composited stale cache" (this
    // fingerprint holds while the PNGs disagree) — the exact question the
    // GPU-dabs display-divergence hunt needed answered.
    {
        let li = app
            .doc
            .layers
            .iter()
            .enumerate()
            .filter(|(_, l)| l.paintable() && !l.is_vector())
            .filter_map(|(i, l)| l.tile_bounds().map(|b| (i, b.2 as u64 * b.3 as u64)))
            .min_by_key(|&(_, area)| area)
            .map(|(i, _)| i)
            .expect("decoded stroke layer");
        let l = &app.doc.layers[li];
        // Sorted like the pre-encode fingerprint — HashMap iteration order
        // would make the hash vary run to run for identical pixels.
        let mut tiles: Vec<_> = l.tiles().map(|(i, _)| i).collect();
        tiles.sort_by_key(|i| (i.y, i.x));
        let (mut sum, mut alpha) = (0u64, 0u64);
        let mut n = 0;
        for i in &tiles {
            let Some(t) = l.tile(*i) else { continue };
            n += 1;
            for (k, v) in t.data().iter().enumerate() {
                sum = sum.wrapping_mul(31).wrapping_add((*v as u64) ^ (k as u64));
                if k % 4 == 3 {
                    alpha += *v as u64;
                }
            }
        }
        println!("[shot] decoded layer fingerprint: tiles={n} alpha_sum={alpha} hash={sum:016x}");
    }

    // --shot-transform: start a Transform on the stroked raster layer and
    // drag a bbox corner (the REAL gesture path — down/move/up), capturing
    // mid-float so the shot proves the veil, the preview mesh, the bbox and
    // the corner handles. Cancelled after capture; the doc is unchanged.
    if shot_transform {
        // The stroke layer: the SMALLEST populated raster (the full-canvas
        // White layers of the frame folders are bigger).
        let li = app
            .doc
            .layers
            .iter()
            .enumerate()
            .filter(|(_, l)| l.paintable() && !l.is_vector())
            .filter_map(|(i, l)| l.tile_bounds().map(|b| (i, b.2 as u64 * b.3 as u64)))
            .min_by_key(|&(_, area)| area)
            .map(|(i, _)| i)
            .ok_or("shot: no raster layer with content")?;
        app.doc.set_active(li);
        // Fresh, unambiguous strokes for the float, aimed in CANVAS space —
        // pushed through `to_screen` because `push_batch` converts back (same
        // coordinate-space rule as `demo_strokes`).
        let saved = app.active_color();
        app.engine_mut().set_color([0.08, 0.05, 0.10]);
        let vp = app.viewport;
        app.begin_stroke(PointerKind::Mouse);
        let batch: Vec<PenSample> = (0..64)
            .map(|k| {
                let t = k as f32 / 63.0;
                let (x, y) = vp.to_screen(340.0 + t * 420.0, 470.0 + (t * 9.0).sin() * 60.0);
                PenSample {
                    x,
                    y,
                    pressure: (0.2 + t * 0.8).min(1.0),
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: k as f64 * 4.0,
                }
            })
            .collect();
        app.push_batch(&batch);
        app.end_stroke();
        app.engine_mut().set_color(saved);
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformStart);
        let drag = app
            .transform_drag
            .as_ref()
            .ok_or("shot: no transform drag")?;
        // Grab the bbox's bottom-right corner; move it out and up (scale +
        // rotate in one gesture).
        let [bx, by] = drag.bbox[2];
        let (sx, sy) = app.viewport.to_screen(bx, by);
        app.canvas_down(sx, sy, PointerKind::Mouse, &[]);
        let (ex, ey) = (sx + 130.0, sy - 80.0);
        app.canvas_move(ex, ey, &[]);
        app.canvas_up(ex, ey, &[]);
    }

    // --shot-selection: a lasso selection through the REAL drag path —
    // proves the marching ants and the Selection Launcher bar (the ants'
    // crawl phase is wall-clock, so this shot is informational, not a
    // pixel gate).
    if shot_selection {
        crate::cmd::dispatch(
            &mut app,
            crate::cmd::AppCmd::SetTool(crate::cmd::Tool::Select),
        );
        let lasso: Vec<(f32, f32)> = (0..=40)
            .map(|k| {
                let t = k as f32 / 40.0;
                let a = t * std::f32::consts::TAU;
                (700.0 + a.cos() * 260.0, 760.0 + a.sin() * 180.0)
            })
            .collect();
        let (sx, sy) = app.viewport.to_screen(lasso[0].0, lasso[0].1);
        app.canvas_down(sx, sy, PointerKind::Mouse, &[]);
        for p in &lasso[1..] {
            let (x, y) = app.viewport.to_screen(p.0, p.1);
            app.canvas_move(x, y, &[]);
        }
        let (ex, ey) = app.viewport.to_screen(lasso[0].0, lasso[0].1);
        app.canvas_up(ex, ey, &[]);
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::SetTool(crate::cmd::Tool::Pen));
    }

    // --shot-framefocus: activate the top frame folder, switch to Object,
    // click a panel through the real path — proves the focus veil, the red
    // bbox + 8 handles, the rotation lollipop, the blue page rect.
    if shot_framefocus {
        let fi = app
            .doc
            .layers
            .iter()
            .position(|l| l.folder && l.is_frame())
            .ok_or("shot: no frame folder")?;
        app.doc.set_active(fi);
        crate::cmd::dispatch(
            &mut app,
            crate::cmd::AppCmd::SetTool(crate::cmd::Tool::Object),
        );
        let (sx, sy) = app.viewport.to_screen(400.0, 400.0);
        app.canvas_down(sx, sy, PointerKind::Mouse, &[]);
        app.canvas_up(sx, sy, &[]);
    }

    // --shot-tone: convert the stroked draw layer into a tone layer through
    // the REAL command path — proves the derived halftone renders on the GPU
    // compositor, the Layer Property Tone section, and the row marker.
    if shot_tone {
        let li = app
            .doc
            .layers
            .iter()
            .enumerate()
            .filter(|(_, l)| l.paintable() && !l.is_vector() && !l.folder)
            .filter_map(|(i, l)| l.tile_bounds().map(|b| (i, b.2 as u64 * b.3 as u64)))
            .min_by_key(|&(_, area)| area)
            .map(|(i, _)| i)
            .ok_or("shot: no raster layer with content")?;
        app.doc.set_active(li);
        crate::cmd::dispatch(
            &mut app,
            crate::cmd::AppCmd::SetTone(Some(mn_core::ToneParams::default())),
        );
    }

    // --shot-dock: tear the Layers palette off into a floating dock window
    // over the canvas — proves the floating surface renders and the column
    // tree reflows (what a manual tab-drag does interactively).
    if shot_dock {
        use crate::ui::dock::{Palette, Pane};
        let path = app
            .dock
            .iter_all_tabs()
            .find(|(_, t)| **t == Pane::Palette(Palette::Layers))
            .map(|(p, _)| p)
            .expect("Layers tab in the default tree");
        app.dock.remove_tab(path);
        let si = app.dock.add_window(vec![Pane::Palette(Palette::Layers)]);
        if let Some(ws) = app.dock.get_window_state_mut(si) {
            ws.set_position(egui::pos2(660.0, 280.0));
            ws.set_size(egui::vec2(250.0, 380.0));
        }
        // The Auto Actions palette is in this shot, and it is only worth
        // shooting with an action OPEN: closed, it is a list of names and
        // the Scratch-style step blocks never render.
        app.action_selected = (!app.actions.is_empty()).then_some(0);
    }

    // The derived tone rasters normally refresh at the head of App::render;
    // this harness never runs it, so a tone layer added above would shoot
    // as an invisible no-op without this.
    app.refresh_tones();

    let img = capture(&mut app, w, h, shot_hero)?;

    if shot_transform {
        // E2E commit proof: the dragged (non-identity) transform commits as
        // exactly ONE undo step that restores the pre-transform pixels.
        let sig0 = tile_sig(&app);
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::TransformCommit);
        let committed = app.doc.can_undo();
        let restored = app.doc.undo() && tile_sig(&app) == sig0;
        println!(
            "[shot] transform e2e: committed={} one-undo-restores={}",
            committed, restored
        );
    }
    img.save(out).map_err(|e| format!("png write: {e}"))?;
    println!("[app] screenshot {}x{} -> {}", w, h, out.display());
    Ok(())
}

/// Canvas-space rectangle the demo strokes are aimed at (x0, y0, x1, y1).
const DEMO_FOOTPRINT: ((i32, i32), (i32, i32)) = ((320, 186), (800, 414));
/// Slack for the footprint self-check: max dab radius (~40px) + tile
/// granularity. The check pins the stroke CENTER path — dab dilation is brush
/// physics, not a coordinate bug.
const TILE: i32 = 128;

/// The hero shot's document (owner's call, 2026-08-22): a BLANK three-page
/// work in the Japanese submission manuscript format — B4 paper at 600 dpi
/// with the Shueisha trim/bleed/inner guides — page 1 cut into five empty
/// panels, pages 2 and 3 left as the single default frame. The README should
/// show the manuscript the app is for, not a demo page of art.
///
/// Built entirely through the real commands: New Comic (the dialog's draft +
/// `NewComicCreate`) and the Frame tool's divide, aimed the way a drag across
/// a panel aims it.
fn hero_doc(app: &mut App) {
    use crate::cmd::{AppCmd, FrameMode, dispatch};

    // The SHIPPED layout, whatever `ui.txt` beside the exe says: a README
    // image taken on a machine whose palettes had been dragged around
    // advertises that machine, not the app. The one departure: Pages gets a
    // leaf of its own under Layers instead of the foot of the left column,
    // where the default's share of a 1000 px window is too short to show a
    // three-page work. Built from the same column builders as `default_tree`,
    // with the same width seeds, so the columns come out shipped-width.
    {
        use crate::ui::dock::{Palette, default_left, merge_columns};
        let mut left = default_left();
        // (bound first: an `if let` would pin the iterator's borrow of `left`
        // across the body, and `remove_tab` needs it mutably.)
        let pages_tab = left
            .iter_all_tabs()
            .find(|(_, t)| **t == Palette::Pages)
            .map(|(p, _)| p);
        if let Some(path) = pages_tab {
            left.remove_tab(path);
        }
        let mut right = egui_dock::DockState::new(vec![Palette::Color, Palette::ColorSet]);
        {
            let tree = right.main_surface_mut();
            let [_, mid] = tree.split_below(
                egui_dock::NodeIndex::root(),
                0.30,
                vec![Palette::Layers, Palette::Actions],
            );
            tree.split_below(mid, 0.42, vec![Palette::Pages]);
        }
        let (l, r) = (
            serde_json::to_string(&left).unwrap_or_default(),
            serde_json::to_string(&right).unwrap_or_default(),
        );
        if let Some(tree) = merge_columns(&l, &r, 186.0, 208.0, 1280.0) {
            app.dock = tree;
        }
    }
    // Page cells small enough that all three fit the Pages leaf (the palette's
    // own size control, which `ui.txt` may have left anywhere).
    app.pages_fit = false;
    app.pages_cell_w = 95.0;
    app.new_doc_draft.setup = mn_core::PageSetup::presets().remove(0);
    app.new_doc_draft.pages = 3;
    app.new_doc_draft.binding_right = true;
    app.new_doc_draft.frame_folder = true;
    app.new_doc_draft.story = String::new();
    dispatch(app, AppCmd::NewComicCreate);
    // New Comic parks the startup canvas in a tab of its own; the README
    // wants one document, not a spare empty one beside it.
    app.close_doc(0);

    // Divide the BORDER, not into new folders: one frame folder holding
    // every panel is what a page cut up in one sitting looks like, and it
    // keeps the Layers palette readable. The gutters are CSP's own defaults
    // (2.96 mm between columns, 9.74 mm between rows) — the Frame tool's
    // per-sub-tool Tool Property, set here because the border sub tool's
    // remembered pair is tighter than a printed page reads at fit zoom.
    app.frame_mode = FrameMode::DivideBorder;
    app.gutter_border_mm = (2.96, 9.74);
    let Some([x0, y0, x1, y1]) = app.page.as_ref().map(|p| p.inner_rect_px()) else {
        return;
    };
    let (w, h) = (x1 - x0, y1 - y0);
    let at = |fx: f32, fy: f32| (x0 + w * fx, y0 + h * fy);
    // Tiers first (each cut segment runs past the paper edge, like a drag
    // that starts and ends off the panel), then one vertical cut inside
    // each of the lower two tiers — the cut only splits frames its SEGMENT
    // touches, so the tier above is untouched.
    let (row1, row2) = (0.30f32, 0.63f32);
    for fy in [row1, row2] {
        let (_, y) = at(0.0, fy);
        dispatch(
            app,
            AppCmd::FrameDivide {
                a: (x0 - w * 0.1, y),
                b: (x1 + w * 0.1, y),
            },
        );
    }
    for (fx, top, bottom) in [(0.45f32, row1, row2), (0.58, row2, 1.0)] {
        let (x, _) = at(fx, 0.0);
        dispatch(
            app,
            AppCmd::FrameDivide {
                a: (x, at(0.0, top).1 + h * 0.02),
                b: (x, at(0.0, bottom).1 - h * 0.02),
            },
        );
    }
    // Then select the folder's draw layer, the way you do before inking:
    // with a FRAME layer active the canvas wears the frame-focus veil (a
    // blue wash over everything outside the panels — `--shot-framefocus` is
    // the shot that exists to show it), and the README wants white paper.
    // `add_frame_folder` pushes White, draw, header, so the draw layer is
    // the row directly under the header.
    if let Some(fi) = app.doc.layers.iter().rposition(|l| l.folder && l.is_frame()) {
        app.doc.set_active(fi.saturating_sub(1));
    }
    // The ACTIVE page's Pages-panel thumbnail is minted at the head of
    // `App::render`, which this harness never runs — without these two lines
    // (render's own) page 1 shows the grey "editing" placeholder while the
    // parked pages show their stashed thumbs.
    let thumb = app.thumb_of_current();
    app.pages[app.page_index].thumb = Some(thumb);
    // A frozen coaching line ("divided into…") reads as noise under a
    // README title.
    app.set_status(String::new());
}

/// A few strokes so the canvas is not blank in the shot (and the tile upload +
/// compositing path is exercised). Coordinates are CANVAS space; they go
/// through `viewport.to_screen` because `push_batch` converts screen→canvas
/// itself (the live WM_POINTER path hands it client-space history batches).
fn demo_strokes(app: &mut App) {
    app.engine_mut().set_color([0.05, 0.05, 0.08]);
    let vp = app.viewport;
    for (i, y0) in [220.0f32, 300.0, 380.0].into_iter().enumerate() {
        app.begin_stroke(PointerKind::Mouse);
        let batch: Vec<PenSample> = (0..80)
            .map(|k| {
                let t = k as f32 / 79.0;
                let (x, y) =
                    vp.to_screen(320.0 + t * 480.0, y0 + (t * 6.0 + i as f32).sin() * 34.0);
                PenSample {
                    x,
                    y,
                    pressure: (0.15 + t * 0.85).min(1.0),
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_ms: k as f64 * 4.0,
                }
            })
            .collect();
        // Feed it the way the message loop does — several small batches with a
        // GPU dab flush between them — instead of one 80-sample push. A single
        // batch means a single flush, which is exactly the case that hides
        // multi-flush GPU dab bugs (and the CPU path is unaffected:
        // `flush_gpu_dabs` returns immediately when no dab stroke is open).
        for chunk in batch.chunks(9) {
            app.push_batch(chunk);
            app.flush_gpu_dabs();
        }
        app.end_stroke();
    }
    let c = app.active_color();
    app.engine_mut().set_color(c);
}

/// A cheap content signature of the active layer (tile set + folded pixel
/// values), order-independent over the tile HashMap — used by the transform
/// e2e check to prove undo restores exactly.
fn tile_sig(app: &App) -> u64 {
    let mut h: u64 = 0;
    let mut n: u64 = 0;
    for (idx, t) in app.doc.active_layer().tiles() {
        n += 1;
        let mut th: u64 = (idx.x as u64) * 31 + (idx.y as u64);
        for y in 0..64 {
            for x in 0..64 {
                let p = t.pixel(x, y);
                th = th.rotate_left(7) ^ (p[0] as u64) ^ ((p[3] as u64) << 32);
            }
        }
        h ^= th;
    }
    h ^ (n << 48)
}

/// `--e2e-dockdrag`: drive the REAL pointer path (Shell events + full egui
/// passes, exactly what WM_MOUSEMOVE/UP do live) through the docking system's
/// drag interactions and print verdicts:
///   1. tear-off — drag a palette tab out over the CANVAS, release: a
///      floating window surface must appear;
///   2. regroup — drag a tab onto a sibling palette's body: they must share
///      a node afterwards;
///   3. move — drag a floating window's tab bar: its bounds must follow.
/// Round-21 shipped `--shot-dock` driving the DockState API directly, which
/// proved rendering but never the pointer pipeline — this closes that gap.
pub fn dockdrag_e2e(cfg: GpuConfig) -> Result<(), String> {
    use crate::ui::dock::{Palette, Pane};
    let renderer = Renderer::new_headless(cfg).map_err(|e| e.to_string())?;
    let (w, h) = (1280u32, 860u32);
    let mut app = App::new(renderer, (w, h), 1.0);
    // Default layout regardless of any ui.txt beside the exe.
    app.dock = crate::ui::dock::default_tree();
    frame(&mut app, w, h);
    frame(&mut app, w, h);

    let leaf_of = |app: &App, p: Pane| -> Option<egui::Rect> {
        app.dock
            .iter_all_tabs()
            .find(|(_, t)| **t == p)
            .and_then(|(path, _)| app.dock[path.node_path()].rect())
    };

    // A safe press point on a tab's TITLE (left end — the × close button owns
    // the right end, and the leaf's collapse button sits before the first
    // tab). Reads the tab's real interact rect back from egui, so the aim
    // follows whatever chrome precedes it.
    let tab_point = |app: &App, p: Pane| -> Option<egui::Pos2> {
        let path = app
            .dock
            .iter_all_tabs()
            .find(|(_, t)| **t == p)
            .map(|(p, _)| p)?;
        let id = egui::Id::new("mn.dock")
            .with((path.surface, "surface"))
            .with((path.node, "node"))
            .with((path.tab, "tab"));
        let r = app.shell.ctx.read_response(id)?.rect;
        Some(egui::pos2(r.left() + 10.0, r.center().y))
    };
    let tool = Pane::Palette(Palette::Tool);
    let layers = Pane::Palette(Palette::Layers);

    // --- 1. tear-off over the canvas --------------------------------------
    // Docking 2: the canvas is a LEAF now, so this release resolves as a
    // tab-insert into it — which patch #16 vetoes and rewrites into a
    // floating window at the pointer. Same gesture, same outcome as ever.
    let canvas_rect = leaf_of(&app, Pane::Canvas).expect("canvas leaf rect");
    let press = tab_point(&app, tool).expect("Tool tab press point");
    drag(&mut app, w, h, press, canvas_rect.center(), 28);
    let torn = app.dock.surfaces_count() > 1;
    println!(
        "[e2e] dock tear-off over canvas: {torn} (surfaces={})",
        app.dock.surfaces_count()
    );

    // --- 2. regroup: Tool tab onto the Sub Tool palette --------------------
    // (reset first so test 1's outcome cannot mask test 2)
    app.dock = crate::ui::dock::default_tree();
    frame(&mut app, w, h);
    let sub = leaf_of(&app, Pane::Palette(Palette::SubTool)).expect("Sub Tool leaf rect");
    let press = tab_point(&app, tool).expect("Tool tab press point");
    // The icon overlay's CENTER button (Append) sits at the hovered leaf's
    // centre — releasing on the leaf body but off-button tears off instead.
    drag(&mut app, w, h, press, sub.center(), 20);
    let grouped = {
        let sub_node = app
            .dock
            .iter_all_tabs()
            .find(|(_, t)| **t == Pane::Palette(Palette::SubTool))
            .map(|(p, _)| p.node_path())
            .expect("Sub Tool node");
        let tabs: Vec<Pane> = app.dock[sub_node]
            .tabs()
            .map(|t| t.to_vec())
            .unwrap_or_default();
        println!("[e2e] dock regroup: sub-tool node tabs = {tabs:?}");
        tabs.contains(&tool)
    };
    println!("[e2e] dock regroup onto sibling: {grouped}");

    // --- 3. move a floating window by its tab bar ---------------------------
    let si = {
        let path = app
            .dock
            .iter_all_tabs()
            .find(|(_, t)| **t == layers)
            .map(|(p, _)| p)
            .expect("Layers tab");
        app.dock.remove_tab(path);
        app.dock.add_window(vec![layers])
    };
    if let Some(ws) = app.dock.get_window_state_mut(si) {
        ws.set_position(egui::pos2(560.0, 260.0));
        ws.set_size(egui::vec2(250.0, 360.0));
    }
    frame(&mut app, w, h);
    frame(&mut app, w, h);
    // The floating surface's rect (found dynamically: a drag detaches the
    // tab into a NEW surface, so `si` can go stale across the gesture).
    let float_rect = |app: &App| -> egui::Rect {
        let path = app
            .dock
            .iter_all_tabs()
            .find(|(_, t)| **t == layers)
            .map(|(p, _)| p)
            .expect("floating Layers tab");
        app.dock[path.node_path()].rect().expect("floating rect")
    };
    let before = float_rect(&app);
    let press = tab_point(&app, layers).expect("floating Layers tab point");
    let drop = egui::pos2(press.x + 90.0, press.y + 50.0);
    drag(&mut app, w, h, press, drop, 8);
    let after = float_rect(&app);
    let moved =
        (after.min.x - before.min.x).abs() > 20.0 || (after.min.y - before.min.y).abs() > 20.0;
    println!(
        "[e2e] dock move floating window: {moved} ({:?} -> {:?})",
        before.min, after.min
    );

    // --- 4. the canvas pane never leaves the main surface -------------------
    // Drag the Canvas tab and release over a palette leaf (vetoed insert, no
    // float — the canvas may not live in a window) and then over the status
    // bar (patch #3's no-leaf fallback, also barred). Both must snap back.
    app.dock = crate::ui::dock::default_tree();
    frame(&mut app, w, h);
    frame(&mut app, w, h);
    let surfaces_before = app.dock.surfaces_count();
    let layers_rect = leaf_of(&app, layers).expect("Layers leaf rect");
    let press = tab_point(&app, Pane::Canvas).expect("Canvas tab press point");
    drag(&mut app, w, h, press, layers_rect.center(), 24);
    let press = tab_point(&app, Pane::Canvas).expect("Canvas tab still docked");
    drag(&mut app, w, h, press, egui::pos2(640.0, h as f32 - 4.0), 24);
    let canvas_docked = app
        .dock
        .iter_all_tabs()
        .any(|(path, t)| *t == Pane::Canvas && path.surface.is_main())
        && app.dock.surfaces_count() == surfaces_before;
    println!("[e2e] canvas pane never floats: {canvas_docked}");

    verdicts(&[
        ("tear-off over canvas", torn),
        ("regroup onto sibling", grouped),
        ("move floating window", moved),
        ("canvas pane never floats", canvas_docked),
    ])
}

/// Fail the run when any named claim came out false.
///
/// These harnesses used to PRINT `name=true/false` and then return `Ok(())`
/// whatever happened, so a regression showed up as one word in a wall of
/// output and an exit code of zero. Anything that can only be checked by
/// driving the real pointer/command path is exactly the coverage you cannot
/// afford to have lying to you.
fn verdicts(claims: &[(&str, bool)]) -> Result<(), String> {
    let failed: Vec<&str> = claims
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(name, _)| *name)
        .collect();
    if failed.is_empty() {
        println!("[e2e] ALL {} verdicts true", claims.len());
        return Ok(());
    }
    Err(format!(
        "{} of {} e2e verdicts FAILED: {}",
        failed.len(),
        claims.len(),
        failed.join(", ")
    ))
}

/// One full egui pass, exactly like a live WM_PAINT frame. The pass output is
/// deliberately not painted — this harness drives logic, not pixels — so the
/// texture deltas are marked handled (epaint panics on dropped deltas).
fn frame(app: &mut App, w: u32, h: u32) {
    let ctx = app.shell.ctx.clone();
    let raw = app.shell.begin((w, h));
    let mut out = ctx.run_ui(raw, |ui| crate::ui::build(ui, app));
    app.shell.end(&out);
    out.textures_delta.clear();
}

/// Press at `from`, drag in `steps` frames to `to`, release — the event
/// sequence the live mouse path produces.
fn drag(app: &mut App, w: u32, h: u32, from: egui::Pos2, to: egui::Pos2, steps: usize) {
    app.shell.on_pointer_button(
        from.x as i32,
        from.y as i32,
        egui::PointerButton::Primary,
        true,
    );
    frame(app, w, h);
    for k in 1..=steps {
        let t = k as f32 / steps as f32;
        let p = egui::pos2(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        app.shell.on_pointer_moved(p.x as i32, p.y as i32);
        frame(app, w, h);
    }
    app.shell.on_pointer_button(
        to.x as i32,
        to.y as i32,
        egui::PointerButton::Primary,
        false,
    );
    frame(app, w, h);
}

/// `--e2e-paneresize`: drive the REAL pointer path against the dock-column
/// resize edges — the owner's report: after dragging a column wider, merely
/// hovering the mouse over the app made the column "animatedly gradually
/// narrower", and the resize cursor only appeared on an exact hit while a
/// white line highlighted the edge otherwise. Verdicts:
///   1. drag-wider — the column width follows the pointer;
///   2. hover-stability — 40 hover frames anywhere over the app must not move
///      the released width by more than 0.5pt;
///   3. cursor-band — the resize cursor shows across the whole hit band.
pub fn paneresize_e2e(cfg: GpuConfig) -> Result<(), String> {
    let renderer = Renderer::new_headless(cfg).map_err(|e| e.to_string())?;
    let (w, h) = (1280u32, 860u32);
    let mut app = App::new(renderer, (w, h), 1.0);
    // Default layout regardless of any ui.txt beside the exe, then reconcile
    // the Pages tab with the (plain-image) doc, exactly like the app does.
    app.dock = crate::ui::dock::default_tree();
    app.sync_pages_palette();
    frame(&mut app, w, h);
    frame(&mut app, w, h);

    // Docking 2: column resizing is egui_dock's separator between the left
    // column's nodes and the canvas leaf. The Tool leaf's right edge sits on
    // that separator; its width is the measure.
    let tool_w = |app: &App| -> f32 {
        app.dock
            .iter_all_tabs()
            .find(|(_, t)| **t == crate::ui::dock::Pane::Palette(crate::ui::dock::Palette::Tool))
            .and_then(|(path, _)| app.dock[path.node_path()].rect())
            .map_or(0.0, |r| r.width())
    };

    // --- 1. drag the column/canvas separator wider --------------------------
    let before = tool_w(&app);
    let edge = egui::pos2(before + 2.0, 300.0);
    drag(&mut app, w, h, edge, egui::pos2(before + 62.0, 300.0), 14);
    frame(&mut app, w, h);
    let after = tool_w(&app);
    let widened = after > before + 40.0;
    println!("[e2e] pane drag-wider: {widened} ({before:.1} -> {after:.1})");

    // --- 2. hover all over the app; the width must hold ---------------------
    let released = tool_w(&app);
    let spots = [
        egui::pos2(640.0, 430.0),           // canvas centre
        egui::pos2(90.0, 120.0),            // over the Tool palette
        egui::pos2(640.0, 60.0),            // menu bar
        egui::pos2(released + 30.0, 300.0), // just right of the edge
        egui::pos2(1200.0, 400.0),          // right column
        egui::pos2(640.0, 830.0),           // status bar
        egui::pos2(90.0, 620.0),            // Pages area
    ];
    let mut min_w = released;
    let mut max_w = released;
    for k in 0..40 {
        let p = spots[k % spots.len()];
        app.shell.on_pointer_moved(p.x as i32, p.y as i32);
        frame(&mut app, w, h);
        min_w = min_w.min(tool_w(&app));
        max_w = max_w.max(tool_w(&app));
    }
    let stable = (max_w - min_w) <= 0.5 && (max_w - released) <= 0.5;
    println!(
        "[e2e] pane hover-stability: {stable} (released {released:.1}, range {min_w:.1}..{max_w:.1})"
    );

    // --- 3. cursor band across the edge -------------------------------------
    // egui_dock's separator carries `extra_interact_width` (12pt total, our
    // style) — the cursor must show across it, not only on an exact hit.
    let e = tool_w(&app) + 2.0;
    let mut band = true;
    let mut report = Vec::new();
    for dx in [-5.0f32, -2.0, 0.0, 2.0, 5.0] {
        app.shell.on_pointer_moved((e + dx) as i32, 300);
        frame(&mut app, w, h);
        let cur = app.shell.cursor;
        let is_resize = matches!(
            cur,
            egui::CursorIcon::ResizeHorizontal
                | egui::CursorIcon::ResizeEast
                | egui::CursorIcon::ResizeWest
                | egui::CursorIcon::ResizeColumn
        );
        report.push(format!("{dx:+.0}pt={:?}", cur));
        if !is_resize {
            band = false;
        }
    }
    println!(
        "[e2e] pane cursor-band ±5pt: {band} ({})",
        report.join(", ")
    );

    // Verdicts from the scoped blocks below, hoisted so the final check can
    // see them — a block-local claim is a claim nothing enforces.
    let page_claims: Vec<(&str, bool)>;
    let tool_claims: Vec<(&str, bool)>;

    // --- 4. the Pages palette follows the document --------------------------
    // Startup doc is a plain image: closed. Adding a page makes it a manga
    // project: open again (the owner's "it's not a manga" report).
    {
        use crate::cmd::{AppCmd, dispatch};
        let pages_open = |app: &App| crate::ui::dock::is_open(app, crate::ui::dock::Palette::Pages);
        let image_case = !pages_open(&app);
        dispatch(&mut app, AppCmd::AddPage);
        let manga_case = pages_open(&app);
        println!(
            "[e2e] pages-palette follows doc: image-closed={image_case} manga-open={manga_case}"
        );
        page_claims = vec![
            ("pages palette closed on a plain image", image_case),
            ("pages palette open on a manga", manga_case),
        ];
    }

    // --- 5. Figure + Gradient ink through the real finishers ----------------
    {
        use crate::cmd::{AppCmd, FigureMode, GradMode, Tool, dispatch};
        dispatch(&mut app, AppCmd::SetTool(Tool::Figure));
        app.figure_mode = FigureMode::Rect;
        let before = tile_sig(&app);
        app.finish_figure_drag((400.0, 300.0), (520.0, 380.0));
        let after_fig = tile_sig(&app);
        dispatch(&mut app, AppCmd::SetTool(Tool::Gradient));
        app.grad_mode = GradMode::FgToBg;
        app.finish_gradient((100.0, 100.0), (300.0, 100.0));
        let after_grad = tile_sig(&app);
        // And one undo returns to the pre-gradient state (each op is a step).
        dispatch(&mut app, AppCmd::Undo);
        let undone = tile_sig(&app);
        println!(
            "[e2e] figure inks={} gradient paints={} gradient-undo-restores={}",
            after_fig != before,
            after_grad != after_fig,
            undone == after_fig
        );
        tool_claims = vec![
            ("figure tool inks", after_fig != before),
            ("gradient paints", after_grad != after_fig),
            ("gradient undo restores", undone == after_fig),
        ];
    }

    let mut claims = vec![
        ("pane drag-wider", widened),
        ("pane hover-stability", stable),
        ("pane cursor-band", band),
    ];
    claims.extend(page_claims);
    claims.extend(tool_claims);
    verdicts(&claims)
}

/// `--e2e-workfolder`: drive the REAL command path through the whole
/// work-folder storage story — create a comic, save it as a work folder, edit/add
/// pages, re-save incrementally, reopen, autosave in place, and export
/// a single-file copy. Prints `[e2e]` lines with true/false verdicts; the whole
/// point is that every claim about the format is checked, not asserted.
pub fn workfolder_e2e(cfg: GpuConfig) -> Result<(), String> {
    let renderer = Renderer::new_headless(cfg).map_err(|e| e.to_string())?;
    let mut app = App::new(renderer, (1280, 860), 1.0);
    use crate::cmd::{AppCmd, dispatch};

    let dir = std::env::temp_dir().join("manganakama-e2e-workfolder");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let index = dir.join(mn_core::project::WORKFOLDER_INDEX);

    // 1. A 3-page comic with a story (is_comic via metadata).
    app.new_doc_draft.pages = 3;
    app.new_doc_draft.story = "E2E".into();
    dispatch(&mut app, AppCmd::NewComicCreate);
    draw_scribble(&mut app);

    // 2. First save: work.mnc + three page files.
    dispatch(&mut app, AppCmd::SaveOraPath(index.clone()));
    let files = || page_files(&dir);
    let f1 = files();
    let h1: Vec<u64> = f1.iter().map(|f| hash_file(&dir.join(f))).collect();
    let three = f1.len() == 3;
    let sniff = mn_core::project::sniff_kind(&index) == mn_core::project::MncKind::WorkFolderIndex;
    println!("[e2e] first-save: files={f1:?} three={three} sniff={sniff}");

    // 3. Add a page + edit another one, then re-save: only the edited page's
    //    file and the new page's file change; the other two stay byte-identical.
    dispatch(&mut app, AppCmd::AddPage);
    dispatch(&mut app, AppCmd::SelectPage(2));
    draw_scribble(&mut app);
    dispatch(&mut app, AppCmd::SaveOraPath(index.clone()));
    let f2 = files();
    let h2: Vec<u64> = f2.iter().map(|f| hash_file(&dir.join(f))).collect();
    let unchanged = f1
        .iter()
        .zip(&h1)
        .filter(|(f, h)| f2.iter().zip(&h2).any(|(g, nh)| g == *f && nh == *h))
        .count();
    let four = f2.len() == 4;
    println!(
        "[e2e] incremental: four={four} untouched-pages-byte-identical={} (expect 2 of 3)",
        unchanged == 2
    );

    // 4. Reopen: page count + story survive, and a clean re-save rewrites
    //    NOTHING (rev bookkeeping survived the round-trip).
    dispatch(&mut app, AppCmd::OpenOraPath(index.clone()));
    let reopened = app.pages.len() == 4 && app.story == "E2E";
    let h3_pre: Vec<u64> = f2.iter().map(|f| hash_file(&dir.join(f))).collect();
    dispatch(&mut app, AppCmd::SaveOraPath(index.clone()));
    let quiet = f2
        .iter()
        .zip(&h3_pre)
        .all(|(f, h)| hash_file(&dir.join(f)) == *h);
    println!("[e2e] reopen: pages-and-story={reopened} clean-resave-rewrites-nothing={quiet}");

    // 5. Autosave (in place): a dirty page file changes on disk, the untouched
    //    ones do not.
    draw_scribble(&mut app);
    let before: Vec<u64> = f2.iter().map(|f| hash_file(&dir.join(f))).collect();
    dispatch(&mut app, AppCmd::Autosave);
    let after: Vec<u64> = f2.iter().map(|f| hash_file(&dir.join(f))).collect();
    let touched = before.iter().zip(&after).filter(|(a, b)| a != b).count();
    println!(
        "[e2e] autosave-in-place: one-dirty-page-rewritten={} untouched={}",
        touched == 1,
        before.len() - touched
    );

    // 6. Single-file export: Comic flavour, same page count, doc_path still
    //    the work folder.
    let copy = dir.join("copy.mnc");
    dispatch(&mut app, AppCmd::ExportMncPath(copy.clone()));
    let proj = mn_core::project::load(&copy).map_err(|e| e.to_string())?;
    let export = proj.pages.len() == 4
        && mn_core::project::sniff_kind(&copy) == mn_core::project::MncKind::Comic;
    let still_folder = app.doc_path.as_deref() == Some(index.as_path());
    println!(
        "[e2e] export-single-file: pages-and-sniff={export} doc-path-untouched={still_folder}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    verdicts(&[
        ("first save writes three page files", three),
        ("work.mnc sniffs as a folder index", sniff),
        ("adding a page makes four", four),
        ("untouched pages stay byte-identical", unchanged == 2),
        ("reopen keeps pages and story", reopened),
        ("a clean re-save rewrites nothing", quiet),
        ("autosave rewrites exactly the dirty page", touched == 1),
        ("single-file export keeps pages and kind", export),
        ("export leaves doc_path on the work folder", still_folder),
    ])
}

/// A short real stroke through the live input path (screen coords, like the
/// WM_POINTER batches).
fn draw_scribble(app: &mut App) {
    let vp = app.viewport;
    app.begin_stroke(PointerKind::Mouse);
    let batch: Vec<PenSample> = (0..16)
        .map(|k| {
            let t = k as f32 / 15.0;
            let (x, y) = vp.to_screen(500.0 + t * 300.0, 500.0 + (t * 7.0).sin() * 80.0);
            PenSample {
                x,
                y,
                pressure: 0.8,
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_ms: k as f64 * 4.0,
            }
        })
        .collect();
    app.push_batch(&batch);
    app.end_stroke();
}

fn page_files(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".ora"))
        .collect();
    v.sort();
    v
}

fn hash_file(p: &std::path::Path) -> u64 {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(p) else {
        return u64::MAX;
    };
    let mut buf = [0u8; 8192];
    let mut h: u64 = 0xcbf29ce484222325;
    while let Ok(n) = f.read(&mut buf) {
        if n == 0 {
            break;
        }
        for b in &buf[..n] {
            h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
        }
    }
    h
}

fn capture(
    app: &mut App,
    w: u32,
    h: u32,
    // Fit the page to the canvas pane once the dock has laid it out (hero
    // shots) — done INSIDE the pass loop because a pass run outside this
    // pipeline would drop its texture deltas and poison the painter.
    fit_after_layout: bool,
) -> Result<image::RgbaImage, String> {
    // The UI, painted into a transparent texture of the same size. The
    // CANVAS renders AFTER these passes (docking 2): the canvas rect — and
    // with it any deferred fit — is only known once the dock tree has laid
    // the canvas pane out, so a canvas rendered first used a viewport the
    // first pass was about to move.
    // Cloned handles (wgpu's are refcounted): the UI closure needs `&mut app`,
    // so a borrow of `app.renderer` may not be alive across it.
    let device = app.renderer.device().clone();
    let queue = app.renderer.queue().clone();
    let (device, queue) = (&device, &queue);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mn.shot.ui"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Must match what the egui painter was built for (the headless
        // renderer reports the canvas format).
        format: app.renderer.output_format(),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // Several passes with real time in between: egui only knows a brand-new
    // window's size on the pass *after* it first appears, and then fades/scales
    // it in over ~0.2 s. A live app repaints through that (egui asks for it via
    // `repaint_delay`); a one-frame screenshot would catch a ghost. The extra
    // quick passes let the budgeted brush previews (1/frame) fill in.
    // Hero shots run extra passes: brush previews trickle one per frame
    // (ui::build resets the budget), and a strip with half its previews
    // missing reads as broken in a README.
    let passes = if fit_after_layout { 48 } else { 16 };
    for pass in 0..passes {
        if pass > 0 && pass < 3 {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        if pass == 3 && fit_after_layout {
            app.fit_to_view();
        }
        let ctx = app.shell.ctx.clone();
        let raw = app.shell.begin((w, h));
        let mut out = ctx.run_ui(raw, |ui| crate::ui::build(ui, app));
        app.shell.end(&out);
        let jobs = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mn.shot"),
        });
        // Clear to transparent; `Shell::paint` loads (it normally draws over
        // the canvas that is already in the swapchain).
        drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mn.shot.clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }));
        app.shell.paint(
            device,
            queue,
            &mut enc,
            &view,
            (w, h),
            &jobs,
            &mut out.textures_delta,
        );
        queue.submit([enc.finish()]);
        app.shell.free(&mut out.textures_delta);
    }

    // The canvas, through the compositor's own offscreen path — the app's
    // now-settled viewport, so the egui overlay (shadow, guides) lines up.
    let vp = app.viewport;
    let mut canvas = app.renderer.render_offscreen_vp(&app.doc, &vp, w, h);
    // Debug: the pure composited present, before the egui overlay — splits
    // "canvas/mip divergence" from "HUD/overlay text differs by design".
    if std::env::var("MN_DUMP_PRESENT").is_ok() {
        let _ = canvas.save("present-dump.png");
    }

    // Read back and composite (egui output is premultiplied).
    let ui_px = read_rgba(device, queue, &target, w, h);
    let bgra = matches!(
        app.renderer.output_format(),
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    for (dst, src) in canvas.chunks_exact_mut(4).zip(ui_px.chunks_exact(4)) {
        let (r, g, b, a) = if bgra {
            (src[2], src[1], src[0], src[3])
        } else {
            (src[0], src[1], src[2], src[3])
        };
        let inv = 255 - a as u32;
        for (i, s) in [r, g, b].into_iter().enumerate() {
            dst[i] = (s as u32 + (dst[i] as u32 * inv + 127) / 255).min(255) as u8;
        }
        dst[3] = 255;
    }
    Ok(canvas)
}

fn read_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    w: u32,
    h: u32,
) -> Vec<u8> {
    const ALIGN: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let unpadded = w * 4;
    let padded = unpadded.div_ceil(ALIGN) * ALIGN;

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mn.shot.readback"),
        size: (padded * h) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("mn.shot.copy"),
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);

    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    let mut out = vec![0u8; (unpadded * h) as usize];
    {
        let view = slice.get_mapped_range().expect("map readback buffer");
        for y in 0..h {
            let src = (y * padded) as usize;
            let dst = (y * unpadded) as usize;
            out[dst..dst + unpadded as usize].copy_from_slice(&view[src..src + unpadded as usize]);
        }
    }
    buffer.unmap();
    out
}
