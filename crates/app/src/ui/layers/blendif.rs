//! Blend If — the Layer Property section for the underlying-luminance gate.
//!
//! One sentence for the artist: **this layer only shows where the page under
//! it is dark enough (or light enough)**. Tone that lands in the shadows and
//! nowhere else, a highlight that stays off the black ink, a texture that
//! only bites on the flats — without painting a mask and without touching a
//! pixel.
//!
//! # Why three controls and not a dialog
//!
//! Photoshop's Blend If is two split sliders (*This Layer* and *Underlying
//! Layer*) times four channels. The owner's ruling on 2026-08-30 was "build
//! it but keep it super basic for now", so this is the one arm anybody
//! actually uses — Underlying, on luminance — as **Show from / Show to /
//! Feather**, sitting in the property panel with the other layer switches
//! rather than behind a modal. The other arms are recorded as deferred in
//! `mn_core::blendif`'s module doc; the struct is shaped so they can be added
//! without moving a call site.
//!
//! The two range bars are clamped against each other HERE rather than left
//! to `BlendIf::normalized`. Normalising a crossed range is the right thing
//! for a file or a script, but under the pointer it reads as the two handles
//! swapping places mid-drag, which is not what the hand asked for.
//!
//! # Undo
//!
//! These are drag sliders that fire every frame, so they opt into the
//! `ParamEditSession` coalescing the live-layer Tool Property sliders use
//! (`crates/app/src/ui/property/gradient.rs`, `AppCmd::ParamEditSession`):
//! one press of Ctrl+Z takes a whole drag back, not one tick of it. The
//! checkbox and the reset button are finished one-shot gestures and record
//! their own step, which is the same opt-IN rule the rest of the app follows.

use super::super::theme::ValueBar;
use super::super::widgets::group_caption;
use crate::app::App;
use crate::cmd::AppCmd;
use mn_core::BlendIf;

