//! The margin stamp on exports — Work Settings ▸ "Print story + page
//! number in margins" (`print_margin_info`). The story title and the page
//! number are drawn into the bottom margin of FINISHED export pixels
//! only: never the canvas, never a preview, never print. Placement is
//! [`mn_core::export::margin_stamp_layout`] — pure, tested there, one
//! home for every eye-test constant; this module renders the two lines
//! through the app's TextEngine and composites them after the finish.

use mn_core::doc::LayerExpression;
use mn_core::export::{self, MarginStampLayout};
use mn_core::page::PageSetup;
use mn_core::text::{RenderedText, TextItem};

/// Stamp one page's margin info into a finished (or finish-bound) export
/// image. Free function, not a method: the Export All loop holds
/// `&mut app.renderer` while an earlier-captured closure calls this with
/// `app.text_engine` — disjoint-field borrows only survive if nothing
/// here needs `&App`.
///
/// `crop_px`/`scale`/`px_height` are the SAME knobs the finish was given
/// (or will be given); the placement maps the page's trim through them so
/// the stamp lands in the output where the margin is. A missing text
/// engine (DirectWrite refused to start) skips the stamp — a dead font
/// stack must not kill an export run.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stamp_margin_info(
    engine: Option<&mn_text::TextEngine>,
    font: &str,
    story: &str,
    img: &mut image::RgbaImage,
    setup: Option<&PageSetup>,
    crop_px: [u32; 4],
    scale: f32,
    px_height: u32,
    colour: LayerExpression,
    number: &str,
) {
    let Some(engine) = engine else { return };
    let (out_px, eff, applied) = export::finish_geometry((img.width(), img.height()), crop_px, scale, px_height);
    let out_dpi = setup
        .filter(|s| s.dpi > 0)
        .map(|s| ((s.dpi as f32 * eff) as u32).clamp(36, 2400))
        .unwrap_or(96);
    let number_sprite = stamp_sprite(engine, font, number, out_dpi);
    let story_sprite = if story.trim().is_empty() {
        None
    } else {
        stamp_sprite(engine, font, story.trim(), out_dpi)
    };
    let layout = margin_stamp_layout(
        setup,
        applied,
        eff,
        out_px,
        number_sprite.as_deref(),
        story_sprite.as_deref(),
    );
    export::apply_margin_stamp(
        img,
        &layout,
        story_sprite.as_deref(),
        number_sprite.as_deref(),
        colour,
    );
}

/// The layout with sprite sizes resolved — a tiny adapter so the sprite
/// dance stays out of the callers.
fn margin_stamp_layout(
    setup: Option<&PageSetup>,
    applied: [u32; 4],
    eff: f32,
    out_px: (u32, u32),
    number: Option<&RenderedText>,
    story: Option<&RenderedText>,
) -> MarginStampLayout {
    let size = |s: Option<&RenderedText>| s.map(|s| s.size).unwrap_or([0, 0]);
    export::margin_stamp_layout(setup, applied, eff, out_px, size(number), size(story))
}

/// One stamp line as a rasterised sprite: plain black text at the stamp's
/// point size, no edge, no wrap. The wrap box is the line's own natural
/// size plus a hair — `render` grows the sprite to the box, so a huge box
/// would balloon it past the sprite cap (and a tiny one would wrap).
fn stamp_sprite(
    engine: &mn_text::TextEngine,
    font: &str,
    text: &str,
    dpi: u32,
) -> Option<std::sync::Arc<RenderedText>> {
    let mut item = TextItem {
        id: 0,
        text: text.to_owned(),
        runs: Vec::new(),
        pos: [0.0, 0.0],
        size: [0.0, 0.0],
        auto_size: true,
        rotation: 0.0,
        font: font.to_owned(),
        size_pt: export::MARGIN_STAMP_PT,
        color: [0, 0, 0],
        outline_px: 0.0,
        outline_color: [255, 255, 255],
        vertical: false,
        align: Default::default(),
        frame_align: Default::default(),
        letter_spacing_pt: 0.0,
        line_spacing: Default::default(),
        ruby: Vec::new(),
        ruby_style: Default::default(),
        tcy: Vec::new(),
        auto_tcy: 0,
        fonts: Vec::new(),
        style: None,
        cache: None,
    };
    let [w, h] = engine.natural_size(&item, dpi).ok()?;
    item.size = [w + 8.0, h + 8.0];
    engine.render(&item, dpi).ok().flatten()
}
