//! DirectWrite/Direct2D text engine — the OS half of text layers.
//!
//! `core::text` owns the model and blits cached sprites; this crate turns a
//! [`TextItem`] into that sprite and answers caret/selection geometry
//! questions. Everything DirectWrite happens here so `mn-core` stays OS-free.
//!
//! Layout rules:
//! * Coordinates in and out are **unrotated box-local px** (origin = the
//!   item's `pos`), 1 px == 1 DIP (render targets run at 96 dpi). The app
//!   converts canvas↔local with `TextItem::to_local/to_canvas`.
//! * Font size: `size_pt` at the *document* dpi → px = pt / 72 × dpi
//!   (dpi 0, the pixel presets, behaves as 96).
//! * Vertical JP = `DWRITE_READING_DIRECTION_TOP_TO_BOTTOM` +
//!   `DWRITE_FLOW_DIRECTION_RIGHT_TO_LEFT`; DirectWrite does the dragon work
//!   (kinsoku wrapping, upright kana, rotated Latin, vertical forms).
//! * It has NO ruby and NO 縦中横, so both are drawn as a second small
//!   layout placed with hit-test geometry taken from the first
//!   ([`InlineDraw`]) rather than as a custom `IDWriteInlineObject`.
//! * The sprite bakes fill colour, per-range bold/italic/underline, rotation
//!   and the edge (フチ, distance-transform outline) — premultiplied RGBA8.
//!
//! All text positions are UTF-16 code units, matching `core::text`.

use std::sync::Arc;

use mn_core::text::{Align, FrameAlign, LineSpacing, RenderedText, TextItem, utf16_to_byte};

use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::{BOOL, HSTRING, Interface, Result, w};
use windows_numerics::{Matrix3x2, Vector2};

/// Sprites larger than this per side are refused (a rotated B4-wide text at a
/// huge point size could otherwise ask for gigabytes).
const MAX_SPRITE: u32 = 16384;

pub struct TextEngine {
    dwrite: IDWriteFactory,
    d2d: ID2D1Factory,
    wic: IWICImagingFactory,
    families: Vec<String>,
    /// Per-(content-hash, dpi) layout cache (plans/05 item 5): the engine
    /// rebuilt a full DirectWrite layout on every arrow key, from seven
    /// callers. LRU-capped at 8 — the machine is RAM-starved and a layout
    /// is a few dozen KB. COM lifetime is the engine's: the map lives here,
    /// the wrappers Release when it drops.
    cache: std::cell::RefCell<LayoutCache>,
    /// TEST seam: how many layouts were actually BUILT (the seven callers'
    /// cache-hit rate is what this round exists for).
    pub layout_builds: std::cell::Cell<u32>,
}

/// The cache itself: key → layout plus LRU order (oldest first).
struct LayoutCache {
    map: std::collections::HashMap<u64, IDWriteTextLayout>,
    order: std::collections::VecDeque<u64>,
}

impl Default for LayoutCache {
    fn default() -> Self {
        Self {
            map: Default::default(),
            order: Default::default(),
        }
    }
}

