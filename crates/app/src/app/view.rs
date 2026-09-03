//! Viewport navigation: pan, the Move▸Rotate sub-tool drag, zoom, fit.
//! `fitted_viewport` (the fit computation) stays private in `app.rs`.

use mn_gpu::Viewport;

use super::{App, fitted_viewport};

/// Exact cache key for a viewport (`Viewport` has no `PartialEq`, and float
/// bits are the right comparison here anyway: the texture is valid for
/// EXACTLY the numbers it was rendered from).
/// The second view's fit. Deliberately NOT `fitted_viewport`: that one
/// floors the zoom at 0.05 because it sizes the DRAWING surface, and a
/// 64pt-wide overview pane of a 2048px page wants 0.03 — the floor cropped
/// the page in exactly the pane the feature exists for. Everything else
/// (the margin preference, centring) is the same.
fn pane_fit(doc_size: (u32, u32), size_px: (u32, u32), margin: f32) -> Viewport {
    let (dw, dh) = (doc_size.0.max(1) as f32, doc_size.1.max(1) as f32);
    let (pw, ph) = (size_px.0.max(1) as f32, size_px.1.max(1) as f32);
    let zoom = ((pw / dw).min(ph / dh) * margin).clamp(1e-4, 8.0);
    Viewport {
        pan: [(pw - dw * zoom) * 0.5, (ph - dh * zoom) * 0.5],
        zoom,
        rotate_rad: 0.0,
        flip_h: false,
        flip_v: false,
    }
}

/// CV-032: the zoom rungs the View menu's Zoom In / Zoom Out keys walk,
/// as scale factors. CSP's own scale list, which is also what
/// Preferences ▸ Canvas ▸ Scale edits there — round numbers a page can be
/// judged at, close together where the work happens (50–200 %) and coarse
/// at the ends where a step is a jump anyway.
///
/// The wheel deliberately does NOT use it: a wheel notch is a continuum
/// (`Prefs::wheel_step`), a keypress is a rung.
pub const ZOOM_LADDER: &[f32] = &[
    0.02, 0.03, 0.04, 0.06, 0.08, 0.125, 0.16, 0.25, 0.33, 0.50, 0.66, 1.0, 1.5, 2.0, 3.0, 4.0,
    6.0, 8.0, 12.0, 16.0, 24.0, 32.0, 64.0,
];

/// The next rung strictly above (`up`) or below the current zoom. The
/// strictness is what makes a repeated press keep moving from a zoom that
/// is already ON a rung; the small epsilon keeps a float-fuzzy 1.0 from
/// counting as "below 1.0" and standing still.
pub fn zoom_ladder_next(zoom: f32, up: bool) -> f32 {
    let eps = 1.0e-3;
    if up {
        ZOOM_LADDER
            .iter()
            .copied()
            .find(|z| *z > zoom * (1.0 + eps))
            .unwrap_or_else(|| ZOOM_LADDER[ZOOM_LADDER.len() - 1])
    } else {
        ZOOM_LADDER
            .iter()
            .copied()
            .rev()
            .find(|z| *z < zoom * (1.0 - eps))
            .unwrap_or(ZOOM_LADDER[0])
    }
}

fn view_key(vp: &Viewport) -> [u32; 5] {
    [
        vp.pan[0].to_bits(),
        vp.pan[1].to_bits(),
        vp.zoom.to_bits(),
        vp.rotate_rad.to_bits(),
        u32::from(vp.flip_h) | (u32::from(vp.flip_v) << 1),
    ]
}

impl App {
    /// Centre of the canvas area (the rect the panels leave free), client px —
    /// the anchor for toolbar zoom/rotate commands.
    pub fn canvas_center(&self) -> [f32; 2] {
        let r = self.shell.canvas_rect_px();
        if r.is_finite() && r.width() > 0.0 {
            [r.center().x, r.center().y]
        } else {
            let (w, h) = self.renderer.surface_size();
            [w as f32 * 0.5, h as f32 * 0.5]
        }
    }

    /// Fit the page on screen with a little margin, centred. Keeps the view
    /// mirror (a flipped view stays flipped through a fit, CSP-style).
    /// Fit the page into the area you can actually SEE.
    ///
    /// The canvas quad is drawn across the WHOLE window with the palette
    /// columns painted on top, so fitting to the window centred the page
    /// behind them: part of it sat under a palette and the part you could
    /// see was smaller than the free space it had. Owner, 2026-08-19: a
    /// canvas "should open just short of being as big as it can be,
    /// regardless of if it's 500x500 or 4000x4000".
    ///
    /// Falls back to the whole surface when the canvas rect is not known yet
    /// — it is `Rect::EVERYTHING` until the first frame lays the UI out, and
    /// a headless renderer has no surface at all.
    pub fn fit_to_view(&mut self) {
        let r = self.shell.canvas_rect_px();
        let (w, h) = (r.width(), r.height());
        if w.is_finite() && h.is_finite() && w >= 1.0 && h >= 1.0 {
            self.viewport = super::fitted_viewport_in(
                &self.doc,
                [r.min.x, r.min.y],
                (w, h),
                self.prefs.fit_margin,
            );
            self.needs_redraw = true;
            return;
        }
        let client = self.renderer.surface_size();
        self.fit_to_view_sized(client);
    }

