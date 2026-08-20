//! Viewport navigation: pan, the Move▸Rotate sub-tool drag, zoom, fit.
//! `fitted_viewport` (the fit computation) stays private in `app.rs`.

use super::{App, fitted_viewport};

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
            let flipped = self.viewport.flip_h;
            self.viewport = fitted_viewport(&self.doc, client, self.prefs.fit_margin);
            if flipped {
                self.viewport.flip_h = true;
                // pan is the top-left's screen spot; mirrored, the page runs
                // left from it, so shift by the page width to recentre.
                self.viewport.pan[0] += self.doc.size.0 as f32 * self.viewport.zoom;
            }
        }
        self.needs_redraw = true;
    }

    // --- navigation ------------------------------------------------------

    pub fn begin_pan(&mut self, client_x: f32, client_y: f32) {
        self.pan_drag = Some(([client_x, client_y], self.viewport.pan));
    }

    // --- rotate drag (Move ▸ Rotate sub tool) -------------------------------

    fn pointer_angle(&self, x: f32, y: f32) -> f32 {
        let c = self.canvas_center();
        (y - c[1]).atan2(x - c[0])
    }

    pub fn begin_rotate(&mut self, x: f32, y: f32) {
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
