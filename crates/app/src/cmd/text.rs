//! `AppCmd` arms and the batch API for lettering: balloons, text
//! items, text styles, and the multi-object edits that move them.

use super::*;

/// One item's field patch for [`AppCmd::TextsPatch`] — every field optional,
/// absent = keep. Serde because this IS the wire shape the automation socket
/// receives (`remote.rs`); the enum stays serde-free, only this leaf speaks
/// JSON.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TextPatch {
    pub id: u64,
    /// New content. Style runs are CLEARED with it — spans are UTF-16
    /// offsets into the old string and would land mid-glyph in the new one.
    pub text: Option<String>,
    /// Direction: `true` = vertical JP columns (right-to-left).
    pub vertical: Option<bool>,
    pub align: Option<mn_core::Align>,
    pub frame_align: Option<mn_core::FrameAlign>,
    pub font: Option<String>,
    pub size_pt: Option<f32>,
    pub pos: Option<[f32; 2]>,
    /// Explicit wrap box; setting it turns `auto_size` off, same as a
    /// hand resize.
    pub size: Option<[f32; 2]>,
}

/// The three batch doors share this shape: warm the ORA caches, clone the
/// set, mutate, re-shape what changed, commit through `set_texts` (one undo
/// press). Returns how many items the mutation actually reached, plus the
/// minted ids for adds. All three tolerate absent ids — a remote batch may
/// race the artist deleting an item, and "3 of 4 landed" is the honest
/// answer, not an error.
pub(crate) fn texts_patch(app: &mut App, layer: usize, patches: &[TextPatch]) -> usize {
    if app.doc.layers.get(layer).and_then(|l| l.texts()).is_none() {
        return 0;
    }
    app.warm_texts(layer);
    let dpi = app.doc_dpi();
    let mut ts = app.doc.layers[layer].texts().unwrap().clone();
    let mut hit = 0;
    for p in patches {
        let Some(i) = ts.index_of_id(p.id) else {
            continue;
        };
        let t = &mut ts.texts[i];
        if let Some(s) = &p.text {
            t.text = s.clone();
            t.runs.clear();
        }
        if let Some(v) = p.vertical {
            t.vertical = v;
        }
        if let Some(a) = p.align {
            t.align = a;
        }
        if let Some(a) = p.frame_align {
            t.frame_align = a;
        }
        if let Some(f) = &p.font {
            t.font = f.clone();
        }
        if let Some(s) = p.size_pt {
            t.size_pt = s.clamp(1.0, 500.0);
        }
        if let Some(pos) = p.pos {
            t.pos = pos;
        }
        if let Some(size) = p.size {
            t.size = size;
            t.auto_size = false;
        }
        if let Some(engine) = app.text_engine.as_ref() {
            // Same order as `edit_item`: natural metrics first when the box
            // is auto-sized (vertical keeps the top-RIGHT corner planted),
            // then the sprite cache.
            if t.auto_size {
                if let Ok(natural) = engine.natural_size(t, dpi) {
                    if t.vertical {
                        t.pos[0] += t.size[0] - natural[0];
                    }
                    t.size = natural;
                }
            }
            t.cache = engine.render(t, dpi).ok().flatten();
        }
        hit += 1;
    }
    if hit > 0 {
        app.doc.set_texts(layer, ts);
    }
    hit
}

pub(crate) fn texts_add(app: &mut App, layer: usize, items: Vec<mn_core::TextItem>) -> Vec<u64> {
    if items.is_empty() || app.doc.layers.get(layer).and_then(|l| l.texts()).is_none() {
        return Vec::new();
    }
    app.warm_texts(layer);
    let dpi = app.doc_dpi();
    let mut ts = app.doc.layers[layer].texts().unwrap().clone();
    let start = ts.texts.len();
    for mut t in items {
        // A template never carries identity (`story_item_template` rule);
        // the commit door mints the real id.
        t.id = 0;
        if let Some(engine) = app.text_engine.as_ref() {
            if t.auto_size {
                if let Ok(natural) = engine.natural_size(&t, dpi) {
                    if t.vertical {
                        t.pos[0] += t.size[0] - natural[0];
                    }
                    t.size = natural;
                }
            }
            t.cache = engine.render(&t, dpi).ok().flatten();
        }
        ts.texts.push(t);
    }
    if !app.doc.set_texts(layer, ts) {
        return Vec::new();
    }
    app.doc.layers[layer]
        .texts()
        .map(|ts| ts.texts[start..].iter().map(|t| t.id).collect())
        .unwrap_or_default()
}

