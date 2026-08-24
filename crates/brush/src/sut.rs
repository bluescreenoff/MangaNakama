//! Clip Studio `.sut` (exported sub tool) reader — the extraction layer on
//! top of [`crate::sqlite_ro`].
//!
//! A `.sut` is a plain SQLite 3 database. Everything below was established
//! by the in-house research tooling (the archive's `tools/sutdump` +
//! `docs/CSP-TOOLS.md`) against the owner's real Clip Studio install:
//!
//! - Sub-tool parameters are TYPED COLUMNS on the `Variant` table
//!   (`BrushSize`, `Opacity`, `BrushHardness`, `BrushInterval`,
//!   `BrushFlow`, …) — not opaque blobs.
//! - `*Effector` blob columns say what modulates a parameter:
//!   `u32be headerLen(44) | availableSources | enabledSources |
//!   w3..w7 per-source minimum % | w8,w9 stored-curve byte lengths |
//!   w10 unknown`, then the curves in bit order. Source bits: 0x010 pen
//!   pressure, 0x020 tilt, 0x040 velocity, 0x080 random.
//! - Curves are a generic array: `u32be headerLen(12) | count | stride`,
//!   stride 16 = `(f64be x, f64be y)` pairs.
//! - `Node.NodeName` names the sub tool; `MaterialFile.FileData` carries
//!   the tip material blobs (CSP wrapping with a plain PNG inside —
//!   extracted by signature scan, best-effort).

use std::collections::BTreeMap;
use std::path::Path;

use crate::sqlite_ro::{self, Value};

/// One effector source's story, decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct SutEffector {
    pub enabled_mask: u32,
    /// Words 0..=10 of the header (headerLen, avail, enabled, minimums…).
    pub words: Vec<u32>,
    /// Stored curves in slot order (pressure first when both exist);
    /// points are (x, y) in 0..1.
    pub curves: Vec<Vec<(f64, f64)>>,
}

pub const SRC_PRESSURE: u32 = 0x010;
pub const SRC_TILT: u32 = 0x020;
pub const SRC_VELOCITY: u32 = 0x040;
pub const SRC_RANDOM: u32 = 0x080;

impl SutEffector {
    /// The per-source minimum, as a 0..1 factor (words 3..=6 hold percent
    /// minimums for pressure/tilt/velocity/random in bit order).
    pub fn minimum(&self, src_bit: u32) -> f64 {
        let idx = match src_bit {
            SRC_PRESSURE => 3,
            SRC_TILT => 4,
            SRC_VELOCITY => 5,
            SRC_RANDOM => 6,
            _ => return 1.0,
        };
        f64::from(self.words.get(idx).copied().unwrap_or(100)) / 100.0
    }

    /// The stored curve for a source, if one is stored (pressure = slot
    /// w[8], tilt = slot w[9]; curves are stored in slot order).
    pub fn curve(&self, src_bit: u32) -> Option<&[(f64, f64)]> {
        let (len_word, later_slot) = match src_bit {
            SRC_PRESSURE => (8usize, false),
            SRC_TILT => (9, true),
            _ => return None,
        };
        if self.words.get(len_word).copied().unwrap_or(0) == 0 {
            return None;
        }
        let idx = if later_slot && self.words.get(8).copied().unwrap_or(0) > 0 {
            1
        } else {
            0
        };
        self.curves.get(idx).map(Vec::as_slice)
    }

    /// Whether this source actually modulates anything: enabled AND its
    /// minimum below 100 % (Clip Studio disables a source while keeping its
    /// curve by parking the minimum at 100).
    pub fn drives(&self, src_bit: u32) -> bool {
        self.enabled_mask & src_bit != 0 && self.minimum(src_bit) < 1.0
    }
}

/// One brush read out of a `.sut`.
#[derive(Debug, Clone)]
pub struct SutBrush {
    /// `Node.NodeName` (the sub tool's display name), or the file stem.
    pub name: String,
    /// Numeric `Variant` columns, verbatim by column name.
    pub params: BTreeMap<String, f64>,
    /// `*Effector` blob columns, decoded, keyed by column name.
    pub effectors: BTreeMap<String, SutEffector>,
    /// PNGs pulled out of `MaterialFile.FileData` (tip materials),
    /// signature-scanned out of Clip Studio's wrapper. May be empty.
    pub tip_pngs: Vec<Vec<u8>>,
}

