//! egui shell: Win32 -> egui input translation, frame running, wgpu painting.
//!
//! There is deliberately **no `egui-winit`** here (docs/ARCHITECTURE.md: we own
//! the message loop), so this file is the whole platform layer: mouse, wheel,
//! keys, text, cursors, DPI. Clipboard and IME are not wired yet.
//!
//! # Pointer routing (the rule that matters)
//!
//! The canvas is whatever rectangle the panels leave free. `owns_pointer()`
//! answers "does egui get this event" from the *position alone*:
//!   1. `Context::layer_id_at` reports a non-background layer (a floating
//!      window, an open menu, a combo popup) — egui's, or
//!   2. the point is outside the free rect, i.e. a panel is there — egui's,
//!   3. otherwise the canvas gets it.
//! Nothing depends on egui having seen a hover first, which matters because a
//! pen goes from nowhere to in-contact in one message: tapping a menu item or
//! a panel button with the pen must not paint a dot through the UI.

use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, ViewportId};
use mn_gpu::Renderer;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};

pub struct Shell {
    /// Cloneable handle (egui `Context` is an `Arc`), so the UI closure can
    /// borrow `&mut App` while the context drives the pass.
    pub ctx: egui::Context,
    painter: egui_wgpu::Renderer,

    events: Vec<Event>,
    modifiers: Modifiers,
    start: Instant,
    /// Physical pixels per logical point, from the window DPI.
    pub ppp: f32,
    focused: bool,
    /// Region the panels left for the canvas, in **points**, last frame.
    canvas_rect_pts: Rect,
    /// UI drawn INSIDE the canvas rect on the background layer (the
    /// selection launcher bar): egui owns the pointer there — pen taps must
    /// not paint through it, and the cursor is egui's arrow, not the canvas
    /// crosshair. Rebuilt every frame by whoever draws such a widget.
    ui_islands: Vec<Rect>,
    wants_keyboard: bool,
    pub cursor: egui::CursorIcon,
    /// UTF-16 high surrogate waiting for its pair (WM_CHAR arrives in halves).
    pending_surrogate: Option<u16>,
    /// Pinned modifier state for tests: when set, `sync_modifiers`
    /// reports THIS instead of reading the physical keyboard, so a human
    /// typing during a test run cannot flake chord matching. Production
    /// never sets it. See `App::new` for the harness-wide "none" pin.
    pub(crate) test_modifiers: Option<Modifiers>,
}

impl Shell {
    pub fn new(renderer: &Renderer, ppp: f32) -> Self {
        let ctx = egui::Context::default();
        crate::ui::theme::apply(&ctx);

        // Japanese glyphs: egui's bundled fonts have none, and brush and layer
        // names here are often Japanese (インク切れ筆ペン renders as tofu
        // otherwise). Meiryo ships with every Windows since Vista.
        let mut fonts = egui::FontDefinitions::default();
        for cand in [
            r"C:\Windows\Fonts\meiryo.ttc",
            r"C:\Windows\Fonts\YuGothM.ttc",
            r"C:\Windows\Fonts\msgothic.ttc",
        ] {
            if let Ok(bytes) = std::fs::read(cand) {
                fonts
                    .font_data
                    .insert("jp".into(), egui::FontData::from_owned(bytes).into());
                for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                    if let Some(list) = fonts.families.get_mut(&fam) {
                        list.push("jp".into());
                    }
                }
                break;
            }
        }
        ctx.set_fonts(fonts);

        let painter = egui_wgpu::Renderer::new(
            renderer.device(),
            renderer.output_format(),
            egui_wgpu::RendererOptions::default(),
        );