    /// The fit against an explicit surface size (the sticky-fit seam — a
    /// headless renderer reports 0x0 and must never fit).
    pub fn fit_to_view_sized(&mut self, client: (u32, u32)) {
        if client.0 > 0 && client.1 > 0 {
            let (flip_h, flip_v) = (self.viewport.flip_h, self.viewport.flip_v);
            self.viewport = fitted_viewport(&self.doc, client, self.prefs.fit_margin);
            if flip_h {
                self.viewport.flip_h = true;
                // pan is the top-left's screen spot; mirrored, the page runs
                // left from it, so shift by the page width to recentre.
                self.viewport.pan[0] += self.doc.size.0 as f32 * self.viewport.zoom;
            }
            if flip_v {
                // Same one axis over: flipped, the page runs UP from pan.
                self.viewport.flip_v = true;
                self.viewport.pan[1] += self.doc.size.1 as f32 * self.viewport.zoom;
            }
        }
        self.needs_redraw = true;
    }

    // --- navigation ------------------------------------------------------

    pub fn begin_pan(&mut self, client_x: f32, client_y: f32) {
        // See `begin_stroke`: input while a tool key is held marks the
        // spring-loaded borrow as used.
        if let Some(s) = &mut self.spring {
            s.pointer_seen = true;
        }
        self.pan_drag = Some(([client_x, client_y], self.viewport.pan));
    }

    // --- rotate drag (Move ▸ Rotate sub tool) -------------------------------

    fn pointer_angle(&self, x: f32, y: f32) -> f32 {
        let c = self.canvas_center();
        (y - c[1]).atan2(x - c[0])
    }

    pub fn begin_rotate(&mut self, x: f32, y: f32) {
        // See `begin_stroke`: the borrow is being used.
        if let Some(s) = &mut self.spring {
            s.pointer_seen = true;
        }
        self.rotate_drag = Some(self.pointer_angle(x, y));
    }

    pub fn rotating(&self) -> bool {
        self.rotate_drag.is_some()
    }

    pub fn update_rotate(&mut self, x: f32, y: f32) {
        let Some(last) = self.rotate_drag else { return };
        let now = self.pointer_angle(x, y);
        let c = self.canvas_center();
        self.viewport.rotate_around(c, now - last);
        self.rotate_drag = Some(now);
        self.needs_redraw = true;
    }

    pub fn end_rotate(&mut self) {
        self.rotate_drag = None;
    }

    pub fn update_pan(&mut self, client_x: f32, client_y: f32) {
        if let Some((anchor, pan0)) = self.pan_drag {
            self.viewport.pan = [
                pan0[0] + (client_x - anchor[0]),
                pan0[1] + (client_y - anchor[1]),
            ];
            self.needs_redraw = true;
        }
    }

    pub fn end_pan(&mut self) {
        self.pan_drag = None;
    }

    pub fn nudge_pan(&mut self, dx: f32, dy: f32) {
        self.viewport.pan[0] += dx;
        self.viewport.pan[1] += dy;
        self.needs_redraw = true;
    }

    /// CV-032: one rung along [`ZOOM_LADDER`], anchored on the canvas-area
    /// centre, with the scale it landed on said out loud — the number is
    /// the whole point of a ladder step.
    pub fn zoom_ladder_step(&mut self, up: bool) {
        let want = zoom_ladder_next(self.viewport.zoom, up);
        let c = self.canvas_center();
        self.viewport.set_zoom_around(c, want);
        self.set_status(format!("zoom {}%", (self.viewport.zoom * 100.0).round()));
        self.needs_redraw = true;
    }

    /// Zoom keeping the canvas point under the cursor fixed.
    pub fn zoom_at(&mut self, client_x: f32, client_y: f32, factor: f32) {
        self.viewport.zoom_around([client_x, client_y], factor);
        self.needs_redraw = true;
    }
}

// --- Navigator palette (CV-030/031/036) ----------------------------------

impl App {
    /// The Navigator thumbnail, re-rendered only when the document
    /// revision moves (the view rect and readouts are per-frame painter
    /// work on top — the thumbnail itself is view-independent).
    pub fn navigator_thumb(&mut self) -> egui::TextureHandle {
        if self.nav_thumb.is_none() || self.nav_thumb_rev != self.doc.revision {
            let img = self.renderer.render_offscreen(&self.doc, 176, 176);
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            let tex = self
                .shell
                .ctx
                .load_texture("mn.navigator", ci, egui::TextureOptions::LINEAR);
            self.nav_thumb_rev = self.doc.revision;
            self.nav_thumb = Some(tex);
        }
        self.nav_thumb.clone().expect("just stored")
    }

