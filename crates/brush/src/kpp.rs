//! Krita `.kpp` brush-preset reader — the preset XML, nothing else.
//!
//! A `.kpp` is not a bespoke container: it is an ordinary PNG (the thumbnail
//! Krita shows in the brush docker) carrying the preset as a **PNG text
//! chunk** with the keyword `preset`. Krita writes it in
//! `KisPaintOpPreset::saveToDevice` (`libs/image/brushengine/kis_paintop_preset.cpp`,
//! GPL-2.0-or-later, Boudewijn Rempt et al.) via `KoStore`/QImage text
//! metadata; the payload is `KisPaintOpPreset::toXML`'s document:
//!
//! ```xml
//! <Preset paintopid="paintbrush" name="Ink_gpen">
//!   <param name="BrushSize" type="internal">5.0</param>
//!   ...
//! </Preset>
//! ```
//!
//! Container walk (PNG spec, ISO/IEC 15948 §5): 8-byte signature, then chunks
//! of `u32 length` (big-endian), 4-byte type, `length` bytes of data, `u32`
//! CRC. We walk it by hand — no PNG decoder, no zlib — and deliberately do
//! **not** verify the CRCs: we are reading metadata out of a file the user
//! already trusts enough to open, and a stale CRC must not cost them a brush.
//!
//! Text chunks come in three flavours; we read the two that are stored plain:
//!
//! - `tEXt` — `keyword\0text` (Latin-1 per spec; Krita writes UTF-8, so we
//!   try UTF-8 first and fall back to Latin-1 byte→char).
//! - `iTXt` — `keyword\0 flag method lang\0 translated\0 text` (UTF-8), read
//!   only when the compression flag is 0.
//! - `zTXt`, and `iTXt` with the flag set, are zlib-deflated. Inflate is a
//!   dependency this crate does not carry, so a `preset` stored that way is a
//!   clear `Err` naming the variant rather than a silent "no preset found".
//!
//! **For the importer:** what comes out of here is dynamics — sizes, opacity,
//! flow, spacing and the curve blobs. The brush TIP usually is **not** in this
//! file: the paintop settings reference it by resource name
//! (`requiredBrushFile`, or a brush definition nested in the paintop's own
//! serialized settings), pointing at a `.kpp`-adjacent `.gbr`/`.png` in the
//! user's Krita resource folder. An import therefore maps dynamics only and
//! keeps our own tip — an honest limitation for the importer to surface in
//! its UI. This module just parses.

use std::collections::BTreeMap;
use std::path::Path;

/// A parsed Krita preset: the XML parameter map from the PNG's `preset` text
/// chunk. Mapping onto engine settings happens at the importer, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KppPreset {
    pub name: String,
    /// Krita's paint-op id, e.g. `paintbrush`.
    pub paintop_id: String,
    /// Raw `<param name>` → value strings, exactly as stored.
    pub params: BTreeMap<String, String>,
}

const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
/// The PNG text keyword Krita stores the preset document under.
const PRESET_KEYWORD: &[u8] = b"preset";

/// Parse a `.kpp` (PNG + `preset` text chunk) into its preset XML values.
///
/// Only structural problems are an `Err`: not a PNG, a chunk length past EOF,
/// a `preset` chunk that is compressed, no `preset` chunk at all, or XML that
/// has no usable `<Preset>` element. Unknown chunks and unknown attributes are
/// skipped, and params are taken verbatim — deciding which of Krita's ~200
/// setting ids mean anything to us is the importer's job.
pub fn parse_kpp(bytes: &[u8]) -> Result<KppPreset, String> {
    let xml = preset_chunk(bytes)?;
    parse_preset_xml(&xml)
}

/// Parse a `.kpp` file from disk. Convenience wrapper over [`parse_kpp`].
pub fn parse_kpp_file(path: &Path) -> Result<KppPreset, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("kpp: {}: {e}", path.display()))?;
    parse_kpp(&bytes)
}