        Self {
            ctx,
            painter,
            events: Vec::new(),
            modifiers: Modifiers::default(),
            start: Instant::now(),
            ppp: if ppp.is_finite() && ppp > 0.0 {
                ppp
            } else {
                1.0
            },
            focused: true,
            canvas_rect_pts: Rect::EVERYTHING,
            ui_islands: Vec::new(),
            wants_keyboard: false,
            cursor: egui::CursorIcon::Default,
            test_modifiers: None,
            pending_surrogate: None,
        }
    }

    // --- routing ---------------------------------------------------------

    /// Does egui get this pointer event instead of the canvas? Client pixels in.
    pub fn owns_pointer(&self, x: i32, y: i32) -> bool {
        let pos = self.pt(x, y);
        match self.ctx.layer_id_at(pos) {
            // A window / menu / popup / tooltip is under the point.
            Some(layer) if layer.order != egui::Order::Background => true,
            // Background layer: panels are egui's, the free rect is the
            // canvas — except registered UI islands drawn over it (the
            // selection launcher), which are egui's too.
            _ => {
                !self.canvas_rect_pts.contains(pos)
                    || self.ui_islands.iter().any(|r| r.contains(pos))
            }
        }
    }

    /// Declare a background-layer widget drawn INSIDE the canvas rect this
    /// frame (see `ui_islands`). Cleared at every `begin`.
    pub fn add_ui_island(&mut self, r: Rect) {
        self.ui_islands.push(r);
    }

    /// True while a text field has focus — app shortcuts must stand down.
    pub fn wants_keyboard(&self) -> bool {
        self.wants_keyboard
    }

    pub fn set_canvas_rect_points(&mut self, r: Rect) {
        self.canvas_rect_pts = r;
    }

    /// The canvas area in points, as last reported — what the chrome
    /// underlay paints AROUND (ui.rs).
    pub fn canvas_rect_points(&self) -> Rect {
        self.canvas_rect_pts
    }

    /// The canvas area in client pixels (what zoom/rotate commands anchor on).
    pub fn canvas_rect_px(&self) -> Rect {
        let r = self.canvas_rect_pts;
        Rect::from_min_max(
            egui::pos2(r.min.x * self.ppp, r.min.y * self.ppp),
            egui::pos2(r.max.x * self.ppp, r.max.y * self.ppp),
        )
    }

    pub fn set_ppp(&mut self, ppp: f32) {
        if ppp.is_finite() && ppp > 0.0 {
            self.ppp = ppp;
        }
    }

    // --- input -----------------------------------------------------------

    #[inline]
    fn pt(&self, x: i32, y: i32) -> Pos2 {
        egui::pos2(x as f32 / self.ppp, y as f32 / self.ppp)
    }

    /// Re-read the modifier keys and emit an event if they moved. Cheap enough
    /// to call on every input message; `GetKeyState` is a thread-local read.
    /// A pinned `test_modifiers` short-circuits the physical read entirely.
    pub fn sync_modifiers(&mut self) -> Modifiers {
        let m = match self.test_modifiers {
            Some(pinned) => pinned,
            None => {
                let down = |vk: i32| unsafe { GetKeyState(vk) } < 0;
                Modifiers {
                    alt: down(VK_MENU as i32),
                    ctrl: down(VK_CONTROL as i32),
                    shift: down(VK_SHIFT as i32),
                    mac_cmd: false,
                    command: down(VK_CONTROL as i32),
                }
            }
        };
        if m != self.modifiers {
            self.modifiers = m;
            self.events.push(Event::ModifiersChanged(m));
        }
        m
    }

    pub fn on_pointer_moved(&mut self, x: i32, y: i32) {
        let p = self.pt(x, y);
        self.events.push(Event::PointerMoved(p));
    }

    pub fn on_pointer_button(&mut self, x: i32, y: i32, button: PointerButton, pressed: bool) {
        let modifiers = self.sync_modifiers();
        let pos = self.pt(x, y);
        // egui needs to know where the pointer is before it is told it was
        // clicked; a pen goes from nowhere to down in one message.
        self.events.push(Event::PointerMoved(pos));
        self.events.push(Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers,
        });
    }

    pub fn on_pointer_gone(&mut self) {
        self.events.push(Event::PointerGone);
    }

    /// Wheel notches (a WHEEL_DELTA unit is one "line" step).
    pub fn on_wheel(&mut self, dx: f32, dy: f32) {
        let modifiers = self.sync_modifiers();
        self.events.push(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(dx, dy),
            phase: egui::TouchPhase::Move,
            modifiers,
        });
    }

    pub fn on_key(&mut self, vk: u16, pressed: bool, repeat: bool) {
        let modifiers = self.sync_modifiers();
        if let Some(key) = vk_to_key(vk) {
            self.events.push(Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat,
                modifiers,
            });
        }
    }

    /// One UTF-16 unit from `WM_CHAR`. Control characters are dropped (egui
    /// gets those as `Event::Key`), surrogate pairs are joined.
    pub fn on_char(&mut self, unit: u16) {
        let scalar = match (self.pending_surrogate.take(), unit) {
            (_, 0xD800..=0xDBFF) => {
                self.pending_surrogate = Some(unit);
                return;
            }
            (Some(hi), 0xDC00..=0xDFFF) => {
                let c = 0x1_0000 + (((hi as u32 - 0xD800) << 10) | (unit as u32 - 0xDC00));
                char::from_u32(c)
            }
            (_, u) => char::from_u32(u as u32),
        };
        let Some(c) = scalar else { return };
        if c.is_control() {
            return;
        }
        // Ctrl+<letter> arrives as WM_CHAR 0x01..0x1A (filtered above) but
        // Ctrl+Alt combos can produce real text — let those through.
        if self.modifiers.ctrl && !self.modifiers.alt {
            return;
        }
        self.events.push(Event::Text(c.to_string()));
    }

    pub fn on_focus(&mut self, focused: bool) {
        self.focused = focused;
        self.events.push(Event::WindowFocused(focused));
        if !focused {
            self.events.push(Event::PointerGone);
        }
    }

    // --- frame -----------------------------------------------------------

    pub fn begin(&mut self, size_px: (u32, u32)) -> RawInput {
        self.ui_islands.clear();
        let mut viewports = egui::ViewportIdMap::default();
        viewports.insert(
            ViewportId::ROOT,
            egui::ViewportInfo {
                native_pixels_per_point: Some(self.ppp),
                focused: Some(self.focused),
                ..Default::default()
            },
        );
        RawInput {
            viewport_id: ViewportId::ROOT,
            viewports,
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(size_px.0 as f32 / self.ppp, size_px.1 as f32 / self.ppp),
            )),
            time: Some(self.start.elapsed().as_secs_f64()),
            events: std::mem::take(&mut self.events),
            focused: self.focused,
            ..Default::default()
        }
    }

    /// Record what the pass decided: cursor, routing flags, repaint request.
    pub fn end(&mut self, out: &egui::FullOutput) -> Duration {
        self.cursor = out.platform_output.cursor_icon;
        self.wants_keyboard = self.ctx.egui_wants_keyboard_input();
        out.viewport_output
            .get(&ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .unwrap_or(Duration::MAX)
    }

    /// Record the egui pass into `encoder`, on top of the already-drawn canvas.
    /// Runs inside `Renderer::render_with_overlay`, so the encoder is submitted
    /// by the compositor right after this returns.
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size_px: (u32, u32),
        jobs: &[egui::ClippedPrimitive],
        textures: &mut egui::TexturesDelta,
    ) {
        let desc = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size_px.0, size_px.1],
            pixels_per_point: self.ppp,
        };

        for (id, deltas) in textures.set.drain() {
            for d in deltas {
                self.painter.update_texture(device, queue, id, &d);
            }
        }

        let user_bufs = self
            .painter
            .update_buffers(device, queue, encoder, jobs, &desc);
        if !user_bufs.is_empty() {
            // Must land before our encoder, which the compositor submits next.
            queue.submit(user_bufs);
        }

        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mn.egui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // The canvas is already in this texture — draw over it.
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.painter
            .render(&mut pass.forget_lifetime(), jobs, &desc);
    }

    /// Free textures egui dropped. Must run **after** the frame is submitted.
    pub fn free(&mut self, textures: &mut egui::TexturesDelta) {
        for id in textures.free.drain() {
            self.painter.free_texture(&id);
        }
    }
}

