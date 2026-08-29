//! The gradient RAMP — what a gradient IS, apart from where it is drawn.
//!
//! `Document::paint_gradient_ramp` (destructive) and
//! `fill_layer::FillKind::Gradient` (live) both evaluate this one type, so
//! a ramp authored in the Tool Property panel behaves identically whether
//! it is baked into pixels or left as a live gradient layer's parameters.
//!
//! The model is CSP's colour bar: two END colours pinned at the ends of the
//! drag plus up to [`MAX_MID`] interior stops with their own position and
//! opacity (`G-008`/`G-013`/`G-014`), and a small bag of options —
//! **edge process** (what happens outside the dragged span, `G-004`),
//! **flip** (`G-002`), **dithering** (`G-005`), **start from centre**
//! (`G-006`), **mixing mode** (`G-009`) and **mixing rate** (`G-015`).
//!
//! Every option's default is the behaviour that shipped before this module
//! existed — a clamped, unflipped, undithered, linear sRGB ramp — which is
//! what lets `#[serde(default)]` load older files pixel-identically.
//!
//! Capacity is fixed rather than a `Vec` on purpose: `FillKind` is `Copy`
//! and is compared by value as the live layer's re-derive stamp. A heap
//! stop list would cost both.

use serde::{Deserialize, Serialize};

/// How many INTERIOR stops a ramp can carry (the two ends are separate and
/// always present, so a ramp is at most `MAX_MID + 2` colours).
pub const MAX_MID: usize = 6;

/// One interior colour stop.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradStop {
    /// Position along the ramp, 0 = the drag start, 1 = the drag end.
    pub pos: f32,
    /// Straight (NOT premultiplied) RGBA — the alpha IS the stop's opacity.
    pub color: [f32; 4],
}

impl Default for GradStop {
    fn default() -> Self {
        Self {
            pos: 0.5,
            color: [0.5, 0.5, 0.5, 1.0],
        }
    }
}

/// The interior stops, kept sorted by position. Fixed capacity so the whole
/// ramp stays `Copy`; `n` beyond `MAX_MID` is impossible through the API but
/// a hand-edited file could claim it, so every reader clamps.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MidStops {
    n: u8,
    s: [GradStop; MAX_MID],
}

impl Default for MidStops {
    fn default() -> Self {
        Self {
            n: 0,
            s: [GradStop::default(); MAX_MID],
        }
    }
}

impl MidStops {
    pub fn len(&self) -> usize {
        (self.n as usize).min(MAX_MID)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() >= MAX_MID
    }

    pub fn as_slice(&self) -> &[GradStop] {
        &self.s[..self.len()]
    }

    pub fn get(&self, i: usize) -> Option<&GradStop> {
        self.as_slice().get(i)
    }

    /// Mutable access. The caller may move `pos`; call [`Self::resort`]
    /// afterwards (the editor does it when the drag ends, so a stop dragged
    /// past its neighbour does not renumber itself mid-gesture).
    pub fn get_mut(&mut self, i: usize) -> Option<&mut GradStop> {
        let n = self.len();
        self.s[..n].get_mut(i)
    }

    /// Insert a stop in position order. Returns its index, or `None` when
    /// the ramp is full.
    pub fn insert(&mut self, stop: GradStop) -> Option<usize> {
        if self.is_full() {
            return None;
        }
        let n = self.len();
        let at = self.s[..n]
            .iter()
            .position(|s| s.pos > stop.pos)
            .unwrap_or(n);
        self.s[at..=n].rotate_right(1);
        self.s[at] = stop;
        self.n = (n + 1) as u8;
        Some(at)
    }

    /// Delete stop `i` (out of range = no-op). Returns whether it went.
    pub fn remove(&mut self, i: usize) -> bool {
        let n = self.len();
        if i >= n {
            return false;
        }
        self.s[i..n].rotate_left(1);
        self.n = (n - 1) as u8;
        true
    }

    /// Re-sort after positions were edited in place. Returns where the stop
    /// that was at `track` ended up, so a drag can keep hold of it.
    pub fn resort(&mut self, track: usize) -> usize {
        let n = self.len();
        if n < 2 || track >= n {
            return track;
        }
        let moved = self.s[track];
        // Insertion sort: n <= MAX_MID, and it is stable enough that equal
        // positions keep their authored order.
        for i in 1..n {
            let mut j = i;
            while j > 0 && self.s[j - 1].pos > self.s[j].pos {
                self.s.swap(j - 1, j);
                j -= 1;
            }
        }
        self.s[..n]
            .iter()
            .position(|s| *s == moved)
            .unwrap_or(track)
    }
}

