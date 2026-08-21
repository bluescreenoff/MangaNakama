//! The References palette (owner ask, 2026-08-21: "a pane with a list of
//! reference images and panes each with one of those reference images open …
//! and can also be free windows ofc"). The palette is the LIST — a dock tab
//! like every other; each viewer is a free-floating `egui::Window` that pans,
//! zooms and picks colour. Viewers are windows rather than dock tabs on
//! purpose: an artist wants three references over the canvas at once, and a
//! dock tab can only show one of a stack.
//!
//! Where the two halves live:
//!
//! - The PATH LIST persists (`ui.txt` `references=`, one escaped JSON line),
//!   so a reference board survives a restart. A path whose file is gone shows
//!   a MISSING placeholder and stays in the list: renaming a reference folder
//!   is the community's own complaint about tools that silently drop them, and
//!   a row you can see is a row you can re-add or forget on purpose.
//! - The open VIEWERS do not persist. Windows that reopen themselves over the
//!   canvas at every launch are a nuisance, not a feature.
//!
//! Memory (the owner draws on a UHD 620 with 16 GB of SHARED RAM, so VRAM is
//! that RAM): nothing here keeps a full-resolution CPU buffer alive next to
//! its texture. Thumbnails cap at [`THUMB_CAP`], a viewer's texture at
//! [`FULL_CAP`], and the decoded buffer is dropped the moment it is uploaded.
//! The only CPU pixels kept are the eyedropper's PICK buffer, capped at
//! [`PICK_CAP`] on the long edge and downscaled with NEAREST — every value in
//! it is a real pixel of the file, never a blend the picker would have
//! invented. And exactly ONE image decodes per frame (`RefBank::budget`, the
//! `preview_budget` rule): a board of forty references trickles in instead of
//! hitching the frame that first shows it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::app::App;
use crate::cmd::AppCmd;

use super::theme;

/// Thumbnail long edge, px (the list rows draw it at `THUMB_CELL` points).
pub const THUMB_CAP: u32 = 96;
/// A viewer texture's long edge, px. A 6000 px photo is a 4096 px texture.
pub const FULL_CAP: u32 = 4096;
/// The eyedropper's CPU copy: ≤ 4 MB per open viewer, and nearest-sampled so
/// the colours in it are the file's own.
const PICK_CAP: u32 = 1024;

const THUMB_CELL: f32 = 48.0;
const ZOOM_MIN: f32 = 0.05;
const ZOOM_MAX: f32 = 32.0;

/// A loaded thumbnail. `missing` is sticky: an entry in the map means the load
/// was already ATTEMPTED, so a gone file costs one decode attempt, not one per
/// frame forever.
pub struct ThumbTex {
    pub tex: egui::TextureHandle,
    pub missing: bool,
}

/// A viewer's texture plus the small CPU copy the eyedropper reads.
pub struct RefTex {
    pub tex: egui::TextureHandle,
    pub w: f32,
    pub h: f32,
    /// Nearest-downscaled (≤ [`PICK_CAP`]) — see the module header.
    pick: image::RgbaImage,
}

/// One open viewer window. Session-only.
pub struct RefView {
    pub path: PathBuf,
    /// A multiplier ON TOP of fit-to-window, so 1.0 always means "fits",
    /// whatever the window has been resized to since.
    pub zoom: f32,
    pub pan: egui::Vec2,
    /// The eyedropper toggle. Alt+click picks regardless, the canvas rule.
    pub pick: bool,
}

/// Every bit of References state, in one App field.
#[derive(Default)]
pub struct RefBank {
    /// The persisted list, in the order the user added them.
    pub paths: Vec<PathBuf>,
    pub thumbs: HashMap<PathBuf, ThumbTex>,
    /// `None` = tried and unreadable (missing file, or a decode we refuse).
    pub full: HashMap<PathBuf, Option<RefTex>>,
    pub views: Vec<RefView>,
    /// Decodes left this frame (`ui::build` sets it to 1).
    pub budget: u32,
    /// A viewer to raise on the next pass (clicking a row whose window is
    /// already open behind another one must not read as a dead click).
    pub focus: Option<PathBuf>,
    /// Set by the palette's button, consumed by `main::pump_commands`: the
    /// file dialog must NOT open from inside the UI build. `App::render` runs
    /// inside the wndproc, and a dialog pumps the message queue — which
    /// re-enters the wndproc while a `&mut App` is alive up the stack
    /// (docs/ARCHITECTURE.md). The queue-and-resolve-later shape is the one
    /// `pump_commands` already uses for every other dialog.
    pub want_add: bool,
}

