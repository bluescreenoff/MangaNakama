//! Text layers — vector text items that rasterize into a layer.
//!
//! Same derived-raster idea as [`crate::frame`] / [`crate::balloon`]: a text
//! layer (`LayerKind::Text`) owns a [`TextSet`] and its pixels are regenerated
//! wholesale by `Document::set_texts` / undo. The difference is that shaping
//! glyphs needs DirectWrite, which `core` must not touch — so every
//! [`TextItem`] carries a **cached sprite** ([`RenderedText`], premultiplied
//! RGBA, outline and rotation already baked in) that the app's text engine
//! (`mn-text`) fills in before any `set_texts` call. `rasterize` is then a
//! pure blit, and undo works because the sprites ride along in the cloned
//! vector state.
//!
//! Sprites are *not* serialized. A freshly ORA-loaded text layer has no
//! caches, which is fine because the loader keeps the layer PNG raster; the
//! app re-shapes every item on a layer before the first edit touches it
//! (`Document::warm_text_caches`).
//!
//! All text indices — carets, selections, style-run lengths — are **UTF-16
//! code units**, the unit DirectWrite ranges use, so nothing ever converts at
//! the engine boundary. Helpers here guarantee indices stay on scalar
//! boundaries (never between surrogate halves).

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::tile::{FIX15_ONE, TILE_SIZE, Tile, TileIdx};

/// Boxes smaller than this per axis (px) are refused by handle drags.
pub const MIN_TEXT_EXTENT: f32 = 8.0;

/// One styled span, `len` in UTF-16 units. Invariants (kept by every editing
/// op here): no zero-length runs, adjacent runs with equal style are merged,
/// lengths sum to the text's UTF-16 length (no runs at all iff text is empty).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleRun {
    pub len: u32,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub strike: bool,
}

impl StyleRun {
    pub fn plain(len: u32) -> Self {
        Self {
            len,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
        }
    }

    fn same_style(a: Self, b: Self) -> bool {
        a.bold == b.bold
            && a.italic == b.italic
            && a.underline == b.underline
            && a.strike == b.strike
    }
}

/// Which of the four flags an editing op targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleFlag {
    Bold,
    Italic,
    Underline,
    Strike,
}

impl StyleRun {
    pub fn get(&self, f: StyleFlag) -> bool {
        match f {
            StyleFlag::Bold => self.bold,
            StyleFlag::Italic => self.italic,
            StyleFlag::Underline => self.underline,
            StyleFlag::Strike => self.strike,
        }
    }

    fn set(&mut self, f: StyleFlag, on: bool) {
        match f {
            StyleFlag::Bold => self.bold = on,
            StyleFlag::Italic => self.italic = on,
            StyleFlag::Underline => self.underline = on,
            StyleFlag::Strike => self.strike = on,
        }
    }
}

/// Row alignment inside the wrap box (CSP "Alignment" / basic Justify).
/// DirectWrite's `DWRITE_TEXT_ALIGNMENT` exactly: in horizontal text
/// Leading/Trailing = left/right; vertical text maps to top/bottom (the
/// engine handles the orientation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Leading,
    Center,
    Trailing,
}

/// Block position inside the wrap box (CSP "Position in frame" /
/// "Align frames"): where the text block sits on the cross axis.
/// `DWRITE_PARAGRAPH_ALIGNMENT`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameAlign {
    #[default]
    Near,
    Center,
    Far,
}

/// Line spacing (CSP "Line space" + "How to specify"): natural, a
/// percentage of the natural line height (100 = natural), or an absolute
/// line height in pt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum LineSpacing {
    #[default]
    Auto,
    /// e.g. 150.0 = 1.5× natural line height.
    Percent(f32),
    /// Absolute line height, pt at the document dpi.
    Pt(f32),
}

/// One furigana (ふりがな / ルビ) annotation: a reading set beside a range of
/// the base text. TX-062 — furigana is on nearly every printed shounen page,
/// and until now we could not set one at all.
///
/// Indices are UTF-16 over `TextItem::text`, like `StyleRun` lengths and
/// every other position in this module. Annotations are kept sorted by
/// `start` and never overlap: setting a reading over a range replaces
/// whatever readings that range already touched, because two readings for
/// one kanji is not a state a page can be in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ruby {
    pub start: u32,
    pub len: u32,
    /// The reading itself — kana in practice, but not enforced: 漢字 with a
    /// Latin gloss is a real (if rarer) manga usage.
    pub text: String,
}

impl Ruby {
    pub fn end(&self) -> u32 {
        self.start + self.len
    }
}

/// How the readings are SET, per text item — CSP's "Reading settings" panel
/// (owner, 2026-08-19: furigana works "it just needs a lot of settings you
/// can access like Clip Studio's").
///
/// Defaults are the typographic ones, not CSP's: half the base size, which
/// is what JIS X 4051 specifies. CSP ships 67 %, which is why a page set
/// there and re-set here will not match until this is turned up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RubyStyle {
    /// Reading size as a percentage of the base text.
    pub size_pct: f32,
    /// Where a reading sits ALONG its base run when it is shorter than the
    /// base: centred (CSP's "Center align", and the usual setting), or
    /// pushed to the run's start or end.
    pub align: Align,
    /// CSP "Adjust reading": nudge along the base run, pt at the document
    /// dpi. Positive moves the way the text reads.
    pub offset_pt: f32,
    /// CSP "Space between reading and main text": extra distance from the
    /// base, pt at the document dpi. 0 = sitting right against it.
    pub gap_pt: f32,
    /// A family for the readings alone. `None` = whatever the base
    /// characters are set in.
    #[serde(default)]
    pub font: Option<String>,
}

impl Default for RubyStyle {
    fn default() -> Self {
        Self {
            size_pct: 50.0,
            align: Align::Center,
            offset_pt: 0.0,
            gap_pt: 0.0,
            font: None,
        }
    }
}

/// A stretch of text set in a DIFFERENT family from the item's own (TX-064).
///
/// Manga needs this constantly and for one reason: a Japanese balloon font
/// has no Latin worth reading, so the SFX, the shout in English, the name
/// on the sign all want their own face inside an otherwise Japanese item.
///
/// Kept out of [`StyleRun`] deliberately: `StyleRun` is `Copy` and read by
/// value all over the editor, and a `String` field would have cost that —
/// a family override is rare, a bold flag is not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontRun {
    pub start: u32,
    pub len: u32,
    pub family: String,
}

impl FontRun {
    pub fn end(&self) -> u32 {
        self.start + self.len
    }
}

/// One 縦中横 (tate-chu-yoko) run: a span — digits in practice — set
/// UPRIGHT and read left-to-right inside a vertical column (TX-063).
///
/// This is how every number on a printed Japanese page is set: 22時, 3人,
/// 第1話. Left to itself a vertical run of Latin is laid on its side, one
/// character under the next (TX-061, the layout engine's own contract), so
/// without this a time or a chapter number in a balloon simply reads wrong.
///
/// Indices are UTF-16 over [`TextItem::text`], like every other span in this
/// module. Runs are kept sorted, non-overlapping, and merged when they touch
/// — two digits marked one at a time are one cell, not two half-cells.
/// Meaningless in horizontal text, where the renderer ignores the list
/// rather than inventing a second meaning for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TcyRun {
    pub start: u32,
    pub len: u32,
}

impl TcyRun {
    pub fn end(&self) -> u32 {
        self.start + self.len
    }
}

/// The longest run Auto 縦中横 will stand upright — CSP's dropdown stops at
/// 4 and so does ours, for the same reason: five digits side by side in a
/// column one em wide is not typesetting, it is a smudge.
pub const AUTO_TCY_MAX: u8 = 4;

/// Move one `(start, len)` span in response to an insertion of `add` units
/// at `at`. Text before the span pushes it along; text INSIDE extends it.
///
/// Shared by furigana, font runs and 縦中横 so they can never disagree about
/// what typing does — the bug that shape of code is famous for.
fn span_insert(start: &mut u32, len: &mut u32, at: u32, add: u32) {
    if at <= *start {
        *start += add;
    } else if at < *start + *len {
        *len += add;
    }
}

/// Clip one `(start, len)` span against a deletion of `a..b`. A span whose
/// whole range is deleted comes back with `len == 0`; callers drop those.
fn span_delete(start: &mut u32, len: &mut u32, a: u32, b: u32) {
    let cut = b - a;
    let (s, e) = (*start, *start + *len);
    let ns = if s >= b { s - cut } else { s.min(a) };
    let ne = if e >= b { e - cut } else { e.min(a) };
    *start = ns;
    *len = ne.saturating_sub(ns);
}

/// The shaped sprite for one item: premultiplied RGBA8, rotation and outline
/// baked in, positioned on the canvas at `origin`.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedText {
    /// Canvas position of the sprite's top-left pixel.
    pub origin: [i32; 2],
    pub size: [u32; 2],
    /// `size[0] * size[1] * 4` premultiplied RGBA bytes.
    pub rgba: Vec<u8>,
}

/// A named work-level text style (TX-styles, crawl TOP-15 #3): the
/// typography dialogue / thought / shout / narration items share. Editing
/// a style re-styles every item carrying its name — CSP's own tutorial
/// warns that changing the font size mid-chapter means going back over
/// every page by hand; the style IS the fix. Placement, rotation,
/// orientation and per-range runs stay the item's own business.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub name: String,
    /// Empty = the style does not pin a family (items keep their font).
    #[serde(default)]
    pub font: String,
    /// Point size at the document dpi. The UI talks Q as well
    /// (1 Q = 0.25 mm ⇒ 1 Q ≈ 0.7087 pt): JP dialogue convention is 18–20 Q.
    pub size_pt: f32,
    pub color: [u8; 3],
    /// フチ width in canvas px, 0 = none.
    #[serde(default)]
    pub outline_px: f32,
    #[serde(default = "white")]
    pub outline_color: [u8; 3],
    #[serde(default)]
    pub letter_spacing_pt: f32,
    #[serde(default)]
    pub line_spacing: LineSpacing,
}

/// 1 Q = 0.25 mm, in points (1 pt = 1/72 in = 25.4/72 mm).
pub const PT_PER_Q: f32 = 0.25 / 25.4 * 72.0;

impl TextStyle {
    /// Stamp this style's typography onto an item and drop its sprite (the
    /// caller re-shapes). The item now carries the style's name.
    pub fn apply(&self, item: &mut TextItem) {
        item.style = Some(self.name.clone());
        if !self.font.is_empty() {
            item.font = self.font.clone();
        }
        item.size_pt = self.size_pt;
        item.color = self.color;
        item.outline_px = self.outline_px;
        item.outline_color = self.outline_color;
        item.letter_spacing_pt = self.letter_spacing_pt;
        item.line_spacing = self.line_spacing;
        item.cache = None;
    }