/// Content hash of every item field the LAYOUT depends on, dpi included:
/// font/size/orientation/alignment/spacing/box/text/ruby-count/runs. A
/// revision counter (the plan's first idea) was rejected: app-side
/// closures mutate layout-affecting fields (size_pt, font, …) outside any
/// core funnel, and one missed bump serves a stale layout — a hash of the
/// content itself cannot go stale.
fn layout_key(item: &TextItem, dpi: u32) -> u64 {
    use std::hash::{Hash, Hasher};
    fn f32_bits(h: &mut std::collections::hash_map::DefaultHasher, b: f32) {
        h.write_u32(b.to_bits());
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    dpi.hash(&mut h);
    item.font.hash(&mut h);
    f32_bits(&mut h, item.size_pt);
    item.vertical.hash(&mut h);
    std::mem::discriminant(&item.align).hash(&mut h);
    std::mem::discriminant(&item.frame_align).hash(&mut h);
    f32_bits(&mut h, item.letter_spacing_pt);
    match item.line_spacing {
        LineSpacing::Auto => h.write_u8(0),
        LineSpacing::Percent(p) => {
            h.write_u8(1);
            f32_bits(&mut h, p);
        }
        LineSpacing::Pt(v) => {
            h.write_u8(2);
            f32_bits(&mut h, v);
        }
    }
    f32_bits(&mut h, item.size[0]);
    f32_bits(&mut h, item.size[1]);
    item.text.hash(&mut h);
    // The Auto+ruby spacing branch reads `!item.ruby.is_empty()` AND
    // `ruby_px` (which reads ruby_style's size) — presence + style change
    // the base layout; the drawn readings do not.
    (item.ruby.len() as u32).hash(&mut h);
    f32_bits(&mut h, item.ruby_style.size_pct);
    std::mem::discriminant(&item.ruby_style.align).hash(&mut h);
    f32_bits(&mut h, item.ruby_style.offset_pt);
    f32_bits(&mut h, item.ruby_style.gap_pt);
    item.ruby_style.font.hash(&mut h);
    // Per-range fonts change glyphs (TX-064) — all of them.
    for fr in &item.fonts {
        fr.start.hash(&mut h);
        fr.len.hash(&mut h);
        fr.family.hash(&mut h);
    }
    // 縦中横 substitution rewrites the string the layout shapes (TX-063).
    for t in &item.tcy {
        t.start.hash(&mut h);
        t.len.hash(&mut h);
    }
    item.auto_tcy.hash(&mut h);
    // Runs are applied to the layout itself (bold is wider) — all of them.
    for r in &item.runs {
        r.len.hash(&mut h);
        r.bold.hash(&mut h);
        r.italic.hash(&mut h);
        r.underline.hash(&mut h);
        r.strike.hash(&mut h);
    }
    h.finish()
}

/// Caret geometry for one text position: the leading-edge point plus the
/// character cell, box-local px.
#[derive(Clone, Copy, Debug, Default)]
pub struct CaretPos {
    pub point: [f32; 2],
    /// x, y, w, h of the position's cell.
    pub cell: [f32; 4],
}

fn px_per_pt(dpi: u32) -> f32 {
    (if dpi == 0 { 96 } else { dpi }) as f32 / 72.0
}

/// Font size in px for an item at a document dpi.
pub fn font_px(item: &TextItem, dpi: u32) -> f32 {
    (item.size_pt * px_per_pt(dpi)).max(1.0)
}

/// Ruby size in px for an item — `RubyStyle::size_pct` of the base. The
/// default is 50 %, what JIS X 4051 specifies; CSP ships 67 %, which is why
/// the percentage is reachable from Tool Property rather than baked in as
/// the constant it was for one round.
fn ruby_px(item: &TextItem, dpi: u32) -> f32 {
    (font_px(item, dpi) * (item.ruby_style.size_pct / 100.0).clamp(0.1, 2.0)).max(1.0)
}

/// A small SECOND layout, measured and placed in the main layout's space —
/// how both furigana and 縦中横 are drawn, since DirectWrite has a call for
/// neither. Drawn with the item's own transform and brush, so it rotates
/// with the box and inherits the colour and the フチ outline for free.
struct InlineDraw {
    layout: IDWriteTextLayout,
    at: [f32; 2],
    size: [f32; 2],
}

/// The string DirectWrite actually lays out. Identical to `item.text` except
/// that in VERTICAL text every character of a 縦中横 run is replaced by an
/// ideographic space (TX-063) — the base characters must not draw themselves
/// when `tcy_draws` is about to draw them upright, and they must not draw
/// themselves ROTATED, which is what a vertical run of Latin means.
///
/// One UTF-16 unit in, one out, always: every index in the model (carets,
/// selections, style runs, readings, font overrides) is a position in this
/// string too, so nothing at the engine boundary has to be remapped. A
/// character is only substituted when the WHOLE of it is inside the run, so
/// a range that somehow ends between surrogate halves cannot manufacture a
/// lone surrogate.
fn layout_utf16(item: &TextItem) -> Vec<u16> {
    let mut out: Vec<u16> = item.text.encode_utf16().collect();
    if !item.vertical {
        return out;
    }
    // The hand-marked runs PLUS whatever Auto 縦中横 found (TX-062). Every
    // site that reads 縦中横 reads the same derived list, so an auto run and
    // a marked one cannot disagree about which characters were replaced.
    let tcy = item.effective_tcy();
    if tcy.is_empty() {
        return out;
    }
    let mut at = 0u32;
    for c in item.text.chars() {
        let n = c.len_utf16() as u32;
        if tcy.iter().any(|t| at >= t.start && at + n <= t.start + t.len) {
            for u in &mut out[at as usize..(at + n) as usize] {
                *u = 0x3000; // U+3000 IDEOGRAPHIC SPACE
            }
        }
        at += n;
    }
    out
}

impl TextEngine {
    pub fn new() -> Result<Self> {
        // S_FALSE (already initialized) and mode mismatches are fine — we only
        // need *a* COM apartment on this thread for WIC.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let d2d: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let wic: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
        let families = enumerate_families(&dwrite).unwrap_or_default();
        Ok(Self {
            dwrite,
            d2d,
            wic,
            families,
            cache: std::cell::RefCell::new(LayoutCache::default()),
            layout_builds: std::cell::Cell::new(0),
        })
    }

    /// Installed family names (ja-JP name preferred), sorted, cached.
    pub fn families(&self) -> &[String] {
        &self.families
    }

    /// Does the system collection know `name` (any locale)?
    pub fn has_family(&self, name: &str) -> bool {
        let mut coll: Option<IDWriteFontCollection> = None;
        if unsafe { self.dwrite.GetSystemFontCollection(&mut coll, false) }.is_err() {
            return false;
        }
        let Some(coll) = coll else { return false };
        let mut idx = 0u32;
        let mut exists = BOOL::default();
        unsafe {
            coll.FindFamilyName(&HSTRING::from(name), &mut idx, &mut exists)
                .is_ok()
                && exists.as_bool()
        }
    }

    /// The default font for new text: the owner's 源暎アンチック v5 if any of
    /// its variants is installed, else the first installed JP fallback.
    pub fn default_family(&self) -> String {
        for exact in ["源暎アンチックv5", "源暎アンチック v5"] {
            if self.has_family(exact) {
                return exact.to_string();
            }
        }
        if let Some(f) = self.families.iter().find(|f| f.contains("源暎アンチック")) {
            return f.clone();
        }
        for fallback in ["メイリオ", "Meiryo", "Yu Gothic", "MS Gothic"] {
            if self.has_family(fallback) {
                return fallback.to_string();
            }
        }
        self.families
            .first()
            .cloned()
            .unwrap_or_else(|| "Meiryo".into())
    }

    /// The cached layout for (item content, dpi) — the plan's per-revision
    /// cache, keyed by a content hash instead (see `layout_key`). All
    /// seven callers (natural_size, render, hit_test_point, caret,
    /// selection_rects, line_move, line_bounds) come through here, so one
    /// arrow key = one cache hit, not seven builds.
    fn layout(&self, item: &TextItem, dpi: u32) -> Result<IDWriteTextLayout> {
        let key = layout_key(item, dpi);
        {
            let mut c = self.cache.borrow_mut();
            if let Some(l) = c.map.get(&key) {
                let l = l.clone();
                // LRU touch.
                if let Some(i) = c.order.iter().position(|&k| k == key) {
                    c.order.remove(i);
                }
                c.order.push_back(key);
                return Ok(l);
            }
        }
        let layout = self.build_layout(item, dpi)?;
        let mut c = self.cache.borrow_mut();
        const CAP: usize = 8;
        while c.order.len() >= CAP {
            if let Some(old) = c.order.pop_front() {
                c.map.remove(&old);
            }
        }
        c.order.push_back(key);
        c.map.insert(key, layout.clone());
        Ok(layout)
    }

    fn build_layout(&self, item: &TextItem, dpi: u32) -> Result<IDWriteTextLayout> {
        self.layout_builds.set(self.layout_builds.get() + 1);
        let format = unsafe {
            self.dwrite.CreateTextFormat(
                &HSTRING::from(item.font.as_str()),
                None,
                DWRITE_FONT_WEIGHT_REGULAR,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                font_px(item, dpi),
                w!("ja-jp"),
            )?
        };
        let utf16 = layout_utf16(item);
        let (max_w, max_h) = (item.size[0].max(1.0), item.size[1].max(1.0));
        let layout = unsafe {
            self.dwrite
                .CreateTextLayout(&utf16, &format, max_w, max_h)?
        };
        if item.vertical {
            unsafe {
                layout.SetReadingDirection(DWRITE_READING_DIRECTION_TOP_TO_BOTTOM)?;
                layout.SetFlowDirection(DWRITE_FLOW_DIRECTION_RIGHT_TO_LEFT)?;
            }
        }
        // Round-34 typography (CSP Text Tool parity). Row alignment, frame
        // position and character spacing map 1:1 onto DirectWrite layout
        // calls; line spacing needs the NATURAL first-line metrics first so
        // Percent(100) stays exactly the font's own height (measured BEFORE
        // the SetLineSpacing call — no feedback loop). `Auto` skips the call
        // entirely, keeping pre-round-34 items pixel-identical.
        unsafe {
            layout.SetTextAlignment(match item.align {
                Align::Leading => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Trailing => DWRITE_TEXT_ALIGNMENT_TRAILING,
            })?;
            layout.SetParagraphAlignment(match item.frame_align {
                FrameAlign::Near => DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                FrameAlign::Center => DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                FrameAlign::Far => DWRITE_PARAGRAPH_ALIGNMENT_FAR,
            })?;
            if item.letter_spacing_pt != 0.0 {
                let px = item.letter_spacing_pt * px_per_pt(dpi);
                let l1: IDWriteTextLayout1 = layout.cast()?;
                l1.SetCharacterSpacing(
                    px,
                    px,
                    0.0,
                    DWRITE_TEXT_RANGE {
                        startPosition: 0,
                        length: utf16.len() as u32,
                    },
                )?;
            }
            // Furigana needs a gap to live in. Left at natural spacing, the
            // reading above line 2 lands on line 1's descenders (in vertical
            // text, on the column to its right — the same collision turned
            // 90°). So an item WITH annotations and no explicit spacing gets
            // the natural metrics plus one ruby height. Items without ruby
            // never reach this branch and stay pixel-identical.
            if item.line_spacing == LineSpacing::Auto && !item.ruby.is_empty() {
                let mut count = 0u32;
                let _ = layout.GetLineMetrics(None, &mut count);
                if count > 0 {
                    let mut lines = vec![DWRITE_LINE_METRICS::default(); count as usize];
                    if layout.GetLineMetrics(Some(&mut lines), &mut count).is_ok()
                        && let Some(l0) = lines.first()
                    {
                        let add = ruby_px(item, dpi);
                        let h = l0.height.max(1.0) + add;
                        // The baseline moves down by the same amount: the gap
                        // opens ABOVE the line, where the reading goes.
                        let b = l0.baseline.max(1.0) + add;
                        layout.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, h, b)?;
                    }
                }
            }
            if !matches!(item.line_spacing, LineSpacing::Auto) {
                let mut count = 0u32;
                let _ = layout.GetLineMetrics(None, &mut count);
                if count > 0 {
                    let mut lines = vec![DWRITE_LINE_METRICS::default(); count as usize];
                    if layout.GetLineMetrics(Some(&mut lines), &mut count).is_ok() {
                        if let Some(l0) = lines.first() {
                            let (h0, b0) = (l0.height.max(1.0), l0.baseline.max(1.0));
                            let (h, b) = match item.line_spacing {
                                LineSpacing::Percent(p) => (h0 * p / 100.0, b0 * p / 100.0),
                                LineSpacing::Pt(v) => {
                                    let px = (v * px_per_pt(dpi)).max(1.0);
                                    (px, b0 * px / h0)
                                }
                                LineSpacing::Auto => unreachable!(),
                            };
                            layout.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, h, b)?;
                        }
                    }
                }
            }
        }
        let mut start = 0u32;
        for run in &item.runs {
            let range = DWRITE_TEXT_RANGE {
                startPosition: start,
                length: run.len,
            };
            unsafe {
                if run.bold {
                    layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range)?;
                }
                if run.italic {
                    layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range)?;
                }
                if run.underline {
                    layout.SetUnderline(true, range)?;
                }
                if run.strike {
                    layout.SetStrikethrough(true, range)?;
                }
            }
            start += run.len;
        }
        // Per-range families (TX-064) go on AFTER the style runs, so a range
        // that is both bold and set in another face keeps its weight.
        for f in &item.fonts {
            unsafe {
                layout.SetFontFamilyName(
                    &HSTRING::from(f.family.as_str()),
                    DWRITE_TEXT_RANGE {
                        startPosition: f.start,
                        length: f.len,
                    },
                )?;
            }
        }
        // 縦中横 (TX-063): those characters are now ideographic spaces, and
        // shrinking them to 1/n of the body size makes the run's n cells add
        // up to exactly ONE em down the column — the square the digits are
        // drawn into by `tcy_draws`. Sizing the HOLE is why this works for a
        // run of any length; the alternative (leave the digits' own advance
        // and cover them) gives a one-em cell for two digits by luck and a
        // one-and-a-half-em cell for three.
        if item.vertical {
            for t in &item.effective_tcy() {
                if t.len == 0 {
                    continue;
                }
                unsafe {
                    layout.SetFontSize(
                        font_px(item, dpi) / t.len as f32,
                        DWRITE_TEXT_RANGE {
                            startPosition: t.start,
                            length: t.len,
                        },
                    )?;
                }
            }
        }
        Ok(layout)
    }

    /// Measure and place every furigana annotation in layout space.
    ///
    /// Horizontal text puts the reading ABOVE its base run, centred on it;
    /// vertical text puts it to the RIGHT of the column, centred along it.
    /// That is where Japanese typesetting puts it, and it is why the vertical
    /// case is not the horizontal case rotated.
    ///
    /// v1 limit, recorded rather than hidden: an annotation whose base run
    /// WRAPS across two lines is drawn against the first line's rect only.
    fn ruby_draws(
        &self,
        item: &TextItem,
        dpi: u32,
        layout: &IDWriteTextLayout,
    ) -> Result<Vec<InlineDraw>> {
        if item.ruby.is_empty() {
            return Ok(Vec::new());
        }
        let size = ruby_px(item, dpi);
        let mut out = Vec::new();
        for r in &item.ruby {
            if r.len == 0 || r.text.is_empty() {
                continue;
            }
            // The reading is set in whatever family its BASE is set in — an
            // English word in a Japanese balloon (TX-064) glossed in the
            // balloon's font would read as a mistake — unless the item names
            // a reading font of its own (CSP's "Reading font").
            let family = item
                .ruby_style
                .font
                .as_deref()
                .filter(|f| !f.trim().is_empty())
                .unwrap_or_else(|| item.font_at(r.start));
            let format = unsafe {
                self.dwrite.CreateTextFormat(
                    &HSTRING::from(family),
                    None,
                    DWRITE_FONT_WEIGHT_REGULAR,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("ja-jp"),
                )?
            };
            let mut count = 0u32;
            let _ = unsafe { layout.HitTestTextRange(r.start, r.len, 0.0, 0.0, None, &mut count) };
            if count == 0 {
                continue;
            }
            let mut hits = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
            if unsafe {
                layout.HitTestTextRange(r.start, r.len, 0.0, 0.0, Some(&mut hits), &mut count)
            }
            .is_err()
            {
                continue;
            }
            hits.truncate(count as usize);
            let Some(base) = hits.iter().find(|m| m.width > 0.0 && m.height > 0.0) else {
                continue;
            };

            let u16s: Vec<u16> = r.text.encode_utf16().collect();
            let rl = unsafe {
                self.dwrite
                    .CreateTextLayout(&u16s, &format, MAX_SPRITE as f32, MAX_SPRITE as f32)?
            };
            if item.vertical {
                unsafe {
                    rl.SetReadingDirection(DWRITE_READING_DIRECTION_TOP_TO_BOTTOM)?;
                    rl.SetFlowDirection(DWRITE_FLOW_DIRECTION_RIGHT_TO_LEFT)?;
                }
            }
            let mut m = DWRITE_TEXT_METRICS::default();
            unsafe { rl.GetMetrics(&mut m)? };
            let (w, h) = (m.width.max(1.0), m.height.max(1.0));
            // Shrink the box onto the text. Vertical flow anchors columns to
            // the box's RIGHT edge, so leaving the measuring box in place
            // would park the reading 16k px from where it belongs.
            unsafe {
                rl.SetMaxWidth(w + 1.0)?;
                rl.SetMaxHeight(h + 1.0)?;
            }
            // CSP's Reading settings, applied here rather than baked in:
            // where the reading sits ALONG the base run (`align`), how far
            // it is nudged along it (`offset_pt`), and how far it stands off
            // the base (`gap_pt`). The along-axis is the reading direction,
            // so it is the Y axis in vertical text and X in horizontal —
            // which is the whole reason this is not one formula.
            let st = &item.ruby_style;
            let ppp = px_per_pt(dpi);
            let (gap, nudge) = (st.gap_pt * ppp, st.offset_pt * ppp);
            let slack = |base_len: f32, ruby_len: f32| match st.align {
                Align::Leading => 0.0,
                Align::Center => (base_len - ruby_len) * 0.5,
                Align::Trailing => base_len - ruby_len,
            };
            let at = if item.vertical {
                [
                    base.left + base.width + gap,
                    base.top + slack(base.height, h) + nudge,
                ]
            } else {
                [
                    base.left + slack(base.width, w) + nudge,
                    base.top - h - gap,
                ]
            };
            out.push(InlineDraw {
                layout: rl,
                at,
                size: [w, h],
            });
        }
        Ok(out)
    }

    /// Measure and place every 縦中横 group (TX-063): the run's characters
    /// laid out HORIZONTALLY at the body size and centred in the one-em hole
    /// `layout` left for them.
    ///
    /// WHY the hole rather than drawing on top of the originals — the two
    /// candidates were (a) substitute a space of the right advance and draw
    /// into it, (b) draw over the cell and hide the base glyphs (a
    /// transparent drawing effect over the range, which D2D's own renderer
    /// honours). (a) is what is implemented, because (b) leaves the ORIGINAL
    /// advance behind: a digit lying on its side steps its own width down
    /// the column — 0.62 em each in Meiryo, measured — so a pair would hold
    /// a cell of 1.24 em and a three-digit run 1.86, and the cell would
    /// change size with the face. Substituting a space whose size we choose
    /// gives exactly one em for a run of any length in any font, which is
    /// the cell the typography actually specifies.
    /// Neither candidate is the Windows-canonical answer — that is a custom
    /// `IDWriteInlineObject`, a COM interface to implement in Rust for one
    /// square of type — and this is the same second-pass technique furigana
    /// already runs on.
    ///
    /// Horizontal items ignore their runs entirely: 縦中横 means "horizontal
    /// inside vertical", and there is no second meaning to invent for it.
    fn tcy_draws(
        &self,
        item: &TextItem,
        dpi: u32,
        layout: &IDWriteTextLayout,
    ) -> Result<Vec<InlineDraw>> {
        if !item.vertical {
            return Ok(Vec::new());
        }
        let tcy = item.effective_tcy();
        if tcy.is_empty() {
            return Ok(Vec::new());
        }
        let em = font_px(item, dpi);
        let mut out = Vec::new();
        for t in &tcy {
            let (b0, b1) = (
                utf16_to_byte(&item.text, t.start),
                utf16_to_byte(&item.text, t.end()),
            );
            let text = &item.text[b0..b1];
            if text.trim().is_empty() {
                continue;
            }
            let mut count = 0u32;
            let _ = unsafe { layout.HitTestTextRange(t.start, t.len, 0.0, 0.0, None, &mut count) };
            if count == 0 {
                continue;
            }
            let mut hits = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
            if unsafe {
                layout.HitTestTextRange(t.start, t.len, 0.0, 0.0, Some(&mut hits), &mut count)
            }
            .is_err()
            {
                continue;
            }
            hits.truncate(count as usize);
            let Some(cell) = hits.iter().find(|m| m.width > 0.0 && m.height > 0.0) else {
                continue;
            };
            let family = item.font_at(t.start);
            // Deliberately NOT given a vertical reading direction: laying
            // these characters out the ordinary way round is the feature.
            let build = |size: f32| -> Result<(IDWriteTextLayout, [f32; 2])> {
                let format = unsafe {
                    self.dwrite.CreateTextFormat(
                        &HSTRING::from(family),
                        None,
                        DWRITE_FONT_WEIGHT_REGULAR,
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        size.max(1.0),
                        w!("ja-jp"),
                    )?
                };
                let u16s: Vec<u16> = text.encode_utf16().collect();
                let tl = unsafe {
                    self.dwrite
                        .CreateTextLayout(&u16s, &format, MAX_SPRITE as f32, MAX_SPRITE as f32)?
                };
                let mut m = DWRITE_TEXT_METRICS::default();
                unsafe { tl.GetMetrics(&mut m)? };
                Ok((tl, [m.width.max(1.0), m.height.max(1.0)]))
            };
            let (mut tl, mut ext) = build(em)?;
            // The group is only ever scaled to keep it out of the NEXT
            // column, so the limit is the column's own width and not the em
            // — a digit is around 0.6 em wide in a text face and a column is
            // around 1.5 em, so the two digits 縦中横 is nearly always used
            // for are set at full size and nothing here fires at all. Three
            // or four do fire, and then this shrinks them UNIFORMLY where
            // real typesetting would CONDENSE (narrow the glyphs without
            // shortening them): DirectWrite has no per-run horizontal scale,
            // and doing it properly means a second transform around the
            // draw. Recorded as the deviation it is.
            //
            // The floor is one em because a line with NOTHING on it but a
            // 縦中横 run — a bare page number — sets its column height from
            // the shrunken spaces, and without the floor the digits would
            // then be condensed to fit a column their own hole created.
            let budget = cell.width.max(em);
            if ext[0] > budget {
                let (t2, e2) = build(em * budget / ext[0])?;
                tl = t2;
                ext = e2;
            }
            // Shrink the measuring box onto the text so the overhang metrics
            // below are measured against the text and not against a 16k box.
            let (bw, bh) = (ext[0] + 1.0, ext[1] + 1.0);
            unsafe {
                tl.SetMaxWidth(bw)?;
                tl.SetMaxHeight(bh)?;
            }
            // Centre the INK in the cell, not the line box: a line box is
            // mostly ascent and descent, and centring that would sit the
            // digits visibly low. The overhang metrics give the real ink
            // edges, so no cap-height constant is guessed here.
            let oh = unsafe { tl.GetOverhangMetrics()? };
            let ink = [[-oh.left, -oh.top], [bw + oh.right, bh + oh.bottom]];
            let at = [
                cell.left + (cell.width - (ink[1][0] - ink[0][0])) * 0.5 - ink[0][0],
                cell.top + (cell.height - (ink[1][1] - ink[0][1])) * 0.5 - ink[0][1],
            ];
            out.push(InlineDraw {
                layout: tl,
                at,
                size: [bw, bh],
            });
        }
        Ok(out)
    }

    /// Natural content extent (box-local px) — what `auto_size` boxes adopt.
    /// Includes trailing whitespace so the caret always has somewhere to sit.
    pub fn natural_size(&self, item: &TextItem, dpi: u32) -> Result<[f32; 2]> {
        // Measure unwrapped: a huge box, so the text's own line breaks decide.
        let mut probe = item.clone();
        probe.size = [MAX_SPRITE as f32, MAX_SPRITE as f32];
        let layout = self.layout(&probe, dpi)?;
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut m)? };
        let w = m.width.max(m.widthIncludingTrailingWhitespace);
        Ok([w.max(1.0), m.height.max(1.0)])
    }

    /// Shape + rasterize one item into its cached sprite. `None` when the
    /// item has no visible ink (empty text). `origin`/rotation/outline are all
    /// baked; the sprite lands at `item.pos` + the returned local offset.
    pub fn render(&self, item: &TextItem, dpi: u32) -> Result<Option<Arc<RenderedText>>> {
        if item.text.is_empty() {
            return Ok(None);
        }
        let layout = self.layout(item, dpi)?;

        // Ink bounds in layout space: metrics ∪ overhang, padded for the edge.
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut m)? };
        let oh = unsafe { layout.GetOverhangMetrics()? };
        let (bw, bh) = (item.size[0].max(1.0), item.size[1].max(1.0));
        let ink_x0 = (m.left).min(-oh.left).min(0.0);
        let ink_y0 = (m.top).min(-oh.top).min(0.0);
        let ink_x1 = (m.left + m.width.max(m.widthIncludingTrailingWhitespace))
            .max(bw + oh.right)
            .max(bw.min(m.left + m.width));
        let ink_y1 = (m.top + m.height).max(bh + oh.bottom);

        // Furigana lives OUTSIDE the base layout's metrics — above the first
        // line, or right of the first column — so the sprite has to grow to
        // hold it or the readings are cropped off the top edge. Measured, not
        // guessed with a padding constant: a long reading over a short word
        // overhangs sideways too. A 縦中横 group is measured the same way for
        // the same reason: it is wider than the column it sits in.
        let mut extras = self.ruby_draws(item, dpi, &layout)?;
        extras.extend(self.tcy_draws(item, dpi, &layout)?);
        let (mut ink_x0, mut ink_y0, mut ink_x1, mut ink_y1) = (ink_x0, ink_y0, ink_x1, ink_y1);
        for r in &extras {
            ink_x0 = ink_x0.min(r.at[0]);
            ink_y0 = ink_y0.min(r.at[1]);
            ink_x1 = ink_x1.max(r.at[0] + r.size[0]);
            ink_y1 = ink_y1.max(r.at[1] + r.size[1]);
        }

        let pad = item.outline_px.max(0.0).ceil() + 2.0;

        // Rotate the ink rect's corners around the box center → sprite bbox.
        let c = [bw * 0.5, bh * 0.5];
        let (sin, cos) = item.rotation.sin_cos();
        let rot = |p: [f32; 2]| -> [f32; 2] {
            let v = [p[0] - c[0], p[1] - c[1]];
            [
                v[0] * cos - v[1] * sin + c[0],
                v[0] * sin + v[1] * cos + c[1],
            ]
        };
        let corners = [
            rot([ink_x0, ink_y0]),
            rot([ink_x1, ink_y0]),
            rot([ink_x1, ink_y1]),
            rot([ink_x0, ink_y1]),
        ];
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for p in corners {
            lo[0] = lo[0].min(p[0]);
            lo[1] = lo[1].min(p[1]);
            hi[0] = hi[0].max(p[0]);
            hi[1] = hi[1].max(p[1]);
        }
        let sx = (lo[0] - pad).floor();
        let sy = (lo[1] - pad).floor();
        let w = ((hi[0] + pad).ceil() - sx) as i64;
        let h = ((hi[1] + pad).ceil() - sy) as i64;
        if w <= 0 || h <= 0 || w > MAX_SPRITE as i64 || h > MAX_SPRITE as i64 {
            return Ok(None);
        }
        let (w, h) = (w as u32, h as u32);

        // Draw the layout rotated about the box center, shifted so the sprite
        // starts at (0,0). Row-vector convention: p' = p·R + t.
        let t = [
            c[0] - (c[0] * cos - c[1] * sin) - sx,
            c[1] - (c[0] * sin + c[1] * cos) - sy,
        ];
        let xform = Matrix3x2 {
            M11: cos,
            M12: sin,
            M21: -sin,
            M22: cos,
            M31: t[0],
            M32: t[1],
        };

        let bmp = unsafe {
            self.wic
                .CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)?
        };
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let rt = unsafe { self.d2d.CreateWicBitmapRenderTarget(&bmp, &props)? };
        let col = |c: [u8; 3], a: f32| D2D1_COLOR_F {
            r: c[0] as f32 / 255.0,
            g: c[1] as f32 / 255.0,
            b: c[2] as f32 / 255.0,
            a,
        };
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            // ClearType needs an opaque backdrop; grayscale AA composes onto
            // transparency correctly.
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            rt.SetTransform(&xform);
            let brush = rt.CreateSolidColorBrush(&col(item.color, 1.0), None)?;
            rt.DrawTextLayout(
                Vector2 { X: 0.0, Y: 0.0 },
                &layout,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
            // The readings and the 縦中横 groups ride the SAME transform and
            // the same brush, so they rotate with the box and pick up the
            // item's colour and (later, on the finished sprite) its outline
            // for free.
            for r in &extras {
                rt.DrawTextLayout(
                    Vector2 {
                        X: r.at[0],
                        Y: r.at[1],
                    },
                    &r.layout,
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
            rt.EndDraw(None, None)?;
        }

        // Read back BGRA premultiplied.
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        unsafe {
            bmp.CopyPixels(std::ptr::null(), w * 4, &mut bgra)?;
        }
        let mut rgba = vec![0u8; bgra.len()];
        for (dst, src) in rgba.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }

        if item.outline_px > 0.05 {
            apply_outline(
                &mut rgba,
                w as usize,
                h as usize,
                item.outline_px,
                item.outline_color,
            );
        }

        Ok(Some(Arc::new(RenderedText {
            origin: [
                (item.pos[0] + sx).round() as i32,
                (item.pos[1] + sy).round() as i32,
            ],
            size: [w, h],
            rgba,
        })))
    }

    /// Box-local point → UTF-16 caret position, plus **affinity**: true when
    /// the point landed on the trailing half of the character, so the caret
    /// belongs to the END of the line that character is on rather than the
    /// start of the next one. At a soft wrap those are the same position and
    /// two different places on the page — see `caret`.
    pub fn hit_test_point(&self, item: &TextItem, dpi: u32, p: [f32; 2]) -> Result<(u32, bool)> {
        let layout = self.layout(item, dpi)?;
        let mut trailing = BOOL::default();
        let mut inside = BOOL::default();
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe { layout.HitTestPoint(p[0], p[1], &mut trailing, &mut inside, &mut m)? };
        let t = trailing.as_bool();
        Ok((m.textPosition + if t { m.length } else { 0 }, t))
    }

    /// Caret geometry at a UTF-16 position.
    ///
    /// `affinity` = the caret trails the character before it. It only matters
    /// where a line WRAPPED: position N is both the end of the wrapped line
    /// and the start of the next, and DirectWrite's leading answer is always
    /// the second one — which is why End used to drop the caret onto the line
    /// below. Asking for the trailing edge of N−1 gets the first.
    pub fn caret(&self, item: &TextItem, dpi: u32, pos: u32, affinity: bool) -> Result<CaretPos> {
        let layout = self.layout(item, dpi)?;
        let mut x = 0f32;
        let mut y = 0f32;
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        let (at, trailing) = if affinity && pos > 0 {
            (pos - 1, true)
        } else {
            (pos, false)
        };
        unsafe { layout.HitTestTextPosition(at, trailing, &mut x, &mut y, &mut m)? };
        Ok(CaretPos {
            point: [x, y],
            cell: [m.left, m.top, m.width, m.height],
        })
    }

    /// Highlight rectangles (box-local px) for a UTF-16 range.
    pub fn selection_rects(
        &self,
        item: &TextItem,
        dpi: u32,
        a: u32,
        b: u32,
    ) -> Result<Vec<[f32; 4]>> {
        let (a, b) = (a.min(b), a.max(b));
        if a == b {
            return Ok(Vec::new());
        }
        let layout = self.layout(item, dpi)?;
        let mut count = 0u32;
        // First call sizes the buffer (E_NOT_SUFFICIENT_BUFFER is expected).
        let _ = unsafe { layout.HitTestTextRange(a, b - a, 0.0, 0.0, None, &mut count) };
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut hits = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
        unsafe { layout.HitTestTextRange(a, b - a, 0.0, 0.0, Some(&mut hits), &mut count)? };
        hits.truncate(count as usize);
        Ok(hits
            .iter()
            .filter(|m| m.width > 0.0 || m.height > 0.0)
            .map(|m| [m.left, m.top, m.width, m.height])
            .collect())
    }

    /// Up/down caret motion: one visual line (or vertical column) in `dir`
    /// (+1 = next line in reading order). `goal` preserves the cross-axis
    /// coordinate across repeated presses; pass the previous return value.
    ///
    /// Takes the caret's `affinity` (so Up from an end-of-wrapped-line caret
    /// starts from the line it is drawn on, not the one below) and returns the
    /// landing affinity: stepping onto a SHORTER line clamps the goal past its
    /// last glyph, and that landing is an end-of-line caret in its own right.
    pub fn line_move(
        &self,
        item: &TextItem,
        dpi: u32,
        pos: u32,
        affinity: bool,
        dir: i32,
        goal: Option<f32>,
    ) -> Result<(u32, f32, bool)> {
        let cp = self.caret(item, dpi, pos, affinity)?;
        let (target, kept_goal);
        if item.vertical {
            // Columns advance right-to-left: next line = one column left.
            let g = goal.unwrap_or(cp.point[1]);
            let step = cp.cell[2].max(1.0);
            target = [cp.cell[0] + cp.cell[2] * 0.5 - dir as f32 * step, g];
            kept_goal = g;
        } else {
            let g = goal.unwrap_or(cp.point[0]);
            let step = cp.cell[3].max(1.0);
            target = [g, cp.cell[1] + cp.cell[3] * 0.5 + dir as f32 * step];
            kept_goal = g;
        }
        let (landed, trailing) = self.hit_test_point(item, dpi, target)?;
        Ok((landed, kept_goal, trailing))
    }

    /// Start and end (before any trailing newline) of the visual line
    /// containing `pos` — Home/End targets.
    pub fn line_bounds(&self, item: &TextItem, dpi: u32, pos: u32) -> Result<(u32, u32)> {
        let layout = self.layout(item, dpi)?;
        let mut count = 0u32;
        let _ = unsafe { layout.GetLineMetrics(None, &mut count) };
        if count == 0 {
            return Ok((0, 0));
        }
        let mut lines = vec![DWRITE_LINE_METRICS::default(); count as usize];
        unsafe { layout.GetLineMetrics(Some(&mut lines), &mut count)? };
        lines.truncate(count as usize);
        let mut start = 0u32;
        for (i, l) in lines.iter().enumerate() {
            let end = start + l.length;
            if pos < end || i + 1 == lines.len() {
                return Ok((start, end.saturating_sub(l.newlineLength).max(start)));
            }
            start = end;
        }
        Ok((start, start))
    }
}