impl RefBank {
    /// Seed from the persisted `references=` line.
    pub fn from_layout(paths: &[String]) -> Self {
        Self {
            paths: paths.iter().map(PathBuf::from).collect(),
            ..Self::default()
        }
    }

    /// Add picked files (the dialog's answer, from `main::pump_commands`).
    /// Duplicates are ignored — the same photo twice is two rows that forget
    /// each other's viewer.
    pub fn add(&mut self, files: Vec<PathBuf>) -> usize {
        let before = self.paths.len();
        for p in files {
            if !self.paths.contains(&p) {
                self.paths.push(p);
            }
        }
        self.paths.len() - before
    }

    /// The persisted form: absolute paths, newest last.
    pub fn to_lines(&self) -> Vec<String> {
        self.paths
            .iter()
            .map(|p| p.display().to_string())
            .collect()
    }
}

/// Remember the list in `ui.txt` (call after every add/forget).
fn save(app: &mut App) {
    let lines = app.refs.to_lines();
    app.layout.note_references(&lines);
}

/// A viewer window's id. Two references can share a file NAME, so the path is
/// the identity — of the window and of its layer, which is what lets a click
/// on an already-open row raise it.
fn view_id(path: &Path) -> egui::Id {
    egui::Id::new(("mn.ref.view", path))
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

// --- the palette ---------------------------------------------------------

pub(super) fn references_palette(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        if ui
            .button("Add images…")
            .on_hover_text("pick reference images — the list is remembered between sessions")
            .clicked()
        {
            app.refs.want_add = true;
        }
        if !app.refs.paths.is_empty() {
            ui.weak(format!("{}", app.refs.paths.len()));
        }
    });
    ui.separator();

    if app.refs.paths.is_empty() {
        ui.weak("no references yet — Add images…, then click one to open it in its own window");
        return;
    }

    let mut open: Option<usize> = None;
    let mut forget: Option<usize> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for i in 0..app.refs.paths.len() {
                ui.horizontal(|ui| {
                    if ref_row(ui, app, i) {
                        open = Some(i);
                    }
                    if ui
                        .small_button("✕")
                        .on_hover_text("forget this reference")
                        .clicked()
                    {
                        forget = Some(i);
                    }
                });
            }
        });

    if let Some(i) = open {
        let path = app.refs.paths[i].clone();
        show_viewer(app, path);
    }
    if let Some(i) = forget {
        let path = app.refs.paths.remove(i);
        app.refs.thumbs.remove(&path);
        app.refs.full.remove(&path);
        app.refs.views.retain(|v| v.path != path);
        save(app);
    }
}

/// One list row: the thumbnail and the file name. Returns true when clicked.
fn ref_row(ui: &mut egui::Ui, app: &mut App, i: usize) -> bool {
    let path = app.refs.paths[i].clone();
    let name = file_label(&path);
    let entry = app.refs.thumbs.get(&path);
    let (tex, missing) = match entry {
        Some(t) => (Some(t.tex.clone()), t.missing),
        None => (None, false),
    };

    let label = if missing {
        egui::RichText::new(name.clone()).small().color(theme::WARN)
    } else {
        egui::RichText::new(name.clone()).small()
    };
    let btn = match &tex {
        Some(t) => egui::Button::image_and_text(
            egui::Image::from_texture(t).max_size(egui::vec2(THUMB_CELL, THUMB_CELL)),
            label,
        ),
        None => egui::Button::new(label),
    };
    let hover = if missing {
        format!(
            "{} is missing — renamed, moved, or on a drive that is not mounted.\n\
             The row stays until you forget it with ✕.",
            path.display()
        )
    } else {
        format!("{}\nclick: open in its own window", path.display())
    };
    let resp = ui.add(btn).on_hover_text(hover);

    // Lazy, and at most one decode per frame for the whole palette.
    if tex.is_none() && ui.is_rect_visible(resp.rect) && app.refs.budget > 0 {
        app.refs.budget -= 1;
        let (img, missing) = thumb_image(&path, THUMB_CAP);
        let tex = app.shell.ctx.load_texture(
            format!("mn.ref.thumb.{}", path.display()),
            img,
            egui::TextureOptions::LINEAR,
        );
        app.refs.thumbs.insert(path.clone(), ThumbTex { tex, missing });
    }
    resp.clicked()
}

