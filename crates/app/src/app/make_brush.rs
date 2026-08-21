//! Make a brush from marks on the canvas (the ROADMAP's "brushes and
//! materials without ceremony", capture half): lasso the marks, Edit ▸
//! Register selection as brush tip, and the selection's ink becomes a
//! pure-stamp preset in group "mine" — written through the same rail the
//! `.abr` import uses (`abr::write_brush`), so the texture picker, Tool
//! Property and test stroke all see it with no extra wiring.
//!
//! The tip mask is the marks' ALPHA (ink on a transparent layer), scaled by
//! the selection's coverage so a feathered lasso fades the tip edge. Marks
//! on an opaque flat (ink on white paper) would read as a solid rectangle
//! that way, so when the lifted region is effectively fully opaque the
//! DARKNESS becomes the ink instead — a black stroke on white is the tip.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::abr::{MAX_DEFAULT_PX, base_settings, set_base, tight_crop, write_brush};
use crate::app::App;
use crate::cmd::AppCmd;

/// Alpha this close to full counts as opaque for the flat-artwork check.
const OPAQUE: u8 = 250;

impl App {
    /// Edit ▸ Register selection as brush tip. Sets its own status lines;
    /// the dispatch arm only has to call it.
    pub fn register_brush_from_selection(&mut self) {
        let Some(root) = self.brushes_root.clone() else {
            return self.set_error("make brush: no brushes folder found");
        };
        match self.register_brush_from_selection_into(root) {
            Some((path, name)) => {
                self.presets = super::scan_presets();
                self.texture_names = super::scan_textures(self.brushes_root.as_deref());
                // Hand the new brush straight to the pen: the test stroke
                // and Tool Property are the tuning UI.
                self.push_cmd(AppCmd::SelectBrush(path));
                self.set_status(format!(
                    "brush \"{name}\" registered (group Mine) — your marks stamp along the stroke; tune size and spacing in Tool Property"
                ));
            }
            None => self.set_error(
                "make brush: lasso some ink on the active raster layer first",
            ),
        }
    }

    /// The write half, target-root injected so tests stay out of the real
    /// assets (the same split `material_register_layer_into` uses). Returns
    /// the written preset's path and display name.
    pub(crate) fn register_brush_from_selection_into(
        &mut self,
        root: PathBuf,
    ) -> Option<(PathBuf, String)> {
        if self.doc.selection.as_ref().is_none_or(|s| s.is_empty()) {
            return None;
        }
        {
            let l = self.doc.active_layer();
            if l.folder || l.is_vector() {
                return None;
            }
        }
        let r = crate::cmd::transform_lift_rect(self)?;
        if r[0] >= r[2] || r[1] >= r[3] {
            return None;
        }
        let src = mn_core::transform::lift_region(
            self.doc.active_layer(),
            r,
            self.doc.selection.as_ref(),
        );
        if src.tiles.is_empty() {
            return None;
        }
        let (w, h) = ((r[2] - r[0]) as u32, (r[3] - r[1]) as u32);
        let count = (w as usize) * (h as usize);
        let mut alpha = vec![0u8; count];
        let mut dark = vec![0u8; count];
        let (mut ink_px, mut opaque_px) = (0usize, 0usize);
        for y in 0..h {
            for x in 0..w {
                let p = src.pixel(r[0] + x as i32, r[1] + y as i32);
                let a = p[3] as u32;
                let i = (y * w + x) as usize;
                let a8 = ((a * 255 + 16384) / 32768) as u8;
                alpha[i] = a8;
                if a8 == 0 {
                    continue;
                }
                ink_px += 1;
                opaque_px += (a8 >= OPAQUE) as usize;
                // Unpremultiply for luma, then scale the darkness by alpha
                // so the selection's feather still fades an opaque tip.
                let un = |c: u16| ((c as u32 * 32768 / a).min(32768) * 255 + 16384) / 32768;
                let luma = (un(p[0]) * 299 + un(p[1]) * 587 + un(p[2]) * 114) / 1000;
                dark[i] = ((255 - luma.min(255)) * a8 as u32 / 255) as u8;
            }
        }
        // Opaque flat: alpha carries no shape, the drawing's darkness does.
        let flat = ink_px > 0 && opaque_px * 100 >= ink_px * 98;
        let gray = if flat { dark } else { alpha };
        let (gray, tw, th) = tight_crop(&gray, w, h)?;

        let natural = tw.max(th) as f64;
        let mut settings = base_settings(natural.min(MAX_DEFAULT_PX));
        // Stamps clearly separated by default (interval ≈ the tip's own
        // diameter): the first test stroke visibly repeats the marks
        // instead of smearing them into a fat line. Spacing is a dial away.
        set_base(&mut settings, "dabs_per_basic_radius", 0.5);
        set_base(&mut settings, "dabs_per_actual_radius", 0.5);
        let mut extras = serde_json::Map::new();
        // Dab-anchored stamp: the tip IS the coverage (the pure-stamp mode
        // the ABR imports use), not a canvas-anchored grain.
        extras.insert("mn-texture-anchor".into(), json!("dab"));

        let n = next_index(&root);
        let name = format!("Canvas brush {n}");
        let mut notes = Vec::new();
        if natural > MAX_DEFAULT_PX {
            notes.push(format!(
                "captured at {natural:.0} px (default capped at {MAX_DEFAULT_PX:.0})"
            ));
        }
        let desc =
            "Brush tip captured from canvas marks (Edit ▸ Register selection as brush tip)"
                .to_string();
        let ok = write_brush(
            &root,
            "mine",
            "mine",
            n,
            &name,
            Some((&gray, tw, th)),
            settings,
            extras,
            desc,
            &notes,
        );
        ok.then(|| (root.join("mine").join(format!("mine-{n}.myb")), name))
    }
}

