//! Filesystem watcher for file objects (row 166, the second paid
//! deferral in `file_object.rs`'s module doc).
//!
//! `ReadDirectoryChangesW` over the DISTINCT parent directories of the
//! active document's linked files, debounced — editors save via
//! temp-write+rename, so one Ctrl+S is a BURST of notifications (temp
//! created, renamed, target modified) and the answer to a burst is one
//! refresh, not three. The thread wakes the UI through the same
//! `PostMessageW(WM_APP+…)` door `remote.rs` established; the wndproc
//! arm feeds the SAME quiet refresh every other door uses, so a watched
//! change and an alt-tab change are indistinguishable downstream
//! (UI-thread, undo-free, external truth — the core module's decisions
//! all stand).
//!
//! # Lifecycle
//!
//! [`sync`] is the only door, called from `pump_commands` after every
//! message batch with the active document's links: equal plans cost a
//! compare, changed plans rebuild the watch set, an EMPTY plan leaves no
//! thread running at all. The three honest-polling doors in
//! `file_object.rs` stay — this is an addition, not a replacement.
//!
//! # The honest limits, written down
//!
//! * Between `GetOverlappedResult` and the re-armed
//!   `ReadDirectoryChangesW` there is the classic API gap where a change
//!   can slip by unreported. Unavoidable without a second thread per
//!   directory, and the polling doors are the backstop — a missed wake
//!   costs the next alt-tab, nothing more.
//! * `WaitForMultipleObjects` waits on at most 64 handles (wake + 63
//!   directories). A work whose file objects live in more folders than
//!   that watches the first 63 (sorted, so deterministically the same
//!   ones); the rest keep their polling doors. A chapter with 60+
//!   background folders has never existed here.
//! * The change test downstream stays (mtime, length), not content: the
//!   explicit Update command remains the escape hatch for a same-stamp
//!   rewrite.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_CREATION,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::win32;

/// The wndproc message that says "a watched source settled". `WM_APP`
/// range beside `remote.rs`'s (+71); +72 is ours.
pub(crate) const MSG: u32 = WM_APP + 72;

/// How long a burst must stay quiet before it counts as one save. 400 ms
/// covers the temp/rename/modify tail of an editor save and the NTFS
/// journal's own follow-up events without feeling like a delay to a
/// human watching the page.
const SETTLE: Duration = Duration::from_millis(400);

/// One watched directory's read slot: the directory handle, the
/// auto-reset event its pending `ReadDirectoryChangesW` signals, and the
/// buffer + OVERLAPPED that read fills. Both boxed: the OS writes into
/// them by address, so they must not move when the `Vec<Slot>` grows.
struct Slot {
    dir: isize,
    ev: isize,
    ov: Box<OVERLAPPED>,
    buf: Box<[u8; 64 * 1024]>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        // Same thread issued the reads, so a plain cancel reaches them.
        // The kernel delivers the cancelled completion INTO `ov` (and may
        // still be filling `buf`), so both Boxes must outlive it: wait
        // for the completion before the handles close and the memory
        // frees, or a rebuild races a use-after-free.
        unsafe {
            CancelIoEx(self.dir as _, &*self.ov);
            let mut bytes = 0u32;
            GetOverlappedResult(self.dir as _, &*self.ov, &mut bytes, 1);
            CloseHandle(self.ev as _);
            CloseHandle(self.dir as _);
        }
    }
}

/// The watch set: which directories to wait on, and which file names
/// inside them belong to links. `PartialEq` is what makes `sync` cheap —
/// an unchanged plan must not rebuild handles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Plan {
    pub(super) dirs: Vec<PathBuf>,
    pub(super) names: BTreeSet<String>,
}

impl Plan {
    pub(super) fn of(links: &[PathBuf]) -> Plan {
        Plan {
            dirs: distinct_parents(links),
            names: basename_set(links),
        }
    }
}

