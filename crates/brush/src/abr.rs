//! Photoshop `.abr` brush-set reader — sampled tips only (TRIAGE 151).
//!
//! Goal: get the tip bitmaps OUT, as grayscale coverage masks for the
//! texture-tip system (`mybrush::TextureMask`). The preset dynamics Photoshop
//! stores (computed brushes, v6 descriptors) are out of scope — an imported
//! tip becomes a new `textured-pencil`-style preset, and the owner retunes it
//! like any brush. CSP's own .abr import behaves the same way: a new tool
//! group with the tip shapes, not a dynamics translation.
//!
//! Layout reference: Krita's `kis_abr_brush_collection.cpp` (GPL-2.0-or-later,
//! Boudewijn Rempt / Lukáš Tvrdý / Eric Lamarque; itself a descendant of the
//! old GIMP `file-abr` plug-in) plus GIMP's historical `devel-docs/abr.txt`.
//! Verified against a real 3.6 MB v6 file (Krita's
//! `brushes_by_mar_ka_d338ela.abr` test asset, vendored into
//! `tests/data/`).
//!
//! All integers big-endian. Two container generations:
//!
//! - **v1/v2** — flat list: `i16 version, i32 count`, then per brush
//!   `i16 type (2 = sampled), i32 body_len`, body =
//!   `4 misc + 2 spacing` (skipped), v2 only a UCS-2 name
//!   (`u32 chars`, then chars × `u16`), `1 antialias + 4×i16 bounds`
//!   (skipped), then `i32 top, left, bottom, right`, `i16 depth` (8/16),
//!   `i8 compress` (0 = raw, else PackBits RLE per row), raster.
//! - **v6** — 8BIM sections; tips live in `8BIMsamp`: `i16 version,
//!   i16 subversion (1|2)`, then `8BIM` + `"samp"` + `i32 section_len`
//!   (which may run past EOF padding — trust the per-tip lengths), per tip
//!   `i32 tip_len` (advance aligned up to 4), `37 key`, sub1 `+10`,
//!   sub2 `+264` skipped, then the same T/L/B/R, depth, compress, raster
//!   as v1/v2. v10+ (CS5 "new-style" descriptors) and computed (type 1)
//!   brushes are skipped, matching every reader in the field.
//!
//! Raster storage: 8-bit gray where **0 = full ink** (Photoshop stores an
//! inverted mask), so we invert to our convention (255 = full coverage) on
//! the way out. RLE rows are Photoshop PackBits — the same codec as PSD
//! layer data: per row a `u16 compressed_len`, then PackBits bytes; `-128`
//! is a no-op padding run, `n in 0..=127` = copy n+1 literals,
//! `n in -127..=-1` = repeat the next byte 1-n times... expressed here in
//! Krita's arithmetic (`-n + 1` repeats).

use std::path::Path;

/// One extracted sampled tip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbrTip {
    /// Display name: the file's own (v2) or `<stem>_<n>` fallback.
    pub name: String,
    /// Coverage mask, 255 = full ink. Length = `width * height`.
    pub gray: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Parse the sampled tips out of an `.abr` file.
