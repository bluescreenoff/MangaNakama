//! Batch layer operations (the ROADMAP's "recordable actions" pain list:
//! rename, renumber, apply tone, export — not macro recording for its own
//! sake). One dialog: pick a SCOPE (all layers / the active folder's
//! children / a name prefix / a name PATTERN), pick an operation, apply.
//!
//! Undo semantics follow the singles they batch: rename is not undoable
//! (neither is a single rename — CSP parity), tone changes land as ONE
//! step (`Document::set_tone_many`, a `Compound` transaction), the draft
//! flag / layer colour / blend mode are display state their single-layer
//! commands do not record either, and export writes files and touches
//! nothing.
//!
//! ALL PAGES (2026-08-21) is a modifier on the scope, not a scope: the
//! open page takes the operation through the normal doors, every OTHER
//! page is decoded from its `PageEntry::bytes`, edited and re-encoded in
//! place — the round trip `AppCmd::CompApplyAllPages` already uses. Those
//! pages are written back DIRECTLY: undo covers the open page only, and
//! the dialog says so rather than pretending otherwise.

use std::path::{Path, PathBuf};

use super::App;
use mn_core::{Blend, Document, tone::ToneParams};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum BatchScope {
    #[default]
    AllLayers,
    /// Children of the ACTIVE folder (the active layer must be one).
    FolderChildren,
    /// Layers whose name starts with the typed prefix.
    Prefix,
    /// Layers whose name matches a wildcard pattern ([`name_matches`]).
    Pattern,
    /// The palette's multi-selection (active row + Ctrl/Shift-picked rows).
    /// With nothing multi-selected that is just the active layer, which is
    /// what CSP means by "the selection" of one row.
    Selected,
}

