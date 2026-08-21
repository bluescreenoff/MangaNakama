//! Persisted palette layout: `UiLayout` (plain k=v lines in `ui.txt`
//! beside the exe — no config framework). Keys are shipped API: they never
//! change once persisted. Since round 21 the palette arrangement lives in
//! the two serialized dock columns (`dock_left=` / `dock_right=`, JSON from
//! egui_dock); the old order/collapsed/floating keys from the pre-docking
//! columns are ignored when found (and migrated away on the next save).

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Persisted palette layout: column widths + the two dock columns (as JSON,
/// handed in by `App::sync_dock_layout` — egui_dock types stay out of this
/// module so it keeps no dependency on them).
pub struct UiLayout {
    pub left_w: f32,
    pub right_w: f32,
    pub dock_left: String,
    pub dock_right: String,
    /// Tool Property sections hidden from the compact palette, comma-joined
    /// (the full-properties window's eye toggles).
    pub prop_hidden: String,
    /// Restored window geometry, `win=x,y,w,h,max` (main.rs feeds it; empty =
    /// first run, default placement).
    pub win: String,
    /// In-app GPU dab switch (`gpu_dabs=1`) — the user-facing replacement
    /// for `--gpu-dabs` (TODO #0.1). The stored value is the REQUESTED
    /// state; startup ANDs it with `gpu_dabs_supported()` in main.rs, so a
    /// ui.txt carried to a weaker adapter just stays on the cpu path.
    /// Default OFF: the round-34 default-ON flip (owner's same-day
    /// instruction) was REVERTED the same day on the auditor's
    /// counter-recommendation relayed by the owner — re-flip criteria
    /// (DECISIONS 8.9): TODO #0.1 (wash/texture/smudge on GPU) has landed
    /// AND a benchmark number exists on the owner's hardware. The View-menu
    /// toggle remains the manual path meanwhile.
    pub gpu_dabs: bool,
    /// Whether ui.txt actually CARRIED a `gpu_dabs=` line (session-only,
    /// never written): the DECISIONS 8.9 re-flip runs as a per-adapter
    /// measured auto-default (`bench::resolve_auto`), and an auto verdict
    /// must never override a choice the user made — absent key = the user
    /// never chose, and only then may the measurement decide.
    pub gpu_dabs_explicit: bool,
    /// Recently used font families, newest first, max 10 (round 34, CSP
    /// Font-list parity) — a JSON string array on one line.
    pub recent_fonts: Vec<String>,
    /// User-added material folders (TRIAGE 133), absolute paths — a JSON
    /// string array on one line. The shipped assets/materials starter
    /// folder is always first and never persisted here.
    pub material_folders: Vec<String>,
    /// Material use counters (frequency-of-use sorting): full path →
    /// count, JSON on one line.
    pub material_uses: String,
    /// Quick Access pins (UI-050), labels joined with U+001F.
    pub quick_pins: String,
    /// Named workspaces (UI-060): one JSON line, [[name, dock_left,
    /// dock_right, left_w, right_w, prop_hidden], ...].
    pub workspaces: String,
    /// The current workspace's NAME (UI-061's checkmark), or "".
    pub workspace_current: String,
    /// Touch TAP gestures, as a bitmask (`gesture.rs`): 1 = two-finger tap
    /// undoes, 2 = three-finger tap redoes, 4 = three-finger tap on the
    /// Navigator resets rotation + flip. Add them; 7 = all three.
    /// **Default 0 — every gesture off**, because a resting palm is also two
    /// contacts and a phantom undo eats the stroke the user just drew. There
    /// is no UI switch yet: this key in `ui.txt` is the switch (keys.html).
    pub touch_gestures: u8,
    /// The Color palette's Recent strip (CO-042): `#rrggbb` entries newest
    /// first, comma-joined, at most `COLOR_HISTORY_MAX`. Kept here rather
    /// than in `swatches.txt` because that file is the user's palette —
    /// chosen colours he may hand-edit or back up — while this is churn
    /// the app generates behind him.
    pub color_history: Vec<String>,
    /// Auto-register eyedropped colours into the Color Set (CO-023).
    /// **Default OFF.** The Recent strip already remembers every pick and
    /// forgets it again; a Color Set that grows on its own is noise by the
    /// end of the first working day, and it is the half the user curates.
    pub auto_swatch: bool,
    /// Reader v2: the last-read PAGE index (reader_close notes it;
    /// reader_open maps it back to a screen). App-level — the honest
    /// single-project v1: multi-project projects overwrite each other
    /// (recorded; per-project needs work-folder identity in the key).
    ///
    /// A page, not a screen: screen numbering depends on the view mode
    /// and the shift-pair offset, neither of which is persisted. The key
    /// is `reader_page=` — the old `reader_last=` meant a screen, so it
    /// falls through the unknown-key branch instead of being misread.
    pub reader_page: usize,
    /// CV-041: the manuscript crop marks and margins (bleed / trim / inner
    /// border / safety) are hidden from the canvas. The page keeps them —
    /// this draws nothing, it deletes nothing, and export never saw them.
    /// **Default false (shown)**, and an absent key stays shown: a ui.txt
    /// written by an older build must not make a working artist's guides
    /// vanish. Persisted, unlike the Tab hides, because it is a workspace
    /// preference rather than a panic button.
    pub guides_hidden: bool,
    /// The brush panel's LIVE test stroke is folded away. **Default false
    /// (shown)** — the strip is the answer to tuning sixteen parameter pages
    /// blind, so an absent key and any junk value both leave it visible; only
    /// a literal `1` folds it, the `guides_hidden` rule.
    pub test_stroke_hidden: bool,
    /// `G-011`/`G-012`: the saved gradient set, `mn_core::GradientSet`'s
    /// item list as one JSON line. Empty = never saved, and startup seeds
    /// the starter set; `[]` is a user who deleted every gradient and is
    /// entitled to keep it empty.
    pub gradients: String,
    /// Per-sub-tool brush SIZES: preset key → dab diameter in canvas px, one
    /// JSON object on one line (`App::preset_key` makes the key — the preset's
    /// path relative to the brushes root, so a moved or re-installed copy
    /// keeps its sizes).
    ///
    /// ONLY sub tools whose size the user actually MOVED off the preset's own
    /// size are in here. An untouched sub tool has no entry and keeps seeding
    /// from its preset, which is what lets a preset update change its default
    /// size (docs/CODE-MAP.md: the preset's size is the DEFAULT, never a
    /// ceiling). "Reset to preset" deletes the entry rather than writing the
    /// default down, so the two states stay distinguishable.
    ///
    /// The key is `sub_tool_size_px=` — NEW, never written by any build. The
    /// old model was a 0.25..4 multiplier where `2.0` was an ordinary value,
    /// so no pre-existing key could be reinterpreted as pixels; naming the
    /// unit in the key is the guarantee (the ui.txt rename rule).
    pub sub_tool_size_px: BTreeMap<String, f32>,
    dirty: bool,
}

impl Default for UiLayout {
    fn default() -> Self {
        Self {
            left_w: 186.0,
            right_w: 208.0,
            dock_left: String::new(),
            dock_right: String::new(),
            prop_hidden: String::new(),
            win: String::new(),
            gpu_dabs: false,
            gpu_dabs_explicit: false,
            recent_fonts: Vec::new(),
            material_folders: Vec::new(),
            material_uses: String::new(),
            quick_pins: String::new(),
            workspaces: String::new(),
            workspace_current: String::new(),
            touch_gestures: 0,
            color_history: Vec::new(),
            auto_swatch: false,
            reader_page: 0,
            guides_hidden: false,
            test_stroke_hidden: false,
            gradients: String::new(),
            sub_tool_size_px: BTreeMap::new(),
            dirty: false,
        }
    }
}

/// The main window's geometry as persisted in ui.txt: physical-pixel screen
/// coordinates (`GetWindowRect`), plus whether the window was maximized —
/// x/y/w/h are the RESTORED rect in that case, so un-maximizing lands where
/// the user left it, not at 1280x860.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WinGeom {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub max: bool,
}

