//! File objects (CSP `FO-001`–`009`, TRIAGE row 166).
//!
//! A layer that REFERENCES an image file on disk instead of owning its
//! pixels outright: the background reused across a whole chapter. Redraw
//! the background once, and every page that placed it as a file object
//! picks the new version up.
//!
//! # What is stored, and where
//!
//! The layer's raster is ORDINARY tiles — the same tiles a plain imported
//! image lands in — DERIVED from the file and re-derivable at any time.
//! [`LayerKind::FileObject`] carries only the recipe: the absolute path,
//! the fit box the pixels were scaled into, and the (mtime, length) stamp
//! the source had when the raster was last built.
//!
//! Keeping the pixels in `tiles` rather than in a derived cache
//! (`fill_tiles`, `corr`, `tone_tiles`) is the whole trick, and it buys
//! three things for free:
//!
//! * the ORA save writes the raster like any other layer, so the file
//!   opens correctly on a machine that has never seen the source;
//! * a BROKEN link keeps its last picture instead of going blank;
//! * a build that predates this module opens the file as a plain raster
//!   layer and shows the right image.
//!
//! # Refusing the brush
//!
//! `LayerKind` is not `Raster`, so [`crate::doc::Layer::is_vector`] is true
//! and [`crate::doc::Layer::paintable`] is false — every raster edit (fill,
//! gradient, transform, filter, clear, merge) already refuses through that
//! one predicate, exactly the way a frame or balloon layer does. Painting
//! on a file object would be thrown away by the next refresh, so it is
//! refused rather than accepted-then-lost.
//!
//! # Undo semantics — deliberate
//!
//! * **Import** ([`Document::add_file_object_layer`]) and **relink**
//!   ([`Document::relink_file_object`]) are the artist's own actions and
//!   each record ONE structure group: one undo press takes them back.
//! * **Refresh** ([`Document::refresh_file_objects`]) records NOTHING.
//!   This follows CSP: an update is EXTERNAL TRUTH arriving, not an edit
//!   the artist made, and "undo" for it is Ctrl+Z on the *other* app. The
//!   honest consequence, written down rather than hidden: an undo press
//!   after a refresh can restore an older stack that still holds the
//!   PREVIOUS raster, because a structure group snapshots whole layers.
//!   Redo puts the refreshed one back.
//!
//! # v1 limits (recorded, not worked around)
//!
//! * No background watcher thread. Updates happen when the app regains
//!   focus and on the explicit "Update file objects" command. A watcher is
//!   a thread, a debounce and a cross-platform dependency for a workflow
//!   whose natural rhythm is "alt-tab back from the other app".
//! * The change test is (mtime, length), not a content hash: a rewrite
//!   that lands in the same millisecond AND keeps the same byte length is
//!   missed. The explicit Update command is the escape hatch.
//! * Transform / tiling / effects on the reference (`FO-006`) are absent —
//!   the fit is scale-to-fit, centred, like every other image import here.

use crate::doc::{Document, Layer, LayerKind};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The source file's identity at the moment its pixels were last read.
/// Millisecond mtime (NTFS is finer, JSON integers are not the place for
/// 100 ns ticks) plus the byte length, because a same-second rewrite of a
/// different size is the common case a bare mtime misses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    /// Milliseconds since the Unix epoch; negative for pre-1970 files.
    pub mtime_ms: i64,
    pub len: u64,
}

/// A layer's link to an external image (`FO-001`, `FO-007`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileObject {
    /// The source, ABSOLUTE. Absolute because a file object's whole point
    /// is surviving where the .mnc moves to; the portability half is the
    /// basename-beside-the-work fallback in [`resolve`].
    pub path: PathBuf,
    /// The box the pixels were scaled into (the canvas size at import).
    /// Stored rather than recomputed so a refresh after a page resize
    /// re-derives at the SAME size the artist placed — the picture does not
    /// jump because the paper changed.
    pub fit: (u32, u32),
    /// What the source looked like when the raster was built.
    #[serde(default)]
    pub stamp: FileStamp,
    /// The last resolve could not read the source. Runtime state, never
    /// persisted: "missing" is a fact about THIS machine right now, and a
    /// file saved on a laptop must not tell the studio desktop its own
    /// perfectly present background is gone.
    #[serde(skip)]
    pub missing: bool,
}

