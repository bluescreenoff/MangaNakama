//! Edit ▸ Transform: the float's DATA — lifted source, absolute affine
//! params, gesture snapshot, the overlay preview image. The pointer
//! gestures live in `canvas_input` (arms of the canvas dispatch chain);
//! commit goes through `mn_core::transform::commit_transform` on
//! `AppCmd::TransformCommit`. Future work lands here: the GPU resample
//! (`Renderer::transform_region`) and the Tool Property numeric fields
//! (via `AppCmd::TransformUpdate`, which shares `TransformDrag::set_params`).
/// An in-progress Transform drag: the lifted region is live, the overlay
/// shows the bounding box with drag handles, Enter commits, Esc cancels.
pub struct TransformDrag {
    /// The lifted float source (CPU for now, GPU path deferred).
    pub source: mn_core::FloatSource,
    /// Current transform (derived from the params by `set_params`).
    pub xform: mn_core::Affine2,
    /// Canvas-space bounding box corners after transform, for hit testing.
    pub bbox: [[f32; 2]; 4],
    /// Absolute transform params around the pivot (the source-rect centre
    /// unless `pivot_override` moves it — TR-003).
    pub sx: f32,
    pub sy: f32,
    pub rad: f32,
    pub tx: f32,
    pub ty: f32,
    /// TR-003: the moved reference point, canvas px. `None` = the
    /// source-rect centre (CSP's default).
    pub pivot_override: Option<[f32; 2]>,
    /// The active pointer gesture, while a press is down.
    pub gesture: Option<TransformGesture>,
    /// True for clipboard floats (TRIAGE 131): an IDENTITY commit must
    /// STAMP the float (its pixels are not on the layer — cut cleared them,
    /// copy never put them there) instead of taking the lifted-transform's
    /// "nothing moved" cancel path.
    pub stamp_on_identity: bool,
    /// True only for floats LIFTED off the layer (Edit ▸ Transform, Flip):
    /// the commit erases the source region. False for every paste —
    /// clipboard, OS DIB, material — whose pixels were never on the layer;
    /// clearing there turned Copy into Cut the moment the float moved
    /// (the r69–r115 audit's worst finding).
    pub clear_source: bool,
    /// The selection AS IT WAS AT LIFT TIME, for the source clear. The live
    /// selection can change while the float is open (Ctrl+D, a new lasso),
    /// and the clear must mirror what `lift_region` actually took.
    pub lift_selection: Option<mn_core::selection::Selection>,
    /// Paste-into-panel (owner HIGH 2026-08-18): when Some(folder), the
    /// COMMIT creates a fresh raster layer as that frame folder's topmost
    /// child and stamps there — the folder seal clips the art to the panel.
    /// `None` (every other float): stamp the active layer as before. The
    /// layer add is structural (clears history, like every layer-list
    /// change); cancel leaves nothing behind.
    pub create_in: Option<usize>,
    /// MT-034: where that fresh layer sits in the folder (material pastes
    /// set it from the palette dropdown; everything else keeps Above).
    pub order: crate::app::MaterialLayerOrder,
    /// Straight-alpha preview of the lifted source, uploaded once at lift;
    /// the overlay draws it through `xform` as a textured mesh (egui-wgpu
    /// renders it — the GPU path; only the lift-time readback is CPU).
    pub preview_tex: Option<egui::TextureHandle>,
}

/// What a press grabbed during an active Transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransformGrab {
    /// Inside the bbox: translate.
    Move,
    /// On bbox corner `i`: uniform scale + rotate, the grabbed corner
    /// tracks the pointer.
    Corner(usize),
    /// On bbox edge-midpoint `i` (TR-004): scale ONE axis — 0 top, 1
    /// right, 2 bottom, 3 left, in bbox-corner index space.
    Edge(usize),
    /// On the pivot marker (TR-003): move the reference point.
    Pivot,
    /// Outside the bbox: rotate around the pivot.
    Rotate,
}

