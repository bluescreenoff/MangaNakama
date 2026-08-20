//! Photoshop ActionDescriptor reader for the `.abr` v6 `desc` section — the
//! brush DYNAMICS the tip-only reader (`abr.rs`) deliberately skips.
//!
//! Layout: the `8BIMdesc` section body is `u32 version (16)` followed by one
//! serialized ActionDescriptor, the same structure PSD stores everywhere
//! (Adobe "Photoshop File Formats Specification", Descriptor structure; also
//! implemented by psd-tools' `descriptor.py`). All integers big-endian.
//!
//! - **Unicode string**: `u32 char count` (count INCLUDES the terminating
//!   NUL), then count × UTF-16BE units.
//! - **ID string**: `u32 len`; len == 0 → the id is the next 4 bytes
//!   (`'Nm  '`, `'Brsh'`), else len bytes of ASCII (`"brushPreset"`).
//! - **Descriptor**: unicode "name from classID" + ID classID + `u32 item
//!   count` + count × (ID key, 4-byte OSType, value).
//! - **OSTypes handled**: `Objc`/`GlbO` descriptor, `VlLs` list, `doub` f64,
//!   `UntF` 4-byte unit + f64, `TEXT` unicode, `enum` two IDs, `long` i32,
//!   `comp` i64, `bool` u8, `type`/`GlbC` class, `alis`/`tdta` length-prefixed
//!   blobs (skipped, length kept). `obj '` references do not occur in brush
//!   descriptors and are a clear `Err` — silently skipping one would desync
//!   the whole walk.
//!
//! The root descriptor is class `null` with one key `Brsh`: a list of
//! `brushPreset` objects. Each carries `Nm`, a `Brsh` object of class
//! `sampledBrush` (tip geometry + `sampledData`, the UUID that joins it to a
//! `samp` tip) or `computedBrush` (round tip: `Dmtr`/`Hrdn`/`Angl`/`Rndn`),
//! and the tool dynamics: `use*` gate bools plus `brVr`-class variance
//! objects (`bVTy` controller, `fStp` fade steps, `jitter` %) under `szVr`
//! (size), `opVr` (opacity), `prVr` (flow), `angleDynamics`,
//! `roundnessDynamics`, `scatterDynamics`, `countDynamics`. Extraction is
//! DEPTH-FIRST BY KEY, not by fixed nesting — Photoshop versions move these
//! around and a missing key is simply "feature off".

use std::collections::BTreeMap;

/// One parsed descriptor value. Only what brush descriptors use.
#[derive(Debug, Clone, PartialEq)]
pub enum Desc {
    /// class name + items in file order (keys repeat across versions; first
    /// wins on lookup).
    Object {
        class: String,
        items: Vec<(String, Desc)>,
    },
    List(Vec<Desc>),
    Double(f64),
    /// `UntF`: value + unit tag (`#Pxl`, `#Prc`, `#Ang`).
    Unit([u8; 4], f64),
    Text(String),
    Enum {
        type_id: String,
        value: String,
    },
    Int(i64),
    Bool(bool),
    Class(String),
    /// `alis`/`tdta`: payload skipped, length kept for honesty.
    Blob(usize),
}

impl Desc {
    /// Depth-first search for `key` anywhere under this value. First match
    /// wins — brush descriptors do not nest the same key with different
    /// meanings at different depths (verified against the real fixture).
    pub fn find(&self, key: &str) -> Option<&Desc> {
        match self {
            Desc::Object { items, .. } => {
                for (k, v) in items {
                    if k == key {
                        return Some(v);
                    }
                }
                items.iter().find_map(|(_, v)| v.find(key))
            }
            Desc::List(items) => items.iter().find_map(|v| v.find(key)),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Desc::Double(v) | Desc::Unit(_, v) => Some(*v),
            Desc::Int(v) => Some(*v as f64),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Desc::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Desc::Text(s) => Some(s),
            _ => None,
        }
    }
    fn find_f64(&self, key: &str) -> Option<f64> {
        self.find(key).and_then(Desc::as_f64)
    }
    fn find_bool(&self, key: &str) -> Option<bool> {
        self.find(key).and_then(Desc::as_bool)
    }
}

