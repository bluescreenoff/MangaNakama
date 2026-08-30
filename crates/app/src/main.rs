//! MangaNakama — Win32 shell.
//!
//! Raw Win32 by design (docs/ARCHITECTURE.md): no winit, no egui-winit, so pen
//! input is ours end to end and the egui integration is hand-rolled (`shell.rs`).
//!
//! Loop shape: `GetMessageW` blocks until there is input, handlers mutate `App`,
//! and anything that changed pixels calls `InvalidateRect` so exactly one
//! `WM_PAINT` does the render (canvas + egui in the same frame). egui's
//! animation requests come back as a one-shot `WM_TIMER`, so an idle window
//! still costs zero.
//!
//! Input routing: `Shell::owns_pointer` decides per press whether an event is
//! the canvas's or egui's; the choice is latched in `App::{mouse,pen}_owner`
//! until the button/pen comes up.

mod app;
mod bench;
mod clipboard;
mod cmd;
mod drop;
mod gesture;
mod input;
mod input_path;
mod keymap;
mod recovery;
mod remote;
mod screenshot;
mod shell;
mod subtools;
mod testlog;
mod text_edit;
mod ui;
mod win32;

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use app::{App, CaptionCmd, Owner, PointerKind, ScreenRect, WinGeom};
use cmd::{AppCmd, Slot, Tool, dispatch};
use input::{PointerDevice, pointer_device, read_pen_batch, read_touch_contact};
use mn_core::PenSample;
use mn_gpu::{GpuConfig, Renderer};
use subtools::Target;
use win32::{
    MK_SHIFT, TABLET_INK_FLAGS, WM_TABLET_QUERYSYSTEMGESTURESTATUS, is_pen_promoted_mouse, loword,
    lparam_points, wide,
};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
use windows_sys::Win32::Graphics::Gdi::{
    InvalidateRect, ScreenToClient, UpdateWindow, ValidateRect,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::Controls::{
    FEEDBACK_GESTURE_PRESSANDTAP, FEEDBACK_PEN_BARRELVISUALIZATION, FEEDBACK_PEN_DOUBLETAP,
    FEEDBACK_PEN_PRESSANDHOLD, FEEDBACK_PEN_RIGHTTAP, FEEDBACK_PEN_TAP,
    FEEDBACK_TOUCH_CONTACTVISUALIZATION, FEEDBACK_TOUCH_DOUBLETAP, FEEDBACK_TOUCH_PRESSANDHOLD,
    FEEDBACK_TOUCH_RIGHTTAP, FEEDBACK_TOUCH_TAP, SetWindowFeedbackSetting,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, GetSystemMetricsForDpi,
    SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::Ime::{
    CANDIDATEFORM, CFS_CANDIDATEPOS, CFS_POINT, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext,
    ImmSetCandidateWindow, ImmSetCompositionWindow,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_SPACE};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_OWNDC, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageExtraInfo,
    GetMessageTime, GetMessageW, GetWindowLongPtrW, GetWindowRect, HTBOTTOM, HTBOTTOMLEFT,
    HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, IDC_ARROW,
    IDC_CROSS, IDC_HAND, IDC_IBEAM, IDC_NO, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE,
    IDC_SIZEWE, IDC_WAIT, IDNO, IDYES, IsIconic, IsZoomed, KillTimer, LoadCursorW, MB_ICONWARNING,
    MB_YESNO, MB_YESNOCANCEL, MINMAXINFO, MSG, MessageBoxW, PostQuitMessage, RegisterClassExW,
    SM_CXPADDEDBORDER, SM_CXSIZEFRAME, SM_CYSIZEFRAME, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE,
    SW_SHOW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW,
    SetCursor, SetTimer, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow,
    TranslateMessage, WHEEL_DELTA, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_DESTROY, WM_DPICHANGED,
    WM_DROPFILES, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_IME_COMPOSITION,
    WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOVE, WM_NCCALCSIZE,
    WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_PAINT, WM_POINTERCAPTURECHANGED, WM_POINTERDOWN,
    WM_POINTERLEAVE, WM_POINTERUP, WM_POINTERUPDATE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR,
    WM_SETFOCUS, WM_SIZE, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows_sys::core::BOOL;

const CLASS_NAME: &str = "MangaNakamaWindow";
const TITLE: &str = "MangaNakama";
/// Constant pressure for the mouse fallback, so everything is testable without
/// a pen (docs/ARCHITECTURE.md).
const MOUSE_PRESSURE: f32 = 0.5;
/// One-shot timer id for egui's "repaint me in N ms" (animations).
const REPAINT_TIMER: usize = 1;
/// Repeating safety-save timer. The period is the `autosave_min` preference
/// (`prefs.txt`) — 15 minutes shipped, which is the owner's original
/// request; 0 means the timer is never armed at all.
const AUTOSAVE_TIMER: usize = 2;
/// Repeating poll, armed ONLY while the background GPU-inking measurement
/// child is running, killed the moment its verdict lands (or after
/// [`MEASURE_GIVE_UP`] ticks, so a child that dies without writing does not
/// leave a timer ticking for the session). Without this the verdict sat in a
/// file nobody re-read until the next launch.
const MEASURE_TIMER: usize = 3;
const MEASURE_POLL_MS: u32 = 3_000;
/// ~3 minutes. The measurement is a 3-rep bench; if it has not written by
/// then it never will.
const MEASURE_GIVE_UP: u32 = 60;

struct Cli {
    warp: bool,
    novsync: bool,
    /// P1: rasterize brush dabs on the GPU (docs/design/GPU-DABS.md). The
    /// CPU path stays the reference/fallback (per-brush routing + canary
    /// repair), so this only ever changes where the pixels are stamped.
    gpu_dabs: bool,
    screenshot: Option<PathBuf>,
    shot_size: (u32, u32),
    /// --screenshot extra: drive a Transform gesture (corner scale+rotate)
    /// and capture mid-float — proves veil, preview mesh, bbox and handles.
    shot_transform: bool,
    /// --screenshot extra: make a lasso selection through the real input path
    /// — proves the marching ants + the Selection Launcher bar.
    shot_selection: bool,
    /// --screenshot extra: activate a frame folder + Object tool + click a
    /// panel — proves the focus tint, bbox/handles, rotation lollipop.
    shot_framefocus: bool,
    /// --screenshot extra: convert the stroked layer to a tone layer —
    /// proves the halftone renders through the GPU compositor.
    shot_tone: bool,
    /// --screenshot extra: tear Layers off as a floating dock window.
    shot_dock: bool,
    /// --screenshot extra: README-grade shot — no diagnostics window.
    shot_hero: bool,
    /// --e2e-workfolder: drive the whole work-folder storage story through
    /// the real command path (no window, no screenshot) and print verdicts.
    e2e_workfolder: bool,
    /// --bench-dabs: run the dab-path benchmark (P3 re-flip criterion)
    /// and exit, writing manganakama-bench.txt beside the exe.
    bench_dabs: bool,
    /// --bench-verdict: the one-shot measurement child the auto-default
    /// spawns — run the short bench, write `gpu-verdict.txt`, exit.
    bench_verdict: bool,
    /// --e2e-dockdrag: drive the docking system's drag interactions through
    /// the real pointer path and print verdicts.
    e2e_dockdrag: bool,
    /// --e2e-paneresize: drive the dock-column resize edges through the real
    /// pointer path (drag, hover stability, cursor band) and print verdicts.
    e2e_paneresize: bool,
}

fn parse_cli() -> Cli {
    let mut cli = Cli {
        warp: false,
        novsync: false,
        gpu_dabs: false,
        screenshot: None,
        shot_size: (1280, 860),
        shot_transform: false,
        shot_selection: false,
        shot_framefocus: false,
        shot_tone: false,
        shot_dock: false,
        shot_hero: false,
        e2e_workfolder: false,
        bench_dabs: false,
        bench_verdict: false,
        e2e_dockdrag: false,
        e2e_paneresize: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--warp" => cli.warp = true,
            "--novsync" => cli.novsync = true,
            "--gpu-dabs" => cli.gpu_dabs = true,
            "--shot-transform" => cli.shot_transform = true,
            "--shot-selection" => cli.shot_selection = true,
            "--shot-framefocus" => cli.shot_framefocus = true,
            "--shot-tone" => cli.shot_tone = true,
            "--shot-dock" => cli.shot_dock = true,
            "--shot-hero" => cli.shot_hero = true,
            "--e2e-workfolder" => cli.e2e_workfolder = true,
            "--bench-dabs" => cli.bench_dabs = true,
            "--bench-verdict" => cli.bench_verdict = true,
            "--e2e-dockdrag" => cli.e2e_dockdrag = true,
            "--e2e-paneresize" => cli.e2e_paneresize = true,
            "--screenshot" => match args.next() {
                Some(p) => cli.screenshot = Some(PathBuf::from(p)),
                None => {
                    eprintln!("[app] --screenshot needs a path");
                    std::process::exit(1);
                }
            },
            "--shot-size" => {
                let ok = args.next().and_then(|s| {
                    let (w, h) = s.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                });
                match ok {
                    Some(wh) => cli.shot_size = wh,
                    None => {
                        eprintln!("[app] --shot-size needs WxH, e.g. 2560x1392");
                        std::process::exit(1);
                    }
                }
            }
            "--help" | "-h" => {
                // Every flag `parse_cli` accepts, in the order it matches
                // them. A flag that exists but is undocumented is a flag
                // nobody uses: --gpu-dabs and the two bench modes were the
                // whole GPU-inking story and none of them were listed.
                println!(
                    "MangaNakama\n\
                     \n  adapter\n  \
                       --warp             force the software (fallback/WARP) adapter\n  \
                       --novsync          present with AutoNoVsync instead of AutoVsync\n  \
                       --gpu-dabs         force inking onto the GPU (overrides the measured default)\n\
                     \n  screenshots\n  \
                       --screenshot PATH  render one offscreen frame (canvas + UI) to PNG and exit\n  \
                       --shot-size WxH    size for --screenshot (default 1280x860)\n  \
                       --shot-transform   --screenshot extra: an active transform box\n  \
                       --shot-selection   --screenshot extra: an active selection\n  \
                       --shot-framefocus  --screenshot extra: a focused frame folder\n  \
                       --shot-tone        --screenshot extra: a tone layer\n  \
                       --shot-dock        --screenshot extra: Layers torn off as a floating dock\n  \
                       --shot-hero        --screenshot extra: README-grade shot, no diagnostics\n\
                     \n  measurement + end-to-end runs (no window; print and exit)\n  \
                       --bench-dabs       time CPU vs GPU inking, write manganakama-bench.txt\n  \
                       --bench-verdict    the short measurement run, writes gpu-verdict.txt\n  \
                       --e2e-workfolder   drive the work-folder storage path\n  \
                       --e2e-dockdrag     drive the dock drag interactions\n  \
                       --e2e-paneresize   drive the dock-column resize edges\n\
                     \n  --help, -h         this list"
                );
                std::process::exit(0);
            }
            other => eprintln!("[app] ignoring unknown flag {other}"),
        }
    }
    cli
}

fn main() {
    // FIRST statement, before anything can fail: a panic crossing the
    // `extern "system"` wndproc aborts the process — the window vanishes
    // with no save prompt and, until this hook existed, the log ended
    // mid-session looking perfectly healthy. The hook is what turns "it
    // just closed" into a line a tester can send.
    testlog::install_panic_hook();
    let cli = parse_cli();
    let cfg = GpuConfig {
        force_fallback: cli.warp,
        no_vsync: cli.novsync,
    };

    if let Some(path) = &cli.screenshot {
        match screenshot::run(
            cfg,
            path,
            cli.shot_size,
            cli.shot_transform,
            cli.shot_selection,
            cli.shot_framefocus,
            cli.shot_tone,
            cli.shot_dock,
            cli.shot_hero,
            cli.gpu_dabs,
        ) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("[app] screenshot failed: {e}");
                std::process::exit(3);
            }
        }
    }

    if cli.bench_dabs {
        match bench::bench_dabs(cfg, 20) {
            Ok(table) => {
                println!("{table}");
                match bench::bench_write(&table) {
                    Some(p) => println!("[bench] written to {}", p.display()),
                    None => eprintln!("[bench] could not write the log file"),
                }
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("[bench] failed: {e}");
                std::process::exit(3);
            }
        }
    }
    if cli.bench_verdict {
        // The auto-default's measurement child: short bench, verdict file,
        // exit. A failure writes nothing — the next launch retries.
        match bench::quick_verdict(cfg, 3) {
            Ok(v) => {
                bench::store_verdict(&v);
                let msg = format!(
                    "[bench] background measurement done: GPU inking {} on this GPU ({})",
                    if v.on { "wins" } else { "loses" },
                    v.summary
                );
                println!("{msg}");
                // Into the tester log too. This child is a GUI-subsystem
                // process with nowhere for stdout to go, so without this
                // line the whole measurement is invisible: the user sees
                // neither that it ran nor what it decided.
                testlog::line(&msg);
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("[bench] verdict failed: {e}");
                testlog::line(&format!("[bench] background measurement failed: {e}"));
                std::process::exit(3);
            }
        }
    }

    if cli.e2e_workfolder {
        match screenshot::workfolder_e2e(cfg) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("[app] work-folder e2e failed: {e}");
                std::process::exit(3);
            }
        }
    }

    if cli.e2e_dockdrag {
        match screenshot::dockdrag_e2e(cfg) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("[app] dock-drag e2e failed: {e}");
                std::process::exit(3);
            }
        }
    }

    if cli.e2e_paneresize {
        match screenshot::paneresize_e2e(cfg) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("[app] pane-resize e2e failed: {e}");
                std::process::exit(3);
            }
        }
    }

    // Per-monitor-v2 before any window exists, so client pixels == device pixels
    // and the pen's screen coordinates land where we think they do.
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
            eprintln!("[app] per-monitor-v2 DPI awareness unavailable; coordinates may drift");
        }
    }

    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = wide(CLASS_NAME);
    let title = wide(TITLE);

    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW | CS_OWNDC,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance as _,
        hIcon: std::ptr::null_mut(),
        // The cursor is chosen per frame in WM_SETCURSOR (egui asks for its own
        // over the panels), so the class cursor is only the fallback.
        hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
        // No background brush: wgpu owns every pixel, GDI erasing would flicker.
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
        hIconSm: std::ptr::null_mut(),
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        eprintln!("[app] RegisterClassExW failed");
        std::process::exit(1);
    }
    println!("[app] window class registered: {CLASS_NAME}");

    // Remembered window placement: restore where the window was (which
    // monitor included) as long as that monitor is still connected; a
    // saved maximized flag re-maximizes at show time.
    let start = match app::peek_win() {
        Some(g) if g.fits_some_monitor(&monitor_rects()) => {
            println!("[app] restoring window at {},{} {}x{}", g.x, g.y, g.w, g.h);
            g
        }
        _ => WinGeom {
            x: CW_USEDEFAULT,
            y: CW_USEDEFAULT,
            w: 1280,
            h: 860,
            max: false,
        },
    };

    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            start.x,
            start.y,
            start.w,
            start.h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance as _,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        eprintln!("[app] CreateWindowExW failed");
        std::process::exit(1);
    }

    // Custom chrome part 1: the DWM-drawn frame (1px border + shadow) follows
    // the immersive dark mode attribute — without this the border is WHITE on
    // a light-theme Windows 10 even though we paint everything else dark.
    // Attribute 20 on 19041+, 19 on the two older builds; try both.
    unsafe {
        let dark: i32 = 1;
        for attr in [20u32, 19u32] {
            if DwmSetWindowAttribute(
                hwnd,
                attr,
                &dark as *const i32 as *const c_void,
                size_of::<i32>() as u32,
            ) == 0
            {
                break;
            }
        }
    }

    disable_pen_feedback(hwnd);

    // Files can be dropped on us from this point on (IO-041). Registered
    // after creation and before the window is shown, so the very first frame
    // already accepts a drop.
    unsafe { win32::accept_dropped_files(hwnd as isize) };

    // Custom chrome part 3: force a WM_NCCALCSIZE(TRUE) re-evaluation NOW,
    // before the window is ever shown. On this Win10 (19044) CreateWindowExW
    // only sends WM_NCCALCSIZE with wParam=FALSE (traced: the TRUE variant
    // never arrives during creation), so DefWindowProc lays out the native
    // caption and keeps it until the first manual resize — the owner saw two
    // title bars, ours and Windows', at every startup. SWP_FRAMECHANGED is the
    // canonical nudge: recompute the frame with our handler, no size/move.
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }

    // Assert the requested geometry AFTER creation: Win10's creation-time
    // WM_DPICHANGED interplay can shrink a freshly created window (observed
    // 853x573 instead of 1280x860 on a 150 % monitor) — and a restored
    // position must win over whatever CW_USEDEFAULT did. The last word is
    // ours; this is also the remembered-geometry restore.
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rc);
        let (x, y) = if start.x == CW_USEDEFAULT {
            (rc.left, rc.top) // keep the system's default placement…
        } else {
            (start.x, start.y) // …but honor a restored position exactly
        };
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            start.w,
            start.h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    note_geom_now(hwnd);

    let (cw, ch) = client_size(hwnd);
    // SAFETY: `hwnd` stays alive until WM_DESTROY, where the App (and with it
    // the Renderer holding the surface) is dropped.
    let renderer =
        match unsafe { Renderer::new_windowed(hwnd as isize, hinstance as isize, cw, ch, cfg) } {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[app] renderer init failed: {e}");
                std::process::exit(2);
            }
        };
    println!("[app] adapter: {}", renderer.adapter_line());

    let ppp = window_ppp(hwnd);
    println!("[app] dpi scale: {ppp:.2}x");
    let app = Box::new(App::new(renderer, (cw, ch), ppp));
    let mut app = app;
    // The UI-size preference multiplies the window DPI from the first
    // frame (prefs load inside App::new, so the scale is only known now;
    // dpi_changed is a no-op at 1.0).
    if (app.prefs.ui_scale - 1.0).abs() > 1e-4 {
        let s = app.prefs.ui_scale;
        app.dpi_changed(ppp * s);
    }
    // The reader's F11 fullscreen needs the window handle (tests run
    // headless with hwnd == 0 — state-only).
    app.hwnd = hwnd as isize;
    // Put every tool back in the sub tool group and row it was left in
    // (owner ask 2026-08-25). Here rather than in `App::new` on purpose:
    // the test suite builds Apps by the hundred and must not inherit the
    // developer's own ui.txt through them.
    subtools::restore_from_memory(&mut app);
    // GPU dabs on at startup — DECISIONS 8.9's re-flip, implemented as a
    // MEASURED per-adapter auto-default: an explicit choice (the --gpu-dabs
    // flag or a gpu_dabs= line the user's ui.txt actually carries) always
    // wins; otherwise a stored `gpu-verdict.txt` for THIS adapter decides;
    // otherwise the app stays on cpu and spawns a one-shot measurement
    // child (`--bench-verdict`, the real bench, no window) whose number
    // takes effect from the NEXT launch. Everything is ANDed with adapter
    // support, so a ui.txt carried to a weaker machine just stays on cpu.
    let explicit = if cli.gpu_dabs {
        Some(true)
    } else if app.layout.gpu_dabs_explicit {
        Some(app.layout.gpu_dabs)
    } else {
        None
    };
    let (want_gpu_dabs, spawn_measurement) = bench::resolve_auto(
        explicit,
        bench::load_verdict(),
        &app.renderer.adapter_line(),
    );
    app.gpu_dabs = want_gpu_dabs && app.renderer.gpu_dabs_supported();
    if want_gpu_dabs && !app.gpu_dabs {
        println!("[app] gpu dabs requested (flag/ui.txt) but unsupported here — CPU dab path");
    }
    if explicit.is_none() && app.gpu_dabs {
        app.set_status(
            "inking runs on the GPU: measured faster on this machine (View menu to change)",
        );
    }
    // LP-022 page half: restore the saved monochrome preview. Display-only,
    // so applying it here is just one renderer flag.
    app.renderer.mono_preview = app.layout.mono_preview;
    // Whether the measurement child started, in words, for the log block
    // below — a user whose GPU was never measured has to be able to see
    // that the attempt happened (or why it could not).
    let mut bench_note: Option<String> = None;
    if spawn_measurement && app.renderer.gpu_dabs_supported() {
        // Detached, windowless (GUI-subsystem exe), exits on its own. If it
        // dies the verdict file stays absent and the next launch retries.
        bench_note = Some(match std::env::current_exe() {
            Ok(exe) => match std::process::Command::new(exe)
                .arg("--bench-verdict")
                .spawn()
            {
                Ok(child) => {
                    // Watch for the verdict THIS session. Armed only here,
                    // killed by the first tick that finds it.
                    unsafe { SetTimer(hwnd, MEASURE_TIMER, MEASURE_POLL_MS, None) };
                    format!(
                        "[app] gpu-inking: measuring this GPU in the background (pid {}); \
                         the result applies as soon as it finishes",
                        child.id()
                    )
                }
                Err(e) => format!("[app] gpu-inking: could not start the measurement: {e}"),
            },
            Err(e) => format!("[app] gpu-inking: could not locate this exe to measure: {e}"),
        });
    }
    // PR-040: ask the PREVIOUS session's verdict before this session appends
    // its own banner to the same file — one line later and the question
    // answers itself.
    let crashed_last_time = recovery::last_session_crashed();

    // The tester log's session banner — what a GitHub issue needs first
    // (README ▸ Testers): adapter identity, GPU-dab capability + routing.
    let mut banner = vec![
        format!(
            "[app] version: {} ({})",
            env!("CARGO_PKG_VERSION"),
            env!("MN_BUILD_SHA")
        ),
        format!("[app] adapter: {}", app.renderer.adapter_line()),
        format!(
            "[app] gpu-dabs: supported={} enabled={}",
            app.renderer.gpu_dabs_supported(),
            app.gpu_dabs
        ),
        // The same sentence Preferences shows, from the same function: the
        // supported/enabled pair above says WHAT, this says WHY, and the
        // two are read from one place so they cannot disagree.
        format!("[app] {}", bench::state_line_for(&app)),
        format!("[app] dpi scale: {ppp:.2}"),
        format!("[app] flags: warp={} gpu_dabs={}", cli.warp, app.gpu_dabs),
    ];
    banner.extend(bench_note);
    // How to get this file to the dev — the log is its own instruction
    // sheet, because nobody rereads a README when something breaks.
    banner.push(
        "[log] safe to share as-is (no names, no paths). Send it to \
         github.com/bluescreenoff/MangaNakama/issues or bluescreen.off@gmail.com"
            .to_owned(),
    );
    testlog::begin_session(&banner);
    // Read the two preferences this function needs BEFORE the App moves into
    // the window's user data and stops being reachable by name.
    let autosave_ms = app.prefs.autosave_ms();
    let recent_depth = app.prefs.recent_depth;
    let automation = app.prefs.automation;
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize) };

    // Tier 3 automation socket (remote.rs): opt-in via prefs. When it is
    // off, also clear any stale `automation.txt` a crashed session left —
    // a discovery file must never advertise a port nobody holds.
    if automation {
        match remote::start(hwnd as isize) {
            Ok(port) => {
                with_app(hwnd, |a| {
                    a.set_status(format!("automation server on 127.0.0.1:{port} (automation.txt)"))
                });
            }
            Err(e) => eprintln!("[app] automation socket failed to bind: {e}"),
        }
    } else {
        remote::remove_auto_file();
    }

    unsafe {
        ShowWindow(hwnd, if start.max { SW_MAXIMIZE } else { SW_SHOW });
        UpdateWindow(hwnd);
        // `autosave_min=0` means OFF: no timer at all, rather than a timer
        // firing at some huge interval. `pump_commands` re-arms this when
        // the Preferences panel changes the interval.
        if autosave_ms > 0 {
            SetTimer(hwnd, AUTOSAVE_TIMER, autosave_ms, None);
        }
    }
    // PR-040 — the autosave's second half. We have written `.autosave.mnc`
    // on a timer for months and never once offered it back; a timer that
    // writes a file nobody reads is not a safety net. Ask now, before the
    // user starts drawing over what was lost, and only when the last
    // session failed to write its clean-exit marker.
    if crashed_last_time
        && let Some(p) =
            recovery::newest_autosave(&app::load_recent_n(recent_depth), &std::env::temp_dir())
    {
        let text = wide(
            "MangaNakama did not exit cleanly last time.\n\n\
             An autosaved copy is newer than the file it belongs to.\n\
             Open the autosave now?\n\n\
             Choosing No changes nothing on disk — the autosave stays where it is.",
        );
        let caption = wide("Recover unsaved work");
        let r = unsafe {
            MessageBoxW(
                hwnd,
                text.as_ptr(),
                caption.as_ptr(),
                MB_YESNO | MB_ICONWARNING,
            )
        };
        // Deliberately no path in the log line: the log's whole promise is
        // that a tester can post it without reading it first.
        testlog::line(if r == IDYES {
            "[recover] autosave offered, accepted"
        } else {
            "[recover] autosave offered, declined"
        });
        if r == IDYES {
            with_app(hwnd, |a| a.push_cmd(AppCmd::OpenOraPath(p)));
            pump_commands(hwnd);
            // The recovered document must NOT keep the autosave as its path.
            // Otherwise Ctrl+S writes back into `<name>.autosave.mnc`, and
            // the next real save of the document it shadows DELETES the file
            // the user has been saving into. Clearing it forces a Save As,
            // which is the honest ask after a recovery.
            with_app(hwnd, |a| {
                a.set_doc_path(None);
                a.set_status("recovered from the autosave — use Save As to choose where it lives");
            });
        }
    }

    println!(
        "[app] ready. pen or left-drag draws | space/middle drag pans | wheel zooms | \
         F1 diagnostics"
    );

    // Idle-wait loop: GetMessageW blocks, so an untouched window costs 0% CPU.
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    loop {
        let r = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if r <= 0 {
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Commands run here, *outside* the wndproc: a file dialog pumps the
        // message queue, which re-enters `wndproc` — doing that while a
        // `&mut App` is alive up the stack would alias it.
        pump_commands(hwnd);
    }
    // A discovery file must not outlive its port: clients finding a stale
    // automation.txt would knock on a socket nobody holds (the crash case
    // is healed at the next launch instead).
    remote::remove_auto_file();
    // The marker whose ABSENCE is the crash signal — the only way to see
    // the class no Rust hook can catch (a stack overflow is killed by the
    // OS outright). A session block with no tail crashed.
    testlog::end_session();
    println!("[app] bye");
}

fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };
    (
        (rc.right - rc.left).max(1) as u32,
        (rc.bottom - rc.top).max(1) as u32,
    )
}

// --- remembered window placement -------------------------------------------
//
// The window reopens on the monitor (and at the size/position) it was closed
// on. Geometry is tracked in a static (main thread only; WM_MOVE/WM_SIZE fire
// before the App exists), pushed into `UiLayout` at drag/resize END and at
// destroy — a crash loses at most the last unfinished drag, and no file
// writes happen mid-drag.

static GEOM: std::sync::Mutex<Option<WinGeom>> = std::sync::Mutex::new(None);

/// Refresh the tracked geometry from the live window. While maximized the
/// RESTORED rect must survive (GetWindowRect would overwrite it with the
/// maximized bounds), so only the flag updates then.
fn note_geom_now(hwnd: HWND) {
    let zoomed = unsafe { IsZoomed(hwnd) } != 0;
    let iconic = unsafe { IsIconic(hwnd) } != 0;
    let rect = (!zoomed && !iconic).then(|| {
        let mut rc: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(hwnd, &mut rc) };
        (rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top)
    });
    let mut g = GEOM.lock().unwrap();
    let mut base = *g.get_or_insert(WinGeom {
        x: CW_USEDEFAULT,
        y: CW_USEDEFAULT,
        w: 1280,
        h: 860,
        max: false,
    });
    if let Some((x, y, w, h)) = rect {
        base.x = x;
        base.y = y;
        base.w = w;
        base.h = h;
    }
    base.max = zoomed;
    *g = Some(base);
}

