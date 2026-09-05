//! Lucide glyphs as egui strokes (owner ask 2026-09-05: "can you have better
//! icons? we have a whole bunch available that Magic Writer uses").
//!
//! The SVGs in `crates/app/assets/icons/lucide/` are read at compile time
//! (`include_str!`), parsed ONCE into polylines on a 24×24 grid, and painted
//! through the same `Painter` API the hand-drawn glyphs use. No image or SVG
//! crate: Lucide is pure strokes (2 px on a 24 px grid, round caps and joins),
//! which is exactly what `egui::Shape::line` draws, so the glyphs stay crisp
//! at any DPI and take the theme colour like every other icon here. Round caps
//! are a filled dot at each open end.
//!
//! Supported: `<path d>` with M L H V C S Q T A Z (absolute and relative,
//! implicit repeats), `<rect>` (with `rx`), `<circle>`, `<ellipse>`, `<line>`,
//! `<polyline>`, `<polygon>`. Anything else in a file is ignored. That covers
//! every file in the Lucide pack (checked: 6036 paths, 528 circles, 403 rects,
//! 144 lines, 16 ellipses, 6 polylines, 2 polygons; nothing else).

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke};
use std::collections::HashMap;
use std::sync::OnceLock;

/// One drawable element of a glyph, in 0..24 units.
#[derive(Clone, Debug)]
pub struct Element {
    /// The flattened outline. Sub-paths are separate elements.
    pub points: Vec<[f32; 2]>,
    pub closed: bool,
    /// `fill="currentColor"` in the source: a solid dot, not an outline.
    pub filled: bool,
}

/// Which elements of a glyph take the accent colour (the detail that says
/// what the icon DOES — the plus, the pupil, the arrowhead).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accent {
    None,
    /// The first `n` elements in file order.
    Head(usize),
    /// The last `n` elements in file order.
    Tail(usize),
}

/// The bundled files. Adding one = a line here + the copy in `assets/`.
const FILES: &[(&str, &str)] = &[
    ("type", include_str!("../../../assets/icons/lucide/type.svg")),
    (
        "message-circle",
        include_str!("../../../assets/icons/lucide/message-circle.svg"),
    ),
    (
        "layout-template",
        include_str!("../../../assets/icons/lucide/layout-template.svg"),
    ),
    ("folder", include_str!("../../../assets/icons/lucide/folder.svg")),
    (
        "folder-open",
        include_str!("../../../assets/icons/lucide/folder-open.svg"),
    ),
    ("file", include_str!("../../../assets/icons/lucide/file.svg")),
    ("spline", include_str!("../../../assets/icons/lucide/spline.svg")),
    ("grip", include_str!("../../../assets/icons/lucide/grip.svg")),
    (
        "file-image",
        include_str!("../../../assets/icons/lucide/file-image.svg"),
    ),
    ("pin", include_str!("../../../assets/icons/lucide/pin.svg")),
    (
        "pencil-line",
        include_str!("../../../assets/icons/lucide/pencil-line.svg"),
    ),
    ("eye", include_str!("../../../assets/icons/lucide/eye.svg")),
    ("eye-off", include_str!("../../../assets/icons/lucide/eye-off.svg")),
    ("lock", include_str!("../../../assets/icons/lucide/lock.svg")),
    (
        "lock-keyhole",
        include_str!("../../../assets/icons/lucide/lock-keyhole.svg"),
    ),
    (
        "corner-down-left",
        include_str!("../../../assets/icons/lucide/corner-down-left.svg"),
    ),
    ("tag", include_str!("../../../assets/icons/lucide/tag.svg")),
    ("file-plus", include_str!("../../../assets/icons/lucide/file-plus.svg")),
    (
        "folder-plus",
        include_str!("../../../assets/icons/lucide/folder-plus.svg"),
    ),
    ("copy", include_str!("../../../assets/icons/lucide/copy.svg")),
    (
        "arrow-down-to-line",
        include_str!("../../../assets/icons/lucide/arrow-down-to-line.svg"),
    ),
    ("trash-2", include_str!("../../../assets/icons/lucide/trash-2.svg")),
    ("funnel", include_str!("../../../assets/icons/lucide/funnel.svg")),
    ("plus", include_str!("../../../assets/icons/lucide/plus.svg")),
];