/// Open (or focus) the viewer for `path`.
fn show_viewer(app: &mut App, path: PathBuf) {
    if app.refs.views.iter().any(|v| v.path == path) {
        app.set_status(format!("{} is already open", file_label(&path)));
        app.refs.focus = Some(path);
        return;
    }
    app.refs.views.push(RefView {
        path,
        zoom: 1.0,
        pan: egui::Vec2::ZERO,
        pick: false,
    });
}

// --- the viewers ---------------------------------------------------------

/// Every open reference viewer. Called from `ui::build` after the palettes,
/// so a viewer floats over the dock columns like the other free windows.
pub(super) fn reference_windows(ctx: &egui::Context, app: &mut App) {
    // A viewer texture (and its pick buffer) belongs to its window: anything
    // with no window open is freed HERE, which is the one door every way of
    // closing a viewer goes through — the window ✕, the palette's forget ✕,
    // and a viewer dropped by any future path. A leak on this seam is a leak
    // of up to 64 MB of shared RAM per reference on the owner's machine.
    if app.refs.full.len() > app.refs.views.len() {
        let open: Vec<PathBuf> = app.refs.views.iter().map(|v| v.path.clone()).collect();
        app.refs.full.retain(|p, _| open.contains(p));
    }
    if app.refs.views.is_empty() {
        app.refs.focus = None;
        return;
    }
    if let Some(p) = app.refs.focus.take() {
        ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, view_id(&p)));
    }
    // The views are swapped out for the call: the window bodies take a
    // `&mut App`, and two mutable aliases of the same struct would not fly
    // (the `dock::column` idiom).
    let mut views = std::mem::take(&mut app.refs.views);
    let mut closed: Vec<PathBuf> = Vec::new();
    for v in &mut views {
        let mut open = true;
        egui::Window::new(file_label(&v.path))
            .id(view_id(&v.path))
            .open(&mut open)
            .default_size(egui::vec2(420.0, 440.0))
            .show(ctx, |ui| viewer_body(ui, app, v));
        if !open {
            closed.push(v.path.clone());
        }
    }
    views.retain(|v| !closed.contains(&v.path));
    // The texture goes back on the next pass, through the prune above.
    app.refs.views = views;
}

