//! The tester log: `manganakama.log`, one appended block per session.
//! Testers attach it to GitHub issues, so it carries diagnostics and
//! nothing else — no file paths, no document names, no user identity.
//! Keep it that way: the point is that a stranger can send this file
//! without thinking twice about what is in it.
//!
//! What a block holds: the build stamp (which release the report is
//! against), the adapter identity, whether strokes ran on the GPU (P1)
//! or CPU, canary repairs (the cursed-driver defense firing) — and the
//! two lines that make a crash legible: `!!! PANIC` from the hook, and
//! the `exited cleanly` marker whose ABSENCE is the crash signal.
//!
//! Keep it tiny — this is a diagnostic channel, not a framework.

use std::io::Write;
use std::path::{Path, PathBuf};

const NAME: &str = "manganakama.log";

static LOG: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
static PATH: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

fn open_append(path: &Path) -> Option<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// Where the log goes: beside the exe first (the portable unzip, where a
/// tester looks without being told), else `%LOCALAPPDATA%\MangaNakama\`.
/// The fallback exists because an installed copy under Program Files
/// cannot write beside itself — that case used to produce NO log and no
/// warning, so the tester who most needed to send a file had none.
fn open_log() -> Option<(PathBuf, std::fs::File)> {
    if let Some(p) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(NAME)))
        && let Some(f) = open_append(&p)
    {
        return Some((p, f));
    }
    let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("MangaNakama");
    std::fs::create_dir_all(&dir).ok()?;
    let p = dir.join(NAME);
    let f = open_append(&p)?;
    Some((p, f))
}

/// Run `f` against the open log, opening it on first use. Lazy on
/// purpose: the panic hook is installed before anything else in `main`,
/// so a panic during startup must still be able to write.
fn with_log(f: impl FnOnce(&mut std::fs::File)) {
    let mut g = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none()
        && let Some((p, file)) = open_log()
    {
        *PATH.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
        *g = Some(file);
    }
    if let Some(file) = g.as_mut() {
        f(file);
        let _ = file.flush();
    }
}

/// The resolved log path, once something has been written. The
/// diagnostics HUD shows it, so "attach the log" is answerable without
/// the tester having to guess where the app decided to put it.
pub fn path() -> Option<PathBuf> {
    PATH.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Open (append) the log and stamp a session banner. Call once at startup.
pub fn begin_session(lines: &[String]) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let lines: Vec<String> = lines.to_vec();
    with_log(|f| {
        let _ = writeln!(f, "\n=== session {now} ===");
        for l in &lines {
            let _ = writeln!(f, "{l}");
        }
    });
}

/// The clean-exit marker. A session block that ends WITHOUT this line did
/// not exit — which is the only way to see the crash class the panic hook
/// cannot catch (a stack overflow is killed by the OS outright, so no
/// Rust hook runs at all).
pub fn end_session() {
    line("=== exited cleanly ===");
}

/// Append one line (mirrors what the console already prints for the events
/// testers care about). Never panics — a bad log must not take the app down.
pub fn line(msg: &str) {
    with_log(|f| {
        let _ = writeln!(f, "{msg}");
    });
}

/// Record every panic in the log before the process goes down, keeping
/// whatever hook was installed before ours. A panic that crosses the
/// `extern "system"` wndproc ABORTS — no unwind, no save prompt, the
/// window simply vanishes — so this hook is the only chance to record
/// why. Install it as the first statement in `main`.
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_owned());
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".to_owned());
        line(&format!("!!! PANIC at {at}: {msg}"));
        prev(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real round trip: a line written through the lazy-open path
    /// lands in the file `path()` reports, and `end_session` writes the
    /// clean-exit marker whose absence is our crash signal.
    ///
    /// It also guards the promise the whole file rests on — that a tester
    /// can post this publicly — by scanning the ACTUAL log content for a
    /// Windows profile path. If any future line ever logs a filename, this
    /// fails.
    #[test]
    fn round_trips_and_carries_no_user_paths() {
        let marker = "[test] audit-marker-7f3a";
        line(marker);
        end_session();
        let Some(p) = path() else {
            println!("[test] SKIP: no writable location (sandboxed runner)");
            return;
        };
        assert!(p.ends_with(NAME), "named {NAME}: {}", p.display());
        let body = std::fs::read_to_string(&p).expect("the log reads back");
        assert!(body.contains(marker), "the written line is in the file");
        assert!(
            body.contains("=== exited cleanly ==="),
            "the clean-exit marker is what a crash is missing"
        );
        // The doxx guard, over real content rather than a synthetic string.
        for (n, l) in body.lines().enumerate() {
            let low = l.to_ascii_lowercase();
            assert!(
                !low.contains(":\\users\\") && !low.contains("/users/"),
                "line {n} leaks a user profile path: {l}"
            );
        }
    }

    /// The fallback resolves to a writable location even when the exe's
    /// own folder is not (the Program Files case).
    #[test]
    fn log_path_resolves_somewhere_writable() {
        let Some((p, _f)) = open_log() else {
            println!("[test] SKIP: no writable location (sandboxed runner)");
            return;
        };
        assert!(p.ends_with(NAME));
        assert!(p.parent().is_some_and(|d| d.exists()), "parent dir exists");
    }
}