pub(crate) fn texts_remove(app: &mut App, layer: usize, ids: &[u64]) -> usize {
    if app.doc.layers.get(layer).and_then(|l| l.texts()).is_none() {
        return 0;
    }
    // Warm BEFORE cloning — the clone must carry the warmed caches, or the
    // survivors of the retain would rasterize to nothing.
    app.warm_texts(layer);
    let mut ts = app.doc.layers[layer].texts().unwrap().clone();
    let before = ts.texts.len();
    ts.texts.retain(|t| !ids.contains(&t.id));
    let gone = before - ts.texts.len();
    if gone > 0 {
        app.doc.set_texts(layer, ts);
    }
    gone
}

/// One balloon's field patch — [`TextPatch`]'s twin for the bubbles, every
/// field optional, absent = keep. Serde because this IS the wire shape the
/// automation socket receives (`remote.rs`, `balloons.patch`).
///
/// Deliberately NO `AppCmd` variant beside [`AppCmd::TextsPatch`] & co: the
/// text variants exist as the future auto-action surface and are dead-code-
/// allowed until one produces them, and adding three more dead variants to
/// mirror a mirror is bookkeeping, not code. `remote.rs` calls the three
/// door fns directly (it wants their counts anyway); the day a queued
/// balloon batch has a producer, the variant is six lines away.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct BalloonPatch {
    pub id: u64,
    /// Position, size AND kind in one field — an ellipse's centre, a rect's
    /// corners and a drawn polygon's points all live inside the shape, so a
    /// move is a whole-shape send. Externally tagged, exactly the JSON
    /// `balloons.list` prints back.
    pub shape: Option<mn_core::BalloonShape>,
    /// The whole tail list, replaced. Tails carry no ids of their own (they
    /// are a short ordered list on one balloon), and half-patching one by
    /// index is the addressing this round exists to get away from.
    pub tails: Option<Vec<mn_core::Tail>>,
    /// CSP's "correct line width" multiplier; the Tool Property bar's range.
    pub width_scale: Option<f32>,
    pub line_color: Option<[u8; 3]>,
    pub fill_color: Option<[u8; 3]>,
    pub line_opacity: Option<f32>,
    pub fill_opacity: Option<f32>,
    /// Screened fill. Double option on purpose: absent = keep, explicit
    /// `null` = back to a flat fill. Without it "un-tone this bubble" would
    /// be a state the wire cannot say.
    #[serde(default, deserialize_with = "present_option")]
    pub fill_tone: Option<Option<mn_core::BalloonTone>>,
}

/// Serde's documented double-option idiom: a field that is present decodes
/// to `Some(_)` even when its value is `null`; `#[serde(default)]` covers
/// absent.
fn present_option<'de, T, D>(d: D) -> Result<Option<T>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(d).map(Some)
}

/// The balloon batch doors — `texts_patch`/`add`/`remove`'s twins, same
/// contract: clone the set, mutate, commit once through
/// `Document::set_balloons` (one undo press for the whole batch), and
/// return how many items the mutation actually reached. Absent ids are
/// skipped rather than an error: a remote batch may race the artist
/// deleting a bubble, and "3 of 4 landed" is the honest answer.
///
/// No cache warming, unlike the text doors: a balloon layer's raster is
/// derived from the vectors inside `set_balloons`, there is no per-item
/// sprite to keep alive.
pub(crate) fn balloons_patch(app: &mut App, layer: usize, patches: &[BalloonPatch]) -> usize {
    let Some(bs) = app.doc.layers.get(layer).and_then(|l| l.balloons()) else {
        return 0;
    };
    let mut bs = bs.clone();
    let mut hit = 0;
    for p in patches {
        let Some(i) = bs.index_of_id(p.id) else {
            continue;
        };
        let mut b = bs.balloons[i].clone();
        if let Some(s) = &p.shape {
            b.shape = s.clone();
        }
        if let Some(t) = &p.tails {
            b.tails = t.clone();
        }
        if let Some(w) = p.width_scale {
            b.width_scale = w.clamp(0.25, 4.0);
        }
        if let Some(c) = p.line_color {
            b.line_color = c;
        }
        if let Some(c) = p.fill_color {
            b.fill_color = c;
        }
        if let Some(o) = p.line_opacity {
            b.line_opacity = o.clamp(0.0, 1.0);
        }
        if let Some(o) = p.fill_opacity {
            b.fill_opacity = o.clamp(0.0, 1.0);
        }
        if let Some(t) = p.fill_tone {
            b.fill_tone = t;
        }
        // A patch that would leave a bubble too small to ink is skipped like
        // an absent id — the same refusal the balloon tool gives a stray
        // drag, and the count tells the client it did not land.
        if !b.is_valid() {
            continue;
        }
        bs.balloons[i] = b;
        hit += 1;
    }
    if hit > 0 {
        app.doc.set_balloons(layer, bs);
    }
    hit
}

