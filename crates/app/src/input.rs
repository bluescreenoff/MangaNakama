//! WM_POINTER pen decoding (plus the little touch decode the tap gestures
//! need).
//!
//! The project-killer risk per docs/ARCHITECTURE.md, so the two rules that
//! actually bite are called out where they happen:
//!   1. `GetPointerPenInfoHistory` returns entries **newest first** — reverse.
//!   2. Pressure is 0..1024, divide by 1024.0 (not 65535, not 255).

use mn_core::PenSample;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::Input::Pointer::{
    GetPointerPenInfo, GetPointerPenInfoHistory, GetPointerTouchInfo, GetPointerType,
    POINTER_FLAG_INCONTACT, POINTER_PEN_INFO, POINTER_TOUCH_INFO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PEN_FLAG_ERASER, PEN_FLAG_INVERTED, PEN_MASK_PRESSURE, PEN_MASK_TILT_X, PEN_MASK_TILT_Y,
    POINTER_INPUT_TYPE, PT_MOUSE, PT_PEN, PT_TOUCH, PT_TOUCHPAD, TOUCH_MASK_CONTACTAREA,
};

/// What kind of device produced a pointer id.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PointerDevice {
    Pen,
    Touch,
    Mouse,
    Other,
}

pub unsafe fn pointer_device(pointer_id: u32) -> PointerDevice {
    let mut t: POINTER_INPUT_TYPE = 0;
    if unsafe { GetPointerType(pointer_id, &mut t) } == 0 {
        return PointerDevice::Other;
    }
    match t {
        PT_PEN => PointerDevice::Pen,
        PT_TOUCH | PT_TOUCHPAD => PointerDevice::Touch,
        PT_MOUSE => PointerDevice::Mouse,
        _ => PointerDevice::Other,
    }
}

/// The two things the tap recogniser (`gesture.rs`) needs about a touch
/// contact and cannot get from the message's `lparam`.
pub struct TouchContact {
    /// `POINTER_INFO::dwTime` — milliseconds on the `GetTickCount` clock, the
    /// same clock the pen path timestamps with. It wraps every 49.7 days;
    /// the recogniser treats a backwards step as "trust nothing".
    pub t_ms: f64,
    /// Longest side of `rcContact` in px, or **0 when the digitizer does not
    /// report a contact area** — which plenty of panels do not, so 0 has to
    /// mean "unknown", never "tiny fingertip".
    pub size_px: f32,
}

/// Decode one touch contact. `None` when the pointer id is not (or is no
/// longer) a touch contact; the caller then falls back to the message time
/// and an unknown contact size.
pub unsafe fn read_touch_contact(pointer_id: u32) -> Option<TouchContact> {
    let mut ti: POINTER_TOUCH_INFO = unsafe { std::mem::zeroed() };
    if unsafe { GetPointerTouchInfo(pointer_id, &mut ti) } == 0 {
        return None;
    }
    // `rcContact` is only meaningful when the mask says so: a zeroed rect on
    // a device without contact-area support would otherwise read as a
    // fingertip-sized patch, which is the one value we must not invent.
    let size_px = if ti.touchMask & TOUCH_MASK_CONTACTAREA != 0 {
        let r = ti.rcContact;
        (r.right - r.left).max(r.bottom - r.top).max(0) as f32
    } else {
        0.0
    };
    Some(TouchContact {
        t_ms: ti.pointerInfo.dwTime as f64,
        size_px,
    })
}

/// One message's worth of pen input: the samples, **plus what the device
/// said about itself**.
///
/// The second half exists because of the shape
/// `docs/CSP-PEN-TABLET-PAINS.md` is full of — the two commonest pen
/// failures are indistinguishable from healthy input once you are looking
/// only at samples. A device that stopped reporting pressure yields the
/// same `0.5` as a device that never had pressure to report (§4.1), and a
/// driver that signals contact through pressure alone yields an EMPTY
/// batch, which looks exactly like a hover (§4.2). Neither can be
/// diagnosed downstream; both are one mask bit up here.
pub struct PenBatch {
    /// In-contact samples, oldest first, in **client** pixels.
    pub samples: Vec<PenSample>,
    /// Pointer reports the history carried **before** the in-contact
    /// filter. `reports > 0 && samples.is_empty()` is §4.2 exactly: input
    /// arrived and we dropped all of it. `reports == 0` means the pointer
    /// id has gone away and this batch describes nothing — every
    /// `WM_POINTERUP` looks like that, so a consumer must not read the
    /// flags below as facts when it is zero.
    pub reports: usize,
    /// `PEN_MASK_PRESSURE` on the newest report. **False means every
    /// `pressure` in `samples` is the 0.5 substitute, not a measurement.**
    pub pressure_reported: bool,
    /// `PEN_MASK_TILT_X`/`_Y` on the newest report; same meaning for the
    /// zeros in `tilt_x`/`tilt_y`.
    pub tilt_reported: bool,
    /// `PEN_FLAG_INVERTED` or `PEN_FLAG_ERASER` — the stylus is tail-end
    /// down. Both bits, because drivers disagree: Wacom sets INVERTED
    /// while the tail hovers and adds ERASER when it touches, and some
    /// third-party drivers only ever set one of the two.
    pub inverted: bool,
}