/// What drives a dynamic in Photoshop (`bVTy`). The numeric ids are the
/// scripting enum, stable since CS: 0 off, 1 fade, 2 pen pressure, 3 pen
/// tilt, 4 stylus wheel, 5 rotation, 6 initial direction, 7 direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Off,
    /// Fade over N steps.
    Fade(u32),
    Pressure,
    Tilt,
    StylusWheel,
    Rotation,
    InitialDirection,
    Direction,
    Unknown(i64),
}

impl Control {
    fn from_group(g: &Desc) -> Control {
        let ty = g.find_f64("bVTy").map(|v| v as i64).unwrap_or(0);
        match ty {
            0 => Control::Off,
            1 => Control::Fade(g.find_f64("fStp").map(|v| v as u32).unwrap_or(0)),
            2 => Control::Pressure,
            3 => Control::Tilt,
            4 => Control::StylusWheel,
            5 => Control::Rotation,
            6 => Control::InitialDirection,
            7 => Control::Direction,
            v => Control::Unknown(v),
        }
    }
    /// Photoshop's UI name, for honest "couldn't translate" labels.
    pub fn label(&self) -> String {
        match self {
            Control::Off => "off".into(),
            Control::Fade(n) => format!("fade over {n} steps"),
            Control::Pressure => "pen pressure".into(),
            Control::Tilt => "pen tilt".into(),
            Control::StylusWheel => "stylus wheel".into(),
            Control::Rotation => "pen rotation".into(),
            Control::InitialDirection => "initial direction".into(),
            Control::Direction => "stroke direction".into(),
            Control::Unknown(v) => format!("unknown controller {v}"),
        }
    }
}

/// One `brVr` variance group: controller + jitter %.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DynGroup {
    pub control: Control,
    pub jitter_pct: f64,
}

impl DynGroup {
    const OFF: DynGroup = DynGroup {
        control: Control::Off,
        jitter_pct: 0.0,
    };
    fn read(preset: &Desc, key: &str) -> DynGroup {
        match preset.find(key) {
            Some(g) => DynGroup {
                control: Control::from_group(g),
                jitter_pct: g.find_f64("jitter").unwrap_or(0.0),
            },
            None => DynGroup::OFF,
        }
    }
    pub fn is_active(&self) -> bool {
        self.control != Control::Off || self.jitter_pct > 0.0
    }
}

/// The tip a preset stamps with.
#[derive(Debug, Clone, PartialEq)]
pub enum BrushKind {
    /// Joined to a `samp` tip by UUID (lowercased, as stored).
    Sampled { sample_id: String },
    /// Photoshop's parametric round tip: fully translatable (hardness,
    /// roundness, angle are all engine settings).
    Computed { hardness_pct: f64 },
}

/// Everything we extract for ONE brush preset. `None`/`false`/`OFF` always
/// means "not present in the file", never a guess.
#[derive(Debug, Clone, PartialEq)]
pub struct AbrPresetInfo {
    pub name: Option<String>,
    pub kind: BrushKind,
    /// Brush size (`Dmtr`, px) — for sampled tips this is the size the user
    /// last set, which may differ from the bitmap's natural size.
    pub diameter_px: Option<f64>,
    pub angle_deg: f64,
    pub roundness_pct: f64,
    pub flip_x: bool,
    pub flip_y: bool,
    /// `Spcn` % of diameter; `None` when the spacing checkbox (`Intr`) is
    /// off (Photoshop then stamps per input event, not per distance).
    pub spacing_pct: Option<f64>,
    // Shape dynamics (gated by useTipDynamics in the file; already resolved
    // here: inactive groups read as OFF).
    pub size: DynGroup,
    pub minimum_diameter_pct: f64,
    pub angle_dyn: DynGroup,
    pub roundness_dyn: DynGroup,
    pub minimum_roundness_pct: f64,
    // Scattering.
    pub scatter: DynGroup,
    pub scatter_both_axes: bool,
    pub count: f64,
    pub count_dyn: DynGroup,
    // Transfer (usePaintDynamics): opacity + flow.
    pub opacity: DynGroup,
    pub flow: DynGroup,
    pub wet_edges: bool,
    pub airbrush: bool,
    /// Feature groups present AND enabled in the file that we do not model
    /// at all (dual brush, color dynamics, pattern texture, noise) — the
    /// importer turns these into honest labels.
    pub unmodeled: Vec<&'static str>,
}

