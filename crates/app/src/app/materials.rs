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

use mn_core::genlines::GenLinesSpec;

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
    /// The material's folder-relative **directory chain** — empty for a file
    /// sitting in the registered root, `sfx/impact` two levels down. The
    /// file name is deliberately not in here: every consumer (a tree node, a
    /// subtree filter, the info strip's "where does this live" line) wants
    /// the containing folder, and `path` already carries the name.
    pub rel: PathBuf,
    /// MT-012: the folder sidecar's tag line for this file, normalised
    /// (`"screentone, dots, 10%"`). Empty = untagged, which is what every
    /// material was before this existed.
    pub tags: String,
    /// What a click PLACES — a bitmap float, or a live generator layer.
    pub kind: MaterialKind,
}

/// A material's two flavours. Bitmaps are the original bank (paste as the
/// move/scale float); a generator material places the thing itself — a
/// layer carrying its [`GenLinesSpec`], editable with the Object tool from
/// the first click instead of baked pixels nobody can re-aim.
#[derive(Clone, Debug, PartialEq)]
pub enum MaterialKind {
    Image,
    GenLines(GenLinesSpec),
}

/// A generator material's file suffix. `focus-lines.gen.json` IS the
/// material (a serialized [`GenLinesSpec`]); a `focus-lines.png` beside it
/// is only that material's THUMBNAIL, never a second material.
pub const GEN_SUFFIX: &str = ".gen.json";

impl MaterialItem {
    pub fn is_generator(&self) -> bool {
        matches!(self.kind, MaterialKind::GenLines(_))
    }

    /// Where the palette cell's picture comes from: the same-stem PNG for
    /// a generator material, the image itself for a bitmap. A generator
    /// with no PNG beside it simply shows as a name (the decode fails, as
    /// it always has for an unreadable image).
    pub fn thumb_path(&self) -> PathBuf {
        match self.kind {
            MaterialKind::Image => self.path.clone(),
            MaterialKind::GenLines(_) => self.path.with_file_name(format!("{}.png", self.name)),
        }
    }
}

/// `focus-lines.gen.json` → `focus-lines`; anything else → `None`.
pub fn gen_material_stem(path: &Path) -> Option<String> {
    let n = path.file_name()?.to_str()?;
    // The lowercased copy is byte-for-byte the same length, so the split
    // below can never land inside a multi-byte character.
    n.to_ascii_lowercase()
        .ends_with(GEN_SUFFIX)
        .then(|| n[..n.len() - GEN_SUFFIX.len()].to_owned())
}

