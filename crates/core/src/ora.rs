//! OpenRaster (`.ora`) save/load — our native document format.
//!
//! ORA is a zip with a fixed shape:
//!
//! ```text
//! mimetype                  "image/openraster", STORED, first entry
//! stack.xml                 layer stack, TOP-FIRST in document order
//! data/layer0.png           one PNG per layer, cropped, with x/y offsets
//! mergedimage.png           the flattened image (required since 0.0.2)
//! Thumbnails/thumbnail.png  <= 256 px on the long edge
//! ```
//!
//! Why ORA and not a bespoke format: Krita, MyPaint, GIMP and Clip Studio can
//! all read it, so nothing the owner draws is ever trapped in this app.
//!
//! # Ordering
//!
//! `Document::layers[0]` is the **bottom** layer. `stack.xml` lists layers
//! **top-first**. This module reverses on the way out and on the way in; that is
//! the only place the two conventions meet.
//!
//! # Round-trip fidelity (read this before "fixing" a failing test)
//!
//! Tiles hold *premultiplied fix15*; PNG holds *straight 8-bit*. Save
//! un-premultiplies to 8-bit, load re-premultiplies to fix15. That pair is
//! exactly lossless for every colour whose alpha is **>= 2/255**: the fix15
//! alpha is then >= 257, so the premultiplied colour has more levels than the
//! 8-bit colour it came from and the rounding inverts.
//!
//! At alpha == 1/255 (fix15 alpha 129) there are only 129 premultiplied levels
//! for 256 possible colour values, so the colour of a *just barely visible*
//! pixel can shift by one 8-bit step. This is inherent to premultiplied storage
//! (MyPaint and Krita have the same bottom-bit behaviour) and is invisible: at
//! alpha 1/255 a full-scale colour error changes the composited result by less
//! than half a level.
//!
//! What is guaranteed, and tested:
//! * structure (names, order, opacity, visibility, blend, offsets) is exact;
//! * pixel data that *came from* 8-bit sources round-trips bit-exactly;
//! * save -> load -> save is stable (idempotent) for arbitrary fix15 data.
//!
//! # Known limitations
//!
//! * Group stacks (`<stack>` nested inside `<stack>`) load as **layer
//!   folders** with the group's opacity/visibility; our folders save as
//!   nested stacks, so Krita/GIMP see real groups. A frame folder addition-
//!   ally writes its derived mask raster as a child layer tagged
//!   `mnc-folder-raster` for foreign readers (we skip it and re-rasterize).
//! * Composite ops outside our phase-1 set (`svg:src-over`, `svg:multiply`,
//!   `svg:screen`) load as Normal, per the ORA spec's fallback rule.
//! * Layer `selected`/`isolation`/`edit-locked` attributes are ignored.

use std::io::{Cursor, Read, Seek, Write};
use std::path::Path;
use std::sync::Arc;

use crate::balloon::BalloonSet;
use crate::blend::straight_u8_to_fix15;
use crate::doc::{Blend, Document, Layer, LayerKind, Paper};
use crate::export::{self, Background};
use crate::frame::FrameSet;
use crate::text::TextSet;
use crate::tile::{TILE_LEN, TILE_SIZE, Tile, TileIdx};

/// Longest edge of `Thumbnails/thumbnail.png`, per the ORA spec.
const THUMB_MAX: u32 = 256;

#[derive(Debug)]
pub struct OraError(pub String);

impl std::fmt::Display for OraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ora: {}", self.0)
    }
}
impl std::error::Error for OraError {}

impl From<std::io::Error> for OraError {
    fn from(e: std::io::Error) -> Self {
        OraError(format!("io: {e}"))
    }
}
impl From<zip::result::ZipError> for OraError {
    fn from(e: zip::result::ZipError) -> Self {
        OraError(format!("zip: {e}"))
    }
}
impl From<image::ImageError> for OraError {
    fn from(e: image::ImageError) -> Self {
        OraError(format!("png: {e}"))
    }
}
impl From<quick_xml::Error> for OraError {
    fn from(e: quick_xml::Error) -> Self {
        OraError(format!("xml: {e}"))
    }
}

// ------------------------------------------------------------------- save --

/// The sibling `<name>.tmp` an atomic write builds into.
///
/// Appended, not substituted: `with_extension("tmp")` would map both
/// `work.mnc` and `work.ora` onto `work.tmp`, and `project::is_workfolder_file`
/// already recognises the append form.
fn tmp_sibling(path: &Path) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    std::path::PathBuf::from(s)
}
/// Build a file in a sibling `.tmp` and rename it over `path` only once it is
/// completely written.
///
/// Two failures this exists to prevent, both of which the previous
/// `File::create(path)` shape had (audit 2026-08-17, finding H2):
///
/// 1. **Truncation.** `File::create` empties the user's existing file before a
///    single new byte is written, so a crash or a full disk mid-save destroyed
///    the old version. Here the old file is untouched until the rename, which
///    is atomic on Windows and POSIX alike.
/// 2. **The swallowed flush.** `BufWriter`'s `Drop` flushes and *discards the
///    error*, so a failed final write returned `Ok(())` and the app reported a
///    successful save of a truncated file. The explicit `flush()?` below is
///    that error.
///
/// The `.tmp` is removed on every failure path so a failed save leaves no
/// debris beside the user's file.
pub(crate) fn write_atomic<F>(path: &Path, write: F) -> Result<(), OraError>
where
    F: FnOnce(&mut std::io::BufWriter<std::fs::File>) -> Result<(), OraError>,
{
    let tmp = tmp_sibling(path);
    let built = (|| {
        let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        write(&mut w)?;
        w.flush()?;
        Ok(())
    })()
    .and_then(|()| std::fs::rename(&tmp, path).map_err(OraError::from));
    if built.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    built
}

/// Write `doc` to `path` as an `.ora`. Atomic: see [`write_atomic`].
pub fn save(doc: &Document, path: &Path) -> Result<(), OraError> {
    write_atomic(path, |w| save_to(doc, w))
}

/// Write `doc` as `.ora` into any seekable sink (tests use a `Cursor`).
pub fn save_to<W: Write + Seek>(doc: &Document, sink: W) -> Result<(), OraError> {
    save_to_with(doc, sink, None)
}

/// Same, carrying a page PREVIEW (owner preview tier, 2026-08-18): raw
/// gray-8 PNG bytes written as `mnc/preview.png` — our own zip entry,
/// OUTSIDE `Thumbnails/`, so foreign ORA readers ignore it and old files
/// stay old-shape (`save_to` writes no entry). Rendered by the caller
/// with EXPORT rules (drafts off) at long edge 1600 — see
/// `App::render_page_preview_png`.
pub fn save_to_with<W: Write + Seek>(
    doc: &Document,
    sink: W,
    preview_png: Option<&[u8]>,
) -> Result<(), OraError> {
    use zip::write::SimpleFileOptions;

    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut zw = zip::ZipWriter::new(sink);

    // 1. mimetype — MUST be the first entry and MUST be stored uncompressed, so
    //    that `file(1)`-style sniffing can read it at a fixed offset.
    zw.start_file("mimetype", stored)?;
    zw.write_all(b"image/openraster")?;

    // 2. layer PNGs (data/layerN.png, N = bottom-first document index) and the
    //    XML that describes them.
    let mut entries: Vec<LayerEntry> = Vec::with_capacity(doc.layers.len());
    for (i, layer) in doc.layers.iter().enumerate() {
        let src = format!("data/layer{i}.png");
        let (img, x, y) = match export::layer_image(layer) {
            Some(v) => v,
            // The spec wants a src on every layer; an empty layer gets a 1x1
            // transparent pixel rather than a missing file.
            None => (image::RgbaImage::new(1, 1), 0, 0),
        };
        zw.start_file(&src, deflated)?;
        zw.write_all(&encode_png(&img)?)?;
        // TRIAGE 138 p2: the layer mask as its own PNG (alpha = coverage,
        // RGB mirrors it for foreign readers' eyes). H2/M1: the image is
        // bbox-cropped, so its pixel origin and the exact tile set ride as
        // attrs next to the path.
        // Vector inking: the stroke record rides as its own zip entry —
        // bulk sample data has no business in a stack.xml attribute, and a
        // foreign OpenRaster app ignores the extra file while still seeing
        // the layer's rendered PNG (editability degrades, pixels never do).
        let strokes_src = layer.strokes.as_ref().map(|set| {
            let ss = format!("data/layer{i}.strokes.json");
            zw.start_file(&ss, deflated).ok();
            zw.write_all(set.to_json().as_bytes()).ok();
            ss
        });
        let mut mask_org = None;
        let mut mask_tiles = None;
        let mask_src = layer.mask.as_ref().map(|m| {
            let ms = format!("data/layer{i}.mask.png");
            let (img, mx, my) = mask_image(m);
            zw.start_file(&ms, deflated).ok();
            if let Some(png) = encode_png(&img).ok() {
                zw.write_all(&png).ok();
            }
            // Sorted: identical documents save identical bytes.
            let mut idxs: Vec<TileIdx> = m.tiles.keys().copied().collect();
            idxs.sort_by_key(|i| i.origin());
            mask_org = Some((mx, my));
            // F3 (audit r69-78): a mask whose tile set IS its bounding
            // rectangle — the common fully-inked case, ~5,760 pairs on a
            // B4 600dpi layer — omits the list. The loader treats
            // "org present, list absent" as the solid bbox (the
            // pre-list semantics), so only genuinely holed masks pay
            // the attribute.
            let solid = {
                let n = idxs.len();
                let (minx, miny) = (
                    idxs.iter().map(|i| i.x).min(),
                    idxs.iter().map(|i| i.y).min(),
                );
                let (maxx, maxy) = (
                    idxs.iter().map(|i| i.x).max(),
                    idxs.iter().map(|i| i.y).max(),
                );
                match (minx, miny, maxx, maxy) {
                    (Some(a), Some(b), Some(c), Some(d)) => {
                        let (w, h) = ((c - a + 1).max(0) as usize, (d - b + 1).max(0) as usize);
                        w.checked_mul(h) == Some(n)
                    }
                    _ => false,
                }
            };
            mask_tiles = if solid { None } else { Some(idxs) };
            ms
        });
        entries.push(LayerEntry {
            name: layer.name.clone(),
            src,
            x,
            y,
            opacity: layer.opacity,
            visible: layer.visible,
            blend: layer.blend,
            // Vector frame/balloon state rides along as private JSON; the PNG
            // above is the raster fallback any other ORA reader will show.
            frames: layer.frames().and_then(|fs| serde_json::to_string(fs).ok()),
            balloons: layer
                .balloons()
                .and_then(|bs| serde_json::to_string(bs).ok()),
            texts: layer.texts().and_then(|ts| serde_json::to_string(ts).ok()),
            label: layer.label,
            layer_colour: layer
                .layer_colour
                .map(|c| format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])),
            layer_sub_colour: layer
                .layer_sub_colour
                .map(|c| format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])),
            expression: layer.expression,
            depth: layer.depth,
            folder: layer.folder,
            open: layer.open,
            has_pixels: !layer.is_empty(),
            clip: layer.clip,
            lock: layer.lock,
            lock_alpha: layer.lock_alpha,
            reference: layer.reference,
            draft: layer.draft,
            through: layer.folder && layer.through,
            // Screen params ride as private JSON; the PNG above stays the
            // painted SOURCE ink (our loader re-derives the halftone from it).
            tone: layer.tone.and_then(|t| serde_json::to_string(&t).ok()),
            // Same deal for the border effect: the outline is derived, so
            // only the params are stored and the raster rebuilds on load.
            edge: layer.edge.and_then(|e| serde_json::to_string(&e).ok()),
            genlines: layer.genlines.and_then(|g| serde_json::to_string(&g).ok()),
            fill: match &layer.kind {
                LayerKind::Fill(k) => serde_json::to_string(k).ok(),
                _ => None,
            },
            mask_src,
            strokes_src,
            mask_enabled: layer.mask.as_ref().map(|m| m.enabled),
            mask_unlinked: (layer.mask.is_some() && !layer.mask_linked).then_some(true),
            mask_org,
            mask_tiles,
        });
    }

    zw.start_file("stack.xml", deflated)?;
    zw.write_all(stack_xml(doc.size.0, doc.size.1, &entries, &doc.comps, doc.paper).as_bytes())?;

    // 3. mergedimage.png — the flattened document, alpha preserved. SCREEN
    // semantics (drafts included): this image is also the Pages-panel
    // thumbnail source; real export paths (save_png) drop drafts.
    let merged = export::composite(doc, Background::Transparent);
    zw.start_file("mergedimage.png", deflated)?;
    zw.write_all(&encode_png(&merged)?)?;

    // 4. Thumbnails/thumbnail.png — <= 256 px on the long edge.
    let (mw, mh) = (merged.width().max(1), merged.height().max(1));
    let scale = (THUMB_MAX as f32 / mw.max(mh) as f32).min(1.0);
    let (tw, th) = (
        ((mw as f32 * scale).round() as u32).max(1),
        ((mh as f32 * scale).round() as u32).max(1),
    );
    let thumb = image::imageops::resize(&merged, tw, th, image::imageops::FilterType::Triangle);
    zw.start_file("Thumbnails/thumbnail.png", deflated)?;
    zw.write_all(&encode_png(&thumb)?)?;

    // 5. mnc/preview.png — the SHARP preview (owner preview tier): gray-8,
    // long edge 1600, export rules. Own namespace, opt-in bytes from the
    // caller; absent = pre-tier file, loads exactly as before.
    if let Some(png) = preview_png {
        zw.start_file("mnc/preview.png", deflated)?;
        zw.write_all(png)?;
    }

    zw.finish()?;
    Ok(())
}