/// Hand the tracked geometry to the App for persistence (drag/resize end).
fn persist_geom(app: &mut App) {
    if let Some(g) = *GEOM.lock().unwrap() {
        app.layout.note_win(&g.to_line());
    }
}

// --- touch tap gestures ----------------------------------------------------
//
// CSP `GS-001`/`GS-002`/`GS-013`: two-finger tap undoes, three-finger tap
// redoes, three fingers on the Navigator put the view back upright and
// unmirrored. The decision — tap or the first two events of a pan, finger or
// palm — is `gesture.rs`, which is pure and unit-tested; this side only feeds
// it events and spends what it returns. The recogniser sits BESIDE the
// existing pan/pinch path rather than in front of it: it never swallows an
// event, so a tap still nudges the view by the pixel or two the fingers
// actually moved.
//
// Like GEOM above this is main-thread-only wndproc state, so it lives in a
// static rather than on `App`.

static TAPS: std::sync::Mutex<gesture::Taps> = std::sync::Mutex::new(gesture::Taps::new());

/// The Navigator palette's rect in CLIENT PIXELS, when it is the visible tab
/// of some dock leaf (any column, docked or torn off). egui_dock records each
/// leaf's laid-out rect in POINTS; touch positions arrive in pixels, hence
/// the `ppp` multiply. `None` = the palette is closed or hidden behind
/// another tab, and a three-finger tap there is an ordinary redo.
fn navigator_rect_px(app: &App, ppp: f32) -> Option<[f32; 4]> {
    let r = app.dock.iter_all_nodes().find_map(|(_, node)| {
        let leaf = node.get_leaf()?;
        (leaf.tabs.get(leaf.active.0)
            == Some(&crate::ui::dock::Pane::Palette(
                crate::ui::dock::Palette::Navigator,
            )))
        .then_some(leaf.rect)
    })?;
    r.is_positive()
        .then(|| [r.min.x * ppp, r.min.y * ppp, r.max.x * ppp, r.max.y * ppp])
}

/// All connected monitors as screen rects.
fn monitor_rects() -> Vec<ScreenRect> {
    unsafe extern "system" fn cb(_hmon: HMONITOR, _hdc: HDC, rc: *mut RECT, data: LPARAM) -> BOOL {
        unsafe {
            let v = &mut *(data as *mut Vec<ScreenRect>);
            v.push(((*rc).left, (*rc).top, (*rc).right, (*rc).bottom));
        }
        1
    }
    let mut v: Vec<ScreenRect> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(cb),
            &mut v as *mut Vec<ScreenRect> as LPARAM,
        );
    }
    v
}

// --- custom chrome (no native title bar) ----------------------------------
//
// The classic "borderless window with a frame" recipe: WS_OVERLAPPEDWINDOW
// stays (so the DWM shadow, snap, and the minimize/maximize animations keep
// working), but WM_NCCALCSIZE makes the whole window rect client — the
// caption simply never gets a region. The egui menu bar IS the title bar
// (drag strip + – □ × buttons, ui/top.rs); resize borders are classified by
// hand in WM_NCHITTEST because DefWindowProc sees client == window and would
// report HTCLIENT everywhere.

/// Frame thickness (resize border + invisible padding) for `hwnd`'s DPI.
unsafe fn frame_metrics(hwnd: HWND) -> (i32, i32) {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let m = |idx: i32| unsafe { GetSystemMetricsForDpi(idx, dpi) };
    (
        m(SM_CXSIZEFRAME) + m(SM_CXPADDEDBORDER),
        m(SM_CYSIZEFRAME) + m(SM_CXPADDEDBORDER),
    )
}

/// Remove the caption; when maximized the window rect hangs off-screen by the
/// frame, so inset the client back or the top/left pixel rows would be lost.
unsafe fn nc_calc_size(hwnd: HWND, lparam: LPARAM) -> LRESULT {
    if unsafe { IsZoomed(hwnd) } != 0 {
        let (fx, fy) = unsafe { frame_metrics(hwnd) };
        let rc = lparam as *mut RECT; // rgrc[0]
        unsafe {
            (*rc).left += fx;
            (*rc).top += fy;
            (*rc).right -= fx;
            (*rc).bottom -= fy;
        }
    }
    0
}

/// Which resize edge (if any) is under the cursor. Screen coordinates in
/// `lparam`; maximized windows are not resizable.
unsafe fn nc_hit_test(hwnd: HWND, lparam: LPARAM) -> LRESULT {
    if unsafe { IsZoomed(hwnd) } != 0 {
        return HTCLIENT as LRESULT;
    }
    let (sx, sy) = lparam_points(lparam);
    let mut pt = POINT { x: sx, y: sy };
    unsafe { ScreenToClient(hwnd, &mut pt) };
    let (w, h) = {
        let (w, h) = client_size(hwnd);
        (w as i32, h as i32)
    };
    let (b, _) = unsafe { frame_metrics(hwnd) };
    let (x, y) = (pt.x, pt.y);
    let hit = if y < b {
        if x < b {
            HTTOPLEFT
        } else if x >= w - b {
            HTTOPRIGHT
        } else {
            HTTOP
        }
    } else if y >= h - b {
        if x < b {
            HTBOTTOMLEFT
        } else if x >= w - b {
            HTBOTTOMRIGHT
        } else {
            HTBOTTOM
        }
    } else if x < b {
        HTLEFT
    } else if x >= w - b {
        HTRIGHT
    } else {
        HTCLIENT
    };
    hit as LRESULT
}

fn window_ppp(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
}

/// Kill every pen/touch visual (the ripple, the press-and-hold ring). They are
/// drawn by the system on top of our swapchain and add perceived latency.
fn disable_pen_feedback(hwnd: HWND) {
    const FEEDBACKS: [i32; 11] = [
        FEEDBACK_TOUCH_CONTACTVISUALIZATION,
        FEEDBACK_PEN_BARRELVISUALIZATION,
        FEEDBACK_PEN_TAP,
        FEEDBACK_PEN_DOUBLETAP,
        FEEDBACK_PEN_PRESSANDHOLD,
        FEEDBACK_PEN_RIGHTTAP,
        FEEDBACK_TOUCH_TAP,
        FEEDBACK_TOUCH_DOUBLETAP,
        FEEDBACK_TOUCH_PRESSANDHOLD,
        FEEDBACK_TOUCH_RIGHTTAP,
        FEEDBACK_GESTURE_PRESSANDTAP,
    ];
    let off: i32 = 0; // BOOL FALSE
    for f in FEEDBACKS {
        unsafe {
            SetWindowFeedbackSetting(
                hwnd,
                f,
                0,
                size_of::<i32>() as u32,
                &off as *const i32 as *const c_void,
            );
        }
    }
}

unsafe fn app_ptr(hwnd: HWND) -> *mut App {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App }
}

/// Borrow the `App` for exactly the length of `f`. Nothing that can re-enter the
/// message loop (file dialogs!) may run inside.
fn with_app<R>(hwnd: HWND, f: impl FnOnce(&mut App) -> R) -> Option<R> {
    let p = unsafe { app_ptr(hwnd) };
    if p.is_null() {
        None
    } else {
        Some(f(unsafe { &mut *p }))
    }
}

fn flush_redraw(hwnd: HWND, app: &mut App) {
    if app.take_redraw() {
        unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
    }
}

/// One tick of the background measurement watch. Returns `true` when the
/// poll is finished and its timer should be killed.
///
/// The measurement child writes `gpu-verdict.txt` and exits; nothing used to
/// re-read it, so a machine measured at 9× faster kept inking on the CPU
/// until the user happened to restart. `gpu_dabs` is read per stroke, so
/// flipping it here is safe mid-session — the change takes effect from the
/// next stroke and never mid-stroke. The switch is NOT marked explicit: this
/// is still the measured auto-default, and the View menu must keep saying so.
fn measurement_poll(app: &mut App) -> bool {
    static TICKS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = TICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let Some(on) = bench::measured_for(&app.renderer.adapter_line()) else {
        return n >= MEASURE_GIVE_UP;
    };
    let want = on && app.renderer.gpu_dabs_supported();
    if want != app.gpu_dabs {
        app.gpu_dabs = want;
        app.set_status(if want {
            "inking now runs on the GPU: measured faster on this machine (View menu to change)"
        } else {
            "inking stays on the CPU: measured faster there on this machine (View menu to change)"
        });
    }
    println!("[app] {}", bench::state_line_for(app));
    true
}

