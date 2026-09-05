//! The background save writer, the "Saving…" pill, and the per-frame poll
//! that puts a finished write into the status line (item K, 2026-09-05).
//!
//! **The fact this file exists for.** Every save in `cmd/file_io.rs` used to
//! run start-to-finish on the UI thread, so the Windows message pump stopped
//! for the whole write. Past about five seconds Windows paints "not
//! responding" over the window — exactly what the owner reported — and a
//! "saving…" widget drawn in the same frame is never shown, because no frame
//! is ever produced. Splitting the save in two fixes both: the ENCODE stays
//! on the UI thread (it reads the live layers, so it must), the WRITE goes
//! here.
//!
//! What crosses the thread boundary is always BYTES — or a `WorkFolder`,
//! which is per-page bytes plus a little metadata. Never the document, never
//! a layer, never anything the UI thread can still be editing.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{LazyLock, Mutex, OnceLock};

use crate::app::App;

/// One unit of work for the writer thread.
pub(crate) enum Write {
    /// A finished file: `.ora`, `.mnc`, `.psd`, `.png`, a script dump.
    File { path: PathBuf, bytes: Vec<u8> },
    /// A work FOLDER. `mn_core::project::save_folder` is already pure IO over
    /// per-page bytes (its caller does the encoding), so the whole call moves
    /// across unchanged.
    Folder {
        wf: Box<mn_core::project::WorkFolder>,
        dir: PathBuf,
        managed: Vec<String>,
    },
}

/// A finished write, waiting for the UI thread to say so.
pub(crate) struct Done {
    /// The label the write was submitted under. Only the tests read it — they
    /// share one process and one board, so "is this MY result" has to be
    /// answerable — but the string already exists, so it is free.
    #[cfg_attr(not(test), allow(dead_code, reason = "read by the tests only"))]
    pub label: String,
    pub ok: bool,
    pub msg: String,
    /// True for the writes whose FAILURE has to put the work back to dirty
    /// (a save). An export or an autosave that fails is news, not a change
    /// of state.
    pub was_a_save: bool,
}

#[derive(Default)]
struct Board {
    /// Labels of the writes queued or running, oldest first.
    pending: Vec<String>,
    done: Vec<Done>,
}

/// One queue, process-wide, on purpose: there is one disk, and two
/// overlapping writes into the same work folder would race each other's
/// `rename`. A single queue makes "a second save while one is in flight
/// queues behind it" the default rather than a rule to remember.
static BOARD: LazyLock<Mutex<Board>> = LazyLock::new(Mutex::default);

fn board() -> std::sync::MutexGuard<'static, Board> {
    // A panicking writer must not take the save indicator down with it.
    BOARD.lock().unwrap_or_else(|e| e.into_inner())
}

/// tmp + flush + fsync + rename — the same contract
/// `mn_core::ora::write_atomic` keeps on the UI-thread path (its own helper
/// is crate-private over there): the previous file survives a crash or a
/// full disk, a failed flush is an error rather than a silently truncated
/// "successful" save, and no `.tmp` debris is left behind.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "out".into());
    let tmp = path.with_file_name(format!("{name}.mn-tmp"));
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return Err(e.to_string());
        }
    }
    let built = (|| -> std::io::Result<()> {
        let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        w.write_all(bytes)?;
        w.flush()?;
        w.into_inner()
            .map_err(|e| std::io::Error::other(e.to_string()))?
            .sync_all()?;
        Ok(())
    })()
    .and_then(|()| std::fs::rename(&tmp, path));
    match built {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e.to_string())
        }
    }
}

fn perform(job: Write) -> Result<String, String> {
    match job {
        Write::File { path, bytes } => {
            let kb = bytes.len() / 1024;
            write_atomic(&path, &bytes)?;
            Ok(format!("saved {} ({kb} KB)", path.display()))
        }
        Write::Folder { wf, dir, managed } => {
            let pages = wf.pages.len();
            match mn_core::project::save_folder(&wf, &dir, &managed) {
                Ok((_, written)) => Ok(format!(
                    "saved work folder {} ({pages} pages, {written} rewritten)",
                    dir.display()
                )),
                Err(e) => Err(e.to_string()),
            }
        }
    }
}

type Job = (String, bool, Write);