/// Distinct parent directories of `links`, sorted so every rebuild of the
/// same link set builds the same plan (and the 63-dir cap below cuts the
/// same ones).
pub(super) fn distinct_parents(links: &[PathBuf]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = links
        .iter()
        .map(|p| p.parent().unwrap_or(p).to_path_buf())
        .collect();
    v.sort();
    v.dedup();
    v.truncate(63);
    v
}

/// The file names the watcher answers to, lowercased: NTFS is
/// case-insensitive, and an editor that rewrites `BG.PNG` saved the same
/// link.
pub(super) fn basename_set(links: &[PathBuf]) -> BTreeSet<String> {
    links
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_lowercase())
        .collect()
}

/// Coalesce one burst: an event re-arms the quiet window, `settled` fires
/// exactly once when the burst has been quiet for [`SETTLE`]. Pure — the
/// tests drive it with explicit `Instant`s, no clock, no sleeps.
#[derive(Default)]
pub(super) struct Debouncer {
    last: Option<Instant>,
}

impl Debouncer {
    pub(super) fn event(&mut self, now: Instant) {
        self.last = Some(now);
    }

    pub(super) fn settled(&mut self, now: Instant) -> bool {
        match self.last {
            Some(t) if now.saturating_duration_since(t) >= SETTLE => {
                self.last = None;
                true
            }
            _ => false,
        }
    }

    /// The wait timeout for the next `WaitForMultipleObjects`:
    /// [`INFINITE`] when idle, else the remainder of the quiet window
    /// (never less than 1 ms, so the settle check actually runs).
    pub(super) fn wait_ms(&self, now: Instant) -> u32 {
        match self.last {
            None => INFINITE,
            Some(t) => SETTLE
                .saturating_sub(now.saturating_duration_since(t))
                .as_millis()
                .clamp(1, u32::MAX as u128) as u32,
        }
    }
}

/// Walk one `FILE_NOTIFY_INFORMATION` buffer to its `(action, name)`
/// pairs. The layout: a u32 stride to the next entry (0 = last), a u32
/// action, a u32 BYTE length, then length bytes of UTF-16.
pub(super) fn notified_names(buf: &[u8]) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 12 <= buf.len() {
        let stride = u32::from_ne_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]) as usize;
        let action = u32::from_ne_bytes([buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7]]);
        let nlen = u32::from_ne_bytes([buf[off + 8], buf[off + 9], buf[off + 10], buf[off + 11]])
            as usize;
        if off + 12 + nlen > buf.len() {
            break; // a torn tail is possible mid-overwrite; drop it
        }
        let units: Vec<u16> = buf[off + 12..off + 12 + nlen]
            .chunks_exact(2)
            .map(|c| u16::from_ne_bytes([c[0], c[1]]))
            .collect();
        out.push((action, String::from_utf16_lossy(&units)));
        if stride == 0 {
            break;
        }
        off += stride;
    }
    out
}

/// Does any notification name a watched link? The OLD-name half of a
/// rename is the temp file an editor wrote first — not the link — so it
/// never counts.
pub(super) fn any_linked(notified: &[(u32, String)], names: &BTreeSet<String>) -> bool {
    notified.iter().any(|(a, n)| {
        *a != FILE_ACTION_RENAMED_OLD_NAME && names.contains(&n.to_lowercase())
    })
}

// --- thread side (statics: the App moves into GWLP_USERDATA and has no
// --- stable address for a field; `remote.rs` set this pattern) ----------

/// `None` = no plan (shut the thread down). The thread re-reads this on
/// every wake event.
static PLAN: Mutex<Option<Arc<Plan>>> = Mutex::new(None);
/// The watcher thread's handle while it runs; joined when the plan empties.
static THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
/// The auto-reset wake event, as an isize (`HANDLE` is not `Send`).
static WAKE: OnceLock<AtomicIsize> = OnceLock::new();
/// Where debounced wakes PostMessage. hwnd-as-isize, like `remote.rs`.
static HWND: AtomicIsize = AtomicIsize::new(0);
/// Diagnostics + the smoke test's observable: how many debounced wakes
/// have fired. A test has no window, so the PostMessage itself is the
/// unobservable half.
static WAKES: AtomicU32 = AtomicU32::new(0);
/// How many times the thread has built a slot set — the smoke test waits
/// for the arm, not for a hope and a timer.
static SERVED: AtomicU32 = AtomicU32::new(0);