/// The spec a generator material carries. `None` when the path is not a
/// generator material at all, or when the file cannot be read or parsed —
/// a corrupt sidecar is not a material, and must not become an empty one.
pub fn read_gen_spec(path: &Path) -> Option<GenLinesSpec> {
    gen_material_stem(path)?;
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Write `<stem>.gen.json` into `dir`, returning the path on success.
pub fn write_gen_spec(dir: &Path, stem: &str, spec: &GenLinesSpec) -> Option<PathBuf> {
    let p = dir.join(format!("{stem}{GEN_SUFFIX}"));
    let text = serde_json::to_string_pretty(spec).ok()?;
    std::fs::write(&p, text).ok()?;
    Some(p)
}

/// How deep a registered folder is walked. A material bank is a shallow
/// thing — CSP's own tree is three levels — so this is really a stop, not a
/// budget: a directory junction pointing back at an ancestor would
/// otherwise recurse until the stack ran out. (Symlinks cannot start that
/// loop at all: `file_type()` reports them as neither dir nor file, so they
/// fall through to the extension filter and are dropped.)
const SCAN_MAX_DEPTH: usize = 6;

/// One registered folder's materials, its subdirectories included, in
/// display order. Free-standing so the scan can be exercised against a temp
/// folder without an App (and a GPU).
pub fn materials_scan_folder(folder: &Path, fi: usize) -> Vec<MaterialItem> {
    let mut items = Vec::new();
    scan_dir(folder, PathBuf::new(), fi, 0, &mut items);
    // (rel, name): a FLAT folder has an empty `rel` on every item, so this
    // is byte-for-byte the old name-only order — the recursive scan changes
    // nothing for the banks that have no subfolders.
    items.sort_by(|a, b| (&a.rel, &a.name).cmp(&(&b.rel, &b.name)));
    items
}

/// The walk's one directory. `rel` is where we are relative to the
/// registered root, and is what every item found here is stamped with.
fn scan_dir(dir: &Path, rel: PathBuf, fi: usize, depth: usize, out: &mut Vec<MaterialItem>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    // One sidecar read per DIRECTORY, not per file. The sidecar is the
    // only home of a tag, so a rescan re-reads it rather than
    // remembering: tags on materials the owner adds by hand survive
    // every rescan, restart and folder re-add. A subfolder carries its
    // own `tags.txt` — the sidecar travels WITH the images it describes.
    let side = MaterialTags::load(dir);
    // `file_type()` comes off the directory entry on both platforms, so
    // splitting here costs nothing; `p.is_dir()` would stat all 2399.
    let (mut dirs, mut paths): (Vec<PathBuf>, Vec<PathBuf>) = (Vec::new(), Vec::new());
    for e in rd.flatten() {
        match e.file_type() {
            Ok(t) if t.is_dir() => dirs.push(e.path()),
            Ok(_) => paths.push(e.path()),
            Err(_) => {}
        }
    }
    // Generator stems first: a `<stem>.png` beside a `<stem>.gen.json` is
    // that generator's thumbnail, so it must not scan as its own material.
    let gen_stems: std::collections::HashSet<String> = paths
        .iter()
        .filter_map(|p| gen_material_stem(p).map(|s| s.to_lowercase()))
        .collect();
    out.extend(paths.iter().filter_map(|p| {
        let (name, kind) = match gen_material_stem(p) {
            Some(stem) => (stem, MaterialKind::GenLines(read_gen_spec(p)?)),
            None => {
                let ext = p
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.to_ascii_lowercase())
                    .unwrap_or_default();
                if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
                    return None;
                }
                let name = p.file_stem()?.to_string_lossy().into_owned();
                if gen_stems.contains(&name.to_lowercase()) {
                    return None;
                }
                (name, MaterialKind::Image)
            }
        };
        let tags = side.get(&p.file_name()?.to_string_lossy()).to_owned();
        Some(MaterialItem {
            name,
            path: p.clone(),
            folder: fi,
            rel: rel.clone(),
            tags,
            kind,
        })
    }));
    if depth + 1 >= SCAN_MAX_DEPTH {
        return;
    }
    for d in dirs {
        let Some(n) = d.file_name() else { continue };
        // A dot-folder is the tool's business (`.git`, `.thumbnails`), not
        // the artist's; the bank should never show one.
        if n.to_string_lossy().starts_with('.') {
            continue;
        }
        scan_dir(&d, rel.join(n), fi, depth + 1, out);
    }
}

/// P1-4's three thumbnail sizes (small / medium / large), in px. Here
/// rather than in the palette because `App::new` seeds the field from it.
pub const THUMB_STEPS: [f32; 3] = [36.0, 52.0, 76.0];

/// How many decoded thumbnails stay resident. Each is its own GPU texture
/// (there is no atlas), so an uncapped cache is one allocation per file —
/// 2399 of them on the owner's bank. Scrolling back over an evicted cell
/// re-decodes it, which is a frame's work and invisible next to the alternative.
pub const THUMB_CACHE_MAX: usize = 300;

// --- the folder tree (P0-1) ----------------------------------------------

/// What the material grid is narrowed to — the tree row that is selected.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum MaterialFilter {
    /// The tree's root row: every registered folder's every material.
    #[default]
    All,
    /// A registered root's subtree: folder index + folder-relative
    /// directory. An empty directory means the root itself, and a directory
    /// always INCLUDES its subfolders (CSP shows one folder's contents; a
    /// bank whose leaves are three deep is unusable if the branches read
    /// empty).
    Dir(usize, PathBuf),
    /// A virtual name-prefix group — folder, directory, leading token. See
    /// [`materials_tree`]; this is ours, not CSP's.
    Prefix(usize, PathBuf, String),
}

impl MaterialFilter {
    /// A stable row id: the collapse set's key and the egui widget salt.
    /// Stable across a rescan, which is the point — collapsing a branch
    /// must survive pressing Rescan.
    pub fn id(&self) -> String {
        match self {
            Self::All => "all".to_owned(),
            Self::Dir(f, d) => format!("d{f}\u{1f}{}", d.display()),
            Self::Prefix(f, d, t) => format!("p{f}\u{1f}{}\u{1f}{t}", d.display()),
        }
    }

    pub fn accepts(&self, item: &MaterialItem) -> bool {
        match self {
            Self::All => true,
            // Component-wise, so `sfx2` is not a child of `sfx`.
            Self::Dir(f, d) => item.folder == *f && item.rel.starts_with(d),
            Self::Prefix(f, d, t) => {
                item.folder == *f
                    && item.rel == *d
                    && name_prefix_token(&item.name).as_deref() == Some(t.as_str())
            }
        }
    }
}