impl FileObject {
    /// The name a file-object layer takes: the source's stem.
    pub fn layer_name(path: &Path) -> String {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "File object".to_owned())
    }
}

/// The stored form of a source path: ABSOLUTE.
///
/// A relative path means "relative to whatever the process's working
/// directory happened to be", which is not a fact about the artwork — it
/// is a fact about how the app was launched, and it stops being true the
/// next session. Joined with the current directory rather than run through
/// `canonicalize`, which on Windows returns `\\?\C:\…` verbatim-UNC paths:
/// correct, ugly in the status line, and a needless change to what the
/// artist typed.
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

/// The source file's current stamp, or `None` when it cannot be stat'd.
pub fn stamp_of(path: &Path) -> Option<FileStamp> {
    let md = std::fs::metadata(path).ok()?;
    let mtime_ms = md
        .modified()
        .ok()
        .map(|t| match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_millis() as i64,
            // Pre-1970 mtime: keep the sign rather than clamp, so two such
            // files still compare unequal.
            Err(e) => -(e.duration().as_millis() as i64),
        })
        .unwrap_or(0);
    Some(FileStamp {
        mtime_ms,
        len: md.len(),
    })
}

/// Where a file object's source actually is, right now.
///
/// The stored absolute path first. If that is gone, try the SAME FILE NAME
/// beside the work (`near` = the folder of the .mnc / .ora): the cheap
/// portability win — a chapter folder copied to another machine, or the
/// backgrounds moved next to the pages, both keep working, and neither
/// costs a dialog. Anything else is a broken link, repaired by
/// `FO-009` (relink) rather than guessed at.
pub fn resolve(fo: &FileObject, near: Option<&Path>) -> Option<PathBuf> {
    if fo.path.is_file() {
        return Some(fo.path.clone());
    }
    let dir = near?;
    let cand = dir.join(fo.path.file_name()?);
    cand.is_file().then_some(cand)
}

/// Read `path` and scale it to sit inside `fit`, never enlarging.
///
/// Shrink-only is the *Import Image as Layer* rule (`cmd::import_image_layer`),
/// not the page-underlay rule (`app::pages::fit_to_paper`, which also scales
/// UP to fill the paper): a file object is an import-as-layer sibling, and
/// blowing a small logo up to page height on its way in would be a decision
/// nobody asked for.
pub fn rasterize(path: &Path, fit: (u32, u32)) -> Result<image::RgbaImage, String> {
    let img = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
    let (iw, ih) = (img.width(), img.height());
    let (fw, fh) = (fit.0.max(1), fit.1.max(1));
    if iw <= fw && ih <= fh {
        return Ok(img);
    }
    let s = (fw as f32 / iw as f32).min(fh as f32 / ih as f32);
    Ok(image::imageops::resize(
        &img,
        ((iw as f32 * s).round() as u32).max(1),
        ((ih as f32 * s).round() as u32).max(1),
        image::imageops::FilterType::Lanczos3,
    ))
}

/// What one [`Document::refresh_file_objects`] pass did (`FO-008`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefreshReport {
    /// File-object layers seen.
    pub checked: usize,
    /// Layers whose raster was re-derived from a changed (or recovered)
    /// source.
    pub updated: usize,
    /// Layers whose source could not be read — raster kept, row flagged.
    pub missing: usize,
    /// Layers whose absolute path was gone but whose file was found beside
    /// the work; the stored path was re-aimed at it.
    pub repathed: usize,
}

impl RefreshReport {
    /// Nothing to say — no file objects, or every one of them unchanged
    /// and present. The caller uses this to stay SILENT on the focus-regain
    /// path: a status line that fires every alt-tab is noise.
    pub fn is_quiet(self) -> bool {
        self.updated == 0 && self.missing == 0 && self.repathed == 0
    }

    /// One line for the status bar, or `None` when [`Self::is_quiet`].
    pub fn status(self) -> Option<String> {
        if self.is_quiet() {
            return None;
        }
        let mut s = String::new();
        if self.updated > 0 {
            s.push_str(&format!("{} file object(s) updated", self.updated));
        }
        if self.repathed > 0 {
            if !s.is_empty() {
                s.push_str(" — ");
            }
            s.push_str(&format!("{} found beside the work", self.repathed));
        }
        if self.missing > 0 {
            if !s.is_empty() {
                s.push_str(" — ");
            }
            s.push_str(&format!(
                "{} source(s) missing (last picture kept; Layer ▸ Relink file object…)",
                self.missing
            ));
        }
        Some(s)
    }
}

