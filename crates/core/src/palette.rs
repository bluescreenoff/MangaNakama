//! `.gpl` (GIMP Palette) parsing — the plain-text swatch format GIMP and
//! Krita both read and write, and the de-facto interchange format for
//! downloadable palettes. MangaNakama imports them into the Color Set.
//!
//! Format:
//!
//! ```text
//! GIMP Palette
//! Name: Skin tones
//! Columns: 4
//! #
//!  48  32  96	Deep purple
//! 255 255 255
//! ```
//!
//! Header lines are `Key: value`; `#` starts a comment; colour lines are
//! three 0..255 integers plus an optional name (spaces allowed). Malformed
//! lines are skipped, not fatal — real-world files carry trailing junk.
//!
//! The hex helpers below live here too: `#rrggbb` is how every palette on
//! the internet is quoted, and one parser serves the Color palette's HEX
//! field, `swatches.txt` and the `.ora` metadata attributes.

/// One Color Set entry: the colour, plus the name its palette file gave it
/// (empty for the built-in set and for colours added with `+` — a name is
/// something a `.gpl` carries, not something we invent).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Swatch {
    pub rgb: [f32; 3],
    pub name: String,
}

impl Swatch {
    /// An unnamed swatch — the common case.
    pub fn new(rgb: [f32; 3]) -> Self {
        Self {
            rgb,
            name: String::new(),
        }
    }
}

/// Parse `#rrggbb`, `rrggbb`, `#rgb` or `rgb` into 0..1 RGB. Case and
/// surrounding whitespace are tolerated; anything else — a wrong length, a
/// non-hex character, an 8-digit `#rrggbbaa` whose alpha we would have to
/// throw away — is `None`, so callers can revert rather than guess at what
/// the user meant.
pub fn parse_hex(s: &str) -> Option<[f32; 3]> {
    let t = s.trim();
    let h = t.strip_prefix('#').unwrap_or(t);
    if (h.len() != 6 && h.len() != 3) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    let [r, g, b] = if h.len() == 6 {
        [(v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff]
    } else {
        // The CSS short form: each digit is doubled, so #f80 == #ff8800.
        [(v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf].map(|d| d * 0x11)
    };
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
}

/// The `#rrggbb` a colour displays and persists as.
pub fn hex_string(rgb: [f32; 3]) -> String {
    let [r, g, b] = to_u8(rgb);
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// A colour rounded to the 8-bit value it displays as. The colour history
/// stores this, so "the same colour" means the same visible swatch rather
/// than two entries differing in the seventh decimal.
pub fn quantize8(rgb: [f32; 3]) -> [f32; 3] {
    to_u8(rgb).map(|v| v as f32 / 255.0)
}

/// 0..1 float channels to 0..255 bytes (out-of-range clamps; NaN lands on 0
/// through Rust's saturating float cast).
pub fn to_u8(rgb: [f32; 3]) -> [u8; 3] {
    rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Parse a `.gpl` into swatches. Errors only when the text contains no
/// colours at all (wrong file, or a palette format we don't know).
pub fn parse_gpl(text: &str) -> Result<Vec<Swatch>, String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line == "GIMP Palette" {
            continue;
        }
        // Header fields ("Name:", "Columns:", "Comment:") fail the
        // three-int parse below and are skipped with the other junk.
        let mut parts = line.split_whitespace();
        let (Some(r), Some(g), Some(b)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(r), Ok(g), Ok(b)) = (r.parse::<u8>(), g.parse::<u8>(), b.parse::<u8>()) else {
            continue;
        };
        let name = parts.collect::<Vec<_>>().join(" ");
        out.push(Swatch {
            rgb: [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0],
            name,
        });
    }
    if out.is_empty() {
        return Err("no colours found in .gpl".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_krita_style_files() {
        let text = "GIMP Palette\nName: Test\nColumns: 4\n#\n\
                    48  32  96\tDeep purple\n255 255 255\n  0   0   0 Black\n";
        let cols = parse_gpl(text).unwrap();
        assert_eq!(cols.len(), 3);
        assert!((cols[0].rgb[0] - 48.0 / 255.0).abs() < 1e-6);
        assert_eq!(cols[0].name, "Deep purple");
        assert_eq!(cols[1].name, "", "unnamed colour has empty name");
        assert_eq!(cols[2].name, "Black");
    }

    #[test]
    fn junk_lines_are_skipped_not_fatal() {
        let text = "GIMP Palette\nName: x\nColumns: bananas\nhello world\n\
                    1 2 3\nnot a color\n255 0 0 red-ish";
        let cols = parse_gpl(text).unwrap();
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[1].name, "red-ish");
    }

    #[test]
    fn no_colours_is_an_error() {
        assert!(parse_gpl("GIMP Palette\nName: empty\n#\n").is_err());
        assert!(parse_gpl("").is_err());
    }

    #[test]
    fn out_of_range_channels_are_skipped() {
        // 256 is not a u8; the line is junk, not a colour.
        assert!(parse_gpl("300 0 0 nope").is_err());
        assert_eq!(parse_gpl("300 0 0 nope\n1 2 3 ok").unwrap().len(), 1);
    }

    /// The four spellings a user actually types or pastes into a HEX field.
    #[test]
    fn hex_accepts_the_four_spellings() {
        let red = [1.0, 0.0, 0.0];
        for s in ["#ff0000", "ff0000", "#FF0000", "  #f00  ", "F00"] {
            assert_eq!(parse_hex(s), Some(red), "{s}");
        }
        // The short form doubles each digit — #f80 is #ff8800, not #f08000.
        assert_eq!(parse_hex("#f80"), parse_hex("#ff8800"));
    }

    /// Anything we cannot read exactly is `None`, so the field reverts
    /// instead of clamping to a colour the user did not ask for.
    #[test]
    fn hex_rejects_what_it_cannot_read_exactly() {
        for s in [
            "",
            "#",
            "#ff00",
            "#ff00000",
            "#gggggg",
            "#ff0000ff", // alpha we'd have to drop
            "#ff 00 00",
            "rgb(1,2,3)",
            "#+f0000",
            "0x00ff00",
        ] {
            assert_eq!(parse_hex(s), None, "{s} must not parse");
        }
    }

    #[test]
    fn hex_string_round_trips_and_quantize_is_idempotent() {
        for hex in ["#000000", "#ffffff", "#4f8cd2", "#0c0c0c"] {
            let rgb = parse_hex(hex).unwrap();
            assert_eq!(hex_string(rgb), hex);
            assert_eq!(quantize8(rgb), rgb, "already 8-bit exact");
        }
        // Off-grid floats snap to the nearest displayable value, and stay.
        let odd = [0.5001, 0.123_456, 0.999_9];
        let q = quantize8(odd);
        assert_eq!(quantize8(q), q);
        assert_eq!(hex_string(odd), hex_string(q));
        // Out of range clamps rather than wrapping.
        assert_eq!(hex_string([-1.0, 2.0, 0.0]), "#00ff00");
    }
}