/// Parse the `desc` section body (the bytes AFTER the `8BIM`+`desc`+len
/// header) into per-preset dynamics.
pub fn parse_desc(body: &[u8]) -> Result<Vec<AbrPresetInfo>, String> {
    let mut r = Reader { buf: body, pos: 0 };
    let version = r.u32()?;
    if version != 16 {
        return Err(format!("desc: descriptor version {version}, want 16"));
    }
    let root = parse_descriptor(&mut r, 0)?;
    let Some(Desc::List(presets)) = root.find("Brsh") else {
        return Err("desc: no Brsh preset list".into());
    };
    Ok(presets.iter().filter_map(extract_preset).collect())
}

/// One brushPreset object → extracted info. Returns `None` for entries with
/// no recognizable tip (neither sampledData nor a computed brush object) —
/// those are tool presets referencing brushes stored elsewhere.
fn extract_preset(p: &Desc) -> Option<AbrPresetInfo> {
    // The tip object: class sampledBrush | computedBrush under key Brsh.
    let tip = p.find("Brsh");
    let kind = match tip {
        Some(t) => {
            if let Some(id) = t.find("sampledData").and_then(Desc::as_str) {
                BrushKind::Sampled {
                    sample_id: id.trim().to_ascii_lowercase(),
                }
            } else if matches!(t, Desc::Object { class, .. } if class == "computedBrush") {
                BrushKind::Computed {
                    hardness_pct: t.find_f64("Hrdn").unwrap_or(100.0),
                }
            } else {
                return None;
            }
        }
        None => return None,
    };
    let tip = tip.expect("checked above");

    let use_tip_dyn = p.find_bool("useTipDynamics").unwrap_or(false);
    let use_scatter = p.find_bool("useScatter").unwrap_or(false);
    let use_paint = p.find_bool("usePaintDynamics").unwrap_or(false);
    let gate = |on: bool, g: DynGroup| if on { g } else { DynGroup::OFF };

    let mut unmodeled = Vec::new();
    for (gate_key, label) in [
        ("useDualBrush", "dual brush"),
        ("useColorDynamics", "color dynamics"),
        ("useTexture", "pattern texture"),
    ] {
        if p.find_bool(gate_key).unwrap_or(false) {
            unmodeled.push(label);
        }
    }
    if p.find_bool("Nose").unwrap_or(false) {
        unmodeled.push("noise");
    }

    Some(AbrPresetInfo {
        name: p
            .find("Nm  ")
            .and_then(Desc::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        kind,
        diameter_px: tip.find_f64("Dmtr"),
        angle_deg: tip.find_f64("Angl").unwrap_or(0.0),
        roundness_pct: tip.find_f64("Rndn").unwrap_or(100.0).clamp(1.0, 100.0),
        flip_x: tip.find_bool("flipX").unwrap_or(false),
        flip_y: tip.find_bool("flipY").unwrap_or(false),
        spacing_pct: if tip.find_bool("Intr").unwrap_or(false) {
            tip.find_f64("Spcn")
        } else {
            None
        },
        size: gate(use_tip_dyn, DynGroup::read(p, "szVr")),
        minimum_diameter_pct: p.find_f64("minimumDiameter").unwrap_or(0.0),
        angle_dyn: gate(use_tip_dyn, DynGroup::read(p, "angleDynamics")),
        roundness_dyn: gate(use_tip_dyn, DynGroup::read(p, "roundnessDynamics")),
        minimum_roundness_pct: p.find_f64("minimumRoundness").unwrap_or(0.0),
        scatter: gate(use_scatter, DynGroup::read(p, "scatterDynamics")),
        scatter_both_axes: p.find_bool("bothAxes").unwrap_or(false),
        count: p.find_f64("Cnt ").unwrap_or(1.0).max(1.0),
        count_dyn: gate(use_scatter, DynGroup::read(p, "countDynamics")),
        opacity: gate(use_paint, DynGroup::read(p, "opVr")),
        flow: gate(use_paint, DynGroup::read(p, "prVr")),
        wet_edges: p.find_bool("Wtdg").unwrap_or(false),
        airbrush: p.find_bool("Rpt ").unwrap_or(false),
        unmodeled,
    })
}

/// Index sampled presets by their `samp` UUID.
pub fn by_sample_id(presets: &[AbrPresetInfo]) -> BTreeMap<&str, &AbrPresetInfo> {
    presets
        .iter()
        .filter_map(|p| match &p.kind {
            BrushKind::Sampled { sample_id } => Some((sample_id.as_str(), p)),
            BrushKind::Computed { .. } => None,
        })
        .collect()
}

/// Recursion guard: hostile nesting must not blow the stack. Real files are
/// ~4 deep.
const MAX_DEPTH: u32 = 32;

fn parse_descriptor(r: &mut Reader, depth: u32) -> Result<Desc, String> {
    if depth > MAX_DEPTH {
        return Err("desc: nesting too deep".into());
    }
    let _name = r.unicode()?; // "name from classID", empty in practice
    let class = r.id()?;
    let count = r.u32()? as usize;
    // Each item is ≥ 8 bytes on disk; a lying count must not reserve.
    if count > r.left() / 8 {
        return Err(format!("desc: item count {count} exceeds section"));
    }
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        let key = r.id()?;
        let value = parse_value(r, depth)?;
        items.push((key, value));
    }
    Ok(Desc::Object { class, items })
}

