//! The Layers palette (the big one) + Layer Property: full stack rows
//! (eye, label rail, thumbnail, rename, folder blocks, drag-reorder via
//! the LayerDrag payload), blend/opacity strip, thumbnails over a
//! checkerboard, and the Layer Property palette body.
//!
//! The two halves live in `rows.rs` and `property.rs`; this file keeps what
//! both of them read (the blend table, the per-type tool list, the layer
//! tints) and re-exports the two pane entries so callers see one module.

use crate::cmd::Tool;
use mn_core::{Blend, LayerKind};

mod blendif;
mod breakout;
mod property;
mod rows;

pub(super) use property::layer_property;
pub(super) use rows::layer_section;

// The picker order is OURS, not CSP's: parts 1, 2 and 3 in the order they
// shipped, appended never re-sorted, because a saved workspace and the
// owner's muscle memory both index into this list. So Color burn sits at the
// bottom rather than next to Color dodge — the search field is the way to
// reach a mode by name.
//
// 27 of CSP's 28. The missing one is Add (Glow): our Add is already the
// premultiplied, saturating add, which IS the stronger of CSP's two — see
// the deviation note in `mn_core::blend`.
pub(super) const BLENDS: [Blend; 27] = [
    Blend::Normal,
    Blend::Multiply,
    Blend::Screen,
    Blend::Add,
    Blend::Subtract,
    Blend::Darken,
    Blend::Lighten,
    Blend::Overlay,
    Blend::SoftLight,
    Blend::HardLight,
    Blend::Difference,
    Blend::Exclusion,
    Blend::Hue,
    Blend::Saturation,
    Blend::Color,
    Blend::ColorBurn,
    Blend::LinearBurn,
    Blend::ColorDodge,
    Blend::GlowDodge,
    Blend::VividLight,
    Blend::LinearLight,
    Blend::PinLight,
    Blend::HardMix,
    Blend::Divide,
    Blend::DarkerColor,
    Blend::LighterColor,
    Blend::Luminosity,
];

/// `LP-025` Tool navigation: the tools that MEAN something on this layer
/// type, in the order a page is made. Every entry is one the layer will
/// actually accept — the mapping is the guards read forwards:
/// `Layer::paintable` for the paint family, `App::live_fill_active` for the
/// two live kinds (a brush there edits the layer's window mask), and
/// `App::guard_frame_layer`'s refusals for the vector kinds, each of which
/// keeps its own editor plus the Object tool.
///
/// **Layer-agnostic tools are deliberately absent** — Select, Wand,
/// Eyedropper and Pan work the same on every layer in the stack, so listing
/// them here would put four cells in every list and teach nothing. This bar
/// answers "what is different about THIS layer", which is the only question
/// a per-type bar can answer.
pub(super) fn tools_for_layer(l: &mn_core::Layer) -> &'static [Tool] {
    if l.folder {
        // A frame folder is still divisible and still an object; a plain
        // folder holds no pixels at all.
        return if l.is_frame() {
            &[Tool::Frame, Tool::Object]
        } else {
            &[]
        };
    }
    match l.kind {
        LayerKind::Text(_) => &[Tool::Text, Tool::Object],
        // The balloon's text is edited in place, so T belongs here too.
        LayerKind::Balloon(_) => &[Tool::Balloon, Tool::Text, Tool::Object],
        LayerKind::Frame(_) => &[Tool::Frame, Tool::Object],
        // Live layers: a brush edits the WINDOW, and the parameters live in
        // Tool Property rather than in a tool of their own.
        LayerKind::Fill(_) | LayerKind::Correction(_) => &[Tool::Pen, Tool::Eraser],
        // Row 166: the raster comes from a file and is re-derived, so no
        // drawing tool applies to it — the Object tool is still honest
        // (selection/inspection), and the repair lives in the row's ≡ menu.
        LayerKind::FileObject(_) => &[Tool::Object],
        // Vector inking: strokes are captured (inking does not ask
        // `paintable`), and Object is what edits the control points. The
        // pixel ops that `paintable` refuses are correctly absent.
        LayerKind::Raster if l.records_strokes() => &[Tool::Pen, Tool::Eraser, Tool::Object],
        LayerKind::Raster => &[
            Tool::Pen,
            Tool::Eraser,
            Tool::Fill,
            Tool::Gradient,
            Tool::Tone,
            Tool::Figure,
            Tool::Liquify,
        ],
    }
}

