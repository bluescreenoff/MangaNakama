//! User preferences: `Prefs`, plain `k=v` lines in `prefs.txt` beside the
//! exe — no config framework, the same idiom as `layout.rs` (`ui.txt`),
//! `recent.txt` and `swatches.txt`. Keys are shipped API: they never change
//! once persisted.
//!
//! **Why not `ui.txt`.** That is the file we tell people to DELETE to fix a
//! wrecked dock, and the screenshot recipe deletes it for an honest capture
//! (docs/DECISIONS.md). Preferences kept there would be silently reset by
//! every layout reset and every `--warp` run, and the user would have no way
//! to tell that had happened — a silent failure, which is the whole
//! argument. `ui.txt` is also workspace-scoped (its dock columns are
//! exported and swapped as named workspaces); preferences are not.
//!
//! **Every default here is today's constant, exactly.** Adding the panel
//! changes nothing for a user who never opens it; the values the owner has
//! opinions about (the undo depth, the 2048² canvas, the New Comic preset)
//! become his to change without a code change, which is the point.
//!
//! **Unknown keys survive a round trip in BOTH directions.** Forward: a key
//! from a newer build is ignored at load. Backward: it is also written back
//! out verbatim at save, so downgrading, drawing, and upgrading again does
//! not silently drop next year's settings. `ui.txt` still has that hole
//! (`to_body` there rewrites the whole file). There is no `version=` key —
//! the key set is the version, and a version number only invites migration
//! code nobody writes and everybody has to read.
//!
//! A missing or corrupt file is not an error: absent → defaults, and a
//! mangled line keeps its own default while the other nine still load. No
//! `Result`, no dialog, no "your settings were reset" toast, no first-run
//! wizard.

use std::path::PathBuf;

/// Autosave interval in minutes — the owner's original 15-minute
/// `AUTOSAVE_MS` in `main.rs`, which now reads this.
pub const AUTOSAVE_MIN: u32 = 15;
/// Mouse-only pull-string smoothing floor, SCREEN px (round-22 fix for
/// sparse `WM_MOUSEMOVE`). The pen bypasses it and keeps the sub tool's own
/// stabilizer setting.
pub const MOUSE_SMOOTH_FLOOR_PX: f32 = 12.0;
/// Fit-to-window margin (owner, 2026-08-19: 0.90 → 0.98 — "opens too small").
pub const FIT_MARGIN: f32 = 0.98;
/// One wheel notch's zoom factor.
pub const WHEEL_STEP: f32 = 1.15;
/// View rotate step in degrees — `-` left, F9 right, the two toolbar
/// buttons and the two View-menu items all used to spell 15 out themselves.
pub const ROTATE_STEP_DEG: f32 = 15.0;
/// How many files the MRU (`recent.txt`) keeps.
pub const RECENT_DEPTH: usize = 8;
/// Point size a freshly typed text item starts at.
pub const TEXT_SIZE_PT: f32 = 12.0;

