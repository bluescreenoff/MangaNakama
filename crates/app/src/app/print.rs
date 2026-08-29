//! File ▸ Print… and View ▸ Print size — the two ends of "see the page at
//! its real physical size" (pro-workflow audit finding 8).
//!
//! Until this module the app could not print at all, and there was no way to
//! put a page on screen at true size either. Both habits it serves are print
//! habits: judging tone density and 級数 legibility at the size the paper
//! will actually be, and the red-pen mark-up revision cycle that needs paper
//! in a hand.
//!
//! **The pixels are the EXPORT pixels.** `print_job` renders through
//! `pages::render_offscreen_drafts_off` (drafts hidden, paper forced visible)
//! and then `mn_core::export::finish_image` — the same door
//! `AppCmd::ExportAllPagesPath` walks. A mono work therefore prints the
//! THRESHOLDED page, which is the same decision the Proof JPEG took
//! (7626499): the page you see is the page that prints. A second compositor
//! for the printer would be a second truth.
//!
//! **Deferred, deliberately.** One page per job (the ACTIVE page) — spread
//! splitting, page ranges and N-up all belong to a print run and are a
//! separate round; crop marks on the printed sheet likewise (the export
//! crop knobs exist, the printer does not read them yet). Printing goes
//! through GDI's `StretchDIBits`, so a 600 dpi B4 page is handed to the
//! driver as one bitmap rather than banded — fine on the desktop drivers
//! this is for, and the honest fix if it ever OOMs is banding.

use crate::app::App;

use windows_sys::Win32::Foundation::{GlobalFree, HGLOBAL, HWND};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteDC, GetDeviceCaps, HALFTONE, HDC,
    HORZRES, LOGPIXELSX, LOGPIXELSY, SRCCOPY, SetStretchBltMode, StretchDIBits, VERTRES,
};
use windows_sys::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
use windows_sys::Win32::UI::Controls::Dialogs::{
    CommDlgExtendedError, PD_NOPAGENUMS, PD_NOSELECTION, PD_RETURNDC,
    PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW, PrintDlgW,
};

// --- the size policy ------------------------------------------------------

/// How a page is laid onto the sheet. CSP's three, same names, same meanings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PrintSize {
    /// Page millimetres onto paper millimetres at the printer's own dpi.
    /// The only one of the three that answers "is this tone readable at
    /// print size", which is the whole reason the feature exists — hence
    /// the default. Meaningless without a page dpi (see [`place`]).
    #[default]
    Actual,
    /// Fit inside the printable area, physical aspect preserved. The
    /// "just show me the whole page on A4" answer.
    Fit,
    /// One document pixel = one printer pixel. What a pixel canvas can
    /// honestly do, and the check for whether a 600 dpi page really is
    /// 600 dpi worth of line.
    Pixel,
}

impl PrintSize {
    /// The menu/dialog order, and the order the prefs key round-trips in.
    pub const ALL: [PrintSize; 3] = [PrintSize::Actual, PrintSize::Fit, PrintSize::Pixel];

    /// The `prefs.txt` spelling. Shipped API — never change one of these.
    pub fn key(self) -> &'static str {
        match self {
            PrintSize::Actual => "actual",
            PrintSize::Fit => "fit",
            PrintSize::Pixel => "pixel",
        }
    }

    /// Read a stored key. `None` for anything this build does not know —
    /// the caller keeps the default and `prefs.rs` keeps the FILE's word,
    /// the same rule the `theme` key follows.
    pub fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == s.trim())
    }

    /// The stored preference, resolved. An unknown value paints the
    /// default rather than wedging the dialog.
    pub fn from_pref(s: &str) -> Self {
        Self::from_key(s).unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            PrintSize::Actual => "Actual size (原寸)",
            PrintSize::Fit => "Scale to paper",
            PrintSize::Pixel => "Pixel size (等倍)",
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            PrintSize::Actual => {
                "The page's millimetres onto the paper's millimetres. A page \
                 bigger than the sheet is centred and clipped — that is the \
                 honest result, not a failure."
            }
            PrintSize::Fit => {
                "Shrink or grow the page until the whole of it sits inside the \
                 printable area. Tone density and type size are no longer true."
            }
            PrintSize::Pixel => {
                "One document pixel per printer dot. A 600 dpi page on a 600 \
                 dpi printer is the same thing as actual size; on a 300 dpi \
                 printer it comes out twice as big."
            }
        }
    }
}