/// A monitor rect (left, top, right, bottom) in screen px — `(i32, i32, i32,
/// i32)` keeps `WinGeom::fits_some_monitor` testable without windowing types.
pub type ScreenRect = (i32, i32, i32, i32);

impl WinGeom {
    pub fn to_line(&self) -> String {
        format!(
            "{},{},{},{},{}",
            self.x, self.y, self.w, self.h, self.max as i32
        )
    }

    pub fn parse(s: &str) -> Option<Self> {
        let v: Vec<i32> = s
            .split(',')
            .map(|t| t.trim().parse::<i32>().ok())
            .collect::<Option<_>>()?;
        match v[..] {
            [x, y, w, h, max] => Some(Self {
                x,
                y,
                w,
                h,
                max: max != 0,
            }),
            _ => None,
        }
    }

    /// Whether the window would land on a currently-connected monitor and be
    /// of a sane size — a position saved on a monitor that has since been
    /// unplugged is dropped rather than restored off-screen.
    pub fn fits_some_monitor(&self, monitors: &[ScreenRect]) -> bool {
        const MIN_W: i32 = 320;
        const MIN_H: i32 = 240;
        let (l, t, r, b) = (self.x, self.y, self.x + self.w, self.y + self.h);
        self.w >= MIN_W
            && self.h >= MIN_H
            && monitors
                .iter()
                .any(|&(ml, mt, mr, mb)| r > ml && b > mt && l < mr && t < mb)
    }
}

