//! GIMP `.gbr` / `.gih` brush reader — coverage masks only.
//!
//! Same goal as `abr.rs`: get the tip bitmaps OUT as grayscale coverage masks
//! for the texture-tip system (`mybrush::TextureMask`). GIMP's own brush
//! parameters beyond spacing (the `.gih` pipe's dimension/rank/placement
//! machinery) are out of scope — an imported tip becomes a preset the owner
//! retunes, exactly like the `.abr` path.
//!
//! Layout reference: GIMP's `devel-docs/gbr.txt` and `devel-docs/gih.txt`
//! (the format is small and fully specified there; no reverse engineering
//! needed, unlike `.abr`). All integers big-endian.
//!
//! - **`.gbr` v2/v3** — `u32 header_size, u32 version, u32 width,
//!   u32 height, u32 bytes_per_pixel, u32 magic ("GIMP"), u32 spacing`
//!   (percent of brush size), then `header_size - 28` bytes of UTF-8 name
//!   (NUL-terminated in practice — we trim), then `width * height * bpp`
//!   bytes of raw, uncompressed pixel data. No compression exists in this
//!   format at any version.
//! - **`.gbr` v1** — the same first five fields and then the name, with **no
//!   magic and no spacing**: fixed part is 20 bytes, so the name is
//!   `header_size - 20`. GIMP's own reader defaults such brushes to spacing
//!   25, and so do we. Detected by the version field; other versions are an
//!   `Err` (v1..v3 is the whole history).
//! - **`.gih`** — a UTF-8 *text* header of exactly two lines: line 1 is the
//!   brush-pipe name, line 2 is `"<count> <params...>"` where count is the
//!   number of frames and the params (`ncells`, `dim`, `ranks`, `placement`,
//!   `cellwidth`…) describe how GIMP *picks* a frame per dab. We read the
//!   count and describe-and-ignore the rest: every frame becomes its own
//!   `GbrBrush`. Immediately after the second newline sit `count`
//!   back-to-back `.gbr` blobs, each self-describing via its own header.
//!
//! Two honest degradations, both of which the importer labels:
//!
//! - **Grayscale ink polarity.** GIMP stores 255 = full ink (the *opposite*
//!   of Photoshop's inverted `.abr` masks), which is already our convention,
//!   so gray data passes straight through with no inversion. Getting this
//!   backwards would silently produce negative-space brushes.
//! - **RGBA "pixmap" brushes.** `bpp == 4` brushes carry colour, and our
//!   masks are coverage-only. We keep the **alpha channel** as coverage and
//!   drop the RGB — a coloured leaf stamp imports as its silhouette, not as
//!   a colour stamp. `bpp` 2 (gray+alpha) and 3 (RGB) do not occur in the
//!   format and are rejected rather than guessed at.

use std::path::Path;

/// One brush frame extracted from a `.gbr` file or a `.gih` frame list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GbrBrush {
    pub name: String,
    /// Coverage mask, 255 = full ink. Length = `width * height`.
    pub gray: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// GIMP's default spacing, percent of brush size (100 = one diameter).
    pub spacing_pct: u32,
}

/// Fixed header bytes before the name, per version.
const V1_HEADER: usize = 20;
const V23_HEADER: usize = 28;
/// `"GIMP"` big-endian.
const GIMP_MAGIC: u32 = 0x4749_4D50;
/// GIMP's fallback for v1 brushes, which store no spacing.
const DEFAULT_SPACING: u32 = 25;
/// Same sanity cap as `abr.rs`: a lying header must not size an allocation.
const MAX_DIM: u32 = 16384;

/// Parse a single `.gbr` brush. `fallback_name` is used when the file
/// carries an empty name (v1 files often do).
pub fn parse_gbr(bytes: &[u8], fallback_name: &str) -> Result<GbrBrush, String> {
    let mut r = Reader::new(bytes);
    read_brush(&mut r, fallback_name)
}