fn viewer_body(ui: &mut egui::Ui, app: &mut App, v: &mut RefView) {
    ui.horizontal(|ui| {
        if ui.small_button("−").on_hover_text("zoom out").clicked() {
            v.zoom = (v.zoom / 1.25).clamp(ZOOM_MIN, ZOOM_MAX);
        }
        if ui.small_button("＋").on_hover_text("zoom in").clicked() {
            v.zoom = (v.zoom * 1.25).clamp(ZOOM_MIN, ZOOM_MAX);
        }
        if ui
            .small_button("Fit")
            .on_hover_text("back to fit-to-window, centred")
            .clicked()
        {
            v.zoom = 1.0;
            v.pan = egui::Vec2::ZERO;
        }
        ui.toggle_value(&mut v.pick, "◉")
            .on_hover_text("eyedropper: click takes the colour under the cursor\n(Alt+click does it with this off)");
        ui.weak(format!("{:.0}%", v.zoom * 100.0));
    });

    let Some((tex, iw, ih)) = ensure_full(app, &v.path) else {
        if matches!(app.refs.full.get(&v.path), Some(None)) {
            ui.colored_label(
                theme::WARN,
                "this file is missing — renamed, moved, or on a drive that is not mounted",
            );
            ui.weak(v.path.display().to_string());
        } else {
            ui.weak("loading…");
            ui.ctx().request_repaint();
        }
        return;
    };

    let (rect, resp) = ui.allocate_exact_size(
        ui.available_size().max(egui::vec2(64.0, 64.0)),
        egui::Sense::click_and_drag(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }

    // Wheel zoom, anchored on the cursor: the image point under the pointer
    // must stay under it, or zooming into a detail walks it off screen.
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.1 {
            let old = v.zoom;
            v.zoom = (v.zoom * (scroll * 0.0015).exp()).clamp(ZOOM_MIN, ZOOM_MAX);
            if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                let k = v.zoom / old;
                v.pan += (1.0 - k) * (p - (rect.center() + v.pan));
            }
        }
    }
    if resp.dragged() {
        v.pan += resp.drag_delta();
    }

    let fit = (rect.width() / iw).min(rect.height() / ih);
    let size = egui::vec2(iw * fit * v.zoom, ih * fit * v.zoom);
    let img_rect = egui::Rect::from_center_size(rect.center() + v.pan, size);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter().with_clip_rect(rect).image(
        tex.id(),
        img_rect,
        uv,
        egui::Color32::WHITE,
    );

    let alt = ui.input(|i| i.modifiers.alt);
    if (v.pick || alt) && resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos()
            && img_rect.contains(p)
        {
            let u = (p.x - img_rect.left()) / img_rect.width();
            let w = (p.y - img_rect.top()) / img_rect.height();
            if let Some(rgb) = sample(app, &v.path, u, w) {
                let (r, g, b) = (
                    (rgb[0] * 255.0).round() as u8,
                    (rgb[1] * 255.0).round() as u8,
                    (rgb[2] * 255.0).round() as u8,
                );
                // The same door the canvas eyedropper uses (`PickColor`
                // dispatches this), so a reference pick joins the Recent
                // strip and honours the auto-swatch switch exactly like one.
                app.push_cmd(AppCmd::SetSlotColor(rgb));
                app.set_status(format!("picked #{r:02x}{g:02x}{b:02x} from {}", file_label(&v.path)));
            }
        }
    } else if v.pick || alt {
        resp.on_hover_cursor(egui::CursorIcon::Crosshair);
    } else {
        resp.on_hover_cursor(egui::CursorIcon::Grab);
    }
}

/// The colour at (u, v) in 0..1 of the file, out of the pick buffer.
fn sample(app: &App, path: &Path, u: f32, v: f32) -> Option<[f32; 3]> {
    let t = app.refs.full.get(path)?.as_ref()?;
    let (w, h) = t.pick.dimensions();
    let x = ((u * w as f32) as u32).min(w.saturating_sub(1));
    let y = ((v * h as f32) as u32).min(h.saturating_sub(1));
    let p = t.pick.get_pixel(x, y).0;
    Some([
        p[0] as f32 / 255.0,
        p[1] as f32 / 255.0,
        p[2] as f32 / 255.0,
    ])
}

/// The viewer's texture, decoding it if the frame's budget allows. `None`
/// means "not this frame" (still loading) or "unreadable" — the caller tells
/// them apart by looking for a `Some(None)` entry in the map.
fn ensure_full(app: &mut App, path: &Path) -> Option<(egui::TextureHandle, f32, f32)> {
    if let Some(entry) = app.refs.full.get(path) {
        let t = entry.as_ref()?;
        return Some((t.tex.clone(), t.w, t.h));
    }
    if app.refs.budget == 0 {
        return None;
    }
    app.refs.budget -= 1;
    let Some(img) = decode(path, FULL_CAP, image::imageops::FilterType::Triangle) else {
        app.refs.full.insert(path.to_owned(), None);
        return None;
    };
    let (w, h) = img.dimensions();
    let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
    let tex = app.shell.ctx.load_texture(
        format!("mn.ref.full.{}", path.display()),
        ci,
        egui::TextureOptions::LINEAR,
    );
    // NEAREST on purpose: a picked colour must be a pixel of the file.
    let pick = cap_long_edge(img, PICK_CAP, image::imageops::FilterType::Nearest);
    // …and the full-resolution CPU buffer dies right here, with the texture
    // uploaded: keeping both is what eats a 16 GB shared-memory machine.
    let t = RefTex {
        tex: tex.clone(),
        w: w as f32,
        h: h as f32,
        pick,
    };
    app.refs.full.insert(path.to_owned(), Some(t));
    Some((tex, w as f32, h as f32))
}