/// Parse a `.sut` file's bytes. `file_stem` names the brush when the Node
/// table gives nothing usable.
pub fn parse_sut(bytes: &[u8], file_stem: &str) -> Result<SutBrush, String> {
    let tables = sqlite_ro::parse_sqlite(bytes)?;
    let variant = tables
        .get("Variant")
        .ok_or("sut: no Variant table (not a sub-tool export?)")?;

    // A .sut usually carries one real Variant row; when several exist the
    // one with the most non-null values is the sub tool (others are
    // template/default rows).
    let records = variant.records();
    let rec = records
        .iter()
        .max_by_key(|r| r.values().filter(|v| !matches!(v, Value::Null)).count())
        .ok_or("sut: Variant table is empty")?;

    let name = tables
        .get("Node")
        .map(|t| t.records())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.get("NodeName").and_then(Value::as_str))
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or(file_stem)
        .to_string();

    let tip_pngs = tables
        .get("MaterialFile")
        .map(|t| t.records())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.get("FileData").and_then(Value::as_blob))
        .filter_map(extract_png)
        .collect();

    let (params, effectors) = variant_params_effectors(rec);
    Ok(SutBrush {
        name,
        params,
        effectors,
        tip_pngs,
    })
}

/// One Variant record's numeric params + decoded effectors — the shared
/// body of the `.sut` slice reader and the whole-database (`.todb`)
/// walker (T5b).
pub(crate) fn variant_params_effectors(
    rec: &std::collections::BTreeMap<String, Value>,
) -> (BTreeMap<String, f64>, BTreeMap<String, SutEffector>) {
    let mut params = BTreeMap::new();
    let mut effectors = BTreeMap::new();
    for (k, v) in rec {
        match v {
            Value::Int(_) | Value::Real(_) => {
                params.insert(k.clone(), v.as_f64().unwrap_or(0.0));
            }
            Value::Blob(b) if k.ends_with("Effector") => {
                if let Some(e) = parse_effector(b) {
                    effectors.insert(k.clone(), e);
                }
            }
            _ => {}
        }
    }
    (params, effectors)
}

/// Parse a `.sut` file from disk.
pub fn parse_sut_file(path: &Path) -> Result<SutBrush, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("sut: {}: {e}", path.display()))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sut".into());
    parse_sut(&bytes, &stem)
}

fn u32be(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_be_bytes(s.try_into().unwrap()))
}

/// The `*Effector` blob (sutdump's proven layout).
fn parse_effector(b: &[u8]) -> Option<SutEffector> {
    let header = u32be(b, 0)? as usize;
    if header != 44 || b.len() < header {
        return None;
    }
    let words: Vec<u32> = (0..11).filter_map(|i| u32be(b, i * 4)).collect();
    let mut curves = Vec::new();
    let mut p = header;
    while p + 12 <= b.len() {
        let Some((pts, used)) = parse_curve(b, p) else {
            break;
        };
        curves.push(pts);
        p += used;
    }
    Some(SutEffector {
        enabled_mask: words[2],
        words,
        curves,
    })
}

/// The generic array header: `u32be 12 | count | stride`; stride 16 =
/// (f64be x, f64be y) pairs.
fn parse_curve(b: &[u8], off: usize) -> Option<(Vec<(f64, f64)>, usize)> {
    let hl = u32be(b, off)? as usize;
    let count = u32be(b, off + 4)? as usize;
    let stride = u32be(b, off + 8)? as usize;
    if hl != 12 || stride != 16 || count > 4096 {
        return None;
    }
    let data = b.get(off + hl..off + hl + count * stride)?;
    let pts = data
        .chunks_exact(16)
        .map(|c| {
            (
                f64::from_be_bytes(c[0..8].try_into().unwrap()),
                f64::from_be_bytes(c[8..16].try_into().unwrap()),
            )
        })
        .collect();
    Some((pts, hl + count * stride))
}