/// Parse a `.gih` brush pipe into one [`GbrBrush`] per frame.
///
/// `fallback_name` names frames whose embedded name is empty; the pipe's own
/// header name is *not* used, because GIMP writes it per frame anyway.
pub fn parse_gih(bytes: &[u8], fallback_name: &str) -> Result<Vec<GbrBrush>, String> {
    // Two text lines, then binary. Split on '\n' only — '\r' is trimmed
    // below so a CRLF-mangled file still reads.
    let nl1 = bytes
        .iter()
        .position(|&b| b == b'\n')
        .ok_or_else(|| "gih: no newline in the header".to_string())?;
    let nl2 = nl1
        + 1
        + bytes[nl1 + 1..]
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| "gih: header has only one line".to_string())?;
    let params = String::from_utf8_lossy(&bytes[nl1 + 1..nl2]);
    // Line 2 is "<count> <key>:<value> ..."; only the count is ours.
    let count: usize = params
        .split_whitespace()
        .next()
        .and_then(|t| t.trim().parse().ok())
        .ok_or_else(|| format!("gih: no frame count in {params:?}"))?;
    if count == 0 {
        return Err("gih: frame count is 0".into());
    }
    // A frame is at least a 20-byte header plus one pixel, so a count that
    // cannot fit in the file is a lie — bail before looping.
    let body = &bytes[nl2 + 1..];
    if count > body.len() / (V1_HEADER + 1) {
        return Err(format!(
            "gih: {count} frames declared, {} bytes of data",
            body.len()
        ));
    }

    let mut r = Reader::new(body);
    let mut frames = Vec::with_capacity(count.min(64));
    for i in 0..count {
        frames.push(
            read_brush(&mut r, fallback_name).map_err(|e| format!("gih frame {}: {e}", i + 1))?,
        );
    }
    // GIMP repeats the same name in every frame of a pipe, so suffixing is
    // the norm — but only do it when the names actually collide, so a pipe
    // with distinct frame names keeps them.
    let mut seen = std::collections::HashSet::with_capacity(frames.len());
    if !frames.iter().all(|f| seen.insert(f.name.as_str())) {
        for (i, f) in frames.iter_mut().enumerate() {
            f.name = format!("{} {}", f.name, i + 1);
        }
    }
    Ok(frames)
}

/// Read one `.gbr` blob at the cursor, leaving it just past the pixel data
/// (this is what makes the `.gih` frame walk possible).
fn read_brush(r: &mut Reader, fallback_name: &str) -> Result<GbrBrush, String> {
    let header_size = r.u32()? as usize;
    let version = r.u32()?;
    let width = r.u32()?;
    let height = r.u32()?;
    let bpp = r.u32()?;
    let (name_len, spacing) = match version {
        1 => {
            let fixed = V1_HEADER;
            if header_size < fixed {
                return Err(format!("gbr v1: header_size {header_size} < {fixed}"));
            }
            (header_size - fixed, DEFAULT_SPACING)
        }
        2 | 3 => {
            let fixed = V23_HEADER;
            if header_size < fixed {
                return Err(format!(
                    "gbr v{version}: header_size {header_size} < {fixed}"
                ));
            }
            let magic = r.u32()?;
            if magic != GIMP_MAGIC {
                return Err(format!("gbr v{version}: bad magic {magic:#010x}"));
            }
            (header_size - fixed, r.u32()?)
        }
        v => return Err(format!("gbr version {v} unsupported")),
    };
    if width == 0 || height == 0 || width > MAX_DIM || height > MAX_DIM {
        return Err(format!("gbr: bad size {width}x{height}"));
    }
    // Name bytes are bounds-checked like everything else, so a lying
    // header_size fails here rather than reserving anything.
    let name = String::from_utf8_lossy(r.take(name_len)?)
        .trim_end_matches('\0')
        .trim()
        .to_string();
    let name = if name.is_empty() {
        fallback_name.to_string()
    } else {
        name
    };

    let px = width as usize * height as usize;
    let raw_len = px
        .checked_mul(bpp as usize)
        .ok_or_else(|| format!("gbr {name:?}: {width}x{height}x{bpp} overflows"))?;
    // Plausibility BEFORE the allocation, same reasoning as abr.rs: the
    // capped dimensions still admit 16384² × 4 = 1 GB from a 28-byte header,
    // and this format has no compression at all — the exact bytes must be
    // present or the header is lying.
    if raw_len > r.left() {
        return Err(format!(
            "gbr {name:?}: {raw_len} pixel bytes declared, {} left",
            r.left()
        ));
    }
    let data = r.take(raw_len)?;
    let gray = match bpp {
        // GIMP stores 255 = full ink already: our convention, no inversion.
        1 => data.to_vec(),
        // Pixmap brush: alpha is the only coverage we can honestly keep.
        4 => data.chunks_exact(4).map(|p| p[3]).collect(),
        b => return Err(format!("gbr {name:?}: {b} bytes per pixel unsupported")),
    };
    Ok(GbrBrush {
        name,
        gray,
        width,
        height,
        spacing_pct: spacing,
    })
}