/// `G-004` — what the ramp does OUTSIDE the length that was dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EdgeProcess {
    /// CSP "Do not repeat": hold the end colour forever. The behaviour that
    /// shipped before edge process existed, so it stays the default.
    #[default]
    Clamp,
    /// Tile the ramp end-to-end.
    Repeat,
    /// Ping-pong: the ramp mirrors on every repeat.
    Reverse,
    /// CSP "Do not draw": leave everything outside the drag untouched.
    Blank,
}

impl EdgeProcess {
    pub fn label(self) -> &'static str {
        match self {
            EdgeProcess::Clamp => "Do not repeat",
            EdgeProcess::Repeat => "Repeat",
            EdgeProcess::Reverse => "Reverse",
            EdgeProcess::Blank => "Do not draw",
        }
    }

    pub const ALL: [EdgeProcess; 4] = [
        EdgeProcess::Clamp,
        EdgeProcess::Repeat,
        EdgeProcess::Reverse,
        EdgeProcess::Blank,
    ];
}

/// `G-009` — the space two stops are mixed in, and `G-010`'s brightness
/// correction beside it.
///
/// Both LIVE in [`crate::mix`] now (triage row 167): the brush needed the
/// same choice and a second copy of Oklab was the wrong way to give it one.
/// Re-exported here because `G-009` is a gradient row and every caller in
/// the tree names it `gradient::MixMode`.
pub use crate::mix::{MAX_BRIGHT, MixMode};

/// Everything about a ramp that is not a colour stop. Every field's default
/// is the pre-existing behaviour — see the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct RampOpts {
    /// `G-004`.
    #[serde(default)]
    pub edge: EdgeProcess,
    /// `G-002`: reverse the ramp without re-dragging it.
    #[serde(default)]
    pub flip: bool,
    /// `G-005`: ordered noise inside the ramp so an 8-bit export does not
    /// print visible bands.
    #[serde(default)]
    pub dither: bool,
    /// `G-006`: the drag START is the middle of the ramp, not one end.
    #[serde(default)]
    pub from_center: bool,
    /// `G-009`.
    #[serde(default)]
    pub mix: MixMode,
    /// `G-010`: 0..=[`MAX_BRIGHT`], Perceptual only. 0 = a plain Oklab lerp;
    /// higher lifts the middle of the ramp back toward the brighter end's
    /// lightness, so blue→yellow stops sagging through a dark middle.
    #[serde(default)]
    pub bright: u8,
    /// `G-015`: -1..1 bias on how fast one stop blends into the next.
    /// 0 = linear; positive holds the earlier colour longer.
    #[serde(default)]
    pub curve: f32,
}

/// An 8×8 ordered (Bayer) matrix. Ordered, not random: a print-bound page
/// wants a stable pattern, and re-deriving a live layer must not reshuffle
/// its noise every frame.
#[rustfmt::skip]
const BAYER8: [u8; 64] = [
     0, 32,  8, 40,  2, 34, 10, 42,
    48, 16, 56, 24, 50, 18, 58, 26,
    12, 44,  4, 36, 14, 46,  6, 38,
    60, 28, 52, 20, 62, 30, 54, 22,
     3, 35, 11, 43,  1, 33,  9, 41,
    51, 19, 59, 27, 49, 17, 57, 25,
    15, 47,  7, 39, 13, 45,  5, 37,
    63, 31, 55, 23, 61, 29, 53, 21,
];

/// -0.5..+0.5, one 8-bit level of dither amplitude at canvas `(x, y)`.
fn dither_offset(x: i32, y: i32) -> f32 {
    let i = (y.rem_euclid(8) * 8 + x.rem_euclid(8)) as usize;
    (BAYER8[i] as f32 + 0.5) / 64.0 - 0.5
}

/// The whole ramp, ready to evaluate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramp {
    /// The colour at position 0 (the drag start).
    pub from: [f32; 4],
    /// The colour at position 1 (the drag end).
    pub to: [f32; 4],
    pub mid: MidStops,
    pub opts: RampOpts,
}

