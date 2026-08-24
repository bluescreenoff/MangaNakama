//! Pattern Studio (ROADMAP: "making a tiling pattern should be one gesture,
//! not a ceremony").
//!
//! The engine already had every ingredient: wrap-around drawing (dabs
//! crossing an edge continue on the opposite side, so a stroke is seamless
//! by construction), a material bank that scans plain PNGs, and an offscreen
//! renderer. What was missing was the AUTHORING PATH — this module is that
//! glue and deliberately nothing more:
//!
//! - **File ▸ New Tiling Pattern** opens a fresh square canvas in a new tab
//!   with wrap ON for both axes, and opens the studio window.
//! - The window shows the canvas **repeated in a grid, live** — re-rendered
//!   only when the document actually changed (tile/doc revisions), through
//!   the same offscreen renderer the page previews use.
//! - **Save as material** composites the whole document with transparent
//!   background at full resolution and registers it exactly like
//!   `material_register_layer` does (straight-alpha PNG, unique stem,
//!   `materials-mine`, rescan) — never trimmed: the canvas rectangle IS the
//!   tile, trimming it would change the repeat.
//!
//! Contrast with the benchmark (Clip Studio's own tutorial for this is a
//! long numbered list): draw, watch it repeat, name it, done.

use std::path::PathBuf;

use crate::app::pages::render_offscreen_drafts_off;
use crate::app::{App, PageEntry};

/// Default pattern canvas edge, px. Square, small enough that the live
/// preview re-render is negligible; Change Canvas Size works as usual for
/// artists who want another tile size.
pub const PATTERN_SIZE: u32 = 512;
/// Preview cell edge, px (one repeat in the studio grid).
const PREVIEW_CELL: u32 = 224;

/// The studio window's state. Lives on `App` like the other tool windows;
/// session-only, nothing persists.
pub struct PatternStudio {
    pub open: bool,
    /// Material name the Save button will use (slugged at save).
    pub name: String,
    /// Repeats per axis in the preview (2 or 3).
    pub grid: u32,
    /// The rendered single-tile preview; drawn `grid × grid` times.
    pub tex: Option<egui::TextureHandle>,
    /// Document state the texture was rendered at.
    seen: u64,
    /// Fresh-texture counter (same trick as the page thumbs: a shared name
    /// would alias every clone of the handle).
    seq: u64,
}

impl Default for PatternStudio {
    fn default() -> Self {
        PatternStudio {
            open: false,
            name: String::new(),
            grid: 3,
            tex: None,
            seen: 0,
            seq: 0,
        }
    }
}

impl App {
    /// One gesture: a fresh square canvas in a NEW tab (the current document
    /// parks, same as New Manga), wrap on for both axes, studio open.
    pub fn pattern_new(&mut self) {
        self.commit_text_edit();
        self.push_doc_slot();
        // Guides cleared BEFORE the doc is built: a leftover comic setup
        // would otherwise seed a frame folder into the pattern tile.
        self.page = None;
        self.doc = self.blank_page_doc_sized(PATTERN_SIZE, PATTERN_SIZE);
        self.pages = vec![PageEntry::active()];
        self.page_index = 0;
        self.set_doc_path(None);
        self.reset_folder_state();
        self.renderer.invalidate();
        self.layer_thumbs.clear();
        self.fit_to_view();
        self.mark_saved();
        // Wrap is the point: seamless by construction. Mirror painting and
        // wrap share an axis slot (the SetWrap dispatch rule), so mirrors
        // drop.
        self.wrap_x = true;
        self.wrap_y = true;
        self.mirror_x = false;
        self.mirror_y = false;
        self.rebuild_twins();
        self.pattern.open = true;
        self.pattern.name.clear();
        self.pattern.tex = None;
        self.pattern.seen = 0;
        self.set_status("pattern canvas: strokes wrap at every edge; the studio shows the repeat");
        self.mark_dirty();
    }

    /// What the preview must track: any tile write (layer revisions) or any
    /// presentation change the tile path cannot see (`Document::touch`).
    fn pattern_doc_state(&self) -> u64 {
        self.doc.revision.max(self.doc.max_revision())
    }