impl UiLayout {
    pub fn note_widths(&mut self, left: f32, right: f32) {
        if (left - self.left_w).abs() > 0.5 || (right - self.right_w).abs() > 0.5 {
            self.left_w = left;
            self.right_w = right;
            self.dirty = true;
        }
    }

    /// The serialized dock columns, saved only when they changed.
    pub fn note_docks(&mut self, left: &str, right: &str) {
        if self.dock_left != left || self.dock_right != right {
            self.dock_left = left.to_owned();
            self.dock_right = right.to_owned();
            self.dirty = true;
        }
    }

    pub fn note_prop_hidden(&mut self, hidden: &str) {
        if self.prop_hidden != hidden {
            self.prop_hidden = hidden.to_owned();
            self.dirty = true;
        }
    }

    /// Remember the window geometry (called at drag/resize end and at exit;
    /// a crash loses at most the last unfinished drag).
    pub fn note_win(&mut self, line: &str) {
        if self.win != line {
            self.win = line.to_owned();
            self.dirty = true;
        }
    }

    /// The GPU dab switch (View menu). Saved only when it changed — and
    /// reaching it AT ALL is the user choosing, which is what makes the key
    /// explicit from here on (`to_body` writes it only then).
    pub fn note_gpu_dabs(&mut self, on: bool) {
        if self.gpu_dabs != on || !self.gpu_dabs_explicit {
            self.gpu_dabs = on;
            self.gpu_dabs_explicit = true;
            self.dirty = true;
        }
    }

    /// The font list's recently-used row (round 34). Saved only on change.
    pub fn note_recent_fonts(&mut self, fonts: &[String]) {
        if self.recent_fonts != fonts {
            self.recent_fonts = fonts.to_vec();
            self.dirty = true;
        }
    }

    /// Material bank edits (folders or use counters) — saved only on
    /// change, like the font row above.
    /// Workspaces + the current name (round 63).
    pub fn note_workspaces(&mut self, ws: &str, current: &str) {
        if self.workspaces != ws || self.workspace_current != current {
            self.workspaces = ws.to_owned();
            self.workspace_current = current.to_owned();
            self.dirty = true;
        }
    }

    /// Quick Access pins (round 59) — saved only on change.
    pub fn note_quick_pins(&mut self, pins: &str) {
        if self.quick_pins != pins {
            self.quick_pins = pins.to_owned();
            self.dirty = true;
        }
    }

    pub fn note_materials(&mut self, folders: &[String], uses: &str) {
        let f: Vec<String> = folders.to_vec();
        if self.material_folders != f || self.material_uses != uses {
            self.material_folders = f;
            self.material_uses = uses.to_owned();
            self.dirty = true;
        }
    }

    /// The Recent colour strip (CO-042) — saved only on change, like the
    /// font row above.
    pub fn note_color_history(&mut self, hex: &[String]) {
        if self.color_history != hex {
            self.color_history = hex.to_vec();
            self.dirty = true;
        }
    }

    /// The auto-register-picks switch (CO-023). Saved only on change.
    pub fn note_auto_swatch(&mut self, on: bool) {
        if self.auto_swatch != on {
            self.auto_swatch = on;
            self.dirty = true;
        }
    }

    /// Reader v2: remember the last-read page (ui.txt `reader_page=`).
    pub fn note_reader_page(&mut self, page: usize) {
        if self.reader_page != page {
            self.reader_page = page;
            self.dirty = true;
        }
    }

    /// CV-041: the crop-mark / margin switch (View menu). Saved only on
    /// change, like the gpu-dabs switch above.
    pub fn note_guides_hidden(&mut self, hidden: bool) {
        if self.guides_hidden != hidden {
            self.guides_hidden = hidden;
            self.dirty = true;
        }
    }

    /// The brush panel's test-stroke fold. Saved only on change, like the
    /// two switches above.
    pub fn note_test_stroke_hidden(&mut self, hidden: bool) {
        if self.test_stroke_hidden != hidden {
            self.test_stroke_hidden = hidden;
            self.dirty = true;
        }
    }

    /// `G-011`/`G-012`: the gradient set, as `GradientSet::to_json`.
    pub fn note_gradients(&mut self, json: &str) {
        if self.gradients != json {
            self.gradients = json.to_owned();
            self.dirty = true;
        }
    }

