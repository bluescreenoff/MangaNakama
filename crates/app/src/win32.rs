//! Win32 bits `windows-sys` does not ship, plus small helpers.
//!
//! Everything here is a literal from the Windows SDK headers. Keep the source
//! header noted next to each block — these are the values that make inking feel
//! immediate instead of laggy, and a wrong constant fails silently (the pen just
//! feels bad, nothing errors).

/// `WM_TABLET_QUERYSYSTEMGESTURESTATUS` (tpcshrd.h). Not in `windows-sys`.
pub const WM_TABLET_QUERYSYSTEMGESTURESTATUS: u32 = 0x02CC;

// tpcshrd.h gesture-status flags. Returning these from the message above is what
// removes the press-and-hold delay before ink appears.
pub const TABLET_DISABLE_PRESSANDHOLD: u32 = 0x0000_0001;
pub const TABLET_DISABLE_PENTAPFEEDBACK: u32 = 0x0000_0008;
pub const TABLET_DISABLE_PENBARRELFEEDBACK: u32 = 0x0000_0010;
pub const TABLET_DISABLE_TOUCHUIFORCEOFF: u32 = 0x0000_0200;
pub const TABLET_DISABLE_TOUCHSWITCH: u32 = 0x0000_8000;
pub const TABLET_DISABLE_FLICKS: u32 = 0x0001_0000;
pub const TABLET_DISABLE_SMOOTHSCROLLING: u32 = 0x0008_0000;
pub const TABLET_DISABLE_FLICKFALLBACKKEYS: u32 = 0x0010_0000;

/// The whole "leave my ink alone" set.
pub const TABLET_INK_FLAGS: u32 = TABLET_DISABLE_PRESSANDHOLD
    | TABLET_DISABLE_PENTAPFEEDBACK
    | TABLET_DISABLE_PENBARRELFEEDBACK
    | TABLET_DISABLE_TOUCHUIFORCEOFF
    | TABLET_DISABLE_TOUCHSWITCH
    | TABLET_DISABLE_FLICKS
    | TABLET_DISABLE_SMOOTHSCROLLING
    | TABLET_DISABLE_FLICKFALLBACKKEYS;

/// `MI_WP_SIGNATURE` / `SIGNATURE_MASK` (winuser.h). Windows synthesises classic
/// `WM_MOUSE*` messages from pen/touch input; those carry this signature in
/// `GetMessageExtraInfo()`. We handle the pen through `WM_POINTER*`, so promoted
/// mouse events must be dropped or every pen stroke gets drawn twice.
pub const MI_WP_SIGNATURE: usize = 0xFF51_5700;
pub const SIGNATURE_MASK: usize = 0xFFFF_FF00;

#[inline]
pub fn is_pen_promoted_mouse(extra: isize) -> bool {
    (extra as usize & SIGNATURE_MASK) == MI_WP_SIGNATURE
}

// winuser.h mouse-message key flags. `windows-sys` files these under
// `Win32::System::SystemServices`; two u32s are not worth enabling that whole
// feature module.
#[allow(dead_code)] // kept beside MK_SHIFT for completeness
pub const MK_CONTROL: u32 = 0x0008;
pub const MK_SHIFT: u32 = 0x0004;

/// NUL-terminated UTF-16 for Win32 string arguments.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// --- clipboard (CF_UNICODETEXT only) ------------------------------------

/// winuser.h. Only this one format is used; not worth a feature module.
const CF_UNICODETEXT: u32 = 13;

/// Read text from the Windows clipboard. `OpenClipboard(null)` binds it to
/// the current task, which is all a same-thread read/write needs.
pub fn clipboard_get_text() -> Option<String> {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0
            || OpenClipboard(std::ptr::null_mut()) == 0
        {
            return None;
        }
        let mut out = None;
        let h = GetClipboardData(CF_UNICODETEXT);
        if !h.is_null() {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                let mut len = 0usize;
                while *p.add(len) != 0 {
                    len += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        out
    }
}

/// Put text on the Windows clipboard. The `GlobalAlloc` buffer is owned by
/// the clipboard once `SetClipboardData` succeeds.
pub fn clipboard_set_text(s: &str) {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    let utf16 = wide(s);
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return;
        }
        EmptyClipboard();
        let bytes = utf16.len() * 2;
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u8;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(utf16.as_ptr() as *const u8, p, bytes);
                GlobalUnlock(h);
                if SetClipboardData(CF_UNICODETEXT, h).is_null() {
                    GlobalFree(h);
                }
            } else {
                GlobalFree(h);
            }
        }
        CloseClipboard();
    }
}

/// `GetTickCount` as milliseconds — the **same clock** `POINTER_INFO::dwTime`
/// stamps every pen sample with (`input.rs`), which is the only reason the
/// HUD's end-to-end input latency is a subtraction instead of a plumbing
/// project. It wraps every 49.7 days and a device that stamps its own clock
/// will not agree with it at all, so the one consumer treats any implausible
/// difference as "unknown" rather than printing a number it cannot defend.
#[inline]
pub fn tick_ms() -> f64 {
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount() as f64 }
}

