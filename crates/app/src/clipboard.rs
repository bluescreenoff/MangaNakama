//! The image clipboard (TRIAGE 131): Cut/Copy/Paste floats + the OS
//! clipboard as CF_DIB, so bitmaps move in and out of the app, not just
//! around inside it.
//!
//! Layout: the DIB codec (`encode_dib`/`decode_dib`) and the pixel-format
//! conversions are PURE functions — unit-tested without Win32. The
//! `clipboard_set_dib`/`clipboard_get_dib` glue follows win32.rs's text
//! precedent (`OpenClipboard(null)`, `GlobalAlloc(GMEM_MOVEABLE)`; the
//! handle belongs to the clipboard after a successful `SetClipboardData`).
//!
//! Pixel math: document tiles are PREMULTIPLIED fix15 (`1.0 == 1<<15`), the
//! DIB is STRAIGHT u8 BGRA — every crossing unpremultiplies/premultiplies
//! with rounding to match, so a copy/paste round trip through the OS
//! clipboard is lossless up to the 8-bit quantization the format itself
//! imposes (the INTERNAL clipboard carries the fix15 original untouched).

use mn_core::{FloatSource, TILE_SIZE, Tile, TileIdx};

/// winuser.h. Only this one format is used; not worth a feature module
/// (win32.rs's CF_UNICODETEXT precedent).
const CF_DIB: u32 = 8;

const FIX15_MAX: u32 = 32767;

/// fix15 premultiplied → straight u8 (r, g, b channels each 0..=255).
#[inline]
fn premul15_to_u8(c: u16, a: u16) -> u8 {
    if a == 0 {
        0
    } else {
        ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8
    }
}

/// straight u8 → premultiplied fix15.
#[inline]
fn u8_to_premul15(c: u8, a: u8) -> u16 {
    ((c as u32 * a as u32 * FIX15_MAX + 127) / (255 * 255)) as u16
}

/// Encode a top-down straight-BGRA image as a 32bpp BI_RGB DIB
/// (BITMAPINFOHEADER + bottom-up rows). `bgra.len()` must be `w * h * 4`.
pub fn encode_dib(bgra: &[u8], w: usize, h: usize) -> Vec<u8> {
    assert_eq!(bgra.len(), w * h * 4);
    let stride = w * 4;
    let mut out = Vec::with_capacity(40 + bgra.len());
    // BITMAPINFOHEADER, all-zeros except the fields that matter.
    let mut hdr = [0u8; 40];
    hdr[0..4].copy_from_slice(&40u32.to_le_bytes()); // biSize
    hdr[4..8].copy_from_slice(&(w as i32).to_le_bytes()); // biWidth
    hdr[8..12].copy_from_slice(&(h as i32).to_le_bytes()); // biHeight > 0: bottom-up
    hdr[12..14].copy_from_slice(&1u16.to_le_bytes()); // biPlanes
    hdr[14..16].copy_from_slice(&32u16.to_le_bytes()); // biBitCount
    hdr[16..20].copy_from_slice(&0u32.to_le_bytes()); // BI_RGB
    hdr[20..24].copy_from_slice(&((w * h * 4) as u32).to_le_bytes()); // biSizeImage
    out.extend_from_slice(&hdr);
    for row in (0..h).rev() {
        out.extend_from_slice(&bgra[row * stride..row * stride + stride]);
    }
    out
}

/// Decode a DIB into a top-down straight-BGRA image. 32bpp and 24bpp
/// BI_RGB only (what every producer of CF_DIB around here emits); returns
/// `None` for anything else rather than guessing through a palette.
pub fn decode_dib(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if bytes.len() < 40 {
        return None;
    }
    let rd_u32 = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let rd_i32 = |o: usize| i32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let rd_u16 = |o: usize| u16::from_le_bytes(bytes[o..o + 2].try_into().unwrap());
    let bi_size = rd_u32(0) as usize;
    if bi_size < 40 || bytes.len() < bi_size {
        return None; // BITMAPV4/V5 headers are longer but start the same;
        // rejecting keeps v1 honest. (Revisit if a producer
        // appears.)
    }
    let w = rd_i32(4);
    let h_raw = rd_i32(8);
    let planes = rd_u16(12);
    let bpp = rd_u16(14);
    let compression = rd_u32(16);
    if planes != 1 || compression != 0 || w <= 0 || h_raw == 0 {
        return None;
    }
    let top_down = h_raw < 0;
    let h = h_raw.unsigned_abs() as usize;
    let w = w as usize;
    let (src_bpp, src_stride) = match bpp {
        32 => (4, w * 4),
        24 => (3, (w * 3 + 3) & !3),
        _ => return None,
    };
    // Checked payload math: a lying header at absurd dimensions must be
    // refused, never wrapped into a bogus slice bound (audit 36–48 §3).
    let payload = src_stride.checked_mul(h)?;
    if bytes.len() - bi_size < payload {
        return None;
    }
    let rows = &bytes[bi_size..];
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let src_row = if top_down { y } else { h - 1 - y };
        let s = &rows[src_row * src_stride..];
        for x in 0..w {
            let d = (y * w + x) * 4;
            match src_bpp {
                4 => out[d..d + 4].copy_from_slice(&s[x * 4..x * 4 + 4]),
                _ => {
                    out[d] = s[x * 3];
                    out[d + 1] = s[x * 3 + 1];
                    out[d + 2] = s[x * 3 + 2];
                    out[d + 3] = 255;
                }
            }
        }
    }
    Some((out, w as u32, h as u32))
}

