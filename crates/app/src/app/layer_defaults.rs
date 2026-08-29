//! `LP-001` Save as default: bake the ACTIVE layer's presentation properties
//! as the starting point for the next layer of that TYPE.
//!
//! Persisted as plain `type.field=value` lines in `layer_defaults.txt` beside
//! the exe — the same idiom as `prefs.txt` / `swatches.txt` / `themes/*.txt`,
//! and for the same reason: no config framework, a file a person can read and
//! delete. It is NOT `prefs.txt`, because that file is the Preferences
//! window's ten hand-set values (its `to_body` enumerates them by name);
//! these are machine-written, one small block per layer type, and a user who
//! wants his tone defaults back should be able to delete THEM without losing
//! his undo depth and canvas size.
//!
//! **Fault tolerance is per LINE and per FIELD**, like `prefs.rs`: a line
//! with no `=` is dropped, a value that will not parse leaves that one
//! property at the built-in default, and a key this build does not know is
//! kept verbatim so an older exe cannot eat a newer one's defaults. There is
//! no version key — the key set is the version.
//!
//! **What is defaulted, and what is deliberately not.** Saved: the things
//! the Layer Property panel itself edits — blend, opacity, the tone effect
//! and its lattice, the border effect, layer/sub colour, the 1-bit preview,
//! and (fill and tone layers only) the live fill parameters. NOT saved:
//! name, id and stack position (identity); visibility, the two locks and
//! clipping (a new layer that arrived hidden or locked reads as a broken
//! app, not a preference); reference and draft (reference is exclusive and
//! draft silently drops the layer out of export — defaulting either is a
//! trap); the label strip (organisation); gradient and correction
//! parameters, which are geometry and dialog state rather than style.

use mn_core::{Blend, FillKind, Layer, LayerExpression, LayerKind};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The type a layer defaults under — the key prefix in the file, and the
/// unit `LP-001` works in ("newly created layers of that type").
///
/// Most specific first, exactly like the palette's `row_glyph`: a folder is
/// a folder before it is a frame, and a stroke-recording raster is a vector
/// layer before it is a raster one. The tone EFFECT is a property, not a
/// type — a toned raster still keys as `raster`, so saving from one makes
/// new raster layers toned, which is what CSP's palette menu does.
pub fn kind_key(l: &Layer) -> &'static str {
    if l.folder {
        return if l.is_frame() {
            "frame-folder"
        } else {
            "folder"
        };
    }
    match l.kind {
        LayerKind::Text(_) => "text",
        LayerKind::Balloon(_) => "balloon",
        LayerKind::Frame(_) => "frame",
        LayerKind::Fill(FillKind::Flat { .. }) => "fill",
        LayerKind::Fill(FillKind::Gradient { .. }) => "gradient",
        LayerKind::Fill(FillKind::Tone { .. }) => "tone",
        LayerKind::Correction(_) => "correction",
        LayerKind::Raster if l.records_strokes() => "vector",
        LayerKind::Raster => "raster",
    }
}

/// The types whose creation path applies a saved default. The others
/// (text, balloon, frame and frame folders) are made BY a tool, out of that
/// tool's own parameters, and wiring a second source of truth into them is
/// a different feature — so the palette does not offer Save as default
/// there rather than saving something that would never be read.
pub fn applies_to(key: &str) -> bool {
    matches!(
        key,
        "raster" | "vector" | "folder" | "fill" | "gradient" | "tone" | "correction"
    )
}

/// Human words for a status line: "…for new tone layers".
pub fn kind_label(key: &str) -> &'static str {
    match key {
        "raster" => "raster layers",
        "vector" => "vector layers",
        "folder" => "folders",
        "fill" => "fill layers",
        "gradient" => "gradient layers",
        "tone" => "tone layers",
        "correction" => "correction layers",
        "text" => "text layers",
        "balloon" => "balloon layers",
        "frame" => "frame layers",
        "frame-folder" => "frame folders",
        _ => "layers of this type",
    }
}