/// Where the page lands on the sheet, printer pixels, origin at the top-left
/// of the PRINTABLE area (which is where GDI puts the DC's origin). `x`/`y`
/// go negative when the page is larger than the sheet: that is the clip, and
/// `StretchDIBits` performs it against the DC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// The policy that actually ran. Differs from the one asked for only in
    /// the one case that has no answer: Actual size on a canvas with no dpi.
    pub used: PrintSize,
}

/// Lay `page_px` onto a sheet.
///
/// `printable_px` is the printable AREA (`HORZRES`/`VERTRES`), not the paper:
/// every printer has a hardware margin, and centring against the paper would
/// put the page off-centre on every sheet by half that margin.
///
/// **Actual size needs `page_dpi`.** A pixel canvas has no physical size at
/// all, so there is nothing to map onto millimetres; asking anyway falls back
/// to `Pixel` and says so through `Placement::used` (the dialog greys the
/// option out before it gets here — this is the belt to that's braces).
///
/// **Aspect is PHYSICAL aspect.** Printers with different x and y resolution
/// exist; scaling by pixel counts would print such a page stretched.
pub fn place(
    page_px: (u32, u32),
    page_dpi: Option<u32>,
    printer_dpi: (u32, u32),
    printable_px: (u32, u32),
    policy: PrintSize,
) -> Placement {
    let pw = page_px.0.max(1) as f64;
    let ph = page_px.1.max(1) as f64;
    let dx = printer_dpi.0.max(1) as f64;
    let dy = printer_dpi.1.max(1) as f64;
    let aw = printable_px.0.max(1) as f64;
    let ah = printable_px.1.max(1) as f64;
    let page_dpi = page_dpi.filter(|d| *d > 0);

    let used = match policy {
        PrintSize::Actual if page_dpi.is_none() => PrintSize::Pixel,
        p => p,
    };
    let (tw, th) = match used {
        PrintSize::Pixel => (pw, ph),
        PrintSize::Actual => {
            let d = page_dpi.unwrap_or(1) as f64;
            (pw * dx / d, ph * dy / d)
        }
        PrintSize::Fit => {
            // The page's size in "document pixels × printer dpi" — a unit
            // that cancels, so the ratio below is the physical one.
            let (iw, ih) = (pw * dx, ph * dy);
            let s = (aw / iw).min(ah / ih);
            (iw * s, ih * s)
        }
    };
    let w = tw.round().max(1.0);
    let h = th.round().max(1.0);
    Placement {
        x: ((aw - w) / 2.0).round() as i32,
        y: ((ah - h) / 2.0).round() as i32,
        w: w as i32,
        h: h as i32,
        used,
    }
}

/// View ▸ Print size: the viewport zoom that puts one page millimetre on one
/// screen millimetre.
///
/// `None` is a REFUSAL, not a zero: a canvas measured in pixels has no
/// millimetres, and picking some number anyway would be a lie told at a size
/// the owner is about to trust.
///
/// The screen's dpi is what Windows reports for the monitor
/// (`GetDpiForWindow`), divided back out of the UI-size preference — the
/// chrome scale must not move the artwork's physical size.
pub fn print_zoom(work_dpi: Option<u32>, monitor_dpi: f32) -> Option<f32> {
    let d = work_dpi.filter(|d| *d > 0)? as f32;
    (monitor_dpi.is_finite() && monitor_dpi > 0.0).then_some(monitor_dpi / d)
}

/// RGBA → the bottom-up-capable 32-bit DIB GDI wants: B, G, R, unused.
///
/// Alpha is composited onto paper white with the same arithmetic
/// `export::save_finished` uses for JPEG, for the same reason — a DIB has no
/// alpha, and handing GDI premultiplied ink would print black on black.
pub fn to_bgrx(img: &image::RgbaImage) -> Vec<u8> {
    let mut out = vec![0u8; img.width() as usize * img.height() as usize * 4];
    for (o, p) in out.chunks_exact_mut(4).zip(img.pixels()) {
        let a = p.0[3] as u32;
        let over = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255).min(255) as u8;
        o[0] = over(p.0[2]);
        o[1] = over(p.0[1]);
        o[2] = over(p.0[0]);
        o[3] = 255;
    }
    out
}

// --- the job --------------------------------------------------------------