struct LayerEntry {
    name: String,
    src: String,
    x: i32,
    y: i32,
    opacity: f32,
    visible: bool,
    blend: Blend,
    frames: Option<String>,
    balloons: Option<String>,
    texts: Option<String>,
    label: Option<[u8; 3]>,
    depth: u8,
    folder: bool,
    open: bool,
    has_pixels: bool,
    clip: bool,
    lock: bool,
    lock_alpha: bool,
    reference: bool,
    draft: bool,
    through: bool,
    /// TRIAGE 138 p2: the mask PNG's zip path (+ None = unmasked).
    mask_src: Option<String>,
    /// Vector inking: the stroke-record sidecar's zip path
    /// (`data/layerN.strokes.json`); None = an ordinary raster layer.
    strokes_src: Option<String>,
    mask_enabled: Option<bool>,
    mask_unlinked: Option<bool>,
    /// Audit H2/M1 (rounds 50-68): the mask PNG is cropped to its tiles'
    /// bbox — the crop's PIXEL origin on the canvas, without which a
    /// corner mask reloaded at (0,0) and hid the wrong region.
    mask_org: Option<(i32, i32)>,
    /// The exact tile set the mask carries. Absent runtime tiles inside
    /// the saved bbox (holes) must not come back as zero-coverage tiles
    /// (= hidden): absent means UNMASKED. Only the recorded set separates
    /// a real all-zero tile (MaskClear) from a hole. None = legacy file.
    mask_tiles: Option<Vec<TileIdx>>,
    tone: Option<String>,
    genlines: Option<String>,
    fill: Option<String>,
    /// LP-002/LP-003 border-effect params, JSON.
    edge: Option<String>,
    /// LP-016 layer colour, hex RRGGBB.
    layer_colour: Option<String>,
    /// LP-017 two-tone SUB colour, hex RRGGBB.
    layer_sub_colour: Option<String>,
    /// LP-022 decrease-colour preview; the default writes no attribute.
    expression: crate::doc::LayerExpression,
}

fn encode_png(img: &image::RgbaImage) -> Result<Vec<u8>, OraError> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)?;
    Ok(buf)
}

/// Canvas size from ORA bytes WITHOUT decoding pixels: the `<image>` root's
/// `w`/`h` attributes in stack.xml. The reader's 1:1 view needs each stashed
/// page's true size (a combined spread is a wider page); this never touches
/// a raster. `None` on anything malformed — callers fall back to a sane
/// default rather than trust a guess.
pub fn ora_canvas_size(bytes: &[u8]) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut s = String::new();
    z.by_name("stack.xml").ok()?.read_to_string(&mut s).ok()?;
    let start = s.find("<image")?;
    let end = start + s[start..].find('>')?;
    let tag = &s[start..end];
    // The leading space keeps `w` from matching inside another attribute's
    // value; `w`/`h` sit before any payload attribute on the image element.
    let attr = |k: &str| -> Option<u32> {
        let pat = format!(" {k}=\"");
        let rest = &tag[tag.find(&pat)? + pat.len()..];
        rest.get(..rest.find('"')?)?.parse().ok()
    };
    Some((attr("w")?, attr("h")?))
}

