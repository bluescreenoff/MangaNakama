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
//! paste-size modes (MT-032), order-in-layer metadata (MT-034).
//!
//! MT-012 (tags) lands as a per-folder `tags.txt` sidecar — see
//! [`MaterialTags`].

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
    /// MT-012: the folder sidecar's tag line for this file, normalised
    /// (`"screentone, dots, 10%"`). Empty = untagged, which is what every
    /// material was before this existed.
    pub tags: String,
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
            // One sidecar read per FOLDER, not per file. The sidecar is the
            // only home of a tag, so a rescan re-reads it rather than
            // remembering: tags on materials the owner adds by hand survive
            // every rescan, restart and folder re-add.
            let side = MaterialTags::load(folder);
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
                    let tags = side.get(&p.file_name()?.to_string_lossy()).to_owned();
                    Some(MaterialItem {
                        name,
                        path: p,
                        folder: fi,
                        tags,
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

// --- MT-012: tags, as a per-folder sidecar -------------------------------

/// The sidecar's file name — one per material folder, beside the images.
pub const TAGS_FILE: &str = "tags.txt";

/// A material folder's `tags.txt`: `<file name>=<comma, separated, tags>`
/// lines, the same plain-text idiom as `prefs.txt` and `ui.txt` (no json,
/// no yaml, no new dependency). It lives WITH the images, so copying a
/// material folder to another machine copies its tags, and a folder the
/// bank has never seen is already tagged the first time it is added.
///
/// Example:
///
/// ```text
/// tone-dots-10pct.png=screentone, dots, light
/// speed-lines.png=effect, action
/// ```
///
/// **Unknown content survives a rewrite, both kinds** (the discipline
/// `prefs.rs` documents): a line this build cannot read — a comment, a key
/// a newer build wrote without an `=` — is kept verbatim and written back,
/// and an entry naming a file that is not in the folder right now is kept
/// too. That second half is what makes the file safe against a bank
/// rescan, a temporarily-renamed image, or a drive that was not mounted.
///
/// The key is the file NAME with its extension (`a.png` and `a.jpg` are
/// two different materials). A file name containing `=` is the one shape
/// this cannot address — the first `=` splits the line, as everywhere else
/// in the repo's `k=v` files.
#[derive(Default, Debug, Clone)]
pub struct MaterialTags {
    entries: std::collections::BTreeMap<String, String>,
    /// Lines with no usable key, kept so a rewrite does not eat them.
    unknown: Vec<String>,
}

/// `" a , b,,c "` → `"a, b, c"`. Commas (and stray newlines from a paste)
/// separate; empty tags vanish. A tag never contains a comma.
fn normalize_tags(tags: &str) -> String {
    tags.split([',', '\n', '\r'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

impl MaterialTags {
    pub fn parse(text: &str) -> Self {
        let mut me = Self::default();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match line.split_once('=') {
                Some((k, v)) if !k.trim().is_empty() => {
                    me.entries.insert(k.trim().to_owned(), normalize_tags(v));
                }
                _ => me.unknown.push(line.to_owned()),
            }
        }
        me
    }

    /// The whole file: our entries (sorted, so a rewrite is a stable diff),
    /// then every line we did not understand, verbatim.
    pub fn to_body(&self) -> String {
        let mut s = String::new();
        for (k, v) in &self.entries {
            s.push_str(k);
            s.push('=');
            s.push_str(v);
            s.push('\n');
        }
        for line in &self.unknown {
            s.push_str(line);
            s.push('\n');
        }
        s
    }

    /// The tag line for one file name; `""` when it has no entry.
    pub fn get(&self, file_name: &str) -> &str {
        self.entries
            .get(file_name)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Empty tags REMOVE the entry — a material whose tags were cleared must
    /// end up byte-identical to one that was never tagged.
    pub fn set(&mut self, file_name: &str, tags: &str) {
        let v = normalize_tags(tags);
        if v.is_empty() {
            self.entries.remove(file_name);
        } else {
            self.entries.insert(file_name.to_owned(), v);
        }
    }

    /// A missing (or unreadable) sidecar is not an error — it is a folder
    /// with no tags, which is exactly what every folder was before.
    pub fn load(folder: &Path) -> Self {
        std::fs::read_to_string(folder.join(TAGS_FILE))
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }

    /// Write the sidecar back. Nothing left to say (last tag cleared, no
    /// unknown lines) deletes the file instead of leaving an empty one, so
    /// "no sidecar" stays the honest resting state. Returns false only when
    /// the disk actually refused.
    pub fn save(&self, folder: &Path) -> bool {
        let p = folder.join(TAGS_FILE);
        if self.entries.is_empty() && self.unknown.is_empty() {
            return std::fs::remove_file(&p).is_ok() || !p.exists();
        }
        std::fs::write(&p, self.to_body()).is_ok()
    }
}

/// The bank's ONE search box matches a material's name or its tags — no
/// second box, no `tag:` prefix syntax. `needle` must already be lowercase.
pub fn material_matches(item: &MaterialItem, needle: &str) -> bool {
    item.name.to_lowercase().contains(needle) || item.tags.to_lowercase().contains(needle)
}

impl App {
    /// Retag one material: rewrite its folder's sidecar and update the bank
    /// in place. In place rather than `materials_scan()` because a rescan
    /// throws away every decoded thumbnail — the palette must not blink
    /// because you typed a tag. Returns false if the sidecar could not be
    /// written (the caller says so in the status bar; the bank then still
    /// shows what is really on disk).
    pub fn material_set_tags(&mut self, path: &Path, tags: &str) -> bool {
        let (Some(folder), Some(file)) = (path.parent(), path.file_name()) else {
            return false;
        };
        let file = file.to_string_lossy().into_owned();
        let mut side = MaterialTags::load(folder);
        side.set(&file, tags);
        if !side.save(folder) {
            return false;
        }
        let now = side.get(&file).to_owned();
        for m in self.materials.iter_mut().filter(|m| m.path == path) {
            m.tags = now.clone();
        }
        true
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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, tags: &str) -> MaterialItem {
        MaterialItem {
            name: name.to_owned(),
            path: PathBuf::from(format!("C:/m/{name}.png")),
            folder: 0,
            tags: tags.to_owned(),
        }
    }

    /// The sidecar's forward-compatibility contract, both halves: a line
    /// this build cannot read is written back verbatim, and an entry for a
    /// material that is not in the folder right now is kept — a rescan (or
    /// an unplugged drive) must never be able to eat someone's tags.
    #[test]
    fn tags_sidecar_roundtrips_unknown_lines_and_absent_materials() {
        let body = "# hand-written notes about this folder\n\
                    tone-dots-10pct.png=screentone, dots\n\
                    gone-for-now.png=effect, action\n\
                    a line from a 2027 build with no equals sign\n";
        let mut side = MaterialTags::parse(body);
        assert_eq!(side.get("tone-dots-10pct.png"), "screentone, dots");

        // This build retags one file and saves.
        side.set("tone-dots-10pct.png", "screentone, dots, light");
        let out = side.to_body();
        assert!(
            out.contains("# hand-written notes about this folder\n"),
            "a comment must survive: {out}"
        );
        assert!(
            out.contains("a line from a 2027 build with no equals sign\n"),
            "…and every other unreadable line: {out}"
        );
        assert!(
            out.contains("gone-for-now.png=effect, action\n"),
            "an entry for a file not in the folder must survive: {out}"
        );
        assert!(out.contains("tone-dots-10pct.png=screentone, dots, light\n"));

        // A second trip is stable — nothing doubles up, nothing is dropped.
        assert_eq!(MaterialTags::parse(&out).to_body(), out);
    }

    /// Whitespace and empty tags are normalised on the way in, and CLEARING
    /// a material's tags removes the entry rather than leaving `name=`, so
    /// "cleared" and "never tagged" are the same bytes.
    #[test]
    fn tags_normalise_and_clearing_removes_the_entry() {
        let mut side = MaterialTags::default();
        side.set("a.png", "  dots ,, light,  ");
        assert_eq!(side.get("a.png"), "dots, light");
        assert_eq!(side.to_body(), "a.png=dots, light\n");

        side.set("a.png", "   ");
        assert_eq!(side.get("a.png"), "");
        assert_eq!(side.to_body(), "", "cleared == never tagged");
    }

    /// The one search box matches tags as well as names, case-insensitively
    /// — and an untagged material behaves exactly as it did before tags
    /// existed (name match only, never a spurious hit).
    #[test]
    fn search_matches_tags_as_well_as_names() {
        let dots = item("tone-dots-10pct", "Screentone, Light");
        let plain = item("speed-lines", "");
        assert!(material_matches(&dots, "dots"), "name still matches");
        assert!(material_matches(&dots, "screentone"), "tag matches");
        assert!(material_matches(&dots, "light"), "a later tag matches too");
        assert!(!material_matches(&dots, "balloon"));
        assert!(material_matches(&plain, "speed"));
        assert!(
            !material_matches(&plain, "screentone"),
            "an untagged material must not match another material's tag"
        );
        assert!(
            material_matches(&plain, ""),
            "an empty box shows everything, as before"
        );
    }

    /// No sidecar on disk = today's behaviour: every material scans with
    /// empty tags, and reading the folder writes nothing.
    #[test]
    fn a_folder_with_no_sidecar_is_untagged_and_stays_that_way() {
        let dir = std::env::temp_dir().join(format!("mn-tags-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let side = MaterialTags::load(&dir);
        assert_eq!(side.get("anything.png"), "");
        assert_eq!(side.to_body(), "");
        assert!(
            !dir.join(TAGS_FILE).exists(),
            "loading must never create the file"
        );
        // Saving an empty sidecar likewise leaves the folder untouched.
        assert!(side.save(&dir));
        assert!(!dir.join(TAGS_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
