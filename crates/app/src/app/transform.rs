//! Edit ▸ Transform: the float's DATA — lifted source, absolute affine
//! params, gesture snapshot, the overlay preview image. The pointer
//! gestures live in `canvas_input` (arms of the canvas dispatch chain);
//! commit goes through `mn_core::transform::commit_transform` on
//! `AppCmd::TransformCommit`. Future work lands here: a GPU resample
//! (unbuilt — `mn_core::transform::commit_transform`'s `resampled` param
//! is its seam) and the Tool Property numeric fields
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
    /// Owner 2026-08-24: a clipboard paste commits onto its OWN fresh
    /// layer even with no folder target — `add_layer_above` the active,
    /// never stamping the active layer's pixels. With `create_in` set it
    /// is redundant (the folder path already creates).
    pub paste_new_layer: bool,
    /// Set by the OBJECT tool's ink grab (owner 2026-08-24): a pure
    /// translation of that lift commits on pointer RELEASE — CSP's Object
    /// tool moves layers directly, and "drag the lineart somewhere, then
    /// press Enter" is the weird half of the gesture. A scale/rotate grab
    /// (corners, stalk, outside-box) keeps the float: those are transform
    /// work, not moves.
    pub object_lift: bool,
    /// MT-034: where that fresh layer sits in the folder (material pastes
    /// set it from the palette dropdown; everything else keeps Above).
    pub order: crate::app::MaterialLayerOrder,
    /// Straight-alpha preview of the lifted source, uploaded once at lift;
    /// the overlay draws it through `xform` as a textured mesh (egui-wgpu
    /// renders it — the GPU path; only the lift-time readback is CPU).
    pub preview_tex: Option<egui::TextureHandle>,
}

/// How far above the box's top edge the rotate stalk floats, in SCREEN px
/// (call sites divide by zoom to get canvas px). ONE value for every
/// rotatable object — frames, balloons, text boxes and the Transform float
/// all draw and hit-test the same lollipop, so it may not drift per object.
pub const ROTATE_STALK_SCREEN: f32 = 26.0;

/// What a press grabbed during an active Transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransformGrab {
    /// Inside the bbox: translate.
    Move,
    /// On bbox corner `i`: scale BOTH axes anchored at the opposite corner
    /// (CSP), the grabbed corner tracking the pointer. Never rotates —
    /// rotation is the stalk and the outside-the-box drag.
    Corner(usize),
    /// On bbox edge-midpoint `i` (TR-004): scale ONE axis — 0 top, 1
    /// right, 2 bottom, 3 left, in bbox-corner index space.
    Edge(usize),
    /// On the pivot marker (TR-003): move the reference point.
    Pivot,
    /// The rotate stalk above the top edge, or anywhere outside the bbox:
    /// rotate around the pivot.
    Rotate,
}

/// Press state for one Transform gesture: the grab target plus the params
/// and the box as it stood at press time.
#[derive(Clone, Copy)]
pub struct TransformGesture {
    pub grab: TransformGrab,
    /// Press point (canvas px).
    pub start: [f32; 2],
    /// The bbox as it sat at press. Corner/Edge scaling anchors on a corner
    /// or edge midpoint OF THIS box, so the anchor holds still and the
    /// grabbed handle tracks the pointer exactly — no jump by the hit-test
    /// slack, no drift as the params move under the drag.
    pub bbox0: [[f32; 2]; 4],
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

    /// The source rect's corners in the same order `set_params` derives
    /// `bbox` from: TL, TR, BR, BL.
    fn src_corners(&self) -> [[f32; 2]; 4] {
        let r = self.source.rect;
        [
            [r[0] as f32, r[1] as f32],
            [r[2] as f32, r[1] as f32],
            [r[2] as f32, r[3] as f32],
            [r[0] as f32, r[3] as f32],
        ]
    }