/// Build a centred layer raster for `img` on a `size` canvas, reusing
/// [`Document::add_layer_from_image`]'s centring through a throwaway
/// document — the `place_draft_underlay` idiom. Doing it on the real
/// document would push a "New layer" undo group nobody asked for.
fn derived_layer(size: (u32, u32), name: &str, img: &image::RgbaImage) -> Layer {
    let mut scratch = Document::new(size.0.max(1), size.1.max(1));
    let at = scratch.add_layer_from_image(name.to_owned(), img);
    scratch.layers.remove(at)
}

/// Just the tiles of [`derived_layer`] — what a re-derive swaps in.
fn derived_tiles(
    size: (u32, u32),
    img: &image::RgbaImage,
) -> std::collections::HashMap<crate::tile::TileIdx, std::sync::Arc<crate::tile::Tile>> {
    derived_layer(size, "", img).take_tiles()
}

impl Document {
    /// `FO-001` — import `path` as a file object layer above the active one.
    ///
    /// One structure group ⇒ one undo press, the `add_layer_above` shape
    /// (clip-run hop included) with the layer built outside so the history
    /// never sees the intermediate.
    pub fn add_file_object_layer(&mut self, path: &Path) -> Result<usize, String> {
        let img = rasterize(path, self.size)?;
        let fo = FileObject {
            path: absolute(path),
            fit: self.size,
            stamp: stamp_of(path).unwrap_or_default(),
            missing: false,
        };
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let mut layer = derived_layer(self.size, &FileObject::layer_name(path), &img);
        layer.kind = LayerKind::FileObject(fo);
        // Same landing spot as a new layer: above the active one, hopping
        // clear of a clip run so nothing already clipped goes invisible.
        let index = self.clip_run_top(self.active);
        let at = (index + 1).min(self.layers.len());
        layer.depth = self.layers.get(index).map(|x| x.depth).unwrap_or(0);
        self.layers.insert(at, layer);
        self.active = at;
        self.normalize_depths();
        self.record_structure("Import file object", before, active_before);
        self.touch();
        Ok(at)
    }

    /// `FO-009` — re-aim layer `li`'s reference at `path` and re-derive.
    /// The repair path for a broken link, and the "use a different file"
    /// path when the link is fine. One undo press.
    pub fn relink_file_object(&mut self, li: usize, path: &Path) -> Result<(), String> {
        let Some(fo) = self.layers.get(li).and_then(|l| l.file_object()) else {
            return Err("that layer is not a file object".into());
        };
        let fit = fo.fit;
        let img = rasterize(path, fit)?;
        let (before, active_before) = (self.stack_snapshot(), self.active);
        let tiles = derived_tiles(self.size, &img);
        let l = &mut self.layers[li];
        l.replace_tiles(tiles);
        l.kind = LayerKind::FileObject(FileObject {
            path: absolute(path),
            fit,
            stamp: stamp_of(path).unwrap_or_default(),
            missing: false,
        });
        self.record_structure("Relink file object", before, active_before);
        self.touch();
        Ok(())
    }