// --- decoding ------------------------------------------------------------

/// Decode `path` with its long edge capped at `cap`. `None` for a file that
/// is missing or that the `image` crate will not read.
fn decode(path: &Path, cap: u32, filter: image::imageops::FilterType) -> Option<image::RgbaImage> {
    let img = image::open(path).ok()?.to_rgba8();
    Some(cap_long_edge(img, cap, filter))
}

fn cap_long_edge(
    img: image::RgbaImage,
    cap: u32,
    filter: image::imageops::FilterType,
) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    if w.max(h) <= cap || w == 0 || h == 0 {
        return img;
    }
    let scale = cap as f32 / w.max(h) as f32;
    let tw = ((w as f32 * scale) as u32).max(1);
    let th = ((h as f32 * scale) as u32).max(1);
    image::imageops::resize(&img, tw, th, filter)
}

/// A thumbnail for `path`, and whether it is the MISSING placeholder. This
/// never fails: a renamed reference is a placeholder you can see and forget,
/// never a row that vanishes and never an error the palette has to render.
pub fn thumb_image(path: &Path, cap: u32) -> (egui::ColorImage, bool) {
    match decode(path, cap, image::imageops::FilterType::Triangle) {
        Some(img) => {
            let (w, h) = img.dimensions();
            (
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw()),
                false,
            )
        }
        None => (missing_image(cap.min(64).max(16)), true),
    }
}

