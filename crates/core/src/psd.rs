//! Layered PSD EXPORT (write-only — the practical studio hand-off the
//! ROADMAP promises in place of a `.clip` writer that cannot exist).
//!
//! Format: Adobe "Photoshop File Formats Specification". Everything is
//! big-endian. The shape written here:
//!
//! - header `8BPS` v1, RGB, 8-bit, 4 composite channels
//! - empty colour-mode + image-resource sections
//! - the layer section, records BOTTOM-to-top — which is exactly
//!   `Document::layers` order, and our flat depth encoding maps 1:1 onto
//!   PSD's group sandwich: a depth INCREASE opens a group (a hidden
//!   `</Layer group>` divider record, `lsct` type 3), and the folder
//!   HEADER (which sits above its children in both models) closes it
//!   (`lsct` 1 open / 2 closed; `pass` blend key when the folder is
//!   pass-through)
//! - per layer: tight display-tile bounds, four channels (A,R,G,B) in
//!   PackBits RLE, straight (unpremultiplied) 8-bit; blend key, opacity,
//!   the clip flag as PSD clipping, visibility; names as Pascal AND
//!   Unicode (`luni`) so Japanese layer names survive
//! - the merged composite (the export composite, drafts excluded from it
//!   exactly like PNG) as RGBA RLE image data
//!
//! Layers whose pixels are DERIVED (tone, fills, frames, balloons, text)
//! export their `display_tiles()` — what they look like, rasterized; the
//! caller must `refresh_derived` first (the CODE-MAP rule). Draft layers
//! are included as ordinary visible layers: this is a working-file
//! hand-off, and the recipient should see what the artist sees.
//!
//! Deliberately not written (v1, recorded here): layer masks (flattened
//! into the pixels via `display_tiles`), layer effects, adjustment
//! layers, and `.psb` (>30k px per side is refused loudly).

use std::io::Write;

use crate::doc::{Blend, Document};
use crate::export::{Background, composite_for_export};
use crate::tile::TILE_SIZE;

#[derive(Debug)]
pub struct PsdError(pub String);

impl std::fmt::Display for PsdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "psd: {}", self.0)
    }
}

/// The PSD blend key for one of our modes, space-padded to 4.
fn blend_key(b: Blend) -> &'static [u8; 4] {
    match b {
        Blend::Normal => b"norm",
        Blend::Multiply => b"mul ",
        Blend::Screen => b"scrn",
        Blend::Add => b"lddg",
        Blend::Subtract => b"fsub",
        Blend::Darken => b"dark",
        Blend::Lighten => b"lite",
        Blend::Overlay => b"over",
        Blend::SoftLight => b"sLit",
        Blend::HardLight => b"hLit",
        Blend::Difference => b"diff",
        Blend::Exclusion => b"smud",
        Blend::Hue => b"hue ",
        Blend::Saturation => b"sat ",
        Blend::Color => b"colr",
        Blend::ColorBurn => b"idiv",
        Blend::LinearBurn => b"lbrn",
        Blend::ColorDodge => b"div ",
        // Glow dodge is CSP-only; Color Dodge is Photoshop's own closest
        // relative (CSP documents it as the compatibility fallback).
        Blend::GlowDodge => b"div ",
        Blend::VividLight => b"vLit",
        Blend::LinearLight => b"lLit",
        Blend::PinLight => b"pLit",
        Blend::HardMix => b"hMix",
        Blend::Divide => b"fdiv",
        Blend::DarkerColor => b"dkCl",
        Blend::LighterColor => b"lgCl",
        Blend::Luminosity => b"lum ",
    }
}

/// One layer flattened to straight-alpha planes inside its tight bounds.
struct Plane {
    /// left, top, right, bottom (PSD order writes top,left,bottom,right).
    rect: [i32; 4],
    /// A, R, G, B — one `w*h` byte plane each (PSD channel ids -1,0,1,2).
    chans: [Vec<u8>; 4],
}