impl Ramp {
    pub fn new(from: [f32; 4], to: [f32; 4], mid: MidStops, opts: RampOpts) -> Self {
        Self {
            from,
            to,
            mid,
            opts,
        }
    }

    /// A plain two-colour ramp with every option at its default.
    pub fn two(from: [f32; 4], to: [f32; 4]) -> Self {
        Self::new(from, to, MidStops::default(), RampOpts::default())
    }

    /// The AFFINE half of the mapping: centre-out and flip, before the edge
    /// process. Kept separate because being affine is what lets
    /// [`Self::draws_span`] bound a whole tile from its corners.
    fn map(&self, u: f32) -> f32 {
        // Centre first: it redefines what "outside the drag" means, so the
        // edge process must see the already-recentred coordinate.
        let u = if self.opts.from_center {
            0.5 + 0.5 * u
        } else {
            u
        };
        if self.opts.flip { 1.0 - u } else { u }
    }

    /// Can this ramp put ink anywhere in a span of projections? Only
    /// "do not draw" can answer no — the caller uses it to skip whole tiles
    /// without allocating them (an allocated tile is an undo snapshot).
    pub fn draws_span(&self, u0: f32, u1: f32) -> bool {
        if self.opts.edge != EdgeProcess::Blank {
            return true;
        }
        let (a, b) = (self.map(u0), self.map(u1));
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        hi >= 0.0 && lo <= 1.0
    }

    /// Map the raw projection `u` (0 at the drag start, 1 at the drag end,
    /// unbounded outside) to the ramp parameter in 0..=1. `None` means
    /// "do not draw" — the pixel is left alone.
    pub fn param(&self, u: f32) -> Option<f32> {
        let u = self.map(u);
        if !u.is_finite() {
            return Some(0.0);
        }
        match self.opts.edge {
            EdgeProcess::Clamp => Some(u.clamp(0.0, 1.0)),
            EdgeProcess::Repeat => Some(u - u.floor()),
            EdgeProcess::Reverse => {
                let m = u.rem_euclid(2.0);
                Some(if m > 1.0 { 2.0 - m } else { m })
            }
            EdgeProcess::Blank => (0.0..=1.0).contains(&u).then_some(u),
        }
    }

    /// The straight-RGBA colour at ramp parameter `t` (0..=1).
    pub fn color_at(&self, t: f32) -> [f32; 4] {
        let t = t.clamp(0.0, 1.0);
        let mid = self.mid.as_slice();
        // Walk the stops low→high, ends included, and find the bracket.
        let (mut p0, mut c0) = (0.0f32, self.from);
        let (mut p1, mut c1) = (1.0f32, self.to);
        for s in mid {
            let sp = s.pos.clamp(0.0, 1.0);
            if sp <= t && sp >= p0 {
                p0 = sp;
                c0 = s.color;
            }
        }
        for s in mid.iter().rev() {
            let sp = s.pos.clamp(0.0, 1.0);
            if sp > t && sp <= p1 {
                p1 = sp;
                c1 = s.color;
            }
        }
        if p1 <= p0 {
            return c1;
        }
        let s = bias((t - p0) / (p1 - p0), self.opts.curve);
        crate::mix::mix_rgba(self.opts.mix, c0, c1, s, self.opts.bright)
    }

    /// Projection → colour at canvas pixel `(x, y)`, dithering included.
    /// `None` = do not draw.
    pub fn eval(&self, u: f32, x: i32, y: i32) -> Option<[f32; 4]> {
        let t = self.param(u)?;
        let mut c = self.color_at(t);
        // Dither only INSIDE the ramp. A clamped tail is a flat field the
        // artist asked for; stippling it would put noise in the very region
        // "fade to transparent" is supposed to leave clean.
        if self.opts.dither && t > 0.0 && t < 1.0 {
            let d = dither_offset(x, y) / 255.0;
            for v in c.iter_mut() {
                *v = (*v + d).clamp(0.0, 1.0);
            }
        }
        Some(c)
    }
}