/// All pen samples coalesced into this message, oldest first, in **client**
/// pixels, and the device facts that came with them.
pub unsafe fn read_pen_batch(hwnd: HWND, pointer_id: u32) -> PenBatch {
    let mut infos = unsafe { pen_history(pointer_id) };

    // (1) The history buffer is newest-first. Painting it in that order draws
    // every coalesced stroke segment backwards, which looks *almost* right and
    // is a nightmare to notice later. Reverse it here, once.
    infos.reverse();

    // Device facts are read from the NEWEST report and read BEFORE the
    // in-contact filter below. That order is the whole point: the batch we
    // are about to empty is precisely the one whose flags have to survive,
    // or "no ink and no explanation" is all the app can ever say about it.
    let reports = infos.len();
    let newest = infos.last();
    let pressure_reported = newest.is_some_and(|pi| pi.penMask & PEN_MASK_PRESSURE != 0);
    let tilt_reported =
        newest.is_some_and(|pi| pi.penMask & (PEN_MASK_TILT_X | PEN_MASK_TILT_Y) != 0);
    let inverted =
        newest.is_some_and(|pi| pi.penFlags & (PEN_FLAG_INVERTED | PEN_FLAG_ERASER) != 0);

    // (3) Drop samples where the pen is not actually touching. The down-event
    // history can include hover entries whose pressure is 0 — the old code
    // substituted 0.5 for those, which stamped a fat blob at every stroke
    // start (owner bug report 2026-08-14).
    infos.retain(|pi| pi.pointerInfo.pointerFlags & POINTER_FLAG_INCONTACT != 0);

    PenBatch {
        samples: infos
            .iter()
            .map(|pi| unsafe { to_sample(hwnd, pi) })
            .collect(),
        reports,
        pressure_reported,
        tilt_reported,
        inverted,
    }
}

unsafe fn pen_history(pointer_id: u32) -> Vec<POINTER_PEN_INFO> {
    let mut count: u32 = 0;
    let ok = unsafe { GetPointerPenInfoHistory(pointer_id, &mut count, std::ptr::null_mut()) };

    if ok != 0 && count > 0 {
        let mut buf: Vec<POINTER_PEN_INFO> = vec![unsafe { std::mem::zeroed() }; count as usize];
        if unsafe { GetPointerPenInfoHistory(pointer_id, &mut count, buf.as_mut_ptr()) } != 0 {
            buf.truncate(count as usize);
            if !buf.is_empty() {
                return buf;
            }
        }
    }

    // No history (or the call failed) — fall back to the single current sample.
    let mut one: POINTER_PEN_INFO = unsafe { std::mem::zeroed() };
    if unsafe { GetPointerPenInfo(pointer_id, &mut one) } != 0 {
        vec![one]
    } else {
        Vec::new()
    }
}

unsafe fn to_sample(hwnd: HWND, pi: &POINTER_PEN_INFO) -> PenSample {
    // `ptPixelLocationRaw` is the un-smoothed device position; the non-raw field
    // has already been through Windows' pointer prediction/filtering, which we
    // do not want in front of our own stabiliser.
    let mut pt: POINT = pi.pointerInfo.ptPixelLocationRaw;
    unsafe { ScreenToClient(hwnd, &mut pt) };

    // (2) 0..1024 -> 0..1. Only a device with NO pressure support at all gets
    // the 0.5 substitute; a real pen reporting 0 means 0 (in-contact samples
    // with pressure 0 exist at the very edge of contact — a light touch, not
    // a half-pressure stamp).
    //
    // The substitute stays — a pen with no pressure must still draw — but it
    // is NOT silent any more: `PenBatch::pressure_reported` carries the mask
    // bit out, the HUD marks the value SUBSTITUTED and the log says so once.
    // Corpus §3.3 is 124 threads of "my pressure stopped working" that no
    // application ever helped anyone confirm.
    let pressure = if pi.penMask & PEN_MASK_PRESSURE != 0 {
        (pi.pressure as f32 / 1024.0).clamp(0.0, 1.0)
    } else {
        0.5
    };

    // Tilt is degrees, -90..90, passed straight through; nothing consumes it
    // until libmypaint lands.
    let tilt_x = if pi.penMask & PEN_MASK_TILT_X != 0 {
        pi.tiltX as f32
    } else {
        0.0
    };
    let tilt_y = if pi.penMask & PEN_MASK_TILT_Y != 0 {
        pi.tiltY as f32
    } else {
        0.0
    };

    PenSample {
        x: pt.x as f32,
        y: pt.y as f32,
        pressure,
        tilt_x,
        tilt_y,
        t_ms: pi.pointerInfo.dwTime as f64,
    }
}