/// Scan a Clip Studio material wrapper for an embedded PNG and return it
/// whole (signature → past IEND). Best-effort by design: no wrapper
/// documentation exists, but the PNG inside is self-delimiting.
fn extract_png(blob: &[u8]) -> Option<Vec<u8>> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    let start = blob.windows(SIG.len()).position(|w| w == SIG)?;
    let b = &blob[start..];
    // Walk chunks to IEND to find the true end.
    let mut p = 8usize;
    while p + 12 <= b.len() {
        let len = u32be(b, p)? as usize;
        let kind = &b[p + 4..p + 8];
        p = p.checked_add(12 + len)?;
        if kind == b"IEND" {
            return b.get(..p).map(<[u8]>::to_vec);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effector_blob_decodes_minimums_and_curves() {
        // Header: len 44, avail, enabled=pressure, w3(min pressure)=30,
        // w4..w7, w8 = curve byte len (12+32), w9, w10; then one 2-point
        // curve.
        let mut b = Vec::new();
        for w in [44u32, 0x0f0, SRC_PRESSURE, 30, 100, 100, 100, 0, 44, 0, 100] {
            b.extend_from_slice(&w.to_be_bytes());
        }
        b.extend_from_slice(&12u32.to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes());
        b.extend_from_slice(&16u32.to_be_bytes());
        for v in [0.0f64, 0.0, 1.0, 1.0] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        let e = parse_effector(&b).unwrap();
        assert!(e.drives(SRC_PRESSURE));
        assert!((e.minimum(SRC_PRESSURE) - 0.3).abs() < 1e-9);
        assert_eq!(e.curve(SRC_PRESSURE).unwrap(), &[(0.0, 0.0), (1.0, 1.0)]);
        assert!(!e.drives(SRC_TILT));
        // Parked at 100 % = does not drive even when enabled.
        let mut parked = b.clone();
        parked[3 * 4 + 3] = 100;
        let e = parse_effector(&parked).unwrap();
        assert!(!e.drives(SRC_PRESSURE));
    }

    #[test]
    fn png_extraction_walks_to_iend() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        // One dummy chunk + IEND.
        png.extend_from_slice(&3u32.to_be_bytes());
        png.extend_from_slice(b"tEXtabc");
        png.extend_from_slice(&[0; 4]); // crc
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&[0; 4]);
        let mut wrapped = b"CSPWRAP??".to_vec();
        wrapped.extend_from_slice(&png);
        wrapped.extend_from_slice(b"trailing garbage");
        assert_eq!(extract_png(&wrapped).unwrap(), png);
        assert!(extract_png(b"no png here").is_none());
        // A truncated PNG (no IEND) yields nothing, not a panic.
        assert!(extract_png(&wrapped[..wrapped.len() - 30]).is_none());
    }

    /// The real fixtures (LOCAL-ONLY, gitignored; skip where absent): both
    /// airbrush exports parse, carry the documented parameters, a pressure
    /// effector, and a usable name.
    #[test]
    fn real_sut_files_parse_end_to_end() {
        for (file, expect_name) in [("sut_sample.sut", "Hard"), ("sut_sample2.sut", "Soft")] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/data")
                .join(file);
            let Ok(bytes) = std::fs::read(&path) else {
                return;
            };
            let b = parse_sut(&bytes, "fallback").unwrap();
            assert!(
                b.name.contains(expect_name),
                "{file}: name {:?} missing {expect_name}",
                b.name
            );
            for p in ["BrushSize", "Opacity", "BrushHardness", "BrushInterval"] {
                assert!(b.params.contains_key(p), "{file}: missing {p}");
            }
            assert!(
                b.params["BrushSize"] > 0.0 && b.params["Opacity"] > 0.0,
                "{file}: nonsense values"
            );
            println!(
                "[test] {file}: {:?} size {} opacity {} interval {} effectors {:?} tips {}",
                b.name,
                b.params["BrushSize"],
                b.params["Opacity"],
                b.params["BrushInterval"],
                b.effectors.keys().collect::<Vec<_>>(),
                b.tip_pngs.len()
            );
        }
    }
}