    /// The fresh-work set, JP-convention sizes. No font is pinned — the
    /// first font the user gives a style sticks from then on.
    pub fn defaults() -> Vec<TextStyle> {
        let base = |name: &str, q: f32| TextStyle {
            name: name.into(),
            font: String::new(),
            size_pt: q * PT_PER_Q,
            color: [0, 0, 0],
            outline_px: 0.0,
            outline_color: [255, 255, 255],
            letter_spacing_pt: 0.0,
            line_spacing: LineSpacing::Percent(150.0),
        };
        vec![
            base("Dialogue", 20.0),
            base("Thought", 18.0),
            TextStyle {
                line_spacing: LineSpacing::Auto,
                ..base("Shout", 28.0)
            },
            base("Narration", 16.0),
        ]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextItem {
    pub text: String,
    /// Style spans over `text`, UTF-16 lengths.
    pub runs: Vec<StyleRun>,
    /// Top-left of the *unrotated* layout box, canvas px.
    pub pos: [f32; 2],
    /// Layout box (wrap bounds), canvas px.
    pub size: [f32; 2],
    /// While true the box follows the layout's natural metrics; any explicit
    /// resize turns it off.
    #[serde(default)]
    pub auto_size: bool,
    /// Radians, clockwise, around the box center.
    #[serde(default)]
    pub rotation: f32,
    /// DirectWrite family name (as enumerated, i.e. possibly Japanese).
    pub font: String,
    /// Point size at the document's dpi.
    pub size_pt: f32,
    pub color: [u8; 3],
    /// Edge (フチ) width in canvas px, 0 = none.
    #[serde(default)]
    pub outline_px: f32,
    #[serde(default = "white")]
    pub outline_color: [u8; 3],
    /// Vertical Japanese layout (columns right-to-left).
    #[serde(default)]
    pub vertical: bool,
    /// Row alignment inside the wrap box (round 34).
    #[serde(default)]
    pub align: Align,
    /// Block position inside the wrap box (round 34).
    #[serde(default)]
    pub frame_align: FrameAlign,
    /// Extra inter-character spacing, pt at the document dpi (negative =
    /// tighter). CSP "Character spacing" — whole item in v1.
    #[serde(default)]
    pub letter_spacing_pt: f32,
    /// Line spacing override (round 34). `Auto` = the font's natural
    /// metrics, bit-identical to pre-round-34 items (no DirectWrite call).
    #[serde(default)]
    pub line_spacing: LineSpacing,
    /// Furigana annotations (TX-062), sorted by `start`, non-overlapping.
    /// `serde(default)` is load-bearing: every text item written before this
    /// field existed must keep opening.
    #[serde(default)]
    pub ruby: Vec<Ruby>,
    /// How those readings are set (size, alignment, gap). `serde(default)`
    /// so items written before the panel existed keep their JIS defaults.
    #[serde(default)]
    pub ruby_style: RubyStyle,
    /// Per-range font overrides (TX-064), sorted by `start`, non-overlapping.
    /// Empty = the whole item is set in `font`, which is every item written
    /// before this field existed.
    #[serde(default)]
    pub fonts: Vec<FontRun>,
    /// 縦中横 runs (TX-063), sorted, non-overlapping, merged when touching.
    /// Empty = every character is set the way its script says, which is
    /// every item written before this field existed.
    #[serde(default)]
    pub tcy: Vec<TcyRun>,
    /// Auto 縦中横 (TX-062): stand any run of up to this many half-width
    /// alphanumerics upright without anyone selecting it. `0` = off, which
    /// is every item written before this field existed and the state in
    /// which `tcy` alone decides. See [`Self::auto_tcy_runs`].
    #[serde(default)]
    pub auto_tcy: u8,
    /// TX-styles: the work style this item follows; editing that style
    /// re-styles every item carrying its name, chapter-wide. `None` =
    /// free-styled, which is every item written before styles existed.
    #[serde(default)]
    pub style: Option<String>,
    /// Shaped sprite, filled by the app's text engine. Never serialized;
    /// deliberately excluded from equality.
    #[serde(skip)]
    pub cache: Option<Arc<RenderedText>>,
}

fn white() -> [u8; 3] {
    [255, 255, 255]
}

/// Model equality ignores the cache (a loaded file has no sprites yet).
impl PartialEq for TextItem {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && self.runs == other.runs
            && self.pos == other.pos
            && self.size == other.size
            && self.auto_size == other.auto_size
            && self.rotation == other.rotation
            && self.font == other.font
            && self.size_pt == other.size_pt
            && self.color == other.color
            && self.outline_px == other.outline_px
            && self.outline_color == other.outline_color
            && self.vertical == other.vertical
            && self.align == other.align
            && self.frame_align == other.frame_align
            && self.letter_spacing_pt == other.letter_spacing_pt
            && self.line_spacing == other.line_spacing
            && self.ruby == other.ruby
            && self.ruby_style == other.ruby_style
            && self.fonts == other.fonts
            && self.tcy == other.tcy
            && self.auto_tcy == other.auto_tcy
            && self.style == other.style
    }
}

/// Which control point of a text box an Object-tool drag grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextHandle {
    /// 0=(x0,y0) 1=(x1,y0) 2=(x1,y1) 3=(x0,y1), in unrotated box space.
    Corner(usize),
    /// Midpoints: 0=top 1=right 2=bottom 3=left.
    Edge(usize),
    /// The lollipop above the top edge; dragging it rotates around center.
    Rotate,
}

// --- UTF-16 index helpers (free functions on &str) --------------------------

/// UTF-16 length of `s`.
pub fn utf16_len(s: &str) -> u32 {
    s.chars().map(|c| c.len_utf16() as u32).sum()
}

/// Byte offset of UTF-16 position `u` (clamped to the end; lands on a scalar
/// boundary — an index inside a surrogate pair snaps past it).
pub fn utf16_to_byte(s: &str, u: u32) -> usize {
    let mut units = 0u32;
    for (b, c) in s.char_indices() {
        if units >= u {
            return b;
        }
        units += c.len_utf16() as u32;
    }
    s.len()
}

/// One GRAPHEME CLUSTER forward from `u` (clamped). Combining marks and
/// ZWJ emoji sequences step as one user-perceived character (plans/05
/// item 5): these power the arrows and Backspace/Delete, and a caret
/// that splits 👨‍👩‍👧 or e+U+0301 is wrong everywhere a user can see it.
/// The one audit-note alternative — routing through DirectWrite's
/// GetClusterMetrics — was rejected for layering: mn-core is
/// platform-free by contract and this runs on every keypress.
pub fn next_boundary(s: &str, u: u32) -> u32 {
    use unicode_segmentation::UnicodeSegmentation;
    let mut units = 0u32;
    for (_, g) in s.grapheme_indices(true) {
        let l: u32 = g.chars().map(|c| c.len_utf16() as u32).sum();
        if units + l > u {
            return units + l;
        }
        units += l;
    }
    units
}

/// One grapheme cluster back from `u` (clamped to 0) — the cluster
/// START, so a mid-cluster position (which stepping never produces, but
/// a drag or an IME commit might) still rounds somewhere sane.
pub fn prev_boundary(s: &str, u: u32) -> u32 {
    use unicode_segmentation::UnicodeSegmentation;
    let mut prev = 0u32;
    let mut units = 0u32;
    for (_, g) in s.grapheme_indices(true) {
        if units >= u {
            break;
        }
        prev = units;
        units += g.chars().map(|c| c.len_utf16() as u32).sum::<u32>();
    }
    prev
}

/// Character classes for Ctrl+arrow word motion. Splitting kanji from kana
/// makes 漢字かな boundaries jump points, which is how JP editors behave.
#[derive(PartialEq, Eq, Clone, Copy)]
enum CharClass {
    Space,
    Word, // ASCII alnum + underscore
    Hiragana,
    Katakana,
    Kanji,
    Other, // punctuation and everything else
}

fn class_of(c: char) -> CharClass {
    match c {
        c if c.is_whitespace() => CharClass::Space,
        c if c.is_ascii_alphanumeric() || c == '_' => CharClass::Word,
        // Fullwidth latin (plans/05 item 5): ＡＺａｚ and ０-９ are one Word
        // run, so Ctrl+Right over ＡＢＣ１２３ jumps the whole run like it
        // does over ABC123. The fullwidth PUNCTUATION around the zone stays
        // Other, and halfwidth katakana keeps its Katakana arm below — no
        // double-classify.
        '\u{FF10}'..='\u{FF19}' | '\u{FF21}'..='\u{FF3A}' | '\u{FF41}'..='\u{FF5A}' => {
            CharClass::Word
        }
        '\u{3040}'..='\u{309F}' => CharClass::Hiragana,
        '\u{30A0}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' | '\u{FF66}'..='\u{FF9D}' => {
            CharClass::Katakana
        }
        '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}' | '々' => {
            CharClass::Kanji
        }
        _ => CharClass::Other,
    }
}

/// Ctrl+Right target: skip the run of the current class, then any whitespace.
pub fn next_word_boundary(s: &str, u: u32) -> u32 {
    let b = utf16_to_byte(s, u);
    let mut it = s[b..].chars().peekable();
    let mut pos = u;
    let Some(&first) = it.peek() else { return pos };
    let cls = class_of(first);
    while let Some(&c) = it.peek() {
        if class_of(c) != cls {
            break;
        }
        pos += c.len_utf16() as u32;
        it.next();
    }
    if cls != CharClass::Space {
        while let Some(&c) = it.peek() {
            if class_of(c) != CharClass::Space {
                break;
            }
            pos += c.len_utf16() as u32;
            it.next();
        }
    }
    pos
}

/// Ctrl+Left target: skip whitespace backwards, then the run of the class of
/// the character before that.
pub fn prev_word_boundary(s: &str, u: u32) -> u32 {
    let b = utf16_to_byte(s, u);
    let mut it = s[..b].chars().rev().peekable();
    let mut pos = u;
    while let Some(&c) = it.peek() {
        if class_of(c) != CharClass::Space {
            break;
        }
        pos -= c.len_utf16() as u32;
        it.next();
    }
    let Some(&first) = it.peek() else { return pos };
    let cls = class_of(first);
    while let Some(&c) = it.peek() {
        if class_of(c) != cls {
            break;
        }
        pos -= c.len_utf16() as u32;
        it.next();
    }
    pos
}

/// Double-click target: the run of same-class characters around `u` — one
/// Latin word, one kanji run, one kana run, one run of punctuation.
///
/// NOT `prev_word_boundary`..`next_word_boundary`: those are the Ctrl+arrow
/// targets and the forward one swallows the space AFTER the word, so a
/// double-click built out of them selects "word ␣" and typing over it eats the
/// gap. A line break never joins a run either — double-clicking at the end of
/// a line selects that line's last word, not the two words either side of the
/// break.
pub fn word_range(s: &str, u: u32) -> (u32, u32) {
    let mut cs: Vec<(u32, char)> = Vec::new();
    let mut n = 0u32;
    for c in s.chars() {
        cs.push((n, c));
        n += c.len_utf16() as u32;
    }
    // The character `u` sits in front of; at the end of the text (or in front
    // of a line break) the one behind it, so a caret past the last glyph still
    // has a word to select.
    let mut hit = match cs.iter().position(|&(i, c)| u < i + c.len_utf16() as u32) {
        Some(i) => i,
        None => match cs.len().checked_sub(1) {
            Some(i) => i,
            None => return (0, 0),
        },
    };
    // A caret in front of whitespace belongs to the word BEHIND it: clicking
    // the gap after a word — or at the end of a line — selects that word.
    if cs[hit].1.is_whitespace()
        && let Some(i) = hit.checked_sub(1)
        && !cs[i].1.is_whitespace()
    {
        hit = i;
    }
    if cs[hit].1 == '\n' {
        return (cs[hit].0, cs[hit].0);
    }
    let cls = class_of(cs[hit].1);
    let (mut a, mut b) = (hit, hit);
    while a > 0 && cs[a - 1].1 != '\n' && class_of(cs[a - 1].1) == cls {
        a -= 1;
    }
    while b + 1 < cs.len() && cs[b + 1].1 != '\n' && class_of(cs[b + 1].1) == cls {
        b += 1;
    }
    (cs[a].0, cs[b].0 + cs[b].1.len_utf16() as u32)
}