fn writer() -> &'static Sender<Job> {
    static TX: OnceLock<Sender<Job>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("mn-save".into())
            .spawn(move || {
                // One job at a time, in submission order: that IS the
                // "writes never overlap" rule.
                while let Ok((label, was_a_save, job)) = rx.recv() {
                    let out = perform(job);
                    let mut b = board();
                    if let Some(i) = b.pending.iter().position(|l| *l == label) {
                        b.pending.remove(i);
                    }
                    b.done.push(match out {
                        Ok(msg) => Done {
                            label,
                            ok: true,
                            msg,
                            was_a_save,
                        },
                        Err(e) => Done {
                            label,
                            ok: false,
                            msg: format!("save failed: {e}"),
                            was_a_save,
                        },
                    });
                }
            })
            .expect("spawn the background save writer");
        tx
    })
}

/// Hand a write to the background thread. `label` is what the pill says.
///
/// A send that fails means the writer thread died (only possible if it
/// panicked), and losing the user's save silently is the one outcome that is
/// not allowed — so that case writes on this thread instead, blocking, the
/// way the whole app used to.
pub(crate) fn submit(label: impl Into<String>, was_a_save: bool, job: Write) {
    let label = label.into();
    board().pending.push(label.clone());
    let sent = writer().send((label.clone(), was_a_save, job));
    if sent.is_ok() {
        // THE ONE SUBTLETY IN THIS FILE. A write may only be left running in
        // the background if somebody is going to come back for it, and the
        // only thing that ever does is [`poll_saves`], once per UI frame. The
        // app has plenty of doors with no frame loop behind them — every unit
        // test, `--e2e-workfolder`, the `--shot-*` screenshot drivers — and
        // all of them do the same thing: dispatch a save, then immediately
        // read the file back. For those, "asynchronous" would mean "the file
        // is not there yet", which is not a mode, it is a bug.
        //
        // So the rule is: no frames, no background. The write still goes
        // through the same queue and the same thread; this just waits for it,
        // which is exactly what the whole app used to do.
        if !frames_are_running() {
            flush();
        }
        return;
    }
    if let Err(e) = sent {
        let (_, _, job) = e.0;
        let out = perform(job);
        let mut b = board();
        if let Some(i) = b.pending.iter().position(|l| *l == label) {
            b.pending.remove(i);
        }
        b.done.push(match out {
            Ok(msg) => Done {
                label,
                ok: true,
                msg,
                was_a_save,
            },
            Err(e) => Done {
                label,
                ok: false,
                msg: format!("save failed: {e}"),
                was_a_save,
            },
        });
    }
}

/// The page ids `mn_core::project::save_folder` WILL assign, computed here
/// instead of waiting for it.
///
/// This exists because the folder save is now asynchronous: the caller has to
/// finish its bookkeeping (`PageEntry::id`, `saved_rev`, `folder_managed`) on
/// the UI thread, before the write has run. The rule is `save_folder`'s own,
/// verbatim — ids are assigned once, in reading order, and never reused — and
/// `the_id_guess_matches_what_save_folder_assigns` below is the pin that
/// keeps the two copies honest.
pub(crate) fn folder_page_ids(wf: &mn_core::project::WorkFolder) -> Vec<u32> {
    let mut next_id = wf.next_id.max(1);
    wf.pages
        .iter()
        .map(|p| {
            if p.id == 0 {
                let id = next_id;
                next_id += 1;
                id
            } else {
                next_id = next_id.max(p.id + 1);
                p.id
            }
        })
        .collect()
}

/// Milliseconds (since [`START`], +1 so `0` can mean "never") at the last UI
/// frame. Written by [`saving_pill`], which `ui::build` calls unconditionally
/// every frame; read by [`frames_are_running`].
static LAST_FRAME_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static START: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);

fn now_ms() -> u64 {
    START.elapsed().as_millis() as u64 + 1
}

/// Is there a UI frame loop behind this call — i.e. will anybody poll the
/// result of a background write? See the long comment in [`submit`].
fn frames_are_running() -> bool {
    // Unit tests share one process and run in parallel, so a timing window
    // would make one test's frame make another test's save asynchronous.
    // Tests are always synchronous, full stop.
    if cfg!(test) {
        return false;
    }
    let last = LAST_FRAME_MS.load(std::sync::atomic::Ordering::Relaxed);
    last != 0 && now_ms().saturating_sub(last) < 500
}