/// The field names this build writes. `capture` clears exactly these for the
/// type it is saving and leaves everything else in the file alone — that is
/// what makes a newer build's key survive an older build's save.
const FIELDS: [&str; 8] = [
    "opacity",
    "blend",
    "colour",
    "sub_colour",
    "expression",
    "tone",
    "edge",
    "fill",
];

/// Saved per-type layer defaults: `type.field` → value, verbatim.
///
/// A raw string map rather than a struct per type, on purpose: unknown keys
/// then survive a round trip for free (we write back what we read), and a
/// value that has rotted costs one property rather than the file.
#[derive(Default, Clone)]
pub struct LayerDefaults {
    map: BTreeMap<String, String>,
    dirty: bool,
}

impl LayerDefaults {
    pub fn load() -> Self {
        let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return Self::default();
        };
        Self::parse(&text)
    }

    /// One `type.field=value` line per entry. A line with no `=`, an empty
    /// key or a key with no type prefix is skipped; everything else loads.
    pub fn parse(text: &str) -> Self {
        let mut me = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let k = k.trim();
            // The prefix IS the type; a bare key belongs to no layer type
            // and would be written back out as a line nothing can read.
            if k.is_empty() || !k.contains('.') {
                continue;
            }
            me.map.insert(k.to_owned(), v.trim().to_owned());
        }
        me
    }

    pub fn to_body(&self) -> String {
        let mut body = String::new();
        for (k, v) in &self.map {
            body.push_str(k);
            body.push('=');
            body.push_str(v);
            body.push('\n');
        }
        body
    }

    /// Write on the button press, not at exit: the action is one explicit
    /// click, and a default the user saved must survive a crash.
    pub fn save_if_dirty(&mut self) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        let Some(p) = path() else { return };
        if self.map.is_empty() {
            // Nothing left to say: delete rather than leave an empty file
            // people would wonder about (the `tags.txt` rule).
            let _ = std::fs::remove_file(p);
            return;
        }
        let _ = std::fs::write(p, self.to_body());
    }

    /// Is there a default saved for this type at all?
    pub fn has(&self, key: &str) -> bool {
        FIELDS.iter().any(|f| self.get(key, f).is_some())
    }

    fn get(&self, key: &str, field: &str) -> Option<&str> {
        self.map
            .get(&format!("{key}.{field}"))
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    fn put(&mut self, key: &str, field: &str, value: Option<String>) {
        let k = format!("{key}.{field}");
        match value {
            Some(v) => self.map.insert(k, v),
            None => self.map.remove(&k),
        };
    }

    /// `LP-001`: bake this layer's properties as the type's default.
    pub fn capture(&mut self, l: &Layer) {
        let k = kind_key(l);
        self.put(k, "opacity", Some(format!("{:.4}", l.opacity)));
        self.put(k, "blend", Some(l.blend.ora_name().to_owned()));
        self.put(k, "colour", l.layer_colour.map(hex));
        self.put(k, "sub_colour", l.layer_sub_colour.map(hex));
        self.put(
            k,
            "expression",
            l.expression.ora_name().map(str::to_owned),
        );
        self.put(k, "tone", l.tone.as_ref().and_then(json));
        self.put(k, "edge", l.edge.as_ref().and_then(json));
        // Only the two fill kinds whose parameters are STYLE. A gradient's
        // are the drag that made it, and a correction's belong to its
        // dialog — see the module header.
        self.put(
            k,
            "fill",
            match l.kind {
                LayerKind::Fill(f @ (FillKind::Flat { .. } | FillKind::Tone { .. })) => json(&f),
                _ => None,
            },
        );
        self.dirty = true;
    }

    /// Drop every field this build knows for one type. A key from a newer
    /// build under the same type stays — "forget my tone default" is not
    /// "delete the file".
    pub fn forget(&mut self, key: &str) {
        for f in FIELDS {
            self.put(key, f, None);
        }
        self.dirty = true;
    }

    /// Apply the saved default for `l`'s own type, in place. Called at
    /// creation, BEFORE the caller's structural undo entry is anything but
    /// a snapshot of the stack without this layer — so undoing the creation
    /// removes the layer wholesale and no second undo step exists.
    ///
    /// Every field is independent: a rotted value leaves that property at
    /// the built-in default and the rest still land.
    pub fn apply(&self, l: &mut Layer) {
        let k = kind_key(l);
        if let Some(v) = self
            .get(k, "opacity")
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
        {
            l.opacity = v.clamp(0.0, 1.0);
        }
        if let Some(v) = self.get(k, "blend") {
            // Round-trip check rather than `from_ora_name` alone: that
            // function answers `Normal` for anything it does not know, and
            // a garbled line must keep the layer's own default instead of
            // silently resetting the blend mode.
            let b = Blend::from_ora_name(v);
            if b.ora_name() == v {
                l.blend = b;
            }
        }
        if let Some(c) = self.get(k, "colour").and_then(unhex) {
            l.layer_colour = Some(c);
        }
        if let Some(c) = self.get(k, "sub_colour").and_then(unhex) {
            l.layer_sub_colour = Some(c);
        }
        if let Some(v) = self.get(k, "expression") {
            let e = LayerExpression::from_ora_name(v);
            if e.ora_name() == Some(v) {
                l.expression = e;
            }
        }
        // The two effects answer to the same refusals as their commands:
        // `Document::set_tone` refuses folders and derived layers, and
        // `set_edge` refuses frame folders. A default must not reach where
        // the command cannot.
        if !l.folder && !l.is_vector() && let Some(t) = self.get(k, "tone").and_then(unjson) {
            l.tone = Some(t);
        }
        if !(l.folder && l.is_frame()) && let Some(e) = self.get(k, "edge").and_then(unjson) {
            l.edge = Some(e);
        }
    }

    /// The live-fill parameters a new fill layer should be born with —
    /// applied as INPUT to `Document::add_fill_layer` rather than patched
    /// afterwards, so the layer's derived-raster stamp is never stale.
    /// A saved payload of a different variant is ignored (the key already
    /// encodes the variant, so this only fires on a hand-edited file).
    pub fn fill_kind(&self, want: FillKind) -> FillKind {
        let mut probe = Layer::new("probe");
        probe.kind = LayerKind::Fill(want);
        match self.get(kind_key(&probe), "fill").and_then(unjson) {
            Some(saved) if same_variant(saved, want) => saved,
            _ => want,
        }
    }

    /// The panel's one-line "this is what you saved" readout — which
    /// PROPERTIES the default covers, not their values. The values are one
    /// glance away (they are what the panel above is showing on the next
    /// layer of this type), and naming them here would mean a second copy
    /// of every blend-mode label.
    pub fn summary(&self, key: &str) -> Option<String> {
        if !self.has(key) {
            return None;
        }
        let mut bits: Vec<&str> = Vec::new();
        if self.get(key, "blend").is_some() || self.get(key, "opacity").is_some() {
            bits.push("blend + opacity");
        }
        if self.get(key, "fill").is_some() {
            bits.push("fill params");
        }
        if self.get(key, "tone").is_some() {
            bits.push("tone");
        }
        if self.get(key, "colour").is_some() {
            bits.push("layer colour");
        }
        if self.get(key, "edge").is_some() {
            bits.push("border effect");
        }
        if self.get(key, "expression").is_some() {
            bits.push("1-bit preview");
        }
        Some(bits.join(", "))
    }
}

