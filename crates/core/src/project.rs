//! The comic project container: `.mnc` (MangaNakama Comic).
//!
//! Two storage shapes share the `.mnc` name:
//!
//! - **Single file** (import/export, autosave fallback): a zip holding one ORA
//!   per page plus a JSON manifest.
//!
//!   ```text
//!   mimetype                  "application/x-manganakama-comic" (stored, first)
//!   project.json              { version, story, binding_right, setup, pages: [..] }
//!   pages/p001.ora            OpenRaster, one per page
//!   ```
//!
//! - **Work folder** (the native multi-page format, CSP-style): a user-chosen
//!   directory holding a tiny `work.mnc` index side by side with the page
//!   files. Saving rewrites only pages whose content revision advanced — no
//!   GB rewrites of untouched pages on every save/autosave — one corrupt page
//!   can no longer take the whole work down, and the pages are directly
//!   editable fallbacks in Krita/CSP (they are standard ORA).
//!
//!   ```text
//!   <chosen folder>/work.mnc  zip: mimetype "application/x-manganakama-workfolder"
//!                             + workfolder.json { story, binding, setup, next_id,
//!                                                 pages: [{file, id, rev}] }
//!   <chosen folder>/p001.ora  OpenRaster, named by stable page id (not order)
//!   ```
//!
//! Pages are kept as **encoded bytes** in memory; the app decodes only the
//! page being edited (decode-on-switch), so a 23-page project does not hold 23
//! decoded print-resolution documents.

use std::io::{Cursor, Read, Seek, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::doc::Document;
use crate::ora::{self, OraError};
use crate::page::PageSetup;

pub const MNC_MIME: &str = "application/x-manganakama-comic";
/// Distinguishes a work-folder index from a single-file comic.
pub const WORKFOLDER_MIME: &str = "application/x-manganakama-workfolder";
/// The index file name inside a work folder.
pub const WORKFOLDER_INDEX: &str = "work.mnc";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub version: u32,
    pub story: String,
    /// Right-bound (Japanese) page order when true.
    pub binding_right: bool,
    /// Shared page geometry (guides, new-page size). `None` = pixel canvas.
    pub setup: Option<PageSetup>,
    /// The work's expression colour (CSP 表現色): Mono = B&W print, and the
    /// preflight's colour-on-mono check keys off it. Manga default.
    #[serde(default)]
    pub expression: Expression,
    /// Perfect-binding spine width, mm. `0` = not set (the preflight
    /// flags a binding that needs one).
    #[serde(default)]
    pub spine_mm: f32,
    /// Cover page designation — page index in reading order. `None` = not
    /// set; the preflight flags a multi-page work with no cover.
    #[serde(default)]
    pub cover: Option<usize>,
    /// Template page (tekno B2) — reading-order index whose bytes seed
    /// every NEW page instead of a blank. Index-bound like `cover`
    /// (reorder/delete does not chase it). `None` = blanks.
    #[serde(default)]
    pub template_page: Option<usize>,
    /// Publisher/printer target (ROADMAP M2) — drives preflight norms
    /// (page-count multiple, screen ruling) and the export finish. `None`
    /// = no target picked; nothing checks or preselects.
    #[serde(default)]
    pub profile: Option<crate::profile::PublisherProfile>,
    /// Page file names inside the zip, reading order.
    pages: Vec<String>,
}

/// What the pages are FOR (CSP expression colour) — decides which content
/// the printer can reproduce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Expression {
    /// Black-and-white print (manga). Colour pixels warn in preflight.
    #[default]
    Mono,
    /// Colour print.
    Colour,
}

impl ProjectMeta {
    /// The metadata as the app holds it, for consumers that check the WORK
    /// (preflight) and do not consult the pages list.
    pub fn for_checks(
        story: String,
        binding_right: bool,
        setup: Option<PageSetup>,
        expression: Expression,
        spine_mm: f32,
        cover: Option<usize>,
    ) -> Self {
        Self {
            version: 1,
            story,
            binding_right,
            setup,
            expression,
            spine_mm,
            cover,
            // Not a preflight input — the checks never read it. (The
            // PROFILE is one; `run_preflight` sets it after construction
            // so this signature does not grow a parameter per field.)
            template_page: None,
            profile: None,
            pages: Vec::new(),
        }
    }
}