/// The Layer Property section. Offered on painted layers only — a folder's
/// gate is refused by `Document::set_layer_blend_if` and ignored by every
/// compositor, so showing the control there would be a lie.
pub(super) fn section(ui: &mut egui::Ui, app: &mut App, i: usize) {
    let Some(l) = app.doc.layers.get(i) else {
        return;
    };
    if l.folder {
        return;
    }
    let cur = l.blend_if;

    ui.add_space(3.0);
    group_caption(ui, "Blend if (underlying)");

    let mut on = cur.is_some();
    if ui
        .checkbox(&mut on, "Only where the page below is in range")
        .on_hover_text(
            "the layer is hidden wherever the composite UNDERNEATH it falls outside the \
             brightness range below — tone that lands only in the shadows, a highlight that \
             stays off the ink. Nothing is erased and no mask is painted; switch it off and \
             the layer is back in full.",
        )
        .changed()
    {
        // ON starts fully open (a visible no-op) rather than guessing a
        // range: the artist then drags the end they meant and watches it
        // bite. Guessing "shadows" would hide half their layer on a tick.
        app.push_cmd(AppCmd::SetLayerBlendIf(i, on.then_some(BlendIf::FULL)));
        return;
    }
    let Some(cur) = cur else {
        return;
    };

    let mut g = cur;
    let mut bars: Vec<egui::Response> = Vec::new();

    let mut lo = g.lo * 100.0;
    let r = ValueBar::new("Show from", 0.0, 100.0)
        .decimals(0)
        .suffix("%")
        .show(ui, &mut lo);
    if r.changed() {
        // Clamped against the other handle instead of swapping with it.
        g.lo = (lo / 100.0).min(g.hi);
    }
    bars.push(r.on_hover_text(
        "the DARKEST underlying brightness this layer still shows on. 0% = no lower limit; \
         raise it and the layer lifts off the blacks.",
    ));

    let mut hi = g.hi * 100.0;
    let r = ValueBar::new("Show to", 0.0, 100.0)
        .decimals(0)
        .suffix("%")
        .show(ui, &mut hi);
    if r.changed() {
        g.hi = (hi / 100.0).max(g.lo);
    }
    bars.push(r.on_hover_text(
        "the BRIGHTEST underlying brightness this layer still shows on. 100% = no upper \
         limit; lower it and the layer drops off the paper white.",
    ));

    let mut feather = g.feather * 100.0;
    let r = ValueBar::new("Feather", 0.0, 50.0)
        .decimals(0)
        .suffix("%")
        .show(ui, &mut feather);
    if r.changed() {
        g.feather = feather / 100.0;
    }
    bars.push(r.on_hover_text(
        "how softly the layer fades out past each end of the range. 0% is a hard edge, which \
         shows as a contour line wherever the page crosses the limit. The fade sits OUTSIDE \
         the range, so widening it never eats into what you already dialled in.",
    ));

    ui.weak(describe(g)).on_hover_text(
        "brightness of the COMPOSITE below this layer, not of the layer itself. Inside a \
         folder that means the folder's own contents — unless the folder is set to Through, \
         where it means the page.",
    );

    if ui
        .small_button("✕ reset")
        .on_hover_text("back to the full range: the layer shows everywhere again")
        .clicked()
    {
        app.push_cmd(AppCmd::SetLayerBlendIf(i, Some(BlendIf::FULL)));
        return;
    }

    let changed = bars.iter().any(|r| r.changed());
    if changed {
        app.push_cmd(AppCmd::SetLayerBlendIf(i, Some(g)));
    }
    // The coalescing opt-in, copied beat for beat from the live-layer Tool
    // Property sliders: the session opens when a control MOVED with the
    // pointer held (pointer-down alone would also catch an unrelated canvas
    // drag) and closes when the pointer comes up. Queued as a command so it
    // lands AFTER this frame's edit — that ordering is what leaves the
    // drag's FIRST tick as the one that records the pre-image.
    let down = ui.ctx().input(|i| i.pointer.any_down());
    if changed && down {
        if app.param_session != Some(i) {
            app.push_cmd(AppCmd::ParamEditSession(Some(i)));
        }
    } else if !down && app.param_session.is_some() {
        app.push_cmd(AppCmd::ParamEditSession(None));
    }
}

/// The current range in words. A percentage pair says what the numbers are;
/// this says what they DO, which is the part that is hard to read off two
/// bars — especially "this is currently doing nothing".
fn describe(g: BlendIf) -> &'static str {
    let lo = g.lo > 0.0;
    let hi = g.hi < 1.0;
    match (lo, hi) {
        (false, false) => "full range — showing everywhere (no gate)",
        (true, false) => "shows on the LIGHTER parts of the page below",
        (false, true) => "shows on the DARKER parts of the page below",
        (true, true) => "shows on the mid-tones of the page below",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The words have to follow the numbers — a panel that says "shadows"
    /// while the range says highlights is worse than no caption at all.
    #[test]
    fn the_caption_follows_the_range() {
        assert_eq!(describe(BlendIf::FULL), "full range — showing everywhere (no gate)");
        assert_eq!(
            describe(BlendIf {
                lo: 0.0,
                hi: 0.4,
                feather: 0.1
            }),
            "shows on the DARKER parts of the page below"
        );
        assert_eq!(
            describe(BlendIf {
                lo: 0.6,
                hi: 1.0,
                feather: 0.1
            }),
            "shows on the LIGHTER parts of the page below"
        );
        assert_eq!(
            describe(BlendIf {
                lo: 0.3,
                hi: 0.7,
                feather: 0.0
            }),
            "shows on the mid-tones of the page below"
        );
    }

    /// The feather is not part of "is this doing anything": it points
    /// outward, so an open range with a feather is still open. The caption
    /// and `BlendIf::is_open` must agree about that.
    #[test]
    fn a_feather_alone_is_still_the_open_caption() {
        let g = BlendIf {
            feather: 0.4,
            ..BlendIf::FULL
        };
        assert!(g.is_open());
        assert_eq!(describe(g), "full range — showing everywhere (no gate)");
    }
}