///
/// `file_stem` names the fallback tips (`"mybrushes_1"`) and shows up in
/// error messages. Unknown/unsupported content is skipped where the format
/// allows it (computed brushes, non-`samp` sections); only structural
/// corruption is an `Err`.
pub fn parse_abr(bytes: &[u8], file_stem: &str) -> Result<Vec<AbrTip>, String> {
    let mut r = Reader::new(bytes);
    let version = r.u16()?;
    match version {
        1 | 2 => {
            let count = r.u32()? as usize;
            let mut tips = Vec::new();
            for i in 0..count {
                let id = i + 1;
                // Type 1 (computed) and unknown types are skipped by length.
                if let Some(tip) = read_v12_brush(&mut r, version, file_stem, id)? {
                    tips.push(tip);
                }
                if r.left() == 0 {
                    break; // truncated count field: take what we got
                }
            }
            Ok(tips)
        }
        6 => {
            let sub = r.u16()?;
            if sub != 1 && sub != 2 {
                return Err(format!("abr v6 subversion {sub} unsupported"));
            }
            let mut tips = Vec::new();
            while !r.remainder().is_empty() {
                let sec = r.tag4()?; // "8BIM"
                if sec != *b"8BIM" {
                    return Err(format!("expected 8BIM section, got {sec:?}"));
                }
                let kind = r.tag4()?;
                let len = r.u32()? as usize;
                let body = r.take(len)?;
                if kind != *b"samp" {
                    continue; // desc/name/pattern sections: not our payload
                }
                // Inside samp: tip blocks are self-delimiting (u32 length,
                // padded to 4). One corrupt tip is skipped, not fatal —
                // Krita warns the same way and keeps the rest.
                let mut b = Reader::new(body);
                let skip = 37 + if sub == 1 { 10 } else { 264 };
                while b.left() > 4 {
                    let tip_len = b.u32()? as usize;
                    // Blocks are length-delimited and 4-aligned; `tip_len`
                    // EXCLUDES these 4 bytes (Krita: next = pos + align4(len)).
                    // Parenthesized on purpose: `&` binds looser than `+`,
                    // and the old un-bracketed form only computed the same
                    // value because `b.pos` happens to stay 4-aligned.
                    let end = (b.pos + ((tip_len + 3) & !3)).min(body.len());
                    if end <= b.pos || end - b.pos < skip + 20 {
                        break; // zero/short block: padding, not a tip
                    }
                    // Fence every read to THIS block: a tip whose declared
                    // raster overruns its own (known) length must fail as a
                    // tip, not eat the next tips' bytes and hand back a
                    // plausible garbage brush after the resync.
                    let mut tb = Reader::new(&body[b.pos..end]);
                    tb.skip(skip)?; // brush key UUID + per-subversion header
                    let id = tips.len() + 1;
                    match read_tip_tail(&mut tb, format!("{file_stem}_{id}")) {
                        Ok(tip) => tips.push(tip),
                        Err(e) => eprintln!("abr: skipping tip {id}: {e}"),
                    }
                    b.pos = end;
                }
            }
            Ok(tips)
        }
        v => Err(format!("abr version {v} unsupported")),
    }
}

/// One v1/v2 brush entry. `Ok(None)` = computed/unknown brush, skipped.
fn read_v12_brush(
    r: &mut Reader,
    version: u16,
    stem: &str,
    id: usize,
) -> Result<Option<AbrTip>, String> {
    let brush_type = r.u16()?;
    let body_len = r.u32()? as usize;
    let body = r.take(body_len)?;
    if brush_type != 2 {
        return Ok(None); // 1 = computed (no raster), anything else unknown
    }
    let mut b = Reader::new(body);
    b.skip(6)?; // u32 misc + i16 spacing
    let name = if version == 2 {
        read_ucs2_name(&mut b)?
    } else {
        None
    };
    b.skip(9)?; // antialias flag + 4 x i16 "short bounds"
    let tip = read_tip_tail(&mut b, name.unwrap_or_else(|| format!("{stem}_{id}")))?;
    // The raster may end a byte or two before the body (length padding).
    Ok(Some(tip))
}

