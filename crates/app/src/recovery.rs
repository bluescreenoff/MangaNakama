//! Crash recovery — the autosave's missing second half (PR-040).
//!
//! We have had a 15-minute autosave for a long time and **nothing ever
//! offered it back**. A timer that writes a file no one reads is not a
//! safety net; it is a file. This module is the read side.
//!
//! # How we know a crash happened
//!
//! Not by a lock file or a sentinel we have to remember to clear — by the
//! log we already write. `testlog` appends `=== exited cleanly ===` as the
//! last thing a normal shutdown does, so a last session block WITHOUT that
//! marker did not finish. That covers the class no Rust hook can catch
//! (a stack overflow is killed by the OS outright), which is exactly the
//! class that costs someone their page.
//!
//! # What we will and will not offer
//!
//! Only an autosave that is **newer than the file it shadows**. A stale
//! `.autosave.mnc` next to a document the user has since saved describes
//! an older state; offering it invites overwriting good work with bad,
//! which is a worse failure than the crash was. The unsaved-document
//! autosave in `%TEMP%` shadows nothing and is always eligible — that is
//! the case where the alternative is losing everything.
//!
//! Work folders are absent from all of this on purpose: they autosave IN
//! PLACE, page by page, so a crash leaves the folder already current and
//! there is nothing to offer back.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The extension the autosave writes beside a saved document. **One
/// definition**: the writer in `cmd.rs` and the reader here must agree, and
/// two string literals in two files is precisely how a recovery feature ends
/// up looking for a file nothing writes.
pub const AUTOSAVE_EXT: &str = "autosave.mnc";

/// Where a never-saved document's autosave goes — it has no folder of its
/// own to live in.
pub fn unsaved_autosave_path() -> PathBuf {
    std::env::temp_dir().join("MangaNakama-autosave.mnc")
}

/// The autosave that shadows a saved document.
pub fn sibling_autosave(doc: &Path) -> PathBuf {
    doc.with_extension(AUTOSAVE_EXT)
}

/// True when the LAST session block in `log` has no clean-exit marker.
///
/// Pure, because this is the decision the whole feature turns on: get it
/// wrong in the false direction and we nag after every normal start; wrong
/// in the true direction and the crash we exist for goes unmentioned.
pub fn last_block_crashed(log: &str) -> bool {
    match log.rfind("=== session ") {
        None => false,
        Some(i) => !log[i..].contains("=== exited cleanly ==="),
    }
}

/// Read the log as it stands BEFORE this session appends to it. Call once,
/// early in `main`, ahead of `testlog::begin_session`.
pub fn last_session_crashed() -> bool {
    existing_log()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|s| last_block_crashed(&s))
}

/// The log's resolved path, WITHOUT creating it — `testlog`'s own resolver
/// opens for append, which would manufacture an empty log and answer its own
/// question.
fn existing_log() -> Option<PathBuf> {
    const NAME: &str = "manganakama.log";
    if let Some(p) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(NAME)))
        && p.is_file()
    {
        return Some(p);
    }
    let p = PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
        .join("MangaNakama")
        .join(NAME);
    p.is_file().then_some(p)
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