fn parse_value(r: &mut Reader, depth: u32) -> Result<Desc, String> {
    let ty = r.tag4()?;
    Ok(match &ty {
        b"Objc" | b"GlbO" => parse_descriptor(r, depth + 1)?,
        b"VlLs" => {
            let count = r.u32()? as usize;
            if count > r.left() / 4 {
                return Err(format!("desc: list count {count} exceeds section"));
            }
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                items.push(parse_value(r, depth + 1)?);
            }
            Desc::List(items)
        }
        b"doub" => Desc::Double(r.f64()?),
        b"UntF" => {
            let unit = r.tag4()?;
            Desc::Unit(unit, r.f64()?)
        }
        b"TEXT" => Desc::Text(r.unicode()?),
        b"enum" => Desc::Enum {
            type_id: r.id()?,
            value: r.id()?,
        },
        b"long" => Desc::Int(r.i32()? as i64),
        b"comp" => Desc::Int(r.i64()?),
        b"bool" => Desc::Bool(r.u8()? != 0),
        b"type" | b"GlbC" => {
            let _name = r.unicode()?;
            Desc::Class(r.id()?)
        }
        b"alis" | b"tdta" => {
            let len = r.u32()? as usize;
            r.skip(len)?;
            Desc::Blob(len)
        }
        b"obj " => return Err("desc: reference values unsupported".into()),
        t => {
            return Err(format!(
                "desc: unknown value type {:?} at byte {}",
                String::from_utf8_lossy(t),
                r.pos
            ));
        }
    })
}