/// Drain queued commands. File dialogs are resolved here, between borrows.
fn pump_commands(hwnd: HWND) {
    loop {
        let Some(cmd) = with_app(hwnd, |a| a.cmds.pop_front()).flatten() else {
            break;
        };
        // No `&mut App` alive across this call.
        let Some(cmd) = resolve_dialog(hwnd, cmd) else {
            continue;
        };
        with_app(hwnd, |a| dispatch(a, cmd));
    }

    // The Preferences panel changed the autosave interval: re-arm the
    // repeating timer. `SetTimer` on a live id replaces the period, but
    // killing first keeps the 0 = off case honest — the timer stops
    // existing rather than being scheduled at some huge interval.
    if let Some(ms) = with_app(hwnd, |a| a.autosave_rearm.take()).flatten() {
        unsafe {
            KillTimer(hwnd, AUTOSAVE_TIMER);
            if ms > 0 {
                SetTimer(hwnd, AUTOSAVE_TIMER, ms, None);
            }
        }
    }

    // The Preferences automation toggle: open or gate the remote socket
    // live (remote.rs; off = refuse auth and requests, the port itself
    // stays bound until restart).
    if let Some(on) = with_app(hwnd, |a| a.automation_apply.take()).flatten() {
        if on {
            match remote::start(hwnd as isize) {
                Ok(port) => {
                    with_app(hwnd, |a| {
                        a.set_status(format!(
                            "automation server on 127.0.0.1:{port} (automation.txt)"
                        ))
                    });
                }
                Err(e) => {
                    with_app(hwnd, |a| {
                        a.set_status(format!("automation socket failed to bind: {e}"))
                    });
                }
            }
        } else {
            remote::stop();
            with_app(hwnd, |a| a.set_status("automation server off"));
        }
    }

    // The Preferences panel moved the UI-size slider: the effective
    // pixels-per-point is window DPI × scale, applied through the same
    // door a monitor DPI change uses (viewport zoom compensates, so the
    // artwork's on-screen size does not move — only the chrome does).
    if let Some(scale) = with_app(hwnd, |a| a.ui_scale_apply.take()).flatten() {
        let base = window_ppp(hwnd);
        with_app(hwnd, |a| a.dpi_changed(base * scale));
    }

    // PR-041: "save recovery data for every operation" — the same
    // `AppCmd::Autosave` the timer fires, driven by the work instead of by
    // the clock. It runs AFTER the drain, not inside it, for the reason the
    // drain loop exists at all: no `&mut App` may be alive while a command
    // runs, and this one writes a file.
    if with_app(hwnd, |a| a.autosave_op_due()).unwrap_or(false) {
        with_app(hwnd, |a| dispatch(a, AppCmd::Autosave));
    }

    // The References palette's "Add images…" button. Like every other file
    // dialog it is resolved HERE, not inside the UI build: a dialog pumps the
    // message queue, which re-enters the wndproc, and `App::render` holds a
    // `&mut App` for the whole frame (docs/ARCHITECTURE.md).
    if with_app(hwnd, |a| std::mem::take(&mut a.refs.want_add)).unwrap_or(false) {
        let picked = rfd::FileDialog::new()
            .set_title("Add reference images")
            .add_filter(
                "Images",
                &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp", "gif"],
            )
            .pick_files();
        if let Some(files) = picked {
            with_app(hwnd, |a| {
                let added = a.refs.add(files);
                let lines = a.refs.to_lines();
                a.layout.note_references(&lines);
                a.set_status(match added {
                    0 => "already in the References palette".to_owned(),
                    1 => "1 reference added".to_owned(),
                    n => format!("{n} references added"),
                });
                a.mark_dirty();
            });
        }
    }

    // Custom title bar: start a window move (the system modal loop pumps
    // messages — WM_PAINT re-enters the wndproc — so it must run HERE, with
    // no `&mut App` alive up the stack). End egui's drag first: the move
    // loop consumes the button-up and egui would keep a phantom press.
    if with_app(hwnd, |a| std::mem::take(&mut a.drag_window)).unwrap_or(false) {
        with_app(hwnd, |a| {
            let (x, y) = a.last_pointer;
            a.shell
                .on_pointer_button(x, y, egui::PointerButton::Primary, false);
            a.shell.on_pointer_gone();
        });
        unsafe {
            ReleaseCapture();
            SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as WPARAM, 0);
        }
    }
    match with_app(hwnd, |a| a.caption_cmd.take()).flatten() {
        Some(CaptionCmd::Minimize) => unsafe {
            ShowWindow(hwnd, SW_MINIMIZE);
        },
        Some(CaptionCmd::ToggleMax) => unsafe {
            if IsZoomed(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            } else {
                ShowWindow(hwnd, SW_MAXIMIZE);
            }
        },
        None => {}
    }

    // Closing ONE DOCUMENT (the tab ×). Same shape as the app close below
    // and for the same reason: the prompt needs a message loop, so it cannot
    // run in the wndproc where the click happened.
    //
    // The tab × shipped for one round with no dirty check at all and threw
    // away a tab's unsaved work on a single click. Adding a second way to
    // close a document meant every way needed auditing, and this one had not
    // been.
    if let Some(i) = with_app(hwnd, |a| a.close_doc_requested.take()).flatten() {
        if with_app(hwnd, |a| a.doc_dirty(i)).unwrap_or(false) {
            // Ask about a document the user can SEE.
            with_app(hwnd, |a| {
                a.switch_doc(i);
            });
            let text = wide("Save changes before closing this document?");
            let caption = wide("MangaNakama");
            let r = unsafe {
                MessageBoxW(
                    hwnd,
                    text.as_ptr(),
                    caption.as_ptr(),
                    MB_YESNOCANCEL | MB_ICONWARNING,
                )
            };
            match r {
                r if r == IDYES => {
                    if let Some(cmd) = resolve_dialog(hwnd, AppCmd::SaveOra) {
                        with_app(hwnd, |a| dispatch(a, cmd));
                    }
                    // Cancelled or failed save: keep the document open. This
                    // is the branch that makes the prompt worth having.
                    if with_app(hwnd, |a| a.dirty()).unwrap_or(false) {
                        return;
                    }
                }
                r if r == IDNO => with_app(hwnd, |a| a.discard_changes()).unwrap_or_default(),
                _ => return,
            }
        }
        // Clean now, one way or another. `close_doc` refuses the last
        // document, and closing the only one means closing the app — which
        // the flow below owns.
        if !with_app(hwnd, |a| a.close_doc(i)).unwrap_or(false) {
            with_app(hwnd, |a| a.close_requested = true);
        }
    }

    // Close flow: prompt about unsaved changes, then really destroy.
    //
    // EVERY open document, not just the visible one. Since documents became
    // tabs, `dirty()` speaks only for the active tab, and quitting on that
    // answer alone would discard the work in all the others silently. So:
    // find a dirty document, SWITCH TO IT so the prompt is about something
    // the user can see, ask, and go round again.
    if with_app(hwnd, |a| std::mem::take(&mut a.close_requested)).unwrap_or(false) {
        // One pass per dirty document. The loop lives HERE rather than
        // re-arming `close_requested` and waiting for another message: an
        // idle window might not deliver one, and a half-finished close is
        // the worst state to leave a save prompt in. No `&mut App` is alive
        // across the MessageBox, which is the rule this whole function obeys.
        loop {
            let Some(i) = with_app(hwnd, |a| a.first_dirty_doc()).flatten() else {
                unsafe { DestroyWindow(hwnd) };
                return;
            };
            // Ask about a document the user can SEE.
            with_app(hwnd, |a| {
                if i != a.active_doc {
                    a.switch_doc(i);
                }
            });
            let text = wide("Save changes before closing?");
            let caption = wide("MangaNakama");
            let r = unsafe {
                MessageBoxW(
                    hwnd,
                    text.as_ptr(),
                    caption.as_ptr(),
                    MB_YESNOCANCEL | MB_ICONWARNING,
                )
            };
            match r {
                r if r == IDYES => {
                    if let Some(cmd) = resolve_dialog(hwnd, AppCmd::SaveOra) {
                        with_app(hwnd, |a| dispatch(a, cmd));
                    }
                    // A save can be cancelled at the file dialog, or fail.
                    // Either way the document is still dirty, and closing
                    // now would be the data loss the prompt exists to stop.
                    if with_app(hwnd, |a| a.dirty()).unwrap_or(false) {
                        return;
                    }
                }
                // Discard THIS document's changes, then ask about the next.
                r if r == IDNO => with_app(hwnd, |a| a.discard_changes()).unwrap_or_default(),
                // Cancel abandons the whole close: nothing saved, nothing
                // discarded, no further tabs asked about.
                _ => return,
            }
        }
    }

    // Title bar: "*name - MangaNakama", polled against the last applied value.
    let changed = with_app(hwnd, |a| {
        let t = a.desired_title();
        if t != a.last_title {
            a.last_title = t.clone();
            Some(t)
        } else {
            None
        }
    })
    .flatten();
    if let Some(title) = changed {
        let w = wide(&title);
        unsafe { SetWindowTextW(hwnd, w.as_ptr()) };
    }
    with_app(hwnd, |a| flush_redraw(hwnd, a));
}

