//! Minimal READ-ONLY SQLite reader — just enough to walk the table b-trees
//! of a well-formed database file. Written for Clip Studio's `.sut` brush
//! files (plain SQLite 3 databases); no dependency, no write path, no SQL.
//!
//! Scope, deliberately: the file header, table b-trees (interior `0x05` +
//! leaf `0x0D` pages), the record format (all serial types), and overflow
//! chains. NOT here: indexes, WAL, freelists, pointer maps, encodings other
//! than UTF-8 — a `.sut` uses none of them, and a file that somehow does
//! simply yields fewer rows or a clean `Err`, never UB (every read is
//! bounds-checked slicing).
//!
//! Format reference: sqlite.org/fileformat2.html. The overflow split is the
//! spec's exact arithmetic: `usable = page_size - reserved`, a table leaf
//! keeps `maxLocal = usable - 35`; when the payload is larger it keeps
//! `minLocal + (payload - minLocal) % (usable - 4)` if that lands in
//! `minLocal..=maxLocal`, else `minLocal`, where
//! `minLocal = (usable - 12) * 32 / 255 - 23`.

use std::collections::BTreeMap;

/// One decoded column value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            Value::Real(v) => Some(*v as i64),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(v) => Some(*v as f64),
            Value::Real(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_blob(&self) -> Option<&[u8]> {
        match self {
            Value::Blob(b) => Some(b),
            _ => None,
        }
    }
}

/// One table: its DDL (for column names) and decoded rows in rowid order.
#[derive(Debug, Clone)]
pub struct Table {
    pub sql: String,
    pub rows: Vec<Vec<Value>>,
}

impl Table {
    /// Column names parsed from the `CREATE TABLE` DDL: the first word of
    /// each top-level comma-separated definition. Good enough for machine
    /// written DDL (Clip Studio's is); table constraints (`PRIMARY KEY(...)`
    /// etc.) start with a keyword and are filtered by upper-case check.
    pub fn columns(&self) -> Vec<String> {
        let Some(open) = self.sql.find('(') else {
            return Vec::new();
        };
        let body = &self.sql[open + 1..self.sql.rfind(')').unwrap_or(self.sql.len())];
        let mut cols = Vec::new();
        let mut depth = 0usize;
        let mut item = String::new();
        for c in body.chars() {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    push_col(&mut cols, &item);
                    item.clear();
                    continue;
                }
                _ => {}
            }
            item.push(c);
        }
        push_col(&mut cols, &item);
        cols
    }

    /// Rows as name → value maps (the walker's friendly face).
    pub fn records(&self) -> Vec<BTreeMap<String, Value>> {
        let cols = self.columns();
        self.rows
            .iter()
            .map(|r| {
                cols.iter()
                    .cloned()
                    .zip(r.iter().cloned())
                    .collect::<BTreeMap<_, _>>()
            })
            .collect()
    }
}

fn push_col(cols: &mut Vec<String>, item: &str) {
    let name = item
        .trim()
        .trim_matches('"')
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches('"')
        .to_string();
    // Table-level constraints start with an (upper-case) keyword.
    const KEYWORDS: [&str; 5] = ["PRIMARY", "UNIQUE", "CHECK", "FOREIGN", "CONSTRAINT"];
    if !name.is_empty() && !KEYWORDS.contains(&name.to_ascii_uppercase().as_str()) {
        cols.push(name);
    }
}

/// Parse a whole SQLite file into its named tables. Structural corruption is
/// an `Err`; an empty or unknown table is simply absent/empty.
pub fn parse_sqlite(bytes: &[u8]) -> Result<BTreeMap<String, Table>, String> {
    let db = Db::new(bytes)?;
    // sqlite_master lives at page 1: rows are (type, name, tbl_name,
    // rootpage, sql).
    let master = db.walk_table(1)?;
    let mut out = BTreeMap::new();
    for row in master {
        let (Some(kind), Some(name), Some(root), Some(sql)) = (
            row.first().and_then(Value::as_str),
            row.get(1).and_then(Value::as_str).map(str::to_string),
            row.get(3).and_then(Value::as_i64),
            row.get(4).and_then(Value::as_str).map(str::to_string),
        ) else {
            continue;
        };
        if kind != "table" || root <= 0 {
            continue;
        }
        let rows = db.walk_table(root as u32)?;
        out.insert(name, Table { sql, rows });
    }
    Ok(out)
}

struct Db<'a> {
    b: &'a [u8],
    page_size: usize,
    usable: usize,
}

impl<'a> Db<'a> {
    fn new(b: &'a [u8]) -> Result<Self, String> {
        if b.len() < 100 || &b[0..16] != b"SQLite format 3\0" {
            return Err("sut: not an SQLite 3 file".into());
        }
        let raw = u16::from_be_bytes([b[16], b[17]]) as usize;
        let page_size = if raw == 1 { 65536 } else { raw };
        if !(512..=65536).contains(&page_size) || !page_size.is_power_of_two() {
            return Err(format!("sut: bad page size {page_size}"));
        }
        let reserved = b[20] as usize;
        let usable = page_size
            .checked_sub(reserved)
            .filter(|u| *u >= 480)
            .ok_or("sut: bad reserved space")?;
        Ok(Db {
            b,
            page_size,
            usable,
        })
    }