    /// The single-tile preview texture, re-rendered only when the document
    /// changed since the last render.
    pub fn pattern_preview_tex(&mut self) -> egui::TextureHandle {
        let state = self.pattern_doc_state();
        if let Some(t) = &self.pattern.tex
            && self.pattern.seen == state
        {
            return t.clone();
        }
        let img = render_offscreen_drafts_off(
            &mut self.renderer,
            &mut self.doc,
            PREVIEW_CELL,
            PREVIEW_CELL,
        );
        let (w, h) = (img.width() as usize, img.height() as usize);
        let ci = egui::ColorImage::from_rgba_unmultiplied([w, h], img.as_raw());
        self.pattern.seq += 1;
        let tex = self.shell.ctx.load_texture(
            format!("mn.pattern.{}", self.pattern.seq),
            ci,
            egui::TextureOptions::LINEAR,
        );
        self.pattern.tex = Some(tex.clone());
        self.pattern.seen = state;
        tex
    }

    /// Composite the whole document (transparent background, full
    /// resolution, NEVER trimmed — the canvas rectangle is the tile) and
    /// register it in the material bank under the studio's name.
    pub fn pattern_save_material(&mut self) -> Option<(PathBuf, String)> {
        let dir = self.registered_material_folder();
        let out = self.pattern_save_material_into(dir)?;
        self.materials_scan();
        Some(out)
    }

    /// The write half, target-directory injected so tests stay out of the
    /// real bank.
    fn pattern_save_material_into(&mut self, dir: PathBuf) -> Option<(PathBuf, String)> {
        self.refresh_tones();
        let img = mn_core::export::composite_for_export(
            &self.doc,
            mn_core::export::Background::Transparent,
        );
        if !img.pixels().any(|p| p.0[3] != 0) {
            return None; // an empty tile is a mistake, not a material
        }
        let base: String = self
            .pattern
            .name
            .trim()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let base = if base.is_empty() {
            "pattern".into()
        } else {
            base
        };
        let mut stem = base.clone();
        let mut path = dir.join(format!("{stem}.png"));
        let mut n = 1;
        while path.exists() {
            n += 1;
            stem = format!("{base}-{n}");
            path = dir.join(format!("{stem}.png"));
        }
        image::save_buffer(
            &path,
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
        Some((path, stem))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn app() -> Option<App> {
        Some(App::new(crate::app::headless_renderer()?, (600, 400), 1.0))
    }

    #[test]
    fn new_pattern_is_square_wrapped_and_studio_open() {
        let Some(mut app) = app() else { return };
        // A mirror left on must drop: wrap and mirror share the axis slot.
        app.mirror_x = true;
        app.pattern_new();
        assert_eq!(app.doc.size, (PATTERN_SIZE, PATTERN_SIZE));
        assert!(app.wrap_x && app.wrap_y);
        assert!(!app.mirror_x && !app.mirror_y);
        assert!(app.pattern.open);
        assert_eq!(app.pages.len(), 1);
        assert!(
            !app.doc.layers.iter().any(|l| l.folder),
            "no leftover frame folder seeds into a pattern tile"
        );
    }

    #[test]
    fn saving_writes_the_untrimmed_transparent_tile_and_never_clobbers() {
        let Some(mut app) = app() else { return };
        app.pattern_new();
        let dir = std::env::temp_dir().join(format!("mn-pattern-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // An empty tile is a mistake, not a material.
        assert!(app.pattern_save_material_into(dir.clone()).is_none());

        const W: u16 = mn_core::FIX15_ONE as u16;
        app.doc.begin_op();
        app.doc
            .active_layer_mut()
            .tile_mut(mn_core::TileIdx::new(0, 0))
            .set_pixel(3, 4, [W, W, W, W]);
        app.doc.end_op();

        app.pattern.name = "Dots! v1".into();
        let (p1, s1) = app.pattern_save_material_into(dir.clone()).expect("saves");
        assert_eq!(s1, "Dots__v1");
        let img = image::open(&p1).unwrap().to_rgba8();
        // Full canvas, never trimmed to the ink: the rectangle IS the tile.
        assert_eq!(img.dimensions(), (PATTERN_SIZE, PATTERN_SIZE));
        assert!(img.get_pixel(3, 4)[3] > 0);
        assert_eq!(
            img.get_pixel(100, 100)[3],
            0,
            "background stays transparent"
        );

        // The same name again suffixes instead of clobbering.
        let (_, s2) = app.pattern_save_material_into(dir.clone()).expect("saves");
        assert_eq!(s2, "Dots__v1-2");
        std::fs::remove_dir_all(&dir).ok();
    }
}