/// `G-015`'s mixing rate: a monotone bias on 0..1. `k` in -1..1; 0 is the
/// identity, positive holds the earlier stop's colour longer.
fn bias(s: f32, k: f32) -> f32 {
    let s = s.clamp(0.0, 1.0);
    let k = k.clamp(-1.0, 1.0);
    if k.abs() < 1e-4 {
        return s;
    }
    let e = if k > 0.0 {
        1.0 + k * 3.0
    } else {
        1.0 / (1.0 - k * 3.0)
    };
    s.powf(e)
}

// --- the gradient SET (`G-011`/`G-012`/`G-016`) --------------------------

/// One saved ramp. Unlike the tool's live ramp this carries its own END
/// colours: a preset that borrowed whatever the palette held would not
/// reproduce, which is the only thing a preset is for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedRamp {
    pub name: String,
    pub from: [f32; 4],
    pub to: [f32; 4],
    #[serde(default)]
    pub mid: MidStops,
    #[serde(default)]
    pub opts: RampOpts,
}

impl NamedRamp {
    pub fn new(name: impl Into<String>, from: [f32; 4], to: [f32; 4]) -> Self {
        Self {
            name: name.into(),
            from,
            to,
            mid: MidStops::default(),
            opts: RampOpts::default(),
        }
    }

    pub fn ramp(&self) -> Ramp {
        Ramp::new(self.from, self.to, self.mid, self.opts)
    }
}

/// `G-011`/`G-012` — the saved gradient list and its CRUD. A plain `Vec`
/// with the three ops that are easy to get wrong (duplicate naming, moving
/// an item and keeping the selection on it, importing) written once and
/// tested, rather than open-coded in the panel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GradientSet {
    pub items: Vec<NamedRamp>,
}