/// Walk the PNG container and hand back the `preset` chunk's text.
fn preset_chunk(bytes: &[u8]) -> Result<String, String> {
    let mut r = Reader::new(bytes);
    if r.take(8)? != &PNG_SIG[..] {
        return Err("kpp: not a PNG (bad signature)".into());
    }
    // A compressed `preset` seen along the way: remembered, not raised on the
    // spot, so a file that also carries a plain copy still parses.
    let mut compressed: Option<&str> = None;
    while r.left() > 0 {
        let len = r.u32()? as usize;
        let kind = r.tag4()?;
        let data = r.take(len)?;
        r.skip(4)?; // CRC: walked past, never verified (see module doc)
        match &kind {
            b"tEXt" => {
                if let Some((kw, text)) = split_nul(data) {
                    if kw == PRESET_KEYWORD {
                        return Ok(text_string(text));
                    }
                }
            }
            b"iTXt" => {
                // keyword \0 compression_flag compression_method
                //   language_tag \0 translated_keyword \0 text
                let Some((kw, rest)) = split_nul(data) else {
                    continue;
                };
                if kw != PRESET_KEYWORD || rest.len() < 2 {
                    continue;
                }
                let flag = rest[0];
                let Some((_lang, rest)) = split_nul(&rest[2..]) else {
                    continue;
                };
                let Some((_translated, text)) = split_nul(rest) else {
                    continue;
                };
                if flag != 0 {
                    compressed.get_or_insert("a compressed iTXt");
                    continue;
                }
                return Ok(text_string(text));
            }
            b"zTXt" => {
                if let Some((kw, _)) = split_nul(data) {
                    if kw == PRESET_KEYWORD {
                        compressed.get_or_insert("zTXt");
                    }
                }
            }
            b"IEND" => break,
            _ => {}
        }
    }
    match compressed {
        Some(v) => Err(format!(
            "kpp: the `preset` text chunk is stored as {v} (zlib-deflated); \
             this reader carries no inflate and cannot read it"
        )),
        None => Err("kpp: no `preset` text chunk in the PNG".into()),
    }
}

/// `keyword\0rest` split used by all three PNG text chunk layouts.
fn split_nul(b: &[u8]) -> Option<(&[u8], &[u8])> {
    let i = b.iter().position(|&c| c == 0)?;
    Some((&b[..i], &b[i + 1..]))
}

/// PNG text is Latin-1 by spec but UTF-8 in practice (Krita, and every other
/// Qt writer). Try UTF-8, fall back to the spec's byte→char mapping.
fn text_string(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

// ---------------------------------------------------------------------------
// XML: a hand-rolled extractor for the one fixed shape Krita writes.
// ---------------------------------------------------------------------------

fn parse_preset_xml(xml: &str) -> Result<KppPreset, String> {
    let (attrs, body, self_closed) = find_element(xml, 0, "Preset")
        .ok_or_else(|| "kpp: no <Preset> element in the preset XML".to_string())?;
    let name = attr(attrs, "name")
        .ok_or_else(|| "kpp: <Preset> has no name attribute".to_string())?;
    let paintop_id = attr(attrs, "paintopid")
        .ok_or_else(|| "kpp: <Preset> has no paintopid attribute".to_string())?;

    let mut params = BTreeMap::new();
    if !self_closed {
        // Bound the param scan to this element so a second document appended
        // after `</Preset>` cannot bleed its params into ours.
        let end = xml[body..]
            .find("</Preset>")
            .map_or(xml.len(), |d| body + d);
        let inner = &xml[..end];
        let mut i = body;
        while let Some((pattrs, after, closed)) = find_element(inner, i, "param") {
            let key = attr(pattrs, "name");
            let mut j = after;
            let value = if closed {
                String::new()
            } else {
                read_param_text(inner, &mut j)?
            };
            if let Some(k) = key {
                params.insert(k, value);
            }
            i = j;
        }
    }
    Ok(KppPreset {
        name,
        paintop_id,
        params,
    })
}

/// Find the next `<name ...>` at or after `from`. Returns the attribute text,
/// the index just past the `>`, and whether the tag closed itself (`/>`).
fn find_element<'a>(s: &'a str, from: usize, name: &str) -> Option<(&'a str, usize, bool)> {
    let mut i = from;
    loop {
        let start = i + s.get(i..)?.find('<')?;
        let after = start + 1;
        if let Some(tail) = s.get(after..)?.strip_prefix(name) {
            if tail.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
                return scan_tag(s, after + name.len());
            }
        }
        i = after;
    }
}