/// Append balloons (id 0; the commit door mints). Callers build the items
/// from the balloon tool's own fresh-bubble defaults — see `remote.rs`'s
/// `NewBalloon` — and are the ones that validate geometry: a degenerate
/// shape arriving here is a caller bug, not a race, so it is refused up
/// front rather than half-committed.
pub(crate) fn balloons_add(app: &mut App, layer: usize, items: Vec<mn_core::Balloon>) -> Vec<u64> {
    let Some(bs) = app.doc.layers.get(layer).and_then(|l| l.balloons()) else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let mut bs = bs.clone();
    let start = bs.balloons.len();
    for mut b in items {
        b.id = 0;
        bs.balloons.push(b);
    }
    if !app.doc.set_balloons(layer, bs) {
        return Vec::new();
    }
    app.doc.layers[layer]
        .balloons()
        .map(|bs| bs.balloons[start..].iter().map(|b| b.id).collect())
        .unwrap_or_default()
}

pub(crate) fn balloons_remove(app: &mut App, layer: usize, ids: &[u64]) -> usize {
    let Some(bs) = app.doc.layers.get(layer).and_then(|l| l.balloons()) else {
        return 0;
    };
    let mut bs = bs.clone();
    let before = bs.balloons.len();
    bs.balloons.retain(|b| !ids.contains(&b.id));
    let gone = before - bs.balloons.len();
    if gone > 0 {
        app.doc.set_balloons(layer, bs);
        // The Object tool's selection is an INDEX pair; leaving it after a
        // removal would silently re-point it at whichever bubble slid down
        // into the slot. `BalloonDelete` clears it for the same reason.
        if app.balloon_sel.map(|(l, _)| l) == Some(layer) {
            app.balloon_sel = None;
        }
    }
    gone
}

