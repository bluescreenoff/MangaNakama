//! Process/thread priority for the window thread (item H, 2026-09-05).
//!
//! The owner: "even the G-pen is laggy while two Claude sessions run. Is our
//! priority low?" We run at NORMAL like every other process, so a background
//! compiler with N worker threads gets the same slice of CPU as the thread
//! that pumps `WM_POINTER` and paints the stroke. Raising the window thread
//! one step fixes the CPU half of that.
//!
//! It is only half. The machine is RAM-starved (15.8 GB with two agent
//! sessions + cargo resident), and a priority class does nothing about
//! paging — a page fault waits on the disk no matter how important the
//! thread is. Worth doing anyway; not a cure for the swapping.
//!
//! **ABOVE_NORMAL, never HIGH or REALTIME.** Those two starve the digitizer
//! driver's own threads and the DWM compositor, which makes pen input worse,
//! not better — the exact opposite of the ask.
//!
//! Brush/dab worker threads are deliberately left at NORMAL: putting the UI
//! thread *above* them is the whole point.

/// Raise this process to ABOVE_NORMAL and the calling thread (the one that
/// owns the window, pumps input and paints) one notch above that.
///
/// Called once from `Shell::new`. No-op off Windows, and no-op under
/// `cfg(test)`: a `cargo test` run must not out-prioritise the app the owner
/// is drawing in.
#[cfg(all(windows, not(test)))]
pub fn raise_for_interactive() {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::{
        ABOVE_NORMAL_PRIORITY_CLASS, GetCurrentProcess, GetCurrentThread, SetPriorityClass,
        SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
    };

    // SAFETY: pseudo-handles from GetCurrentProcess/GetCurrentThread are
    // always valid and need no close; both setters only read the constants.
    let (proc_ok, thread_ok, err) = unsafe {
        let p = SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS) != 0;
        let t = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) != 0;
        (p, t, GetLastError())
    };
    if proc_ok && thread_ok {
        println!("[app] priority: above-normal");
    } else {
        println!(
            "[app] priority: normal — SetPriorityClass ok={proc_ok}, SetThreadPriority \
             ok={thread_ok}, GetLastError {err}"
        );
    }
}

#[cfg(not(all(windows, not(test))))]
pub fn raise_for_interactive() {}