/// The autosave worth offering back, or `None`.
///
/// `recents` is the MRU list; `temp` is where the unsaved-document autosave
/// lives (a parameter so the rule can be tested against a scratch directory
/// instead of the real `%TEMP%`). Candidates:
///
/// * `temp/MangaNakama-autosave.mnc` — shadows nothing, always eligible.
/// * `temp/MangaNakama-autosave[-N]/work.mnc` — same, as an incremental
///   work folder (05 item 1); ranked by the index's own mtime.
/// * for each recent document, its sibling `.autosave.mnc` — eligible only
///   while it is NEWER than the document itself.
///
/// Work-folder indexes (`work.mnc`) are skipped: they save in place.
/// The newest survivor wins, because that is the one closest to what was on
/// screen when the process died.
pub fn newest_autosave(recents: &[PathBuf], temp: &Path) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    let mut consider = |p: PathBuf, t: SystemTime| match &best {
        Some((bt, _)) if *bt >= t => {}
        _ => best = Some((t, p)),
    };

    // Every never-saved document's stash, one per tab slot
    // (`MangaNakama-autosave.mnc`, `-1.mnc`, `-2.mnc`, …). They were a single
    // shared file until 2026-08-20, which meant one unsaved tab could
    // overwrite another's.
    //
    // Since 2026-08-24 (05 item 1) a pathless work autosaves as a work
    // FOLDER (`MangaNakama-autosave[-N]\work.mnc`) — offered by the index's
    // own mtime. The monolithic spelling is still matched: a pre-fix
    // session may have left one behind.
    if let Ok(entries) = std::fs::read_dir(temp) {
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|n| n.to_str());
            if !name.is_some_and(|n| n.starts_with("MangaNakama-autosave")) {
                continue;
            }
            if p.is_dir() {
                let inner = p.join(mn_core::project::WORKFOLDER_INDEX);
                if let Some(t) = modified(&inner) {
                    consider(inner, t);
                }
            } else if name.is_some_and(|n| n.ends_with(".mnc"))
                && let Some(t) = modified(&p)
            {
                consider(p, t);
            }
        }
    }

    for doc in recents {
        if doc
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case(mn_core::project::WORKFOLDER_INDEX))
        {
            continue;
        }
        let side = sibling_autosave(doc);
        let Some(t) = modified(&side) else { continue };
        // Newer than the document it shadows, or the document is gone.
        let fresh = modified(doc).is_none_or(|d| t > d);
        if fresh {
            consider(side, t);
        }
    }

    best.map(|(_, p)| p)
}

/// Drop the autosave that shadows `doc` — call after a SUCCESSFUL save of
/// `doc`, where the saved file is by definition the better copy. Without
/// this, a crash weeks later offers a stale autosave that predates the work
/// the user actually kept.
pub fn clear_sibling_autosave(doc: &Path) {
    let _ = std::fs::remove_file(sibling_autosave(doc));
}