#[inline]
pub fn loword(v: usize) -> u16 {
    (v & 0xFFFF) as u16
}

/// Signed 16-bit halves of an LPARAM, as used by the classic mouse messages.
#[inline]
pub fn lparam_points(lp: isize) -> (i32, i32) {
    let x = (lp & 0xFFFF) as u16 as i16 as i32;
    let y = ((lp >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pen_promoted_mouse_is_detected() {
        assert!(is_pen_promoted_mouse(0xFF51_5700u32 as isize));
        assert!(is_pen_promoted_mouse(0xFF51_5701u32 as isize)); // low byte = pen id
        assert!(!is_pen_promoted_mouse(0));
        assert!(!is_pen_promoted_mouse(0x1234_5678));
    }

    #[test]
    fn lparam_halves_are_signed() {
        // (-3, -7) packed the way Windows packs client coords.
        let lp = (((-7i32 as u16 as u32) << 16) | (-3i32 as u16 as u32)) as isize;
        assert_eq!(lparam_points(lp), (-3, -7));
        assert_eq!(lparam_points(0x0064_00C8), (200, 100));
    }
}

/// Reader fullscreen (owner top item 2026-08-18): borderless over the
/// window's monitor, saving the style + rect for restore. F11 / the
/// reader's lifecycle drive it; the values below are WinUser.h literals.
pub unsafe fn set_window_fullscreen(
    hwnd: isize,
    on: bool,
    saved: &mut Option<crate::app::reader::FsSaved>,
) {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, GetWindowRect, SWP_FRAMECHANGED, SWP_NOACTIVATE,
        SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
        WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
    };

    // Edition 2024: an `unsafe fn` body is not itself an unsafe block.
    unsafe {
        let hwnd = hwnd as HWND;
        if on {
            if saved.is_none() {
                let mut rc: RECT = std::mem::zeroed();
                GetWindowRect(hwnd, &mut rc);
                *saved = Some(crate::app::reader::FsSaved {
                    style: GetWindowLongPtrW(hwnd, GWL_STYLE),
                    rect: [rc.left, rc.top, rc.right, rc.bottom],
                });
            }
            let drop = (WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX)
                as isize;
            let style = (GetWindowLongPtrW(hwnd, GWL_STYLE) & !drop) | WS_POPUP as isize;
            SetWindowLongPtrW(hwnd, GWL_STYLE, style);
            let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(mon, &mut mi) != 0 {
                let r = mi.rcMonitor;
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        } else if let Some(s) = saved.take() {
            SetWindowLongPtrW(hwnd, GWL_STYLE, s.style);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                s.rect[0],
                s.rect[1],
                s.rect[2] - s.rect[0],
                s.rect[3] - s.rect[1],
                SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

/// Let the shell drop files on this window (IO-041). `WM_DROPFILES` is the
/// classic path: no OLE, no `IDropTarget`, no COM apartment to keep alive for
/// the process lifetime. What it costs us is drag-over feedback (no highlight
/// while the cursor is above the window) and the drop point, which we do not
/// use — see `drop.rs` for why the gesture deliberately means the same thing
/// everywhere on the window.
pub unsafe fn accept_dropped_files(hwnd: isize) {
    use windows_sys::Win32::UI::Shell::DragAcceptFiles;
    unsafe { DragAcceptFiles(hwnd as _, 1) };
}

/// The paths carried by a `WM_DROPFILES`. **Always** calls `DragFinish`: the
/// shell allocated that memory on our behalf and nothing else will free it.
///
/// `DragQueryFileW(h, u32::MAX, ..)` is the documented way to ask for the
/// count; per index, a first call with a null buffer returns the length in
/// characters, excluding the terminator.
pub unsafe fn dropped_paths(hdrop: usize) -> Vec<std::path::PathBuf> {
    use windows_sys::Win32::UI::Shell::{DragFinish, DragQueryFileW};
    let h = hdrop as *mut core::ffi::c_void;
    let mut out = Vec::new();
    unsafe {
        let n = DragQueryFileW(h, u32::MAX, std::ptr::null_mut(), 0);
        for i in 0..n {
            let len = DragQueryFileW(h, i, std::ptr::null_mut(), 0);
            if len == 0 {
                continue;
            }
            let mut buf = vec![0u16; len as usize + 1];
            let got = DragQueryFileW(h, i, buf.as_mut_ptr(), buf.len() as u32);
            if got > 0 {
                out.push(std::path::PathBuf::from(String::from_utf16_lossy(
                    &buf[..got as usize],
                )));
            }
        }
        DragFinish(h);
    }
    out
}

/// Open a path with the OS default handler (Help ▸ Manual — the default
/// browser for the manual's HTML). Fire-and-forget: a failure shows as
/// "nothing happened", which the status line's path already explains.
pub unsafe fn shell_open(path: &std::path::Path) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let w = wide(&path.to_string_lossy());
    let verb = wide("open");
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            w.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            9,
        ); // SW_SHOW
    }
}
