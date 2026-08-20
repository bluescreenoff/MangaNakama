//! The material bank (TRIAGE 133 part 1): scanned image materials and
//! their use counters. Folders: the shipped `assets/materials` starter
//! set plus user-added folders persisted in `ui.txt` (the owner's laptop
//! points at his local CSP materials folder — local use only, never
//! distributed; DECISIONS 8.5's bring-your-own-materials rule).
//!
//! v1 scope: plain image materials (PNG/JPEG), pasted at natural size as
//! the move/scale float (the clipboard's stamp path), with the owner's
//! **tiling** ask — one click covers the canvas in N×N copies as a single
//! float, usable as a mask to draw through. Deferred to part 2 (TRIAGE
//! row 133's named rows): Toning (image → screentone, MT-014), the five
//! paste-size modes (MT-032), order-in-layer metadata (MT-034), tags
//! (MT-012).

use std::path::{Path, PathBuf};

use super::App;

/// One scanned material. Display name = file stem; the full path is the
/// identity (use counters key on it — folders that move lose their
/// counts, an accepted v1 trade).
#[derive(Clone, Debug)]
pub struct MaterialItem {
    pub name: String,
    pub path: PathBuf,
    /// Index into the folder list (grouping + display).
    pub folder: usize,
}

impl App {
    /// The default (shipped starter) materials folder, exe-relative like
    /// `brushes_root` — resolved without the non-empty check so an empty
    /// folder still shows in the palette as "add materials here".
    pub fn materials_default_folder() -> Option<PathBuf> {
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            for anc in ["..", "../..", "../../.."] {
                let p = dir.join(anc).join("assets/materials");
                if p.is_dir() {
                    return Some(p);
                }
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            let p = cwd.join("assets/materials");
            if p.is_dir() {
                return Some(p);
            }
        }
        None
    }

    /// Rescan every folder into the bank (idempotent; called at startup,
    /// after folder edits, and by the palette's Rescan button).
    pub fn materials_scan(&mut self) {
        let mut out = Vec::new();
        for (fi, folder) in self.material_folders.iter().enumerate() {
            let Ok(rd) = std::fs::read_dir(folder) else {
                continue;
            };
            let mut items: Vec<MaterialItem> = rd
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let ext = p
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| x.to_ascii_lowercase())
                        .unwrap_or_default();
                    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
                        return None;
                    }
                    let name = p.file_stem()?.to_string_lossy().into_owned();
                    Some(MaterialItem {
                        name,
                        path: p,
                        folder: fi,
                    })
                })
                .collect();
            items.sort_by(|a, b| a.name.cmp(&b.name));
            out.extend(items);
        }
        self.materials = out;
        // Folder names for the palette's grouping labels.
        self.material_folder_names = self
            .material_folders
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string())
            })
            .collect();
        self.material_thumbs.clear();
    }

    /// Count a use (frequency-of-use sorting, MT-016's input) and mark the
    /// layout dirty for the next ui.txt save.
    pub fn material_note_use(&mut self, path: &Path) {
        let key = path.display().to_string();
        let c = self.material_uses.entry(key).or_insert(0);
        *c += 1;
        self.layout.note_materials(
            &self.user_material_folders(),
            &serde_json::to_string(&self.material_uses).unwrap_or_default(),
        );
    }

    /// The user-added folders as persisted strings (the shipped starter
    /// folder is implicit and never persisted).
    pub fn user_material_folders(&self) -> Vec<String> {
        self.material_folders[1..]
            .iter()
            .map(|p| p.display().to_string())
            .collect()
    }
}

// --- TRIAGE 151 v1: register + bulk import (MT-020's raster half) --------

impl App {
    /// Where registered/imported materials land: the first user-added
    /// folder if one exists, else a new exe-adjacent `materials-mine`
    /// (created and attached, persisted with the other user folders).
    pub fn registered_material_folder(&mut self) -> PathBuf {
        if self.material_folders.len() > 1 {
            return self.material_folders[1].clone();
        }
        let base = Self::materials_default_folder()
            .and_then(|p| p.parent().map(|a| a.to_path_buf()))
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|e| e.parent().map(|p| p.to_path_buf()))
            });
        let dir = base
            .unwrap_or_else(|| PathBuf::from("."))
            .join("materials-mine");
        let _ = std::fs::create_dir_all(&dir);
        self.material_folders.push(dir.clone());
        self.layout.note_materials(
            &self.user_material_folders(),
            &serde_json::to_string(&self.material_uses).unwrap_or_default(),
        );
        dir
    }

    /// MT-020 (raster half): the active layer becomes an image material —
    /// selection-scoped (no selection = the whole layer's ink), exported
    /// as a straight-alpha PNG into the registered folder, name taken
    /// from the layer. Vector/balloon/text type-follows is part 2.
    pub fn material_register_layer(&mut self) -> Option<(PathBuf, String)> {
        let l = self.doc.active_layer();
        if l.folder || l.is_vector() {
            return None;
        }
        let Some(r) = crate::cmd::transform_lift_rect(self) else {
            return None;
        };
        if r[0] >= r[2] || r[1] >= r[3] {
            return None;
        }
        let src = mn_core::transform::lift_region(l, r, self.doc.selection.as_ref());
        if src.tiles.is_empty() {
            return None;
        }
        let (w, h) = ((r[2] - r[0]) as u32, (r[3] - r[1]) as u32);
        let mut img = image::RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let p = src.pixel(r[0] + x as i32, r[1] + y as i32);
                let a = p[3] as u32;
                let px = if a == 0 {
                    [0, 0, 0, 0]
                } else {
                    let un =
                        |c: u16| (((c as u32 * 32768 / a).min(32768) * 255 + 16384) / 32768) as u8;
                    [
                        un(p[0]),
                        un(p[1]),
                        un(p[2]),
                        ((a * 255 + 16384) / 32768) as u8,
                    ]
                };
                img.put_pixel(x, y, image::Rgba(px));
            }
        }
        let base: String = l
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
        let base = if base.is_empty() {
            "material".into()
        } else {
            base
        };
        let dir = self.registered_material_folder();
        let mut stem = base.clone();
        let mut path = dir.join(format!("{stem}.png"));
        let mut n = 1;
        while path.exists() {
            n += 1;
            stem = format!("{base}-{n}");
            path = dir.join(format!("{stem}.png"));
        }
        image::save_buffer(&path, img.as_raw(), w, h, image::ExtendedColorType::Rgba8).ok()?;
        self.materials_scan();
        Some((path, stem))
    }

    /// Row 151's bulk half: copy every image file from `src` into the
    /// registered folder (existing names kept — no clobbering). Returns
    /// the number imported.
    pub fn material_import_folder(&mut self, src: &Path) -> usize {
        let dst = self.registered_material_folder();
        let Ok(rd) = std::fs::read_dir(src) else {
            return 0;
        };
        let mut n = 0;
        for e in rd.flatten() {
            let p = e.path();
            let ext = p
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.to_ascii_lowercase())
                .unwrap_or_default();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
                continue;
            }
            let Some(name) = p.file_name() else { continue };
            let target = dst.join(name);
            if target.exists() {
                continue;
            }
            if std::fs::copy(&p, &target).is_ok() {
                n += 1;
            }
        }
        if n > 0 {
            self.materials_scan();
        }
        n
    }
}