/// One row of the folder tree, emitted in **pre-order** — a node's whole
/// subtree follows it contiguously at a greater `depth`, which is what lets
/// the palette hide a collapsed branch by skipping rows until `depth` drops
/// back to the collapsed node's own.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialNode {
    pub label: String,
    pub depth: usize,
    pub filter: MaterialFilter,
    /// How many materials the row would show (a directory counts its whole
    /// subtree, so a branch never reads empty).
    pub count: usize,
    pub children: bool,
}

/// OUR ADDITION, not CSP: a flat folder past this many DIRECT materials
/// gets virtual name-prefix groups under it. CSP's answer to 2399 files in
/// one directory is "put them in subfolders", which is true and useless —
/// the owner's bank arrived flat and re-filing it by hand is a day's work.
/// Below the threshold a folder is browsable as-is and inventing groups
/// would only add rows.
const FLAT_GROUP_MIN: usize = 200;
/// …and only a token that actually names a family becomes a row. A handful
/// of files sharing a first word is noise, not a folder.
const FLAT_GROUP_TOKEN_MIN: usize = 8;

/// A material name's leading word, lowercased, for the virtual grouping.
/// Leading separators are skipped FIRST because the owner's own bank is
/// full of `_boom_ horizontal.png` — the empty token before that first `_`
/// names nothing and would group half the bank under it.
pub fn name_prefix_token(name: &str) -> Option<String> {
    const SEP: [char; 3] = ['_', '-', ' '];
    let t = name.trim_start_matches(SEP);
    let tok = &t[..t.find(SEP).unwrap_or(t.len())];
    // One character is an index letter, not a family name.
    (tok.chars().count() >= 2).then(|| tok.to_lowercase())
}

/// Build the palette's folder tree from the scanned items' `rel` paths.
/// Pure and free-standing so it can be tested without an App; the palette
/// caches the result on `App::material_tree` rather than rebuilding it per
/// frame (the counts are an O(dirs × items) sweep).
pub fn materials_tree(items: &[MaterialItem], folder_names: &[String]) -> Vec<MaterialNode> {
    let mut out = vec![MaterialNode {
        label: "All materials".to_owned(),
        depth: 0,
        filter: MaterialFilter::All,
        count: items.len(),
        children: !folder_names.is_empty(),
    }];
    for (f, fname) in folder_names.iter().enumerate() {
        // Directories keyed by their COMPONENT LIST, not by PathBuf: a
        // `Vec<String>` sorts a parent immediately before its own
        // descendants (pre-order), while raw path bytes do not — `a\b`
        // sorts after `aB` on Windows, which would split a subtree.
        let mut dirs: std::collections::BTreeSet<Vec<String>> = std::collections::BTreeSet::new();
        // The registered root always has a row, even with nothing in it:
        // "my folder is registered and empty" is information.
        dirs.insert(Vec::new());
        for it in items.iter().filter(|i| i.folder == f) {
            let comps: Vec<String> = it
                .rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            for n in 0..=comps.len() {
                dirs.insert(comps[..n].to_vec());
            }
        }
        for comps in &dirs {
            let rel: PathBuf = comps.iter().collect();
            let count = items
                .iter()
                .filter(|i| i.folder == f && i.rel.starts_with(&rel))
                .count();
            let has_sub = dirs
                .iter()
                .any(|c| c.len() == comps.len() + 1 && c.starts_with(comps.as_slice()));
            let groups = flat_prefix_groups(items, f, &rel);
            out.push(MaterialNode {
                // The registered folder's own name stays the top node.
                label: comps.last().cloned().unwrap_or_else(|| fname.clone()),
                depth: 1 + comps.len(),
                filter: MaterialFilter::Dir(f, rel.clone()),
                count,
                children: has_sub || !groups.is_empty(),
            });
            // Groups are emitted before the real subdirectories: both are
            // children of this node, and sibling order inside a subtree is
            // free — pre-order only requires the subtree stay contiguous.
            for (tok, n) in groups {
                out.push(MaterialNode {
                    label: tok.clone(),
                    depth: 2 + comps.len(),
                    filter: MaterialFilter::Prefix(f, rel.clone(), tok),
                    count: n,
                    children: false,
                });
            }
        }
    }
    out
}