/// Point the watcher at `links` — the active document's file-object
/// links, called from `pump_commands` after every message batch, so
/// every path that can change them (open, page hop, tab switch, import,
/// relink, refresh-repath) is covered by one compare. UI thread only.
/// Empty `links` stops the thread: no file objects, no thread.
pub(crate) fn sync(hwnd: isize, links: &[PathBuf]) {
    let plan = (!links.is_empty()).then(|| Arc::new(Plan::of(links)));
    {
        let mut g = PLAN.lock().unwrap();
        if g.as_deref() == plan.as_deref() {
            return; // the common case: nothing moved
        }
        *g = plan.clone();
    }
    HWND.store(hwnd, Ordering::SeqCst);
    let ev = WAKE
        .get_or_init(|| {
            let h = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
            AtomicIsize::new(h as isize)
        })
        .load(Ordering::SeqCst);
    if ev == 0 {
        // No event, no watcher; the polling doors keep the feature whole.
        *PLAN.lock().unwrap() = None;
        return;
    }
    if plan.is_some() {
        let mut t = THREAD.lock().unwrap();
        if t.is_none() {
            *t = Some(std::thread::spawn(run));
        }
    }
    unsafe { SetEvent(ev as _) };
    if plan.is_none() {
        // Outside every lock: the thread reads PLAN on its way out.
        let t = THREAD.lock().unwrap().take();
        if let Some(t) = t {
            let _ = t.join();
        }
    }
}

/// The watcher thread: serve the current plan until it changes, wake the
/// UI once per debounced burst, exit when the plan empties.
fn run() {
    let wake = WAKE
        .get()
        .map(|a| a.load(Ordering::SeqCst))
        .unwrap_or(0);
    if wake == 0 {
        return;
    }
    let mut serving: Option<Arc<Plan>> = None;
    let mut slots: Vec<Slot> = Vec::new();
    let mut debounce = Debouncer::default();
    loop {
        let current = PLAN.lock().unwrap().clone();
        let Some(plan) = current else {
            break; // plan emptied: shut down
        };
        if serving.as_deref() != Some(plan.as_ref()) {
            slots = open_slots(&plan); // Drop on the old set closes it
            serving = Some(plan.clone());
            SERVED.fetch_add(1, Ordering::SeqCst);
        }
        let mut hs: Vec<HANDLE> = Vec::with_capacity(slots.len() + 1);
        hs.push(wake as _);
        hs.extend(slots.iter().map(|s| s.ev as HANDLE));
        let r = unsafe {
            WaitForMultipleObjects(hs.len() as u32, hs.as_ptr(), 0, debounce.wait_ms(Instant::now()))
        };
        if r == WAIT_FAILED {
            // Defensive: re-open rather than spin on a broken wait.
            std::thread::sleep(Duration::from_millis(250));
            serving = None; // force a rebuild at the loop top
            continue;
        }
        if r == WAIT_TIMEOUT {
            if debounce.settled(Instant::now()) {
                WAKES.fetch_add(1, Ordering::SeqCst);
                unsafe {
                    PostMessageW(HWND.load(Ordering::SeqCst) as _, MSG, 0, 0);
                }
            }
            continue;
        }
        let idx = (r - WAIT_OBJECT_0) as usize;
        if idx == 0 {
            continue; // sync woke us: re-read the plan
        }
        if let Some(slot) = slots.get_mut(idx - 1)
            && let Some(notified) = slot.collect()
            && any_linked(&notified, &plan.names)
        {
            debounce.event(Instant::now());
        }
    }
}