impl crate::app::App {
    /// Seed a layer that was created THIS command with its type's saved
    /// default. Called immediately after the `Document` add call and before
    /// anything else: the add already took the structural snapshot of the
    /// stack WITHOUT this layer, so these writes ride inside that one undo
    /// step — one press still removes the layer wholesale, and redo brings
    /// it back with the defaults on it.
    ///
    /// It writes the fields directly rather than through `set_tone` /
    /// `set_edge`, which push undo entries of their own; on a layer this
    /// young there is nothing derived to invalidate (a fresh layer has no
    /// tiles and no caches).
    pub(crate) fn apply_layer_defaults(&mut self, index: usize) {
        let d = std::mem::take(&mut self.layer_defaults);
        let mut toned = false;
        if let Some(l) = self.doc.layers.get_mut(index) {
            d.apply(l);
            toned = l.tone.is_some();
        }
        self.layer_defaults = d;
        if toned {
            self.refresh_tones();
        }
    }
}

fn same_variant(a: FillKind, b: FillKind) -> bool {
    std::mem::discriminant(&a) == std::mem::discriminant(&b)
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn unhex(s: &str) -> Option<[u8; 3]> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?])
}

fn json<T: serde::Serialize>(v: &T) -> Option<String> {
    serde_json::to_string(v).ok()
}