/// A whole comic: metadata + every page as encoded ORA bytes.
#[derive(Clone, Debug)]
pub struct Project {
    pub meta: ProjectMeta,
    pub pages: Vec<Vec<u8>>,
}

impl Project {
    pub fn new(story: String, setup: Option<PageSetup>, binding_right: bool) -> Self {
        Self {
            meta: ProjectMeta {
                version: 1,
                story,
                binding_right,
                setup,
                expression: Expression::default(),
                spine_mm: 0.0,
                cover: None,
                template_page: None,
                profile: None,
                pages: Vec::new(),
            },
            pages: Vec::new(),
        }
    }
}

/// Encode a document to ORA bytes (the page currency of a project).
pub fn doc_to_bytes(doc: &Document) -> Result<Vec<u8>, OraError> {
    doc_to_bytes_with(doc, None)
}

/// Same, embedding the caller-rendered page preview as `mnc/preview.png`
/// (owner preview tier, 2026-08-18). `None` = the old shape.
pub fn doc_to_bytes_with(doc: &Document, preview_png: Option<&[u8]>) -> Result<Vec<u8>, OraError> {
    let mut buf = Cursor::new(Vec::new());
    ora::save_to_with(doc, &mut buf, preview_png)?;
    Ok(buf.into_inner())
}

pub fn bytes_to_doc(bytes: &[u8]) -> Result<Document, OraError> {
    ora::load_from(Cursor::new(bytes))
}

/// Pull the embedded thumbnail out of a page's ORA bytes without decoding the
/// whole document (ORA ships `Thumbnails/thumbnail.png`, ≤256px).
pub fn page_thumb(bytes: &[u8]) -> Option<image::RgbaImage> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut file = zip.by_name("Thumbnails/thumbnail.png").ok()?;
    let mut png = Vec::new();
    file.read_to_end(&mut png).ok()?;
    Some(image::load_from_memory(&png).ok()?.to_rgba8())
}

/// Pull the SHARP page preview (`mnc/preview.png`, gray-8, export rules —
/// drafts off) without decoding the document. `None` for pages saved
/// before the preview tier or through paths that do not render one.
pub fn page_preview(bytes: &[u8]) -> Option<image::GrayImage> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut file = zip.by_name("mnc/preview.png").ok()?;
    let mut png = Vec::new();
    file.read_to_end(&mut png).ok()?;
    Some(image::load_from_memory(&png).ok()?.to_luma8())
}

/// Write the single-file `.mnc` at `path`. Atomic — same contract and the same
/// reasons as [`crate::ora::save`] (audit finding H2): the previous file
/// survives a crash or a full disk, and a failed final flush is an error
/// instead of a silently truncated "successful" save.
pub fn save(project: &Project, path: &Path) -> Result<(), OraError> {
    crate::ora::write_atomic(path, |w| save_to(project, w))
}

pub fn save_to<W: Write + Seek>(project: &Project, sink: W) -> Result<(), OraError> {
    use zip::write::SimpleFileOptions;
    let mut zip = zip::ZipWriter::new(sink);

    // Mimetype first and stored, same convention as ORA itself.
    zip.start_file(
        "mimetype",
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )?;
    zip.write_all(MNC_MIME.as_bytes())?;

    let mut meta = project.meta.clone();
    meta.pages = (0..project.pages.len())
        .map(|i| format!("pages/p{:03}.ora", i + 1))
        .collect();
    zip.start_file("project.json", SimpleFileOptions::default())?;
    let json =
        serde_json::to_vec_pretty(&meta).map_err(|e| OraError(format!("project.json: {e}")))?;
    zip.write_all(&json)?;

    // Page ORAs are zips themselves — store, don't double-compress.
    for (name, bytes) in meta.pages.iter().zip(&project.pages) {
        zip.start_file(
            name.as_str(),
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
        )?;
        zip.write_all(bytes)?;
    }
    zip.finish()?;
    Ok(())
}

pub fn load(path: &Path) -> Result<Project, OraError> {
    let file = std::fs::File::open(path)?;
    load_from(std::io::BufReader::new(file))
}