/// Next free `mine-N` index: captured brushes never overwrite each other,
/// even after deletions in the middle of the run.
fn next_index(root: &Path) -> usize {
    let mut max = 0usize;
    if let Ok(rd) = std::fs::read_dir(root.join("mine")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(num) = name
                .strip_prefix("mine-")
                .and_then(|s| s.strip_suffix(".myb"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                max = max.max(num);
            }
        }
    }
    max + 1
}

#[cfg(test)]
mod tests {
    use mn_core::TileIdx;

    use crate::app::{App, headless_renderer};

    const W: u16 = mn_core::FIX15_ONE as u16;

    fn tmp_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mn-mkbrush-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ink(app: &mut App, px: &[(i32, i32)], color: [u16; 4]) {
        app.doc.begin_op();
        for &(x, y) in px {
            let idx = TileIdx::of_pixel(x, y);
            app.doc.active_layer_mut().tile_mut(idx).set_pixel(
                (x - idx.origin().0) as usize,
                (y - idx.origin().1) as usize,
                color,
            );
        }
        app.doc.end_op();
    }

    /// Transparent-layer marks: the tip is the ALPHA, tight-cropped, the
    /// preset is a dab-anchored stamp in group "mine".
    #[test]
    fn selection_marks_become_a_mine_stamp_preset() {
        let Some(renderer) = headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (600, 400), 1.0);
        let root = tmp_root("marks");
        // A 3-px diagonal of ink well inside a larger selection.
        ink(&mut app, &[(100, 100), (101, 101), (102, 102)], [W, 0, 0, W]);
        app.doc.selection = Some(mn_core::Selection::from_rect(
            &app.doc, 90.0, 90.0, 130.0, 130.0,
        ));
        let (path, name) = app
            .register_brush_from_selection_into(root.clone())
            .expect("marks inside a selection register");
        assert_eq!(name, "Canvas brush 1");
        let myb: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(myb["group"], "mine");
        assert_eq!(myb["mn-texture-anchor"], "dab");
        assert_eq!(myb["mn-texture"], "mine-1");
        // The texture is the TIGHT crop of the marks (3×3), padded square.
        let img = image::open(root.join("textures/mine-1.png")).unwrap();
        assert_eq!((img.width(), img.height()), (3, 3), "tight crop, not bbox");
        // A second capture numbers up instead of overwriting.
        let (_, name2) = app
            .register_brush_from_selection_into(root.clone())
            .unwrap();
        assert_eq!(name2, "Canvas brush 2");
    }

    /// Ink on an opaque white flat: alpha is a solid block, so the DARKNESS
    /// becomes the tip — the black stroke, not the paper.
    #[test]
    fn opaque_flat_uses_darkness_as_the_tip() {
        let Some(renderer) = headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (600, 400), 1.0);
        let root = tmp_root("flat");
        // White 8×8 flat with one black pixel in the middle.
        let mut px = Vec::new();
        for y in 100..108 {
            for x in 100..108 {
                px.push((x, y));
            }
        }
        ink(&mut app, &px, [W, W, W, W]);
        ink(&mut app, &[(103, 103)], [0, 0, 0, W]);
        app.doc.selection = Some(mn_core::Selection::from_rect(
            &app.doc, 100.0, 100.0, 108.0, 108.0,
        ));
        let (path, _) = app
            .register_brush_from_selection_into(root.clone())
            .expect("opaque flat registers via darkness");
        let myb: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let tex = myb["mn-texture"].as_str().unwrap();
        let img = image::open(root.join(format!("textures/{tex}.png")))
            .unwrap()
            .to_luma8();
        assert_eq!(
            (img.width(), img.height()),
            (1, 1),
            "the tip tight-crops to the one dark pixel, not the white paper"
        );
        assert!(img.get_pixel(0, 0).0[0] > 200, "dark ink = high mask value");
    }

    /// No selection, or a selection over nothing: refuse instead of writing
    /// a blank preset.
    #[test]
    fn refuses_without_selection_or_ink() {
        let Some(renderer) = headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (600, 400), 1.0);
        let root = tmp_root("refuse");
        assert!(
            app.register_brush_from_selection_into(root.clone()).is_none(),
            "no selection"
        );
        app.doc.selection = Some(mn_core::Selection::from_rect(
            &app.doc, 10.0, 10.0, 40.0, 40.0,
        ));
        assert!(
            app.register_brush_from_selection_into(root).is_none(),
            "selection over empty canvas"
        );
    }
}