/// The virtual groups for one directory, `(token, count)` sorted by token.
/// Empty unless the directory is big enough to need them.
fn flat_prefix_groups(items: &[MaterialItem], f: usize, rel: &Path) -> Vec<(String, usize)> {
    let direct = items.iter().filter(|i| i.folder == f && i.rel == *rel);
    let mut by: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut n = 0usize;
    for it in direct {
        n += 1;
        if let Some(t) = name_prefix_token(&it.name) {
            *by.entry(t).or_default() += 1;
        }
    }
    if n <= FLAT_GROUP_MIN {
        return Vec::new();
    }
    by.into_iter()
        .filter(|&(_, c)| c >= FLAT_GROUP_TOKEN_MIN)
        .collect()
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
            out.extend(materials_scan_folder(folder, fi));
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
        self.material_tree = materials_tree(&self.materials, &self.material_folder_names);
        // A filter (and a selection) is an INDEX into what was just thrown
        // away. Dropping a filter whose row no longer exists is what stops
        // Rescan from leaving the grid mysteriously empty; a filter that
        // survived keeps its row, so rescanning inside a folder stays put.
        if !self
            .material_tree
            .iter()
            .any(|n| n.filter == self.material_filter)
        {
            self.material_filter = MaterialFilter::All;
        }
        self.material_selected = None;
        self.material_thumbs.clear();
        self.material_thumb_lru.clear();
    }

    /// Note that a thumbnail was DRAWN this frame, and evict the coldest
    /// ones once the cache is over [`THUMB_CACHE_MAX`]. Eviction runs in a
    /// batch down to 7/8 of the cap, so a grid whose visible cells sit
    /// exactly on the line does not evict-and-re-mint one texture per frame.
    pub fn material_thumb_touch(&mut self, path: &Path) {
        self.material_thumb_tick += 1;
        self.material_thumb_lru
            .insert(path.to_path_buf(), self.material_thumb_tick);
        if self.material_thumbs.len() <= THUMB_CACHE_MAX {
            return;
        }
        let drop_n = self.material_thumbs.len() - (THUMB_CACHE_MAX - THUMB_CACHE_MAX / 8);
        let mut cold: Vec<(u64, PathBuf)> = self
            .material_thumbs
            .keys()
            .map(|p| {
                (
                    self.material_thumb_lru.get(p).copied().unwrap_or(0),
                    p.clone(),
                )
            })
            .collect();
        cold.sort_unstable_by_key(|(t, _)| *t);
        for (_, p) in cold.iter().take(drop_n) {
            self.material_thumbs.remove(p);
            self.material_thumb_lru.remove(p);
        }
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
    ///
    /// A layer that was GENERATED (effect lines) registers as a generator
    /// material instead: the PNG becomes its thumbnail and a `.gen.json`
    /// beside it carries the spec, so a tuned effect-line layer comes back
    /// out of the bank live rather than as flattened ink.
    pub fn material_register_layer(&mut self) -> Option<(PathBuf, String)> {
        let dir = self.registered_material_folder();
        let out = self.material_register_layer_into(dir)?;
        self.materials_scan();
        Some(out)
    }

    /// The write half, target-directory injected so tests stay out of the
    /// real bank (the same split `pattern_save_material` uses).
    fn material_register_layer_into(&mut self, dir: PathBuf) -> Option<(PathBuf, String)> {
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
        // A generated layer registers LIVE — the spec is the material, the
        // PNG only its thumbnail, so both names have to be free.
        let spec = self.doc.active_layer().genlines;
        let taken = |dir: &Path, stem: &str| {
            dir.join(format!("{stem}.png")).exists()
                || (spec.is_some() && dir.join(format!("{stem}{GEN_SUFFIX}")).exists())
        };
        let mut stem = base.clone();
        let mut n = 1;
        while taken(&dir, &stem) {
            n += 1;
            stem = format!("{base}-{n}");
        }
        let path = dir.join(format!("{stem}.png"));
        image::save_buffer(&path, img.as_raw(), w, h, image::ExtendedColorType::Rgba8).ok()?;
        // The generator sidecar is the material's identity, so it — not the
        // thumbnail — is what the caller reports.
        let path = match spec {
            Some(s) => write_gen_spec(&dir, &stem, &s).unwrap_or(path),
            None => path,
        };
        Some((path, stem))
    }

    /// Row 151's bulk half: copy every image file from `src` into the
    /// registered folder (existing names kept — no clobbering), carrying
    /// the copied files' tags with them. Returns the number imported.
    pub fn material_import_folder(&mut self, src: &Path) -> usize {
        let dst = self.registered_material_folder();
        let copied = material_import_files(src, &dst);
        material_import_tags(src, &dst, &copied);
        if !copied.is_empty() {
            self.materials_scan();
        }
        copied.len()
    }
}

/// The file half of the import: copy every material file from `src` into
/// `dst` — images and generator sidecars — keeping existing names. Returns the names actually copied —
/// which, by that no-clobber rule, are exactly the ones that are NEW in
/// `dst`.
fn material_import_files(src: &Path, dst: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(src) else {
        return Vec::new();
    };
    let mut copied = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let ext = p
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x.to_ascii_lowercase())
            .unwrap_or_default();
        // Generator sidecars ride along with the images: importing a
        // folder that holds them and leaving the specs behind would turn
        // live materials into their own thumbnails.
        if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp")
            && gen_material_stem(&p).is_none()
        {
            continue;
        }
        let Some(name) = p.file_name() else { continue };
        let target = dst.join(name);
        if target.exists() {
            continue;
        }
        if std::fs::copy(&p, &target).is_ok() {
            copied.push(name.to_string_lossy().into_owned());
        }
    }
    copied
}