/// The ten values that are genuinely user preferences rather than
/// architecture constants (docs/design/PREFERENCES-SPEC.md §0 and, more
/// importantly, §5 — the list of what stays hardcoded and why).
pub struct Prefs {
    /// 0 = off; otherwise 5..=60 (CSP's own range).
    pub autosave_min: u32,
    /// PR-041: write the recovery copy after every operation instead of
    /// waiting for the timer. OFF by default, because on a print-resolution
    /// page the write is not free and the 15-minute clock is the right
    /// default for most people — this is the setting for the session you
    /// cannot afford to repeat.
    ///
    /// It does not replace the timer: with both on you get whichever comes
    /// first, which is what "belt and braces" means and what CSP does.
    pub autosave_every_op: bool,
    /// Undo groups kept per document, 50..=5000.
    pub undo_depth: usize,
    /// Mouse smoothing floor, screen px; 0 = off (mouse then behaves like
    /// the pen and takes the sub tool's stabilizer verbatim).
    pub mouse_smooth_px: f32,
    /// The blank startup canvas, px.
    pub new_canvas: (u32, u32),
    /// Fit-to-window margin, 0.80..=1.00.
    pub fit_margin: f32,
    /// Wheel zoom factor per notch, 1.02..=1.50.
    pub wheel_step: f32,
    /// View rotation step, 1..=90 degrees.
    pub rotate_step_deg: f32,
    /// MRU length, 1..=32.
    pub recent_depth: usize,
    /// New text items' point size, 4..=72 (the Tool Property slider's range).
    pub text_size_pt: f32,
    /// New Comic's starting page preset, by `PageSetup::name`. Empty — the
    /// default — and any name this build does not know both resolve to the
    /// first preset, which is exactly today's behaviour.
    pub new_preset: String,
    /// The status bar's "N pages unexported" chip (owner ask 2026-08-22).
    /// ON by default — it is the reminder for the case it was asked for
    /// ("I fixed two panels and forgot to re-export"), and it says nothing
    /// at all until a work has been exported once.
    pub export_reminder: bool,
    /// The Layers palette's command-icon size, px, 14..=32 (owner
    /// 2026-08-21: "a bit bigger by default, and a setting"). The toggle
    /// strip above the list derives from it at 0.8×.
    pub palette_icon_px: f32,
    /// The chrome's colour scheme, by `ui::theme` built-in name. A name this
    /// build does not know resolves to `dark` at read time and is otherwise
    /// left alone — same rule as `new_preset`, and the reason a theme from a
    /// newer build survives a downgrade instead of being rewritten to `dark`.
    pub theme: String,
    /// Whole-UI size multiplier, 0.75..=1.75, 1.0 = the window DPI alone
    /// (owner 2026-08-21: "his UI reads too small"). Applied by scaling the
    /// shell's effective pixels-per-point, so fonts, spacing, icons, input
    /// mapping and the canvas hole all move together — never a font-size
    /// sweep.
    pub ui_scale: f32,
    /// Tint icons by what they do (`ui::icons::IconRole`), ON by default —
    /// the owner asked for coloured icons everywhere and an off switch, in
    /// that order. Off paints every glyph in plain chrome grey, which is
    /// what this app looked like before 2026-08-22.
    pub icon_colours: bool,
    /// plans/05 item 6: show 3D-pose materials in the bank (their tree
    /// branch and grid rows). OFF by default — the owner's locked call
    /// (2026-08-24): hidden by default, a setting unhides, never a
    /// "can't use" badge.
    pub show_pose3d_materials: bool,
    /// T1 step 3: the theme editor's "save as" name field. Deliberately
    /// NOT serialized — it is typing state, not a preference; to_body
    /// names its keys explicitly so this never leaks into prefs.txt.
    pub theme_save_name: String,
    /// `k=v` lines this build does not recognise, kept so saving here does
    /// not delete a newer build's settings.
    unknown: Vec<String>,
    dirty: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            autosave_min: AUTOSAVE_MIN,
            autosave_every_op: false,
            undo_depth: mn_core::UNDO_LIMIT,
            mouse_smooth_px: MOUSE_SMOOTH_FLOOR_PX,
            new_canvas: mn_core::DEFAULT_SIZE,
            fit_margin: FIT_MARGIN,
            wheel_step: WHEEL_STEP,
            rotate_step_deg: ROTATE_STEP_DEG,
            recent_depth: RECENT_DEPTH,
            text_size_pt: TEXT_SIZE_PT,
            new_preset: String::new(),
            export_reminder: true,
            palette_icon_px: PALETTE_ICON_PX,
            theme: THEME.to_owned(),
            ui_scale: 1.0,
            icon_colours: true,
            show_pose3d_materials: false,
            theme_save_name: String::new(),
            unknown: Vec::new(),
            dirty: false,
        }
    }
}

/// Default Layers-palette command-icon size, px.
pub const PALETTE_ICON_PX: f32 = 20.0;
/// The colour scheme a fresh install starts in — the one every screenshot in
/// `docs/` was taken in.
pub const THEME: &str = "dark";

/// Clamp that also catches NaN and infinity — `f32::clamp` passes NaN
/// straight through, and a NaN fit margin is a canvas that never appears.
fn finite(v: f32, fallback: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { fallback }
}