/// Virtual-key code -> egui key. Values are winuser.h literals; only the keys
/// the shell actually reacts to (plus text-editing keys egui needs) are mapped.
fn vk_to_key(vk: u16) -> Option<Key> {
    match vk {
        0x41..=0x5A => Key::from_name(&((b'A' + (vk - 0x41) as u8) as char).to_string()), // A..Z
        0x30..=0x39 => Key::from_name(&((b'0' + (vk - 0x30) as u8) as char).to_string()), // 0..9
        0x70..=0x87 => Key::from_name(&format!("F{}", vk - 0x6F)), // VK_F1..VK_F24
        0x08 => Some(Key::Backspace),
        0x09 => Some(Key::Tab),
        0x0D => Some(Key::Enter),
        0x1B => Some(Key::Escape),
        0x20 => Some(Key::Space),
        0x21 => Some(Key::PageUp),
        0x22 => Some(Key::PageDown),
        0x23 => Some(Key::End),
        0x24 => Some(Key::Home),
        0x25 => Some(Key::ArrowLeft),
        0x26 => Some(Key::ArrowUp),
        0x27 => Some(Key::ArrowRight),
        0x28 => Some(Key::ArrowDown),
        0x2D => Some(Key::Insert),
        0x2E => Some(Key::Delete),
        0xBB => Some(Key::Equals),       // VK_OEM_PLUS
        0xBC => Some(Key::Comma),        // VK_OEM_COMMA
        0xBD => Some(Key::Minus),        // VK_OEM_MINUS
        0xBE => Some(Key::Period),       // VK_OEM_PERIOD
        0xDB => Some(Key::OpenBracket),  // VK_OEM_4
        0xDD => Some(Key::CloseBracket), // VK_OEM_6
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_function_keys_map() {
        assert_eq!(vk_to_key(0x5A), Some(Key::Z));
        assert_eq!(vk_to_key(0x42), Some(Key::B));
        assert_eq!(vk_to_key(0x31), Some(Key::Num1));
        assert_eq!(vk_to_key(0x70), Some(Key::F1));
        assert_eq!(vk_to_key(0x7B), Some(Key::F12));
        assert_eq!(vk_to_key(0xDB), Some(Key::OpenBracket));
        assert_eq!(vk_to_key(0x01), None); // VK_LBUTTON is not a key
    }
}
