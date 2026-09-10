use super::super::*;
use crate::export::{Background, composite};

/// A 128×128 red image, which centres exactly over a 128×128 canvas.
fn red_page() -> image::RgbaImage {
    image::RgbaImage::from_pixel(128, 128, image::Rgba([255, 0, 0, 255]))
}

#[test]
fn import_with_no_selection_makes_a_plain_unmasked_layer() {
    let mut doc = Document::new(128, 128);
    let (at, masked) = doc.add_layer_from_image_masked("ref", &red_page());
    assert!(!masked, "no selection, nothing to mask against");
    assert!(
        doc.layers[at].mask.is_none(),
        "an all-hidden mask here would look exactly like a failed import"
    );
    let img = composite(&doc, Background::Transparent);
    assert_eq!(img.get_pixel(100, 5).0[3], 255, "the whole image is there");
}

#[test]
fn import_with_a_selection_hides_everything_outside_it() {
    let mut doc = Document::new(128, 128);
    // The left half only.
    doc.selection = Some(crate::selection::Selection::from_rect(
        &doc, 0.0, 0.0, 64.0, 128.0,
    ));
    let (at, masked) = doc.add_layer_from_image_masked("ref", &red_page());
    assert!(masked, "a selection was active, so the mask is the point");
    assert!(doc.layers[at].mask.is_some());

    let img = composite(&doc, Background::Transparent);
    assert_eq!(img.get_pixel(5, 5).0[3], 255, "inside the selection: kept");
    assert_eq!(img.get_pixel(100, 5).0[3], 0, "outside: hidden");
}

/// The mask hides; it does not destroy. Deleting it brings the whole
/// import back — which is the entire argument for a mask over a crop.
#[test]
fn the_mask_is_reversible_and_the_pixels_survive() {
    let mut doc = Document::new(128, 128);
    doc.selection = Some(crate::selection::Selection::from_rect(
        &doc, 0.0, 0.0, 64.0, 128.0,
    ));
    let (at, _) = doc.add_layer_from_image_masked("ref", &red_page());
    assert!(doc.mask_delete(at));
    let img = composite(&doc, Background::Transparent);
    assert_eq!(
        img.get_pixel(100, 5).0[3],
        255,
        "the pixels outside the selection were never thrown away"
    );
}