pub fn load_from<R: Read + Seek>(source: R) -> Result<Project, OraError> {
    let mut zip = zip::ZipArchive::new(source)?;
    let meta: ProjectMeta = {
        let mut f = zip
            .by_name("project.json")
            .map_err(|_| OraError("not a MangaNakama comic (no project.json)".into()))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        serde_json::from_slice(&buf).map_err(|e| OraError(format!("project.json: {e}")))?
    };
    let mut pages = Vec::with_capacity(meta.pages.len());
    for name in &meta.pages {
        let mut f = zip
            .by_name(name)
            .map_err(|_| OraError(format!("missing page {name}")))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        pages.push(buf);
    }
    if pages.is_empty() {
        return Err(OraError("comic has no pages".into()));
    }
    Ok(Project { meta, pages })
}

// --- work folder ---------------------------------------------------------

/// Which flavour of `.mnc` a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MncKind {
    /// Single-file comic zip.
    Comic,
    /// `work.mnc` index of a work folder.
    WorkFolderIndex,
    /// Not a MangaNakama container.
    Unknown,
}

/// Read just the `mimetype` entry of a `.mnc` zip to tell the flavours apart.
pub fn sniff_kind(path: &Path) -> MncKind {
    let Ok(file) = std::fs::File::open(path) else {
        return MncKind::Unknown;
    };
    let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        return MncKind::Unknown;
    };
    let Ok(mut f) = zip.by_name("mimetype") else {
        return MncKind::Unknown;
    };
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        return MncKind::Unknown;
    }
    match buf.as_str() {
        MNC_MIME => MncKind::Comic,
        WORKFOLDER_MIME => MncKind::WorkFolderIndex,
        _ => MncKind::Unknown,
    }
}

/// A page's entry in the work-folder index.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FolderPageMeta {
    pub file: String,
    pub id: u32,
    /// Content revision already on disk for this file.
    pub rev: u64,
    /// Content revision the last EXPORT of this page wrote (0 = never
    /// exported). Defaulted, so a work saved before the reminder existed
    /// loads clean and simply says "never exported" until the next export.
    #[serde(default)]
    pub exported_rev: u64,
}

/// The work-folder index payload (`workfolder.json` inside `work.mnc`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FolderMeta {
    pub version: u32,
    pub story: String,
    pub binding_right: bool,
    pub setup: Option<PageSetup>,
    /// The work's expression colour (see `Expression`). Preflight input.
    #[serde(default)]
    pub expression: Expression,
    /// Perfect-binding spine width, mm (0 = unset). Preflight input.
    #[serde(default)]
    pub spine_mm: f32,
    /// Cover page designation — page index in reading order. Preflight input.
    #[serde(default)]
    pub cover: Option<usize>,
    /// Template page — see `ProjectMeta::template_page`.
    #[serde(default)]
    pub template_page: Option<usize>,
    /// Publisher/printer target — see `ProjectMeta::profile`.
    #[serde(default)]
    pub profile: Option<crate::profile::PublisherProfile>,
    /// Next free page identity; ids are never reused inside a work.
    pub next_id: u32,
    /// Pages in READING order — the list order is the page order, the file
    /// names are stable identities.
    pub pages: Vec<FolderPageMeta>,
}

/// One page of a folder-format work: identity, change revisions, ORA bytes.
#[derive(Clone, Debug)]
pub struct FolderPage {
    pub id: u32,
    /// Current content revision. `0` = not yet folder-backed (the save assigns
    /// the id and treats the page as new).
    pub rev: u64,
    /// Revision already on disk; a page with `rev <= saved_rev` and an
    /// existing file is skipped by [`save_folder`].
    pub saved_rev: u64,
    /// Revision the last export of this page wrote (0 = never exported).
    /// Rides the index rather than a file in the export folder on purpose:
    /// an app that writes bookkeeping into an output folder unasked is a
    /// trap (plans/21-M5-M6-DECISIONS.md, owner ask 2026-08-22).
    pub exported_rev: u64,
    pub bytes: Vec<u8>,
}

/// A whole work-folder work: metadata + pages with folder bookkeeping.
#[derive(Clone, Debug)]
pub struct WorkFolder {
    pub story: String,
    pub binding_right: bool,
    pub setup: Option<PageSetup>,
    pub expression: Expression,
    pub spine_mm: f32,
    pub cover: Option<usize>,
    pub template_page: Option<usize>,
    pub profile: Option<crate::profile::PublisherProfile>,
    pub next_id: u32,
    pub pages: Vec<FolderPage>,
}