fn planes_of(doc: &Document, li: usize) -> Plane {
    let l = &doc.layers[li];
    let tiles = l.display_tiles();
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for idx in tiles.keys() {
        let (ox, oy) = idx.origin();
        x0 = x0.min(ox);
        y0 = y0.min(oy);
        x1 = x1.max(ox + TILE_SIZE as i32);
        y1 = y1.max(oy + TILE_SIZE as i32);
    }
    if x0 > x1 {
        // An empty layer still needs a valid (zero-area) record.
        return Plane {
            rect: [0, 0, 0, 0],
            chans: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
        };
    }
    // Clamp to the canvas: PSD tolerates out-of-canvas bounds, but our
    // off-canvas tiles are scratch, not content.
    let (w, h) = (doc.size.0 as i32, doc.size.1 as i32);
    let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(w), y1.min(h));
    let (pw, ph) = ((x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize);
    let mut chans = [
        vec![0u8; pw * ph],
        vec![0u8; pw * ph],
        vec![0u8; pw * ph],
        vec![0u8; pw * ph],
    ];
    for (idx, t) in tiles {
        let (ox, oy) = idx.origin();
        for py in 0..TILE_SIZE {
            let cy = oy + py as i32;
            if cy < y0 || cy >= y1 {
                continue;
            }
            for px in 0..TILE_SIZE {
                let cx = ox + px as i32;
                if cx < x0 || cx >= x1 {
                    continue;
                }
                let p = t.pixel(px, py); // premultiplied fix15
                let a = p[3] as u32;
                let o = (cy - y0) as usize * pw + (cx - x0) as usize;
                if a == 0 {
                    continue; // planes are zeroed
                }
                // Unpremultiply, the register-layer rounding.
                let un = |c: u16| (((c as u32 * 32768 / a).min(32768) * 255 + 16384) / 32768) as u8;
                chans[0][o] = ((a * 255 + 16384) / 32768) as u8;
                chans[1][o] = un(p[0]);
                chans[2][o] = un(p[1]);
                chans[3][o] = un(p[2]);
            }
        }
    }
    Plane {
        rect: [x0, y0, x1, y1],
        chans,
    }
}

/// PackBits one row (the encoder half of the abr reader's decoder).
fn packbits_row(row: &[u8], out: &mut Vec<u8>) {
    let n = row.len();
    let mut i = 0;
    while i < n {
        // Run of equal bytes?
        let mut run = 1;
        while i + run < n && row[i + run] == row[i] && run < 128 {
            run += 1;
        }
        if run >= 2 {
            // Header for a repeat of n is (1 - n) as i8; n = 128 must not
            // overflow the i8 math (1 - 128 = -127, computed wide).
            out.push((1 - run as i32) as u8);
            out.push(row[i]);
            i += run;
            continue;
        }
        // Literal run until the next 3-byte repeat (2 is break-even).
        let start = i;
        i += 1;
        while i < n && i - start < 128 {
            let mut r = 1;
            while i + r < n && row[i + r] == row[i] && r < 3 {
                r += 1;
            }
            if r >= 3 {
                break;
            }
            i += 1;
        }
        out.push((i - start - 1) as u8);
        out.extend_from_slice(&row[start..i]);
    }
}

/// RLE-compress one plane: the per-row length table, then the rows.
/// Returns (lengths, data).
fn rle_plane(plane: &[u8], w: usize, h: usize) -> (Vec<u16>, Vec<u8>) {
    let mut lens = Vec::with_capacity(h);
    let mut data = Vec::new();
    for y in 0..h {
        let before = data.len();
        packbits_row(&plane[y * w..(y + 1) * w], &mut data);
        lens.push((data.len() - before) as u16);
    }
    (lens, data)
}

fn pascal_name_padded(name: &str) -> Vec<u8> {
    // Pascal string, padded so (1 + len + pad) is a multiple of 4. The
    // byte name is a lossy ASCII fallback; the real name rides `luni`.
    let ascii: Vec<u8> = name
        .chars()
        .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
        .take(255)
        .collect();
    let mut out = vec![ascii.len() as u8];
    out.extend_from_slice(&ascii);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

fn luni_block(name: &str) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let mut data = (units.len() as u32).to_be_bytes().to_vec();
    for u in &units {
        data.extend_from_slice(&u.to_be_bytes());
    }
    while data.len() % 4 != 0 {
        data.push(0);
    }
    let mut out = b"8BIMluni".to_vec();
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&data);
    out
}