/// One composited page on its way to a printer. Built inside the app (it
/// needs the renderer), printed outside it (`PrintDlgW` pumps the message
/// queue, which re-enters the wndproc — docs/ARCHITECTURE.md).
pub struct Job {
    pub bgrx: Vec<u8>,
    pub w: u32,
    pub h: u32,
    /// The WORK's own print resolution; `None` = a pixel canvas.
    pub page_dpi: Option<u32>,
    pub size: PrintSize,
    /// What the print queue calls the job.
    pub doc_name: String,
}

impl App {
    /// The monitor's dpi as Windows reports it, with the UI-size preference
    /// divided back out (`shell.ppp` is window dpi × `ui_scale`, and the
    /// chrome multiplier must not change what a millimetre is).
    pub fn monitor_dpi(&self) -> f32 {
        let s = if self.prefs.ui_scale.is_finite() && self.prefs.ui_scale > 0.0 {
            self.prefs.ui_scale
        } else {
            1.0
        };
        self.shell.ppp / s * 96.0
    }

    /// Composite the ACTIVE page for the printer, through the export door.
    ///
    /// Drafts are hidden and the paper is forced visible exactly as
    /// `ExportPngPath` does — the transparency checker is screen furniture
    /// and "hide the paper" is a hole check, neither of which may reach a
    /// sheet of paper. The finish then reduces to the work's expression, so
    /// a mono work prints the thresholded page.
    pub fn print_job(&mut self) -> Result<Job, String> {
        self.commit_text_edit();
        self.refresh_tones();
        let (w, h) = self.doc.size;
        if w == 0 || h == 0 {
            return Err("nothing to print: the page is empty".into());
        }
        self.renderer.set_paper_override(Some(mn_core::Paper {
            visible: true,
            ..self.doc.paper
        }));
        let img = {
            let Self { renderer, doc, .. } = self;
            super::pages::render_offscreen_drafts_off(renderer, doc, w, h)
        };
        self.renderer.set_paper_override(None);
        let colour = match self.expression {
            mn_core::Expression::Mono => mn_core::LayerExpression::Mono,
            mn_core::Expression::Colour => mn_core::LayerExpression::Colour,
        };
        // Scale 1.0: the printer's own scaling happens in `place`, in
        // printer pixels, where it belongs. Comic is the export default and
        // only bites on a mono finish.
        let img = mn_core::export::finish_image(
            img,
            1.0,
            colour,
            mn_core::export::Resample::Comic,
        );
        let stem = if self.story.trim().is_empty() {
            "MangaNakama".to_owned()
        } else {
            self.story.trim().to_owned()
        };
        Ok(Job {
            bgrx: to_bgrx(&img),
            w: img.width(),
            h: img.height(),
            page_dpi: self.work_dpi(),
            size: PrintSize::from_pref(&self.prefs.print_size),
            doc_name: format!("{stem} p{}", self.page_index + 1),
        })
    }
}

// --- Win32 ----------------------------------------------------------------

/// `DeleteDC` on ANY exit path, including the early returns a failing driver
/// takes. A leaked printer DC is a print job the spooler never finishes.
struct Dc(HDC);
impl Drop for Dc {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { DeleteDC(self.0) };
        }
    }
}

/// `PrintDlgW` allocates `hDevMode`/`hDevNames` and hands ownership over —
/// even when the user cancels, and even when it fails.
struct Global(HGLOBAL);
impl Drop for Global {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { GlobalFree(self.0) };
        }
    }
}

/// `StretchDIBits`' documented failure value. It is declared `i32` here, so
/// the `GDI_ERROR` bit pattern reads as -1; a driver returning 0 scan lines
/// is NOT an error and must not be reported as one.
const GDI_ERROR_I32: i32 = -1;