// --- rotation helpers -------------------------------------------------------

fn rot(v: [f32; 2], a: f32) -> [f32; 2] {
    let (s, c) = a.sin_cos();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c]
}

impl TextItem {
    pub fn new(pos: [f32; 2], font: String, size_pt: f32, color: [u8; 3], vertical: bool) -> Self {
        Self {
            text: String::new(),
            runs: Vec::new(),
            pos,
            size: [0.0, 0.0],
            auto_size: true,
            rotation: 0.0,
            font,
            size_pt,
            color,
            outline_px: 0.0,
            outline_color: white(),
            vertical,
            align: Align::default(),
            frame_align: FrameAlign::default(),
            letter_spacing_pt: 0.0,
            line_spacing: LineSpacing::default(),
            ruby: Vec::new(),
            ruby_style: RubyStyle::default(),
            fonts: Vec::new(),
            tcy: Vec::new(),
            auto_tcy: 0,
            style: None,
            cache: None,
        }
    }

    /// The runs Auto 縦中横 finds for itself (TX-062): every maximal run of
    /// half-width alphanumerics no longer than `auto_tcy` characters.
    ///
    /// This is the setting that makes vertical Japanese work without anyone
    /// thinking about it — 第1話, 22時, 3年 are typed and come out upright.
    /// A run LONGER than the limit is left alone rather than shrunk to fit:
    /// a phone number standing upright in a one-em cell is unreadable, and
    /// CSP's dropdown stops at 4 for exactly that reason.
    ///
    /// Derived, never stored. Two consequences, both deliberate: turning the
    /// setting off restores the page exactly (nothing was written into
    /// `tcy`), and typing a digit next to an existing pair re-evaluates the
    /// whole run instead of leaving a stale two-character cell behind.
    ///
    /// A hand-marked run WINS: any auto candidate that overlaps one is
    /// dropped, so the manual toggle keeps meaning what it says over the
    /// spans it covers. (Auto cannot be *cancelled* per-span — CSP has the
    /// same hole and tells you to set Auto to None first. Manual's own
    /// off-switch only clears what `tcy` holds.)
    pub fn auto_tcy_runs(&self) -> Vec<TcyRun> {
        if self.auto_tcy == 0 {
            return Vec::new();
        }
        let limit = self.auto_tcy.min(AUTO_TCY_MAX) as u32;
        let mut out: Vec<TcyRun> = Vec::new();
        // Runs are counted in CHARACTERS (what the CSP dropdown means) but
        // stored in UTF-16 units like every other span here. ASCII is one
        // unit per character, so the two agree — the separate counter is
        // there so the invariant is visible rather than assumed.
        let (mut at, mut start, mut chars) = (0u32, 0u32, 0u32);
        let flush = |out: &mut Vec<TcyRun>, start: u32, end: u32, chars: u32| {
            if chars > 0 && chars <= limit {
                out.push(TcyRun {
                    start,
                    len: end - start,
                });
            }
        };
        for c in self.text.chars() {
            if c.is_ascii_alphanumeric() {
                if chars == 0 {
                    start = at;
                }
                chars += 1;
            } else {
                flush(&mut out, start, at, chars);
                chars = 0;
            }
            at += c.len_utf16() as u32;
        }
        flush(&mut out, start, at, chars);
        out.retain(|a| {
            !self
                .tcy
                .iter()
                .any(|m| a.start < m.end() && m.start < a.end())
        });
        out
    }

    /// Every 縦中横 run the renderer should honour: the hand-marked ones plus
    /// whatever Auto found, sorted by `start` and non-overlapping.
    ///
    /// NOT merged where two runs touch, unlike [`Self::set_tcy`]: a manual
    /// run that happens to end where an auto run begins is two decisions and
    /// therefore two cells, and fusing them would silently widen one of
    /// them.
    pub fn effective_tcy(&self) -> Vec<TcyRun> {
        if self.auto_tcy == 0 {
            return self.tcy.clone();
        }
        let mut out = self.tcy.clone();
        out.extend(self.auto_tcy_runs());
        out.sort_by_key(|t| t.start);
        out
    }

    /// Turn 縦中横 on or off over UTF-16 range `a..b` (TX-063). Returns
    /// whether anything changed, so callers can skip an undo step.
    ///
    /// Same split-and-merge shape as [`Self::set_font_range`], and for the
    /// same reason: turning it OFF over the middle of a run has to leave the
    /// characters on either side of the selection upright, and turning it ON
    /// beside an existing run has to give one cell rather than two — 22 is a
    /// single square on the page whether it was marked in one gesture or in
    /// two. (Furigana replaces instead, because one kanji cannot carry two
    /// readings; a stretch of digits has no such conflict to resolve.)
    pub fn set_tcy(&mut self, a: u32, b: u32, on: bool) -> bool {
        let n = self.utf16_len();
        let (a, b) = (a.min(b).min(n), a.max(b).min(n));
        if a == b {
            return false;
        }
        let before = self.tcy.clone();
        // Keep the `start..a` and `b..end` remnants of everything the range
        // touches; a run outside it falls entirely into one of the two.
        let mut out: Vec<TcyRun> = Vec::with_capacity(self.tcy.len() + 2);
        for t in self.tcy.drain(..) {
            let (s, e) = (t.start, t.end());
            if s < a {
                out.push(TcyRun {
                    start: s,
                    len: e.min(a) - s,
                });
            }
            if e > b {
                let s = s.max(b);
                out.push(TcyRun {
                    start: s,
                    len: e - s,
                });
            }
        }
        if on {
            out.push(TcyRun {
                start: a,
                len: b - a,
            });
        }
        out.sort_by_key(|t| t.start);
        self.tcy = out.into_iter().fold(Vec::new(), |mut acc, t| {
            match acc.last_mut() {
                Some(p) if p.end() == t.start => p.len += t.len,
                _ => acc.push(t),
            }
            acc
        });
        let changed = self.tcy != before;
        if changed {
            self.cache = None;
        }
        changed
    }

    /// Set (or clear) the font family over UTF-16 range `a..b` (TX-064).
    ///
    /// An override the range only partly covers is **split**, not dropped:
    /// setting a line in Arial and then three characters of it in Impact is
    /// the ordinary gesture, and the Arial on either side has to survive it.
    /// (Furigana replaces instead — see [`Self::set_ruby`] — because one
    /// kanji cannot carry two readings, which is not true of two faces.)
    ///
    /// Passing the item's own `font` clears the override rather than storing
    /// a redundant one — otherwise changing the item font later would leave
    /// islands of the old face behind with no way to see why. With the split
    /// above, that clears the selection alone and leaves the rest standing.
    pub fn set_font_range(&mut self, a: u32, b: u32, family: &str) -> bool {
        let n = self.utf16_len();
        let (a, b) = (a.min(b).min(n), a.max(b).min(n));
        if a == b {
            return false;
        }
        let before = self.fonts.clone();
        // Keep the `start..a` and `b..end` remnants of everything the range
        // touches; a run outside it falls entirely into one of the two.
        let mut out: Vec<FontRun> = Vec::with_capacity(self.fonts.len() + 2);
        for f in self.fonts.drain(..) {
            let (s, e) = (f.start, f.end());
            if s < a {
                out.push(FontRun {
                    start: s,
                    len: e.min(a) - s,
                    family: f.family.clone(),
                });
            }
            if e > b {
                let s = s.max(b);
                out.push(FontRun {
                    start: s,
                    len: e - s,
                    family: f.family,
                });
            }
        }
        let family = family.trim();
        if !family.is_empty() && family != self.font {
            out.push(FontRun {
                start: a,
                len: b - a,
                family: family.to_owned(),
            });
        }
        out.sort_by_key(|f| f.start);
        // Touching pieces of one family are one run again. Without this,
        // re-setting part of a run in the face it already had would leave a
        // seam behind and report a change nothing on screen can show.
        self.fonts = out.into_iter().fold(Vec::new(), |mut acc, f| {
            match acc.last_mut() {
                Some(p) if p.end() == f.start && p.family == f.family => p.len += f.len,
                _ => acc.push(f),
            }
            acc
        });
        let changed = self.fonts != before;
        if changed {
            self.cache = None;
        }
        changed
    }

    /// The family in force at UTF-16 position `pos` — an override if one
    /// covers it, else the item's own.
    pub fn font_at(&self, pos: u32) -> &str {
        self.fonts
            .iter()
            .find(|f| pos >= f.start && pos < f.end())
            .map(|f| f.family.as_str())
            .unwrap_or(&self.font)
    }

    /// Set (or clear) the reading over UTF-16 range `a..b`.
    ///
    /// An empty `reading` clears; otherwise every annotation the range
    /// touches is replaced, because one kanji cannot carry two readings.
    /// Returns whether anything changed, so callers can skip an undo step.
    pub fn set_ruby(&mut self, a: u32, b: u32, reading: &str) -> bool {
        let n = self.utf16_len();
        let (a, b) = (a.min(b).min(n), a.max(b).min(n));
        if a == b {
            return false;
        }
        let before = self.ruby.clone();
        // Overlap, not containment: a partial overlap is still a conflict.
        self.ruby.retain(|r| r.end() <= a || r.start >= b);
        let reading = reading.trim();
        if !reading.is_empty() {
            self.ruby.push(Ruby {
                start: a,
                len: b - a,
                text: reading.to_owned(),
            });
            self.ruby.sort_by_key(|r| r.start);
        }
        let changed = self.ruby != before;
        if changed {
            self.cache = None;
        }
        changed
    }

    /// The annotation covering UTF-16 position `pos`, if any — what the UI
    /// shows when the caret lands inside an annotated word.
    pub fn ruby_at(&self, pos: u32) -> Option<&Ruby> {
        self.ruby.iter().find(|r| pos >= r.start && pos < r.end())
    }

    pub fn utf16_len(&self) -> u32 {
        utf16_len(&self.text)
    }

    pub fn center(&self) -> [f32; 2] {
        [
            self.pos[0] + self.size[0] * 0.5,
            self.pos[1] + self.size[1] * 0.5,
        ]
    }