    /// The rotate stalk's tip, canvas px. The top-edge midpoint pushed along
    /// the box's OUTWARD TOP NORMAL — the box rotates with the float, so the
    /// offset follows it instead of being "up the screen".
    ///
    /// The hit test and the overlay both call this: a stalk you can see but
    /// not grab (or the reverse) is the classic drift between the two.
    pub fn stalk_point(&self, zoom: f32) -> [f32; 2] {
        let mid = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
        let top = mid(self.bbox[0], self.bbox[1]);
        let bottom = mid(self.bbox[2], self.bbox[3]);
        let v = [top[0] - bottom[0], top[1] - bottom[1]];
        let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
        // Degenerate (zero-height) box: fall back to straight up.
        let n = if len > 1e-6 {
            [v[0] / len, v[1] / len]
        } else {
            [0.0, -1.0]
        };
        let off = ROTATE_STALK_SCREEN / zoom.max(0.01);
        [top[0] + n[0] * off, top[1] + n[1] * off]
    }

    /// What a press at `p` (canvas px) grabs. Pure and `&self` so the cursor
    /// code can ask the same question on hover that the press answers.
    ///
    /// Priority — rotate stalk, then corners, then edge midpoints, then the
    /// reference-point marker (or any Alt press that missed a handle), then
    /// inside the quad, then everything else rotates.
    pub fn hit_test(&self, p: [f32; 2], zoom: f32, alt: bool) -> TransformGrab {
        let tol = (10.0 / zoom.max(0.01)).max(2.0);
        let d = |q: [f32; 2]| (q[0] - p[0]).abs() + (q[1] - p[1]).abs();
        if d(self.stalk_point(zoom)) <= tol * 1.4 {
            return TransformGrab::Rotate;
        }
        // Corner slack is capped at a third of the shortest side: on a small
        // float the four 14px discs would otherwise cover the whole box and
        // Move (and the edge handles) would be unreachable.
        let side = |i: usize| {
            let (a, b) = (self.bbox[i], self.bbox[(i + 1) % 4]);
            ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
        };
        let min_side = (0..4).map(side).fold(f32::INFINITY, f32::min);
        let corner_tol = (tol * 1.4).min(0.33 * min_side);
        let mut best: Option<(usize, f32)> = None;
        for (i, c) in self.bbox.iter().enumerate() {
            let dist = d(*c);
            if dist <= corner_tol && best.is_none_or(|(_, b)| dist < b) {
                best = Some((i, dist));
            }
        }
        if let Some((i, _)) = best {
            return TransformGrab::Corner(i);
        }
        for i in 0..4 {
            let (a, b) = (self.bbox[i], self.bbox[(i + 1) % 4]);
            if d([(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]) <= tol {
                return TransformGrab::Edge(i);
            }
        }
        if d(self.pivot()) <= tol * 1.2 || alt {
            return TransformGrab::Pivot;
        }
        if point_in_quad(p, self.bbox) {
            TransformGrab::Move
        } else {
            TransformGrab::Rotate
        }
    }

    /// Fold a pointer position into the absolute params for the gesture `g`.
    /// Pure (no renderer, no shell) so the CSP behaviours below are unit
    /// testable; `canvas_input` only reads the modifiers and calls this.
    ///
    /// CSP, verified against the real app: a CORNER scales both axes with
    /// the OPPOSITE CORNER pinned, an EDGE MIDPOINT scales one axis with the
    /// OPPOSITE EDGE pinned, and NEITHER changes the angle — rotation is the
    /// stalk and the outside-the-box drag. Alt switches scaling back to
    /// "about the reference point", which is CSP's other mode.
    pub fn apply_gesture(
        &mut self,
        g: &TransformGesture,
        p: [f32; 2],
        shift: bool,
        alt: bool,
        keep_aspect: bool,
    ) {
        if !p[0].is_finite() || !p[1].is_finite() {
            return;
        }
        let pivot = self.pivot();
        // The transform's centre in canvas space at press time.
        let c = [pivot[0] + g.tx0, pivot[1] + g.ty0];
        let (sin, cos) = g.rad0.sin_cos();
        // Into / out of the box's own frame (R0 and its inverse).
        let unrot = |v: [f32; 2]| [cos * v[0] + sin * v[1], -sin * v[0] + cos * v[1]];
        // Magnitude clamp that KEEPS THE SIGN: dragging a handle through the
        // anchor flips the float (CSP does), and a plain `clamp` would pin it
        // at +0.02 and refuse to mirror.
        let clamp_scale = |s: f32| -> f32 {
            if s.is_finite() {
                s.abs().clamp(0.02, 100.0).copysign(s)
            } else {
                0.02
            }
        };
        let snap45 = |a: f32| {
            let q = std::f32::consts::FRAC_PI_4;
            (a / q).round() * q
        };
        let mid = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
        let src = self.src_corners();
        match g.grab {
            TransformGrab::Move => {
                let (mut dx, mut dy) = (p[0] - g.start[0], p[1] - g.start[1]);
                if shift {
                    // Constrain to the nearer 45° ray through the press.
                    let len = (dx * dx + dy * dy).sqrt();
                    let a = snap45(dy.atan2(dx));
                    dx = len * a.cos();
                    dy = len * a.sin();
                }
                self.set_params(g.sx0, g.sy0, g.rad0, g.tx0 + dx, g.ty0 + dy);
            }
            TransformGrab::Corner(i) => {
                // Anchor: the opposite corner (Alt: the reference point).
                let (anchor, src_anchor) = if alt {
                    (c, pivot)
                } else {
                    (g.bbox0[(i + 2) % 4], src[(i + 2) % 4])
                };
                let u0 = unrot([g.bbox0[i][0] - anchor[0], g.bbox0[i][1] - anchor[1]]);
                let u = unrot([p[0] - anchor[0], p[1] - anchor[1]]);
                if u0[0].abs() <= 1e-3 || u0[1].abs() <= 1e-3 {
                    return;
                }
                let (mut rx, mut ry) = (u[0] / u0[0], u[1] / u0[1]);
                if keep_aspect || shift {
                    let r = (u[0] * u0[0] + u[1] * u0[1]) / (u0[0] * u0[0] + u0[1] * u0[1]);
                    rx = r;
                    ry = r;
                }
                let (sx, sy) = (clamp_scale(g.sx0 * rx), clamp_scale(g.sy0 * ry));
                self.pin(g, sx, sy, anchor, src_anchor, alt);
            }
            TransformGrab::Edge(i) => {
                // Anchor: the midpoint of the OPPOSITE edge (Alt: the
                // reference point, CSP's scale-from-reference-point).
                let m0 = mid(g.bbox0[i], g.bbox0[(i + 1) % 4]);
                let (anchor, src_anchor) = if alt {
                    (c, pivot)
                } else {
                    (
                        mid(g.bbox0[(i + 2) % 4], g.bbox0[(i + 3) % 4]),
                        mid(src[(i + 2) % 4], src[(i + 3) % 4]),
                    )
                };
                let u0 = unrot([m0[0] - anchor[0], m0[1] - anchor[1]]);
                let u = unrot([p[0] - anchor[0], p[1] - anchor[1]]);
                // Right/left edges move x, top/bottom move y — in the box's
                // frame, so a standing rotation tracks the pointer.
                let ax = i % 2 == 1;
                let (n0, n) = if ax { (u0[0], u[0]) } else { (u0[1], u[1]) };
                if n0.abs() <= 1e-3 {
                    return;
                }
                let r = n / n0;
                let both = keep_aspect || shift;
                let sx = if ax || both {
                    clamp_scale(g.sx0 * r)
                } else {
                    g.sx0
                };
                let sy = if !ax || both {
                    clamp_scale(g.sy0 * r)
                } else {
                    g.sy0
                };
                self.pin(g, sx, sy, anchor, src_anchor, alt);
            }
            TransformGrab::Pivot => {
                if shift {
                    // Snap the offset from the source-rect centre to 45°.
                    let r = self.source.rect;
                    let ctr = [(r[0] + r[2]) as f32 * 0.5, (r[1] + r[3]) as f32 * 0.5];
                    let (dx, dy) = (p[0] - ctr[0], p[1] - ctr[1]);
                    let len = (dx * dx + dy * dy).sqrt();
                    let a = snap45(dy.atan2(dx));
                    self.set_pivot([ctr[0] + len * a.cos(), ctr[1] + len * a.sin()]);
                } else {
                    self.set_pivot(p);
                }
            }
            TransformGrab::Rotate => {
                let v0 = [g.start[0] - c[0], g.start[1] - c[1]];
                let v1 = [p[0] - c[0], p[1] - c[1]];
                if v0[0] * v0[0] + v0[1] * v0[1] > 1e-6 && v1[0] * v1[0] + v1[1] * v1[1] > 1e-6 {
                    let mut da = v1[1].atan2(v1[0]) - v0[1].atan2(v0[0]);
                    if shift {
                        da = snap45(da);
                    }
                    self.set_params(g.sx0, g.sy0, g.rad0 + da, g.tx0, g.ty0);
                }
            }
        }
    }

    /// Apply a scale-only change and solve the translation that keeps
    /// `anchor` (the source point `src_anchor` under the transform) exactly
    /// where it was. Alt keeps the press-time translation instead, which is
    /// what makes Alt scale about the reference point.
    fn pin(
        &mut self,
        g: &TransformGesture,
        sx: f32,
        sy: f32,
        anchor: [f32; 2],
        src_anchor: [f32; 2],
        alt: bool,
    ) {
        if alt {
            self.set_params(sx, sy, g.rad0, g.tx0, g.ty0);
            return;
        }
        let pivot = self.pivot();
        let (sin, cos) = g.rad0.sin_cos();
        // R0 · S · (src_anchor − pivot), matching `scale_rotate_around`.
        let s = [
            sx * (src_anchor[0] - pivot[0]),
            sy * (src_anchor[1] - pivot[1]),
        ];
        let v = [cos * s[0] - sin * s[1], sin * s[0] + cos * s[1]];
        self.set_params(
            sx,
            sy,
            g.rad0,
            anchor[0] - pivot[0] - v[0],
            anchor[1] - pivot[1] - v[1],
        );
    }
}

/// Point-in-convex-quad (the bbox is a transformed rect, always convex):
/// the cross product of every edge must agree on a side.
pub(crate) fn point_in_quad(p: [f32; 2], q: [[f32; 2]; 4]) -> bool {
    let mut sign = 0i32;
    for i in 0..4 {
        let a = q[i];
        let b = q[(i + 1) % 4];
        let cross = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        let s = if cross > 0.0 {
            1
        } else if cross < 0.0 {
            -1
        } else {
            0
        };
        if s != 0 {
            if sign == 0 {
                sign = s;
            } else if sign != s {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drag over a 100x60 source rect at the origin — no tiles needed,
    /// the params and the derived bbox are all this exercises.
    fn drag() -> TransformDrag {
        drag_rect([0, 0, 100, 60])
    }

    fn drag_rect(rect: [i32; 4]) -> TransformDrag {
        let source = mn_core::FloatSource {
            tiles: Default::default(),
            rect,
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
            paste_new_layer: false,
            object_lift: false,
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

    /// The gesture a real press at `p` would build (same fields, same hit
    /// test), so these tests exercise the shipping decision, not a fixture.
    fn press(d: &TransformDrag, p: [f32; 2], alt: bool) -> TransformGesture {
        TransformGesture {
            grab: d.hit_test(p, 1.0, alt),
            start: p,
            bbox0: d.bbox,
            sx0: d.sx,
            sy0: d.sy,
            rad0: d.rad,
            tx0: d.tx,
            ty0: d.ty,
        }
    }

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-2 && (a[1] - b[1]).abs() < 1e-2
    }

    /// The whole hit-test priority list on a 100x60 box at zoom 1 — the one
    /// table both the press and the cursor read.
    #[test]
    fn transform_hit_test_table() {
        let d = drag();
        let h = |p: [f32; 2]| d.hit_test(p, 1.0, false);
        for i in 0..4 {
            assert_eq!(h(d.bbox[i]), TransformGrab::Corner(i), "corner {i}");
        }
        for (i, m) in [[50.0, 0.0], [100.0, 30.0], [50.0, 60.0], [0.0, 30.0]]
            .into_iter()
            .enumerate()
        {
            assert_eq!(h(m), TransformGrab::Edge(i), "edge midpoint {i}");
        }
        assert_eq!(d.stalk_point(1.0), [50.0, -26.0], "26 screen px above");
        assert_eq!(h([50.0, -26.0]), TransformGrab::Rotate, "the stalk");
        assert_eq!(
            h([50.0, -8.0]),
            TransformGrab::Edge(0),
            "just above the top edge is still the edge handle, not the stalk"
        );
        assert_eq!(h([50.0, 30.0]), TransformGrab::Pivot, "reference point");
        assert_eq!(h([30.0, 20.0]), TransformGrab::Move, "inside");
        assert_eq!(h([500.0, 500.0]), TransformGrab::Rotate, "outside");
        assert_eq!(
            d.hit_test([30.0, 20.0], 1.0, true),
            TransformGrab::Pivot,
            "Alt inside places the reference point"
        );
        assert_eq!(
            d.hit_test(d.bbox[0], 1.0, true),
            TransformGrab::Corner(0),
            "Alt on a handle still takes the handle"
        );
    }

    /// The stalk rides the box's own top normal: stand the float on its side
    /// and the lollipop stands with it. A `−y` implementation puts it at
    /// (80, 4) instead — inside the box, unreachable.
    #[test]
    fn transform_stalk_follows_a_standing_rotation() {
        let mut d = drag();
        d.set_params(1.0, 1.0, std::f32::consts::FRAC_PI_2, 0.0, 0.0);
        let s = d.stalk_point(1.0);
        assert!(near(s, [106.0, 30.0]), "stalk off the rotated top: {s:?}");
        assert_eq!(d.hit_test(s, 1.0, false), TransformGrab::Rotate);
        assert_eq!(
            d.hit_test([78.0, 4.0], 1.0, false),
            TransformGrab::Move,
            "straight up the SCREEN is just inside the box now"
        );
    }

    /// Corner slack is capped at a third of the shortest side: on a 12px
    /// float the four discs would otherwise cover the box and nothing but
    /// corners would be reachable.
    #[test]
    fn transform_corner_slack_shrinks_with_the_box() {
        let d = drag_rect([0, 0, 12, 12]);
        for i in 0..4 {
            assert_ne!(
                d.hit_test([6.0, 6.0], 1.0, false),
                TransformGrab::Corner(i),
                "the centre of a 12px box is not corner {i}"
            );
        }
        assert_eq!(d.hit_test([0.0, 0.0], 1.0, false), TransformGrab::Corner(0));
    }

    /// THE owner bug (2026-08-23): dragging a corner of the float ROTATED
    /// it. In CSP a corner scales both axes and never touches the angle —
    /// rotation is the stalk and the outside-the-box drag.
    #[test]
    fn transform_corner_drag_never_changes_rad() {
        for rad0 in [0.0f32, 0.7] {
            let mut d = drag();
            d.set_params(1.0, 1.0, rad0, 0.0, 0.0);
            let start = d.bbox[2];
            let g = press(&d, start, false);
            assert_eq!(g.grab, TransformGrab::Corner(2));
            d.apply_gesture(&g, [start[0] + 130.0, start[1] - 80.0], false, false, false);
            assert!(
                (d.rad - rad0).abs() < 1e-6,
                "corner scaled to rad {} from {rad0}",
                d.rad
            );
        }
    }

    /// CSP's anchor: the corner opposite the one you grabbed holds still,
    /// under a standing rotation too. (Before the fix it ran away — the
    /// scale was taken about the reference point.)
    #[test]
    fn transform_corner_pins_the_opposite_corner() {
        for rad0 in [0.0f32, 0.7] {
            let mut d = drag();
            d.set_params(1.0, 1.0, rad0, 0.0, 0.0);
            let (start, opp) = (d.bbox[2], d.bbox[0]);
            let g = press(&d, start, false);
            d.apply_gesture(&g, [start[0] + 40.0, start[1] + 25.0], false, false, false);
            assert!(
                near(d.bbox[0], opp),
                "opposite corner moved: {:?}",
                d.bbox[0]
            );
        }
    }

    /// …and the grabbed corner sits exactly under the pointer (aspect off).
    #[test]
    fn transform_corner_tracks_the_pointer() {
        for rad0 in [0.0f32, 0.7] {
            let mut d = drag();
            d.set_params(1.0, 1.0, rad0, 0.0, 0.0);
            let start = d.bbox[2];
            let to = [start[0] + 40.0, start[1] + 25.0];
            let g = press(&d, start, false);
            d.apply_gesture(&g, to, false, false, false);
            assert!(
                near(d.bbox[2], to),
                "corner off the pointer: {:?}",
                d.bbox[2]
            );
        }
    }

    /// TR-004 with the CSP anchor: a side handle scales one axis and pins
    /// the OPPOSITE side (not the reference point).
    #[test]
    fn transform_edge_pins_the_opposite_edge() {
        for rad0 in [0.0f32, 0.7] {
            let mut d = drag();
            d.set_params(1.0, 1.0, rad0, 0.0, 0.0);
            let mid = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
            let start = mid(d.bbox[1], d.bbox[2]);
            let opp = mid(d.bbox[3], d.bbox[0]);
            let g = press(&d, start, false);
            assert_eq!(g.grab, TransformGrab::Edge(1));
            d.apply_gesture(&g, [start[0] + 50.0, start[1] + 10.0], false, false, false);
            assert!(near(mid(d.bbox[3], d.bbox[0]), opp), "left edge moved");
            assert!((d.sy - 1.0).abs() < 1e-6, "one axis only: sy {}", d.sy);
            assert!(d.sx > 1.0, "and the other one grew: sx {}", d.sx);
            assert!((d.rad - rad0).abs() < 1e-6, "no rotation");
        }
    }

    /// Shift (for one drag) and the Keep-aspect setting (persistently) both
    /// tie the two axes to a single ratio.
    #[test]
    fn transform_shift_or_keep_aspect_locks_the_ratio() {
        for (shift, keep) in [(true, false), (false, true)] {
            let mut d = drag();
            let start = d.bbox[2];
            let g = press(&d, start, false);
            // A deliberately lopsided pull: free scaling would split.
            d.apply_gesture(&g, [start[0] + 100.0, start[1] + 2.0], shift, false, keep);
            assert!(
                (d.sx - d.sy).abs() < 1e-4,
                "shift={shift} keep={keep}: sx {} sy {}",
                d.sx,
                d.sy
            );
        }
    }

    /// Alt is CSP's other mode: scale about the reference point, which stays
    /// put while BOTH sides of the box move.
    #[test]
    fn transform_alt_scales_about_the_reference_point() {
        let mut d = drag();
        let pivot = d.pivot();
        let start = d.bbox[2];
        let opp = d.bbox[0];
        let g = press(&d, start, false);
        d.apply_gesture(&g, [start[0] + 50.0, start[1] + 30.0], false, true, false);
        assert!(near(d.xform.apply(pivot), pivot), "the pivot held still");
        assert!(!near(d.bbox[0], opp), "the opposite corner moved too");
        assert!(
            (d.tx, d.ty) == (0.0, 0.0),
            "Alt keeps the press translation"
        );
    }

    /// Shift snaps a rotation drag to 45° steps (a 40° pull lands on 45).
    #[test]
    fn transform_shift_snaps_rotation_to_45() {
        let mut d = drag();
        let c = d.pivot();
        let start = [c[0] + 100.0, c[1]];
        let g = press(&d, start, false);
        assert_eq!(g.grab, TransformGrab::Rotate);
        let a = 40.0f32.to_radians();
        d.apply_gesture(
            &g,
            [c[0] + 100.0 * a.cos(), c[1] + 100.0 * a.sin()],
            true,
            false,
            false,
        );
        assert!(
            (d.rad - std::f32::consts::FRAC_PI_4).abs() < 1e-4,
            "snapped to {}°",
            d.rad.to_degrees()
        );
    }

    /// Drag a corner past its anchor and the float MIRRORS (CSP does): the
    /// scale goes negative instead of pinning at the clamp, and nothing
    /// becomes NaN on the way through zero.
    #[test]
    fn transform_corner_drag_through_zero_flips() {
        let mut d = drag();
        let start = d.bbox[2];
        let g = press(&d, start, false);
        // Exactly on the anchor: degenerate, must stay finite.
        d.apply_gesture(&g, [0.0, 0.0], false, false, false);
        assert!(d.sx.is_finite() && d.sy.is_finite() && d.bbox[2][0].is_finite());
        // Through and out the far side.
        d.apply_gesture(&g, [-50.0, -30.0], false, false, false);
        assert!(d.sx < 0.0 && d.sy < 0.0, "mirrored: {} {}", d.sx, d.sy);
        assert!(near(d.bbox[2], [-50.0, -30.0]), "still under the pointer");
        assert!(near(d.bbox[0], [0.0, 0.0]), "still anchored");
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