/// Open one slot per plan directory. A directory that will not open
/// (network share dropped, permissions) is skipped, not fatal — its
/// polling doors still work.
fn open_slots(plan: &Plan) -> Vec<Slot> {
    let mut v = Vec::new();
    for d in &plan.dirs {
        let wide = win32::wide(&d.to_string_lossy());
        let dir = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if dir as isize == 0 || dir as isize == -1 {
            continue;
        }
        let ev = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if ev as isize == 0 {
            unsafe { CloseHandle(dir) };
            continue;
        }
        let mut slot = Slot {
            dir: dir as isize,
            ev: ev as isize,
            ov: Box::new(unsafe { std::mem::zeroed() }),
            buf: Box::new([0u8; 64 * 1024]),
        };
        slot.ov.hEvent = ev;
        if slot.arm() {
            v.push(slot);
        }
    }
    v
}

impl Slot {
    /// Post one async `ReadDirectoryChangesW` into the buffer. Subtree
    /// OFF: links sit IN their directory, and a subtree watch would
    /// forward every temp file a whole tree deep.
    fn arm(&mut self) -> bool {
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_LAST_WRITE
            | FILE_NOTIFY_CHANGE_SIZE
            | FILE_NOTIFY_CHANGE_CREATION;
        *self.ov = unsafe { std::mem::zeroed() };
        self.ov.hEvent = self.ev as _;
        let mut ret = 0u32;
        let ok = unsafe {
            ReadDirectoryChangesW(
                self.dir as _,
                self.buf.as_mut_ptr() as *mut _,
                self.buf.len() as u32,
                0,
                filter,
                &mut ret,
                &mut *self.ov,
                None,
            )
        };
        ok != 0
    }

    /// Take the completed read's bytes and re-arm; `None` when the read
    /// failed (torn state — the next rebuild re-opens this directory).
    fn collect(&mut self) -> Option<Vec<(u32, String)>> {
        let mut bytes = 0u32;
        let ok = unsafe { GetOverlappedResult(self.dir as _, &*self.ov, &mut bytes, 0) };
        if ok == 0 || bytes as usize > self.buf.len() {
            return None;
        }
        let out = notified_names(&self.buf[..bytes as usize]);
        self.arm();
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dir dedup + determinism: same links → same plan, overlapping
    /// folders collapse, and the cap keeps the wait legal.
    #[test]
    fn the_plan_dedups_dirs_and_lowercases_names() {
        let mk = |s: &str| PathBuf::from(s);
        let links = vec![
            mk("C:/art/bg.png"),
            mk("C:/art/ref.png"),
            mk("D:/refs/bg.PNG"),
        ];
        let p = Plan::of(&links);
        assert_eq!(p.dirs, vec![mk("C:/art"), mk("D:/refs")]);
        assert_eq!(p.names, ["bg.png", "ref.png"].into_iter().map(String::from).collect());
        // Truncation at 63 dirs, deterministically: sorted first.
        let many: Vec<PathBuf> = (0..100).map(|i| mk(&format!("D:/d{i}/x.png"))).collect();
        assert_eq!(Plan::of(&many).dirs.len(), 63);
    }

    /// One wake per burst: re-arms on each event, fires once after the
    /// quiet window, stays quiet until the next burst.
    #[test]
    fn the_debouncer_fires_once_per_burst_and_rearms() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        assert_eq!(d.wait_ms(t0), INFINITE, "idle waits forever");

        d.event(t0);
        d.event(t0 + Duration::from_millis(120)); // the burst's tail
        assert!(!d.settled(t0 + Duration::from_millis(300)), "still bursting");
        assert!(d.settled(t0 + Duration::from_millis(600)), "quiet long enough");
        assert!(!d.settled(t0 + Duration::from_millis(900)), "fires exactly once");

        d.event(t0 + Duration::from_millis(1000));
        assert_eq!(
            d.wait_ms(t0 + Duration::from_millis(1100)),
            300,
            "the remaining quiet window, as a wait timeout"
        );
        assert!(d.settled(t0 + Duration::from_millis(1500)), "the next burst wakes again");
    }