/// The shared sampled-tip tail: bounds, depth, compression, raster.
fn read_tip_tail(b: &mut Reader, name: String) -> Result<AbrTip, String> {
    let top = b.i32()?;
    let left = b.i32()?;
    let bottom = b.i32()?;
    let right = b.i32()?;
    let depth = b.u16()?;
    let compression = b.u8()?;
    let width = right.saturating_sub(left).max(0) as u32;
    let height = bottom.saturating_sub(top).max(0) as u32;
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(format!("tip {name:?}: bad bounds {width}x{height}"));
    }
    if depth != 8 && depth != 16 {
        return Err(format!("tip {name:?}: depth {depth} unsupported"));
    }
    let bpp = (depth / 8) as usize;
    let row_bytes = width as usize * bpp;
    let raw_len = row_bytes * height as usize;
    // Plausibility BEFORE the allocation: the declared bounds admit
    // 16384² × 2 = 512 MB from a ~30-byte header, and the v6 walk skips a
    // bad tip and continues — a corrupt download repeated that commit
    // spike per tip on a 16 GB machine. Uncompressed rasters need their
    // exact bytes present; PackBits expands at most 128:1 (a 2-byte run
    // header codes ≤128 output bytes).
    let cap = if compression == 0 {
        b.left()
    } else {
        b.left().saturating_mul(128)
    };
    if raw_len > cap {
        return Err(format!(
            "tip {name:?}: raster claims {raw_len} bytes with {} in the block",
            b.left()
        ));
    }
    let mut raw = vec![0u8; raw_len];
    if compression == 0 {
        raw.copy_from_slice(b.take(row_bytes * height as usize)?);
    } else {
        let mut row_lens = Vec::with_capacity(height as usize);
        for _ in 0..height {
            row_lens.push(b.u16()? as usize);
        }
        for (y, &clen) in row_lens.iter().enumerate() {
            let packed = b.take(clen)?;
            let off = y * row_bytes;
            let written = packbits_unpack(packed, &mut raw[off..])?;
            if written < row_bytes {
                return Err(format!("tip {name:?}: RLE row {y} short"));
            }
        }
    }
    // 16-bit: keep the HIGH byte (big-endian) of each sample.
    let gray = if bpp == 1 {
        raw
    } else {
        raw.chunks_exact(2).map(|p| p[0]).collect()
    };
    // Photoshop stores ink inverted (0 = full ink); TextureMask wants
    // 255 = full coverage.
    let mut gray = gray;
    for v in &mut gray {
        *v = 255 - *v;
    }
    Ok(AbrTip {
        name,
        gray,
        width,
        height,
    })
}

/// v2's optional per-brush name: `u32 char count` + count × BE `u16`
/// (UCS-2, NUL-terminated in practice). Missing → `None`.
fn read_ucs2_name(b: &mut Reader) -> Result<Option<String>, String> {
    let chars = b.u32()? as usize;
    if chars == 0 {
        return Ok(None);
    }
    if chars > 4096 {
        return Err(format!("abr: name length {chars} implausible"));
    }
    let units = b.take(chars * 2)?;
    let mut s = String::new();
    for pair in units.chunks_exact(2) {
        let u = u16::from_be_bytes([pair[0], pair[1]]);
        if u == 0 {
            break;
        }
        s.push(char::from_u32(u as u32).unwrap_or('\u{FFFD}'));
    }
    Ok(Some(s))
}