/// Drop a never-saved document's `%TEMP%` stash. Call it when that document
/// gains a real path: the stash is superseded the moment there is a file.
///
/// Without this the stash lives forever, and `newest_autosave` — which
/// treats it as "shadows nothing, always eligible" — happily offers a
/// months-old scratch document after an unrelated crash, under a dialog that
/// claims it is newer than the file it belongs to.
pub fn clear_unsaved_stash(slot: usize) {
    // Both spellings (05 item 1): the pre-2026-08-24 monolithic file and
    // the incremental work folder that replaced it.
    let _ = std::fs::remove_file(crate::app::unsaved_autosave_path_for(slot));
    let index = crate::app::unsaved_autosave_folder_for(slot);
    if let Some(dir) = index.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN: &str = "\
=== session 100 ===
build 1.0
=== exited cleanly ===
";

    #[test]
    fn a_clean_block_is_not_a_crash() {
        assert!(!last_block_crashed(CLEAN));
    }

    #[test]
    fn a_block_without_the_marker_is_a_crash() {
        let log = format!("{CLEAN}\n=== session 200 ===\nbuild 1.0\n!!! PANIC at x.rs:1: boom\n");
        assert!(last_block_crashed(&log));
    }

    /// The one that matters: an OLD crash followed by a clean run must not
    /// keep nagging. Only the LAST block is the question.
    #[test]
    fn an_earlier_crash_is_forgotten_once_a_session_exits_cleanly() {
        let log = format!("=== session 100 ===\ndied here\n{CLEAN}");
        assert!(!last_block_crashed(&log));
    }

    #[test]
    fn no_log_at_all_is_not_a_crash() {
        assert!(!last_block_crashed(""));
        assert!(!last_block_crashed("stray text with no session banner"));
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mn-recover-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &Path, body: &str) {
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn the_unsaved_autosave_is_always_eligible() {
        let d = scratch("unsaved");
        assert_eq!(newest_autosave(&[], &d), None, "nothing written yet");
        let a = d.join("MangaNakama-autosave.mnc");
        touch(&a, "x");
        assert_eq!(newest_autosave(&[], &d), Some(a));
        std::fs::remove_dir_all(&d).ok();
    }

    /// A `.autosave.mnc` OLDER than the document it shadows describes a
    /// state the user has already replaced — offering it would invite
    /// overwriting good work with stale work.
    #[test]
    fn a_stale_sibling_is_not_offered() {
        let d = scratch("stale");
        let doc = d.join("ch1.ora");
        let side = d.join("ch1.autosave.mnc");
        touch(&side, "old");
        std::thread::sleep(std::time::Duration::from_millis(20));
        touch(&doc, "saved after the autosave");

        let empty = d.join("no-temp-here");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(newest_autosave(&[doc.clone()], &empty), None);

        // ...and the same file IS offered once it is the newer one.
        std::thread::sleep(std::time::Duration::from_millis(20));
        touch(&side, "new");
        assert_eq!(newest_autosave(&[doc], &empty), Some(side));
        std::fs::remove_dir_all(&d).ok();
    }

    /// Work folders save in place; there is no shadow copy to restore, and
    /// offering one would be offering the same bytes back.
    #[test]
    fn work_folder_indexes_are_skipped() {
        let d = scratch("folder");
        let index = d.join(mn_core::project::WORKFOLDER_INDEX);
        touch(&index, "index");
        touch(&d.join("work.autosave.mnc"), "should never be offered");
        let empty = d.join("no-temp-here");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(newest_autosave(&[index], &empty), None);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_newest_candidate_wins() {
        let d = scratch("newest");
        let temp = d.join("t");
        std::fs::create_dir_all(&temp).unwrap();
        touch(&temp.join("MangaNakama-autosave.mnc"), "older");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let doc = d.join("ch2.ora");
        let side = d.join("ch2.autosave.mnc");
        touch(&side, "newer");

        assert_eq!(newest_autosave(&[doc], &temp), Some(side));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn clearing_removes_only_the_shadow() {
        let d = scratch("clear");
        let doc = d.join("ch3.ora");
        let side = sibling_autosave(&doc);
        touch(&doc, "real");
        touch(&side, "shadow");
        clear_sibling_autosave(&doc);
        assert!(!side.exists(), "the shadow is gone");
        assert!(doc.exists(), "the document is NOT");
        std::fs::remove_dir_all(&d).ok();
    }

    /// 05 item 1: a pathless work's stash is a work FOLDER now — offered
    /// by its index, ranked by the index's own mtime against the
    /// pre-fix monolithic spelling (which a pre-fix session may still
    /// have left behind).
    #[test]
    fn the_unsaved_folder_stash_is_offered_by_its_index() {
        let d = scratch("folder-stash");
        let mono = d.join("MangaNakama-autosave-5.mnc");
        let dir = d.join("MangaNakama-autosave-5");
        std::fs::create_dir_all(&dir).unwrap();
        let wf = dir.join(mn_core::project::WORKFOLDER_INDEX);
        touch(&mono, "old format, older");
        std::thread::sleep(std::time::Duration::from_millis(20));
        touch(&wf, "new format, newer");
        assert_eq!(newest_autosave(&[], &d), Some(wf.clone()));

        // ...and the reverse ordering: a newer monolithic file wins —
        // ranking is by freshness, not by format.
        std::thread::sleep(std::time::Duration::from_millis(20));
        touch(&mono, "newer now");
        assert_eq!(newest_autosave(&[], &d), Some(mono.clone()));

        // A stash-named folder WITHOUT an index is not a stash.
        std::fs::create_dir_all(d.join("MangaNakama-autosave-6")).unwrap();
        assert_eq!(newest_autosave(&[], &d), Some(mono));
        std::fs::remove_dir_all(&d).ok();
    }

    /// `clear_unsaved_stash` must kill BOTH spellings. It resolves the
    /// real `%TEMP%`, so it runs on a slot no real session fills (30
    /// tabs) and cleans up after itself.
    #[test]
    fn clearing_the_unsaved_stash_removes_folder_and_file() {
        let file = crate::app::unsaved_autosave_path_for(30);
        let index = crate::app::unsaved_autosave_folder_for(30);
        let dir = index.parent().unwrap().to_path_buf();
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir_all(&dir);

        std::fs::create_dir_all(&dir).unwrap();
        touch(&file, "pre-fix monolithic");
        touch(&index, "incremental folder");

        clear_unsaved_stash(30);
        assert!(!file.exists(), "the monolithic stash is gone");
        assert!(!dir.exists(), "the stash FOLDER is gone whole");
    }
}