impl Prefs {
    /// The autosave timer's period in ms; 0 = off (the timer is not armed).
    pub fn autosave_ms(&self) -> u32 {
        self.autosave_min.saturating_mul(60_000)
    }

    /// The New Comic starting preset. An unknown name (a `prefs.txt` from a
    /// build with a different preset list, or a typo) falls back to the
    /// first preset rather than failing.
    pub fn new_preset_setup(&self) -> mn_core::PageSetup {
        let mut list = mn_core::PageSetup::presets();
        match list.iter().position(|p| p.name == self.new_preset) {
            Some(i) => list.remove(i),
            None => list.remove(0),
        }
    }

    /// The panel calls this when a widget reported `changed()`. egui only
    /// reports a REAL value change, which is why this module has one flag
    /// rather than `layout.rs`'s ten `note_*` setters: nothing but the
    /// Preferences window ever writes a preference, so there is no
    /// per-frame writer to guard against.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// The "Reset to defaults" footer. Unknown keys are NOT dropped — this
    /// resets our settings, not a newer build's.
    pub fn reset(&mut self) {
        let unknown = std::mem::take(&mut self.unknown);
        *self = Self {
            unknown,
            dirty: true,
            ..Self::default()
        };
    }

    pub fn save_if_dirty(&mut self) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        let Some(p) = prefs_path() else { return };
        let _ = std::fs::write(p, self.to_body());
    }

    /// The full `prefs.txt` content: our keys, then any line a newer build
    /// wrote that this one does not understand, verbatim.
    fn to_body(&self) -> String {
        let mut body = format!(
            "autosave_min={}\nautosave_every_op={}\nundo_depth={}\nmouse_smooth_px={}\nnew_canvas_w={}\nnew_canvas_h={}\nfit_margin={}\nwheel_step={}\nrotate_step_deg={}\nrecent_depth={}\ntext_size_pt={}\nnew_preset={}\nexport_reminder={}\npalette_icon_px={}\ntheme={}\nicon_colours={}\nui_scale={}\nshow_pose3d_materials={}\n",
            self.autosave_min,
            u8::from(self.autosave_every_op),
            self.undo_depth,
            self.mouse_smooth_px,
            self.new_canvas.0,
            self.new_canvas.1,
            self.fit_margin,
            self.wheel_step,
            self.rotate_step_deg,
            self.recent_depth,
            self.text_size_pt,
            self.new_preset.replace('\n', ""),
            u8::from(self.export_reminder),
            self.palette_icon_px,
            self.theme.replace('\n', ""),
            u8::from(self.icon_colours),
            self.ui_scale,
            u8::from(self.show_pose3d_materials),
        );
        for line in &self.unknown {
            body.push_str(line);
            body.push('\n');
        }
        body
    }

    /// Apply one `key=value` line. A value that will not parse leaves its
    /// field at the value it already had — per LINE, never per file.
    fn apply_kv(&mut self, line: &str) {
        let Some((k, v)) = line.split_once('=') else {
            return;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "autosave_min" => self.autosave_min = v.parse().unwrap_or(self.autosave_min),
            // Written as 0/1, but a hand-edited "true" is what a person
            // reaching for this file would type, so take that too.
            "autosave_every_op" => {
                self.autosave_every_op = match v {
                    "1" | "true" => true,
                    "0" | "false" => false,
                    _ => self.autosave_every_op,
                }
            }
            "undo_depth" => self.undo_depth = v.parse().unwrap_or(self.undo_depth),
            "mouse_smooth_px" => self.mouse_smooth_px = v.parse().unwrap_or(self.mouse_smooth_px),
            "new_canvas_w" => self.new_canvas.0 = v.parse().unwrap_or(self.new_canvas.0),
            "new_canvas_h" => self.new_canvas.1 = v.parse().unwrap_or(self.new_canvas.1),
            "fit_margin" => self.fit_margin = v.parse().unwrap_or(self.fit_margin),
            "wheel_step" => self.wheel_step = v.parse().unwrap_or(self.wheel_step),
            "rotate_step_deg" => self.rotate_step_deg = v.parse().unwrap_or(self.rotate_step_deg),
            "recent_depth" => self.recent_depth = v.parse().unwrap_or(self.recent_depth),
            "text_size_pt" => self.text_size_pt = v.parse().unwrap_or(self.text_size_pt),
            "new_preset" => self.new_preset = v.to_owned(),
            // Same two spellings as `autosave_every_op`, and the same
            // refusal to read gibberish as "off".
            "export_reminder" => {
                self.export_reminder = match v {
                    "1" | "true" => true,
                    "0" | "false" => false,
                    _ => self.export_reminder,
                }
            }
            "palette_icon_px" => self.palette_icon_px = v.parse().unwrap_or(self.palette_icon_px),
            // Not validated here on purpose: an unrecognised name resolves to
            // `dark` at read time (`ui::theme::by_name`) and the file keeps
            // what it said, so a theme added in a newer build is not deleted
            // by an older one. An EMPTY value is the one exception — that is
            // a mangled line, not a choice.
            "theme" => {
                if !v.is_empty() {
                    self.theme = v.to_owned();
                }
            }
            // Both spellings, and gibberish keeps the value — the same rule
            // as the two flags above. Reading "icon_colours=yes" as OFF
            // would silently grey the whole app.
            "icon_colours" => {
                self.icon_colours = match v {
                    "1" | "true" => true,
                    "0" | "false" => false,
                    _ => self.icon_colours,
                }
            }
            // The same honest-bool rule: gibberish keeps the default.
            "show_pose3d_materials" => {
                self.show_pose3d_materials = match v {
                    "1" | "true" => true,
                    "0" | "false" => false,
                    _ => self.show_pose3d_materials,
                }
            }
            "ui_scale" => self.ui_scale = v.parse().unwrap_or(self.ui_scale),
            // A key we do not know is a key from a NEWER build. Keep the
            // line so the next save writes it back out instead of eating it.
            _ if !k.is_empty() => self.unknown.push(line.to_owned()),
            _ => {}
        }
    }

    /// Clamp at LOAD, not at write: a hand-edited `undo_depth=99999999`
    /// becomes 5000 rather than an out-of-memory, and the file the user
    /// edited still says what he typed until he touches the panel.
    fn clamp(&mut self) {
        // 0 stays 0 — that is "off", not "as often as possible".
        if self.autosave_min != 0 {
            self.autosave_min = self.autosave_min.clamp(5, 60);
        }
        self.undo_depth = self.undo_depth.clamp(50, 5000);
        self.palette_icon_px = finite(self.palette_icon_px, PALETTE_ICON_PX, 14.0, 32.0);
        self.mouse_smooth_px = finite(
            self.mouse_smooth_px,
            MOUSE_SMOOTH_FLOOR_PX,
            0.0,
            mn_core::stabilize::MAX_STRING_PX,
        );
        self.new_canvas.0 = self.new_canvas.0.clamp(1, 65535);
        self.new_canvas.1 = self.new_canvas.1.clamp(1, 65535);
        self.fit_margin = finite(self.fit_margin, FIT_MARGIN, 0.80, 1.00);
        self.wheel_step = finite(self.wheel_step, WHEEL_STEP, 1.02, 1.50);
        self.rotate_step_deg = finite(self.rotate_step_deg, ROTATE_STEP_DEG, 1.0, 90.0);
        self.recent_depth = self.recent_depth.clamp(1, 32);
        self.text_size_pt = finite(self.text_size_pt, TEXT_SIZE_PT, 4.0, 72.0);
        self.ui_scale = finite(self.ui_scale, 1.0, 0.75, 1.75);
        // A blank name would be written back out as a blank `theme=` line —
        // the only "clamp" a theme name gets. An UNKNOWN name is left alone
        // (see `apply_kv`); it simply paints `dark`.
        if self.theme.trim().is_empty() {
            self.theme = THEME.to_owned();
        }
    }

    pub(super) fn load() -> Self {
        let mut me = Self::default();
        let Some(text) = prefs_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return me;
        };
        for line in text.lines() {
            me.apply_kv(line);
        }
        me.clamp();
        me.dirty = false;
        me
    }
}