/// Turn "ask the user for a path" commands into their resolved forms.
fn resolve_dialog(hwnd: HWND, cmd: AppCmd) -> Option<AppCmd> {
    let current = || with_app(hwnd, |a| a.doc_path.clone()).flatten();
    let default_save_name = || {
        with_app(hwnd, |a| {
            let stem = if a.story.trim().is_empty() {
                "untitled"
            } else {
                a.story.trim()
            };
            if a.is_comic() {
                format!("{stem}.mnc")
            } else {
                format!("{stem}.ora")
            }
        })
        .unwrap_or_else(|| "untitled.ora".to_owned())
    };
    // Comics save into a user-chosen folder (the native CSP-style work
    // folder: work.mnc index + pNNN.ora side by side — the folder may already
    // exist, save_work_folder guards against clobbering a foreign work).
    let is_comic = || with_app(hwnd, |a| a.is_comic()).unwrap_or(false);
    let pick_work_folder = |title: &str| {
        rfd::FileDialog::new()
            .set_title(title)
            .pick_folder()
            .map(|d| AppCmd::SaveOraPath(d.join(mn_core::project::WORKFOLDER_INDEX)))
    };
    match cmd {
        AppCmd::OpenOra => rfd::FileDialog::new()
            .set_title("Open")
            .add_filter("MangaNakama Comic / OpenRaster", &["mnc", "ora"])
            .pick_file()
            .map(AppCmd::OpenOraPath),
        AppCmd::SaveOra => match current() {
            Some(p) => Some(AppCmd::SaveOraPath(p)),
            None if is_comic() => pick_work_folder("Save Work Folder"),
            None => save_comic_dialog("Save", &default_save_name()).map(AppCmd::SaveOraPath),
        },
        AppCmd::SaveOraAs => {
            if is_comic() {
                pick_work_folder("Save Work Folder As")
            } else {
                save_comic_dialog("Save As", &default_save_name()).map(AppCmd::SaveOraPath)
            }
        }
        AppCmd::ExportMnc => {
            // Always .mnc: `default_save_name` picks by is_comic(), which on
            // a single page suggested "untitled.ora" in a dialog whose whole
            // point is the single-file comic (owner report, 2026-08-21).
            let name = default_save_name();
            let name = name
                .strip_suffix(".ora")
                .map_or(name.clone(), |s| format!("{s}.mnc"));
            // ONE filter, not `save_comic_dialog`'s pair: rfd takes the default
            // extension from the FIRST filter, and offering OpenRaster here let
            // the user save the single-file comic under a name the format does
            // not match (parked owner nit, plan 30-LOW).
            rfd::FileDialog::new()
                .set_title("Export Single File")
                .add_filter("MangaNakama Comic", &["mnc"])
                .set_file_name(&name)
                .save_file()
                .map(AppCmd::ExportMncPath)
        }
        AppCmd::SaveDuplicate => {
            // IO-003: the COPY takes the work's own shape, so the picker is
            // the same one Save As opens — a comic asks for a folder (the
            // native work folder), a single page for a file. Named from the
            // current path where there is one, so "chapter.mnc" suggests
            // "chapter copy.mnc" rather than the story title again.
            let stem = current()
                .and_then(|p| {
                    if p.file_name()
                        .is_some_and(|n| n.eq_ignore_ascii_case("work.mnc"))
                    {
                        p.parent()?.file_name()?.to_str().map(str::to_owned)
                    } else {
                        p.file_stem()?.to_str().map(str::to_owned)
                    }
                })
                .unwrap_or_else(|| default_save_name().replace(".ora", "").replace(".mnc", ""));
            if is_comic() {
                rfd::FileDialog::new()
                    .set_title(&format!("Save Duplicate of \"{stem}\" — pick an empty folder"))
                    .pick_folder()
                    .map(|d| AppCmd::SaveDuplicatePath(d.join(mn_core::project::WORKFOLDER_INDEX)))
            } else {
                save_comic_dialog("Save Duplicate", &format!("{stem} copy.ora"))
                    .map(AppCmd::SaveDuplicatePath)
            }
        }
        // Workflow audit finding 8. Two steps, in this order for one reason:
        // the composite needs `&mut App` (renderer + document) and `PrintDlgW`
        // must NOT have one alive — it runs a modal loop that re-enters the
        // wndproc. So the pixels are made inside `with_app`, the dialog and
        // the spool run outside it, and the verdict goes back as a command.
        AppCmd::PrintGo => {
            let job = with_app(hwnd, |a| a.print_job())?;
            let (msg, warn) = match job {
                Err(e) => (e, true),
                Ok(job) => match app::print::run(hwnd, &job) {
                    Ok(m) => (m, false),
                    Err(e) => (e, true),
                },
            };
            Some(AppCmd::PrintResult { msg, warn })
        }
        AppCmd::ExportAllPagesGo => rfd::FileDialog::new()
            .set_title("Export All Pages (numbered PNGs)")
            .pick_folder()
            .map(AppCmd::ExportAllPagesPath),
        AppCmd::ExportText => {
            let name = with_app(hwnd, |a| {
                format!("{}-text.txt", crate::cmd::default_export_stem(a))
            })
            .unwrap_or_else(|| "page-text.txt".to_owned());
            save_dialog("Export Text (script)", "txt", &name).map(AppCmd::ExportTextPath)
        }
        AppCmd::CompExportAll => rfd::FileDialog::new()
            .set_title("Export One Image Set Per Layer Comp")
            .pick_folder()
            .map(AppCmd::CompExportAllPath),
        AppCmd::BatchExportPngs => rfd::FileDialog::new()
            .set_title("Export layer PNGs into…")
            .pick_folder()
            .map(AppCmd::BatchExportPngsPath),
        AppCmd::ExportPsd => {
            let name = current()
                .and_then(|p| {
                    p.file_stem()
                        .map(|s| format!("{}.psd", s.to_string_lossy()))
                })
                .unwrap_or_else(|| "untitled.psd".to_owned());
            save_dialog("Export layered PSD", "psd", &name).map(AppCmd::ExportPsdPath)
        }
        AppCmd::ExportPng => {
            let name = current()
                .and_then(|p| {
                    p.file_stem()
                        .map(|s| format!("{}.png", s.to_string_lossy()))
                })
                .unwrap_or_else(|| "untitled.png".to_owned());
            save_dialog("Export PNG", "png", &name).map(AppCmd::ExportPngPath)
        }
        AppCmd::ImportImage => rfd::FileDialog::new()
            .set_title("Import Image as Layer")
            .add_filter(
                "Images",
                &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp", "gif"],
            )
            .pick_file()
            .map(AppCmd::ImportImagePath),
        AppCmd::ImportImageDraft => rfd::FileDialog::new()
            .set_title("Import Image as Draft Layer")
            .add_filter(
                "Images",
                &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp", "gif"],
            )
            .pick_file()
            .map(AppCmd::ImportImageDraftPath),
        AppCmd::ImportAbr => rfd::FileDialog::new()
            .set_title("Import Brushes")
            .add_filter(
                "Brushes (Photoshop, GIMP, Krita, Clip Studio)",
                &["abr", "gbr", "gih", "kpp", "sut", "todb"],
            )
            .pick_file()
            .map(AppCmd::ImportAbrPath),
        AppCmd::ImportPage => rfd::FileDialog::new()
            .set_title("Import Page")
            .add_filter(
                "OpenRaster / Images",
                &["ora", "png", "jpg", "jpeg", "bmp", "tif", "tiff"],
            )
            .pick_file()
            .map(AppCmd::ImportPagePath),
        // Images only, unlike Import Page: a batch underlay is placed INTO
        // pages that already exist, and an .ora is a page, not a photo.
        AppCmd::BatchImportPages => rfd::FileDialog::new()
            .set_title("Batch Import Pages — pick every rough")
            .add_filter("Images", &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp"])
            .pick_files()
            .map(AppCmd::BatchImportPagesPicked),
        // Workflow audit §11: the source is a WORK, either flavour of
        // `.mnc` — a name work is pages, not an image.
        AppCmd::StampNamePages => rfd::FileDialog::new()
            .set_title("Stamp Name Pages — pick the ネーム work")
            .add_filter("MangaNakama work", &["mnc"])
            .pick_file()
            .map(AppCmd::StampNamePagesPath),
        AppCmd::ImportPalette => rfd::FileDialog::new()
            .set_title("Import Palette (.gpl)")
            .add_filter("GIMP/Krita palette", &["gpl"])
            .pick_file()
            .map(AppCmd::ImportPalettePath),
        AppCmd::ImportGradient => rfd::FileDialog::new()
            .set_title("Import Gradient (.ggr)")
            .add_filter("GIMP gradient", &["ggr"])
            .pick_file()
            .map(AppCmd::ImportGradientPath),
        AppCmd::ReplacePage => rfd::FileDialog::new()
            .set_title("Replace Current Page")
            .add_filter(
                "OpenRaster / Images",
                &["ora", "png", "jpg", "jpeg", "bmp", "tif", "tiff"],
            )
            .pick_file()
            .map(AppCmd::ReplacePagePath),
        // Row 166 file objects. Same picker as Import Image as Layer — a
        // file object IS an imported image, with a link kept.
        AppCmd::ImportFileObject => rfd::FileDialog::new()
            .set_title("Import Image as File Object")
            .add_filter(
                "Images",
                &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp", "gif"],
            )
            .pick_file()
            .map(AppCmd::ImportFileObjectPath),
        // `FO-009`. The row is offered on any layer (the palette's ≡ menu
        // and the command palette both aim at one), so the "is that a file
        // object?" question is answered HERE — before a dialog opens — and
        // the picker starts in the folder the dead link pointed at.
        AppCmd::RelinkFileObject(which) => {
            let Some(li) = with_app(hwnd, |a| a.relink_target(which)).flatten() else {
                with_app(hwnd, |a| {
                    a.set_status("Relink file object: select a file object layer first")
                });
                return None;
            };
            let mut d = rfd::FileDialog::new()
                .set_title("Relink File Object")
                .add_filter(
                    "Images",
                    &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp", "gif"],
                );
            if let Some(dir) = with_app(hwnd, |a| a.file_object_dir(li)).flatten() {
                d = d.set_directory(dir);
            }
            d.pick_file()
                .map(|p| AppCmd::RelinkFileObjectPath(li, p))
        }
        other => Some(other),
    }
}

fn save_dialog(title: &str, ext: &str, name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(title)
        .add_filter(ext, &[ext])
        .set_file_name(name)
        .save_file()
}

/// Save dialog offering both project formats; the suggested name's extension
/// picks the default.
fn save_comic_dialog(title: &str, name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(title)
        .add_filter("MangaNakama Comic", &["mnc"])
        .add_filter("OpenRaster (single page)", &["ora"])
        .set_file_name(name)
        .save_file()
}

/// egui asked to be repainted after `d`. `ZERO` = now (an animation is
/// running), `MAX` = never (idle) — anything else becomes one timer.
fn schedule_repaint(hwnd: HWND, d: Duration) {
    unsafe { KillTimer(hwnd, REPAINT_TIMER) };
    if d.is_zero() {
        unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
    } else if d < Duration::from_secs(1) {
        let ms = (d.as_millis() as u32).max(1);
        unsafe { SetTimer(hwnd, REPAINT_TIMER, ms, None) };
    }
}

fn apply_cursor(app: &App) {
    let (x, y) = app.last_pointer;
    let icon = if app.shell.owns_pointer(x, y) {
        app.shell.cursor
    } else {
        app.canvas_cursor()
    };
    let id = match icon {
        egui::CursorIcon::Default | egui::CursorIcon::None => IDC_ARROW,
        egui::CursorIcon::PointingHand | egui::CursorIcon::Grab | egui::CursorIcon::Grabbing => {
            IDC_HAND
        }
        egui::CursorIcon::Text | egui::CursorIcon::VerticalText => IDC_IBEAM,
        egui::CursorIcon::Crosshair | egui::CursorIcon::Cell => IDC_CROSS,
        egui::CursorIcon::Wait | egui::CursorIcon::Progress => IDC_WAIT,
        egui::CursorIcon::NotAllowed | egui::CursorIcon::NoDrop => IDC_NO,
        egui::CursorIcon::ResizeHorizontal
        | egui::CursorIcon::ResizeEast
        | egui::CursorIcon::ResizeWest
        | egui::CursorIcon::ResizeColumn => IDC_SIZEWE,
        egui::CursorIcon::ResizeVertical
        | egui::CursorIcon::ResizeNorth
        | egui::CursorIcon::ResizeSouth
        | egui::CursorIcon::ResizeRow => IDC_SIZENS,
        egui::CursorIcon::ResizeNwSe
        | egui::CursorIcon::ResizeNorthWest
        | egui::CursorIcon::ResizeSouthEast => IDC_SIZENWSE,
        egui::CursorIcon::ResizeNeSw
        | egui::CursorIcon::ResizeNorthEast
        | egui::CursorIcon::ResizeSouthWest => IDC_SIZENESW,
        egui::CursorIcon::Move | egui::CursorIcon::AllScroll => IDC_SIZEALL,
        _ => IDC_ARROW,
    };
    unsafe { SetCursor(LoadCursorW(std::ptr::null_mut(), id)) };
}

/// The WM_KEYDOWN half of the app-shortcut path: the `shortcut` table,
/// plus the spring-load arm around it (workflow-audit #6 — CSP's hold-a-
/// tool-key-to-borrow habit). Arming is detected by what the press QUEUED
/// rather than by hooking every SetTool arm: the table has a dozen of
/// them, and one chokepoint cannot drift. A press that queued a tool
/// switch — or flipped the Move tool's pan/rotate sub mode, which H/R do
/// as a side effect without changing `app.tool` — arms the spring;
/// `App::spring_release` decides on the keyup whether it was a borrow
/// (restore) or a tap (today's latch, exactly).
fn key_down(app: &mut App, vk: u16, repeat: bool) {
    let queued = app.cmds.len();
    let (tool0, pan0) = (app.tool, app.pan_mode);
    shortcut(app, vk, repeat);
    if repeat || app.spring.is_some() || app.cmds.len() <= queued {
        return;
    }
    // The chokepoint, in the two shapes a tool press can queue. `SetSubTool`
    // joined it with the targeting model (owner ask 2026-08-25): its state
    // half runs at DISPATCH, so unlike the old H/R arms there is nothing
    // changed yet to compare against — what arms the spring is whether the
    // queued row is the one the tool is already on.
    let borrowed = match app.cmds.back() {
        Some(AppCmd::SetTool(t)) => {
            let t = *t;
            (t != tool0 || app.pan_mode != pan0).then_some(t)
        }
        Some(AppCmd::SetSubTool(s)) => {
            let s = *s;
            (s.tool() != tool0 || !crate::subtools::is_current(app, s)).then_some(s.tool())
        }
        _ => None,
    };
    if let Some(borrowed) = borrowed {
        app.spring = Some(crate::app::SpringLoad {
            vk,
            saved: tool0,
            saved_pan: pan0,
            borrowed,
            at: std::time::Instant::now(),
            pointer_seen: false,
        });
    }
}

/// The built-in TOOL keys, as TARGETS (owner ask 2026-08-25). One row per
/// key, in bound order: two or more targets on a key cycle on repeat press,
/// which is how the hand-written `T` (Text ⇄ Balloon) and `E` (Eraser ⇄ Pen)
/// flips are expressed now — `crate::subtools::press` runs them, and the
/// migration is pinned by `subtools::tests`.
///
/// These are ours, not CSP's, and the owner said so explicitly: he is fine
/// with a different default set, the MODEL is the ask. Where his set differs
/// is recorded in `docs/manual/keys.html`.
///
/// When `Tool::Ruler` exists (scope call still with the owner — it is a tool,
/// not a keymap entry), it joins `U` as a third target after Frame border,
/// which is his CSP order: Figure → Frame Border → Ruler on one key.
fn builtin_targets(vk: u16, shift: bool) -> Option<&'static [Target]> {
    use crate::cmd::{PanMode, SubTool};
    use crate::subtools::{SubToolPath, group};
    // Named because a `&[…]` built inside the match arm is a temporary; a
    // const item is the 'static the caller holds.
    const HAND: &[Target] = &[Target::SubTool(SubToolPath {
        tool: Tool::Pan,
        group: group::MOVE,
        sub: SubTool::Pan(PanMode::Hand),
    })];
    const ROTATE: &[Target] = &[Target::SubTool(SubToolPath {
        tool: Tool::Pan,
        group: group::MOVE,
        sub: SubTool::Pan(PanMode::Rotate),
    })];
    Some(match vk {
        // P / B — B stays a pen alias (old MangaNakama habit).
        0x50 | 0x42 => &[Target::Tool(Tool::Pen)],
        0x47 => &[Target::Tool(Tool::Fill)],   // G
        0x4D => &[Target::Tool(Tool::Select)], // M
        0x57 => &[Target::Tool(Tool::Wand)],   // W (CSP auto select)
        0x4F => &[Target::Tool(Tool::Object)], // O (the cycle arm runs first)
        // F = Figure, V = Gradient (my picks — no CSP defaults in his set).
        0x46 => &[Target::Tool(Tool::Figure)],
        0x56 => &[Target::Tool(Tool::Gradient)],
        0x55 => &[Target::Tool(Tool::Frame)], // U (CSP: frame border)
        // CSP puts Text and Balloon both on T, and duplicates cycle.
        0x54 => &[Target::Tool(Tool::Text), Target::Tool(Tool::Balloon)],
        0x49 => &[Target::Tool(Tool::Eyedrop)], // I
        // H / R: the Move tool's two sub tools, named as sub tools now
        // rather than reached by poking `pan_mode` on the way past.
        0x48 => HAND,
        0x52 if !shift => ROTATE,
        0x45 => &[Target::Tool(Tool::Eraser), Target::Tool(Tool::Pen)], // E
        _ => return None,
    })
}