/// Big-endian cursor over the input — every read is bounds-checked.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    fn left(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.left() < n {
            return Err(format!("gbr: truncated at byte {} (+{n})", self.pos));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
}

/// Parse a `.gbr` or `.gih` file from disk, dispatching on the extension
/// (case-insensitive). A `.gbr` yields a one-element vec so both kinds feed
/// the importer through one signature.
pub fn parse_gimp_brush_file(path: &Path) -> Result<Vec<GbrBrush>, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "brush".into());
    let bytes = std::fs::read(path).map_err(|e| format!("gbr: {}: {e}", path.display()))?;
    match ext.as_str() {
        "gbr" => parse_gbr(&bytes, &stem).map(|b| vec![b]),
        "gih" => parse_gih(&bytes, &stem),
        other => Err(format!("{}: not a GIMP brush (.{other})", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A v2/v3 `.gbr` blob: header, name (NUL-terminated), raw pixels.
    fn gbr_v23(
        version: u32,
        name: &str,
        spacing: u32,
        w: u32,
        h: u32,
        bpp: u32,
        px: &[u8],
    ) -> Vec<u8> {
        let mut b = Vec::new();
        let name_len = name.len() + 1;
        b.extend_from_slice(&((V23_HEADER + name_len) as u32).to_be_bytes());
        b.extend_from_slice(&version.to_be_bytes());
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&bpp.to_be_bytes());
        b.extend_from_slice(&GIMP_MAGIC.to_be_bytes());
        b.extend_from_slice(&spacing.to_be_bytes());
        b.extend_from_slice(name.as_bytes());
        b.push(0);
        b.extend_from_slice(px);
        b
    }

    /// A v1 blob: no magic, no spacing.
    fn gbr_v1(name: &str, w: u32, h: u32, px: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&((V1_HEADER + name.len() + 1) as u32).to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(name.as_bytes());
        b.push(0);
        b.extend_from_slice(px);
        b
    }

    /// `.gih`: two text lines then `frames` concatenated `.gbr` blobs.
    fn gih(name: &str, params: &str, frames: &[Vec<u8>]) -> Vec<u8> {
        let mut b = format!("{name}\n{} {params}\n", frames.len()).into_bytes();
        for f in frames {
            b.extend_from_slice(f);
        }
        b
    }

    #[test]
    fn v2_grayscale_round_trips_with_name_and_spacing() {
        let px = [0u8, 64, 128, 255, 200, 10];
        let f = gbr_v23(2, "Round Ink", 42, 3, 2, 1, &px);
        let b = parse_gbr(&f, "fallback").unwrap();
        assert_eq!(b.name, "Round Ink");
        assert_eq!((b.width, b.height), (3, 2));
        assert_eq!(b.spacing_pct, 42);
        // No inversion: GIMP already stores 255 = full ink.
        assert_eq!(b.gray, px.to_vec());
        // v3 shares the layout exactly.
        let f3 = gbr_v23(3, "Three", 10, 3, 2, 1, &px);
        assert_eq!(parse_gbr(&f3, "fallback").unwrap().name, "Three");
    }

    #[test]
    fn v1_without_magic_parses_with_default_spacing() {
        let px = [5u8, 6, 7, 8];
        let b = parse_gbr(&gbr_v1("Old", 4, 1, &px), "fallback").unwrap();
        assert_eq!(b.name, "Old");
        assert_eq!(b.spacing_pct, DEFAULT_SPACING);
        assert_eq!(b.gray, px.to_vec());
        // An empty embedded name falls back to the caller's.
        let b = parse_gbr(&gbr_v1("", 4, 1, &px), "fallback").unwrap();
        assert_eq!(b.name, "fallback");
    }

    #[test]
    fn rgba_pixmap_keeps_alpha_as_coverage() {
        // Two pixels: opaque red, half-transparent green. RGB is dropped.
        let px = [255u8, 0, 0, 255, 0, 255, 0, 128];
        let b = parse_gbr(&gbr_v23(2, "Leaf", 20, 2, 1, 4, &px), "f").unwrap();
        assert_eq!(b.gray, vec![255, 128]);
        assert_eq!(b.gray.len(), (b.width * b.height) as usize);
        // The channel counts the format does not define are rejected, not
        // guessed at (2 = gray+alpha, 3 = RGB).
        for bpp in [0u32, 2, 3, 5] {
            assert!(parse_gbr(&gbr_v23(2, "x", 20, 1, 1, bpp, &[0; 8]), "f").is_err());
        }
    }

    #[test]
    fn gih_yields_one_brush_per_frame() {
        let f1 = gbr_v23(2, "Pipe", 15, 2, 1, 1, &[10, 20]);
        let f2 = gbr_v23(2, "Pipe", 15, 1, 2, 1, &[30, 40]);
        let brushes = parse_gih(&gih("Pipe", "ncells:2 dim:1 ranks:2", &[f1, f2]), "f").unwrap();
        assert_eq!(brushes.len(), 2);
        // Names collide (GIMP repeats them) -> suffixed.
        assert_eq!(brushes[0].name, "Pipe 1");
        assert_eq!(brushes[1].name, "Pipe 2");
        assert_eq!(brushes[0].gray, vec![10, 20]);
        assert_eq!((brushes[1].width, brushes[1].height), (1, 2));
        assert_eq!(brushes[1].spacing_pct, 15);

        // Distinct frame names are left alone.
        let a = gbr_v23(2, "Up", 15, 1, 1, 1, &[1]);
        let b = gbr_v23(2, "Down", 15, 1, 1, 1, &[2]);
        let brushes = parse_gih(&gih("Arrows", "2 dim:1", &[a, b]), "f").unwrap();
        assert_eq!(brushes[0].name, "Up");
        assert_eq!(brushes[1].name, "Down");
    }

    #[test]
    fn gih_rejects_bad_headers() {
        assert!(parse_gih(b"no newline at all", "f").is_err());
        assert!(parse_gih(b"only one line\n", "f").is_err());
        assert!(parse_gih(b"name\nnot-a-number\n", "f").is_err());
        assert!(parse_gih(b"name\n0 dim:1\n", "f").is_err());
        // A count far past what the body could hold must not loop-and-fail
        // 4 billion times.
        assert!(parse_gih(b"name\n4000000000 dim:1\n", "f").is_err());
        // Honest count, missing frame data.
        let f1 = gbr_v23(2, "P", 15, 1, 1, 1, &[9]);
        assert!(parse_gih(&gih("P", "dim:1", &[f1.clone(), f1]), "f").is_ok());
    }

    #[test]
    fn lying_headers_and_giant_dimensions_are_rejected() {
        // header_size below the fixed part for the version.
        let mut f = gbr_v23(2, "x", 10, 1, 1, 1, &[1]);
        f[0..4].copy_from_slice(&8u32.to_be_bytes());
        assert!(parse_gbr(&f, "f").is_err());
        // header_size past EOF (name would run off the end).
        let mut f = gbr_v23(2, "x", 10, 1, 1, 1, &[1]);
        f[0..4].copy_from_slice(&0xFFFF_FF00u32.to_be_bytes());
        assert!(parse_gbr(&f, "f").is_err());
        // Missing/!= "GIMP" magic on a v2 file.
        let mut f = gbr_v23(2, "x", 10, 1, 1, 1, &[1]);
        f[20..24].copy_from_slice(b"XXXX");
        assert!(parse_gbr(&f, "f").is_err());
        // Zero and over-cap dimensions.
        assert!(parse_gbr(&gbr_v23(2, "x", 10, 0, 4, 1, &[]), "f").is_err());
        assert!(parse_gbr(&gbr_v23(2, "x", 10, 4, 0, 1, &[]), "f").is_err());
        assert!(parse_gbr(&gbr_v23(2, "x", 10, 20000, 4, 1, &[]), "f").is_err());
        // Plausible dimensions, absent data: must NOT allocate 1 GB.
        assert!(parse_gbr(&gbr_v23(2, "x", 10, 16000, 16000, 4, &[]), "f").is_err());
        // Unknown version.
        assert!(parse_gbr(&gbr_v23(7, "x", 10, 1, 1, 1, &[1]), "f").is_err());
        // Empty input.
        assert!(parse_gbr(&[], "f").is_err());
    }

    #[test]
    fn file_dispatch_is_case_insensitive() {
        let dir = std::env::temp_dir().join("mn-brush-gbr-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Dots.GBR");
        std::fs::write(&p, gbr_v23(2, "Dots", 30, 2, 1, 1, &[7, 8])).unwrap();
        let v = parse_gimp_brush_file(&p).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "Dots");

        let f1 = gbr_v23(2, "", 30, 1, 1, 1, &[1]);
        let f2 = gbr_v23(2, "", 30, 1, 1, 1, &[2]);
        let p = dir.join("Spray.GiH");
        std::fs::write(&p, gih("Spray", "dim:1", &[f1, f2])).unwrap();
        let v = parse_gimp_brush_file(&p).unwrap();
        // Empty embedded names fall back to the stem, then collide.
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "Spray 1");

        assert!(parse_gimp_brush_file(&dir.join("x.abr")).is_err());
        assert!(parse_gimp_brush_file(&dir.join("no-extension")).is_err());
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::tests_support::*;
    use super::*;

    /// Same self-audit as `abr.rs`: the byte walk must never panic on
    /// hostile input. Every single-byte truncation of a synthetic v2 file
    /// and of a two-frame `.gih` must return Ok or Err — never a panic,
    /// never a hang, never a huge allocation.
    #[test]
    fn truncation_never_panics() {
        let f = v2_file();
        for cut in 0..=f.len() {
            let _ = parse_gbr(&f[..cut], "f");
        }
        let g = gih_file();
        for cut in 0..=g.len() {
            let _ = parse_gih(&g[..cut], "f");
        }
        // And every cut fed to the WRONG parser, since dispatch is by
        // extension and a mislabelled file is a real thing.
        for cut in 0..=f.len() {
            let _ = parse_gih(&f[..cut], "f");
        }
        for cut in 0..=g.len() {
            let _ = parse_gbr(&g[..cut], "f");
        }
        // Only the untruncated forms parse.
        assert!(parse_gbr(&f, "f").is_ok());
        assert_eq!(parse_gih(&g, "f").unwrap().len(), 2);
    }
}

/// Fixtures shared by the two test modules.
#[cfg(test)]
mod tests_support {
    use super::*;

    pub fn v2_file() -> Vec<u8> {
        let mut b = Vec::new();
        let name = b"Fuzz\0";
        b.extend_from_slice(&((V23_HEADER + name.len()) as u32).to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes());
        b.extend_from_slice(&3u32.to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&GIMP_MAGIC.to_be_bytes());
        b.extend_from_slice(&25u32.to_be_bytes());
        b.extend_from_slice(name);
        b.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        b
    }

    pub fn gih_file() -> Vec<u8> {
        let mut b = b"Fuzz Pipe\n2 ncells:2 dim:1 ranks:2 placement:constant\n".to_vec();
        b.extend_from_slice(&v2_file());
        b.extend_from_slice(&v2_file());
        b
    }
}