/// A `FloatSource` as a top-down straight-BGRA image (its tight rect).
pub fn floatsource_to_bgra(src: &FloatSource) -> (Vec<u8>, u32, u32) {
    let w = (src.rect[2] - src.rect[0]).max(0) as usize;
    let h = (src.rect[3] - src.rect[1]).max(0) as usize;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let p = src.pixel(src.rect[0] + x as i32, src.rect[1] + y as i32);
            let d = (y * w + x) * 4;
            out[d] = premul15_to_u8(p[2], p[3]); // B
            out[d + 1] = premul15_to_u8(p[1], p[3]); // G
            out[d + 2] = premul15_to_u8(p[0], p[3]); // R
            out[d + 3] = (p[3] as u32 * 255 / FIX15_MAX) as u8; // A
        }
    }
    (out, w as u32, h as u32)
}

/// A top-down straight-BGRA image as a `FloatSource` whose rect sits at
/// `at` (top-left, canvas px), clipped to the canvas extent `ex × ey`.
/// Fully-off-canvas produces an empty source (the caller reports it).
pub fn bgra_to_floatsource(
    bgra: &[u8],
    w: u32,
    h: u32,
    at: [i32; 2],
    ex: i32,
    ey: i32,
) -> FloatSource {
    let (w, h) = (w as usize, h as usize);
    let mut rect = [at[0], at[1], at[0] + w as i32, at[1] + h as i32];
    rect[0] = rect[0].clamp(0, ex);
    rect[1] = rect[1].clamp(0, ey);
    rect[2] = rect[2].clamp(0, ex);
    rect[3] = rect[3].clamp(0, ey);
    let mut tiles = std::collections::HashMap::new();
    if rect[0] < rect[2] && rect[1] < rect[3] {
        let t0 = TileIdx::of_pixel(rect[0], rect[1]);
        let t1 = TileIdx::of_pixel(rect[2] - 1, rect[3] - 1);
        for ty in t0.y..=t1.y {
            for tx in t0.x..=t1.x {
                let ti = TileIdx::new(tx, ty);
                let mut tile = Tile::default();
                let (ox, oy) = ti.origin();
                let mut any = false;
                for ly in 0..TILE_SIZE {
                    for lx in 0..TILE_SIZE {
                        let (cx, cy) = (ox + lx as i32, oy + ly as i32);
                        if cx < rect[0] || cy < rect[1] || cx >= rect[2] || cy >= rect[3] {
                            continue;
                        }
                        let s = ((cy - at[1]) as usize * w + (cx - at[0]) as usize) * 4;
                        let a = bgra[s + 3];
                        let px = [
                            u8_to_premul15(bgra[s + 2], a), // R
                            u8_to_premul15(bgra[s + 1], a), // G
                            u8_to_premul15(bgra[s], a),     // B
                            (a as u32 * FIX15_MAX / 255) as u16,
                        ];
                        if px[3] > 0 {
                            any = true;
                            tile.set_pixel(lx, ly, px);
                        }
                    }
                }
                if any {
                    tiles.insert(ti, std::sync::Arc::new(tile));
                }
            }
        }
    }
    FloatSource { tiles, rect }
}

/// Put an image on the Windows clipboard as a DIB. Best-effort: failures
/// return false and the caller reports via the status line (the INTERNAL
/// clipboard still holds the full-fidelity copy).
pub fn clipboard_set_dib(bgra: &[u8], w: usize, h: usize) -> bool {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    let dib = encode_dib(bgra, w, h);
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();
        let mut ok = false;
        let handle = GlobalAlloc(GMEM_MOVEABLE, dib.len());
        if !handle.is_null() {
            let p = GlobalLock(handle) as *mut u8;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(dib.as_ptr(), p, dib.len());
                GlobalUnlock(handle);
                ok = !SetClipboardData(CF_DIB, handle).is_null();
            }
            if !ok {
                GlobalFree(handle);
            }
        }
        CloseClipboard();
        ok
    }
}

