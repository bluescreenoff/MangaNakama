//! Touch TAP gestures — the pure state machine (CSP `GS-001`, `GS-002`,
//! `GS-013`; docs/CSP-SURFACE.md 750_gestures).
//!
//! Two fingers tapped = undo, three = redo, three on the Navigator = the
//! canvas back upright and unmirrored in one gesture. The actions are the
//! easy half. The hard half — and the whole reason this file exists apart
//! from the Win32 side — is telling a TAP from the first two events of a
//! pan/pinch, and telling a deliberate two-finger tap from a PALM: a hand
//! resting on the glass is also "two contacts", and a false undo silently
//! eats the stroke the user just drew. CSP's answer is a switch per gesture
//! (`GS-008`) because the discrimination is never perfect; ours are the
//! same, and default to OFF.
//!
//! Nothing here knows about Win32 or egui. Events in are `(pointer id,
//! position, timestamp, contact size)` plus up/down, the answer out is an
//! [`Action`] — so every rejection rule below is testable without a window
//! and without a touchscreen.
//!
//! The shape of the rule set: a *cycle* opens when the first contact lands
//! on an empty surface and resolves when the last one lifts. It fires only
//! if every contact landed together, stayed still, was fingertip-sized, and
//! the whole thing was over quickly. Anything else spoils the cycle, and a
//! spoiled cycle is silent — it never falls back to a smaller gesture.

/// Enable bits, persisted as one `touch_gestures=` integer in `ui.txt`
/// (`app/layout.rs`). Add them: 3 = both taps, 7 = everything.
pub const UNDO: u8 = 1 << 0;
pub const REDO: u8 = 1 << 1;
pub const RESET_VIEW: u8 = 1 << 2;

/// A tap is SHORT — first contact down to last contact up. Both mobile
/// platforms and Windows' own press-and-hold call 500 ms a long press, and
/// nothing about a tap should be near that line; 300 ms leaves room for a
/// lazy tap while a rested hand (which is *seconds*) is never close.
pub const TAP_MS: f64 = 300.0;

/// A multi-finger tap lands TOGETHER. Real fingers touch down a few tens of
/// ms apart (pointer messages themselves arrive every 8-16 ms), so 100 ms is
/// generous for a hand; it is the palm-first case this rejects — a palm
/// settling and *then* a finger arriving reads as a much longer gap.
pub const SPREAD_MS: f64 = 100.0;

/// A tap is STILL — travel from each contact's own landing point, in logical
/// px. Windows' mouse drag threshold (`SM_CXDRAG`) is 4 px and Android's
/// touch slop is 8 dp; a fingertip wobbles more than a mouse, and a real pan
/// is past 12 px within a couple of events, so this separates the two
/// without demanding a surgeon's hand.
pub const SLOP_PX: f32 = 12.0;

/// Longest side of a contact patch that can still be a fingertip, logical px
/// — about 20 mm at 96 dpi. A fingertip reports 8-12 mm, a palm heel 40 mm
/// and up. Digitizers that do not report a contact area report 0, and 0 is
/// read as "unknown", never as "tiny": the size rule can only ever reject,
/// never be the thing that lets a gesture through.
pub const PALM_PX: f32 = 75.0;

/// Nothing is bound above three fingers, and a fourth contact means a hand
/// went down, not a gesture.
const MAX_CONTACTS: usize = 3;

/// What a resolved tap asks the app to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Two-finger tap (`GS-001`).
    Undo,
    /// Three-finger tap away from the Navigator (`GS-002`).
    Redo,
    /// Three-finger tap on the Navigator (`GS-013`): rotation back to zero
    /// and the mirror off — the escape hatch from a canvas knocked askew by
    /// a stray two-finger twist.
    ResetView,
}

/// One contact that is currently down, and where it landed.
struct Contact {
    id: u32,
    x0: f32,
    y0: f32,
}

/// The recogniser. One per window; feed it every touch down/move/up.
pub struct Taps {
    /// Which gestures are live (the `ui.txt` bitmask) — re-read at the start
    /// of every cycle, so turning one off takes effect on the next tap.
    enabled: u8,
    /// Device px per logical px (`GetDpiForWindow()/96`): the px thresholds
    /// above are logical, the events arrive in device px.
    scale: f32,
    /// The Navigator palette's rect in client px, when it is a visible tab.
    navigator: Option<[f32; 4]>,
    live: Vec<Contact>,
    /// Contacts that landed during this cycle (never decremented — a lift
    /// does not make a three-finger tap into a two-finger one).
    downs: usize,
    /// Sum of the landing positions, for the centroid the Navigator test
    /// uses.
    sum: [f32; 2],
    /// When the cycle's first contact landed.
    t_first: f64,
    /// Whether anything has lifted yet this cycle.
    saw_up: bool,
    /// Set by any rule below; a spoiled cycle can never fire.
    spoiled: bool,
}