/// Big-endian bounds-checked cursor (the same idiom as `abr.rs`; duplicated
/// because that one is private to its module and eight tiny methods do not
/// justify a shared-visibility refactor).
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn left(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.left() < n {
            return Err(format!("desc: truncated at byte {} (+{n})", self.pos));
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
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> Result<i32, String> {
        let s = self.take(4)?;
        Ok(i32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i64(&mut self) -> Result<i64, String> {
        let s = self.take(8)?;
        Ok(i64::from_be_bytes(s.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64, String> {
        let s = self.take(8)?;
        Ok(f64::from_be_bytes(s.try_into().unwrap()))
    }
    /// `u32 char count` (incl. NUL) + UTF-16BE units, NUL-trimmed.
    fn unicode(&mut self) -> Result<String, String> {
        let chars = self.u32()? as usize;
        if chars > self.left() / 2 {
            return Err(format!("desc: string length {chars} exceeds section"));
        }
        let units = self.take(chars * 2)?;
        let mut s = String::with_capacity(chars);
        for pair in units.chunks_exact(2) {
            let u = u16::from_be_bytes([pair[0], pair[1]]);
            if u == 0 {
                break;
            }
            s.push(char::from_u32(u as u32).unwrap_or('\u{FFFD}'));
        }
        Ok(s)
    }
    /// ID string: `u32 len`, 0 → 4-byte tag (kept verbatim, spaces and
    /// all — `"Nm  "`, `"Cnt "`), else len ASCII bytes.
    fn id(&mut self) -> Result<String, String> {
        let len = self.u32()? as usize;
        if len == 0 {
            let t = self.take(4)?;
            Ok(String::from_utf8_lossy(t).into_owned())
        } else {
            if len > self.left() {
                return Err(format!("desc: id length {len} exceeds section"));
            }
            Ok(String::from_utf8_lossy(self.take(len)?).into_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- serializer helpers: build descriptor bytes the way Photoshop does --

    fn unicode(s: &str) -> Vec<u8> {
        let mut b = Vec::new();
        let units: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
        b.extend_from_slice(&(units.len() as u32).to_be_bytes());
        for u in units {
            b.extend_from_slice(&u.to_be_bytes());
        }
        b
    }
    fn id(s: &str) -> Vec<u8> {
        let mut b = Vec::new();
        if s.len() == 4 && s.is_ascii() {
            b.extend_from_slice(&0u32.to_be_bytes());
            b.extend_from_slice(s.as_bytes());
        } else {
            b.extend_from_slice(&(s.len() as u32).to_be_bytes());
            b.extend_from_slice(s.as_bytes());
        }
        b
    }
    fn objc(class: &str, items: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut b = unicode("");
        b.extend(id(class));
        b.extend_from_slice(&(items.len() as u32).to_be_bytes());
        for (k, v) in items {
            b.extend(id(k));
            b.extend_from_slice(v);
        }
        b
    }
    fn val_objc(class: &str, items: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut b = b"Objc".to_vec();
        b.extend(objc(class, items));
        b
    }
    fn val_unit(unit: &[u8; 4], v: f64) -> Vec<u8> {
        let mut b = b"UntF".to_vec();
        b.extend_from_slice(unit);
        b.extend_from_slice(&v.to_be_bytes());
        b
    }
    fn val_bool(v: bool) -> Vec<u8> {
        vec![b'b', b'o', b'o', b'l', v as u8]
    }
    fn val_long(v: i32) -> Vec<u8> {
        let mut b = b"long".to_vec();
        b.extend_from_slice(&v.to_be_bytes());
        b
    }
    fn val_text(s: &str) -> Vec<u8> {
        let mut b = b"TEXT".to_vec();
        b.extend(unicode(s));
        b
    }
    fn val_doub(v: f64) -> Vec<u8> {
        let mut b = b"doub".to_vec();
        b.extend_from_slice(&v.to_be_bytes());
        b
    }
    fn val_list(items: &[Vec<u8>]) -> Vec<u8> {
        let mut b = b"VlLs".to_vec();
        b.extend_from_slice(&(items.len() as u32).to_be_bytes());
        for i in items {
            b.extend_from_slice(i);
        }
        b
    }
    fn brvr(bvty: i32, fstp: i32, jitter: f64) -> Vec<u8> {
        val_objc(
            "brVr",
            &[
                ("bVTy", val_long(bvty)),
                ("fStp", val_long(fstp)),
                ("jitter", val_unit(b"#Prc", jitter)),
            ],
        )
    }
    fn section(root_items: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut b = 16u32.to_be_bytes().to_vec();
        b.extend(objc("null", root_items));
        b
    }

    fn sampled_preset(uuid: &str) -> Vec<u8> {
        val_objc(
            "brushPreset",
            &[
                ("Nm  ", val_text("Scatter Ink")),
                (
                    "Brsh",
                    val_objc(
                        "sampledBrush",
                        &[
                            ("Dmtr", val_unit(b"#Pxl", 64.0)),
                            ("Angl", val_unit(b"#Ang", 30.0)),
                            ("Rndn", val_unit(b"#Prc", 80.0)),
                            ("Spcn", val_unit(b"#Prc", 25.0)),
                            ("Intr", val_bool(true)),
                            ("flipX", val_bool(true)),
                            ("flipY", val_bool(false)),
                            ("sampledData", val_text(uuid)),
                        ],
                    ),
                ),
                ("useTipDynamics", val_bool(true)),
                ("szVr", brvr(2, 0, 10.0)),
                ("minimumDiameter", val_unit(b"#Prc", 40.0)),
                ("angleDynamics", brvr(7, 0, 0.0)),
                ("roundnessDynamics", brvr(0, 0, 0.0)),
                ("useScatter", val_bool(true)),
                ("scatterDynamics", brvr(0, 0, 120.0)),
                ("bothAxes", val_bool(true)),
                ("Cnt ", val_doub(3.0)),
                ("countDynamics", brvr(0, 0, 0.0)),
                ("usePaintDynamics", val_bool(true)),
                ("opVr", brvr(0, 0, 15.0)),
                ("prVr", brvr(2, 0, 0.0)),
                ("useColorDynamics", val_bool(false)),
                ("useDualBrush", val_bool(true)),
                ("useTexture", val_bool(false)),
                ("Wtdg", val_bool(true)),
                ("Nose", val_bool(false)),
                ("Rpt ", val_bool(false)),
            ],
        )
    }

    #[test]
    fn sampled_preset_extracts_everything() {
        let uuid = "2205283B-E0F2-11df-AC64-AC9512EB12E7";
        let bytes = section(&[("Brsh", val_list(&[sampled_preset(uuid)]))]);
        let presets = parse_desc(&bytes).unwrap();
        assert_eq!(presets.len(), 1);
        let p = &presets[0];
        assert_eq!(p.name.as_deref(), Some("Scatter Ink"));
        assert_eq!(
            p.kind,
            BrushKind::Sampled {
                sample_id: uuid.to_ascii_lowercase()
            }
        );
        assert_eq!(p.diameter_px, Some(64.0));
        assert_eq!(p.angle_deg, 30.0);
        assert_eq!(p.roundness_pct, 80.0);
        assert!(p.flip_x && !p.flip_y);
        assert_eq!(p.spacing_pct, Some(25.0));
        assert_eq!(p.size.control, Control::Pressure);
        assert_eq!(p.size.jitter_pct, 10.0);
        assert_eq!(p.minimum_diameter_pct, 40.0);
        assert_eq!(p.angle_dyn.control, Control::Direction);
        assert!(!p.roundness_dyn.is_active());
        assert_eq!(p.scatter.jitter_pct, 120.0);
        assert!(p.scatter_both_axes);
        assert_eq!(p.count, 3.0);
        assert_eq!(p.opacity.jitter_pct, 15.0);
        assert_eq!(p.flow.control, Control::Pressure);
        assert!(p.wet_edges);
        assert_eq!(p.unmodeled, vec!["dual brush"]);
    }

    #[test]
    fn spacing_checkbox_off_means_no_spacing() {
        let preset = val_objc(
            "brushPreset",
            &[(
                "Brsh",
                val_objc(
                    "sampledBrush",
                    &[
                        ("Spcn", val_unit(b"#Prc", 25.0)),
                        ("Intr", val_bool(false)),
                        ("sampledData", val_text("abc")),
                    ],
                ),
            )],
        );
        let presets = parse_desc(&section(&[("Brsh", val_list(&[preset]))])).unwrap();
        assert_eq!(presets[0].spacing_pct, None);
    }

    #[test]
    fn gates_off_zero_their_groups() {
        // Groups present with live values, but every use* gate false → OFF.
        let preset = val_objc(
            "brushPreset",
            &[
                (
                    "Brsh",
                    val_objc("sampledBrush", &[("sampledData", val_text("abc"))]),
                ),
                ("useTipDynamics", val_bool(false)),
                ("szVr", brvr(2, 0, 50.0)),
                ("useScatter", val_bool(false)),
                ("scatterDynamics", brvr(0, 0, 500.0)),
                ("usePaintDynamics", val_bool(false)),
                ("opVr", brvr(2, 0, 50.0)),
            ],
        );
        let presets = parse_desc(&section(&[("Brsh", val_list(&[preset]))])).unwrap();
        let p = &presets[0];
        assert!(!p.size.is_active());
        assert!(!p.scatter.is_active());
        assert!(!p.opacity.is_active());
    }

    #[test]
    fn computed_brush_and_unrecognized_entries() {
        let computed = val_objc(
            "brushPreset",
            &[
                ("Nm  ", val_text("Hard Round")),
                (
                    "Brsh",
                    val_objc(
                        "computedBrush",
                        &[
                            ("Dmtr", val_unit(b"#Pxl", 20.0)),
                            ("Hrdn", val_unit(b"#Prc", 90.0)),
                        ],
                    ),
                ),
            ],
        );
        // A preset whose Brsh is neither sampled nor computed is dropped.
        let alien = val_objc(
            "brushPreset",
            &[("Brsh", val_objc("futureBrush", &[("X", val_bool(true))]))],
        );
        let presets =
            parse_desc(&section(&[("Brsh", val_list(&[computed, alien]))])).unwrap();
        assert_eq!(presets.len(), 1);
        assert_eq!(
            presets[0].kind,
            BrushKind::Computed { hardness_pct: 90.0 }
        );
        assert_eq!(presets[0].diameter_px, Some(20.0));
    }

    #[test]
    fn fade_and_unknown_controllers_are_kept_honest() {
        assert_eq!(
            Control::from_group(&Desc::Object {
                class: "brVr".into(),
                items: vec![
                    ("bVTy".into(), Desc::Int(1)),
                    ("fStp".into(), Desc::Int(25)),
                ],
            }),
            Control::Fade(25)
        );
        assert_eq!(Control::Fade(25).label(), "fade over 25 steps");
        assert!(Control::Unknown(9).label().contains('9'));
    }

    /// The REAL fixture's desc section: 36 presets in the file, 32 sampled
    /// (joinable by UUID to the 31 samp tips — one tip serves two presets),
    /// plus computed entries. Pins the full parse walk against Photoshop's
    /// own serializer, not our test helpers.
    #[test]
    fn real_fixture_desc_parses_and_joins() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/abr_v6_sample.abr");
        let Ok(bytes) = std::fs::read(&path) else {
            return; // local-only fixture (see abr.rs): skip silently
        };
        // Walk the 8BIM sections to the desc body.
        let mut pos = 4;
        let mut desc = None;
        while pos + 12 <= bytes.len() {
            let kind = &bytes[pos + 4..pos + 8];
            let len = u32::from_be_bytes(bytes[pos + 8..pos + 12].try_into().unwrap()) as usize;
            let body = &bytes[pos + 12..(pos + 12 + len).min(bytes.len())];
            if kind == b"desc" {
                desc = Some(body);
            }
            pos += 12 + len;
        }
        let presets = parse_desc(desc.expect("fixture has a desc section")).unwrap();
        assert!(presets.len() >= 30, "got {}", presets.len());
        let sampled = by_sample_id(&presets);
        assert!(sampled.len() >= 30);
        // Every sampled preset carries a plausible UUID join key.
        for id in sampled.keys() {
            assert_eq!(id.len(), 36, "not a uuid: {id}");
        }
        // The set is scatter-heavy: at least one preset uses scatter with a
        // real percentage, and at least one drives size by pen pressure.
        assert!(presets.iter().any(|p| p.scatter.jitter_pct > 0.0));
        assert!(
            presets
                .iter()
                .any(|p| p.size.control == Control::Pressure || p.flow.control == Control::Pressure)
        );
        // Spacing comes through as sane percentages where enabled.
        for p in &presets {
            if let Some(s) = p.spacing_pct {
                assert!((1.0..=1000.0).contains(&s), "spacing {s}");
            }
        }
    }

    #[test]
    fn truncation_never_panics() {
        let uuid = "2205283B-E0F2-11df-AC64-AC9512EB12E7";
        let bytes = section(&[("Brsh", val_list(&[sampled_preset(uuid)]))]);
        for cut in 0..bytes.len() {
            let _ = parse_desc(&bytes[..cut]);
        }
        // Lying counts must not allocate or hang.
        let mut lie = section(&[("Brsh", val_list(&[]))]);
        let n = lie.len();
        lie[n - 4..].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        assert!(parse_desc(&lie).is_err());
    }
}