/// Scan from just past a tag's name to its closing `>`, skipping over quoted
/// attribute values (XML escapes `"` inside them, so this is sound).
fn scan_tag(s: &str, p: usize) -> Option<(&str, usize, bool)> {
    let b = s.as_bytes();
    let mut quote = 0u8;
    for i in p..b.len() {
        let c = b[i];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            let attrs = s[p..i].trim_end();
            let closed = attrs.ends_with('/');
            return Some((attrs.trim_end_matches('/'), i + 1, closed));
        }
    }
    None
}

/// One attribute value, unescaped. `None` if the attribute is absent.
fn attr(attrs: &str, key: &str) -> Option<String> {
    let b = attrs.as_bytes();
    let ws = |c: u8| c.is_ascii_whitespace();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && ws(b[i]) {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != b'=' && !ws(b[i]) {
            i += 1;
        }
        if ks == i {
            return None; // no name here: malformed tail, stop
        }
        let name = &attrs[ks..i];
        while i < b.len() && ws(b[i]) {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            continue; // valueless attribute; `i` already advanced past it
        }
        i += 1;
        while i < b.len() && ws(b[i]) {
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
        let q = b[i];
        let value = if q == b'"' || q == b'\'' {
            i += 1;
            let vs = i;
            while i < b.len() && b[i] != q {
                i += 1;
            }
            let v = &attrs[vs..i];
            i += 1; // past the closing quote (or harmlessly past the end)
            v
        } else {
            let vs = i;
            while i < b.len() && !ws(b[i]) {
                i += 1;
            }
            &attrs[vs..i]
        };
        if name == key {
            return Some(unescape(value));
        }
    }
    None
}

/// Read a `<param>`'s content, advancing `i` past the `</param>`.
///
/// Text runs are unescaped exactly ONE level: Krita stores whole curve/XML
/// blobs in params, and the caller wants that blob raw (`<curve …>`), not a
/// second parse of it. CDATA sections (newer presets) are copied verbatim.
fn read_param_text(s: &str, i: &mut usize) -> Result<String, String> {
    let mut out = String::new();
    loop {
        let rest = s
            .get(*i..)
            .filter(|r| !r.is_empty())
            .ok_or_else(|| "kpp: unterminated <param>".to_string())?;
        if let Some(cdata) = rest.strip_prefix("<![CDATA[") {
            let end = cdata
                .find("]]>")
                .ok_or_else(|| "kpp: unterminated CDATA in <param>".to_string())?;
            out.push_str(&cdata[..end]);
            *i += "<![CDATA[".len() + end + "]]>".len();
        } else if rest.starts_with("</param") {
            let gt = rest
                .find('>')
                .ok_or_else(|| "kpp: unterminated </param>".to_string())?;
            *i += gt + 1;
            return Ok(out);
        } else {
            // Text up to the next `<`, starting the search one char in so a
            // stray `<` that opens nothing we know is taken as literal text
            // and the walk always makes progress.
            let first = rest.chars().next().map_or(1, char::len_utf8);
            let next = rest[first..].find('<').map_or(rest.len(), |d| d + first);
            out.push_str(&unescape(&rest[..next]));
            *i += next;
        }
    }
}

/// XML entity decode, one level. Unknown entities are left verbatim.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let decoded = tail[1..]
            .find(';')
            .and_then(|d| entity(&tail[1..1 + d]).map(|c| (c, d + 2)));
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(e: &str) -> Option<char> {
    match e {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let n = e.strip_prefix('#')?;
            let v = match n.strip_prefix('x').or_else(|| n.strip_prefix('X')) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => n.parse::<u32>().ok()?,
            };
            char::from_u32(v)
        }
    }
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
            return Err(format!("kpp: truncated at byte {} (+{n})", self.pos));
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
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Length-framed chunk with a deliberately bogus CRC — the walk must not
    /// care (see module doc).
    pub(super) fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&(data.len() as u32).to_be_bytes());
        v.extend_from_slice(kind);
        v.extend_from_slice(data);
        v.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        v
    }

    /// Signature + a dummy-but-correctly-framed IHDR + `body` + IEND.
    pub(super) fn png(body: &[u8]) -> Vec<u8> {
        let mut v = PNG_SIG.to_vec();
        v.extend_from_slice(&chunk(b"IHDR", &[0u8; 13]));
        v.extend_from_slice(body);
        v.extend_from_slice(&chunk(b"IEND", &[]));
        v
    }

    pub(super) fn text(kind: &[u8; 4], keyword: &str, tail: &[u8]) -> Vec<u8> {
        let mut d = keyword.as_bytes().to_vec();
        d.push(0);
        d.extend_from_slice(tail);
        chunk(kind, &d)
    }

    /// Three params: a plain value, an escaped XML blob, and a CDATA blob.
    pub(super) const XML: &str = concat!(
        "<?xml version=\"1.0\"?>\n",
        "<Preset paintopid=\"paintbrush\" name=\"Ink_gpen &amp; co\" version=\"5.0\">\n",
        "  <param name=\"BrushSize\" type=\"internal\">5.0</param>\n",
        "  <param name=\"CurveDynamics\" type=\"string\">",
        "&lt;curve name=&quot;pressure&quot;&gt;0,0;1,1&lt;/curve&gt;</param>\n",
        "  <param name=\"Texture/Pattern\" type=\"bytearray\">",
        "<![CDATA[raw <blob> & bytes]]></param>\n",
        "</Preset>\n",
    );

    pub(super) fn good_kpp() -> Vec<u8> {
        png(&text(b"tEXt", "preset", XML.as_bytes()))
    }

    #[test]
    fn parses_name_paintop_and_params() {
        let p = parse_kpp(&good_kpp()).unwrap();
        assert_eq!(p.name, "Ink_gpen & co");
        assert_eq!(p.paintop_id, "paintbrush");
        assert_eq!(p.params.len(), 3);
        assert_eq!(p.params["BrushSize"], "5.0");
        // Exactly ONE level of unescaping: the curve blob stays raw XML text.
        assert_eq!(
            p.params["CurveDynamics"],
            "<curve name=\"pressure\">0,0;1,1</curve>"
        );
        // CDATA is verbatim — `&` and `<` inside are NOT entities.
        assert_eq!(p.params["Texture/Pattern"], "raw <blob> & bytes");
    }

    #[test]
    fn reads_uncompressed_itxt_and_skips_foreign_chunks() {
        // iTXt: keyword \0 flag method lang \0 translated \0 text
        let mut tail = vec![0u8, 0u8];
        tail.extend_from_slice(b"en\0\0");
        tail.extend_from_slice(XML.as_bytes());
        let mut body = chunk(b"pHYs", &[1, 2, 3, 4]);
        body.extend_from_slice(&text(b"tEXt", "Software", b"Krita"));
        body.extend_from_slice(&text(b"iTXt", "preset", &tail));
        let p = parse_kpp(&png(&body)).unwrap();
        assert_eq!(p.paintop_id, "paintbrush");
        assert_eq!(p.params.len(), 3);
    }

    #[test]
    fn self_closing_param_numeric_entities_and_single_quotes() {
        let xml = concat!(
            "<Preset paintopid='sketchbrush' name='A&#65;&#x42;'>",
            "<param name=\"Empty\"/>",
            "<param name=\"Sep\">a&#44;b&unknown;c</param>",
            "</Preset>",
        );
        let p = parse_kpp(&png(&text(b"tEXt", "preset", xml.as_bytes()))).unwrap();
        assert_eq!(p.name, "AAB");
        assert_eq!(p.paintop_id, "sketchbrush");
        assert_eq!(p.params["Empty"], "");
        // An entity we do not know survives verbatim rather than vanishing.
        assert_eq!(p.params["Sep"], "a,b&unknown;c");
    }

    #[test]
    fn missing_preset_chunk_is_err() {
        let e = parse_kpp(&png(&text(b"tEXt", "Title", b"not a preset"))).unwrap_err();
        assert!(e.contains("no `preset`"), "{e}");
    }

    #[test]
    fn compressed_ztxt_preset_is_rejected_by_name() {
        // zTXt: keyword \0 compression_method, then deflated bytes.
        let mut body = text(b"tEXt", "Title", b"x");
        body.extend_from_slice(&text(b"zTXt", "preset", &[0, 0x78, 0x9C, 0x01]));
        let e = parse_kpp(&png(&body)).unwrap_err();
        assert!(e.contains("zTXt"), "{e}");
    }

    #[test]
    fn compressed_itxt_preset_is_rejected_by_name() {
        let mut tail = vec![1u8, 0u8]; // compression flag set
        tail.extend_from_slice(b"en\0\0");
        tail.extend_from_slice(&[0x78, 0x9C, 0x01]);
        let e = parse_kpp(&png(&text(b"iTXt", "preset", &tail))).unwrap_err();
        assert!(e.contains("iTXt"), "{e}");
    }

    #[test]
    fn rejects_non_png_and_unusable_xml() {
        assert!(parse_kpp(b"").is_err());
        assert!(parse_kpp(b"not a png at all").is_err());
        // A PNG whose preset text is not a preset document.
        assert!(parse_kpp(&png(&text(b"tEXt", "preset", b"<Nope/>"))).is_err());
        // <Preset> without the required attributes.
        let e = parse_kpp(&png(&text(b"tEXt", "preset", b"<Preset name=\"x\"/>"))).unwrap_err();
        assert!(e.contains("paintopid"), "{e}");
        // Unterminated param body.
        assert!(
            parse_kpp(&png(&text(
                b"tEXt",
                "preset",
                b"<Preset paintopid=\"p\" name=\"n\"><param name=\"a\">1"
            )))
            .is_err()
        );
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::tests::{XML, good_kpp, png, text};
    use super::*;

    /// The container walk and the XML scan must never panic or hang on
    /// hostile input: cut a good file at every byte, and lie about a chunk
    /// length. Every cut yields Ok or Err, never a panic.
    #[test]
    fn truncation_never_panics() {
        let good = good_kpp();
        for cut in 0..good.len() {
            let _ = parse_kpp(&good[..cut]);
        }
        // A chunk length far past EOF must be a clean Err, not an allocation.
        let mut lie = good.clone();
        let at = PNG_SIG.len();
        lie[at..at + 4].copy_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
        assert!(parse_kpp(&lie).is_err());
        // Every byte cut of the XML itself, inside a well-formed PNG — this
        // is the half of the surface the whole-file cuts never reach, since a
        // short file dies in the container walk before the XML is seen.
        for cut in 0..XML.len() {
            if !XML.is_char_boundary(cut) {
                continue;
            }
            let _ = parse_kpp(&png(&text(b"tEXt", "preset", &XML.as_bytes()[..cut])));
        }
    }
}