    /// Drag-to-pan from the thumbnail: centre the view on a CANVAS point.
    pub fn navigator_pan_to(&mut self, cx: f32, cy: f32) {
        let c = self.canvas_center();
        let s = self.viewport.to_screen(cx, cy);
        self.viewport.pan[0] += c[0] - s.0;
        self.viewport.pan[1] += c[1] - s.1;
        self.needs_redraw = true;
    }

    /// CV-021's "New Window", as a pane: a SECOND live view of the page
    /// being drawn, with its own zoom and pan.
    ///
    /// Deliberately view-only. Rendering two live GPU viewports is out by
    /// design (docs/DOCKING-2.md: one live drawing surface, parked pages
    /// are bytes, the target machine has an iGPU) and `Shell::owns_pointer`
    /// routes the pen by ONE canvas rect, so the second view composites
    /// offscreen through its own viewport and shows the result — exactly
    /// the mechanism the Navigator thumbnail already runs on, one size up
    /// and steerable. It therefore updates when the document revision
    /// moves, i.e. at every stroke end, not mid-stroke.
    ///
    /// The long-edge cap keeps a dragged-huge pane from minting
    /// page-sized textures per stroke (the Pages palette's clamp, same
    /// reasoning).
    pub const VIEW_PANE_MAX_PX: f32 = 1200.0;

    /// The viewport the second view is showing `size_px` through. Untouched
    /// (`view_pane_vp` = `None`) it simply fits the page into the pane, so
    /// resizing or re-docking the pane needs no bookkeeping at all.
    pub fn view_pane_viewport(&self, size_px: (u32, u32)) -> Viewport {
        self.view_pane_vp
            .unwrap_or_else(|| pane_fit(self.doc.size, size_px, self.prefs.fit_margin))
    }

    /// Put the second view back to "the whole page" — the fit is recomputed
    /// from the pane's size on every frame after this, so it also un-sticks
    /// a view the user had panned away.
    pub fn view_pane_fit(&mut self) {
        self.view_pane_vp = None;
    }

    /// Zoom the second view by `factor`, keeping the page point under
    /// `anchor` (target pixels, pane-local) where it is.
    pub fn view_pane_zoom(&mut self, size_px: (u32, u32), anchor: [f32; 2], factor: f32) {
        let mut vp = self.view_pane_viewport(size_px);
        // The clamp is the pane's own: below the fit there is nothing left
        // to see, and past 8x a second view stops being an overview.
        let fit = pane_fit(self.doc.size, size_px, self.prefs.fit_margin).zoom;
        let want = (vp.zoom * factor).clamp(fit * 0.5, 8.0);
        if vp.zoom > 0.0 {
            vp.zoom_around(anchor, want / vp.zoom);
        }
        self.view_pane_vp = Some(vp);
    }

    /// Pan the second view by a target-pixel delta.
    pub fn view_pane_pan(&mut self, size_px: (u32, u32), dx: f32, dy: f32) {
        let mut vp = self.view_pane_viewport(size_px);
        vp.pan[0] += dx;
        vp.pan[1] += dy;
        self.view_pane_vp = Some(vp);
    }

    /// The second view's texture: one offscreen composite of the LIVE
    /// document through the pane's own viewport, cached against everything
    /// it was rendered from (revision, size, viewport). Same door the
    /// `--screenshot` harness uses, so what the pane shows is what the
    /// canvas would show at that viewport.
    pub fn view_pane_texture(&mut self, size_px: (u32, u32)) -> egui::TextureHandle {
        let (w, h) = (size_px.0.max(1), size_px.1.max(1));
        let vp = self.view_pane_viewport((w, h));
        let key = (self.doc.revision, w, h, view_key(&vp));
        if self.view_pane_tex.is_none() || self.view_pane_key != Some(key) {
            let img = self.renderer.render_offscreen_vp(&self.doc, &vp, w, h);
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            // One handle exists at a time (the pane is deduped to one), so
            // a fixed name cannot alias a second live texture the way page
            // thumbnails could.
            let tex = self
                .shell
                .ctx
                .load_texture("mn.viewpane", ci, egui::TextureOptions::LINEAR);
            self.view_pane_key = Some(key);
            self.view_pane_tex = Some(tex);
        }
        self.view_pane_tex.clone().expect("just stored")
    }

    /// CV-036 sticky fit: while the toggle is on, a surface-size change
    /// re-fits the page. Checked from the Navigator body each frame (the
    /// toggle lives in the palette, so the behavior does too).
    pub fn navigator_sticky_fit_check(&mut self) {
        let sz = self.renderer.surface_size();
        self.navigator_sticky_fit_apply(sz);
    }

    /// The sticky-fit logic against an explicit size (the test seam — a
    // headless renderer reports a 0x0 surface, which must never refit).
    pub fn navigator_sticky_fit_apply(&mut self, sz: (u32, u32)) {
        if self.fit_sticky && sz != self.nav_last_surface && sz.0 > 0 && sz.1 > 0 {
            self.fit_to_view_sized(sz);
        }
        self.nav_last_surface = sz;
    }
}