fn lsct_block(kind: u32) -> Vec<u8> {
    let mut out = b"8BIMlsct".to_vec();
    out.extend_from_slice(&4u32.to_be_bytes());
    out.extend_from_slice(&kind.to_be_bytes());
    out
}

/// One layer record + its channel data, appended to the two streams.
#[allow(clippy::too_many_arguments)] // a serializer's flat inputs
fn push_layer(
    records: &mut Vec<u8>,
    channels: &mut Vec<u8>,
    plane: &Plane,
    name: &str,
    blend: &[u8; 4],
    opacity: u8,
    clipping: bool,
    visible: bool,
    lsct: Option<u32>,
) {
    let [x0, y0, x1, y1] = plane.rect;
    let (w, h) = ((x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize);
    records.extend_from_slice(&(y0).to_be_bytes());
    records.extend_from_slice(&(x0).to_be_bytes());
    records.extend_from_slice(&(y1).to_be_bytes());
    records.extend_from_slice(&(x1).to_be_bytes());
    records.extend_from_slice(&4u16.to_be_bytes()); // channel count
    // Channel infos + data. Order A(-1), R(0), G(1), B(2).
    for (ci, id) in [(-1i16, 0usize), (0, 1), (1, 2), (2, 3)] {
        let (lens, data) = rle_plane(&plane.chans[id], w, h);
        let chan_len = 2 + lens.len() * 2 + data.len();
        records.extend_from_slice(&ci.to_be_bytes());
        records.extend_from_slice(&(chan_len as u32).to_be_bytes());
        channels.extend_from_slice(&1u16.to_be_bytes()); // RLE
        for l in &lens {
            channels.extend_from_slice(&l.to_be_bytes());
        }
        channels.extend_from_slice(&data);
    }
    records.extend_from_slice(b"8BIM");
    records.extend_from_slice(blend);
    records.push(opacity);
    records.push(u8::from(clipping));
    // Flags: bit 1 SET = hidden.
    records.push(if visible { 0 } else { 2 });
    records.push(0); // filler
    let name_p = pascal_name_padded(name);
    let luni = luni_block(name);
    let lsct_b = lsct.map(lsct_block).unwrap_or_default();
    let extra_len = 4 + 4 + name_p.len() + luni.len() + lsct_b.len();
    records.extend_from_slice(&(extra_len as u32).to_be_bytes());
    records.extend_from_slice(&0u32.to_be_bytes()); // no layer mask data
    records.extend_from_slice(&0u32.to_be_bytes()); // no blending ranges
    records.extend_from_slice(&name_p);
    records.extend_from_slice(&luni);
    records.extend_from_slice(&lsct_b);
}

/// Write the document as a layered PSD. `refresh_derived` must have run
/// (every pixel read goes through `display_tiles`).
pub fn save_psd<W: Write>(doc: &Document, mut out: W) -> Result<(), PsdError> {
    let (w, h) = (doc.size.0, doc.size.1);
    if w == 0 || h == 0 || w > 30_000 || h > 30_000 {
        return Err(PsdError(format!(
            "canvas {w}x{h} outside PSD's 30000 px limit (PSB is not written)"
        )));
    }

    let mut records = Vec::new();
    let mut channels = Vec::new();
    let mut count: i16 = 0;
    let empty = Plane {
        rect: [0, 0, 0, 0],
        chans: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
    };

    // Bottom-to-top; our depth encoding maps directly (module doc).
    let mut depth = 0u8;
    for (li, l) in doc.layers.iter().enumerate() {
        while l.depth > depth {
            // A group opens beneath these children.
            push_layer(
                &mut records,
                &mut channels,
                &empty,
                "</Layer group>",
                b"norm",
                255,
                false,
                true,
                Some(3),
            );
            count += 1;
            depth += 1;
        }
        if l.folder {
            // The header closes the group it owns.
            depth = l.depth;
            let blend = if l.through { b"pass" } else { blend_key(l.blend) };
            push_layer(
                &mut records,
                &mut channels,
                &planes_of(doc, li), // frame folders carry border ink
                &l.name,
                blend,
                (l.opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
                false,
                l.visible,
                Some(if l.open { 1 } else { 2 }),
            );
            count += 1;
            continue;
        }
        push_layer(
            &mut records,
            &mut channels,
            &planes_of(doc, li),
            &l.name,
            blend_key(l.blend),
            (l.opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
            l.clip,
            l.visible,
            None,
        );
        count += 1;
    }
    // Groups still open at the top would desync every reader — the model
    // guarantees headers close them, but a guarantee is not a guard.
    if depth != 0 {
        return Err(PsdError("unbalanced folder nesting".into()));
    }

    let e = |err: std::io::Error| PsdError(err.to_string());

    // -- header --
    out.write_all(b"8BPS").map_err(e)?;
    out.write_all(&1u16.to_be_bytes()).map_err(e)?;
    out.write_all(&[0; 6]).map_err(e)?;
    out.write_all(&4u16.to_be_bytes()).map_err(e)?; // composite RGBA
    out.write_all(&h.to_be_bytes()).map_err(e)?;
    out.write_all(&w.to_be_bytes()).map_err(e)?;
    out.write_all(&8u16.to_be_bytes()).map_err(e)?;
    out.write_all(&3u16.to_be_bytes()).map_err(e)?; // RGB
    out.write_all(&0u32.to_be_bytes()).map_err(e)?; // colour mode data
    out.write_all(&0u32.to_be_bytes()).map_err(e)?; // image resources

    // -- layer & mask info --
    let mut layer_info = (count).to_be_bytes().to_vec();
    layer_info.extend_from_slice(&records);
    layer_info.extend_from_slice(&channels);
    if layer_info.len() % 2 != 0 {
        layer_info.push(0);
    }
    let section_len = 4 + layer_info.len() + 4; // layer info + global mask
    out.write_all(&(section_len as u32).to_be_bytes()).map_err(e)?;
    out.write_all(&(layer_info.len() as u32).to_be_bytes())
        .map_err(e)?;
    out.write_all(&layer_info).map_err(e)?;
    out.write_all(&0u32.to_be_bytes()).map_err(e)?; // global layer mask

    // -- merged composite (RGBA planes, RLE) --
    let merged = composite_for_export(doc, Background::Transparent);
    let (mw, mh) = (merged.width() as usize, merged.height() as usize);
    let raw = merged.as_raw();
    let mut planes = vec![vec![0u8; mw * mh]; 4];
    for (i, px) in raw.chunks_exact(4).enumerate() {
        planes[0][i] = px[0];
        planes[1][i] = px[1];
        planes[2][i] = px[2];
        planes[3][i] = px[3];
    }
    out.write_all(&1u16.to_be_bytes()).map_err(e)?; // RLE
    let mut all_lens = Vec::new();
    let mut all_data = Vec::new();
    for p in &planes {
        let (lens, data) = rle_plane(p, mw, mh);
        all_lens.extend(lens);
        all_data.extend(data);
    }
    for l in &all_lens {
        out.write_all(&l.to_be_bytes()).map_err(e)?;
    }
    out.write_all(&all_data).map_err(e)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TileIdx;

    /// PackBits: our encoder must round-trip through the same decoder the
    /// abr reader ships (re-implemented here byte-for-byte).
    fn unpack(src: &[u8], expect: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(expect);
        let mut s = 0;
        while s < src.len() && out.len() < expect {
            let n = src[s] as i8;
            s += 1;
            if n >= 0 {
                for _ in 0..=n {
                    out.push(src[s]);
                    s += 1;
                }
            } else if n != -128 {
                for _ in 0..(1 - n as i32) {
                    out.push(src[s]);
                }
                s += 1;
            }
        }
        out
    }

    #[test]
    fn packbits_round_trips() {
        for row in [
            vec![0u8; 64],
            (0..=255u8).collect::<Vec<_>>(),
            vec![1, 1, 1, 2, 3, 3, 3, 3, 4, 5, 5],
            b"aaabccddddddde".to_vec(),
        ] {
            let mut packed = Vec::new();
            packbits_row(&row, &mut packed);
            assert_eq!(unpack(&packed, row.len()), row, "row {row:?}");
        }
    }

    /// Structure: a doc with a folder (two children), blend modes, a clip
    /// flag and a Japanese name serializes; the mini-reader walks the whole
    /// file — header, every record, channel lengths, group sandwich, luni —
    /// and lands exactly at the composite.
    #[test]
    fn psd_structure_walks_end_to_end() {
        let mut doc = Document::new(200, 120);
        const W: u16 = crate::FIX15_ONE as u16;
        doc.begin_op();
        doc.active_layer_mut()
            .tile_mut(TileIdx::new(0, 0))
            .set_pixel(5, 6, [W, 0, 0, W]);
        doc.end_op();
        doc.layers[0].name = "ベタ".into();
        doc.layers[0].blend = Blend::Multiply;
        let f = doc.add_folder_above(0, "Folder");
        let inner = doc.add_layer_in_folder(f, "inner").unwrap();
        doc.layers[inner].clip = true;

        let mut buf = Vec::new();
        save_psd(&doc, &mut buf).unwrap();

        // -- mini reader --
        let be16 = |o: usize| u16::from_be_bytes([buf[o], buf[o + 1]]);
        let be32 = |o: usize| u32::from_be_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
        assert_eq!(&buf[0..4], b"8BPS");
        assert_eq!(be16(4), 1);
        assert_eq!(be16(12), 4);
        assert_eq!(be32(14), 120);
        assert_eq!(be32(18), 200);
        assert_eq!(be16(22), 8);
        assert_eq!(be16(24), 3);
        let mut o = 26;
        assert_eq!(be32(o), 0); // colour mode
        o += 4;
        assert_eq!(be32(o), 0); // resources
        o += 4;
        let section_len = be32(o) as usize;
        o += 4;
        let section_end = o + section_len;
        let layer_info_len = be32(o) as usize;
        o += 4;
        let count = i16::from_be_bytes([buf[o], buf[o + 1]]);
        o += 2;
        // bottom layer, then the group divider, inner, header = 4 records.
        assert_eq!(count, 4);
        let mut chan_total = 0usize;
        let mut names = Vec::new();
        let mut lscts = Vec::new();
        let mut clips = Vec::new();
        for _ in 0..count {
            o += 16; // bounds
            let nchan = be16(o) as usize;
            o += 2;
            assert_eq!(nchan, 4);
            for _ in 0..nchan {
                o += 2;
                chan_total += be32(o) as usize;
                o += 4;
            }
            assert_eq!(&buf[o..o + 4], b"8BIM");
            o += 8; // sig + blend
            o += 4; // opacity, clipping, flags, filler
            clips.push(buf[o - 3]);
            let extra = be32(o) as usize;
            o += 4;
            let extra_end = o + extra;
            o += 8; // empty mask + ranges
            let plen = buf[o] as usize;
            names.push(String::from_utf8_lossy(&buf[o + 1..o + 1 + plen]).into_owned());
            // The pad is relative to the STRING start, not the file offset.
            let raw = 1 + plen;
            o += raw + ((4 - raw % 4) % 4);
            // additional blocks until extra_end
            while o < extra_end {
                assert_eq!(&buf[o..o + 4], b"8BIM");
                let key = &buf[o + 4..o + 8];
                let blen = be32(o + 8) as usize;
                if key == b"lsct" {
                    lscts.push(be32(o + 12));
                }
                o = o + 12 + blen;
            }
            assert_eq!(o, extra_end);
        }
        // The channel data follows the records and is exactly as declared.
        o += chan_total;
        assert!(o <= section_end);
        // The Japanese name survived as luni (check the raw bytes exist).
        let jp: Vec<u8> = "ベタ".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        assert!(
            buf.windows(jp.len()).any(|w| w == jp),
            "unicode layer name missing"
        );
        assert_eq!(names[1], "</Layer group>");
        assert_eq!(lscts, vec![3, 1], "divider below, open header above");
        assert_eq!(clips[2], 1, "the clip flag rode across");
        // Merged composite: compression flag right after the section.
        assert_eq!(be16(section_end), 1);
        // And the file does not end before the RLE tables.
        assert!(buf.len() > section_end + 2 + 4 * 120 * 2);
        // Alignment guarantee: layer info even.
        assert_eq!(layer_info_len % 2, 0);
    }

    /// Empty documents and the size guard.
    #[test]
    fn psd_guards() {
        let doc = Document::new(200, 120);
        let mut buf = Vec::new();
        save_psd(&doc, &mut buf).unwrap();
        assert!(!buf.is_empty());
        let big = Document::new(40_000, 100);
        assert!(save_psd(&big, &mut Vec::new()).is_err());
    }
}