    fn page(&self, no: u32) -> Result<&'a [u8], String> {
        let no = no as usize;
        if no == 0 {
            return Err("sut: page 0 referenced".into());
        }
        let start = (no - 1) * self.page_size;
        self.b
            .get(start..start + self.page_size)
            .ok_or_else(|| format!("sut: page {no} past EOF"))
    }

    /// Walk one table b-tree, depth-first, decoding every leaf cell.
    fn walk_table(&self, root: u32) -> Result<Vec<Vec<Value>>, String> {
        let mut rows = Vec::new();
        // Explicit stack, bounded: a cycle in a corrupt file must not hang.
        let mut stack = vec![root];
        let mut visited = 0usize;
        let max_pages = self.b.len() / self.page_size + 1;
        while let Some(no) = stack.pop() {
            visited += 1;
            if visited > max_pages {
                return Err("sut: b-tree walk exceeds the page count (cycle?)".into());
            }
            let page = self.page(no)?;
            // Page 1 carries the 100-byte file header before its page header.
            let hoff = if no == 1 { 100 } else { 0 };
            let typ = *page.get(hoff).ok_or("sut: truncated page header")?;
            let ncell =
                u16::from_be_bytes([page[hoff + 3], page[hoff + 4]]) as usize;
            match typ {
                0x05 => {
                    // Interior: right-most pointer + per-cell child pointers.
                    let right = u32::from_be_bytes([
                        page[hoff + 8],
                        page[hoff + 9],
                        page[hoff + 10],
                        page[hoff + 11],
                    ]);
                    stack.push(right);
                    let cells = hoff + 12;
                    for i in 0..ncell {
                        let off = cells + i * 2;
                        let ptr = u16::from_be_bytes([page[off], page[off + 1]]) as usize;
                        let cell = page.get(ptr..).ok_or("sut: cell pointer past page")?;
                        if cell.len() < 4 {
                            return Err("sut: short interior cell".into());
                        }
                        stack.push(u32::from_be_bytes([cell[0], cell[1], cell[2], cell[3]]));
                    }
                }
                0x0D => {
                    let cells = hoff + 8;
                    for i in 0..ncell {
                        let off = cells + i * 2;
                        let ptr = u16::from_be_bytes([page[off], page[off + 1]]) as usize;
                        let cell = page.get(ptr..).ok_or("sut: cell pointer past page")?;
                        rows.push(self.leaf_cell(cell)?);
                    }
                }
                t => return Err(format!("sut: unexpected page type {t:#x}")),
            }
        }
        Ok(rows)
    }

    /// One table-leaf cell: payload length, rowid, payload (+ overflow).
    fn leaf_cell(&self, cell: &[u8]) -> Result<Vec<Value>, String> {
        let mut p = 0usize;
        let payload_len = varint(cell, &mut p)? as usize;
        let _rowid = varint(cell, &mut p)?;
        // The spec's local/overflow split.
        let max_local = self.usable - 35;
        let payload = if payload_len <= max_local {
            let local = cell
                .get(p..p + payload_len)
                .ok_or("sut: payload past cell")?;
            local.to_vec()
        } else {
            let min_local = (self.usable - 12) * 32 / 255 - 23;
            let k = min_local + (payload_len - min_local) % (self.usable - 4);
            let local_len = if k <= max_local { k } else { min_local };
            let local = cell
                .get(p..p + local_len)
                .ok_or("sut: local payload past cell")?;
            let mut buf = local.to_vec();
            let ovp = cell
                .get(p + local_len..p + local_len + 4)
                .ok_or("sut: missing overflow pointer")?;
            let mut next = u32::from_be_bytes([ovp[0], ovp[1], ovp[2], ovp[3]]);
            let mut guard = 0usize;
            while next != 0 && buf.len() < payload_len {
                guard += 1;
                if guard > self.b.len() / self.page_size + 1 {
                    return Err("sut: overflow chain cycle".into());
                }
                let page = self.page(next)?;
                next = u32::from_be_bytes([page[0], page[1], page[2], page[3]]);
                let want = payload_len - buf.len();
                let chunk = &page[4..4 + want.min(self.usable - 4)];
                buf.extend_from_slice(chunk);
            }
            if buf.len() < payload_len {
                return Err("sut: overflow chain short".into());
            }
            buf
        };
        decode_record(&payload)
    }
}