    /// Remember one sub tool's brush size, or forget it. `Some(px)` is a size
    /// the user moved off the preset's own; `None` means "back to the preset"
    /// and DELETES the entry — writing the default down would freeze it, and
    /// a preset whose shipped size changes has to be able to move an
    /// untouched sub tool with it.
    pub fn note_sub_tool_size(&mut self, key: &str, px: Option<f32>) {
        let changed = match px {
            Some(px) => self.sub_tool_size_px.insert(key.to_owned(), px) != Some(px),
            None => self.sub_tool_size_px.remove(key).is_some(),
        };
        self.dirty |= changed;
    }

    pub fn save_if_dirty(&mut self) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        let Some(p) = layout_path() else { return };
        let _ = std::fs::write(p, self.to_body());
    }

    /// The full ui.txt content (one k=v per line — keys are shipped API).
    /// Exactly what `save_if_dirty` writes: the save half of the seam
    /// `from_body` completes.
    pub(super) fn to_body(&self) -> String {
        format!(
            "left_w={:.0}\nright_w={:.0}\ndock_left={}\ndock_right={}\nprop_hidden={}\nwin={}\n{}recent_fonts={}\nmaterial_folders={}\nmaterial_uses={}\nquick_pins={}\nworkspaces={}\nworkspace_current={}\ntouch_gestures={}\ncolor_history={}\nauto_swatch={}\nreader_page={}\nguides_hidden={}\ntest_stroke_hidden={}\ngradients={}\nsub_tool_size_px={}\n",
            self.left_w,
            self.right_w,
            self.dock_left,
            self.dock_right,
            self.prop_hidden,
            self.win,
            // ABSENT unless the user actually chose: the tri-state IS the
            // absence of this key (`gpu_dabs_explicit`), so writing it
            // unconditionally forged a choice on the very first clean exit
            // — after which `resolve_auto` saw an explicit "off", never
            // spawned the measurement child, and the measured default
            // could never happen on that machine again. This shipped: the
            // owner's own ui.txt carried `gpu_dabs=0` he never typed.
            if self.gpu_dabs_explicit {
                format!("gpu_dabs={}\n", self.gpu_dabs as u8)
            } else {
                String::new()
            },
            serde_json::to_string(&self.recent_fonts).unwrap_or_default(),
            serde_json::to_string(&self.material_folders).unwrap_or_default(),
            self.material_uses,
            self.quick_pins.replace('\n', ""),
            self.workspaces.replace('\n', ""),
            self.workspace_current.replace('\n', ""),
            self.touch_gestures,
            self.color_history.join(","),
            self.auto_swatch as u8,
            self.reader_page,
            self.guides_hidden as u8,
            self.test_stroke_hidden as u8,
            self.gradients.replace('\n', ""),
            serde_json::to_string(&self.sub_tool_size_px).unwrap_or_default(),
        )
    }

    /// Apply one `key=value` line (unknown keys are ignored so ui.txt from a
    /// newer build survives an older exe).
    fn apply_kv(&mut self, line: &str) {
        let Some((k, v)) = line.split_once('=') else {
            return;
        };
        match k.trim() {
            "left_w" => self.left_w = v.trim().parse().unwrap_or(self.left_w),
            "right_w" => self.right_w = v.trim().parse().unwrap_or(self.right_w),
            // One line each — JSON without newlines (serde_json compact).
            "dock_left" if !line.contains('\n') => self.dock_left = v.to_owned(),
            "dock_right" if !line.contains('\n') => self.dock_right = v.to_owned(),
            "prop_hidden" => self.prop_hidden = v.trim().to_owned(),
            "win" => self.win = v.trim().to_owned(),
            "quick_pins" => self.quick_pins = v.trim().to_owned(),
            "workspaces" => self.workspaces = v.trim().to_owned(),
            "workspace_current" => self.workspace_current = v.trim().to_owned(),
            // `reader_page` only: a pre-rename `reader_last=` held a SCREEN
            // index, so it is deliberately unknown here and ignored.
            "reader_page" => self.reader_page = v.trim().parse().unwrap_or(self.reader_page),
            // Junk degrades to 0 (all gestures off), never to "all on".
            "touch_gestures" => self.touch_gestures = v.trim().parse().unwrap_or(0),
            // Comma-joined `#rrggbb`. Entries that are not colours are kept
            // as written here and dropped where they are decoded, so one
            // fat-fingered edit costs that colour and nothing else.
            "color_history" => {
                self.color_history = v
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            // `1` turns it on, anything else off — the gpu_dabs rule, and
            // for the same reason: junk must degrade to the quiet default.
            "auto_swatch" => self.auto_swatch = v.trim() == "1",
            // CV-041: only `1` hides. Junk, an older build's ui.txt and an
            // absent key all leave the guides SHOWN — the direction that
            // cannot lose a manga artist's crop marks.
            "guides_hidden" => self.guides_hidden = v.trim() == "1",
            // Same direction of failure: only `1` folds the test stroke away.
            "test_stroke_hidden" => self.test_stroke_hidden = v.trim() == "1",
            // `1` requests on, anything else off — the round-32 rule,
            // restored by the round-34 revert (default-off until TODO #0.1
            // + an on-hardware benchmark number justify the flip).
            "gpu_dabs" => {
                self.gpu_dabs = v.trim() == "1";
                self.gpu_dabs_explicit = true;
            }
            // JSON string array, one line (like the dock columns).
            "recent_fonts" if !line.contains('\n') => {
                if let Ok(f) = serde_json::from_str::<Vec<String>>(v) {
                    self.recent_fonts = f;
                }
            }
            // TRIAGE 133: user material folders (JSON string array) and
            // use counters (JSON map path→count).
            "material_folders" if !line.contains('\n') => {
                if let Ok(f) = serde_json::from_str::<Vec<String>>(v) {
                    self.material_folders = f;
                }
            }
            "material_uses" if !line.contains('\n') => {
                self.material_uses = v.to_owned();
            }
            // `G-011`: the gradient set, one JSON line.
            "gradients" if !line.contains('\n') => self.gradients = v.to_owned(),
            // Per-sub-tool sizes, JSON map preset key → canvas px. Entries
            // outside the Size control's own range (and NaN) are dropped
            // here rather than clamped: a number this build cannot mean is a
            // hand-edit or a newer build's, and seeding from the preset is
            // the honest answer to both. A malformed line costs the map and
            // nothing else — every sub tool then starts at its preset size.
            "sub_tool_size_px" if !line.contains('\n') => {
                if let Ok(m) = serde_json::from_str::<BTreeMap<String, f32>>(v) {
                    self.sub_tool_size_px = m
                        .into_iter()
                        .filter(|(_, px)| {
                            px.is_finite()
                                && (crate::cmd::SIZE_PX_MIN..=crate::cmd::SIZE_PX_MAX).contains(px)
                        })
                        .collect();
                }
            }
            _ => {}
        }
    }

    pub(super) fn load() -> Self {
        match layout_path().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(text) => Self::from_body(&text),
            None => Self::default(),
        }
    }

    /// Everything `load` does except reading the file — so a test can drive
    /// the real save→load path (`to_body` → this) without touching the ui.txt
    /// beside the test exe, which the parallel runner shares.
    pub(super) fn from_body(text: &str) -> Self {
        let mut me = Self::default();
        for line in text.lines() {
            me.apply_kv(line);
        }
        me.left_w = me.left_w.clamp(150.0, 420.0);
        me.right_w = me.right_w.clamp(160.0, 420.0);
        me.dirty = false;
        me
    }
}