    /// `FO-008` — re-derive every file-object layer whose source changed.
    ///
    /// `near` is the folder the document was loaded from (the .mnc / .ora
    /// work folder), used for the basename fallback in [`resolve`]. Records
    /// no undo (see the module doc); `touch()` runs only when something
    /// actually changed, so an idle alt-tab does not dirty the document.
    pub fn refresh_file_objects(&mut self, near: Option<&Path>) -> RefreshReport {
        let mut r = RefreshReport::default();
        for li in 0..self.layers.len() {
            let Some(fo) = self.layers[li].file_object() else {
                continue;
            };
            r.checked += 1;
            let fo = fo.clone();
            let Some(found) = resolve(&fo, near) else {
                r.missing += 1;
                if let LayerKind::FileObject(f) = &mut self.layers[li].kind {
                    f.missing = true;
                }
                continue;
            };
            let repathed = found != fo.path;
            let stamp = stamp_of(&found).unwrap_or_default();
            // A recovered link re-derives even when the stamp matches: the
            // raster on screen came from whatever the file said the LAST
            // time it was readable, which may be a different file entirely.
            if !repathed && !fo.missing && stamp == fo.stamp {
                continue;
            }
            let img = match rasterize(&found, fo.fit) {
                Ok(i) => i,
                Err(_) => {
                    // Present but undecodable (half-written, or somebody
                    // renamed a .psd to .png): that is a broken link too.
                    r.missing += 1;
                    if let LayerKind::FileObject(f) = &mut self.layers[li].kind {
                        f.missing = true;
                    }
                    continue;
                }
            };
            let tiles = derived_tiles(self.size, &img);
            self.layers[li].replace_tiles(tiles);
            if let LayerKind::FileObject(f) = &mut self.layers[li].kind {
                f.path = found;
                f.stamp = stamp;
                f.missing = false;
            }
            r.updated += 1;
            if repathed {
                r.repathed += 1;
            }
        }
        if !r.is_quiet() {
            self.touch();
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Background, composite};
    use crate::tile::TileIdx;

    /// A scratch directory under the OS temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let d = std::env::temp_dir().join(format!("mn-fileobj-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).expect("temp dir");
            TempDir(d)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Write a `w x h` PNG of one opaque colour.
    fn write_png(path: &Path, w: u32, h: u32, rgb: [u8; 3]) {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
        img.save(path).expect("write png");
        // Windows mtime resolution is fine, but a test that writes twice in
        // the same tick would compare equal. Nudge the stamp so the change
        // test sees what a human editing in another app would produce.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    /// The centre pixel of the EXPORT-rules composite, straight RGBA.
    fn centre(doc: &Document) -> [u8; 4] {
        let img = composite(doc, Background::Transparent);
        img.get_pixel(doc.size.0 / 2, doc.size.1 / 2).0
    }

    /// `FO-001`: importing derives the raster FROM the file, and what the
    /// artist sees — the composite, not "some tiles exist" — is the image.
    #[test]
    fn import_derives_the_raster_and_the_composite_shows_it() {
        let d = TempDir::new("import");
        let src = d.path("bg.png");
        write_png(&src, 64, 64, [200, 30, 40]);

        let mut doc = Document::new(128, 128);
        let at = doc.add_file_object_layer(&src).expect("import");
        assert!(doc.layers[at].file_object().is_some(), "kind carries the ref");
        assert_eq!(doc.layers[at].name, "bg");
        let px = centre(&doc);
        assert_eq!(
            [px[0], px[1], px[2], px[3]],
            [200, 30, 40, 255],
            "the composite shows the file's pixels"
        );
        let fo = doc.layers[at].file_object().unwrap();
        assert_eq!(fo.fit, (128, 128));
        assert!(fo.stamp.len > 0 && !fo.missing);
    }

    /// The import is ONE undo press — the whole point of building the layer
    /// outside the document and recording a single structure group.
    #[test]
    fn import_file_object_is_one_undo_press() {
        let d = TempDir::new("undo");
        let src = d.path("bg.png");
        write_png(&src, 32, 32, [10, 220, 60]);

        let mut doc = Document::new(64, 64);
        let n = doc.layers.len();
        doc.add_file_object_layer(&src).expect("import");
        assert_eq!(doc.layers.len(), n + 1);
        assert!(doc.undo(), "one press");
        assert_eq!(doc.layers.len(), n, "and the layer is gone after it");
        assert!(
            doc.layers.iter().all(|l| l.file_object().is_none()),
            "no file object survives the single press"
        );
    }

    /// The derived raster is not hand-editable: `paintable()` is false, so
    /// every raster op refuses through the one predicate they all share.
    #[test]
    fn file_object_refuses_the_brush_and_every_raster_edit() {
        let d = TempDir::new("refuse");
        let src = d.path("bg.png");
        write_png(&src, 32, 32, [0, 0, 255]);

        let mut doc = Document::new(64, 64);
        let at = doc.add_file_object_layer(&src).expect("import");
        assert!(doc.layers[at].is_vector(), "not a plain raster");
        assert!(!doc.layers[at].paintable(), "refuses raster edits");
        // A concrete one, end to end: the gradient tool paints nothing.
        let ramp = crate::gradient::Ramp::new(
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
            Default::default(),
            Default::default(),
        );
        assert!(
            !doc.paint_gradient_ramp([0.0, 0.0], [64.0, 64.0], &ramp),
            "a raster paint on a file object is refused, not silently lost"
        );
        let px = centre(&doc);
        assert_eq!([px[0], px[1], px[2]], [0, 0, 255], "the picture is intact");
    }

    /// `FO-008`: a changed source is picked up by a refresh, through the
    /// SAME fit params, and an unchanged one costs nothing.
    #[test]
    fn refresh_picks_up_a_changed_file() {
        let d = TempDir::new("refresh");
        let src = d.path("bg.png");
        write_png(&src, 64, 64, [200, 30, 40]);

        let mut doc = Document::new(128, 128);
        let at = doc.add_file_object_layer(&src).expect("import");
        let rev = doc.revision;

        // Nothing changed: no work, no dirtying.
        let r = doc.refresh_file_objects(None);
        assert_eq!(r.checked, 1);
        assert!(r.is_quiet(), "an unchanged source is a no-op: {r:?}");
        assert_eq!(doc.revision, rev, "and does not touch the document");

        // The artist redrew the background in the other app.
        write_png(&src, 64, 64, [20, 90, 240]);
        let r = doc.refresh_file_objects(None);
        assert_eq!((r.updated, r.missing), (1, 0), "{r:?}");
        let px = centre(&doc);
        assert_eq!(
            [px[0], px[1], px[2]],
            [20, 90, 240],
            "the composite shows the NEW file"
        );
        assert_eq!(
            doc.layers[at].file_object().unwrap().stamp,
            stamp_of(&src).unwrap(),
            "the stamp advanced with it"
        );
        assert!(doc.revision > rev, "a real change dirties the document");
    }

    /// The fit params survive: a source that grows re-derives into the box
    /// the artist placed it in, not into whatever the canvas is now.
    #[test]
    fn refresh_reuses_the_stored_fit_box() {
        let d = TempDir::new("fit");
        let src = d.path("bg.png");
        write_png(&src, 400, 400, [9, 9, 9]);

        let mut doc = Document::new(100, 100);
        let at = doc.add_file_object_layer(&src).expect("import");
        assert_eq!(doc.layers[at].file_object().unwrap().fit, (100, 100));
        let (_, _, w0, h0) = doc.layers[at].tile_bounds().expect("pixels");

        write_png(&src, 800, 800, [9, 9, 9]);
        let r = doc.refresh_file_objects(None);
        assert_eq!(r.updated, 1);
        let (_, _, w1, h1) = doc.layers[at].tile_bounds().expect("pixels");
        assert_eq!((w0, h0), (w1, h1), "same fit box ⇒ same on-page size");
    }

    /// A vanished source keeps the last picture and flags the layer — never
    /// a blank layer, never a dialog.
    #[test]
    fn missing_file_keeps_the_last_raster_and_flags_the_layer() {
        let d = TempDir::new("missing");
        let src = d.path("bg.png");
        write_png(&src, 64, 64, [200, 30, 40]);

        let mut doc = Document::new(128, 128);
        let at = doc.add_file_object_layer(&src).expect("import");
        std::fs::remove_file(&src).expect("delete the source");

        let r = doc.refresh_file_objects(None);
        assert_eq!((r.updated, r.missing), (0, 1), "{r:?}");
        assert!(
            doc.layers[at].file_object().unwrap().missing,
            "the row can say so"
        );
        let px = centre(&doc);
        assert_eq!(
            [px[0], px[1], px[2], px[3]],
            [200, 30, 40, 255],
            "the last picture is still there"
        );
        assert!(r.status().is_some_and(|s| s.contains("missing")));
    }

    /// `FO-009`: relink re-aims the reference and re-derives, in one press.
    #[test]
    fn relink_re_derives_from_the_new_file() {
        let d = TempDir::new("relink");
        let (a, b) = (d.path("a.png"), d.path("b.png"));
        write_png(&a, 64, 64, [200, 30, 40]);
        write_png(&b, 64, 64, [30, 200, 40]);

        let mut doc = Document::new(128, 128);
        let at = doc.add_file_object_layer(&a).expect("import");
        std::fs::remove_file(&a).expect("break the link");
        doc.refresh_file_objects(None);
        assert!(doc.layers[at].file_object().unwrap().missing);

        doc.relink_file_object(at, &b).expect("relink");
        let fo = doc.layers[at].file_object().unwrap();
        assert_eq!(fo.path, b);
        assert!(!fo.missing, "the link is whole again");
        let px = centre(&doc);
        assert_eq!([px[0], px[1], px[2]], [30, 200, 40], "and it re-derived");

        assert!(doc.undo(), "relink is one undo press");
        let px = centre(&doc);
        assert_eq!([px[0], px[1], px[2]], [200, 30, 40], "back to a.png's pixels");

        // Relinking something that is not a file object is refused, not a
        // panic and not a silent no-op.
        let plain = doc.add_layer("plain");
        assert!(doc.relink_file_object(plain, &b).is_err());
    }

    /// The portability fallback: the absolute path is gone, but the same
    /// file name sits beside the work. Found, re-derived, and the stored
    /// path is re-aimed so the next refresh is a straight hit.
    #[test]
    fn a_moved_source_is_found_beside_the_work() {
        let d = TempDir::new("beside");
        let away = d.path("elsewhere");
        std::fs::create_dir_all(&away).expect("dir");
        let original = away.join("bg.png");
        write_png(&original, 64, 64, [200, 30, 40]);

        let mut doc = Document::new(128, 128);
        let at = doc.add_file_object_layer(&original).expect("import");

        // The chapter was copied to another machine: the old absolute path
        // is meaningless, but the background travelled with the pages.
        std::fs::remove_dir_all(&away).expect("drop the old location");
        let beside = d.path("bg.png");
        write_png(&beside, 64, 64, [20, 90, 240]);

        let r = doc.refresh_file_objects(Some(&d.0));
        assert_eq!((r.updated, r.repathed, r.missing), (1, 1, 0), "{r:?}");
        assert_eq!(doc.layers[at].file_object().unwrap().path, beside);
        let px = centre(&doc);
        assert_eq!([px[0], px[1], px[2]], [20, 90, 240]);

        // Without the hint it is simply missing — the fallback never guesses
        // outside the work folder.
        let mut doc2 = Document::new(128, 128);
        doc2.layers[0].kind = LayerKind::FileObject(FileObject {
            path: away.join("bg.png"),
            fit: (128, 128),
            stamp: FileStamp::default(),
            missing: false,
        });
        assert_eq!(doc2.refresh_file_objects(None).missing, 1);
    }

    /// The stored path is absolute even when the caller handed over a
    /// relative one — a link that only resolves from the directory the app
    /// happened to be launched from is not a link.
    #[test]
    fn the_stored_path_is_always_absolute() {
        let d = TempDir::new("abs");
        let src = d.path("bg.png");
        write_png(&src, 8, 8, [7, 7, 7]);
        // A relative path built by walking DOWN from the cwd: it does not
        // have to exist for `absolute` to be the right answer, but making it
        // resolvable keeps this an end-to-end test.
        let cwd = std::env::current_dir().expect("cwd");
        let Ok(rel) = src.strip_prefix(&cwd) else {
            // The temp dir is on another volume (the usual case on this
            // machine): the unit-level check still holds.
            assert!(absolute(Path::new("some/relative.png")).is_absolute());
            return;
        };
        let mut doc = Document::new(64, 64);
        let at = doc.add_file_object_layer(rel).expect("import");
        assert!(doc.layers[at].file_object().unwrap().path.is_absolute());
    }

    /// Shrink-only, like Import Image as Layer: a small file is placed at
    /// its own size rather than blown up to the page.
    #[test]
    fn a_small_source_is_not_enlarged() {
        let d = TempDir::new("small");
        let src = d.path("logo.png");
        write_png(&src, 16, 16, [1, 2, 3]);

        let mut doc = Document::new(256, 256);
        let at = doc.add_file_object_layer(&src).expect("import");
        // Centred at its own 16x16: the tile the centre falls in has ink,
        // the far corner does not.
        assert_eq!(centre(&doc)[0], 1);
        assert!(
            doc.layers[at].display_tile(TileIdx::new(0, 0)).is_none()
                || doc.layers[at]
                    .display_tile(TileIdx::new(0, 0))
                    .is_some_and(|t| t.pixel(0, 0)[3] == 0),
            "not scaled up to fill the canvas"
        );
    }
}