/// Every bundled glyph, parsed once.
fn glyphs() -> &'static HashMap<&'static str, Vec<Element>> {
    static G: OnceLock<HashMap<&'static str, Vec<Element>>> = OnceLock::new();
    G.get_or_init(|| FILES.iter().map(|(n, s)| (*n, parse_svg(s))).collect())
}

/// The parsed elements of a bundled glyph (`None` = not bundled).
pub fn glyph(name: &str) -> Option<&'static [Element]> {
    glyphs().get(name).map(|v| v.as_slice())
}

/// Paint `name` to fill `r`, `base` for the silhouette and `accent` (when
/// given) for the elements `which` selects.
pub fn paint(p: &Painter, r: Rect, name: &str, base: Color32, accent: Option<Color32>, which: Accent) {
    let Some(els) = glyph(name) else {
        return;
    };
    let n = els.len();
    for (i, e) in els.iter().enumerate() {
        let accented = match which {
            Accent::None => false,
            Accent::Head(k) => i < k,
            Accent::Tail(k) => i + k >= n,
        };
        let col = if accented { accent.unwrap_or(base) } else { base };
        paint_element(p, r, e, col);
    }
}

/// Paint `main` at 82 % in the top-left and a `plus` badge in the accent
/// colour at the bottom-right — the "make one of these" family (the
/// hand-drawn set wears a corner plus the same way, owner 2026-08-21).
pub fn paint_badged(p: &Painter, r: Rect, main: &str, base: Color32, accent: Option<Color32>) {
    let w = r.width().min(r.height());
    let main_r = Rect::from_min_size(r.min, egui::vec2(w * 0.82, w * 0.82));
    paint(p, main_r, main, base, None, Accent::None);
    let br = Rect::from_min_size(
        egui::pos2(r.min.x + w * 0.52, r.min.y + w * 0.52),
        egui::vec2(w * 0.48, w * 0.48),
    );
    // A disc of the panel colour behind the plus so it reads over the
    // subject instead of tangling with it.
    p.circle_filled(br.center(), w * 0.26, super::super::theme::c().panel);
    paint(p, br, "plus", accent.unwrap_or(base), None, Accent::None);
}

fn paint_element(p: &Painter, r: Rect, e: &Element, col: Color32) {
    let w = r.width().min(r.height());
    // Lucide's 2-on-24 stroke, nudged up a touch so a 14 px icon does not
    // vanish into the row; the hand-drawn set sits at ~0.11·w.
    let sw = (w * (2.0 / 24.0) * 1.15).clamp(1.0, 2.4);
    let stroke = Stroke::new(sw, col);
    let ox = r.min.x + (r.width() - w) * 0.5;
    let oy = r.min.y + (r.height() - w) * 0.5;
    let s = w / 24.0;
    let pts: Vec<Pos2> = e
        .points
        .iter()
        .map(|q| Pos2::new(ox + q[0] * s, oy + q[1] * s))
        .collect();
    if pts.is_empty() {
        return;
    }
    if e.filled {
        // A `fill="currentColor"` dot: fill the outline (Lucide only fills
        // tiny circles, so a convex fill is exact).
        p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
        return;
    }
    if pts.len() == 1 {
        p.circle_filled(pts[0], sw * 0.5, col);
        return;
    }
    if e.closed {
        p.add(Shape::closed_line(pts, stroke));
    } else {
        // Round caps: a dot at each open end.
        p.circle_filled(pts[0], sw * 0.5, col);
        p.circle_filled(pts[pts.len() - 1], sw * 0.5, col);
        p.add(Shape::line(pts, stroke));
    }
}

// ---------------------------------------------------------------------------
// SVG parsing: elements
// ---------------------------------------------------------------------------