fn layout_path() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("ui.txt"))
}

/// The saved window geometry, read straight from ui.txt — startup needs it
/// before the window (and with it the App/UiLayout) exists.
pub fn peek_win() -> Option<WinGeom> {
    let text = layout_path().and_then(|p| std::fs::read_to_string(p).ok())?;
    text.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == "win")
            .then(|| WinGeom::parse(v.trim()))
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_geom_line_roundtrip() {
        let g = WinGeom {
            x: -1920,
            y: 37,
            w: 1280,
            h: 860,
            max: true,
        };
        assert_eq!(WinGeom::parse(&g.to_line()), Some(g));
        let g2 = WinGeom {
            x: 5,
            y: 6,
            w: 700,
            h: 500,
            max: false,
        };
        assert_eq!(WinGeom::parse(&g2.to_line()), Some(g2));
    }

    /// No MULTIPLIER-era key is ever read. Brush size became an ABSOLUTE px
    /// diameter (it was a 0.25..4 multiplier), and `2.0` is a plausible value
    /// under either meaning — so a stale `2.0` must never be read as 2 px.
    /// Sizes DO persist now, but only under the new `sub_tool_size_px=` key,
    /// which names its unit and which no older build ever wrote: these four
    /// hand-written or older-build spellings stay unknown keys forever.
    #[test]
    fn no_multiplier_era_key_is_ever_read() {
        const OLD: [&str; 4] = ["size", "brush_size", "tool_size", "size_multiplier"];
        let mut me = UiLayout::default();
        let before = me.to_body();
        for k in OLD {
            me.apply_kv(&format!("{k}=2.0"));
        }
        assert_eq!(before, me.to_body(), "stale size keys must change nothing");
        assert!(
            me.sub_tool_size_px.is_empty(),
            "and none of them may seed a sub tool's size"
        );
        for k in OLD {
            assert!(
                !me.to_body().lines().any(|l| l.starts_with(&format!("{k}="))),
                "ui.txt must not persist a brush size under `{k}`"
            );
        }
    }

    /// Good-first-issue #1: per-sub-tool sizes survive save→load through the
    /// real body, only entries the user MOVED are written, "back to the
    /// preset" deletes rather than freezes, and junk degrades to "no
    /// override" (= seed from the preset), never to a bogus pixel size.
    #[test]
    fn sub_tool_sizes_roundtrip_through_the_body() {
        let mut me = UiLayout::default();
        assert!(me.sub_tool_size_px.is_empty(), "nothing until the user acts");
        me.note_sub_tool_size("csp/g-pen.myb", Some(37.5));
        assert!(me.dirty);
        // An untouched sub tool is an ABSENT entry, not a written default.
        me.note_sub_tool_size("mypaint/pen.myb", None);
        assert_eq!(me.sub_tool_size_px.len(), 1);

        let body = me.to_body();
        assert!(
            body.contains("\nsub_tool_size_px={\"csp/g-pen.myb\":37.5}\n"),
            "one JSON line, `/`-separated relative keys: {body}"
        );

        // The real load path (file read aside).
        let back = UiLayout::from_body(&body);
        assert_eq!(back.sub_tool_size_px.get("csp/g-pen.myb"), Some(&37.5));
        assert_eq!(back.sub_tool_size_px.get("mypaint/pen.myb"), None);
        assert!(!back.dirty, "a fresh load is not a pending save");

        // Re-noting the same size must not re-dirty a clean layout...
        let mut back = back;
        back.note_sub_tool_size("csp/g-pen.myb", Some(37.5));
        assert!(!back.dirty);
        // ...and "back to the preset" removes the entry (it does not write
        // the preset's size down, which would freeze a preset update out).
        back.note_sub_tool_size("csp/g-pen.myb", None);
        assert!(back.dirty && back.sub_tool_size_px.is_empty());
        let cleared = UiLayout::from_body(&back.to_body());
        assert!(cleared.sub_tool_size_px.is_empty());

        // Junk: a mangled line costs the map, and out-of-range or non-finite
        // entries are dropped so no brush is ever seeded a nonsense size.
        let mut junk = UiLayout::default();
        junk.apply_kv("sub_tool_size_px=not json");
        assert!(junk.sub_tool_size_px.is_empty());
        junk.apply_kv(r#"sub_tool_size_px={"a.myb":0,"b.myb":999999,"d.myb":24}"#);
        assert_eq!(junk.sub_tool_size_px.len(), 1, "only the sane entry lands");
        assert_eq!(junk.sub_tool_size_px.get("d.myb"), Some(&24.0));
    }

    #[test]
    fn win_geom_parse_junk() {
        assert_eq!(WinGeom::parse(""), None);
        assert_eq!(WinGeom::parse("1,2,3"), None);
        assert_eq!(WinGeom::parse("a,b,c,d,e"), None);
        assert_eq!(WinGeom::parse("1,2,3,4,5,6"), None);
    }

    #[test]
    fn win_geom_fits_monitor_rules() {
        let monitors = [(0, 0, 1920, 1080), (1920, 0, 3840, 1080)];
        let on_second = WinGeom {
            x: 2000,
            y: 10,
            w: 800,
            h: 600,
            max: false,
        };
        let straddling = WinGeom {
            x: 1800,
            y: 0,
            w: 400,
            h: 600,
            max: false,
        };
        assert!(on_second.fits_some_monitor(&monitors));
        assert!(straddling.fits_some_monitor(&monitors));
        // Saved on a monitor that is no longer connected.
        let gone = WinGeom {
            x: -1920,
            y: 0,
            w: 800,
            h: 600,
            max: false,
        };
        assert!(!gone.fits_some_monitor(&monitors));
        // Degenerate sizes are not restored.
        let tiny = WinGeom {
            x: 0,
            y: 0,
            w: 100,
            h: 600,
            max: false,
        };
        assert!(!tiny.fits_some_monitor(&monitors));
        assert!(!on_second.fits_some_monitor(&[]));
    }

    /// The gpu-dabs switch persists through the ui.txt body: default OFF,
    /// only `1` turns it on, junk and absent keys leave the default — the
    /// round-32 rule, RESTORED by the round-34 revert (the same-day
    /// default-ON flip — owner instruction — was superseded by the
    /// auditor's counter-recommendation relayed by the owner; re-flip
    /// criteria in DECISIONS 8.9: TODO #0.1 landed + a benchmark number on
    /// the owner's hardware). main.rs still ANDs the request with
    /// `gpu_dabs_supported()`, so unsupported adapters stay CPU regardless.
    #[test]
    fn gpu_dabs_roundtrips_through_the_body() {
        let mut me = UiLayout::default();
        assert!(!me.gpu_dabs, "default off (the round-34 revert)");
        me.note_gpu_dabs(true);
        assert!(me.gpu_dabs);
        let body = me.to_body();
        assert!(body.contains("\ngpu_dabs=1\n"), "{body}");

        let mut back = UiLayout::default();
        for line in body.lines() {
            back.apply_kv(line);
        }
        assert!(back.gpu_dabs, "the saved body must restore the switch");
        // `note` with the same value must not re-dirty a clean layout.
        back.dirty = false;
        back.note_gpu_dabs(true);
        assert!(!back.dirty);

        // Absent key (any ui.txt without it) keeps the default: off.
        let mut fresh = UiLayout::default();
        fresh.apply_kv("left_w=200");
        assert!(!fresh.gpu_dabs, "no gpu_dabs key = off");

        let mut junk = UiLayout::default();
        junk.apply_kv("gpu_dabs=0");
        assert!(!junk.gpu_dabs);
        junk.apply_kv("gpu_dabs=yes"); // anything but "1" stays off
        assert!(!junk.gpu_dabs);
        junk.apply_kv("gpu_dabs=something-newer");
        assert!(!junk.gpu_dabs, "unknown values degrade to off, never on");
    }

    /// The tri-state survives a save. A layout the user never touched must
    /// write NO `gpu_dabs=` line, because the absence of the key is the
    /// only thing telling startup "he never chose — you may measure".
    ///
    /// Failed against the old code, which wrote `gpu_dabs=0` every time:
    /// one clean exit forged an explicit "off", startup then honoured that
    /// forged choice, the measurement child was never spawned again, and
    /// the measured GPU default became unreachable on that machine — with
    /// nothing anywhere saying so.
    #[test]
    fn an_untouched_gpu_dabs_key_is_not_written_back() {
        let fresh = UiLayout::default();
        let body = fresh.to_body();
        assert!(
            !body.contains("gpu_dabs="),
            "an unchosen switch must not be written down: {body}"
        );
        // …and re-reading that body leaves the tri-state unset, so the
        // measurement is still allowed to decide.
        let mut back = UiLayout::default();
        for line in body.lines() {
            back.apply_kv(line);
        }
        assert!(!back.gpu_dabs_explicit, "a round trip must not forge a choice");

        // Using the View-menu toggle IS the choice — even when it picks the
        // value the layout already held (turning it off while it is off is
        // still "I chose off", and must stop the auto path).
        let mut chose_off = UiLayout::default();
        chose_off.note_gpu_dabs(false);
        assert!(chose_off.gpu_dabs_explicit);
        assert!(chose_off.dirty, "the choice must reach the disk");
        assert!(chose_off.to_body().contains("\ngpu_dabs=0\n"));
    }

    /// Round 34: the font list's recently-used row persists as a one-line
    /// JSON array (JP names intact), junk leaves the current list alone,
    /// and an unchanged list does not re-dirty.
    #[test]
    fn recent_fonts_roundtrip_through_the_body() {
        let mut me = UiLayout::default();
        assert!(me.recent_fonts.is_empty());
        let fonts: Vec<String> = ["源暎アンチックv5", "Meiryo", "Yu Gothic"]
            .map(|s| s.to_string())
            .to_vec();
        me.note_recent_fonts(&fonts);
        let body = me.to_body();
        assert!(
            body.lines()
                .any(|l| l.starts_with("recent_fonts=") && l.contains("源暎")),
            "one line, JP intact: {body}"
        );

        let mut back = UiLayout::default();
        for line in body.lines() {
            back.apply_kv(line);
        }
        assert_eq!(back.recent_fonts, fonts, "the body restores the list");

        // Junk / non-array values leave the current list alone.
        let mut junk = UiLayout::default();
        junk.apply_kv("recent_fonts=[not json");
        assert!(junk.recent_fonts.is_empty());
        junk.apply_kv("recent_fonts=\"scalar\"");
        assert!(junk.recent_fonts.is_empty());

        // `note` with the same list must not re-dirty a clean layout.
        back.dirty = false;
        back.note_recent_fonts(&fonts);
        assert!(!back.dirty);
    }

    /// CO-042/CO-023: the Recent colour strip and the auto-register switch
    /// survive the ui.txt body, and a mangled line costs at most the
    /// colours in it — never the file, never a surprise switch-on.
    #[test]
    fn color_history_and_auto_swatch_roundtrip_through_the_body() {
        let mut me = UiLayout::default();
        assert!(me.color_history.is_empty());
        assert!(!me.auto_swatch, "picks do not join the set unless asked");

        let hist: Vec<String> = ["#000000", "#4f8cd2", "#ffffff"]
            .map(|s| s.to_string())
            .to_vec();
        me.note_color_history(&hist);
        me.note_auto_swatch(true);
        let body = me.to_body();
        assert!(
            body.contains("\ncolor_history=#000000,#4f8cd2,#ffffff\n"),
            "one line, newest first: {body}"
        );
        assert!(body.contains("\nauto_swatch=1\n"), "{body}");

        let mut back = UiLayout::default();
        for line in body.lines() {
            back.apply_kv(line);
        }
        assert_eq!(back.color_history, hist);
        assert!(back.auto_swatch);

        // `note` with the same values must not re-dirty a clean layout.
        back.dirty = false;
        back.note_color_history(&hist);
        back.note_auto_swatch(true);
        assert!(!back.dirty);

        // An emptied history is a real state and must persist as one.
        back.note_color_history(&[]);
        assert!(back.dirty);
        let mut cleared = UiLayout::default();
        for line in back.to_body().lines() {
            cleared.apply_kv(line);
        }
        assert!(cleared.color_history.is_empty());

        // Junk: blanks are dropped, the rest is carried and rejected later.
        let mut junk = UiLayout::default();
        junk.apply_kv("color_history=,,#00ff00 , bananas,");
        assert_eq!(junk.color_history, ["#00ff00", "bananas"]);
        junk.apply_kv("auto_swatch=yes");
        assert!(!junk.auto_swatch, "unknown values degrade to off, never on");
    }

    /// CV-041: the crop-mark / margin switch persists through the ui.txt
    /// body — and, the half that matters, it degrades towards SHOWN. An
    /// absent key (every ui.txt written before this build), junk, and
    /// anything that is not `1` all leave a working artist's guides on the
    /// page; only a deliberate `1` hides them.
    #[test]
    fn guides_hidden_roundtrips_and_degrades_to_shown() {
        let mut me = UiLayout::default();
        assert!(!me.guides_hidden, "default: guides shown");
        me.note_guides_hidden(true);
        assert!(me.guides_hidden);
        let body = me.to_body();
        assert!(body.contains("\nguides_hidden=1\n"), "{body}");

        let mut back = UiLayout::default();
        for line in body.lines() {
            back.apply_kv(line);
        }
        assert!(back.guides_hidden, "the saved body must restore the switch");
        // `note` with the same value must not re-dirty a clean layout.
        back.dirty = false;
        back.note_guides_hidden(true);
        assert!(!back.dirty);
        back.note_guides_hidden(false);
        assert!(back.dirty && !back.guides_hidden);

        // An older build's ui.txt has no such key: guides stay shown.
        let mut fresh = UiLayout::default();
        fresh.apply_kv("left_w=200");
        assert!(!fresh.guides_hidden, "no guides_hidden key = shown");

        let mut junk = UiLayout::default();
        for line in ["guides_hidden=0", "guides_hidden=yes", "guides_hidden="] {
            junk.apply_kv(line);
            assert!(!junk.guides_hidden, "{line} must not hide the guides");
        }
    }

    /// The brush panel's test-stroke fold rides the same rules: it round-trips
    /// through the body, and every way of being unreadable leaves the strip
    /// SHOWN — a preview nobody can find is the bug the feature exists to fix.
    #[test]
    fn test_stroke_fold_roundtrips_and_degrades_to_shown() {
        let mut me = UiLayout::default();
        assert!(!me.test_stroke_hidden, "default: the strip is shown");
        me.note_test_stroke_hidden(true);
        let body = me.to_body();
        assert!(body.contains("\ntest_stroke_hidden=1\n"), "{body}");

        let mut back = UiLayout::default();
        for line in body.lines() {
            back.apply_kv(line);
        }
        assert!(back.test_stroke_hidden, "the saved body restores the fold");
        back.dirty = false;
        back.note_test_stroke_hidden(true);
        assert!(!back.dirty, "an unchanged fold must not re-dirty ui.txt");

        let mut junk = UiLayout::default();
        for line in ["test_stroke_hidden=0", "test_stroke_hidden=no", "left_w=200"] {
            junk.apply_kv(line);
            assert!(!junk.test_stroke_hidden, "{line} must not fold the strip");
        }
    }
}