/// SQLite record format: a varint header length, serial types, then values.
fn decode_record(b: &[u8]) -> Result<Vec<Value>, String> {
    let mut hp = 0usize;
    let header_len = varint(b, &mut hp)? as usize;
    if header_len > b.len() {
        return Err("sut: record header past payload".into());
    }
    let mut types = Vec::new();
    while hp < header_len {
        types.push(varint(b, &mut hp)?);
    }
    let mut vp = header_len;
    let mut out = Vec::with_capacity(types.len());
    for t in types {
        let take = |n: usize, vp: &mut usize| -> Result<&[u8], String> {
            let s = b.get(*vp..*vp + n).ok_or("sut: value past payload")?;
            *vp += n;
            Ok(s)
        };
        let be_int = |s: &[u8]| -> i64 {
            let mut v = if s.first().is_some_and(|c| c & 0x80 != 0) {
                -1i64
            } else {
                0
            };
            for &c in s {
                v = (v << 8) | i64::from(c);
            }
            v
        };
        out.push(match t {
            0 => Value::Null,
            1..=4 => Value::Int(be_int(take(t as usize, &mut vp)?)),
            5 => Value::Int(be_int(take(6, &mut vp)?)),
            6 => Value::Int(be_int(take(8, &mut vp)?)),
            7 => Value::Real(f64::from_be_bytes(
                take(8, &mut vp)?.try_into().unwrap(),
            )),
            8 => Value::Int(0),
            9 => Value::Int(1),
            t if t >= 12 && t % 2 == 0 => {
                Value::Blob(take((t as usize - 12) / 2, &mut vp)?.to_vec())
            }
            t if t >= 13 => Value::Text(
                String::from_utf8_lossy(take((t as usize - 13) / 2, &mut vp)?).into_owned(),
            ),
            t => return Err(format!("sut: reserved serial type {t}")),
        });
    }
    Ok(out)
}

/// SQLite's big-endian varint: up to 9 bytes, the 9th carrying 8 bits.
fn varint(b: &[u8], p: &mut usize) -> Result<i64, String> {
    let mut v: i64 = 0;
    for i in 0..9 {
        let c = *b.get(*p).ok_or("sut: varint past end")?;
        *p += 1;
        if i == 8 {
            return Ok((v << 8) | i64::from(c));
        }
        v = (v << 7) | i64::from(c & 0x7f);
        if c & 0x80 == 0 {
            return Ok(v);
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_and_serial_ints_decode() {
        let mut p = 0;
        assert_eq!(varint(&[0x7f], &mut p).unwrap(), 127);
        p = 0;
        assert_eq!(varint(&[0x81, 0x00], &mut p).unwrap(), 128);
        p = 0;
        // 9-byte varint: the last byte contributes all 8 bits.
        let nine = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        assert_eq!(varint(&nine, &mut p).unwrap(), -1);
        // Record: header(3) [type1, type7], values 0x05, f64 1.5.
        let mut rec = vec![3u8, 1, 7, 5];
        rec.extend_from_slice(&1.5f64.to_be_bytes());
        assert_eq!(
            decode_record(&rec).unwrap(),
            vec![Value::Int(5), Value::Real(1.5)]
        );
        // Negative 1-byte int sign-extends.
        let rec = vec![2u8, 1, 0xfe];
        assert_eq!(decode_record(&rec).unwrap(), vec![Value::Int(-2)]);
    }

    #[test]
    fn ddl_column_parse_skips_constraints() {
        let t = Table {
            sql: "CREATE TABLE X(_PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, \
                  Name TEXT DEFAULT NULL, Curve BLOB, PRIMARY KEY(_PW_ID))"
                .into(),
            rows: vec![],
        };
        assert_eq!(t.columns(), vec!["_PW_ID", "Name", "Curve"]);
    }

    /// The real .sut fixture (LOCAL-ONLY, gitignored — third-party brush
    /// data, never redistributed; tests skip where absent): the whole file
    /// walks, the documented Clip Studio tables come out, and the Variant
    /// row exposes the named parameter columns the importer maps.
    #[test]
    fn real_sut_walks_and_names_its_columns() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/sut_sample.sut");
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let tables = parse_sqlite(&bytes).unwrap();
        for want in ["Variant", "Node", "Manager"] {
            assert!(tables.contains_key(want), "missing table {want}");
        }
        let variant = &tables["Variant"];
        let cols = variant.columns();
        for c in ["BrushSize", "Opacity", "BrushHardness", "BrushInterval"] {
            assert!(cols.contains(&c.to_string()), "missing column {c}");
        }
        assert!(!variant.rows.is_empty(), "no Variant rows");
        // Every row decodes to the full column count.
        for r in &variant.rows {
            assert_eq!(r.len(), cols.len());
        }
        let node = &tables["Node"];
        assert!(
            node.records()
                .iter()
                .any(|r| r.get("NodeName").and_then(Value::as_str).is_some()),
            "no named node"
        );
    }

    /// Truncation fuzz: cuts at raw offsets never panic or hang.
    #[test]
    fn truncation_never_panics() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/sut_sample.sut");
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        for cut in (0..bytes.len()).step_by(37 * 1024).chain([100, 512, 4097]) {
            let _ = parse_sqlite(&bytes[..cut]);
        }
    }
}