    /// Canvas point → unrotated box-local point (origin at `pos`).
    pub fn to_local(&self, p: [f32; 2]) -> [f32; 2] {
        let c = self.center();
        let v = rot([p[0] - c[0], p[1] - c[1]], -self.rotation);
        [v[0] + self.size[0] * 0.5, v[1] + self.size[1] * 0.5]
    }

    /// Unrotated box-local point → canvas point.
    pub fn to_canvas(&self, p: [f32; 2]) -> [f32; 2] {
        let c = self.center();
        let v = rot(
            [p[0] - self.size[0] * 0.5, p[1] - self.size[1] * 0.5],
            self.rotation,
        );
        [v[0] + c[0], v[1] + c[1]]
    }

    /// Is the canvas point inside the (rotated) box, grown by `slack` px?
    pub fn contains(&self, p: [f32; 2], slack: f32) -> bool {
        let l = self.to_local(p);
        l[0] >= -slack
            && l[1] >= -slack
            && l[0] <= self.size[0] + slack
            && l[1] <= self.size[1] + slack
    }

    /// The four rotated box corners, canvas space (for overlays).
    pub fn corners(&self) -> [[f32; 2]; 4] {
        let [w, h] = self.size;
        [
            self.to_canvas([0.0, 0.0]),
            self.to_canvas([w, 0.0]),
            self.to_canvas([w, h]),
            self.to_canvas([0.0, h]),
        ]
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.pos[0] += dx;
        self.pos[1] += dy;
        if let Some(c) = &mut self.cache {
            let c = Arc::make_mut(c);
            c.origin[0] += dx.round() as i32;
            c.origin[1] += dy.round() as i32;
        }
    }

    /// Every draggable control point in canvas space. `rotate_offset` is how
    /// far the rotate lollipop floats above the top edge (screen-scaled by the
    /// caller).
    pub fn handles(&self, rotate_offset: f32) -> Vec<([f32; 2], TextHandle)> {
        let [w, h] = self.size;
        let mut out = vec![
            (self.to_canvas([0.0, 0.0]), TextHandle::Corner(0)),
            (self.to_canvas([w, 0.0]), TextHandle::Corner(1)),
            (self.to_canvas([w, h]), TextHandle::Corner(2)),
            (self.to_canvas([0.0, h]), TextHandle::Corner(3)),
            (self.to_canvas([w * 0.5, 0.0]), TextHandle::Edge(0)),
            (self.to_canvas([w, h * 0.5]), TextHandle::Edge(1)),
            (self.to_canvas([w * 0.5, h]), TextHandle::Edge(2)),
            (self.to_canvas([0.0, h * 0.5]), TextHandle::Edge(3)),
        ];
        out.push((
            self.to_canvas([w * 0.5, -rotate_offset]),
            TextHandle::Rotate,
        ));
        out
    }

    pub fn handle_near(&self, p: [f32; 2], radius: f32, rotate_offset: f32) -> Option<TextHandle> {
        let mut best: Option<(TextHandle, f32)> = None;
        for (pos, h) in self.handles(rotate_offset) {
            let d = ((pos[0] - p[0]).powi(2) + (pos[1] - p[1]).powi(2)).sqrt();
            if d <= radius && best.map_or(true, |(_, bd)| d < bd) {
                best = Some((h, d));
            }
        }
        best.map(|(h, _)| h)
    }

    /// Drag `handle` to canvas point `p`. Corner/edge drags keep the opposite
    /// corner/edge fixed *in canvas space* (correct even while rotated) and
    /// turn `auto_size` off; the rotate handle sets `rotation`.
    pub fn apply_handle(&mut self, handle: TextHandle, p: [f32; 2]) {
        match handle {
            TextHandle::Rotate => {
                let c = self.center();
                self.rotation = (p[1] - c[1]).atan2(p[0] - c[0]) + std::f32::consts::FRAC_PI_2;
            }
            TextHandle::Corner(i) => {
                let [w, h] = self.size;
                let fixed = match i {
                    0 => [w, h],
                    1 => [0.0, h],
                    2 => [0.0, 0.0],
                    _ => [w, 0.0],
                };
                let pl = self.to_local(p);
                self.rebox(fixed, pl, [true, true]);
            }
            TextHandle::Edge(i) => {
                let [w, h] = self.size;
                let (fixed, axes) = match i {
                    0 => ([w * 0.5, h], [false, true]), // drag top, bottom fixed
                    1 => ([0.0, h * 0.5], [true, false]),
                    2 => ([w * 0.5, 0.0], [false, true]),
                    _ => ([w, h * 0.5], [true, false]),
                };
                let pl = self.to_local(p);
                self.rebox(fixed, pl, axes);
            }
        }
    }

    /// Rebuild the box between local points `fixed` and `drag` (per enabled
    /// axis; a disabled axis keeps its current extent and the fixed point's
    /// side). The fixed point keeps its canvas position.
    fn rebox(&mut self, fixed: [f32; 2], drag: [f32; 2], axes: [bool; 2]) {
        let old_size = self.size;
        let mut lo = [0.0f32; 2];
        let mut hi = [0.0f32; 2];
        for a in 0..2 {
            if axes[a] {
                let f = fixed[a];
                let sign = if drag[a] >= f { 1.0 } else { -1.0 };
                let ext = (drag[a] - f).abs().max(MIN_TEXT_EXTENT);
                let d = f + sign * ext;
                lo[a] = f.min(d);
                hi[a] = f.max(d);
            } else {
                lo[a] = 0.0;
                hi[a] = old_size[a];
            }
        }
        // Canvas position of the fixed local point must not move.
        let anchor_canvas = self.to_canvas(fixed);
        let new_size = [hi[0] - lo[0], hi[1] - lo[1]];
        // Where the fixed point sits inside the NEW box, locally.
        let anchor_new = [fixed[0] - lo[0], fixed[1] - lo[1]];
        self.size = new_size;
        self.auto_size = false;
        // Solve pos so that to_canvas(anchor_new) == anchor_canvas.
        let c_off = rot(
            [
                anchor_new[0] - new_size[0] * 0.5,
                anchor_new[1] - new_size[1] * 0.5,
            ],
            self.rotation,
        );
        let center = [anchor_canvas[0] - c_off[0], anchor_canvas[1] - c_off[1]];
        self.pos = [center[0] - new_size[0] * 0.5, center[1] - new_size[1] * 0.5];
    }

    // --- editing ops (all indices UTF-16, kept on scalar boundaries) --------

    /// Style governing the caret at `pos`: the run containing the previous
    /// character, or the first run when at the start (plain when empty).
    pub fn style_at(&self, pos: u32) -> StyleRun {
        if self.runs.is_empty() {
            return StyleRun::plain(0);
        }
        let mut acc = 0u32;
        for r in &self.runs {
            acc += r.len;
            if pos <= acc {
                return *r;
            }
        }
        *self.runs.last().unwrap()
    }

    /// Insert `s` at UTF-16 position `at`, inheriting the style at the caret.
    pub fn insert(&mut self, at: u32, s: &str) {
        if s.is_empty() {
            return;
        }
        let at = at.min(self.utf16_len());
        let b = utf16_to_byte(&self.text, at);
        self.text.insert_str(b, s);
        let add = utf16_len(s);
        if self.runs.is_empty() {
            self.runs.push(StyleRun::plain(add));
        } else {
            // Grow the run containing the caret (the char before `at`).
            let mut acc = 0u32;
            let mut grown = false;
            for r in &mut self.runs {
                acc += r.len;
                if at <= acc {
                    r.len += add;
                    grown = true;
                    break;
                }
            }
            if !grown {
                self.runs.last_mut().unwrap().len += add;
            }
        }
        // Furigana, font overrides and 縦中横 ride the same edit through one
        // shared rule. Text inserted BEFORE a span pushes it along; inserted
        // INSIDE one extends it rather than dropping it — you are editing
        // that word, and a reading you can see and correct beats one that
        // vanished while you typed.
        for r in &mut self.ruby {
            span_insert(&mut r.start, &mut r.len, at, add);
        }
        for f in &mut self.fonts {
            span_insert(&mut f.start, &mut f.len, at, add);
        }
        for t in &mut self.tcy {
            span_insert(&mut t.start, &mut t.len, at, add);
        }
        self.cache = None;
    }

    /// Delete UTF-16 range `a..b` (order-normalized, clamped).
    pub fn delete_range(&mut self, a: u32, b: u32) {
        let n = self.utf16_len();
        let (a, b) = (a.min(b).min(n), a.max(b).min(n));
        if a == b {
            return;
        }
        let (ba, bb) = (utf16_to_byte(&self.text, a), utf16_to_byte(&self.text, b));
        self.text.replace_range(ba..bb, "");
        // Trim `b - a` units out of the runs.
        let mut remain = b - a;
        let mut acc = 0u32;
        for r in &mut self.runs {
            let start = acc;
            acc += r.len;
            if acc <= a {
                continue;
            }
            let cut_start = a.max(start) - start;
            let cut = (r.len - cut_start).min(remain);
            r.len -= cut;
            remain -= cut;
            if remain == 0 {
                break;
            }
        }
        self.runs.retain(|r| r.len > 0);
        self.normalize_runs();
        // Clip the spans to the same cut. One whose text is gone entirely
        // goes with it — a reading with nothing to read is not something to
        // keep alive out of politeness, and neither is a font override over
        // characters that no longer exist.
        for r in &mut self.ruby {
            span_delete(&mut r.start, &mut r.len, a, b);
        }
        self.ruby.retain(|r| r.len > 0);
        for f in &mut self.fonts {
            span_delete(&mut f.start, &mut f.len, a, b);
        }
        self.fonts.retain(|f| f.len > 0);
        for t in &mut self.tcy {
            span_delete(&mut t.start, &mut t.len, a, b);
        }
        self.tcy.retain(|t| t.len > 0);
        self.cache = None;
    }

    /// True iff every character in `a..b` has `flag` set (false for an empty
    /// range) — drives toggle semantics: set unless all set, else clear.
    pub fn range_has_all(&self, a: u32, b: u32, flag: StyleFlag) -> bool {
        let (a, b) = (a.min(b), a.max(b));
        if a == b {
            return false;
        }
        let mut acc = 0u32;
        for r in &self.runs {
            let start = acc;
            acc += r.len;
            if acc <= a {
                continue;
            }
            if start >= b {
                break;
            }
            if !r.get(flag) {
                return false;
            }
        }
        true
    }

    /// Set `flag` to `on` over UTF-16 range `a..b`.
    pub fn set_style(&mut self, a: u32, b: u32, flag: StyleFlag, on: bool) {
        let n = self.utf16_len();
        let (a, b) = (a.min(b).min(n), a.max(b).min(n));
        if a == b {
            return;
        }
        let mut out: Vec<StyleRun> = Vec::with_capacity(self.runs.len() + 2);
        let mut acc = 0u32;
        for r in &self.runs {
            let start = acc;
            let end = acc + r.len;
            acc = end;
            // Split into up-to-three pieces: before, inside, after.
            let cut_a = a.clamp(start, end);
            let cut_b = b.clamp(start, end);
            for (s, e, styled) in [
                (start, cut_a, false),
                (cut_a, cut_b, true),
                (cut_b, end, false),
            ] {
                if e > s {
                    let mut piece = *r;
                    piece.len = e - s;
                    if styled {
                        piece.set(flag, on);
                    }
                    out.push(piece);
                }
            }
        }
        self.runs = out;
        self.normalize_runs();
        self.cache = None;
    }