impl Taps {
    pub const fn new() -> Self {
        Self {
            enabled: 0,
            scale: 1.0,
            navigator: None,
            live: Vec::new(),
            downs: 0,
            sum: [0.0, 0.0],
            t_first: 0.0,
            saw_up: false,
            spoiled: false,
        }
    }

    /// The knobs the caller owns: the enable bitmask, the display scale, and
    /// where the Navigator is right now. Call it before each [`Taps::down`];
    /// it never disturbs a cycle in progress.
    pub fn configure(&mut self, enabled: u8, scale: f32, navigator: Option<[f32; 4]>) {
        self.enabled = enabled;
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        self.navigator = navigator;
    }

    /// A contact landed. `size_px` is the longest side of the contact patch,
    /// or 0 when the device does not report one.
    pub fn down(&mut self, id: u32, x: f32, y: f32, t_ms: f64, size_px: f32) {
        if self.live.is_empty() {
            // Nothing on the glass: this opens a fresh cycle.
            self.reset_cycle();
            self.t_first = t_ms;
        } else if self.saw_up {
            // A finger landing after another has already lifted is a roll,
            // not a tap — and it would otherwise inflate `downs`.
            self.spoiled = true;
        }
        self.downs += 1;
        self.sum[0] += x;
        self.sum[1] += y;
        if self.downs > MAX_CONTACTS
            || size_px > PALM_PX * self.scale
            || t_ms - self.t_first > SPREAD_MS
            || t_ms < self.t_first // GetTickCount wrapped: trust nothing
            || self.live.iter().any(|c| c.id == id)
        {
            self.spoiled = true;
        }
        self.live.push(Contact { id, x0: x, y0: y });
    }

    /// A contact moved. Only the distance from where it landed matters — a
    /// pan/pinch/twist crosses the slop within the first events, a tap never
    /// does.
    pub fn moved(&mut self, id: u32, x: f32, y: f32) {
        let slop = SLOP_PX * self.scale;
        let far = self
            .live
            .iter()
            .any(|c| c.id == id && (x - c.x0).powi(2) + (y - c.y0).powi(2) > slop * slop);
        if far {
            self.spoiled = true;
        }
    }

    /// A contact lifted. Returns the gesture only on the lift that empties
    /// the glass — fingers coming up one at a time still resolve as one tap.
    pub fn up(&mut self, id: u32, t_ms: f64) -> Option<Action> {
        match self.live.iter().position(|c| c.id == id) {
            Some(i) => {
                self.live.remove(i);
            }
            // An id we never saw land (a contact that started before the
            // window had focus, or one we dropped): our picture of the
            // contact set is incomplete, so nothing this cycle is trustable.
            None => {
                self.spoiled = true;
                return None;
            }
        }
        self.saw_up = true;
        if t_ms - self.t_first > TAP_MS || t_ms < self.t_first {
            self.spoiled = true;
        }
        if !self.live.is_empty() {
            return None;
        }
        let action = self.resolve();
        self.reset_cycle();
        action
    }

    /// Drop whatever was in progress (focus loss — the ups will never come).
    pub fn cancel(&mut self) {
        self.live.clear();
        self.reset_cycle();
    }

    fn resolve(&self) -> Option<Action> {
        if self.spoiled || self.downs == 0 {
            return None;
        }
        let n = self.downs as f32;
        let c = [self.sum[0] / n, self.sum[1] / n];
        let on = |bit: u8| self.enabled & bit != 0;
        match self.downs {
            2 => on(UNDO).then_some(Action::Undo),
            // The Navigator arm and the plain arm are different gestures
            // with different switches: turning the Navigator one off does
            // NOT make a tap there redo instead.
            3 if self.over_navigator(c) => on(RESET_VIEW).then_some(Action::ResetView),
            3 => on(REDO).then_some(Action::Redo),
            _ => None,
        }
    }