/// Photoshop PackBits (one RLE row). Returns bytes written.
fn packbits_unpack(src: &[u8], dst: &mut [u8]) -> Result<usize, String> {
    let (mut s, mut d) = (0, 0);
    while s < src.len() && d < dst.len() {
        let n = src[s] as i8;
        s += 1;
        if n >= 0 {
            for _ in 0..n as usize + 1 {
                if s >= src.len() || d >= dst.len() {
                    return Err("packbits: literal run past end".into());
                }
                dst[d] = src[s];
                s += 1;
                d += 1;
            }
        } else if n != -128 {
            if s >= src.len() {
                return Err("packbits: repeat run past end".into());
            }
            let count = (-n as usize) + 1;
            let v = src[s];
            s += 1;
            for _ in 0..count {
                if d >= dst.len() {
                    break; // runs may overrun the row; clamp, PSD allows it
                }
                dst[d] = v;
                d += 1;
            }
        } // -128 = no-op padding
    }
    Ok(d)
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
    fn remainder(&self) -> &'a [u8] {
        &self.buf[self.pos.min(self.buf.len())..]
    }
    fn left(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.left() < n {
            return Err(format!("abr: truncated at byte {} (+{n})", self.pos));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn skip(&mut self, n: usize) -> Result<(), String> {
        self.take(n).map(|_| ())
    }
    fn tag4(&mut self) -> Result<[u8; 4], String> {
        let s = self.take(4)?;
        Ok([s[0], s[1], s[2], s[3]])
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn i32(&mut self) -> Result<i32, String> {
        let s = self.take(4)?;
        Ok(i32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
}

/// Parse a `.abr` file from disk. Convenience wrapper over [`parse_abr`].
pub fn parse_abr_file(path: &Path) -> Result<Vec<AbrTip>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("abr: {}: {e}", path.display()))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "abr".into());
    parse_abr(&bytes, &stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an 8-bit raw v1 file with one 3x2 tip (ink = 255 - value).
    fn v1_file(gray: &[u8], w: i32, h: i32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes()); // sampled
        let body_len = 6 + 9 + 4 * 4 + 2 + 1 + gray.len();
        b.extend_from_slice(&(body_len as u32).to_be_bytes());
        b.extend_from_slice(&[0; 6]); // misc + spacing
        b.extend_from_slice(&[0; 9]); // antialias + short bounds
        b.extend_from_slice(&0i32.to_be_bytes()); // top
        b.extend_from_slice(&0i32.to_be_bytes()); // left
        b.extend_from_slice(&h.to_be_bytes()); // bottom
        b.extend_from_slice(&w.to_be_bytes()); // right
        b.extend_from_slice(&8u16.to_be_bytes()); // depth
        b.push(0); // raw
        b.extend_from_slice(gray);
        b
    }

    #[test]
    fn v1_raw_tip_round_trips_and_inverts() {
        // Stored ink (0 = full): a gradient column; expect 255 - v out.
        let stored = [255, 128, 0, 64, 192, 32];
        let tips = parse_abr(&v1_file(&stored, 3, 2), "t").unwrap();
        assert_eq!(tips.len(), 1);
        let t = &tips[0];
        assert_eq!((t.width, t.height), (3, 2));
        assert_eq!(t.name, "t_1");
        assert_eq!(t.gray, vec![0, 127, 255, 191, 63, 223]);
    }

    #[test]
    fn v1_computed_brush_is_skipped_not_fatal() {
        let mut b = Vec::new();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes()); // computed
        b.extend_from_slice(&4u32.to_be_bytes()); // 4-byte body
        b.extend_from_slice(&[1, 2, 3, 4]);
        let tip = v1_file(&[10], 1, 1);
        b.extend_from_slice(&tip[6..]); // header once
        let tips = parse_abr(&b, "t").unwrap();
        assert_eq!(tips.len(), 1);
        assert_eq!(tips[0].gray, vec![245]);
    }

    #[test]
    fn v2_named_tip_with_rle_row() {
        // 4x1 tip, PackBits row: [-3 (repeat next 4x)] 200.
        let mut row = vec![0xFD, 200];
        let mut b = Vec::new();
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes()); // sampled
        let name = "Ink";
        let body_len = 6 + 4 + name.len() * 2 + 2 + 9 + 4 * 4 + 2 + 1 + 2 + row.len();
        b.extend_from_slice(&(body_len as u32).to_be_bytes());
        b.extend_from_slice(&[0; 6]);
        b.extend_from_slice(&((name.len() + 1) as u32).to_be_bytes()); // incl. NUL
        for u in name.encode_utf16().chain(std::iter::once(0)) {
            b.extend_from_slice(&u.to_be_bytes());
        }
        b.extend_from_slice(&[0; 9]);
        b.extend_from_slice(&0i32.to_be_bytes());
        b.extend_from_slice(&0i32.to_be_bytes());
        b.extend_from_slice(&1i32.to_be_bytes());
        b.extend_from_slice(&4i32.to_be_bytes());
        b.extend_from_slice(&8u16.to_be_bytes());
        b.push(1); // RLE
        b.extend_from_slice(&(row.len() as u16).to_be_bytes());
        b.append(&mut row);
        let tips = parse_abr(&b, "t").unwrap();
        assert_eq!(tips.len(), 1);
        assert_eq!(tips[0].name, "Ink");
        assert_eq!(tips[0].gray, vec![55; 4]); // 255 - 200
    }

    /// The real thing: a v6 sub2 brush set from the Krita repo's test data
    /// (abr_v6_sample.abr — a LOCAL-ONLY fixture, gitignored: it is
    /// third-party brush data we do not redistribute; provenance in
    /// DECISIONS 8.47. The test skips silently where the file is absent —
    /// CI and fresh clones — and `v6_two_tips_with_foreign_section` below
    /// pins the same container walk synthetically) — 31 PackBits tips,
    /// then trailing patt/desc sections. The walk lands exactly on the
    /// samp section end. Set fact: tip 11 (199x20) is a legitimately BLANK
    /// tip — the parser keeps it; blank-dropping is the importer's call.
    #[test]
    fn real_v6_fixture_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/abr_v6_sample.abr");
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return, // fixture not shipped: skip silently
        };
        let tips = parse_abr(&bytes, "abr").unwrap();
        assert_eq!(tips.len(), 31, "any walk desync changes the count");
        for t in &tips {
            assert!(t.width > 0 && t.height > 0);
            assert_eq!(t.gray.len(), (t.width * t.height) as usize);
        }
        // First tip's real geometry (567x701, RLE) pins the skip arithmetic.
        assert_eq!((tips[0].width, tips[0].height), (567, 701));
        assert!(tips.iter().any(|t| t.gray.iter().any(|&v| v > 200)));
        assert!(tips.iter().any(|t| t.gray.iter().any(|&v| v < 50)));
        // The blank tip comes through as all-zero coverage, not an error.
        assert_eq!(tips[10].width, 199);
        assert_eq!(tips[10].gray.iter().copied().max(), Some(0));
    }

    /// Synthetic v6/sub2 with two tips: full container walk — version,
    /// subversion, an unrelated 8BIM section skipped by length, then samp
    /// with two length-delimited tips (raw raster, 4-aligned blocks).
    #[test]
    fn v6_two_tips_with_foreign_section() {
        let tip_body = |stored: &[u8], w: i32, h: i32| -> Vec<u8> {
            let mut b = Vec::new();
            b.extend_from_slice(&0i32.to_be_bytes());
            b.extend_from_slice(&0i32.to_be_bytes());
            b.extend_from_slice(&h.to_be_bytes());
            b.extend_from_slice(&w.to_be_bytes());
            b.extend_from_slice(&8u16.to_be_bytes());
            b.push(0);
            b.extend_from_slice(stored);
            b
        };
        let inner1 = [
            &[0u8; 37][..],
            &[0u8; 264][..],
            &tip_body(&[100, 150], 2, 1),
        ]
        .concat();
        let inner2 = [&[0u8; 37][..], &[0u8; 264][..], &tip_body(&[200], 1, 1)].concat();
        let mut samp = Vec::new();
        for inner in [inner1, inner2] {
            let mut block = inner.clone();
            while block.len() % 4 != 0 {
                block.push(0); // stored length is the whole 4-aligned block
            }
            samp.extend_from_slice(&(block.len() as u32).to_be_bytes());
            samp.extend_from_slice(&block);
        }
        let mut f = Vec::new();
        f.extend_from_slice(&6u16.to_be_bytes());
        f.extend_from_slice(&2u16.to_be_bytes()); // subversion 2
        f.extend_from_slice(b"8BIMdesc");
        f.extend_from_slice(&3u32.to_be_bytes());
        f.extend_from_slice(&[9, 9, 9]); // skipped by section length
        f.extend_from_slice(b"8BIMsamp");
        f.extend_from_slice(&(samp.len() as u32).to_be_bytes());
        f.extend_from_slice(&samp);
        let tips = parse_abr(&f, "set").unwrap();
        assert_eq!(tips.len(), 2);
        assert_eq!(tips[0].name, "set_1");
        assert_eq!((tips[0].width, tips[0].height), (2, 1));
        assert_eq!(tips[0].gray, vec![155, 105]);
        assert_eq!(tips[1].gray, vec![55]);
    }

    #[test]
    fn rejects_unknown_version_and_truncation() {
        assert!(parse_abr(&10u16.to_be_bytes(), "t").is_err());
        assert!(parse_abr(&1u16.to_be_bytes(), "t").is_err()); // count missing
        let mut b = v1_file(&[1, 2, 3], 3, 1);
        b.truncate(b.len() - 1); // raster short
        assert!(parse_abr(&b, "t").is_err());
    }

    #[test]
    fn packbits_handles_literals_repeats_and_nop() {
        let mut dst = [0u8; 8];
        // 2 literals, -128 nop, -3 (repeat 4x) 9
        let src = [1, 5, 6, 0x80, 0xFD, 9];
        let n = packbits_unpack(&src, &mut dst).unwrap();
        assert_eq!(n, 6);
        assert_eq!(&dst[..6], &[5, 6, 9, 9, 9, 9]);
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;

    /// SELF-AUDIT (Opus 0ee84f8's named blind spot): the byte walks must
    /// never panic on hostile input. Truncate the real 3.6 MB v6 set at
    /// every 128 KB boundary and a v1 file at every byte — every cut
    /// yields Ok (fewer tips) or Err, never a panic, never a hang.
    #[test]
    fn truncation_never_panics() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/abr_v6_sample.abr");
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        // The real file's samp section declares its full length, so any
        // mid-section cut is a clean Err at the section take — the value
        // here is NO PANIC across every cut, not partial parses.
        for cut in (128..bytes.len()).step_by(128 * 1024) {
            if let Ok(tips) = parse_abr(&bytes[..cut], "f") {
                assert!(tips.len() <= 31);
            }
        }

        // The v1 file at every single byte cut.
        let mut b = Vec::new();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(&25u32.to_be_bytes());
        b.extend_from_slice(&[0; 6]);
        b.extend_from_slice(&[0; 9]);
        b.extend_from_slice(&0i32.to_be_bytes());
        b.extend_from_slice(&0i32.to_be_bytes());
        b.extend_from_slice(&1i32.to_be_bytes());
        b.extend_from_slice(&2i32.to_be_bytes());
        b.extend_from_slice(&8u16.to_be_bytes());
        b.push(0);
        b.extend_from_slice(&[10, 20]);
        for cut in 0..b.len() {
            let _ = parse_abr(&b[..cut], "f");
        }
        // A synthetic v6 file at every single byte cut too.
        let tip_body = |stored: &[u8], w: i32, h: i32| -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&0i32.to_be_bytes());
            v.extend_from_slice(&0i32.to_be_bytes());
            v.extend_from_slice(&h.to_be_bytes());
            v.extend_from_slice(&w.to_be_bytes());
            v.extend_from_slice(&8u16.to_be_bytes());
            v.push(0);
            v.extend_from_slice(stored);
            v
        };
        let inner = [
            &[0u8; 37][..],
            &[0u8; 264][..],
            &tip_body(&[100, 150], 2, 1),
        ]
        .concat();
        let mut v6 = Vec::new();
        v6.extend_from_slice(&6u16.to_be_bytes());
        v6.extend_from_slice(&2u16.to_be_bytes());
        v6.extend_from_slice(b"8BIMsamp");
        v6.extend_from_slice(&((inner.len() + 4) as u32).to_be_bytes());
        v6.extend_from_slice(&(inner.len() as u32).to_be_bytes());
        v6.extend_from_slice(&inner);
        for cut in 0..v6.len() {
            let _ = parse_abr(&v6[..cut], "f");
        }

        // A lying body length far past EOF.
        let mut lie = b.clone();
        lie[6..10].copy_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
        assert!(parse_abr(&lie, "f").is_err());
        // A lying v6 section length.
        let mut v6 = Vec::new();
        v6.extend_from_slice(&6u16.to_be_bytes());
        v6.extend_from_slice(&2u16.to_be_bytes());
        v6.extend_from_slice(b"8BIMsamp");
        v6.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        assert!(parse_abr(&v6, "f").is_err());
    }
}
