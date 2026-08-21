//! Batch layer operations (the ROADMAP's "recordable actions" pain list:
//! rename, renumber, apply tone, export — not macro recording for its own
//! sake). One dialog: pick a SCOPE (all layers / the active folder's
//! children / a name prefix), pick an operation, apply.
//!
//! Undo semantics follow the singles they batch: rename is not undoable
//! (neither is a single rename — CSP parity), tone changes land as ONE
//! step (`Document::set_tone_many`, a `Compound` transaction), export
//! writes files and touches nothing.

use std::path::{Path, PathBuf};

use super::App;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum BatchScope {
    #[default]
    AllLayers,
    /// Children of the ACTIVE folder (the active layer must be one).
    FolderChildren,
    /// Layers whose name starts with the typed prefix.
    Prefix,
    /// The palette's multi-selection (active row + Ctrl/Shift-picked rows).
    /// With nothing multi-selected that is just the active layer, which is
    /// what CSP means by "the selection" of one row.
    Selected,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum BatchOp {
    /// Rename every match with the pattern (`{n}` = 1-based counter,
    /// `{name}` = the current name). "Panel {n}" renumbers.
    #[default]
    Rename,
    /// Copy the ACTIVE layer's tone params onto every match.
    ToneFromActive,
    /// Remove tone from every match.
    ToneClear,
    /// Write each match as a full-canvas PNG into a chosen folder.
    ExportPngs,
}

#[derive(Default)]
pub struct BatchOps {
    pub open: bool,
    pub scope: BatchScope,
    pub prefix: String,
    pub op: BatchOp,
    pub pattern: String,
}

impl App {
    /// The layer indices the current scope selects, bottom-to-top.
    /// Folder headers themselves are never matched (renaming or toning a
    /// header from a batch is a surprise, not a service).
    pub fn batch_matches(&self) -> Vec<usize> {
        match self.batch.scope {
            BatchScope::AllLayers => (0..self.doc.layers.len())
                .filter(|&i| !self.doc.layers[i].folder)
                .collect(),
            BatchScope::FolderChildren => {
                let hi = self.doc.active;
                let Some(header) = self.doc.layers.get(hi) else {
                    return Vec::new();
                };
                if !header.folder {
                    return Vec::new();
                }
                // Children sit BELOW the header, at greater depth, until
                // the depth returns to the header's.
                let mut out = Vec::new();
                for i in (0..hi).rev() {
                    let l = &self.doc.layers[i];
                    if l.depth <= header.depth {
                        break;
                    }
                    if !l.folder {
                        out.push(i);
                    }
                }
                out.reverse();
                out
            }
            BatchScope::Prefix => {
                let p = self.batch.prefix.trim();
                if p.is_empty() {
                    return Vec::new();
                }
                (0..self.doc.layers.len())
                    .filter(|&i| {
                        !self.doc.layers[i].folder && self.doc.layers[i].name.starts_with(p)
                    })
                    .collect()
            }
            BatchScope::Selected => self
                .doc
                .multi_targets()
                .into_iter()
                .filter(|&i| self.doc.layers.get(i).is_some_and(|l| !l.folder))
                .collect(),
        }
    }

    /// Apply the non-export operations. Returns a status line.
    pub fn batch_apply(&mut self) -> String {
        let matches = self.batch_matches();
        if matches.is_empty() {
            return "batch: nothing matches that scope".into();
        }
        match self.batch.op {
            BatchOp::Rename => {
                let pattern = self.batch.pattern.clone();
                if pattern.trim().is_empty() {
                    return "batch: the rename pattern is empty".into();
                }
                // Top-to-bottom numbering: artists count panels from the
                // top of the stack, so {n}=1 is the topmost match.
                for (n, &i) in matches.iter().rev().enumerate() {
                    let name = pattern
                        .replace("{n}", &(n + 1).to_string())
                        .replace("{name}", &self.doc.layers[i].name);
                    self.doc.rename_layer(i, name);
                }
                self.mark_dirty();
                format!("batch: renamed {} layers", matches.len())
            }
            BatchOp::ToneFromActive => {
                let Some(tone) = self
                    .doc
                    .layers
                    .get(self.doc.active)
                    .and_then(|l| l.tone)
                    .map(Some)
                else {
                    return "batch: the active layer has no tone to copy".into();
                };
                let n = self.doc.set_tone_many(&matches, tone);
                for &i in &matches {
                    self.renderer.evict_layer(i);
                }
                self.refresh_tones();
                self.mark_dirty();
                format!("batch: tone applied to {n} layers (one undo step)")
            }
            BatchOp::ToneClear => {
                let n = self.doc.set_tone_many(&matches, None);
                for &i in &matches {
                    self.renderer.evict_layer(i);
                }
                self.refresh_tones();
                self.mark_dirty();
                format!("batch: tone removed from {n} layers (one undo step)")
            }
            BatchOp::ExportPngs => {
                // Resolved through the folder dialog; the dispatch routes
                // to `batch_export_pngs` with the picked path.
                String::new()
            }
        }
    }

    /// Write every match as `<NN>-<name>.png` (full canvas, straight
    /// alpha) into `dir`. Numbering is top-to-bottom like the renamer.
    pub fn batch_export_pngs(&mut self, dir: &Path) -> String {
        self.refresh_tones();
        let matches = self.batch_matches();
        if matches.is_empty() {
            return "batch: nothing matches that scope".into();
        }
        let mut written = 0usize;
        for (n, &i) in matches.iter().rev().enumerate() {
            let img = layer_png(self, i);
            let safe: String = self.doc.layers[i]
                .name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let path = dir.join(format!("{:02}-{}.png", n + 1, safe));
            if img.save(&path).is_ok() {
                written += 1;
            }
        }
        format!("batch: {written} layer PNGs -> {}", dir.display())
    }
}

/// One layer alone, full canvas, straight alpha — through the display path
/// (derived rasters included; `refresh_tones` must have run).
fn layer_png(app: &App, li: usize) -> image::RgbaImage {
    let (w, h) = app.doc.size;
    let mut img = image::RgbaImage::new(w, h);
    let l = &app.doc.layers[li];
    for (idx, t) in l.display_tiles() {
        let (ox, oy) = idx.origin();
        for py in 0..mn_core::TILE_SIZE {
            let y = oy + py as i32;
            if y < 0 || y >= h as i32 {
                continue;
            }
            for px in 0..mn_core::TILE_SIZE {
                let x = ox + px as i32;
                if x < 0 || x >= w as i32 {
                    continue;
                }
                let p = t.pixel(px, py);
                let a = p[3] as u32;
                if a == 0 {
                    continue;
                }
                let un = |c: u16| (((c as u32 * 32768 / a).min(32768) * 255 + 16384) / 32768) as u8;
                img.put_pixel(
                    x as u32,
                    y as u32,
                    image::Rgba([un(p[0]), un(p[1]), un(p[2]), ((a * 255 + 16384) / 32768) as u8]),
                );
            }
        }
    }
    img
}

/// Export dir memory for the pump (rfd runs in main.rs).
pub fn _dir_placeholder() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use mn_core::{TileIdx, tone::ToneParams};

    fn app() -> Option<App> {
        let mut app = App::new(super::super::headless_renderer()?, (600, 400), 1.0);
        // Three layers + a folder with one child.
        app.doc.rename_layer(0, "base");
        app.doc.add_layer("Panel a");
        app.doc.add_layer("Panel b");
        let f = app.doc.add_folder_above(app.doc.active, "F");
        app.doc.add_layer_in_folder(f, "inner");
        Some(app)
    }

    #[test]
    fn scopes_select_the_right_layers() {
        let Some(mut app) = app() else { return };
        app.batch.scope = BatchScope::AllLayers;
        let names = |app: &App, idxs: &[usize]| -> Vec<String> {
            idxs.iter().map(|&i| app.doc.layers[i].name.clone()).collect()
        };
        let m = app.batch_matches();
        assert_eq!(
            names(&app, &m),
            vec!["base", "Panel a", "Panel b", "inner"],
            "all layers, no folder headers"
        );
        app.batch.scope = BatchScope::Prefix;
        app.batch.prefix = "Panel".into();
        assert_eq!(names(&app, &app.batch_matches()), vec!["Panel a", "Panel b"]);
        // Folder children: select the folder header first.
        let f = app
            .doc
            .layers
            .iter()
            .position(|l| l.folder)
            .unwrap();
        app.doc.set_active(f);
        app.batch.scope = BatchScope::FolderChildren;
        assert_eq!(names(&app, &app.batch_matches()), vec!["inner"]);
    }

    /// The palette multi-selection is a scope of its own: exactly the
    /// picked rows, folder headers dropped like every other scope, and an
    /// empty multi-selection meaning the active layer alone.
    #[test]
    fn selected_scope_follows_the_palette() {
        let Some(mut app) = app() else { return };
        let names = |app: &App, idxs: &[usize]| -> Vec<String> {
            idxs.iter().map(|&i| app.doc.layers[i].name.clone()).collect()
        };
        let idx = |app: &App, n: &str| app.doc.layers.iter().position(|l| l.name == n).unwrap();
        app.batch.scope = BatchScope::Selected;

        // Two of the three ordinary layers, Ctrl+click style.
        let (a, b) = (idx(&app, "Panel a"), idx(&app, "Panel b"));
        app.doc.set_active(a);
        assert!(app.doc.toggle_multi(b));
        assert_eq!(names(&app, &app.batch_matches()), vec!["Panel a", "Panel b"]);

        // A folder header in the selection is not a match.
        let f = idx(&app, "F");
        assert!(app.doc.toggle_multi(f));
        assert!(app.doc.multi_targets().contains(&f), "header really is selected");
        assert_eq!(
            names(&app, &app.batch_matches()),
            vec!["Panel a", "Panel b"],
            "folder headers never match"
        );

        // Nothing multi-selected = the active layer alone.
        app.doc.set_active(idx(&app, "base"));
        assert!(app.doc.layer_multi.is_empty());
        assert_eq!(names(&app, &app.batch_matches()), vec!["base"]);
    }

    #[test]
    fn rename_pattern_numbers_top_down() {
        let Some(mut app) = app() else { return };
        app.batch.scope = BatchScope::Prefix;
        app.batch.prefix = "Panel".into();
        app.batch.op = BatchOp::Rename;
        app.batch.pattern = "コマ {n} ({name})".into();
        let s = app.batch_apply();
        assert!(s.contains("renamed 2"), "{s}");
        // Top of the stack is {n}=1: "Panel b" sits above "Panel a".
        let by_name = |app: &App, n: &str| app.doc.layers.iter().any(|l| l.name == n);
        assert!(by_name(&app, "コマ 1 (Panel b)"));
        assert!(by_name(&app, "コマ 2 (Panel a)"));
    }

    /// Batch tone = ONE undo step across every match (the Compound
    /// transaction), and one undo takes it all back.
    #[test]
    fn batch_tone_is_one_undo_step() {
        let Some(mut app) = app() else { return };
        // Give the active layer a tone to copy.
        let li = app.doc.active;
        assert!(app.doc.set_tone(li, Some(ToneParams::default())));
        let steps = app.doc.undo_len();
        app.batch.scope = BatchScope::Prefix;
        app.batch.prefix = "Panel".into();
        app.batch.op = BatchOp::ToneFromActive;
        let s = app.batch_apply();
        assert!(s.contains("2 layers"), "{s}");
        assert_eq!(app.doc.undo_len(), steps + 1, "one step for the batch");
        let toned = |app: &App| {
            app.doc
                .layers
                .iter()
                .filter(|l| l.name.starts_with("Panel") && l.tone.is_some())
                .count()
        };
        assert_eq!(toned(&app), 2);
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
        assert_eq!(toned(&app), 0, "one undo clears the whole batch");
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Redo);
        assert_eq!(toned(&app), 2);
    }

    #[test]
    fn export_writes_one_png_per_match() {
        let Some(mut app) = app() else { return };
        const W: u16 = mn_core::FIX15_ONE as u16;
        // Ink the two panels so the files carry pixels.
        for name in ["Panel a", "Panel b"] {
            let i = app.doc.layers.iter().position(|l| l.name == name).unwrap();
            app.doc.set_active(i);
            app.doc.begin_op();
            app.doc
                .active_layer_mut()
                .tile_mut(TileIdx::new(0, 0))
                .set_pixel(3, 4, [W, W, W, W]);
            app.doc.end_op();
        }
        app.batch.scope = BatchScope::Prefix;
        app.batch.prefix = "Panel".into();
        let dir = std::env::temp_dir().join(format!("mn-batch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = app.batch_export_pngs(&dir);
        assert!(s.contains("2 layer PNGs"), "{s}");
        let img = image::open(dir.join("01-Panel_b.png")).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), app.doc.size, "full canvas, whatever it is");
        assert!(img.get_pixel(3, 4)[3] > 0);
        std::fs::remove_dir_all(&dir).ok();
    }
}