/// Stable file name for a page identity (order changes never rename files).
pub fn page_file_name(id: u32) -> String {
    format!("p{id:03}.ora")
}

/// Is this file name one a work folder owns (`work.mnc` or a `pNNN.ora`,
/// plus their `.tmp` write-siblings)? Used by the app's "is this folder free
/// for a new work" guard.
pub fn is_workfolder_file(name: &str) -> bool {
    let name = name.strip_suffix(".tmp").unwrap_or(name);
    if name.eq_ignore_ascii_case(WORKFOLDER_INDEX) {
        return true;
    }
    let Some(stem) = name.strip_suffix(".ora") else {
        return false;
    };
    !stem.is_empty()
        && stem.len() >= 2
        && stem.starts_with('p')
        && stem[1..].bytes().all(|b| b.is_ascii_digit())
}

/// Write (or incrementally update) a work folder: `dir/work.mnc` + `pNNN.ora`
/// side by side. Each changed page is written to a `.tmp` and renamed into
/// place (atomic per file on Windows — `std::fs::rename` replaces), and the
/// index is committed LAST, so a crash mid-save can only leave pages NEWER
/// than the index records, never older.
///
/// `managed` is the previous index's file list — files in it that are no
/// longer referenced (pages deleted since) are removed after the index lands.
/// Files we never wrote are never touched.
///
/// Returns the assigned page ids in reading order (fresh ids for pages that
/// entered with `id == 0`) so the caller can update its bookkeeping.
pub fn save_folder(
    wf: &WorkFolder,
    dir: &Path,
    managed: &[String],
) -> Result<(Vec<u32>, usize), OraError> {
    std::fs::create_dir_all(dir)?;
    let mut written = 0usize;
    let mut referenced: Vec<String> = Vec::with_capacity(wf.pages.len());

    // Ids are assigned once, in reading order, and never reused.
    let mut next_id = wf.next_id.max(1);
    let ids: Vec<u32> = wf
        .pages
        .iter()
        .map(|p| {
            if p.id == 0 {
                let id = next_id;
                next_id += 1;
                id
            } else {
                next_id = next_id.max(p.id + 1);
                p.id
            }
        })
        .collect();

    for (p, &id) in wf.pages.iter().zip(&ids) {
        let name = page_file_name(id);
        let path = dir.join(&name);
        if p.rev > p.saved_rev || !path.exists() {
            let tmp = dir.join(format!("{name}.tmp"));
            {
                let file = std::fs::File::create(&tmp)?;
                let mut w = std::io::BufWriter::new(file);
                w.write_all(&p.bytes)?;
                w.flush()?;
            }
            std::fs::rename(&tmp, &path)?;
            written += 1;
        }
        referenced.push(name);
    }

    let meta = FolderMeta {
        version: 2,
        story: wf.story.clone(),
        binding_right: wf.binding_right,
        setup: wf.setup.clone(),
        expression: wf.expression,
        spine_mm: wf.spine_mm,
        cover: wf.cover,
        template_page: wf.template_page,
        profile: wf.profile.clone(),
        next_id,
        pages: wf
            .pages
            .iter()
            .zip(&ids)
            .zip(&referenced)
            .map(|((p, &id), file)| FolderPageMeta {
                file: file.clone(),
                id,
                rev: p.rev.max(1),
                exported_rev: p.exported_rev,
            })
            .collect(),
    };

    // Commit the index last.
    let index = dir.join(WORKFOLDER_INDEX);
    let tmp = dir.join(format!("{WORKFOLDER_INDEX}.tmp"));
    {
        let file = std::fs::File::create(&tmp)?;
        save_index_to(&meta, std::io::BufWriter::new(file))?;
    }
    std::fs::rename(&tmp, &index)?;

    // Cleanup: only files the previous index listed (ours), never foreign ones.
    for name in managed {
        if !referenced.iter().any(|r| r == name) {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }
    Ok((ids, written))
}

fn save_index_to<W: Write + Seek>(meta: &FolderMeta, sink: W) -> Result<(), OraError> {
    use zip::write::SimpleFileOptions;
    let mut zip = zip::ZipWriter::new(sink);
    zip.start_file(
        "mimetype",
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )?;
    zip.write_all(WORKFOLDER_MIME.as_bytes())?;
    zip.start_file("workfolder.json", SimpleFileOptions::default())?;
    let json =
        serde_json::to_vec_pretty(meta).map_err(|e| OraError(format!("workfolder.json: {e}")))?;
    zip.write_all(&json)?;
    zip.finish()?;
    Ok(())
}

/// Load a work folder. `path` may be the folder itself or its `work.mnc`.
pub fn load_folder(path: &Path) -> Result<WorkFolder, OraError> {
    let index = if path.is_dir() {
        path.join(WORKFOLDER_INDEX)
    } else {
        path.to_path_buf()
    };
    let dir = index
        .parent()
        .ok_or_else(|| OraError("work folder has no parent directory".into()))?
        .to_path_buf();
    let file = std::fs::File::open(&index)?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))?;
    {
        let mut m = zip
            .by_name("mimetype")
            .map_err(|_| OraError("not a work-folder index (no mimetype)".into()))?;
        let mut buf = String::new();
        m.read_to_string(&mut buf)?;
        if buf != WORKFOLDER_MIME {
            return Err(OraError(format!(
                "not a work-folder index (mimetype {buf})"
            )));
        }
    }
    let meta: FolderMeta = {
        let mut f = zip
            .by_name("workfolder.json")
            .map_err(|_| OraError("not a work-folder index (no workfolder.json)".into()))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        serde_json::from_slice(&buf).map_err(|e| OraError(format!("workfolder.json: {e}")))?
    };
    if meta.pages.is_empty() {
        return Err(OraError("work folder has no pages".into()));
    }
    let mut pages = Vec::with_capacity(meta.pages.len());
    for pm in &meta.pages {
        let mut f = std::fs::File::open(dir.join(&pm.file))
            .map_err(|e| OraError(format!("page {}: {e}", pm.file)))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        pages.push(FolderPage {
            id: pm.id,
            rev: pm.rev,
            saved_rev: pm.rev,
            exported_rev: pm.exported_rev,
            bytes: buf,
        });
    }
    let next_id = meta
        .next_id
        .max(meta.pages.iter().map(|p| p.id).max().unwrap_or(0) + 1);
    Ok(WorkFolder {
        story: meta.story,
        binding_right: meta.binding_right,
        setup: meta.setup,
        expression: meta.expression,
        spine_mm: meta.spine_mm,
        cover: meta.cover,
        template_page: meta.template_page,
        profile: meta.profile,
        next_id,
        pages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::{FIX15_ONE, TileIdx};

    /// Rulers RIDE the document bytes (contract inverted 2026-08-23 —
    /// they were session-only, and perspective grids died with every
    /// restart). Page stash → decode keeps the set; `App::adopt_page_doc`
    /// now only fills in pages that saved none.
    #[test]
    fn rulers_ride_the_document_bytes() {
        let mut d = Document::new(64, 64);
        d.rulers.items.push(crate::ruler::Ruler::Line {
            a: [1.0, 2.0],
            b: [3.0, 4.0],
        });
        d.rulers.curves.push(crate::ruler::CurveRuler {
            pts: vec![[0.0, 0.0], [5.0, 5.0]],
        });
        d.rulers.on = true;
        let back = bytes_to_doc(&doc_to_bytes(&d).unwrap()).unwrap();
        assert_eq!(back.rulers, d.rulers, "a decoded page keeps its rulers");
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mnc-wf-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn two_page_work() -> WorkFolder {
        let mut d1 = Document::new(128, 128);
        d1.active_layer_mut()
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(3, 3, [0, 0, 0, FIX15_ONE as u16]);
        let d2 = Document::new(128, 128);
        WorkFolder {
            story: "TEKNO".into(),
            binding_right: true,
            setup: Some(crate::page::PageSetup::presets()[0].clone()),
            expression: Expression::Mono,
            spine_mm: 6.2,
            cover: Some(1),
            template_page: Some(0),
            profile: crate::profile::PublisherProfile::builtins().pop(),
            next_id: 0,
            pages: vec![
                FolderPage {
                    id: 0,
                    rev: 1,
                    saved_rev: 0,
                    exported_rev: 0,
                    bytes: doc_to_bytes(&d1).unwrap(),
                },
                FolderPage {
                    id: 0,
                    rev: 2,
                    saved_rev: 0,
                    exported_rev: 0,
                    bytes: doc_to_bytes(&d2).unwrap(),
                },
            ],
        }
    }

    /// TRIAGE 132: the print-metadata fields survive a folder round trip,
    /// and a PRE-preflight index (no expression/spine/cover keys at all)
    /// still loads with the documented defaults — old works are unaffected.
    #[test]
    fn print_metadata_round_trips_and_old_indexes_default() {
        let dir = temp_dir("printmeta");
        let wf = two_page_work();
        let (ids, _) = save_folder(&wf, &dir, &[]).unwrap();
        let back = load_folder(&dir).unwrap();
        assert_eq!(back.expression, Expression::Mono);
        assert!((back.spine_mm - 6.2).abs() < 1e-6);
        assert_eq!(back.cover, Some(1));
        assert_eq!(back.template_page, Some(0), "tekno B2 rides the index");
        assert_eq!(
            back.profile.as_ref().map(|p| p.name.clone()),
            crate::profile::PublisherProfile::builtins().pop().map(|p| p.name),
            "M2: the publisher profile rides the index too"
        );
        let _ = ids;

        // The pre-132 index shape: every new key absent.
        let old = r#"{
            "version": 2,
            "story": "TEKNO",
            "binding_right": true,
            "setup": null,
            "next_id": 3,
            "pages": []
        }"#;
        let m: FolderMeta = serde_json::from_str(old).expect("old index must parse");
        assert_eq!(m.expression, Expression::Mono);
        assert_eq!(m.spine_mm, 0.0);
        assert_eq!(m.cover, None);
    }

    /// The unexported-pages reminder (owner ask 2026-08-22): the export
    /// revision round trips per page, and an index written before the
    /// reminder existed — no `exported_rev` key at all — loads as "never
    /// exported" instead of failing to parse. Old works must open clean.
    #[test]
    fn export_revision_round_trips_and_old_indexes_default() {
        let dir = temp_dir("exportrev");
        let mut wf = two_page_work();
        wf.pages[0].exported_rev = 7;
        save_folder(&wf, &dir, &[]).unwrap();
        let back = load_folder(&dir).unwrap();
        assert_eq!(back.pages[0].exported_rev, 7, "the export revision persists");
        assert_eq!(back.pages[1].exported_rev, 0, "an unexported page stays 0");
        let _ = std::fs::remove_dir_all(&dir);

        // The pre-reminder page entry: file, id, rev and nothing else.
        let old = r#"{ "file": "p001.ora", "id": 1, "rev": 42 }"#;
        let pm: FolderPageMeta = serde_json::from_str(old).expect("old page entry must parse");
        assert_eq!(pm.rev, 42);
        assert_eq!(pm.exported_rev, 0, "never exported, not 'up to date'");
    }

    #[test]
    fn workfolder_roundtrip_incremental_saves_and_cleanup() {
        let dir = temp_dir("rt");
        let mut wf = two_page_work();

        // First save: both pages written, ids assigned.
        let (ids, written) = save_folder(&wf, &dir, &[]).unwrap();
        assert_eq!(written, 2);
        assert_eq!(ids, vec![1, 2]);
        assert!(dir.join(WORKFOLDER_INDEX).is_file());
        assert!(dir.join("p001.ora").is_file());
        assert!(dir.join("p002.ora").is_file());
        // Caller-side bookkeeping: saved_rev := rev, ids land in the pages.
        for (p, &id) in wf.pages.iter_mut().zip(&ids) {
            p.saved_rev = p.rev;
            p.id = id;
        }

        // Unchanged re-save: only the index is rewritten.
        let (_, written) = save_folder(&wf, &dir, &["p001.ora".into(), "p002.ora".into()]).unwrap();
        assert_eq!(written, 0);

        // One page edited: only that file is rewritten.
        wf.pages[0].rev = 5;
        wf.pages[0].bytes = {
            let mut d = Document::new(128, 128);
            d.active_layer_mut().tile_mut(TileIdx::new(1, 1)).set_pixel(
                4,
                4,
                [0, 0, 0, FIX15_ONE as u16],
            );
            doc_to_bytes(&d).unwrap()
        };
        let (_, written) = save_folder(&wf, &dir, &["p001.ora".into(), "p002.ora".into()]).unwrap();
        assert_eq!(written, 1);
        wf.pages[0].saved_rev = 5;

        // Round-trip: load via the index path AND the folder path.
        for p in [&dir.join(WORKFOLDER_INDEX), &dir] {
            let back = load_folder(p).unwrap();
            assert_eq!(back.story, "TEKNO");
            assert!(back.binding_right);
            assert_eq!(back.pages.len(), 2);
            assert_eq!(back.pages[0].id, 1);
            assert_eq!(back.pages[0].saved_rev, back.pages[0].rev);
            assert_eq!(back.next_id, 3);
            assert_eq!(back.setup.as_ref().unwrap().dpi, 600);
            let doc = bytes_to_doc(&back.pages[0].bytes).unwrap();
            assert_eq!(
                doc.active_layer()
                    .tile(TileIdx::new(1, 1))
                    .unwrap()
                    .pixel(4, 4)[3],
                FIX15_ONE as u16
            );
        }

        // Page deleted: the managed file is cleaned, the rest survives.
        wf.pages.truncate(1);
        let (_, written) = save_folder(&wf, &dir, &["p001.ora".into(), "p002.ora".into()]).unwrap();
        assert_eq!(written, 0);
        assert!(dir.join("p001.ora").is_file());
        assert!(!dir.join("p002.ora").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sniff_kind_tells_the_flavours_apart() {
        let dir = temp_dir("sniff");
        save_folder(&two_page_work(), &dir, &[]).unwrap();
        assert_eq!(
            sniff_kind(&dir.join(WORKFOLDER_INDEX)),
            MncKind::WorkFolderIndex
        );

        let mut p = Project::new("S".into(), None, false);
        p.pages.push(doc_to_bytes(&Document::new(64, 64)).unwrap());
        let single = dir.join("single.mnc");
        save(&p, &single).unwrap();
        assert_eq!(sniff_kind(&single), MncKind::Comic);
        assert_eq!(sniff_kind(&dir.join("p001.ora")), MncKind::Unknown);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workfolder_file_names() {
        assert!(is_workfolder_file("work.mnc"));
        assert!(is_workfolder_file("p001.ora"));
        assert!(is_workfolder_file("p12.ora"));
        assert!(!is_workfolder_file("page.ora"));
        assert!(!is_workfolder_file("cover art.ora"));
        assert!(is_workfolder_file("work.mnc.tmp"));
        assert!(is_workfolder_file("p001.ora.tmp"));
        assert_eq!(page_file_name(7), "p007.ora");
    }

    #[test]
    fn project_roundtrip_preserves_pages_and_meta() {
        let mut d1 = Document::new(128, 128);
        d1.active_layer_mut()
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(3, 3, [0, 0, 0, FIX15_ONE as u16]);
        let d2 = Document::new(128, 128);

        let mut p = Project::new(
            "TEKNO".into(),
            Some(crate::page::PageSetup::presets()[0].clone()),
            true,
        );
        p.pages.push(doc_to_bytes(&d1).unwrap());
        p.pages.push(doc_to_bytes(&d2).unwrap());

        let mut buf = Cursor::new(Vec::new());
        save_to(&p, &mut buf).unwrap();
        let loaded = load_from(Cursor::new(buf.into_inner())).unwrap();

        assert_eq!(loaded.meta.story, "TEKNO");
        assert!(loaded.meta.binding_right);
        assert_eq!(loaded.pages.len(), 2);
        let back = bytes_to_doc(&loaded.pages[0]).unwrap();
        assert_eq!(
            back.active_layer()
                .tile(TileIdx::new(0, 0))
                .unwrap()
                .pixel(3, 3)[3],
            FIX15_ONE as u16
        );
        assert!(
            page_thumb(&loaded.pages[0]).is_some(),
            "ORA thumbnail readable"
        );
        assert_eq!(loaded.meta.setup.as_ref().unwrap().dpi, 600);
    }
}