/// Press state for one Transform gesture: the grab target plus the params
/// and anchor captured at press time.
#[derive(Clone, Copy)]
pub struct TransformGesture {
    pub grab: TransformGrab,
    /// Press point (canvas px).
    pub start: [f32; 2],
    /// The grabbed bbox corner as it sat at press (Corner gestures); the
    /// scale/rotate math anchors on this, so the corner tracks the pointer
    /// exactly instead of jumping by the hit-test slack.
    pub anchor: [f32; 2],
    /// Params at press.
    pub sx0: f32,
    pub sy0: f32,
    pub rad0: f32,
    pub tx0: f32,
    pub ty0: f32,
}

impl TransformDrag {
    /// The transform's pivot: the moved reference point (TR-003) or the
    /// source-rect centre.
    pub fn pivot(&self) -> [f32; 2] {
        self.pivot_override.unwrap_or_else(|| {
            let r = self.source.rect;
            [(r[0] + r[2]) as f32 * 0.5, (r[1] + r[3]) as f32 * 0.5]
        })
    }

    /// TR-003: move the reference point and re-derive from the SAME
    /// params. Deviation from CSP (recorded): the visible content shifts
    /// when the pivot moves — CSP also re-derives, but keeps the preview
    /// pinned differently; ours is the honest re-derivation.
    pub fn set_pivot(&mut self, p: [f32; 2]) {
        let (sx, sy, rad, tx, ty) = (self.sx, self.sy, self.rad, self.tx, self.ty);
        self.pivot_override = Some(p);
        self.set_params(sx, sy, rad, tx, ty);
    }

    /// TR-019/T-021: mirror about the pivot, horizontally or vertically,
    /// in CANVAS space — composed with any standing rotation, so a 30°
    /// rotated float flips as it reads on screen (R(θ)·diag(−1,1) =
    /// R(−θ)·diag(1,−1) and kin: the angle reflects and the opposite
    /// scale negates).
    pub fn flip(&mut self, horizontal: bool) {
        let (sx, sy, rad, tx, ty) = (self.sx, self.sy, self.rad, self.tx, self.ty);
        if horizontal {
            self.set_params(-sx, sy, -rad, tx, ty);
        } else {
            self.set_params(sx, -sy, -rad, tx, ty);
        }
    }

    /// True when the accumulated params amount to no visible change (the
    /// identity affine comes out of `set_params` bit-exact).
    pub fn is_identity(&self) -> bool {
        self.xform == mn_core::Affine2::IDENTITY
    }

    /// T-020: back to the state the transform was lifted in, WITHOUT
    /// leaving the transform — the alternative today is Esc and starting
    /// over, which also throws away the selection you lifted.
    ///
    /// The reference point is deliberately left where the user put it: it
    /// is a setting, not part of the transformation, and identity params
    /// derive the identity affine around ANY pivot, so the float lands back
    /// on its source pixels either way.
    pub fn reset(&mut self) {
        self.set_params(1.0, 1.0, 0.0, 0.0, 0.0);
    }