impl BatchScope {
    /// Can this scope mean anything on a page that is not open? "All" and
    /// the two name tests read only the layer list, so they can. The
    /// folder scope keys on the ACTIVE folder and the selection scope on
    /// the palette's picked rows — both are properties of the page being
    /// edited, and guessing an equivalent on page 12 would be inventing
    /// intent.
    pub fn travels_to_other_pages(self) -> bool {
        matches!(self, Self::AllLayers | Self::Prefix | Self::Pattern)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum BatchOp {
    /// Rename every match with the pattern (`{n}` = 1-based counter,
    /// `{name}` = the current name). "Panel {n}" renumbers.
    #[default]
    Rename,
    /// Copy the ACTIVE layer's tone params onto every match.
    ToneFromActive,
    /// Remove tone from every match.
    ToneClear,
    /// Set or clear the draft flag on every match.
    Draft,
    /// Set or clear the display colour (layer colour) on every match.
    Colour,
    /// Set the blend mode on every match.
    BlendMode,
    /// Write each match as a full-canvas PNG into a chosen folder.
    ExportPngs,
}

impl BatchOp {
    /// Export resolves through a folder dialog and writes files for the
    /// open page; there is nothing sane for it to do to the other pages.
    fn travels_to_other_pages(self) -> bool {
        !matches!(self, Self::ExportPngs)
    }
}

/// The layer-colour chip set (CSP's default two-tone palette). Same eight
/// values the Layers palette offers for a single layer (`ui::layers`'
/// `LAYER_TINTS`) — kept here because the batch dialog must not depend on
/// the palette's private chrome.
pub const BATCH_TINTS: [[u8; 3]; 8] = [
    [0x2a, 0x6f, 0xf4], // blue
    [0xe5, 0x4b, 0x4b], // red
    [0x3f, 0xb2, 0x5e], // green
    [0xf2, 0xb8, 0x1c], // amber
    [0x9b, 0x59, 0xd0], // purple
    [0xe8, 0x7e, 0xb5], // pink
    [0x26, 0xc6, 0xc9], // cyan
    [0x8a, 0x8f, 0x98], // grey
];

#[derive(Default)]
pub struct BatchOps {
    pub open: bool,
    pub scope: BatchScope,
    pub prefix: String,
    pub op: BatchOp,
    pub pattern: String,
    /// `BatchScope::Pattern`'s needle — "sketch", "*sketch*", "rough_*".
    pub name_pat: String,
    /// `BatchOp::Draft`: on = mark as draft, off = clear the flag.
    pub draft_on: bool,
    /// `BatchOp::Colour`: `None` = back to stock (no display tint).
    pub colour: Option<[u8; 3]>,
    /// `BatchOp::BlendMode`'s target mode.
    pub blend: Blend,
    /// Apply to every page of the work, not only the open one.
    pub all_pages: bool,
}

impl BatchOps {
    /// Is the "all pages" box actually in force? Ticking it under a scope
    /// or operation that cannot travel means nothing — the dialog disables
    /// the box there, and this is the same answer for the apply path, so a
    /// stale tick from a previous scope cannot leak through.
    pub fn all_pages_live(&self) -> bool {
        self.all_pages && self.scope.travels_to_other_pages() && self.op.travels_to_other_pages()
    }
}

/// Wildcard name test — case-insensitive, `*` matches any run of
/// characters (including none).
///
/// A pattern with NO `*` is a plain substring test: typing `sketch` finds
/// `Sketch`, `rough sketch 3` and `SKETCH_A` without anyone having to
/// learn glob syntax. The moment a `*` appears the pattern is anchored at
/// both ends instead, so `sketch*` means "starts with", `*sketch` means
/// "ends with", and `a*b*c` means the three pieces in that order. An
/// empty pattern matches nothing (the same refusal the prefix scope makes
/// — a batch that silently means "everything" is how art gets flattened).
///
/// Hand-rolled on purpose: this is twenty lines, a regex crate is a
/// dependency and a syntax the owner did not ask for.
pub fn name_matches(pattern: &str, name: &str) -> bool {
    let pat = pattern.to_lowercase();
    let hay = name.to_lowercase();
    if pat.is_empty() {
        return false;
    }
    if !pat.contains('*') {
        return hay.contains(&pat);
    }
    // `split('*')` always yields at least two pieces here. The first
    // anchors the start, the last anchors the end, the rest must occur in
    // order in between; empty pieces (`**`, a leading or trailing `*`)
    // fall out of the same arithmetic for free.
    let parts: Vec<&str> = pat.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !hay.starts_with(first) {
        return false;
    }
    // `starts_with` proved `first` is a byte prefix, so this slice sits on
    // a char boundary even when either side is multi-byte.
    let mut rest = &hay[first.len()..];
    for m in &parts[1..parts.len() - 1] {
        match rest.find(m) {
            Some(p) => rest = &rest[p + m.len()..],
            None => return false,
        }
    }
    // The end anchor may not re-use characters an earlier piece consumed:
    // `a*a` matches "aa", not "a".
    rest.len() >= last.len() && rest.ends_with(last)
}

/// The layer indices a scope selects inside ONE document, bottom-to-top.
/// Split out from [`App::batch_matches`] because the all-pages run asks
/// the same question of every other page's decoded document.
pub fn matches_in(doc: &Document, b: &BatchOps) -> Vec<usize> {
    let ordinary = |i: &usize| doc.layers.get(*i).is_some_and(|l| !l.folder);
    match b.scope {
        BatchScope::AllLayers => (0..doc.layers.len()).filter(ordinary).collect(),
        BatchScope::FolderChildren => {
            let hi = doc.active;
            let Some(header) = doc.layers.get(hi) else {
                return Vec::new();
            };
            if !header.folder {
                return Vec::new();
            }
            // Children sit BELOW the header, at greater depth, until
            // the depth returns to the header's.
            let mut out = Vec::new();
            for i in (0..hi).rev() {
                let l = &doc.layers[i];
                if l.depth <= header.depth {
                    break;
                }
                if !l.folder {
                    out.push(i);
                }
            }
            out.reverse();
            out
        }
        BatchScope::Prefix => {
            let p = b.prefix.trim();
            if p.is_empty() {
                return Vec::new();
            }
            (0..doc.layers.len())
                .filter(ordinary)
                .filter(|&i| doc.layers[i].name.starts_with(p))
                .collect()
        }
        BatchScope::Pattern => {
            let p = b.name_pat.trim();
            (0..doc.layers.len())
                .filter(ordinary)
                .filter(|&i| name_matches(p, &doc.layers[i].name))
                .collect()
        }
        BatchScope::Selected => doc.multi_targets().into_iter().filter(ordinary).collect(),
    }
}

/// The operation with its payload already read off the dialog (and, for
/// `ToneFromActive`, off the OPEN page's active layer): every page then
/// applies the identical edit rather than re-deciding per page.
#[derive(Clone)]
enum Resolved {
    Rename(String),
    Tone(Option<ToneParams>),
    Draft(bool),
    Colour(Option<[u8; 3]>),
    BlendMode(Blend),
}

/// Rename top-to-bottom: artists count panels from the top of the stack,
/// so `{n}` = 1 is the topmost match.
fn rename_matches(doc: &mut Document, matches: &[usize], pattern: &str) -> usize {
    let mut n_done = 0;
    for (n, &i) in matches.iter().rev().enumerate() {
        let Some(old) = doc.layers.get(i).map(|l| l.name.clone()) else {
            continue;
        };
        let name = pattern
            .replace("{n}", &(n + 1).to_string())
            .replace("{name}", &old);
        if doc.rename_layer(i, name) {
            n_done += 1;
        }
    }
    n_done
}

/// Apply a resolved operation to a document that is NOT the open page.
///
/// These are the same `Document` setters the single-layer command arms
/// call (`AppCmd::SetLayerDraft` → `Document::set_layer_draft`, and so
/// on); what the arms add on top — status line, `mark_dirty`, tile
/// eviction — is live-document housekeeping a stashed page has none of.
/// Nothing here shifts a layer index, so the decoded document's own undo
/// history (which is thrown away with the document) stays consistent.
fn apply_to_page_doc(doc: &mut Document, matches: &[usize], r: &Resolved) -> usize {
    match r {
        Resolved::Rename(p) => rename_matches(doc, matches, p),
        Resolved::Tone(t) => doc.set_tone_many(matches, *t),
        Resolved::Draft(v) => matches
            .iter()
            .filter(|&&i| doc.set_layer_draft(i, *v))
            .count(),
        Resolved::Colour(c) => matches
            .iter()
            .filter(|&&i| doc.set_layer_colour(i, *c))
            .count(),
        Resolved::BlendMode(b) => matches
            .iter()
            .filter(|&&i| doc.set_layer_blend(i, *b))
            .count(),
    }
}

impl App {
    /// The layer indices the current scope selects, bottom-to-top.
    /// Folder headers themselves are never matched (renaming or toning a
    /// header from a batch is a surprise, not a service).
    pub fn batch_matches(&self) -> Vec<usize> {
        matches_in(&self.doc, &self.batch)
    }

    /// Apply the non-export operations. Returns a status line.
    pub fn batch_apply(&mut self) -> String {
        let all_pages = self.batch.all_pages_live();
        let matches = self.batch_matches();
        if matches.is_empty() && !all_pages {
            return "batch: nothing matches that scope".into();
        }
        // Read the dialog ONCE. The all-pages run then repeats exactly
        // this edit; re-deciding per page would let, say, the tone source
        // drift to whatever each page happens to call its active layer.
        let resolved = match self.batch.op {
            BatchOp::Rename => {
                let pattern = self.batch.pattern.clone();
                if pattern.trim().is_empty() {
                    return "batch: the rename pattern is empty".into();
                }
                Resolved::Rename(pattern)
            }
            BatchOp::ToneFromActive => {
                let Some(tone) = self
                    .doc
                    .layers
                    .get(self.doc.active)
                    .and_then(|l| l.tone)
                    .map(Some)
                else {
                    return "batch: the active layer has no tone to copy".into();
                };
                Resolved::Tone(tone)
            }
            BatchOp::ToneClear => Resolved::Tone(None),
            BatchOp::Draft => Resolved::Draft(self.batch.draft_on),
            BatchOp::Colour => Resolved::Colour(self.batch.colour),
            BatchOp::BlendMode => Resolved::BlendMode(self.batch.blend),
            BatchOp::ExportPngs => {
                // Resolved through the folder dialog; the dispatch routes
                // to `batch_export_pngs` with the picked path.
                return String::new();
            }
        };
        let mut msg = self.batch_apply_here(&matches, &resolved);
        if all_pages {
            msg.push_str(&self.batch_other_pages(&resolved));
        }
        msg
    }

    /// The OPEN page's half of an apply: every change goes through the
    /// door its single-layer command uses, so the caches those arms carry
    /// (redraw, tile eviction, derived-raster refresh) all fire.
    fn batch_apply_here(&mut self, matches: &[usize], r: &Resolved) -> String {
        use crate::cmd::{AppCmd, dispatch};
        match r {
            Resolved::Rename(pattern) => {
                let n = rename_matches(&mut self.doc, matches, pattern);
                self.mark_dirty();
                format!("batch: renamed {n} layers")
            }
            Resolved::Tone(tone) => {
                let n = self.doc.set_tone_many(matches, *tone);
                for &i in matches {
                    self.renderer.evict_layer(i);
                }
                self.refresh_tones();
                self.mark_dirty();
                let what = if tone.is_some() {
                    "tone applied to"
                } else {
                    "tone removed from"
                };
                format!("batch: {what} {n} layers (one undo step)")
            }
            // Draft / colour / blend are display state: the single-layer
            // commands record no undo step either, so neither does the
            // batch. The dialog says so out loud instead of implying an
            // undo that is not there.
            Resolved::Draft(v) => {
                for &i in matches {
                    dispatch(self, AppCmd::SetLayerDraft(i, *v));
                }
                let what = if *v { "marked" } else { "cleared" };
                format!("batch: draft flag {what} on {} layers", matches.len())
            }
            Resolved::Colour(c) => {
                for &i in matches {
                    dispatch(self, AppCmd::SetLayerColour(i, *c));
                }
                let what = if c.is_some() { "set on" } else { "cleared on" };
                format!("batch: layer colour {what} {} layers", matches.len())
            }
            Resolved::BlendMode(b) => {
                for &i in matches {
                    dispatch(self, AppCmd::SetLayerBlend(i, *b));
                }
                format!("batch: blend mode set on {} layers", matches.len())
            }
        }
    }

    /// Apply the same edit to every OTHER page of the work, in place.
    ///
    /// The round trip is `AppCmd::CompApplyAllPages`': stash the open page
    /// first (so nothing in flight is lost), then decode each other page's
    /// bytes, edit the decoded document, re-encode. The decoded documents
    /// never become `self.doc`, so the `adopt_page_doc` ruler trap cannot
    /// bite — rulers are session state that only matters to a document the
    /// tab is editing. Each touched page takes a fresh content revision
    /// (the folder save's skip hint, and the key the sharp preview caches
    /// on) and drops its thumbnail, which the Pages panel rebuilds lazily
    /// from the new bytes.
    ///
    /// These writes are DIRECT: undo covers the open page only.
    fn batch_other_pages(&mut self, r: &Resolved) -> String {
        if let Err(e) = self.stash_current_page() {
            return format!(" — other pages skipped: {e}");
        }
        let (mut pages, mut layers, mut failed) = (0usize, 0usize, 0usize);
        for i in 0..self.pages.len() {
            if i == self.page_index {
                continue; // the live document already took it
            }
            let Some(bytes) = self.pages[i].bytes.as_deref() else {
                failed += 1;
                continue;
            };
            let Ok(mut doc) = mn_core::project::bytes_to_doc(bytes) else {
                failed += 1;
                continue;
            };
            let m = matches_in(&doc, &self.batch);
            if m.is_empty() {
                continue;
            }
            let n = apply_to_page_doc(&mut doc, &m, r);
            if n == 0 {
                continue;
            }
            let Ok(nb) = mn_core::project::doc_to_bytes(&doc) else {
                failed += 1;
                continue;
            };
            let rev = self.page_rev_next();
            let e = &mut self.pages[i];
            e.bytes = Some(nb);
            e.rev = rev;
            e.doc_rev = 0;
            e.thumb = None;
            pages += 1;
            layers += n;
        }
        self.mark_pages_dirty();
        self.mark_dirty();
        let mut s = format!("; {layers} more layers on {pages} other pages (saved directly)");
        if failed > 0 {
            s.push_str(&format!(" — {failed} page(s) could not be read"));
        }
        s
    }

    /// Write every match as `<NN>-<name>.png` (full canvas, straight
    /// alpha) into `dir`. Numbering is top-to-bottom like the renamer.
    pub fn batch_export_pngs(&mut self, dir: &Path) -> String {
        self.refresh_tones();
        let matches = self.batch_matches();
        if matches.is_empty() {
            return "batch: nothing matches that scope".into();
        }
        let mut written = 0usize;
        for (n, &i) in matches.iter().rev().enumerate() {
            let img = layer_png(self, i);
            let safe: String = self.doc.layers[i]
                .name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let path = dir.join(format!("{:02}-{}.png", n + 1, safe));
            if img.save(&path).is_ok() {
                written += 1;
            }
        }
        format!("batch: {written} layer PNGs -> {}", dir.display())
    }
}

/// One layer alone, full canvas, straight alpha — through the display path
/// (derived rasters included; `refresh_tones` must have run).
fn layer_png(app: &App, li: usize) -> image::RgbaImage {
    let (w, h) = app.doc.size;
    let mut img = image::RgbaImage::new(w, h);
    let l = &app.doc.layers[li];
    for (idx, t) in l.display_tiles() {
        let (ox, oy) = idx.origin();
        for py in 0..mn_core::TILE_SIZE {
            let y = oy + py as i32;
            if y < 0 || y >= h as i32 {
                continue;
            }
            for px in 0..mn_core::TILE_SIZE {
                let x = ox + px as i32;
                if x < 0 || x >= w as i32 {
                    continue;
                }
                let p = t.pixel(px, py);
                let a = p[3] as u32;
                if a == 0 {
                    continue;
                }
                let un = |c: u16| (((c as u32 * 32768 / a).min(32768) * 255 + 16384) / 32768) as u8;
                img.put_pixel(
                    x as u32,
                    y as u32,
                    image::Rgba([un(p[0]), un(p[1]), un(p[2]), ((a * 255 + 16384) / 32768) as u8]),
                );
            }
        }
    }
    img
}

/// Export dir memory for the pump (rfd runs in main.rs).
pub fn _dir_placeholder() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use mn_core::{TileIdx, tone::ToneParams};

    fn app() -> Option<App> {
        let mut app = App::new(super::super::headless_renderer()?, (600, 400), 1.0);
        // Three layers + a folder with one child.
        app.doc.rename_layer(0, "base");
        app.doc.add_layer("Panel a");
        app.doc.add_layer("Panel b");
        let f = app.doc.add_folder_above(app.doc.active, "F");
        app.doc.add_layer_in_folder(f, "inner");
        Some(app)
    }

    #[test]
    fn scopes_select_the_right_layers() {
        let Some(mut app) = app() else { return };
        app.batch.scope = BatchScope::AllLayers;
        let names = |app: &App, idxs: &[usize]| -> Vec<String> {
            idxs.iter().map(|&i| app.doc.layers[i].name.clone()).collect()
        };
        let m = app.batch_matches();
        assert_eq!(
            names(&app, &m),
            vec!["base", "Panel a", "Panel b", "inner"],
            "all layers, no folder headers"
        );
        app.batch.scope = BatchScope::Prefix;
        app.batch.prefix = "Panel".into();
        assert_eq!(names(&app, &app.batch_matches()), vec!["Panel a", "Panel b"]);
        // Folder children: select the folder header first.
        let f = app
            .doc
            .layers
            .iter()
            .position(|l| l.folder)
            .unwrap();
        app.doc.set_active(f);
        app.batch.scope = BatchScope::FolderChildren;
        assert_eq!(names(&app, &app.batch_matches()), vec!["inner"]);
    }

    /// The palette multi-selection is a scope of its own: exactly the
    /// picked rows, folder headers dropped like every other scope, and an
    /// empty multi-selection meaning the active layer alone.
    #[test]
    fn selected_scope_follows_the_palette() {
        let Some(mut app) = app() else { return };
        let names = |app: &App, idxs: &[usize]| -> Vec<String> {
            idxs.iter().map(|&i| app.doc.layers[i].name.clone()).collect()
        };
        let idx = |app: &App, n: &str| app.doc.layers.iter().position(|l| l.name == n).unwrap();
        app.batch.scope = BatchScope::Selected;

        // Two of the three ordinary layers, Ctrl+click style.
        let (a, b) = (idx(&app, "Panel a"), idx(&app, "Panel b"));
        app.doc.set_active(a);
        assert!(app.doc.toggle_multi(b));
        assert_eq!(names(&app, &app.batch_matches()), vec!["Panel a", "Panel b"]);

        // A folder header in the selection is not a match.
        let f = idx(&app, "F");
        assert!(app.doc.toggle_multi(f));
        assert!(app.doc.multi_targets().contains(&f), "header really is selected");
        assert_eq!(
            names(&app, &app.batch_matches()),
            vec!["Panel a", "Panel b"],
            "folder headers never match"
        );

        // Nothing multi-selected = the active layer alone.
        app.doc.set_active(idx(&app, "base"));
        assert!(app.doc.layer_multi.is_empty());
        assert_eq!(names(&app, &app.batch_matches()), vec!["base"]);
    }

    /// The hand-rolled wildcard matcher, which is the whole of the
    /// "layers with sketch in the name" ask.
    #[test]
    fn wildcard_matcher_anchors_and_ignores_case() {
        // No star = substring, anywhere, either case.
        assert!(name_matches("sketch", "Sketch"));
        assert!(name_matches("SKETCH", "rough sketch 3"));
        assert!(name_matches("etc", "sketch"));
        assert!(!name_matches("sketch", "inks"));
        // An empty pattern matches nothing — never "everything".
        assert!(!name_matches("", "sketch"));
        assert!(!name_matches("   ", "sketch"));

        // A star anchors both ends of what is left.
        assert!(name_matches("sketch*", "sketch A"), "prefix");
        assert!(!name_matches("sketch*", "rough sketch"), "prefix anchors");
        assert!(name_matches("*sketch", "rough sketch"), "suffix");
        assert!(!name_matches("*sketch", "sketch A"), "suffix anchors");
        assert!(name_matches("*sketch*", "a sketch b"), "middle");
        assert!(name_matches("*", "anything at all"));
        assert!(name_matches("SK*CH", "sketch"), "case-insensitive anchors");

        // Several stars: the pieces must appear in order.
        assert!(name_matches("rough*sketch*v2", "rough_sketch_final_v2"));
        assert!(!name_matches("rough*sketch*v2", "rough_v2_sketch"));
        assert!(name_matches("a*b*c*d", "abcd"), "adjacent pieces");

        // The end anchor may not re-use what the start anchor ate.
        assert!(name_matches("a*a", "aa"));
        assert!(!name_matches("a*a", "a"));

        // Multi-byte names must not panic on the byte slicing.
        assert!(name_matches("コマ*", "コマ 1 (Panel b)"));
        assert!(name_matches("*ネーム", "下描きネーム"));
        assert!(!name_matches("ネーム*", "下描きネーム"));
    }

    /// The owner's ask, end to end: every layer whose name carries
    /// "sketch" becomes a draft layer in one gesture — and nothing else
    /// does. Draft is display state, so like the single-layer toggle it
    /// records no undo step; the test pins that promise too, because a
    /// batch that quietly pushed N steps would be the bug.
    #[test]
    fn wildcard_draft_batch_flags_only_the_matches() {
        let Some(mut app) = app() else { return };
        app.doc.rename_layer(0, "rough sketch");
        app.doc.add_layer("SKETCH_final");
        app.doc.add_layer("inks");
        let steps = app.doc.undo_len();

        app.batch.scope = BatchScope::Pattern;
        app.batch.name_pat = "*sketch*".into();
        app.batch.op = BatchOp::Draft;
        app.batch.draft_on = true;
        let m = app.batch_matches();
        assert_eq!(m.len(), 2, "both spellings, no folder header");
        let s = app.batch_apply();
        assert!(s.contains("draft flag marked on 2"), "{s}");

        let drafted: Vec<String> = app
            .doc
            .layers
            .iter()
            .filter(|l| l.draft)
            .map(|l| l.name.clone())
            .collect();
        assert_eq!(drafted, vec!["rough sketch", "SKETCH_final"]);
        assert_eq!(
            app.doc.undo_len(),
            steps,
            "display flags record no undo step, exactly like the single toggle"
        );

        // And the clearing direction takes them all back.
        app.batch.draft_on = false;
        app.batch_apply();
        assert!(!app.doc.layers.iter().any(|l| l.draft));
    }

    /// Layer colour and blend take the same road, so one test covers the
    /// remaining two operations (the payload comes off the dialog).
    #[test]
    fn colour_and_blend_batches_reach_every_match() {
        let Some(mut app) = app() else { return };
        app.batch.scope = BatchScope::Prefix;
        app.batch.prefix = "Panel".into();

        app.batch.op = BatchOp::Colour;
        app.batch.colour = Some(BATCH_TINTS[1]);
        let s = app.batch_apply();
        assert!(s.contains("layer colour set on 2"), "{s}");
        let tinted = |app: &App| {
            app.doc
                .layers
                .iter()
                .filter(|l| l.layer_colour == Some(BATCH_TINTS[1]))
                .count()
        };
        assert_eq!(tinted(&app), 2);

        app.batch.op = BatchOp::BlendMode;
        app.batch.blend = mn_core::Blend::Multiply;
        let s = app.batch_apply();
        assert!(s.contains("blend mode set on 2"), "{s}");
        assert_eq!(
            app.doc
                .layers
                .iter()
                .filter(|l| l.blend == mn_core::Blend::Multiply)
                .count(),
            2
        );
        // The non-matching layers were left alone by both.
        assert!(app.doc.layers.iter().any(|l| l.name == "base"
            && l.layer_colour.is_none()
            && l.blend == mn_core::Blend::Normal));

        // Back to stock clears the tint.
        app.batch.op = BatchOp::Colour;
        app.batch.colour = None;
        let s = app.batch_apply();
        assert!(s.contains("layer colour cleared on 2"), "{s}");
        assert_eq!(tinted(&app), 0);
    }

    /// "All layers in ALL PAGES": the open page rides the normal path and
    /// every other page is decoded, edited and re-encoded in place. Also
    /// pins the refusal — a scope that only means something on the open
    /// page must not travel.
    #[test]
    fn all_pages_batch_reaches_the_stashed_pages() {
        let Some(mut app) = app() else { return };
        // A second page with its own sketch layer, stashed as bytes the
        // way every non-open page is held.
        let mut p2 = app.blank_page_doc();
        p2.add_layer("Sketch two");
        p2.add_layer("inks two");
        let bytes = mn_core::project::doc_to_bytes(&p2).expect("encode page 2");
        let e = app.fresh_page(Some(bytes), None);
        app.pages.push(e);
        app.doc.rename_layer(0, "sketch one");

        app.batch.scope = BatchScope::Pattern;
        app.batch.name_pat = "sketch".into();
        app.batch.op = BatchOp::Draft;
        app.batch.draft_on = true;
        app.batch.all_pages = true;
        assert!(app.batch.all_pages_live(), "a name scope travels");
        let s = app.batch_apply();
        assert!(s.contains("draft flag marked on 1"), "open page: {s}");
        assert!(s.contains("1 more layers on 1 other pages"), "page 2: {s}");

        // Page 2's stashed bytes really carry the flag now.
        let b = app.pages[1].bytes.as_ref().expect("page 2 still stashed");
        let d = mn_core::project::bytes_to_doc(b).expect("decode page 2");
        let drafted: Vec<&str> = d
            .layers
            .iter()
            .filter(|l| l.draft)
            .map(|l| l.name.as_str())
            .collect();
        assert_eq!(drafted, vec!["Sketch two"], "only the match, other page");
        assert!(app.pages[1].thumb.is_none(), "stale thumbnail dropped");

        // The palette-selection scope cannot travel: another page has no
        // picked rows to mean.
        app.batch.scope = BatchScope::Selected;
        assert!(!app.batch.all_pages_live());
    }

    #[test]
    fn rename_pattern_numbers_top_down() {
        let Some(mut app) = app() else { return };
        app.batch.scope = BatchScope::Prefix;
        app.batch.prefix = "Panel".into();
        app.batch.op = BatchOp::Rename;
        app.batch.pattern = "コマ {n} ({name})".into();
        let s = app.batch_apply();
        assert!(s.contains("renamed 2"), "{s}");
        // Top of the stack is {n}=1: "Panel b" sits above "Panel a".
        let by_name = |app: &App, n: &str| app.doc.layers.iter().any(|l| l.name == n);
        assert!(by_name(&app, "コマ 1 (Panel b)"));
        assert!(by_name(&app, "コマ 2 (Panel a)"));
    }

    /// Batch tone = ONE undo step across every match (the Compound
    /// transaction), and one undo takes it all back.
    #[test]
    fn batch_tone_is_one_undo_step() {
        let Some(mut app) = app() else { return };
        // Give the active layer a tone to copy.
        let li = app.doc.active;
        assert!(app.doc.set_tone(li, Some(ToneParams::default())));
        let steps = app.doc.undo_len();
        app.batch.scope = BatchScope::Prefix;
        app.batch.prefix = "Panel".into();
        app.batch.op = BatchOp::ToneFromActive;
        let s = app.batch_apply();
        assert!(s.contains("2 layers"), "{s}");
        assert_eq!(app.doc.undo_len(), steps + 1, "one step for the batch");
        let toned = |app: &App| {
            app.doc
                .layers
                .iter()
                .filter(|l| l.name.starts_with("Panel") && l.tone.is_some())
                .count()
        };
        assert_eq!(toned(&app), 2);
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Undo);
        assert_eq!(toned(&app), 0, "one undo clears the whole batch");
        crate::cmd::dispatch(&mut app, crate::cmd::AppCmd::Redo);
        assert_eq!(toned(&app), 2);
    }

    #[test]
    fn export_writes_one_png_per_match() {
        let Some(mut app) = app() else { return };
        const W: u16 = mn_core::FIX15_ONE as u16;
        // Ink the two panels so the files carry pixels.
        for name in ["Panel a", "Panel b"] {
            let i = app.doc.layers.iter().position(|l| l.name == name).unwrap();
            app.doc.set_active(i);
            app.doc.begin_op();
            app.doc
                .active_layer_mut()
                .tile_mut(TileIdx::new(0, 0))
                .set_pixel(3, 4, [W, W, W, W]);
            app.doc.end_op();
        }
        app.batch.scope = BatchScope::Prefix;
        app.batch.prefix = "Panel".into();
        let dir = std::env::temp_dir().join(format!("mn-batch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = app.batch_export_pngs(&dir);
        assert!(s.contains("2 layer PNGs"), "{s}");
        let img = image::open(dir.join("01-Panel_b.png")).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), app.doc.size, "full canvas, whatever it is");
        assert!(img.get_pixel(3, 4)[3] > 0);
        std::fs::remove_dir_all(&dir).ok();
    }
}