/// Parse a Lucide SVG body into elements. Unknown tags are skipped; a
/// malformed `d` yields whatever was parsed up to the fault (never a panic —
/// these are compile-time assets, and the pin test walks every one).
pub fn parse_svg(text: &str) -> Vec<Element> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt + 1..];
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[..gt];
        rest = &rest[gt + 1..];
        let name_end = tag.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(tag.len());
        let name = &tag[..name_end];
        let attrs = &tag[name_end..];
        let filled = attr(attrs, "fill").is_some_and(|f| f == "currentColor");
        match name {
            "path" => {
                if let Some(d) = attr(attrs, "d") {
                    for (pts, closed) in parse_path(d) {
                        out.push(Element {
                            points: pts,
                            closed,
                            filled,
                        });
                    }
                }
            }
            "rect" => {
                let f = |k: &str| attr(attrs, k).and_then(|v| v.parse::<f32>().ok());
                let (Some(w), Some(h)) = (f("width"), f("height")) else {
                    continue;
                };
                let (x, y) = (f("x").unwrap_or(0.0), f("y").unwrap_or(0.0));
                let rx = f("rx").or(f("ry")).unwrap_or(0.0).min(w * 0.5).min(h * 0.5);
                out.push(Element {
                    points: rounded_rect(x, y, w, h, rx),
                    closed: true,
                    filled,
                });
            }
            "circle" | "ellipse" => {
                let f = |k: &str| attr(attrs, k).and_then(|v| v.parse::<f32>().ok());
                let (cx, cy) = (f("cx").unwrap_or(0.0), f("cy").unwrap_or(0.0));
                let (rx, ry) = if name == "circle" {
                    let r = f("r").unwrap_or(0.0);
                    (r, r)
                } else {
                    (f("rx").unwrap_or(0.0), f("ry").unwrap_or(0.0))
                };
                out.push(Element {
                    points: ellipse(cx, cy, rx, ry),
                    closed: true,
                    filled,
                });
            }
            "line" => {
                let f = |k: &str| attr(attrs, k).and_then(|v| v.parse::<f32>().ok());
                out.push(Element {
                    points: vec![
                        [f("x1").unwrap_or(0.0), f("y1").unwrap_or(0.0)],
                        [f("x2").unwrap_or(0.0), f("y2").unwrap_or(0.0)],
                    ],
                    closed: false,
                    filled,
                });
            }
            "polyline" | "polygon" => {
                if let Some(pts) = attr(attrs, "points") {
                    let nums = numbers(pts);
                    let points: Vec<[f32; 2]> = nums.chunks_exact(2).map(|c| [c[0], c[1]]).collect();
                    out.push(Element {
                        points,
                        closed: name == "polygon",
                        filled,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// `key="value"` inside a tag's attribute text.
fn attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = attrs;
    while let Some(i) = rest.find(key) {
        let before_ok = i == 0 || !rest.as_bytes()[i - 1].is_ascii_alphanumeric() && rest.as_bytes()[i - 1] != b'-';
        let after = &rest[i + key.len()..];
        let after_t = after.trim_start();
        if before_ok && after_t.starts_with('=') {
            let v = after_t[1..].trim_start();
            if let Some(q) = v.strip_prefix('"') {
                return q.find('"').map(|e| &q[..e]);
            }
        }
        rest = &rest[i + key.len()..];
    }
    None
}

// ---------------------------------------------------------------------------
// SVG parsing: path data
// ---------------------------------------------------------------------------

/// The SVG number lexer: `-.5.5` is two numbers, `1e-3` is one.
fn numbers(s: &str) -> Vec<f32> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() {
            let start = i;
            if c == b'-' || c == b'+' {
                i += 1;
            }
            let mut seen_dot = false;
            while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && !seen_dot)) {
                if b[i] == b'.' {
                    seen_dot = true;
                }
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let mut j = i + 1;
                if j < b.len() && (b[j] == b'-' || b[j] == b'+') {
                    j += 1;
                }
                if j < b.len() && b[j].is_ascii_digit() {
                    i = j;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            if let Ok(v) = s[start..i].parse::<f32>() {
                out.push(v);
            }
            if i == start {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Tokens of a `d` attribute: a command letter or a number.
enum Tok {
    Cmd(u8),
    Num(f32),
}

fn tokenize(d: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let b = d.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphabetic() {
            out.push(Tok::Cmd(c));
            i += 1;
        } else if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() {
            // Reuse the number lexer on the maximal numeric run.
            let mut j = i;
            while j < b.len() && !b[j].is_ascii_alphabetic() {
                j += 1;
            }
            for v in numbers(&d[i..j]) {
                out.push(Tok::Num(v));
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Flatten path data into sub-paths: (points, closed).
fn parse_path(d: &str) -> Vec<(Vec<[f32; 2]>, bool)> {
    let toks = tokenize(d);
    let mut subs: Vec<(Vec<[f32; 2]>, bool)> = Vec::new();
    let mut cur: Vec<[f32; 2]> = Vec::new();
    let mut pos = [0.0f32, 0.0];
    let mut start = [0.0f32, 0.0];
    // Reflected control point for S/T.
    let mut last_c: Option<[f32; 2]> = None;
    let mut last_q: Option<[f32; 2]> = None;
    let mut i = 0;
    let mut cmd = b'M';
    let take = |i: &mut usize, toks: &[Tok]| -> Option<f32> {
        match toks.get(*i) {
            Some(Tok::Num(v)) => {
                *i += 1;
                Some(*v)
            }
            _ => None,
        }
    };
    let flush = |cur: &mut Vec<[f32; 2]>, subs: &mut Vec<(Vec<[f32; 2]>, bool)>, closed: bool| {
        if !cur.is_empty() {
            subs.push((std::mem::take(cur), closed));
        }
    };
    while i < toks.len() {
        if let Tok::Cmd(c) = toks[i] {
            cmd = c;
            i += 1;
        }
        let rel = cmd.is_ascii_lowercase();
        let up = cmd.to_ascii_uppercase();
        let base = if rel { pos } else { [0.0, 0.0] };
        match up {
            b'M' => {
                let (Some(x), Some(y)) = (take(&mut i, &toks), take(&mut i, &toks)) else { break };
                flush(&mut cur, &mut subs, false);
                pos = [base[0] + x, base[1] + y];
                start = pos;
                cur.push(pos);
                // Implicit repeats after M are L.
                cmd = if rel { b'l' } else { b'L' };
                last_c = None;
                last_q = None;
            }
            b'L' => {
                let (Some(x), Some(y)) = (take(&mut i, &toks), take(&mut i, &toks)) else { break };
                pos = [base[0] + x, base[1] + y];
                cur.push(pos);
                last_c = None;
                last_q = None;
            }
            b'H' => {
                let Some(x) = take(&mut i, &toks) else { break };
                pos = [base[0] + x, pos[1]];
                cur.push(pos);
                last_c = None;
                last_q = None;
            }
            b'V' => {
                let Some(y) = take(&mut i, &toks) else { break };
                pos = [pos[0], base[1] + y];
                cur.push(pos);
                last_c = None;
                last_q = None;
            }
            b'C' | b'S' => {
                let c1 = if up == b'C' {
                    let (Some(x), Some(y)) = (take(&mut i, &toks), take(&mut i, &toks)) else { break };
                    [base[0] + x, base[1] + y]
                } else {
                    last_c.map(|c| [2.0 * pos[0] - c[0], 2.0 * pos[1] - c[1]]).unwrap_or(pos)
                };
                let (Some(x2), Some(y2), Some(x), Some(y)) = (
                    take(&mut i, &toks),
                    take(&mut i, &toks),
                    take(&mut i, &toks),
                    take(&mut i, &toks),
                ) else {
                    break;
                };
                let c2 = [base[0] + x2, base[1] + y2];
                let end = [base[0] + x, base[1] + y];
                cubic(&mut cur, pos, c1, c2, end);
                last_c = Some(c2);
                last_q = None;
                pos = end;
            }
            b'Q' | b'T' => {
                let c1 = if up == b'Q' {
                    let (Some(x), Some(y)) = (take(&mut i, &toks), take(&mut i, &toks)) else { break };
                    [base[0] + x, base[1] + y]
                } else {
                    last_q.map(|c| [2.0 * pos[0] - c[0], 2.0 * pos[1] - c[1]]).unwrap_or(pos)
                };
                let (Some(x), Some(y)) = (take(&mut i, &toks), take(&mut i, &toks)) else { break };
                let end = [base[0] + x, base[1] + y];
                quad(&mut cur, pos, c1, end);
                last_q = Some(c1);
                last_c = None;
                pos = end;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(rot), Some(large), Some(sweep), Some(x), Some(y)) = (
                    take(&mut i, &toks),
                    take(&mut i, &toks),
                    take(&mut i, &toks),
                    take(&mut i, &toks),
                    take(&mut i, &toks),
                    take(&mut i, &toks),
                    take(&mut i, &toks),
                ) else {
                    break;
                };
                let end = [base[0] + x, base[1] + y];
                arc(&mut cur, pos, rx, ry, rot, large != 0.0, sweep != 0.0, end);
                pos = end;
                last_c = None;
                last_q = None;
            }
            b'Z' => {
                flush(&mut cur, &mut subs, true);
                pos = start;
                last_c = None;
                last_q = None;
                // A Z followed by numbers is malformed; a following command
                // letter is picked up by the loop head.
                if let Some(Tok::Num(_)) = toks.get(i) {
                    break;
                }
            }
            _ => break,
        }
    }
    flush(&mut cur, &mut subs, false);
    subs
}

const CURVE_STEPS: usize = 10;

fn cubic(out: &mut Vec<[f32; 2]>, p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2]) {
    for k in 1..=CURVE_STEPS {
        let t = k as f32 / CURVE_STEPS as f32;
        let u = 1.0 - t;
        let x = u * u * u * p0[0] + 3.0 * u * u * t * p1[0] + 3.0 * u * t * t * p2[0] + t * t * t * p3[0];
        let y = u * u * u * p0[1] + 3.0 * u * u * t * p1[1] + 3.0 * u * t * t * p2[1] + t * t * t * p3[1];
        out.push([x, y]);
    }
}

fn quad(out: &mut Vec<[f32; 2]>, p0: [f32; 2], p1: [f32; 2], p2: [f32; 2]) {
    for k in 1..=CURVE_STEPS {
        let t = k as f32 / CURVE_STEPS as f32;
        let u = 1.0 - t;
        let x = u * u * p0[0] + 2.0 * u * t * p1[0] + t * t * p2[0];
        let y = u * u * p0[1] + 2.0 * u * t * p1[1] + t * t * p2[1];
        out.push([x, y]);
    }
}

/// SVG elliptical arc, endpoint → centre parameterisation (SVG 1.1 §F.6),
/// flattened at ~15° per step.
#[allow(clippy::too_many_arguments)]
fn arc(
    out: &mut Vec<[f32; 2]>,
    p0: [f32; 2],
    rx: f32,
    ry: f32,
    rot_deg: f32,
    large: bool,
    sweep: bool,
    p1: [f32; 2],
) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-6 || ry < 1e-6 || (p0[0] - p1[0]).abs() < 1e-6 && (p0[1] - p1[1]).abs() < 1e-6 {
        out.push(p1);
        return;
    }
    let phi = rot_deg.to_radians();
    let (sin_p, cos_p) = phi.sin_cos();
    let dx = (p0[0] - p1[0]) * 0.5;
    let dy = (p0[1] - p1[1]) * 0.5;
    let x1p = cos_p * dx + sin_p * dy;
    let y1p = -sin_p * dx + cos_p * dy;
    // Scale radii up if the arc cannot fit.
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = if den.abs() < 1e-12 { 0.0 } else { (num / den).max(0.0).sqrt() };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * (rx * y1p / ry);
    let cyp = coef * (-(ry * x1p) / rx);
    let cx = cos_p * cxp - sin_p * cyp + (p0[0] + p1[0]) * 0.5;
    let cy = sin_p * cxp + cos_p * cyp + (p0[1] + p1[1]) * 0.5;
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| -> f32 {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let mut a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let theta1 = ang(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut dtheta = ang(
        (x1p - cxp) / rx,
        (y1p - cyp) / ry,
        (-x1p - cxp) / rx,
        (-y1p - cyp) / ry,
    );
    if !sweep && dtheta > 0.0 {
        dtheta -= std::f32::consts::TAU;
    } else if sweep && dtheta < 0.0 {
        dtheta += std::f32::consts::TAU;
    }
    let steps = ((dtheta.abs() / 15f32.to_radians()).ceil() as usize).max(1);
    for k in 1..=steps {
        let t = theta1 + dtheta * (k as f32 / steps as f32);
        let (st, ct) = t.sin_cos();
        let x = cos_p * rx * ct - sin_p * ry * st + cx;
        let y = sin_p * rx * ct + cos_p * ry * st + cy;
        out.push([x, y]);
    }
    // Land exactly on the endpoint regardless of rounding.
    if let Some(last) = out.last_mut() {
        *last = p1;
    }
}

fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<[f32; 2]> {
    let n = if rx.max(ry) < 1.5 { 10 } else { 24 };
    (0..n)
        .map(|k| {
            let t = k as f32 / n as f32 * std::f32::consts::TAU;
            [cx + rx * t.cos(), cy + ry * t.sin()]
        })
        .collect()
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<[f32; 2]> {
    if r <= 1e-6 {
        return vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    }
    let mut out = Vec::with_capacity(4 * 6);
    // Corner centres, clockwise from top-left; each quarter runs 90°.
    let corners = [
        ([x + w - r, y + r], -90.0f32),
        ([x + w - r, y + h - r], 0.0),
        ([x + r, y + h - r], 90.0),
        ([x + r, y + r], 180.0),
    ];
    for (c, a0) in corners {
        for k in 0..=5 {
            let a = (a0 + 90.0 * k as f32 / 5.0).to_radians();
            out.push([c[0] + r * a.cos(), c[1] + r * a.sin()]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every bundled file parses into at least one element, and every point
    /// sits on the 24-grid (a broken lexer shows up as a wild coordinate).
    #[test]
    fn every_bundled_glyph_parses_onto_the_grid() {
        for (name, _) in FILES {
            let els = glyph(name).unwrap_or_else(|| panic!("{name} missing"));
            assert!(!els.is_empty(), "{name}: no elements");
            for e in els {
                assert!(!e.points.is_empty(), "{name}: empty element");
                for p in &e.points {
                    assert!(
                        (-1.0..=25.0).contains(&p[0]) && (-1.0..=25.0).contains(&p[1]),
                        "{name}: point off the grid {p:?}"
                    );
                }
            }
        }
    }

    /// The arc-heavy Lucide bubble: one sub-path, flattened to a real curve,
    /// ending where the path data says (the tail's tip).
    #[test]
    fn arcs_flatten_and_land_on_their_endpoints() {
        let els = glyph("message-circle").unwrap();
        assert_eq!(els.len(), 1);
        let pts = &els[0].points;
        assert!(pts.len() > 30, "flattened, got {}", pts.len());
        // The 10-unit circle passes near the top of the grid.
        assert!(pts.iter().any(|p| p[1] < 2.5), "reaches the top");
        assert!(!els[0].closed);
    }

    /// The lexer: signs, leading dots, exponents, run-together numbers.
    #[test]
    fn the_number_lexer_splits_svg_shorthand() {
        assert_eq!(numbers("3.413-.998"), vec![3.413, -0.998]);
        assert_eq!(numbers("-1.5.5"), vec![-1.5, 0.5]);
        assert_eq!(numbers("1e-3 2"), vec![0.001, 2.0]);
        assert_eq!(numbers("0 0 1-1-1"), vec![0.0, 0.0, 1.0, -1.0, -1.0]);
    }

    /// Z closes, M starts a fresh sub-path, and H/V/relative moves compose.
    #[test]
    fn subpaths_close_and_restart() {
        let subs = parse_path("M2 2h4v4H2Z m8 0l2 2");
        assert_eq!(subs.len(), 2);
        assert!(subs[0].1, "first closed");
        assert_eq!(subs[0].0, vec![[2.0, 2.0], [6.0, 2.0], [6.0, 6.0], [2.0, 6.0]]);
        assert!(!subs[1].1);
        // Relative m after Z is relative to the sub-path start (2,2).
        assert_eq!(subs[1].0, vec![[10.0, 2.0], [12.0, 4.0]]);
    }

    /// Rects with `rx` become rounded outlines; `fill="currentColor"` marks a dot.
    #[test]
    fn rects_and_filled_dots() {
        let els = parse_svg(r#"<rect width="18" height="7" x="3" y="3" rx="1" /><circle cx="7.5" cy="7.5" r=".5" fill="currentColor" />"#);
        assert_eq!(els.len(), 2);
        assert!(els[0].closed && els[0].points.len() == 24);
        assert!(els[1].filled);
    }
}