    fn over_navigator(&self, p: [f32; 2]) -> bool {
        self.navigator
            .is_some_and(|[l, t, r, b]| p[0] >= l && p[0] <= r && p[1] >= t && p[1] <= b)
    }

    /// Cycle state only — the caller's knobs survive.
    fn reset_cycle(&mut self) {
        self.downs = 0;
        self.sum = [0.0, 0.0];
        self.t_first = 0.0;
        self.saw_up = false;
        self.spoiled = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: u8 = UNDO | REDO | RESET_VIEW;

    fn armed(enabled: u8) -> Taps {
        let mut t = Taps::new();
        t.configure(enabled, 1.0, None);
        t
    }

    /// Land `n` fingers 40 px apart at `t0`, lift them all at `t0 + dt`.
    /// Returns whatever the last lift resolved to.
    fn tap(g: &mut Taps, n: u32, t0: f64, dt: f64, x: f32, y: f32) -> Option<Action> {
        for i in 0..n {
            g.down(i, x + i as f32 * 40.0, y, t0, 0.0);
        }
        let mut last = None;
        for i in 0..n {
            last = g.up(i, t0 + dt);
        }
        last
    }

    #[test]
    fn a_clean_two_finger_tap_fires_undo_exactly_once() {
        let mut g = armed(ALL);
        g.down(1, 100.0, 100.0, 1000.0, 0.0);
        g.down(2, 160.0, 104.0, 1012.0, 0.0);
        // A tap is not perfectly still; a few px of wobble must survive.
        g.moved(1, 102.0, 101.0);
        g.moved(2, 158.0, 106.0);
        assert_eq!(g.up(1, 1090.0), None, "the first lift resolves nothing");
        assert_eq!(g.up(2, 1105.0), Some(Action::Undo));
        // Nothing lingers: the same lift cannot fire twice.
        assert_eq!(g.up(2, 1110.0), None);
    }

    #[test]
    fn a_slow_two_finger_hold_is_not_a_tap() {
        let mut g = armed(ALL);
        g.down(1, 100.0, 100.0, 1000.0, 0.0);
        g.down(2, 160.0, 100.0, 1010.0, 0.0);
        // Held past the tap window, then lifted cleanly.
        assert_eq!(g.up(1, 1900.0), None);
        assert_eq!(g.up(2, 1910.0), None, "a hold is not a tap");
    }

    #[test]
    fn a_two_finger_drag_is_not_a_tap() {
        let mut g = armed(ALL);
        g.down(1, 100.0, 100.0, 1000.0, 0.0);
        g.down(2, 160.0, 100.0, 1008.0, 0.0);
        for k in 1..=6 {
            let d = k as f32 * 5.0;
            g.moved(1, 100.0 + d, 100.0);
            g.moved(2, 160.0 + d, 100.0);
        }
        assert_eq!(g.up(1, 1080.0), None);
        assert_eq!(
            g.up(2, 1090.0),
            None,
            "a pan that happened to be quick is still a pan"
        );
    }

    /// A pinch: the fingers move apart rather than together, so neither
    /// midpoint nor duration would catch it — per-contact travel does.
    #[test]
    fn a_quick_pinch_is_not_a_tap() {
        let mut g = armed(ALL);
        g.down(1, 100.0, 100.0, 1000.0, 0.0);
        g.down(2, 160.0, 100.0, 1005.0, 0.0);
        g.moved(1, 80.0, 100.0);
        g.moved(2, 180.0, 100.0);
        assert_eq!(g.up(1, 1060.0), None);
        assert_eq!(g.up(2, 1070.0), None);
    }

    #[test]
    fn a_palm_next_to_a_finger_is_not_a_tap() {
        let mut g = armed(ALL);
        // The heel of the hand: a contact patch no fingertip can produce.
        g.down(1, 300.0, 400.0, 1000.0, 140.0);
        g.down(2, 360.0, 402.0, 1006.0, 22.0);
        assert_eq!(g.up(1, 1080.0), None);
        assert_eq!(g.up(2, 1090.0), None, "a big contact is never a fingertip");
    }

    #[test]
    fn contacts_landing_far_apart_in_time_are_not_a_tap() {
        let mut g = armed(ALL);
        g.down(1, 300.0, 400.0, 1000.0, 0.0);
        // Well inside the tap window, but nothing about this is one gesture.
        g.down(2, 360.0, 400.0, 1180.0, 0.0);
        assert_eq!(g.up(1, 1220.0), None);
        assert_eq!(g.up(2, 1230.0), None);
    }

    #[test]
    fn three_fingers_fire_redo_and_never_undo() {
        let mut g = armed(ALL);
        assert_eq!(
            tap(&mut g, 3, 5000.0, 80.0, 200.0, 200.0),
            Some(Action::Redo)
        );
        // And the two-finger tap still works right after it.
        assert_eq!(
            tap(&mut g, 2, 6000.0, 60.0, 200.0, 200.0),
            Some(Action::Undo)
        );
    }

    #[test]
    fn one_finger_and_four_fingers_fire_nothing() {
        let mut g = armed(ALL);
        // One finger is the pan gesture; four is a hand landing.
        assert_eq!(tap(&mut g, 1, 1000.0, 50.0, 200.0, 200.0), None);
        assert_eq!(tap(&mut g, 4, 2000.0, 50.0, 200.0, 200.0), None);
    }

    #[test]
    fn three_fingers_on_the_navigator_reset_the_view_instead_of_redoing() {
        let mut g = Taps::new();
        g.configure(ALL, 1.0, Some([800.0, 40.0, 990.0, 300.0]));
        // Centroid inside the Navigator rect.
        assert_eq!(
            tap(&mut g, 3, 1000.0, 60.0, 850.0, 120.0),
            Some(Action::ResetView)
        );
        // The same tap on the canvas is still Redo.
        assert_eq!(
            tap(&mut g, 3, 2000.0, 60.0, 200.0, 500.0),
            Some(Action::Redo)
        );
        // Two fingers on the Navigator are still Undo — only the
        // three-finger arm splits by place.
        assert_eq!(
            tap(&mut g, 2, 3000.0, 60.0, 850.0, 120.0),
            Some(Action::Undo)
        );
    }

    #[test]
    fn a_disabled_gesture_fires_nothing() {
        // Each switch is independent, and a disabled Navigator reset does
        // NOT fall back to redo.
        let mut off = armed(0);
        assert_eq!(tap(&mut off, 2, 1000.0, 50.0, 10.0, 10.0), None);
        assert_eq!(tap(&mut off, 3, 2000.0, 50.0, 10.0, 10.0), None);

        let mut undo_only = armed(UNDO);
        assert_eq!(
            tap(&mut undo_only, 2, 1000.0, 50.0, 10.0, 10.0),
            Some(Action::Undo)
        );
        assert_eq!(tap(&mut undo_only, 3, 2000.0, 50.0, 10.0, 10.0), None);

        let mut redo_only = armed(REDO);
        assert_eq!(tap(&mut redo_only, 2, 1000.0, 50.0, 10.0, 10.0), None);
        assert_eq!(
            tap(&mut redo_only, 3, 2000.0, 50.0, 10.0, 10.0),
            Some(Action::Redo)
        );

        let mut no_reset = Taps::new();
        no_reset.configure(UNDO | REDO, 1.0, Some([0.0, 0.0, 400.0, 400.0]));
        assert_eq!(
            tap(&mut no_reset, 3, 1000.0, 50.0, 100.0, 100.0),
            None,
            "the Navigator tap is its own gesture, not a redo in disguise"
        );
    }

    #[test]
    fn fingers_released_one_at_a_time_still_resolve() {
        let mut g = armed(ALL);
        g.down(1, 100.0, 100.0, 1000.0, 0.0);
        g.down(2, 150.0, 100.0, 1008.0, 0.0);
        g.down(3, 200.0, 100.0, 1016.0, 0.0);
        assert_eq!(g.up(2, 1120.0), None); // middle finger first
        assert_eq!(g.up(3, 1180.0), None);
        assert_eq!(
            g.up(1, 1240.0),
            Some(Action::Redo),
            "a staggered release inside the tap window is still one tap"
        );
    }

    #[test]
    fn a_finger_landing_after_another_lifted_is_not_a_tap() {
        let mut g = armed(ALL);
        g.down(1, 100.0, 100.0, 1000.0, 0.0);
        g.down(2, 150.0, 100.0, 1008.0, 0.0);
        assert_eq!(g.up(1, 1030.0), None);
        g.down(3, 200.0, 100.0, 1040.0, 0.0); // a roll, not a tap
        assert_eq!(g.up(2, 1060.0), None);
        assert_eq!(g.up(3, 1080.0), None);
    }

    /// The realistic palm case: the hand never lifts. Nothing may fire while
    /// it is down, and the recogniser must recover once it does.
    #[test]
    fn a_resting_palm_blocks_taps_until_it_lifts() {
        let mut g = armed(ALL);
        g.down(9, 500.0, 700.0, 1000.0, 0.0); // size unknown — worst case
        for _ in 0..3 {
            g.down(1, 100.0, 100.0, 2000.0, 0.0);
            g.down(2, 150.0, 100.0, 2008.0, 0.0);
            assert_eq!(g.up(1, 2060.0), None);
            assert_eq!(g.up(2, 2070.0), None, "the palm is still part of the set");
        }
        assert_eq!(g.up(9, 9000.0), None);
        assert_eq!(
            tap(&mut g, 2, 10000.0, 60.0, 100.0, 100.0),
            Some(Action::Undo),
            "the glass is clear again"
        );
    }

    #[test]
    fn a_stray_lift_cannot_fire_and_the_next_tap_still_works() {
        let mut g = armed(ALL);
        // An id we never saw land (focus arrived mid-contact).
        assert_eq!(g.up(77, 1000.0), None);
        assert_eq!(tap(&mut g, 2, 2000.0, 60.0, 10.0, 10.0), Some(Action::Undo));
    }

    #[test]
    fn cancel_drops_the_gesture_in_progress() {
        let mut g = armed(ALL);
        g.down(1, 100.0, 100.0, 1000.0, 0.0);
        g.down(2, 150.0, 100.0, 1008.0, 0.0);
        g.cancel(); // focus loss: the ups may never arrive
        assert_eq!(g.up(1, 1040.0), None);
        assert_eq!(g.up(2, 1050.0), None);
        assert_eq!(
            tap(&mut g, 2, 2000.0, 60.0, 10.0, 10.0),
            Some(Action::Undo),
            "and the recogniser is usable afterwards"
        );
    }

    /// The px rules are logical px: on a 200 % display the same finger
    /// wobble is twice as many device px, and the same palm twice as wide.
    #[test]
    fn the_display_scale_scales_the_px_rules() {
        let mut hidpi = Taps::new();
        hidpi.configure(ALL, 2.0, None);
        hidpi.down(1, 100.0, 100.0, 1000.0, 0.0);
        hidpi.down(2, 200.0, 100.0, 1008.0, 0.0);
        hidpi.moved(1, 118.0, 100.0); // 18 px: over the 1x slop, under 2x
        assert_eq!(hidpi.up(1, 1060.0), None);
        assert_eq!(hidpi.up(2, 1070.0), Some(Action::Undo));

        let mut onex = armed(ALL);
        onex.down(1, 100.0, 100.0, 1000.0, 0.0);
        onex.down(2, 200.0, 100.0, 1008.0, 0.0);
        onex.moved(1, 118.0, 100.0);
        assert_eq!(onex.up(1, 1060.0), None);
        assert_eq!(onex.up(2, 1070.0), None, "18 px at 1x is a drag");

        // A patch that is a palm at 1x is still a fingertip at 2x.
        let mut hidpi2 = Taps::new();
        hidpi2.configure(ALL, 2.0, None);
        hidpi2.down(1, 100.0, 100.0, 1000.0, 100.0);
        hidpi2.down(2, 200.0, 100.0, 1008.0, 100.0);
        assert_eq!(hidpi2.up(1, 1060.0), None);
        assert_eq!(hidpi2.up(2, 1070.0), Some(Action::Undo));
    }

    /// `GetTickCount` wraps every 49.7 days. A timestamp that goes backwards
    /// must never be read as "0 ms elapsed, what a quick tap".
    #[test]
    fn a_backwards_clock_never_fires() {
        let mut g = armed(ALL);
        g.down(1, 100.0, 100.0, 4_294_967_000.0, 0.0);
        g.down(2, 150.0, 100.0, 12.0, 0.0); // wrapped mid-gesture
        assert_eq!(g.up(1, 40.0), None);
        assert_eq!(g.up(2, 50.0), None);

        let mut g2 = armed(ALL);
        g2.down(1, 100.0, 100.0, 4_294_967_000.0, 0.0);
        g2.down(2, 150.0, 100.0, 4_294_967_010.0, 0.0);
        assert_eq!(g2.up(1, 20.0), None); // the wrap lands on the lift
        assert_eq!(g2.up(2, 30.0), None);
    }
}