/// The placeholder: a dark plate with a warning-coloured cross.
fn missing_image(size: u32) -> egui::ColorImage {
    let n = size as usize;
    let mut ci = egui::ColorImage::filled([n, n], theme::FIELD);
    let edge = (n / 5).max(2);
    for i in edge..n.saturating_sub(edge) {
        let t = (i - edge) as f32 / ((n - 2 * edge).max(1)) as f32;
        let j = edge + (t * (n - 2 * edge) as f32) as usize;
        for (x, y) in [(i, j), (i, n - 1 - j)] {
            if x < n && y < n {
                ci.pixels[y * n + x] = theme::WARN;
            }
        }
    }
    ci
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference whose file was renamed away must come back as the MISSING
    /// placeholder — not an error, not a panic, and not an empty image the
    /// palette would draw as a hole. (Failed against a `decode(..).unwrap()`
    /// loader: that one panics on the very case this feature exists for.)
    #[test]
    fn a_missing_reference_thumbnail_is_a_placeholder() {
        let gone = std::env::temp_dir().join("mn-no-such-reference-9f2c1.png");
        let _ = std::fs::remove_file(&gone);
        let (img, missing) = thumb_image(&gone, THUMB_CAP);
        assert!(missing, "a gone file must report missing");
        assert!(img.size[0] >= 16 && img.size[1] >= 16, "drawable placeholder");
        assert_eq!(img.pixels.len(), img.size[0] * img.size[1]);
        assert!(
            img.pixels.iter().any(|p| *p == theme::WARN),
            "the placeholder must be visibly marked, not a blank plate"
        );
        // A directory is not an image either, and must degrade the same way.
        let (_, missing) = thumb_image(&std::env::temp_dir(), THUMB_CAP);
        assert!(missing);
    }

    /// A real file decodes, and both caps hold — the thumbnail is small, and
    /// nothing keeps a full-resolution buffer around to get there.
    #[test]
    fn a_real_reference_decodes_capped_to_the_thumbnail_size() {
        let p = std::env::temp_dir().join("mn-reference-cap-test-9f2c1.png");
        let img = image::RgbaImage::from_pixel(400, 200, image::Rgba([12, 200, 40, 255]));
        img.save(&p).expect("write the test png");
        let (ci, missing) = thumb_image(&p, THUMB_CAP);
        assert!(!missing);
        assert_eq!(ci.size, [THUMB_CAP as usize, (THUMB_CAP / 2) as usize]);
        // The pick buffer's NEAREST downscale keeps the file's own values.
        let pick = decode(&p, PICK_CAP, image::imageops::FilterType::Nearest).unwrap();
        assert_eq!(pick.get_pixel(0, 0).0, [12, 200, 40, 255]);
        let _ = std::fs::remove_file(&p);
    }

    /// The palette and a viewer must survive real UI passes: the list row
    /// decodes its thumbnail, the viewer decodes its texture, and both do it
    /// ONE image per frame (the `preview_budget` rule) — a frame that loaded
    /// two would be the hitch the budget exists to prevent.
    #[test]
    fn the_palette_and_a_viewer_render_and_load_one_image_per_frame() {
        let Some(renderer) = crate::app::headless_renderer() else {
            return;
        };
        let dir = std::env::temp_dir();
        let mut made = Vec::new();
        for (n, c) in [(0u8, [200, 30, 40, 255]), (1, [30, 40, 200, 255])] {
            let p = dir.join(format!("mn-ref-render-{n}-9f2c1.png"));
            image::RgbaImage::from_pixel(64, 48, image::Rgba(c))
                .save(&p)
                .expect("write the test png");
            made.push(p);
        }
        let gone = dir.join("mn-ref-render-gone-9f2c1.png");
        let _ = std::fs::remove_file(&gone);

        let mut app = crate::app::App::new(renderer, (900, 700), 1.0);
        app.refs.paths = vec![made[0].clone(), made[1].clone(), gone.clone()];
        crate::ui::dock::reopen(&mut app, crate::ui::dock::Palette::References);
        show_viewer(&mut app, made[0].clone());
        assert_eq!(app.refs.views.len(), 1);
        // Asking twice focuses rather than stacking a second window on the
        // same image (the window id is the path).
        show_viewer(&mut app, made[0].clone());
        assert_eq!(app.refs.views.len(), 1);

        let ctx = app.shell.ctx.clone();
        let mut loaded_per_frame = Vec::new();
        for _ in 0..8 {
            let before = app.refs.thumbs.len() + app.refs.full.len();
            let raw = app.shell.begin((900, 700));
            let mut out = ctx.run_ui(raw, |ui| crate::ui::build(ui, &mut app));
            // No GPU pass in this test, so the deltas are ours to drop.
            out.textures_delta.clear();
            loaded_per_frame.push(app.refs.thumbs.len() + app.refs.full.len() - before);
        }
        assert!(
            loaded_per_frame.iter().all(|&n| n <= 1),
            "one image per frame, never a burst: {loaded_per_frame:?}"
        );
        assert_eq!(app.refs.thumbs.len(), 3, "every visible row got a thumbnail");
        assert!(
            app.refs.thumbs[&gone].missing,
            "the renamed file is a placeholder row, not a hole"
        );
        assert!(
            app.refs.full.get(&made[0]).is_some_and(|t| t.is_some()),
            "the open viewer loaded its texture"
        );
        assert!(
            !app.refs.full.contains_key(&made[1]),
            "a reference with no viewer open must not cost a full texture"
        );

        // Closing the window hands the texture (and its pick buffer) back.
        app.refs.views.clear();
        let raw = app.shell.begin((900, 700));
        let mut out = ctx.run_ui(raw, |ui| crate::ui::build(ui, &mut app));
        out.textures_delta.clear();
        assert!(app.refs.full.is_empty(), "closed viewers free their VRAM");

        for p in made {
            let _ = std::fs::remove_file(p);
        }
    }

    /// The list is a set in practice: adding the same reference twice is one
    /// row, because two rows would fight over one viewer window id.
    #[test]
    fn adding_the_same_reference_twice_is_one_row() {
        let mut bank = RefBank::default();
        let a = PathBuf::from(r"D:\refs\hand.png");
        assert_eq!(bank.add(vec![a.clone(), PathBuf::from(r"D:\refs\eye.png")]), 2);
        assert_eq!(bank.add(vec![a]), 0);
        assert_eq!(bank.paths.len(), 2);
        assert_eq!(bank.to_lines()[0], r"D:\refs\hand.png");
    }
}