/// The Layer-colour chip set (CSP's default two-tone palette). The FIRST
/// entry is also what `AppCmd::ActiveLayer(ToggleColour)` turns a layer on
/// with, so the keyboard and the palette checkbox agree on "on".
pub(crate) const LAYER_TINTS: [[u8; 3]; 8] = [
    [0x2a, 0x6f, 0xf4], // blue
    [0xe5, 0x4b, 0x4b], // red
    [0x3f, 0xb2, 0x5e], // green
    [0xf2, 0xb8, 0x1c], // amber
    [0x9b, 0x59, 0xd0], // purple
    [0xe8, 0x7e, 0xb5], // pink
    [0x26, 0xc6, 0xc9], // cyan
    [0x8a, 0x8f, 0x98], // grey
];

pub(super) fn blend_name(b: Blend) -> &'static str {
    match b {
        Blend::Normal => "Normal",
        Blend::Multiply => "Multiply",
        Blend::Screen => "Screen",
        Blend::Darken => "Darken",
        Blend::Lighten => "Lighten",
        Blend::Add => "Add",
        Blend::Subtract => "Subtract",
        Blend::Overlay => "Overlay",
        Blend::SoftLight => "Soft light",
        Blend::HardLight => "Hard light",
        Blend::Difference => "Difference",
        Blend::Exclusion => "Exclusion",
        Blend::Hue => "Hue",
        Blend::Saturation => "Saturation",
        Blend::Color => "Color",
        Blend::ColorBurn => "Color burn",
        Blend::LinearBurn => "Linear burn",
        Blend::ColorDodge => "Color dodge",
        Blend::GlowDodge => "Glow dodge",
        Blend::VividLight => "Vivid light",
        Blend::LinearLight => "Linear light",
        Blend::PinLight => "Pin light",
        Blend::HardMix => "Hard mix",
        Blend::Divide => "Divide",
        Blend::DarkerColor => "Darker color",
        Blend::LighterColor => "Lighter color",
        // CSP's label. Photoshop/SVG call the same operator Luminosity;
        // the enum and the ORA name use theirs, the owner sees CSP's.
        Blend::Luminosity => "Brightness",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The palette's tests straddle both halves of the split (the row filter
    // and the per-type tool bar share one fixture set), so they stay here
    // and reach into the two child modules by name.
    use super::super::icons::Icon;
    use super::property::tool_icon;
    use super::rows::{LABEL_COLORS, LayerFilter, active_frame_folder, mask_thumb_image, row_glyph};
    use crate::app::LayerFilterKind;
    use mn_core::{Document, FillKind, FrameSet, TextSet};

    fn plain(kind: LayerFilterKind, needle: &str) -> LayerFilter {
        LayerFilter {
            needle: needle.to_owned(),
            kind,
            ref_only: false,
            no_draft: false,
            label: None,
            frame_scope: None,
            frame_scope_wanted: false,
        }
    }

    /// The colour filter matches a row's OWN label exactly — an unlabelled
    /// row and a differently-labelled row both drop.
    #[test]
    fn filter_matches_by_label_colour() {
        let mut d = stack();
        let blue = LABEL_COLORS[0];
        let red = LABEL_COLORS[1];
        d.layers[0].label = Some(blue);
        let mut f = plain(LayerFilterKind::All, "");
        f.label = Some(blue);
        let hits: Vec<usize> = (0..d.layers.len()).filter(|&i| f.passes(&d, i)).collect();
        assert_eq!(hits, vec![0], "only the blue-labelled row");
        f.label = Some(red);
        assert_eq!(
            (0..d.layers.len()).filter(|&i| f.passes(&d, i)).count(),
            0,
            "no red rows anywhere"
        );
    }

    fn empty_balloons() -> mn_core::BalloonSet {
        mn_core::BalloonSet {
            balloons: Vec::new(),
            border_px: 4.0,
            pressure_width: false,
        }
    }

    /// One layer of every kind the bar has to answer for.
    fn one_of_each() -> Vec<(&'static str, mn_core::Layer)> {
        let l = |k: LayerKind| {
            let mut l = mn_core::Layer::new("x");
            l.kind = k;
            l
        };
        let mut folder = mn_core::Layer::new("folder");
        folder.folder = true;
        let mut frame_folder = l(LayerKind::Frame(FrameSet::single_rect(
            [0.0, 0.0, 8.0, 8.0],
            2.0,
        )));
        frame_folder.folder = true;
        let mut vector = mn_core::Layer::new("vector");
        vector.strokes = Some(Default::default());
        vec![
            ("raster", mn_core::Layer::new("raster")),
            ("vector", vector),
            ("folder", folder),
            ("frame-folder", frame_folder),
            (
                "frame",
                l(LayerKind::Frame(FrameSet::single_rect(
                    [0.0, 0.0, 8.0, 8.0],
                    2.0,
                ))),
            ),
            ("balloon", l(LayerKind::Balloon(empty_balloons()))),
            ("text", l(LayerKind::Text(TextSet::default()))),
            (
                "fill",
                l(LayerKind::Fill(FillKind::Flat { color: [0.0; 4] })),
            ),
            (
                "tone",
                l(LayerKind::Fill(FillKind::Tone {
                    tone: mn_core::ToneParams::default(),
                    density: 0.4,
                })),
            ),
            (
                "correction",
                l(LayerKind::Correction(mn_core::Adjust::Invert)),
            ),
        ]
    }

    /// LP-025: the bar's contents per layer type. The table is asserted
    /// where it is a taste call, but the load-bearing half is the invariant
    /// underneath it — a tool is listed only where the app would actually
    /// take it, so the bar can never advertise a tool the guards refuse.
    #[test]
    fn tool_nav_lists_only_tools_the_layer_accepts() {
        for (name, l) in one_of_each() {
            let tools = tools_for_layer(&l);
            let mut seen = tools.to_vec();
            seen.sort_by_key(|t| format!("{t:?}"));
            seen.dedup();
            assert_eq!(seen.len(), tools.len(), "{name}: a tool listed twice");
            for &t in tools {
                assert!(
                    tool_icon(t).is_some(),
                    "{name}: {t:?} has no strip icon to draw"
                );
            }

            // Brushes: exactly the three cases that take one — a plain
            // raster (`paintable`), a stroke-recording raster (inking is
            // captured), and the live kinds (the brush edits the window).
            let live = matches!(l.kind, LayerKind::Fill(_) | LayerKind::Correction(_));
            let takes_brush = l.paintable() || l.records_strokes() || live;
            assert_eq!(
                tools.contains(&Tool::Pen),
                takes_brush,
                "{name}: the pen must be listed exactly where a stroke lands"
            );
            assert_eq!(tools.contains(&Tool::Eraser), takes_brush, "{name}");

            // The raster-edit family is `paintable`'s own list: everything
            // else re-derives its pixels and would lose the edit.
            for t in [Tool::Fill, Tool::Gradient, Tool::Tone, Tool::Liquify] {
                assert!(
                    !tools.contains(&t) || l.paintable(),
                    "{name}: {t:?} needs a paintable layer"
                );
            }
            // Object edits geometry, so it is offered exactly where there
            // is geometry to edit.
            let has_geometry = l.is_vector() || l.records_strokes();
            assert!(
                !tools.contains(&Tool::Object) || has_geometry,
                "{name}: nothing for the Object tool to grab"
            );
            // Text only where text lives.
            assert!(
                !tools.contains(&Tool::Text) || l.is_text() || l.is_balloon(),
                "{name}: the text tool would make a new layer here"
            );
            // Layer-agnostic tools are never listed (see the doc comment).
            for t in [Tool::Select, Tool::Wand, Tool::Eyedrop, Tool::Pan] {
                assert!(!tools.contains(&t), "{name}: {t:?} works everywhere");
            }
            // Only a plain folder gets an empty bar; every other type has
            // at least one tool to offer.
            assert_eq!(
                tools.is_empty(),
                name == "folder",
                "{name}: {tools:?} — only a plain folder holds nothing"
            );
        }

        // The two taste calls worth pinning by name.
        let mut balloon = mn_core::Layer::new("b");
        balloon.kind = LayerKind::Balloon(empty_balloons());
        assert_eq!(
            tools_for_layer(&balloon).to_vec(),
            vec![Tool::Balloon, Tool::Text, Tool::Object],
            "a balloon's text is edited in place, so T belongs on its bar"
        );
        let mut ff = mn_core::Layer::new("ff");
        ff.folder = true;
        ff.kind = LayerKind::Frame(FrameSet::single_rect([0.0, 0.0, 8.0, 8.0], 2.0));
        assert_eq!(
            tools_for_layer(&ff).to_vec(),
            vec![Tool::Frame, Tool::Object],
            "a frame folder is still divisible"
        );
    }

    /// A stack with one of each thing the filter can name.
    fn stack() -> Document {
        let mut d = Document::new(200, 200);
        d.layers[0].name = "rough sketch".into();
        d.layers[0].draft = true;
        d.add_text_layer("Dialogue", TextSet { texts: Vec::new() });
        d.layers.last_mut().unwrap().reference = true;
        d
    }

    /// SL-004 + SL-001: the name test is a case-insensitive substring and
    /// the type test names the layer kinds, not their storage.
    #[test]
    fn filter_matches_by_name_and_type() {
        let d = stack();
        let f = plain(LayerFilterKind::All, "DIALOG".to_lowercase().as_str());
        let hits: Vec<usize> = (0..d.layers.len()).filter(|&i| f.passes(&d, i)).collect();
        assert_eq!(hits.len(), 1, "one name match");
        assert!(d.layers[hits[0]].is_text());

        let f = plain(LayerFilterKind::Text, "");
        assert_eq!(
            (0..d.layers.len()).filter(|&i| f.passes(&d, i)).count(),
            1,
            "one text layer"
        );
        let f = plain(LayerFilterKind::Raster, "");
        let hits: Vec<usize> = (0..d.layers.len()).filter(|&i| f.passes(&d, i)).collect();
        assert_eq!(hits, vec![0], "raster = neither folder nor vector kind");
    }

    /// SL-002/SL-003: the two property narrowings, and the fact that they
    /// AND with the rest rather than replacing it.
    #[test]
    fn filter_narrows_by_property() {
        let d = stack();
        let mut f = plain(LayerFilterKind::All, "");
        f.no_draft = true;
        assert!(!f.passes(&d, 0), "the draft row is excluded");
        assert!(f.passes(&d, 1));

        let mut f = plain(LayerFilterKind::All, "");
        f.ref_only = true;
        assert!(!f.passes(&d, 0));
        assert!(f.passes(&d, 1), "the reference row survives");

        // AND, not OR: a reference row whose name misses still fails.
        let mut f = plain(LayerFilterKind::All, "zzz");
        f.ref_only = true;
        assert!(!f.passes(&d, 1));
    }

    /// The owner's r-round eye test: every row type must LOOK different.
    /// Pins the marker each kind resolves to, and the two overlaps that
    /// decide the order — a frame FOLDER is both, and a tone/vector layer
    /// is an ordinary raster with something recorded beside it.
    #[test]
    fn every_layer_kind_gets_its_own_glyph() {
        let mut d = Document::new(200, 200);
        assert_eq!(row_glyph(&d.layers[0]), None, "a plain raster stays bare");

        let li = d.add_layer("Vector 1");
        d.layers[li].strokes = Some(mn_core::StrokeSet::default());
        assert_eq!(row_glyph(&d.layers[li]), Some(Icon::Vector));

        let li = d.add_layer("Tone 1");
        d.layers[li].tone = Some(mn_core::tone::ToneParams::default());
        assert_eq!(row_glyph(&d.layers[li]), Some(Icon::Tone));
        // A tone that also records strokes is still a TONE: the screen is
        // what the row shows on the canvas.
        d.layers[li].strokes = Some(mn_core::StrokeSet::default());
        assert_eq!(row_glyph(&d.layers[li]), Some(Icon::Tone));

        let li = d.add_layer("Flat");
        d.layers[li].kind = LayerKind::Fill(FillKind::Flat {
            color: [0.0, 0.0, 0.0, 1.0],
        });
        assert_eq!(row_glyph(&d.layers[li]), Some(Icon::Fill));
        // A LIVE fill layer whose parameters ARE a screentone reads as one.
        d.layers[li].kind = LayerKind::Fill(FillKind::Tone {
            tone: mn_core::tone::ToneParams::default(),
            density: 0.5,
        });
        assert_eq!(row_glyph(&d.layers[li]), Some(Icon::Tone));

        d.add_text_layer("Dialogue", TextSet { texts: Vec::new() });
        assert_eq!(row_glyph(d.layers.last().unwrap()), Some(Icon::Text));
        d.add_balloon_layer(
            "Bubbles",
            mn_core::BalloonSet {
                balloons: Vec::new(),
                border_px: 2.0,
                pressure_width: false,
            },
        );
        assert_eq!(row_glyph(d.layers.last().unwrap()), Some(Icon::Balloon));

        // A frame folder is a folder AND a frame — the koma marker wins.
        let hdr = d.add_frame_folder("Frame 1", FrameSet::single_rect([1.0, 1.0, 9.0, 9.0], 2.0));
        assert_eq!(row_glyph(&d.layers[hdr]), Some(Icon::Frame));
        let plain = d.add_folder_above(hdr, "Group");
        assert_eq!(row_glyph(&d.layers[plain]), Some(Icon::Folder));

        // Row 166: a file object's pixels are ordinary tiles, so the glyph
        // is the ONLY thing on the row that says it is a live reference.
        let li = d.add_layer("Classroom");
        d.layers[li].kind = LayerKind::FileObject(mn_core::FileObject {
            path: "C:/art/classroom.png".into(),
            fit: (200, 200),
            stamp: Default::default(),
            missing: false,
        });
        assert_eq!(row_glyph(&d.layers[li]), Some(Icon::FileObject));
        // And no drawing tool applies to it — the derived-raster refusal,
        // reached through `is_vector()` rather than a new special case.
        let tools = tools_for_layer(&d.layers[li]);
        assert!(!d.layers[li].paintable());
        assert!(
            !tools.contains(&Tool::Pen) && !tools.contains(&Tool::Fill),
            "a file object offers no brush: {tools:?}"
        );
    }

    /// The mask cell's picture IS the coverage, and the rule that decides
    /// what an EMPTY mask looks like is the compositor's: an absent tile is
    /// unmasked, i.e. fully visible. So a fresh LM-001 mask reads as a light
    /// square (the layer shows through) and a zero-coverage tile reads dark.
    /// Get this backwards and every masked row shows the mask inverted.
    #[test]
    fn mask_thumb_paints_coverage_light_on_dark() {
        let mut d = Document::new(64, 64);
        d.layers[0].mask = Some(mn_core::doc::LayerMask {
            tiles: std::collections::HashMap::new(),
            enabled: true,
            revision: mn_core::next_revision(),
        });
        let img = mask_thumb_image(&d, 0);
        assert_eq!(img.pixels.len(), 20 * 20);
        assert!(
            img.pixels.iter().all(|c| c.r() > 0xc0),
            "no tiles = unmasked = light"
        );

        let m = d.layers[0].mask.as_mut().unwrap();
        // A transparent tile is zero coverage — the mask hides that region.
        m.tiles.insert(
            mn_core::TileIdx { x: 0, y: 0 },
            std::sync::Arc::new(mn_core::Tile::new_transparent()),
        );
        let img = mask_thumb_image(&d, 0);
        assert!(
            img.pixels.iter().all(|c| c.r() < 0x30),
            "zero coverage = hidden = dark"
        );
    }

    /// SL-003's manga row: the scope is the frame folder BLOCK — header
    /// plus every child — and the walk finds it from a layer nested
    /// inside, not only from the header itself.
    #[test]
    fn frame_folder_scope_covers_the_block() {
        let mut d = Document::new(200, 200);
        let hdr = d.add_frame_folder(
            "Frame 1",
            FrameSet::single_rect([10.0, 10.0, 90.0, 90.0], 2.0),
        );
        // add_frame_folder leaves the folder's own draw layer active.
        let inside = d.active;
        assert!(inside < hdr, "the draw layer is a child of the header");
        assert_eq!(
            active_frame_folder(&d, inside),
            Some(hdr),
            "walked up from a child"
        );
        assert_eq!(
            active_frame_folder(&d, hdr),
            Some(hdr),
            "the header is its own folder"
        );

        let block = d.block_range(hdr);
        let f = LayerFilter {
            needle: String::new(),
            kind: LayerFilterKind::All,
            ref_only: false,
            no_draft: false,
            label: None,
            frame_scope: Some(block.clone()),
            frame_scope_wanted: true,
        };
        for i in 0..d.layers.len() {
            assert_eq!(
                f.passes(&d, i),
                block.contains(&i),
                "layer {i} inside the block?"
            );
        }

        // Asked for a scope, no frame folder above the active layer: the
        // filter matches NOTHING, deliberately — the count row explains.
        let flat = Document::new(64, 64);
        assert_eq!(active_frame_folder(&flat, 0), None);
        let f = LayerFilter {
            needle: String::new(),
            kind: LayerFilterKind::All,
            ref_only: false,
            no_draft: false,
            label: None,
            frame_scope: None,
            frame_scope_wanted: true,
        };
        assert!(!f.passes(&flat, 0));
    }
}