/// The label of the write in flight — `None` when the disk is idle. This is
/// what the "Saving…" pill draws.
pub(crate) fn in_flight() -> Option<String> {
    board().pending.first().cloned()
}

/// How many writes are queued, the running one included.
pub(crate) fn queued() -> usize {
    board().pending.len()
}

/// Block until every queued write has landed. The close path wants this (a
/// queued save must not die with the process) and so do the tests.
pub(crate) fn flush() {
    // Polling rather than a condvar: this runs at most a handful of times in
    // a session, and a condvar here would be a second synchronisation
    // primitive to get right for no gain.
    for _ in 0..30_000 {
        if queued() == 0 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Drain the background writer into the status line. Called once per UI
/// frame from [`crate::ui::build`], which is the only place holding a
/// `&mut App` every frame.
///
/// A FAILED save puts the work back to dirty: the bookkeeping (`mark_saved`,
/// `set_doc_path`, the per-page `saved_rev`) ran optimistically when the
/// bytes were handed over, and the one thing that must not survive a failed
/// write is "this work is safe on disk".
pub(crate) fn poll_saves(app: &mut App) {
    let done = std::mem::take(&mut board().done);
    for d in done {
        if d.ok {
            app.set_status(d.msg);
        } else {
            if d.was_a_save {
                app.mark_pages_dirty();
            }
            app.set_error(d.msg);
        }
    }
}

/// The "Saving…" pill: top-right of the canvas, never a modal (a modal
/// steals the pen mid-stroke). Drawn on a foreground layer so no palette can
/// cover it, and it asks for the next frame itself so the spinner turns and
/// [`poll_saves`] keeps being reached while the write runs.
pub(crate) fn saving_pill(ui: &egui::Ui, canvas: egui::Rect) {
    // The heartbeat `frames_are_running` reads. It has to be the FIRST thing
    // here, before the early return: "a frame happened" is true whether or
    // not there was anything to draw.
    LAST_FRAME_MS.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
    let Some(label) = in_flight() else { return };
    let ctx = ui.ctx();
    ctx.request_repaint();
    let extra = queued().saturating_sub(1);
    let name = Path::new(&label)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or(label);
    let text = if extra > 0 {
        format!("Saving… {name}  (+{extra})")
    } else {
        format!("Saving… {name}")
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("mn.saving.pill"),
    ));
    let c = crate::ui::theme::c();
    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), c.text);
    let pad = egui::vec2(10.0, 6.0);
    const SPIN_W: f32 = 18.0;
    let size = galley.size() + pad * 2.0 + egui::vec2(SPIN_W, 0.0);
    let right = canvas.right() - 12.0;
    let rect = egui::Rect::from_min_size(
        egui::pos2(right - size.x, canvas.top() + 12.0),
        size,
    );
    painter.rect_filled(rect, 6.0, c.panel);
    painter.rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, c.accent),
        egui::StrokeKind::Inside,
    );
    // A turning arc rather than a dot crawl: one shape, and it reads as
    // "still working" at a glance from the corner of an eye.
    let hub = egui::pos2(rect.left() + pad.x + SPIN_W * 0.5, rect.center().y);
    let t = ctx.input(|i| i.time) as f32 * 3.0;
    let arc: Vec<egui::Pos2> = (0..=12)
        .map(|k| {
            let a = t + k as f32 / 12.0 * 4.2;
            egui::pos2(hub.x + 5.5 * a.cos(), hub.y + 5.5 * a.sin())
        })
        .collect();
    painter.add(egui::Shape::line(arc, egui::Stroke::new(1.8, c.accent)));
    painter.galley(
        egui::pos2(rect.left() + pad.x + SPIN_W, rect.top() + pad.y),
        galley,
        c.text,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The board is one process-wide static and cargo runs tests in
    /// parallel, so every test here holds this lock: without it one test's
    /// `pending.clear()` lands in the middle of another's assertion.
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Take just the results of MY writes off the shared board. Cargo runs
    /// these tests in parallel in one process, and the board is one board:
    /// draining it wholesale is how one test ate another's result (and did,
    /// on the first run of this file).
    fn mine(label: &str) -> Vec<Done> {
        let mut b = board();
        let (mine, rest): (Vec<Done>, Vec<Done>) =
            std::mem::take(&mut b.done).into_iter().partition(|d| d.label == label);
        b.done = rest;
        mine
    }

    /// The pill is verified the only way a pill can be verified without a
    /// window: set the state, run one headless egui frame, and read the text
    /// the frame painted.
    #[test]
    fn a_write_in_flight_draws_the_saving_pill() {
        let _serial = serial();
        board().pending.clear();
        board().done.clear();
        assert!(in_flight().is_none(), "the disk starts idle");
        board().pending.push("C:/work/chapter7/work.mnc".into());

        let ctx = egui::Context::default();
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            saving_pill(ui, egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1200.0, 800.0),
            ));
        });
        fn walk(s: &egui::epaint::Shape, into: &mut String) {
            match s {
                egui::epaint::Shape::Text(t) => {
                    into.push_str(t.galley.text());
                    into.push('\n');
                }
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, into)),
                _ => {}
            }
        }
        let mut text = String::new();
        for c in &out.shapes {
            walk(&c.shape, &mut text);
        }
        out.drop_without_applying_deltas();
        println!("[pill] {text}");
        assert!(
            text.contains("Saving…") && text.contains("work.mnc"),
            "the pill must name what is being written: {text}"
        );
        board().pending.clear();
    }

    /// The round trip: bytes go out, the thread writes them, the poll turns
    /// the result into the status line.
    #[test]
    fn the_background_writer_lands_the_bytes_and_reports_back() {
        let _serial = serial();
        let dir = std::env::temp_dir().join(format!("mn-savebg-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("hello.bin");
        submit(
            path.display().to_string(),
            true,
            Write::File {
                path: path.clone(),
                bytes: b"page bytes".to_vec(),
            },
        );
        flush();
        assert_eq!(
            std::fs::read(&path).expect("the writer landed the file"),
            b"page bytes"
        );
        let done = mine(&path.display().to_string());
        assert!(done.iter().any(|d| d.ok), "the write reported success");
        // No `.mn-tmp` debris beside it.
        assert!(!dir.join("hello.bin.mn-tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The bookkeeping now runs before the write does, on ids this file
    /// GUESSES. This is the pin that the guess is not a guess.
    #[test]
    fn the_id_guess_matches_what_save_folder_assigns() {
        let _serial = serial();
        let page = |id: u32| mn_core::project::FolderPage {
            id,
            rev: 1,
            saved_rev: 0,
            exported_rev: 0,
            uid: id as u64 + 900,
            bytes: b"not really an ora".to_vec(),
        };
        for (next_id, ids) in [
            (1u32, vec![0, 0, 0]),
            (7, vec![3, 0, 9, 0]),
            (1, vec![5, 5, 0]),
            (0, vec![0]),
        ] {
            let wf = mn_core::project::WorkFolder {
                story: String::new(),
                binding_right: true,
                setup: None,
                expression: mn_core::Expression::Mono,
                spine_mm: 0.0,
                cover: None,
                template_page: None,
                print_margin_info: false,
                print_crop_marks: false,
                profile: None,
                next_id,
                pages: ids.iter().map(|&i| page(i)).collect(),
            };
            let guess = folder_page_ids(&wf);
            let dir = std::env::temp_dir().join(format!(
                "mn-idpin-{}-{next_id}-{}",
                std::process::id(),
                ids.len()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            let (real, _) =
                mn_core::project::save_folder(&wf, &dir, &[]).expect("the folder write");
            assert_eq!(guess, real, "next_id {next_id}, ids {ids:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A write into a path that cannot exist fails LOUDLY, and the failure
    /// is flagged as a save so the poll can put the work back to dirty.
    #[test]
    fn a_failed_write_comes_back_as_an_error() {
        let _serial = serial();
        let bad = std::env::temp_dir()
            .join("mn-savebg-nope")
            .join("a\0b")
            .join("x.ora");
        submit(
            "mn-test-bad-path.ora",
            true,
            Write::File {
                path: bad,
                bytes: vec![1, 2, 3],
            },
        );
        flush();
        let done = mine("mn-test-bad-path.ora");
        assert!(
            done.iter().any(|d| !d.ok && d.was_a_save),
            "a failed save must come back as a failed SAVE"
        );
    }
}