/// Read a DIB off the Windows clipboard as a top-down straight-BGRA image.
pub fn clipboard_get_dib() -> Option<(Vec<u8>, u32, u32)> {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    unsafe {
        if IsClipboardFormatAvailable(CF_DIB) == 0 || OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let mut out = None;
        let h = GetClipboardData(CF_DIB);
        if !h.is_null() {
            let p = GlobalLock(h) as *const u8;
            if !p.is_null() {
                // GlobalSize gives the allocation's byte count — decode
                // defensively against a short header and let `decode_dib`'s
                // own bounds checks do the rest.
                let len = windows_sys::Win32::System::Memory::GlobalSize(h);
                out = decode_dib(std::slice::from_raw_parts(p, len));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×1 image: opaque red, half-transparent blue.
    fn sample_bgra() -> (Vec<u8>, usize, usize) {
        (vec![0, 0, 255, 255, 255, 0, 0, 128], 2, 1)
    }

    #[test]
    fn dib_round_trip_is_lossless() {
        let (bgra, w, h) = sample_bgra();
        let dib = encode_dib(&bgra, w, h);
        let (out, ow, oh) = decode_dib(&dib).expect("decode");
        assert_eq!((ow, oh), (2, 1));
        assert_eq!(out, bgra, "32bpp BI_RGB must survive a round trip");
    }

    #[test]
    fn dib_header_fields() {
        let (bgra, w, h) = sample_bgra();
        let dib = encode_dib(&bgra, w, h);
        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
        assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 2);
        // Positive height = bottom-up: the LAST source row leads the data.
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(dib[14..16].try_into().unwrap()), 32);
        assert_eq!(&dib[40..44], &[0, 0, 255, 255]);
    }

    #[test]
    fn dib_top_down_height_decodes() {
        // Hand-build a 1×2 top-down DIB (negative height): row order flips.
        let mut dib = vec![0u8; 40 + 8];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&(-2i32).to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[40..44].copy_from_slice(&[1, 1, 1, 255]); // first stored row
        dib[44..48].copy_from_slice(&[2, 2, 2, 255]);
        let (out, w, h) = decode_dib(&dib).expect("decode");
        assert_eq!((w, h), (1, 2));
        assert_eq!(out[0], 1, "top-down: stored row 0 is image row 0");
        assert_eq!(out[4], 2);
    }

    #[test]
    fn dib_24bpp_stride_and_pad() {
        // 3px wide 24bpp: row = 9 bytes, padded to 12. Hand-build it.
        let mut dib = vec![0u8; 40 + 12 + 12];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&3i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&24u16.to_le_bytes());
        // Bottom-up single row: BGR triples.
        dib[40..43].copy_from_slice(&[10, 20, 30]);
        dib[43..46].copy_from_slice(&[40, 50, 60]);
        dib[46..49].copy_from_slice(&[70, 80, 90]);
        let (out, w, h) = decode_dib(&dib).expect("decode");
        assert_eq!((w, h), (3, 1));
        assert_eq!(out[0..4], [10, 20, 30, 255]);
        assert_eq!(out[4..8], [40, 50, 60, 255]);
        assert_eq!(out[8..12], [70, 80, 90, 255]);
    }

    #[test]
    fn dib_rejects_compressed_and_short() {
        let (bgra, w, h) = sample_bgra();
        let mut dib = encode_dib(&bgra, w, h);
        dib[16..20].copy_from_slice(&1u32.to_le_bytes()); // BI_RLE8
        assert!(decode_dib(&dib).is_none());
        assert!(decode_dib(&dib[..10]).is_none());
    }

    #[test]
    fn dib_rejects_absurd_dimensions() {
        // Hostile header: max-i32 w×h claims a payload far past any real
        // clipboard buffer. The payload size is computed with checked math
        // (audit 36–48 §3), so this must come back None — never wrap, never
        // panic, never allocate.
        let mut dib = [0u8; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&i32::MAX.to_le_bytes());
        dib[8..12].copy_from_slice(&i32::MAX.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        assert!(decode_dib(&dib).is_none());
    }

    #[test]
    fn fix15_bgra_round_trip_quantizes_only() {
        let (bgra, w, h) = sample_bgra();
        let src = bgra_to_floatsource(&bgra, w as u32, h as u32, [100, 50], 4096, 4096);
        assert_eq!(src.rect, [100, 50, 102, 51]);
        let (back, bw, bh) = floatsource_to_bgra(&src);
        assert_eq!((bw, bh), (2, 1));
        // Opaque red: exact. Half-alpha blue: ±1 from the double rounding.
        assert_eq!(back[0..4], bgra[0..4]);
        for i in 0..4 {
            assert!(
                (back[4 + i] as i16 - bgra[4 + i] as i16).abs() <= 1,
                "channel {i}: {} vs {}",
                back[4 + i],
                bgra[4 + i]
            );
        }
    }

    #[test]
    fn floatsource_off_canvas_is_empty_but_sane() {
        let (bgra, w, h) = sample_bgra();
        let src = bgra_to_floatsource(&bgra, w as u32, h as u32, [-10, -10], 64, 64);
        assert!(src.tiles.is_empty());
        assert!(src.rect[0] >= src.rect[2] || src.rect[1] >= src.rect[3]);
    }
}
