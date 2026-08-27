//! Organise brush presets in place (the ROADMAP's "brushes and materials
//! without ceremony", manage half): Rename / Duplicate / Delete on the Sub
//! Tool list's right-click menu, for the presets the artist owns.
//!
//! Only `mine/` and `imported/` are editable, and every handler re-checks
//! that itself instead of trusting the caller: a rename in a shipped group
//! would come back with the next build, and a delete there would not come
//! back at all.
//!
//! Rename edits the .myb's `"name"` field and NEVER the file name. The
//! per-sub-tool size persisted in `ui.txt` is keyed on the preset's path
//! relative to the brushes root (`App::preset_key`), so renaming the file
//! would silently throw away the size the artist set on that brush.

use std::path::{Path, PathBuf};

use crate::app::App;
use crate::cmd::AppCmd;

/// The groups the artist owns — their own captures and their imports.
const OWNED: [&str; 2] = ["mine", "imported"];

impl App {
    /// Sub Tool ▸ right-click ▸ Rename. Sets its own status lines; the
    /// dispatch arm only has to call it.
    pub fn rename_brush(&mut self, path: PathBuf, name: String) {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return self.set_error("rename: a brush needs a name");
        }
        let Some(root) = self.brushes_root.clone() else {
            return self.set_error("rename: no brushes folder found");
        };
        if !self.rename_brush_into(root, &path, &name) {
            return self.set_error(refusal("rename", &path));
        }
        self.rescan_keeping_selection();
        self.set_status(format!(
            "renamed to \"{name}\" — the file is unchanged, so this brush keeps its saved size"
        ));
    }

    /// Sub Tool ▸ right-click ▸ Duplicate.
    pub fn duplicate_brush(&mut self, path: PathBuf) {
        let Some(root) = self.brushes_root.clone() else {
            return self.set_error("duplicate: no brushes folder found");
        };
        let Some((_, name)) = self.duplicate_brush_into(root, &path) else {
            return self.set_error(refusal("duplicate", &path));
        };
        self.rescan_keeping_selection();
        self.set_status(format!(
            "duplicated as \"{name}\" — tune it in Tool Property"
        ));
    }

    /// Sub Tool ▸ right-click ▸ Delete. No confirm dialog, so the status
    /// line has to say what went and that nothing brings it back.
    pub fn delete_brush(&mut self, path: PathBuf) {
        let Some(root) = self.brushes_root.clone() else {
            return self.set_error("delete: no brushes folder found");
        };
        let was_selected = self
            .selected_preset
            .and_then(|i| self.presets.get(i))
            .is_some_and(|(_, p)| *p == path);
        let Some(name) = self.delete_brush_into(root, &path) else {
            return self.set_error(refusal("delete", &path));
        };
        self.rescan_keeping_selection();
        // The selected sub tool just stopped existing: land on a real brush
        // rather than leaving the pen pointed at nothing.
        if was_selected && let Some((_, first)) = self.presets.first() {
            let first = first.clone();
            self.push_cmd(AppCmd::SelectBrush(first));
        }
        self.set_status(format!(
            "deleted brush \"{name}\" (file removed — nothing to undo)"
        ));
    }

    /// The write half, brushes root injected so tests stay out of the real
    /// assets (the split `register_brush_from_selection_into` uses). Returns
    /// whether the preset's `"name"` was rewritten.
    pub(crate) fn rename_brush_into(&mut self, root: PathBuf, path: &Path, name: &str) -> bool {
        owned(&root, path) && write_named(path, path, name)
    }

    /// Copy the preset to the next free `<prefix>-N.myb` beside it. Returns
    /// the copy's path and display name. The `mn-texture` reference rides
    /// along unchanged — the tip mask is shared, not owned.
    pub(crate) fn duplicate_brush_into(
        &mut self,
        root: PathBuf,
        path: &Path,
    ) -> Option<(PathBuf, String)> {
        if !owned(&root, path) {
            return None;
        }
        let dir = path.parent()?;
        let stem = path.file_stem()?.to_str()?;
        let dst = free_copy_path(dir, stem);
        let name = format!("{} copy", display_name(path));
        write_named(path, &dst, &name).then_some((dst, name))
    }

    /// Remove the .myb. The texture PNG stays: `textures/` is shared, and a
    /// duplicate or an import can point at the same mask.
    pub(crate) fn delete_brush_into(&mut self, root: PathBuf, path: &Path) -> Option<String> {
        if !owned(&root, path) {
            return None;
        }
        let name = display_name(path);
        std::fs::remove_file(path).ok()?;
        // The row is gone, so is its cached stroke.
        self.brush_previews.remove(path);
        Some(name)
    }

    /// Brush shape presets (B-009..013), our v1: "Save current settings
    /// as brush" — the selected preset cloned into `mine/` with the
    /// CURRENT Tool Property state folded back into the file, so the
    /// brush as tuned becomes a named sub tool of its own. Applying a
    /// shape = picking the saved preset (CSP separates shape from
    /// colour; our preset is the whole brush — the recorded v1
    /// deviation). The source may be a SHIPPED preset: tuning a stock
    /// brush and keeping the tuned copy is the whole point, and the copy
    /// always lands in owned territory.
    pub fn save_current_brush(&mut self) {
        let Some(root) = self.brushes_root.clone() else {
            return self.set_error("save brush: no brushes folder found");
        };
        let Some(src) = self
            .selected_preset
            .and_then(|i| self.presets.get(i))
            .map(|(_, p)| p.clone())
        else {
            return self.set_error("save brush: no sub tool selected");
        };
        let props = self.props_current;
        let textures = self.texture_names.clone();
        let Some((path, name)) = self.save_current_into(root, &src, &props, &textures) else {
            return self.set_error("save brush: the preset could not be read");
        };
        self.rescan_keeping_selection();
        if let Some((_, p)) = self.presets.iter().find(|(_, p)| *p == path) {
            let p = p.clone();
            self.push_cmd(AppCmd::SelectBrush(p));
        }
        self.set_status(format!(
            "saved as \"{name}\" in Mine — the tuned brush is a sub tool of its own now"
        ));
    }

    /// The write half, root injected like its siblings. Returns the new
    /// file and its display name.
    pub(crate) fn save_current_into(
        &self,
        root: PathBuf,
        src: &Path,
        props: &crate::cmd::ToolProps,
        texture_names: &[String],
    ) -> Option<(PathBuf, String)> {
        let mut json = read_preset(src)?;
        let mine = root.join("mine");
        std::fs::create_dir_all(&mine).ok()?;
        let stem = src.file_stem()?.to_str()?;
        let dst = free_copy_path(&mine, stem);
        let name = format!("{} copy", display_name(src));
        json["name"] = serde_json::json!(name);
        json["group"] = serde_json::json!("mine");
        if !json.get("settings").is_some_and(|s| s.is_object()) {
            json["settings"] = serde_json::json!({});
        }
        let settings = json.get_mut("settings")?.as_object_mut()?;
        let set = |s: &mut serde_json::Map<String, serde_json::Value>, k: &str, v: f64| {
            s.insert(k.into(), serde_json::json!({ "base_value": v }));
        };
        // Size: the px rail inverted through the same ln the imports use.
        set(settings, "radius_logarithmic", super::abr::rlog(props.size_px as f64));
        // Wash pair: the sliders' current split, or the plain per-dab
        // opacity in build-up.
        json["mn-wash"] = serde_json::json!(props.wash);
        if props.wash {
            json["mn-wash-opacity"] = serde_json::json!(props.opacity);
            set(settings, "opaque", props.flow as f64);
        } else {
            set(settings, "opaque", props.opacity as f64);
        }
        json["mn-hard-dab"] = serde_json::json!(props.hard_dab);
        json["mn-scatter"] = serde_json::json!(props.scatter);
        if props.brush_blend != mn_core::Blend::Normal {
            json["mn-brush-blend"] = serde_json::json!(props.brush_blend.short_name());
        } else {
            json.as_object_mut()?.remove("mn-brush-blend");
        }
        match props.brush_draw.key_name() {
            Some(k) => {
                json["mn-brush-draw"] = serde_json::json!(k);
            }
            None => {
                json.as_object_mut()?.remove("mn-brush-draw");
            }
        }
        match props.texture {
            0 => {
                let o = json.as_object_mut()?;
                o.remove("mn-texture");
                o.remove("mn-texture-scroll");
                o.remove("mn-texture-rotate");
            }
            n => {
                if let Some(t) = texture_names.get(n as usize - 1) {
                    json["mn-texture"] = serde_json::json!(t);
                    json["mn-texture-scroll"] = serde_json::json!(props.texture_scroll);
                    let r = match props.texture_rotate {
                        mn_brush::TextureRotate::Fixed => "fixed",
                        mn_brush::TextureRotate::Direction => "direction",
                        mn_brush::TextureRotate::Tilt => "tilt",
                    };
                    json["mn-texture-rotate"] = serde_json::json!(r);
                }
            }
        }
        serde_json::to_string_pretty(&json)
            .ok()
            .and_then(|text| std::fs::write(&dst, text).ok())
            .is_some()
            .then_some((dst, name))
    }

    /// Rescan and keep the selection on the same PRESET: every index above a
    /// created or deleted file moves, and `selected_preset` is an index.
    fn rescan_keeping_selection(&mut self) {
        let was = self
            .selected_preset
            .and_then(|i| self.presets.get(i))
            .map(|(_, p)| p.clone());
        self.presets = super::scan_presets();
        self.selected_preset = was.and_then(|p| self.presets.iter().position(|(_, q)| *q == p));
    }
}