    /// Set absolute params and re-derive `xform` + `bbox` from the source
    /// rect. The one place the derived state is computed — the
    /// `TransformUpdate` command and the drag gestures share it.
    pub fn set_params(&mut self, sx: f32, sy: f32, rad: f32, tx: f32, ty: f32) {
        let r = self.source.rect;
        let pivot = self.pivot();
        self.sx = sx;
        self.sy = sy;
        self.rad = rad;
        self.tx = tx;
        self.ty = ty;
        self.xform = mn_core::Affine2::scale_rotate_around(pivot, sx, sy, rad, [tx, ty]);
        let corners = [
            [r[0] as f32, r[1] as f32],
            [r[2] as f32, r[1] as f32],
            [r[2] as f32, r[3] as f32],
            [r[0] as f32, r[3] as f32],
        ];
        for (out, c) in self.bbox.iter_mut().zip(corners) {
            *out = self.xform.apply(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drag over a 100x60 source rect at the origin — no tiles needed,
    /// the params and the derived bbox are all this exercises.
    fn drag() -> TransformDrag {
        let source = mn_core::FloatSource {
            tiles: Default::default(),
            rect: [0, 0, 100, 60],
        };
        let mut d = TransformDrag {
            source,
            xform: mn_core::Affine2::IDENTITY,
            bbox: [[0.0; 2]; 4],
            sx: 1.0,
            sy: 1.0,
            rad: 0.0,
            tx: 0.0,
            ty: 0.0,
            pivot_override: None,
            gesture: None,
            stamp_on_identity: false,
            clear_source: false,
            lift_selection: None,
            create_in: None,
            order: crate::app::MaterialLayerOrder::Above,
            preview_tex: None,
        };
        d.set_params(1.0, 1.0, 0.0, 0.0, 0.0);
        d
    }

    /// T-020: reset returns the float to the pixels it was lifted from —
    /// bit-exact identity, since that is what `is_identity` (and with it
    /// the commit path's "nothing moved" branch) tests.
    #[test]
    fn transform_reset_returns_to_the_lift() {
        let mut d = drag();
        let bbox0 = d.bbox;
        d.set_params(1.8, 0.4, 0.7, 33.0, -12.0);
        assert!(!d.is_identity(), "the test's own premise");
        d.reset();
        assert!(d.is_identity(), "reset must land on the identity affine");
        assert_eq!((d.sx, d.sy, d.rad, d.tx, d.ty), (1.0, 1.0, 0.0, 0.0, 0.0));
        assert_eq!(d.bbox, bbox0, "the box is back on the source rect");
    }

    /// The reference point is a SETTING, not part of the transformation:
    /// reset leaves it where the user put it, and identity params still
    /// derive the identity affine around it — so the float lands back on
    /// its source pixels whichever corner the pivot sits in.
    #[test]
    fn transform_reset_keeps_the_reference_point() {
        let mut d = drag();
        let bbox0 = d.bbox;
        d.set_pivot([0.0, 0.0]);
        d.set_params(2.0, 2.0, 1.0, 5.0, 5.0);
        d.reset();
        assert_eq!(d.pivot_override, Some([0.0, 0.0]), "the pivot survives");
        assert!(d.is_identity(), "identity around ANY pivot is identity");
        assert_eq!(d.bbox, bbox0);
    }
}

/// Build the overlay preview of a lifted source: straight-alpha RGBA, long
/// side clamped to `max_px` (the commit path resamples at full resolution;
/// this is interactive feedback only). Walks DESTINATION pixels, so the cost
/// is bounded by the preview size, never the source size.
pub fn transform_preview(src: &mn_core::FloatSource, max_px: u32) -> Option<egui::ColorImage> {
    let (w0, h0) = (
        (src.rect[2] - src.rect[0]).max(0) as u32,
        (src.rect[3] - src.rect[1]).max(0) as u32,
    );
    if w0 == 0 || h0 == 0 || src.tiles.is_empty() {
        return None;
    }
    let scale = (max_px as f32 / w0.max(h0) as f32).min(1.0);
    let (w, h) = (
        ((w0 as f32 * scale).ceil() as usize).max(1),
        ((h0 as f32 * scale).ceil() as usize).max(1),
    );
    let mut img = egui::ColorImage::new([w, h], vec![egui::Color32::TRANSPARENT; w * h]);
    for py in 0..h {
        let sy = src.rect[1] + ((py as f32 + 0.5) / scale) as i32;
        for px in 0..w {
            let sx = src.rect[0] + ((px as f32 + 0.5) / scale) as i32;
            let p = src.pixel(sx, sy);
            if p[3] == 0 {
                continue;
            }
            let a = p[3] as u32;
            // premultiplied fix15 → straight u8
            let un = |c: u16| -> u8 {
                (((c as u32 * 32768 / a).min(32768) * 255 + 16384) / 32768) as u8
            };
            img.pixels[py * w + px] = egui::Color32::from_rgba_unmultiplied(
                un(p[0]),
                un(p[1]),
                un(p[2]),
                ((a * 255 + 16384) / 32768) as u8,
            );
        }
    }
    Some(img)
}