    fn normalize_runs(&mut self) {
        let mut out: Vec<StyleRun> = Vec::with_capacity(self.runs.len());
        for r in self.runs.drain(..) {
            if r.len == 0 {
                continue;
            }
            match out.last_mut() {
                Some(last) if StyleRun::same_style(*last, r) => last.len += r.len,
                _ => out.push(r),
            }
        }
        self.runs = out;
        debug_assert_eq!(
            self.runs.iter().map(|r| r.len).sum::<u32>(),
            utf16_len(&self.text)
        );
    }
}

/// Every text item on a text layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct TextSet {
    pub texts: Vec<TextItem>,
}

impl TextSet {
    /// Topmost item whose (rotated, slack-grown) box contains `p`.
    pub fn text_at(&self, p: [f32; 2], slack: f32) -> Option<usize> {
        self.texts.iter().rposition(|t| t.contains(p, slack))
    }

    /// Blit every cached sprite into sparse tiles (source-over in item order).
    /// Items without a cache contribute nothing — the app shapes before it
    /// commits. Pure, so undo can re-derive the raster from cloned state.
    pub fn rasterize(&self, size: (u32, u32)) -> HashMap<TileIdx, Arc<Tile>> {
        let mut build: HashMap<TileIdx, Tile> = HashMap::new();
        let (cw, ch) = (size.0 as i64, size.1 as i64);
        for item in &self.texts {
            let Some(sp) = &item.cache else { continue };
            let (w, h) = (sp.size[0] as i64, sp.size[1] as i64);
            if w == 0 || h == 0 {
                continue;
            }
            let (ox, oy) = (sp.origin[0] as i64, sp.origin[1] as i64);
            let x0 = ox.max(0);
            let y0 = oy.max(0);
            let x1 = (ox + w).min(cw);
            let y1 = (oy + h).min(ch);
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            for cy in y0..y1 {
                let sy = (cy - oy) as usize;
                let row = &sp.rgba[sy * w as usize * 4..(sy + 1) * w as usize * 4];
                let mut cx = x0;
                while cx < x1 {
                    let idx = TileIdx::of_pixel(cx as i32, cy as i32);
                    let (tox, toy) = idx.origin();
                    let run_end = x1.min((tox + TILE_SIZE as i32) as i64);
                    let tile = build.entry(idx).or_insert_with(Tile::new_transparent);
                    let data = tile.data_mut();
                    for x in cx..run_end {
                        let s = (x - ox) as usize * 4;
                        let sa = row[s + 3];
                        if sa == 0 {
                            continue;
                        }
                        let o = Tile::offset((x - tox as i64) as usize, (cy - toy as i64) as usize);
                        let to15 = |v: u8| (v as u32 * FIX15_ONE / 255) as u32;
                        let (sr, sg, sb, sa15) =
                            (to15(row[s]), to15(row[s + 1]), to15(row[s + 2]), to15(sa));
                        // src-over in fix15: out = src + dst * (1 - sa)
                        let inv = FIX15_ONE - sa15;
                        for (ch_i, sv) in [sr, sg, sb, sa15].into_iter().enumerate() {
                            let d = data[o + ch_i] as u32;
                            data[o + ch_i] = (sv + ((d * inv) >> 15)).min(FIX15_ONE) as u16;
                        }
                    }
                    cx = run_end;
                }
            }
        }
        build
            .into_iter()
            .filter(|(_, t)| !t.is_blank())
            .map(|(k, v)| (k, Arc::new(v)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str) -> TextItem {
        let mut t = TextItem::new([0.0, 0.0], "Meiryo".into(), 12.0, [0, 0, 0], false);
        t.insert(0, text);
        t
    }

    #[test]
    fn utf16_helpers_respect_surrogates() {
        let s = "a𠮷b"; // 𠮷 is 2 UTF-16 units, 4 UTF-8 bytes
        assert_eq!(utf16_len(s), 4);
        assert_eq!(utf16_to_byte(s, 0), 0);
        assert_eq!(utf16_to_byte(s, 1), 1);
        assert_eq!(utf16_to_byte(s, 2), 5, "inside the pair snaps past it");
        assert_eq!(utf16_to_byte(s, 3), 5);
        assert_eq!(next_boundary(s, 1), 3, "steps over the whole pair");
        assert_eq!(prev_boundary(s, 3), 1);
        assert_eq!(prev_boundary(s, 4), 3);
        assert_eq!(next_boundary(s, 4), 4, "clamped at end");
    }

    /// Grapheme-cluster stepping (plans/05 item 5): combining marks, ZWJ
    /// emoji and flag pairs are ONE step for the arrows, Backspace and
    /// Delete — a base character never loses its accents, a family emoji
    /// never loses a member.
    #[test]
    fn clusters_step_as_one() {
        // e + COMBINING ACUTE: one cluster, 2 UTF-16 units.
        let acc = "e\u{0301}x";
        assert_eq!(next_boundary(acc, 0), 2, "the accent rides along");
        assert_eq!(prev_boundary(acc, 2), 0, "and back");
        assert_eq!(prev_boundary(acc, 1), 0, "mid-cluster rounds to its start");
        assert_eq!(next_boundary(acc, 1), 2, "mid-cluster rounds to its end");
        // 説 + grade-1 dakuten (combining): one cluster for the caret even
        // though it decomposes.
        let daku = "\u{8AAC}\u{3099}！";
        assert_eq!(utf16_len(daku), 3);
        assert_eq!(next_boundary(daku, 0), 2, "the dakuten rides along");
        assert_eq!(prev_boundary(daku, 2), 0);
        // ZWJ family: 3 astral + 2 ZWJ = 8 UTF-16 units, ONE cluster.
        let fam = "👨‍👩‍👧";
        assert_eq!(utf16_len(fam), 8);
        assert_eq!(next_boundary(fam, 0), 8, "the family stays together");
        assert_eq!(prev_boundary(fam, 8), 0);
        assert_eq!(next_boundary(fam, 4), 8, "mid-sequence snaps to the end");
        assert_eq!(prev_boundary(fam, 4), 0, "...and back to the start");
        // Regional-indicator flag pair: one cluster, 4 units.
        let flag = "🇯🇵";
        assert_eq!(utf16_len(flag), 4);
        assert_eq!(next_boundary(flag, 0), 4);
        assert_eq!(prev_boundary(flag, 4), 0);
        // Plain text is unchanged.
        assert_eq!(next_boundary("abc", 1), 2);
        assert_eq!(prev_boundary("abc", 2), 1);
        assert_eq!(next_boundary("", 0), 0);
        assert_eq!(prev_boundary("", 0), 0);
    }

    #[test]
    fn word_boundaries_split_scripts() {
        let s = "hello world";
        assert_eq!(next_word_boundary(s, 0), 6, "skips word + space");
        assert_eq!(prev_word_boundary(s, 6), 0);
        assert_eq!(prev_word_boundary(s, 11), 6);

        // Kanji run then hiragana run then katakana.
        let jp = "漢字かなカナ";
        assert_eq!(next_word_boundary(jp, 0), 2, "kanji run");
        assert_eq!(next_word_boundary(jp, 2), 4, "hiragana run");
        assert_eq!(next_word_boundary(jp, 4), 6);
        assert_eq!(prev_word_boundary(jp, 6), 4);
        assert_eq!(prev_word_boundary(jp, 4), 2);
    }

    /// What a double-click selects. Script changes are word edges (a JP
    /// editor's rule), the space after a word is NOT part of it, and a line
    /// break stops the run.
    #[test]
    fn word_range_selects_one_run() {
        let s = "hello world";
        assert_eq!(word_range(s, 0), (0, 5), "clicked in the first word");
        assert_eq!(word_range(s, 3), (0, 5));
        assert_eq!(word_range(s, 5), (0, 5), "the caret after 'hello'");
        assert_eq!(word_range(s, 6), (6, 11));
        assert_eq!(word_range(s, 11), (6, 11), "past the last glyph");

        // Mixed JP: 漢字 かな カナ ABC 123 each stand alone.
        let jp = "漢字かなカナABC123";
        assert_eq!(word_range(jp, 1), (0, 2), "kanji run");
        assert_eq!(word_range(jp, 3), (2, 4), "hiragana run");
        assert_eq!(word_range(jp, 5), (4, 6), "katakana run");
        assert_eq!(word_range(jp, 7), (6, 12), "ABC123 is one Word run");

        // A break is a wall, in both directions.
        let two = "ab\ncd";
        assert_eq!(word_range(two, 2), (0, 2), "end of line 1 takes 'ab'");
        assert_eq!(word_range(two, 3), (3, 5));
        assert_eq!(word_range("", 0), (0, 0), "an empty box selects nothing");

        // Surrogate pairs count as the 2 UTF-16 units they are.
        assert_eq!(word_range("a𠮷b", 1), (1, 3), "the pair is its own run");
    }

    /// Fullwidth latin (plans/05 item 5): ＡＢＣ１２３ is one Word run —
    /// Ctrl+Right over it jumps the whole run, ascii and fullwidth latin
    /// mix into one run, fullwidth punctuation stays Other, and halfwidth
    /// katakana remains its own class beside the fullwidth letters.
    #[test]
    fn fullwidth_latin_is_a_word_run() {
        let s = "ＡＢＣ１２３ xyz";
        assert_eq!(word_range(s, 0), (0, 6), "the whole fullwidth run");
        assert_eq!(word_range(s, 5), (0, 6));
        assert_eq!(word_range(s, 7), (7, 10));
        assert_eq!(next_word_boundary(s, 0), 7, "skips the run AND the space");
        assert_eq!(prev_word_boundary(s, 10), 7, "...back over the same edge");
        // Mixed widths are ONE word — the widths are spelling, not a boundary.
        let mix = "abcＤＥfg";
        assert_eq!(word_range(mix, 0), (0, 7));
        assert_eq!(word_range(mix, 4), (0, 7));
        // Fullwidth punctuation stays Other: ！ is its own run.
        let punct = "Ａ！Ｂ";
        assert_eq!(word_range(punct, 0), (0, 1));
        assert_eq!(word_range(punct, 1), (1, 2), "fullwidth ！ is not Word");
        assert_eq!(word_range(punct, 2), (2, 3));
        // Halfwidth katakana beside fullwidth letters: distinct classes.
        let hw = "ｱｲＡＢ";
        assert_eq!(word_range(hw, 0), (0, 2), "halfwidth kana run");
        assert_eq!(word_range(hw, 2), (2, 4), "fullwidth letters run");
    }

    #[test]
    fn insert_delete_keep_runs_consistent() {
        let mut t = item("hello");
        assert_eq!(t.runs, vec![StyleRun::plain(5)]);
        t.set_style(0, 2, StyleFlag::Bold, true);
        assert_eq!(t.runs.len(), 2);
        // Insert inside the bold run inherits bold.
        t.insert(1, "XY");
        assert_eq!(t.text, "hXYello");
        assert_eq!(t.runs[0].len, 4);
        assert!(t.runs[0].bold);
        // Insert at position 0 also joins the first run.
        t.insert(0, "A");
        assert_eq!(t.runs[0].len, 5);
        // Delete across the run boundary.
        t.delete_range(3, 6);
        assert_eq!(t.text, "AhXlo");
        assert_eq!(t.utf16_len(), 5);
        assert_eq!(t.runs.iter().map(|r| r.len).sum::<u32>(), 5);
        // Deleting everything empties the runs.
        t.delete_range(0, 99);
        assert!(t.text.is_empty() && t.runs.is_empty());
        // Insert into empty works.
        t.insert(0, "ね");
        assert_eq!(t.runs, vec![StyleRun::plain(1)]);
    }

    #[test]
    fn strike_behaves_like_the_other_flags() {
        let mut t = item("abcdef");
        t.set_style(2, 5, StyleFlag::Strike, true);
        assert!(t.range_has_all(2, 5, StyleFlag::Strike));
        assert!(!t.range_has_all(0, 6, StyleFlag::Strike));
        assert_eq!(t.runs.len(), 3, "plain / strike / plain");
        // Toggling a different flag must not disturb strike (same_style grew
        // a field — the merge invariant is the regression risk).
        t.set_style(0, 2, StyleFlag::Bold, true);
        assert!(t.range_has_all(2, 4, StyleFlag::Strike));
        t.set_style(2, 5, StyleFlag::Strike, false);
        assert_eq!(t.runs.len(), 2, "strike range collapsed back onto plain");
        assert!(!t.runs.iter().any(|r| r.strike));
    }

    #[test]
    fn typography_fields_roundtrip_and_default_old_files() {
        let mut t = item("hello");
        t.align = Align::Center;
        t.frame_align = FrameAlign::Far;
        t.letter_spacing_pt = 0.5;
        t.line_spacing = LineSpacing::Percent(180.0);
        t.set_style(0, 2, StyleFlag::Strike, true);
        let json = serde_json::to_string(&t).unwrap();
        let back: TextItem = serde_json::from_str(&json).unwrap();
        assert!(
            back == t,
            "every round-34 field survives the JSON round-trip"
        );
        assert!(back.runs[0].strike);

        // A pre-round-34 item (no align/frame_align/letter_spacing_pt/
        // line_spacing keys, runs without `strike`) loads with the old
        // defaults — old ORA files must render exactly as before.
        let old = r#"{
            "text":"abc","runs":[{"len":3,"bold":true,"italic":false,"underline":false}],
            "pos":[0.0,0.0],"size":[10.0,10.0],"rotation":0.0,
            "font":"Meiryo","size_pt":12.0,"color":[0,0,0]
        }"#;
        let t: TextItem = serde_json::from_str(old).expect("pre-round-34 JSON loads");
        assert_eq!(t.align, Align::Leading);
        assert_eq!(t.frame_align, FrameAlign::Near);
        assert_eq!(t.letter_spacing_pt, 0.0);
        assert_eq!(t.line_spacing, LineSpacing::Auto);
        assert!(!t.runs[0].strike);
        assert!(!t.auto_size, "serde default on auto_size keeps its meaning");
    }

    #[test]
    fn style_toggle_semantics() {
        let mut t = item("abcdef");
        t.set_style(1, 4, StyleFlag::Italic, true);
        assert!(!t.range_has_all(0, 6, StyleFlag::Italic));
        assert!(t.range_has_all(1, 4, StyleFlag::Italic));
        assert!(t.range_has_all(2, 3, StyleFlag::Italic));
        t.set_style(0, 6, StyleFlag::Italic, true);
        assert!(t.range_has_all(0, 6, StyleFlag::Italic));
        assert_eq!(t.runs.len(), 1, "merged back to one run");
        t.set_style(0, 6, StyleFlag::Italic, false);
        assert_eq!(t.runs, vec![StyleRun::plain(6)]);
        // style_at inherits from the char before the caret.
        t.set_style(0, 3, StyleFlag::Bold, true);
        assert!(t.style_at(3).bold);
        assert!(!t.style_at(4).bold);
        assert!(t.style_at(0).bold, "start of text uses the first run");
    }

    #[test]
    fn box_math_roundtrips_under_rotation() {
        let mut t = item("x");
        t.pos = [100.0, 50.0];
        t.size = [60.0, 40.0];
        t.rotation = 0.7;
        let p = [117.0, 63.0];
        let back = t.to_canvas(t.to_local(p));
        assert!((back[0] - p[0]).abs() < 1e-3 && (back[1] - p[1]).abs() < 1e-3);
        assert!(t.contains(t.center(), 0.0));
        assert!(!t.contains([300.0, 300.0], 0.0));

        // Corner drag keeps the opposite corner pinned in canvas space.
        let before = t.corners()[2]; // (x1,y1)
        let target = t.to_canvas([-20.0, -10.0]);
        t.apply_handle(TextHandle::Corner(0), target);
        let after = t.corners()[2];
        assert!(
            (before[0] - after[0]).abs() < 1e-2,
            "{before:?} vs {after:?}"
        );
        assert!((before[1] - after[1]).abs() < 1e-2);
        assert!((t.size[0] - 80.0).abs() < 1e-2 && (t.size[1] - 50.0).abs() < 1e-2);
        assert!(!t.auto_size);

        // Edge drag only moves one axis.
        let w_before = t.size[0];
        let bottom_mid = t.to_canvas([t.size[0] * 0.5, t.size[1] + 25.0]);
        t.apply_handle(TextHandle::Edge(2), bottom_mid);
        assert!((t.size[0] - w_before).abs() < 1e-3);
        assert!((t.size[1] - 75.0).abs() < 1e-2);

        // Minimum extent guard.
        let tiny = t.to_canvas([1.0, t.size[1] * 0.5]);
        t.apply_handle(TextHandle::Edge(1), tiny);
        assert!(t.size[0] >= MIN_TEXT_EXTENT - 1e-3);
    }

    #[test]
    fn rotate_handle_sets_angle() {
        let mut t = item("x");
        t.pos = [0.0, 0.0];
        t.size = [100.0, 40.0];
        // Handle straight above the center = angle 0.
        t.apply_handle(TextHandle::Rotate, [50.0, -100.0]);
        assert!(t.rotation.abs() < 1e-3);
        // Handle to the right = quarter turn clockwise.
        t.apply_handle(TextHandle::Rotate, [200.0, 20.0]);
        assert!((t.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-3);
    }

    fn sprite(origin: [i32; 2], w: u32, h: u32, rgba: [u8; 4]) -> Arc<RenderedText> {
        Arc::new(RenderedText {
            origin,
            size: [w, h],
            rgba: (0..w * h).flat_map(|_| rgba).collect(),
        })
    }

    #[test]
    fn rasterize_blits_sprites_sparsely() {
        let mut a = item("a");
        a.cache = Some(sprite([10, 10], 20, 20, [0, 0, 0, 255])); // opaque black
        let set = TextSet { texts: vec![a] };
        let tiles = set.rasterize((256, 256));
        assert!(!tiles.is_empty());
        let idx = TileIdx::of_pixel(15, 15);
        let (ox, oy) = idx.origin();
        let px = tiles[&idx].pixel((15 - ox) as usize, (15 - oy) as usize);
        assert_eq!(px[3], FIX15_ONE as u16);
        assert_eq!(px[0], 0);
        // Pixel outside the sprite is transparent / absent.
        assert!(
            tiles
                .get(&TileIdx::of_pixel(200, 200))
                .map_or(true, |t| t.pixel(8, 8)[3] == 0)
        );
        // Far tiles were never created.
        assert!(!tiles.contains_key(&TileIdx::of_pixel(200, 200)));
    }

    #[test]
    fn rasterize_composites_overlaps_and_clips() {
        // 50% white sprite over an opaque black one.
        let mut a = item("a");
        a.cache = Some(sprite([0, 0], 8, 8, [0, 0, 0, 255]));
        let mut b = item("b");
        b.cache = Some(sprite([0, 0], 8, 8, [128, 128, 128, 128]));
        let set = TextSet { texts: vec![a, b] };
        let tiles = set.rasterize((64, 64));
        let px = tiles[&TileIdx::new(0, 0)].pixel(2, 2);
        assert_eq!(px[3], FIX15_ONE as u16, "opaque under stays opaque");
        assert!(px[0] > FIX15_ONE as u16 / 3, "white blended over black");

        // Sprite hanging off the canvas clips instead of panicking.
        let mut c = item("c");
        c.cache = Some(sprite([-4, -4], 8, 8, [0, 0, 0, 255]));
        let set = TextSet { texts: vec![c] };
        let tiles = set.rasterize((64, 64));
        assert_eq!(tiles[&TileIdx::new(0, 0)].pixel(0, 0)[3], FIX15_ONE as u16);
        assert_eq!(tiles[&TileIdx::new(0, 0)].pixel(5, 5)[3], 0);
    }

    #[test]
    fn text_at_prefers_topmost_and_respects_rotation() {
        let mut a = item("a");
        a.pos = [0.0, 0.0];
        a.size = [100.0, 100.0];
        let mut b = item("b");
        b.pos = [50.0, 0.0];
        b.size = [100.0, 100.0];
        let set = TextSet { texts: vec![a, b] };
        assert_eq!(set.text_at([75.0, 50.0], 0.0), Some(1));
        assert_eq!(set.text_at([25.0, 50.0], 0.0), Some(0));
        assert_eq!(set.text_at([300.0, 300.0], 0.0), None);
    }
}

#[cfg(test)]
mod ruby_tests {
    use super::*;

    fn item(text: &str) -> TextItem {
        let mut it = TextItem::new([0.0, 0.0], "Meiryo".into(), 12.0, [0, 0, 0], true);
        it.insert(0, text);
        it
    }

    #[test]
    fn setting_a_reading_records_the_range() {
        let mut it = item("漢字とかな");
        assert!(it.set_ruby(0, 2, "かんじ"));
        assert_eq!(it.ruby.len(), 1);
        assert_eq!((it.ruby[0].start, it.ruby[0].len), (0, 2));
        assert_eq!(it.ruby[0].text, "かんじ");
        assert_eq!(it.ruby_at(1).map(|r| r.text.as_str()), Some("かんじ"));
        assert!(it.ruby_at(3).is_none());
    }

    /// One kanji cannot carry two readings, so a new one over the same
    /// letters replaces rather than stacks.
    #[test]
    fn an_overlapping_reading_replaces_the_old_one() {
        let mut it = item("漢字とかな");
        it.set_ruby(0, 2, "かんじ");
        it.set_ruby(1, 3, "じと");
        assert_eq!(it.ruby.len(), 1);
        assert_eq!(it.ruby[0].text, "じと");
    }

    #[test]
    fn an_empty_reading_clears_and_reports_the_change() {
        let mut it = item("漢字とかな");
        it.set_ruby(0, 2, "かんじ");
        assert!(it.set_ruby(0, 2, "   "), "whitespace clears");
        assert!(it.ruby.is_empty());
        assert!(!it.set_ruby(0, 2, ""), "clearing nothing changes nothing");
    }

    /// Typing before an annotated word must carry the reading along with it,
    /// or the furigana silently drifts onto the wrong kanji.
    #[test]
    fn text_inserted_before_a_reading_pushes_it_along() {
        let mut it = item("漢字");
        it.set_ruby(0, 2, "かんじ");
        it.insert(0, "この");
        assert_eq!((it.ruby[0].start, it.ruby[0].len), (2, 2));
        assert_eq!(it.ruby_at(2).map(|r| r.text.as_str()), Some("かんじ"));
    }

    #[test]
    fn text_inserted_inside_a_reading_extends_it() {
        let mut it = item("漢字");
        it.set_ruby(0, 2, "かんじ");
        it.insert(1, "X");
        assert_eq!((it.ruby[0].start, it.ruby[0].len), (0, 3));
    }

    #[test]
    fn text_inserted_after_a_reading_leaves_it_alone() {
        let mut it = item("漢字");
        it.set_ruby(0, 2, "かんじ");
        it.insert(2, "です");
        assert_eq!((it.ruby[0].start, it.ruby[0].len), (0, 2));
    }

    #[test]
    fn deleting_before_a_reading_shifts_it_back() {
        let mut it = item("この漢字");
        it.set_ruby(2, 4, "かんじ");
        it.delete_range(0, 2);
        assert_eq!((it.ruby[0].start, it.ruby[0].len), (0, 2));
    }

    #[test]
    fn deleting_part_of_the_base_shrinks_the_reading() {
        let mut it = item("漢字です");
        it.set_ruby(0, 2, "かんじ");
        it.delete_range(1, 3);
        assert_eq!((it.ruby[0].start, it.ruby[0].len), (0, 1));
        assert_eq!(
            it.ruby[0].text, "かんじ",
            "the reading itself is the user's"
        );
    }

    /// A reading with nothing left to read is not kept alive out of
    /// politeness — it would render over unrelated text.
    #[test]
    fn deleting_the_whole_base_drops_the_reading() {
        let mut it = item("漢字です");
        it.set_ruby(0, 2, "かんじ");
        it.delete_range(0, 2);
        assert!(it.ruby.is_empty());
    }

    #[test]
    fn several_readings_stay_sorted_and_independent() {
        let mut it = item("漢字と仮名");
        it.set_ruby(3, 5, "かな");
        it.set_ruby(0, 2, "かんじ");
        assert_eq!(it.ruby.iter().map(|r| r.start).collect::<Vec<_>>(), [0, 3]);
        it.insert(2, "XX");
        assert_eq!(it.ruby.iter().map(|r| r.start).collect::<Vec<_>>(), [0, 5]);
        assert_eq!(it.ruby[0].len, 2, "the earlier one is untouched");
    }

    /// The compatibility promise: a text item written before furigana
    /// existed has no `ruby` key at all and must still load.
    #[test]
    fn items_without_the_field_still_deserialize() {
        let json = r#"{"text":"あ","runs":[{"len":1,"bold":false,"italic":false,
            "underline":false,"strike":false}],"pos":[0.0,0.0],"size":[10.0,10.0],
            "font":"Meiryo","size_pt":12.0,"color":[0,0,0]}"#;
        let it: TextItem = serde_json::from_str(json).expect("old items keep opening");
        assert!(it.ruby.is_empty());
    }

    #[test]
    fn readings_survive_a_round_trip() {
        let mut it = item("漢字");
        it.set_ruby(0, 2, "かんじ");
        let s = serde_json::to_string(&it).unwrap();
        let back: TextItem = serde_json::from_str(&s).unwrap();
        assert_eq!(back.ruby, it.ruby);
        assert_eq!(back, it);
    }
}