impl GradientSet {
    /// The set a first run gets. Deliberately small and manga-shaped: the
    /// two fades an inker actually reaches for plus one soft grey.
    pub fn starter() -> Self {
        let black = [0.0, 0.0, 0.0, 1.0];
        let white = [1.0, 1.0, 1.0, 1.0];
        let clear_black = [0.0, 0.0, 0.0, 0.0];
        let mut soft = NamedRamp::new("Soft grey", white, [0.35, 0.35, 0.38, 1.0]);
        soft.opts.mix = MixMode::Perceptual;
        soft.opts.dither = true;
        Self {
            items: vec![
                NamedRamp::new("Black to white", black, white),
                NamedRamp::new("Black to transparent", black, clear_black),
                soft,
            ],
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Copy item `i` in just after itself, with a name that is free in the
    /// set. Returns the new index.
    pub fn duplicate(&mut self, i: usize) -> Option<usize> {
        let mut copy = self.items.get(i)?.clone();
        copy.name = self.free_name(&copy.name);
        self.items.insert(i + 1, copy);
        Some(i + 1)
    }

    /// `G-012`'s Up/Down. Returns where the item ended up (unchanged when
    /// it was already at the end it was pushed toward).
    pub fn move_by(&mut self, i: usize, delta: i32) -> usize {
        let n = self.items.len();
        if i >= n {
            return i;
        }
        let j = (i as i32 + delta).clamp(0, n as i32 - 1) as usize;
        if j != i {
            self.items.swap(i, j);
        }
        j
    }

    /// `<name>`, then `<name> 2`, `<name> 3`… — the first one nothing else
    /// in the set is using.
    pub fn free_name(&self, base: &str) -> String {
        let base = base.trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ');
        let base = if base.is_empty() { "Gradient" } else { base };
        if !self.items.iter().any(|it| it.name == base) {
            return base.to_string();
        }
        for n in 2..1000 {
            let cand = format!("{base} {n}");
            if !self.items.iter().any(|it| it.name == cand) {
                return cand;
            }
        }
        base.to_string()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.items).unwrap_or_else(|_| "[]".into())
    }

    pub fn from_json(s: &str) -> Self {
        Self {
            items: serde_json::from_str(s).unwrap_or_default(),
        }
    }
}

/// `G-016` — import. CSP imports `.cgs` and Photoshop `.grd`; both are
/// undocumented binary formats, so what we read instead is GIMP's `.ggr`,
/// which is a documented plain-text one that every gradient pack on the
/// internet is also published in. Segment boundaries become interior stops;
/// a ramp with more boundaries than [`MAX_MID`] keeps the earliest ones and
/// still lands its two ends, so an over-long import degrades instead of
/// failing.
///
/// Returns the ramp, or a one-line reason it is not a gradient file.
pub fn import_ggr(text: &str) -> Result<NamedRamp, String> {
    let mut lines = text.lines().map(str::trim);
    if lines.next().map(|l| l.starts_with("GIMP Gradient")) != Some(true) {
        return Err("not a GIMP gradient (.ggr) file".into());
    }
    let mut name = String::from("Imported");
    let mut header = lines.next().unwrap_or("");
    if let Some(rest) = header.strip_prefix("Name:") {
        name = rest.trim().to_string();
        header = lines.next().unwrap_or("");
    }
    let count: usize = header
        .parse()
        .map_err(|_| "no segment count after the header".to_string())?;
    // 13 numbers is the documented minimum row; later GIMP writes 15.
    let rows: Vec<Vec<f32>> = lines
        .take(count)
        .map(|l| {
            l.split_whitespace()
                .filter_map(|f| f.parse().ok())
                .collect()
        })
        .filter(|v: &Vec<f32>| v.len() >= 13)
        .collect();
    let (first, last) = match (rows.first(), rows.last()) {
        (Some(f), Some(l)) => (f, l),
        _ => return Err("no usable segments".into()),
    };
    let mut out = NamedRamp::new(
        name,
        [first[3], first[4], first[5], first[6]],
        [last[7], last[8], last[9], last[10]],
    );
    // Every interior boundary: the segment's right edge, coloured by the
    // colour that segment ends on.
    for w in rows.windows(2) {
        let stop = GradStop {
            pos: w[0][2].clamp(0.0, 1.0),
            color: [w[0][7], w[0][8], w[0][9], w[0][10]],
        };
        if out.mid.insert(stop).is_none() {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn black_to_white() -> Ramp {
        Ramp::two([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0])
    }

    /// The defaults must reproduce EXACTLY the two-colour clamped lerp that
    /// shipped before this module — this is what makes `#[serde(default)]`
    /// safe for files saved by the old build.
    #[test]
    fn defaults_are_the_old_two_colour_lerp() {
        let r = black_to_white();
        for i in 0..=40 {
            let u = -0.5 + i as f32 * 0.05;
            let t = u.clamp(0.0, 1.0);
            let c = r.eval(u, 3, 7).expect("clamp always draws");
            let want = [0.0 + (1.0 - 0.0) * t; 3];
            for k in 0..3 {
                assert_eq!(c[k], want[k], "u={u} channel {k}");
            }
            assert_eq!(c[3], 1.0);
        }
    }

    /// `G-004`, all four. Outside the dragged span is where they differ.
    #[test]
    fn edge_process_covers_the_four_cases() {
        let mut r = black_to_white();

        r.opts.edge = EdgeProcess::Clamp;
        assert_eq!(r.param(-0.4), Some(0.0));
        assert_eq!(r.param(1.9), Some(1.0));

        r.opts.edge = EdgeProcess::Repeat;
        // 1.25 tiles back to a quarter in; -0.25 wraps to three quarters.
        assert!((r.param(1.25).unwrap() - 0.25).abs() < 1e-6);
        assert!((r.param(-0.25).unwrap() - 0.75).abs() < 1e-6);

        r.opts.edge = EdgeProcess::Reverse;
        // Ping-pong: 1.25 is 0.75 on the way back, 2.25 is 0.25 again.
        assert!((r.param(1.25).unwrap() - 0.75).abs() < 1e-6);
        assert!((r.param(2.25).unwrap() - 0.25).abs() < 1e-6);

        r.opts.edge = EdgeProcess::Blank;
        assert_eq!(r.param(-0.01), None, "before the drag: untouched");
        assert_eq!(r.param(1.01), None, "after the drag: untouched");
        assert_eq!(r.param(0.5), Some(0.5), "inside still draws");
    }

    /// `G-002` flip and `G-006` start-from-centre, including the order they
    /// compose in: centring redefines the span, flip then reverses it.
    #[test]
    fn flip_and_from_center() {
        let mut r = black_to_white();
        r.opts.flip = true;
        assert_eq!(r.param(0.0), Some(1.0), "flip: the start is now the end");
        assert_eq!(r.param(0.25), Some(0.75));

        let mut r = black_to_white();
        r.opts.from_center = true;
        assert_eq!(r.param(0.0), Some(0.5), "the drag start is the middle");
        assert_eq!(r.param(1.0), Some(1.0), "the drag end is still the end");
        assert_eq!(r.param(-1.0), Some(0.0), "and it runs back the other way");

        r.opts.flip = true;
        assert_eq!(r.param(0.0), Some(0.5), "centre is fixed under a flip");
        assert_eq!(r.param(1.0), Some(0.0));
    }

    /// `G-008`/`G-013`: an interior stop bends the ramp where it sits, and
    /// insertion keeps the list sorted however it is fed.
    #[test]
    fn interior_stops_sort_and_bend_the_ramp() {
        let mut mid = MidStops::default();
        assert_eq!(
            mid.insert(GradStop {
                pos: 0.75,
                color: [1.0, 0.0, 0.0, 1.0],
            }),
            Some(0)
        );
        assert_eq!(
            mid.insert(GradStop {
                pos: 0.25,
                color: [0.0, 1.0, 0.0, 1.0],
            }),
            Some(0),
            "the earlier position lands first"
        );
        let p: Vec<f32> = mid.as_slice().iter().map(|s| s.pos).collect();
        assert_eq!(p, vec![0.25, 0.75]);

        let r = Ramp::new(
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            mid,
            RampOpts::default(),
        );
        let g = r.color_at(0.25);
        assert!(g[1] > 0.99 && g[0] < 0.01, "the green stop is exact: {g:?}");
        let red = r.color_at(0.75);
        assert!(red[0] > 0.99 && red[1] < 0.01, "and the red one: {red:?}");
        let between = r.color_at(0.5);
        assert!(
            between[0] > 0.4 && between[1] > 0.4,
            "halfway between two stops mixes them: {between:?}"
        );
        assert!(
            r.color_at(0.0)[1] < 0.01,
            "before the first interior stop it comes from the end colour"
        );

        // Capacity is real, and remove/resort keep the invariant.
        let mut full = MidStops::default();
        for i in 0..MAX_MID {
            assert!(
                full.insert(GradStop {
                    pos: i as f32 / MAX_MID as f32,
                    ..Default::default()
                })
                .is_some()
            );
        }
        assert!(full.is_full());
        assert_eq!(full.insert(GradStop::default()), None, "no seventh stop");
        assert!(full.remove(0));
        assert_eq!(full.len(), MAX_MID - 1);
    }

    /// Dragging a stop past its neighbour re-sorts and the drag keeps hold
    /// of the SAME stop (the editor tracks it by the returned index).
    #[test]
    fn resort_tracks_the_dragged_stop() {
        let mut mid = MidStops::default();
        mid.insert(GradStop {
            pos: 0.2,
            color: [1.0, 0.0, 0.0, 1.0],
        });
        mid.insert(GradStop {
            pos: 0.8,
            color: [0.0, 0.0, 1.0, 1.0],
        });
        mid.get_mut(0).unwrap().pos = 0.9; // dragged past the blue one
        let now = mid.resort(0);
        assert_eq!(now, 1, "the red stop is second now");
        assert!(mid.get(1).unwrap().color[0] > 0.99, "and it is still red");
        assert!(mid.get(0).unwrap().color[2] > 0.99);
    }

    /// `G-015`: the bias is monotone, hits the endpoints exactly, and 0 is
    /// the identity (so an unedited ramp is untouched by the feature).
    #[test]
    fn mixing_rate_is_monotone_and_neutral_at_zero() {
        for k in [-1.0f32, -0.5, 0.0, 0.5, 1.0] {
            assert_eq!(bias(0.0, k), 0.0);
            assert_eq!(bias(1.0, k), 1.0);
            let mut prev = -1.0;
            for i in 0..=20 {
                let v = bias(i as f32 / 20.0, k);
                assert!(v >= prev - 1e-6, "k={k} must not go backwards");
                prev = v;
            }
        }
        assert_eq!(bias(0.3, 0.0), 0.3);
        assert!(bias(0.5, 1.0) < 0.5, "positive holds the earlier colour");
        assert!(bias(0.5, -1.0) > 0.5);
    }

    /// `G-009`: Oklab round-trips, and its midpoint is a different colour
    /// from the sRGB midpoint — which is the whole reason the mode exists.
    #[test]
    fn perceptual_mixing_round_trips_and_differs() {
        for c in [
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.2, 0.6, 0.9],
            [0.9, 0.1, 0.4],
        ] {
            let back = crate::mix::oklab_to_srgb(crate::mix::srgb_to_oklab(c));
            for k in 0..3 {
                assert!((back[k] - c[k]).abs() < 2e-3, "{c:?} -> {back:?}");
            }
        }
        let blue = [0.0, 0.0, 1.0, 1.0];
        let yellow = [1.0, 1.0, 0.0, 1.0];
        let std = Ramp::two(blue, yellow);
        let mut perc = std;
        perc.opts.mix = MixMode::Perceptual;
        let a = std.color_at(0.5);
        let b = perc.color_at(0.5);
        let diff: f32 = (0..3).map(|k| (a[k] - b[k]).abs()).sum();
        assert!(diff > 0.05, "the two midpoints must differ: {a:?} {b:?}");
        // The ends are still exactly the authored colours in both modes.
        for k in 0..3 {
            assert!((perc.color_at(0.0)[k] - blue[k]).abs() < 2e-3);
            assert!((perc.color_at(1.0)[k] - yellow[k]).abs() < 2e-3);
        }
    }

    /// `G-005`: dithering perturbs neighbouring pixels of the SAME ramp
    /// position, stays under one 8-bit level, and leaves the clamped tail
    /// alone (no stipple in the region a fade is meant to leave clean).
    #[test]
    fn dithering_varies_inside_and_never_outside() {
        let mut r = black_to_white();
        r.opts.dither = true;
        let a = r.eval(0.5, 0, 0).unwrap();
        let b = r.eval(0.5, 1, 0).unwrap();
        assert_ne!(a[0], b[0], "adjacent pixels must not agree");
        assert!((a[0] - b[0]).abs() < 1.5 / 255.0, "and only by an LSB");
        // Deterministic: the same pixel twice gives the same noise.
        assert_eq!(r.eval(0.5, 0, 0).unwrap(), a);
        // The clamped tail is untouched.
        let plain = black_to_white();
        assert_eq!(r.eval(-1.0, 3, 5).unwrap(), plain.eval(-1.0, 3, 5).unwrap());
        assert_eq!(r.eval(2.0, 3, 5).unwrap(), plain.eval(2.0, 3, 5).unwrap());
    }

    /// `G-009`'s fourth mode. Blending in linear light puts a black→white
    /// midpoint well ABOVE the sRGB one (0.5 encoded is only ~21% of the
    /// light), which is the entire visible difference.
    #[test]
    fn linear_mixing_is_brighter_in_the_middle() {
        let mut r = black_to_white();
        let std_mid = r.color_at(0.5)[0];
        r.opts.mix = MixMode::Linear;
        let lin_mid = r.color_at(0.5)[0];
        assert!(
            lin_mid > std_mid + 0.15,
            "linear light is brighter at halfway: {lin_mid} vs {std_mid}"
        );
        // The ends stay exactly the authored colours, as in every mode.
        assert!(r.color_at(0.0)[0] < 1e-3 && r.color_at(1.0)[0] > 1.0 - 1e-3);
    }

    /// `G-010`: correction lifts the sagging middle of a Perceptual ramp,
    /// scales with the level, and CANNOT move either authored end.
    #[test]
    fn brightness_correction_lifts_only_the_middle() {
        let blue = [0.1, 0.1, 0.8, 1.0];
        let yellow = [0.9, 0.9, 0.1, 1.0];
        let mut r = Ramp::two(blue, yellow);
        r.opts.mix = MixMode::Perceptual;
        let lum = |c: [f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];

        let mut prev = lum(r.color_at(0.5));
        for level in 1..=MAX_BRIGHT {
            r.opts.bright = level;
            let now = lum(r.color_at(0.5));
            assert!(now > prev, "level {level} must lift further: {now} {prev}");
            prev = now;
            for (t, want) in [(0.0, blue), (1.0, yellow)] {
                let got = r.color_at(t);
                for k in 0..3 {
                    assert!(
                        (got[k] - want[k]).abs() < 3e-3,
                        "level {level} moved the end at t={t}: {got:?}"
                    );
                }
            }
        }
        // Off in every other mode, whatever the level says.
        r.opts.mix = MixMode::Standard;
        let mut plain = r;
        plain.opts.bright = 0;
        assert_eq!(r.color_at(0.5), plain.color_at(0.5));
    }

    /// `G-012`'s list ops: duplicate names itself out of the way, Up/Down
    /// reports where the item went, and neither can run off the ends.
    #[test]
    fn gradient_set_list_ops() {
        let mut set = GradientSet::starter();
        let n = set.len();
        assert!(n >= 2, "the starter set is not empty");

        let at = set.duplicate(0).expect("copied");
        assert_eq!(at, 1, "the copy lands directly after the original");
        assert_eq!(set.len(), n + 1);
        assert_ne!(set.items[0].name, set.items[1].name, "and is renamed");
        assert_eq!(
            (set.items[0].from, set.items[0].mid, set.items[0].opts),
            (set.items[1].from, set.items[1].mid, set.items[1].opts),
            "but is otherwise the same ramp"
        );

        assert_eq!(set.move_by(1, -1), 0, "Up swaps with the one above");
        assert_eq!(set.move_by(0, -1), 0, "and stops at the top");
        let last = set.len() - 1;
        assert_eq!(set.move_by(last, 1), last, "Down stops at the bottom");
        assert_eq!(set.move_by(99, -1), 99, "an out-of-range index is a no-op");

        // Round-trips through the one line that goes in ui.txt.
        let back = GradientSet::from_json(&set.to_json());
        assert_eq!(back, set, "the saved set reloads exactly");
        assert!(
            GradientSet::from_json("not json at all").is_empty(),
            "a corrupt line loses the set, it does not panic"
        );
    }

    /// `G-016`: a real GIMP `.ggr` body, its segment boundaries becoming
    /// interior stops. A file with more boundaries than fit degrades to the
    /// first [`MAX_MID`] rather than being refused.
    #[test]
    fn ggr_import_reads_segments_and_degrades() {
        let ggr = "GIMP Gradient\nName: Test Ramp\n2\n\
            0.0 0.25 0.5 1.0 0.0 0.0 1.0 0.0 1.0 0.0 1.0 0 0\n\
            0.5 0.75 1.0 0.0 1.0 0.0 1.0 0.0 0.0 1.0 1.0 0 0\n";
        let g = import_ggr(ggr).expect("a valid .ggr");
        assert_eq!(g.name, "Test Ramp");
        assert_eq!(g.from, [1.0, 0.0, 0.0, 1.0], "the first segment's left end");
        assert_eq!(g.to, [0.0, 0.0, 1.0, 1.0], "the last segment's right end");
        assert_eq!(g.mid.len(), 1, "one interior boundary between two segments");
        let s = g.mid.get(0).unwrap();
        assert!((s.pos - 0.5).abs() < 1e-6);
        assert_eq!(s.color, [0.0, 1.0, 0.0, 1.0], "coloured by the boundary");

        assert!(
            import_ggr("PNG\u{0}\r\n").is_err(),
            "a non-gradient is refused"
        );
        assert!(
            import_ggr("GIMP Gradient\nName: x\n0\n").is_err(),
            "and an empty one"
        );

        let mut many = String::from("GIMP Gradient\nName: Long\n20\n");
        for i in 0..20 {
            let (l, r) = (i as f32 / 20.0, (i + 1) as f32 / 20.0);
            let m = (l + r) / 2.0;
            many.push_str(&format!(
                "{l} {m} {r} 0.0 0.0 0.0 1.0 1.0 1.0 1.0 1.0 0 0\n"
            ));
        }
        let long = import_ggr(&many).expect("an over-long ramp still imports");
        assert_eq!(long.mid.len(), MAX_MID, "kept as many stops as fit");
    }

    /// A corrupt/hand-edited file claiming more stops than fit must not
    /// index out of bounds — `n` is clamped by every reader.
    #[test]
    fn an_over_long_stop_count_is_clamped() {
        let json = format!(
            r#"{{"n":{},"s":[{}]}}"#,
            250,
            (0..MAX_MID)
                .map(|_| r#"{"pos":0.5,"color":[0.0,0.0,0.0,1.0]}"#)
                .collect::<Vec<_>>()
                .join(",")
        );
        let mid: MidStops = serde_json::from_str(&json).unwrap();
        assert_eq!(mid.len(), MAX_MID);
        assert_eq!(mid.as_slice().len(), MAX_MID);
        assert!(mid.get(MAX_MID).is_none());
    }
}