fn enumerate_families(dwrite: &IDWriteFactory) -> Result<Vec<String>> {
    let mut coll: Option<IDWriteFontCollection> = None;
    unsafe { dwrite.GetSystemFontCollection(&mut coll, false)? };
    let Some(coll) = coll else {
        return Ok(Vec::new());
    };
    let n = unsafe { coll.GetFontFamilyCount() };
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let Ok(fam) = (unsafe { coll.GetFontFamily(i) }) else {
            continue;
        };
        let Ok(names) = (unsafe { fam.GetFamilyNames() }) else {
            continue;
        };
        let mut idx = 0u32;
        let mut exists = BOOL::default();
        for locale in [w!("ja-jp"), w!("en-us")] {
            if unsafe { names.FindLocaleName(locale, &mut idx, &mut exists) }.is_err() {
                exists = BOOL::default();
            }
            if exists.as_bool() {
                break;
            }
        }
        if !exists.as_bool() {
            idx = 0;
        }
        let Ok(len) = (unsafe { names.GetStringLength(idx) }) else {
            continue;
        };
        let mut buf = vec![0u16; len as usize + 1];
        if unsafe { names.GetString(idx, &mut buf) }.is_ok() {
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Grow an outline (edge colour) around the glyph alpha via an exact Euclidean
/// distance transform, then composite the original fill back over it. Round
/// joins for free — exactly the manga フチ look.
///
/// The transform itself moved to `mn_core::edge` when the per-layer border
/// effect (LP-002/LP-003) needed the same geometry: text フチ and a layer
/// keyline are now the same code, and cannot drift apart.
fn apply_outline(rgba: &mut [u8], w: usize, h: usize, radius: f32, color: [u8; 3]) {
    use mn_core::edge::INF;
    // Squared distance to the nearest "inked" pixel (alpha ≥ 128).
    let mut d2 = vec![INF; w * h];
    for (i, px) in rgba.chunks_exact(4).enumerate() {
        if px[3] >= 128 {
            d2[i] = 0.0;
        }
    }
    mn_core::edge::dist_sq(&mut d2, w, h);
    // Composite: fill over outline.
    for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
        let oa = (radius + 0.5 - d2[i].sqrt()).clamp(0.0, 1.0);
        if oa <= 0.0 {
            continue;
        }
        let fa = px[3] as f32 / 255.0;
        let blend = oa * (1.0 - fa);
        for c in 0..3 {
            px[c] = (px[c] as f32 + color[c] as f32 * blend).round().min(255.0) as u8;
        }
        px[3] = ((fa + blend) * 255.0).round().min(255.0) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mn_core::text::StyleFlag;

    fn engine() -> TextEngine {
        TextEngine::new().expect("DirectWrite factory")
    }

    fn item(text: &str, vertical: bool) -> TextItem {
        let mut t = TextItem::new([0.0, 0.0], "Meiryo".into(), 24.0, [0, 0, 0], vertical);
        t.insert(0, text);
        t.size = [400.0, 400.0];
        t.auto_size = false;
        t
    }

    fn ink_count(r: &RenderedText) -> usize {
        r.rgba.chunks_exact(4).filter(|p| p[3] > 64).count()
    }

    /// plans/05 item 5: the layout cache — every caller shares ONE build
    /// per (content, dpi). Repeats hit; a content change (text, size_pt)
    /// or a dpi change misses. The content-HASH key (not the plan's
    /// revision counter) is what makes the size_pt case safe: app closures
    /// mutate layout fields outside any core funnel, and a hash cannot go
    /// stale. NOTE natural_size measures through a huge-box PROBE clone —
    /// its own stable key, one build for any number of calls.
    #[test]
    fn the_layout_cache_hits_until_content_or_dpi_moves() {
        let e = engine();
        let it = item("cache me", false);
        let before = e.layout_builds.get();
        let _ = e.natural_size(&it, 600);
        let _ = e.natural_size(&it, 600);
        assert_eq!(
            e.layout_builds.get(),
            before + 1,
            "the probe key builds once however often natural_size asks"
        );
        // The real-box callers share a second key: first line_bounds pays
        // the build, the caret and repeats ride it.
        let _ = e.line_bounds(&it, 600, 0);
        assert_eq!(e.layout_builds.get(), before + 2);
        let _ = e.line_bounds(&it, 600, 2);
        let _ = e.caret(&it, 600, 1, false).ok();
        assert_eq!(e.layout_builds.get(), before + 2, "caret rides the same key");
        // Text changed → new key → rebuild.
        let mut it2 = it.clone();
        it2.insert(0, "x");
        let _ = e.line_bounds(&it2, 600, 0);
        assert_eq!(e.layout_builds.get(), before + 3);
        // Size changed — the closure-mutation case → rebuild.
        let mut it3 = it.clone();
        it3.size_pt = 40.0;
        let _ = e.line_bounds(&it3, 600, 0);
        assert_eq!(e.layout_builds.get(), before + 4);
        // DPI changed → rebuild ...
        let _ = e.line_bounds(&it, 300, 0);
        assert_eq!(e.layout_builds.get(), before + 5);
        // ... and the earlier entries are still cached.
        let _ = e.line_bounds(&it, 600, 0);
        assert_eq!(e.layout_builds.get(), before + 5);
    }

    #[test]
    fn renders_ink_and_respects_orientation() {
        let e = engine();
        let h = e
            .render(&item("あいうえお", false), 96)
            .unwrap()
            .expect("sprite");
        assert!(ink_count(&h) > 50, "horizontal text has ink");
        let hs = e.natural_size(&item("あいうえお", false), 96).unwrap();
        let vs = e.natural_size(&item("あいうえお", true), 96).unwrap();
        assert!(hs[0] > hs[1], "horizontal runs wide: {hs:?}");
        assert!(vs[1] > vs[0], "vertical stacks tall: {vs:?}");
        let v = e
            .render(&item("あいうえお", true), 96)
            .unwrap()
            .expect("sprite");
        assert!(ink_count(&v) > 50, "vertical text has ink");
    }

    #[test]
    fn empty_text_renders_none_but_carets_work() {
        let e = engine();
        let t = item("", false);
        assert!(e.render(&t, 96).unwrap().is_none());
        let c = e.caret(&t, 96, 0, false).unwrap();
        assert!(c.cell[3] > 1.0, "empty layout still has a line height");
    }

    #[test]
    fn outline_adds_edge_pixels() {
        let e = engine();
        let mut t = item("あ", false);
        let plain = e.render(&t, 96).unwrap().unwrap();
        t.outline_px = 4.0;
        t.outline_color = [255, 0, 0];
        let edged = e.render(&t, 96).unwrap().unwrap();
        assert!(ink_count(&edged) > ink_count(&plain), "edge grows coverage");
        let red = edged
            .rgba
            .chunks_exact(4)
            .filter(|p| p[3] > 128 && p[0] > 128 && p[1] < 64)
            .count();
        assert!(red > 20, "outline colour is present ({red})");
    }

    #[test]
    fn styles_change_the_raster() {
        let e = engine();
        let plain = e.render(&item("test text", false), 96).unwrap().unwrap();
        let mut bold = item("test text", false);
        bold.set_style(0, bold.utf16_len(), StyleFlag::Bold, true);
        let b = e.render(&bold, 96).unwrap().unwrap();
        assert!(ink_count(&b) > ink_count(&plain), "bold is heavier");
        let mut under = item("test text", false);
        under.set_style(0, under.utf16_len(), StyleFlag::Underline, true);
        let u = e.render(&under, 96).unwrap().unwrap();
        assert!(ink_count(&u) > ink_count(&plain), "underline adds ink");
    }

    #[test]
    fn rotation_rotates_the_sprite() {
        let e = engine();
        let flat = e
            .render(&item("ながいテキスト", false), 96)
            .unwrap()
            .unwrap();
        let mut rot = item("ながいテキスト", false);
        rot.rotation = std::f32::consts::FRAC_PI_2;
        let r = e.render(&rot, 96).unwrap().unwrap();
        // A quarter turn swaps the ink's aspect: the sprite covers the box
        // either way, so compare ink bounding boxes.
        let bounds = |s: &RenderedText| {
            let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
            for y in 0..s.size[1] {
                for x in 0..s.size[0] {
                    if s.rgba[((y * s.size[0] + x) * 4 + 3) as usize] > 64 {
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x);
                        y1 = y1.max(y);
                    }
                }
            }
            (x1.saturating_sub(x0), y1.saturating_sub(y0))
        };
        let (fw, fh) = bounds(&flat);
        let (rw, rh) = bounds(&r);
        assert!(fw > fh * 2, "flat text is wide");
        assert!(rh > rw * 2, "rotated text is tall ({rw}x{rh})");
    }

    #[test]
    fn strike_adds_ink() {
        let e = engine();
        let plain = e.render(&item("test text", false), 96).unwrap().unwrap();
        let mut t = item("test text", false);
        t.set_style(0, t.utf16_len(), StyleFlag::Strike, true);
        let s = e.render(&t, 96).unwrap().unwrap();
        assert!(ink_count(&s) > ink_count(&plain), "strikethrough adds ink");
    }

    #[test]
    fn letter_spacing_widens_the_line() {
        let e = engine();
        let base = e.natural_size(&item("あいうえお", false), 96).unwrap();
        let mut t = item("あいうえお", false);
        t.letter_spacing_pt = 6.0;
        let wide = e.natural_size(&t, 96).unwrap();
        // 6 pt = 8 px at 96 dpi across 4 gaps ⇒ well over 20 px wider.
        assert!(wide[0] > base[0] + 20.0, "{base:?} vs {wide:?}");
    }

    #[test]
    fn line_spacing_scales_and_100_is_natural() {
        let e = engine();
        let text = |ls: LineSpacing| {
            let mut t = item("あい\nうえ", false);
            t.line_spacing = ls;
            t
        };
        let auto = e.natural_size(&text(LineSpacing::Auto), 96).unwrap();
        let p100 = e
            .natural_size(&text(LineSpacing::Percent(100.0)), 96)
            .unwrap();
        let p200 = e
            .natural_size(&text(LineSpacing::Percent(200.0)), 96)
            .unwrap();
        let pt80 = e.natural_size(&text(LineSpacing::Pt(80.0)), 96).unwrap();
        assert!(
            (auto[1] - p100[1]).abs() < 2.0,
            "100% == natural: {auto:?} {p100:?}"
        );
        assert!(p200[1] > auto[1] * 1.5, "200% stretches: {auto:?} {p200:?}");
        assert!(
            pt80[1] > auto[1] * 1.2,
            "absolute 80 pt lines: {auto:?} {pt80:?}"
        );
    }

    #[test]
    fn alignment_and_frame_position_move_the_block() {
        let e = engine();
        // Two ~120 px lines wrapped in a 200 px box: row alignment shifts
        // where line 1 starts on x.
        let mut t = item("あいうえお あいうえお", false);
        t.size = [200.0, 400.0];
        t.auto_size = false;
        let lead = e.caret(&t, 96, 0, false).unwrap().point[0];
        t.align = Align::Center;
        let mid = e.caret(&t, 96, 0, false).unwrap().point[0];
        t.align = Align::Trailing;
        let trail = e.caret(&t, 96, 0, false).unwrap().point[0];
        assert!(
            mid > lead + 4.0,
            "centered line starts right of leading ({lead} {mid})"
        );
        assert!(
            trail > mid + 4.0,
            "trailing starts right of center ({mid} {trail})"
        );

        // Single short line in a tall box: Near pins it to the top, Far
        // sinks it toward the bottom.
        let mut b = item("あ", false);
        b.size = [400.0, 300.0];
        b.auto_size = false;
        let near_y = e.caret(&b, 96, 0, false).unwrap().point[1];
        b.frame_align = FrameAlign::Far;
        let far_y = e.caret(&b, 96, 0, false).unwrap().point[1];
        assert!(
            far_y > near_y + 100.0,
            "Far sinks the block ({near_y} {far_y})"
        );
    }

    #[test]
    fn hit_testing_roundtrips() {
        let e = engine();
        let t = item("abc\ndef", false);
        let c = e.caret(&t, 96, 2, false).unwrap();
        let back = e
            .hit_test_point(&t, 96, [c.point[0] + 0.1, c.point[1] + c.cell[3] * 0.5])
            .unwrap();
        assert_eq!(back, (2, false), "leading half of the character at 2");
        let rects = e.selection_rects(&t, 96, 0, 3).unwrap();
        assert!(!rects.is_empty());
        // Line motion: from "b" down into the second line.
        let (down, _, _) = e.line_move(&t, 96, 1, false, 1, None).unwrap();
        assert!((4..=7).contains(&down), "moved into line 2: {down}");
        let (start, end) = e.line_bounds(&t, 96, 1).unwrap();
        assert_eq!((start, end), (0, 3));
        let (s2, e2) = e.line_bounds(&t, 96, 5).unwrap();
        assert_eq!((s2, e2), (4, 7));
    }

    /// A soft wrap has ONE UTF-16 position for two places on the page: the
    /// end of the line that wrapped and the start of the line under it.
    /// Pressing End (or clicking past the last glyph) means the first, and
    /// DirectWrite only answers that when it is asked with `trailing`.
    #[test]
    fn caret_at_a_soft_wrap_stays_on_the_line_that_ended() {
        let e = engine();
        let mut t = item("aaaa bbbb cccc dddd", false);
        t.size = [120.0, 400.0]; // narrow enough to wrap
        let (_, end) = e.line_bounds(&t, 96, 0).unwrap();
        assert!(
            end > 0 && end < t.utf16_len(),
            "the string wrapped somewhere: {end}"
        );
        let home = e.caret(&t, 96, 0, false).unwrap();
        let at_end = e.caret(&t, 96, end, true).unwrap();
        assert!(
            (at_end.point[1] - home.point[1]).abs() < 1.0,
            "End stays on line 1 ({} vs {})",
            at_end.point[1],
            home.point[1]
        );
        assert!(
            at_end.point[0] > home.point[0] + 10.0,
            "and sits at the END of it ({} vs {})",
            at_end.point[0],
            home.point[0]
        );
        // Without the trailing bit the same position is the start of line 2 —
        // the bug this pins.
        let leading = e.caret(&t, 96, end, false).unwrap();
        assert!(
            leading.point[1] > home.point[1],
            "the leading answer is the next line down"
        );

        // Vertical: columns instead of rows, same ambiguity.
        let mut v = item("あいうえおかきくけこさしすせそ", true);
        v.size = [400.0, 120.0];
        let (_, vend) = e.line_bounds(&v, 96, 0).unwrap();
        assert!(vend > 0 && vend < v.utf16_len(), "the column wrapped: {vend}");
        let vhome = e.caret(&v, 96, 0, false).unwrap();
        let vtail = e.caret(&v, 96, vend, true).unwrap();
        assert!(
            (vtail.point[0] - vhome.point[0]).abs() < 1.0,
            "End stays in column 1 ({} vs {})",
            vtail.point[0],
            vhome.point[0]
        );
        assert!(
            vtail.point[1] > vhome.point[1] + 10.0,
            "at the bottom of it ({} vs {})",
            vtail.point[1],
            vhome.point[1]
        );
    }

    #[test]
    fn vertical_line_move_walks_columns() {
        let e = engine();
        let t = item("あい\nうえ", true);
        // Caret in column 1 (あい); next line = column to the LEFT.
        let (moved, _, _) = e.line_move(&t, 96, 0, false, 1, None).unwrap();
        assert!((3..=6).contains(&moved), "into second column: {moved}");
    }

    #[test]
    fn families_enumerate_and_default_resolves() {
        let e = engine();
        assert!(!e.families().is_empty());
        // What this test is actually for: enumeration works and the default
        // resolves to something REAL. It used to require Meiryo to be
        // installed, which encoded "the development laptop" as a
        // requirement — a CI runner is a stock Windows Server with no
        // Japanese fonts, and the fallback chain exists precisely so that
        // machine still gets a usable face.
        let d = e.default_family();
        assert!(e.has_family(&d), "default family {d} is installed");
    }
}

#[cfg(test)]
mod ruby_render_tests {
    use super::*;

    fn engine() -> TextEngine {
        TextEngine::new().expect("DirectWrite factory")
    }

    fn item(text: &str, vertical: bool) -> TextItem {
        let mut t = TextItem::new([0.0, 0.0], "Meiryo".into(), 24.0, [0, 0, 0], vertical);
        t.insert(0, text);
        t.size = [400.0, 400.0];
        t.auto_size = false;
        t
    }

    fn ink(r: &RenderedText) -> usize {
        r.rgba.chunks_exact(4).filter(|p| p[3] > 64).count()
    }

    /// The whole point: a reading puts MORE ink on the page than the base
    /// text alone. Without this the feature can "work" while drawing
    /// nothing, which is exactly how a two-pass draw fails.
    #[test]
    fn a_reading_adds_ink() {
        let e = engine();
        let plain = item("漢字", true);
        let mut ruby = plain.clone();
        assert!(ruby.set_ruby(0, 2, "かんじ"));

        let a = e.render(&plain, 96).unwrap().expect("sprite");
        let b = e.render(&ruby, 96).unwrap().expect("sprite");
        assert!(
            ink(&b) > ink(&a),
            "furigana must add ink: base {} vs annotated {}",
            ink(&a),
            ink(&b)
        );
    }

    /// Vertical Japanese sets the reading to the RIGHT of the column;
    /// horizontal sets it ABOVE the word. The sprite has to grow on the
    /// matching axis, or the readings are simply cropped away.
    #[test]
    fn the_sprite_grows_on_the_axis_the_reading_uses() {
        let e = engine();

        let v = item("漢字", true);
        let mut vr = v.clone();
        vr.set_ruby(0, 2, "かんじ");
        let (v0, v1) = (
            e.render(&v, 96).unwrap().unwrap(),
            e.render(&vr, 96).unwrap().unwrap(),
        );
        assert!(
            v1.size[0] > v0.size[0],
            "vertical text grows WIDER (reading sits right of the column): {} -> {}",
            v0.size[0],
            v1.size[0]
        );

        let h = item("漢字", false);
        let mut hr = h.clone();
        hr.set_ruby(0, 2, "かんじ");
        let (h0, h1) = (
            e.render(&h, 96).unwrap().unwrap(),
            e.render(&hr, 96).unwrap().unwrap(),
        );
        assert!(
            h1.size[1] > h0.size[1],
            "horizontal text grows TALLER (reading sits above the word): {} -> {}",
            h0.size[1],
            h1.size[1]
        );
    }

    /// An annotated item opens a gap between lines for the readings to live
    /// in; an item without any is left exactly as it was.
    #[test]
    fn readings_open_a_line_gap_and_only_for_annotated_items() {
        let e = engine();
        let two_lines = |ruby: bool| {
            let mut t = item("漢字\n仮名", true);
            if ruby {
                t.set_ruby(0, 2, "かんじ");
            }
            e.natural_size(&t, 96).unwrap()
        };
        let plain = two_lines(false);
        let annotated = two_lines(true);
        // Vertical: "line spacing" is the column pitch, i.e. the x extent.
        assert!(
            annotated[0] > plain[0],
            "the columns spread to make room: {} -> {}",
            plain[0],
            annotated[0]
        );

        // The negative control that matters for every existing file: an
        // item with no readings measures exactly as before.
        let before = e.natural_size(&item("漢字\n仮名", true), 96).unwrap();
        assert_eq!(before, plain, "unannotated text is untouched");
    }

    /// A reading whose base text was deleted must not render — the sprite
    /// goes back to what the plain text alone produces.
    #[test]
    fn clearing_the_reading_restores_the_plain_sprite() {
        let e = engine();
        let plain = item("漢字", true);
        let mut t = plain.clone();
        t.set_ruby(0, 2, "かんじ");
        t.set_ruby(0, 2, "");
        assert!(t.ruby.is_empty());
        let a = e.render(&plain, 96).unwrap().unwrap();
        let b = e.render(&t, 96).unwrap().unwrap();
        assert_eq!((a.size, ink(&a)), (b.size, ink(&b)));
    }
}

#[cfg(test)]
mod font_mix_tests {
    use super::*;

    fn engine() -> TextEngine {
        TextEngine::new().expect("DirectWrite factory")
    }

    fn item(text: &str) -> TextItem {
        let mut t = TextItem::new([0.0, 0.0], "Meiryo".into(), 24.0, [0, 0, 0], false);
        t.insert(0, text);
        t.size = [400.0, 200.0];
        t.auto_size = false;
        t
    }

    /// A range set in another family must actually reach DirectWrite. Two
    /// faces at the same size differ in metrics and outline, so identical
    /// pixels would mean `SetFontFamilyName` never landed — which is the
    /// silent way this feature fails.
    #[test]
    fn a_font_run_changes_the_pixels() {
        let e = engine();
        let plain = item("これはCOOLです");

        // A family that exists here and is not the item's own. Picked from
        // the live system list so the test cannot depend on a font the
        // machine happens not to have.
        let other = ["Impact", "Arial", "Courier New", "Times New Roman"]
            .into_iter()
            .find(|f| e.has_family(f) && *f != plain.font);
        let Some(other) = other else {
            println!("[test] SKIP: no second family installed");
            return;
        };

        let mut mixed = plain.clone();
        assert!(mixed.set_font_range(3, 7, other));

        let a = e.render(&plain, 96).unwrap().expect("sprite");
        let b = e.render(&mixed, 96).unwrap().expect("sprite");
        assert!(
            a.size != b.size || a.rgba != b.rgba,
            "setting COOL in {other} changed nothing — the range never reached DirectWrite"
        );
    }

    /// The negative control every existing file depends on: an item with no
    /// overrides renders exactly as it did before font runs existed.
    #[test]
    fn an_item_without_overrides_is_untouched() {
        let e = engine();
        let a = e.render(&item("これはCOOLです"), 96).unwrap().unwrap();
        let mut b_item = item("これはCOOLです");
        // Set and then clear: the item must land back on byte-identical ink.
        b_item.set_font_range(3, 7, "Impact");
        b_item.set_font_range(3, 7, "Meiryo");
        assert!(b_item.fonts.is_empty());
        let b = e.render(&b_item, 96).unwrap().unwrap();
        assert_eq!(a.size, b.size);
        assert_eq!(a.rgba, b.rgba);
    }
}

#[cfg(test)]
mod tcy_render_tests {
    use super::*;

    fn engine() -> TextEngine {
        TextEngine::new().expect("DirectWrite factory")
    }

    fn item(text: &str, vertical: bool) -> TextItem {
        let mut t = TextItem::new([0.0, 0.0], "Meiryo".into(), 24.0, [0, 0, 0], vertical);
        t.insert(0, text);
        t.size = [400.0, 400.0];
        t.auto_size = false;
        t
    }

    fn ink(r: &RenderedText) -> usize {
        r.rgba.chunks_exact(4).filter(|p| p[3] > 64).count()
    }

    /// Width and height of the inked area.
    fn ink_box(s: &RenderedText) -> (u32, u32) {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..s.size[1] {
            for x in 0..s.size[0] {
                if s.rgba[((y * s.size[0] + x) * 4 + 3) as usize] > 64 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x1.saturating_sub(x0), y1.saturating_sub(y0))
    }

    /// The feature has to reach the pixels at all. Identical rasters would
    /// mean neither the substitution nor the second draw landed — the silent
    /// way a two-pass draw fails.
    #[test]
    fn a_tcy_run_changes_the_vertical_render() {
        let e = engine();
        let plain = item("22時", true);
        let mut marked = plain.clone();
        assert!(marked.set_tcy(0, 2, true));
        let a = e.render(&plain, 96).unwrap().expect("sprite");
        let b = e.render(&marked, 96).unwrap().expect("sprite");
        assert!(
            a.size != b.size || a.rgba != b.rgba,
            "marking 22 as 縦中横 changed nothing"
        );
    }

    /// The two claims that separate a working 縦中横 from a broken one.
    ///
    /// UPRIGHT: two digits sitting side by side ink a WIDE box; the same two
    /// digits laid on their side and stacked down the column ink a TALL one.
    ///
    /// ONCE: the base characters must not also draw themselves. If they did,
    /// the ink would be the digits twice — sideways under upright — so it is
    /// compared against the same digits rendered horizontally, which is
    /// exactly one copy of them at the same size.
    #[test]
    fn the_digits_are_set_upright_and_drawn_only_once() {
        let e = engine();
        let stacked = e.render(&item("22", true), 96).unwrap().expect("sprite");
        let mut t = item("22", true);
        t.set_tcy(0, 2, true);
        let upright = e.render(&t, 96).unwrap().expect("sprite");
        let flat = e.render(&item("22", false), 96).unwrap().expect("sprite");

        let (sw, sh) = ink_box(&stacked);
        let (uw, uh) = ink_box(&upright);
        assert!(sh > sw, "rotated and stacked, 22 inks a tall box ({sw}x{sh})");
        assert!(uw > uh, "upright, 22 inks a wide box ({uw}x{uh})");

        // Nothing else is on this line, so the column is as narrow as the
        // hole itself and the pair is condensed into the em — hence the
        // tolerance rather than an equality. What is being ruled out is a
        // SECOND copy, which lands three quarters of it outside the first.
        let (one, drawn) = (ink(&flat), ink(&upright));
        assert!(
            drawn < one * 3 / 2,
            "the base characters drew too: {drawn} px against {one} for one copy of 22"
        );
        assert!(
            drawn * 2 > one,
            "the digits are missing ink: {drawn} px against {one} for one copy of 22"
        );
    }

    /// The whole design in one measurement: a 縦中横 run occupies EXACTLY
    /// one character cell down the column, whatever its length. Two digits
    /// that were two cells become one, and everything after them moves up by
    /// precisely one em.
    #[test]
    fn a_run_takes_exactly_one_cell_down_the_column() {
        let e = engine();
        let plain = item("午後22時", true);
        let mut marked = plain.clone();
        marked.set_tcy(2, 4, true);
        let em = font_px(&plain, 96);

        let a = e.natural_size(&plain, 96).unwrap();
        let b = e.natural_size(&marked, 96).unwrap();
        assert!(
            (b[1] - em * 4.0).abs() < 1.0,
            "午, 後, the pair in ONE cell, 時 — four em cells: {b:?}"
        );
        assert!(
            a[1] > b[1] + 1.0,
            "sideways, the digits took their own advance instead: {a:?} -> {b:?}"
        );
        let cb = e.caret(&marked, 96, 4, false).unwrap();
        assert!(
            (cb.cell[1] - em * 3.0).abs() < 1.0,
            "時 starts the fourth cell: {cb:?}"
        );
    }

    /// The negative control every existing file depends on: an item with no
    /// runs renders exactly as it did before 縦中横 existed, to the byte.
    #[test]
    fn an_item_without_runs_is_untouched() {
        let e = engine();
        let a = e.render(&item("22時", true), 96).unwrap().unwrap();
        let mut t = item("22時", true);
        t.set_tcy(0, 2, true);
        assert!(t.set_tcy(0, 2, false));
        assert!(t.tcy.is_empty());
        let b = e.render(&t, 96).unwrap().unwrap();
        assert_eq!(a.size, b.size);
        assert_eq!(a.rgba, b.rgba);
    }

    /// 縦中横 means "horizontal inside vertical". A run left on an item that
    /// is then turned horizontal must do nothing at all rather than punch a
    /// hole in the line.
    #[test]
    fn horizontal_text_ignores_its_runs() {
        let e = engine();
        let a = e.render(&item("22時", false), 96).unwrap().unwrap();
        let mut t = item("22時", false);
        t.set_tcy(0, 2, true);
        let b = e.render(&t, 96).unwrap().unwrap();
        assert_eq!(a.size, b.size);
        assert_eq!(a.rgba, b.rgba);
    }

    /// Carets, selections and readings are UTF-16 positions in the string
    /// DirectWrite laid out, so the substitution must not change its length
    /// — a run inside a longer line must leave every position after it where
    /// it was.
    #[test]
    fn the_substitution_keeps_every_index_meaning_the_same_thing() {
        let e = engine();
        let mut t = item("午後22時\n半", true);
        let em = font_px(&t, 96);
        let before = e.line_bounds(&t, 96, 0).unwrap();
        assert_eq!(before, (0, 5));
        t.set_tcy(2, 4, true);
        assert_eq!(
            e.line_bounds(&t, 96, 0).unwrap(),
            before,
            "the line still holds the same five positions"
        );
        // 時 is position 4 either way: a full character cell, not one of the
        // two half-cells the run was replaced by.
        let full = e.caret(&t, 96, 4, false).unwrap();
        let inside = e.caret(&t, 96, 2, false).unwrap();
        assert!((full.cell[3] - em).abs() < 1.0, "position 4 is 時: {full:?}");
        assert!(
            (inside.cell[3] - em / 2.0).abs() < 1.0,
            "position 2 is half the hole the two digits left: {inside:?}"
        );
    }
}

#[cfg(test)]
mod ruby_style_tests {
    use super::*;
    use mn_core::text::Align;

    fn engine() -> TextEngine {
        TextEngine::new().expect("DirectWrite factory")
    }

    fn annotated(vertical: bool) -> TextItem {
        let mut t = TextItem::new([0.0, 0.0], "Meiryo".into(), 24.0, [0, 0, 0], vertical);
        t.insert(0, "漢字です");
        t.size = [400.0, 400.0];
        t.auto_size = false;
        t.set_ruby(0, 2, "かんじ");
        t
    }

    fn ink(r: &RenderedText) -> usize {
        r.rgba.chunks_exact(4).filter(|p| p[3] > 64).count()
    }

    /// CSP ships 67 %, JIS says 50 %, so the percentage has to be reachable
    /// — and it has to actually reach the glyphs.
    #[test]
    fn the_reading_size_percentage_changes_the_reading() {
        let e = engine();
        let small = annotated(true);
        let mut big = small.clone();
        big.ruby_style.size_pct = 100.0;

        let a = e.render(&small, 96).unwrap().unwrap();
        let b = e.render(&big, 96).unwrap().unwrap();
        assert!(
            ink(&b) > ink(&a),
            "a bigger reading must put more ink down: {} vs {}",
            ink(&a),
            ink(&b)
        );
    }

    /// The gap pushes the reading AWAY from the base — outward, on the axis
    /// the orientation uses. It therefore grows the sprite.
    #[test]
    fn the_gap_pushes_the_reading_off_the_base() {
        let e = engine();
        let tight = annotated(true);
        let mut loose = tight.clone();
        loose.ruby_style.gap_pt = 6.0;

        let a = e.render(&tight, 96).unwrap().unwrap();
        let b = e.render(&loose, 96).unwrap().unwrap();
        assert!(
            b.size[0] > a.size[0],
            "vertical text: the gap is horizontal ({} -> {})",
            a.size[0],
            b.size[0]
        );
    }

    /// Alignment moves the reading ALONG the base run. Centre and start
    /// cannot produce the same picture unless the setting is ignored.
    #[test]
    fn alignment_moves_the_reading_along_the_word() {
        let e = engine();
        let mut centre = annotated(true);
        centre.ruby_style.align = Align::Center;
        let mut start = centre.clone();
        start.ruby_style.align = Align::Leading;

        let a = e.render(&centre, 96).unwrap().unwrap();
        let b = e.render(&start, 96).unwrap().unwrap();
        assert!(
            a.rgba != b.rgba || a.size != b.size,
            "centred and start-aligned readings rendered identically"
        );
    }

    /// "Adjust reading" nudges along the same axis; a nudge of zero must be
    /// the same picture, and a real one must not be.
    #[test]
    fn adjust_nudges_and_zero_is_a_no_op() {
        let e = engine();
        let plain = annotated(false);
        let mut zero = plain.clone();
        zero.ruby_style.offset_pt = 0.0;
        let mut moved = plain.clone();
        moved.ruby_style.offset_pt = 4.0;

        let a = e.render(&plain, 96).unwrap().unwrap();
        let z = e.render(&zero, 96).unwrap().unwrap();
        let m = e.render(&moved, 96).unwrap().unwrap();
        assert_eq!((a.size, a.rgba.len()), (z.size, z.rgba.len()));
        assert_eq!(a.rgba, z.rgba, "a zero adjust changed the render");
        assert!(m.rgba != a.rgba || m.size != a.size, "the adjust did nothing");
    }

    /// The defaults are the typographic ones, and an item that never touches
    /// the panel renders exactly as it did before the panel existed.
    #[test]
    fn the_defaults_are_jis_not_csp() {
        let s = mn_core::text::RubyStyle::default();
        assert_eq!(s.size_pct, 50.0, "JIS X 4051; CSP's 67 is a setting away");
        assert_eq!(s.align, Align::Center);
        assert_eq!(s.gap_pt, 0.0);
        assert_eq!(s.offset_pt, 0.0);
        assert!(s.font.is_none(), "readings follow their base by default");
    }
}

#[cfg(test)]
mod auto_tcy_render_tests {
    use super::*;

    fn engine() -> TextEngine {
        TextEngine::new().expect("DirectWrite factory")
    }

    fn item(text: &str, vertical: bool) -> TextItem {
        let mut t = TextItem::new([0.0, 0.0], "Meiryo".into(), 24.0, [0, 0, 0], vertical);
        t.insert(0, text);
        t.size = [400.0, 400.0];
        t.auto_size = false;
        t
    }

    /// The claim Auto 縦中横 makes, tested against the thing it claims to be
    /// equal to: a digit the setting found renders EXACTLY as the same digit
    /// marked by hand. Not "differs from off" — that would pass if auto put
    /// the cell anywhere at all.
    #[test]
    fn an_auto_run_renders_identically_to_the_hand_marked_one() {
        let e = engine();
        let off = item("第1話", true);
        let mut auto = off.clone();
        auto.auto_tcy = 1;
        let mut manual = off.clone();
        assert!(manual.set_tcy(1, 2, true));

        let a = e.render(&off, 96).unwrap().expect("sprite");
        let b = e.render(&auto, 96).unwrap().expect("sprite");
        let c = e.render(&manual, 96).unwrap().expect("sprite");
        assert!(
            a.size != b.size || a.rgba != b.rgba,
            "Auto 縦中横 changed nothing — the derived run never reached the layout"
        );
        assert_eq!(b.size, c.size, "auto and hand-marked disagree on the sprite");
        assert_eq!(
            b.rgba, c.rgba,
            "auto put the upright cell somewhere the hand-marked run does not"
        );
    }

    /// One em down the column for the whole run, exactly as for a marked one
    /// — the measurement that says the hole was sized and not just filled.
    #[test]
    fn an_auto_run_takes_exactly_one_cell_down_the_column() {
        let e = engine();
        let mut t = item("午後22時", true);
        t.auto_tcy = 2;
        let em = font_px(&t, 96);
        let m = e.natural_size(&t, 96).unwrap();
        assert!(
            (m[1] - em * 4.0).abs() < 1.0,
            "午, 後, the pair in ONE cell, 時 — four em cells: {m:?}"
        );
    }

    /// A run past the limit is left lying down, to the byte. This is the
    /// negative control for the limit itself: if the length test were
    /// off-by-one or absent, a phone number would stand upright in one cell.
    #[test]
    fn a_run_past_the_limit_renders_as_if_the_setting_were_off() {
        let e = engine();
        let off = item("12345時", true);
        let mut on = off.clone();
        on.auto_tcy = 4;
        let a = e.render(&off, 96).unwrap().unwrap();
        let b = e.render(&on, 96).unwrap().unwrap();
        assert_eq!(a.size, b.size);
        assert_eq!(a.rgba, b.rgba);
    }

    /// Horizontal text ignores the setting, like it ignores marked runs —
    /// and an item left at the default renders exactly as it did before the
    /// setting existed, which is what every file already on disk needs.
    #[test]
    fn horizontal_text_and_the_default_are_both_untouched() {
        let e = engine();
        let flat = item("22時", false);
        let mut flat_auto = flat.clone();
        flat_auto.auto_tcy = 2;
        let a = e.render(&flat, 96).unwrap().unwrap();
        let b = e.render(&flat_auto, 96).unwrap().unwrap();
        assert_eq!(a.size, b.size);
        assert_eq!(a.rgba, b.rgba, "縦中横 leaked into horizontal text");

        let plain = item("22時", true);
        assert_eq!(plain.auto_tcy, 0, "new items are off until asked");
        let c = e.render(&plain, 96).unwrap().unwrap();
        let mut zero = plain.clone();
        zero.auto_tcy = 0;
        let d = e.render(&zero, 96).unwrap().unwrap();
        assert_eq!(c.rgba, d.rgba);
    }
}