#[cfg(test)]
mod font_run_tests {
    use super::*;

    fn item(text: &str) -> TextItem {
        item_in("Meiryo", text)
    }

    fn item_in(family: &str, text: &str) -> TextItem {
        let mut it = TextItem::new([0.0, 0.0], family.into(), 12.0, [0, 0, 0], true);
        it.insert(0, text);
        it
    }

    /// The whole shape `fonts` promises, checked in one place: sorted by
    /// start, never overlapping, never empty, never past the text.
    fn check_shape(it: &TextItem) {
        let n = it.utf16_len();
        let mut prev_end = 0;
        for f in &it.fonts {
            assert!(f.len > 0, "zero-length run in {:?}", it.fonts);
            assert!(
                f.start >= prev_end,
                "out of order or overlapping: {:?}",
                it.fonts
            );
            assert!(f.end() <= n, "run runs past the text: {:?}", it.fonts);
            prev_end = f.end();
        }
    }

    fn shape(it: &TextItem) -> Vec<(u32, u32, &str)> {
        it.fonts
            .iter()
            .map(|f| (f.start, f.len, f.family.as_str()))
            .collect()
    }

    #[test]
    fn a_range_can_be_set_in_another_family() {
        let mut it = item("これはCOOLです");
        assert!(it.set_font_range(3, 7, "Impact"));
        assert_eq!(it.font_at(3), "Impact");
        assert_eq!(it.font_at(6), "Impact");
        assert_eq!(it.font_at(7), "Meiryo", "outside the run: the item's font");
        assert_eq!(it.font_at(0), "Meiryo");
    }