/// The tag half (good-first-issue #4): the two sidecars merge without any
/// conflict semantics, because a file that was actually copied is new in
/// `dst` — so it cannot already have a destination entry to disagree with.
/// The merge is therefore a plain add of the copied files' source entries;
/// a file the copy SKIPPED (its name was taken) contributes nothing, and
/// whatever the destination says about that name stands.
///
/// The source's unknown lines (comments, future keys) are deliberately not
/// carried: they describe the source folder, not this one. And when
/// nothing gets tagged we do not touch — or create — the destination
/// sidecar at all, keeping "no sidecar" the honest resting state.
fn material_import_tags(src: &Path, dst: &Path, copied: &[String]) {
    let from = MaterialTags::load(src);
    let tagged: Vec<&String> = copied.iter().filter(|n| !from.get(n).is_empty()).collect();
    if tagged.is_empty() {
        return;
    }
    let mut into = MaterialTags::load(dst);
    for name in tagged {
        into.set(name, from.get(name));
    }
    let _ = into.save(dst);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, tags: &str) -> MaterialItem {
        MaterialItem {
            name: name.to_owned(),
            path: PathBuf::from(format!("C:/m/{name}.png")),
            folder: 0,
            rel: PathBuf::new(),
            tags: tags.to_owned(),
            kind: MaterialKind::Image,
        }
    }

    /// A material in a named subfolder of registered root `f`. The rel is
    /// re-collected through `components()` so a `/` written here becomes
    /// whatever separator this platform's scan would have produced.
    fn item_in(f: usize, rel: &str, name: &str) -> MaterialItem {
        MaterialItem {
            rel: Path::new(rel).components().collect(),
            folder: f,
            ..item(name, "")
        }
    }

    fn spec() -> GenLinesSpec {
        GenLinesSpec {
            focus: true,
            a: 300.0,
            b: 200.0,
            c: 60.0,
            d: 400.0,
            count: 48,
            width: 4.0,
            jitter: 0.3,
            seed: 9,
            // The bank stores whatever the generator says; these tests are
            // about the SIDECAR round trip, so the newer generator
            // parameters ride along at their defaults.
            ..Default::default()
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

    // --- good-first-issue #4: tags ride `Import folder…` ------------------

    /// A fresh `src`/`dst` pair under one temp folder. The image files are
    /// never decoded on this path (the import is a byte copy), so their
    /// contents are only there to prove which bytes survived.
    fn import_dirs(tag: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("mn-import-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, dst) = (base.join("src"), base.join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        (src, dst)
    }

    /// The happy path: a tagged material arrives tagged, and the bank's one
    /// search box finds it by that tag in its new home.
    #[test]
    fn import_carries_the_tags_of_the_files_it_copied() {
        let (src, dst) = import_dirs("carries");
        std::fs::write(src.join("tone.png"), "tone-bytes").unwrap();
        std::fs::write(src.join("notes.txt"), "not an image").unwrap();
        // A generator sidecar is a material too — leaving it behind would
        // import a live material as its own thumbnail.
        write_gen_spec(&src, "focus", &spec()).unwrap();
        std::fs::write(src.join(TAGS_FILE), "tone.png=screentone, dots\n").unwrap();

        let copied = material_import_files(&src, &dst);
        material_import_tags(&src, &dst, &copied);

        let mut got = copied.clone();
        got.sort(); // read_dir order is the filesystem's business
        assert_eq!(
            got,
            vec![format!("focus{GEN_SUFFIX}"), "tone.png".to_owned()],
            "materials only — never the notes file"
        );
        assert_eq!(
            materials_scan_folder(&dst, 0)
                .iter()
                .find(|m| m.name == "focus")
                .map(|m| m.kind.clone()),
            Some(MaterialKind::GenLines(spec())),
            "the generator arrives live"
        );
        assert!(dst.join("tone.png").exists());
        let side = MaterialTags::load(&dst);
        assert_eq!(side.get("tone.png"), "screentone, dots");
        // …which is what the bank would scan, so the search box matches it.
        let scanned = item("tone", side.get("tone.png"));
        assert!(material_matches(&scanned, "screentone"));
        let _ = std::fs::remove_dir_all(src.parent().unwrap());
    }

    /// Source with no sidecar = today's behaviour byte-for-byte: the files
    /// land untagged and no sidecar is invented in the destination.
    #[test]
    fn import_from_an_untagged_folder_creates_no_sidecar() {
        let (src, dst) = import_dirs("nosidecar");
        std::fs::write(src.join("a.png"), "a").unwrap();

        let copied = material_import_files(&src, &dst);
        material_import_tags(&src, &dst, &copied);

        assert_eq!(copied, vec!["a.png".to_owned()]);
        assert!(dst.join("a.png").exists());
        assert!(
            !dst.join(TAGS_FILE).exists(),
            "nothing to tag must not create a sidecar"
        );
        let _ = std::fs::remove_dir_all(src.parent().unwrap());
    }

    /// A file whose name was already taken is NOT copied, so its source tag
    /// must not land either — the destination's own tag (and its bytes) win.
    #[test]
    fn import_does_not_tag_a_file_it_skipped() {
        let (src, dst) = import_dirs("skipped");
        std::fs::write(src.join("a.png"), "source-bytes").unwrap();
        std::fs::write(src.join(TAGS_FILE), "a.png=from-the-source\n").unwrap();
        std::fs::write(dst.join("a.png"), "destination-bytes").unwrap();
        std::fs::write(dst.join(TAGS_FILE), "a.png=mine\n").unwrap();

        let copied = material_import_files(&src, &dst);
        material_import_tags(&src, &dst, &copied);

        assert!(copied.is_empty(), "the name was taken");
        assert_eq!(
            std::fs::read_to_string(dst.join("a.png")).unwrap(),
            "destination-bytes"
        );
        assert_eq!(MaterialTags::load(&dst).get("a.png"), "mine");
        let _ = std::fs::remove_dir_all(src.parent().unwrap());
    }

    /// The destination sidecar's own content survives the merge — including
    /// its comments and its entries for files that are not there right now
    /// — while the SOURCE's comments stay behind: they described the source
    /// folder, not this one.
    #[test]
    fn import_keeps_the_destination_sidecar_and_leaves_source_comments_behind() {
        let (src, dst) = import_dirs("merge");
        std::fs::write(src.join("b.png"), "b").unwrap();
        std::fs::write(
            src.join(TAGS_FILE),
            "# notes about the SOURCE folder\nb.png=effect, action\n",
        )
        .unwrap();
        std::fs::write(
            dst.join(TAGS_FILE),
            "# my own notes\ngone-for-now.png=kept\n",
        )
        .unwrap();

        let copied = material_import_files(&src, &dst);
        material_import_tags(&src, &dst, &copied);

        let out = std::fs::read_to_string(dst.join(TAGS_FILE)).unwrap();
        assert!(out.contains("# my own notes\n"), "{out}");
        assert!(out.contains("gone-for-now.png=kept\n"), "{out}");
        assert!(out.contains("b.png=effect, action\n"), "{out}");
        assert!(
            !out.contains("SOURCE folder"),
            "the source's comments describe the source: {out}"
        );
        let _ = std::fs::remove_dir_all(src.parent().unwrap());
    }

    // --- P0-1: the recursive scan and the folder tree --------------------

    /// A flat folder behaves EXACTLY as it did before the scan learned to
    /// recurse: the same materials, in the same name order, every one of
    /// them at the registered root (`rel` empty).
    #[test]
    fn materials_scan_of_a_flat_folder_is_unchanged_and_carries_an_empty_rel() {
        let dir = gen_dir("flat");
        for n in ["b.png", "a.png", "c.jpg", "notes.txt"] {
            std::fs::write(dir.join(n), "x").unwrap();
        }
        let items = materials_scan_folder(&dir, 0);
        assert_eq!(
            items.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c"],
            "name order, images only — the old contract"
        );
        assert!(
            items.iter().all(|m| m.rel.as_os_str().is_empty()),
            "a root file has no sub-path: {items:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Subdirectories are walked, each item carries the folder-relative
    /// DIRECTORY it was found in, a nested `tags.txt` tags its own
    /// neighbours, and a dot-folder is not the artist's business.
    #[test]
    fn materials_scan_walks_subfolders_and_stamps_each_item_with_its_rel() {
        let dir = gen_dir("nested");
        let sfx = dir.join("sfx");
        let impact = sfx.join("impact");
        std::fs::create_dir_all(&impact).unwrap();
        std::fs::create_dir_all(dir.join(".cache")).unwrap();
        std::fs::write(dir.join("root.png"), "x").unwrap();
        std::fs::write(sfx.join("whoosh.png"), "x").unwrap();
        std::fs::write(impact.join("boom.png"), "x").unwrap();
        std::fs::write(dir.join(".cache/ignored.png"), "x").unwrap();
        std::fs::write(sfx.join(TAGS_FILE), "whoosh.png=effect, air\n").unwrap();

        let items = materials_scan_folder(&dir, 2);
        let mut got: Vec<(String, String)> = items
            .iter()
            .map(|m| (m.rel.display().to_string(), m.name.clone()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                (String::new(), "root".to_owned()),
                ("sfx".to_owned(), "whoosh".to_owned()),
                (
                    Path::new("sfx/impact").components().collect::<PathBuf>().display().to_string(),
                    "boom".to_owned()
                ),
            ],
            "a dot-folder must not scan, and every hit knows where it lives"
        );
        assert!(items.iter().all(|m| m.folder == 2));
        assert_eq!(
            items.iter().find(|m| m.name == "whoosh").unwrap().tags,
            "effect, air",
            "a subfolder's sidecar tags its OWN neighbours"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The tree is pre-order (a node's subtree follows it contiguously at a
    /// greater depth), the registered folder's own name is the top node, a
    /// directory counts its whole subtree, and selecting a branch shows
    /// everything under it — not just its direct files.
    #[test]
    fn materials_tree_is_preorder_and_a_branch_includes_its_subtree() {
        let items = vec![
            item_in(0, "", "root-a"),
            item_in(0, "sfx", "whoosh"),
            item_in(0, "sfx/impact", "boom"),
            item_in(1, "", "other"),
        ];
        let tree = materials_tree(&items, &["bank".into(), "mine".into()]);
        let shape: Vec<(usize, &str, usize)> = tree
            .iter()
            .map(|n| (n.depth, n.label.as_str(), n.count))
            .collect();
        assert_eq!(
            shape,
            vec![
                (0, "All materials", 4),
                (1, "bank", 3),
                (2, "sfx", 2),
                (3, "impact", 1),
                (1, "mine", 1),
            ],
            "roots keep their registered names; a branch counts its subtree"
        );
        // Depth only ever grows by one, which is what makes "skip until the
        // depth drops back" a correct collapse.
        for w in tree.windows(2) {
            assert!(w[1].depth <= w[0].depth + 1, "not pre-order: {tree:?}");
        }
        let dir_sfx = &tree[2].filter;
        assert!(
            items.iter().filter(|i| dir_sfx.accepts(i)).count() == 2,
            "selecting sfx must include sfx/impact"
        );
        assert!(
            !dir_sfx.accepts(&item_in(0, "sfx2", "decoy")),
            "component-wise: sfx2 is not inside sfx"
        );
        assert!(tree[1].children && !tree[4].children);
    }

    /// OUR ADDITION: a flat folder past the threshold grows virtual
    /// name-prefix groups, labelled with the token, and only for tokens that
    /// actually name a family. A folder under the threshold grows none —
    /// which is why the flat-folder behaviour above is still the old one.
    #[test]
    fn materials_tree_groups_a_huge_flat_folder_by_name_prefix() {
        // The owner's real shapes: a leading `_` before the word, and a
        // leading space.
        let mut items: Vec<MaterialItem> = Vec::new();
        for n in 0..12 {
            items.push(item_in(0, "", &format!("_boom_ {n}")));
        }
        for n in 0..9 {
            items.push(item_in(0, "", &format!(" Painter {n}")));
        }
        for n in 0..5 {
            items.push(item_in(0, "", &format!("lonely-{n}")));
        }
        // …padded past FLAT_GROUP_MIN with names that share nothing.
        for n in 0..(FLAT_GROUP_MIN + 1 - items.len()) {
            items.push(item_in(0, "", &format!("x{n}y{n}")));
        }
        let tree = materials_tree(&items, &["bank".into()]);
        let groups: Vec<(&str, usize)> = tree
            .iter()
            .filter(|n| matches!(n.filter, MaterialFilter::Prefix(..)))
            .map(|n| (n.label.as_str(), n.count))
            .collect();
        assert_eq!(
            groups,
            vec![("boom", 12), ("painter", 9)],
            "leading separators are skipped, and 5 files are not a folder"
        );
        assert_eq!(tree[1].depth + 1, tree[2].depth, "groups hang off the root");
        let boom = &tree[2].filter;
        assert_eq!(items.iter().filter(|i| boom.accepts(i)).count(), 12);

        // One material short of the threshold: no groups at all.
        items.pop();
        assert!(
            !materials_tree(&items, &["bank".into()])
                .iter()
                .any(|n| matches!(n.filter, MaterialFilter::Prefix(..))),
            "a browsable folder must not sprout invented rows"
        );
    }

    /// A tree row's id is what the collapse set stores, so two different
    /// rows must never share one — and the token rule must not turn a
    /// one-letter lead into a group name.
    #[test]
    fn materials_filter_ids_are_distinct_and_prefix_tokens_need_two_chars() {
        let a = MaterialFilter::Dir(0, PathBuf::from("sfx"));
        let b = MaterialFilter::Dir(1, PathBuf::from("sfx"));
        let c = MaterialFilter::Prefix(0, PathBuf::from("sfx"), "boom".into());
        let ids = [MaterialFilter::All.id(), a.id(), b.id(), c.id()];
        let mut uniq = ids.to_vec();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), ids.len(), "colliding tree ids: {ids:?}");
        assert_eq!(name_prefix_token("_boom_ horizontal").as_deref(), Some("boom"));
        assert_eq!(name_prefix_token(" Painter 11").as_deref(), Some("painter"));
        assert_eq!(name_prefix_token("a-1").as_deref(), None, "one letter names nothing");
        assert_eq!(name_prefix_token("___").as_deref(), None);
    }

    // --- generator materials (the bank places effect lines LIVE) ---------

    fn gen_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mn-genmat-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The scan's half of the contract: `<stem>.gen.json` IS the material,
    /// the same-stem PNG is only its thumbnail (never a second material),
    /// and a sidecar that will not parse is no material at all.
    #[test]
    fn scan_reads_a_gen_json_as_a_generator_with_its_png_as_the_thumbnail() {
        let dir = gen_dir("scan");
        write_gen_spec(&dir, "focus-lines", &spec()).expect("sidecar written");
        // The scan never decodes an image, so these bytes only prove which
        // file the bank pointed at.
        std::fs::write(dir.join("focus-lines.png"), "thumbnail-bytes").unwrap();
        std::fs::write(dir.join("tone.png"), "tone-bytes").unwrap();
        std::fs::write(dir.join("broken.gen.json"), "{ not json").unwrap();
        std::fs::write(dir.join(TAGS_FILE), "focus-lines.gen.json=effect, action\n").unwrap();

        let items = materials_scan_folder(&dir, 3);
        let names: Vec<&str> = items.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            ["focus-lines", "tone"],
            "one generator, one bitmap — and nothing from the unreadable sidecar"
        );
        let g = &items[0];
        assert!(g.is_generator());
        assert_eq!(g.kind, MaterialKind::GenLines(spec()));
        assert_eq!(g.path, dir.join(format!("focus-lines{GEN_SUFFIX}")));
        assert_eq!(g.thumb_path(), dir.join("focus-lines.png"));
        assert_eq!(g.folder, 3);
        assert_eq!(g.tags, "effect, action", "tags key on the sidecar's name");
        assert_eq!(items[1].kind, MaterialKind::Image);
        assert_eq!(items[1].thumb_path(), items[1].path, "a bitmap is its own thumb");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The round trip that makes generator materials worth having: a tuned
    /// effect-line layer registers as a LIVE material (spec + thumbnail),
    /// while an ordinary raster layer registers exactly as it always did.
    #[test]
    fn registering_a_generated_layer_writes_the_gen_sidecar() {
        let Some(r) = crate::app::headless_renderer() else {
            return;
        };
        let mut app = App::new(r, (600, 400), 1.0);
        let dir = gen_dir("register");

        let li = app.doc.add_layer("Focus lines");
        let s = spec();
        app.doc.layers[li].genlines = Some(s);
        assert!(app.doc.regen_genlines(li, s), "the layer has ink to lift");

        let (p, stem) = app
            .material_register_layer_into(dir.clone())
            .expect("a generated layer registers");
        assert_eq!(stem, "Focus_lines");
        assert_eq!(
            p,
            dir.join(format!("Focus_lines{GEN_SUFFIX}")),
            "the spec is the material's identity, not the PNG"
        );
        assert!(
            dir.join("Focus_lines.png").exists(),
            "the PNG stays, as the thumbnail"
        );
        let items = materials_scan_folder(&dir, 0);
        assert_eq!(items.len(), 1, "one material, not two: {items:?}");
        assert_eq!(
            items[0].kind,
            MaterialKind::GenLines(s),
            "the parameters came back out of the bank"
        );

        // An ordinary raster layer is untouched by any of this.
        const W: u16 = mn_core::FIX15_ONE as u16;
        app.doc.add_layer("plain");
        app.doc.begin_op();
        app.doc
            .active_layer_mut()
            .tile_mut(mn_core::TileIdx::new(0, 0))
            .set_pixel(3, 4, [W, W, W, W]);
        app.doc.end_op();
        let (p2, stem2) = app
            .material_register_layer_into(dir.clone())
            .expect("a raster layer registers");
        assert_eq!(stem2, "plain");
        assert_eq!(p2, dir.join("plain.png"));
        assert!(
            !dir.join(format!("plain{GEN_SUFFIX}")).exists(),
            "no spec, no sidecar"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