/// Show the Windows print dialog and print `job`. `Ok` carries the status
/// line, including the "you cancelled" one — a cancel is a normal outcome,
/// not an error, and must not paint the status bar red.
///
/// MUST be called with no `&mut App` alive: the dialog runs its own modal
/// message loop, which re-enters the wndproc.
pub fn run(hwnd: HWND, job: &Job) -> Result<String, String> {
    unsafe {
        let mut pd: PRINTDLGW = std::mem::zeroed();
        pd.lStructSize = size_of::<PRINTDLGW>() as u32;
        pd.hwndOwner = hwnd;
        // No page numbers and no selection: this job is one page, and
        // offering a range we do not honour would be a lie in a dialog.
        // USEDEVMODECOPIESANDCOLLATE lets the DRIVER do copies (nCopies
        // comes back 1 and the pages come out of the machine N times).
        pd.Flags = PD_RETURNDC | PD_NOPAGENUMS | PD_NOSELECTION | PD_USEDEVMODECOPIESANDCOLLATE;
        pd.nCopies = 1;
        let ok = PrintDlgW(&mut pd);
        // Ours whatever happened — freed before any early return below.
        let _dev_mode = Global(pd.hDevMode);
        let _dev_names = Global(pd.hDevNames);
        let dc = Dc(pd.hDC);
        if ok == 0 {
            // 0 with no extended error is the user pressing Cancel.
            let e = CommDlgExtendedError();
            return if e == 0 {
                Ok("print cancelled".to_owned())
            } else {
                Err(format!(
                    "the print dialog failed (0x{e:04x}) — is a printer installed?"
                ))
            };
        }
        if dc.0.is_null() {
            return Err("the printer driver returned no device context".into());
        }
        let printer_dpi = (
            GetDeviceCaps(dc.0, LOGPIXELSX as i32).max(1) as u32,
            GetDeviceCaps(dc.0, LOGPIXELSY as i32).max(1) as u32,
        );
        let printable = (
            GetDeviceCaps(dc.0, HORZRES as i32).max(1) as u32,
            GetDeviceCaps(dc.0, VERTRES as i32).max(1) as u32,
        );
        let p = place(
            (job.w, job.h),
            job.page_dpi,
            printer_dpi,
            printable,
            job.size,
        );

        let name = crate::win32::wide(&job.doc_name);
        let mut di: DOCINFOW = std::mem::zeroed();
        di.cbSize = size_of::<DOCINFOW>() as i32;
        di.lpszDocName = name.as_ptr();
        if StartDocW(dc.0, &di) <= 0 {
            return Err("the printer refused the job (StartDoc)".into());
        }
        if StartPage(dc.0) <= 0 {
            AbortDoc(dc.0);
            return Err("the printer refused the page (StartPage)".into());
        }
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = job.w as i32;
        // Negative height = top-down rows, which is the order every image
        // in this program is in. Flipping the pixels instead would cost a
        // full copy of a print-resolution page.
        bmi.bmiHeader.biHeight = -(job.h as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        // HALFTONE, not the default COLORONCOLOR: shrinking 1-bit line art
        // by dropping rows is exactly the hairline-eating bug the comic
        // downscale exists to avoid (7626499). Averaging to grey and
        // letting the driver screen it keeps the line ON the page.
        SetStretchBltMode(dc.0, HALFTONE);
        let blit = StretchDIBits(
            dc.0,
            p.x,
            p.y,
            p.w,
            p.h,
            0,
            0,
            job.w as i32,
            job.h as i32,
            job.bgrx.as_ptr().cast(),
            &bmi,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
        if blit == GDI_ERROR_I32 {
            AbortDoc(dc.0);
            return Err("the printer driver rejected the page image".into());
        }
        if EndPage(dc.0) <= 0 {
            AbortDoc(dc.0);
            return Err("the page did not finish (EndPage)".into());
        }
        if EndDoc(dc.0) <= 0 {
            return Err("the job did not finish (EndDoc)".into());
        }
        let note = if p.used == job.size {
            String::new()
        } else {
            format!(" (no page dpi — printed at {})", p.used.label())
        };
        Ok(format!(
            "printed {}×{} px at {} — {}×{} printer px on a {}×{} area{note}",
            job.w, job.h, p.used.label(), p.w, p.h, printable.0, printable.1
        ))
    }
}

// --- the pre-dialog -------------------------------------------------------

/// The size policy is picked HERE, before the Windows print dialog, because
/// the Windows one has nowhere to put it — and it is the only decision in
/// this feature that a manga page actually turns on.
pub(crate) fn print_window(ctx: &egui::Context, app: &mut App) {
    if !app.print_open {
        return;
    }
    let mut open = true;
    let mut go = false;
    let mut cancel = false;
    let dpi = app.work_dpi();
    let (w, h) = app.doc.size;
    egui::Window::new("Print")
        .open(&mut open)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -30.0))
        .show(ctx, |ui| {
            ui.label(match dpi {
                Some(d) => format!(
                    "{w} × {h} px at {d} dpi — {:.1} × {:.1} mm",
                    w as f32 * 25.4 / d as f32,
                    h as f32 * 25.4 / d as f32
                ),
                None => format!("{w} × {h} px — this canvas has no dpi"),
            });
            ui.add_space(6.0);
            let cur = PrintSize::from_pref(&app.prefs.print_size);
            let mut picked = None;
            for p in PrintSize::ALL {
                // Actual size on a pixel canvas is not a preference anyone
                // can hold: there are no millimetres to be actual about.
                let enabled = p != PrintSize::Actual || dpi.is_some();
                if ui
                    .add_enabled(enabled, egui::RadioButton::new(cur == p, p.label()))
                    .on_hover_text(p.note())
                    .on_disabled_hover_text(
                        "This canvas is measured in pixels only, so there is no \
                         page dpi to be actual size against. Give the work a page \
                         setup (File ▸ Work settings) or print at Pixel size.",
                    )
                    .clicked()
                {
                    picked = Some(p);
                }
            }
            if let Some(p) = picked {
                app.prefs.print_size = p.key().to_owned();
                app.prefs.mark_dirty();
            }
            if cur == PrintSize::Actual && dpi.is_none() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Actual size needs a page dpi — this page will print at \
                         Pixel size instead.",
                    )
                    .weak()
                    .size(11.0),
                );
            }
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "The printer gets the EXPORT image: drafts hidden, paper white, \
                     and a mono work thresholded — the page you see is the page that \
                     prints. One page per job (the active one).",
                )
                .weak()
                .size(11.0),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .button("Print…")
                    .on_hover_text("Opens the Windows printer dialog.")
                    .clicked()
                {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    app.print_open = open && !go && !cancel;
    if go {
        app.push_cmd(crate::cmd::AppCmd::PrintGo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×3 inch page (1200×1800 at 600 dpi) printed actual size on a 300
    /// dpi printer must occupy 2×3 inches of paper — 600×900 printer px —
    /// and sit in the middle of the printable area.
    #[test]
    fn actual_size_is_page_mm_on_paper_mm() {
        let p = place((1200, 1800), Some(600), (300, 300), (2550, 3300), PrintSize::Actual);
        assert_eq!((p.w, p.h), (600, 900), "2in × 3in at 300 dpi");
        assert_eq!(
            (p.x, p.y),
            (975, 1200),
            "centred in the printable area, not the paper"
        );
        assert_eq!(p.used, PrintSize::Actual);
    }

    /// The one case actual size cannot answer: no page dpi. It must fall
    /// back to Pixel and SAY so, not invent 96 or 600.
    #[test]
    fn actual_size_falls_back_to_pixels_without_a_page_dpi() {
        let none = place((800, 600), None, (300, 300), (2550, 3300), PrintSize::Actual);
        assert_eq!(none.used, PrintSize::Pixel, "the fallback is reported");
        assert_eq!((none.w, none.h), (800, 600), "…and it is 1:1");
        assert_eq!((none.x, none.y), (875, 1350));
        // A stored `dpi = 0` is the same nothing — `PageSetup::dpi` uses 0
        // for "no dpi" and it must not divide by zero into infinity.
        let zero = place((800, 600), Some(0), (300, 300), (2550, 3300), PrintSize::Actual);
        assert_eq!(zero, none);
    }

    /// Fit grows or shrinks until the whole page is inside, and keeps the
    /// aspect: 1200×1800 (2:3) into 2550×3300 fills the height.
    #[test]
    fn scale_to_paper_fits_inside_and_keeps_the_aspect() {
        let p = place((1200, 1800), Some(600), (300, 300), (2550, 3300), PrintSize::Fit);
        assert_eq!((p.w, p.h), (2200, 3300), "height-bound, 2:3 preserved");
        assert!(p.w <= 2550 && p.h <= 3300, "inside the printable area");
        assert_eq!((p.x, p.y), (175, 0), "centred on the bound axis");
        assert_eq!(p.used, PrintSize::Fit);
        // Fit needs no page dpi at all — the ratio it preserves is the
        // page's own, and a pixel canvas has one of those.
        let no_dpi = place((1200, 1800), None, (300, 300), (2550, 3300), PrintSize::Fit);
        assert_eq!(no_dpi, p);
    }

    /// A printer whose x and y resolutions differ must not print the page
    /// stretched: the aspect that is preserved is the PHYSICAL one.
    #[test]
    fn anisotropic_printer_dpi_keeps_the_physical_aspect() {
        // A square 2in × 2in page.
        let a = place((1200, 1200), Some(600), (600, 300), (2400, 2400), PrintSize::Actual);
        assert_eq!((a.w, a.h), (1200, 600));
        assert_eq!(a.w as f32 / 600.0, a.h as f32 / 300.0, "2 inches each way");

        let f = place((1200, 1200), Some(600), (600, 300), (1200, 1200), PrintSize::Fit);
        assert_eq!((f.w, f.h), (1200, 600));
        assert_eq!(f.w as f32 / 600.0, f.h as f32 / 300.0, "still square on paper");
        assert!(f.w <= 1200 && f.h <= 1200);
    }

    /// Pixel size is 1:1 with the printer's dots whatever either dpi says.
    #[test]
    fn pixel_size_is_one_doc_pixel_per_printer_pixel() {
        let p = place((1000, 500), Some(600), (300, 300), (2550, 3300), PrintSize::Pixel);
        assert_eq!((p.w, p.h), (1000, 500));
        assert_eq!((p.x, p.y), (775, 1400));
        assert_eq!(p.used, PrintSize::Pixel);
    }

    /// A B4 600 dpi page at actual size on a 600 dpi A4 printer does not
    /// fit. The answer is a centred, clipped page — negative origins, which
    /// is exactly what StretchDIBits needs to clip symmetrically — not a
    /// silent shrink that would destroy the one thing actual size is for.
    #[test]
    fn a_page_bigger_than_the_paper_centres_and_clips() {
        let p = place((6071, 8598), Some(600), (600, 600), (4800, 6600), PrintSize::Actual);
        assert_eq!((p.w, p.h), (6071, 8598), "not shrunk");
        assert_eq!((p.x, p.y), (-636, -999), "overhang split evenly both ways");
        assert_eq!(p.x + p.w - 4800, -p.x - 1, "…within a pixel of symmetric");
    }

    /// Degenerate inputs produce a usable rect instead of a divide-by-zero
    /// or a zero-area blit.
    #[test]
    fn degenerate_sizes_never_produce_an_empty_or_infinite_rect() {
        for policy in PrintSize::ALL {
            let p = place((0, 0), Some(0), (0, 0), (0, 0), policy);
            assert!(p.w >= 1 && p.h >= 1, "{policy:?} -> {p:?}");
        }
    }

    /// View ▸ Print size: the factor is monitor dpi over work dpi, and a
    /// canvas with no dpi is REFUSED rather than guessed at.
    #[test]
    fn print_size_zoom_is_monitor_over_work_dpi_and_refuses_a_pixel_canvas() {
        // A 600 dpi page on a 96 dpi display is shown at 16%.
        assert_eq!(print_zoom(Some(600), 96.0), Some(0.16));
        // 150% Windows scaling: 144 dpi, so the same page grows to 24%.
        assert_eq!(print_zoom(Some(600), 144.0), Some(0.24));
        // A 96 dpi "web" page is 1:1 on a 96 dpi screen.
        assert_eq!(print_zoom(Some(96), 96.0), Some(1.0));
        // The refusals.
        assert_eq!(print_zoom(None, 96.0), None, "pixel canvas: no millimetres");
        assert_eq!(print_zoom(Some(0), 96.0), None, "0 dpi is the same nothing");
        assert_eq!(print_zoom(Some(600), 0.0), None);
        assert_eq!(print_zoom(Some(600), f32::NAN), None);
    }

    /// The DIB conversion: channel order swapped, alpha composited onto
    /// paper white (never dropped — that would print ink as black on black).
    #[test]
    fn bgrx_swaps_channels_and_composites_onto_paper_white() {
        let mut img = image::RgbaImage::new(3, 1);
        img.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 0, 0]));
        img.put_pixel(2, 0, image::Rgba([0, 0, 0, 128]));
        let b = to_bgrx(&img);
        assert_eq!(b.len(), 12);
        assert_eq!(&b[0..4], &[30, 20, 10, 255], "B, G, R, unused");
        assert_eq!(&b[4..8], &[255, 255, 255, 255], "transparent = paper");
        assert_eq!(&b[8..12], &[127, 127, 127, 255], "half-covered ink");
    }

    /// The prefs spelling round-trips and an unknown one resolves to the
    /// default instead of wedging the dialog.
    #[test]
    fn print_size_keys_round_trip() {
        for p in PrintSize::ALL {
            assert_eq!(PrintSize::from_key(p.key()), Some(p));
            assert_eq!(PrintSize::from_pref(p.key()), p);
        }
        assert_eq!(PrintSize::from_key("from-the-future"), None);
        assert_eq!(PrintSize::from_pref("from-the-future"), PrintSize::Actual);
        assert_eq!(PrintSize::default(), PrintSize::Actual);
    }
}