    /// Storing an override equal to the item's own font would leave islands
    /// of the OLD face behind the day the item font changes, with nothing on
    /// screen to explain why.
    #[test]
    fn picking_the_items_own_family_clears_the_override() {
        let mut it = item("これはCOOLです");
        it.set_font_range(3, 7, "Impact");
        assert!(it.set_font_range(3, 7, "Meiryo"));
        assert!(it.fonts.is_empty());
    }

    /// A partial overlap used to take the whole earlier run with it. The new
    /// face wins over the characters it was actually set on, and no others.
    #[test]
    fn an_overlapping_range_splits_the_old_one() {
        let mut it = item("これはCOOLです");
        it.set_font_range(3, 7, "Impact");
        it.set_font_range(5, 9, "Arial");
        assert_eq!(shape(&it), [(3, 2, "Impact"), (5, 4, "Arial")]);
        assert_eq!(it.font_at(3), "Impact");
        assert_eq!(it.font_at(5), "Arial");
        check_shape(&it);
    }

    /// The traced failure: a line set in Arial, then three characters of it
    /// in Impact, used to lose the Arial entirely — everything outside the
    /// second selection fell back to the item's own face with no undo step
    /// to blame and nothing on screen to explain it.
    #[test]
    fn sub_ranging_an_override_keeps_the_parts_outside_it() {
        let mut it = item_in("源暎アンチック", "0123456789");
        assert!(it.set_font_range(0, 10, "Arial"));
        assert!(it.set_font_range(3, 5, "Impact"));
        assert_eq!(it.font_at(0), "Arial");
        assert_eq!(it.font_at(3), "Impact");
        assert_eq!(it.font_at(6), "Arial");
        assert_eq!(it.font_at(9), "Arial");
        check_shape(&it);
    }

    /// Splitting in the middle is two remnants plus the new run, in order —
    /// the shape every other op in this module reads.
    #[test]
    fn splitting_a_run_leaves_two_remnants_around_the_new_one() {
        let mut it = item("0123456789");
        it.set_font_range(1, 9, "Arial");
        it.set_font_range(4, 6, "Impact");
        assert_eq!(
            shape(&it),
            [(1, 3, "Arial"), (4, 2, "Impact"), (6, 3, "Arial")]
        );
        check_shape(&it);
    }

    /// Clearing is the documented way to take one word back out of an
    /// override, so it must cut a hole rather than wipe the run.
    #[test]
    fn clearing_a_sub_range_leaves_the_rest_of_the_override_standing() {
        let mut it = item("0123456789");
        it.set_font_range(0, 10, "Arial");
        assert!(
            it.set_font_range(4, 6, "Meiryo"),
            "the item's own font clears"
        );
        assert_eq!(shape(&it), [(0, 4, "Arial"), (6, 4, "Arial")]);
        assert_eq!(it.font_at(3), "Arial");
        assert_eq!(it.font_at(4), "Meiryo");
        assert_eq!(it.font_at(6), "Arial");
        check_shape(&it);
    }

    /// Re-setting part of a run in the face it already has changes nothing a
    /// reader could see, so it must not report a change (or leave a seam
    /// behind for the next split to trip over).
    #[test]
    fn re_setting_part_of_a_run_in_its_own_face_is_not_a_change() {
        let mut it = item("0123456789");
        it.set_font_range(0, 10, "Arial");
        assert!(!it.set_font_range(3, 5, "Arial"));
        assert_eq!(shape(&it), [(0, 10, "Arial")]);
        check_shape(&it);
    }

    /// Typing and deleting have to move all three pieces of a split set, not
    /// just the first one the bookkeeping happens to reach.
    #[test]
    fn an_edit_across_a_split_set_moves_every_piece() {
        let mut it = item("0123456789");
        it.set_font_range(0, 10, "Arial");
        it.set_font_range(3, 5, "Impact");
        it.insert(0, "XX");
        assert_eq!(
            shape(&it),
            [(2, 3, "Arial"), (5, 2, "Impact"), (7, 5, "Arial")]
        );
        // Typing inside the middle piece extends it and pushes the tail.
        it.insert(6, "Y");
        assert_eq!(
            shape(&it),
            [(2, 3, "Arial"), (5, 3, "Impact"), (8, 5, "Arial")]
        );
        // A deletion that eats into the first piece clips it and drags the
        // other two back by the whole cut.
        it.delete_range(0, 3);
        assert_eq!(
            shape(&it),
            [(0, 2, "Arial"), (2, 3, "Impact"), (5, 5, "Arial")]
        );
        assert_eq!(it.font_at(0), "Arial");
        assert_eq!(it.font_at(2), "Impact");
        assert_eq!(it.font_at(5), "Arial");
        check_shape(&it);
    }

    /// Font runs and furigana share one span rule, so typing must move them
    /// identically — this is the test that keeps them from drifting apart.
    #[test]
    fn font_runs_and_readings_move_together_under_the_same_edit() {
        let mut it = item("漢字COOL");
        it.set_ruby(0, 2, "かんじ");
        it.set_font_range(2, 6, "Impact");
        it.insert(0, "あ");
        assert_eq!((it.ruby[0].start, it.ruby[0].len), (1, 2));
        assert_eq!((it.fonts[0].start, it.fonts[0].len), (3, 4));
        it.delete_range(0, 1);
        assert_eq!((it.ruby[0].start, it.ruby[0].len), (0, 2));
        assert_eq!((it.fonts[0].start, it.fonts[0].len), (2, 4));
    }

    #[test]
    fn deleting_the_whole_run_drops_the_override() {
        let mut it = item("これはCOOLです");
        it.set_font_range(3, 7, "Impact");
        it.delete_range(3, 7);
        assert!(it.fonts.is_empty());
        assert_eq!(it.font_at(3), "Meiryo");
    }