    /// Hand-built FILE_NOTIFY_INFORMATION buffer: two entries, one
    /// stride-padded, parsed name-exact — the wire format the thread
    /// walks is pinned here, not assumed.
    #[test]
    fn notify_buffers_parse_to_names() {
        let mut b: Vec<u8> = Vec::new();
        let entry = |action: u32, name: &str, b: &mut Vec<u8>| {
            let units: Vec<u16> = name.encode_utf16().collect();
            let head = 12 + units.len() * 2;
            let pad = (4 - head % 4) % 4; // entries stride on u32
            b.extend_from_slice(&((head + pad) as u32).to_ne_bytes());
            b.extend_from_slice(&action.to_ne_bytes());
            b.extend_from_slice(&((units.len() * 2) as u32).to_ne_bytes());
            for u in units {
                b.extend_from_slice(&u.to_ne_bytes());
            }
            b.extend(std::iter::repeat(0u8).take(pad));
        };
        entry(3, "bg.png", &mut b); // FILE_ACTION_MODIFIED
        entry(5, "bg.png", &mut b); // FILE_ACTION_RENAMED_NEW_NAME
        let names = notified_names(&b);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].1, "bg.png");
        assert_eq!(names[1], (5, "bg.png".to_owned()));
        // A torn tail is dropped, not read past.
        let mut torn = b.clone();
        torn.truncate(torn.len() - 3);
        assert_eq!(notified_names(&torn).len(), 1);
    }

    /// Only the link's own names count, case-insensitively, and a
    /// rename's OLD half (the editor's temp file) does not.
    #[test]
    fn only_linked_names_match() {
        let names = basename_set(&[PathBuf::from("C:/art/bg.png")]);
        assert!(any_linked(&[(3, "bg.png".into())], &names));
        assert!(any_linked(&[(5, "BG.PNG".into())], &names), "case-insensitive");
        assert!(any_linked(&[(2, "bg.png".into())], &names), "a removal is a change");
        assert!(!any_linked(
            &[(4, "bg.png".into()), (5, "tmp4a7f".into())], &names
        ));
        assert!(!any_linked(&[(3, "tmp4a7f".into())], &names), "temps never match");
    }

    /// The honest smoke: a real directory, a real burst of writes, ONE
    /// debounced wake. The PostMessage half is unobservable without a
    /// window, so the wake COUNTER is the seam. Bounded loops with real
    /// deadlines, never fixed sleeps-as-assertions.
    #[test]
    fn a_burst_of_saves_lands_one_wake() {
        let dir = std::env::temp_dir().join(format!("mn-fo-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let src = dir.join("bg.png");
        let mut n = 0u32;
        let write = |n: u32| {
            image::RgbaImage::from_pixel(8, 8, image::Rgba([n as u8, 3, 7, 255]))
                .save(&src)
                .expect("write");
        };
        write(0);

        sync(0, &[src.clone()]); // hwnd 0: PostMessageW is a no-op
        let served0 = SERVED.load(Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(10);
        while SERVED.load(Ordering::SeqCst) == served0 {
            assert!(Instant::now() < deadline, "watcher never armed");
            std::thread::sleep(Duration::from_millis(20));
        }

        // One Ctrl+S in an editor: several writes inside the settle window.
        let wakes0 = WAKES.load(Ordering::SeqCst);
        for i in 1..=3 {
            n = n + i;
            write(n);
            std::thread::sleep(Duration::from_millis(60));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while WAKES.load(Ordering::SeqCst) == wakes0 {
            assert!(Instant::now() < deadline, "the burst never woke the UI");
            std::thread::sleep(Duration::from_millis(20));
        }
        // And nothing more: one burst, one wake.
        std::thread::sleep(SETTLE * 3);
        assert_eq!(
            WAKES.load(Ordering::SeqCst),
            wakes0 + 1,
            "a burst coalesces to exactly one wake"
        );

        sync(0, &[]); // shut down — the plan-empty join happens here
        let _ = std::fs::remove_dir_all(&dir);
    }
}