/// Hand-rolled because the document is a few lines of XML with no mixed
/// content; a writer library would be more ceremony than the escaping it
/// saves.
///
/// Folders become nested `<stack>` elements (what Krita/CSP write for
/// groups), so any ORA reader sees real groups. A frame folder's derived
/// raster (gutter + borders) is emitted as its stack's **first child layer**
/// tagged `mnc-folder-raster="1"` — foreign readers show the mask, our loader
/// skips it and re-rasterizes from the `mnc-frames` vectors on the stack.
fn stack_xml(
    w: u32,
    h: u32,
    entries: &[LayerEntry],
    comps: &[crate::doc::LayerComp],
    paper: crate::doc::Paper,
) -> String {
    let mut s = String::with_capacity(256 + entries.len() * 160);
    s.push_str("<?xml version='1.0' encoding='UTF-8'?>\n");
    s.push_str(&format!("<image version=\"0.0.3\" w=\"{w}\" h=\"{h}\""));
    // LC-001 comps ride the image element (doc-level, one attr).
    if !comps.is_empty()
        && let Ok(j) = serde_json::to_string(comps)
    {
        s.push_str(&format!(" mnc-comps=\"{}\"", xml_escape(&j)));
    }
    // PA-001: the paper rides the image element too — it belongs to the
    // document, not to any layer. Both attrs are OMITTED at the default
    // (opaque white), so a file whose paper was never touched is written
    // exactly as it was before PA-001 and reads back the same in any other
    // ORA reader.
    if paper.colour != Paper::default().colour {
        let [r, g, b] = paper.colour;
        s.push_str(&format!(" mnc-paper=\"#{r:02x}{g:02x}{b:02x}\""));
    }
    if !paper.visible {
        s.push_str(" mnc-paper-hidden=\"1\"");
    }
    s.push_str(">\n");
    s.push_str(" <stack>\n");

    let mnc_attrs = |e: &LayerEntry| -> String {
        let mut extra = e
            .frames
            .as_deref()
            .map(|j| format!(" mnc-frames=\"{}\"", xml_escape(j)))
            .unwrap_or_default();
        if let Some(j) = e.balloons.as_deref() {
            extra.push_str(&format!(" mnc-balloons=\"{}\"", xml_escape(j)));
        }
        if let Some(j) = e.texts.as_deref() {
            extra.push_str(&format!(" mnc-texts=\"{}\"", xml_escape(j)));
        }
        if let Some(j) = e.tone.as_deref() {
            extra.push_str(&format!(" mnc-tone=\"{}\"", xml_escape(j)));
        }
        if let Some(j) = e.edge.as_deref() {
            extra.push_str(&format!(" mnc-edge=\"{}\"", xml_escape(j)));
        }
        if let Some(j) = e.fill.as_deref() {
            extra.push_str(&format!(" mnc-fill=\"{}\"", xml_escape(j)));
        }
        if let Some(j) = e.genlines.as_deref() {
            extra.push_str(&format!(" mnc-genlines=\"{}\"", xml_escape(j)));
        }
        if let Some([r, g, b]) = e.label {
            extra.push_str(&format!(" mnc-label=\"#{r:02x}{g:02x}{b:02x}\""));
        }
        if e.clip {
            extra.push_str(" mnc-clip=\"1\"");
        }
        if e.lock {
            extra.push_str(" mnc-lock=\"1\"");
        }
        if e.lock_alpha {
            extra.push_str(" mnc-alpha-lock=\"1\"");
        }
        if e.reference {
            extra.push_str(" mnc-reference=\"1\"");
        }
        if e.draft {
            extra.push_str(" mnc-draft=\"1\"");
        }
        if e.through {
            extra.push_str(" mnc-through=\"1\"");
        }
        if let Some(ss) = &e.strokes_src {
            extra.push_str(&format!(" mnc-strokes=\"{}\"", ss));
        }
        if let Some(ms) = &e.mask_src {
            extra.push_str(&format!(" mnc-mask=\"{}\"", ms));
            // H2/M1: where the cropped image sits, and which tiles exist.
            if let Some((ox, oy)) = e.mask_org {
                extra.push_str(&format!(" mnc-mask-org=\"{ox},{oy}\""));
            }
            if let Some(ts) = &e.mask_tiles {
                // Tile INDICES (the parser feeds them to TileIdx::new);
                // the pixel-space origin is the separate mnc-mask-org.
                let list = ts
                    .iter()
                    .map(|i| format!("{},{}", i.x, i.y))
                    .collect::<Vec<_>>()
                    .join(";");
                extra.push_str(&format!(" mnc-mask-tiles=\"{list}\""));
            }
            if e.mask_enabled == Some(false) {
                extra.push_str(" mnc-mask-enabled=\"0\"");
            }
            if e.mask_unlinked == Some(true) {
                extra.push_str(" mnc-mask-unlinked=\"1\"");
            }
        }
        // `#` like `mnc-label`, and readable by a build that predates this
        // fix (that reader REQUIRED the `#`, which is how the bug hid).
        if let Some(c) = &e.layer_colour {
            extra.push_str(&format!(" mnc-lcolour=\"#{c}\""));
        }
        if let Some(c) = &e.layer_sub_colour {
            extra.push_str(&format!(" mnc-lsubcolour=\"#{c}\""));
        }
        if let Some(x) = e.expression.ora_name() {
            extra.push_str(&format!(" mnc-expr=\"{x}\""));
        }
        extra
    };
    let indent = |depth: u8| "  ".repeat(depth as usize + 1);

    // TOP-FIRST: reverse of our bottom-first Vec. A folder header precedes its
    // children in this order, so it opens the <stack> they land in.
    let mut open: u8 = 0;
    for e in entries.iter().rev() {
        while open > e.depth {
            open -= 1;
            s.push_str(&format!("{}</stack>\n", indent(open)));
        }
        let ind = indent(e.depth);
        if e.folder {
            s.push_str(&format!(
                "{}<stack name=\"{}\" opacity=\"{}\" visibility=\"{}\" composite-op=\"{}\" mnc-folder=\"1\"{}{}>\n",
                ind,
                xml_escape(&e.name),
                fmt_opacity(e.opacity),
                if e.visible { "visible" } else { "hidden" },
                e.blend.ora_name(),
                if e.open { String::new() } else { " mnc-open=\"0\"".to_owned() },
                mnc_attrs(e),
            ));
            open = e.depth + 1;
            if e.has_pixels {
                // The header's own raster, as a plain layer foreign readers
                // can composite. Loaders that know us skip it.
                s.push_str(&format!(
                    "{}<layer name=\"{}\" src=\"{}\" x=\"{}\" y=\"{}\" opacity=\"1\" visibility=\"visible\" composite-op=\"svg:src-over\" mnc-folder-raster=\"1\"/>\n",
                    indent(open),
                    xml_escape(&e.name),
                    xml_escape(&e.src),
                    e.x,
                    e.y,
                ));
            }
        } else {
            s.push_str(&format!(
                "{}<layer name=\"{}\" src=\"{}\" x=\"{}\" y=\"{}\" opacity=\"{}\" visibility=\"{}\" composite-op=\"{}\"{}/>\n",
                ind,
                xml_escape(&e.name),
                xml_escape(&e.src),
                e.x,
                e.y,
                fmt_opacity(e.opacity),
                if e.visible { "visible" } else { "hidden" },
                e.blend.ora_name(),
                mnc_attrs(e),
            ));
        }
    }
    while open > 0 {
        open -= 1;
        s.push_str(&format!("{}</stack>\n", indent(open)));
    }
    s.push_str(" </stack>\n</image>\n");
    s
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Short, round-trippable decimal (`1`, `0.5`, `0.333333`) — `{}` on f32 can
/// print `0.30000001192092896`, which is ugly in a file humans open.
fn fmt_opacity(v: f32) -> String {
    let v = v.clamp(0.0, 1.0);
    if v == v.round() {
        format!("{}", v as i32)
    } else {
        let s = format!("{v:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

// ------------------------------------------------------------------- load --

/// Read an `.ora` from disk.
pub fn load(path: &Path) -> Result<Document, OraError> {
    let file = std::fs::File::open(path)?;
    load_from(std::io::BufReader::new(file))
}

/// Read an `.ora` from any seekable source.
pub fn load_from<R: Read + Seek>(source: R) -> Result<Document, OraError> {
    let mut zip = zip::ZipArchive::new(source)?;

    let xml = {
        let mut f = zip
            .by_name("stack.xml")
            .map_err(|_| OraError("no stack.xml (not an OpenRaster file?)".into()))?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        s
    };
    let (w, h, parsed, comps, paper) = parse_stack_xml(&xml)?;

    let mut doc = Document::new(w.max(1), h.max(1));
    doc.paper = paper;
    doc.layers.clear();

    // stack.xml is top-first; push in reverse so layers[0] is the bottom.
    // (A folder header therefore lands *above* its children — our convention.)
    for e in parsed.iter().rev() {
        let mut layer = Layer::new(e.name.clone());
        layer.opacity = e.opacity.clamp(0.0, 1.0);
        layer.visible = e.visible;
        layer.blend = e.blend;
        layer.depth = e.depth;
        layer.folder = e.folder;
        layer.open = e.open;
        layer.clip = e.clip;
        layer.lock = e.lock;
        layer.lock_alpha = e.lock_alpha;
        layer.reference = e.reference;
        layer.draft = e.draft;
        layer.through = layer.folder && e.through;
        layer.tone = e.tone;
        // A folder can carry no border effect (`Document::set_edge` refuses
        // one) — a hand-edited or future file must not sneak one in.
        layer.edge = e.edge.filter(|_| !layer.folder);

        layer.label = e.label;
        layer.layer_colour = e.layer_colour;
        layer.layer_sub_colour = e.layer_sub_colour;
        layer.expression = e.expression;

        if let Some(fs) = &e.frames {
            // Frame layer: the raster is derived, so rebuild it from the
            // vectors instead of decoding the fallback PNG (folders get
            // border + coverage mask, flat layers the gutter raster).
            layer.kind = LayerKind::Frame(fs.clone());
            Document::derive_frame_raster(&mut layer, (w.max(1), h.max(1)));
        } else if let Some(bs) = &e.balloons {
            layer.replace_tiles(bs.rasterize((w.max(1), h.max(1))));
            layer.kind = LayerKind::Balloon(bs.clone());
        } else if let Some(ts) = &e.texts {
            // Text layer: shaping needs DirectWrite, which core cannot call —
            // keep the PNG raster (it *is* the exact saved pixels) and leave
            // sprite caches empty; the app warms them before the first edit.
            if let Some(bytes) = read_entry(&mut zip, &e.src) {
                let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?
                    .to_rgba8();
                paint_into_layer(&mut layer, &img, e.x, e.y);
            }
            layer.kind = LayerKind::Text(ts.clone());
        } else if let Some(k) = &e.fill {
            // Live fill layer (TRIAGE 137): the DERIVED raster re-derives from
            // the params + the persisted mask window; no PNG fallback needed.
            layer.kind = LayerKind::Fill(*k);
        } else if let Some(bytes) = read_entry(&mut zip, &e.src) {
            let img =
                image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?.to_rgba8();
            paint_into_layer(&mut layer, &img, e.x, e.y);
        }
        // SF-004/005: generated effect lines keep their params (the PNG the
        // plain-raster arm decoded above IS their raster; regen is
        // explicit, on edit). Deliberately NOT chained into the arms —
        // chaining made every vector layer also paint its PNG fallback
        // over the re-rasterized tiles (found by the balloon round-trip).
        if let Some(g) = &e.genlines {
            layer.genlines = Some(*g);
        }
        // Vector inking: reattach the stroke record. An unreadable sidecar
        // degrades to an EMPTY-but-present set (the raster is intact; the
        // record is gone) — never a load failure.
        if let Some(ss) = &e.strokes_src {
            let set = read_entry(&mut zip, ss)
                .map(|b| crate::stroke_set::StrokeSet::from_json(&String::from_utf8_lossy(&b)))
                .unwrap_or_default();
            layer.strokes = Some(set);
        }
        // TRIAGE 138 p2: restore the mask (alpha = coverage). Absent attr
        // or unreadable entry = unmasked (old files load unchanged).
        if let Some(ms) = &e.mask_src
            && let Some(bytes) = read_entry(&mut zip, ms)
            && let Ok(img) = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        {
            layer.mask = Some(mask_from_image(
                img.to_rgba8(),
                e.mask_enabled,
                e.mask_org,
                e.mask_tiles.as_deref(),
            ));
        }
        // LM-009: linked is the default; only the explicit unlinked attr
        // opts out (absent = linked, so old files load unchanged).
        if e.mask_unlinked {
            layer.mask_linked = false;
        }
        doc.layers.push(layer);
    }

    if doc.layers.is_empty() {
        doc.layers.push(Layer::new("Layer 1"));
    }
    doc.comps = comps;
    doc.normalize_depths();
    doc.active = doc.layers.len() - 1;
    doc.clear_history();
    doc.touch();
    Ok(doc)
}

/// The mask as a straight RGBA image (coverage in alpha, mirrored in RGB)
/// cropped to its tiles' extent, plus the crop's PIXEL origin — H2: without
/// it the loader cannot place a corner mask anywhere but (0,0). A 1×1
/// transparent image at (0,0) for an empty mask.
fn mask_image(m: &crate::doc::LayerMask) -> (image::RgbaImage, i32, i32) {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for idx in m.tiles.keys() {
        let (ox, oy) = idx.origin();
        x0 = x0.min(ox);
        y0 = y0.min(oy);
        x1 = x1.max(ox + TILE_SIZE as i32);
        y1 = y1.max(oy + TILE_SIZE as i32);
    }
    if x0 >= x1 {
        return (image::RgbaImage::new(1, 1), 0, 0);
    }
    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let mut img = image::RgbaImage::new(w, h);
    for (idx, t) in &m.tiles {
        let (ox, oy) = idx.origin();
        let d = t.data();
        for py in 0..TILE_SIZE {
            for px in 0..TILE_SIZE {
                let c = (d[(py * TILE_SIZE + px) * 4 + 3] as u32 * 255 / 32768) as u8;
                img.put_pixel(
                    (ox - x0) as u32 + px as u32,
                    (oy - y0) as u32 + py as u32,
                    image::Rgba([c, c, c, c]),
                );
            }
        }
    }
    (img, x0, y0)
}

/// Coverage tiles from a mask image (alpha channel → fix15).
///
/// `org` places the (bbox-cropped) image on the canvas; `tiles` is the
/// exact tile set to materialize. Without `tiles` (legacy files, saved
/// before the audit fixes) every tile in the image extent is inserted —
/// holes inside the bbox then come back zero-coverage (hidden), which is
/// the M1 bug; new saves record the set so absent stays absent.
fn mask_from_image(
    img: image::RgbaImage,
    enabled: bool,
    org: Option<(i32, i32)>,
    tiles: Option<&[TileIdx]>,
) -> crate::doc::LayerMask {
    use crate::doc::LayerMask;
    let mut mask = LayerMask {
        enabled,
        revision: crate::tile::next_revision(),
        ..Default::default()
    };
    let (w, h) = (img.width() as i32, img.height() as i32);
    let t = TILE_SIZE as i32;
    let (idxs, (ox, oy)) = match tiles {
        Some(list) => (list.to_vec(), org.unwrap_or((0, 0))),
        None => {
            // F3's compact form: org PRESENT + list absent = the tile set
            // is its own bounding rectangle — the extent, anchored at the
            // org. org ABSENT is the true legacy shape at (0,0).
            let (bx, by) = org.unwrap_or((0, 0));
            let mut v = Vec::new();
            for ty in 0..(h + t - 1) / t {
                for tx in 0..(w + t - 1) / t {
                    v.push(TileIdx::new(tx + bx.div_euclid(t), ty + by.div_euclid(t)));
                }
            }
            (v, (bx, by))
        }
    };
    for idx in idxs {
        let (tx, ty) = idx.origin();
        // A zero-coverage tile must mean HIDDEN (p1's construction
        // invariant — found by the round-trip test); an ABSENT one means
        // unmasked. The recorded tile set is what keeps them apart.
        let mut tile = Tile::new_transparent();
        let d = tile.data_mut();
        for py in 0..TILE_SIZE {
            for px in 0..TILE_SIZE {
                let (x, y) = (tx + px as i32 - ox, ty + py as i32 - oy);
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let a = img.get_pixel(x as u32, y as u32).0[3] as u32;
                let c = (a * 32768 / 255) as u16;
                let o = (py * TILE_SIZE + px) * 4;
                d[o] = c;
                d[o + 1] = c;
                d[o + 2] = c;
                d[o + 3] = c;
            }
        }
        mask.tiles.insert(idx, std::sync::Arc::new(tile));
    }
    mask
}

/// Pull one entry's bytes. Tolerates the `./`-prefixed paths some writers emit.
fn read_entry<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Option<Vec<u8>> {
    let candidates = [name.to_string(), name.trim_start_matches("./").to_string()];
    for c in candidates {
        if let Ok(mut f) = zip.by_name(&c) {
            let mut buf = Vec::new();
            if f.read_to_end(&mut buf).is_ok() {
                return Some(buf);
            }
        }
    }
    None
}

/// Write a decoded layer PNG into sparse tiles at canvas offset `(x, y)`.
///
/// Tiles that come out fully transparent are never allocated — sparse layers are
/// the point.
fn paint_into_layer(layer: &mut Layer, img: &image::RgbaImage, x: i32, y: i32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    if w <= 0 || h <= 0 {
        return;
    }
    let t = TILE_SIZE as i32;
    let tx0 = x.div_euclid(t);
    let ty0 = y.div_euclid(t);
    let tx1 = (x + w - 1).div_euclid(t);
    let ty1 = (y + h - 1).div_euclid(t);

    let mut buf = vec![0u16; TILE_LEN];
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let idx = TileIdx::new(tx, ty);
            let (ox, oy) = idx.origin();
            buf.fill(0);
            let mut any = false;

            for ly in 0..TILE_SIZE {
                let sy = oy + ly as i32 - y;
                if sy < 0 || sy >= h {
                    continue;
                }
                for lx in 0..TILE_SIZE {
                    let sx = ox + lx as i32 - x;
                    if sx < 0 || sx >= w {
                        continue;
                    }
                    let p = straight_u8_to_fix15(img.get_pixel(sx as u32, sy as u32).0);
                    if p == [0, 0, 0, 0] {
                        continue;
                    }
                    any = true;
                    let o = (ly * TILE_SIZE + lx) * 4;
                    buf[o..o + 4].copy_from_slice(&p);
                }
            }

            if any {
                // set_tile (not tile_mut) so loading never lands in an open op.
                let mut tile = Tile::new_transparent();
                tile.data_mut().copy_from_slice(&buf);
                layer.set_tile(idx, Some(Arc::new(tile)));
            }
        }
    }
}

fn parse_hex_rgb(s: &str) -> Option<[u8; 3]> {
    // The `#` is OPTIONAL, and that is a bug fix, not tolerance for its own
    // sake: `mnc-label` writes one and `mnc-lcolour` never did, so requiring
    // it meant every LP-016 layer colour was silently dropped on load — the
    // pixels came back, the colour they display in did not. The writer now
    // emits `#` everywhere; this repairs the files already written without.
    //
    // `palette::parse_hex` is the one that does it, rather than a second
    // hand-rolled parser here: it already strips the `#`, it also takes the
    // 3-digit short form, and it checks the digits are hex. A local
    // `from_str_radix` does not — it would accept `+12345` as a colour.
    crate::palette::parse_hex(s).map(crate::palette::to_u8)
}

struct ParsedLayer {
    name: String,
    src: String,
    x: i32,
    y: i32,
    opacity: f32,
    visible: bool,
    blend: Blend,
    frames: Option<FrameSet>,
    balloons: Option<BalloonSet>,
    texts: Option<TextSet>,
    tone: Option<crate::tone::ToneParams>,
    /// LP-002/LP-003 border effect (`mnc-edge`); absent = no outline, which
    /// is what every file written before this round says.
    edge: Option<crate::edge::EdgeParams>,
    fill: Option<crate::fill_layer::FillKind>,
    genlines: Option<crate::genlines::GenLinesSpec>,
    /// TRIAGE 138 p2: `mnc-mask` zip path + the enabled flag.
    mask_src: Option<String>,
    /// Vector inking: `mnc-strokes` sidecar zip path.
    strokes_src: Option<String>,
    mask_enabled: bool,
    mask_unlinked: bool,
    /// Audit H2/M1: the cropped mask image's pixel origin, and the exact
    /// tile set it carries (None = legacy file → old load semantics).
    mask_org: Option<(i32, i32)>,
    mask_tiles: Option<Vec<TileIdx>>,
    label: Option<[u8; 3]>,
    /// LP-016 layer colour (display tint), hex RRGGBB.
    layer_colour: Option<[u8; 3]>,
    /// LP-017 two-tone SUB colour, hex RRGGBB.
    layer_sub_colour: Option<[u8; 3]>,
    /// LP-022 decrease-colour preview; absent = `Colour`.
    expression: crate::doc::LayerExpression,
    /// Nesting level: 0 = directly in the root stack.
    depth: u8,
    /// This entry is a group `<stack>` (a folder), not a pixel layer.
    folder: bool,
    /// Folder expand state (`mnc-open`), default open.
    open: bool,
    clip: bool,
    lock: bool,
    lock_alpha: bool,
    reference: bool,
    draft: bool,
    through: bool,
}

/// `"x,y"` → `(x, y)` — the `mnc-mask-org` attr form.
fn parse_i32_pair(s: &str) -> Option<(i32, i32)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// `"x,y;x,y"` → tile indices (`mnc-mask-tiles`); unparseable entries drop.
fn parse_tile_list(s: &str) -> Vec<TileIdx> {
    s.split(';')
        .filter_map(parse_i32_pair)
        .map(|(x, y)| TileIdx::new(x, y))
        .collect()
}

/// Parse `stack.xml`. Returns `(w, h, layers-in-document-order)` — i.e. top
/// first, exactly as the file lists them. Nested group stacks come back as
/// folder entries followed by their children at `depth + 1` (Krita/GIMP
/// groups load as folders too). A child tagged `mnc-folder-raster` is our own
/// saved fallback raster of a frame folder — it is skipped, the vectors on
/// the stack element rebuild it.
fn parse_stack_xml(
    xml: &str,
) -> Result<(u32, u32, Vec<ParsedLayer>, Vec<crate::doc::LayerComp>, Paper), OraError> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let (mut w, mut h) = (0u32, 0u32);
    let mut comps: Vec<crate::doc::LayerComp> = Vec::new();
    // PA-001: absent attrs mean the default paper, which is what every file
    // written before PA-001 says.
    let mut paper = Paper::default();
    let mut layers = Vec::new();
    // <stack> nesting level: 0 outside, 1 in the root stack, 2+ in folders.
    let mut stack_level: u8 = 0;

    loop {
        let ev = reader.read_event();
        let (e, self_closing) = match &ev {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => (e, false),
            Ok(Event::Empty(e)) => (e, true),
            Ok(Event::End(e)) => {
                let local = e.local_name();
                if local.as_ref() == b"stack" {
                    stack_level = stack_level.saturating_sub(1);
                }
                continue;
            }
            Ok(_) => continue,
            Err(err) => return Err(OraError(format!("stack.xml: {err}"))),
        };

        let raw = e.name();
        let local = raw.local_name();
        let tag = String::from_utf8_lossy(local.as_ref()).to_string();

        let mut attrs: Vec<(String, String)> = Vec::new();
        for a in e.attributes() {
            let a = a.map_err(|err| OraError(format!("xml attribute: {err}")))?;
            let k = a.key.local_name();
            let key = String::from_utf8_lossy(k.as_ref()).to_string();
            let val = a
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|err| OraError(format!("xml value: {err}")))?
                .to_string();
            attrs.push((key, val));
        }
        let get = |name: &str| -> Option<&str> {
            attrs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };

        match tag.as_str() {
            "image" => {
                // LC-001: doc-level comps on the image element.
                if let Some(j) = get("mnc-comps") {
                    if let Ok(c) = serde_json::from_str::<Vec<crate::doc::LayerComp>>(j) {
                        comps = c;
                    }
                }
                // PA-001: doc-level paper on the image element. An
                // unparseable colour keeps the default rather than guessing.
                if let Some(c) = get("mnc-paper").and_then(parse_hex_rgb) {
                    paper.colour = c;
                }
                paper.visible = get("mnc-paper-hidden").is_none();
                w = get("w").and_then(|v| v.parse().ok()).unwrap_or(0);
                h = get("h").and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            "stack" => {
                if stack_level >= 1 {
                    // A group inside the root stack: a folder.
                    layers.push(ParsedLayer {
                        name: get("name").unwrap_or("Folder").to_string(),
                        src: String::new(),
                        x: 0,
                        y: 0,
                        opacity: get("opacity").and_then(|v| v.parse().ok()).unwrap_or(1.0),
                        visible: get("visibility").map(|v| v != "hidden").unwrap_or(true),
                        blend: get("composite-op")
                            .map(Blend::from_ora_name)
                            .unwrap_or(Blend::Normal),
                        frames: get("mnc-frames").and_then(|j| serde_json::from_str(j).ok()),
                        balloons: None,
                        texts: None,
                        label: get("mnc-label").and_then(parse_hex_rgb),
                        layer_colour: get("mnc-lcolour").and_then(parse_hex_rgb),
                        layer_sub_colour: get("mnc-lsubcolour").and_then(parse_hex_rgb),
                        expression: get("mnc-expr")
                            .map(crate::doc::LayerExpression::from_ora_name)
                            .unwrap_or_default(),
                        depth: stack_level - 1,
                        folder: true,
                        open: get("mnc-open") != Some("0"),
                        clip: false,
                        lock: get("mnc-lock").is_some(),
                        lock_alpha: false,
                        reference: get("mnc-reference").is_some(),
                        draft: get("mnc-draft").is_some(),
                        through: get("mnc-through").is_some(),
                        tone: get("mnc-tone").and_then(|j| serde_json::from_str(j).ok()),
                        edge: get("mnc-edge").and_then(|j| serde_json::from_str(j).ok()),
                        fill: get("mnc-fill").and_then(|j| serde_json::from_str(j).ok()),
                        genlines: get("mnc-genlines").and_then(|j| serde_json::from_str(j).ok()),
                        mask_src: get("mnc-mask").map(str::to_string),
                        strokes_src: get("mnc-strokes").map(str::to_string),
                        mask_enabled: get("mnc-mask-enabled") != Some("0"),
                        mask_unlinked: get("mnc-mask-unlinked").is_some(),
                        mask_org: get("mnc-mask-org").and_then(parse_i32_pair),
                        mask_tiles: get("mnc-mask-tiles").map(parse_tile_list),
                    });
                }
                if !self_closing {
                    stack_level += 1;
                }
            }
            "layer" => {
                if get("mnc-folder-raster").is_some() {
                    // Our own fallback raster of the enclosing frame folder;
                    // the vectors rebuild it.
                    continue;
                }
                layers.push(ParsedLayer {
                    name: get("name").unwrap_or("Layer").to_string(),
                    src: get("src").unwrap_or_default().to_string(),
                    x: get("x").and_then(|v| v.parse().ok()).unwrap_or(0),
                    y: get("y").and_then(|v| v.parse().ok()).unwrap_or(0),
                    opacity: get("opacity").and_then(|v| v.parse().ok()).unwrap_or(1.0),
                    visible: get("visibility").map(|v| v != "hidden").unwrap_or(true),
                    blend: get("composite-op")
                        .map(Blend::from_ora_name)
                        .unwrap_or(Blend::Normal),
                    frames: get("mnc-frames").and_then(|j| serde_json::from_str(j).ok()),
                    balloons: get("mnc-balloons").and_then(|j| serde_json::from_str(j).ok()),
                    texts: get("mnc-texts").and_then(|j| serde_json::from_str(j).ok()),
                    label: get("mnc-label").and_then(parse_hex_rgb),
                    depth: stack_level.saturating_sub(1),
                    layer_colour: get("mnc-lcolour").and_then(parse_hex_rgb),
                    layer_sub_colour: get("mnc-lsubcolour").and_then(parse_hex_rgb),
                    expression: get("mnc-expr")
                        .map(crate::doc::LayerExpression::from_ora_name)
                        .unwrap_or_default(),
                    folder: false,
                    open: true,
                    clip: get("mnc-clip").is_some(),
                    lock: get("mnc-lock").is_some(),
                    lock_alpha: get("mnc-alpha-lock").is_some(),
                    reference: get("mnc-reference").is_some(),
                    draft: get("mnc-draft").is_some(),
                    through: get("mnc-through").is_some(),
                    tone: get("mnc-tone").and_then(|j| serde_json::from_str(j).ok()),
                    edge: get("mnc-edge").and_then(|j| serde_json::from_str(j).ok()),
                    fill: get("mnc-fill").and_then(|j| serde_json::from_str(j).ok()),
                    genlines: get("mnc-genlines").and_then(|j| serde_json::from_str(j).ok()),
                    mask_src: get("mnc-mask").map(str::to_string),
                    strokes_src: get("mnc-strokes").map(str::to_string),
                    mask_enabled: get("mnc-mask-enabled") != Some("0"),
                    mask_unlinked: get("mnc-mask-unlinked").is_some(),
                    mask_org: get("mnc-mask-org").and_then(parse_i32_pair),
                    mask_tiles: get("mnc-mask-tiles").map(parse_tile_list),
                });
            }
            _ => {}
        }
    }

    if w == 0 || h == 0 {
        return Err(OraError("stack.xml has no image size".into()));
    }
    Ok((w, h, layers, comps, paper))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// r108: `ora_canvas_size` reads the true canvas off stack.xml without
    /// touching a raster — the reader's 1:1 mode (and its spread pages)
    /// depends on it being exact, including odd sizes; garbage is a None,
    /// never a guess.
    #[test]
    fn canvas_size_reads_stack_xml_without_decoding() {
        let doc = crate::doc::Document::new(1234, 777);
        let mut buf = std::io::Cursor::new(Vec::new());
        save_to(&doc, &mut buf).unwrap();
        assert_eq!(ora_canvas_size(buf.get_ref()), Some((1234, 777)));
        assert_eq!(ora_canvas_size(b"not a zip"), None);
    }

    /// LC-001: comps persist as one `mnc-comps` attr on the image element
    /// and load back identically (TRIAGE 139's persistence half).
    #[test]
    fn comps_round_trip_through_ora() {
        let mut doc = crate::doc::Document::new(128, 128);
        doc.layers[0].visible = false;
        doc.comps.push(crate::doc::LayerComp {
            name: "no text".into(),
            vis: vec![false, true],
        });
        doc.add_layer("Layer 2");
        let mut buf = std::io::Cursor::new(Vec::new());
        save_to(&doc, &mut buf).unwrap();
        {
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(buf.get_ref().clone())).unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut z.by_name("stack.xml").unwrap(), &mut s).unwrap();
            assert!(s.contains("mnc-comps="), "{s}");
        }
        let reloaded = load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        assert_eq!(reloaded.comps.len(), 1);
        assert_eq!(reloaded.comps[0].name, "no text");
        assert_eq!(reloaded.comps[0].vis, vec![false, true]);
    }

    /// TRIAGE 138 p2: a masked layer round-trips through ORA — the mask
    /// still hides the same half, the enabled flag survives, and an
    /// unmasked save stays old-shape (no mnc-mask attr). Audit H2 re-point:
    /// the mask lives at tiles (3,2)/(3,3) — the old loader dropped the
    /// crop origin and reloaded it at (0,0), hiding the wrong region; the
    /// origin-anchored (0,0) shape this test used before could not see it.
    #[test]
    fn mask_round_trips_through_ora() {
        let mut doc = crate::doc::Document::new(256, 256);
        doc.begin_op();
        for idx in [TileIdx::new(3, 2), TileIdx::new(3, 3)] {
            let t = doc.layers[0].tile_mut(idx);
            for p in 0..crate::tile::TILE_PIXELS {
                t.set_pixel(p % 64, p / 64, [32768, 0, 0, 32768]);
            }
        }
        doc.end_op();
        doc.selection = Some(crate::selection::Selection::from_rect(
            &doc, 192.0, 128.0, 256.0, 192.0,
        ));
        assert!(doc.mask_outside_selection(0));

        let mut buf = std::io::Cursor::new(Vec::new());
        save_to(&doc, &mut buf).unwrap();
        // The origin rides as an attr; the tile LIST rides only when the
        // set is NOT its own bounding rectangle (F3 — the solid 1x2
        // fixture omits it; the loader's org-only path is exactly the
        // solid bbox).
        {
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(buf.get_ref().clone())).unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut z.by_name("stack.xml").unwrap(), &mut s).unwrap();
            assert!(s.contains("mnc-mask-org=\"192,128\""), "crop origin saved");
            assert!(!s.contains("mnc-mask-tiles"), "solid bbox omits the list");
        }
        let reloaded = load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        let m = reloaded.layers[0].mask.as_ref().expect("mask survived");
        assert!(m.enabled);
        assert!(m.tiles.contains_key(&TileIdx::new(3, 2)));
        assert!(m.tiles.contains_key(&TileIdx::new(3, 3)));
        assert_eq!(
            m.tiles.len(),
            2,
            "no tiles materialized at the canvas origin"
        );
        let img = crate::export::composite(&reloaded, crate::export::Background::Transparent);
        assert_eq!(img.get_pixel(197, 133).0[3], 255, "inside: kept");
        assert_eq!(img.get_pixel(197, 197).0[3], 0, "outside: still hidden");

        // The disabled flag survives too.
        let mut doc2 = doc.clone();
        doc2.mask_set_enabled(0, false);
        let mut b2 = std::io::Cursor::new(Vec::new());
        save_to(&doc2, &mut b2).unwrap();
        let r2 = load_from(std::io::Cursor::new(b2.into_inner())).unwrap();
        assert!(!r2.layers[0].mask.as_ref().unwrap().enabled);

        // An unmasked save carries no mnc-mask (old files stay old-shape).
        let mut plain = crate::doc::Document::new(64, 64);
        plain.begin_op();
        plain.layers[0]
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(1, 1, [1, 2, 3, 32768]);
        plain.end_op();
        let mut b3 = std::io::Cursor::new(Vec::new());
        save_to(&plain, &mut b3).unwrap();
        let xml = {
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(b3.into_inner())).unwrap();
            let mut f = z.by_name("stack.xml").unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut f, &mut s).unwrap();
            s
        };
        assert!(!xml.contains("mnc-mask"), "unmasked files stay old-shape");
    }

    /// Audit M1: a tile ABSENT from the mask but inside the saved bbox (a
    /// hole) must come back ABSENT — absent means unmasked/visible, and
    /// layer ink showing through a hole is exactly the "painted after the
    /// mask was made" case. The old loader materialized every bbox tile,
    /// turning holes into zero-coverage (hidden). The hole is hand-built
    /// here (command-built masks tile-match the layer); draw-on-mask
    /// creates the shape in the wild.
    #[test]
    fn mask_holes_inside_the_bbox_stay_visible() {
        let mut doc = crate::doc::Document::new(256, 64);
        // Ink only on the middle tile — visible solely through the hole.
        doc.begin_op();
        let t = doc.layers[0].tile_mut(TileIdx::new(1, 0));
        for p in 0..crate::tile::TILE_PIXELS {
            t.set_pixel(p % 64, p / 64, [32768, 0, 0, 32768]);
        }
        doc.end_op();
        // A mask with a hole: full coverage at (0,0) and (2,0), NOTHING at
        // (1,0) — the saved image spans all three, its middle is zeros.
        let full = || {
            let mut tile = Tile::new_transparent();
            tile.data_mut().fill(32768);
            std::sync::Arc::new(tile)
        };
        doc.layers[0].mask = Some(crate::doc::LayerMask {
            tiles: std::collections::HashMap::from([
                (TileIdx::new(0, 0), full()),
                (TileIdx::new(2, 0), full()),
            ]),
            enabled: true,
            revision: crate::tile::next_revision(),
        });
        let img = crate::export::composite(&doc, crate::export::Background::Transparent);
        assert_eq!(
            img.get_pixel(80, 5).0[3],
            255,
            "live: ink shows through the hole"
        );

        let mut buf = std::io::Cursor::new(Vec::new());
        save_to(&doc, &mut buf).unwrap();
        let reloaded = load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        let m = reloaded.layers[0].mask.as_ref().expect("mask survived");
        assert!(
            !m.tiles.contains_key(&TileIdx::new(1, 0)),
            "hole stays absent"
        );
        let img = crate::export::composite(&reloaded, crate::export::Background::Transparent);
        assert_eq!(
            img.get_pixel(80, 5).0[3],
            255,
            "reloaded: the hole still reveals its ink"
        );
    }

    /// Owner preview tier (2026-08-18): `mnc/preview.png` round trips
    /// byte-exactly through save_with/page_preview, and plain `save_to`
    /// writes NO entry — old files stay old-shape and foreign ORA readers
    /// never see ours.
    #[test]
    fn preview_entry_round_trips_and_stays_opt_in() {
        let mut doc = crate::doc::Document::new(64, 64);
        doc.begin_op();
        doc.layers[0]
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(1, 1, [1, 2, 3, 32768]);
        doc.end_op();
        let mut prev = image::GrayImage::new(2, 1);
        prev.put_pixel(0, 0, image::Luma([7]));
        prev.put_pixel(1, 0, image::Luma([250]));
        let mut png = Vec::new();
        image::DynamicImage::ImageLuma8(prev)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let mut buf = Cursor::new(Vec::new());
        save_to_with(&doc, &mut buf, Some(&png)).unwrap();
        let back = crate::project::page_preview(&buf.into_inner()).expect("preview entry extracts");
        assert_eq!(back.dimensions(), (2, 1));
        assert_eq!(back.get_pixel(0, 0)[0], 7);
        assert_eq!(back.get_pixel(1, 0)[0], 250);

        let mut plain = Cursor::new(Vec::new());
        save_to(&doc, &mut plain).unwrap();
        assert!(
            crate::project::page_preview(&plain.into_inner()).is_none(),
            "plain saves carry no preview (old shape)"
        );
    }

    use crate::blend::f32_to_fix15;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mn-ora-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("temp dir");
        d
    }

    /// Audit H2: `save` must replace the target atomically and leave no `.tmp`
    /// debris beside it.
    #[test]
    fn save_replaces_atomically_and_leaves_no_debris() {
        let dir = temp_dir("atomic");
        let path = dir.join("work.ora");
        std::fs::write(&path, b"PREVIOUS VERSION").expect("seed");

        save(&eight_bit_doc(), &path).expect("save");

        assert!(
            load(&path).is_ok(),
            "the replaced file is not a loadable ORA"
        );
        assert!(
            !tmp_sibling(&path).exists(),
            "left a .tmp beside the user's file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Audit H2, the half that mattered most: when a save FAILS, whatever was
    /// already at `path` must be untouched. The old `File::create(path)` shape
    /// truncated it before writing a byte, so a mid-save failure destroyed the
    /// previous version.
    ///
    /// The failure is forced by pointing `save` at an existing directory: the
    /// `.tmp` builds fine, the rename onto a non-empty directory cannot
    /// succeed.
    #[test]
    fn a_failed_save_does_not_destroy_the_previous_file() {
        let dir = temp_dir("keep");
        let target = dir.join("occupied.ora");
        std::fs::create_dir_all(&target).expect("dir in the way");
        let canary = target.join("precious.txt");
        std::fs::write(&canary, b"do not lose me").expect("seed");

        let err = save(&eight_bit_doc(), &target);

        assert!(
            err.is_err(),
            "renaming onto a non-empty directory must fail"
        );
        assert_eq!(
            std::fs::read(&canary).expect("the previous contents survive"),
            b"do not lose me",
            "a failed save destroyed data that was already there"
        );
        assert!(
            !tmp_sibling(&target).exists(),
            "a failed save left .tmp debris behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn roundtrip(doc: &Document) -> Document {
        let mut buf = Vec::new();
        save_to(doc, Cursor::new(&mut buf)).expect("save");
        load_from(Cursor::new(buf)).expect("load")
    }

    fn to_bytes(doc: &Document) -> Vec<u8> {
        let mut buf = Vec::new();
        save_to(doc, Cursor::new(&mut buf)).expect("save");
        buf
    }

    /// A document whose pixels all originate from 8-bit straight values, which
    /// is the case the format can carry exactly.
    fn eight_bit_doc() -> Document {
        let mut doc = Document::new(192, 128);
        doc.rename_layer(0, "Paper");
        doc.add_layer("Ink");
        doc.set_layer_opacity(1, 0.5);
        doc.set_layer_blend(1, Blend::Multiply);

        for (li, base) in [(0usize, 0u8), (1, 128)] {
            for tx in 0..2 {
                let tile = doc.layers[li].tile_mut(TileIdx::new(tx, 0));
                for y in 0..TILE_SIZE {
                    for x in 0..TILE_SIZE {
                        // Alphas 2..=255 only (see the module docs on alpha 1).
                        let a = (2 + ((x + y * 3 + base as usize) % 254)) as u8;
                        let px = [
                            (x * 4 % 256) as u8,
                            (y * 4 % 256) as u8,
                            base.wrapping_add((x + y) as u8),
                            a,
                        ];
                        tile.set_pixel(x, y, straight_u8_to_fix15(px));
                    }
                }
            }
        }
        doc.set_layer_visible(0, false);
        doc
    }

    #[test]
    fn mimetype_is_the_first_entry_and_stored() {
        let doc = Document::new(64, 64);
        let bytes = to_bytes(&doc);
        // Local file header of entry 0, right at offset 0.
        assert_eq!(&bytes[0..4], b"PK\x03\x04");
        // compression method (u16 @ 8) == 0 (stored)
        assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), 0);
        let name_len = u16::from_le_bytes([bytes[26], bytes[27]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[28], bytes[29]]) as usize;
        assert_eq!(&bytes[30..30 + name_len], b"mimetype");
        let data = 30 + name_len + extra_len;
        assert_eq!(&bytes[data..data + 16], b"image/openraster");
    }

    #[test]
    fn archive_contains_every_required_entry() {
        let doc = eight_bit_doc();
        let bytes = to_bytes(&doc);
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        for want in [
            "mimetype",
            "stack.xml",
            "data/layer0.png",
            "data/layer1.png",
            "mergedimage.png",
            "Thumbnails/thumbnail.png",
        ] {
            assert!(
                names.contains(&want.to_string()),
                "missing {want} in {names:?}"
            );
        }
    }

    #[test]
    fn structure_roundtrips_exactly() {
        let doc = eight_bit_doc();
        let back = roundtrip(&doc);

        assert_eq!(back.size, doc.size);
        assert_eq!(back.layers.len(), doc.layers.len());
        for (a, b) in doc.layers.iter().zip(back.layers.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.visible, b.visible);
            assert_eq!(a.blend, b.blend);
            assert!(
                (a.opacity - b.opacity).abs() < 1e-4,
                "{} vs {}",
                a.opacity,
                b.opacity
            );
            assert_eq!(a.tile_count(), b.tile_count(), "layer {}", a.name);
        }
        assert!(!back.can_undo(), "a freshly loaded document has no history");
    }

    #[test]
    fn reference_and_draft_flags_roundtrip() {
        let mut doc = Document::new(96, 96);
        doc.add_layer("Ink");
        assert!(doc.set_layer_reference(1, true));
        assert!(doc.set_layer_draft(0, true));
        // A draft folder cascades at composite time; the attr rides the stack.
        let fi = doc.add_folder_above(1, "Draft folder");
        assert!(doc.set_layer_draft(fi, true));

        let back = roundtrip(&doc);
        assert!(
            back.layers.iter().any(|l| l.reference),
            "reference survived"
        );
        assert_eq!(
            back.reference_layer_index()
                .map(|i| back.layers[i].name.as_str()),
            Some("Ink")
        );
        assert_eq!(
            back.effective_drafts().iter().filter(|d| **d).count(),
            doc.effective_drafts().iter().filter(|d| **d).count(),
            "draft flags (folder included) survived"
        );
    }

    #[test]
    fn eight_bit_pixels_roundtrip_bit_exactly() {
        let doc = eight_bit_doc();
        let back = roundtrip(&doc);
        for (li, layer) in doc.layers.iter().enumerate() {
            for (idx, tile) in layer.tiles() {
                let other = back.layers[li]
                    .tile(idx)
                    .unwrap_or_else(|| panic!("layer {li} lost tile {idx:?}"));
                assert_eq!(
                    tile.data(),
                    other.data(),
                    "layer {li} tile {idx:?} changed across save/load"
                );
            }
        }
    }

    #[test]
    fn arbitrary_fix15_data_is_stable_after_one_roundtrip() {
        // Not 8-bit-derived: a fix15 ramp, including the lossy bottom end.
        let mut doc = Document::new(64, 64);
        {
            let tile = doc.layers[0].tile_mut(TileIdx::new(0, 0));
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let a = f32_to_fix15((x * TILE_SIZE + y) as f32 / TILE_PIXELS_F);
                    let c = a / 3;
                    tile.set_pixel(x, y, [c, a / 2, a, a]);
                }
            }
        }
        let once = roundtrip(&doc);
        let twice = roundtrip(&once);
        for (idx, tile) in once.layers[0].tiles() {
            assert_eq!(
                tile.data(),
                twice.layers[0].tile(idx).unwrap().data(),
                "second round-trip must be a fixed point"
            );
        }
    }

    #[test]
    fn offsets_survive_and_empty_layers_are_legal() {
        let mut doc = Document::new(512, 512);
        doc.layers[0].tile_mut(TileIdx::new(4, 5)).set_pixel(
            3,
            3,
            straight_u8_to_fix15([1, 2, 3, 255]),
        );
        doc.add_layer("blank");
        let back = roundtrip(&doc);
        assert_eq!(back.layers.len(), 2);
        assert_eq!(back.layers[0].tile_count(), 1);
        let t = back.layers[0]
            .tile(TileIdx::new(4, 5))
            .expect("offset tile");
        assert_eq!(t.pixel(3, 3), straight_u8_to_fix15([1, 2, 3, 255]));
        assert!(back.layers[1].is_empty());
    }

    #[test]
    fn foreign_stack_xml_shapes_parse() {
        // Namespaced, nested group stack, unknown composite-op, self-closing and
        // paired <layer> forms — roughly what Krita/GIMP emit.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<image version="0.0.3" w="100" h="50" xmlns="http://www.freedesktop.org/standards/openraster">
  <stack opacity="1" name="root">
    <stack name="group" opacity="0.5" visibility="visible">
      <layer name="in group" src="data/a.png" x="10" y="-20" opacity="0.25" composite-op="svg:screen"/>
    </stack>
    <layer name="odd &amp; quoted" src="data/b.png" composite-op="svg:plus-lighter" visibility="hidden"></layer>
  </stack>
</image>"#;
        let (w, h, layers, _comps, paper) = parse_stack_xml(xml).unwrap();
        assert_eq!((w, h), (100, 50));
        assert_eq!(
            paper,
            Paper::default(),
            "PA-001: a stack.xml with no paper attrs is the default paper"
        );
        assert_eq!(layers.len(), 3, "the group loads as a folder + its child");
        assert!(layers[0].folder);
        assert_eq!(layers[0].name, "group");
        assert!((layers[0].opacity - 0.5).abs() < 1e-6, "group opacity kept");
        assert_eq!(layers[0].depth, 0);
        assert_eq!(layers[1].name, "in group");
        assert_eq!(layers[1].depth, 1, "child sits inside the folder");
        assert_eq!((layers[1].x, layers[1].y), (10, -20));
        assert_eq!(layers[1].blend, Blend::Screen);
        assert!((layers[1].opacity - 0.25).abs() < 1e-6);
        assert_eq!(layers[2].name, "odd & quoted");
        // `svg:plus-lighter` is a real CSS/SVG operator we do NOT implement —
        // the point of this row. It used to be `svg:color-dodge`, which
        // stopped being an unknown op the round colour dodge shipped; pick a
        // replacement from OUTSIDE `Blend::ALL` if this ever needs changing
        // again, or the assertion quietly stops testing anything.
        assert_eq!(layers[2].blend, Blend::Normal, "unknown op falls back");
        assert_eq!(layers[2].depth, 0);
        assert!(!layers[2].visible);
        assert!(
            (layers[2].opacity - 1.0).abs() < 1e-6,
            "missing opacity defaults to 1"
        );

        assert!(parse_stack_xml("<image/>").is_err(), "size is mandatory");
    }

    #[test]
    fn folders_roundtrip_as_nested_stacks() {
        // A frame folder (header + White + draw layer) over a base layer.
        let mut doc = Document::new(256, 256);
        doc.rename_layer(0, "Base");
        let fs = FrameSet::single_rect([32.0, 32.0, 224.0, 224.0], 4.0);
        let hi = doc.add_frame_folder("Frame 1", fs.clone());
        doc.set_layer_opacity(hi, 0.75);
        doc.set_folder_open(hi, false);

        // The XML really nests, and carries the fallback raster child.
        let bytes = to_bytes(&doc);
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("stack.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(
            xml.contains("mnc-folder=\"1\""),
            "folder saved as a stack:\n{xml}"
        );
        assert!(
            xml.contains("mnc-folder-raster=\"1\""),
            "foreign-reader fallback:\n{xml}"
        );
        assert!(xml.contains("mnc-open=\"0\""), "collapse state saved");
        let stack_open = xml.match_indices("<stack").count();
        assert_eq!(stack_open, 2, "root + one folder stack:\n{xml}");

        let back = roundtrip(&doc);
        assert_eq!(back.layers.len(), 4, "fallback raster child was skipped");
        let names: Vec<&str> = back.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Base", "White", "Layer 1", "Frame 1"]);
        let header = &back.layers[3];
        assert!(header.folder && !header.open);
        assert!((header.opacity - 0.75).abs() < 1e-4);
        assert_eq!(
            header.frames().unwrap(),
            &fs,
            "vectors survive on the stack"
        );
        assert!(header.tile_count() > 0, "mask raster rebuilt from vectors");
        assert_eq!(back.layers[1].depth, 1);
        assert_eq!(back.layers[2].depth, 1);
        assert_eq!(back.children_range(3), 1..3);
        // The White child kept its pixels.
        assert_eq!(
            back.layers[1].tile(TileIdx::new(0, 0)).unwrap().pixel(3, 3),
            [crate::tile::FIX15_ONE as u16; 4]
        );
    }

    /// The trap that ate fourteen million alpha units of a test's ink (r126,
    /// first draft of the work-folder round-trip): a decode leaves the frame
    /// folder's HEADER active, and a header's raster is DERIVED from the
    /// vectors — the arm above re-makes it — so a stroke put there measures
    /// fine in memory and is gone by the next load. It reads like the encoder
    /// ate it. Nothing did; there was never anything to write.
    ///
    /// A drawing user does not reach it: the app refuses the press before the
    /// brush starts, in `App::guard_frame_layer` ("this is the frame folder
    /// itself — pick a layer inside it to draw"), and every other ink path —
    /// fill, gradient, figure, paste, cut, clear, transform — asks
    /// `paintable()` first. (One gap, named in the commit: a Figure polygon is
    /// guarded at its FIRST vertex only.) Otherwise it takes code holding a
    /// `&mut Layer` to lose ink here, which means tests. This is the test that
    /// says so.
    ///
    /// The two REAL layers in the same folder carry their pixels through the
    /// same round trip, and a tone layer's ink does too (its PNG is the
    /// painted SOURCE, see `tone_params_roundtrip_and_png_stays_the_source`) —
    /// the loss belongs to derived rasters alone.
    #[test]
    fn a_decoded_frame_folder_page_activates_the_derived_header() {
        // A New Comic page, exactly as `App::blank_page_doc_sized` seeds it.
        let mut doc = Document::new(256, 256);
        doc.add_frame_folder(
            "Frame 1",
            FrameSet::single_rect([32.0, 32.0, 224.0, 224.0], 4.0),
        );
        assert_eq!(doc.active, 2, "fresh: the draw layer inside the folder");
        assert!(doc.active_layer().paintable());

        let mut back = roundtrip(&doc);
        assert_eq!(
            back.active,
            back.layers.len() - 1,
            "decoded: the topmost layer, which is the folder's header"
        );
        let (white, draw, header) = (1usize, 2usize, back.active);
        assert!(back.layers[header].folder && back.layers[header].is_frame());
        assert!(
            !back.layers[header].paintable(),
            "the active layer after a decode takes no ink"
        );

        // Paint the same mark on the header and on both of its children, in
        // a tile the panel interior leaves blank.
        let mark = straight_u8_to_fix15([255, 0, 0, 255]);
        let spot = TileIdx::new(1, 1);
        for li in [header, draw, white] {
            back.layers[li].tile_mut(spot).set_pixel(5, 5, mark);
            assert_eq!(
                back.layers[li].tile(spot).unwrap().pixel(5, 5),
                mark,
                "all three measure identical in memory — the shape of the mistake"
            );
        }

        let back2 = roundtrip(&back);
        for li in [draw, white] {
            assert_eq!(
                back2.layers[li].tile(spot).unwrap().pixel(5, 5),
                mark,
                "a real raster layer keeps its ink, folder or no folder"
            );
        }
        let alpha: u64 = back2.layers[header]
            .tile(spot)
            .map_or(0, |t| t.data().chunks(4).map(|p| p[3] as u64).sum());
        assert_eq!(
            alpha, 0,
            "the header's raster came back from the vectors, mark and all"
        );
    }

    #[test]
    fn top_first_xml_order() {
        let mut doc = Document::new(64, 64);
        doc.rename_layer(0, "bottom");
        doc.add_layer("top");
        let bytes = to_bytes(&doc);
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("stack.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        let top = xml.find("top").unwrap();
        let bottom = xml.find("bottom").unwrap();
        assert!(
            top < bottom,
            "stack.xml must list the top layer first:\n{xml}"
        );

        let back = load_from(Cursor::new(to_bytes(&doc))).unwrap();
        assert_eq!(back.layers[0].name, "bottom");
        assert_eq!(back.layers[1].name, "top");
    }

    #[test]
    fn thumbnail_is_bounded() {
        let doc = Document::new(1024, 512);
        let bytes = to_bytes(&doc);
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut png = Vec::new();
        zip.by_name("Thumbnails/thumbnail.png")
            .unwrap()
            .read_to_end(&mut png)
            .unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert!(img.width() <= THUMB_MAX && img.height() <= THUMB_MAX);
        assert_eq!((img.width(), img.height()), (256, 128), "aspect kept");
    }

    #[test]
    fn text_layers_roundtrip_vectors_and_keep_the_png_raster() {
        use crate::text::{RenderedText, StyleFlag, TextItem, TextSet};
        let mut doc = Document::new(256, 256);
        let mut item = TextItem::new([40.0, 40.0], "Meiryo".into(), 12.0, [0, 0, 0], true);
        item.insert(0, "テスト");
        item.set_style(0, 2, StyleFlag::Bold, true);
        item.size = [80.0, 120.0];
        item.rotation = 0.3;
        item.outline_px = 3.0;
        // A synthetic sprite stands in for the DirectWrite raster.
        item.cache = Some(Arc::new(RenderedText {
            origin: [40, 40],
            size: [16, 16],
            rgba: (0..16 * 16).flat_map(|_| [0, 0, 0, 255]).collect(),
        }));
        let ts = TextSet { texts: vec![item] };
        doc.add_text_layer("Text 1", ts.clone());

        let back = roundtrip(&doc);
        let tl = &back.layers[1];
        assert_eq!(tl.name, "Text 1");
        let got = tl.texts().expect("kind survives");
        assert_eq!(got, &ts, "model state is exact across save/load");
        assert!(got.texts[0].cache.is_none(), "sprites are not serialized");
        assert!(got.texts[0].runs[0].bold, "style runs survive");
        // The PNG fallback carries the exact pixels the sprite produced.
        for (idx, tile) in doc.layers[1].tiles() {
            assert_eq!(
                tile.data(),
                back.layers[1].tile(idx).expect("tile").data(),
                "loaded PNG raster differs at {idx:?}"
            );
        }
    }

    #[test]
    fn frame_layers_roundtrip_as_vectors() {
        let mut doc = Document::new(256, 256);
        let fs = FrameSet::single_rect([64.0, 64.0, 192.0, 192.0], 6.0);
        doc.add_frame_layer("Frame 1", fs.clone());

        let back = roundtrip(&doc);
        let frame_layer = &back.layers[1];
        assert_eq!(frame_layer.name, "Frame 1");
        let got = frame_layer.frames().expect("kind survives");
        assert_eq!(got, &fs, "vector state is exact across save/load");
        assert!(frame_layer.tile_count() > 0, "raster was rebuilt on load");
        // And the derived raster matches what the original document had.
        for (idx, tile) in doc.layers[1].tiles() {
            assert_eq!(
                tile.data(),
                back.layers[1].tile(idx).expect("tile").data(),
                "re-rasterized tile {idx:?} differs"
            );
        }
    }

    #[test]
    fn balloon_layers_roundtrip_as_vectors() {
        use crate::balloon::{Balloon, BalloonShape, Tail};
        let mut doc = Document::new(256, 256);
        let bs = BalloonSet {
            pressure_width: false,
            balloons: vec![Balloon {
                shape: BalloonShape::Ellipse {
                    center: [128.0, 100.0],
                    radii: [60.0, 40.0],
                },
                tails: vec![Tail {
                    base: [128.0, 120.0],
                    tip: [128.0, 200.0],
                    width: 24.0,
                    ..Default::default()
                }],

                ..Default::default()
            }],
            border_px: 4.0,
        };
        doc.add_balloon_layer("Balloon 1", bs.clone());

        let back = roundtrip(&doc);
        let bl = &back.layers[1];
        assert_eq!(bl.name, "Balloon 1");
        let got = bl.balloons().expect("kind survives");
        assert_eq!(got, &bs, "vector state is exact across save/load");
        assert!(bl.tile_count() > 0, "raster was rebuilt on load");
        for (idx, tile) in doc.layers[1].tiles() {
            assert_eq!(
                tile.data(),
                back.layers[1].tile(idx).expect("tile").data(),
                "re-rasterized tile {idx:?} differs"
            );
        }
    }

    #[test]
    fn junk_input_is_an_error_not_a_panic() {
        assert!(load_from(Cursor::new(b"not a zip".to_vec())).is_err());
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
            zw.start_file("hello.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(b"hi").unwrap();
            zw.finish().unwrap();
        }
        assert!(
            load_from(Cursor::new(buf)).is_err(),
            "zip without stack.xml"
        );
    }

    #[test]
    fn tone_params_roundtrip_and_png_stays_the_source() {
        use crate::tone::{ToneDensity, ToneParams, TonePattern};
        let mut doc = Document::new(128, 128);
        {
            // A PATCH, not a lone pixel: the derived raster only inks where
            // the source does, and one pixel may legitimately fall between
            // two dots of the screen (which the lattice offset below moves).
            let t = doc.active_layer_mut().tile_mut(TileIdx::new(0, 0));
            for y in 0..24 {
                for x in 0..24 {
                    t.set_pixel(x, y, straight_u8_to_fix15([0, 0, 0, 128]));
                }
            }
        }
        let src = doc.active_layer().tile(TileIdx::new(0, 0)).unwrap().clone();

        // Every field, including the ones added with the shapes/density/
        // posterization/offset round — `mnc-tone` is one serde blob, so a
        // field that does not survive here does not survive a save.
        let p = ToneParams {
            pattern: TonePattern::Lines,
            lpi: 42.5,
            angle_deg: 30.0,
            offset: [3.5, -2.0],
            posterize: Some(6),
            density: ToneDensity::Specified(0.4),
        };
        assert!(doc.set_tone(0, Some(p)));
        let mut back = roundtrip(&doc);

        let got = back.layers[0].tone.expect("tone attr survived");
        assert_eq!(got, p);
        // The layer PNG carries the SOURCE ink, not the derived raster — our
        // loader re-derives; foreign readers see the editable marks.
        assert_eq!(
            back.layers[0].tile(TileIdx::new(0, 0)).unwrap().data(),
            src.data(),
            "source pixels must ride the file untouched"
        );
        back.refresh_derived(600);
        assert!(
            !back.layers[0]
                .display_tile(TileIdx::new(0, 0))
                .unwrap()
                .is_blank()
        );
    }

    /// LP-002/003/017/022 ride the file as attributes and the derived
    /// outline rebuilds on load — the same "params, not pixels" contract the
    /// tone uses. The negative half matters as much: a document with none of
    /// them set must write NO new attributes, so a file saved after this
    /// round is byte-shaped exactly like one saved before it.
    #[test]
    fn layer_effects_round_trip_through_ora() {
        use crate::doc::LayerExpression;
        use crate::edge::EdgeParams;
        let mut doc = crate::doc::Document::new(256, 256);
        doc.begin_op();
        doc.layers[0]
            .tile_mut(TileIdx::new(1, 1))
            .set_pixel(10, 10, [0, 0, 0, 32768]);
        doc.end_op();
        let p = EdgeParams {
            width_px: 5.0,
            colour: [0, 128, 255],
        };
        assert!(doc.set_edge(0, Some(p)));
        assert!(doc.set_layer_sub_colour(0, Some([0xf2, 0xb8, 0x1c])));
        assert!(doc.set_layer_colour(0, Some([0x2a, 0x6f, 0xf4])));
        assert!(doc.set_layer_expression(0, LayerExpression::Mono));

        let mut back = roundtrip(&doc);
        assert_eq!(back.layers[0].edge, Some(p));
        // LP-016's own colour is in here because it did NOT survive: the
        // writer omitted the `#` that `parse_hex_rgb` demanded, so every
        // layer colour was silently dropped on load. Found by this test.
        assert_eq!(back.layers[0].layer_colour, Some([0x2a, 0x6f, 0xf4]));
        assert_eq!(back.layers[0].layer_sub_colour, Some([0xf2, 0xb8, 0x1c]));
        assert_eq!(back.layers[0].expression, LayerExpression::Mono);
        // The outline is DERIVED: nothing of it is stored, and one refresh
        // rebuilds it from the source ink the PNG carried.
        back.refresh_derived(600);
        assert_eq!(
            back.layers[0]
                .display_tile(TileIdx::new(1, 1))
                .expect("outline rebuilt on load")
                .pixel(13, 10)[3],
            32768
        );

        // A plain document writes none of the three attributes.
        let plain = crate::doc::Document::new(64, 64);
        let mut buf = std::io::Cursor::new(Vec::new());
        save_to(&plain, &mut buf).unwrap();
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(buf.into_inner())).unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut z.by_name("stack.xml").unwrap(), &mut s).unwrap();
        for attr in ["mnc-edge", "mnc-lsubcolour", "mnc-expr"] {
            assert!(!s.contains(attr), "{attr} written for a stock layer: {s}");
        }
    }

    /// PA-001: the paper is document state, so it rides the ORA round trip —
    /// and it rides it *cheaply*: a document whose paper was never touched
    /// writes neither attr, so files from before PA-001 are byte-identical
    /// and load as the default white.
    #[test]
    fn paper_round_trips_and_the_default_costs_no_attrs() {
        // A real layer with real pixels, so this exercises the file a page
        // actually is rather than an empty stack.
        let fixture = || {
            let mut d = Document::new(64, 64);
            d.begin_op();
            d.layers[0]
                .tile_mut(TileIdx::new(0, 0))
                .set_pixel(4, 4, [0, 0, 0, 32768]);
            d.end_op();
            d
        };

        let doc = fixture();
        let mut buf = std::io::Cursor::new(Vec::new());
        save_to(&doc, &mut buf).unwrap();
        {
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(buf.get_ref().clone())).unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut z.by_name("stack.xml").unwrap(), &mut s).unwrap();
            assert!(!s.contains("mnc-paper"), "the default paper writes nothing");
        }
        let back = load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        assert_eq!(back.paper, Paper::default());

        let mut doc = fixture();
        doc.set_paper_colour([250, 243, 224]);
        doc.set_paper_visible(false);
        let mut buf = std::io::Cursor::new(Vec::new());
        save_to(&doc, &mut buf).unwrap();
        {
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(buf.get_ref().clone())).unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut z.by_name("stack.xml").unwrap(), &mut s).unwrap();
            assert!(s.contains("mnc-paper=\"#faf3e0\""), "colour on the image el");
            assert!(s.contains("mnc-paper-hidden=\"1\""), "and the eye");
        }
        let back = load_from(std::io::Cursor::new(buf.into_inner())).unwrap();
        assert_eq!(back.paper.colour, [250, 243, 224]);
        assert!(!back.paper.visible, "the eye survives the round trip");
    }

    const TILE_PIXELS_F: f32 = (TILE_SIZE * TILE_SIZE) as f32;
}