/// True for a `.myb` sitting directly in `<root>/mine` or `<root>/imported`.
/// Path-shaped rather than name-shaped on purpose: this is the check that
/// stops a command carrying an arbitrary path from deleting an arbitrary
/// file.
fn owned(root: &Path, path: &Path) -> bool {
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("myb"))
    {
        return false;
    }
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let mut parts = rel.components();
    let Some(group) = parts.next().and_then(|c| c.as_os_str().to_str()) else {
        return false;
    };
    // <group>/<file>.myb and nothing deeper — no `..` walking back out.
    parts.next().is_some() && parts.next().is_none() && OWNED.contains(&group)
}

/// What the picker calls a preset: its `"name"` field, else the file stem
/// (the same rule `BrushLibrary` displays by).
fn display_name(path: &Path) -> String {
    read_preset(path)
        .and_then(|j| {
            j.get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
}

fn read_preset(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.is_object().then_some(json)
}

/// Write `src`'s preset to `dst` under a new display name. `src == dst` is
/// the rename; a different `dst` is the duplicate.
fn write_named(src: &Path, dst: &Path, name: &str) -> bool {
    let Some(mut json) = read_preset(src) else {
        return false;
    };
    json["name"] = serde_json::json!(name);
    serde_json::to_string_pretty(&json)
        .ok()
        .and_then(|text| std::fs::write(dst, text).ok())
        .is_some()
}

/// The next free `<prefix>-N.myb` beside the original, where the prefix is
/// the stem without its own trailing number: `mine-3` copies to `mine-4`,
/// not to `mine-3-1`. Numbering off the highest index in the folder (the
/// `next_index` rule) means a duplicate never lands on a name in use, even
/// after deletions in the middle of the run.
fn free_copy_path(dir: &Path, stem: &str) -> PathBuf {
    let prefix = stem
        .rsplit_once('-')
        .filter(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        .map_or(stem, |(head, _)| head);
    let mut max = 0usize;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            if let Some(n) = file
                .strip_prefix(&format!("{prefix}-"))
                .and_then(|s| s.strip_suffix(".myb"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                max = max.max(n);
            }
        }
    }
    dir.join(format!("{prefix}-{}.myb", max + 1))
}

/// One refusal line for all three verbs — the reason is always the same.
fn refusal(verb: &str, path: &Path) -> String {
    format!(
        "{verb}: \"{}\" is a shipped brush — only Mine and Imported can be organised",
        display_name(path)
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::owned;
    use crate::app::{App, headless_renderer};

    fn tmp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mn-brushmng-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn preset(root: &Path, group: &str, stem: &str, name: &str) -> PathBuf {
        let dir = root.join(group);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{stem}.myb"));
        let json = serde_json::json!({
            "comment": "MyPaint brush file",
            "name": name,
            "group": group,
            "mn-texture": "mine-1",
            "settings": {},
            "version": 3
        });
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
        path
    }

    fn name_of(path: &Path) -> String {
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        json["name"].as_str().unwrap().to_owned()
    }

    /// Rename touches the JSON only: the FILE keeps its stem, because the
    /// persisted sub-tool size is keyed on the path.
    #[test]
    fn rename_edits_the_json_name_and_keeps_the_file() {
        let Some(renderer) = headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (600, 400), 1.0);
        let root = tmp_root("rename");
        let path = preset(&root, "mine", "mine-1", "Canvas brush 1");
        assert!(app.rename_brush_into(root.clone(), &path, "Rough inker"));
        assert!(path.exists(), "the file survives its own rename");
        assert_eq!(name_of(&path), "Rough inker");
        // The name the picker shows follows the JSON, not the stem.
        let found = mn_brush::BrushLibrary::scan(&root);
        assert_eq!(found, vec![("Rough inker".to_owned(), path)]);
    }

    /// A duplicate numbers up and never writes over a sibling.
    #[test]
    fn duplicate_numbers_up_and_never_overwrites() {
        let Some(renderer) = headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (600, 400), 1.0);
        let root = tmp_root("dup");
        let path = preset(&root, "mine", "mine-1", "Canvas brush 1");
        let (first, name) = app
            .duplicate_brush_into(root.clone(), &path)
            .expect("a mine/ preset duplicates");
        assert_eq!(first, root.join("mine/mine-2.myb"));
        assert_eq!(name, "Canvas brush 1 copy");
        assert_eq!(
            name_of(&path),
            "Canvas brush 1",
            "the original is untouched"
        );
        // The shared tip mask rides along — textures are not owned by one
        // preset.
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&first).unwrap()).unwrap();
        assert_eq!(json["mn-texture"], "mine-1");
        // Same source again: the taken -2 is stepped over, not clobbered.
        let (second, _) = app.duplicate_brush_into(root.clone(), &path).unwrap();
        assert_eq!(second, root.join("mine/mine-3.myb"));
        assert_eq!(
            name_of(&first),
            "Canvas brush 1 copy",
            "the first copy is not overwritten by the second"
        );
    }

    /// Delete removes the .myb and nothing else, and the rescan drops the row.
    #[test]
    fn delete_removes_only_the_myb() {
        let Some(renderer) = headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (600, 400), 1.0);
        let root = tmp_root("delete");
        let path = preset(&root, "mine", "mine-1", "Canvas brush 1");
        let keep = preset(&root, "mine", "mine-2", "Canvas brush 2");
        std::fs::create_dir_all(root.join("textures")).unwrap();
        let tex = root.join("textures/mine-1.png");
        std::fs::write(&tex, b"not really a png").unwrap();

        assert_eq!(
            app.delete_brush_into(root.clone(), &path).as_deref(),
            Some("Canvas brush 1")
        );
        assert!(!path.exists());
        assert!(tex.exists(), "the tip mask can be shared — it stays");
        // The rescan the handler runs (same call as `scan_presets`).
        let found = mn_brush::BrushLibrary::scan(&root);
        assert_eq!(found, vec![("Canvas brush 2".to_owned(), keep)]);
    }

    /// Shipped groups and anything outside the brushes root are refused by
    /// the handlers themselves, not merely hidden by the menu.
    #[test]
    fn refuses_shipped_groups_and_paths_outside_the_root() {
        let Some(renderer) = headless_renderer() else {
            return;
        };
        let mut app = App::new(renderer, (600, 400), 1.0);
        let root = tmp_root("refuse");
        let shipped = preset(&root, "csp", "kabura", "カブラペン");
        let stray = tmp_root("refuse-outside").join("stray.myb");
        std::fs::write(&stray, "{\"name\":\"stray\"}").unwrap();

        for path in [&shipped, &stray] {
            assert!(!app.rename_brush_into(root.clone(), path, "hijacked"));
            assert!(app.duplicate_brush_into(root.clone(), path).is_none());
            assert!(app.delete_brush_into(root.clone(), path).is_none());
            assert!(path.exists(), "a refused delete leaves the file alone");
        }
        assert_eq!(name_of(&shipped), "カブラペン");
        // Escapes and nested paths are refused too: `owned` is the gate, and
        // it only ever says yes to <root>/<mine|imported>/<file>.myb.
        assert!(!owned(&root, &root.join("mine/deep/x.myb")));
        assert!(!owned(&root, &root.join("mine/../csp/kabura.myb")));
        assert!(owned(&root, &root.join("imported/set-1.myb")));
    }

    /// Brush shape presets v1: the tuned state lands in `mine/` as a
    /// preset that loads back with exactly those settings — from a
    /// SHIPPED source too (tune a stock brush, keep the tuned copy).
    #[test]
    fn save_current_folds_the_tuned_state_into_a_new_preset() {
        let Some(renderer) = headless_renderer() else {
            return;
        };
        let app = App::new(renderer, (600, 400), 1.0);
        let root = tmp_root("savecur");
        // A SHIPPED source with its own texture reference.
        let src = preset(&root, "csp", "kabura", "カブラペン");
        let mut props = crate::cmd::ToolProps::default();
        props.size_px = 42.0;
        props.wash = true;
        props.opacity = 0.7;
        props.flow = 0.35;
        props.brush_blend = mn_core::Blend::LinearBurn;
        props.brush_draw = mn_brush::BrushDraw::Background;
        props.texture = 1;
        props.texture_scroll = 3.0;
        props.texture_rotate = mn_brush::TextureRotate::Tilt;
        let names = vec!["grain-7".to_owned()];
        let (path, name) = app
            .save_current_into(root.clone(), &src, &props, &names)
            .expect("the shipped preset saves into mine/");
        assert_eq!(path, root.join("mine/kabura-1.myb"));
        assert_eq!(name, "カブラペン copy");
        assert!(src.exists(), "the shipped original is untouched");

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["group"], "mine");
        assert_eq!(json["mn-wash"], true);
        assert_eq!(json["mn-wash-opacity"], 0.7);
        assert_eq!(json["mn-brush-blend"], "linear-burn");
        assert_eq!(json["mn-brush-draw"], "background");
        assert_eq!(json["mn-texture"], "grain-7");
        assert_eq!(json["mn-texture-scroll"], 3.0);
        assert_eq!(json["mn-texture-rotate"], "tilt");
        let rl = json["settings"]["radius_logarithmic"]["base_value"]
            .as_f64()
            .unwrap();
        assert!((rl - (42.0 / 2.0).ln()).abs() < 1e-6, "size inverted: {rl}");

        // The saved preset LOADS with the tuned state (texture absent on
        // disk is fine — an unresolvable name keeps the brush usable).
        let b = mn_brush::MyBrush::load(&path).expect("loads");
        assert!(b.wash());
        assert!((b.wash_opacity() - 0.7).abs() < 1e-6);
        assert_eq!(b.wash_blend(), mn_core::Blend::LinearBurn);
        assert_eq!(b.wash_draw(), mn_brush::BrushDraw::Background);

        // Texture OFF drops the keys: the saved brush draws untextured.
        props.texture = 0;
        let (path2, _) = app
            .save_current_into(root.clone(), &src, &props, &names)
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path2).unwrap()).unwrap();
        assert!(json.get("mn-texture").is_none());
        assert!(json.get("mn-texture-rotate").is_none());
        // Normal blend leaves no stale key behind.
        props.brush_blend = mn_core::Blend::Normal;
        let (path3, _) = app
            .save_current_into(root.clone(), &src, &props, &names)
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path3).unwrap()).unwrap();
        assert!(json.get("mn-brush-blend").is_none());
    }
}