fn unjson<T: serde::de::DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_str(s).ok()
}

fn path() -> Option<PathBuf> {
    // Under `cargo test` the "exe" is the test binary in target/debug/deps,
    // so a default saved by one test would be loaded by every other test's
    // new layer — a leak between tests that would look like a bug in
    // whatever ran next. The file is a real-run concern; the tests drive
    // `parse`/`capture`/`apply` directly.
    if cfg!(test) {
        return None;
    }
    Some(
        std::env::current_exe()
            .ok()?
            .parent()?
            .join("layer_defaults.txt"),
    )
}

/// Where the file lives, for the panel's hover text — so a user who has
/// wedged a default can delete it without asking anyone.
pub fn path_hint() -> String {
    path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "layer_defaults.txt".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mn_core::{ToneParams, edge::EdgeParams};

    fn raster() -> Layer {
        Layer::new("Layer 1")
    }

    fn tone_fill() -> Layer {
        let mut l = Layer::new("Tone");
        l.kind = LayerKind::Fill(FillKind::Tone {
            tone: ToneParams::default(),
            density: 0.4,
        });
        l
    }

    /// The type key is the unit `LP-001` works in, and it is most-specific
    /// first: a folder is a folder, a stroke-recording raster is a vector
    /// layer, and each live fill variant keys apart.
    #[test]
    fn every_layer_kind_has_its_own_key() {
        assert_eq!(kind_key(&raster()), "raster");

        let mut v = raster();
        v.strokes = Some(Default::default());
        assert_eq!(kind_key(&v), "vector");

        let mut f = raster();
        f.folder = true;
        assert_eq!(kind_key(&f), "folder");
        f.kind = LayerKind::Frame(mn_core::FrameSet::single_rect([0.0, 0.0, 8.0, 8.0], 2.0));
        assert_eq!(kind_key(&f), "frame-folder");

        assert_eq!(kind_key(&tone_fill()), "tone");
        let mut flat = raster();
        flat.kind = LayerKind::Fill(FillKind::Flat { color: [0.0; 4] });
        assert_eq!(kind_key(&flat), "fill");
        let mut text = raster();
        text.kind = LayerKind::Text(Default::default());
        assert_eq!(kind_key(&text), "text");

        // A toned RASTER is still a raster: the tone is a property the
        // default carries, not a type of its own.
        let mut toned = raster();
        toned.tone = Some(ToneParams::default());
        assert_eq!(kind_key(&toned), "raster");
    }

    /// Save-as-default, restart, new layer: every saved property comes back
    /// through the file body, and nothing else moves.
    #[test]
    fn a_saved_default_round_trips_through_the_file() {
        let mut src = raster();
        src.opacity = 0.42;
        src.blend = Blend::Multiply;
        src.layer_colour = Some([0x2a, 0x6f, 0xf4]);
        src.layer_sub_colour = Some([0xff, 0xff, 0x00]);
        src.expression = LayerExpression::Mono;
        src.tone = Some(ToneParams {
            lpi: 72.5,
            angle_deg: 15.0,
            ..ToneParams::default()
        });
        src.edge = Some(EdgeParams {
            width_px: 3.5,
            colour: [255, 255, 255],
            style: Default::default(),
        });

        let mut d = LayerDefaults::default();
        d.capture(&src);
        let reloaded = LayerDefaults::parse(&d.to_body());

        let mut fresh = raster();
        reloaded.apply(&mut fresh);
        assert!((fresh.opacity - 0.42).abs() < 1e-4, "{}", fresh.opacity);
        assert_eq!(fresh.blend, Blend::Multiply);
        assert_eq!(fresh.layer_colour, Some([0x2a, 0x6f, 0xf4]));
        assert_eq!(fresh.layer_sub_colour, Some([0xff, 0xff, 0x00]));
        assert_eq!(fresh.expression, LayerExpression::Mono);
        assert_eq!(fresh.tone.expect("tone default").lpi, 72.5);
        assert_eq!(fresh.edge.expect("edge default").width_px, 3.5);

        // Identity and safety fields are NOT part of the deal.
        assert_eq!(fresh.name, "Layer 1");
        assert!(fresh.visible && !fresh.lock && !fresh.clip && !fresh.draft);
    }

    /// The default is keyed by TYPE: a raster default must not land on a
    /// tone layer, and forgetting one type leaves the other alone.
    #[test]
    fn only_the_saved_type_picks_the_default_up() {
        let mut src = raster();
        src.opacity = 0.25;
        src.blend = Blend::Screen;
        let mut d = LayerDefaults::default();
        d.capture(&src);

        let mut other = tone_fill();
        let (was_op, was_blend) = (other.opacity, other.blend);
        d.apply(&mut other);
        assert_eq!(other.opacity, was_op, "a tone layer is not a raster layer");
        assert_eq!(other.blend, was_blend);

        let mut mine = raster();
        d.apply(&mut mine);
        assert_eq!(mine.blend, Blend::Screen, "its own type does pick it up");

        d.capture(&tone_fill());
        assert!(d.has("raster") && d.has("tone"));
        d.forget("tone");
        assert!(d.has("raster"), "forgetting one type keeps the others");
        assert!(!d.has("tone"));
    }

    /// Corruption is per LINE and per FIELD, exactly like `prefs.txt`: one
    /// mangled row keeps its own default and every other row still loads.
    #[test]
    fn one_bad_line_does_not_take_the_file_down() {
        let d = LayerDefaults::parse(
            "raster.opacity=0.5\n\
             this line has no equals sign at all\n\
             raster.blend=svg:not-a-mode\n\
             =0.9\n\
             bare_key_with_no_type=1\n\
             raster.colour=nonsense\n\
             raster.tone={\"pattern\":\n\
             \n\
             raster.expression=mono\n",
        );
        let mut l = raster();
        l.blend = Blend::Normal;
        d.apply(&mut l);
        assert_eq!(l.opacity, 0.5, "the good line before the mess still loads");
        assert_eq!(
            l.blend,
            Blend::Normal,
            "an unknown blend name keeps the layer's own mode, it does not reset it"
        );
        assert_eq!(l.layer_colour, None, "a mangled colour is skipped");
        assert_eq!(l.tone, None, "half a JSON object is not a tone default");
        assert_eq!(
            l.expression,
            LayerExpression::Mono,
            "and the good line AFTER the mess still loads"
        );
    }

    /// An older exe reading a newer file must not delete the keys it never
    /// knew — neither on save nor on "forget this type".
    #[test]
    fn unknown_keys_survive_a_downgrade() {
        let mut d = LayerDefaults::parse(
            "raster.opacity=0.5\nraster.from_2027=on\nhologram.opacity=0.1\n",
        );
        d.capture(&raster());
        let body = d.to_body();
        assert!(body.contains("raster.from_2027=on"), "{body}");
        assert!(body.contains("hologram.opacity=0.1"), "{body}");
        d.forget("raster");
        let body = d.to_body();
        assert!(body.contains("raster.from_2027=on"), "{body}");
        assert!(!body.contains("raster.opacity"), "{body}");
    }

    /// A default must not reach where the matching command refuses to go:
    /// `set_tone` refuses folders and derived layers, `set_edge` refuses
    /// frame folders.
    #[test]
    fn a_tone_default_is_refused_where_the_tone_command_would_be() {
        let mut src = raster();
        src.tone = Some(ToneParams::default());
        src.edge = Some(EdgeParams {
            width_px: 2.0,
            colour: [0, 0, 0],
            style: Default::default(),
        });
        let mut d = LayerDefaults::default();
        d.capture(&src);
        // Re-file the same block under the folder and frame-folder types so
        // `apply` is the thing under test, not `kind_key`.
        let folder_defaults = LayerDefaults::parse(&d.to_body().replace("raster.", "folder."));

        let mut f = raster();
        f.folder = true;
        folder_defaults.apply(&mut f);
        assert_eq!(f.tone, None, "folders cannot be tones");
        assert!(f.edge.is_some(), "a plain folder DOES take the edge effect");

        let frame_body = d.to_body().replace("raster.", "frame-folder.");
        let mut ff = raster();
        ff.folder = true;
        ff.kind = LayerKind::Frame(mn_core::FrameSet::single_rect([0.0, 0.0, 8.0, 8.0], 2.0));
        LayerDefaults::parse(&frame_body).apply(&mut ff);
        assert_eq!(ff.tone, None);
        assert_eq!(ff.edge, None, "frame folders refuse the border effect");
    }

    /// The live-fill payload is saved for the two STYLE kinds and read back
    /// as creation input; a gradient's geometry is deliberately not saved.
    #[test]
    fn fill_params_default_for_fill_and_tone_but_not_gradient() {
        let mut src = tone_fill();
        src.kind = LayerKind::Fill(FillKind::Tone {
            tone: ToneParams {
                lpi: 30.0,
                ..ToneParams::default()
            },
            density: 0.9,
        });
        let mut d = LayerDefaults::default();
        d.capture(&src);

        let stock = FillKind::Tone {
            tone: ToneParams::default(),
            density: 0.4,
        };
        match d.fill_kind(stock) {
            FillKind::Tone { tone, density } => {
                assert_eq!(tone.lpi, 30.0);
                assert_eq!(density, 0.9);
            }
            k => panic!("wrong kind back: {k:?}"),
        }

        // A flat fill keys apart and is untouched by the tone default.
        let flat = FillKind::Flat { color: [1.0; 4] };
        assert_eq!(d.fill_kind(flat), flat);

        // Gradients save the generic properties but no payload.
        let mut g = raster();
        g.kind = LayerKind::Fill(FillKind::Gradient {
            a: [0.0, 0.0],
            b: [10.0, 10.0],
            from: [0.0; 4],
            to: [1.0; 4],
            mid: Default::default(),
            opts: Default::default(),
        });
        g.opacity = 0.5;
        d.capture(&g);
        assert!(d.has("gradient"), "the generic properties are saved");
        assert!(
            d.summary("gradient").is_some_and(|s| !s.contains("fill")),
            "…but not the ramp geometry"
        );
    }

    /// Only the types whose creation path reads a default are offered one.
    #[test]
    fn save_as_default_is_offered_where_it_is_read() {
        for k in ["raster", "vector", "folder", "fill", "gradient", "tone", "correction"] {
            assert!(applies_to(k), "{k}");
        }
        for k in ["text", "balloon", "frame", "frame-folder"] {
            assert!(!applies_to(k), "{k}");
        }
    }
}