/// Keyboard shortcuts that belong to the app, not to egui. Returns true when
/// the key was consumed.
///
/// The table is the owner's own CSP shortcut set, mined from his install's
/// `Shortcut/default.khc` (2026-08-14): tool keys are CSP's (P pen, E eraser,
/// G fill, W wand, M select, O object, U frame, T text, I eyedropper, H hand),
/// menu keys match the dump (Ctrl+A/D, Ctrl+Shift+D/I, Alt+Del fill,
/// Shift+Del clear-outside, Ctrl+Shift+E stamp, Alt+[/] layer walk, `,`/`.`
/// sub tool step, Ctrl+9 / Ctrl+Shift+9 flip view H / V).
fn shortcut(app: &mut App, vk: u16, repeat: bool) -> bool {
    // The reader OWNS the keyboard while open (owner top item
    // 2026-08-18): Esc exits, F11 fullscreen, arrows / PgUp / PgDn turn
    // (direction-aware), Home / End jump. Runs BEFORE the global table —
    // PgUp/PgDn are the owner's ZOOM keys otherwise.
    if app.reader.open {
        match vk {
            0x1B if !repeat => {
                app.reader_close();
                return true;
            }
            0x7A if !repeat => {
                app.reader_toggle_fullscreen();
                return true;
            }
            // Reader v2: F flags the current spread (the proofreading
            // loop's "this hand is wrong" marker).
            0x46 if !repeat => {
                app.reader_toggle_flag_here();
                return true;
            }
            // Reader v2.1: 1 toggles 1:1 zoom — the tone-moiré check
            // (one canvas px per screen px, drag pans).
            0x31 if !repeat => {
                app.reader_toggle_zoom();
                return true;
            }
            0x25 | 0x21 if !repeat => {
                // Left / PageUp: the reading-forward side (left in RTL).
                let d = app.reader_left_delta();
                app.reader_turn(d);
                return true;
            }
            0x27 | 0x22 if !repeat => {
                let d = app.reader_left_delta();
                app.reader_turn(-d);
                return true;
            }
            0x24 if !repeat => {
                // Home: first screen (the clamp does the jump).
                app.reader_turn(i32::MIN / 2);
                return true;
            }
            0x23 if !repeat => {
                app.reader_turn(i32::MAX / 2);
                return true;
            }
            _ => {}
        }
    }
    let m = app.shell.sync_modifiers();
    let (ctrl, shift, alt) = (m.ctrl, m.shift, m.alt);

    // keys.json (workflow-audit #5): the user's own chords, consulted
    // BEFORE the built-in table so a rebind can shadow a default. Exact
    // modifier match — a bare-key binding does not fire shifted. Repeat
    // gating mirrors the table below: only the walk/undo family repeats.
    if let Some(b) = app.keymap.lookup(ctrl, shift, alt, vk) {
        match b.clone() {
            crate::keymap::Bind::Cmd(c) => {
                if !repeat
                    || matches!(
                        c,
                        AppCmd::Undo | AppCmd::Redo | AppCmd::LayerAbove | AppCmd::LayerBelow
                    )
                {
                    app.push_cmd(c);
                }
            }
            // A bound target list is a tool key like any other: it cycles on
            // repeat PRESS, never on auto-repeat.
            crate::keymap::Bind::Targets(t) => {
                if !repeat {
                    crate::subtools::press(app, &t);
                }
            }
        }
        return true;
    }

    let cmd = match (ctrl, vk) {
        (true, 0x5A) if shift => Some(AppCmd::Redo), // Ctrl+Shift+Z
        (true, 0x5A) => Some(AppCmd::Undo),          // Ctrl+Z
        (true, 0x59) => Some(AppCmd::Redo),          // Ctrl+Y
        // Save As, all three CSP chords: Ctrl+Shift+S / Ctrl+Alt+S / Shift+Alt+S.
        (true, 0x53) if shift || alt => Some(AppCmd::SaveOraAs),
        (false, 0x53) if shift && alt => Some(AppCmd::SaveOraAs),
        (true, 0x53) => Some(AppCmd::SaveOra), // Ctrl+S
        (true, 0x4E) if shift => Some(AppCmd::AddLayer), // Ctrl+Shift+N
        (true, 0x4E) => Some(AppCmd::NewDoc),  // Ctrl+N
        (true, 0x4F) => Some(AppCmd::OpenOra), // Ctrl+O
        (true, 0x30) if alt => Some(AppCmd::Zoom100), // Ctrl+Alt+0 (CSP pixel size)
        (true, 0x30) => Some(AppCmd::ZoomFit), // Ctrl+0
        (true, 0x31) => Some(AppCmd::Zoom100), // Ctrl+1
        (true, 0x39) if shift => Some(AppCmd::FlipViewV), // Ctrl+Shift+9
        (true, 0x39) => Some(AppCmd::FlipView), // Ctrl+9 (owner's viewreversehorz)
        (true, 0x41) => Some(AppCmd::SelectAll), // Ctrl+A
        (true, 0x44) if shift => Some(AppCmd::Reselect), // Ctrl+Shift+D
        (true, 0x44) => Some(AppCmd::Deselect), // Ctrl+D
        (true, 0x49) if shift => Some(AppCmd::SelectInvert), // Ctrl+Shift+I
        (true, 0x54) => Some(AppCmd::TransformStart), // Ctrl+T
        (true, 0x43) => Some(AppCmd::Copy),    // Ctrl+C
        (true, 0x58) => Some(AppCmd::Cut),     // Ctrl+X
        (true, 0x56) if shift => Some(AppCmd::PasteInPlace), // Ctrl+Shift+V
        (true, 0x56) => Some(AppCmd::Paste),   // Ctrl+V (into the panel)
        (true, 0x45) if shift => Some(AppCmd::StampVisible), // Ctrl+Shift+E
        (true, 0x45) => Some(AppCmd::MergeDown), // Ctrl+E
        (true, 0x47) => Some(AppCmd::AddFolder), // Ctrl+G (folder & insert)
        // TOOL keys live in `builtin_targets` below, not here — a shortcut
        // aims at a tool, a sub tool GROUP or an exact sub tool now, and
        // repeat-press cycles when a key carries more than one. This arm is
        // the ONE tool key that is not a targeting question: in the Object
        // tool with something picked, O cycles the stacked objects UNDER the
        // cursor (owner item 2026-08-19) — a different feature that happens
        // to share a letter. With nothing picked it falls through to the
        // table and means the tool.
        (false, 0x4F)
            if app.tool == Tool::Object
                && (app.object_sel.is_some()
                    || app.text_sel.is_some()
                    || app.balloon_sel.is_some()
                    || app.gen_sel.is_some()) =>
        {
            Some(AppCmd::ObjectCycle(!shift))
        }
        // Del family: Alt fills, Shift clears outside, plain deletes/clears.
        (false, 0x2E) if alt => Some(AppCmd::FillSelection),
        (false, 0x2E) if shift => Some(AppCmd::ClearOutside),
        (false, 0x2E) if app.tool == Tool::Object && !app.object_multi.is_empty() => {
            Some(AppCmd::ObjectMultiDelete)
        }
        (false, 0x2E) if app.tool == Tool::Object => app
            .text_sel
            .map(|(layer, text)| AppCmd::TextDelete { layer, text })
            .or_else(|| {
                app.balloon_sel
                    .map(|(layer, balloon)| AppCmd::BalloonDelete { layer, balloon })
            })
            .or_else(|| {
                app.object_sel
                    .map(|(layer, frame)| AppCmd::FrameDelete { layer, frame })
            })
            .or_else(|| app.vector_sel.map(|stroke| AppCmd::VectorDelete { stroke }))
            .or(Some(AppCmd::ClearLayer)),
        (false, 0x2E) => Some(AppCmd::ClearLayer),
        // C: the transparent colour slot — erase with the current brush.
        (false, 0x43) => Some(AppCmd::SetSlot(if app.slot == Slot::Transparent {
            Slot::Main
        } else {
            Slot::Transparent
        })),
        (false, 0x58) => Some(AppCmd::SwapColors),  // X
        (false, 0x77) => Some(AppCmd::ResetColors), // F8 (main black / sub white)
        // View rotate steps, the owner's bindings: `-` left, F9 right. The
        // step is the `rotate_step_deg` preference (15° shipped).
        (false, 0xBD) => Some(AppCmd::RotateView(-app.prefs.rotate_step_deg.to_radians())),
        (false, 0x78) => Some(AppCmd::RotateView(app.prefs.rotate_step_deg.to_radians())),
        (false, 0x21) => Some(AppCmd::ZoomStep(1.25)), // PageUp (CSP zoom in)
        (false, 0x22) => Some(AppCmd::ZoomStep(1.0 / 1.25)), // PageDown
        // Page navigation (PM-021): Ctrl-modified, so the bare zoom keys
        // keep the owner's CSP bindings. Ctrl+Home/End = first/last.
        (true, 0x21) => Some(AppCmd::PagePrev), // Ctrl+PageUp
        (true, 0x22) => Some(AppCmd::PageNext), // Ctrl+PageDown
        (true, 0x24) => Some(AppCmd::PageFirst), // Ctrl+Home
        (true, 0x23) => Some(AppCmd::PageLast), // Ctrl+End
        // Alt+] / Alt+[ — walk the active layer up/down the stack.
        (false, 0xDD) if alt => Some(AppCmd::LayerAbove),
        (false, 0xDB) if alt => Some(AppCmd::LayerBelow),
        _ => None,
    };
    if let Some(c) = cmd {
        if !repeat
            || matches!(
                c,
                AppCmd::Undo | AppCmd::Redo | AppCmd::LayerAbove | AppCmd::LayerBelow
            )
        {
            app.push_cmd(c);
        }
        return true;
    }

    // The tool keys, as targets. Ctrl-free like the arms they replaced, and
    // AFTER them so the Object cycle and the Ctrl chords keep their letters.
    // A held key repeats; a repeat must not walk a cycle on, so only the
    // first press aims.
    if !ctrl && let Some(targets) = builtin_targets(vk, shift) {
        if !repeat {
            crate::subtools::press(app, targets);
        }
        return true;
    }

    match (ctrl, vk) {
        // Ctrl+[ / Ctrl+] — brush opacity down/up (CSP toolopacity±).
        (true, 0xDB) => {
            let v = (app.props_current.opacity - 0.05).max(0.0);
            app.push_cmd(AppCmd::SetOpacity(v));
            true
        }
        (true, 0xDD) => {
            let v = (app.props_current.opacity + 0.05).min(1.0);
            app.push_cmd(AppCmd::SetOpacity(v));
            true
        }
        (false, 0xDB) => {
            app.step_brush_size(false); // [ — CSP-style gradiated px ladder
            true
        }
        (false, 0xDD) => {
            app.step_brush_size(true); // ]
            true
        }
        // , / . — previous/next sub tool (CSP subtoolprevious/next).
        (false, 0xBC) => {
            app.step_subtool(false);
            true
        }
        (false, 0xBE) => {
            app.step_subtool(true);
            true
        }
        // Ctrl+K — the command palette (type-to-run, brushes included).
        // Free in the owner's CSP set, and the editor chord everyone
        // already knows. It only OPENS here: once the overlay's field has
        // focus the shell reports `wants_keyboard` and this table stands
        // down, so Esc (or a run) is the way back out.
        // Through a command, not a direct `ui::` call: that is what lets
        // `keys.json` move Ctrl+K itself (follow-up (b), 2026-08-29).
        (true, 0x4B) if !repeat => {
            app.push_cmd(AppCmd::CommandPalette);
            true
        }
        (true, 0x57) => {
            // Ctrl+W — close (the prompt runs in pump_commands).
            app.close_requested = true;
            true
        }
        // Enter / Esc close or cancel an in-progress polyline frame.
        (false, 0x0D) if app.frame_poly.is_some() => {
            app.finish_frame_poly();
            true
        }
        (false, 0x1B) if app.frame_poly.is_some() => {
            app.cancel_frame_poly();
            true
        }
        // Figure ▸ Polygon: Enter closes, Esc cancels.
        (false, 0x0D) if app.figure_poly.is_some() => {
            app.finish_figure_poly();
            true
        }
        (false, 0x1B) if app.figure_poly.is_some() => {
            app.figure_poly = None;
            app.set_status("polygon cancelled");
            app.mark_dirty();
            true
        }
        // Row 157 / FG-012: Backspace walks a multi-point figure back one
        // point at a time instead of throwing the whole thing away. Safe to
        // sit in this table — the arm above `key_down`'s caller stands down
        // entirely while canvas text or an egui field has the keyboard, so
        // this can never eat a character.
        (false, 0x08) if app.figure_poly.is_some() => {
            app.figure_undo_point();
            true
        }
        // Row 157 / FG-002 + FG-011: a figure waiting on its second stage.
        // BEFORE the plain figure-drag Esc arm, and before the transform
        // arms, for the usual reason — the more specific gesture wins.
        (false, 0x0D) if app.figure_stage2.is_some() => {
            app.finish_figure_stage2();
            true
        }
        (false, 0x1B) if app.figure_stage2.is_some() => {
            app.cancel_figure_stage2();
            true
        }
        // FI-050/FI-051: a freeform gradient mid-gesture. BEFORE the plain
        // drag arm, same as the figure's second stage — the more specific
        // gesture wins, and this one is the only gradient that can be
        // mid-gesture with no button held. Enter paints what is drawn,
        // Backspace takes the last line back, Esc throws it all away.
        (false, 0x0D) if app.grad_free.is_some() => {
            app.commit_grad_free();
            true
        }
        (false, 0x08) if app.grad_free.is_some() => {
            app.grad_free_undo_guide();
            true
        }
        (false, 0x1B) if app.grad_free.is_some() => {
            app.cancel_grad_free();
            true
        }
        // Esc cancels an in-progress figure/gradient drag.
        (false, 0x1B) if app.figure_drag.is_some() || app.grad_drag.is_some() => {
            app.figure_drag = None;
            app.grad_drag = None;
            app.mark_dirty();
            true
        }
        // Rulers part 2: Enter finishes an in-progress curve ruler
        // (before the transform arm — a curve in progress is more
        // specific than any float).
        (false, 0x0D) if app.curve_pending.is_some() => {
            app.finish_curve_ruler();
            true
        }
        // Enter commits a pending transform, Esc cancels.
        (false, 0x0D) if app.transform_drag.is_some() => {
            app.push_cmd(AppCmd::TransformCommit);
            true
        }
        (false, 0x1B) if app.transform_drag.is_some() => {
            app.push_cmd(AppCmd::TransformCancel);
            true
        }
        // L-001/L-002 magnetic lasso: Enter closes the outline, Esc throws
        // the trace away, Backspace walks the anchors back one at a time
        // (and at the first anchor cancels, so the key is never dead).
        (false, 0x0D) if app.magnetic.is_some() => {
            app.magnetic_close();
            true
        }
        (false, 0x1B) if app.magnetic.is_some() => {
            app.magnetic_cancel();
            true
        }
        (false, 0x08) if app.magnetic.is_some() => {
            app.magnetic_undo_anchor();
            true
        }
        (false, 0x70) => {
            app.hud_open = !app.hud_open; // F1
            app.mark_dirty();
            true
        }
        (false, 0x09) if shift => {
            // Shift+Tab (UI-032): the chrome as well — top bar and status
            // bar. Not persisted, deliberately (see `App::chrome_hidden`):
            // the top bar is this window's title bar, so a hide that
            // survived a restart could strand the owner with no – □ ×.
            app.chrome_hidden = !app.chrome_hidden;
            app.mark_dirty();
            true
        }
        (false, 0x09) => {
            // Tab: hide every palette for a clean full-canvas view (CSP).
            app.panels_hidden = !app.panels_hidden;
            app.mark_dirty();
            true
        }
        // The way back, whatever put the UI away: Esc restores both hides.
        // LAST of the Esc arms, so a figure/polyline/transform in progress
        // still cancels first — this only fires when Esc had nothing else
        // to do. It exists because hiding the chrome hides the close button
        // with it, and "press the same key again" is a poor only answer
        // when the user has forgotten which key it was.
        (false, 0x1B) if app.panels_hidden || app.chrome_hidden => {
            app.panels_hidden = false;
            app.chrome_hidden = false;
            app.set_status("palettes and chrome back");
            app.mark_dirty();
            true
        }
        _ => false,
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Custom chrome first: these two arrive before the App exists (during
    // CreateWindowExW) and never touch it.
    if msg == WM_NCCALCSIZE && wparam != 0 {
        return unsafe { nc_calc_size(hwnd, lparam) };
    }
    if msg == WM_NCHITTEST {
        return unsafe { nc_hit_test(hwnd, lparam) };
    }
    if msg == WM_GETMINMAXINFO {
        // Floor the resize (owner report 2026-08-20): below ~660 logical px
        // the menu row cannot hold the menus plus the − □ × caption cluster
        // — the bar wrapped onto a second line, the title painted over the
        // menus, and the window buttons fell off the end of the row. The
        // bar also degrades gracefully now (ui/top.rs hides the command
        // clusters and the title as space runs out), but a window too small
        // to show its own close button is never a size worth allowing.
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let mmi = lparam as *mut MINMAXINFO;
        unsafe {
            (*mmi).ptMinTrackSize.x = (660 * dpi as i32) / 96;
            (*mmi).ptMinTrackSize.y = (420 * dpi as i32) / 96;
        }
        return 0;
    }

    // Messages arriving before/after the App exists (WM_CREATE, post-destroy).
    let p = unsafe { app_ptr(hwnd) };
    if p.is_null() {
        return match msg {
            WM_TABLET_QUERYSYSTEMGESTURESTATUS => TABLET_INK_FLAGS as LRESULT,
            WM_MOVE | WM_SIZE | WM_EXITSIZEMOVE => {
                note_geom_now(hwnd);
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        };
    }
    let app: &mut App = unsafe { &mut *p };

    match msg {
        // Inking must never wait on press-and-hold, flicks or palm-rejection UI.
        WM_TABLET_QUERYSYSTEMGESTURESTATUS => TABLET_INK_FLAGS as LRESULT,

        WM_ERASEBKGND => 1, // wgpu paints everything; do not let GDI flicker.

        // A file drop (IO-041). All this arm does is turn the shell's HDROP
        // into paths — what a drop MEANS lives in `drop::plan`, so it is
        // testable without a window. The commands queue and run in
        // `pump_commands`, which matters: opening a project pumps dialogs.
        WM_DROPFILES => {
            let paths = unsafe { win32::dropped_paths(wparam) };
            let (cmds, note) = drop::plan(&paths);
            if let Some(note) = note {
                app.set_status(note);
            }
            for c in cmds {
                app.push_cmd(c);
            }
            app.mark_dirty();
            schedule_repaint(hwnd, Duration::ZERO);
            0
        }

        WM_PAINT => {
            unsafe { ValidateRect(hwnd, std::ptr::null()) };
            let out = app.render();
            schedule_repaint(hwnd, out.repaint_after);
            0
        }

        // Tier 3 automation (remote.rs): parked remote requests. Served
        // HERE — UI thread, `&mut App` in hand like every other arm — so a
        // remote edit goes through the same dispatch doors as a click.
        // Wire methods never open dialogs, so the pump_commands dialog
        // dance does not apply.
        m if m == remote::MSG => {
            for p in remote::take_pending() {
                let resp = remote::respond(app, &p.req);
                let _ = p.reply.send(resp);
            }
            flush_redraw(hwnd, app);
            0
        }

        WM_TIMER => {
            if wparam == REPAINT_TIMER {
                unsafe { KillTimer(hwnd, REPAINT_TIMER) };
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            } else if wparam == AUTOSAVE_TIMER {
                // Repeating: never killed. Executed by pump_commands.
                app.push_cmd(AppCmd::Autosave);
            } else if wparam == MEASURE_TIMER && measurement_poll(app) {
                unsafe { KillTimer(hwnd, MEASURE_TIMER) };
            }
            0
        }

        // Close goes through pump_commands: the unsaved-changes prompt is a
        // modal dialog, which must not run while this `&mut App` is alive.
        WM_CLOSE => {
            app.close_requested = true;
            0
        }

        WM_SETCURSOR => {
            if loword(lparam as usize) as u32 == HTCLIENT {
                apply_cursor(app);
                1
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }

        WM_SIZE => {
            let (w, h) = client_size(hwnd);
            app.win_maximized = unsafe { IsZoomed(hwnd) } != 0;
            note_geom_now(hwnd);
            app.resize(w, h);
            flush_redraw(hwnd, app);
            0
        }

        // The system's modal move/resize loop is over: persist placement now
        // (during the drag only the static tracks it — no file churn).
        WM_EXITSIZEMOVE => {
            note_geom_now(hwnd);
            persist_geom(app);
            0
        }

        WM_MOVE => {
            note_geom_now(hwnd);
            0
        }

        WM_SETFOCUS | WM_KILLFOCUS => {
            // Focus left mid-contact: the lifts will be delivered to whoever
            // has focus now, so drop the half-seen gesture rather than let it
            // resolve against the next tap's fingers.
            TAPS.lock().unwrap_or_else(|e| e.into_inner()).cancel();
            if msg == WM_KILLFOCUS {
                // …and for the same reason, every OTHER latch. The key-up
                // for a held space bar goes to the window that has focus
                // now, so without this the pen pans forever once you come
                // back (docs/CSP-PEN-TABLET-PAINS.md §4.6).
                app.cancel_input_latches("focus left the window");
            }
            app.shell.on_focus(msg == WM_SETFOCUS);
            // Row 166 `FO-008`, the automatic half: coming back to the front
            // is the moment "I redrew the background in the other app"
            // becomes true. Silent when nothing changed, and it stats
            // nothing at all on a page with no file objects.
            if msg == WM_SETFOCUS {
                app.refresh_file_objects_quiet();
            }
            app.mark_dirty();
            flush_redraw(hwnd, app);
            0
        }

        // Capture went elsewhere mid-gesture: an installer stealing the
        // foreground, a driver's own popup, another app calling SetCapture.
        // The pen-up/button-up that would have closed the stroke will be
        // delivered to that window, so close it here (§4.4 — the orphaned
        // undo bracket that self-heals on the NEXT pen-down, which is
        // precisely why nobody ever reported it accurately).
        //
        // The `WM_CAPTURECHANGED` guard is load-bearing and is the trap in
        // this arm: our own `ReleaseCapture()` in the button-up handlers
        // sends this message SYNCHRONOUSLY, re-entering the wndproc in the
        // middle of a perfectly healthy mouse stroke. Windows sets lParam to
        // the window GAINING capture — NULL on a plain release, and us when
        // we take capture back — so those two cases are exactly the ones to
        // ignore. `WM_POINTERCAPTURECHANGED` needs no such guard: we never
        // call `SetPointerCapture`, so it can only ever come from outside.
        WM_CAPTURECHANGED if lparam != 0 && lparam as HWND != hwnd => {
            app.cancel_input_latches("another window took the pointer");
            flush_redraw(hwnd, app);
            0
        }

        WM_POINTERCAPTURECHANGED => {
            app.cancel_input_latches("the pen was captured elsewhere");
            flush_redraw(hwnd, app);
            0
        }

        WM_DPICHANGED => {
            // lparam is the suggested window rect for the new DPI.
            let rc = lparam as *const RECT;
            if !rc.is_null() {
                let r = unsafe { *rc };
                unsafe {
                    SetWindowPos(
                        hwnd,
                        std::ptr::null_mut(),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                };
            }
            // hiword(wparam) is the new dpi; ask the window to be safe.
            // × ui_scale: the UI-size preference rides the same effective
            // pixels-per-point everywhere (see pump_commands).
            app.dpi_changed(window_ppp(hwnd) * app.prefs.ui_scale);
            flush_redraw(hwnd, app);
            0
        }

        // --- pen ----------------------------------------------------------
        WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP => {
            let id = loword(wparam) as u32;
            let dev = unsafe { pointer_device(id) };
            // Owner-correction probe: count EVERY pointer message by
            // device class at the wndproc entry — delivery, not maths.
            if app.touch_probe.enabled {
                let di = match dev {
                    PointerDevice::Pen => 0,
                    PointerDevice::Touch => 1,
                    PointerDevice::Mouse => 2,
                    PointerDevice::Other => 3,
                };
                let mi = match msg {
                    WM_POINTERDOWN => 0,
                    WM_POINTERUPDATE => 1,
                    _ => 2,
                };
                app.touch_probe.bump(di, mi);
            }
            match dev {
                PointerDevice::Pen => {
                    let report = unsafe { read_pen_batch(hwnd, id) };
                    let batch = &report.samples[..];
                    let pos = batch.last().map(|s| (s.x as i32, s.y as i32));
                    if let Some(p) = pos {
                        app.last_pointer = p;
                        app.pointer_visible = true;
                        // Row 157: the figure tool's second stage steers on
                        // hover, with no button down — so it rides here,
                        // beside `last_pointer`, and not in `canvas_move`
                        // (which only runs while a button is held).
                        app.figure_hover(p.0, p.1);
                    }
                    // BEFORE the dispatch below, so a stylus flipped onto its
                    // tail has already selected the eraser by the time the
                    // pen-down that follows opens a stroke, and so the
                    // stroke's dropped-report baseline is current.
                    app.note_pen_report(&report);
                    match msg {
                        WM_POINTERDOWN => {
                            let (x, y) = pos.unwrap_or(app.last_pointer);
                            if app.shell.owns_pointer(x, y) {
                                // Pen on a panel drives the UI like a mouse.
                                app.pen_owner = Owner::Egui;
                                app.shell.on_pointer_button(
                                    x,
                                    y,
                                    egui::PointerButton::Primary,
                                    true,
                                );
                                app.mark_dirty();
                            } else {
                                app.pen_owner = Owner::Canvas;
                                app.canvas_down(x as f32, y as f32, PointerKind::Pen, batch);
                            }
                        }
                        WM_POINTERUPDATE => match app.pen_owner {
                            Owner::Canvas => {
                                let (x, y) = pos.unwrap_or(app.last_pointer);
                                app.canvas_move(x as f32, y as f32, batch);
                            }
                            // Hovering, or dragging an egui widget: either way
                            // egui needs the position.
                            _ => {
                                if let Some((x, y)) = pos {
                                    app.shell.on_pointer_moved(x, y);
                                    app.mark_dirty();
                                }
                            }
                        },
                        _ => {
                            match app.pen_owner {
                                Owner::Canvas => {
                                    let (x, y) = pos.unwrap_or(app.last_pointer);
                                    app.canvas_up(x as f32, y as f32, batch);
                                }
                                Owner::Egui => {
                                    let (x, y) = pos.unwrap_or(app.last_pointer);
                                    app.shell.on_pointer_button(
                                        x,
                                        y,
                                        egui::PointerButton::Primary,
                                        false,
                                    );
                                    // A pen that leaves the panel should not
                                    // keep it hovered.
                                    app.shell.on_pointer_gone();
                                    app.mark_dirty();
                                }
                                Owner::None => {}
                            }
                            app.pen_owner = Owner::None;
                        }
                    }
                    flush_redraw(hwnd, app);
                    0
                }
                // Touch never draws (palms are harmless) — it navigates:
                // one finger pans, two fingers pinch-zoom. Pointer messages
                // carry SCREEN coordinates in lparam, unlike mouse messages.
                PointerDevice::Touch => {
                    let (sx, sy) = lparam_points(lparam);
                    let mut pt = POINT { x: sx, y: sy };
                    unsafe { ScreenToClient(hwnd, &mut pt) };
                    let (x, y) = (pt.x as f32, pt.y as f32);
                    // The tap recogniser sees the same event first, but only
                    // ever watches — the pan/pinch below runs regardless.
                    // A device with no contact-area support (or a lift whose
                    // info has already gone) falls back to the message time
                    // and an unknown patch size, both of which the machine
                    // reads conservatively.
                    let c = unsafe { read_touch_contact(id) };
                    let t_ms =
                        c.as_ref()
                            .map(|c| c.t_ms)
                            .unwrap_or_else(|| unsafe { GetMessageTime() } as u32 as f64);
                    let size_px = c.map(|c| c.size_px).unwrap_or(0.0);
                    let action = {
                        let mut taps = TAPS.lock().unwrap_or_else(|e| e.into_inner());
                        match msg {
                            WM_POINTERDOWN => {
                                // The shell's ppp IS the effective one
                                // (window DPI × UI scale) — the navigator
                                // rect and tap slop must use the same
                                // space egui laid out in.
                                let ppp = app.shell.ppp;
                                taps.configure(
                                    app.layout.touch_gestures,
                                    ppp,
                                    navigator_rect_px(app, ppp),
                                );
                                taps.down(id, x, y, t_ms, size_px);
                                None
                            }
                            WM_POINTERUPDATE => {
                                taps.moved(id, x, y);
                                None
                            }
                            _ => taps.up(id, t_ms),
                        }
                    };
                    match msg {
                        WM_POINTERDOWN => app.touch_down(id, x, y),
                        WM_POINTERUPDATE => app.touch_move(id, x, y),
                        _ => app.touch_up(id),
                    }
                    match action {
                        Some(gesture::Action::Undo) => app.push_cmd(AppCmd::Undo),
                        Some(gesture::Action::Redo) => app.push_cmd(AppCmd::Redo),
                        // "Reset rotation AND flip" used to be spelled out
                        // here as two commands; CV-035 gave it a name, and
                        // the gesture now runs the same View-menu item.
                        Some(gesture::Action::ResetView) => app.push_cmd(AppCmd::RotateFlipReset),
                        None => {}
                    }
                    flush_redraw(hwnd, app);
                    0
                }
                _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
            }
        }

        // --- mouse fallback -----------------------------------------------
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            if is_pen_promoted_mouse(unsafe { GetMessageExtraInfo() }) {
                return 0;
            }
            let (x, y) = lparam_points(lparam);
            app.last_pointer = (x, y);
            unsafe { SetCapture(hwnd) };
            let button = if msg == WM_LBUTTONDOWN {
                egui::PointerButton::Primary
            } else {
                egui::PointerButton::Secondary
            };
            if app.shell.owns_pointer(x, y) {
                app.mouse_owner = Owner::Egui;
                app.shell.on_pointer_button(x, y, button, true);
                app.mark_dirty();
            } else if msg == WM_LBUTTONDOWN {
                app.mouse_owner = Owner::Canvas;
                app.canvas_down(
                    x as f32,
                    y as f32,
                    PointerKind::Mouse,
                    &[mouse_sample(x, y)],
                );
            } else if app.figure_poly.is_some() {
                // Row 157 / FG-012: right-click over the canvas is CSP's
                // other way to take back the last point. Gated on a figure
                // being in progress so the button stays free otherwise.
                app.figure_undo_point();
            }
            flush_redraw(hwnd, app);
            0
        }

        WM_MOUSEMOVE => {
            if is_pen_promoted_mouse(unsafe { GetMessageExtraInfo() }) {
                return 0;
            }
            let (x, y) = lparam_points(lparam);
            app.last_pointer = (x, y);
            app.pointer_visible = true;
            // Row 157: same as the pen path — the second stage of a figure
            // gesture follows the pointer with nothing pressed.
            app.figure_hover(x, y);
            // A real mouse (pen-promoted messages returned above) means the
            // stylus is out of the picture — the tail-end eraser latch must
            // not survive into it. Same rule as every other latch here.
            app.set_pen_inverted(false);
            // Ask for one WM_MOUSELEAVE so the brush ring can hide when the
            // mouse exits the window (re-armed on every move; cheap).
            unsafe {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut tme);
            }
            match app.mouse_owner {
                Owner::Canvas => {
                    app.canvas_move(x as f32, y as f32, &[mouse_sample(x, y)]);
                }
                _ => {
                    app.shell.on_pointer_moved(x, y);
                    app.mark_dirty();
                }
            }
            flush_redraw(hwnd, app);
            0
        }

        WM_LBUTTONUP | WM_RBUTTONUP => {
            if is_pen_promoted_mouse(unsafe { GetMessageExtraInfo() }) {
                return 0;
            }
            let (x, y) = lparam_points(lparam);
            app.last_pointer = (x, y);
            unsafe { ReleaseCapture() };
            let button = if msg == WM_LBUTTONUP {
                egui::PointerButton::Primary
            } else {
                egui::PointerButton::Secondary
            };
            match app.mouse_owner {
                Owner::Canvas => {
                    app.canvas_up(x as f32, y as f32, &[]);
                }
                _ => {
                    app.shell.on_pointer_button(x, y, button, false);
                    app.mark_dirty();
                }
            }
            app.mouse_owner = Owner::None;
            flush_redraw(hwnd, app);
            0
        }

        WM_MBUTTONDOWN => {
            let (x, y) = lparam_points(lparam);
            unsafe { SetCapture(hwnd) };
            app.begin_pan(x as f32, y as f32);
            0
        }

        WM_MBUTTONUP => {
            unsafe { ReleaseCapture() };
            app.end_pan();
            0
        }

        WM_MOUSEWHEEL => {
            let delta = ((wparam >> 16) & 0xFFFF) as u16 as i16 as f32 / WHEEL_DELTA as f32;
            let keys = loword(wparam) as u32;
            // lparam here is in *screen* coordinates, unlike the other mouse
            // messages — a classic Win32 trap.
            let (sx, sy) = lparam_points(lparam);
            let mut pt = POINT { x: sx, y: sy };
            unsafe { ScreenToClient(hwnd, &mut pt) };
            app.last_pointer = (pt.x, pt.y);

            if app.shell.owns_pointer(pt.x, pt.y) {
                app.shell.on_wheel(0.0, delta);
                app.mark_dirty();
            } else if keys & MK_SHIFT != 0 {
                // Wheel over the canvas: the view is the user's from here
                // (App::render's deferred startup fit stands down).
                app.startup_fit_pending = false;
                app.nudge_pan(delta * 80.0, 0.0);
            } else {
                app.startup_fit_pending = false;
                // Owner directive: the wheel zooms (Ctrl+wheel too, so the
                // old habit keeps working). Shift+wheel pans horizontally.
                // One notch = the `wheel_step` preference (1.15 shipped).
                let step = app.prefs.wheel_step;
                app.zoom_at(pt.x as f32, pt.y as f32, step.powf(delta));
            }
            flush_redraw(hwnd, app);
            0
        }

        // Alt-chords (Alt+Delete fill, Alt+[/] layer walk) arrive as SYS keys.
        // Anything unconsumed falls through so Alt+F4 and menu access work.
        WM_SYSKEYDOWN => {
            let vk = wparam as u16;
            let repeat = (lparam & (1 << 30)) != 0;
            let editing = app.text_editing() && !app.shell.wants_keyboard();
            if !editing && !app.shell.wants_keyboard() && shortcut(app, vk, repeat) {
                app.mark_dirty();
                flush_redraw(hwnd, app);
                0
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }

        WM_KEYDOWN | WM_KEYUP => {
            let vk = wparam as u16;
            let pressed = msg == WM_KEYDOWN;
            let repeat = pressed && (lparam & (1 << 30)) != 0;

            // Canvas text editing swallows the keyboard (space types a space,
            // B stays a letter, arrows move the caret) — egui fields excepted.
            let editing = app.text_editing() && !app.shell.wants_keyboard();
            if vk == VK_SPACE && !editing {
                app.space_down = pressed;
                if !pressed {
                    app.end_pan();
                }
            }
            if editing {
                if pressed {
                    let m = app.shell.sync_modifiers();
                    app.text_key(vk, m.ctrl, m.shift);
                }
                app.mark_dirty();
                flush_redraw(hwnd, app);
                return 0;
            }
            // App shortcuts stand down while egui has a focused text field.
            if pressed && !app.shell.wants_keyboard() {
                key_down(app, vk, repeat);
            } else if !pressed {
                // The spring's own keyup must fire even if egui grabbed
                // the keyboard mid-hold — a borrow that survives its
                // release is a latch by accident.
                app.spring_release(vk);
            }
            app.shell.on_key(vk, pressed, repeat);
            app.mark_dirty();
            flush_redraw(hwnd, app);
            0
        }

        WM_CHAR => {
            if app.text_editing() && !app.shell.wants_keyboard() {
                app.text_char(wparam as u16);
            } else {
                app.shell.on_char(wparam as u16);
            }
            app.mark_dirty();
            flush_redraw(hwnd, app);
            0
        }

        // Position the IME composition AND candidate windows at the caret,
        // then let the default composition UI run (WM_CHAR delivers the
        // committed text). plans/05 item 5 v1: START used to be the only
        // positioning moment, so a composition whose caret moved left the
        // window stranded — and the candidate list was never told anything,
        // parking itself over the text being typed. INLINE preview (the
        // composition string drawn underlined at the caret, no default UI)
        // is the later, bigger slice.
        WM_IME_STARTCOMPOSITION | WM_IME_COMPOSITION => {
            position_ime_at_caret(hwnd, &app);
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        // The pen left hover range / the mouse left the window: the brush
        // ring must not stay parked at its last position (owner report).
        WM_POINTERLEAVE | WM_MOUSELEAVE => {
            app.pointer_visible = false;
            app.mark_dirty();
            flush_redraw(hwnd, app);
            0
        }

        WM_DESTROY => {
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            note_geom_now(hwnd);
            persist_geom(app);
            // The sub tool in hand never had a switch to write it down, so
            // exiting IS its switch: this is what carries a size the user
            // dialled and then quit on into `ui.txt`. Lock-aware, like every
            // other switch (`store_current_props`).
            app.store_current_props();
            subtools::note_memory(app);
            app.layout.save_if_dirty();
            app.prefs.save_if_dirty();
            // The telemetry exit summary, while the App still exists —
            // `end_session` after the message loop only stamps the marker.
            app.diag.flush_composite_summary();
            // Drop the App (and the Renderer's surface) while the HWND is valid.
            drop(unsafe { Box::from_raw(p) });
            unsafe { PostQuitMessage(0) };
            0
        }

        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Point the IME's composition string and its CANDIDATE list at the caret
/// (plans/05 item 5, v1). Called at composition START and on every
/// WM_IME_COMPOSITION — a caret that moves mid-composition keeps its
/// windows with it. The candidate list sits one line under the caret so
/// it stops covering the text being typed; the OS nudges it for screen
/// edges itself. (Inline composition preview — the string drawn
/// underlined at the caret, no default UI — is the later, bigger slice.)
fn position_ime_at_caret(hwnd: HWND, app: &App) {
    let Some((x, y)) = app.ime_caret_client_px() else {
        return;
    };
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_null() {
            return;
        }
        let form = COMPOSITIONFORM {
            dwStyle: CFS_POINT,
            ptCurrentPos: POINT { x, y },
            rcArea: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
        };
        ImmSetCompositionWindow(himc, &form);
        let cand = CANDIDATEFORM {
            dwIndex: 0,
            dwStyle: CFS_CANDIDATEPOS,
            ptCurrentPos: POINT { x, y: y + 18 },
            rcArea: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
        };
        ImmSetCandidateWindow(himc, &cand);
        ImmReleaseContext(hwnd, himc);
    }
}

fn mouse_sample(x: i32, y: i32) -> PenSample {
    PenSample {
        x: x as f32,
        y: y as f32,
        pressure: MOUSE_PRESSURE,
        tilt_x: 0.0,
        tilt_y: 0.0,
        t_ms: now_ms(),
    }
}

/// Milliseconds since the first call. The pen carries its own timestamps
/// (`POINTER_INFO::dwTime`); the mouse path has none, and libmypaint *divides*
/// by the gap between samples — a constant `t_ms` makes every dtime ~0, which
/// pins `slow_tracking` smoothing (0.65 in pen.myb) at ~1.5% per sample and
/// leaves the brush crawling far behind the cursor.
fn now_ms() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}