pub(super) fn run(app: &mut App, cmd: AppCmd, cmd_tail: CmdTail) {
    match cmd {
        AppCmd::BalloonAdd { balloon } => {
            // The active balloon layer takes it; failing that, the topmost
            // visible, unlocked balloon layer does (CSP keeps a page's
            // balloons on one layer — the "Add to selected" default). A
            // fresh layer only when the page has none to join: before
            // this, bubble → words → bubble → words bred a layer pair per
            // balloon, eight balloons = sixteen layers (surface pass
            // 2026-09-02).
            let li = if app.doc.active_layer().is_balloon() {
                Some(app.doc.active)
            } else {
                (0..app.doc.layers.len()).rev().find(|&i| {
                    let l = &app.doc.layers[i];
                    l.is_balloon() && l.visible && !l.lock
                })
            };
            let selected = match li {
                Some(li) => {
                    let mut bs = app.doc.layers[li].balloons().expect("is_balloon").clone();
                    bs.balloons.push(balloon);
                    let last = bs.balloons.len() - 1;
                    app.doc.set_balloons(li, bs);
                    // The drawn object's layer is the selected one (CSP),
                    // so the Layers palette and the next tail drag agree
                    // on where it went.
                    app.doc.active = li;
                    (li, last)
                }
                None => {
                    // Fresh layer per balloon, CSP-style; border from Tool
                    // Property. Structural op — clears history, like frames.
                    let border = app.mm_to_px(app.balloon_border_mm).max(2.0);
                    let n = app.doc.layers.iter().filter(|l| l.is_balloon()).count() + 1;
                    let mut bs = BalloonSet::new(border);
                    bs.balloons.push(balloon);
                    let li = app.doc.add_balloon_layer(format!("Balloon {n}"), bs);
                    app.renderer.invalidate();
                    (li, 0)
                }
            };
            // The fresh balloon is SELECTED (CSP selects a drawn object) —
            // O's handles and the Tool Property rows apply to it immediately.
            app.balloon_sel = Some(selected);
            app.set_status("balloon added — O edits it, Tail mode attaches a tail");
            app.mark_dirty();
        }
        AppCmd::BalloonTailAdd {
            layer,
            balloon,
            tail,
        } => {
            if let Some(bs) = app.doc.layers.get(layer).and_then(|l| l.balloons()) {
                let mut bs = bs.clone();
                if let Some(b) = bs.balloons.get_mut(balloon) {
                    b.tails.push(tail);
                    app.doc.set_balloons(layer, bs);
                    app.set_status("tail attached");
                    app.mark_dirty();
                }
            }
        }
        AppCmd::BalloonCommit { layer, balloons } => {
            if app.doc.set_balloons(layer, balloons) {
                app.mark_dirty();
            }
        }
        AppCmd::ObjectMultiDelete => {
            app.cancel_text_edit();
            let mut members: Vec<crate::app::ObjRef> = app.object_multi.clone();
            if let Some(p) = app.object_selection() {
                if !members.contains(&p) {
                    members.push(p);
                }
            }
            // Group by kind+layer so per-layer removals apply largest-
            // index-first (removing balloon 0 must not shift balloon 1's
            // index before it goes).
            let mut balloons: Vec<(usize, usize)> = members
                .iter()
                .filter_map(|r| match r {
                    crate::app::ObjRef::Balloon(l, b) => Some((*l, *b)),
                    _ => None,
                })
                .collect();
            balloons.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
            let mut texts: Vec<(usize, usize)> = members
                .iter()
                .filter_map(|r| match r {
                    crate::app::ObjRef::Text(l, t) => Some((*l, *t)),
                    _ => None,
                })
                .collect();
            texts.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
            let mut pushed = 0usize;
            for (l, b) in balloons {
                if let Some(bs) = app.doc.layers.get(l).and_then(|ly| ly.balloons()).cloned() {
                    if b < bs.balloons.len() {
                        let mut bs = bs;
                        bs.balloons.remove(b);
                        app.doc.set_balloons(l, bs);
                        pushed += 1;
                    }
                }
            }
            for (l, t) in texts {
                if let Some(ts) = app.doc.layers.get(l).and_then(|ly| ly.texts()).cloned() {
                    if t < ts.texts.len() {
                        let mut ts = ts;
                        ts.texts.remove(t);
                        app.warm_texts(l);
                        app.doc.set_texts(l, ts);
                        pushed += 1;
                    }
                }
            }
            if pushed > 1 {
                app.doc.wrap_recent("Delete objects", pushed);
            }
            let n = pushed;
            app.clear_object_selection();
            app.set_status(format!(
                "{n} object{} deleted — one undo",
                if n == 1 { "" } else { "s" }
            ));
            app.mark_dirty();
        }
        AppCmd::ObjectMultiMove { dx, dy } => {
            app.cancel_text_edit();
            let mut members: Vec<crate::app::ObjRef> = app.object_multi.clone();
            if let Some(p) = app.object_selection()
                && !members.contains(&p)
            {
                members.push(p);
            }
            let (fdx, fdy) = (dx as f32, dy as f32);
            let mut pushed = 0usize;
            let mut moved = 0usize;
            // Texts and balloons batch per layer — one undo group per
            // layer, not per item. A move never removes anything, so
            // member indices stay valid in any order.
            let mut text_layers: Vec<(usize, Vec<usize>)> = Vec::new();
            let mut balloon_layers: Vec<(usize, Vec<usize>)> = Vec::new();
            for r in &members {
                match r {
                    crate::app::ObjRef::Text(l, t) => {
                        if let Some(e) = text_layers.iter_mut().find(|(la, _)| *la == *l) {
                            e.1.push(*t);
                        } else {
                            text_layers.push((*l, vec![*t]));
                        }
                    }
                    crate::app::ObjRef::Balloon(l, b) => {
                        if let Some(e) = balloon_layers.iter_mut().find(|(la, _)| *la == *l) {
                            e.1.push(*b);
                        } else {
                            balloon_layers.push((*l, vec![*b]));
                        }
                    }
                    _ => {}
                }
            }
            for (l, tis) in text_layers {
                let Some(mut ts) = app.doc.layers.get(l).and_then(|ly| ly.texts()).cloned()
                else {
                    continue;
                };
                let mut n = 0;
                for t in tis {
                    if t < ts.texts.len() {
                        // A translate keeps the rendered sprite valid —
                        // the same rule the single-text MoveWhole drag
                        // commits under (no re-render, no cache miss).
                        ts.texts[t].translate(fdx, fdy);
                        n += 1;
                    }
                }
                if n > 0 {
                    app.doc.set_texts(l, ts);
                    pushed += 1;
                    moved += n;
                }
            }
            for (l, bis) in balloon_layers {
                let Some(mut bs) = app.doc.layers.get(l).and_then(|ly| ly.balloons()).cloned()
                else {
                    continue;
                };
                let mut n = 0;
                for b in bis {
                    if b < bs.balloons.len() {
                        bs.balloons[b].translate(fdx, fdy);
                        n += 1;
                    }
                }
                if n > 0 {
                    app.doc.set_balloons(l, bs);
                    pushed += 1;
                    moved += n;
                }
            }
            // Effect-line runs: focus kinds carry their centre in a/b,
            // speed lines only their convergence point — translate what
            // is positional, regen, and count only successful regens.
            for r in &members {
                if let crate::app::ObjRef::Gen(l) = r
                    && let Some(mut spec) = app.doc.layers.get(*l).and_then(|ly| ly.genlines.clone())
                {
                    if spec.focus {
                        spec.a += fdx;
                        spec.b += fdy;
                    }
                    if let Some(c) = spec.converge.as_mut() {
                        c[0] += fdx;
                        c[1] += fdy;
                    }
                    if app.doc.regen_genlines(*l, spec) {
                        pushed += 1;
                        moved += 1;
                    }
                }
            }
            // Frame folders: the panel geometry moves, the folder's
            // children's pixels move with it — the single-folder
            // MoveWhole semantics, per member.
            for r in &members {
                if let crate::app::ObjRef::Frame(l, f) = r {
                    let Some(mut fs) = app.doc.layers.get(*l).and_then(|ly| ly.frames()).cloned()
                    else {
                        continue;
                    };
                    if *f >= fs.frames.len() {
                        continue;
                    }
                    fs.frames[*f].translate(fdx, fdy);
                    for k in app.doc.children_range(*l) {
                        let mask_before = app.doc.layers[k].mask.clone();
                        let rev0 = mask_before.as_ref().map(|m| m.revision);
                        app.doc.begin_op_on(k);
                        app.doc.set_op_label("Move panel");
                        app.doc.layers[k].translate_content(dx, dy);
                        app.doc.end_op();
                        let rev1 = app.doc.layers[k].mask.as_ref().map(|m| m.revision);
                        if rev1 != rev0 {
                            app.doc.record_mask_change(k, mask_before, "Move panel");
                        }
                        pushed += 1;
                    }
                    app.doc.set_frames(*l, fs);
                    pushed += 1;
                    moved += 1;
                }
            }
            if pushed > 1 {
                app.doc.wrap_recent("Move objects", pushed);
            }
            if moved > 0 {
                app.set_status(format!(
                    "moved {moved} object{} — one undo",
                    if moved == 1 { "" } else { "s" }
                ));
                app.mark_dirty();
            }
            app.mark_dirty();
        }
        AppCmd::BalloonDelete { layer, balloon } => {
            if let Some(bs) = app.doc.layers.get(layer).and_then(|l| l.balloons()) {
                let mut bs = bs.clone();
                if balloon < bs.balloons.len() {
                    bs.balloons.remove(balloon);
                    app.doc.set_balloons(layer, bs);
                    app.balloon_sel = None;
                    app.set_status("balloon deleted");
                    app.mark_dirty();
                }
            }
        }

        AppCmd::TextStyleUpsert(style) => {
            match app
                .doc
                .text_styles
                .iter_mut()
                .find(|s| s.name == style.name)
            {
                Some(s) => *s = style.clone(),
                None => app.doc.text_styles.push(style.clone()),
            }
            let n = app.apply_text_style_current(&style);
            app.doc.touch();
            app.set_status(if n > 0 {
                format!(
                    "style \"{}\": {n} text(s) on this page restyled",
                    style.name
                )
            } else {
                format!(
                    "style \"{}\" saved — no text on this page uses it yet",
                    style.name
                )
            });
            app.mark_dirty();
        }
        AppCmd::TextStyleDelete(name) => {
            app.doc.text_styles.retain(|s| s.name != name);
            // Items keep their look; only the reference clears, layer by
            // layer through the normal text door so it undoes cleanly.
            let mut groups = 0usize;
            for li in 0..app.doc.layers.len() {
                let hit = app
                    .doc
                    .layers
                    .get(li)
                    .and_then(|l| l.texts())
                    .is_some_and(|ts| {
                        ts.texts
                            .iter()
                            .any(|t| t.style.as_deref() == Some(name.as_str()))
                    });
                if !hit {
                    continue;
                }
                app.warm_texts(li);
                let Some(ts) = app.doc.layers.get(li).and_then(|l| l.texts()) else {
                    continue;
                };
                let mut ts = ts.clone();
                for t in ts.texts.iter_mut() {
                    if t.style.as_deref() == Some(name.as_str()) {
                        t.style = None;
                    }
                }
                if app.doc.set_texts(li, ts) {
                    groups += 1;
                }
            }
            if groups > 1 {
                app.doc.wrap_recent("Forget text style", groups);
            }
            app.doc.touch();
            app.set_status(format!("style \"{name}\" forgotten — text keeps its look"));
            app.mark_dirty();
        }
        AppCmd::TextStyleAllPages => {
            let (pages, items) = app.apply_text_styles_other_pages();
            app.set_status(format!(
                "styles pushed to the whole work: {items} text(s) on {pages} other page(s) \
                 restyled (saved directly — undo covers this page only)"
            ));
            app.mark_dirty();
        }
        AppCmd::TextStyleAssign { layer, item, name } => {
            let style = name
                .as_deref()
                .and_then(|n| app.doc.text_styles.iter().find(|s| s.name == n))
                .cloned();
            let Some(ts) = app.doc.layers.get(layer).and_then(|l| l.texts()) else {
                return;
            };
            if item >= ts.texts.len() {
                return;
            }
            app.warm_texts(layer);
            let mut ts = app.doc.layers[layer].texts().unwrap().clone();
            match (&name, style) {
                (Some(_), Some(s)) => {
                    let dpi = app.doc_dpi();
                    s.apply(&mut ts.texts[item]);
                    if let Some(engine) = app.text_engine.as_ref() {
                        ts.texts[item].cache = engine.render(&ts.texts[item], dpi).ok().flatten();
                    }
                }
                _ => ts.texts[item].style = None,
            }
            if app.doc.set_texts(layer, ts) {
                app.set_status(match &name {
                    Some(n) => format!("text follows style \"{n}\""),
                    None => "text detached from its style".into(),
                });
                app.mark_dirty();
            }
        }
        AppCmd::TextCommit { layer, texts } => {
            if app.doc.set_texts(layer, texts) {
                app.mark_dirty();
            }
        }
        AppCmd::TextDelete { layer, text } => {
            app.cancel_text_edit();
            if let Some(ts) = app.doc.layers.get(layer).and_then(|l| l.texts()) {
                let mut ts = ts.clone();
                if text < ts.texts.len() {
                    ts.texts.remove(text);
                    app.warm_texts(layer);
                    app.doc.set_texts(layer, ts);
                    app.text_sel = None;
                    app.set_status("text deleted");
                    app.mark_dirty();
                }
            }
        }
        AppCmd::TextsPatch { layer, patches } => {
            let n = texts_patch(app, layer, &patches);
            if n > 0 {
                app.set_status(format!("{n} text(s) updated"));
                app.mark_dirty();
            }
        }
        AppCmd::TextsAdd { layer, items } => {
            let ids = texts_add(app, layer, items);
            if !ids.is_empty() {
                app.set_status(format!("{} text(s) added", ids.len()));
                app.mark_dirty();
            }
        }
        AppCmd::TextsRemove { layer, ids } => {
            let n = texts_remove(app, layer, &ids);
            if n > 0 {
                app.set_status(format!("{n} text(s) removed"));
                app.mark_dirty();
            }
        }
        other => return edit::run(app, other, cmd_tail),
    }
    run_cmd_tail(app, cmd_tail);
}