    #[test]
    fn items_without_the_field_still_deserialize() {
        let json = r#"{"text":"あ","runs":[{"len":1,"bold":false,"italic":false,
            "underline":false,"strike":false}],"pos":[0.0,0.0],"size":[10.0,10.0],
            "font":"Meiryo","size_pt":12.0,"color":[0,0,0]}"#;
        let it: TextItem = serde_json::from_str(json).expect("old items keep opening");
        assert!(it.fonts.is_empty());
        assert_eq!(it.font_at(0), "Meiryo");
    }

    #[test]
    fn overrides_survive_a_round_trip() {
        let mut it = item("これはCOOLです");
        it.set_font_range(3, 7, "Impact");
        let s = serde_json::to_string(&it).unwrap();
        let back: TextItem = serde_json::from_str(&s).unwrap();
        assert_eq!(back.fonts, it.fonts);
        assert_eq!(back, it);
    }
}

#[cfg(test)]
mod tcy_tests {
    use super::*;

    fn item(text: &str) -> TextItem {
        let mut it = TextItem::new([0.0, 0.0], "Meiryo".into(), 12.0, [0, 0, 0], true);
        it.insert(0, text);
        it
    }

    fn shape(it: &TextItem) -> Vec<(u32, u32)> {
        it.tcy.iter().map(|t| (t.start, t.len)).collect()
    }

    #[test]
    fn marking_a_range_records_it_and_reports_the_change() {
        let mut it = item("22時に");
        assert!(it.set_tcy(0, 2, true));
        assert_eq!(shape(&it), [(0, 2)]);
        assert!(!it.set_tcy(0, 2, true), "marking it again changes nothing");
        assert!(it.set_tcy(0, 2, false));
        assert!(it.tcy.is_empty());
        assert!(!it.set_tcy(0, 2, false), "clearing nothing changes nothing");
        assert!(!it.set_tcy(1, 1, true), "an empty range is not an edit");
    }

    /// Two digits marked one at a time are ONE cell. A pair of touching
    /// half-cells would set 22 as two stacked squares, which is the thing
    /// 縦中横 exists to prevent.
    #[test]
    fn touching_runs_merge_into_one_cell() {
        let mut it = item("22時に");
        it.set_tcy(0, 1, true);
        it.set_tcy(1, 2, true);
        assert_eq!(shape(&it), [(0, 2)]);
    }

    /// Turning it off over the middle of a run leaves the ends upright —
    /// the same split `set_font_range` does, and for the same reason.
    #[test]
    fn clearing_the_middle_of_a_run_splits_it() {
        let mut it = item("0123456789");
        it.set_tcy(0, 10, true);
        assert!(it.set_tcy(4, 6, false));
        assert_eq!(shape(&it), [(0, 4), (6, 4)]);
    }

    #[test]
    fn a_run_rides_the_edits_that_move_its_characters() {
        let mut it = item("22時");
        it.set_tcy(0, 2, true);
        it.insert(0, "午後");
        assert_eq!(shape(&it), [(2, 2)], "typing in front carries it along");
        it.insert(3, "3");
        assert_eq!(shape(&it), [(2, 3)], "typing inside extends it");
        it.delete_range(0, 2);
        assert_eq!(shape(&it), [(0, 3)], "deleting in front drags it back");
        it.delete_range(1, 3);
        assert_eq!(shape(&it), [(0, 1)], "deleting part of it shrinks it");
        it.delete_range(0, 1);
        assert!(it.tcy.is_empty(), "with its characters gone, so is the run");
    }

    /// All three span lists share one rule; this is the test that keeps
    /// 縦中横 from drifting away from furigana and font runs.
    #[test]
    fn tcy_moves_with_readings_and_font_runs_under_one_edit() {
        let mut it = item("漢字22");
        it.set_ruby(0, 2, "かんじ");
        it.set_font_range(2, 4, "Impact");
        it.set_tcy(2, 4, true);
        it.insert(0, "あ");
        assert_eq!((it.ruby[0].start, it.ruby[0].len), (1, 2));
        assert_eq!((it.fonts[0].start, it.fonts[0].len), (3, 2));
        assert_eq!(shape(&it), [(3, 2)]);
        it.delete_range(0, 1);
        assert_eq!((it.ruby[0].start, it.ruby[0].len), (0, 2));
        assert_eq!((it.fonts[0].start, it.fonts[0].len), (2, 2));
        assert_eq!(shape(&it), [(2, 2)]);
    }

    /// The compatibility promise: every text item written before today has
    /// no `tcy` key at all and must still open.
    #[test]
    fn items_without_the_field_still_deserialize() {
        let json = r#"{"text":"あ","runs":[{"len":1,"bold":false,"italic":false,
            "underline":false,"strike":false}],"pos":[0.0,0.0],"size":[10.0,10.0],
            "font":"Meiryo","size_pt":12.0,"color":[0,0,0]}"#;
        let it: TextItem = serde_json::from_str(json).expect("old items keep opening");
        assert!(it.tcy.is_empty());
    }

    #[test]
    fn runs_survive_a_round_trip() {
        let mut it = item("22時に");
        it.set_tcy(0, 2, true);
        let s = serde_json::to_string(&it).unwrap();
        let back: TextItem = serde_json::from_str(&s).unwrap();
        assert_eq!(back.tcy, it.tcy);
        assert_eq!(back, it, "and the field is part of item equality");
    }
}

#[cfg(test)]
mod auto_tcy_tests {
    use super::*;

    /// Vertical by default — Auto 縦中横 is only meaningful there, and the
    /// derived list is computed the same way regardless so the renderer's
    /// own orientation gate stays the single place that decision is made.
    fn item(text: &str, n: u8) -> TextItem {
        let mut it = TextItem::new([0.0, 0.0], "Meiryo".into(), 12.0, [0, 0, 0], true);
        it.insert(0, text);
        it.auto_tcy = n;
        it
    }

    fn spans(it: &TextItem) -> Vec<(u32, u32)> {
        it.effective_tcy()
            .iter()
            .map(|t| (t.start, t.len))
            .collect()
    }

    /// The heart of TX-062: maximal alphanumeric runs no longer than the
    /// limit are stood upright, longer ones are left alone.
    ///
    /// Positions are counted out by hand rather than searched for, because
    /// an off-by-one here puts the upright cell on the wrong character and
    /// nothing in a render test would say so.
    #[test]
    fn runs_within_the_limit_are_found_and_longer_ones_are_left_alone() {
        //  第  1  話  ␠  2  2  時  ␠  1  2  3   4   5
        //  0   1  2   3  4  5  6   7  8  9  10  11  12
        let text = "第1話 22時 12345";
        assert_eq!(utf16_len(text), 13);

        assert_eq!(spans(&item(text, 0)), Vec::<(u32, u32)>::new(), "off");
        assert_eq!(spans(&item(text, 1)), vec![(1, 1)], "only the lone 1");
        assert_eq!(spans(&item(text, 2)), vec![(1, 1), (4, 2)]);
        assert_eq!(
            spans(&item(text, 4)),
            vec![(1, 1), (4, 2)],
            "12345 is five characters — past the dropdown's own ceiling"
        );
    }

    /// A run is measured WHOLE, not truncated to the limit. "223" at a limit
    /// of 2 is not "22" plus a stray 3 — either the number stands up or it
    /// does not, and splitting it would set 22 upright and lay the 3 on its
    /// side, which is worse than doing nothing.
    #[test]
    fn a_run_is_taken_whole_or_not_at_all() {
        assert_eq!(spans(&item("223時", 2)), Vec::<(u32, u32)>::new());
        assert_eq!(spans(&item("22時", 2)), vec![(0, 2)]);
    }

    /// Auto is DERIVED, so an edit re-evaluates it. Typing a third digit
    /// into a pair must take the cell apart again; the alternative — a run
    /// materialized into `tcy` at the moment it was typed — leaves a stale
    /// two-character cell with a digit hanging off it.
    #[test]
    fn editing_re_evaluates_instead_of_leaving_a_stale_cell() {
        let mut it = item("22時", 2);
        assert_eq!(spans(&it), vec![(0, 2)]);
        it.insert(2, "3");
        assert_eq!(it.text, "223時");
        assert_eq!(spans(&it), Vec::<(u32, u32)>::new(), "three no longer fits");
        it.delete_range(0, 1);
        assert_eq!(it.text, "23時");
        assert_eq!(spans(&it), vec![(0, 2)], "back to a pair, back upright");
        assert!(
            it.tcy.is_empty(),
            "and nothing was ever written to the model"
        );
    }

    /// A hand-marked run wins over an auto candidate that touches it, so the
    /// 縦中横 button keeps meaning what it says. Without this the two lists
    /// would overlap and the same characters would be substituted twice.
    #[test]
    fn a_hand_marked_run_beats_the_auto_candidate_it_overlaps() {
        let mut it = item("22時", 2);
        assert!(it.set_tcy(0, 1, true));
        assert_eq!(
            spans(&it),
            vec![(0, 1)],
            "the marked single digit stands; the pair candidate is dropped"
        );
        // Non-overlapping: the mark on 時 leaves the pair candidate alone.
        let mut it = item("22時", 2);
        assert!(it.set_tcy(2, 3, true));
        assert_eq!(spans(&it), vec![(0, 2), (2, 1)]);
        // ...and touching runs stay TWO runs: fusing them would set 時 into
        // the digits' cell.
        assert_eq!(it.effective_tcy().len(), 2);
    }

    /// The setting rides the file, the derived runs do not — and an item
    /// written before the field existed loads with Auto off, which is the
    /// state in which it renders exactly as it always did.
    #[test]
    fn the_setting_round_trips_and_old_files_load_with_it_off() {
        let it = item("第1話", 2);
        let json = serde_json::to_string(&it).unwrap();
        assert!(
            !json.contains("\"tcy\":[{"),
            "auto runs must not be written into the model: {json}"
        );
        let back: TextItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.auto_tcy, 2);
        assert_eq!(back, it, "and the field is part of item equality");

        let old = r#"{
            "text":"第1話","runs":[{"len":3}],
            "pos":[0.0,0.0],"size":[10.0,10.0],
            "font":"Meiryo","size_pt":12.0,"color":[0,0,0]
        }"#;
        let t: TextItem = serde_json::from_str(old).expect("pre-TX-062 JSON loads");
        assert_eq!(t.auto_tcy, 0);
        assert!(t.effective_tcy().is_empty());
    }

    /// Line breaks and kana end a run: 縦中横 is a horizontal island inside
    /// one column, and an island cannot span two of them.
    #[test]
    fn a_run_cannot_cross_a_line_break() {
        let it = item("1\n2", 2);
        assert_eq!(spans(&it), vec![(0, 1), (2, 1)], "two islands, not one");
        let it = item("1あ2", 2);
        assert_eq!(spans(&it), vec![(0, 1), (2, 1)]);
    }

    /// Full-width digits are already upright in a vertical column, so Auto
    /// must not touch them — standing them up again would draw them into a
    /// one-em hole at half size for nothing.
    #[test]
    fn full_width_digits_are_left_to_the_layout_engine() {
        assert_eq!(spans(&item("２２時", 2)), Vec::<(u32, u32)>::new());
    }
}