fn prefs_path() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("prefs.txt"))
}

/// The panel's grey footer line: where the file is, so a user who has wedged
/// a setting can delete it without asking anyone. Falls back to the bare
/// name if the exe path is somehow unreadable.
pub fn path_hint() -> String {
    prefs_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "prefs.txt".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_body(body: &str) -> Prefs {
        let mut p = Prefs::default();
        for line in body.lines() {
            p.apply_kv(line);
        }
        p.clamp();
        p
    }

    /// The load-bearing test of the whole round: every default is TODAY'S
    /// constant. If one of these ever fails, shipping the panel changed
    /// behaviour for a user who never opened it.
    #[test]
    fn defaults_are_todays_constants() {
        let p = Prefs::default();
        assert_eq!(p.autosave_min, 15, "main.rs AUTOSAVE_MS = 15 * 60 * 1000");
        assert_eq!(p.autosave_ms(), 15 * 60 * 1000);
        assert!(
            !p.autosave_every_op,
            "PR-041 is opt-in: the clock alone is what this build did before"
        );
        assert_eq!(p.undo_depth, mn_core::UNDO_LIMIT);
        assert_eq!(p.undo_depth, 400, "the owner moved it 200 → 400");
        assert_eq!(p.mouse_smooth_px, 12.0);
        assert_eq!(p.new_canvas, mn_core::DEFAULT_SIZE);
        assert_eq!(p.new_canvas, (2048, 2048));
        assert_eq!(p.fit_margin, 0.98);
        assert_eq!(p.wheel_step, 1.15);
        assert_eq!(p.rotate_step_deg, 15.0);
        assert_eq!(p.recent_depth, 8);
        assert_eq!(p.text_size_pt, 12.0);
        assert!(
            p.export_reminder,
            "the unexported-pages reminder ships ON (owner ask 2026-08-22)"
        );
        assert!(
            p.icon_colours,
            "coloured icons ship ON; monochrome is the opt-out (owner 2026-08-21)"
        );
        // Empty resolves to the first preset — Shueisha A (Jump), which is
        // what `NewComicDraft::default` used to spell out itself.
        assert!(p.new_preset.is_empty());
        assert_eq!(
            p.new_preset_setup().name,
            mn_core::PageSetup::presets()[0].name
        );
    }

    /// The theme name is a plain string that round-trips, and an unknown one
    /// is NOT rewritten — a `prefs.txt` naming a theme a newer build added
    /// must come back with that name after this build has saved over it,
    /// even though this build paints `dark` meanwhile.
    #[test]
    fn theme_roundtrips_and_an_unknown_name_paints_dark() {
        assert_eq!(Prefs::default().theme, "dark");
        assert_eq!(
            crate::ui::theme::by_name(&Prefs::default().theme),
            crate::ui::theme::DARK
        );

        let mut me = Prefs::default();
        me.theme = "violet".to_owned();
        let back = from_body(&me.to_body());
        assert_eq!(back.theme, "violet");
        assert_eq!(
            crate::ui::theme::by_name(&back.theme),
            crate::ui::theme::VIOLET
        );

        // From the future: kept verbatim, painted as dark, written back out.
        let future = from_body("theme=solarized-2027\n");
        assert_eq!(future.theme, "solarized-2027");
        assert_eq!(
            crate::ui::theme::by_name(&future.theme),
            crate::ui::theme::DARK,
            "an unknown theme must fall back rather than wedge start-up"
        );
        assert!(future.to_body().contains("theme=solarized-2027\n"));
        assert_eq!(
            future.to_body().matches("theme=").count(),
            1,
            "and it must not ALSO be kept as an unknown key: {}",
            future.to_body()
        );

        // A mangled line keeps the default, like every other key here — and
        // must not write a blank `theme=` back out.
        assert_eq!(from_body("theme=\n").theme, "dark");
    }

    #[test]
    fn body_roundtrips() {
        let mut me = Prefs::default();
        me.autosave_min = 30;
        me.autosave_every_op = true;
        me.undo_depth = 1000;
        me.mouse_smooth_px = 0.0;
        me.new_canvas = (1200, 1800);
        me.fit_margin = 0.9;
        me.wheel_step = 1.05;
        me.rotate_step_deg = 5.0;
        me.recent_depth = 16;
        me.text_size_pt = 20.0;
        me.new_preset = "Doujinshi B5 600dpi (同人誌)".to_owned();
        me.export_reminder = false;
        me.ui_scale = 1.25;

        let back = from_body(&me.to_body());
        assert_eq!(back.ui_scale, 1.25, "UI size persists");
        assert!(!back.export_reminder, "turning the reminder off persists");
        assert_eq!(back.autosave_min, 30);
        assert!(back.autosave_every_op);
        assert_eq!(back.undo_depth, 1000);
        assert_eq!(back.mouse_smooth_px, 0.0);
        assert_eq!(back.new_canvas, (1200, 1800));
        assert_eq!(back.fit_margin, 0.9);
        assert_eq!(back.wheel_step, 1.05);
        assert_eq!(back.rotate_step_deg, 5.0);
        assert_eq!(back.recent_depth, 16);
        assert_eq!(back.text_size_pt, 20.0);
        assert_eq!(back.new_preset, "Doujinshi B5 600dpi (同人誌)");
        assert_eq!(
            back.new_preset_setup().name,
            "Doujinshi B5 600dpi (同人誌)",
            "a known preset name resolves to that preset"
        );
    }

    /// Corruption is per LINE. One mangled row keeps its own default and
    /// every other row still loads — no `Result`, no reset-everything.
    #[test]
    fn one_bad_line_does_not_take_the_file_down() {
        let p = from_body(
            "autosave_min=30\n\
             undo_depth=not-a-number\n\
             \n\
             this line has no equals sign at all\n\
             mouse_smooth_px=\n\
             recent_depth=16\n\
             text_size_pt=20\n",
        );
        assert_eq!(p.autosave_min, 30, "the good lines before it still load");
        assert_eq!(p.undo_depth, 400, "the mangled line keeps its default");
        assert_eq!(p.mouse_smooth_px, 12.0, "an empty value keeps its default");
        assert_eq!(p.recent_depth, 16, "the good lines after it still load");
        assert_eq!(p.text_size_pt, 20.0);
    }

    /// The hole `ui.txt` has and this file does not: an OLDER exe reading a
    /// NEWER file must not delete the keys it never knew. Downgrade, draw,
    /// upgrade, and next year's settings are still there.
    #[test]
    fn unknown_keys_survive_a_downgrade() {
        let newer = "autosave_min=30\n\
                     ruler_snap_deg=7.5\n\
                     undo_depth=800\n\
                     some_2027_feature=on\n";
        let mut me = from_body(newer);
        assert_eq!(me.autosave_min, 30);
        assert_eq!(me.undo_depth, 800);

        // This build then changes something and saves.
        me.autosave_min = 5;
        let body = me.to_body();
        assert!(body.contains("\nautosave_min=5\n") || body.starts_with("autosave_min=5\n"));
        assert!(
            body.contains("ruler_snap_deg=7.5"),
            "an unknown key must be written back verbatim: {body}"
        );
        assert!(
            body.contains("some_2027_feature=on"),
            "…all of them, not just the first: {body}"
        );
        // And they survive a second round trip rather than doubling up.
        let again = from_body(&body).to_body();
        assert_eq!(again.matches("ruler_snap_deg=7.5").count(), 1, "{again}");
    }

    /// Reset restores every value but keeps a newer build's keys — it
    /// resets OUR settings, not someone else's.
    #[test]
    fn reset_keeps_unknown_keys() {
        let mut me = from_body("undo_depth=900\nfrom_the_future=1\n");
        assert_eq!(me.undo_depth, 900);
        me.reset();
        assert_eq!(me.undo_depth, mn_core::UNDO_LIMIT);
        assert!(me.dirty, "the footer button must schedule a save");
        assert!(me.to_body().contains("from_the_future=1"));
    }

    /// Clamping happens at LOAD. Nonsense in the file becomes a usable
    /// value in memory instead of an out-of-memory or a blank canvas.
    #[test]
    fn hand_edited_nonsense_is_clamped_at_load() {
        let p = from_body(
            "undo_depth=99999999\n\
             autosave_min=3\n\
             mouse_smooth_px=9000\n\
             fit_margin=NaN\n\
             wheel_step=inf\n\
             rotate_step_deg=0\n\
             recent_depth=0\n\
             text_size_pt=-4\n\
             new_canvas_w=0\n\
             new_canvas_h=999999\n",
        );
        assert_eq!(p.undo_depth, 5000);
        assert_eq!(p.autosave_min, 5, "below the range, not off");
        assert_eq!(p.mouse_smooth_px, mn_core::stabilize::MAX_STRING_PX);
        assert_eq!(p.fit_margin, FIT_MARGIN, "NaN falls back, it does not clamp");
        assert_eq!(p.wheel_step, WHEEL_STEP, "infinity likewise");
        assert_eq!(p.rotate_step_deg, 1.0);
        assert_eq!(p.recent_depth, 1);
        assert_eq!(p.text_size_pt, 4.0);
        assert_eq!(p.new_canvas, (1, 65535));

        // Zero autosave is the one value that must NOT be clamped up: it
        // means off, and clamping it to 5 would turn a deliberate "never
        // autosave" into "autosave every five minutes".
        assert_eq!(from_body("autosave_min=0\n").autosave_min, 0);
        assert_eq!(from_body("autosave_min=0\n").autosave_ms(), 0);
    }

    /// An empty or absent file is not an error and never produces a wizard:
    /// `load` on a path that does not exist is just `Default`.
    #[test]
    fn empty_file_is_the_defaults() {
        let p = from_body("");
        let d = Prefs::default();
        assert_eq!(p.autosave_min, d.autosave_min);
        assert_eq!(p.undo_depth, d.undo_depth);
        assert_eq!(p.new_canvas, d.new_canvas);
        assert!(!p.dirty, "reading must never schedule a write");
    }

    /// The file's booleans are WRITTEN as 0/1, but `true`/`false` is what a
    /// person hand-editing prefs.txt types, so both read — and anything
    /// else keeps the default rather than being read as false, which would
    /// silently turn the setting off.
    #[test]
    fn the_per_operation_flag_reads_both_spellings_and_ignores_gibberish() {
        assert!(from_body("autosave_every_op=1\n").autosave_every_op);
        assert!(from_body("autosave_every_op=true\n").autosave_every_op);
        assert!(!from_body("autosave_every_op=0\n").autosave_every_op);
        assert!(!from_body("autosave_every_op=false\n").autosave_every_op);

        let mut p = Prefs::default();
        p.autosave_every_op = true;
        p.apply_kv("autosave_every_op=yes please");
        assert!(
            p.autosave_every_op,
            "an unparseable value keeps the value it had — per line, as everywhere else here"
        );

        assert!(!from_body("export_reminder=0\n").export_reminder);
        assert!(!from_body("export_reminder=false\n").export_reminder);
        assert!(from_body("export_reminder=1\n").export_reminder);
        assert!(
            from_body("export_reminder=maybe\n").export_reminder,
            "gibberish must not silently turn the reminder off"
        );

        assert!(!from_body("icon_colours=0\n").icon_colours);
        assert!(!from_body("icon_colours=false\n").icon_colours);
        assert!(from_body("icon_colours=1\n").icon_colours);
        assert!(
            from_body("icon_colours=nope\n").icon_colours,
            "gibberish must not silently grey every icon in the app"
        );
    }

    /// The icon-colour switch survives the round trip both ways — turning it
    /// off is the whole point of the setting, and an off that comes back on
    /// after a restart is the same bug as no setting at all.
    #[test]
    fn icon_colours_round_trips() {
        let mut me = Prefs::default();
        me.icon_colours = false;
        let back = from_body(&me.to_body());
        assert!(!back.icon_colours);
        assert!(me.to_body().contains("icon_colours=0\n"));
        assert!(
            from_body(&back.to_body())
                .to_body()
                .contains("icon_colours=0")
        );

        // And back on again.
        let on = from_body("icon_colours=1\n");
        assert!(on.icon_colours);
        assert!(on.to_body().contains("icon_colours=1\n"));
    }
}
